# mythics.gg Logger (desktop app)

The desktop app that uploads World of Warcraft's combat log to [mythics.gg](https://mythics.gg), live and from past logs. Open source under the [MIT licence](LICENSE). Its server is mythics.gg's own, in a separate, private repository; the contract between them is [docs/logger-api.md](docs/logger-api.md), and the planning document is [docs/desktop-logger-scope.md](docs/desktop-logger-scope.md). Issue numbers in the code (#322 and so on), and "mythics.gg issue N" in the history, are that repository's.

Windows only for now. Releases are on this repository's [Releases](../../releases) page, built by its public GitHub Actions (`.github/workflows/build.yml`). They're unsigned until the SignPath Foundation accepts the project (see [Code signing policy](#code-signing-policy)), so Windows SmartScreen warns before the first run.

**What it may do, and never:** it reads only the combat log text files in the game's `Logs` folder. It never reads the game's memory, injects anything, changes game files or settings, or automates play. It's safe beside the Warcraft Logs uploader: both only read the log the game is writing (opened read-only, sharing read, write and delete), and this app never changes or deletes it.

The one change it may make to your logs, and only if you ask: it moves finished logs, compressed as `.zip` files, into `Logs\MythicsLogsArchive`, when you turn on Settings > Archive > "Archive logs once uploaded" or select Archive on a log in the Backlog tab. It never archives the log the game is writing, one changed in the last 10 minutes, one another program has open, or one with pulls still to upload, and it deletes the original only after checking the `.zip` against it. It deletes archived copies only if you turn on "Delete archived logs after" (30, 60 or 90 days), and then only archives it made itself. Both settings are off until you turn them on.

## Layout

| Path | What |
|---|---|
| `core/` | The engine, no UI: find the Logs folder (`wowdir`), tail the newest log (`tailer`), cut pulls and keys (`splitter`), compress into 4 MB zstd chunks (`chunker`), the on-disk queue (`queue`), the API client (`api`), sign-in with PKCE and a loopback redirect (`auth`), uploads with resume and backoff (`uploader`), past logs (`backlog`), the speed limit (`throttle`), archiving finished logs (`archive`) |
| `core/tests/` | Tests against small made-up logs in `fixtures/` (players "Player1…", real boss names) and a stub HTTP server. Never a real log or a real server |
| `core/examples/measure.rs` | Reads a real Logs folder locally and prints counts, sizes and peak memory only |
| `core/examples/backlog_size.rs` | What a folder of past logs would upload with each Backlog setting, at level 10 and at the backlog's level. Counts, sizes and times only |
| `core/examples/headless.rs` | The upload path without the window, against a local copy of the server (its token from `mt10 logger dev-token`, [docs/logger-api.md](docs/logger-api.md) section 9; the server's code is private): `--backlog <file>` (add `--all-pulls` for every pull in full) or `--live <folder>`. Prints ids, counts and times only |
| `src-tauri/` | The Tauri 2 shell: settings, the token in Windows Credential Manager, the tail thread and upload task, tray, start with Windows, single instance, and the window's commands |
| `src/` | The window: Preact and TypeScript, the site's design tokens and the brand's fonts and purple |
| `test/` | vitest against happy-dom, with a fake app (`test/fake.ts`) |
| `docs/` | The API contract and the scope, copied from the mythics.gg repository |
| `.github/workflows/build.yml` | Builds and tests on GitHub's Windows runner; on a `v*` tag, attaches the installer to a release |

## Commands

Run from the repository's root. Needs Rust (stable, MSVC on Windows), Node 22 or later, and WebView2 (part of Windows 11).

```sh
npm ci                                   # never symlink node_modules from another checkout
npm run typecheck && npm test            # the window
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npm run tauri dev                        # the app, against the site in Settings
npm run tauri build -- --debug           # an unsigned installer in target/debug/bundle/nsis
npm run tauri build                      # the release installer in target/release/bundle/nsis
npm run dev                              # the window alone in a browser, with the fake app (?tab=backlog, ?first-run)
```

If `CI=1` is set in your shell, the Tauri CLI refuses it: use `CI=true`.

The site address is a setting in development builds (or with `MYTHICS_LOGGER_DEV=1`), for a local API such as `http://127.0.0.1:8000`. Tests never talk to a real server.

## How it works

- **Segments.** One per boss pull (`ENCOUNTER_START` to `_END`) or Mythic+ key (`CHALLENGE_MODE_START` to `_END`), prefixed with the file's `COMBAT_LOG_VERSION` header line and its latest `ZONE_CHANGE` line before the segment, both byte for byte. The game writes the zone change on entering the instance, long before the first pull, so without it the site can't name the raid ("Unnamed raid", mythics.gg issue 343). A file opened part-way through (a restart, live logging switched on) looks back for that line (`tailer::last_zone_change`), so a pull goes with the same bytes however it was read. Bosses inside a key stay in the key's segment. A pull or key without its end goes as kind `segment`. The SHA-256 is of the uncompressed segment, and the server de-duplicates on it; the fingerprint ("Ask before uploading") is made from the pull's own lines, so the zone line doesn't change it.
- **Live.** The tailer follows the newest `WoWCombatLog*.txt` through a 1 MB buffer, holds back a partial last line, finishes the old file when a new one appears, and starts again if the file is cleared. A file changed in the last 10 minutes is read from its start; an older one is left to the Backlog tab. Its place (the start of any open pull) is saved, so a restart loses nothing. A live pull is compressed as soon as it ends and goes before any past log.
- **Live logging on or off.** Off until the player chooses: once logged in, the app asks "Upload your pulls live while you play?" (Yes / Not now), and the answer is kept. The switch is in Settings, on the Live tab when it's off, and in the tray menu ("Turn live logging on/off"); it takes effect within a second, without a restart (`workers::Follower`). Off, the tail thread holds no file open and reads nothing, and the header says "Live logging off"; past logs still go, but only from the Backlog tab. Switched on while the app runs, every log already in the folder is followed from its end, so nothing from before the switch is sent (Backlog is for that); a log the game starts afterwards is read from its start. On at start-up, it carries on from its saved place as above. Pulls already queued when it's switched off still finish uploading.
- **Past logs.** The Backlog tab finds every combat log in the Logs folder and one level below (Warcraft Logs' and Raider.IO's archive folders), or files the player chooses; reads each one, streaming, to list its pulls and keys and what's already uploaded; and queues what the player ticks, with a visibility. Reports are cached by path, size and modification time. Uploads can be paused, and resume after a restart.
- **Smaller past logs** (the owner's decisions, 29 Sep 2026):
  - *Max compression:* past logs compress at zstd level 19 with long-distance matching (window log 27) in pieces of up to 32 MB, two at a time on background-priority threads (`chunker::Profile`, `priority.rs`); live pulls stay at level 10 in 8 MB pieces, to be on the site in minutes.
  - *Kills and each boss's best wipe* (the Backlog setting, on by default; "All pulls" is the other choice): per log file and boss (encounter and difficulty), every kill and the wipe that got the boss lowest go in full; every other wipe goes as a pull summary, a few hundred bytes (`plan.rs`; `POST /api/logger/pull-summaries`, up to 200 a request). Keys, and anything the server couldn't file (no roster hash), always go in full. The History tab shows a summarised wipe as "Wipe, 42.2% (details not uploaded)".
  - *Boss health* for picking the best wipe is the server parser's own rule, ported (`bosshp.rs`); on the owner's 559 real pulls it matched the server on every one (533 equal, 26 with no health on either side).
  - The tab shows about how much the chosen logs would upload with the setting ("about 640 MB"), at the ratio measured below.
- **Archiving finished logs** (this repository's issue 4, the owner's decisions of 1 Oct 2026). Off until the player chooses: "Archive logs once uploaded" in Settings > Archive archives a log once every pull of it the app queued is uploaded (none waiting, uploading or refused), checked each minute in the background; the Archive button on a log in the Backlog tab archives one at once. A log becomes `Logs\MythicsLogsArchive\<its name>.zip` (`WoWCombatLog-092826_200101.txt` in `WoWCombatLog-092826_200101.zip`), holding the `.txt` under its own name, deflated, ZIP64 when it's big, with the zip comment "mythics.gg Logger archive" (`core/src/archive.rs`). Only a log directly in the `Logs` folder, and never the newest log (the game's), one changed in the last 10 minutes, one with pulls still queued or uploading, or one another program has open (the app first opens it with no sharing at all, which fails if anyone else has it; while archiving it holds it open sharing read only, so nobody can change or delete it). Streamed through a 1 MB buffer at no more than 50 MB a second, on a background-priority thread. The `.zip` is written to `<name>.zip.partial`, read back (its comment, its one entry's name, size and CRC-32 against the original's, and every byte decompressed again), renamed, and only then is the original deleted. Any failure removes the partial `.zip`, keeps the original and tells the player; a log already moved by another tool is skipped quietly. "Delete archived logs after" (Never, the default, or 30, 60 or 90 days) deletes, each hour, only `.zip` files the app made (a combat log's name and that comment) last changed longer ago than that. Settings > Archive shows the folder's size and opens it.
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

- The server's side ([docs/logger-api.md](docs/logger-api.md), mythics.gg issue 326): checked end to end locally with `examples/headless.rs` on a real log (backlog, live tail, repeat, delete). Not yet against mythics.gg itself.
- "Only upload my guild's raids and keys": stored, not applied; needs the server.
- Signing (SignPath, once the Foundation accepts the project), the Microsoft Store, and auto-update.
- macOS paths and build (`wowdir.rs` has the TODO).
- The Battle.net agent's install list as another place to look for the game.
- Trash between pulls, and a pull's phase: the server's parse.
- Zipped archives in the Backlog tab.

## Releases

Every release is built by this repository's own workflow, `.github/workflows/build.yml`, on a GitHub-hosted Windows runner, from the tagged commit and nothing else: typecheck and tests for the window, rustfmt, clippy and tests for the Rust workspace, then the NSIS installer (`npm run tauri build`). Pushing a tag `v<version>` (the version in `src-tauri/tauri.conf.json`, `package.json` and `Cargo.toml`) attaches the installer and its SHA-256 to a draft release, which a maintainer reviews and publishes. Nobody uploads an installer built anywhere else. Every release's notes end with a "Code signing policy" section linking the one below; while `SIGNED` is `"false"` in the workflow's "Draft release" step it says the build is unsigned, and set to `"true"` (once SignPath signs the installer) it carries SignPath's credit line instead.

## Code signing policy

Free code signing provided by [SignPath.io](https://about.signpath.io), certificate by [SignPath Foundation](https://signpath.org).

**Status:** the project has applied to the SignPath Foundation's free programme for open-source projects. Until the application is accepted, releases are unsigned, and each release's notes say so.

- **Committers and reviewers:** [@Bysonuk](https://github.com/Bysonuk). Changes from anyone else come as pull requests and are reviewed before they're merged.
- **Approvers:** [@Bysonuk](https://github.com/Bysonuk), who approves every signing request.
- **Privacy:** see [Privacy](#privacy) below, and the site's privacy policy at <https://mythics.gg/privacy/>. The installer shows the same privacy statement, with the MIT licence, before it installs anything.
- **Builds:** only from this repository's public GitHub Actions workflow (`.github/workflows/build.yml`), from a tagged commit on the default branch. No installer built on anyone's own computer is signed.
- **What's signed:** the app's own installer and executable only (`mythics-logger.exe` and its NSIS installer). Third-party components are included as their authors publish them, under their own licences, and aren't re-signed.

### Privacy

You choose what to upload. Nothing is sent until you log in.

- Live logging is off until you turn it on: once you've logged in, the app asks "Upload your pulls live while you play?" (Yes / Not now). Turn it off at any time in Settings > Live logging, or with "Turn live logging off" in the tray icon's menu; off, the app doesn't read your combat log as you play.
- Past logs go only when you choose them in the Backlog tab.
- Archiving is off until you turn it on. With Settings > Archive > "Archive logs once uploaded", or Archive on a log in the Backlog tab, the app moves finished logs into `Logs\MythicsLogsArchive` as `.zip` files, on this computer only; "Delete archived logs after" (off unless you choose 30, 60 or 90 days) deletes archives it made once they're that old.
- You choose who sees each upload (Public, Guild only or Private) in Settings > Visibility for new uploads, or on each upload in History, where you can also delete it.
- Log out (Settings > Account) and the app sends nothing more.

The installer shows this statement (`src-tauri/installer/privacy-and-licence.txt`, with the MIT licence) before it installs; `test/installer.test.ts` keeps it in step with the app's setting names.

This program connects to no networked system other than the mythics.gg site (`https://mythics.gg`, or another site address set on purpose in its development settings, for testing against a local server), and sends nothing until the player has logged in with Battle.net through that site in their own browser. What it sends, and only as the player chooses:

- **Combat log segments:** the boss pulls and Mythic+ keys from World of Warcraft's combat log text files (`WoWCombatLog*.txt` in the game's `Logs` folder), compressed, each with the visibility the player picks (Public, Guild only or Private). Live logging is off until the player turns it on; past logs go only when the player chooses them in the Backlog tab. A combat log records everyone near the player: other players' names, realms, gear and what they cast.
- **With each upload:** the log file's name (never its folder, which can hold the Windows user name) and a hash of that name and the file's first time (so the site shows one file as one log), the segment's start and end times, the boss or dungeon, its difficulty or key level, its size and SHA-256, a hash of the pull's player list (to ask first whether a raid member uploaded it already), the region, the visibility, and the app's version.
- **Summaries:** for past logs, by default, wipes other than each boss's best go as a summary only: the boss, when, how long, the boss's health at the end, and the player-list hash.

Besides the `Logs` folder, it reads only where World of Warcraft is installed, to find that folder (the game's install path in the Windows registry, and the usual install folders). It never reads the game's memory or its other files, and never changes game settings. It changes no combat log unless you archive logs (above): then it writes `.zip` files into `Logs\MythicsLogsArchive` and deletes each original after checking its `.zip`, and deletes old archives it made only if you turn that on. Archives stay on this computer: nothing about them is sent. It keeps its sign-in in Windows Credential Manager, its settings and upload queue in its own folders, and a log file of its own that holds counts, offsets and error kinds, never a line from a combat log, a name or an address. It never sends anything to anyone but mythics.gg. The site's privacy policy is at <https://mythics.gg/privacy/>.

## Uninstall

Windows Settings > Apps > Installed apps > mythics.gg Logger > Uninstall. Logging out first (Settings > Account > Log out) also removes your sign-in from this computer and ends it on mythics.gg.

The uninstaller removes the app and its Start with Windows entry. It leaves your data behind unless you tick its "Delete the application data" box, which removes both of the app's folders:

- `%APPDATA%\gg.mythics.logger`: settings (`settings.json`), the upload queue (`queue.json` and `chunks\`), the live log's saved place (`tail.json`) and the Backlog tab's cache (`backlog-reports.json`).
- `%LOCALAPPDATA%\gg.mythics.logger`: the app's own log file (`logs\`) and the window's WebView2 data.

Either way, delete those folders by hand to remove them later.

Logs you archived stay where they are, in the game's `Logs\MythicsLogsArchive` folder (for example `C:\Program Files (x86)\World of Warcraft\_retail_\Logs\MythicsLogsArchive`); neither uninstalling nor "Delete the application data" touches them. Each `.zip` holds one combat log: unzip any you want to keep, then delete the folder in File Explorer to remove them. Before uninstalling, Settings > Archive > Open archive folder shows where it is. The sign-in token stays in Windows Credential Manager if you didn't log out: remove it in Control Panel > Credential Manager > Windows Credentials > Generic Credentials, `app-token.gg.mythics.logger`, or with `cmdkey /delete:app-token.gg.mythics.logger`. Your uploads stay on mythics.gg until you delete them there (My logs).

## Licence

[MIT](LICENSE). The mythics.gg name and logo (the icons in `src-tauri/icons/` and the mark in `src/components.tsx`) identify the official app; a fork should use its own.
