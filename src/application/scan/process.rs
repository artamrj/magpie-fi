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
    // Text sources first; SongRec listens only when they are not sure. If the
    // text alone still cannot decide, listen once more before asking a person:
    // the audio settles most "which of these is it?" questions.
    let mut listen = Listen::WhenUnsure;
    loop {
        let identified = identify(
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
            listen,
        )
        .await?;
        let can_listen_again = !identified.listened
            && listen == Listen::WhenUnsure
            && infra_providers::songrec::available();
        let mut candidates = identified.candidates;
        let raw_candidate_count = candidates.len();
        dedupe_candidates(&mut candidates);
        sort_candidates_for_decision(&mut candidates);
        tracing::info!(
            file,
            raw = raw_candidate_count,
            usable = candidates.len(),
            listened = identified.listened,
            "providers returned candidates"
        );
        let outcome = decide(state, &info, filename, &candidates).await?;
        if !matches!(outcome, Decided::Filed { .. }) && can_listen_again {
            tracing::info!(
                file,
                "the text sources could not decide; listening to the audio"
            );
            listen = Listen::Always;
            continue;
        }
        return match outcome {
            Decided::Filed { candidate, message } => {
                tracing::info!(
                    file,
                    score = candidate.score.round(),
                    artist = candidate.artist,
                    title = candidate.title,
                    "selected a match"
                );
                persist_match(&state.pool, path, &info, &candidate, &message).await?;
                Ok(ProcessOutcome::Matched)
            }
            Decided::NothingFound => {
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
                Ok(ProcessOutcome::NeedsReview)
            }
            Decided::Unsure { reason } => {
                if candidates[0].score >= 40.0 {
                    persist_review(&state.pool, path, &info, &candidates, &reason).await?;
                } else {
                    persist_unmatched(
                        &state.pool,
                        path,
                        &info,
                        "No candidate met strict matching rules; counted as unmatched",
                    )
                    .await?;
                }
                tracing::warn!(file, "{reason}");
                Ok(ProcessOutcome::NeedsReview)
            }
            Decided::Incomplete {
                index,
                candidate,
                missing,
            } => {
                candidates[index] = candidate;
                candidates.swap(0, index);
                let message = format!(
                    "The song was recognized, but no {} was found for this release, so it waits here instead of being filed incomplete.",
                    missing.join(" or ")
                );
                persist_review(&state.pool, path, &info, &candidates, &message).await?;
                tracing::warn!(file, "{message}");
                Ok(ProcessOutcome::NeedsReview)
            }
        };
    }
}

/// How many other releases of the same recording are tried when the chosen
/// one has no verified cover. Each try may download covers.
const MAX_ALTERNATE_RELEASES: usize = 3;

enum Decided {
    Filed {
        candidate: infra_providers::Candidate,
        message: String,
    },
    NothingFound,
    Unsure {
        reason: String,
    },
    /// The recording is known, but no release of it has everything filing needs.
    Incomplete {
        index: usize,
        candidate: infra_providers::Candidate,
        missing: Vec<String>,
    },
}

/// Picks the recording, then the first of its releases that can be filed
/// complete (title, artist, album, verified cover).
async fn decide(
    state: &Arc<AppState>,
    info: &audio::AudioInfo,
    filename: &str,
    candidates: &[infra_providers::Candidate],
) -> Result<Decided> {
    if candidates.is_empty() {
        return Ok(Decided::NothingFound);
    }
    let evidence = || crate::application::smart_approval::TrackEvidence {
        filename,
        title: info.title.as_deref(),
        artist: info.artist.as_deref(),
        album: info.album.as_deref(),
        isrc: info.isrc.as_deref(),
    };
    let Some(ranked) = crate::application::smart_approval::rank(evidence(), candidates) else {
        return Ok(Decided::Unsure {
            reason: crate::application::smart_approval::unsure_reason(evidence(), candidates),
        });
    };
    let mut first_incomplete = None;
    let releases = std::iter::once(ranked.candidate_index).chain(
        ranked
            .alternates
            .iter()
            .copied()
            .take(MAX_ALTERNATE_RELEASES),
    );
    for index in releases {
        let mut candidate = candidates[index].clone();
        let completion = crate::application::finalize_selection(
            state,
            &mut candidate,
            Some(crate::application::Donors {
                candidates,
                existing_album: info.album.as_deref(),
            }),
            Some(crate::application::FileTags {
                artist: info.artist.as_deref(),
                title: info.title.as_deref(),
            }),
        )
        .await?;
        if completion.core_complete {
            let fallback = if index == ranked.candidate_index {
                ""
            } else {
                "; the best release had no usable cover, so another release of the same song was used"
            };
            let message = format!(
                "Smart auto-selected ({:.0}%): {}{fallback}; {}",
                ranked.confidence,
                ranked.explanation,
                completion.summary()
            );
            return Ok(Decided::Filed { candidate, message });
        }
        if first_incomplete.is_none() {
            let missing = completion
                .missing_fields
                .iter()
                .filter(|field| matches!(field.as_str(), "title" | "artist" | "album" | "cover"))
                .cloned()
                .collect();
            first_incomplete = Some((index, candidate, missing));
        }
    }
    let (index, candidate, missing) = first_incomplete.expect("the chosen release is always tried");
    Ok(Decided::Incomplete {
        index,
        candidate,
        missing,
    })
}
