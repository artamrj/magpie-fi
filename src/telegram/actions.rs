//! Everything a person can press. Button `callback_data` is written and read
//! here only, so a button and the code that handles it can't drift apart.
//! Taps on a request card (a Bot API 10.4 widget) send the same short codes
//! and go through the same permission check.

use super::{
    Context, Role, To, answer, edit, edit_rich, role, say, send, texts, types::WidgetQuery,
};
use crate::{
    acquire::store,
    types::{CandidateId, TrackId},
};
use anyhow::Result;
use serde_json::{Value, json};
use std::{fmt, sync::Arc};

/// Telegram allows at most 64 bytes of button data (and of widget queries).
const MAX_DATA: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    /// Stop the songs of a request that have not started.
    Stop(i64),
    /// Try the failed songs of a request again.
    Retry(i64),
    /// List the songs of a request that failed or need someone.
    Details(i64),
    /// Choose a candidate for a song waiting in Review.
    Pick { track: i64, candidate: i64 },
    /// None of the candidates: remove the song.
    Drop(i64),
    /// Make a one-time invite link (owners).
    Invite,
    /// Remove a guest (owners).
    Revoke(i64),
}

impl fmt::Display for Action {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stop(request) => write!(formatter, "stop:{request}"),
            Self::Retry(request) => write!(formatter, "retry:{request}"),
            Self::Details(request) => write!(formatter, "details:{request}"),
            Self::Pick { track, candidate } => write!(formatter, "pick:{track}:{candidate}"),
            Self::Drop(track) => write!(formatter, "drop:{track}"),
            Self::Invite => write!(formatter, "invite"),
            Self::Revoke(user) => write!(formatter, "revoke:{user}"),
        }
    }
}

impl Action {
    /// Reads button data. Anything unknown or malformed is `None`.
    pub fn parse(data: &str) -> Option<Self> {
        if data.len() > MAX_DATA {
            return None;
        }
        let mut parts = data.split(':');
        let name = parts.next()?;
        let mut number = || parts.next().and_then(|part| part.parse::<i64>().ok());
        let action = match name {
            "stop" => Self::Stop(number()?),
            "retry" => Self::Retry(number()?),
            "details" => Self::Details(number()?),
            "pick" => Self::Pick {
                track: number()?,
                candidate: number()?,
            },
            "drop" => Self::Drop(number()?),
            "invite" => Self::Invite,
            "revoke" => Self::Revoke(number()?),
            _ => return None,
        };
        // Nothing may follow the expected parts.
        (number().is_none() && action.to_string() == data).then_some(action)
    }

    /// An inline button that sends this action, coloured by what it does.
    pub fn button(self, text: &str) -> Value {
        let mut button = json!({"text": text, "callback_data": self.to_string()});
        if let Some(style) = self.style() {
            button["style"] = json!(style);
        }
        button
    }

    /// Bot API 9.4 button colours: green accepts, red stops or removes, blue is
    /// the main next step. Plain buttons only show something.
    fn style(self) -> Option<&'static str> {
        match self {
            Self::Pick { .. } | Self::Invite => Some("success"),
            Self::Stop(_) | Self::Drop(_) | Self::Revoke(_) => Some("danger"),
            Self::Retry(_) => Some("primary"),
            Self::Details(_) => None,
        }
    }

    /// The same button greyed out: shown but does nothing (Bot API 10.3), so a
    /// row of buttons keeps its shape while a request is busy.
    pub fn disabled_button(text: &str) -> Value {
        json!({"text": text, "disabled": {}})
    }

    /// Owners may press anything; guests only buttons of their own requests.
    async fn allowed(self, context: &Context, user: i64) -> Result<bool> {
        let role = role(context, user).await;
        if role == Role::Owner {
            return Ok(true);
        }
        if role != Role::Guest {
            return Ok(false);
        }
        let user = user.to_string();
        let pool = &context.state.pool;
        Ok(match self {
            Self::Stop(request) | Self::Retry(request) | Self::Details(request) => {
                store::requester_of_request(pool, request).await?.as_deref() == Some(user.as_str())
            }
            Self::Pick { track, .. } | Self::Drop(track) => store::requester_of_track(pool, track)
                .await?
                .is_some_and(|(_, requester)| requester == user),
            Self::Invite | Self::Revoke(_) => false,
        })
    }
}

