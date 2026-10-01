//! What the app holds while it runs, and the snapshot the UI draws from.

use crate::settings::Settings;
use mythics_logger_core::api::{Api, FightRow, Main, UploadRow, Visibility};
use mythics_logger_core::archive::{self, FolderSize, NotEligible, Skips};
use mythics_logger_core::backlog::FileReport;
use mythics_logger_core::plan::{self, BacklogPulls};
use mythics_logger_core::queue::{now_ms, Counts, Item, Origin, Queue, State};
use mythics_logger_core::splitter::{Current, Kind, Segment};
use mythics_logger_core::throttle::Throttle;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub struct AppState {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub settings: Mutex<Settings>,
    pub queue: Arc<Mutex<Queue>>,
    pub live: Mutex<LiveStatus>,
    pub backlog: Mutex<Backlog>,
    pub throttle: Arc<Throttle>,
    pub token: Mutex<Option<String>>,
    /// Upload progress by SHA-256: (chunks sent, chunks).
    pub progress: Mutex<HashMap<String, (u32, u32)>>,
    /// Hashes the server says it has, from the last History fetch.
    pub server_shas: Mutex<HashSet<String>>,
    /// The server's last word on each upload by id: whether it's parsed yet,
    /// its page, and the pulls and keys it found. Filled by History and by
    /// the poll while tonight's uploads are processing (`workers.rs`).
    pub server_uploads: Mutex<HashMap<String, UploadRow>>,
    pub signing_in: AtomicBool,
    pub cancel_sign_in: AtomicBool,
    pub wake: tokio::sync::Notify,
    /// Set when a token was refused mid-upload.
    pub signed_out_notice: AtomicBool,
    pub archive: Mutex<ArchiveState>,
    /// Logs whose remaining pulls the player chose to skip (Backlog).
    pub skips: Mutex<Skips>,
    /// The in-game addon (`addon.rs`), and the nudge for its job.
    pub addon: Mutex<crate::addon::AddonState>,
    pub addon_wake: tokio::sync::Notify,
    /// The app's own updates (`app_update.rs`).
    pub app_update: Mutex<crate::app_update::AppUpdateState>,
    pub app_update_wake: tokio::sync::Notify,
}

/// Archiving finished logs (`mythics_logger_core::archive`).
#[derive(Debug, Default)]
pub struct ArchiveState {
    /// The log being archived now: its path, bytes done and of. One at a
    /// time, from the Backlog tab or "Archive logs once uploaded".
    pub busy: Option<(PathBuf, u64, u64)>,
    /// The archive folder's size, the Logs folder it's in, and when.
    pub size: Option<(Instant, PathBuf, FolderSize)>,
}

impl ArchiveState {
    /// The archive folder's size, measured at most once a minute (and again
    /// after anything is archived or deleted).
    pub fn size(&mut self, logs_dir: &Path) -> FolderSize {
        match &self.size {
            Some((at, dir, s)) if dir == logs_dir && at.elapsed() < Duration::from_secs(60) => *s,
            _ => {
                let s = archive::folder_size(logs_dir);
                self.size = Some((Instant::now(), logs_dir.to_path_buf(), s));
                s
            }
        }
    }
}

/// The archive folder, for Settings, and the log being archived.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ArchiveView {
    /// `<Logs>\MythicsLogsArchive`, once there's a Logs folder.
    pub folder: Option<String>,
    pub exists: bool,
    pub size: u64,
    pub files: u32,
    pub busy: Option<ArchiveBusy>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ArchiveBusy {
    pub path: String,
    pub name: String,
    pub pct: u32,
}

pub fn archive_view(a: &mut ArchiveState, logs_dir: Option<&Path>) -> ArchiveView {
    let size = logs_dir.map(|d| a.size(d)).unwrap_or_default();
    ArchiveView {
        folder: logs_dir.map(|d| archive::folder(d).to_string_lossy().into_owned()),
        exists: logs_dir.is_some_and(|d| archive::folder(d).is_dir()),
        size: size.bytes,
        files: size.files,
        busy: a.busy.as_ref().map(|(p, done, of)| ArchiveBusy {
            path: p.to_string_lossy().into_owned(),
            name: p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            pct: if *of == 0 {
                0
            } else {
                (done * 100 / of).min(100) as u32
            },
        }),
    }
}

impl AppState {
    pub fn settings_path(&self) -> PathBuf {
        self.config_dir.join("settings.json")
    }

    pub fn skips_path(&self) -> PathBuf {
        self.data_dir.join("skipped-logs.json")
    }

