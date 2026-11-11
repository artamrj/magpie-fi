//! Albums Navidrome shows twice. Navidrome tells albums apart by name, album artist,
//! MusicBrainz album ID and release date (see [`AlbumTags`]), so one album whose songs
//! disagree on any of them ("Khalij" and "Khaleej", "1999" and "1999-03-01") appears as two.
//! New songs take their album's details when they are filed
//! ([`library::align_with_existing_release`]); this finds albums that were split before and
//! makes them one when the owner asks on the Artists page: every song takes the details most
//! of the album has, and moves into the album's folder.
//!
//! Callers hold the workflow lock, so no scan or filing runs at the same time.

use crate::{
    app::AppState,
    application::{
        artist_changes::{self, Changed},
        files, library,
    },
    domain::names::{album_key, match_key},
    infrastructure::media::tag_writer::{self, AlbumTags, TagEdit},
};
use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
};

/// One album of an artist that Navidrome shows more than once.
#[derive(Debug, Serialize)]
pub struct SplitAlbum {
    /// Sent back to make it one album.
    pub key: String,
    /// The name it keeps.
    pub album: String,
    pub songs: usize,
    /// Each way the album is tagged, the one it keeps first.
    pub versions: Vec<AlbumVersion>,
}

#[derive(Debug, Serialize)]
pub struct AlbumVersion {
    pub album: String,
    pub year: Option<String>,
    /// "single" or "EP"; none for an album.
    pub kind: Option<String>,
    pub songs: usize,
    /// The details every song gets.
    pub kept: bool,
    /// What sets it apart from the one kept, in plain words.
    pub differs_in: Vec<String>,
}

#[derive(Clone, Debug)]
struct Song {
    path: PathBuf,
    tags: AlbumTags,
}

#[derive(Debug)]
struct Group {
    key: String,
    songs: Vec<Song>,
}

/// What a split album becomes.
#[derive(Debug)]
struct Plan {
    album: AlbumTags,
    folder: PathBuf,
    summary: SplitAlbum,
}

/// The artist's albums that Navidrome shows more than once.
pub async fn split_albums(state: &Arc<AppState>, artist: i64) -> Result<Vec<SplitAlbum>> {
    let folders = artist_folders(state, artist).await?;
    let groups = tokio::task::spawn_blocking(move || groups(&folders)).await?;
    Ok(groups
        .iter()
        .filter_map(plan)
        .map(|plan| plan.summary)
        .collect())
}

/// Makes one album out of the split album `key` (from [`split_albums`]).
pub async fn join(state: &Arc<AppState>, artist: i64, key: &str) -> Result<Changed> {
    let folders = artist_folders(state, artist).await?;
    let groups = tokio::task::spawn_blocking(move || groups(&folders)).await?;
    let Some((group, plan)) = groups
        .iter()
        .find(|group| group.key == key)
        .and_then(|group| plan(group).map(|plan| (group, plan)))
    else {
        bail!("this album is already one album");
    };
    let root = PathBuf::from(&state.config.output_dir);
    let mut changed = Changed::default();
    for song in &group.songs {
        let new_kind = kind(&song.tags) != kind(&plan.album);
        if song.tags != plan.album || new_kind {
            let (path, album) = (song.path.clone(), plan.album.clone());
            let written = tokio::task::spawn_blocking(move || {
                files::edit_on_copy(&path, "album", |copy| {
                    let mut written = tag_writer::write_album_tags(copy, &album)?;
                    if new_kind {
                        // A single's "track 1 of 1" is not its place on the album.
                        tag_writer::edit_fields(
                            copy,
                            &TagEdit {
                                track: Some(None),
                                track_total: Some(None),
                                disc: Some(None),
                                disc_total: Some(None),
                                ..TagEdit::default()
                            },
                        )?;
                        written = true;
                    }
                    Ok(written)
                })
            })
            .await??;
            if written {
                changed.retagged += 1;
                sqlx::query("UPDATE library_items SET album=? WHERE path=?")
                    .bind(&plan.album.album)
                    .bind(song.path.to_string_lossy().as_ref())
                    .execute(&state.pool)
                    .await?;
            }
        }
        if song.path.parent() == Some(plan.folder.as_path()) {
            continue;
        }
        let name = song
            .path
            .file_name()
            .context("song has no name")?
            .to_string_lossy()
            .into_owned();
        let name = if new_kind {
            without_track_number(&name)
        } else {
            name
        };
        let target = artist_changes::unused(plan.folder.join(name)).await?;
        artist_changes::move_song(state, &root, &song.path, &target).await?;
        changed.moved += 1;
    }
    tracing::info!(
        album = ?plan.album.album,
        retagged = changed.retagged,
        moved = changed.moved,
        "made one album"
    );
    Ok(changed)
}

