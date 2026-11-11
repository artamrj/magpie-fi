//! SoundCloud links: title, artist and cover from the public oEmbed page.
//! Search was removed because it needs an app key SoundCloud no longer gives out.
use super::{Candidate, request_json};
use anyhow::{Result, bail};
use reqwest::Client;
use serde_json::Value;

pub async fn lookup_url(client: &Client, url: &str) -> Result<Candidate> {
    validate_url(url)?;
    let raw = request_json(
        client
            .get("https://soundcloud.com/oembed")
            .query(&[("url", url), ("format", "json")]),
    )
    .await?;
    candidate_from_oembed(&raw).ok_or_else(|| anyhow::anyhow!("SoundCloud returned no title"))
}

fn validate_url(url: &str) -> Result<()> {
    let parsed = reqwest::Url::parse(url)?;
    if parsed.scheme() != "https"
        || !matches!(
            parsed.host_str(),
            Some("soundcloud.com" | "www.soundcloud.com")
        )
    {
        bail!("only HTTPS SoundCloud source links are supported");
    }
    Ok(())
}

fn candidate_from_oembed(raw: &Value) -> Option<Candidate> {
    let raw_title = raw["title"].as_str()?.trim();
    let author = raw["author_name"].as_str().unwrap_or("SoundCloud artist");
    let suffix = format!(" by {author}");
    let cleaned =
        strip_audio_extension(raw_title.strip_suffix(&suffix).unwrap_or(raw_title).trim());
    let (artist, title) =
        split_artist_title(cleaned).unwrap_or_else(|| (author.to_owned(), cleaned.to_owned()));
    let credits = crate::domain::credits::normalize_featured(&artist, &title);
    Some(Candidate {
        provider: "soundcloud".into(),
        title: credits.title,
        artist: credits.artist,
        artist_credits: credits.artists,
        cover_url: raw["thumbnail_url"].as_str().map(upgrade_artwork_url),
        score: 94.0,
        score_breakdown: Some(
            serde_json::json!({
                "source": "soundcloud_user_source_url",
                "sources": ["SoundCloud"],
                "user_verified_source": true
            })
            .to_string(),
        ),
        raw_json: raw.to_string(),
        ..Default::default()
    })
}

fn split_artist_title(value: &str) -> Option<(String, String)> {
    [" - ", " – ", " — ", " | "]
        .into_iter()
        .find_map(|separator| {
            let (artist, title) = value.split_once(separator)?;
            (!artist.trim().is_empty() && !title.trim().is_empty())
                .then(|| (artist.trim().to_owned(), title.trim().to_owned()))
        })
}

fn strip_audio_extension(value: &str) -> &str {
    for extension in [".mp3", ".m4a", ".flac", ".wav", ".ogg", ".opus"] {
        if value.to_ascii_lowercase().strip_suffix(extension).is_some() {
            return &value[..value.len() - extension.len()];
        }
    }
    value
}

pub fn upgrade_artwork_url(url: &str) -> String {
    const SIZES: [&str; 7] = [
        "-large.",
        "-t300x300.",
        "-t200x200.",
        "-t120x120.",
        "-t67x67.",
        "-small.",
        "-tiny.",
    ];
    SIZES
        .into_iter()
        .fold(url.to_owned(), |url, size| url.replace(size, "-t500x500."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_user_supplied_soundcloud_example() {
        let candidate = candidate_from_oembed(&serde_json::json!({
            "title": "Arta - Hanooz Yadame (Ft Koorosh, Sami Low, & Raha) by Sina Mohtasham",
            "author_name": "Sina Mohtasham",
            "thumbnail_url": "https://i1.sndcdn.com/artworks-id-t500x500.jpg"
        }))
        .unwrap();
        assert_eq!(candidate.artist, "Arta feat. Koorosh, Sami Low, & Raha");
        assert_eq!(candidate.title, "Hanooz Yadame");
        assert_eq!(candidate.score, 94.0);
    }

    #[test]
    fn removes_an_audio_extension_from_oembed_title() {
        let candidate = candidate_from_oembed(&serde_json::json!({
            "title": "Mamehaye Golshifteh.mp3 by Amir Tataloo",
            "author_name": "Amir Tataloo",
            "thumbnail_url": "https://i1.sndcdn.com/artworks-id-t500x500.jpg"
        }))
        .unwrap();
        assert_eq!(candidate.title, "Mamehaye Golshifteh");
        assert_eq!(candidate.artist, "Amir Tataloo");
    }
}
