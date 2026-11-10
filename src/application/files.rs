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
            std::fs::rename(&copy, song)?;
        }
        Ok(changed)
    });
    if !matches!(result, Ok(true)) {
        let _ = std::fs::remove_file(&copy);
    }
    result
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
