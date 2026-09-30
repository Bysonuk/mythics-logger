// One combat log file, as mythics.gg shows it: its Raid bosses (each with
// its pulls) and its Mythic+ keys (docs/specs/logger-api.md, "Sessions").
// The Live and History tabs both group their rows this way; each draws its
// own rows.

import type { ComponentChildren } from "preact";
import { difficultyName, fightText, formatDate } from "./format";
import type { Fight, LogPlace, ServerStatus } from "./types";

/** What grouping needs of a row: its log, and what it is. */
export interface LogEntry extends LogPlace {
  /** A raid member's copy: its log is theirs, so not this log's link. */
  already?: boolean;
  kind: string | null;
  opened_as?: string | null;
  name: string | null;
  difficulty: number | null;
  success: boolean | null;
  start_time: string | null;
}

export interface BossGroup<T> {
  key: string;
  name: string;
  difficulty: number | null;
  boss_url: string | null;
  killed: boolean;
  pulls: T[];
}

export interface LogGroup<T> {
  group: string;
  title: string;
  log_url: string | null;
  raid: BossGroup<T>[];
  mplus: T[];
  dungeon: T[];
}

/** The game's raid difficulty ids (the server's list: mt10/logs/sessions.py). */
const RAID = new Set([3, 4, 5, 6, 7, 9, 14, 15, 16, 17, 33, 151, 220]);

export function sectionOf(e: LogEntry): "raid" | "mplus" | "dungeon" {
  if (e.section === "raid" || e.section === "mplus" || e.section === "dungeon") return e.section;
  if ((e.opened_as ?? e.kind) === "key") return "mplus";
  if (e.difficulty == null || RAID.has(e.difficulty)) return "raid";
  return "dungeon";
}

/** Rows into logs, in the order the rows come; within a log, raid bosses
 *  in the order first met, each with its pulls. */
export function groupLogs<T extends LogEntry>(rows: T[]): LogGroup<T>[] {
  const logs = new Map<string, LogGroup<T> & { first: string | null }>();
  for (const r of rows) {
    let g = logs.get(r.group);
    if (!g) {
      g = { group: r.group, title: "", log_url: null, raid: [], mplus: [], dungeon: [], first: null };
      logs.set(r.group, g);
    }
    if (!r.already) g.log_url = g.log_url ?? r.log_url;
    if (r.file_name && !g.title) g.title = r.file_name;
    if (r.start_time && (!g.first || r.start_time < g.first)) g.first = r.start_time;
    const where = sectionOf(r);
    if (where === "mplus") g.mplus.push(r);
    else if (where === "dungeon") g.dungeon.push(r);
    else {
      const key = r.encounter_id != null ? `e${r.encounter_id}` : `n${r.name ?? ""}`;
      let boss = g.raid.find((b) => b.key === key);
      if (!boss) {
        boss = { key, name: r.name ?? "Boss", difficulty: r.difficulty, boss_url: null, killed: false, pulls: [] };
        g.raid.push(boss);
      }
      boss.boss_url = boss.boss_url ?? r.boss_url;
      boss.killed = boss.killed || r.success === true;
      boss.pulls.push(r);
    }
  }
  return [...logs.values()].map(({ first, ...g }) => ({ ...g, title: g.title || `Log of ${formatDate(first)}` }));
}

/**
 * One log: its title and "View log", then Raid (a heading per boss, linked
 * to the boss's page, over its pulls) and Mythic+ (its keys). The rows are
 * the caller's.
 */
