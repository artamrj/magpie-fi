use crate::{app::AppState, http::handlers};
use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use std::sync::Arc;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/health", get(health))
        .route("/me", get(crate::http::auth::me))
        .route("/login", get(crate::http::auth::login))
        .route("/logout", post(crate::http::auth::logout))
        .route("/library/recent", get(handlers::recent_library))
        .route("/setup", get(handlers::setup))
        .route("/status", get(handlers::workspace))
        .route("/inbox/check", post(handlers::check_inbox))
        .route("/stop", post(handlers::stop_scan))
        .route("/tracks", get(handlers::list_tracks))
        .route("/artists", get(handlers::list_artists))
        .route("/artists/{id}", post(handlers::change_artist))
        .route("/artists/{id}/albums", get(handlers::split_albums))
        .route("/files", get(handlers::list_folder))
        .route("/files/search", get(handlers::search_files))
        .route(
            "/files/song",
            get(handlers::song_details).put(handlers::save_song),
        )
        .route("/files/songs", axum::routing::put(handlers::save_songs))
        .route("/files/folder-tags", get(handlers::folder_tags))
        .route("/files/artwork", get(handlers::song_artwork))
        .route("/files/audio", get(handlers::song_audio))
        .route(
            "/tracks/{id}",
            axum::routing::delete(handlers::remove_track),
        )
        .route("/tracks/retry-issues", post(handlers::retry_issues))
        .route("/tracks/auto-approve", post(handlers::auto_approve_review))
        .route("/tracks/{id}/audio", get(handlers::track_audio))
        .route("/source/resolve", post(handlers::resolve_source))
        .route("/tracks/{id}/choose", post(handlers::select_candidate))
        .route("/tracks/{id}/review", post(handlers::return_to_review))
        .route("/tracks/{id}/file-anyway", post(handlers::file_anyway))
        .route(
            "/tracks/{id}/manual",
            axum::routing::put(handlers::manual_candidate),
        )
        .route(
            "/tracks/{id}/artwork",
            axum::routing::put(handlers::update_artwork),
        )
        .route(
            "/tracks/{id}/artwork/search",
            post(handlers::search_artwork),
        )
        .route(
            "/tracks/{id}/artwork/preview",
            get(handlers::artwork_preview),
        )
        .route(
            "/tracks/{id}/artwork/original",
            get(handlers::original_artwork_preview),
        )
        .route(
            "/candidates/{id}/artwork/preview",
            get(handlers::candidate_artwork_preview),
        )
        .route("/write", post(handlers::start_apply))
        .route(
            "/requests",
            get(handlers::list_requests).post(handlers::create_request),
        )
        .route("/requests/{id}", get(handlers::request_songs))
        .route("/requests/{id}/retry", post(handlers::retry_request))
        .fallback(api_not_found)
}

async fn health(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match sqlx::query_scalar::<_, i64>("SELECT 1")
        .fetch_one(&state.pool)
        .await
    {
        Ok(1) => (StatusCode::OK, Json(serde_json::json!({"status":"ok"}))),
        Ok(_) | Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"status":"unavailable"})),
        ),
    }
}

async fn api_not_found() -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({"error":"API route not found"})),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{app::AppState, config::Config, infrastructure::db};
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    async fn test_state() -> Arc<AppState> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("router.sqlite");
        let pool = db::connect(path.to_str().unwrap()).await.unwrap();
        std::mem::forget(dir);
        Arc::new(AppState::new(Config::default(), pool))
    }

    #[tokio::test]
    async fn unknown_api_route_returns_json_404() {
        let app = router().with_state(test_state().await);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/missing")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["error"], "API route not found");
    }

    #[tokio::test]
    async fn the_files_tab_refuses_paths_outside_the_library() {
        let state = test_state().await;
        for uri in [
            "/files?path=..%2Fsecret",
            "/files/song?path=%2Fetc%2Fpasswd",
            "/files/audio?path=a%2F..%2F..%2Fx.mp3",
            "/files/artwork?path=.magpie-fi%2Fstaging%2Fx.mp3",
            "/files/search?q=x&in=..",
        ] {
            let response = router()
                .with_state(state.clone())
                .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                StatusCode::UNPROCESSABLE_ENTITY,
                "{uri} must be refused"
            );
        }
    }
}
