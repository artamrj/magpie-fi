//! The Telegram bot, inside magpie. It reads messages with long polling,
//! turns links and audio files into requests (the same queue the web page
//! uses), and keeps one status message per request up to date (`status.rs`).

pub mod api;
mod status;
pub mod texts;

use crate::{
    acquire::{links, store, worker},
    app::AppState,
    config::TelegramConfig,
    types::{CandidateId, TrackId},
};
use anyhow::Result;
use api::Bot;
use frankenstein::{
    types::{CallbackQuery, ChatType, MaybeInaccessibleMessage, Message, User},
    updates::{Update, UpdateContent},
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

/// Audio files sent within this time join the same request (a forwarded album).
const UPLOAD_BATCH_WINDOW: Duration = Duration::from_secs(3);

pub struct Context {
    pub state: Arc<AppState>,
    pub bot: Bot,
    pub config: TelegramConfig,
    pub username: String,
    /// (chat, user) → the upload request still open for more files.
    uploads: Mutex<HashMap<(i64, u64), (i64, Instant)>>,
}

/// Runs the bot until magpie stops. Does nothing without a bot token.
pub async fn run(state: Arc<AppState>) {
    let Some(config) = state.config.read().await.telegram.clone() else {
        return;
    };
    let bot = match Bot::new(&config) {
        Ok(bot) => bot,
        Err(error) => {
            tracing::error!(%error, "Telegram client could not start");
            return;
        }
    };
    let me = loop {
        match bot.call::<_, User>("getMe", &json!({})).await {
            Ok(me) => break me,
            Err(error) => {
                let wrong_token = error
                    .downcast_ref::<api::TelegramError>()
                    .is_some_and(|error| error.code == 401 || error.code == 404);
                if wrong_token {
                    tracing::error!(
                        "Telegram refused MAGPIE_TELEGRAM_BOT_TOKEN; check the token from @BotFather"
                    );
                } else {
                    tracing::warn!(%error, "Telegram is not reachable yet; trying again in 30 s");
                }
                tokio::time::sleep(Duration::from_secs(if wrong_token { 300 } else { 30 })).await;
            }
        }
    };
    let username = me.username.unwrap_or_default();
    tracing::info!(bot = %username, "Telegram bot is listening");
    let _: Option<bool> = bot
        .try_call("deleteWebhook", &json!({"drop_pending_updates": false}))
        .await;
    let _: Option<bool> = bot
        .try_call(
            "setMyCommands",
            &json!({"commands": [
                {"command": "help", "description": "What I can do"},
                {"command": "queue", "description": "What is on the way"},
                {"command": "health", "description": "Is everything working"},
                {"command": "guests", "description": "Invite people (owners)"},
                {"command": "web", "description": "Log in to the web page (owners)"},
                {"command": "id", "description": "Your Telegram ID"},
            ]}),
        )
        .await;
    // Owners get a "magpie" button next to the message box that opens the web page.
    if let Some(url) = mini_app_url(&state.config.read().await.public_url, "") {
        for owner in &config.owners {
            let _: Option<bool> = bot
                .try_call(
                    "setChatMenuButton",
                    &json!({"chat_id": owner, "menu_button": {
                        "type": "web_app", "text": "magpie", "web_app": {"url": url}
                    }}),
                )
                .await;
        }
    }
    let context = Arc::new(Context {
        state,
        bot,
        config,
        username,
        uploads: Mutex::new(HashMap::new()),
    });
    tokio::spawn(status::run(context.clone()));
    poll(context).await;
}

async fn poll(context: Arc<Context>) {
    let mut offset: i64 = read_state(&context.state.pool, "update_offset")
        .await
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    loop {
        let updates: Vec<Update> = match context
            .bot
            .call(
                "getUpdates",
                &json!({"offset": offset, "timeout": 50, "allowed_updates": ["message", "callback_query"]}),
            )
            .await
        {
            Ok(updates) => updates,
            Err(error) => {
                tracing::warn!(%error, "reading Telegram updates failed; trying again");
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
        };
        for update in updates {
            offset = i64::from(update.update_id) + 1;
            write_state(&context.state.pool, "update_offset", &offset.to_string()).await;
            let result = match update.content {
                UpdateContent::Message(message) => on_message(&context, &message).await,
                UpdateContent::CallbackQuery(query) => on_button(&context, &query).await,
                _ => Ok(()),
            };
            if let Err(error) = result {
                tracing::warn!(%error, "handling a Telegram update failed");
            }
        }
    }
}

// ── Who may use the bot ─────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Role {
    Owner,
    Guest,
    Stranger,
}

async fn role(context: &Context, user: u64) -> Role {
    let id = i64::try_from(user).unwrap_or(-1);
    if context.config.owners.contains(&id) {
        return Role::Owner;
    }
    let guest: Option<i64> =
        sqlx::query_scalar("SELECT user_id FROM telegram_guests WHERE user_id=?")
            .bind(id)
            .fetch_optional(&context.state.pool)
            .await
            .unwrap_or(None);
    if guest.is_some() {
        Role::Guest
    } else {
        Role::Stranger
    }
}

// ── Messages ────────────────────────────────────────────────────────────────

async fn on_message(context: &Arc<Context>, message: &Message) -> Result<()> {
    let Some(user) = message.from.as_deref() else {
        return Ok(());
    };
    if message.chat.type_field != ChatType::Private {
        return Ok(());
    }
    let chat = message.chat.id;
    let text = message
        .text
        .clone()
        .or_else(|| message.caption.clone())
        .unwrap_or_default();

    if let Some(command) = text.strip_prefix('/') {
        let (name, argument) = command
            .split_once(char::is_whitespace)
            .map_or((command, ""), |(name, rest)| (name, rest.trim()));
        let name = name.split('@').next().unwrap_or(name);
        return on_command(context, message, user, name, argument).await;
    }

    let role = role(context, user.id).await;
    if role == Role::Stranger {
        send(context, chat, &texts::welcome(false, user.id as i64), None).await;
        return Ok(());
    }
    if let Some(file) = audio_file(message) {
        return on_upload(context, message, user, file).await;
    }
    let urls = find_urls(&text);
    if urls.is_empty() {
        send(
            context,
            chat,
            "Send me a song, album or playlist link, or an audio file. /help",
            None,
        )
        .await;
        return Ok(());
    }
    for url in urls {
        let expanded = links::expand_short_link(&context.state.client, &url)
            .await
            .unwrap_or(url.clone());
        let Some(link) = links::classify(&expanded) else {
            send(
                context,
                chat,
                &format!("I can't read this link: {}", texts::escape(&url)),
                Some(message.message_id),
            )
            .await;
            continue;
        };
        let request = store::create_request(
            &context.state.pool,
            &link.url,
            link.scope,
            "telegram",
            Some(&user.id.to_string()),
        )
        .await?;
        attach_status_message(context, request, message).await?;
        tokio::spawn(worker::read_request(context.state.clone(), request, link));
    }
    react(context, chat, message.message_id, "👀").await;
    Ok(())
}

/// Sends the first version of a request's status message and remembers it.
async fn attach_status_message(context: &Context, request: i64, message: &Message) -> Result<()> {
    let sent = send(
        context,
        message.chat.id,
        "🔎 Reading the link…",
        Some(message.message_id),
    )
    .await;
    if let Some(sent) = sent {
        store::attach_telegram(
            &context.state.pool,
            request,
            message.chat.id,
            message.message_id,
            sent.message_id,
        )
        .await?;
    }
    Ok(())
}

struct AudioFile {
    id: String,
    name: String,
    title: Option<String>,
    artist: Option<String>,
    duration: Option<f64>,
    size: Option<u64>,
}

const AUDIO_EXTENSIONS: [&str; 10] = [
    "aac", "aiff", "aif", "alac", "flac", "m4a", "mp3", "ogg", "opus", "wav",
];

fn audio_file(message: &Message) -> Option<AudioFile> {
    if let Some(audio) = message.audio.as_deref() {
        return Some(AudioFile {
            id: audio.file_id.clone(),
            name: audio
                .file_name
                .clone()
                .unwrap_or_else(|| "audio.mp3".into()),
            title: audio.title.clone(),
            artist: audio.performer.clone(),
            duration: (audio.duration > 0).then_some(f64::from(audio.duration)),
            size: audio.file_size,
        });
    }
    let document = message.document.as_deref()?;
    let name = document.file_name.clone().unwrap_or_default();
    let extension = std::path::Path::new(&name)
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    let looks_like_audio = document
        .mime_type
        .as_deref()
        .is_some_and(|mime| mime.starts_with("audio/"))
        || AUDIO_EXTENSIONS.contains(&extension.as_str());
    looks_like_audio.then(|| AudioFile {
        id: document.file_id.clone(),
        name,
        title: None,
        artist: None,
        duration: None,
        size: document.file_size,
    })
}

async fn on_upload(
    context: &Arc<Context>,
    message: &Message,
    user: &User,
    file: AudioFile,
) -> Result<()> {
    let chat = message.chat.id;
    if context.bot.is_cloud()
        && file
            .size
            .is_some_and(|size| size > api::CLOUD_DOWNLOAD_LIMIT)
    {
        send(
            context,
            chat,
            "This file is bigger than 20 MB, the most Telegram lets bots download. Start the \
             <code>tgapi</code> helper (see README) to accept files up to 2 GB.",
            Some(message.message_id),
        )
        .await;
        return Ok(());
    }
    // Files forwarded together join one request.
    let key = (chat, user.id);
    let request = {
        let mut uploads = context.uploads.lock().await;
        match uploads.get(&key) {
            Some((request, last)) if last.elapsed() < UPLOAD_BATCH_WINDOW => {
                let request = *request;
                uploads.insert(key, (request, Instant::now()));
                request
            }
            _ => {
                let request =
                    store::create_upload_request(&context.state.pool, &user.id.to_string()).await?;
                uploads.insert(key, (request, Instant::now()));
                attach_status_message(context, request, message).await?;
                react(context, chat, message.message_id, "👀").await;
                request
            }
        }
    };
    let title = file.title.clone().unwrap_or_else(|| {
        std::path::Path::new(&file.name)
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| "audio".into())
    });
    let wanted = store::add_upload(
        &context.state.pool,
        request,
        &title,
        file.artist.as_deref(),
        file.duration,
    )
    .await?;
    let context = context.clone();
    tokio::spawn(async move {
        let config = context.state.config.read().await.clone();
        let extension = std::path::Path::new(&file.name)
            .extension()
            .and_then(|value| value.to_str())
            .map(str::to_ascii_lowercase)
            .filter(|extension| AUDIO_EXTENSIONS.contains(&extension.as_str()))
            .unwrap_or_else(|| "mp3".into());
        let target = config.staging_dir.join(format!("u{wanted}.{extension}"));
        let part = config
            .staging_dir
            .join(".part")
            .join(format!("u{wanted}.{extension}"));
        let result = async {
            tokio::fs::create_dir_all(part.parent().unwrap_or(&config.staging_dir)).await?;
            context.bot.download(&file.id, &part).await?;
            tokio::fs::rename(&part, &target).await?;
            store::mark_identifying(&context.state.pool, wanted, &target, None).await
        }
        .await;
        match result {
            Ok(()) => context.state.notify_automation_scheduler(),
            Err(error) => {
                let _ = tokio::fs::remove_file(&part).await;
                let _ =
                    store::mark_failed(&context.state.pool, wanted, &format!("{error:#}")).await;
            }
        }
    });
    Ok(())
}

