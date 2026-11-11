use crate::domain::versions;
pub(crate) use crate::domain::versions::version_tags;
use crate::infrastructure::providers::Candidate;
use std::collections::{BTreeSet, HashSet};

#[derive(Default)]
pub struct TrackEvidence<'a> {
    pub filename: &'a str,
    pub title: Option<&'a str>,
    pub artist: Option<&'a str>,
    pub album: Option<&'a str>,
    /// The file's own ISRC tag, already in its plain form.
    pub isrc: Option<&'a str>,
}

#[derive(Debug)]
pub struct Decision {
    pub candidate_id: i64,
    pub confidence: f64,
    pub explanation: String,
}

#[derive(Debug)]
pub struct RankedDecision {
    pub candidate_index: usize,
    pub confidence: f64,
    pub explanation: String,
}

struct Evaluated<'a> {
    index: usize,
    candidate: &'a Candidate,
    total: f64,
    title_similarity: f64,
    artist_similarity: f64,
    title_available: bool,
    artist_available: bool,
    sources: usize,
    audio_recognition: bool,
    /// The source gives the file's own ISRC and the same length.
    isrc_proof: bool,
    duration_close: bool,
    /// The source gives no length, and every other source that lists the
    /// same song with a length is this far (more than 15 s) from the file.
    length_contradicted: Option<f64>,
    album_similarity: Option<f64>,
    variants: BTreeSet<&'static str>,
    variant_conflict: bool,
    original_album: bool,
}

pub fn select(evidence: TrackEvidence<'_>, candidates: &[Candidate]) -> Option<Decision> {
    let ranked = rank(evidence, candidates)?;
    Some(Decision {
        candidate_id: candidates.get(ranked.candidate_index)?.id?,
        confidence: ranked.confidence,
        explanation: ranked.explanation,
    })
}

/// Every usable candidate measured against the file, best first.
fn evaluate_all<'a>(
    evidence: &TrackEvidence<'_>,
    candidates: &'a [Candidate],
) -> Vec<Evaluated<'a>> {
    let source_text = [evidence.filename, evidence.title.unwrap_or_default()].join(" ");
    let expected_variants = version_tags(&source_text);
    let mut evaluated = candidates
        .iter()
        .enumerate()
        .filter(|(_, candidate)| {
            !candidate.title.trim().is_empty() && !candidate.artist.trim().is_empty()
        })
        .map(|(index, candidate)| {
            evaluate(index, evidence, candidate, &expected_variants, candidates)
        })
        .collect::<Vec<_>>();
    evaluated.sort_by(|left, right| right.total.total_cmp(&left.total));
    evaluated
}

pub fn rank(evidence: TrackEvidence<'_>, candidates: &[Candidate]) -> Option<RankedDecision> {
    let evaluated = evaluate_all(&evidence, candidates)
        .into_iter()
        .filter(is_supported)
        .collect::<Vec<_>>();

    let best = evaluated.first()?;
    if best.total < 68.0 {
        return None;
    }
    // Other releases of the same recording (the single, the album, a store that
    // lists fewer singers) are not rivals: any of them is the right song. The
    // margin is kept against the best match that is a different recording.
    if let Some(rival) = evaluated
        .iter()
        .skip(1)
        .find(|other| !same_recording(best, other))
        && best.total - rival.total < 12.0
    {
        return None;
    }

    let mut reasons = Vec::new();
    if best.isrc_proof {
        reasons.push("same ISRC as the file".to_owned());
    }
    if best.audio_recognition {
        reasons.push("audio recognition".to_owned());
    }
    if best.title_similarity >= 0.90 && best.artist_similarity >= 0.82 {
        reasons.push("exact title and artist".to_owned());
    }
    if best.sources >= 2 {
        reasons.push(format!("{}-source agreement", best.sources));
    }
    if let Some(delta) = best.candidate.duration_delta
        && delta <= 8.0
    {
        reasons.push(format!("{delta:.1}s duration match"));
    }
    if best
        .album_similarity
        .is_some_and(|similarity| similarity >= 0.82)
    {
        reasons.push("existing album match".to_owned());
    } else if best.original_album {
        reasons.push("original album release preferred".to_owned());
    } else if is_album_release(best.candidate) {
        reasons.push("album release preferred".to_owned());
    }

    let confidence = best.total.clamp(0.0, 99.0);
    Some(RankedDecision {
        candidate_index: best.index,
        confidence,
        explanation: if reasons.is_empty() {
            "strong combined evidence".to_owned()
        } else {
            reasons.join(", ")
        },
    })
}

/// Why [`rank`] would not choose on its own, in plain words for the Review
/// screen: the one thing a person should check.
pub fn unsure_reason(evidence: TrackEvidence<'_>, candidates: &[Candidate]) -> String {
    let evaluated = evaluate_all(&evidence, candidates);
    let supported = evaluated
        .iter()
        .filter(|candidate| is_supported(candidate))
        .collect::<Vec<_>>();
    if let Some(best) = supported.first() {
        if let Some(rival) = supported
            .iter()
            .skip(1)
            .find(|other| !same_recording(best, other))
            && best.total - rival.total < 12.0
        {
            return format!(
                "Two different recordings fit almost equally well: {} and {}.",
                describe(best.candidate),
                describe(rival.candidate)
            );
        }
        return format!(
            "The best match, {}, is not certain enough to file without a look.",
            describe(best.candidate)
        );
    }
    let Some(top) = evaluated.first() else {
        return "No source found this song.".to_owned();
    };
    let found = describe(top.candidate);
    let source = crate::domain::provider_name(&top.candidate.provider);
    if let Some(delta) = top
        .candidate
        .duration_delta
        .filter(|delta| *delta > MAX_AUTOMATIC_DURATION_GAP_SECONDS)
    {
        return format!(
            "The closest match, {found}, is {delta:.0} seconds longer or shorter than this file: it may be a music video, a live or an edited version."
        );
    }
    if let Some(delta) = top.length_contradicted {
        return format!(
            "The closest match, {found}, comes from a source that gives no length, and the sources that do are {delta:.0} seconds longer or shorter than this file: it may be a music video, a live or an edited version."
        );
    }
    if top.variant_conflict {
        return format!(
            "The closest match, {found}, is a different version of the song than this file."
        );
    }
    if top.artist_available && top.artist_similarity < 0.72 {
        return format!("The closest match, {found}, is credited to other singers.");
    }
    if top.sources < 2 && !top.audio_recognition {
        if top.candidate.duration_delta.is_none() {
            return format!(
                "Only {source} found {found}, and it gives no length to compare with this file."
            );
        }
        return format!(
            "Only {source} found {found}, and one source is not enough unless title, artist and length all match."
        );
    }
    format!("The closest match, {found}, does not match the file closely enough.")
}

