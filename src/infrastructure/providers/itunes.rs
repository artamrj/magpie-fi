use super::Candidate;
use super::request_json;
use crate::infrastructure::provider_cache::{cached, search_key};
use anyhow::Result;
use chrono::Duration;
use reqwest::Client;
use serde_json::Value;
use sqlx::SqlitePool;

pub async fn search(
    pool: &SqlitePool,
    client: &Client,
    title: &str,
    artist: Option<&str>,
    album: Option<&str>,
) -> Result<Vec<Candidate>> {
    let base_term = artist
        .filter(|value| !value.trim().is_empty())
        .map(|artist| format!("{artist} {title}"))
        .unwrap_or_else(|| title.to_owned());
    let mut terms = vec![base_term.clone(), title.to_owned()];
    if let Some(short_artist) = artist.and_then(|value| value.split_once(" - ").map(|pair| pair.0))
    {
        terms.push(format!("{short_artist} {title}"));
    }
    if let Some(album) = album.filter(|value| !value.trim().is_empty()) {
        terms.push(format!("{base_term} {album}"));
        for segment in album
            .split('|')
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            terms.push(segment.to_owned());
            if let Some(short_artist) =
                artist.and_then(|value| value.split_once(" - ").map(|pair| pair.0))
            {
                terms.push(format!("{short_artist} {segment}"));
            }
        }
    }
    terms.sort();
    terms.dedup();
    terms.retain(|term| !term.trim().is_empty());
    let mut out = Vec::new();
    // The same song turns up for several search terms; keep it once.
    let mut seen = std::collections::HashSet::new();
    for term in terms {
        let raw = search_term(pool, client, &term).await?;
        out.extend(
            raw["results"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|value| seen.insert(value["trackId"].as_i64()))
                .filter_map(parse_result),
        );
    }
    Ok(out)
}

async fn search_term(pool: &SqlitePool, client: &Client, term: &str) -> Result<Value> {
    cached(
        pool,
        "itunes",
        &search_key(term),
        Duration::days(7),
        || async {
            wait_turn().await;
            request_json(client.get("https://itunes.apple.com/search").query(&[
                ("term", term),
                ("media", "music"),
                ("entity", "song"),
                ("limit", "10"),
            ]))
            .await
        },
    )
    .await
}

/// Apple allows about 20 searches a minute per address and answers 429 above
/// that; one request every 3 s (cached answers are free) stays under it.
async fn wait_turn() {
    static NEXT_REQUEST: std::sync::OnceLock<tokio::sync::Mutex<tokio::time::Instant>> =
        std::sync::OnceLock::new();
    let gate = NEXT_REQUEST.get_or_init(|| tokio::sync::Mutex::new(tokio::time::Instant::now()));
    let mut next = gate.lock().await;
    let now = tokio::time::Instant::now();
    if *next > now {
        tokio::time::sleep(*next - now).await;
    }
    *next = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
}

/// Apple's track and collection IDs stay in `raw_json`: the candidate's
/// recording and release IDs are MusicBrainz IDs.
fn parse_result(value: &Value) -> Option<Candidate> {
    let album = value["collectionName"].as_str().map(str::to_owned);
    let track_total = value["trackCount"].as_i64();
    let release_type = infer_release_type(album.as_deref(), track_total);
    Some(Candidate {
        provider: "itunes".into(),
        title: value["trackName"].as_str()?.to_owned(),
        artist: value["artistName"].as_str()?.to_owned(),
        album,
        album_artist: value["collectionArtistName"].as_str().map(str::to_owned),
        track_number: value["trackNumber"].as_i64(),
        track_total,
        disc_number: value["discNumber"].as_i64(),
        disc_total: value["discCount"].as_i64(),
        year: value["releaseDate"]
            .as_str()
            .and_then(|date| date.get(..4))
            .map(str::to_owned),
        release_date: value["releaseDate"]
            .as_str()
            .and_then(|date| date.get(..10))
            .map(str::to_owned),
        genre: value["primaryGenreName"].as_str().map(str::to_owned),
        cover_url: value["artworkUrl100"]
            .as_str()
            .map(|url| url.replace("100x100bb", "1200x1200bb")),
        duration_delta: value["trackTimeMillis"]
            .as_f64()
            .map(|value| value / 1000.0),
        release_type,
        raw_json: value.to_string(),
        ..Default::default()
    })
}

fn infer_release_type(album: Option<&str>, track_total: Option<i64>) -> Option<String> {
    let album = album?.trim().to_ascii_lowercase();
    if (album.ends_with(" - single") || album == "single") && track_total.is_none_or(|n| n == 1) {
        Some("single".into())
    } else if album.ends_with(" - ep") || album == "ep" {
        Some("ep".into())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_international_catalog_result() {
        let candidate = parse_result(&serde_json::json!({
            "trackId": 1,
            "collectionId": 2,
            "trackName": "Khaar",
            "artistName": "Amir Tataloo",
            "collectionName": "Khaar - Single",
            "trackTimeMillis": 427076,
            "releaseDate": "2019-06-23T12:00:00Z",
            "primaryGenreName": "Pop",
            "artworkUrl100": "https://example.test/100x100bb.jpg",
            "trackNumber": 1,
            "trackCount": 1
        }))
        .unwrap();
        assert_eq!(candidate.artist, "Amir Tataloo");
        assert_eq!(candidate.release_date.as_deref(), Some("2019-06-23"));
        assert_eq!(candidate.duration_delta, Some(427.076));
        assert_eq!(candidate.release_type.as_deref(), Some("single"));
        assert_eq!(
            candidate.cover_url.as_deref(),
            Some("https://example.test/1200x1200bb.jpg")
        );
        // Apple IDs are not MusicBrainz IDs.
        assert!(candidate.recording_id.is_none() && candidate.release_id.is_none());
        assert!(candidate.raw_json.contains("\"trackId\":1"));
    }

    #[test]
    fn does_not_infer_single_from_an_album_name_containing_the_word() {
        assert_eq!(infer_release_type(Some("Single Ladies"), Some(12)), None);
        assert_eq!(infer_release_type(Some("Single Collection"), Some(1)), None);
    }
}
