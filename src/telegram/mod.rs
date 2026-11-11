//! The Telegram bot, inside magpie-fi. It reads messages with long polling,
//! turns links and audio files into requests (the same queue the web page
//! uses), and keeps one status message per request up to date.
//!
//! - `api.rs`: the Bot API client; `types.rs`: the Bot API types we read.
//! - `intake.rs`: links and audio files; `commands.rs`: `/…` commands and guests.
//! - `actions.rs`: every button and card tap, its permission check and what it does.
//! - `cards.rs`: requests as live cards (Bot API 10.4 widgets) and their state.
//! - `library.rs`: `/library`, one shared card with the library at a glance.
//! - `files.rs`: "📲 Send me the file", a filed song sent into the chat.
//! - `status.rs`: the loop that updates status messages and asks about Review.
//! - `texts.rs`: everything the bot says.

mod actions;
pub mod api;
mod cards;
mod commands;
mod files;
mod intake;
mod library;
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
    /// Request → its card's cover colours (`None`: its cover had none), so a
    /// song's cover is read once, not every few seconds.
    colours: Mutex<HashMap<i64, Option<cards::Colours>>>,
    /// What the shared library card shows now, so it is only edited on a change.
    library_shown: Mutex<Option<String>>,
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
        colours: Mutex::new(HashMap::new()),
        library_shown: Mutex::new(None),
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
                &json!({"offset": offset, "timeout": 50,
                        "allowed_updates": ["message", "callback_query", "widget_query"]}),
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
            } else if let Some(query) = &update.widget_query {
                actions::on_card_tap(&context, query).await
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
        // An ephemeral command (Bot API 10.2) gets an ephemeral answer.
        let reply = match message.ephemeral_message_id {
            Some(message) => To::Sender {
                chat,
                user: user.id,
                message,
            },
            None => To::chat(chat),
        };
        return commands::on_command(context, &reply, user, name, argument).await;
    }

    if role(context, user.id).await == Role::Stranger {
        send(context, chat, &texts::stranger(user.id), None).await;
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

/// How sending a card (a widget) went.
pub enum Card {
    Sent(Box<Message>),
    /// Telegram said no: use text in this chat for a while.
    Refused,
    /// Unknown: it may have arrived, so nothing else is sent.
    Lost,
}

/// Sends a widget (`method` is `sendWidget` or `sendSharedWidget`). If
/// Telegram refuses it with a Mini App button below, it is tried once without,
/// since widgets are new and the rules may differ.
async fn send_widget(context: &Context, method: &str, mut params: Value) -> Card {
    loop {
        match context.bot.call::<_, Message>(method, &params).await {
            Ok(sent) => return Card::Sent(Box::new(sent)),
            Err(error) if api::is_refusal(&error) => {
                let dropped = params
                    .as_object_mut()
                    .and_then(|params| params.remove("reply_markup"))
                    .is_some();
                if dropped {
                    tracing::info!(%error, "Telegram refused a card with an app button; trying without");
                    continue;
                }
                tracing::info!(%error, "Telegram refused a card; using text in this chat for a day");
                return Card::Refused;
            }
            Err(error) => {
                tracing::warn!(%error, "sending a card failed");
                return Card::Lost;
            }
        }
    }
}

/// A card state's revision: the card ignores states older than the one it shows.
fn revision() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Who sees a message.
#[derive(Clone, Debug)]
pub enum To {
    /// Everyone in the chat, optionally as a reply.
    Chat { chat: i64, reply_to: Option<i32> },
    /// Only the person who pressed a button (an ephemeral reply). If Telegram
    /// refuses, it becomes a reply to the button's message.
    Presser {
        chat: i64,
        query: String,
        message: i32,
    },
    /// Only the person who sent an ephemeral command (`is_ephemeral`).
    Sender { chat: i64, user: i64, message: i32 },
}

impl To {
    pub fn chat(chat: i64) -> Self {
        Self::Chat {
            chat,
            reply_to: None,
        }
    }

    /// The same message for the whole chat.
    fn visible(&self) -> Self {
        match self {
            Self::Chat { .. } => self.clone(),
            Self::Presser { chat, message, .. } => Self::Chat {
                chat: *chat,
                reply_to: Some(*message),
            },
            Self::Sender { chat, .. } => Self::chat(*chat),
        }
    }

    fn params(&self) -> Value {
        match self {
            Self::Chat { chat, reply_to } => {
                let mut params = json!({ "chat_id": chat });
                if let Some(reply_to) = reply_to {
                    params["reply_parameters"] =
                        json!({"message_id": reply_to, "allow_sending_without_reply": true});
                }
                params
            }
            Self::Presser { chat, query, .. } => json!({
                "chat_id": chat,
                "ephemeral_message_parameters": {"callback_query_id": query},
            }),
            Self::Sender {
                chat,
                user,
                message,
            } => json!({
                "chat_id": chat,
                "ephemeral_message_parameters": {"receiver_user_id": user},
                "reply_parameters": {"ephemeral_message_id": message},
            }),
        }
    }
}

/// Sends a message. Nothing is lost when Telegram refuses a newer kind of
/// message: an ephemeral one goes to the whole chat instead, and a rich one is
/// sent as plain text. Only a clear refusal falls back; a network error does
/// not, because the first message may have arrived after all.
pub async fn say(
    context: &Context,
    to: &To,
    text: &texts::Rich,
    keyboard: Option<Value>,
) -> Option<Message> {
    if !matches!(to, To::Chat { .. }) {
        match deliver(context, to, text, keyboard.clone()).await {
            Delivery::Sent(message) => return Some(*message),
            Delivery::Lost => return None,
            Delivery::Refused => {}
        }
    }
    match deliver(context, &to.visible(), text, keyboard).await {
        Delivery::Sent(message) => Some(*message),
        Delivery::Lost | Delivery::Refused => None,
    }
}

enum Delivery {
    Sent(Box<Message>),
    /// Telegram said no.
    Refused,
    /// Unknown: it may have arrived.
    Lost,
}

async fn deliver(
    context: &Context,
    to: &To,
    text: &texts::Rich,
    keyboard: Option<Value>,
) -> Delivery {
    let mut params = to.params();
    if let Some(keyboard) = keyboard {
        params["reply_markup"] = keyboard;
    }
    if let Some(html) = &text.html {
        let mut rich = params.clone();
        rich["rich_message"] = json!({ "html": html });
        match context.bot.call("sendRichMessage", &rich).await {
            Ok(message) => return Delivery::Sent(Box::new(message)),
            Err(error) if api::is_refusal(&error) => {
                tracing::info!(%error, "Telegram refused a rich message; sending plain text");
            }
            Err(error) => {
                tracing::warn!(%error, "sending a Telegram message failed");
                return Delivery::Lost;
            }
        }
    }
    params["text"] = json!(text.plain);
    params["parse_mode"] = json!("HTML");
    params["link_preview_options"] = json!({"is_disabled": true});
    match context.bot.call("sendMessage", &params).await {
        Ok(message) => Delivery::Sent(Box::new(message)),
        Err(error) => {
            tracing::warn!(%error, "sending a Telegram message failed");
            if api::is_refusal(&error) {
                Delivery::Refused
            } else {
                Delivery::Lost
            }
        }
    }
}

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
    let to = To::Chat { chat, reply_to };
    say(context, &to, &texts::Rich::plain(text), keyboard).await
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

/// Edits a message into a rich message, or into plain text if Telegram refuses.
pub async fn edit_rich(
    context: &Context,
    chat: i64,
    message: i32,
    text: &texts::Rich,
    keyboard: Option<Value>,
) {
    if let Some(html) = &text.html {
        let mut params =
            json!({"chat_id": chat, "message_id": message, "rich_message": {"html": html}});
        if let Some(keyboard) = &keyboard {
            params["reply_markup"] = keyboard.clone();
        }
        match context
            .bot
            .call::<_, Value>("editMessageText", &params)
            .await
        {
            Ok(_) => return,
            Err(error) if api::is_not_modified(&error) => return,
            Err(error) if api::is_refusal(&error) => {
                tracing::info!(%error, "Telegram refused a rich edit; using plain text");
            }
            Err(error) => {
                tracing::warn!(%error, "editing a Telegram message failed");
                return;
            }
        }
    }
    edit(context, chat, message, &text.plain, keyboard).await;
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
        /// Refuse rich messages, as a Telegram that doesn't know them would.
        refuse_rich: std::sync::atomic::AtomicBool,
        /// Refuse ephemeral messages (they can be refused or lost).
        refuse_ephemeral: std::sync::atomic::AtomicBool,
        /// Refuse widgets, as a Telegram without them would.
        refuse_cards: std::sync::atomic::AtomicBool,
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
        body: axum::body::Bytes,
    ) -> Json<Value> {
        // JSON calls are kept as they are; uploads (multipart forms) as their
        // text, which is enough to see the fields and file names.
        let params: Value = serde_json::from_slice(&body)
            .unwrap_or_else(|_| json!({ "form": String::from_utf8_lossy(&body) }));
        fake.calls
            .lock()
            .unwrap()
            .push((method.clone(), params.clone()));
        let refused = (method == "sendRichMessage"
            && fake.refuse_rich.load(std::sync::atomic::Ordering::SeqCst))
            || (params.get("ephemeral_message_parameters").is_some()
                && fake
                    .refuse_ephemeral
                    .load(std::sync::atomic::Ordering::SeqCst))
            || (method == "sendWidget"
                && fake.refuse_cards.load(std::sync::atomic::Ordering::SeqCst));
        if refused {
            return Json(
                json!({"ok": false, "error_code": 400, "description": "Bad Request: not here"}),
            );
        }
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
            "sendMessage" | "sendRichMessage" | "sendWidget" | "sendSharedWidget" | "sendAudio"
            | "editMessageText" => {
                let mut next = fake.next_message.lock().unwrap();
                *next += 1;
                let mut sent = json!({"message_id": 1000 + *next, "date": 0, "chat": {"id": params["chat_id"], "type": "private"}, "text": params["text"]});
                // A new shared widget comes back with its ID.
                if params["is_shared"] == true {
                    sent["widget"] =
                        json!({"id": format!("shared{}", *next), "html": params["html"]});
                }
                sent
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
        start_full(public_url, false).await
    }

    /// A bot that shows requests as cards (widgets).
    async fn start_cards() -> (tempfile::TempDir, Arc<FakeTelegram>, Arc<AppState>) {
        start_full(Some("https://music.example"), true).await
    }

    async fn start_full(
        public_url: Option<&str>,
        cards: bool,
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
            cards,
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
        let welcome = wait_for(&fake, "sendRichMessage", "Open magpie-fi").await;
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
        let welcome = wait_for(&fake, "sendRichMessage", "/help shows").await;
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
        wait_for(&fake, "sendRichMessage", "/guests").await;
        fake.push(message(3, OWNER, "/guests"));
        let guests = wait_for(&fake, "sendRichMessage", "Create an invite link").await;
        assert_eq!(
            guests["reply_markup"]["inline_keyboard"][0][0]["style"],
            "success"
        );
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
        wait_for(&fake, "sendRichMessage", "Welcome, User200").await;
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

        // Details lists the stopped songs, seen only by the person who pressed.
        fake.push(button(3, OWNER, &format!("details:{friends}")));
        let details = wait_for(&fake, "sendRichMessage", "Failed: Stopped").await;
        assert_eq!(
            details["ephemeral_message_parameters"]["callback_query_id"],
            "q3"
        );
        // When Telegram refuses that, it becomes a plain reply to the status message.
        fake.refuse_rich
            .store(true, std::sync::atomic::Ordering::SeqCst);
        fake.refuse_ephemeral
            .store(true, std::sync::atomic::Ordering::SeqCst);
        fake.push(button(7, OWNER, &format!("details:{friends}")));
        // (The refused tries are recorded too; the reply is the one that counts.)
        let details = wait_for(&fake, "sendMessage", "allow_sending_without_reply").await;
        assert!(
            details["text"]
                .as_str()
                .unwrap()
                .contains("failed: Stopped")
        );
        assert_eq!(details["reply_parameters"]["message_id"], 900);
        assert!(details.get("ephemeral_message_parameters").is_none());

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

    /// A song sent by `chat` that waits in Review with one candidate.
    async fn song_in_review(
        directory: &tempfile::TempDir,
        state: &AppState,
        chat: u64,
    ) -> (i64, i64) {
        let pool = &state.pool;
        let request = store::create_upload_request(pool, &chat.to_string())
            .await
            .unwrap();
        store::attach_telegram(pool, request, chat as i64, 5, 900)
            .await
            .unwrap();
        let wanted = store::add_upload(pool, request, "Song", Some("Artist"), None)
            .await
            .unwrap();
        let staging = directory
            .path()
            .join(format!("Artist - Song (u{wanted}).mp3"));
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
        (track, candidate)
    }

    #[tokio::test]
    async fn review_questions_offer_candidates_in_chat() {
        let (directory, fake, state) = start().await;
        let (track, candidate) = song_in_review(&directory, &state, OWNER).await;

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
        assert!(rows.contains(r#""style":"success""#) && rows.contains(r#""style":"danger""#));
        // Asked once only.
        tokio::time::sleep(Duration::from_secs(4)).await;
        let asked = fake
            .calls("sendMessage")
            .iter()
            .filter(|params| params.to_string().contains("Not sure which song"))
            .count();
        assert_eq!(asked, 1);
    }

    #[tokio::test]
    async fn only_owners_are_sent_to_the_app() {
        let (directory, fake, state) = start_with(Some("https://music.example")).await;
        let (owner_track, _) = song_in_review(&directory, &state, OWNER).await;
        let (guest_track, _) = song_in_review(&directory, &state, FRIEND).await;

        let owner = wait_for(&fake, "sendMessage", &format!("track={owner_track}")).await;
        assert_eq!(owner["chat_id"], json!(OWNER));
        // Guests can't open the web page, so they choose in the chat.
        let guest = wait_for(&fake, "sendMessage", &format!("drop:{guest_track}")).await;
        assert_eq!(guest["chat_id"], json!(FRIEND));
        assert!(!guest.to_string().contains("web_app"));
    }

    #[tokio::test]
    async fn rich_messages_fall_back_to_plain_text() {
        let (_directory, fake, _state) = start().await;
        fake.refuse_rich
            .store(true, std::sync::atomic::Ordering::SeqCst);
        fake.push(message(1, OWNER, "/help"));
        let help = wait_for(&fake, "sendMessage", "/guests · invite people").await;
        assert_eq!(help["parse_mode"], "HTML");
        assert_eq!(fake.calls("sendRichMessage").len(), 1);
    }

    #[tokio::test]
    async fn ephemeral_commands_get_ephemeral_answers() {
        let (_directory, fake, _state) = start().await;
        let menu = wait_for(&fake, "setMyCommands", "queue").await;
        let commands = menu["commands"].as_array().unwrap();
        let ephemeral = |name: &str| {
            commands
                .iter()
                .find(|command| command["command"] == name)
                .unwrap()
                .get("is_ephemeral")
                .is_some()
        };
        assert!(ephemeral("id") && ephemeral("queue") && !ephemeral("guests"));

        // An ephemeral /id: message_id is 0 and the answer replies to the ephemeral ID.
        let mut command = message(1, OWNER, "/id");
        command["message"]["message_id"] = json!(0);
        command["message"]["ephemeral_message_id"] = json!(55);
        fake.push(command.clone());
        let answer = wait_for(&fake, "sendMessage", "Your Telegram ID").await;
        assert_eq!(
            answer["ephemeral_message_parameters"]["receiver_user_id"],
            json!(OWNER)
        );
        assert_eq!(answer["reply_parameters"]["ephemeral_message_id"], 55);

        // If Telegram refuses, the answer goes to the chat as usual.
        fake.refuse_ephemeral
            .store(true, std::sync::atomic::Ordering::SeqCst);
        command["update_id"] = json!(2);
        fake.push(command);
        for _ in 0..100 {
            if fake.calls("sendMessage").len() == 3 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let calls = fake.calls("sendMessage");
        assert_eq!(calls.len(), 3);
        assert!(calls[2].get("ephemeral_message_parameters").is_none());
        assert_eq!(calls[2]["chat_id"], json!(OWNER));
    }

    fn card_tap(update: u32, user: u64, data: &str) -> Value {
        json!({"update_id": update, "widget_query": {
            "id": format!("w{update}"), "chat_instance": "c", "type": {"type": "data"},
            "from": {"id": user, "is_bot": false, "first_name": format!("User{user}")},
            "data": data
        }})
    }

    /// The card state in an `editMessageWidgetData` or `sendWidget` call.
    fn card_state(params: &Value) -> Value {
        serde_json::from_str(params["chat_data"].as_str().unwrap()).unwrap()
    }

    #[tokio::test]
    async fn owners_see_requests_as_live_cards() {
        let (_directory, fake, _state) = start_cards().await;
        wait_for(&fake, "setMyCommands", "queue").await;
        fake.push(audio(1, OWNER, "One More Time", 5_000_000));

        let card = wait_for(&fake, "sendWidget", "html").await;
        assert!(
            card["html"]
                .as_str()
                .unwrap()
                .contains("magpie-fi request card")
        );
        assert_eq!(card["allow_queries"], true);
        assert_eq!(card["caption"], texts::READING_LINK);
        assert_eq!(card["reply_parameters"]["message_id"], 1);
        assert_eq!(
            card["reply_markup"]["inline_keyboard"][0][0]["web_app"]["url"],
            "https://music.example/"
        );
        let first = card_state(&card);
        assert_eq!(
            (first["s"].as_str(), first["k"].as_str()),
            (Some("reading"), Some("upload"))
        );

        // The status loop redraws the card quietly and keeps the caption current.
        let update = wait_for(&fake, "editMessageWidgetData", "One More Time").await;
        let state = card_state(&update);
        assert_eq!(state["s"], "busy");
        assert!(state["v"].as_i64().unwrap() > first["v"].as_i64().unwrap());
        let caption = wait_for(&fake, "editMessageCaption", "On the way").await;
        assert!(
            caption["caption"]
                .as_str()
                .unwrap()
                .contains("Daft Punk – One More Time")
        );
        assert!(caption["reply_markup"].to_string().contains("web_app"));
        // No text status message was sent.
        assert!(
            !fake
                .calls("sendMessage")
                .iter()
                .any(|params| params.to_string().contains(texts::READING_LINK))
        );
    }

    #[tokio::test]
    async fn refused_cards_fall_back_to_text() {
        let (_directory, fake, state) = start_cards().await;
        wait_for(&fake, "setMyCommands", "queue").await;
        fake.refuse_cards
            .store(true, std::sync::atomic::Ordering::SeqCst);
        fake.push(audio(1, OWNER, "One More Time", 5_000_000));

        wait_for(&fake, "sendMessage", texts::READING_LINK).await;
        // Tried with the app button, then without, then text.
        let tries = fake.calls("sendWidget");
        assert_eq!(tries.len(), 2);
        assert!(tries[0].get("reply_markup").is_some() && tries[1].get("reply_markup").is_none());
        // Text for this chat for a while.
        assert!(
            !store::cards_allowed(&state.pool, OWNER as i64)
                .await
                .unwrap()
        );
        assert!(
            store::cards_allowed(&state.pool, FRIEND as i64)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn card_taps_follow_the_same_rules_as_buttons() {
        let (_directory, fake, state) = start_cards().await;
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

        fake.push(card_tap(1, FRIEND, &format!("stop:{owners}")));
        let refused = wait_for(&fake, "answerWidgetQuery", "w1").await;
        assert!(
            refused["data"]
                .as_str()
                .unwrap()
                .contains(texts::NOT_ALLOWED)
        );

        fake.push(card_tap(2, OWNER, &format!("stop:{owners}")));
        let stopped = wait_for(&fake, "answerWidgetQuery", "w2").await;
        assert!(
            stopped["data"]
                .as_str()
                .unwrap()
                .contains("Stopped 2 songs")
        );

        fake.push(card_tap(3, OWNER, &format!("details:{owners}")));
        let details = wait_for(&fake, "answerWidgetQuery", "w3").await;
        let answer: Value = serde_json::from_str(details["data"].as_str().unwrap()).unwrap();
        assert_eq!(answer["d"][0][1], "Failed: Stopped");

        // Junk is answered too, so the card doesn't wait.
        fake.push(card_tap(4, OWNER, "launch:everything"));
        wait_for(&fake, "answerWidgetQuery", "w4").await;
    }

    #[tokio::test]
    async fn a_finished_playlist_card_celebrates_once() {
        let (directory, fake, state) = start_cards().await;
        wait_for(&fake, "setMyCommands", "queue").await;
        let pool = &state.pool;
        let url = "https://x/playlist/party";
        let request =
            store::create_request(pool, url, links::Scope::Playlist, "telegram", Some("100"))
                .await
                .unwrap();
        let collection = links::Collection {
            title: Some("Party".into()),
            owner: None,
            songs: (0..5)
                .map(|index| links::Wanted {
                    title: format!("Song {index}"),
                    ..links::Wanted::default()
                })
                .collect(),
        };
        store::save_collection(pool, request, url, links::Scope::Playlist, &collection)
            .await
            .unwrap();
        store::attach_telegram(pool, request, OWNER as i64, 5, 900)
            .await
            .unwrap();
        store::set_card(pool, request, texts::READING_LINK)
            .await
            .unwrap();
        let file = directory.path().join("a.m4a");
        tokio::fs::write(&file, b"a").await.unwrap();
        while let Some(song) = store::claim_next(pool).await.unwrap() {
            store::mark_in_library(pool, song.id, file.to_str().unwrap())
                .await
                .unwrap();
        }

        let done = wait_for(&fake, "editMessageWidgetData", r#"\"s\":\"done\""#).await;
        assert_eq!(card_state(&done)["n"][0], 5);
        let party = wait_for(&fake, "sendMessage", "🎉").await;
        assert!(
            party["text"]
                .as_str()
                .unwrap()
                .contains("<b>Party</b> is in your library: 5 of 5")
        );
        assert!(party["message_effect_id"].is_string());
        assert_eq!(party["reply_parameters"]["message_id"], 900);
        tokio::time::sleep(Duration::from_secs(4)).await;
        let parties = fake
            .calls("sendMessage")
            .iter()
            .filter(|params| params.to_string().contains("🎉"))
            .count();
        assert_eq!(parties, 1);
    }

    #[tokio::test]
    async fn review_questions_become_cards_to_tap() {
        let (directory, fake, state) = start_cards().await;
        let (track, candidate) = song_in_review(&directory, &state, OWNER).await;

        let card = wait_for(&fake, "sendWidget", "review card").await;
        assert!(
            card["caption"]
                .as_str()
                .unwrap()
                .contains("Not sure which song")
        );
        assert_eq!(card["allow_queries"], true);
        let choices = card_state(&card);
        assert_eq!(choices["s"], "ask");
        assert_eq!(choices["r"], "Two songs fit");
        assert_eq!(choices["o"][0][0], candidate);
        assert_eq!(choices["o"][0][1], "Song");
        assert!(
            card["reply_markup"]["inline_keyboard"][0][0]["web_app"]["url"]
                .as_str()
                .unwrap()
                .ends_with(&format!("track={track}"))
        );
        // No text question as well.
        assert!(
            !fake
                .calls("sendMessage")
                .iter()
                .any(|params| params.to_string().contains("Not sure which song"))
        );
    }

    #[tokio::test]
    async fn review_card_taps_choose_or_remove() {
        let (directory, fake, state) = start_cards().await;
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
        let (track, candidate) = song_in_review(&directory, &state, FRIEND).await;
        let card = wait_for(&fake, "sendWidget", "review card").await;
        // Guests can't open the web page, so their card has no app button.
        assert!(card.get("reply_markup").is_none());

        // Someone else can't choose for the guest.
        fake.push(card_tap(1, 300, &format!("pick:{track}:{candidate}")));
        let refused = wait_for(&fake, "answerWidgetQuery", "w1").await;
        assert!(
            refused["data"]
                .as_str()
                .unwrap()
                .contains(texts::NOT_ALLOWED)
        );

        // The guest picks: the card answers at once, then shows the outcome.
        // (A choice that no longer exists, so the test stays off the network:
        // a real choice looks up missing details online.) The card says the
        // song is gone instead of hanging.
        fake.push(card_tap(2, FRIEND, &format!("pick:{track}:999999")));
        let answer = wait_for(&fake, "answerWidgetQuery", "w2").await;
        assert!(
            answer["data"]
                .as_str()
                .unwrap()
                .contains(texts::SAVING_CHOICE)
        );
        let ended = wait_for(&fake, "editMessageWidgetData", r#"\"s\":\"gone\""#).await;
        assert_eq!(card_state(&ended)["id"], track);
        let caption = wait_for(&fake, "editMessageCaption", "no longer available").await;
        assert!(caption.get("reply_markup").is_none());
        // The card is marked as ended right after its caption edit.
        let mut closed = 0;
        for _ in 0..100 {
            closed = sqlx::query_scalar("SELECT review_card FROM wanted LIMIT 1")
                .fetch_one(&state.pool)
                .await
                .unwrap();
            if closed == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(closed, 2);
    }

    #[tokio::test]
    async fn review_cards_follow_choices_made_in_the_app() {
        let (directory, fake, state) = start_cards().await;
        song_in_review(&directory, &state, OWNER).await;
        wait_for(&fake, "sendWidget", "review card").await;

        // The owner removes the song on the web page.
        sqlx::query("UPDATE wanted SET status='removed'")
            .execute(&state.pool)
            .await
            .unwrap();
        let ended = wait_for(&fake, "editMessageWidgetData", r#"\"s\":\"removed\""#).await;
        assert!(
            card_state(&ended)["m"]
                .as_str()
                .unwrap()
                .contains("Removed")
        );
        wait_for(&fake, "editMessageCaption", "Removed").await;
        // Only once.
        tokio::time::sleep(Duration::from_secs(4)).await;
        let edits = fake
            .calls("editMessageWidgetData")
            .iter()
            .filter(|params| params.to_string().contains("removed"))
            .count();
        assert_eq!(edits, 1);
    }

    async fn add_to_library(state: &AppState, title: &str) {
        sqlx::query(
            "INSERT INTO library_items(path,file_size,file_mtime_ns,duration,title,artist,source,
               added_at,checked_at)
             VALUES(?,1,1,3600,?,'Daft Punk','inbox',?,?)",
        )
        .bind(format!("/music/{title}.m4a"))
        .bind(title)
        .bind(chrono::Utc::now().to_rfc3339())
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(&state.pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn the_library_card_is_shared_pinned_and_live() {
        let (_directory, fake, state) = start_cards().await;
        wait_for(&fake, "setMyCommands", "library").await;
        add_to_library(&state, "One More Time").await;

        // The first /library makes the shared card and pins it quietly.
        fake.push(message(1, OWNER, "/library"));
        let card = wait_for(&fake, "sendWidget", "is_shared").await;
        assert!(
            card["html"]
                .as_str()
                .unwrap()
                .contains("magpie-fi library card")
        );
        let shown: Value = serde_json::from_str(card["public_data"].as_str().unwrap()).unwrap();
        assert_eq!(shown["n"], json!([1, 1, 1]));
        assert_eq!(shown["a"][0], json!(["One More Time", "Daft Punk"]));
        assert_eq!(card["caption"], texts::LIBRARY_CAPTION);
        let pin = wait_for(&fake, "pinChatMessage", "message_id").await;
        assert_eq!(pin["disable_notification"], true);
        let first_card = pin["message_id"].clone();

        // The next /library sends the same shared card and moves the pin.
        fake.push(message(2, OWNER, "/library"));
        let again = wait_for(&fake, "sendSharedWidget", "widget_id").await;
        assert!(again["widget_id"].as_str().unwrap().starts_with("shared"));
        let unpin = wait_for(&fake, "unpinChatMessage", "message_id").await;
        assert_eq!(unpin["message_id"], first_card);
        assert_eq!(fake.calls("sendWidget").len(), 1);

        // A new song: every copy is updated with one edit.
        add_to_library(&state, "Aerodynamic").await;
        let update = wait_for(&fake, "editSharedWidgetData", "Aerodynamic").await;
        let shown: Value = serde_json::from_str(update["public_data"].as_str().unwrap()).unwrap();
        assert_eq!(shown["n"][0], 2);
        assert_eq!(update["widget_id"], again["widget_id"]);
    }

    #[tokio::test]
    async fn a_changed_library_card_is_made_again() {
        let (_directory, fake, state) = start_cards().await;
        wait_for(&fake, "setMyCommands", "library").await;
        // A shared card made by an older magpie-fi with different card code.
        write_state(&state.pool, "library_widget", "0123456789ab:old").await;
        fake.push(message(1, OWNER, "/library"));
        wait_for(&fake, "sendWidget", "is_shared").await;
        assert!(fake.calls("sendSharedWidget").is_empty());
        let saved = read_state(&state.pool, "library_widget").await.unwrap();
        assert!(saved.starts_with(&cards::library_version()) && saved.contains("shared"));
    }

    #[tokio::test]
    async fn the_library_without_cards_and_for_guests() {
        let (_directory, fake, state) = start().await;
        wait_for(&fake, "setMyCommands", "library").await;
        add_to_library(&state, "One More Time").await;
        fake.push(message(1, OWNER, "/library"));
        let text = wait_for(&fake, "sendRichMessage", "Your library").await;
        assert!(text.to_string().contains("One More Time · Daft Punk"));
        assert!(fake.calls("sendWidget").is_empty());

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
        fake.push(message(2, FRIEND, "/library"));
        wait_for(&fake, "sendMessage", "don't know that command").await;
    }

    /// A finished one-song request whose song was filed at `file`.
    async fn filed_song(state: &AppState, file: &std::path::Path, requested_by: &str) -> i64 {
        let pool = &state.pool;
        let url = format!("https://x/track/{requested_by}");
        let request = store::create_request(
            pool,
            &url,
            links::Scope::Track,
            "telegram",
            Some(requested_by),
        )
        .await
        .unwrap();
        let collection = links::Collection {
            title: None,
            owner: None,
            songs: vec![links::Wanted {
                title: "One More Time".into(),
                artist: Some("Daft Punk".into()),
                ..links::Wanted::default()
            }],
        };
        store::save_collection(pool, request, &url, links::Scope::Track, &collection)
            .await
            .unwrap();
        let chat: i64 = requested_by.parse().unwrap();
        store::attach_telegram(pool, request, chat, 5, 900)
            .await
            .unwrap();
        let song = store::claim_next(pool).await.unwrap().unwrap();
        store::mark_in_library(pool, song.id, file.to_str().unwrap())
            .await
            .unwrap();
        request
    }

    #[tokio::test]
    async fn a_filed_song_can_be_sent_into_the_chat() {
        let (_directory, fake, state) = start().await;
        wait_for(&fake, "setMyCommands", "queue").await;
        let library = std::path::Path::new(&state.config.output_dir).join("Daft Punk");
        tokio::fs::create_dir_all(&library).await.unwrap();
        let file = library.join("One More Time.m4a");
        tokio::fs::write(&file, b"not really audio").await.unwrap();
        let file = tokio::fs::canonicalize(&file).await.unwrap();
        sqlx::query(
            "INSERT INTO library_items(path,file_size,file_mtime_ns,duration,title,artist,source,
               added_at,checked_at) VALUES(?,1,1,320.4,'One More Time','Daft Punk','inbox','now','now')",
        )
        .bind(file.to_str().unwrap())
        .execute(&state.pool)
        .await
        .unwrap();
        let request = filed_song(&state, &file, "100").await;

        // The finished status message offers the file.
        let done = wait_for(&fake, "editMessageText", &format!("file:{request}")).await;
        assert!(done.to_string().contains(texts::SEND_FILE));

        fake.push(button(1, OWNER, &format!("file:{request}")));
        wait_for(&fake, "answerCallbackQuery", texts::SENDING_FILE).await;
        let sent = wait_for(&fake, "sendAudio", "One More Time.m4a").await;
        let form = sent["form"].as_str().unwrap();
        for field in [
            "name=\"title\"\r\n\r\nOne More Time",
            "name=\"performer\"\r\n\r\nDaft Punk",
            "name=\"duration\"\r\n\r\n320",
            "not really audio",
            "\"message_id\":900",
        ] {
            assert!(form.contains(field), "{field:?} missing in {form}");
        }
    }

    #[tokio::test]
    async fn only_library_songs_go_out_and_only_to_their_people() {
        let (directory, fake, state) = start().await;
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
        // A "library path" outside the library folder.
        let outside = directory.path().join("secret.m4a");
        tokio::fs::write(&outside, b"secret").await.unwrap();
        let request = filed_song(&state, &outside, "100").await;

        fake.push(button(1, FRIEND, &format!("file:{request}")));
        wait_for(&fake, "answerCallbackQuery", texts::NOT_ALLOWED).await;
        fake.push(button(2, OWNER, &format!("file:{request}")));
        wait_for(&fake, "sendMessage", texts::FILE_GONE).await;
        assert!(fake.calls("sendAudio").is_empty());

        // Too big for Telegram: a sparse 60 MB file costs no disk space.
        let library = std::path::Path::new(&state.config.output_dir);
        tokio::fs::create_dir_all(library).await.unwrap();
        let big = library.join("Long Mix.flac");
        std::fs::File::create(&big)
            .unwrap()
            .set_len(60 * 1024 * 1024)
            .unwrap();
        sqlx::query("UPDATE wanted SET library_path=? WHERE request_id=?")
            .bind(big.to_str().unwrap())
            .bind(request)
            .execute(&state.pool)
            .await
            .unwrap();
        fake.push(button(3, OWNER, &format!("file:{request}")));
        wait_for(&fake, "sendMessage", "60 MB").await;
        assert!(fake.calls("sendAudio").is_empty());
    }
}
