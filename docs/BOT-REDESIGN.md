# magpie-fi — Telegram bot redesign (Bot API 10.4)

> Status: **agreed, being built step by step** (Nov 2026). Steps A to E and the main part of F are built. Written against
> Bot API 10.4 (9 Nov 2026).
> Once a phase is built, [BLUEPRINT.md](BLUEPRINT.md) and
> [TECHNICAL-OVERVIEW.md](TECHNICAL-OVERVIEW.md) are updated in the same commit.

The bot works, but it is plain text: one status message with a `▓░` bar, three text buttons and
a "which song is this?" question. Bot API 10.1–10.4 added things that fit magpie-fi well: HTML
widgets inside message bubbles, rich messages, coloured and disabled buttons, ephemeral replies.
This plan rebuilds `src/telegram/` around them. The pipeline, the queue, the rules in
BLUEPRINT §2 and the Mini App stay as they are.

---

## 1. What the new API gives us, and what we use

| Feature (API version) | Use in magpie-fi | Decision |
|---|---|---|
| **HTML widgets** (10.4): `sendWidget`, `editMessageWidgetData`, `answerWidgetQuery`, `widget_query` update | Live **request card** and **review card** inside the chat | **Yes, core of the redesign** |
| **Shared widgets** (10.4): `is_shared`, `sendSharedWidget`, `editSharedWidgetData` | One **library card** whose numbers change everywhere at once | Yes |
| **Ephemeral replies** (10.2–10.4): `ephemeral_message_parameters.callback_query_id`, `BotCommand.is_ephemeral` | Short answers that don't fill the chat: "Not allowed", details, `/id`, `/queue` | Yes, for things that may be lost |
| **Rich messages** (10.1–10.3): `sendRichMessage`, tables, expandable quotes, button blocks | `/start`, `/help`, guests list, failure details | Yes |
| **Button style** (9.4): `style: success / danger / primary` | Pick = green, Stop / None of these = red, Retry = blue | Yes |
| **Disabled buttons** (10.3): `disabled` | Show "Retry failed" greyed out instead of making buttons jump around | Yes |
| **Message effects**: `message_effect_id` (private chats) | 🎉 once when a whole album or playlist is in | Yes, small |
| `sendAudio` | **📲 Send me the file** (roadmap Phase 5) | Yes |
| Private-chat topics (9.3/9.4), topic links (10.4) | Separate "Requests" and "Review" threads | Later, only if owners turn topics on |
| Drafts / streaming (`sendMessageDraft`, 9.3–10.3) | Made for AI text; the card already shows progress | No |
| Checklists (9.1) | A playlist as a checklist | No — the widget does it better, max 30 tasks |
| Custom emoji button icons (9.4) | Nicer icons | Only if the owner has Premium; optional |
| TON wallet, gifts, paid broadcast, Stars, suggested posts, guest mode, communities | — | **No.** Nothing to sell; rule 7 (free only) |

### Widget limits that shape the design

These come from the widgets guide and decide almost everything below:

- **No network inside a widget.** No `fetch`, no external images. Covers can't be loaded from
  `/api/.../artwork/preview`. A widget only knows its own HTML and its state.
- **HTML is fixed at send time.** It can't be replaced, only its state: `chat_data` (per message)
  and `public_data` (shared), **1024 bytes each**. Edits are silent: no "edited" mark, no
  notification.
- **HTML ≤ 32 KB, and ≤ 4 KB after zlib compression.** Each widget must be small, hand-written
  code with no framework.
- **Taps send at most 64 bytes** (`TelegramWidget.request(data)`), one tap per request, 5 requests
  per 5 s. The bot must answer within 15 s with up to 1024 bytes.
- **Production: private chats only**, and widget messages are protected (no forwarding). That's
  fine: the bot is private-chat only already.
- Sandbox: QuickJS, 8 MB memory, 2000 nodes, 400 CSS rules.

So a widget is a small **view**. The state is a compact JSON the bot keeps up to date, and the
widget draws it. Taps go back to the bot as short action codes.

---

## 2. What the user sees

### 2.1 Request card (replaces the status message)

Sent as a reply when a link or file arrives. `chat_data` is updated by the existing 3 s loop
(`status.rs`), now with `editMessageWidgetData` instead of `editMessageText`.

- Header: album or playlist name, artist, source badge (Spotify, YouTube, file…).
- Background: a soft gradient from the **cover's colours**. Once the song has a verified cover,
  the bot reads 3 colours from it (the `image` crate is already a dependency) and puts them into
  `chat_data`. Only from a filed song's cover, which passed the cover gate (rule 3). (Built
  without the thumbnail idea: colours alone keep the state small.)
- A progress ring and one chip per song: waiting · downloading · identifying · in library ·
  needs you · failed. Chips animate when they change. Long playlists show counts plus the last
  few songs, so the 1024 bytes are enough.