/// “Title” by Artist (Album), for Review messages.
fn describe(candidate: &Candidate) -> String {
    match meaningful_album(candidate.album.as_deref()) {
        Some(album) => format!(
            "“{}” by {} ({album})",
            candidate.title.trim(),
            candidate.artist.trim()
        ),
        None => format!(
            "“{}” by {}",
            candidate.title.trim(),
            candidate.artist.trim()
        ),
    }
}

fn evaluate<'a>(
    index: usize,
    evidence: &TrackEvidence<'_>,
    candidate: &'a Candidate,
    expected_variants: &BTreeSet<&'static str>,
    candidates: &[Candidate],
) -> Evaluated<'a> {
    let title_similarity = evidence
        .title
        .map(|title| {
            title_similarity(title, &candidate.title).max(versions::version_aware_title_similarity(
                title,
                &candidate.title,
            ))
        })
        .unwrap_or(0.5);
    let artist_similarity = evidence
        .artist
        .map(|artist| {
            artist_similarity(artist, &candidate.artist).max(versions::credited_artist_similarity(
                artist,
                evidence.title.unwrap_or_default(),
                &candidate.artist,
                &candidate.title,
            ))
        })
        .unwrap_or(0.5);
    let variants = version_tags(&candidate.title);
    let variant_conflict = variants != *expected_variants;
    let audio_recognition = has_audio_recognition(candidate);
    let isrc_proof = evidence
        .isrc
        .zip(candidate.isrc.as_deref())
        .is_some_and(|(file, found)| {
            crate::domain::audio::normalize_isrc(found).is_some_and(|found| found == file)
        })
        && candidate
            .duration_delta
            .is_some_and(|delta| delta <= ISRC_PROOF_MAX_DURATION_GAP_SECONDS);
    let mut sources = source_count(candidate);
    let agreeing_providers = candidates
        .iter()
        .filter(|other| {
            other.id != candidate.id
                && other.provider != candidate.provider
                && candidates_agree(candidate, other)
        })
        .map(|other| other.provider.as_str())
        .collect::<HashSet<_>>()
        .len();
    sources = sources.max(1 + agreeing_providers);

    let duration_score = match candidate.duration_delta {
        Some(delta) if delta <= 3.0 => 16.0,
        Some(delta) if delta <= 8.0 => 13.0,
        Some(delta) if delta <= 15.0 => 7.0,
        Some(delta) if delta <= 30.0 => 1.0,
        Some(_) => -14.0,
        None if audio_recognition => 6.0,
        None => 0.0,
    };
    let duration_close = candidate.duration_delta.is_some_and(|delta| delta <= 8.0);
    // A source without lengths (Genius, some Radio Javan pages) cannot be
    // checked against the file by itself. The sources that list the same
    // song with a length can: when even the closest of them is far from the
    // file, this is another cut of the song or a wrongly tagged file, however
    // many sources agree on the name.
    let length_contradicted = if candidate.duration_delta.is_none() && !audio_recognition {
        candidates
            .iter()
            // By place in the list, not by id: during a scan no match has an id yet.
            .filter(|other| !std::ptr::eq(*other, candidate) && candidates_agree(candidate, other))
            .filter_map(|other| other.duration_delta)
            .min_by(f64::total_cmp)
            .filter(|closest| *closest > MAX_AUTOMATIC_DURATION_GAP_SECONDS)
    } else {
        None
    };
    let existing_album = meaningful_album(evidence.album);
    let album_similarity = existing_album
        .zip(candidate.album.as_deref())
        .map(|(existing, found)| text_similarity(existing, found));
    let album_score = if let Some(similarity) = album_similarity {
        if similarity < 0.40 {
            -8.0
        } else {
            similarity * 18.0
        }
    } else if is_album_release(candidate) {
        6.0
    } else if candidate.is_compilation {
        -7.0
    } else {
        0.0
    };
    let metadata_score = if meaningful_album(candidate.album.as_deref()).is_some() {
        3.0
    } else {
        -4.0
    } + if candidate.cover_url.is_some() {
        3.0
    } else {
        -2.0
    } + if candidate.release_date.is_some() || candidate.year.is_some() {
        2.0
    } else {
        0.0
    } + if candidate.track_number.is_some() {
        1.5
    } else {
        0.0
    } + if candidate.genre.is_some() { 1.5 } else { 0.0 }
        + if candidate.album_artist.is_some() {
            1.0
        } else {
            0.0
        }
        + if candidate.isrc.is_some() { 1.0 } else { 0.0 };
    let original_album = existing_album.is_none()
        && is_album_release(candidate)
        && original_album_year(candidate, candidates)
            .is_some_and(|original| candidate_year(candidate).is_some_and(|year| year == original));
    let unexpected_variants = variants.difference(expected_variants).count() as f64;
    let missing_variants = expected_variants.difference(&variants).count() as f64;
    let variant_score = -(unexpected_variants * 22.0) - (missing_variants * 16.0);
    let source_score = (sources.saturating_sub(1).min(3) as f64) * 5.0;
    let title_score = if evidence.title.is_some() {
        title_similarity * 28.0
    } else {
        21.0
    };
    let artist_score = if evidence.artist.is_some() {
        artist_similarity * 18.0
    } else {
        15.0
    };
    let total = title_score
        + artist_score
        + duration_score
        + album_score
        + metadata_score
        + source_score
        + provider_trust(&candidate.provider)
        + (candidate.score.clamp(0.0, 100.0) * 0.08)
        + if audio_recognition { 15.0 } else { 0.0 }
        + if isrc_proof { 15.0 } else { 0.0 }
        + if original_album { 4.0 } else { 0.0 }
        + variant_score
        + if candidate.is_compilation { -7.0 } else { 0.0 };

    Evaluated {
        index,
        candidate,
        total,
        title_similarity,
        artist_similarity,
        title_available: evidence.title.is_some(),
        artist_available: evidence.artist.is_some(),
        sources,
        audio_recognition,
        isrc_proof,
        duration_close,
        length_contradicted,
        album_similarity,
        variants,
        variant_conflict,
        original_album,
    }
}

