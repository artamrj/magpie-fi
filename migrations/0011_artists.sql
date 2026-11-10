-- magpie-fi: one identity per artist. Every spelling seen for an artist points
-- to it, so all spellings share one name in the tags and one library folder.
CREATE TABLE IF NOT EXISTS artists (
  id INTEGER PRIMARY KEY,
  -- The name written to ARTISTS / ALBUMARTISTS and used for the folder.
  name TEXT NOT NULL,
  musicbrainz_id TEXT UNIQUE,
  -- 1: one artist even when the name has "&" or "," (a band or duo), known
  -- from MusicBrainz or the owner. 0: may be split when every part is a known artist.
  single_entity INTEGER NOT NULL DEFAULT 0,
  -- 'owner' names are never replaced automatically.
  name_source TEXT NOT NULL DEFAULT 'auto',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS artist_names (
  match_key TEXT PRIMARY KEY,
  artist_id INTEGER NOT NULL REFERENCES artists(id) ON DELETE CASCADE,
  spelling TEXT NOT NULL,
  -- Looser key for Latin transliterations; used only when it is unique.
  latin_key TEXT,
  source TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS artist_names_artist ON artist_names(artist_id);
CREATE INDEX IF NOT EXISTS artist_names_latin ON artist_names(latin_key);

-- The owner's "these are separate artists" for one credit text.
CREATE TABLE IF NOT EXISTS artist_splits (
  match_key TEXT PRIMARY KEY,
  spelling TEXT NOT NULL,
  parts_json TEXT NOT NULL,
  created_at TEXT NOT NULL
);

-- Duplicate checks by sound and by "same song, other spelling".
ALTER TABLE library_items ADD COLUMN fingerprint TEXT;
ALTER TABLE library_items ADD COLUMN title_key TEXT;
-- Artist identities of the song, as "|3|17|" (first = primary).
ALTER TABLE library_items ADD COLUMN artist_ids TEXT;
CREATE INDEX IF NOT EXISTS library_items_title_key ON library_items(title_key);
CREATE INDEX IF NOT EXISTS library_items_duration ON library_items(duration);

-- The owner said "file it anyway" for a possible duplicate.
ALTER TABLE tracks ADD COLUMN allow_duplicate INTEGER NOT NULL DEFAULT 0;
