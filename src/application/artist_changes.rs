//! The owner's fixes to artists: rename, merge two into one, split a credit
//! such as `Ebi & Shadmehr Aghili` into separate artists, or mark a name as one
//! artist (a band). Renames, merges and splits also rewrite the artist tags of
//! the songs (on a checked copy that then replaces the file) and move them to
//! the right artist folder; playlists follow.
//!
//! Callers hold the workflow lock, so no scan or filing runs at the same time.

use crate::{
    app::AppState,
    application::{
        artists::{self, ids_from_text, ids_text},
        files, library,
    },
    domain::{credits::ArtistCredit, names::match_key},
    infrastructure::{lyrics, media::tag_writer},
};
use anyhow::{Context, Result, bail};
use chrono::Utc;
use serde::Serialize;
use sqlx::SqlitePool;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Debug, Serialize)]
pub struct ArtistSummary {
    pub id: i64,
    pub name: String,
    pub spellings: Vec<String>,
    pub songs: i64,
    pub musicbrainz_id: Option<String>,
    /// Known to be one artist although the name has "&" or ",".
    pub single_entity: bool,
    /// The name looks like several artists ("A & B", "A, B", "A x B").
    pub looks_like_several: bool,
    /// Albums of this artist whose songs spell the album name differently ("Khalij" and
    /// "Khaleej"), so Navidrome shows them twice. A quick look at the index only; opening
    /// the artist checks every album detail (see `album_changes::split_albums`).
    pub albums_to_check: usize,
}

#[derive(Debug, Default, Serialize, PartialEq, Eq)]
pub struct Changed {
    /// Songs whose tags were rewritten.
    pub retagged: usize,
    /// Songs moved to another artist folder.
    pub moved: usize,
}

/// id, name, MusicBrainz ID, one artist, songs, spellings.
type SummaryRow = (i64, String, Option<String>, bool, i64, Option<String>);

/// Every artist with songs in the library, most songs first.
pub async fn list(pool: &SqlitePool) -> Result<Vec<ArtistSummary>> {
    let rows: Vec<SummaryRow> = sqlx::query_as(
        "SELECT artists.id, artists.name, artists.musicbrainz_id, artists.single_entity,
                (SELECT count(*) FROM library_items
                  WHERE artist_ids LIKE '%|' || artists.id || '|%') AS songs,
                (SELECT group_concat(spelling, char(31)) FROM artist_names
                  WHERE artist_names.artist_id=artists.id) AS spellings
         FROM artists WHERE songs > 0 ORDER BY songs DESC, artists.name",
    )
    .fetch_all(pool)
    .await?;
    let albums_to_check = albums_spelled_twice(pool).await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, name, musicbrainz_id, single_entity, songs, spellings)| {
                let mut spellings = spellings
                    .unwrap_or_default()
                    .split('\u{1f}')
                    .filter(|spelling| !spelling.is_empty() && *spelling != name)
                    .map(str::to_owned)
                    .collect::<Vec<_>>();
                spellings.sort();
                spellings.dedup();
                let looks_like_several = !single_entity
                    && [
                        " & ", ", ", " x ", " X ", " و ", "، ", " with ", " vs ", " feat", " ft. ",
                    ]
                    .iter()
                    .any(|separator| name.contains(separator));
                ArtistSummary {
                    albums_to_check: albums_to_check.get(&id).copied().unwrap_or(0),
                    id,
                    name,
                    spellings,
                    songs,
                    musicbrainz_id,
                    single_entity,
                    looks_like_several,
                }
            },
        )
        .collect())
}

