//! The library: where every song is filed (BLUEPRINT.md §5), the trash that
//! keeps originals for a while, and the permanent index that remembers every
//! recording in the library so the same song is never filed twice.

use crate::{
    application::input_dedup::{self, RecordingEvidence},
    domain::{audio, credits},
    infrastructure::{fingerprint_cache, media::fingerprint, providers::Candidate},
};
use anyhow::{Context, Result};
use chrono::Utc;
use sqlx::SqlitePool;
use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use unicode_normalization::UnicodeNormalization;

const DURATION_TOLERANCE_SECONDS: f64 = 3.0;
const MAX_NAME_BYTES: usize = 180;
const VARIOUS_ARTISTS: &str = "Various Artists";

// ── Layout ──────────────────────────────────────────────────────────────────

/// `{AlbumArtist}/{Album} ({Year})[ [Single]|[EP]]/{Disc-}{Track:02} - {Title}.{ext}`
/// Compilations live under "Various Artists" and keep the track artist in the
/// file name. `extension` comes without the dot.
pub fn destination(library: &Path, candidate: &Candidate, extension: &str) -> PathBuf {
    let names = credits::normalize_featured(&candidate.artist, &candidate.title);
    let title = safe_name(&names.title, "Unknown Title");
    let track_artist = safe_name(&names.artist, "Unknown Artist");
    let compilation = is_compilation(candidate);

    let album_artist = if compilation {
        VARIOUS_ARTISTS.to_owned()
    } else {
        let declared = candidate
            .album_artist
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .map(str::to_owned)
            .or_else(|| {
                candidate
                    .album_artist_credits
                    .first()
                    .map(|credit| credit.name.clone())
            })
            .or_else(|| names.artists.first().map(|credit| credit.name.clone()))
            .unwrap_or_else(|| names.artist.clone());
        safe_name(&declared, "Unknown Artist")
    };

    let album = candidate
        .album
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(&names.title);
    let mut release = safe_name(album, "Unknown Album");
    if let Some(year) = release_year(candidate) {
        release = format!("{release} ({year})");
    }
    match release_kind(candidate) {
        ReleaseKind::Single => release.push_str(" [Single]"),
        ReleaseKind::Ep => release.push_str(" [EP]"),
        ReleaseKind::Album => {}
    }
    let release = truncate_name(&release);

    let mut file = String::new();
    let multi_disc = candidate.disc_total.is_some_and(|total| total > 1)
        || candidate.disc_number.is_some_and(|disc| disc > 1);
    if let Some(track) = candidate.track_number.filter(|track| *track > 0) {
        if multi_disc && let Some(disc) = candidate.disc_number.filter(|disc| *disc > 0) {
            file.push_str(&format!("{disc}-"));
        }
        file.push_str(&format!("{track:02} - "));
    }
    if compilation {
        file.push_str(&format!("{track_artist} - "));
    }
    file.push_str(&title);
    let file = truncate_name(&file);
    let extension = extension.trim_start_matches('.').to_ascii_lowercase();

    library
        .join(album_artist)
        .join(release)
        .join(format!("{file}.{extension}"))
}

#[derive(Debug, Eq, PartialEq)]
enum ReleaseKind {
    Album,
    Single,
    Ep,
}

fn release_kind(candidate: &Candidate) -> ReleaseKind {
    match candidate
        .release_type
        .as_deref()
        .map(|value| value.trim().to_ascii_lowercase())
        .as_deref()
    {
        Some("single") => ReleaseKind::Single,
        Some("ep") => ReleaseKind::Ep,
        _ => ReleaseKind::Album,
    }
}

fn is_compilation(candidate: &Candidate) -> bool {
    candidate.is_compilation
        || candidate
            .album_artist
            .as_deref()
            .is_some_and(|artist| artist.trim().eq_ignore_ascii_case(VARIOUS_ARTISTS))
        || candidate
            .release_type
            .as_deref()
            .is_some_and(|kind| kind.trim().eq_ignore_ascii_case("compilation"))
}

