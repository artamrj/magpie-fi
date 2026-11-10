use super::{ArtistCredit, Candidate, phrase, request_json};
use crate::infrastructure::provider_cache::{cached, search_key};
use anyhow::{Result, bail};
use chrono::Duration;
use reqwest::Client;
use serde_json::Value;
use sqlx::SqlitePool;

pub async fn search(
    pool: &SqlitePool,
    client: &Client,
    title: &str,
    artist: Option<&str>,
) -> Result<Vec<Candidate>> {
    let query = match artist.filter(|value| !value.trim().is_empty()) {
        Some(artist) => format!("artist:{} track:{}", phrase(artist), phrase(title)),
        None => format!("track:{}", phrase(title)),
    };
    let raw = cached(
        pool,
        "deezer",
        &search_key(&query),
        Duration::days(7),
        || async {
            let value = request_json(
                client
                    .get("https://api.deezer.com/search")
                    .query(&[("q", query.as_str()), ("limit", "10")]),
            )
            .await?;
            if let Some(message) = value["error"]["message"].as_str() {
                bail!("Deezer API error: {message}");
            }
            Ok(value)
        },
    )
    .await?;
    Ok(parse_results(&raw))
}

pub async fn lookup_isrc(
    pool: &SqlitePool,
    client: &Client,
    isrc: &str,
) -> Result<Option<Candidate>> {
    let isrc = isrc.trim().to_ascii_uppercase();
    let key = search_key(&format!("isrc:{isrc}"));
    let raw = cached(pool, "deezer", &key, Duration::days(7), || {
        request_json(client.get(format!("https://api.deezer.com/track/isrc:{isrc}")))
    })
    .await?;
    if raw["error"].is_object() {
        return Ok(None);
    }
    Ok(parse_results(&serde_json::json!({"data":[raw]}))
        .into_iter()
        .next())
}

fn parse_results(raw: &Value) -> Vec<Candidate> {
    raw["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|track| {
            let mut artist_credits = track["contributors"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|artist| artist["name"].as_str())
                .map(|name| ArtistCredit::new(name, ""))
                .collect::<Vec<_>>();
            if artist_credits.is_empty() {
                artist_credits.push(ArtistCredit::new(track["artist"]["name"].as_str()?, ""));
            }
            if artist_credits.len() > 1 {
                let last = artist_credits.len() - 1;
                for credit in &mut artist_credits[..last] {
                    credit.join_phrase = " & ".into();
                }
            }
            Some(Candidate {
                provider: "deezer".into(),
                title: track["title"].as_str()?.to_owned(),
                artist: crate::domain::credits::display_artist(&artist_credits),
                artist_credits,
                album: track["album"]["title"].as_str().map(str::to_owned),
                isrc: track["isrc"].as_str().map(str::to_owned),
                cover_url: track["album"]["cover_xl"]
                    .as_str()
                    .or_else(|| track["album"]["cover_big"].as_str())
                    .map(str::to_owned),
                duration_delta: track["duration"].as_f64(),
                // Deezer IDs remain in raw_json. Candidate recording/release IDs
                // are MusicBrainz-specific and must not receive foreign IDs.
                raw_json: track.to_string(),
                ..Default::default()
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_track_without_mislabeling_deezer_ids_as_musicbrainz_ids() {
        let candidates = parse_results(&serde_json::json!({"data": [{
            "id": 8011857,
            "title": "Count on Me",
            "isrc": "USEE11000168",
            "duration": 197,
            "artist": {"id": 429675, "name": "Bruno Mars"},
            "album": {
                "id": 739505,
                "title": "Doo-Wops & Hooligans",
                "cover_xl": "https://example.test/cover.jpg"
            }
        }]}));
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].provider, "deezer");
        assert_eq!(candidates[0].artist, "Bruno Mars");
        assert_eq!(candidates[0].isrc.as_deref(), Some("USEE11000168"));
        assert_eq!(candidates[0].duration_delta, Some(197.0));
        assert!(candidates[0].recording_id.is_none());
        assert!(candidates[0].release_id.is_none());
    }
}