/// Per main artist: how many albums have songs that spell the album name differently.
async fn albums_spelled_twice(pool: &SqlitePool) -> Result<std::collections::HashMap<i64, usize>> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT artist_ids, album FROM library_items
         WHERE artist_ids IS NOT NULL AND album IS NOT NULL AND album<>''",
    )
    .fetch_all(pool)
    .await?;
    let mut spellings = std::collections::HashMap::<(i64, String), BTreeSet<String>>::new();
    for (ids, album) in rows {
        let Some(&main) = ids_from_text(&ids).first() else {
            continue;
        };
        let key = crate::domain::names::album_key(&album);
        if !key.is_empty() {
            spellings.entry((main, key)).or_default().insert(album);
        }
    }
    let mut counts = std::collections::HashMap::new();
    for ((artist, _), names) in spellings {
        if names.len() > 1 {
            *counts.entry(artist).or_default() += 1;
        }
    }
    Ok(counts)
}

pub async fn rename(state: &Arc<AppState>, id: i64, new_name: &str) -> Result<Changed> {
    let new_name = crate::domain::credits::clean_text(new_name);
    if new_name.is_empty() {
        bail!("the new name is empty");
    }
    let spellings = spellings(&state.pool, id).await?;
    if let Some(owner) = owner_of(&state.pool, &new_name).await?
        && owner != id
    {
        bail!("another artist already has this name; merge them instead");
    }
    sqlx::query("UPDATE artists SET name=?,name_source='owner',updated_at=? WHERE id=?")
        .bind(&new_name)
        .bind(Utc::now().to_rfc3339())
        .bind(id)
        .execute(&state.pool)
        .await?;
    add_spelling(&state.pool, id, &new_name).await?;
    rewrite_songs(state, id, &spellings, std::slice::from_ref(&new_name)).await
}

/// `from` becomes part of `into`: its spellings, its songs and its folder.
pub async fn merge(state: &Arc<AppState>, from: i64, into: i64) -> Result<Changed> {
    if from == into {
        bail!("an artist cannot be merged into itself");
    }
    let target_name: String = sqlx::query_scalar("SELECT name FROM artists WHERE id=?")
        .bind(into)
        .fetch_optional(&state.pool)
        .await?
        .context("the artist to merge into does not exist")?;
    let spellings = spellings(&state.pool, from).await?;
    let mut transaction = state.pool.begin().await?;
    sqlx::query("UPDATE artist_names SET artist_id=? WHERE artist_id=?")
        .bind(into)
        .bind(from)
        .execute(&mut *transaction)
        .await?;
    // Keep the MusicBrainz ID of the merged artist when the other has none.
    let moved_id: Option<String> =
        sqlx::query_scalar("SELECT musicbrainz_id FROM artists WHERE id=?")
            .bind(from)
            .fetch_one(&mut *transaction)
            .await?;
    sqlx::query("DELETE FROM artists WHERE id=?")
        .bind(from)
        .execute(&mut *transaction)
        .await?;
    if let Some(moved_id) = moved_id {
        sqlx::query("UPDATE artists SET musicbrainz_id=? WHERE id=? AND musicbrainz_id IS NULL")
            .bind(moved_id)
            .bind(into)
            .execute(&mut *transaction)
            .await?;
    }
    transaction.commit().await?;
    replace_ids(&state.pool, from, &[into]).await?;
    rewrite_songs(state, into, &spellings, &[target_name]).await
}

/// `id` names several artists: from now on its spellings mean `parts`, and its
/// songs are filed under the first part with every part in the ARTISTS tag.
pub async fn split(state: &Arc<AppState>, id: i64, parts: &[String]) -> Result<Changed> {
    let parts = parts
        .iter()
        .map(|part| crate::domain::credits::clean_text(part))
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.len() < 2 {
        bail!("give at least two artists");
    }
    let spellings = spellings(&state.pool, id).await?;
    let credits = parts
        .iter()
        .map(|part| ArtistCredit::new(part, ""))
        .collect::<Vec<_>>();
    let part_ids = artists::resolve_names(&state.pool, credits).await?;
    if part_ids.contains(&id) {
        bail!("one of the parts is this same artist");
    }
    let part_names = part_names(&state.pool, &part_ids).await?;
    let now = Utc::now().to_rfc3339();
    let parts_json = serde_json::to_string(&part_names)?;
    let mut transaction = state.pool.begin().await?;
    for spelling in &spellings {
        sqlx::query(
            "INSERT INTO artist_splits(match_key,spelling,parts_json,created_at) VALUES(?,?,?,?)
             ON CONFLICT(match_key) DO UPDATE SET parts_json=excluded.parts_json",
        )
        .bind(match_key(spelling))
        .bind(spelling)
        .bind(&parts_json)
        .bind(&now)
        .execute(&mut *transaction)
        .await?;
    }
    sqlx::query("DELETE FROM artists WHERE id=?")
        .bind(id)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    replace_ids(&state.pool, id, &part_ids).await?;
    rewrite_songs(state, part_ids[0], &spellings, &part_names).await
}

