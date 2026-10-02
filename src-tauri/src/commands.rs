//! What the window can ask the app to do. Errors come back as short codes;
//! the window turns them into words (never a server's own message).

use crate::settings::{valid_origin, Settings};
use crate::state::{history_rows, snapshot, AppState, HistoryRow, Snapshot};
use crate::workers::changed;
use mythics_logger_core::api::{ApiError, UploadRow, Visibility};
use mythics_logger_core::archive::{self, Outcome};
use mythics_logger_core::auth::{AuthError, Loopback, Pkce};
use mythics_logger_core::backlog::{self, ReportCache};
use mythics_logger_core::plan::{self, BacklogPulls};
use mythics_logger_core::wowdir;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, State};
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_dialog::DialogExt as _;
use tauri_plugin_opener::OpenerExt as _;

type St<'a> = State<'a, Arc<AppState>>;

pub(crate) fn api_code(e: &ApiError) -> String {
    match e {
        ApiError::Offline => "offline".into(),
        ApiError::Unauthorized => "signed_out".into(),
        ApiError::Busy { .. } => "busy".into(),
        ApiError::BadReply => "bad_reply".into(),
        ApiError::Refused { code, status } => {
            code.clone().unwrap_or_else(|| format!("refused_{status}"))
        }
    }
}

#[tauri::command]
pub fn get_state(state: St<'_>) -> Snapshot {
    snapshot(&state)
}

#[tauri::command]
pub async fn log_in(app: AppHandle, state: St<'_>) -> Result<(), String> {
    let state: Arc<AppState> = state.inner().clone();
    if state.signing_in.swap(true, Ordering::SeqCst) {
        return Err("busy".into());
    }
    state.cancel_sign_in.store(false, Ordering::SeqCst);
    changed(&app);
    let result = sign_in(&app, &state).await;
    state.signing_in.store(false, Ordering::SeqCst);
    if result.is_ok() {
        state.signed_out_notice.store(false, Ordering::SeqCst);
        state.wake.notify_one();
    }
    changed(&app);
    result
}

async fn sign_in(app: &AppHandle, state: &Arc<AppState>) -> Result<(), String> {
    let pkce = Pkce::new();
    let lb = Loopback::bind().map_err(|_| "listen".to_string())?;
    let api = state.api();
    let url = api.auth_start_url(lb.port(), &pkce.state, &pkce.challenge);
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|_| "browser".to_string())?;
    let st = state.clone();
    let expected = pkce.state.clone();
    let code = tauri::async_runtime::spawn_blocking(move || {
        lb.wait_for_code(&expected, Duration::from_secs(600), &|| {
            st.cancel_sign_in.load(Ordering::SeqCst)
        })
    })
    .await
    .map_err(|_| "listen".to_string())?
    .map_err(|e| {
        match e {
            AuthError::TimedOut => "timed_out",
            AuthError::StateMismatch => "state",
            AuthError::Denied => "cancelled",
            AuthError::Listen => "listen",
        }
        .to_string()
    })?;
    let signed = api
        .exchange(&code, &pkce.verifier)
        .await
        .map_err(|e| api_code(&e))?;
    crate::token::save(&signed.token).map_err(|_| "keyring".to_string())?;
    *state.token.lock().expect("token") = Some(signed.token);
    let mut s = state.settings.lock().expect("settings");
    s.main = Some(signed.main);
    s.first_run_done = true;
    let _ = s.save(&state.settings_path());
    log::info!("logged in");
    Ok(())
}

#[tauri::command]
pub fn cancel_log_in(state: St<'_>) {
    state.cancel_sign_in.store(true, Ordering::SeqCst);
}

#[tauri::command]
pub async fn log_out(app: AppHandle, state: St<'_>) -> Result<(), String> {
    let api = state.api();
    if api.has_token() {
        // Best effort: the token is forgotten here either way.
        let _ = api.revoke().await;
    }
    crate::token::clear();
    *state.token.lock().expect("token") = None;
    state.server_uploads.lock().expect("server uploads").clear();
    // The live report's link is that account's: the app forgets it (it
    // keeps working on the site until stopped there, under My logs).
    crate::share::forget(&state);
    {
        let mut s = state.settings.lock().expect("settings");
        s.main = None;
        let _ = s.save(&state.settings_path());
    }
    log::info!("logged out");
    changed(&app);
    Ok(())
}

