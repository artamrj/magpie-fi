use crate::{
    domain::provider_name,
    infrastructure::providers::{ArtworkStatus, Candidate},
};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CompletionReport {
    pub score: u8,
    pub core_complete: bool,
    pub enriched_fields: Vec<String>,
    pub missing_fields: Vec<String>,
}

impl CompletionReport {
    pub fn summary(&self) -> String {
        if self.missing_fields.is_empty() {
            format!("metadata audit {}% complete", self.score)
        } else {
            format!(
                "metadata audit {}% complete; missing {}",
                self.score,
                self.missing_fields.join(", ")
            )
        }
    }
}

/// Complete a selected recording from corroborating catalog rows. Existing values
/// are never replaced, and release-specific fields are copied only from a matching
/// album so a compilation cannot silently change the chosen original release.
pub fn complete(
    selected: &mut Candidate,
    candidates: &[Candidate],
    existing_album: Option<&str>,
) -> CompletionReport {
    normalize_empty_fields(selected);
    let mut agreeing = candidates
        .iter()
        .filter(|candidate| same_recording(selected, candidate))
        .collect::<Vec<_>>();
    agreeing.sort_by(|left, right| donor_quality(right).total_cmp(&donor_quality(left)));

    let mut enriched = Vec::new();
    use_the_most_common_spelling(selected, &agreeing, &mut enriched);
    complete_release_identity(selected, &agreeing, &mut enriched);
    if selected.album.is_none()
        && let Some(donor) = choose_album_donor(&agreeing, existing_album)
    {
        fill_string(
            &mut selected.album,
            donor.album.as_ref(),
            "album",
            &mut enriched,
        );
        fill_string(
            &mut selected.album_artist,
            donor.album_artist.as_ref().or(Some(&donor.artist)),
            "album artist",
            &mut enriched,
        );
    }

    let release_donors = agreeing
        .iter()
        .copied()
        .filter(|candidate| {
            albums_compatible(selected.album.as_deref(), candidate.album.as_deref())
        })
        .collect::<Vec<_>>();
    if selected.album_artist.is_none() {
        let donor = release_donors
            .iter()
            .find_map(|candidate| candidate.album_artist.as_ref().or(Some(&candidate.artist)));
        fill_string(
            &mut selected.album_artist,
            donor,
            "album artist",
            &mut enriched,
        );
    }
    fill_from_release(
        &mut selected.track_number,
        &release_donors,
        |candidate| candidate.track_number,
        "track number",
        &mut enriched,
    );
    fill_from_release(
        &mut selected.track_total,
        &release_donors,
        |candidate| candidate.track_total,
        "track total",
        &mut enriched,
    );
    fill_from_release(
        &mut selected.disc_number,
        &release_donors,
        |candidate| candidate.disc_number,
        "disc number",
        &mut enriched,
    );
    fill_from_release(
        &mut selected.disc_total,
        &release_donors,
        |candidate| candidate.disc_total,
        "disc total",
        &mut enriched,
    );
    fill_release_string(
        &mut selected.release_date,
        &release_donors,
        |candidate| candidate.release_date.as_ref(),
        "release date",
        &mut enriched,
    );
    fill_release_string(
        &mut selected.year,
        &release_donors,
        |candidate| candidate.year.as_ref(),
        "year",
        &mut enriched,
    );
    if selected.year.is_none()
        && let Some(year) = selected
            .release_date
            .as_deref()
            .and_then(|date| date.get(..4))
    {
        selected.year = Some(year.to_owned());
        enriched.push("year".into());
    }
    take_earliest_album_year(selected, &release_donors, &mut enriched);
    fill_release_string(
        &mut selected.label,
        &release_donors,
        |candidate| candidate.label.as_ref(),
        "label",
        &mut enriched,
    );
    fill_release_string(
        &mut selected.release_country,
        &release_donors,
        |candidate| candidate.release_country.as_ref(),
        "release country",
        &mut enriched,
    );

    fill_any_string(
        &mut selected.genre,
        &agreeing,
        |candidate| candidate.genre.as_ref(),
        "genre",
        &mut enriched,
    );
    fill_any_string(
        &mut selected.composer,
        &agreeing,
        |candidate| candidate.composer.as_ref(),
        "composer",
        &mut enriched,
    );
    fill_any_string(
        &mut selected.isrc,
        &agreeing,
        |candidate| candidate.isrc.as_ref(),
        "ISRC",
        &mut enriched,
    );
    if selected.cover_url.is_none()
        && let Some(url) = agreeing
            .iter()
            .filter_map(|candidate| {
                candidate
                    .cover_url
                    .as_ref()
                    .map(|url| (artwork_priority(&candidate.provider), candidate.score, url))
            })
            .max_by(|left, right| {
                left.0
                    .cmp(&right.0)
                    .then_with(|| left.1.total_cmp(&right.1))
            })
            .map(|(_, _, url)| url)
    {
        selected.cover_url = Some(url.clone());
        enriched.push("cover".into());
    }

    let (album_defaulted, album_artist_defaulted) = normalize_release_fields(selected);
    if album_defaulted {
        enriched.push("album".into());
    }
    if album_artist_defaulted {
        enriched.push("album artist".into());
    }

    enriched.sort();
    enriched.dedup();
    let report = audit(selected, enriched);
    record_report(selected, &agreeing, &report);
    report
}

