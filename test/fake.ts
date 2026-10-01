// A fake app for the views: a realistic snapshot (fake players, real boss
// names) and a bridge that records what the window asked for.

import type { Bridge } from "../src/bridge";
import type { History, Pull, Snapshot, UploadStatus } from "../src/types";

export function pull(over: Partial<Pull>): Pull {
  return {
    sha256: Math.random().toString(16).slice(2),
    kind: "encounter",
    opened_as: "encounter",
    name: "Plexus Sentinel",
    difficulty: 16,
    key_level: null,
    pull_number: 1,
    success: false,
    boss_hp_pct: 23.4,
    duration_ms: 220_000,
    size: 12_400_000,
    start_time: "2026-09-28T20:05:00.000+01:00",
    state: "done",
    progress_pct: null,
    error: null,
    upload_id: "41",
    visibility: "public",
    server_status: null,
    fights: [],
    already: false,
    group: "k:night",
    file_name: "WoWCombatLog-092826_200101.txt",
    encounter_id: 3129,
    log_url: null,
    page_url: null,
    boss_url: null,
    section: null,
    ...over,
  };
}

export function snapshot(over: Partial<Snapshot> = {}): Snapshot {
  return {
    signed_in: true,
    signing_in: false,
    signed_out_notice: false,
    main: { name: "Player1", realm: "Tarren Mill", region: "eu", guild: "Guild One" },
    settings: {
      default_visibility: "guild",
      logs_dir: "C:\\Program Files (x86)\\World of Warcraft\\_retail_\\Logs",
      site_origin: "https://mythics.gg",
      start_with_windows: false,
      only_my_guild: false,
      upload_limit_kbps: 0,
      region: "eu",
      main: null,
      first_run_done: true,
      live_logging: true,
      live_asked: true,
      backlog_pulls: "kills_and_best_wipe",
      archive_uploaded: false,
      archive_delete_after_days: 0,
    },
    dev: false,
    version: "0.1.0",
    live: {
      status: "live",
      logs_dir: "C:\\Program Files (x86)\\World of Warcraft\\_retail_\\Logs",
      file: "WoWCombatLog-092826_200101.txt",
      zone: "Manaforge Omega",
      difficulty: 16,
      advanced: true,
      current: { kind: "encounter", name: "Plexus Sentinel", difficulty: 16, key_level: null, start_time: "9/28/2026 20:30:00.0001" },
      last_activity_ms: Date.now(),
    },
    pulls: [
      pull({ pull_number: 2, state: "uploading", progress_pct: 64, success: true, boss_hp_pct: null, duration_ms: 312_499 }),
      pull({
        pull_number: 1,
        server_status: "parsed",
        log_url: "/logs/12/",
        page_url: "/logs/12/pulls/9/",
        boss_url: "/logs/12/bosses/3129/",
        section: "raid",
        fights: [
          {
            id: "9",
            kind: "encounter",
            encounter_id: 3129,
            name: "Plexus Sentinel",
            difficulty: 16,
            key_level: null,
            kill: false,
            duration_ms: 220_000,
            parent_id: null,
            section: "raid",
            in_key: null,
            url: "/logs/12/pulls/9/",
            boss_url: "/logs/12/bosses/3129/",
          },
        ],
      }),
    ],
    backlog: {
      scanning: false,
      files_done: 3,
      files_total: 3,
      current_name: null,
      current_pct: null,
      files: [
        {
          path: "C:\\Logs\\WoWCombatLog-092826_200101.txt",
          name: "WoWCombatLog-092826_200101.txt",
          folder: null,
          size: 86_000_000,
          modified_ms: Date.UTC(2026, 8, 28, 22),
          first_time: null,
          analysed: false,
          encounters: 0,
          keys: 0,
          segments: 0,
          already: 0,
          queued: 0,
          estimate_best: 0,
          estimate_all: 0,
          summaries: 0,
          advanced: null,
          version: null,
          live: true,
          archive_block: "newest",
        },
        {
          path: "C:\\Logs\\WoWCombatLog-092126_193000.txt",
          name: "WoWCombatLog-092126_193000.txt",
          folder: null,
          size: 9_000_000_000,
          modified_ms: Date.UTC(2026, 8, 21, 23),
          first_time: "2026-09-21T19:30:00.000+01:00",
          analysed: true,
          encounters: 4,
          keys: 1,
          segments: 5,
          already: 2,
          queued: 0,
          estimate_best: 150_000_000,
          estimate_all: 240_000_000,
          summaries: 1,
          advanced: true,
          version: 22,
          live: false,
          archive_block: null,
        },
        {
          path: "C:\\Logs\\RaiderIOLogsArchive\\WoWCombatLog-112120_120130.txt",
          name: "WoWCombatLog-112120_120130.txt",
          folder: "RaiderIOLogsArchive",
          size: 2_200_000_000,
          modified_ms: Date.UTC(2020, 10, 21, 13),
          first_time: "2020-11-21T12:01:34.071",
          analysed: true,
          encounters: 1,
          keys: 0,
          segments: 1,
          already: 0,
          queued: 0,
          estimate_best: 91_000_000,
          estimate_all: 91_000_000,
          summaries: 0,
          advanced: null,
          version: null,
          live: false,
          archive_block: "not_in_logs",
        },
      ],
      total_size: 11_286_000_000,
      paused: false,
      done: 412,
      total: 1036,
      failed: 0,
    },
    archive: {
      folder: "C:\\Program Files (x86)\\World of Warcraft\\_retail_\\Logs\\MythicsLogsArchive",
      exists: true,
      size: 1_240_000_000,
      files: 14,
      busy: null,
    },
    counts: { live_waiting: 1, backlog_waiting: 624, backlog_done: 412, backlog_total: 1036, failed: 0, done: 413 },
    ...over,
  };
}

export type Call = [string, ...unknown[]];

export function fakeBridge(
  snap: Snapshot,
  history?: History,
  recent?: UploadStatus[],
): Bridge & { calls: Call[]; fire: () => void } {
  const calls: Call[] = [];
  let listener: (() => void) | null = null;
  const rec =
    <T,>(name: string, result: T) =>
    (...args: unknown[]) => {
      calls.push([name, ...args]);
      return Promise.resolve(result);
    };
  return {
    calls,
    fire: () => listener?.(),
    getState: () => Promise.resolve(snap),
    onChanged(cb) {
      listener = cb;
      return () => {
        listener = null;
      };
    },
    logIn: rec("logIn", undefined),
    cancelLogIn: rec("cancelLogIn", undefined),
    logOut: rec("logOut", undefined),
    saveSettings: rec("saveSettings", snap),
    chooseLogsFolder: rec("chooseLogsFolder", null),
    findLogsFolder: rec("findLogsFolder", "C:\\Logs"),
    backlogScan: rec("backlogScan", undefined),
    backlogChooseFiles: rec("backlogChooseFiles", 0),
    backlogCancel: rec("backlogCancel", undefined),
    backlogUpload: rec("backlogUpload", 3),
    backlogPause: rec("backlogPause", undefined),
    archiveLog: rec("archiveLog", "WoWCombatLog-092126_193000.zip"),
    openArchiveFolder: rec("openArchiveFolder", undefined),
    history: rec("history", history ?? { rows: [], offline: false }),
    setUploadVisibility: rec("setUploadVisibility", undefined),
    deleteUpload: rec("deleteUpload", undefined),
    openSite: rec("openSite", undefined),
    openLog: rec("openLog", undefined),
    recentUploads: rec("recentUploads", recent ?? []),
  };
}
