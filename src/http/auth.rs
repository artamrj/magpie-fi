//! Who is using the web page. Without Telegram the page is local-only and
//! needs no login (as before). With Telegram, every API call needs either
//!
//! - Telegram's signed Mini App data (`Authorization: tma <initData>`), or
//! - a session cookie, created by a one-time link the bot sends for `/web`.
//!
//! Only owners may use the web page; guests use the bot.

use crate::{app::AppState, config::TelegramConfig};
use anyhow::Result;
use axum::{
    Json,
    extract::{Query, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Redirect, Response},
};
use chrono::{Duration, Utc};
use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use std::sync::Arc;

pub const COOKIE: &str = "magpie_session";
const SESSION_DAYS: i64 = 30;
const LINK_MINUTES: i64 = 10;
/// Mini App data older than this is not accepted.
const INIT_DATA_MAX_AGE_SECONDS: i64 = 24 * 60 * 60;

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Owner,
    Guest,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct Identity {
    pub user_id: i64,
    pub name: Option<String>,
    pub role: Role,
    /// How this request proved who it is: "telegram", "session" or "local".
    pub via: &'static str,
}

/// Paths that work without a login. Inside the `/api` router the prefix is
/// already stripped, so both forms are accepted.
fn is_public(path: &str) -> bool {
    matches!(
        path.strip_prefix("/api").unwrap_or(path),
        "/health" | "/login" | "/me"
    )
}

/// Guards every `/api` route. Adds the [`Identity`] to the request.
pub async fn require_login(
    State(state): State<Arc<AppState>>,
    mut request: Request,
    next: Next,
) -> Response {
    let telegram = state.config.telegram.clone();
    let identity = match telegram.as_ref() {
        None => Some(Identity {
            user_id: 0,
            name: None,
            role: Role::Owner,
            via: "local",
        }),
        Some(telegram) => identify(&state, telegram, request.headers()).await,
    };
    let path = request.uri().path().to_owned();
    if let Some(identity) = identity.clone() {
        request.extensions_mut().insert(identity);
    }
    if is_public(&path) {
        return next.run(request).await;
    }
    match identity {
        None => error(
            StatusCode::UNAUTHORIZED,
            "Log in: send /web to your magpie bot",
        ),
        Some(identity) if identity.role != Role::Owner => error(
            StatusCode::FORBIDDEN,
            "The web page is for owners. Send links to the bot instead.",
        ),
        Some(_) => next.run(request).await,
    }
}

fn error(status: StatusCode, message: &str) -> Response {
    (status, Json(serde_json::json!({"error": message}))).into_response()
}

async fn identify(
    state: &AppState,
    telegram: &TelegramConfig,
    headers: &HeaderMap,
) -> Option<Identity> {
    if let Some(init_data) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("tma "))
    {
        let user = verify_init_data(init_data, &telegram.token, Utc::now().timestamp())?;
        let role = role(&state.pool, telegram, user.id).await?;
        return Some(Identity {
            user_id: user.id,
            name: user.name,
            role,
            via: "telegram",
        });
    }
    let user_id = session_user(&state.pool, headers).await?;
    let role = role(&state.pool, telegram, user_id).await?;
    Some(Identity {
        user_id,
        name: None,
        role,
        via: "session",
    })
}

/// The user of a valid session cookie, if the request carries one.
async fn session_user(pool: &SqlitePool, headers: &HeaderMap) -> Option<i64> {
    let token = cookie(headers, COOKIE)?;
    sqlx::query_scalar("SELECT user_id FROM sessions WHERE token_hash=? AND expires_at>?")
        .bind(hash(&token))
        .bind(Utc::now().to_rfc3339())
        .fetch_optional(pool)
        .await
        .ok()?
}

async fn role(pool: &SqlitePool, telegram: &TelegramConfig, user_id: i64) -> Option<Role> {
    if telegram.owners.contains(&user_id) {
        return Some(Role::Owner);
    }
    let guest: Option<i64> =
        sqlx::query_scalar("SELECT user_id FROM telegram_guests WHERE user_id=?")
            .bind(user_id)
            .fetch_optional(pool)
            .await
            .ok()?;
    guest.map(|_| Role::Guest)
}

#[derive(Debug, Eq, PartialEq)]
pub struct TelegramUser {
    pub id: i64,
    pub name: Option<String>,
}