/// A candidate whose length differs more than this from the file is never
/// chosen automatically. Catalogs searched with the same tags agreeing with
/// each other says nothing about the audio itself (a wrongly tagged file).
const MAX_AUTOMATIC_DURATION_GAP_SECONDS: f64 = 15.0;

/// An ISRC names a recording, but a download can still be another cut of it
/// (a music video, a radio edit). The same code only proves the song when the
/// lengths agree too (BLUEPRINT rule: proof with lengths within 3 s).
const ISRC_PROOF_MAX_DURATION_GAP_SECONDS: f64 = 3.0;

fn is_supported(candidate: &Evaluated<'_>) -> bool {
    if candidate
        .candidate
        .duration_delta
        .is_some_and(|delta| delta > MAX_AUTOMATIC_DURATION_GAP_SECONDS)
        || candidate.length_contradicted.is_some()
    {
        return false;
    }
    let exact_text = candidate.title_available
        && candidate.artist_available
        && candidate.title_similarity >= 0.90
        && candidate.artist_similarity >= 0.82;
    // Upload sites (Audiomack, SoundCloud, YouTube) show whatever the uploader
    // typed, often the same wrong tags as the file: alone they prove nothing.
    let credible_catalog = matches!(
        candidate.candidate.provider.as_str(),
        "itunes"
            | "deezer"
            | "musicbrainz"
            | "spotify"
            | "radiojavan"
            | "navahang"
            | "shazam"
            | "genius"
    );
    let corroborated = candidate.sources >= 2
        && (!candidate.title_available || candidate.title_similarity >= 0.82)
        && (!candidate.artist_available || candidate.artist_similarity >= 0.72);
    let recognized = candidate.audio_recognition
        && (!candidate.title_available || candidate.title_similarity >= 0.72)
        && (!candidate.artist_available || candidate.artist_similarity >= 0.65);
    let catalog_exact = credible_catalog && exact_text && candidate.duration_close;
    let weak_raw_score = candidate.candidate.score < 55.0;
    let proven = candidate.audio_recognition || candidate.isrc_proof;

    (recognized || candidate.isrc_proof || corroborated || catalog_exact)
        && !(weak_raw_score && !proven && candidate.sources < 2)
        && !candidate.variant_conflict
}

fn same_recording(left: &Evaluated<'_>, right: &Evaluated<'_>) -> bool {
    if left
        .candidate
        .isrc
        .as_deref()
        .zip(right.candidate.isrc.as_deref())
        .is_some_and(|(left, right)| left.eq_ignore_ascii_case(right))
    {
        return true;
    }
    let matches_file = |candidate: &Candidate| {
        candidate
            .duration_delta
            .is_some_and(|delta| delta <= SAME_SONG_MAX_DURATION_GAP_SECONDS)
    };
    let same_title = title_similarity(&left.candidate.title, &right.candidate.title) >= 0.90;
    // Sources spell Persian titles in Latin letters their own way ("Gheseh Eshgh" on Apple
    // Music, "Ghesseyeh Eshgh" on MusicBrainz). The same words in another spelling are the
    // same song when both lengths match the file too.
    let same_spelling = !same_title
        && crate::domain::names::same_loose_title(&left.candidate.title, &right.candidate.title)
        && matches_file(left.candidate)
        && matches_file(right.candidate);
    if left.variants != right.variants || !(same_title || same_spelling) {
        return false;
    }
    if artist_similarity(&left.candidate.artist, &right.candidate.artist).max(
        versions::artists_similarity(&left.candidate.artist, &right.candidate.artist),
    ) >= 0.82
    {
        return true;
    }
    // One store credits only some of the singers ("54 Ultra" for "ANOTR feat.
    // 54 Ultra"). With the same title and both lengths matching the file, it is
    // the same song listed with a shorter credit, not a rival recording.
    matches_file(left.candidate)
        && matches_file(right.candidate)
        && versions::credit_within(&left.candidate.artist, &right.candidate.artist)
}

/// Two releases whose lengths are both this close to the file's are the same
/// cut of the song (BLUEPRINT rule: proof with lengths within 3 s).
const SAME_SONG_MAX_DURATION_GAP_SECONDS: f64 = 3.0;

fn candidates_agree(left: &Candidate, right: &Candidate) -> bool {
    let left_variants = version_tags(&left.title);
    let right_variants = version_tags(&right.title);
    left_variants == right_variants
        && title_similarity(&left.title, &right.title) >= 0.90
        && artist_similarity(&left.artist, &right.artist)
            .max(versions::artists_similarity(&left.artist, &right.artist))
            >= 0.82
}

pub(crate) fn has_audio_recognition(candidate: &Candidate) -> bool {
    candidate
        .score_breakdown
        .as_deref()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        .is_some_and(|value| {
            value["audio_recognition"].as_bool() == Some(true)
                || value["acoustid"].as_f64().is_some_and(|score| score > 0.0)
        })
}

