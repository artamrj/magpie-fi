# AGENTS.md — magpie-fi

magpie-fi is a self-hosted music service for one household. Links (Spotify, Apple Music, Deezer,
YouTube, Shazam), Telegram files and an inbox folder all go through one pipeline: get the audio →
identify it with free online sources → fix tags, cover, lyrics and loudness → file it into
`MUSIC_ROOT/library`. Navidrome (run by the user, not part of this repo) plays that folder.

One Rust binary (edition 2024, axum + tokio + SQLite via sqlx) serves a React 19 + Vite + TypeScript
page from `web/`, built as a Telegram Mini App. It ships as one Docker image with ffmpeg, fpcalc,
SongRec, rsgain, yt-dlp and Deno inside.

Read before larger changes:

- `docs/BLUEPRINT.md`: what and why (principles, library layout, naming, tags, decisions).
- `docs/TECHNICAL-OVERVIEW.md`: how (pipeline steps, scoring weights, API, tables, file map).
- `docs/REDESIGN.md`: an agreed redesign being built step by step. Check it before touching
  identification, covers or the Mini App.

## Commands

Run from the repo root unless noted. These are exactly what CI runs (`.github/workflows/magpie-fi.yml`).

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --locked        # keep it warning-free
cargo test --locked                        # ~400 tests, ~25 s; needs ffmpeg + ffprobe on PATH
cargo test --locked journeys               # whole journeys through the API (src/http/journeys.rs)
cargo test --locked smart_approval         # one module or test name: faster while iterating

cd web && npm ci && npm run format:check && npm run lint && npm run build   # build = tsc -b + vite
cd web && npm run format                   # fix formatting (Prettier, printWidth 100)

./scripts/dev.sh                           # backend :7331 (cargo watch) + Vite :5173 with /api proxy
```

- `dev.sh` needs `cargo-watch`. Local data lives in `.local/` (gitignored). Drop test songs into
  `.local/music/inbox`.
- Tests marked `#[ignore = "live … probe"]` hit real websites. Never make a normal test use the
  network.
- Missing tools (fpcalc, songrec, rsgain) switch features off and do not crash. A local run can
  behave differently from the Docker image for this reason.

## Rules the code must keep

These come from `docs/BLUEPRINT.md` §2. Breaking one loses or corrupts someone's music.

1. **Never overwrite or `rm` user files.** Write into `library/` only through
   `publish_no_clobber` (`src/application/apply.rs`). User originals go to
   `.magpie-fi/trash/<date>/`. Only magpie-fi's own downloads in `.magpie-fi/staging/` are
   deleted. Files already in the library are indexed, never moved or renamed, except by explicit
   Artists-page actions.
2. **Sure → file, unsure → Review.** Don't loosen the auto-approval rules in
   `src/application/smart_approval.rs` (score ≥ 68, 12-point gap, version match, ≤ 15 s length
   difference, judged through agreeing sources when the match itself has no length and
   refused when no source has one, supported by audio or 2 sources) unless asked. A wrong match must never reach the
   library silently.
3. **Cover gate.** No verified cover means the song is not filed. Keep `core_complete` (title,
   artist, album, verified cover) as the bar on every path to Ready.
4. **Duplicates by proof, never by title alone.** Proof is the same bytes, the same or a
   similar-sounding fingerprint, an ISRC or a MusicBrainz ID, each with lengths within 3 s.
   Only proof from the audio may trash or replace a file. An ISRC or MusicBrainz ID comes from
   the match, so it only sends the song to Review as a possible duplicate.
