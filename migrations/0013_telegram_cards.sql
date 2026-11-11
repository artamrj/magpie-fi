-- Telegram: a request's status message can be a live card (a Bot API 10.4 widget)
-- instead of text. The card's caption is what the chat list and older apps show.
ALTER TABLE requests ADD COLUMN card_kind TEXT NOT NULL DEFAULT 'text';
ALTER TABLE requests ADD COLUMN card_caption TEXT;

-- Chats whose Telegram refused a card get text until this time, then cards are
-- tried again.
CREATE TABLE IF NOT EXISTS telegram_chats (
  chat_id INTEGER PRIMARY KEY,
  cards_off_until TEXT
);
