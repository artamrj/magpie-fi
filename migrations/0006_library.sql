-- magpie-fi: settings now come only from the environment.
DROP TABLE IF EXISTS settings;

-- Every recording in the library, kept permanently (unlike `tracks`, which
-- only holds songs still on their way from the inbox). Used to recognise a
-- song that is already in the library, whoever sends it and however.
CREATE TABLE IF NOT EXISTS library_items (
  path TEXT PRIMARY KEY,
  file_size INTEGER NOT NULL,
  file_mtime_ns INTEGER NOT NULL,
  duration REAL,
  content_key TEXT,
  isrc TEXT,
  recording_id TEXT,
  title TEXT,
  artist TEXT,
  album TEXT,
  format TEXT NOT NULL DEFAULT '',
  bitrate INTEGER,
  source TEXT NOT NULL,
  added_at TEXT NOT NULL,
  checked_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS library_items_content_key ON library_items(content_key);
CREATE INDEX IF NOT EXISTS library_items_isrc ON library_items(isrc);
CREATE INDEX IF NOT EXISTS library_items_recording_id ON library_items(recording_id);