/// Only the fields the window sends are changed.
#[derive(Debug, Deserialize)]
pub struct SettingsPatch {
    pub default_visibility: Option<Visibility>,
    pub site_origin: Option<String>,
    pub start_with_windows: Option<bool>,
    pub only_my_guild: Option<bool>,
    pub upload_limit_kbps: Option<u32>,
    pub region: Option<String>,
    pub first_run_done: Option<bool>,
    /// Answers the first-run question too, whichever way.
    pub live_logging: Option<bool>,
    pub backlog_pulls: Option<BacklogPulls>,
    pub archive_uploaded: Option<bool>,
    pub archive_delete_after_days: Option<u32>,
    /// "Keep the addon up to date"; answers the first-run question too.
    pub addon_auto_update: Option<bool>,
}

/// The tray's "Turn live logging on/off": the same as the switch in Settings.
pub fn toggle_live(app: &AppHandle, state: &AppState) {
    {
        let mut s = state.settings.lock().expect("settings");
        let on = !s.live_logging;
        s.choose_live(on);
        if s.save(&state.settings_path()).is_err() {
            log::warn!("couldn't save the live logging setting");
        }
        log::info!(
            "live logging switched {} from the tray",
            if on { "on" } else { "off" }
        );
    }
    crate::update_tray(app);
    changed(app);
}

#[tauri::command]
pub fn save_settings(
    app: AppHandle,
    state: St<'_>,
    patch: SettingsPatch,
) -> Result<Snapshot, String> {
    {
        let mut s = state.settings.lock().expect("settings");
        let mut next: Settings = s.clone();
        if let Some(v) = patch.default_visibility {
            next.default_visibility = v;
        }
        if let Some(o) = patch.site_origin {
            let o = valid_origin(&o).ok_or("origin")?;
            if o != next.site_origin {
                // A token for one site means nothing to another.
                crate::token::clear();
                *state.token.lock().expect("token") = None;
                // Nor its upload ids.
                state.server_uploads.lock().expect("server uploads").clear();
                crate::share::forget(&state);
                next.main = None;
            }
            next.site_origin = o;
        }
        if let Some(on) = patch.start_with_windows {
            let al = app.autolaunch();
            let r = if on { al.enable() } else { al.disable() };
            r.map_err(|_| "autostart")?;
            next.start_with_windows = on;
        }
        if let Some(v) = patch.only_my_guild {
            next.only_my_guild = v;
        }
        if let Some(k) = patch.upload_limit_kbps {
            next.upload_limit_kbps = k;
            state.throttle.set_rate(k as u64 * 1000);
        }
        if let Some(r) = patch.region {
            if r == "eu" || r == "us" {
                next.region = r;
            }
        }
        if let Some(d) = patch.first_run_done {
            next.first_run_done = d;
        }
        if let Some(on) = patch.live_logging {
            if on != next.live_logging {
                log::info!("live logging switched {}", if on { "on" } else { "off" });
            }
            next.choose_live(on);
        }
        if let Some(b) = patch.backlog_pulls {
            next.backlog_pulls = b;
        }
        if let Some(on) = patch.archive_uploaded {
            if on != next.archive_uploaded {
                log::info!(
                    "archiving uploaded logs switched {}",
                    if on { "on" } else { "off" }
                );
            }
            next.archive_uploaded = on;
        }
        if let Some(d) = patch.archive_delete_after_days {
            if !crate::settings::ARCHIVE_DAYS.contains(&d) {
                return Err("save".into());
            }
            next.archive_delete_after_days = d;
        }
        let addon_on = patch.addon_auto_update == Some(true) && !next.addon_auto_update;
        if let Some(on) = patch.addon_auto_update {
            if on != next.addon_auto_update {
                log::info!(
                    "keeping the addon up to date switched {}",
                    if on { "on" } else { "off" }
                );
            }
            next.choose_addon(on);
        }
        next.save(&state.settings_path()).map_err(|_| "save")?;
        *s = next;
        if addon_on {
            // Switched on: look now, not in six hours.
            state.addon.lock().expect("addon").check_now = true;
            state.addon_wake.notify_one();
        }
    }
    // The tail thread reads the setting every second; the tray says it now.
    crate::update_tray(&app);
    changed(&app);
    Ok(snapshot(&state))
}

