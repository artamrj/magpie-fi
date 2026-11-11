use anyhow::{Context, Result};
use chrono::{DateTime, Local, Utc};
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};
use std::{path::Path, str::FromStr, time::Duration};

const DAILY_CACHE_CLEANUP_KEY: &str = "last_disposable_cache_cleanup";
const MEDIA_CACHE_LIMIT_BYTES: u64 = 100 * 1024 * 1024;
static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

pub mod candidates;
pub mod tracks;

pub async fn connect(path: &str) -> Result<SqlitePool> {
    if let Some(parent) = Path::new(path).parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let url = format!("sqlite://{path}?mode=rwc");
    let options = SqliteConnectOptions::from_str(&url)?
        .create_if_missing(true)
        .foreign_keys(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(Duration::from_secs(30));
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .acquire_timeout(Duration::from_secs(30))
        .connect_with(options)
        .await?;
    MIGRATOR
        .run(&pool)
        .await
        .context("database migration failed")?;
    restrict_database_permissions(path).await?;
    Ok(pool)
}

#[cfg(unix)]
async fn restrict_database_permissions(path: &str) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .await
        .with_context(|| format!("could not protect database file {path}"))?;
    Ok(())
}

#[cfg(not(unix))]
async fn restrict_database_permissions(_path: &str) -> Result<()> {
    Ok(())
}

/// Startup cleanup: work interrupted by a restart is marked failed and expired
/// provider responses are removed.
pub async fn cleanup(pool: &SqlitePool) -> Result<()> {
    sqlx::query("UPDATE tracks SET stage='failed',status='provider_error',stage_message='Interrupted by restart',error=COALESCE(error,'Interrupted by backend restart') WHERE status='processing'")
        .execute(pool).await?;
    delete_expired_provider_cache(pool).await?;
    Ok(())
}

/// Provider rows store `expires_at` as RFC 3339 UTC, so they compare as text
/// against an RFC 3339 "now".
async fn delete_expired_provider_cache(pool: &SqlitePool) -> Result<u64> {
    Ok(
        sqlx::query("DELETE FROM provider_cache WHERE expires_at <= ?")
            .bind(Utc::now().to_rfc3339())
            .execute(pool)
            .await?
            .rows_affected(),
    )
}

/// Once per local calendar day: drops expired provider responses and keeps
/// media-analysis caches under 100 MiB. Workflow data is never touched.
pub async fn run_daily_cache_cleanup_if_due(pool: &SqlitePool) -> Result<bool> {
    let last_cleanup: Option<String> =
        sqlx::query_scalar("SELECT value FROM maintenance WHERE key=?")
            .bind(DAILY_CACHE_CLEANUP_KEY)
            .fetch_optional(pool)
            .await?;
    let now = Utc::now();
    let today = now.with_timezone(&Local).date_naive();
    let cleanup_due = last_cleanup
        .as_deref()
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .is_none_or(|last| last.with_timezone(&Local).date_naive() < today);
    if !cleanup_due {
        return Ok(false);
    }

    let provider_rows_removed = delete_expired_provider_cache(pool).await?;
    let media_rows_removed =
        enforce_media_cache_limit_with_limit(pool, MEDIA_CACHE_LIMIT_BYTES).await?;
    sqlx::query(
        "INSERT INTO maintenance(key,value) VALUES(?,?) \
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
    )
    .bind(DAILY_CACHE_CLEANUP_KEY)
    .bind(now.to_rfc3339())
    .execute(pool)
    .await?;

    if provider_rows_removed + media_rows_removed > 0 {
        // DELETE makes pages reusable, while VACUUM and the WAL checkpoint
        // return that space to the filesystem.
        compact(pool).await?;
    }
    tracing::info!(
        provider_rows_removed,
        media_rows_removed,
        "daily cache maintenance complete"
    );
    Ok(true)
}

/// Enforces the combined 100 MiB limit between the daily runs, so a large
/// scan cannot leave analysis caches oversized all day.
pub async fn enforce_media_cache_limit(pool: &SqlitePool) -> Result<u64> {
    let removed = enforce_media_cache_limit_with_limit(pool, MEDIA_CACHE_LIMIT_BYTES).await?;
    if removed > 0 {
        compact(pool).await?;
        tracing::info!(removed, "media-analysis cache limit enforced");
    }
    Ok(removed)
}

