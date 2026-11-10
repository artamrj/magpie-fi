//! Artist identities: one row per real artist (`artists`) with every spelling
//! seen for it (`artist_names`). Every credit is resolved to its identity, so
//! `گوگوش`, `Googoosh` and `GOOGOOSH` give one name in the tags and one folder.
//!
//! Credits such as `Ebi & Shadmehr Aghili` are split into separate artists when
//! every part is a known artist and the whole is not known to be one band
//! (MusicBrainz or the owner says so). The owner's own split rules always win.
//!
//! New artists are only learned when a song is filed or the library is indexed
//! ([`Mode::Learn`]); scans only look names up, so search noise never becomes
//! an artist.

use crate::{
    domain::{
        credits::{ArtistCredit, display_artist},
        names::{has_arabic_script, latin_key, match_key},
    },
    infrastructure::providers::{Candidate, musicbrainz},
};
use anyhow::Result;
use chrono::Utc;
use reqwest::Client;
use sqlx::SqlitePool;
use std::collections::HashMap;

/// Creating identities is serialized, so two songs by a new artist filed at
/// the same moment never create it twice.
static LEARN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Words between artists in a credit. `feat.`, `/` and `;` are split earlier
/// (they always separate artists); these may be part of a band's name.
const SEPARATORS: [&str; 11] = [
    " & ", " x ", " X ", " × ", ", ", "، ", " و ", " with ", " With ", " vs. ", " vs ",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mode {
    /// Use known identities only (scans, choosing a match).
    Lookup,
    /// Also create identities for new artists (filing, indexing the library).
    Learn,
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct Artist {
    pub id: i64,
    pub name: String,
    pub musicbrainz_id: Option<String>,
    pub single_entity: bool,
}

/// Resolves every artist and album-artist credit of these candidates and
/// rebuilds their display strings. With `client`, new Persian-script names are
/// looked up on MusicBrainz so they join the artist's Latin spelling.
pub async fn resolve_candidates(
    pool: &SqlitePool,
    client: Option<&Client>,
    candidates: &mut [Candidate],
    mode: Mode,
) -> Result<()> {
    let musicbrainz = match (mode, client) {
        (Mode::Learn, Some(client)) => musicbrainz_identities(pool, client, candidates).await,
        _ => HashMap::new(),
    };
    let _learning = match mode {
        Mode::Learn => Some(LEARN.lock().await),
        Mode::Lookup => None,
    };
    let mut order = (0..candidates.len()).collect::<Vec<_>>();
    order.sort_by_key(|index| std::cmp::Reverse(authority(&candidates[*index].provider)));
    // Within one batch, unknown names that are the same name get one spelling
    // (the most trusted source's), even when nothing is learned.
    let mut seen = HashMap::<String, String>::new();
    let resolver = Resolver {
        pool,
        mode,
        musicbrainz: &musicbrainz,
    };
    for index in order {
        let candidate = &mut candidates[index];
        let source = candidate.provider.clone();
        candidate.artist_credits = resolver
            .resolve_list(
                std::mem::take(&mut candidate.artist_credits),
                &source,
                &mut seen,
            )
            .await?;
        candidate.album_artist_credits = resolver
            .resolve_list(
                std::mem::take(&mut candidate.album_artist_credits),
                &source,
                &mut seen,
            )
            .await?;
        if !candidate.artist_credits.is_empty() {
            candidate.artist = display_artist(&candidate.artist_credits);
        }
        if !candidate.album_artist_credits.is_empty() {
            candidate.album_artist = Some(display_artist(&candidate.album_artist_credits));
        }
    }
    Ok(())
}

/// Identities for plain names (from the tags of library files). Learns new ones.
pub async fn resolve_names(pool: &SqlitePool, credits: Vec<ArtistCredit>) -> Result<Vec<i64>> {
    let _learning = LEARN.lock().await;
    let resolver = Resolver {
        pool,
        mode: Mode::Learn,
        musicbrainz: &HashMap::new(),
    };
    let credits = resolver
        .resolve_list(credits, "library", &mut HashMap::new())
        .await?;
    Ok(credits
        .iter()
        .filter_map(|credit| credit.artist_id)
        .collect())
}

/// `|3|17|`: how library rows store a song's artists (first = primary).
pub fn ids_text(ids: &[i64]) -> Option<String> {
    (!ids.is_empty()).then(|| {
        let joined = ids.iter().map(i64::to_string).collect::<Vec<_>>().join("|");
        format!("|{joined}|")
    })
}

pub fn ids_from_text(text: &str) -> Vec<i64> {
    text.split('|').filter_map(|id| id.parse().ok()).collect()
}

struct Resolver<'a> {
    pool: &'a SqlitePool,
    mode: Mode,
    musicbrainz: &'a HashMap<String, musicbrainz::ArtistIdentity>,
}

impl Resolver<'_> {
    async fn resolve_list(
        &self,
        credits: Vec<ArtistCredit>,
        source: &str,
        seen: &mut HashMap<String, String>,
    ) -> Result<Vec<ArtistCredit>> {
        let mut out: Vec<ArtistCredit> = Vec::with_capacity(credits.len());
        for credit in credits {
            for mut part in self.split(credit).await? {
                self.resolve(&mut part, source, seen).await?;
                // The same artist twice in one credit ("Ebi & Ebi") is shown once.
                if let Some(id) = part.artist_id
                    && out.iter().any(|existing| existing.artist_id == Some(id))
                {
                    continue;
                }
                out.push(part);
            }
        }
        if let Some(last) = out.last_mut() {
            last.join_phrase.clear();
        }
        Ok(out)
    }

    /// One credit, or its parts when it names several known artists.
    async fn split(&self, credit: ArtistCredit) -> Result<Vec<ArtistCredit>> {
        if credit.musicbrainz_id.is_some() {
            return Ok(vec![credit]);
        }
        let key = match_key(&credit.name);
        if let Some(parts) = owner_split(self.pool, &key).await? {
            return Ok(with_join(parts, &credit.join_phrase));
        }
        let pieces = split_on_separators(&credit.name);
        if pieces.len() < 2 {
            return Ok(vec![credit]);
        }
        if let Some(whole) = by_key(self.pool, &key).await?
            && whole.single_entity
        {
            return Ok(vec![credit]);
        }
        for piece in &pieces {
            if self.known(&piece.name).await?.is_none() {
                return Ok(vec![credit]);
            }
        }
        Ok(with_join(pieces, &credit.join_phrase))
    }

    /// A known identity for this spelling, without learning anything.
    async fn known(&self, name: &str) -> Result<Option<Artist>> {
        let key = match_key(name);
        if let Some(artist) = by_key(self.pool, &key).await? {
            return Ok(Some(artist));
        }
        if let Some(latin) = latin_key(name)
            && let Some(artist) = by_unique_latin_key(self.pool, &latin).await?
        {
            return Ok(Some(artist));
        }
        if let Some(found) = self.musicbrainz.get(&key) {
            if let Some(artist) = by_musicbrainz_id(self.pool, &found.id).await? {
                return Ok(Some(artist));
            }
            for alias in &found.aliases {
                if let Some(artist) = by_key(self.pool, &match_key(alias)).await? {
                    return Ok(Some(artist));
                }
            }
        }
        Ok(None)
    }

    async fn resolve(
        &self,
        credit: &mut ArtistCredit,
        source: &str,
        seen: &mut HashMap<String, String>,
    ) -> Result<()> {
        let key = match_key(&credit.name);
        if key.is_empty() {
            return Ok(());
        }
        let mut artist = match credit.musicbrainz_id.as_deref() {
            Some(id) => by_musicbrainz_id(self.pool, id).await?,
            None => None,
        };
        if artist.is_none() {
            artist = self.known(&credit.name).await?;
        }
        if artist.is_none() && self.mode == Mode::Learn {
            artist = Some(self.create(credit, source).await?);
        }
        let Some(artist) = artist else {
            let spelling = seen.entry(key).or_insert_with(|| credit.name.clone());
            credit.name = spelling.clone();
            return Ok(());
        };
        if self.mode == Mode::Learn {
            remember_name(self.pool, artist.id, &credit.name, source).await?;
            if artist.musicbrainz_id.is_none()
                && let Some(id) = credit.musicbrainz_id.as_deref()
            {
                sqlx::query(
                    "UPDATE artists SET musicbrainz_id=?,updated_at=? WHERE id=?
                     AND NOT EXISTS(SELECT 1 FROM artists WHERE musicbrainz_id=?)",
                )
                .bind(id)
                .bind(Utc::now().to_rfc3339())
                .bind(artist.id)
                .bind(id)
                .execute(self.pool)
                .await?;
            }
        }
        credit.name = artist.name.clone();
        credit.artist_id = Some(artist.id);
        if credit.musicbrainz_id.is_none() {
            credit.musicbrainz_id = artist.musicbrainz_id;
        }
        Ok(())
    }

    async fn create(&self, credit: &ArtistCredit, source: &str) -> Result<Artist> {
        let found = self.musicbrainz.get(&match_key(&credit.name));
        let musicbrainz_id = credit
            .musicbrainz_id
            .clone()
            .or_else(|| found.map(|found| found.id.clone()));
        // MusicBrainz lists this exact name as one artist: never split it later.
        let single_entity = musicbrainz_id.is_some();
        let now = Utc::now().to_rfc3339();
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO artists(name,musicbrainz_id,single_entity,created_at,updated_at)
             VALUES(?,?,?,?,?) RETURNING id",
        )
        .bind(&credit.name)
        .bind(&musicbrainz_id)
        .bind(single_entity)
        .bind(&now)
        .bind(&now)
        .fetch_one(self.pool)
        .await?;
        remember_name(self.pool, id, &credit.name, source).await?;
        for alias in found
            .map(|found| found.aliases.as_slice())
            .unwrap_or_default()
        {
            remember_name(self.pool, id, alias, "musicbrainz").await?;
        }
        Ok(Artist {
            id,
            name: credit.name.clone(),
            musicbrainz_id,
            single_entity,
        })
    }
}

