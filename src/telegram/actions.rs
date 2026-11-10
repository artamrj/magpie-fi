//! Everything a person can press. Button `callback_data` is written and read
//! here only, so a button and the code that handles it can't drift apart.
//! Widget taps (Bot API 10.4) will send the same short codes.

use super::{Context, Role, answer, edit, role, send, texts};
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

    /// An inline button that sends this action.
    pub fn button(self, text: &str) -> Value {
        json!({"text": text, "callback_data": self.to_string()})
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
            let stopped = store::stop_request(pool, request).await?;
            answer(context, &query.id, &texts::stopped(stopped)).await;
        }
        Action::Retry(request) => {
            let queued = store::retry_failed(pool, request).await?;
            store::reopen(pool, request).await?;
            context.state.notify_acquire();
            answer(context, &query.id, &texts::retrying(queued)).await;
        }
        Action::Details(request) => {
            answer(context, &query.id, "").await;
            if let Some((chat, message)) = message {
                let songs = store::request_songs(pool, request).await?;
                send(context, chat, &texts::details(&songs), Some(message)).await;
            }
        }
        Action::Pick { track, candidate } => {
            let reply = pick_candidate(context, track, candidate).await;
            answer(context, &query.id, "").await;
            if let Some((chat, message)) = message {
                edit(context, chat, message, reply, None).await;
            }
        }
        Action::Drop(track) => {
            let removed = crate::http::handlers::remove_track(
                axum::extract::State(context.state.clone()),
                axum::extract::Path(TrackId(track)),
            )
            .await
            .is_ok();
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
                edit(context, chat, message, &text, Some(keyboard)).await;
            }
        }
    }
    Ok(())
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
