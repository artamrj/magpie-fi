# 🐦 magpie-fi — Blueprint

> **magpie-fi** — *steals every shiny song and hides it in your own nest.*
>
> One self-hosted system that replaces Spotify / Apple Music for you and your people:
> **send anything to Telegram → it lands in Navidrome, correctly tagged, within a minute.**

This document merges two existing projects into one plan:

| Project | Repo | Language | Size | Role in the merge |
|---|---|---|---|---|
| **sync-me-maybe** v0.13.0 | `../sync-me-maybe` | Python 3.11, python-telegram-bot 22 | ~8k lines + 3.7k tests | **Gateway & Acquirer** — talks to you on Telegram and *gets* the audio |
| **ununknown** v0.6.0 | `../ununknown` | Rust (axum 0.8, sqlx, lofty) + React 19 | ~21k Rust + 6k TS | **Librarian** — *makes the audio right* and owns the library |

Contents

0. [The name](#0-the-name)
1. [What you have today](#1-what-you-have-today)
2. [Where they overlap — and where they fight](#2-where-they-overlap--and-where-they-fight)
3. [The goal of the merged project](#3-the-goal-of-the-merged-project)
4. [Architecture](#4-architecture)
5. [Music folder structure for Navidrome](#5-music-folder-structure-for-navidrome)
6. [Modern things to use (Navidrome, Telegram, tooling)](#6-modern-things-to-use)
7. [Repository layout of this folder](#7-repository-layout-of-this-folder)
8. [Roadmap](#8-roadmap)
9. [Decisions you still need to make](#9-decisions-you-still-need-to-make)

---

## 0. The name

**magpie-fi** — repo: `github.com/artamrj/magpie-fi`

A magpie is the bird that grabs every shiny thing it sees and carries it home to its nest.
This project does the same with songs. The name has three meanings:

| Read it as… | Meaning |
|---|---|
| **hi-fi** | High-quality sound: correct tags, verified covers, lyrics, ReplayGain. |
| **wi-fi** | It lives on your home network, self-hosted, owned by you. |
| **magpie-fy** (said out loud) | To grab every shiny song from everywhere (YouTube, Spotify links, Shazam, Telegram uploads, old folders) and stash it in your own nest. |

It joins the family of names: **sync-me-maybe** (the bot) + **ununknown** (the fixer) → **magpie-fi** (the whole nest).

### The secret rhyme 🐦

An old English counting rhyme says what it means to see magpies:

> *One for sorrow,*
> *Two for joy,*
> *Three for a girl,*
> *Four for a boy,*
> *Five for silver,*
> *Six for gold,*
> *Seven for a secret never to be told.*

magpie-fi hides this rhyme in the bot as an easter egg. Nobody is told; people discover it.

| Rhyme line | When the bot says it |
|---|---|
| 🐦 *One for sorrow* | A request finally failed (after all retries). |
| 🐦🐦 *Two for joy* | A song was filed into the library. |
| 🐦🐦🐦 *Three for a girl* · 🐦🐦🐦🐦 *Four for a boy* | Not mapped to anything. They stay in the rhyme only, for the full poem in `/help`. |
| 🐦🐦🐦🐦🐦 *Five for silver* | The song was saved in lossy quality (AAC/Opus from YouTube). |
| 🐦🐦🐦🐦🐦🐦 *Six for gold* | The song was saved **lossless** (FLAC). |
| 🐦🐦🐦🐦🐦🐦🐦 *Seven for a secret never to be told* | Hidden `/seven` command (not in the command menu): shows nest stats (songs, artists, hours of music, money not paid to subscriptions) and one random forgotten song from your library. |

Implementation notes:
- The line goes at the end of the bot's final status message (one short line, in italics), so it never gets in the way.
- A setting `MAGPIE_RHYME=true|false` turns it off for people who want plain messages.
- The rhyme is a traditional folk rhyme, so it is free to use.

---

## 1. What you have today

### 1.1 sync-me-maybe — "get me this song"

A Telegram bot. You send it a link or an audio file; it puts a file in your music folder.

```text
Telegram message
 → classify links (YouTube / YT Music / Spotify / Apple Music / Shazam; tracks, albums, playlists)
 → expand albums & playlists (public scraping, no API keys)
 → check its own library index (ISRC, YouTube video ID, artist+title) → skip duplicates
 → Deezer metadata (by ISRC or scored search) + YouTube Music match (title 45% / artist 35% / duration 20%)
 → yt-dlp download (m4a / opus stream copy, or mp3 320)
 → mutagen tags + embedded cover, LRCLIB synced lyrics (.lrc sidecar)
 → move to MUSIC_DIR/Album Artist/Album/01 - Title.ext
 → write Playlists/<Owner - Name>.m3u8
 → debounced Navidrome startScan (Subsonic API)
 → live status message in Telegram (Preparing → Queued → Downloading → Completed)
```

Strong points: great Telegram UX (one editable status message per request, cancel / rerun-failed buttons,
guest invite links, upload batching), a persisted queue in SQLite with retry back-off (30 s, 2 min, 10 min),
smart YouTube Music matching that penalises live/cover/remix/sped-up versions, playlists as `.m3u8`.

Weak points (from its own docs):
- **Telegram uploads are stored raw** in the root of `MUSIC_DIR` — no tags fixed, no rename, no cover.
- Metadata comes from one catalog (Deezer). Wrong album picks (compilation/soundtrack) happen.
- No audio fingerprinting, so duplicates are found by text, not by sound.
- Telegram's cloud Bot API only lets bots download files up to **20 MB**, so large FLAC uploads fail.
- No human review step — if the match is wrong, the wrong file goes straight into the library.

### 1.2 ununknown — "fix this folder"

A local web app. You point it at a messy folder; it writes corrected copies to an output folder.

```text
walk input folder
 → read tags (lofty) → decode check (ffmpeg) → Chromaprint fingerprint (fpcalc) / SHA-256
 → group duplicates by proof only (same hash, same fingerprint ±3 s, same ISRC ±3 s), keep best quality
 → provider cascade: AcoustID → YouTube ID in filename → MusicBrainz → iTunes, Deezer, Radio Javan,
   Audiomack, Navahang → Genius → SongRec/Shazam → AudD → Spotify (ISRC cross-check)
   → SoundCloud, Discogs, Last.fm, TheAudioDB, Wikidata
 → smart auto-select (weighted score ≥ 68, no different recording within 12 points, version tags must match)
 → metadata completion (only from agreeing recordings / compatible releases) + verified cover art
 → READY or REVIEW (React UI: choose candidate, manual edit, paste a song link, fix artwork)
 → write: ReplayGain → mandatory verified cover → tags (incl. MusicBrainz IDs, multi-value artists)
   → verify tags → atomic no-clobber publish (hard link) → optional remove original
 → automatic mode every 5 min: only new/changed files, only strict matches
```

Strong points: a careful identification engine (20 providers, fingerprinting, Persian catalogs),
proof-based dedup, mandatory verified artwork, ReplayGain, integrity check + repair, atomic writes,
a review UI, hardened Docker image (read-only FS, dropped capabilities, Trivy scan, multi-arch release).

Weak points:
- It has **no way to get new music** — it only fixes what is already on disk.
- Output naming is `<same subfolder as input>/Artist - Title.ext` — it keeps your mess's folder shape.
- The UI is loopback-only with no login, so you can't use it from your phone.
- It doesn't know about Navidrome (no scan trigger, no playlists, no lyrics).

### 1.3 Side by side

| Capability | sync-me-maybe | ununknown |
|---|---|---|
| Input | Telegram links + uploads | A folder on disk |
| Get new audio | ✅ yt-dlp (YouTube / YT Music) | ❌ |
| Expand playlists / albums | ✅ Spotify, Apple, YouTube | ❌ |
| Identify by sound | ❌ | ✅ AcoustID, SongRec/Shazam, AudD |
| Metadata catalogs | Deezer, iTunes, YT Music | 20 providers incl. MusicBrainz + Persian |
| Matching / scoring | 3-signal score, version penalties | Multi-signal smart approval + source agreement |
| Human review | ❌ | ✅ web UI |
| Tags | mutagen, basic set + cover | lofty, full set + MBIDs + ReplayGain, verified |
| Cover art | Deezer 1000px, unverified | Verified, release-matched, reused across album |
| Lyrics | ✅ LRCLIB `.lrc` | ❌ |
| Duplicates | Text index (ISRC / video ID / artist+title) | Proof-based (hash / fingerprint / ISRC) |
| Library layout | `Album Artist/Album/NN - Title` | `<input subfolder>/Artist - Title` |
| Playlists | ✅ `.m3u8` | ❌ |
| Navidrome | ✅ startScan, ping | ❌ |
| Queue & retries | ✅ persisted, back-off | ✅ per-track retry, workflow cancel |
| Remote access | Telegram (anywhere) | Loopback only |
| Docker | PUID/PGID, simple | PUID/PGID, read-only, cap_drop, Trivy, multi-arch |

---

## 2. Where they overlap — and where they fight

### 2.1 Duplicated work (same job, done twice)

| Area | sync-me-maybe | ununknown | Keep in the merge |
|---|---|---|---|
| Metadata lookup | `music/metadata.py` (Deezer) | `providers/deezer.rs` + 19 more | **ununknown** cascade |
| Link → metadata | `music/providers/*` (Spotify, Apple, Shazam, YouTube) | `/api/source/resolve` (Shazam, Spotify, YouTube, Genius, RJ, Navahang, Audiomack) | Bot keeps link **expansion** (playlists); single-track identity goes through the core |
| Scoring | `music/matching.py` | `smart_approval.rs`, `scan/score.rs` | **ununknown** for identity; bot keeps its YT Music *download-source* matcher |
| Tag writing | `music/tagging.py` (mutagen) | `media/tag_writer.rs` (lofty) | **ununknown** — one tag writer only |
| Cover art | Deezer URL, embed | verified worker | **ununknown** |
| Duplicate detection | `library/index.py` | `input_dedup.rs` + caches | **ununknown** becomes the one library index; bot *asks* it |
| Folder walking / indexing | `LibraryIndex.scan()` | `scan::run` | **ununknown** |
| File naming / moving | `library/storage.py` | `apply.rs::destination` + `publish_no_clobber` | **ununknown**'s safe publish, with the **new layout** (§5) |
| SQLite + queue + retry | `database.py`, `queueing/` | `db/`, workflow state | Each service keeps its own DB; core owns library state |
| Docker entrypoint (PUID/PGID drop) | `docker-entrypoint.sh` | `scripts/docker-entrypoint.sh` | ununknown's hardened version, shared |

### 2.2 Conflicts (they would break each other if you just ran both)

1. **Two writers in one music folder.** The bot moves files into the library; ununknown also publishes into a folder. Two programs renaming and deleting in the same tree = race conditions and duplicates.
2. **Two different folder layouts.** `Album Artist/Album/01 - Title.m4a` vs `Artist - Title.m4a`. Point ununknown at the bot's folder and it flattens albums.
3. **Two tag philosophies.** The bot trusts Deezer's first confident hit. ununknown would re-identify the same file, maybe pick a different album, and rewrite it.
4. **Two duplicate indexes** that disagree (text vs audio proof) → one says "new", the other says "duplicate".
5. **Two config systems.** Env vars only (bot) vs. env + settings saved in SQLite (ununknown).

### 2.3 The natural seam

The bot's biggest gap — *"Telegram uploads are not transcoded or metadata-normalized"* — is
exactly ununknown's entire purpose. And ununknown's biggest gap — *"it can't get new music and
can't be used from a phone"* — is exactly what the bot does. So the split is:

> **The bot gets audio and talks to people. The core decides what the audio is and files it.**
> Nothing but the core ever writes into the library.

---

## 3. The goal of the merged project

### Mission

**A personal music service you own.** Anything you hear — a Spotify link a friend sends, a
YouTube video, a Shazam result, a FLAC someone forwards in Telegram, an old folder of MP3s — ends
up in Navidrome once, with correct tags, cover, lyrics and loudness, playable on every device,
with no subscription.

### What "done" looks like (user stories)

1. *I send a Spotify / Apple Music / YouTube / Shazam link to the bot* → within ~1 minute the
   song is in Navidrome, in the right album folder, with cover, synced lyrics and ReplayGain.
   The bot replies with a link to play it.
2. *I send a playlist link* → each song is stored once, a Navidrome playlist appears with the
   same order, and (optionally) the bot keeps checking that playlist for new songs.
3. *I forward a FLAC or an MP3 with a garbage filename* → it gets identified by sound,
   tagged and filed — no longer dumped raw into the root.
4. *The system isn't sure what a song is* → the bot asks me with 2–3 buttons ("Is it this
   one?"), or I open the review screen inside Telegram as a Mini App.
5. *I drop an old folder into the inbox share* → it is cleaned, de-duplicated and filed
   automatically; only uncertain tracks wait for me.
6. *A friend wants to add music* → I send them a guest invite; their requests go through the
   same pipeline (and, optionally, my approval).
7. *Discovery* → once a week the system builds a "new for you" playlist from my listening
   (ListenBrainz/Last.fm), fetches what's missing, and tells me on Telegram.

### Non-goals (to keep it small and stable)

- Not a Navidrome fork. Navidrome stays a stock container; we talk to it through its APIs.
- Not a public multi-tenant service. One household, a handful of guests.
- No DRM circumvention. Spotify/Apple links are used as *search input*, like today.
- Not a Lidarr replacement for "monitor every album an artist ever releases" (can integrate later).

### Quality bar (what makes it "very good")

- **One copy per recording.** Proof-based duplicate detection across every input path.
- **Never destroy.** Atomic no-clobber writes; originals go to a trash with retention, not `rm`.
- **Never wrong silently.** Low confidence → ask the human; never guess into the library.
- **Survives restarts.** Every queue and every button persisted.
- **Boring to run.** One `compose.yaml`, health checks, automatic updates, backups.

---

## 4. Architecture

### 4.1 Principles

1. **Single writer.** Only `core` writes into `library/`. Everyone else writes into `inbox/`.
2. **Inbox → Library pipeline.** Every input (link download, Telegram upload, folder drop) becomes a
   file in `inbox/` plus an optional **hint** (what we already know). One pipeline handles all.
3. **Hints, not decisions.** When the bot downloads from a Spotify link it *knows* the ISRC,
   title, artists, duration and playlist position. It passes that as a hint. The core treats a
   hint as strong evidence (fast path, no human needed) but still verifies duration/fingerprint.
4. **Telegram is the remote control; Navidrome is the player; the web UI is the workbench.**
5. **Stock Navidrome.** Read-only mount, configured by env, talked to via Subsonic/OpenSubsonic
   API (and plugins only where they clearly help, see §6).
6. **Each service owns its data.** `core.db` = library truth; `bot.db` = Telegram conversations.

### 4.2 The picture

```text
                ┌───────────────────────── Telegram ─────────────────────────┐
                │  chats, links, audio uploads, inline buttons, Mini App      │
                └───────────────┬──────────────────────────────▲──────────────┘
                                │ updates                      │ status edits / review prompts
                       ┌────────▼──────────────────────────────┴────────┐
                       │  bot  (Python — from sync-me-maybe)             │
                       │  • Telegram UX, auth, guests                    │
                       │  • link classify + playlist/album expansion     │
                       │  • YT Music source matching + yt-dlp download   │
                       │  • Telegram file download (via local Bot API)   │
                       └───────┬──────────────────────────▲──────────────┘
           writes audio +      │ POST /api/ingest          │ SSE /api/events
           hint.json to inbox/ │ GET  /api/library/lookup  │ (progress, needs_review, filed)
                       ┌───────▼──────────────────────────┴──────────────┐
                       │  core  (Rust — from ununknown)                   │
                       │  • inbox watcher + ingest jobs                   │
                       │  • integrity → fingerprint → dedup → identify    │
                       │  • smart approval / review queue                 │
                       │  • tags, cover, lyrics (LRCLIB), ReplayGain      │
                       │  • file into library/ (layout §5), playlists     │
                       │  • Navidrome scan + song lookup                  │
                       │  • serves web UI = Telegram Mini App             │
                       └───────┬─────────────────────────────────────────┘
                               │ only writer
         ┌─────────────────────▼───────────────────────┐   Subsonic API
         │  /srv/music/library   (read-only for ND)    │◄──────────────┐
         └─────────────────────┬───────────────────────┘               │
                               │                               ┌───────┴──────┐
                               └──────────────────────────────►│  navidrome   │──► phone / desktop
                                                               └──────────────┘    clients
```

### 4.3 Flows

**A. Link (single track)**
1. Bot classifies the link, gets title/artists/duration/ISRC from the provider (as today).
2. Bot asks core `GET /api/library/lookup?isrc=…&artist=…&title=…` → already owned? reply "already in library" + play link. Done.
3. Bot matches the best YouTube Music source (its existing matcher) and downloads with yt-dlp
   to `inbox/links/<job-id>.opus` + `inbox/links/<job-id>.hint.json`.
4. Bot calls `POST /api/ingest {path, hint, requested_by, reply_to}` and listens on SSE.
5. Core: integrity → fingerprint → dedup against library → identify **with the hint as a
   first-class candidate** → completion + cover + lyrics + ReplayGain → file into `library/` →
   Navidrome scan → `filed` event with the Navidrome song ID.
6. Bot edits the status message: ✅ + "▶ Play" (Navidrome share link) / path.

**B. Playlist / album link** — bot expands to N track jobs (as today) and sends a
`playlist` hint (`{name, source_url, position}`); core writes `library/Playlists/<name>.m3u8`
with the *final* library paths after each track is filed.

**C. Telegram upload** — bot downloads the file (local Bot API server, up to 2 GB) into
`inbox/telegram/` with a hint containing only the original filename and the Telegram
`performer`/`title` fields. Core runs the full identification cascade (this fixes the
"raw uploads" gap).

**D. Folder drop** — anything copied into `inbox/drop/` (SMB/AirDrop/Syncthing) is picked
up by the core watcher; no bot involved. Result summary can be pushed to the owner on Telegram.

**E. Needs review** — core emits `needs_review {track_id, top_candidates[3], reason}`. Bot
sends one compact message: cover + "Is it one of these?" + 3 buttons + "Open review" (Mini App)
+ "Keep as-is". Answer → `POST /api/tracks/{id}/choose` → filed.

### 4.4 Why keep two languages (and not rewrite)

| Option | Cost | Verdict |
|---|---|---|
| **Python bot + Rust core** (recommended) | Two toolchains, one HTTP contract | Uses both codebases almost as-is. yt-dlp, ytmusicapi and python-telegram-bot are Python-native; the identification engine, fingerprints and tag writing are already excellent in Rust. |
| All Rust (teloxide + yt-dlp subprocess) | Rewrite ~8k lines of bot + no Rust ytmusicapi | Possible later; not worth it first. |
| All Python | Rewrite ~21k lines of hard-won Rust logic | No. |

The rule that keeps polyglot cheap: **one clear boundary** (HTTP + SSE, JSON schema shared in
`contracts/`), no shared database, each side tested on its own.

### 4.5 The contract (first version)

```http
POST /api/ingest
{ "path": "inbox/links/8f3c.opus",
  "origin": "link|upload|drop",
  "requested_by": {"telegram_user_id": 123, "role": "owner|guest"},
  "hint": {
    "source_url": "https://open.spotify.com/track/…",
    "title": "…", "artists": ["…"], "album": "…", "duration_s": 213.4,
    "isrc": "USUM71703861", "deezer_id": 123, "youtube_video_id": "…",
    "playlist": {"name": "Owner - Road Trip", "source_url": "…", "position": 7}
  } }
→ 202 { "job_id": "…" }

GET  /api/library/lookup?isrc=&youtube_video_id=&artist=&title=   → { "found": true, "path": "…", "navidrome_id": "…" }
GET  /api/events  (Server-Sent Events)
     job.progress | job.needs_review | job.filed | job.duplicate | job.failed
POST /api/tracks/{id}/choose      (already exists in ununknown)
```

Auth between bot and core: a shared secret header on the internal Docker network.
Auth for humans in the web UI: Telegram Mini App `initData` validation (see §6), so the
review UI is safe to reach from your phone without opening a port to the internet.

### 4.6 Data ownership

| Store | Owner | Contains |
|---|---|---|
| `data/core/core.sqlite` | core | library index (path, fingerprint, ISRC, MBIDs, Navidrome ID), ingest jobs, candidates, review queue, caches, playlists |
| `data/bot/bot.sqlite` | bot | Telegram requests, status messages, callback buttons, guests, playlist subscriptions |
| `music/library/` | core | the music, read-only for everyone else |
| `navidrome/data/` | navidrome | users, play counts, stars, ratings — **back this up**, it's your listening history |

---

## 5. Music folder structure for Navidrome

Navidrome groups albums and artists by **tags, not folders**. So the folder layout is for
*you*, for backups, for other tools (Picard, beets, Plex/Jellyfin later) and for never
getting lost — while the tags are what Navidrome shows. Make both clean and consistent.

### 5.1 Top-level layout (one disk / one dataset)

```text
/srv/music/                         ← one filesystem: moves & hard links are atomic
├── library/                        ← Navidrome MusicFolder (mounted :ro into Navidrome)
│   ├── Radiohead/
│   │   ├── artist.jpg
│   │   ├── OK Computer (1997)/
│   │   │   ├── cover.jpg
│   │   │   ├── 01 - Airplane.flac
│   │   │   ├── 01 - Airplane.lrc
│   │   │   └── 02 - Paranoid Android.flac
│   │   └── Creep (1992) [Single]/
│   │       ├── cover.jpg
│   │       └── 01 - Creep.m4a
│   ├── Googoosh/
│   │   └── Pol (2000)/                  ← Persian names stay in their script (NFC-normalized)
│   ├── Various Artists/
│   │   └── Pulp Fiction (1994)/
│   │       └── 01 - Dick Dale & His Del-Tones - Misirlou.m4a
│   └── Playlists/
│       └── Arta - Road Trip.m3u8        ← relative paths, auto-imported by Navidrome
├── inbox/                          ← everything new lands here; only core reads/clears it
│   ├── links/                      ← bot downloads (+ .hint.json)
│   ├── telegram/                   ← Telegram uploads
│   └── drop/                       ← SMB/Syncthing share for manual dumps
├── review/                         ← (optional) files core couldn't identify after N days
└── .trash/                         ← processed originals & replaced files, purged after 30 days
```

Why this way:
- `library/` read-only for Navidrome = Navidrome can never damage files.
- `inbox/`, `library/` and `.trash/` on **the same filesystem** — ununknown publishes with
  hard links and the bot moves with `rename`; both are atomic only inside one filesystem.
- Playlists live *inside* the music folder because Navidrome's `PlaylistsPath` is relative to
  the music folder.

### 5.2 Naming rules (the "core" writes exactly this)

```text
{AlbumArtist}/{Album} ({OriginalYear})[ [Single]|[EP]]/{Disc-}{Track:02} - {Title}.{ext}
```

| Case | Example |
|---|---|
| Normal album | `Daft Punk/Discovery (2001)/03 - Digital Love.flac` |
| Multi-disc (keep flat, prefix disc) | `Pink Floyd/The Wall (1979)/2-01 - Hey You.flac` |
| Single / EP | `Dua Lipa/Houdini (2023) [Single]/01 - Houdini.m4a` |
| Compilation / soundtrack | `Various Artists/Drive (2011)/02 - Kavinsky - Nightcall.m4a` |
| Featured artists | Only in tags (`ARTISTS`), never in folder names |
| Unknown album after review | Doesn't enter `library/` — stays in review |

Rules:
- **Singles are albums.** Replace the bot's old `Artist/Title.ext` loose files with a one-track
  release folder and `RELEASETYPE=single`. Navidrome can then show Albums / Singles / EPs separately.
- Folder artist = **album artist** (first credited if missing), never the track artist.
- Year = **original release year** (avoids the same album appearing as 2 folders after a remaster).
- Unicode **NFC** for all names (macOS writes NFD over SMB → "same" name twice).
- Replace `/ \ : * ? " < > |` and control chars; trim trailing dots/spaces; ≤180 bytes per part.
- Sidecars share the audio basename: `01 - Title.lrc`. One `cover.jpg` per album folder (also
  embedded), `artist.jpg` per artist folder.
- **Never re-encode lossy → lossy.** Keep YouTube's original Opus/AAC stream; keep FLAC as FLAC.
  Let Navidrome transcode on the fly for mobile data.

### 5.3 Tags the core writes (Navidrome-first)

| Tag | Notes |
|---|---|
| `TITLE`, `ARTIST` (display string), `ARTISTS` (multi-value) | multi-value lets Navidrome link each featured artist |
| `ALBUM`, `ALBUMARTIST`, `ALBUMARTISTS` | |
| `TRACKNUMBER`/`TRACKTOTAL`, `DISCNUMBER`/`DISCTOTAL` | |
| `DATE`, `ORIGINALDATE` | Navidrome shows original date when present |
| `RELEASETYPE` (`album`/`single`/`ep`/`compilation`), `COMPILATION=1` | |
| `GENRE` (multi-value) | from Wikidata/Last.fm/Deezer, keep a small controlled list |
| `ISRC`, `LABEL`, `LANGUAGE`, `COMPOSER` | |
| `MUSICBRAINZ_TRACKID`, `MUSICBRAINZ_ALBUMID`, `MUSICBRAINZ_ARTISTID`, `MUSICBRAINZ_ALBUMARTISTID` | Navidrome uses MBIDs for stable identity |
| `REPLAYGAIN_TRACK_GAIN/PEAK`, `REPLAYGAIN_ALBUM_GAIN/PEAK` | add **album** gain (today only track gain) |
| `ALBUMVERSION` (`Deluxe`, `2011 Remaster`, …) | part of Navidrome's default album ID — keeps editions apart instead of merging them |
| `LYRICS` (embedded) + `.lrc` sidecar | sidecar for Navidrome, embedded for portability. Navidrome 0.63+ also reads `.elrc`/`.ttml` (word-by-word karaoke) — write `.elrc` when LRCLIB reports `hasWordSync` |
| `ARTISTSORT` (Latin transliteration, optional) | nice for Persian artists in A–Z lists |
| Comment / custom `SOURCE` | where it came from (Spotify URL, Telegram upload, drop) |
| Embedded cover ≥ 1000 px JPEG | plus `cover.jpg` in the folder |

Why MusicBrainz IDs matter so much now: since the 0.55 scanner rewrite, Navidrome's album ID is
`musicbrainz_albumid`, falling back to `albumartist + album + albumversion + releasedate`. With MBIDs, an album
stays one album even after retagging or moving. Without them, a small tag difference splits one album into two.
ununknown already writes MBIDs whenever AcoustID/MusicBrainz identified the track. Keep that as a top priority.

Three Navidrome rules to respect:
- Write **ID3v2.4** (not 2.3) for MP3s so multi-value tags survive.
- **Never retag and move in the same scan.** The core files a track in one atomic step
  (tags are written *before* it enters `library/`), so this is satisfied automatically.
- The file watcher (`Scanner.WatcherWait`, on by default) usually does **not** fire on SMB/NFS mounts.
  If the library lives on a NAS share, the core's explicit `startScan` call stays necessary.

---

## 6. Modern things to use

Researched on 2026-11-09 from release pages and official docs. ⚠️ = something that changed
under your current code and needs action.

### 6.1 Navidrome (latest stable **0.64.2**, 2026-09-24)

| What's new | Since | How this project uses it |
|---|---|---|
| ⚠️ **All IDs re-encoded** to 128-bit base62 | 0.64.0 | Never store Navidrome IDs as permanent truth. Store the library path; look the ID up again (`search3`) when you need it. Back up `navidrome/data` before upgrading. |
| ⚠️ **Built-in Spotify agent removed** | 0.61 | Artist images come from Deezer/Last.fm, or from the official Apple Music plugin. Put `artist.jpg` in artist folders (core can fetch it). |
| New scanner: multi-value artists, roles, persistent IDs (MBID-first), file watcher, resumable scans | 0.55 | Folder/tag rules in §5. |
| **Multi-library** with per-user permissions | 0.58 | Optional: a separate library per person (e.g. guests' requests), or a "Persian" library. |
| Missing-files management + `navidrome missing fix` (moves plays/stars to the new file) | 0.56 / 0.64 | Used once, when the old library is re-filed into the new layout (Roadmap phase 1). |
| **WASM plugins** (Go/Rust/Python/TS; metadata, scrobbler, lyrics, scheduler, task queue, KV, Subsonic API access) | 0.57 → rewritten 0.60 | Later: a small Rust plugin (`nd-pdk`) for "request this from inside Navidrome" or "send to Telegram". Heavy work stays in core. |
| Selective scan `navidrome scan --target` | 0.59 | Not needed — Subsonic `startScan` + watcher are enough. |
| Search rebuilt on SQLite FTS5 | 0.61 | Core's `library/lookup` can ask Navidrome `search3` to confirm a song is visible. |
| Server-side transcoding decisions (OpenSubsonic `transcoding`) + concurrency limits | 0.61 / 0.62 | Store originals (Opus/AAC/FLAC); let Navidrome transcode for mobile data. |
| **Sonic similarity** via plugins (e.g. AudioMuse-AI) | 0.62 | Phase 4: "radio"/Instant Mix that sounds right, Spotify-style. |
| **Structured lyrics**: TTML, ELRC (word-by-word), SRT, LRC sidecars, OpenSubsonic `songLyrics` v2 | 0.63 | Write `.lrc`, and `.elrc` when word timing exists. |
| `EnableSharing` on by default; shares owner-scoped | 0.63 / 0.64 | The bot replies with a Navidrome **share link** so guests can play without an account. |
| Experimental **Jellyfin Music API** (Finamp, Jellify) | 0.64 | Optional: more client choice. |
| Built-in backups (`Backup.Path/Schedule/Count`) | — | Turn on. Your play counts and stars live only in Navidrome's DB. |

**Navidrome config for this project** (`config/navidrome.toml` + env):

```toml
MusicFolder = "/music"            # = /srv/music/library, mounted :ro
DataFolder  = "/data"
Scanner.Schedule     = "@every 6h" # safety net; core triggers scans after every filing
Scanner.WatcherWait  = "5s"
Scanner.PurgeMissing = "never"     # keep stats if a file moves; clean with `navidrome missing fix`
AutoImportPlaylists  = true
PlaylistsPath        = "Playlists"
LyricsPriority       = ".ttml,.elrc,.lrc,.txt,embedded"
CoverArtPriority     = "cover.*, folder.*, front.*, embedded, external"
ArtistArtPriority    = "artist.*, album/artist.*, external"
EnableSharing        = true
Agents               = "deezer,lastfm,listenbrainz"
Backup.Path          = "/backup"
Backup.Schedule      = "0 4 * * *"
Backup.Count         = 14
EnableInsightsCollector = false    # your choice; anonymous telemetry, on by default
```

**Players (what to install).** All of these talk Subsonic/OpenSubsonic to Navidrome:

| Platform | Pick | Why |
|---|---|---|
| iPhone / CarPlay | **Arpeggi** (free, word-by-word lyrics) or **Amperfy** (free, open source) · Narjo / play:Sub (paid) | offline sync, synced lyrics, CarPlay |
| Android / Android Auto | **Symfonium** (paid after trial, best offline) or **Tempus** (free, open source fork of Tempo) | offline, lyrics, Android Auto |
| Desktop + web | **Feishin** (also has a smart-playlist editor) · Supersonic | modern, lyrics |
| Both phones, free | **Substreamer** | CarPlay + Android Auto |

### 6.2 Telegram (Bot API **10.4**, released 2026-11-09)

| Feature | API | Use it for |
|---|---|---|
| ⚠️ **Self-hosted Bot API server** (`tdlib/telegram-bot-api`, image `aiogram/telegram-bot-api`) | — | Cloud bots can only **download files ≤ 20 MB** and upload ≤ 50 MB. Local server: unlimited download, 2 GB upload, and `getFile` returns a **local path** — the bot just moves the file into `inbox/telegram/`. Fixes FLAC uploads. Needs `api_id`/`api_hash` from my.telegram.org. |
| **Topics in private chats** | 9.3 / 9.4 | The bot creates topics in your DM: *Requests*, *Review*, *New in library*, *Errors*. No more one long mixed chat. |
| **Reactions** | 7.0 | React 👀 → 👍 / 🤷 on the user's message as a lightweight ack (replaces the sticker trick). |
| **Mini Apps**: fullscreen, home-screen shortcut, Secondary button, DeviceStorage/SecureStorage, `downloadFile` | 8.0 / 9.0 | The review workbench on your phone (§6.4). |
| ⚠️ **Mini App same-origin hardening** | 10.2 (enforced 2026-07-20) | Serve the whole Mini App from **one HTTPS origin** (the core). No cross-origin iframes or redirects. |
| `sendMessageDraft` / rich-message drafts (streamed text) | 9.3 / 9.5 / 10.1 | Optional: live "Downloading 43%…" without many edits. Plain edits throttled to every 2.5 s (what you do now) are fine. |
| **Rich messages** (`sendRichMessage`, audio/table/list blocks) | 10.1–10.3 | Album / playlist result cards with a track table. |
| **Disabled inline buttons**, ephemeral callback replies | 10.3 / 10.4 | Grey out "Rerun failed" while it runs. Show "Not authorized" only to the person who pressed. |
| **Button colours & custom-emoji icons** | 9.4 | Green "✓ This one" / red "✗ None" in review prompts. |
| `sendAudio` (MP3/M4A, performer/title/320 px thumb) | — | "📲 Send me the file" button: the song from your library, with cover, in Telegram's own player. |
| **Profile music** — `getUserProfileAudios` | 9.4 | Fun extra: "/import_profile" pulls songs from a user's Telegram profile playlist into the library. |
| Deep links `t.me/<bot>?startapp=review_<id>` | — | Notifications open the Mini App directly on the right track. |

**Library choice.** **python-telegram-bot 22.8** (June 2026) supports up to Bot API **10.0**, and the
10.1+ work (rich messages, widgets) hasn't landed yet. **aiogram 3.31** (Aug 2026) tracks the API
within days (10.3). Recommendation: **stay on PTB for now.** Your queue, persistence and handler
code are built on it. Call 10.1+ methods through `Bot.do_api_request()` when you want them.
Move to aiogram only if you start leaning heavily on rich messages or widgets.

**Python.** Move the bot from 3.11 → **3.14** (3.13 is now security-only; 3.15 is too new for
some libraries).

### 6.3 Getting audio (acquisition)

| Item | State in Nov 2026 | Action |
|---|---|---|
| **yt-dlp 2026.10.26** | YouTube needs a JS runtime (Deno ✓ you have it) **and increasingly PO tokens** (SABR streaming) | Add the **`bgutil-ytdlp-pot-provider` 2.0.2** sidecar (compose profile) + plugin. Pin yt-dlp + `yt-dlp-ejs` and let Renovate auto-merge updates fast — YouTube breaks things every few weeks. |
| Formats | 251 = Opus ~130–160 kbps (best free), 140 = AAC 128, 141/774 = Premium only (cookies) | Keep stream copy (you do). Never re-encode lossy→lossy. |
| **ytmusicapi 1.12.3** | Works, unofficial | Keep `filter="songs"` matching. |
| ⚠️ **Spotify Web API** | Since Feb/Mar 2026 Dev Mode: app owner **must have Premium**, max 5 users, search limit 10, **playlist items only for playlists you own/collaborate on**, batch endpoints removed | The bot's tokenless public-page scraping is now the *only* way to read other people's playlists. Keep it, and add tests that alert when it breaks. In core, make the Spotify provider optional; prefer Deezer/MusicBrainz by ISRC. |
| iTunes Search/Lookup | ~20 req/min/IP, no key | Cache aggressively (core already has `provider_cache`). |
| Deezer public API | No key, rate-limited | Primary ISRC → metadata bridge. |
| **slskd 0.26** (Soulseek) | Active, REST API | *Optional* lossless source (compose profile). Verify with fingerprint + duration + a check for fake FLACs (lossy files re-encoded as FLAC). ⚖️ This is file sharing, so the legal risk is yours. |
| Tidal/Qobuz rippers (streamrip, OrpheusDL, Octo-Fiesta) | Exist | ⚖️ Break those services' terms; accounts get banned. Not in scope. |
| Lidarr | v3.1.6, plugins (Tubifarry) | Not a dependency: its metadata server was down Apr–Aug 2025. Your pipeline already does what you'd need from it. |

### 6.4 Identification, tags, audio

| Item | Version | Note |
|---|---|---|
| Chromaprint / fpcalc | 1.6.x (FFmpeg 8 support) | ✓ already in the ununknown image |
| AcoustID | 3 req/s, **non-commercial only** | fine for personal use |
| MusicBrainz API | 1 req/s, descriptive User-Agent mandatory | ✓ already rate-limited in core |
| SongRec (Shazam) | 0.8.1 | unofficial endpoint — fallback only (as now) |
| **LRCLIB** | `/api/get` (duration ±2 s), new `hasWordSync`, 90 req/min with a User-Agent | move lyrics from bot → core; send a real User-Agent |
| **rsgain 3.8** | ReplayGain 2.0, `rsgain easy` per album folder | Add **album gain**. Core currently measures track gain only. Run `rsgain easy` on the album folder after filing. Writes R128 tags for Opus correctly. |
| FFmpeg | 9.0.x (8.1 still maintained) | either |
| **lofty 0.25.5** | Rust tags | Bump from 0.24 (pre-1.0, minor API breaks) |
| mutagen 1.48.1 | Python tags | will no longer be needed in the bot once core owns tagging |
| MusicBrainz Picard **3.0.4** | Qt6, new plugin system | manual fallback for weird albums |
| beets 2.14.1 | CLI tagger | reference only; album-centric matcher doesn't fit "one song at a time" |
| AudioMuse-AI 3.7 | sonic analysis + Navidrome plugin | Phase 4 "radio" |

### 6.5 Code & platform stack

| Layer | Choice | Why |
|---|---|---|
| Core | **Rust 2024, axum 0.8, sqlx 0.9, tokio, lofty 0.25** | what ununknown already runs |
| Bot | **Python 3.14 + uv**, python-telegram-bot 22.8, `httpx` (replace `requests`) | async HTTP everywhere; one less thread hop |
| Web / Mini App | **React 19 + Vite 8** (already) + **TanStack Router/Query** + **Tailwind v4 + shadcn/ui** + **`@tma.js/sdk`** (3.3; 4.0 in RC) | one SPA, two modes: desktop workbench and Telegram Mini App |
| Mini App auth | Validate `initData` HMAC (key = HMAC_SHA256("WebAppData", bot_token)) in core; send it as `Authorization: tma <initData>` | no passwords. Telegram user ID → owner/guest role |
| Contract | `contracts/openapi.yaml` → generated TS types for web, a pydantic client for bot | the two languages can't drift apart |
| DB | SQLite per service, WAL, `synchronous=NORMAL`, `busy_timeout`, single writer, **on SSD not on the NAS share** | proven at this scale; Navidrome itself had locking bugs on slow storage until 0.64.2 |
| Jobs | Keep your own SQLite job tables (both projects already have them) | Huey/SAQ/Procrastinate/Temporal add infra for nothing here |
| Logs | JSON logs (`structlog` / `tracing` JSON) with a shared `job_id` across bot ↔ core | follow one song end-to-end |
| Containers | Compose v2+ syntax `compose.yaml`, healthchecks + `depends_on: service_healthy`, **profiles** for optional sidecars, read-only FS + `cap_drop` (ununknown style) for all services | |
| Images | multi-arch amd64/arm64, GHCR, Trivy (ununknown's release workflow, reused for both) | |
| Updates | **Renovate** (Docker + Cargo + uv + npm in one place), auto-merge yt-dlp / yt-dlp-ejs / bgutil when tests pass | |
| Remote access | **Tailscale** for players (Navidrome over your tailnet). **Tailscale Funnel or Cloudflare Tunnel** only for the Mini App HTTPS origin. ⚠️ Don't stream audio through Cloudflare (their terms) | |
| Backups | **restic + Backrest** UI. `sqlite3 .backup` before snapshot (or Litestream). Back up: both DBs, `navidrome/data`, `config/`, and the library | |
| Hardware | Intel N100/N150 mini-PC (fingerprinting + rsgain on big backlogs) or Pi 5 8 GB + NVMe | |

---

## 7. Repository layout of this folder

A **monorepo**. Both old repos are imported **with their git history**. Use
`git filter-repo --to-subdirectory-filter core` on a clone of ununknown and `… bot` on a clone of
sync-me-maybe, then merge them into this repo with `--allow-unrelated-histories`.

```text
magpie-fi/                       ← today this folder is still called navidrome-arta
├── BLUEPRINT.md                 ← this file
├── README.md                    ← "how to run it" (short)
├── compose.yaml                 ← navidrome + core + bot + tgapi  (profiles: pot, slskd, backup)
├── .env.example
├── renovate.json
├── config/
│   └── navidrome.toml
├── contracts/
│   ├── openapi.yaml             ← core HTTP API: source of truth
│   └── hint.schema.json         ← what bot may tell core about a file
│
├── core/                        ← Rust, from ununknown (librarian)
│   ├── Cargo.toml  Dockerfile  migrations/
│   └── src/
│       ├── domain/              (matcher, credits, genre, audio — unchanged)
│       ├── application/
│       │   ├── scan/ smart_approval.rs metadata_completion.rs artwork.rs apply.rs   (existing)
│       │   ├── ingest/          NEW  inbox watcher, ingest jobs, hint → candidate
│       │   ├── library/         NEW  layout (§5), library index, playlists, lyrics, album gain
│       │   └── events.rs        NEW  SSE event bus
│       ├── infrastructure/
│       │   ├── providers/       (existing 20 providers + lrclib.rs NEW)
│       │   ├── media/           (existing + rsgain.rs NEW)
│       │   └── navidrome.rs     NEW  Subsonic client: startScan, search3, createShare
│       └── http/
│           ├── handlers/ingest.rs library.rs events.rs   NEW
│           └── auth.rs          NEW  service token + Telegram initData
│
├── web/                         ← React, from ununknown/frontend (workbench + Mini App)
│   └── src/ app/ api/(generated) routes/ telegram/
│
├── bot/                         ← Python, from sync-me-maybe (gateway + acquirer)
│   ├── pyproject.toml  Dockerfile
│   └── src/magpie_bot/
│       ├── telegram/            (handlers, commands, callbacks, uploads, requests, safe_api, persistence)
│       ├── ui/                  (messages, keyboards)
│       ├── acquire/             (urls, providers/, matching.py, downloader.py)
│       ├── queueing/            (unchanged)
│       └── core_client.py       NEW  ingest, lookup, SSE listener
│
├── navidrome-plugin/            ← later (Rust nd-pdk)
├── scripts/
│   ├── migrate-library.sh       ← dry-run → apply re-filing of the old library
│   └── backup.sh
├── docs/
│   └── adr/                     ← one short file per big decision (0001-single-writer.md, …)
└── .github/workflows/           ← core.yml, bot.yml, web.yml (path-filtered), release.yml
```

**What moves where**

| From sync-me-maybe | Fate |
|---|---|
| `telegram_bot/*`, `ui/*`, `queueing/*`, `auth.py`, `config.py` | **keep** → `bot/` |
| `music/urls.py`, `music/providers/*`, `music/matching.py`, `music/downloader.py` | **keep** → `bot/acquire/` |
| `music/metadata.py` (Deezer), `music/enrichment.py` | shrink: canonical metadata comes from core `/api/source/resolve` |
| `music/tagging.py`, `music/lyrics.py`, `music/filenames.py` | **delete** → core does it |
| `library/index.py`, `library/storage.py`, `library/playlists.py`, `library/navidrome.py` | **delete** → core `application/library/` + `infrastructure/navidrome.rs` |

| From ununknown | Fate |
|---|---|
| Everything in `src/` | **keep** → `core/` |
| "Rows deleted after write" | ⚠️ **change**: keep a permanent `library_items` table (path, fingerprint, ISRC, MBIDs, source, added_by). That table becomes the one duplicate index for everything. |
| `destination()` → `<input subfolder>/Artist - Title` | **replace** with §5 layout |
| Loopback-only, no auth | **replace** with service token + Telegram initData; still never exposed without HTTPS |
| Settings in SQLite + env | keep, env wins (as now) |

---

## 8. Roadmap

Each phase is useful on its own. You can stop after any of them.

### Phase 0 — "Glue mode": works this weekend, almost no code ✅ built
- Monorepo with both histories (`core/` = ununknown, `bot/` = sync-me-maybe), one `compose.yaml`
  with **navidrome + bot + core**. Root CI per component, one release workflow publishing
  `magpie-fi-core` and `magpie-fi-bot` images.
- Bot `MUSIC_DIR` = `MUSIC_ROOT/inbox` (the inbox **root**: ununknown keeps the input's subfolders,
  so `inbox/links/…` would have become `library/links/…`). Bot temp folder = `inbox/.incoming`, so
  finished files appear with one atomic rename.
- Core input = `inbox`, output = `library`, automatic filing every 2 min, "remove input after output"
  on. Two small core changes made this possible: the env overrides `UNUNKNOWN_DELETE_SOURCE_AFTER_WRITE`,
  `UNUNKNOWN_AUTOMATIC_SCAN_ENABLED`, `UNUNKNOWN_AUTOMATIC_SCAN_INTERVAL_MINUTES`, and skipping
  `.hidden` / `@nas` folders while scanning.
- Navidrome 0.64.2 reads `library/` read-only, config in `config/navidrome.toml`.
- ✅ Done when a Spotify link sent to the bot appears in Navidrome after the next automatic cycle.
- Known gaps (listed in README): bot says "done" before filing; naming is `Album Artist/Album/Artist - Title`;
  uploads land in the library root; bot `.m3u8` playlists stay in the inbox; lyrics off; bot forgets songs
  once they leave the inbox; 20 MB Telegram download limit.

### Phase 1 — Core becomes the librarian
- Permanent `library_items` table. Library layout from §5. Album folders, `cover.jpg`, `artist.jpg`.
- Move lyrics (LRCLIB), `.m3u8` playlists and the Navidrome client into core. Add rsgain album gain.
- `POST /api/ingest`, `GET /api/library/lookup`, `GET /api/events` (SSE). Hint → first-class candidate.
- `scripts/migrate-library.sh`: plan (dry-run report) → apply re-filing of your existing library →
  `navidrome missing fix` to keep play counts/stars.
- ✅ Done when the same song arriving from a link, an upload and a folder drop ends up as **one**
  file at the §5 path, and Navidrome shows it with cover, lyrics and gain.

### Phase 2 — Bot becomes the thin gateway
- Delete bot tagging/storage/index. Bot calls `lookup` before downloading and `ingest` after.
- Status message follows core events: …Downloading → Identifying → **In library ▶ Play** (share link).
- `needs_review` → compact prompt with 3 candidate buttons (coloured, 9.4) + "Open review".
- Run the **local Bot API server** (2 GB files). Use DM topics for Requests / Review / Library news.
- Python 3.14, `httpx`, Renovate.
- ✅ Done when a 300 MB FLAC album forwarded in Telegram ends up correctly filed, and a doubtful
  song gets resolved with one tap.

### Phase 3 — The workbench in your pocket
- The React app runs as a **Telegram Mini App** (`@tma.js/sdk`, theme variables, Main/Secondary buttons,
  fullscreen) on one HTTPS origin via Tailscale Funnel or Cloudflare Tunnel. initData auth in core.
- Review queue, manual edit, artwork fix and "recently added" all on your phone. The desktop browser keeps working.
- ✅ Done when you can clear the whole review queue from your phone without opening a laptop.

### Phase 4 — Better than a subscription
- **Playlist subscriptions**: `/follow <playlist link>` re-checks daily and fetches new songs. The Navidrome
  playlist is kept in the same order.
- **Discovery**: Navidrome → ListenBrainz scrobbling. Weekly, fetch "Weekly Exploration" tracks you don't
  own (the Explo idea, but through *your* pipeline) → Navidrome playlist "New for you" → Telegram note.
- **Sonic radio**: AudioMuse-AI + Navidrome sonic-similarity plugin.
- "📲 Send me the file" (`sendAudio`), `/import_profile` (Telegram profile music), guest share links.
- Optional `slskd` profile for lossless with fake-FLAC detection.
- Optional small **Navidrome plugin** (Rust) so you can request songs from inside your player.

### Phase 5 — Boring operations (do alongside)
- Healthchecks on every service, Backrest/restic nightly, Navidrome built-in backup, SQLite `.backup`.
- Renovate fast lane for yt-dlp + EJS + PO-token provider. Scraper canary tests (Spotify/Apple public pages)
  that alert you on Telegram when a provider breaks.
- JSON logs with `job_id`. `/health` in Telegram shows all services.

---

## 9. Decisions you still need to make

| # | Question | My recommendation |
|---|---|---|
| 1 | Keep Python + Rust, or rewrite to one language? | **Keep both** with the HTTP contract (§4.4). Revisit only if maintaining two toolchains really hurts. |
| 2 | python-telegram-bot or aiogram? | **Stay on PTB 22.x**; use raw `do_api_request` for 10.1+ features. |
| 3 | Default format for YouTube downloads | **m4a (AAC)** if your household uses iPhones/CarPlay (best compatibility); **opus** otherwise (smaller, slightly better). Never mp3. |
| 4 | Should guest requests go straight into the library? | Into the library, but **always through review** if confidence isn't high. Or ask the owner first (a setting). |
| 5 | Re-file your existing library into the new layout? | **Yes, once**, with the dry-run report first and `navidrome missing fix` after. Keep a full backup first. |
| 6 | Library on the NAS share or local disk? | Music can live on the NAS; **databases on local SSD**. Inbox, library and trash on the **same** filesystem. |
| 7 | Remote access | Tailscale for listening; one HTTPS origin (Funnel/Tunnel) only for the Mini App. |
| 8 | Lossless via Soulseek? | Optional profile, off by default. The legal risk is yours. |
| 9 | Persian music naming | Tags in original script, `ARTISTSORT` in Latin; folders follow the album-artist tag (NFC). ununknown's "prefer Latin" canonical-name setting becomes a per-library option. |
| 10 | Project name | ✅ **Decided: `magpie-fi`** (§0). Names inside the code: Rust crate `magpie-core`, Python package `magpie_bot`, env prefix `MAGPIE_`, images `ghcr.io/artamrj/magpie-fi-core` / `-bot` / `-web`. |
