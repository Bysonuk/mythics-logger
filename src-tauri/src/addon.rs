//! The in-game addon in the app (this repository's issue 7): the first-run
//! question's answer, "Keep the addon up to date", Settings' Check now and
//! Install, and the background job. The work itself, and every safety rule,
//! is `mythics_logger_core::addon`.
//!
//! The network is asked only when the player has said yes ("Keep the addon
//! up to date" on: at start and every 6 hours), or selects Check now or
//! Install. With the setting off, nothing is fetched by itself.

use crate::state::AppState;
use crate::workers::changed;
use mythics_logger_core::addon::{self, Action, AddonError, Game, Latest, Outcome, Source, Want};
use mythics_logger_core::queue::now_ms;
use mythics_logger_core::wowdir;
use serde::Serialize;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, State};

/// How often "Keep the addon up to date" asks the site.
pub const CHECK_EVERY: Duration = Duration::from_secs(6 * 3600);
/// How often the job looks at the disk (and, while an update waits, whether
/// the game has closed). Neither touches the network.
const TICK: Duration = Duration::from_secs(30);
const TICK_WAITING: Duration = Duration::from_secs(10);
/// Looking for the game's folder when none is known yet.
const SEARCH_EVERY: Duration = Duration::from_secs(600);

/// What the app knows about the addon, kept by the job.
#[derive(Debug)]
pub struct AddonState {
    pub latest: Option<Latest>,
    /// "unknown" (not asked yet), "available", or "unavailable" (the site
    /// doesn't publish it yet).
    pub availability: &'static str,
    /// "idle", "checking", "installing" or "waiting_for_game".
    pub status: &'static str,
    /// The last attempt's error code, for the window.
    pub error: Option<&'static str>,
    pub checked_ms: Option<u64>,
    /// Read from the disk each tick.
    pub game_found: bool,
    pub installed: Option<addon::Installed>,
    /// Asked for from the window.
    pub check_now: bool,
    pub install_now: bool,
    /// An install or update waiting for the game to close.
    pub waiting: Option<Want>,
}

impl Default for AddonState {
    fn default() -> Self {
        Self {
            latest: None,
            availability: "unknown",
            status: "idle",
            error: None,
            checked_ms: None,
            game_found: false,
            installed: None,
            check_now: false,
            install_now: false,
            waiting: None,
        }
    }
}

/// The addon, for Settings and the first-run question.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct AddonView {
    /// The game's `_retail_` folder is known, so the addon can go in.
    pub game_found: bool,
    /// Any of the addon's folders is there.
    pub present: bool,
    /// The installed version, if it says one.
    pub installed: Option<String>,
    pub latest: Option<String>,
    pub availability: &'static str,
    /// What Install or Update would do, once the latest is known.
    pub action: Option<Action>,
    pub status: &'static str,
    pub error: Option<&'static str>,
    pub checked_ms: Option<u64>,
}

pub fn view(a: &AddonState) -> AddonView {
    let i = a.installed.as_ref();
    AddonView {
        game_found: a.game_found,
        present: i.is_some_and(|i| !i.present.is_empty()),
        installed: i.and_then(|i| i.version.clone()),
        latest: a.latest.as_ref().map(|l| l.version.clone()),
        availability: a.availability,
        action: match (i, &a.latest) {
            (Some(i), Some(l)) => Some(addon::action(i, l)),
            _ => None,
        },
        status: a.status,
        error: a.error,
        checked_ms: a.checked_ms,
    }
}

/// Reads the installed addon from the disk: the latest release's folders,
/// or only the core's until it's known.
fn refresh(a: &mut AddonState, game: Option<&Game>) {
    a.game_found = game.is_some();
    let folders = a
        .latest
        .as_ref()
        .map(|l| l.folders.clone())
        .unwrap_or_else(|| vec![addon::CORE.to_string()]);
    a.installed = game.map(|g| addon::installed(g, &folders));
}

/// What to do this tick, if anything: what the player asked for first, then
/// an update waiting for the game (once it's closed), then the regular
/// check while "Keep the addon up to date" is on.
pub fn next_want(
    a: &mut AddonState,
    auto: bool,
    due: bool,
    game_running: &dyn Fn() -> bool,
) -> Option<Want> {
    if std::mem::take(&mut a.install_now) {
        return Some(Want::Install);
    }
    if std::mem::take(&mut a.check_now) {
        return Some(if auto { Want::Auto } else { Want::Check });
    }
    if let Some(w) = a.waiting {
        if w == Want::Auto && !auto {
            // Turned off while it waited.
            a.waiting = None;
            a.status = "idle";
            return None;
        }
        return (!game_running()).then_some(w);
    }
    (auto && due).then_some(Want::Auto)
}

