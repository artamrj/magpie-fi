use super::*;

/// Read-only view of the configuration. Settings come from `MAGPIE_*`
/// environment variables; the web page shows them but cannot change them.
pub async fn setup(State(s): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let cfg = &s.config;
    // The tool checks start processes, so they run off the async runtime.
    let ytdlp_config = cfg.clone();
    let (fpcalc, ffmpeg, songrec, rsgain, ytdlp) = tokio::task::spawn_blocking(move || {
        (
            crate::infrastructure::process::available("fpcalc", "-version"),
            crate::infrastructure::media::integrity::available(),
            crate::infrastructure::providers::songrec::available(),
            crate::infrastructure::media::rsgain::available(),
            crate::acquire::ytdlp::available(&ytdlp_config),
        )
    })
    .await
    .unwrap_or_default();
    let guests: i64 = sqlx::query_scalar("SELECT count(*) FROM telegram_guests")
        .fetch_one(&s.pool)
        .await
        .unwrap_or_default();
    let telegram = match cfg.telegram.as_ref() {
        None => serde_json::json!({"configured": false}),
        Some(telegram) => serde_json::json!({
            "configured": true,
            "owners": telegram.owners.len(),
            "guests": guests,
        }),
    };
    let library_songs: i64 = sqlx::query_scalar("SELECT count(*) FROM library_items")
        .fetch_one(&s.pool)
        .await
        .unwrap_or_default();
    Json(serde_json::json!({
        "inbox_dir": cfg.input_dir,
        "library_dir": cfg.output_dir,
        "trash_dir": cfg.trash_dir,
        "trash_days": cfg.trash_days,
        "inbox_check_seconds": cfg.inbox_check_seconds,
        "lyrics": cfg.lyrics,
        "sidecars": cfg.sidecars,
        "telegram": telegram,
        "library_songs": library_songs,
        "rsgain": rsgain,
        "ytdlp": ytdlp,
        "audio_format": cfg.audio_format.as_str(),
        "max_parallel_downloads": cfg.max_parallel_downloads,
        "sources": {
            "musicbrainz": true,
            "navahang": true,
            "audiomack": true,
            "itunes": true,
            "deezer": true,
            "radiojavan": true,
            "cover_art_archive": true,
            "fpcalc": fpcalc,
            "ffmpeg": ffmpeg,
            "songrec": songrec,
            "shazam": true,
            "integrity_check": ffmpeg,
            "acoustid": !cfg.acoustid_key.is_empty(),
            "soundcloud": true,
            "soundcloud_search": !cfg.soundcloud_client_id.is_empty() && !cfg.soundcloud_client_secret.is_empty(),
            "youtube": !cfg.youtube_api_key.is_empty(),
            "discogs": !cfg.discogs_token.is_empty(),
            "lastfm": !cfg.lastfm_key.is_empty(),
            "genius": true
        }
    }))
}
