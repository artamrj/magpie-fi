//! Lyrics from LRCLIB (https://lrclib.net). Normally they go inside the song
//! (Navidrome reads LRC timestamps there as synced lyrics); with sidecars on they
//! are also saved next to it: `.lrc` for synced lyrics, `.txt` for plain lyrics.

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
    match lookup(client, title, artist, album, duration).await? {
        Some(track) => save(audio, &track).await,
        None => Ok(Saved::Nothing),
    }
}

/// The lyrics to put inside the song: synced (LRC text) when LRCLIB has them,
/// plain otherwise. `None` for instrumentals and songs LRCLIB does not know.
pub async fn fetch(
    client: &reqwest::Client,
    title: &str,
    artist: &str,
    album: Option<&str>,
    duration: Option<f64>,
) -> Result<Option<String>> {
    Ok(lookup(client, title, artist, album, duration)
        .await?
        .and_then(|track| best_text(&track)))
}

fn best_text(track: &LrclibTrack) -> Option<String> {
    if track.instrumental {
        return None;
    }
    [&track.synced_lyrics, &track.plain_lyrics]
        .into_iter()
        .find_map(|value| crate::infrastructure::providers::nonempty(value.as_deref()))
        .map(|value| value.trim_end().to_owned())
}

async fn lookup(
    client: &reqwest::Client,
    title: &str,
    artist: &str,
    album: Option<&str>,
    duration: Option<f64>,
) -> Result<Option<LrclibTrack>> {
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
    let response = client
        .get(LRCLIB_GET)
        .query(&query)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    Ok(Some(response.error_for_status()?.json().await?))
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

/// LRC text with timestamps, as opposed to plain lines.
pub fn is_synced(text: &str) -> bool {
    text.lines().take(20).any(|line| {
        let line = line.trim_start();
        line.len() >= 9
            && line.starts_with('[')
            && line.as_bytes()[1].is_ascii_digit()
            && line.as_bytes()[2].is_ascii_digit()
            && line.as_bytes()[3] == b':'
    })
}

/// Writes synced lyrics as `<song>.lrc` next to the song, which Navidrome reads before the
/// lyrics inside the file. An existing lyrics file of any kind is left alone.
pub async fn write_lrc(audio: &Path, text: &str) -> Result<Option<PathBuf>> {
    if !is_synced(text) || sidecar_exists(audio).await {
        return Ok(None);
    }
    let target = audio.with_extension("lrc");
    let temporary = audio.with_extension("lrc.part");
    tokio::fs::write(&temporary, format!("{}\n", text.trim_end())).await?;
    tokio::fs::rename(&temporary, &target).await?;
    Ok(Some(target))
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

/// The `.lrc` or `.txt` lyrics next to a song that can be put inside it.
pub fn embeddable_sidecar(audio: &Path) -> Option<PathBuf> {
    ["lrc", "txt"]
        .into_iter()
        .map(|extension| audio.with_extension(extension))
        .find(|path| path.is_file())
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

        // Inside the song: synced text first, nothing for instrumentals.
        assert_eq!(best_text(&both).as_deref(), Some("[00:01.00] hello"));
        assert_eq!(best_text(&plain).as_deref(), Some("words"));
        assert_eq!(best_text(&instrumental), None);
    }
}
