//! Scan workflow drivers: run_automatic (the inbox check)/retry/run_files plus the shared
//! types the rest of the scan pipeline operates on.

pub(super) use crate::infrastructure::providers as infra_providers;
pub(super) use crate::{
    app::AppState,
    application::{
        canonical_names::normalize_credits,
        files,
        input_dedup::{self, RecordingEvidence},
        smart_approval::{has_audio_recognition, source_count},
    },
    domain::{
        audio,
        matcher::{artist_similarity, normalize_match_key, text_close, title_similarity},
        provider_name,
    },
    infrastructure::{
        fingerprint_cache,
        media::{fingerprint, integrity},
    },
    types::WorkflowPhase,
};
pub(super) use anyhow::{Context, Result};
pub(super) use chrono::Utc;
pub(super) use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
pub(super) use tokio::{
    sync::{Mutex, Semaphore, mpsc},
    task::JoinSet,
};

mod persist;
mod process;
mod providers;

pub(super) use persist::*;
pub(super) use process::*;
pub(super) use providers::*;

pub async fn run_automatic(state: Arc<AppState>) -> Result<usize> {
    if !state
        .try_claim_workflow(WorkflowPhase::Scan, "Checking the inbox")
        .await
    {
        return Ok(0);
    }
    let cfg = &state.config;
    let (files, walk_errors) = discover_files_in_background(PathBuf::from(&cfg.input_dir)).await?;
    let (downloads, _) = discover_files_in_background(cfg.staging_dir.clone()).await?;
    let walked_cleanly = walk_errors.is_empty();
    for error in walk_errors {
        tracing::error!(error, "inbox folder walk error");
    }
    if walked_cleanly && let Ok(inbox) = tokio::fs::canonicalize(&cfg.input_dir).await {
        forget_deleted_songs(&state, &inbox).await?;
    }
    let settle = Duration::from_secs(cfg.inbox_settle_seconds);
    let pending = automatic_pending_files(&state.pool, files, downloads, settle).await?;
    // Found files have their real path, so staging is compared by its real path.
    let staging = tokio::fs::canonicalize(&cfg.staging_dir)
        .await
        .unwrap_or_else(|_| cfg.staging_dir.clone());
    let (pending, more_waiting) = next_batch(pending, &staging);
    if pending.is_empty() {
        state
            .finish_workflow(WorkflowPhase::Idle, "Inbox: nothing new")
            .await;
        return Ok(0);
    }
    let snapshot_files = pending.clone();
    let cancelled = run_files(
        state.clone(),
        pending,
        "Identifying new music from the inbox",
        "Inbox identification complete",
    )
    .await?;
    if cancelled {
        return Ok(0);
    }
    remember_unmarked_files(&state.pool, &snapshot_files).await?;
    if more_waiting {
        // The next batch starts right after this one is filed, without
        // waiting for the next inbox check.
        state.notify_automation_scheduler();
    }
    Ok(snapshot_files.len())
}

/// The files of one inbox check: magpie-fi's own downloads first (someone is
/// waiting for those), then inbox files in path order, so the songs of one
/// album folder stay together. Also says whether more files are left.
fn next_batch(mut pending: Vec<PathBuf>, staging: &Path) -> (Vec<PathBuf>, bool) {
    pending.sort_by_cached_key(|path| (!path.starts_with(staging), path.clone()));
    let more_waiting = pending.len() > crate::config::INBOX_BATCH;
    pending.truncate(crate::config::INBOX_BATCH);
    (pending, more_waiting)
}

pub async fn retry_files(state: Arc<AppState>, files: Vec<PathBuf>) -> Result<()> {
    let snapshot_files = files.clone();
    let cancelled = run_files(
        state.clone(),
        files,
        "Checking files with issues",
        "Issue check complete",
    )
    .await?;
    if !cancelled {
        remember_unmarked_files(&state.pool, &snapshot_files).await?;
    }
    Ok(())
}

/// Clears a song from Review without touching any file: its cached checks, its
/// download entry and its row. When other copies of the same recording were
/// waiting behind it as duplicates, the best one takes its place and its matches.
/// `source` is the file's real path (the recorded path when the file is gone).
pub async fn forget_track(
    pool: &sqlx::SqlitePool,
    track: &crate::infrastructure::db::tracks::Track,
    source: &Path,
) -> Result<()> {
    sqlx::query("UPDATE wanted SET status='removed',error=NULL,updated_at=? WHERE staging_path=?")
        .bind(Utc::now().to_rfc3339())
        .bind(source.to_string_lossy().as_ref())
        .execute(pool)
        .await?;
    let canonical_path = source.to_string_lossy().into_owned();
    let mut transaction = pool.begin().await?;
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
        let duplicate_tracks: Vec<crate::infrastructure::db::tracks::Track> =
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "SELECT {} FROM tracks
             WHERE id!=? AND status='duplicate' AND stage='skipped'
               AND content_fingerprint=? AND is_missing=0
               AND (? LIKE 'sha256:%' OR (duration IS NOT NULL AND ABS(duration - ?) <= 3.0))",
                crate::infrastructure::db::tracks::TRACK_FIELDS
            )))
            .bind(track.id.0)
            .bind(content_key)
            .bind(content_key)
            .bind(track.duration)
            .fetch_all(&mut *transaction)
            .await?;
        let evidence = duplicate_tracks
            .iter()
            .map(|duplicate| input_dedup::RecordingEvidence {
                path: duplicate.path.clone().into(),
                format: duplicate.format.clone().unwrap_or_default(),
                bitrate: duplicate
                    .bitrate
                    .and_then(|value| u32::try_from(value).ok()),
                duration: duplicate.duration,
                content_key: duplicate.content_fingerprint.clone(),
                isrc: None,
                fingerprint: None,
            })
            .collect::<Vec<_>>();
        if let Some(group) = input_dedup::group_recordings(&evidence).first() {
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
            .bind(Utc::now().to_rfc3339())
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
            .bind(Utc::now().to_rfc3339())
            .bind(promoted.id.0)
            .bind(content_key)
            .bind(content_key)
            .bind(track.duration)
            .execute(&mut *transaction)
            .await?;
        }
    }
    sqlx::query("DELETE FROM tracks WHERE id=?")
        .bind(track.id.0)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(())
}

