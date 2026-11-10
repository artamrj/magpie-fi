//! The Files tab: walk `library/` one folder at a time, search it, and fix a
//! song's tags by hand.
//!
//! Every path the page sends is relative to the library and goes through
//! [`resolve`] first, so nothing outside the library can be listed or read.
//! Listings come from the disk (so new or unindexed files show up) and the
//! titles from `library_items` (so a folder of 500 songs opens without reading
//! 500 files).

use crate::{
    application::{files, library},
    domain::names::match_key,
    infrastructure::media::tag_writer::{self, TagEdit},
};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path, PathBuf},
};
use unicode_normalization::UnicodeNormalization;

/// Songs a search returns at most; the page says when there are more.
const SEARCH_SONGS: usize = 100;
const SEARCH_FOLDERS: usize = 30;

#[derive(Debug, thiserror::Error)]
pub enum BrowseError {
    #[error("That path is not inside the library")]
    Outside,
    #[error("That file or folder is no longer in the library")]
    NotFound,
    /// What the page sent cannot be written; the text says why.
    #[error("{0}")]
    Invalid(String),
    #[error("The file changed since you opened it. Open it again to see the new tags.")]
    Changed,
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

type Result<T> = std::result::Result<T, BrowseError>;

#[derive(Debug, Serialize)]
pub struct FolderEntry {
    pub path: String,
    pub name: String,
    /// Songs anywhere inside, as far as the index knows.
    pub songs: usize,
    /// Folders directly inside (albums, for an artist folder).
    pub folders: usize,
}

#[derive(Debug, Serialize)]
pub struct SongEntry {
    pub path: String,
    pub name: String,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub format: String,
    pub duration: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct Folder {
    pub path: String,
    pub name: String,
    pub folders: Vec<FolderEntry>,
    pub songs: Vec<SongEntry>,
    /// Songs anywhere inside this folder.
    pub songs_inside: usize,
}

#[derive(Debug, Serialize)]
pub struct SearchResult {
    pub folders: Vec<FolderEntry>,
    pub songs: Vec<SongEntry>,
    /// More songs matched than were returned.
    pub more: bool,
}

#[derive(Debug, Serialize)]
pub struct SongDetails {
    pub path: String,
    pub name: String,
    pub folder: String,
    pub size: u64,
    pub format: String,
    /// Size and modification time; a save is refused when the file no longer matches.
    pub version: String,
    /// WAV and AIFF tags are not edited.
    pub editable: bool,
    #[serde(flatten)]
    pub tags: tag_writer::SongTags,
}

/// Turns a path relative to the library into a path on disk. Refuses absolute
/// paths, `..`, hidden or NAS system folders (`.magpie-fi`, `@eaDir`), and
/// anything that leads outside the library through a link.
pub fn resolve(root: &Path, relative: &str) -> Result<PathBuf> {
    if relative.contains('\0') {
        return Err(BrowseError::Outside);
    }
    let mut path = root.to_path_buf();
    for component in Path::new(relative).components() {
        match component {
            Component::Normal(name) if !is_hidden(name.to_str().unwrap_or(".")) => path.push(name),
            Component::CurDir => {}
            _ => return Err(BrowseError::Outside),
        }
    }
    let real = path.canonicalize().map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => BrowseError::NotFound,
        _ => BrowseError::Other(error.into()),
    })?;
    let real_root = root
        .canonicalize()
        .map_err(|error| BrowseError::Other(error.into()))?;
    if !real.starts_with(&real_root) {
        return Err(BrowseError::Outside);
    }
    Ok(path)
}

fn is_hidden(name: &str) -> bool {
    name.starts_with('.') || name.starts_with('@')
}

/// `a/b` + `c` = `a/b/c`; the library root is the empty string.
fn join(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_owned()
    } else {
        format!("{parent}/{name}")
    }
}

/// The prefix every indexed path inside `folder` starts with.
fn prefix(folder: &Path) -> String {
    format!("{}/", folder.to_string_lossy().trim_end_matches('/'))
}

fn relative(root: &Path, path: &str) -> Option<String> {
    path.strip_prefix(&prefix(root)).map(str::to_owned)
}

