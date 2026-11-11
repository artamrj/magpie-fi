//! Everything the bot says, and every button label. Plain functions and
//! constants, so they are easy to test and to change in one place.

use crate::acquire::store::{RequestSummary, WantedView};

/// Telegram HTML needs only these three escaped.
pub fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// A message as a rich message (Bot API 10.1, written in its HTML form), with
/// the plain text sent instead when Telegram refuses the rich one.
#[derive(Debug)]
pub struct Rich {
    pub html: Option<String>,
    pub plain: String,
}

impl Rich {
    /// A message that is plain text only.
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            html: None,
            plain: text.into(),
        }
    }
}

const SOURCES: &str = "Spotify, Apple Music, Deezer, YouTube, YouTube Music or Shazam";

/// What strangers see: the bot is private, and their ID to give the owner.
pub fn stranger(user_id: i64) -> String {
    format!(
        "🐦 <b>magpie-fi</b>\n\nThis is a private music bot. Ask its owner for an invite.\n\
         Your Telegram ID: <code>{user_id}</code>"
    )
}

/// `/start` for owners and guests.
pub fn start() -> Rich {
    Rich {
        html: Some(format!(
            "<h3>🐦 magpie-fi</h3>\
             <p>Send me a song, album or playlist link, or an audio file. I find it, tag it \
             and put it in your library.</p>\
             <table bordered compact>\
             <tr><th>Links</th><td>{}</td></tr>\
             <tr><th>Files</th><td>MP3, M4A, FLAC, OGG, Opus, WAV, AIFF · up to 20 MB</td></tr>\
             </table>\
             <p>/help shows everything I can do.</p>",
            SOURCES.replace(", ", " · ").replace(" or ", " · ")
        )),
        plain: format!(
            "🐦 <b>magpie-fi</b>\n\nSend me a song, album or playlist link ({}) or an audio \
             file. I find it, tag it and put it in your library.\n\n/help shows everything I \
             can do.",
            SOURCES.replace(" or ", ", ")
        ),
    }
}

