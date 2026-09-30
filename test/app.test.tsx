import { render } from "preact";
import { act } from "preact/test-utils";
import { afterEach, describe, expect, it, vi } from "vitest";
import { App } from "../src/App";
import type { TabId } from "../src/components";
import type { Fight, History, HistoryRow, Snapshot } from "../src/types";
import { fakeBridge, pull, snapshot } from "./fake";

let host: HTMLElement;
afterEach(() => {
  render(null, host);
  document.body.innerHTML = "";
});

async function mount(snap: Snapshot, tab: TabId = "live", history?: History) {
  const bridge = fakeBridge(snap, history);
  host = document.createElement("div");
  document.body.append(host);
  await act(async () => {
    render(<App bridge={bridge} initialTab={tab} />, host);
  });
  // Let the first getState (and any effect's request) settle.
  await act(async () => {
    await new Promise((r) => setTimeout(r, 0));
  });
  return bridge;
}

const text = () => host.textContent ?? "";
function button(label: string | RegExp): HTMLButtonElement {
  const b = [...host.querySelectorAll("button")].find((x) =>
    typeof label === "string" ? x.textContent?.trim() === label : label.test(x.textContent ?? ""),
  );
  if (!b) throw new Error(`no button ${String(label)}`);
  return b;
}
async function click(el: HTMLElement) {
  await act(async () => {
    el.click();
  });
}

describe("first run", () => {
  it("asks to log in with Battle.net, and says what the app reads", async () => {
    const bridge = await mount(snapshot({ signed_in: false, main: null }));
    expect(host.querySelector("h1")?.textContent).toContain("Logger");
    const login = button("Log in with Battle.net");
    expect(text()).toContain("This opens your browser");
    expect(text()).toContain("reads only the combat log text file");
    expect(text()).toContain("alongside the Warcraft Logs uploader");
    await click(login);
    expect(bridge.calls).toContainEqual(["logIn"]);
  });

  it("waits for the browser, and can cancel", async () => {
    const bridge = await mount(snapshot({ signed_in: false, signing_in: true, main: null }));
    expect(text()).toContain("Waiting for you to log in in your browser");
    await click(button("Cancel"));
    expect(bridge.calls).toContainEqual(["cancelLogIn"]);
  });
});

describe("the header", () => {
  it("shows the main character and guild, and tabs that work by keyboard", async () => {
    await mount(snapshot());
    const account = host.querySelector(".account")!;
    expect(account.textContent).toBe("Player1 · Tarren Mill<Guild One>");
    const tabs = [...host.querySelectorAll<HTMLButtonElement>('[role="tab"]')];
    expect(tabs.map((t) => t.textContent)).toEqual(["Live", "Backlog", "History", "Settings"]);
    expect(tabs[0]!.getAttribute("aria-selected")).toBe("true");
    expect(tabs[1]!.tabIndex).toBe(-1);
    await act(async () => {
      tabs[0]!.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }));
    });
    const now = [...host.querySelectorAll<HTMLButtonElement>('[role="tab"]')];
    expect(now[1]!.getAttribute("aria-selected")).toBe("true");
    expect(host.querySelector("#panel-backlog")!.hasAttribute("hidden")).toBe(false);
    expect(host.querySelector("#panel-live")!.hasAttribute("hidden")).toBe(true);
    await act(async () => {
      now[1]!.dispatchEvent(new KeyboardEvent("keydown", { key: "End", bubbles: true }));
    });
    expect(host.querySelector('[role="tab"][aria-selected="true"]')!.textContent).toBe("Settings");
  });
});

