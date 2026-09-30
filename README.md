# mythics.gg Logger (desktop app)

The desktop app that uploads World of Warcraft's combat log to mythics.gg, live and from past logs (#322; scope in `docs/specs/desktop-logger-scope.md`, #321). Built here while it's private; it moves to its own public repository before the first release, so SignPath can sign it.

**What it may do, and never:** it reads only the combat log text files in the game's `Logs` folder. It never reads the game's memory, injects anything, changes game files or settings, or automates play. It's safe beside the Warcraft Logs uploader: both only read the same file (opened read-only, sharing read, write and delete), and this app never changes or deletes it.

## Layout

| Path | What |
|---|---|
| `core/` | The engine, no UI: find the Logs folder (`wowdir`), tail the newest log (`tailer`), cut pulls and keys (`splitter`), compress into 4 MB zstd chunks (`chunker`), the on-disk queue (`queue`), the API client (`api`), sign-in with PKCE and a loopback redirect (`auth`), uploads with resume and backoff (`uploader`), past logs (`backlog`), the speed limit (`throttle`) |
| `core/tests/` | Tests against small made-up logs in `fixtures/` (players "Player1…", real boss names) and a stub HTTP server. Never a real log or a real server |
| `core/examples/measure.rs` | Reads a real Logs folder locally and prints counts, sizes and peak memory only |
| `core/examples/backlog_size.rs` | What a folder of past logs would upload with each Backlog setting, at level 10 and at the backlog's level. Counts, sizes and times only |
| `core/examples/headless.rs` | The upload path without the window, against a local server (`mt10 logger dev-token`): `--backlog <file>` (add `--all-pulls` for every pull in full) or `--live <folder>`. Prints ids, counts and times only |
| `src-tauri/` | The Tauri 2 shell: settings, the token in Windows Credential Manager, the tail thread and upload task, tray, start with Windows, single instance, and the window's commands |
| `src/` | The window: Preact and TypeScript, the site's design tokens and the brand's fonts and purple |
| `test/` | vitest against happy-dom, with a fake app (`test/fake.ts`) |

## Commands

Run from `desktop/`. Needs Rust (stable, MSVC on Windows), Node 22 or later, and WebView2 (part of Windows 11).

```sh
npm ci                                   # never symlink node_modules from another checkout
npm run typecheck && npm test            # the window
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npm run tauri dev                        # the app, against the site in Settings
npm run tauri build -- --debug           # an unsigned installer in target/debug/bundle/nsis
npm run dev                              # the window alone in a browser, with the fake app (?tab=backlog, ?first-run)
```

If `CI=1` is set in your shell, the Tauri CLI refuses it: use `CI=true`.

The site address is a setting in development builds (or with `MYTHICS_LOGGER_DEV=1`), for a local API such as `http://127.0.0.1:8000`. Tests never talk to a real server.

## How it works