/// `http(s)://…` links in a message, without trailing punctuation, in order.
pub fn find_urls(text: &str) -> Vec<String> {
    let mut urls: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        let Some(start) = word.find("http://").or_else(|| word.find("https://")) else {
            continue;
        };
        let url = word[start..]
            .trim_end_matches(['.', ',', ';', ')', ']', '>', '!', '?', '"', '\''])
            .to_owned();
        if !urls.contains(&url) {
            urls.push(url);
        }
    }
    urls
}

// ── Commands ────────────────────────────────────────────────────────────────

async fn on_command(
    context: &Arc<Context>,
    message: &Message,
    user: &User,
    name: &str,
    argument: &str,
) -> Result<()> {
    let chat = message.chat.id;
    let user_id = i64::try_from(user.id).unwrap_or(-1);
    if name == "id" {
        send(
            context,
            chat,
            &format!("Your Telegram ID: <code>{user_id}</code>"),
            None,
        )
        .await;
        return Ok(());
    }
    if name == "start"
        && let Some(token) = argument.strip_prefix("invite_")
    {
        return redeem_invite(context, chat, user, token).await;
    }
    let role = role(context, user.id).await;
    if role == Role::Stranger {
        send(context, chat, &texts::welcome(false, user_id), None).await;
        return Ok(());
    }
    match name {
        "start" => send(context, chat, &texts::welcome(true, user_id), None).await,
        "help" => send(context, chat, &texts::help(role == Role::Owner), None).await,
        "queue" => send(context, chat, &queue_text(context).await?, None).await,
        "health" => send(context, chat, &health_text(context).await, None).await,
        "seven" => send(context, chat, &seven_text(context).await?, None).await,
        "web" if role == Role::Owner => {
            let public_url = context.state.config.read().await.public_url.clone();
            let base = public_url
                .clone()
                .unwrap_or_else(|| "http://127.0.0.1:7331".into());
            let link =
                crate::http::auth::create_login_link(&context.state.pool, user_id, &base).await?;
            let text = format!(
                "Your login link for the web page. It works once, for 10 minutes, and keeps \
                 that browser logged in for 30 days:\n{}{}",
                texts::escape(&link),
                if public_url.is_none() {
                    "\n\nOpen it on the server itself (MAGPIE_PUBLIC_URL is not set)."
                } else {
                    ""
                }
            );
            let keyboard = public_url
                .is_some()
                .then(|| json!({"inline_keyboard": [[{"text": "🌐 Open magpie", "url": link}]]}));
            send_with(context, chat, &text, None, keyboard).await
        }
        "guests" if role == Role::Owner => {
            let (text, keyboard) = guests_view(context).await?;
            send_with(context, chat, &text, None, Some(keyboard)).await
        }
        _ => send(context, chat, "I don't know that command. /help", None).await,
    };
    Ok(())
}

