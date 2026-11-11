use super::*;
use crate::application::{
    Donors, FileTags, finalize_selection, metadata_completion::CompletionReport,
};
use crate::infrastructure::{
    db::candidates as candidate_db,
    providers::{self, ArtworkCandidate, ArtworkStatus},
};
use crate::types::TrackStage;
use axum::http::{HeaderMap, StatusCode, header};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

/// How many songs of each section `GET /api/tracks` sends unless asked for more.
const DEFAULT_SECTION_ROWS: usize = 50;
const MAX_SECTION_ROWS: usize = 1000;

#[derive(Deserialize, Default)]
pub struct TrackListQuery {
    /// Words to look for in the file name, title, artist and album.
    #[serde(default)]
    q: String,
    /// Songs per section.
    limit: Option<usize>,
}

/// How many songs wait in each section of Review, whatever is searched for.
#[derive(Default, Serialize)]
pub struct TrackCounts {
    review: usize,
    /// Damaged, missing or failed songs.
    issues: usize,
    /// Duplicate uploads that were skipped (listed under Issues).
    skipped: usize,
    working: usize,
    ready: usize,
    /// Review songs whose first match is strong enough to accept without a look.
    confident: usize,
    damaged: usize,
}

/// One row of the Review list: the song, the match that was chosen for it (if
/// any) and what the list needs to know about the others.
#[derive(Serialize)]
pub struct TrackRow {
    #[serde(flatten)]
    track: Track,
    /// Only the chosen match. All of them come with `GET /api/tracks/{id}`.
    candidates: Vec<Candidate>,
    candidate_count: i64,
    /// `review`, `issues`, `working` or `ready`.
    group: &'static str,
    confident: bool,
}

#[derive(Serialize)]
pub struct TrackList {
    counts: TrackCounts,
    /// Songs per section that fit the search (the rows below are cut at `limit`).
    matching: TrackCounts,
    /// The songs that wait for the owner, in list order: Review first, then
    /// Issues. Ids only, for "next song" and "song 3 of 40".
    order: Vec<TrackId>,
    confident_titles: Vec<String>,
    tracks: Vec<TrackRow>,
}

/// `GET /api/tracks?q=&limit=`: the Review list.
///
/// The page asks for this every few seconds. A first import can leave more
/// than a thousand songs waiting, each with up to twenty matches, so the list
/// carries the counts of every section but only the first `limit` songs of
/// each, and of their matches only the chosen one. That keeps an answer at a
/// few dozen kilobytes however long the queue is; the matches of one song are
/// fetched when it is opened.
pub async fn list_tracks(
    State(s): State<Arc<AppState>>,
    axum::extract::Query(query): axum::extract::Query<TrackListQuery>,
) -> ApiResult<Json<TrackList>> {
    let limit = query
        .limit
        .unwrap_or(DEFAULT_SECTION_ROWS)
        .clamp(1, MAX_SECTION_ROWS);
    let words = query
        .q
        .split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>();

    let tracks: Vec<Track> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {} FROM tracks ORDER BY path",
        track_db::TRACK_FIELDS
    )))
    .fetch_all(&s.pool)
    .await?;
    // Per song: how many matches, the best score and the second best.
    let scores: Vec<(i64, i64, f64, Option<f64>)> = sqlx::query_as(
        "SELECT track_id, COUNT(*), MAX(score), MAX(CASE WHEN place=2 THEN score END)
         FROM (SELECT track_id, score,
                 ROW_NUMBER() OVER (PARTITION BY track_id ORDER BY score DESC, id) AS place
               FROM candidates)
         GROUP BY track_id",
    )
    .fetch_all(&s.pool)
    .await?;
    let scores = scores
        .into_iter()
        .map(|(track, count, best, second)| (track, (count, best, second)))
        .collect::<std::collections::HashMap<_, _>>();
    // The chosen match of every song, as far as searching and sorting need it.
    type Chosen = (i64, Option<String>, Option<String>, Option<String>, String);
    let chosen: Vec<Chosen> = sqlx::query_as(
        "SELECT tracks.id, candidates.title, candidates.artist, candidates.album,
                candidates.artwork_status
         FROM tracks JOIN candidates ON candidates.id=tracks.selected_candidate_id",
    )
    .fetch_all(&s.pool)
    .await?;
    let chosen = chosen
        .into_iter()
        .map(|(track, title, artist, album, artwork)| (track, (title, artist, album, artwork)))
        .collect::<std::collections::HashMap<_, _>>();

    let mut list = TrackList {
        counts: TrackCounts::default(),
        matching: TrackCounts::default(),
        order: Vec::new(),
        confident_titles: Vec::new(),
        tracks: Vec::new(),
    };
    let mut sections: [Vec<(Track, i64, bool)>; 4] = Default::default();
    let mut issue_order = Vec::new();
    for track in tracks {
        let skipped = track.stage == TrackStage::Skipped;
        let problem = !skipped
            && (track.is_missing
                || track.stage == TrackStage::Failed
                || matches!(
                    track.status.as_str(),
                    "corrupt" | "failed" | "provider_error"
                ));
        let section = if problem || skipped {
            1
        } else if track.stage == TrackStage::Review {
            0
        } else if track.stage == TrackStage::Ready && track.selected_candidate_id.is_some() {
            3
        } else {
            2
        };
        let own = chosen.get(&track.id.0);
        let (count, best, second) = scores.get(&track.id.0).copied().unwrap_or((0, 0.0, None));
        let confident = section == 0
            && count > 0
            && best >= 85.0
            && second.is_none_or(|second| best - second >= 8.0)
            && !track.stage_message.as_deref().is_some_and(|message| {
                message.starts_with(crate::application::apply::POSSIBLE_DUPLICATE)
            })
            && own.is_none_or(|(.., artwork)| artwork != "retryable_error");
        let title = own
            .and_then(|(title, ..)| title.clone())
            .or_else(|| track.current_title.clone())
            .unwrap_or_else(|| track.filename.clone());

        for (counts, counted) in [(&mut list.counts, true), (&mut list.matching, false)] {
            if !counted {
                let text = [
                    Some(track.filename.as_str()),
                    track.current_title.as_deref(),
                    track.current_artist.as_deref(),
                    track.current_album.as_deref(),
                    own.and_then(|(title, ..)| title.as_deref()),
                    own.and_then(|(_, artist, ..)| artist.as_deref()),
                    own.and_then(|(_, _, album, _)| album.as_deref()),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase();
                if !words.iter().all(|word| text.contains(word.as_str())) {
                    break;
                }
            }
            match section {
                0 => counts.review += 1,
                1 if skipped => counts.skipped += 1,
                1 => counts.issues += 1,
                2 => counts.working += 1,
                _ => counts.ready += 1,
            }
            counts.confident += usize::from(confident);
            counts.damaged += usize::from(track.status == "corrupt");
            if counted {
                continue;
            }
            // This song fits the search.
            if confident && list.confident_titles.len() < 3 {
                list.confident_titles.push(title.clone());
            }
            match section {
                0 => list.order.push(track.id),
                1 if !skipped => issue_order.push(track.id),
                _ => {}
            }
            if sections[section].len() < limit {
                sections[section].push((track.clone(), count, confident));
            }
        }
    }
    list.order.extend(issue_order);

    let shown = sections.iter().flatten().map(|(track, ..)| track.id);
    let mut matches = track_db::selected_candidates(&s.pool, shown).await?;
    for (section, group) in sections
        .into_iter()
        .zip(["review", "issues", "working", "ready"])
    {
        for (track, candidate_count, confident) in section {
            list.tracks.push(TrackRow {
                candidates: matches.remove(&track.id).into_iter().collect(),
                track,
                candidate_count,
                group,
                confident,
            });
        }
    }
    Ok(Json(list))
}

pub async fn remove_track(
    State(s): State<Arc<AppState>>,
    Path(id): Path<TrackId>,
) -> ApiResult<Json<serde_json::Value>> {
    if s.workflow_running().await {
        return Err(ApiError::conflict(
            "Wait for the current scan or write operation to finish",
        ));
    }

    let track = track_db::track(&s.pool, id).await?;
    // A file you already deleted yourself leaves nothing to move to the trash:
    // the song is only cleared from the list, whatever state it was in.
    let source = match tokio::fs::canonicalize(&track.path).await {
        Ok(source) => Some(source),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(ApiError::Internal(error.into())),
    };
    if let Some(source) = &source {
        if track.stage != TrackStage::Review || track.selected_candidate_id.is_some() {
            return Err(ApiError::validation(
                "Only unresolved tracks in Review can be removed",
            ));
        }
        let cfg = &s.config;
        let input_dir = tokio::fs::canonicalize(&cfg.input_dir)
            .await
            .map_err(|error| anyhow::anyhow!("could not verify the music folder: {error}"))?;
        let in_inbox = *source != input_dir && source.starts_with(&input_dir);
        if !in_inbox && !source.starts_with(&cfg.staging_dir) {
            return Err(ApiError::validation(
                "Refusing to remove a file outside the inbox",
            ));
        }
        if !tokio::fs::metadata(source).await?.is_file() {
            return Err(ApiError::validation(
                "The selected music path is not a regular file",
            ));
        }

        // Your own files go to the trash; magpie-fi's downloads are simply deleted.
        crate::application::library::discard_original(
            &cfg.trash_dir,
            &cfg.staging_dir,
            &input_dir,
            source,
        )
        .await
        .map_err(|error| {
            ApiError::Internal(anyhow::anyhow!("could not remove music file: {error}"))
        })?;
    }
    let source = source.unwrap_or_else(|| PathBuf::from(&track.path));
    crate::application::scan::forget_track(&s.pool, &track, &source).await?;

    tracing::info!(
        file = track.filename,
        source = %source.display(),
        "removed music file from review"
    );

    Ok(Json(serde_json::json!({
        "removed": true,
        "filename": track.filename,
    })))
}

