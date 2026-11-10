//! Runtime configuration. Everything comes from `MAGPIE_*` environment variables
//! (usually a `.env` file read by Docker Compose). Nothing is stored in the database,
//! so the environment is the one place to look.

use anyhow::{Result, bail};
use std::path::{Path, PathBuf};

/// Sent with every outgoing HTTP request. MusicBrainz and LRCLIB ask for a
/// descriptive agent with a way to reach the project.
pub const USER_AGENT: &str = concat!(
    "magpie-fi/",
    env!("CARGO_PKG_VERSION"),
    " ( https://github.com/artamrj/magpie-fi )"
);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NavidromeConfig {
    pub url: String,
    pub user: String,
    pub password: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TelegramConfig {
    pub token: String,
    /// Telegram user IDs that may use the bot and invite guests.
    pub owners: Vec<i64>,
    /// Always `https://api.telegram.org`; tests point it at a fake server.
    pub api_url: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioFormat {
    M4a,
    Opus,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub db_path: String,
    /// `MUSIC_ROOT/inbox`: new music waits here (your drop zone).
    pub input_dir: String,
    /// `MUSIC_ROOT/library`: the finished library Navidrome plays.
    pub output_dir: String,
    /// `MUSIC_ROOT/.magpie/trash`: originals and replaced files, kept for `trash_days`.
    pub trash_dir: PathBuf,
    /// `MUSIC_ROOT/.magpie/staging`: downloads waiting to be identified and filed.
    pub staging_dir: PathBuf,
    /// The data folder (database, the self-updated yt-dlp).
    pub data_dir: PathBuf,
    pub inbox_check_seconds: u64,
    /// Files changed more recently than this are still being copied and wait.
    pub inbox_settle_seconds: u64,
    pub trash_days: u64,
    pub lyrics: bool,
    pub navidrome: Option<NavidromeConfig>,
    pub telegram: Option<TelegramConfig>,
    /// The magpie rhyme at the end of finished Telegram requests.
    pub rhyme: bool,
    /// The web page's public HTTPS address (needed for the Telegram Mini App).
    pub public_url: Option<String>,
    /// `m4a` or `opus`: YouTube's own audio stream, never re-encoded to another lossy format.
    pub audio_format: AudioFormat,
    pub max_parallel_downloads: usize,
    pub download_pause_seconds: u64,
    pub max_download_seconds: u64,
    pub max_collection_tracks: usize,
    pub ytdlp_bin: String,
    pub ytdlp_cookies: Option<PathBuf>,
    /// Keep a self-updating copy of yt-dlp in `data_dir/bin` (the Docker image does).
    pub ytdlp_self_update: bool,
    pub acoustid_key: String,
    pub audd_token: String,
    pub spotify_client_id: String,
    pub spotify_client_secret: String,
    pub soundcloud_client_id: String,
    pub soundcloud_client_secret: String,
    pub youtube_api_key: String,
    pub discogs_token: String,
    pub lastfm_key: String,
    pub theaudiodb_key: String,
    pub musicbrainz_user_agent: String,
    pub scan_workers: usize,
    pub fingerprint_workers: usize,
    pub lookup_workers: usize,
    pub write_workers: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self::with_folders(Path::new(".local/music"), Path::new(".local/data"))
    }
}

impl Config {
    /// Defaults for a music root and a data folder.
    pub fn with_folders(music_root: &Path, data_dir: &Path) -> Self {
        let path = |value: PathBuf| value.to_string_lossy().into_owned();
        Self {
            db_path: path(data_dir.join("magpie.sqlite")),
            input_dir: path(music_root.join("inbox")),
            output_dir: path(music_root.join("library")),
            trash_dir: music_root.join(".magpie").join("trash"),
            staging_dir: music_root.join(".magpie").join("staging"),
            data_dir: data_dir.to_path_buf(),
            inbox_check_seconds: 60,
            inbox_settle_seconds: 30,
            trash_days: 30,
            lyrics: true,
            navidrome: None,
            telegram: None,
            rhyme: true,
            public_url: None,
            audio_format: AudioFormat::M4a,
            max_parallel_downloads: 2,
            download_pause_seconds: 3,
            max_download_seconds: 900,
            max_collection_tracks: 1000,
            ytdlp_bin: "yt-dlp".into(),
            ytdlp_cookies: None,
            ytdlp_self_update: false,
            acoustid_key: String::new(),
            audd_token: String::new(),
            spotify_client_id: String::new(),
            spotify_client_secret: String::new(),
            soundcloud_client_id: String::new(),
            soundcloud_client_secret: String::new(),
            youtube_api_key: String::new(),
            discogs_token: String::new(),
            lastfm_key: String::new(),
            theaudiodb_key: String::new(),
            musicbrainz_user_agent: USER_AGENT.into(),
            scan_workers: 6,
            fingerprint_workers: 3,
            lookup_workers: 3,
            write_workers: 2,
        }
    }

    pub fn from_env() -> Result<Self> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// Builds the configuration from any key/value source (the environment in
    /// production, a map in tests). Empty values count as unset.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let get = |name: &str| {
            lookup(name)
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        };
        let music_root = get("MAGPIE_MUSIC_ROOT").unwrap_or_else(|| ".local/music".into());
        let data_dir = get("MAGPIE_DATA_DIR").unwrap_or_else(|| ".local/data".into());
        let mut config = Self::with_folders(Path::new(&music_root), Path::new(&data_dir));

        let number = |name: &str, default: u64| -> Result<u64> {
            match get(name) {
                None => Ok(default),
                Some(value) => value
                    .parse()
                    .map_err(|_| anyhow::anyhow!("{name} must be a whole number, got '{value}'")),
            }
        };
        let flag = |name: &str, default: bool| -> Result<bool> {
            match get(name) {
                None => Ok(default),
                Some(value) => parse_bool(&value)
                    .ok_or_else(|| anyhow::anyhow!("{name} must be true or false, got '{value}'")),
            }
        };
        config.inbox_check_seconds = number("MAGPIE_INBOX_CHECK_SECONDS", 60)?;
        config.inbox_settle_seconds = number("MAGPIE_INBOX_SETTLE_SECONDS", 30)?;
        config.trash_days = number("MAGPIE_TRASH_DAYS", 30)?;
        config.lyrics = flag("MAGPIE_LYRICS", true)?;
        config.audio_format = match get("MAGPIE_AUDIO_FORMAT").as_deref() {
            None | Some("m4a") => AudioFormat::M4a,
            Some("opus") => AudioFormat::Opus,
            Some(other) => bail!("MAGPIE_AUDIO_FORMAT must be m4a or opus, got '{other}'"),
        };
        config.max_parallel_downloads =
            usize::try_from(number("MAGPIE_MAX_PARALLEL_DOWNLOADS", 2)?).unwrap_or(2);
        config.download_pause_seconds = number("MAGPIE_DOWNLOAD_PAUSE_SECONDS", 3)?;
        config.max_download_seconds = number("MAGPIE_MAX_DOWNLOAD_SECONDS", 900)?;
        config.max_collection_tracks =
            usize::try_from(number("MAGPIE_MAX_COLLECTION_TRACKS", 1000)?).unwrap_or(1000);
        if let Some(bin) = get("MAGPIE_YTDLP_BIN") {
            config.ytdlp_bin = bin;
        }
        config.ytdlp_cookies = get("MAGPIE_YTDLP_COOKIES").map(PathBuf::from);
        config.ytdlp_self_update = flag("MAGPIE_YTDLP_SELF_UPDATE", false)?;

        // Without a login there is no Navidrome connection (the URL alone is fine:
        // Docker Compose always sets it). A login without a URL, or half a login,
        // is a mistake worth stopping for.
        config.navidrome = match (
            get("MAGPIE_NAVIDROME_URL"),
            get("MAGPIE_NAVIDROME_USER"),
            get("MAGPIE_NAVIDROME_PASSWORD"),
        ) {
            (_, None, None) => None,
            (Some(url), Some(user), Some(password)) => Some(NavidromeConfig {
                url: url.trim_end_matches('/').to_owned(),
                user,
                password,
            }),
            _ => bail!(
                "to connect Navidrome set MAGPIE_NAVIDROME_URL, MAGPIE_NAVIDROME_USER \
                 and MAGPIE_NAVIDROME_PASSWORD together"
            ),
        };

        config.telegram = match get("MAGPIE_TELEGRAM_BOT_TOKEN") {
            None => None,
            Some(token) => {
                let owners = parse_user_ids(&get("MAGPIE_TELEGRAM_OWNERS").unwrap_or_default())?;
                if owners.is_empty() {
                    bail!(
                        "MAGPIE_TELEGRAM_OWNERS needs at least one Telegram user ID \
                         (send /id to the bot to see yours)"
                    );
                }
                Some(TelegramConfig {
                    token,
                    owners,
                    api_url: "https://api.telegram.org".into(),
                })
            }
        };
        config.rhyme = flag("MAGPIE_RHYME", true)?;
        config.public_url =
            get("MAGPIE_PUBLIC_URL").map(|url| url.trim_end_matches('/').to_owned());
        if let Some(url) = config.public_url.as_deref()
            && !(url.starts_with("https://") || url.starts_with("http://"))
        {
            bail!("MAGPIE_PUBLIC_URL must start with https:// (or http:// on your own network)");
        }

        let text = |field: &mut String, name: &str| {
            if let Some(value) = get(name) {
                *field = value;
            }
        };
        text(&mut config.acoustid_key, "MAGPIE_ACOUSTID_KEY");
        text(&mut config.audd_token, "MAGPIE_AUDD_TOKEN");
        text(&mut config.spotify_client_id, "MAGPIE_SPOTIFY_CLIENT_ID");
        text(
            &mut config.spotify_client_secret,
            "MAGPIE_SPOTIFY_CLIENT_SECRET",
        );
        text(
            &mut config.soundcloud_client_id,
            "MAGPIE_SOUNDCLOUD_CLIENT_ID",
        );
        text(
            &mut config.soundcloud_client_secret,
            "MAGPIE_SOUNDCLOUD_CLIENT_SECRET",
        );
        text(&mut config.youtube_api_key, "MAGPIE_YOUTUBE_API_KEY");
        text(&mut config.discogs_token, "MAGPIE_DISCOGS_TOKEN");
        text(&mut config.lastfm_key, "MAGPIE_LASTFM_KEY");
        text(&mut config.theaudiodb_key, "MAGPIE_THEAUDIODB_KEY");
        text(
            &mut config.musicbrainz_user_agent,
            "MAGPIE_MUSICBRAINZ_USER_AGENT",
        );
        if config.public_url.is_some() && config.telegram.is_none() {
            bail!(
                "MAGPIE_PUBLIC_URL needs Telegram: without MAGPIE_TELEGRAM_BOT_TOKEN the web page \
                 has no login and must not be reachable from the internet"
            );
        }
        config.normalize();
        Ok(config)
    }

    pub fn normalize(&mut self) {
        self.scan_workers = self.scan_workers.clamp(1, 32);
        self.fingerprint_workers = self.fingerprint_workers.clamp(1, 16);
        self.lookup_workers = self.lookup_workers.clamp(1, 16);
        self.write_workers = self.write_workers.clamp(1, 8);
        self.inbox_check_seconds = self.inbox_check_seconds.clamp(10, 24 * 60 * 60);
        self.inbox_settle_seconds = self.inbox_settle_seconds.min(60 * 60);
        self.trash_days = self.trash_days.clamp(1, 3650);
        self.max_parallel_downloads = self.max_parallel_downloads.clamp(1, 8);
        self.max_download_seconds = self.max_download_seconds.clamp(30, 6 * 60 * 60);
        self.max_collection_tracks = self.max_collection_tracks.clamp(1, 10_000);
    }
}

