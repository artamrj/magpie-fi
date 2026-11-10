use crate::{
    infrastructure::providers::{ArtworkStatus, Candidate},
    types::{CandidateId, TrackId, TrackStage},
};
use serde::Serialize;
use sqlx::FromRow;
use std::collections::HashMap;

pub const TRACK_FIELDS: &str = "id,path,output_path,filename,format,bitrate,duration,content_fingerprint,current_title,current_artist,current_album,current_album_artist,current_track_number,selected_candidate_id,status,error,is_missing,stage,stage_message";

/// Every candidate column except `raw_json`, which is only kept for the record.
pub const CANDIDATE_FIELDS: &str = "id,track_id,provider,title,artist,artist_credits_json,album,album_artist,album_artist_credits_json,track_number,track_total,disc_number,disc_total,year,genre,composer,label,isrc,cover_url,artwork_candidates_json,artwork_status,artwork_message,musicbrainz_recording_id,musicbrainz_release_id,release_country,release_date,release_type,release_secondary_types,is_compilation,duration_delta,score_breakdown,musicbrainz_artist_id,musicbrainz_album_artist_id,score";

#[derive(Clone, Debug, Serialize, FromRow)]
pub struct Track {
    pub(crate) id: TrackId,
    #[serde(skip_serializing)]
    pub(crate) path: String,
    #[serde(skip_serializing)]
    pub(crate) output_path: Option<String>,
    pub(crate) filename: String,
    pub(crate) format: Option<String>,
    #[serde(skip_serializing)]
    pub(crate) bitrate: Option<i64>,
    pub(crate) duration: Option<f64>,
    #[serde(skip_serializing)]
    pub(crate) content_fingerprint: Option<String>,
    pub(crate) current_title: Option<String>,
    pub(crate) current_artist: Option<String>,
    pub(crate) current_album: Option<String>,
    pub(crate) current_album_artist: Option<String>,
    pub(crate) current_track_number: Option<i64>,
    pub(crate) selected_candidate_id: Option<CandidateId>,
    pub(crate) status: String,
    pub(crate) error: Option<String>,
    pub(crate) is_missing: bool,
    pub(crate) stage: TrackStage,
    pub(crate) stage_message: Option<String>,
}

#[derive(Clone, Debug, FromRow)]
pub struct CandidateRow {
    pub(crate) id: CandidateId,
    pub(crate) track_id: TrackId,
    pub(crate) provider: String,
    pub(crate) title: Option<String>,
    pub(crate) artist: Option<String>,
    pub(crate) artist_credits_json: Option<String>,
    pub(crate) album: Option<String>,
    pub(crate) album_artist: Option<String>,
    pub(crate) album_artist_credits_json: Option<String>,
    pub(crate) track_number: Option<i64>,
    pub(crate) track_total: Option<i64>,
    pub(crate) disc_number: Option<i64>,
    pub(crate) disc_total: Option<i64>,
    pub(crate) year: Option<String>,
    pub(crate) genre: Option<String>,
    pub(crate) composer: Option<String>,
    pub(crate) label: Option<String>,
    pub(crate) isrc: Option<String>,
    pub(crate) cover_url: Option<String>,
    pub(crate) artwork_candidates_json: Option<String>,
    pub(crate) artwork_status: ArtworkStatus,
    pub(crate) artwork_message: Option<String>,
    pub(crate) musicbrainz_recording_id: Option<String>,
    pub(crate) musicbrainz_release_id: Option<String>,
    pub(crate) release_country: Option<String>,
    pub(crate) release_date: Option<String>,
    pub(crate) release_type: Option<String>,
    pub(crate) release_secondary_types: Option<String>,
    pub(crate) is_compilation: bool,
    pub(crate) duration_delta: Option<f64>,
    pub(crate) score_breakdown: Option<String>,
    pub(crate) musicbrainz_artist_id: Option<String>,
    pub(crate) musicbrainz_album_artist_id: Option<String>,
    pub(crate) score: f64,
}

impl CandidateRow {
    pub fn value(&self) -> Candidate {
        let artist =
            crate::domain::credits::prefer_latin_alias(self.artist.as_deref().unwrap_or_default());
        let credits = crate::domain::credits::normalize_structured(
            &artist,
            self.title.as_deref().unwrap_or_default(),
            json_or_default(self.artist_credits_json.as_deref()),
        );
        Candidate {
            id: Some(self.id.0),
            provider: self.provider.clone(),
            title: credits.title,
            artist: credits.artist,
            artist_credits: credits.artists,
            album: self.album.clone(),
            album_artist: self
                .album_artist
                .as_deref()
                .map(crate::domain::credits::prefer_latin_alias),
            album_artist_credits: json_or_default(self.album_artist_credits_json.as_deref()),
            track_number: self.track_number,
            track_total: self.track_total,
            disc_number: self.disc_number,
            disc_total: self.disc_total,
            year: self.year.clone(),
            genre: self.genre.clone(),
            composer: self.composer.clone(),
            label: self.label.clone(),
            isrc: self.isrc.clone(),
            cover_url: self.cover_url.clone(),
            artwork_candidates: json_or_default(self.artwork_candidates_json.as_deref()),
            artwork_status: self.artwork_status,
            artwork_message: self.artwork_message.clone(),
            recording_id: self.musicbrainz_recording_id.clone(),
            release_id: self.musicbrainz_release_id.clone(),
            release_country: self.release_country.clone(),
            release_date: self.release_date.clone(),
            release_type: self.release_type.clone(),
            release_secondary_types: self.release_secondary_types.clone(),
            is_compilation: self.is_compilation,
            duration_delta: self.duration_delta,
            score_breakdown: self.score_breakdown.clone(),
            artist_id: self.musicbrainz_artist_id.clone(),
            album_artist_id: self.musicbrainz_album_artist_id.clone(),
            score: self.score,
            raw_json: String::new(),
        }
    }
}

