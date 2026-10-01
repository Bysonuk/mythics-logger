import { useCallback, useEffect, useRef, useState } from "preact/hooks";
import type { Bridge } from "./bridge";
import { Account, Notice, TABS, Tabs, Wordmark, type TabId } from "./components";
import { errorText } from "./format";
import type { Snapshot } from "./types";
import { Backlog } from "./views/Backlog";
import { AddonPrompt } from "./views/AddonPrompt";
import { FirstRun } from "./views/FirstRun";
import { History } from "./views/History";
import { Live } from "./views/Live";
import { LivePrompt } from "./views/LivePrompt";
import { Settings } from "./views/Settings";

export function App({ bridge, initialTab = "live" }: { bridge: Bridge; initialTab?: TabId }) {
  const [snap, setSnap] = useState<Snapshot | null>(null);
  const [tab, setTab] = useState<TabId>(initialTab);
  const [error, setError] = useState<string | null>(null);
  const [message, setMessage] = useState("");
  const [now, setNow] = useState(Date.now());
  const pending = useRef(false);

  const refresh = useCallback(() => {
    // Many "changed" events in a burst (a big upload): one request at a time.
    if (pending.current) return;
    pending.current = true;
    bridge
      .getState()
      .then(setSnap)
      .catch(() => undefined)
      .finally(() => {
        pending.current = false;
      });
  }, [bridge]);

  useEffect(() => {
    refresh();
    const off = bridge.onChanged(refresh);
    // A backstop in case an event is missed, and to keep "Live" honest.
    const timer = setInterval(() => {
      setNow(Date.now());
      refresh();
    }, 5000);
    return () => {
      off();
      clearInterval(timer);
    };
  }, [bridge, refresh]);

  const onError = (code: string) => {
    setError(code);
    setMessage(errorText(code));
  };
  const announce = (msg: string) => {
    setError(null);
    setMessage(msg);
  };
  const changeTab = (t: TabId) => {
    setError(null);
    setTab(t);
  };

  if (!snap) {
    return (
      <main class="first-run" id="main">
        <p role="status">Starting…</p>
      </main>
    );
  }

  if (!snap.signed_in) {
    return (
      <>
        <FirstRun snap={snap} bridge={bridge} error={error} onError={onError} />
        <div class="visually-hidden" role="status" aria-live="polite">
          {message}
        </div>
      </>
    );
  }

  if (!snap.settings.live_asked) {
    return (
      <>
        <LivePrompt bridge={bridge} error={error} onError={onError} />
        <div class="visually-hidden" role="status" aria-live="polite">
          {message}
        </div>
      </>
    );
  }

  // Asked once, as soon as the app knows where the game is.
  if (!snap.settings.addon_asked && snap.addon.game_found) {
    return (
      <>
        <AddonPrompt snap={snap} bridge={bridge} error={error} onError={onError} />
        <div class="visually-hidden" role="status" aria-live="polite">
          {message}
        </div>
      </>
    );
  }

  return (
    <div class="app">
      <header class="top">
        <Wordmark />
        <Tabs current={tab} onChange={changeTab} />
        {snap.settings.live_logging ? null : (
          <span class="live-off">
            <span class="dot" aria-hidden="true" /> Live logging off
          </span>
        )}
        <Account main={snap.main} />
      </header>
      <main class="content" id="main">
        {TABS.map((t) => (
          <section
            key={t.id}
            id={`panel-${t.id}`}
            role="tabpanel"
            aria-labelledby={`tab-${t.id}`}
            hidden={t.id !== tab}
            tabIndex={0}
            class="panel"
          >
            {t.id !== tab ? null : (
              <>
                <h1 class="visually-hidden">{t.label}</h1>
                {error ? <Notice tone="bad">{errorText(error)}</Notice> : null}
                {t.id === "live" ? <Live snap={snap} bridge={bridge} now={now} onError={onError} announce={announce} /> : null}
                {t.id === "backlog" ? <Backlog snap={snap} bridge={bridge} onError={onError} announce={announce} /> : null}
                {t.id === "history" ? <History bridge={bridge} announce={announce} /> : null}
                {t.id === "settings" ? <Settings snap={snap} bridge={bridge} onError={onError} announce={announce} /> : null}
              </>
            )}
          </section>
        ))}
      </main>
      <div class="visually-hidden" role="status" aria-live="polite">
        {message}
      </div>
    </div>
  );
}