/// Checks Telegram's signature on Mini App data
/// (https://core.telegram.org/bots/webapps#validating-data-received-via-the-mini-app):
/// `hash` must equal HMAC-SHA256(key = HMAC-SHA256("WebAppData", bot token),
/// data = the other fields sorted and joined as "key=value" lines).
pub fn verify_init_data(init_data: &str, bot_token: &str, now: i64) -> Option<TelegramUser> {
    let mut fields: Vec<(String, String)> = Vec::new();
    let mut received_hash = None;
    for (key, value) in form_urlencoded_pairs(init_data) {
        if key == "hash" {
            received_hash = Some(value);
        } else {
            fields.push((key, value));
        }
    }
    let received_hash = hex::decode(received_hash?).ok()?;
    fields.sort();
    let check_string = fields
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut secret = Hmac::<Sha256>::new_from_slice(b"WebAppData").ok()?;
    secret.update(bot_token.as_bytes());
    let secret = secret.finalize().into_bytes();
    let mut mac = Hmac::<Sha256>::new_from_slice(&secret).ok()?;
    mac.update(check_string.as_bytes());
    mac.verify_slice(&received_hash).ok()?;

    let field = |name: &str| {
        fields
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value)
    };
    let auth_date: i64 = field("auth_date")?.parse().ok()?;
    if now - auth_date > INIT_DATA_MAX_AGE_SECONDS || auth_date - now > 300 {
        return None;
    }
    let user: serde_json::Value = serde_json::from_str(field("user")?).ok()?;
    let name = [user["first_name"].as_str(), user["last_name"].as_str()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
    Some(TelegramUser {
        id: user["id"].as_i64()?,
        name: (!name.is_empty()).then_some(name),
    })
}

/// `a=1&b=%7B%7D` → [("a","1"), ("b","{}")], with `+` meaning a space.
fn form_urlencoded_pairs(input: &str) -> Vec<(String, String)> {
    input
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            let decode = |text: &str| {
                percent_encoding::percent_decode_str(&text.replace('+', " "))
                    .decode_utf8_lossy()
                    .into_owned()
            };
            (decode(key), decode(value))
        })
        .collect()
}

fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(key, _)| *key == name)
        .map(|(_, value)| value.to_owned())
}

fn hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

fn random_token() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|error| anyhow::anyhow!("no randomness: {error}"))?;
    Ok(hex::encode(bytes))
}

/// A one-time login link for the bot's `/web` command.
pub async fn create_login_link(pool: &SqlitePool, user_id: i64, base_url: &str) -> Result<String> {
    let token = random_token()?;
    sqlx::query("DELETE FROM login_links WHERE expires_at<?")
        .bind(Utc::now().to_rfc3339())
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO login_links(token_hash,user_id,expires_at) VALUES(?,?,?)")
        .bind(hash(&token))
        .bind(user_id)
        .bind((Utc::now() + Duration::minutes(LINK_MINUTES)).to_rfc3339())
        .execute(pool)
        .await?;
    Ok(format!("{base_url}/api/login?token={token}"))
}

#[derive(serde::Deserialize)]
pub struct LoginQuery {
    token: String,
}

/// `GET /api/login?token=…` — uses a one-time link and starts a 30-day session.
pub async fn login(
    State(state): State<Arc<AppState>>,
    Query(query): Query<LoginQuery>,
) -> Response {
    let user_id: Option<i64> = sqlx::query_scalar(
        "DELETE FROM login_links WHERE token_hash=? AND expires_at>? RETURNING user_id",
    )
    .bind(hash(&query.token))
    .bind(Utc::now().to_rfc3339())
    .fetch_optional(&state.pool)
    .await
    .unwrap_or(None);
    let Some(user_id) = user_id else {
        return (
            StatusCode::UNAUTHORIZED,
            axum::response::Html(
                "<p>This login link was already used or is too old. Send /web to your magpie bot for a new one.</p>",
            ),
        )
            .into_response();
    };
    let cookie = match start_session(&state, user_id).await {
        Ok(cookie) => cookie,
        Err(_) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not start a session",
            );
        }
    };
    let mut response = Redirect::to("/").into_response();
    if let Ok(value) = HeaderValue::from_str(&cookie) {
        response.headers_mut().insert(header::SET_COOKIE, value);
    }
    response
}