async fn queue_text(context: &Context) -> Result<String> {
    let requests = store::list_requests(&context.state.pool, 30).await?;
    let active: Vec<_> = requests
        .iter()
        .filter(|request| request.status == "reading" || request.working > 0)
        .collect();
    if active.is_empty() {
        return Ok("Nothing on the way. 🐦".into());
    }
    let mut text = String::from("<b>On the way</b>\n");
    for request in active.iter().take(10) {
        let name = request
            .title
            .clone()
            .unwrap_or_else(|| request.kind.clone());
        text.push_str(&format!(
            "• {} — {} of {} done, {} on the way\n",
            texts::escape(&name),
            request.done,
            request.total,
            request.working
        ));
    }
    Ok(text)
}

async fn health_text(context: &Context) -> String {
    let config = context.state.config.read().await.clone();
    let songs: i64 = sqlx::query_scalar("SELECT count(*) FROM library_items")
        .fetch_one(&context.state.pool)
        .await
        .unwrap_or_default();
    let writable = tokio::fs::metadata(&config.output_dir)
        .await
        .is_ok_and(|metadata| !metadata.permissions().readonly());
    let navidrome = match config.navidrome.as_ref() {
        None => "not connected".to_owned(),
        Some(navidrome) => {
            if crate::infrastructure::navidrome::ping(&context.state.client, navidrome)
                .await
                .is_ok()
            {
                "✅ reachable".into()
            } else {
                "⚠️ not reachable".into()
            }
        }
    };
    let check = |ok: bool| if ok { "✅" } else { "⚠️" };
    format!(
        "<b>Health</b>\n\
         Library: {} {songs} songs\n\
         yt-dlp: {}\n\
         Navidrome: {navidrome}",
        check(writable),
        check(crate::acquire::ytdlp::available(&config)),
    )
}

