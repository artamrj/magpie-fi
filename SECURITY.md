# Security policy

magpie moves and deletes files in your music folder, so its web page must only be reachable by you.

## How it is protected

- **Loopback only.** `compose.yaml` publishes the web page on `127.0.0.1` only, and magpie itself
  refuses a non-loopback address unless the container opts in. Reach it from elsewhere with
  `ssh -L` or Tailscale Funnel, never by publishing the port.
- **Telegram is the login.** Without a bot token the page has no login, so magpie refuses to start
  with `MAGPIE_PUBLIC_URL` but no Telegram. With Telegram, every API call except `/api/health`,
  `/api/login` and `/api/me` needs a logged-in **owner** (guests use the bot and get 403):
  - inside the Mini App, Telegram's signed `initData` (HMAC-SHA256 with a key derived from the bot
    token, at most 24 hours old);
  - in a browser, a one-time link from `/web` (valid 10 minutes) that starts a 30-day session
    (`HttpOnly` cookie, `Secure` over HTTPS). Links and sessions are stored only as SHA-256 hashes.
- **Same origin.** Write requests from another website's origin are rejected; responses carry a
  strict Content-Security-Policy that only allows Telegram's Mini App script and frame.
- **Secrets stay in `.env`** (`chmod 600 .env`). Nothing secret is stored in the database, and the
  database file is created owner-only (`0600`). Bot API errors are logged without their URL, which
  contains the bot token.
- **Container:** runs as `PUID:PGID` (root only to fix folder owners, then `setpriv`), read-only root
  filesystem, all capabilities dropped except the five the entrypoint needs, `no-new-privileges`.

## Reporting a vulnerability

Use GitHub's private security advisory flow on
[artamrj/magpie-fi](https://github.com/artamrj/magpie-fi/security/advisories/new). Do not include
real tokens, private music files or personal file paths in a report.
