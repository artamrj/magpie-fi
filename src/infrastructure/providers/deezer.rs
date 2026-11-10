use super::{ArtistCredit, ArtworkCandidate, Candidate, phrase, request_json};
use crate::infrastructure::provider_cache::{cached, search_key};
use anyhow::{Result, bail};
use chrono::Duration;
use reqwest::Client;
use serde_json::Value;
use sqlx::SqlitePool;

/// Searches Deezer. The filtered query (`artist:"…" track:"…"`) is precise but Deezer's
/// filtered index misses some tracks that its plain search knows, so an empty answer is
/// asked again as plain words.
pub async fn search(
    pool: &SqlitePool,
    client: &Client,
    title: &str,
    artist: Option<&str>,
) -> Result<Vec<Candidate>> {
    for query in queries(title, artist) {
        let candidates = parse_results(&fetch_search(pool, client, &query).await?);
        if !candidates.is_empty() {
            return Ok(candidates);
        }
    }
    Ok(Vec::new())
}

/// The filtered query first, then the plain words.
fn queries(title: &str, artist: Option<&str>) -> Vec<String> {
    let title = title.trim();
    match artist.map(str::trim).filter(|value| !value.is_empty()) {
        Some(artist) => vec![
            format!("artist:{} track:{}", phrase(artist), phrase(title)),
            format!("{artist} {title}"),
        ],
        None => vec![format!("track:{}", phrase(title)), title.to_owned()],
    }
}

async fn fetch_search(pool: &SqlitePool, client: &Client, query: &str) -> Result<Value> {
    cached(
        pool,
        "deezer",
        &search_key(query),
        Duration::days(7),
        || async {
            let value = request_json(
                client
                    .get("https://api.deezer.com/search")
                    .query(&[("q", query), ("limit", "10")]),
            )
            .await?;
            if let Some(message) = value["error"]["message"].as_str() {
                bail!("Deezer API error: {message}");
            }
            Ok(value)
        },
    )
    .await
}

/// Album covers found by the album's name and artist (`search/album`), the
/// filtered query first, then plain words. The caller checks that album and
/// artist match the chosen release.
pub async fn album_covers(
    pool: &SqlitePool,
    client: &Client,
    artist: &str,
    album: &str,
) -> Result<Vec<ArtworkCandidate>> {
    let (artist, album) = (artist.trim(), album.trim());
    for query in [
        format!("artist:{} album:{}", phrase(artist), phrase(album)),
        format!("{artist} {album}"),
    ] {
        let raw = cached(
            pool,
            "deezer",
            &search_key(&format!("album:{query}")),
            Duration::days(7),
            || async {
                let value = request_json(
                    client
                        .get("https://api.deezer.com/search/album")
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
        let covers = parse_album_covers(&raw);
        if !covers.is_empty() {
            return Ok(covers);
        }
    }
    Ok(Vec::new())
}

fn parse_album_covers(raw: &Value) -> Vec<ArtworkCandidate> {
    raw["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|album| {
            Some(ArtworkCandidate {
                provider: "Deezer (album)".into(),
                url: album["cover_xl"]
                    .as_str()
                    .or_else(|| album["cover_big"].as_str())?
                    .to_owned(),
                album: album["title"].as_str().map(str::to_owned),
                artist: album["artist"]["name"].as_str().map(str::to_owned),
                ..Default::default()
            })
        })
        .collect()
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
    fn album_covers_carry_their_album_and_artist() {
        let raw = serde_json::json!({"data": [
            {"title": "Koohe Yakh", "artist": {"name": "Ebi"}, "cover_xl": "https://e.test/xl.jpg"},
            {"title": "No cover", "artist": {"name": "Ebi"}}
        ]});
        let covers = parse_album_covers(&raw);
        assert_eq!(covers.len(), 1);
        assert_eq!(covers[0].url, "https://e.test/xl.jpg");
        assert_eq!(covers[0].album.as_deref(), Some("Koohe Yakh"));
        assert_eq!(covers[0].artist.as_deref(), Some("Ebi"));
    }

    #[test]
    fn a_filtered_search_is_retried_as_plain_words() {
        assert_eq!(
            queries("Donyaye Movazi", Some("Tqvis & Ferap")),
            vec![
                "artist:\"Tqvis & Ferap\" track:\"Donyaye Movazi\"",
                "Tqvis & Ferap Donyaye Movazi"
            ]
        );
        assert_eq!(
            queries("Donyaye Movazi", Some("  ")),
            vec!["track:\"Donyaye Movazi\"", "Donyaye Movazi"]
        );
    }

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
