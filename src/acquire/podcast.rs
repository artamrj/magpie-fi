//! Radio Javan podcast episodes (DJ mixes). Unlike songs, these are downloaded from Radio
//! Javan itself: an episode is a one-hour mix that is rarely on YouTube in one piece, and
//! Radio Javan serves free episodes as a public MP3 without DRM. Episodes only Radio Javan
//! Premium members may play are refused.

use crate::{config::Config, infrastructure::providers::radiojavan};
use anyhow::{Context, Result, bail};
use std::{path::PathBuf, time::Duration};
use tokio::io::AsyncWriteExt;

/// One episode is about 90 MB at 192 kbps; this leaves room for the longest shows.
const MAX_EPISODE_BYTES: u64 = 1024 * 1024 * 1024;
/// The shared client gives up after 12 seconds; a whole episode needs longer.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// Whether a queued song comes from a Radio Javan podcast link.
pub fn is_podcast_link(url: &str) -> bool {
    matches!(
        radiojavan::player_slug(url),
        Ok((radiojavan::LinkKind::Podcast, _))
    )
}

/// Why a download did not happen, and whether trying again later can help.
pub struct DownloadError {
    pub error: anyhow::Error,
    pub retry: bool,
}

impl From<anyhow::Error> for DownloadError {
    fn from(error: anyhow::Error) -> Self {
        Self { error, retry: true }
    }
}

fn refused(message: &str) -> DownloadError {
    DownloadError {
        error: anyhow::anyhow!("{message}"),
        retry: false,
    }
}

/// Downloads the episode behind `url` into `staging/<stem>.<ext>`, as Radio Javan sends it
/// (no re-encoding). The file appears there only once it is complete.
pub async fn download(
    pool: &sqlx::SqlitePool,
    client: &reqwest::Client,
    config: &Config,
    url: &str,
    stem: &str,
) -> std::result::Result<PathBuf, DownloadError> {
    let episode = radiojavan::podcast(pool, client, url).await?;
    if episode.premium_only {
        return Err(refused(
            "this podcast is for Radio Javan Premium members only; magpie-fi downloads free podcasts only",
        ));
    }
    let Some(audio) = episode.audio_url else {
        return Err(refused("Radio Javan gives no audio for this podcast"));
    };
    let extension = audio_extension(&audio).ok_or_else(|| refused("unknown podcast audio type"))?;
    let part = config.staging_dir.join(".part");
    tokio::fs::create_dir_all(&part)
        .await
        .context("could not create the staging folder")?;
    let partial = part.join(format!("{stem}.{extension}"));
    let finished = config.staging_dir.join(format!("{stem}.{extension}"));
    let result = save(client, &audio, &partial).await;
    if let Err(error) = result {
        let _ = tokio::fs::remove_file(&partial).await;
        return Err(error.into());
    }
    tokio::fs::rename(&partial, &finished)
        .await
        .context("could not move the podcast into staging")?;
    Ok(finished)
}

async fn save(client: &reqwest::Client, audio: &str, path: &std::path::Path) -> Result<()> {
    let mut response = client
        .get(audio)
        .timeout(DOWNLOAD_TIMEOUT)
        .send()
        .await
        .context("Radio Javan did not answer")?
        .error_for_status()
        .context("Radio Javan refused the podcast download")?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_EPISODE_BYTES)
    {
        bail!("the podcast is larger than 1 GB");
    }
    let mut file = tokio::fs::File::create(path)
        .await
        .context("could not create the podcast file")?;
    let mut written = 0u64;
    while let Some(chunk) = response
        .chunk()
        .await
        .context("the podcast download broke off")?
    {
        written += chunk.len() as u64;
        if written > MAX_EPISODE_BYTES {
            bail!("the podcast is larger than 1 GB");
        }
        file.write_all(&chunk).await?;
    }
    file.flush().await?;
    if written == 0 {
        bail!("Radio Javan sent an empty podcast file");
    }
    Ok(())
}

/// The file type from the audio link: Radio Javan serves `.mp3` and `.m4a`.
fn audio_extension(url: &str) -> Option<&'static str> {
    let path = reqwest::Url::parse(url).ok()?.path().to_ascii_lowercase();
    if path.ends_with(".mp3") {
        Some("mp3")
    } else if path.ends_with(".m4a") {
        Some("m4a")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knows_podcast_links_and_audio_types() {
        assert!(is_podcast_link(
            "https://play.radiojavan.com/podcast/dor-dor-8"
        ));
        assert!(!is_podcast_link(
            "https://play.radiojavan.com/song/some-song"
        ));
        assert!(!is_podcast_link("https://www.youtube.com/watch?v=abc"));
        assert_eq!(
            audio_extension("https://host2.rjmedia-content.app/media/podcast/mp3-192/2979-c.mp3"),
            Some("mp3")
        );
        assert_eq!(audio_extension("https://e.test/a.M4A?x=1"), Some("m4a"));
        assert_eq!(audio_extension("https://e.test/a.exe"), None);
    }
}
