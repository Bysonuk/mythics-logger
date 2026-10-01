// The installer's privacy and licence page (SignPath Foundation's terms: an
// app that sends data must show its privacy policy during installation, and
// say how to turn the sending off). Tauri's NSIS installer shows
// `bundle.licenseFile` as a page before installing. These checks keep that
// page, the README and the app's own setting names in step.
import { describe, expect, it } from "vitest";
import page from "../src-tauri/installer/privacy-and-licence.txt?raw";
import conf from "../src-tauri/tauri.conf.json?raw";
import licence from "../LICENSE?raw";
import readme from "../README.md?raw";
import settingsView from "../src/views/Settings.tsx?raw";
import livePrompt from "../src/views/LivePrompt.tsx?raw";
import addonPrompt from "../src/views/AddonPrompt.tsx?raw";
import addonSettings from "../src/views/AddonSettings.tsx?raw";
import shell from "../src-tauri/src/lib.rs?raw";

const flat = (s: string) => s.replace(/\s+/g, " ").trim();

describe("installer privacy page", () => {
  it("is the installer's licence page", () => {
    const bundle = JSON.parse(conf).bundle;
    expect(bundle.licenseFile).toBe("installer/privacy-and-licence.txt");
    expect(bundle.publisher).toBe("mythics.gg");
  });

  it("carries the MIT licence word for word", () => {
    expect(flat(page)).toContain(flat(licence));
  });

  it("says nothing is sent until the player logs in, and links the policy", () => {
    expect(page).toContain("You choose what to upload. Nothing is sent until you log in.");
    expect(page).toContain("https://mythics.gg/privacy/");
    expect(readme).toContain("You choose what to upload. Nothing is sent until you log in.");
  });

  it("names the app's real controls", () => {
    expect(page).toContain("Settings > Live logging");
    expect(settingsView).toContain('<Card title="Live logging">');
    expect(page).toContain('"Turn live logging off"');
    expect(shell).toContain('"Turn live logging off"');
    expect(page).toContain('"Upload your pulls live while you play?"');
    expect(livePrompt).toContain("Upload your pulls live while you play?");
    expect(page).toContain("Settings > Visibility for new uploads");
    expect(settingsView).toContain('legend="Visibility for new uploads"');
    expect(page).toContain("Log out (Settings > Account)");
    expect(settingsView).toContain('<Card title="Account">');
    expect(page).toContain('Settings > Archive > "Archive logs once uploaded"');
    expect(settingsView).toContain('<Card title="Archive">');
    expect(settingsView).toContain("Archive logs once uploaded");
    expect(page).toContain('"Delete archived logs after"');
    expect(settingsView).toContain("Delete archived logs after");
    expect(page).toContain('"Install the mythics.gg addon?"');
    expect(addonPrompt).toContain('"Install the mythics.gg addon?"');
    expect(page).toContain('Settings > In-game addon > "Keep the addon up to date"');
    expect(addonSettings).toContain('<Card title="In-game addon">');
    expect(addonSettings).toContain("Keep the addon up to date");
  });

  it("says the addon is installed only if the player says yes, what it touches, and what's left after uninstalling", () => {
    // Installing the addon ends the promise that the app never changes the
    // game's files (this repository's issue 7): each place says exactly
    // what it does instead.
    for (const text of [page, readme]) {
      expect(flat(text)).toContain("The mythics.gg addon is installed only if you say yes");
      expect(flat(text)).toContain("sending nothing but those requests");
      expect(flat(text)).toMatch(/never another addon's/);
    }
    expect(flat(page)).toContain("delete its Mythics and Mythics_ folders to remove it");
    expect(flat(readme)).toContain("The app never puts back an addon you removed.");
  });

  it("says archiving moves and may delete logs, only if the player turns it on, and what's left after uninstalling", () => {
    // Archiving ends the app's old promise that it never changes or deletes
    // a combat log (this repository's issue 4): each place says exactly
    // what it does instead.
    for (const text of [page, readme]) {
      expect(flat(text)).toContain("Archiving is off until you turn it on.");
      expect(flat(text)).toContain("It changes no combat log unless you archive logs");
    }
    expect(flat(page)).toContain("Logs you archived stay in the game's Logs\\MythicsLogsArchive folder");
    expect(flat(readme)).toContain("Logs you archived stay where they are");
  });
});
