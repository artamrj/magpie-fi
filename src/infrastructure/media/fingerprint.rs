use crate::infrastructure::process;
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::{path::Path, time::Duration};
use tokio::process::Command;

#[derive(Deserialize)]
struct Output {
    fingerprint: String,
    duration: f64,
}

/// Chromaprint's `fpcalc`: the AcoustID fingerprint and the duration.
pub async fn calculate(path: &Path) -> Result<(String, f64)> {
    let mut command = Command::new("fpcalc");
    command.arg("-json").arg(path);
    let result = process::run(command, Duration::from_secs(180)).await?;
    if !result.status.success() {
        bail!("fpcalc failed: {}", String::from_utf8_lossy(&result.stderr));
    }
    let parsed: Output = serde_json::from_slice(&result.stdout).context("invalid fpcalc output")?;
    Ok((parsed.fingerprint, parsed.duration))
}
