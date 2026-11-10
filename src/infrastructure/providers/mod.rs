pub mod acoustid;
pub mod audd;
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
pub mod spotify;
pub mod wikidata;
pub mod youtube;

use anyhow::Result;
use reqwest::{Client, RequestBuilder};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

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

/// An OAuth client-credentials token (Spotify, SoundCloud), reused until
/// shortly before it expires.
#[derive(Default)]
pub struct ClientCredentials {
    token: Mutex<Option<(String, Instant)>>,
}

impl ClientCredentials {
    pub async fn token(
        &self,
        client: &Client,
        token_url: &str,
        client_id: &str,
        client_secret: &str,
    ) -> Result<String> {
        let mut cached = self.token.lock().await;
        if let Some((token, expires_at)) = cached.as_ref()
            && *expires_at > Instant::now() + Duration::from_secs(30)
        {
            return Ok(token.clone());
        }
        let raw = request_json(
            client
                .post(token_url)
                .basic_auth(client_id, Some(client_secret))
                .header("Accept", "application/json")
                .form(&[("grant_type", "client_credentials")]),
        )
        .await?;
        let token = raw["access_token"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("the token response had no access token"))?
            .to_owned();
        let lifetime = raw["expires_in"].as_u64().unwrap_or(3600).max(60);
        *cached = Some((
            token.clone(),
            Instant::now() + Duration::from_secs(lifetime),
        ));
        Ok(token)
    }
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
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

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

    #[tokio::test]
    async fn client_credentials_are_fetched_once_with_basic_auth() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let app = axum::Router::new().route(
            "/token",
            axum::routing::post(move |headers: axum::http::HeaderMap| async move {
                counter.fetch_add(1, Ordering::SeqCst);
                // "id:secret" in base64.
                assert_eq!(headers["authorization"], "Basic aWQ6c2VjcmV0");
                axum::Json(serde_json::json!({"access_token": "t1", "expires_in": 3600}))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/token", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let credentials = ClientCredentials::default();
        for _ in 0..2 {
            let token = credentials
                .token(&Client::new(), &url, "id", "secret")
                .await
                .unwrap();
            assert_eq!(token, "t1");
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