/// One folder: its subfolders with song counts, and its songs.
pub async fn list_folder(pool: &SqlitePool, root: &Path, folder: &str) -> Result<Folder> {
    let directory = resolve(root, folder)?;
    if !directory.is_dir() {
        return Err(BrowseError::NotFound);
    }
    let folder = folder.trim_matches('/').to_owned();
    let mut folder_names = Vec::new();
    let mut file_names = Vec::new();
    let mut entries = tokio::fs::read_dir(&directory)
        .await
        .map_err(|error| BrowseError::Other(error.into()))?;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| BrowseError::Other(error.into()))?
    {
        // Names that are not UTF-8 cannot be sent back by the page; links are not
        // followed, like the library scan.
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Ok(kind) = entry.file_type().await else {
            continue;
        };
        if is_hidden(&name) {
            continue;
        }
        if kind.is_dir() {
            folder_names.push(name);
        } else if kind.is_file() && crate::domain::audio::is_supported(Path::new(&name)) {
            file_names.push(name);
        }
    }

    let start = prefix(&directory);
    let inside: Vec<String> = sqlx::query_scalar(
        "SELECT substr(path, length(?1) + 1) FROM library_items
         WHERE substr(path, 1, length(?1)) = ?1",
    )
    .bind(&start)
    .fetch_all(pool)
    .await
    .map_err(|error| BrowseError::Other(error.into()))?;
    let mut counts: BTreeMap<&str, (usize, BTreeSet<&str>)> = BTreeMap::new();
    for rest in &inside {
        let mut parts = rest.split('/');
        let (Some(child), Some(second)) = (parts.next(), parts.next()) else {
            continue;
        };
        let count = counts.entry(child).or_default();
        count.0 += 1;
        if parts.next().is_some() {
            count.1.insert(second);
        }
    }

    let mut folders: Vec<FolderEntry> = folder_names
        .into_iter()
        .map(|name| {
            let (songs, subfolders) = counts
                .get(name.as_str())
                .map_or((0, 0), |(songs, folders)| (*songs, folders.len()));
            FolderEntry {
                path: join(&folder, &name),
                name,
                songs,
                folders: subfolders,
            }
        })
        .collect();
    folders.sort_by_key(|entry| sort_key(&entry.name));

    let rows = indexed_songs(pool, &start).await?;
    let mut songs: Vec<SongEntry> = file_names
        .into_iter()
        .map(|name| {
            let row = rows.get(&name);
            song_entry(join(&folder, &name), name, row)
        })
        .collect();
    songs.sort_by_key(|song| sort_key(&song.name));

    Ok(Folder {
        name: folder.rsplit('/').next().unwrap_or_default().to_owned(),
        path: folder,
        folders,
        songs,
        songs_inside: inside.len(),
    })
}

fn sort_key(name: &str) -> String {
    name.nfc().flat_map(char::to_lowercase).collect()
}

#[derive(sqlx::FromRow)]
struct SongRow {
    path: String,
    title: Option<String>,
    artist: Option<String>,
    album: Option<String>,
    format: String,
    duration: Option<f64>,
}

/// Index rows of the files directly in one folder, by file name.
async fn indexed_songs(pool: &SqlitePool, start: &str) -> Result<BTreeMap<String, SongRow>> {
    let rows: Vec<SongRow> = sqlx::query_as(
        "SELECT path,title,artist,album,format,duration FROM library_items
         WHERE substr(path, 1, length(?1)) = ?1
           AND instr(substr(path, length(?1) + 1), '/') = 0",
    )
    .bind(start)
    .fetch_all(pool)
    .await
    .map_err(|error| BrowseError::Other(error.into()))?;
    Ok(rows
        .into_iter()
        .map(|row| (row.path[start.len()..].to_owned(), row))
        .collect())
}

fn song_entry(path: String, name: String, row: Option<&SongRow>) -> SongEntry {
    let stem = Path::new(&name)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| name.clone());
    let extension = Path::new(&name)
        .extension()
        .map(|extension| extension.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    SongEntry {
        title: row
            .and_then(|row| row.title.clone())
            .filter(|title| !title.trim().is_empty())
            .unwrap_or(stem),
        artist: row.and_then(|row| row.artist.clone()),
        album: row.and_then(|row| row.album.clone()),
        format: row
            .map(|row| row.format.clone())
            .filter(|format| !format.is_empty())
            .unwrap_or(extension),
        duration: row.and_then(|row| row.duration),
        path,
        name,
    }
}

