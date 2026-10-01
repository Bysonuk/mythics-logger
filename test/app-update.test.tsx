// The app's own updates in the window: "Version X is ready" with the
// release notes, Update now / Later, and Settings' Check for updates.
import { render } from "preact";
import { act } from "preact/test-utils";
import { afterEach, describe, expect, it } from "vitest";
import { App } from "../src/App";
import type { TabId } from "../src/components";
import { releaseNotes } from "../src/format";
import type { AppUpdateView, Snapshot } from "../src/types";
import { fakeBridge, snapshot } from "./fake";

let host: HTMLElement;
afterEach(() => {
  if (host) render(null, host);
  document.body.innerHTML = "";
});

async function mount(snap: Snapshot, tab: TabId = "live") {
  const bridge = fakeBridge(snap);
  host = document.createElement("div");
  document.body.append(host);
  await act(async () => {
    render(<App bridge={bridge} initialTab={tab} />, host);
  });
  await act(async () => {
    await new Promise((r) => setTimeout(r, 0));
  });
  return bridge;
}

const button = (label: string) => {
  const b = [...host.querySelectorAll("button")].find((x) => x.textContent?.trim() === label);
  if (!b) throw new Error(`no button ${label}`);
  return b;
};
const offer = () => host.querySelector(".update-offer");

// GitHub's generated notes, as the release workflow writes them.
const NOTES = [
  "## What's Changed",
  "* Install the mythics.gg addon and keep it up to date, if the player says yes by @Bysonuk in https://github.com/Bysonuk/mythics-logger/pull/9",
  "* Remove our own data packs a new addon release no longer has by @Bysonuk in https://github.com/Bysonuk/mythics-logger/pull/11",
  "",
  "**Full Changelog**: https://github.com/Bysonuk/mythics-logger/compare/v0.1.1...v0.1.2",
].join("\r\n");

function ready(over: Partial<AppUpdateView> = {}): Snapshot {
  const s = snapshot();
  s.app_update = {
    available: { version: "0.1.2", notes: NOTES, date: "2026-10-02T09:00:00Z" },
    offer: true,
    status: "idle",
    progress_pct: null,
    error: null,
    checked_ms: Date.now() - 60_000,
    ...over,
  };
  return s;
}

describe("release notes", () => {
  it("lists what changed, without who and the link", () => {
    expect(releaseNotes(NOTES)).toEqual([
      "Install the mythics.gg addon and keep it up to date, if the player says yes",
      "Remove our own data packs a new addon release no longer has",
    ]);
    expect(releaseNotes(null)).toEqual([]);
  });
});

describe("a new version of the app", () => {
  it("says it's ready, with what changed, and installs only on Update now", async () => {
    const bridge = await mount(ready());
    expect(offer()!.querySelector("h2")!.textContent).toBe("Version 0.1.2 is ready");
    const items = [...offer()!.querySelectorAll("li")].map((l) => l.textContent);
    expect(items).toEqual([
      "Install the mythics.gg addon and keep it up to date, if the player says yes",
      "Remove our own data packs a new addon release no longer has",
    ]);
    expect(bridge.calls).toEqual([]);
    await act(async () => {
      button("Update now").click();
    });
    expect(bridge.calls).toContainEqual(["appUpdateInstall"]);
  });

  it("goes away on Later", async () => {
    const bridge = await mount(ready());
    await act(async () => {
      button("Later").click();
    });
    expect(bridge.calls).toEqual([["appUpdateLater"]]);
  });

  it("isn't shown with nothing new, or once put off", async () => {
    await mount(snapshot());
    expect(offer()).toBeNull();
    await mount(ready({ offer: false }));
    expect(offer()).toBeNull();
  });

  it("shows the download, and says when it failed", async () => {
    await mount(ready({ status: "downloading", progress_pct: 42 }));
    expect(offer()!.textContent).toContain("Downloading… 42%");
    expect(button("Update now").disabled).toBe(true);
    expect(button("Later").disabled).toBe(true);
    await mount(ready({ error: "update_install" }));
    expect(offer()!.textContent).toContain("didn't pass its signature check, so nothing changed");
  });
});

describe("Settings > About and privacy", () => {
  it("shows the version and checks for updates when asked", async () => {
    const bridge = await mount(snapshot(), "settings");
    expect(host.textContent).toContain("Version 0.1.0");
    expect(host.textContent).toContain("It asks GitHub");
    await act(async () => {
      button("Check for updates").click();
    });
    expect(bridge.calls).toContainEqual(["appUpdateCheck"]);
  });

  it("says when it's up to date, or offers a version put off with Later", async () => {
    const s = snapshot();
    s.app_update = { ...s.app_update, checked_ms: Date.now() - 2 * 3_600_000 };
    await mount(s, "settings");
    expect(host.textContent).toContain("Up to date (checked 2 hours ago).");
    const bridge = await mount(ready({ offer: false }), "settings");
    expect(host.textContent).toContain("Version 0.1.2 is ready.");
    await act(async () => {
      button("Update now").click();
    });
    expect(bridge.calls).toContainEqual(["appUpdateInstall"]);
  });
});
