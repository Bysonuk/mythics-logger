import type { ComponentChildren } from "preact";
import { useRef } from "preact/hooks";
import { VISIBILITY_HINT, VISIBILITY_LABEL } from "./format";
import type { Main, Visibility } from "./types";

/** The Shard M (docs/brand/logo/mythics-mark.svg), in the theme's brand colours. */
export function Mark({ size = 28 }: { size?: number }) {
  return (
    <svg class="mark" viewBox="0 0 64 64" width={size} height={size} aria-hidden="true" focusable="false">
      <path d="M6 56V8L32 36L58 8V56H48V32L32 50L16 32V56Z" fill="var(--brand-main)" />
      <path d="M6 8L32 36L16 32Z" fill="var(--brand-highlight)" />
      <path d="M58 8L48 32V56H58Z" fill="var(--brand-shadow)" />
    </svg>
  );
}

export function Wordmark() {
  return (
    <span class="wordmark">
      <Mark />
      <span class="wordmark-text">
        mythics<span class="gg">.gg</span> <span class="product">Logger</span>
      </span>
    </span>
  );
}

export type TabId = "live" | "backlog" | "history" | "settings";
export const TABS: { id: TabId; label: string }[] = [
  { id: "live", label: "Live" },
  { id: "backlog", label: "Backlog" },
  { id: "history", label: "History" },
  { id: "settings", label: "Settings" },
];

/** WAI-ARIA tabs: arrow keys, Home and End move between them, and the one
 *  with focus is shown (automatic activation). */
export function Tabs({ current, onChange }: { current: TabId; onChange: (t: TabId) => void }) {
  const refs = useRef<(HTMLButtonElement | null)[]>([]);
  const move = (to: number) => {
    const i = (to + TABS.length) % TABS.length;
    const tab = TABS[i];
    if (!tab) return;
    onChange(tab.id);
    refs.current[i]?.focus();
  };
  const onKey = (e: KeyboardEvent, i: number) => {
    const keys: Record<string, number> = { ArrowRight: i + 1, ArrowLeft: i - 1, Home: 0, End: TABS.length - 1 };
    const to = keys[e.key];
    if (to === undefined) return;
    e.preventDefault();
    move(to);
  };
  return (
    <div class="tabs" role="tablist" aria-label="Sections">
      {TABS.map((t, i) => (
        <button
          key={t.id}
          ref={(el) => {
            refs.current[i] = el;
          }}
          type="button"
          role="tab"
          id={`tab-${t.id}`}
          aria-controls={`panel-${t.id}`}
          aria-selected={t.id === current}
          tabIndex={t.id === current ? 0 : -1}
          class="tab"
          onClick={() => onChange(t.id)}
          onKeyDown={(e) => onKey(e, i)}
        >
          {t.label}
        </button>
      ))}
    </div>
  );
}

/** The signed-in player, top right: main character and guild. Never a BattleTag. */
export function Account({ main }: { main: Main | null }) {
  if (!main?.name) return <span class="account account-empty">Logged in</span>;
  return (
    <span class="account">
      <span class="account-name">
        {main.name}
        {main.realm ? <span class="account-realm"> · {main.realm}</span> : null}
      </span>
      {main.guild ? <span class="account-guild">&lt;{main.guild}&gt;</span> : null}
    </span>
  );
}

export function VisibilityRadios({
  name,
  value,
  onChange,
  legend,
}: {
  name: string;
  value: Visibility;
  onChange: (v: Visibility) => void;
  legend: string;
}) {
  return (
    <fieldset class="radios">
      <legend>{legend}</legend>
      {(Object.keys(VISIBILITY_LABEL) as Visibility[]).map((v) => (
        <label key={v} class="radio">
          <input type="radio" name={name} value={v} checked={value === v} onChange={() => onChange(v)} />
          <span>
            <span class="radio-label">{VISIBILITY_LABEL[v]}</span>
            <span class="radio-hint">{VISIBILITY_HINT[v]}</span>
          </span>
        </label>
      ))}
    </fieldset>
  );
}

export function VisibilitySelect({
  value,
  onChange,
  label,
  id,
}: {
  value: Visibility;
  onChange: (v: Visibility) => void;
  label: string;
  id: string;
}) {
  return (
    <span class="field-inline">
      <label for={id}>{label}</label>
      <select id={id} value={value} onChange={(e) => onChange((e.currentTarget as HTMLSelectElement).value as Visibility)}>
        {(Object.keys(VISIBILITY_LABEL) as Visibility[]).map((v) => (
          <option key={v} value={v}>
            {VISIBILITY_LABEL[v]}
          </option>
        ))}
      </select>
    </span>
  );
}

export function Card({ title, children, class: cls }: { title?: string; children: ComponentChildren; class?: string }) {
  return (
    <section class={`card ${cls ?? ""}`}>
      {title ? <h2 class="card-title">{title}</h2> : null}
      {children}
    </section>
  );
}

export function Notice({ tone, children }: { tone: "warn" | "bad" | "good" | "info"; children: ComponentChildren }) {
  return <p class={`notice notice-${tone}`}>{children}</p>;
}