describe("the Live tab", () => {
  it("shows the status card from the mockup", async () => {
    await mount(snapshot());
    const card = host.querySelector(".status-card")!.textContent ?? "";
    expect(card).toContain("Live");
    expect(card).toContain("Manaforge Omega · Mythic");
    expect(card).toContain("WoWCombatLog-092826_200101.txt");
    expect(card).toContain("Advanced logging on ✓");
    expect(card).toContain("Plexus Sentinel");
    expect(card).toContain("Uploads when the pull ends · usually 1–3 min");
  });

  it("lists tonight's pulls with their result and status", async () => {
    await mount(snapshot());
    const rows = [...host.querySelectorAll(".pull")].map((r) => r.textContent ?? "");
    expect(rows).toHaveLength(2);
    expect(rows[0]).toContain("Pull 2");
    expect(rows[0]).toContain("Kill · 5:12");
    expect(rows[0]).toContain("Uploading 64%");
    expect(rows[1]).toContain("Wipe · 23.4%");
    expect(rows[1]).toContain("12.4 MB");
    expect(rows[1]).toContain("On site ✓");
    const bars = host.querySelectorAll("progress");
    expect(bars[0]!.getAttribute("aria-label")).toBe("Plexus Sentinel, pull 2: Uploading 64%");
    expect(text()).toContain("1 pull waiting to upload");
  });

  it("warns when Advanced Combat Logging is off", async () => {
    const s = snapshot();
    s.live.advanced = false;
    await mount(s);
    expect(text()).toContain("Advanced Combat Logging is off");
    expect(text()).not.toContain("Advanced logging on");
  });

  it("says what to do when the game isn't logging", async () => {
    const s = snapshot({ pulls: [] });
    s.live = { ...s.live, status: "waiting", file: null, zone: null, current: null, advanced: null, last_activity_ms: 0 };
    await mount(s);
    expect(text()).toContain("Waiting for a combat log");
    expect(text()).toContain("/combatlog");
    expect(text()).toContain("Pulls and keys appear here as they end.");
  });

  it("shows a key with its level and result", async () => {
    const s = snapshot({
      pulls: [pull({ kind: "key", opened_as: "key", name: "The Blinding Vale", key_level: 14, success: true, duration_ms: 1_681_247, difficulty: 8 })],
    });
    await mount(s);
    const row = host.querySelector(".pull")!.textContent ?? "";
    expect(row).toContain("The Blinding Vale +14");
    expect(row).toContain("Completed in 28:01");
    expect(row).not.toContain("Pull 1");
  });

  it("links the log, the boss and each parsed pull, and says Processing until then", async () => {
    const s = snapshot({
      pulls: [
        pull({ pull_number: 3, upload_id: "43", state: "uploading", progress_pct: 10 }),
        pull({ pull_number: 2, upload_id: "42", server_status: "queued" }),
        pull({
          pull_number: 1,
          upload_id: "41",
          server_status: "parsed",
          log_url: "/logs/12/",
          page_url: "/logs/12/pulls/9/",
          boss_url: "/logs/12/bosses/3129/",
          section: "raid",
        }),
      ],
    });
    const bridge = await mount(s);
    // One log, its raid boss once, over its three pulls.
    expect(host.querySelectorAll(".log")).toHaveLength(1);
    expect(host.querySelector(".log-title")!.textContent).toBe("WoWCombatLog-092826_200101.txt");
    expect([...host.querySelectorAll(".log-section")].map((h) => h.textContent)).toEqual(["Raid"]);
    expect(host.querySelectorAll(".boss")).toHaveLength(1);
    const rows = [...host.querySelectorAll(".pull")];
    expect(rows.map((r) => r.querySelector(".pull-title")!.textContent)).toEqual(["Pull 3", "Pull 2", "Pull 1"]);
    // Still sending: no link and no "Processing" yet, just the upload.
    expect(rows[0]!.querySelector(".site-links")).toBeNull();
    expect(rows[1]!.querySelector(".site-links")!.textContent).toBe("Processing…");
    expect(rows[1]!.textContent).not.toContain("View");
    const one = rows[2]!.querySelector<HTMLButtonElement>(".site-links button")!;
    expect(one.textContent).toBe("View");
    expect(one.getAttribute("aria-label")).toBe("View Plexus Sentinel, pull 1, on mythics.gg");
    await click(button("View log"));
    await click(button("Plexus Sentinel"));
    await click(one);
    expect(bridge.calls).toContainEqual(["openLog", "/logs/12/"]);
    expect(bridge.calls).toContainEqual(["openLog", "/logs/12/bosses/3129/"]);
    expect(bridge.calls).toContainEqual(["openLog", "/logs/12/pulls/9/"]);
    expect(button("Plexus Sentinel").getAttribute("aria-label")).toBe("Plexus Sentinel, Mythic: every pull, on mythics.gg");
  });

  it("groups by log file: Raid by boss with kills marked, and Mythic+ keys", async () => {
    const keyFights: Fight[] = [
      { id: "7", kind: "key", name: "The Blinding Vale", difficulty: null, key_level: 14, kill: true, duration_ms: 1_681_247, parent_id: null, section: "mplus", url: "/logs/12/keys/7/" },
      { id: "8", kind: "encounter", encounter_id: 3199, name: "Lightblossom Trinity", difficulty: 8, key_level: null, kill: true, duration_ms: 158_794, parent_id: "7", in_key: "7", section: "mplus", url: "/logs/12/pulls/8/" },
    ];
    const s = snapshot({
      pulls: [
        pull({ sha256: "k1", kind: "key", opened_as: "key", name: "The Blinding Vale", key_level: 14, difficulty: null, encounter_id: null, success: true, duration_ms: 1_681_247, upload_id: "44", server_status: "parsed", log_url: "/logs/12/", page_url: "/logs/12/keys/7/", section: "mplus", fights: keyFights }),
        pull({ sha256: "r2", name: "Loom'ithar", encounter_id: 3131, pull_number: 1, success: true, upload_id: "43" }),
        pull({ sha256: "r1", pull_number: 2, success: true, upload_id: "42" }),
        pull({ sha256: "r0", pull_number: 1, success: false, upload_id: "41" }),
        pull({ sha256: "o1", group: "k:other", file_name: "WoWCombatLog-092726_190000.txt", upload_id: "40" }),
      ],
    });
    const bridge = await mount(s);
    const logs = [...host.querySelectorAll(".log")];
    expect(logs.map((l) => l.querySelector(".log-title")!.textContent)).toEqual([
      "WoWCombatLog-092826_200101.txt",
      "WoWCombatLog-092726_190000.txt",
    ]);
    const night = logs[0]!;
    expect([...night.querySelectorAll(".log-section")].map((h) => h.textContent)).toEqual(["Raid", "Mythic+"]);
    const bosses = [...night.querySelectorAll(".boss")];
    expect(bosses.map((b) => b.querySelector(".boss-name, .boss-link")!.textContent)).toEqual(["Loom'ithar", "Plexus Sentinel"]);
    // Plexus Sentinel: a wipe, then the kill, marked.
    expect(bosses[1]!.classList.contains("boss-killed")).toBe(true);
    expect(bosses[1]!.textContent).toContain("Killed · 2 pulls");
    const pulls = [...bosses[1]!.querySelectorAll(".pull")];
    expect(pulls.map((p) => p.classList.contains("pull-kill"))).toEqual([true, false]);
    // The key, and the boss inside it, each link to their own page.
    const key = night.querySelector(".log-section + .pulls .pull")!;
    expect(key.textContent).toContain("The Blinding Vale +14");
    const views = [...key.querySelectorAll<HTMLButtonElement>("button")];
    expect(views.map((b) => b.getAttribute("aria-label"))).toEqual([
      "View The Blinding Vale +14, on mythics.gg",
      "View Lightblossom Trinity · Mythic+ · Kill · 2:39, on mythics.gg",
    ]);
    await click(views[0]!);
    await click(views[1]!);
    expect(bridge.calls).toContainEqual(["openLog", "/logs/12/keys/7/"]);
    expect(bridge.calls).toContainEqual(["openLog", "/logs/12/pulls/8/"]);
  });

  it("marks a pull a raid member already uploaded, and links their copy", async () => {
    const s = snapshot({
      pulls: [
        pull({ sha256: "r1", pull_number: 2, success: true, upload_id: "42", server_status: "parsed", log_url: "/logs/12/", page_url: "/logs/12/pulls/9/" }),
        pull({
          sha256: "r0",
          pull_number: 1,
          upload_id: null,
          already: true,
          server_status: "parsed",
          log_url: "/logs/3/",
          page_url: "/logs/3/pulls/31/",
          boss_url: "/logs/3/bosses/3129/",
        }),
      ],
    });
    const bridge = await mount(s);
    const rows = [...host.querySelectorAll(".pull")];
    expect(rows[1]!.textContent).toContain("Already on mythics.gg (uploaded by a raid member)");
    const view = rows[1]!.querySelector<HTMLButtonElement>(".site-links button")!;
    await click(view);
    expect(bridge.calls).toContainEqual(["openLog", "/logs/3/pulls/31/"]);
    // The log's own link is this log's, not the raid member's.
    await click(button("View log"));
    expect(bridge.calls).toContainEqual(["openLog", "/logs/12/"]);
    expect(bridge.calls).not.toContainEqual(["openLog", "/logs/3/"]);
  });

  it("says Processing for a sent pull the server hasn't been asked about yet", async () => {
    await mount(snapshot({ pulls: [pull({ upload_id: "41" })] }));
    expect(host.querySelector(".site-links")!.textContent).toBe("Processing…");
  });

  it("says what went wrong when a page won't open", async () => {
    const s = snapshot({ pulls: [pull({ upload_id: "41", server_status: "parsed", log_url: "/logs/12/" })] });
    const bridge = await mount(s);
    bridge.openLog = () => Promise.reject("path");
    await click(button("View log"));
    await act(async () => {
      await new Promise((r) => setTimeout(r, 0));
    });
    expect(text()).toContain("The app couldn't open that page.");
  });
});

