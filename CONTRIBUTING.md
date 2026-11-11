# Contributing to magpie-fi

Thank you for helping. magpie-fi is a small project for one household's music, so changes
stay small and plain.

## Report a problem or ask for a feature

Open an [issue](https://github.com/artamrj/magpie-fi/issues/new/choose). For a problem, say
what you sent (a link, a file, a folder), what happened and what you expected. The bot's `/health`
command and `docker compose logs magpie-fi` usually show the reason.

Never paste your bot token, API keys or `cookies.txt` into an issue.

## Change the code

1. Read [AGENTS.md](AGENTS.md). It holds the rules the code must keep (never overwrite a
   user's file, unsure songs go to Review, and so on) and how to make common changes.
2. Start the app locally:

   ```bash
   ./scripts/dev.sh
   ```

   It needs Rust, Node.js, `cargo-watch` and ffmpeg. Local data goes to `.local/`.
3. Before you open a pull request, run what CI runs:

   ```bash
   cargo fmt --all -- --check && cargo clippy --all-targets --locked && cargo test --locked
   ```

   ```bash
   cd web && npm ci && npm run format:check && npm run lint && npm run build
   ```

4. If you change behaviour, settings, routes or tables, update `docs/BLUEPRINT.md` or
   `docs/TECHNICAL-OVERVIEW.md` in the same commit.
5. Write commit subjects as [Conventional Commits](https://www.conventionalcommits.org) that
   say what the user will notice, in plain words. They become the release notes.

## Ask first

Open an issue before you start on any of these, because they change every existing library
or the image:

- a new Rust crate or npm package
- approval thresholds, duplicate rules, library naming or folder layout
- login, security headers, `compose.yaml` hardening or the Dockerfile
- a source that needs a paid API, a borrowed token or breaks copy protection (these are not
  accepted)
