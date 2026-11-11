//! Checked cover images, kept as files in a `covers` folder next to the
//! database, so every song of an album shares one download.
//!
//! The database only keeps a small note per cover address (`provider_cache`,
//! source `artwork-url`): which image it is, its size and what the check
//! found. The image itself is a file named after its SHA-256:
//!
//! ```text
//! DATA_DIR/covers/3f/3fa9…c1      one image, whatever addresses lead to it
//! ```
//!
//! Why files: images are hundreds of kilobytes each, and a database is made
//! for many small things. Inside SQLite they made the file grow past a
//! gigabyte on a large import, and every cleanup had to copy them around.
//!
//! What keeps this reliable:
//! - a file is written under a temporary name and renamed into place, so a
//!   half-written image is never read;
//! - every read checks the image against its SHA-256, so a damaged file is
//!   dropped and downloaded again, never used as a cover;
//! - the folder is only a cache: deleting it loses nothing but downloads.

use crate::infrastructure::{
    media::tag_writer::{ArtworkCheck, ArtworkInfo},
    provider_cache,
};
use anyhow::{Context, Result};
use base64::{Engine, engine::general_purpose::STANDARD};
use chrono::{Duration, Utc};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    time::SystemTime,
};

/// The `provider_cache` source of the notes.
pub const CACHE: &str = "artwork-url";
/// A cover is kept this long after it was downloaded.
const KEPT_DAYS: i64 = 30;
/// All covers together. 200 MiB holds the covers of about 500 albums, far
/// more than are being worked on at any one time.
const LIMIT_BYTES: u64 = 200 * 1024 * 1024;
/// Files younger than this are never swept: their note may not be written yet.
const SWEEP_SPARES: std::time::Duration = std::time::Duration::from_secs(60 * 60);

/// A cover read back from the cache.
pub struct StoredCover {
    pub data: Vec<u8>,
    /// What the check found when the cover was stored. `None` for entries
    /// from before the result was kept; the caller checks those again.
    pub check: Option<ArtworkCheck>,
}

/// The covers folder: next to the database file, so each database has its
/// own. `None` for a database that is not a file (nothing is cached then).
pub fn folder(pool: &SqlitePool) -> Option<PathBuf> {
    let options = pool.connect_options();
    let database = options.get_filename();
    let parent = database.parent()?;
    if database.file_name()?.to_str()? == ":memory:" {
        return None;
    }
    Some(parent.join("covers"))
}

/// Two letters of the name make a subfolder, so no folder gets thousands of files.
fn file_of(folder: &Path, sha256: &str) -> PathBuf {
    folder.join(sha256.get(..2).unwrap_or("00")).join(sha256)
}