pub(crate) fn source_count(candidate: &Candidate) -> usize {
    candidate
        .score_breakdown
        .as_deref()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        .and_then(|value| {
            value["sources"].as_array().map(|sources| {
                sources
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .collect::<HashSet<_>>()
                    .len()
            })
        })
        .unwrap_or(1)
        .max(1)
}

fn is_album_release(candidate: &Candidate) -> bool {
    candidate
        .release_type
        .as_deref()
        .is_some_and(|kind| kind.eq_ignore_ascii_case("album"))
        && !candidate.is_compilation
        && !candidate
            .release_secondary_types
            .as_deref()
            .is_some_and(|types| {
                let types = types.to_lowercase();
                types.contains("compilation") || types.contains("live")
            })
}

fn original_album_year(candidate: &Candidate, candidates: &[Candidate]) -> Option<i32> {
    candidates
        .iter()
        .filter(|other| {
            is_album_release(other) && candidates_agree(candidate, other) && !other.is_compilation
        })
        .filter_map(candidate_year)
        .min()
}

fn candidate_year(candidate: &Candidate) -> Option<i32> {
    candidate
        .release_date
        .as_deref()
        .or(candidate.year.as_deref())
        .and_then(|date| date.get(..4))
        .and_then(|year| year.parse().ok())
}

fn meaningful_album(album: Option<&str>) -> Option<&str> {
    album.filter(|album| !album.trim().is_empty() && !album.trim().starts_with('@'))
}

fn provider_trust(provider: &str) -> f64 {
    match provider {
        "spotify" | "itunes" | "deezer" | "musicbrainz" | "radiojavan" | "navahang" | "shazam"
        | "genius" => 5.0,
        "acoustid" | "songrec" => 4.0,
        "discogs" | "soundcloud" | "audiomack" => 2.0,
        "lastfm" | "youtube" => 1.0,
        _ => 0.0,
    }
}

fn title_similarity(left: &str, right: &str) -> f64 {
    let left_normalized = normalized_text(left);
    let right_normalized = normalized_text(right);
    let direct = strsim::normalized_levenshtein(&left_normalized, &right_normalized);
    let left_key = left_normalized.replace(' ', "");
    let right_key = right_normalized.replace(' ', "");
    let meaningful = |value: &str| {
        value
            .split_whitespace()
            .filter(|word| !matches!(*word, "a" | "an" | "the" | "such"))
            .collect::<Vec<_>>()
            .join(" ")
    };
    if (!meaningful(&left_normalized).is_empty()
        && meaningful(&left_normalized) == meaningful(&right_normalized))
        || (left_key.len().min(right_key.len()) >= 4
            && (left_key.starts_with(&right_key) || right_key.starts_with(&left_key)))
    {
        direct.max(0.94)
    } else {
        direct
    }
}

fn artist_similarity(left: &str, right: &str) -> f64 {
    let direct = text_similarity(left, right);
    let left_key = normalized_text(left).replace(' ', "");
    let right_key = normalized_text(right).replace(' ', "");
    if left_key.len().min(right_key.len()) >= 5
        && (left_key.starts_with(&right_key) || right_key.starts_with(&left_key))
    {
        direct.max(0.92)
    } else {
        direct
    }
}

fn text_similarity(left: &str, right: &str) -> f64 {
    strsim::normalized_levenshtein(&normalized_text(left), &normalized_text(right))
}

