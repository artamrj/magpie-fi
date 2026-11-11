# 🐦 magpie-fi — Blueprint

> **magpie-fi** — *steals every shiny song and hides it in your own nest.*
>
> A self-hosted music service for you and a few people: **send a link or a file → it lands in
> your library, correctly tagged, ready for Navidrome.**

This is the design document: what magpie-fi is, how it is built, the rules it follows and what comes
next. What it does is in [README.md](../README.md); the exact algorithms are in
[TECHNICAL-OVERVIEW.md](TECHNICAL-OVERVIEW.md).

Contents

1. [What magpie-fi is](#1-what-magpie-fi-is)
2. [Principles](#2-principles)
3. [Architecture](#3-architecture)
4. [Identification and decisions](#4-identification-and-decisions)
5. [Library layout and naming](#5-library-layout-and-naming)
6. [Data and state](#6-data-and-state)
7. [Security model](#7-security-model)
8. [Decisions](#8-decisions)
9. [Roadmap](#9-roadmap)
10. [History](#10-history)

---

## 1. What magpie-fi is

magpie-fi is **one Rust program with a React web page, in one Docker image**
(`ghcr.io/artamrj/magpie-fi`). It:

- **gets songs** from links (Spotify, Apple Music, Deezer, YouTube / YouTube Music, Shazam) sent
  to its Telegram bot or pasted in the web page, from audio files sent to the bot, and from a
  folder you drop music into (`MUSIC_ROOT/inbox`);
- **identifies** each song by its sound and its tags with 15 online sources;
- **fixes** its tags, cover, lyrics and loudness;
- **files** it into a clean library (`MUSIC_ROOT/library`), once per recording;
- **asks you** (Telegram or web page) when it is not sure.

[Navidrome](https://www.navidrome.org) plays the library on every device. You run Navidrome
yourself, with its music folder pointed at `MUSIC_ROOT/library`; it is not part of this project's
`compose.yaml`.

The name: a magpie grabs every shiny thing and carries it home; *hi-fi* for correct tags and
verified covers, *wi-fi* because it lives on your own network.

**Non-goals:** not a Navidrome fork, plugin or API client; not a public multi-tenant service (one
household and a few guests); no DRM circumvention (Spotify, Apple Music and Deezer links are read
only for the song list, the audio comes from YouTube; the one exception is a free Radio Javan
podcast episode, which Radio Javan serves as a public MP3 without DRM and is downloaded from
there, while Premium-only episodes are refused); not Lidarr (no "monitor every album").

---

## 2. Principles

1. **One of everything.** One program, one image, one database, one config (`MAGPIE_*` in
   `.env`), one queue, one tag writer, one duplicate index. Every job is done in exactly one place.
2. **Your files are sacred.** Nothing is overwritten (atomic, no-clobber publish). Originals go to
   a trash with retention, never straight to `rm`. Existing library files are indexed, not moved
   or renamed. Their tags change only when you ask: a fix on the Artists page, or a field you
   edit on the Files tab, written on a copy that replaces the song in one rename. Only magpie-fi
   writes into `library/`.
3. **Sure → file, unsure → ask.** A song is filed automatically only when the evidence is strong
   (audio recognition or several agreeing sources, matching length, matching version). Everything
   else waits for you. A wrong guess never goes silently into the library.
4. **One pipeline for every input.** A link, a Telegram file and a dropped file all end up as a
   file in the inbox or in `.magpie-fi/staging/` and go through the same identification and filing.
5. **One copy per recording.** Duplicates are found by proof (same bytes, same fingerprint, same
   ISRC or MusicBrainz ID with matching length), never by title alone. A lossless copy replaces a
   lossy one; anything else that is already there goes to the trash.
6. **No Navidrome API.** magpie-fi only writes files (songs with tags, cover and lyrics inside,
   and `.m3u8` playlists).
   Navidrome's file watcher picks them up. No Navidrome credentials, no coupling to its versions.
7. **Tools are bundled programs.** ffmpeg, fpcalc, SongRec, rsgain, yt-dlp and Deno ship in the
   image and are run with timeouts. A missing optional tool turns a feature off, it doesn't stop
   magpie-fi.
8. **Boring to run.** Settings only in `.env`, one port, read-only container, survives
   restarts (queue, buttons and review state live in the database).

---

## 3. Architecture

### 3.1 Modules

```text
src/
├── main.rs        startup, background loops, HTTP server, graceful shutdown
├── config.rs      MAGPIE_* settings and the fixed worker limits
├── app/           shared state: database pool, HTTP client, workflow slot
├── acquire/       links → wanted songs → YouTube Music match → yt-dlp → .magpie-fi/staging
├── application/
│   ├── scan/      inbox check: walk, read, integrity, fingerprint, source cascade, persist
│   ├── input_dedup.rs · smart_approval.rs · metadata_completion.rs · artwork.rs
│   ├── apply.rs   filing into the library
│   └── library.rs layout and naming, library index, trash, folder cover, playlists
├── domain/        audio reading, text matching, artist credits, genres
├── infrastructure/
│   ├── providers/ identification, cover and genre sources
│   ├── media/     integrity, fingerprint, tag writer, repair, rsgain
│   ├── db/        SQLite, tracks and candidates
│   └── lyrics.rs · provider_cache.rs · fingerprint_cache.rs · process.rs · resilient_http.rs
├── http/          API routes, login, security headers
└── telegram/      bot client, commands and buttons, status messages, texts
web/               React page (Vite), built as a Telegram Mini App: Add, Review, Library
migrations/        the SQLite schema
```

The folder names `application/`, `infrastructure/`, `domain/` and `http/` come from ununknown and
were kept on purpose (§8).

### 3.2 How a song flows

```text
 Telegram (links · files · buttons · Mini App)          web page (paste a link, review)
              │                                                    │
              ▼                                                    ▼
 ┌─ acquire ──────────────────────────────────────────────────────────────────────────┐
 │ read the link → one `wanted` row per song → already in the library? → stop          │
 │ else best YouTube Music match → yt-dlp (stream copy) → .magpie-fi/staging/ + tags   │
 └───────────────────────────────────────┬────────────────────────────────────────────┘
 Telegram file → .magpie-fi/staging/ ────┤
 your drop → MUSIC_ROOT/inbox/ ──────────┤ (waits until the file stops changing)
                                         ▼
 ┌─ inbox cycle (every minute) ────────────────────────────────────────────────────────┐
 │ scan:  read tags → decode check → fingerprint → group duplicates                    │
 │        → 15 sources → smart auto-select → completion → verified cover               │
 │        sure → Ready        unsure → Review (web page, Telegram question)            │
 │ apply: Ready → already in library? → tags + cover + lyrics → no-clobber publish     │
 │        → trash → index → playlists;  failure → Issues                               │
 │        after the batch: rsgain per album folder                                     │
 └───────────────────────────────────────┬────────────────────────────────────────────┘
                                         ▼
                              MUSIC_ROOT/library ──► Navidrome (file watcher) ──► your apps
```

### 3.3 Background loops (`src/main.rs`)

| Loop | When | What |
|---|---|---|
| Inbox cycle | every `MAGPIE_INBOX_CHECK_SECONDS` (60), or on "Check inbox now" | identify new files, then file everything Ready |
| Library index | at start, then every 6 hours | index files added by hand, forget deleted ones |
| Download worker | continuously | works through `wanted`, `MAGPIE_MAX_PARALLEL_DOWNLOADS` at a time |
| Telegram bot | continuously (long polling) | commands, links, files, buttons; status messages every 3 s |
| Daily maintenance | at start if due, then at midnight | expired source answers, media-cache limit, trash purge |
| Media-cache limit | hourly, when idle | keep analysis caches under 100 MiB |
| yt-dlp update | daily | update the copy in `DATA_ROOT/magpie-fi/bin` |

Only one workflow (inbox cycle, "File now", auto-approve, "Check & fix") runs at a time.

### 3.4 Web page, Mini App and Telegram

- One axum server serves the API and the built web page from disk (`/usr/share/magpie-fi` in the
  image), so everything comes from one origin, as Telegram's Mini App rules require.
- The web page is designed as a Telegram Mini App and looks the same in a browser: one column
  of Telegram-style grouped lists, colours from Telegram's theme variables (`--tg-theme-*`, with
  Telegram's iOS day and night colours as fallbacks), and four tabs:
  - **Add**: paste a link (or send one to the bot), then a short list like a browser's
    downloads: what is on the way now, and what settled in the last day (added, needs your
    pick, failed with the reason and a Retry); older entries behind "Show earlier". A song that
    needs a pick opens in Review; an album or playlist opens its songs. Sending the same link
    again retries the existing entry instead of adding a second one; a song removed in Review
    leaves the list.
  - **Review**: songs grouped by what has to happen to them (needs your pick, issues, ready to
    file), with "Accept N confident matches" and "Check and repair" as the first rows. One song
    opens as its own screen that asks one question: which release (a radio list with only the
    differences as chips, details on demand), pick a cover, file anyway, repair, or remove. The
    manual tag editor is a mode of that screen. The screen's action is Telegram's native Main
    Button (Secondary Button for Skip / Remove), drawn in the page outside Telegram.
  - **Artists**: song and artist counts, and the artists (rename, join, split a duet, make an
    album that Navidrome shows twice one album, from a bottom sheet). On a phone and in Telegram the status (bot, sources, folders, downloads,
    log out) sits at the bottom of this tab; a wide window shows it at the foot of the sidebar
    instead, in rows like the sidebar nav: "Ready" with the song count, then one row only for
    each problem (bot off, sources missing, no yt-dlp), with a short explanation on hover.
  - **Files**: the `library/` folder as it is on disk, one folder at a time (a folder tree on a
    wide screen), a search over titles, artists, albums and paths, and one song's tags, cover,
    sound and path with a player. The tags (title, artist, album, album artist, track, disc,
    year, genre, composer, label, ISRC) can be edited: only changed fields are written, a file
    that changed since it was opened is not overwritten, and the toast's Undo writes the old
    values back. "Edit all songs" (on an album folder; the pencil in the desktop tree) sets
    album, album artist, artist, year or genre for every song directly in that folder, with
    "Different" shown where the songs disagree; it writes nothing when one song changed since
    the folder was opened. Folders and files are never renamed there, so a new album name or
    year does not move the song (Navidrome goes by the tags). Hidden folders (`.magpie-fi`,
    `@eaDir`) and anything outside `library/` are never shown.
  A browser window wider than 1024 px shows a sidebar, the list and the song side by side
  (Enter uses the match, → goes to the next song). Telegram's panel is never that wide. From
  1360 px (a 15–16" laptop or a desktop screen) the song also puts the release list and "what
  changes in your file" next to each other, so both are in view without scrolling.
- With `MAGPIE_PUBLIC_URL` (HTTPS) the same page opens **inside Telegram** as a Mini App, from the
  bot's menu button or "📱 Open review" (`?view=review`).
- The bot: one status message per request, edited as songs arrive, with Stop / Retry failed /
  Details buttons; "Not sure which song this is" questions with one button that opens the Mini
  App on that song (up to three candidate buttons in chat only when there is no HTTPS address);
  `/start`,
  `/help`, `/queue`, `/health`, `/id`, `/guests`, `/web`.

### 3.5 The image and deployment

- `ghcr.io/artamrj/magpie-fi`, linux/amd64 + linux/arm64, Debian trixie slim (yt-dlp and Deno
  need glibc). Contains `magpie-fi`, the web page, ffmpeg/ffprobe, fpcalc, `songrec-lib-cli`,
  rsgain, yt-dlp (zipapp, self-updating), Deno and tini.
- `compose.yaml` has one service, `magpie-fi`: port `MAGPIE_PORT` (7331) on every host interface, `MUSIC_ROOT` mounted whole at
  `/music` (so moves are renames), `DATA_ROOT/magpie-fi` at `/data`, `config/ytdlp` read-only for an
  optional `cookies.txt`.
- The entrypoint creates magpie-fi's folders, fixes their owner to `PUID:PGID` (never your existing
  music files) and drops to that user.
- Releases: `scripts/release.sh X.Y.Z` bumps the version everywhere, writes `CHANGELOG.md` from
  the commit subjects (git-cliff, `cliff.toml`), runs the CI checks, commits and tags locally. A
  pushed `v*` tag builds and pushes the image (Trivy scan included) and makes the GitHub release
  with that version's CHANGELOG section as its notes. The Docker build cache lives in the registry
  (`magpie-fi:buildcache-amd64` / `-arm64`); the Debian update layer is refreshed weekly.
  Dependabot updates Cargo, npm, GitHub Actions and Docker base images monthly.

---

## 4. Identification and decisions

Details and exact weights: [TECHNICAL-OVERVIEW §8–11](TECHNICAL-OVERVIEW.md#8-library-index-and-duplicates).

- **Evidence from the file.** Tags, length, a decode check (damaged files go to Issues and can be
  repaired), a Chromaprint fingerprint, or SHA-256 when fingerprinting fails.
- **Evidence from a link.** A download carries the link's title, artist, album and ISRC as tags,
  so it is usually identified without questions — but still checked against length and sound.
  The ISRC tag of any file (from a link, a store or a ripper) is read back: a source with the
  same code and a length within 3 s is proof, and Deezer is asked for the code directly.
- **Only free sources, cheap first.** AcoustID (by fingerprint) and MusicBrainz; YouTube (video
  ID in the filename); Apple Music, Deezer and the Persian catalogs Radio Javan, Audiomack and
  Navahang; Genius; SongRec (Shazam) for audio recognition when nothing else matched or the text
  sources could not decide, never on a file longer than 20 minutes (Shazam hears a slice, so
  on a DJ mix it names one song in it); SoundCloud,
  Discogs and Last.fm. Sources that need a (free) key are off without it. Cover Art Archive
  supplies covers, Wikidata artist genres, LRCLIB lyrics.
- **Smart approval.** Every candidate gets a weighted score (title, artist, length, album,
  completeness, agreement between sources, audio recognition, version tags). Automatic only when
  the best one is supported (recognised by audio, the file's own ISRC with the same length,
  confirmed by two sources, or an exact catalog match), scores ≥ 68, has no different recording
  within 12 points, has the same version tags (live, remix, session, …) and is at most 15 s off
  the audio's length. Other releases of the same recording (the single and the album, a store
  that lists fewer singers with the same length, a Persian title in another Latin spelling
  with the same length) are not rivals. Review says in plain words what is unsure.
- **Completion.** Missing fields are filled only from agreeing recordings and compatible releases;
  existing values are never replaced, with two exceptions. A Persian title spelled differently
  by the sources takes the spelling most sources use. An old album takes the earliest year
  another listing of the same album gives, because stores show the year it went digital (a
  single, EP or compilation never sets an album's year).
- **The file decides who is the main singer.** Sources disagree about duets and featured
  songs: a release by the featured artist, Radio Javan or a YouTube channel list that artist
  first, and some list only the featured singers. When the file's own artist tag names a main
  singer that the chosen match credits in another position, that singer goes first and the
  others become featured. When the match leaves the main singer out but every name it gives is
  in the file ("Sohrab MJ & Orchid" for "Amir Tataloo feat. Sohrab MJ & Orchid"), the main
  singer is put back first. A single's album artist follows. Such a match is also not an exact
  artist match: it is filed on its own only with the audio or a second source agreeing. Review
  shows "Lists X first" or "Leaves out X" on such a match.
- **DJ mixes are podcast episodes.** Radio Javan lists DJ mixes as podcasts. A podcast
  episode is filed as one track of its show: "Dor Dor 8 (Old School)" by Deejay Al is track 8
  of the album "Dor Dor", release type broadcast with the "dj-mix" secondary type, as
  MusicBrainz files mixes. A file of such a mix is found by its tags and length.
- **A stage name alone is the same person.** Sources spell singers shorter: Shazam gives "MJ
  feat. Tataloo & Orchid" and Deezer "MJ" for a file tagged "Amir Tataloo feat. Sohrab MJ &
  Orchid". A name that is the end of another one ("Tataloo" of "Amir Tataloo") is that person;
  a first name alone ("Amir") is not. When a match names exactly the file's singers, it is told
  the file's way as soon as it is found, before Review, ranking and approval: the file's order,
  the fuller spelling of each name. So Review shows what would really be written, and two
  sources that spell the same singers differently are one recording, not two.
- **Verified covers.** A cover must belong to the chosen release, download, decode, and not be
  blank or tiny. No verified cover → no filing (the song waits in Review). A cover verified for
  one track of an album is reused for the others.
- **Tags before covers.** Correct tags matter more than a cover: the best match is never swapped
  for another release just because that one has a cover. A sure match without a cover waits in
  Review with the match chosen, and the page only asks for a cover. Covers are looked for by the
  song, then by the album (album name and artist on Apple Music and Deezer, other editions of the
  same MusicBrainz album). The **cover finder** searches again after each inbox check (each song
  at most every 12 hours) and files the song as soon as one is found.
- **Review.** Choose a candidate, paste a song link, type the details, change the cover, or
  remove the file. In Telegram: pick one of three buttons or "None of these".

---

## 5. Library layout and naming

Navidrome groups albums and artists by **tags**, not folders. The folder layout is for you, for
backups and for other tools; both are kept clean and consistent.

### 5.1 Folders

```text
MUSIC_ROOT/                         one filesystem: every move is an atomic rename
├── library/                        Navidrome's music folder; only magpie-fi writes here
│   ├── Radiohead/
│   │   ├── OK Computer (1997)/
│   │   │   ├── 01 - Airbag.flac          tags, cover and lyrics inside the file
│   │   │   └── 02 - Paranoid Android.flac
│   │   └── Creep (1992) [Single]/
│   │       └── 01 - Creep.m4a
│   ├── Googoosh/Pol (2000)/        names stay in their own script (NFC)
│   ├── Various Artists/Pulp Fiction (1994)/01 - Dick Dale & His Del-Tones - Misirlou.m4a
│   └── Playlists/Arta - Road Trip.m3u8      relative paths
├── inbox/                          your drop zone; magpie-fi empties it
└── .magpie-fi/                     hidden work area
    ├── staging/                    downloads and Telegram files on their way
    └── trash/<date>/               originals and replaced files, kept MAGPIE_TRASH_DAYS (30)
DATA_ROOT/magpie-fi/                magpie-fi.sqlite, bin/yt-dlp (keep on a local SSD)
```

- `inbox/` is only for you; magpie-fi's own downloads go to `.magpie-fi/staging/`.
- Downloads are deleted after filing; your own originals go to the trash.
- Playlists live inside the library because Navidrome's `PlaylistsPath` is relative to it.

### 5.2 Naming rules

```text
{AlbumArtist}/{Album} ({Year})[ [Single]|[EP]]/{Disc-}{Track:02} - {Title}.{ext}
```

| Case | Example |
|---|---|
| Album | `Daft Punk/Discovery (2001)/03 - Digital Love.flac` |
| Multi-disc (flat, disc prefix) | `Pink Floyd/The Wall (1979)/2-01 - Hey You.flac` |
| Single / EP | `Dua Lipa/Houdini (2023) [Single]/01 - Houdini.m4a` |
| Compilation | `Various Artists/Drive (2011)/02 - Kavinsky - Nightcall.m4a` |
| Featured artists | only in tags, never in folder names |
| Several artists (`Ebi & Shadmehr Aghili`) | `Ebi/Song (2024) [Single]/01 - Song.m4a`; both in `ARTISTS` |
| Unknown album | does not enter `library/`; stays in Review |

- **Singles are albums**: a one-track release folder, keeping the song's own title as album name.
- Folder artist = **the first album artist's identity**, never the track artist and never a
  combined `A & B` name. Every spelling of an artist (`گوگوش`, `Googoosh`, `GOOGOOSH`) has one
  identity, so one folder (§5.4).
- Songs of one new album filed together get one year, so the album never splits.
- **An album already in the library decides its details.** Navidrome tells albums apart by
  name, album artist (or its MusicBrainz ID), MusicBrainz album ID and release date, so songs
  of one album that disagree on any of them show up as two albums. A new song whose album the
  album artist already has (the same album in any spelling: "Khalij" = "Khaleej", a store's
  " - Single" ending, any year) joins that folder and takes the album's details from a song
  already there: name, album artist, release date, year, MusicBrainz album and release-group
  IDs, release type, compilation flag and label. A single named like an album the artist has
  (the title song) joins the album, without the single's track number. Editions stay apart
  ("Khalij (Remastered)").
- Every name is Unicode **NFC**; `/ \ : * ? " < > |` and control characters are replaced,
  trailing dots and spaces trimmed, at most 180 bytes per part.
- Name collisions get ` (2)`, ` (3)`, … — unless the existing file is the same audio with the
  same cover, then nothing new is written.
- **Never re-encode.** YouTube's own Opus/AAC stream is kept; FLAC stays FLAC.

### 5.3 What magpie-fi writes

| What | Notes |
|---|---|
| Title, artist, `ARTISTS` (multi-value), album, album artist, `ALBUMARTISTS` | multi-value lets Navidrome link each featured artist; in M4A all values go into one atom, since TagLib reads only the first of duplicated atoms |
| `ARTISTSORT` / `ALBUMARTISTSORT` | the Latin spelling when the display name is Persian, so Navidrome sorts it among the Latin names |
| Track and disc number and totals, genre, ISRC, label, language, composer | |
| `DATE` (year), `RELEASEDATE` (full date when known), `ORIGINALDATE` (first release) | Navidrome groups and sorts albums by the release date |
| MusicBrainz recording, release, release-group, release-track, artist and album-artist IDs | only real MusicBrainz IDs; release and release-track IDs give Navidrome stable album and track identity across renames |
| Release type (`RELEASETYPE` / "MusicBrainz Album Type"), compilation flag | Navidrome can show albums, singles and EPs apart |
| Embedded front cover, JPEG at most 1200 px; the same image as `cover.jpg` in the album folder | verified, matching the release; Navidrome looks for `cover.jpg` first |
| ReplayGain track + album (`rsgain easy` per album folder) | R128 tags for Opus; skipped with a warning if rsgain is missing |
| Lyrics from LRCLIB, inside the song: LRC text with timestamps (synced) or plain | `LYRICS` (FLAC/Opus), `©lyr` (M4A), `USLT` (MP3); lyrics already inside a file are kept |
| Synced lyrics also as `<song>.lrc` next to the song | Navidrome reads the file before the tag, Symfonium the tag before the server; never overwrites an existing lyrics file |
| With `MAGPIE_SIDECARS=true`: nothing inside the song; `.lrc`/`.txt` next to it | for players that only read separate files |
| `Playlists/<Owner - Name>.m3u8` | one per playlist link, in the link's order |

Tags are written and verified on a temporary copy **before** the file enters `library/`, so
Navidrome never sees a half-tagged file and never sees a retag and a move in one scan.

**Formats.** Nothing lossy is ever re-encoded: a song keeps the codec it arrived with. Links
are downloaded with `MAGPIE_AUDIO_FORMAT=best` (the default): YouTube's Opus stream in `.opus`,
AAC in `.m4a` where that is all there is. Sent files stay MP3, M4A, FLAC or Opus. WAV and AIFF
become FLAC when filed (lossless to lossless; they cannot be tagged). Damaged files are the one
exception: repair re-encodes within the same codec.

### 5.4 Artists

One identity per artist (`artists`, every spelling in `artist_names`), so all spellings share one
name in `ARTISTS` / `ALBUMARTISTS` and one folder:

- Spellings match by a key that unifies Persian and Arabic letters, ZWNJ, accents, `&`/`and` and
  a leading `the`; Latin transliterations (`Yeganeh`/`Yegane`) match when only one artist fits.
  A new Persian-script name is looked up on MusicBrainz and joins its Latin spelling.
- `A & B`, `A x B`, `A, B`, `A و B` become separate artists only when each part is a known artist
  and the whole is not known as one (a band or duo with an MBID, or marked by you).
- New artists are learned only when a song is filed or the library is indexed, never from search
  results.
- `feat.`, `ft.`, `featuring`, `/` and `;` always separate artists, even inside one name a source
  sent (`Tommee Profitt feat. Fleurie`). Artists learned that way by older versions are split
  automatically after indexing; their songs move to the main artist's folder.
- The **Artists** page lists them with their spellings: rename, join two, split a duet, or mark
  "one artist". The songs are retagged and moved; Navidrome may then see them as new songs.
- **Albums shown twice.** Opening an artist checks its albums (every song's album details) and
  offers "Make one album" for each album Navidrome would show more than once: every song takes
  the details most of the album has (ties: an album over a single, a MusicBrainz release, a full
  date) and moves into the folder most of those songs are in; a single that joins loses its
  track number. The list marks artists with an album spelled two ways ("Albums?").

### 5.5 Navidrome

Recommended Navidrome settings: file watcher on, playlists auto-imported from `Playlists`,
`Scanner.PurgeMissing = "never"`. Navidrome uses the cover and lyrics inside each song (it reads
LRC timestamps there as synced lyrics) because the album folders hold no `cover.*` or `.lrc` files.
Libraries filed before one-file mode are tidied once: lyrics files are put inside their songs and,
where every song has its cover inside, `cover.*` goes to the trash.

---

## 6. Data and state

**Configuration** is environment only (`.env`, `MAGPIE_*`). The web page shows it but cannot
change it. Worker counts are fixed in `src/config.rs`.

**One SQLite file**, `DATA_ROOT/magpie-fi/magpie-fi.sqlite`: WAL, `synchronous=NORMAL`, file mode
`0600`. Keep it on a local disk, not a network share.

| Area | Tables |
|---|---|
| Songs on their way | `tracks` (deleted once filed), `candidates`, `artwork_overrides`, `automatic_scan_files` |
| The library | `library_items` (permanent index), `canonical_names` |
| Getting music | `requests`, `wanted` (the download queue, with retries after 30 s, 2 min, 10 min), `playlists` |
| Telegram and login | `telegram_guests`, `telegram_invites`, `telegram_state`, `login_links`, `sessions` |
| Caches | `provider_cache`, `fingerprint_cache`, `integrity_cache`, `content_hash_cache` |
| Bookkeeping | `maintenance` |

**Caches and their limits**

- Source answers and downloaded covers (`provider_cache`) keep a lifetime per source, from 7 to
  90 days. Expired rows are removed at start and once a day. Failed lookups are not cached.
- File analysis (fingerprint, integrity, content hash) is keyed by path, size and mtime, and
  capped at 100 MiB together; the oldest rows go first.

**Trash.** `MUSIC_ROOT/.magpie-fi/trash/<date>/`, one folder per day, emptied after
`MAGPIE_TRASH_DAYS`. Restoring a song is a file move.

**Backups.** Back up `.env`, `DATA_ROOT/magpie-fi/` and the library. Navidrome's own database
(users, play counts, stars) is backed up on Navidrome's side.

---

## 7. Security model

- The port is published on every interface of the host, so other devices on your network can
  open the page. Remote access is Tailscale Funnel or `ssh -L`.
- Without Telegram there is no login: anyone who can reach the port can use the page. A public
  address without Telegram is refused at start.
- With Telegram, Telegram is the login: signed Mini App data, or a one-time `/web` link that
  starts a 30-day session. Owners only; guests use the bot.
- Secrets live only in `.env`; login tokens are stored as hashes; the database is `0600`.
- The container runs as `PUID:PGID` with a read-only root filesystem and almost no capabilities.

---

## 8. Decisions

| # | Question | Answer and reason |
|---|---|---|
| 1 | One language or two? | **One Rust program, one image.** The Python bot was ported and deleted: one queue, one database, one set of tests. |
| 2 | Navidrome in the stack? | **No.** First planned as part of `compose.yaml` with API calls (scan, search, share links); dropped: the file watcher is enough, and users already run Navidrome their own way. |
| 3 | Telegram library | **`frankenstein` for types only**, requests through magpie-fi's own HTTP client (one reqwest version), own small router. |
| 4 | Self-hosted Bot API server for files over 20 MB? | **No.** One more container and Telegram app keys for little gain; the bot explains the limit. |
| 5 | Base image | **Debian trixie slim**, because yt-dlp and Deno need glibc. |
| 6 | Where settings live | **Environment only.** One place to look; the page shows, doesn't edit. |
| 7 | Login | **Telegram** (Mini App data, `/web` one-time link). No passwords; owners only. |
| 8 | Download format | **m4a** by default (iPhone/CarPlay), **opus** optional. Stream copy, never re-encoded. |
| 9 | Re-file an existing library | **Move it into the inbox.** No separate re-file tool or dry-run report; the normal pipeline does it, uncertain songs wait in Review. |
| 10 | Where data lives | `MUSIC_ROOT` on one filesystem (atomic moves); `DATA_ROOT` on a local SSD. |
| 11 | Remote access | **Tailscale Funnel or `ssh -L`.** The optional Cloudflare Tunnel profile was removed: one less container and token. |
| 12 | Reading Spotify/Apple links | **Public pages and documented APIs only**, no borrowed tokens. Spotify playlists are read as far as the public page shows (about 100 songs). |
| 13 | Job system / event bus | **None.** The `wanted` table is the queue; the web page polls, the bot reads the same tables every 3 s. |
| 14 | Rename the inherited folders? | **No.** It would change hundreds of imports and nothing else. |
| 15 | Web page in the binary? | **No, served from disk** next to it (`MAGPIE_WEB_DIR`); same single origin. |
| 16 | ReplayGain | **Only rsgain, per album folder after each batch.** The per-file ffmpeg pass was removed: rsgain already measures track and album gain. |
| 17 | Fewer sources | **Two were removed** (Nov 2026): an extra metadata catalog and the Wikidata song search. Wikidata stays for artist genres. |
| 18 | Paid sources | **None.** AudD and the Spotify Web API (owner needs Premium since 2026) were removed; Spotify links are read from the public page. Free-key sources (AcoustID, Last.fm, Discogs, YouTube, SoundCloud) stay, off unless a key is set. |
| 19 | A song fails while filing | **It goes to Issues** instead of being retried every cycle; the others continue. |
| 20 | Persian music | Tags and folders in the original script, NFC-normalised. |
| 21 | Dependency updates | **Dependabot**, monthly. yt-dlp updates itself daily. |
| 22 | Project name | **magpie-fi** everywhere: crate, binary, image, compose service, folders (`.magpie-fi/`, `magpie-fi.sqlite`). Settings keep the short prefix `MAGPIE_`. |

---

## 9. Roadmap

### Done

- **Phase 1 — One home.** ununknown became `magpie-fi`: inbox checked every minute, §5 layout,
  permanent library index (no duplicates, lossless replaces lossy), trash, `cover.jpg`, LRCLIB
  lyrics, album ReplayGain, release types, settings only in the environment.
- **Phase 2 — Getting music.** Links from Spotify, Apple Music, Deezer, YouTube and Shazam;
  YouTube Music matching; yt-dlp downloads into staging; one queue with retries; playlists as
  `.m3u8`; "already have it?" before downloading.
- **Phase 3 — Telegram in Rust.** The Python bot was replaced: owners and invited guests, links
  and files into the same pipeline, live status messages, review questions with buttons.
- **Phase 4 — In your pocket.** Telegram login, the web page as a Mini App, `/web` login links,
  recently added songs. An automatic decision never contradicts the audio length.
- **Cleanup (Nov 2026).** Navidrome and the Cloudflare Tunnel left `compose.yaml`; the entrypoint
  prepares the folders itself; ReplayGain only from rsgain; fewer sources; failed filings go to
  Issues; per-source cache lifetimes; bounded media caches; tool timeouts; covers only through
  magpie-fi's own endpoints; unused tables and columns dropped.
- **One name (Nov 2026).** The program, image, compose service and folders are all called
  magpie-fi. `.magpie/` and `magpie.sqlite` are moved to the new names on first start, and
  everyone logs in once more (new cookie name). Coming from an earlier version, move
  `DATA_ROOT/magpie` to `DATA_ROOT/magpie-fi` on the host before starting.

### Next (Phase 5 — better than a subscription)

- **Follow playlists:** `/follow <link>` checks daily and fetches new songs; the playlist keeps
  its order.
- **Discovery:** weekly "New for you" from ListenBrainz/Last.fm listening, missing songs fetched,
  a note on Telegram.
- **Play links:** "▶ Play" buttons and guest share links (needs Navidrome's own public address).
- **"📲 Send me the file"** from the library into Telegram's player.
- **Sonic radio** through a Navidrome plugin (e.g. AudioMuse-AI) — on Navidrome's side.
- **More tags:** original release date, album version (Deluxe, Remaster) to keep editions apart,
  sort names for Persian artists, word-synced `.elrc` lyrics when LRCLIB has them.

### Known gaps

- Spotify playlists: only what the public page shows (about the first 100 songs).
- "Stop" in Telegram drops songs that have not started; a running download finishes.
- No canary test yet that warns when Spotify or Apple change their public pages.

**Ideas, not planned:** lossless from Soulseek (slskd) with fake-FLAC detection (file sharing:
the legal risk is yours); a small Navidrome plugin to request songs from inside the player.

---

## 10. History

magpie-fi grew out of two earlier projects, kept in this repository's history:
[ununknown](https://github.com/artamrj/ununknown), a local Rust + React app that identified and
fixed a folder of music (the identification engine, review page and safe publishing), and
[sync-me-maybe](https://github.com/artamrj/sync-me-maybe), a Python Telegram bot that fetched
songs from links (link reading, YouTube Music matching, status messages, guests). In November
2026 ununknown became `magpie-fi` and the bot was rewritten in Rust inside it; the Python code was
deleted. Git history has the details of the merge.
