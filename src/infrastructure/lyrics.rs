//! Lyrics from LRCLIB (https://lrclib.net), saved next to the song so Navidrome
//! shows them: `.lrc` for synced lyrics, `.txt` for plain lyrics.

use anyhow::Result;
use serde::Deserialize;
use std::path::{Path, PathBuf};

const LRCLIB_GET: &str = "https://lrclib.net/api/get";

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LrclibTrack {
    #[serde(default)]
    instrumental: bool,
    plain_lyrics: Option<String>,
    synced_lyrics: Option<String>,
}

#[derive(Debug, Eq, PartialEq)]
pub enum Saved {
    Synced(PathBuf),
    Plain(PathBuf),
    /// Instrumental, not found, or a sidecar already exists.
    Nothing,
}

/// Looks the song up (LRCLIB needs the length within ±2 s) and writes the
/// sidecar. Never overwrites an existing `.lrc`/`.txt`.
pub async fn fetch_and_save(
    client: &reqwest::Client,
    audio: &Path,
    title: &str,
    artist: &str,
    album: Option<&str>,
    duration: Option<f64>,
) -> Result<Saved> {
    if sidecar_exists(audio).await {
        return Ok(Saved::Nothing);
    }
    let mut query = vec![
        ("track_name", title.to_owned()),
        ("artist_name", artist.to_owned()),
    ];
    if let Some(album) = album.filter(|album| !album.trim().is_empty()) {
        query.push(("album_name", album.to_owned()));
    }
    if let Some(duration) = duration.filter(|duration| *duration > 0.0) {
        query.push(("duration", format!("{}", duration.round() as i64)));
    }
    let response = client.get(LRCLIB_GET).query(&query).send().await?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(Saved::Nothing);
    }
    let track: LrclibTrack = response.error_for_status()?.json().await?;
    save(audio, &track).await
}

async fn save(audio: &Path, track: &LrclibTrack) -> Result<Saved> {
    if track.instrumental {
        return Ok(Saved::Nothing);
    }
    let text = |value: &Option<String>| {
        crate::infrastructure::providers::nonempty(value.as_deref())
            .map(|value| format!("{value}\n"))
    };
    if let Some(synced) = text(&track.synced_lyrics) {
        let target = audio.with_extension("lrc");
        tokio::fs::write(&target, synced).await?;
        return Ok(Saved::Synced(target));
    }
    if let Some(plain) = text(&track.plain_lyrics) {
        let target = audio.with_extension("txt");
        tokio::fs::write(&target, plain).await?;
        return Ok(Saved::Plain(target));
    }
    Ok(Saved::Nothing)
}

async fn sidecar_exists(audio: &Path) -> bool {
    for extension in ["lrc", "elrc", "ttml", "txt"] {
        if tokio::fs::try_exists(audio.with_extension(extension))
            .await
            .unwrap_or(false)
        {
            return true;
        }
    }
    false
}

/// The lyric files that belong to a song (moved along when the song moves).
pub fn sidecars(audio: &Path) -> Vec<PathBuf> {
    ["lrc", "elrc", "ttml", "txt"]
        .into_iter()
        .map(|extension| audio.with_extension(extension))
        .filter(|path| path.is_file())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn synced_lyrics_win_and_instrumentals_write_nothing() {
        let directory = tempfile::tempdir().unwrap();
        let audio = directory.path().join("01 - Song.m4a");
        let both = LrclibTrack {
            instrumental: false,
            plain_lyrics: Some("hello".into()),
            synced_lyrics: Some("[00:01.00] hello".into()),
        };
        assert_eq!(
            save(&audio, &both).await.unwrap(),
            Saved::Synced(directory.path().join("01 - Song.lrc"))
        );
        assert!(sidecar_exists(&audio).await);
        assert_eq!(sidecars(&audio).len(), 1);

        let other = directory.path().join("02 - Other.m4a");
        let plain = LrclibTrack {
            plain_lyrics: Some("words".into()),
            ..LrclibTrack::default()
        };
        assert!(matches!(
            save(&other, &plain).await.unwrap(),
            Saved::Plain(_)
        ));

        let instrumental = LrclibTrack {
            instrumental: true,
            synced_lyrics: Some("[00:01.00] ignored".into()),
            ..LrclibTrack::default()
        };
        let third = directory.path().join("03 - Instrumental.m4a");
        assert_eq!(save(&third, &instrumental).await.unwrap(), Saved::Nothing);
    }
}
