//! `/start`, `/help`, `/queue`, `/health`, `/web`, `/guests`, `/id`, and guest
//! invites.

use super::{
    Context, Role, actions::Action, mini_app_button, mini_app_url, role, send, send_with,
    set_menu_button, texts, types::User,
};
use crate::acquire::store;
use anyhow::Result;
use serde_json::{Value, json};
use std::sync::Arc;

/// The commands shown in Telegram's command menu (`setMyCommands`).
pub fn menu() -> Value {
    let commands: Vec<Value> = texts::COMMANDS
        .iter()
        .map(|(command, description)| json!({"command": command, "description": description}))
        .collect();
    json!({ "commands": commands })
}

pub async fn on_command(
    context: &Arc<Context>,
    chat: i64,
    user: &User,
    name: &str,
    argument: &str,
) -> Result<()> {
    if name == "id" {
        send(context, chat, &texts::your_id(user.id), None).await;
        return Ok(());
    }
    if name == "start"
        && let Some(token) = argument.strip_prefix("invite_")
    {
        return redeem_invite(context, chat, user, token).await;
    }
    let role = role(context, user.id).await;
    if role == Role::Stranger {
        send(context, chat, &texts::welcome(false, user.id), None).await;
        return Ok(());
    }
    let public_url = &context.state.config.public_url;
    match name {
        "start" => {
            let mut keyboard = None;
            if role == Role::Owner {
                set_menu_button(&context.bot, public_url, user.id).await;
                keyboard = mini_app_url(public_url, "").map(
                    |url| json!({"inline_keyboard": [[mini_app_button(texts::OPEN_APP, &url)]]}),
                );
            }
            send_with(
                context,
                chat,
                &texts::welcome(true, user.id),
                None,
                keyboard,
            )
            .await
        }
        "help" => send(context, chat, &texts::help(role == Role::Owner), None).await,
        "queue" => {
            let requests = store::list_requests(&context.state.pool, 30).await?;
            send(context, chat, &texts::queue(&requests), None).await
        }
        "health" => send(context, chat, &health(context).await, None).await,
        "web" if role == Role::Owner => {
            let base = public_url
                .clone()
                .unwrap_or_else(|| "http://127.0.0.1:7331".into());
            let link =
                crate::http::auth::create_login_link(&context.state.pool, user.id, &base).await?;
            // Inside Telegram the Mini App logs in by itself; the link is for a browser.
            let mut rows = Vec::new();
            if let Some(url) = mini_app_url(public_url, "") {
                rows.push(vec![mini_app_button(texts::OPEN_IN_TELEGRAM, &url)]);
            }
            if public_url.is_some() {
                rows.push(vec![json!({"text": texts::OPEN_IN_BROWSER, "url": link})]);
            }
            let keyboard = (!rows.is_empty()).then(|| json!({ "inline_keyboard": rows }));
            let text = texts::login_link(&link, public_url.is_some());
            send_with(context, chat, &text, None, keyboard).await
        }
        "guests" if role == Role::Owner => {
            let (text, keyboard) = guests_view(context).await?;
            send_with(context, chat, &text, None, Some(keyboard)).await
        }
        _ => send(context, chat, texts::UNKNOWN_COMMAND, None).await,
    };
    Ok(())
}

async fn health(context: &Context) -> String {
    let config = &context.state.config;
    let songs: i64 = sqlx::query_scalar("SELECT count(*) FROM library_items")
        .fetch_one(&context.state.pool)
        .await
        .unwrap_or_default();
    let writable = tokio::fs::metadata(&config.output_dir)
        .await
        .is_ok_and(|metadata| !metadata.permissions().readonly());
    texts::health(&texts::Health {
        library_writable: writable,
        songs,
        ytdlp: crate::acquire::ytdlp::available(config),
        mini_app: mini_app_url(&config.public_url, "").is_some(),
    })
}

// ── Guests ──────────────────────────────────────────────────────────────────

pub async fn guests_view(context: &Context) -> Result<(String, Value)> {
    let guests: Vec<(i64, Option<String>)> =
        sqlx::query_as("SELECT user_id,name FROM telegram_guests ORDER BY created_at")
            .fetch_all(&context.state.pool)
            .await?;
    let guests: Vec<(i64, String)> = guests
        .into_iter()
        .map(|(id, name)| (id, name.unwrap_or_else(|| id.to_string())))
        .collect();
    let mut rows = vec![vec![Action::Invite.button(texts::CREATE_INVITE)]];
    for (id, name) in &guests {
        rows.push(vec![Action::Revoke(*id).button(&texts::remove_guest(name))]);
    }
    Ok((texts::guests(&guests), json!({"inline_keyboard": rows})))
}

async fn redeem_invite(context: &Context, chat: i64, user: &User, token: &str) -> Result<()> {
    let created_by: Option<i64> =
        sqlx::query_scalar("DELETE FROM telegram_invites WHERE token=? RETURNING created_by")
            .bind(token)
            .fetch_optional(&context.state.pool)
            .await?;
    let Some(created_by) = created_by else {
        send(context, chat, texts::INVITE_USED, None).await;
        return Ok(());
    };
    let name = user.display_name();
    sqlx::query(
        "INSERT INTO telegram_guests(user_id,name,invited_by,created_at) VALUES(?,?,?,?)
         ON CONFLICT(user_id) DO NOTHING",
    )
    .bind(user.id)
    .bind(&name)
    .bind(created_by)
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(&context.state.pool)
    .await?;
    send(context, chat, &texts::welcome_guest(&name), None).await;
    Ok(())
}

/// A one-time invite link for a guest.
pub async fn create_invite(context: &Context, owner: i64) -> Result<String> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|error| anyhow::anyhow!("no randomness: {error}"))?;
    let token = hex::encode(bytes);
    sqlx::query("INSERT INTO telegram_invites(token,created_by,created_at) VALUES(?,?,?)")
        .bind(&token)
        .bind(owner)
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(&context.state.pool)
        .await?;
    Ok(format!(
        "https://t.me/{}?start=invite_{token}",
        context.username
    ))
}
