//! Match keys for names: different spellings of the same name get the same key.
//! Keys are only compared and stored for lookups; they are never written to
//! tags or shown to people (the original spelling, ZWNJ included, is kept).

use unicode_normalization::UnicodeNormalization;

/// The key two spellings share when they are the same name:
/// - Unicode compatibility forms folded (NFKC), accents removed (`Beyoncé` = `Beyonce`);
/// - Arabic and Persian letters unified (`ي`/`ی`, `ك`/`ک`, `ة`/`ه`, alef and hamza forms),
///   tatweel, harakat, ZWNJ/ZWJ and direction marks removed, Persian and Arabic digits as 0-9;
/// - lowercase, punctuation as spaces (apostrophes and dots dropped), `&` = `and`,
///   a leading `the` dropped, spaces collapsed.
pub fn match_key(value: &str) -> String {
    let folded: String = value
        .nfkc()
        .collect::<String>()
        .nfd()
        .filter(|character| !is_ignorable(*character))
        .map(unify_letter)
        .collect::<String>()
        .nfc()
        .flat_map(char::to_lowercase)
        .collect();
    let mut words = Vec::new();
    let mut word = String::new();
    for character in folded.chars() {
        match character {
            '\'' | '’' | '‘' | '`' | '´' | '.' => {}
            '&' => {
                push_word(&mut words, &mut word);
                words.push("and".to_owned());
            }
            character if character.is_alphanumeric() => word.push(character),
            _ => push_word(&mut words, &mut word),
        }
    }
    push_word(&mut words, &mut word);
    if words.len() > 1 && words[0] == "the" {
        words.remove(0);
    }
    words.join(" ")
}

/// A looser key for Latin transliterations of the same name (`Yeganeh` =
/// `Yegane`, `Googoosh` = `Gugush`, `Chavoshi` = `Chavooshi`). `None` for names
/// that are not Latin script or too short to compare this loosely.
pub fn latin_key(value: &str) -> Option<String> {
    let key = match_key(value);
    if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == ' ') {
        return None;
    }
    let words = key
        .split(' ')
        .map(fold_transliteration)
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>();
    let folded = words.join(" ");
    (folded.chars().filter(char::is_ascii_alphabetic).count() >= 5).then_some(folded)
}

/// A key for titles written in different Latin spellings of the same Persian words:
/// "Gheseh Eshgh" = "Ghesseyeh Eshgh" = "Ghesse-ye Eshgh". On top of [`latin_key`], the
/// ezafe ("-ye", "-yeh", "-e") that links a word to the next one is dropped. Only words
/// followed by another word carry an ezafe, so a last word ("Sweetie") is kept whole.
/// `None` when the title is not Latin script or too short to compare this loosely.
pub fn loose_title_key(value: &str) -> Option<String> {
    let key = latin_key(value)?;
    // "Ghesse-ye" or "Ghese-e": the ezafe was written as its own word.
    let words = key
        .split(' ')
        .filter(|word| !matches!(*word, "ie" | "e"))
        .collect::<Vec<_>>();
    let last = words.len().saturating_sub(1);
    let words = words
        .iter()
        .enumerate()
        .map(|(index, word)| match word.strip_suffix("ie") {
            Some(stem) if index < last && stem.len() >= 3 => stem,
            _ => word,
        })
        .collect::<Vec<_>>();
    Some(words.join(" "))
}

/// Two titles that are the same Persian words in different Latin spellings
/// (see [`loose_title_key`]).
pub fn same_loose_title(left: &str, right: &str) -> bool {
    loose_title_key(left).is_some_and(|key| Some(key) == loose_title_key(right))
}

/// Two album names are the same album: the same name, or the same Persian words in another
/// Latin spelling ("Khalij" = "Khaleej"), or one with a store's " - Single" / " - EP" ending.
/// Editions stay apart: "Khalij (Remastered)" is another album.
pub fn same_album(left: &str, right: &str) -> bool {
    let key = album_key(left);
    !key.is_empty() && key == album_key(right)
}

/// Words a store puts after an album name that are not part of it: Persian labels sell
/// "Ba Shoma (With You) - Persian Music" for the album "Ba Shoma (With You)".
const STORE_LABELS: [&str; 1] = [" - persian music"];

