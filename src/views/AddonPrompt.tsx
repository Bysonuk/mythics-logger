import type { Bridge } from "../bridge";
import { Mark, Notice } from "../components";
import { errorText } from "../format";
import type { Snapshot } from "../types";

/** What the addon is, in one line: the prompt and Settings say the same. */
export const ADDON_WHY =
  "The mythics.gg addon shows the top players of each spec in game: their stats, trinkets, talents and cooldown timelines.";

/** Asked once, after the live logging question, as soon as the app knows
 *  where World of Warcraft is (the owner's decision on mythics-logger issue
 *  7). Yes installs the addon and keeps it up to date; Not now leaves both
 *  off. Either answer is kept and not asked again. */
export function AddonPrompt({ snap, bridge, error, onError }: { snap: Snapshot; bridge: Bridge; error: string | null; onError: (c: string) => void }) {
  const choose = async (yes: boolean) => {
    try {
      await bridge.saveSettings({ addon_auto_update: yes });
      if (yes) await bridge.addonInstall();
    } catch (e) {
      onError(String(e));
    }
  };
  const there = snap.addon.present;
  return (
    <main class="first-run" id="main">
      <Mark size={56} />
      <h1 class="page-title">{there ? "Keep the mythics.gg addon up to date?" : "Install the mythics.gg addon?"}</h1>
      <p class="lead">{ADDON_WHY}</p>
      <p>
        {there
          ? "Select Yes and the app installs each new version from mythics.gg into World of Warcraft's Interface\\AddOns folder, when the game isn't running."
          : "Select Yes and the app installs it from mythics.gg into World of Warcraft's Interface\\AddOns folder, and keeps it up to date when the game isn't running."}
      </p>
      {error ? <Notice tone="bad">{errorText(error)}</Notice> : null}
      <div class="actions actions-center">
        <button type="button" class="button button-primary button-big" onClick={() => void choose(true)}>
          Yes
        </button>
        <button type="button" class="button button-big" onClick={() => void choose(false)}>
          Not now
        </button>
      </div>
      <p class="meta">It never changes other addons or your saved settings. You can change this any time in Settings.</p>
    </main>
  );
}