pub fn audit(candidate: &Candidate, enriched_fields: Vec<String>) -> CompletionReport {
    let cover = candidate.artwork_status == ArtworkStatus::Verified;
    let fields = [
        ("title", nonempty(Some(&candidate.title)).is_some(), 18_u8),
        ("artist", nonempty(Some(&candidate.artist)).is_some(), 18),
        ("album", nonempty(candidate.album.as_deref()).is_some(), 16),
        ("cover", cover, 16),
        (
            "year",
            nonempty(candidate.year.as_deref()).is_some()
                || nonempty(candidate.release_date.as_deref()).is_some(),
            10,
        ),
        ("genre", nonempty(candidate.genre.as_deref()).is_some(), 8),
        ("track number", candidate.track_number.is_some(), 6),
        (
            "album artist",
            nonempty(candidate.album_artist.as_deref()).is_some(),
            4,
        ),
        ("ISRC", nonempty(candidate.isrc.as_deref()).is_some(), 4),
    ];
    let score = fields
        .iter()
        .filter(|(_, present, _)| *present)
        .map(|(_, _, weight)| *weight)
        .sum();
    let missing_fields = fields
        .iter()
        .filter(|(_, present, _)| !*present)
        .map(|(name, _, _)| (*name).to_owned())
        .collect::<Vec<_>>();
    let core_complete = fields[..4].iter().all(|(_, present, _)| *present);
    CompletionReport {
        score,
        core_complete,
        enriched_fields,
        missing_fields,
    }
}

pub fn reassess(candidate: &mut Candidate) -> CompletionReport {
    normalize_release_fields(candidate);
    let enriched_fields = candidate
        .score_breakdown
        .as_deref()
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .and_then(|value| {
            value["metadata_completion"]["enriched_fields"]
                .as_array()
                .map(|fields| {
                    fields
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect::<Vec<_>>()
                })
        })
        .unwrap_or_default();
    let report = audit(candidate, enriched_fields);
    record_report(candidate, &[], &report);
    report
}

/// Only a real year is ever written: four digits, alone or at the start of a date
/// ("2020", "2020-10-05"). A day-first date ("05.10.2020", "5/10/2020") is turned round;
/// anything else ("05.10.1", a half-typed value) is dropped in favour of the release date.
fn clean_year(candidate: &mut Candidate) {
    let Some(raw) = candidate.year.as_deref().map(str::trim) else {
        return;
    };
    let starts_with_year = |value: &str| {
        value.len() >= 4
            && value.as_bytes()[..4].iter().all(u8::is_ascii_digit)
            && matches!(value.as_bytes().get(4), None | Some(b'-'))
    };
    if starts_with_year(raw) {
        return;
    }
    let parts = raw
        .split(['.', '/'])
        .map(|part| part.trim().parse::<u32>().ok())
        .collect::<Option<Vec<_>>>()
        .unwrap_or_default();
    let day_first = match parts.as_slice() {
        [day @ 1..=31, month @ 1..=12, year @ 1000..=9999] => Some((*year, *month, *day)),
        _ => None,
    };
    let from_release_date = candidate
        .release_date
        .as_deref()
        .map(str::trim)
        .filter(|date| starts_with_year(date))
        .map(|date| date[..4].to_owned());
    candidate.year = match day_first {
        Some((year, month, day)) => {
            if candidate
                .release_date
                .as_deref()
                .is_none_or(|date| date.trim().is_empty())
            {
                candidate.release_date = Some(format!("{year:04}-{month:02}-{day:02}"));
            }
            Some(format!("{year:04}"))
        }
        None => from_release_date,
    };
}