/// The library folders named after this artist (its name or a spelling it has).
async fn artist_folders(state: &Arc<AppState>, artist: i64) -> Result<Vec<PathBuf>> {
    let root = PathBuf::from(&state.config.output_dir);
    let names: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM artists WHERE id=?
         UNION SELECT spelling FROM artist_names WHERE artist_id=?",
    )
    .bind(artist)
    .bind(artist)
    .fetch_all(&state.pool)
    .await?;
    let keys = names
        .iter()
        .map(|name| match_key(&library::safe_name(name, "")))
        .filter(|key| !key.is_empty())
        .collect::<BTreeSet<_>>();
    let paths: Vec<String> =
        sqlx::query_scalar("SELECT path FROM library_items WHERE artist_ids LIKE ?")
            .bind(format!("%|{artist}|%"))
            .fetch_all(&state.pool)
            .await?;
    Ok(paths
        .iter()
        .filter_map(|path| {
            let mut parts = Path::new(path).strip_prefix(&root).ok()?.components();
            let folder = parts.next()?.as_os_str().to_string_lossy().into_owned();
            // Artist/Album/song: a song directly in the artist folder has no album folder.
            parts.next()?;
            parts.next()?;
            keys.contains(&match_key(&folder))
                .then(|| root.join(folder))
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect())
}

/// Every song in the album folders of `artist_folders`, grouped by album. Blocking.
fn groups(artist_folders: &[PathBuf]) -> Vec<Group> {
    let mut groups = BTreeMap::<String, Vec<Song>>::new();
    for artist_folder in artist_folders {
        let Ok(entries) = std::fs::read_dir(artist_folder) else {
            continue;
        };
        let artist_name = artist_folder
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        for entry in entries.flatten() {
            let folder = entry.path();
            let hidden = entry.file_name().to_string_lossy().starts_with('.');
            if hidden || !folder.is_dir() {
                continue;
            }
            for path in files::audio_files(&folder).0 {
                let Ok(tags) = tag_writer::read_album_tags(&path) else {
                    continue;
                };
                let Some(name) = tags.album.as_deref() else {
                    continue;
                };
                let key = album_key(name);
                if key.is_empty() {
                    continue;
                }
                groups
                    .entry(format!("{artist_name}/{key}"))
                    .or_default()
                    .push(Song { path, tags });
            }
        }
    }
    groups
        .into_iter()
        .map(|(key, mut songs)| {
            songs.sort_by(|left, right| left.path.cmp(&right.path));
            Group { key, songs }
        })
        .collect()
}

/// What a group becomes, or `None` when Navidrome already shows it as one album in one
/// folder.
fn plan(group: &Group) -> Option<Plan> {
    let mut versions = BTreeMap::<_, Vec<&Song>>::new();
    for song in &group.songs {
        versions
            .entry(song.tags.grouping_key())
            .or_default()
            .push(song);
    }
    let folders = group
        .songs
        .iter()
        .filter_map(|song| song.path.parent())
        .collect::<BTreeSet<_>>();
    if versions.len() < 2 && folders.len() < 2 {
        return None;
    }
    // Most songs first; then an album over a single, a MusicBrainz release, a full date.
    let rank = |songs: &Vec<&Song>| {
        let tags = &songs[0].tags;
        (
            songs.len(),
            kind(tags).is_none(),
            tags.release_id.is_some(),
            tags.release_date.as_deref().map_or(0, str::len),
        )
    };
    let kept = versions.values().max_by_key(|songs| rank(songs))?.clone();
    let album = kept[0].tags.clone();
    let mut counts = BTreeMap::<&Path, usize>::new();
    for song in &kept {
        if let Some(parent) = song.path.parent() {
            *counts.entry(parent).or_default() += 1;
        }
    }
    let folder = counts
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(folder, _)| folder.to_path_buf())?;
    let mut summary_versions = versions
        .values()
        .map(|songs| {
            let tags = &songs[0].tags;
            AlbumVersion {
                album: tags.album.clone().unwrap_or_default(),
                year: year(tags),
                kind: kind(tags).map(str::to_owned),
                songs: songs.len(),
                kept: tags.grouping_key() == album.grouping_key(),
                differs_in: differences(tags, &album),
            }
        })
        .collect::<Vec<_>>();
    summary_versions.sort_by_key(|version| (!version.kept, std::cmp::Reverse(version.songs)));
    Some(Plan {
        summary: SplitAlbum {
            key: group.key.clone(),
            album: album.album.clone().unwrap_or_default(),
            songs: group.songs.len(),
            versions: summary_versions,
        },
        album,
        folder,
    })
}