describe("the Backlog tab", () => {
  it("summarises what it found and labels past logs clearly", async () => {
    await mount(snapshot(), "backlog");
    // The file being logged live isn't counted as an older log.
    expect(text()).toContain("Found 2 older logs in …\\Logs (11.2 GB)");
    expect(text()).toContain("never count as live");
    expect(text()).toContain("Pulls already on mythics.gg are skipped");
    button("Choose files…");
    button("Upload all");
  });

  it("shows each file's date, size, pulls and keys, and what's already uploaded", async () => {
    await mount(snapshot(), "backlog");
    const files = [...host.querySelectorAll(".file")];
    const live = files.find((f) => f.textContent?.includes("Being logged live now"))!;
    expect(live.querySelector<HTMLInputElement>("input")!.disabled).toBe(true);
    const past = files.find((f) => f.textContent?.includes("WoWCombatLog-092126_193000.txt"))!;
    expect(past.textContent).toContain("21 Sept 2026");
    expect(past.textContent).toContain("9.0 GB");
    expect(past.textContent).toContain("4 boss pulls and 1 key");
    expect(past.textContent).toContain("2 of 5 pulls already uploaded (skipped)");
    const old = files.find((f) => f.textContent?.includes("RaiderIOLogsArchive"))!;
    expect(old.textContent).toContain("Older log: times are approximate");
  });

  it("uploads all, or only the ticked files, with the chosen visibility", async () => {
    const bridge = await mount(snapshot(), "backlog");
    const select = host.querySelector<HTMLSelectElement>("#backlog-visibility")!;
    expect(select.value).toBe("guild"); // from Settings
    await click(button("Upload all"));
    expect(bridge.calls).toContainEqual([
      "backlogUpload",
      ["C:\\Logs\\WoWCombatLog-092126_193000.txt", "C:\\Logs\\RaiderIOLogsArchive\\WoWCombatLog-112120_120130.txt"],
      "guild",
    ]);
    await act(async () => {
      select.value = "private";
      select.dispatchEvent(new Event("change", { bubbles: true }));
    });
    const box = [...host.querySelectorAll<HTMLInputElement>(".file input")].find((i) => !i.disabled)!;
    await click(box);
    await click(button("Upload 1 chosen log"));
    expect(bridge.calls.at(-1)).toEqual(["backlogUpload", ["C:\\Logs\\WoWCombatLog-092126_193000.txt"], "private"]);
  });

  it("shows overall progress and can pause", async () => {
    const bridge = await mount(snapshot(), "backlog");
    expect(text()).toContain("412 of 1,036 pulls uploaded");
    await click(button("Pause"));
    expect(bridge.calls).toContainEqual(["backlogPause", true]);
  });

  it("shows reading progress for a big file, and can stop", async () => {
    const s = snapshot();
    s.backlog = { ...s.backlog, scanning: true, files_done: 1, files_total: 3, current_name: "WoWCombatLog-081026_212218.txt", current_pct: 45 };
    const bridge = await mount(s, "backlog");
    expect(text()).toContain("Reading 2 of 3 logs: WoWCombatLog-081026_212218.txt");
    expect(host.querySelector<HTMLProgressElement>(".scan progress")!.value).toBe(45);
    await click(button("Stop reading"));
    expect(bridge.calls).toContainEqual(["backlogCancel"]);
  });

  it("says in plain words which pulls go, and about how much, before sending", async () => {
    const bridge = await mount(snapshot(), "backlog");
    const radios = [...host.querySelectorAll<HTMLInputElement>('input[name="backlog-pulls"]')];
    expect(radios.map((r) => r.value)).toEqual(["kills_and_best_wipe", "all"]);
    expect(radios[0]!.checked).toBe(true);
    const group = host.querySelector("fieldset.radios")!;
    expect(group.querySelector("legend")!.textContent).toBe("Pulls to upload");
    expect(group.textContent).toContain("Kills and each boss's best wipe");
    expect(group.textContent).toContain("so your pull count and best % on mythics.gg stay right");
    // Both past logs: 150 MB and 91 MB with this setting.
    const estimate = host.querySelector(".estimate")!;
    expect(estimate.getAttribute("role")).toBe("status");
    expect(estimate.textContent).toBe("All: about 241 MB to upload, with 1 wipe sent as a result only.");
    // Only the ticked file.
    const box = [...host.querySelectorAll<HTMLInputElement>(".file input")].find((i) => !i.disabled)!;
    await click(box);
    expect(host.querySelector(".estimate")!.textContent).toBe(
      "1 chosen log: about 150 MB to upload, with 1 wipe sent as a result only.",
    );
    await click(radios[1]!);
    expect(bridge.calls).toContainEqual(["saveSettings", { backlog_pulls: "all" }]);
  });

  it("estimates every pull in full with All pulls", async () => {
    const s = snapshot();
    s.settings = { ...s.settings, backlog_pulls: "all" };
    await mount(s, "backlog");
    expect(host.querySelector<HTMLInputElement>('input[value="all"]')!.checked).toBe(true);
    expect(host.querySelector(".estimate")!.textContent).toBe("All: about 331 MB to upload.");
  });

  it("looks for logs the first time it opens", async () => {
    const s = snapshot();
    s.backlog = { ...s.backlog, files: [], total: 0, done: 0 };
    const bridge = await mount(s, "backlog");
    expect(bridge.calls).toContainEqual(["backlogScan"]);
  });
});