/// A SHA-256 as stored: 64 hex digits. Anything else never becomes a path.
fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn sha256_of(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

/// The cover kept for `key` (a cover address), or `None` when there is none
/// or its file is gone or damaged.
pub async fn load(pool: &SqlitePool, key: &str) -> Result<Option<StoredCover>> {
    let Some(note) = provider_cache::get(pool, CACHE, key).await? else {
        return Ok(None);
    };
    let check = stored_check(&note);
    // From before covers were files: the image is inside the note.
    if let Some(encoded) = note["data_base64"].as_str() {
        let data = STANDARD.decode(encoded)?;
        return Ok(Some(StoredCover { data, check }));
    }
    let (Some(folder), Some(sha256)) = (folder(pool), note["sha256"].as_str()) else {
        return Ok(None);
    };
    if !valid_sha256(sha256) {
        provider_cache::delete(pool, CACHE, key).await?;
        return Ok(None);
    }
    let file = file_of(&folder, sha256);
    let intact = match tokio::fs::read(&file).await {
        Ok(data) => {
            let expected = sha256.to_owned();
            tokio::task::spawn_blocking(move || (sha256_of(&data) == expected).then_some(data))
                .await
                .context("cover check stopped")?
        }
        Err(_) => None,
    };
    match intact {
        Some(data) => Ok(Some(StoredCover { data, check })),
        None => {
            // Missing or damaged: forget it, so it is downloaded again.
            tracing::warn!(file = %file.display(), "a cached cover was missing or damaged; it will be downloaded again");
            let _ = tokio::fs::remove_file(&file).await;
            provider_cache::delete(pool, CACHE, key).await?;
            Ok(None)
        }
    }
}

/// Keeps a checked cover for `key`.
pub async fn store(pool: &SqlitePool, key: &str, data: &[u8], check: &ArtworkCheck) -> Result<()> {
    let Some(folder) = folder(pool) else {
        return Ok(());
    };
    let sha256 = check.info.sha256.clone();
    if !valid_sha256(&sha256) {
        anyhow::bail!("a cover has no valid SHA-256");
    }
    write_file(&folder, &sha256, data).await?;
    provider_cache::put(
        pool,
        CACHE,
        key,
        &json!({
            "sha256": sha256,
            "bytes": data.len(),
            "width": check.info.width,
            "height": check.info.height,
            "mime": check.info.mime,
            "single_colour": check.single_colour,
        }),
        Utc::now() + Duration::days(KEPT_DAYS),
    )
    .await?;
    enforce_limit(pool, LIMIT_BYTES).await?;
    Ok(())
}

/// Writes the image under a temporary name and renames it into place. The
/// same image already there (another address of the same cover) is kept.
async fn write_file(folder: &Path, sha256: &str, data: &[u8]) -> Result<()> {
    let file = file_of(folder, sha256);
    if tokio::fs::metadata(&file)
        .await
        .is_ok_and(|metadata| metadata.len() == data.len() as u64)
    {
        return Ok(());
    }
    let parent = file.parent().context("cover file has no folder")?;
    tokio::fs::create_dir_all(parent)
        .await
        .with_context(|| format!("could not create the covers folder {}", parent.display()))?;
    let unique = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |time| time.as_nanos());
    let temporary = parent.join(format!(".{sha256}.{}.{unique}.part", std::process::id()));
    let written = async {
        tokio::fs::write(&temporary, data).await?;
        tokio::fs::rename(&temporary, &file).await
    }
    .await;
    if written.is_err() {
        let _ = tokio::fs::remove_file(&temporary).await;
    }
    written.with_context(|| format!("could not keep a cover in {}", parent.display()))
}

/// Forgets the cover kept for `key`. Its file goes when nothing else uses it.
pub async fn forget(pool: &SqlitePool, key: &str) -> Result<()> {
    let sha256 = provider_cache::get(pool, CACHE, key)
        .await?
        .and_then(|note| note["sha256"].as_str().map(str::to_owned));
    provider_cache::delete(pool, CACHE, key).await?;
    if let Some(sha256) = sha256 {
        remove_unused(pool, &[sha256]).await?;
    }
    Ok(())
}

/// Deletes the files of these images that no note points to any more.
async fn remove_unused(pool: &SqlitePool, images: &[String]) -> Result<()> {
    let Some(folder) = folder(pool) else {
        return Ok(());
    };
    for sha256 in images.iter().filter(|sha256| valid_sha256(sha256)) {
        let used: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM provider_cache
                           WHERE provider=? AND json_extract(response_json,'$.sha256')=?)",
        )
        .bind(CACHE)
        .bind(sha256)
        .fetch_one(pool)
        .await?;
        if !used {
            let _ = tokio::fs::remove_file(file_of(&folder, sha256)).await;
        }
    }
    Ok(())
}

/// Keeps all covers together under `limit_bytes`. Runs whenever a cover is
/// added, so the limit also holds in the middle of a long import. The covers
/// kept the shortest time go first, down to four fifths of the limit (so this
/// does not run for every cover once the cache is full). A removed cover is
/// simply downloaded again if another song of its album turns up.
async fn enforce_limit(pool: &SqlitePool, limit_bytes: u64) -> Result<usize> {
    // Notes from before covers were files have no `bytes`; their own size is
    // the image they still carry.
    const SIZE: &str = "COALESCE(json_extract(response_json,'$.bytes'),
                                 length(CAST(response_json AS BLOB)))";
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT COALESCE(SUM({SIZE}),0) FROM provider_cache WHERE provider=?"
    )))
    .bind(CACHE)
    .fetch_one(pool)
    .await?;
    if u64::try_from(total).unwrap_or(0) <= limit_bytes {
        return Ok(0);
    }
    let keep = i64::try_from(limit_bytes / 5 * 4).unwrap_or(i64::MAX);
    let removed: Vec<(String, Option<String>)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT cache_key, json_extract(response_json,'$.sha256') FROM (
           SELECT cache_key, response_json,
             SUM({SIZE}) OVER (ORDER BY expires_at DESC, cache_key) AS kept
           FROM provider_cache WHERE provider=?
         ) WHERE kept > ?"
    )))
    .bind(CACHE)
    .bind(keep)
    .fetch_all(pool)
    .await?;
    let mut transaction = pool.begin().await?;
    for (key, _) in &removed {
        sqlx::query("DELETE FROM provider_cache WHERE provider=? AND cache_key=?")
            .bind(CACHE)
            .bind(key)
            .execute(&mut *transaction)
            .await?;
    }
    transaction.commit().await?;
    let images = removed
        .iter()
        .filter_map(|(_, sha256)| sha256.clone())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    remove_unused(pool, &images).await?;
    tracing::info!(removed = removed.len(), "cover cache limit enforced");
    Ok(removed.len())
}

