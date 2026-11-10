//! What people send: links become requests, audio files become uploads.
//! Both go into the same queue the web page uses.

use super::{Context, api, react, send, texts, types::Message, types::User};
use crate::acquire::{links, store, worker};
use anyhow::Result;
use std::{
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

/// Audio files sent within this time join the same request (a forwarded album).
pub const UPLOAD_BATCH_WINDOW: Duration = Duration::from_secs(3);

const AUDIO_EXTENSIONS: [&str; 10] = [
    "aac", "aiff", "aif", "alac", "flac", "m4a", "mp3", "ogg", "opus", "wav",
];

/// A message with links: one request per link.
pub async fn on_links(
    context: &Arc<Context>,
    message: &Message,
    user: &User,
    urls: Vec<String>,
) -> Result<()> {
    let chat = message.chat.id;
    for url in urls {
        let expanded = links::expand_short_link(&context.state.client, &url)
            .await
            .unwrap_or(url.clone());
        let Some(link) = links::classify(&expanded) else {
            send(
                context,
                chat,
                &texts::cant_read_link(&url),
                Some(message.message_id),
            )
            .await;
            continue;
        };
        // The same link again: its failed songs are tried again under a new status message.
        if let Some(request) = store::find_reusable_request(&context.state.pool, &link.url).await? {
            let queued = store::retry_failed(&context.state.pool, request).await?;
            store::touch_request(&context.state.pool, request).await?;
            if queued > 0 {
                context.state.notify_acquire();
            }
            attach_status_message(context, request, message).await?;
            continue;
        }
        let request = store::create_request(
            &context.state.pool,
            &link.url,
            link.scope,
            "telegram",
            Some(&user.id.to_string()),
        )
        .await?;
        attach_status_message(context, request, message).await?;
        tokio::spawn(worker::read_request(context.state.clone(), request, link));
    }
    react(context, chat, message.message_id, "👀").await;
    Ok(())
}

/// Sends the first version of a request's status message and remembers it.
async fn attach_status_message(context: &Context, request: i64, message: &Message) -> Result<()> {
    let sent = send(
        context,
        message.chat.id,
        texts::READING_LINK,
        Some(message.message_id),
    )
    .await;
    if let Some(sent) = sent {
        store::attach_telegram(
            &context.state.pool,
            request,
            message.chat.id,
            message.message_id,
            sent.message_id,
        )
        .await?;
    }
    Ok(())
}

/// An audio file in a message, as an audio message or as a document.
pub struct AudioFile {
    id: String,
    name: String,
    title: Option<String>,
    artist: Option<String>,
    duration: Option<f64>,
    size: Option<u64>,
}

pub fn audio_file(message: &Message) -> Option<AudioFile> {
    if let Some(audio) = &message.audio {
        return Some(AudioFile {
            id: audio.file_id.clone(),
            name: audio
                .file_name
                .clone()
                .unwrap_or_else(|| "audio.mp3".into()),
            title: audio.title.clone(),
            artist: audio.performer.clone(),
            duration: (audio.duration > 0).then_some(f64::from(audio.duration)),
            size: audio.file_size,
        });
    }
    let document = message.document.as_ref()?;
    let name = document.file_name.clone().unwrap_or_default();
    let looks_like_audio = document
        .mime_type
        .as_deref()
        .is_some_and(|mime| mime.starts_with("audio/"))
        || audio_extension(&name).is_some();
    looks_like_audio.then(|| AudioFile {
        id: document.file_id.clone(),
        name,
        title: None,
        artist: None,
        duration: None,
        size: document.file_size,
    })
}

/// The file's extension when it is one of the audio formats magpie-fi takes.
fn audio_extension(name: &str) -> Option<String> {
    Path::new(name)
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .filter(|extension| AUDIO_EXTENSIONS.contains(&extension.as_str()))
}

pub async fn on_upload(
    context: &Arc<Context>,
    message: &Message,
    user: &User,
    file: AudioFile,
) -> Result<()> {
    let chat = message.chat.id;
    if let Some(size) = file.size.filter(|size| *size > api::DOWNLOAD_LIMIT) {
        send(
            context,
            chat,
            &texts::too_big(&file.name, size),
            Some(message.message_id),
        )
        .await;
        return Ok(());
    }
    // Files forwarded together join one request.
    let key = (chat, user.id);
    let request = {
        let mut uploads = context.uploads.lock().await;
        uploads.retain(|_, (_, last)| last.elapsed() < UPLOAD_BATCH_WINDOW);
        match uploads.get(&key) {
            Some((request, _)) => {
                let request = *request;
                uploads.insert(key, (request, Instant::now()));
                request
            }
            _ => {
                let request =
                    store::create_upload_request(&context.state.pool, &user.id.to_string()).await?;
                uploads.insert(key, (request, Instant::now()));
                attach_status_message(context, request, message).await?;
                react(context, chat, message.message_id, "👀").await;
                request
            }
        }
    };
    let title = file.title.clone().unwrap_or_else(|| {
        Path::new(&file.name)
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| "audio".into())
    });
    let wanted = store::add_upload(
        &context.state.pool,
        request,
        &title,
        file.artist.as_deref(),
        file.duration,
    )
    .await?;
    tokio::spawn(download(context.clone(), file, title, wanted));
    Ok(())
}

