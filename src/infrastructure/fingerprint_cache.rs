use crate::infrastructure::file_stamp::FileStamp;
use anyhow::Result;
use chrono::Utc;
use sqlx::SqlitePool;
use std::{future::Future, path::Path};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FingerprintSource {
    Cache,
    Generated,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FingerprintResult {
    pub fingerprint: String,
    pub duration: f64,
    pub source: FingerprintSource,
}

pub async fn get_or_calculate<F, Fut>(
    pool: &SqlitePool,
    path: &Path,
    calculate: F,
) -> Result<FingerprintResult>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<(String, f64)>>,
{
    let stamp = FileStamp::of(path).await?;
    let path_text = path.to_string_lossy();
    if let Some((fingerprint, duration)) = get(pool, path_text.as_ref(), stamp).await? {
        return Ok(FingerprintResult {
            fingerprint,
            duration,
            source: FingerprintSource::Cache,
        });
    }

    let (fingerprint, duration) = calculate().await?;
    put(pool, path_text.as_ref(), stamp, &fingerprint, duration).await?;
    Ok(FingerprintResult {
        fingerprint,
        duration,
        source: FingerprintSource::Generated,
    })
}

pub async fn cached(pool: &SqlitePool, path: &Path) -> Result<Option<FingerprintResult>> {
    let stamp = FileStamp::of(path).await?;
    Ok(get(pool, path.to_string_lossy().as_ref(), stamp)
        .await?
        .map(|(fingerprint, duration)| FingerprintResult {
            fingerprint,
            duration,
            source: FingerprintSource::Cache,
        }))
}

/// `file_mtime` holds nanoseconds, like the other file caches.
async fn get(pool: &SqlitePool, path: &str, stamp: FileStamp) -> Result<Option<(String, f64)>> {
    Ok(sqlx::query_as(
        "SELECT fingerprint,duration FROM fingerprint_cache WHERE path=? AND file_size=? AND file_mtime=?",
    )
    .bind(path)
    .bind(stamp.size)
    .bind(stamp.mtime_ns)
    .fetch_optional(pool)
    .await?)
}

async fn put(
    pool: &SqlitePool,
    path: &str,
    stamp: FileStamp,
    fingerprint: &str,
    duration: f64,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO fingerprint_cache(path,file_size,file_mtime,fingerprint,duration,updated_at) VALUES(?,?,?,?,?,?) \
         ON CONFLICT(path) DO UPDATE SET file_size=excluded.file_size,file_mtime=excluded.file_mtime,fingerprint=excluded.fingerprint,duration=excluded.duration,updated_at=excluded.updated_at",
    )
    .bind(path)
    .bind(stamp.size)
    .bind(stamp.mtime_ns)
    .bind(fingerprint)
    .bind(duration)
    .bind(Utc::now().to_rfc3339())
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    async fn test_pool() -> SqlitePool {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.sqlite");
        let pool = crate::infrastructure::db::connect(path.to_str().unwrap())
            .await
            .unwrap();
        std::mem::forget(dir);
        pool
    }

    #[tokio::test]
    async fn cache_miss_calculates_and_unchanged_file_reuses() {
        let pool = test_pool().await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("song.mp3");
        tokio::fs::write(&path, b"audio").await.unwrap();
        let calls = Arc::new(AtomicUsize::new(0));

        let first = get_or_calculate(&pool, &path, {
            let calls = calls.clone();
            || async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(("fp1".into(), 12.0))
            }
        })
        .await
        .unwrap();
        let second = get_or_calculate(&pool, &path, {
            let calls = calls.clone();
            || async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(("fp2".into(), 13.0))
            }
        })
        .await
        .unwrap();

        assert_eq!(first.source, FingerprintSource::Generated);
        assert_eq!(second.source, FingerprintSource::Cache);
        assert_eq!(second.fingerprint, "fp1");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn changed_file_regenerates() {
        let pool = test_pool().await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("song.mp3");
        tokio::fs::write(&path, b"audio").await.unwrap();
        get_or_calculate(&pool, &path, || async { Ok(("fp1".into(), 12.0)) })
            .await
            .unwrap();

        tokio::fs::write(&path, b"changed audio").await.unwrap();
        let result = get_or_calculate(&pool, &path, || async { Ok(("fp2".into(), 14.0)) })
            .await
            .unwrap();

        assert_eq!(result.source, FingerprintSource::Generated);
        assert_eq!(result.fingerprint, "fp2");
    }
}
