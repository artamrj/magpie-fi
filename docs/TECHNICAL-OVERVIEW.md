# magpie — Technical Overview & End-to-End Algorithm

magpie is the program at the heart of magpie-fi. It watches an inbox folder, reads
every new audio file, verifies it can be decoded, identifies it through a cascade of
online metadata providers, lets the user approve or correct uncertain matches, and
files each song into the library (with ReplayGain, verified cover art and lyrics),
moving the original to a trash folder. The identification engine comes from the
earlier *ununknown* project.

This document describes how the whole system works: the components, the data model,
and the exact algorithm the pipeline runs from first launch to final corrected file.

---

## 1. High-level architecture

```
┌──────────────────────────────────────────────────────────────────────┐
│                         Browser (React SPA)                          │
│Setup / Scan button / Review queue / Inspector / Write dock           │
└──────────────────────────────────▲───────────────────────────────────┘
                                   │  local HTTP  (127.0.0.1:7331)
┌──────────────────────────────────┴───────────────────────────────────┐
│                          Rust server (axum)                          │
│HTTP router  →  handlers  →  application services                     │
│                                                                      │
│  ┌─────────────────────────────────────────────────────────────┐     │
│  │ scan (run · process · providers · persist)                  │     │
│  │ input_dedup · smart_approval · metadata_completion · apply  │     │
│  └──────────────────────────────┬──────────────────────────────┘     │
│                                  │                                   │
│  ┌──────────────────────────────┴───────────────────────────────┐    │
│  │  Infrastructure layer                                        │    │
│  │  media: audio reader · integrity · fingerprint · replaygain ·│    │
│  │         tag_writer · repair                                  │    │
│  │  providers: 20+ metadata/audio sources (see §7)              │    │
│  │  db: SQLite + repository (tracks/candidates) + caches        │    │
│  └──────────────────────────────────────────────────────────────┘    │
└──────────────────────────────────────────────────────────────────────┘
```

- **Web page** (`web/`): a React SPA served by the same Rust process. It polls
  `/api/status` and `/api/tracks` and renders the review workspace.
- **Backend** (`src/`): Rust + axum + tokio + SQLite (`sqlx`). All heavy work
  (audio decoding, fingerprinting, network lookups, tag writing) runs in workers with
  bounded concurrency.
- **Storage**: one SQLite database plus audio/tag caches. No external services are
  required to run; metadata providers are queried over HTTP when configured/enabled.

The API is deliberately small: `setup`, `status`, `inbox/check`, `tracks`, `write`
(`src/http/router.rs`). Production builds bind only to loopback and refuse
non-loopback bind addresses because the API has intentional access to local paths
(`src/main.rs:160`).

---

## 2. Data model

SQLite schema is versioned in `migrations/` and applied at startup.

| Table | Purpose |
|---|---|
| `tracks` | One row per input file: current tags, status, stage, content fingerprint, selected candidate, output path |
| `candidates` | One row per metadata suggestion from a provider; full field set + `score` + `score_breakdown` (JSON) |
| `candidate_sources` | Per-provider evidence rows for a candidate |
| `provider_cache` | Keyed HTTP responses from metadata providers (disposable, cleared at midnight) |
| `fingerprint_cache` | Chromaprint result keyed by path+size+mtime |
| `integrity_cache` | Healthy/corrupt verdict keyed by path+size+mtime |
| `replaygain_cache` | Track gain/peak keyed by path+size+mtime |
| `content_hash_cache` | SHA-256 fallback for duplicate detection |
| `artwork_overrides` | User-confirmed cover URL per path |
| `automatic_scan_files` | Size+mtime snapshot so automatic scans only re-process changed files |
| `settings` / `maintenance` | Key/value configuration |

**Track lifecycle** (the `status`/`stage` columns drive the UI):

