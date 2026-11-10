use crate::{
    app::AppState,
    application::scan as scan_pipeline,
    http::error::{ApiError, ApiResult},
    infrastructure::{media::tag_writer, providers::Candidate},
    types::{CandidateId, TrackId, WorkflowPhase},
};
use anyhow::Result;
use axum::{
    Json,
    extract::{Path, State},
};
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Arc};

pub(super) use crate::infrastructure::db::tracks::{CandidateRow, Track};

mod apply;
mod queries;
mod requests;
mod scan;
mod settings;
mod tracks;
mod workspace;

pub use apply::start_apply;
pub use requests::{create_request, list_requests, request_songs, retry_request};
pub use scan::{check_inbox, retry_issues, run_automatic_cycle, stop_scan};
pub use settings::setup;
pub use tracks::{
    artwork_preview, auto_approve_review, candidate_artwork_preview, list_tracks, manual_candidate,
    original_artwork_preview, remove_track, resolve_source, return_to_review, search_artwork,
    select_candidate, track_audio, update_artwork,
};
pub use workspace::workspace;

#[derive(Deserialize)]
pub struct SelectRequest {
    candidate_id: Option<CandidateId>,
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
pub struct TrackPage {
    items: Vec<WorkspaceTrack>,
    total: i64,
}

#[derive(Serialize)]
pub struct AutoApproveResult {
    approved: u64,
    remaining: i64,
    low_confidence: u64,
    unavailable: u64,
}
