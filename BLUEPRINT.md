# 🐦 magpie-fi — Blueprint

> **magpie-fi** — *steals every shiny song and hides it in your own nest.*
>
> One self-hosted system that replaces Spotify / Apple Music for you and your people:
> **send anything to Telegram → it lands in Navidrome, correctly tagged, within a minute.**

This document merges two existing projects into **one program, one image**:

| Project | Repo | Language | Size | Role in the merge |
|---|---|---|---|---|
| **ununknown** v0.6.0 | `../ununknown` | Rust (axum 0.8, sqlx, lofty) + React 19 | ~21k Rust + 6k TS | **The foundation.** magpie-fi grows out of this code |
| **sync-me-maybe** v0.13.0 | `../sync-me-maybe` | Python 3.11, python-telegram-bot 22 | ~8k lines + 3.7k tests | **The donor.** Its Telegram experience and "get the song" logic are rewritten in Rust, then the Python code is deleted |

**In one sentence:** magpie-fi is one Rust program in one Docker image. Navidrome runs next to it,
and one `docker compose up` starts both (§4).

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
9. [Decisions](#9-decisions)

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

| Area | sync-me-maybe | ununknown | In magpie-fi |
|---|---|---|---|
| Metadata lookup | `music/metadata.py` (Deezer) | `providers/deezer.rs` + 19 more | ununknown's 20-source cascade |
| Link → metadata | `music/providers/*` (Spotify, Apple, Shazam, YouTube) | `/api/source/resolve` (Shazam, Spotify, YouTube, Genius, RJ, Navahang, Audiomack) | One link reader in `acquire/` (ported bot scrapers + ununknown's resolvers) |
| Scoring | `music/matching.py` | `smart_approval.rs`, `scan/score.rs` | One scorer: ununknown's, plus the bot's version penalties |
| Tag writing | `music/tagging.py` (mutagen) | `media/tag_writer.rs` (lofty) | ununknown's (lofty) |
| Cover art | Deezer URL, embed | verified worker | ununknown's verified worker |
| Duplicate detection | `library/index.py` | `input_dedup.rs` + caches | One permanent library index |
| Folder walking / indexing | `LibraryIndex.scan()` | `scan::run` | ununknown's |
| File naming / moving | `library/storage.py` | `apply.rs::destination` + `publish_no_clobber` | ununknown's safe publish, with the **new layout** (§5) |
| SQLite + queue + retry | `database.py`, `queueing/` | `db/`, workflow state | One database, one job queue |
| Docker entrypoint (PUID/PGID drop) | `docker-entrypoint.sh` | `scripts/docker-entrypoint.sh` | One image, ununknown's hardened style |

### 2.2 Conflicts (they would break each other if you just ran both)

1. **Two writers in one music folder.** The bot moves files into the library; ununknown also publishes into a folder. Two programs renaming and deleting in the same tree = race conditions and duplicates.
2. **Two different folder layouts.** `Album Artist/Album/01 - Title.m4a` vs `Artist - Title.m4a`. Point ununknown at the bot's folder and it flattens albums.
3. **Two tag philosophies.** The bot trusts Deezer's first confident hit. ununknown would re-identify the same file, maybe pick a different album, and rewrite it.
4. **Two duplicate indexes** that disagree (text vs audio proof) → one says "new", the other says "duplicate".
5. **Two config systems.** Env vars only (bot) vs. env + settings saved in SQLite (ununknown).

### 2.3 The natural seam

The bot's biggest gap — *"Telegram uploads are not transcoded or metadata-normalized"* — is
exactly ununknown's entire purpose. And ununknown's biggest gap — *"it can't get new music and
can't be used from a phone"* — is exactly what the bot does. So they belong together:

> **Getting the song and talking to people** (the bot's strength) and **deciding what the song is
> and filing it** (ununknown's strength) become two modules of **one program**.
> Only the library module ever writes into the library.

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

- Not a Navidrome fork. Navidrome stays its own official container next to magpie; we talk to it through its API.
- Not a public multi-tenant service. One household, a handful of guests.
- No DRM circumvention. Spotify/Apple links are used as *search input*, like today.
- Not a Lidarr replacement for "monitor every album an artist ever releases" (can integrate later).

### Quality bar (what makes it "very good")

- **One copy per recording.** Proof-based duplicate detection across every input path.
- **Never destroy.** Atomic no-clobber writes; originals go to a trash with retention, not `rm`.
- **Never wrong silently.** Low confidence → ask the human; never guess into the library.
- **Survives restarts.** Every queue and every button persisted.
- **One of everything.** One image, one database, one config, one queue. No job is done twice.
- **Boring to run.** One `compose.yaml`, health checks, automatic updates, backups.

---

## 4. Architecture

> **Decision (2026-11-10):** magpie-fi is **one program, in one language (Rust), shipped as one
> Docker image.** The Python bot is ported into it and then deleted. Navidrome runs next to it as
> its own official image, in the same `compose.yaml`, so one `docker compose up` starts everything.

### 4.1 Principles

1. **One of everything.** One program, one image, one database, one config (`MAGPIE_*`), one job
   queue, one tag writer, one duplicate index, one link reader, one scorer. Every job is done in
   exactly one place.
2. **Single writer.** Only the `library` module writes into `library/`.
3. **One pipeline for every input.** A link, a Telegram upload, a file dropped in `inbox/` and a
   link pasted in the web page all become the same thing — an **item** — and go through the same
   stages (§4.3).
4. **What you asked for is evidence.** A Spotify link already tells us the ISRC, title, artists and
   duration. That "wanted" information becomes a first-class candidate during identification, so
   most link downloads are filed without questions — but still checked against duration and fingerprint.
5. **Ask, don't guess.** If magpie isn't sure, the item waits for a human. Nothing is filed silently wrong.
6. **Telegram is the remote control, Navidrome is the player, the web page is the workbench**
   (and the same page opens inside Telegram as a Mini App).
7. **Stock Navidrome.** Read-only music mount, talked to only through its API.
8. **Outside tools are bundled programs, not a second language.** ffmpeg, fpcalc, songrec, rsgain,
   yt-dlp and Deno ship inside the image and are called like ununknown already calls ffmpeg.

### 4.2 The picture

```text
                 Telegram  (links · uploads · buttons · Mini App)
                                │          ▲
╔═══════════════════════════════╪══════════╪═════════════════════════════╗
║  magpie  — one Rust program   ▼          │ status + review prompts     ║
║                                                                        ║
║   telegram ─────►  jobs: one queue  ◄───── http: web page, Mini App,   ║
║                  (requests · items)        API, live updates, login    ║
║                          │                                             ║
║          ┌───────────────┼──────────────────┐                          ║
║          ▼               ▼                  ▼                          ║
║      acquire  ──────► identify  ──────►  library  ──────► navidrome    ║
║      links,           fingerprint,       tags, cover,     client:      ║
║      playlists,       dedup, 20          lyrics, gain,    scan, find,  ║
║      YT Music,        sources,           §5 layout,       share        ║
║      yt-dlp           scoring, review    trash                         ║
║                                                                        ║
║   bundled tools: ffmpeg · fpcalc · songrec · rsgain · yt-dlp · deno    ║
╚════════════════════════════════════════════╪═══════════════════╪═══════╝
                                    only writer│        Subsonic API│
                                               ▼                    ▼
                                    MUSIC_ROOT/library ──read-only──► navidrome ──► your apps
```

### 4.3 The pipeline

Every item goes through these stages. Each stage is a durable job in the database.

| # | Stage | What happens | Skipped for |
|---|---|---|---|
| 1 | **Expand** | A playlist/album link becomes N *wanted* tracks, plus a playlist record (name, order, source) | single tracks, uploads, drops |
| 2 | **Already have it?** | Library index lookup by ISRC, source ID (YouTube video ID…) and normalised artist + title. Hit → reply "already in your library ▶ Play" | uploads, drops (checked by fingerprint in stage 4) |
| 3 | **Get** | Pick the best YouTube Music source and download with yt-dlp, or download the Telegram file, into `.magpie/staging/` | drops (already on disk) |
| 4 | **Identify** | Decode check → fingerprint → duplicate check against the library (proof only) → candidates from *wanted* + the 20 sources → smart scoring | — |
| 5 | **Decide** | Sure → stage 6. Not sure → **review**: Telegram prompt with 3 candidate buttons + "Open review", and the web review list | — |
| 6 | **File** | Tags, verified cover, lyrics, ReplayGain (track + album) → atomic no-clobber move into `library/` (§5) → original to `.magpie/trash/` → update playlists | — |
| 7 | **Publish** | Debounced Navidrome scan → find the song's Navidrome ID → share link → "✅ In your library ▶ Play" to whoever asked | — |

How each input enters:

| Input | Enters at | Requester |
|---|---|---|
| Link sent to the bot | 1 (or 2 for a single track) | that Telegram user |
| Audio file sent to the bot | 3 (Telegram download) | that Telegram user |
| File copied into `inbox/` (SMB, Syncthing…) | 4, once the file has stopped growing | owner (summary on Telegram) |
| Link pasted in the web page | 1 / 2 | the logged-in web user |
| "Follow playlist" check (Phase 5) | 1, only for new songs | the follower |

### 4.4 Why one Rust program — and how the Python-only parts are replaced

Rust, because ununknown is the larger and more careful half (21k lines: fingerprints, scoring,
safe file moves), a single Rust binary uses little memory on a NAS or Pi, and the compiler protects
a program whose job is moving your files. The bot is the smaller half and is mostly Telegram
screens and link scrapers — the easy part to port.

| Needed | In the Python bot | In magpie |
|---|---|---|
| Downloading from YouTube | `yt-dlp` Python library | The official standalone **`yt-dlp` program** inside the image, run as a tool. magpie checks for a new yt-dlp every night and keeps it in `DATA_ROOT/bin`, so YouTube fixes arrive without waiting for a new magpie image. |
| YouTube Music search | `ytmusicapi` | yt-dlp's own YouTube Music search (`music.youtube.com/search?q=…#songs`) returns songs with duration; magpie's matcher scores them. *(Check the exact output in Phase 2.)* |
| Telegram | `python-telegram-bot` | **`frankenstein`** crate (0.52.1, Sept 2026; follows the Bot API closely) + a small router of our own. magpie already has its own queue and state, so a big bot framework adds nothing. (`teloxide` was considered: last release July 2025, behind the Bot API.) |
| Tags | `mutagen` | `lofty` (already used) |
| Spotify / Apple page reading | `requests` + BeautifulSoup | `reqwest` + `scraper` + `serde_json` |

**Cost:** about 5,500 Python lines get ported (≈3,300 Telegram + messages, ≈2,200 links, playlists,
matching, download). About 2,400 lines are simply deleted because the Rust side already does that
job better (tagging, storage, duplicate index, metadata, queue, config).

### 4.5 Requests, jobs, items — how state works

| Thing | What it is | Example |
|---|---|---|
| **request** | One thing one person asked, once | "this Spotify playlist", sent by Arta at 21:03 |
| **item** | One song on its way: staging path, *wanted* info, fingerprint, candidates, decision, final path | track 7 of that playlist |
| **job** | One durable unit of work for an item or request: kind, state, attempts, `not_before` | `acquire` item 7, attempt 2 |

- **Retries** with back-off 30 s → 2 min → 10 min (from the bot). Permanent errors are not retried.
- **Limits per resource** (from ununknown): downloads 2, fingerprints 3, lookups 3, tag writes 2,
  SongRec 2, MusicBrainz 1 request/s, plus a polite pause between YouTube downloads.
- **One event bus.** Every state change becomes an event. Two listeners render it: the Telegram
  status message (edited at most every 2.5 s, final state immediately — from the bot) and the web
  page (live updates).
- **Survives restarts.** Jobs continue where they stopped; Telegram buttons keep working for 30 days.

### 4.6 Web page, Mini App and login

- One axum server. The React page is compiled **into** the binary, so everything comes from one
  origin — which Telegram's Mini App rules (Bot API 10.2) require.
- **Login is Telegram, everywhere.** In the Mini App, magpie checks Telegram's signed `initData`.
  On a computer, send `/web` to the bot and it replies with a one-time login link. No passwords;
  the same owner/guest roles as in the bot.
- Published on `127.0.0.1` / your Tailscale network by default. A public HTTPS address
  (Tailscale Funnel or Cloudflare Tunnel) is only needed for the Mini App.

### 4.7 Data and config

| Where | What |
|---|---|
| `DATA_ROOT/magpie.sqlite` | **the one database**: library index, requests, items, jobs, candidates, review queue, playlists, follows, Telegram buttons, guests, provider cache |
| `DATA_ROOT/bin/` | auto-updated `yt-dlp` |
| `MUSIC_ROOT/library/` | your music (only magpie writes, Navidrome reads) |
| `MUSIC_ROOT/inbox/` | your drop zone |
| `MUSIC_ROOT/.magpie/` | `staging/` (work in progress) and `trash/` (originals, kept 30 days) |
| `DATA_ROOT/navidrome/` | Navidrome's database: play counts, stars — **back this up** |

**Config = environment only** (`.env`, all keys start with `MAGPIE_`). Nothing is configured in
the database, so there is one place to look. The web page *shows* the configuration and which
sources are active, but doesn't edit secrets.

### 4.8 What is inside the image

`ghcr.io/artamrj/magpie-fi` — linux/amd64 + linux/arm64, Debian slim base (yt-dlp and Deno need
glibc; ffmpeg and Chromaprint come from Debian packages):

- `magpie` — the program, with the web page built in
- `ffmpeg`/`ffprobe`, `fpcalc` (Chromaprint), `songrec`, `rsgain`, `yt-dlp`, `deno`
- `tini`; starts as root only to drop to `PUID:PGID`; read-only root filesystem

`compose.yaml` = **magpie + navidrome**. Optional helpers, off by default, each its own official image:

| Profile | Image | Turn on when |
|---|---|---|
| `tgapi` | `aiogram/telegram-bot-api` | you send files bigger than 20 MB (FLAC) — raises the limit to 2 GB |
| `pot` | `bgutil-ytdlp-pot-provider` | YouTube starts refusing downloads without a PO token |
| `slskd` | `slskd/slskd` | you want lossless from Soulseek (⚖️ your legal risk) |

---

## 5. Music folder structure for Navidrome

Navidrome groups albums and artists by **tags, not folders**. So the folder layout is for
*you*, for backups, for other tools (Picard, beets, Plex/Jellyfin later) and for never
getting lost — while the tags are what Navidrome shows. Make both clean and consistent.

### 5.1 Top-level layout (one disk / one dataset)

```text
/srv/music/                         ← MUSIC_ROOT. One filesystem: moves are atomic
├── library/                        ← Navidrome MusicFolder (mounted :ro into Navidrome)
│   ├── Radiohead/
│   │   ├── artist.jpg
│   │   ├── OK Computer (1997)/
│   │   │   ├── cover.jpg
│   │   │   ├── 01 - Airbag.flac
│   │   │   ├── 01 - Airbag.lrc
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
├── inbox/                          ← YOUR drop zone (SMB, Syncthing, AirDrop…). magpie empties it
└── .magpie/                        ← magpie's own work area (hidden; Navidrome never sees it)
    ├── staging/                    ← downloads and Telegram uploads being identified, or waiting in review
    └── trash/                      ← originals and replaced files, deleted after 30 days
```

Why this way:
- `library/` is read-only for Navidrome, so Navidrome can never damage files.
- `inbox/` is only for you. Downloads and uploads go straight to `.magpie/staging/`, so your drop zone
  never fills with half-finished files.
- `library/`, `inbox/` and `.magpie/` sit on **the same filesystem**, so every move is one atomic rename.
- Nothing is deleted right away: originals go to `.magpie/trash/` first.
- Playlists live *inside* the music folder because Navidrome's `PlaylistsPath` is relative to it.

### 5.2 Naming rules (magpie writes exactly this)

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

### 5.3 Tags magpie writes (Navidrome-first)

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
- **Never retag and move in the same scan.** magpie files a track in one atomic step
  (tags are written *before* it enters `library/`), so this is satisfied automatically.
- The file watcher (`Scanner.WatcherWait`, on by default) usually does **not** fire on SMB/NFS mounts.
  If the library lives on a NAS share, magpie's explicit `startScan` call stays necessary.

---

## 6. Modern things to use

Researched on 2026-11-09 from release pages and official docs. ⚠️ = something that changed
under your current code and needs action.

### 6.1 Navidrome (latest stable **0.64.2**, 2026-09-24)

| What's new | Since | How this project uses it |
|---|---|---|
| ⚠️ **All IDs re-encoded** to 128-bit base62 | 0.64.0 | Never store Navidrome IDs as permanent truth. Store the library path; look the ID up again (`search3`) when you need it. Back up `navidrome/data` before upgrading. |
| ⚠️ **Built-in Spotify agent removed** | 0.61 | Artist images come from Deezer/Last.fm, or from the official Apple Music plugin. magpie puts `artist.jpg` in artist folders. |
| New scanner: multi-value artists, roles, persistent IDs (MBID-first), file watcher, resumable scans | 0.55 | Folder/tag rules in §5. |
| **Multi-library** with per-user permissions | 0.58 | Optional: a separate library per person (e.g. guests' requests), or a "Persian" library. |
| Missing-files management + `navidrome missing fix` (moves plays/stars to the new file) | 0.56 / 0.64 | Used once, when the old library is re-filed into the new layout (Roadmap phase 1). |
| **WASM plugins** (Go/Rust/Python/TS; metadata, scrobbler, lyrics, scheduler, task queue, KV, Subsonic API access) | 0.57 → rewritten 0.60 | Later: a small Rust plugin (`nd-pdk`) for "request this from inside Navidrome" or "send to Telegram". Heavy work stays in magpie. |
| Selective scan `navidrome scan --target` | 0.59 | Not needed — Subsonic `startScan` + watcher are enough. |
| Search rebuilt on SQLite FTS5 | 0.61 | After filing, magpie asks Navidrome `search3` for the song's ID to send a play link. |
| Server-side transcoding decisions (OpenSubsonic `transcoding`) + concurrency limits | 0.61 / 0.62 | Store originals (Opus/AAC/FLAC); let Navidrome transcode for mobile data. |
| **Sonic similarity** via plugins (e.g. AudioMuse-AI) | 0.62 | Phase 5: "radio"/Instant Mix that sounds right, Spotify-style. |
| **Structured lyrics**: TTML, ELRC (word-by-word), SRT, LRC sidecars, OpenSubsonic `songLyrics` v2 | 0.63 | Write `.lrc`, and `.elrc` when word timing exists. |
| `EnableSharing` on by default; shares owner-scoped | 0.63 / 0.64 | The bot replies with a Navidrome **share link** so guests can play without an account. |
| Experimental **Jellyfin Music API** (Finamp, Jellify) | 0.64 | Optional: more client choice. |
| Built-in backups (`Backup.Path/Schedule/Count`) | — | Turn on. Your play counts and stars live only in Navidrome's DB. |

**Navidrome config for this project** (`config/navidrome.toml` + env):

```toml
MusicFolder = "/music"            # = /srv/music/library, mounted :ro
DataFolder  = "/data"
Scanner.Schedule     = "@every 6h" # safety net; magpie triggers scans after every filing
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
| ⚠️ **Self-hosted Bot API server** (`tdlib/telegram-bot-api`, image `aiogram/telegram-bot-api`) | — | Cloud bots can only **download files ≤ 20 MB** and upload ≤ 50 MB. Local server: unlimited download, 2 GB upload, and `getFile` returns a **local path** — magpie just moves the file into `.magpie/staging/`. Fixes FLAC uploads. Needs `api_id`/`api_hash` from my.telegram.org. |
| **Topics in private chats** | 9.3 / 9.4 | The bot creates topics in your DM: *Requests*, *Review*, *New in library*, *Errors*. No more one long mixed chat. |
| **Reactions** | 7.0 | React 👀 → 👍 / 🤷 on the user's message as a lightweight ack (replaces the sticker trick). |
| **Mini Apps**: fullscreen, home-screen shortcut, Secondary button, DeviceStorage/SecureStorage, `downloadFile` | 8.0 / 9.0 | The review workbench on your phone (§4.6, Phase 4). |
| ⚠️ **Mini App same-origin hardening** | 10.2 (enforced 2026-07-20) | Serve the whole Mini App from **one HTTPS origin** (magpie). No cross-origin iframes or redirects. |
| `sendMessageDraft` / rich-message drafts (streamed text) | 9.3 / 9.5 / 10.1 | Optional: live "Downloading 43%…" without many edits. Plain edits throttled to every 2.5 s (what you do now) are fine. |
| **Rich messages** (`sendRichMessage`, audio/table/list blocks) | 10.1–10.3 | Album / playlist result cards with a track table. |
| **Disabled inline buttons**, ephemeral callback replies | 10.3 / 10.4 | Grey out "Rerun failed" while it runs. Show "Not authorized" only to the person who pressed. |
| **Button colours & custom-emoji icons** | 9.4 | Green "✓ This one" / red "✗ None" in review prompts. |
| `sendAudio` (MP3/M4A, performer/title/320 px thumb) | — | "📲 Send me the file" button: the song from your library, with cover, in Telegram's own player. |
| **Profile music** — `getUserProfileAudios` | 9.4 | Fun extra: "/import_profile" pulls songs from a user's Telegram profile playlist into the library. |
| Deep links `t.me/<bot>?startapp=review_<id>` | — | Notifications open the Mini App directly on the right track. |

**Library choice (Rust).** **`frankenstein` 0.52.1** (Sept 2026) is a thin, typed Bot API client
that follows new Telegram versions closely. magpie adds its own small router on top, because it
already has its own queue, state and persistence. `teloxide` 0.17 (last release July 2025) is
behind the Bot API and brings a framework we don't need. For comparison, in Python
python-telegram-bot 22.8 stops at Bot API 10.0 and aiogram 3.31 at 10.3.

### 6.3 Getting audio (acquisition)

| Item | State in Nov 2026 | Action |
|---|---|---|
| **yt-dlp 2026.10.26** | YouTube needs a JS runtime (Deno ✓ you have it) **and increasingly PO tokens** (SABR streaming) | Bundle the standalone `yt-dlp` program + Deno in the image, and let magpie **update yt-dlp itself every night** (into `DATA_ROOT/bin`) — YouTube breaks things every few weeks. Optional `pot` profile: **`bgutil-ytdlp-pot-provider` 2.0.2** when PO tokens become necessary. |
| Formats | 251 = Opus ~130–160 kbps (best free), 140 = AAC 128, 141/774 = Premium only (cookies) | Keep stream copy (you do). Never re-encode lossy→lossy. |
| ytmusicapi 1.12.3 | Works, unofficial, Python | **Not needed:** yt-dlp's YouTube Music search (`#songs`) gives the same song results; magpie scores them. |
| ⚠️ **Spotify Web API** | Since Feb/Mar 2026 Dev Mode: app owner **must have Premium**, max 5 users, search limit 10, **playlist items only for playlists you own/collaborate on**, batch endpoints removed | Public-page reading (ported from the bot into `acquire/`) is now the *only* way to read other people's playlists. Add canary tests that warn when it breaks. The Spotify API provider stays optional; prefer Deezer/MusicBrainz by ISRC. |
| iTunes Search/Lookup | ~20 req/min/IP, no key | Cache aggressively (`provider_cache`, from ununknown). |
| Deezer public API | No key, rate-limited | Primary ISRC → metadata bridge. |
| **slskd 0.26** (Soulseek) | Active, REST API | *Optional* lossless source (compose profile). Verify with fingerprint + duration + a check for fake FLACs (lossy files re-encoded as FLAC). ⚖️ This is file sharing, so the legal risk is yours. |
| Tidal/Qobuz rippers (streamrip, OrpheusDL, Octo-Fiesta) | Exist | ⚖️ Break those services' terms; accounts get banned. Not in scope. |
| Lidarr | v3.1.6, plugins (Tubifarry) | Not a dependency: its metadata server was down Apr–Aug 2025. Your pipeline already does what you'd need from it. |

### 6.4 Identification, tags, audio

| Item | Version | Note |
|---|---|---|
| Chromaprint / fpcalc | 1.6.x (FFmpeg 8 support) | ✓ bundled in the image |
| AcoustID | 3 req/s, **non-commercial only** | fine for personal use |
| MusicBrainz API | 1 req/s, descriptive User-Agent mandatory | ✓ already rate-limited (from ununknown) |
| SongRec (Shazam) | 0.8.1 | unofficial endpoint — fallback only (as now) |
| **LRCLIB** | `/api/get` (duration ±2 s), new `hasWordSync`, 90 req/min with a User-Agent | lyrics in the `library` module; send a real User-Agent |
| **rsgain 3.8** | ReplayGain 2.0, `rsgain easy` per album folder | Add **album gain**. ununknown measured track gain only. Run `rsgain easy` on the album folder after filing. Writes R128 tags for Opus correctly. |
| FFmpeg | 9.0.x (8.1 still maintained) | either |
| **lofty 0.25.5** | Rust tags | Bump from 0.24 (pre-1.0, minor API breaks) |
| MusicBrainz Picard **3.0.4** | Qt6, new plugin system | manual fallback for weird albums |
| beets 2.14.1 | CLI tagger | reference only; album-centric matcher doesn't fit "one song at a time" |
| AudioMuse-AI 3.7 | sonic analysis + Navidrome plugin | Phase 5 "radio" |

### 6.5 Code & platform stack

| Layer | Choice | Why |
|---|---|---|
| Program | **Rust 2024, tokio, axum 0.8, sqlx 0.9, lofty 0.25, reqwest, frankenstein** | ununknown's stack, plus a Telegram client |
| Web / Mini App | **React 19 + Vite 8** (already) + **TanStack Router/Query** + **Tailwind v4 + shadcn/ui** + **`@tma.js/sdk`** (3.3; 4.0 in RC), built **into** the binary | one page, two modes: desktop workbench and Telegram Mini App |
| Login | Telegram `initData` check (key = HMAC_SHA256("WebAppData", bot_token)) in the Mini App; one-time `/web` link on a computer | no passwords; owner/guest roles from the bot |
| API types | Rust types → generated TypeScript types for `web/` | the page and the program can't drift apart |
| Database | **One SQLite file**, WAL, `synchronous=NORMAL`, `busy_timeout`, one writer task, **on SSD, not on the NAS share** | proven at this scale; Navidrome itself had locking bugs on slow storage until 0.64.2 |
| Jobs | Own SQLite job tables (§4.5) | Huey/SAQ/Temporal would add infrastructure for nothing |
| Logs | JSON logs (`tracing`) with an `item_id` on every line | follow one song through every stage |
| Image | **One image**, Debian slim, multi-arch amd64/arm64, GHCR, Trivy scan (ununknown's release workflow) | |
| Compose | `compose.yaml` = magpie + navidrome, health checks, read-only root FS, `cap_drop`, **profiles** for optional helpers | |
| Updates | **Renovate** (Docker + Cargo + npm); yt-dlp updates itself nightly | |
| Remote access | **Tailscale** for players (Navidrome over your tailnet). **Tailscale Funnel or Cloudflare Tunnel** only for the Mini App HTTPS address. ⚠️ Don't stream audio through Cloudflare (their terms) | |
| Backups | **restic + Backrest** UI. `sqlite3 .backup` before each snapshot. Back up: `magpie.sqlite`, `navidrome/` data, `.env`, `config/`, and the library | |
| Hardware | Intel N100/N150 mini-PC (fingerprinting + rsgain on big backlogs) or Pi 5 8 GB + NVMe | |

---

## 7. Repository layout of this folder

One Rust crate at the root (`core/` was moved there with its history; the Python bot was
rewritten into `telegram/` and deleted). The folders inherited from ununknown keep their names;
new parts get their own top-level module.

```text
magpie-fi/                       ← this folder is still called navidrome-arta on disk
├── BLUEPRINT.md  README.md  SECURITY.md  LICENSE
├── Cargo.toml  Cargo.lock       crate "magpie-fi", binary "magpie"
├── Dockerfile                   the one image (Debian + ffmpeg, fpcalc, SongRec, rsgain, yt-dlp, Deno)
├── compose.yaml                 magpie + navidrome  (profile "tgapi": Telegram files up to 2 GB)
├── .env.example                 every setting, all MAGPIE_*
├── config/navidrome.toml  config/ytdlp/
├── migrations/                  one database schema
├── src/
│   ├── main.rs  config.rs       startup, folders, background loops; MAGPIE_* settings
│   ├── acquire/                 NEW (Phase 2): links, matching, yt-dlp, download queue + worker
│   ├── application/
│   │   ├── scan/ smart_approval.rs metadata_completion.rs artwork.rs input_dedup.rs   identify
│   │   ├── apply.rs             filing step
│   │   └── library.rs           NEW (Phase 1): layout, trash, library index, cover, playlists
│   ├── domain/                  audio reading, matcher, credits, genres
│   ├── infrastructure/          db, providers (20 sources), media tools, lyrics, navidrome
│   ├── http/                    API routes and handlers (incl. requests.rs)
│   └── telegram/                NEW (Phase 3): bot, status messages, review questions, guests
├── web/                         React: review workbench + Get music (Mini App in Phase 4)
├── scripts/                     docker-entrypoint.sh, setup-folders.sh
├── docs/TECHNICAL-OVERVIEW.md
└── .github/workflows/           magpie.yml · release.yml · security.yml
```

### 7.1 Where every part of the old projects goes

**From sync-me-maybe (Python, ~8,000 lines)**

| Old file(s) | Lines | Fate |
|---|---|---|
| `telegram_bot/handlers.py` | 1,179 | **Port** → `telegram/` (message routing). Its download/tag/store logic is **not** ported: the pipeline (§4.3) does that now |
| `telegram_bot/uploads.py`, `requests.py`, `callbacks.py`, `commands.py`, `runtime.py`, `safe_api.py`, `app.py` | 1,756 | **Port** → `telegram/` (upload batching, status lifecycle, buttons, commands, retry on Telegram errors) |
| `ui/messages.py` | 369 | **Port** → `telegram/` (texts, progress bar, keyboards) |
| `music/urls.py`, `music/providers/*` (spotify, apple, shazam, youtube, public_scrape) | 1,630 | **Port** → `acquire/` (link reading and playlist/album expansion) |
| `music/matching.py` | 337 | **Merge** into one scorer: its version-penalty rules (live, remix, sped up…) join ununknown's matcher |
| `music/downloader.py` | 227 | **Port** → `tools/` + `acquire/` (yt-dlp runner, timeout, cancel) |
| `music/lyrics.py`, `library/playlists.py`, `library/navidrome.py` | 264 | **Rewrite small** → `library/`, `navidrome/` |
| `music/tagging.py`, `music/metadata.py`, `music/enrichment.py`, `library/storage.py`, `library/index.py`, `music/filenames.py` | 1,177 | **Delete** — ununknown already does it better |
| `queueing/*`, `database.py`, `telegram_bot/persistence.py`, `config.py`, `auth.py`, `main.py`, facades | ~1,000 | **Delete** — replaced by the one queue, one database, one config |
| `pyproject.toml`, `uv.lock`, Python Dockerfile, ruff/mypy setup | — | **Delete** |

**From ununknown (Rust + React)**

| Old part | Fate |
|---|---|
| `src/domain/*`, `application/scan`, `smart_approval`, `metadata_completion`, `artwork`, `input_dedup`, `infrastructure/providers/*`, `media/*` | **Keep** → `identify/`, `library/`, `tools/` |
| `application/apply.rs` (`publish_no_clobber`) | **Keep** → `library/` with the §5 layout instead of `<input subfolder>/Artist - Title` |
| `frontend/` | **Keep** → `web/` |
| "Scan" clears the whole workspace; tracks deleted after writing | **Replace** with the permanent library index + items |
| Setup screen with input/output folders | **Delete** — fixed paths (`inbox/` → `library/`) |
| Settings and API keys stored in SQLite | **Delete** — environment only |
| "Remove input after output" switch | **Replace** — originals always go to `.magpie/trash/` |
| Loopback-only, no login | **Replace** with Telegram login (§4.6) |
| `/api/identify`-style one-shot workflow and the frontend "activity" heartbeat | **Replace** with the always-on job queue |

**Duplicates resolved**

| Was done twice | Now done once, in |
|---|---|
| Reading Spotify / Apple / Shazam / YouTube links | `acquire/providers` |
| Metadata lookup (Deezer vs 20 sources) | `identify/providers` |
| Scoring (bot matcher vs smart approval) | one scorer in `identify/` |
| Tag writing (mutagen vs lofty) | `library/` (lofty) |
| Cover art (unverified vs verified) | `identify/artwork` (verified) |
| Duplicate detection (text vs proof) | `library/index` + fingerprints |
| File naming and moving | `library/` (§5, atomic, no-clobber) |
| Queue, retries, persistence | `jobs/` |
| Database, config, Docker entrypoint, CI, release | one each |

---

## 8. Roadmap

Each phase ends with something that works. You can use magpie-fi after any of them.
Every phase lists what it **deletes**, so the project gets smaller, not bigger.

### Phase 0 — Bridge ✅ built · superseded by Phase 1
The old bot and old ununknown glued together through `inbox/`. Phase 1 replaced the ununknown
half; the bot half stays as the optional `bot` profile until Phase 3.
- ⛔ Deleted in Phase 3: `bot/`, the bot container, the `magpie-fi-bot` image.

### Phase 1 — One home ✅ built and tested in Docker (2026-11-10)
Turned ununknown into the magpie program and gave it a real library.
- `core/` moved to the repo root with its history, crate `magpie-fi`, binary `magpie`,
  `frontend/` → `web/`. All settings are `MAGPIE_*` environment variables; nothing is stored
  in the database any more.
- **Inbox:** checked every minute (or "Check inbox" in the web page). Files still being copied
  wait (`MAGPIE_INBOX_SETTLE_SECONDS`). Automation no longer pauses while the web page is open.
- **Library:** §5 layout and naming, permanent library index (`library_items`): a song you
  already own is recognised by fingerprint, ISRC or MusicBrainz ID and its new copy goes to the
  trash; a lossless copy replaces a lossy one. Existing library files are indexed at start.
- **Trash:** originals and replaced files go to `MUSIC_ROOT/.magpie/trash/<date>/`, emptied after
  `MAGPIE_TRASH_DAYS`.
- **Extras:** `cover.jpg` per album, synced lyrics from LRCLIB (`.lrc`/`.txt`), track + album
  ReplayGain (rsgain), `RELEASETYPE` tag, Navidrome scan after each batch.
- **Fix:** singles keep their own title as album name (ununknown renamed them all to "Single",
  which would merge every single of an artist into one Navidrome album).
- **One image:** `ghcr.io/artamrj/magpie-fi`, Debian trixie + ffmpeg, fpcalc, SongRec, rsgain.
  Release scan blocks on fixable findings. Navidrome image from `ghcr.io/navidrome/navidrome`.
- ⛔ Deleted: input/output setup screen, settings in the database, workspace reset ("Scan"),
  open-page pause and heartbeat, "remove input" switch, ununknown's release-archive scripts.
- Simplified on the way: **no separate re-file tool** (move an old library into the inbox; the
  same pipeline re-files it) and **no `artist.jpg`** (Navidrome fetches artist pictures from
  Deezer itself). `.m3u8` playlists move to Phase 2, where playlist links are handled.
- Tested end to end with real songs: 3 songs identified and filed with cover, synced lyrics and
  album gain; an inbox duplicate and a re-sent song went to the trash; a half-downloaded file was
  left alone; Navidrome listed all three, the single as `single`.
- The old Python bot still runs as an optional bridge: `docker compose --profile bot up -d`.
- The source keeps ununknown's folder names (`application/`, `infrastructure/`, `domain/`,
  `http/`); Phase 2 decided not to rename them (see Phase 2).

### Phase 2 — Getting music, in Rust ✅ built (2026-11-10)
- **Get music** in the web page: paste a link, follow every song until it is in the library,
  retry failed songs.
- `src/acquire/`: link reading for Spotify (embed page), Apple Music (iTunes Lookup API, public
  playlist page), Deezer (public API), YouTube/YouTube Music (yt-dlp) and Shazam; YouTube Music
  source matching with the bot's scoring rules; yt-dlp download (stream copy, m4a/opus).
- One queue in the database (`requests`, `wanted`, `playlists`), retries after 30 s / 2 min / 10 min,
  at most `MAGPIE_MAX_PARALLEL_DOWNLOADS` at a time, survives restarts.
- "Already have it?" before downloading: same link or video, ISRC, or same title (versions kept
  apart) + first artist + length.
- Downloads land in `.magpie/staging/` and go through the **same** inbox pipeline (identify, review,
  cover, lyrics, gain, library check). The link's title/artist/album/ISRC are written as tags first.
- Playlist links become `library/Playlists/<Owner - Name>.m3u8` in the link's order; sending the
  same playlist again rewrites the same file and downloads only what is missing.
- Albums stay together: a song whose catalog year differs from an album folder that already
  exists joins that folder and year.
- The image bundles yt-dlp (zipapp, self-updating daily into `DATA_DIR/bin`) and Deno.
- Decided on the way:
  - **No anonymous-token tricks.** The old bot read Spotify playlists past 100 songs with the
    embed page's access token, and Apple collections with a token taken from Apple's website.
    magpie uses only public pages and documented APIs. Spotify playlists are therefore read as
    far as the public embed page shows (about 100 songs).
  - **No generic `jobs/` module:** the `wanted` table is the queue; a second abstraction would
    only add code.
  - **No source-folder reshuffle:** `acquire/` sits next to the inherited folders; renaming those
    would change hundreds of imports and nothing else (see §7).
  - **No event bus yet:** the web page polls every 2 s while something is moving. Telegram status
    messages (Phase 3) will read the same tables.
- ✅ Done when pasting a Spotify playlist link gives a Navidrome playlist in the same order, every
  song stored once, and pasting it again downloads nothing.

### Phase 3 — Telegram in Rust → one image ✅ built (2026-11-10)
magpie-fi is now one program in one image; the Python bot is gone.
- `src/telegram/`: long polling, owners from `MAGPIE_TELEGRAM_OWNERS`, guests invited by owners
  with a one-time link (`/guests`) and kept in the database (the old bot forgot them on restart).
- Links → the same request queue as the web page. Audio files → `.magpie/staging/` and the
  normal pipeline (files forwarded together become one request). The old bot stored uploads raw;
  now they are identified, tagged and filed like everything else.
- One status message per request, edited as songs move ("2 of 12 in your library · 3 on the way"),
  with Stop, Retry failed and Details buttons that work after restarts (they point at the request
  in the database). 👀 on your message when it arrives, 👌 or 🤷 when it is finished.
- Review in Telegram: "Not sure which song this is" with up to three candidates and "None of
  these", using the same choose/remove logic as the web page.
- `/start` `/help` (with the whole rhyme) `/queue` `/health` `/id` `/guests`, hidden `/seven`;
  the rhyme line on finished requests (`MAGPIE_RHYME`).
- Optional `tgapi` profile (Telegram's own Bot API server) for files up to 2 GB.
- `frankenstein` is used for Telegram's types only; requests go through magpie's own HTTP client
  (its built-in client would have pulled in a second reqwest version).
- Tested against a fake Telegram server in the test suite (strangers, owners, invites, guests,
  buttons, status messages). Not yet tested with a real bot token, nor the `tgapi` helper.
- ⛔ Deleted: `bot/` (≈8,000 lines of Python), the bot container and image, Python CI and
  Dependabot entries.
- Moved to Phase 4: the `/web` login link (the web page has no login yet) and "Open review" /
  "▶ Play" buttons (they need an address reachable from your phone). DM topics were left out.

### Phase 4 — The workbench in your pocket ✅ built (2026-11-10)
- **Login** (`src/http/auth.rs`): with Telegram configured every API call needs either Telegram's
  signed Mini App data (checked with the bot token) or a 30-day session from a one-time `/web`
  link. Owners only; guests use the bot. Without Telegram nothing changes (local-only, no login),
  and a public address without Telegram is refused at startup.
- **Mini App:** the same React page opens inside Telegram (`telegram-web-app.js`): full height,
  Telegram's light/dark theme, straight to Review or Get music (`?view=`). The "magpie" menu
  button for owners and "📱 Open review" on review questions open it.
- **HTTPS address:** `MAGPIE_PUBLIC_URL`, via Tailscale Funnel (one command) or the new optional
  Cloudflare Tunnel profile (`tunnel`).
- "Recently added" list and "Log out" on the web page.
- Fixed on the way: a wrongly tagged file (a 2-minute tone tagged "The Weeknd – Blinding Lights")
  was filed automatically because several catalogs agreed on the tags. An automatic decision now
  never contradicts the audio length (more than 15 s off → Review). Inside Telegram, confirmations
  use Telegram's own dialog.
- Tested in Docker at phone size as a Mini App (Telegram-signed data, Telegram's script): login,
  theme, deep link to Review, cover and audio previews, and clearing the review list.
- Moved to Phase 5: "▶ Play" buttons and guest share links (Navidrome shares need Navidrome's own
  public address).
- ✅ Done when you can clear the whole review list from your phone.

### Phase 5 — Better than a subscription
- **Follow playlists:** `/follow <link>` checks daily and fetches new songs; the Navidrome playlist keeps the order.
- **Discovery:** Navidrome → ListenBrainz scrobbling; weekly, fetch "Weekly Exploration" songs you
  don't own → "New for you" playlist → Telegram note.
- **Sonic radio:** AudioMuse-AI + Navidrome's sonic-similarity plugin.
- "📲 Send me the file", `/import_profile` (Telegram profile music), guest share links.
- Optional `slskd` profile for lossless, with fake-FLAC detection.
- Optional small Navidrome plugin (Rust) to request songs from inside your player.

### Alongside every phase — boring operations
- Health checks, nightly backups (Backrest/restic, `sqlite3 .backup`), Navidrome's built-in backup.
- Fast automatic updates for yt-dlp. Canary tests for Spotify/Apple page reading that warn you on
  Telegram when a site changes.
- JSON logs with an `item_id` you can follow through every stage. `/health` in Telegram.

---

## 9. Decisions

| # | Question | Answer |
|---|---|---|
| 1 | One language or two? | ✅ **Decided: one program in Rust, one image** (§4). Navidrome stays its own official image in the same `compose.yaml`. |
| 2 | Telegram library | ✅ **Recommended: `frankenstein`** (thin, up to date) + own router. Recheck its Bot API version when Phase 3 starts. |
| 3 | Base image | **Debian slim** (yt-dlp and Deno need glibc). ununknown's Alpine image is replaced. |
| 4 | Where settings live | **Environment only** (`MAGPIE_*`). The web page shows them, doesn't edit them. |
| 5 | Login on a computer | **`/web` command → one-time login link.** No passwords. |
| 6 | Default format for YouTube downloads | **m4a (AAC)** if your household uses iPhones/CarPlay; **opus** otherwise. Never mp3. |
| 7 | Guest requests | Filed like yours, but **always through review** unless confidence is high. Option: owner approves first. |
| 8 | Re-file your existing library | **Yes, once** (Phase 1), with the dry-run report first and a full backup. |
| 9 | Library on the NAS share or local disk? | Music can live on the NAS; **`DATA_ROOT` on a local SSD**. `inbox/`, `library/` and `.magpie/` on the **same** filesystem. |
| 10 | Remote access | Tailscale for listening; one HTTPS address (Funnel/Tunnel) only for the Mini App. |
| 11 | Lossless via Soulseek? | Optional profile, off by default. The legal risk is yours. |
| 12 | Persian music naming | Tags in the original script, `ARTISTSORT` in Latin; folders follow the album-artist tag (NFC). |
| 13 | Rarely useful sources (SoundCloud needs credentials, AudD is paid) | Keep them, **off unless a key is set** — as today. |
| 14 | Project name | ✅ **Decided: `magpie-fi`** (§0). Crate `magpie-fi`, binary `magpie`, env prefix `MAGPIE_`, image `ghcr.io/artamrj/magpie-fi`. |