/// Songs whose title, artist, album or path contains every word, and folders
/// whose name matches. `within` limits the search to one folder.
pub async fn search(
    pool: &SqlitePool,
    root: &Path,
    query: &str,
    within: &str,
) -> Result<SearchResult> {
    let words: Vec<String> = query
        .nfc()
        .collect::<String>()
        .split_whitespace()
        .take(6)
        .map(str::to_owned)
        .collect();
    if words.is_empty() {
        return Ok(SearchResult {
            folders: Vec::new(),
            songs: Vec::new(),
            more: false,
        });
    }
    let scope = resolve(root, within)?;
    let start = prefix(&scope);

    // Plain LIKE finds Latin text in any case; `title_key` also finds titles
    // written with other Persian letter forms or without accents.
    let mut sql = String::from(
        "SELECT path,title,artist,album,format,duration FROM library_items
         WHERE substr(path, 1, length(?)) = ?",
    );
    for _ in &words {
        sql.push_str(
            " AND (title LIKE ? ESCAPE '\\' OR artist LIKE ? ESCAPE '\\'
                   OR album LIKE ? ESCAPE '\\' OR substr(path, ?) LIKE ? ESCAPE '\\'
                   OR title_key LIKE ? ESCAPE '\\')",
        );
    }
    sql.push_str(" ORDER BY path LIMIT ?");
    // Only the fixed clause above is repeated, once per word; every value is bound.
    let mut statement = sqlx::query_as::<_, SongRow>(sqlx::AssertSqlSafe(sql))
        .bind(&start)
        .bind(&start);
    let after_root = (prefix(root).chars().count() + 1) as i64;
    for word in &words {
        let pattern = like_pattern(word);
        let key = match_key(word);
        let key_pattern = if key.is_empty() {
            pattern.clone()
        } else {
            like_pattern(&key)
        };
        statement = statement
            .bind(pattern.clone())
            .bind(pattern.clone())
            .bind(pattern.clone())
            .bind(after_root)
            .bind(pattern)
            .bind(key_pattern);
    }
    let rows = statement
        .bind((SEARCH_SONGS + 1) as i64)
        .fetch_all(pool)
        .await
        .map_err(|error| BrowseError::Other(error.into()))?;
    let more = rows.len() > SEARCH_SONGS;
    let songs = rows
        .iter()
        .take(SEARCH_SONGS)
        .filter_map(|row| {
            let path = relative(root, &row.path)?;
            let name = path.rsplit('/').next()?.to_owned();
            Some(song_entry(path, name, Some(row)))
        })
        .collect();

    let folders = search_folders(pool, root, &start, &words).await?;
    Ok(SearchResult {
        folders,
        songs,
        more,
    })
}

/// Folders inside the scope whose path holds every word and whose own name
/// holds at least one: "daft discovery" finds `Daft Punk/Discovery (2001)`
/// but not `Daft Punk` itself.
async fn search_folders(
    pool: &SqlitePool,
    root: &Path,
    start: &str,
    words: &[String],
) -> Result<Vec<FolderEntry>> {
    let paths: Vec<String> =
        sqlx::query_scalar("SELECT path FROM library_items WHERE substr(path, 1, length(?1)) = ?1")
            .bind(start)
            .fetch_all(pool)
            .await
            .map_err(|error| BrowseError::Other(error.into()))?;
    let scope = relative(root, start).unwrap_or_default();
    let scope = scope.trim_end_matches('/');
    let mut folders: BTreeMap<String, (usize, BTreeSet<String>)> = BTreeMap::new();
    for path in &paths {
        let Some(path) = relative(root, path) else {
            continue;
        };
        let parts: Vec<&str> = path.split('/').collect();
        for depth in 1..parts.len() {
            let folder = parts[..depth].join("/");
            if folder.len() <= scope.len() {
                continue;
            }
            let entry = folders.entry(folder).or_default();
            entry.0 += 1;
            if depth + 1 < parts.len() {
                entry.1.insert(parts[depth].to_owned());
            }
        }
    }
    let keys: Vec<String> = words
        .iter()
        .map(|word| match_key(word))
        .filter(|key| !key.is_empty())
        .collect();
    if keys.is_empty() {
        return Ok(Vec::new());
    }
    let mut found: Vec<FolderEntry> = folders
        .into_iter()
        .filter(|(path, _)| {
            let below = path[scope.len()..].trim_start_matches('/');
            let whole = match_key(below);
            let name = match_key(path.rsplit('/').next().unwrap_or_default());
            keys.iter().all(|key| whole.contains(key.as_str()))
                && keys.iter().any(|key| name.contains(key.as_str()))
        })
        .take(SEARCH_FOLDERS)
        .map(|(path, (songs, subfolders))| FolderEntry {
            name: path.rsplit('/').next().unwrap_or_default().to_owned(),
            path,
            songs,
            folders: subfolders.len(),
        })
        .collect();
    found.sort_by_key(|entry| (entry.path.matches('/').count(), sort_key(&entry.name)));
    Ok(found)
}