fn normalized_text(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            '÷' => " divide ".to_owned(),
            '×' => " multiply ".to_owned(),
            '+' => " plus ".to_owned(),
            '=' => " equals ".to_owned(),
            '&' => " and ".to_owned(),
            character if character.is_alphanumeric() => {
                character.to_lowercase().collect::<String>()
            }
            _ => " ".to_owned(),
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(id: i64, provider: &str, title: &str, album: &str, score: f64) -> Candidate {
        Candidate {
            id: Some(id),
            provider: provider.into(),
            title: title.into(),
            artist: "Ed Sheeran".into(),
            album: Some(album.into()),
            duration_delta: Some(1.0),
            score,
            score_breakdown: Some(serde_json::json!({"sources":[provider]}).to_string()),
            ..Default::default()
        }
    }

    fn performed(
        id: i64,
        provider: &str,
        title: &str,
        artist: &str,
        album: &str,
        delta: f64,
    ) -> Candidate {
        Candidate {
            artist: artist.into(),
            duration_delta: Some(delta),
            cover_url: Some("https://example.test/cover.jpg".into()),
            year: Some("2020".into()),
            ..candidate(id, provider, title, album, 85.0)
        }
    }

    #[test]
    fn a_remix_is_found_from_the_names_in_its_title() {
        // The file says Linkin Park; the remix is credited to the remixers.
        let evidence = TrackEvidence {
            filename: "u9.mp3",
            title: Some("In The End  (Mellen Gi & Tommee Profitt Remix)"),
            artist: Some("Linkin Park"),
            album: None,
            isrc: None,
        };
        let candidates = vec![
            // Upload-site copies with the file's own wrong credit.
            performed(
                1,
                "audiomack",
                "In The End (Mellen Gi & Tommee Profitt Remix)",
                "Linkin Park",
                "",
                0.0,
            ),
            performed(
                2,
                "audiomack",
                "In The End (Mellen Gi Tommee Profitt Remix)",
                "Linkin Park",
                "",
                3.0,
            ),
            // The official recordings.
            performed(
                3,
                "itunes",
                "In the End (Mellen Gi Remix)",
                "Tommee Profitt, Mellen Gi & Fleurie",
                "In the End - Single",
                0.0,
            ),
            // The same recording again, with the artists in another order.
            performed(
                6,
                "itunes",
                "In The End (Mellen Gi Remix)",
                "Tommee Profitt, Fleurie & Mellen Gi",
                "The Remixes, Vol. 1 - EP",
                0.0,
            ),
            performed(
                4,
                "itunes",
                "In The End (Mellen Gi Remix/Slowed Down)",
                "Tommee Profitt, Fleurie & Mellen Gi",
                "In The End (Anniversary Edition) - EP",
                38.0,
            ),
            performed(
                5,
                "itunes",
                "In the End",
                "LINKIN PARK",
                "Hybrid Theory",
                2.0,
            ),
        ];
        let decision = rank(evidence, &candidates).expect("the official remix is chosen");
        assert!(matches!(
            candidates[decision.candidate_index].id,
            Some(3 | 6)
        ));
    }

    #[test]
    fn an_upload_site_alone_never_decides() {
        let evidence = TrackEvidence {
            filename: "song.mp3",
            title: Some("Some Song"),
            artist: Some("Some Artist"),
            album: None,
            isrc: None,
        };
        let candidates = vec![performed(
            1,
            "audiomack",
            "Some Song",
            "Some Artist",
            "",
            0.0,
        )];
        assert!(rank(evidence, &candidates).is_none());
    }

    #[test]
    fn a_cover_is_matched_to_its_own_performer() {
        let evidence = TrackEvidence {
            filename: "The Macarons Project - Fly me to the moon (u6).mp3",
            title: Some("Fly me to the moon"),
            artist: Some("The Macarons Project"),
            album: None,
            isrc: None,
        };
        let candidates = vec![
            performed(
                1,
                "itunes",
                "Fly Me to the Moon (feat. Count Basie and His Orchestra)",
                "Frank Sinatra",
                "Ultimate Sinatra",
                7.0,
            ),
            performed(
                2,
                "itunes",
                "Fly Me to the Moon",
                "The Macarons Project",
                "Fly Me to the Moon - Single",
                0.0,
            ),
            performed(
                3,
                "deezer",
                "Fly Me To The Moon",
                "Bobby Womack",
                "Fly Me To The Moon",
                27.0,
            ),
        ];
        let decision = rank(evidence, &candidates).expect("the cover's own release is chosen");
        assert_eq!(candidates[decision.candidate_index].id, Some(2));
    }

    #[test]
    fn credits_in_titles_are_read() {
        assert_eq!(
            versions::title_credits("In The End (Mellen Gi & Tommee Profitt Remix)"),
            ["Mellen Gi", "Tommee Profitt"]
        );
        assert_eq!(
            versions::title_credits("Song (feat. Fleurie) [Extended Mix]"),
            ["Fleurie"]
        );
        assert!(versions::title_credits("Song (Radio Edit)").is_empty());
        assert_eq!(
            versions::base_title("In the End (Mellen Gi Remix)"),
            "In the End"
        );
        assert!(
            versions::version_aware_title_similarity(
                "In The End (Mellen Gi & Tommee Profitt Remix)",
                "In the End (Mellen Gi Remix)"
            ) >= 0.95
        );
        // Another remixer is another version.
        assert!(
            versions::version_aware_title_similarity(
                "In The End (Mellen Gi Remix)",
                "In the End (Steve Aoki Remix)"
            ) < 0.95
        );
    }

    #[test]
    fn a_source_that_leaves_out_the_main_singer_is_not_filed_on_its_own() {
        // The file credits Amir Tataloo with two featured singers; one catalog lists only the
        // featured singers. Sharing their names is not proof it is the right credit.
        let evidence = || TrackEvidence {
            filename: "male-man-bash.mp3",
            title: Some("Male Man Bash"),
            artist: Some("Amir Tataloo feat. Sohrab MJ & Orchid"),
            album: None,
            isrc: None,
        };
        let only_featured = vec![performed(
            1,
            "itunes",
            "Male Man Bash",
            "Sohrab MJ & Orchid",
            "Male Man Bash - Single",
            1.0,
        )];
        assert!(select(evidence(), &only_featured).is_none());

        // A catalog that names everyone is still filed automatically.
        let everyone = vec![performed(
            1,
            "itunes",
            "Male Man Bash",
            "Amir Tataloo, Sohrab MJ & Orchid",
            "Male Man Bash - Single",
            1.0,
        )];
        assert!(select(evidence(), &everyone).is_some());
    }

    #[test]
    fn shazam_spelling_the_singers_its_own_way_is_the_same_recording() {
        // Shazam lists a featured singer first and spells the names shorter; Apple Music
        // agrees with the file. Both are the same recording with the same singers.
        let evidence = || TrackEvidence {
            filename: "male-man-bash.mp3",
            title: Some("Male Man Bash"),
            artist: Some("Amir Tataloo feat. Sohrab MJ & Orchid"),
            album: None,
            isrc: None,
        };
        let file = crate::application::FileTags {
            artist: Some("Amir Tataloo feat. Sohrab MJ & Orchid"),
            title: Some("Male Man Bash"),
        };
        let mut shazam = performed(
            1,
            "songrec",
            "Male Man Bash",
            "MJ feat. Tataloo & Orchid",
            "Male Man Bash",
            1.0,
        );
        shazam.score = 96.0;
        shazam.score_breakdown = Some(
            serde_json::json!({"audio_recognition": true, "sources": ["SongRec / Shazam"]})
                .to_string(),
        );
        let apple = performed(
            2,
            "itunes",
            "Male Man Bash",
            "Amir Tataloo feat. Sohrab MJ & Orchid",
            "Male Man Bash (feat. Orchid & Sohrab MJ) - EP",
            1.0,
        );
        let mut candidates = vec![shazam, apple];
        for candidate in &mut candidates {
            crate::application::align_with_file_credit(candidate, file);
        }
        assert_eq!(
            candidates[0].artist,
            "Amir Tataloo feat. Sohrab MJ & Orchid"
        );
        let reason = unsure_reason(evidence(), &candidates);
        assert!(!reason.starts_with("Two different recordings"), "{reason}");
        let decision = select(evidence(), &candidates).expect("audio and a catalog agree");
        let chosen = candidates
            .iter()
            .find(|candidate| candidate.id == Some(decision.candidate_id))
            .unwrap();
        assert!(chosen.artist.starts_with("Amir Tataloo"));
    }

    #[test]
    fn agreeing_catalogs_cannot_outvote_the_audio_length() {
        // A two-minute file tagged as a famous song: every catalog finds that
        // song by its tags, but the song is 80 seconds longer than the file.
        let evidence = TrackEvidence {
            filename: "test-tone.m4a",
            title: Some("Perfect"),
            artist: Some("Ed Sheeran"),
            album: None,
            isrc: None,
        };
        let mut wrong = Vec::new();
        for (id, provider) in [(1, "itunes"), (2, "deezer"), (3, "musicbrainz")] {
            let mut song = candidate(id, provider, "Perfect", "÷", 95.0);
            song.duration_delta = Some(80.0);
            song.score_breakdown = Some(
                serde_json::json!({"sources": ["itunes", "deezer", "musicbrainz"]}).to_string(),
            );
            wrong.push(song);
        }
        assert!(select(evidence, &wrong).is_none());

        // The same agreement with a matching length is still filed automatically.
        for song in &mut wrong {
            song.duration_delta = Some(1.0);
        }
        let evidence = TrackEvidence {
            filename: "test-tone.m4a",
            title: Some("Perfect"),
            artist: Some("Ed Sheeran"),
            album: None,
            isrc: None,
        };
        assert!(select(evidence, &wrong).is_some());
    }

    #[test]
    fn lower_score_plain_recording_beats_high_score_duet() {
        let mut duet = candidate(
            1,
            "songrec",
            "Perfect Duet (Ed Sheeran & Beyoncé)",
            "Perfect Duet",
            99.0,
        );
        duet.duration_delta = None;
        duet.score_breakdown =
            Some(serde_json::json!({"audio_recognition":true,"sources":["SongRec"]}).to_string());
        let plain = candidate(2, "deezer", "Perfect", "÷ (Deluxe)", 94.6);

        let decision = select(
            TrackEvidence {
                filename: "Ed-Sheeran-Perfect-128.mp3",
                title: Some("Perfect"),
                artist: Some("Ed Sheeran"),
                album: Some("Divide (Deluxe)"),
                isrc: None,
            },
            &[duet, plain],
        )
        .unwrap();

        assert_eq!(decision.candidate_id, 2);
    }

    #[test]
    fn normal_album_recording_beats_higher_score_stripped_version() {
        let mut stripped = candidate(
            1,
            "deezer",
            "Kippe (stripped)",
            "stundenull (stripped)",
            98.0,
        );
        stripped.artist = "ENNIO".into();
        let mut album = candidate(2, "musicbrainz", "Kippe", "Nirvana", 88.0);
        album.artist = "ENNIO".into();
        album.release_type = Some("Album".into());
        album.score_breakdown =
            Some(serde_json::json!({"sources":["MusicBrainz","Deezer"]}).to_string());

        let decision = select(
            TrackEvidence {
                filename: "ENNIO - Kippe.mp3",
                title: Some("Kippe"),
                artist: Some("ENNIO"),
                album: None,
                isrc: None,
            },
            &[stripped, album],
        )
        .unwrap();

        assert_eq!(decision.candidate_id, 2);
        assert!(decision.explanation.contains("album release"));
    }

    #[test]
    fn existing_album_context_beats_a_higher_raw_score() {
        let wrong_album = candidate(1, "itunes", "Perfect", "Perfect - Single", 98.0);
        let right_album = candidate(2, "deezer", "Perfect", "÷ (Deluxe)", 90.0);

        let decision = select(
            TrackEvidence {
                filename: "Ed Sheeran - Perfect.mp3",
                title: Some("Perfect"),
                artist: Some("Ed Sheeran"),
                album: Some("Divide (Deluxe)"),
                isrc: None,
            },
            &[wrong_album, right_album],
        )
        .unwrap();

        assert_eq!(decision.candidate_id, 2);
        assert!(decision.explanation.contains("existing album"));
    }

    #[test]
    fn a_well_described_single_beats_an_incomplete_equal_score_row() {
        let mut apple = candidate(
            1,
            "itunes",
            "Khoroosh e Zendegi",
            "Khoroosh e Zendegi - Single",
            98.0,
        );
        apple.artist = "Ebi".into();
        apple.year = Some("2023".into());
        let mut incomplete = candidate(
            2,
            "deezer",
            "Khoroosh e Zendegi",
            "Khoroosh e Zendegi",
            98.0,
        );
        incomplete.artist = "Ebi".into();

        let decision = select(
            TrackEvidence {
                filename: "Ebi - Khoroosh e Zendegi.mp3",
                title: Some("Khoroosh e Zendegi"),
                artist: Some("Ebi"),
                album: None,
                isrc: None,
            },
            &[apple, incomplete],
        )
        .unwrap();

        assert_eq!(decision.candidate_id, 1);
    }

    #[test]
    fn complete_catalog_row_beats_a_slightly_higher_incomplete_row() {
        let incomplete = candidate(1, "deezer", "Perfect", "÷ (Deluxe)", 98.0);
        let mut complete = candidate(2, "itunes", "Perfect", "÷ (Deluxe)", 95.0);
        complete.album_artist = Some("Ed Sheeran".into());
        complete.year = Some("2017".into());
        complete.genre = Some("Pop".into());
        complete.track_number = Some(5);
        complete.cover_url = Some("https://example.test/divide.jpg".into());
        complete.isrc = Some("GBAHS1700024".into());

        let decision = select(
            TrackEvidence {
                filename: "Ed Sheeran - Perfect.mp3",
                title: Some("Perfect"),
                artist: Some("Ed Sheeran"),
                album: Some("Divide (Deluxe)"),
                isrc: None,
            },
            &[incomplete, complete],
        )
        .unwrap();

        assert_eq!(decision.candidate_id, 2);
    }

    #[test]
    fn many_sources_agreeing_on_the_name_do_not_outweigh_a_wrong_length() {
        // A 45-second file tagged "Moein - Kabeh". Genius lists the song without a
        // length; Apple Music and Deezer list it too, minutes longer than the file.
        let evidence = || TrackEvidence {
            filename: "Moein - Kabeh.mp3",
            title: Some("Kabeh"),
            artist: Some("Moein"),
            album: None,
            isrc: None,
        };
        let song = |id: i64, provider: &str, delta: Option<f64>| {
            let mut found = candidate(id, provider, "Kabeh", "Kabeh", 98.0);
            found.artist = "Moein".into();
            found.duration_delta = delta;
            found.score_breakdown = Some(
                serde_json::json!({"sources": ["Apple Music", "Deezer", "Genius", "Radio Javan"]})
                    .to_string(),
            );
            found
        };
        let wrong_length = [
            song(1, "genius", None),
            song(2, "itunes", Some(255.0)),
            song(3, "deezer", Some(254.0)),
        ];
        assert!(select(evidence(), &wrong_length).is_none());
        // During a scan the matches are not saved yet and have no id.
        let mut unsaved = wrong_length.clone();
        for found in &mut unsaved {
            found.id = None;
        }
        assert!(rank(evidence(), &unsaved).is_none());
        let reason = unsure_reason(evidence(), &wrong_length);
        assert!(
            reason.contains("gives no length") && reason.contains("254 seconds"),
            "{reason}"
        );

        // The same song at the right length is still chosen on its own, and a
        // source without a length is still fine next to one that fits.
        let right_length = [
            song(1, "genius", None),
            song(2, "itunes", Some(2.0)),
            song(3, "deezer", Some(254.0)),
        ];
        assert!(select(evidence(), &right_length).is_some());
        // Audio recognition is proof about the sound itself and needs no length.
        let mut recognized = song(1, "songrec", None);
        recognized.score_breakdown = Some(
            serde_json::json!({"audio_recognition": true, "sources": ["SongRec"]}).to_string(),
        );
        assert!(select(evidence(), &[recognized, song(2, "itunes", Some(255.0))]).is_some());
    }

    #[test]
    fn low_score_uncorroborated_guess_is_not_approved() {
        let mut weak = candidate(1, "lastfm", "Different song", "", 47.0);
        weak.duration_delta = None;

        assert!(
            select(
                TrackEvidence {
                    filename: "Unknown.mp3",
                    title: Some("Unknown"),
                    artist: Some("Unknown Artist"),
                    album: None,
                    isrc: None,
                },
                &[weak],
            )
            .is_none()
        );
    }

    #[test]
    fn audio_recognition_can_identify_a_track_without_existing_tags() {
        let mut recognized = candidate(1, "songrec", "Teardrop", "Mezzanine", 96.0);
        recognized.artist = "Massive Attack".into();
        recognized.duration_delta = None;
        recognized.score_breakdown =
            Some(serde_json::json!({"audio_recognition":true,"sources":["SongRec"]}).to_string());

        let decision = select(
            TrackEvidence {
                filename: "unknown-001.mp3",
                title: None,
                artist: None,
                album: None,
                isrc: None,
            },
            &[recognized],
        )
        .unwrap();

        assert_eq!(decision.candidate_id, 1);
        assert!(decision.explanation.contains("audio recognition"));
    }

    #[test]
    fn close_conflicting_recordings_remain_for_review() {
        let mut first = candidate(1, "deezer", "Halo", "Halo", 92.0);
        first.artist = "Beyoncé".into();
        let mut second = candidate(2, "itunes", "Halo", "Halo", 91.0);
        second.artist = "LP".into();

        assert!(
            select(
                TrackEvidence {
                    filename: "Halo.mp3",
                    title: Some("Halo"),
                    artist: None,
                    album: None,
                    isrc: None,
                },
                &[first, second],
            )
            .is_none()
        );
    }

    #[test]
    fn ignores_filename_filler_words_when_ranking_title() {
        assert!(title_similarity("Such A Lonely Day", "Lonely Day") >= 0.94);
    }

    #[test]
    fn a_store_that_lists_fewer_singers_is_not_a_rival_recording() {
        // Real answers for "Talk To You" (Nov 2026): Apple Music credits only 54 Ultra,
        // Deezer and MusicBrainz credit ANOTR feat. 54 Ultra. Same song, same length.
        let evidence = || TrackEvidence {
            filename: "ANOTR feat. 54 Ultra - Talk To You.mp3",
            title: Some("Talk To You"),
            artist: Some("ANOTR feat. 54 Ultra"),
            album: Some("Talk To You (feat. 54 Ultra) - Single"),
            isrc: None,
        };
        let mut apple = performed(
            1,
            "itunes",
            "Talk To You",
            "54 Ultra",
            "Talk To You (feat. 54 Ultra) - Single",
            2.1,
        );
        apple.score_breakdown =
            Some(serde_json::json!({"sources":["Apple Music","Audiomack"]}).to_string());
        apple.score = 90.0;
        let deezer = performed(
            2,
            "deezer",
            "Talk To You",
            "ANOTR feat. 54 Ultra",
            "Talk To You (ft. 54 Ultra)",
            2.0,
        );
        let candidates = vec![apple.clone(), deezer.clone()];
        assert!(
            rank(evidence(), &candidates).is_some(),
            "one song, two listings"
        );

        // With a length that does not match the file, a shorter credit stays a rival.
        let mut far = deezer;
        far.duration_delta = Some(7.0);
        assert!(!same_recording(
            &evaluate_all(&evidence(), &[apple.clone()])[0],
            &evaluate_all(&evidence(), &[far])[0],
        ));
    }

    #[test]
    fn the_margin_is_kept_against_every_different_recording() {
        // The runner-up is another release of the best match, but a different recording
        // follows close behind: that one still has to be clearly beaten.
        let evidence = TrackEvidence {
            filename: "Halo.mp3",
            title: Some("Halo"),
            artist: None,
            album: None,
            isrc: None,
        };
        let mut first = candidate(1, "deezer", "Halo", "I Am... Sasha Fierce", 92.0);
        first.artist = "Beyoncé".into();
        let mut again = candidate(2, "itunes", "Halo", "Halo - Single", 91.5);
        again.artist = "Beyoncé".into();
        let mut other = candidate(3, "musicbrainz", "Halo", "Halo", 91.0);
        other.artist = "LP".into();
        other.score_breakdown =
            Some(serde_json::json!({"sources":["MusicBrainz","Spotify"]}).to_string());
        assert!(rank(evidence, &[first, again, other]).is_none());
    }

    #[test]
    fn the_files_own_isrc_with_the_same_length_proves_the_song() {
        let evidence = |isrc| TrackEvidence {
            filename: "u7.mp3",
            title: Some("Gheseh Eshgh"),
            artist: Some("Ebi"),
            album: None,
            isrc,
        };
        // One source, a spelling the text rules call only "close", no second source.
        let mut found = performed(1, "deezer", "Ghesseyeh Eshgh", "Ebi", "Koohe Yakh", 1.0);
        found.isrc = Some("IR-AB1-08-00001".into());
        assert!(rank(evidence(None), std::slice::from_ref(&found)).is_none());
        let decision = rank(evidence(Some("IRAB10800001")), std::slice::from_ref(&found))
            .expect("the code and the length agree");
        assert!(decision.explanation.contains("same ISRC"));

        // The same code on a cut 10 s longer (a music video) is not proof.
        found.duration_delta = Some(10.0);
        assert!(rank(evidence(Some("IRAB10800001")), &[found]).is_none());
    }

    #[test]
    fn a_session_recording_is_another_version() {
        let evidence = || TrackEvidence {
            filename: "Marie.mp3",
            title: Some("Marie"),
            artist: Some("AnnenMayKantereit"),
            album: None,
            isrc: None,
        };
        let session = performed(
            1,
            "musicbrainz",
            "Marie (Dach Session)",
            "AnnenMayKantereit",
            "Marie",
            1.0,
        );
        assert!(rank(evidence(), &[session]).is_none());
    }

    #[test]
    fn another_latin_spelling_of_a_persian_title_is_not_a_rival() {
        // Apple Music writes "Gheseh Eshgh", MusicBrainz "Ghesseyeh Eshgh": the same song on
        // the same album with the same length, so the song is filed without asking.
        let evidence = || TrackEvidence {
            filename: "Ebi - Gheseh Eshgh.mp3",
            title: Some("Gheseh Eshgh"),
            artist: Some("Ebi"),
            album: Some("Koohe Yakh"),
            isrc: None,
        };
        let apple = performed(1, "itunes", "Gheseh Eshgh", "Ebi", "Koohe Yakh", 0.3);
        // MusicBrainz comes from the audio fingerprint, so it is supported on its own.
        let mut musicbrainz = performed(
            2,
            "musicbrainz",
            "Ghesseyeh Eshgh",
            "Ebi",
            "Koohe Yakh",
            0.3,
        );
        musicbrainz.score_breakdown =
            Some(serde_json::json!({"acoustid": 0.95, "sources": ["musicbrainz"]}).to_string());
        assert!(
            rank(evidence(), &[apple.clone(), musicbrainz.clone()]).is_some(),
            "one song in two spellings"
        );

        // With a length that does not match the file, the other spelling is another song.
        let mut far = musicbrainz;
        far.duration_delta = Some(9.0);
        assert!(!same_recording(
            &evaluate_all(&evidence(), &[apple])[0],
            &evaluate_all(&evidence(), &[far])[0],
        ));
    }

    #[test]
    fn review_says_what_to_check_in_plain_words() {
        // Genius gives no length: say so instead of "length differs".
        let evidence = || TrackEvidence {
            filename: "No Longer Slaves.mp3",
            title: Some("No Longer Slaves"),
            artist: Some("Bright Ones (Ft. Josie Buchanan & Peyton Allen)"),
            album: Some("Bright Ones"),
            isrc: None,
        };
        let mut genius = performed(
            1,
            "genius",
            "No Longer Slaves",
            "Bright Ones feat. Peyton Allen feat. Josie Buchanan",
            "Bright Ones",
            0.0,
        );
        genius.duration_delta = None;
        let reason = unsure_reason(evidence(), &[genius]);
        assert!(
            reason.starts_with("Only Genius found “No Longer Slaves”")
                && reason.contains("gives no length"),
            "{reason}"
        );

        let longer = performed(
            1,
            "itunes",
            "No Longer Slaves",
            "Bright Ones",
            "Bright Ones",
            41.0,
        );
        let reason = unsure_reason(evidence(), &[longer]);
        assert!(reason.contains("41 seconds longer or shorter"), "{reason}");

        let two_sources = Some(serde_json::json!({"sources":["Deezer","Spotify"]}).to_string());
        let mut first = candidate(1, "deezer", "Halo", "Halo", 92.0);
        first.artist = "Beyoncé".into();
        first.score_breakdown = two_sources.clone();
        let mut second = candidate(2, "itunes", "Halo", "Halo", 91.0);
        second.artist = "LP".into();
        second.score_breakdown = two_sources;
        let reason = unsure_reason(
            TrackEvidence {
                filename: "Halo.mp3",
                title: Some("Halo"),
                ..Default::default()
            },
            &[first, second],
        );
        assert!(
            reason.starts_with("Two different recordings fit"),
            "{reason}"
        );
    }
}