/// `GET /api/tracks/{id}`: one song with every match found for it.
pub async fn get_track(
    State(s): State<Arc<AppState>>,
    Path(id): Path<TrackId>,
) -> ApiResult<Json<WorkspaceTrack>> {
    let track = track_db::track(&s.pool, id).await?;
    let candidates = track_db::candidates(&s.pool, id).await?;
    Ok(Json(WorkspaceTrack { track, candidates }))
}

/// Picks matches for review tracks where the evidence is strong enough. Holds
/// the workflow slot, because finishing each match downloads covers.
pub async fn auto_approve_review(
    State(s): State<Arc<AppState>>,
) -> ApiResult<Json<AutoApproveResult>> {
    let state = s.clone();
    in_workflow(
        &s,
        WorkflowPhase::Fetch,
        "Approving matches in review",
        async move { approve_review_tracks(&state).await },
    )
    .await?
    .map(Json)
}

async fn approve_review_tracks(s: &AppState) -> ApiResult<AutoApproveResult> {
    let tracks: Vec<Track> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {} FROM tracks WHERE stage='review' AND selected_candidate_id IS NULL ORDER BY id",
        track_db::TRACK_FIELDS
    )))
    .fetch_all(&s.pool)
    .await?;
    let mut candidates_by_track =
        track_db::candidates_by_track(&s.pool, tracks.iter().map(|track| track.id)).await?;

    let mut result = AutoApproveResult {
        approved: 0,
        low_confidence: 0,
        unavailable: 0,
    };
    for track in tracks {
        if s.workflow_cancelled().await {
            break;
        }
        if track.status == "corrupt" || track.is_missing {
            result.unavailable += 1;
            continue;
        }
        let candidates = candidates_by_track.remove(&track.id).unwrap_or_default();
        if candidates.is_empty() {
            result.unavailable += 1;
            continue;
        }
        let evidence = crate::application::smart_approval::TrackEvidence {
            filename: &track.filename,
            title: track.current_title.as_deref(),
            artist: track.current_artist.as_deref(),
            album: track.current_album.as_deref(),
            // Not stored for songs already in Review; a new check reads it from the file.
            isrc: None,
        };
        let Some(decision) = crate::application::smart_approval::select(evidence, &candidates)
        else {
            result.low_confidence += 1;
            continue;
        };
        let Some(mut selected) = candidates
            .iter()
            .find(|candidate| candidate.id == Some(decision.candidate_id))
            .cloned()
        else {
            result.unavailable += 1;
            continue;
        };
        let completion = finalize_selection(
            s,
            &mut selected,
            Some(Donors {
                candidates: &candidates,
                existing_album: track.current_album.as_deref(),
            }),
            Some(FileTags {
                artist: track.current_artist.as_deref(),
                title: track.current_title.as_deref(),
            }),
        )
        .await?;
        if !completion.core_complete {
            result.low_confidence += 1;
            continue;
        }
        let message = format!(
            "Smart auto-approved ({:.0}%): {}; {}",
            decision.confidence,
            decision.explanation,
            completion.summary()
        );
        let mut transaction = s.pool.begin().await?;
        candidate_db::update(&mut *transaction, &selected).await?;
        result.approved += sqlx::query(
            "UPDATE tracks
             SET selected_candidate_id=?,status='selected',stage='ready',stage_message=?,updated_at=?
             WHERE id=? AND stage='review' AND selected_candidate_id IS NULL",
        )
        .bind(decision.candidate_id)
        .bind(message)
        .bind(chrono::Utc::now().to_rfc3339())
        .bind(track.id.0)
        .execute(&mut *transaction)
        .await?
        .rows_affected();
        transaction.commit().await?;
    }
    Ok(result)
}

pub async fn track_audio(
    State(s): State<Arc<AppState>>,
    Path(id): Path<TrackId>,
    headers: HeaderMap,
) -> ApiResult<axum::response::Response> {
    let track = track_db::track(&s.pool, id).await?;
    stream_audio(
        std::path::Path::new(&track.path),
        audio_mime(track.format.as_deref().unwrap_or_default()),
        &headers,
    )
    .await
}

/// Streams an audio file with byte ranges, so the player can seek anywhere.
pub(super) async fn stream_audio(
    path: &std::path::Path,
    mime: &'static str,
    headers: &HeaderMap,
) -> ApiResult<axum::response::Response> {
    let metadata = tokio::fs::metadata(path).await.map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            ApiError::not_found("audio source file no longer exists")
        } else {
            ApiError::from(error)
        }
    })?;
    let total = metadata.len();
    if total == 0 {
        return Err(ApiError::validation("audio source file is empty"));
    }
    let requested = match parse_byte_range(
        headers
            .get(header::RANGE)
            .and_then(|value| value.to_str().ok()),
        total,
    ) {
        Ok(range) => range,
        Err(()) => {
            return Ok(axum::response::Response::builder()
                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                .header(header::CONTENT_RANGE, format!("bytes */{total}"))
                .body(axum::body::Body::empty())
                .map_err(anyhow::Error::from)?);
        }
    };
    let (start, end, status) = requested.map_or((0, total - 1, StatusCode::OK), |(start, end)| {
        (start, end, StatusCode::PARTIAL_CONTENT)
    });
    let length = end - start + 1;
    let mut file = tokio::fs::File::open(path).await?;
    file.seek(std::io::SeekFrom::Start(start)).await?;
    let body = axum::body::Body::from_stream(tokio_util::io::ReaderStream::new(file.take(length)));

    let mut response = axum::response::Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, mime)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, length.to_string())
        .header(header::CACHE_CONTROL, "private, no-store");
    if status == StatusCode::PARTIAL_CONTENT {
        response = response.header(
            header::CONTENT_RANGE,
            format!("bytes {start}-{end}/{total}"),
        );
    }
    Ok(response.body(body).map_err(anyhow::Error::from)?)
}
fn parse_byte_range(
    value: Option<&str>,
    total: u64,
) -> std::result::Result<Option<(u64, u64)>, ()> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.strip_prefix("bytes=").ok_or(())?;
    if value.contains(',') {
        return Err(());
    }
    let (start, end) = value.split_once('-').ok_or(())?;
    if start.is_empty() {
        let suffix = end.parse::<u64>().map_err(|_| ())?;
        if suffix == 0 {
            return Err(());
        }
        return Ok(Some((total.saturating_sub(suffix), total - 1)));
    }
    let start = start.parse::<u64>().map_err(|_| ())?;
    if start >= total {
        return Err(());
    }
    let end = if end.is_empty() {
        total - 1
    } else {
        end.parse::<u64>().map_err(|_| ())?.min(total - 1)
    };
    if end < start {
        return Err(());
    }
    Ok(Some((start, end)))
}

pub(super) fn audio_mime(format: &str) -> &'static str {
    match format {
        "mp3" => "audio/mpeg",
        "m4a" | "mp4" | "aac" => "audio/mp4",
        "flac" => "audio/flac",
        "ogg" => "audio/ogg",
        "opus" => "audio/ogg; codecs=opus",
        "wav" => "audio/wav",
        "aiff" | "aif" => "audio/aiff",
        _ => "application/octet-stream",
    }
}

pub async fn select_candidate(
    State(s): State<Arc<AppState>>,
    Path(id): Path<TrackId>,
    Json(body): Json<SelectRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let candidate_id = body
        .candidate_id
        .ok_or_else(|| ApiError::validation("candidate is required"))?;
    let track = track_db::track(&s.pool, id).await?;
    let candidates = track_db::candidates(&s.pool, id).await?;
    let Some(mut selected) = candidates
        .iter()
        .find(|candidate| candidate.id == Some(candidate_id.0))
        .cloned()
    else {
        return Err(ApiError::not_found("candidate not found for this track"));
    };
    let completion = finalize_selection(
        &s,
        &mut selected,
        Some(Donors {
            candidates: &candidates,
            existing_album: track.current_album.as_deref(),
        }),
        Some(FileTags {
            artist: track.current_artist.as_deref(),
            title: track.current_title.as_deref(),
        }),
    )
    .await?;
    let mut transaction = s.pool.begin().await?;
    candidate_db::update(&mut *transaction, &selected).await?;
    let (status, stage, message) = readiness_state(&selected, &completion, "Selected by you");
    sqlx::query("UPDATE tracks SET selected_candidate_id=?,status=?,stage=?,stage_message=?,updated_at=? WHERE id=?")
        .bind(candidate_id.0)
        .bind(status)
        .bind(stage)
        .bind(message)
        .bind(chrono::Utc::now().to_rfc3339())
        .bind(id.0)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    let cover_verified = selected.artwork_status == ArtworkStatus::Verified;
    Ok(Json(
        serde_json::json!({"selected": true, "ready": stage == "ready", "cover_verified": cover_verified}),
    ))
}

