//! The library: where every song is filed (docs/BLUEPRINT.md §5), the trash that
//! keeps originals for a while, and the permanent index that remembers every
//! recording in the library so the same song is never filed twice.

use crate::{
    application::{
        files,
        input_dedup::{self, RecordingEvidence},
    },
    domain::{audio, credits},
    infrastructure::{
        fingerprint_cache,
        media::{
            fingerprint,
            tag_writer::{self, AlbumTags},
        },
        providers::Candidate,
    },
};
use anyhow::{Context, Result};
use chrono::Utc;
use sqlx::SqlitePool;
use std::path::{Path, PathBuf};
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

    // One folder per primary artist: the first album artist's identity name, so
    // a duet goes to `Ebi/…`, never `Ebi & Shadmehr Aghili/`. The other singers
    // get the song through the ARTISTS tag, not through a folder.
    let album_artist = if compilation {
        VARIOUS_ARTISTS.to_owned()
    } else {
        // A featured artist never names a folder, whatever a source sent.
        let without_featured = |name: &str| {
            credits::normalize_featured(name, "")
                .artists
                .first()
                .map_or_else(|| name.to_owned(), |credit| credit.name.clone())
        };
        let declared = candidate
            .album_artist_credits
            .first()
            .map(|credit| without_featured(&credit.name))
            .or_else(|| {
                candidate
                    .album_artist
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
                    .map(|value| {
                        credits::normalize_featured(value, "")
                            .artists
                            .first()
                            .map_or_else(|| value.to_owned(), |credit| credit.name.clone())
                    })
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReleaseKind {
    Album,
    Single,
    Ep,
}

/// The folder-name suffix of a release kind: ` [Single]`, ` [EP]` or nothing.
pub(crate) fn release_suffix(candidate: &Candidate) -> &'static str {
    match release_kind(candidate) {
        ReleaseKind::Single => " [Single]",
        ReleaseKind::Ep => " [EP]",
        ReleaseKind::Album => "",
    }
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

/// Keeps an album together. Catalogs disagree about an album: its spelling ("Khalij" on one,
/// "Khaleej" on another), its year or full release date, which edition it is (another
/// MusicBrainz album ID). Navidrome tells albums apart by all of these, so one album would
/// show up twice. When the album artist already has a folder for this album (the same album
/// by [`crate::domain::names::same_album`], any year), the song joins it and takes the
/// album's details from a song already there (see [`AlbumTags`]). A single named like an
/// album the artist has (the title song) joins that album. Without a readable song in the
/// folder, only the folder's year is taken.
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
    let kind = release_kind(candidate);
    let ((album, _), _) = folder_release(&current);
    let Ok(entries) = std::fs::read_dir(artist_folder) else {
        return;
    };
    // The album itself before a single or EP of the same name; then the fullest folder.
    let best = entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let ((existing_album, year), existing_kind) = folder_release(&name);
            let joins = existing_kind == kind || existing_kind == ReleaseKind::Album;
            (joins && crate::domain::names::same_album(existing_album, album))
                .then(|| (entry.path(), year.map(str::to_owned), existing_kind))
        })
        .map(|(folder, year, existing_kind)| {
            let songs = files::audio_files(&folder).0;
            (folder, year, existing_kind, songs)
        })
        .max_by_key(|(_, _, existing_kind, songs)| {
            (*existing_kind == ReleaseKind::Album, songs.len())
        });
    let Some((_, year, existing_kind, mut songs)) = best else {
        return;
    };
    songs.sort();
    let album_tags = songs
        .iter()
        .find_map(|song| tag_writer::read_album_tags(song).ok())
        .filter(|tags| tags.album.is_some());
    match album_tags {
        Some(tags) => join_album(candidate, tags, year, existing_kind),
        None => {
            if let Some(year) = year
                && candidate.year.as_deref() != Some(year.as_str())
            {
                if candidate
                    .release_date
                    .as_deref()
                    .is_some_and(|date| !date.starts_with(&year))
                {
                    candidate.release_date = None;
                }
                candidate.year = Some(year);
            }
        }
    }
}

