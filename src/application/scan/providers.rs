use super::*;

/// When SongRec (Shazam) listens to the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Listen {
    /// Only when no fingerprint match and no strong text match was found.
    WhenUnsure,
    /// Always: the text sources were asked and could not decide.
    Always,
}

pub(crate) struct Identified {
    pub(crate) candidates: Vec<infra_providers::Candidate>,
    /// SongRec was asked (it may still have recognized nothing).
    pub(crate) listened: bool,
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn identify(
    state: &Arc<AppState>,
    cfg: &crate::config::Config,
    limits: &PipelineLimits,
    path: &Path,
    fingerprint: FingerprintEvidence<'_>,
    current: &audio::AudioInfo,
    filename: &str,
    listen: Listen,
) -> Result<Identified> {
    let mut out = match query_musicbrainz_acoustid(
        state,
        cfg,
        limits,
        fingerprint.value,
        fingerprint.duration,
        current,
        filename,
    )
    .await
    {
        Ok(candidates) => candidates,
        Err(error) => {
            handle_provider_error(limits, filename, "acoustid", error).await;
            Vec::new()
        }
    };
    let acoustid_matched = !out.is_empty();
    if !cfg.youtube_api_key.trim().is_empty() {
        out.extend(query_youtube(state, limits, &cfg.youtube_api_key, current, filename).await);
    }
    out.extend(query_musicbrainz_text(state, limits, current, filename).await);
    if !provider_disabled(limits, "itunes").await {
        match query_requested_song(state, path, current, filename).await {
            Ok(candidate) => out.extend(candidate),
            Err(error) => handle_provider_error(limits, filename, "itunes", error).await,
        }
    }
    if !provider_disabled(limits, "itunes").await {
        match query_itunes(state, current, filename).await {
            Ok(candidates) => out.extend(candidates),
            Err(error) => handle_provider_error(limits, filename, "itunes", error).await,
        }
    }
    out.extend(query_deezer(state, limits, current, filename).await);
    out.extend(query_radiojavan(state, limits, current, filename).await);
    out.extend(query_audiomack(state, limits, current, filename).await);
    out.extend(query_navahang(state, limits, current, filename).await);
    if needs_genius_enrichment(&out) {
        out.extend(query_genius(state, limits, current, filename).await);
    }
    out.extend(query_deezer_isrc(state, limits, current, filename, &out).await);
    let has_strong_free_candidate = out.iter().any(|candidate| candidate.score >= 90.0);
    let mut recognized_info = None;
    let listened = infra_providers::songrec::available()
        && !is_long_mix(current.duration)
        && match listen {
            Listen::WhenUnsure => !acoustid_matched && !has_strong_free_candidate,
            Listen::Always => true,
        };
    if listened {
        let songrec_candidates =
            query_songrec(state, limits, path, fingerprint.value, filename).await;
        if let Some(recognized) = songrec_candidates.first() {
            let info = audio_info_from_recognition(recognized, current);
            out.extend(query_recognized_catalogs(state, limits, &info, filename).await);
            recognized_info = Some(info);
        }
        out.extend(songrec_candidates);
    }
    let catalog_info = recognized_info.as_ref().unwrap_or(current);
    out.extend(query_discogs(state, limits, catalog_info, filename).await);
    out.extend(query_lastfm(state, limits, catalog_info, filename).await);
    // Scores above were measured against the source's own credit. From here on (Review,
    // source agreement, approval) a match naming the file's singers is told the file's way.
    for candidate in &mut out {
        crate::application::align_with_file_credit(
            candidate,
            crate::application::FileTags {
                artist: current.artist.as_deref(),
                title: current.title.as_deref(),
            },
        );
    }
    crate::application::canonical_names::canonicalize_candidates(
        &state.pool,
        None,
        &mut out,
        crate::application::artists::Mode::Lookup,
    )
    .await?;
    apply_source_agreement(&mut out)?;
    enrich_artwork_fallbacks(&mut out);
    apply_artwork_override(&state.pool, path, &mut out).await?;
    let artist_genres = if crate::domain::genre::needs_artist_lookup(&out, catalog_info) {
        if let Some(artist) = catalog_info.artist.as_deref() {
            match infra_providers::wikidata::artist_genres(&state.pool, &state.client, artist).await
            {
                Ok(genres) => genres,
                Err(error) => {
                    tracing::warn!(
                        file = filename,
                        "artist genre enrichment failed; using track evidence: {error:#}"
                    );
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };
    crate::domain::genre::enrich(&mut out, catalog_info, &artist_genres)?;
    if out.is_empty() {
        tracing::warn!(
            file = filename,
            "no enabled metadata source returned candidates"
        );
    }
    Ok(Identified {
        candidates: out,
        listened,
    })
}

/// Files longer than this are DJ mixes, podcasts or whole concerts, not one song.
const LONG_MIX_SECONDS: f64 = 20.0 * 60.0;

/// SongRec sends Shazam a slice of the file, so on a one-hour mix it names whichever song
/// plays in that slice ("Shamdoony" for "Dor Dor 8"). Such a file is found by its tags and
/// length only (Radio Javan lists DJ mixes as podcasts).
pub(crate) fn is_long_mix(duration: f64) -> bool {
    duration > LONG_MIX_SECONDS
}

/// Looks the file's own ISRC up on Deezer (free, no key), unless a source
/// already returned that code. Text searches miss songs whose names are
/// spelled differently (Persian transliterations); the code does not.
pub(crate) async fn query_deezer_isrc(
    state: &Arc<AppState>,
    limits: &PipelineLimits,
    current: &audio::AudioInfo,
    filename: &str,
    found: &[infra_providers::Candidate],
) -> Vec<infra_providers::Candidate> {
    let Some(isrc) = current.isrc.as_deref() else {
        return Vec::new();
    };
    let already_found = found.iter().any(|candidate| {
        candidate
            .isrc
            .as_deref()
            .and_then(audio::normalize_isrc)
            .is_some_and(|found| found == isrc)
    });
    if already_found || provider_disabled(limits, "deezer").await {
        return Vec::new();
    }
    let started = Instant::now();
    match infra_providers::deezer::lookup_isrc(&state.pool, &state.client, isrc).await {
        Ok(found) => {
            let mut out = Vec::new();
            if let Some(mut candidate) = found {
                match score_text_candidate(&mut candidate, current, "deezer_isrc_lookup") {
                    Ok(()) => out.push(candidate),
                    Err(error) => {
                        tracing::warn!(file = filename, "could not score the ISRC match: {error:#}")
                    }
                }
            }
            log_provider_count(filename, "deezer_isrc", out.len(), started);
            out
        }
        Err(error) => {
            handle_provider_error(limits, filename, "deezer", error).await;
            Vec::new()
        }
    }
}

pub(crate) fn sort_candidates_for_decision(candidates: &mut [infra_providers::Candidate]) {
    candidates.sort_by(|a, b| {
        candidate_trust_tier(b)
            .cmp(&candidate_trust_tier(a))
            .then_with(|| b.score.total_cmp(&a.score))
    });
}

pub(crate) fn candidate_trust_tier(candidate: &infra_providers::Candidate) -> u8 {
    if has_audio_recognition(candidate) {
        3
    } else if matches!(
        candidate.provider.as_str(),
        "musicbrainz"
            | "itunes"
            | "deezer"
            | "spotify"
            | "radiojavan"
            | "audiomack"
            | "navahang"
            | "genius"
    ) {
        2
    } else if source_count(candidate) >= 2 {
        1
    } else {
        0
    }
}

pub(crate) fn audio_info_from_recognition(
    recognized: &infra_providers::Candidate,
    current: &audio::AudioInfo,
) -> audio::AudioInfo {
    audio::AudioInfo {
        title: Some(recognized.title.clone()),
        artist: Some(recognized.artist.clone()),
        album: recognized.album.clone(),
        album_artist: recognized.album_artist.clone(),
        duration: current.duration,
        format: current.format.clone(),
        ..Default::default()
    }
}

pub(crate) async fn query_recognized_catalogs(
    state: &Arc<AppState>,
    limits: &PipelineLimits,
    recognized: &audio::AudioInfo,
    filename: &str,
) -> Vec<infra_providers::Candidate> {
    let mut out = Vec::new();
    out.extend(query_musicbrainz_text(state, limits, recognized, filename).await);
    if !provider_disabled(limits, "itunes").await {
        match query_itunes(state, recognized, filename).await {
            Ok(candidates) => out.extend(candidates),
            Err(error) => handle_provider_error(limits, filename, "itunes", error).await,
        }
    }
    out.extend(query_deezer(state, limits, recognized, filename).await);
    out.extend(query_radiojavan(state, limits, recognized, filename).await);
    out.extend(query_audiomack(state, limits, recognized, filename).await);
    out.extend(query_navahang(state, limits, recognized, filename).await);
    if needs_genius_enrichment(&out) {
        out.extend(query_genius(state, limits, recognized, filename).await);
    }
    out
}

/// Shared scaffolding for metadata-catalog lookups: disabled-provider gate,
/// optional config-key gate, title gate, timing, error handling, and the
/// common text-scoring pass. Provider-specific steps (AcoustID, SongRec,
/// YouTube, and the alias-enriching iTunes
/// search) stay explicit in `identify`.
/// Per-provider knobs for the shared catalog runner.
struct CatalogSpec {
    /// Provider id used for the disabled gate, skip/error logs, and timing.
    provider: &'static str,
    /// `score_breakdown` source tag passed to `score_text_candidate`.
    source: &'static str,
    /// Gate the lookup on a non-empty current title.
    requires_title: bool,
    /// Config gate that returns a skip reason when a required key is missing.
    key_check: fn(&crate::config::Config) -> Option<&'static str>,
}

async fn query_catalog<F, P>(
    state: &Arc<AppState>,
    limits: &PipelineLimits,
    current: &audio::AudioInfo,
    filename: &str,
    spec: CatalogSpec,
    search: F,
) -> Vec<infra_providers::Candidate>
where
    F: FnOnce(Arc<AppState>, audio::AudioInfo) -> P,
    P: std::future::Future<Output = Result<Vec<infra_providers::Candidate>>>,
{
    if provider_disabled(limits, spec.provider).await {
        return Vec::new();
    }
    if let Some(reason) = (spec.key_check)(&state.config) {
        log_provider_skip(filename, spec.provider, reason);
        return Vec::new();
    }
    if spec.requires_title
        && current
            .title
            .as_deref()
            .is_none_or(|title| title.trim().is_empty())
    {
        return Vec::new();
    }
    let started = Instant::now();
    match search(state.clone(), current.clone()).await {
        Ok(mut candidates) => {
            for candidate in &mut candidates {
                let _ = score_text_candidate(candidate, current, spec.source);
            }
            log_provider_count(filename, spec.provider, candidates.len(), started);
            candidates
        }
        Err(error) => {
            handle_provider_error(limits, filename, spec.provider, error).await;
            Vec::new()
        }
    }
}

pub(crate) async fn query_songrec(
    state: &Arc<AppState>,
    limits: &PipelineLimits,
    path: &Path,
    fingerprint: &str,
    filename: &str,
) -> Vec<infra_providers::Candidate> {
    if provider_disabled(limits, "songrec").await {
        return Vec::new();
    }
    let started = Instant::now();
    let result = match limits.songrec.acquire().await {
        Ok(_permit) => infra_providers::songrec::recognize(&state.pool, path, fingerprint).await,
        Err(error) => Err(error.into()),
    };
    match result {
        Ok(mut candidates) => {
            for candidate in &mut candidates {
                normalize_credits(candidate);
            }
            log_provider_count(filename, "songrec", candidates.len(), started);
            candidates
        }
        Err(error) => {
            handle_provider_error(limits, filename, "songrec", error).await;
            Vec::new()
        }
    }
}

pub(crate) async fn query_youtube(
    state: &Arc<AppState>,
    limits: &PipelineLimits,
    api_key: &str,
    current: &audio::AudioInfo,
    filename: &str,
) -> Vec<infra_providers::Candidate> {
    if provider_disabled(limits, "youtube").await {
        return Vec::new();
    }
    let started = Instant::now();
    match infra_providers::youtube::lookup_filename_id(
        &state.pool,
        &state.client,
        api_key,
        filename,
    )
    .await
    {
        Ok(mut candidates) => {
            for candidate in &mut candidates {
                let _ = score_text_candidate(candidate, current, "youtube_exact_video_id");
            }
            log_provider_count(filename, "youtube", candidates.len(), started);
            candidates
        }
        Err(error) => {
            handle_provider_error(limits, filename, "youtube", error).await;
            Vec::new()
        }
    }
}

/// The Apple Music number in the link a download was asked for with: an Apple
/// song link, or a Shazam `/song/<n>/` link (Shazam uses Apple's numbers).
pub(crate) fn requested_apple_song(url: &str) -> Option<(String, Option<String>)> {
    use crate::acquire::links::{Scope, Service, classify};
    let link = classify(url).filter(|link| link.scope == Scope::Track)?;
    let numbered = !link.id.is_empty() && link.id.chars().all(|c| c.is_ascii_digit());
    match link.service {
        Service::Apple if numbered => Some((link.id, link.country)),
        // Shazam's older /track/ numbers are its own, not Apple's.
        Service::Shazam if numbered && link.url.contains("/song/") => Some((link.id, None)),
        _ => None,
    }
}

/// The song someone linked, asked for by its number. Apple's search leaves
/// some releases out ("Donyaye Movazi" by Tqvis & Ferap is found by number
/// only), and without this the song that was asked for is missing from the
/// matches. It is scored against the file like any other match.
pub(crate) async fn query_requested_song(
    state: &Arc<AppState>,
    path: &Path,
    current: &audio::AudioInfo,
    filename: &str,
) -> Result<Option<infra_providers::Candidate>> {
    let Some(url) = crate::acquire::store::requested_link(&state.pool, path).await? else {
        return Ok(None);
    };
    let Some((id, country)) = requested_apple_song(&url) else {
        return Ok(None);
    };
    let started = Instant::now();
    let mut candidate =
        infra_providers::itunes::lookup_song(&state.pool, &state.client, &id, country.as_deref())
            .await?;
    if let Some(candidate) = &mut candidate {
        score_text_candidate(candidate, current, "itunes_requested_link")?;
    }
    log_provider_count(filename, "itunes link", candidate.iter().count(), started);
    Ok(candidate)
}

pub(crate) async fn query_itunes(
    state: &Arc<AppState>,
    current: &audio::AudioInfo,
    filename: &str,
) -> Result<Vec<infra_providers::Candidate>> {
    let Some(title) = current
        .title
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(Vec::new());
    };
    let started = Instant::now();
    let mut candidates = infra_providers::itunes::search(
        &state.pool,
        &state.client,
        title,
        current.artist.as_deref(),
        current.album.as_deref(),
    )
    .await?;
    for candidate in &mut candidates {
        score_text_candidate(candidate, current, "itunes_catalog_search")?;
    }
    let needs_alias_search = current
        .artist
        .as_deref()
        .is_some_and(|artist| !artist.is_ascii())
        && candidates.iter().all(|candidate| candidate.score < 70.0);
    if needs_alias_search && let Some(artist) = current.artist.as_deref() {
        let aliases =
            infra_providers::musicbrainz::artist_aliases(&state.pool, &state.client, artist)
                .await
                .unwrap_or_default();
        for alias in aliases {
            let mut alias_info = current.clone();
            alias_info.artist = Some(alias.clone());
            let Ok(mut alias_candidates) =
                infra_providers::itunes::search(&state.pool, &state.client, "", Some(&alias), None)
                    .await
            else {
                continue;
            };
            for candidate in &mut alias_candidates {
                score_text_candidate(candidate, &alias_info, "itunes_artist_alias_search")?;
            }
            candidates.extend(alias_candidates);
        }
    }
    log_provider_count(filename, "itunes", candidates.len(), started);
    Ok(candidates)
}

pub(crate) async fn query_deezer(
    state: &Arc<AppState>,
    limits: &PipelineLimits,
    current: &audio::AudioInfo,
    filename: &str,
) -> Vec<infra_providers::Candidate> {
    query_catalog(
        state,
        limits,
        current,
        filename,
        CatalogSpec {
            provider: "deezer",
            source: "deezer_catalog_search",
            requires_title: true,
            key_check: |_| None,
        },
        |state, current| async move {
            let title = current.title.as_deref().unwrap_or_default();
            infra_providers::deezer::search(
                &state.pool,
                &state.client,
                title,
                current.artist.as_deref(),
            )
            .await
        },
    )
    .await
}

pub(crate) async fn query_radiojavan(
    state: &Arc<AppState>,
    limits: &PipelineLimits,
    current: &audio::AudioInfo,
    filename: &str,
) -> Vec<infra_providers::Candidate> {
    query_catalog(
        state,
        limits,
        current,
        filename,
        CatalogSpec {
            provider: "radiojavan",
            source: "radiojavan_catalog_search",
            requires_title: true,
            key_check: |_| None,
        },
        |state, current| async move {
            let title = current.title.as_deref().unwrap_or_default();
            infra_providers::radiojavan::search(
                &state.pool,
                &state.client,
                title,
                current.artist.as_deref(),
            )
            .await
        },
    )
    .await
}

pub(crate) async fn query_audiomack(
    state: &Arc<AppState>,
    limits: &PipelineLimits,
    current: &audio::AudioInfo,
    filename: &str,
) -> Vec<infra_providers::Candidate> {
    query_catalog(
        state,
        limits,
        current,
        filename,
        CatalogSpec {
            provider: "audiomack",
            source: "audiomack_catalog_search",
            requires_title: true,
            key_check: |_| None,
        },
        |state, current| async move {
            let title = current.title.as_deref().unwrap_or_default();
            infra_providers::audiomack::search(
                &state.pool,
                &state.client,
                title,
                current.artist.as_deref(),
            )
            .await
        },
    )
    .await
}

pub(crate) async fn query_navahang(
    state: &Arc<AppState>,
    limits: &PipelineLimits,
    current: &audio::AudioInfo,
    filename: &str,
) -> Vec<infra_providers::Candidate> {
    query_catalog(
        state,
        limits,
        current,
        filename,
        CatalogSpec {
            provider: "navahang",
            source: "navahang_catalog_search",
            requires_title: true,
            key_check: |_| None,
        },
        |state, current| async move {
            let title = current.title.as_deref().unwrap_or_default();
            infra_providers::navahang::search(
                &state.pool,
                &state.client,
                title,
                current.artist.as_deref(),
            )
            .await
        },
    )
    .await
}

pub(crate) async fn query_genius(
    state: &Arc<AppState>,
    limits: &PipelineLimits,
    current: &audio::AudioInfo,
    filename: &str,
) -> Vec<infra_providers::Candidate> {
    query_catalog(
        state,
        limits,
        current,
        filename,
        CatalogSpec {
            provider: "genius",
            source: "genius_catalog_search",
            requires_title: true,
            key_check: |_| None,
        },
        |state, current| async move {
            let title = current.title.as_deref().unwrap_or_default();
            infra_providers::genius::search(
                &state.pool,
                &state.client,
                title,
                current.artist.as_deref(),
            )
            .await
        },
    )
    .await
}

pub(crate) fn needs_genius_enrichment(candidates: &[infra_providers::Candidate]) -> bool {
    !candidates.iter().any(|candidate| {
        candidate.score >= 90.0
            && candidate
                .album
                .as_deref()
                .is_some_and(|album| !album.trim().is_empty())
            && candidate
                .cover_url
                .as_deref()
                .is_some_and(|cover| !cover.trim().is_empty())
    })
}

pub(crate) async fn query_musicbrainz_acoustid(
    state: &Arc<AppState>,
    cfg: &crate::config::Config,
    limits: &PipelineLimits,
    fingerprint: &str,
    duration: f64,
    current: &audio::AudioInfo,
    filename: &str,
) -> Result<Vec<infra_providers::Candidate>> {
    let mut out = Vec::new();
    if cfg.acoustid_key.is_empty() || fingerprint.is_empty() {
        log_provider_skip(filename, "acoustid", "AcoustID API key is missing");
        return Ok(out);
    }
    let started = Instant::now();
    let hits = {
        let _permit = limits.acoustid.acquire().await?;
        infra_providers::acoustid::lookup(
            &state.pool,
            &state.client,
            &cfg.acoustid_key,
            fingerprint,
            duration,
        )
        .await?
    };
    log_provider_count(filename, "acoustid", hits.len(), started);
    for hit in hits.into_iter().take(3) {
        let mut candidate =
            infra_providers::musicbrainz::recording(&state.pool, &state.client, &hit.recording_id)
                .await?;
        let candidate_duration = candidate.duration_delta;
        let outcome = crate::domain::matcher::score_candidate(
            current,
            crate::domain::matcher::CandidateInput {
                title: &candidate.title,
                artist: &candidate.artist,
                album: candidate.album.as_deref(),
                candidate_duration,
            },
            crate::domain::matcher::ScoreMode::Acoustid {
                acoustid_score: hit.score,
                is_compilation: candidate.is_compilation,
            },
            &candidate.provider,
        )?;
        candidate.score = outcome.score;
        candidate.duration_delta = outcome.duration_delta;
        candidate.score_breakdown = Some(outcome.breakdown_json);
        normalize_credits(&mut candidate);
        out.push(candidate);
    }
    Ok(out)
}

pub(crate) async fn query_musicbrainz_text(
    state: &Arc<AppState>,
    limits: &PipelineLimits,
    current: &audio::AudioInfo,
    filename: &str,
) -> Vec<infra_providers::Candidate> {
    query_catalog(
        state,
        limits,
        current,
        filename,
        CatalogSpec {
            provider: "musicbrainz",
            source: "musicbrainz_text_search",
            requires_title: true,
            key_check: |_| None,
        },
        |state, current| async move {
            let title = current.title.as_deref().unwrap_or_default();
            infra_providers::musicbrainz::search(
                &state.pool,
                &state.client,
                title,
                current.artist.as_deref(),
            )
            .await
        },
    )
    .await
}

pub(crate) async fn query_discogs(
    state: &Arc<AppState>,
    limits: &PipelineLimits,
    current: &audio::AudioInfo,
    filename: &str,
) -> Vec<infra_providers::Candidate> {
    query_catalog(
        state,
        limits,
        current,
        filename,
        CatalogSpec {
            provider: "discogs",
            source: "discogs_release_search",
            requires_title: false,
            key_check: |cfg| {
                cfg.discogs_token
                    .trim()
                    .is_empty()
                    .then_some("Discogs token/API key is missing")
            },
        },
        |state, current| async move {
            let token = state.config.discogs_token.trim().to_owned();
            infra_providers::discogs::search(&state.pool, &state.client, Some(&token), &current)
                .await
        },
    )
    .await
}

pub(crate) async fn query_lastfm(
    state: &Arc<AppState>,
    limits: &PipelineLimits,
    current: &audio::AudioInfo,
    filename: &str,
) -> Vec<infra_providers::Candidate> {
    query_catalog(
        state,
        limits,
        current,
        filename,
        CatalogSpec {
            provider: "lastfm",
            source: "lastfm_track_search",
            requires_title: false,
            key_check: |cfg| {
                cfg.lastfm_key
                    .trim()
                    .is_empty()
                    .then_some("Last.fm API key is missing")
            },
        },
        |state, current| async move {
            let api_key = state.config.lastfm_key.trim().to_owned();
            infra_providers::lastfm::search(&state.pool, &state.client, &api_key, &current).await
        },
    )
    .await
}

/// Scores a text-search candidate against the file's current tags.
pub(crate) fn score_text_candidate(
    candidate: &mut infra_providers::Candidate,
    current: &audio::AudioInfo,
    source: &str,
) -> Result<()> {
    normalize_credits(candidate);
    preserve_album_context_for_catalog_single(candidate, current);
    let outcome = crate::domain::matcher::score_candidate(
        current,
        crate::domain::matcher::CandidateInput {
            title: &candidate.title,
            artist: &candidate.artist,
            album: candidate.album.as_deref(),
            candidate_duration: candidate.duration_delta,
        },
        crate::domain::matcher::ScoreMode::Text {
            source,
            provider_label: provider_name(&candidate.provider),
        },
        &candidate.provider,
    )?;
    candidate.duration_delta = outcome.duration_delta;
    candidate.score = outcome.score;
    candidate.score_breakdown = Some(outcome.breakdown_json);
    Ok(())
}

pub(crate) fn preserve_album_context_for_catalog_single(
    candidate: &mut infra_providers::Candidate,
    current: &audio::AudioInfo,
) {
    let Some(current_album) = current
        .album
        .as_deref()
        .filter(|album| !album.trim().is_empty() && !album.trim().starts_with('@'))
    else {
        return;
    };
    let is_catalog_single = candidate
        .album
        .as_deref()
        .is_some_and(|album| album.to_lowercase().contains("single"));
    let album_track = current.track_number.is_some_and(|number| number > 1);
    let duration_close = candidate
        .duration_delta
        .is_some_and(|duration| (current.duration - duration).abs() <= 5.0);
    let text_exact = current
        .title
        .as_deref()
        .is_some_and(|title| title_similarity(title, &candidate.title) >= 0.94)
        && current
            .artist
            .as_deref()
            .is_some_and(|artist| artist_similarity(artist, &candidate.artist) >= 0.90);
    if is_catalog_single && album_track && duration_close && text_exact {
        candidate.album = Some(current_album.to_owned());
        candidate.track_number = current.track_number.map(i64::from);
        candidate.track_total = None;
        candidate.release_id = None;
    }
}

pub(crate) fn apply_source_agreement(candidates: &mut [infra_providers::Candidate]) -> Result<()> {
    let snapshot = candidates.to_vec();
    for candidate in candidates {
        let mut sources = candidate_source_list(candidate);
        let mut agreeing_providers = HashSet::new();
        let mut disagreeing_providers = HashSet::new();
        for other in &snapshot {
            if other.provider == candidate.provider {
                continue;
            }
            if candidate_agrees(candidate, other) {
                sources.push(provider_name(&other.provider).to_owned());
                if agreeing_providers.insert(other.provider.as_str()) {
                    let cap = if has_audio_recognition(candidate) {
                        99.0
                    } else {
                        98.0
                    };
                    candidate.score = (candidate.score + 6.0).min(cap);
                }
            } else if text_close(&candidate.title, &other.title, 0.55)
                && !text_close(&candidate.artist, &other.artist, 0.55)
                && disagreeing_providers.insert(other.provider.as_str())
            {
                candidate.score = (candidate.score - 4.0).max(0.0);
            }
        }
        set_score_sources(candidate, sources)?;
    }
    Ok(())
}

/// Gives each candidate the covers of every agreeing catalog row, best
/// provider first, so a broken primary cover can fall back to another one.
pub(crate) fn enrich_artwork_fallbacks(candidates: &mut [infra_providers::Candidate]) {
    let covers = (0..candidates.len())
        .map(|index| {
            let candidate = &candidates[index];
            let own_url = candidate.cover_url.as_deref();
            let mut sources = candidates
                .iter()
                .enumerate()
                .filter(|(other_index, other)| {
                    other.cover_url.is_some()
                        && (*other_index == index || artwork_agrees(candidate, other))
                })
                .map(|(_, other)| other)
                .collect::<Vec<_>>();
            sources.sort_by(|left, right| {
                artwork_provider_priority(&right.provider)
                    .cmp(&artwork_provider_priority(&left.provider))
                    .then_with(|| right.score.total_cmp(&left.score))
            });
            sources.dedup_by(|left, right| left.cover_url == right.cover_url);
            sources
                .into_iter()
                .map(|source| infra_providers::ArtworkCandidate {
                    provider: provider_name(&source.provider).to_owned(),
                    url: source.cover_url.clone().unwrap_or_default(),
                    own_record: source.cover_url.as_deref() == own_url,
                    release_id: source.release_id.clone(),
                    isrc: source.isrc.clone(),
                    album: source.album.clone(),
                    artist: Some(source.artist.clone()),
                    ..Default::default()
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    for (candidate, artwork) in candidates.iter_mut().zip(covers) {
        if let Some(best) = artwork.first() {
            candidate.cover_url = Some(best.url.clone());
        }
        candidate.artwork_candidates = artwork;
    }
}

pub(crate) fn artwork_agrees(
    left: &infra_providers::Candidate,
    right: &infra_providers::Candidate,
) -> bool {
    if !candidate_agrees(left, right) {
        return false;
    }
    if left
        .isrc
        .as_deref()
        .zip(right.isrc.as_deref())
        .is_some_and(|(left_isrc, right_isrc)| left_isrc.eq_ignore_ascii_case(right_isrc))
    {
        return true;
    }
    match (left.album.as_deref(), right.album.as_deref()) {
        (Some(left_album), Some(right_album)) => text_close(left_album, right_album, 0.65),
        (Some(_), None) => false,
        _ => true,
    }
}

pub(crate) fn artwork_provider_priority(provider: &str) -> u8 {
    match provider {
        "itunes" | "spotify" => 8,
        "musicbrainz" | "radiojavan" | "audiomack" | "navahang" | "genius" => 7,
        "soundcloud" => 6,
        "deezer" => 5,
        "discogs" => 4,
        "lastfm" => 2,
        "youtube" => 1,
        _ => 0,
    }
}

pub(crate) async fn apply_artwork_override(
    pool: &sqlx::SqlitePool,
    path: &Path,
    candidates: &mut [infra_providers::Candidate],
) -> Result<()> {
    let override_row: Option<(String, String, String)> =
        sqlx::query_as("SELECT title,artist,cover_url FROM artwork_overrides WHERE path=?")
            .bind(path.to_string_lossy().as_ref())
            .fetch_optional(pool)
            .await?;
    let Some((title, artist, cover_url)) = override_row else {
        return Ok(());
    };
    let reference = infra_providers::Candidate {
        title,
        artist,
        ..Default::default()
    };
    for candidate in candidates
        .iter_mut()
        .filter(|candidate| candidate_agrees(candidate, &reference))
    {
        candidate.cover_url = Some(cover_url.clone());
        candidate.artwork_candidates.insert(
            0,
            infra_providers::ArtworkCandidate {
                provider: "User verified".into(),
                url: cover_url.clone(),
                user_confirmed: true,
                own_record: false,
                same_recording: false,
                release_id: candidate.release_id.clone(),
                isrc: candidate.isrc.clone(),
                album: candidate.album.clone(),
                artist: Some(candidate.artist.clone()),
            },
        );
    }
    Ok(())
}

pub(crate) fn dedupe_candidates(candidates: &mut Vec<infra_providers::Candidate>) {
    candidates.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut seen = std::collections::HashSet::new();
    candidates.retain(|candidate| {
        let key = [
            normalize_match_key(&candidate.title),
            normalize_match_key(&candidate.artist),
            normalize_match_key(candidate.album.as_deref().unwrap_or_default()),
        ]
        .join("|");
        seen.insert(key)
    });
}

pub(crate) fn candidate_agrees(
    left: &infra_providers::Candidate,
    right: &infra_providers::Candidate,
) -> bool {
    if left
        .isrc
        .as_deref()
        .zip(right.isrc.as_deref())
        .is_some_and(|(left_isrc, right_isrc)| left_isrc.eq_ignore_ascii_case(right_isrc))
    {
        return true;
    }
    text_close(&left.title, &right.title, 0.82)
        && (text_close(&left.artist, &right.artist, 0.75)
            || crate::domain::versions::artists_similarity(&left.artist, &right.artist) >= 0.99)
        && match (left.album.as_deref(), right.album.as_deref()) {
            (Some(left_album), Some(right_album)) => text_close(left_album, right_album, 0.65),
            _ => true,
        }
}

pub(crate) fn candidate_source_list(candidate: &infra_providers::Candidate) -> Vec<String> {
    let mut out = vec![provider_name(&candidate.provider).to_owned()];
    if let Some(value) = candidate
        .score_breakdown
        .as_deref()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        && let Some(sources) = value["sources"].as_array()
    {
        out.extend(
            sources
                .iter()
                .filter_map(|source| source.as_str().map(str::to_owned)),
        );
    }
    out.sort();
    out.dedup();
    out
}

pub(crate) fn set_score_sources(
    candidate: &mut infra_providers::Candidate,
    sources: Vec<String>,
) -> Result<()> {
    let mut value = candidate
        .score_breakdown
        .as_deref()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    value["sources"] = serde_json::to_value(sources)?;
    value["source_agreement"] = serde_json::json!(value["sources"].as_array().map_or(0, Vec::len));
    value["final_score"] = serde_json::json!(candidate.score);
    candidate.score_breakdown = Some(value.to_string());
    Ok(())
}

pub(crate) fn log_provider_count(filename: &str, provider: &str, count: usize, started: Instant) {
    tracing::debug!(
        file = filename,
        provider,
        count,
        duration_ms = started.elapsed().as_millis() as u64,
        "provider lookup finished"
    );
}

pub(crate) fn log_provider_skip(filename: &str, provider: &str, reason: &str) {
    tracing::debug!(file = filename, provider, "provider skipped: {reason}");
}

/// Why a source is not asked right now.
pub(crate) struct ProviderPause {
    /// `None`: off for the rest of this scan (a wrong key does not fix itself).
    pub(super) until: Option<Instant>,
    failures: u32,
}

pub(crate) async fn provider_disabled(limits: &PipelineLimits, provider: &str) -> bool {
    let pauses = limits.disabled_providers.lock().await;
    let Some(pause) = pauses.get(provider) else {
        return false;
    };
    match pause.until {
        // Off for the whole scan (a wrong key): nothing to wait for.
        None => true,
        Some(until) if Instant::now() < until => {
            // A song goes on without this source. It is counted, so the songs
            // identified meanwhile can get a second look (see `run_files`).
            limits
                .lookups_missed
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            true
        }
        Some(_) => false,
    }
}

/// Pauses a source after a failure. A refused key turns it off for the rest of
/// the scan; a rate limit or timeout pauses it for 30 s, doubling up to 5
/// minutes, so a busy source is asked again later instead of never. Returns
/// how long, or `None` when it was already paused.
pub(crate) async fn pause_provider(
    limits: &PipelineLimits,
    provider: &str,
    permanent: bool,
) -> Option<String> {
    let mut pauses = limits.disabled_providers.lock().await;
    let now = Instant::now();
    let pause = pauses.entry(provider.to_owned()).or_insert(ProviderPause {
        until: Some(now),
        failures: 0,
    });
    let until = pause.until?;
    if now < until {
        return None;
    }
    if permanent {
        pause.until = None;
        return Some("for the rest of this scan".into());
    }
    pause.failures += 1;
    let seconds = (30_u64 << (pause.failures - 1).min(4)).min(300);
    pause.until = Some(now + Duration::from_secs(seconds));
    Some(format!("for {seconds} s"))
}

pub(crate) async fn handle_provider_error(
    limits: &PipelineLimits,
    filename: &str,
    provider: &str,
    error: anyhow::Error,
) {
    let error_text = format!("{error:#}");
    let lower = error_text.to_ascii_lowercase();
    let pause_reason = if lower.contains("401 unauthorized") {
        Some((
            "authentication failed; check the configured API key/token",
            true,
        ))
    } else if lower.contains("429 too many requests")
        // Sources also say "slow down" in other words: Apple and sites behind
        // Cloudflare answer 403, Deezer names its quota, a busy server 503.
        // Each of these pauses the source for a while, so a long scan does
        // not keep knocking and get the address blocked.
        || lower.contains("403 forbidden")
        || lower.contains("503 service unavailable")
        || lower.contains("quota")
        || lower.contains("rate limit")
    {
        Some(("rate limited by provider", false))
    } else if lower.contains("504 gateway timeout") || lower.contains("timed out") {
        Some(("provider timed out", false))
    } else {
        None
    };

    if let Some((reason, permanent)) = pause_reason {
        if !permanent {
            limits
                .lookups_missed
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        if let Some(how_long) = pause_provider(limits, provider, permanent).await {
            tracing::warn!(
                file = filename,
                provider,
                error = error_text,
                "provider paused {how_long}: {reason}"
            );
        }
        return;
    }
    tracing::error!(
        file = filename,
        provider,
        error = error_text,
        "provider lookup failed"
    );
}