/// Creates a 30-day session and returns the `Set-Cookie` value for it.
async fn start_session(state: &AppState, user_id: i64) -> Result<String> {
    let token = random_token()?;
    let now = Utc::now();
    sqlx::query("DELETE FROM sessions WHERE expires_at<?")
        .bind(now.to_rfc3339())
        .execute(&state.pool)
        .await?;
    sqlx::query("INSERT INTO sessions(token_hash,user_id,created_at,expires_at) VALUES(?,?,?,?)")
        .bind(hash(&token))
        .bind(user_id)
        .bind(now.to_rfc3339())
        .bind((now + Duration::days(SESSION_DAYS)).to_rfc3339())
        .execute(&state.pool)
        .await?;
    let https = state
        .config
        .public_url
        .as_deref()
        .is_some_and(|url| url.starts_with("https://"));
    // SameSite=None lets Telegram Web (which shows the Mini App in a frame)
    // send it too; browsers only accept that over HTTPS.
    Ok(format!(
        "{COOKIE}={token}; Path=/; HttpOnly; Max-Age={}{}",
        SESSION_DAYS * 24 * 60 * 60,
        if https {
            "; SameSite=None; Secure"
        } else {
            "; SameSite=Lax"
        }
    ))
}

/// `POST /api/logout` — ends this browser's session.
pub async fn logout(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Some(token) = cookie(&headers, COOKIE) {
        let _ = sqlx::query("DELETE FROM sessions WHERE token_hash=?")
            .bind(hash(&token))
            .execute(&state.pool)
            .await;
    }
    let mut response = Json(serde_json::json!({"logged_out": true})).into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_static("magpie_session=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0"),
    );
    response
}

