-- More of what Navidrome reads: the full release dates, sort names for Persian-named
-- artists, and the MusicBrainz release-group and release-track IDs.
ALTER TABLE candidates ADD COLUMN musicbrainz_release_group_id TEXT;
ALTER TABLE candidates ADD COLUMN musicbrainz_release_track_id TEXT;
ALTER TABLE candidates ADD COLUMN original_date TEXT;
ALTER TABLE candidates ADD COLUMN artist_sort TEXT;
ALTER TABLE candidates ADD COLUMN album_artist_sort TEXT;
