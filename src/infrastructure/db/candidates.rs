//! Writes of the `candidates` table, in one place so the column list exists once.

use crate::infrastructure::providers::Candidate;
use anyhow::Result;
use sqlx::SqliteExecutor;

/// Stores a new candidate for a track and returns its id.
pub async fn insert(
    executor: impl SqliteExecutor<'_>,
    track_id: i64,
    candidate: &Candidate,
) -> Result<i64> {
    let provider = if candidate.provider.is_empty() {
        "musicbrainz"
    } else {
        candidate.provider.as_str()
    };
    Ok(sqlx::query(
        "INSERT INTO candidates(track_id,provider,title,artist,artist_credits_json,album,
           album_artist,album_artist_credits_json,track_number,track_total,disc_number,disc_total,
           year,genre,composer,label,isrc,cover_url,artwork_candidates_json,artwork_status,
           artwork_message,musicbrainz_recording_id,musicbrainz_release_id,release_country,
           release_date,release_type,release_secondary_types,is_compilation,duration_delta,
           score_breakdown,musicbrainz_artist_id,musicbrainz_album_artist_id,score,raw_json,
           musicbrainz_release_group_id,musicbrainz_release_track_id,original_date,artist_sort,
           album_artist_sort)
         VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
    )
    .bind(track_id)
    .bind(provider)
    .bind(&candidate.title)
    .bind(&candidate.artist)
    .bind(serde_json::to_string(&candidate.artist_credits)?)
    .bind(&candidate.album)
    .bind(&candidate.album_artist)
    .bind(serde_json::to_string(&candidate.album_artist_credits)?)
    .bind(candidate.track_number)
    .bind(candidate.track_total)
    .bind(candidate.disc_number)
    .bind(candidate.disc_total)
    .bind(&candidate.year)
    .bind(&candidate.genre)
    .bind(&candidate.composer)
    .bind(&candidate.label)
    .bind(&candidate.isrc)
    .bind(&candidate.cover_url)
    .bind(serde_json::to_string(&candidate.artwork_candidates)?)
    .bind(candidate.artwork_status)
    .bind(&candidate.artwork_message)
    .bind(&candidate.recording_id)
    .bind(&candidate.release_id)
    .bind(&candidate.release_country)
    .bind(&candidate.release_date)
    .bind(&candidate.release_type)
    .bind(&candidate.release_secondary_types)
    .bind(candidate.is_compilation)
    .bind(candidate.duration_delta)
    .bind(&candidate.score_breakdown)
    .bind(&candidate.artist_id)
    .bind(&candidate.album_artist_id)
    .bind(candidate.score)
    .bind(&candidate.raw_json)
    .bind(&candidate.release_group_id)
    .bind(&candidate.release_track_id)
    .bind(&candidate.original_date)
    .bind(&candidate.artist_sort)
    .bind(&candidate.album_artist_sort)
    .execute(executor)
    .await?
    .last_insert_rowid())
}

/// Saves the completed metadata of an existing candidate (its identity,
/// provider ids and raw response stay as they were).
pub async fn update(executor: impl SqliteExecutor<'_>, candidate: &Candidate) -> Result<()> {
    let id = candidate
        .id
        .ok_or_else(|| anyhow::anyhow!("candidate has no database id"))?;
    sqlx::query(
        "UPDATE candidates SET title=?,artist=?,artist_credits_json=?,album=?,album_artist=?,
         album_artist_credits_json=?,track_number=?,track_total=?,disc_number=?,disc_total=?,year=?,
         genre=?,composer=?,label=?,isrc=?,cover_url=?,artwork_candidates_json=?,artwork_status=?,
         artwork_message=?,release_country=?,release_date=?,release_type=?,release_secondary_types=?,
         is_compilation=?,score_breakdown=?,original_date=?,artist_sort=?,album_artist_sort=?
         WHERE id=?",
    )
    .bind(&candidate.title)
    .bind(&candidate.artist)
    .bind(serde_json::to_string(&candidate.artist_credits)?)
    .bind(&candidate.album)
    .bind(&candidate.album_artist)
    .bind(serde_json::to_string(&candidate.album_artist_credits)?)
    .bind(candidate.track_number)
    .bind(candidate.track_total)
    .bind(candidate.disc_number)
    .bind(candidate.disc_total)
    .bind(&candidate.year)
    .bind(&candidate.genre)
    .bind(&candidate.composer)
    .bind(&candidate.label)
    .bind(&candidate.isrc)
    .bind(&candidate.cover_url)
    .bind(serde_json::to_string(&candidate.artwork_candidates)?)
    .bind(candidate.artwork_status)
    .bind(&candidate.artwork_message)
    .bind(&candidate.release_country)
    .bind(&candidate.release_date)
    .bind(&candidate.release_type)
    .bind(&candidate.release_secondary_types)
    .bind(candidate.is_compilation)
    .bind(&candidate.score_breakdown)
    .bind(&candidate.original_date)
    .bind(&candidate.artist_sort)
    .bind(&candidate.album_artist_sort)
    .bind(id)
    .execute(executor)
    .await?;
    Ok(())
}