/// Apply the canonical release fields. A single or EP keeps its own title as
/// the album (Navidrome groups albums by name, so a generic "Single" album would
/// merge every single of an artist into one). Store suffixes such as
/// " - Single" are dropped because the release type already says it.
///
/// Returns which fields were changed so metadata-completion reports can include
/// the generated values.
pub fn normalize_release_fields(candidate: &mut Candidate) -> (bool, bool) {
    clean_year(candidate);
    let original_album = candidate.album.clone();
    if candidate
        .release_type
        .as_deref()
        .is_none_or(|kind| kind.trim().is_empty())
        && let Some(kind) = inferred_release_type(candidate)
    {
        candidate.release_type = Some(kind);
    }
    let usable = |value: String| {
        let value = value.trim().to_owned();
        (!value.is_empty()
            && !value.eq_ignore_ascii_case("single")
            && !value.eq_ignore_ascii_case("ep"))
        .then_some(value)
    };
    let from_album = candidate.album.as_deref().and_then(|album| {
        usable(strip_release_suffix(
            &crate::domain::credits::release_title_without_featured(album),
        ))
    });
    let from_title = usable(crate::domain::credits::release_title_without_featured(
        &candidate.title,
    ));
    candidate.album = from_album.or(from_title).or(original_album.clone());
    let album_defaulted = candidate.album != original_album;

    let album_artist_defaulted = candidate
        .album_artist
        .as_deref()
        .is_none_or(|album_artist| album_artist.trim().is_empty());
    if album_artist_defaulted {
        if candidate.is_compilation {
            candidate.album_artist = Some("Various Artists".into());
            candidate.album_artist_credits = vec![crate::domain::credits::ArtistCredit::new(
                "Various Artists",
                "",
            )];
        } else {
            // The album belongs to the main singers; a guest on the song ("feat.") does not
            // become an artist of the album.
            let credits = if candidate.artist_credits.is_empty() {
                crate::domain::credits::normalize_featured(&candidate.artist, &candidate.title)
                    .artists
            } else {
                candidate.artist_credits.clone()
            };
            let main = crate::domain::credits::main_credits(&credits);
            candidate.album_artist = Some(if main.is_empty() {
                candidate.artist.clone()
            } else {
                crate::domain::credits::display_artist(&main)
            });
            candidate.album_artist_credits = main;
        }
    } else if candidate.album_artist_credits.is_empty()
        && let Some(album_artist) = candidate.album_artist.as_deref()
    {
        let credits = crate::domain::credits::normalize_featured(album_artist, "");
        candidate.album_artist = Some(credits.artist);
        candidate.album_artist_credits = credits.artists;
    }

    (album_defaulted, album_artist_defaulted)
}

fn complete_release_identity(
    selected: &mut Candidate,
    agreeing: &[&Candidate],
    enriched: &mut Vec<String>,
) {
    if selected.release_type.is_none()
        && let Some(kind) = agreeing
            .iter()
            .filter(|candidate| release_evidence_compatible(selected, candidate))
            .filter_map(|candidate| {
                inferred_release_type(candidate).map(|kind| {
                    (
                        provider_priority(&candidate.provider),
                        candidate.score as i64,
                        kind,
                    )
                })
            })
            .max_by_key(|(authority, score, _)| (*authority, *score))
            .map(|(_, _, kind)| kind)
    {
        selected.release_type = Some(kind);
        enriched.push("release type".into());
    }
}

fn release_evidence_compatible(selected: &Candidate, donor: &Candidate) -> bool {
    match (selected.album.as_deref(), donor.album.as_deref()) {
        (None, _) | (_, None) => true,
        (Some(selected), Some(donor)) => {
            let selected = comparable_release_title(selected);
            let donor = comparable_release_title(donor);
            selected == donor || strsim::normalized_levenshtein(&selected, &donor) >= 0.82
        }
    }
}

fn comparable_release_title(value: &str) -> String {
    let cleaned = crate::domain::credits::release_title_without_featured(value);
    let lower = cleaned.to_lowercase();
    let without_type = [" - single", " - ep"]
        .into_iter()
        .find_map(|suffix| lower.strip_suffix(suffix))
        .unwrap_or(&lower);
    normalized(without_type)
}

/// "Farangis - Single" → "Farangis", "Moorche - EP" → "Moorche".
fn strip_release_suffix(album: &str) -> String {
    let trimmed = album.trim();
    let lower = trimmed.to_ascii_lowercase();
    for suffix in [
        " - single",
        " - ep",
        " (single)",
        " [single]",
        " (ep)",
        " [ep]",
    ] {
        if lower.ends_with(suffix) {
            return trimmed[..trimmed.len() - suffix.len()].trim().to_owned();
        }
    }
    trimmed.to_owned()
}

