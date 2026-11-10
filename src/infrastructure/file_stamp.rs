use anyhow::{Context, Result};
use std::{path::Path, time::UNIX_EPOCH};

/// Size and modification time (nanoseconds): when both are unchanged, a
/// cached result for the file is still valid.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FileStamp {
    pub size: i64,
    pub mtime_ns: i64,
}

impl FileStamp {
    pub async fn of(path: &Path) -> Result<Self> {
        let metadata = tokio::fs::metadata(path).await?;
        Ok(Self {
            size: i64::try_from(metadata.len()).context("file is too large")?,
            mtime_ns: metadata
                .modified()?
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
                .try_into()
                .context("file modification time is out of range")?,
        })
    }
}
