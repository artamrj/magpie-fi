use super::{Candidate, nonempty, request_json};
use crate::infrastructure::provider_cache::{cached, search_key};
use anyhow::{Context, Result, bail};
use chrono::{Duration, NaiveDate};
use percent_encoding::percent_decode_str;
use reqwest::Client;
use serde_json::Value;
use sqlx::SqlitePool;
use std::collections::HashSet;

const PROVIDER: &str = "radiojavan";
const API_BASE: &str = "https://rj-deskcloud.com/api2";
const RJ_USER_AGENT: &str = "Radio Javan/4.0.2 magpie-fi/0.1";

pub async fn search(
    pool: &SqlitePool,
    client: &Client,
    title: &str,
    artist: Option<&str>,
) -> Result<Vec<Candidate>> {
    let query = match artist.filter(|value| !value.trim().is_empty()) {
        Some(artist) => format!("{} {}", artist.trim(), title.trim()),
        None => title.trim().to_owned(),
    };
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let raw = cached(
        pool,
        PROVIDER,
        &search_key(&query),
        Duration::days(7),
        || api(client, "search", ("query", &query)),
    )
    .await?;
    Ok(parse_search_results(&raw))
}

/// A pasted Radio Javan song or podcast link as a match.
pub async fn lookup_url(pool: &SqlitePool, client: &Client, url: &str) -> Result<Candidate> {
    let mut candidate = match player_slug(url)? {
        (LinkKind::Song, slug) => {
            let key = format!("song:{}", slug.to_ascii_lowercase());
            let raw = cached(pool, PROVIDER, &key, Duration::days(14), || {
                api(client, "mp3", ("id", &slug))
            })
            .await?;
            let track = raw
                .get("mp3")
                .filter(|value| value.is_object())
                .or_else(|| raw.get("data").filter(|value| value.is_object()))
                .unwrap_or(&raw);
            candidate_from_track(track)
                .ok_or_else(|| anyhow::anyhow!("Radio Javan returned no usable song metadata"))?
        }
        (LinkKind::Podcast, _) => podcast(pool, client, url).await?.candidate,
    };
    candidate.score = 96.0;
    candidate.score_breakdown = Some(
        serde_json::json!({
            "source": "radiojavan_user_source_url",
            "sources": ["Radio Javan"],
            "user_verified_source": true,
            "final_score": 96.0
        })
        .to_string(),
    );
    Ok(candidate)
}

/// A Radio Javan podcast episode: its details as a match, and its audio.
#[derive(Clone, Debug)]
pub struct Podcast {
    pub candidate: Candidate,
    /// Radio Javan's own MP3 of the episode (free and public, no DRM).
    pub audio_url: Option<String>,
    /// Only Radio Javan Premium members may play it: magpie-fi doesn't download those.
    pub premium_only: bool,
}

/// Reads a podcast link (`play.radiojavan.com/podcast/dor-dor-8`).
pub async fn podcast(pool: &SqlitePool, client: &Client, url: &str) -> Result<Podcast> {
    let (LinkKind::Podcast, slug) = player_slug(url)? else {
        bail!("Radio Javan URL must point to a podcast");
    };
    let key = format!("podcast:{}", slug.to_ascii_lowercase());
    let raw = cached(pool, PROVIDER, &key, Duration::days(14), || {
        api(client, "podcast", ("id", &slug))
    })
    .await?;
    let episode = raw
        .get("podcast")
        .filter(|value| value.is_object())
        .unwrap_or(&raw);
    let candidate = candidate_from_podcast(episode)
        .ok_or_else(|| anyhow::anyhow!("Radio Javan returned no usable podcast details"))?;
    Ok(Podcast {
        candidate,
        audio_url: episode["link"]
            .as_str()
            .map(str::trim)
            .filter(|link| link.starts_with("https://"))
            .map(str::to_owned),
        premium_only: episode["premium_only"].as_bool().unwrap_or(false),
    })
}

