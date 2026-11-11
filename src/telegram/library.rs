//! The library card: one shared Bot API 10.4 widget with the library at a
//! glance (`/library`, owners). Every copy, in every owner's chat, shows the
//! same `public_data`, so one edit updates them all. Its ID is kept in
//! `telegram_state` together with a fingerprint of its code: a shared widget
//! keeps the code it was made with, so a changed card makes a new one.

use super::{Card, Context, To, api, cards, read_state, say, status, texts, write_state};
use crate::acquire::store;
use anyhow::Result;
use serde_json::{Value, json};

const STATE_KEY: &str = "library_widget";

/// Copies of the card refresh at least this often (seconds) while open.
const CACHE_SECONDS: u64 = 10;

/// The numbers the card shows.
pub async fn stats(context: &Context) -> Result<cards::LibraryStats> {
    let pool = &context.state.pool;
    let config = &context.state.config;
    let (songs, artists, seconds): (i64, i64, f64) = sqlx::query_as(
        "SELECT count(*),
           count(DISTINCT CASE WHEN coalesce(artist,'')<>'' THEN artist END),
           coalesce(sum(duration),0.0)
         FROM library_items",
    )
    .fetch_one(pool)
    .await?;
    // Files that were already in the library when it was first indexed are not "added".
    let recent: Vec<(String, String)> = sqlx::query_as(
        "SELECT coalesce(title,''),coalesce(artist,'') FROM library_items
         WHERE source<>'existing' ORDER BY added_at DESC LIMIT 3",
    )
    .fetch_all(pool)
    .await?;
    let on_the_way: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM wanted w
         LEFT JOIN tracks t ON t.path=w.staging_path AND w.status='identifying'
         WHERE w.status IN ('pending','downloading')
            OR (w.status='identifying' AND coalesce(t.stage,'') NOT IN ('review','failed'))",
    )
    .fetch_one(pool)
    .await?;
    let in_review: i64 =
        sqlx::query_scalar("SELECT count(*) FROM tracks WHERE stage='review' AND is_missing=0")
            .fetch_one(pool)
            .await?;
    let library_writable = tokio::fs::metadata(&config.output_dir)
        .await
        .is_ok_and(|metadata| !metadata.permissions().readonly());
    #[allow(clippy::cast_possible_truncation)]
    let hours = (seconds / 3600.0) as i64;
    Ok(cards::LibraryStats {
        songs,
        artists,
        hours,
        recent,
        on_the_way,
        in_review,
        library_writable,
        ytdlp: crate::acquire::ytdlp::available(config),
        mini_app: super::mini_app_url(&config.public_url, "").is_some(),
    })
}

/// The shared card's ID, if one was made with the card's current code.
async fn widget_id(context: &Context) -> Option<String> {
    let saved = read_state(&context.state.pool, STATE_KEY).await?;
    let (version, id) = saved.split_once(':')?;
    (version == cards::library_version()).then(|| id.to_owned())
}

/// `/library`: the shared card (made the first time), pinned in this chat.
/// Text instead where the chat can't show cards.
pub async fn send(context: &Context, chat: i64) -> Result<()> {
    let pool = &context.state.pool;
    let stats = stats(context).await?;
    if context.config.cards && store::cards_allowed(pool, chat).await? {
        let keyboard = status::app_keyboard(context, chat, false);
        let mut sent = Card::Refused;
        if let Some(id) = widget_id(context).await {
            let mut params = json!({
                "chat_id": chat,
                "widget_id": id,
                "caption": texts::LIBRARY_CAPTION,
                "parse_mode": "HTML",
            });
            if let Some(keyboard) = &keyboard {
                params["reply_markup"] = keyboard.clone();
            }
            sent = super::send_widget(context, "sendSharedWidget", params).await;
        }
        // No shared card yet, an older card's code, or Telegram forgot it: make one.
        if matches!(sent, Card::Refused) {
            sent = create(context, chat, &stats, keyboard).await;
        }
        match sent {
            Card::Sent(message) => {
                pin(context, chat, message.message_id).await?;
                return Ok(());
            }
            Card::Lost => return Ok(()),
            Card::Refused => store::cards_refused(pool, chat).await?,
        }
    }
    say(context, &To::chat(chat), &texts::library(&stats), None).await;
    Ok(())
}

/// Makes a new shared card in this chat and remembers its ID.
async fn create(
    context: &Context,
    chat: i64,
    stats: &cards::LibraryStats,
    keyboard: Option<Value>,
) -> Card {
    let shown = cards::library_state(stats).to_string();
    let mut params = json!({
        "chat_id": chat,
        "html": cards::LIBRARY_HTML,
        "public_data": shown,
        "caption": texts::LIBRARY_CAPTION,
        "parse_mode": "HTML",
        "is_shared": true,
    });
    if let Some(keyboard) = keyboard {
        params["reply_markup"] = keyboard;
    }
    let sent = super::send_widget(context, "sendWidget", params).await;
    if let Card::Sent(message) = &sent
        && let Some(id) = message
            .widget
            .as_ref()
            .and_then(|widget| widget.id.as_deref())
    {
        let saved = format!("{}:{id}", cards::library_version());
        write_state(&context.state.pool, STATE_KEY, &saved).await;
        *context.library_shown.lock().await = Some(shown);
    }
    sent
}

/// Pins the newest library card quietly, in place of the one before.
async fn pin(context: &Context, chat: i64, message: i32) -> Result<()> {
    let pool = &context.state.pool;
    if let Some(old) = store::library_message(pool, chat).await? {
        let _: Option<bool> = context
            .bot
            .try_call(
                "unpinChatMessage",
                &json!({"chat_id": chat, "message_id": old}),
            )
            .await;
    }
    let _: Option<bool> = context
        .bot
        .try_call(
            "pinChatMessage",
            &json!({"chat_id": chat, "message_id": message, "disable_notification": true}),
        )
        .await;
    store::set_library_message(pool, chat, message).await
}

/// Updates every copy of the card when the numbers changed. Called by the
/// status loop; Telegram allows one shared-widget edit per second.
pub async fn refresh(context: &Context) -> Result<()> {
    if !context.config.cards {
        return Ok(());
    }
    let Some(id) = widget_id(context).await else {
        return Ok(());
    };
    let shown = cards::library_state(&stats(context).await?).to_string();
    let mut last = context.library_shown.lock().await;
    if last.as_deref() == Some(shown.as_str()) {
        return Ok(());
    }
    let params = json!({"widget_id": id, "public_data": shown, "cache_time": CACHE_SECONDS});
    match context
        .bot
        .call::<_, Value>("editSharedWidgetData", &params)
        .await
    {
        Ok(_) => *last = Some(shown),
        Err(error) if api::is_not_modified(&error) => *last = Some(shown),
        Err(error) => {
            tracing::warn!(%error, "updating the library card failed");
            // Telegram said no: wait for the next change instead of asking every few seconds.
            if api::is_refusal(&error) {
                *last = Some(shown);
            }
        }
    }
    Ok(())
}
