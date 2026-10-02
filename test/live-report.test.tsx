// The live report's link on the Live tab (this repository's issue 15), in
// each state the app sends, against the fake app. The link's token is made up.
import { render } from "preact";
import { act } from "preact/test-utils";
import { afterEach, describe, expect, it, vi } from "vitest";
import { App } from "../src/App";
import type { LiveShareView, Snapshot } from "../src/types";
import { fakeBridge, snapshot } from "./fake";

const URL = "https://mythics.gg/shared/AbCdEfGhIjKlMnOpQr_-12/";

let host: HTMLElement;
afterEach(() => {
  render(null, host);
  document.body.innerHTML = "";
  vi.unstubAllGlobals();
});

function withShare(share: Partial<LiveShareView>, over: Partial<Snapshot> = {}): Snapshot {
  return snapshot({ live_share: { status: "waiting", url: null, visibility: null, error: null, ...share }, ...over });
}

async function mount(snap: Snapshot) {
  const bridge = fakeBridge(snap);
  host = document.createElement("div");
  document.body.append(host);
  await act(async () => {
    render(<App bridge={bridge} initialTab="live" />, host);
  });
  await act(async () => {
    await new Promise((r) => setTimeout(r, 0));
  });
  return bridge;
}

const text = () => host.textContent ?? "";
const card = () => [...host.querySelectorAll(".card")].find((c) => c.querySelector("h2")?.textContent === "Live report") ?? null;
const hasButton = (label: string) => [...host.querySelectorAll("button")].some((x) => x.textContent?.trim() === label);
function button(label: string): HTMLButtonElement {
  const b = [...host.querySelectorAll("button")].find((x) => x.textContent?.trim() === label);
  if (!b) throw new Error(`no button ${label}`);
  return b;
}
async function click(el: HTMLElement) {
  await act(async () => {
    el.click();
  });
  await act(async () => {
    await new Promise((r) => setTimeout(r, 0));
  });
}

