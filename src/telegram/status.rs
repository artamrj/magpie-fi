//! Keeps every Telegram request's status message up to date, and asks the
//! sender when a song waits in review. Reads the same tables as the web page,
//! every few seconds; a message is only edited when its text changed.

use super::{Context, actions::Action, api, edit_params, react, send_with, texts};
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
    for open in store::open_telegram_requests(&context.state.pool).await? {
        let summary = &open.summary;
        let view = texts::StatusView {
            song: open.first_song.clone(),
            newly_filed: open.newly_filed,
        };
        let text = texts::status(summary, &view);
        let finished = texts::is_final(summary);
        if open.status_text.as_deref() == Some(text.as_str()) && !finished {
            continue;
        }
        let message = i32::try_from(open.status_message_id).unwrap_or_default();
        let app = app_url(context, open.chat_id, summary.review > 0);
        let params = edit_params(
            open.chat_id,
            message,
            &text,
            keyboard(summary, finished, app),
        );
        match context
            .bot
            .call::<_, Value>("editMessageText", &params)
            .await
        {
            Ok(_) => {}
            Err(error) if api::is_not_modified(&error) => {}
            Err(error) if api::is_gone(&error) => {
                // The status message was deleted: stop updating it.
                store::set_status_text(&context.state.pool, summary.id, &text, true).await?;
                continue;
            }
            Err(error) => {
                tracing::warn!(%error, request = summary.id, "could not update a status message");
                continue;
            }
        }
        store::set_status_text(&context.state.pool, summary.id, &text, finished).await?;
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

/// The Mini App for this chat: only owners may open the web page, and only
/// over HTTPS. It opens on Review when a song there waits for them.
fn app_url(context: &Context, chat: i64, review: bool) -> Option<String> {
    if !context.config.owners.contains(&chat) {
        return None;
    }
    let view = if review { "review" } else { "" };
    super::mini_app_url(&context.state.config.public_url, view)
}

/// Stop while songs are waiting; Retry and Details when something failed.
/// While a playlist is busy the row keeps its shape: Retry and Details stay
/// greyed out until they can do something, so buttons don't jump around.
fn keyboard(summary: &store::RequestSummary, finished: bool, app: Option<String>) -> Option<Value> {
    let problems = summary.failed > 0 || summary.review > 0;
    let mut row = Vec::new();
    if !finished && summary.working > 0 && summary.total > 1 {
        row.push(Action::Stop(summary.id).button(texts::STOP));
        row.push(Action::disabled_button(texts::RETRY_FAILED));
        row.push(if problems {
            Action::Details(summary.id).button(texts::DETAILS)
        } else {
            Action::disabled_button(texts::DETAILS)
        });
    } else {
        if finished && summary.failed > 0 {
            row.push(Action::Retry(summary.id).button(texts::RETRY_FAILED));
        }
        if problems {
            row.push(Action::Details(summary.id).button(texts::DETAILS));
        }
    }
    let mut rows = Vec::new();
    if !row.is_empty() {
        rows.push(row);
    }
    if let Some(url) = app {
        rows.push(vec![super::mini_app_button(texts::OPEN_IN_APP, &url)]);
    }
    Some(json!({ "inline_keyboard": rows }))
}

/// "Which song is this?" for songs sent from Telegram that wait in review.
/// Owners with a Mini App get one button that opens the app on that song;
/// guests (who can't open the web page) and chats without an HTTPS address get
/// up to three candidates in the chat.
async fn ask_reviews(context: &Context) -> Result<()> {
    for question in store::unasked_reviews(&context.state.pool).await? {
        let song = match &question.artist {
            Some(artist) => format!("{artist} – {}", question.title),
            None => question.title.clone(),
        };
        let (answer, rows) =
            match super::mini_app_track_url(&context.state.config.public_url, question.track_id)
                .filter(|_| context.config.owners.contains(&question.chat_id))
            {
                Some(url) => (
                    texts::ReviewAnswer::InApp,
                    vec![vec![super::mini_app_button(texts::CHOOSE_IN_APP, &url)]],
                ),
                None => chat_choices(context, &question).await?,
            };
        let text = texts::review_question(&song, question.reason.as_deref(), &answer);
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

/// Candidate buttons for chats without a Mini App.
async fn chat_choices(
    context: &Context,
    question: &store::ReviewQuestion,
) -> Result<(texts::ReviewAnswer, Vec<Vec<Value>>)> {
    let candidates: Vec<Choice> = sqlx::query_as(
        "SELECT id,title,artist,album,year FROM candidates WHERE track_id=?
         ORDER BY score DESC LIMIT 3",
    )
    .bind(question.track_id)
    .fetch_all(&context.state.pool)
    .await?;
    let mut rows: Vec<Vec<Value>> = candidates
        .iter()
        .map(|choice| {
            let pick = Action::Pick {
                track: question.track_id,
                candidate: choice.id,
            };
            vec![pick.button(&shorten(&choice.label(), 60))]
        })
        .collect();
    rows.push(vec![
        Action::Drop(question.track_id).button(texts::NONE_OF_THESE),
    ]);
    let answer = if candidates.is_empty() {
        texts::ReviewAnswer::NothingFound
    } else {
        texts::ReviewAnswer::InChat
    };
    Ok((answer, rows))
}

/// A candidate offered as an answer button.
#[derive(sqlx::FromRow)]
struct Choice {
    id: i64,
    title: String,
    artist: String,
    album: Option<String>,
    year: Option<String>,
}

impl Choice {
    /// "Artist – Title · Album (Year)".
    fn label(&self) -> String {
        let mut label = format!("{} – {}", self.artist, self.title);
        if let Some(album) = self.album.as_deref().filter(|album| !album.is_empty()) {
            label.push_str(&format!(" · {album}"));
        }
        if let Some(year) = self.year.as_deref().and_then(|year| year.get(..4)) {
            label.push_str(&format!(" ({year})"));
        }
        label
    }
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
        // Busy: Stop works, Retry and Details are greyed out.
        let busy = keyboard(&summary, false, None).unwrap();
        let row = &busy["inline_keyboard"][0];
        assert_eq!(row[0]["callback_data"], "stop:7");
        assert_eq!(row[0]["style"], "danger");
        assert!(row[1]["disabled"].is_object() && row[2]["disabled"].is_object());
        // Done with a failure: Retry and Details work, Stop is gone.
        summary.working = 0;
        summary.failed = 1;
        let done = keyboard(&summary, true, None).unwrap().to_string();
        assert!(done.contains("retry:7") && done.contains("details:7") && !done.contains("stop"));
        assert!(!done.contains("disabled"));
        // All fine, with the app: only the app button.
        summary.failed = 0;
        let app = keyboard(&summary, true, Some("https://m.example/".into())).unwrap();
        assert_eq!(app["inline_keyboard"].as_array().unwrap().len(), 1);
        assert_eq!(
            app["inline_keyboard"][0][0]["web_app"]["url"],
            "https://m.example/"
        );
    }

    #[test]
    fn long_labels_are_shortened() {
        assert_eq!(shorten("abc", 5), "abc");
        assert_eq!(shorten("abcdefgh", 5), "abcd…");
    }
}
