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
  });
});
