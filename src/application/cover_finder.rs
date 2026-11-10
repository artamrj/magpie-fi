//! The cover finder. A song whose match is sure but which has no verified cover
//! waits in Review with that match kept as it is: correct tags matter more than
//! a cover, so the release is never changed just to get one. After each inbox
//! check this looks for the missing covers again, through the same cover worker
//! as filing (`artwork::ensure_usable_cover`). Catalogs and the library change
//! slowly, so one song is searched at most once every [`SEARCH_AGAIN_AFTER_HOURS`].
//! A song whose cover turns up becomes Ready and is filed with the next batch.

use crate::{
    app::AppState,
    application::{artwork, metadata_completion},
    infrastructure::{
        db::{candidates as candidate_db, tracks as track_db},
        providers::Candidate,
    },
    types::{CandidateId, WorkflowPhase},
};
use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use std::sync::Arc;

/// How long a song waits before its cover is searched again.
const SEARCH_AGAIN_AFTER_HOURS: i64 = 12;

/// Songs searched in one run, so a long list never holds up filing for long.
const MAX_SONGS_PER_RUN: usize = 25;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct CoverFinderResult {
    pub searched: usize,
    pub found: usize,
}

/// Looks for the covers of songs waiting for one. Takes the workflow slot, and
/// does nothing when another job holds it.
pub async fn run(state: &Arc<AppState>) -> Result<CoverFinderResult> {
    let waiting = waiting_songs(&state.pool).await?;
    let now = Utc::now();
    let due = waiting
        .into_iter()
        .filter(|(_, candidate)| search_due(candidate, now))
        .take(MAX_SONGS_PER_RUN)
        .collect::<Vec<_>>();
    if due.is_empty() {
        return Ok(CoverFinderResult::default());
    }
    if !state
        .try_claim_workflow(WorkflowPhase::Fetch, "Looking for missing covers")
        .await
    {
        return Ok(CoverFinderResult::default());
    }
    let result = search(state, due).await;
    state.reset_workflow(WorkflowPhase::Idle, "Ready").await;
    let result = result?;
    if result.searched > 0 {
        tracing::info!(
            searched = result.searched,
            found = result.found,
            "looked for missing covers"
        );
    }
    Ok(result)
}

async fn search(state: &Arc<AppState>, due: Vec<(i64, Candidate)>) -> Result<CoverFinderResult> {
    let mut result = CoverFinderResult::default();
    for (track_id, mut candidate) in due {
        if state.workflow_cancelled().await {
            break;
        }
        let Some(candidate_id) = candidate.id else {
            continue;
        };
        {
            let _permit = state.artwork_downloads.acquire().await?;
            artwork::ensure_usable_cover(&state.pool, &state.client, &mut candidate).await;
        }
        let report = metadata_completion::reassess(&mut candidate);
        result.searched += 1;
        let mut transaction = state.pool.begin().await?;
        // Only while the song still waits with this match: the owner may have
        // picked another one, or filed it, while the search ran.
        let still_waiting = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tracks WHERE id=? AND stage='review' AND selected_candidate_id=?",
        )
        .bind(track_id)
        .bind(candidate_id)
        .fetch_one(&mut *transaction)
        .await?
            > 0;
        if !still_waiting {
            continue;
        }
        candidate_db::update(&mut *transaction, &candidate).await?;
        if report.core_complete {
            result.found += sqlx::query(
                "UPDATE tracks SET status='selected',stage='ready',stage_message=?,updated_at=?
                 WHERE id=? AND stage='review' AND selected_candidate_id=?",
            )
            .bind(format!(
                "Cover found later by the cover finder; {}",
                report.summary()
            ))
            .bind(Utc::now().to_rfc3339())
            .bind(track_id)
            .bind(candidate_id)
            .execute(&mut *transaction)
            .await?
            .rows_affected() as usize;
        }
        transaction.commit().await?;
    }
    Ok(result)
}

/// Songs in Review whose chosen match only lacks a cover, oldest first.
async fn waiting_songs(pool: &sqlx::SqlitePool) -> Result<Vec<(i64, Candidate)>> {
    let rows: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT tracks.id,tracks.selected_candidate_id FROM tracks
         JOIN candidates ON candidates.id=tracks.selected_candidate_id
         WHERE tracks.stage='review' AND tracks.is_missing=0
           AND candidates.artwork_status IN ('cover_required','retryable_error')
         ORDER BY tracks.updated_at",
    )
    .fetch_all(pool)
    .await?;
    let mut out = Vec::new();
    for (track_id, candidate_id) in rows {
        if let Some(candidate) = track_db::candidate(pool, CandidateId(candidate_id)).await? {
            out.push((track_id, candidate));
        }
    }
    Ok(out)
}