/// A track becomes Ready only when the whole core set is complete, which
/// includes a verified matching cover. Tracks without usable artwork stay in
/// review so a file is never written missing, broken, or wrongly matched art.
fn readiness_state(
    candidate: &Candidate,
    completion: &CompletionReport,
    prefix: &str,
) -> (&'static str, &'static str, String) {
    if completion.core_complete {
        return (
            "selected",
            "ready",
            format!("{prefix}; {}", completion.summary()),
        );
    }
    let detail = candidate
        .artwork_message
        .clone()
        .unwrap_or_else(|| completion.summary());
    ("needs_review", "review", format!("{prefix}; {detail}"))
}

pub async fn return_to_review(
    State(s): State<Arc<AppState>>,
    Path(id): Path<TrackId>,
) -> ApiResult<Json<serde_json::Value>> {
    if s.workflow_running().await {
        return Err(ApiError::conflict(
            "Wait for the current scan or write operation to finish",
        ));
    }
    let track = track_db::track(&s.pool, id).await?;
    if track.selected_candidate_id.is_none() {
        return Err(ApiError::validation(
            "This track does not have an identification to undo",
        ));
    }
    let message = if track.output_path.is_some() {
        "Returned to review; the existing corrected output was not removed"
    } else {
        "Returned to review"
    };
    let changed = sqlx::query(
        "UPDATE tracks
         SET selected_candidate_id=NULL,status='needs_review',stage='review',stage_message=?,updated_at=?
         WHERE id=? AND selected_candidate_id IS NOT NULL",
    )
    .bind(message)
    .bind(chrono::Utc::now().to_rfc3339())
    .bind(id.0)
    .execute(&s.pool)
    .await?
    .rows_affected();
    if changed == 0 {
        return Err(ApiError::conflict(
            "The track identification was already changed",
        ));
    }
    Ok(Json(serde_json::json!({"returned_to_review": true})))
}

/// `POST /api/tracks/{id}/file-anyway`: the owner checked a possible duplicate
/// and wants this song filed too.
pub async fn file_anyway(
    State(s): State<Arc<AppState>>,
    Path(id): Path<TrackId>,
) -> ApiResult<Json<serde_json::Value>> {
    if s.workflow_running().await {
        return Err(ApiError::conflict(
            "Wait for the current scan or write operation to finish",
        ));
    }
    let changed = sqlx::query(
        "UPDATE tracks SET allow_duplicate=1,status='selected',stage='ready',
           stage_message='Will be filed although a similar song is in your library',updated_at=?
         WHERE id=? AND selected_candidate_id IS NOT NULL AND stage='review'",
    )
    .bind(chrono::Utc::now().to_rfc3339())
    .bind(id.0)
    .execute(&s.pool)
    .await?
    .rows_affected();
    if changed == 0 {
        return Err(ApiError::validation(
            "Only a chosen song waiting in Review can be filed anyway",
        ));
    }
    s.notify_automation_scheduler();
    Ok(Json(serde_json::json!({"ready": true})))
}

pub async fn manual_candidate(
    State(s): State<Arc<AppState>>,
    Path(id): Path<TrackId>,
    Json(value): Json<CandidateEdit>,
) -> ApiResult<Json<serde_json::Value>> {
    save_own_tags(&s, id, value, "Entered manually").await
}

/// `POST /api/tracks/{id}/keep-tags`: none of the matches is right and the
/// file's own tags are. They become the chosen details, as if typed by hand,
/// so the song still needs a verified cover before it is filed.
pub async fn keep_file_tags(
    State(s): State<Arc<AppState>>,
    Path(id): Path<TrackId>,
) -> ApiResult<Json<serde_json::Value>> {
    let track = track_db::track(&s.pool, id).await?;
    let (Some(title), Some(artist)) = (
        track.current_title.clone().filter(|v| !v.trim().is_empty()),
        track
            .current_artist
            .clone()
            .filter(|v| !v.trim().is_empty()),
    ) else {
        return Err(ApiError::validation(
            "The file has no title or artist of its own; edit the tags instead",
        ));
    };
    // Title, artist and album are the ones the page shows. The rest is read
    // from the file so a year or genre it already has is not lost.
    let path = std::path::PathBuf::from(&track.path);
    let rest = tokio::task::spawn_blocking(move || tag_writer::read_details(&path))
        .await
        .map_err(anyhow::Error::from)?
        .unwrap_or_default();
    let edit = CandidateEdit {
        title,
        artist,
        artist_credits: Vec::new(),
        album: track.current_album.clone(),
        album_artist: track.current_album_artist.clone(),
        album_artist_credits: Vec::new(),
        track_number: track.current_track_number,
        track_total: rest.track_total.map(i64::from),
        disc_number: rest.disc.map(i64::from),
        disc_total: rest.disc_total.map(i64::from),
        year: rest.year,
        genre: rest.genre,
        composer: rest.composer,
        label: rest.label,
        isrc: rest.isrc,
        release_date: rest.release_date,
        cover_url: None,
    };
    // You chose the file over every match, so its own picture may be its
    // cover, but only one that looks like a real cover: see `check_own_cover`.
    let own_cover = providers::cover_art_archive::file_cover_url(&track.path);
    let refused = providers::cover_art_archive::fetch_verified_url_cached(
        &s.pool, &s.client, &own_cover, false,
    )
    .await
    .err()
    .map(|error| format!("{error:#}"));
    let edit = CandidateEdit {
        cover_url: refused.is_none().then_some(own_cover),
        ..edit
    };
    let Json(mut response) = save_own_tags(&s, id, edit, "Kept the file's own tags").await?;
    // Why the file's own picture was not used, for the page to say.
    response["own_cover_refused"] = serde_json::json!(refused);
    Ok(Json(response))
}

/// Saves details that come from you and not from a catalog as the chosen
/// `manual` candidate. `prefix` starts the status line shown on the song.
async fn save_own_tags(
    s: &Arc<AppState>,
    id: TrackId,
    value: CandidateEdit,
    prefix: &str,
) -> ApiResult<Json<serde_json::Value>> {
    if value.title.trim().is_empty() || value.artist.trim().is_empty() {
        return Err(ApiError::validation("Title and artist are required"));
    }
    let track: Option<(String, String)> =
        sqlx::query_as("SELECT status,path FROM tracks WHERE id=?")
            .bind(id.0)
            .fetch_optional(&s.pool)
            .await?;
    let Some((status, track_path)) = track else {
        return Err(ApiError::not_found("track not found"));
    };
    if status == "corrupt" {
        return Err(ApiError::validation(
            "Damaged audio cannot be marked ready; repair or replace the source file first",
        ));
    }
    // A file's own picture can only be the cover of that same file.
    let own_cover = providers::cover_art_archive::file_cover_url(&track_path);
    if value
        .cover_url
        .as_deref()
        .map(str::trim)
        .is_some_and(|url| providers::cover_art_archive::is_file_cover_url(url) && url != own_cover)
    {
        return Err(ApiError::validation("That cover belongs to another file"));
    }
    // The page sends the Artist line and one chip per artist. Chips that name other
    // people than the line are stale, and would silently undo what you typed.
    use crate::domain::credits::names_same_people;
    let artist_credits = if names_same_people(&value.artist, &value.title, &value.artist_credits) {
        value.artist_credits
    } else {
        Vec::new()
    };
    let album_artist_credits = match value.album_artist.as_deref() {
        Some(album_artist) if names_same_people(album_artist, "", &value.album_artist_credits) => {
            value.album_artist_credits
        }
        _ => Vec::new(),
    };
    let mut candidate = Candidate {
        provider: "manual".into(),
        title: value.title,
        artist: value.artist,
        artist_credits,
        album: value.album,
        album_artist: value.album_artist,
        album_artist_credits,
        track_number: value.track_number,
        track_total: value.track_total,
        disc_number: value.disc_number,
        disc_total: value.disc_total,
        year: value.year,
        genre: value.genre,
        composer: value.composer,
        label: value.label,
        isrc: value.isrc,
        release_date: value.release_date,
        score: 100.0,
        raw_json: "{}".into(),
        ..Default::default()
    };
    crate::application::canonical_names::normalize_credits(&mut candidate);
    crate::application::metadata_completion::normalize_release_fields(&mut candidate);
    if let Some(url) = value
        .cover_url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
    {
        candidate.cover_url = Some(url.to_owned());
        candidate.artwork_candidates = vec![ArtworkCandidate {
            provider: "User verified".into(),
            url: url.to_owned(),
            user_confirmed: true,
            isrc: candidate.isrc.clone(),
            album: candidate.album.clone(),
            artist: Some(candidate.artist.clone()),
            ..Default::default()
        }];
    }
    let completion = finalize_selection(s, &mut candidate, None, None).await?;
    let candidate_id = candidate_db::insert(&s.pool, id.0, &candidate).await?;
    // A cover choice is remembered for the next scan, but not the file's own
    // picture: that one is already in the file.
    if completion.core_complete
        && let Some(cover_url) = candidate.cover_url.as_deref()
        && cover_url != own_cover
    {
        persist_artwork_override(
            &s.pool,
            &track_path,
            &candidate.title,
            &candidate.artist,
            cover_url,
        )
        .await?;
    }
    let (status, stage, message) = readiness_state(&candidate, &completion, prefix);
    sqlx::query(
        "UPDATE tracks SET selected_candidate_id=?,status=?,stage=?,stage_message=? WHERE id=?",
    )
    .bind(candidate_id)
    .bind(status)
    .bind(stage)
    .bind(message)
    .bind(id.0)
    .execute(&s.pool)
    .await?;
    let cover_verified = candidate.artwork_status == ArtworkStatus::Verified;
    Ok(Json(
        serde_json::json!({"selected": true, "ready": stage == "ready", "cover_verified": cover_verified, "candidate_id": candidate_id}),
    ))
}

