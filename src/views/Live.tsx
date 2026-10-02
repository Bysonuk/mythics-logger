import { useState } from "preact/hooks";
import type { Bridge } from "../bridge";
import { Card, Notice } from "../components";
import {
  difficultyName,
  errorText,
  formatBytes,
  formatCount,
  formatTime,
  pullResult,
  pullStatus,
  pullTitle,
} from "../format";
import { groupLogs, LogGroups, PageLinks } from "../logs";
import type { LiveShareView, Pull, Snapshot } from "../types";
import { LIVE_WHY } from "./LivePrompt";

/** A log counts as live while the game wrote to it in the last 10 minutes. */
const LIVE_WINDOW_MS = 10 * 60 * 1000;

export function Live({
  snap,
  bridge,
  now,
  onError,
  announce,
}: {
  snap: Snapshot;
  bridge: Bridge;
  now: number;
  onError: (code: string) => void;
  announce: (msg: string) => void;
}) {
  const live = snap.live;
  const open = (path: string) => {
    bridge.openLog(path).catch((e: unknown) => onError(String(e)));
  };
  const active = live.status === "live" && live.last_activity_ms > 0 && now - live.last_activity_ms < LIVE_WINDOW_MS;
  return (
    <div class="stack">
      <Card class="status-card">
        {!snap.settings.live_logging ? (
          <>
            <p class="status-line">
              <span class="dot" aria-hidden="true" /> <strong>Live logging is off</strong>
            </p>
            <p>The app isn't following your combat log or uploading pulls as you play. You can still upload past logs from Backlog.</p>
            <p class="meta">{LIVE_WHY}</p>
            <button
              type="button"
              class="button button-primary"
              onClick={() =>
                bridge
                  .saveSettings({ live_logging: true })
                  .then(() => announce("Live logging is on. Pulls from now on upload as they end."))
                  .catch((e: unknown) => onError(String(e)))
              }
            >
              Turn on live logging
            </button>
          </>
        ) : live.status === "searching" ? (
          <>
            <p class="status-line">
              <span class="dot dot-idle" aria-hidden="true" /> <strong>Looking for World of Warcraft</strong>
            </p>
            <p>If it's installed somewhere unusual, choose its folder.</p>
            <button
              type="button"
              class="button"
              onClick={() => bridge.chooseLogsFolder().catch((e: unknown) => onError(String(e)))}
            >
              Choose folder…
            </button>
          </>
        ) : (
          <>
            <p class="status-line">
              <span class={`dot ${active ? "dot-live" : "dot-idle"}`} aria-hidden="true" />{" "}
              <strong>{active ? "Live" : live.file ? "Watching" : "Waiting for a combat log"}</strong>
              {active && live.zone ? (
                <span class="status-zone">
                  {live.zone}
                  {difficultyName(live.difficulty) ? ` · ${difficultyName(live.difficulty)}` : ""}
                </span>
              ) : null}
            </p>
            {live.file ? (
              <p class="meta">
                Watching <span class="mono">{live.file}</span>
              </p>
            ) : (
              <p>
                Type <span class="mono">/combatlog</span> in the game, or use a logging addon. The app picks the log up by itself.
              </p>
            )}
            {live.advanced === true ? <p class="good">Advanced logging on ✓</p> : null}
            {live.advanced === false ? (
              <Notice tone="warn">
                Advanced Combat Logging is off. Turn it on in the game under System, Network, so mythics.gg can show boss health, gear and
                talents.
              </Notice>
            ) : null}
            {live.current ? (
              <p>
                <span class="label">Current {live.current.kind === "key" ? "key" : "pull"}</span>{" "}
                {pullTitle({ ...live.current, opened_as: live.current.kind })}
                {live.current.kind !== "key" && difficultyName(live.current.difficulty) ? ` · ${difficultyName(live.current.difficulty)}` : ""}
              </p>
            ) : active ? (
              <p class="meta">No pull in progress.</p>
            ) : live.file ? (
              <p class="meta">Nothing new in the log yet. It updates when the game writes to it.</p>
            ) : null}
            <p class="meta">Uploads when the pull ends · usually 1–3 min</p>
          </>
        )}
      </Card>

      {snap.settings.live_logging && snap.live_share.status !== "off" ? (
        <LiveReport share={snap.live_share} bridge={bridge} onError={onError} announce={announce} />
      ) : null}

      {snap.counts.live_waiting > 0 ? (
        <p class="meta">{formatCount(snap.counts.live_waiting, "pull")} waiting to upload</p>
      ) : null}

      <Card title="Tonight's pulls">
        {snap.pulls.length === 0 ? (
          <p class="meta">Pulls and keys appear here as they end.</p>
        ) : (
          <LogGroups
            groups={groupLogs(snap.pulls)}
            rowKey={(p) => p.sha256}
            row={(p, where) => <PullRow pull={p} where={where} onOpen={open} />}
            onOpen={open}
          />
        )}
      </Card>

      <p class="meta footnote">
        Safe to run alongside the Warcraft Logs uploader: both only read the same file, and this app never changes it.
      </p>
    </div>
  );
}