/// MusicBrainz identities for new Persian-script names, fetched before the
/// learning lock (MusicBrainz allows one request a second). Failures only mean
/// "not linked to its Latin spelling yet".
async fn musicbrainz_identities(
    pool: &SqlitePool,
    client: &Client,
    candidates: &[Candidate],
) -> HashMap<String, musicbrainz::ArtistIdentity> {
    let mut out = HashMap::new();
    let names = candidates
        .iter()
        .flat_map(|candidate| {
            candidate
                .artist_credits
                .iter()
                .chain(&candidate.album_artist_credits)
        })
        .filter(|credit| credit.musicbrainz_id.is_none())
        .flat_map(|credit| {
            let mut names = vec![credit.name.clone()];
            names.extend(
                split_on_separators(&credit.name)
                    .into_iter()
                    .map(|part| part.name),
            );
            names
        })
        .filter(|name| has_arabic_script(name))
        .collect::<Vec<_>>();
    for name in names {
        let key = match_key(&name);
        if out.contains_key(&key) || by_key(pool, &key).await.ok().flatten().is_some() {
            continue;
        }
        match musicbrainz::artist_identity(pool, client, &name).await {
            Ok(Some(found)) => {
                out.insert(key, found);
            }
            Ok(None) => {}
            Err(error) => tracing::debug!(%error, name, "MusicBrainz artist lookup failed"),
        }
    }
    out
}

