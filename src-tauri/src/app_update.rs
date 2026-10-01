//! The app's own updates, through Tauri's updater plugin (the owner's
//! decision, 1 Oct 2026). It asks GitHub for the newest *published* release's
//! `latest.json` (`plugins.updater.endpoints` in `tauri.conf.json`; a draft
//! is never "latest", so never offered), at start and every 6 hours, whether
//! or not the player is logged in: it's a GET to github.com with no player
//! data. A new version is never installed silently: the window shows
//! "Version X is ready" with the release notes, and "Update now" downloads
//! it, has the plugin check its signature against the public key in
//! `tauri.conf.json`, runs the same NSIS installer a fresh download would,
//! and the app restarts. "Later" hides the offer until the app next starts.

use crate::state::AppState;
use crate::workers::changed;
use mythics_logger_core::queue::now_ms;
use serde::Serialize;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, State};
use tauri_plugin_updater::{Update, UpdaterExt};

/// How often the app asks for a new version of itself.
pub const CHECK_EVERY: Duration = Duration::from_secs(6 * 3600);
/// The first check waits for start-up to settle.
const FIRST_CHECK_AFTER: Duration = Duration::from_secs(10);
const CHECK_TIMEOUT: Duration = Duration::from_secs(30);

/// A newer version, as its release describes it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Available {
    pub version: String,
    /// The release's notes: what changed since the last release.
    pub notes: Option<String>,
    /// When it was published, RFC 3339.
    pub date: Option<String>,
}

#[derive(Default)]
pub struct AppUpdateState {
    pub available: Option<Available>,
    /// The plugin's handle for it, to download and install.
    pub update: Option<Update>,
    /// "idle", "checking", "downloading" or "installing" ("" is idle).
    pub status: &'static str,
    /// Bytes downloaded, and of how many if the server said.
    pub progress: Option<(u64, Option<u64>)>,
    pub error: Option<&'static str>,
    /// The version the player said Later to, this run.
    pub later: Option<String>,
    pub checked_ms: Option<u64>,
    pub check_now: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AppUpdateView {
    pub available: Option<Available>,
    /// The offer is shown: there's a new version and the player hasn't said
    /// Later to it.
    pub offer: bool,
    pub status: &'static str,
    pub progress_pct: Option<u32>,
    pub error: Option<&'static str>,
    pub checked_ms: Option<u64>,
}

pub fn view(a: &AppUpdateState) -> AppUpdateView {
    AppUpdateView {
        offer: a
            .available
            .as_ref()
            .is_some_and(|v| a.later.as_deref() != Some(v.version.as_str())),
        available: a.available.clone(),
        status: if a.status.is_empty() {
            "idle"
        } else {
            a.status
        },
        progress_pct: a.progress.and_then(|(done, of)| {
            of.filter(|&n| n > 0)
                .map(|n| (done.saturating_mul(100) / n).min(100) as u32)
        }),
        error: a.error,
        checked_ms: a.checked_ms,
    }
}

fn busy(a: &AppUpdateState) -> bool {
    matches!(a.status, "checking" | "downloading" | "installing")
}

/// Keeps what a check found.
fn found(a: &mut AppUpdateState, r: Result<Option<Update>, ()>, now: u64) {
    a.status = "idle";
    a.checked_ms = Some(now);
    match r {
        Ok(Some(u)) => {
            a.available = Some(Available {
                version: u.version.clone(),
                notes: u.body.clone(),
                date: u
                    .raw_json
                    .get("pub_date")
                    .and_then(|d| d.as_str())
                    .map(str::to_string),
            });
            a.update = Some(u);
            a.error = None;
        }
        Ok(None) => {
            a.available = None;
            a.update = None;
            a.error = None;
        }
        Err(()) => a.error = Some("update_check"),
    }
}

async fn check(app: &AppHandle, state: &AppState) {
    {
        let mut a = state.app_update.lock().expect("app update");
        if busy(&a) {
            return;
        }
        a.status = "checking";
    }
    changed(app);
    let r = match app.updater_builder().timeout(CHECK_TIMEOUT).build() {
        Ok(u) => u.check().await.map_err(|e| {
            // The error's words can carry the URL; its kind is enough.
            log::info!("couldn't check for app updates: {}", error_kind(&e));
        }),
        Err(e) => {
            log::warn!("the updater isn't set up: {}", error_kind(&e));
            Err(())
        }
    };
    if let Ok(Some(u)) = &r {
        log::info!("version {} of the app is ready", u.version);
    }
    found(
        &mut state.app_update.lock().expect("app update"),
        r,
        now_ms(),
    );
    changed(app);
}

fn error_kind(e: &tauri_plugin_updater::Error) -> &'static str {
    use tauri_plugin_updater::Error as E;
    match e {
        E::Reqwest(_) | E::Network(_) => "network",
        E::ReleaseNotFound => "no release",
        E::Minisign(_) | E::SignatureUtf8(_) => "signature",
        _ => "other",
    }
}

