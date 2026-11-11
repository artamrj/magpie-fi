//! Small file helpers shared by the scan, dedup, library and filing code.

use anyhow::Result;
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    path::{Path, PathBuf},
};

/// `(size, modification time in ns)`: tells whether a file changed since it
/// was last looked at.
pub async fn stamp(path: &Path) -> Result<(i64, i64)> {
    let stamp = crate::infrastructure::file_stamp::FileStamp::of(path).await?;
    Ok((stamp.size, stamp.mtime_ns))
}

/// Hex SHA-256 of a file's bytes, computed off the async runtime.
pub async fn sha256(path: &Path) -> Result<String> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let mut file = std::fs::File::open(path)?;
        let mut digest = Sha256::new();
        let mut buffer = vec![0_u8; 128 * 1024];
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            digest.update(&buffer[..count]);
        }
        Ok(hex::encode(digest.finalize()))
    })
    .await?
}

/// Changes a library song on a hidden copy in the same folder, then swaps the
/// copy in with one rename, so Navidrome or a player never sees a half-written
/// file. `edit` returns whether it changed anything; without a change, or on an
/// error, the copy is removed and the song is left as it was. Blocking.
///
/// Before the swap the copy is checked (it must still be the whole song) and
/// written out to the disk, because after the rename the old file is gone: a
/// tag writer that cut the audio, or a power cut, must not cost the song.
pub fn edit_on_copy(
    song: &Path,
    label: &str,
    edit: impl FnOnce(&Path) -> Result<bool>,
) -> Result<bool> {
    use anyhow::Context;
    let name = song
        .file_name()
        .context("song has no name")?
        .to_string_lossy();
    let extension = song
        .extension()
        .map(|extension| extension.to_string_lossy().into_owned())
        .unwrap_or_default();
    let copy = song.with_file_name(format!(".{name}.magpie-fi-{label}.{extension}"));
    std::fs::copy(song, &copy)?;
    let result = edit(&copy).and_then(|changed| {
        if changed {
            still_the_whole_song(song, &copy)?;
            std::fs::File::open(&copy)?.sync_all()?;
            std::fs::rename(&copy, song)?;
        }
        Ok(changed)
    });
    if !matches!(result, Ok(true)) {
        let _ = std::fs::remove_file(&copy);
    }
    result
}

/// The edited copy must still read as audio of the same length as the song.
/// A song that could not be read before the edit is not judged.
fn still_the_whole_song(song: &Path, copy: &Path) -> Result<()> {
    use anyhow::Context;
    let Ok(before) = crate::domain::audio::read(song) else {
        return Ok(());
    };
    let after = crate::domain::audio::read(copy)
        .context("the changed copy is not readable audio any more; the song was left as it was")?;
    if (before.duration - after.duration).abs() > 1.0 {
        anyhow::bail!(
            "the changed copy is {:.0} s long, the song {:.0} s; the song was left as it was",
            after.duration,
            before.duration
        );
    }
    Ok(())
}

/// `Song.mp3` → `Song (2).mp3`.
pub fn numbered(path: &Path, number: usize) -> PathBuf {
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name = match path.extension() {
        Some(extension) => format!("{stem} ({number}).{}", extension.to_string_lossy()),
        None => format!("{stem} ({number})"),
    };
    path.with_file_name(name)
}

/// Supported audio files under `root`, plus any walk errors. Hidden and NAS
/// system folders (`.incoming`, `.trash`, `@eaDir`) are skipped: other
/// programs keep unfinished files there. Blocking; run it off the runtime.
pub fn audio_files(root: &Path) -> (Vec<PathBuf>, Vec<String>) {
    let mut errors = Vec::new();
    let files = walkdir::WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            entry.depth() == 0
                || !entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with('.') || name.starts_with('@'))
        })
        .filter_map(|entry| {
            entry
                .map_err(|error| errors.push(format!("{error:#}")))
                .ok()
        })
        .filter(|entry| {
            entry.file_type().is_file() && crate::domain::audio::is_supported(entry.path())
        })
        .map(walkdir::DirEntry::into_path)
        .collect();
    (files, errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(directory: &Path) -> Option<PathBuf> {
        let path = directory.join("01 - Song.mp3");
        let status = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
            .arg("sine=frequency=440:duration=3")
            .args(["-q:a", "9"])
            .arg(&path)
            .status()
            .ok()?;
        status.success().then_some(path)
    }

    fn hidden_files(directory: &Path) -> usize {
        std::fs::read_dir(directory)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with('.'))
            .count()
    }

    #[test]
    fn an_edit_that_breaks_the_audio_leaves_the_song_as_it_was() {
        let directory = tempfile::tempdir().unwrap();
        let Some(song) = song(directory.path()) else {
            return;
        };
        let before = std::fs::read(&song).unwrap();

        // A tag writer that destroys the file.
        let broken = edit_on_copy(&song, "test", |copy| {
            std::fs::write(copy, b"no audio left")?;
            Ok(true)
        });
        assert!(broken.is_err());
        assert_eq!(std::fs::read(&song).unwrap(), before);

        // One that loses most of the song.
        let cut = edit_on_copy(&song, "test", |copy| {
            let status = std::process::Command::new("ffmpeg")
                .args([
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-y",
                    "-f",
                    "lavfi",
                    "-i",
                ])
                .arg("sine=frequency=440:duration=1")
                .args(["-q:a", "9"])
                .arg(copy)
                .status()?;
            assert!(status.success());
            Ok(true)
        });
        assert!(cut.is_err());
        assert_eq!(std::fs::read(&song).unwrap(), before);

        // One that fails, and one that changes nothing.
        assert!(edit_on_copy(&song, "test", |_| anyhow::bail!("cannot write")).is_err());
        assert!(!edit_on_copy(&song, "test", |_| Ok(false)).unwrap());
        assert_eq!(std::fs::read(&song).unwrap(), before);
        assert_eq!(hidden_files(directory.path()), 0, "no copy is left behind");
    }

    #[test]
    fn a_good_edit_replaces_the_song() {
        let directory = tempfile::tempdir().unwrap();
        let Some(song) = song(directory.path()) else {
            return;
        };
        let before = std::fs::read(&song).unwrap();
        let changed = edit_on_copy(&song, "test", |copy| {
            crate::infrastructure::media::tag_writer::embed_lyrics(copy, "la la la")?;
            Ok(true)
        })
        .unwrap();
        assert!(changed);
        assert_ne!(std::fs::read(&song).unwrap(), before);
        assert_eq!(
            crate::infrastructure::media::tag_writer::read_lyrics(&song)
                .unwrap()
                .as_deref(),
            Some("la la la")
        );
        assert_eq!(hidden_files(directory.path()), 0);
    }
}