fn json_or_default<T: serde::de::DeserializeOwned + Default>(raw: Option<&str>) -> T {
    raw.and_then(|raw| serde_json::from_str(raw).ok())
        .unwrap_or_default()
}

#[derive(Debug, thiserror::Error)]
pub enum TrackError {
    #[error("{0}")]
    NotFound(String),
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

pub async fn track(pool: &sqlx::SqlitePool, id: TrackId) -> Result<Track, TrackError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {TRACK_FIELDS} FROM tracks WHERE id=?"
    )))
    .bind(id.0)
    .fetch_one(pool)
    .await?)
}

pub async fn candidate(
    pool: &sqlx::SqlitePool,
    id: CandidateId,
) -> Result<Option<Candidate>, TrackError> {
    let row: Option<CandidateRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {CANDIDATE_FIELDS} FROM candidates WHERE id=?"
    )))
    .bind(id.0)
    .fetch_optional(pool)
    .await?;
    Ok(row.as_ref().map(CandidateRow::value))
}

pub async fn selected(
    pool: &sqlx::SqlitePool,
    id: TrackId,
) -> Result<(Track, Candidate), TrackError> {
    let track = track(pool, id).await?;
    let cid = track
        .selected_candidate_id
        .ok_or_else(|| TrackError::NotFound("track has no selected candidate".into()))?;
    let candidate = candidate(pool, cid)
        .await?
        .ok_or_else(|| TrackError::NotFound("selected candidate no longer exists".into()))?;
    Ok((track, candidate))
}

/// Pairs each track with its selected candidate.
pub async fn selected_for_tracks(
    pool: &sqlx::SqlitePool,
    tracks: Vec<Track>,
) -> Result<Vec<(Track, Candidate)>, TrackError> {
    let ids = tracks
        .iter()
        .filter_map(|track| track.selected_candidate_id.map(|id| id.0))
        .collect::<Vec<_>>();
    let rows: Vec<CandidateRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {CANDIDATE_FIELDS} FROM candidates WHERE id IN (SELECT value FROM json_each(?))"
    )))
    .bind(serde_json::json!(ids).to_string())
    .fetch_all(pool)
    .await?;
    let mut candidates = rows
        .into_iter()
        .map(|row| (row.id.0, row.value()))
        .collect::<HashMap<_, _>>();
    let mut out = Vec::with_capacity(tracks.len());
    for track in tracks {
        let candidate_id = track
            .selected_candidate_id
            .ok_or_else(|| TrackError::NotFound("track has no selected candidate".into()))?;
        let candidate = candidates
            .remove(&candidate_id.0)
            .ok_or_else(|| TrackError::NotFound("selected candidate no longer exists".into()))?;
        out.push((track, candidate));
    }
    Ok(out)
}

pub async fn candidates(
    pool: &sqlx::SqlitePool,
    id: TrackId,
) -> Result<Vec<Candidate>, TrackError> {
    let rows: Vec<CandidateRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {CANDIDATE_FIELDS} FROM candidates WHERE track_id=? ORDER BY score DESC"
    )))
    .bind(id.0)
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(CandidateRow::value).collect())
}

/// The candidates of these tracks, best score first.
pub async fn candidates_by_track(
    pool: &sqlx::SqlitePool,
    tracks: impl IntoIterator<Item = TrackId>,
) -> Result<HashMap<TrackId, Vec<Candidate>>, TrackError> {
    let ids = tracks.into_iter().map(|id| id.0).collect::<Vec<_>>();
    let rows: Vec<CandidateRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {CANDIDATE_FIELDS} FROM candidates
         WHERE track_id IN (SELECT value FROM json_each(?))
         ORDER BY track_id, score DESC"
    )))
    .bind(serde_json::json!(ids).to_string())
    .fetch_all(pool)
    .await?;
    let mut by_track = HashMap::<TrackId, Vec<Candidate>>::new();
    for row in rows {
        by_track.entry(row.track_id).or_default().push(row.value());
    }
    Ok(by_track)
}
