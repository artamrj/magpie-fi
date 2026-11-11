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

    #[test]
    fn the_card_code_is_within_telegram_limits() {
        // The compressed limit (4096 bytes) is checked by web/scripts/check-widgets.mjs.
        assert!(REQUEST_HTML.len() <= 32768);
        assert!(!REQUEST_HTML.contains('\0'));
        assert!(serde_json::to_string(&reading(1, "album")).unwrap().len() <= MAX_DATA);
    }
}