/// Artists learned with a featured artist inside their name ("Tommee Profitt
/// feat. Fleurie", from a source that sent it as one name) are split into
/// their artists, and their songs move to the main artist's folder. `feat.`
/// never belongs to a name, so this needs no owner decision. Returns how many.
pub async fn split_featured_artists(state: &Arc<AppState>) -> Result<usize> {
    let rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT id,name FROM artists
         WHERE name LIKE '% feat. %' OR name LIKE '% feat %' OR name LIKE '% ft. %'
            OR name LIKE '% ft %' OR name LIKE '% featuring %'",
    )
    .fetch_all(&state.pool)
    .await?;
    let mut done = 0;
    for (id, name) in rows {
        let parts = crate::domain::credits::normalize_featured(&name, "")
            .artists
            .into_iter()
            .map(|credit| credit.name)
            .collect::<Vec<_>>();
        if parts.len() < 2 {
            continue;
        }
        match split(state, id, &parts).await {
            Ok(changed) => {
                tracing::info!(
                    artist = name,
                    moved = changed.moved,
                    "split a featured artist out of an artist name"
                );
                done += 1;
            }
            Err(error) => tracing::warn!(
                artist = name,
                error = format!("{error:#}"),
                "could not split a featured artist"
            ),
        }
    }
    Ok(done)
}

/// "This is one artist" (a band or duo with "&" in its name): never split it.
pub async fn mark_one_artist(pool: &SqlitePool, id: i64) -> Result<()> {
    let changed = sqlx::query("UPDATE artists SET single_entity=1,updated_at=? WHERE id=?")
        .bind(Utc::now().to_rfc3339())
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected();
    if changed == 0 {
        bail!("artist not found");
    }
    Ok(())
}

async fn spellings(pool: &SqlitePool, id: i64) -> Result<Vec<String>> {
    let name: String = sqlx::query_scalar("SELECT name FROM artists WHERE id=?")
        .bind(id)
        .fetch_optional(pool)
        .await?
        .context("artist not found")?;
    let mut spellings: Vec<String> =
        sqlx::query_scalar("SELECT spelling FROM artist_names WHERE artist_id=?")
            .bind(id)
            .fetch_all(pool)
            .await?;
    spellings.insert(0, name);
    let mut seen = BTreeSet::new();
    spellings.retain(|spelling| seen.insert(match_key(spelling)));
    Ok(spellings)
}

async fn owner_of(pool: &SqlitePool, name: &str) -> Result<Option<i64>> {
    Ok(
        sqlx::query_scalar("SELECT artist_id FROM artist_names WHERE match_key=?")
            .bind(match_key(name))
            .fetch_optional(pool)
            .await?,
    )
}

async fn add_spelling(pool: &SqlitePool, id: i64, spelling: &str) -> Result<()> {
    sqlx::query(
        "INSERT INTO artist_names(match_key,artist_id,spelling,latin_key,source) VALUES(?,?,?,?,'owner')
         ON CONFLICT(match_key) DO UPDATE SET artist_id=excluded.artist_id",
    )
    .bind(match_key(spelling))
    .bind(id)
    .bind(spelling)
    .bind(crate::domain::names::latin_key(spelling))
    .execute(pool)
    .await?;
    Ok(())
}