/// 🐦×7 Seven for a secret never to be told: a few numbers and one song you forgot.
async fn seven_text(context: &Context) -> Result<String> {
    let (songs, artists, seconds, first): (i64, i64, Option<f64>, Option<String>) = sqlx::query_as(
        "SELECT count(*),count(DISTINCT lower(artist)),sum(duration),min(added_at)
             FROM library_items",
    )
    .fetch_one(&context.state.pool)
    .await?;
    let months = first
        .and_then(|date| chrono::DateTime::parse_from_rfc3339(&date).ok())
        .map(|date| {
            ((chrono::Utc::now() - date.with_timezone(&chrono::Utc)).num_days() / 30).max(1)
        })
        .unwrap_or(1);
    let forgotten: Option<(Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT artist,title FROM library_items WHERE title IS NOT NULL ORDER BY random() LIMIT 1",
    )
    .fetch_optional(&context.state.pool)
    .await?;
    let mut text = format!(
        "🐦🐦🐦🐦🐦🐦🐦 <i>Seven for a secret never to be told</i>\n\n\
         {songs} songs · {artists} artists · {:.0} hours of music\n\
         About €{} not paid to a streaming subscription so far.",
        seconds.unwrap_or_default() / 3600.0,
        months * 11
    );
    if let Some((artist, Some(title))) = forgotten {
        text.push_str(&format!(
            "\n\nListen again: <b>{}</b>{}",
            texts::escape(&title),
            artist
                .map(|artist| format!(" — {}", texts::escape(&artist)))
                .unwrap_or_default()
        ));
    }
    Ok(text)
}

// ── Guests ──────────────────────────────────────────────────────────────────

async fn guests_view(context: &Context) -> Result<(String, Value)> {
    let guests: Vec<(i64, Option<String>)> =
        sqlx::query_as("SELECT user_id,name FROM telegram_guests ORDER BY created_at")
            .fetch_all(&context.state.pool)
            .await?;
    let mut text = String::from("<b>Guests</b> can send links and files, nothing else.\n");
    let mut rows = vec![vec![
        json!({"text": "➕ Create an invite link", "callback_data": "invite"}),
    ]];
    if guests.is_empty() {
        text.push_str("\nNo guests yet.");
    }
    for (id, name) in guests {
        let label = name.unwrap_or_else(|| id.to_string());
        text.push_str(&format!("\n• {}", texts::escape(&label)));
        rows.push(vec![
            json!({"text": format!("Remove {label}"), "callback_data": format!("revoke:{id}")}),
        ]);
    }
    Ok((text, json!({"inline_keyboard": rows})))
}