fn inferred_release_type(candidate: &Candidate) -> Option<String> {
    if let Some(kind) = candidate
        .release_type
        .as_deref()
        .map(str::trim)
        .filter(|kind| !kind.is_empty())
    {
        return Some(kind.to_ascii_lowercase());
    }
    let album = candidate.album.as_deref()?.trim().to_ascii_lowercase();
    if (album.ends_with(" - single") || album == "single")
        && candidate.track_total.is_none_or(|count| count == 1)
    {
        Some("single".into())
    } else if album.ends_with(" - ep") || album == "ep" {
        Some("ep".into())
    } else {
        None
    }
}

fn choose_album_donor<'a>(
    candidates: &[&'a Candidate],
    existing_album: Option<&str>,
) -> Option<&'a Candidate> {
    if let Some(existing) = existing_album.and_then(|value| nonempty(Some(value))) {
        let best = candidates
            .iter()
            .filter(|candidate| candidate.album.is_some())
            .map(|candidate| {
                (
                    text_similarity(existing, candidate.album.as_deref().unwrap_or_default()),
                    donor_quality(candidate),
                    *candidate,
                )
            })
            .max_by(|left, right| {
                left.0
                    .total_cmp(&right.0)
                    .then_with(|| left.1.total_cmp(&right.1))
            });
        if let Some((similarity, _, candidate)) = best
            && similarity >= 0.72
        {
            return Some(candidate);
        }
    }

    let mut frequency = HashMap::<String, usize>::new();
    for album in candidates
        .iter()
        .filter_map(|candidate| candidate.album.as_deref())
    {
        *frequency.entry(normalized(album)).or_default() += 1;
    }
    candidates
        .iter()
        .copied()
        .filter(|candidate| candidate.album.is_some())
        .max_by(|left, right| {
            album_donor_quality(left, &frequency).total_cmp(&album_donor_quality(right, &frequency))
        })
}

fn album_donor_quality(candidate: &Candidate, frequency: &HashMap<String, usize>) -> f64 {
    let agreement = candidate
        .album
        .as_deref()
        .map(normalized)
        .and_then(|album| frequency.get(&album).copied())
        .unwrap_or_default() as f64;
    donor_quality(candidate)
        + agreement * 8.0
        + if is_album_release(candidate) {
            8.0
        } else {
            0.0
        }
        - if candidate.is_compilation { 18.0 } else { 0.0 }
}

fn donor_quality(candidate: &Candidate) -> f64 {
    provider_priority(&candidate.provider) as f64 * 10.0
        + candidate.score.clamp(0.0, 100.0) * 0.08
        + [
            candidate.album.is_some(),
            candidate.cover_url.is_some(),
            candidate.year.is_some() || candidate.release_date.is_some(),
            candidate.genre.is_some(),
            candidate.track_number.is_some(),
        ]
        .into_iter()
        .filter(|present| *present)
        .count() as f64
}

pub(crate) fn same_recording(left: &Candidate, right: &Candidate) -> bool {
    if left
        .isrc
        .as_deref()
        .zip(right.isrc.as_deref())
        .is_some_and(|(left, right)| left.eq_ignore_ascii_case(right))
    {
        return true;
    }
    let both_close = |limit: f64| match (left.duration_delta, right.duration_delta) {
        (Some(left), Some(right)) => left <= limit && right <= limit,
        _ => false,
    };
    // The same Persian words in another Latin spelling ("Gheseh Eshgh" and "Ghesseyeh
    // Eshgh") count as the same title when both lengths match the file.
    let same_title = text_similarity(&left.title, &right.title) >= 0.90
        || (crate::domain::names::same_loose_title(&left.title, &right.title) && both_close(3.0));
    crate::domain::versions::version_tags(&left.title)
        == crate::domain::versions::version_tags(&right.title)
        && same_title
        && text_similarity(&left.artist, &right.artist).max(
            crate::domain::versions::artists_similarity(&left.artist, &right.artist),
        ) >= 0.82
        && match (left.duration_delta, right.duration_delta) {
            (Some(left), Some(right)) => left <= 15.0 && right <= 15.0,
            _ => true,
        }
}

pub(crate) fn albums_compatible(selected: Option<&str>, donor: Option<&str>) -> bool {
    match (
        selected.and_then(|value| nonempty(Some(value))),
        donor.and_then(|value| nonempty(Some(value))),
    ) {
        // "Farangis" and "Farangis - Single" are the same release title.
        (Some(selected), Some(donor)) => {
            text_similarity(
                &comparable_release_title(selected),
                &comparable_release_title(donor),
            ) >= 0.72
        }
        _ => false,
    }
}

