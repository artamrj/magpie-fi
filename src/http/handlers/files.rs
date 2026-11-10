//! The Files tab: browse and search `library/`, look at one song, fix its tags.

use super::tracks::{artwork_response, audio_mime, stream_audio};
use super::*;
use crate::application::file_browser::{
    self, BrowseError, Folder, Saved, SearchResult, SongChanges, SongDetails,
};
use axum::{extract::Query, http::HeaderMap};

#[derive(Deserialize)]
pub struct PathQuery {
    #[serde(default)]
    path: String,
}

#[derive(Deserialize)]
pub struct SearchQuery {
    #[serde(default)]
    q: String,
    /// A folder to search in; the whole library when empty.
    #[serde(default, rename = "in")]
    within: String,
}

impl From<BrowseError> for ApiError {
    fn from(error: BrowseError) -> Self {
        match error {
            BrowseError::Outside => ApiError::validation(error.to_string()),
            BrowseError::NotFound => ApiError::not_found(error.to_string()),
            BrowseError::Invalid(message) => ApiError::validation(message),
            BrowseError::Changed => ApiError::conflict(error.to_string()),
            BrowseError::Other(error) => ApiError::Internal(error),
        }
    }
}

fn library(s: &AppState) -> PathBuf {
    PathBuf::from(&s.config.output_dir)
}

/// `GET /api/files?path=` — one folder of the library.
pub async fn list_folder(
    State(s): State<Arc<AppState>>,
    Query(query): Query<PathQuery>,
) -> ApiResult<Json<Folder>> {
    Ok(Json(
        file_browser::list_folder(&s.pool, &library(&s), &query.path).await?,
    ))
}

/// `GET /api/files/search?q=&in=` — songs and folders matching every word.
pub async fn search_files(
    State(s): State<Arc<AppState>>,
    Query(query): Query<SearchQuery>,
) -> ApiResult<Json<SearchResult>> {
    Ok(Json(
        file_browser::search(&s.pool, &library(&s), &query.q, &query.within).await?,
    ))
}

/// `GET /api/files/song?path=` — every tag of one song, read from the file.
pub async fn song_details(
    State(s): State<Arc<AppState>>,
    Query(query): Query<PathQuery>,
) -> ApiResult<Json<SongDetails>> {
    Ok(Json(file_browser::song(&library(&s), &query.path).await?))
}

#[derive(Deserialize)]
pub struct SongEdit {
    path: String,
    /// The `version` the page read; a file changed since then is not overwritten.
    version: String,
    changes: SongChanges,
}

/// `PUT /api/files/song` — writes the changed fields into the file. Runs in the
/// workflow slot, so no scan or filing touches the library meanwhile.
pub async fn save_song(
    State(s): State<Arc<AppState>>,
    Json(body): Json<SongEdit>,
) -> ApiResult<Json<Saved>> {
    if !s
        .try_claim_workflow(WorkflowPhase::Apply, "Saving a song's tags")
        .await
    {
        return Err(ApiError::conflict(
            "Wait for the current scan or write operation to finish",
        ));
    }
    let result = file_browser::edit_song(
        &s.pool,
        &library(&s),
        &body.path,
        &body.version,
        &body.changes,
    )
    .await;
    s.reset_workflow(WorkflowPhase::Idle, "Ready").await;
    Ok(Json(result?))
}

/// `GET /api/files/artwork?path=` — the cover inside the song.
pub async fn song_artwork(
    State(s): State<Arc<AppState>>,
    Query(query): Query<PathQuery>,
) -> ApiResult<axum::response::Response> {
    let file = file_browser::song_file(&library(&s), &query.path)?;
    let artwork = tokio::task::spawn_blocking(move || tag_writer::read_artwork(&file))
        .await
        .map_err(anyhow::Error::from)??
        .ok_or_else(|| ApiError::not_found("This song has no cover inside"))?;
    artwork_response(artwork)
}

/// `GET /api/files/audio?path=` — the song itself, for the player.
pub async fn song_audio(
    State(s): State<Arc<AppState>>,
    Query(query): Query<PathQuery>,
    headers: HeaderMap,
) -> ApiResult<axum::response::Response> {
    let file = file_browser::song_file(&library(&s), &query.path)?;
    let format = file
        .extension()
        .map(|extension| extension.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    stream_audio(&file, audio_mime(&format), &headers).await
}
