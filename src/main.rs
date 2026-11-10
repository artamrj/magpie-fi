mod acquire;
mod app;
mod application;
mod config;
mod domain;
mod http;
mod infrastructure;
mod types;

use anyhow::{Context, Result, bail};
use axum::{Router, extract::DefaultBodyLimit, middleware};
use chrono::TimeZone;
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::net::TcpListener;
use tower_http::{
    services::{ServeDir, ServeFile},
    trace::TraceLayer,
};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let address = listen_address()?;
    let frontend = frontend_directory()?;

    let mut config = config::Config::from_env()?;
    prepare_folders(&config)?;
    resolve_folders(&mut config)?;
    let pool = infrastructure::db::connect(&config.db_path).await?;
    infrastructure::db::cleanup(&pool, &config).await?;
    infrastructure::db::run_daily_cache_cleanup_if_due(&pool).await?;
    purge_trash(&config).await;
    infrastructure::db::enforce_media_cache_limit(&pool).await?;
    let state = Arc::new(app::AppState::new(config, pool));
    let maintenance_state = state.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(duration_until_next_local_midnight()).await;
            while maintenance_state.workflow_running().await {
                tokio::time::sleep(Duration::from_secs(60)).await;
            }
            if let Err(error) =
                infrastructure::db::run_daily_cache_cleanup_if_due(&maintenance_state.pool).await
            {
                tracing::warn!(%error, "daily disposable cache cleanup failed");
            }
            let config = maintenance_state.config.read().await.clone();
            purge_trash(&config).await;
        }
    });
    let media_cache_state = state.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60 * 60));
        interval.tick().await;
        loop {
            interval.tick().await;
            if media_cache_state.workflow_running().await {
                continue;
            }
            if let Err(error) =
                infrastructure::db::enforce_media_cache_limit(&media_cache_state.pool).await
            {
                tracing::warn!(%error, "media-analysis cache limit check failed");
            }
        }
    });
    let inbox_state = state.clone();
    tokio::spawn(async move {
        // Check the inbox at a fixed interval, or right away when someone asks
        // ("Check inbox now" in the web page). A cycle never runs twice at once.
        let (interval, library_root) = {
            let config = inbox_state.config.read().await;
            (
                Duration::from_secs(config.inbox_check_seconds),
                PathBuf::from(&config.output_dir),
            )
        };
        // The library index is refreshed before the first check (so songs you
        // already own are recognised) and then every six hours.
        let mut next_index = tokio::time::Instant::now();
        loop {
            if tokio::time::Instant::now() >= next_index {
                match application::library::index_library(&inbox_state.pool, &library_root).await {
                    Ok(0) => {}
                    Ok(count) => tracing::info!(count, "indexed library files"),
                    Err(error) => tracing::warn!(%error, "library indexing failed"),
                }
                next_index = tokio::time::Instant::now() + Duration::from_secs(6 * 60 * 60);
            }
            if !inbox_state.workflow_running().await
                && let Err(error) = http::handlers::run_automatic_cycle(inbox_state.clone()).await
            {
                tracing::error!(%error, "inbox cycle failed");
                inbox_state
                    .finish_workflow(
                        types::WorkflowPhase::Failed,
                        "failed",
                        format!("Inbox check failed: {error}"),
                    )
                    .await;
            }
            tokio::select! {
                () = tokio::time::sleep(interval) => {}
                () = inbox_state.wait_for_automation_change() => {}
            }
        }
    });
    tokio::spawn(acquire::worker::run(state.clone()));
    let updater_state = state.clone();
    tokio::spawn(async move {
        // yt-dlp breaks whenever YouTube changes, so it is updated every day.
        loop {
            let config = updater_state.config.read().await.clone();
            if config.ytdlp_self_update {
                match acquire::ytdlp::self_update(&config).await {
                    Ok(message) => tracing::info!(%message, "yt-dlp update check"),
                    Err(error) => tracing::warn!(%error, "yt-dlp update failed"),
                }
            }
            tokio::time::sleep(Duration::from_secs(24 * 60 * 60)).await;
        }
    });
    let api = http::router().layer(middleware::from_fn(http::protect_local_api));
    let app = Router::new().nest("/api", api);
    let app = if let Some(directory) = &frontend {
        app.fallback_service(
            ServeDir::new(directory)
                .append_index_html_on_directories(true)
                .not_found_service(ServeFile::new(directory.join("index.html"))),
        )
    } else {
        tracing::warn!("frontend build not found; API is available but the bundled UI is disabled");
        app
    };
    let app = app
        .with_state(state.clone())
        .layer(DefaultBodyLimit::max(256 * 1024))
        .layer(TraceLayer::new_for_http())
        .layer(middleware::from_fn(http::security_headers));

    tracing::info!(%address, web = ?frontend, "magpie is ready");
    axum::serve(TcpListener::bind(address).await?, app)
        .with_graceful_shutdown(shutdown_signal(state.clone()))
        .await?;
    state.pool.close().await;
    Ok(())
}