/// Keeps an album together. Catalogs often disagree about a year (the single's
/// year for one song, the album's for the next), which would give one album
/// two folders and two albums in Navidrome. When the album artist already has
/// a folder for this album (same name and kind, another year), the song takes
/// that folder's year.
pub fn align_with_existing_release(library: &Path, candidate: &mut Candidate) {
    let probe = destination(library, candidate, "x");
    let (Some(release_folder), Some(artist_folder)) =
        (probe.parent(), probe.parent().and_then(Path::parent))
    else {
        return;
    };
    let Some(current) = release_folder
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
    else {
        return;
    };
    let suffix = match release_kind(candidate) {
        ReleaseKind::Single => " [Single]",
        ReleaseKind::Ep => " [EP]",
        ReleaseKind::Album => "",
    };
    let album = current
        .strip_suffix(suffix)
        .map(|name| strip_year(name).0.to_owned())
        .unwrap_or_else(|| current.clone());
    let Ok(entries) = std::fs::read_dir(artist_folder) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == current || !entry.path().is_dir() {
            continue;
        }
        let Some(rest) = name.strip_suffix(suffix) else {
            continue;
        };
        if suffix.is_empty() && (rest.ends_with(" [Single]") || rest.ends_with(" [EP]")) {
            continue;
        }
        if let (existing_album, Some(year)) = strip_year(rest)
            && existing_album == album
        {
            candidate.year = Some(year.to_owned());
            if candidate
                .release_date
                .as_deref()
                .is_some_and(|date| !date.starts_with(year))
            {
                candidate.release_date = None;
            }
            return;
        }
    }
}

/// "Discovery (2001)" → ("Discovery", Some("2001")).
fn strip_year(name: &str) -> (&str, Option<&str>) {
    if let Some(open) = name.rfind(" (")
        && name.ends_with(')')
    {
        let year = &name[open + 2..name.len() - 1];
        if year.len() == 4 && year.chars().all(|character| character.is_ascii_digit()) {
            return (&name[..open], Some(year));
        }
    }
    (name, None)
}

fn release_year(candidate: &Candidate) -> Option<String> {
    [candidate.year.as_deref(), candidate.release_date.as_deref()]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find_map(|value| {
            let year = value.get(..4)?;
            (year.chars().all(|character| character.is_ascii_digit()) && year != "0000")
                .then(|| year.to_owned())
        })
}