fn split_on_separators(name: &str) -> Vec<ArtistCredit> {
    let mut parts = Vec::new();
    let mut rest = name.trim();
    while let Some((index, separator)) = SEPARATORS
        .iter()
        .filter_map(|separator| rest.find(separator).map(|index| (index, *separator)))
        .min_by_key(|(index, _)| *index)
    {
        let part = rest[..index].trim();
        if !part.is_empty() {
            parts.push(ArtistCredit::new(part, normal_join(separator)));
        }
        rest = rest[index + separator.len()..].trim();
    }
    if !rest.is_empty() {
        parts.push(ArtistCredit::new(rest, ""));
    }
    parts
}

fn normal_join(separator: &str) -> String {
    match separator.trim() {
        "," | "،" => ", ".into(),
        "x" | "X" | "×" => " x ".into(),
        other => format!(" {other} "),
    }
}

fn with_join(mut parts: Vec<ArtistCredit>, last_join: &str) -> Vec<ArtistCredit> {
    if let Some(last) = parts.last_mut() {
        last.join_phrase = last_join.to_owned();
    }
    parts
}

async fn owner_split(pool: &SqlitePool, key: &str) -> Result<Option<Vec<ArtistCredit>>> {
    let parts: Option<String> =
        sqlx::query_scalar("SELECT parts_json FROM artist_splits WHERE match_key=?")
            .bind(key)
            .fetch_optional(pool)
            .await?;
    Ok(parts
        .and_then(|json| serde_json::from_str::<Vec<String>>(&json).ok())
        .map(|names| {
            let count = names.len();
            names
                .into_iter()
                .enumerate()
                .map(|(index, name)| {
                    ArtistCredit::new(name, if index + 1 < count { " & " } else { "" })
                })
                .collect()
        }))
}

