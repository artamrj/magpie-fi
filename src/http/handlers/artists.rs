//! The Artists page: every artist in the library and the owner's fixes.

use super::*;
use crate::application::{
    album_changes::{self, SplitAlbum},
    artist_changes::{self, ArtistSummary, Changed},
};

/// `GET /api/artists`
pub async fn list_artists(State(s): State<Arc<AppState>>) -> ApiResult<Json<Vec<ArtistSummary>>> {
    Ok(Json(artist_changes::list(&s.pool).await?))
}

/// `GET /api/artists/{id}/albums`: the artist's albums Navidrome shows more than once.
pub async fn split_albums(
    State(s): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Vec<SplitAlbum>>> {
    Ok(Json(album_changes::split_albums(&s, id).await?))
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ArtistChange {
    /// A better name for the tags and the folder.
    Rename { name: String },
    /// This artist is the same as `into`.
    Merge { into: i64 },
    /// This name means several artists.
    Split { parts: Vec<String> },
    /// One artist although the name has "&" or "," (a band or duo).
    OneArtist,
    /// Songs of one album that Navidrome shows twice become one album (`key` from
    /// `GET /api/artists/{id}/albums`).
    JoinAlbum { key: String },
}

/// `POST /api/artists/{id}`: rename, merge, split, mark as one artist, or make one album.
/// Songs are retagged and moved, so nothing else may run meanwhile.
pub async fn change_artist(
    State(s): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(change): Json<ArtistChange>,
) -> ApiResult<Json<Changed>> {
    let state = s.clone();
    let work = async move {
        let s = state;
        match change {
            ArtistChange::Rename { name } => artist_changes::rename(&s, id, &name).await,
            ArtistChange::Merge { into } => artist_changes::merge(&s, id, into).await,
            ArtistChange::Split { parts } => artist_changes::split(&s, id, &parts).await,
            ArtistChange::OneArtist => artist_changes::mark_one_artist(&s.pool, id)
                .await
                .map(|()| Changed::default()),
            ArtistChange::JoinAlbum { key } => album_changes::join(&s, id, &key).await,
        }
    };
    in_workflow(
        &s,
        WorkflowPhase::Apply,
        "Updating an artist in the library",
        work,
    )
    .await?
    .map(Json)
    .map_err(|error| ApiError::validation(format!("{error:#}")))
}
