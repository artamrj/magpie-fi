# magpie-fi — Technical Overview

magpie-fi is one program. It gets songs from links (Telegram bot, web
page), files sent to the bot and an inbox folder; checks that each file can be decoded;
identifies it through a cascade of online sources; lets you approve or correct uncertain matches;
and files each song into the library with verified cover art, lyrics and ReplayGain, moving the
original to a trash folder. The identification engine comes from the earlier *ununknown* project.

This document describes how it works: the parts, the data, and the exact steps a song goes
through. The design and its reasons are in [BLUEPRINT.md](BLUEPRINT.md).

---

## 1. Architecture

```
┌────────────────────────────┐      ┌──────────────────────────────┐
│ Web page (React, web/)     │      │ Telegram (links, files,      │
│ Get music · All tracks ·   │      │ buttons, Mini App)           │
│ Review · Issues · Ready ·  │      └──────────────┬───────────────┘
│ Inspector · status dock    │                     │ Bot API (long polling)
└─────────────▲──────────────┘                     │
              │ HTTP 127.0.0.1:7331                │
┌─────────────┴────────────────────────────────────▼───────────────┐
│ magpie-fi (Rust, axum + tokio + SQLite/sqlx)                     │
│                                                                  │
│  http: router → auth → handlers        telegram: bot, status     │
│  acquire: links → wanted → yt-dlp → .magpie-fi/staging           │
│  application:                                                    │
│    scan (walk · process · providers · persist)                   │
│    input_dedup · smart_approval · metadata_completion · artwork  │
│    apply (file into library) · library (layout, index, trash)    │
│  domain: audio reading, matcher, credits, genres                 │
│  infrastructure:                                                 │
│    providers: identification, cover and genre sources (§10)      │
│    media: integrity · fingerprint · tag_writer · repair · rsgain │
│    db + caches · lyrics (LRCLIB) · process (tool runner)         │
└───────────────────────────────┬──────────────────────────────────┘
                                │ only writer
                                ▼
                       MUSIC_ROOT/library ──► Navidrome (run by you)
```

- **Web page** (`web/`): React, built with Vite and served from disk by the same process
  (`MAGPIE_WEB_DIR`, `/usr/share/magpie-fi` in the image). It polls `/api/status`, `/api/tracks`
  and `/api/requests`.
- **Program** (`src/`): one binary. Heavy work (decoding, fingerprinting, network lookups, tag
  writing) runs with bounded concurrency (§13).
- **Storage**: one SQLite file plus caches in it. Settings come only from `MAGPIE_*` environment
  variables (`src/config.rs`).
- magpie-fi never calls Navidrome. Navidrome's file watcher sees new files and playlists.

---

## 2. Data model

The schema is in `migrations/` and is applied at startup. Current tables:

| Table | Purpose |
|---|---|
| `tracks` | One row per inbox/staging file on its way: current tags, `status`/`stage`, content key, selected candidate. Deleted once the song is filed. |
| `candidates` | One row per suggestion from a source: full field set, `score`, `score_breakdown` (JSON: sources, duration delta, audio recognition, artwork), artist credits, artwork list and status |
| `artwork_overrides` | Cover chosen by you for a file, so it survives a rescan |
| `canonical_names` | Preferred spelling of artist/album names (aliases, Latin vs. original script) |
| `automatic_scan_files` | Size + mtime of every checked file, so only new or changed files are processed |
| `library_items` | The permanent library index: one row per audio file in `library/` (§8) |
| `requests` | One row per link someone asked for; Telegram status message ids |
| `wanted` | One row per song a request wants, in order: status, attempts, staging/library path |
| `playlists` | One row per playlist link → one `.m3u8` file |
| `telegram_guests`, `telegram_invites`, `telegram_state` | Invited guests, one-time invite tokens, the update offset |
| `login_links`, `sessions` | Web login: one-time links and 30-day sessions (SHA-256 hashes only) |
| `provider_cache` | Source answers and downloaded cover bytes, each with an expiry (§14) |
| `fingerprint_cache`, `integrity_cache`, `content_hash_cache` | Per-file analysis results keyed by path + size + mtime |
| `maintenance` | Small key/value bookkeeping (last daily cleanup) |