/// One path component: Unicode NFC (macOS/SMB otherwise create look-alike
/// duplicates), no characters that Windows/SMB reject, no leading or trailing
/// dots and spaces, at most 180 bytes.
pub fn safe_name(value: &str, fallback: &str) -> String {
    let cleaned = value
        .nfc()
        .map(|character| {
            if character.is_control()
                || matches!(
                    character,
                    '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'
                )
            {
                ' '
            } else {
                character
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let cleaned = truncate_name(cleaned.trim_matches([' ', '.']));
    if cleaned.is_empty() {
        fallback.to_owned()
    } else {
        cleaned
    }
}

fn truncate_name(value: &str) -> String {
    if value.len() <= MAX_NAME_BYTES {
        return value.to_owned();
    }
    let mut end = MAX_NAME_BYTES;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].trim_end_matches([' ', '.']).to_owned()
}

// ── Trash ───────────────────────────────────────────────────────────────────

/// Moves a file into `trash/<date>/…`, keeping its path relative to `base`
/// (the inbox or the library) so you can tell where it came from. Never
/// overwrites: a numbered name is used when the target exists.
pub async fn move_to_trash(trash: &Path, base: &Path, file: &Path) -> Result<PathBuf> {
    let relative = file
        .strip_prefix(base)
        .ok()
        .filter(|relative| !relative.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .or_else(|| file.file_name().map(PathBuf::from))
        .context("file to trash has no name")?;
    let day = Utc::now().format("%Y-%m-%d").to_string();
    let target = trash.join(day).join(relative);
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let mut candidate = target.clone();
    let mut number = 2;
    while tokio::fs::try_exists(&candidate).await? {
        candidate = numbered(&target, number);
        number += 1;
    }
    match tokio::fs::rename(file, &candidate).await {
        Ok(()) => {}
        // A different filesystem (should not happen with the recommended layout).
        Err(error) if error.raw_os_error() == Some(libc_exdev()) => {
            tokio::fs::copy(file, &candidate).await?;
            tokio::fs::remove_file(file).await?;
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("could not move {} to trash", file.display()));
        }
    }
    remove_empty_parents(file, base).await;
    Ok(candidate)
}

const fn libc_exdev() -> i32 {
    18
}

fn numbered(path: &Path, number: usize) -> PathBuf {
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

/// Removes folders left empty after a move, up to (not including) `base`.
async fn remove_empty_parents(file: &Path, base: &Path) {
    let mut current = file.parent();
    while let Some(folder) = current {
        if folder == base || !folder.starts_with(base) {
            break;
        }
        if tokio::fs::remove_dir(folder).await.is_err() {
            break;
        }
        current = folder.parent();
    }
}

/// Deletes trash days older than `days`. Trash folders are named `YYYY-MM-DD`.
pub async fn purge_trash(trash: &Path, days: u64) -> Result<usize> {
    let cutoff = (Utc::now() - chrono::Duration::days(i64::try_from(days).unwrap_or(i64::MAX)))
        .format("%Y-%m-%d")
        .to_string();
    let mut removed = 0;
    let Ok(mut entries) = tokio::fs::read_dir(trash).await else {
        return Ok(0);
    };
    while let Some(entry) = entries.next_entry().await? {
        let name = entry.file_name().to_string_lossy().into_owned();
        let is_day =
            name.len() == 10 && chrono::NaiveDate::parse_from_str(&name, "%Y-%m-%d").is_ok();
        if is_day && name < cutoff && entry.file_type().await?.is_dir() {
            tokio::fs::remove_dir_all(entry.path()).await?;
            removed += 1;
        }
    }
    Ok(removed)
}

// ── Index ───────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct LibraryEntry {
    pub path: String,
    pub duration: Option<f64>,
    pub content_key: Option<String>,
    pub isrc: Option<String>,
    pub recording_id: Option<String>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub format: String,
    pub bitrate: Option<u32>,
    pub source: String,
}

/// Saves (or refreshes) one library file in the index.
pub async fn remember(pool: &SqlitePool, entry: &LibraryEntry) -> Result<()> {
    let (size, modified) = file_identity(Path::new(&entry.path)).await?;
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO library_items(path,file_size,file_mtime_ns,duration,content_key,isrc,
           recording_id,title,artist,album,format,bitrate,source,added_at,checked_at)
         VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)
         ON CONFLICT(path) DO UPDATE SET
           file_size=excluded.file_size, file_mtime_ns=excluded.file_mtime_ns,
           duration=excluded.duration, content_key=excluded.content_key, isrc=excluded.isrc,
           recording_id=excluded.recording_id, title=excluded.title, artist=excluded.artist,
           album=excluded.album, format=excluded.format, bitrate=excluded.bitrate,
           checked_at=excluded.checked_at",
    )
    .bind(&entry.path)
    .bind(size)
    .bind(modified)
    .bind(entry.duration)
    .bind(&entry.content_key)
    .bind(entry.isrc.as_deref().and_then(input_dedup::normalize_isrc))
    .bind(&entry.recording_id)
    .bind(&entry.title)
    .bind(&entry.artist)
    .bind(&entry.album)
    .bind(&entry.format)
    .bind(entry.bitrate.map(i64::from))
    .bind(&entry.source)
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn forget(pool: &SqlitePool, path: &Path) -> Result<()> {
    sqlx::query("DELETE FROM library_items WHERE path=?")
        .bind(path.to_string_lossy().as_ref())
        .execute(pool)
        .await?;
    Ok(())
}

type Row = (
    String,
    Option<f64>,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
    Option<i64>,
);

/// Finds a library file that is provably the same recording: the same content
/// (identical file or identical audio fingerprint), the same ISRC or the same
/// MusicBrainz recording — the last three only with lengths within 3 seconds.
/// Rows whose file has disappeared are dropped on the way.
pub async fn find_same_recording(
    pool: &SqlitePool,
    evidence: &RecordingEvidence,
    recording_id: Option<&str>,
) -> Result<Option<LibraryEntry>> {
    let isrc = evidence
        .isrc
        .as_deref()
        .and_then(input_dedup::normalize_isrc);
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT path,duration,content_key,isrc,recording_id,format,bitrate FROM library_items
         WHERE (content_key IS NOT NULL AND content_key=?)
            OR (isrc IS NOT NULL AND isrc=?)
            OR (recording_id IS NOT NULL AND recording_id=?)
         ORDER BY path",
    )
    .bind(&evidence.content_key)
    .bind(&isrc)
    .bind(recording_id)
    .fetch_all(pool)
    .await?;
    for (path, duration, content_key, row_isrc, row_recording, format, bitrate) in rows {
        let exact = evidence
            .content_key
            .as_deref()
            .is_some_and(|key| key.starts_with("sha256:") && content_key.as_deref() == Some(key));
        let close = match (evidence.duration, duration) {
            (Some(left), Some(right)) => (left - right).abs() <= DURATION_TOLERANCE_SECONDS,
            _ => false,
        };
        let same_fingerprint = evidence
            .content_key
            .as_deref()
            .is_some_and(|key| key.starts_with("fp:") && content_key.as_deref() == Some(key));
        let same_isrc = isrc.is_some() && row_isrc == isrc;
        let same_recording = recording_id.is_some() && row_recording.as_deref() == recording_id;
        if !(exact || (close && (same_fingerprint || same_isrc || same_recording))) {
            continue;
        }
        if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
            forget(pool, Path::new(&path)).await?;
            continue;
        }
        return Ok(Some(LibraryEntry {
            path,
            duration,
            content_key,
            isrc: row_isrc,
            recording_id: row_recording,
            format,
            bitrate: bitrate.and_then(|value| u32::try_from(value).ok()),
            ..LibraryEntry::default()
        }));
    }
    Ok(None)
}

