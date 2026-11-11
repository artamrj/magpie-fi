//! Requests, wanted songs and playlists in the database.

use super::links::{Collection, Scope};
use anyhow::Result;
use chrono::Utc;
use serde::Serialize;
use sqlx::SqlitePool;
use std::path::Path;

/// Waits before the 2nd and the 3rd (last) attempt.
const RETRY_DELAYS_SECONDS: [i64; 2] = [30, 120];
pub const MAX_ATTEMPTS: i64 = 3;

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct WantedRow {
    pub id: i64,
    pub request_id: i64,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub track_number: Option<i64>,
    pub duration: Option<f64>,
    pub isrc: Option<String>,
    pub source_url: Option<String>,
    pub youtube_id: Option<String>,
    pub attempts: i64,
}

const WANTED_FIELDS: &str =
    "id,request_id,title,artist,album,track_number,duration,isrc,source_url,youtube_id,attempts";

fn now() -> String {
    Utc::now().to_rfc3339()
}

pub async fn create_request(
    pool: &SqlitePool,
    url: &str,
    scope: Scope,
    origin: &str,
    requested_by: Option<&str>,
) -> Result<i64> {
    let now = now();
    Ok(sqlx::query(
        "INSERT INTO requests(url,kind,origin,requested_by,status,created_at,updated_at)
         VALUES(?,?,?,?,'reading',?,?)",
    )
    .bind(url)
    .bind(scope.as_str())
    .bind(origin)
    .bind(requested_by)
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await?
    .last_insert_rowid())
}

/// Stores the songs a link stands for. A playlist link also gets (or takes
/// over) its playlist record, so the same playlist always maps to one file.
pub async fn save_collection(
    pool: &SqlitePool,
    request_id: i64,
    url: &str,
    scope: Scope,
    collection: &Collection,
) -> Result<()> {
    let now = now();
    let mut transaction = pool.begin().await?;
    sqlx::query("UPDATE requests SET title=?,owner=?,status='ready',updated_at=? WHERE id=?")
        .bind(&collection.title)
        .bind(&collection.owner)
        .bind(&now)
        .bind(request_id)
        .execute(&mut *transaction)
        .await?;
    for (position, song) in collection.songs.iter().enumerate() {
        sqlx::query(
            "INSERT INTO wanted(request_id,position,title,artist,album,track_number,duration,isrc,
               source_url,youtube_id,status,updated_at)
             VALUES(?,?,?,?,?,?,?,?,?,?,'pending',?)",
        )
        .bind(request_id)
        .bind(position as i64 + 1)
        .bind(&song.title)
        .bind(&song.artist)
        .bind(&song.album)
        .bind(song.track_number)
        .bind(song.duration)
        .bind(&song.isrc)
        .bind(&song.source_url)
        .bind(&song.youtube_id)
        .bind(&now)
        .execute(&mut *transaction)
        .await?;
    }
    if scope == Scope::Playlist {
        let name = playlist_name(collection);
        sqlx::query(
            "INSERT INTO playlists(source_url,name,request_id,updated_at) VALUES(?,?,?,?)
             ON CONFLICT(source_url) DO UPDATE SET request_id=excluded.request_id,
               updated_at=excluded.updated_at",
        )
        .bind(url)
        .bind(name)
        .bind(request_id)
        .bind(&now)
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await?;
    Ok(())
}

/// "Owner - Title", or just the title when the owner is a service name.
fn playlist_name(collection: &Collection) -> String {
    let title = collection
        .title
        .clone()
        .unwrap_or_else(|| "Playlist".to_owned());
    match collection.owner.as_deref().map(str::trim) {
        Some(owner)
            if !owner.is_empty()
                && ![
                    "spotify",
                    "apple music",
                    "deezer",
                    "youtube",
                    "youtube music",
                ]
                .contains(&owner.to_ascii_lowercase().as_str()) =>
        {
            format!("{owner} - {title}")
        }
        _ => title,
    }
}

pub async fn request_failed(pool: &SqlitePool, request_id: i64, error: &str) -> Result<()> {
    sqlx::query("UPDATE requests SET status='failed',error=?,updated_at=? WHERE id=?")
        .bind(error)
        .bind(now())
        .bind(request_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Claims the next song that is due, marking it as downloading.
pub async fn claim_next(pool: &SqlitePool) -> Result<Option<WantedRow>> {
    let now = now();
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "UPDATE wanted SET status='downloading',updated_at=?
         WHERE id=(SELECT id FROM wanted WHERE status='pending'
                   AND (not_before IS NULL OR not_before<=?) ORDER BY id LIMIT 1)
         RETURNING {WANTED_FIELDS}"
    )))
    .bind(&now)
    .bind(&now)
    .fetch_optional(pool)
    .await?)
}