/// One call to Radio Javan's desktop API.
async fn api(client: &Client, endpoint: &str, parameter: (&str, &str)) -> Result<Value> {
    let value = request_json(
        client
            .get(format!("{API_BASE}/{endpoint}"))
            .header("Accept", "application/json")
            .header("x-rj-user-agent", RJ_USER_AGENT)
            .query(&[parameter]),
    )
    .await?;
    reject_api_error(&value)?;
    Ok(value)
}

fn reject_api_error(raw: &Value) -> Result<()> {
    if let Some(message) = raw["error"]
        .as_str()
        .or_else(|| raw["error"]["message"].as_str())
        .or_else(|| {
            raw["message"]
                .as_str()
                .filter(|_| raw["status"].as_str() == Some("error"))
        })
    {
        bail!("Radio Javan API error: {message}");
    }
    Ok(())
}

/// What a Radio Javan player link points at.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LinkKind {
    Song,
    Podcast,
}

/// The kind and slug of a `play.radiojavan.com/song/…` or `/podcast/…` link.
pub fn player_slug(url: &str) -> Result<(LinkKind, String)> {
    let parsed = reqwest::Url::parse(url).context("invalid Radio Javan URL")?;
    if parsed.scheme() != "https"
        || !matches!(
            parsed.host_str(),
            Some("play.radiojavan.com" | "www.play.radiojavan.com")
        )
    {
        bail!("only HTTPS Radio Javan player links are supported");
    }
    let path = parsed.path();
    let (kind, encoded) = if let Some(rest) = path.strip_prefix("/song/") {
        (LinkKind::Song, rest)
    } else if let Some(rest) = path.strip_prefix("/podcast/") {
        (LinkKind::Podcast, rest)
    } else {
        bail!("Radio Javan URL must point to a song or a podcast");
    };
    let encoded = encoded.trim_end_matches('/');
    if encoded.is_empty() || encoded.contains('/') {
        bail!("Radio Javan URL must point to a song or a podcast");
    }
    let slug = percent_decode_str(encoded)
        .decode_utf8()
        .context("Radio Javan URL is not valid UTF-8")?
        .into_owned();
    Ok((kind, slug))
}

#[cfg(test)]
fn song_slug(url: &str) -> Result<String> {
    match player_slug(url)? {
        (LinkKind::Song, slug) => Ok(slug),
        (LinkKind::Podcast, _) => bail!("Radio Javan URL must point to a song"),
    }
}

fn parse_search_results(raw: &Value) -> Vec<Candidate> {
    let mut tracks = raw["mp3s"]
        .as_array()
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    if raw["top"]["type"].as_str() == Some("mp3") {
        tracks.push(&raw["top"]);
    } else if raw["top"]["mp3"].is_object() {
        tracks.push(&raw["top"]["mp3"]);
    }
    // A file of a DJ mix is found as the podcast episode it came from.
    let podcasts = raw["podcasts"]
        .as_array()
        .into_iter()
        .flatten()
        .take(5)
        .filter_map(candidate_from_podcast);
    let mut seen = HashSet::new();
    tracks
        .into_iter()
        .filter_map(candidate_from_track)
        .chain(podcasts)
        .filter(|candidate| {
            let key = format!(
                "{}|{}",
                candidate.title.to_ascii_lowercase(),
                candidate.artist.to_ascii_lowercase()
            );
            seen.insert(key)
        })
        .take(10)
        .collect()
}

