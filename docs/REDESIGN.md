# magpie-fi — Redesign proposal (Nov 2026)

> Status: **agreed, being built step by step** (§3). It answers four problems found when running
> v0.1.0 on a NAS. [BLUEPRINT.md](BLUEPRINT.md) is updated once a part is built.
>
> Decisions (Nov 2026): keep `Artist/Album` folders built from one identity per artist; everything
> inside one file by default; build in steps, not a rewrite; HTTPS for the Mini App comes from the
> owner's own Cloudflare Tunnel set in `MAGPIE_PUBLIC_URL` (nothing added to `compose.yaml`).

1. The Telegram Mini App does not open inside Telegram; it opens as a web page.
2. Folders: will duplicates be found when an artist or album is spelled differently? Would one
   flat folder be better?
3. Identification: too many songs need review, and a chosen match "needs a cover" again right
   after choosing it. The design is not smart, efficient, reliable or stable enough.
4. One song becomes several files (song, `cover.jpg`, `.lrc`). Everything should be in one file.

---

## 1. What is wrong today

### 1.1 Mini App

- `/web` answers with a **link button** (`src/telegram/mod.rs:475`), which Telegram opens in the
  browser. A Mini App needs a `web_app` button.
- The real Mini App button (next to the message box) is only set when `MAGPIE_PUBLIC_URL` is an
  `https://` address the phone can reach, only at startup and only for owners (`mod.rs:91-101`); a failure is
  only a log line. `/start` has no button at all.
- Telegram only opens Mini Apps from a public HTTPS address. The NAS publishes the page on
  `127.0.0.1` only, so without a tunnel (Tailscale Funnel or Cloudflare Tunnel) there can be no
  Mini App. Nothing tells the owner this.

### 1.2 The cover that is "needed again" (the main reason songs go to Review)

One bug causes both complaints. Every chosen or auto-matched song must have a verified cover
before it is Ready, and the check rejects covers that belong to the chosen match itself:

1. Choosing runs `normalize_release_fields`, which rewrites the album: `Farangis - Single` →
   `Farangis` (`metadata_completion.rs:275-300`).
2. The cover entries were copied at scan time with the raw album (`X - Single`) or no album.
3. `artwork_matches_candidate` (`artwork.rs:372-410`) compares albums as raw text and needs a
   similarity of 0.72 (`metadata_completion.rs:510-518`). `Farangis` vs `Farangis - Single` is
   0.53, `Shape of You` vs `Shape of You - Single` 0.63: **every single with a short title fails.**
   Entries with no album (Navahang, most Radio Javan singles, Genius, YouTube) always fail.
4. The match's own cover URL is deduplicated away by the stale copy (`artwork.rs:56`), the
   fallback search uses the same check, and on failure `cover_url` is erased (`artwork.rs:232`).
5. The cover previews use different, looser logic, so the page shows a cover while the server
   refuses it.

The same check runs during automatic scans, so correctly identified songs land in Review with
"could not find cover".

### 1.3 Why identification is fragile

- **Four different "do these agree?" rules**, two different text-similarity functions and about
  seven separate provider-trust tables. They disagree with each other (iTunes `X - Single` and
  Deezer `X` agree in one place and not in another).
- **One blended score** mixes proof (fingerprint, ISRC) with hints (album similarity, number of
  sources). Missing optional data counts against a match instead of being left out.
- **Sources are asked one after another** (~12 per song). After a single timeout or 429 a source
  is switched off for the rest of the run, so songs later in a batch get fewer sources and go to
  Review, and are never retried. iTunes ignores this switch and keeps hitting its rate limit.
- **Unavailable is treated as "no match".** A source being down sends songs to Review.
- **Persian names**: `گوگوش` vs `Googoosh`, `ي/ی`, `ك/ک`, ZWNJ, transliteration variants are
  treated as different artists and different recordings ("ambiguous release").
- **State**: decision data lives in a free-form JSON string changed by five functions; only the
  top 5 candidates are kept (Undo has nothing to go back to); user actions (choose, cover,
  manual edit) run without a lock against a scan or a filing in progress, so a rescan can delete
  the candidate the owner just chose; some writes are not in a transaction; one track can be
  "review" and "verified" at once and get stuck.

### 1.4 Folders and duplicates

