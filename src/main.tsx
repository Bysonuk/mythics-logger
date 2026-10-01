import "@fontsource-variable/oxanium";
import "@fontsource-variable/ibm-plex-sans";
import "./styles/tokens.css";
import "./styles/app.css";
import { render } from "preact";
import { App } from "./App";
import { type Bridge, tauriBridge } from "./bridge";

async function bridge(): Promise<Bridge> {
  // `npm run dev` in a plain browser (no Tauri): the tests' fake app, so the
  // views can be looked at and screenshotted. Never in a build.
  if (import.meta.env.DEV && !("__TAURI_INTERNALS__" in window)) {
    const { fakeBridge, snapshot } = await import("../test/fake");
    const q = new URLSearchParams(location.search);
    const snap = snapshot(q.has("first-run") ? { signed_in: false, main: null } : {});
    // ?ask-live: the live logging question; ?live-off: the app with it off.
    if (q.has("ask-live") || q.has("live-off")) {
      snap.settings.live_logging = false;
      snap.settings.live_asked = q.has("live-off");
      snap.live = { ...snap.live, status: "off", file: null, zone: null, current: null, advanced: null };
    }
    // ?ask-addon: the addon question; ?addon-update: an update waiting for the game.
    if (q.has("ask-addon")) {
      snap.settings.addon_asked = false;
      snap.addon = { ...snap.addon, present: false, installed: null, action: null };
    }
    if (q.has("addon-update")) {
      snap.addon = { ...snap.addon, installed: "2.0.0", action: "update", status: "waiting_for_game" };
    }
    // ?app-update: a new version of the app on offer.
    if (q.has("app-update")) {
      snap.app_update = {
        ...snap.app_update,
        available: {
          version: "0.1.2",
          notes: "## What's Changed\n* Install the mythics.gg addon and keep it up to date by @Bysonuk in https://github.com/Bysonuk/mythics-logger/pull/9",
          date: "2026-10-02T09:00:00Z",
        },
        offer: true,
      };
    }
    return fakeBridge(snap);
  }
  return tauriBridge();
}

const host = document.getElementById("app");
if (host) {
  void bridge().then((b) => {
    const tab = new URLSearchParams(location.search).get("tab");
    const initial = tab === "backlog" || tab === "history" || tab === "settings" ? tab : "live";
    render(<App bridge={b} initialTab={initial} />, host);
  });
}
