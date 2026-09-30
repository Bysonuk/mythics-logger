import type { Bridge } from "../bridge";
import { Mark, Notice } from "../components";
import { errorText } from "../format";
import type { Snapshot } from "../types";

export function FirstRun({ snap, bridge, error, onError }: { snap: Snapshot; bridge: Bridge; error: string | null; onError: (c: string) => void }) {
  const logIn = () => bridge.logIn().catch((e: unknown) => onError(String(e)));
  return (
    <main class="first-run" id="main">
      <Mark size={56} />
      <h1 class="page-title">
        mythics<span class="gg">.gg</span> Logger
      </h1>
      <p class="lead">Upload your raids and Mythic+ keys to mythics.gg as you play, and your past logs too.</p>
      {snap.signed_out_notice ? <Notice tone="warn">{errorText("signed_out")}</Notice> : null}
      {error ? <Notice tone="bad">{errorText(error)}</Notice> : null}
      {snap.signing_in ? (
        <div class="stack-tight" role="status">
          <p>Waiting for you to log in in your browser…</p>
          <button type="button" class="button button-quiet" onClick={() => void bridge.cancelLogIn()}>
            Cancel
          </button>
        </div>
      ) : (
        <div class="stack-tight">
          <button type="button" class="button button-primary button-big" onClick={() => void logIn()}>
            Log in with Battle.net
          </button>
          <p class="meta">This opens your browser. Come back here once you've logged in.</p>
        </div>
      )}
      <ul class="promises">
        <li>It reads only the combat log text file World of Warcraft writes. It never touches the game itself.</li>
        <li>Safe to run alongside the Warcraft Logs uploader: both only read the same file.</li>
        <li>You choose who sees each upload: Public, Guild only or Private. You can delete any of them.</li>
      </ul>
    </main>
  );
}