/// True when `new` should replace `existing`: only a lossless file replaces a
/// lossy one. Anything else keeps what is already in the library.
pub fn is_upgrade(new: &RecordingEvidence, existing: &LibraryEntry) -> bool {
    input_dedup::is_lossless(&new.format) && !input_dedup::is_lossless(&existing.format)
}

/// Walks the library and indexes files that are new or changed since the last
/// pass (music you already had, files you added by hand, files whose tags were
/// rewritten). Entries for deleted files are removed.
pub async fn index_library(pool: &SqlitePool, library: &Path) -> Result<usize> {
    let root = library.to_path_buf();
    let files = tokio::task::spawn_blocking(move || {
        walkdir::WalkDir::new(&root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|entry| {
                entry.depth() == 0
                    || !entry
                        .file_name()
                        .to_str()
                        .is_some_and(|name| name.starts_with('.') || name.starts_with('@'))
            })
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file() && audio::is_supported(entry.path()))
            .map(walkdir::DirEntry::into_path)
            .collect::<Vec<_>>()
    })
    .await?;

    let known: Vec<(String, i64, i64)> =
        sqlx::query_as("SELECT path,file_size,file_mtime_ns FROM library_items")
            .fetch_all(pool)
            .await?;
    let known: std::collections::HashMap<String, (i64, i64)> = known
        .into_iter()
        .map(|(path, size, modified)| (path, (size, modified)))
        .collect();
    let present: std::collections::HashSet<String> = files
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    for path in known.keys().filter(|path| !present.contains(*path)) {
        forget(pool, Path::new(path)).await?;
    }

    let mut indexed = 0;
    for path in files {
        let text = path.to_string_lossy().into_owned();
        let Ok(identity) = file_identity(&path).await else {
            continue;
        };
        if known.get(&text) == Some(&identity) {
            continue;
        }
        match describe_file(pool, &path).await {
            Ok(entry) => {
                remember(pool, &entry).await?;
                indexed += 1;
            }
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "could not index library file")
            }
        }
    }
    Ok(indexed)
}

