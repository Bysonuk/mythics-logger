import { useState } from "preact/hooks";
import type { Bridge } from "../bridge";
import { Account, Card, Notice, VisibilityRadios } from "../components";
import { formatBytes, formatCount } from "../format";
import type { SettingsPatch, Snapshot } from "../types";
import { LIVE_WHY } from "./LivePrompt";

/** "Delete archived logs after": never (0), or after so many days. */
export const ARCHIVE_DAYS: { days: number; label: string }[] = [
  { days: 0, label: "Never" },
  { days: 30, label: "30 days" },
  { days: 60, label: "60 days" },
  { days: 90, label: "90 days" },
];

const SPEEDS: { kbps: number; label: string }[] = [
  { kbps: 0, label: "No limit" },
  { kbps: 5000, label: "5 MB a second" },
  { kbps: 2000, label: "2 MB a second" },
  { kbps: 1000, label: "1 MB a second" },
  { kbps: 500, label: "500 KB a second" },
];

export function Settings({
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
  const s = snap.settings;
  const [origin, setOrigin] = useState(s.site_origin);
  const save = async (patch: SettingsPatch, said = "Saved.") => {
    try {
      await bridge.saveSettings(patch);
      announce(said);
    } catch (e) {
      onError(String(e));
    }
  };

  return (
    <div class="stack">
      <Card title="Account">
        <p>
          <Account main={snap.main} />
        </p>
        <button type="button" class="button" onClick={() => void bridge.logOut()}>
          Log out
        </button>
      </Card>

      <Card title="Live logging">
        <label class="check">
          <input
            type="checkbox"
            role="switch"
            checked={s.live_logging}
            onChange={(e) => {
              const on = (e.currentTarget as HTMLInputElement).checked;
              void save({ live_logging: on }, on ? "Live logging is on." : "Live logging is off.");
            }}
          />
          <span>
            Live logging
            <span class="radio-hint">Upload pulls automatically while you play</span>
          </span>
        </label>
        <p class="meta">
          {LIVE_WHY} Off, the app doesn't follow your combat log as you play; past logs upload only when you choose them in Backlog.
        </p>
      </Card>

      <Card title="Uploads">
        <VisibilityRadios
          name="default-visibility"
          legend="Visibility for new uploads"
          value={s.default_visibility}
          onChange={(v) => void save({ default_visibility: v })}
        />
        <p class="meta">You can change any upload's visibility later, in History, or delete it.</p>

        <div class="field">
          <label for="speed">Upload speed limit</label>
          <select
            id="speed"
            value={String(s.upload_limit_kbps)}
            onChange={(e) => void save({ upload_limit_kbps: Number((e.currentTarget as HTMLSelectElement).value) })}
          >
            {SPEEDS.map((o) => (
              <option key={o.kbps} value={String(o.kbps)}>
                {o.label}
              </option>
            ))}
          </select>
        </div>

        <div class="field">
          <label for="region">Region</label>
          <select id="region" value={s.region} onChange={(e) => void save({ region: (e.currentTarget as HTMLSelectElement).value })}>
            <option value="eu">Europe</option>
            <option value="us">Americas</option>
          </select>
        </div>

        <label class="check">
          <input type="checkbox" checked={s.only_my_guild} onChange={(e) => void save({ only_my_guild: (e.currentTarget as HTMLInputElement).checked })} />
          <span>
            Only upload my guild's raids and keys
            <span class="radio-hint">Coming soon: your choice is saved, and applies once mythics.gg can tell which raids and keys are your guild's.</span>
          </span>
        </label>
      </Card>

      <Card title="Archive">
        <label class="check">
          <input
            type="checkbox"
            checked={s.archive_uploaded}
            onChange={(e) => {
              const on = (e.currentTarget as HTMLInputElement).checked;
              void save({ archive_uploaded: on }, on ? "Logs will be archived once uploaded." : "Logs won't be archived by themselves.");
            }}
          />
          <span>
            Archive logs once uploaded
            <span class="radio-hint">
              Once every pull of a log is uploaded, the app moves it into Logs\MythicsLogsArchive as a .zip, about a tenth of its size.
              Never the log the game is writing, or one another program has open.
            </span>
          </span>
        </label>

        <div class="field">
          <label for="archive-days">Delete archived logs after</label>
          <select
            id="archive-days"
            aria-describedby="archive-days-hint"
            value={String(s.archive_delete_after_days)}
            onChange={(e) => void save({ archive_delete_after_days: Number((e.currentTarget as HTMLSelectElement).value) })}
          >
            {ARCHIVE_DAYS.map((o) => (
              <option key={o.days} value={String(o.days)}>
                {o.label}
              </option>
            ))}
          </select>
        </div>
        <p class="meta" id="archive-days-hint">
          Only archives this app made are deleted, never anything else in the folder. Unzip an archive to get its log back.
        </p>

        {snap.archive.exists ? (
          <p class="actions">
            <span class="archive-size">
              {formatBytes(snap.archive.size)} in {formatCount(snap.archive.files, "file")}
            </span>
            <button
              type="button"
              class="button button-quiet"
              onClick={() =>
                bridge
                  .openArchiveFolder()
                  .then(() => undefined)
                  .catch((e: unknown) => onError(String(e)))
              }
            >
              Open archive folder
            </button>
          </p>
        ) : (
          <p class="meta">Nothing archived yet. You can also archive a log yourself in Backlog.</p>
        )}
      </Card>

      <Card title="World of Warcraft folder">
        {s.logs_dir ? (
          <p class="mono path">{s.logs_dir}</p>
        ) : (
          <Notice tone="warn">The app hasn't found World of Warcraft yet.</Notice>
        )}
        <div class="actions">
          <button type="button" class="button" onClick={() => bridge.chooseLogsFolder().catch((e: unknown) => onError(String(e)))}>
            Choose folder…
          </button>
          <button
            type="button"
            class="button button-quiet"
            onClick={() =>
              bridge
                .findLogsFolder()
                .then(() => announce("Found World of Warcraft."))
                .catch((e: unknown) => onError(String(e)))
            }
          >
            Find it for me
          </button>
        </div>
      </Card>

      <Card title="Starting up">
        <label class="check">
          <input
            type="checkbox"
            checked={s.start_with_windows}
            onChange={(e) => void save({ start_with_windows: (e.currentTarget as HTMLInputElement).checked })}
          />
          <span>
            Start with Windows
            <span class="radio-hint">Starts in the system tray, ready for your raid. Closing the window keeps it there; Quit from the tray icon.</span>
          </span>
        </label>
      </Card>

      {snap.dev ? (
        <Card title="Site address (development)">
          <form
            class="actions"
            onSubmit={(e) => {
              e.preventDefault();
              void save({ site_origin: origin }, "Site address saved. Log in again for that site.");
            }}
          >
            <label for="origin" class="visually-hidden">
              Site address
            </label>
            <input id="origin" type="url" value={origin} onInput={(e) => setOrigin((e.currentTarget as HTMLInputElement).value)} />
            <button type="submit" class="button">
              Save
            </button>
          </form>
          <p class="meta">For testing against a local server, such as http://127.0.0.1:8000.</p>
        </Card>
      ) : null}

      <About snap={snap} bridge={bridge} />
    </div>
  );
}

export function About({ snap, bridge }: { snap: Snapshot; bridge: Bridge }) {
  return (
    <Card title="About and privacy">
      <h3 class="group-title">What the app reads</h3>
      <p>
        Only the combat log text files World of Warcraft writes to its Logs folder (<span class="mono">WoWCombatLog*.txt</span>). It never
        reads the game's memory or other files, never changes game settings, and never controls the game.
      </p>
      <h3 class="group-title">What it sends</h3>
      <p>
        Your boss pulls and Mythic+ keys, compressed, to mythics.gg, with the visibility you choose. Your combat log records everyone near
        you: other players' names, realms, gear and what they cast. We use it to build rankings and guild pages. Private logs are never
        shown on the site.
      </p>
      <h3 class="group-title">What it keeps on this computer</h3>
      <p>
        Your sign-in, in Windows Credential Manager. Uploads waiting to be sent. The app's own log file records counts and problems only,
        never lines from your combat log or anyone's name.
      </p>
      <h3 class="group-title">What it changes</h3>
      <p>
        Nothing, unless you archive logs: then it moves finished logs into Logs\MythicsLogsArchive as .zip files (Settings &gt; Archive, or
        Archive in Backlog), and deletes archives it made only if you choose "Delete archived logs after". It never archives the log the
        game is writing, or one another program has open.
      </p>
      <h3 class="group-title">Alongside Warcraft Logs</h3>
      <p>
        Safe to run alongside the Warcraft Logs uploader: both read the log the game is writing, and this app never changes or deletes
        it. Archiving skips any log the uploader has open.
      </p>
      <p class="actions">
        <button type="button" class="button button-quiet" onClick={() => void bridge.openSite("/privacy/")}>
          Privacy policy
        </button>
        <span class="meta">Version {snap.version}</span>
      </p>
    </Card>
  );
}
