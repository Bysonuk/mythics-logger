import { describe, expect, it } from "vitest";
import {
  errorText,
  fightText,
  fileContents,
  formatBytes,
  formatDate,
  formatDuration,
  pullResult,
  pullStatus,
  pullTitle,
  summaryResult,
} from "../src/format";
import type { Fight } from "../src/types";

describe("formatting", () => {
  it("the pulls and keys the site found", () => {
    const f = (over: Partial<Fight>): Fight => ({
      id: "9",
      kind: "encounter",
      name: "Plexus Sentinel",
      difficulty: 16,
      key_level: null,
      kill: true,
      duration_ms: 312_499,
      parent_id: null,
      url: "/logs/41/pulls/9/",
      ...over,
    });
    expect(fightText(f({}))).toBe("Plexus Sentinel · Mythic · Kill · 5:12");
    expect(fightText(f({ kill: false, duration_ms: 252_000 }))).toBe("Plexus Sentinel · Mythic · Wipe · 4:12");
    expect(fightText(f({ kind: "key", name: "The Blinding Vale", key_level: 14, duration_ms: 1_681_247, difficulty: null }))).toBe(
      "The Blinding Vale +14 · Completed in 28:01",
    );
    expect(fightText(f({ kind: "key", name: null, key_level: null, kill: null, duration_ms: null }))).toBe("Mythic+ key");
  });

  it("sizes", () => {
    expect(formatBytes(11_286_000_000)).toBe("11.3 GB");
    expect(formatBytes(412_000_000)).toBe("412 MB");
    expect(formatBytes(86_105_564)).toBe("86.1 MB");
    expect(formatBytes(1)).toBe("1 byte");
  });

  it("durations", () => {
    expect(formatDuration(252_000)).toBe("4:12");
    expect(formatDuration(1_681_247)).toBe("28:01");
    expect(formatDuration(3_852_000)).toBe("1:04:12");
  });

  it("dates, as the site writes them", () => {
    expect(formatDate("2026-09-26T11:16:00+01:00")).toBe("26 Sept 2026");
    expect(formatDate(null)).toBe("Unknown date");
  });

  it("a pull's result", () => {
    const base = { kind: "encounter", opened_as: "encounter", key_level: null } as const;
    expect(pullResult({ ...base, success: false, boss_hp_pct: 23.4, duration_ms: 220_000 })).toBe("Wipe · 23.4% · 3:40");
    expect(pullResult({ ...base, success: true, boss_hp_pct: null, duration_ms: 312_499 })).toBe("Kill · 5:12");
    expect(pullResult({ kind: "key", opened_as: "key", key_level: 14, success: true, boss_hp_pct: null, duration_ms: 1_681_247 })).toBe(
      "Completed in 28:01",
    );
    expect(pullResult({ ...base, kind: "segment", success: null, boss_hp_pct: null, duration_ms: null })).toBe("Pull not finished");
    expect(pullTitle({ kind: "key", opened_as: "key", name: "The Blinding Vale", key_level: 14, difficulty: 8 })).toBe("The Blinding Vale +14");
  });

  it("a summarised wipe's result", () => {
    expect(summaryResult(42)).toBe("Wipe, 42.0% (details not uploaded)");
    expect(summaryResult(23.45)).toBe("Wipe, 23.4% (details not uploaded)");
    expect(summaryResult(null)).toBe("Wipe (details not uploaded)");
  });

  it("a pull's status, as the mockup says it", () => {
    expect(pullStatus({ state: "done", progress_pct: null, error: null })).toBe("On site ✓");
    expect(pullStatus({ state: "uploading", progress_pct: 64, error: null })).toBe("Uploading 64%");
    expect(pullStatus({ state: "waiting", progress_pct: null, error: "offline" })).toBe("Waiting to retry");
  });

  it("a file's contents before upload", () => {
    expect(fileContents({ analysed: true, encounters: 12, keys: 3 })).toBe("12 boss pulls and 3 keys");
    expect(fileContents({ analysed: true, encounters: 1, keys: 0 })).toBe("1 boss pull");
    expect(fileContents({ analysed: false, encounters: 0, keys: 0 })).toBe("Not read yet");
  });

  it("errors say what failed and what to do, never the raw code", () => {
    for (const code of ["offline", "timed_out", "no_logs", "log_too_new", "refused_418", "something_new"]) {
      const text = errorText(code);
      expect(text).not.toContain(code);
      expect(text).toMatch(/\.$/);
    }
  });
});
