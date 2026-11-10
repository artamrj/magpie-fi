//! Runs yt-dlp (https://github.com/yt-dlp/yt-dlp) as a program: YouTube Music
//! search, video details, playlist listing and audio download. The Docker image
//! bundles the standalone yt-dlp and Deno (its JavaScript runtime for YouTube).

use crate::{
    config::{AudioFormat, Config},
    infrastructure::providers::text,
};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

const LOOKUP_TIMEOUT: Duration = Duration::from_secs(90);

#[derive(Clone, Debug, Default)]
pub struct Entry {
    pub id: String,
    pub title: String,
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub duration: Option<f64>,
}

#[derive(Clone, Debug, Default)]
pub struct Listing {
    pub title: Option<String>,
    pub owner: Option<String>,
    pub entries: Vec<Entry>,
}

/// The yt-dlp to run: the self-updated copy in the data folder when there is
/// one, otherwise the configured binary.
pub fn binary(config: &Config) -> PathBuf {
    let updated = updated_copy(config);
    if config.ytdlp_self_update && updated.is_file() {
        updated
    } else {
        PathBuf::from(&config.ytdlp_bin)
    }
}

fn updated_copy(config: &Config) -> PathBuf {
    config.data_dir.join("bin").join("yt-dlp")
}

pub fn available(config: &Config) -> bool {
    crate::infrastructure::process::available(binary(config), "--version")
}

/// Keeps a writable copy of the bundled yt-dlp in `data/bin` and updates it to
/// the latest stable release, so YouTube fixes arrive without a new image.
pub async fn self_update(config: &Config) -> Result<String> {
    if !config.ytdlp_self_update {
        return Ok("self-update is off".into());
    }
    let copy = updated_copy(config);
    if !copy.is_file() {
        let bundled = which(&config.ytdlp_bin).context("bundled yt-dlp not found")?;
        if let Some(parent) = copy.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::copy(&bundled, &copy).await?;
        set_executable(&copy).await?;
    }
    let output = tokio::time::timeout(
        Duration::from_secs(300),
        tokio::process::Command::new(&copy)
            .args(["--update-to", "stable"])
            .kill_on_drop(true)
            .output(),
    )
    .await
    .context("yt-dlp update timed out")??;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if !output.status.success() {
        bail!(
            "yt-dlp update failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(text.lines().last().unwrap_or("updated").to_owned())
}

#[cfg(unix)]
async fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).await?;
    Ok(())
}

#[cfg(not(unix))]
async fn set_executable(_path: &Path) -> Result<()> {
    Ok(())
}

fn which(program: &str) -> Option<PathBuf> {
    let candidate = PathBuf::from(program);
    if candidate.is_absolute() {
        return candidate.is_file().then_some(candidate);
    }
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|folder| folder.join(program))
            .find(|path| path.is_file())
    })
}

fn command(config: &Config) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(binary(config));
    command
        .args(["--no-warnings", "--ignore-config", "--no-cache-dir"])
        .kill_on_drop(true);
    if let Some(cookies) = config.ytdlp_cookies.as_ref().filter(|path| path.is_file()) {
        command.arg("--cookies").arg(cookies);
    }
    command
}

async fn json(config: &Config, args: &[&str]) -> Result<Value> {
    let output = tokio::time::timeout(LOOKUP_TIMEOUT, command(config).args(args).output())
        .await
        .context("yt-dlp did not answer in time")??;
    if !output.status.success() {
        bail!("{}", last_error_line(&output.stderr));
    }
    serde_json::from_slice(&output.stdout).context("yt-dlp printed something that is not JSON")
}

/// The most useful line of yt-dlp's error output: the last `ERROR:` line, or
/// else the last line it printed.
fn last_error_line(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    lines
        .iter()
        .rev()
        .find(|line| line.starts_with("ERROR"))
        .or(lines.last())
        .map(|line| line.trim_start_matches("ERROR: ").to_owned())
        .unwrap_or_else(|| "yt-dlp failed".into())
}

/// Official songs on YouTube Music. Only id and title come back here; call
/// [`details`] for the artist and length.
pub async fn search_music_songs(config: &Config, query: &str, limit: usize) -> Result<Vec<Entry>> {
    let url = format!(
        "https://music.youtube.com/search?q={}#songs",
        percent_encoding::utf8_percent_encode(query, percent_encoding::NON_ALPHANUMERIC)
    );
    let items = format!("1-{limit}");
    let value = json(
        config,
        &["--flat-playlist", "-J", "--playlist-items", &items, &url],
    )
    .await?;
    Ok(entries(&value))
}

/// Ordinary YouTube videos (the fallback when YouTube Music has no match).
pub async fn search_videos(config: &Config, query: &str, limit: usize) -> Result<Vec<Entry>> {
    let search = format!("ytsearch{limit}:{query}");
    let value = json(config, &["--flat-playlist", "-J", &search]).await?;
    Ok(entries(&value))
}

/// Full details of one video: artists, album and length.
pub async fn details(config: &Config, id_or_url: &str) -> Result<Entry> {
    let value = json(
        config,
        &[
            "-J",
            "--skip-download",
            "--no-playlist",
            &watch_url(id_or_url),
        ],
    )
    .await?;
    Ok(entry(&value))
}

