//! The download worker. For every wanted song:
//! already in the library? → find the right YouTube Music upload → download it
//! into `.magpie/staging/` → write what the link said as tags → hand it to the
//! inbox cycle, which identifies and files it like any other new music.

use super::{
    links,
    matching::{self, Candidate, Target},
    store::{self, WantedRow},
    ytdlp,
};
use crate::{app::AppState, application::input_dedup, config::Config};
use anyhow::Result;
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::Semaphore;

const IDLE_WAIT: Duration = Duration::from_secs(60);

enum Outcome {
    /// Already in the library at this path.
    InLibrary(String),
    /// Downloaded into staging; the inbox cycle takes it from here.
    Downloaded { path: PathBuf, youtube_id: String },
}

struct Failure {
    message: String,
    /// Worth trying again later (network, YouTube hiccup).
    retry: bool,
}

impl From<anyhow::Error> for Failure {
    fn from(error: anyhow::Error) -> Self {
        Self {
            message: format!("{error:#}"),
            retry: true,
        }
    }
}

/// Runs forever: claims due songs, at most `max_parallel_downloads` at a time.
pub async fn run(state: Arc<AppState>) {
    match store::reset_interrupted(&state.pool).await {
        Ok(0) => {}
        Ok(count) => tracing::info!(count, "resuming downloads interrupted by a restart"),
        Err(error) => tracing::warn!(%error, "could not resume interrupted downloads"),
    }
    let parallel = state.config.read().await.max_parallel_downloads;
    let slots = Arc::new(Semaphore::new(parallel));
    loop {
        loop {
            let Ok(slot) = slots.clone().acquire_owned().await else {
                return;
            };
            match store::claim_next(&state.pool).await {
                Ok(Some(row)) => {
                    let state = state.clone();
                    tokio::spawn(async move {
                        process(&state, row).await;
                        drop(slot);
                        state.notify_acquire();
                    });
                }
                Ok(None) => {
                    drop(slot);
                    super::scan_for_playlists(&state).await;
                    break;
                }
                Err(error) => {
                    tracing::warn!(%error, "could not read the download queue");
                    break;
                }
            }
        }
        let wait = match store::next_due(&state.pool).await {
            Ok(Some(due)) => (due - chrono::Utc::now())
                .to_std()
                .unwrap_or(Duration::ZERO)
                .min(IDLE_WAIT),
            _ => IDLE_WAIT,
        };
        tokio::select! {
            () = tokio::time::sleep(wait) => {}
            () = state.wait_for_acquire() => {}
        }
    }
}

async fn process(state: &Arc<AppState>, row: WantedRow) {
    let config = state.config.read().await.clone();
    let label = match row.artist.as_deref() {
        Some(artist) => format!("{artist} – {}", row.title),
        None => row.title.clone(),
    };
    match acquire(state, &config, &row).await {
        Ok(Outcome::InLibrary(path)) => {
            tracing::info!(song = %label, %path, "already in the library");
            if let Err(error) = store::mark_in_library(&state.pool, row.id, &path).await {
                tracing::warn!(%error, "could not record a library song");
            }
            super::refresh_playlists(state, &[row.request_id], true).await;
        }
        Ok(Outcome::Downloaded { path, youtube_id }) => {
            tracing::info!(song = %label, file = %path.display(), "downloaded; identifying next");
            if let Err(error) =
                store::mark_identifying(&state.pool, row.id, &path, &youtube_id).await
            {
                tracing::warn!(%error, "could not record a download");
            }
            // The inbox cycle identifies and files it right away.
            state.notify_automation_scheduler();
            tokio::time::sleep(Duration::from_secs(config.download_pause_seconds)).await;
        }
        Err(failure) => {
            let again =
                store::mark_attempt_failed(&state.pool, &row, &failure.message, failure.retry)
                    .await
                    .unwrap_or(false);
            tracing::warn!(song = %label, error = %failure.message, retry = again, "download failed");
        }
    }
}

async fn acquire(
    state: &Arc<AppState>,
    config: &Config,
    row: &WantedRow,
) -> std::result::Result<Outcome, Failure> {
    if let Some(path) = in_library(state, row).await? {
        return Ok(Outcome::InLibrary(path));
    }
    let youtube_id = match row.youtube_id.clone() {
        Some(id) => id,
        None => choose_source(config, row).await?,
    };
    if let Some(path) = store::known_source(&state.pool, Some(&youtube_id), None).await? {
        return Ok(Outcome::InLibrary(path));
    }
    tokio::fs::create_dir_all(&config.staging_dir)
        .await
        .map_err(anyhow::Error::from)?;
    let path = ytdlp::download(
        config,
        &youtube_id,
        &config.staging_dir,
        &format!("w{}", row.id),
    )
    .await
    .map_err(|error| Failure {
        retry: !format!("{error:#}").contains("unavailable"),
        message: format!("{error:#}"),
    })?;
    let tagged = path.clone();
    let song = row.clone();
    if let Err(error) = tokio::task::spawn_blocking(move || write_request_tags(&tagged, &song))
        .await
        .map_err(anyhow::Error::from)
        .and_then(|result| result)
    {
        tracing::warn!(%error, "could not write the request's tags; identification uses the file as is");
    }
    Ok(Outcome::Downloaded { path, youtube_id })
}

