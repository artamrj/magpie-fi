use super::*;
use crate::infrastructure::db::candidates;

pub(crate) async fn persist_duplicate_members(
    state: &AppState,
    representative: &PreparedInput,
    duplicates: Vec<PreparedInput>,
    total: usize,
) {
    for duplicate in duplicates {
        if state.workflow_cancelled().await {
            return;
        }
        let file = duplicate.path.display().to_string();
        match persist_input_duplicate(&state.pool, &duplicate, &representative.path).await {
            Ok(()) => tracing::info!(
                file,
                kept = %representative.path.display(),
                "skipped duplicate input before online lookup"
            ),
            Err(error) => {
                tracing::error!(file, "could not store duplicate input result: {error:#}")
            }
        }
        finish_scan_progress(state, total).await;
    }
}

/// Stores an automatically selected match: the track becomes Ready.
pub(crate) async fn persist_match(
    pool: &sqlx::SqlitePool,
    path: &Path,
    info: &audio::AudioInfo,
    candidate: &infra_providers::Candidate,
    message: &str,
) -> Result<()> {
    let mut tx = pool.begin().await?;
    let id = upsert_track(
        &mut tx,
        path,
        Some(info),
        "selected",
        "ready",
        Some(message),
        None,
    )
    .await?;
    let candidate_id = candidates::insert(&mut *tx, id, candidate).await?;
    sqlx::query("UPDATE tracks SET selected_candidate_id=? WHERE id=?")
        .bind(candidate_id)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// Stores a recognized track whose only gap is the cover: the match stays
/// chosen (as when the owner picks it), so the page asks for a cover only and
/// the cover finder can complete it later.
pub(crate) async fn persist_waiting_for_cover(
    pool: &sqlx::SqlitePool,
    path: &Path,
    info: &audio::AudioInfo,
    candidates: &[infra_providers::Candidate],
    message: &str,
) -> Result<()> {
    let mut tx = pool.begin().await?;
    let id = upsert_track(
        &mut tx,
        path,
        Some(info),
        "needs_review",
        "review",
        Some(message),
        None,
    )
    .await?;
    let mut chosen = None;
    for candidate in candidates.iter().take(5) {
        let candidate_id = candidates::insert(&mut *tx, id, candidate).await?;
        chosen.get_or_insert(candidate_id);
    }
    sqlx::query("UPDATE tracks SET selected_candidate_id=? WHERE id=?")
        .bind(chosen)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// Stores a track that needs review, with its best candidates.
pub(crate) async fn persist_review(
    pool: &sqlx::SqlitePool,
    path: &Path,
    info: &audio::AudioInfo,
    candidates: &[infra_providers::Candidate],
    message: &str,
) -> Result<()> {
    let mut tx = pool.begin().await?;
    let id = upsert_track(
        &mut tx,
        path,
        Some(info),
        "needs_review",
        "review",
        Some(message),
        None,
    )
    .await?;
    for candidate in candidates.iter().take(5) {
        candidates::insert(&mut *tx, id, candidate).await?;
    }
    tx.commit().await?;
    Ok(())
}

pub(crate) async fn persist_unmatched(
    pool: &sqlx::SqlitePool,
    path: &Path,
    info: &audio::AudioInfo,
    message: &str,
) -> Result<()> {
    persist_outcome(
        pool,
        path,
        Some(info),
        "needs_review",
        "review",
        message,
        None,
    )
    .await
}

pub(crate) async fn persist_failed(
    pool: &sqlx::SqlitePool,
    path: &Path,
    error: &str,
) -> Result<()> {
    persist_outcome(
        pool,
        path,
        None,
        "provider_error",
        "failed",
        "Track failed after retries",
        Some(error),
    )
    .await
}

pub(crate) async fn persist_corrupt(
    pool: &sqlx::SqlitePool,
    path: &Path,
    info: &audio::AudioInfo,
    diagnostic: &str,
) -> Result<()> {
    persist_outcome(
        pool,
        path,
        Some(info),
        "corrupt",
        "failed",
        "Audio file is damaged and cannot be written safely",
        Some(diagnostic),
    )
    .await
}

async fn persist_outcome(
    pool: &sqlx::SqlitePool,
    path: &Path,
    info: Option<&audio::AudioInfo>,
    status: &str,
    stage: &str,
    message: &str,
    error: Option<&str>,
) -> Result<()> {
    let mut tx = pool.begin().await?;
    upsert_track(&mut tx, path, info, status, stage, Some(message), error).await?;
    tx.commit().await?;
    Ok(())
}

pub(crate) async fn persist_analysis_identity(
    pool: &sqlx::SqlitePool,
    input: &PreparedInput,
) -> Result<()> {
    let stamp = files::stamp(&input.path).await.ok();
    sqlx::query(
        "UPDATE tracks SET bitrate=?,file_size=?,file_mtime=?,content_fingerprint=?,updated_at=?
         WHERE path=?",
    )
    .bind(
        input
            .info
            .as_ref()
            .and_then(|info| info.bitrate)
            .map(i64::from),
    )
    .bind(stamp.map(|value| value.0))
    .bind(stamp.map(|value| value.1))
    .bind(&input.content_key)
    .bind(Utc::now().to_rfc3339())
    .bind(input.path.to_string_lossy().as_ref())
    .execute(pool)
    .await?;
    Ok(())
}

pub(crate) async fn persist_input_duplicate(
    pool: &sqlx::SqlitePool,
    input: &PreparedInput,
    representative: &Path,
) -> Result<()> {
    let message = format!("Duplicate of input file: {}", representative.display());
    let mut transaction = pool.begin().await?;
    let id = upsert_track(
        &mut transaction,
        &input.path,
        input.info.as_ref(),
        "duplicate",
        "skipped",
        Some(&message),
        None,
    )
    .await?;
    let stamp = files::stamp(&input.path).await.ok();
    sqlx::query(
        "UPDATE tracks SET output_path=?,bitrate=?,file_size=?,file_mtime=?,content_fingerprint=?
         WHERE id=?",
    )
    .bind(representative.to_string_lossy().as_ref())
    .bind(
        input
            .info
            .as_ref()
            .and_then(|info| info.bitrate)
            .map(i64::from),
    )
    .bind(stamp.map(|value| value.0))
    .bind(stamp.map(|value| value.1))
    .bind(&input.content_key)
    .bind(id)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(())
}

/// Creates or refreshes the track row for `path` with a new scan result. Any
/// previous selection and candidates are dropped. Returns the track id.
async fn upsert_track(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    path: &Path,
    info: Option<&audio::AudioInfo>,
    status: &str,
    stage: &str,
    stage_message: Option<&str>,
    error: Option<&str>,
) -> Result<i64> {
    let now = Utc::now().to_rfc3339();
    let filename = path
        .file_name()
        .and_then(|v| v.to_str())
        .context("invalid filename")?;
    let format = info
        .map(|info| info.format.clone())
        .or_else(|| {
            path.extension()
                .and_then(|extension| extension.to_str())
                .map(str::to_ascii_lowercase)
        })
        .unwrap_or_default();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO tracks(path,filename,format,duration,current_title,current_artist,
           current_album,current_album_artist,current_track_number,selected_candidate_id,status,
           error,is_missing,first_seen_at,last_seen_at,last_scanned_at,stage,stage_message,updated_at)
         VALUES(?,?,?,?,?,?,?,?,?,NULL,?,?,0,?,?,?,?,?,?)
         ON CONFLICT(path) DO UPDATE SET
           filename=excluded.filename,format=excluded.format,duration=excluded.duration,
           current_title=excluded.current_title,current_artist=excluded.current_artist,
           current_album=excluded.current_album,current_album_artist=excluded.current_album_artist,
           current_track_number=excluded.current_track_number,selected_candidate_id=NULL,
           status=excluded.status,error=excluded.error,is_missing=0,
           last_seen_at=excluded.last_seen_at,last_scanned_at=excluded.last_scanned_at,
           stage=excluded.stage,stage_message=excluded.stage_message,updated_at=excluded.updated_at
         RETURNING id",
    )
    .bind(path.to_string_lossy().as_ref())
    .bind(filename)
    .bind(&format)
    .bind(info.map(|info| info.duration))
    .bind(info.and_then(|info| info.title.as_ref()))
    .bind(info.and_then(|info| info.artist.as_ref()))
    .bind(info.and_then(|info| info.album.as_ref()))
    .bind(info.and_then(|info| info.album_artist.as_ref()))
    .bind(info.and_then(|info| info.track_number.map(i64::from)))
    .bind(status)
    .bind(error)
    .bind(&now)
    .bind(&now)
    .bind(&now)
    .bind(stage)
    .bind(stage_message)
    .bind(&now)
    .fetch_one(&mut **tx)
    .await?;
    sqlx::query("DELETE FROM candidates WHERE track_id=?")
        .bind(id)
        .execute(&mut **tx)
        .await?;
    Ok(id)
}
