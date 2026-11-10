//! "Get music": paste a song, album or playlist link; follow its progress.

use super::*;
use crate::acquire::{links, store, worker};

#[derive(Deserialize)]
pub struct NewRequest {
    url: String,
}

/// `POST /api/requests` — reads the link in the background and queues its songs.
pub async fn create_request(
    State(s): State<Arc<AppState>>,
    Json(body): Json<NewRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let raw = body.url.trim();
    if raw.is_empty() {
        return Err(ApiError::validation("Paste a song, album or playlist link"));
    }
    let url = links::expand_short_link(&s.client, raw)
        .await
        .unwrap_or_else(|_| raw.to_owned());
    let Some(link) = links::classify(&url) else {
        return Err(ApiError::validation(
            "magpie-fi can read Spotify, Apple Music, Deezer, YouTube, YouTube Music and Shazam links",
        ));
    };
    let id = store::create_request(&s.pool, &link.url, link.scope, "web", None).await?;
    tokio::spawn(worker::read_request(s.clone(), id, link));
    Ok(Json(serde_json::json!({"id": id})))
}

/// `GET /api/requests` — the latest requests with their progress.
pub async fn list_requests(
    State(s): State<Arc<AppState>>,
) -> ApiResult<Json<Vec<store::RequestSummary>>> {
    Ok(Json(store::list_requests(&s.pool, 30).await?))
}

/// `GET /api/requests/{id}` — every song of one request.
pub async fn request_songs(
    State(s): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Vec<store::WantedView>>> {
    Ok(Json(store::request_songs(&s.pool, id).await?))
}

/// `POST /api/requests/{id}/retry` — failed songs go back into the queue.
pub async fn retry_request(
    State(s): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    let queued = store::retry_failed(&s.pool, id).await?;
    s.notify_acquire();
    Ok(Json(serde_json::json!({"queued": queued})))
}

#[derive(Serialize, sqlx::FromRow)]
pub struct RecentSong {
    title: Option<String>,
    artist: Option<String>,
    album: Option<String>,
    added_at: String,
}

/// `GET /api/library/recent` — the songs filed most recently.
pub async fn recent_library(State(s): State<Arc<AppState>>) -> ApiResult<Json<Vec<RecentSong>>> {
    Ok(Json(
        sqlx::query_as(
            "SELECT title,artist,album,added_at FROM library_items WHERE source='inbox'
             ORDER BY added_at DESC LIMIT 30",
        )
        .fetch_all(&s.pool)
        .await?,
    ))
}