```
new/discovered → needs_review/review ──▶ selected/ready ──▶ applied (row deleted after write)
                        │                        │
                        ├─ duplicate/skipped     ├─ needs_review/review (returned, "Undo")
                        ├─ corrupt/failed        └─ corrupt/failed
                        └─ unmatched/review
```

---

## 3. Startup sequence

1. Read the configuration from `MAGPIE_*` environment variables (`src/config.rs`),
   create the inbox, library, trash and data folders, connect to SQLite, run
   migrations, apply pending cleanup, empty trash days older than `MAGPIE_TRASH_DAYS`.
2. Launch background tasks (`src/main.rs`):
   - **Midnight cleanup** — deletes disposable provider responses / downloaded artwork
     while idle, and empties old trash days.
   - **Hourly media-cache limit** — enforces a shared 100 MiB budget on
     fingerprint/integrity/ReplayGain caches (oldest first).
   - **Inbox loop** — first refreshes the library index (§5a), then runs an inbox
     cycle every `MAGPIE_INBOX_CHECK_SECONDS` or immediately on `POST /api/inbox/check`.
     The library index is refreshed again every six hours.
3. Start the HTTP server, serving `/api/*` and the built web page.

---

## 4. The end-to-end algorithm (user journey)

### Phase 0 — Configuration

Folders and provider credentials come from the environment (`.env`). Nothing is
configured in the database; `GET /api/setup` only shows the current values.

### Phase 1 — Inbox check (every minute, or `POST /api/inbox/check`)

`scan::run_automatic` (`src/application/scan/mod.rs`):

1. Walk the inbox **off the async runtime** (`spawn_blocking`), skipping folders and
   files whose name starts with `.` or `@` (unfinished downloads, NAS metadata).
2. Keep only files that are new or changed since the last check (size + mtime in
   `automatic_scan_files`) **and** unchanged for `MAGPIE_INBOX_SETTLE_SECONDS`, so a
   file that is still being copied waits for the next check.
3. For each file, in parallel (limited by `metadata` + `fingerprint` semaphores):
   - Read tags/audio properties with `audio::read`.
   - Check decodability with `integrity::check` (cached).
   - Compute a Chromaprint fingerprint (cached) or fall back to SHA-256.
4. **Duplicate detection** inside the inbox runs before any network lookup (see §5).

### Phase 2 — Per-track matching

Each duplicate group is processed as one "file job" (`scan::process::process_file` →
`process`, `src/application/scan/process.rs:3`). Two attempts are allowed per
track; transient provider failures retry with backoff.

For each member of the group:

1. **Read metadata** — title/artist/album/etc. plus format, bitrate, duration.
2. **Integrity check** — decode the stream. Corrupt files are marked `corrupt`,
   blocked from writing, and counted as failed. (A later "check & fix" step can
   salvage them via `media/repair.rs`.)
3. **Fingerprint** — `fpcalc` via `fingerprint::calculate`, cached by
   path+size+mtime. Failure is non-fatal; text/web sources still run.
4. **Identify** — run the provider cascade (§7) and collect candidates.
5. **Dedupe + sort candidates** by trust tier then score.
6. **Smart auto-selection** (§6) decides whether to auto-approve.
   - No candidates → `unmatched`, stays in Review with a human-readable reason.
   - Candidates exist but fail the strict rules → stays in Review with the exact
     reason (ambiguous release, conflicting album, duration mismatch, different
     performer, etc.).
   - A decision is made → run **metadata completion** (§8).
7. **Metadata completion worker** merges missing fields only from *agreeing*
   recordings and *compatible* releases, then verifies the actual cover image by
   downloading it (§8). If title/artist/album/cover are still missing, the track
   stays in Review instead of creating an incomplete identification.
8. Persist the track + candidates + selected candidate. Matched tracks go to
   **Ready**; everything else stays in **Review**.

The whole scan stream is written through a single DB writer task
(`scan::persist::db_writer`, `src/application/scan/persist.rs:56`) so SQLite is only
ever touched from one connection at a time.