describe("the live report link", () => {
  it("isn't shown while not live logging", async () => {
    const snap = withShare({ status: "off" });
    await mount(snap);
    expect(card()).toBeNull();
    const off = withShare({ status: "waiting" });
    off.settings.live_logging = false;
    await act(async () => render(null, host));
    await mount(off);
    expect(card()).toBeNull();
    expect(text()).not.toContain("live report");
  });

  it("waits for the first pull to upload", async () => {
    await mount(withShare({ status: "waiting" }));
    expect(card()?.textContent).toContain("Your live report link appears here once the first pull uploads.");
    expect(host.querySelector("#live-report-link")).toBeNull();
    expect(hasButton("Copy link")).toBe(false);
  });

  it("says it's making the link", async () => {
    await mount(withShare({ status: "creating" }));
    expect(card()?.textContent).toContain("Making your live report link…");
  });

  it("shows the link, copies it, opens it and stops sharing", async () => {
    const writeText = vi.fn(() => Promise.resolve());
    vi.stubGlobal("navigator", { ...navigator, clipboard: { writeText } });
    const bridge = await mount(withShare({ status: "ready", url: URL }));
    const field = host.querySelector<HTMLInputElement>("#live-report-link")!;
    expect(field.value).toBe(URL);
    expect(field.readOnly).toBe(true);
    expect(host.querySelector('label[for="live-report-link"]')?.textContent).toBe("Live report link");
    expect(card()?.textContent).toContain("Anyone with the link can follow this log's Public pulls");

    await click(button("Copy link"));
    expect(writeText).toHaveBeenCalledWith(URL);
    expect(card()?.querySelector('[role="status"]')?.textContent).toBe("Link copied");

    await click(button("Open in browser"));
    expect(bridge.calls).toContainEqual(["liveShareOpen"]);

    await click(button("Stop sharing"));
    expect(bridge.calls).toContainEqual(["liveShareRevoke"]);
    expect(text()).toContain("Sharing stopped. The link no longer works.");
  });

  it("says how to copy by hand when the clipboard refuses", async () => {
    vi.stubGlobal("navigator", { ...navigator, clipboard: { writeText: () => Promise.reject(new Error("denied")) } });
    await mount(withShare({ status: "ready", url: URL }));
    await click(button("Copy link"));
    expect(card()?.textContent).toContain("Couldn't copy. Select the link and press Ctrl+C.");
  });

  it("says why a Private or Guild only log has no link, and how to change it", async () => {
    for (const [visibility, name] of [
      ["private", "Private"],
      ["guild", "Guild only"],
    ] as const) {
      await mount(withShare({ status: "not_public", visibility }));
      const words = card()?.textContent ?? "";
      expect(words).toContain(`its pulls upload as ${name}, and a live report shows Public pulls only`);
      expect(words).toContain("set Visibility for new uploads to Public in Settings, or make a pull Public in History");
      expect(host.querySelector("#live-report-link")).toBeNull();
      expect(hasButton("Copy link")).toBe(false);
      render(null, host);
      document.body.innerHTML = "";
    }
  });

  it("after Stop sharing, offers a new link", async () => {
    const bridge = await mount(withShare({ status: "revoked" }));
    expect(card()?.textContent).toContain("Sharing is off for this log. The old link no longer works.");
    expect(host.querySelector("#live-report-link")).toBeNull();
    await click(button("Make a new link"));
    expect(bridge.calls).toContainEqual(["liveShareNew"]);
  });

  it("hides the feature, with a note, while mythics.gg doesn't have it", async () => {
    await mount(withShare({ status: "unavailable" }));
    expect(card()?.textContent).toContain("Live report links aren't available from mythics.gg yet.");
    expect(card()?.querySelectorAll("button").length).toBe(0);
  });

  it("logging out stops the link; when it can't, says to stop it on mythics.gg", async () => {
    const snap = withShare({ status: "ready", url: URL });
    const bridge = fakeBridge(snap);
    // The app logged out, but couldn't reach mythics.gg to stop the link.
    bridge.logOut = (...args: unknown[]) => {
      bridge.calls.push(["logOut", ...args]);
      return Promise.reject("share_stop_failed");
    };
    host = document.createElement("div");
    document.body.append(host);
    await act(async () => {
      render(<App bridge={bridge} initialTab="settings" />, host);
    });
    await act(async () => {
      await new Promise((r) => setTimeout(r, 0));
    });
    await click(button("Log out"));
    expect(bridge.calls).toContainEqual(["logOut"]);
    expect(text()).toContain("Couldn't stop your live report link. Stop it on mythics.gg under Your logs.");
  });

  it("moving to another site address says the same when the link couldn't be stopped", async () => {
    const bridge = fakeBridge(withShare({ status: "ready", url: URL }, { dev: true }));
    bridge.saveSettings = () => Promise.reject("share_stop_failed");
    host = document.createElement("div");
    document.body.append(host);
    await act(async () => {
      render(<App bridge={bridge} initialTab="settings" />, host);
    });
    await act(async () => {
      await new Promise((r) => setTimeout(r, 0));
    });
    const form = host.querySelector<HTMLFormElement>("#origin")?.closest("form");
    expect(form).toBeTruthy();
    await act(async () => {
      form!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    });
    await act(async () => {
      await new Promise((r) => setTimeout(r, 0));
    });
    expect(text()).toContain("Couldn't stop your live report link. Stop it on mythics.gg under Your logs.");
  });

  it("says what failed, in its own words, and tries again", async () => {
    const bridge = await mount(withShare({ status: "error", error: "offline" }));
    expect(card()?.textContent).toContain("Couldn't make your live report link. Couldn't reach mythics.gg.");
    await click(button("Try again"));
    expect(bridge.calls).toContainEqual(["liveShareNew"]);
  });
});
