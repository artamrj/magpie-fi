use crate::{
    app::AppState,
    application::{
        files,
        input_dedup::{self, RecordingEvidence},
        library,
    },
    infrastructure::{
        db::{
            self,
            tracks::{TRACK_FIELDS, Track, candidates_by_track, selected_for_tracks},
        },
        fingerprint_cache, lyrics,
        media::{fingerprint, rsgain, tag_writer},
        providers::Candidate,
    },
    types::{TrackId, WorkflowPhase},
};
use anyhow::{Context, Result, anyhow, bail};
use chrono::Utc;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::Arc,
};

/// Tracks that are identified, complete and waiting to be filed.
const READY: &str =
    "selected_candidate_id IS NOT NULL AND is_missing=0 AND status!='corrupt' AND stage='ready'";

#[derive(Default)]
pub(crate) struct PreparedApply {
    pub(crate) items: Vec<PreviewItem>,
    pub(crate) selected_count: usize,
    pub(crate) outputs: usize,
    pub(crate) duplicates_skipped: usize,
}

pub(crate) struct PreviewItem {
    track_id: TrackId,
    filename: String,
    current_path: String,
    destination_path: String,
    candidate: Candidate,
    evidence: RecordingEvidence,
    duplicates: Vec<DuplicateSource>,
}

struct DuplicateSource {
    track_id: TrackId,
    filename: String,
    current_path: String,
    source_missing: bool,
}

struct PlannedSource {
    track: Track,
    candidate: Option<Candidate>,
    available: bool,
}

/// Where the track goes in the library (docs/BLUEPRINT.md §5). Songs of an album
/// that is already in the library join its folder and year. The format is
/// sniffed again from the file, because an old scan may have trusted a wrong
/// extension (for example AAC audio in a file named `.mp3`).
async fn destination(
    output_dir: &str,
    track: &Track,
    candidate: &mut Candidate,
) -> Result<PathBuf> {
    let library = PathBuf::from(output_dir);
    let source = PathBuf::from(&track.path);
    let stored_format = track.format.clone();
    let mut aligned = candidate.clone();
    let (aligned, destination) = tokio::task::spawn_blocking(move || {
        library::align_with_existing_release(&library, &mut aligned);
        let extension = crate::domain::audio::read(&source)
            .ok()
            .map(|info| info.format)
            .into_iter()
            .chain(stored_format)
            .chain(
                source
                    .extension()
                    .and_then(|value| value.to_str())
                    .map(str::to_owned),
            )
            .find(|value| !value.is_empty())
            .ok_or_else(|| anyhow!("cannot tell the audio format of {}", source.display()))?;
        // WAV and AIFF become FLAC when filed (see `convert_to_flac`).
        let extension = if matches!(
            extension.to_ascii_lowercase().as_str(),
            "wav" | "aiff" | "aif"
        ) {
            "flac".to_owned()
        } else {
            extension
        };
        let destination = library::destination(&library, &aligned, &extension);
        Ok::<_, anyhow::Error>((aligned, destination))
    })
    .await??;
    *candidate = aligned;
    Ok(destination)
}

async fn has_ready_tracks(pool: &sqlx::SqlitePool) -> Result<bool> {
    Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT EXISTS(SELECT 1 FROM tracks WHERE {READY})"
    )))
    .fetch_one(pool)
    .await?)
}

pub(crate) async fn prepare_apply(s: &AppState) -> Result<PreparedApply> {
    let ready_tracks: Vec<Track> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {TRACK_FIELDS} FROM tracks WHERE {READY}"
    )))
    .fetch_all(&s.pool)
    .await?;
    if ready_tracks.is_empty() {
        return Ok(PreparedApply::default());
    }
    let by_track = candidates_by_track(&s.pool, ready_tracks.iter().map(|track| track.id)).await?;
    let selected = selected_for_tracks(&s.pool, ready_tracks).await?;
    let skipped_tracks: Vec<Track> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {TRACK_FIELDS} FROM tracks
         WHERE status='duplicate' AND stage='skipped' AND is_missing=0
           AND content_fingerprint IS NOT NULL
         ORDER BY path"
    )))
    .fetch_all(&s.pool)
    .await?;

    let mut sources = selected
        .into_iter()
        .map(|(track, candidate)| PlannedSource {
            track,
            candidate: Some(candidate),
            available: false,
        })
        .chain(skipped_tracks.into_iter().map(|track| PlannedSource {
            track,
            candidate: None,
            available: false,
        }))
        .collect::<Vec<_>>();

    // Reapply release and identity rules at the last mutable boundary. This
    // also repairs ready-but-unwritten selections created by an older scan.
    for source in &mut sources {
        let PlannedSource {
            track,
            candidate: Some(candidate),
            ..
        } = source
        else {
            continue;
        };
        crate::application::metadata_completion::complete(
            candidate,
            by_track
                .get(&track.id)
                .map(Vec::as_slice)
                .unwrap_or_default(),
            track.current_album.as_deref(),
        );
        // Filing is where new artists are learned (with MusicBrainz for
        // Persian-script names, so they join their Latin spelling).
        crate::application::canonical_names::canonicalize_candidates(
            &s.pool,
            Some(&s.client),
            std::slice::from_mut(candidate),
            crate::application::artists::Mode::Learn,
        )
        .await?;
        db::candidates::update(&s.pool, candidate).await?;
    }

    let mut evidence = Vec::with_capacity(sources.len());
    for source in &mut sources {
        source.available = tokio::fs::metadata(&source.track.path)
            .await
            .is_ok_and(|metadata| metadata.is_file());
        if source.available {
            evidence
                .push(recording_evidence(&s.pool, &source.track, source.candidate.as_ref()).await?);
            continue;
        }
        sqlx::query(
            "UPDATE tracks SET is_missing=1,status='failed',stage='failed',
             stage_message='Source file is missing; restore it and scan again',updated_at=?
             WHERE id=?",
        )
        .bind(Utc::now().to_rfc3339())
        .bind(source.track.id.0)
        .execute(&s.pool)
        .await?;
        evidence.push(RecordingEvidence {
            path: source.track.path.clone().into(),
            format: source.track.format.clone().unwrap_or_default(),
            bitrate: source
                .track
                .bitrate
                .and_then(|value| u32::try_from(value).ok()),
            duration: source.track.duration,
            content_key: source.track.content_fingerprint.clone(),
            isrc: source
                .candidate
                .as_ref()
                .and_then(|candidate| candidate.isrc.clone()),
            fingerprint: None,
        });
    }

    let mut prepared = PreparedApply::default();
    for group in input_dedup::group_recordings(&evidence) {
        let Some(candidate_owner) = group
            .members
            .iter()
            .copied()
            .find(|index| sources[*index].candidate.is_some())
        else {
            continue;
        };
        let Some(representative) = group
            .members
            .iter()
            .copied()
            .find(|index| sources[*index].available)
        else {
            continue;
        };
        let available = group
            .members
            .iter()
            .filter(|index| sources[**index].available)
            .count();
        match plan_item(
            s,
            &sources,
            &evidence,
            group.members,
            candidate_owner,
            representative,
        )
        .await
        {
            Ok(item) => {
                prepared.selected_count += available;
                prepared.items.push(item);
            }
            // One bad track must not hold up the others.
            Err(error) => {
                let track = &sources[representative].track;
                tracing::error!(file = track.path, "could not plan filing: {error:#}");
                mark_failed(&s.pool, track.id, &error).await?;
            }
        }
    }
    align_planned_albums(&s.config.output_dir, &mut prepared.items);
    prepared.outputs = prepared.items.len();
    prepared.duplicates_skipped = prepared.selected_count.saturating_sub(prepared.outputs);
    Ok(prepared)
}

/// Songs of one new album filed in the same batch get the year of the first
/// one, so they share one folder and Navidrome never splits the album.
fn align_planned_albums(output_dir: &str, items: &mut [PreviewItem]) {
    let library = Path::new(output_dir);
    let mut first_year = std::collections::HashMap::<(PathBuf, String, &str), String>::new();
    for item in items {
        let destination = PathBuf::from(&item.destination_path);
        let (Some(artist_folder), Some(album), Some(year)) = (
            destination.parent().and_then(Path::parent),
            item.candidate.album.as_deref(),
            library::release_year(&item.candidate),
        ) else {
            continue;
        };
        let key = (
            artist_folder.to_path_buf(),
            crate::domain::names::match_key(album),
            library::release_suffix(&item.candidate),
        );
        let first = first_year.entry(key).or_insert_with(|| year.clone());
        if *first == year {
            continue;
        }
        item.candidate.year = Some(first.clone());
        if item
            .candidate
            .release_date
            .as_deref()
            .is_some_and(|date| !date.starts_with(first.as_str()))
        {
            item.candidate.release_date = None;
        }
        let extension = destination
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("mp3");
        item.destination_path = library::destination(library, &item.candidate, extension)
            .to_string_lossy()
            .into_owned();
    }
}