- Buttons inside the widget: **Stop**, **Retry failed**, **Details**. They call
  `request("stop:12")`, the same codes as today's `callback_data`.
- Below the bubble (`reply_markup`): one `web_app` button, **🎵 Open in the app**, when there is an
  HTTPS URL (REDESIGN §2.8 promised this and it was never built).
- When it's done: the final state, the 👌/🤷 reaction as today, and for an album or playlist
  of 5+ songs a short "✅ 12 songs added" message with a 🎉 effect.

### 2.2 Review card (replaces "🤔 Not sure which song this is")

- Shows what we know about the file (name, length, what the fingerprint found) and up to
  **3 candidates** as cards: title, artist, album, year, a score bar, length difference
  ("+2 s"), and badges for what supports it (audio match, MusicBrainz, iTunes…).
- Tap a card → `request("pick:<track>:<n>")` → the bot runs the same `select_candidate` as now and
  answers; the card switches to "✅ Filing it" or "needs a cover — finish in the app".
- **None of these** (red) → `drop:<track>`.
- **🎵 Choose in the app** stays as the `web_app` button below, for covers and manual edits.
- No sound preview inside the widget (no network). Optional later: a "▶ 20 s clip" tap that makes
  the bot send a short `sendAudio` clip of the user's own file.

### 2.3 Library card (shared widget)

- One shared widget made with `is_shared`; its `widget_id` is kept in `telegram_state`.
- `public_data`: number of songs and artists, the last 3 songs added, songs on the way, songs
  waiting in Review, health (library writable, yt-dlp, HTTPS). Updated with
  `editSharedWidgetData` at most once a second, only when something changed.
- Sent with `sendSharedWidget` on `/start` and `/library`, and pinned in each owner's chat.
  Guests get a version without health and Review counts.
- `/queue` and `/health` become ephemeral answers that open on the same numbers.

### 2.4 Messages and buttons

- `/start` and `/help` become rich messages: a short heading, the supported sources as a compact
  table, an expandable "How it works" quote, and a button block.
- **Details** becomes an ephemeral rich message with a table of failed songs and their reasons.
  It's no longer kept in the chat history.
- "Not allowed", "Removed", "That was already handled" → ephemeral replies or the plain
  `answerCallbackQuery` toast.
- `/guests`: a rich table with a red **Remove** button per guest and a green **Invite** button.
- Button colours are the same everywhere: green = accept, red = remove or stop, blue = main action.

### 2.5 New: 📲 Send me the file

Built in step F for the bot (the web page button is still to do). A button on a finished single
song that sends the filed song with `sendAudio`:
title, artist, cover as thumbnail, under 50 MB. The file is read from the library and never
changed. This was already on the roadmap (BLUEPRINT Phase 5).

### 2.6 When widgets don't work

The API is two days old. Some clients may not show widgets, and `sendWidget` may fail. So:

- If `sendWidget` fails, the bot falls back to today's text status message, remembers that per
  chat, and tries widgets again after a day.
- Every widget has a plain **caption** ("Album · 7 of 12 in your library") so the message still
  says something in notifications and in clients that can't draw it.
- A switch `MAGPIE_TELEGRAM_CARDS` (default on) as an off switch for the owner.

---

## 3. How the code is rebuilt

"From scratch" means the `src/telegram/` module, not the pipeline. The DB helpers in
`src/acquire/store.rs`, `select_candidate`, `remove_track`, the login in `src/http/auth.rs` and
the Mini App are kept.

### 3.1 Drop `frankenstein`, use our own types

frankenstein 0.52 doesn't know Bot API 10.x (widgets, rich messages, ephemeral, button style), and
we only use it for a few types. New `src/telegram/types.rs`: serde structs for only what we send
and receive (Update, Message, User, CallbackQuery, WidgetQuery, Widget, InlineKeyboardButton with
`style` / `disabled`, ReplyParameters, EphemeralMessageParameters, rich message blocks we use).
Unknown fields are ignored, so new API versions never break parsing. This removes a crate.
**Built in step A**; BLUEPRINT decision #3 is updated.

### 3.2 New layout of `src/telegram/`