/// Songs interrupted by a restart start again. Called once at start, when
/// nothing is being read or downloaded yet.
///
/// A file that was being received from Telegram cannot be fetched again
/// without its message, so it is marked failed (it would otherwise be looked
/// up on YouTube by its name). A link that was still being read is marked
/// failed too: nothing would ever finish reading it, and sending the same
/// link again would only join the dead request.
pub async fn reset_interrupted(pool: &SqlitePool) -> Result<u64> {
    let now = now();
    sqlx::query(
        "UPDATE wanted SET status='failed',
           error='Receiving this file was interrupted by a restart. Send it again.',updated_at=?
         WHERE status='downloading'
           AND request_id IN (SELECT id FROM requests WHERE kind='upload')",
    )
    .bind(&now)
    .execute(pool)
    .await?;
    sqlx::query(
        "UPDATE requests SET status='failed',
           error='Reading this link was interrupted by a restart. Send it again.',updated_at=?
         WHERE status='reading'",
    )
    .bind(&now)
    .execute(pool)
    .await?;
    Ok(
        sqlx::query("UPDATE wanted SET status='pending' WHERE status='downloading'")
            .execute(pool)
            .await?
            .rows_affected(),
    )
}

/// When the next delayed retry is due, if any.
pub async fn next_due(pool: &SqlitePool) -> Result<Option<chrono::DateTime<Utc>>> {
    let value: Option<String> = sqlx::query_scalar(
        "SELECT min(not_before) FROM wanted WHERE status='pending' AND not_before IS NOT NULL",
    )
    .fetch_one(pool)
    .await?;
    Ok(value
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(&value).ok())
        .map(|value| value.with_timezone(&Utc)))
}