pub async fn update_artwork(
    State(s): State<Arc<AppState>>,
    Path(id): Path<TrackId>,
    Json(value): Json<ArtworkEdit>,
) -> ApiResult<Json<serde_json::Value>> {
    let (track, mut selected) = track_db::selected(&s.pool, id).await?;
    let supplied = value.cover_url.trim();
    let parsed = reqwest::Url::parse(supplied).map_err(|_| {
        ApiError::validation(
            "Enter a valid HTTPS image, or a Spotify, Apple Music, Deezer, Shazam, SoundCloud, Audiomack, Navahang, Radio Javan, or Genius song URL",
        )
    })?;
    // The one address that is not a web link: this file's own picture.
    let own_cover = providers::cover_art_archive::file_cover_url(&track.path);
    if parsed.scheme() != "https" && supplied != own_cover {
        return Err(ApiError::validation("Cover URLs must use HTTPS"));
    }
    // A song page link stands for that song's cover.
    let cover_url = match lookup_source(&s, supplied, &parsed).await {
        Some((site, found)) => found
            .map_err(|error| ApiError::validation(format!("{site} link failed: {error:#}")))?
            .cover_url
            .ok_or_else(|| ApiError::validation(format!("{site} did not return cover artwork")))?,
        None => supplied.to_owned(),
    };
    let (_, info) = providers::cover_art_archive::fetch_verified_url_cached(
        &s.pool, &s.client, &cover_url, true,
    )
    .await
    .map_err(|error| ApiError::validation(format!("Cover download failed: {error:#}")))?;
    selected.cover_url = Some(cover_url.clone());
    selected.artwork_candidates = vec![ArtworkCandidate {
        provider: "User verified".into(),
        url: cover_url.clone(),
        user_confirmed: true,
        own_record: false,
        same_recording: false,
        release_id: selected.release_id.clone(),
        isrc: selected.isrc.clone(),
        album: selected.album.clone(),
        artist: Some(selected.artist.clone()),
    }];
    selected.artwork_status = ArtworkStatus::Verified;
    let changed = sqlx::query(
        "UPDATE candidates SET cover_url=?,artwork_candidates_json=?,artwork_status='verified',
         artwork_message=? WHERE id=(SELECT selected_candidate_id FROM tracks WHERE id=?)",
    )
    .bind(&cover_url)
    .bind(serde_json::to_string(&selected.artwork_candidates)?)
    .bind(format!(
        "Verified {}x{} {} cover supplied by you",
        info.width, info.height, info.mime
    ))
    .bind(id.0)
    .execute(&s.pool)
    .await?
    .rows_affected();
    if changed == 0 {
        return Err(ApiError::not_found(
            "Select metadata for this track before setting its cover",
        ));
    }
    if cover_url != own_cover {
        persist_artwork_override(
            &s.pool,
            &track.path,
            &selected.title,
            &selected.artist,
            &cover_url,
        )
        .await?;
    }
    selected.artwork_message = Some("Verified cover supplied by you".into());
    let completion = crate::application::metadata_completion::reassess(&mut selected);
    let (status, stage, message) = readiness_state(&selected, &completion, "Artwork updated");
    sqlx::query("UPDATE tracks SET status=?,stage=?,stage_message=?,updated_at=? WHERE id=?")
        .bind(status)
        .bind(stage)
        .bind(message)
        .bind(chrono::Utc::now().to_rfc3339())
        .bind(id.0)
        .execute(&s.pool)
        .await?;
    Ok(Json(serde_json::json!({"cover_url": cover_url})))
}

/// Re-run the cover-art worker for the selected candidate without rescanning
/// audio. This is the on-demand "find cover art" action: it re-checks every
/// catalog source plus artwork already verified for the same release, and
/// either verifies a cover (making the track Ready) or keeps it in Review.
pub async fn search_artwork(
    State(s): State<Arc<AppState>>,
    Path(id): Path<TrackId>,
) -> ApiResult<Json<serde_json::Value>> {
    let (_, mut candidate) = track_db::selected(&s.pool, id).await?;
    let completion = finalize_selection(&s, &mut candidate, None, None).await?;
    let mut transaction = s.pool.begin().await?;
    candidate_db::update(&mut *transaction, &candidate).await?;
    let (status, stage, message) =
        readiness_state(&candidate, &completion, "Cover search complete");
    sqlx::query("UPDATE tracks SET status=?,stage=?,stage_message=?,updated_at=? WHERE id=?")
        .bind(status)
        .bind(stage)
        .bind(message)
        .bind(chrono::Utc::now().to_rfc3339())
        .bind(id.0)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(Json(serde_json::json!({
        "cover_verified": candidate.artwork_status == ArtworkStatus::Verified,
        "ready": stage == "ready",
        "artwork_status": candidate.artwork_status,
        "artwork_message": candidate.artwork_message,
        "cover_url": candidate.cover_url,
    })))
}

async fn persist_artwork_override(
    pool: &sqlx::SqlitePool,
    path: &str,
    title: &str,
    artist: &str,
    cover_url: &str,
) -> Result<()> {
    sqlx::query("INSERT INTO artwork_overrides(path,title,artist,cover_url,updated_at) VALUES(?,?,?,?,?) ON CONFLICT(path) DO UPDATE SET title=excluded.title,artist=excluded.artist,cover_url=excluded.cover_url,updated_at=excluded.updated_at")
        .bind(path)
        .bind(title)
        .bind(artist)
        .bind(cover_url)
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn artwork_preview(
    State(s): State<Arc<AppState>>,
    Path(id): Path<TrackId>,
) -> ApiResult<axum::response::Response> {
    // Only the cover that will be written; the file's old cover has its own
    // preview, so a refused cover never looks accepted.
    let (track, candidate) = track_db::selected(&s.pool, id).await?;
    let artwork = crate::application::apply::resolve_artwork(&s, &track.filename, &candidate)
        .await
        .map_err(|_| ApiError::not_found("No matching cover is available for this track"))?;
    artwork_response(artwork)
}

pub async fn original_artwork_preview(
    State(s): State<Arc<AppState>>,
    Path(id): Path<TrackId>,
) -> ApiResult<axum::response::Response> {
    let track = track_db::track(&s.pool, id).await?;
    let artwork = tokio::task::spawn_blocking(move || {
        tag_writer::read_artwork(std::path::Path::new(&track.path))
    })
    .await
    .map_err(anyhow::Error::from)?
    // A file that cannot be read (damaged, or gone) has no picture to show.
    // That is "not found" like any song without one, not a server error.
    .ok()
    .flatten()
    .ok_or_else(|| ApiError::not_found("No embedded artwork is available for this track"))?;
    artwork_response(artwork)
}

pub async fn candidate_artwork_preview(
    State(s): State<Arc<AppState>>,
    Path(id): Path<CandidateId>,
) -> ApiResult<axum::response::Response> {
    let candidate = track_db::candidate(&s.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("candidate not found"))?;
    // The same check as filing: the page only shows a cover that would be used.
    match crate::application::artwork::resolve_for_write(&s.pool, &s.client, &candidate).await {
        Ok(bytes) => artwork_response(bytes),
        Err(_) => Err(ApiError::not_found(
            "No matching catalog artwork is available for this candidate",
        )),
    }
}

pub(super) fn artwork_response(artwork: Vec<u8>) -> ApiResult<axum::response::Response> {
    let mime = artwork_mime(&artwork);
    Ok(axum::response::Response::builder()
        .header(axum::http::header::CONTENT_TYPE, mime)
        .header(axum::http::header::CACHE_CONTROL, "private, max-age=300")
        .body(axum::body::Body::from(artwork))
        .map_err(anyhow::Error::from)?)
}

fn artwork_mime(data: &[u8]) -> &'static str {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if data.starts_with(b"GIF8") {
        "image/gif"
    } else if data.starts_with(b"RIFF") && data.get(8..12) == Some(b"WEBP") {
        "image/webp"
    } else {
        "image/jpeg"
    }
}

