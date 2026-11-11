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

/// [`run`] for code that is not async (tag reading and writing run on
/// blocking threads). The command is killed when it takes longer than `timeout`.
pub fn run_blocking(mut command: std::process::Command, timeout: Duration) -> Result<Output> {
    use std::io::Read;
    let program = command.get_program().to_string_lossy().into_owned();
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("could not start {program}"))?;
    // Both pipes are read while the command runs, so a talkative command
    // never blocks on a full pipe.
    let reader = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut bytes);
            }
            bytes
        })
    };
    let stdout = reader(child.stdout.take().map(|pipe| Box::new(pipe) as _));
    let stderr = reader(child.stderr.take().map(|pipe| Box::new(pipe) as _));
    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        match child.try_wait()? {
            Some(status) => break status,
            None if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                anyhow::bail!("{program} timed out after {} s", timeout.as_secs());
            }
            None => std::thread::sleep(Duration::from_millis(5)),
        }
    };
    Ok(Output {
        status,
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
    })
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

    #[test]
    fn slow_blocking_commands_are_stopped_and_output_is_read() {
        let mut command = std::process::Command::new("sleep");
        command.arg("5");
        let started = std::time::Instant::now();
        let error = run_blocking(command, Duration::from_millis(100)).unwrap_err();
        assert!(error.to_string().contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(3));

        let mut command = std::process::Command::new("sh");
        command.args(["-c", "echo out; echo err >&2; exit 3"]);
        let output = run_blocking(command, Duration::from_secs(5)).unwrap();
        assert_eq!(output.status.code(), Some(3));
        assert_eq!(output.stdout, b"out\n");
        assert_eq!(output.stderr, b"err\n");
    }

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