/// Songs waiting in Review whose file you deleted from the inbox yourself are
/// dropped, so the list does not keep asking about files that are gone. Only
/// songs without a chosen match go: a choice you made is kept, and filing
/// reports that file as missing. Nothing happens while `inbox` itself cannot
/// be read (an unplugged disk must not empty Review).
async fn forget_deleted_songs(state: &AppState, inbox: &Path) -> Result<usize> {
    if tokio::fs::read_dir(inbox).await.is_err() {
        return Ok(0);
    }
    let gone = |path: &str| {
        let path = PathBuf::from(path);
        async move {
            matches!(
                tokio::fs::metadata(&path).await,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound
            )
        }
    };
    let fields = crate::infrastructure::db::tracks::TRACK_FIELDS;
    let mut forgotten = 0;
    // Waiting duplicates first, so a deleted copy is never put in a deleted song's place.
    for filter in [
        "status='duplicate' AND stage='skipped'",
        "stage='review' AND selected_candidate_id IS NULL",
    ] {
        let tracks: Vec<crate::infrastructure::db::tracks::Track> = sqlx::query_as(
            sqlx::AssertSqlSafe(format!("SELECT {fields} FROM tracks WHERE {filter}")),
        )
        .fetch_all(&state.pool)
        .await?;
        for track in tracks {
            let path = Path::new(&track.path);
            let ours = path.starts_with(inbox) || path.starts_with(&state.config.staging_dir);
            if !ours || !gone(&track.path).await {
                continue;
            }
            forget_track(&state.pool, &track, path).await?;
            tracing::info!(
                file = track.filename,
                "forgot a song whose file was deleted"
            );
            forgotten += 1;
        }
    }
    Ok(forgotten)
}

/// Audio files under `root` as real paths (the form every track row uses).
fn discover_files(root: &Path) -> (Vec<PathBuf>, Vec<String>) {
    let (files, errors) = files::audio_files(root);
    let mut files = files
        .into_iter()
        .filter_map(|path| path.canonicalize().ok())
        .collect::<Vec<_>>();
    files.sort();
    (files, errors)
}

async fn discover_files_in_background(root: PathBuf) -> Result<(Vec<PathBuf>, Vec<String>)> {
    tokio::task::spawn_blocking(move || discover_files(&root))
        .await
        .context("music folder discovery task failed")
}

/// Inbox and staging files that are new or changed since the last check. An
/// inbox file that changed within `settle` is probably still being copied, so
/// it waits for the next check; magpie-fi's own downloads are complete when they
/// appear in staging. Files that are gone are forgotten.
async fn automatic_pending_files(
    pool: &sqlx::SqlitePool,
    inbox: Vec<PathBuf>,
    downloads: Vec<PathBuf>,
    settle: Duration,
) -> Result<Vec<PathBuf>> {
    let settled_before = SystemTime::now()
        .checked_sub(settle)
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(i64::MAX, |time| {
            i64::try_from(time.as_nanos()).unwrap_or(i64::MAX)
        });
    let rows: Vec<(String, i64, i64, String)> =
        sqlx::query_as("SELECT path,file_size,file_mtime_ns,checked_at FROM automatic_scan_files")
            .fetch_all(pool)
            .await?;
    // Songs that wait for a second look are left alone until the longest
    // pause of a source is over.
    let too_early = |checked_at: &str| {
        chrono::DateTime::parse_from_rfc3339(checked_at).is_ok_and(|checked| {
            Utc::now()
                .signed_duration_since(checked)
                .to_std()
                .unwrap_or_default()
                < SECOND_LOOK_AFTER
        })
    };
    let not_yet = rows
        .iter()
        .filter(|(_, size, _, checked_at)| *size == SECOND_LOOK_PENDING && too_early(checked_at))
        .map(|(path, ..)| path.clone())
        .collect::<std::collections::HashSet<_>>();
    let mut known = rows
        .into_iter()
        .map(|(path, size, modified, _)| (path, (size, modified)))
        .collect::<HashMap<_, _>>();
    let candidates = inbox
        .into_iter()
        .map(|path| (path, settled_before))
        .chain(downloads.into_iter().map(|path| (path, i64::MAX)));
    let mut pending = Vec::new();
    for (path, settled_before) in candidates {
        let previous = known.remove(path.to_string_lossy().as_ref());
        let Ok(stamp) = files::stamp(&path).await else {
            continue;
        };
        if stamp.1 <= settled_before
            && previous != Some(stamp)
            && !not_yet.contains(path.to_string_lossy().as_ref())
        {
            pending.push(path);
        }
    }
    if !known.is_empty() {
        let mut transaction = pool.begin().await?;
        for path in known.keys() {
            sqlx::query("DELETE FROM automatic_scan_files WHERE path=?")
                .bind(path)
                .execute(&mut *transaction)
                .await?;
        }
        transaction.commit().await?;
    }
    Ok(pending)
}

/// How long a song waits for its second look: the longest a source is paused.
const SECOND_LOOK_AFTER: Duration = Duration::from_secs(5 * 60);
/// The size stored for a file that waits for its second look. No file has
/// it, so the file counts as changed and is identified again.
const SECOND_LOOK_PENDING: i64 = -1;

/// Remembers songs that were identified while a source was paused. The ones
/// left unsure (in Review with no match chosen) are marked for one more look;
/// the others, and songs that already had their second look, are remembered
/// as usual. A song is never looked at a third time: a source that stays
/// down must not make the inbox check run in circles.
async fn remember_for_second_look(pool: &sqlx::SqlitePool, files: &[PathBuf]) -> Result<()> {
    for path in files {
        let text = path.to_string_lossy();
        let unsure: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM tracks WHERE path=? AND stage='review'
                           AND selected_candidate_id IS NULL)",
        )
        .bind(text.as_ref())
        .fetch_one(pool)
        .await?;
        let had_second_look: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM automatic_scan_files WHERE path=? AND second_look=1)",
        )
        .bind(text.as_ref())
        .fetch_one(pool)
        .await?;
        if !unsure || had_second_look {
            remember_scanned_files(pool, std::slice::from_ref(path)).await?;
            continue;
        }
        sqlx::query(
            "INSERT INTO automatic_scan_files(path,file_size,file_mtime_ns,checked_at,second_look)
             VALUES(?,?,?,?,1)
             ON CONFLICT(path) DO UPDATE SET
               file_size=excluded.file_size,
               file_mtime_ns=excluded.file_mtime_ns,
               checked_at=excluded.checked_at,
               second_look=1",
        )
        .bind(text.as_ref())
        .bind(SECOND_LOOK_PENDING)
        .bind(SECOND_LOOK_PENDING)
        .bind(Utc::now().to_rfc3339())
        .execute(pool)
        .await?;
        tracing::info!(file = %path.display(), "a source was paused; this song gets a second look");
    }
    Ok(())
}

/// [`remember_scanned_files`] for the end of a run: every file of the run is
/// remembered, except those just marked for a second look.
async fn remember_unmarked_files(pool: &sqlx::SqlitePool, files: &[PathBuf]) -> Result<()> {
    let waiting: Vec<String> =
        sqlx::query_scalar("SELECT path FROM automatic_scan_files WHERE file_size=?")
            .bind(SECOND_LOOK_PENDING)
            .fetch_all(pool)
            .await?;
    let waiting = waiting
        .into_iter()
        .collect::<std::collections::HashSet<_>>();
    let unmarked = files
        .iter()
        .filter(|path| !waiting.contains(path.to_string_lossy().as_ref()))
        .cloned()
        .collect::<Vec<_>>();
    remember_scanned_files(pool, &unmarked).await
}

