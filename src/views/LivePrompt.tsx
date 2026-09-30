import type { Bridge } from "../bridge";
import { Mark, Notice } from "../components";
import { errorText } from "../format";

/** Why live uploads matter, in one line: the prompt, Settings and the Live tab say the same. */
export const LIVE_WHY =
  "Live uploads are what power the Race to World First and the live pages on mythics.gg. Private and Guild only visibility still apply.";

/** Set-up's second step, once logged in: live logging stays off until the
 *  player says (the owner, 29 Sep 2026: some players won't want to stream
 *  their logs, which is fine). Either answer is kept and not asked again. */
export function LivePrompt({ bridge, error, onError }: { bridge: Bridge; error: string | null; onError: (c: string) => void }) {
  const choose = (on: boolean) => {
    bridge.saveSettings({ live_logging: on }).catch((e: unknown) => onError(String(e)));
  };
  return (
    <main class="first-run" id="main">
      <Mark size={56} />
      <h1 class="page-title">Upload your pulls live while you play?</h1>
      <p class="lead">{LIVE_WHY}</p>
      {error ? <Notice tone="bad">{errorText(error)}</Notice> : null}
      <div class="actions actions-center">
        <button type="button" class="button button-primary button-big" onClick={() => choose(true)}>
          Yes
        </button>
        <button type="button" class="button button-big" onClick={() => choose(false)}>
          Not now
        </button>
      </div>
      <p class="meta">
        You can change this any time in Settings, or from the tray icon. Past logs go only when you choose them in Backlog.
      </p>
    </main>
  );
}