- Duplicates are **never** found by folder name. The library index matches by identical file
  hash, or duration ±3 s plus fingerprint key, ISRC or MusicBrainz recording ID. A flat folder
  would not find one more duplicate.
- The real weaknesses are elsewhere:
  - The fingerprint key is a hash of the whole fingerprint, so two encodes of the same song (a
    YouTube download and a FLAC) never match. Without an ISRC or MusicBrainz ID the same song
    can be filed twice.
  - Artist spellings (`گوگوش` / `Googoosh`, `Beyoncé` / `Beyonce`) create two artist folders,
    and files already filed never move when the preferred spelling changes.
  - Album folders compare names exactly, and songs of one new album in one batch can get two
    years and split.
  - Replacing a lossy file with lossless does not update playlists that point at the old file.
- A flat folder would also break things: Navidrome uses any `cover.jpg` in a folder for every
  album in it, and album ReplayGain is measured per folder.

### 1.5 Extra files per song

- The cover is **already embedded** in every song (and checked after writing); `cover.jpg` per
  album folder is extra.
- Lyrics are only written as `.lrc` / `.txt` next to the song. Worse, retagging rebuilds the tags
  from scratch and **throws away lyrics already embedded** in the original file.
- Navidrome (v0.64) reads embedded lyrics in every format we write and treats LRC timestamps
  inside them as synced lyrics: `LYRICS` (FLAC/Opus/Ogg), `©lyr` (M4A), `USLT` (MP3). It uses
  the embedded cover when no `cover.*` image is in the folder.

---

## 2. The new design

Ideas taken from tools that have solved this for years: beets and Lidarr (weighted distance,
strong-ID shortcut, thresholds with a margin), MusicBrainz Picard (recording first, release
second), Harmony (collect claims from every source, then merge with recorded disagreements).

```text
 file ─► Probe ─► Gather ─► Resolve ─► Enrich ─► Write ─► File ─► Index
          │         │          │          │         │        │
          │         │          └─► Review (only real doubt, with the reason)
          │         └─► Waiting for source (a source was down: retried later, not Review)
          └─► Issues (cannot be decoded)
```

### 2.1 One item, one state machine

- One row per song (`items`): `stage`, `version`, `attempts`, `next_retry_at`, `last_error`,
  and the audio fingerprint as the identity of the item.
- Stages: `probed → gathered → resolved → enriched → written → filed`, plus `review`,
  `waiting_for_source`, `failed`.
- **Every change goes through one command function** that checks the `version` it read
  (optimistic locking). A scan, a filing and the owner's click can never overwrite each other:
  the loser re-reads and tries again or reports "this song changed, reload".
- Each stage is a pure function of what is stored plus cached source answers, so re-running a
  stage gives the same result.

### 2.2 Sources give claims, not decisions

- Every source becomes a `Provider` with declared abilities (`by_fingerprint`, `by_isrc`,
  `by_text`, `cover`, `lyrics`) and returns **claims**: title, artists, album, release type,
  date, ISRC, MusicBrainz IDs, provider IDs, length, and the covers that come with that exact
  record.
- All claims are stored (`claims` table), not only the top 5. Undo and "show other matches"
  always work, and re-deciding after a rule change needs no network.
- Every provider is wrapped the same way:
  - its own **rate limit** (MusicBrainz 1/s, iTunes ~20/min, …);
  - **timeout**, **retry with backoff** that honours `Retry-After`;
  - a **circuit breaker**: after repeated failures the source pauses for a few minutes and its
    answer is "unavailable", never "no match";
  - a **cache** with one request in flight per question (six songs asking the same thing make
    one call), and short lifetimes for empty answers.
- Sources are asked **in parallel** with a deadline per song. A strong proof (below) stops the
  search early.

### 2.3 Resolve: proof first, then a distance, then a human

One `matching` module replaces the four agreement rules, two similarity functions and seven
trust tables.

1. **Proof (accept at once when consistent)**
   - AcoustID fingerprint match with high score and length within 3 s;
   - the same ISRC from two independent sources;
   - a MusicBrainz recording ID already in the file's tags.