async fn remember_scanned_files(pool: &sqlx::SqlitePool, files: &[PathBuf]) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    let mut transaction = pool.begin().await?;
    for path in files {
        let Ok((size, modified)) = files::stamp(path).await else {
            continue;
        };
        sqlx::query(
            "INSERT INTO automatic_scan_files(path,file_size,file_mtime_ns,checked_at)
             VALUES(?,?,?,?)
             ON CONFLICT(path) DO UPDATE SET
               file_size=excluded.file_size,
               file_mtime_ns=excluded.file_mtime_ns,
               checked_at=excluded.checked_at",
        )
        .bind(path.to_string_lossy().as_ref())
        .bind(size)
        .bind(modified)
        .bind(&now)
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await?;
    Ok(())
}

async fn run_files(
    state: Arc<AppState>,
    mut files: Vec<PathBuf>,
    starting_message: &'static str,
    completion_message: &'static str,
) -> Result<bool> {
    files.sort();
    files.dedup();
    let total = files.len();
    let limits = Arc::new(PipelineLimits {
        metadata: Semaphore::new(crate::config::SCAN_WORKERS),
        fingerprint: Semaphore::new(crate::config::FINGERPRINT_WORKERS),
        acoustid: Semaphore::new(crate::config::LOOKUP_WORKERS),
        // Two workers keep first-time recognition from serializing the whole
        // library while still avoiding the burst traffic that triggers Shazam
        // rate limits.
        songrec: Semaphore::new(2),
        disabled_providers: Mutex::new(HashMap::new()),
        lookups_missed: std::sync::atomic::AtomicU64::new(0),
    });
    state
        .set_workflow(
            WorkflowPhase::Fetch,
            "Analyzing input recordings for duplicates",
            0,
            total,
            None,
        )
        .await;
    let jobs = prepare_input_jobs(&state, &limits, files).await?;
    if state.workflow_cancelled().await {
        return Ok(true);
    }
    state
        .set_workflow(WorkflowPhase::Fetch, starting_message, 0, total, None)
        .await;
    let scan_workers = crate::config::SCAN_WORKERS.min(total.max(1));
    let (file_tx, file_rx) = mpsc::channel::<FileJob>(scan_workers * 2);
    let file_rx = Arc::new(Mutex::new(file_rx));
    let mut tasks = JoinSet::new();
    for _ in 0..scan_workers {
        let state = state.clone();
        let limits = limits.clone();
        let file_rx = file_rx.clone();
        tasks.spawn(async move {
            loop {
                let job = file_rx.lock().await.recv().await;
                let Some(job) = job else {
                    break;
                };
                let paths = job
                    .members
                    .iter()
                    .map(|member| member.path.clone())
                    .collect::<Vec<_>>();
                // On its own task: a song that makes the identifier panic is
                // marked failed and shown in Issues, and this worker carries
                // on with the next song. (When every worker died, the scan
                // used to wait forever with the workflow slot taken.)
                let missed_before = limits
                    .lookups_missed
                    .load(std::sync::atomic::Ordering::Relaxed);
                let work = AbortOnDrop(tokio::spawn({
                    let (state, limits) = (state.clone(), limits.clone());
                    async move { process_file(&state, &limits, job, total).await }
                }));
                if let Err(error) = work.finished().await {
                    tracing::error!(files = ?paths, "identifying a song stopped unexpectedly: {error}");
                    for path in &paths {
                        let _ = persist_failed(
                            &state.pool,
                            path,
                            "magpie-fi could not read this file (an unexpected error). \
                             The other songs were not affected.",
                        )
                        .await;
                    }
                }
                // Remembered song by song: after a restart or "Stop" only the
                // songs that were not finished are identified again, and the
                // matches already chosen in Review stay chosen. A stopped
                // song is not finished.
                if state.workflow_cancelled().await {
                    break;
                }
                // A source was paused while this song was identified. If the
                // song was left unsure, it is looked at once more when the
                // source is back; otherwise it would wait in Review for good
                // without the answer of what may be its best source.
                let source_missed = limits
                    .lookups_missed
                    .load(std::sync::atomic::Ordering::Relaxed)
                    != missed_before;
                let remembered = if source_missed {
                    remember_for_second_look(&state.pool, &paths).await
                } else {
                    remember_scanned_files(&state.pool, &paths).await
                };
                if let Err(error) = remembered {
                    tracing::warn!(%error, "could not remember a scanned file");
                }
            }
        });
    }
    for job in jobs {
        if state.workflow_cancelled().await {
            break;
        }
        file_tx.send(job).await?;
    }
    drop(file_tx);

    while let Some(result) = tasks.join_next().await {
        if let Err(error) = result {
            tracing::warn!("scan worker task failed: {error:#}");
        }
        if state.workflow_cancelled().await {
            tasks.abort_all();
        }
    }
    let cancelled = state.workflow_cancelled().await;
    if cancelled {
        state
            .finish_workflow(WorkflowPhase::Idle, "Scan stopped")
            .await;
    } else {
        state
            .finish_workflow(WorkflowPhase::Preview, completion_message)
            .await;
    }
    Ok(cancelled)
}

/// A task that stops when its owner is stopped ("Stop" aborts the scan
/// workers, and the song they are working on must stop with them).
struct AbortOnDrop<T>(tokio::task::JoinHandle<T>);

impl<T> AbortOnDrop<T> {
    async fn finished(mut self) -> Result<T, tokio::task::JoinError> {
        (&mut self.0).await
    }
}

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn prepare_input_jobs(
    state: &Arc<AppState>,
    limits: &Arc<PipelineLimits>,
    files: Vec<PathBuf>,
) -> Result<Vec<FileJob>> {
    let total = files.len();
    let mut tasks = JoinSet::new();
    let mut files = files.into_iter();
    let workers = limits.metadata.available_permits().max(1);
    let mut prepared = Vec::new();
    loop {
        while tasks.len() < workers {
            let Some(path) = files.next() else {
                break;
            };
            let state = state.clone();
            let limits = limits.clone();
            tasks.spawn(async move { prepare_input(&state, &limits, path).await });
        }
        let Some(result) = tasks.join_next().await else {
            break;
        };
        let input: PreparedInput = result.context("input duplicate-analysis worker failed")?;
        state
            .set_workflow(
                WorkflowPhase::Fetch,
                "Analyzing input recordings for duplicates",
                prepared.len() + 1,
                total,
                Some(input.path.to_string_lossy().into_owned()),
            )
            .await;
        prepared.push(Some(input));
        if state.workflow_cancelled().await {
            tasks.abort_all();
            break;
        }
    }

    let evidence = prepared
        .iter()
        .flatten()
        .map(|input| RecordingEvidence {
            path: input.path.clone(),
            format: input
                .info
                .as_ref()
                .map(|info| info.format.clone())
                .unwrap_or_default(),
            bitrate: input.info.as_ref().and_then(|info| info.bitrate),
            duration: input.info.as_ref().map(|info| info.duration),
            content_key: input.content_key.clone(),
            isrc: None,
            fingerprint: input.fingerprint.clone(),
        })
        .collect::<Vec<_>>();
    Ok(input_dedup::group_recordings(&evidence)
        .into_iter()
        .map(|group| FileJob {
            members: group
                .members
                .into_iter()
                .filter_map(|index| prepared[index].take())
                .collect(),
        })
        .collect())
}