### Phase 3 — Review loop (human in the loop)

Tracks that auto-selection refused stay in Review (`stage='review'`). The user can:

- **Choose a candidate** (`POST /api/tracks/{id}/choose`) — re-runs completion +
  cover verification for that candidate. The choice is accepted on recording
  identity alone (title + artist + album); the track becomes Ready even if no
  catalog cover could be verified, and the cover is retried at write time.
- **Smart auto-select** (`POST /api/tracks/auto-approve`) — bulk-runs the same
  `smart_approval::select` + completion + cover pipeline over all Review items.
  Ambiguous/low-confidence ones stay in Review.
- **Edit manually** (`PUT /api/tracks/{id}/manual`) — free-form editor; pasting a
  Shazam/Spotify/YouTube/etc. song link can auto-fill fields via
  `/api/source/resolve`. Creates a `manual` candidate (score 100).
- **Update artwork** (`PUT /api/tracks/{id}/artwork`) — accepts a cover URL or a
  supported song link, verifies the downloaded image, records an `artwork_overrides`
  row so the choice survives future rescans.
- **Undo identification** (`POST /api/tracks/{id}/review`) — clears the selected
  candidate without deleting saved candidates.
- **Remove the file** (`DELETE /api/tracks/{id}`) — deletes the source file *only if*
  it resolves inside the configured input folder, and promotes the best remaining
  duplicate input if one exists.
- **Check & fix issues** (`POST /api/tracks/retry-issues`) — re-runs the pipeline on
  `corrupt`/`failed`/`provider_error`/missing/retryable-cover tracks. Corrupt files
  first go through `repair::repair`, which skips unreadable frames, re-encodes the
  valid audio, and keeps the damaged original as a `.magpie-damaged` backup.

### Phase 4 — File into the library (each inbox cycle, or `POST /api/write`)

The **apply service** (`src/application/apply.rs`) files every Ready track:

1. **Prepare** (`prepare_apply`): load every `stage='ready'` track with its selected
   candidate, re-run completion/canonicalization at this last mutable boundary,
   re-group by recording evidence, pick one representative per group (promoting the
   best *available* input if the selected one is missing), and compute each
   destination with the library layout (`library::destination`,
   `Album Artist/Album (Year)[ [Single]|[EP]]/NN - Title.ext`).
2. **Per song** (`apply`):
   - **Already in the library?** `library::find_same_recording` looks the recording up
     in the permanent index (§5a). If it is there and the new file is not an upgrade
     (lossless replacing lossy), the new file and its inbox duplicates go to the trash.
   - **ReplayGain** — measure loudness with `replaygain::get_or_analyze` (cached).
     Failure never blocks filing.
   - **Artwork** — `resolve_artwork` fetches the verified cover bytes (mandatory; no
     usable matching cover → back to Review).
   - **Copy to a temporary file** in the album folder (`.Name.magpie-{id}.ext`),
     **write tags** and verify them, **publish atomically** with `publish_no_clobber` (§9).
   - **Original to trash** — `MUSIC_ROOT/.magpie/trash/<date>/<path in inbox>`, only after
     the filed song exists. A replaced lossy copy (and its lyrics) goes there too.
   - **Index** the filed song, write `cover.jpg` into the album folder if it has none,
     and save lyrics from LRCLIB as a `.lrc`/`.txt` sidecar (`MAGPIE_LYRICS`).
   - Inbox duplicates are finished the same way (`finish_duplicate`); the `tracks`
     row is deleted.
3. **After the batch** — `rsgain easy` re-measures every album folder that got new
   songs (album gain), then one Navidrome `startScan` (when a login is configured).

---

## 4b. Getting music from links (`src/acquire/`)