pub async fn resolve_source(
    State(s): State<Arc<AppState>>,
    Json(value): Json<SourceLookupRequest>,
) -> ApiResult<Json<Candidate>> {
    let url = value.url.trim();
    let parsed = reqwest::Url::parse(url).map_err(|_| {
        ApiError::validation(
            "Enter a valid Spotify, Apple Music, Deezer, Shazam, SoundCloud, Audiomack, Navahang, Radio Javan, Genius, or YouTube URL",
        )
    })?;
    let (_, found) = lookup_source(&s, url, &parsed).await.ok_or_else(|| {
        ApiError::validation("Source lookup failed: this site is not a supported music source")
    })?;
    let candidate =
        found.map_err(|error| ApiError::validation(format!("Source lookup failed: {error:#}")))?;
    Ok(Json(candidate))
}

/// Reads a song page of a supported music site. `None` when the link is not
/// from one; otherwise the site's name and what it returned.
async fn lookup_source(
    s: &AppState,
    url: &str,
    parsed: &reqwest::Url,
) -> Option<(&'static str, Result<Candidate>)> {
    let host = parsed.host_str()?.to_ascii_lowercase();
    // Every service the Add page reads a song from works here too, through the
    // same reader, so a site added there never has to be added here again.
    use crate::acquire::links;
    let expanded = match links::expand_short_link(&s.client, url).await {
        Ok(expanded) => expanded,
        Err(error) => return Some(("The short link", Err(error))),
    };
    if let Some((name, provider)) = links::classify(&expanded).and_then(|link| link_reader(&link)) {
        return Some((name, lookup_link_source(s, &expanded, provider).await));
    }
    let site = ["www.", "m.", "open.", "music.", "play."]
        .iter()
        .find_map(|prefix| host.strip_prefix(prefix))
        .unwrap_or(&host);
    let (pool, client) = (&s.pool, &s.client);
    Some(match site {
        // A Spotify, Apple Music, Deezer or Shazam link that is not a song.
        "spotify.com" | "apple.com" | "deezer.com" | "shazam.com" => (
            "This link",
            Err(anyhow::anyhow!("this is not a link to one song")),
        ),
        "soundcloud.com" => (
            "SoundCloud",
            providers::soundcloud::lookup_url(client, url).await,
        ),
        "audiomack.com" => (
            "Audiomack",
            providers::audiomack::lookup_url(pool, client, url).await,
        ),
        "navahang.com" => (
            "Navahang",
            providers::navahang::lookup_url(pool, client, url).await,
        ),
        "radiojavan.com" => (
            "Radio Javan",
            providers::radiojavan::lookup_url(pool, client, url).await,
        ),
        "genius.com" => (
            "Genius",
            providers::genius::lookup_url(pool, client, url).await,
        ),
        "youtube.com" | "youtu.be" => {
            ("YouTube", providers::youtube::lookup_url(client, url).await)
        }
        _ => return None,
    })
}

/// The name and source label of a link the Add page's reader fills the tags from, or
/// `None` for a service with a song page reader of its own. Every service is listed, so
/// adding one to the Add page does not compile until it is decided here too.
fn link_reader(link: &crate::acquire::links::Link) -> Option<(&'static str, &'static str)> {
    use crate::acquire::links::Service;
    match link.service {
        Service::Spotify => Some(("Spotify", "spotify")),
        Service::Apple => Some(("Apple Music", "itunes")),
        Service::Deezer => Some(("Deezer", "deezer")),
        Service::Shazam => Some(("Shazam", "shazam")),
        Service::YouTube | Service::RadioJavan => None,
    }
}