describe("the Settings tab", () => {
  it("has the visibility default, the folder, start with Windows off, and the guild-only preference", async () => {
    const bridge = await mount(snapshot(), "settings");
    const radios = [...host.querySelectorAll<HTMLInputElement>('input[name="default-visibility"]')];
    expect(radios.map((r) => r.parentElement!.textContent)).toEqual([
      expect.stringContaining("Public"),
      expect.stringContaining("Guild only"),
      expect.stringContaining("Private"),
    ]);
    expect(radios[1]!.checked).toBe(true);
    await click(radios[2]!);
    expect(bridge.calls).toContainEqual(["saveSettings", { default_visibility: "private" }]);
    expect(text()).toContain("_retail_\\Logs");
    const startup = [...host.querySelectorAll<HTMLInputElement>('input[type="checkbox"]')].find((c) =>
      c.parentElement!.textContent!.includes("Start with Windows"),
    )!;
    expect(startup.checked).toBe(false);
    expect(text()).toContain("Only upload my guild's raids and keys");
    expect(text()).toContain("Coming soon");
    // The site address is for development builds only.
    expect(host.querySelector("#origin")).toBeNull();
  });

  it("says what the app reads and sends", async () => {
    await mount(snapshot(), "settings");
    expect(text()).toContain("What the app reads");
    expect(text()).toContain("never lines from your combat log or anyone's name");
    expect(text()).toContain("Private logs are never shown on the site");
  });

  it("logs out", async () => {
    const bridge = await mount(snapshot(), "settings");
    await click(button("Log out"));
    expect(bridge.calls).toContainEqual(["logOut"]);
  });

  it("has a Live logging switch that saves at once", async () => {
    const bridge = await mount(snapshot(), "settings");
    const sw = host.querySelector<HTMLInputElement>('input[role="switch"]')!;
    expect(sw.parentElement!.textContent).toContain("Live logging");
    expect(sw.parentElement!.textContent).toContain("Upload pulls automatically while you play");
    expect(sw.checked).toBe(true);
    await click(sw);
    expect(bridge.calls).toContainEqual(["saveSettings", { live_logging: false }]);
    expect(text()).toContain("Race to World First");
  });

  it("shows the switch off when live logging is off", async () => {
    const s = snapshot();
    s.settings.live_logging = false;
    const bridge = await mount(s, "settings");
    const sw = host.querySelector<HTMLInputElement>('input[role="switch"]')!;
    expect(sw.checked).toBe(false);
    await click(sw);
    expect(bridge.calls).toContainEqual(["saveSettings", { live_logging: true }]);
  });
});

