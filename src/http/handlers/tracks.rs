use super::*;
use crate::application::{Donors, finalize_selection, metadata_completion::CompletionReport};
use crate::infrastructure::{
    db::candidates as candidate_db,
    providers::{self, ArtworkCandidate, ArtworkStatus},
};
use crate::types::TrackStage;
use axum::http::{HeaderMap, StatusCode, header};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

pub async fn list_tracks(State(s): State<Arc<AppState>>) -> ApiResult<Json<Vec<WorkspaceTrack>>> {
    let tracks: Vec<Track> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {} FROM tracks ORDER BY path LIMIT 10000",
        track_db::TRACK_FIELDS
    )))
    .fetch_all(&s.pool)
    .await?;
    let mut by_track =
        track_db::candidates_by_track(&s.pool, tracks.iter().map(|track| track.id)).await?;
    Ok(Json(
        tracks
            .into_iter()
            .map(|track| WorkspaceTrack {
                candidates: by_track.remove(&track.id).unwrap_or_default(),
                track,
            })
            .collect(),
    ))
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
    if track.stage != TrackStage::Review || track.selected_candidate_id.is_some() {
        return Err(ApiError::validation(
            "Only unresolved tracks in Review can be removed",
        ));
    }

    let cfg = &s.config;
    let input_dir = tokio::fs::canonicalize(&cfg.input_dir)
        .await
        .map_err(|error| anyhow::anyhow!("could not verify the music folder: {error}"))?;
    let source = tokio::fs::canonicalize(&track.path)
        .await
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                ApiError::not_found("music file no longer exists")
            } else {
                ApiError::Internal(error.into())
            }
        })?;
    let in_inbox = source != input_dir && source.starts_with(&input_dir);
    if !in_inbox && !source.starts_with(&cfg.staging_dir) {
        return Err(ApiError::validation(
            "Refusing to remove a file outside the inbox",
        ));
    }
    if !tokio::fs::metadata(&source).await?.is_file() {
        return Err(ApiError::validation(
            "The selected music path is not a regular file",
        ));
    }

    // Your own files go to the trash; magpie's downloads are simply deleted.
    crate::application::library::discard_original(
        &cfg.trash_dir,
        &cfg.staging_dir,
        &input_dir,
        &source,
    )
    .await
    .map_err(|error| ApiError::Internal(anyhow::anyhow!("could not remove music file: {error}")))?;
    sqlx::query(
        "UPDATE wanted SET status='failed',error='Removed in review',updated_at=? WHERE staging_path=?",
    )
    .bind(chrono::Utc::now().to_rfc3339())
    .bind(source.to_string_lossy().as_ref())
    .execute(&s.pool)
    .await?;

    let canonical_path = source.to_string_lossy().into_owned();
    let mut transaction = s.pool.begin().await?;
    for table in [
        "fingerprint_cache",
        "content_hash_cache",
        "integrity_cache",
        "artwork_overrides",
    ] {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DELETE FROM {table} WHERE path=? OR path=?"
        )))
        .bind(&track.path)
        .bind(&canonical_path)
        .execute(&mut *transaction)
        .await?;
    }
    if let Some(content_key) = track.content_fingerprint.as_deref() {
        let duplicate_tracks: Vec<Track> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {} FROM tracks
             WHERE id!=? AND status='duplicate' AND stage='skipped'
               AND content_fingerprint=? AND is_missing=0
               AND (? LIKE 'sha256:%' OR (duration IS NOT NULL AND ABS(duration - ?) <= 3.0))",
            track_db::TRACK_FIELDS
        )))
        .bind(track.id.0)
        .bind(content_key)
        .bind(content_key)
        .bind(track.duration)
        .fetch_all(&mut *transaction)
        .await?;
        let evidence = duplicate_tracks
            .iter()
            .map(
                |duplicate| crate::application::input_dedup::RecordingEvidence {
                    path: duplicate.path.clone().into(),
                    format: duplicate.format.clone().unwrap_or_default(),
                    bitrate: duplicate
                        .bitrate
                        .and_then(|value| u32::try_from(value).ok()),
                    duration: duplicate.duration,
                    content_key: duplicate.content_fingerprint.clone(),
                    isrc: None,
                },
            )
            .collect::<Vec<_>>();
        if let Some(group) = crate::application::input_dedup::group_recordings(&evidence).first() {
            let promoted = &duplicate_tracks[group.representative];
            sqlx::query("DELETE FROM candidates WHERE track_id=?")
                .bind(promoted.id.0)
                .execute(&mut *transaction)
                .await?;
            sqlx::query("UPDATE candidates SET track_id=? WHERE track_id=?")
                .bind(promoted.id.0)
                .bind(track.id.0)
                .execute(&mut *transaction)
                .await?;
            sqlx::query(
                "UPDATE tracks SET output_path=NULL,status='review',stage='review',
                 selected_candidate_id=NULL,error=NULL,
                 stage_message='Promoted after the previous duplicate representative was removed',
                 updated_at=? WHERE id=?",
            )
            .bind(chrono::Utc::now().to_rfc3339())
            .bind(promoted.id.0)
            .execute(&mut *transaction)
            .await?;
            sqlx::query(
                "UPDATE tracks SET output_path=?,stage_message=?,updated_at=?
                 WHERE id!=? AND status='duplicate' AND stage='skipped'
                   AND content_fingerprint=?
                   AND (? LIKE 'sha256:%' OR (duration IS NOT NULL AND ABS(duration - ?) <= 3.0))",
            )
            .bind(&promoted.path)
            .bind(format!("Duplicate of input file: {}", promoted.path))
            .bind(chrono::Utc::now().to_rfc3339())
            .bind(promoted.id.0)
            .bind(content_key)
            .bind(content_key)
            .bind(track.duration)
            .execute(&mut *transaction)
            .await?;
        }
    }
    sqlx::query("DELETE FROM tracks WHERE id=?")
        .bind(id.0)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;

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

