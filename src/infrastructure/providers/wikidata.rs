//! Artist genres from Wikidata ("genre", P136), used when the catalogs give
//! a track no genre.

use super::request_json;
use crate::infrastructure::provider_cache::{cached, search_key};
use anyhow::Result;
use chrono::Duration;
use reqwest::Client;
use sqlx::SqlitePool;

const API: &str = "https://www.wikidata.org/w/api.php";

pub async fn artist_genres(
    pool: &SqlitePool,
    client: &Client,
    artist: &str,
) -> Result<Vec<String>> {
    let search_key_value = search_key(&format!("artist-profile {artist}"));
    let search = cached(
        pool,
        "wikidata_artist",
        &search_key_value,
        Duration::days(30),
        || {
            request_json(client.get(API).query(&[
                ("action", "wbsearchentities"),
                ("format", "json"),
                ("language", "en"),
                ("limit", "3"),
                ("search", artist),
            ]))
        },
    )
    .await?;
    let Some(entity_id) = search["search"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|result| {
            result["label"].as_str().is_some_and(|label| {
                strsim::normalized_levenshtein(&label.to_lowercase(), &artist.to_lowercase())
                    >= 0.72
            })
        })
        .and_then(|result| result["id"].as_str())
    else {
        return Ok(Vec::new());
    };

    let entity = cached(
        pool,
        "wikidata_entity",
        &format!("entity:{entity_id}"),
        Duration::days(30),
        || {
            request_json(client.get(API).query(&[
                ("action", "wbgetentities"),
                ("format", "json"),
                ("props", "claims"),
                ("ids", entity_id),
            ]))
        },
    )
    .await?;
    let genre_ids = entity["entities"][entity_id]["claims"]["P136"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|claim| claim["mainsnak"]["datavalue"]["value"]["id"].as_str())
        .collect::<Vec<_>>();
    if genre_ids.is_empty() {
        return Ok(Vec::new());
    }
    let ids = genre_ids.join("|");
    let labels = cached(
        pool,
        "wikidata_labels",
        &search_key(&format!("genre-labels {ids}")),
        Duration::days(90),
        || {
            request_json(client.get(API).query(&[
                ("action", "wbgetentities"),
                ("format", "json"),
                ("props", "labels"),
                ("languages", "en"),
                ("ids", ids.as_str()),
            ]))
        },
    )
    .await?;
    Ok(genre_ids
        .iter()
        .filter_map(|id| labels["entities"][*id]["labels"]["en"]["value"].as_str())
        .map(str::to_owned)
        .collect())
}