async fn part_names(pool: &SqlitePool, ids: &[i64]) -> Result<Vec<String>> {
    let mut names = Vec::with_capacity(ids.len());
    for id in ids {
        names.push(
            sqlx::query_scalar("SELECT name FROM artists WHERE id=?")
                .bind(id)
                .fetch_one(pool)
                .await?,
        );
    }
    Ok(names)
}

/// Library rows that listed `old` now list `new` in its place.
async fn replace_ids(pool: &SqlitePool, old: i64, new: &[i64]) -> Result<()> {
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT path,artist_ids FROM library_items WHERE artist_ids LIKE ?")
            .bind(format!("%|{old}|%"))
            .fetch_all(pool)
            .await?;
    for (path, ids) in rows {
        let mut replaced = Vec::new();
        for id in ids_from_text(&ids) {
            for id in if id == old { new.to_vec() } else { vec![id] } {
                if !replaced.contains(&id) {
                    replaced.push(id);
                }
            }
        }
        sqlx::query("UPDATE library_items SET artist_ids=? WHERE path=?")
            .bind(ids_text(&replaced))
            .bind(&path)
            .execute(pool)
            .await?;
    }
    Ok(())
}

/// Rewrites the artist tags of `artist`'s songs from any of `old_spellings`
/// to `new_names`, and moves songs out of folders named after an old spelling
/// into the folder of `new_names[0]`.
async fn rewrite_songs(
    state: &Arc<AppState>,
    artist: i64,
    old_spellings: &[String],
    new_names: &[String],
) -> Result<Changed> {
    let library_root = PathBuf::from(&state.config.output_dir);
    let new_folder = library_root.join(library::safe_name(&new_names[0], "Unknown Artist"));
    let old_folders = old_spellings
        .iter()
        .map(|spelling| library_root.join(library::safe_name(spelling, "Unknown Artist")))
        .filter(|folder| *folder != new_folder)
        .collect::<BTreeSet<_>>();

    let mut songs: BTreeSet<PathBuf> =
        sqlx::query_scalar::<_, String>("SELECT path FROM library_items WHERE artist_ids LIKE ?")
            .bind(format!("%|{artist}|%"))
            .fetch_all(&state.pool)
            .await?
            .into_iter()
            .map(PathBuf::from)
            .collect();
    for folder in &old_folders {
        let folder = folder.clone();
        let (found, _) = tokio::task::spawn_blocking(move || files::audio_files(&folder)).await?;
        songs.extend(found);
    }

    let mut changed = Changed::default();
    // "EBI" → "Ebi" on a disk that ignores case: the folder itself is renamed.
    for folder in &old_folders {
        if same_folder(folder, &new_folder) {
            let moved = rename_folder(state, folder, &new_folder).await?;
            songs = songs
                .into_iter()
                .map(|song| match song.strip_prefix(folder) {
                    Ok(rest) => new_folder.join(rest),
                    Err(_) => song,
                })
                .collect();
            changed.moved += moved;
        }
    }
    let old_folders = old_folders
        .into_iter()
        .filter(|folder| !same_folder(folder, &new_folder))
        .collect::<BTreeSet<_>>();
    for song in songs {
        if !song.is_file() {
            continue;
        }
        let (path, old, new) = (song.clone(), old_spellings.to_vec(), new_names.to_vec());
        match tokio::task::spawn_blocking(move || retag(&path, &old, &new)).await? {
            Ok(true) => changed.retagged += 1,
            Ok(false) => {}
            Err(error) => {
                tracing::warn!(error = format!("{error:#}"), song = %song.display(), "could not rewrite the artist tags");
                continue;
            }
        }
        let Some(rest) = old_folders
            .iter()
            .find_map(|folder| song.strip_prefix(folder).ok())
        else {
            continue;
        };
        let target = unused(new_folder.join(rest)).await?;
        move_song(state, &library_root, &song, &target).await?;
        changed.moved += 1;
    }
    Ok(changed)
}