| Part | File | Job |
|---|---|---|
| Link reading | `links.rs` | Spotify embed page, iTunes Lookup API and Apple playlist page, Deezer public API, yt-dlp for YouTube, the Shazam provider. Only public pages and documented APIs. |
| Queue | `store.rs`, `migrations/0007_requests.sql` | `requests` (one per link), `wanted` (one per song, in order), `playlists` (one per playlist link). Retries after 30 s, 2 min, 10 min; then failed. |
| Matching | `matching.rs` | Title 45 %, artist 35 %, length 20 %, version penalties (live, remix, sped up …). Official YouTube Music songs first (`NORMAL`), ordinary videos only as a strict fallback (`STRICT`). |
| yt-dlp | `ytdlp.rs` | YouTube Music song search, details, playlists, audio download (stream copy to m4a/opus), daily self-update of the copy in `DATA_DIR/bin`. |
| Worker | `worker.rs` | Per song: already in the library (same link or video, ISRC, or same title + first artist + length)? → choose the source → download to `.magpie/staging/` → write the link's title/artist/album/ISRC as tags. |

Downloads are then identified and filed by the normal inbox cycle, which also walks
`.magpie/staging/` (without the settle wait). When a download is filed, or recognised as a song
that is already in the library, `acquire::downloaded_file_done` records the library path and
rewrites the request's playlist (`library/Playlists/<name>.m3u8`, relative paths). Downloads are
deleted after filing instead of going to the trash.

## 5a. Library index

`library_items` (`src/application/library.rs`) keeps one row per audio file in the
library: path, size, mtime, duration, content key (fingerprint or SHA-256), ISRC,
MusicBrainz recording ID, format and bitrate. It is filled when magpie files a song
and by `library::index_library`, which walks the library at startup and every six
hours to index music added by hand and forget deleted files. A new song is the same
recording as a library file when the content is identical, or when the fingerprint,
ISRC or MusicBrainz recording ID match **and** the durations are within 3 seconds.

## 5. Duplicate detection algorithm (inside the inbox)

`input_dedup::group_recordings` (`src/application/input_dedup.rs:51`) groups input
files using **proof-only evidence** (never title similarity):

- **Exact SHA-256** (`sha256:` key) — definitive, unioned unconditionally.
- **Chromaprint fingerprint** (`fp:` key) — unioned only when durations match within
  ±3 seconds.
- **ISRC** — unioned only when the 12-character normalized ISRC matches *and*
  durations are within ±3 seconds.

Group membership is computed with a disjoint-set union; within each group the
representative is the **best-quality** file: lossless formats first, then higher
bitrate, then stable path order (`compare_quality`).

Only the representative is matched online and produces one corrected output; the
others are recorded as `duplicate/skipped` and later collapsed into that single
output. Different audio, remixes, live versions, and materially different durations
never union, so they remain separate and receive numbered filenames when corrected
names collide.

The same grouping is re-run at **apply time** (over `RecordingEvidence` built from
the DB) so the workspace and the write queue always agree.

---

## 6. Smart auto-selection

`smart_approval::rank` (`src/application/smart_approval.rs:51`) scores every
candidate instead of blindly trusting the raw provider score:

**Score components** (weighted sum):

| Signal | Weight / effect |
|---|---|
| Title similarity (normalized Levenshtein) | ×28 (or 21 if no existing title) |
| Artist similarity | ×18 (or 15 if no existing artist) |
| Duration delta | +16 (≤3s), +13 (≤8s), +7 (≤15s), +1 (≤30s), −14 (>30s) |
| Album match to existing tag | similarity ×18, −8 if conflicting |
| Metadata completeness | album, cover, year, track no., genre, album artist, ISRC |
| Multi-source agreement | up to +15 |
| Provider trust | 0–5 |
| Audio recognition bonus | +15 |
| Original album year bonus | +4 |
| Version tags (live/remix/acoustic/…) | −22 per unexpected, −16 per missing |
| Compilation penalty | −7 |

**Decision rules** — a candidate is auto-approved only when:
- It is *supported*: recognized by audio, corroborated by ≥2 sources, or an exact
  catalog match (credible provider + ≥0.90/≥0.82 title/artist + close duration).