/// Reads the tags, checks integrity and computes the content key of one input
/// file. The results are reused by the identification step.
async fn prepare_input(state: &AppState, limits: &PipelineLimits, path: PathBuf) -> PreparedInput {
    let info = read_info(limits, &path).await.ok();
    let integrity = match info {
        Some(_) => Some(check_integrity(state, &path).await),
        None => None,
    };
    let healthy = matches!(
        integrity,
        Some(IntegrityCheck::Healthy | IntegrityCheck::Unavailable(_))
    );
    let (content_key, fingerprint) = if healthy {
        match limits.fingerprint.acquire().await {
            Ok(_permit) => match fingerprint_cache::get_or_calculate(&state.pool, &path, || async {
                fingerprint::calculate(&path).await
            })
            .await
            {
                Ok(value) => (
                    input_dedup::fingerprint_key(&value.fingerprint),
                    Some(value.fingerprint),
                ),
                Err(_) => (
                    input_dedup::sha256_cached(&state.pool, &path)
                        .await
                        .ok()
                        .map(|hash| input_dedup::hash_key(&hash)),
                    None,
                ),
            },
            Err(_) => (None, None),
        }
    } else {
        (None, None)
    };
    PreparedInput {
        path,
        info,
        integrity,
        content_key,
        fingerprint,
    }
}

/// Tags and audio properties, read off the async runtime.
pub(super) async fn read_info(limits: &PipelineLimits, path: &Path) -> Result<audio::AudioInfo> {
    let _permit = limits.metadata.acquire().await?;
    let owned = path.to_path_buf();
    tokio::task::spawn_blocking(move || audio::read(&owned))
        .await
        .context("metadata reader task failed")?
        .with_context(|| format!("failed to read metadata from {}", path.display()))
}

pub(super) async fn check_integrity(state: &AppState, path: &Path) -> IntegrityCheck {
    match integrity::check(&state.pool, path).await {
        Ok(integrity::Integrity::Healthy) => IntegrityCheck::Healthy,
        Ok(integrity::Integrity::Corrupt(diagnostic)) => IntegrityCheck::Corrupt(diagnostic),
        Err(error) => IntegrityCheck::Unavailable(format!("{error:#}")),
    }
}

async fn finish_scan_progress(state: &AppState, total: usize) {
    let processed = state.finish_track(total).await;
    state
        .set_workflow(
            WorkflowPhase::Fetch,
            "Matching tracks",
            processed,
            total,
            None,
        )
        .await;
}

pub(super) struct PipelineLimits {
    pub(super) metadata: Semaphore,
    pub(super) fingerprint: Semaphore,
    pub(super) acoustid: Semaphore,
    pub(super) songrec: Semaphore,
    pub(super) disabled_providers: Mutex<HashMap<String, ProviderPause>>,
    /// How often a song went on without a source that was paused for a while
    /// (a rate limit, a timeout).
    pub(super) lookups_missed: std::sync::atomic::AtomicU64,
}

pub(super) struct FileJob {
    pub(super) members: Vec<PreparedInput>,
}

pub(super) struct PreparedInput {
    pub(super) path: PathBuf,
    pub(super) info: Option<audio::AudioInfo>,
    pub(super) integrity: Option<IntegrityCheck>,
    pub(super) content_key: Option<String>,
    pub(super) fingerprint: Option<String>,
}

#[derive(Clone, Debug)]
pub(super) enum IntegrityCheck {
    Healthy,
    Corrupt(String),
    /// The check itself could not run (no FFmpeg, timeout); matching goes on.
    Unavailable(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ProcessOutcome {
    Matched,
    NeedsReview,
    Corrupt,
}

#[derive(Clone, Copy)]
pub(super) struct FingerprintEvidence<'a> {
    pub(super) value: &'a str,
    pub(super) duration: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> PipelineLimits {
        PipelineLimits {
            metadata: Semaphore::new(1),
            fingerprint: Semaphore::new(1),
            acoustid: Semaphore::new(1),
            songrec: Semaphore::new(1),
            disabled_providers: Mutex::new(HashMap::new()),
            lookups_missed: std::sync::atomic::AtomicU64::new(0),
        }
    }

    #[tokio::test]
    async fn a_busy_source_is_paused_and_asked_again_later() {
        let limits = limits();
        let rate_limited = || anyhow::anyhow!("HTTP status client error (429 Too Many Requests)");

        handle_provider_error(&limits, "a.mp3", "itunes", rate_limited()).await;
        assert!(provider_disabled(&limits, "itunes").await);
        assert!(!provider_disabled(&limits, "deezer").await);

        // Another song failing during the same pause does not lengthen it.
        assert_eq!(pause_provider(&limits, "itunes", false).await, None);

        // Once the pause is over the source is asked again; the next failure waits longer.
        limits
            .disabled_providers
            .lock()
            .await
            .get_mut("itunes")
            .unwrap()
            .until = Some(Instant::now());
        assert!(!provider_disabled(&limits, "itunes").await);
        assert_eq!(
            pause_provider(&limits, "itunes", false).await.as_deref(),
            Some("for 60 s")
        );
    }

    #[tokio::test]
    async fn a_refused_key_turns_the_source_off_for_the_scan() {
        let limits = limits();
        let refused = anyhow::anyhow!("HTTP status client error (401 Unauthorized)");
        handle_provider_error(&limits, "a.mp3", "spotify", refused).await;
        assert!(provider_disabled(&limits, "spotify").await);
        assert_eq!(pause_provider(&limits, "spotify", false).await, None);
        assert!(provider_disabled(&limits, "spotify").await);
    }

    #[tokio::test]
    async fn a_source_that_says_slow_down_in_other_words_is_paused_too() {
        for (source, answer) in [
            ("itunes", "HTTP status client error (403 Forbidden) for url"),
            ("deezer", "Deezer error 4: Quota limit exceeded"),
            (
                "genius",
                "HTTP status server error (503 Service Unavailable)",
            ),
            ("shazam", "SongRec: rate limit reached"),
        ] {
            let limits = limits();
            handle_provider_error(&limits, "a.mp3", source, anyhow::anyhow!(answer)).await;
            assert!(provider_disabled(&limits, source).await, "{source}");
        }
        // Any other failure is only logged: the next song asks again.
        let limits = limits();
        let other = anyhow::anyhow!("error decoding response body");
        handle_provider_error(&limits, "a.mp3", "itunes", other).await;
        assert!(!provider_disabled(&limits, "itunes").await);
    }

    #[test]
    fn discovery_skips_hidden_and_nas_folders() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        for path in [
            "Artist/Album/01 - Song.m4a",
            ".incoming/partial.m4a",
            "Artist/.trash/old.mp3",
            "@eaDir/thumb.flac",
            "Artist/._Song.m4a",
        ] {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, b"audio").unwrap();
        }