**Track lifecycle** (`status`/`stage`; the web page's tabs are Review, Issues and Ready):

```
new → review (needs you) ──▶ ready ──▶ filed (row deleted)
        │                      │
        ├─ duplicate/skipped   ├─ review (cover failed, or "Undo")
        ├─ corrupt/failed      └─ failed (filing failed → Issues)
        └─ unmatched/review
```

---

## 3. Startup and background loops

`src/main.rs`:

1. Read the configuration, create the inbox, library, trash, staging and data folders, connect
   to SQLite (WAL, `0600` file), run migrations. Tracks left `processing` by a restart are marked
   failed; expired source answers are removed.
2. Run the daily cache cleanup if it has not run today, empty trash days older than
   `MAGPIE_TRASH_DAYS`, and enforce the media-cache limit.
3. Start the background tasks:
   - **Midnight**: daily cache cleanup (§14) and trash purge, after any running workflow.
   - **Hourly**: media-cache limit, skipped while a workflow runs.
   - **Inbox loop**: refresh the library index (at start, then every six hours), then one inbox
     cycle every `MAGPIE_INBOX_CHECK_SECONDS` or right away on `POST /api/inbox/check`.
   - **Download worker** (`acquire::worker`), **Telegram bot** (`telegram::run`).
   - **yt-dlp updater**: once a day, when `MAGPIE_YTDLP_SELF_UPDATE` is on (it is in the image).
4. Serve `/api/*` and the web page. On SIGTERM/Ctrl+C the running workflow is asked to stop at a
   safe point and given up to 30 s.

---

## 4. The inbox pipeline

### Phase 1 — Inbox check

`scan::run_automatic` (`src/application/scan/mod.rs`):

1. Walk the inbox and `.magpie-fi/staging/` off the async runtime, skipping files and folders whose
   name starts with `.` or `@` (unfinished downloads, NAS metadata).
2. Keep files that are new or changed since the last check (`automatic_scan_files`). Inbox files
   must also be unchanged for `MAGPIE_INBOX_SETTLE_SECONDS`; staging files don't wait.
3. For each file, in parallel: read tags and audio properties, check decodability (cached),
   compute a Chromaprint fingerprint (cached) or fall back to SHA-256.
4. Group duplicates inside the inbox (§8) before any network lookup.

### Phase 2 — Identify each song

Each duplicate group is one job (`scan::process::process_file`). A job gets two attempts.

1. **Corrupt** files are marked `corrupt` and go to Issues.
2. **Identify** with the source cascade (§10). A source that answers 401 (wrong key) is skipped
   for the rest of that check. A 429 or a timeout pauses it for 30 s, doubling up to 5 minutes,
   after which it is asked again. iTunes gets at most one search every 3 s (Apple allows ~20 a
   minute).
3. **Sort** candidates by trust tier, then score.
4. **Smart auto-selection** (§9) decides. No candidates → `unmatched`; candidates that fail the
   rules → Review with the exact reason (ambiguous release, conflicting album, duration
   mismatch, different performer, …).
5. **Metadata completion** and **cover verification** (§11). If title, artist, album or a
   verified cover is still missing, the song stays in Review.
6. The result is written in one transaction per file (`scan/persist.rs`). Matched songs are
   **Ready**.

### Phase 3 — Review

In Review you can:

- **Choose a candidate** (`POST /api/tracks/{id}/choose`): re-runs completion and cover
  verification for it.
- **Auto-approve** (`POST /api/tracks/auto-approve`): the same rules as Phase 2 over all of
  Review; doubtful songs stay.
- **Edit by hand** (`PUT /api/tracks/{id}/manual`): a `manual` candidate (score 100). Pasting a
  Shazam/Spotify/YouTube/… song link fills the fields (`POST /api/source/resolve`).
- **Change the cover** (`PUT /api/tracks/{id}/artwork`): a cover URL or a song link; the image is
  verified and remembered in `artwork_overrides`. **Search again**:
  `POST /api/tracks/{id}/artwork/search`.
- **Undo** (`POST /api/tracks/{id}/review`): clears the choice, keeps the candidates.
- **Remove** (`DELETE /api/tracks/{id}`): only unresolved Review songs inside the inbox or
  staging. Your files go to the trash; magpie-fi's downloads are deleted.
- **Check & fix** (`POST /api/tracks/retry-issues`): re-runs everything in Issues. Corrupt files
  first go through `media::repair`, which re-encodes the decodable audio and keeps the damaged
  original as a `.magpie-fi-damaged` backup.

The Telegram bot asks about Telegram songs in Review with up to three candidates and calls the
same choose/remove logic.

### Phase 4 — File into the library

Each inbox cycle ends with `apply::apply_ready_automatically`; `POST /api/write` starts the same
step now. `src/application/apply.rs`:

1. **Prepare**: load every Ready track with its candidate, re-run completion and name
   canonicalisation, re-group duplicates, pick the best available file per group, and compute
   the destination (`library::destination`, §5 of the blueprint).
2. **Per song**:
   - **Already in the library?** (`library::find_same_recording`). If yes and the new file is not
     an upgrade (lossless over lossy), the new file and its inbox duplicates go to the trash.
   - **Cover**: fetch the verified bytes; no usable cover → back to Review, with the cover marked
     as needed so the inspector asks for one.
   - **Lyrics** from LRCLIB (`MAGPIE_LYRICS`, 10 s limit; a failed lookup means no new lyrics).
   - **Copy** to a temporary file in the album folder, **write and verify tags**, the embedded
     cover and the lyrics, **publish** without overwriting anything (§12).
   - **Original to trash** (`.magpie-fi/trash/<date>/…`), only after the new file exists. A replaced
     lossy copy and its lyrics go there too. Downloads from staging are deleted instead.
   - **Index** the new file. With `MAGPIE_SIDECARS=true` also write `cover.jpg` into the album
     folder if it has none and save the lyrics as `.lrc` (synced) or `.txt`. None of these extras
     can undo the filing.
   - If anything fails, that song is marked `failed` (Issues) and the next one continues.
3. **After the batch**: `rsgain easy` on every album folder that got new songs writes track and
   album ReplayGain (R128 tags for Opus). Without rsgain no gain tags are written and one warning
   is logged per batch.

---

## 5. Getting music from links (`src/acquire/`)

| Part | File | Job |
|---|---|---|
| Link reading | `links.rs` | Spotify embed page, iTunes Lookup API and Apple playlist page, Deezer public API, yt-dlp for YouTube; Shazam `/song/<n>/` links through the iTunes Lookup API (the number is the Apple Music ID; Shazam's pages refuse automated requests), the Shazam page only as a fallback. Only public pages and documented APIs. |
| Queue | `store.rs` | `requests`, `wanted`, `playlists`. Retries after 30 s, 2 min, 10 min; then failed. A request's `title` falls back to its first song. |
| Matching | `matching.rs` | Title 45 %, artist 35 %, length 20 %, version penalties (live, remix, sped up …). Official YouTube Music songs first, ordinary videos only as a strict fallback. |
| yt-dlp | `ytdlp.rs` | YouTube Music search, details, playlists, audio download (stream copy to m4a/opus), daily self-update of the copy in `DATA_DIR/bin`. |
| Worker | `worker.rs` | Per song: already in the library (same link or video, ISRC, or same title + first artist + length)? → choose the source → download to `.magpie-fi/staging/` → write the link's title/artist/album/ISRC as tags. |

Downloads are then identified and filed by the normal inbox cycle. When a download is filed, or
found to be in the library already, `acquire::downloaded_file_done` records the library path and
rewrites the request's playlist (`library/Playlists/<name>.m3u8`, relative paths).

## 6. Telegram (`src/telegram/`)

| Part | File | Job |
|---|---|---|
| Client | `api.rs` | Bot API calls to `api.telegram.org` with magpie-fi's HTTP client. Waits on "retry after", ignores "message is not modified", drops the URL (it holds the token) from every error. Bots can download files up to 20 MB; bigger files get a clear answer. |
| Types | `types.rs` | The Bot API types the bot reads, written by hand (Bot API 10.4). Only the fields in use; unknown fields are ignored, so a new API version never breaks reading updates. |
| Bot | `mod.rs` | Long polling (offset in `telegram_state`), owners, guests and strangers, sends a message to the right handler, Mini App links, small send/edit/answer/react helpers. |
| Intake | `intake.rs` | Links → `requests`; audio files → `wanted` rows downloaded into staging. Files sent within 3 s join one upload request (a forwarded album). |
| Commands | `commands.rs` | `/start`, `/help`, `/queue`, `/health`, `/web`, `/guests`, `/id`, random one-time invite tokens. |
| Buttons | `actions.rs` | One `Action` type for every button: its `callback_data` (`stop:12`, `pick:3:9`, at most 64 bytes), who may press it (owners anything, guests only their own requests and songs), and what it does. |
| Status | `status.rs` | Every 3 s: reads all open requests with their progress in one query, edits a status message only when its text changed, closes it when nothing is left, and asks about songs waiting in Review. |
| Texts | `texts.rs` | Everything the bot says and every button label, and the progress bar. |

## 7. Web page login (`src/http/auth.rs`)

- Without Telegram there is no login: anyone who can reach the port can use the page, and
  `MAGPIE_PUBLIC_URL` is refused.
- With Telegram, `require_login` guards every `/api` route except `health`, `login` and `me`:
  - **Mini App**: `Authorization: tma <initData>`; the HMAC-SHA256 signature is checked with a key
    derived from the bot token, and the data may be at most 24 h old. `/api/me` also sets a session
    cookie, so `<img>` and `<audio>` requests work.
  - **Browser**: `/web` in the bot creates a one-time link (`login_links`, 10 minutes) →
    `/api/login` starts a 30-day session (`HttpOnly` cookie; `SameSite=None; Secure` over HTTPS).
  - Only owners get through; guests get 403.
- Write requests from a foreign `Origin` are rejected. The CSP allows Telegram's Mini App script
  and framing by `web.telegram.org`, nothing else. Covers are always loaded through magpie-fi's own
  preview endpoints, never from catalog URLs.
- The Mini App opens on Review with `?view=review`.

---

## 8. Library index and duplicates

**Library index.** `library_items` (`src/application/library.rs`) keeps one row per audio file in
the library: path, size, mtime, duration, content key (fingerprint or SHA-256), ISRC, MusicBrainz
recording ID, format, bitrate, the Chromaprint fingerprint, the title key and the artist
identities (`|3|17|`, primary first). It is filled when magpie-fi files a song and by
`library::index_library` (at start and every six hours), which also indexes music added by hand
and forgets deleted files; rows from before these columns are filled 500 per pass. A new song is
the same recording as a library file when the content is identical, or when the fingerprint
string, ISRC or MusicBrainz recording ID match **and** the durations are within 3 seconds, or
when the fingerprints **sound the same** (`domain::fingerprint::similarity` ≥ 0.85, best of ±15 s
offsets; re-encodes score 0.95–1.0, different songs 0.45–0.69) with durations within 3 seconds.

**Possible duplicates.** Without that proof, a song with the same title key (`feat.` removed,
spelling unified), a length within 3 s and an overlapping primary artist (in any order or
spelling) goes back to Review as "Possible duplicate" before filing. **File anyway** files it.

**Duplicates inside the inbox.** `input_dedup::group_recordings` groups files by proof only, never
by title:

- same SHA-256 — always;
- same Chromaprint fingerprint — only when durations are within ±3 s;
- same ISRC — only when durations are within ±3 s;
- fingerprints that sound the same (≥ 0.85) — only when durations are within ±3 s.

The best-quality file of each group (lossless first, then bitrate, then path) is identified
online; the others become `duplicate/skipped` and are trashed when it is filed. Remixes, live
versions and different lengths never group. The same grouping runs again when filing.

**Files tab** (`src/application/file_browser.rs`). Every path the page sends is
relative to `library/` and goes through `file_browser::resolve`: absolute paths, `..`, hidden or
NAS folders (`.`/`@`) and links that lead outside the library are refused (422). A folder
listing reads the folder itself (so unindexed files show, named after the file) and takes
titles and song counts from `library_items`, so no tags are read to list a folder. Search
needs every word to appear in the title, artist, album, path or `title_key` (so `ي`/`ی` and
accents do not matter for titles), returns at most 100 songs, and finds folders whose path holds
every word and whose own name holds one. One song's details are read fresh from the file with
`tag_writer::read_details`, exactly as written (nothing cleaned up), with a `version` (size and
modification time).

Saving (`PUT /files/song`, `file_browser::edit_song`) takes the workflow slot, then:
1. refuses the save with 409 when the file's version is not the one the page read;
2. checks the typed text (one line, NFC, at most 500 characters; title and artist stay; numbers
   1–999; a year of four digits; a 12-character ISRC) and keeps only fields that differ;
3. writes them with `tag_writer::edit_fields` on a hidden copy in the same folder
   (`files::edit_on_copy`, shared with the Artists page), reads the copy back, and renames it
   over the song only when every field reads as written;
4. describes the song again into `library_items`.
A new artist or album artist rewrites ARTISTS / ALBUMARTISTS from the text (`A feat. B` gives
two, `A & B` stays one) and drops that artist's sort name and MusicBrainz artist IDs; a new year
also replaces a release date of another year; an empty field removes the tag. The answer holds
the old values of the changed fields, and the page's Undo saves them again. WAV and AIFF are not
edited, and no file or folder is renamed.

"Edit all songs" reads `GET /files/folder-tags` (every song directly in the folder with its
version, and per field the value they share or how many different values there are), then sends
one `PUT /files/songs` with an edit per song (at most 500). `file_browser::edit_songs` checks
every song first (version, values, no song twice) and writes nothing when one fails; then it
writes them one by one and reports any song that failed on disk. Undo sends each song's own old
values through the same route.

---

## 9. Smart auto-selection

`smart_approval::rank` (`src/application/smart_approval.rs`) scores every candidate:

| Signal | Weight / effect |
|---|---|
| Title similarity | ×28 (or 21 if the file has no title) |
| Artist similarity | ×18 (or 15 if the file has no artist) |
| Duration delta | +16 (≤3 s), +13 (≤8 s), +7 (≤15 s), +1 (≤30 s), −14 (>30 s) |
| Album match to existing tag | similarity ×18, −8 if conflicting |
| Metadata completeness | album, cover, year, track no., genre, album artist, ISRC |
| Agreement between sources | up to +15 |
| Source trust | 0–5 |
| Recognised by audio | +15 |
| The file's own ISRC, length within 3 s | +15 |
| Original album year | +4 |
| Version tags (live/remix/acoustic/…) | −22 per unexpected, −16 per missing |
| Compilation | −7 |

A candidate is approved automatically only when:

- it is *supported*: recognised by audio, carrying the file's own ISRC with a length within 3 s,
  confirmed by ≥2 sources, or an exact catalog match (credible source, ≥0.90 title / ≥0.82
  artist similarity, close duration);
- its total is ≥ 68 and no *different* recording is within 12 points. Every supported candidate
  below the best is checked, not only the runner-up. Releases of the *same* recording are not
  rivals: same ISRC; or same version tags and ≥0.90 title, plus either ≥0.82 artist similarity
  or one credit naming only some of the other's singers ("54 Ultra" within "ANOTR feat. 54
  Ultra") with both lengths within 3 s of the file. A title in another Latin spelling of the
  same Persian words counts as the same title when both lengths are within 3 s of the file
  (`names::loose_title_key`: the artist Latin key, plus the ezafe "-ye"/"-yeh"/"-e" dropped
  from words that are not last, so "Gheseh Eshgh" = "Ghesseyeh Eshgh");
- its version tags match the file's (`session` counts as a version, so "Marie (Dach Session)" is
  not "Marie"), and its length is at most 15 s off the audio.

**The best match is kept, cover or not** (`scan::process::decide`). The chosen release is
completed and needs title, artist, album and a verified cover. Correct tags matter more than a
cover, so the release is never swapped for another one that has a cover. When only the cover is
missing, the song waits in Review **with its match chosen** (as when you pick it yourself), the
page asks only for a cover, and the cover finder (§11) keeps looking.

**Listening as a tie-breaker.** When the text sources cannot decide (no approval, or a release
missing its title, artist or album) and SongRec has not listened yet, `identify` runs again with `Listen::Always`: SongRec
recognises the audio and its result is looked up in the catalogs. The text answers come from
`provider_cache`, so the second pass costs one recognition.

Everything else stays in Review with a reason in plain words (`smart_approval::unsure_reason`):
two different recordings fit equally well, the closest match is N seconds longer or shorter, is
another version, is credited to other singers, or only one source found it (and whether that
source gave a length at all). The Review screen shows this text. A release without a length
(Genius) shows "Not known" for length, not "different".

**Titles and artists** are compared the same way in the per-source score and here
(`domain::versions`):

- Names credited in a title count as artists of the song: "In The End (Mellen Gi & Tommee
  Profitt Remix)" tagged *Linkin Park* matches "In the End (Mellen Gi Remix)" by *Tommee Profitt,
  Mellen Gi & Fleurie* (two shared names = full artist match, one = 0.85).
