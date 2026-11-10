//! Everything the bot says, and every button label. Plain functions and
//! constants, so they are easy to test and to change in one place.

use crate::acquire::store::{RequestSummary, WantedView};

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
    text.trim_end().to_owned()
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

// ── Commands ────────────────────────────────────────────────────────────────

/// Telegram's command menu: (command, description).
pub const COMMANDS: [(&str, &str); 6] = [
    ("help", "What I can do"),
    ("queue", "What is on the way"),
    ("health", "Is everything working"),
    ("guests", "Invite people (owners)"),
    ("web", "Log in to the web page (owners)"),
    ("id", "Your Telegram ID"),
];

/// The Mini App button next to the message box.
pub const MENU_BUTTON: &str = "magpie-fi";
pub const OPEN_APP: &str = "📱 Open magpie-fi";
pub const OPEN_IN_TELEGRAM: &str = "📱 Open in Telegram";
pub const OPEN_IN_BROWSER: &str = "🌐 Open in a browser";
pub const UNKNOWN_COMMAND: &str = "I don't know that command. /help";
pub const SEND_SOMETHING: &str = "Send me a song, album or playlist link, or an audio file. /help";

pub fn your_id(user_id: i64) -> String {
    format!("Your Telegram ID: <code>{user_id}</code>")
}

pub fn login_link(link: &str, has_public_url: bool) -> String {
    format!(
        "Your login link for the web page. It works once, for 10 minutes, and keeps \
         that browser logged in for 30 days:\n{}{}",
        escape(link),
        if has_public_url {
            ""
        } else {
            "\n\nOpen it on the server itself (MAGPIE_PUBLIC_URL is not set)."
        }
    )
}

/// `/queue`: the requests that still have songs on the way.
pub fn queue(requests: &[RequestSummary]) -> String {
    let active: Vec<_> = requests
        .iter()
        .filter(|request| request.status == "reading" || request.working > 0)
        .collect();
    if active.is_empty() {
        return "Nothing on the way. 🐦".into();
    }
    let mut text = String::from("<b>On the way</b>\n");
    for request in active.iter().take(10) {
        let name = request.title.as_deref().unwrap_or(&request.kind);
        text.push_str(&format!(
            "• {} — {} of {} done, {} on the way\n",
            escape(name),
            request.done,
            request.total,
            request.working
        ));
    }
    text
}

/// What `/health` checks.
pub struct Health {
    pub library_writable: bool,
    pub songs: i64,
    pub ytdlp: bool,
    pub mini_app: bool,
}

pub fn health(health: &Health) -> String {
    let check = |ok: bool| if ok { "✅" } else { "⚠️" };
    let mini_app = if health.mini_app {
        "✅"
    } else {
        "⚠️ off: set MAGPIE_PUBLIC_URL to the page's https:// address"
    };
    format!(
        "<b>Health</b>\n\
         Library: {} {} songs\n\
         yt-dlp: {}\n\
         Mini App: {mini_app}",
        check(health.library_writable),
        health.songs,
        check(health.ytdlp),
    )
}

// ── Guests ──────────────────────────────────────────────────────────────────

pub const CREATE_INVITE: &str = "➕ Create an invite link";
pub const INVITE_USED: &str = "This invite was already used or is not valid.";
pub const REMOVED_GUEST: &str = "Removed";

/// `/guests`: (user ID, name) of every guest.
pub fn guests(guests: &[(i64, String)]) -> String {
    let mut text = String::from("<b>Guests</b> can send links and files, nothing else.\n");
    if guests.is_empty() {
        text.push_str("\nNo guests yet.");
    }
    for (_, name) in guests {
        text.push_str(&format!("\n• {}", escape(name)));
    }
    text
}

pub fn remove_guest(name: &str) -> String {
    format!("Remove {name}")
}

pub fn invite_link(link: &str) -> String {
    format!("Send this link to your guest. It works once:\n{link}")
}

pub fn welcome_guest(name: &str) -> String {
    format!("Welcome, {}! 🐦\n\n{}", escape(name), help(false))
}

// ── Links and buttons ───────────────────────────────────────────────────────

