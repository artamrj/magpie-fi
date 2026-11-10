//! Downloading and checking cover images. Checked covers are cached by URL for
//! 30 days, so every track of an album shares one download.

use crate::infrastructure::{
    media::tag_writer::{ArtworkCheck, ArtworkInfo, MAX_ARTWORK_BYTES, check_artwork},
    provider_cache::{self, url_key},
};
use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use chrono::{Duration, Utc};
use reqwest::Client;
use serde_json::{Value, json};
use sqlx::SqlitePool;

const CACHE: &str = "artwork-url";

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
    let key = url_key(url);
    if let Some(value) = provider_cache::get(pool, CACHE, &key).await?
        && let Some(encoded) = value["data_base64"].as_str()
    {
        let data = STANDARD.decode(encoded)?;
        // Images cached with their check result are not decoded again.
        if let Some(check) = cached_check(&value)
            && (trusted_exact_release || !check.single_colour)
        {
            return Ok((data, check.info));
        }
        match check_in_background(data, trusted_exact_release).await {
            Ok((data, check)) => return Ok((data, check.info)),
            Err(_) => provider_cache::delete(pool, CACHE, &key).await?,
        }
    }
    let data = fetch(client, url).await?;
    let (data, check) = check_in_background(data, trusted_exact_release).await?;
    provider_cache::put(
        pool,
        CACHE,
        &key,
        &json!({
            "data_base64": STANDARD.encode(&data),
            "sha256": check.info.sha256,
            "width": check.info.width,
            "height": check.info.height,
            "mime": check.info.mime,
            "single_colour": check.single_colour,
        }),
        Utc::now() + Duration::days(30),
    )
    .await?;
    Ok((data, check.info))
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

fn cached_check(value: &Value) -> Option<ArtworkCheck> {
    let mime = match value["mime"].as_str()? {
        "image/jpeg" => "image/jpeg",
        "image/png" => "image/png",
        "image/gif" => "image/gif",
        "image/webp" => "image/webp",
        _ => return None,
    };
    Some(ArtworkCheck {
        info: ArtworkInfo {
            width: u32::try_from(value["width"].as_u64()?).ok()?,
            height: u32::try_from(value["height"].as_u64()?).ok()?,
            mime,
            sha256: value["sha256"].as_str()?.to_owned(),
        },
        single_colour: value["single_colour"].as_bool()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // A second read uses the stored check result.
        let again = provider_cache::get(&pool, CACHE, &url_key(url))
            .await
            .unwrap();
        assert!(again.is_some());
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