- A match that names the file's featured singers but leaves out its main singer ("Sohrab MJ &
  Orchid" for *Amir Tataloo feat. Sohrab MJ & Orchid*) scores at most 0.75 for the artist:
  below the 0.82 of an exact match, so it needs the audio or a second source. Remixers named
  in the title don't trigger this. When such a match is used, the main singer is put back
  first (`credits::restore_missing_main`).
- Two remix (live, acoustic…) titles match when the song and the remixer agree.
- Artist lists are compared as sets of people: "A, B & C" = "A, C & B".
- **Upload sites** (Audiomack, SoundCloud, YouTube) show what the uploader typed, often the same
  wrong tags as the file, so they are not an exact catalog match by themselves.

**Tags that advertise** a channel or site (`@Channel`, `t.me/…`, `www.….ir`, `کانال …`) count as
no tag; a "Artist - Title" file name is then used. Files sent to the Telegram bot get Telegram's
title and performer written into them and are named `Artist - Title (u6).ext`.

---

## 10. Source cascade

`scan::providers::identify` (`src/application/scan/providers.rs`) asks 15 identification sources,
cheap and free ones first:

1. **AcoustID** (fingerprint, needs `MAGPIE_ACOUSTID_KEY`) → **MusicBrainz** recordings.
2. **YouTube Data API**, only if the filename contains a video ID (needs a key).
3. **MusicBrainz** text search.
4. **Apple Music (iTunes)**, **Deezer**, **Radio Javan**, **Audiomack** (an upload site),
   **Navahang** — free
   catalogs, good for Persian and international music.