/// "single" or "EP" from the release type; `None` for an album.
fn kind(tags: &AlbumTags) -> Option<&'static str> {
    match tags
        .release_type
        .as_deref()
        .map(|kind| kind.trim().to_ascii_lowercase())
        .as_deref()
    {
        Some("single") => Some("single"),
        Some("ep") => Some("EP"),
        _ => None,
    }
}

fn year(tags: &AlbumTags) -> Option<String> {
    [tags.release_date.as_deref(), tags.year.as_deref()]
        .into_iter()
        .flatten()
        .find_map(|date| date.get(..4).map(str::to_owned))
}

/// What sets `tags` apart from the album kept, as Navidrome sees it.
fn differences(tags: &AlbumTags, kept: &AlbumTags) -> Vec<String> {
    let (mine, theirs) = (tags.grouping_key(), kept.grouping_key());
    let mut out = Vec::new();
    if tags.album != kept.album {
        out.push("spelling".to_owned());
    }
    if mine.1 != theirs.1 {
        out.push("album artist".to_owned());
    }
    if mine.2 != theirs.2 {
        out.push("release date".to_owned());
    }
    if mine.3 != theirs.3 {
        out.push("edition".to_owned());
    }
    if kind(tags) != kind(kept) {
        out.push(kind(tags).unwrap_or("album").to_owned());
    }
    out
}

