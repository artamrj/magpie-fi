//! Running external tools (ffmpeg, fpcalc, rsgain, SongRec, yt-dlp).

use anyhow::{Context, Result};
use std::{
    collections::HashMap,
    path::PathBuf,
    process::{Output, Stdio},
    sync::{Mutex, OnceLock},
    time::Duration,
};
use tokio::process::Command;

/// Runs `command` to the end and returns its output. A command that takes
/// longer than `timeout` is killed, not left running in the background.
pub async fn run(mut command: Command, timeout: Duration) -> Result<Output> {
    let program = command
        .as_std()
        .get_program()
        .to_string_lossy()
        .into_owned();
    command.stdin(Stdio::null()).kill_on_drop(true);
    tokio::time::timeout(timeout, command.output())
        .await
        .with_context(|| format!("{program} timed out after {} s", timeout.as_secs()))?
        .with_context(|| format!("could not start {program}"))
}

/// Does `program <argument>` (usually `--version`) run successfully? Each
/// program is probed once; the answer is remembered until magpie-fi restarts.
pub fn available(program: impl Into<PathBuf>, argument: &str) -> bool {
    static KNOWN: OnceLock<Mutex<HashMap<(PathBuf, String), bool>>> = OnceLock::new();
    let key = (program.into(), argument.to_owned());
    let known = KNOWN.get_or_init(Mutex::default);
    if let Some(answer) = known.lock().ok().and_then(|map| map.get(&key).copied()) {
        return answer;
    }
    let answer = std::process::Command::new(&key.0)
        .arg(argument)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if let Ok(mut map) = known.lock() {
        map.insert(key, answer);
    }
    answer
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn slow_commands_are_stopped() {
        let mut command = Command::new("sleep");
        command.arg("5");
        let started = std::time::Instant::now();
        let error = run(command, Duration::from_millis(100)).await.unwrap_err();
        assert!(error.to_string().contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn missing_programs_are_not_available() {
        assert!(!available("magpie-fi-no-such-program", "--version"));
        assert!(!available("magpie-fi-no-such-program", "--version"));
    }
}
