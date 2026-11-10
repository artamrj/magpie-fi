//! Catalog answers kept in SQLite so the same lookup is not repeated while it
//! is fresh. `expires_at` is RFC 3339 UTC with whole seconds
//! (`2026-01-02T03:04:05Z`), so it also compares correctly as text.

use anyhow::Result;
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use std::future::Future;

/// `time` in the format stored in `provider_cache.expires_at`.
pub fn timestamp(time: DateTime<Utc>) -> String {
    time.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// The cached answer, or `fetch()`'s answer, which is then kept for `ttl`.
/// Failed fetches are not cached.
pub async fn cached<F, Fut>(
    pool: &SqlitePool,
    provider: &str,
    key: &str,
    ttl: Duration,
    fetch: F,
) -> Result<Value>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<Value>>,
{
    if let Some(value) = get(pool, provider, key).await? {
        return Ok(value);
    }
    let value = fetch().await?;
    put(pool, provider, key, &value, Utc::now() + ttl).await?;
    Ok(value)
}

pub async fn get(pool: &SqlitePool, provider: &str, key: &str) -> Result<Option<Value>> {
    let row: Option<String> = sqlx::query_scalar(
        "SELECT response_json FROM provider_cache WHERE provider=? AND cache_key=? AND expires_at>?",
    )
    .bind(provider)
    .bind(key)
    .bind(timestamp(Utc::now()))
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|json| serde_json::from_str(&json)).transpose()?)
}

pub async fn put(
    pool: &SqlitePool,
    provider: &str,
    key: &str,
    response: &Value,
    expires_at: DateTime<Utc>,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO provider_cache(provider,cache_key,response_json,expires_at) VALUES(?,?,?,?) \
         ON CONFLICT(provider,cache_key) DO UPDATE SET response_json=excluded.response_json, expires_at=excluded.expires_at",
    )
    .bind(provider)
    .bind(key)
    .bind(serde_json::to_string(response)?)
    .bind(timestamp(expires_at))
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn delete(pool: &SqlitePool, provider: &str, key: &str) -> Result<()> {
    sqlx::query("DELETE FROM provider_cache WHERE provider=? AND cache_key=?")
        .bind(provider)
        .bind(key)
        .execute(pool)
        .await?;
    Ok(())
}

pub fn fingerprint_key(fingerprint: &str) -> String {
    format!(
        "fingerprint:{}",
        hex::encode(Sha256::digest(fingerprint.trim()))
    )
}

pub fn recording_key(mbid: &str) -> String {
    format!("recording:{}", mbid.trim().to_ascii_lowercase())
}

/// Case and spacing do not change a catalog search.
pub fn search_key(query: &str) -> String {
    format!(
        "search:{}",
        query
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_ascii_lowercase()
    )
}

pub fn release_key(release_id: &str) -> String {
    format!("release:{}", release_id.trim().to_ascii_lowercase())
}

/// URLs are case-sensitive, so they are kept as they are.
pub fn url_key(url: &str) -> String {
    format!("url:{}", url.trim())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    async fn test_pool() -> (tempfile::TempDir, SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.sqlite");
        let pool = crate::infrastructure::db::connect(path.to_str().unwrap())
            .await
            .unwrap();
        (dir, pool)
    }

    #[tokio::test]
    async fn provider_cache_hits_and_overwrites() {
        let (_dir, pool) = test_pool().await;
        let tomorrow = Utc::now() + Duration::days(1);
        put(
            &pool,
            "musicbrainz",
            "recording:abc",
            &serde_json::json!({"title":"old"}),
            tomorrow,
        )
        .await
        .unwrap();
        put(
            &pool,
            "musicbrainz",
            "recording:abc",
            &serde_json::json!({"title":"new"}),
            tomorrow,
        )
        .await
        .unwrap();
        let hit = get(&pool, "musicbrainz", "recording:abc")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(hit["title"], "new");
    }

    #[tokio::test]
    async fn provider_cache_ignores_expired_rows() {
        let (_dir, pool) = test_pool().await;
        let past = Utc::now() - Duration::seconds(1);
        put(
            &pool,
            "musicbrainz",
            "recording:old",
            &serde_json::json!({}),
            past,
        )
        .await
        .unwrap();
        assert!(
            get(&pool, "musicbrainz", "recording:old")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn cached_fetches_once_and_never_caches_failures() {
        let (_dir, pool) = test_pool().await;
        let calls = &AtomicUsize::new(0);
        let fetch = move || async move {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(serde_json::json!({"n": 1}))
        };
        assert!(
            cached(&pool, "p", "fail", Duration::days(1), || async {
                anyhow::bail!("offline")
            })
            .await
            .is_err()
        );
        assert!(get(&pool, "p", "fail").await.unwrap().is_none());
        for _ in 0..2 {
            let value = cached(&pool, "p", "k", Duration::days(1), fetch)
                .await
                .unwrap();
            assert_eq!(value["n"], 1);
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn timestamps_sort_as_text() {
        let earlier = timestamp(Utc::now());
        let later = timestamp(Utc::now() + Duration::seconds(1));
        assert!(earlier < later);
        assert!(earlier.ends_with('Z') && earlier.len() == 20);
    }

    #[test]
    fn cache_keys_are_stable() {
        assert_eq!(recording_key(" ABC "), "recording:abc");
        assert_eq!(
            search_key("Recording:Song   AND Artist:Me"),
            "search:recording:song and artist:me"
        );
        assert_eq!(release_key(" REL "), "release:rel");
        assert!(fingerprint_key("abc").starts_with("fingerprint:"));
        assert_eq!(
            url_key("https://x.test/Cover.JPG"),
            "url:https://x.test/Cover.JPG"
        );
    }
}
