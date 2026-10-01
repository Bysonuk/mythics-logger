import type { Bridge } from "../bridge";
import { Card, Notice } from "../components";
import { errorText, formatAgo } from "../format";
import type { AddonAction, AddonView, SettingsPatch, Snapshot } from "../types";
import { ADDON_WHY } from "./AddonPrompt";

/** The button for what Install would do; none when there's nothing to do. */
export function addonButton(a: AddonView): string | null {
  if (!a.present) return "Install";
  const byAction: Partial<Record<AddonAction, string>> = {
    update: "Update",
    repair: "Repair",
    unbuilt: "Install",
  };
  return a.action ? (byAction[a.action] ?? null) : null;
}

function latestText(a: AddonView): string {
  if (a.availability === "unavailable") return "Not available from mythics.gg yet";
  return a.latest ?? "Not checked yet";
}

function installedText(a: AddonView): string {
  if (!a.present) return "Not installed";
  if (a.action === "repair") return a.installed ? `${a.installed}, some files missing` : "Some files missing";
  return a.installed ?? "Unknown version";
}

/** Settings > In-game addon: "Keep the addon up to date", the installed and
 *  latest versions, Check now, and Install or Update. */
export function AddonSettings({
  snap,
  bridge,
  save,
  onError,
  now = Date.now(),
}: {
  snap: Snapshot;
  bridge: Bridge;
  save: (patch: SettingsPatch, said?: string) => Promise<void>;
  onError: (code: string) => void;
  now?: number;
}) {
  const a = snap.addon;
  const busy = a.status === "checking" || a.status === "installing";
  const action = addonButton(a);
  return (
    <Card title="In-game addon">
      <label class="check">
        <input
          type="checkbox"
          role="switch"
          checked={snap.settings.addon_auto_update}
          onChange={(e) => {
            const on = (e.currentTarget as HTMLInputElement).checked;
            void save({ addon_auto_update: on }, on ? "The addon will be kept up to date." : "The addon won't be updated by itself.");
          }}
        />
        <span>
          Keep the addon up to date
          <span class="radio-hint">
            Installs each new version from mythics.gg into Interface\AddOns when the game isn't running. Never other addons or your saved
            settings.
          </span>
        </span>
      </label>
      <p class="meta">{ADDON_WHY}</p>

      {a.game_found ? (
        <dl class="addon-versions">
          <dt>Installed</dt>
          <dd>{installedText(a)}</dd>
          <dt>Latest</dt>
          <dd>{latestText(a)}</dd>
        </dl>
      ) : (
        <Notice tone="warn">The app hasn't found World of Warcraft's _retail_ folder yet. Choose your World of Warcraft folder below.</Notice>
      )}

      {a.status === "waiting_for_game" ? <Notice tone="info">An addon update is ready; it installs when you close the game.</Notice> : null}
      {a.action === "newer" ? <Notice tone="info">You have a newer version than mythics.gg's, so the app leaves it alone.</Notice> : null}
      {a.action === "linked" ? <Notice tone="info">{errorText("addon_linked")}</Notice> : null}
      {a.error ? <Notice tone="bad">{errorText(a.error)}</Notice> : null}

      <p class="actions">
        <button type="button" class="button button-quiet" disabled={busy} onClick={() => void bridge.addonCheck().catch((e: unknown) => onError(String(e)))}>
          Check now
        </button>
        {action && a.game_found ? (
          <button type="button" class="button" disabled={busy} onClick={() => void bridge.addonInstall().catch((e: unknown) => onError(String(e)))}>
            {action}
          </button>
        ) : null}
        <span class="meta" role="status">
          {a.status === "checking"
            ? "Checking…"
            : a.status === "installing"
              ? "Installing…"
              : a.checked_ms
                ? `Checked ${formatAgo(a.checked_ms, now)}`
                : ""}
        </span>
      </p>
    </Card>
  );
}