fn candidate_from_track(track: &Value) -> Option<Candidate> {
    let title = nonempty(track["song"].as_str())?;
    let artist = nonempty(track["artist"].as_str())?;
    let release_date = parse_release_date(track);
    let album = track["album"]
        .as_str()
        .or_else(|| track["album"]["album"].as_str())
        .or_else(|| track["album"]["title"].as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    Some(Candidate {
        provider: PROVIDER.into(),
        title: title.to_owned(),
        artist: artist.to_owned(),
        album,
        year: release_date
            .as_deref()
            .and_then(|date| date.get(..4))
            .map(str::to_owned),
        release_date,
        cover_url: track["photo"]
            .as_str()
            .or_else(|| track["thumbnail"].as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned),
        duration_delta: track["duration"].as_f64(),
        raw_json: track.to_string(),
        ..Default::default()
    })
}

/// A podcast episode is filed as one track of its show: "Dor Dor 8 (Old School)" by Deejay Al
/// is track 8 of "Dor Dor". A DJ mix is a broadcast with the "dj-mix" secondary type, as
/// MusicBrainz files them; a talk show is a plain broadcast.
fn candidate_from_podcast(episode: &Value) -> Option<Candidate> {
    let title = nonempty(episode["title"].as_str())?;
    let artist = nonempty(episode["podcast_artist"].as_str())?;
    let show_permlink = episode["show_permlink"].as_str().map(str::trim);
    let show = show_permlink
        .map(|permlink| permlink.replace('-', " "))
        .filter(|show| !show.trim().is_empty());
    let episode_number = show_permlink
        .zip(episode["permlink"].as_str())
        .and_then(|(show, permlink)| {
            let rest = permlink.get(show.len()..)?;
            permlink
                .get(..show.len())
                .is_some_and(|start| start.eq_ignore_ascii_case(show))
                .then_some(rest)
        })
        .and_then(|rest| rest.strip_prefix('-'))
        .and_then(|number| number.parse::<i64>().ok());
    let release_date = parse_release_date(episode);
    let talk = episode["talk"].as_bool().unwrap_or(false);
    Some(Candidate {
        provider: PROVIDER.into(),
        title: title.to_owned(),
        artist: artist.to_owned(),
        album_artist: show.as_ref().map(|_| artist.to_owned()),
        album: show,
        track_number: episode_number,
        year: release_date
            .as_deref()
            .and_then(|date| date.get(..4))
            .map(str::to_owned),
        release_date,
        release_type: Some("broadcast".into()),
        release_secondary_types: (!talk).then(|| "dj-mix".into()),
        cover_url: episode["photo"]
            .as_str()
            .or_else(|| episode["thumbnail"].as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned),
        duration_delta: episode["duration"].as_f64(),
        raw_json: episode.to_string(),
        ..Default::default()
    })
}

fn parse_release_date(track: &Value) -> Option<String> {
    if let Some(created) = track["created_at"].as_str()
        && created.len() >= 10
        && NaiveDate::parse_from_str(&created[..10], "%Y-%m-%d").is_ok()
    {
        return Some(created[..10].to_owned());
    }
    track["date"]
        .as_str()
        .and_then(|date| NaiveDate::parse_from_str(date, "%b %d, %Y").ok())
        .map(|date| date.format("%Y-%m-%d").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example_track() -> Value {
        serde_json::json!({
            "id": 100168,
            "artist": "DJ Sasha",
            "song": "Tehraan Kenaret (Remix)",
            "photo": "https://assets.rjassets.com/static/mp3/dj-sasha-tehraan-kenaret-(remix)/74f18a0fed173b7.jpg",
            "duration": 183.798,
            "created_at": "2021-08-31T05:40:24-04:00",
            "album": null,
            "permlink": "DJ-Sasha-Tehraan-Kenaret-(Remix)"
        })
    }

    #[test]
    fn parses_search_metadata_and_original_artwork() {
        let candidates = parse_search_results(&serde_json::json!({"mp3s": [example_track()]}));
        assert_eq!(candidates.len(), 1);
        let candidate = &candidates[0];
        assert_eq!(candidate.provider, "radiojavan");
        assert_eq!(candidate.title, "Tehraan Kenaret (Remix)");
        assert_eq!(candidate.artist, "DJ Sasha");
        assert_eq!(candidate.year.as_deref(), Some("2021"));
        assert_eq!(candidate.release_date.as_deref(), Some("2021-08-31"));
        assert_eq!(candidate.duration_delta, Some(183.798));
        assert!(candidate.album.is_none());
        assert!(
            candidate
                .cover_url
                .as_deref()
                .is_some_and(|url| url.starts_with("https://assets.rjassets.com/"))
        );
    }

    #[test]
    fn extracts_literal_and_encoded_song_slugs() {
        assert_eq!(
            song_slug("https://play.radiojavan.com/song/dj-sasha-tehraan-kenaret-(remix)").unwrap(),
            "dj-sasha-tehraan-kenaret-(remix)"
        );
        assert_eq!(
            song_slug("https://play.radiojavan.com/song/dj-sasha-tehraan-kenaret-%28remix%29")
                .unwrap(),
            "dj-sasha-tehraan-kenaret-(remix)"
        );
    }

    fn example_podcast() -> Value {
        serde_json::json!({
            "id": 2979,
            "title": "Dor Dor 8 (Old School)",
            "podcast_artist": "Deejay Al",
            "show_permlink": "Dor-Dor",
            "permlink": "Dor-Dor-8",
            "duration": 3584.21,
            "created_at": "2020-08-17T00:00:00-04:00",
            "date": "Deejay Al",
            "talk": false,
            "premium_only": false,
            "photo": "https://assets.rjassets.com/static/podcasts/dor-dor-8/7a6914eb094c891.jpg",
            "link": "https://host2.rjmedia-content.app/media/podcast/mp3-192/2979-cedb7bb84319d95.mp3"
        })
    }

    #[test]
    fn a_podcast_episode_is_a_track_of_its_show() {
        let candidates = parse_search_results(&serde_json::json!({
            "mp3s": [],
            "podcasts": [example_podcast()],
            "top": []
        }));
        assert_eq!(candidates.len(), 1);
        let episode = &candidates[0];
        assert_eq!(episode.title, "Dor Dor 8 (Old School)");
        assert_eq!(episode.artist, "Deejay Al");
        assert_eq!(episode.album.as_deref(), Some("Dor Dor"));
        assert_eq!(episode.album_artist.as_deref(), Some("Deejay Al"));
        assert_eq!(episode.track_number, Some(8));
        assert_eq!(episode.release_date.as_deref(), Some("2020-08-17"));
        assert_eq!(episode.release_type.as_deref(), Some("broadcast"));
        assert_eq!(episode.release_secondary_types.as_deref(), Some("dj-mix"));
        assert_eq!(episode.duration_delta, Some(3584.21));
    }

    #[test]
    fn reads_song_and_podcast_links() {
        assert_eq!(
            player_slug("https://play.radiojavan.com/podcast/dor-dor-8").unwrap(),
            (LinkKind::Podcast, "dor-dor-8".to_owned())
        );
        assert_eq!(
            player_slug("https://play.radiojavan.com/song/dj-sasha-x/").unwrap(),
            (LinkKind::Song, "dj-sasha-x".to_owned())
        );
        assert!(player_slug("https://play.radiojavan.com/podcast/").is_err());
        assert!(song_slug("https://play.radiojavan.com/podcast/dor-dor-8").is_err());
    }

    #[tokio::test]
    #[ignore = "live Radio Javan API probe"]
    async fn live_podcast_probe() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("radiojavan-live.sqlite");
        let pool = crate::infrastructure::db::connect(database.to_str().unwrap())
            .await
            .unwrap();
        let client = Client::new();
        let url = "https://play.radiojavan.com/podcast/dor-dor-8";
        let episode = podcast(&pool, &client, url).await.unwrap();
        assert_eq!(episode.candidate.title, "Dor Dor 8 (Old School)");
        assert_eq!(episode.candidate.artist, "Deejay Al");
        assert_eq!(episode.candidate.album.as_deref(), Some("Dor Dor"));
        assert_eq!(episode.candidate.track_number, Some(8));
        assert!(!episode.premium_only);
        assert!(episode.audio_url.is_some_and(|url| url.ends_with(".mp3")));
        // A file tagged like the episode finds it by search too.
        let found = search(&pool, &client, "Dor Dor 8 (Old School)", Some("Deejay Al"))
            .await
            .unwrap();
        assert!(
            found
                .iter()
                .any(|candidate| candidate.title == "Dor Dor 8 (Old School)"
                    && candidate.album.as_deref() == Some("Dor Dor"))
        );
    }

    #[test]
    fn rejects_non_song_and_non_radiojavan_urls() {
        assert!(song_slug("https://play.radiojavan.com/artist/dj-sasha").is_err());
        assert!(song_slug("https://example.com/song/dj-sasha").is_err());
    }
}
