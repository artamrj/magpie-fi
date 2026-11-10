use crate::{domain::credits::identity_key, infrastructure::providers::Candidate};
use anyhow::Result;
use chrono::Utc;
use sqlx::SqlitePool;
use std::collections::HashMap;

#[derive(Clone)]
struct Canonical {
    value: String,
    authority: i64,
}

/// Resolves every artist to its identity (see [`crate::application::artists`])
/// and gives every title the most trusted spelling seen so far, across these
/// candidates and earlier scans. Expects credits already normalized (see
/// [`normalize_credits`]). `Mode::Learn` (filing) also creates new artists.
pub async fn canonicalize_candidates(
    pool: &SqlitePool,
    client: Option<&reqwest::Client>,
    candidates: &mut [Candidate],
    mode: crate::application::artists::Mode,
) -> Result<()> {
    crate::application::artists::resolve_candidates(pool, client, candidates, mode).await?;
    let mut winners = HashMap::<(String, String), Canonical>::new();
    for candidate in candidates.iter() {
        let authority = provider_authority(&candidate.provider);
        winners
            .entry(("title".into(), title_identity(candidate)))
            .and_modify(|current| {
                if authority > current.authority {
                    *current = Canonical {
                        value: candidate.title.clone(),
                        authority,
                    };
                }
            })
            .or_insert_with(|| Canonical {
                value: candidate.title.clone(),
                authority,
            });
    }

    // A stored spelling with at least the same authority wins; the row that
    // results from the upsert is the canonical one.
    let now = Utc::now().to_rfc3339();
    let mut transaction = pool.begin().await?;
    for ((kind, key), winner) in &mut winners {
        (winner.value, winner.authority) = sqlx::query_as(
            "INSERT INTO canonical_names(kind,identity_key,canonical_value,authority,updated_at)
             VALUES(?,?,?,?,?) ON CONFLICT(kind,identity_key) DO UPDATE SET
             canonical_value=CASE WHEN excluded.authority>authority THEN excluded.canonical_value ELSE canonical_value END,
             authority=MAX(authority,excluded.authority),updated_at=excluded.updated_at
             RETURNING canonical_value,authority",
        )
        .bind(kind)
        .bind(key)
        .bind(&winner.value)
        .bind(winner.authority)
        .bind(&now)
        .fetch_one(&mut *transaction)
        .await?;
    }
    transaction.commit().await?;

    for candidate in candidates {
        if let Some(canonical) = winners.get(&("title".into(), title_identity(candidate))) {
            candidate.title = canonical.value.clone();
        }
    }
    Ok(())
}

/// Splits featured artists out of titles and artist strings into structured
/// credits, preferring a Latin alias for names that have one.
pub fn normalize_credits(candidate: &mut Candidate) {
    let artist = crate::domain::credits::prefer_latin_alias(&candidate.artist);
    let credits = crate::domain::credits::normalize_structured(
        &artist,
        &candidate.title,
        std::mem::take(&mut candidate.artist_credits),
    );
    candidate.artist = credits.artist;
    candidate.title = credits.title;
    candidate.artist_credits = credits.artists;
    if let Some(album_artist) = candidate.album_artist.as_deref() {
        let album_artist = crate::domain::credits::prefer_latin_alias(album_artist);
        let credits = crate::domain::credits::normalize_structured(
            &album_artist,
            "",
            std::mem::take(&mut candidate.album_artist_credits),
        );
        candidate.album_artist = Some(credits.artist);
        candidate.album_artist_credits = credits.artists;
    }
}

fn title_identity(candidate: &Candidate) -> String {
    if let Some(id) = candidate.recording_id.as_deref() {
        return format!("mbid:{}", id.trim().to_ascii_lowercase());
    }
    if let Some(isrc) = candidate.isrc.as_deref() {
        return format!("isrc:{}", isrc.trim().to_ascii_uppercase());
    }
    format!(
        "text:{}|{}",
        identity_key(&candidate.artist),
        identity_key(&candidate.title)
    )
}

fn provider_authority(provider: &str) -> i64 {
    match provider {
        "musicbrainz" => 100,
        "spotify" => 95,
        "itunes" | "deezer" => 90,
        "shazam" | "genius" => 80,
        "manual" => 70,
        _ => 60,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::providers::ArtistCredit;

    #[tokio::test]
    async fn trusted_spelling_wins_and_persists() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("canonical.sqlite");
        let pool = crate::infrastructure::db::connect(database.to_str().unwrap())
            .await
            .unwrap();
        let mut candidates = vec![
            Candidate {
                provider: "genius".into(),
                title: "SONG".into(),
                artist: "ANDY".into(),
                artist_credits: vec![ArtistCredit::new("ANDY", "")],
                isrc: Some("IR-TEST".into()),
                ..Default::default()
            },
            Candidate {
                provider: "musicbrainz".into(),
                title: "Song".into(),
                artist: "Andy".into(),
                artist_credits: vec![ArtistCredit::new("Andy", "")],
                isrc: Some("IR-TEST".into()),
                ..Default::default()
            },
        ];
        canonicalize_candidates(
            &pool,
            None,
            &mut candidates,
            crate::application::artists::Mode::Lookup,
        )
        .await
        .unwrap();
        assert!(
            candidates
                .iter()
                .all(|candidate| candidate.artist == "Andy")
        );
        assert!(candidates.iter().all(|candidate| candidate.title == "Song"));
    }

    #[tokio::test]
    async fn one_band_identity_uses_the_same_catalog_spelling_everywhere() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("band-canonical.sqlite");
        let pool = crate::infrastructure::db::connect(database.to_str().unwrap())
            .await
            .unwrap();
        let mut candidates = vec![Candidate {
            provider: "musicbrainz".into(),
            title: "Love You Like a Love Song".into(),
            artist: "Selena Gomez & The Scene".into(),
            artist_credits: vec![ArtistCredit::new("Selena Gomez & the Scene", "")],
            album_artist: Some("Selena Gomez & The Scene".into()),
            album_artist_credits: vec![ArtistCredit::new("Selena Gomez & The Scene", "")],
            ..Default::default()
        }];

        canonicalize_candidates(
            &pool,
            None,
            &mut candidates,
            crate::application::artists::Mode::Lookup,
        )
        .await
        .unwrap();

        let candidate = &candidates[0];
        assert_eq!(candidate.artist, "Selena Gomez & the Scene");
        assert_eq!(
            candidate.album_artist.as_deref(),
            Some("Selena Gomez & the Scene")
        );
        assert_eq!(candidate.artist_credits.len(), 1);
        assert_eq!(candidate.album_artist_credits.len(), 1);
    }
}