/// An album name without a store's label ending (see [`STORE_LABELS`]). The one place that
/// knows these endings: the name written to a file and every album comparison use it.
pub fn without_store_label(value: &str) -> &str {
    let trimmed = value.trim();
    // ASCII lowercasing keeps byte positions, so the ending can be cut from `trimmed`.
    let lower = trimmed.to_ascii_lowercase();
    STORE_LABELS
        .iter()
        .find(|ending| lower.len() > ending.len() && lower.ends_with(*ending))
        .map_or(trimmed, |ending| {
            trimmed[..trimmed.len() - ending.len()].trim_end()
        })
}

/// One key for every spelling of an album name (see [`same_album`]), to group songs by.
pub fn album_key(value: &str) -> String {
    let trimmed = without_store_label(value);
    // ASCII lowercasing keeps byte positions, so the ending can be cut from `trimmed`.
    let lower = trimmed.to_ascii_lowercase();
    let plain = [" - single", " - ep", " (single)", " (ep)"]
        .iter()
        .find(|ending| lower.ends_with(*ending))
        .map_or(trimmed, |ending| &trimmed[..trimmed.len() - ending.len()]);
    loose_title_key(plain).unwrap_or_else(|| match_key(plain))
}

fn fold_transliteration(word: &str) -> String {
    let mut word = word.to_owned();
    for (from, to) in [
        // Persian "و" is written o, oo, ou or u.
        ("ou", "u"),
        ("oo", "u"),
        ("o", "u"),
        ("ee", "i"),
        ("ie", "i"),
        ("aa", "a"),
        ("gh", "q"),
        ("kh", "x"),
        ("w", "v"),
        ("y", "i"),
    ] {
        word = word.replace(from, to);
    }
    let mut collapsed = String::with_capacity(word.len());
    for character in word.chars() {
        if !collapsed.ends_with(character) {
            collapsed.push(character);
        }
    }
    // A final "h" after a vowel is not pronounced: Yeganeh = Yegane.
    if collapsed.len() > 2
        && collapsed.ends_with('h')
        && collapsed[..collapsed.len() - 1].ends_with(['a', 'e', 'i', 'o', 'u'])
    {
        collapsed.pop();
    }
    collapsed
}

fn push_word(words: &mut Vec<String>, word: &mut String) {
    if !word.is_empty() {
        words.push(std::mem::take(word));
    }
}

/// Marks that do not change which name it is.
fn is_ignorable(character: char) -> bool {
    matches!(
        character,
        // Latin combining accents (after NFD)
        '\u{0300}'..='\u{036F}'
        // Arabic harakat, hamza and madda marks, superscript alef, tatweel
        | '\u{064B}'..='\u{065F}'
        | '\u{0670}'
        | '\u{0640}'
        // zero-width space, ZWNJ, ZWJ, LRM, RLM, bidi embeddings and isolates, BOM
        | '\u{200B}'..='\u{200F}'
        | '\u{202A}'..='\u{202E}'
        | '\u{2066}'..='\u{2069}'
        | '\u{FEFF}'
    )
}

fn unify_letter(character: char) -> char {
    match character {
        'ي' | 'ى' | 'ئ' | 'ې' | 'ۍ' => 'ی',
        'ك' | 'ڪ' => 'ک',
        'ة' | 'ۀ' | 'ە' | 'ہ' | 'ھ' | 'ۂ' => 'ه',
        'أ' | 'إ' | 'آ' | 'ٱ' | 'ٲ' | 'ٳ' => 'ا',
        'ؤ' => 'و',
        '٠'..='٩' => char::from(b'0' + (character as u32 - '٠' as u32) as u8),
        '۰'..='۹' => char::from(b'0' + (character as u32 - '۰' as u32) as u8),
        other => other,
    }
}