fn fill_string(
    target: &mut Option<String>,
    value: Option<&String>,
    name: &str,
    enriched: &mut Vec<String>,
) {
    if target.is_none()
        && let Some(value) = value.filter(|value| nonempty(Some(value)).is_some())
    {
        *target = Some(value.clone());
        enriched.push(name.to_owned());
    }
}

fn fill_from_release<T: Copy>(
    target: &mut Option<T>,
    donors: &[&Candidate],
    value: impl Fn(&Candidate) -> Option<T>,
    name: &str,
    enriched: &mut Vec<String>,
) {
    if target.is_none()
        && let Some(found) = donors.iter().find_map(|candidate| value(candidate))
    {
        *target = Some(found);
        enriched.push(name.to_owned());
    }
}

/// Sources spell Persian titles in Latin letters their own way. When the matches for this
/// song differ only in spelling ("Gheseh Eshgh" on Apple Music, Radio Javan and Audiomack,
/// "Ghesseyeh Eshgh" on MusicBrainz), the spelling most sources use wins. A tie keeps the
/// chosen match's spelling.
fn use_the_most_common_spelling(
    selected: &mut Candidate,
    agreeing: &[&Candidate],
    enriched: &mut Vec<String>,
) {
    let key = |title: &str| crate::domain::names::match_key(title);
    let mut votes: HashMap<String, (usize, String)> = HashMap::new();
    let mut vote = |candidate: &Candidate| {
        let entry = votes
            .entry(key(&candidate.title))
            .or_insert((0, candidate.title.clone()));
        entry.0 += crate::application::smart_approval::source_count(candidate);
    };
    vote(selected);
    for candidate in agreeing {
        let itself = candidate.id.is_some() && candidate.id == selected.id;
        if !itself && crate::domain::names::same_loose_title(&candidate.title, &selected.title) {
            vote(candidate);
        }
    }
    let own = votes
        .get(&key(&selected.title))
        .map_or(0, |(count, _)| *count);
    let Some((_, (count, title))) = votes.into_iter().max_by_key(|(_, (count, _))| *count) else {
        return;
    };
    if count > own {
        selected.title = title;
        enriched.push("title spelling".into());
    }
}

/// Stores list an old album under the year it went digital: Ebi's "Koohe Yakh" is 2008 on
/// Apple Music and 1991 on MusicBrainz. Another listing of the same album with an earlier
/// year is closer to when it really came out, so that year wins. Only the same album counts:
/// a single or EP usually comes out before its album, and a compilation is another release.
fn take_earliest_album_year(
    selected: &mut Candidate,
    donors: &[&Candidate],
    enriched: &mut Vec<String>,
) {
    let Some(current) = release_year(selected) else {
        return;
    };
    let Some(album) = nonempty(selected.album.as_deref()) else {
        return;
    };
    if !is_plain_album(selected) {
        return;
    }
    let earliest = donors
        .iter()
        .copied()
        .filter(|donor| is_plain_album(donor))
        .filter(|donor| {
            donor.album.as_deref().is_some_and(|other| {
                comparable_release_title(album) == comparable_release_title(other)
                    || crate::domain::names::same_loose_title(album, other)
            })
        })
        .filter_map(|donor| release_year(donor).map(|year| (year, donor)))
        .filter(|(year, _)| (1900..current).contains(year))
        .min_by_key(|(year, _)| *year);
    let Some((year, donor)) = earliest else {
        return;
    };
    let year = year.to_string();
    selected.release_date = donor
        .release_date
        .clone()
        .filter(|date| date.starts_with(&year));
    selected.year = Some(year);
    enriched.push("original year".into());
}

fn release_year(candidate: &Candidate) -> Option<i32> {
    nonempty(candidate.year.as_deref())
        .or(nonempty(candidate.release_date.as_deref()))
        .and_then(|value| value.get(..4))
        .and_then(|year| year.parse().ok())
}