/// Saves an uploaded file into staging, then hands it to identification.
async fn download(context: Arc<Context>, file: AudioFile, title: String, wanted: i64) {
    let config = &context.state.config;
    let extension = audio_extension(&file.name).unwrap_or_else(|| "mp3".into());
    // "Artist - Title (u6).mp3": readable in Review, and a hint for identification.
    let stem = match file
        .artist
        .as_deref()
        .filter(|artist| !artist.trim().is_empty())
    {
        Some(artist) => format!("{artist} - {title}"),
        None => title.clone(),
    };
    let stem = crate::application::library::safe_name(&stem, "audio");
    let target = config
        .staging_dir
        .join(format!("{stem} (u{wanted}).{extension}"));
    let part = config
        .staging_dir
        .join(".part")
        .join(format!("u{wanted}.{extension}"));
    let result = async {
        tokio::fs::create_dir_all(part.parent().unwrap_or(&config.staging_dir)).await?;
        context.bot.download(&file.id, &part).await?;
        // Files forwarded from channels often have no tags or a channel's
        // name as artist; Telegram's title and performer are the better start.
        let (tagged, hint_title, hint_artist) =
            (part.clone(), file.title.clone(), file.artist.clone());
        if let Err(error) = tokio::task::spawn_blocking(move || {
            crate::infrastructure::media::tag_writer::apply_telegram_details(
                &tagged,
                hint_title.as_deref(),
                hint_artist.as_deref(),
            )
        })
        .await
        .map_err(anyhow::Error::from)
        .and_then(|result| result)
        {
            tracing::warn!(%error, "could not write Telegram's title and artist into the file");
        }
        tokio::fs::rename(&part, &target).await?;
        store::mark_identifying(&context.state.pool, wanted, &target, None).await
    }
    .await;
    match result {
        Ok(()) => context.state.notify_automation_scheduler(),
        Err(error) => {
            // Only magpie-fi's own partial download is removed (staging/.part).
            let _ = tokio::fs::remove_file(&part).await;
            let _ = store::mark_failed(&context.state.pool, wanted, &format!("{error:#}")).await;
        }
    }
}

/// `http(s)://…` links in a message, without trailing punctuation, in order.
pub fn find_urls(text: &str) -> Vec<String> {
    let mut urls: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        let Some(start) = word.find("http://").or_else(|| word.find("https://")) else {
            continue;
        };
        let url = word[start..]
            .trim_end_matches(['.', ',', ';', ')', ']', '>', '!', '?', '"', '\''])
            .to_owned();
        if !urls.contains(&url) {
            urls.push(url);
        }
    }
    urls
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_are_found_in_order_without_punctuation() {
        let text = "listen (https://open.spotify.com/track/1), and https://youtu.be/x! also https://open.spotify.com/track/1";
        assert_eq!(
            find_urls(text),
            vec!["https://open.spotify.com/track/1", "https://youtu.be/x"]
        );
        assert!(find_urls("no links here").is_empty());
    }

    #[test]
    fn audio_documents_are_known_by_type_or_extension() {
        let document = |name: &str, mime: Option<&str>| -> Message {
            serde_json::from_value(serde_json::json!({
                "message_id": 1, "chat": {"id": 1, "type": "private"},
                "document": {"file_id": "f", "file_name": name, "mime_type": mime}
            }))
            .unwrap()
        };
        assert!(audio_file(&document("Song.FLAC", None)).is_some());
        assert!(audio_file(&document("voice", Some("audio/ogg"))).is_some());
        assert!(audio_file(&document("notes.pdf", Some("application/pdf"))).is_none());
        assert_eq!(audio_extension("a.Opus").as_deref(), Some("opus"));
        assert_eq!(audio_extension("a.exe"), None);
    }
}