/// `GET /api/me` — who the page is talking to (or that a login is needed).
/// The Mini App also gets a session cookie here (unless it already has a valid
/// one), so cover images and audio previews (which cannot send the Telegram
/// header) work too.
pub async fn me(State(state): State<Arc<AppState>>, request: Request) -> Response {
    let Some(identity) = request.extensions().get::<Identity>().cloned() else {
        return error(
            StatusCode::UNAUTHORIZED,
            "Log in: send /web to your magpie bot",
        );
    };
    let mut response = Json(identity.clone()).into_response();
    if identity.via == "telegram"
        && identity.role == Role::Owner
        && session_user(&state.pool, request.headers()).await != Some(identity.user_id)
        && let Ok(cookie) = start_session(&state, identity.user_id).await
        && let Ok(value) = HeaderValue::from_str(&cookie)
    {
        response.headers_mut().insert(header::SET_COOKIE, value);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "12345:test-token";

    /// Builds Mini App data signed the way Telegram does.
    fn signed(fields: &[(&str, &str)], token: &str) -> String {
        let mut sorted: Vec<_> = fields.to_vec();
        sorted.sort();
        let check = sorted
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut secret = Hmac::<Sha256>::new_from_slice(b"WebAppData").unwrap();
        secret.update(token.as_bytes());
        let secret = secret.finalize().into_bytes();
        let mut mac = Hmac::<Sha256>::new_from_slice(&secret).unwrap();
        mac.update(check.as_bytes());
        let hash = hex::encode(mac.finalize().into_bytes());
        let encode = |text: &str| {
            percent_encoding::utf8_percent_encode(text, percent_encoding::NON_ALPHANUMERIC)
                .to_string()
        };
        fields
            .iter()
            .map(|(key, value)| format!("{key}={}", encode(value)))
            .chain(std::iter::once(format!("hash={hash}")))
            .collect::<Vec<_>>()
            .join("&")
    }

    const USER: &str = r#"{"id":100,"first_name":"Arta","username":"arta"}"#;

    #[test]
    fn genuine_telegram_data_is_accepted() {
        let data = signed(
            &[("auth_date", "1000"), ("query_id", "AAA"), ("user", USER)],
            TOKEN,
        );
        assert_eq!(
            verify_init_data(&data, TOKEN, 1060),
            Some(TelegramUser {
                id: 100,
                name: Some("Arta".into())
            })
        );
    }

    #[test]
    fn forged_old_or_foreign_data_is_refused() {
        let data = signed(&[("auth_date", "1000"), ("user", USER)], TOKEN);
        // Another bot's token.
        assert_eq!(verify_init_data(&data, "999:other", 1060), None);
        // Changed after signing (someone claims to be user 1).
        let forged = data.replace("100", "1");
        assert_eq!(verify_init_data(&forged, TOKEN, 1060), None);
        // Too old.
        assert_eq!(
            verify_init_data(&data, TOKEN, 1000 + 2 * 24 * 60 * 60),
            None
        );
        // No hash at all.
        assert_eq!(verify_init_data("auth_date=1000", TOKEN, 1060), None);
    }

    async fn app(telegram: bool) -> (tempfile::TempDir, Arc<AppState>, axum::Router) {
        let directory = tempfile::tempdir().unwrap();
        let mut config = crate::config::Config::with_folders(
            &directory.path().join("music"),
            &directory.path().join("data"),
        );
        if telegram {
            config.telegram = Some(TelegramConfig {
                token: TOKEN.into(),
                owners: vec![100],
            });
        }
        let pool =
            crate::infrastructure::db::connect(directory.path().join("a.sqlite").to_str().unwrap())
                .await
                .unwrap();
        let state = Arc::new(AppState::new(config, pool));
        let router = axum::Router::new()
            .nest(
                "/api",
                crate::http::router().layer(axum::middleware::from_fn_with_state(
                    state.clone(),
                    require_login,
                )),
            )
            .with_state(state.clone());
        (directory, state, router)
    }

    async fn get(router: &axum::Router, uri: &str, headers: &[(&str, &str)]) -> Response {
        use tower::ServiceExt;
        let mut request = axum::http::Request::builder().uri(uri);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        router
            .clone()
            .oneshot(request.body(axum::body::Body::empty()).unwrap())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn without_telegram_the_local_page_needs_no_login() {
        let (_directory, _state, router) = app(false).await;
        assert_eq!(
            get(&router, "/api/status", &[]).await.status(),
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn with_telegram_a_one_time_link_logs_a_browser_in() {
        let (_directory, state, router) = app(true).await;
        assert_eq!(
            get(&router, "/api/status", &[]).await.status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            get(&router, "/api/health", &[]).await.status(),
            StatusCode::OK
        );

        let link = create_login_link(&state.pool, 100, "https://magpie.example")
            .await
            .unwrap();
        let path = link.trim_start_matches("https://magpie.example");
        let response = get(&router, path, &[]).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let cookie = response.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .to_owned();
        assert!(cookie.contains("HttpOnly"));
        let session = cookie.split(';').next().unwrap().to_owned();

        // The link works once.
        assert_eq!(
            get(&router, path, &[]).await.status(),
            StatusCode::UNAUTHORIZED
        );
        // The browser is logged in.
        assert_eq!(
            get(&router, "/api/status", &[("cookie", &session)])
                .await
                .status(),
            StatusCode::OK
        );
        // Logging out ends it.
        use tower::ServiceExt;
        let logout = axum::http::Request::builder()
            .method("POST")
            .uri("/api/logout")
            .header("cookie", &session)
            .body(axum::body::Body::empty())
            .unwrap();
        router.clone().oneshot(logout).await.unwrap();
        assert_eq!(
            get(&router, "/api/status", &[("cookie", &session)])
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn the_mini_app_signs_in_owners_and_turns_guests_away() {
        let (_directory, state, router) = app(true).await;
        let now = Utc::now().timestamp().to_string();
        let owner = format!(
            "tma {}",
            signed(&[("auth_date", &now), ("user", USER)], TOKEN)
        );
        assert_eq!(
            get(&router, "/api/status", &[("authorization", &owner)])
                .await
                .status(),
            StatusCode::OK
        );
        // /api/me also gives the Mini App a cookie for images and audio.
        let me = get(&router, "/api/me", &[("authorization", &owner)]).await;
        assert_eq!(me.status(), StatusCode::OK);
        let session = me.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        // Reloading the Mini App with that cookie keeps the same session.
        let again = get(
            &router,
            "/api/me",
            &[("authorization", &owner), ("cookie", &session)],
        )
        .await;
        assert_eq!(again.status(), StatusCode::OK);
        assert!(!again.headers().contains_key(header::SET_COOKIE));
        let sessions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
            .fetch_one(&state.pool)
            .await
            .unwrap();
        assert_eq!(sessions, 1);

        sqlx::query("INSERT INTO telegram_guests(user_id,name,invited_by,created_at) VALUES(200,'g',100,'now')")
            .execute(&state.pool)
            .await
            .unwrap();
        let guest_user = r#"{"id":200,"first_name":"Guest"}"#;
        let guest = format!(
            "tma {}",
            signed(&[("auth_date", &now), ("user", guest_user)], TOKEN)
        );
        assert_eq!(
            get(&router, "/api/status", &[("authorization", &guest)])
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        // A stranger with real Telegram data is still nobody here.
        let stranger_user = r#"{"id":300,"first_name":"Stranger"}"#;
        let stranger = format!(
            "tma {}",
            signed(&[("auth_date", &now), ("user", stranger_user)], TOKEN)
        );
        assert_eq!(
            get(&router, "/api/status", &[("authorization", &stranger)])
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }

    #[test]
    fn cookies_are_read_by_name() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("theme=dark; magpie_session=abc"),
        );
        assert_eq!(cookie(&headers, COOKIE).as_deref(), Some("abc"));
        assert_eq!(cookie(&headers, "missing"), None);
    }
}