/// `%word%` with LIKE's own wildcards taken literally.
fn like_pattern(word: &str) -> String {
    let escaped = word
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    format!("%{escaped}%")
}

/// Everything the file says about itself, read fresh from the disk.
pub async fn song(root: &Path, path: &str) -> Result<SongDetails> {
    let file = resolve(root, path)?;
    if !file.is_file() || !crate::domain::audio::is_supported(&file) {
        return Err(BrowseError::NotFound);
    }
    let version = version(&file).await?;
    let size = tokio::fs::metadata(&file)
        .await
        .map_err(|error| BrowseError::Other(error.into()))?
        .len();
    let reading = file.clone();
    let tags = tokio::task::spawn_blocking(move || tag_writer::read_details(&reading))
        .await
        .map_err(|error| BrowseError::Other(error.into()))??;
    let path = path.trim_matches('/').to_owned();
    let (folder, name) = path.rsplit_once('/').unwrap_or(("", &path));
    let format = file
        .extension()
        .map(|extension| extension.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    Ok(SongDetails {
        folder: folder.to_owned(),
        name: name.to_owned(),
        editable: !matches!(format.as_str(), "wav" | "aiff" | "aif"),
        format,
        size,
        version,
        tags,
        path: path.clone(),
    })
}

async fn version(file: &Path) -> Result<String> {
    let (size, modified) = files::stamp(file).await?;
    Ok(format!("{size}-{modified}"))
}

/// Fields typed on the Files tab, as text. A missing field is left alone; an
/// empty one removes the tag.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SongChanges {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album_artist: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track_total: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disc: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disc_total: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub genre: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub composer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isrc: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Saved {
    /// How many fields really changed (0 when every value was already there).
    pub changed: usize,
    /// The old values of the changed fields; saving them again undoes the edit.
    pub before: SongChanges,
    pub song: SongDetails,
}

/// One line of text: NFC, trimmed, at most 500 characters. `None` for empty.
fn line(field: &str, value: &str) -> Result<Option<String>> {
    let value: String = value.nfc().collect::<String>().trim().to_owned();
    if value.chars().any(char::is_control) {
        return Err(BrowseError::Invalid(format!("{field} must be one line")));
    }
    if value.chars().count() > 500 {
        return Err(BrowseError::Invalid(format!("{field} is too long")));
    }
    Ok((!value.is_empty()).then_some(value))
}

fn number(field: &str, value: &str) -> Result<Option<u32>> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    match value.parse::<u32>() {
        Ok(number @ 1..=999) => Ok(Some(number)),
        _ => Err(BrowseError::Invalid(format!(
            "{field} must be a number from 1 to 999"
        ))),
    }
}