fn listen_address() -> Result<SocketAddr> {
    let raw = std::env::var("MAGPIE_BIND").unwrap_or_else(|_| "127.0.0.1:7331".into());
    let allow_non_loopback = std::env::var("MAGPIE_ALLOW_NON_LOOPBACK").is_ok_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes"
        )
    });
    parse_listen_address(&raw, allow_non_loopback)
}

fn parse_listen_address(raw: &str, allow_non_loopback: bool) -> Result<SocketAddr> {
    let address: SocketAddr = raw
        .parse()
        .with_context(|| format!("invalid MAGPIE_BIND address: {raw}"))?;
    if !address.ip().is_loopback() && !allow_non_loopback {
        bail!(
            "MAGPIE_BIND must use a loopback address; containers may set \
             MAGPIE_ALLOW_NON_LOOPBACK=true only when the published host port remains loopback-only"
        )
    }
    Ok(address)
}

async fn purge_trash(config: &config::Config) {
    match application::library::purge_trash(&config.trash_dir, config.trash_days).await {
        Ok(0) => {}
        Ok(days) => tracing::info!(days, "emptied old trash days"),
        Err(error) => tracing::warn!(%error, "could not empty old trash"),
    }
}

/// Creates the inbox, library, trash and database folders so a fresh install
/// starts without manual steps.
fn prepare_folders(config: &config::Config) -> Result<()> {
    let database_folder = Path::new(&config.db_path)
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    for folder in [
        Some(Path::new(&config.input_dir)),
        Some(Path::new(&config.output_dir)),
        Some(config.trash_dir.as_path()),
        Some(config.staging_dir.as_path()),
        database_folder,
    ]
    .into_iter()
    .flatten()
    {
        std::fs::create_dir_all(folder)
            .with_context(|| format!("could not create folder {}", folder.display()))?;
    }
    Ok(())
}

/// Uses the real paths of the music folders (symlinks resolved), the same form
/// the inbox scan records for every file, so "is this file in staging?" is a
/// simple prefix check everywhere.
fn resolve_folders(config: &mut config::Config) -> Result<()> {
    let real = |path: &Path| {
        std::fs::canonicalize(path)
            .with_context(|| format!("cannot open folder {}", path.display()))
    };
    config.input_dir = real(Path::new(&config.input_dir))?
        .to_string_lossy()
        .into_owned();
    config.output_dir = real(Path::new(&config.output_dir))?
        .to_string_lossy()
        .into_owned();
    config.trash_dir = real(&config.trash_dir)?;
    config.staging_dir = real(&config.staging_dir)?;
    Ok(())
}

fn frontend_directory() -> Result<Option<PathBuf>> {
    if let Some(configured) = std::env::var_os("MAGPIE_WEB_DIR") {
        let directory = PathBuf::from(configured);
        if !directory.join("index.html").is_file() {
            bail!(
                "MAGPIE_WEB_DIR does not contain index.html: {}",
                directory.display()
            );
        }
        return Ok(Some(directory));
    }

    let mut candidates = Vec::new();
    if let Ok(executable) = std::env::current_exe()
        && let Some(bin_directory) = executable.parent()
    {
        candidates.push(bin_directory.join("../share/magpie"));
    }
    candidates.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("web/dist"));
    Ok(candidates
        .into_iter()
        .find(|directory| directory.join("index.html").is_file()))
}

async fn shutdown_signal(state: Arc<app::AppState>) {
    let interrupt = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::error!(%error, "failed to install Ctrl+C handler");
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(error) => tracing::error!(%error, "failed to install SIGTERM handler"),
        }
    };

    #[cfg(unix)]
    tokio::select! {
        () = interrupt => {},
        () = terminate => {},
    }
    #[cfg(not(unix))]
    interrupt.await;

    tracing::info!("shutdown requested; stopping active workflow safely");
    state.cancel_workflow().await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while state.workflow_running().await && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn duration_until_next_local_midnight() -> Duration {
    let now = chrono::Local::now();
    let Some(tomorrow) = now.date_naive().succ_opt() else {
        return Duration::from_secs(24 * 60 * 60);
    };
    let Some(midnight) = tomorrow.and_hms_opt(0, 0, 0) else {
        return Duration::from_secs(24 * 60 * 60);
    };
    let Some(next_midnight) = chrono::Local.from_local_datetime(&midnight).earliest() else {
        return Duration::from_secs(24 * 60 * 60);
    };
    (next_midnight - now)
        .to_std()
        .unwrap_or_else(|_| Duration::from_secs(24 * 60 * 60))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listen_address_accepts_loopback_by_default() {
        assert_eq!(
            parse_listen_address("127.0.0.1:7331", false).unwrap(),
            "127.0.0.1:7331".parse().unwrap()
        );
    }

    #[test]
    fn listen_address_rejects_non_loopback_without_explicit_opt_in() {
        let error = parse_listen_address("0.0.0.0:7331", false).unwrap_err();
        assert!(error.to_string().contains("MAGPIE_ALLOW_NON_LOOPBACK"));
    }

    #[test]
    fn listen_address_accepts_container_bind_with_explicit_opt_in() {
        assert_eq!(
            parse_listen_address("0.0.0.0:7331", true).unwrap(),
            "0.0.0.0:7331".parse().unwrap()
        );
    }
}