5. **Genius**, only when the results still lack details.
6. **Deezer by ISRC** (`track/isrc:…`, free), when the file has an ISRC tag and no source
   returned that code yet. Finds songs whose names are spelled differently in the catalogs.
7. **SongRec** (Shazam recognition), when AcoustID found nothing and no candidate scores ≥ 90,
   or on the second pass when the text sources could not decide (§9); at most two at a time.
   Its artist/title is fed back through the catalogs of step 3–5.
8. **SoundCloud**, **Discogs**, **Last.fm** — each only with its free key or token. No paid
   service is used.

Free without a key: MusicBrainz, Apple Music, Deezer, Radio Javan, Audiomack, Navahang, Genius,
SongRec. Other sources: **Cover Art Archive** (covers by MusicBrainz release), **Wikidata**
(artist genres, only when the candidates have no specific genre), **LRCLIB** (lyrics when filing).

After the cascade: names are canonicalised (`canonical_names`, `domain/credits`), agreeing
candidates are boosted (`apply_source_agreement`), cover fallbacks and `artwork_overrides` are
applied, and genres are filled. Only real MusicBrainz IDs are ever written to MusicBrainz tags.

All HTTP goes through one client with timeouts (`resilient_http` adds a DNS-over-HTTPS fallback
for known catalog hosts) and through `provider_cache`. MusicBrainz is limited to one request per
second. `RUST_LOG=magpie_fi=debug` logs what every source returned or why it was skipped.

