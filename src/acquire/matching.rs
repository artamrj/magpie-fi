//! Picks the YouTube (Music) upload that is the song someone asked for.
//! Title 45 %, artist 35 %, length 20 %; live, cover, remix, sped-up and
//! similar versions are penalised unless the request asks for them.

use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

/// Words that mark a different version of a song.
const VARIANT_WORDS: &[&str] = &[
    "live",
    "cover",
    "karaoke",
    "instrumental",
    "remix",
    "sped up",
    "slowed",
    "nightcore",
    "reverb",
    "8d",
    "acapella",
    "a cappella",
    "mashup",
    "tribute",
    "acoustic",
    "unplugged",
    "demo",
    "extended",
];

/// Bracketed notes that do not change which recording it is.
const NEUTRAL_NOTE_WORDS: &[&str] = &[
    "feat",
    "ft",
    "featuring",
    "with",
    "remaster",
    "remastered",
    "radio edit",
    "explicit",
    "clean",
    "original",
    "single",
    "album version",
    "official",
    "audio",
    "from",
];

#[derive(Clone, Debug, Default)]
pub struct Target {
    pub title: String,
    pub artist: Option<String>,
    pub duration: Option<f64>,
}

#[derive(Clone, Debug, Default)]
pub struct Candidate {
    pub id: String,
    pub title: String,
    pub artists: Vec<String>,
    pub duration: Option<f64>,
    /// An official audio upload (a YouTube Music "song").
    pub official: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct Thresholds {
    pub min_total: f64,
    pub min_title: f64,
    pub min_artist: f64,
    pub max_duration_diff: f64,
}

/// For official YouTube Music songs.
pub const NORMAL: Thresholds = Thresholds {
    min_total: 72.0,
    min_title: 70.0,
    min_artist: 65.0,
    max_duration_diff: 15.0,
};

/// For ordinary videos, which are more often the wrong version.
pub const STRICT: Thresholds = Thresholds {
    min_total: 82.0,
    min_title: 80.0,
    min_artist: 75.0,
    max_duration_diff: 7.0,
};

#[derive(Clone, Copy, Debug)]
pub struct Score {
    pub total: f64,
    pub accepted: bool,
}

pub fn score(target: &Target, candidate: &Candidate, thresholds: Thresholds) -> Score {
    let title = title_score(&target.title, &candidate.title);
    let artist = target
        .artist
        .as_deref()
        .map(|artist| artist_score(artist, &candidate.artists));
    let duration = match (target.duration, candidate.duration) {
        (Some(wanted), Some(found)) if wanted > 0.0 && found > 0.0 => {
            Some(100.0 * (-0.1 * (wanted - found).abs()).exp())
        }
        _ => None,
    };

    let mut weighted = vec![(title, 0.45)];
    if let Some(artist) = artist {
        weighted.push((artist, 0.35));
    }
    if let Some(duration) = duration {
        weighted.push((duration, 0.20));
    }
    let weight: f64 = weighted.iter().map(|(_, weight)| weight).sum();
    let mut total = weighted
        .iter()
        .map(|(value, weight)| value * weight)
        .sum::<f64>()
        / weight;
    total -= variant_penalty(&target.title, &candidate.title);
    if candidate.official {
        total += 3.0;
    }
    let total = total.clamp(0.0, 100.0);

    let mut accepted = title >= thresholds.min_title
        && artist.is_none_or(|artist| artist >= thresholds.min_artist)
        && total >= thresholds.min_total;
    if let (Some(wanted), Some(found)) = (target.duration, candidate.duration)
        && (wanted - found).abs() > thresholds.max_duration_diff
    {
        accepted = false;
    }
    Score { total, accepted }
}

/// The best accepted candidate, if any.
pub fn best<'a>(
    target: &Target,
    candidates: &'a [Candidate],
    thresholds: Thresholds,
) -> Option<(&'a Candidate, Score)> {
    candidates
        .iter()
        .map(|candidate| (candidate, score(target, candidate, thresholds)))
        .filter(|(_, score)| score.accepted)
        .max_by(|left, right| left.1.total.total_cmp(&right.1.total))
}

