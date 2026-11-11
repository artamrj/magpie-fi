use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ArtistCredit {
    pub name: String,
    #[serde(default)]
    pub join_phrase: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub musicbrainz_id: Option<String>,
    /// The artist identity (`artists.id`) this credit was resolved to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artist_id: Option<i64>,
}

impl ArtistCredit {
    pub fn new(name: impl Into<String>, join_phrase: impl Into<String>) -> Self {
        Self {
            name: clean_text(&name.into()),
            join_phrase: join_phrase.into(),
            musicbrainz_id: None,
            artist_id: None,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Credits {
    pub artist: String,
    pub title: String,
    pub artists: Vec<ArtistCredit>,
}

pub fn clean_text(value: &str) -> String {
    value
        .nfc()
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Two spellings with the same key are the same name (see [`super::names::match_key`]).
pub fn identity_key(value: &str) -> String {
    super::names::match_key(value)
}

pub fn prefer_latin_alias(value: &str) -> String {
    let value = clean_text(value);
    let has_latin = value
        .chars()
        .any(|character| character.is_ascii_alphabetic());
    let has_arabic = value.chars().any(is_arabic_script);
    if !has_latin || !has_arabic {
        return value;
    }
    let without_arabic = value
        .chars()
        .map(|character| {
            if is_arabic_script(character) {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    without_arabic
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_matches(|character: char| {
            character.is_whitespace() || matches!(character, '&' | ',' | ';' | '-' | '–' | '—')
        })
        .trim()
        .to_owned()
}

fn is_arabic_script(character: char) -> bool {
    matches!(
        character,
        '\u{0600}'..='\u{06ff}'
            | '\u{0750}'..='\u{077f}'
            | '\u{08a0}'..='\u{08ff}'
            | '\u{fb50}'..='\u{fdff}'
            | '\u{fe70}'..='\u{feff}'
    )
}

/// Normalize a display credit while retaining the individual artist identities.
/// Only separators recognized by Navidrome are parsed. In particular, `&`,
/// `and`, `x`, commas, and bare slashes are deliberately left inside a name.
pub fn normalize_featured(artist: &str, title: &str) -> Credits {
    let artist = clean_text(artist);
    let (title, featured) = remove_feature_clause(title);
    let mut artists = parse_supported_credit(&artist);

    if let Some(featured) = featured {
        let featured = parse_supported_credit(&featured);
        for credit in featured {
            if artists
                .iter()
                .any(|existing| same_name(&existing.name, &credit.name))
            {
                continue;
            }
            if let Some(last) = artists.last_mut() {
                last.join_phrase = " feat. ".into();
            }
            artists.push(ArtistCredit::new(credit.name, ""));
        }
    }
    if artists.is_empty() && !artist.is_empty() {
        artists.push(ArtistCredit::new(&artist, ""));
    }
    let artist = display_artist(&artists);
    Credits {
        artist,
        title: clean_text(&title),
        artists,
    }
}

pub fn normalize_structured(display: &str, title: &str, mut artists: Vec<ArtistCredit>) -> Credits {
    for credit in &mut artists {
        credit.name = prefer_latin_alias(&credit.name);
    }
    artists.retain(|credit| !credit.name.is_empty());
    // Only the display text: read it the plain way. The featured singers in the title must
    // join the display artist, not replace it ("Amir Tataloo" + "… (feat. Sohrab MJ &
    // Orchid)" used to become "Sohrab MJ & Orchid").
    if artists.is_empty() {
        return normalize_featured(display, title);
    }
    apply_display_joins(display, &mut artists);
    for credit in &mut artists {
        credit.join_phrase = normalize_join_phrase(&credit.join_phrase);
    }
    dedupe_credits(&mut artists);
    if artists.len() > 1 && artists.iter().all(|credit| credit.join_phrase.is_empty()) {
        let last = artists.len() - 1;
        for credit in &mut artists[..last] {
            credit.join_phrase = " / ".into();
        }
    }

    let (title, featured) = remove_feature_clause(title);
    if let Some(featured) = featured {
        for featured in parse_supported_credit(&featured) {
            if artists
                .iter()
                .any(|existing| same_name(&existing.name, &featured.name))
            {
                continue;
            }
            if let Some(last) = artists.last_mut() {
                last.join_phrase = " feat. ".into();
            }
            artists.push(ArtistCredit::new(featured.name, ""));
        }
    }
    Credits {
        artist: display_artist(&artists),
        title: clean_text(&title),
        artists,
    }
}

fn apply_display_joins(display: &str, artists: &mut [ArtistCredit]) {
    if artists.len() < 2 {
        return;
    }
    let display = clean_text(display);
    let Some(mut cursor) = display.find(&artists[0].name) else {
        return;
    };
    cursor += artists[0].name.len();
    let mut joins = Vec::with_capacity(artists.len() - 1);
    for next in &artists[1..] {
        let Some(relative) = display[cursor..].find(&next.name) else {
            return;
        };
        joins.push(display[cursor..cursor + relative].to_owned());
        cursor += relative + next.name.len();
    }
    if !display[cursor..].trim().is_empty() || joins.iter().any(|join| join.trim().is_empty()) {
        return;
    }
    for (credit, join) in artists.iter_mut().zip(joins) {
        credit.join_phrase = join;
    }
    if let Some(last) = artists.last_mut() {
        last.join_phrase.clear();
    }
}

pub fn display_artist(artists: &[ArtistCredit]) -> String {
    artists
        .iter()
        .map(|credit| format!("{}{}", clean_text(&credit.name), credit.join_phrase))
        .collect::<String>()
        .trim()
        .to_owned()
}

pub fn individual_names(artists: &[ArtistCredit]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for credit in artists {
        let name = clean_text(&credit.name);
        if !name.is_empty() && !names.iter().any(|other| same_name(other, &name)) {
            names.push(name);
        }
    }
    names
}

/// Remove performer credits from a release title without changing legitimate
/// title/version text. Release-type policy (for example, mapping a confirmed
/// standalone single to `Single`) is handled by metadata completion.
pub fn release_title_without_featured(value: &str) -> String {
    remove_feature_clause(value).0
}

/// The main singer a credit names: the first name, before any `feat.` and before the loose
/// separators sources use between co-singers ("Ebi & Shadmehr", "Sasy x Tataloo").
pub fn main_name(artist: &str, title: &str) -> Option<String> {
    let credits = normalize_featured(artist, title);
    let first = credits.artists.first()?.name.clone();
    let first = loose_first_name(&first);
    (!first.is_empty()).then_some(first)
}

/// Separators sources use between co-singers inside one credit.
const LOOSE_SEPARATORS: [&str; 9] = [
    ", ", "، ", " & ", " x ", " and ", " و ", " vs ", " vs. ", " with ",
];

/// The earliest loose separator in `value`: (byte index, separator length).
fn next_loose_separator(value: &str) -> Option<(usize, usize)> {
    // ASCII lowercasing keeps byte positions, so the index is valid in `value` too.
    let lower = value.to_ascii_lowercase();
    LOOSE_SEPARATORS
        .iter()
        .filter_map(|separator| lower.find(separator).map(|index| (index, separator.len())))
        .min_by_key(|(index, _)| *index)
}

fn loose_first_name(value: &str) -> String {
    let end = next_loose_separator(value).map_or(value.len(), |(index, _)| index);
    clean_text(&value[..end])
}

/// Every person in one credit, split at the loose separators too:
/// "Sohrab MJ & Orchid" → Sohrab MJ, Orchid.
pub fn loose_names(value: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = value;
    while let Some((index, length)) = next_loose_separator(rest) {
        names.push(clean_text(&rest[..index]));
        rest = &rest[index + length..];
    }
    names.push(clean_text(rest));
    names.retain(|name| !name.is_empty());
    names
}

/// `short` is `full` with its first word(s) left off: the stage name or surname alone.
/// Catalogs credit "Tataloo" for "Amir Tataloo" and "MJ" for "Sohrab MJ". Only the end of
/// the name counts: a first name alone ("Amir", "Sohrab") is shared by too many people.
pub fn is_short_form(short: &str, full: &str) -> bool {
    let (short, full) = (identity_key(short), identity_key(full));
    short.chars().filter(|c| c.is_alphanumeric()).count() >= 2
        && full.len() > short.len()
        && full.ends_with(&format!(" {short}"))
}

/// Two names in the credits of one song are the same person: the same name, or one is the
/// other's short form ([`is_short_form`]).
pub fn same_person(left: &str, right: &str) -> bool {
    same_name(left, right) || is_short_form(left, right) || is_short_form(right, left)
}

/// The match's credit told the way the file tells it, when both name exactly the same
/// people. Sources list the same singers in another order or spell them shorter: Shazam
/// gives "MJ feat. Tataloo & Orchid" and Deezer "Amir Tataloo & MJ & Orchid" for a file
/// tagged "Amir Tataloo feat. Sohrab MJ & Orchid". The file's order wins (its first name
/// is the main singer, the others are featured) and the fuller spelling of each name wins.
/// When the order already agrees, only the spellings change and the source's joins stay.
/// A credit with someone the file doesn't name, or without someone it does, is left alone.
/// Returns whether anything changed.
pub fn align_with_file(
    artists: Vec<ArtistCredit>,
    file_artist: &str,
    file_title: &str,
) -> (Vec<ArtistCredit>, bool) {
    let file_people = individual_names(&normalize_featured(file_artist, file_title).artists)
        .iter()
        .flat_map(|name| loose_names(name))
        .collect::<Vec<_>>();
    let match_people = artists
        .iter()
        .flat_map(|credit| {
            let names = loose_names(&credit.name);
            // A MusicBrainz ID belongs to the whole credit: keep it only for one person.
            let id = (names.len() == 1)
                .then(|| credit.musicbrainz_id.clone())
                .flatten();
            names.into_iter().map(move |name| (name, id.clone()))
        })
        .collect::<Vec<_>>();
    if file_people.is_empty() || file_people.len() != match_people.len() {
        return (artists, false);
    }
    // Which source name is which file name; each source name is used once.
    let mut order = Vec::with_capacity(file_people.len());
    for person in &file_people {
        let Some(index) = (0..match_people.len())
            .find(|index| !order.contains(index) && same_person(&match_people[*index].0, person))
        else {
            return (artists, false);
        };
        order.push(index);
    }
    let fuller = |source: &str, file: &str| {
        if is_short_form(source, file) {
            file.to_owned()
        } else {
            source.to_owned()
        }
    };
    let reordered = order.iter().enumerate().any(|(at, index)| at != *index);
    if !reordered {
        let respelled = file_people
            .iter()
            .zip(&match_people)
            .any(|(file, (source, _))| is_short_form(source, file));
        if !respelled {
            return (artists, false);
        }
        let mut people = file_people.iter();
        let artists = artists
            .into_iter()
            .map(|credit| ArtistCredit {
                name: respell_loose(&credit.name, |source| {
                    people
                        .next()
                        .map_or_else(|| source.to_owned(), |file| fuller(source, file))
                }),
                ..credit
            })
            .collect();
        return (artists, true);
    }
    let count = order.len();
    let artists = file_people
        .iter()
        .zip(&order)
        .enumerate()
        .map(|(at, (file, index))| {
            let (source, id) = &match_people[*index];
            let join = match (at, count - at) {
                (_, 1) => "",
                (0, _) => " feat. ",
                _ => " & ",
            };
            ArtistCredit {
                musicbrainz_id: id.clone(),
                ..ArtistCredit::new(fuller(source, file), join)
            }
        })
        .collect();
    (artists, true)
}

/// `value` with every loose name passed through `respell`, its separators kept.
fn respell_loose(value: &str, mut respell: impl FnMut(&str) -> String) -> String {
    let mut out = String::new();
    let mut rest = value;
    while let Some((index, length)) = next_loose_separator(rest) {
        out.push_str(&respell(&clean_text(&rest[..index])));
        out.push_str(&rest[index..index + length]);
        rest = &rest[index + length..];
    }
    out.push_str(&respell(&clean_text(rest)));
    clean_text(&out)
}

/// Puts the file's main singer back when a source credits only the featured singers
/// ("Sohrab MJ & Orchid" for a file tagged "Amir Tataloo feat. Sohrab MJ & Orchid"). Only
/// when every name the source gives is also in the file, so a match by someone else, or the
/// same singers written in another script, is left alone. Returns whether anything changed.
pub fn restore_missing_main(
    mut artists: Vec<ArtistCredit>,
    main: &str,
    file_artist: &str,
    file_title: &str,
) -> (Vec<ArtistCredit>, bool) {
    let match_names = artists
        .iter()
        .flat_map(|credit| loose_names(&credit.name))
        .collect::<Vec<_>>();
    if match_names.is_empty() || match_names.iter().any(|name| same_person(name, main)) {
        return (artists, false);
    }
    let file_names = normalize_featured(file_artist, file_title)
        .artists
        .iter()
        .flat_map(|credit| loose_names(&credit.name))
        .filter(|name| !same_name(name, main))
        .collect::<Vec<_>>();
    let all_in_file = match_names
        .iter()
        .all(|name| file_names.iter().any(|other| same_person(other, name)));
    if !all_in_file {
        return (artists, false);
    }
    artists.insert(0, ArtistCredit::new(main, " feat. "));
    if let Some(last) = artists.last_mut() {
        last.join_phrase.clear();
    }
    (artists, true)
}

/// Puts the song's main singer first when a source listed them after a featured artist
/// ("Ferap feat. Tqvis" for a file tagged "Tqvis & Ferap"). The file's own tags say who is
/// main; the others become featured. Returns whether anything moved.
pub fn keep_main_first(mut artists: Vec<ArtistCredit>, main: &str) -> (Vec<ArtistCredit>, bool) {
    let is_main = |credit: &ArtistCredit| {
        same_person(&credit.name, main) || same_person(&loose_first_name(&credit.name), main)
    };
    let Some(index) = artists.iter().position(is_main) else {
        return (artists, false);
    };
    if index == 0 {
        return (artists, false);
    }
    let mut primary = artists.remove(index);
    if is_short_form(&primary.name, main) {
        primary.name = main.to_owned();
    }
    primary.join_phrase = " feat. ".into();
    artists.insert(0, primary);
    if let Some(last) = artists.last_mut() {
        last.join_phrase.clear();
    }
    (artists, true)
}

fn dedupe_credits(artists: &mut Vec<ArtistCredit>) {
    let mut out: Vec<ArtistCredit> = Vec::new();
    for credit in artists.drain(..) {
        if let Some(existing) = out
            .iter_mut()
            .find(|existing| same_name(&existing.name, &credit.name))
        {
            if existing.musicbrainz_id.is_none() {
                existing.musicbrainz_id = credit.musicbrainz_id;
            }
            continue;
        }
        out.push(credit);
    }
    if let Some(last) = out.last_mut() {
        last.join_phrase.clear();
    }
    *artists = out;
}

fn parse_supported_credit(value: &str) -> Vec<ArtistCredit> {
    let mut rest = clean_text(value);
    let mut out = Vec::new();
    while !rest.is_empty() {
        let lower = rest.to_lowercase();
        let next = [" / ", " feat. ", " feat ", " ft. ", " ft ", "; "]
            .into_iter()
            .filter_map(|separator| lower.find(separator).map(|index| (index, separator)))
            .min_by_key(|(index, _)| *index);
        let Some((index, separator)) = next else {
            out.push(ArtistCredit::new(rest, ""));
            break;
        };
        let name = rest[..index].to_owned();
        let join = normalize_join_phrase(separator);
        out.push(ArtistCredit::new(name, join));
        rest = rest[index + separator.len()..].trim().to_owned();
    }
    out.retain(|credit| !credit.name.is_empty());
    out
}

fn normalize_join_phrase(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "feat" | "feat." | "ft" | "ft." | "featuring" => " feat. ".into(),
        "/" => " / ".into(),
        ";" => "; ".into(),
        _ => value.to_owned(),
    }
}

fn remove_feature_clause(title: &str) -> (String, Option<String>) {
    let title = clean_text(title);
    let lower = title.to_lowercase();
    for marker in ["(feat. ", "(feat ", "(ft. ", "(ft ", "(featuring "] {
        if let Some(start) = lower.find(marker)
            && let Some(relative_close) = title[start + marker.len()..].find(')')
        {
            let close = start + marker.len() + relative_close;
            let featured = clean_text(&title[start + marker.len()..close]);
            let cleaned = clean_text(&format!("{} {}", &title[..start], &title[close + 1..]));
            let (cleaned, _) = remove_feature_clause(&cleaned);
            return (cleaned, (!featured.is_empty()).then_some(featured));
        }
    }
    for marker in [" feat. ", " feat ", " ft. ", " ft ", " featuring "] {
        if let Some(start) = lower.rfind(marker) {
            let featured = clean_text(&title[start + marker.len()..]);
            if !featured.is_empty() {
                return (clean_text(&title[..start]), Some(featured));
            }
        }
    }
    (title, None)
}

fn same_name(left: &str, right: &str) -> bool {
    identity_key(left) == identity_key(right)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_files_main_singer_goes_first_whatever_a_source_said() {
        assert_eq!(
            main_name("Tqvis & Ferap", "Donyaye Movazi").as_deref(),
            Some("Tqvis")
        );
        assert_eq!(
            main_name("Arta", "Mi Amor (ft. Saaren)").as_deref(),
            Some("Arta")
        );
        let source = normalize_featured("Ferap", "Donyaye Movazi (feat. Tqvis)").artists;
        let (fixed, moved) = keep_main_first(source, "Tqvis");
        assert!(moved);
        assert_eq!(display_artist(&fixed), "Tqvis feat. Ferap");
        // Already first, or not credited at all: nothing moves.
        let same = normalize_featured("Tqvis feat. Ferap", "").artists;
        assert!(!keep_main_first(same, "Tqvis").1);
        let other = normalize_featured("Maria", "").artists;
        assert!(!keep_main_first(other, "Tqvis").1);
    }

    #[test]
    fn a_main_singer_the_source_left_out_is_put_back() {
        let file = "Amir Tataloo feat. Sohrab MJ & Orchid";
        let source = normalize_featured("Sohrab MJ & Orchid", "Male Man Bash").artists;
        let (fixed, restored) = restore_missing_main(source, "Amir Tataloo", file, "Male Man Bash");
        assert!(restored);
        assert_eq!(
            display_artist(&fixed),
            "Amir Tataloo feat. Sohrab MJ & Orchid"
        );
        // The main singer is already there: nothing is added twice.
        let source = normalize_featured("Sohrab MJ & Amir Tataloo", "").artists;
        assert!(!restore_missing_main(source, "Amir Tataloo", file, "Male Man Bash").1);
        // A name the file doesn't have means another credit or another script: left alone.
        let source = normalize_featured("Sohrab MJ & Someone Else", "").artists;
        assert!(!restore_missing_main(source, "Amir Tataloo", file, "Male Man Bash").1);
        let source = normalize_featured("Amir Tataloo feat. Sohrab MJ", "").artists;
        assert!(!restore_missing_main(source, "امیر تتلو", "امیر تتلو feat. Sohrab MJ", "").1);
        assert_eq!(loose_names("Sohrab MJ & Orchid"), ["Sohrab MJ", "Orchid"]);
    }

    #[test]
    fn featured_singers_in_the_title_join_the_main_singer() {
        // Apple Music gives a plain artist and the featured singers in the title.
        let credits = normalize_structured(
            "Amir Tataloo",
            "Male Man Bash (feat. Sohrab MJ & Orchid)",
            Vec::new(),
        );
        assert_eq!(credits.artist, "Amir Tataloo feat. Sohrab MJ & Orchid");
        assert_eq!(credits.title, "Male Man Bash");
    }

    #[test]
    fn a_stage_name_alone_is_the_same_person() {
        assert!(same_person("Tataloo", "Amir Tataloo"));
        assert!(same_person("Sohrab MJ", "MJ"));
        assert!(same_person("تتلو", "امیر تتلو"));
        // A first name alone is not enough, nor is a different surname.
        assert!(!same_person("Amir", "Amir Tataloo"));
        assert!(!same_person("Sohrab MJ", "Sohrab Pakzad"));
        assert!(!same_person("Taloo", "Amir Tataloo"));
    }

    #[test]
    fn the_same_singers_are_told_the_way_the_file_tells_them() {
        let file = "Amir Tataloo feat. Sohrab MJ & Orchid";
        let aligned = |artist: &str, title: &str| {
            let source = normalize_structured(artist, title, Vec::new()).artists;
            let (artists, changed) = align_with_file(source, file, "Male Man Bash");
            (display_artist(&artists), changed)
        };
        // Shazam: a featured singer first and shorter spellings.
        assert_eq!(
            aligned("MJ feat. Tataloo & Orchid", "Male Man Bash"),
            (file.into(), true)
        );
        assert_eq!(
            aligned("MJ", "Male Man Bash (feat. Tataloo & Orchid)"),
            (file.into(), true)
        );
        // Deezer: the right order, "MJ" for "Sohrab MJ"; its joins stay.
        assert_eq!(
            aligned("Amir Tataloo & MJ & Orchid", "Male Man Bash"),
            ("Amir Tataloo & Sohrab MJ & Orchid".into(), true)
        );
        // Apple Music and SoundCloud already agree.
        assert_eq!(
            aligned("Amir Tataloo", "Male Man Bash (feat. Sohrab MJ & Orchid)"),
            (file.into(), false)
        );
        // Someone the file doesn't name, or a singer missing: left alone.
        assert_eq!(
            aligned("MJ feat. Tataloo & Someone", "Male Man Bash"),
            ("MJ feat. Tataloo & Someone".into(), false)
        );
        assert_eq!(
            aligned("Sohrab Pakzad", "Male Man Bash"),
            ("Sohrab Pakzad".into(), false)
        );
        // A band named with "&" in the same order is not split.
        let source = normalize_featured("Simon & Garfunkel", "").artists;
        let (artists, changed) = align_with_file(source, "Simon and Garfunkel", "");
        assert!(!changed);
        assert_eq!(display_artist(&artists), "Simon & Garfunkel");
    }

    #[test]
    fn moves_featured_artist_from_title_to_artist_credit() {
        let credits = normalize_featured("Arta", "Mi Amor (ft. Saaren)");
        assert_eq!(credits.title, "Mi Amor");
        assert_eq!(credits.artist, "Arta feat. Saaren");
        assert_eq!(individual_names(&credits.artists), ["Arta", "Saaren"]);
    }

    #[test]
    fn supported_navidrome_separators_are_parsed() {
        for artist in [
            "Alice / Bob",
            "Alice feat. Bob",
            "Alice feat Bob",
            "Alice ft. Bob",
            "Alice ft Bob",
            "Alice; Bob",
        ] {
            assert_eq!(
                individual_names(&normalize_featured(artist, "Song").artists),
                ["Alice", "Bob"],
                "{artist}"
            );
        }
    }

    #[test]
    fn unsupported_ambiguous_separators_are_not_parsed() {
        for artist in [
            "Simon & Garfunkel",
            "Earth, Wind & Fire",
            "A and B",
            "A x B",
            "AC/DC",
        ] {
            assert_eq!(
                normalize_featured(artist, "Song").artists.len(),
                1,
                "{artist}"
            );
        }
    }

    #[test]
    fn preserves_structured_co_primary_join() {
        let credits = normalize_structured(
            "Alice & Bob",
            "Song",
            vec![
                ArtistCredit::new("Alice", " & "),
                ArtistCredit::new("Bob", ""),
            ],
        );
        assert_eq!(credits.artist, "Alice & Bob");
        assert_eq!(individual_names(&credits.artists), ["Alice", "Bob"]);
    }

    #[test]
    fn bilingual_alias_prefers_the_latin_name() {
        assert_eq!(prefer_latin_alias("Hayedeh هايده"), "Hayedeh");
        assert_eq!(prefer_latin_alias("هایده Hayedeh"), "Hayedeh");
        assert_eq!(prefer_latin_alias("Ali علی & Reza رضا"), "Ali & Reza");
    }

    #[test]
    fn persian_only_artist_is_preserved() {
        assert_eq!(prefer_latin_alias("هایده"), "هایده");
    }

    #[test]
    fn removes_a_duplicate_trailing_feature_credit() {
        let credits = normalize_featured(
            "Arta",
            "Hanooz Yadame (feat. Koorosh, Sami Low & Raha) feat. Koorosh,Sami Low,Raha",
        );
        assert_eq!(credits.title, "Hanooz Yadame");
        assert_eq!(credits.artist, "Arta feat. Koorosh, Sami Low & Raha");
    }

    #[test]
    fn unicode_and_whitespace_are_canonicalized() {
        assert_eq!(clean_text("  Cafe\u{301}   Song "), "Café Song");
        assert_eq!(identity_key(" ANDY "), identity_key("Andy"));
    }

    #[test]
    fn removes_featured_credit_from_release_title_but_keeps_version_text() {
        assert_eq!(
            release_title_without_featured("Moorche (feat. Mehrad Hidden) - EP"),
            "Moorche - EP"
        );
        assert_eq!(
            release_title_without_featured("Midnight (Live)"),
            "Midnight (Live)"
        );
    }
}