---

## 11. Metadata completion and cover verification

`metadata_completion::complete` (`src/application/metadata_completion.rs`):

1. Collect **agreeing recordings** (same ISRC, or same version tags + ≥0.90 title or the same
   Persian words in another spelling with both lengths within 3 s + ≥0.82 artist + compatible
   duration).
2. **Never replace** a field that has a value; only fill missing ones. Two exceptions:
   - **Title spelling:** when agreeing matches differ only in spelling, the spelling most
     sources use wins (Apple Music + Radio Javan + Audiomack "Gheseh Eshgh" over MusicBrainz
     "Ghesseyeh Eshgh"); a tie keeps the chosen match's spelling.
   - **Album year:** stores list old albums under the year they went digital. An earlier year
     from another listing of the same plain album (same title, not a single, EP, compilation
     or live album) wins: Ebi's "Koohe Yakh" is 1991 on MusicBrainz, not 2008 as on Apple
     Music. The release date follows when it is in that year, otherwise it is cleared.
3. Copy release fields (album, album artist, track/disc number, dates, label, country) **only
   from compatible releases**, so a compilation can't replace the original album.
4. Normalise `Single`/`EP` and album-artist defaults.
5. `audit` gives a completeness score and `core_complete` (title, artist, album, **verified
   cover**).

