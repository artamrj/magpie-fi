pub mod acoustid;
pub mod audiomack;
pub mod cover_art_archive;
pub mod deezer;
pub mod discogs;
pub mod genius;
pub mod itunes;
pub mod lastfm;
pub mod musicbrainz;
pub mod navahang;
pub mod radiojavan;
pub mod shazam;
pub mod songrec;
pub mod soundcloud;
pub mod wikidata;
pub mod youtube;

use anyhow::Result;
use reqwest::{Client, RequestBuilder};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use crate::domain::credits::ArtistCredit;

/// For sites that serve their public pages and APIs only to browsers.
pub const BROWSER_USER_AGENT: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 Chrome/138 Safari/537.36";

/// Trimmed text, or `None` when there is none.
pub fn nonempty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

/// `value[key]` as trimmed, non-empty text.
pub fn text(value: &Value, key: &str) -> Option<String> {
    nonempty(value[key].as_str()).map(str::to_owned)
}

/// A search phrase for Spotify and Deezer queries (`track:"…"`). Their query
/// syntax has no escape character, so quotes inside the term are dropped.
pub fn phrase(term: &str) -> String {
    format!("\"{}\"", term.replace('"', " ").trim())
}

/// Sends the request and reads the JSON answer. Errors leave out the URL,
/// which can carry an API key.
pub async fn request_json(request: RequestBuilder) -> Result<Value> {
    Ok(request
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(reqwest::Error::without_url)?
        .json()
        .await
        .map_err(reqwest::Error::without_url)?)
}

/// A public web page, fetched like a browser does.
pub async fn browser_page(client: &Client, url: &str) -> Result<String> {
    Ok(client
        .get(url)
        .header("Accept", "text/html,application/xhtml+xml")
        .header("User-Agent", BROWSER_USER_AGENT)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)?
        .text()
        .await?)
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "TEXT", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum ArtworkStatus {
    #[default]
    Searching,
    Verified,
    RetryableError,
    CoverRequired,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ArtworkCandidate {
    pub provider: String,
    pub url: String,
    pub user_confirmed: bool,
    /// The cover came with the exact catalog record this candidate is, so it
    /// belongs to it by definition (no album or artist comparison needed).
    #[serde(default)]
    pub own_record: bool,
    /// The cover came from a catalog row of this same recording: the title,
    /// artist and version agree. Its album may be another one (the album the
    /// song is on, for a song filed as its own single).
    #[serde(default)]
    pub same_recording: bool,
    pub release_id: Option<String>,
    pub isrc: Option<String>,
    pub album: Option<String>,
    pub artist: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Candidate {
    pub id: Option<i64>,
    pub provider: String,
    pub title: String,
    pub artist: String,
    #[serde(default)]
    pub artist_credits: Vec<ArtistCredit>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    #[serde(default)]
    pub album_artist_credits: Vec<ArtistCredit>,
    pub track_number: Option<i64>,
    pub track_total: Option<i64>,
    pub disc_number: Option<i64>,
    pub disc_total: Option<i64>,
    pub year: Option<String>,
    pub genre: Option<String>,
    pub composer: Option<String>,
    pub label: Option<String>,
    pub isrc: Option<String>,
    pub cover_url: Option<String>,
    #[serde(default)]
    pub artwork_candidates: Vec<ArtworkCandidate>,
    #[serde(default)]
    pub artwork_status: ArtworkStatus,
    pub artwork_message: Option<String>,
    pub recording_id: Option<String>,
    pub release_id: Option<String>,
    /// MusicBrainz release group: every edition of one album.
    pub release_group_id: Option<String>,
    /// MusicBrainz track on the release: Navidrome's stable track identity.
    pub release_track_id: Option<String>,
    /// First release of the recording (YYYY, YYYY-MM or YYYY-MM-DD).
    pub original_date: Option<String>,
    /// Latin sort names for artists whose display name is Persian.
    pub artist_sort: Option<String>,
    pub album_artist_sort: Option<String>,
    pub release_country: Option<String>,
    pub release_date: Option<String>,
    pub release_type: Option<String>,
    pub release_secondary_types: Option<String>,
    pub is_compilation: bool,
    pub duration_delta: Option<f64>,
    pub score_breakdown: Option<String>,
    pub artist_id: Option<String>,
    pub album_artist_id: Option<String>,
    pub score: f64,
    #[serde(skip_serializing)]
    pub raw_json: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phrases_cannot_be_broken_by_quotes() {
        assert_eq!(phrase("Say \"Hi\""), "\"Say  Hi\"");
        assert_eq!(phrase("Plain"), "\"Plain\"");
    }

    #[tokio::test]
    async fn request_errors_do_not_show_api_keys() {
        let error = request_json(Client::new().get("http://127.0.0.1:1/?api_key=SECRET-KEY"))
            .await
            .unwrap_err();
        assert!(!format!("{error:#} {error:?}").contains("SECRET-KEY"));
    }
}