fn set_logs_dir(state: &AppState, dir: PathBuf) -> Result<String, String> {
    let logs = wowdir::resolve_pick(&dir).map_err(|e| match e {
        wowdir::PickError::Missing => "missing".to_string(),
        wowdir::PickError::NoLogs => "no_logs".to_string(),
    })?;
    let mut s = state.settings.lock().expect("settings");
    s.logs_dir = Some(logs.clone());
    s.save(&state.settings_path()).map_err(|_| "save")?;
    Ok(logs.to_string_lossy().into_owned())
}

#[tauri::command]
pub async fn choose_logs_folder(app: AppHandle, state: St<'_>) -> Result<Option<String>, String> {
    let picked = app
        .dialog()
        .file()
        .set_title("Choose your World of Warcraft folder")
        .blocking_pick_folder();
    let Some(p) = picked.and_then(|p| p.into_path().ok()) else {
        return Ok(None);
    };
    let r = set_logs_dir(&state, p).map(Some);
    changed(&app);
    r
}

#[tauri::command]
pub fn find_logs_folder(app: AppHandle, state: St<'_>) -> Result<String, String> {
    let found = wowdir::find(None).ok_or("not_found")?;
    let r = set_logs_dir(&state, found);
    changed(&app);
    r
}

fn cache_path(state: &AppState) -> PathBuf {
    state.data_dir.join("backlog-reports.json")
}

/// Reads files one at a time on a background thread, with progress.
fn analyse_in_background(app: AppHandle, state: Arc<AppState>, files: Vec<backlog::LogFile>) {
    std::thread::spawn(move || {
        let live_file = {
            let l = state.live.lock().expect("live");
            match (&l.logs_dir, &l.file) {
                (Some(d), Some(f)) => Some(PathBuf::from(d).join(f)),
                _ => None,
            }
        };
        let mut cache = ReportCache::load(&cache_path(&state));
        {
            let mut b = state.backlog.lock().expect("backlog");
            b.scanning = true;
            b.cancel = false;
            b.files_done = 0;
            b.files_total = files.len() as u32;
            for f in &files {
                b.reports.retain(|r| r.file.path != f.path);
                b.pending.retain(|p| p.path != f.path);
                match cache.get(f) {
                    Some(r) => b.reports.push(r.clone()),
                    None => b.pending.push(f.clone()),
                }
            }
        }
        changed(&app);
        for f in &files {
            if state.backlog.lock().expect("backlog").cancel {
                break;
            }
            let cached = cache.get(f).is_some();
            if !cached && live_file.as_deref() != Some(f.path.as_path()) {
                let mut last = std::time::Instant::now();
                let r = backlog::analyse(f, |n| {
                    if last.elapsed() > Duration::from_millis(250) {
                        last = std::time::Instant::now();
                        state.backlog.lock().expect("backlog").current =
                            Some((f.name.clone(), n, f.size));
                        changed(&app);
                    }
                    !state.backlog.lock().expect("backlog").cancel
                });
                match r {
                    Ok(report) if report.complete => {
                        cache.put(report.clone());
                        let _ = cache.save(&cache_path(&state));
                        let mut b = state.backlog.lock().expect("backlog");
                        b.pending.retain(|p| p.path != f.path);
                        b.reports.push(report);
                    }
                    Ok(_) => {}
                    Err(e) => log::warn!("couldn't read a past log: {}", e.kind()),
                }
            }
            let mut b = state.backlog.lock().expect("backlog");
            b.files_done += 1;
            b.current = None;
            drop(b);
            changed(&app);
        }
        cache.retain_existing();
        let _ = cache.save(&cache_path(&state));
        let mut b = state.backlog.lock().expect("backlog");
        b.scanning = false;
        b.current = None;
        drop(b);
        changed(&app);
    });
}

#[tauri::command]
pub fn backlog_scan(app: AppHandle, state: St<'_>) -> Result<(), String> {
    let dir = state
        .settings
        .lock()
        .expect("settings")
        .logs_dir
        .clone()
        .ok_or("no_folder")?;
    if state.backlog.lock().expect("backlog").scanning {
        return Err("busy".into());
    }
    let files = backlog::find_logs(&dir);
    analyse_in_background(app, state.inner().clone(), files);
    Ok(())
}

