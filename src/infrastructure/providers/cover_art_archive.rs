//! Downloading and checking cover images. Checked covers are kept for 30 days
//! as files (`infrastructure::cover_files`), so every track of an album shares
//! one download.

use crate::infrastructure::{
    cover_files,
    media::tag_writer::{
        ArtworkCheck, ArtworkInfo, MAX_ARTWORK_BYTES, check_artwork, check_own_cover, read_artwork,
    },
    provider_cache::url_key,
};
use anyhow::{Context, Result, bail};
use reqwest::Client;
use sqlx::SqlitePool;

/// The front cover of a MusicBrainz release group: the album across all its
/// editions. Used when the chosen edition has no image of its own; another
/// edition of the same album usually does.
pub fn release_group_front_url(release_group_id: &str) -> String {
    format!("https://coverartarchive.org/release-group/{release_group_id}/front-500")
}

/// Marks a cover that is not on the web: the picture inside one of your own
/// files. Only "keep the file's own tags" in Review hands one out.
const FILE_COVER: &str = "magpie-file-cover:";

/// The cover address that stands for the picture inside the file at `path`.
pub fn file_cover_url(path: &str) -> String {
    format!("{FILE_COVER}{path}")
}

pub fn is_file_cover_url(url: &str) -> bool {
    url.starts_with(FILE_COVER)
}

/// Reads and checks a file's own picture. Nothing is cached: the file is
/// local, and another file may later sit at the same path.
async fn file_cover(path: &str) -> Result<(Vec<u8>, ArtworkInfo)> {
    let path = std::path::PathBuf::from(path);
    tokio::task::spawn_blocking(move || {
        let data = read_artwork(&path)
            .context("the file could not be read for its own picture")?
            .context("the file has no usable picture of its own")?;
        let info = check_own_cover(&data)?;
        Ok((data, info))
    })
    .await
    .context("cover check stopped")?
}

/// Downloads an image, refusing anything over the size limit (also while it
/// streams in, when the server sends no length).
pub async fn fetch(client: &Client, url: &str) -> Result<Vec<u8>> {
    let too_big = || anyhow::anyhow!("cover image exceeds the 20 MB safety limit");
    let mut response = crate::infrastructure::resilient_http::get(client, url)
        .await?
        .error_for_status()?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_ARTWORK_BYTES as u64)
    {
        return Err(too_big());
    }
    let mut data = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if data.len() + chunk.len() > MAX_ARTWORK_BYTES {
            return Err(too_big());
        }
        data.extend_from_slice(&chunk);
    }
    Ok(data)
}

/// A checked cover image and what it is. `trusted_exact_release` accepts a
/// plain single-colour image (some real releases have one).
pub async fn fetch_verified_url_cached(
    pool: &SqlitePool,
    client: &Client,
    url: &str,
    trusted_exact_release: bool,
) -> Result<(Vec<u8>, ArtworkInfo)> {
    if let Some(path) = url.strip_prefix(FILE_COVER) {
        return file_cover(path).await;
    }
    let key = url_key(url);
    if let Some(stored) = cover_files::load(pool, &key).await? {
        // Images kept with their check result are not decoded again.
        if let Some(check) = stored.check
            && (trusted_exact_release || !check.single_colour)
        {
            return Ok((stored.data, check.info));
        }
        match check_in_background(stored.data, trusted_exact_release).await {
            Ok((data, check)) => {
                keep(pool, &key, &data, &check).await;
                return Ok((data, check.info));
            }
            Err(_) => cover_files::forget(pool, &key).await?,
        }
    }
    let data = fetch(client, url).await?;
    let (data, check) = check_in_background(data, trusted_exact_release).await?;
    keep(pool, &key, &data, &check).await;
    Ok((data, check.info))
}

/// Keeps a checked cover for the other songs of its album. A cover that
/// cannot be kept (a full disk) is still used for this song.
async fn keep(pool: &SqlitePool, key: &str, data: &[u8], check: &ArtworkCheck) {
    if let Err(error) = cover_files::store(pool, key, data, check).await {
        tracing::warn!("could not keep a cover for later: {error:#}");
    }
}

