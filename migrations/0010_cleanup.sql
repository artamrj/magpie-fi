-- Nothing read these: candidate evidence lives in candidates.score_breakdown,
-- and filed tracks are deleted right after filing.
DROP TABLE IF EXISTS candidate_sources;
ALTER TABLE tracks DROP COLUMN last_apply_run_id;
ALTER TABLE tracks DROP COLUMN last_applied_at;
ALTER TABLE tracks DROP COLUMN retry_count;
ALTER TABLE tracks DROP COLUMN next_retry_at;

-- ReplayGain is measured by rsgain per album folder; the per-file cache is gone.
DROP TABLE IF EXISTS replaygain_cache;