/// Checks at start and every 6 hours, and when the player selects Check
/// for updates. Development builds check only when asked.
pub async fn app_update_forever(app: AppHandle, state: Arc<AppState>) {
    tokio::time::sleep(FIRST_CHECK_AFTER).await;
    let mut last: Option<Instant> = None;
    loop {
        let asked = std::mem::take(&mut state.app_update.lock().expect("app update").check_now);
        let due = !cfg!(debug_assertions) && last.is_none_or(|t| t.elapsed() >= CHECK_EVERY);
        if asked || due {
            last = Some(Instant::now());
            check(&app, &state).await;
        }
        let _ =
            tokio::time::timeout(Duration::from_secs(60), state.app_update_wake.notified()).await;
    }
}

type St<'a> = State<'a, Arc<AppState>>;

/// "Check for updates" in Settings.
#[tauri::command]
pub fn app_update_check(state: St<'_>) {
    state.app_update.lock().expect("app update").check_now = true;
    state.app_update_wake.notify_one();
}

/// "Later": the offer goes until the app next starts (or a newer version).
#[tauri::command]
pub fn app_update_later(app: AppHandle, state: St<'_>) {
    {
        let mut a = state.app_update.lock().expect("app update");
        a.later = a.available.as_ref().map(|v| v.version.clone());
    }
    changed(&app);
}

/// "Update now": downloads the new version, which the plugin checks against
/// the public key before running its installer. On Windows the installer
/// takes over and the app exits, to be started again by it.
#[tauri::command]
pub async fn app_update_install(app: AppHandle, state: St<'_>) -> Result<(), String> {
    let update = {
        let mut a = state.app_update.lock().expect("app update");
        if busy(&a) {
            return Err("busy".into());
        }
        let u = a.update.clone().ok_or("update_gone")?;
        a.status = "downloading";
        a.progress = Some((0, None));
        a.error = None;
        u
    };
    changed(&app);
    log::info!("updating the app to {}", update.version);
    let (st, h) = (state.inner().clone(), app.clone());
    let mut last = Instant::now();
    let (st2, h2) = (state.inner().clone(), app.clone());
    let r = update
        .download_and_install(
            move |chunk, total| {
                let mut a = st.app_update.lock().expect("app update");
                let done = a.progress.map_or(0, |(d, _)| d) + chunk as u64;
                a.progress = Some((done, total));
                drop(a);
                if last.elapsed() > Duration::from_millis(250) {
                    last = Instant::now();
                    changed(&h);
                }
            },
            move || {
                st2.app_update.lock().expect("app update").status = "installing";
                changed(&h2);
            },
        )
        .await;
    match r {
        // Not on Windows (the installer has ended the app by now).
        Ok(()) => app.restart(),
        Err(e) => {
            log::warn!("couldn't update the app: {}", error_kind(&e));
            {
                let mut a = state.app_update.lock().expect("app update");
                a.status = "idle";
                a.progress = None;
                a.error = Some("update_install");
            }
            changed(&app);
            Err("update_install".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready(v: &str) -> AppUpdateState {
        AppUpdateState {
            available: Some(Available {
                version: v.into(),
                notes: Some("## What's Changed\n* Install the mythics.gg addon".into()),
                date: Some("2026-10-02T09:00:00Z".into()),
            }),
            ..Default::default()
        }
    }

    #[test]
    fn a_new_version_is_offered_until_the_player_says_later() {
        let mut a = ready("0.1.2");
        let v = view(&a);
        assert!(v.offer);
        assert_eq!(v.status, "idle");
        assert_eq!(v.available.unwrap().version, "0.1.2");
        a.later = Some("0.1.2".into());
        assert!(!view(&a).offer, "Later hides it");
        // A newer one still comes up.
        a.available.as_mut().unwrap().version = "0.1.3".into();
        assert!(view(&a).offer);
        assert!(!view(&AppUpdateState::default()).offer);
    }

    #[test]
    fn progress_and_errors() {
        let mut a = ready("0.1.2");
        a.status = "downloading";
        a.progress = Some((5_000_000, Some(20_000_000)));
        assert_eq!(view(&a).progress_pct, Some(25));
        a.progress = Some((5, None));
        assert_eq!(view(&a).progress_pct, None, "no size given");
        assert!(busy(&a));
        found(&mut a, Err(()), 9);
        assert_eq!(view(&a).error, Some("update_check"));
        assert_eq!(view(&a).checked_ms, Some(9));
        assert!(view(&a).offer, "a failed check keeps what it knew");
        found(&mut a, Ok(None), 10);
        assert_eq!(view(&a).available, None);
        assert_eq!(view(&a).error, None);
    }
}

#[cfg(test)]
mod config_tests {
    #[test]
    fn updates_come_from_published_github_releases_signed_for_their_version() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let u = &conf["plugins"]["updater"];
        assert_eq!(
            u["endpoints"],
            serde_json::json!([
                "https://github.com/Bysonuk/mythics-logger/releases/latest/download/latest.json"
            ])
        );
        assert_eq!(
            u["requireSignedVersion"], true,
            "no downgrade to an older signed release"
        );
        assert!(u["pubkey"].as_str().is_some_and(|k| k.len() > 100));
        // Only the release build makes the updater's signed copy: pull
        // requests build without the signing key.
        assert!(conf["bundle"].get("createUpdaterArtifacts").is_none());
        let release: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.updater.conf.json")).unwrap();
        assert_eq!(release["bundle"]["createUpdaterArtifacts"], true);
    }
}