/// A pressed button.
pub async fn on_button(context: &Arc<Context>, query: &super::types::CallbackQuery) -> Result<()> {
    let Some(action) = query.data.as_deref().and_then(Action::parse) else {
        answer(context, &query.id, "").await;
        return Ok(());
    };
    if !action.allowed(context, query.from.id).await? {
        answer(context, &query.id, texts::NOT_ALLOWED).await;
        return Ok(());
    }
    let message = query
        .message
        .as_ref()
        .map(|message| (message.chat.id, message.message_id));
    let pool = &context.state.pool;
    match action {
        Action::Stop(request) => {
            answer(context, &query.id, &stop(context, request).await?).await;
        }
        Action::Retry(request) => {
            answer(context, &query.id, &retry(context, request).await?).await;
        }
        Action::Details(request) => {
            // Only the person who pressed sees it, so the chat stays tidy; it
            // can be pressed again at any time.
            if let Some((chat, message)) = message {
                let songs = store::request_songs(pool, request).await?;
                let to = To::Presser {
                    chat,
                    query: query.id.clone(),
                    message,
                };
                say(context, &to, &texts::details(&songs), None).await;
            }
            answer(context, &query.id, "").await;
        }
        Action::Pick { track, candidate } => {
            let reply = pick_candidate(context, track, candidate).await;
            answer(context, &query.id, "").await;
            if let Some((chat, message)) = message {
                edit(context, chat, message, reply, None).await;
            }
        }
        Action::Drop(track) => {
            let removed = remove_song(context, track).await;
            answer(context, &query.id, "").await;
            if let Some((chat, message)) = message {
                let text = if removed {
                    texts::REMOVED_SONG
                } else {
                    texts::SONG_ALREADY_GONE
                };
                edit(context, chat, message, text, None).await;
            }
        }
        Action::Invite => {
            let link = super::commands::create_invite(context, query.from.id).await?;
            answer(context, &query.id, "").await;
            if let Some((chat, _)) = message {
                send(context, chat, &texts::invite_link(&link), None).await;
            }
        }
        Action::Revoke(user) => {
            sqlx::query("DELETE FROM telegram_guests WHERE user_id=?")
                .bind(user)
                .execute(pool)
                .await?;
            answer(context, &query.id, texts::REMOVED_GUEST).await;
            if let Some((chat, message)) = message {
                let (text, keyboard) = super::commands::guests_view(context).await?;
                edit_rich(context, chat, message, &text, Some(keyboard)).await;
            }
        }
    }
    Ok(())
}

/// Stops a request's songs that have not started; returns the reply.
async fn stop(context: &Context, request: i64) -> Result<String> {
    let stopped = store::stop_request(&context.state.pool, request).await?;
    Ok(texts::stopped(stopped))
}

/// Tries a request's failed songs again; returns the reply.
async fn retry(context: &Context, request: i64) -> Result<String> {
    let pool = &context.state.pool;
    let queued = store::retry_failed(pool, request).await?;
    store::reopen(pool, request).await?;
    context.state.notify_acquire();
    Ok(texts::retrying(queued))
}

/// A tap on a request card. The card shows the answer itself (`m` a short
/// message, `d` rows for Details), and the status loop redraws the card. Every
/// tap is answered, even a refused or failed one, or the card waits 15 s.
pub async fn on_card_tap(context: &Arc<Context>, query: &WidgetQuery) -> Result<()> {
    let reply = match card_reply(context, query).await {
        Ok(reply) => reply,
        Err(error) => {
            tracing::warn!(%error, "handling a card tap failed");
            json!({ "m": texts::TAP_FAILED }).to_string()
        }
    };
    let _: Option<bool> = context
        .bot
        .try_call(
            "answerWidgetQuery",
            &json!({"widget_query_id": query.id, "data": reply}),
        )
        .await;
    Ok(())
}