/// Builds an index entry from a file's tags and audio fingerprint.
async fn describe_file(pool: &SqlitePool, path: &Path) -> Result<LibraryEntry> {
    let owned = path.to_path_buf();
    let (info, ids) = tokio::task::spawn_blocking(move || {
        let info = audio::read(&owned)?;
        Ok::<_, anyhow::Error>((info, read_identifiers(&owned)))
    })
    .await??;
    let content_key = match fingerprint_cache::get_or_calculate(pool, path, || {
        fingerprint::calculate(path)
    })
    .await
    {
        Ok(result) => input_dedup::fingerprint_key(&result.fingerprint),
        Err(_) => Some(input_dedup::hash_key(
            &input_dedup::sha256_cached(pool, path).await?,
        )),
    };
    Ok(LibraryEntry {
        path: path.to_string_lossy().into_owned(),
        duration: (info.duration > 0.0).then_some(info.duration),
        content_key,
        isrc: ids.0,
        recording_id: ids.1,
        title: info.title,
        artist: info.artist,
        album: info.album,
        format: info.format,
        bitrate: info.bitrate,
        source: "existing".into(),
    })
}

/// ISRC and MusicBrainz recording ID from the file's tags, when present.
fn read_identifiers(path: &Path) -> (Option<String>, Option<String>) {
    use lofty::{file::TaggedFileExt, probe::Probe, tag::ItemKey};
    let Ok(file) = Probe::open(path).and_then(|probe| probe.guess_file_type()?.read()) else {
        return (None, None);
    };
    let tag = file.primary_tag().or_else(|| file.first_tag());
    let get = |key: ItemKey| {
        tag.and_then(|tag| tag.get_string(key))
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
    };
    (get(ItemKey::Isrc), get(ItemKey::MusicBrainzRecordingId))
}

async fn file_identity(path: &Path) -> Result<(i64, i64)> {
    let metadata = tokio::fs::metadata(path).await?;
    let modified = metadata
        .modified()
        .unwrap_or(SystemTime::UNIX_EPOCH)
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_nanos();
    Ok((
        i64::try_from(metadata.len()).unwrap_or(i64::MAX),
        i64::try_from(modified).unwrap_or(i64::MAX),
    ))
}

/// Writes `cover.jpg` (or `.png`/`.webp`, matching the image) into the album
/// folder when the folder has no cover file yet. Navidrome prefers it over
/// embedded art.
pub async fn write_folder_cover(album_folder: &Path, image: &[u8]) -> Result<Option<PathBuf>> {
    let mut entries = tokio::fs::read_dir(album_folder).await?;
    while let Some(entry) = entries.next_entry().await? {
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        if name.starts_with("cover.") || name.starts_with("folder.") || name.starts_with("front.") {
            return Ok(None);
        }
    }
    let extension = match image::guess_format(image) {
        Ok(image::ImageFormat::Png) => "png",
        Ok(image::ImageFormat::WebP) => "webp",
        Ok(image::ImageFormat::Gif) => "gif",
        _ => "jpg",
    };
    let target = album_folder.join(format!("cover.{extension}"));
    let temporary = album_folder.join(format!(".cover.{extension}.part"));
    tokio::fs::write(&temporary, image).await?;
    tokio::fs::rename(&temporary, &target).await?;
    Ok(Some(target))
}

// ── Playlists ───────────────────────────────────────────────────────────────

/// Writes `library/Playlists/<name>.m3u8` with paths relative to the playlist
/// file, which Navidrome imports automatically. Songs outside the library are
/// left out. Returns true when the file changed.
pub async fn write_playlist(library: &Path, name: &str, songs: &[String]) -> Result<bool> {
    let folder = library.join("Playlists");
    tokio::fs::create_dir_all(&folder).await?;
    let mut content = format!("#EXTM3U\n#PLAYLIST:{}\n", name.replace('\n', " "));
    for song in songs {
        let Ok(relative) = Path::new(song).strip_prefix(library) else {
            continue;
        };
        content.push_str("../");
        content.push_str(&relative.to_string_lossy());
        content.push('\n');
    }
    let target = folder.join(format!("{}.m3u8", safe_name(name, "Playlist")));
    if tokio::fs::read_to_string(&target).await.ok().as_deref() == Some(content.as_str()) {
        return Ok(false);
    }
    let temporary = folder.join(format!(".{}.m3u8.part", safe_name(name, "Playlist")));
    tokio::fs::write(&temporary, &content).await?;
    tokio::fs::rename(&temporary, &target).await?;
    Ok(true)
}

