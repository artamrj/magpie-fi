-- A song identified while a source was paused (rate limit, timeout) and left
-- unsure gets one more look when the source is back. 1: that look was
-- already asked for, so it is never asked a third time.
ALTER TABLE automatic_scan_files ADD COLUMN second_look INTEGER NOT NULL DEFAULT 0;