- Its total is ≥ 68, and no *different* recording is within 12 points (ties with the
  same recording are tolerated; `same_recording` compares ISRC/version-tags/title).
- Its version tags match the filename's version tags.

Tracks that fail stay in Review with a specific human reason (this is why "Auto
approve" never writes a questionable release, a duet over a plain recording, a
stripped version over an album version, or a wrong album).

---

## 7. Provider cascade (identification)

`scan::providers::identify` (`src/application/scan/providers.rs:3`) runs a staged
cascade — the "catalog runner". Order matters — cheap/free/authoritative sources run
first, expensive or optional ones only when needed:

1. **AcoustID** (fingerprint lookup) → returns MusicBrainz recordings.
2. **YouTube Data API** — only if a filename contains an exact video ID.
3. **MusicBrainz text search** (title/artist).
4. **Apple Music (iTunes)**, **Deezer**, **Radio Javan**, **Audiomack**, **Navahang**
   — free catalogs, good for Persian + international music.
5. **Genius** — only when enrichment is still needed.
6. **SongRec / Shazam** (audio recognition) — only when AcoustID found nothing and
   no ≥90-score candidate exists; recognition is serialized (max 2 concurrent) and
   cached. Its artist/title is then **fed back through the catalogs** to find album,
   artwork, credits, release info, ISRC.
7. **AudD** — only if SongRec also failed (needs a token).
8. **Spotify** — ISRC cross-check plus release/artwork/track-number enrichment.
9. **SoundCloud** (needs credentials), then **Discogs**, **Last.fm**,
   **TheAudioDB**, **Wikidata** (also supplies artist genres).

Post-processing per track:
- Credits are normalized (`canonical_names`, `domain/credits`).
- Candidates are canonicalized (alias/prefer-latin names).
- **Source agreement** (`apply_source_agreement`) boosts corroborating rows.
- Artwork fallbacks + `artwork_overrides` are applied.
- Genres are enriched from Wikidata artist lookups when needed.
- The `score` stored per candidate is the final blended score with a JSON
  `score_breakdown` (sources, duration delta, audio-recognition flag, artwork list).

All HTTP goes through `resilient_http` (timeouts, retries) and a
`provider_cache`; MusicBrainz requests are rate-limited to one per second.

---

## 8. Metadata completion & cover verification

`metadata_completion::complete` (`src/application/metadata_completion.rs:32`):

1. Collect **agreeing recordings** (same ISRC, or same version tags + ≥0.90 title +
   ≥0.82 artist + compatible duration), ranked by donor quality.
2. **Never replace** a field that already has a value — only fill missing ones.
3. Copy release-specific fields (album, album artist, track/disc number, dates,
   label, country) **only from albums compatible with the chosen release** so a
   compilation can't silently change the original release.
4. Normalize release fields: `Single`/`EP` naming and album artist defaults.
5. `audit` produces a weighted completeness score plus the readiness flag
   `core_complete` (title, artist, album, **verified cover**).

**Cover-art worker** (`src/application/artwork.rs`):

- `collect_artwork_candidates` gathers every allowed URL for the selected
  recording: the candidate's own artwork list + `cover_url`, `score_breakdown`
  artwork alternatives, and **artwork already verified for the same release or
  ISRC elsewhere in the library** (`library_verified_artwork`). Bytes are shared
  through the artwork-url provider cache, so the first verified cover of an album
  becomes available to every other track on the same release.
- Each candidate image must *match the selected release* (release ID, ISRC+album,
  or compatible album+artist) unless user-confirmed. It is downloaded and
  validated (`cover_art_archive` / `inspect_artwork`), rejecting broken, blank,
  corrupted, or too-small images.