- **Segments.** One per boss pull (`ENCOUNTER_START` to `_END`) or Mythic+ key (`CHALLENGE_MODE_START` to `_END`), prefixed with the file's `COMBAT_LOG_VERSION` header line. Bosses inside a key stay in the key's segment. A pull or key without its end goes as kind `segment`. The SHA-256 is of the uncompressed segment, and the server de-duplicates on it.
- **Live.** The tailer follows the newest `WoWCombatLog*.txt` through a 1 MB buffer, holds back a partial last line, finishes the old file when a new one appears, and starts again if the file is cleared. A file changed in the last 10 minutes is read from its start; an older one is left to the Backlog tab. Its place (the start of any open pull) is saved, so a restart loses nothing. A live pull is compressed as soon as it ends and goes before any past log.
- **Live logging on or off.** Off until the player chooses: once logged in, the app asks "Upload your pulls live while you play?" (Yes / Not now), and the answer is kept. The switch is in Settings, on the Live tab when it's off, and in the tray menu ("Turn live logging on/off"); it takes effect within a second, without a restart (`workers::Follower`). Off, the tail thread holds no file open and reads nothing, and the header says "Live logging off"; past logs still go, but only from the Backlog tab. Switched on while the app runs, every log already in the folder is followed from its end, so nothing from before the switch is sent (Backlog is for that); a log the game starts afterwards is read from its start. On at start-up, it carries on from its saved place as above. Pulls already queued when it's switched off still finish uploading.
- **Past logs.** The Backlog tab finds every combat log in the Logs folder and one level below (Warcraft Logs' and Raider.IO's archive folders), or files the player chooses; reads each one, streaming, to list its pulls and keys and what's already uploaded; and queues what the player ticks, with a visibility. Reports are cached by path, size and modification time. Uploads can be paused, and resume after a restart.
- **Smaller past logs** (the owner's decisions, 29 Sep 2026):
  - *Max compression:* past logs compress at zstd level 19 with long-distance matching (window log 27) in pieces of up to 32 MB, two at a time on background-priority threads (`chunker::Profile`, `priority.rs`); live pulls stay at level 10 in 8 MB pieces, to be on the site in minutes.
  - *Kills and each boss's best wipe* (the Backlog setting, on by default; "All pulls" is the other choice): per log file and boss (encounter and difficulty), every kill and the wipe that got the boss lowest go in full; every other wipe goes as a pull summary, a few hundred bytes (`plan.rs`; `POST /api/logger/pull-summaries`, up to 200 a request). Keys, and anything the server couldn't file (no roster hash), always go in full. The History tab shows a summarised wipe as "Wipe, 42.2% (details not uploaded)".
  - *Boss health* for picking the best wipe is the server parser's own rule, ported (`bosshp.rs`); on the owner's 559 real pulls it matched the server on every one (533 equal, 26 with no health on either side).
  - The tab shows about how much the chosen logs would upload with the setting ("about 640 MB"), at the ratio measured below.
- **Ask before uploading.** Before sending, the waiting pulls' and keys' fingerprints (boss or dungeon, start, and a hash of the `COMBATANT_INFO` roster, as the contract's "Ask before uploading" defines it) go to `POST /api/logger/fingerprints` in one request. One a raid member has already uploaded (and the player may see) isn't sent: it shows as "Already on mythics.gg (uploaded by a raid member)", with View linking their copy. A server without the route gets every upload, as before; offline, the question waits with the upload's backoff.
- **Uploads.** `POST /api/logger/uploads` (idempotent by SHA-256; it says which chunks it has, and the chunk size it holds the upload to, which the app cuts at), `PUT …/chunks/{n}` for the rest, then `…/complete`; one the server already has (queued or parsed) sends nothing more. Network and server errors back off from 5 s to 10 minutes, never sooner than a `Retry-After` (the daily quota); `upload_busy`, `chunks_missing` and `hash_mismatch` (cut and send again) are retried; any other refusal is kept with its code and not retried; a refused token signs the app out. Old logs' times, which have no offset, get this computer's.
- **Log sessions.** Every segment of one file carries the same `session_key` (`core/src/session.rs`: the SHA-256 of the file's name and its first line's timestamp), with the file's name (never its folder), so the site shows the file as one log (the contract's "Sessions").
- **View on mythics.gg.** The Live and History tabs group their rows by log (one file), each with "View log" (`/logs/{log}/`), then **Raid**, a heading per boss linked to its page (`/logs/{log}/bosses/{encounter}/`) over its pulls, kills marked, and **Mythic+**, its keys (`/logs/{log}/keys/{fight}/`) with the bosses inside each; every pull has "View" (`/logs/{log}/pulls/{fight}/`). Until the server has parsed an upload, "Processing…". The tray menu's "Open My logs" opens `/account/logs/`. The paths are the server's (`log_url`, `boss_url`, each fight's `url`); `src-tauri/src/links.rs` opens only those four shapes, with numeric ids, and only on the site in Settings (`https://mythics.gg`, or `http://127.0.0.1:…` in development), never an address from anywhere else. While any of tonight's uploads is processing, the app asks for the newest page of uploads every 15 s, doubling to 2 minutes while nothing changes, and not at all otherwise; the History tab does the same for what it shows.
- **Privacy.** The app's own log file (in the app's log folder) holds counts, offsets and error kinds, never a log line, a name or a URL. The token is in the credential store; settings hold the main character and guild shown top right, never a BattleTag.

## Measured

On a real 86 MB log (one +14 key with 4 bosses), locally: split in 0.2 s; zstd level 10 makes it 4.7 MB (18:1) at about 0.2 s per 8 MB chunk, against 7.0 MB (12:1) at 25 ms at the default level 3; peak memory 35 MB for the whole tail, split and compress (15.7 MB at level 3). A 2.6 GB made-up log tails and splits in under 10 s with a peak of 5.2 MB.

The owner's archive of past logs (100 files, 40.6 GB; 559 boss pulls and 236 keys, 36.9 GB of them worth sending), with `examples/backlog_size.rs` (counts and sizes only; level 19 on a quarter of the segments, scaled up):

| | Before (level 10, all pulls) | Level 19 long, all pulls | Level 19 long, kills and best wipes |
|---|---|---|---|
| Sent in full | 36.9 GB | 36.9 GB | 28.4 GB (238 wipes summarised) |
| Uploaded | 2.04 GB | 1.53 GB | 1.19 GB, plus about 0.1 MB of summaries |

Level 19 with long-distance matching does about 1.0 to 1.5 MB of log a second a thread against 27 to 40 MB at level 10, and holds about 140 MB per thread while it works. So the whole archive takes 5 to 8 hours of one thread's time with the default setting, 3 to 4 hours of waiting with the two background threads, against about 20 minutes at level 10 on one. Nobody waits for a past log, and live pulls always go first.

## Still to do

- The server's side (`docs/specs/logger-api.md`, #326): checked end to end locally with `examples/headless.rs` on a real log (backlog, live tail, repeat, delete). Not yet against mythics.gg itself.
- "Only upload my guild's raids and keys": stored, not applied; needs the server.
- Signing (SignPath), the Microsoft Store, auto-update, and the move to a public repo.
- macOS paths and build (`wowdir.rs` has the TODO).
- The Battle.net agent's install list as another place to look for the game.
- Trash between pulls, and a pull's phase: the server's parse.
- Zipped archives in the Backlog tab.