/// True when the text has Arabic-script letters (Persian, Arabic, Kurdish…).
pub fn has_arabic_script(value: &str) -> bool {
    value.chars().any(|character| {
        matches!(character, '\u{0600}'..='\u{06FF}' | '\u{0750}'..='\u{077F}' | '\u{FB50}'..='\u{FDFF}' | '\u{FE70}'..='\u{FEFF}')
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_album_in_several_spellings() {
        assert!(same_album("Khalij", "Khaleej"));
        assert!(same_album("Khaleej - Single", "Khalij"));
        // A store's label after the name is not part of the album.
        assert!(same_album(
            "Ba Shoma (With You) - Persian Music",
            "Ba Shoma (With You)"
        ));
        assert_eq!(
            without_store_label("Ba Shoma (With You) - Persian Music"),
            "Ba Shoma (With You)"
        );
        // An album that is only called that keeps its name.
        assert_eq!(without_store_label("Persian Music"), "Persian Music");
        assert!(same_album(
            "Apple Music Live: Billie Eilish",
            "apple music live billie eilish"
        ));
        assert!(!same_album("Khalij", "Khalij (Remastered)"));
        assert!(!same_album("Khalij", "Lalehzaar"));
        assert!(!same_album("", ""));
    }

    #[test]
    fn persian_spellings_of_one_name_share_a_key() {
        // Arabic yeh/kaf vs Persian, ZWNJ vs none vs space-less, harakat.
        assert_eq!(match_key("علي"), match_key("علی"));
        assert_eq!(match_key("كوروش"), match_key("کوروش"));
        assert_eq!(match_key("می‌خواهم"), match_key("میخواهم"));
        assert_eq!(match_key("مُحسن"), match_key("محسن"));
        assert_eq!(match_key("آرش"), match_key("ارش"));
        assert_eq!(match_key("ترانهٔ ۲"), match_key("ترانه 2"));
        assert_ne!(match_key("ابی"), match_key("شادمهر"));
    }

    #[test]
    fn latin_spellings_of_one_name_share_a_key() {
        assert_eq!(match_key("Beyoncé"), match_key("BEYONCE"));
        assert_eq!(match_key("Guns N' Roses"), match_key("guns n roses"));
        assert_eq!(
            match_key("Simon & Garfunkel"),
            match_key("Simon and Garfunkel")
        );
        assert_eq!(match_key("The Beatles"), match_key("Beatles"));
        assert_eq!(match_key("  Mohsen   Yeganeh "), "mohsen yeganeh");
        // "The The" keeps its words.
        assert_eq!(match_key("The The"), "the");
        assert_ne!(match_key("Ebi"), match_key("Ebi & Shadmehr"));
    }

    #[test]
    fn transliterations_share_a_loose_key() {
        assert_eq!(latin_key("Mohsen Yeganeh"), latin_key("Mohsen Yegane"));
        assert_eq!(latin_key("Googoosh"), latin_key("Gugush"));
        assert_eq!(latin_key("Mohsen Chavoshi"), latin_key("Mohsen Chavooshi"));
        assert_eq!(latin_key("Shadmehr Aghili"), latin_key("Shadmehr Aqili"));
        assert_ne!(latin_key("Moein"), latin_key("Mohsen"));
        // Too short or not Latin: no loose key.
        assert_eq!(latin_key("Ebi"), None);
        assert_eq!(latin_key("گوگوش"), None);
    }

    #[test]
    fn persian_titles_in_other_latin_spellings_share_a_loose_key() {
        // MusicBrainz and Apple Music spell Ebi's "قصه عشق" differently.
        assert!(same_loose_title("Gheseh Eshgh", "Ghesseyeh Eshgh"));
        assert!(same_loose_title("Gheseh Eshgh", "Ghesse-ye Eshgh"));
        assert!(same_loose_title("Ghesseh-e Eshgh", "Gheseh Eshgh"));
        assert!(same_loose_title("Koohe Yakh", "Kouhe Yakh"));
        // Different words stay different, and a last word keeps its ending.
        assert!(!same_loose_title("Gheseh Eshgh", "Gheseh Gham"));
        assert!(!same_loose_title("Sweetie", "Sweet"));
        // Persian script has no loose key.
        assert!(!same_loose_title("قصه عشق", "قصه عشق"));
    }

    #[test]
    fn arabic_script_is_detected() {
        assert!(has_arabic_script("گوگوش"));
        assert!(has_arabic_script("Googoosh گوگوش"));
        assert!(!has_arabic_script("Googoosh"));
    }
}
