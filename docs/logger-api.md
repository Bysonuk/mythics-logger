# The desktop logger's API

> **The app-facing part of the contract.** The full contract, with the server's parser, storage, retention, local testing and the site's own read API (sections 7 to 9 and 11), stays in the mythics.gg repository, which is private, and that copy is the canonical one: where they differ, it wins. Sections keep their numbers, so a reference to section 7, 8 or 9 is to that copy. Paths such as `mt10/…` are in that repository.


The contract between the mythics.gg desktop logger (#322) and the site. The app is built against it in parallel, so a change here is a change in both, in the same pull request or a pair of them. Phase 1: the server stores and parses uploads, and publishes nothing from them yet (#323 is the staff-only preview that comes next).


**Uploads come only from the app.** The owner's decision (29 Sep 2026): combat logs are uploaded through the desktop app, never as a `.txt` through the website, and past logs go through the app's backlog importer. So every `/api/logger/uploads*` route takes the app's token in `Authorization: Bearer` and nothing else: a site session cookie alone gets `401 token_missing`, and the site has no upload form or route for logs.

Conventions:
- JSON bodies and answers are **snake_case**, as below.
- Times are ISO 8601 with a time zone (`2026-09-28T19:58:00+00:00`). A time without one is refused.
- Sizes are bytes. MiB is 1,048,576 bytes.

## 1. Signing in

The site is the app's OAuth server, as RFC 8252 describes for native apps: PKCE, and a redirect to a port on the player's own computer. The app never sees a Battle.net token or the site's cookie.

1. The app makes a PKCE pair (a random `code_verifier` of 43 to 128 characters from `A-Z a-z 0-9 - . _ ~`, and `code_challenge = BASE64URL(SHA256(verifier))`, no padding), a random `state`, and listens on `127.0.0.1:<port>`.
2. It opens the system browser at

   `GET /api/logger/auth/start?port=<1024-65535>&state=<s>&code_challenge=<S256 challenge>`

   `code_challenge_method` may be given; it must be `S256`. `state` is 1 to 256 characters from `A-Z a-z 0-9 - . _ ~`.
   - Not signed in on the site: the browser goes to the site's Battle.net sign-in first (303 to `/api/auth/login?next=…`) and comes back here after it.
   - Signed in: a small page, "Allow the mythics.gg logger on this computer?", naming the account's main character (never the BattleTag), with **Allow** and **Cancel**.
   - A bad request (port out of range, no state, a challenge that isn't 43 base64url characters, another method): a 400 page saying to sign in from the logger again. Nothing goes to the loopback address.
3. **Allow** sends the browser to `http://127.0.0.1:<port>/callback?code=<code>&state=<s>`. **Cancel** sends it to `http://127.0.0.1:<port>/callback?error=access_denied&state=<s>`. The app must check `state` is its own.
   The code is one-time and lives **2 minutes**. The page's form carries the session's CSRF token, and the site checks it and the request's origin.
4. The app swaps the code:

   `POST /api/logger/auth/token` `{"code": "...", "code_verifier": "..."}`

   ```json
   {
     "token": "mtl_…",
     "account": {"battletag": "Name#1234"},
     "main": {"region": "eu", "realmSlug": "tarren-mill", "realm": "Tarren Mill", "name": "…", "id": 1, "cls": "Mage", "level": 80, "faction": "ALLIANCE"}
   }
   ```

   `main` is `null` when the account has no characters. A wrong verifier, a used or expired code: `400 invalid_grant`, and the code is gone either way, so a wrong guess can't be retried. Limited to 30 swaps per 10 minutes per IP.

**The token:**
- Sent as `Authorization: Bearer mtl_…` on every call below. Keep it in the operating system's credential store.
- It can upload, and read and manage the account's own uploads. Nothing else: the dashboard and admin routes take the session cookie and CSRF token, never a bearer token.
- It lasts **90 days from its last use**, renewed as it's used, so a logger that runs weekly never signs in again. Using it counts as a sign-in for the account's retention (accounts go 12 months after the last one).
- The server keeps only its SHA-256.
- An unknown, revoked or expired token: `401 token_invalid`. No token: `401 token_missing`. Both carry `WWW-Authenticate: Bearer`. The app should then sign in again.

**Revoking:**
- From the app (sign out): `POST /api/logger/auth/revoke` with the token. `204`.
- From the account, on the site (API only for now; the page comes later): `GET /api/logger/tokens` lists the account's live tokens (`{"tokens": [{"id", "createdAt", "lastUsedAt", "expiresAt"}]}`, camelCase like the rest of the site's API), and `DELETE /api/logger/tokens/{id}` revokes one (`204`; `404 token_not_found`). These take the session cookie, and the delete the CSRF token, as the dashboard does.

## 2. Segments

A segment is plain combat log text, UTF-8, exactly as the game wrote it:
- the file's header lines (`COMBAT_LOG_VERSION,…`, and the `ZONE_CHANGE` and `MAP_CHANGE` after it, as the app has them);
- then everything from `ENCOUNTER_START` to `ENCOUNTER_END` (`kind: "encounter"`), or from `CHALLENGE_MODE_START` to `CHALLENGE_MODE_END` (`kind: "key"`, holding the key's boss pulls), or any other run of whole lines (`kind: "segment"`: a backlog file, trash, a pull cut short).

The app never edits lines. The server keeps the segment as sent (compressed) and parses it; a segment that turns out to hold no combat log fails with `not_a_combat_log`.

## 3. Uploads

### Ask before uploading

Everyone in a raid logs the same pull, so most of a raid's uploads would be copies. The owner's decision (29 Sep 2026, the "hybrid" approach to upload size): before sending a pull or a key, the app asks whether the site already has it, and skips the ones it has. The parser's matching of copies (section 6) stays as the safety net for whatever is sent.

`POST /api/logger/fingerprints`

```json
{"fingerprints": [
  {"kind": "key", "map_id": 584, "start_time": "2026-09-27T20:39:19.187+00:00", "roster_hash": "<64 hex>"},
  {"kind": "encounter", "encounter_id": 3176, "start_time": "2026-09-28T19:52:00+00:00", "roster_hash": "<64 hex>"}
]}
```

Answer, `200`, one per fingerprint in the same order:

```json
{"fingerprints": [
  {"status": "have", "fight_id": 31, "url": "/logs/3/keys/31/", "log_url": "/logs/3/", "boss_url": null},
  {"status": "need"}
]}
```

| Field | Rule |
|---|---|
| `kind` | `encounter` (a boss pull) or `key`. A segment that never ended (`segment`) isn't asked about: it's sent |
| `encounter_id` | A pull's: `ENCOUNTER_START`'s first field. Required for `encounter` |
| `map_id` | A key's challenge mode id: `CHALLENGE_MODE_START`'s third field. Required for `key` |
| `start_time` | The pull's or key's first line, in UTC, as `start_time` on create |
| `roster_hash` | Below |
| `fingerprints` | 1 to 200 a request |

**The roster hash**, computed the same way by the app and the server (`mt10/logs/fingerprints.py`, `roster_hash`): the SHA-256, lowercase hex, of the pull's player GUIDs, **sorted** (byte order), **de-duplicated**, joined with `\n`, with no final newline. The GUIDs are those of the `COMBATANT_INFO` lines (their first field) after the segment's opening line: for a pull, those before its `ENCOUNTER_END`; for a key, those that aren't inside one of its boss pulls (the ones after `CHALLENGE_MODE_START`). That's what the parser stores as the fight's roster. A pull with no `COMBATANT_INFO` has no fingerprint: the app sends it.

**A match** (`have`) is a parsed copy of the same kind and encounter (or dungeon), started within **5 seconds** of `start_time` (**15** for a key: each uploader's clock is their own computer's), with the same roster hash, in an upload the token's account may see: its own, or a **Public** one. The account's own copy comes first. `url` is that copy's page (section 3, "Page links"), `log_url` its log's, and `boss_url` a raid pull's boss page.

**Never revealed:** anyone else's Private or Guild only copy answers `need`, exactly as if there were none. The player's app then uploads theirs, which is stored and parsed as normal, joining the same fight group (section 6). A character removed on request (section 8) is out of the stored roster, so a pull with them never matches and is sent.

The app keeps a skipped segment in its own list as "Already on mythics.gg (uploaded by a raid member)", with a View link to `url`; nothing of it is uploaded, and it counts towards no quota.

A pull summary (below) is never a match: it has no detail, so a pull with only a summary on the site answers `need`.

### Pull summaries

The owner's decision (29 Sep 2026, "kills and each boss's best wipe by default"): a **past log** sends every kill and each boss's best wipe in full, and for every other wipe only a summary, a few hundred bytes instead of megabytes. The site's pull counts, wipe history and best % stay right; the wipe's own page says "Wipe, 42% (details not uploaded)". The app's Backlog tab has the setting ("Kills and each boss's best wipe", the default, or "All pulls"). **Live logging always sends every pull in full.**

Which wipes the app summarises, per log file and boss (encounter and difficulty): all but the one that got the boss lowest (`boss_hp_pct`, by the parser's rule below: the app has a port of it, `core/src/bosshp.rs`), the longest if several got as low. A wipe without a roster hash or a start goes in full. Keys, and the bosses inside them, always go in full.

`POST /api/logger/pull-summaries`

```json
{
  "summaries": [
    {"encounter_id": 3129, "name": "Plexus Sentinel", "difficulty": 16, "group_size": 20,
     "instance_id": 2810, "start_time": "2026-09-28T19:05:00+00:00",
     "end_time": "2026-09-28T19:08:40+00:00", "duration_ms": 220000, "boss_hp_pct": 42.17,
     "success": false, "roster_hash": "<64 hex>"}
  ],
  "region": "eu", "visibility": "public", "client_version": "0.1.0",
  "session_key": "5f1c…", "file_name": "WoWCombatLog-092826_195000.txt",
  "file_start_time": "2026-09-28T18:50:00+00:00"
}
```

| Field | Rule |
|---|---|
| `summaries` | 1 to 200 a request (the body may be 160 KiB) |
| `encounter_id` | `ENCOUNTER_START`'s first field. Required |
| `name`, `difficulty`, `group_size`, `instance_id` | Optional: `ENCOUNTER_START`'s other fields. `name` is 1 to 200 characters |
| `start_time`, `end_time` | The pull's first and last line, in UTC, as `start_time` and `end_time` on create; at most a day apart. `end_time` at most an hour ahead of the server (`time_in_future`) |
| `duration_ms` | Optional: `ENCOUNTER_END`'s fight time. Without it, `end_time` less `start_time` |
| `boss_hp_pct` | Optional, 0 to 100: the boss's health at the end, by the parser's rule |
| `success` | `false` (a wipe) or `null` (the log never finished the pull). A kill always goes in full: `true` is refused |
| `roster_hash` | As "Ask before uploading". Required |
| `region`, `visibility`, `client_version`, `session_key`, `file_name`, `file_start_time` | As on create: every summary in the request is in that one log |

Answer, `200`, one per summary in the same order:

```json
{"summaries": [
  {"status": "stored", "upload_id": 57, "fight_id": 88, "url": "/logs/12/pulls/88/",
   "log_url": "/logs/12/", "boss_url": "/logs/12/bosses/3129/"},
  {"status": "have", "upload_id": null, "fight_id": 31, "url": "/logs/3/pulls/31/",
   "log_url": "/logs/3/", "boss_url": "/logs/3/bosses/3129/"}
]}
```

- **`stored`:** the summary is an upload of `kind: "summary"`, `status: "parsed"` at once, `size: 0` and no chunks, in the file's log, with its visibility. It's listed, changed and deleted like any upload. Its one fight has `"summary": true`, the fields above and nothing else (no players, gear, totals, deaths or casts), and joins no fight group, so it never counts as a copy of the pull: `logs.public_fights` never shows it.
- **`have`:** the account may already see the pull in full (its own copy, or a raid member's Public one, by the fingerprint rule): nothing is stored, and the pages are that copy's.
- **Idempotent by fingerprint:** the same summary again (the same boss and roster, started within 5 seconds) answers with the same `upload_id`, and stores nothing new.
- **A full copy replaces a summary:** when the account's own full copy of the pull is parsed (the same boss, started within 5 seconds), its summary is deleted. So a player who switches to "All pulls" and sends the log again ends up with one copy of each pull.
- **Limits:** 60 requests a minute per token, on top of the 600. No quota: a summary is a few hundred bytes.

### Create, or resume

`POST /api/logger/uploads`

```json
{
  "sha256": "<64 lowercase hex: SHA-256 of the whole segment, uncompressed>",
  "size": 5242880,
  "chunk_size": 4194304,
  "kind": "encounter",
  "encounter_id": 3202,
  "difficulty": 16,
  "key_level": null,
  "map_id": 2810,
  "start_time": "2026-09-28T19:52:00+00:00",
  "end_time": "2026-09-28T19:58:00+00:00",
  "region": "eu",
  "visibility": "public",
  "client_version": "0.1.0",
  "session_key": "5f1c…",
  "file_name": "WoWCombatLog-092826_195000.txt",
  "file_start_time": "2026-09-28T18:50:00+00:00"
}
```

| Field | Rule |
|---|---|
| `sha256` | Of the decompressed segment |
| `size` | 1 byte to 2 GiB, decompressed |
| `chunk_size` | 64 KiB to 64 MiB, **decompressed** bytes per chunk. Chunk `n` is bytes `n × chunk_size` to `(n + 1) × chunk_size` of the segment; the last is the remainder. At most 32,768 chunks |
| `kind` | `encounter`, `key` or `segment` |
| `encounter_id`, `difficulty`, `key_level`, `map_id` | Optional: what the app saw, for its own list. The parse reads the real values from the log. `map_id` is a key's challenge mode id (`CHALLENGE_MODE_START`'s third field, such as 584) or a pull's instance id (`ENCOUNTER_START`'s last field, such as 2810) |
| `start_time`, `end_time` | The segment's first and last line, in UTC. `end_time` may be at most an hour ahead of the server's clock (`time_in_future`). An old log without a year or offset in its lines gets both from `start_time` |
| `region` | `eu` or `us` |
| `visibility` | `public`, `guild` or `private` (section 5) |
| `client_version` | 1 to 32 characters of `A-Z a-z 0-9 . + _ -` |
| `session_key` | Optional: 8 to 128 characters of `A-Z a-z 0-9 _ . : -`, the same for every segment of one log file ("Sessions" below). Without it the upload is a log of its own |
| `file_name` | Optional, only with `session_key`: the log file's **name**, never its path (a path can hold the player's Windows user name). 1 to 255 characters with no `/`, `\` or `:` |
| `file_start_time` | Optional, only with `session_key`: the file's first timestamp, in UTC. Without it the log starts at its earliest segment's `start_time` |

Answer, `200`:

```json
{"id": 41, "status": "receiving", "received": [], "chunk_size": 4194304, "chunk_count": 2, "historical": false, "…": "…"}
```

It's **idempotent by (account, sha256)**: sending the same segment again answers with the same `id`, its `status` and the chunks already `received`, so the app resumes (sends the missing chunks) or skips (it's `queued` or `parsed`). A repeat keeps the first request's `chunk_size`: the app must use the one in the answer. No quota is charged for a repeat.

### Send a chunk

`PUT /api/logger/uploads/{id}/chunks/{n}`, body: the chunk compressed with **zstd** (one or more frames), at most **4 MiB** compressed. Any `Content-Type`. `204`.

The app compresses **live pulls at zstd level 10**: on the 86 MB real log (section 7) that's 4.7 MB against 7.0 MB at zstd's default level 3, at about 0.2 s per 8 MB chunk (25 ms at level 3) on the developer's computer. **Past logs** go at **level 19 with long-distance matching** and chunks of up to 32 MiB (the owner's decision, 29 Sep 2026: "max compression for backlog"), on a background-priority thread. The server takes any level; it keeps the chunks as sent (joined, still compressed) for a year.

**The window:** a chunk's frames may ask for a zstd window of up to **2^27 bytes (128 MiB)**; a bigger one is refused (`422 chunk_rejected`), so an upload can't make the server hold more than that to decompress it (`mt10/logs/store.py`, `MAX_WINDOW`). The app sets window log 27 for past logs, and zstd shrinks the window to the chunk's size when it knows it, as the app's frames do. A frame that doesn't say its size can still make the server hold the whole 128 MiB while it's read.

- The chunk must decompress to exactly its share of the segment (`chunk_size`, or the remainder for the last): `422 chunk_rejected` otherwise, or when it isn't zstd. Decompression stops one byte past the expected size, so a zip bomb is refused cheaply.
- `n` from 0 to `chunk_count − 1`: `422 chunk_out_of_range` otherwise.
- Over 4 MiB: `413 chunk_too_large`, or `413 body_too_large` when the declared `Content-Length` is too big (refused before the body is read; the proxy in front stops anything over 5 MB).
- Sending a chunk again replaces it. Chunks may arrive in any order, and in parallel.
- After `complete`: `409 upload_not_receiving`.

### Complete

`POST /api/logger/uploads/{id}/complete`. `200` with the upload, `status: "queued"`.

- Every chunk must have arrived: `409 chunks_missing` with `"missing": [n, …]` (the first 1,000) otherwise.
- The server joins the chunks into one stored segment and checks the SHA-256 of the whole, decompressed. A mismatch: `422 hash_mismatch`, and every chunk is dropped: send them all again (the app can't know which was wrong).
- Then it queues the parse: at once for a live upload, at a lower priority for a backlog one.
- Completing again answers the same, with nothing redone.

### List

`GET /api/logger/uploads?cursor=<c>&limit=<1-100, default 50>`, newest first:

```json
{"uploads": [{"id": 41, "status": "parsed", "session_id": 12, "log_url": "/logs/12/", "…": "…",
              "fights": [{"id": 7, "url": "/logs/12/keys/7/", "…": "…"}]}],
 "next_cursor": "40"}
```

Each upload carries its `fights` as in "One upload" below: an empty list until it's `parsed`.

`next_cursor` is `null` on the last page; pass it back as `cursor` for the next. Only the token's own account's uploads.

`status` is `receiving` (chunks still to come), `queued` (complete, waiting for the parser), `parsed` or `failed` (with `error`: `not_a_combat_log`, `segment_missing`).

### One upload

`GET /api/logger/uploads/{id}`: the upload, its `received` chunks, and what the parse found, for the app to show ("Wipe, 4:12, boss at 23%"):

```json
{
  "id": 41, "status": "parsed", "log_version": 22, "historical": false, "raw_deleted": false, "…": "…",
  "fights": [
    {"id": 7, "kind": "key", "encounter_id": null, "name": "The Blinding Vale", "difficulty": null,
     "challenge_mode_id": 584, "key_level": 14, "affixes": [9, 10, 147],
     "started_at": "2026-09-27T20:39:19.187000+00:00", "duration_ms": 1681247, "success": true,
     "kill": true, "boss_hp_pct": null, "canonical": true, "parent_id": null,
     "upload_id": 41, "section": "mplus", "in_key": null,
     "url": "/logs/12/keys/7/", "boss_url": null},
    {"id": 8, "kind": "encounter", "encounter_id": 3199, "name": "Lightblossom Trinity", "difficulty": 8,
     "…": "…", "kill": true, "boss_hp_pct": 0.0, "canonical": true, "parent_id": 7,
     "upload_id": 41, "section": "mplus", "in_key": 7,
     "url": "/logs/12/pulls/8/", "boss_url": null},
    {"id": 9, "kind": "encounter", "encounter_id": 3176, "name": "…", "difficulty": 16,
     "…": "…", "section": "raid", "in_key": null,
     "url": "/logs/12/pulls/9/", "boss_url": "/logs/12/bosses/3176/"}
  ]
}
```

`canonical` says whether this copy is the pull's canonical one (section 6). `kill` is `success` by its player-facing name: the boss died, or the key was completed; `null` if the log never said. `summary` is `true` for a pull summary's fight (only its result, "details not uploaded").

`section` is where the fight goes in its log ("Sessions" below): `raid` (a raid boss pull), `mplus` (a key, or a boss inside one: `in_key` is that key's fight id) or `dungeon` (a dungeon boss outside any key). `boss_url` is only a raid pull's.

The upload fields in every answer: `id, status, kind, visibility, historical, sha256, size, chunk_size, chunk_count, encounter_id, difficulty, key_level, map_id, start_time, end_time, region, client_version, log_version, error, created_at, completed_at, parsed_at, raw_deleted, session_id, log_url, url`, plus `received` where it says so. `url` is the same as `log_url`, kept for older apps.

### Sessions

A **log session** is one combat log file's uploads, together again: the app cuts a `WoWCombatLog-*.txt` into segments, and the site shows the file as one log, split into its **Raid** bosses and its **Mythic+** keys (the owner's request, 29 Sep 2026). Table `logs.sessions`, one per (account, `session_key`).

- **The key:** the app sends the same `session_key` with every segment of one file, and a different one for every file. The app's is the SHA-256 (hex) of the file's name, a newline, and the file's first line's timestamp as the file has it: stable across restarts and re-imports, and different when WoW starts a new file. The server doesn't check how it's made: it finds the account's session with that key, or makes one. Another account's same key is another session.
- **No key** (an older app): the upload is a session of its own.
- **The file's name** is kept (only the name, never a path) and shown only to the uploader.
- **Uploads made before sessions** (migration 0066) each got a session of their own.
- A session holds raid and Mythic+ alike. **A boss inside a key is never a raid boss:** it's the key's when the parser saw it inside the key (`parent_id`), or when it started within the key's time (from its start to its end) in the same dungeon; otherwise a raid difficulty (3 to 7, 9, 14 to 17, 33, 151, 220; with none, a group over 5) makes it a raid boss, and anything else a dungeon boss outside a key (`section: "dungeon"`).
- **Visibility** stays per upload: a session shows each viewer only the uploads they may see (section 5), and one with nothing they may see is a `404`, as if it didn't exist.
- A session left with no uploads is deleted by the daily retention job, a day later.

`GET /api/logger/sessions/{id}`, for the app's History tab:

```json
{
  "id": 12, "log_url": "/logs/12/", "file_name": "WoWCombatLog-092826_195000.txt",
  "first_time": "2026-09-28T18:50:00+00:00", "region": "eu", "created_at": "…",
  "uploads": [{"id": 41, "status": "parsed", "kind": "key", "visibility": "public",
               "start_time": "…", "end_time": "…"}],
  "raid": [
    {"encounter_id": 3176, "name": "…", "boss_url": "/logs/12/bosses/3176/", "killed": true,
     "pulls": [{"id": 9, "kill": false, "url": "/logs/12/pulls/9/", "…": "…"},
               {"id": 10, "kill": true, "url": "/logs/12/pulls/10/", "…": "…"}]}
  ],
  "mplus": [
    {"id": 7, "kind": "key", "key_level": 14, "url": "/logs/12/keys/7/", "…": "…",
     "bosses": [{"id": 8, "in_key": 7, "url": "/logs/12/pulls/8/", "…": "…"}]}
  ],
  "dungeon": []
}
```

Raid bosses in the order first pulled, each with its pulls in order, pull summaries among them (`"summary": true`); `killed` if any pull was a kill; `best_hp_pct` the lowest boss health of any wipe, summaries too (`null` if none says). Keys in order, each with the bosses inside it. Every fight is as in "One upload". `file_name` is `null` to anyone but the uploader. Uploads still `receiving` or `queued` are listed, with nothing parsed from them yet. `404 session_not_found` for no such session, or one with nothing the token's account may see.

### Page links

The app links each log, raid boss, pull and key to its page on the site, opening the system browser at the site's origin plus these paths (the log pages, #327). `{s}` is the session's id:

| Page | Path | In the answer |
|---|---|---|
| The log, with Raid and Mythic+ sections | `/logs/{s}/` | the upload's `log_url` (`null` until it's `parsed`, as there's nothing of it to show before), the session's `log_url` |
| A raid boss in the log, all its pulls | `/logs/{s}/bosses/{encounter_id}/` | a raid pull's `boss_url`, a session raid boss's `boss_url` |
| One pull: a raid pull, or a dungeon boss (inside a key or not) | `/logs/{s}/pulls/{fight_id}/` | the fight's `url` |
| A key | `/logs/{s}/keys/{fight_id}/` | the key's `url` |
| My logs | `/account/logs/` | not in any answer: the app knows it |

Paths only, never an origin: the app keeps the site's origin (`https://mythics.gg`, or a local one for testing) and opens nothing outside it. While an upload is `receiving` or `queued` the app shows it as processing and polls the list gently until it's `parsed` or `failed`.

### Change visibility

`PATCH /api/logger/uploads/{id}` `{"visibility": "private"}`. `200` with the upload. Takes effect at once, for every reader.

### Delete

`DELETE /api/logger/uploads/{id}`. `204`. The uploader can delete any of their logs at any time (the owner's decision): the raw segment and chunks, and everything parsed from it, go at once. If the bucket can't be reached, the row still goes and the daily retention job deletes the objects.

## 4. Errors, limits and the quota

Every refusal on these routes has the site's shape, with a code for the app to act on; `detail` is plain English for logs and people, not for showing:

```json
{"detail": "Some chunks haven't arrived.", "code": "chunks_missing", "missing": [3, 4]}
```

| Code | Status | When |
|---|---|---|
| `invalid_request` | 422 | A body or query that doesn't meet the rules above; `errors` lists `{loc, msg}` |
| `token_missing` | 401 | No bearer token (a session cookie doesn't count) |
| `token_invalid` | 401 | Unknown, revoked or expired token |
| `invalid_grant` | 400 | The code swap failed (section 1) |
| `upload_not_found` | 404 | No such upload, or it's someone else's: the same answer either way |
| `session_not_found` | 404 | No such log session, or nothing in it the token's account may see |
| `upload_not_receiving` | 409 | A chunk for a completed upload |
| `upload_busy` | 409 | The same upload is being created by a request racing this one: try again |
| `chunks_missing` | 409 | `complete` before every chunk arrived; `missing` lists them |
| `chunk_rejected` | 422 | Not zstd, the wrong size decompressed, or a window over 128 MiB |
| `chunk_out_of_range` | 422 | No such chunk number |
| `chunk_too_large`, `body_too_large` | 413 | Over 4 MiB compressed |
| `hash_mismatch` | 422 | The joined chunks don't match `sha256`: send them all again |
| `time_in_future` | 422 | `end_time` over an hour ahead of the server: check the clock |
| `quota_reached` | 429 | Section below; `retry_after` and `Retry-After` say when |
| `rate_limited` | 429 | Too many requests; `Retry-After` says when |
| `uploads_unavailable` | 503 | Uploads are switched off on this server |
| `token_not_found` | 404 | Revoking a token the account doesn't have (the site's route) |

**Rate limits:** 600 requests a minute per token (every route above), 60 of them to `pull-summaries`, 30 code swaps per 10 minutes per IP, 20 allow-page posts per 10 minutes per session. On a `429`, wait `Retry-After` seconds.

### Quota

Per account, over a rolling 24 hours, counted in **decompressed bytes** when an upload is created (a repeat of the same upload isn't charged again, and deleting an upload doesn't give its bytes back):
- **20 GiB a day** (`MT10_LOGGER_DAILY_BYTES`);
- **200 GiB a day in the account's first 7 days** as an uploader (`MT10_LOGGER_FIRST_WEEK_DAILY_BYTES`), counted from its first upload.

**Big backlogs:** the first-week allowance lets a new player send their old logs at once: a raid night is about 500 MB, so 200 GiB is some 400 nights a day. After the first week, a backlog bigger than a day's allowance keeps going over several days: the refusal is `429 quota_reached` with `Retry-After` (and `retry_after` in the body), the seconds until enough of the last 24 hours' uploads age out for this one to fit. The app keeps the rest queued, waits, and carries on; nothing is lost. The app should send live pulls before backlog ones, so a raid night is never stuck behind an import.

## 5. Visibility

Each upload is **Public**, **Guild only** or **Private** (the owner's decision, 29 Sep 2026), set at upload (the app's default comes from its settings) and changed at any time:

- **Public:** may appear on the site and in public rankings.
- **Guild only:** the uploader's guild staff and members. Nothing reads guild logs yet; until the first page that does (#323) checks guild membership, only the uploader sees them.
- **Private:** only the uploader. Never on a public page or in a public ranking; staff see it only to handle a report or a removal.

Enforced now:
- Through the API, only the uploader reads, changes or deletes an upload (`mt10/logs/ingest.py`, `visible_to`), and another account's upload is a plain `404`.
- The publisher's database login can't read the `logs` schema at all, so nothing is published from it until a later change grants that on purpose.
- `logs.public_fights` (section 6) is the only view a public page may read.
- **Character profiles** (`mt10/logs/characters.py`, #323's staff preview): every player in every parsed upload is registered by GUID, with each key and raid pull they were in. A profile counts only uploads its viewer may see: a Private upload never counts, nor gives a name, for anyone but its uploader; Guild only counts for its uploader and staff; staff see Public and Guild only. Deleting an upload takes its rows, and a removed character is never registered.

## 6. Past logs, and copies of the same pull

**Backlogs.** The app's backlog importer sends old files through this same API. An upload whose segment ended more than **30 minutes** before it was sent is **historical** (`"historical": true`): set at create from `end_time`, and again at parse from the log's own last line, so it can only become historical, never live. Historical uploads parse at a lower priority, and never trigger live features (the Race to World First's live updates, later).

**Older log formats.** The parser reads `COMBAT_LOG_VERSION` and records it on the upload (`log_version`, `build_version`, and whether advanced logging was on). Version 22 (The War Within and Midnight) is checked against a real log. Older versions are parsed as the community's notes describe them (section 7), and the parts it can't read are skipped, not fatal: a line it can't read is counted and passed over, and an event it doesn't know is counted. A newer version than 22 is parsed as far as it matches version 22. Older expansions' encounters need nothing special: their ids and difficulties are stored as the log gives them. Every upload keeps its raw segment for a year, so a parser fix can re-parse it (`parser_version` says which parse it had).

**Copies of one pull.** Everyone in a raid logs the same pull, so several uploads can hold it with different bytes, and `sha256` can't tell. The parse groups them: a fight joins an existing group (`logs.fight_groups`) when it's
- the same encounter id and difficulty (for a key: the same dungeon and key level),
- started within **5 seconds** of it (15 for a key: each uploader's clock is their own computer's),
- with at least **half** of the smaller roster (player GUIDs) in both.

Every copy is kept. The earliest is canonical (`logs.canonical_fights`, for filling gaps from the others later). A group is public only if at least one copy is Public, and a public page reads `logs.public_fights`: the earliest **Public** copy of each group. A Private copy, and so its uploader, never reaches it, even when it's the canonical one.

## 10. Page links

The site's pages for our own logs (#323, the staff-only preview). The app links to them, straight to a log, a key, a raid boss or a pull, so they are a contract: keep them stable.

A **log** is one combat log session: one WoW log file as the app read it, which may hold several uploads (segments), raid pulls and Mythic+ keys alike (the owner's decision, 29 Sep 2026). Until the server groups uploads into sessions (`logs.sessions`, being added on `bysonuk/322-upload-fight-ids`), each upload is a log of its own and `log_id` is its upload `id`; the addresses stay the same when the grouping lands.

| Page | Address |
|---|---|
| A log: its **Raid** section (grouped by raid, then boss: each boss's pulls, wipes with boss % and stage, the kill marked, and the clear time) and its **Mythic+** section (each key: dungeon, level, timed or not, time, deaths). A dungeon boss inside a key is only ever in that key. A log of one kind shows only that section | `/logs/{log_id}/` |
| One raid boss in the log: all its pulls, with a pull picker | `/logs/{log_id}/bosses/{encounter_id}/` |
| One fight: a raid pull, or a dungeon boss inside a key, with a breadcrumb to its raid and boss, or its key | `/logs/{log_id}/pulls/{fight_id}/` |
| One key: its overview, the bosses inside it (linking to their pull pages), deaths, the timer, and the whole key's players and breakdowns | `/logs/{log_id}/keys/{fight_id}/` |
| My logs: the account's uploads, who may see each, delete | `/account/logs/` |

`fight_id` is a fight's `id` (in `GET /api/logger/uploads/{id}`'s `fights`); `encounter_id` is the game's (`ENCOUNTER_START`'s). A pull or key asked for under another log's address, or a boss the log doesn't hold, is not found.

- **Signed out:** the site's "Log in with Battle.net", which comes back to the same address. Every one of these addresses works when opened directly.
- **Signed in, no access:** the site's ordinary not-found page, the same as for an id that doesn't exist.
- **Who has access** (the owner's rule for the preview): the uploader, to every upload of theirs; site staff, to any upload that isn't Private. Nobody else, Public uploads included, until the public pages read `logs.public_fights`.
- Never indexed (`noindex`, and Caddy's `X-Robots-Tag`), no link preview, no sitemap entry. The account menu shows "Our logs (preview)" to staff and to anyone who has uploaded a log.
