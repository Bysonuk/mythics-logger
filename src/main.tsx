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