/// The typed text as values to write, keeping only fields that differ from
/// the file. Also returns the old values of those fields, as typed text.
fn plan(changes: &SongChanges, now: &tag_writer::SongTags) -> Result<(TagEdit, SongChanges)> {
    let mut edit = TagEdit::default();
    let mut before = SongChanges::default();
    let shown = |value: &Option<String>| value.clone().unwrap_or_default();
    let count = |value: Option<u32>| value.map(|number| number.to_string()).unwrap_or_default();
    macro_rules! text {
        ($field:ident, $label:literal) => {
            if let Some(typed) = &changes.$field {
                let value = line($label, typed)?;
                if value != now.$field {
                    before.$field = Some(shown(&now.$field));
                    edit.$field = Some(value);
                }
            }
        };
    }
    macro_rules! numeric {
        ($field:ident, $label:literal) => {
            if let Some(typed) = &changes.$field {
                let value = number($label, typed)?;
                if value != now.$field {
                    before.$field = Some(count(now.$field));
                    edit.$field = Some(value);
                }
            }
        };
    }
    text!(title, "Title");
    text!(artist, "Artist");
    text!(album, "Album");
    text!(album_artist, "Album artist");
    text!(genre, "Genre");
    text!(composer, "Composer");
    text!(label, "Label");
    numeric!(track, "Track");
    numeric!(track_total, "Track total");
    numeric!(disc, "Disc");
    numeric!(disc_total, "Disc total");
    if let Some(typed) = &changes.year {
        let value = line("Year", typed)?;
        if let Some(year) = &value
            && !(year.len() == 4
                && year
                    .parse::<u32>()
                    .is_ok_and(|number| (1000..3000).contains(&number)))
        {
            return Err(BrowseError::Invalid(
                "Year must be four digits, like 2013".into(),
            ));
        }
        // A full date in the file ("2013-05-17") is the same year as "2013".
        let same = match (&value, &now.year) {
            (Some(year), Some(old)) => old.starts_with(year.as_str()),
            (None, None) => true,
            _ => false,
        };
        if !same {
            before.year = Some(shown(&now.year));
            edit.year = Some(value);
        }
    }
    if let Some(typed) = &changes.isrc {
        let value = match line("ISRC", typed)? {
            None => None,
            Some(text) => Some(
                crate::application::input_dedup::normalize_isrc(&text).ok_or_else(|| {
                    BrowseError::Invalid(
                        "An ISRC has 12 letters and digits, like USQX91300108".into(),
                    )
                })?,
            ),
        };
        if value != now.isrc {
            before.isrc = Some(shown(&now.isrc));
            edit.isrc = Some(value);
        }
    }
    if matches!(edit.title, Some(None)) || matches!(edit.artist, Some(None)) {
        return Err(BrowseError::Invalid(
            "A song needs a title and an artist".into(),
        ));
    }
    Ok((edit, before))
}

/// The file says what was meant to be written; otherwise the save is undone.
fn check(written: &tag_writer::SongTags, edit: &TagEdit) -> anyhow::Result<()> {
    let mut wrong = Vec::new();
    macro_rules! same {
        ($field:ident) => {
            if let Some(expected) = &edit.$field
                && *expected != written.$field
            {
                wrong.push(stringify!($field));
            }
        };
    }
    same!(title);
    same!(artist);
    same!(album);
    same!(album_artist);
    same!(genre);
    same!(composer);
    same!(label);
    same!(isrc);
    same!(track);
    same!(track_total);
    same!(disc);
    same!(disc_total);
    same!(year);
    if wrong.is_empty() {
        Ok(())
    } else {
        anyhow::bail!("the file did not keep {}", wrong.join(", "))
    }
}

/// Writes the changed fields of one song. The caller holds the workflow slot,
/// so no scan or filing touches the library meanwhile.
pub async fn edit_song(
    pool: &SqlitePool,
    root: &Path,
    path: &str,
    expected_version: &str,
    changes: &SongChanges,
) -> Result<Saved> {
    let file = song_file(root, path)?;
    if version(&file).await? != expected_version {
        return Err(BrowseError::Changed);
    }
    let reading = file.clone();
    let now = tokio::task::spawn_blocking(move || tag_writer::read_details(&reading))
        .await
        .map_err(|error| BrowseError::Other(error.into()))??;
    let (edit, before) = plan(changes, &now)?;
    let changed = [
        edit.title.is_some(),
        edit.artist.is_some(),
        edit.album.is_some(),
        edit.album_artist.is_some(),
        edit.track.is_some(),
        edit.track_total.is_some(),
        edit.disc.is_some(),
        edit.disc_total.is_some(),
        edit.year.is_some(),
        edit.genre.is_some(),
        edit.composer.is_some(),
        edit.label.is_some(),
        edit.isrc.is_some(),
    ]
    .into_iter()
    .filter(|changed| *changed)
    .count();
    if !edit.is_empty() {
        let writing = file.clone();
        tokio::task::spawn_blocking(move || {
            files::edit_on_copy(&writing, "edit", |copy| {
                tag_writer::edit_fields(copy, &edit)?;
                check(&tag_writer::read_details(copy)?, &edit)?;
                Ok(true)
            })
        })
        .await
        .map_err(|error| BrowseError::Other(error.into()))?
        .map_err(|error| BrowseError::Invalid(format!("Could not save the tags: {error:#}")))?;
        // Search, lists and the Artists page see the new tags at once.
        match library::describe_file(pool, &file).await {
            Ok(entry) => library::remember(pool, &entry).await?,
            Err(error) => {
                tracing::warn!(path = %file.display(), error = format!("{error:#}"), "could not index the edited song")
            }
        }
    }
    Ok(Saved {
        changed,
        before,
        song: song(root, path).await?,
    })
}