/// Removes a file that magpie created itself (a download in staging) or moves
/// anything else to the trash.
pub async fn discard_original(
    trash: &Path,
    staging: &Path,
    inbox: &Path,
    file: &Path,
) -> Result<Option<PathBuf>> {
    if file.starts_with(staging) {
        tokio::fs::remove_file(file).await?;
        return Ok(None);
    }
    move_to_trash(trash, inbox, file).await.map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate() -> Candidate {
        Candidate {
            title: "Digital Love".into(),
            artist: "Daft Punk".into(),
            album: Some("Discovery".into()),
            album_artist: Some("Daft Punk".into()),
            track_number: Some(3),
            year: Some("2001".into()),
            release_type: Some("album".into()),
            ..Candidate::default()
        }
    }

    fn path(candidate: &Candidate, extension: &str) -> String {
        destination(Path::new("/lib"), candidate, extension)
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn normal_album_track() {
        assert_eq!(
            path(&candidate(), "flac"),
            "/lib/Daft Punk/Discovery (2001)/03 - Digital Love.flac"
        );
    }

    #[test]
    fn multi_disc_tracks_get_a_disc_prefix() {
        let mut value = candidate();
        value.disc_number = Some(2);
        value.disc_total = Some(2);
        value.track_number = Some(1);
        assert_eq!(
            path(&value, "flac"),
            "/lib/Daft Punk/Discovery (2001)/2-01 - Digital Love.flac"
        );
        value.disc_number = Some(1);
        assert!(path(&value, "flac").ends_with("/1-01 - Digital Love.flac"));
    }

    #[test]
    fn singles_and_eps_are_marked() {
        let mut value = candidate();
        value.album = Some("Houdini".into());
        value.title = "Houdini".into();
        value.artist = "Dua Lipa".into();
        value.album_artist = Some("Dua Lipa".into());
        value.year = Some("2023-11-09".into());
        value.track_number = Some(1);
        value.release_type = Some("Single".into());
        assert_eq!(
            path(&value, "m4a"),
            "/lib/Dua Lipa/Houdini (2023) [Single]/01 - Houdini.m4a"
        );
        value.release_type = Some("ep".into());
        assert!(path(&value, "m4a").contains("/Houdini (2023) [EP]/"));
    }

    #[test]
    fn compilations_go_under_various_artists_with_the_artist_in_the_name() {
        let mut value = candidate();
        value.title = "Nightcall".into();
        value.artist = "Kavinsky".into();
        value.album = Some("Drive".into());
        value.album_artist = Some("Various Artists".into());
        value.year = Some("2011".into());
        value.track_number = Some(2);
        assert_eq!(
            path(&value, "M4A"),
            "/lib/Various Artists/Drive (2011)/02 - Kavinsky - Nightcall.m4a"
        );
    }

    #[test]
    fn featured_artists_stay_out_of_folder_names() {
        let mut value = candidate();
        value.artist = "Daft Punk feat. Romanthony".into();
        value.album_artist = None;
        let built = path(&value, "flac");
        assert!(built.starts_with("/lib/Daft Punk/"), "{built}");
    }

    #[test]
    fn missing_year_and_track_number_are_left_out() {
        let mut value = candidate();
        value.year = None;
        value.track_number = None;
        assert_eq!(
            path(&value, "mp3"),
            "/lib/Daft Punk/Discovery/Digital Love.mp3"
        );
    }

    #[test]
    fn a_new_song_joins_its_album_even_when_catalogs_disagree_on_the_year() {
        let directory = tempfile::tempdir().unwrap();
        let library = directory.path();
        std::fs::create_dir_all(library.join("Daft Punk/Discovery (2001)")).unwrap();
        std::fs::create_dir_all(library.join("Daft Punk/Discovery (2001) [Single]")).unwrap();
        let mut song = candidate();
        song.album = Some("Discovery".into());
        song.year = Some("2000".into());
        song.release_date = Some("2000-11-13".into());

        align_with_existing_release(library, &mut song);

        assert_eq!(song.year.as_deref(), Some("2001"));
        assert_eq!(song.release_date, None);
        assert!(path(&song, "m4a").contains("/Discovery (2001)/"));

        // A different album keeps its own year.
        let mut other = candidate();
        other.album = Some("Homework".into());
        other.year = Some("1997".into());
        align_with_existing_release(library, &mut other);
        assert_eq!(other.year.as_deref(), Some("1997"));
    }

    #[test]
    fn names_are_safe_on_every_filesystem() {
        assert_eq!(safe_name("AC/DC", "x"), "AC DC");
        assert_eq!(safe_name("  ...What?  ", "x"), "What");
        assert_eq!(safe_name("", "Unknown"), "Unknown");
        // Decomposed "é" (macOS) becomes the composed form.
        assert_eq!(safe_name("Beyonce\u{301}", "x"), "Beyoncé");
        assert_eq!(safe_name("گوگوش", "x"), "گوگوش");
        let long = "a".repeat(400);
        assert_eq!(safe_name(&long, "x").len(), MAX_NAME_BYTES);
    }

    #[tokio::test]
    async fn trash_keeps_the_relative_path_and_never_overwrites() {
        let directory = tempfile::tempdir().unwrap();
        let inbox = directory.path().join("inbox");
        let trash = directory.path().join("trash");
        let first = inbox.join("Artist/Album/song.mp3");
        tokio::fs::create_dir_all(first.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&first, b"one").await.unwrap();

        let moved = move_to_trash(&trash, &inbox, &first).await.unwrap();
        assert!(moved.ends_with("Artist/Album/song.mp3"));
        assert!(!first.exists());
        // The emptied folders inside the inbox are cleaned up, the inbox stays.
        assert!(!inbox.join("Artist").exists());
        assert!(inbox.exists());

        tokio::fs::create_dir_all(first.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&first, b"two").await.unwrap();
        let again = move_to_trash(&trash, &inbox, &first).await.unwrap();
        assert!(again.ends_with("Artist/Album/song (2).mp3"));
        assert_eq!(tokio::fs::read(&moved).await.unwrap(), b"one");
    }

    #[tokio::test]
    async fn old_trash_days_are_purged() {
        let directory = tempfile::tempdir().unwrap();
        let trash = directory.path();
        tokio::fs::create_dir_all(trash.join("2001-01-01/a"))
            .await
            .unwrap();
        let today = Utc::now().format("%Y-%m-%d").to_string();
        tokio::fs::create_dir_all(trash.join(&today)).await.unwrap();
        tokio::fs::create_dir_all(trash.join("not-a-day"))
            .await
            .unwrap();

        assert_eq!(purge_trash(trash, 30).await.unwrap(), 1);
        assert!(!trash.join("2001-01-01").exists());
        assert!(trash.join(today).exists());
        assert!(trash.join("not-a-day").exists());
    }

    async fn pool() -> (tempfile::TempDir, SqlitePool) {
        let directory = tempfile::tempdir().unwrap();
        let pool = crate::infrastructure::db::connect(
            directory.path().join("library.sqlite").to_str().unwrap(),
        )
        .await
        .unwrap();
        (directory, pool)
    }

    #[tokio::test]
    async fn the_same_recording_is_found_by_fingerprint_isrc_or_musicbrainz_id() {
        let (directory, pool) = pool().await;
        let file = directory.path().join("song.m4a");
        tokio::fs::write(&file, b"audio").await.unwrap();
        remember(
            &pool,
            &LibraryEntry {
                path: file.to_string_lossy().into_owned(),
                duration: Some(200.0),
                content_key: Some("fp:abc".into()),
                isrc: Some("us-um7-17-03861".into()),
                recording_id: Some("mbid-1".into()),
                format: "m4a".into(),
                source: "inbox".into(),
                ..LibraryEntry::default()
            },
        )
        .await
        .unwrap();

        let probe = |key: Option<&str>, isrc: Option<&str>, duration: f64| RecordingEvidence {
            path: PathBuf::from("/inbox/new.flac"),
            format: "flac".into(),
            bitrate: None,
            duration: Some(duration),
            content_key: key.map(str::to_owned),
            isrc: isrc.map(str::to_owned),
        };
        let hit = |evidence: RecordingEvidence, mbid: Option<&'static str>| {
            let pool = pool.clone();
            async move {
                find_same_recording(&pool, &evidence, mbid)
                    .await
                    .unwrap()
                    .is_some()
            }
        };
        assert!(hit(probe(Some("fp:abc"), None, 201.0), None).await);
        assert!(hit(probe(None, Some("USUM71703861"), 198.0), None).await);
        assert!(hit(probe(None, None, 200.5), Some("mbid-1")).await);
        // Same fingerprint but a very different length is a different recording.
        assert!(!hit(probe(Some("fp:abc"), None, 260.0), None).await);
        assert!(!hit(probe(Some("fp:other"), Some("GBAAA0000001"), 200.0), None).await);

        // A deleted file no longer counts and is removed from the index.
        tokio::fs::remove_file(&file).await.unwrap();
        assert!(!hit(probe(Some("fp:abc"), None, 200.0), None).await);
        let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM library_items")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(rows, 0);
    }

    #[test]
    fn only_lossless_replaces_lossy() {
        let new = |format: &str| RecordingEvidence {
            path: PathBuf::new(),
            format: format.into(),
            bitrate: Some(320_000),
            duration: None,
            content_key: None,
            isrc: None,
        };
        let existing = |format: &str| LibraryEntry {
            format: format.into(),
            ..LibraryEntry::default()
        };
        assert!(is_upgrade(&new("flac"), &existing("m4a")));
        assert!(!is_upgrade(&new("mp3"), &existing("m4a")));
        assert!(!is_upgrade(&new("flac"), &existing("flac")));
    }

    #[tokio::test]
    async fn playlists_use_relative_paths_and_skip_outside_songs() {
        let directory = tempfile::tempdir().unwrap();
        let library = directory.path().join("library");
        let songs = vec![
            library
                .join("A/Album (2001)/01 - One.m4a")
                .to_string_lossy()
                .into_owned(),
            "/elsewhere/two.m4a".to_owned(),
            library
                .join("B/Single (2020) [Single]/01 - Three.m4a")
                .to_string_lossy()
                .into_owned(),
        ];
        assert!(
            write_playlist(&library, "Arta - Road Trip", &songs)
                .await
                .unwrap()
        );
        let text = tokio::fs::read_to_string(library.join("Playlists/Arta - Road Trip.m3u8"))
            .await
            .unwrap();
        assert_eq!(
            text,
            "#EXTM3U\n#PLAYLIST:Arta - Road Trip\n../A/Album (2001)/01 - One.m4a\n../B/Single (2020) [Single]/01 - Three.m4a\n"
        );
        assert!(
            !write_playlist(&library, "Arta - Road Trip", &songs)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn own_downloads_are_deleted_and_other_files_trashed() {
        let directory = tempfile::tempdir().unwrap();
        let (trash, staging, inbox) = (
            directory.path().join("trash"),
            directory.path().join("staging"),
            directory.path().join("inbox"),
        );
        tokio::fs::create_dir_all(&staging).await.unwrap();
        tokio::fs::create_dir_all(&inbox).await.unwrap();
        let download = staging.join("w1.m4a");
        let dropped = inbox.join("mine.mp3");
        tokio::fs::write(&download, b"d").await.unwrap();
        tokio::fs::write(&dropped, b"m").await.unwrap();

        assert_eq!(
            discard_original(&trash, &staging, &inbox, &download)
                .await
                .unwrap(),
            None
        );
        assert!(!download.exists());
        let trashed = discard_original(&trash, &staging, &inbox, &dropped)
            .await
            .unwrap()
            .unwrap();
        assert!(trashed.starts_with(&trash));
    }

    #[tokio::test]
    async fn folder_cover_is_written_once() {
        let directory = tempfile::tempdir().unwrap();
        let png = {
            let image = image::RgbaImage::from_pixel(4, 4, image::Rgba([1, 2, 3, 255]));
            let mut output = std::io::Cursor::new(Vec::new());
            image::DynamicImage::ImageRgba8(image)
                .write_to(&mut output, image::ImageFormat::Png)
                .unwrap();
            output.into_inner()
        };
        let written = write_folder_cover(directory.path(), &png).await.unwrap();
        assert_eq!(written, Some(directory.path().join("cover.png")));
        assert_eq!(
            write_folder_cover(directory.path(), &png).await.unwrap(),
            None
        );
    }
}
