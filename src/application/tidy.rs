//! One file per song. Songs filed with separate lyrics and cover files get the
//! lyrics put inside them, and the extra files go to the trash (kept for
//! `MAGPIE_TRASH_DAYS`, so nothing is lost). Runs after the library index when
//! `MAGPIE_SIDECARS` is off; with nothing left to tidy it only reads folders.

use crate::{
    app::AppState,
    application::library,
    domain::audio,
    infrastructure::{lyrics, media::tag_writer},
};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Eq, PartialEq)]
pub struct Tidied {
    /// Lyrics files now inside their song.
    pub lyrics: usize,
    /// Album cover files removed because every song there has the cover inside.
    pub covers: usize,
}

/// The cover files magpie-fi writes per album folder. Other images
/// (`folder.jpg`, `front.png`) are the owner's and are left alone.
const COVER_FILES: [&str; 4] = ["cover.jpg", "cover.png", "cover.webp", "cover.gif"];

pub async fn embed_sidecars(state: &AppState) -> Result<Tidied> {
    let library_root = PathBuf::from(&state.config.output_dir);
    let trash = state.config.trash_dir.clone();
    let root = library_root.clone();
    let (songs, covers) = tokio::task::spawn_blocking(move || find_extra_files(&root)).await?;

    let mut tidied = Tidied::default();
    for (song, sidecar) in songs {
        let (song_path, sidecar_path) = (song.clone(), sidecar.clone());
        let embedded =
            tokio::task::spawn_blocking(move || embed_one(&song_path, &sidecar_path)).await?;
        match embedded {
            Ok(()) => {
                library::move_to_trash(&trash, &library_root, &sidecar).await?;
                tidied.lyrics += 1;
            }
            Err(error) => {
                tracing::warn!(error = format!("{error:#}"), song = %song.display(), "could not put the lyrics inside the song");
            }
        }
    }
    for cover in covers {
        let Some(folder) = cover.parent().map(Path::to_path_buf) else {
            continue;
        };
        if tokio::task::spawn_blocking(move || every_song_has_a_cover(&folder)).await? {
            library::move_to_trash(&trash, &library_root, &cover).await?;
            tidied.covers += 1;
        }
    }
    Ok(tidied)
}

/// Songs with a `.lrc`/`.txt` next to them, and magpie-fi's album cover files.
/// Hidden folders and files (work in progress, NAS metadata) are skipped.
fn find_extra_files(library_root: &Path) -> (Vec<(PathBuf, PathBuf)>, Vec<PathBuf>) {
    let mut songs = Vec::new();
    let mut covers = Vec::new();
    let entries = walkdir::WalkDir::new(library_root)
        .into_iter()
        .filter_entry(|entry| entry.depth() == 0 || !is_hidden(entry.path()))
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file());
    for entry in entries {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        if COVER_FILES.contains(&name.as_str()) {
            covers.push(path.to_path_buf());
        } else if is_taggable(path)
            && let Some(sidecar) = lyrics::embeddable_sidecar(path)
        {
            songs.push((path.to_path_buf(), sidecar));
        }
    }
    (songs, covers)
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
fn embed_one(song: &Path, sidecar: &Path) -> Result<()> {
    if tag_writer::read_lyrics(song)?.is_some() {
        return Ok(());
    }
    let text = std::fs::read_to_string(sidecar)
        .with_context(|| format!("could not read {}", sidecar.display()))?;
    let text = text.trim_end();
    if text.trim().is_empty() {
        return Ok(());
    }
    let name = song
        .file_name()
        .context("song has no name")?
        .to_string_lossy();
    let extension = song
        .extension()
        .map(|extension| extension.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temporary = song.with_file_name(format!(".{name}.magpie-fi-tidy.{extension}"));
    let result = std::fs::copy(song, &temporary)
        .map_err(anyhow::Error::from)
        .and_then(|_| tag_writer::embed_lyrics(&temporary, text))
        .and_then(|()| std::fs::rename(&temporary, song).map_err(anyhow::Error::from));
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

fn every_song_has_a_cover(folder: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return false;
    };
    let songs = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && !is_hidden(path) && audio::is_supported(path))
        .collect::<Vec<_>>();
    !songs.is_empty()
        && songs.iter().all(|song| {
            is_taggable(song) && tag_writer::read_artwork(song).ok().flatten().is_some()
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::providers::Candidate;

    #[tokio::test]
    async fn lyrics_and_covers_move_inside_the_songs() {
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

        assert_eq!(
            tidied,
            Tidied {
                lyrics: 1,
                covers: 1
            }
        );
        assert_eq!(
            tag_writer::read_lyrics(&song).unwrap().as_deref(),
            Some("[00:01.00] hello")
        );
        assert!(!album.join("01 - Song.lrc").exists());
        assert!(!album.join("cover.png").exists());
        assert!(album.join("folder.jpg").exists());
        // Nothing is left to do the second time.
        assert_eq!(embed_sidecars(&state).await.unwrap(), Tidied::default());
    }
}