async fn redeem_invite(context: &Context, chat: i64, user: &User, token: &str) -> Result<()> {
    let created_by: Option<i64> =
        sqlx::query_scalar("DELETE FROM telegram_invites WHERE token=? RETURNING created_by")
            .bind(token)
            .fetch_optional(&context.state.pool)
            .await?;
    let Some(created_by) = created_by else {
        send(
            context,
            chat,
            "This invite was already used or is not valid.",
            None,
        )
        .await;
        return Ok(());
    };
    let name = user
        .username
        .clone()
        .map(|username| format!("@{username}"))
        .unwrap_or_else(|| user.first_name.clone());
    sqlx::query(
        "INSERT INTO telegram_guests(user_id,name,invited_by,created_at) VALUES(?,?,?,?)
         ON CONFLICT(user_id) DO NOTHING",
    )
    .bind(i64::try_from(user.id).unwrap_or(-1))
    .bind(&name)
    .bind(created_by)
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(&context.state.pool)
    .await?;
    send(
        context,
        chat,
        &format!(
            "Welcome, {}! 🐦\n\n{}",
            texts::escape(&name),
            texts::help(false)
        ),
        None,
    )
    .await;
    Ok(())
}

// ── Buttons ─────────────────────────────────────────────────────────────────

async fn on_button(context: &Arc<Context>, query: &CallbackQuery) -> Result<()> {
    let data = query.data.clone().unwrap_or_default();
    let role = role(context, query.from.id).await;
    let user = query.from.id.to_string();
    let message = match &query.message {
        Some(MaybeInaccessibleMessage::Message(message)) => Some(message.as_ref()),
        _ => None,
    };
    let (action, argument) = data.split_once(':').unwrap_or((data.as_str(), ""));
    let number: i64 = argument
        .split(':')
        .next()
        .unwrap_or("")
        .parse()
        .unwrap_or(-1);

    // Owners may press anything; others only buttons of their own requests.
    let allowed = match action {
        "invite" | "revoke" => role == Role::Owner,
        "stop" | "retry" | "details" => {
            role == Role::Owner
                || (role == Role::Guest
                    && store::requester_of_request(&context.state.pool, number)
                        .await?
                        .as_deref()
                        == Some(user.as_str()))
        }
        "pick" | "drop" => {
            role == Role::Owner
                || (role == Role::Guest
                    && store::requester_of_track(&context.state.pool, number)
                        .await?
                        .is_some_and(|(_, requester)| requester == user))
        }
        _ => false,
    };
    if !allowed {
        answer(context, &query.id, "Not allowed").await;
        return Ok(());
    }

    match action {
        "stop" => {
            let stopped = store::stop_request(&context.state.pool, number).await?;
            answer(
                context,
                &query.id,
                &format!("Stopped {stopped} songs that had not started"),
            )
            .await;
        }
        "retry" => {
            let queued = store::retry_failed(&context.state.pool, number).await?;
            store::reopen(&context.state.pool, number).await?;
            context.state.notify_acquire();
            answer(context, &query.id, &format!("Trying {queued} songs again")).await;
        }
        "details" => {
            answer(context, &query.id, "").await;
            if let Some(message) = message {
                let text = details_text(context, number).await?;
                send(context, message.chat.id, &text, Some(message.message_id)).await;
            }
        }
        "pick" => {
            let candidate: i64 = argument
                .split(':')
                .nth(1)
                .and_then(|value| value.parse().ok())
                .unwrap_or(-1);
            let result = crate::http::handlers::select_candidate(
                axum::extract::State(context.state.clone()),
                axum::extract::Path(TrackId(number)),
                axum::Json(crate::http::handlers::SelectRequest {
                    candidate_id: Some(CandidateId(candidate)),
                }),
            )
            .await;
            let reply = match result {
                Ok(axum::Json(value)) if value["ready"] == true => {
                    context.state.notify_automation_scheduler();
                    "✅ Got it. Filing it now."
                }
                Ok(_) => {
                    "Saved your choice, but no matching cover was found yet. Finish it on the web page under Review."
                }
                Err(_) => "That choice is no longer available. Open Review on the web page.",
            };
            answer(context, &query.id, "").await;
            if let Some(message) = message {
                edit(context, message.chat.id, message.message_id, reply, None).await;
            }
        }
        "drop" => {
            let removed = crate::http::handlers::remove_track(
                axum::extract::State(context.state.clone()),
                axum::extract::Path(TrackId(number)),
            )
            .await;
            answer(context, &query.id, "").await;
            if let Some(message) = message {
                let text = if removed.is_ok() {
                    "🗑 Removed. It won't be added."
                } else {
                    "That song is already gone."
                };
                edit(context, message.chat.id, message.message_id, text, None).await;
            }
        }
        "invite" => {
            let token = invite_token();
            sqlx::query("INSERT INTO telegram_invites(token,created_by,created_at) VALUES(?,?,?)")
                .bind(&token)
                .bind(i64::try_from(query.from.id).unwrap_or(-1))
                .bind(chrono::Utc::now().to_rfc3339())
                .execute(&context.state.pool)
                .await?;
            answer(context, &query.id, "").await;
            if let Some(message) = message {
                send(
                    context,
                    message.chat.id,
                    &format!(
                        "Send this link to your guest. It works once:\nhttps://t.me/{}?start=invite_{token}",
                        context.username
                    ),
                    None,
                )
                .await;
            }
        }
        "revoke" => {
            sqlx::query("DELETE FROM telegram_guests WHERE user_id=?")
                .bind(number)
                .execute(&context.state.pool)
                .await?;
            answer(context, &query.id, "Removed").await;
            if let Some(message) = message {
                let (text, keyboard) = guests_view(context).await?;
                edit(
                    context,
                    message.chat.id,
                    message.message_id,
                    &text,
                    Some(keyboard),
                )
                .await;
            }
        }
        _ => answer(context, &query.id, "").await,
    }
    Ok(())
}