/// A Spotify, Apple Music, Deezer or Shazam song link, read the free way the bot reads links:
/// Spotify's public embed page, and Apple's lookup for Shazam (the Shazam
/// number is the Apple Music ID).
async fn lookup_link_source(s: &AppState, url: &str, provider: &str) -> Result<Candidate> {
    use crate::acquire::links;
    let link = links::classify(url)
        .filter(|link| link.scope == links::Scope::Track)
        .ok_or_else(|| anyhow::anyhow!("this is not a song link"))?;
    let song = links::read(&s.client, &s.pool, &s.config, &link)
        .await?
        .songs
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("the link has no song"))?;
    let label = crate::domain::provider_name(provider);
    Ok(Candidate {
        provider: provider.into(),
        title: song.title,
        artist: song.artist.unwrap_or_default(),
        album: song.album,
        track_number: song.track_number,
        year: song
            .release_date
            .as_deref()
            .and_then(|date| date.get(..4))
            .map(str::to_owned),
        release_date: song.release_date,
        isrc: song.isrc,
        cover_url: song.cover_url,
        duration_delta: song.duration,
        score: 97.0,
        score_breakdown: Some(
            serde_json::json!({
                "source": format!("{provider}_user_source_url"),
                "sources": [label],
                "user_verified_source": true,
                "final_score": 97.0
            })
            .to_string(),
        ),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::Config, infrastructure::db};

    #[tokio::test]
    async fn remove_track_deletes_review_file_and_database_record() {
        let directory = tempfile::tempdir().unwrap();
        let input = directory.path().join("music");
        tokio::fs::create_dir(&input).await.unwrap();
        let source = input.join("remove-me.mp3");
        tokio::fs::write(&source, b"test audio").await.unwrap();
        let database = directory.path().join("remove-track.sqlite");
        let pool = db::connect(database.to_str().unwrap()).await.unwrap();
        let config = Config {
            input_dir: input.to_string_lossy().into_owned(),
            ..Config::default()
        };
        let state = Arc::new(AppState::new(config, pool.clone()));
        let track_id = insert_test_track(&pool, source.to_str().unwrap(), "needs_review", 0).await;
        insert_test_candidate(&pool, track_id, "Candidate", 70.0).await;

        let response = remove_track(State(state), Path(TrackId(track_id)))
            .await
            .unwrap()
            .0;

        assert_eq!(response["removed"], true);
        assert!(!source.exists());
        let tracks: i64 = sqlx::query_scalar("SELECT count(*) FROM tracks WHERE id=?")
            .bind(track_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        let candidates: i64 =
            sqlx::query_scalar("SELECT count(*) FROM candidates WHERE track_id=?")
                .bind(track_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(tracks, 0);
        assert_eq!(candidates, 0);
    }

    #[tokio::test]
    async fn remove_track_refuses_files_outside_music_folder() {
        let directory = tempfile::tempdir().unwrap();
        let input = directory.path().join("music");
        tokio::fs::create_dir(&input).await.unwrap();
        let source = directory.path().join("keep-me.mp3");
        tokio::fs::write(&source, b"test audio").await.unwrap();
        let database = directory.path().join("safe-remove-track.sqlite");
        let pool = db::connect(database.to_str().unwrap()).await.unwrap();
        let config = Config {
            input_dir: input.to_string_lossy().into_owned(),
            ..Config::default()
        };
        let state = Arc::new(AppState::new(config, pool.clone()));
        let track_id = insert_test_track(&pool, source.to_str().unwrap(), "needs_review", 0).await;

        let result = remove_track(State(state), Path(TrackId(track_id))).await;

        assert!(matches!(result, Err(ApiError::Validation(_))));
        assert!(source.exists());
        let tracks: i64 = sqlx::query_scalar("SELECT count(*) FROM tracks WHERE id=?")
            .bind(track_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(tracks, 1);
    }

    #[tokio::test]
    async fn removing_a_duplicate_representative_promotes_the_best_remaining_input() {
        let directory = tempfile::tempdir().unwrap();
        let input = directory.path().join("music");
        tokio::fs::create_dir(&input).await.unwrap();
        let representative = input.join("representative.mp3");
        let promoted = input.join("promoted.mp3");
        tokio::fs::write(&representative, b"audio").await.unwrap();
        tokio::fs::write(&promoted, b"audio").await.unwrap();
        let database = directory.path().join("promote-duplicate.sqlite");
        let pool = db::connect(database.to_str().unwrap()).await.unwrap();
        let state = Arc::new(AppState::new(
            Config {
                input_dir: input.to_string_lossy().into_owned(),
                ..Config::default()
            },
            pool.clone(),
        ));
        let representative_id = sqlx::query(
            "INSERT INTO tracks(path,filename,format,bitrate,duration,content_fingerprint,status,
             is_missing,first_seen_at,last_seen_at,last_scanned_at,stage)
             VALUES(?,'representative.mp3','mp3',192,180.0,'fp:same','needs_review',0,
             'now','now','now','review')",
        )
        .bind(representative.to_string_lossy().as_ref())
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_rowid();
        let candidate_id = insert_test_candidate(&pool, representative_id, "Candidate", 70.0).await;
        let promoted_id = sqlx::query(
            "INSERT INTO tracks(path,filename,format,bitrate,duration,content_fingerprint,status,
             is_missing,first_seen_at,last_seen_at,last_scanned_at,stage)
             VALUES(?,'promoted.mp3','mp3',320,180.0,'fp:same','duplicate',0,
             'now','now','now','skipped')",
        )
        .bind(promoted.to_string_lossy().as_ref())
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_rowid();

        let _ = remove_track(State(state), Path(TrackId(representative_id)))
            .await
            .unwrap();

        let row: (String, String, Option<i64>) =
            sqlx::query_as("SELECT status,stage,selected_candidate_id FROM tracks WHERE id=?")
                .bind(promoted_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(row, ("review".into(), "review".into(), None));
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT track_id FROM candidates WHERE id=?")
                .bind(candidate_id)
                .fetch_one(&pool)
                .await
                .unwrap(),
            promoted_id
        );
        assert!(!representative.exists());
        assert!(promoted.exists());
    }

    #[tokio::test]
    async fn removing_a_song_whose_file_is_already_gone_clears_it_from_the_list() {
        let directory = tempfile::tempdir().unwrap();
        let input = directory.path().join("input");
        tokio::fs::create_dir(&input).await.unwrap();
        let database = directory.path().join("already-gone.sqlite");
        let pool = db::connect(database.to_str().unwrap()).await.unwrap();
        let state = Arc::new(AppState::new(
            Config {
                input_dir: input.to_string_lossy().into_owned(),
                ..Config::default()
            },
            pool.clone(),
        ));
        // Marked as an issue, as a failed check leaves it: Remove must still work.
        let track_id = sqlx::query(
            "INSERT INTO tracks(path,filename,status,is_missing,first_seen_at,last_seen_at,
             last_scanned_at,stage) VALUES(?,'gone.mp3','failed',1,'now','now','now','failed')",
        )
        .bind(input.join("gone.mp3").to_string_lossy().as_ref())
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_rowid();
        insert_test_candidate(&pool, track_id, "Candidate", 70.0).await;

        let Json(response) = remove_track(State(state), Path(TrackId(track_id)))
            .await
            .unwrap();

        assert_eq!(response["removed"], true);
        let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tracks")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(left, 0);
    }

    /// One song as four sites show it. Run with
    /// `cargo test --locked every_supported_song_link -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore = "live link probe"]
    async fn every_supported_song_link_fills_in_the_tags() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("links.sqlite");
        let pool = db::connect(database.to_str().unwrap()).await.unwrap();
        let state = Arc::new(AppState::new(Config::default(), pool));
        for url in [
            "https://music.apple.com/de/song/boodi-yaram-feat-hoomaan/1618435063",
            "https://play.radiojavan.com/song/alireza-jj-sijal-sepehr-khalse-boodi-yaram-(ft-hoomaan)",
            "https://www.youtube.com/watch?v=nRWpJZRkTrk",
            "https://open.spotify.com/track/1aFllLRJyLxIriyOL2g8Ww",
        ] {
            let found = resolve_source(
                State(state.clone()),
                Json(SourceLookupRequest { url: url.into() }),
            )
            .await;
            match found {
                Ok(Json(song)) => {
                    println!(
                        "{url}\n  title={:?} artist={:?} album={:?} year={:?} date={:?} cover={:?} credits={:?}",
                        song.title,
                        song.artist,
                        song.album,
                        song.year,
                        song.release_date,
                        song.cover_url,
                        song.artist_credits
                            .iter()
                            .map(|c| c.name.clone())
                            .collect::<Vec<_>>()
                    );
                    assert!(song.title.contains("Boodi Yaram"), "{url}: {}", song.title);
                }
                Err(_) => panic!("{url} could not be read"),
            }
        }
    }

    #[test]
    fn song_links_the_add_page_reads_also_fill_in_the_tags() {
        use crate::acquire::links::classify;
        for (url, name) in [
            (
                "https://music.apple.com/de/song/boodi-yaram-feat-hoomaan/1618435063",
                "Apple Music",
            ),
            (
                "https://open.spotify.com/track/1aFllLRJyLxIriyOL2g8Ww",
                "Spotify",
            ),
            ("https://www.deezer.com/en/track/1109731", "Deezer"),
            (
                "https://www.shazam.com/song/1845811123/donyaye-movazi",
                "Shazam",
            ),
        ] {
            let link = classify(url).unwrap_or_else(|| panic!("{url} is not recognised"));
            assert_eq!(
                link_reader(&link).map(|(site, _)| site),
                Some(name),
                "{url}"
            );
        }
    }

    #[test]
    fn stale_artist_chips_do_not_undo_the_artist_you_typed() {
        use crate::domain::credits::{ArtistCredit, names_same_people};
        let chips = [
            ArtistCredit::new("Alireza Jj", " feat. "),
            ArtistCredit::new("Hoomaan", ""),
        ];
        assert!(names_same_people(
            "Alireza JJ feat. Hoomaan",
            "Boodi Yaram",
            &chips
        ));
        assert!(names_same_people(
            "Alireza JJ",
            "Boodi Yaram (feat. Hoomaan)",
            &chips
        ));
        assert!(!names_same_people(
            "Alireza JJ, Sijal, Sepehr Khalse ft. Hoomaan",
            "Boodi Yaram",
            &chips
        ));
    }

    #[tokio::test]
    async fn manual_metadata_with_a_verified_cover_becomes_ready() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("manual.sqlite");
        let pool = db::connect(database.to_str().unwrap()).await.unwrap();
        let state = Arc::new(AppState::new(Config::default(), pool.clone()));
        crate::infrastructure::provider_cache::put(
            &pool,
            "artwork-url",
            &crate::infrastructure::provider_cache::url_key(
                "https://example.test/manual-cover.jpg",
            ),
            &serde_json::json!({
                "data_base64": base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    crate::infrastructure::media::tag_writer::test_artwork_png()
                )
            }),
            chrono::Utc::now() + chrono::Duration::days(1),
        )
        .await
        .unwrap();
        let track_id = sqlx::query("INSERT INTO tracks(path,filename,status,is_missing,first_seen_at,last_seen_at,last_scanned_at,stage) VALUES('/music/unknown.mp3','unknown.mp3','needs_review',0,'now','now','now','review')")
            .execute(&pool)
            .await
            .unwrap()
            .last_insert_rowid();

        let Json(response) = manual_candidate(
            State(state),
            Path(TrackId(track_id)),
            Json(CandidateEdit {
                title: "Correct title".into(),
                artist: "Correct artist".into(),
                artist_credits: Vec::new(),
                album: Some("Correct album".into()),
                album_artist: None,
                album_artist_credits: Vec::new(),
                track_number: Some(1),
                track_total: None,
                disc_number: None,
                disc_total: None,
                year: Some("2026".into()),
                genre: None,
                composer: None,
                label: None,
                isrc: None,
                release_date: None,
                cover_url: Some("https://example.test/manual-cover.jpg".into()),
            }),
        )
        .await
        .unwrap();

        assert_eq!(response["ready"], true);
        assert_eq!(response["cover_verified"], true);
        let row: (String, String, String) = sqlx::query_as("SELECT tracks.stage,candidates.title,candidates.artist FROM tracks JOIN candidates ON candidates.id=tracks.selected_candidate_id WHERE tracks.id=?")
            .bind(track_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            row,
            (
                "ready".into(),
                "Correct title".into(),
                "Correct artist".into()
            )
        );
    }

    #[tokio::test]
    async fn manual_metadata_without_a_cover_stays_in_review() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("manual-no-cover.sqlite");
        let pool = db::connect(database.to_str().unwrap()).await.unwrap();
        let mut offline = AppState::new(Config::default(), pool.clone());
        // The cover search must fail here, and fail without the internet.
        offline.client = reqwest::Client::builder()
            .proxy(reqwest::Proxy::all("http://127.0.0.1:9").unwrap())
            .build()
            .unwrap();
        let state = Arc::new(offline);
        let track_id = sqlx::query("INSERT INTO tracks(path,filename,status,is_missing,first_seen_at,last_seen_at,last_scanned_at,stage) VALUES('/music/unknown.mp3','unknown.mp3','needs_review',0,'now','now','now','review')")
            .execute(&pool)
            .await
            .unwrap()
            .last_insert_rowid();

        let Json(response) = manual_candidate(
            State(state),
            Path(TrackId(track_id)),
            Json(CandidateEdit {
                title: "Correct title".into(),
                artist: "Correct artist".into(),
                artist_credits: Vec::new(),
                album: Some("Correct album".into()),
                album_artist: None,
                album_artist_credits: Vec::new(),
                track_number: Some(1),
                track_total: None,
                disc_number: None,
                disc_total: None,
                year: Some("2026".into()),
                genre: None,
                composer: None,
                label: None,
                isrc: None,
                release_date: None,
                cover_url: None,
            }),
        )
        .await
        .unwrap();

        assert_eq!(response["selected"], true);
        assert_eq!(response["ready"], false);
        assert_eq!(response["cover_verified"], false);
        let row: (String, String, String) = sqlx::query_as(
            "SELECT tracks.stage,tracks.status,(SELECT artwork_status FROM candidates WHERE id=tracks.selected_candidate_id)
             FROM tracks WHERE id=?",
        )
        .bind(track_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.0, "review");
        assert_eq!(row.1, "needs_review");
        assert_ne!(row.2, "verified");
    }

    #[tokio::test]
    async fn keeping_the_files_own_tags_selects_them_and_still_needs_a_cover() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("keep-tags.sqlite");
        let pool = db::connect(database.to_str().unwrap()).await.unwrap();
        let mut offline = AppState::new(Config::default(), pool.clone());
        // The cover search must fail here, and fail without the internet.
        offline.client = reqwest::Client::builder()
            .proxy(reqwest::Proxy::all("http://127.0.0.1:9").unwrap())
            .build()
            .unwrap();
        let state = Arc::new(offline);
        let track_id = sqlx::query("INSERT INTO tracks(path,filename,status,is_missing,first_seen_at,last_seen_at,last_scanned_at,stage,current_title,current_artist) VALUES('/music/own.mp3','own.mp3','needs_review',0,'now','now','now','review','Mano Bebakhsh (Nostalogy)','Marjan Kandi')")
            .execute(&pool)
            .await
            .unwrap()
            .last_insert_rowid();
        // A wrong match that must not be the one chosen.
        sqlx::query(
            "INSERT INTO candidates(track_id,provider,title,artist,album,duration_delta,score,score_breakdown)
             VALUES(?,'deezer','Other song','Other singer','Other album',1.0,60.0,'{}')",
        )
        .bind(track_id)
        .execute(&pool)
        .await
        .unwrap();

        let Json(response) = keep_file_tags(State(state.clone()), Path(TrackId(track_id)))
            .await
            .unwrap();

        assert_eq!(response["selected"], true);
        // The cover gate holds: the file's tags alone do not make it Ready.
        assert_eq!(response["ready"], false);
        let row: (String, String, String, String, String) = sqlx::query_as(
            "SELECT tracks.stage,candidates.provider,candidates.title,candidates.artist,tracks.stage_message
             FROM tracks JOIN candidates ON candidates.id=tracks.selected_candidate_id WHERE tracks.id=?",
        )
        .bind(track_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.0, "review");
        assert_eq!(row.1, "manual");
        assert_eq!(row.2, "Mano Bebakhsh (Nostalogy)");
        assert_eq!(row.3, "Marjan Kandi");
        assert!(row.4.starts_with("Kept the file's own tags"));

        // A file without a title of its own has nothing to keep.
        let untitled = sqlx::query("INSERT INTO tracks(path,filename,status,is_missing,first_seen_at,last_seen_at,last_scanned_at,stage,current_artist) VALUES('/music/untitled.mp3','untitled.mp3','needs_review',0,'now','now','now','review','Marjan Kandi')")
            .execute(&pool)
            .await
            .unwrap()
            .last_insert_rowid();
        assert!(
            keep_file_tags(State(state), Path(TrackId(untitled)))
                .await
                .is_err()
        );
    }

    /// A short MP3 with `cover` inside, or `None` without ffmpeg.
    fn song_with_cover(directory: &std::path::Path, cover: Vec<u8>) -> Option<String> {
        let path = directory.join("own-cover.mp3");
        let made = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
            .arg("sine=frequency=440:duration=0.1")
            .args(["-q:a", "9"])
            .arg(&path)
            .status()
            .ok()?;
        assert!(made.success());
        let tags = Candidate {
            title: "Mano Bebakhsh (Nostalogy)".into(),
            artist: "Marjan Kandi".into(),
            ..Default::default()
        };
        tag_writer::write(&path, &tags, Some(cover), None).unwrap();
        Some(path.to_string_lossy().into_owned())
    }

    async fn keep_tags_of(path: &str, database: &str) -> (serde_json::Value, String, String) {
        let pool = db::connect(database).await.unwrap();
        let mut offline = AppState::new(Config::default(), pool.clone());
        // Nothing here may reach the internet.
        offline.client = reqwest::Client::builder()
            .proxy(reqwest::Proxy::all("http://127.0.0.1:9").unwrap())
            .build()
            .unwrap();
        let track_id = sqlx::query("INSERT INTO tracks(path,filename,status,is_missing,first_seen_at,last_seen_at,last_scanned_at,stage,current_title,current_artist) VALUES(?,'own-cover.mp3','needs_review',0,'now','now','now','review','Mano Bebakhsh (Nostalogy)','Marjan Kandi')")
            .bind(path)
            .execute(&pool)
            .await
            .unwrap()
            .last_insert_rowid();
        let Json(response) = keep_file_tags(State(Arc::new(offline)), Path(TrackId(track_id)))
            .await
            .unwrap();
        let row: (String, String) = sqlx::query_as(
            "SELECT tracks.stage,candidates.artwork_status FROM tracks
             JOIN candidates ON candidates.id=tracks.selected_candidate_id WHERE tracks.id=?",
        )
        .bind(track_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        // The file's own picture is never remembered as a cover choice for a later scan.
        let remembered: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM artwork_overrides")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(remembered, 0);
        (response, row.0, row.1)
    }

    #[tokio::test]
    async fn keeping_the_files_tags_uses_its_own_square_cover() {
        let directory = tempfile::tempdir().unwrap();
        let Some(path) = song_with_cover(
            directory.path(),
            tag_writer::test_artwork_png_sized(600, 600),
        ) else {
            return;
        };
        let database = directory.path().join("own-cover.sqlite");
        let (response, stage, artwork) = keep_tags_of(&path, database.to_str().unwrap()).await;

        assert_eq!(response["ready"], true);
        assert_eq!(response["cover_verified"], true);
        assert!(response["own_cover_refused"].is_null());
        assert_eq!(stage, "ready");
        assert_eq!(artwork, "verified");
    }

    #[tokio::test]
    async fn keeping_the_files_tags_refuses_a_video_frame_as_cover() {
        let directory = tempfile::tempdir().unwrap();
        let Some(path) = song_with_cover(
            directory.path(),
            tag_writer::test_artwork_png_sized(800, 600),
        ) else {
            return;
        };
        let database = directory.path().join("video-frame.sqlite");
        let (response, stage, artwork) = keep_tags_of(&path, database.to_str().unwrap()).await;

        assert_eq!(response["ready"], false);
        assert!(
            response["own_cover_refused"]
                .as_str()
                .unwrap()
                .contains("not square")
        );
        assert_eq!(stage, "review");
        assert_ne!(artwork, "verified");
    }

    #[tokio::test]
    async fn another_files_picture_cannot_be_typed_in_as_a_cover() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("other-file.sqlite");
        let pool = db::connect(database.to_str().unwrap()).await.unwrap();
        let state = Arc::new(AppState::new(Config::default(), pool.clone()));
        let track_id = sqlx::query("INSERT INTO tracks(path,filename,status,is_missing,first_seen_at,last_seen_at,last_scanned_at,stage) VALUES('/music/mine.mp3','mine.mp3','needs_review',0,'now','now','now','review')")
            .execute(&pool)
            .await
            .unwrap()
            .last_insert_rowid();
        let result = manual_candidate(
            State(state),
            Path(TrackId(track_id)),
            Json(CandidateEdit {
                title: "Song".into(),
                artist: "Artist".into(),
                artist_credits: Vec::new(),
                album: None,
                album_artist: None,
                album_artist_credits: Vec::new(),
                track_number: None,
                track_total: None,
                disc_number: None,
                disc_total: None,
                year: None,
                genre: None,
                composer: None,
                label: None,
                isrc: None,
                release_date: None,
                cover_url: Some(providers::cover_art_archive::file_cover_url(
                    "/music/someone-elses.mp3",
                )),
            }),
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn explicit_choice_without_a_verified_cover_stays_in_review() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("choose-cover-pending.sqlite");
        let pool = db::connect(database.to_str().unwrap()).await.unwrap();
        let state = Arc::new(AppState::new(Config::default(), pool.clone()));
        let track_id = sqlx::query("INSERT INTO tracks(path,filename,status,is_missing,first_seen_at,last_seen_at,last_scanned_at,stage) VALUES('/music/cover-pending.mp3','cover-pending.mp3','needs_review',0,'now','now','now','review')")
            .execute(&pool)
            .await
            .unwrap()
            .last_insert_rowid();
        let candidate_id = sqlx::query(
            "INSERT INTO candidates(track_id,provider,title,artist,album,cover_url,duration_delta,score,score_breakdown)
             VALUES(?,'deezer','Song','Artist','Album','http://127.0.0.1:1/broken.jpg',1.0,95.0,'{}')",
        )
        .bind(track_id)
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_rowid();

        let Json(response) = select_candidate(
            State(state),
            Path(TrackId(track_id)),
            Json(SelectRequest {
                candidate_id: Some(CandidateId(candidate_id)),
            }),
        )
        .await
        .unwrap();

        assert_eq!(response["selected"], true);
        assert_eq!(response["ready"], false);
        assert_eq!(response["cover_verified"], false);
        let row: (Option<i64>, String, String, String) = sqlx::query_as(
            "SELECT selected_candidate_id,status,stage,
                    (SELECT artwork_status FROM candidates WHERE id=?) FROM tracks WHERE id=?",
        )
        .bind(candidate_id)
        .bind(track_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.0, Some(candidate_id));
        assert_eq!(row.1, "needs_review");
        assert_eq!(row.2, "review");
        assert_ne!(row.3, "verified");
    }

    #[tokio::test]
    async fn artwork_search_uses_a_same_release_cover_to_make_the_track_ready() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("artwork-search.sqlite");
        let pool = db::connect(database.to_str().unwrap()).await.unwrap();
        let state = Arc::new(AppState::new(Config::default(), pool.clone()));
        crate::infrastructure::provider_cache::put(
            &pool,
            "artwork-url",
            &crate::infrastructure::provider_cache::url_key(
                "https://example.test/release-cover.jpg",
            ),
            &serde_json::json!({
                "data_base64": base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    crate::infrastructure::media::tag_writer::test_artwork_png()
                )
            }),
            chrono::Utc::now() + chrono::Duration::days(1),
        )
        .await
        .unwrap();
        let track_id = sqlx::query("INSERT INTO tracks(path,filename,status,is_missing,first_seen_at,last_seen_at,last_scanned_at,stage) VALUES('/music/pending-cover.mp3','pending-cover.mp3','needs_review',0,'now','now','now','review')")
            .execute(&pool)
            .await
            .unwrap()
            .last_insert_rowid();
        let candidate_id = sqlx::query(
            "INSERT INTO candidates(track_id,provider,title,artist,album,musicbrainz_release_id,cover_url,duration_delta,score,score_breakdown,artwork_status)
             VALUES(?,'deezer','Song','Artist','Album','b10bbbfc-cf9e-42e0-be17-e2c3e1d2600d','http://127.0.0.1:1/broken.jpg',1.0,95.0,'{}','retryable_error')",
        )
        .bind(track_id)
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_rowid();
        sqlx::query("UPDATE tracks SET selected_candidate_id=? WHERE id=?")
            .bind(candidate_id)
            .bind(track_id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO candidates(track_id,provider,title,artist,album,musicbrainz_release_id,cover_url,score,artwork_status)
             VALUES(?,'itunes','Song','Artist','Album','b10bbbfc-cf9e-42e0-be17-e2c3e1d2600d','https://example.test/release-cover.jpg',90,'verified')",
        )
        .bind(track_id)
        .execute(&pool)
        .await
        .unwrap();

        let Json(response) = search_artwork(State(state), Path(TrackId(track_id)))
            .await
            .unwrap();

        assert_eq!(response["cover_verified"], true);
        assert_eq!(response["ready"], true);
        let row: (String, String, String) = sqlx::query_as(
            "SELECT tracks.stage,tracks.status,(SELECT artwork_status FROM candidates WHERE id=?) FROM tracks WHERE id=?",
        )
        .bind(candidate_id)
        .bind(track_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.0, "ready");
        assert_eq!(row.1, "selected");
        assert_eq!(row.2, "verified");
    }

    #[tokio::test]
    async fn auto_approve_uses_smart_selection_and_leaves_unsafe_tracks() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("auto-approve.sqlite");
        let pool = db::connect(database.to_str().unwrap()).await.unwrap();
        let state = Arc::new(AppState::new(Config::default(), pool.clone()));
        crate::infrastructure::provider_cache::put(
            &pool,
            "artwork-url",
            &crate::infrastructure::provider_cache::url_key("https://example.test/cover.jpg"),
            &serde_json::json!({
                "data_base64": base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    crate::infrastructure::media::tag_writer::test_artwork_png()
                )
            }),
            chrono::Utc::now() + chrono::Duration::days(1),
        )
        .await
        .unwrap();

        let review_id = insert_test_track(&pool, "/music/review.mp3", "needs_review", 0).await;
        let smart_id = insert_test_candidate(&pool, review_id, "Song", 90.0).await;
        let highest_id = insert_test_candidate(&pool, review_id, "Song (Live)", 99.0).await;
        let corrupt_id = insert_test_track(&pool, "/music/corrupt.mp3", "corrupt", 0).await;
        insert_test_candidate(&pool, corrupt_id, "Must stay", 99.0).await;
        insert_test_track(&pool, "/music/unmatched.mp3", "needs_review", 0).await;
        let missing_id = insert_test_track(&pool, "/music/missing.mp3", "needs_review", 1).await;
        insert_test_candidate(&pool, missing_id, "Missing source", 99.0).await;

        let result = auto_approve_review(State(state)).await.unwrap().0;

        assert_eq!(result.approved, 1);
        assert_eq!(result.low_confidence, 0);
        assert_eq!(result.unavailable, 3);
        let selected: (Option<i64>, String, String, Option<String>) = sqlx::query_as(
            "SELECT selected_candidate_id,status,stage,stage_message FROM tracks WHERE id=?",
        )
        .bind(review_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(selected.0, Some(smart_id));
        assert_ne!(selected.0, Some(highest_id));
        assert_eq!(selected.1, "selected");
        assert_eq!(selected.2, "ready");
        assert!(selected.3.unwrap().starts_with("Smart auto-approved"));
        let untouched: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM tracks WHERE id IN (?,?) AND stage='review' AND selected_candidate_id IS NULL",
        )
        .bind(corrupt_id)
        .bind(missing_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(untouched, 2);
    }

    #[tokio::test]
    async fn auto_approve_waits_for_a_running_workflow_and_frees_the_slot() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("auto-approve-busy.sqlite");
        let pool = db::connect(database.to_str().unwrap()).await.unwrap();
        let state = Arc::new(AppState::new(Config::default(), pool));

        assert!(
            state
                .try_claim_workflow(WorkflowPhase::Scan, "Scanning")
                .await
        );
        assert!(matches!(
            auto_approve_review(State(state.clone())).await,
            Err(ApiError::Conflict(_))
        ));
        state.reset_workflow(WorkflowPhase::Idle, "Ready").await;

        let _ = auto_approve_review(State(state.clone())).await.unwrap();
        assert!(!state.workflow_running().await);
    }

    #[tokio::test]
    async fn return_to_review_clears_selection_but_keeps_candidates() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("return-to-review.sqlite");
        let pool = db::connect(database.to_str().unwrap()).await.unwrap();
        let state = Arc::new(AppState::new(Config::default(), pool.clone()));
        let track_id = sqlx::query("INSERT INTO tracks(path,filename,status,is_missing,first_seen_at,last_seen_at,last_scanned_at,stage) VALUES('/music/ready.mp3','ready.mp3','selected',0,'now','now','now','ready')")
            .execute(&pool)
            .await
            .unwrap()
            .last_insert_rowid();
        let candidate_id = sqlx::query("INSERT INTO candidates(track_id,provider,title,artist,score) VALUES(?,'deezer','Song','Artist',95)")
            .bind(track_id)
            .execute(&pool)
            .await
            .unwrap()
            .last_insert_rowid();
        sqlx::query("UPDATE tracks SET selected_candidate_id=? WHERE id=?")
            .bind(candidate_id)
            .bind(track_id)
            .execute(&pool)
            .await
            .unwrap();

        let _ = return_to_review(State(state), Path(TrackId(track_id)))
            .await
            .unwrap();

        let track: (Option<i64>, String, String, Option<String>) = sqlx::query_as(
            "SELECT selected_candidate_id,status,stage,stage_message FROM tracks WHERE id=?",
        )
        .bind(track_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(track.0, None);
        assert_eq!(track.1, "needs_review");
        assert_eq!(track.2, "review");
        assert_eq!(track.3.as_deref(), Some("Returned to review"));
        let candidates: i64 =
            sqlx::query_scalar("SELECT count(*) FROM candidates WHERE track_id=?")
                .bind(track_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(candidates, 1);
    }

    async fn insert_test_track(
        pool: &sqlx::SqlitePool,
        path: &str,
        status: &str,
        is_missing: i64,
    ) -> i64 {
        sqlx::query("INSERT INTO tracks(path,filename,current_title,current_artist,status,is_missing,first_seen_at,last_seen_at,last_scanned_at,stage) VALUES(?,?,'Song','Artist',?,?,'now','now','now','review')")
            .bind(path)
            .bind(path.rsplit('/').next().unwrap())
            .bind(status)
            .bind(is_missing)
            .execute(pool)
            .await
            .unwrap()
            .last_insert_rowid()
    }

    async fn insert_test_candidate(
        pool: &sqlx::SqlitePool,
        track_id: i64,
        title: &str,
        score: f64,
    ) -> i64 {
        sqlx::query("INSERT INTO candidates(track_id,provider,title,artist,album,cover_url,duration_delta,score,score_breakdown) VALUES(?,'deezer',?,'Artist','Album','https://example.test/cover.jpg',1.0,?,'{\"sources\":[\"Deezer\"]}')")
            .bind(track_id)
            .bind(title)
            .bind(score)
            .execute(pool)
            .await
            .unwrap()
            .last_insert_rowid()
    }

    #[test]
    fn parses_browser_audio_ranges() {
        assert_eq!(parse_byte_range(None, 100), Ok(None));
        assert_eq!(parse_byte_range(Some("bytes=0-9"), 100), Ok(Some((0, 9))));
        assert_eq!(parse_byte_range(Some("bytes=90-"), 100), Ok(Some((90, 99))));
        assert_eq!(parse_byte_range(Some("bytes=-10"), 100), Ok(Some((90, 99))));
        assert!(parse_byte_range(Some("bytes=100-"), 100).is_err());
        assert!(parse_byte_range(Some("items=0-9"), 100).is_err());
    }
}
