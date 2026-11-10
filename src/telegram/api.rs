//! A small Telegram Bot API client: frankenstein's typed parameters and
//! results, sent with magpie's own HTTP client.

use crate::config::TelegramConfig;
use anyhow::{Context, Result, bail};
use frankenstein::response::{ErrorResponse, MethodResponse};
use serde::{Serialize, de::DeserializeOwned};
use std::{path::Path, time::Duration};

/// Telegram lets bots download files up to 20 MB
/// (<https://core.telegram.org/bots/api#getfile>).
pub const DOWNLOAD_LIMIT: u64 = 20 * 1024 * 1024;

pub const TOO_BIG: &str = "Telegram only lets bots receive files up to 20 MB";

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
    pub fn new(config: &TelegramConfig) -> Result<Self> {
        Ok(Self {
            // Long polling holds a request open for up to 50 s.
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(75))
                .user_agent(crate::config::USER_AGENT)
                .build()?,
            api_url: config.api_url.clone(),
            token: config.token.clone(),
        })
    }

    /// Calls a Bot API method. Waits and tries again when Telegram asks to
    /// slow down; "message is not modified" counts as success.
    pub async fn call<P: Serialize + ?Sized, R: DeserializeOwned>(
        &self,
        method: &str,
        params: &P,
    ) -> Result<R> {
        let url = format!("{}/bot{}/{method}", self.api_url, self.token);
        for attempt in 0..4 {
            let response = match self.client.post(&url).json(params).send().await {
                Ok(response) => response,
                Err(error) if attempt < 3 => {
                    tracing::debug!(%error, method, "Telegram request failed; retrying");
                    tokio::time::sleep(Duration::from_secs(2 << attempt)).await;
                    continue;
                }
                Err(error) => return Err(error.into()),
            };
            let body = response.bytes().await?;
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
            .await?
            .error_for_status()?
            .bytes()
            .await?;
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