/// The songs of a YouTube or YouTube Music playlist.
pub async fn playlist(config: &Config, url: &str) -> Result<Listing> {
    let value = json(config, &["--flat-playlist", "-J", url]).await?;
    Ok(Listing {
        title: text(&value, "title"),
        owner: text(&value, "uploader").or_else(|| text(&value, "channel")),
        entries: entries(&value),
    })
}

/// Downloads the audio into `folder` as `<stem>.<opus|m4a>`. YouTube's own
/// stream is kept (no re-encoding to another lossy format). The file is
/// written under `folder/.part` first and moved into place when complete.
pub async fn download(
    config: &Config,
    id_or_url: &str,
    folder: &Path,
    stem: &str,
) -> Result<PathBuf> {
    let part = folder.join(".part");
    tokio::fs::create_dir_all(&part).await?;
    // `best` keeps the stream's own codec in its own container (Opus in .opus, AAC in .m4a).
    // A named format re-encodes only when the chosen stream is not already that codec.
    let (selector, codec) = match config.audio_format {
        AudioFormat::Best => (
            "bestaudio[acodec=opus]/bestaudio[ext=m4a]/bestaudio",
            "best",
        ),
        AudioFormat::M4a => ("bestaudio[ext=m4a]/bestaudio", "m4a"),
        AudioFormat::Opus => ("bestaudio[acodec=opus]/bestaudio", "opus"),
    };
    let template = part.join(format!("{stem}.%(ext)s"));
    let mut child = command(config);
    child
        .args([
            "--no-playlist",
            "--no-mtime",
            "--no-progress",
            "-f",
            selector,
        ])
        .args(["-x", "--audio-format", codec, "--audio-quality", "0"])
        .arg("-o")
        .arg(&template)
        .args(["--print", "after_move:filepath"])
        .arg(watch_url(id_or_url));
    let output = tokio::time::timeout(
        Duration::from_secs(config.max_download_seconds),
        child.output(),
    )
    .await;
    let output = match output {
        Ok(result) => result?,
        Err(_) => {
            remove_partials(&part, stem).await;
            bail!(
                "download took longer than {} seconds",
                config.max_download_seconds
            );
        }
    };
    if !output.status.success() {
        remove_partials(&part, stem).await;
        bail!("{}", last_error_line(&output.stderr));
    }
    let printed = String::from_utf8_lossy(&output.stdout);
    let downloaded = printed
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_file())
        .context("yt-dlp finished but the audio file is missing")?;
    let extension = downloaded
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or(codec)
        .to_ascii_lowercase();
    let target = folder.join(format!("{stem}.{extension}"));
    tokio::fs::rename(&downloaded, &target).await?;
    remove_partials(&part, stem).await;
    Ok(target)
}

async fn remove_partials(part: &Path, stem: &str) {
    let Ok(mut entries) = tokio::fs::read_dir(part).await else {
        return;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        if entry.file_name().to_string_lossy().starts_with(stem) {
            let _ = tokio::fs::remove_file(entry.path()).await;
        }
    }
}

fn watch_url(id_or_url: &str) -> String {
    if id_or_url.starts_with("http") {
        id_or_url.to_owned()
    } else {
        format!("https://www.youtube.com/watch?v={id_or_url}")
    }
}

fn entries(value: &Value) -> Vec<Entry> {
    value["entries"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(entry)
                .filter(|entry| !entry.id.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn entry(value: &Value) -> Entry {
    let mut artists: Vec<String> = value["artists"]
        .as_array()
        .map(|names| {
            names
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    if artists.is_empty()
        && let Some(artist) = text(value, "artist")
            .or_else(|| text(value, "channel"))
            .or_else(|| text(value, "uploader"))
    {
        artists.push(super::matching::clean_channel_name(&artist));
    }
    Entry {
        id: text(value, "id").unwrap_or_default(),
        title: text(value, "track")
            .or_else(|| text(value, "title"))
            .unwrap_or_default(),
        artists,
        album: text(value, "album"),
        duration: value["duration"]
            .as_f64()
            .filter(|duration| *duration > 0.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_search_results_and_details() {
        let search = serde_json::json!({
            "entries": [
                {"id": "J7p4bzqLvCw", "title": "Blinding Lights", "_type": "url"},
                {"id": "", "title": "broken"}
            ]
        });
        let found = entries(&search);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "J7p4bzqLvCw");

        let details = serde_json::json!({
            "id": "J7p4bzqLvCw", "title": "Blinding Lights", "track": "Blinding Lights",
            "artists": ["The Weeknd"], "album": "After Hours", "duration": 202
        });
        let found = entry(&details);
        assert_eq!(found.artists, vec!["The Weeknd"]);
        assert_eq!(found.duration, Some(202.0));

        let video = serde_json::json!({"id": "x", "title": "Song", "channel": "Artist - Topic"});
        assert_eq!(entry(&video).artists, vec!["Artist"]);
    }

    #[test]
    fn plain_ids_become_watch_links() {
        assert_eq!(watch_url("abc"), "https://www.youtube.com/watch?v=abc");
        assert_eq!(watch_url("https://youtu.be/abc"), "https://youtu.be/abc");
    }

    #[test]
    fn the_error_line_is_reported() {
        let stderr = b"WARNING: something\nERROR: [youtube] abc: Video unavailable\n";
        assert_eq!(last_error_line(stderr), "[youtube] abc: Video unavailable");
    }
}