async fn card_reply(context: &Arc<Context>, query: &WidgetQuery) -> Result<String> {
    let message = |text: &str| json!({ "m": text }).to_string();
    let Some(action) = Action::parse(&query.data) else {
        return Ok(message(""));
    };
    if !action.allowed(context, query.from.id).await? {
        return Ok(message(texts::NOT_ALLOWED));
    }
    Ok(match action {
        Action::Stop(request) => message(&stop(context, request).await?),
        Action::Retry(request) => message(&retry(context, request).await?),
        Action::Details(request) => {
            let songs = store::request_songs(&context.state.pool, request).await?;
            super::cards::details_answer(&songs)
        }
        // From a review card. Choosing can take longer than the 15 s a card
        // waits for an answer (it may look for a cover), so the answer comes
        // now and the card shows how it ended when it's done.
        Action::Pick { track, candidate } => {
            let context = context.clone();
            tokio::spawn(async move {
                if let Err(error) = pick_on_card(&context, track, candidate).await {
                    tracing::warn!(%error, "choosing from a review card failed");
                }
            });
            message(texts::SAVING_CHOICE)
        }
        Action::Drop(track) => {
            let card = store::review_card_of_track(&context.state.pool, track).await?;
            let removed = remove_song(context, track).await;
            let (state, words) = if removed {
                ("removed", texts::REVIEW_REMOVED)
            } else {
                ("gone", texts::SONG_ALREADY_GONE)
            };
            if let Some(card) = card {
                super::status::end_review_card(context, &card, state, words, None).await?;
            }
            message(words)
        }
        // Cards don't offer the guest buttons.
        Action::Invite | Action::Revoke(_) => message(""),
    })
}

/// A choice made on a review card; the card then shows how it went.
async fn pick_on_card(context: &Context, track: i64, candidate: i64) -> Result<()> {
    let card = store::review_card_of_track(&context.state.pool, track).await?;
    let reply = pick_candidate(context, track, candidate).await;
    let Some(card) = card else {
        return Ok(());
    };
    let (state, app) = match reply {
        texts::PICKED => ("filed", None),
        // Saved, but the cover is missing: finish it in the app.
        texts::PICKED_NO_COVER => (
            "cover",
            super::mini_app_track_url(&context.state.config.public_url, track)
                .filter(|_| context.config.owners.contains(&card.chat_id)),
        ),
        _ => ("gone", None),
    };
    super::status::end_review_card(context, &card, state, reply, app).await
}

/// "None of these": the song waiting in Review is removed, the same as Remove
/// on the web page. False when it was no longer there.
async fn remove_song(context: &Context, track: i64) -> bool {
    crate::http::handlers::remove_track(
        axum::extract::State(context.state.clone()),
        axum::extract::Path(TrackId(track)),
    )
    .await
    .is_ok()
}

/// Selects a candidate for a song waiting in review; returns the reply.
async fn pick_candidate(context: &Context, track: i64, candidate: i64) -> &'static str {
    let result = crate::http::handlers::select_candidate(
        axum::extract::State(context.state.clone()),
        axum::extract::Path(TrackId(track)),
        axum::Json(crate::http::handlers::SelectRequest {
            candidate_id: Some(CandidateId(candidate)),
        }),
    )
    .await;
    match result {
        Ok(axum::Json(value)) if value["ready"] == true => {
            context.state.notify_automation_scheduler();
            texts::PICKED
        }
        Ok(_) => texts::PICKED_NO_COVER,
        Err(_) => texts::PICK_GONE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_action_reads_back_the_same() {
        let actions = [
            Action::Stop(7),
            Action::Retry(7),
            Action::Details(7),
            Action::Pick {
                track: 12,
                candidate: 99,
            },
            Action::Drop(12),
            Action::Invite,
            Action::Revoke(200),
        ];
        for action in actions {
            assert_eq!(Action::parse(&action.to_string()), Some(action));
        }
    }

    #[test]
    fn buttons_are_coloured_by_what_they_do() {
        assert_eq!(Action::Drop(1).button("x")["style"], "danger");
        assert_eq!(
            Action::Pick {
                track: 1,
                candidate: 2
            }
            .button("x")["style"],
            "success"
        );
        assert!(Action::Details(1).button("x").get("style").is_none());
        let off = Action::disabled_button("x");
        assert!(off.get("callback_data").is_none() && off["disabled"] == json!({}));
    }

    #[test]
    fn junk_is_refused() {
        for data in [
            "",
            "stop",
            "stop:",
            "stop:x",
            "stop:1:2",
            "pick:1",
            "invite:1",
            "launch:1",
            "stop:+1",
            "stop:01",
            &format!("stop:{}", "1".repeat(70)),
        ] {
            assert_eq!(Action::parse(data), None, "{data:?}");
        }
    }
}
