//! Request cards: each request shown as a live Bot API 10.4 widget instead of
//! a text status message. The card's HTML (`widgets/request.html`) is fixed
//! when it is sent; after that the bot only changes its state (`chat_data`,
//! at most 1024 bytes), and the card redraws itself. Edits are silent.
//!
//! The state is compact JSON:
//! - `v` revision (the card ignores older states), `id` request ID
//! - `s` "reading", "busy", "done" or "failed"; `k` kind; `t` name; `e` error
//! - `n` [in library, total, on the way, need you, failed, removed]
//! - `g` one letter per song: w waiting, d downloading, i identifying,
//!   r needs you, n filed now, h was already there, f failed, x removed
//! - `w` the song that changed last: its letter, then "Artist – Title"
//! - `b` buttons to show: s Stop, r Retry failed, d Details
//! - `c` three colours from a filed song's verified cover

use super::texts::{self, StatusView};
use crate::acquire::store::{RequestSummary, WantedView};
use serde_json::{Value, json};
use std::path::Path;

/// The request card's code, sent once per request.
pub const REQUEST_HTML: &str = include_str!("widgets/request.html");

/// The review card's code ("which song is this?"), sent once per song in Review.
pub const REVIEW_HTML: &str = include_str!("widgets/review.html");

/// The library card's code. It is one shared widget for every owner's chat.
pub const LIBRARY_HTML: &str = include_str!("widgets/library.html");

/// Telegram's limit for a widget's state and for an answer to a tap.
pub const MAX_DATA: usize = 1024;

/// At most this many song dots; the counts stay exact beyond it.
const MAX_DOTS: usize = 200;

/// Cover colours for the card's background, as `#rrggbb`.
pub type Colours = [String; 3];

/// The first state of a new card, before the link is read.
pub fn reading(request: i64, kind: &str) -> Value {
    json!({"id": request, "s": "reading", "k": kind, "n": [0, 0, 0, 0, 0, 0]})
}

/// The card's state for a request, without its revision. Callers compare it
/// with the last state sent, and add `v` only when sending.
pub fn request_state(
    summary: &RequestSummary,
    view: &StatusView,
    songs: &[WantedView],
    colours: Option<&Colours>,
) -> Value {
    let finished = texts::is_final(summary);
    let state = match summary.status.as_str() {
        "reading" => "reading",
        "failed" => "failed",
        _ if finished => "done",
        _ => "busy",
    };
    let mut buttons = String::new();
    if !finished && summary.working > 0 && summary.total > 1 {
        buttons.push('s');
    }
    if finished && summary.failed > 0 {
        buttons.push('r');
    }
    if summary.failed > 0 || summary.review > 0 {
        buttons.push('d');
    }
    let dots: String = songs
        .iter()
        .take(MAX_DOTS)
        .map(|song| letter(&song.status))
        .collect();
    let mut card = json!({
        "id": summary.id,
        "s": state,
        "k": summary.kind,
        "t": name(summary, view),
        "n": [summary.done, summary.total, summary.working, summary.review,
              summary.failed, summary.removed],
        "g": dots,
        "b": buttons,
    });
    if let Some(song) = latest(songs).filter(|_| summary.total > 1) {
        card["w"] = json!(format!("{}{}", letter(&song.status), song_name(song)));
    }
    if let Some(error) = summary.error.as_deref().filter(|_| state == "failed") {
        card["e"] = json!(error);
    }
    if let Some(colours) = colours {
        card["c"] = json!(colours);
    }
    fit(card)
}

/// Adds the revision, keeping the state within Telegram's limit.
pub fn with_revision(mut state: Value, revision: i64) -> String {
    state["v"] = json!(revision);
    fit(state).to_string()
}

/// The card's title. Before a link is read there is no name yet, only the
/// kind, which the card shows already.
fn name(summary: &RequestSummary, view: &StatusView) -> String {
    let name = texts::request_name(summary, view);
    if summary.title.is_none() && name == summary.kind {
        String::new()
    } else {
        name
    }
}

fn letter(status: &str) -> char {
    match status {
        "downloading" => 'd',
        "identifying" => 'i',
        "review" => 'r',
        "filed" => 'n',
        "in_library" => 'h',
        "failed" => 'f',
        "removed" => 'x',
        _ => 'w',
    }
}

fn song_name(song: &WantedView) -> String {
    match &song.artist {
        Some(artist) => format!("{artist} – {}", song.title),
        None => song.title.clone(),
    }
}