2. **Distance** (beets-style, 0 = identical): weighted parts for title, artist, length, album,
   source trust. A part is only counted when **both** sides have it; missing data lowers
   confidence, it is never a penalty. Each part's value is stored, so Review can say exactly
   what disagrees ("length differs by 22 s", "artist spelled differently").
3. **Decision**
   - auto: proof, or distance below the strong threshold **and** clearly better than any match
     that is a *different recording*;
   - review: real doubt, shown with the reason and all claims;
   - waiting for source: a source that could decide it was unavailable; retried automatically.
4. **Recording first, release second.** Once the recording is known, the release is chosen by
   preference: original album, then single, then EP; compilations last; earliest date.

All thresholds and weights are in one place and covered by tests built from real claims
(including Persian songs), so a change can be measured before it ships.

### 2.4 Names: one identity per artist

- **One match key** used everywhere (never written to tags): NFKC; `ي→ی`, `ك→ک`, `ة→ه`, heh and
  alef variants unified; tatweel, diacritics, ZWNJ and direction marks removed; spaces collapsed;
  lowercase.
- **Latin key** for transliterations: `ou/oo→u`, `ee/ie→i`, `w→v`, `gh/q→q`, doubled letters
  collapsed; plus a consonant skeleton to compare Persian script with a Latin spelling.
- **Artist identity table**: one row per real artist (MusicBrainz ID or provider IDs) with
  every spelling ever seen, one display name and one folder name. Similarity is the best match
  over all known spellings. The owner can merge two artists in the web page.
- Folders come from the identity, so they never fork. When the display name changes, the
  artist's files are moved (no retag in the same step, so Navidrome keeps play counts).

### 2.4b Songs with several artists

**Today.** Text from a source is only split at ` / `, `feat.`, `ft.` and `;`. `&`, `x`, `,` and
`و` stay inside one name on purpose (`Simon & Garfunkel` is one band). So `Ebi & Shadmehr Aghili`
from iTunes becomes one artist: its own folder, its own artist in Navidrome, and the song is
missing from both singers' pages. Only sources that list artists separately (MusicBrainz, Spotify)
give two artists. The folder uses the album artist text as written, so `Ebi & Shadmehr Aghili/`,
`Shadmehr Aghili & Ebi/` and `Ebi/` can all appear for one duet.

**Like Spotify and Apple Music.** A song has an ordered list of artists, each a real artist
identity; the title never carries `(feat. …)`; the song shows on every artist's page; the album
belongs to its album artist(s). Navidrome (since 0.55) works the same way when the tags say so:

| Tag | Example | Navidrome uses it for |
|---|---|---|
| `ARTIST` | `Ebi & Shadmehr Aghili` | the display text, exactly as credited |
| `ARTISTS` (one value per artist) | `Ebi`, `Shadmehr Aghili` | linking the song to each artist |
| `ALBUMARTIST` / `ALBUMARTISTS` | `Ebi` | which artist the album belongs to |
| `MUSICBRAINZ_ARTISTID` (one per artist) | two IDs, same order | stable artist identity |

**The rules**

1. **Split by identity, not by character.** A credit is split at `&`, `x`, `,`, `و`, `،`, `with`,
   `vs.` only when every part is a known artist identity (§2.4) and the whole is not one (a band
   such as `Simon & Garfunkel` or `Earth, Wind & Fire`, known from MusicBrainz or the owner).
   Structured lists from a source are used as given.
2. **One folder per primary artist.** The folder is the first album artist's identity folder:
   `Ebi/Song (2024) [Single]/01 - Song.m4a`, never `Ebi & Shadmehr Aghili/`. The second singer
   gets the song through `ARTISTS`, not through a folder. Compilations stay in `Various Artists/`.
3. **Featured artists** are moved out of the title into `ARTISTS` (`feat.` stays in the `ARTIST`
   display text).
4. **The owner can fix it once**: "these are two artists" or "this is one band" in the web page,
   stored in the identity table, so it applies to every later song.

**Duplicates with several artists.** A recording is the same when:

- the audio says so: AcoustID or fingerprint *similarity* (step C replaces the exact hash), ISRC,
  or MusicBrainz recording ID; credits play no part in this; or