#[tauri::command]
pub async fn backlog_choose_files(app: AppHandle, state: St<'_>) -> Result<u32, String> {
    if state.backlog.lock().expect("backlog").scanning {
        return Err("busy".into());
    }
    let picked = app
        .dialog()
        .file()
        .set_title("Choose combat logs")
        .add_filter("Combat logs", &["txt"])
        .blocking_pick_files()
        .unwrap_or_default();
    let files: Vec<_> = picked
        .into_iter()
        .filter_map(|p| p.into_path().ok())
        .filter_map(|p| backlog::describe(&p).ok())
        .collect();
    let n = files.len() as u32;
    if n > 0 {
        analyse_in_background(app, state.inner().clone(), files);
    }
    Ok(n)
}

#[tauri::command]
pub fn backlog_cancel(state: St<'_>) {
    state.backlog.lock().expect("backlog").cancel = true;
}

/// Queues the chosen files' pulls and keys as past logs, skipping any the app
/// or the server already has: in full, or as pull summaries for the wipes
/// the Backlog setting leaves out (`plan`). Returns how many were queued.
#[tauri::command]
pub fn backlog_upload(
    app: AppHandle,
    state: St<'_>,
    paths: Vec<String>,
    visibility: Visibility,
) -> Result<u32, String> {
    if state.token.lock().expect("token").is_none() {
        return Err("signed_out".into());
    }
    let (region, mode) = {
        let s = state.settings.lock().expect("settings");
        (s.region(), s.backlog_pulls)
    };
    let server = state.server_shas.lock().expect("server").clone();
    let reports: Vec<_> = {
        let b = state.backlog.lock().expect("backlog");
        b.reports
            .iter()
            .filter(|r| paths.iter().any(|p| r.file.path.as_os_str() == p.as_str()))
            .cloned()
            .collect()
    };
    // Not a log being archived right now: it's about to go.
    let archiving = state
        .archive
        .lock()
        .expect("archive")
        .busy
        .as_ref()
        .map(|(p, _, _)| p.clone());
    let mut added = 0;
    {
        let mut q = state.queue.lock().expect("queue");
        for r in reports
            .iter()
            .filter(|r| archiving.as_deref() != Some(r.file.path.as_path()))
        {
            for item in
                plan::queue_items(r, mode, visibility, &region, |s| server.contains(&s.sha256))
            {
                if q.add(item) {
                    added += 1;
                }
            }
        }
        q.save().map_err(|_| "save")?;
    }
    log::info!("queued {added} past-log segments");
    state.wake.notify_one();
    changed(&app);
    Ok(added)
}

#[tauri::command]
pub fn backlog_pause(app: AppHandle, state: St<'_>, paused: bool) -> Result<(), String> {
    {
        let mut q = state.queue.lock().expect("queue");
        q.backlog_paused = paused;
        q.save().map_err(|_| "save")?;
    }
    state.wake.notify_one();
    changed(&app);
    Ok(())
}

/// Archives one finished log from the Backlog tab, on a background-priority
/// thread, with progress in the snapshot. Returns the archive's name.
#[tauri::command]
pub async fn archive_log(app: AppHandle, state: St<'_>, path: String) -> Result<String, String> {
    let state: Arc<AppState> = state.inner().clone();
    let file = PathBuf::from(path);
    let rx = mythics_logger_core::priority::spawn_low(move || {
        crate::workers::archive_one(&app, &state, &file)
    });
    match rx.await.map_err(|_| "archive_io".to_string())?? {
        Outcome::Archived { zip, .. } => Ok(zip
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()),
        // Moved or deleted by another tool already: nothing to do.
        Outcome::Vanished => Err("gone".into()),
    }
}

/// "Skip the rest of this log" in the Backlog tab, or undoing it: the log's
/// pulls not yet uploaded won't be (any still waiting leave the queue), and
/// "Archive logs once uploaded" may archive it.
#[tauri::command]
pub fn backlog_skip(app: AppHandle, state: St<'_>, path: String, skip: bool) -> Result<(), String> {
    let file = PathBuf::from(path);
    let size = std::fs::metadata(&file)
        .map_err(|_| "file_missing".to_string())?
        .len();
    if skip {
        let mut q = state.queue.lock().expect("queue");
        if q.remove_waiting_of(&file) > 0 {
            q.save().map_err(|_| "save")?;
        }
    }
    {
        let mut s = state.skips.lock().expect("skips");
        s.set(&file, size, skip);
        s.save(&state.skips_path()).map_err(|_| "save")?;
    }
    log::info!(
        "the rest of a past log {}",
        if skip { "skipped" } else { "no longer skipped" }
    );
    changed(&app);
    Ok(())
}

