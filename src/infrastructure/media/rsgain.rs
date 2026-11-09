//! Album ReplayGain with rsgain (https://github.com/complexlogic/rsgain).
//! `rsgain easy` measures every song in an album folder and writes track and
//! album gain (ReplayGain 2.0, −18 LUFS; R128 tags for Opus).

use anyhow::{Result, bail};
use std::path::Path;

pub fn available() -> bool {
    std::process::Command::new("rsgain")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

/// Re-measures one album folder. Runs off the async runtime.
pub async fn tag_album_folder(folder: &Path) -> Result<()> {
    let output = tokio::process::Command::new("rsgain")
        .arg("easy")
        .arg("--quiet")
        .arg(folder)
        .kill_on_drop(true)
        .output()
        .await?;
    if !output.status.success() {
        bail!(
            "rsgain failed for {}: {}",
            folder.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}
