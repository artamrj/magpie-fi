//! The Telegram bot, inside magpie-fi. It reads messages with long polling,
//! turns links and audio files into requests (the same queue the web page
//! uses), and keeps one status message per request up to date.
//!
//! - `api.rs`: the Bot API client; `types.rs`: the Bot API types we read.
//! - `intake.rs`: links and audio files; `commands.rs`: `/…` commands and guests.
//! - `actions.rs`: every button, its permission check and what it does.
//! - `status.rs`: the loop that updates status messages and asks about Review.
//! - `texts.rs`: everything the bot says.

mod actions;
pub mod api;
mod commands;
mod intake;
mod status;
pub mod texts;
mod types;

use crate::{app::AppState, config::TelegramConfig};
use api::Bot;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
use types::{Message, Update, User};

pub struct Context {
    pub state: Arc<AppState>,
    pub bot: Bot,
    pub config: TelegramConfig,
    pub username: String,
    /// (chat, user) → the upload request still open for more files. Only
    /// uploads of the last few seconds are kept.
    uploads: Mutex<HashMap<(i64, i64), (i64, Instant)>>,
}

/// Runs the bot until magpie-fi stops. Does nothing without a bot token.
pub async fn run(state: Arc<AppState>) {
    let Some(config) = state.config.telegram.clone() else {
        return;
    };
    match Bot::new(&config.token) {
        Ok(bot) => serve(state, config, bot).await,
        Err(error) => tracing::error!(%error, "Telegram client could not start"),
    }
}