async fn compact(pool: &SqlitePool) -> Result<()> {
    sqlx::query("VACUUM").execute(pool).await?;
    sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
        .execute(pool)
        .await?;
    Ok(())
}

/// Removes the oldest media-analysis rows (across all cache tables) until the
/// estimated total size fits `limit_bytes`.
async fn enforce_media_cache_limit_with_limit(pool: &SqlitePool, limit_bytes: u64) -> Result<u64> {
    let mut transaction = pool.begin().await?;
    sqlx::query(
        "CREATE TEMP TABLE media_cache_victims AS
         WITH entries AS (
           SELECT 'fingerprint' AS kind,path,updated_at AS cached_at,
             length(CAST(path AS BLOB)) + length(CAST(fingerprint AS BLOB))
             + length(CAST(updated_at AS BLOB)) + 32 AS bytes
           FROM fingerprint_cache
           UNION ALL
           SELECT 'integrity',path,checked_at,
             length(CAST(path AS BLOB)) + length(CAST(COALESCE(diagnostic,'') AS BLOB))
             + length(CAST(checked_at AS BLOB)) + 40
           FROM integrity_cache
           UNION ALL
           SELECT 'content_hash',path,updated_at,
             length(CAST(path AS BLOB)) + length(CAST(sha256 AS BLOB))
             + length(CAST(updated_at AS BLOB)) + 32
           FROM content_hash_cache
         ),
         ranked AS (
           SELECT kind,path,
             SUM(bytes) OVER () AS total,
             COALESCE(SUM(bytes) OVER (ORDER BY cached_at,kind,path
               ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING), 0) AS older
           FROM entries
         )
         SELECT kind,path FROM ranked WHERE total - older > ?",
    )
    .bind(i64::try_from(limit_bytes).unwrap_or(i64::MAX))
    .execute(&mut *transaction)
    .await?;
    let mut removed = 0;
    for (table, kind) in [
        ("fingerprint_cache", "fingerprint"),
        ("integrity_cache", "integrity"),
        ("content_hash_cache", "content_hash"),
    ] {
        removed += sqlx::query(sqlx::AssertSqlSafe(format!(
            "DELETE FROM {table} WHERE path IN (SELECT path FROM media_cache_victims WHERE kind=?)"
        )))
        .bind(kind)
        .execute(&mut *transaction)
        .await?
        .rows_affected();
    }
    sqlx::query("DROP TABLE media_cache_victims")
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_pool() -> SqlitePool {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.sqlite");
        let pool = connect(path.to_str().unwrap()).await.unwrap();
        std::mem::forget(dir);
        pool
    }

    async fn insert_provider_row(pool: &SqlitePool, key: &str, expires_at: DateTime<Utc>) {
        sqlx::query("INSERT INTO provider_cache(provider,cache_key,response_json,expires_at) VALUES('musicbrainz',?,'{}',?)")
            .bind(key)
            .bind(expires_at.to_rfc3339())
            .execute(pool)
            .await
            .unwrap();
    }

    async fn provider_keys(pool: &SqlitePool) -> Vec<String> {
        sqlx::query_scalar("SELECT cache_key FROM provider_cache ORDER BY cache_key")
            .fetch_all(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn cleanup_removes_only_expired_provider_cache_rows() {
        let pool = test_pool().await;
        // Same day as now: a `datetime('now')` comparison got these wrong.
        insert_provider_row(&pool, "expired", Utc::now() - chrono::Duration::minutes(1)).await;
        insert_provider_row(&pool, "fresh", Utc::now() + chrono::Duration::minutes(1)).await;

        cleanup(&pool).await.unwrap();

        assert_eq!(provider_keys(&pool).await, vec!["fresh"]);
    }

    #[tokio::test]
    async fn daily_cleanup_keeps_fresh_provider_rows_and_media_caches() {
        let pool = test_pool().await;
        insert_provider_row(&pool, "expired", Utc::now() - chrono::Duration::days(1)).await;
        insert_provider_row(&pool, "fresh", Utc::now() + chrono::Duration::days(30)).await;
        sqlx::query("INSERT INTO fingerprint_cache(path,file_size,file_mtime,fingerprint,duration,updated_at) VALUES('/music/song.mp3',1,2,'fingerprint',3.0,datetime('now'))")
            .execute(&pool)
            .await
            .unwrap();

        assert!(run_daily_cache_cleanup_if_due(&pool).await.unwrap());
        assert_eq!(provider_keys(&pool).await, vec!["fresh"]);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM fingerprint_cache")
                .fetch_one(&pool)
                .await
                .unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn daily_cleanup_does_not_run_twice_within_one_day() {
        let pool = test_pool().await;
        assert!(run_daily_cache_cleanup_if_due(&pool).await.unwrap());
        insert_provider_row(&pool, "expired", Utc::now() - chrono::Duration::days(1)).await;

        assert!(!run_daily_cache_cleanup_if_due(&pool).await.unwrap());
        assert_eq!(provider_keys(&pool).await, vec!["expired"]);
    }

    #[tokio::test]
    async fn media_cache_limit_evicts_the_oldest_entries_across_tables() {
        let pool = test_pool().await;
        sqlx::query("INSERT INTO fingerprint_cache(path,file_size,file_mtime,fingerprint,duration,updated_at) VALUES('/old.mp3',1,2,?,3.0,'2026-01-01T00:00:00Z')")
            .bind("x".repeat(100))
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO integrity_cache(path,file_size,file_mtime_ns,is_healthy,diagnostic,checked_at) VALUES('/new.mp3',1,2,1,?,'2026-01-02T00:00:00Z')")
            .bind("y".repeat(100))
            .execute(&pool)
            .await
            .unwrap();

        assert_eq!(
            enforce_media_cache_limit_with_limit(&pool, 180)
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM fingerprint_cache")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM integrity_cache")
                .fetch_one(&pool)
                .await
                .unwrap(),
            1
        );
        // Under the limit nothing more goes.
        assert_eq!(
            enforce_media_cache_limit_with_limit(&pool, 180)
                .await
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn new_databases_record_the_schema_migration() {
        let pool = test_pool().await;
        let versions: Vec<i64> =
            sqlx::query_scalar("SELECT version FROM _sqlx_migrations ORDER BY version")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(
            versions,
            vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14]
        );
        for (table, expected) in [
            ("reference_files", 0),
            ("candidate_sources", 0),
            ("replaygain_cache", 0),
            ("content_hash_cache", 1),
        ] {
            assert_eq!(
                sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?",
                )
                .bind(table)
                .fetch_one(&pool)
                .await
                .unwrap(),
                expected,
                "{table}"
            );
        }
        let dropped_columns: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pragma_table_info('tracks')
             WHERE name IN ('last_applied_at','last_apply_run_id','retry_count','next_retry_at')",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(dropped_columns, 0);
    }

    #[tokio::test]
    async fn input_only_upgrade_removes_reference_index_and_requeues_stale_duplicates() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("reference-upgrade.sqlite");
        let url = format!("sqlite://{}?mode=rwc", path.display());
        let legacy = SqlitePoolOptions::new().connect(&url).await.unwrap();
        sqlx::raw_sql(include_str!("../../../migrations/0001_initial.sql"))
            .execute(&legacy)
            .await
            .unwrap();
        sqlx::raw_sql(include_str!(
            "../../../migrations/0002_automatic_scan_files.sql"
        ))
        .execute(&legacy)
        .await
        .unwrap();
        sqlx::raw_sql(include_str!(
            "../../../migrations/0003_reference_library.sql"
        ))
        .execute(&legacy)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO tracks(path,filename,status,is_missing,first_seen_at,last_seen_at,
             last_scanned_at,stage) VALUES('/input/song.mp3','song.mp3','duplicate',0,
             'now','now','now','skipped')",
        )
        .execute(&legacy)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO automatic_scan_files(path,file_size,file_mtime_ns,checked_at)
             VALUES('/input/song.mp3',1,1,'now')",
        )
        .execute(&legacy)
        .await
        .unwrap();
        legacy.close().await;

        let pool = connect(path.to_str().unwrap()).await.unwrap();

        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM tracks")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM automatic_scan_files")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='reference_files'",
            )
            .fetch_one(&pool)
            .await
            .unwrap(),
            0
        );
    }
}