pub const READING_LINK: &str = "🔎 Reading the link…";
pub const NOT_ALLOWED: &str = "Not allowed";
pub const STOP: &str = "⏹ Stop";
pub const RETRY_FAILED: &str = "🔁 Retry failed";
pub const DETAILS: &str = "ℹ️ Details";
pub const REMOVED_SONG: &str = "🗑 Removed. It won't be added.";
pub const SONG_ALREADY_GONE: &str = "That song is already gone.";
pub const PICKED: &str = "✅ Got it. Filing it now.";
pub const PICKED_NO_COVER: &str = "Saved your choice, but no matching cover was found yet. \
                                   Finish it on the web page under Review.";
pub const PICK_GONE: &str = "That choice is no longer available. Open Review on the web page.";

pub fn cant_read_link(url: &str) -> String {
    format!("I can't read this link: {}", escape(url))
}

pub fn stopped(songs: u64) -> String {
    format!("Stopped {songs} songs that had not started")
}

pub fn retrying(songs: u64) -> String {
    format!("Trying {songs} songs again")
}

/// The Details button: songs of a request that failed or need someone.
pub fn details(songs: &[WantedView]) -> String {
    const SHOWN: usize = 30;
    let problems: Vec<_> = songs
        .iter()
        .filter(|song| matches!(song.status.as_str(), "failed" | "review"))
        .collect();
    if problems.is_empty() {
        return "Every song is fine. 🐦".into();
    }
    let mut text = String::new();
    for song in problems.iter().take(SHOWN) {
        let what = if song.status == "review" {
            "needs you"
        } else {
            "failed"
        };
        text.push_str(&format!(
            "• <b>{}</b>{} — {what}{}\n",
            escape(&song.title),
            song.artist
                .as_deref()
                .map(|artist| format!(" · {}", escape(artist)))
                .unwrap_or_default(),
            song.error
                .as_deref()
                .map(|error| format!(": {}", escape(error)))
                .unwrap_or_default()
        ));
    }
    if problems.len() > SHOWN {
        text.push_str(&format!("…and {} more", problems.len() - SHOWN));
    }
    text
}

// ── Review questions ────────────────────────────────────────────────────────

pub const CHOOSE_IN_APP: &str = "🎵 Choose in the app";
pub const NONE_OF_THESE: &str = "🗑 None of these";

/// How a review question can be answered.
pub enum ReviewAnswer {
    /// In the Mini App (one button).
    InApp,
    /// With candidate buttons in the chat.
    InChat,
    /// Nothing fits; only the web page can help.
    NothingFound,
}

/// "🤔 Not sure which song this is" for a song waiting in Review.
pub fn review_question(song: &str, reason: Option<&str>, answer: &ReviewAnswer) -> String {
    let mut text = format!("🤔 Not sure which song this is:\n<b>{}</b>", escape(song));
    if let Some(reason) = reason {
        text.push_str(&format!("\n<i>{}</i>", escape(reason)));
    }
    text.push_str(match answer {
        ReviewAnswer::InApp => "\n\nChoose the right one in the app.",
        ReviewAnswer::InChat => "\n\nIs it one of these?",
        ReviewAnswer::NothingFound => {
            "\n\nI found nothing that fits. Open Review on the web page to type it in."
        }
    });
    text
}

// ── Status messages ─────────────────────────────────────────────────────────

/// What a status message needs besides the counts.
#[derive(Debug, Default)]
pub struct StatusView {
    /// "Artist – Title" for a single song or upload.
    pub song: Option<String>,
    /// Songs filed now (not counting ones that were already in the library).
    pub newly_filed: i64,
}

/// Is the request finished (nothing on the way, nothing waiting for a person)?
pub fn is_final(summary: &RequestSummary) -> bool {
    summary.status == "failed"
        || (summary.status == "ready" && summary.working == 0 && summary.review == 0)
}

