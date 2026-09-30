// Numbers, sizes, times and the app's words, in en-GB, matching the site
// (web/src/app/format.ts): "26 Sept 2026, 11:16 BST", "4 hours ago".

import type { BacklogFile, Fight, Kind, Pull, Visibility } from "./types";

const DAY = new Intl.DateTimeFormat("en-GB", { day: "numeric", month: "short", year: "numeric" });
const DAY_TIME = new Intl.DateTimeFormat("en-GB", {
  day: "numeric",
  month: "short",
  year: "numeric",
  hour: "2-digit",
  minute: "2-digit",
  timeZoneName: "short",
});
const TIME = new Intl.DateTimeFormat("en-GB", { hour: "2-digit", minute: "2-digit" });

function valid(iso: string | null | undefined): Date | null {
  if (!iso) return null;
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? null : d;
}

export function formatDate(iso: string | null | undefined): string {
  const d = valid(iso);
  return d ? DAY.format(d) : "Unknown date";
}

export function formatDateTime(iso: string | null | undefined): string {
  const d = valid(iso);
  return d ? DAY_TIME.format(d) : "Unknown date";
}

export function formatTime(iso: string | null | undefined): string {
  const d = valid(iso);
  return d ? TIME.format(d) : "";
}

export function formatAgo(ms: number, now: number = Date.now()): string {
  const minutes = Math.floor((now - ms) / 60_000);
  if (minutes < 60) return minutes < 1 ? "just now" : minutes === 1 ? "1 minute ago" : `${minutes} minutes ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return hours === 1 ? "1 hour ago" : `${hours} hours ago`;
  const days = Math.floor(hours / 24);
  return days === 1 ? "yesterday" : `${days} days ago`;
}

/** "4:12", or "1:04:12" for a long key. */
export function formatDuration(ms: number): string {
  const total = Math.round(ms / 1000);
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = String(total % 60).padStart(2, "0");
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${s}` : `${m}:${s}`;
}

/** "11.2 GB", "412 MB", "86 KB": decimal units, as Windows Explorer's
 *  neighbours in the download world do; one decimal under 100. */
export function formatBytes(n: number): string {
  const units = ["bytes", "KB", "MB", "GB", "TB"];
  let v = n;
  let i = 0;
  while (v >= 1000 && i < units.length - 1) {
    v /= 1000;
    i++;
  }
  if (i === 0) return `${n.toLocaleString("en-GB")} ${n === 1 ? "byte" : "bytes"}`;
  return `${v < 100 ? v.toFixed(1) : Math.round(v)} ${units[i]}`;
}

export function formatCount(n: number, noun: string, plural = `${noun}s`): string {
  return `${n.toLocaleString("en-GB")} ${n === 1 ? noun : plural}`;
}

/** Raid and dungeon difficulty ids, as the game names them. */
export function difficultyName(id: number | null | undefined): string | null {
  switch (id) {
    case 14:
      return "Normal";
    case 15:
      return "Heroic";
    case 16:
      return "Mythic";
    case 17:
      return "Raid Finder";
    case 1:
      return "Normal";
    case 2:
      return "Heroic";
    case 23:
      return "Mythic";
    case 8:
      return "Mythic+";
    default:
      return null;
  }
}

export const VISIBILITY_LABEL: Record<Visibility, string> = {
  public: "Public",
  guild: "Guild only",
  private: "Private",
};

export const VISIBILITY_HINT: Record<Visibility, string> = {
  public: "On mythics.gg for everyone, and in rankings.",
  guild: "Only your guild's members and staff can see it.",
  private: "Only you can see it. Never on public pages or in rankings.",
};

/** What a pull came to: "Wipe · 23.4%", "Kill · 5:12", "+14 · timed in 28:01". */
export function pullResult(p: Pick<Pull, "kind" | "opened_as" | "success" | "boss_hp_pct" | "duration_ms" | "key_level">): string {
  if (p.kind === "segment") return p.opened_as === "key" ? "Key not finished" : "Pull not finished";
  if (p.kind === "key") {
    const time = p.duration_ms != null ? ` in ${formatDuration(p.duration_ms)}` : "";
    return p.success ? `Completed${time}` : `Not completed${time}`;
  }
  if (p.success) return `Kill · ${p.duration_ms != null ? formatDuration(p.duration_ms) : ""}`.replace(/ · $/, "");
  const hp = p.boss_hp_pct != null ? ` · ${p.boss_hp_pct.toFixed(1)}%` : "";
  const time = p.duration_ms != null ? ` · ${formatDuration(p.duration_ms)}` : "";
  return `Wipe${hp}${time}`;
}

