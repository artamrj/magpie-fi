use super::*;

pub(crate) async fn process_file(
    state: &Arc<AppState>,
    limits: &PipelineLimits,
    job: FileJob,
    total: usize,
) {
    let mut members = job.members.into_iter();
    while let Some(member) = members.next() {
        if state.workflow_cancelled().await {
            return;
        }
        let filename = member
            .path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("audio")
            .to_owned();
        let file = member.path.display().to_string();
        const ATTEMPTS: usize = 2;
        let mut outcome = None;
        for attempt in 1..=ATTEMPTS {
            if state.workflow_cancelled().await {
                return;
            }
            state
                .start_track(
                    total,
                    filename.clone(),
                    format!("Matching {filename} · attempt {attempt}/{ATTEMPTS}"),
                )
                .await;
            match process(state, limits, &member).await {
                Ok(result) => {
                    outcome = Some(result);
                    break;
                }
                Err(error) if attempt < ATTEMPTS => {
                    tracing::warn!(file, attempt, "track attempt failed; retrying: {error:#}");
                    tokio::time::sleep(Duration::from_millis(400 * attempt as u64)).await;
                }
                Err(error) => {
                    let error_text = format!("{error:#}");
                    tracing::error!(
                        file,
                        attempt,
                        error = error_text,
                        "track failed after retries"
                    );
                    if let Err(persist_error) =
                        persist_failed(&state.pool, &member.path, &error_text).await
                    {
                        tracing::error!(file, "failed to persist failed track: {persist_error:#}");
                    }
                }
            }
        }
        if let Err(error) = persist_analysis_identity(&state.pool, &member).await {
            tracing::warn!(file, "could not persist input identity: {error:#}");
        }
        finish_scan_progress(state, total).await;
        if matches!(
            outcome,
            Some(ProcessOutcome::Matched | ProcessOutcome::NeedsReview)
        ) {
            persist_duplicate_members(state, &member, members.collect(), total).await;
            return;
        }
    }
}