5. **One of everything.** One tag writer (`infrastructure/media/tag_writer.rs`), one queue (the
   `wanted` table), one HTTP client (`AppState.client`; only Telegram's long polling has its own),
   one place for settings (`src/config.rs`). Extend these; don't add a second one.
6. **No Navidrome API.** magpie-fi only writes files and `.m3u8` playlists.
7. **Free and public only.** No paid APIs, no borrowed or scraped tokens, no DRM circumvention:
   Spotify, Apple and Deezer links give the song list only, and audio comes from YouTube.
   Sources that need a key stay off without one.
8. **Never re-encode lossy audio.** WAV/AIFF → FLAC and repairing damaged files are the only
   exceptions.
9. Write only real MusicBrainz IDs into MusicBrainz tags. Keep names in their own script
   (Persian stays Persian), NFC-normalised.

## How to make common changes

- **New setting**: add `MAGPIE_*` to `src/config.rs` (with a doc comment), `.env.example`,
  `compose.yaml` `environment:`, `src/http/handlers/settings.rs` (if the page should show it),
  and the docs.
- **Schema change**: add the next numbered file in `migrations/` (`0014_…sql`). Never edit a
  migration that already exists: sqlx checksums applied ones, and an edit breaks startup for
  every existing install. Queries use runtime `sqlx::query*` functions, not macros, so no
  `DATABASE_URL` is needed.
- **New source/provider**: put it in `src/infrastructure/providers/`. Send its requests through
  `provider_cache` and the shared client, and respect rate limits (MusicBrainz 1/s, iTunes one
  search per 3 s). Error messages must never contain keys or tokens; tests exist for this, so
  add one for yours.
- **External program**: run it through `infrastructure::process::run` (or `run_blocking` in
  code that is not async) with a timeout, and check it with `process::available`.
- **Changing a file that is in the library**: only through `files::edit_on_copy`, which checks
  the copy and flushes it before the swap.
- **Long background job**: take the workflow slot with `AppState::try_claim_workflow`, so only
  one of inbox cycle, filing, auto-approve or repair runs at a time. Work done inside a request
  goes through `AppState::run_in_workflow`, which frees the slot when the page stops waiting.
  A job that holds the slot runs under `AppState::guard_workflow`, so a panic frees it.
- **Telegram text**: every message the bot sends lives in `src/telegram/texts.rs`.
- **Web page**: API calls go through `web/src/api/client.ts` and React Query hooks in
  `web/src/api/queries.ts`. Types live in `web/src/api/types.ts`. Take colours from Telegram's
  `--tg-theme-*` variables with fallbacks (`web/src/styles/index.css`). Load covers only from
  magpie-fi's own `/api/.../artwork/preview` endpoints, never from catalog URLs. `GET /api/tracks`
  is a paged list with only each song's chosen match; a list that can grow with the library
  must be paged or searched on the server like it, never sent whole. The preview
  runs the same check as filing, so the page only shows a cover that would really be written.

## Code style

- Rust: `anyhow::Result` with `.context(...)` for app code, `thiserror` for typed errors,
  `tracing` for logs. Tests sit in `#[cfg(test)] mod tests` at the bottom of each file and use
  `tempfile` and the file's own `test_pool` helpers. Copy the nearest existing test. Journeys
  that cross several parts (Review → library → edit → rename) live in `src/http/journeys.rs`
  and drive the real router with an HTTP client that cannot reach the internet.
- Plain words everywhere: in UI text, bot messages, comments, docs and commit messages. Say
  "song", "file", "Review", not internal jargon. Comments explain *why*, as in `src/config.rs`.
- Keep the inherited folder names (`application/`, `domain/`, `infrastructure/`, `http/`). Don't
  restructure or rename them (BLUEPRINT decision 14).

## Docs stay true

Any change to behaviour, settings, API routes, tables, tags, file layout or scoring updates the
matching section of `docs/BLUEPRINT.md` and/or `docs/TECHNICAL-OVERVIEW.md` **in the same
commit**. Some sections have already drifted (artists tables, default audio format, `cover.jpg`).
When code and docs disagree, trust the code, fix the doc you touch, and mention it.

## Git

- Conventional Commits with an optional scope: `feat:`, `fix(web):`, `refactor:`, `chore:`.
  Write the subject as the user-visible effect in plain words ("the file decides who is the main
  singer of a duet"). Use body bullets that give the reason.
- You may commit locally once every check above passes. **Ask before** pushing, tagging, opening
  a PR, force-pushing or rewriting history.
- **Releases** (only when asked):
  - Run `scripts/release.sh X.Y.Z` (needs `git-cliff`: `brew install git-cliff`). It bumps
    `Cargo.toml`, `Cargo.lock`, `web/package.json`, `web/package-lock.json` and the
    `MAGPIE_TAG=` example, writes `CHANGELOG.md`, runs the checks, commits
    `chore: release X.Y.Z` and tags locally. It never pushes.
  - The commit subjects become the release notes, so write them for the user to read.
    Preview the next notes with `git cliff --unreleased --tag vX.Y.Z`.
  - `git push --atomic origin main vX.Y.Z` builds and publishes the image and the GitHub
    release. Ask first.

## Ask first

- Adding a Rust crate or npm package. The image and the dependency surface are kept small.
  Dependabot pins TypeScript's major version for typescript-eslint.
- Changing approval thresholds, duplicate rules, library naming or folder layout. Every existing
  library would change.
- Changing the security setup: login (`src/http/auth.rs`), CSP and Origin checks, `compose.yaml`
  hardening, the Dockerfile, or the entrypoint's privilege drop.

## Never

- Commit `.env`, cookies, tokens, `.local/`, `data/`, `music/`, `target/` or `web/dist/`.
- Paste real API keys or bot tokens into code, tests or logs.
- Run an external program without a timeout, or write into `library/` without the no-clobber
  publish.