/// One output per group of duplicate inputs: the best available file carries
/// the selected candidate, the others are filed as its duplicates.
async fn plan_item(
    s: &AppState,
    sources: &[PlannedSource],
    evidence: &[RecordingEvidence],
    members: Vec<usize>,
    candidate_owner: usize,
    representative: usize,
) -> Result<PreviewItem> {
    let mut candidate = sources[candidate_owner]
        .candidate
        .clone()
        .context("duplicate group has no selected candidate")?;
    if representative != candidate_owner {
        promote_apply_representative(
            &s.pool,
            &sources[candidate_owner].track,
            &sources[representative].track,
            sources[candidate_owner].available,
        )
        .await?;
    }
    let track = &sources[representative].track;
    let destination = destination(&s.config.output_dir, track, &mut candidate).await?;
    let duplicates = members
        .into_iter()
        .filter(|index| *index != representative)
        .map(|index| DuplicateSource {
            track_id: sources[index].track.id,
            filename: sources[index].track.filename.clone(),
            current_path: sources[index].track.path.clone(),
            source_missing: !sources[index].available,
        })
        .collect();
    let mut item_evidence = evidence[representative].clone();
    if item_evidence.isrc.is_none() {
        item_evidence.isrc = candidate.isrc.clone();
    }
    Ok(PreviewItem {
        track_id: track.id,
        filename: track.filename.clone(),
        current_path: track.path.clone(),
        destination_path: destination.to_string_lossy().into_owned(),
        candidate,
        evidence: item_evidence,
        duplicates,
    })
}

/// Takes the track out of the ready set so a failing file is not retried
/// every inbox cycle; "Retry issues" picks it up again.
async fn mark_failed(
    pool: &sqlx::SqlitePool,
    track_id: TrackId,
    error: &anyhow::Error,
) -> Result<()> {
    sqlx::query(
        "UPDATE tracks SET status='failed',stage='failed',error=?,
         stage_message='Filing failed; check the file and retry',updated_at=? WHERE id=?",
    )
    .bind(format!("{error:#}"))
    .bind(Utc::now().to_rfc3339())
    .bind(track_id.0)
    .execute(pool)
    .await?;
    Ok(())
}

async fn promote_apply_representative(
    pool: &sqlx::SqlitePool,
    candidate_owner: &Track,
    representative: &Track,
    owner_available: bool,
) -> Result<()> {
    let candidate_id = candidate_owner
        .selected_candidate_id
        .ok_or_else(|| anyhow!("duplicate representative has no selected candidate"))?;
    let now = Utc::now().to_rfc3339();
    let mut transaction = pool.begin().await?;
    sqlx::query("DELETE FROM candidates WHERE track_id=?")
        .bind(representative.id.0)
        .execute(&mut *transaction)
        .await?;
    sqlx::query("UPDATE candidates SET track_id=? WHERE track_id=?")
        .bind(representative.id.0)
        .bind(candidate_owner.id.0)
        .execute(&mut *transaction)
        .await?;
    sqlx::query(
        "UPDATE tracks SET output_path=NULL,selected_candidate_id=?,status='selected',stage='ready',
         is_missing=0,error=NULL,stage_message='Promoted as the best available duplicate input',
         updated_at=? WHERE id=?",
    )
    .bind(candidate_id.0)
    .bind(&now)
    .bind(representative.id.0)
    .execute(&mut *transaction)
    .await?;
    if owner_available {
        sqlx::query(
            "UPDATE tracks SET output_path=?,selected_candidate_id=NULL,status='duplicate',
             stage='skipped',error=NULL,stage_message=?,updated_at=? WHERE id=?",
        )
        .bind(&representative.path)
        .bind(format!("Duplicate of input file: {}", representative.path))
        .bind(&now)
        .bind(candidate_owner.id.0)
        .execute(&mut *transaction)
        .await?;
    } else {
        sqlx::query("UPDATE tracks SET selected_candidate_id=NULL,updated_at=? WHERE id=?")
            .bind(&now)
            .bind(candidate_owner.id.0)
            .execute(&mut *transaction)
            .await?;
    }
    transaction.commit().await?;
    Ok(())
}

pub(crate) enum Claim {
    /// Another workflow is running.
    Busy,
    NothingReady,
    Ready(PreparedApply),
}

/// Claims the workflow slot and prepares every ready track for filing. On
/// `Ready` the caller owns the slot and must hand the items to [`run_claimed`].
pub(crate) async fn claim_ready(s: &AppState) -> Result<Claim> {
    if !has_ready_tracks(&s.pool).await? {
        return Ok(Claim::NothingReady);
    }
    if !s
        .try_claim_workflow(WorkflowPhase::Apply, "Filing songs into the library")
        .await
    {
        return Ok(Claim::Busy);
    }
    match prepare_apply(s).await {
        Ok(prepared) if prepared.selected_count > 0 => Ok(Claim::Ready(prepared)),
        Ok(_) => {
            s.reset_workflow(WorkflowPhase::Idle, "Ready").await;
            Ok(Claim::NothingReady)
        }
        Err(error) => {
            s.reset_workflow(WorkflowPhase::Idle, "Ready").await;
            Err(error)
        }
    }
}

/// Files the prepared items and releases the workflow slot.
pub(crate) async fn run_claimed(s: Arc<AppState>, items: Vec<PreviewItem>) -> Result<usize> {
    let result = apply(&s, items).await;
    if s.workflow_cancelled().await {
        s.finish_workflow(WorkflowPhase::Idle, "Filing stopped")
            .await;
    } else {
        match &result {
            Ok(count) => {
                s.finish_workflow(
                    WorkflowPhase::Finish,
                    format!(
                        "Inbox done · {count} {} filed",
                        if *count == 1 { "song" } else { "songs" }
                    ),
                )
                .await
            }
            Err(error) => {
                s.finish_workflow(WorkflowPhase::Failed, error.to_string())
                    .await
            }
        }
    }
    result
}

pub(crate) async fn apply_ready_automatically(s: Arc<AppState>) -> Result<usize> {
    match claim_ready(&s).await? {
        Claim::Ready(prepared) => run_claimed(s, prepared.items).await,
        Claim::Busy | Claim::NothingReady => Ok(0),
    }
}

async fn recording_evidence(
    pool: &sqlx::SqlitePool,
    track: &Track,
    candidate: Option<&Candidate>,
) -> Result<RecordingEvidence> {
    let path = Path::new(&track.path);
    let cached = fingerprint_cache::cached(pool, path).await?;
    let content_key = if track.content_fingerprint.is_some() {
        track.content_fingerprint.clone()
    } else if let Some(value) = &cached {
        input_dedup::fingerprint_key(&value.fingerprint)
    } else {
        Some(input_dedup::hash_key(
            &input_dedup::sha256_cached(pool, path).await?,
        ))
    };
    Ok(RecordingEvidence {
        path: path.to_path_buf(),
        format: track.format.clone().unwrap_or_default(),
        bitrate: track.bitrate.and_then(|value| u32::try_from(value).ok()),
        duration: track.duration,
        content_key,
        isrc: candidate.and_then(|candidate| candidate.isrc.clone()),
        fingerprint: cached.map(|value| value.fingerprint),
    })
}

#[derive(Debug, Eq, PartialEq)]
enum Publication {
    Written(PathBuf),
    Reused(PathBuf),
}

impl Publication {
    fn path(&self) -> &Path {
        match self {
            Self::Written(path) | Self::Reused(path) => path,
        }
    }

    fn reused_existing(&self) -> bool {
        matches!(self, Self::Reused(_))
    }
}