/** A past log's wipe sent as a summary: "Wipe, 42.2% (details not uploaded)". */
export function summaryResult(bossHpPct: number | null | undefined): string {
  return `Wipe${bossHpPct != null ? `, ${bossHpPct.toFixed(1)}%` : ""} (details not uploaded)`;
}

export function pullTitle(p: Pick<Pull, "kind" | "opened_as" | "name" | "key_level" | "difficulty">): string {
  const name = p.name ?? (p.opened_as === "key" ? "Mythic+ key" : "Boss");
  if (p.opened_as === "key") return p.key_level != null ? `${name} +${p.key_level}` : name;
  return name;
}

/** A pull or key the server found: "Plexus Sentinel · Mythic · Kill · 5:12",
 *  "The Blinding Vale +14 · Completed in 28:01". */
export function fightText(f: Fight): string {
  const time = f.duration_ms != null ? formatDuration(f.duration_ms) : null;
  if (f.kind === "key") {
    const name = `${f.name ?? "Mythic+ key"}${f.key_level != null ? ` +${f.key_level}` : ""}`;
    const done = f.kill === true ? "Completed" : f.kill === false ? "Not completed" : null;
    return [name, done && time ? `${done} in ${time}` : (done ?? time)].filter(Boolean).join(" · ");
  }
  const result = f.kill === true ? "Kill" : f.kill === false ? "Wipe" : null;
  return [f.name ?? "Boss", difficultyName(f.difficulty), result, time].filter(Boolean).join(" · ");
}

export function kindWord(k: Kind | string | null): string {
  return k === "key" ? "Key" : k === "encounter" ? "Pull" : "Unfinished";
}

/** A file's contents, before anything is sent: "12 pulls and 3 keys". */
export function fileContents(f: Pick<BacklogFile, "encounters" | "keys" | "analysed">): string {
  if (!f.analysed) return "Not read yet";
  if (f.encounters === 0 && f.keys === 0) return "No pulls or keys";
  const parts: string[] = [];
  if (f.encounters > 0) parts.push(formatCount(f.encounters, "boss pull"));
  if (f.keys > 0) parts.push(formatCount(f.keys, "key"));
  return parts.join(" and ");
}

/** Error codes from the app, in our words: what failed and what to do next. */
export function errorText(code: string): string {
  switch (code) {
    case "offline":
      return "Couldn't reach mythics.gg. Check your connection; the app tries again by itself.";
    case "busy":
      return "mythics.gg is busy. The app tries again in a moment.";
    case "signed_out":
      return "You've been logged out. Log in with Battle.net again to carry on uploading.";
    case "timed_out":
      return "Logging in took too long. Select Log in with Battle.net to try again.";
    case "cancelled":
      return "Logging in was cancelled.";
    case "state":
      return "That sign-in didn't come from this app. Select Log in with Battle.net to try again.";
    case "listen":
      return "The app couldn't wait for your browser. Check your firewall allows it, then try again.";
    case "path":
      return "The app couldn't open that page. Check the site address in Settings, or find the log under My logs on mythics.gg.";
    case "browser":
      return "Your browser didn't open. Check you have a default browser set, then try again.";
    case "keyring":
      return "The app couldn't save your sign-in in Windows Credential Manager. Try again.";
    case "no_logs":
      return "That folder has no Logs folder in it. Choose the World of Warcraft folder, or the _retail_ folder inside it.";
    case "missing":
      return "That folder doesn't exist. Choose another.";
    case "not_found":
      return "The app couldn't find World of Warcraft. Choose its folder yourself.";
    case "no_folder":
      return "Choose your World of Warcraft folder first, in Settings.";
    case "origin":
      return "That address isn't allowed. Use https, or http to this computer.";
    case "autostart":
      return "Windows didn't let the app change that. Try again.";
    case "log_too_new":
      return "This log is from a newer game version than mythics.gg reads yet. It will be read once the site catches up.";
    case "file_missing":
      return "The log file was moved or deleted before it could be sent.";
    case "file_changed":
      return "The log file changed before it could be sent.";
    default:
      return "Something went wrong. Try again in a moment.";
  }
}

/** Said of a pull or key the app didn't send, as a raid member had. */
export const ALREADY = "Already on mythics.gg (uploaded by a raid member)";

/** A queued pull's status, in words. */
export function pullStatus(p: Pick<Pull, "state" | "progress_pct" | "error"> & { already?: boolean }): string {
  if (p.already) return ALREADY;
  switch (p.state) {
    case "done":
      return "On site ✓";
    case "uploading":
      return `Uploading ${p.progress_pct ?? 0}%`;
    case "failed":
      return "Not sent";
    default:
      return p.error === "offline" || p.error === "server_busy" ? "Waiting to retry" : "Waiting";
  }
}