/// Daily: deletes cover files that no note points to (their note expired) and
/// unfinished downloads left by a stop. Files of the last hour are spared,
/// because a cover is written before its note.
pub async fn sweep(pool: &SqlitePool) -> Result<usize> {
    sweep_older_than(pool, SWEEP_SPARES).await
}

async fn sweep_older_than(pool: &SqlitePool, spare: std::time::Duration) -> Result<usize> {
    let Some(folder) = folder(pool) else {
        return Ok(0);
    };
    let used: Vec<Option<String>> = sqlx::query_scalar(
        "SELECT json_extract(response_json,'$.sha256') FROM provider_cache
         WHERE provider=? AND expires_at>?",
    )
    .bind(CACHE)
    .bind(provider_cache::timestamp(Utc::now()))
    .fetch_all(pool)
    .await?;
    let used = used.into_iter().flatten().collect::<HashSet<_>>();
    tokio::task::spawn_blocking(move || {
        let mut removed = 0;
        let files = walkdir::WalkDir::new(&folder)
            .min_depth(1)
            .max_depth(2)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file());
        for entry in files {
            let name = entry.file_name().to_string_lossy();
            let old_enough = entry
                .metadata()
                .ok()
                .and_then(|metadata| metadata.modified().ok())
                .and_then(|modified| modified.elapsed().ok())
                .is_some_and(|age| age >= spare);
            if old_enough
                && !used.contains(name.as_ref())
                && std::fs::remove_file(entry.path()).is_ok()
            {
                removed += 1;
            }
        }
        removed
    })
    .await
    .context("cover sweep stopped")
}

/// Once, at start: covers that an older version kept inside the database are
/// written out as files. Returns how many were moved; the caller compacts the
/// database afterwards, which gives the space back to the disk.
pub async fn move_out_of_database(pool: &SqlitePool) -> Result<usize> {
    let keys: Vec<String> = sqlx::query_scalar(
        "SELECT cache_key FROM provider_cache
         WHERE provider=? AND response_json LIKE '%\"data_base64\"%'",
    )
    .bind(CACHE)
    .fetch_all(pool)
    .await?;
    let mut moved = 0;
    for key in keys {
        let stored = match load(pool, &key).await {
            Ok(Some(stored)) => stored,
            Ok(None) => continue,
            Err(_) => {
                provider_cache::delete(pool, CACHE, &key).await?;
                continue;
            }
        };
        match stored.check {
            Some(check) if folder(pool).is_some() => {
                store(pool, &key, &stored.data, &check).await?;
                moved += 1;
            }
            // Never checked, or nowhere to put it: it is downloaded again when needed.
            _ => provider_cache::delete(pool, CACHE, &key).await?,
        }
    }
    Ok(moved)
}