/// The song being worked on now, or else the one that changed last.
fn latest(songs: &[WantedView]) -> Option<&WantedView> {
    let busy = |song: &&WantedView| matches!(song.status.as_str(), "downloading" | "identifying");
    songs
        .iter()
        .filter(busy)
        .max_by(|a, b| a.updated_at.cmp(&b.updated_at))
        .or_else(|| {
            songs
                .iter()
                .filter(|song| song.status != "pending")
                .max_by(|a, b| a.updated_at.cmp(&b.updated_at))
        })
}

/// Shortens what can be shortened until the state fits in 1024 bytes:
/// first the names, then the song dots (the counts stay exact).
fn fit(mut card: Value) -> Value {
    for limit in [80, 48, 24] {
        if card.to_string().len() <= MAX_DATA {
            return card;
        }
        for key in ["t", "w", "e"] {
            if let Some(text) = card[key].as_str() {
                card[key] = json!(shorten(text, limit));
            }
        }
    }
    while card.to_string().len() > MAX_DATA {
        let dots = card["g"].as_str().unwrap_or_default();
        if dots.is_empty() {
            card.as_object_mut().map(|card| card.remove("c"));
            break;
        }
        let keep = dots.len() / 2;
        card["g"] = json!(dots[..keep].to_owned());
    }
    card
}

fn shorten(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let mut short: String = text.chars().take(limit - 1).collect();
    short.push('…');
    short
}

/// The answer to the card's Details button: the songs with problems as
/// [song, what happened], as many as fit in 1024 bytes.
pub fn details_answer(songs: &[WantedView]) -> String {
    let rows = texts::details_rows(songs);
    let mut shown = rows.len();
    loop {
        let mut answer = json!({ "d": rows[..shown] });
        if shown == 0 {
            answer["m"] = json!(texts::EVERY_SONG_FINE);
        } else if shown < rows.len() {
            answer["m"] = json!(texts::and_more(rows.len() - shown));
        }
        let answer = answer.to_string();
        if answer.len() <= MAX_DATA || shown == 0 {
            return answer;
        }
        shown -= 1;
    }
}

// ── Review cards ────────────────────────────────────────────────────────────
//
// State keys: `id` track ID; `s` "ask", or how it ended: "filed", "cover"
// (chosen, but finish in the app), "removed", "gone" (no longer in Review),
// "app" (sorted out in the app); `f` the file as "Artist – Title"; `l` its
// length in seconds; `r` why magpie-fi is unsure; `m` the outcome's words;
// `o` up to three choices, each [candidate ID, title, artist, "album · year",
// score 0-100, length difference in seconds or null, 1 if recognised from the
// audio, "the sources that found it"].

/// One possible match offered on a review card.
#[derive(Debug, sqlx::FromRow)]
pub struct ReviewChoice {
    pub id: i64,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub year: Option<String>,
    pub score: f64,
    pub duration_delta: Option<f64>,
    pub score_breakdown: Option<String>,
}

/// What the review card says about the file itself.
#[derive(Debug, Default)]
pub struct ReviewFile {
    pub song: String,
    pub seconds: Option<f64>,
    pub reason: Option<String>,
}

pub fn review_state(track: i64, file: &ReviewFile, choices: &[ReviewChoice]) -> Value {
    let options: Vec<Value> = choices
        .iter()
        .take(3)
        .map(|choice| {
            let mut release = choice
                .album
                .clone()
                .filter(|album| !album.trim().is_empty());
            if let Some(year) = choice.year.as_deref().and_then(|year| year.get(..4)) {
                release = Some(match release {
                    Some(album) => format!("{album} · {year}"),
                    None => year.to_owned(),
                });
            }
            let support = crate::application::smart_approval::breakdown_sources(
                choice.score_breakdown.as_deref(),
            );
            let audio = crate::application::smart_approval::breakdown_has_audio_recognition(
                choice.score_breakdown.as_deref(),
            );
            json!([
                choice.id,
                choice.title,
                choice.artist,
                release.unwrap_or_default(),
                choice.score.clamp(0.0, 100.0).round() as i64,
                choice.duration_delta.map(|delta| delta.round() as i64),
                i64::from(audio),
                support
                    .iter()
                    .map(|source| texts::source_name(source))
                    .collect::<Vec<_>>()
                    .join(" · "),
            ])
        })
        .collect();
    let mut card = json!({"id": track, "s": "ask", "f": file.song, "o": options});
    if let Some(seconds) = file.seconds.filter(|seconds| *seconds > 0.0) {
        card["l"] = json!(seconds.round() as i64);
    }
    if let Some(reason) = &file.reason {
        card["r"] = json!(reason);
    }
    fit_review(card)
}