        let (files, errors) = discover_files(root);

        assert!(errors.is_empty());
        let expected = root
            .join("Artist/Album/01 - Song.m4a")
            .canonicalize()
            .unwrap();
        assert_eq!(files, vec![expected]);
    }

    #[test]
    fn a_big_inbox_is_taken_in_batches_with_downloads_first() {
        let staging = Path::new("/music/.magpie-fi/staging");
        let mut pending = (0..crate::config::INBOX_BATCH + 5)
            .map(|number| PathBuf::from(format!("/music/inbox/Album/{number:04}.mp3")))
            .collect::<Vec<_>>();
        pending.push(staging.join("download.m4a"));

        let (batch, more_waiting) = next_batch(pending, staging);

        assert!(more_waiting);
        assert_eq!(batch.len(), crate::config::INBOX_BATCH);
        assert_eq!(batch[0], staging.join("download.m4a"));
        assert_eq!(batch[1], PathBuf::from("/music/inbox/Album/0000.mp3"));

        let (batch, more_waiting) = next_batch(vec![PathBuf::from("/music/inbox/a.mp3")], staging);
        assert_eq!(batch.len(), 1);
        assert!(!more_waiting);
    }

    #[tokio::test]
    async fn a_song_left_unsure_while_a_source_was_paused_gets_one_second_look() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("second-look.sqlite");
        let pool = crate::infrastructure::db::connect(database.to_str().unwrap())
            .await
            .unwrap();
        let file = |name: &str| {
            let path = directory.path().join(name);
            std::fs::write(&path, name).unwrap();
            path.canonicalize().unwrap()
        };
        let (unsure, chosen, filed) = (file("unsure.mp3"), file("chosen.mp3"), file("filed.mp3"));
        for (path, stage, selected) in [(&unsure, "review", None), (&chosen, "review", Some(7))] {
            sqlx::query(
                "INSERT INTO tracks(path,filename,status,is_missing,first_seen_at,last_seen_at,
                   last_scanned_at,stage,selected_candidate_id)
                 VALUES(?,'x','needs_review',0,'now','now','now',?,?)",
            )
            .bind(path.to_string_lossy().as_ref())
            .bind(stage)
            .bind(selected)
            .execute(&pool)
            .await
            .unwrap();
        }
        let all = vec![unsure.clone(), chosen.clone(), filed.clone()];
        let pending = |pool: sqlx::SqlitePool| {
            let all = all.clone();
            async move {
                automatic_pending_files(&pool, all, vec![], Duration::ZERO)
                    .await
                    .unwrap()
            }
        };

        // A source was paused while all three were identified.
        remember_for_second_look(&pool, &all).await.unwrap();
        // The end of the run does not undo the mark.
        remember_unmarked_files(&pool, &all).await.unwrap();
        // Nothing is asked again right away: the source may still be paused.
        assert!(pending(pool.clone()).await.is_empty());

        // Five minutes later only the unsure song is looked at again. A song
        // with a chosen match or one that is not in Review is left alone.
        sqlx::query("UPDATE automatic_scan_files SET checked_at=?")
            .bind((Utc::now() - chrono::Duration::minutes(6)).to_rfc3339())
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(pending(pool.clone()).await, vec![unsure.clone()]);

        // The source is still down at the second look: no third one.
        remember_for_second_look(&pool, std::slice::from_ref(&unsure))
            .await
            .unwrap();
        sqlx::query("UPDATE automatic_scan_files SET checked_at=?")
            .bind((Utc::now() - chrono::Duration::minutes(6)).to_rfc3339())
            .execute(&pool)
            .await
            .unwrap();
        assert!(pending(pool.clone()).await.is_empty());
    }

    #[tokio::test]
    async fn a_paused_source_counts_the_songs_that_went_on_without_it() {
        let limits = limits();
        let missed = || {
            limits
                .lookups_missed
                .load(std::sync::atomic::Ordering::Relaxed)
        };
        assert!(!provider_disabled(&limits, "itunes").await);
        assert_eq!(missed(), 0);
        let busy = anyhow::anyhow!("HTTP status client error (429 Too Many Requests)");
        handle_provider_error(&limits, "a.mp3", "itunes", busy).await;
        assert_eq!(missed(), 1);
        assert!(provider_disabled(&limits, "itunes").await);
        assert_eq!(missed(), 2);
        // A refused key is off for good; waiting would not help.
        let refused = anyhow::anyhow!("HTTP status client error (401 Unauthorized)");
        handle_provider_error(&limits, "a.mp3", "spotify", refused).await;
        assert!(provider_disabled(&limits, "spotify").await);
        assert_eq!(missed(), 2);
    }

    #[tokio::test]
    async fn automatic_scan_only_returns_new_or_changed_files() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("automatic-files.sqlite");
        let pool = crate::infrastructure::db::connect(database.to_str().unwrap())
            .await
            .unwrap();
        let source = directory.path().join("song.mp3");
        tokio::fs::write(&source, b"first version").await.unwrap();
        let source = source.canonicalize().unwrap();

        let pending = automatic_pending_files(&pool, vec![source.clone()], vec![], Duration::ZERO)
            .await
            .unwrap();
        assert_eq!(pending, vec![source.clone()]);

        remember_scanned_files(&pool, std::slice::from_ref(&source))
            .await
            .unwrap();
        assert!(
            automatic_pending_files(&pool, vec![source.clone()], vec![], Duration::ZERO)
                .await
                .unwrap()
                .is_empty()
        );

        tokio::fs::write(&source, b"a changed and longer version")
            .await
            .unwrap();
        let pending = automatic_pending_files(&pool, vec![source.clone()], vec![], Duration::ZERO)
            .await
            .unwrap();
        assert_eq!(pending, vec![source]);

        // A file that left the inbox is forgotten.
        automatic_pending_files(&pool, vec![], vec![], Duration::ZERO)
            .await
            .unwrap();
        let remembered: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM automatic_scan_files")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(remembered, 0);
    }

    #[tokio::test]
    async fn songs_whose_file_was_deleted_from_the_inbox_leave_review() {
        let directory = tempfile::tempdir().unwrap();
        let inbox = directory.path().join("inbox");
        tokio::fs::create_dir(&inbox).await.unwrap();
        let inbox = inbox.canonicalize().unwrap();
        let kept = inbox.join("still-here.mp3");
        tokio::fs::write(&kept, b"audio").await.unwrap();
        let database = directory.path().join("deleted.sqlite");
        let pool = crate::infrastructure::db::connect(database.to_str().unwrap())
            .await
            .unwrap();
        let state = AppState::new(
            crate::config::Config {
                input_dir: inbox.to_string_lossy().into_owned(),
                ..Default::default()
            },
            pool.clone(),
        );
        let add = |name: &'static str, path: String, status: &'static str, stage: &'static str| {
            let pool = pool.clone();
            async move {
                sqlx::query(
                    "INSERT INTO tracks(path,filename,status,is_missing,first_seen_at,
                     last_seen_at,last_scanned_at,stage) VALUES(?,?,?,0,'now','now','now',?)",
                )
                .bind(path)
                .bind(name)
                .bind(status)
                .bind(stage)
                .execute(&pool)
                .await
                .unwrap()
                .last_insert_rowid()
            }
        };
        let path = |name: &str| inbox.join(name).to_string_lossy().into_owned();
        add(
            "still-here.mp3",
            path("still-here.mp3"),
            "needs_review",
            "review",
        )
        .await;
        add("deleted.mp3", path("deleted.mp3"), "needs_review", "review").await;
        add(
            "deleted-copy.mp3",
            path("deleted-copy.mp3"),
            "duplicate",
            "skipped",
        )
        .await;
        // A deleted file elsewhere is not the inbox's business.
        add(
            "elsewhere.mp3",
            "/somewhere/else.mp3".into(),
            "needs_review",
            "review",
        )
        .await;
        // A match you already chose is kept; filing reports the missing file.
        let chosen = add("chosen.mp3", path("chosen.mp3"), "selected", "review").await;
        let candidate = sqlx::query(
            "INSERT INTO candidates(track_id,provider,title,artist,score) VALUES(?,'manual','Song','Artist',100.0)",
        )
        .bind(chosen)
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_rowid();
        sqlx::query("UPDATE tracks SET selected_candidate_id=? WHERE id=?")
            .bind(candidate)
            .bind(chosen)
            .execute(&pool)
            .await
            .unwrap();

        // An inbox that cannot be read (an unplugged disk) changes nothing.
        let unplugged = directory.path().join("not-mounted");
        assert_eq!(forget_deleted_songs(&state, &unplugged).await.unwrap(), 0);

        assert_eq!(forget_deleted_songs(&state, &inbox).await.unwrap(), 2);
        let mut left: Vec<String> = sqlx::query_scalar("SELECT filename FROM tracks")
            .fetch_all(&pool)
            .await
            .unwrap();
        left.sort();
        assert_eq!(left, ["chosen.mp3", "elsewhere.mp3", "still-here.mp3"]);
        assert!(kept.exists());
    }

    #[tokio::test]
    async fn inbox_files_still_being_copied_wait_for_the_next_check() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("settle.sqlite");
        let pool = crate::infrastructure::db::connect(database.to_str().unwrap())
            .await
            .unwrap();
        let source = directory.path().join("copying.flac");
        tokio::fs::write(&source, b"half a file").await.unwrap();

        let pending =
            automatic_pending_files(&pool, vec![source.clone()], vec![], Duration::from_secs(60))
                .await
                .unwrap();
        assert!(pending.is_empty());
        // magpie-fi's own downloads skip the wait.
        let pending = automatic_pending_files(&pool, vec![], vec![source], Duration::from_secs(60))
            .await
            .unwrap();
        assert_eq!(pending.len(), 1);
    }

    async fn test_pool() -> sqlx::SqlitePool {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("scan-pipeline.sqlite");
        let pool = crate::infrastructure::db::connect(path.to_str().unwrap())
            .await
            .unwrap();
        std::mem::forget(dir);
        pool
    }

    #[tokio::test]
    async fn persist_unmatched_upserts_review_track_with_metadata() {
        let pool = test_pool().await;
        let path = Path::new("/music/input/unmatched.mp3");
        let info = audio::AudioInfo {
            title: Some("Current title".into()),
            artist: Some("Current artist".into()),
            album: Some("Current album".into()),
            album_artist: Some("Current album artist".into()),
            track_number: Some(7),
            genre: Some("Rock".into()),
            isrc: None,
            duration: 181.4,
            bitrate: Some(320),
            format: "mp3".into(),
        };

        persist_unmatched(&pool, path, &info, "No candidate met threshold 90")
            .await
            .unwrap();

        let row: (String, String, Option<String>, Option<String>, Option<i64>) =
            sqlx::query_as("SELECT stage,status,stage_message,current_title,selected_candidate_id FROM tracks WHERE path=?")
                .bind(path.to_string_lossy().as_ref())
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(row.0, "review");
        assert_eq!(row.1, "needs_review");
        assert_eq!(row.2.as_deref(), Some("No candidate met threshold 90"));
        assert_eq!(row.3.as_deref(), Some("Current title"));
        assert_eq!(row.4, None);
    }

    #[tokio::test]
    async fn persist_failed_upserts_failed_track_with_error() {
        let pool = test_pool().await;
        let path = Path::new("/music/input/broken.flac");
        persist_failed(&pool, path, "fpcalc failed: command not found")
            .await
            .unwrap();

        let row: (String, String, Option<String>, Option<String>, Option<i64>) =
            sqlx::query_as("SELECT stage,status,stage_message,error,selected_candidate_id FROM tracks WHERE path=?")
                .bind(path.to_string_lossy().as_ref())
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(row.0, "failed");
        assert_eq!(row.1, "provider_error");
        assert_eq!(row.2.as_deref(), Some("Track failed after retries"));
        assert_eq!(row.3.as_deref(), Some("fpcalc failed: command not found"));
        assert_eq!(row.4, None);
    }

    #[tokio::test]
    async fn persist_corrupt_blocks_selection_and_keeps_readable_metadata() {
        let pool = test_pool().await;
        let path = Path::new("/music/input/damaged.mp3");
        let info = audio::AudioInfo {
            title: Some("Readable title".into()),
            artist: Some("Readable artist".into()),
            duration: 42.0,
            format: "mp3".into(),
            ..Default::default()
        };
        persist_corrupt(&pool, path, &info, "Invalid frame header")
            .await
            .unwrap();

        let row: (String, String, Option<String>, Option<String>, Option<i64>) =
            sqlx::query_as("SELECT stage,status,error,current_title,selected_candidate_id FROM tracks WHERE path=?")
                .bind(path.to_string_lossy().as_ref())
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(row.0, "failed");
        assert_eq!(row.1, "corrupt");
        assert_eq!(row.2.as_deref(), Some("Invalid frame header"));
        assert_eq!(row.3.as_deref(), Some("Readable title"));
        assert_eq!(row.4, None);
    }

    #[test]
    fn source_agreement_adds_provider_evidence_and_score() {
        let mut candidates = vec![
            infra_providers::Candidate {
                provider: "discogs".into(),
                title: "Song".into(),
                artist: "Artist".into(),
                album: Some("Album".into()),
                score: 82.0,
                score_breakdown: Some(serde_json::json!({"sources":["Discogs"]}).to_string()),
                ..Default::default()
            },
            infra_providers::Candidate {
                provider: "lastfm".into(),
                title: "Song".into(),
                artist: "Artist".into(),
                album: Some("Album".into()),
                score: 80.0,
                score_breakdown: Some(serde_json::json!({"sources":["Last.fm"]}).to_string()),
                ..Default::default()
            },
        ];

        apply_source_agreement(&mut candidates).unwrap();

        assert!(candidates[0].score > 82.0);
        let why: serde_json::Value =
            serde_json::from_str(candidates[0].score_breakdown.as_deref().unwrap()).unwrap();
        assert!(
            why["sources"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("Discogs"))
        );
        assert!(
            why["sources"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("Last.fm"))
        );
    }

    #[test]
    fn artwork_fallbacks_keep_matching_catalog_covers_only() {
        let mut candidates = vec![
            infra_providers::Candidate {
                provider: "musicbrainz".into(),
                title: "Song".into(),
                artist: "Artist".into(),
                album: Some("Album".into()),
                cover_url: Some("https://example.test/musicbrainz.jpg".into()),
                score: 94.0,
                ..Default::default()
            },
            infra_providers::Candidate {
                provider: "itunes".into(),
                title: "Song".into(),
                artist: "Artist".into(),
                album: Some("Album".into()),
                cover_url: Some("https://example.test/correct.jpg".into()),
                score: 91.0,
                ..Default::default()
            },
            infra_providers::Candidate {
                provider: "youtube".into(),
                title: "Song".into(),
                artist: "Different Artist".into(),
                cover_url: Some("https://example.test/wrong.jpg".into()),
                score: 60.0,
                ..Default::default()
            },
        ];

        enrich_artwork_fallbacks(&mut candidates);

        assert_eq!(
            candidates[0].cover_url.as_deref(),
            Some("https://example.test/correct.jpg")
        );
        let urls = candidates[0]
            .artwork_candidates
            .iter()
            .map(|item| item.url.as_str())
            .collect::<Vec<_>>();
        assert!(urls.contains(&"https://example.test/correct.jpg"));
        assert!(!urls.contains(&"https://example.test/wrong.jpg"));

        // Each candidate knows which cover came with its own catalog record,
        // including the YouTube row that agrees with nobody else.
        let own = |candidate: &infra_providers::Candidate| {
            candidate
                .artwork_candidates
                .iter()
                .filter(|item| item.own_record)
                .map(|item| item.url.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            own(&candidates[0]),
            ["https://example.test/musicbrainz.jpg"]
        );
        assert_eq!(own(&candidates[2]), ["https://example.test/wrong.jpg"]);
    }

    #[test]
    fn provider_only_candidate_without_agreement_is_not_safe_auto_select() {
        let candidate = infra_providers::Candidate {
            provider: "discogs".into(),
            title: "Song".into(),
            artist: "Artist".into(),
            score: 88.0,
            duration_delta: Some(1.0),
            score_breakdown: Some(
                serde_json::json!({
                    "acoustid": 0.0,
                    "sources": ["Discogs"]
                })
                .to_string(),
            ),
            ..Default::default()
        };

        assert!(!has_audio_recognition(&candidate));
        assert_eq!(source_count(&candidate), 1);
        assert_eq!(candidate_trust_tier(&candidate), 0);
    }

    #[test]
    fn fingerprint_backed_musicbrainz_candidate_outranks_noisy_external_results() {
        let mut candidates = vec![
            infra_providers::Candidate {
                provider: "lastfm".into(),
                title: "Out of Time".into(),
                artist: "The Weeknd".into(),
                score: 96.0,
                duration_delta: Some(1.0),
                score_breakdown: Some(serde_json::json!({"sources":["Last.fm"]}).to_string()),
                ..Default::default()
            },
            infra_providers::Candidate {
                provider: "musicbrainz".into(),
                title: "Out of Time".into(),
                artist: "The Weeknd".into(),
                album: Some("Dawn FM".into()),
                score: 94.0,
                duration_delta: Some(1.0),
                score_breakdown: Some(
                    serde_json::json!({
                        "acoustid": 0.98,
                        "sources": ["AcoustID", "MusicBrainz"]
                    })
                    .to_string(),
                ),
                ..Default::default()
            },
        ];

        sort_candidates_for_decision(&mut candidates);

        assert_eq!(candidates[0].provider, "musicbrainz");
        assert!(has_audio_recognition(&candidates[0]));
        let decision = crate::application::smart_approval::rank(
            crate::application::smart_approval::TrackEvidence {
                filename: "The Weeknd - Out of Time.mp3",
                title: Some("Out of Time"),
                artist: Some("The Weeknd"),
                album: Some("Dawn FM"),
                isrc: None,
            },
            &candidates,
        )
        .unwrap();
        assert_eq!(candidates[decision.candidate_index].provider, "musicbrainz");
    }

    #[test]
    fn dedupe_candidates_keeps_strongest_duplicate() {
        let mut candidates = vec![
            infra_providers::Candidate {
                provider: "lastfm".into(),
                title: "Song!".into(),
                artist: "Artist".into(),
                album: Some("Album".into()),
                score: 72.0,
                ..Default::default()
            },
            infra_providers::Candidate {
                provider: "discogs".into(),
                title: "Song".into(),
                artist: "Artist".into(),
                album: Some("Album".into()),
                score: 80.0,
                ..Default::default()
            },
        ];

        dedupe_candidates(&mut candidates);

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].provider, "discogs");
    }

    #[tokio::test]
    async fn a_sure_match_without_a_cover_stays_chosen_while_it_waits() {
        let pool = test_pool().await;
        let path = Path::new("/music/input/marie.mp3");
        let info = audio::AudioInfo {
            title: Some("Marie".into()),
            artist: Some("AnnenMayKantereit".into()),
            duration: 225.0,
            format: "mp3".into(),
            ..Default::default()
        };
        let chosen = infra_providers::Candidate {
            provider: "musicbrainz".into(),
            title: "Marie".into(),
            artist: "AnnenMayKantereit".into(),
            album: Some("Schlagschatten (Sessions / Video Album)".into()),
            artwork_status: infra_providers::ArtworkStatus::CoverRequired,
            score: 89.0,
            raw_json: "{}".into(),
            ..Default::default()
        };
        let other = infra_providers::Candidate {
            provider: "itunes".into(),
            album: Some("Schlagschatten".into()),
            ..chosen.clone()
        };

        persist_waiting_for_cover(&pool, path, &info, &[chosen, other], "waiting")
            .await
            .unwrap();

        let (stage, album): (String, String) = sqlx::query_as(
            "SELECT tracks.stage,candidates.album FROM tracks
             JOIN candidates ON candidates.id=tracks.selected_candidate_id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(stage, "review");
        assert_eq!(album, "Schlagschatten (Sessions / Video Album)");
    }

    #[tokio::test]
    async fn persist_review_keeps_non_musicbrainz_provider() {
        let pool = test_pool().await;
        let path = Path::new("/music/input/discogs-review.mp3");
        let info = audio::AudioInfo {
            title: Some("Song".into()),
            artist: Some("Artist".into()),
            duration: 180.0,
            format: "mp3".into(),
            ..Default::default()
        };
        let candidates = vec![infra_providers::Candidate {
            provider: "discogs".into(),
            title: "Song".into(),
            artist: "Artist".into(),
            score: 84.0,
            raw_json: "{}".into(),
            ..Default::default()
        }];

        persist_review(&pool, path, &info, &candidates, "Review required")
            .await
            .unwrap();

        let provider: String = sqlx::query_scalar("SELECT provider FROM candidates")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(provider, "discogs");
    }

    #[test]
    fn exact_single_keeps_known_album_track_context() {
        let info = audio::AudioInfo {
            title: Some("Khaar".into()),
            artist: Some("Amir Tataloo".into()),
            album: Some("Barzakh".into()),
            track_number: Some(11),
            duration: 427.102,
            ..Default::default()
        };
        let mut candidate = infra_providers::Candidate {
            provider: "itunes".into(),
            title: "Khaar".into(),
            artist: "Amir Tataloo".into(),
            album: Some("Khaar - Single".into()),
            track_number: Some(1),
            track_total: Some(1),
            duration_delta: Some(427.076),
            release_id: Some("single-release".into()),
            ..Default::default()
        };
        preserve_album_context_for_catalog_single(&mut candidate, &info);
        assert_eq!(candidate.album.as_deref(), Some("Barzakh"));
        assert_eq!(candidate.track_number, Some(11));
        assert_eq!(candidate.track_total, None);
        assert_eq!(candidate.release_id, None);
    }

    #[test]
    fn ignores_filename_filler_words_when_matching_title() {
        assert!(title_similarity("Such A Lonely Day", "Lonely Day") >= 0.94);
    }

    #[test]
    fn reported_lonely_day_catalog_match_scores_as_reliable() {
        let info = audio::AudioInfo {
            title: Some("Such A Lonely Day".into()),
            artist: Some("System Of A Down".into()),
            duration: 167.993,
            ..Default::default()
        };
        let mut candidate = infra_providers::Candidate {
            provider: "itunes".into(),
            title: "Lonely Day".into(),
            artist: "System Of A Down".into(),
            album: Some("Hypnotize".into()),
            duration_delta: Some(167.907),
            ..Default::default()
        };

        score_text_candidate(&mut candidate, &info, "itunes_catalog_search").unwrap();

        assert!(candidate.score >= 90.0);
        assert!(candidate.duration_delta.is_some_and(|delta| delta < 1.0));
    }

    #[test]
    fn the_apple_number_is_read_from_the_link_a_download_was_asked_for_with() {
        use super::providers::requested_apple_song;
        assert_eq!(
            requested_apple_song(
                "https://music.apple.com/us/album/donyaye-movazi/1845811122?i=1845811123&uo=4"
            ),
            Some(("1845811123".into(), Some("us".into())))
        );
        assert_eq!(
            requested_apple_song("https://www.shazam.com/song/1845811123/donyaye-movazi"),
            Some(("1845811123".into(), None))
        );
        // A whole album, Shazam's own older numbers and other services name no Apple song.
        for url in [
            "https://music.apple.com/us/album/donyaye-movazi-single/1845811122",
            "https://www.shazam.com/track/54321/some-song",
            "https://open.spotify.com/track/4uLU6hMCjMI75M1A2tKUQC",
        ] {
            assert_eq!(requested_apple_song(url), None, "{url}");
        }
    }

    #[test]
    fn the_linked_song_is_chosen_over_same_named_songs_by_other_singers() {
        // Apple's search for "Donyaye Movazi" lists only other singers' songs;
        // the linked one comes from the lookup by number.
        let info = audio::AudioInfo {
            title: Some("Donyaye Movazi".into()),
            artist: Some("Tqvis & Ferap".into()),
            album: Some("Donyaye Movazi - Single".into()),
            duration: 201.0,
            ..Default::default()
        };
        let apple = |artist: &str, seconds: f64| infra_providers::Candidate {
            provider: "itunes".into(),
            title: "Donyaye Movazi".into(),
            artist: artist.into(),
            album: Some("Donyaye Movazi - Single".into()),
            duration_delta: Some(seconds),
            ..Default::default()
        };
        let mut candidates = vec![
            apple("Maria", 199.871),
            apple("Hamed Monavari", 217.8),
            apple("Tqvis & Ferap", 201.29),
        ];
        score_text_candidate(&mut candidates[0], &info, "itunes_catalog_search").unwrap();
        score_text_candidate(&mut candidates[1], &info, "itunes_catalog_search").unwrap();
        score_text_candidate(&mut candidates[2], &info, "itunes_requested_link").unwrap();
        apply_source_agreement(&mut candidates).unwrap();
        dedupe_candidates(&mut candidates);
        sort_candidates_for_decision(&mut candidates);

        assert_eq!(candidates[0].artist, "Tqvis & Ferap");
        let decision = crate::application::smart_approval::rank(
            crate::application::smart_approval::TrackEvidence {
                filename: "w12.opus",
                title: info.title.as_deref(),
                artist: info.artist.as_deref(),
                album: info.album.as_deref(),
                isrc: None,
            },
            &candidates,
        )
        .unwrap();
        assert_eq!(candidates[decision.candidate_index].artist, "Tqvis & Ferap");
    }

    #[test]
    fn genius_is_reserved_for_incomplete_or_weak_catalog_results() {
        let complete = infra_providers::Candidate {
            score: 94.0,
            album: Some("Hypnotize".into()),
            cover_url: Some("https://example.test/hypnotize.jpg".into()),
            ..Default::default()
        };
        let missing_cover = infra_providers::Candidate {
            cover_url: None,
            ..complete.clone()
        };
        assert!(!needs_genius_enrichment(&[complete]));
        assert!(needs_genius_enrichment(&[missing_cover]));
        assert!(needs_genius_enrichment(&[]));
    }
}