| File | Job |
|---|---|
| `mod.rs` | Start, long polling (`allowed_updates`: message, callback_query, widget_query), dispatch |
| `api.rs` | Kept: retries, `retry_after`, token never in errors. Adds `send_widget`, `edit_widget_data`, `answer_widget_query`, `send_rich`, ephemeral helpers |
| `types.rs` | Our Bot API 10.4 types (3.1) |
| `actions.rs` | **One `Action` enum** (`Stop`, `Retry`, `Details`, `Pick`, `Drop`, `Invite`, `Revoke`, `SendFile`) parsed from both `callback_data` and widget `data`, with **one** permission check (today's `may_press`) |
| `intake.rs` | Links and uploads (moved from `mod.rs`, logic unchanged) |
| `commands.rs` | `/start`, `/help`, `/library`, `/queue`, `/health`, `/web`, `/guests`, `/id` |
| `cards.rs` | Builds the compact `chat_data` / `public_data` JSON for request, review and library cards, with a byte-budget trimmer (always ≤ 1024 bytes) |
| `status.rs` | The 3 s loop: same tables, now updates widget state or text (fallback) |
| `rich.rs` | Small builders for rich messages and coloured button rows |
| `texts.rs` | **All** bot text, including the ~25 strings now inline in `mod.rs` / `status.rs` |

Widget HTML lives in `src/telegram/widgets/` (inside `src/`, so the Docker build needs no
change). It's hand-written: plain JS, Telegram theme colours from the `--tg-theme-*` variables,
Persian/RTL-safe (`dir="auto"`). Rust embeds it with `include_str!`. A small Node script
(`web/scripts/check-widgets.mjs`, built-in `zlib`, no new package) runs in `npm run lint` and
fails if a widget is over 32 KB, over 4 KB compressed, or contains a NUL. The card's words live
in the widget itself, since it can't fetch them; everything else the bot says stays in `texts.rs`.

### 3.3 Database (`migrations/0015`–`0017`)

Built as `0015_telegram_cards.sql`, `0016_telegram_review_cards.sql` and
`0017_telegram_library_card.sql`. They were 0013–0015 in the 0.7.0 release; `main` has since
released its own 0013 and 0014, so these follow them.

- `requests.card_kind` (`text` | `widget`) so the loop knows which edit to use.
- `telegram_chats` (chat_id, widgets_ok_until, library_message_id) for the fallback and the
  pinned library card.
- `telegram_state.library_widget_id`.

No existing migration is edited.

### 3.4 Rules this must keep

- Widget taps are user input: they go through the same `Action` parse and permission check as
  buttons. A guest's tap only works on their own songs.
- `chat_data` holds nothing secret (no tokens, no file paths) — the user's client can read it.
- Covers and colours in a card come only from a **verified** cover.
- Ephemeral messages may be lost, so they're only used for things that can also be found
  elsewhere (Details is also in the app).
- One HTTP client, one queue, one tag writer — the bot adds none.

### 3.5 Tests

Extend the existing `FakeTelegram` server in `mod.rs` tests:

- request card: `sendWidget` first, `editMessageWidgetData` on progress, no edit when the state
  is the same;
- fallback to text when `sendWidget` fails;
- widget `pick` / `drop` / `stop` taps, including a guest tapping someone else's song;
- `chat_data` never over 1024 bytes for a 500-song playlist;
- `Action` parsing round-trips and rejects junk over 64 bytes;
- shared library card edited at most once per second;
- texts: every string comes from `texts.rs`.

Live checks on Telegram's **test server** first (widgets work there in groups too), behind an
ignored `live … probe` test.

---

## 4. Order of work

Each step ships on its own and keeps the bot working.

| Step | What | Visible change |
|---|---|---|
| **A. Base** ✅ built | Own types, `actions.rs`, split `mod.rs` into `intake.rs` and `commands.rs`, all text into `texts.rs`, tests for uploads, buttons and review questions | None |
| **B. Messages** ✅ built | Rich `/start` `/help` `/guests` and Details, coloured and disabled buttons, ephemeral Details and `/queue` `/health` `/id`, `web_app` button on owners' status messages, plain-text fallbacks. Guests no longer get a web-page button on review questions (they can't open the page) | Clearer, tidier chat |
| **C. Request card** ✅ built | `widgets/request.html`, `cards.rs`, the loop edits card state and caption, text fallback for a day after a refusal, `MAGPIE_TELEGRAM_CARDS` off switch, 🎉 at the end | The big one |
| **D. Review card** ✅ built | `review.html`, choices with score bars, length difference, audio match and sources; taps choose or remove; the card shows how it ended, also after a choice in the app | Choosing in the chat |
| **E. Library card** ✅ built | Shared widget, `/library` (owners only for now), pinned quietly, refreshed every 10 s on a change, a new shared card when its code changes | Live library at a glance |
| **F. Extras** | ✅ 📲 Send me the file (bot; a web page button is still to do). Not built: optional clip preview, optional topics | — |

Docs: BLUEPRINT §3.3/§3.4 and decision #3, TECHNICAL-OVERVIEW §6/§7 and the settings list
(`MAGPIE_TELEGRAM_CARDS`) are updated with each step. Known drift to fix along the way: the
"📱 Open review" button name, the missing status-message button, "menu button set when an owner
writes", and TECHNICAL-OVERVIEW still saying the bot always shows three candidates.

---

## 5. Open questions

1. ~~Drop `frankenstein` for our own types?~~ Done in step A.
2. ~~Off switch or only the automatic fallback?~~ Both, built in step C (`MAGPIE_TELEGRAM_CARDS`).
3. Should guests get the request card and the library card, or only owners?
4. Private-chat topics ("Requests" / "Review"): worth it, or keep one chat?