/// Picks matches for review tracks where the evidence is strong enough. Holds
/// the workflow slot, because finishing each match downloads covers.
pub async fn auto_approve_review(
    State(s): State<Arc<AppState>>,
) -> ApiResult<Json<AutoApproveResult>> {
    if !s
        .try_claim_workflow(WorkflowPhase::Fetch, "Approving matches in review")
        .await
    {
        return Err(ApiError::conflict(
            "Wait for the current scan or write operation to finish",
        ));
    }
    let result = approve_review_tracks(&s).await;
    s.reset_workflow(WorkflowPhase::Idle, "Ready").await;
    result.map(Json)
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
    let path = std::path::Path::new(&track.path);
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
        .header(header::CONTENT_TYPE, audio_mime(&track))
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

fn audio_mime(track: &Track) -> &'static str {
    match track.format.as_deref().unwrap_or_default() {
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

pub async fn manual_candidate(
    State(s): State<Arc<AppState>>,
    Path(id): Path<TrackId>,
    Json(value): Json<CandidateEdit>,
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
    let mut candidate = Candidate {
        provider: "manual".into(),
        title: value.title,
        artist: value.artist,
        artist_credits: value.artist_credits,
        album: value.album,
        album_artist: value.album_artist,
        album_artist_credits: value.album_artist_credits,
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
    let completion = finalize_selection(&s, &mut candidate, None).await?;
    let candidate_id = candidate_db::insert(&s.pool, id.0, &candidate).await?;
    if completion.core_complete
        && let Some(cover_url) = candidate.cover_url.as_deref()
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
    let (status, stage, message) = readiness_state(&candidate, &completion, "Entered manually");
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
            "Enter a valid HTTPS image, Shazam, Spotify, SoundCloud, Audiomack, Navahang, Radio Javan, or Genius song URL",
        )
    })?;
    if parsed.scheme() != "https" {
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
    persist_artwork_override(
        &s.pool,
        &track.path,
        &selected.title,
        &selected.artist,
        &cover_url,
    )
    .await?;
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
    let completion = finalize_selection(&s, &mut candidate, None).await?;
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
    let (track, candidate) = track_db::selected(&s.pool, id).await?;
    let artwork =
        match crate::application::apply::resolve_artwork(&s, &track.filename, &candidate).await {
            Ok(bytes) => Some(bytes),
            Err(_) => tokio::task::spawn_blocking(move || {
                tag_writer::read_artwork(std::path::Path::new(&track.path))
            })
            .await
            .map_err(anyhow::Error::from)??,
        }
        .ok_or_else(|| ApiError::not_found("No valid artwork is available for this track"))?;
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
    .map_err(anyhow::Error::from)??
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
    let mut urls = candidate.cover_url.into_iter().collect::<Vec<_>>();
    for item in candidate.artwork_candidates {
        if !urls.contains(&item.url) {
            urls.push(item.url);
        }
    }
    for url in urls {
        if let Ok(bytes) =
            providers::cover_art_archive::fetch_url_cached(&s.pool, &s.client, &url).await
            && tag_writer::validate_artwork(&bytes).is_ok()
        {
            return artwork_response(bytes);
        }
    }
    Err(ApiError::not_found(
        "No valid catalog artwork is available for this candidate",
    ))
}

fn artwork_response(artwork: Vec<u8>) -> ApiResult<axum::response::Response> {
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
            "Enter a valid Shazam, Spotify, SoundCloud, Audiomack, Navahang, Radio Javan, Genius, or YouTube URL",
        )
    })?;
    let (_, found) = lookup_source(&s, url, &parsed).await.ok_or_else(|| {
        ApiError::validation("Source lookup failed: this site is not a supported music source")
    })?;
    let mut candidate =
        found.map_err(|error| ApiError::validation(format!("Source lookup failed: {error:#}")))?;
    if candidate.provider == "shazam" {
        enrich_shazam_with_spotify(&s, &mut candidate).await;
    }
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
    let site = ["www.", "m.", "open.", "music.", "play."]
        .iter()
        .find_map(|prefix| host.strip_prefix(prefix))
        .unwrap_or(&host);
    let (pool, client) = (&s.pool, &s.client);
    Some(match site {
        "spotify.com" => ("Spotify", lookup_spotify_source(s, url).await),
        "shazam.com" => (
            "Shazam",
            providers::shazam::lookup_url(pool, client, url).await,
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

async fn lookup_spotify_source(s: &AppState, url: &str) -> Result<Candidate> {
    let cfg = &s.config;
    providers::spotify::lookup_url(
        &s.client,
        &s.spotify_auth,
        &cfg.spotify_client_id,
        &cfg.spotify_client_secret,
        url,
    )
    .await
}

async fn enrich_shazam_with_spotify(s: &AppState, candidate: &mut Candidate) {
    let cfg = &s.config;
    if cfg.spotify_client_id.trim().is_empty() || cfg.spotify_client_secret.trim().is_empty() {
        return;
    }
    let isrcs = candidate.isrc.clone().into_iter().collect::<Vec<_>>();
    let Ok(results) = providers::spotify::search(
        &s.client,
        &s.spotify_auth,
        &cfg.spotify_client_id,
        &cfg.spotify_client_secret,
        &candidate.title,
        Some(&candidate.artist),
        &isrcs,
    )
    .await
    else {
        return;
    };
    let spotify = results.iter().find(|result| {
        candidate
            .isrc
            .as_deref()
            .zip(result.isrc.as_deref())
            .is_some_and(|(left, right)| left.eq_ignore_ascii_case(right))
    });
    let Some(spotify) = spotify.or_else(|| results.first()) else {
        return;
    };
    fill_missing_source_fields(candidate, spotify);
    candidate.score_breakdown = Some(
        serde_json::json!({
            "source": "shazam_user_source_url",
            "sources": ["Shazam", "Spotify"],
            "user_verified_source": true,
            "spotify_catalog_enriched": true,
            "final_score": candidate.score
        })
        .to_string(),
    );
}

fn fill_missing_source_fields(candidate: &mut Candidate, source: &Candidate) {
    macro_rules! fill {
        ($field:ident) => {
            if candidate.$field.is_none() {
                candidate.$field = source.$field.clone();
            }
        };
    }
    fill!(album);
    fill!(album_artist);
    fill!(track_number);
    fill!(track_total);
    fill!(disc_number);
    fill!(disc_total);
    fill!(year);
    fill!(genre);
    fill!(composer);
    fill!(label);
    fill!(isrc);
    fill!(cover_url);
    fill!(release_date);
    fill!(release_type);
    fill!(duration_delta);
    candidate.is_compilation |= source.is_compilation;
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
        let state = Arc::new(AppState::new(Config::default(), pool.clone()));
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