/// Two names for one folder (a disk that ignores letter case).
fn same_folder(left: &Path, right: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::metadata(left), std::fs::metadata(right)) {
        (Ok(left), Ok(right)) => left.dev() == right.dev() && left.ino() == right.ino(),
        _ => false,
    }
}

/// Renames a whole artist folder through a temporary name (so a change of
/// letter case also works) and updates every song that was in it.
async fn rename_folder(state: &Arc<AppState>, from: &Path, to: &Path) -> Result<usize> {
    let folder = from.to_path_buf();
    let (songs, _) = tokio::task::spawn_blocking(move || files::audio_files(&folder)).await?;
    let name = from
        .file_name()
        .context("folder has no name")?
        .to_string_lossy();
    let temporary = from.with_file_name(format!(".{name}.magpie-fi-rename"));
    tokio::fs::rename(from, &temporary).await?;
    if let Err(error) = tokio::fs::rename(&temporary, to).await {
        // Back to its old name: a hidden folder would vanish from the library.
        tokio::fs::rename(&temporary, from).await.with_context(|| {
            format!(
                "the artist folder is left as {} and must be renamed back by hand",
                temporary.display()
            )
        })?;
        return Err(error)
            .with_context(|| format!("could not rename {} to {}", from.display(), to.display()));
    }
    for song in &songs {
        let Ok(rest) = song.strip_prefix(from) else {
            continue;
        };
        let moved = to.join(rest);
        sqlx::query("UPDATE library_items SET path=? WHERE path=?")
            .bind(moved.to_string_lossy().as_ref())
            .bind(song.to_string_lossy().as_ref())
            .execute(&state.pool)
            .await?;
        crate::acquire::library_file_moved(state, song, &moved).await;
    }
    Ok(songs.len())
}

/// Replaces every old spelling on a copy, then swaps the copy in.
fn retag(song: &Path, old_spellings: &[String], new_names: &[String]) -> Result<bool> {
    files::edit_on_copy(song, "artist", |copy| {
        let mut changed = false;
        for old in old_spellings {
            changed |= tag_writer::replace_artist(copy, old, new_names)?;
        }
        Ok(changed)
    })
}

pub(crate) async fn unused(target: PathBuf) -> Result<PathBuf> {
    let mut candidate = target.clone();
    let mut number = 2;
    while tokio::fs::try_exists(&candidate).await? {
        candidate = files::numbered(&target, number);
        number += 1;
    }
    Ok(candidate)
}

/// Moves a song (and its lyrics files, and the album cover once the album
/// folder is empty) and updates the index, the songs and the playlists.
pub(crate) async fn move_song(
    state: &Arc<AppState>,
    root: &Path,
    from: &Path,
    to: &Path,
) -> Result<()> {
    if let Some(parent) = to.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::rename(from, to)
        .await
        .with_context(|| format!("could not move {} to {}", from.display(), to.display()))?;
    let trash = &state.config.trash_dir;
    for sidecar in lyrics::sidecars(from) {
        let Some(extension) = sidecar.extension() else {
            continue;
        };
        let target = to.with_extension(extension);
        // A rename would silently replace lyrics that are already there
        // (another format of the same song), so those are kept and this
        // file goes to the trash.
        if tokio::fs::try_exists(&target).await.unwrap_or(true) {
            if let Err(error) = library::move_to_trash(trash, root, &sidecar).await {
                tracing::warn!(%error, file = %sidecar.display(), "could not move a lyrics file to the trash");
            }
        } else {
            let _ = tokio::fs::rename(&sidecar, &target).await;
        }
    }
    if let (Some(old_album), Some(new_album)) = (from.parent(), to.parent()) {
        move_lonely_covers(trash, root, old_album, new_album).await;
    }
    sqlx::query("UPDATE library_items SET path=? WHERE path=?")
        .bind(to.to_string_lossy().as_ref())
        .bind(from.to_string_lossy().as_ref())
        .execute(&state.pool)
        .await?;
    crate::acquire::library_file_moved(state, from, to).await;
    library::remove_empty_parents(from, root).await;
    Ok(())
}

