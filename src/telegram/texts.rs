//! What the bot says. Plain functions, so they are easy to test.

use crate::acquire::store::RequestSummary;

pub const RHYME: [&str; 7] = [
    "One for sorrow",
    "Two for joy",
    "Three for a girl",
    "Four for a boy",
    "Five for silver",
    "Six for gold",
    "Seven for a secret never to be told",
];

/// Telegram HTML needs only these three escaped.
pub fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

pub fn welcome(authorized: bool, user_id: i64) -> String {
    if authorized {
        "🐦 <b>magpie-fi</b>\n\nSend me a song, album or playlist link (Spotify, Apple Music, \
         Deezer, YouTube, YouTube Music, Shazam) or an audio file. I find it, tag it and put it \
         in your library.\n\n/help shows everything I can do."
            .to_owned()
    } else {
        format!(
            "🐦 <b>magpie-fi</b>\n\nThis is a private music bot. Ask its owner for an invite.\n\
             Your Telegram ID: <code>{user_id}</code>"
        )
    }
}

pub fn help(owner: bool) -> String {
    let mut text = String::from(
        "<b>Send me</b>\n\
         • a song, album or playlist link from Spotify, Apple Music, Deezer, YouTube, YouTube \
           Music or Shazam\n\
         • audio files (forward several at once)\n\n\
         Songs you already have are not downloaded again. A playlist link also becomes a \
         playlist in Navidrome. When I'm not sure which song something is, I ask.\n\n\
         <b>Commands</b>\n\
         /queue · what is on the way\n\
         /health · is everything working\n\
         /id · your Telegram ID\n",
    );
    if owner {
        text.push_str("/guests · invite people or remove them\n");
        text.push_str("/web · log in to the web page\n");
    }
    text.push_str("\n<i>");
    for (index, line) in RHYME.iter().enumerate() {
        text.push_str(&"🐦".repeat(index + 1));
        text.push(' ');
        text.push_str(line);
        text.push('\n');
    }
    text.push_str("</i>");
    text
}

/// Answer to an upload over Telegram's 20 MB limit for bots.
pub fn too_big(name: &str, bytes: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let megabytes = bytes as f64 / (1024.0 * 1024.0);
    format!(
        "⚠️ <b>{}</b> is {megabytes:.0} MB. Telegram only lets bots receive files up to \
         20 MB, so I can't take this one.\n\nSend a smaller version (MP3 or M4A), a link \
         to the song, or copy the file into the inbox folder.",
        escape(name)
    )
}

/// What a status message needs besides the counts.
#[derive(Debug, Default)]
pub struct StatusView {
    /// "Artist – Title" for a single song or upload.
    pub song: Option<String>,
    /// The single song was filed as lossless audio (FLAC and friends).
    pub lossless: bool,
    /// Songs filed now (not counting ones that were already in the library).
    pub newly_filed: i64,
}

/// Is the request finished (nothing on the way, nothing waiting for a person)?
pub fn is_final(summary: &RequestSummary) -> bool {
    summary.status == "failed"
        || (summary.status == "ready" && summary.working == 0 && summary.review == 0)
}

pub fn status(summary: &RequestSummary, view: &StatusView, rhyme: bool) -> String {
    let name = match summary.kind.as_str() {
        "track" | "upload" if summary.total <= 1 => {
            view.song.clone().unwrap_or_else(|| "your song".to_owned())
        }
        "upload" => format!("{} files", summary.total),
        _ => summary
            .title
            .clone()
            .unwrap_or_else(|| summary.kind.clone()),
    };
    let kind = match summary.kind.as_str() {
        "album" => " · album",
        "playlist" => " · playlist",
        _ => "",
    };
    let header = format!("<b>{}</b>{kind}", escape(&name));

    if summary.status == "reading" {
        return format!("🔎 {header}\nReading the link…");
    }
    if summary.status == "failed" {
        let reason = summary.error.as_deref().unwrap_or("unknown error");
        return format!("⚠️ {header}\nCouldn't read this link: {}", escape(reason));
    }

    let finished = is_final(summary);
    let icon = if !finished {
        "⏳"
    } else if summary.done == summary.total {
        "✅"
    } else if summary.done == 0 {
        "⚠️"
    } else {
        "☑️"
    };
    let mut text = format!("{icon} {header}\n");
    if summary.total > 1 {
        text.push_str(&format!(
            "{} {} of {} in your library\n",
            bar(summary.done, summary.total),
            summary.done,
            summary.total
        ));
    } else if summary.done == 1 {
        text.push_str(if view.newly_filed > 0 {
            "Added to your library\n"
        } else {
            "Already in your library\n"
        });
    }
    let mut details = Vec::new();
    if summary.working > 0 {
        details.push(if summary.total > 1 {
            format!("{} on the way", summary.working)
        } else {
            "On the way…".to_owned()
        });
    }
    if summary.review > 0 {
        details.push(if summary.total > 1 {
            format!("{} need you (see below)", summary.review)
        } else {
            "Not sure which song this is (see below)".to_owned()
        });
    }
    if summary.failed > 0 {
        details.push(if summary.total > 1 {
            format!("{} failed", summary.failed)
        } else {
            "Failed".to_owned()
        });
    }
    if !details.is_empty() {
        text.push_str(&details.join(" · "));
        text.push('\n');
    }
    if finished
        && rhyme
        && let Some(line) = rhyme_line(summary, view)
    {
        text.push_str(&format!("\n<i>{line}</i>"));
    }
    text.trim_end().to_owned()
}

