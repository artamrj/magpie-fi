//! Album ReplayGain with rsgain (https://github.com/complexlogic/rsgain).
//! `rsgain easy` measures every song in an album folder and writes track and
//! album gain (ReplayGain 2.0, −18 LUFS; R128 tags for Opus).

use crate::infrastructure::process;
use anyhow::{Result, bail};
use std::{path::Path, time::Duration};
use tokio::process::Command;

pub fn available() -> bool {
    process::available("rsgain", "--version")
}

/// Re-measures one album folder.
pub async fn tag_album_folder(folder: &Path) -> Result<()> {
    let mut command = Command::new("rsgain");
    command.arg("easy").arg("--quiet").arg(folder);
    let output = process::run(command, Duration::from_secs(30 * 60)).await?;
    if !output.status.success() {
        bail!(
            "rsgain failed for {}: {}",
            folder.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}
