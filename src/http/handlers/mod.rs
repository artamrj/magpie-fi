use crate::{
    app::{AppState, state::Workflow},
    http::error::{ApiError, ApiResult},
    infrastructure::{
        db::tracks::{self as track_db, Track},
        media::tag_writer,
        providers::Candidate,
    },
    types::{CandidateId, TrackId, WorkflowPhase},
};
use anyhow::Result;
use axum::{
    Json,
    extract::{Path, State},
};
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Arc};

mod apply;
mod artists;
mod files;
mod requests;
mod scan;
mod settings;
mod tracks;

pub use apply::start_apply;
pub use artists::{change_artist, list_artists, split_albums};
pub use files::{
    folder_tags, list_folder, save_song, save_songs, search_files, song_artwork, song_audio,
    song_details,
};
pub use requests::{create_request, list_requests, recent_library, request_songs, retry_request};
pub use scan::{check_inbox, retry_issues, run_automatic_cycle, stop_scan};
pub use settings::setup;
pub use tracks::{
    artwork_preview, auto_approve_review, candidate_artwork_preview, file_anyway, keep_file_tags,
    list_tracks, manual_candidate, original_artwork_preview, remove_track, resolve_source,
    return_to_review, search_artwork, select_candidate, track_audio, update_artwork,
};

/// `GET /api/status`: what the background workflow is doing.
pub async fn workspace(State(s): State<Arc<AppState>>) -> Json<Workflow> {
    Json(s.workflow.read().await.clone())
}

#[derive(Deserialize)]
pub struct SelectRequest {
    pub(crate) candidate_id: Option<CandidateId>,
}

#[derive(Deserialize)]
pub struct CandidateEdit {
    title: String,
    artist: String,
    #[serde(default)]
    artist_credits: Vec<crate::domain::credits::ArtistCredit>,
    album: Option<String>,
    album_artist: Option<String>,
    #[serde(default)]
    album_artist_credits: Vec<crate::domain::credits::ArtistCredit>,
    track_number: Option<i64>,
    track_total: Option<i64>,
    disc_number: Option<i64>,
    disc_total: Option<i64>,
    year: Option<String>,
    genre: Option<String>,
    composer: Option<String>,
    label: Option<String>,
    isrc: Option<String>,
    release_date: Option<String>,
    cover_url: Option<String>,
}

#[derive(Deserialize)]
pub struct ArtworkEdit {
    cover_url: String,
}

#[derive(Deserialize)]
pub struct SourceLookupRequest {
    url: String,
}

#[derive(Serialize)]
pub struct WorkspaceTrack {
    #[serde(flatten)]
    track: Track,
    candidates: Vec<Candidate>,
}

#[derive(Serialize)]
pub struct AutoApproveResult {
    approved: u64,
    low_confidence: u64,
    unavailable: u64,
}
