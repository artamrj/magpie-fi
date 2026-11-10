//! The Files tab: browse and search `library/`, and look at one song. Read-only.

use super::tracks::{artwork_response, audio_mime, stream_audio};
use super::*;
use crate::application::file_browser::{self, BrowseError, Folder, SearchResult, SongDetails};
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