/// "01 - Khaleej.mp3" → "Khaleej.mp3"; "1-03 - Song.flac" → "Song.flac".
fn without_track_number(name: &str) -> String {
    let Some((number, rest)) = name.split_once(" - ") else {
        return name.to_owned();
    };
    let is_number = !number.is_empty()
        && number
            .split('-')
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()));
    if is_number && !rest.is_empty() {
        rest.to_owned()
    } else {
        name.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{application::artists, infrastructure::providers::Candidate};

    async fn state(directory: &Path) -> Arc<AppState> {
        let mut config =
            crate::config::Config::with_folders(&directory.join("music"), &directory.join("data"));
        config.output_dir = directory.join("library").to_string_lossy().into_owned();
        let pool =
            crate::infrastructure::db::connect(directory.join("albums.sqlite").to_str().unwrap())
                .await
                .unwrap();
        Arc::new(AppState::new(config, pool))
    }

    fn installed() -> bool {
        std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_ok()
    }

    /// A filed song with these details, and its index row.
    async fn filed(state: &Arc<AppState>, relative: &str, song: &Candidate) -> PathBuf {
        let path = PathBuf::from(&state.config.output_dir).join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let status = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
            .arg("sine=frequency=440:duration=0.2")
            .args(["-q:a", "9"])
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success());
        tag_writer::write(&path, song, None, None).unwrap();
        let names = crate::domain::credits::normalize_featured(&song.artist, &song.title);
        let ids = artists::resolve_names(&state.pool, names.artists)
            .await
            .unwrap();
        library::remember(
            &state.pool,
            &library::LibraryEntry {
                path: path.to_string_lossy().into_owned(),
                duration: Some(0.2),
                title: Some(song.title.clone()),
                album: song.album.clone(),
                format: "mp3".into(),
                source: "inbox".into(),
                artist_ids: ids,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        path
    }

    fn song(title: &str, album: &str, date: &str, kind: &str) -> Candidate {
        Candidate {
            title: title.into(),
            artist: "Ebi".into(),
            album: Some(album.into()),
            album_artist: Some("Ebi".into()),
            year: Some(date[..4].into()),
            release_date: Some(date.into()),
            release_type: Some(kind.into()),
            track_number: Some(1),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn an_album_split_by_spelling_and_date_becomes_one() {
        if !installed() {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let state = state(directory.path()).await;
        let album = "Ebi/Khalij (1999)";
        filed(
            &state,
            &format!("{album}/01 - Hamdam.mp3"),
            &song("Hamdam", "Khalij", "1999-03-01", "album"),
        )
        .await;
        filed(
            &state,
            &format!("{album}/02 - Kolbeh Man.mp3"),
            &song("Kolbeh Man", "Khalij", "1999-03-01", "album"),
        )
        .await;
        // The same album, another date (Navidrome shows it twice).
        let dated = filed(
            &state,
            &format!("{album}/03 - Medade Rangi.mp3"),
            &song("Medade Rangi", "Khalij", "1999", "album"),
        )
        .await;
        // The title song, spelled another way and filed as a single.
        let single = filed(
            &state,
            "Ebi/Khaleej (1999) [Single]/01 - Khaleej.mp3",
            &song("Khaleej", "Khaleej", "1999", "single"),
        )
        .await;
        // Another album stays as it is.
        filed(
            &state,
            "Ebi/Lalehzaar (2022)/01 - Lalehzaar.mp3",
            &song("Lalehzaar", "Lalehzaar", "2022", "album"),
        )
        .await;
        let ebi: i64 = sqlx::query_scalar("SELECT id FROM artists WHERE name='Ebi'")
            .fetch_one(&state.pool)
            .await
            .unwrap();

        let split = split_albums(&state, ebi).await.unwrap();
        assert_eq!(split.len(), 1, "{split:?}");
        let khalij = &split[0];
        assert_eq!(khalij.album, "Khalij");
        assert_eq!(khalij.songs, 4);
        assert!(khalij.versions[0].kept);
        assert_eq!(khalij.versions[0].songs, 2);
        assert!(
            khalij
                .versions
                .iter()
                .any(|version| version.differs_in.contains(&"spelling".to_owned()))
        );

        let changed = join(&state, ebi, &khalij.key).await.unwrap();
        assert_eq!(changed.retagged, 2);
        assert_eq!(changed.moved, 1);
        let folder = PathBuf::from(&state.config.output_dir).join(album);
        let moved = folder.join("Khaleej.mp3");
        assert!(moved.is_file());
        assert!(!single.exists());
        let wanted = tag_writer::read_album_tags(&folder.join("01 - Hamdam.mp3")).unwrap();
        for path in [&dated, &moved] {
            assert_eq!(tag_writer::read_album_tags(path).unwrap(), wanted);
        }
        assert_eq!(tag_writer::read_details(&moved).unwrap().track, None);
        let indexed: String = sqlx::query_scalar("SELECT album FROM library_items WHERE path=?")
            .bind(moved.to_string_lossy().as_ref())
            .fetch_one(&state.pool)
            .await
            .unwrap();
        assert_eq!(indexed, "Khalij");
        assert!(split_albums(&state, ebi).await.unwrap().is_empty());
        assert!(join(&state, ebi, &khalij.key).await.is_err());
    }

    #[test]
    fn track_numbers_leave_the_file_name() {
        assert_eq!(without_track_number("01 - Khaleej.mp3"), "Khaleej.mp3");
        assert_eq!(without_track_number("1-03 - Song.flac"), "Song.flac");
        assert_eq!(
            without_track_number("Shab - e Yalda.mp3"),
            "Shab - e Yalda.mp3"
        );
        assert_eq!(without_track_number("Song.mp3"), "Song.mp3");
    }
}