/// Lowercase, no accents, no punctuation, single spaces.
pub fn normalize(value: &str) -> String {
    let folded: String = value
        .nfkd()
        .filter(|character| !is_combining_mark(*character))
        .collect::<String>()
        .to_lowercase()
        .replace('&', " and ");
    folded
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || character == '_' || character.is_whitespace() {
                character
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// 0–100 similarity that tolerates word order and extra words.
pub fn similarity(left: &str, right: &str) -> f64 {
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    if left == right {
        return 100.0;
    }
    let mut ratio = strsim::normalized_levenshtein(left, right) * 100.0;
    let left_words: std::collections::HashSet<&str> = left.split(' ').collect();
    let right_words: std::collections::HashSet<&str> = right.split(' ').collect();
    let (shorter, longer) = if left_words.len() <= right_words.len() {
        (&left_words, &right_words)
    } else {
        (&right_words, &left_words)
    };
    if !shorter.is_empty() && shorter.is_subset(longer) {
        // Every word of one side appears in the other ("Song" vs "Song Remaster").
        let coverage = shorter.len() as f64 / longer.len() as f64;
        ratio = ratio.max(60.0 + 40.0 * coverage);
    }
    ratio
}

fn title_score(target: &str, candidate: &str) -> f64 {
    similarity(&normalize(target), &normalize(candidate))
        .max(similarity(&base_title(target), &base_title(candidate)))
}

fn artist_score(target: &str, candidates: &[String]) -> f64 {
    let names: Vec<String> = candidates
        .iter()
        .map(|name| normalize(name))
        .filter(|name| !name.is_empty())
        .collect();
    if names.is_empty() {
        return 0.0;
    }
    let wanted = normalize(&primary_artist(target));
    if !wanted.is_empty() && names.join(" ").contains(&wanted) {
        return 100.0;
    }
    names
        .iter()
        .map(|name| similarity(&wanted, name))
        .fold(0.0, f64::max)
}

/// Drops bracketed parts and " - …" / " feat. …" suffixes.
pub fn base_title(value: &str) -> String {
    let mut stripped = String::new();
    let mut depth = 0usize;
    for character in value.chars() {
        match character {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = depth.saturating_sub(1),
            _ if depth == 0 => stripped.push(character),
            _ => {}
        }
    }
    if let Some(index) = stripped.find(" - ") {
        stripped.truncate(index);
    }
    let lower = stripped.to_lowercase();
    for marker in [" feat. ", " feat ", " ft. ", " ft ", " featuring "] {
        if let Some(index) = lower.find(marker) {
            stripped.truncate(index);
            break;
        }
    }
    let normalized = normalize(&stripped);
    if normalized.is_empty() {
        normalize(value)
    } else {
        normalized
    }
}

/// The first credited artist of "A, B & C feat. D".
pub fn primary_artist(value: &str) -> String {
    let lower = value.to_lowercase();
    let mut end = value.len();
    for separator in [
        ",",
        ";",
        " & ",
        " feat. ",
        " feat ",
        " ft. ",
        " ft ",
        " featuring ",
        " with ",
        " x ",
    ] {
        if let Some(index) = lower.find(separator) {
            end = end.min(index);
        }
    }
    let first = value[..end].trim();
    if first.is_empty() {
        value.trim().to_owned()
    } else {
        first.to_owned()
    }
}

/// "Artist - Topic" or "ArtistVEVO" → "Artist".
pub fn clean_channel_name(value: &str) -> String {
    let trimmed = value.trim();
    let lower = trimmed.to_lowercase();
    for suffix in ["- topic", " topic", "vevo", " official"] {
        if lower.ends_with(suffix) {
            let cleaned = trimmed[..trimmed.len() - suffix.len()]
                .trim()
                .trim_end_matches('-')
                .trim();
            if !cleaned.is_empty() {
                return cleaned.to_owned();
            }
        }
    }
    trimmed.to_owned()
}

/// A title key for "is this the same song?": featured artists and neutral
/// notes ("Remastered 2011", "feat. X") are dropped, version words (live,
/// remix, acoustic …) are kept, so a live version never counts as the studio one.
pub fn identity_title(value: &str) -> String {
    let mut kept = String::new();
    let mut note = String::new();
    let mut depth = 0usize;
    for character in value.chars() {
        match character {
            '(' | '[' => {
                depth += 1;
                note.clear();
            }
            ')' | ']' if depth > 0 => {
                depth -= 1;
                if !is_neutral_note(&note) {
                    kept.push(' ');
                    kept.push_str(&note);
                }
            }
            _ if depth > 0 => note.push(character),
            _ => kept.push(character),
        }
    }
    if let Some(index) = kept.find(" - ") {
        let (head, tail) = kept.split_at(index);
        if is_neutral_note(&tail[3..]) {
            kept = head.to_owned();
        }
    }
    let lower = kept.to_lowercase();
    for marker in [" feat. ", " feat ", " ft. ", " ft ", " featuring "] {
        if let Some(index) = lower.find(marker) {
            kept.truncate(index);
            break;
        }
    }
    normalize(&kept)
}

fn is_neutral_note(note: &str) -> bool {
    let normalized = format!(" {} ", normalize(note));
    NEUTRAL_NOTE_WORDS
        .iter()
        .any(|word| normalized.contains(&format!(" {word} ")))
        && !VARIANT_WORDS
            .iter()
            .any(|word| normalized.contains(&format!(" {word} ")))
}

/// Removes "(Official Video)"-style noise and a leading "Artist - " from a video title.
pub fn clean_video_title(title: &str, artists: &[String]) -> String {
    let mut cleaned = String::new();
    let mut note = String::new();
    let mut depth = 0usize;
    for character in title.chars() {
        match character {
            '(' | '[' => {
                depth += 1;
                note.clear();
            }
            ')' | ']' if depth > 0 => {
                depth -= 1;
                let lower = note.to_lowercase();
                let noise = [
                    "official", "lyric", "audio", "video", "visuali", "hd", "4k", "mv",
                ]
                .iter()
                .any(|word| lower.contains(word));
                if !noise {
                    cleaned.push_str(&format!("({note})"));
                }
            }
            _ if depth > 0 => note.push(character),
            _ => cleaned.push(character),
        }
    }
    if let Some(index) = cleaned.find(" - ") {
        let prefix = normalize(&cleaned[..index]);
        if artists
            .iter()
            .any(|artist| similarity(&prefix, &normalize(artist)) >= 80.0)
        {
            cleaned = cleaned[index + 3..].to_owned();
        }
    }
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if cleaned.is_empty() {
        title.to_owned()
    } else {
        cleaned
    }
}

fn variant_penalty(target_title: &str, candidate_title: &str) -> f64 {
    let wanted = format!(" {} ", normalize(target_title));
    let found = format!(" {} ", normalize(candidate_title));
    let mut penalty = 0.0;
    for word in VARIANT_WORDS {
        let token = format!(" {word} ");
        if found.contains(&token) && !wanted.contains(&token) {
            penalty += 35.0;
        }
    }
    // Any other bracketed note the request lacks ("(Piano)") costs a little,
    // so the exact title wins ties between versions.
    let wanted_notes: Vec<String> = bracket_notes(target_title)
        .iter()
        .map(|note| normalize(note))
        .collect();
    for note in bracket_notes(candidate_title) {
        let normalized = normalize(&note);
        let neutral = NEUTRAL_NOTE_WORDS
            .iter()
            .any(|word| format!(" {normalized} ").contains(&format!(" {word} ")));
        if !wanted_notes.contains(&normalized) && !neutral {
            penalty += 8.0;
        }
    }
    penalty
}

fn bracket_notes(value: &str) -> Vec<String> {
    let mut notes = Vec::new();
    let mut current: Option<String> = None;
    for character in value.chars() {
        match (character, current.as_mut()) {
            ('(' | '[', None) => current = Some(String::new()),
            (')' | ']', Some(_)) => notes.extend(current.take()),
            (_, Some(note)) => note.push(character),
            _ => {}
        }
    }
    notes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(title: &str, artist: &str, duration: f64) -> Target {
        Target {
            title: title.into(),
            artist: Some(artist.into()),
            duration: Some(duration),
        }
    }

    fn song(id: &str, title: &str, artist: &str, duration: f64) -> Candidate {
        Candidate {
            id: id.into(),
            title: title.into(),
            artists: vec![artist.into()],
            duration: Some(duration),
            official: true,
        }
    }

    #[test]
    fn the_original_beats_live_remix_and_sped_up_versions() {
        let wanted = target("Blinding Lights", "The Weeknd", 200.0);
        let candidates = vec![
            song("live", "Blinding Lights (Live)", "The Weeknd", 210.0),
            song("sped", "Blinding Lights (Sped Up)", "The Weeknd", 150.0),
            song("remix", "Blinding Lights - Remix", "The Weeknd", 203.0),
            song("original", "Blinding Lights", "The Weeknd", 202.0),
        ];
        let (best, score) = best(&wanted, &candidates, NORMAL).unwrap();
        assert_eq!(best.id, "original");
        assert!(score.total > 90.0, "{score:?}");
    }

    #[test]
    fn a_requested_version_is_not_penalised() {
        let wanted = target("Blinding Lights (Live)", "The Weeknd", 210.0);
        let candidates = vec![song("live", "Blinding Lights (Live)", "The Weeknd", 211.0)];
        assert!(best(&wanted, &candidates, NORMAL).is_some());
    }

    #[test]
    fn wrong_artist_or_length_is_rejected() {
        let wanted = target("Hello", "Adele", 295.0);
        assert!(
            best(
                &wanted,
                &[song("a", "Hello", "Lionel Richie", 296.0)],
                NORMAL
            )
            .is_none()
        );
        assert!(best(&wanted, &[song("b", "Hello", "Adele", 380.0)], NORMAL).is_none());
        assert!(best(&wanted, &[song("c", "Hello", "Adele", 296.0)], NORMAL).is_some());
    }

    #[test]
    fn featured_artists_and_accents_do_not_hurt() {
        let wanted = target(
            "Despacito (feat. Daddy Yankee)",
            "Luis Fonsi, Daddy Yankee",
            228.0,
        );
        let candidates = vec![song("x", "Despacito", "Luis Fonsi", 229.0)];
        assert!(best(&wanted, &candidates, NORMAL).is_some());
        assert_eq!(normalize("Beyoncé & JAY-Z"), "beyonce and jay z");
    }

    #[test]
    fn identity_titles_ignore_notes_but_keep_versions() {
        assert_eq!(
            identity_title("Song (Remastered 2011)"),
            identity_title("Song")
        );
        assert_eq!(identity_title("Song (feat. X)"), identity_title("Song"));
        assert_eq!(
            identity_title("Song - 2011 Remaster"),
            identity_title("Song")
        );
        assert_ne!(identity_title("Song (Live)"), identity_title("Song"));
        assert_ne!(identity_title("Song - Remix"), identity_title("Song"));
    }

    #[test]
    fn video_titles_lose_noise_and_the_artist_prefix() {
        let artists = vec!["Adele".to_owned()];
        assert_eq!(
            clean_video_title("Adele - Hello (Official Music Video)", &artists),
            "Hello"
        );
        assert_eq!(
            clean_video_title("Hello (Live at the BBC)", &artists),
            "Hello (Live at the BBC)"
        );
    }

    #[test]
    fn helpers_clean_artists_titles_and_channels() {
        assert_eq!(primary_artist("Luis Fonsi, Daddy Yankee"), "Luis Fonsi");
        assert_eq!(
            primary_artist("Calvin Harris feat. Rihanna"),
            "Calvin Harris"
        );
        assert_eq!(base_title("Song (Remastered 2011) - Live"), "song");
        assert_eq!(clean_channel_name("Daft Punk - Topic"), "Daft Punk");
        assert_eq!(clean_channel_name("AdeleVEVO"), "Adele");
        assert_eq!(bracket_notes("A (b) [c]"), vec!["b", "c"]);
    }
}
