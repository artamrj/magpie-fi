-- Telegram: the library card (a shared Bot API 10.4 widget) pinned in a chat, so
-- a newer one can replace its pin.
ALTER TABLE telegram_chats ADD COLUMN library_message_id INTEGER;