async fn by_key(pool: &SqlitePool, key: &str) -> Result<Option<Artist>> {
    Ok(sqlx::query_as(
        "SELECT artists.id,artists.name,artists.musicbrainz_id,artists.single_entity FROM artist_names
         JOIN artists ON artists.id=artist_names.artist_id WHERE artist_names.match_key=?",
    )
    .bind(key)
    .fetch_optional(pool)
    .await?)
}

async fn by_unique_latin_key(pool: &SqlitePool, latin: &str) -> Result<Option<Artist>> {
    let found: Vec<Artist> = sqlx::query_as(
        "SELECT DISTINCT artists.id,artists.name,artists.musicbrainz_id,artists.single_entity FROM artist_names
         JOIN artists ON artists.id=artist_names.artist_id WHERE artist_names.latin_key=? LIMIT 2",
    )
    .bind(latin)
    .fetch_all(pool)
    .await?;
    Ok(match <[Artist; 1]>::try_from(found) {
        Ok([artist]) => Some(artist),
        Err(_) => None,
    })
}

async fn by_musicbrainz_id(pool: &SqlitePool, id: &str) -> Result<Option<Artist>> {
    Ok(sqlx::query_as(
        "SELECT artists.id,artists.name,artists.musicbrainz_id,artists.single_entity FROM artists WHERE musicbrainz_id=?",
    )
    .bind(id.trim().to_ascii_lowercase())
    .fetch_optional(pool)
    .await?)
}

async fn remember_name(
    pool: &SqlitePool,
    artist_id: i64,
    spelling: &str,
    source: &str,
) -> Result<()> {
    let key = match_key(spelling);
    if key.is_empty() {
        return Ok(());
    }
    sqlx::query(
        "INSERT OR IGNORE INTO artist_names(match_key,artist_id,spelling,latin_key,source)
         VALUES(?,?,?,?,?)",
    )
    .bind(key)
    .bind(artist_id)
    .bind(spelling)
    .bind(latin_key(spelling))
    .bind(source)
    .execute(pool)
    .await?;
    Ok(())
}