/// Decoding a large image takes a while; keep it off the async workers.
async fn check_in_background(
    data: Vec<u8>,
    trusted_exact_release: bool,
) -> Result<(Vec<u8>, ArtworkCheck)> {
    tokio::task::spawn_blocking(move || {
        let check = check_artwork(&data)?;
        if check.single_colour && !trusted_exact_release {
            bail!("cover image is a blank constant-color placeholder");
        }
        Ok((data, check))
    })
    .await
    .context("cover check stopped")?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::{cover_files::CACHE, provider_cache};
    use base64::{Engine, engine::general_purpose::STANDARD};
    use chrono::{Duration, Utc};
    use serde_json::json;

    async fn test_pool() -> (tempfile::TempDir, SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.sqlite");
        let pool = crate::infrastructure::db::connect(path.to_str().unwrap())
            .await
            .unwrap();
        (dir, pool)
    }

    #[tokio::test]
    async fn cached_cover_art_by_url_decodes_bytes() {
        let (_dir, pool) = test_pool().await;
        let expected = crate::infrastructure::media::tag_writer::test_artwork_png();
        let url = "http://127.0.0.1:1/Should-Not-Be-Called";
        provider_cache::put(
            &pool,
            CACHE,
            &url_key(url),
            &json!({ "data_base64": STANDARD.encode(&expected) }),
            Utc::now() + Duration::days(1),
        )
        .await
        .unwrap();

        let (data, info) = fetch_verified_url_cached(&pool, &Client::new(), url, false)
            .await
            .unwrap();
        assert_eq!(data, expected);
        assert_eq!((info.width, info.height), (320, 320));
        // It was kept inside the database by an older version; after this
        // first use it is a file with its check result, and reads the same.
        let note = provider_cache::get(&pool, CACHE, &url_key(url))
            .await
            .unwrap()
            .unwrap();
        assert!(note["data_base64"].is_null(), "{note}");
        assert_eq!(note["width"], 320);
        let (again, _) = fetch_verified_url_cached(&pool, &Client::new(), url, false)
            .await
            .unwrap();
        assert_eq!(again, expected);
    }

    #[tokio::test]
    async fn a_cover_is_downloaded_once_and_then_read_from_its_file() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (directory, pool) = test_pool().await;
        let image = crate::infrastructure::media::tag_writer::test_artwork_png_sized(600, 600);
        // A local server that answers exactly one request.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let body = image.clone();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 2048];
            let _ = socket.read(&mut request).await;
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            socket.write_all(head.as_bytes()).await.unwrap();
            socket.write_all(&body).await.unwrap();
        });
        let url = format!("http://{address}/cover.png");

        let (first, info) = fetch_verified_url_cached(&pool, &Client::new(), &url, false)
            .await
            .unwrap();
        assert_eq!(first, image);
        assert_eq!((info.width, info.height), (600, 600));
        // The image is a file next to the database; the database has a short note.
        let file = directory
            .path()
            .join("covers")
            .join(&info.sha256[..2])
            .join(&info.sha256);
        assert_eq!(std::fs::read(&file).unwrap(), image);
        let stored: i64 = sqlx::query_scalar(
            "SELECT length(response_json) FROM provider_cache WHERE provider='artwork-url'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(stored < 400, "{stored}");

        // The server is gone: the second song of the album reads the file.
        let (second, _) = fetch_verified_url_cached(&pool, &Client::new(), &url, false)
            .await
            .unwrap();
        assert_eq!(second, image);

        // A file damaged on disk is not used: the cover is asked for again,
        // and with the server gone that fails instead of filing a broken image.
        std::fs::write(&file, vec![0_u8; image.len()]).unwrap();
        assert!(
            fetch_verified_url_cached(&pool, &Client::new(), &url, false)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn downloads_stop_at_the_size_limit_without_a_length() {
        use tokio::io::AsyncWriteExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 1024];
            let _ = tokio::io::AsyncReadExt::read(&mut socket, &mut request).await;
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n")
                .await
                .unwrap();
            let chunk = vec![b'x'; 1024 * 1024];
            for _ in 0..21 {
                let header = format!("{:x}\r\n", chunk.len());
                if socket.write_all(header.as_bytes()).await.is_err()
                    || socket.write_all(&chunk).await.is_err()
                    || socket.write_all(b"\r\n").await.is_err()
                {
                    return;
                }
            }
            let _ = socket.write_all(b"0\r\n\r\n").await;
        });
        let error = fetch(&Client::new(), &format!("http://{address}/cover.jpg"))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("20 MB"));
    }
}
