//! Versions and credits in song titles: "In The End (Mellen Gi & Tommee
//! Profitt Remix)" is a remix of "In The End", credited to Mellen Gi and Tommee
//! Profitt. Used by both the per-source score and the automatic choice, so they
//! compare titles and artists the same way.

use super::matcher::{artist_similarity, title_similarity};
use std::collections::{BTreeSet, HashSet};

/// The kind of version a title names (remix, live, acoustic…).
pub fn version_tags(value: &str) -> BTreeSet<&'static str> {
    let normalized = normalized_text(value);
    let words = normalized.split_whitespace().collect::<HashSet<_>>();
    let mut tags = BTreeSet::new();
    for (tag, aliases) in [
        ("live", &["live"] as &[&str]),
        ("remix", &["remix", "mix"] as &[&str]),
        ("acoustic", &["acoustic", "unplugged"] as &[&str]),
        // A studio or rooftop session ("Marie (Dach Session)") is its own performance.
        ("session", &["session", "sessions"] as &[&str]),
        ("instrumental", &["instrumental"] as &[&str]),
        ("karaoke", &["karaoke"] as &[&str]),
        ("demo", &["demo"] as &[&str]),
        ("stripped", &["stripped"] as &[&str]),
        ("sped", &["sped"] as &[&str]),
        ("slowed", &["slowed"] as &[&str]),
        ("duet", &["duet"] as &[&str]),
        ("symphony", &["symphony", "symphonic"] as &[&str]),
    ] {
        if aliases.iter().any(|alias| words.contains(alias)) {
            tags.insert(tag);
        }
    }
    tags
}

/// Words in brackets that describe a version, not a person ("Extended Mix").
const VERSION_WORDS: &[&str] = &[
    "extended",
    "radio",
    "club",
    "original",
    "dub",
    "instrumental",
    "official",
    "remastered",
    "remaster",
    "live",
    "acoustic",
    "slowed",
    "sped",
    "up",
    "reverb",
    "version",
    "edit",
    "mix",
    "remix",
    "album",
    "single",
    "explicit",
    "clean",
    "deluxe",
    "bonus",
    "track",
    "audio",
    "video",
];

/// Names a title credits: "In The End (Mellen Gi & Tommee Profitt Remix)" →
/// Mellen Gi, Tommee Profitt; "Song (feat. Fleurie)" → Fleurie.
pub fn title_credits(title: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = title;
    while let Some(open) = rest.find(['(', '[']) {
        let close = if rest[open..].starts_with('(') {
            ')'
        } else {
            ']'
        };
        let Some(length) = rest[open + 1..].find(close) else {
            break;
        };
        let inner = rest[open + 1..open + 1 + length].trim();
        rest = &rest[open + 1 + length + 1..];
        let prefix = ["feat. ", "feat ", "ft. ", "ft ", "featuring ", "with "]
            .iter()
            .find(|prefix| {
                inner
                    .get(..prefix.len())
                    .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
            });
        let credited = if let Some(prefix) = prefix {
            &inner[prefix.len()..]
        } else if let Some(word) = [" remix", " mix", " rework", " edit", " version", " cover"]
            .iter()
            .find(|word| {
                inner
                    .len()
                    .checked_sub(word.len())
                    .and_then(|start| inner.get(start..))
                    .is_some_and(|tail| tail.eq_ignore_ascii_case(word))
            })
        {
            &inner[..inner.len() - word.len()]
        } else {
            continue;
        };
        names.extend(split_names(credited));
    }
    names
}

/// The people in an artist credit: "Tommee Profitt, Mellen Gi & Fleurie".
fn split_names(credit: &str) -> Vec<String> {
    let mut names = vec![credit.to_owned()];
    for separator in [
        ",", " & ", " x ", " X ", " and ", " feat. ", " ft. ", " / ", ";", "،", " و ",
    ] {
        names = names
            .iter()
            .flat_map(|name| name.split(separator).map(str::to_owned).collect::<Vec<_>>())
            .collect();
    }
    names
        .into_iter()
        .map(|name| name.trim().to_owned())
        .filter(|name| {
            let key = normalized_text(name);
            key.chars().filter(|c| c.is_alphanumeric()).count() >= 2
                && !key.split_whitespace().all(|word| {
                    VERSION_WORDS.contains(&word) || word.chars().all(|c| c.is_ascii_digit())
                })
        })
        .collect()
}

/// The title without anything in brackets: "In The End (Mellen Gi Remix)" → "In The End".
pub fn base_title(title: &str) -> String {
    let mut out = String::new();
    let mut depth = 0_i32;
    for character in title.chars() {
        match character {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = (depth - 1).max(0),
            _ if depth == 0 => out.push(character),
            _ => {}
        }
    }
    out.trim().to_owned()
}