export function LogGroups<T extends LogEntry>({
  groups,
  row,
  rowKey,
  onOpen,
}: {
  groups: LogGroup<T>[];
  row: (r: T, where: "raid" | "mplus" | "dungeon") => ComponentChildren;
  rowKey: (r: T) => string;
  onOpen: (path: string) => void;
}) {
  return (
    <div class="logs">
      {groups.map((g) => {
        const id = `log-${g.group.replace(/[^A-Za-z0-9_-]/g, "")}`;
        return (
          <section key={g.group} class="log" aria-labelledby={id}>
            <div class="log-head">
              <h3 id={id} class="log-title">
                {g.title}
              </h3>
              {g.log_url ? (
                <button type="button" class="link" aria-label={`View log: ${g.title}, on mythics.gg`} onClick={() => onOpen(g.log_url ?? "")}>
                  View log
                </button>
              ) : null}
            </div>
            {g.raid.length > 0 ? (
              <>
                <h4 class="log-section">Raid</h4>
                <ul class="bosses">
                  {g.raid.map((b) => {
                    const diff = difficultyName(b.difficulty);
                    return (
                      <li key={b.key} class={b.killed ? "boss boss-killed" : "boss"}>
                        <p class="boss-head">
                          {b.boss_url ? (
                            <button
                              type="button"
                              class="link boss-link"
                              aria-label={`${b.name}${diff ? `, ${diff}` : ""}: every pull, on mythics.gg`}
                              onClick={() => onOpen(b.boss_url ?? "")}
                            >
                              {b.name}
                            </button>
                          ) : (
                            <span class="boss-name">{b.name}</span>
                          )}
                          <span class="meta">
                            {[diff, b.killed ? "Killed" : null, `${b.pulls.length} ${b.pulls.length === 1 ? "pull" : "pulls"}`]
                              .filter(Boolean)
                              .join(" · ")}
                          </span>
                        </p>
                        <ul class="pulls">
                          {b.pulls.map((p) => (
                            <li key={rowKey(p)} class={p.success === true ? "pull pull-kill" : "pull"}>
                              {row(p, "raid")}
                            </li>
                          ))}
                        </ul>
                      </li>
                    );
                  })}
                </ul>
              </>
            ) : null}
            {g.mplus.length > 0 ? (
              <>
                <h4 class="log-section">Mythic+</h4>
                <ul class="pulls">
                  {g.mplus.map((k) => (
                    <li key={rowKey(k)} class={k.success === true ? "pull pull-kill" : "pull"}>
                      {row(k, "mplus")}
                    </li>
                  ))}
                </ul>
              </>
            ) : null}
            {g.dungeon.length > 0 ? (
              <>
                <h4 class="log-section">Dungeons</h4>
                <ul class="pulls">
                  {g.dungeon.map((d) => (
                    <li key={rowKey(d)} class={d.success === true ? "pull pull-kill" : "pull"}>
                      {row(d, "dungeon")}
                    </li>
                  ))}
                </ul>
              </>
            ) : null}
          </section>
        );
      })}
    </div>
  );
}

/**
 * A row's own page: "View" once the server has parsed its upload, else
 * "Processing…"; for a key, each boss inside it too.
 */
export function PageLinks({
  status,
  page,
  title,
  fights,
  onOpen,
}: {
  status: ServerStatus | null;
  page: string | null;
  title: string;
  fights: Fight[];
  onOpen: (path: string) => void;
}) {
  if (status === "failed") return <p class="meta site-links">mythics.gg couldn't read this log, so it has no page.</p>;
  if (status !== "parsed" || !page) return <p class="meta site-links site-processing">Processing…</p>;
  const inside = fights.filter((f) => f.kind === "encounter" && (f.in_key != null || f.parent_id != null));
  return (
    <div class="site-links">
      <button type="button" class="link" aria-label={`View ${title}, on mythics.gg`} onClick={() => onOpen(page)}>
        View
      </button>
      {inside.length > 0 ? (
        <ul class="fights" aria-label={`Bosses in ${title}`}>
          {inside.map((f) => {
            const what = fightText(f);
            return (
              <li key={f.id} class="fight fight-in-key">
                <span>{what}</span>
                {f.url ? (
                  <button type="button" class="link" aria-label={`View ${what}, on mythics.gg`} onClick={() => onOpen(f.url ?? "")}>
                    View
                  </button>
                ) : null}
              </li>
            );
          })}
        </ul>
      ) : null}
    </div>
  );
}
