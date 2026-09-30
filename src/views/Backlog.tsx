import { useEffect, useState } from "preact/hooks";
import type { Bridge } from "../bridge";
import { Card, Notice, VisibilitySelect } from "../components";
import { fileContents, formatBytes, formatCount, formatDate, VISIBILITY_LABEL } from "../format";
import type { BacklogFile, BacklogPulls, Snapshot, Visibility } from "../types";

/** "…\Logs": the folder's last part, so the path fits and still says where. */
export function shortFolder(path: string | null): string {
  if (!path) return "your Logs folder";
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts.length > 1 ? `…\\${parts.at(-1)}` : path;
}

function selectable(f: BacklogFile): boolean {
  return f.analysed && !f.live && f.segments - f.already - f.queued > 0;
}

/** The Backlog setting, in the player's words. */
const PULLS: { value: BacklogPulls; label: string; hint: string }[] = [
  {
    value: "kills_and_best_wipe",
    label: "Kills and each boss's best wipe",
    hint: "Every kill, and the wipe that got each boss lowest, with all their detail. Other wipes send only their result (when, how long, the boss's health), so your pull count and best % on mythics.gg stay right. Much smaller.",
  },
  {
    value: "all",
    label: "All pulls",
    hint: "Every pull with all its detail: the biggest upload.",
  },
];

/** What the files would upload with the setting: "about 640 MB". */
export function estimate(files: BacklogFile[], pulls: BacklogPulls): number {
  return files.reduce((n, f) => n + (pulls === "all" ? f.estimate_all : f.estimate_best), 0);
}