describe("live logging", () => {
  function off(): Snapshot {
    const s = snapshot({ pulls: [] });
    s.settings.live_logging = false;
    s.live = { ...s.live, status: "off", file: null, zone: null, current: null, advanced: null, last_activity_ms: 0 };
    return s;
  }

  it("asks once logged in, before anything else, and keeps Yes", async () => {
    const s = snapshot();
    s.settings.live_logging = false;
    s.settings.live_asked = false;
    const bridge = await mount(s);
    expect(host.querySelector("h1")!.textContent).toBe("Upload your pulls live while you play?");
    expect(text()).toContain("Race to World First");
    expect(text()).toContain("Private and Guild only visibility still apply");
    expect(host.querySelector('[role="tab"]')).toBeNull();
    await click(button("Yes"));
    expect(bridge.calls).toEqual([["saveSettings", { live_logging: true }]]);
  });

  it("keeps Not now, and uploads nothing", async () => {
    const s = snapshot();
    s.settings.live_logging = false;
    s.settings.live_asked = false;
    const bridge = await mount(s);
    await click(button("Not now"));
    expect(bridge.calls).toEqual([["saveSettings", { live_logging: false }]]);
  });

  it("isn't asked again once answered", async () => {
    await mount(off());
    expect(text()).not.toContain("Upload your pulls live while you play?");
    expect(host.querySelectorAll('[role="tab"]')).toHaveLength(4);
  });

  it("says so on the Live tab and in the header, with a way to turn it on", async () => {
    const bridge = await mount(off());
    const card = host.querySelector(".status-card")!.textContent ?? "";
    expect(card).toContain("Live logging is off");
    expect(card).toContain("Backlog");
    expect(card).not.toContain("Watching");
    expect(card).not.toContain("/combatlog");
    expect(host.querySelector(".top")!.textContent).toContain("Live logging off");
    await click(button("Turn on live logging"));
    expect(bridge.calls).toContainEqual(["saveSettings", { live_logging: true }]);
    expect(host.querySelector('[role="status"]')!.textContent).toContain("Live logging is on");
  });

  it("shows nothing about it in the header while it's on", async () => {
    await mount(snapshot());
    expect(host.querySelector(".live-off")).toBeNull();
    expect(text()).not.toContain("Live logging is off");
  });
});