/// The song takes the album's details, so it lands in the album's folder and Navidrome sees
/// one album. The song's own details (title, artists, recording) stay.
fn join_album(
    candidate: &mut Candidate,
    tags: AlbumTags,
    folder_year: Option<String>,
    kind: ReleaseKind,
) {
    let changed_kind = release_kind(candidate) != kind;
    candidate.album = tags.album;
    if let Some(album_artist) = tags.album_artist {
        let names = if tags.album_artists.is_empty() {
            credits::normalize_featured(&album_artist, "")
                .artists
                .into_iter()
                .map(|credit| credit.name)
                .collect()
        } else {
            tags.album_artists
        };
        let ids =
            (tags.release_artist_ids.len() == names.len()).then_some(&tags.release_artist_ids);
        let count = names.len();
        candidate.album_artist_credits = names
            .into_iter()
            .enumerate()
            .map(|(index, name)| credits::ArtistCredit {
                musicbrainz_id: ids.map(|ids| ids[index].clone()),
                ..credits::ArtistCredit::new(name, if index + 1 < count { " & " } else { "" })
            })
            .collect();
        candidate.album_artist = Some(album_artist);
        candidate.album_artist_sort = tags.album_artist_sort;
    }
    if candidate.release_id.as_deref().map(str::to_ascii_lowercase)
        != tags.release_id.as_deref().map(str::to_ascii_lowercase)
    {
        // The track ID belongs to the other edition.
        candidate.release_track_id = None;
    }
    candidate.release_id = tags.release_id;
    candidate.release_group_id = tags.release_group_id;
    candidate.release_date = tags.release_date;
    candidate.original_date = tags.original_date;
    candidate.year = tags
        .year
        .and_then(|year| year.get(..4).map(str::to_owned))
        .or(folder_year);
    candidate.release_type = tags.release_type.or_else(|| {
        Some(
            match kind {
                ReleaseKind::Album => "album",
                ReleaseKind::Single => "single",
                ReleaseKind::Ep => "ep",
            }
            .to_owned(),
        )
    });
    candidate.is_compilation = tags.compilation;
    candidate.label = tags.label;
    if changed_kind {
        // A single's "track 1 of 1" is not its place on the album.
        candidate.track_number = None;
        candidate.track_total = None;
        candidate.disc_number = None;
        candidate.disc_total = None;
    }
}

/// The same as joining an album in the library, for two songs filed in one batch: `song`
/// takes the album details of `album_song`, another song of the same album that a catalog
/// spelled or dated differently. A single joins an album (the title song), an album never
/// joins a single. Returns whether the song joined.
pub(crate) fn join_planned_album(song: &mut Candidate, album_song: &Candidate) -> bool {
    let (kind, album_kind) = (release_kind(song), release_kind(album_song));
    if kind != album_kind && album_kind != ReleaseKind::Album {
        return false;
    }
    song.album = album_song.album.clone();
    song.album_artist = album_song.album_artist.clone();
    song.album_artist_credits = album_song.album_artist_credits.clone();
    song.album_artist_sort = album_song.album_artist_sort.clone();
    if song.release_id.as_deref().map(str::to_ascii_lowercase)
        != album_song
            .release_id
            .as_deref()
            .map(str::to_ascii_lowercase)
    {
        // The track ID belongs to the other edition.
        song.release_track_id = None;
    }
    song.release_id = album_song.release_id.clone();
    song.release_group_id = album_song.release_group_id.clone();
    song.release_date = album_song.release_date.clone();
    song.original_date = album_song.original_date.clone();
    song.year = album_song.year.clone();
    song.release_type = album_song.release_type.clone();
    song.is_compilation = album_song.is_compilation;
    song.label = album_song.label.clone();
    if kind != album_kind {
        // A single's "track 1 of 1" is not its place on the album.
        song.track_number = None;
        song.track_total = None;
        song.disc_number = None;
        song.disc_total = None;
    }
    true
}