export function Backlog({
  snap,
  bridge,
  onError,
  announce,
}: {
  snap: Snapshot;
  bridge: Bridge;
  onError: (code: string) => void;
  announce: (msg: string) => void;
}) {
  const b = snap.backlog;
  const [chosen, setChosen] = useState<Set<string>>(new Set());
  const [visibility, setVisibility] = useState<Visibility>(snap.settings.default_visibility);
  const [asked, setAsked] = useState(false);

  // Look for past logs the first time the tab opens.
  useEffect(() => {
    if (!asked && !b.scanning && b.files.length === 0 && snap.settings.logs_dir) {
      setAsked(true);
      bridge.backlogScan().catch((e: unknown) => onError(String(e)));
    }
  }, [asked, b.scanning, b.files.length, snap.settings.logs_dir]);

  const offered = b.files.filter(selectable);
  // The file being logged live isn't a past log.
  const past = b.files.filter((f) => !f.live);
  const toggle = (path: string) => {
    const next = new Set(chosen);
    if (next.has(path)) next.delete(path);
    else next.add(path);
    setChosen(next);
  };
  const upload = async (paths: string[]) => {
    try {
      const n = await bridge.backlogUpload(paths, visibility);
      setChosen(new Set());
      announce(
        n === 0
          ? "Nothing new to upload: those pulls are already on mythics.gg or waiting."
          : `${formatCount(n, "pull or key", "pulls and keys")} added to the upload list, as ${VISIBILITY_LABEL[visibility]}.`,
      );
    } catch (e) {
      onError(String(e));
    }
  };
  const chosenFiles = offered.filter((f) => chosen.has(f.path));
  const chosenPaths = chosenFiles.map((f) => f.path);
  const pulls = snap.settings.backlog_pulls;
  const setPulls = (v: BacklogPulls) => {
    bridge
      .saveSettings({ backlog_pulls: v })
      .then(() => undefined)
      .catch((e: unknown) => onError(String(e)));
  };
  const toSend = estimate(chosenFiles.length > 0 ? chosenFiles : offered, pulls);
  const summaries = (chosenFiles.length > 0 ? chosenFiles : offered).reduce((n, f) => n + f.summaries, 0);

  return (
    <div class="stack">
      <Notice tone="info">
        Past logs are packed as small as they go and uploaded in the background, at low priority. They never count as live, and they don't slow the game down.
      </Notice>

      <Card>
        <h2 class="card-title">Past logs</h2>
        {b.scanning ? (
          <div class="scan" role="status">
            <p>
              Reading {b.files_done + 1 > b.files_total ? b.files_total : b.files_done + 1} of {formatCount(b.files_total, "log")}
              {b.current_name ? (
                <>
                  : <span class="mono">{b.current_name}</span>
                </>
              ) : null}
            </p>
            <progress max={100} value={b.current_pct ?? 0} aria-label="Reading past logs" />
            <button type="button" class="button button-quiet" onClick={() => void bridge.backlogCancel()}>
              Stop reading
            </button>
          </div>
        ) : (
          <p>
            Found {formatCount(past.length, "older log")} in {shortFolder(snap.settings.logs_dir)} (
            {formatBytes(past.reduce((n, f) => n + f.size, 0))})
          </p>
        )}
        <fieldset class="radios">
          <legend>Pulls to upload</legend>
          {PULLS.map((p) => (
            <label key={p.value} class="radio">
              <input type="radio" name="backlog-pulls" value={p.value} checked={pulls === p.value} onChange={() => setPulls(p.value)} />
              <span>
                <span class="radio-label">{p.label}</span>
                <span class="radio-hint">{p.hint}</span>
              </span>
            </label>
          ))}
        </fieldset>
        {offered.length > 0 && !b.scanning ? (
          <p class="estimate" role="status">
            {chosenFiles.length > 0 ? `${formatCount(chosenFiles.length, "chosen log")}: ` : "All: "}
            about {formatBytes(toSend)} to upload
            {pulls === "kills_and_best_wipe" && summaries > 0 ? `, with ${formatCount(summaries, "wipe")} sent as a result only` : ""}.
          </p>
        ) : null}
        <div class="actions">
          <button
            type="button"
            class="button"
            disabled={b.scanning}
            onClick={() =>
              bridge
                .backlogChooseFiles()
                .then(() => undefined)
                .catch((e: unknown) => onError(String(e)))
            }
          >
            Choose files…
          </button>
          <VisibilitySelect id="backlog-visibility" label="Visibility" value={visibility} onChange={setVisibility} />
          <button
            type="button"
            class="button button-primary"
            disabled={b.scanning || offered.length === 0}
            onClick={() => void upload(offered.map((f) => f.path))}
          >
            Upload all
          </button>
          {chosenPaths.length > 0 ? (
            <button type="button" class="button button-primary" onClick={() => void upload(chosenPaths)}>
              Upload {formatCount(chosenPaths.length, "chosen log")}
            </button>
          ) : null}
        </div>
        <p class="meta">
          Pulls already on mythics.gg are skipped, so uploading a log twice sends nothing new.
          {snap.settings.upload_limit_kbps > 0
            ? ` Upload speed is limited to ${formatBytes(snap.settings.upload_limit_kbps * 1000)} a second (Settings).`
            : ""}
        </p>
      </Card>

      {b.total > 0 ? (
        <Card title="Uploading past logs">
          <p>
            {b.done.toLocaleString("en-GB")} of {formatCount(b.total, "pull")} uploaded
            {b.paused ? " · paused" : ""}
          </p>
          <progress max={b.total} value={b.done} aria-label="Past logs uploaded" />
          {b.failed > 0 ? <p class="meta">{formatCount(b.failed, "pull")} couldn't be sent.</p> : null}
          {b.done < b.total ? (
            <button type="button" class="button" onClick={() => void bridge.backlogPause(!b.paused)}>
              {b.paused ? "Resume" : "Pause"}
            </button>
          ) : null}
        </Card>
      ) : null}

      {b.files.length > 0 ? (
        <Card title="Logs found">
          <ul class="files">
            {b.files.map((f) => (
              <FileRow key={f.path} file={f} checked={chosen.has(f.path)} onToggle={() => toggle(f.path)} />
            ))}
          </ul>
        </Card>
      ) : null}
    </div>
  );
}

function FileRow({ file: f, checked, onToggle }: { file: BacklogFile; checked: boolean; onToggle: () => void }) {
  const id = `file-${f.path.replace(/[^a-zA-Z0-9]/g, "-")}`;
  const canPick = selectable(f);
  const date = formatDate(f.first_time ?? new Date(f.modified_ms).toISOString());
  const left = f.segments - f.already - f.queued;
  let state: string;
  if (f.live) state = "Being logged live now";
  else if (!f.analysed) state = "Not read yet";
  else if (f.segments === 0) state = "Nothing to upload";
  else if (left === 0 && f.queued > 0) state = `${formatCount(f.queued, "pull")} waiting to upload`;
  else if (left === 0) state = "All uploaded";
  else if (f.already > 0) state = `${f.already.toLocaleString("en-GB")} of ${formatCount(f.segments, "pull")} already uploaded (skipped)`;
  else state = `${formatCount(f.segments, "pull")} to upload`;
  return (
    <li class={`file ${canPick ? "" : "file-off"}`}>
      <input type="checkbox" id={id} checked={checked} disabled={!canPick} onChange={onToggle} />
      <label for={id} class="file-main">
        <span class="file-date">{date}</span>
        <span class="file-name mono">
          {f.folder ? `${f.folder}\\` : ""}
          {f.name}
        </span>
        <span class="meta">
          {formatBytes(f.size)}
          {f.live ? "" : ` · ${fileContents(f)}`}
          {f.advanced === false ? " · Advanced logging was off" : ""}
          {f.analysed && f.version == null ? " · Older log: times are approximate" : ""}
        </span>
      </label>
      <span class="file-state">{state}</span>
    </li>
  );
}
