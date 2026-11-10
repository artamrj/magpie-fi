-- Telegram: where a request's status message lives, and whether it is final.
ALTER TABLE requests ADD COLUMN chat_id INTEGER;
ALTER TABLE requests ADD COLUMN status_message_id INTEGER;
ALTER TABLE requests ADD COLUMN user_message_id INTEGER;
ALTER TABLE requests ADD COLUMN status_text TEXT;
ALTER TABLE requests ADD COLUMN closed INTEGER NOT NULL DEFAULT 0;

-- The "which song is this?" question sent for a song in review.
ALTER TABLE wanted ADD COLUMN review_message_id INTEGER;

-- People an owner invited. Owners themselves come from MAGPIE_TELEGRAM_OWNERS.
CREATE TABLE IF NOT EXISTS telegram_guests (
  user_id INTEGER PRIMARY KEY,
  name TEXT,
  invited_by INTEGER NOT NULL,
  created_at TEXT NOT NULL
);

-- One-time invite links (t.me/<bot>?start=invite_<token>).
CREATE TABLE IF NOT EXISTS telegram_invites (
  token TEXT PRIMARY KEY,
  created_by INTEGER NOT NULL,
  created_at TEXT NOT NULL
);

-- Small values the bot keeps between restarts (the update offset).
CREATE TABLE IF NOT EXISTS telegram_state (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
