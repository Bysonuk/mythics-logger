// The in-game addon in the window: the first-run question and Settings >
// In-game addon, against the fake app.
import { render } from "preact";
import { act } from "preact/test-utils";
import { afterEach, describe, expect, it } from "vitest";
import { App } from "../src/App";
import type { TabId } from "../src/components";
import type { AddonView, Snapshot } from "../src/types";
import { fakeBridge, snapshot } from "./fake";

let host: HTMLElement;
afterEach(() => {
  render(null, host);
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

const text = () => host.textContent ?? "";
function button(label: string): HTMLButtonElement {
  const b = [...host.querySelectorAll("button")].find((x) => x.textContent?.trim() === label);
  if (!b) throw new Error(`no button ${label}`);
  return b;
}
const hasButton = (label: string) => [...host.querySelectorAll("button")].some((x) => x.textContent?.trim() === label);
async function click(el: HTMLElement) {
  await act(async () => {
    el.click();
  });
  await act(async () => {
    await new Promise((r) => setTimeout(r, 0));
  });
}

/** Not asked yet, the game found, no addon installed. */
function unasked(addon: Partial<AddonView> = {}): Snapshot {
  const s = snapshot();
  s.settings.addon_asked = false;
  s.settings.addon_auto_update = false;
  s.addon = {
    ...s.addon,
    present: false,
    installed: null,
    latest: null,
    availability: "unknown",
    action: null,
    checked_ms: null,
    ...addon,
  };
  return s;
}

function withAddon(addon: Partial<AddonView>, auto = true): Snapshot {
  const s = snapshot();
  s.settings.addon_auto_update = auto;
  s.addon = { ...s.addon, ...addon };
  return s;
}

const card = () => [...host.querySelectorAll(".card")].find((c) => c.querySelector("h2")?.textContent === "In-game addon")!;

describe("the addon question", () => {
  it("asks once the game is found, and Yes installs it and keeps it up to date", async () => {
    const bridge = await mount(unasked());
    expect(host.querySelector("h1")!.textContent).toBe("Install the mythics.gg addon?");
    expect(text()).toContain("Interface\\AddOns");
    expect(text()).toContain("when the game isn't running");
    expect(text()).toContain("never changes other addons or your saved settings");
    expect(host.querySelector('[role="tab"]')).toBeNull();
    await click(button("Yes"));
    expect(bridge.calls).toEqual([["saveSettings", { addon_auto_update: true }], ["addonInstall"]]);
  });

  it("keeps Not now, and installs nothing", async () => {
    const bridge = await mount(unasked());
    await click(button("Not now"));
    expect(bridge.calls).toEqual([["saveSettings", { addon_auto_update: false }]]);
  });

  it("asks about updates when the addon is there already", async () => {
    await mount(unasked({ present: true, installed: "2.0.0" }));
    expect(host.querySelector("h1")!.textContent).toBe("Keep the mythics.gg addon up to date?");
  });

  it("waits for the game's folder, and comes after the live logging question", async () => {
    await mount(unasked({ game_found: false }));
    expect(text()).not.toContain("Install the mythics.gg addon?");
    expect(host.querySelectorAll('[role="tab"]')).toHaveLength(4);

    const s = unasked();
    s.settings.live_asked = false;
    await mount(s);
    expect(host.querySelector("h1")!.textContent).toBe("Upload your pulls live while you play?");
  });

  it("isn't asked again once answered", async () => {
    await mount(withAddon({}, false));
    expect(text()).not.toContain("Install the mythics.gg addon?");
  });
});

describe("Settings > In-game addon", () => {
  it("shows the switch, the installed and latest versions, and Check now", async () => {
    const bridge = await mount(snapshot(), "settings");
    const c = card();
    const sw = c.querySelector<HTMLInputElement>('input[role="switch"]')!;
    expect(sw.checked).toBe(true);
    expect(c.textContent).toContain("Keep the addon up to date");
    expect(c.textContent).toContain("Installed2.1.0");
    expect(c.textContent).toContain("Latest2.1.0");
    expect(c.textContent).toContain("Checked 4 hours ago");
    expect(hasButton("Update")).toBe(false);
    expect(hasButton("Install")).toBe(false);
    await click(button("Check now"));
    expect(bridge.calls).toContainEqual(["addonCheck"]);
    await act(async () => {
      sw.click();
    });
    expect(bridge.calls).toContainEqual(["saveSettings", { addon_auto_update: false }]);
  });

  it("offers Update for an older version, Repair for missing files, Install when it isn't there", async () => {
    let bridge = await mount(withAddon({ installed: "2.0.0", latest: "2.1.0", action: "update" }), "settings");
    expect(card().textContent).toContain("Installed2.0.0");
    await click(button("Update"));
    expect(bridge.calls).toContainEqual(["addonInstall"]);

    bridge = await mount(withAddon({ action: "repair" }), "settings");
    expect(card().textContent).toContain("2.1.0, some files missing");
    expect(hasButton("Repair")).toBe(true);

    bridge = await mount(withAddon({ present: false, installed: null, action: "install" }, false), "settings");
    expect(card().textContent).toContain("Not installed");
    await click(button("Install"));
    expect(bridge.calls).toContainEqual(["addonInstall"]);
  });

  it("says an update waits for the game to close", async () => {
    await mount(withAddon({ installed: "2.0.0", action: "update", status: "waiting_for_game" }), "settings");
    expect(card().textContent).toContain("An addon update is ready; it installs when you close the game.");
  });

  it("says when mythics.gg doesn't publish the addon yet, or it hasn't looked", async () => {
    await mount(withAddon({ latest: null, availability: "unavailable", action: null }), "settings");
    expect(card().textContent).toContain("Not available from mythics.gg yet");
    await mount(withAddon({ latest: null, availability: "unknown", action: null, checked_ms: null }, false), "settings");
    expect(card().textContent).toContain("Not checked yet");
  });

  it("says what went wrong in its own words, and shows progress", async () => {
    await mount(withAddon({ error: "addon_checksum" }), "settings");
    expect(card().textContent).toContain("didn't match its checksum, so nothing was installed");
    await mount(withAddon({ status: "installing", installed: "2.0.0", action: "update" }), "settings");
    expect(card().textContent).toContain("Installing…");
    expect(button("Update").disabled).toBe(true);
    expect(button("Check now").disabled).toBe(true);
  });

  it("leaves a newer test version or a linked folder alone", async () => {
    await mount(withAddon({ installed: "2.2.0-test.1", action: "newer" }), "settings");
    expect(card().textContent).toContain("newer version than mythics.gg's");
    expect(hasButton("Update")).toBe(false);
    await mount(withAddon({ action: "linked" }), "settings");
    expect(card().textContent).toContain("links to another folder");
  });

  it("asks for the game's folder when it isn't known", async () => {
    await mount(withAddon({ game_found: false, present: false, installed: null, action: null }), "settings");
    expect(card().textContent).toContain("hasn't found World of Warcraft's _retail_ folder");
    expect(hasButton("Install")).toBe(false);
  });
});