/// `/help`; owners also see their own commands.
pub fn help(owner: bool) -> Rich {
    let commands: Vec<&Command> = COMMANDS
        .iter()
        .filter(|command| command.name != "help" && (owner || !command.owners_only))
        .collect();
    let mut html = format!(
        "<h3>What I can do</h3>\
         <ul>\
         <li>Send a <b>link</b> to a song, album or playlist from {SOURCES}.</li>\
         <li>Send <b>audio files</b>. Forward several at once and they become one request.</li>\
         </ul>\
         <blockquote expandable><b>How it works.</b> Songs you already have are not \
         downloaded again. A playlist link also becomes a playlist in Navidrome. When I'm not \
         sure which song something is, I ask.</blockquote>\
         <h4>Commands</h4><table compact>"
    );
    let mut plain = format!(
        "<b>Send me</b>\n\
         • a song, album or playlist link from {SOURCES}\n\
         • audio files (forward several at once)\n\n\
         Songs you already have are not downloaded again. A playlist link also becomes a \
         playlist in Navidrome. When I'm not sure which song something is, I ask.\n\n\
         <b>Commands</b>\n"
    );
    for command in commands {
        html.push_str(&format!(
            "<tr><td>/{}</td><td>{}</td></tr>",
            command.name, command.help
        ));
        plain.push_str(&format!("/{} · {}\n", command.name, command.help));
    }
    html.push_str("</table>");
    Rich {
        html: Some(html),
        plain: plain.trim_end().to_owned(),
    }
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

/// A command in Telegram's command menu.
pub struct Command {
    pub name: &'static str,
    /// Shown in the menu.
    pub menu: &'static str,
    /// Shown in /help.
    pub help: &'static str,
    pub owners_only: bool,
    /// The answer is ephemeral (Bot API 10.2): seen once, not kept in the chat.
    pub ephemeral: bool,
}

pub const COMMANDS: [Command; 7] = [
    Command {
        name: "help",
        menu: "What I can do",
        help: "what I can do",
        owners_only: false,
        ephemeral: false,
    },
    Command {
        name: "queue",
        menu: "What is on the way",
        help: "what is on the way",
        owners_only: false,
        ephemeral: true,
    },
    Command {
        name: "health",
        menu: "Is everything working",
        help: "is everything working",
        owners_only: false,
        ephemeral: true,
    },
    Command {
        name: "library",
        menu: "Your library at a glance (owners)",
        help: "your library at a glance, kept up to date",
        owners_only: true,
        ephemeral: false,
    },
    Command {
        name: "guests",
        menu: "Invite people (owners)",
        help: "invite people or remove them",
        owners_only: true,
        ephemeral: false,
    },
    Command {
        name: "web",
        menu: "Log in to the web page (owners)",
        help: "log in to the web page",
        owners_only: true,
        ephemeral: false,
    },
    Command {
        name: "id",
        menu: "Your Telegram ID",
        help: "your Telegram ID",
        owners_only: false,
        ephemeral: true,
    },
];

/// The Mini App button next to the message box.
pub const MENU_BUTTON: &str = "magpie-fi";
pub const OPEN_APP: &str = "📱 Open magpie-fi";
pub const OPEN_IN_TELEGRAM: &str = "📱 Open in Telegram";
pub const OPEN_IN_BROWSER: &str = "🌐 Open in a browser";
pub const OPEN_IN_APP: &str = "🎵 Open in the app";
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

// ── Library ─────────────────────────────────────────────────────────────────

pub const LIBRARY_CAPTION: &str = "📚 <b>Your library</b>";

/// `/library` as text, where the chat can't show the card.
pub fn library(stats: &crate::telegram::cards::LibraryStats) -> Rich {
    let recent: Vec<String> = stats
        .recent
        .iter()
        .map(|(title, artist)| format!("{} · {}", escape(title), escape(artist)))
        .collect();
    let mut html = format!(
        "<h3>📚 Your library</h3>\
         <table bordered compact><tr><th>Songs</th><th>Artists</th><th>Hours</th></tr>\
         <tr><td>{}</td><td>{}</td><td>{}</td></tr></table>",
        stats.songs, stats.artists, stats.hours
    );
    let mut plain = format!(
        "📚 <b>Your library</b>\n{} songs · {} artists · {} hours",
        stats.songs, stats.artists, stats.hours
    );
    if !recent.is_empty() {
        html.push_str("<h4>Just added</h4><ul>");
        plain.push_str("\n\n<b>Just added</b>");
        for song in &recent {
            html.push_str(&format!("<li>{song}</li>"));
            plain.push_str(&format!("\n• {song}"));
        }
        html.push_str("</ul>");
    }
    let mut status = Vec::new();
    if stats.on_the_way > 0 {
        status.push(format!("⏳ {} on the way", stats.on_the_way));
    }
    if stats.in_review > 0 {
        status.push(format!("🤔 {} in Review", stats.in_review));
    }
    if !status.is_empty() {
        let status = status.join(" · ");
        html.push_str(&format!("<p>{status}</p>"));
        plain.push_str(&format!("\n\n{status}"));
    }
    Rich {
        html: Some(html),
        plain,
    }
}

// ── Guests ──────────────────────────────────────────────────────────────────

pub const CREATE_INVITE: &str = "➕ Create an invite link";
pub const INVITE_USED: &str = "This invite was already used or is not valid.";
pub const REMOVED_GUEST: &str = "Removed";

/// `/guests`: (user ID, name) of every guest.
pub fn guests(guests: &[(i64, String)]) -> Rich {
    let mut html =
        String::from("<h3>Guests</h3><p>Guests can send links and files, nothing else.</p>");
    let mut plain = String::from("<b>Guests</b> can send links and files, nothing else.\n");
    if guests.is_empty() {
        html.push_str("<p><i>No guests yet.</i></p>");
        plain.push_str("\nNo guests yet.");
    } else {
        html.push_str("<table bordered striped compact><tr><th>Name</th><th>Telegram ID</th></tr>");
        for (id, name) in guests {
            html.push_str(&format!(
                "<tr><td>{}</td><td><code>{id}</code></td></tr>",
                escape(name)
            ));
            plain.push_str(&format!("\n• {}", escape(name)));
        }
        html.push_str("</table>");
    }
    Rich {
        html: Some(html),
        plain,
    }
}

pub fn remove_guest(name: &str) -> String {
    format!("Remove {name}")
}

pub fn invite_link(link: &str) -> String {
    format!("Send this link to your guest. It works once:\n{link}")
}

pub fn welcome_guest(name: &str) -> Rich {
    let help = help(false);
    Rich {
        html: help
            .html
            .map(|html| format!("<h2>Welcome, {}! 🐦</h2>{html}", escape(name))),
        plain: format!("Welcome, {}! 🐦\n\n{}", escape(name), help.plain),
    }
}

// ── Links and buttons ───────────────────────────────────────────────────────

pub const READING_LINK: &str = "🔎 Reading the link…";
pub const NOT_ALLOWED: &str = "Not allowed";
pub const TAP_FAILED: &str = "That didn't work. Try again.";
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

pub const EVERY_SONG_FINE: &str = "Every song is fine. 🐦";

pub fn and_more(count: usize) -> String {
    format!("…and {count} more")
}

/// Details for a card, as plain text: [song, what happened] per song that
/// failed or needs someone.
pub fn details_rows(songs: &[WantedView]) -> Vec<[String; 2]> {
    songs
        .iter()
        .filter(|song| matches!(song.status.as_str(), "failed" | "review"))
        .map(|song| {
            let name = match &song.artist {
                Some(artist) => format!("{} · {artist}", song.title),
                None => song.title.clone(),
            };
            let what = if song.status == "review" {
                "Needs you"
            } else {
                "Failed"
            };
            let what = match &song.error {
                Some(error) => format!("{what}: {error}"),
                None => what.to_owned(),
            };
            [name, what]
        })
        .collect()
}

/// The Details button: songs of a request that failed or need someone.
pub fn details(songs: &[WantedView]) -> Rich {
    const SHOWN: usize = 30;
    let problems: Vec<_> = songs
        .iter()
        .filter(|song| matches!(song.status.as_str(), "failed" | "review"))
        .collect();
    if problems.is_empty() {
        return Rich::plain(EVERY_SONG_FINE);
    }
    let mut html = String::from(
        "<table bordered striped compact><tr><th>Song</th><th>What happened</th></tr>",
    );
    let mut plain = String::new();
    for song in problems.iter().take(SHOWN) {
        let name = format!(
            "<b>{}</b>{}",
            escape(&song.title),
            song.artist
                .as_deref()
                .map(|artist| format!(" · {}", escape(artist)))
                .unwrap_or_default()
        );
        let what = if song.status == "review" {
            "needs you"
        } else {
            "failed"
        };
        let error = song.error.as_deref().map(escape);
        let reason = error
            .as_deref()
            .map(|error| format!(": {error}"))
            .unwrap_or_default();
        let mut what_html = if song.status == "review" {
            "Needs you"
        } else {
            "Failed"
        }
        .to_owned();
        what_html.push_str(&reason);
        html.push_str(&format!("<tr><td>{name}</td><td>{what_html}</td></tr>"));
        plain.push_str(&format!("• {name} — {what}{reason}\n"));
    }
    html.push_str("</table>");
    if problems.len() > SHOWN {
        let more = and_more(problems.len() - SHOWN);
        html.push_str(&format!("<footer>{more}</footer>"));
        plain.push_str(&more);
    }
    Rich {
        html: Some(html),
        plain,
    }
}

// ── Review questions ────────────────────────────────────────────────────────

pub const CHOOSE_IN_APP: &str = "🎵 Choose in the app";
pub const SAVING_CHOICE: &str = "Saving your choice…";
pub const REVIEW_DONE_IN_APP: &str = "Sorted out in the app. 🐦";
pub const REVIEW_REMOVED: &str = "🗑 Removed. It won't be added.";
pub const REVIEW_GONE: &str = "This song is no longer waiting in Review.";

/// A source's name as people know it (stored names are sometimes IDs).
pub fn source_name(source: &str) -> String {
    match source {
        "musicbrainz" => "MusicBrainz",
        "deezer" => "Deezer",
        "itunes" => "iTunes",
        "discogs" => "Discogs",
        "lastfm" => "Last.fm",
        "songrec" => "SongRec / Shazam",
        "youtube" => "YouTube",
        "soundcloud" => "SoundCloud",
        "acoustid" => "AcoustID",
        other => other,
    }
    .to_owned()
}
pub const NONE_OF_THESE: &str = "🗑 None of these";

/// How a review question can be answered.
pub enum ReviewAnswer {
    /// In the Mini App (one button).
    InApp,
    /// With candidate buttons in the chat.
    InChat,
    /// Nothing fits; only the web page can help.
    NothingFound,
    /// On a review card, which shows the choices itself.
    InCard,
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
        ReviewAnswer::InCard => "",
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

/// The request's name: the song for one song, "3 files", or the album or
/// playlist title.
pub fn request_name(summary: &RequestSummary, view: &StatusView) -> String {
    match summary.kind.as_str() {
        "track" | "upload" if summary.total <= 1 => {
            view.song.clone().unwrap_or_else(|| "your song".to_owned())
        }
        "upload" => format!("{} files", summary.total),
        _ => summary
            .title
            .clone()
            .unwrap_or_else(|| summary.kind.clone()),
    }
}

/// The text status message (with a progress bar).
pub fn status(summary: &RequestSummary, view: &StatusView) -> String {
    status_text(summary, view, true)
}

/// A card's caption: the same words without the bar, because the card shows
/// the progress itself. It is also what the chat list, notifications and apps
/// without widgets show.
pub fn caption(summary: &RequestSummary, view: &StatusView) -> String {
    status_text(summary, view, false)
}

/// The note sent once an album or playlist is done: unlike edits, it notifies.
pub fn celebrate(summary: &RequestSummary, view: &StatusView) -> String {
    format!(
        "🎉 <b>{}</b> is in your library: {} of {} songs.",
        escape(&request_name(summary, view)),
        summary.done,
        summary.total
    )
}

fn status_text(summary: &RequestSummary, view: &StatusView, with_bar: bool) -> String {
    let name = request_name(summary, view);
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
        if with_bar {
            text.push_str(&bar(summary.done, summary.total));
            text.push(' ');
        }
        text.push_str(&format!(
            "{} of {} in your library\n",
            summary.done, summary.total
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
        // A card's caption has the same words, without the bar.
        let caption = caption(&summary("playlist", 4, 1, 2, 0, 1), &view);
        assert!(caption.contains("\n1 of 4 in your library\n2 on the way · 1 failed"));
        assert!(!caption.contains('▓'));
        let party = celebrate(&summary("playlist", 4, 4, 0, 0, 0), &view);
        assert_eq!(
            party,
            "🎉 <b>Road &lt;Trip&gt;</b> is in your library: 4 of 4 songs."
        );
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
        assert!(!text.plain.contains("Fine"));
        assert!(
            text.plain
                .contains("• <b>Lost</b> · A&amp;B — failed: no &lt;audio&gt;")
        );
        assert!(text.plain.contains("<b>Unsure</b> · A&amp;B — needs you"));
        let html = text.html.unwrap();
        assert!(html.contains("<td><b>Lost</b> · A&amp;B</td><td>Failed: no &lt;audio&gt;</td>"));
        let fine = details(&[song("Fine", "ready", None)]);
        assert!(fine.plain.contains("Every song is fine") && fine.html.is_none());
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
        let owner = help(true);
        assert!(owner.plain.contains("/guests · invite people"));
        assert!(owner.html.unwrap().contains("<tr><td>/guests</td>"));
        let guest = help(false);
        assert!(!guest.plain.contains("/guests"));
        assert!(!guest.html.unwrap().contains("/guests"));
    }

    #[test]
    fn rich_messages_keep_names_escaped() {
        let text = guests(&[(7, "<Ana>".into())]);
        assert!(
            text.html
                .unwrap()
                .contains("<td>&lt;Ana&gt;</td><td><code>7</code></td>")
        );
        assert!(text.plain.contains("• &lt;Ana&gt;"));
        let welcome = welcome_guest("A&B");
        assert!(
            welcome
                .html
                .unwrap()
                .starts_with("<h2>Welcome, A&amp;B! 🐦</h2>")
        );
        assert!(
            start()
                .html
                .unwrap()
                .contains("Spotify · Apple Music · Deezer")
        );
    }
}