/// Is this song already in the library? By the link it came from, its ISRC,
/// or the same title (versions kept apart) and first artist.
async fn in_library(state: &Arc<AppState>, row: &WantedRow) -> Result<Option<String>> {
    if let Some(path) = store::known_source(
        &state.pool,
        row.youtube_id.as_deref(),
        row.source_url.as_deref(),
    )
    .await?
    {
        return Ok(Some(path));
    }
    if let Some(isrc) = row.isrc.as_deref().and_then(input_dedup::normalize_isrc) {
        let paths: Vec<String> = sqlx::query_scalar("SELECT path FROM library_items WHERE isrc=?")
            .bind(isrc)
            .fetch_all(&state.pool)
            .await?;
        for path in paths {
            if tokio::fs::try_exists(&path).await.unwrap_or(false) {
                return Ok(Some(path));
            }
        }
    }
    let Some(artist) = row.artist.as_deref() else {
        return Ok(None);
    };
    let title = matching::identity_title(&row.title);
    let artist = matching::normalize(&matching::primary_artist(artist));
    if title.is_empty() || artist.is_empty() {
        return Ok(None);
    }
    let rows: Vec<(String, Option<String>, Option<String>, Option<f64>)> = sqlx::query_as(
        "SELECT path,title,artist,duration FROM library_items WHERE title IS NOT NULL",
    )
    .fetch_all(&state.pool)
    .await?;
    for (path, library_title, library_artist, duration) in rows {
        let same_title = library_title
            .as_deref()
            .is_some_and(|value| matching::identity_title(value) == title);
        let same_artist = library_artist.as_deref().is_some_and(|value| {
            let value = matching::normalize(value);
            value == artist || format!(" {value} ").contains(&format!(" {artist} "))
        });
        let close = match (row.duration, duration) {
            (Some(left), Some(right)) => (left - right).abs() <= 5.0,
            _ => true,
        };
        if same_title && same_artist && close && tokio::fs::try_exists(&path).await.unwrap_or(false)
        {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

/// Finds the YouTube Music song (or, failing that, a strict video match).
async fn choose_source(config: &Config, row: &WantedRow) -> std::result::Result<String, Failure> {
    let target = Target {
        title: row.title.clone(),
        artist: row.artist.clone(),
        duration: row.duration,
    };
    let query = match row.artist.as_deref() {
        Some(artist) => format!("{} {}", matching::primary_artist(artist), row.title),
        None => row.title.clone(),
    };

    let songs = ytdlp::search_music_songs(config, &query, 5).await?;
    let mut checked = Vec::new();
    for entry in songs
        .iter()
        .filter(|entry| {
            matching::similarity(
                &matching::base_title(&row.title),
                &matching::base_title(&entry.title),
            ) >= 60.0
        })
        .take(3)
    {
        let Ok(details) = ytdlp::details(config, &entry.id).await else {
            continue;
        };
        checked.push(Candidate {
            id: entry.id.clone(),
            title: details.title,
            artists: details.artists,
            duration: details.duration,
            official: true,
        });
        if let Some((best, score)) = matching::best(&target, &checked, matching::NORMAL)
            && score.total >= 90.0
        {
            return Ok(best.id.clone());
        }
    }
    if let Some((best, _)) = matching::best(&target, &checked, matching::NORMAL) {
        return Ok(best.id.clone());
    }

    let videos: Vec<Candidate> = ytdlp::search_videos(config, &query, 5)
        .await?
        .into_iter()
        .map(|entry| Candidate {
            title: matching::clean_video_title(&entry.title, &entry.artists),
            id: entry.id,
            artists: entry.artists,
            duration: entry.duration,
            official: false,
        })
        .collect();
    matching::best(&target, &videos, matching::STRICT)
        .map(|(best, _)| best.id.clone())
        .ok_or_else(|| Failure {
            message: "no matching song on YouTube Music or YouTube".into(),
            retry: false,
        })
}

/// Writes what the link said (title, artists, album, track number, ISRC) into
/// the download, so identification starts from the right song.
fn write_request_tags(path: &std::path::Path, row: &WantedRow) -> Result<()> {
    use lofty::{
        config::WriteOptions,
        file::TaggedFileExt,
        prelude::*,
        probe::Probe,
        tag::{ItemKey, Tag},
    };
    let mut file = Probe::open(path)?.guess_file_type()?.read()?;
    if file.primary_tag().is_none() {
        let kind = file.primary_tag_type();
        file.insert_tag(Tag::new(kind));
    }
    let tag = file
        .primary_tag_mut()
        .ok_or_else(|| anyhow::anyhow!("no tag could be added"))?;
    tag.set_title(row.title.clone());
    if let Some(artist) = &row.artist {
        tag.set_artist(artist.clone());
    }
    if let Some(album) = &row.album {
        tag.set_album(album.clone());
    }
    if let Some(track) = row.track_number.and_then(|value| u32::try_from(value).ok()) {
        tag.set_track(track);
    }
    if let Some(isrc) = &row.isrc {
        tag.insert_text(ItemKey::Isrc, isrc.clone());
    }
    file.save_to_path(path, WriteOptions::default())?;
    Ok(())
}

/// Reads a link and queues its songs (used by the web page and later by Telegram).
pub async fn read_request(state: Arc<AppState>, request_id: i64, link: links::Link) {
    let config = state.config.read().await.clone();
    match links::read(&state.client, &state.pool, &config, &link).await {
        Ok(collection) => {
            let count = collection.songs.len();
            match store::save_collection(
                &state.pool,
                request_id,
                &link.url,
                link.scope,
                &collection,
            )
            .await
            {
                Ok(()) => {
                    tracing::info!(request_id, count, title = ?collection.title, "link read; songs queued");
                    state.notify_acquire();
                }
                Err(error) => {
                    let _ =
                        store::request_failed(&state.pool, request_id, &format!("{error:#}")).await;
                }
            }
        }
        Err(error) => {
            tracing::warn!(request_id, %error, "could not read link");
            let _ = store::request_failed(&state.pool, request_id, &format!("{error:#}")).await;
        }
    }
}