fn destination_variant_number(preferred: &Path, candidate: &Path) -> Option<usize> {
    if candidate.file_name() == preferred.file_name() {
        return Some(1);
    }
    if candidate.extension() != preferred.extension() {
        return None;
    }
    let preferred_stem = preferred.file_stem()?.to_str()?;
    let candidate_stem = candidate.file_stem()?.to_str()?;
    let number = candidate_stem
        .strip_prefix(preferred_stem)?
        .strip_prefix(" (")?
        .strip_suffix(')')?
        .parse::<usize>()
        .ok()?;
    (number >= 2).then_some(number)
}

async fn existing_destination_variants(preferred: &Path) -> Result<Vec<PathBuf>> {
    let parent = preferred
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut directory = match tokio::fs::read_dir(parent).await {
        Ok(directory) => directory,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut variants = Vec::new();
    while let Some(entry) = directory.next_entry().await? {
        let path = entry.path();
        if let Some(number) = destination_variant_number(preferred, &path)
            && tokio::fs::metadata(&path)
                .await
                .is_ok_and(|metadata| metadata.is_file())
        {
            variants.push((number, path));
        }
    }
    variants.sort_by_key(|(number, _)| *number);
    Ok(variants.into_iter().map(|(_, path)| path).collect())
}

struct AudioEquivalenceProbe {
    file_size: u64,
    file_hash: String,
    fingerprint: Option<Option<(String, f64)>>,
}

async fn files_are_equivalent_audio(
    temporary: &Path,
    existing: &Path,
    probe: &mut Option<AudioEquivalenceProbe>,
) -> Result<bool> {
    if probe.is_none() {
        let metadata = tokio::fs::metadata(temporary).await?;
        *probe = Some(AudioEquivalenceProbe {
            file_size: metadata.len(),
            file_hash: files::sha256(temporary).await?,
            fingerprint: None,
        });
    }
    let probe = probe.as_mut().expect("temporary probe initialized");
    let existing_metadata = tokio::fs::metadata(existing).await?;
    if !existing_metadata.is_file() {
        return Ok(false);
    }
    if probe.file_size == existing_metadata.len() {
        let existing_hash = files::sha256(existing).await?;
        if probe.file_hash == existing_hash {
            return Ok(true);
        }
    }

    if probe.fingerprint.is_none() {
        probe.fingerprint = Some(fingerprint::calculate(temporary).await.ok());
    }
    let (
        Some((temporary_fingerprint, temporary_duration)),
        Ok((existing_fingerprint, existing_duration)),
    ) = (
        probe.fingerprint.as_ref().and_then(Option::as_ref),
        fingerprint::calculate(existing).await,
    )
    else {
        return Ok(false);
    };
    Ok(temporary_fingerprint == &existing_fingerprint
        && (temporary_duration - existing_duration).abs() <= 3.0)
}

async fn discard_temporary(temporary: &Path) {
    if let Err(error) = tokio::fs::remove_file(temporary).await {
        tracing::warn!(
            path = %temporary.display(),
            %error,
            "reused existing output but could not remove temporary file"
        );
    }
}

async fn paths_are_same_existing_file(left: &Path, right: &Path) -> bool {
    tokio::try_join!(
        tokio::fs::canonicalize(left),
        tokio::fs::canonicalize(right)
    )
    .is_ok_and(|(left, right)| left == right)
}

/// Reuses an equivalent existing output instead of creating another numbered
/// copy. A genuinely different recording is still published without replacing
/// anything already present. Hard links keep the final publication atomic.
///
/// An existing output is only reused when it also carries the exact artwork we
/// are about to publish, so a stale file with missing or wrong cover art is
/// never silently accepted as the "corrected" output.
async fn publish_no_clobber(
    temporary: &Path,
    preferred: &Path,
    excluded_source: Option<&Path>,
    expected_artwork: Option<&[u8]>,
) -> Result<Publication> {
    tokio::fs::OpenOptions::new()
        .write(true)
        .open(temporary)
        .await?
        .sync_all()
        .await?;

    let mut equivalence_probe = None;
    for existing in existing_destination_variants(preferred).await? {
        if let Some(source) = excluded_source
            && paths_are_same_existing_file(source, &existing).await
        {
            continue;
        }
        if files_are_equivalent_audio(temporary, &existing, &mut equivalence_probe).await?
            && existing_file_has_expected_artwork(&existing, expected_artwork).await?
        {
            discard_temporary(temporary).await;
            return Ok(Publication::Reused(existing));
        }
    }

    let mut number = 1;
    loop {
        let destination = if number == 1 {
            preferred.to_owned()
        } else {
            files::numbered(preferred, number)
        };
        match tokio::fs::hard_link(temporary, &destination).await {
            Ok(()) => {
                if let Err(error) = tokio::fs::remove_file(temporary).await {
                    tracing::warn!(
                        path = %temporary.display(),
                        %error,
                        "published output but could not remove temporary link"
                    );
                }
                return Ok(Publication::Written(destination));
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if let Some(source) = excluded_source
                    && paths_are_same_existing_file(source, &destination).await
                {
                    number += 1;
                    continue;
                }
                if files_are_equivalent_audio(temporary, &destination, &mut equivalence_probe)
                    .await?
                    && existing_file_has_expected_artwork(&destination, expected_artwork).await?
                {
                    discard_temporary(temporary).await;
                    return Ok(Publication::Reused(destination));
                }
                number += 1;
            }
            Err(error) => return Err(error.into()),
        }
    }
}

/// A reusable existing output must already contain the exact cover we are about
/// to write. When no artwork is expected (defensive path) any existing output
/// passes this check.
async fn existing_file_has_expected_artwork(
    existing: &Path,
    expected: Option<&[u8]>,
) -> Result<bool> {
    let Some(expected) = expected else {
        return Ok(true);
    };
    let existing = existing.to_path_buf();
    let expected = expected.to_vec();
    let matches = tokio::task::spawn_blocking(move || match tag_writer::read_artwork(&existing) {
        Ok(Some(bytes)) => bytes == expected,
        _ => false,
    })
    .await?;
    Ok(matches)
}

fn temporary_destination(destination: &Path, track_id: i64) -> PathBuf {
    let parent = destination.parent().unwrap_or_else(|| Path::new(""));
    let stem = destination
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("corrected-track");
    let extension = destination
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("audio");
    parent.join(format!(".{stem}.magpie-fi-{track_id}.{extension}"))
}

enum Filing {
    Filed,
    /// Already in the library, or sent back to review.
    NotFiled,
}

/// Files every prepared track into the library. Per track:
/// already in the library? → original to trash (or replace a lossy copy with
/// a lossless one) · verified cover · tags · atomic no-clobber publish ·
/// original to trash · index · folder cover · lyrics.
/// After the batch: ReplayGain per touched album folder.
/// Returns how many songs were filed (songs already in the library don't count).
async fn apply(s: &Arc<AppState>, items: Vec<PreviewItem>) -> Result<usize> {
    let total = items.len();
    let mut touched_albums = BTreeSet::new();
    let mut filed = 0usize;
    for (index, item) in items.into_iter().enumerate() {
        if s.workflow_cancelled().await {
            break;
        }
        s.set_workflow(
            WorkflowPhase::Apply,
            "Filing songs into the library",
            index,
            total,
            Some(item.current_path.clone()),
        )
        .await;
        match file_item(s, &item, &mut touched_albums).await {
            Ok(Filing::Filed) => filed += 1,
            Ok(Filing::NotFiled) => {}
            // One bad track must not hold up the others.
            Err(error) => {
                tracing::error!(file = item.filename, "filing failed: {error:#}");
                mark_failed(&s.pool, item.track_id, &error).await?;
            }
        }
    }
    finish_batch(&touched_albums).await;
    Ok(filed)
}

async fn file_item(
    s: &Arc<AppState>,
    item: &PreviewItem,
    touched_albums: &mut BTreeSet<PathBuf>,
) -> Result<Filing> {
    let cfg = &s.config;
    tracing::info!(
        file = item.filename,
        track_id = item.track_id.0,
        source = item.current_path,
        destination = item.destination_path,
        "filing into the library"
    );
    let src = PathBuf::from(&item.current_path);
    let candidate = &item.candidate;

    // 1. Is this recording already in the library?
    let replaces = match library::find_same_recording(
        &s.pool,
        &item.evidence,
        candidate.recording_id.as_deref(),
    )
    .await?
    {
        Some(existing) if !library::is_upgrade(&item.evidence, &existing) => {
            skip_known_recording(s, item, &existing.path).await?;
            return Ok(Filing::NotFiled);
        }
        Some(existing) => Some(PathBuf::from(existing.path)),
        None => None,
    };
    let artist_ids = candidate
        .artist_credits
        .iter()
        .filter_map(|credit| credit.artist_id)
        .collect::<Vec<_>>();
    if replaces.is_none()
        && let Some(existing) = library::find_possible_duplicate(
            &s.pool,
            &candidate.title,
            item.evidence.duration,
            &artist_ids,
        )
        .await?
        && !duplicate_allowed(s, item.track_id).await?
    {
        return_track_as_possible_duplicate(s, item.track_id, &existing).await?;
        return Ok(Filing::NotFiled);
    }

    let artwork = match resolve_artwork(s, &item.filename, candidate).await {
        Ok(artwork) => artwork,
        Err(error) => {
            return_track_for_cover(s, item.track_id, candidate.id, &error).await?;
            return Ok(Filing::NotFiled);
        }
    };
    // The same JPEG goes inside the song and, as cover.jpg, next to it.
    let artwork = tokio::task::spawn_blocking(move || tag_writer::prepare_cover(&artwork))
        .await?
        .context("could not prepare the cover")?;
    let dest = PathBuf::from(&item.destination_path);
    if paths_resolve_to_same_target(&src, &dest).await? {
        bail!("refusing to file a song onto itself: {}", src.display());
    }
    let temporary = temporary_destination(&dest, item.track_id.0);
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("could not create the album folder {}", parent.display()))?;
    }
    if is_lossless_uncompressed(&src) {
        // WAV and AIFF cannot be tagged reliably: they become FLAC, which loses nothing.
        convert_to_flac(&src, &temporary).await?;
    } else {
        tokio::fs::copy(&src, &temporary).await.with_context(|| {
            format!(
                "could not copy {} to {}",
                src.display(),
                temporary.display()
            )
        })?;
    }

    // 2. Tags, cover and lyrics (verified) and atomic publish.
    let fetched_lyrics = if cfg.lyrics {
        fetch_lyrics(s, item).await
    } else {
        None
    };
    let lyrics = if cfg.sidecars {
        None
    } else {
        fetched_lyrics.clone()
    };
    let published = match write_tags(s, &temporary, candidate, &artwork, lyrics).await {
        Ok(()) => publish_no_clobber(&temporary, &dest, Some(&src), Some(&artwork)).await,
        Err(error) => Err(error),
    };
    let publication = match published {
        Ok(publication) => publication,
        Err(error) => {
            let _ = tokio::fs::remove_file(&temporary).await;
            return Err(error.context("tag writing failed"));
        }
    };
    let final_path = publication.path().to_owned();
    let reused_existing = publication.reused_existing();

    // 3. The original goes to the trash, the library remembers the new file.
    trash_source_after_output(
        &src,
        &final_path,
        &cfg.trash_dir,
        &cfg.staging_dir,
        Path::new(&cfg.input_dir),
    )
    .await?;
    crate::acquire::downloaded_file_done(s, &src, &final_path, reused_existing).await;
    if let Some(old) = replaces.filter(|old| *old != final_path) {
        for file in std::iter::once(old.clone()).chain(lyrics::sidecars(&old)) {
            library::move_to_trash(&cfg.trash_dir, Path::new(&cfg.output_dir), &file).await?;
        }
        library::forget(&s.pool, &old).await?;
        crate::acquire::library_file_moved(s, &old, &final_path).await;
        tracing::info!(
            file = item.filename,
            replaced = %old.display(),
            "replaced a lossy copy with lossless audio"
        );
    }
    library::remember(
        &s.pool,
        &library::LibraryEntry {
            path: final_path.to_string_lossy().into_owned(),
            duration: item.evidence.duration,
            content_key: item.evidence.content_key.clone(),
            isrc: item.evidence.isrc.clone(),
            recording_id: candidate.recording_id.clone(),
            title: Some(candidate.title.clone()),
            artist: Some(candidate.artist.clone()),
            album: candidate.album.clone(),
            format: item.evidence.format.clone(),
            bitrate: item.evidence.bitrate,
            source: "inbox".into(),
            fingerprint: item.evidence.fingerprint.clone(),
            artist_ids,
        },
    )
    .await?;

    // 4. Extras. None of them can undo a successful filing. Separate cover and
    // lyrics files only when asked for: normally they are inside the song.
    // Navidrome looks for cover.jpg before the cover inside the song, and reads a .lrc
    // before embedded lyrics, so both are written next to the song as well.
    if let Some(album_folder) = final_path.parent() {
        if let Err(error) = library::write_folder_cover(album_folder, &artwork).await {
            tracing::warn!(%error, folder = %album_folder.display(), "could not write cover file");
        }
        touched_albums.insert(album_folder.to_path_buf());
    }
    if cfg.lyrics && cfg.sidecars {
        save_lyrics(s, item, &final_path).await;
    } else if let Some(text) = fetched_lyrics.as_deref() {
        match lyrics::write_lrc(&final_path, text).await {
            Ok(Some(path)) => tracing::debug!(file = %path.display(), "wrote synced lyrics file"),
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(%error, file = %final_path.display(), "could not write the lyrics file")
            }
        }
    }
    if reused_existing {
        tracing::info!(
            file = item.filename,
            output = %final_path.display(),
            "already in the library with the same tags and cover; original moved to trash"
        );
    } else {
        tracing::info!(file = item.filename, output = %final_path.display(), "filed into the library");
    }
    for duplicate in &item.duplicates {
        if let Err(error) = finish_duplicate(s, duplicate, &final_path).await {
            let detail = format!("{error:#}");
            let _ = sqlx::query(
                "UPDATE tracks SET status='failed',stage='failed',error=?,stage_message='Duplicate output was avoided, but moving the duplicate to trash failed',updated_at=? WHERE id=?",
            )
            .bind(&detail)
            .bind(Utc::now().to_rfc3339())
            .bind(duplicate.track_id.0)
            .execute(&s.pool)
            .await;
            tracing::error!(
                file = duplicate.filename,
                error = detail,
                "duplicate output was avoided, but moving the duplicate to trash failed"
            );
        }
    }
    sqlx::query("DELETE FROM tracks WHERE id=?")
        .bind(item.track_id.0)
        .execute(&s.pool)
        .await?;
    Ok(Filing::Filed)
}