/// "123, 456;789" → [123, 456, 789].
fn parse_user_ids(value: &str) -> Result<Vec<i64>> {
    value
        .split([',', ';', ' '])
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| {
            part.parse::<i64>()
                .map_err(|_| anyhow::anyhow!("'{part}' in MAGPIE_TELEGRAM_OWNERS is not a user ID"))
        })
        .collect()
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn from(pairs: &[(&str, &str)]) -> Result<Config> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect();
        Config::from_lookup(|name| map.get(name).cloned())
    }

    #[test]
    fn folders_come_from_the_music_root_and_data_dir() {
        let config = from(&[
            ("MAGPIE_MUSIC_ROOT", "/srv/music"),
            ("MAGPIE_DATA_DIR", "/data"),
        ])
        .unwrap();
        assert_eq!(config.input_dir, "/srv/music/inbox");
        assert_eq!(config.output_dir, "/srv/music/library");
        assert_eq!(config.trash_dir, PathBuf::from("/srv/music/.magpie/trash"));
        assert_eq!(
            config.staging_dir,
            PathBuf::from("/srv/music/.magpie/staging")
        );
        assert_eq!(config.db_path, "/data/magpie.sqlite");
    }

    #[test]
    fn navidrome_connects_only_with_a_full_login() {
        let url_only = from(&[("MAGPIE_NAVIDROME_URL", "http://nd:4533")]).unwrap();
        assert!(url_only.navidrome.is_none());
        assert!(
            from(&[
                ("MAGPIE_NAVIDROME_URL", "http://nd:4533"),
                ("MAGPIE_NAVIDROME_USER", "admin"),
            ])
            .is_err()
        );
        assert!(
            from(&[
                ("MAGPIE_NAVIDROME_USER", "admin"),
                ("MAGPIE_NAVIDROME_PASSWORD", "secret"),
            ])
            .is_err()
        );
        let config = from(&[
            ("MAGPIE_NAVIDROME_URL", "http://nd:4533/"),
            ("MAGPIE_NAVIDROME_USER", "admin"),
            ("MAGPIE_NAVIDROME_PASSWORD", "secret"),
        ])
        .unwrap();
        assert_eq!(config.navidrome.unwrap().url, "http://nd:4533");
    }

    #[test]
    fn invalid_numbers_and_flags_are_rejected_with_the_variable_name() {
        let error = from(&[("MAGPIE_INBOX_CHECK_SECONDS", "soon")]).unwrap_err();
        assert!(error.to_string().contains("MAGPIE_INBOX_CHECK_SECONDS"));
        let error = from(&[("MAGPIE_LYRICS", "maybe")]).unwrap_err();
        assert!(error.to_string().contains("MAGPIE_LYRICS"));
    }

    #[test]
    fn download_settings_are_read_and_validated() {
        let config = from(&[
            ("MAGPIE_AUDIO_FORMAT", "opus"),
            ("MAGPIE_MAX_PARALLEL_DOWNLOADS", "20"),
            ("MAGPIE_YTDLP_COOKIES", "/config/cookies.txt"),
        ])
        .unwrap();
        assert_eq!(config.audio_format, AudioFormat::Opus);
        assert_eq!(config.max_parallel_downloads, 8);
        assert_eq!(
            config.ytdlp_cookies,
            Some(PathBuf::from("/config/cookies.txt"))
        );
        assert!(from(&[("MAGPIE_AUDIO_FORMAT", "mp3")]).is_err());
    }

    #[test]
    fn telegram_needs_a_token_and_owners() {
        assert!(from(&[]).unwrap().telegram.is_none());
        assert!(from(&[("MAGPIE_TELEGRAM_BOT_TOKEN", "1:abc")]).is_err());
        let config = from(&[
            ("MAGPIE_TELEGRAM_BOT_TOKEN", "1:abc"),
            ("MAGPIE_TELEGRAM_OWNERS", "123, 456;789"),
        ])
        .unwrap();
        let telegram = config.telegram.unwrap();
        assert_eq!(telegram.owners, vec![123, 456, 789]);
        assert_eq!(telegram.api_url, "https://api.telegram.org");
        assert!(
            from(&[
                ("MAGPIE_TELEGRAM_BOT_TOKEN", "1:abc"),
                ("MAGPIE_TELEGRAM_OWNERS", "me")
            ])
            .is_err()
        );
    }

    #[test]
    fn a_public_address_requires_the_telegram_login() {
        assert!(from(&[("MAGPIE_PUBLIC_URL", "https://magpie.example")]).is_err());
        let config = from(&[
            ("MAGPIE_PUBLIC_URL", "https://magpie.example/"),
            ("MAGPIE_TELEGRAM_BOT_TOKEN", "1:abc"),
            ("MAGPIE_TELEGRAM_OWNERS", "1"),
        ])
        .unwrap();
        assert_eq!(config.public_url.as_deref(), Some("https://magpie.example"));
        assert!(from(&[("MAGPIE_PUBLIC_URL", "magpie.example")]).is_err());
    }

    #[test]
    fn empty_values_count_as_unset() {
        let config = from(&[("MAGPIE_ACOUSTID_KEY", "  "), ("MAGPIE_LYRICS", "")]).unwrap();
        assert!(config.acoustid_key.is_empty());
        assert!(config.lyrics);
    }

    #[test]
    fn unsafe_values_are_bounded() {
        let mut config = Config {
            scan_workers: 0,
            fingerprint_workers: usize::MAX,
            lookup_workers: 0,
            write_workers: usize::MAX,
            inbox_check_seconds: 0,
            trash_days: 0,
            ..Config::default()
        };
        config.normalize();
        assert_eq!(config.scan_workers, 1);
        assert_eq!(config.fingerprint_workers, 16);
        assert_eq!(config.lookup_workers, 1);
        assert_eq!(config.write_workers, 8);
        assert_eq!(config.inbox_check_seconds, 10);
        assert_eq!(config.trash_days, 1);
    }

    #[test]
    fn boolean_values_are_parsed() {
        assert_eq!(parse_bool("true"), Some(true));
        assert_eq!(parse_bool(" ON "), Some(true));
        assert_eq!(parse_bool("0"), Some(false));
        assert_eq!(parse_bool("maybe"), None);
    }
}
