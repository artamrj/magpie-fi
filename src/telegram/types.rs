//! The Bot API types magpie-fi reads, written by hand (Bot API 10.4).
//!
//! Only the fields the bot uses are here. Serde ignores every other field, so
//! a new Bot API version never breaks reading updates. IDs are `i64` because
//! Telegram's user and chat IDs fit in 52 bits; message IDs are `i32`, as in
//! the database.

use serde::Deserialize;

/// Every Bot API answer: `{"ok": true, "result": …}` or an error.
#[derive(Debug, Deserialize)]
pub struct Response<R> {
    pub ok: bool,
    pub result: Option<R>,
    #[serde(default)]
    pub error_code: u64,
    #[serde(default)]
    pub description: String,
    pub parameters: Option<ResponseParameters>,
}

#[derive(Debug, Deserialize)]
pub struct ResponseParameters {
    /// Seconds to wait before trying again (flood control).
    pub retry_after: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct Update {
    pub update_id: i64,
    pub message: Option<Message>,
    pub callback_query: Option<CallbackQuery>,
}

#[derive(Debug, Deserialize)]
pub struct User {
    pub id: i64,
    #[serde(default)]
    pub first_name: String,
    pub username: Option<String>,
}

impl User {
    /// "@name", or the first name for people without a username.
    pub fn display_name(&self) -> String {
        self.username
            .as_ref()
            .map_or_else(|| self.first_name.clone(), |name| format!("@{name}"))
    }
}

#[derive(Debug, Deserialize)]
pub struct Chat {
    pub id: i64,
    /// "private", "group", "supergroup" or "channel".
    #[serde(rename = "type", default)]
    pub kind: String,
}

impl Chat {
    pub fn is_private(&self) -> bool {
        self.kind == "private"
    }
}

/// A message. Also used for the message under a pressed button: when that one
/// is too old to read, Telegram only sends its chat and ID, which is all the
/// bot needs from it.
#[derive(Debug, Deserialize)]
pub struct Message {
    pub message_id: i32,
    pub chat: Chat,
    pub from: Option<User>,
    pub text: Option<String>,
    pub caption: Option<String>,
    pub audio: Option<Audio>,
    pub document: Option<Document>,
}

#[derive(Debug, Deserialize)]
pub struct Audio {
    pub file_id: String,
    #[serde(default)]
    pub duration: u32,
    pub performer: Option<String>,
    pub title: Option<String>,
    pub file_name: Option<String>,
    pub file_size: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct Document {
    pub file_id: String,
    pub file_name: Option<String>,
    pub mime_type: Option<String>,
    pub file_size: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct CallbackQuery {
    pub id: String,
    pub from: User,
    pub message: Option<Message>,
    pub data: Option<String>,
}

/// The answer to `getFile`.
#[derive(Debug, Deserialize)]
pub struct File {
    pub file_path: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_fields_and_old_button_messages_are_fine() {
        // A newer API version adds fields; a pressed button's message is too old
        // to read (date 0, only chat and ID).
        let update: Update = serde_json::from_value(serde_json::json!({
            "update_id": 9,
            "brand_new_field": {"anything": true},
            "callback_query": {
                "id": "q", "chat_instance": "c", "data": "stop:1",
                "from": {"id": 5, "is_bot": false, "first_name": "Ana", "is_premium": true},
                "message": {"message_id": 7, "date": 0, "chat": {"id": 5, "type": "private"}}
            }
        }))
        .unwrap();
        let query = update.callback_query.unwrap();
        assert_eq!(query.from.display_name(), "Ana");
        let message = query.message.unwrap();
        assert_eq!((message.chat.id, message.message_id), (5, 7));
        assert!(message.chat.is_private());
    }

    #[test]
    fn errors_carry_their_wait_time() {
        let error: Response<bool> = serde_json::from_value(serde_json::json!({
            "ok": false, "error_code": 429, "description": "Too Many Requests",
            "parameters": {"retry_after": 3}
        }))
        .unwrap();
        assert!(!error.ok && error.result.is_none());
        assert_eq!(error.parameters.unwrap().retry_after, Some(3));
    }
}
