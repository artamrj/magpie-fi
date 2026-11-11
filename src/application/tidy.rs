//! Songs filed before lyrics went inside them get the lyrics from the `.lrc` or `.txt` next
//! to them. The file stays: Navidrome reads it before the lyrics inside the song. Runs after
//! the library index; with nothing left to do it only reads folders.

use crate::{
    app::AppState,
    domain::audio,
    infrastructure::{lyrics, media::tag_writer},
};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Eq, PartialEq)]
pub struct Tidied {
    /// Songs that got the lyrics from the file next to them.
    pub lyrics: usize,
}

pub async fn embed_sidecars(state: &AppState) -> Result<Tidied> {
    let root = PathBuf::from(&state.config.output_dir);
    let songs = tokio::task::spawn_blocking(move || find_lyrics_files(&root)).await?;

    let mut tidied = Tidied::default();
    for (song, sidecar) in songs {
        let (song_path, sidecar_path) = (song.clone(), sidecar.clone());
        let embedded =
            tokio::task::spawn_blocking(move || embed_one(&song_path, &sidecar_path)).await?;
        match embedded {
            Ok(true) => tidied.lyrics += 1,
            Ok(false) => {}
            Err(error) => {
                tracing::warn!(error = format!("{error:#}"), song = %song.display(), "could not put the lyrics inside the song");
            }
        }
    }
    Ok(tidied)
}

/// Songs with a `.lrc`/`.txt` next to them. Hidden folders and files (work in progress,
/// NAS metadata) are skipped.
fn find_lyrics_files(library_root: &Path) -> Vec<(PathBuf, PathBuf)> {
    let mut songs = Vec::new();
    let entries = walkdir::WalkDir::new(library_root)
        .into_iter()
        .filter_entry(|entry| entry.depth() == 0 || !is_hidden(entry.path()))
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file());
    for entry in entries {
        let path = entry.path();
        if is_taggable(path)
            && let Some(sidecar) = lyrics::embeddable_sidecar(path)
        {
            songs.push((path.to_path_buf(), sidecar));
        }
    }
    songs
}

fn is_hidden(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with('.'))
}

/// WAV and AIFF are never tagged, so their sidecars stay.
fn is_taggable(path: &Path) -> bool {
    audio::is_supported(path)
        && !path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                matches!(
                    extension.to_ascii_lowercase().as_str(),
                    "wav" | "aiff" | "aif"
                )
            })
}

/// Puts the lyrics inside a copy of the song, checks them, then swaps the copy
/// in. A song that already has lyrics inside keeps them; the file is untouched.
/// Returns whether the song changed.
fn embed_one(song: &Path, sidecar: &Path) -> Result<bool> {
    if tag_writer::read_lyrics(song)?.is_some() {
        return Ok(false);
    }
    let text = std::fs::read_to_string(sidecar)
        .with_context(|| format!("could not read {}", sidecar.display()))?;
    let text = text.trim_end();
    if text.trim().is_empty() {
        return Ok(false);
    }
    let text = text.to_owned();
    crate::application::files::edit_on_copy(song, "tidy", move |copy| {
        tag_writer::embed_lyrics(copy, &text)?;
        Ok(true)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::providers::Candidate;

    #[tokio::test]
    async fn lyrics_files_go_inside_the_songs_and_stay() {
        if std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_err()
        {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let mut config = crate::config::Config::with_folders(
            &directory.path().join("music"),
            &directory.path().join("data"),
        );
        let album = directory.path().join("music/library/Artist/Album (2020)");
        std::fs::create_dir_all(&album).unwrap();
        config.output_dir = directory
            .path()
            .join("music/library")
            .to_string_lossy()
            .into_owned();
        let pool =
            crate::infrastructure::db::connect(directory.path().join("t.sqlite").to_str().unwrap())
                .await
                .unwrap();
        let state = AppState::new(config, pool);

        let song = album.join("01 - Song.mp3");
        let status = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
            .arg("sine=frequency=440:duration=0.1")
            .args(["-q:a", "9"])
            .arg(&song)
            .status()
            .unwrap();
        assert!(status.success());
        let candidate = Candidate {
            title: "Song".into(),
            artist: "Artist".into(),
            ..Default::default()
        };
        tag_writer::write(
            &song,
            &candidate,
            Some(tag_writer::test_artwork_png()),
            None,
        )
        .unwrap();
        std::fs::write(album.join("01 - Song.lrc"), "[00:01.00] hello\n").unwrap();
        std::fs::write(album.join("cover.png"), tag_writer::test_artwork_png()).unwrap();
        std::fs::write(album.join("folder.jpg"), b"owner's own image").unwrap();

        let tidied = embed_sidecars(&state).await.unwrap();

        assert_eq!(tidied, Tidied { lyrics: 1 });
        assert_eq!(
            tag_writer::read_lyrics(&song).unwrap().as_deref(),
            Some("[00:01.00] hello")
        );
        // Navidrome reads these before the song's own tags: they stay.
        assert!(album.join("01 - Song.lrc").exists());
        assert!(album.join("cover.png").exists());
        assert!(album.join("folder.jpg").exists());
        // Nothing is left to do the second time.
        assert_eq!(embed_sidecars(&state).await.unwrap(), Tidied::default());
    }
}
