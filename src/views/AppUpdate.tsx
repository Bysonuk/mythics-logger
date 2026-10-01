import type { Bridge } from "../bridge";
import { Notice } from "../components";
import { errorText, formatAgo, formatDate, releaseNotes } from "../format";
import type { Snapshot } from "../types";

function progressText(s: Snapshot["app_update"]): string {
  if (s.status === "installing") return "Installing… The app restarts by itself.";
  if (s.status === "downloading") return s.progress_pct === null ? "Downloading…" : `Downloading… ${s.progress_pct}%`;
  return "";
}

/** "Version X is ready", above every tab, with what changed: never installed
 *  without the player saying so (the owner, 1 Oct 2026). */
export function UpdateOffer({ snap, bridge, onError }: { snap: Snapshot; bridge: Bridge; onError: (code: string) => void }) {
  const u = snap.app_update;
  if (!u.available || !(u.offer || u.status === "downloading" || u.status === "installing")) return null;
  const notes = releaseNotes(u.available.notes);
  const busy = u.status === "downloading" || u.status === "installing";
  return (
    <section class="update-offer" aria-labelledby="update-offer-title">
      <h2 id="update-offer-title" class="group-title">
        Version {u.available.version} is ready
      </h2>
      {u.available.date ? <p class="meta">Released {formatDate(u.available.date)}</p> : null}
      {notes.length > 0 ? (
        <ul class="update-notes">
          {notes.map((n, i) => (
            <li key={i}>{n}</li>
          ))}
        </ul>
      ) : null}
      {u.error === "update_install" ? <Notice tone="bad">{errorText(u.error)}</Notice> : null}
      <p class="actions">
        <button
          type="button"
          class="button button-primary"
          disabled={busy}
          onClick={() => void bridge.appUpdateInstall().catch((e: unknown) => onError(String(e)))}
        >
          Update now
        </button>
        <button type="button" class="button button-quiet" disabled={busy} onClick={() => void bridge.appUpdateLater()}>
          Later
        </button>
        <span class="meta" role="status">
          {progressText(u)}
        </span>
      </p>
    </section>
  );
}

/** The version, and Check for updates, in Settings > About and privacy. */
export function AppVersion({ snap, bridge, now = Date.now() }: { snap: Snapshot; bridge: Bridge; now?: number }) {
  const u = snap.app_update;
  const status =
    u.status === "checking"
      ? "Checking…"
      : u.available
        ? `Version ${u.available.version} is ready.`
        : u.checked_ms
          ? `Up to date (checked ${formatAgo(u.checked_ms, now)}).`
          : "";
  return (
    <>
      <p class="actions">
        <span class="meta">Version {snap.version}</span>
        <button type="button" class="button button-quiet" disabled={u.status !== "idle"} onClick={() => void bridge.appUpdateCheck()}>
          Check for updates
        </button>
        <span class="meta" role="status">
          {status}
        </span>
      </p>
      {u.error === "update_check" ? <p class="meta">{errorText(u.error)}</p> : null}
      {u.available && !u.offer && u.status === "idle" ? (
        <p class="actions">
          <button type="button" class="button" onClick={() => void bridge.appUpdateInstall()}>
            Update now
          </button>
        </p>
      ) : null}
    </>
  );
}
