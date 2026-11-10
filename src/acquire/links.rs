//! Turns a music link into the list of songs it stands for. Only public pages
//! and documented public APIs are read; no audio comes from these services
//! (it is downloaded from YouTube by [`super::worker`]).
//!
//! | Service | Song | Album | Playlist |
//! |---|---|---|---|
//! | Spotify | embed page | embed page | embed page (as many songs as it shows) |
//! | Apple Music | iTunes Lookup API | iTunes Lookup API | public playlist page |
//! | Deezer | public API | public API | public API |
//! | YouTube / YouTube Music | yt-dlp | — | yt-dlp |
//! | Shazam | song page | — | — |

use super::ytdlp;
use crate::{
    config::Config,
    infrastructure::providers::{self, text},
};
use anyhow::{Context, Result, bail};
use serde_json::Value;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Service {
    Spotify,
    Apple,
    Deezer,
    YouTube,
    Shazam,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Scope {
    Track,
    Album,
    Playlist,
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Track => "track",
            Self::Album => "album",
            Self::Playlist => "playlist",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Link {
    pub service: Service,
    pub scope: Scope,
    pub id: String,
    pub url: String,
    /// Apple storefront ("us"), used for iTunes lookups.
    pub country: Option<String>,
}

/// One song a link asks for.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Wanted {
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub track_number: Option<i64>,
    pub duration: Option<f64>,
    pub isrc: Option<String>,
    pub source_url: Option<String>,
    pub youtube_id: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Collection {
    pub title: Option<String>,
    pub owner: Option<String>,
    pub songs: Vec<Wanted>,
}

/// Recognises a link. Short links (spotify.link, link.deezer.com) must be
/// expanded with [`expand_short_link`] first.
pub fn classify(raw: &str) -> Option<Link> {
    let url = reqwest::Url::parse(raw.trim()).ok()?;
    let host = url
        .host_str()?
        .trim_start_matches("www.")
        .to_ascii_lowercase();
    let parts: Vec<&str> = url
        .path_segments()
        .map(|segments| segments.filter(|part| !part.is_empty()).collect())
        .unwrap_or_default();
    let query = |name: &str| {
        url.query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
    };
    let link = |service, scope, id: &str, country: Option<String>| {
        Some(Link {
            service,
            scope,
            id: id.to_owned(),
            url: raw.trim().to_owned(),
            country,
        })
    };
    match host.as_str() {
        "open.spotify.com" => {
            // Skip a leading "intl-xx" segment.
            let parts: Vec<&str> = parts
                .into_iter()
                .skip_while(|part| part.starts_with("intl-"))
                .collect();
            let scope = match *parts.first()? {
                "track" => Scope::Track,
                "album" => Scope::Album,
                "playlist" => Scope::Playlist,
                _ => return None,
            };
            link(Service::Spotify, scope, parts.get(1)?, None)
        }
        "music.apple.com" | "itunes.apple.com" => {
            let country = parts
                .first()
                .filter(|part| part.len() == 2)
                .map(|part| part.to_string());
            let kind_index = parts
                .iter()
                .position(|part| matches!(*part, "album" | "song" | "playlist"))?;
            let id = parts.last()?.trim_start_matches("id");
            match parts[kind_index] {
                "song" => link(Service::Apple, Scope::Track, id, country),
                "album" => match query("i") {
                    Some(track) => link(Service::Apple, Scope::Track, &track, country),
                    None => link(Service::Apple, Scope::Album, id, country),
                },
                _ => link(Service::Apple, Scope::Playlist, parts.last()?, country),
            }
        }
        "deezer.com" => {
            let index = parts
                .iter()
                .position(|part| matches!(*part, "track" | "album" | "playlist"))?;
            let scope = match parts[index] {
                "track" => Scope::Track,
                "album" => Scope::Album,
                _ => Scope::Playlist,
            };
            link(Service::Deezer, scope, parts.get(index + 1)?, None)
        }
        "youtube.com" | "m.youtube.com" | "music.youtube.com" => {
            if let Some(video) = query("v") {
                link(Service::YouTube, Scope::Track, &video, None)
            } else if let Some(list) = query("list") {
                link(Service::YouTube, Scope::Playlist, &list, None)
            } else if parts.first() == Some(&"shorts") {
                link(Service::YouTube, Scope::Track, parts.get(1)?, None)
            } else {
                None
            }
        }
        "youtu.be" => link(Service::YouTube, Scope::Track, parts.first()?, None),
        // shazam.com/song/1845811123/donyaye-movazi (also /en-us/song/…): the
        // number is the song's Apple Music ID.
        _ if host.ends_with("shazam.com") => {
            let song = parts
                .iter()
                .position(|part| *part == "song" || *part == "track")?;
            let id = parts
                .get(song + 1)
                .filter(|id| id.chars().all(|character| character.is_ascii_digit()))?;
            link(Service::Shazam, Scope::Track, id, None)
        }
        _ => None,
    }
}

/// Follows a short link (spotify.link, link.deezer.com, deezer.page.link) to
/// the real one. Other links are returned unchanged.
pub async fn expand_short_link(client: &reqwest::Client, raw: &str) -> Result<String> {
    let host = reqwest::Url::parse(raw.trim())
        .ok()
        .and_then(|url| url.host_str().map(str::to_ascii_lowercase))
        .unwrap_or_default();
    if !matches!(
        host.as_str(),
        "spotify.link" | "link.deezer.com" | "deezer.page.link" | "spoti.fi"
    ) {
        return Ok(raw.trim().to_owned());
    }
    let response = client.get(raw.trim()).send().await?;
    Ok(response.url().to_string())
}

/// Reads every song of a link (one song for a track link).
pub async fn read(
    client: &reqwest::Client,
    pool: &sqlx::SqlitePool,
    config: &Config,
    link: &Link,
) -> Result<Collection> {
    let mut collection = match link.service {
        Service::Spotify => spotify(client, link).await?,
        Service::Apple => apple(client, link).await?,
        Service::Deezer => deezer(client, link).await?,
        Service::YouTube => youtube(config, link).await?,
        Service::Shazam => shazam(client, pool, link).await?,
    };
    collection
        .songs
        .retain(|song| !song.title.trim().is_empty());
    if collection.songs.is_empty() {
        bail!("this link has no songs that magpie-fi can read");
    }
    collection.songs.truncate(config.max_collection_tracks);
    Ok(collection)
}

// ── Spotify: the public embed page ──────────────────────────────────────────

async fn spotify(client: &reqwest::Client, link: &Link) -> Result<Collection> {
    let html = client
        .get(format!(
            "https://open.spotify.com/embed/{}/{}",
            link.scope.as_str(),
            link.id
        ))
        .header("Accept-Language", "en")
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    parse_spotify_embed(&html, link)
}

fn parse_spotify_embed(html: &str, link: &Link) -> Result<Collection> {
    let data = script_json(html, "__NEXT_DATA__").context("Spotify page has no song data")?;
    let entity = &data["props"]["pageProps"]["state"]["data"]["entity"];
    if link.scope == Scope::Track {
        let artists: Vec<String> = entity["artists"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|artist| artist["name"].as_str().map(str::to_owned))
            .collect();
        return Ok(Collection {
            songs: vec![Wanted {
                title: text(entity, "name")
                    .or_else(|| text(entity, "title"))
                    .unwrap_or_default(),
                artist: (!artists.is_empty()).then(|| artists.join(", ")),
                duration: milliseconds(&entity["duration"]),
                source_url: Some(format!("https://open.spotify.com/track/{}", link.id)),
                ..Wanted::default()
            }],
            ..Collection::default()
        });
    }
    let title = text(entity, "name").or_else(|| text(entity, "title"));
    let subtitle = text(entity, "subtitle");
    let songs = entity["trackList"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(index, track)| Wanted {
            title: text(track, "title").unwrap_or_default(),
            artist: text(track, "subtitle").map(|artists| artists.replace('\u{a0}', " ")),
            album: (link.scope == Scope::Album)
                .then(|| title.clone())
                .flatten(),
            track_number: (link.scope == Scope::Album).then_some(index as i64 + 1),
            duration: milliseconds(&track["duration"]),
            source_url: track["uri"]
                .as_str()
                .and_then(|uri| uri.strip_prefix("spotify:track:"))
                .map(|id| format!("https://open.spotify.com/track/{id}")),
            ..Wanted::default()
        })
        .collect();
    Ok(Collection {
        title,
        owner: subtitle,
        songs,
    })
}

// ── Apple Music: iTunes Lookup API and the public playlist page ─────────────

async fn apple(client: &reqwest::Client, link: &Link) -> Result<Collection> {
    let country = link.country.clone().unwrap_or_else(|| "us".into());
    if link.scope == Scope::Playlist {
        let html = client
            .get(&link.url)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        return parse_apple_playlist(&html, &link.url);
    }
    let value: Value = client
        .get("https://itunes.apple.com/lookup")
        .query(&[
            ("id", link.id.as_str()),
            ("entity", "song"),
            ("limit", "200"),
            ("country", country.as_str()),
        ])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(parse_itunes_lookup(&value, link.scope))
}

fn parse_itunes_lookup(value: &Value, scope: Scope) -> Collection {
    let results = value["results"].as_array().cloned().unwrap_or_default();
    let collection = results
        .iter()
        .find(|item| item["wrapperType"] == "collection");
    let songs = results
        .iter()
        .filter(|item| item["wrapperType"] == "track" && item["kind"] == "song")
        .map(|item| Wanted {
            title: text(item, "trackName").unwrap_or_default(),
            artist: text(item, "artistName"),
            album: text(item, "collectionName"),
            track_number: item["trackNumber"].as_i64(),
            duration: milliseconds(&item["trackTimeMillis"]),
            source_url: text(item, "trackViewUrl"),
            ..Wanted::default()
        })
        .collect();
    Collection {
        title: (scope == Scope::Album)
            .then(|| collection.and_then(|item| text(item, "collectionName")))
            .flatten(),
        owner: collection.and_then(|item| text(item, "artistName")),
        songs,
    }
}

fn parse_apple_playlist(html: &str, url: &str) -> Result<Collection> {
    let data =
        script_json(html, "serialized-server-data").context("Apple Music page has no song data")?;
    let mut songs = Vec::new();
    collect_apple_tracks(&data, &mut songs);
    for song in &mut songs {
        song.source_url.get_or_insert_with(|| url.to_owned());
    }
    // <title>‎Today’s Hits - Playlist - Apple Music</title>
    let title = between(html, "<title>", "</title>")
        .map(|title| {
            title
                .trim_start_matches('\u{200e}')
                .trim_end_matches(" - Apple Music")
                .trim_end_matches(" - Playlist")
                .trim()
                .to_owned()
        })
        .filter(|title| !title.is_empty());
    Ok(Collection {
        title,
        owner: None,
        songs,
    })
}

fn collect_apple_tracks(value: &Value, songs: &mut Vec<Wanted>) {
    match value {
        Value::Object(map) => {
            if map.contains_key("artistName")
                && map.contains_key("title")
                && map.contains_key("duration")
            {
                songs.push(Wanted {
                    title: text(value, "title").unwrap_or_default(),
                    artist: text(value, "artistName"),
                    album: value["tertiaryLinks"][0]["title"]
                        .as_str()
                        .map(str::to_owned),
                    duration: milliseconds(&value["duration"]),
                    source_url: value["contentDescriptor"]["url"]
                        .as_str()
                        .map(str::to_owned),
                    ..Wanted::default()
                });
                return;
            }
            for child in map.values() {
                collect_apple_tracks(child, songs);
            }
        }
        Value::Array(items) => {
            for child in items {
                collect_apple_tracks(child, songs);
            }
        }
        _ => {}
    }
}

// ── Deezer: public API ──────────────────────────────────────────────────────

async fn deezer(client: &reqwest::Client, link: &Link) -> Result<Collection> {
    let get = |path: String| async move {
        let value: Value = client
            .get(format!("https://api.deezer.com/{path}"))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        if let Some(message) = value["error"]["message"].as_str() {
            bail!("Deezer: {message}");
        }
        Ok::<_, anyhow::Error>(value)
    };
    match link.scope {
        Scope::Track => {
            let track = get(format!("track/{}", link.id)).await?;
            Ok(Collection {
                songs: vec![deezer_track(&track, None)],
                ..Collection::default()
            })
        }
        Scope::Album => {
            let album = get(format!("album/{}", link.id)).await?;
            let title = text(&album, "title");
            let songs = album["tracks"]["data"]
                .as_array()
                .into_iter()
                .flatten()
                .enumerate()
                .map(|(index, track)| {
                    let mut song = deezer_track(track, title.clone());
                    song.track_number.get_or_insert(index as i64 + 1);
                    song
                })
                .collect();
            Ok(Collection {
                title,
                owner: album["artist"]["name"].as_str().map(str::to_owned),
                songs,
            })
        }
        Scope::Playlist => {
            let playlist = get(format!("playlist/{}", link.id)).await?;
            let mut songs = Vec::new();
            let mut index = 0;
            loop {
                let page = get(format!(
                    "playlist/{}/tracks?limit=100&index={index}",
                    link.id
                ))
                .await?;
                let data = page["data"].as_array().cloned().unwrap_or_default();
                if data.is_empty() {
                    break;
                }
                index += data.len();
                songs.extend(data.iter().map(|track| deezer_track(track, None)));
                if page["next"].as_str().is_none() || songs.len() >= 10_000 {
                    break;
                }
            }
            Ok(Collection {
                title: text(&playlist, "title"),
                owner: playlist["creator"]["name"].as_str().map(str::to_owned),
                songs,
            })
        }
    }
}

fn deezer_track(track: &Value, album: Option<String>) -> Wanted {
    let contributors: Vec<String> = track["contributors"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|artist| artist["name"].as_str().map(str::to_owned))
        .collect();
    Wanted {
        title: text(track, "title").unwrap_or_default(),
        artist: if contributors.is_empty() {
            track["artist"]["name"].as_str().map(str::to_owned)
        } else {
            Some(contributors.join(", "))
        },
        album: album.or_else(|| track["album"]["title"].as_str().map(str::to_owned)),
        track_number: track["track_position"].as_i64(),
        duration: track["duration"].as_f64().filter(|value| *value > 0.0),
        isrc: text(track, "isrc"),
        source_url: text(track, "link"),
        youtube_id: None,
    }
}

// ── YouTube and Shazam ──────────────────────────────────────────────────────

async fn youtube(config: &Config, link: &Link) -> Result<Collection> {
    if link.scope == Scope::Track {
        let entry = ytdlp::details(config, &link.id).await?;
        return Ok(Collection {
            songs: vec![youtube_song(entry)],
            ..Collection::default()
        });
    }
    let listing = ytdlp::playlist(
        config,
        &format!("https://www.youtube.com/playlist?list={}", link.id),
    )
    .await?;
    Ok(Collection {
        title: listing.title,
        owner: listing.owner,
        songs: listing.entries.into_iter().map(youtube_song).collect(),
    })
}

fn youtube_song(entry: ytdlp::Entry) -> Wanted {
    Wanted {
        title: entry.title,
        artist: (!entry.artists.is_empty()).then(|| entry.artists.join(", ")),
        album: entry.album,
        duration: entry.duration,
        source_url: Some(format!("https://www.youtube.com/watch?v={}", entry.id)),
        youtube_id: Some(entry.id),
        ..Wanted::default()
    }
}

async fn shazam(
    client: &reqwest::Client,
    pool: &sqlx::SqlitePool,
    link: &Link,
) -> Result<Collection> {
    // Shazam's song pages refuse automated requests; the same song is in
    // Apple's lookup API under the same number, which is stable and official.
    if link.url.contains("/song/") {
        let apple_link = Link {
            service: Service::Apple,
            scope: Scope::Track,
            ..link.clone()
        };
        match apple(client, &apple_link).await {
            Ok(mut collection) if !collection.songs.is_empty() => {
                collection.songs.truncate(1);
                return Ok(collection);
            }
            Ok(_) => {}
            Err(error) => tracing::debug!(%error, "Apple lookup for a Shazam link failed"),
        }
    }
    let candidate = providers::shazam::lookup_url(pool, client, &link.url).await?;
    Ok(Collection {
        songs: vec![Wanted {
            title: candidate.title,
            artist: Some(candidate.artist).filter(|artist| !artist.is_empty()),
            album: candidate.album,
            track_number: candidate.track_number,
            isrc: candidate.isrc,
            source_url: Some(link.url.clone()),
            ..Wanted::default()
        }],
        ..Collection::default()
    })
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// The JSON inside `<script id="…">…</script>`.
fn script_json(html: &str, id: &str) -> Option<Value> {
    let marker = format!("id=\"{id}\"");
    let start = html.find(&marker)?;
    let open = start + html[start..].find('>')? + 1;
    let close = open + html[open..].find("</script>")?;
    serde_json::from_str(&html[open..close]).ok()
}

fn between<'a>(text: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let from = text.find(start)? + start.len();
    let to = from + text[from..].find(end)?;
    Some(&text[from..to])
}

fn milliseconds(value: &Value) -> Option<f64> {
    value.as_f64().filter(|ms| *ms > 0.0).map(|ms| ms / 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind(url: &str) -> Option<(Service, Scope, String)> {
        classify(url).map(|link| (link.service, link.scope, link.id))
    }

    #[test]
    fn recognises_links_from_every_service() {
        use Scope::*;
        use Service::*;
        let cases = [
            (
                "https://open.spotify.com/track/0VjIjW4GlUZAMYd2vXMi3b?si=x",
                Spotify,
                Track,
                "0VjIjW4GlUZAMYd2vXMi3b",
            ),
            (
                "https://open.spotify.com/intl-de/album/4aawyAB9vmqN3uQ7FjRGTy",
                Spotify,
                Album,
                "4aawyAB9vmqN3uQ7FjRGTy",
            ),
            (
                "https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M",
                Spotify,
                Playlist,
                "37i9dQZF1DXcBWIGoYBM5M",
            ),
            (
                "https://music.apple.com/us/album/blinding-lights/1499378108?i=1499378615",
                Apple,
                Track,
                "1499378615",
            ),
            (
                "https://music.apple.com/us/song/blinding-lights/1499378615",
                Apple,
                Track,
                "1499378615",
            ),
            (
                "https://music.apple.com/us/album/after-hours/1499378108",
                Apple,
                Album,
                "1499378108",
            ),
            (
                "https://music.apple.com/us/playlist/todays-hits/pl.f4d106fed2bd41149aaacabb233eb5eb",
                Apple,
                Playlist,
                "pl.f4d106fed2bd41149aaacabb233eb5eb",
            ),
            (
                "https://www.deezer.com/en/track/3135556",
                Deezer,
                Track,
                "3135556",
            ),
            (
                "https://www.deezer.com/album/302127",
                Deezer,
                Album,
                "302127",
            ),
            (
                "https://www.deezer.com/fr/playlist/908622995",
                Deezer,
                Playlist,
                "908622995",
            ),
            (
                "https://music.youtube.com/watch?v=J7p4bzqLvCw&feature=share",
                YouTube,
                Track,
                "J7p4bzqLvCw",
            ),
            (
                "https://youtu.be/J7p4bzqLvCw",
                YouTube,
                Track,
                "J7p4bzqLvCw",
            ),
            (
                "https://www.youtube.com/playlist?list=PL123",
                YouTube,
                Playlist,
                "PL123",
            ),
            (
                "https://www.youtube.com/shorts/abc123",
                YouTube,
                Track,
                "abc123",
            ),
            (
                "https://www.shazam.com/song/1499378615/blinding-lights",
                Shazam,
                Track,
                "1499378615",
            ),
            (
                "https://www.shazam.com/en-us/song/1845811123/donyaye-movazi#referrer=x",
                Shazam,
                Track,
                "1845811123",
            ),
        ];
        for (url, service, scope, id) in cases {
            assert_eq!(kind(url), Some((service, scope, id.to_owned())), "{url}");
        }
        assert_eq!(classify("https://open.spotify.com/artist/123"), None);
        assert_eq!(classify("https://example.com/song"), None);
        assert_eq!(classify("not a link"), None);
        assert_eq!(
            classify("https://music.apple.com/gb/album/x/1")
                .unwrap()
                .country
                .as_deref(),
            Some("gb")
        );
    }

    fn link(scope: Scope) -> Link {
        Link {
            service: Service::Spotify,
            scope,
            id: "abc".into(),
            url: "https://open.spotify.com/x/abc".into(),
            country: None,
        }
    }

    fn page(entity: Value) -> String {
        format!(
            r#"<html><script id="__NEXT_DATA__" type="application/json">{}</script></html>"#,
            serde_json::json!({"props": {"pageProps": {"state": {"data": {"entity": entity}}}}})
        )
    }

    #[test]
    fn reads_spotify_embed_pages() {
        let track = page(serde_json::json!({
            "type": "track", "name": "Blinding Lights", "duration": 200040,
            "artists": [{"name": "The Weeknd"}]
        }));
        let collection = parse_spotify_embed(&track, &link(Scope::Track)).unwrap();
        assert_eq!(collection.songs[0].artist.as_deref(), Some("The Weeknd"));
        assert_eq!(collection.songs[0].duration, Some(200.04));

        let album = page(serde_json::json!({
            "type": "album", "name": "Global Warming", "subtitle": "Pitbull",
            "trackList": [
                {"uri": "spotify:track:1", "title": "Global Warming (feat. Sensato)", "subtitle": "Pitbull,\u{a0}Sensato", "duration": 85400},
                {"uri": "spotify:track:2", "title": "Don't Stop the Party", "subtitle": "Pitbull", "duration": 206120}
            ]
        }));
        let collection = parse_spotify_embed(&album, &link(Scope::Album)).unwrap();
        assert_eq!(collection.title.as_deref(), Some("Global Warming"));
        assert_eq!(collection.owner.as_deref(), Some("Pitbull"));
        assert_eq!(collection.songs.len(), 2);
        assert_eq!(
            collection.songs[0].artist.as_deref(),
            Some("Pitbull, Sensato")
        );
        assert_eq!(collection.songs[1].track_number, Some(2));
        assert_eq!(collection.songs[1].album.as_deref(), Some("Global Warming"));
        assert_eq!(
            collection.songs[0].source_url.as_deref(),
            Some("https://open.spotify.com/track/1")
        );
    }

    #[test]
    fn reads_itunes_lookup_and_apple_playlist_pages() {
        let lookup = serde_json::json!({"results": [
            {"wrapperType": "collection", "collectionName": "Please Please Me", "artistName": "The Beatles"},
            {"wrapperType": "track", "kind": "song", "trackName": "Love Me Do", "artistName": "The Beatles",
             "collectionName": "Please Please Me", "trackNumber": 8, "trackTimeMillis": 140173}
        ]});
        let album = parse_itunes_lookup(&lookup, Scope::Album);
        assert_eq!(album.title.as_deref(), Some("Please Please Me"));
        assert_eq!(album.songs[0].track_number, Some(8));

        let html = format!(
            "<title>\u{200e}Today’s Hits - Playlist - Apple Music</title><script type=\"application/json\" id=\"serialized-server-data\">{}</script>",
            serde_json::json!([{"data": {"sections": [{"items": [
                {"title": "Choosin' Texas", "artistName": "Ella Langley", "duration": 232226,
                 "tertiaryLinks": [{"title": "Choosin' Texas - Single"}]}
            ]}]}}])
        );
        let playlist =
            parse_apple_playlist(&html, "https://music.apple.com/us/playlist/x/pl.1").unwrap();
        assert_eq!(playlist.title.as_deref(), Some("Today’s Hits"));
        assert_eq!(playlist.songs[0].artist.as_deref(), Some("Ella Langley"));
        assert_eq!(playlist.songs[0].duration, Some(232.226));
    }

    #[test]
    fn reads_deezer_tracks() {
        let track = serde_json::json!({
            "title": "Harder, Better, Faster, Stronger", "duration": 224, "isrc": "GBDUW0000059",
            "track_position": 4, "artist": {"name": "Daft Punk"}, "album": {"title": "Discovery"},
            "contributors": [{"name": "Daft Punk"}]
        });
        let song = deezer_track(&track, None);
        assert_eq!(song.isrc.as_deref(), Some("GBDUW0000059"));
        assert_eq!(song.album.as_deref(), Some("Discovery"));
        assert_eq!(song.track_number, Some(4));
    }
}