/// The check result written by [`store`].
fn stored_check(note: &Value) -> Option<ArtworkCheck> {
    let mime = match note["mime"].as_str()? {
        "image/jpeg" => "image/jpeg",
        "image/png" => "image/png",
        "image/gif" => "image/gif",
        "image/webp" => "image/webp",
        _ => return None,
    };
    Some(ArtworkCheck {
        info: ArtworkInfo {
            width: u32::try_from(note["width"].as_u64()?).ok()?,
            height: u32::try_from(note["height"].as_u64()?).ok()?,
            mime,
            sha256: note["sha256"].as_str()?.to_owned(),
        },
        single_colour: note["single_colour"].as_bool()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::media::tag_writer::{check_artwork, test_artwork_png_sized};

    async fn test_pool() -> (tempfile::TempDir, SqlitePool) {
        let directory = tempfile::tempdir().unwrap();
        let pool = crate::infrastructure::db::connect(
            directory.path().join("covers.sqlite").to_str().unwrap(),
        )
        .await
        .unwrap();
        (directory, pool)
    }

    fn cover(size: u32) -> (Vec<u8>, ArtworkCheck) {
        let data = test_artwork_png_sized(size, size);
        let check = check_artwork(&data).unwrap();
        (data, check)
    }

    fn files(folder: &Path) -> Vec<String> {
        let mut names = walkdir::WalkDir::new(folder)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        names.sort();
        names
    }

    async fn notes(pool: &SqlitePool) -> Vec<String> {
        sqlx::query_scalar(
            "SELECT cache_key FROM provider_cache WHERE provider=? ORDER BY cache_key",
        )
        .bind(CACHE)
        .fetch_all(pool)
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn a_cover_is_a_file_next_to_the_database_and_reads_back_whole() {
        let (directory, pool) = test_pool().await;
        let (data, check) = cover(400);

        store(&pool, "url:one", &data, &check).await.unwrap();

        let folder = directory.path().join("covers");
        assert_eq!(folder, self::folder(&pool).unwrap());
        let sha256 = check.info.sha256.clone();
        assert!(folder.join(&sha256[..2]).join(&sha256).is_file());
        // The database keeps a note of a few hundred bytes, not the image.
        let note: String =
            sqlx::query_scalar("SELECT response_json FROM provider_cache WHERE provider=?")
                .bind(CACHE)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(note.len() < 400 && !note.contains("data_base64"), "{note}");

        let stored = load(&pool, "url:one").await.unwrap().unwrap();
        assert_eq!(stored.data, data);
        assert_eq!(stored.check, Some(check));
        assert!(load(&pool, "url:other").await.unwrap().is_none());
        // No temporary file is left behind.
        assert_eq!(files(&folder), vec![sha256]);
    }

    #[tokio::test]
    async fn two_addresses_of_one_image_share_one_file() {
        let (directory, pool) = test_pool().await;
        let (data, check) = cover(400);
        store(&pool, "url:one", &data, &check).await.unwrap();
        store(&pool, "url:two", &data, &check).await.unwrap();
        let folder = directory.path().join("covers");
        assert_eq!(files(&folder).len(), 1);

        // Forgetting one address keeps the file for the other.
        forget(&pool, "url:one").await.unwrap();
        assert_eq!(files(&folder).len(), 1);
        assert!(load(&pool, "url:two").await.unwrap().is_some());
        forget(&pool, "url:two").await.unwrap();
        assert!(files(&folder).is_empty());
    }

    #[tokio::test]
    async fn a_damaged_or_missing_file_is_never_used_as_a_cover() {
        let (directory, pool) = test_pool().await;
        let (data, check) = cover(400);
        let file = file_of(&directory.path().join("covers"), &check.info.sha256);

        // Damaged on disk: same length, other bytes.
        store(&pool, "url:one", &data, &check).await.unwrap();
        let mut damaged = data.clone();
        let middle = damaged.len() / 2;
        damaged[middle] ^= 0xff;
        std::fs::write(&file, &damaged).unwrap();
        assert!(load(&pool, "url:one").await.unwrap().is_none());
        assert!(
            notes(&pool).await.is_empty(),
            "it is forgotten, so it is fetched again"
        );
        assert!(!file.exists());

        // Deleted by hand (the folder is only a cache).
        store(&pool, "url:one", &data, &check).await.unwrap();
        std::fs::remove_dir_all(directory.path().join("covers")).unwrap();
        assert!(load(&pool, "url:one").await.unwrap().is_none());
        // And it can be stored again afterwards.
        store(&pool, "url:one", &data, &check).await.unwrap();
        assert_eq!(load(&pool, "url:one").await.unwrap().unwrap().data, data);
    }

    #[tokio::test]
    async fn a_note_with_a_strange_name_never_becomes_a_path() {
        let (directory, pool) = test_pool().await;
        let outside = directory.path().join("secret.txt");
        std::fs::write(&outside, b"not a cover").unwrap();
        provider_cache::put(
            &pool,
            CACHE,
            "url:evil",
            &json!({"sha256": "../secret.txt", "bytes": 11}),
            Utc::now() + Duration::days(1),
        )
        .await
        .unwrap();

        assert!(load(&pool, "url:evil").await.unwrap().is_none());
        forget(&pool, "url:evil").await.unwrap();
        assert_eq!(std::fs::read(&outside).unwrap(), b"not a cover");
    }

    #[tokio::test]
    async fn the_covers_stay_under_the_limit_and_the_newest_are_kept() {
        let (directory, pool) = test_pool().await;
        let folder = directory.path().join("covers");
        let mut sizes = Vec::new();
        for (number, days) in [(1_u32, 10), (2, 30), (3, 20), (4, 25), (5, 5)] {
            let (data, check) = cover(300 + number * 10);
            store(&pool, &format!("url:{number}"), &data, &check)
                .await
                .unwrap();
            sqlx::query("UPDATE provider_cache SET expires_at=? WHERE cache_key=?")
                .bind(provider_cache::timestamp(Utc::now() + Duration::days(days)))
                .bind(format!("url:{number}"))
                .execute(&pool)
                .await
                .unwrap();
            sizes.push(data.len() as u64);
        }
        // Another kind of cached answer is never touched.
        provider_cache::put(
            &pool,
            "musicbrainz",
            "search:song",
            &json!({}),
            Utc::now() + Duration::days(1),
        )
        .await
        .unwrap();
        let total: u64 = sizes.iter().sum();

        // Room for everything: nothing happens.
        assert_eq!(enforce_limit(&pool, total).await.unwrap(), 0);
        assert_eq!(files(&folder).len(), 5);
        // One byte too many: down to four fifths, the ones kept longest stay.
        let removed = enforce_limit(&pool, total - 1).await.unwrap();
        assert!(removed >= 1);
        let kept = notes(&pool).await;
        assert!(
            kept.contains(&"url:2".to_owned()) && !kept.contains(&"url:5".to_owned()),
            "{kept:?}"
        );
        // Every note has its file and every file its note.
        assert_eq!(files(&folder).len(), kept.len());
        for key in &kept {
            assert!(load(&pool, key).await.unwrap().is_some(), "{key}");
        }
        let others: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM provider_cache WHERE provider='musicbrainz'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(others, 1);
    }

    #[tokio::test]
    async fn the_sweep_removes_files_nothing_points_to_and_spares_fresh_ones() {
        let (directory, pool) = test_pool().await;
        let folder = directory.path().join("covers");
        let (kept, kept_check) = cover(400);
        let (expired, expired_check) = cover(500);
        store(&pool, "url:kept", &kept, &kept_check).await.unwrap();
        store(&pool, "url:expired", &expired, &expired_check)
            .await
            .unwrap();
        sqlx::query("UPDATE provider_cache SET expires_at=? WHERE cache_key='url:expired'")
            .bind(provider_cache::timestamp(Utc::now() - Duration::days(1)))
            .execute(&pool)
            .await
            .unwrap();
        // An unfinished download left by a stop.
        std::fs::write(folder.join("ab.123.part"), b"half").unwrap();

        // Files of the last hour are spared: a cover is written before its note.
        assert_eq!(sweep(&pool).await.unwrap(), 0);
        assert_eq!(files(&folder).len(), 3);

        assert_eq!(
            sweep_older_than(&pool, std::time::Duration::ZERO)
                .await
                .unwrap(),
            2
        );
        assert_eq!(files(&folder), vec![kept_check.info.sha256.clone()]);
        assert!(load(&pool, "url:kept").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn covers_an_older_version_kept_in_the_database_become_files() {
        let (directory, pool) = test_pool().await;
        let (data, check) = cover(400);
        // With its check result: it is moved.
        provider_cache::put(
            &pool,
            CACHE,
            "url:checked",
            &json!({
                "data_base64": STANDARD.encode(&data),
                "sha256": check.info.sha256,
                "width": check.info.width,
                "height": check.info.height,
                "mime": check.info.mime,
                "single_colour": check.single_colour,
            }),
            Utc::now() + Duration::days(9),
        )
        .await
        .unwrap();
        // From before check results were kept: still readable until then.
        provider_cache::put(
            &pool,
            CACHE,
            "url:unchecked",
            &json!({"data_base64": STANDARD.encode(&data)}),
            Utc::now() + Duration::days(9),
        )
        .await
        .unwrap();
        let old = load(&pool, "url:unchecked").await.unwrap().unwrap();
        assert_eq!((old.data.len(), old.check), (data.len(), None));

        assert_eq!(move_out_of_database(&pool).await.unwrap(), 1);

        assert_eq!(notes(&pool).await, vec!["url:checked"]);
        let in_database: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM provider_cache WHERE response_json LIKE '%data_base64%'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(in_database, 0);
        assert_eq!(files(&directory.path().join("covers")).len(), 1);
        let moved = load(&pool, "url:checked").await.unwrap().unwrap();
        assert_eq!((moved.data, moved.check), (data, Some(check)));
        // Nothing is left to move the second time.
        assert_eq!(move_out_of_database(&pool).await.unwrap(), 0);
    }
}