async fn details_text(context: &Context, request: i64) -> Result<String> {
    let songs = store::request_songs(&context.state.pool, request).await?;
    let problems: Vec<_> = songs
        .iter()
        .filter(|song| matches!(song.status.as_str(), "failed" | "review"))
        .collect();
    if problems.is_empty() {
        return Ok("Every song is fine. 🐦".into());
    }
    let mut text = String::new();
    for song in problems.iter().take(30) {
        let what = if song.status == "review" {
            "needs you"
        } else {
            "failed"
        };
        text.push_str(&format!(
            "• <b>{}</b>{} — {what}{}\n",
            texts::escape(&song.title),
            song.artist
                .as_deref()
                .map(|artist| format!(" · {}", texts::escape(artist)))
                .unwrap_or_default(),
            song.error
                .as_deref()
                .map(|error| format!(": {}", texts::escape(error)))
                .unwrap_or_default()
        ));
    }
    if problems.len() > 30 {
        text.push_str(&format!("…and {} more", problems.len() - 30));
    }
    Ok(text)
}

/// The web page as a Telegram Mini App. Telegram only opens HTTPS addresses.
pub fn mini_app_url(public_url: &Option<String>, view: &str) -> Option<String> {
    let url = public_url
        .as_deref()
        .filter(|url| url.starts_with("https://"))?;
    Some(if view.is_empty() {
        format!("{url}/")
    } else {
        format!("{url}/?view={view}")
    })
}

fn invite_token() -> String {
    use sha2::{Digest, Sha256};
    let seed = format!(
        "{:?}-{}-{:?}",
        std::time::SystemTime::now(),
        std::process::id(),
        std::thread::current().id()
    );
    hex::encode(Sha256::digest(seed.as_bytes()))[..20].to_owned()
}

// ── Small helpers ───────────────────────────────────────────────────────────

pub async fn send(
    context: &Context,
    chat: i64,
    text: &str,
    reply_to: Option<i32>,
) -> Option<Message> {
    send_with(context, chat, text, reply_to, None).await
}

pub async fn send_with(
    context: &Context,
    chat: i64,
    text: &str,
    reply_to: Option<i32>,
    keyboard: Option<Value>,
) -> Option<Message> {
    let mut params = json!({
        "chat_id": chat,
        "text": text,
        "parse_mode": "HTML",
        "link_preview_options": {"is_disabled": true},
    });
    if let Some(reply_to) = reply_to {
        params["reply_parameters"] =
            json!({"message_id": reply_to, "allow_sending_without_reply": true});
    }
    if let Some(keyboard) = keyboard {
        params["reply_markup"] = keyboard;
    }
    context.bot.try_call("sendMessage", &params).await
}

pub async fn edit(context: &Context, chat: i64, message: i32, text: &str, keyboard: Option<Value>) {
    let mut params = json!({
        "chat_id": chat,
        "message_id": message,
        "text": text,
        "parse_mode": "HTML",
        "link_preview_options": {"is_disabled": true},
    });
    if let Some(keyboard) = keyboard {
        params["reply_markup"] = keyboard;
    }
    let _: Option<Value> = context.bot.try_call("editMessageText", &params).await;
}

async fn answer(context: &Context, query: &str, text: &str) {
    let _: Option<bool> = context
        .bot
        .try_call(
            "answerCallbackQuery",
            &json!({"callback_query_id": query, "text": text}),
        )
        .await;
}