/// 🐦 One for sorrow (nothing worked), 🐦🐦 Two for joy (songs added), and for
/// a single song 🐦🐦🐦🐦🐦 Five for silver (lossy) or 🐦🐦🐦🐦🐦🐦 Six for gold (lossless).
fn rhyme_line(summary: &RequestSummary, view: &StatusView) -> Option<String> {
    let index = if summary.done == 0 && summary.failed > 0 {
        0
    } else if view.newly_filed == 0 {
        return None;
    } else if summary.total == 1 {
        if view.lossless { 5 } else { 4 }
    } else {
        1
    };
    Some(format!("{} {}", "🐦".repeat(index + 1), RHYME[index]))
}

fn bar(done: i64, total: i64) -> String {
    let filled = if total > 0 {
        usize::try_from(done * 10 / total).unwrap_or(0).min(10)
    } else {
        0
    };
    format!("{}{}", "▓".repeat(filled), "░".repeat(10 - filled))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(
        kind: &str,
        total: i64,
        done: i64,
        working: i64,
        review: i64,
        failed: i64,
    ) -> RequestSummary {
        RequestSummary {
            id: 1,
            kind: kind.into(),
            title: Some("Road <Trip>".into()),
            status: "ready".into(),
            total,
            done,
            working,
            review,
            failed,
            ..RequestSummary::default()
        }
    }

    #[test]
    fn playlists_show_progress_and_finish_with_the_rhyme() {
        let view = StatusView {
            newly_filed: 3,
            ..StatusView::default()
        };
        let busy = status(&summary("playlist", 4, 1, 2, 0, 1), &view, true);
        assert!(
            busy.starts_with("⏳ <b>Road &lt;Trip&gt;</b> · playlist"),
            "{busy}"
        );
        assert!(busy.contains("▓▓░░░░░░░░ 1 of 4 in your library"));
        assert!(busy.contains("2 on the way · 1 failed"));
        assert!(!busy.contains("for joy"));

        let done = status(&summary("playlist", 4, 4, 0, 0, 0), &view, true);
        assert!(done.starts_with("✅"));
        assert!(done.ends_with("<i>🐦🐦 Two for joy</i>"));
        assert!(!status(&summary("playlist", 4, 4, 0, 0, 0), &view, false).contains("joy"));
    }

    #[test]
    fn single_songs_say_silver_or_gold() {
        let mut view = StatusView {
            song: Some("Daft Punk – One More Time".into()),
            newly_filed: 1,
            lossless: false,
        };
        let text = status(&summary("track", 1, 1, 0, 0, 0), &view, true);
        assert!(text.contains("<b>Daft Punk – One More Time</b>"));
        assert!(text.contains("Added to your library"));
        assert!(text.ends_with("Five for silver</i>"));
        view.lossless = true;
        assert!(
            status(&summary("upload", 1, 1, 0, 0, 0), &view, true).ends_with("Six for gold</i>")
        );
    }

    #[test]
    fn already_owned_and_failed_songs() {
        let view = StatusView {
            song: Some("A – B".into()),
            ..StatusView::default()
        };
        let owned = status(&summary("track", 1, 1, 0, 0, 0), &view, true);
        assert!(owned.contains("Already in your library"));
        assert!(!owned.contains("<i>"));
        let failed = status(&summary("track", 1, 0, 0, 0, 1), &view, true);
        assert!(failed.starts_with("⚠️"));
        assert!(failed.ends_with("🐦 One for sorrow</i>"));
        let waiting = summary("track", 1, 0, 0, 1, 0);
        assert!(!is_final(&waiting));
        assert!(status(&waiting, &view, true).contains("Not sure which song"));
    }

    #[test]
    fn big_files_get_a_clear_answer() {
        let text = too_big("Song <live>.flac", 45 * 1024 * 1024);
        assert!(text.contains("<b>Song &lt;live&gt;.flac</b> is 45 MB"));
        assert!(text.contains("up to 20 MB"));
    }

    #[test]
    fn help_contains_the_whole_rhyme() {
        let text = help(true);
        assert!(text.contains("/guests"));
        assert!(text.contains("🐦🐦🐦🐦🐦🐦🐦 Seven for a secret never to be told"));
        assert!(!help(false).contains("/guests"));
    }
}
