//! A small Subsonic API client for Navidrome: ask it to scan after new songs
//! were filed, and check that it is reachable.

use crate::config::NavidromeConfig;
use anyhow::{Result, bail};
use md5::{Digest, Md5};
use serde_json::Value;

const API_VERSION: &str = "1.16.1";
const CLIENT_NAME: &str = "magpie-fi";

/// Starts a quick scan (only changed folders are read).
pub async fn start_scan(client: &reqwest::Client, config: &NavidromeConfig) -> Result<()> {
    call(client, config, "startScan").await.map(|_| ())
}

pub async fn ping(client: &reqwest::Client, config: &NavidromeConfig) -> Result<()> {
    call(client, config, "ping").await.map(|_| ())
}

async fn call(client: &reqwest::Client, config: &NavidromeConfig, method: &str) -> Result<Value> {
    let salt = salt();
    let token = token(&config.password, &salt);
    let url = format!("{}/rest/{method}", config.url);
    let body: Value = client
        .get(url)
        .query(&[
            ("u", config.user.as_str()),
            ("t", token.as_str()),
            ("s", salt.as_str()),
            ("v", API_VERSION),
            ("c", CLIENT_NAME),
            ("f", "json"),
        ])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let response = &body["subsonic-response"];
    if response["status"] != "ok" {
        let message = response["error"]["message"]
            .as_str()
            .unwrap_or("unknown error");
        bail!("Navidrome refused {method}: {message}");
    }
    Ok(body)
}

/// Subsonic token auth: md5(password + salt), so the password is never sent.
fn token(password: &str, salt: &str) -> String {
    hex::encode(Md5::digest(format!("{password}{salt}").as_bytes()))
}

fn salt() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    hex::encode(Md5::digest(
        format!("{nanos}-{}", std::process::id()).as_bytes(),
    ))[..12]
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_matches_the_subsonic_documentation_example() {
        // From the Subsonic API docs: password "sesame", salt "c19b2d".
        assert_eq!(
            token("sesame", "c19b2d"),
            "26719a1196d2a940705a59634eb18eab"
        );
    }

    #[test]
    fn salts_are_short_and_change() {
        let first = salt();
        assert_eq!(first.len(), 12);
        std::thread::sleep(std::time::Duration::from_millis(2));
        assert_ne!(first, salt());
    }
}