- `ensure_usable_cover` is the scan/selection worker. On temporary failures
  (timeout/429/5xx) it marks `retryable_error`; if the stored URLs are unusable it
  refreshes once from Deezer-by-ISRC + iTunes/Deezer, then checks the library
  again. Only when nothing verifies does it declare the cover missing
  (`cover_required`).
- `resolve_for_write` is the apply-time fetch: it returns the exact bytes to embed
  or fails, so a file is never written without valid matching artwork.

**Readiness**: `core_complete` — which requires a *verified* cover — is the bar for
every path to Ready: automatic write, explicit `/choose`, manual entry, artwork
update, and the on-demand `/tracks/{id}/artwork/search` worker. A track with a
missing, broken, or merely retryable cover stays in Review; the UI offers a
"Search for cover art" action that re-runs the worker for that track.

---

## 9. Atomic, no-clobber publication

`publish_no_clobber` (`src/application/apply.rs:644`):

1. `fsync` the finished temporary file.
2. List existing destination variants (`Name.ext`, `Name (2).ext`, …).
3. If an existing output is **equivalent audio** (same file size + SHA-256, or same
   Chromaprint fingerprint + ≤3s duration) **and already carries the exact artwork
   that is about to be published**, reuse it and delete the temporary — never
   creating a numbered copy. A stale output with missing or different cover art is
   not reused; the fresh temporary is published instead, so reused files always
   have valid matching artwork.
4. Otherwise create a **hard link** from the temporary to the next free numbered
   destination. `AlreadyExists` bumps the number. The source of the write is
   excluded from equivalence checks so re-running your own output isn't mistaken for
   a different recording.

This makes publication atomic, never clobbers existing files, and never creates
needless duplicates.

---

## 10. Concurrency, rate limiting, and lifecycle

Bounded concurrency lives in `AppState` (`src/app/state.rs`) and
`scan::PipelineLimits` (`src/application/scan/mod.rs:532`):

| Resource | Limit |
|---|---|
| `scan_workers` (tag readers / scan jobs) | configurable, default 6 |
| `fingerprint_workers` | default 3 |
| `lookup_workers` (metadata lookups + artwork downloads) | default 3 |
| SongRec recognition | hard cap 2 |
| `write_workers` (tag writes) | default 2 |
| MusicBrainz HTTP | 1 req/s |

- A single DB **writer task** serializes persistence during scans; apply uses
  per-operation transactions.
- **Cancellation**: `POST /api/stop` sets a `cancelled` flag that every worker checks
  at safe boundaries; SIGTERM/Ctrl+C asks the active workflow to stop, waits up to
  30 s, drains HTTP, and closes SQLite.
- **One workflow at a time**: an inbox cycle and a manual "File now" never run at
  the same time (`try_claim_workflow`). Choices made in Review are filed by the next
  cycle.

---

## 11. Caching summary

| Cache | Key | Invalidated by |
|---|---|---|
| `provider_cache` | provider + search key | expiry (disposable, midnight cleanup) |
| `fingerprint_cache` | path + size + mtime | file change / cache-limit eviction |
| `integrity_cache` | path + size + mtime | explicit retry deletes the row |
| `replaygain_cache` | path + size + mtime(ns) | file change |
| `content_hash_cache` | path + size + mtime(ns) | file change |
| `artwork_overrides` | path | overwritten when user confirms new cover |

Fingerprint/integrity/ReplayGain caches share a 100 MiB budget; the oldest entries
are evicted first (checked at startup and hourly while idle).

---

## 12. API reference

