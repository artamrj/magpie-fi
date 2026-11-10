-- One row per link someone asked for (a song, an album or a playlist).
CREATE TABLE IF NOT EXISTS requests (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  url TEXT NOT NULL,
  kind TEXT NOT NULL,            -- track | album | playlist
  title TEXT,
  owner TEXT,
  origin TEXT NOT NULL,          -- web | telegram
  requested_by TEXT,
  status TEXT NOT NULL,          -- reading | ready | failed
  error TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

-- One row per song a request wants, in the request's order.
CREATE TABLE IF NOT EXISTS wanted (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  request_id INTEGER NOT NULL REFERENCES requests(id) ON DELETE CASCADE,
  position INTEGER NOT NULL,
  title TEXT NOT NULL,
  artist TEXT,
  album TEXT,
  track_number INTEGER,
  duration REAL,
  isrc TEXT,
  source_url TEXT,
  youtube_id TEXT,
  -- pending → downloading → identifying → filed | in_library; or failed
  status TEXT NOT NULL,
  attempts INTEGER NOT NULL DEFAULT 0,
  not_before TEXT,
  error TEXT,
  staging_path TEXT,
  library_path TEXT,
  updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS wanted_request ON wanted(request_id, position);
CREATE INDEX IF NOT EXISTS wanted_status ON wanted(status, not_before);
CREATE INDEX IF NOT EXISTS wanted_staging ON wanted(staging_path);
CREATE INDEX IF NOT EXISTS wanted_youtube ON wanted(youtube_id);

-- A playlist link becomes a Navidrome playlist (an .m3u8 file in the library).
-- Sending the same playlist again updates the same file in the new order.
CREATE TABLE IF NOT EXISTS playlists (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  source_url TEXT NOT NULL UNIQUE,
  name TEXT NOT NULL,
  request_id INTEGER NOT NULL REFERENCES requests(id) ON DELETE CASCADE,
  updated_at TEXT NOT NULL
);