/// Opens `Logs\MythicsLogsArchive` in Explorer, if it's there.
#[tauri::command]
pub fn open_archive_folder(app: AppHandle, state: St<'_>) -> Result<(), String> {
    let dir = state
        .settings
        .lock()
        .expect("settings")
        .logs_dir
        .clone()
        .ok_or("no_folder")?;
    let folder = archive::folder(&dir);
    if !folder.is_dir() {
        return Err("no_archive".into());
    }
    app.opener()
        .open_path(folder.to_string_lossy(), None::<&str>)
        .map_err(|_| "explorer".to_string())
}

#[derive(Debug, Serialize)]
pub struct History {
    pub rows: Vec<HistoryRow>,
    /// The server couldn't be reached: these are the app's own records.
    pub offline: bool,
}

#[tauri::command]
pub async fn history(state: St<'_>) -> Result<History, String> {
    let api = state.api();
    if !api.has_token() {
        return Err("signed_out".into());
    }
    let (server, offline) = match api.list_uploads().await {
        Ok(rows) => {
            let mut shas = state.server_shas.lock().expect("server");
            shas.extend(rows.iter().filter_map(|r| r.sha256.clone()));
            remember(&state, &rows);
            (rows, false)
        }
        Err(ApiError::Unauthorized) => return Err("signed_out".into()),
        Err(_) => (Vec::new(), true),
    };
    let items = state.queue.lock().expect("queue").items().to_vec();
    Ok(History {
        rows: history_rows(&server, &items),
        offline,
    })
}

/// Keeps the server's word on each upload, for the Live tab's links.
pub fn remember(state: &AppState, rows: &[UploadRow]) {
    let mut known = state.server_uploads.lock().expect("server uploads");
    for r in rows {
        known.insert(r.id.clone(), r.clone());
    }
}

/// The newest uploads' status, pages and pulls, in one request: the History
/// tab asks every so often while any it shows are still processing.
#[tauri::command]
pub async fn recent_uploads(app: AppHandle, state: St<'_>) -> Result<Vec<UploadRow>, String> {
    let rows = state
        .api()
        .recent_uploads(100)
        .await
        .map_err(|e| api_code(&e))?;
    remember(&state, &rows);
    changed(&app);
    Ok(rows)
}

#[tauri::command]
pub async fn set_upload_visibility(
    state: St<'_>,
    id: String,
    visibility: Visibility,
) -> Result<(), String> {
    state
        .api()
        .set_visibility(&id, visibility)
        .await
        .map_err(|e| api_code(&e))?;
    let mut q = state.queue.lock().expect("queue");
    let sha = q
        .items()
        .iter()
        .find(|i| i.upload_id.as_deref() == Some(id.as_str()))
        .map(|i| i.sha256.clone());
    if let Some(i) = sha.and_then(|s| q.get_mut(&s)) {
        i.visibility = visibility;
    }
    let _ = q.save();
    Ok(())
}

#[tauri::command]
pub async fn delete_upload(state: St<'_>, id: String) -> Result<(), String> {
    state
        .api()
        .delete_upload(&id)
        .await
        .map_err(|e| api_code(&e))?;
    state
        .server_uploads
        .lock()
        .expect("server uploads")
        .remove(&id);
    // The local record stays (as uploaded), so the same pull isn't offered
    // again from the Backlog tab.
    log::info!("deleted an upload");
    Ok(())
}

/// Opens a page of the site in the browser: a path on the site in Settings,
/// never anywhere else (`links::page_url`).
#[tauri::command]
pub fn open_site(app: AppHandle, state: St<'_>, path: Option<String>) -> Result<(), String> {
    open_page(&app, &state, path.as_deref().unwrap_or("/"))
}

/// Opens a log page: the log, a raid boss, a pull or a key. Only the
/// contract's "Page links" shapes, on the site in Settings (`links.rs`).
#[tauri::command]
pub fn open_log(app: AppHandle, state: St<'_>, path: String) -> Result<(), String> {
    let path = crate::links::log_page(&path).ok_or("path")?;
    open_page(&app, &state, &path)
}

pub fn open_page(app: &AppHandle, state: &AppState, path: &str) -> Result<(), String> {
    let origin = state.settings.lock().expect("settings").site_origin.clone();
    let url = crate::links::page_url(&origin, path).ok_or("path")?;
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|_| "browser".to_string())
}
