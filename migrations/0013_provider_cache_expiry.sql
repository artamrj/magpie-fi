-- The daily "remove expired answers" pass looked at every cached row, cover
-- images included, to read its expiry. With this index it only touches the
-- expired ones.
CREATE INDEX IF NOT EXISTS provider_cache_expires ON provider_cache(expires_at);
