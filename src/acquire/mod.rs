//! Getting music: links → wanted songs → YouTube Music downloads → staging.
//! From staging the inbox cycle identifies and files every download, so a
//! download gets exactly the same treatment as a file you drop in the inbox.

pub mod links;
pub mod matching;
pub mod store;
pub mod worker;
pub mod ytdlp;

use crate::{app::AppState, application::library};
use std::{path::Path, sync::Arc};

/// Called by the filing step for every original it finishes with. When the
/// original was one of our downloads, its wanted song now points at the
/// library file, and playlists that contain it are rewritten.
pub async fn downloaded_file_done(
    state: &Arc<AppState>,
    original: &Path,
    library_path: &Path,
    already_owned: bool,
) {
    let staging = state.config.read().await.staging_dir.clone();
    if !original.starts_with(&staging) {
        return;
    }
    let status = if already_owned { "in_library" } else { "filed" };
    match store::finished_download(&state.pool, original, library_path, status).await {
        Ok(requests) if !requests.is_empty() => refresh_playlists(state, &requests).await,
        Ok(_) => {}
        Err(error) => tracing::warn!(%error, "could not record a filed download"),
    }
}

/// Rewrites the playlist files of these requests. Navidrome's file watcher
/// picks up the change.
pub async fn refresh_playlists(state: &Arc<AppState>, requests: &[i64]) {
    let config = state.config.read().await.clone();
    let library_root = Path::new(&config.output_dir);
    for request in requests {
        match store::playlist_for_request(&state.pool, *request).await {
            Ok(Some((name, songs))) => match library::write_playlist(library_root, &name, &songs)
                .await
            {
                Ok(_) => {}
                Err(error) => tracing::warn!(%error, playlist = %name, "could not write playlist"),
            },
            Ok(None) => {}
            Err(error) => tracing::warn!(%error, "could not read playlist songs"),
        }
    }
}
