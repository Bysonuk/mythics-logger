// The app's state as the Rust side sends it (src-tauri/src/state.rs).

export type Visibility = "public" | "guild" | "private";
export type Kind = "encounter" | "key" | "segment";

export interface Main {
  name: string | null;
  realm: string | null;
  region: string | null;
  guild: string | null;
}

export interface Settings {
  default_visibility: Visibility;
  logs_dir: string | null;
  site_origin: string;
  start_with_windows: boolean;
  only_my_guild: boolean;
  upload_limit_kbps: number;
  region: string;
  main: Main | null;
  first_run_done: boolean;
  /** Upload pulls as they end. Off until the player chooses. */
  live_logging: boolean;
  /** The first-run question about live logging has been answered. */
  live_asked: boolean;
  /** Which of a past log's pulls go in full: every kill and each boss's
   *  best wipe (the other wipes as summaries), or all of them. */
  backlog_pulls: BacklogPulls;
  /** Archive a finished log once all its pulls are uploaded. Off by default. */
  archive_uploaded: boolean;
  /** Delete the app's archives older than this many days: 0 (never), 30, 60 or 90. */
  archive_delete_after_days: number;
  /** "Keep the addon up to date". Off unless the player says yes. */
  addon_auto_update: boolean;
  /** "Install the mythics.gg addon?" has been answered. */
  addon_asked: boolean;
}

export type BacklogPulls = "kills_and_best_wipe" | "all";

export interface Current {
  kind: Kind;
  name: string | null;
  difficulty: number | null;
  key_level: number | null;
  start_time: string;
}

export interface LiveStatus {
  status: "off" | "searching" | "waiting" | "live" | "";
  logs_dir: string | null;
  file: string | null;
  zone: string | null;
  difficulty: number | null;
  advanced: boolean | null;
  current: Current | null;
  last_activity_ms: number;
}

/** Where an upload sits in its log (one combat log file), and its pages on
 *  the site: paths, opened only on the site in Settings. */
export interface LogPlace {
  /** The same for every upload of one log file. */
  group: string;
  /** The log file's name, when this app sent it. */
  file_name: string | null;
  encounter_id: number | null;
  /** The log's page, once the server has parsed this upload. */
  log_url: string | null;
  /** The pull's or key's own page. */
  page_url: string | null;
  /** A raid pull's boss page: every pull of that boss in the log. */
  boss_url: string | null;
  /** The server's word on where it goes. */
  section: "raid" | "mplus" | "dungeon" | null;
}

export interface Pull extends LogPlace {
  sha256: string;
  kind: Kind;
  opened_as: Kind;
  name: string | null;
  difficulty: number | null;
  key_level: number | null;
  pull_number: number;
  success: boolean | null;
  boss_hp_pct: number | null;
  duration_ms: number | null;
  size: number;
  start_time: string;
  state: "waiting" | "uploading" | "done" | "failed";
  progress_pct: number | null;
  error: string | null;
  upload_id: string | null;
  visibility: Visibility;
  /** The server's status once asked; null until then. */
  server_status: ServerStatus | null;
  fights: Fight[];
  /** Not sent: a raid member had uploaded it already; the links are to
   *  their copy. */
  already: boolean;
}

export type ServerStatus = "receiving" | "queued" | "parsed" | "failed";

/** A boss pull or key the server found in an upload, with its page. */
export interface Fight {
  id: string;
  kind: "encounter" | "key" | null;
  encounter_id?: number | null;
  name: string | null;
  difficulty: number | null;
  key_level: number | null;
  kill: boolean | null;
  duration_ms: number | null;
  parent_id: string | null;
  /** "raid", "mplus" (a key, or a boss inside one) or "dungeon". */
  section?: "raid" | "mplus" | "dungeon" | null;
  /** The key a boss was fought in. */
  in_key?: string | null;
  /** Its page: a pull's or a key's. */
  url: string | null;
  /** A raid pull's boss page. */
  boss_url?: string | null;
  /** The boss's health at the end, in percent. */
  boss_hp_pct?: number | null;
  /** A pull summary: the wipe's result only, its detail never uploaded. */
  summary?: boolean;
}

/** The server's word on one upload (`recent_uploads`). */
export interface UploadStatus {
  id: string;
  status: ServerStatus | null;
  session_id?: string | null;
  log_url?: string | null;
  /** The same as log_url, from servers before log sessions. */
  url: string | null;
  fights: Fight[];
}