    pub fn api(&self) -> Api {
        let origin = self.settings.lock().expect("settings").site_origin.clone();
        Api::new(&origin).with_token(self.token.lock().expect("token").clone())
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct LiveStatus {
    /// "off" (live logging is switched off in Settings), "searching" (no
    /// Logs folder yet), "waiting" (no log being written), or "live".
    pub status: &'static str,
    pub logs_dir: Option<String>,
    /// The file's name only.
    pub file: Option<String>,
    pub zone: Option<String>,
    pub difficulty: Option<u32>,
    pub advanced: Option<bool>,
    pub current: Option<Current>,
    pub last_activity_ms: u64,
}

#[derive(Debug, Clone, Default)]
pub struct Backlog {
    pub scanning: bool,
    pub cancel: bool,
    pub files_done: u32,
    pub files_total: u32,
    /// Bytes read of the file being read now, and its size.
    pub current: Option<(String, u64, u64)>,
    pub reports: Vec<FileReport>,
    /// Files found but not read yet (a scan in progress).
    pub pending: Vec<mythics_logger_core::backlog::LogFile>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PullView {
    pub sha256: String,
    pub kind: Kind,
    pub opened_as: Kind,
    pub name: Option<String>,
    pub difficulty: Option<u32>,
    pub key_level: Option<u32>,
    /// This boss's pull count on this difficulty tonight.
    pub pull_number: u32,
    pub success: Option<bool>,
    pub boss_hp_pct: Option<f64>,
    pub duration_ms: Option<u64>,
    pub size: u64,
    pub start_time: String,
    /// "waiting", "uploading", "done", "failed".
    pub state: &'static str,
    pub progress_pct: Option<u32>,
    pub error: Option<String>,
    pub upload_id: Option<String>,
    pub visibility: Visibility,
    /// Not sent: a raid member had uploaded it already, and the links are
    /// to their copy.
    pub already: bool,
    /// The server's status once it's been asked: "receiving", "queued",
    /// "parsed" or "failed".
    pub server_status: Option<String>,
    /// The log it's in: the pulls and keys of one file share it, so the tab
    /// groups them (`group`), and links the log once one is parsed.
    #[serde(flatten)]
    pub log: LogPlace,
    /// The pulls and keys the server found, each with its page.
    pub fights: Vec<FightRow>,
}

/// Where an upload sits in its log, and its pages on the site.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct LogPlace {
    /// The same for every upload of one log file: the app's session key,
    /// else the server's session, else the upload itself.
    pub group: String,
    /// The log file's name (never its path), when this app sent it.
    pub file_name: Option<String>,
    pub encounter_id: Option<u32>,
    /// The log's page, once the server has parsed this upload.
    pub log_url: Option<String>,
    /// The pull's or key's own page.
    pub page_url: Option<String>,
    /// A raid pull's boss page: every pull of that boss in the log.
    pub boss_url: Option<String>,
    /// The server's word on where it goes: "raid", "mplus" or "dungeon".
    pub section: Option<String>,
}

/// The pull or key an upload is: its key, else its one pull.
fn top_fight(fights: &[FightRow]) -> Option<&FightRow> {
    fights
        .iter()
        .find(|f| f.kind.as_deref() == Some("key"))
        .or_else(|| fights.first())
}

/// An upload's place, from the app's record and the server's.
pub fn log_place(
    local: Option<&Item>,
    server: Option<&UploadRow>,
    keys_by_session: &HashMap<String, String>,
) -> LogPlace {
    let top = server.and_then(|r| top_fight(&r.fights));
    // A raid member's copy, or this pull's own summary on the site.
    let already = local.and_then(|i| i.already.as_ref().or(i.summarised.as_ref()));
    let session_id = server.and_then(|r| r.session_id.clone());
    let group = match (
        local.and_then(|i| i.session_key.clone()),
        session_id.as_ref(),
    ) {
        (Some(k), _) => format!("k:{k}"),
        (None, Some(s)) => keys_by_session
            .get(s)
            .map(|k| format!("k:{k}"))
            .unwrap_or_else(|| format!("s:{s}")),
        (None, None) => match (local, server) {
            (Some(i), _) => format!("u:{}", i.sha256),
            (None, Some(r)) => format!("u:{}", r.id),
            (None, None) => String::new(),
        },
    };
    LogPlace {
        group,
        file_name: local.and_then(|i| i.file_name.clone()),
        encounter_id: local
            .and_then(|i| i.segment.encounter_id)
            .or_else(|| server.and_then(|r| r.encounter_id))
            .or_else(|| top.and_then(|f| f.encounter_id)),
        log_url: server
            .and_then(|r| r.log_page().map(str::to_string))
            .or_else(|| already.and_then(|a| a.log_url.clone())),
        page_url: top
            .and_then(|f| f.url.clone())
            .or_else(|| already.and_then(|a| a.url.clone())),
        boss_url: top
            .and_then(|f| f.boss_url.clone())
            .or_else(|| already.and_then(|a| a.boss_url.clone())),
        section: top.and_then(|f| f.section.clone()),
    }
}

/// The app's session key for each server session it knows of: so an
/// upload sent from here and one only the server lists (the same log, from
/// another computer's queue) group together.
pub fn keys_by_session(server: &[&UploadRow], items: &[Item]) -> HashMap<String, String> {
    let by_id: HashMap<&str, &Item> = items
        .iter()
        .filter_map(|i| i.upload_id.as_deref().map(|id| (id, i)))
        .collect();
    let mut out = HashMap::new();
    for r in server {
        if let (Some(s), Some(k)) = (
            r.session_id.as_ref(),
            by_id
                .get(r.id.as_str())
                .and_then(|i| i.session_key.as_ref()),
        ) {
            out.insert(s.clone(), k.clone());
        }
    }
    out
}

fn state_name(s: State) -> &'static str {
    match s {
        State::Waiting => "waiting",
        State::Uploading => "uploading",
        State::Done => "done",
        State::Failed => "failed",
    }
}

/// Tonight's live pulls, newest first: live items from the last 16 hours.
pub fn tonights_pulls(
    items: &[Item],
    progress: &HashMap<String, (u32, u32)>,
    server: &HashMap<String, UploadRow>,
    now: u64,
) -> Vec<PullView> {
    let since = tonight_since(now);
    let mut counts: HashMap<(Option<u32>, Option<u32>), u32> = HashMap::new();
    let known: Vec<&UploadRow> = server.values().collect();
    let keys = keys_by_session(&known, items);
    let mut out: Vec<PullView> = items
        .iter()
        .filter(|i| i.origin == Origin::Live && i.added_ms >= since)
        .map(|i| {
            let s = &i.segment;
            let n = counts.entry((s.encounter_id, s.difficulty)).or_insert(0);
            *n += 1;
            let progress_pct = match (i.state, progress.get(&i.sha256)) {
                (State::Uploading, Some((sent, total))) if *total > 0 => Some(sent * 100 / total),
                (State::Uploading, _) => Some(0),
                _ => None,
            };
            let on_server = i.upload_id.as_ref().and_then(|id| server.get(id));
            PullView {
                server_status: on_server.and_then(|r| r.status.clone()).or_else(|| {
                    // A raid member's copy: already parsed, or it wouldn't match.
                    i.already.as_ref().map(|_| "parsed".to_string())
                }),
                already: i.already.is_some(),
                log: log_place(Some(i), on_server, &keys),
                fights: on_server.map(|r| r.fights.clone()).unwrap_or_default(),
                sha256: i.sha256.clone(),
                kind: s.kind,
                opened_as: s.opened_as,
                name: s.name.clone(),
                difficulty: s.difficulty,
                key_level: s.key_level,
                pull_number: *n,
                success: s.success,
                boss_hp_pct: s.boss_hp_pct,
                duration_ms: s.duration_ms,
                size: s.size,
                start_time: s.start_iso(i.year_hint),
                state: state_name(i.state),
                progress_pct,
                error: i.error.clone(),
                upload_id: i.upload_id.clone(),
                visibility: i.visibility,
            }
        })
        .collect();
    out.reverse();
    out
}

/// Tonight: the last 16 hours.
fn tonight_since(now: u64) -> u64 {
    now.saturating_sub(16 * 3600 * 1000)
}

/// Tonight's sent uploads the server hasn't finished with, by id: the ones
/// the Live tab is waiting to link. Also any the server last said was queued
/// for its parser (from History). Not one it says is still receiving: that's
/// this app's own upload, still going, or one given up on.
pub fn awaiting_parse(
    items: &[Item],
    server: &HashMap<String, UploadRow>,
    now: u64,
) -> Vec<String> {
    let since = tonight_since(now);
    let mut ids: Vec<String> = items
        .iter()
        .filter(|i| i.origin == Origin::Live && i.added_ms >= since && i.state == State::Done)
        .filter_map(|i| i.upload_id.clone())
        .filter(|id| server.get(id).is_none_or(UploadRow::is_processing))
        .collect();
    ids.extend(
        server
            .values()
            .filter(|r| r.status.as_deref() == Some("queued"))
            .map(|r| r.id.clone()),
    );
    ids.sort();
    ids.dedup();
    ids
}

#[derive(Debug, Clone, Serialize)]
pub struct BacklogFileView {
    pub path: String,
    pub name: String,
    /// The folder it's in, when it isn't the Logs folder itself.
    pub folder: Option<String>,
    pub size: u64,
    pub modified_ms: u64,
    pub first_time: Option<String>,
    pub analysed: bool,
    pub encounters: u32,
    pub keys: u32,
    /// Pulls and keys that would be sent.
    pub segments: u32,
    /// Of those, already uploaded (skipped).
    pub already: u32,
    /// Of those, waiting in the queue.
    pub queued: u32,
    /// About what the rest would upload, compressed, with the Backlog
    /// setting's "Kills and each boss's best wipe", and with "All pulls".
    pub estimate_best: u64,
    pub estimate_all: u64,
    /// Of the rest, the wipes "Kills and each boss's best wipe" summarises.
    pub summaries: u32,
    pub advanced: Option<bool>,
    pub version: Option<u32>,
    /// The file being logged live right now: not offered here.
    pub live: bool,
    /// Why it can't be archived now, from the quick checks (the newest log,
    /// changed in the last 10 minutes, pulls still to upload, not directly
    /// in the Logs folder); `None` if it may be. Whether another program has
    /// it open is checked only when it's archived.
    pub archive_block: Option<NotEligible>,
    /// The player chose to skip the rest of this log: its pulls not yet
    /// uploaded won't be, and "Archive logs once uploaded" may archive it.
    pub skipped: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct BacklogView {
    pub scanning: bool,
    pub files_done: u32,
    pub files_total: u32,
    pub current_name: Option<String>,
    pub current_pct: Option<u32>,
    pub files: Vec<BacklogFileView>,
    pub total_size: u64,
    pub paused: bool,
    /// Past-log pulls and keys uploaded, of those queued.
    pub done: u32,
    pub total: u32,
    pub failed: u32,
}

/// What `backlog_view` needs to say whether each log may be archived: the
/// newest combat log in the Logs folder, and now.
pub struct ArchiveCheck<'a> {
    pub newest: Option<&'a Path>,
    pub now: SystemTime,
    /// Logs whose rest the player skipped.
    pub skips: Option<&'a Skips>,
}

fn skipped(check: &ArchiveCheck<'_>, path: &Path, size: u64) -> bool {
    check.skips.is_some_and(|s| s.is_skipped(path, size))
}

fn archive_block(
    path: &Path,
    modified_ms: u64,
    live: bool,
    logs_dir: Option<&Path>,
    q: &Queue,
    check: &ArchiveCheck<'_>,
) -> Option<NotEligible> {
    if live {
        return Some(NotEligible::Newest);
    }
    let Some(dir) = logs_dir else {
        return Some(NotEligible::NotInLogs);
    };
    archive::blocker(
        path,
        dir,
        check.newest,
        UNIX_EPOCH + Duration::from_millis(modified_ms),
        check.now,
        archive::QUIET_FOR,
        q.has_pending(path),
    )
}

pub fn backlog_view(
    b: &Backlog,
    q: &Queue,
    server: &HashSet<String>,
    logs_dir: Option<&std::path::Path>,
    live_file: Option<&std::path::Path>,
    check: &ArchiveCheck<'_>,
) -> BacklogView {
    let mut files: Vec<BacklogFileView> = Vec::new();
    let folder_of = |p: &std::path::Path| {
        let parent = p.parent()?;
        if Some(parent) == logs_dir {
            None
        } else {
            parent.file_name().map(|n| n.to_string_lossy().into_owned())
        }
    };
    for r in &b.reports {
        let mut segments = 0;
        let mut already = 0;
        let mut queued = 0;
        for s in r.uploadable() {
            segments += 1;
            match q.get(&s.sha256).map(|i| i.state) {
                Some(State::Done) => already += 1,
                Some(State::Waiting | State::Uploading) => queued += 1,
                _ if server.contains(&s.sha256) => already += 1,
                _ => {}
            }
        }
        // What's left to send: not in the queue in any state, not on the site.
        let sent = |s: &Segment| q.contains(&s.sha256) || server.contains(&s.sha256);
        let best = plan::sizes(r, BacklogPulls::KillsAndBestWipe, sent);
        let all = plan::sizes(r, BacklogPulls::All, sent);
        files.push(BacklogFileView {
            path: r.file.path.to_string_lossy().into_owned(),
            name: r.file.name.clone(),
            folder: folder_of(&r.file.path),
            size: r.file.size,
            modified_ms: r.file.modified_ms,
            first_time: r.first_time.clone(),
            analysed: true,
            encounters: r.encounters,
            keys: r.keys,
            segments,
            already,
            queued,
            estimate_best: best.estimate(),
            estimate_all: all.estimate(),
            summaries: best.summaries,
            advanced: r.header.as_ref().and_then(|h| h.advanced),
            version: r.header.as_ref().and_then(|h| h.version),
            live: live_file == Some(r.file.path.as_path()),
            archive_block: archive_block(
                &r.file.path,
                r.file.modified_ms,
                live_file == Some(r.file.path.as_path()),
                logs_dir,
                q,
                check,
            ),
            skipped: skipped(check, &r.file.path, r.file.size),
        });
    }
    for f in &b.pending {
        files.push(BacklogFileView {
            path: f.path.to_string_lossy().into_owned(),
            name: f.name.clone(),
            folder: folder_of(&f.path),
            size: f.size,
            modified_ms: f.modified_ms,
            first_time: None,
            analysed: false,
            encounters: 0,
            keys: 0,
            segments: 0,
            already: 0,
            queued: 0,
            estimate_best: 0,
            estimate_all: 0,
            summaries: 0,
            advanced: None,
            version: None,
            live: live_file == Some(f.path.as_path()),
            archive_block: archive_block(
                &f.path,
                f.modified_ms,
                live_file == Some(f.path.as_path()),
                logs_dir,
                q,
                check,
            ),
            skipped: skipped(check, &f.path, f.size),
        });
    }
    files.sort_by_key(|f| std::cmp::Reverse(f.modified_ms));
    let c: Counts = q.counts();
    BacklogView {
        scanning: b.scanning,
        files_done: b.files_done,
        files_total: b.files_total,
        current_name: b.current.as_ref().map(|c| c.0.clone()),
        current_pct: b
            .current
            .as_ref()
            .map(|(_, n, t)| if *t == 0 { 100 } else { (n * 100 / t) as u32 }),
        total_size: files.iter().map(|f| f.size).sum(),
        files,
        paused: q.backlog_paused,
        done: c.backlog_done,
        total: c.backlog_total,
        failed: q
            .items()
            .iter()
            .filter(|i| i.origin == Origin::Backlog && i.state == State::Failed)
            .count() as u32,
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub signed_in: bool,
    pub signing_in: bool,
    /// The token was refused: log in again.
    pub signed_out_notice: bool,
    pub main: Option<Main>,
    pub settings: Settings,
    pub dev: bool,
    pub version: &'static str,
    pub live: LiveStatus,
    pub pulls: Vec<PullView>,
    pub backlog: BacklogView,
    pub counts: Counts,
    pub archive: ArchiveView,
    pub addon: crate::addon::AddonView,
    pub app_update: crate::app_update::AppUpdateView,
}

pub fn snapshot(s: &AppState) -> Snapshot {
    let settings = s.settings.lock().expect("settings").clone();
    let live = s.live.lock().expect("live").clone();
    let progress = s.progress.lock().expect("progress").clone();
    let server = s.server_shas.lock().expect("server").clone();
    let q = s.queue.lock().expect("queue");
    let live_path = match (&live.logs_dir, &live.file) {
        (Some(d), Some(f)) => Some(PathBuf::from(d).join(f)),
        _ => None,
    };
    let b = s.backlog.lock().expect("backlog");
    // The newest log is the game's: the folder is read only when there are
    // logs to show.
    let newest = match (
        &settings.logs_dir,
        b.reports.is_empty() && b.pending.is_empty(),
    ) {
        (Some(d), false) => mythics_logger_core::tailer::newest_log(d)
            .ok()
            .flatten()
            .map(|(p, _, _)| p),
        _ => None,
    };
    let skips = s.skips.lock().expect("skips");
    let backlog = backlog_view(
        &b,
        &q,
        &server,
        settings.logs_dir.as_deref(),
        live_path.as_deref(),
        &ArchiveCheck {
            newest: newest.as_deref(),
            now: SystemTime::now(),
            skips: Some(&skips),
        },
    );
    drop(skips);
    drop(b);
    let archive = archive_view(
        &mut s.archive.lock().expect("archive"),
        settings.logs_dir.as_deref(),
    );
    Snapshot {
        signed_in: s.token.lock().expect("token").is_some(),
        signing_in: s.signing_in.load(std::sync::atomic::Ordering::Relaxed),
        signed_out_notice: s
            .signed_out_notice
            .load(std::sync::atomic::Ordering::Relaxed),
        main: settings.main.clone(),
        dev: cfg!(debug_assertions) || std::env::var_os("MYTHICS_LOGGER_DEV").is_some(),
        version: mythics_logger_core::CLIENT_VERSION,
        pulls: tonights_pulls(
            q.items(),
            &progress,
            &s.server_uploads.lock().expect("server uploads"),
            now_ms(),
        ),
        counts: q.counts(),
        backlog,
        archive,
        addon: crate::addon::view(&s.addon.lock().expect("addon")),
        app_update: crate::app_update::view(&s.app_update.lock().expect("app update")),
        live,
        settings,
    }
}

/// History rows: the server's list, with the app's own record for anything
/// the server hasn't listed yet.
#[derive(Debug, Clone, Serialize)]
pub struct HistoryRow {
    pub id: Option<String>,
    pub sha256: Option<String>,
    pub kind: Option<String>,
    pub name: Option<String>,
    pub difficulty: Option<u32>,
    pub key_level: Option<u32>,
    pub success: Option<bool>,
    pub start_time: Option<String>,
    pub size: Option<u64>,
    pub visibility: Option<Visibility>,
    pub past: bool,
    /// The server's status: "receiving", "queued", "parsed" or "failed".
    pub status: Option<String>,
    /// Its log (for grouping) and its pages on the site.
    #[serde(flatten)]
    pub log: LogPlace,
    /// The pulls and keys the server found, each with its page.
    pub fights: Vec<FightRow>,
    pub on_server: bool,
    /// Not sent: a raid member had uploaded it already (the links are to
    /// their copy).
    pub already: bool,
    /// A past log's wipe sent as a pull summary: its result only, "details
    /// not uploaded".
    pub summary: bool,
    /// The boss's health at the end, in percent.
    pub boss_hp_pct: Option<f64>,
}

pub fn history_rows(server: &[UploadRow], items: &[Item]) -> Vec<HistoryRow> {
    let by_sha: HashMap<&str, &Item> = items.iter().map(|i| (i.sha256.as_str(), i)).collect();
    let by_id: HashMap<&str, &Item> = items
        .iter()
        .filter_map(|i| i.upload_id.as_deref().map(|id| (id, i)))
        .collect();
    let mut seen = HashSet::new();
    let known: Vec<&UploadRow> = server.iter().collect();
    let keys = keys_by_session(&known, items);
    let mut rows: Vec<HistoryRow> = server
        .iter()
        .map(|r| {
            let local = r
                .sha256
                .as_deref()
                .and_then(|s| by_sha.get(s))
                .or_else(|| by_id.get(r.id.as_str()))
                .copied();
            if let Some(l) = local {
                seen.insert(l.sha256.clone());
            }
            // The key, or the boss pull, that the upload is: for an upload
            // this app has no record of (sent from another computer).
            let top = r.fights.iter().find(|f| f.parent_id.is_none());
            HistoryRow {
                id: Some(r.id.clone()),
                sha256: r.sha256.clone().or_else(|| local.map(|l| l.sha256.clone())),
                kind: r
                    .kind
                    .clone()
                    .or_else(|| local.map(|l| l.segment.kind.as_str().to_string())),
                name: local
                    .and_then(|l| l.segment.name.clone())
                    .or_else(|| top.and_then(|f| f.name.clone())),
                difficulty: r
                    .difficulty
                    .or_else(|| local.and_then(|l| l.segment.difficulty)),
                key_level: r
                    .key_level
                    .or_else(|| local.and_then(|l| l.segment.key_level)),
                success: local
                    .and_then(|l| l.segment.success)
                    .or_else(|| top.and_then(|f| f.kill)),
                start_time: r
                    .start_time
                    .clone()
                    .or_else(|| local.map(|l| l.segment.start_iso(l.year_hint))),
                size: r.size.or_else(|| local.map(|l| l.segment.size)),
                visibility: r.visibility.or_else(|| local.map(|l| l.visibility)),
                past: local.is_some_and(|l| l.origin == Origin::Backlog),
                status: r.status.clone(),
                log: log_place(local, Some(r), &keys),
                fights: r.fights.clone(),
                on_server: true,
                already: false,
                summary: r.kind.as_deref() == Some("summary")
                    || top.is_some_and(|f| f.summary)
                    || local.is_some_and(|l| l.summarised.is_some()),
                boss_hp_pct: top
                    .and_then(|f| f.boss_hp_pct)
                    .or_else(|| local.and_then(|l| l.segment.boss_hp_pct)),
            }
        })
        .collect();
    for i in items
        .iter()
        .filter(|i| i.state == State::Done && !seen.contains(&i.sha256))
    {
        if server
            .iter()
            .any(|r| r.id.as_str() == i.upload_id.as_deref().unwrap_or("-"))
        {
            continue;
        }
        rows.push(HistoryRow {
            id: i.upload_id.clone(),
            sha256: Some(i.sha256.clone()),
            kind: Some(i.segment.kind.as_str().to_string()),
            name: i.segment.name.clone(),
            difficulty: i.segment.difficulty,
            key_level: i.segment.key_level,
            success: i.segment.success,
            start_time: Some(i.segment.start_iso(i.year_hint)),
            size: Some(i.segment.size),
            visibility: Some(i.visibility),
            past: i.origin == Origin::Backlog,
            status: i
                .already
                .as_ref()
                .or(i.summarised.as_ref())
                .map(|_| "parsed".to_string()),
            log: log_place(Some(i), None, &keys),
            fights: Vec::new(),
            on_server: false,
            already: i.already.is_some(),
            summary: i.summarised.is_some(),
            boss_hp_pct: i.segment.boss_hp_pct,
        });
    }
    rows.sort_by(|a, b| b.start_time.cmp(&a.start_time));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use mythics_logger_core::backlog::{analyse, describe};
    use mythics_logger_core::splitter::split_file;

    const RAID: &str = include_str!("../../core/tests/fixtures/raid_night.txt");
    const KEY: &str = include_str!("../../core/tests/fixtures/mplus_key.txt");

    fn no_check() -> ArchiveCheck<'static> {
        ArchiveCheck {
            newest: None,
            now: SystemTime::now(),
            skips: None,
        }
    }

    fn setup(body: &str) -> (tempfile::TempDir, PathBuf, Queue) {
        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join("WoWCombatLog-092826_200101.txt");
        std::fs::write(&log, body.replace("\r\n", "\n").replace('\n', "\r\n")).unwrap();
        let q = Queue::load(&tmp.path().join("data"));
        (tmp, log, q)
    }

    #[test]
    fn tonights_pulls_number_each_boss_and_show_progress() {
        let (_tmp, log, mut q) = setup(RAID);
        let mut segs = Vec::new();
        split_file(&log, |s| segs.push(s), |_| true).unwrap();
        for s in segs {
            q.add(Item::new(
                Origin::Live,
                log.clone(),
                s,
                Visibility::Public,
                "eu",
            ));
        }
        let second = q.items()[1].sha256.clone();
        q.get_mut(&second).unwrap().state = State::Uploading;
        let progress = HashMap::from([(second.clone(), (16u32, 25u32))]);
        let none = HashMap::new();
        let pulls = tonights_pulls(q.items(), &progress, &none, now_ms());
        assert_eq!(pulls.len(), 2);
        // Newest first: the kill (pull 2) at 64%, then the wipe (pull 1).
        assert_eq!(pulls[0].pull_number, 2);
        assert_eq!(pulls[0].progress_pct, Some(64));
        assert_eq!(pulls[0].state, "uploading");
        assert_eq!(pulls[1].pull_number, 1);
        assert_eq!(pulls[1].boss_hp_pct, Some(23.4));
        // Past logs never show as tonight's pulls.
        let old = now_ms() + 17 * 3600 * 1000;
        assert!(tonights_pulls(q.items(), &progress, &none, old).is_empty());
    }

    #[test]
    fn tonights_pulls_link_once_the_server_has_parsed_them() {
        let (_tmp, log, mut q) = setup(RAID);
        let mut segs = Vec::new();
        split_file(&log, |s| segs.push(s), |_| true).unwrap();
        for (i, s) in segs.into_iter().enumerate() {
            let sha = s.sha256.clone();
            q.add(Item::new(
                Origin::Live,
                log.clone(),
                s,
                Visibility::Public,
                "eu",
            ));
            let it = q.get_mut(&sha).unwrap();
            it.state = State::Done;
            it.upload_id = Some(format!("{}", 40 + i));
        }
        let now = now_ms();
        // Nothing heard from the server yet: both are awaited.
        let mut server = HashMap::new();
        assert_eq!(awaiting_parse(q.items(), &server, now), ["40", "41"]);
        let rows: Vec<UploadRow> = serde_json::from_value(serde_json::json!([
            {"id": 41, "status": "parsed", "session_id": 12, "log_url": "/logs/12/",
             "url": "/logs/12/", "fights": [
                {"id": 9, "kind": "encounter", "encounter_id": 3129, "name": "Plexus Sentinel",
                 "difficulty": 16, "kill": true, "duration_ms": 312499, "section": "raid",
                 "url": "/logs/12/pulls/9/", "boss_url": "/logs/12/bosses/3129/"}]},
            {"id": 40, "status": "queued", "session_id": 12, "log_url": null, "fights": []}
        ]))
        .unwrap();
        for r in rows {
            server.insert(r.id.clone(), r);
        }
        let pulls = tonights_pulls(q.items(), &HashMap::new(), &server, now);
        assert_eq!(pulls[0].upload_id.as_deref(), Some("41"));
        assert_eq!(pulls[0].server_status.as_deref(), Some("parsed"));
        assert_eq!(pulls[0].log.log_url.as_deref(), Some("/logs/12/"));
        assert_eq!(pulls[0].log.page_url.as_deref(), Some("/logs/12/pulls/9/"));
        assert_eq!(
            pulls[0].log.boss_url.as_deref(),
            Some("/logs/12/bosses/3129/")
        );
        assert_eq!(pulls[0].log.section.as_deref(), Some("raid"));
        assert_eq!(pulls[0].fights[0].url.as_deref(), Some("/logs/12/pulls/9/"));
        assert_eq!(pulls[1].server_status.as_deref(), Some("queued"));
        assert_eq!(pulls[1].log.log_url, None);
        // One file, one log: both pulls group together, by the file's key.
        assert_eq!(pulls[0].log.group, pulls[1].log.group);
        assert!(pulls[0].log.group.starts_with("k:"));
        assert_eq!(
            pulls[0].log.file_name.as_deref(),
            Some("WoWCombatLog-092826_200101.txt")
        );
        // Only the one still queued is polled for.
        assert_eq!(awaiting_parse(q.items(), &server, now), ["40"]);
        server.get_mut("40").unwrap().status = Some("parsed".into());
        assert!(awaiting_parse(q.items(), &server, now).is_empty());
        // A backlog upload the server said was queued (from History) is too;
        // one it says is still receiving is this app's to finish, not polled.
        let more: Vec<UploadRow> = serde_json::from_value(serde_json::json!([
            {"id": 7, "status": "queued"}, {"id": 8, "status": "receiving"}
        ]))
        .unwrap();
        for r in more {
            server.insert(r.id.clone(), r);
        }
        assert_eq!(awaiting_parse(q.items(), &server, now), ["7"]);
    }

    #[test]
    fn backlog_counts_what_is_already_uploaded_or_waiting() {
        let (_tmp, log, mut q) = setup(&format!("{RAID}{KEY}"));
        let report = analyse(&describe(&log).unwrap(), |_| true).unwrap();
        let segs: Vec<_> = report.uploadable().cloned().collect();
        assert_eq!(segs.len(), 3);
        // One already sent by this app, one the server knows from elsewhere,
        // one waiting.
        q.add(Item::new(
            Origin::Backlog,
            log.clone(),
            segs[0].clone(),
            Visibility::Public,
            "eu",
        ));
        q.get_mut(&segs[0].sha256).unwrap().state = State::Done;
        q.add(Item::new(
            Origin::Backlog,
            log.clone(),
            segs[2].clone(),
            Visibility::Public,
            "eu",
        ));
        let server = HashSet::from([segs[1].sha256.clone()]);
        let b = Backlog {
            reports: vec![report],
            ..Default::default()
        };
        let v = backlog_view(&b, &q, &server, log.parent(), None, &no_check());
        let f = &v.files[0];
        assert_eq!((f.segments, f.already, f.queued), (3, 2, 1));
        assert_eq!((f.encounters, f.keys), (6, 1));
        assert!(!f.live);
        assert_eq!((v.done, v.total), (1, 2));
        let v = backlog_view(&b, &q, &server, log.parent(), Some(&log), &no_check());
        assert!(v.files[0].live, "the file being logged live is marked");
    }

    #[test]
    fn backlog_says_which_logs_may_be_archived_and_why_not() {
        let (_tmp, log, mut q) = setup(RAID);
        let report = analyse(&describe(&log).unwrap(), |_| true).unwrap();
        let segs: Vec<_> = report.uploadable().cloned().collect();
        let b = Backlog {
            reports: vec![report],
            ..Default::default()
        };
        let later = ArchiveCheck {
            newest: None,
            now: SystemTime::now() + Duration::from_secs(3600),
            skips: None,
        };
        let block = |q: &Queue, live: Option<&Path>, c: &ArchiveCheck<'_>| {
            backlog_view(&b, q, &HashSet::new(), log.parent(), live, c).files[0].archive_block
        };
        // Just written: the game may still be writing to it.
        assert_eq!(block(&q, None, &no_check()), Some(NotEligible::Recent));
        assert_eq!(block(&q, None, &later), None);
        assert_eq!(block(&q, Some(&log), &later), Some(NotEligible::Newest));
        let newest = ArchiveCheck {
            newest: Some(&log),
            now: later.now,
            skips: None,
        };
        assert_eq!(block(&q, None, &newest), Some(NotEligible::Newest));

        // Its pulls waiting, then uploading, then done.
        q.add(Item::new(
            Origin::Backlog,
            log.clone(),
            segs[0].clone(),
            Visibility::Public,
            "eu",
        ));
        assert_eq!(block(&q, None, &later), Some(NotEligible::Queued));
        q.get_mut(&segs[0].sha256).unwrap().state = State::Uploading;
        assert_eq!(block(&q, None, &later), Some(NotEligible::Queued));
        q.get_mut(&segs[0].sha256).unwrap().state = State::Failed;
        assert_eq!(block(&q, None, &later), None, "the player may archive it");
        assert_eq!(q.files(), std::slice::from_ref(&log));

        // Skipping the rest of it shows, while the file is the same size.
        let mut skips = Skips::default();
        let size = std::fs::metadata(&log).unwrap().len();
        skips.set(&log, size, true);
        let with = ArchiveCheck {
            newest: None,
            now: later.now,
            skips: Some(&skips),
        };
        let v = backlog_view(&b, &q, &HashSet::new(), log.parent(), None, &with);
        assert!(v.files[0].skipped);
        skips.set(&log, size + 1, true);
        let with = ArchiveCheck {
            newest: None,
            now: later.now,
            skips: Some(&skips),
        };
        let v = backlog_view(&b, &q, &HashSet::new(), log.parent(), None, &with);
        assert!(!v.files[0].skipped, "the log changed since");

        // Not directly in the Logs folder (another tool's archive folder).
        let v = backlog_view(
            &b,
            &q,
            &HashSet::new(),
            Some(Path::new("C:/elsewhere")),
            None,
            &later,
        );
        assert_eq!(v.files[0].archive_block, Some(NotEligible::NotInLogs));
        // As the window reads it.
        assert_eq!(
            serde_json::to_value(NotEligible::InUse).unwrap(),
            serde_json::json!("in_use")
        );
    }

