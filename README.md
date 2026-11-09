# 🐦 magpie-fi

Your own music service. Send a song link or an audio file to a Telegram bot. It gets
downloaded, identified, tagged and filed, and shows up in [Navidrome](https://www.navidrome.org)
on every device. No subscription.

The full plan is in [BLUEPRINT.md](BLUEPRINT.md). This repo joins two earlier projects (with their history):

| Folder | Was | Job |
|---|---|---|
| [`bot/`](bot/) | [sync-me-maybe](https://github.com/artamrj/sync-me-maybe) | Telegram bot: takes links and uploads, downloads the audio |
| [`core/`](core/) | [ununknown](https://github.com/artamrj/ununknown) | Identifies each song, fixes tags, cover and ReplayGain, files it into the library. Has a review UI |
| Navidrome | stock image | Plays the library to apps on your phone, computer and car |

## How it works right now (Phase 0, "glue mode")

```text
Telegram ─► bot ─► MUSIC_ROOT/inbox/ ─► core ─► MUSIC_ROOT/library/ ─► Navidrome ─► your apps
                                        │
                                        └─ unsure? the song waits in the inbox for you to
                                           review at http://127.0.0.1:7331
```

1. The bot downloads into `inbox/`. Files you copy into `inbox/` yourself are handled too.
2. Every 2 minutes the core checks the inbox. Songs it is sure about are tagged and written to
   `library/`, and the inbox copy is removed.
3. Navidrome notices new files in `library/` within seconds.

## Setup

You need Docker with Compose, and a bot token from [@BotFather](https://t.me/BotFather).

```bash
cp .env.example .env
chmod 600 .env
```

Edit `.env`: set `TELEGRAM_BOT_TOKEN`, `ALLOWED_TELEGRAM_USER_IDS`, `MUSIC_ROOT`, `DATA_ROOT`,
`PUID` and `PGID`. A free `ACOUSTID_KEY` is strongly recommended because it identifies songs by sound.

```bash
sudo ./scripts/setup-folders.sh
```

```bash
docker compose up -d --build
```

The first build compiles the core (Rust) and takes several minutes. After you publish a
release (see below) you can use `docker compose pull` instead.

Then:

- **Navidrome:** open `http://<server>:4533` and create the admin user.
- **Bot:** send `/id` to your bot to get your Telegram user ID, then send it a song link.
- **Review UI:** open `http://127.0.0.1:7331` on the server. From another computer, run
  `ssh -L 7331:127.0.0.1:7331 <server>` first.

If you already have a music library, point `MUSIC_ROOT` at a folder where `library/` is that
library. The core writes into it but never overwrites existing files. Make a backup first.

## Folders

```text
MUSIC_ROOT/
├── inbox/              new and unchecked music (bot downloads, your drops)
│   └── .incoming/      bot's unfinished downloads, ignored by the core
└── library/            Navidrome reads this (read-only)
    └── Playlists/
DATA_ROOT/
├── navidrome/          users, play counts, stars: back this up
├── navidrome-backup/   Navidrome's daily backup
├── core/               core database and caches
└── bot/                bot database (queue, buttons)
```

## Known gaps in Phase 0

These are fixed in Phase 1 and 2 (see [BLUEPRINT.md §8](BLUEPRINT.md#8-roadmap)).

- The bot says "Completed" when the download finishes. The song reaches Navidrome a few
  minutes later, or waits in review if the core is unsure.
- Library files are named `Album Artist/Album/Artist - Title.ext`. The final layout from
  BLUEPRINT.md §5 comes in Phase 1.
- Telegram uploads land in the library root as `Artist - Title.ext`.
- Playlist links still download every song, but the bot's `.m3u8` files stay in
  `inbox/Playlists/` with wrong paths, so no Navidrome playlist appears yet.
- Lyrics are off, because sidecar files would be left behind in the inbox.
- The bot only remembers what is still in the inbox. If you send a song again after it was
  filed, it downloads it again. The core then recognises the copy and does not duplicate it.
- Telegram limits bots to downloading files up to 20 MB, so large FLAC uploads fail.

## Development

```bash
cd core && cargo test && (cd frontend && npm ci && npm run build)
```

```bash
cd bot && uv sync --group dev && uv run pytest
```

Each part keeps its own README: [core/README.md](core/README.md), [bot/README.md](bot/README.md).
CI runs only for the part that changed (`.github/workflows/core.yml`, `bot.yml`).

## Releases

Push a tag like `v0.1.0`. GitHub Actions builds `ghcr.io/artamrj/magpie-fi-core` and
`ghcr.io/artamrj/magpie-fi-bot` for amd64 and arm64. Set `MAGPIE_TAG=0.1.0` in `.env` to pin it.
New GHCR packages are private: make them public in the package settings, or run `docker login ghcr.io` on the server.