/// Two versions are the same when the song, the kind of version and (where
/// both name one) the remixer agree: "In The End (Mellen Gi & Tommee Profitt
/// Remix)" = "In the End (Mellen Gi Remix)".
pub fn version_aware_title_similarity(file: &str, candidate: &str) -> f64 {
    let direct = title_similarity(file, candidate);
    let variants = version_tags(file);
    if variants.is_empty() || variants != version_tags(candidate) {
        return direct;
    }
    if title_similarity(&base_title(file), &base_title(candidate)) < 0.94 {
        return direct;
    }
    let (file_credits, candidate_credits) = (title_credits(file), title_credits(candidate));
    let remixer_agrees = file_credits.is_empty()
        || candidate_credits.is_empty()
        || candidate_credits.iter().any(|credit| {
            file_credits
                .iter()
                .any(|other| artist_similarity(credit, other) >= 0.9)
        });
    if remixer_agrees {
        direct.max(0.95)
    } else {
        direct
    }
}

/// The file's artist tag and the names its title credits ("… (Mellen Gi &
/// Tommee Profitt Remix)") against the candidate's performers. Two shared
/// names is a full match; one shared name (a duet, a remixer) is a good one.
/// The same people in any order: "Tommee Profitt, Mellen Gi & Fleurie" =
/// "Tommee Profitt, Fleurie & Mellen Gi". Otherwise the plain text similarity.
pub fn artists_similarity(left: &str, right: &str) -> f64 {
    let direct = artist_similarity(left, right);
    let (left_names, right_names) = (split_names(left), split_names(right));
    let covers = |names: &[String], others: &[String]| {
        names.iter().all(|name| {
            others
                .iter()
                .any(|other| artist_similarity(name, other) >= 0.9)
        })
    };
    if left_names.len() > 1
        && left_names.len() == right_names.len()
        && covers(&left_names, &right_names)
        && covers(&right_names, &left_names)
    {
        direct.max(1.0)
    } else {
        direct
    }
}

/// Every person the shorter credit names is also in the longer one: "54 Ultra"
/// is within "ANOTR feat. 54 Ultra". Stores often list only some of a song's
/// singers; that alone does not make it another recording.
pub fn credit_within(left: &str, right: &str) -> bool {
    let (left_names, right_names) = (split_names(left), split_names(right));
    let (shorter, longer) = if left_names.len() <= right_names.len() {
        (left_names, right_names)
    } else {
        (right_names, left_names)
    };
    !shorter.is_empty()
        && shorter.iter().all(|name| {
            longer
                .iter()
                .any(|other| artist_similarity(name, other) >= 0.9)
        })
}

pub fn credited_artist_similarity(
    file_artist: &str,
    file_title: &str,
    candidate_artist: &str,
    candidate_title: &str,
) -> f64 {
    let mut credited = split_names(file_artist);
    credited.extend(title_credits(file_title));
    let mut performers = split_names(candidate_artist);
    performers.extend(title_credits(candidate_title));
    let mut shared = performers
        .iter()
        .filter(|performer| {
            credited
                .iter()
                .any(|name| artist_similarity(name, performer) >= 0.9)
        })
        .map(|performer| normalized_text(performer))
        .collect::<Vec<_>>();
    shared.sort();
    shared.dedup();
    let similarity: f64 = match shared.len() {
        0 => 0.0,
        1 => 0.85,
        _ => 1.0,
    };
    if leaves_out_main_singer(file_artist, file_title, &performers) {
        similarity.min(MAIN_SINGER_MISSING_SIMILARITY)
    } else {
        similarity
    }
}

/// A source that credits only the featured singers ("Sohrab MJ & Orchid" for a file tagged
/// "Amir Tataloo feat. Sohrab MJ & Orchid") shares names with the file but leaves out its
/// main singer. That is not an exact artist match: it needs the audio or a second source
/// before a song is filed on its own. Remixers named in the title don't count: a remix is
/// rightly credited to them, not to the original singer.
fn leaves_out_main_singer(file_artist: &str, file_title: &str, performers: &[String]) -> bool {
    let singers = super::credits::normalize_featured(file_artist, file_title)
        .artists
        .iter()
        .flat_map(|credit| super::credits::loose_names(&credit.name))
        .collect::<Vec<_>>();
    let Some((main, featured)) = singers.split_first() else {
        return false;
    };
    let credits = |name: &str| {
        performers
            .iter()
            .any(|performer| artist_similarity(name, performer) >= 0.9)
    };
    !credits(main) && featured.iter().any(|name| credits(name))
}

/// Below the 0.82 an exact title-and-artist match needs, at or above the 0.72 that audio
/// recognition or two agreeing sources need (see `smart_approval::is_supported`).
const MAIN_SINGER_MISSING_SIMILARITY: f64 = 0.75;

fn normalized_text(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
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