/// A normal studio album: not a single, EP, compilation or live album.
fn is_plain_album(candidate: &Candidate) -> bool {
    let album = candidate
        .album
        .as_deref()
        .unwrap_or_default()
        .trim()
        .to_lowercase();
    let secondary = candidate
        .release_secondary_types
        .as_deref()
        .unwrap_or_default()
        .to_lowercase();
    candidate
        .release_type
        .as_deref()
        .is_none_or(|kind| kind.trim().eq_ignore_ascii_case("album"))
        && !candidate.is_compilation
        && !candidate
            .album_artist
            .as_deref()
            .is_some_and(|artist| artist.trim().eq_ignore_ascii_case("various artists"))
        && !album.ends_with(" - single")
        && !album.ends_with(" - ep")
        && !secondary.contains("compilation")
        && !secondary.contains("live")
        // Apple: a single has 1-3 songs.
        && candidate.track_total.is_none_or(|total| total > 3)
}

fn fill_release_string(
    target: &mut Option<String>,
    donors: &[&Candidate],
    value: impl Fn(&Candidate) -> Option<&String>,
    name: &str,
    enriched: &mut Vec<String>,
) {
    if target.is_none()
        && let Some(found) = donors.iter().find_map(|candidate| value(candidate))
    {
        *target = Some(found.clone());
        enriched.push(name.to_owned());
    }
}

fn fill_any_string(
    target: &mut Option<String>,
    donors: &[&Candidate],
    value: impl Fn(&Candidate) -> Option<&String>,
    name: &str,
    enriched: &mut Vec<String>,
) {
    fill_release_string(target, donors, value, name, enriched);
}

