import { useEffect, useRef, useState } from "preact/hooks";
import type { Bridge } from "../bridge";
import { Card, Notice, VisibilitySelect } from "../components";
import { ALREADY, difficultyName, errorText, formatBytes, formatDateTime, summaryResult } from "../format";
import { groupLogs, LogGroups, PageLinks } from "../logs";
import type { HistoryRow, UploadStatus, Visibility } from "../types";

/** How often to ask about uploads still processing: from 15 s, doubling
 *  while nothing changes, to 2 minutes. The same as the app's own poll. */
export const FIRST_POLL_MS = 15_000;
const MAX_POLL_MS = 120_000;

/** A row brought up to date from the server's newest word on its upload. */
export function refreshed(r: HistoryRow, u: UploadStatus): HistoryRow {
  const top = u.fights.find((f) => f.kind === "key") ?? u.fights[0];
  return {
    ...r,
    status: u.status,
    fights: u.fights,
    log_url: u.log_url ?? u.url ?? null,
    page_url: top?.url ?? null,
    boss_url: top?.boss_url ?? null,
    section: top?.section ?? r.section,
    encounter_id: r.encounter_id ?? top?.encounter_id ?? null,
  };
}

export function History({ bridge, announce }: { bridge: Bridge; announce: (msg: string) => void }) {
  const [rows, setRows] = useState<HistoryRow[] | null>(null);
  const [offline, setOffline] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [confirming, setConfirming] = useState<string | null>(null);

  const load = () => {
    setError(null);
    bridge
      .history()
      .then((h) => {
        setRows(h.rows);
        setOffline(h.offline);
      })
      .catch((e: unknown) => setError(String(e)));
  };
  useEffect(load, []);

  // While any upload is still being received or read, ask again now and
  // then (one request for the newest), less often the longer nothing changes.
  const processing = rows?.some((r) => r.on_server && (r.status === "receiving" || r.status === "queued")) ?? false;
  const gap = useRef(FIRST_POLL_MS);
  useEffect(() => {
    if (!processing) {
      gap.current = FIRST_POLL_MS;
      return;
    }
    const timer = setTimeout(() => {
      bridge
        .recentUploads()
        .then((fresh) => {
          const by = new Map(fresh.map((u) => [u.id, u]));
          const moved = rows?.some((r) => r.id != null && by.has(r.id) && by.get(r.id)?.status !== r.status) ?? false;
          gap.current = moved ? FIRST_POLL_MS : Math.min(gap.current * 2, MAX_POLL_MS);
          setRows(
            (rs) =>
              rs?.map((r) => {
                const u = r.id ? by.get(r.id) : undefined;
                return u ? refreshed(r, u) : r;
              }) ?? null,
          );
        })
        .catch(() => {
          gap.current = Math.min(gap.current * 2, MAX_POLL_MS);
          // Try again: a new array with the same rows re-arms this effect.
          setRows((rs) => (rs ? [...rs] : rs));
        });
    }, gap.current);
    return () => clearTimeout(timer);
  }, [processing, rows, bridge]);

  const open = (path: string) => {
    bridge.openLog(path).catch((e: unknown) => setError(String(e)));
  };

  const setVis = async (id: string, v: Visibility) => {
    try {
      await bridge.setUploadVisibility(id, v);
      setRows((rs) => rs?.map((r) => (r.id === id ? { ...r, visibility: v } : r)) ?? null);
      announce("Visibility changed.");
    } catch (e) {
      setError(String(e));
    }
  };
  const remove = async (id: string) => {
    try {
      await bridge.deleteUpload(id);
      setRows((rs) => rs?.filter((r) => r.id !== id) ?? null);
      setConfirming(null);
      announce("Deleted from mythics.gg.");
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <div class="stack">
      <Card>
        <div class="card-head">
          <h2 class="card-title">Your uploads</h2>
          <button type="button" class="button button-quiet" onClick={load}>
            Refresh
          </button>
        </div>
        {error ? <Notice tone="bad">{errorText(error)}</Notice> : null}
        {offline ? <Notice tone="warn">Couldn't reach mythics.gg, so this is what this app sent. Refresh to try again.</Notice> : null}
        {rows === null && !error ? <p role="status">Loading your uploads…</p> : null}
        {rows && rows.length === 0 ? <p class="meta">Nothing uploaded yet.</p> : null}
        {rows && rows.length > 0 ? (
          <LogGroups
            groups={groupLogs(rows)}
            rowKey={(r) => r.id ?? r.sha256 ?? ""}
            onOpen={open}
            row={(r, where) => {
              const key = r.id ?? r.sha256 ?? "";
              const what =
                r.kind === "key"
                  ? `${r.name ?? "Mythic+ key"}${r.key_level != null ? ` +${r.key_level}` : ""}`
                  : `${r.name ?? "Boss pull"}${difficultyName(r.difficulty) ? ` · ${difficultyName(r.difficulty)}` : ""}`;
              const result =
                r.kind === "key"
                  ? null
                  : r.summary
                    ? summaryResult(r.boss_hp_pct)
                    : r.success === true
                      ? "Kill"
                      : r.success === false
                        ? "Wipe"
                        : null;
              return (
                <>
                  <div class="history-main">
                    <span class="pull-title">{where === "raid" && r.kind !== "key" ? (result ?? "Pull") : what}</span>
                    <span class="meta">
                      {[
                        formatDateTime(r.start_time),
                        where === "raid" ? null : result,
                        r.size != null && !r.summary ? formatBytes(r.size) : null,
                        r.past ? "Past log" : null,
                      ]
                        .filter(Boolean)
                        .join(" · ")}
                    </span>
                  </div>
                  <div class="history-side">
                    {r.id && r.on_server ? (
                      <>
                        <VisibilitySelect
                          id={`vis-${key}`}
                          label="Visibility"
                          value={r.visibility ?? "public"}
                          onChange={(v) => void setVis(r.id ?? "", v)}
                        />
                        {confirming === r.id ? (
                          <span class="confirm" role="group" aria-label="Confirm delete">
                            <span>Delete this log from mythics.gg? This can't be undone.</span>
                            <button type="button" class="button button-danger" onClick={() => void remove(r.id ?? "")}>
                              Delete
                            </button>
                            <button type="button" class="button button-quiet" onClick={() => setConfirming(null)}>
                              Keep
                            </button>
                          </span>
                        ) : (
                          <button type="button" class="button button-quiet" onClick={() => setConfirming(r.id)}>
                            Delete…
                          </button>
                        )}
                      </>
                    ) : r.already ? (
                      <span class="meta">{ALREADY}</span>
                    ) : (
                      <span class="meta">Sent; not listed by mythics.gg yet</span>
                    )}
                  </div>
                  {(r.id && r.on_server) || r.already ? (
                    <PageLinks status={r.status} page={r.page_url} title={what} fights={r.fights} onOpen={open} />
                  ) : null}
                </>
              );
            }}
          />
        ) : null}
      </Card>
    </div>
  );
}