/// How a review card ended: `state` is one of the outcome states above. The
/// card shows its own icon, so a leading emoji in `words` is left out.
pub fn review_outcome(track: i64, song: &str, state: &str, words: &str) -> Value {
    let words = words.trim_start_matches(|letter: char| !letter.is_alphanumeric());
    json!({"id": track, "s": state, "f": shorten(song, 120), "m": shorten(words, 300)})
}

/// Shortens the names, then drops the last choices, until the state fits.
fn fit_review(mut card: Value) -> Value {
    for limit in [80, 48, 28] {
        if with_revision(card.clone(), i64::MAX).len() <= MAX_DATA {
            return card;
        }
        for key in ["f", "r"] {
            if let Some(text) = card[key].as_str() {
                card[key] = json!(shorten(text, limit));
            }
        }
        for option in card["o"].as_array_mut().into_iter().flatten() {
            for index in [1, 2, 3, 7] {
                if let Some(text) = option[index].as_str() {
                    option[index] = json!(shorten(text, limit));
                }
            }
        }
    }
    while with_revision(card.clone(), i64::MAX).len() > MAX_DATA {
        match card["o"].as_array_mut() {
            Some(options) if !options.is_empty() => {
                options.pop();
            }
            _ => break,
        }
    }
    card
}

// ── The library card ────────────────────────────────────────────────────────
//
// One shared widget: every copy shows the same `public_data`, so the bot
// updates them all with one edit. Keys: `n` [songs, artists, hours of music];
// `a` the last songs magpie-fi filed, each [title, artist]; `w` songs on the
// way; `r` songs waiting in Review; `h` problems: l library not writable,
// y yt-dlp missing, m no Mini App (no HTTPS address).

/// What the library card shows.
#[derive(Debug, Default, PartialEq)]
pub struct LibraryStats {
    pub songs: i64,
    pub artists: i64,
    pub hours: i64,
    /// Latest first, as (title, artist).
    pub recent: Vec<(String, String)>,
    pub on_the_way: i64,
    pub in_review: i64,
    pub library_writable: bool,
    pub ytdlp: bool,
    pub mini_app: bool,
}

pub fn library_state(stats: &LibraryStats) -> Value {
    let mut problems = String::new();
    for (ok, code) in [
        (stats.library_writable, 'l'),
        (stats.ytdlp, 'y'),
        (stats.mini_app, 'm'),
    ] {
        if !ok {
            problems.push(code);
        }
    }
    let mut card = json!({
        "n": [stats.songs, stats.artists, stats.hours],
        "a": stats.recent.iter().take(3).map(|(title, artist)| json!([title, artist])).collect::<Vec<_>>(),
        "w": stats.on_the_way,
        "r": stats.in_review,
        "h": problems,
    });
    for limit in [60, 32, 16] {
        if card.to_string().len() <= MAX_DATA {
            break;
        }
        for song in card["a"].as_array_mut().into_iter().flatten() {
            for part in [0, 1] {
                if let Some(text) = song[part].as_str() {
                    song[part] = json!(shorten(text, limit));
                }
            }
        }
    }
    card
}

/// A short fingerprint of the library card's code. A shared widget keeps the
/// code it was made with, so a new magpie-fi version with a changed card makes
/// a new shared widget instead of sending the old one again.
pub fn library_version() -> String {
    use sha2::{Digest, Sha256};
    hex::encode(&Sha256::digest(LIBRARY_HTML.as_bytes())[..6])
}

/// Three colours for the card from a song's embedded cover: the top-left, the
/// middle and the bottom-right of the picture, darkened when very light so
/// white text stays readable. `None` when the file has no valid cover.
pub fn cover_colours(file: &Path) -> Option<Colours> {
    let bytes = crate::infrastructure::media::tag_writer::read_artwork(file)
        .ok()
        .flatten()?;
    colours_of(&bytes)
}