fn authority(provider: &str) -> i64 {
    match provider {
        "musicbrainz" => 100,
        "spotify" => 95,
        "itunes" | "deezer" => 90,
        "shazam" | "audd" | "genius" => 80,
        "manual" => 70,
        _ => 60,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn pool() -> (tempfile::TempDir, SqlitePool) {
        let directory = tempfile::tempdir().unwrap();
        let pool = crate::infrastructure::db::connect(
            directory.path().join("artists.sqlite").to_str().unwrap(),
        )
        .await
        .unwrap();
        (directory, pool)
    }

    fn song(provider: &str, artist: &str) -> Candidate {
        let mut candidate = Candidate {
            provider: provider.into(),
            title: "Song".into(),
            artist: artist.into(),
            ..Default::default()
        };
        crate::application::canonical_names::normalize_credits(&mut candidate);
        candidate
    }

    async fn learn(pool: &SqlitePool, provider: &str, artist: &str) -> Candidate {
        let mut candidates = vec![song(provider, artist)];
        resolve_candidates(pool, None, &mut candidates, Mode::Learn)
            .await
            .unwrap();
        candidates.remove(0)
    }

    #[tokio::test]
    async fn every_spelling_of_an_artist_gets_one_name() {
        let (_directory, pool) = pool().await;
        let first = learn(&pool, "itunes", "Mohsen Yeganeh").await;
        let id = first.artist_credits[0].artist_id.unwrap();
        for spelling in ["MOHSEN YEGANEH", "Mohsen Yegane", "mohsen  yeganeh"] {
            let again = learn(&pool, "radiojavan", spelling).await;
            assert_eq!(again.artist, "Mohsen Yeganeh", "{spelling}");
            assert_eq!(again.artist_credits[0].artist_id, Some(id));
        }
        let other = learn(&pool, "itunes", "Mohsen Chavoshi").await;
        assert_ne!(other.artist_credits[0].artist_id, Some(id));
    }

    #[tokio::test]
    async fn a_duet_is_split_only_when_both_singers_are_known() {
        let (_directory, pool) = pool().await;
        // Nobody known yet: the credit stays whole.
        let unknown = learn(&pool, "itunes", "Ebi & Shadmehr Aghili").await;
        assert_eq!(unknown.artist_credits.len(), 1);

        learn(&pool, "musicbrainz", "Ebi").await;
        learn(&pool, "musicbrainz", "Shadmehr Aghili").await;
        let duet = learn(&pool, "itunes", "Ebi & Shadmehr Aghili").await;
        let names = duet
            .artist_credits
            .iter()
            .map(|credit| credit.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, ["Ebi", "Shadmehr Aghili"]);
        assert_eq!(duet.artist, "Ebi & Shadmehr Aghili");
    }

    #[tokio::test]
    async fn a_band_with_and_in_its_name_stays_one_artist() {
        let (_directory, pool) = pool().await;
        learn(&pool, "musicbrainz", "Simon").await;
        learn(&pool, "musicbrainz", "Garfunkel").await;
        // MusicBrainz lists the duo as one artist (it has an MBID).
        let mut band = song("musicbrainz", "Simon & Garfunkel");
        band.artist_credits[0].musicbrainz_id = Some("5d02f264-e225-41ff-83f7-d9b1f0b1874a".into());
        let mut candidates = vec![band];
        resolve_candidates(&pool, None, &mut candidates, Mode::Learn)
            .await
            .unwrap();
        // Another source writes the same duo without an ID: still one artist.
        let again = learn(&pool, "itunes", "Simon and Garfunkel").await;
        assert_eq!(again.artist_credits.len(), 1);
        assert_eq!(again.artist, "Simon & Garfunkel");
    }

    #[tokio::test]
    async fn the_owner_can_split_a_credit() {
        let (_directory, pool) = pool().await;
        sqlx::query(
            "INSERT INTO artist_splits(match_key,spelling,parts_json,created_at) VALUES(?,?,?,'now')",
        )
        .bind(match_key("Sasy x Tataloo"))
        .bind("Sasy x Tataloo")
        .bind(r#"["Sasy","Amir Tataloo"]"#)
        .execute(&pool)
        .await
        .unwrap();
        let split = learn(&pool, "itunes", "Sasy x Tataloo").await;
        assert_eq!(split.artist, "Sasy & Amir Tataloo");
        assert_eq!(split.artist_credits.len(), 2);
    }

    #[tokio::test]
    async fn scans_never_learn_artists() {
        let (_directory, pool) = pool().await;
        let mut candidates = vec![
            song("genius", "Search Noise"),
            song("itunes", "SEARCH NOISE"),
        ];
        resolve_candidates(&pool, None, &mut candidates, Mode::Lookup)
            .await
            .unwrap();
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM artists")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
        // The same name in one batch still gets one spelling (the more trusted source's).
        assert_eq!(candidates[0].artist, candidates[1].artist);
        assert_eq!(candidates[0].artist, "SEARCH NOISE");
    }

    #[test]
    fn credits_split_at_the_usual_words() {
        let parts = split_on_separators("Ebi, Dariush & Googoosh");
        let names = parts
            .iter()
            .map(|part| part.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, ["Ebi", "Dariush", "Googoosh"]);
        assert_eq!(split_on_separators("ابی و شادمهر").len(), 2);
        assert_eq!(split_on_separators("Malcolm X").len(), 1);
        assert_eq!(ids_from_text(&ids_text(&[3, 17]).unwrap()), [3, 17]);
    }
}