pub fn status(summary: &RequestSummary, view: &StatusView) -> String {
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
    if summary.removed > 0 {
        details.push(if summary.total > 1 {
            format!("{} removed", summary.removed)
        } else {
            "Removed in Review".to_owned()
        });
    }
    if !details.is_empty() {
        text.push_str(&details.join(" · "));
        text.push('\n');
    }
    text.trim_end().to_owned()
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
    fn playlists_show_progress() {
        let view = StatusView {
            newly_filed: 3,
            ..StatusView::default()
        };
        let busy = status(&summary("playlist", 4, 1, 2, 0, 1), &view);
        assert!(
            busy.starts_with("⏳ <b>Road &lt;Trip&gt;</b> · playlist"),
            "{busy}"
        );
        assert!(busy.contains("▓▓░░░░░░░░ 1 of 4 in your library"));
        assert!(busy.contains("2 on the way · 1 failed"));

        let done = status(&summary("playlist", 4, 4, 0, 0, 0), &view);
        assert!(done.starts_with("✅"));
        assert!(done.ends_with("4 of 4 in your library"));
    }

    #[test]
    fn single_songs_say_added() {
        let view = StatusView {
            song: Some("Daft Punk – One More Time".into()),
            newly_filed: 1,
        };
        let text = status(&summary("track", 1, 1, 0, 0, 0), &view);
        assert!(text.contains("<b>Daft Punk – One More Time</b>"));
        assert!(text.contains("Added to your library"));
    }

    #[test]
    fn already_owned_and_failed_songs() {
        let view = StatusView {
            song: Some("A – B".into()),
            ..StatusView::default()
        };
        let owned = status(&summary("track", 1, 1, 0, 0, 0), &view);
        assert!(owned.contains("Already in your library"));
        assert!(!owned.contains("<i>"));
        let failed = status(&summary("track", 1, 0, 0, 0, 1), &view);
        assert!(failed.starts_with("⚠️"));
        let waiting = summary("track", 1, 0, 0, 1, 0);
        assert!(!is_final(&waiting));
        assert!(status(&waiting, &view).contains("Not sure which song"));
    }

    #[test]
    fn big_files_get_a_clear_answer() {
        let text = too_big("Song <live>.flac", 45 * 1024 * 1024);
        assert!(text.contains("<b>Song &lt;live&gt;.flac</b> is 45 MB"));
        assert!(text.contains("up to 20 MB"));
    }

    #[test]
    fn details_list_only_songs_with_problems() {
        let song = |title: &str, status: &str, error: Option<&str>| WantedView {
            position: 0,
            title: title.into(),
            artist: Some("A&B".into()),
            status: status.into(),
            error: error.map(str::to_owned),
            track_id: None,
            updated_at: String::new(),
        };
        let text = details(&[
            song("Fine", "ready", None),
            song("Lost", "failed", Some("no <audio>")),
            song("Unsure", "review", None),
        ]);
        assert!(!text.contains("Fine"));
        assert!(text.contains("• <b>Lost</b> · A&amp;B — failed: no &lt;audio&gt;"));
        assert!(text.contains("<b>Unsure</b> · A&amp;B — needs you"));
        assert!(details(&[song("Fine", "ready", None)]).contains("Every song is fine"));
    }

    #[test]
    fn review_questions_say_how_to_answer() {
        let app = review_question("A – <B>", Some("two matches"), &ReviewAnswer::InApp);
        assert!(app.contains("<b>A – &lt;B&gt;</b>\n<i>two matches</i>"));
        assert!(app.ends_with("in the app."));
        assert!(review_question("x", None, &ReviewAnswer::NothingFound).contains("nothing"));
    }

    #[test]
    fn queue_lists_only_busy_requests() {
        let mut busy = summary("playlist", 4, 1, 2, 0, 0);
        busy.title = Some("Mix".into());
        let idle = || summary("album", 2, 2, 0, 0, 0);
        assert_eq!(queue(&[idle()]), "Nothing on the way. 🐦");
        assert!(queue(&[busy, idle()]).contains("• Mix — 1 of 4 done, 2 on the way"));
    }

    #[test]
    fn health_explains_a_missing_https_address() {
        let text = health(&Health {
            library_writable: true,
            songs: 12,
            ytdlp: false,
            mini_app: false,
        });
        assert!(text.contains("Library: ✅ 12 songs"));
        assert!(text.contains("yt-dlp: ⚠️"));
        assert!(text.contains("MAGPIE_PUBLIC_URL"));
    }

    #[test]
    fn help_shows_owner_commands_to_owners() {
        let text = help(true);
        assert!(text.contains("/guests"));
        assert!(!help(false).contains("/guests"));
    }
}