export interface BacklogFile {
  path: string;
  name: string;
  folder: string | null;
  size: number;
  modified_ms: number;
  first_time: string | null;
  analysed: boolean;
  encounters: number;
  keys: number;
  segments: number;
  already: number;
  queued: number;
  /** About what the rest would upload, compressed, with each setting. */
  estimate_best: number;
  estimate_all: number;
  /** Of the rest, the wipes "Kills and each boss's best wipe" summarises. */
  summaries: number;
  advanced: boolean | null;
  version: number | null;
  live: boolean;
  /** Why it can't be archived now; null if it can. */
  archive_block: ArchiveBlock | null;
  /** The player chose to skip the rest of this log: what isn't uploaded
   *  won't be, and "Archive logs once uploaded" may archive it. */
  skipped: boolean;
}

/** Why a log can't be archived (src-tauri's `archive_block`, and the codes
 *  an attempt can fail with). */
export type ArchiveBlock = "newest" | "recent" | "queued" | "in_use" | "not_in_logs" | "gone";

/** The archive folder, Logs\MythicsLogsArchive, and the log being archived. */
export interface ArchiveView {
  folder: string | null;
  exists: boolean;
  size: number;
  files: number;
  busy: { path: string; name: string; pct: number } | null;
}

/** What Install or Update would do (core's `addon::Action`). */
export type AddonAction = "nothing" | "install" | "update" | "repair" | "newer" | "unbuilt" | "linked";

/** The mythics.gg in-game addon (src-tauri's `addon.rs`). */
export interface AddonView {
  /** The game's _retail_ folder is known, so the addon can go in. */
  game_found: boolean;
  /** Any of the addon's folders is in Interface\AddOns. */
  present: boolean;
  installed: string | null;
  latest: string | null;
  /** "unavailable": mythics.gg doesn't publish the addon yet. */
  availability: "unknown" | "available" | "unavailable";
  action: AddonAction | null;
  status: "idle" | "checking" | "installing" | "waiting_for_game";
  /** The last attempt's error code (`errorText`). */
  error: string | null;
  checked_ms: number | null;
}

/** A newer version of this app (src-tauri's `app_update.rs`). */
export interface AppUpdateView {
  available: { version: string; notes: string | null; date: string | null } | null;
  /** Show "Version X is ready": there is one, and the player hasn't said Later. */
  offer: boolean;
  status: "idle" | "checking" | "downloading" | "installing";
  progress_pct: number | null;
  /** "update_check" or "update_install" (`errorText`). */
  error: string | null;
  checked_ms: number | null;
}

/** The live report's link (src-tauri's `share.rs`): a page of the log being
 *  written that anyone with the link can open. */
export interface LiveShareView {
  /** "off": not live logging. "waiting": no pull uploaded yet.
   *  "not_public": the log's pulls go Guild only or Private. "unavailable":
   *  mythics.gg doesn't have share links yet. */
  status: "off" | "waiting" | "not_public" | "creating" | "ready" | "revoked" | "unavailable" | "error";
  /** The link, when ready. */
  url: string | null;
  /** Why there's no link (with "not_public"). */
  visibility: Visibility | null;
  /** The last attempt's error code (`errorText`). */
  error: string | null;
}

export interface BacklogView {
  scanning: boolean;
  files_done: number;
  files_total: number;
  current_name: string | null;
  current_pct: number | null;
  files: BacklogFile[];
  total_size: number;
  paused: boolean;
  done: number;
  total: number;
  failed: number;
}

export interface Counts {
  live_waiting: number;
  backlog_waiting: number;
  backlog_done: number;
  backlog_total: number;
  failed: number;
  done: number;
}

export interface Snapshot {
  signed_in: boolean;
  signing_in: boolean;
  signed_out_notice: boolean;
  main: Main | null;
  settings: Settings;
  dev: boolean;
  version: string;
  live: LiveStatus;
  pulls: Pull[];
  backlog: BacklogView;
  counts: Counts;
  archive: ArchiveView;
  addon: AddonView;
  app_update: AppUpdateView;
  live_share: LiveShareView;
}

export interface HistoryRow extends LogPlace {
  id: string | null;
  sha256: string | null;
  kind: string | null;
  name: string | null;
  difficulty: number | null;
  key_level: number | null;
  success: boolean | null;
  start_time: string | null;
  size: number | null;
  visibility: Visibility | null;
  past: boolean;
  status: ServerStatus | null;
  fights: Fight[];
  on_server: boolean;
  /** Not sent: a raid member had uploaded it already. */
  already: boolean;
  /** A past log's wipe sent as a summary: its result only. */
  summary: boolean;
  /** The boss's health at the end, in percent. */
  boss_hp_pct: number | null;
}

export interface History {
  rows: HistoryRow[];
  offline: boolean;
}

export type SettingsPatch = Partial<
  Pick<
    Settings,
    | "default_visibility"
    | "site_origin"
    | "start_with_windows"
    | "only_my_guild"
    | "upload_limit_kbps"
    | "region"
    | "first_run_done"
    | "live_logging"
    | "backlog_pulls"
    | "archive_uploaded"
    | "archive_delete_after_days"
    | "addon_auto_update"
  >
>;