/// Keeps what one pass found.
pub fn apply(a: &mut AddonState, want: Want, r: Result<Outcome, AddonError>, now: u64) {
    a.checked_ms = Some(now);
    a.waiting = None;
    match r {
        Ok(Outcome::Checked { latest, .. }) | Ok(Outcome::Installed { latest, .. }) => {
            a.latest = Some(latest);
            a.availability = "available";
            a.error = None;
        }
        Ok(Outcome::WaitingForGame { latest, .. }) => {
            a.latest = Some(latest);
            a.availability = "available";
            a.error = None;
            a.waiting = Some(want);
        }
        Err(AddonError::NotAvailable) => {
            a.latest = None;
            a.availability = "unavailable";
            a.error = None;
        }
        Err(e) => a.error = Some(e.code()),
    }
    a.status = if a.waiting.is_some() {
        "waiting_for_game"
    } else {
        "idle"
    };
}

/// The addon's job, for the app's lifetime.
pub async fn addon_forever(app: AppHandle, state: Arc<AppState>) {
    let mut last_check: Option<Instant> = None;
    let mut last_search: Option<Instant> = None;
    let download_dir = state.data_dir.join("addon");
    loop {
        // Logged out, the app sends nothing more (the README's promise): not
        // even this check, until the player logs in again.
        let signed_in = state.token.lock().expect("token").is_some();
        let (auto, origin, logs_dir) = {
            let s = state.settings.lock().expect("settings");
            (
                s.addon_auto_update && signed_in,
                s.site_origin.clone(),
                s.logs_dir.clone(),
            )
        };
        // The first-run question waits for the game's folder: look for it,
        // as the live log does, if nobody has yet.
        let logs_dir = match logs_dir {
            Some(d) => Some(d),
            None if last_search.is_none_or(|t| t.elapsed() > SEARCH_EVERY) => {
                last_search = Some(Instant::now());
                let found = wowdir::find(None);
                if let Some(f) = &found {
                    let mut s = state.settings.lock().expect("settings");
                    if s.logs_dir.is_none() {
                        s.logs_dir = Some(f.clone());
                        let _ = s.save(&state.settings_path());
                        log::info!("found the Logs folder");
                    }
                }
                found
            }
            None => None,
        };
        let game = logs_dir.as_deref().and_then(Game::from_logs_dir);
        let due = last_check.is_none_or(|t| t.elapsed() >= CHECK_EVERY);

        let (want, before) = {
            let mut a = state.addon.lock().expect("addon");
            let before = view(&a);
            refresh(&mut a, game.as_ref());
            let want = next_want(&mut a, auto, due, &addon::game_running);
            if let Some(w) = want {
                a.status = if w == Want::Install {
                    "installing"
                } else {
                    "checking"
                };
            }
            (want, before)
        };

        if let Some(want) = want {
            changed(&app);
            let source = Source::new(&origin);
            let r = match (&game, want) {
                (Some(g), _) => {
                    addon::sync(&source, g, want, &addon::game_running, &download_dir).await
                }
                (None, Want::Install) => Err(AddonError::NoGame),
                // No game folder: only find out what's current.
                (None, _) => source.latest().await.map(|latest| Outcome::Checked {
                    latest,
                    action: Action::Install,
                }),
            };
            if want != Want::Install || r.is_ok() {
                last_check = Some(Instant::now());
            }
            let mut a = state.addon.lock().expect("addon");
            apply(&mut a, want, r, now_ms());
            refresh(&mut a, game.as_ref());
        }
        let (now, waiting) = {
            let a = state.addon.lock().expect("addon");
            (view(&a), a.waiting.is_some())
        };
        if want.is_some() || now != before {
            changed(&app);
        }
        let tick = if waiting { TICK_WAITING } else { TICK };
        let _ = tokio::time::timeout(tick, state.addon_wake.notified()).await;
    }
}

type St<'a> = State<'a, Arc<AppState>>;

/// "Check now" in Settings.
#[tauri::command]
pub fn addon_check(app: AppHandle, state: St<'_>) {
    {
        let mut a = state.addon.lock().expect("addon");
        a.check_now = true;
        a.status = "checking";
    }
    state.addon_wake.notify_one();
    changed(&app);
}