/** One pull or key in its log: under its boss in Raid ("Pull 2"), or a key
 *  in Mythic+ ("The Blinding Vale +14"). */
function PullRow({ pull: p, where, onOpen }: { pull: Pull; where: "raid" | "mplus" | "dungeon"; onOpen: (path: string) => void }) {
  const diff = p.opened_as === "key" ? null : difficultyName(p.difficulty);
  const pct = p.state === "done" ? 100 : (p.progress_pct ?? 0);
  const title = pullTitle(p);
  // Under its boss's heading, a raid pull is its number; the boss is above.
  const label = where === "raid" && p.opened_as === "encounter" ? `${title}, pull ${p.pull_number}` : title;
  return (
    <>
      <div class="pull-main">
        <span class="pull-title">
          {where === "raid" && p.opened_as === "encounter" ? `Pull ${p.pull_number}` : title}
          {where === "dungeon" && p.opened_as === "encounter" ? <span class="pull-number"> · Pull {p.pull_number}</span> : null}
        </span>
        <span class="meta">
          {[where === "raid" ? null : diff, pullResult(p), formatBytes(p.size), formatTime(p.start_time)].filter(Boolean).join(" · ")}
        </span>
      </div>
      <div class="pull-side">
        <progress max={100} value={pct} aria-label={`${label}: ${pullStatus(p)}`} />
        <span class={`pull-status status-${p.state}`}>{pullStatus(p)}</span>
        {p.state === "failed" && p.error ? <span class="meta">{errorText(p.error)}</span> : null}
      </div>
      {p.state === "done" && (p.upload_id || p.already) ? (
        <PageLinks status={p.server_status} page={p.page_url} title={label} fights={p.fights} onOpen={onOpen} />
      ) : null}
    </>
  );
}

const VISIBILITY_NAME = { public: "Public", guild: "Guild only", private: "Private" } as const;

/** The live report's link: a page of tonight's log that anyone with the
 *  link can follow on mythics.gg as pulls upload (src-tauri's `share.rs`). */
function LiveReport({
  share,
  bridge,
  onError,
  announce,
}: {
  share: LiveShareView;
  bridge: Bridge;
  onError: (code: string) => void;
  announce: (msg: string) => void;
}) {
  const [copied, setCopied] = useState<"" | "copied" | "failed">("");
  const run = (p: Promise<void>, said?: string) =>
    p.then(() => (said ? announce(said) : undefined)).catch((e: unknown) => onError(String(e)));
  const copy = async (url: string) => {
    try {
      await navigator.clipboard.writeText(url);
      setCopied("copied");
    } catch {
      setCopied("failed");
    }
  };
  return (
    <Card title="Live report" class="live-report">
      {share.status === "waiting" || share.status === "creating" ? (
        <p class="meta" role="status">
          {share.status === "waiting" ? "Your live report link appears here once the first pull uploads." : "Making your live report link…"}
        </p>
      ) : share.status === "ready" && share.url ? (
        <>
          <div class="field">
            <label for="live-report-link">Live report link</label>
            <input
              id="live-report-link"
              class="share-link"
              type="url"
              readOnly
              value={share.url}
              onFocus={(e) => (e.currentTarget as HTMLInputElement).select()}
            />
          </div>
          <div class="actions">
            <button type="button" class="button button-primary" onClick={() => void copy(share.url ?? "")}>
              Copy link
            </button>
            <button type="button" class="button" onClick={() => void run(bridge.liveShareOpen())}>
              Open in browser
            </button>
            <button
              type="button"
              class="button"
              onClick={() => {
                setCopied("");
                void run(bridge.liveShareRevoke(), "Sharing stopped. The link no longer works.");
              }}
            >
              Stop sharing
            </button>
            <span class="meta" role="status">
              {copied === "copied" ? "Link copied" : copied === "failed" ? "Couldn't copy. Select the link and press Ctrl+C." : ""}
            </span>
          </div>
          <p class="meta">
            Anyone with the link can follow this log's Public pulls on mythics.gg as they upload, without logging in. Stop sharing turns the
            link off at once.
          </p>
        </>
      ) : share.status === "not_public" ? (
        <Notice tone="info">
          No link for this log: its pulls upload as {VISIBILITY_NAME[share.visibility ?? "private"]}, and a live report shows Public pulls
          only. To share it, set Visibility for new uploads to Public in Settings, or make a pull Public in History.
        </Notice>
      ) : share.status === "revoked" ? (
        <>
          <p>Sharing is off for this log. The old link no longer works.</p>
          <div class="actions">
            <button type="button" class="button" onClick={() => void run(bridge.liveShareNew(), "New live report link made.")}>
              Make a new link
            </button>
          </div>
        </>
      ) : share.status === "unavailable" ? (
        <p class="meta">Live report links aren't available from mythics.gg yet. They'll appear here once the site has them.</p>
      ) : share.status === "error" ? (
        <>
          <Notice tone="warn">Couldn't make your live report link. {errorText(share.error ?? "")}</Notice>
          <div class="actions">
            <button type="button" class="button" onClick={() => void run(bridge.liveShareNew(), "Live report link made.")}>
              Try again
            </button>
          </div>
        </>
      ) : null}
    </Card>
  );
}