pub async fn react(context: &Context, chat: i64, message: i32, emoji: &str) {
    let _: Option<bool> = context
        .bot
        .try_call(
            "setMessageReaction",
            &json!({"chat_id": chat, "message_id": message, "reaction": [{"type": "emoji", "emoji": emoji}]}),
        )
        .await;
}

async fn read_state(pool: &sqlx::SqlitePool, key: &str) -> Option<String> {
    sqlx::query_scalar("SELECT value FROM telegram_state WHERE key=?")
        .bind(key)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
}

async fn write_state(pool: &sqlx::SqlitePool, key: &str, value: &str) {
    let _ = sqlx::query(
        "INSERT INTO telegram_state(key,value) VALUES(?,?)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
    )
    .bind(key)
    .bind(value)
    .execute(pool)
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, extract::State as AxumState, routing::post};
    use std::sync::Mutex as StdMutex;

    /// A pretend Telegram: hands out queued updates and records every call.
    #[derive(Default)]
    struct FakeTelegram {
        updates: StdMutex<Vec<Value>>,
        calls: StdMutex<Vec<(String, Value)>>,
        next_message: StdMutex<i32>,
    }

    impl FakeTelegram {
        fn push(&self, update: Value) {
            self.updates.lock().unwrap().push(update);
        }
        fn calls(&self, method: &str) -> Vec<Value> {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .filter(|(name, _)| name == method)
                .map(|(_, params)| params.clone())
                .collect()
        }
    }

    async fn fake_api(
        AxumState(fake): AxumState<Arc<FakeTelegram>>,
        axum::extract::Path((_, method)): axum::extract::Path<(String, String)>,
        Json(params): Json<Value>,
    ) -> Json<Value> {
        fake.calls
            .lock()
            .unwrap()
            .push((method.clone(), params.clone()));
        let result = match method.as_str() {
            "getMe" => {
                json!({"id": 1, "is_bot": true, "first_name": "magpie", "username": "magpie_test_bot"})
            }
            "getUpdates" => {
                let pending: Vec<Value> = fake.updates.lock().unwrap().drain(..).collect();
                if pending.is_empty() {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                json!(pending)
            }
            "sendMessage" | "editMessageText" => {
                let mut next = fake.next_message.lock().unwrap();
                *next += 1;
                json!({"message_id": 1000 + *next, "date": 0, "chat": {"id": params["chat_id"], "type": "private"}, "text": params["text"]})
            }
            _ => json!(true),
        };
        Json(json!({"ok": true, "result": result}))
    }

    fn message(update: u32, user: u64, text: &str) -> Value {
        json!({"update_id": update, "message": {
            "message_id": update, "date": 0,
            "chat": {"id": user, "type": "private"},
            "from": {"id": user, "is_bot": false, "first_name": format!("User{user}")},
            "text": text
        }})
    }

    fn button(update: u32, user: u64, data: &str) -> Value {
        json!({"update_id": update, "callback_query": {
            "id": format!("q{update}"), "chat_instance": "c", "data": data,
            "from": {"id": user, "is_bot": false, "first_name": format!("User{user}")},
            "message": {"message_id": 900, "date": 0, "chat": {"id": user, "type": "private"}, "text": "x"}
        }})
    }

    async fn wait_for(fake: &FakeTelegram, method: &str, contains: &str) -> Value {
        for _ in 0..100 {
            if let Some(found) = fake
                .calls(method)
                .into_iter()
                .find(|params| params.to_string().contains(contains))
            {
                return found;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!(
            "no {method} containing {contains:?}; calls: {:?}",
            fake.calls.lock().unwrap()
        );
    }

    const OWNER: u64 = 100;
    const FRIEND: u64 = 200;

    async fn start() -> (tempfile::TempDir, Arc<FakeTelegram>, Arc<AppState>) {
        let fake = Arc::new(FakeTelegram::default());
        let app = Router::new()
            .route("/{bot}/{method}", post(fake_api))
            .with_state(fake.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let directory = tempfile::tempdir().unwrap();
        let mut config = crate::config::Config::with_folders(
            &directory.path().join("music"),
            &directory.path().join("data"),
        );
        config.telegram = Some(TelegramConfig {
            token: "1:test".into(),
            owners: vec![OWNER as i64],
            api_url: format!("http://{address}"),
        });
        let pool =
            crate::infrastructure::db::connect(directory.path().join("t.sqlite").to_str().unwrap())
                .await
                .unwrap();
        let state = Arc::new(AppState::new(config, pool));
        tokio::spawn(run(state.clone()));
        (directory, fake, state)
    }

    #[tokio::test]
    async fn strangers_owners_and_invited_guests() {
        let (_directory, fake, _state) = start().await;
        wait_for(&fake, "setMyCommands", "queue").await;

        // A stranger is turned away but learns their ID.
        fake.push(message(1, FRIEND, "https://open.spotify.com/track/abc"));
        let reply = wait_for(&fake, "sendMessage", "private music bot").await;
        assert!(reply["text"].as_str().unwrap().contains("200"));

        // The owner gets help (with the rhyme) and creates an invite.
        fake.push(message(2, OWNER, "/help"));
        wait_for(&fake, "sendMessage", "Seven for a secret").await;
        fake.push(message(3, OWNER, "/guests"));
        wait_for(&fake, "sendMessage", "Create an invite link").await;
        fake.push(button(4, OWNER, "invite"));
        let invite = wait_for(&fake, "sendMessage", "start=invite_").await;
        let link = invite["text"].as_str().unwrap();
        assert!(link.contains("https://t.me/magpie_test_bot?start=invite_"));
        let token = link
            .split("start=invite_")
            .nth(1)
            .unwrap()
            .trim()
            .to_owned();

        // The friend redeems it once and becomes a guest; the link then stops working.
        fake.push(message(5, FRIEND, &format!("/start invite_{token}")));
        wait_for(&fake, "sendMessage", "Welcome, User200").await;
        fake.push(message(6, 300, &format!("/start invite_{token}")));
        wait_for(&fake, "sendMessage", "already used").await;

        // Guests cannot manage guests or press owner buttons.
        fake.push(message(7, FRIEND, "/guests"));
        wait_for(&fake, "sendMessage", "don't know that command").await;
        fake.push(button(8, FRIEND, "revoke:200"));
        wait_for(&fake, "answerCallbackQuery", "Not allowed").await;

        // The owner removes the guest again.
        fake.push(button(9, OWNER, "revoke:200"));
        wait_for(&fake, "answerCallbackQuery", "Removed").await;
        // Removed guests are strangers again.
        let turned_away = |fake: &FakeTelegram| {
            fake.calls("sendMessage")
                .iter()
                .filter(|params| {
                    params["text"]
                        .as_str()
                        .unwrap_or("")
                        .contains("private music bot")
                })
                .count()
        };
        let before = turned_away(&fake);
        fake.push(message(10, FRIEND, "/help"));
        for _ in 0..100 {
            if turned_away(&fake) > before {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(turned_away(&fake), before + 1);
    }

    #[tokio::test]
    async fn status_messages_follow_the_request_and_finish_with_the_rhyme() {
        let (directory, fake, state) = start().await;
        wait_for(&fake, "setMyCommands", "queue").await;

        // A playlist request from Telegram with two songs, one of them already filed.
        let pool = &state.pool;
        let request = store::create_request(
            pool,
            "https://x/playlist/1",
            links::Scope::Playlist,
            "telegram",
            Some("100"),
        )
        .await
        .unwrap();
        let collection = links::Collection {
            title: Some("Road Trip".into()),
            owner: None,
            songs: ["A", "B"]
                .iter()
                .map(|title| links::Wanted {
                    title: (*title).into(),
                    artist: Some("Artist".into()),
                    ..links::Wanted::default()
                })
                .collect(),
        };
        store::save_collection(
            pool,
            request,
            "https://x/playlist/1",
            links::Scope::Playlist,
            &collection,
        )
        .await
        .unwrap();
        store::attach_telegram(pool, request, OWNER as i64, 5, 900)
            .await
            .unwrap();
        let first = store::claim_next(pool).await.unwrap().unwrap();
        let file = directory.path().join("a.m4a");
        tokio::fs::write(&file, b"a").await.unwrap();
        store::mark_in_library(pool, first.id, file.to_str().unwrap())
            .await
            .unwrap();

        let busy = wait_for(&fake, "editMessageText", "1 of 2 in your library").await;
        assert!(busy.to_string().contains(&format!("stop:{request}")));

        // The second song is filed: the message turns final and the user's message gets a reaction.
        let second = store::claim_next(pool).await.unwrap().unwrap();
        let staging = directory.path().join("w2.m4a");
        store::mark_identifying(pool, second.id, &staging, None)
            .await
            .unwrap();
        store::finished_download(pool, &staging, &file, "filed")
            .await
            .unwrap();
        let done = wait_for(&fake, "editMessageText", "Two for joy").await;
        assert!(
            done["text"]
                .as_str()
                .unwrap()
                .starts_with("✅ <b>Road Trip</b> · playlist")
        );
        wait_for(&fake, "setMessageReaction", "👌").await;
    }

    #[test]
    fn urls_are_found_in_order_without_punctuation() {
        let text = "listen (https://open.spotify.com/track/1), and https://youtu.be/x! also https://open.spotify.com/track/1";
        assert_eq!(
            find_urls(text),
            vec!["https://open.spotify.com/track/1", "https://youtu.be/x"]
        );
        assert!(find_urls("no links here").is_empty());
    }
}