/// A song file inside the library, for its cover and its audio.
pub fn song_file(root: &Path, path: &str) -> Result<PathBuf> {
    let file = resolve(root, path)?;
    if file.is_file() && crate::domain::audio::is_supported(&file) {
        Ok(file)
    } else {
        Err(BrowseError::NotFound)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::library::{LibraryEntry, remember};

    async fn test_pool(directory: &Path) -> SqlitePool {
        crate::infrastructure::db::connect(directory.join("files.sqlite").to_str().unwrap())
            .await
            .unwrap()
    }

    /// A library with two artists; the files are empty stand-ins, so only the
    /// index gives them titles.
    async fn library() -> (tempfile::TempDir, PathBuf, SqlitePool) {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir(directory.path().join("library")).unwrap();
        // The server canonicalizes the library folder at startup; so do the tests.
        let root = directory.path().join("library").canonicalize().unwrap();
        let pool = test_pool(directory.path()).await;
        for (path, title, artist, album) in [
            (
                "Daft Punk/Discovery (2001)/03 - Digital Love.flac",
                "Digital Love",
                "Daft Punk",
                "Discovery",
            ),
            (
                "Daft Punk/Random Access Memories (2013)/08 - Get Lucky.m4a",
                "Get Lucky",
                "Daft Punk feat. Pharrell Williams",
                "Random Access Memories",
            ),
            (
                "Radiohead/OK Computer (1997)/11 - Lucky.flac",
                "Lucky",
                "Radiohead",
                "OK Computer",
            ),
            (
                "شادمهر عقیلی/سایه (2000)/01 - سایه.m4a",
                "سایه",
                "شادمهر عقیلی",
                "سایه",
            ),
            ("100%_Hits/01 - Song.mp3", "Song", "Someone", "100% Hits"),
        ] {
            let file = root.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(&file, b"stand-in").unwrap();
            remember(
                &pool,
                &LibraryEntry {
                    path: file.to_string_lossy().into_owned(),
                    title: Some(title.into()),
                    artist: Some(artist.into()),
                    album: Some(album.into()),
                    format: "flac".into(),
                    source: "existing".into(),
                    ..LibraryEntry::default()
                },
            )
            .await
            .unwrap();
        }
        (directory, root, pool)
    }

    #[tokio::test]
    async fn paths_outside_the_library_are_refused() {
        let (directory, root, _pool) = library().await;
        let sibling = directory.path().join("library2");
        std::fs::create_dir(&sibling).unwrap();
        std::fs::write(directory.path().join("secret.mp3"), b"x").unwrap();
        for path in [
            "../secret.mp3",
            "/etc",
            "Daft Punk/../../secret.mp3",
            "../library2",
        ] {
            assert!(
                matches!(resolve(&root, path), Err(BrowseError::Outside)),
                "{path} must be refused"
            );
        }
        // magpie-fi's own and NAS system folders stay hidden.
        std::fs::create_dir(root.join(".magpie-fi")).unwrap();
        assert!(matches!(
            resolve(&root, ".magpie-fi"),
            Err(BrowseError::Outside)
        ));
        // A link that leads out of the library is refused too.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&sibling, root.join("Linked")).unwrap();
            assert!(matches!(
                resolve(&root, "Linked"),
                Err(BrowseError::Outside)
            ));
        }
        assert!(matches!(
            resolve(&root, "Nobody"),
            Err(BrowseError::NotFound)
        ));
        assert_eq!(resolve(&root, "").unwrap(), root);
        assert_eq!(
            resolve(&root, "Daft Punk/").unwrap(),
            root.join("Daft Punk")
        );
    }

    #[tokio::test]
    async fn a_folder_lists_its_folders_with_counts_and_its_songs() {
        let (_directory, root, pool) = library().await;
        std::fs::write(root.join("Loose Track.mp3"), b"x").unwrap();
        std::fs::write(root.join("playlist.m3u8"), b"x").unwrap();
        std::fs::create_dir(root.join("@eaDir")).unwrap();

        let top = list_folder(&pool, &root, "").await.unwrap();
        let names: Vec<_> = top
            .folders
            .iter()
            .map(|folder| folder.name.as_str())
            .collect();
        assert_eq!(
            names,
            ["100%_Hits", "Daft Punk", "Radiohead", "شادمهر عقیلی"]
        );
        let daft = &top.folders[1];
        assert_eq!((daft.songs, daft.folders), (2, 2));
        assert_eq!(daft.path, "Daft Punk");
        assert_eq!(top.songs_inside, 5);
        // A file the index does not know yet still shows, named after the file.
        assert_eq!(top.songs.len(), 1);
        assert_eq!(top.songs[0].title, "Loose Track");
        assert_eq!(top.songs[0].format, "mp3");

        let album = list_folder(&pool, &root, "Daft Punk/Random Access Memories (2013)")
            .await
            .unwrap();
        assert_eq!(album.name, "Random Access Memories (2013)");
        assert_eq!(album.songs.len(), 1);
        assert_eq!(album.songs[0].title, "Get Lucky");
        assert_eq!(
            album.songs[0].path,
            "Daft Punk/Random Access Memories (2013)/08 - Get Lucky.m4a"
        );

        assert!(matches!(
            list_folder(
                &pool,
                &root,
                "Daft Punk/Random Access Memories (2013)/08 - Get Lucky.m4a"
            )
            .await,
            Err(BrowseError::NotFound)
        ));
    }

    #[tokio::test]
    async fn search_finds_songs_and_folders_by_every_word() {
        let (_directory, root, pool) = library().await;
        let titles = |result: &SearchResult| {
            result
                .songs
                .iter()
                .map(|song| song.title.clone())
                .collect::<Vec<_>>()
        };

        let lucky = search(&pool, &root, "LUCKY", "").await.unwrap();
        assert_eq!(titles(&lucky), ["Get Lucky", "Lucky"]);
        assert!(!lucky.more);

        // Words may come from different fields; the scope limits the search.
        assert_eq!(
            titles(&search(&pool, &root, "daft lucky", "").await.unwrap()),
            ["Get Lucky"]
        );
        assert_eq!(
            titles(&search(&pool, &root, "lucky", "Radiohead").await.unwrap()),
            ["Lucky"]
        );

        // Persian with Arabic letter forms (ي for ی) still finds the title.
        assert_eq!(
            titles(&search(&pool, &root, "سايه", "").await.unwrap()),
            ["سایه"]
        );

        // LIKE wildcards are plain characters.
        assert_eq!(
            titles(&search(&pool, &root, "100%", "").await.unwrap()),
            ["Song"]
        );
        assert!(
            titles(&search(&pool, &root, "%", "").await.unwrap())
                .iter()
                .all(|title| title == "Song")
        );

        let folders = search(&pool, &root, "daft discovery", "").await.unwrap();
        let found: Vec<_> = folders
            .folders
            .iter()
            .map(|folder| folder.path.as_str())
            .collect();
        assert_eq!(found, ["Daft Punk/Discovery (2001)"]);
        let artist = search(&pool, &root, "daft", "").await.unwrap();
        assert_eq!(artist.folders[0].path, "Daft Punk");
        assert_eq!(artist.folders[0].songs, 2);

        assert!(
            search(&pool, &root, "   ", "")
                .await
                .unwrap()
                .songs
                .is_empty()
        );
        assert!(matches!(
            search(&pool, &root, "lucky", "../").await,
            Err(BrowseError::Outside)
        ));
    }

    #[tokio::test]
    async fn a_save_writes_only_what_changed_and_can_be_undone() {
        if std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_err()
        {
            return;
        }
        let (_directory, root, pool) = library().await;
        let relative = "Radiohead/OK Computer (1997)/12 - Edit Me.flac";
        let file = root.join(relative);
        let status = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
            .arg("sine=frequency=440:duration=1")
            .args([
                "-metadata",
                "title=Edit Me",
                "-metadata",
                "artist=Radiohead",
                "-metadata",
                "genre=Alternative",
                "-metadata",
                "composer=Thom Yorke",
            ])
            .arg(&file)
            .status()
            .unwrap();
        assert!(status.success());
        let opened = song(&root, relative).await.unwrap();

        // A file that changed since it was opened is not overwritten.
        let changes = SongChanges {
            title: Some(" Edit Me ".into()),
            genre: Some("Art Rock".into()),
            composer: Some(String::new()),
            ..SongChanges::default()
        };
        assert!(matches!(
            edit_song(&pool, &root, relative, "0-0", &changes).await,
            Err(BrowseError::Changed)
        ));

        // Bad input is refused before anything is written.
        for bad in [
            SongChanges {
                title: Some(String::new()),
                ..SongChanges::default()
            },
            SongChanges {
                year: Some("13".into()),
                ..SongChanges::default()
            },
            SongChanges {
                track: Some("one".into()),
                ..SongChanges::default()
            },
            SongChanges {
                isrc: Some("nope".into()),
                ..SongChanges::default()
            },
            SongChanges {
                album: Some("two\nlines".into()),
                ..SongChanges::default()
            },
        ] {
            assert!(matches!(
                edit_song(&pool, &root, relative, &opened.version, &bad).await,
                Err(BrowseError::Invalid(_))
            ));
        }
        assert!(matches!(
            edit_song(&pool, &root, "../x.flac", &opened.version, &changes).await,
            Err(BrowseError::Outside)
        ));

        // The title is the same once trimmed, so only two fields change.
        let saved = edit_song(&pool, &root, relative, &opened.version, &changes)
            .await
            .unwrap();
        assert_eq!(saved.changed, 2);
        assert_eq!(saved.before.genre.as_deref(), Some("Alternative"));
        assert_eq!(saved.before.composer.as_deref(), Some("Thom Yorke"));
        assert_eq!(saved.before.title, None);
        assert_eq!(saved.song.tags.genre.as_deref(), Some("Art Rock"));
        assert_eq!(saved.song.tags.composer, None);
        assert_ne!(saved.song.version, opened.version);
        // No hidden copy is left in the album folder, and the index knows the song.
        let leftovers: Vec<_> = std::fs::read_dir(file.parent().unwrap())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().starts_with('.'))
            .collect();
        assert!(leftovers.is_empty());
        let indexed: Option<String> =
            sqlx::query_scalar("SELECT title FROM library_items WHERE path=?")
                .bind(file.to_string_lossy().as_ref())
                .fetch_optional(&pool)
                .await
                .unwrap();
        assert_eq!(indexed.as_deref(), Some("Edit Me"));

        // Saving the old values again is the undo.
        let undone = edit_song(&pool, &root, relative, &saved.song.version, &saved.before)
            .await
            .unwrap();
        assert_eq!(undone.song.tags.genre.as_deref(), Some("Alternative"));
        assert_eq!(undone.song.tags.composer.as_deref(), Some("Thom Yorke"));

        // Nothing new to write leaves the file alone.
        let current = SongChanges {
            genre: Some("Alternative".into()),
            ..SongChanges::default()
        };
        let same = edit_song(&pool, &root, relative, &undone.song.version, &current)
            .await
            .unwrap();
        assert_eq!(same.changed, 0);
        assert_eq!(same.song.version, undone.song.version);
    }

    #[tokio::test]
    async fn song_details_are_read_from_the_file() {
        if std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_err()
        {
            return;
        }
        let (_directory, root, _pool) = library().await;
        let file = root.join("Radiohead/OK Computer (1997)/12 - Tagged.flac");
        let status = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
            .arg("sine=frequency=440:duration=2")
            .args([
                "-metadata",
                "title=Tagged",
                "-metadata",
                "artist=Radiohead",
                "-metadata",
                "album=OK Computer",
                "-metadata",
                "genre=Alternative",
                "-metadata",
                "track=12/12",
            ])
            .arg(&file)
            .status()
            .unwrap();
        assert!(status.success());

        let details = song(&root, "Radiohead/OK Computer (1997)/12 - Tagged.flac")
            .await
            .unwrap();
        assert_eq!(details.tags.title.as_deref(), Some("Tagged"));
        assert_eq!(details.tags.genre.as_deref(), Some("Alternative"));
        assert_eq!(details.tags.track, Some(12));
        assert_eq!(details.folder, "Radiohead/OK Computer (1997)");
        assert_eq!(details.format, "flac");
        assert!(details.tags.duration > 1.5);
        assert!(!details.tags.has_cover);

        assert!(matches!(
            song(&root, "Radiohead").await,
            Err(BrowseError::NotFound)
        ));
    }
}