async fn serve(state: Arc<AppState>, config: TelegramConfig, bot: Bot) {
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
    let _: Option<bool> = bot.try_call("setMyCommands", &commands::menu()).await;
    // Owners get a "magpie-fi" button next to the message box that opens the web page.
    if mini_app_url(&state.config.public_url, "").is_some() {
        for owner in &config.owners {
            set_menu_button(&bot, &state.config.public_url, *owner).await;
        }
    } else {
        tracing::warn!(
            "MAGPIE_PUBLIC_URL is not an https:// address, so the bot has no Mini App button; \
             set it to the page's public HTTPS address (for example a Cloudflare Tunnel)"
        );
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
            offset = update.update_id + 1;
            write_state(&context.state.pool, "update_offset", &offset.to_string()).await;
            let result = if let Some(message) = &update.message {
                on_message(&context, message).await
            } else if let Some(query) = &update.callback_query {
                actions::on_button(&context, query).await
            } else {
                Ok(())
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

async fn role(context: &Context, user: i64) -> Role {
    if context.config.owners.contains(&user) {
        return Role::Owner;
    }
    let guest: Option<i64> =
        sqlx::query_scalar("SELECT user_id FROM telegram_guests WHERE user_id=?")
            .bind(user)
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

/// Private chats only: a command, an audio file, or links.
async fn on_message(context: &Arc<Context>, message: &Message) -> anyhow::Result<()> {
    let Some(user) = &message.from else {
        return Ok(());
    };
    if !message.chat.is_private() {
        return Ok(());
    }
    let chat = message.chat.id;
    let text = message
        .text
        .as_deref()
        .or(message.caption.as_deref())
        .unwrap_or_default();

    if let Some(command) = text.strip_prefix('/') {
        let (name, argument) = command
            .split_once(char::is_whitespace)
            .map_or((command, ""), |(name, rest)| (name, rest.trim()));
        let name = name.split('@').next().unwrap_or(name);
        return commands::on_command(context, chat, user, name, argument).await;
    }

    if role(context, user.id).await == Role::Stranger {
        send(context, chat, &texts::welcome(false, user.id), None).await;
        return Ok(());
    }
    if let Some(file) = intake::audio_file(message) {
        return intake::on_upload(context, message, user, file).await;
    }
    let urls = intake::find_urls(text);
    if urls.is_empty() {
        send(context, chat, texts::SEND_SOMETHING, None).await;
        return Ok(());
    }
    intake::on_links(context, message, user, urls).await
}

// ── Mini App ────────────────────────────────────────────────────────────────

/// Shows the "magpie-fi" Mini App button next to the message box in an owner's
/// chat. Telegram keeps it, so this only needs to run again when something changed.
async fn set_menu_button(bot: &Bot, public_url: &Option<String>, chat: i64) {
    let Some(url) = mini_app_url(public_url, "") else {
        return;
    };
    let _: Option<bool> = bot
        .try_call(
            "setChatMenuButton",
            &json!({"chat_id": chat, "menu_button": {
                "type": "web_app", "text": texts::MENU_BUTTON, "web_app": {"url": url}
            }}),
        )
        .await;
}

/// An inline button that opens the page inside Telegram, not in the browser.
pub fn mini_app_button(text: &str, url: &str) -> Value {
    json!({"text": text, "web_app": {"url": url}})
}

/// The Mini App opened straight on one song's screen in Review.
pub fn mini_app_track_url(public_url: &Option<String>, track_id: i64) -> Option<String> {
    mini_app_url(public_url, "review").map(|url| format!("{url}&track={track_id}"))
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
    let _: Option<Value> = context
        .bot
        .try_call(
            "editMessageText",
            &edit_params(chat, message, text, keyboard),
        )
        .await;
}

/// `editMessageText` parameters for an HTML message without link previews.
pub fn edit_params(chat: i64, message: i32, text: &str, keyboard: Option<Value>) -> Value {
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
    params
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
    use crate::acquire::{links, store};
    use axum::{
        Json, Router,
        extract::State as AxumState,
        routing::{get, post},
    };
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
                json!({"id": 1, "is_bot": true, "first_name": "magpie-fi", "username": "magpie_fi_test_bot"})
            }
            "getUpdates" => {
                let pending: Vec<Value> = fake.updates.lock().unwrap().drain(..).collect();
                if pending.is_empty() {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                json!(pending)
            }
            "getFile" => {
                json!({"file_id": params["file_id"], "file_unique_id": "u", "file_path": "music/a.mp3"})
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
        start_with(None).await
    }

    async fn start_with(
        public_url: Option<&str>,
    ) -> (tempfile::TempDir, Arc<FakeTelegram>, Arc<AppState>) {
        let fake = Arc::new(FakeTelegram::default());
        let app = Router::new()
            .route("/{bot}/{method}", post(fake_api))
            .route("/file/{bot}/{*path}", get(|| async { "not really audio" }))
            .with_state(fake.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let directory = tempfile::tempdir().unwrap();
        let mut config = crate::config::Config::with_folders(
            &directory.path().join("music"),
            &directory.path().join("data"),
        );
        let telegram = TelegramConfig {
            token: "1:test".into(),
            owners: vec![OWNER as i64],
        };
        config.telegram = Some(telegram.clone());
        config.public_url = public_url.map(str::to_owned);
        let pool =
            crate::infrastructure::db::connect(directory.path().join("t.sqlite").to_str().unwrap())
                .await
                .unwrap();
        let state = Arc::new(AppState::new(config, pool));
        let bot = Bot::with_api_url(&format!("http://{address}"), &telegram.token).unwrap();
        tokio::spawn(serve(state.clone(), telegram, bot));
        (directory, fake, state)
    }

    #[tokio::test]
    async fn owners_get_mini_app_buttons_that_open_inside_telegram() {
        let (_directory, fake, _state) = start_with(Some("https://music.example")).await;
        let menu = wait_for(&fake, "setChatMenuButton", "web_app").await;
        assert_eq!(menu["chat_id"], json!(OWNER));
        assert_eq!(
            menu["menu_button"]["web_app"]["url"],
            "https://music.example/"
        );

        fake.push(message(1, OWNER, "/start"));
        let welcome = wait_for(&fake, "sendMessage", "Open magpie-fi").await;
        assert_eq!(
            welcome["reply_markup"]["inline_keyboard"][0][0]["web_app"]["url"],
            "https://music.example/"
        );

        fake.push(message(2, OWNER, "/web"));
        let login = wait_for(&fake, "sendMessage", "Open in a browser").await;
        let rows = &login["reply_markup"]["inline_keyboard"];
        assert_eq!(rows[0][0]["web_app"]["url"], "https://music.example/");
        assert!(
            rows[1][0]["url"]
                .as_str()
                .unwrap()
                .starts_with("https://music.example/")
        );
    }

    #[test]
    fn a_review_question_links_to_its_song_in_the_app() {
        let public = Some("https://music.example".to_owned());
        assert_eq!(
            mini_app_track_url(&public, 12).as_deref(),
            Some("https://music.example/?view=review&track=12")
        );
        assert_eq!(
            mini_app_track_url(&Some("http://nas:7331".to_owned()), 12),
            None
        );
    }

    #[tokio::test]
    async fn without_https_there_is_no_mini_app_button() {
        let (_directory, fake, _state) = start().await;
        fake.push(message(1, OWNER, "/start"));
        let welcome = wait_for(&fake, "sendMessage", "/help shows").await;
        assert!(welcome.get("reply_markup").is_none());
        assert!(fake.calls("setChatMenuButton").is_empty());
        fake.push(message(2, OWNER, "/health"));
        wait_for(&fake, "sendMessage", "MAGPIE_PUBLIC_URL").await;
    }

    #[tokio::test]
    async fn strangers_owners_and_invited_guests() {
        let (_directory, fake, _state) = start().await;
        wait_for(&fake, "setMyCommands", "queue").await;

        // A stranger is turned away but learns their ID.
        fake.push(message(1, FRIEND, "https://open.spotify.com/track/abc"));
        let reply = wait_for(&fake, "sendMessage", "private music bot").await;
        assert!(reply["text"].as_str().unwrap().contains("200"));

        // The owner gets help and creates an invite.
        fake.push(message(2, OWNER, "/help"));
        wait_for(&fake, "sendMessage", "/guests").await;
        fake.push(message(3, OWNER, "/guests"));
        wait_for(&fake, "sendMessage", "Create an invite link").await;
        fake.push(button(4, OWNER, "invite"));
        let invite = wait_for(&fake, "sendMessage", "start=invite_").await;
        let link = invite["text"].as_str().unwrap();
        assert!(link.contains("https://t.me/magpie_fi_test_bot?start=invite_"));
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
    async fn status_messages_follow_the_request() {
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
        let done = wait_for(&fake, "editMessageText", "✅").await;
        assert!(
            done["text"]
                .as_str()
                .unwrap()
                .starts_with("✅ <b>Road Trip</b> · playlist")
        );
        wait_for(&fake, "setMessageReaction", "👌").await;
    }

    fn audio(update: u32, user: u64, title: &str, size: u64) -> Value {
        json!({"update_id": update, "message": {
            "message_id": update, "date": 0,
            "chat": {"id": user, "type": "private"},
            "from": {"id": user, "is_bot": false, "first_name": format!("User{user}")},
            "audio": {"file_id": format!("f{update}"), "file_unique_id": "u", "duration": 200,
                      "performer": "Daft Punk", "title": title, "file_name": "x.mp3",
                      "file_size": size}
        }})
    }

    async fn count(pool: &sqlx::SqlitePool, sql: &str) -> i64 {
        sqlx::query_scalar(sqlx::AssertSqlSafe(sql.to_owned()))
            .fetch_one(pool)
            .await
            .unwrap()
    }

    /// A Telegram playlist request with two songs that have not started.
    async fn playlist(state: &AppState, requested_by: &str) -> i64 {
        let pool = &state.pool;
        let url = format!("https://x/playlist/{requested_by}");
        let request = store::create_request(
            pool,
            &url,
            links::Scope::Playlist,
            "telegram",
            Some(requested_by),
        )
        .await
        .unwrap();
        let collection = links::Collection {
            title: Some("Mix".into()),
            owner: None,
            songs: ["A", "B"]
                .iter()
                .map(|title| links::Wanted {
                    title: (*title).into(),
                    ..links::Wanted::default()
                })
                .collect(),
        };
        store::save_collection(pool, request, &url, links::Scope::Playlist, &collection)
            .await
            .unwrap();
        request
    }

    #[tokio::test]
    async fn forwarded_files_join_one_upload_request() {
        let (directory, fake, state) = start().await;
        wait_for(&fake, "setMyCommands", "queue").await;

        // Two songs forwarded together, and one that is too big for bots.
        fake.push(audio(1, OWNER, "One More Time", 5_000_000));
        fake.push(audio(2, OWNER, "Aerodynamic", 6_000_000));
        fake.push(audio(3, OWNER, "Huge", 30 * 1024 * 1024));
        wait_for(&fake, "sendMessage", "up to 20 MB").await;

        let pool = &state.pool;
        for _ in 0..100 {
            if count(
                pool,
                "SELECT count(*) FROM wanted WHERE status='identifying'",
            )
            .await
                == 2
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(
            count(pool, "SELECT count(*) FROM requests WHERE kind='upload'").await,
            1
        );
        assert_eq!(
            count(
                pool,
                "SELECT count(*) FROM wanted WHERE status='identifying'"
            )
            .await,
            2
        );
        let status_messages = fake
            .calls("sendMessage")
            .iter()
            .filter(|params| params.to_string().contains(texts::READING_LINK))
            .count();
        assert_eq!(status_messages, 1);
        // Saved into staging under a readable name, without the partial copy.
        let staged: Vec<String> = std::fs::read_dir(directory.path().join("music"))
            .unwrap()
            .flatten()
            .flat_map(|entry| walkdir::WalkDir::new(entry.path()))
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            staged
                .iter()
                .any(|name| name.starts_with("Daft Punk - One More Time (u")
                    && name.ends_with(".mp3")),
            "{staged:?}"
        );
        assert!(!staged.iter().any(|name| name.starts_with("u1.")));
    }

    #[tokio::test]
    async fn buttons_check_who_presses_them() {
        let (_directory, fake, state) = start().await;
        wait_for(&fake, "setMyCommands", "queue").await;
        sqlx::query(
            "INSERT INTO telegram_guests(user_id,name,invited_by,created_at) VALUES(?,?,?,?)",
        )
        .bind(FRIEND as i64)
        .bind("Friend")
        .bind(OWNER as i64)
        .bind("now")
        .execute(&state.pool)
        .await
        .unwrap();
        let owners = playlist(&state, "100").await;
        let friends = playlist(&state, "200").await;

        // A guest can't touch the owner's request, but can their own.
        fake.push(button(1, FRIEND, &format!("stop:{owners}")));
        wait_for(&fake, "answerCallbackQuery", "Not allowed").await;
        fake.push(button(2, FRIEND, &format!("stop:{friends}")));
        wait_for(&fake, "answerCallbackQuery", "Stopped 2 songs").await;

        // Details lists the stopped songs as a reply to the status message.
        fake.push(button(3, OWNER, &format!("details:{friends}")));
        let details = wait_for(&fake, "sendMessage", "failed: Stopped").await;
        assert_eq!(details["reply_parameters"]["message_id"], 900);

        // Songs that are gone get a clear answer instead of an error.
        fake.push(button(4, OWNER, "pick:999:1"));
        wait_for(&fake, "editMessageText", "no longer available").await;
        fake.push(button(5, OWNER, "drop:999"));
        wait_for(&fake, "editMessageText", "already gone").await;

        // Unknown buttons are quietly answered, not refused.
        fake.push(button(6, OWNER, "launch:1"));
        wait_for(&fake, "answerCallbackQuery", "q6").await;
        let refused = fake
            .calls("answerCallbackQuery")
            .iter()
            .filter(|params| params["text"] == texts::NOT_ALLOWED)
            .count();
        assert_eq!(refused, 1);
    }

    #[tokio::test]
    async fn review_questions_offer_candidates_in_chat() {
        let (directory, fake, state) = start().await;
        let pool = &state.pool;
        let request = store::create_upload_request(pool, "100").await.unwrap();
        store::attach_telegram(pool, request, OWNER as i64, 5, 900)
            .await
            .unwrap();
        let wanted = store::add_upload(pool, request, "Song", Some("Artist"), None)
            .await
            .unwrap();
        let staging = directory.path().join("Artist - Song (u1).mp3");
        store::mark_identifying(pool, wanted, &staging, None)
            .await
            .unwrap();
        let track = sqlx::query(
            "INSERT INTO tracks(path,filename,status,is_missing,first_seen_at,last_seen_at,
               last_scanned_at,stage,stage_message)
             VALUES(?,'x.mp3','needs_review',0,'now','now','now','review','Two songs fit')",
        )
        .bind(staging.to_str().unwrap())
        .execute(pool)
        .await
        .unwrap()
        .last_insert_rowid();
        let candidate = sqlx::query(
            "INSERT INTO candidates(track_id,provider,title,artist,album,score,score_breakdown)
             VALUES(?,'deezer','Song','Artist','Album',80.0,'{}')",
        )
        .bind(track)
        .execute(pool)
        .await
        .unwrap()
        .last_insert_rowid();

        let question = wait_for(&fake, "sendMessage", "Not sure which song").await;
        assert!(
            question["text"]
                .as_str()
                .unwrap()
                .contains("<i>Two songs fit</i>")
        );
        let rows = question["reply_markup"]["inline_keyboard"].to_string();
        assert!(
            rows.contains(&format!("pick:{track}:{candidate}")),
            "{rows}"
        );
        assert!(rows.contains(&format!("drop:{track}")));
        assert!(rows.contains("Artist – Song · Album"));
        // Asked once only.
        tokio::time::sleep(Duration::from_secs(4)).await;
        let asked = fake
            .calls("sendMessage")
            .iter()
            .filter(|params| params.to_string().contains("Not sure which song"))
            .count();
        assert_eq!(asked, 1);
    }
}