**Cover worker** (`src/application/artwork.rs`):

- `collect_artwork_candidates`: the candidate's own artwork list and `cover_url`, plus covers
  already verified for the same release or ISRC in the library. Release IDs are compared only
  when they are MusicBrainz IDs.
- Each image must belong to the selected recording: you chose it, it came with that match's own
  catalog record, or it shares the release ID, the ISRC (+ album), or a compatible album + artist.
  Albums are compared without release labels (`Farangis` = `Farangis - Single`); a catalog row
  without an album name only needs the artist (it already agreed on the title). The image is
  downloaded and checked; broken, blank or single-colour images and images smaller than 300×300
  are rejected.
- The candidate and track cover previews use the same check, so the page only shows a cover that
  would be written.
- `ensure_usable_cover`: temporary failures (timeout, 429, 5xx) mark the cover retryable. If the
  stored URLs are unusable it tries, in order: a fresh lookup of the **song** (Deezer by ISRC,
  iTunes and Deezer by title); then the **album itself**: the front cover of the MusicBrainz
  release group (any edition of the same album, Cover Art Archive), and album searches by album
  name and album artist on iTunes (`entity=album`) and Deezer (`search/album`). Each is still
  checked against album and artist (the album artist counts too). The release and its tags
  never change. Only then is the cover "missing"; the time of that search is stored as
  `metadata_completion.cover_searched_at`.