/// "Khalij (1999) [Single]" → (("Khalij", Some("1999")), Single).
fn folder_release(name: &str) -> ((&str, Option<&str>), ReleaseKind) {
    let (rest, kind) = if let Some(rest) = name.strip_suffix(" [Single]") {
        (rest, ReleaseKind::Single)
    } else if let Some(rest) = name.strip_suffix(" [EP]") {
        (rest, ReleaseKind::Ep)
    } else {
        (name, ReleaseKind::Album)
    };
    (strip_year(rest), kind)
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

pub(crate) fn release_year(candidate: &Candidate) -> Option<String> {
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
        candidate = files::numbered(&target, number);
        number += 1;
    }
    match tokio::fs::rename(file, &candidate).await {
        Ok(()) => {}
        // A different filesystem (should not happen with the recommended layout).
        Err(error) if error.kind() == std::io::ErrorKind::CrossesDevices => {
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

/// Removes folders left empty after a move, up to (not including) `base`.
pub(crate) async fn remove_empty_parents(file: &Path, base: &Path) {
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
    /// Compressed Chromaprint fingerprint, for "sounds the same" checks.
    pub fingerprint: Option<String>,
    /// Artist identities of the song, primary first.
    pub artist_ids: Vec<i64>,
    /// Set by [`find_same_recording`]: the two files themselves are the same
    /// recording (same bytes or same sound). False when only the ISRC or
    /// MusicBrainz ID of the chosen match says so, and the match can be wrong.
    pub proven_by_audio: bool,
}

/// The title as compared for "probably the same song": no `feat.` part, and
/// the same key for every spelling.
pub fn title_key(title: &str) -> String {
    crate::domain::names::match_key(&credits::release_title_without_featured(title))
}

/// Saves (or refreshes) one library file in the index.
pub async fn remember(pool: &SqlitePool, entry: &LibraryEntry) -> Result<()> {
    let (size, modified) = files::stamp(Path::new(&entry.path)).await?;
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO library_items(path,file_size,file_mtime_ns,duration,content_key,isrc,
           recording_id,title,artist,album,format,bitrate,source,added_at,checked_at,
           fingerprint,title_key,artist_ids)
         VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)
         ON CONFLICT(path) DO UPDATE SET
           file_size=excluded.file_size, file_mtime_ns=excluded.file_mtime_ns,
           duration=excluded.duration, content_key=excluded.content_key, isrc=excluded.isrc,
           recording_id=excluded.recording_id, title=excluded.title, artist=excluded.artist,
           album=excluded.album, format=excluded.format, bitrate=excluded.bitrate,
           checked_at=excluded.checked_at, fingerprint=excluded.fingerprint,
           title_key=excluded.title_key, artist_ids=excluded.artist_ids",
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
    .bind(&entry.fingerprint)
    .bind(entry.title.as_deref().map(title_key))
    .bind(crate::application::artists::ids_text(&entry.artist_ids))
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
///
/// A file proven by its audio wins over one found only by ISRC or MusicBrainz
/// ID. Those two come from the chosen match, not from the new file, so the
/// caller must ask the owner before it trashes or replaces anything on them
/// (`proven_by_audio` is false).
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
    let mut by_match_only = None;
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
        let proven_by_audio = exact || (close && same_fingerprint);
        let entry = LibraryEntry {
            path,
            duration,
            content_key,
            isrc: row_isrc,
            recording_id: row_recording,
            format,
            bitrate: bitrate.and_then(|value| u32::try_from(value).ok()),
            proven_by_audio,
            ..LibraryEntry::default()
        };
        if proven_by_audio {
            return Ok(Some(entry));
        }
        by_match_only.get_or_insert(entry);
    }
    Ok(find_same_sound(pool, evidence).await?.or(by_match_only))
}

/// path, duration, content key, format, bitrate, fingerprint.
type SoundRow = (
    String,
    Option<f64>,
    Option<String>,
    String,
    Option<i64>,
    String,
);

/// A library file that sounds the same (another encode of this recording),
/// with a length within 3 s.
async fn find_same_sound(
    pool: &SqlitePool,
    evidence: &RecordingEvidence,
) -> Result<Option<LibraryEntry>> {
    let (Some(duration), Some(raw)) = (
        evidence.duration,
        evidence
            .fingerprint
            .as_deref()
            .and_then(crate::domain::fingerprint::decode),
    ) else {
        return Ok(None);
    };
    let rows: Vec<SoundRow> = sqlx::query_as(
        "SELECT path,duration,content_key,format,bitrate,fingerprint FROM library_items
             WHERE fingerprint IS NOT NULL AND duration BETWEEN ? AND ? ORDER BY path",
    )
    .bind(duration - DURATION_TOLERANCE_SECONDS)
    .bind(duration + DURATION_TOLERANCE_SECONDS)
    .fetch_all(pool)
    .await?;
    for (path, row_duration, content_key, format, bitrate, fingerprint) in rows {
        let Some(row_raw) = crate::domain::fingerprint::decode(&fingerprint) else {
            continue;
        };
        if !input_dedup::sounds_the_same((&raw, Some(duration)), (&row_raw, row_duration)) {
            continue;
        }
        if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
            forget(pool, Path::new(&path)).await?;
            continue;
        }
        return Ok(Some(LibraryEntry {
            path,
            duration: row_duration,
            content_key,
            format,
            bitrate: bitrate.and_then(|value| u32::try_from(value).ok()),
            proven_by_audio: true,
            ..LibraryEntry::default()
        }));
    }
    Ok(None)
}

