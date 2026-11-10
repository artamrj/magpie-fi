use crate::domain::versions;
pub(crate) use crate::domain::versions::version_tags;
use crate::infrastructure::providers::Candidate;
use std::collections::{BTreeSet, HashSet};

pub struct TrackEvidence<'a> {
    pub filename: &'a str,
    pub title: Option<&'a str>,
    pub artist: Option<&'a str>,
    pub album: Option<&'a str>,
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
    duration_close: bool,
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

pub fn rank(evidence: TrackEvidence<'_>, candidates: &[Candidate]) -> Option<RankedDecision> {
    let source_text = [evidence.filename, evidence.title.unwrap_or_default()].join(" ");
    let expected_variants = version_tags(&source_text);
    let mut evaluated = candidates
        .iter()
        .enumerate()
        .filter(|(_, candidate)| {
            !candidate.title.trim().is_empty() && !candidate.artist.trim().is_empty()
        })
        .map(|(index, candidate)| {
            evaluate(index, &evidence, candidate, &expected_variants, candidates)
        })
        .filter(is_supported)
        .collect::<Vec<_>>();
    evaluated.sort_by(|left, right| right.total.total_cmp(&left.total));

    let best = evaluated.first()?;
    if best.total < 68.0 {
        return None;
    }
    if let Some(second) = evaluated.get(1)
        && !same_recording(best, second)
        && best.total - second.total < 12.0
    {
        return None;
    }

    let mut reasons = Vec::new();
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
        duration_close,
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

fn is_supported(candidate: &Evaluated<'_>) -> bool {
    if candidate
        .candidate
        .duration_delta
        .is_some_and(|delta| delta > MAX_AUTOMATIC_DURATION_GAP_SECONDS)
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

    (recognized || corroborated || catalog_exact)
        && !(weak_raw_score && !candidate.audio_recognition && candidate.sources < 2)
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
    left.variants == right.variants
        && title_similarity(&left.candidate.title, &right.candidate.title) >= 0.90
        && artist_similarity(&left.candidate.artist, &right.candidate.artist).max(
            versions::artists_similarity(&left.candidate.artist, &right.candidate.artist),
        ) >= 0.82
}

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
    fn agreeing_catalogs_cannot_outvote_the_audio_length() {
        // A two-minute file tagged as a famous song: every catalog finds that
        // song by its tags, but the song is 80 seconds longer than the file.
        let evidence = TrackEvidence {
            filename: "test-tone.m4a",
            title: Some("Perfect"),
            artist: Some("Ed Sheeran"),
            album: None,
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
            },
            &[incomplete, complete],
        )
        .unwrap();

        assert_eq!(decision.candidate_id, 2);
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
}