- **Cover finder** (`src/application/cover_finder.rs`): in every inbox cycle, between identifying
  and filing, it runs the cover worker again for songs in Review whose chosen match lacks a cover
  (`cover_required` or `retryable_error`): at most 25 per run, each at most once every 12 hours.
  It takes the workflow slot. A song whose cover is found becomes Ready and is filed in the same
  cycle; one the owner changed meanwhile is left alone.
- `resolve_for_write`: at filing time, returns the exact bytes to embed or fails, so no file is
  written without a matching cover.

`core_complete` is the bar for every path to Ready: automatic, chosen, manual, new cover, cover
search.

---

## 12. Publishing without overwriting

`publish_no_clobber` (`src/application/apply.rs`):

1. `fsync` the finished temporary file.
2. List existing variants (`Name.ext`, `Name (2).ext`, …).
3. If one is the **same audio** (same size + SHA-256, or same fingerprint within 3 s) **and
   already has the same cover**, keep it and delete the temporary file.
4. Otherwise **hard-link** the temporary file to the next free name (`AlreadyExists` → next
   number) and remove the temporary name.

Nothing is ever overwritten, and no needless numbered copies appear.

---

## 13. Concurrency, tools and lifecycle

Fixed limits (`src/config.rs`, `scan::PipelineLimits`):

| Resource | Limit |
|---|---|
| Files read at once during a check | 6 |
| Fingerprints | 3 |
| Catalog lookups / cover downloads | 3 |
| SongRec recognitions | 2 |
| Tag writes | 2 |
| MusicBrainz | 1 request/s |
| Downloads | `MAGPIE_MAX_PARALLEL_DOWNLOADS` (default 2) + `MAGPIE_DOWNLOAD_PAUSE_SECONDS` |

- **External tools**: the integrity check, fingerprint, repair, rsgain and SongRec run
  through `infrastructure::process`: whether a tool is installed is checked once per
  run, and every call has a timeout after which the process is killed (e.g. 3 min for a
  fingerprint, 30 min for rsgain on one album). yt-dlp calls have their own timeouts
  (`MAGPIE_MAX_DOWNLOAD_SECONDS` for a download, default 15 min).
- **One workflow at a time**: an inbox cycle, "File now", auto-approve and "Check & fix" never run
  together (`try_claim_workflow`). `POST /api/stop` asks the running one to stop at a safe point.

---

## 14. Caching

| Cache | Key | Lifetime |
|---|---|---|
| `provider_cache` | source + query/ID/URL | per source, 7–90 days (e.g. AcoustID 90, MusicBrainz recordings and covers 30, catalog searches 7–14). Failed lookups are not cached. Expired rows are removed at start and daily. |
| `fingerprint_cache`, `integrity_cache`, `content_hash_cache` | path + size + mtime | until the file changes; together capped at **100 MiB**, oldest first (at start, daily and hourly when idle) |
| `artwork_overrides` | path | until you choose another cover |

After a cleanup that removed rows the database is compacted (`VACUUM`).

---

## 15. API reference

All routes are under `/api` (`src/http/router.rs`).