/// A library song that is probably the same as this one although nothing
/// proves it: the same title (without `feat.`), lengths within 3 s, and the
/// primary artist of one among the artists of the other. Spelling and order of
/// the artists do not matter. Used to ask the owner, never to skip by itself.
pub async fn find_possible_duplicate(
    pool: &SqlitePool,
    title: &str,
    duration: Option<f64>,
    artist_ids: &[i64],
) -> Result<Option<String>> {
    let (Some(duration), Some(primary)) = (duration, artist_ids.first()) else {
        return Ok(None);
    };
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT path,artist_ids FROM library_items
         WHERE title_key=? AND artist_ids IS NOT NULL AND duration BETWEEN ? AND ? ORDER BY path",
    )
    .bind(title_key(title))
    .bind(duration - DURATION_TOLERANCE_SECONDS)
    .bind(duration + DURATION_TOLERANCE_SECONDS)
    .fetch_all(pool)
    .await?;
    for (path, ids) in rows {
        let ids = crate::application::artists::ids_from_text(&ids);
        let overlap =
            ids.contains(primary) || ids.first().is_some_and(|id| artist_ids.contains(id));
        if overlap && tokio::fs::try_exists(&path).await.unwrap_or(false) {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

/// Is a file with exactly this audio (its content key) in the library?
pub async fn has_content(pool: &SqlitePool, content_key: &str) -> Result<bool> {
    let paths: Vec<String> =
        sqlx::query_scalar("SELECT path FROM library_items WHERE content_key=?")
            .bind(content_key)
            .fetch_all(pool)
            .await?;
    for path in paths {
        if tokio::fs::try_exists(&path).await.unwrap_or(false) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Hidden work files that a stop in the middle of a job left in the library:
/// copies of songs magpie-fi was tagging or filing (`.Song.mp3.magpie-fi-…`,
/// `.Song.tag-clean-…`), and an artist folder caught half-way through a
/// rename (`.Artist.magpie-fi-rename`). Called once at start, when no job is
/// running, so every such file is stale.
///
/// The copies go to the trash, never straight to `rm`: the song they were
/// made from is still in its place. A half-renamed artist folder gets its old
/// name back, because every scan skips hidden folders and its songs would
/// otherwise be gone from the library.
pub async fn recover_leftovers(library: &Path, trash: &Path) -> Result<usize> {
    const RENAMING: &str = ".magpie-fi-rename";
    let root = library.to_path_buf();
    let (folders, files) = tokio::task::spawn_blocking(move || {
        let (mut folders, mut files) = (Vec::new(), Vec::new());
        let mut walk = walkdir::WalkDir::new(&root).follow_links(false).into_iter();
        while let Some(Ok(entry)) = walk.next() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if entry.depth() == 0 || !name.starts_with('.') {
                continue;
            }
            if entry.file_type().is_dir() {
                if name.ends_with(RENAMING) {
                    folders.push(entry.path().to_path_buf());
                }
                // Other hidden folders (the trash, NAS metadata) are not ours to look into.
                walk.skip_current_dir();
            } else if entry.file_type().is_file()
                && (name.contains(".magpie-fi-") || name.contains(".tag-clean-"))
            {
                files.push(entry.path().to_path_buf());
            }
        }
        (folders, files)
    })
    .await?;

    let mut recovered = 0;
    for folder in folders {
        let Some(name) = folder
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_prefix('.'))
            .and_then(|name| name.strip_suffix(RENAMING))
        else {
            continue;
        };
        let original = folder.with_file_name(name);
        if tokio::fs::try_exists(&original).await.unwrap_or(true) {
            tracing::warn!(folder = %folder.display(), "a half-renamed artist folder could not get its name back: the name is taken");
            continue;
        }
        match tokio::fs::rename(&folder, &original).await {
            Ok(()) => {
                tracing::warn!(folder = %original.display(), "an artist folder was caught half-way through a rename and got its old name back");
                recovered += 1;
            }
            Err(error) => {
                tracing::warn!(%error, folder = %folder.display(), "could not give a half-renamed artist folder its name back");
            }
        }
    }
    for file in files {
        match move_to_trash(trash, library, &file).await {
            Ok(_) => recovered += 1,
            Err(error) => {
                tracing::warn!(%error, file = %file.display(), "could not move a leftover work file to the trash");
            }
        }
    }
    Ok(recovered)
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
    let (files, walk_errors) =
        tokio::task::spawn_blocking(move || files::audio_files(&root)).await?;
    // A folder that could not be read (a disk not mounted, a permission
    // problem) says nothing about its songs. Forgetting them would make
    // magpie-fi download songs you already have.
    let walked_cleanly = walk_errors.is_empty();
    for error in walk_errors {
        tracing::error!(
            error,
            "library folder walk error; no song is forgotten in this pass"
        );
    }

    // Rows from before artist identities and sound checks are described again,
    // a few hundred per pass so the inbox is never held up for long.
    let known: Vec<(String, i64, i64, bool)> = sqlx::query_as(
        "SELECT path,file_size,file_mtime_ns,
                (title_key IS NULL OR (fingerprint IS NULL AND content_key LIKE 'fp:%'))
         FROM library_items",
    )
    .fetch_all(pool)
    .await?;
    let mut backfill_left = 500_usize;
    let known: std::collections::HashMap<String, ((i64, i64), bool)> = known
        .into_iter()
        .map(|(path, size, modified, incomplete)| (path, ((size, modified), incomplete)))
        .collect();
    let present: std::collections::HashSet<String> = files
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    if walked_cleanly {
        for path in known.keys().filter(|path| !present.contains(*path)) {
            forget(pool, Path::new(path)).await?;
        }
    }

    let mut indexed = 0;
    for path in files {
        let text = path.to_string_lossy().into_owned();
        let Ok(identity) = files::stamp(&path).await else {
            continue;
        };
        if let Some((stamp, incomplete)) = known.get(&text)
            && *stamp == identity
        {
            if !*incomplete || backfill_left == 0 {
                continue;
            }
            backfill_left -= 1;
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
pub async fn describe_file(pool: &SqlitePool, path: &Path) -> Result<LibraryEntry> {
    let owned = path.to_path_buf();
    let (info, ids) = tokio::task::spawn_blocking(move || {
        let info = audio::read(&owned)?;
        Ok::<_, anyhow::Error>((info, read_identifiers(&owned)))
    })
    .await??;
    let (content_key, fingerprint) = match fingerprint_cache::get_or_calculate(pool, path, || {
        fingerprint::calculate(path)
    })
    .await
    {
        Ok(result) => (
            input_dedup::fingerprint_key(&result.fingerprint),
            Some(result.fingerprint),
        ),
        Err(_) => (
            Some(input_dedup::hash_key(
                &input_dedup::sha256_cached(pool, path).await?,
            )),
            None,
        ),
    };
    let artist_ids = match info.artist.as_deref() {
        Some(artist) => {
            let names = credits::normalize_featured(artist, info.title.as_deref().unwrap_or(""));
            crate::application::artists::resolve_names(pool, names.artists).await?
        }
        None => Vec::new(),
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
        fingerprint,
        artist_ids,
        proven_by_audio: false,
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

/// Writes `cover.jpg` (or `.png`/`.webp`, matching the image) into the album
/// folder when the folder has no cover file yet. Navidrome prefers it over
/// embedded art.
pub async fn write_folder_cover(album_folder: &Path, image: &[u8]) -> Result<()> {
    let mut entries = tokio::fs::read_dir(album_folder).await?;
    while let Some(entry) = entries.next_entry().await? {
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        if name.starts_with("cover.") || name.starts_with("folder.") || name.starts_with("front.") {
            return Ok(());
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
    Ok(())
}

// ── Playlists ───────────────────────────────────────────────────────────────

/// Writes `library/Playlists/<name>.m3u8` with paths relative to the playlist
/// file, which Navidrome imports automatically. Songs outside the library are
/// left out. An unchanged playlist is not rewritten.
pub async fn write_playlist(library: &Path, name: &str, songs: &[String]) -> Result<()> {
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
        return Ok(());
    }
    let temporary = folder.join(format!(".{}.m3u8.part", safe_name(name, "Playlist")));
    tokio::fs::write(&temporary, &content).await?;
    tokio::fs::rename(&temporary, &target).await?;
    Ok(())
}

/// Removes a file that magpie-fi created itself (a download in staging) or moves
/// anything else to the trash.
pub async fn discard_original(
    trash: &Path,
    staging: &Path,
    inbox: &Path,
    file: &Path,
) -> Result<()> {
    if file.starts_with(staging) {
        tokio::fs::remove_file(file).await?;
    } else {
        move_to_trash(trash, inbox, file).await?;
    }
    Ok(())
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
    fn a_duet_is_filed_under_its_first_artist() {
        let mut value = candidate();
        value.title = "Song".into();
        value.artist = "Ebi & Shadmehr Aghili".into();
        value.album = Some("Song".into());
        value.album_artist = Some("Ebi & Shadmehr Aghili".into());
        value.album_artist_credits = vec![
            crate::domain::credits::ArtistCredit::new("Ebi", " & "),
            crate::domain::credits::ArtistCredit::new("Shadmehr Aghili", ""),
        ];
        value.year = Some("2024".into());
        value.track_number = Some(1);
        value.release_type = Some("Single".into());
        assert_eq!(
            path(&value, "m4a"),
            "/lib/Ebi/Song (2024) [Single]/01 - Song.m4a"
        );
    }

    #[test]
    fn a_featured_album_artist_never_names_the_folder() {
        let mut value = candidate();
        value.title = "In the End".into();
        value.artist = "Tommee Profitt".into();
        value.album = Some("In the End".into());
        value.album_artist = Some("Tommee Profitt feat. Fleurie".into());
        value.album_artist_credits = vec![crate::domain::credits::ArtistCredit::new(
            "Tommee Profitt feat. Fleurie",
            "",
        )];
        assert!(path(&value, "m4a").starts_with("/lib/Tommee Profitt/"));
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
    fn a_new_song_takes_the_details_of_its_album_in_the_library() {
        if std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_err()
        {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let library = directory.path();
        let folder = library.join("Ebi/Khalij (1999)");
        std::fs::create_dir_all(&folder).unwrap();
        let filed = folder.join("01 - Hamdam.mp3");
        let status = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
            .arg("sine=frequency=440:duration=0.1")
            .args(["-q:a", "9"])
            .arg(&filed)
            .status()
            .unwrap();
        assert!(status.success());
        let album = Candidate {
            title: "Hamdam".into(),
            artist: "Ebi".into(),
            album: Some("Khalij".into()),
            album_artist: Some("Ebi".into()),
            year: Some("1999".into()),
            release_date: Some("1999-03-01".into()),
            release_id: Some("6b1d6b26-1c1e-4c4e-9d4a-5f4f4b1d2a11".into()),
            release_type: Some("album".into()),
            ..Default::default()
        };
        tag_writer::write(&filed, &album, None, None).unwrap();

        // Another source spells the album its own way and calls the title song a single.
        let mut song = Candidate {
            title: "Khaleej".into(),
            artist: "Ebi".into(),
            album: Some("Khaleej".into()),
            album_artist: Some("Ebi".into()),
            year: Some("2001".into()),
            release_date: Some("2001-01-01".into()),
            release_type: Some("single".into()),
            release_track_id: Some("0c1d6b26-1c1e-4c4e-9d4a-5f4f4b1d2a11".into()),
            track_number: Some(1),
            track_total: Some(1),
            ..Default::default()
        };
        align_with_existing_release(library, &mut song);

        assert_eq!(song.album.as_deref(), Some("Khalij"));
        assert_eq!(song.year.as_deref(), Some("1999"));
        assert_eq!(song.release_date.as_deref(), Some("1999-03-01"));
        assert_eq!(song.release_id, album.release_id);
        assert_eq!(song.release_track_id, None);
        assert_eq!(song.release_type.as_deref(), Some("album"));
        assert_eq!(song.track_number, None);
        assert_eq!(song.title, "Khaleej");
        assert_eq!(
            destination(library, &song, "flac"),
            folder.join("Khaleej.flac")
        );

        // Another album of the artist is left alone.
        let mut other = Candidate {
            title: "Lalehzaar".into(),
            artist: "Ebi".into(),
            album: Some("Lalehzaar".into()),
            year: Some("2022".into()),
            ..Default::default()
        };
        align_with_existing_release(library, &mut other);
        assert_eq!(other.album.as_deref(), Some("Lalehzaar"));
        assert_eq!(other.year.as_deref(), Some("2022"));
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
    async fn work_files_left_by_a_stop_are_cleared_and_a_half_renamed_folder_comes_back() {
        let directory = tempfile::tempdir().unwrap();
        let library = directory.path().join("library");
        let trash = library.join(".magpie-fi/trash");
        let album = library.join("Ebi/Khalij");
        std::fs::create_dir_all(&album).unwrap();
        std::fs::write(album.join("01 - Song.mp3"), b"the song").unwrap();
        std::fs::write(album.join(".01 - Song.mp3.magpie-fi-artist.mp3"), b"copy").unwrap();
        std::fs::write(album.join(".01 - Song.tag-clean-77.mp3"), b"copy").unwrap();
        std::fs::write(album.join(".DS_Store"), b"not ours").unwrap();
        // An artist folder caught between the two steps of a rename.
        let hidden = library.join(".Googoosh.magpie-fi-rename");
        std::fs::create_dir_all(hidden.join("Album")).unwrap();
        std::fs::write(hidden.join("Album/01 - Song.mp3"), b"googoosh").unwrap();
        // Something already in the trash is left alone.
        std::fs::create_dir_all(trash.join("2026-01-01")).unwrap();
        std::fs::write(trash.join("2026-01-01/.old.magpie-fi-x.mp3"), b"old").unwrap();

        assert_eq!(recover_leftovers(&library, &trash).await.unwrap(), 3);

        assert_eq!(
            std::fs::read(album.join("01 - Song.mp3")).unwrap(),
            b"the song"
        );
        assert!(!album.join(".01 - Song.mp3.magpie-fi-artist.mp3").exists());
        assert!(!album.join(".01 - Song.tag-clean-77.mp3").exists());
        assert!(album.join(".DS_Store").exists());
        assert_eq!(
            std::fs::read(library.join("Googoosh/Album/01 - Song.mp3")).unwrap(),
            b"googoosh"
        );
        assert!(!hidden.exists());
        assert!(trash.join("2026-01-01/.old.magpie-fi-x.mp3").exists());
        // Nothing is left to do the second time.
        assert_eq!(recover_leftovers(&library, &trash).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn indexing_finds_songs_added_by_hand_and_forgets_deleted_ones() {
        if std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_err()
        {
            return;
        }
        let (directory, pool) = pool().await;
        let library = directory.path().join("library");
        let folder = library.join("Ebi/Khalij (1999)");
        std::fs::create_dir_all(&folder).unwrap();
        let song = folder.join("01 - Hamdam.mp3");
        let status = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
            .arg("sine=frequency=440:duration=2")
            .args(["-q:a", "9"])
            .arg(&song)
            .status()
            .unwrap();
        assert!(status.success());
        let before = std::fs::read(&song).unwrap();
        let count = || {
            let pool = pool.clone();
            async move {
                sqlx::query_scalar::<_, i64>("SELECT count(*) FROM library_items")
                    .fetch_one(&pool)
                    .await
                    .unwrap()
            }
        };

        assert_eq!(index_library(&pool, &library).await.unwrap(), 1);
        assert_eq!(count().await, 1);
        // Indexing only reads: the song is byte for byte what it was.
        assert_eq!(std::fs::read(&song).unwrap(), before);
        // Nothing changed, so nothing is indexed again.
        assert_eq!(index_library(&pool, &library).await.unwrap(), 0);

        // The disk is not there (not mounted): every song is still known.
        let unmounted = directory.path().join("not-mounted");
        assert_eq!(index_library(&pool, &unmounted).await.unwrap(), 0);
        assert_eq!(count().await, 1);

        // The song is really deleted: now it is forgotten.
        std::fs::remove_file(&song).unwrap();
        assert_eq!(index_library(&pool, &library).await.unwrap(), 0);
        assert_eq!(count().await, 0);
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
            fingerprint: None,
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
        // Only the audio proves it about the file itself; an ISRC or a
        // MusicBrainz ID comes from the match, which can be the wrong one.
        let by_audio = |evidence: RecordingEvidence, mbid: Option<&'static str>| {
            let pool = pool.clone();
            async move {
                find_same_recording(&pool, &evidence, mbid)
                    .await
                    .unwrap()
                    .unwrap()
                    .proven_by_audio
            }
        };
        assert!(
            by_audio(
                probe(Some("fp:abc"), Some("USUM71703861"), 201.0),
                Some("mbid-1")
            )
            .await
        );
        assert!(!by_audio(probe(None, Some("USUM71703861"), 198.0), None).await);
        assert!(!by_audio(probe(None, None, 200.5), Some("mbid-1")).await);
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

    async fn test_pool(directory: &Path) -> SqlitePool {
        crate::infrastructure::db::connect(directory.join("library.sqlite").to_str().unwrap())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn another_encode_of_a_library_song_is_found_by_sound() {
        let installed = |tool: &str| {
            std::process::Command::new(tool)
                .arg("-version")
                .output()
                .is_ok()
        };
        if !installed("ffmpeg") || !installed("fpcalc") {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let pool = test_pool(directory.path()).await;
        let encode = |name: &str, source: &str, codec: &[&str]| {
            let path = directory.path().join(name);
            let status = std::process::Command::new("ffmpeg")
                .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
                .arg(source)
                .args(codec)
                .arg(&path)
                .status()
                .unwrap();
            assert!(status.success());
            path
        };
        let song = "aevalsrc=0.4*sin(2*PI*t*220*pow(2\\,mod(floor(t*3)*5\\,12)/12))\
                    *(0.6+0.4*sin(3*PI*t))+0.3*sin(1250*2*PI*t*(1+0.2*sin(0.7*t)))\
                    +0.25*sin(2*PI*t*110*pow(2\\,mod(floor(t/2)*7\\,12)/12)):s=44100:d=30";
        let other = "aevalsrc=0.4*sin(2*PI*t*196*pow(2\\,mod(floor(t*2.3)*3\\,12)/12))\
                     *(0.6+0.4*sin(5*PI*t))+0.3*sin(880*2*PI*t*(1+0.1*sin(1.3*t))):s=44100:d=30";
        let filed = encode("filed.flac", song, &["-c:a", "flac"]);
        let download = encode("download.opus", song, &["-c:a", "libopus", "-b:a", "64k"]);
        let different = encode("different.opus", other, &["-c:a", "libopus", "-b:a", "64k"]);
        let (filed_print, filed_length) = fingerprint::calculate(&filed).await.unwrap();
        remember(
            &pool,
            &LibraryEntry {
                path: filed.to_string_lossy().into_owned(),
                duration: Some(filed_length),
                content_key: input_dedup::fingerprint_key(&filed_print),
                format: "flac".into(),
                source: "inbox".into(),
                fingerprint: Some(filed_print),
                ..LibraryEntry::default()
            },
        )
        .await
        .unwrap();

        let probe = |path: &Path| {
            let path = path.to_path_buf();
            async move {
                let (print, length) = fingerprint::calculate(&path).await.unwrap();
                RecordingEvidence {
                    path,
                    format: "opus".into(),
                    duration: Some(length),
                    content_key: input_dedup::fingerprint_key(&print),
                    fingerprint: Some(print),
                    ..RecordingEvidence::default()
                }
            }
        };
        let same = probe(&download).await;
        // Different fingerprint strings: the old exact check alone would miss it.
        assert_ne!(
            same.content_key,
            input_dedup::fingerprint_key(&{ fingerprint::calculate(&filed).await.unwrap().0 })
        );
        let found = find_same_recording(&pool, &same, None).await.unwrap();
        assert_eq!(found.unwrap().path, filed.to_string_lossy());
        let unrelated = probe(&different).await;
        assert!(
            find_same_recording(&pool, &unrelated, None)
                .await
                .unwrap()
                .is_none()
        );

        // Two different steady tones of one length have near-identical,
        // unchanging fingerprints; they must never count as one song.
        let tone = encode(
            "tone-a.mp3",
            "sine=frequency=440:duration=30",
            &["-c:a", "libmp3lame"],
        );
        let other_tone = encode(
            "tone-b.mp3",
            "sine=frequency=550:duration=30",
            &["-c:a", "libmp3lame"],
        );
        let (tone_print, tone_length) = fingerprint::calculate(&tone).await.unwrap();
        assert_eq!(input_dedup::fingerprint_key(&tone_print), None);
        remember(
            &pool,
            &LibraryEntry {
                path: tone.to_string_lossy().into_owned(),
                duration: Some(tone_length),
                format: "mp3".into(),
                source: "inbox".into(),
                fingerprint: Some(tone_print),
                ..LibraryEntry::default()
            },
        )
        .await
        .unwrap();
        let other = probe(&other_tone).await;
        assert!(
            find_same_recording(&pool, &other, None)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn the_same_duet_in_another_spelling_is_a_possible_duplicate() {
        let directory = tempfile::tempdir().unwrap();
        let pool = test_pool(directory.path()).await;
        let filed = directory.path().join("Ebi - Song.m4a");
        tokio::fs::write(&filed, b"audio").await.unwrap();
        // Filed as "Ebi feat. Shadmehr Aghili – Song (feat. Shadmehr Aghili)".
        remember(
            &pool,
            &LibraryEntry {
                path: filed.to_string_lossy().into_owned(),
                duration: Some(200.0),
                title: Some("Song (feat. Shadmehr Aghili)".into()),
                format: "m4a".into(),
                source: "inbox".into(),
                artist_ids: vec![1, 2],
                ..LibraryEntry::default()
            },
        )
        .await
        .unwrap();
        let possible = |title: &'static str, length: f64, ids: Vec<i64>| {
            let pool = pool.clone();
            async move {
                find_possible_duplicate(&pool, title, Some(length), &ids)
                    .await
                    .unwrap()
                    .is_some()
            }
        };
        // "Shadmehr Aghili & Ebi – SONG": other order, other spelling of the title.
        assert!(possible("SONG", 201.5, vec![2, 1]).await);
        assert!(possible("Song", 199.0, vec![1]).await);
        // A remix, another length or other artists are different songs.
        assert!(!possible("Song (Remix)", 200.0, vec![1, 2]).await);
        assert!(!possible("Song", 230.0, vec![1, 2]).await);
        assert!(!possible("Song", 200.0, vec![7]).await);
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
            fingerprint: None,
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
        write_playlist(&library, "Arta - Road Trip", &songs)
            .await
            .unwrap();
        let text = tokio::fs::read_to_string(library.join("Playlists/Arta - Road Trip.m3u8"))
            .await
            .unwrap();
        assert_eq!(
            text,
            "#EXTM3U\n#PLAYLIST:Arta - Road Trip\n../A/Album (2001)/01 - One.m4a\n../B/Single (2020) [Single]/01 - Three.m4a\n"
        );
        write_playlist(&library, "Arta - Road Trip", &songs)
            .await
            .unwrap();
        assert_eq!(
            tokio::fs::read_to_string(library.join("Playlists/Arta - Road Trip.m3u8"))
                .await
                .unwrap(),
            text
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

        discard_original(&trash, &staging, &inbox, &download)
            .await
            .unwrap();
        assert!(!download.exists());
        assert!(!trash.exists());
        discard_original(&trash, &staging, &inbox, &dropped)
            .await
            .unwrap();
        assert!(!dropped.exists());
        let (trashed, _) = files::audio_files(&trash);
        assert_eq!(trashed.len(), 1);
        assert!(trashed[0].ends_with("mine.mp3"));
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
        write_folder_cover(directory.path(), &png).await.unwrap();
        let cover = directory.path().join("cover.png");
        assert_eq!(std::fs::read(&cover).unwrap(), png);
        write_folder_cover(directory.path(), b"other")
            .await
            .unwrap();
        assert_eq!(std::fs::read(&cover).unwrap(), png);
    }
}
