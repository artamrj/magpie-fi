//! Keeps every Telegram request's status message up to date, and asks the
//! sender when a song waits in review. Reads the same tables as the web page,
//! every few seconds; a message is only edited when its text changed.

use super::{Context, api, react, send_with, texts};
use crate::acquire::store;
use anyhow::Result;
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};

const INTERVAL: Duration = Duration::from_secs(3);

pub async fn run(context: Arc<Context>) {
    loop {
        if let Err(error) = refresh(&context).await {
            tracing::warn!(%error, "updating Telegram status messages failed");
        }
        if let Err(error) = ask_reviews(&context).await {
            tracing::warn!(%error, "asking about review songs failed");
        }
        tokio::time::sleep(INTERVAL).await;
    }
}

async fn refresh(context: &Context) -> Result<()> {
    let rhyme = context.state.config.read().await.rhyme;
    for open in store::open_telegram_requests(&context.state.pool).await? {
        let Some(summary) = store::request_summary(&context.state.pool, open.id).await? else {
            continue;
        };
        let view = view(context, open.id).await?;
        let text = texts::status(&summary, &view, rhyme);
        let finished = texts::is_final(&summary);
        if open.status_text.as_deref() == Some(text.as_str()) && !finished {
            continue;
        }
        let message = i32::try_from(open.status_message_id).unwrap_or_default();
        let params = json!({
            "chat_id": open.chat_id,
            "message_id": message,
            "text": text,
            "parse_mode": "HTML",
            "link_preview_options": {"is_disabled": true},
            "reply_markup": keyboard(&summary, finished),
        });
        match context
            .bot
            .call::<_, Value>("editMessageText", &params)
            .await
        {
            Ok(_) => {}
            Err(error) if api::is_not_modified(&error) => {}
            Err(error) if api::is_gone(&error) => {
                // The status message was deleted: stop updating it.
                store::set_status_text(&context.state.pool, open.id, &text, true).await?;
                continue;
            }
            Err(error) => {
                tracing::warn!(%error, request = open.id, "could not update a status message");
                continue;
            }
        }
        store::set_status_text(&context.state.pool, open.id, &text, finished).await?;
        if finished && let Some(user_message) = open.user_message_id {
            let emoji = if summary.done > 0 { "👌" } else { "🤷" };
            react(
                context,
                open.chat_id,
                i32::try_from(user_message).unwrap_or_default(),
                emoji,
            )
            .await;
        }
    }
    Ok(())
}

async fn view(context: &Context, request: i64) -> Result<texts::StatusView> {
    let songs = store::request_songs(&context.state.pool, request).await?;
    let first = songs.first();
    Ok(texts::StatusView {
        song: first.map(|song| match &song.artist {
            Some(artist) => format!("{artist} – {}", song.title),
            None => song.title.clone(),
        }),
        lossless: first
            .and_then(|song| song.library_path.as_deref())
            .is_some_and(is_lossless_file),
        newly_filed: songs.iter().filter(|song| song.status == "filed").count() as i64,
    })
}

fn is_lossless_file(path: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .is_some_and(|extension| {
            matches!(extension.as_str(), "flac" | "wav" | "aiff" | "aif" | "alac")
        })
}

/// Stop while songs are waiting; Retry and Details when something failed.
fn keyboard(summary: &store::RequestSummary, finished: bool) -> Option<Value> {
    let mut row = Vec::new();
    if !finished && summary.working > 0 && summary.total > 1 {
        row.push(json!({"text": "⏹ Stop", "callback_data": format!("stop:{}", summary.id)}));
    }
    if finished && summary.failed > 0 {
        row.push(
            json!({"text": "🔁 Retry failed", "callback_data": format!("retry:{}", summary.id)}),
        );
    }
    if summary.failed > 0 || summary.review > 0 {
        row.push(json!({"text": "ℹ️ Details", "callback_data": format!("details:{}", summary.id)}));
    }
    Some(json!({"inline_keyboard": if row.is_empty() { vec![] } else { vec![row] }}))
}

/// "Which song is this?" with up to three candidates, for songs sent from
/// Telegram that wait in review.
async fn ask_reviews(context: &Context) -> Result<()> {
    for question in store::unasked_reviews(&context.state.pool).await? {
        let candidates: Vec<(i64, String, String, Option<String>, Option<String>)> =
            sqlx::query_as(
                "SELECT id,title,artist,album,year FROM candidates WHERE track_id=?
             ORDER BY score DESC LIMIT 3",
            )
            .bind(question.track_id)
            .fetch_all(&context.state.pool)
            .await?;
        let mut rows: Vec<Vec<Value>> = candidates
            .iter()
            .map(|(id, title, artist, album, year)| {
                let mut label = format!("{artist} – {title}");
                if let Some(album) = album.as_deref().filter(|album| !album.is_empty()) {
                    label.push_str(&format!(" · {album}"));
                }
                if let Some(year) = year.as_deref().and_then(|year| year.get(..4)) {
                    label.push_str(&format!(" ({year})"));
                }
                vec![json!({
                    "text": shorten(&label, 60),
                    "callback_data": format!("pick:{}:{id}", question.track_id),
                })]
            })
            .collect();
        rows.push(vec![json!({
            "text": "🗑 None of these",
            "callback_data": format!("drop:{}", question.track_id),
        })]);
        if let Some(url) =
            super::mini_app_url(&context.state.config.read().await.public_url, "review")
        {
            rows.push(vec![
                json!({"text": "📱 Open review", "web_app": {"url": url}}),
            ]);
        }
        let song = match &question.artist {
            Some(artist) => format!("{artist} – {}", question.title),
            None => question.title.clone(),
        };
        let mut text = format!(
            "🤔 Not sure which song this is:\n<b>{}</b>",
            texts::escape(&song)
        );
        if let Some(reason) = question.reason.as_deref() {
            text.push_str(&format!("\n<i>{}</i>", texts::escape(reason)));
        }
        text.push_str(if candidates.is_empty() {
            "\n\nI found nothing that fits. Open Review on the web page to type it in."
        } else {
            "\n\nIs it one of these?"
        });
        let sent = send_with(
            context,
            question.chat_id,
            &text,
            None,
            Some(json!({"inline_keyboard": rows})),
        )
        .await;
        // Remember the question even if sending failed, so it is not repeated forever.
        let message = sent.map_or(0, |message| message.message_id);
        store::set_review_message(&context.state.pool, question.wanted_id, message).await?;
    }
    Ok(())
}

fn shorten(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let mut short: String = text.chars().take(limit - 1).collect();
    short.push('…');
    short
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buttons_follow_the_state_of_the_request() {
        let mut summary = store::RequestSummary {
            id: 7,
            total: 4,
            working: 2,
            ..store::RequestSummary::default()
        };
        let busy = keyboard(&summary, false).unwrap().to_string();
        assert!(busy.contains("stop:7") && !busy.contains("retry"));
        summary.working = 0;
        summary.failed = 1;
        let done = keyboard(&summary, true).unwrap().to_string();
        assert!(done.contains("retry:7") && done.contains("details:7") && !done.contains("stop"));
    }

    #[test]
    fn long_labels_are_shortened() {
        assert_eq!(shorten("abc", 5), "abc");
        assert_eq!(shorten("abcdefgh", 5), "abcd…");
        assert!(is_lossless_file("/m/a.FLAC"));
        assert!(!is_lossless_file("/m/a.m4a"));
    }
}
