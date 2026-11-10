use super::*;

/// Read-only view of the configuration. Settings come from `MAGPIE_*`
/// environment variables; the web page shows them but cannot change them.

pub async fn setup(State(s): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let cfg = s.config.read().await;
    let fpcalc = std::process::Command::new("fpcalc")
        .arg("-version")
        .output()
        .is_ok();
    let ffmpeg = crate::infrastructure::media::replaygain::available();
    let songrec = crate::infrastructure::providers::songrec::available();
    let navidrome = match cfg.navidrome.as_ref() {
        None => serde_json::json!({"configured": false}),
        Some(navidrome) => serde_json::json!({
            "configured": true,
            "url": navidrome.url,
            "reachable": crate::infrastructure::navidrome::ping(&s.client, navidrome).await.is_ok(),
        }),
    };
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
            "large_files": !telegram.api_url.contains("api.telegram.org"),
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
        "navidrome": navidrome,
        "telegram": telegram,
        "library_songs": library_songs,
        "rsgain": crate::infrastructure::media::rsgain::available(),
        "ytdlp": crate::acquire::ytdlp::available(&cfg),
        "audio_format": match cfg.audio_format {
            crate::config::AudioFormat::M4a => "m4a",
            crate::config::AudioFormat::Opus => "opus",
        },
        "max_parallel_downloads": cfg.max_parallel_downloads,
        "sources": {
            "musicbrainz": true,
            "navahang": true,
            "audiomack": true,
            "itunes": true,
            "deezer": true,
            "radiojavan": true,
            "wikidata": true,
            "cover_art_archive": true,
            "fpcalc": fpcalc,
            "ffmpeg": ffmpeg,
            "songrec": songrec,
            "shazam": true,
            "integrity_check": crate::infrastructure::media::integrity::available(),
            "acoustid": !cfg.acoustid_key.is_empty(),
            "audd": !cfg.audd_token.is_empty(),
            "spotify": !cfg.spotify_client_id.is_empty() && !cfg.spotify_client_secret.is_empty(),
            "soundcloud": true,
            "soundcloud_search": !cfg.soundcloud_client_id.is_empty() && !cfg.soundcloud_client_secret.is_empty(),
            "youtube": !cfg.youtube_api_key.is_empty(),
            "discogs": !cfg.discogs_token.is_empty(),
            "lastfm": !cfg.lastfm_key.is_empty(),
            "genius": true,
            "theaudiodb": !cfg.theaudiodb_key.is_empty()
        }
    }))
}