- without that proof: the **set** of artist identities overlaps on the primary artist, the title
  key (without `feat.` and version noise) matches, and the length is within 3 s. Order and
  spelling do not matter, so `Ebi & Shadmehr – Song`, `Shadmehr feat. Ebi – Song` and
  `Ebi – Song (feat. Shadmehr)` are one song. This rule only sends the new copy to Review as a
  possible duplicate; it never deletes anything by itself.
- A version word (`Remix`, `Live`, `Acoustic`, a different featured artist on a remix) keeps them
  apart, as today.

### 2.5 Covers: separate, explained, never lost

- A cover that comes **with the winning record** (same provider, same record) is accepted
  without a text comparison. Cover Art Archive by release ID is accepted the same way.
- Other covers need two sources to agree (perceptual hash), or the cover already embedded in the
  file, or the owner's choice.
- Square, at least 500 px; embedded at about 1200 px JPEG.
- The cover has its own status next to the match. A known recording without an accepted cover
  shows "cover needed: <reason>", keeps every option it found and never erases one. Previews
  and the check use the same function, so what the page shows is what will be written.

### 2.6 One file per song

- Default: **everything inside the audio file**: tags, cover and lyrics (LRC text with
  timestamps in `LYRICS` / `©lyr` / `USLT`). No `cover.jpg`, no `.lrc`, no `.txt`.
- Lyrics already embedded in an original are kept.
- `MAGPIE_SIDECARS=true` keeps writing `cover.jpg` and `.lrc` for players that need them.
- A one-time tidy step embeds existing `.lrc` / `.txt` files and moves the sidecars to the
  trash.

### 2.7 Folders and duplicates

- **Keep `Artist/Album (Year)/NN - Title`**, built from the artist identity (§2.4). Navidrome
  groups by tags anyway; folders stay readable on the NAS and in folder-based players, and per
  album ReplayGain keeps working.
- Album name and year are decided from the database, including the other songs of the same
  batch, so an album never splits.
- Duplicates are found by recording, not by folder: AcoustID ID or fingerprint similarity (not
  a hash of the whole fingerprint), ISRC, MusicBrainz ID. A "probably the same song" rule (same
  artist identity, same title key, length within 3 s) sends the song to Review as a possible
  duplicate instead of filing it twice.
- Replacing a file updates every playlist that points to it.

### 2.8 Mini App

- `/start`, `/web` and status messages get a **Mini App button** (`web_app`) when an HTTPS
  address is set; the login link stays for normal browsers.
- The menu button is set again when an owner writes to the bot, and errors are logged.
- `/health` and the web page say plainly when `MAGPIE_PUBLIC_URL` is missing or not HTTPS, and
  how to get one on a NAS (Tailscale Funnel or Cloudflare Tunnel).

---

## 3. How to get there

Built step by step, each step shippable and tested, instead of one big rewrite: a rewrite would
lose the many small lessons already in the code (Persian tags, YouTube quirks, safe publishing).

| Step | What | Fixes | Status |
|---|---|---|---|
| A | Mini App buttons and warnings; cover bug (own-record covers, release-aware album compare, one check for preview and filing); stuck tracks; sources pause instead of switching off; iTunes rate limit | 1, most of 3 | **built** |
| B | One file per song (embedded lyrics, keep existing lyrics, optional sidecars, tidy step) | 4 | **built** |
| C | Names module and artist identity; several artists per song (§2.4b); folders from the primary artist's identity; batch-aware album/year; better duplicate detection; playlist update on replace; Artists page | 2 | **built** |
| D | Provider layer: abilities, rate limits, breakers, single-flight cache, parallel with deadline | 3 (speed, reliability) | |
| E | Claims, resolver and item state machine with optimistic locking; Review shows the reason; automatic retry of "waiting for source" | 3 (smart, stable) | |

Notes on C: a duet whose singers are both unknown when it is filed is learned as one artist; it
splits automatically for later songs once both are known, and the Artists page splits the ones
already filed. Fingerprints only cover a song's first 2 minutes (fpcalc's default), and a
sped-up or pitch-shifted upload does not match by sound.

Notes on A: a refused cover's URL is still cleared from `cover_url`, but every option stays in the
candidate's cover list, so nothing is lost. Songs scanned while a source was paused are not yet
re-checked automatically when it comes back; that is step E ("waiting for source").

Steps A and B are independent and small. C, D and E build on each other and touch the same
code, so they go one after another.
