# mythics.gg Desktop Logger: scope

> **A shortened public copy.** The full planning document (29 Sep 2026), with the server side, hosting and costs, stays in the mythics.gg repository, which is private. This copy keeps the owner's decisions and the parts about the app: what it does, what it uploads, and privacy. It's the plan as written then: where it differs from the code or from the API contract ([logger-api.md](logger-api.md)), those win. Issue numbers (#322 and so on) are the mythics.gg repository's.

A desktop app, like the Warcraft Logs uploader (now the Archon app), that reads the combat log World of Warcraft writes to disk and sends it to mythics.gg. The site stores the log, reads it, and builds pages from it: pulls and kill times, gear and talents, timelines, and the Race to World First as it happens. The owner's request (29 Sep 2026): a desktop app that players and staff sign in to, that uploads the combat log live and as backlogs of old logs, "so we get our own logs from now on".

## Decided, 29 Sep 2026

The owner's decisions, on #322:

| Topic | Decision |
| --- | --- |
| **Open source, signing and distribution** | The app is open source, in its own public repository (this one). Windows builds are signed free through the SignPath Foundation, from a public GitHub Actions build, with the Microsoft Store as a second channel. macOS comes later. No paid Windows certificate |
| **Visibility** | Each upload is **Public**, **Guild only** or **Private**, with a default in Settings. A Private log is visible only to its uploader: never on public pages or in rankings, and seen by staff only to handle a report or a removal. The uploader can delete any of their logs |
| **Retention** | Raw logs are kept 1 year |
| **Staff preview first** | Data from the app's uploads goes into a staff-only preview of the site first (#323), before any public page uses it |
| **Reading the log** | Go ahead, as Warcraft Logs and Archon do. The app says plainly that it's safe to run alongside the Warcraft Logs uploader |
| **Server API** | Built to a fixed contract: [logger-api.md](logger-api.md). Where this page differs from it, the contract wins |
| **Licence** (1 Oct 2026, #417) | MIT |

Later decisions (29 Sep 2026) are in the code and the README: live logging is off until the player chooses; live pulls compress at zstd level 10 and past logs at level 19 with long-distance matching; past logs send kills and each boss's best wipe in full and other wipes as summaries by default; the app asks the site before sending whether a raid member already uploaded a pull.

On 1 Oct 2026 (this repository's issue 4) the owner decided the app may archive finished logs, which ends the plan's "never moves or deletes a log": off until the player turns it on, or selects Archive on a log, it moves a finished log into `Logs\MythicsLogsArchive` as a `.zip` and deletes the original only after checking the `.zip`; never the log the game is writing, one another program has open, or one with pulls still to upload. An optional clean-up, off by default, deletes the app's own archives after 30, 60 or 90 days. Where this page says the app never moves or deletes a log, that's the rule except for archiving the player chose. The README says exactly what it does.

Also on 1 Oct 2026 (this repository's issue 7; mythics.gg issue 419) the owner decided the app may install the mythics.gg in-game addon and keep it up to date, which ends the plan's "never changes addon folders" for the addon's own folders only. The app asks once, "Install the mythics.gg addon?"; Yes installs it and turns on "Keep the addon up to date", Not now leaves it off. Releases come from mythics.gg (`/data/addon/latest.json` and the zip it names), not CurseForge. The zip's size and SHA-256 are checked before anything is extracted, every entry is checked, and each of the addon's folders is replaced whole, only while the game isn't running; a data pack of ours that a new release no longer has is removed in the same step. Never another addon's folder, and never `WTF`.

## 1. Who uses it

| Who | What they do in the app |
| --- | --- |
| **Player** | Uploads their own logs (live or past logs) and picks Public, Guild only or Private |
| **Guild raid leader or officer** | The guild's logger: a log records everyone in range, so one logger can record every raid member's pulls (as guilds use Warcraft Logs today) |
| **Staff** | Upload as anyone else |

**Sign-in:** one login everywhere. The app signs in with Battle.net through the site's own sign-in, in the player's browser, with PKCE and a redirect to a port on the player's own computer (RFC 8252, OAuth for native apps). The app never holds the site's Battle.net client secret or a Battle.net token, only an app token for mythics.gg, kept in the operating system's credential store (Windows Credential Manager). The token can upload and manage the player's own uploads, nothing else, and can be revoked. The details are [logger-api.md](logger-api.md), section 1.

## 2. How WoW's combat log works

### Turning it on

| Setting | What it does |
| --- | --- |
| `/combatlog` | Starts writing every combat event the client sees to a text file. Typing it again, logging out, disconnecting or exiting the game stops it, so it must be turned on each session (addons such as Loggerhead, or DBM's auto-logging, do it) ([Warcraft Logs help](https://www.warcraftlogs.com/help/start), [Warcraft Wiki](https://warcraft.wiki.gg/wiki/COMBAT_LOG_EVENT)) |
| Advanced Combat Logging (System > Network) | Adds a block of fields to most events: the unit's GUID, owner, health, power, position and more. Also turns on `COMBATANT_INFO` (gear and talents). Persists across sessions |

The app checks the log's header and warns when `ADVANCED_LOG_ENABLED` is 0. It can't turn either setting on: that would mean changing game settings or automating the game (section 5). It tells the player how.

### The files

- **Where:** `<WoW install>\_retail_\Logs\` on Windows, typically `C:\Program Files (x86)\World of Warcraft\_retail_\Logs\`. Classic and test clients are out of scope.
- **Names:** since about patch 9.0.5 (March 2021), retail writes a new file each time logging starts, named `WoWCombatLog-MMDDYY_HHMMSS.txt`. Older logs, and many guides, use a single `WoWCombatLog.txt` that grows without end (one player's reached 42 GB). The app reads both, and orders files by modification time, not by name.

### The line format

Each line is a timestamp, **two spaces**, then a comma-separated event:

```
9/28/2026 20:15:01.123-4  SPELL_DAMAGE,Player-1403-0A1B2C3D,"Name-Realm-EU",0x512,0x0,Creature-0-…,"Boss",0x10a48,0x0,…
```

- **Timestamp:** the computer's local time. Current retail writes month/day/year, milliseconds and the UTC offset; older logs have no year and no offset.
- **Header:** `COMBAT_LOG_VERSION,22,ADVANCED_LOG_ENABLED,1,BUILD_VERSION,12.0.0,PROJECT_ID,1` in current retail.
- **Names** are quoted when they hold special characters, so a reader must handle quoted commas.
- **The events the app keys on:** `ENCOUNTER_START` and `_END` (a boss pull, kill or wipe), `CHALLENGE_MODE_START` and `_END` (a Mythic+ key), `COMBATANT_INFO` (each player's gear and talents at the pull) and `ZONE_CHANGE` (which raid or dungeon).

### What "live" really means

The game buffers the file: it writes when enough events have piled up, not on a timer. In a raid pull events arrive by the thousand each second, so the file updates every few seconds, but `ENCOUNTER_END` can sit in the buffer for a while after a wipe. The aim is a pull on the site within a few minutes of it ending: "per pull", not "per second". The last pull of the night is written when logging stops; a forced close of the game can lose what was still buffered.

## 3. The app

### Framework

**Tauri 2**: a Rust core with a small web window in Preact and TypeScript, the stack the site uses. Players run it during raids, so memory and CPU matter more than for most apps; the installer is small; and the hot path (tailing, hashing, compressing, uploading) is the kind of code Rust does well.

### Finding the logs folder

In this order, stopping at the first that has a `Logs` folder:

1. The folder the player chose before.
2. **Windows:** the registry value `InstallPath` under the World of Warcraft key (the 32-bit view), normalised (it points at `_retail_`, sometimes without a drive letter or with a trailing backslash).
3. The default install folders.
4. Otherwise the player picks the folder, and the app says what's wrong in plain words.

### Tailing live, with rotation

- **Open read-only and share everything** (on Windows `FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE`), so the game keeps writing and anyone can still rename or delete the file. The app never locks, moves or deletes the log it follows (archiving, decided later, only takes finished logs: see above).
- **Read from the last offset** in fixed buffers, and hold an incomplete last line until its newline arrives.
- **Rotation:** a new `WoWCombatLog-*.txt` means a new session; the app finishes the old file and moves to the new one. A file that shrinks or is replaced (another uploader's "clear logs after upload") is read again from the start.
- **Resume after a restart:** the app keeps its place in each file, so a restart loses nothing.

### Splitting into pulls and keys

The app reads only enough to find boundaries; the site does the real reading. Each boss pull and each Mythic+ key is a **segment**: whole lines, byte for byte as the game wrote them, with the file's header line. The app never edits lines.

### Compression and upload

- **zstd** per segment, with the SHA-256 of the segment.
- **Resumable, chunked uploads**, idempotent: sending a segment twice is harmless; the site keeps one.
- **Offline queue:** if mythics.gg or the network is down, segments wait on disk and go when it's back. The app shows what's waiting.
- **Bandwidth:** a speed limit setting exists for streamers.

### Past logs (the backlog)

- The player picks files, or the app lists the logs it finds. It shows each file's date, size, pulls and keys before anything is sent, and the player chooses what goes.
- **Duplicates:** before sending, the app asks the site which pulls it already has, so the same pull uploaded by several people in a raid is sent once.
- Past logs upload at low priority, in the background, and never count as live.

### Tray, start-up and updates

| Part | Plan |
| --- | --- |
| Tray | Closes to the tray; a small window shows the current session and the last pull, with a link to it on the site |
| Start with the computer | Off by default; a setting turns it on |
| Updates | Later: signed updates, never mid-raid |
| Windows signing | The SignPath Foundation (decided); see the README's code signing policy |
| Microsoft Store | A second channel (decided) |
| macOS | Later |

### Settings

| Setting | Default | Notes |
| --- | --- | --- |
| Logs folder | Found automatically | Above |
| Visibility (**decided**) | Chosen in Settings; can be changed per upload | **Public:** on the site, and counts for rankings. **Guild only:** the uploader and the guild; never on public pages. **Private:** only the uploader; never on public pages or in rankings |
| Delete a log | – | The uploader can delete any of their logs from the app or the site |
| What to upload | Raids and Mythic+ only | |
| Delete my logs after upload | Not offered | Many players use Warcraft Logs too; deleting could break it |
| Archive logs once uploaded (1 Oct 2026, issue 4) | Off | Moves a finished log into `Logs\MythicsLogsArchive` as a `.zip`, checked before the original goes. Skips any log another program (such as the Warcraft Logs uploader) has open |
| Delete archived logs after (1 Oct 2026, issue 4) | Never | 30, 60 or 90 days; only archives the app made |
| Keep the addon up to date (1 Oct 2026, issue 7) | Off unless the player says yes to "Install the mythics.gg addon?" | Installs each new version of the mythics.gg addon from mythics.gg into `_retail_\Interface\AddOns` while the game isn't running; only the addon's own folders |
| Upload speed limit | No limit | |

The app explains, in its own words, what gets uploaded: "Your combat log records everyone near you: other players' names, realms, gear and what they cast. We use it to build rankings and guild pages. Private logs are never shown on the site." With a link to the privacy policy.

## 4. Privacy

### Whose data is in a log

A combat log records **everyone near the logger**: other players' names, realms, GUIDs, gear, talents, positions and every spell they cast. Most of that is already public on the Armory and Warcraft Logs, but the people in a log didn't choose to upload it. So:

- **Opt-in:** nothing leaves a computer unless its owner installs the app, logs in and chooses what to send (live logging is off until they turn it on).
- **Visibility (decided):** each upload is Public, Guild only or Private. Private logs are never on public pages, in rankings or in aggregate counts. The uploader can delete any of their logs.
- **Staff access (decided):** staff see a Private log only to handle a report or a removal, and every look is recorded. Raw logs are never downloadable by staff.
- **Removal:** a player can ask mythics.gg to hide or remove their character from pages built from logs (the site's removal request, #131).
- **The site's privacy policy** covers combat logs: what's in them, why, how long they're kept, and how to object.
- **What the app itself sends and keeps** is in the README's Privacy section.

## 5. What the app never does

The app is a file reader and an uploader, and must never look like anything else to the game's anti-cheat.

| The app never | Why |
| --- | --- |
| Opens the game's process, reads its memory, or takes a handle to it | That's what cheats do |
| Injects code, hooks, overlays or DLLs into the game | The same |
| Sends keys, clicks or chat to the game, or types `/combatlog` for the player | Automated control. The player turns logging on, or uses an in-game addon that does |
| Changes game files or settings (`Config.wtf`, CVars, other addons' folders) | It explains settings instead. The one exception is the mythics.gg addon's own folders, and only if the player says yes (1 Oct 2026, above) |
| Reads anything but the `Logs` folder (and where the game is installed, to find it) | Least privilege: not the `WTF` folder, saved variables or other games. With the addon kept up to date, it also reads the addon's own `.toc` files, and the names of running programs to wait until the game is closed |
| Touches the game's network traffic | Interception |

- **It only reads a text file** the game writes for the player, as Warcraft Logs' uploader does.
- **It coexists with other uploaders.** Several programs can read the same file at once, so a guild can run this app and Archon side by side. It never deletes a log, except by archiving it when the player turns that on or selects Archive (1 Oct 2026, above), and then never one another program has open. It copes when another tool moves or deletes a log.

## Sources

Checked 29 Sep 2026.

- Warcraft Wiki, COMBAT_LOG_EVENT. https://warcraft.wiki.gg/wiki/COMBAT_LOG_EVENT
- WowCoach, line format. https://wowcoach.gg/docs/combat-log/line-format
- WowCoach, COMBATANT_INFO. https://wowcoach.gg/docs/combat-log/combatant-info
- WowCoach, enabling combat logging. https://wowcoach.gg/blog/how-to-enable-combat-logging-wow
- Warcraft Logs, getting started. https://www.warcraftlogs.com/help/start
- Blizzard forums, "Bit of help with logs, my logs are being split" (Mar 2021). https://us.forums.blizzard.com/en/wow/t/bit-of-help-with-logs-my-logs-are-being-split/918786
- Blizzard forums, "WoWCombatLog.txt file 42 GB's". https://us.forums.blizzard.com/en/wow/t/wowcombatlogtxt-file-42-gbs/477846
- RFC 8252, OAuth 2.0 for Native Apps. https://datatracker.ietf.org/doc/html/rfc8252
- Tauri. https://v2.tauri.app/
- SignPath Foundation, conditions for open-source projects. https://signpath.org/terms.html
- SignPath, GitHub as a trusted build system. https://docs.signpath.io/trusted-build-systems/github