/// Writes and verifies the tags and cover of the temporary copy, off the
/// async runtime and within the tag-writer limit.
fn is_lossless_uncompressed(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "wav" | "aiff" | "aif"
            )
        })
}

/// WAV or AIFF to FLAC: the same samples, about half the size, and a container that holds tags.
async fn convert_to_flac(source: &Path, target: &Path) -> Result<()> {
    let output = tokio::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(source)
        .args([
            "-map",
            "0:a:0",
            "-map_metadata",
            "0",
            "-c:a",
            "flac",
            "-compression_level",
            "8",
            "-f",
            "flac",
        ])
        .arg(target)
        .output()
        .await
        .context("ffmpeg could not be started")?;
    if !output.status.success() {
        bail!(
            "could not convert {} to FLAC: {}",
            source.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

async fn write_tags(
    s: &AppState,
    target: &Path,
    candidate: &Candidate,
    artwork: &[u8],
    lyrics: Option<String>,
) -> Result<()> {
    let permit = s.tag_writes.clone().acquire_owned().await?;
    let path = target.to_path_buf();
    let candidate = candidate.clone();
    let artwork = artwork.to_vec();
    let sanitized = tokio::task::spawn_blocking(move || {
        let target = path;
        let _permit = permit;
        let sanitized = tag_writer::write_resilient(
            &target,
            &candidate,
            Some(artwork.clone()),
            lyrics.as_deref(),
        )?;
        tag_writer::verify_written_metadata(&target, &candidate)?;
        tag_writer::verify_embedded_artwork(&target, &artwork)?;
        if let Some(lyrics) = lyrics.as_deref() {
            tag_writer::verify_lyrics(&target, lyrics)?;
        }
        Ok::<_, anyhow::Error>(sanitized)
    })
    .await??;
    if sanitized {
        tracing::info!(
            file = %target.display(),
            "removed malformed legacy tags with lossless stream-copy and retried"
        );
    }
    Ok(())
}

/// The recording is already in the library and the new file is not better:
/// the new file and its input duplicates go to the trash.
async fn skip_known_recording(
    s: &Arc<AppState>,
    item: &PreviewItem,
    library_path: &str,
) -> Result<()> {
    let cfg = &s.config;
    let inbox = Path::new(&cfg.input_dir);
    let sources = std::iter::once((item.track_id, item.current_path.as_str(), false)).chain(
        item.duplicates.iter().map(|duplicate| {
            (
                duplicate.track_id,
                duplicate.current_path.as_str(),
                duplicate.source_missing,
            )
        }),
    );
    for (track_id, path, missing) in sources {
        let original = Path::new(path);
        if !missing {
            library::discard_original(&cfg.trash_dir, &cfg.staging_dir, inbox, original).await?;
        }
        crate::acquire::downloaded_file_done(s, original, Path::new(library_path), true).await;
        sqlx::query("DELETE FROM tracks WHERE id=?")
            .bind(track_id.0)
            .execute(&s.pool)
            .await?;
    }
    tracing::info!(
        file = item.filename,
        library = library_path,
        "already in your library; the new copy was moved to trash"
    );
    Ok(())
}

/// Lyrics to put inside the song. A failed lookup only means no new lyrics.
async fn fetch_lyrics(s: &AppState, item: &PreviewItem) -> Option<String> {
    let candidate = &item.candidate;
    let names = crate::domain::credits::normalize_featured(&candidate.artist, &candidate.title);
    match lyrics::fetch(
        &s.client,
        &names.title,
        &names.artist,
        candidate.album.as_deref(),
        item.evidence.duration,
    )
    .await
    {
        Ok(found) => found,
        Err(error) => {
            tracing::warn!(%error, file = %item.filename, "lyrics lookup failed");
            None
        }
    }
}

async fn save_lyrics(s: &AppState, item: &PreviewItem, audio: &Path) {
    let candidate = &item.candidate;
    let names = crate::domain::credits::normalize_featured(&candidate.artist, &candidate.title);
    match lyrics::fetch_and_save(
        &s.client,
        audio,
        &names.title,
        &names.artist,
        candidate.album.as_deref(),
        item.evidence.duration,
    )
    .await
    {
        Ok(lyrics::Saved::Synced(path) | lyrics::Saved::Plain(path)) => {
            tracing::info!(file = item.filename, lyrics = %path.display(), "saved lyrics");
        }
        Ok(lyrics::Saved::Nothing) => {}
        Err(error) => {
            tracing::warn!(%error, file = %item.filename, "lyrics lookup failed");
        }
    }
}

/// After a batch: album ReplayGain for every album folder that got new songs.
async fn finish_batch(touched_albums: &BTreeSet<PathBuf>) {
    if touched_albums.is_empty() {
        return;
    }
    if !rsgain::available() {
        tracing::warn!("rsgain is not installed; ReplayGain tags were not written");
        return;
    }
    for folder in touched_albums {
        if let Err(error) = rsgain::tag_album_folder(folder).await {
            tracing::warn!(%error, folder = %folder.display(), "album ReplayGain failed");
        }
    }
}

/// Start of the Review message for a possible duplicate (the web page looks for it).
pub const POSSIBLE_DUPLICATE: &str = "Possible duplicate: you may already have this song as ";

async fn duplicate_allowed(s: &AppState, track_id: TrackId) -> Result<bool> {
    Ok(
        sqlx::query_scalar::<_, bool>("SELECT allow_duplicate FROM tracks WHERE id=?")
            .bind(track_id.0)
            .fetch_optional(&s.pool)
            .await?
            .unwrap_or(false),
    )
}

/// Probably already in the library under another spelling: the owner decides
/// ("File anyway" files it; otherwise remove it from the inbox).
async fn return_track_as_possible_duplicate(
    s: &AppState,
    track_id: TrackId,
    existing: &str,
) -> Result<()> {
    let shown = Path::new(existing)
        .strip_prefix(&s.config.output_dir)
        .map_or_else(|_| existing.to_owned(), |path| path.display().to_string());
    let detail = format!("{POSSIBLE_DUPLICATE}{shown}");
    sqlx::query(
        "UPDATE tracks SET status='needs_review',stage='review',stage_message=?,error=NULL,updated_at=? WHERE id=?",
    )
    .bind(&detail)
    .bind(Utc::now().to_rfc3339())
    .bind(track_id.0)
    .execute(&s.pool)
    .await?;
    tracing::info!(
        track_id = track_id.0,
        existing,
        "possible duplicate returned to review"
    );
    Ok(())
}

/// The cover failed at the last check. The match stays chosen, but its cover is
/// marked as missing (or as a temporary failure that "Check & fix" retries), so
/// the page asks for a cover instead of showing a verified one that is stuck.
async fn return_track_for_cover(
    state: &AppState,
    track_id: TrackId,
    candidate_id: Option<i64>,
    error: &anyhow::Error,
) -> Result<()> {
    let detail = format!("Cover verification failed before writing: {error:#}");
    let status = if crate::application::artwork::is_retryable_artwork_error(error) {
        crate::infrastructure::providers::ArtworkStatus::RetryableError
    } else {
        crate::infrastructure::providers::ArtworkStatus::CoverRequired
    };
    let mut transaction = state.pool.begin().await?;
    sqlx::query(
        "UPDATE tracks SET status='needs_review',stage='review',stage_message=?,error=NULL,updated_at=? WHERE id=?",
    )
    .bind(&detail)
    .bind(Utc::now().to_rfc3339())
    .bind(track_id.0)
    .execute(&mut *transaction)
    .await?;
    if let Some(candidate_id) = candidate_id {
        sqlx::query("UPDATE candidates SET artwork_status=?,artwork_message=? WHERE id=?")
            .bind(status)
            .bind(&detail)
            .bind(candidate_id)
            .execute(&mut *transaction)
            .await?;
    }
    transaction.commit().await?;
    tracing::warn!(track_id = track_id.0, detail, "track returned to review");
    Ok(())
}

async fn finish_duplicate(
    state: &Arc<AppState>,
    duplicate: &DuplicateSource,
    output: &Path,
) -> Result<()> {
    let source = Path::new(&duplicate.current_path);
    if !duplicate.source_missing {
        if paths_resolve_to_same_target(source, output).await? {
            bail!(
                "refusing to trash a duplicate because it is the filed song: {}",
                source.display()
            );
        }
        let cfg = &state.config;
        trash_source_after_output(
            source,
            output,
            &cfg.trash_dir,
            &cfg.staging_dir,
            Path::new(&cfg.input_dir),
        )
        .await?;
    }
    crate::acquire::downloaded_file_done(state, source, output, true).await;
    sqlx::query("DELETE FROM tracks WHERE id=?")
        .bind(duplicate.track_id.0)
        .execute(&state.pool)
        .await?;
    tracing::info!(
        file = duplicate.filename,
        source = duplicate.current_path,
        output = %output.display(),
        source_missing = duplicate.source_missing,
        "duplicate copy in the inbox; kept one song and moved this copy to trash"
    );
    Ok(())
}

/// Fetch the exact verified artwork bytes to embed, or fail so the caller can
/// return the track to review. Artwork is never optional at write time: a file
/// is only published when it has valid matching cover art. Only artwork that
/// matches the selected release is considered.
pub(crate) async fn resolve_artwork(
    state: &AppState,
    filename: &str,
    candidate: &Candidate,
) -> Result<Vec<u8>> {
    let _permit = state.artwork_downloads.acquire().await?;
    match crate::application::artwork::resolve_for_write(&state.pool, &state.client, candidate)
        .await
    {
        Ok(bytes) => {
            tracing::debug!(
                file = filename,
                provider = candidate.provider,
                url = candidate.cover_url,
                "downloaded verified cover art"
            );
            Ok(bytes)
        }
        Err(error) => {
            tracing::warn!(
                file = filename,
                "no matching cover artwork is available: {error:#}"
            );
            Err(error)
        }
    }
}

async fn paths_resolve_to_same_target(source: &Path, destination: &Path) -> Result<bool> {
    let source = tokio::fs::canonicalize(source).await?;
    let destination_parent = destination
        .parent()
        .ok_or_else(|| anyhow!("output file has no parent directory"))?;
    tokio::fs::create_dir_all(destination_parent).await?;
    let destination = match tokio::fs::canonicalize(destination).await {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            tokio::fs::canonicalize(destination_parent).await?.join(
                destination
                    .file_name()
                    .ok_or_else(|| anyhow!("output file has no filename"))?,
            )
        }
        Err(error) => return Err(error.into()),
    };
    Ok(source == destination)
}

/// Moves the original to the trash (or deletes it when it was one of magpie-fi's
/// own downloads), but only after the filed song exists.
async fn trash_source_after_output(
    source: &Path,
    destination: &Path,
    trash: &Path,
    staging: &Path,
    inbox: &Path,
) -> Result<()> {
    let output = tokio::fs::metadata(destination)
        .await
        .map_err(|error| anyhow!("filed song is unavailable: {error}"))?;
    if !output.is_file() {
        bail!("filed song is not a regular file")
    }
    library::discard_original(trash, staging, inbox, source).await
}
#[cfg(test)]
mod tests {
    use super::*;

    fn other_artwork_png() -> Vec<u8> {
        let image = image::RgbaImage::from_fn(320, 320, |x, y| {
            if (x / 25 + y / 25) % 3 == 0 {
                image::Rgba([190, 40, 80, 255])
            } else if (x / 25 + y / 25) % 3 == 1 {
                image::Rgba([40, 190, 120, 255])
            } else {
                image::Rgba([90, 70, 220, 255])
            }
        });
        let mut output = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut output, image::ImageFormat::Png)
            .unwrap();
        output.into_inner()
    }

    #[test]
    fn one_new_album_in_one_batch_gets_one_year() {
        let item = |title: &str, year: &str| {
            let candidate = Candidate {
                title: title.into(),
                artist: "Ebi".into(),
                album: Some("Khalij".into()),
                album_artist: Some("Ebi".into()),
                year: Some(year.into()),
                ..Default::default()
            };
            PreviewItem {
                track_id: TrackId(1),
                filename: format!("{title}.mp3"),
                current_path: format!("/inbox/{title}.mp3"),
                destination_path: library::destination(Path::new("/lib"), &candidate, "mp3")
                    .to_string_lossy()
                    .into_owned(),
                candidate,
                evidence: RecordingEvidence::default(),
                duplicates: Vec::new(),
            }
        };
        // One catalog says 1999, another 2001 for a later track of the same album.
        let mut items = vec![item("Khalij", "1999"), item("Shabe Eshgh", "2001-05-01")];
        align_planned_albums("/lib", &mut items);
        assert_eq!(items[1].candidate.year.as_deref(), Some("1999"));
        assert_eq!(items[1].candidate.release_date, None);
        assert!(
            items[1]
                .destination_path
                .starts_with("/lib/Ebi/Khalij (1999)/"),
            "{}",
            items[1].destination_path
        );
    }

    #[tokio::test]
    async fn a_cover_that_fails_at_filing_is_asked_for_again() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("cover-recheck.sqlite");
        let pool = crate::infrastructure::db::connect(database.to_str().unwrap())
            .await
            .unwrap();
        let track_id = sqlx::query("INSERT INTO tracks(path,filename,status,is_missing,first_seen_at,last_seen_at,last_scanned_at,stage) VALUES('/music/song.mp3','song.mp3','selected',0,'now','now','now','ready')")
            .execute(&pool)
            .await
            .unwrap()
            .last_insert_rowid();
        let candidate_id = sqlx::query("INSERT INTO candidates(track_id,provider,title,artist,score,artwork_status) VALUES(?,'deezer','Song','Artist',80,'verified')")
            .bind(track_id)
            .execute(&pool)
            .await
            .unwrap()
            .last_insert_rowid();
        let state = AppState::new(crate::config::Config::default(), pool.clone());

        return_track_for_cover(
            &state,
            TrackId(track_id),
            Some(candidate_id),
            &anyhow!("no usable matching cover could be downloaded"),
        )
        .await
        .unwrap();

        let stage: String = sqlx::query_scalar("SELECT stage FROM tracks WHERE id=?")
            .bind(track_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        let cover: String = sqlx::query_scalar("SELECT artwork_status FROM candidates WHERE id=?")
            .bind(candidate_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(stage, "review");
        assert_eq!(cover, "cover_required");
    }

    #[tokio::test]
    async fn automatic_write_excludes_tracks_that_are_still_in_review() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("review-write.sqlite");
        let pool = crate::infrastructure::db::connect(database.to_str().unwrap())
            .await
            .unwrap();
        let track_id = sqlx::query("INSERT INTO tracks(path,filename,status,is_missing,first_seen_at,last_seen_at,last_scanned_at,stage) VALUES('/music/review.mp3','review.mp3','needs_review',0,'now','now','now','review')")
            .execute(&pool)
            .await
            .unwrap()
            .last_insert_rowid();
        let candidate_id = sqlx::query("INSERT INTO candidates(track_id,provider,title,artist,score) VALUES(?,'deezer','Song','Artist',80)")
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
        let state = Arc::new(AppState::new(
            crate::config::Config::default(),
            pool.clone(),
        ));

        let prepared = prepare_apply(&state).await.unwrap();

        assert_eq!(prepared.selected_count, 0);
        assert!(prepared.items.is_empty());
    }

    #[tokio::test]
    async fn prepared_write_includes_early_input_duplicates_in_one_output() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("input-duplicates.sqlite");
        let pool = crate::infrastructure::db::connect(database.to_str().unwrap())
            .await
            .unwrap();
        let input = directory.path().join("input");
        let output = directory.path().join("output");
        tokio::fs::create_dir_all(&input).await.unwrap();
        tokio::fs::create_dir_all(&output).await.unwrap();
        let kept_path = input.join("kept.flac");
        let duplicate_path = input.join("duplicate.mp3");
        tokio::fs::write(&kept_path, b"kept").await.unwrap();
        tokio::fs::write(&duplicate_path, b"duplicate")
            .await
            .unwrap();
        let kept_id = sqlx::query(
            "INSERT INTO tracks(path,filename,format,bitrate,duration,content_fingerprint,status,
             is_missing,first_seen_at,last_seen_at,last_scanned_at,stage)
             VALUES(?,'kept.flac','flac',900,180.0,'fp:same','selected',0,'now','now','now','ready')",
        )
        .bind(kept_path.to_string_lossy().as_ref())
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_rowid();
        let candidate_id = sqlx::query(
            "INSERT INTO candidates(track_id,provider,title,artist,isrc,score)
             VALUES(?,'deezer','Song','Artist',NULL,100)",
        )
        .bind(kept_id)
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_rowid();
        sqlx::query("UPDATE tracks SET selected_candidate_id=? WHERE id=?")
            .bind(candidate_id)
            .bind(kept_id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO tracks(path,filename,format,bitrate,duration,content_fingerprint,status,
             is_missing,first_seen_at,last_seen_at,last_scanned_at,stage)
             VALUES(?,'duplicate.mp3','mp3',320,181.0,'fp:same','duplicate',0,'now','now','now','skipped')",
        )
        .bind(duplicate_path.to_string_lossy().as_ref())
        .execute(&pool)
        .await
        .unwrap();
        let state = Arc::new(AppState::new(
            crate::config::Config {
                input_dir: input.to_string_lossy().into_owned(),
                output_dir: output.to_string_lossy().into_owned(),
                ..Default::default()
            },
            pool,
        ));

        let prepared = prepare_apply(&state).await.unwrap();

        assert_eq!(prepared.selected_count, 2);
        assert_eq!(prepared.outputs, 1);
        assert_eq!(prepared.duplicates_skipped, 1);
        assert_eq!(prepared.items[0].current_path, kept_path.to_string_lossy());
        assert_eq!(prepared.items[0].duplicates.len(), 1);
    }

    #[tokio::test]
    async fn prepared_write_promotes_an_available_duplicate_when_the_selected_source_is_missing() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("missing-representative.sqlite");
        let pool = crate::infrastructure::db::connect(database.to_str().unwrap())
            .await
            .unwrap();
        let input = directory.path().join("input");
        let output = directory.path().join("output");
        tokio::fs::create_dir_all(&input).await.unwrap();
        tokio::fs::create_dir_all(&output).await.unwrap();
        let missing_path = input.join("missing.flac");
        let promoted_path = input.join("available.mp3");
        tokio::fs::write(&promoted_path, b"available")
            .await
            .unwrap();
        let missing_id = sqlx::query(
            "INSERT INTO tracks(path,filename,format,bitrate,duration,content_fingerprint,status,
             is_missing,first_seen_at,last_seen_at,last_scanned_at,stage)
             VALUES(?,'missing.flac','flac',900,180.0,'fp:same','selected',0,'now','now','now','ready')",
        )
        .bind(missing_path.to_string_lossy().as_ref())
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_rowid();
        let candidate_id = sqlx::query(
            "INSERT INTO candidates(track_id,provider,title,artist,score)
             VALUES(?,'deezer','Song','Artist',100)",
        )
        .bind(missing_id)
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_rowid();
        sqlx::query("UPDATE tracks SET selected_candidate_id=? WHERE id=?")
            .bind(candidate_id)
            .bind(missing_id)
            .execute(&pool)
            .await
            .unwrap();
        let promoted_id = sqlx::query(
            "INSERT INTO tracks(path,filename,format,bitrate,duration,content_fingerprint,status,
             is_missing,first_seen_at,last_seen_at,last_scanned_at,stage)
             VALUES(?,'available.mp3','mp3',320,180.0,'fp:same','duplicate',0,'now','now','now','skipped')",
        )
        .bind(promoted_path.to_string_lossy().as_ref())
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_rowid();
        let state = Arc::new(AppState::new(
            crate::config::Config {
                input_dir: input.to_string_lossy().into_owned(),
                output_dir: output.to_string_lossy().into_owned(),
                ..Default::default()
            },
            pool.clone(),
        ));

        let prepared = prepare_apply(&state).await.unwrap();

        assert_eq!(prepared.outputs, 1);
        assert_eq!(prepared.items[0].track_id.0, promoted_id);
        assert_eq!(
            prepared.items[0].current_path,
            promoted_path.to_string_lossy()
        );
        assert!(prepared.items[0].duplicates[0].source_missing);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT track_id FROM candidates WHERE id=?")
                .bind(candidate_id)
                .fetch_one(&pool)
                .await
                .unwrap(),
            promoted_id
        );
    }

    async fn insert_ready_track(
        pool: &sqlx::SqlitePool,
        path: &Path,
        format: Option<&str>,
        content_key: &str,
    ) -> i64 {
        let track_id = sqlx::query(
            "INSERT INTO tracks(path,filename,format,duration,content_fingerprint,status,
             is_missing,first_seen_at,last_seen_at,last_scanned_at,stage)
             VALUES(?,?,?,180.0,?,'selected',0,'now','now','now','ready')",
        )
        .bind(path.to_string_lossy().as_ref())
        .bind(path.file_name().unwrap().to_string_lossy().as_ref())
        .bind(format)
        .bind(content_key)
        .execute(pool)
        .await
        .unwrap()
        .last_insert_rowid();
        let candidate_id = sqlx::query(
            "INSERT INTO candidates(track_id,provider,title,artist,album,cover_url,score,artwork_status)
             VALUES(?,'deezer','Song','Artist','Album','https://example.test/cover.png',95,'verified')",
        )
        .bind(track_id)
        .execute(pool)
        .await
        .unwrap()
        .last_insert_rowid();
        sqlx::query("UPDATE tracks SET selected_candidate_id=? WHERE id=?")
            .bind(candidate_id)
            .bind(track_id)
            .execute(pool)
            .await
            .unwrap();
        track_id
    }

    async fn filing_state(directory: &Path) -> Arc<AppState> {
        let config =
            crate::config::Config::with_folders(&directory.join("music"), &directory.join("data"));
        tokio::fs::create_dir_all(&config.input_dir).await.unwrap();
        tokio::fs::create_dir_all(&config.output_dir).await.unwrap();
        let pool = crate::infrastructure::db::connect(&config.db_path)
            .await
            .unwrap();
        crate::infrastructure::provider_cache::put(
            &pool,
            "artwork-url",
            &crate::infrastructure::provider_cache::url_key("https://example.test/cover.png"),
            &serde_json::json!({
                "data_base64": base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    tag_writer::test_artwork_png()
                )
            }),
            Utc::now() + chrono::Duration::days(1),
        )
        .await
        .unwrap();
        Arc::new(AppState::new(config, pool))
    }

    async fn stage_of(pool: &sqlx::SqlitePool, track_id: i64) -> (String, String) {
        sqlx::query_as("SELECT status,stage FROM tracks WHERE id=?")
            .bind(track_id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn a_song_whose_tags_cannot_be_written_leaves_the_ready_set() {
        let directory = tempfile::tempdir().unwrap();
        let state = filing_state(directory.path()).await;
        let source = Path::new(&state.config.input_dir).join("broken.mp3");
        tokio::fs::write(&source, b"not audio at all")
            .await
            .unwrap();
        let track_id = insert_ready_track(&state.pool, &source, Some("mp3"), "fp:broken").await;

        let Claim::Ready(prepared) = claim_ready(&state).await.unwrap() else {
            panic!("the track should be ready");
        };
        assert_eq!(run_claimed(state.clone(), prepared.items).await.unwrap(), 0);

        assert_eq!(
            stage_of(&state.pool, track_id).await,
            ("failed".into(), "failed".into())
        );
        assert!(source.exists());
        // The next inbox cycle does not try it again.
        assert!(matches!(
            claim_ready(&state).await.unwrap(),
            Claim::NothingReady
        ));
        assert!(!state.workflow_running().await);
    }

    #[tokio::test]
    async fn one_unplannable_song_does_not_hold_up_the_others() {
        let directory = tempfile::tempdir().unwrap();
        let state = filing_state(directory.path()).await;
        let inbox = Path::new(&state.config.input_dir);
        let unknown = inbox.join("no-extension");
        let good = inbox.join("good.flac");
        tokio::fs::write(&unknown, b"?").await.unwrap();
        tokio::fs::write(&good, b"audio").await.unwrap();
        let unknown_id = insert_ready_track(&state.pool, &unknown, None, "fp:unknown").await;
        let good_id = insert_ready_track(&state.pool, &good, Some("flac"), "fp:good").await;

        let prepared = prepare_apply(&state).await.unwrap();

        assert_eq!(prepared.items.len(), 1);
        assert_eq!(prepared.items[0].track_id.0, good_id);
        assert_eq!(
            stage_of(&state.pool, unknown_id).await,
            ("failed".into(), "failed".into())
        );
    }

    #[test]
    fn recognizes_only_numbered_variants_of_the_preferred_destination() {
        let base = std::path::Path::new("/output/Artist - Song.mp3");
        assert_eq!(destination_variant_number(base, base), Some(1));
        assert_eq!(
            destination_variant_number(base, std::path::Path::new("/output/Artist - Song (2).mp3")),
            Some(2)
        );
        assert_eq!(
            destination_variant_number(
                base,
                std::path::Path::new("/output/Artist - Song Remix (2).mp3")
            ),
            None
        );
        assert_eq!(
            destination_variant_number(
                base,
                std::path::Path::new("/output/Artist - Song (2).flac")
            ),
            None
        );
    }

    #[tokio::test]
    async fn publishing_never_replaces_an_existing_output() {
        let directory = tempfile::tempdir().unwrap();
        let temporary = directory.path().join(".temporary.mp3");
        let preferred = directory.path().join("Artist - Song.mp3");
        tokio::fs::write(&temporary, b"new output").await.unwrap();
        tokio::fs::write(&preferred, b"existing output")
            .await
            .unwrap();

        let published = publish_no_clobber(&temporary, &preferred, None, None)
            .await
            .unwrap();

        assert_eq!(
            published,
            Publication::Written(directory.path().join("Artist - Song (2).mp3"))
        );
        assert_eq!(
            tokio::fs::read(&preferred).await.unwrap(),
            b"existing output"
        );
        assert_eq!(
            tokio::fs::read(published.path()).await.unwrap(),
            b"new output"
        );
        assert!(!temporary.exists());
    }

    #[tokio::test]
    async fn equivalent_existing_output_is_reused_without_numbering() {
        let directory = tempfile::tempdir().unwrap();
        let temporary = directory.path().join(".temporary.mp3");
        let preferred = directory.path().join("Artist - Song.mp3");
        tokio::fs::write(&temporary, b"same corrected audio")
            .await
            .unwrap();
        tokio::fs::write(&preferred, b"same corrected audio")
            .await
            .unwrap();

        let published = publish_no_clobber(&temporary, &preferred, None, None)
            .await
            .unwrap();

        assert_eq!(published, Publication::Reused(preferred));
        assert!(!temporary.exists());
        assert!(!directory.path().join("Artist - Song (2).mp3").exists());
    }

    #[tokio::test]
    async fn audio_fingerprint_reuses_existing_output_with_different_tags() {
        if std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_err()
            || std::process::Command::new("fpcalc")
                .arg("-version")
                .output()
                .is_err()
        {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let temporary = directory.path().join(".temporary.mp3");
        let preferred = directory.path().join("Artist - Song.mp3");
        let generated = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
            .arg("sine=frequency=440:duration=4")
            .args(["-q:a", "4", "-metadata", "title=Existing"])
            .arg(&preferred)
            .status()
            .unwrap();
        assert!(generated.success());
        let retagged = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-i"])
            .arg(&preferred)
            .args([
                "-map",
                "0:a:0",
                "-c:a",
                "copy",
                "-metadata",
                "title=Retagged",
            ])
            .arg(&temporary)
            .status()
            .unwrap();
        assert!(retagged.success());
        assert_ne!(
            files::sha256(&temporary).await.unwrap(),
            files::sha256(&preferred).await.unwrap()
        );

        let published = publish_no_clobber(&temporary, &preferred, None, None)
            .await
            .unwrap();

        assert_eq!(published, Publication::Reused(preferred));
        assert!(!temporary.exists());
        assert!(!directory.path().join("Artist - Song (2).mp3").exists());
    }

    #[tokio::test]
    async fn existing_output_with_different_artwork_is_not_reused() {
        if std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_err()
        {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.mp3");
        let existing = directory.path().join("Artist - Song.mp3");
        let temporary = directory.path().join(".temporary.mp3");
        let status = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
            .arg("sine=frequency=440:duration=4")
            .args(["-q:a", "4"])
            .arg(&source)
            .status()
            .unwrap();
        assert!(status.success());
        tokio::fs::copy(&source, &existing).await.unwrap();
        tokio::fs::copy(&source, &temporary).await.unwrap();
        let candidate = crate::infrastructure::providers::Candidate {
            title: "Song".into(),
            artist: "Artist".into(),
            ..Default::default()
        };
        let artwork_a = crate::infrastructure::media::tag_writer::test_artwork_png();
        let artwork_b = other_artwork_png();
        crate::infrastructure::media::tag_writer::write(
            &existing,
            &candidate,
            Some(artwork_a.clone()),
            None,
        )
        .unwrap();
        crate::infrastructure::media::tag_writer::write(
            &temporary,
            &candidate,
            Some(artwork_b.clone()),
            None,
        )
        .unwrap();

        let published = publish_no_clobber(&temporary, &existing, None, Some(&artwork_b))
            .await
            .unwrap();

        assert_eq!(
            published,
            Publication::Written(directory.path().join("Artist - Song (2).mp3"))
        );
        assert!(!temporary.exists());
        assert_eq!(
            crate::infrastructure::media::tag_writer::read_artwork(published.path()).unwrap(),
            Some(artwork_b)
        );
        assert_eq!(
            crate::infrastructure::media::tag_writer::read_artwork(&existing).unwrap(),
            Some(artwork_a)
        );
    }

    #[tokio::test]
    async fn reused_output_moves_the_original_to_trash() {
        let directory = tempfile::tempdir().unwrap();
        let inbox = directory.path().join("inbox");
        let trash = directory.path().join("trash");
        tokio::fs::create_dir_all(&inbox).await.unwrap();
        let source = inbox.join("input.mp3");
        let temporary = directory.path().join(".temporary.mp3");
        let preferred = directory.path().join("Artist - Song.mp3");
        tokio::fs::write(&source, b"same corrected audio")
            .await
            .unwrap();
        tokio::fs::copy(&source, &temporary).await.unwrap();
        tokio::fs::copy(&source, &preferred).await.unwrap();

        let published = publish_no_clobber(&temporary, &preferred, None, None)
            .await
            .unwrap();
        assert!(published.reused_existing());
        trash_source_after_output(
            &source,
            published.path(),
            &trash,
            &directory.path().join("staging"),
            &inbox,
        )
        .await
        .unwrap();

        assert!(!source.exists());
        let (trashed, _) = files::audio_files(&trash);
        assert_eq!(trashed.len(), 1);
        assert!(trashed[0].ends_with("input.mp3"));
        assert!(preferred.exists());
        assert!(!directory.path().join("Artist - Song (2).mp3").exists());
    }

    #[tokio::test]
    async fn source_file_is_not_mistaken_for_an_existing_corrected_output() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("Artist - Song.mp3");
        let existing_output = directory.path().join("Artist - Song (2).mp3");
        let temporary = directory.path().join(".temporary.mp3");
        tokio::fs::write(&source, b"same audio").await.unwrap();
        tokio::fs::copy(&source, &existing_output).await.unwrap();
        tokio::fs::copy(&source, &temporary).await.unwrap();

        let published = publish_no_clobber(&temporary, &source, Some(&source), None)
            .await
            .unwrap();

        assert_eq!(published, Publication::Reused(existing_output));
        assert!(source.exists());
        assert!(!temporary.exists());
    }

    #[test]
    fn temporary_destination_keeps_audio_extension() {
        assert_eq!(
            temporary_destination(std::path::Path::new("/output/Artist - Song.mp3"), 42),
            PathBuf::from("/output/.Artist - Song.magpie-fi-42.mp3")
        );
    }

    #[tokio::test]
    async fn source_is_trashed_only_when_the_filed_song_exists() {
        let directory = tempfile::tempdir().unwrap();
        let inbox = directory.path().join("inbox");
        let trash = directory.path().join("trash");
        tokio::fs::create_dir_all(&inbox).await.unwrap();
        let source = inbox.join("input.mp3");
        let output = directory.path().join("output.mp3");
        tokio::fs::write(&source, b"original").await.unwrap();

        let staging = directory.path().join("staging");
        assert!(
            trash_source_after_output(&source, &output, &trash, &staging, &inbox)
                .await
                .is_err()
        );
        assert!(source.exists());

        tokio::fs::write(&output, b"corrected").await.unwrap();
        trash_source_after_output(&source, &output, &trash, &staging, &inbox)
            .await
            .unwrap();
        assert!(!source.exists());
        let (trashed, _) = files::audio_files(&trash);
        assert_eq!(tokio::fs::read(&trashed[0]).await.unwrap(), b"original");
        assert_eq!(tokio::fs::read(&output).await.unwrap(), b"corrected");
    }

    #[tokio::test]
    async fn duplicate_source_is_trashed_after_the_kept_song_exists() {
        let directory = tempfile::tempdir().unwrap();
        let config = crate::config::Config::with_folders(
            &directory.path().join("music"),
            &directory.path().join("data"),
        );
        let inbox = PathBuf::from(&config.input_dir);
        tokio::fs::create_dir_all(&inbox).await.unwrap();
        let pool =
            crate::infrastructure::db::connect(directory.path().join("d.sqlite").to_str().unwrap())
                .await
                .unwrap();
        let source = inbox.join("duplicate.mp3");
        let output = directory.path().join("Artist - Song.mp3");
        tokio::fs::write(&source, b"duplicate").await.unwrap();
        tokio::fs::write(&output, b"corrected").await.unwrap();
        let track_id = sqlx::query("INSERT INTO tracks(path,filename,status,is_missing,first_seen_at,last_seen_at,last_scanned_at,stage) VALUES(?,'duplicate.mp3','selected',0,'now','now','now','ready')")
            .bind(source.to_string_lossy().as_ref())
            .execute(&pool)
            .await
            .unwrap()
            .last_insert_rowid();
        let trash = config.trash_dir.clone();
        let state = Arc::new(AppState::new(config, pool.clone()));
        let duplicate = DuplicateSource {
            track_id: TrackId(track_id),
            filename: "duplicate.mp3".into(),
            current_path: source.to_string_lossy().into_owned(),
            source_missing: false,
        };

        finish_duplicate(&state, &duplicate, &output).await.unwrap();

        assert!(!source.exists());
        assert!(trash.exists());
        assert!(output.exists());
        let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM tracks WHERE id=?")
            .bind(track_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(remaining, 0);
    }

    #[tokio::test]
    async fn identical_source_and_destination_are_detected() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("song.mp3");
        tokio::fs::write(&source, b"audio").await.unwrap();
        assert!(
            paths_resolve_to_same_target(&source, &source)
                .await
                .unwrap()
        );
    }
}