pub async fn mark_in_library(pool: &SqlitePool, id: i64, library_path: &str) -> Result<()> {
    sqlx::query(
        "UPDATE wanted SET status='in_library',library_path=?,error=NULL,updated_at=? WHERE id=?",
    )
    .bind(library_path)
    .bind(now())
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn mark_identifying(
    pool: &SqlitePool,
    id: i64,
    staging_path: &Path,
    youtube_id: Option<&str>,
) -> Result<()> {
    sqlx::query(
        "UPDATE wanted SET status='identifying',staging_path=?,
           youtube_id=coalesce(?,youtube_id),error=NULL,updated_at=?
         WHERE id=?",
    )
    .bind(staging_path.to_string_lossy().as_ref())
    .bind(youtube_id)
    .bind(now())
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// A failed attempt: try again later, or give up after the last attempt.
/// `retry` is false for problems that waiting cannot fix.
pub async fn mark_attempt_failed(
    pool: &SqlitePool,
    row: &WantedRow,
    error: &str,
    retry: bool,
) -> Result<bool> {
    let attempts = row.attempts + 1;
    let will_retry = retry && attempts < MAX_ATTEMPTS;
    let not_before = will_retry.then(|| {
        let delay = RETRY_DELAYS_SECONDS[usize::try_from(attempts - 1).unwrap_or(0).min(1)];
        (Utc::now() + chrono::Duration::seconds(delay)).to_rfc3339()
    });
    sqlx::query(
        "UPDATE wanted SET status=?,attempts=?,not_before=?,error=?,updated_at=? WHERE id=?",
    )
    .bind(if will_retry { "pending" } else { "failed" })
    .bind(attempts)
    .bind(not_before)
    .bind(error)
    .bind(now())
    .bind(row.id)
    .execute(pool)
    .await?;
    Ok(will_retry)
}

/// The download from `staging_path` is now in the library at `library_path`
/// (filed, or recognised as a song you already had). Returns the requests
/// whose playlists need rewriting.
pub async fn finished_download(
    pool: &SqlitePool,
    staging_path: &Path,
    library_path: &Path,
    status: &str,
) -> Result<Vec<i64>> {
    Ok(sqlx::query_scalar(
        "UPDATE wanted SET status=?,library_path=?,error=NULL,updated_at=?
         WHERE staging_path=? AND status='identifying' RETURNING request_id",
    )
    .bind(status)
    .bind(library_path.to_string_lossy().as_ref())
    .bind(now())
    .bind(staging_path.to_string_lossy().as_ref())
    .fetch_all(pool)
    .await?)
}

/// The link a downloaded file was asked for with, while it is being identified.
pub async fn requested_link(pool: &SqlitePool, staging_path: &Path) -> Result<Option<String>> {
    Ok(sqlx::query_scalar(
        "SELECT source_url FROM wanted
         WHERE staging_path=? AND status='identifying' AND source_url IS NOT NULL
         ORDER BY id DESC LIMIT 1",
    )
    .bind(staging_path.to_string_lossy().as_ref())
    .fetch_optional(pool)
    .await?)
}

/// A library file that came from this YouTube video or this source link before.
pub async fn known_source(
    pool: &SqlitePool,
    youtube_id: Option<&str>,
    source_url: Option<&str>,
) -> Result<Option<String>> {
    let paths: Vec<String> = sqlx::query_scalar(
        "SELECT library_path FROM wanted
         WHERE status IN ('filed','in_library') AND library_path IS NOT NULL
           AND ((? IS NOT NULL AND youtube_id=?) OR (? IS NOT NULL AND source_url=?))
         ORDER BY id DESC",
    )
    .bind(youtube_id)
    .bind(youtube_id)
    .bind(source_url)
    .bind(source_url)
    .fetch_all(pool)
    .await?;
    for path in paths {
        if tokio::fs::try_exists(&path).await.unwrap_or(false) {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

/// The latest request for the same link that is still being read or has songs, so sending
/// a link twice does not make two entries. A link that could not be read is tried afresh.
pub async fn find_reusable_request(pool: &SqlitePool, url: &str) -> Result<Option<i64>> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM requests WHERE url=? AND status IN ('reading','ready')
         ORDER BY id DESC LIMIT 1",
    )
    .bind(url)
    .fetch_optional(pool)
    .await?)
}

/// Marks a request as asked for again: it moves to the top of the list.
pub async fn touch_request(pool: &SqlitePool, request_id: i64) -> Result<()> {
    sqlx::query("UPDATE requests SET updated_at=? WHERE id=?")
        .bind(now())
        .bind(request_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Failed songs of a request go back into the queue.
pub async fn retry_failed(pool: &SqlitePool, request_id: i64) -> Result<u64> {
    Ok(sqlx::query(
        "UPDATE wanted SET status='pending',attempts=0,not_before=NULL,error=NULL,updated_at=?
         WHERE request_id=? AND status='failed'",
    )
    .bind(now())
    .bind(request_id)
    .execute(pool)
    .await?
    .rows_affected())
}

/// The same link was sent again: its failed songs go back into the queue, and so does a
/// link whose every song was removed in Review, because sending it again is asking for it
/// again. A removed song inside an album or playlist that still has other songs stays
/// removed, so sending the playlist again does not bring back what the owner threw out.
pub async fn ask_again(pool: &SqlitePool, request_id: i64) -> Result<u64> {
    Ok(sqlx::query(
        "UPDATE wanted SET status='pending',attempts=0,not_before=NULL,error=NULL,
           review_message_id=NULL,review_card=0,updated_at=?
         WHERE request_id=? AND (status='failed' OR (status='removed' AND NOT EXISTS
           (SELECT 1 FROM wanted other
             WHERE other.request_id=wanted.request_id AND other.status!='removed')))",
    )
    .bind(now())
    .bind(request_id)
    .execute(pool)
    .await?
    .rows_affected())
}

/// A library file moved (replaced by a better copy, or its artist renamed):
/// the songs that pointed at it follow. Returns the requests whose playlists
/// must be rewritten.
pub async fn library_file_moved(pool: &SqlitePool, old: &Path, new: &Path) -> Result<Vec<i64>> {
    let mut requests: Vec<i64> = sqlx::query_scalar(
        "UPDATE wanted SET library_path=?,updated_at=? WHERE library_path=? RETURNING request_id",
    )
    .bind(new.to_string_lossy().as_ref())
    .bind(Utc::now().to_rfc3339())
    .bind(old.to_string_lossy().as_ref())
    .fetch_all(pool)
    .await?;
    requests.sort_unstable();
    requests.dedup();
    Ok(requests)
}

/// The playlist file of a request, with its songs in order.
pub async fn playlist_for_request(
    pool: &SqlitePool,
    request_id: i64,
) -> Result<Option<(String, Vec<String>)>> {
    let name: Option<String> = sqlx::query_scalar("SELECT name FROM playlists WHERE request_id=?")
        .bind(request_id)
        .fetch_optional(pool)
        .await?;
    let Some(name) = name else {
        return Ok(None);
    };
    let paths: Vec<String> = sqlx::query_scalar(
        "SELECT library_path FROM wanted WHERE request_id=? AND library_path IS NOT NULL
         ORDER BY position",
    )
    .bind(request_id)
    .fetch_all(pool)
    .await?;
    Ok(Some((name, paths)))
}

// ── Telegram ────────────────────────────────────────────────────────────────

/// Remembers where a request came from in Telegram and its status message.
pub async fn attach_telegram(
    pool: &SqlitePool,
    request_id: i64,
    chat_id: i64,
    user_message_id: i32,
    status_message_id: i32,
) -> Result<()> {
    // A new status message starts from scratch, also for a request asked for again.
    sqlx::query(
        "UPDATE requests SET chat_id=?,user_message_id=?,status_message_id=?,status_text=NULL,closed=0
         WHERE id=?",
    )
        .bind(chat_id)
        .bind(user_message_id)
        .bind(status_message_id)
        .bind(request_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// A request for audio files sent to the bot.
pub async fn create_upload_request(pool: &SqlitePool, requested_by: &str) -> Result<i64> {
    let now = now();
    Ok(sqlx::query(
        "INSERT INTO requests(url,kind,origin,requested_by,status,created_at,updated_at)
         VALUES('','upload','telegram',?,'ready',?,?)",
    )
    .bind(requested_by)
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await?
    .last_insert_rowid())
}

/// One sent audio file, downloading from Telegram.
pub async fn add_upload(
    pool: &SqlitePool,
    request_id: i64,
    title: &str,
    artist: Option<&str>,
    duration: Option<f64>,
) -> Result<i64> {
    Ok(sqlx::query(
        "INSERT INTO wanted(request_id,position,title,artist,duration,status,updated_at)
         VALUES(?,(SELECT coalesce(max(position),0)+1 FROM wanted WHERE request_id=?),?,?,?,
                'downloading',?)",
    )
    .bind(request_id)
    .bind(request_id)
    .bind(title)
    .bind(artist)
    .bind(duration)
    .bind(now())
    .execute(pool)
    .await?
    .last_insert_rowid())
}

pub async fn mark_failed(pool: &SqlitePool, id: i64, error: &str) -> Result<()> {
    sqlx::query("UPDATE wanted SET status='failed',error=?,updated_at=? WHERE id=?")
        .bind(error)
        .bind(now())
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Songs of a request that have not started yet are dropped.
pub async fn stop_request(pool: &SqlitePool, request_id: i64) -> Result<u64> {
    Ok(sqlx::query(
        "UPDATE wanted SET status='failed',error='Stopped',updated_at=?
         WHERE request_id=? AND status='pending'",
    )
    .bind(now())
    .bind(request_id)
    .execute(pool)
    .await?
    .rows_affected())
}

/// A Telegram request whose status message is not final yet, with what the
/// message shows.
#[derive(Debug, sqlx::FromRow)]
pub struct OpenStatus {
    #[sqlx(flatten)]
    pub summary: RequestSummary,
    pub chat_id: i64,
    pub status_message_id: i64,
    pub user_message_id: Option<i64>,
    pub status_text: Option<String>,
    /// "text" or "card" (a live Bot API widget).
    pub card_kind: String,
    /// The card's caption as last sent.
    pub card_caption: Option<String>,
    /// "Artist – Title" of the first song.
    pub first_song: Option<String>,
    /// Songs filed now (not counting ones that were already in the library).
    pub newly_filed: i64,
}

/// Every open Telegram request with its progress, in one query.
pub async fn open_telegram_requests(pool: &SqlitePool) -> Result<Vec<OpenStatus>> {
    let sql = summary_sql(
        &format!(
            ",r.chat_id,r.status_message_id,r.user_message_id,r.status_text,
              r.card_kind,r.card_caption,{FIRST_SONG} AS first_song,
              coalesce(sum(w.status='filed'),0) AS newly_filed"
        ),
        "r.chat_id IS NOT NULL AND r.status_message_id IS NOT NULL AND r.closed=0",
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .fetch_all(pool)
        .await?)
}

pub async fn set_status_text(
    pool: &SqlitePool,
    request_id: i64,
    text: &str,
    closed: bool,
) -> Result<()> {
    sqlx::query("UPDATE requests SET status_text=?,closed=?,updated_at=? WHERE id=?")
        .bind(text)
        .bind(closed)
        .bind(now())
        .bind(request_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// The request's status message is a live card, last sent with this caption.
pub async fn set_card(pool: &SqlitePool, request_id: i64, caption: &str) -> Result<()> {
    sqlx::query("UPDATE requests SET card_kind='card',card_caption=? WHERE id=?")
        .bind(caption)
        .bind(request_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// The library file of the first song this request filed, if any. Filed songs
/// passed the cover gate, so their embedded cover is a verified one.
pub async fn first_filed_song(pool: &SqlitePool, request_id: i64) -> Result<Option<String>> {
    Ok(sqlx::query_scalar(
        "SELECT library_path FROM wanted
         WHERE request_id=? AND status='filed' AND library_path IS NOT NULL
         ORDER BY position LIMIT 1",
    )
    .bind(request_id)
    .fetch_optional(pool)
    .await?)
}

/// The library file of the request's first song that is in the library
/// (filed now, or already there).
pub async fn first_library_song(pool: &SqlitePool, request_id: i64) -> Result<Option<String>> {
    Ok(sqlx::query_scalar(
        "SELECT library_path FROM wanted
         WHERE request_id=? AND status IN ('filed','in_library') AND library_path IS NOT NULL
         ORDER BY position LIMIT 1",
    )
    .bind(request_id)
    .fetch_optional(pool)
    .await?)
}

/// May this chat get cards now? Not while an earlier refusal is fresh.
pub async fn cards_allowed(pool: &SqlitePool, chat_id: i64) -> Result<bool> {
    let until: Option<String> =
        sqlx::query_scalar("SELECT cards_off_until FROM telegram_chats WHERE chat_id=?")
            .bind(chat_id)
            .fetch_optional(pool)
            .await?
            .flatten();
    Ok(until.is_none_or(|until| until.as_str() <= now().as_str()))
}

/// Telegram refused a card in this chat: use text for a day.
pub async fn cards_refused(pool: &SqlitePool, chat_id: i64) -> Result<()> {
    let until = (chrono::Utc::now() + chrono::Duration::days(1)).to_rfc3339();
    sqlx::query(
        "INSERT INTO telegram_chats(chat_id,cards_off_until) VALUES(?,?)
         ON CONFLICT(chat_id) DO UPDATE SET cards_off_until=excluded.cards_off_until",
    )
    .bind(chat_id)
    .bind(until)
    .execute(pool)
    .await?;
    Ok(())
}

/// The library card pinned in this chat, if any.
pub async fn library_message(pool: &SqlitePool, chat_id: i64) -> Result<Option<i32>> {
    Ok(
        sqlx::query_scalar("SELECT library_message_id FROM telegram_chats WHERE chat_id=?")
            .bind(chat_id)
            .fetch_optional(pool)
            .await?
            .flatten(),
    )
}

pub async fn set_library_message(pool: &SqlitePool, chat_id: i64, message_id: i32) -> Result<()> {
    sqlx::query(
        "INSERT INTO telegram_chats(chat_id,library_message_id) VALUES(?,?)
         ON CONFLICT(chat_id) DO UPDATE SET library_message_id=excluded.library_message_id",
    )
    .bind(chat_id)
    .bind(message_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// A finished request gets a live status message again (retry pressed).
pub async fn reopen(pool: &SqlitePool, request_id: i64) -> Result<()> {
    sqlx::query("UPDATE requests SET closed=0 WHERE id=?")
        .bind(request_id)
        .execute(pool)
        .await?;
    Ok(())
}

#[derive(Debug, sqlx::FromRow)]
pub struct ReviewQuestion {
    pub wanted_id: i64,
    pub chat_id: i64,
    pub title: String,
    pub artist: Option<String>,
    pub track_id: i64,
    pub reason: Option<String>,
}

/// Songs sent from Telegram that wait in review and were not asked about yet.
pub async fn unasked_reviews(pool: &SqlitePool) -> Result<Vec<ReviewQuestion>> {
    Ok(sqlx::query_as(
        "SELECT w.id AS wanted_id,r.chat_id,w.title,w.artist,t.id AS track_id,
           t.stage_message AS reason
         FROM wanted w JOIN requests r ON r.id=w.request_id
         JOIN tracks t ON t.path=w.staging_path
         WHERE r.chat_id IS NOT NULL AND w.status='identifying' AND t.stage='review'
           AND w.review_message_id IS NULL",
    )
    .fetch_all(pool)
    .await?)
}

pub async fn set_review_message(pool: &SqlitePool, wanted_id: i64, message_id: i32) -> Result<()> {
    sqlx::query("UPDATE wanted SET review_message_id=? WHERE id=?")
        .bind(message_id)
        .bind(wanted_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Who asked for the song in this review track, and in which chat.
/// A review question sent as a card, still waiting for a choice.
pub async fn set_review_card(pool: &SqlitePool, wanted_id: i64, message_id: i32) -> Result<()> {
    sqlx::query("UPDATE wanted SET review_message_id=?,review_card=1 WHERE id=?")
        .bind(message_id)
        .bind(wanted_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Where a song's review card is: (wanted ID, chat, message, still waiting).
#[derive(Debug, sqlx::FromRow)]
pub struct ReviewCard {
    pub wanted_id: i64,
    pub chat_id: i64,
    pub message_id: i32,
    pub track_id: Option<i64>,
    /// The song's status now ("filed", "removed", "identifying", …).
    pub status: String,
    /// "Artist – Title", as the card showed it.
    pub song: String,
}

const REVIEW_CARD: &str = "SELECT w.id AS wanted_id,r.chat_id,w.review_message_id AS message_id,
       t.id AS track_id,w.status,
       CASE WHEN w.artist IS NULL THEN w.title ELSE w.artist||' – '||w.title END AS song
     FROM wanted w JOIN requests r ON r.id=w.request_id
     LEFT JOIN tracks t ON t.path=w.staging_path";

/// The open review card for a Review song, if it was asked about with one.
pub async fn review_card_of_track(pool: &SqlitePool, track_id: i64) -> Result<Option<ReviewCard>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{REVIEW_CARD} WHERE t.id=? AND w.review_card=1 AND r.chat_id IS NOT NULL
           AND w.review_message_id>0 LIMIT 1"
    )))
    .bind(track_id)
    .fetch_optional(pool)
    .await?)
}

/// Open review cards whose song left Review some other way (chosen or removed
/// in the app, or filed by itself), so the card can say so.
pub async fn settled_review_cards(pool: &SqlitePool) -> Result<Vec<ReviewCard>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{REVIEW_CARD} WHERE w.review_card=1 AND r.chat_id IS NOT NULL AND w.review_message_id>0
           AND (w.status<>'identifying' OR t.id IS NULL OR t.stage<>'review')"
    )))
    .fetch_all(pool)
    .await?)
}

/// The review card now shows its outcome and is not updated again.
pub async fn close_review_card(pool: &SqlitePool, wanted_id: i64) -> Result<()> {
    sqlx::query("UPDATE wanted SET review_card=2 WHERE id=?")
        .bind(wanted_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn requester_of_track(pool: &SqlitePool, track_id: i64) -> Result<Option<(i64, String)>> {
    Ok(sqlx::query_as(
        "SELECT r.id,coalesce(r.requested_by,'') FROM tracks t
         JOIN wanted w ON w.staging_path=t.path JOIN requests r ON r.id=w.request_id
         WHERE t.id=? LIMIT 1",
    )
    .bind(track_id)
    .fetch_optional(pool)
    .await?)
}

pub async fn requester_of_request(pool: &SqlitePool, request_id: i64) -> Result<Option<String>> {
    Ok(
        sqlx::query_scalar("SELECT requested_by FROM requests WHERE id=?")
            .bind(request_id)
            .fetch_optional(pool)
            .await?
            .flatten(),
    )
}

// ── What the web page shows ─────────────────────────────────────────────────

#[derive(Debug, Default, Serialize, sqlx::FromRow)]
pub struct RequestSummary {
    pub id: i64,
    pub url: String,
    pub kind: String,
    /// The request's own title, or "Artist – Title" of its first song.
    pub title: Option<String>,
    pub status: String,
    pub error: Option<String>,
    pub total: i64,
    /// Filed now or already in the library.
    pub done: i64,
    pub working: i64,
    pub review: i64,
    pub failed: i64,
    /// Removed by the owner in Review.
    pub removed: i64,
    pub created_at: String,
    pub updated_at: String,
    /// The Review track of the first song that waits for the owner, so the page can open it.
    pub track_id: Option<i64>,
    /// The first song's error, for a request of one song.
    pub first_error: Option<String>,
}

/// "Artist – Title" (or just the title) of a request's first song.
const FIRST_SONG: &str = "(SELECT CASE WHEN coalesce(f.artist,'')='' THEN f.title
       ELSE f.artist||' – '||f.title END
     FROM wanted f WHERE f.request_id=r.id ORDER BY f.position LIMIT 1)";

/// The [`RequestSummary`] query for the requests matching `filter`, with
/// `extra` columns after the summary's own.
fn summary_sql(extra: &str, filter: &str) -> String {
    format!(
        "SELECT r.id,r.url,r.kind,coalesce(r.title,{FIRST_SONG}) AS title,r.status,r.error,
           count(w.id) AS total,
           coalesce(sum(w.status IN ('filed','in_library')),0) AS done,
           coalesce(sum(w.status IN ('pending','downloading')
             OR (w.status='identifying' AND coalesce(t.stage,'') NOT IN ('review','failed'))),0) AS working,
           coalesce(sum(w.status='identifying' AND t.stage IN ('review','failed')),0) AS review,
           coalesce(sum(w.status='failed'),0) AS failed,
           coalesce(sum(w.status='removed'),0) AS removed,
           r.created_at,r.updated_at,
           (SELECT t2.id FROM wanted f JOIN tracks t2 ON t2.path=f.staging_path
             WHERE f.request_id=r.id AND f.status='identifying' AND t2.stage IN ('review','failed')
             ORDER BY f.position LIMIT 1) AS track_id,
           (SELECT f.error FROM wanted f WHERE f.request_id=r.id AND f.error IS NOT NULL
             ORDER BY f.position LIMIT 1) AS first_error
           {extra}
         FROM requests r
         LEFT JOIN wanted w ON w.request_id=r.id
         LEFT JOIN tracks t ON t.path=w.staging_path AND w.status='identifying'
         WHERE {filter}
         GROUP BY r.id ORDER BY r.updated_at DESC, r.id DESC"
    )
}

pub async fn list_requests(pool: &SqlitePool, limit: i64) -> Result<Vec<RequestSummary>> {
    let sql = summary_sql("", "1") + " LIMIT ?";
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(limit)
        .fetch_all(pool)
        .await?)
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct WantedView {
    pub position: i64,
    pub title: String,
    pub artist: Option<String>,
    pub status: String,
    pub error: Option<String>,
    /// The Review track, while the song waits for the owner.
    pub track_id: Option<i64>,
    pub updated_at: String,
}

pub async fn request_songs(pool: &SqlitePool, request_id: i64) -> Result<Vec<WantedView>> {
    Ok(sqlx::query_as(
        "SELECT w.position,w.title,w.artist,
           CASE WHEN w.status='identifying' AND t.stage='review' THEN 'review'
                WHEN w.status='identifying' AND t.stage='failed' THEN 'review'
                ELSE w.status END AS status,
           coalesce(w.error, CASE WHEN w.status='identifying' THEN t.stage_message END) AS error,
           CASE WHEN w.status='identifying' AND t.stage IN ('review','failed') THEN t.id END AS track_id,
           w.updated_at
         FROM wanted w LEFT JOIN tracks t ON t.path=w.staging_path
         WHERE w.request_id=? ORDER BY w.position",
    )
    .bind(request_id)
    .fetch_all(pool)
    .await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acquire::links::Wanted;

    async fn pool() -> (tempfile::TempDir, SqlitePool) {
        let directory = tempfile::tempdir().unwrap();
        let pool = crate::infrastructure::db::connect(
            directory.path().join("requests.sqlite").to_str().unwrap(),
        )
        .await
        .unwrap();
        (directory, pool)
    }

    fn collection(songs: &[&str]) -> Collection {
        Collection {
            title: Some("Road Trip".into()),
            owner: Some("Arta".into()),
            songs: songs
                .iter()
                .map(|title| Wanted {
                    title: (*title).into(),
                    artist: Some("Artist".into()),
                    ..Wanted::default()
                })
                .collect(),
        }
    }

    #[tokio::test]
    async fn a_restart_does_not_leave_links_or_uploads_stuck() {
        let (_directory, pool) = pool().await;
        // A link that was still being read.
        let reading = create_request(
            &pool,
            "https://example.test/album",
            Scope::Album,
            "web",
            None,
        )
        .await
        .unwrap();
        // A link with one song in the middle of its download.
        let ready = create_request(
            &pool,
            "https://example.test/song",
            Scope::Track,
            "web",
            None,
        )
        .await
        .unwrap();
        save_collection(
            &pool,
            ready,
            "https://example.test/song",
            Scope::Track,
            &collection(&["One"]),
        )
        .await
        .unwrap();
        let download = claim_next(&pool).await.unwrap().unwrap().id;
        // A file that was being received from Telegram.
        let upload_request = create_upload_request(&pool, "42").await.unwrap();
        let upload = add_upload(&pool, upload_request, "Sent file", None, None)
            .await
            .unwrap();

        assert_eq!(reset_interrupted(&pool).await.unwrap(), 1);

        let status = |table: &'static str, id: i64| {
            let pool = pool.clone();
            async move {
                sqlx::query_scalar::<_, String>(sqlx::AssertSqlSafe(format!(
                    "SELECT status FROM {table} WHERE id=?"
                )))
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap()
            }
        };
        assert_eq!(status("wanted", download).await, "pending");
        assert_eq!(status("wanted", upload).await, "failed");
        assert_eq!(status("requests", reading).await, "failed");
        assert_eq!(status("requests", ready).await, "ready");
        // Sending the link again starts a fresh request.
        assert_eq!(
            find_reusable_request(&pool, "https://example.test/album")
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn songs_are_claimed_once_and_retried_with_backoff() {
        let (_directory, pool) = pool().await;
        let request = create_request(&pool, "https://x/playlist/1", Scope::Playlist, "web", None)
            .await
            .unwrap();
        save_collection(
            &pool,
            request,
            "https://x/playlist/1",
            Scope::Playlist,
            &collection(&["A", "B"]),
        )
        .await
        .unwrap();

        let first = claim_next(&pool).await.unwrap().unwrap();
        let second = claim_next(&pool).await.unwrap().unwrap();
        assert_eq!((first.title.as_str(), second.title.as_str()), ("A", "B"));
        assert!(claim_next(&pool).await.unwrap().is_none());

        // A failure waits before the next attempt.
        assert!(
            mark_attempt_failed(&pool, &first, "network", true)
                .await
                .unwrap()
        );
        assert!(claim_next(&pool).await.unwrap().is_none());
        assert!(next_due(&pool).await.unwrap().is_some());
        // A problem that waiting cannot fix fails at once.
        assert!(
            !mark_attempt_failed(&pool, &second, "no match", false)
                .await
                .unwrap()
        );
        assert_eq!(retry_failed(&pool, request).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn filed_downloads_build_the_playlist_in_order() {
        let (directory, pool) = pool().await;
        let request = create_request(&pool, "https://x/playlist/1", Scope::Playlist, "web", None)
            .await
            .unwrap();
        save_collection(
            &pool,
            request,
            "https://x/playlist/1",
            Scope::Playlist,
            &collection(&["A", "B"]),
        )
        .await
        .unwrap();
        let a = claim_next(&pool).await.unwrap().unwrap();
        let b = claim_next(&pool).await.unwrap().unwrap();
        let staging = directory.path().join("w1.m4a");
        mark_identifying(&pool, a.id, &staging, Some("yt-a"))
            .await
            .unwrap();
        let library_b = directory.path().join("B.m4a");
        tokio::fs::write(&library_b, b"b").await.unwrap();
        mark_in_library(&pool, b.id, library_b.to_str().unwrap())
            .await
            .unwrap();

        // While it is being identified, the scan can ask which link it came from.
        assert_eq!(requested_link(&pool, &staging).await.unwrap(), None);
        sqlx::query("UPDATE wanted SET source_url='https://x/song/1' WHERE id=?")
            .bind(a.id)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            requested_link(&pool, &staging).await.unwrap().as_deref(),
            Some("https://x/song/1")
        );

        let library_a = directory.path().join("A.m4a");
        tokio::fs::write(&library_a, b"a").await.unwrap();
        let requests = finished_download(&pool, &staging, &library_a, "filed")
            .await
            .unwrap();
        assert_eq!(requests, vec![request]);

        let (name, paths) = playlist_for_request(&pool, request).await.unwrap().unwrap();
        assert_eq!(name, "Arta - Road Trip");
        assert_eq!(
            paths,
            vec![library_a.to_str().unwrap(), library_b.to_str().unwrap()]
        );
        // The same video is recognised next time.
        assert_eq!(
            known_source(&pool, Some("yt-a"), None)
                .await
                .unwrap()
                .as_deref(),
            library_a.to_str()
        );

        let summary = &list_requests(&pool, 10).await.unwrap()[0];
        assert_eq!((summary.total, summary.done), (2, 2));
        assert_eq!(summary.title.as_deref(), Some("Road Trip"));

        // A lossless copy replaces B: the playlist follows it.
        let lossless_b = directory.path().join("B.flac");
        assert_eq!(
            library_file_moved(&pool, &library_b, &lossless_b)
                .await
                .unwrap(),
            vec![request]
        );
        let (_, paths) = playlist_for_request(&pool, request).await.unwrap().unwrap();
        assert_eq!(paths[1], lossless_b.to_str().unwrap());
    }

    #[tokio::test]
    async fn untitled_requests_are_named_after_their_first_song() {
        let (_directory, pool) = pool().await;
        let request = create_request(&pool, "https://x/track/1", Scope::Track, "telegram", None)
            .await
            .unwrap();
        let mut songs = collection(&["One", "Two"]);
        songs.title = None;
        save_collection(&pool, request, "https://x/track/1", Scope::Track, &songs)
            .await
            .unwrap();
        attach_telegram(&pool, request, 1, 2, 3).await.unwrap();
        let finished = claim_next(&pool).await.unwrap().unwrap();
        mark_in_library(&pool, finished.id, "/music/one.m4a")
            .await
            .unwrap();

        let summary = &list_requests(&pool, 10).await.unwrap()[0];
        assert_eq!(summary.title.as_deref(), Some("Artist – One"));
        let open = &open_telegram_requests(&pool).await.unwrap()[0];
        assert_eq!(open.summary.id, request);
        assert_eq!(open.first_song.as_deref(), Some("Artist – One"));
        assert_eq!((open.summary.total, open.summary.done), (2, 1));
        assert_eq!(open.newly_filed, 0);
        assert_eq!(open.status_message_id, 3);
    }

    #[tokio::test]
    async fn a_link_sent_again_reuses_its_request_unless_reading_failed() {
        let (_directory, pool) = pool().await;
        let first = create_request(&pool, "https://x/track/1", Scope::Track, "web", None)
            .await
            .unwrap();
        assert_eq!(
            find_reusable_request(&pool, "https://x/track/1")
                .await
                .unwrap(),
            Some(first)
        );
        assert_eq!(
            find_reusable_request(&pool, "https://x/track/2")
                .await
                .unwrap(),
            None
        );
        request_failed(&pool, first, "unreadable").await.unwrap();
        assert_eq!(
            find_reusable_request(&pool, "https://x/track/1")
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn removed_songs_are_not_failed_and_are_not_retried() {
        let (_directory, pool) = pool().await;
        let request = create_request(&pool, "https://x/album/1", Scope::Album, "web", None)
            .await
            .unwrap();
        save_collection(
            &pool,
            request,
            "https://x/album/1",
            Scope::Album,
            &collection(&["A", "B"]),
        )
        .await
        .unwrap();
        sqlx::query("UPDATE wanted SET status='removed' WHERE title='A'")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE wanted SET status='failed',error='boom' WHERE title='B'")
            .execute(&pool)
            .await
            .unwrap();
        let summary = list_requests(&pool, 10).await.unwrap().remove(0);
        assert_eq!((summary.removed, summary.failed), (1, 1));
        assert_eq!(summary.first_error.as_deref(), Some("boom"));
        assert_eq!(retry_failed(&pool, request).await.unwrap(), 1);
        let statuses: Vec<String> =
            sqlx::query_scalar("SELECT status FROM wanted ORDER BY position")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(statuses, vec!["removed", "pending"]);
    }

    #[tokio::test]
    async fn a_removed_song_comes_back_when_its_link_is_sent_again() {
        let (_directory, pool) = pool().await;
        let single = create_request(&pool, "https://x/track/1", Scope::Track, "web", None)
            .await
            .unwrap();
        save_collection(
            &pool,
            single,
            "https://x/track/1",
            Scope::Track,
            &collection(&["A"]),
        )
        .await
        .unwrap();
        let album = create_request(&pool, "https://x/album/1", Scope::Album, "web", None)
            .await
            .unwrap();
        save_collection(
            &pool,
            album,
            "https://x/album/1",
            Scope::Album,
            &collection(&["B", "C"]),
        )
        .await
        .unwrap();
        sqlx::query(
            "UPDATE wanted SET status='removed',review_message_id=7 WHERE title IN ('A','B')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("UPDATE wanted SET status='failed',error='boom' WHERE title='C'")
            .execute(&pool)
            .await
            .unwrap();

        // The only song of the link was removed: asking again fetches it again, and the
        // bot may ask about it again.
        assert_eq!(ask_again(&pool, single).await.unwrap(), 1);
        let (status, asked): (String, Option<i64>) =
            sqlx::query_as("SELECT status,review_message_id FROM wanted WHERE title='A'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!((status.as_str(), asked), ("pending", None));

        // In an album with other songs, only the failed one is tried again.
        assert_eq!(ask_again(&pool, album).await.unwrap(), 1);
        let statuses: Vec<String> =
            sqlx::query_scalar("SELECT status FROM wanted WHERE request_id=? ORDER BY position")
                .bind(album)
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(statuses, vec!["removed", "pending"]);
    }

    #[tokio::test]
    async fn the_same_playlist_sent_again_takes_over_its_file() {
        let (_directory, pool) = pool().await;
        for _ in 0..2 {
            let request =
                create_request(&pool, "https://x/playlist/1", Scope::Playlist, "web", None)
                    .await
                    .unwrap();
            save_collection(
                &pool,
                request,
                "https://x/playlist/1",
                Scope::Playlist,
                &collection(&["A"]),
            )
            .await
            .unwrap();
        }
        let rows: Vec<i64> = sqlx::query_scalar("SELECT request_id FROM playlists")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(rows, vec![2]);
    }

    #[test]
    fn service_owners_are_left_out_of_playlist_names() {
        let mut value = collection(&[]);
        value.owner = Some("Spotify".into());
        assert_eq!(playlist_name(&value), "Road Trip");
    }
}