    #[test]
    fn the_archive_folder_and_the_log_being_archived() {
        let tmp = tempfile::tempdir().unwrap();
        let mut a = ArchiveState::default();
        let v = archive_view(&mut a, None);
        assert_eq!((v.folder, v.exists, v.size), (None, false, 0));
        let v = archive_view(&mut a, Some(tmp.path()));
        assert!(v.folder.unwrap().ends_with("MythicsLogsArchive"));
        assert!(!v.exists);
        let dir = archive::folder(tmp.path());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("WoWCombatLog-092126_193000.zip"), vec![0u8; 1500]).unwrap();
        // Measured at most once a minute, unless forgotten.
        assert_eq!(archive_view(&mut a, Some(tmp.path())).size, 0);
        a.size = None;
        let log = tmp.path().join("WoWCombatLog-092126_193000.txt");
        a.busy = Some((log, 300, 1200));
        let v = archive_view(&mut a, Some(tmp.path()));
        assert!(v.exists);
        assert_eq!((v.size, v.files), (1500, 1));
        let busy = v.busy.unwrap();
        assert_eq!(busy.name, "WoWCombatLog-092126_193000.txt");
        assert_eq!(busy.pct, 25);
    }

    #[test]
    fn backlog_estimates_what_each_setting_would_upload() {
        // Tonight's wipe and kill, then a second wipe of the same boss at a
        // higher health: that one is summarised.
        let worse = RAID
            .lines()
            .skip_while(|l| !l.contains("ENCOUNTER_START"))
            .take_while(|l| !l.contains("SPELL_RESURRECT"))
            .map(|l| {
                l.replace("20:0", "21:0")
                    .replace("234000000", "900000000")
                    .replace("00001A0001", "00001C0001")
                    + "\n"
            })
            .collect::<String>();
        let (_tmp, log, mut q) = setup(&format!("{RAID}{worse}"));
        let report = analyse(&describe(&log).unwrap(), |_| true).unwrap();
        let segs: Vec<_> = report.uploadable().cloned().collect();
        assert_eq!(segs.len(), 3);
        assert_eq!(segs[2].boss_hp_pct, Some(90.0));
        let b = Backlog {
            reports: vec![report],
            ..Default::default()
        };
        let v = backlog_view(&b, &q, &HashSet::new(), log.parent(), None, &no_check());
        let f = &v.files[0];
        assert_eq!(f.summaries, 1);
        let ratio = mythics_logger_core::chunker::BACKLOG_RATIO;
        let bytes = |s: &[&mythics_logger_core::splitter::Segment]| {
            (s.iter().map(|s| s.size).sum::<u64>() as f64 / ratio) as u64
        };
        assert_eq!(
            f.estimate_best,
            bytes(&[&segs[0], &segs[1]]) + mythics_logger_core::chunker::SUMMARY_BYTES
        );
        assert_eq!(f.estimate_all, bytes(&[&segs[0], &segs[1], &segs[2]]));
        // Once they're queued, nothing's left to estimate.
        for s in &segs {
            q.add(Item::new(
                Origin::Backlog,
                log.clone(),
                s.clone(),
                Visibility::Public,
                "eu",
            ));
        }
        let v = backlog_view(&b, &q, &HashSet::new(), log.parent(), None, &no_check());
        assert_eq!((v.files[0].estimate_best, v.files[0].summaries), (0, 0));
    }

    #[test]
    fn history_shows_a_summarised_wipe() {
        let (_tmp, log, mut q) = setup(RAID);
        let mut segs = Vec::new();
        split_file(&log, |s| segs.push(s), |_| true).unwrap();
        let wipe = segs[0].clone();
        q.add(Item::summarised_wipe(
            log.clone(),
            wipe.clone(),
            Visibility::Public,
            "eu",
        ));
        let it = q.get_mut(&wipe.sha256).unwrap();
        it.state = State::Done;
        it.upload_id = Some("70".into());
        it.summarised = Some(mythics_logger_core::queue::Already {
            fight_id: Some("90".into()),
            url: Some("/logs/5/pulls/90/".into()),
            log_url: Some("/logs/5/".into()),
            boss_url: Some("/logs/5/bosses/3129/".into()),
        });
        // Not listed by the server yet: the app's own record.
        let rows = history_rows(&[], q.items());
        let row = &rows[0];
        assert!(row.summary && !row.already);
        assert_eq!(row.boss_hp_pct, Some(23.4));
        assert_eq!(row.status.as_deref(), Some("parsed"));
        assert_eq!(row.log.page_url.as_deref(), Some("/logs/5/pulls/90/"));
        // Listed: the server's word.
        let server: Vec<UploadRow> = serde_json::from_value(serde_json::json!([
            {"id": 70, "kind": "summary", "status": "parsed", "size": 0, "session_id": 5,
             "log_url": "/logs/5/", "fights": [
                {"id": 90, "kind": "encounter", "encounter_id": 3129, "name": "Plexus Sentinel",
                 "difficulty": 16, "kill": false, "boss_hp_pct": 23.4, "summary": true,
                 "section": "raid", "url": "/logs/5/pulls/90/", "boss_url": "/logs/5/bosses/3129/"}]}
        ]))
        .unwrap();
        let rows = history_rows(&server, &[]);
        assert!(rows[0].summary);
        assert_eq!(rows[0].success, Some(false));
        assert_eq!(rows[0].boss_hp_pct, Some(23.4));
    }

    #[test]
    fn history_merges_the_servers_list_with_the_apps_own() {
        let (_tmp, log, mut q) = setup(RAID);
        let mut segs = Vec::new();
        split_file(&log, |s| segs.push(s), |_| true).unwrap();
        for (i, s) in segs.into_iter().enumerate() {
            let sha = s.sha256.clone();
            q.add(Item::new(
                Origin::Live,
                log.clone(),
                s,
                Visibility::Public,
                "eu",
            ));
            let it = q.get_mut(&sha).unwrap();
            it.state = State::Done;
            it.upload_id = Some(format!("u{i}"));
        }
        let server: Vec<UploadRow> = serde_json::from_value(serde_json::json!([
            {"id": "u0", "visibility": "private", "start_time": "2026-09-28T20:05:00+01:00"}
        ]))
        .unwrap();
        let rows = history_rows(&server, q.items());
        assert_eq!(rows.len(), 2);
        // Both from one file: one log.
        assert_eq!(rows[0].log.group, rows[1].log.group);
        let on = rows.iter().find(|r| r.on_server).unwrap();
        assert_eq!(on.name.as_deref(), Some("Plexus Sentinel"));
        assert_eq!(on.visibility, Some(Visibility::Private));
        assert!(rows
            .iter()
            .any(|r| !r.on_server && r.id.as_deref() == Some("u1")));
    }

    #[test]
    fn a_pull_already_on_the_site_links_to_the_raid_members_copy() {
        let (_tmp, log, mut q) = setup(RAID);
        let mut segs = Vec::new();
        split_file(&log, |s| segs.push(s), |_| true).unwrap();
        let sha = segs[0].sha256.clone();
        for s in segs {
            q.add(Item::new(
                Origin::Live,
                log.clone(),
                s,
                Visibility::Public,
                "eu",
            ));
        }
        let it = q.get_mut(&sha).unwrap();
        it.state = State::Done;
        it.already = Some(mythics_logger_core::queue::Already {
            fight_id: Some("31".into()),
            url: Some("/logs/3/pulls/31/".into()),
            log_url: Some("/logs/3/".into()),
            boss_url: Some("/logs/3/bosses/3129/".into()),
        });
        let pulls = tonights_pulls(q.items(), &HashMap::new(), &HashMap::new(), now_ms());
        let skipped = pulls.iter().find(|p| p.sha256 == sha).unwrap();
        assert!(skipped.already);
        assert_eq!(skipped.server_status.as_deref(), Some("parsed"));
        assert_eq!(skipped.log.page_url.as_deref(), Some("/logs/3/pulls/31/"));
        assert_eq!(
            skipped.log.boss_url.as_deref(),
            Some("/logs/3/bosses/3129/")
        );
        // Still in this file's log, beside the pull that was sent.
        assert_eq!(skipped.log.group, pulls[0].log.group);
        // Nothing to poll the server about.
        assert!(awaiting_parse(q.items(), &HashMap::new(), now_ms()).is_empty());
        let rows = history_rows(&[], q.items());
        let row = rows
            .iter()
            .find(|r| r.sha256.as_deref() == Some(&sha))
            .unwrap();
        assert!(row.already && !row.on_server);
        assert_eq!(row.log.log_url.as_deref(), Some("/logs/3/"));
    }

    #[test]
    fn history_carries_the_servers_status_page_and_pulls() {
        // An upload from another computer: the name comes from what the
        // server found in it.
        let server: Vec<UploadRow> = serde_json::from_value(serde_json::json!([
            {"id": 41, "status": "parsed", "kind": "key", "key_level": 14, "session_id": 5,
             "log_url": "/logs/5/", "start_time": "2026-09-27T20:39:19+00:00", "fights": [
                {"id": 7, "kind": "key", "name": "The Blinding Vale", "key_level": 14,
                 "kill": true, "section": "mplus", "url": "/logs/5/keys/7/"},
                {"id": 8, "kind": "encounter", "name": "Lightblossom Trinity", "difficulty": 8,
                 "kill": true, "parent_id": 7, "in_key": 7, "section": "mplus",
                 "url": "/logs/5/pulls/8/"}]},
            {"id": 42, "status": "queued", "session_id": 5, "start_time": "2026-09-27T21:00:00+00:00"},
            {"id": 43, "status": "queued", "session_id": 6, "start_time": "2026-09-26T21:00:00+00:00"}
        ]))
        .unwrap();
        let rows = history_rows(&server, &[]);
        let queued = &rows[0];
        assert_eq!(queued.status.as_deref(), Some("queued"));
        assert_eq!(queued.log.log_url, None);
        assert!(queued.fights.is_empty());
        let key = &rows[1];
        assert_eq!(key.name.as_deref(), Some("The Blinding Vale"));
        assert_eq!(key.success, Some(true));
        assert_eq!(key.log.log_url.as_deref(), Some("/logs/5/"));
        assert_eq!(key.log.page_url.as_deref(), Some("/logs/5/keys/7/"));
        assert_eq!(key.log.section.as_deref(), Some("mplus"));
        assert_eq!(key.fights.len(), 2);
        // Grouped by the server's session when this app didn't send them.
        assert_eq!(queued.log.group, "s:5");
        assert_eq!(key.log.group, "s:5");
        assert_eq!(rows[2].log.group, "s:6");
    }
}