/// When only cover images are left in the old album folder, they follow.
/// A cover the new folder already has is kept; the old one goes to the trash.
async fn move_lonely_covers(trash: &Path, root: &Path, old_album: &Path, new_album: &Path) {
    let Ok(mut entries) = tokio::fs::read_dir(old_album).await else {
        return;
    };
    let mut covers = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        if name.starts_with("cover.") {
            covers.push(entry.path());
        } else {
            return;
        }
    }
    for cover in covers {
        if let Some(name) = cover.file_name() {
            let target = new_album.join(name);
            match tokio::fs::try_exists(&target).await {
                Ok(true) => {
                    if let Err(error) = library::move_to_trash(trash, root, &cover).await {
                        tracing::warn!(%error, file = %cover.display(), "could not move a cover to the trash");
                    }
                }
                Ok(false) => {
                    let _ = tokio::fs::rename(&cover, &target).await;
                }
                // Unknown: leave the cover where it is.
                Err(_) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::providers::Candidate;

    async fn state(directory: &Path) -> Arc<AppState> {
        let mut config =
            crate::config::Config::with_folders(&directory.join("music"), &directory.join("data"));
        config.output_dir = directory.join("library").to_string_lossy().into_owned();
        let pool =
            crate::infrastructure::db::connect(directory.join("changes.sqlite").to_str().unwrap())
                .await
                .unwrap();
        Arc::new(AppState::new(config, pool))
    }

    #[tokio::test]
    async fn moving_a_song_never_overwrites_lyrics_or_deletes_a_cover() {
        let directory = tempfile::tempdir().unwrap();
        let state = state(directory.path()).await;
        let root = PathBuf::from(&state.config.output_dir);
        let old_album = root.join("Old/Album");
        let new_album = root.join("New/Album");
        std::fs::create_dir_all(&old_album).unwrap();
        std::fs::create_dir_all(&new_album).unwrap();
        // The new folder already has the song in another format, its lyrics and a cover.
        std::fs::write(new_album.join("01 - Song.flac"), b"flac").unwrap();
        std::fs::write(new_album.join("01 - Song.lrc"), b"lyrics that were there").unwrap();
        std::fs::write(new_album.join("cover.jpg"), b"cover that was there").unwrap();
        let from = old_album.join("01 - Song.mp3");
        std::fs::write(&from, b"mp3").unwrap();
        std::fs::write(old_album.join("01 - Song.lrc"), b"lyrics that move").unwrap();
        std::fs::write(old_album.join("cover.jpg"), b"cover that moves").unwrap();

        let to = new_album.join("01 - Song.mp3");
        move_song(&state, &root, &from, &to).await.unwrap();

        assert_eq!(std::fs::read(&to).unwrap(), b"mp3");
        assert_eq!(
            std::fs::read(new_album.join("01 - Song.lrc")).unwrap(),
            b"lyrics that were there"
        );
        assert_eq!(
            std::fs::read(new_album.join("cover.jpg")).unwrap(),
            b"cover that was there"
        );
        // What could not move is in the trash, not gone.
        let trashed = crate::application::files::audio_files(&state.config.trash_dir).0;
        assert!(trashed.is_empty());
        let mut kept = Vec::new();
        for entry in walkdir::WalkDir::new(&state.config.trash_dir)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file())
        {
            kept.push(std::fs::read(entry.path()).unwrap());
        }
        kept.sort();
        assert_eq!(
            kept,
            vec![b"cover that moves".to_vec(), b"lyrics that move".to_vec()]
        );
        assert!(!old_album.exists(), "the empty old folder is removed");
    }

    #[tokio::test]
    async fn a_folder_rename_that_fails_puts_the_folder_back() {
        let directory = tempfile::tempdir().unwrap();
        let state = state(directory.path()).await;
        let root = PathBuf::from(&state.config.output_dir);
        let from = root.join("Artist");
        std::fs::create_dir_all(from.join("Album")).unwrap();
        std::fs::write(from.join("Album/01 - Song.mp3"), b"mp3").unwrap();
        // The new name is taken by a folder with songs: the rename cannot happen.
        let to = root.join("Taken");
        std::fs::create_dir_all(to.join("Album")).unwrap();
        std::fs::write(to.join("Album/other.mp3"), b"other").unwrap();

        assert!(rename_folder(&state, &from, &to).await.is_err());

        assert_eq!(
            std::fs::read(from.join("Album/01 - Song.mp3")).unwrap(),
            b"mp3"
        );
        assert_eq!(std::fs::read(to.join("Album/other.mp3")).unwrap(), b"other");
        assert!(!root.join(".Artist.magpie-fi-rename").exists());
    }

    /// A filed song by `artist`, with its index row.
    async fn filed(state: &Arc<AppState>, relative: &str, artist: &str) -> PathBuf {
        let path = PathBuf::from(&state.config.output_dir).join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let status = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
            .arg("sine=frequency=440:duration=0.2")
            .args(["-c:a", "aac"])
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success());
        let candidate = Candidate {
            title: "Song".into(),
            artist: artist.into(),
            album: Some("Song".into()),
            album_artist: Some(artist.into()),
            ..Default::default()
        };
        tag_writer::write(&path, &candidate, None, None).unwrap();
        let names = crate::domain::credits::normalize_featured(artist, "Song");
        let ids = artists::resolve_names(&state.pool, names.artists)
            .await
            .unwrap();
        library::remember(
            &state.pool,
            &library::LibraryEntry {
                path: path.to_string_lossy().into_owned(),
                duration: Some(0.2),
                title: Some("Song".into()),
                format: "m4a".into(),
                source: "inbox".into(),
                artist_ids: ids,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        path
    }

    fn installed() -> bool {
        std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_ok()
    }

    fn artists_tag(path: &Path) -> Vec<String> {
        use lofty::{file::TaggedFileExt, probe::Probe, tag::ItemKey};
        let file = Probe::open(path).unwrap().read().unwrap();
        file.primary_tag()
            .unwrap()
            .get_strings(ItemKey::TrackArtists)
            .map(str::to_owned)
            .collect()
    }

    async fn id_of(state: &Arc<AppState>, name: &str) -> i64 {
        sqlx::query_scalar("SELECT artist_id FROM artist_names WHERE match_key=?")
            .bind(match_key(name))
            .fetch_one(&state.pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn splitting_a_duet_moves_it_under_the_first_singer() {
        if !installed() {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let state = state(directory.path()).await;
        let duet = filed(
            &state,
            "Ebi & Shadmehr Aghili/Song (2024) [Single]/01 - Song.m4a",
            "Ebi & Shadmehr Aghili",
        )
        .await;
        let combined = id_of(&state, "Ebi & Shadmehr Aghili").await;

        let changed = split(&state, combined, &["Ebi".into(), "Shadmehr Aghili".into()])
            .await
            .unwrap();

        assert_eq!(
            changed,
            Changed {
                retagged: 1,
                moved: 1
            }
        );
        let moved =
            PathBuf::from(&state.config.output_dir).join("Ebi/Song (2024) [Single]/01 - Song.m4a");
        assert!(moved.is_file());
        assert!(!duet.exists());
        assert!(!duet.parent().unwrap().parent().unwrap().exists());
        assert_eq!(artists_tag(&moved), ["Ebi", "Shadmehr Aghili"]);
        // The library index and later songs follow the split.
        let listed = list(&state.pool).await.unwrap();
        let names = listed
            .iter()
            .map(|artist| artist.name.as_str())
            .collect::<Vec<_>>();
        assert!(
            names.contains(&"Ebi") && names.contains(&"Shadmehr Aghili"),
            "{names:?}"
        );
        assert!(!names.contains(&"Ebi & Shadmehr Aghili"));
    }

    #[tokio::test]
    async fn merging_two_spellings_gives_one_folder() {
        if !installed() {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let state = state(directory.path()).await;
        filed(&state, "Googoosh/Pol (2000)/01 - Song.m4a", "Googoosh").await;
        let persian = filed(&state, "گوگوش/Kooch (2004)/01 - Song.m4a", "گوگوش").await;
        let (latin, script) = (
            id_of(&state, "Googoosh").await,
            id_of(&state, "گوگوش").await,
        );
        assert_ne!(latin, script);

        let changed = merge(&state, script, latin).await.unwrap();

        assert_eq!(changed.moved, 1);
        let moved =
            PathBuf::from(&state.config.output_dir).join("Googoosh/Kooch (2004)/01 - Song.m4a");
        assert!(moved.is_file() && !persian.exists());
        assert_eq!(artists_tag(&moved), ["Googoosh"]);
        // The Persian spelling now means the same artist.
        assert_eq!(id_of(&state, "گوگوش").await, latin);
        let listed = list(&state.pool).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].songs, 2);
        assert!(listed[0].spellings.contains(&"گوگوش".to_owned()));
    }

    #[tokio::test]
    async fn a_featured_artist_folder_is_repaired() {
        if !installed() {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let state = state(directory.path()).await;
        // Learned by an older version as one artist, with its own folder.
        sqlx::query("INSERT INTO artists(id,name,created_at,updated_at) VALUES(50,'Tommee Profitt feat. Fleurie','now','now')")
            .execute(&state.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO artist_names(match_key,artist_id,spelling,source) VALUES(?,50,'Tommee Profitt feat. Fleurie','old')")
            .bind(match_key("Tommee Profitt feat. Fleurie"))
            .execute(&state.pool)
            .await
            .unwrap();
        let relative = "Tommee Profitt feat. Fleurie/In the End (2019)/02 - In the End.m4a";
        let path = PathBuf::from(&state.config.output_dir).join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let status = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
            .arg("sine=frequency=440:duration=0.2")
            .args(["-c:a", "aac"])
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success());
        let candidate = Candidate {
            title: "In the End".into(),
            artist: "Tommee Profitt feat. Fleurie".into(),
            album: Some("In the End".into()),
            album_artist: Some("Tommee Profitt feat. Fleurie".into()),
            artist_credits: vec![ArtistCredit::new("Tommee Profitt feat. Fleurie", "")],
            ..Default::default()
        };
        tag_writer::write(&path, &candidate, None, None).unwrap();
        library::remember(
            &state.pool,
            &library::LibraryEntry {
                path: path.to_string_lossy().into_owned(),
                title: Some("In the End".into()),
                format: "m4a".into(),
                source: "inbox".into(),
                artist_ids: vec![50],
                ..Default::default()
            },
        )
        .await
        .unwrap();

        assert_eq!(split_featured_artists(&state).await.unwrap(), 1);

        let moved = PathBuf::from(&state.config.output_dir)
            .join("Tommee Profitt/In the End (2019)/02 - In the End.m4a");
        assert!(moved.is_file() && !path.exists());
        assert_eq!(artists_tag(&moved), ["Tommee Profitt", "Fleurie"]);
        assert_eq!(split_featured_artists(&state).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn renaming_rewrites_tags_and_folder() {
        if !installed() {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let state = state(directory.path()).await;
        filed(&state, "EBI/Khalij (1999)/01 - Song.m4a", "EBI").await;
        let id = id_of(&state, "EBI").await;
        rename(&state, id, "Ebi").await.unwrap();
        let moved = PathBuf::from(&state.config.output_dir).join("Ebi/Khalij (1999)/01 - Song.m4a");
        assert!(moved.is_file());
        assert_eq!(artists_tag(&moved), ["Ebi"]);
        mark_one_artist(&state.pool, id).await.unwrap();
    }
}