| Method & path | Purpose |
|---|---|
| `GET /health` | Database reachable? (no login) |
| `GET /me` | Who is using the page, or 401 (no login) |
| `GET /login?token=` | Use a one-time login link, start a session (no login) |
| `POST /logout` | End this browser's session |
| `GET /setup` | Read-only settings, tool and source status |
| `GET /status` | What the background workflow is doing: phase, message, progress |
| `POST /inbox/check` | Check the inbox now |
| `POST /stop` | Stop the running workflow at a safe point |
| `POST /write` | File all Ready songs now |
| `GET /tracks` | All tracks with their candidates (a plain array) |
| `DELETE /tracks/{id}` | Remove an unresolved Review file (inbox or staging only) |
| `POST /tracks/retry-issues` | Repair and re-run everything in Issues |
| `POST /tracks/auto-approve` | Smart auto-select over Review (`approved`, `low_confidence`) |
| `GET /tracks/{id}/audio` | Stream the file (range requests) |
| `POST /tracks/{id}/choose` | Accept a candidate |
| `POST /tracks/{id}/review` | Undo the choice |
| `PUT /tracks/{id}/manual` | Save details typed by hand |
| `PUT /tracks/{id}/artwork` | Set and verify a cover URL or song link |
| `POST /tracks/{id}/artwork/search` | Search covers again |
| `GET /tracks/{id}/artwork/preview` | The cover that will be embedded |
| `GET /tracks/{id}/artwork/original` | The cover embedded in the file now |
| `GET /candidates/{id}/artwork/preview` | A candidate's catalog cover, through magpie-fi |
| `POST /source/resolve` | Read a song link (Shazam, Spotify, YouTube, …) into fields |
| `GET /library/recent` | The songs filed most recently |
| `GET /artists` | Every artist with song count, spellings and "looks like two" |
| `POST /artists/{id}` | Rename, join, split or confirm an artist (retags and moves its songs) |
| `GET /files?path=` | One folder of `library/` ("" is the library): folders with counts, songs |
| `GET /files/search?q=&in=` | Songs and folders matching every word, optionally inside one folder |
| `GET /files/song?path=` | Every tag of one song, read from the file, with its `version` |
| `PUT /files/song` | Write changed tags `{path, version, changes}`; returns `{changed, before, song}` |
| `PUT /files/songs` | Write several songs `{edits: [{path, version, changes}]}`; returns `{saved, failed}` |
| `GET /files/folder-tags?path=` | The songs directly in a folder with versions, and per field the shared value or a count |
| `GET /files/artwork?path=` | The cover inside one song |
| `GET /files/audio?path=` | Stream one song (range requests) |
| `POST /requests` | Read a link and queue its songs |
| `GET /requests` | The latest requests with progress counts |
| `GET /requests/{id}` | Every song of one request with its status |
| `POST /requests/{id}/retry` | Queue a request's failed songs again |

---

## 16. Key source files

| Area | File |
|---|---|
| Startup, background loops, shutdown | `src/main.rs` |
| Settings and fixed limits | `src/config.rs` |
| App state, workflow claim | `src/app/state.rs` |
| HTTP routes, login, security headers | `src/http/router.rs`, `src/http/auth.rs`, `src/http/mod.rs`, `src/http/handlers/*` |
| Inbox check and identification | `src/application/scan/` (`mod.rs`, `process.rs`, `providers.rs`, `persist.rs`) |
| Duplicates inside the inbox | `src/application/input_dedup.rs` |
| Smart auto-selection | `src/application/smart_approval.rs` |
| Metadata completion | `src/application/metadata_completion.rs` |
| Cover worker | `src/application/artwork.rs` |
| Cover finder (songs waiting for a cover) | `src/application/cover_finder.rs` |
| Filing | `src/application/apply.rs` |
| Library layout, index, trash, playlists | `src/application/library.rs` |
| Files tab (browse, search, song details, tag editing, whole folders) | `src/application/file_browser.rs`, `src/http/handlers/files.rs`, `web/src/app/components/Files.tsx` |
| Getting music from links | `src/acquire/` |
| Telegram bot | `src/telegram/` |
| Tracks and candidates in the database | `src/infrastructure/db/` |
| Integrity, fingerprint, tags, repair, ReplayGain | `src/infrastructure/media/` |
| Sources | `src/infrastructure/providers/` |
| Lyrics, caches, tool runner | `src/infrastructure/lyrics.rs`, `provider_cache.rs`, `fingerprint_cache.rs`, `process.rs` |
| Schema | `migrations/*.sql` |
| Web page | `web/src/app/App.tsx` and `web/src/app/components/` |

---

## 17. Summary flow

```
Link (web / Telegram) → wanted → already in library? → yt-dlp → .magpie-fi/staging ──┐
Telegram file → .magpie-fi/staging ──────────────────────────────────────────────────┤
Your drop → inbox (waits until it stops changing) ───────────────────────────────────┤
                                                                                     ▼
Inbox cycle (every minute)
  ├─ read tags → integrity check → fingerprint → group duplicates
  ├─ source cascade → candidates → smart auto-select
  │     ├─ sure → completion → verified cover → READY
  │     └─ unsure / unmatched → REVIEW;  corrupt → ISSUES
  └─ file every READY song: already in library? (trash, or lossless upgrade)
       → verified cover → temp copy → tags (verified) → no-clobber publish
       → original to trash → index;  failure → ISSUES
       → after the batch: rsgain per album folder → Navidrome's watcher sees the files
```