describe("the History tab", () => {
  const history: History = {
    offline: false,
    rows: [
      {
        id: "u7",
        sha256: "ab",
        kind: "encounter",
        name: "Plexus Sentinel",
        difficulty: 16,
        key_level: null,
        success: true,
        start_time: "2026-09-28T20:15:00.000+01:00",
        size: 12_400_000,
        visibility: "public",
        past: true,
        status: "parsed",
        fights: [],
        on_server: true,
        already: false,
        summary: false,
        boss_hp_pct: 0,
        group: "s:3",
        file_name: null,
        encounter_id: 3129,
        log_url: "/logs/3/",
        page_url: "/logs/3/pulls/5/",
        boss_url: "/logs/3/bosses/3129/",
        section: "raid",
      },
    ],
  };

  it("changes visibility and deletes only after confirming", async () => {
    const bridge = await mount(snapshot(), "history", history);
    expect(host.querySelector(".boss-link")!.textContent).toBe("Plexus Sentinel");
    expect(host.querySelector(".boss-head")!.textContent).toContain("Mythic · Killed · 1 pull");
    expect(host.querySelector(".log-title")!.textContent).toBe("Log of 28 Sept 2026");
    expect(text()).toContain("Past log");
    const select = host.querySelector<HTMLSelectElement>("#vis-u7")!;
    await act(async () => {
      select.value = "private";
      select.dispatchEvent(new Event("change", { bubbles: true }));
    });
    expect(bridge.calls).toContainEqual(["setUploadVisibility", "u7", "private"]);
    await click(button("Delete…"));
    expect(bridge.calls.some((c) => c[0] === "deleteUpload")).toBe(false);
    expect(text()).toContain("This can't be undone.");
    await click(button("Delete"));
    expect(bridge.calls).toContainEqual(["deleteUpload", "u7"]);
    expect(host.querySelectorAll(".pull")).toHaveLength(0);
  });

  const key: Fight = {
    id: "12",
    kind: "key",
    name: "The Blinding Vale",
    difficulty: null,
    key_level: 14,
    kill: true,
    duration_ms: 1_681_247,
    parent_id: null,
    section: "mplus",
    url: "/logs/8/keys/12/",
  };
  const boss: Fight = {
    id: "13",
    kind: "encounter",
    name: "Lightblossom Trinity",
    difficulty: 8,
    key_level: null,
    kill: true,
    duration_ms: 90_000,
    parent_id: "12",
    in_key: "12",
    section: "mplus",
    url: "/logs/8/pulls/13/",
  };
  const row = (over: Partial<HistoryRow>): HistoryRow => ({ ...history.rows[0]!, ...over });

  it("groups a log's raid pulls and keys, and links the log, the key and each boss in it", async () => {
    const bridge = await mount(snapshot(), "history", {
      offline: false,
      rows: [
        row({
          id: "8",
          kind: "key",
          name: "The Blinding Vale",
          key_level: 14,
          difficulty: null,
          encounter_id: null,
          log_url: "/logs/3/",
          page_url: "/logs/3/keys/12/",
          boss_url: null,
          section: "mplus",
          fights: [key, boss],
        }),
        history.rows[0]!,
        row({ id: "6", group: "s:2", log_url: "/logs/2/", start_time: "2026-09-21T20:15:00.000+01:00" }),
      ],
    });
    const logs = [...host.querySelectorAll(".log")];
    expect(logs).toHaveLength(2);
    expect([...logs[0]!.querySelectorAll(".log-section")].map((h) => h.textContent)).toEqual(["Raid", "Mythic+"]);
    const keyRow = logs[0]!.querySelectorAll(".log-section + .pulls .pull")[0]!;
    expect(keyRow.textContent).toContain("The Blinding Vale +14");
    expect(keyRow.querySelector(".fight-in-key")!.textContent).toContain("Lightblossom Trinity · Mythic+ · Kill · 1:30");
    const [viewLog] = [...logs[0]!.querySelectorAll<HTMLButtonElement>(".log-head button")];
    expect(viewLog!.getAttribute("aria-label")).toBe("View log: Log of 28 Sept 2026, on mythics.gg");
    await click(viewLog!);
    expect(bridge.calls).toContainEqual(["openLog", "/logs/3/"]);
    const views = [...keyRow.querySelectorAll<HTMLButtonElement>(".site-links button")];
    expect(views.map((b) => b.textContent)).toEqual(["View", "View"]);
    await click(views[0]!);
    await click(views[1]!);
    expect(bridge.calls).toContainEqual(["openLog", "/logs/3/keys/12/"]);
    expect(bridge.calls).toContainEqual(["openLog", "/logs/8/pulls/13/"]);
    await click(logs[0]!.querySelector<HTMLButtonElement>(".boss-link")!);
    expect(bridge.calls).toContainEqual(["openLog", "/logs/3/bosses/3129/"]);
  });

  it("says Processing until the log is parsed, asking gently, then links it", async () => {
    vi.useFakeTimers();
    try {
      const bridge = fakeBridge(
        snapshot(),
        { offline: false, rows: [row({ id: "9", status: "queued", log_url: null, page_url: null, boss_url: null, section: null })] },
        [{ id: "9", status: "queued", url: null, fights: [] }],
      );
      host = document.createElement("div");
      document.body.append(host);
      await act(async () => {
        render(<App bridge={bridge} initialTab="history" />, host);
      });
      await act(async () => {
        await vi.advanceTimersByTimeAsync(0);
      });
      expect(host.querySelector(".site-processing")?.textContent).toBe("Processing…");
      expect(text()).not.toContain("View log");
      expect(host.querySelector(".boss-link")).toBeNull();
      const asked = () => bridge.calls.filter((c) => c[0] === "recentUploads").length;
      await act(async () => {
        await vi.advanceTimersByTimeAsync(14_000);
      });
      expect(asked()).toBe(0);
      await act(async () => {
        await vi.advanceTimersByTimeAsync(1_000);
      });
      expect(asked()).toBe(1);
      // Nothing changed: the next ask waits twice as long.
      await act(async () => {
        await vi.advanceTimersByTimeAsync(29_000);
      });
      expect(asked()).toBe(1);
      await act(async () => {
        await vi.advanceTimersByTimeAsync(1_000);
      });
      expect(asked()).toBe(2);
      // Now it's parsed: the links appear and the asking stops.
      bridge.recentUploads = () => {
        bridge.calls.push(["recentUploads"]);
        return Promise.resolve([
          {
            id: "9",
            status: "parsed",
            session_id: "4",
            log_url: "/logs/4/",
            url: "/logs/4/",
            fights: [
              {
                ...boss,
                id: "20",
                encounter_id: 3129,
                name: "Plexus Sentinel",
                difficulty: 16,
                parent_id: null,
                in_key: null,
                section: "raid" as const,
                url: "/logs/4/pulls/20/",
                boss_url: "/logs/4/bosses/3129/",
              },
            ],
          },
        ]);
      };
      await act(async () => {
        await vi.advanceTimersByTimeAsync(60_000);
      });
      expect(asked()).toBe(3);
      expect(host.querySelector(".site-processing")).toBeNull();
      await click(button("View log"));
      expect(bridge.calls).toContainEqual(["openLog", "/logs/4/"]);
      await click(button("View"));
      expect(bridge.calls).toContainEqual(["openLog", "/logs/4/pulls/20/"]);
      await click(host.querySelector<HTMLButtonElement>(".boss-link")!);
      expect(bridge.calls).toContainEqual(["openLog", "/logs/4/bosses/3129/"]);
      await act(async () => {
        await vi.advanceTimersByTimeAsync(600_000);
      });
      expect(asked()).toBe(3);
    } finally {
      vi.useRealTimers();
    }
  });

  it("shows a pull that wasn't sent, as a raid member had, with their copy's page", async () => {
    const bridge = await mount(snapshot(), "history", {
      offline: false,
      rows: [row({ id: null, on_server: false, already: true, status: "parsed", page_url: "/logs/3/pulls/31/", log_url: "/logs/3/" })],
    });
    expect(text()).toContain("Already on mythics.gg (uploaded by a raid member)");
    expect(host.querySelector("select")).toBeNull();
    expect(text()).not.toContain("Delete");
    await click(button("View"));
    expect(bridge.calls).toContainEqual(["openLog", "/logs/3/pulls/31/"]);
  });

  it("shows a past log's summarised wipes with their result, and their page", async () => {
    const bridge = await mount(snapshot(), "history", {
      offline: false,
      rows: [
        history.rows[0]!,
        row({
          id: "70",
          kind: "summary",
          success: false,
          summary: true,
          boss_hp_pct: 42.17,
          size: 0,
          start_time: "2026-09-28T20:05:00.000+01:00",
          page_url: "/logs/3/pulls/90/",
        }),
      ],
    });
    const boss = host.querySelector(".boss")!;
    expect(boss.querySelector(".boss-head")!.textContent).toContain("Killed · 2 pulls");
    const pulls = [...boss.querySelectorAll(".pull")];
    const wipe = pulls.find((p) => p.textContent?.includes("details not uploaded"))!;
    expect(wipe.querySelector(".pull-title")!.textContent).toBe("Wipe, 42.2% (details not uploaded)");
    expect(wipe.textContent).not.toContain("0 bytes");
    await click([...wipe.querySelectorAll<HTMLButtonElement>("button")].find((b) => b.textContent === "View")!);
    expect(bridge.calls).toContainEqual(["openLog", "/logs/3/pulls/90/"]);
  });

  it("says when mythics.gg couldn't read a log", async () => {
    await mount(snapshot(), "history", { offline: false, rows: [row({ status: "failed", log_url: null, page_url: null, boss_url: null })] });
    expect(text()).toContain("mythics.gg couldn't read this log");
    expect(text()).not.toContain("View log");
  });
});

describe("words", () => {
  it("never shows a BattleTag or the word token", async () => {
    await mount(snapshot());
    for (const tab of ["Backlog", "History", "Settings"]) {
      await click([...host.querySelectorAll<HTMLButtonElement>('[role="tab"]')].find((t) => t.textContent === tab)!);
      expect(text()).not.toMatch(/#\d{4}/);
      expect(text().toLowerCase()).not.toContain("token");
      expect(text().toLowerCase()).not.toContain("battletag");
    }
  });
});