pub(crate) async fn process(
    state: &Arc<AppState>,
    limits: &PipelineLimits,
    input: &PreparedInput,
) -> Result<ProcessOutcome> {
    let path = input.path.as_path();
    let filename = path.file_name().and_then(|v| v.to_str()).unwrap_or("audio");
    let file = path.display().to_string();
    let info = match &input.info {
        Some(info) => info.clone(),
        None => read_info(limits, path).await?,
    };
    tracing::debug!(
        file,
        format = info.format,
        duration = info.duration.round(),
        title = info.title.as_deref().unwrap_or("missing"),
        "read tags"
    );
    let integrity = match &input.integrity {
        Some(integrity) => integrity.clone(),
        None => check_integrity(state, path).await,
    };
    match integrity {
        IntegrityCheck::Healthy => {}
        IntegrityCheck::Corrupt(diagnostic) => {
            persist_corrupt(&state.pool, path, &info, &diagnostic).await?;
            tracing::error!(
                file,
                diagnostic,
                "audio file is damaged; blocked from metadata writing"
            );
            return Ok(ProcessOutcome::Corrupt);
        }
        IntegrityCheck::Unavailable(error) => {
            tracing::warn!(
                file,
                error,
                "integrity check unavailable; continuing metadata matching"
            );
        }
    }
    let started = Instant::now();
    let fingerprint_result = {
        let _permit = limits.fingerprint.acquire().await?;
        fingerprint_cache::get_or_calculate(&state.pool, path, || async {
            fingerprint::calculate(path)
                .await
                .with_context(|| format!("failed to fingerprint {}", path.display()))
        })
        .await
    };
    let (fp, duration) = match fingerprint_result {
        Ok(result) => {
            tracing::debug!(
                file,
                cached = matches!(result.source, fingerprint_cache::FingerprintSource::Cache),
                duration_ms = started.elapsed().as_millis() as u64,
                "fingerprint ready"
            );
            (result.fingerprint, result.duration)
        }
        Err(error) => {
            tracing::warn!(
                file,
                "fingerprint unavailable; continuing with text and web sources: {error:#}"
            );
            (String::new(), info.duration)
        }
    };
    let cfg = &state.config;
    let mut candidates = identify(
        state,
        cfg,
        limits,
        path,
        FingerprintEvidence {
            value: &fp,
            duration,
        },
        &info,
        filename,
    )
    .await?;
    let raw_candidate_count = candidates.len();
    dedupe_candidates(&mut candidates);
    sort_candidates_for_decision(&mut candidates);
    tracing::info!(
        file,
        raw = raw_candidate_count,
        usable = candidates.len(),
        "providers returned candidates"
    );
    let Some(best) = candidates.first() else {
        let fingerprint_note = if fp.is_empty() {
            " Fingerprint creation failed; install Chromaprint (fpcalc) to identify difficult tracks."
        } else if cfg.acoustid_key.is_empty() {
            " A fingerprint was created, but online fingerprint lookup needs an AcoustID API key."
        } else {
            ""
        };
        let message = format!(
            "No catalog match from Apple Music, Deezer, MusicBrainz, or the enabled optional sources.{fingerprint_note}"
        );
        persist_unmatched(&state.pool, path, &info, &message).await?;
        tracing::warn!(file, "{message}");
        return Ok(ProcessOutcome::NeedsReview);
    };
    let smart_decision = crate::application::smart_approval::rank(
        crate::application::smart_approval::TrackEvidence {
            filename,
            title: info.title.as_deref(),
            artist: info.artist.as_deref(),
            album: info.album.as_deref(),
        },
        &candidates,
    );
    let Some(smart_decision) = smart_decision else {
        let message = if best.score >= 40.0 {
            let mut source_names = candidates
                .iter()
                .flat_map(candidate_source_list)
                .collect::<HashSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            source_names.sort();
            let sources = source_names.join(", ");
            let title_match = info
                .title
                .as_deref()
                .is_some_and(|title| title_similarity(title, &best.title) >= 0.94);
            let artist_match = info
                .artist
                .as_deref()
                .is_some_and(|artist| artist_similarity(artist, &best.artist) >= 0.75);
            match (
                candidates.len(),
                info.album.as_deref(),
                best.album.as_deref(),
            ) {
                _ if title_match
                    && artist_match
                    && best.duration_delta.is_some_and(|delta| delta > 15.0) =>
                {
                    format!(
                        "Found matching catalog metadata, but this audio is {:.0} seconds longer or shorter; it may be a music-video, live, or edited version. Review before applying it.",
                        best.duration_delta.unwrap_or_default()
                    )
                }
                _ if title_match && !artist_match => format!(
                    "Found this title in {sources}, but only by different performers; keep the current artist or enter this performance manually."
                ),
                (1, Some(existing), Some(found)) if text_similarity(existing, found) < 0.65 => {
                    format!(
                        "Found one close match from {sources}, but its album “{found}” conflicts with the existing album “{existing}”; review before applying it."
                    )
                }
                _ => format!(
                    "Found {} possible release(s) from {sources}, but the release is ambiguous; choose the correct one.",
                    candidates.len()
                ),
            }
        } else {
            "No candidate met strict matching rules; counted as unmatched".to_owned()
        };
        if best.score >= 40.0 {
            persist_review(&state.pool, path, &info, &candidates, &message).await?;
        } else {
            persist_unmatched(&state.pool, path, &info, &message).await?;
        }
        tracing::warn!(file, "{message}");
        return Ok(ProcessOutcome::NeedsReview);
    };
    let mut candidate = candidates[smart_decision.candidate_index].clone();
    let completion = crate::application::finalize_selection(
        state,
        &mut candidate,
        Some(crate::application::Donors {
            candidates: &candidates,
            existing_album: info.album.as_deref(),
        }),
        Some(crate::application::FileTags {
            artist: info.artist.as_deref(),
            title: info.title.as_deref(),
        }),
    )
    .await?;
    if !completion.core_complete {
        candidates[smart_decision.candidate_index] = candidate;
        candidates.swap(0, smart_decision.candidate_index);
        let message = format!(
            "A recording match was found, but the metadata worker could not find {}. It remains in review instead of creating an incomplete identification.",
            completion.missing_fields.join(", ")
        );
        persist_review(&state.pool, path, &info, &candidates, &message).await?;
        tracing::warn!(file, "{message}");
        return Ok(ProcessOutcome::NeedsReview);
    }
    let selection_message = format!(
        "Smart auto-selected ({:.0}%): {}; {}",
        smart_decision.confidence,
        smart_decision.explanation,
        completion.summary()
    );
    tracing::info!(
        file,
        score = candidate.score.round(),
        artist = candidate.artist,
        title = candidate.title,
        "selected a match"
    );
    persist_match(&state.pool, path, &info, &candidate, &selection_message).await?;
    Ok(ProcessOutcome::Matched)
}