/// Whether the last empty search (recorded by the cover worker) is long enough ago.
fn search_due(candidate: &Candidate, now: DateTime<Utc>) -> bool {
    candidate
        .score_breakdown
        .as_deref()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        .and_then(|value| {
            value["metadata_completion"]["cover_searched_at"]
                .as_str()
                .and_then(|at| DateTime::parse_from_rfc3339(at).ok())
        })
        .is_none_or(|last| {
            now - last.with_timezone(&Utc) >= Duration::hours(SEARCH_AGAIN_AFTER_HOURS)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::Config, infrastructure::providers::ArtworkCandidate};

    async fn offline_state() -> (tempfile::TempDir, Arc<AppState>) {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("cover-finder.sqlite");
        let pool = crate::infrastructure::db::connect(database.to_str().unwrap())
            .await
            .unwrap();
        let mut state = AppState::new(Config::default(), pool);
        // Tests never reach the internet: every request fails at once.
        state.client = reqwest::Client::builder()
            .proxy(reqwest::Proxy::all("http://127.0.0.1:9").unwrap())
            .build()
            .unwrap();
        (directory, Arc::new(state))
    }

    /// A song in Review whose chosen match has no cover, as the scan leaves it.
    async fn waiting_song(state: &AppState, artwork: Vec<ArtworkCandidate>) -> (i64, i64) {
        let track_id = sqlx::query(
            "INSERT INTO tracks(path,filename,status,is_missing,first_seen_at,last_seen_at,last_scanned_at,stage)
             VALUES('/music/inbox/marie.mp3','marie.mp3','needs_review',0,'now','now','now','review')",
        )
        .execute(&state.pool)
        .await
        .unwrap()
        .last_insert_rowid();
        let candidate = Candidate {
            provider: "musicbrainz".into(),
            title: "Marie".into(),
            artist: "AnnenMayKantereit".into(),
            album: Some("Schlagschatten".into()),
            artwork_candidates: artwork,
            artwork_status: crate::infrastructure::providers::ArtworkStatus::CoverRequired,
            score: 90.0,
            ..Default::default()
        };
        let candidate_id = candidate_db::insert(&state.pool, track_id, &candidate)
            .await
            .unwrap();
        sqlx::query("UPDATE tracks SET selected_candidate_id=? WHERE id=?")
            .bind(candidate_id)
            .bind(track_id)
            .execute(&state.pool)
            .await
            .unwrap();
        (track_id, candidate_id)
    }

    async fn stage(state: &AppState, track_id: i64) -> String {
        sqlx::query_scalar("SELECT stage FROM tracks WHERE id=?")
            .bind(track_id)
            .fetch_one(&state.pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn a_cover_found_later_makes_the_song_ready_with_its_match_unchanged() {
        let (_directory, state) = offline_state().await;
        let url = "https://example.test/schlagschatten.png";
        crate::infrastructure::provider_cache::put(
            &state.pool,
            "artwork-url",
            &crate::infrastructure::provider_cache::url_key(url),
            &serde_json::json!({"data_base64": base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                crate::infrastructure::media::tag_writer::test_artwork_png(),
            )}),
            Utc::now() + Duration::days(1),
        )
        .await
        .unwrap();
        let (track_id, candidate_id) = waiting_song(
            &state,
            vec![ArtworkCandidate {
                provider: "Deezer (album)".into(),
                url: url.into(),
                album: Some("Schlagschatten".into()),
                artist: Some("AnnenMayKantereit".into()),
                ..Default::default()
            }],
        )
        .await;

        let result = run(&state).await.unwrap();

        assert_eq!(
            result,
            CoverFinderResult {
                searched: 1,
                found: 1
            }
        );
        assert_eq!(stage(&state, track_id).await, "ready");
        let candidate = track_db::candidate(&state.pool, CandidateId(candidate_id))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(candidate.album.as_deref(), Some("Schlagschatten"));
        assert_eq!(candidate.cover_url.as_deref(), Some(url));
        // The slot is free again for filing.
        assert!(!state.workflow_running().await);
    }

    #[tokio::test]
    async fn a_song_without_a_cover_keeps_waiting_and_is_not_searched_again_at_once() {
        let (_directory, state) = offline_state().await;
        let (track_id, _) = waiting_song(&state, Vec::new()).await;

        let first = run(&state).await.unwrap();
        assert_eq!(
            first,
            CoverFinderResult {
                searched: 1,
                found: 0
            }
        );
        assert_eq!(stage(&state, track_id).await, "review");

        let again = run(&state).await.unwrap();
        assert_eq!(again, CoverFinderResult::default());
    }

    #[test]
    fn a_search_is_due_twelve_hours_after_the_last_empty_one() {
        let now = Utc::now();
        let searched = |hours_ago: i64| Candidate {
            score_breakdown: Some(
                serde_json::json!({"metadata_completion": {
                    "cover_searched_at": (now - Duration::hours(hours_ago)).to_rfc3339()
                }})
                .to_string(),
            ),
            ..Default::default()
        };
        assert!(search_due(&Candidate::default(), now));
        assert!(!search_due(&searched(1), now));
        assert!(search_due(&searched(13), now));
    }
}
