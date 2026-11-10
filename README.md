# 🐦 magpie-fi

Your own music service. Send a Spotify, Apple Music, Deezer or YouTube link to your Telegram
bot or paste it in the web page, or drop music into a folder. magpie gets the songs, identifies them, fixes their tags, cover, lyrics and
loudness, files them into a clean library, and [Navidrome](https://www.navidrome.org) plays
them on every device. No subscription.

It is **one program in one Docker image**. You run Navidrome yourself and point it at the
library folder; magpie never talks to Navidrome's API. The design is in
[BLUEPRINT.md](BLUEPRINT.md). magpie-fi grew out of two earlier projects whose history is kept
in this repository: [ununknown](https://github.com/artamrj/ununknown) (identifying and filing)
and [sync-me-maybe](https://github.com/artamrj/sync-me-maybe) (the Telegram bot, rewritten in
Rust inside magpie).

## How it works

```text
links (Telegram, web page) ─► magpie downloads ─┐
files sent to the bot ──────────────────────────┼─► magpie ─► MUSIC_ROOT/library ─► Navidrome ─► your apps
your drops ─► MUSIC_ROOT/inbox ─────────────────┘     │
                                                      ├─ sure?      → filed automatically
                                                      └─ not sure?  → asks you (Telegram or web page)
```

### Getting music from a link

Open **Get music** in the web page and paste a link:

| Service | Song | Album | Playlist |
|---|---|---|---|
| Spotify | ✅ | ✅ | ✅ (as many songs as Spotify's public page shows, about 100) |
| Apple Music | ✅ | ✅ | ✅ |
| Deezer | ✅ | ✅ | ✅ |
| YouTube / YouTube Music | ✅ | — | ✅ |
| Shazam | ✅ | — | — |

For every song magpie first checks whether you already have it (then nothing is downloaded).
Otherwise it finds the official song on YouTube Music (live, cover, remix and sped-up versions are
avoided unless you asked for one), downloads YouTube's own audio stream without re-encoding,
and hands it to the same identification and filing as everything else. A playlist link also
becomes a Navidrome playlist in the same order; sending it again updates that playlist and only
downloads songs you don't have yet. The audio always comes from YouTube; Spotify, Apple Music
and Deezer links are only read for the song list.

### Filing

Every minute magpie checks the inbox and its own downloads. For each new song it:

1. checks that the audio is not damaged and computes its audio fingerprint,
2. identifies it with 15 sources (AcoustID, MusicBrainz, Shazam via SongRec, Deezer, Apple Music,
   Persian catalogs, …; 8 of them need no key),
3. if it is sure, checks the library: if you already have this recording, the new copy goes to
   the trash (unless it is lossless and your copy is not — then it replaces it),
4. otherwise writes the tags, a verified cover and synced lyrics, and files it as
   `Album Artist/Album (Year)/01 - Title.ext`,
5. moves the original to `MUSIC_ROOT/.magpie/trash/` (kept 30 days).

After each batch, rsgain writes ReplayGain (track and album) for every album folder that got new
songs. Navidrome's file watcher shows the songs within seconds.

Songs it is not sure about wait in **Review** on magpie's web page, where you pick the right
match, paste a song link, or type the details. A song that fails while being filed (for example
a file that can't be written) goes to **Issues** instead of blocking the others; **Check & fix**
there tries it again.

## Setup

You need Docker with Compose.

```bash
cp .env.example .env
chmod 600 .env
```

Edit `.env`: set `MUSIC_ROOT`, `DATA_ROOT`, `PUID` and `PGID`. A free `MAGPIE_ACOUSTID_KEY` is
strongly recommended (it identifies songs by sound).

```bash
docker compose up -d --build
```

The first build compiles magpie and takes several minutes (or use a release, see Releases).
On start the container creates `inbox/`,
`library/` and `.magpie/trash/` in `MUSIC_ROOT` and its database folder in `DATA_ROOT`, owned by
`PUID:PGID`. Your existing music files are never re-owned.

Then:

1. **Add music:** copy files or whole folders into `MUSIC_ROOT/inbox`.
2. **Review:** open `http://127.0.0.1:7331` on the server. From another computer, run
   `ssh -L 7331:127.0.0.1:7331 <server>` first.
3. **Listen:** point Navidrome at `MUSIC_ROOT/library` (see below).

### Navidrome

Run [Navidrome](https://www.navidrome.org/docs/installation/) however you like, with its music
folder set to `MUSIC_ROOT/library` (read-only is enough). magpie needs no Navidrome login. These
[options](https://www.navidrome.org/docs/usage/configuration/options/) work well with it:

```toml
Scanner.WatcherEnabled = true       # new songs show up within seconds
Scanner.PurgeMissing   = "never"    # keep play counts and stars of files that moved
AutoImportPlaylists    = true
PlaylistsPath          = "Playlists" # magpie writes playlists to library/Playlists
LyricsPriority         = ".lrc,.txt,embedded"
CoverArtPriority       = "cover.*, folder.*, front.*, embedded, external"
```

On a network share (SMB/NFS) the file watcher may miss changes; keep Navidrome's scheduled scan
on as a safety net.

### Telegram

1. Create a bot with [@BotFather](https://t.me/BotFather) and put its token in `.env` as
   `MAGPIE_TELEGRAM_BOT_TOKEN`.
2. Start magpie, send `/id` to your bot, and put the number in `.env` as
   `MAGPIE_TELEGRAM_OWNERS` (several owners: `123,456`). Run `docker compose up -d` again.
3. Send the bot a song, album or playlist link, or audio files.

Each request gets one message that updates as songs arrive (“2 of 12 in your library ·
3 on the way”). Buttons let you stop, retry failed songs, or see what went wrong. When magpie
isn't sure which song you sent, it asks with up to three choices. Owners can invite guests
with `/guests` (a one-time link). `/help`, `/queue`, `/health` and `/id` do what they say.

**Files over 20 MB** (most FLAC): Telegram only lets bots receive files up to 20 MB
([Bot API docs](https://core.telegram.org/bots/api#getfile)). The bot says so when a file is
too big. Send an MP3/M4A, a link to the song, or copy the file into the inbox folder instead.

### Phone and Telegram Mini App

With an HTTPS address, the web page opens **inside Telegram** (the "magpie" button next to the
message box, and "📱 Open review" on the bot's questions), so you can review songs, fix covers and
follow downloads on your phone. Telegram is the login: owners only, no passwords. See
[SECURITY.md](SECURITY.md).

1. Give the web page an HTTPS address. Simplest, if the server runs Tailscale:
   `tailscale funnel --bg 7331` on the server. Your address is
   `https://<server>.<tailnet>.ts.net`.
2. Set `MAGPIE_PUBLIC_URL` in `.env` to that address and run `docker compose up -d`.
3. In the bot, tap **magpie** next to the message box (owners), or send `/web` for a login link
   for a computer's browser (it works once and keeps that browser logged in for 30 days).

Once Telegram is set up, the web page always needs a login, also on the server itself (send
`/web`). magpie refuses to start with a public address but no Telegram, because the page
would then be open to everyone.

### Music you already have

- **Keep it where it is:** put it in `MUSIC_ROOT/library`. magpie indexes it on start, so new
  copies of songs you already own are recognised, and files them next to it. Your files are not
  moved or renamed.
- **Clean it up:** move it into `MUSIC_ROOT/inbox` instead. magpie re-identifies and re-files
  everything into the new layout, and uncertain songs wait in Review. Make a backup first.
  Navidrome play counts and stars of moved songs are not carried over.

## Folders

```text
MUSIC_ROOT/
├── inbox/                 drop new music here
├── library/               the clean library; Navidrome reads it, only magpie writes it
│   ├── Artist/Album (Year)/01 - Title.flac · 01 - Title.lrc · cover.jpg
│   └── Playlists/
└── .magpie/
    ├── staging/           downloads and Telegram files on their way
    └── trash/             originals and replaced files, by day, emptied after 30 days
DATA_ROOT/
└── magpie/                magpie's database (magpie.sqlite) and the self-updating yt-dlp
```

Keep `MUSIC_ROOT` on one disk, so moving a song from the inbox to the library is instant. Put
`DATA_ROOT` on a local disk (SSD), not on a network share.

## Settings

All settings are in `.env` and start with `MAGPIE_` — see [.env.example](.env.example). The web
page's status panel shows the current values. To change one, edit `.env` and run
`docker compose up -d`. For more detail in the logs, set `RUST_LOG=magpie=debug` (shows what
every source answered for every song).

## Known gaps

What is planned next is in [BLUEPRINT.md §9](BLUEPRINT.md#9-roadmap).

- Spotify playlists: only the songs Spotify's public page shows are read (about the first 100).
- "Stop" in Telegram drops the songs that have not started; a download already running finishes.

## Development

```bash
./dev.sh
```

This runs magpie and the web page with live reload on <http://localhost:5173>, using
`.local/music/inbox` as the inbox. Checks:

```bash
cargo fmt --all -- --check && cargo test
```

```bash
cd web && npm run lint && npm run build
```

How identification works in detail: [docs/TECHNICAL-OVERVIEW.md](docs/TECHNICAL-OVERVIEW.md).

## Releases

Push a tag like `v0.1.0`. GitHub Actions builds `ghcr.io/artamrj/magpie-fi` for amd64 and
arm64. Set `MAGPIE_TAG=0.1.0` in `.env` to pin it.
New GHCR packages are private: make them public in the package settings, or run
`docker login ghcr.io` on the server.