/// "Install", "Update" or "Repair" in Settings, and Yes to the first-run
/// question.
#[tauri::command]
pub fn addon_install(app: AppHandle, state: St<'_>) {
    {
        let mut a = state.addon.lock().expect("addon");
        a.install_now = true;
        a.status = "installing";
    }
    state.addon_wake.notify_one();
    changed(&app);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn latest() -> Latest {
        addon::parse_latest(
            serde_json::json!({
                "version": "2.1.0",
                "zip": "addon/Mythics-2.1.0.zip",
                "sha256": "b".repeat(64),
                "size": 100,
                "folders": ["Mythics", "Mythics_Data_EU"],
            })
            .to_string()
            .as_bytes(),
        )
        .unwrap()
    }

    #[test]
    fn nothing_is_fetched_by_itself_while_the_setting_is_off() {
        let mut a = AddonState::default();
        assert_eq!(next_want(&mut a, false, true, &|| false), None);
        assert_eq!(next_want(&mut a, true, true, &|| false), Some(Want::Auto));
        assert_eq!(next_want(&mut a, true, false, &|| false), None, "not due");
        // Check now with it off only looks.
        a.check_now = true;
        assert_eq!(
            next_want(&mut a, false, false, &|| false),
            Some(Want::Check)
        );
        a.install_now = true;
        a.check_now = true;
        assert_eq!(
            next_want(&mut a, false, false, &|| false),
            Some(Want::Install)
        );
        assert_eq!(
            next_want(&mut a, false, false, &|| false),
            Some(Want::Check)
        );
    }

    #[test]
    fn an_update_waits_for_the_game_then_goes() {
        let mut a = AddonState::default();
        apply(
            &mut a,
            Want::Auto,
            Ok(Outcome::WaitingForGame {
                latest: latest(),
                action: Action::Update,
            }),
            1,
        );
        assert_eq!(a.status, "waiting_for_game");
        assert_eq!(view(&a).status, "waiting_for_game");
        // Still running: nothing, however due.
        assert_eq!(next_want(&mut a, true, true, &|| true), None);
        // Closed: it goes at once, without waiting for the next check.
        assert_eq!(next_want(&mut a, true, false, &|| false), Some(Want::Auto));
        // Turned off while waiting: forgotten.
        assert_eq!(next_want(&mut a, false, false, &|| false), None);
        assert_eq!(a.waiting, None);
        assert_eq!(a.status, "idle");
    }

    #[test]
    fn not_published_yet_and_errors_are_kept_for_the_window() {
        let mut a = AddonState::default();
        apply(&mut a, Want::Auto, Err(AddonError::NotAvailable), 5);
        let v = view(&a);
        assert_eq!(v.availability, "unavailable");
        assert_eq!(v.error, None);
        assert_eq!(v.checked_ms, Some(5));
        apply(&mut a, Want::Install, Err(AddonError::Checksum), 6);
        assert_eq!(view(&a).error, Some("addon_checksum"));
        apply(
            &mut a,
            Want::Install,
            Ok(Outcome::Installed {
                latest: latest(),
                action: Action::Install,
            }),
            7,
        );
        let v = view(&a);
        assert_eq!(v.error, None);
        assert_eq!(v.latest.as_deref(), Some("2.1.0"));
        assert_eq!(v.availability, "available");
    }

    #[test]
    fn the_view_reads_the_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let logs = tmp.path().join("World of Warcraft/_retail_/Logs");
        std::fs::create_dir_all(&logs).unwrap();
        let game = Game::from_logs_dir(&logs).unwrap();
        let mut a = AddonState::default();
        refresh(&mut a, Some(&game));
        let v = view(&a);
        assert!(v.game_found);
        assert!(!v.present);
        assert_eq!(v.action, None, "the latest isn't known yet");
        a.latest = Some(latest());
        let core = game.addons().join("Mythics");
        std::fs::create_dir_all(&core).unwrap();
        std::fs::write(core.join("Mythics.toc"), "## Version: 2.0.0\n").unwrap();
        refresh(&mut a, Some(&game));
        let v = view(&a);
        assert!(v.present);
        assert_eq!(v.installed.as_deref(), Some("2.0.0"));
        assert_eq!(v.action, Some(Action::Update));
        refresh(&mut a, None);
        assert!(!view(&a).game_found);
    }
}