fn colours_of(bytes: &[u8]) -> Option<Colours> {
    let image = image::load_from_memory(bytes).ok()?.to_rgb8();
    let small = image::imageops::thumbnail(&image, 6, 6);
    let average = |from: (u32, u32), to: (u32, u32)| {
        let mut sum = [0_u32; 3];
        let mut count = 0;
        for y in from.1..to.1 {
            for x in from.0..to.0 {
                let pixel = small.get_pixel(x, y);
                for channel in 0..3 {
                    sum[channel] += u32::from(pixel[channel]);
                }
                count += 1;
            }
        }
        let mut rgb = sum.map(|total| total / count.max(1));
        // Rec. 601 brightness; above about 60 % white text gets hard to read.
        let brightness = (299 * rgb[0] + 587 * rgb[1] + 114 * rgb[2]) / 1000;
        if brightness > 150 {
            rgb = rgb.map(|channel| channel * 150 / brightness);
        }
        format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])
    };
    Some([
        average((0, 0), (3, 3)),
        average((1, 1), (5, 5)),
        average((3, 3), (6, 6)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(total: i64, done: i64, working: i64, failed: i64) -> RequestSummary {
        RequestSummary {
            id: 9,
            kind: "playlist".into(),
            title: Some("Road Trip".into()),
            status: "ready".into(),
            total,
            done,
            working,
            failed,
            ..RequestSummary::default()
        }
    }

    fn song(title: &str, status: &str, updated_at: &str) -> WantedView {
        WantedView {
            position: 0,
            title: title.into(),
            artist: Some("Artist".into()),
            status: status.into(),
            error: Some("no audio".into()),
            track_id: None,
            updated_at: updated_at.into(),
        }
    }

    #[test]
    fn the_card_shows_every_song_and_the_one_being_worked_on() {
        let songs = [
            song("A", "filed", "1"),
            song("B", "downloading", "2"),
            song("C", "pending", "3"),
            song("D", "failed", "4"),
        ];
        let state = request_state(&summary(4, 1, 2, 1), &StatusView::default(), &songs, None);
        assert_eq!(state["s"], "busy");
        assert_eq!(state["g"], "ndwf");
        assert_eq!(state["w"], "dArtist – B");
        assert_eq!(state["b"], "sd");
        assert_eq!(state["n"], json!([1, 4, 2, 0, 1, 0]));
        assert!(state.get("c").is_none() && state.get("v").is_none());

        let done = request_state(&summary(4, 3, 0, 1), &StatusView::default(), &songs, None);
        assert_eq!(
            (done["s"].as_str(), done["b"].as_str()),
            (Some("done"), Some("rd"))
        );
        let sent: Value = serde_json::from_str(&with_revision(done, 7)).unwrap();
        assert_eq!(sent["v"], 7);
    }

    #[test]
    fn huge_playlists_still_fit() {
        let long = "Ein sehr langer Titel mit vielen Wörtern ".repeat(10);
        let songs: Vec<WantedView> = (0..500)
            .map(|index| song(&long, if index % 3 == 0 { "filed" } else { "pending" }, "1"))
            .collect();
        let mut big = summary(500, 167, 333, 0);
        big.title = Some("پلی‌لیست ".repeat(40));
        let colours = [
            "#112233".to_owned(),
            "#445566".to_owned(),
            "#778899".to_owned(),
        ];
        let state = request_state(&big, &StatusView::default(), &songs, Some(&colours));
        let sent = with_revision(state, i64::MAX);
        assert!(sent.len() <= MAX_DATA, "{} bytes", sent.len());
        let sent: Value = serde_json::from_str(&sent).unwrap();
        assert_eq!(sent["n"][1], 500);
        assert!(!sent["g"].as_str().unwrap().is_empty());
        assert!(sent["t"].as_str().unwrap().ends_with('…'));
    }

    #[test]
    fn an_unread_link_has_no_title_yet() {
        let mut failed = summary(0, 0, 0, 0);
        failed.title = None;
        failed.status = "failed".into();
        failed.error = Some("This playlist is private.".into());
        let state = request_state(&failed, &StatusView::default(), &[], None);
        assert_eq!(
            (state["t"].as_str(), state["s"].as_str()),
            (Some(""), Some("failed"))
        );
        assert_eq!(state["e"], "This playlist is private.");
    }

    #[test]
    fn details_answers_fit_too() {
        let songs: Vec<WantedView> = (0..80)
            .map(|index| song(&format!("Song number {index}"), "failed", "1"))
            .collect();
        let answer = details_answer(&songs);
        assert!(answer.len() <= MAX_DATA);
        let answer: Value = serde_json::from_str(&answer).unwrap();
        assert!(answer["m"].as_str().unwrap().contains("more"));
        assert_eq!(answer["d"][0][1], "Failed: no audio");
        let fine: Value = serde_json::from_str(&details_answer(&[])).unwrap();
        assert_eq!(fine["m"], texts::EVERY_SONG_FINE);
    }

    #[test]
    fn colours_come_from_the_cover_and_stay_readable() {
        // Left half black, right half white.
        let image = image::RgbImage::from_fn(60, 60, |x, _| {
            if x < 30 {
                image::Rgb([0, 0, 0])
            } else {
                image::Rgb([255, 255, 255])
            }
        });
        let mut png = Vec::new();
        image::DynamicImage::ImageRgb8(image)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let [first, _, last] = colours_of(&png).unwrap();
        assert_eq!(first, "#000000");
        // White is darkened so white text on it can be read.
        assert_eq!(last, "#969696");
        assert!(colours_of(b"not an image").is_none());
    }

    fn choice(id: i64, title: &str, breakdown: serde_json::Value) -> ReviewChoice {
        ReviewChoice {
            id,
            title: title.into(),
            artist: "Daft Punk".into(),
            album: Some("Discovery".into()),
            year: Some("2001-03-12".into()),
            score: 104.6,
            duration_delta: Some(2.4),
            score_breakdown: Some(breakdown.to_string()),
        }
    }

    #[test]
    fn review_cards_show_what_supports_each_choice() {
        let file = ReviewFile {
            song: "Daft Punk – One More Time".into(),
            seconds: Some(320.4),
            reason: Some("Two songs fit".into()),
        };
        let choices = [
            choice(
                5,
                "One More Time",
                json!({"audio_recognition": true, "sources": ["SongRec / Shazam", "deezer", "deezer"]}),
            ),
            choice(
                6,
                "One More Time (Radio Edit)",
                json!({"sources": ["itunes"]}),
            ),
        ];
        let state = review_state(12, &file, &choices);
        assert_eq!(
            (state["s"].as_str(), state["l"].as_i64()),
            (Some("ask"), Some(320))
        );
        assert_eq!(
            state["o"][0],
            json!([
                5,
                "One More Time",
                "Daft Punk",
                "Discovery · 2001",
                100,
                2,
                1,
                "SongRec / Shazam · Deezer"
            ])
        );
        assert_eq!(state["o"][1][6], 0);
        assert_eq!(state["o"][1][7], "iTunes");
        let ended = review_outcome(12, "Daft Punk – One More Time", "filed", texts::PICKED);
        assert_eq!(ended["s"], "filed");
        assert_eq!(ended["m"], "Got it. Filing it now.");
        assert_eq!(ended["f"], "Daft Punk – One More Time");
    }

    #[test]
    fn long_review_cards_still_fit() {
        let long = "Une chanson avec un titre vraiment beaucoup trop long ".repeat(8);
        let file = ReviewFile {
            song: long.clone(),
            seconds: None,
            reason: Some(long.clone()),
        };
        let choices: Vec<ReviewChoice> = (0..3)
            .map(|id| choice(id, &long, json!({"sources": [long.clone(), long.clone()]})))
            .collect();
        let sent = with_revision(review_state(1, &file, &choices), i64::MAX);
        assert!(sent.len() <= MAX_DATA, "{} bytes", sent.len());
        let sent: Value = serde_json::from_str(&sent).unwrap();
        assert!(!sent["o"].as_array().unwrap().is_empty());
    }

    #[test]
    fn the_library_card_lists_problems_and_fits() {
        let long = "A song title that goes on and on and on ".repeat(10);
        let stats = LibraryStats {
            songs: 1234,
            artists: 210,
            hours: 81,
            recent: vec![(long.clone(), long.clone()); 5],
            on_the_way: 3,
            in_review: 1,
            library_writable: true,
            ytdlp: false,
            mini_app: false,
        };
        let state = library_state(&stats);
        assert!(state.to_string().len() <= MAX_DATA);
        assert_eq!(state["n"], json!([1234, 210, 81]));
        assert_eq!(state["h"], "ym");
        assert_eq!(state["a"].as_array().unwrap().len(), 3);
        assert_eq!(library_version().len(), 12);
    }

    #[test]
    fn the_card_code_is_within_telegram_limits() {
        assert!(LIBRARY_HTML.len() <= 32768 && !LIBRARY_HTML.contains('\0'));
        assert!(REVIEW_HTML.len() <= 32768 && !REVIEW_HTML.contains('\0'));
        // The compressed limit (4096 bytes) is checked by web/scripts/check-widgets.mjs.
        assert!(REQUEST_HTML.len() <= 32768);
        assert!(!REQUEST_HTML.contains('\0'));
        assert!(serde_json::to_string(&reading(1, "album")).unwrap().len() <= MAX_DATA);
    }
}
