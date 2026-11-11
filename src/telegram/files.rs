//! "📲 Send me the file": a filed song sent into the chat with `sendAudio`,
//! so it can be played or saved in Telegram. The file is only read, never
//! changed, and only from inside the library folder.

use super::{Context, api, send, texts, types::Message};
use crate::acquire::store;
use anyhow::{Context as _, Result};
use serde_json::json;
use std::path::{Path, PathBuf};

/// Sends the song a request put in the library. Problems (too big, gone) are
/// answered in the chat, so pressing the button always gives an answer.
pub async fn send_song(context: &Context, chat: i64, request: i64) {
    let reply_to = status_message(context, request, chat).await;
    let answer = match prepare(context, request).await {
        Ok(Prepared::Song(song)) => match upload(context, chat, reply_to, song).await {
            Ok(()) => return,
            Err(error) => {
                tracing::warn!(%error, request, "sending a song to Telegram failed");
                texts::FILE_FAILED.to_owned()
            }
        },
        Ok(Prepared::TooBig(bytes)) => texts::file_too_big(bytes),
        Ok(Prepared::Gone) => texts::FILE_GONE.to_owned(),
        Err(error) => {
            tracing::warn!(%error, request, "finding a song to send failed");
            texts::FILE_FAILED.to_owned()
        }
    };
    send(context, chat, &answer, reply_to).await;
}

/// The request's status message in this chat, to reply to.
async fn status_message(context: &Context, request: i64, chat: i64) -> Option<i32> {
    sqlx::query_scalar("SELECT status_message_id FROM requests WHERE id=? AND chat_id=?")
        .bind(request)
        .bind(chat)
        .fetch_optional(&context.state.pool)
        .await
        .ok()
        .flatten()
}

enum Prepared {
    Song(Song),
    /// Over Telegram's 50 MB limit for bots.
    TooBig(u64),
    /// Not in the library (any more).
    Gone,
}

struct Song {
    path: PathBuf,
    title: Option<String>,
    artist: Option<String>,
    seconds: Option<f64>,
}

async fn prepare(context: &Context, request: i64) -> Result<Prepared> {
    let pool = &context.state.pool;
    let Some(path) = store::first_library_song(pool, request).await? else {
        return Ok(Prepared::Gone);
    };
    let Some(path) = inside_library(&context.state.config.output_dir, Path::new(&path)).await
    else {
        return Ok(Prepared::Gone);
    };
    let size = tokio::fs::metadata(&path).await?.len();
    if size > api::UPLOAD_LIMIT {
        return Ok(Prepared::TooBig(size));
    }
    let details: Option<(Option<String>, Option<String>, Option<f64>)> =
        sqlx::query_as("SELECT title,artist,duration FROM library_items WHERE path=?")
            .bind(path.to_string_lossy().as_ref())
            .fetch_optional(pool)
            .await?;
    let (title, artist, seconds) = details.unwrap_or_default();
    Ok(Prepared::Song(Song {
        path,
        title,
        artist,
        seconds,
    }))
}

/// The file, if it really is inside the library folder (after following
/// links), so the button can never send anything else.
async fn inside_library(library: &str, path: &Path) -> Option<PathBuf> {
    let library = tokio::fs::canonicalize(library).await.ok()?;
    let file = tokio::fs::canonicalize(path).await.ok()?;
    (file.starts_with(&library) && file.is_file()).then_some(file)
}

async fn upload(context: &Context, chat: i64, reply_to: Option<i32>, song: Song) -> Result<()> {
    let bytes = tokio::fs::read(&song.path)
        .await
        .context("could not read the song")?;
    let file_name = song.path.file_name().map_or_else(
        || "song".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    let mut params = json!({ "chat_id": chat });
    if let Some(title) = &song.title {
        params["title"] = json!(title);
    }
    if let Some(artist) = &song.artist {
        params["performer"] = json!(artist);
    }
    if let Some(seconds) = song.seconds {
        params["duration"] = json!(seconds.round() as i64);
    }
    if let Some(reply_to) = reply_to {
        params["reply_parameters"] =
            json!({"message_id": reply_to, "allow_sending_without_reply": true});
    }
    let mut files = vec![api::Upload {
        field: "audio",
        file_name,
        bytes,
    }];
    let path = song.path.clone();
    if let Ok(Some(thumbnail)) = tokio::task::spawn_blocking(move || thumbnail(&path)).await {
        params["thumbnail"] = json!("attach://thumb");
        files.push(api::Upload {
            field: "thumb",
            file_name: "cover.jpg".into(),
            bytes: thumbnail,
        });
    }
    let _: Message = context.bot.upload("sendAudio", &params, &files).await?;
    Ok(())
}

/// The song's embedded cover as Telegram wants a thumbnail: a JPEG at most
/// 320 px wide and 200 kB. Covers in the library passed the cover gate.
fn thumbnail(path: &Path) -> Option<Vec<u8>> {
    let cover = crate::infrastructure::media::tag_writer::read_artwork(path)
        .ok()
        .flatten()?;
    let image = image::load_from_memory(&cover)
        .ok()?
        .thumbnail(320, 320)
        .to_rgb8();
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 85)
        .encode_image(&image)
        .ok()?;
    (jpeg.len() <= 200 * 1024).then_some(jpeg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn only_files_inside_the_library_can_be_sent() {
        let directory = tempfile::tempdir().unwrap();
        let library = directory.path().join("library");
        tokio::fs::create_dir_all(library.join("Daft Punk"))
            .await
            .unwrap();
        let song = library.join("Daft Punk/One More Time.m4a");
        tokio::fs::write(&song, b"a").await.unwrap();
        let outside = directory.path().join("secret.txt");
        tokio::fs::write(&outside, b"a").await.unwrap();
        let library = library.to_str().unwrap();

        assert!(inside_library(library, &song).await.is_some());
        assert!(inside_library(library, &outside).await.is_none());
        // ".." can't climb out of the library.
        let sneaky = Path::new(library).join("../secret.txt");
        assert!(inside_library(library, &sneaky).await.is_none());
        assert!(
            inside_library(library, &Path::new(library).join("Daft Punk"))
                .await
                .is_none()
        );
        assert!(
            inside_library(library, Path::new("/nowhere/at/all.mp3"))
                .await
                .is_none()
        );
    }
}