| Method & path | Purpose |
|---|---|
| `GET  /api/setup` | Read-only configuration and tool/provider status |
| `GET  /api/status` | Workflow phase/progress + matched/review/failed counts |
| `POST /api/inbox/check` | Check the inbox now instead of at the next interval |
| `POST /api/stop` | Cancel current scan/write at a safe boundary |
| `GET  /api/tracks` | All tracks + candidates |
| `DELETE /api/tracks/{id}` | Remove a review file from disk (input folder only) |
| `POST /api/tracks/retry-issues` | Repair + re-run failed/corrupt/retryable tracks |
| `POST /api/tracks/auto-approve` | Bulk smart auto-select over Review |
| `GET  /api/tracks/{id}/audio` | Range-stream the source file for playback |
| `POST /api/tracks/{id}/choose` | Accept a candidate |
| `POST /api/tracks/{id}/review` | Undo identification (back to Review) |
| `PUT  /api/tracks/{id}/manual` | Save a manually entered candidate |
| `PUT  /api/tracks/{id}/artwork` | Set + verify a cover URL / song link |
| `POST /api/tracks/{id}/artwork/search` | Re-run the cover-art worker for a track |
| `POST /api/source/resolve` | Resolve a Shazam/Spotify/YouTube/… link to metadata |
| `GET  /api/candidates/{id}/artwork/preview`, etc. | Artwork previews |
| `POST /api/write` | File all Ready tracks into the library now |
| `POST /api/requests` | Read a link and queue its songs |
| `GET  /api/requests` | The latest requests with progress counts |
| `GET  /api/requests/{id}` | Every song of one request with its status |
| `POST /api/requests/{id}/retry` | Queue a request's failed songs again |

---

## 13. Key source files

| Area | File |
|---|---|
| Entry point, startup tasks, graceful shutdown | `src/main.rs` |
| HTTP routes / API contract | `src/http/router.rs`, `src/http/handlers/*` |
| Scan + identify pipeline (walk, file jobs, cascade, DB writer) | `src/application/scan/` (`mod.rs`, `process.rs`, `providers.rs`, `persist.rs`, `score.rs`) |
| Catalog runner (provider cascade) | `src/application/scan/providers.rs` |
| Duplicate detection | `src/application/input_dedup.rs` |
| Smart auto-selection | `src/application/smart_approval.rs` |
| Metadata completion | `src/application/metadata_completion.rs` |
| Cover-art worker (search, verify, reuse, apply-time fetch) | `src/application/artwork.rs` |
| Apply service (prepare / write / publish) | `src/application/apply.rs` (+ endpoint `src/http/handlers/apply.rs`) |
| Review / scan / workspace handlers | `src/http/handlers/tracks.rs`, `scan.rs`, `workspace.rs`, `queries.rs` |
| Repository (track + candidate types and queries) | `src/infrastructure/db/tracks.rs` |
| Audio read / integrity / fingerprint / replaygain / tag write / repair | `src/infrastructure/media/*` |
| Provider integrations | `src/infrastructure/providers/*` |
| App state, workflow, limits | `src/app/state.rs`, `src/application/scan/mod.rs` |
| Configuration | `src/config.rs` |
| SQLite schema | `migrations/*.sql` |
| Library layout, trash, index, folder cover | `src/application/library.rs` |
| Lyrics, Navidrome, album gain | `src/infrastructure/lyrics.rs`, `navidrome.rs`, `media/rsgain.rs` |
| Configuration | `src/config.rs` |
| Web page | `web/src/app/App.tsx` |

---

## 14. Summary flow

```
Inbox check (every minute)
  ├─ walk inbox (skip .hidden/@nas, wait for files to settle) → read tags
  │   → integrity check → fingerprint → dedup group
  ├─ provider cascade → candidates → smart auto-select
  │     ├─ auto-approved → metadata completion → cover verified → READY
  │     └─ uncertain/unmatched/corrupt → REVIEW
Review → choose / auto-select / manual / source-link / artwork / retry-issues
  ├─ every path to READY requires core_complete, incl. a verified cover
  └─ the next inbox cycle files what you approved
File → per song: already in library? (skip or lossless upgrade) → ReplayGain
     → verified cover → copy to temp → write+verify tags → atomic no-clobber publish
     → original to trash → index → cover.jpg → lyrics
     → after the batch: album gain (rsgain) → Navidrome scan
```
