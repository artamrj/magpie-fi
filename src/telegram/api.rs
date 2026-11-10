//! A small Telegram Bot API client: frankenstein's typed parameters and
//! results, sent with magpie's own HTTP client.
//!
//! Bot API URLs contain the bot token, so every HTTP error drops its URL
//! before it can reach a log, the database or a message.

use anyhow::{Context, Result, bail};
use frankenstein::response::{ErrorResponse, MethodResponse};
use serde::{Serialize, de::DeserializeOwned};
use std::{path::Path, time::Duration};

/// Telegram lets bots download files up to 20 MB
/// (<https://core.telegram.org/bots/api#getfile>).
pub const DOWNLOAD_LIMIT: u64 = 20 * 1024 * 1024;

pub const TOO_BIG: &str = "Telegram only lets bots receive files up to 20 MB";

const API_URL: &str = "https://api.telegram.org";

#[derive(Clone)]
pub struct Bot {
    client: reqwest::Client,
    api_url: String,
    token: String,
}

/// Telegram answered with an error (not a network problem).
#[derive(Debug)]
pub struct TelegramError {
    pub code: u64,
    pub description: String,
}

impl std::fmt::Display for TelegramError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "Telegram error {}: {}",
            self.code, self.description
        )
    }
}

impl std::error::Error for TelegramError {}

impl Bot {
    pub fn new(token: &str) -> Result<Self> {
        Self::with_api_url(API_URL, token)
    }

    /// Tests point the bot at a fake Telegram.
    pub(super) fn with_api_url(api_url: &str, token: &str) -> Result<Self> {
        Ok(Self {
            // Long polling holds a request open for up to 50 s.
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(75))
                .user_agent(crate::config::USER_AGENT)
                .build()?,
            api_url: api_url.to_owned(),
            token: token.to_owned(),
        })
    }

    /// Calls a Bot API method. Waits and tries again when Telegram asks to
    /// slow down or cannot be reached; "message is not modified" counts as
    /// success. A message is never sent twice: `send…` methods are only
    /// repeated when the request surely did not reach Telegram.
    pub async fn call<P: Serialize + ?Sized, R: DeserializeOwned>(
        &self,
        method: &str,
        params: &P,
    ) -> Result<R> {
        let url = format!("{}/bot{}/{method}", self.api_url, self.token);
        let repeatable = !method.starts_with("send");
        for attempt in 0..4 {
            let response = match self
                .client
                .post(&url)
                .json(params)
                .send()
                .await
                .map_err(reqwest::Error::without_url)
            {
                Ok(response) => response,
                Err(error) if attempt < 3 && (repeatable || error.is_connect()) => {
                    tracing::debug!(%error, method, "Telegram request failed; retrying");
                    tokio::time::sleep(Duration::from_secs(2 << attempt)).await;
                    continue;
                }
                Err(error) => return Err(error.into()),
            };
            let body = response
                .bytes()
                .await
                .map_err(reqwest::Error::without_url)?;
            if let Ok(ok) = serde_json::from_slice::<MethodResponse<R>>(&body) {
                return Ok(ok.result);
            }
            let error: ErrorResponse = serde_json::from_slice(&body)
                .with_context(|| format!("unexpected answer from Telegram for {method}"))?;
            if let Some(wait) = error.parameters.as_ref().and_then(|p| p.retry_after) {
                tokio::time::sleep(Duration::from_secs(u64::from(wait) + 1)).await;
                continue;
            }
            return Err(TelegramError {
                code: error.error_code,
                description: error.description,
            }
            .into());
        }
        bail!("Telegram kept asking to slow down ({method})")
    }

    /// Like [`Bot::call`], but only logs failures (for status edits and the like).
    pub async fn try_call<P: Serialize + ?Sized, R: DeserializeOwned>(
        &self,
        method: &str,
        params: &P,
    ) -> Option<R> {
        match self.call(method, params).await {
            Ok(result) => Some(result),
            Err(error) if is_not_modified(&error) => None,
            Err(error) => {
                tracing::warn!(%error, method, "Telegram call failed");
                None
            }
        }
    }

    /// Saves a file sent to the bot (at most [`DOWNLOAD_LIMIT`]).
    pub async fn download(&self, file_id: &str, target: &Path) -> Result<()> {
        let file: frankenstein::types::File = match self
            .call("getFile", &serde_json::json!({"file_id": file_id}))
            .await
        {
            Ok(file) => file,
            Err(error) if error.to_string().contains("file is too big") => {
                bail!("{TOO_BIG}")
            }
            Err(error) => return Err(error),
        };
        let path = file
            .file_path
            .context("Telegram did not give a download path for this file")?;
        let url = format!("{}/file/bot{}/{path}", self.api_url, self.token);
        let bytes = self
            .client
            .get(url)
            .timeout(Duration::from_secs(600))
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(reqwest::Error::without_url)?
            .bytes()
            .await
            .map_err(reqwest::Error::without_url)?;
        tokio::fs::write(target, &bytes).await?;
        Ok(())
    }
}

pub fn is_not_modified(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<TelegramError>()
        .is_some_and(|error| error.description.contains("message is not modified"))
}

/// The message to edit no longer exists (deleted by the user).
pub fn is_gone(error: &anyhow::Error) -> bool {
    error.downcast_ref::<TelegramError>().is_some_and(|error| {
        error.description.contains("message to edit not found")
            || error.description.contains("message can't be edited")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, routing::post};

    #[tokio::test]
    async fn errors_never_show_the_bot_token() {
        // getFile works, but the file download itself fails (404).
        let app = Router::new().route(
            "/{bot}/getFile",
            post(|| async {
                Json(serde_json::json!({"ok": true, "result": {
                    "file_id": "f", "file_unique_id": "u", "file_path": "music/a.mp3"
                }}))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let token = "123:SECRET-TOKEN";
        let bot = Bot::with_api_url(&format!("http://{address}"), token).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let error = bot
            .download("f", &directory.path().join("a.mp3"))
            .await
            .unwrap_err();
        assert!(!format!("{error:#} {error:?}").contains("SECRET-TOKEN"));
    }
}