fn record_report(candidate: &mut Candidate, donors: &[&Candidate], report: &CompletionReport) {
    let mut breakdown = candidate
        .score_breakdown
        .as_deref()
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    let mut sources = breakdown["sources"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect::<HashSet<_>>();
    sources.insert(provider_name(&candidate.provider).to_owned());
    for donor in donors {
        sources.insert(provider_name(&donor.provider).to_owned());
    }
    let mut sources = sources.into_iter().collect::<Vec<_>>();
    sources.sort();
    let cover_verified = breakdown["metadata_completion"]["cover_verified"]
        .as_bool()
        .unwrap_or(false);
    let cover_source = breakdown["metadata_completion"]["cover_source"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| {
            if candidate.cover_url.is_some() {
                "catalog".into()
            } else {
                "missing".into()
            }
        });
    let artwork_failures = breakdown["metadata_completion"]["artwork_failures"].clone();
    // Written by the cover worker; the cover finder reads it to space its searches.
    let cover_searched_at = breakdown["metadata_completion"]["cover_searched_at"].clone();
    breakdown["sources"] = serde_json::json!(sources);
    breakdown["metadata_completion"] = serde_json::json!({
        "score": report.score,
        "core_complete": report.core_complete,
        "enriched_fields": report.enriched_fields,
        "missing_fields": report.missing_fields,
        "cover_source": cover_source,
        "cover_verified": cover_verified,
        "artwork_failures": artwork_failures,
        "cover_searched_at": cover_searched_at
    });
    candidate.score_breakdown = Some(breakdown.to_string());
}

fn normalize_empty_fields(candidate: &mut Candidate) {
    for value in [
        &mut candidate.album,
        &mut candidate.album_artist,
        &mut candidate.year,
        &mut candidate.genre,
        &mut candidate.composer,
        &mut candidate.label,
        &mut candidate.isrc,
        &mut candidate.cover_url,
        &mut candidate.release_date,
        &mut candidate.release_country,
    ] {
        if value
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
        {
            *value = None;
        }
    }
}

fn provider_priority(provider: &str) -> u8 {
    match provider {
        "musicbrainz" | "itunes" | "spotify" | "radiojavan" | "audiomack" | "navahang"
        | "shazam" | "genius" => 6,
        "deezer" | "songrec" => 5,
        "discogs" => 4,
        "lastfm" | "soundcloud" => 3,
        _ => 2,
    }
}

fn artwork_priority(provider: &str) -> u8 {
    match provider {
        "itunes" | "spotify" | "shazam" => 8,
        "musicbrainz" | "radiojavan" | "audiomack" | "navahang" | "genius" => 7,
        "soundcloud" | "songrec" => 6,
        "deezer" => 5,
        "discogs" => 4,
        _ => 2,
    }
}

fn is_album_release(candidate: &Candidate) -> bool {
    candidate
        .release_type
        .as_deref()
        .is_some_and(|kind| kind.eq_ignore_ascii_case("album"))
        && !candidate.is_compilation
}

pub(crate) fn text_similarity(left: &str, right: &str) -> f64 {
    strsim::normalized_levenshtein(&normalized(left), &normalized(right))
}

fn normalized(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_alphanumeric() {
                character.to_lowercase().collect::<String>()
            } else {
                " ".to_owned()
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn nonempty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(provider: &str, album: Option<&str>) -> Candidate {
        Candidate {
            provider: provider.into(),
            title: "Farangis".into(),
            artist: "Siavash Ghomayshi".into(),
            album: album.map(str::to_owned),
            duration_delta: Some(1.0),
            score: 90.0,
            ..Default::default()
        }
    }

    #[test]
    fn only_a_real_year_is_kept() {
        let year_of = |year: &str, release_date: Option<&str>| {
            let mut song = candidate("manual", Some("Double Album"));
            song.year = Some(year.into());
            song.release_date = release_date.map(str::to_owned);
            normalize_release_fields(&mut song);
            (song.year, song.release_date)
        };
        assert_eq!(year_of("2020", None).0.as_deref(), Some("2020"));
        assert_eq!(year_of("2020-10-05", None).0.as_deref(), Some("2020-10-05"));
        // A date written day first.
        assert_eq!(
            year_of("05.10.2020", None),
            (Some("2020".into()), Some("2020-10-05".into()))
        );
        // A half-typed value is never written; the release date still knows the year.
        assert_eq!(year_of("05.10.1", None).0, None);
        assert_eq!(
            year_of("05.10.1", Some("2020-10-05")).0.as_deref(),
            Some("2020")
        );
    }

    #[test]
    fn a_guest_on_the_song_is_not_an_album_artist() {
        let mut song = candidate("deezer", Some("Double Album"));
        song.artist = "Alireza JJ, Sijal & Sepehr Khalse feat. Hoomaan".into();
        song.album_artist = None;
        normalize_release_fields(&mut song);
        assert_eq!(
            song.album_artist.as_deref(),
            Some("Alireza JJ, Sijal & Sepehr Khalse")
        );
    }

    #[test]
    fn fills_missing_fields_from_same_recording_and_release() {
        let mut selected = candidate("musicbrainz", Some("Khabe Baroon"));
        let mut deezer = candidate("deezer", Some("Khabe Baroon"));
        deezer.album_artist = Some("Siavash Ghomayshi".into());
        deezer.year = Some("1993".into());
        deezer.genre = Some("Pop".into());
        deezer.track_number = Some(3);
        deezer.cover_url = Some("https://example.test/farangis.jpg".into());

        let report = complete(&mut selected, &[deezer], None);

        assert!(!report.core_complete);
        assert_eq!(selected.year.as_deref(), Some("1993"));
        assert_eq!(selected.genre.as_deref(), Some("Pop"));
        assert_eq!(selected.track_number, Some(3));
        assert_eq!(
            selected.cover_url.as_deref(),
            Some("https://example.test/farangis.jpg")
        );
    }

    #[test]
    fn an_old_album_keeps_its_earliest_year_not_the_store_upload_date() {
        // Apple Music lists Ebi's "Koohe Yakh" under 2008, when it went digital; MusicBrainz
        // spells the song another way and has the album in 1991.
        let ebi = |provider: &str, title: &str, year: &str, date: &str| Candidate {
            provider: provider.into(),
            title: title.into(),
            artist: "Ebi".into(),
            album: Some("Koohe Yakh".into()),
            year: Some(year.into()),
            release_date: Some(date.into()),
            track_total: Some(12),
            duration_delta: Some(0.5),
            score: 95.0,
            ..Default::default()
        };
        let mut selected = ebi("itunes", "Gheseh Eshgh", "2008", "2008-07-25");
        let musicbrainz = ebi("musicbrainz", "Ghesseyeh Eshgh", "1991", "1991-02-20");
        complete(&mut selected, &[musicbrainz], None);
        assert_eq!(selected.year.as_deref(), Some("1991"));
        assert_eq!(selected.release_date.as_deref(), Some("1991-02-20"));

        // The year comes from MusicBrainz, but the spelling from the three other sources.
        let sources = |names: &[&str]| Some(serde_json::json!({ "sources": names }).to_string());
        let mut selected = ebi("musicbrainz", "Ghesseyeh Eshgh", "1991", "1991-02-20");
        selected.score_breakdown = sources(&["musicbrainz"]);
        let mut apple = ebi("itunes", "Gheseh Eshgh", "2008", "2008-07-25");
        apple.score_breakdown = sources(&["itunes", "radiojavan", "audiomack"]);
        complete(&mut selected, &[apple], None);
        assert_eq!(selected.title, "Gheseh Eshgh");
        assert_eq!(selected.year.as_deref(), Some("1991"));

        // A single, an EP or a compilation that came out earlier is another release.
        for other in ["single", "compilation"] {
            let mut selected = ebi("itunes", "Gheseh Eshgh", "2008", "2008-07-25");
            let mut earlier = ebi("deezer", "Gheseh Eshgh", "1977", "1977-01-01");
            if other == "single" {
                earlier.release_type = Some("single".into());
                earlier.album = Some("Koohe Yakh - Single".into());
            } else {
                earlier.is_compilation = true;
            }
            complete(&mut selected, &[earlier], None);
            assert_eq!(selected.year.as_deref(), Some("2008"), "{other}");
        }
    }

    #[test]
    fn does_not_merge_a_different_version_or_artist() {
        let mut selected = candidate("musicbrainz", Some("Khabe Baroon"));
        let mut remix = candidate("radiojavan", Some("Farangis Remix"));
        remix.title = "Farangis (Remix)".into();
        remix.cover_url = Some("https://example.test/remix.jpg".into());
        let mut other_artist = candidate("itunes", Some("Farangis - Single"));
        other_artist.artist = "Another Artist".into();
        other_artist.genre = Some("Rock".into());

        let report = complete(&mut selected, &[remix, other_artist], None);

        assert!(!report.core_complete);
        assert!(selected.cover_url.is_none());
        assert!(selected.genre.is_none());
    }

    #[test]
    fn defaults_missing_release_fields_for_a_single() {
        let mut selected = candidate("manual", None);

        let report = complete(&mut selected, &[], None);

        // A release without an album name is named after the song.
        assert_eq!(selected.album.as_deref(), Some(selected.title.as_str()));
        assert_eq!(selected.album_artist.as_deref(), Some("Siavash Ghomayshi"));
        assert!(report.enriched_fields.contains(&"album".to_owned()));
        assert!(report.enriched_fields.contains(&"album artist".to_owned()));
    }

    #[test]
    fn normalizes_single_release_names_but_preserves_one_word_albums() {
        let mut single = candidate("itunes", Some("Farangis - Single"));
        single.release_type = Some("single".into());
        let mut album = candidate("musicbrainz", Some("Thriller"));

        normalize_release_fields(&mut single);
        normalize_release_fields(&mut album);

        assert_eq!(single.album.as_deref(), Some("Farangis"));
        assert_eq!(album.album.as_deref(), Some("Thriller"));
    }

    #[test]
    fn release_type_not_album_text_controls_single_normalization() {
        let mut named_single = candidate("musicbrainz", Some("Single Ladies"));
        named_single.release_type = Some("album".into());
        let mut credited_ep = candidate("musicbrainz", Some("Moorche (feat. Mehrad Hidden) - EP"));
        credited_ep.release_type = Some("ep".into());

        normalize_release_fields(&mut named_single);
        normalize_release_fields(&mut credited_ep);

        assert_eq!(named_single.album.as_deref(), Some("Single Ladies"));
        assert_eq!(credited_ep.album.as_deref(), Some("Moorche"));
    }

    #[test]
    fn agreeing_catalog_release_type_turns_a_selected_deezer_release_into_single() {
        let mut selected = candidate("deezer", Some("Moorche (feat. Mehrad Hidden)"));
        selected.title = "Moorche".into();
        selected.artist = "Sepehr Khalse feat. Mehrad Hidden".into();
        let mut itunes = selected.clone();
        itunes.provider = "itunes".into();
        itunes.album = Some("Moorche (feat. Mehrad Hidden) - Single".into());
        itunes.release_type = Some("single".into());
        itunes.track_total = Some(1);

        complete(&mut selected, &[itunes], None);

        assert_eq!(selected.release_type.as_deref(), Some("single"));
        assert_eq!(selected.album.as_deref(), Some("Moorche"));
    }

    #[test]
    fn singles_without_a_usable_album_take_the_song_title() {
        let mut missing = candidate("deezer", None);
        missing.title = "Lose U".into();
        missing.release_type = Some("single".into());
        normalize_release_fields(&mut missing);
        assert_eq!(missing.album.as_deref(), Some("Lose U"));

        let mut generic = candidate("deezer", Some("Single"));
        generic.title = "Lose U (feat. Someone)".into();
        normalize_release_fields(&mut generic);
        assert_eq!(generic.album.as_deref(), Some("Lose U"));

        let mut store_suffix = candidate("itunes", Some("Houdini - Single"));
        store_suffix.track_total = Some(1);
        normalize_release_fields(&mut store_suffix);
        assert_eq!(store_suffix.album.as_deref(), Some("Houdini"));
        assert_eq!(store_suffix.release_type.as_deref(), Some("single"));
    }
}
