//! The background jobs: following the live log, sending the queue, and
//! asking the server whether what was sent has been parsed yet.

use crate::state::{AppState, LiveStatus};
use mythics_logger_core::queue::{Item, Origin};
use mythics_logger_core::splitter::{Kind, Segment, Splitter};
use mythics_logger_core::tailer::{is_combat_log_name, live_start, Event, Tailer};
use mythics_logger_core::uploader::{self, Step};
use mythics_logger_core::wowdir;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tauri::{AppHandle, Emitter};

/// Tells the window something changed; it asks for a fresh snapshot.
pub fn changed(app: &AppHandle) {
    let _ = app.emit("changed", ());
}

/// Where the tailer stopped in each file, so a restart carries on from there
/// without losing a pull that was under way.
#[derive(Default, Serialize, Deserialize)]
struct TailState {
    offsets: HashMap<PathBuf, u64>,
}

impl TailState {
    fn load(p: &Path) -> Self {
        std::fs::read(p)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }
    fn save(&self, p: &Path) {
        let tmp = p.with_extension("json.tmp");
        if serde_json::to_vec(self)
            .ok()
            .and_then(|b| std::fs::write(&tmp, b).ok())
            .is_some()
        {
            let _ = std::fs::rename(tmp, p);
        }
    }
}

/// An unfinished pull or key smaller than this (a few seconds of a reset
/// or a disconnect) isn't worth a slot on the server.
const MIN_UNFINISHED: u64 = 64 * 1024;

fn worth_sending(s: &Segment) -> bool {
    s.kind != Kind::Segment || s.size > MIN_UNFINISHED
}

/// What one poll of the live log found.
#[derive(Default)]
pub struct Polled {
    /// Each finished pull or key with the file its offsets refer to: when
    /// logging restarts, the old file's last pull arrives in the same poll as
    /// the new file opens.
    pub segments: Vec<(PathBuf, Segment)>,
    /// Any new line at all.
    pub activity: bool,
    /// The file being followed.
    pub file: Option<PathBuf>,
}

/// Follows the newest combat log in a folder while live logging is on, and
/// holds nothing open while it's off.
pub struct Follower {
    tail_path: PathBuf,
    offsets: TailState,
    tailer: Option<Tailer>,
    splitter: Splitter,
    last_saved: Option<(PathBuf, u64)>,
    /// The setting at the last tick; `None` before the first.
    was_on: Option<bool>,
    /// Switched on while the app was running: every log already in the
    /// folder is followed from its end, so nothing from before the switch
    /// goes (the Backlog tab is for that).
    from_end: bool,
}

impl Follower {
    pub fn new(tail_path: PathBuf) -> Self {
        Self {
            offsets: TailState::load(&tail_path),
            tail_path,
            tailer: None,
            splitter: Splitter::new(),
            last_saved: None,
            was_on: None,
            from_end: false,
        }
    }

    /// Whether a combat log is open for following.
    #[cfg(test)]
    pub fn is_following(&self) -> bool {
        self.tailer.is_some()
    }

    pub fn splitter(&self) -> &Splitter {
        &self.splitter
    }

    /// One tick with the live logging setting. Off, it lets go of the file
    /// and reads nothing (`None`). On at start-up it carries on from its
    /// saved place, as before the switch existed; switched on while running,
    /// it starts from the end of what's there.
    pub fn tick(&mut self, on: bool, dir: &Path, now: SystemTime) -> Option<Polled> {
        match (self.was_on, on) {
            (_, false) => {
                self.tailer = None;
                self.splitter = Splitter::new();
                self.last_saved = None;
            }
            (Some(false), true) => self.from_end = true,
            _ => {}
        }
        self.was_on = Some(on);
        on.then(|| self.poll(dir, now))
    }

    fn poll(&mut self, dir: &Path, now: SystemTime) -> Polled {
        if self.tailer.as_ref().map(|t| t.dir()) != Some(dir) {
            self.tailer = Some(Tailer::new(dir));
            self.splitter = Splitter::new();
        }
        if std::mem::take(&mut self.from_end) {
            // A saved place is from the last time it was on: the gap since
            // then was never meant to go.
            self.offsets.offsets = log_ends(dir);
            self.offsets.save(&self.tail_path);
            log::info!(
                "live logging switched on: following {} logs from their end",
                self.offsets.offsets.len()
            );
        }
        let t = self.tailer.as_mut().expect("tailer");
        let offsets = &self.offsets;
        let splitter = &mut self.splitter;

        let mut out = Polled::default();
        let mut reading: Option<PathBuf> = t.current_path().map(Path::to_path_buf);
        let result = t.poll(
            &mut |path, len, mtime| {
                offsets
                    .offsets
                    .get(path)
                    .copied()
                    .filter(|&o| o <= len)
                    .unwrap_or_else(|| live_start(len, mtime, now))
            },
            &mut |e| match e {
                Event::Opened {
                    path, header, zone, ..
                } => {
                    reading = Some(path.to_path_buf());
                    // Opened part-way: carry on as if read from the start,
                    // with its header and its zone (`Splitter::resuming`).
                    *splitter = Splitter::resuming(header.as_deref(), zone.as_deref());
                }
                Event::Line { offset, bytes } => {
                    out.activity = true;
                    if let (Some(seg), Some(p)) = (splitter.feed(offset, bytes), &reading) {
                        out.segments.push((p.clone(), seg));
                    }
                }
                Event::Restarted { .. } => *splitter = Splitter::new(),
                Event::Closed { path, .. } => {
                    if let Some(seg) = splitter.finish() {
                        out.segments.push((path.to_path_buf(), seg));
                    }
                    *splitter = Splitter::new();
                }
            },
        );
        if let Err(e) = result {
            log::warn!("couldn't read the Logs folder: {}", e.kind());
        }
        out.file = t.current_path().map(Path::to_path_buf);

        // Save our place: the start of an open pull, else where reading stopped.
        if let Some((path, pos)) = t.position() {
            let at = self.splitter.open_start().unwrap_or(pos);
            let key = (path.to_path_buf(), at);
            if self.last_saved.as_ref() != Some(&key) {
                self.offsets.offsets.insert(key.0.clone(), at);
                self.offsets.offsets.retain(|p, _| p.exists());
                self.offsets.save(&self.tail_path);
                self.last_saved = Some(key);
            }
        }
        out
    }
}

/// Every combat log in the folder with its length now: its metadata only,
/// never its contents.
fn log_ends(dir: &Path) -> HashMap<PathBuf, u64> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return HashMap::new();
    };
    entries
        .flatten()
        .filter(|e| e.file_name().to_str().is_some_and(is_combat_log_name))
        .filter_map(|e| Some((e.path(), e.metadata().ok()?.len())))
        .collect()
}

/// Follows the newest combat log while live logging is on, cutting pulls and
/// keys as they end and queueing each at once. Runs on its own thread for
/// the app's lifetime; the setting is read every tick, so switching it takes
/// effect within a second.
pub fn tail_forever(app: AppHandle, state: Arc<AppState>) {
    let mut follower = Follower::new(state.data_dir.join("tail.json"));
    let mut last_search = Instant::now() - Duration::from_secs(60);

    loop {
        let (on, dir) = {
            let s = state.settings.lock().expect("settings");
            (s.live_logging, s.logs_dir.clone())
        };
        if !on {
            // Nothing is opened or searched for: the file isn't touched.
            follower.tick(false, Path::new(""), SystemTime::now());
            let was = {
                let mut live = state.live.lock().expect("live");
                let was = live.status;
                *live = LiveStatus {
                    status: "off",
                    logs_dir: dir.map(|d| d.to_string_lossy().into_owned()),
                    ..Default::default()
                };
                was
            };
            if was != "off" {
                log::info!("live logging is off");
                changed(&app);
            }
            std::thread::sleep(Duration::from_secs(1));
            continue;
        }
        let dir = match dir {
            Some(d) if d.is_dir() => Some(d),
            _ if last_search.elapsed() > Duration::from_secs(30) => {
                last_search = Instant::now();
                let found = wowdir::find(None);
                if let Some(f) = &found {
                    let mut s = state.settings.lock().expect("settings");
                    s.logs_dir = Some(f.clone());
                    let _ = s.save(&state.settings_path());
                    log::info!("found the Logs folder");
                }
                found
            }
            _ => None,
        };
        let Some(dir) = dir else {
            set_status(&state, "searching", None);
            changed(&app);
            std::thread::sleep(Duration::from_secs(2));
            continue;
        };
        let Some(polled) = follower.tick(true, &dir, SystemTime::now()) else {
            continue;
        };

        let got_new = !polled.segments.is_empty();
        for (seg_file, seg) in polled
            .segments
            .into_iter()
            .filter(|(_, s)| worth_sending(s))
        {
            queue_live(&state, seg_file, seg);
        }

        let was = {
            let splitter = follower.splitter();
            let mut live = state.live.lock().expect("live");
            let was = live.status;
            live.status = if polled.file.is_some() {
                "live"
            } else {
                "waiting"
            };
            live.logs_dir = Some(dir.to_string_lossy().into_owned());
            live.file = polled
                .file
                .as_ref()
                .and_then(|f| f.file_name())
                .map(|n| n.to_string_lossy().into_owned());
            if let Some((z, d)) = splitter.zone() {
                live.zone = Some(z.to_string());
                live.difficulty = Some(d);
            }
            live.advanced = splitter.header().and_then(|h| h.advanced);
            live.current = splitter.current();
            if polled.activity {
                live.last_activity_ms = mythics_logger_core::queue::now_ms();
            }
            was
        };

        if polled.activity || got_new || was == "off" {
            changed(&app);
        }
        if got_new {
            state.wake.notify_one();
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn set_status(state: &AppState, status: &'static str, dir: Option<String>) {
    let mut live = state.live.lock().expect("live");
    live.status = status;
    live.logs_dir = dir;
}

fn queue_live(state: &AppState, file: PathBuf, seg: Segment) {
    let (vis, region) = {
        let s = state.settings.lock().expect("settings");
        (s.default_visibility, s.region())
    };
    let sha = seg.sha256.clone();
    let added = {
        let mut q = state.queue.lock().expect("queue");
        let added = q.add(Item::new(Origin::Live, file, seg, vis, &region));
        let _ = q.save();
        added
    };
    if added {
        log::info!("queued a live segment");
        // Compress now, while the file is certainly there.
        if let Err(e) = uploader::prepare_now(&state.queue, &sha) {
            log::warn!("couldn't compress a live segment: {e}");
        }
    }
}

/// Sends the queue, one item at a time, for the app's lifetime.
pub async fn upload_forever(app: AppHandle, state: Arc<AppState>) {
    let mut last_emit = Instant::now();
    loop {
        let api = state.api();
        if !api.has_token() {
            wait(&state, Duration::from_secs(5)).await;
            continue;
        }
        let app2 = app.clone();
        let st2 = state.clone();
        let on_progress = move |sha: &str, sent: u32, total: u32| {
            st2.progress
                .lock()
                .expect("progress")
                .insert(sha.to_string(), (sent, total));
            changed(&app2);
        };
        let step = uploader::step(&state.queue, &api, &state.throttle, &on_progress).await;
        match step {
            Step::Idle | Step::Paused => {
                let wait_ms = {
                    let q = state.queue.lock().expect("queue");
                    let now = mythics_logger_core::queue::now_ms();
                    q.next_wake_ms()
                        .map(|t| t.saturating_sub(now).clamp(500, 30_000))
                        .unwrap_or(30_000)
                };
                wait(&state, Duration::from_millis(wait_ms)).await;
            }
            Step::SignedOut => {
                log::info!("the app token was refused; logging in again is needed");
                *state.token.lock().expect("token") = None;
                crate::token::clear();
                state.signed_out_notice.store(true, Ordering::Relaxed);
                changed(&app);
            }
            Step::AlreadyThere { .. } | Step::Summarised { .. } => changed(&app),
            Step::Uploaded { ref sha256, .. } | Step::Failed { ref sha256, .. } => {
                state.progress.lock().expect("progress").remove(sha256);
                changed(&app);
            }
            Step::Retrying { ref sha256 } => {
                state.progress.lock().expect("progress").remove(sha256);
                changed(&app);
                // Don't spin when everything is backing off.
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }
        if last_emit.elapsed() > Duration::from_secs(60) {
            state
                .queue
                .lock()
                .expect("queue")
                .tidy(Duration::from_secs(90 * 86_400));
            last_emit = Instant::now();
        }
    }
}

/// The gap between asks while an upload is being processed: it doubles each
/// time nothing changed, and starts again when something did.
pub fn next_poll(current: Duration, something_changed: bool) -> Duration {
    const FIRST: Duration = Duration::from_secs(15);
    const MOST: Duration = Duration::from_secs(120);
    if something_changed {
        FIRST
    } else {
        (current * 2).clamp(FIRST, MOST)
    }
}

/// Asks the server, gently, about tonight's uploads until each is parsed (or
/// failed), so the Live tab can link to its page. One request for the newest
/// page each time, and none at all while nothing is waiting.
pub async fn poll_uploads_forever(app: AppHandle, state: Arc<AppState>) {
    let mut gap = next_poll(Duration::ZERO, true);
    loop {
        tokio::time::sleep(gap).await;
        let api = state.api();
        if !api.has_token() {
            continue;
        }
        let waiting = {
            let q = state.queue.lock().expect("queue");
            let known = state.server_uploads.lock().expect("server uploads");
            crate::state::awaiting_parse(q.items(), &known, mythics_logger_core::queue::now_ms())
        };
        if waiting.is_empty() {
            gap = next_poll(Duration::ZERO, true);
            continue;
        }
        let Ok(rows) = api.recent_uploads(100).await else {
            gap = next_poll(gap, false);
            continue;
        };
        let moved = {
            let known = state.server_uploads.lock().expect("server uploads");
            rows.iter().any(|r| {
                waiting.contains(&r.id) && known.get(&r.id).map(|k| &k.status) != Some(&r.status)
            })
        };
        crate::commands::remember(&state, &rows);
        if moved {
            changed(&app);
        }
        gap = next_poll(gap, moved);
    }
}

async fn wait(state: &AppState, d: Duration) {
    let _ = tokio::time::timeout(d, state.wake.notified()).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Two Mythic Plexus Sentinel pulls, a wipe and a kill (fake players).
    const RAID_NIGHT: &str = include_str!("../../core/tests/fixtures/raid_night.txt");
    /// Its zone change on entering the raid, before the first pull, as the
    /// file has it (checked out with either line ending).
    fn raid_zone() -> Option<&'static str> {
        let l = RAID_NIGHT.split_inclusive('\n').nth(1).unwrap();
        assert!(l.contains("  ZONE_CHANGE,2810,"));
        Some(l)
    }
    /// A third pull, after the ones above.
    const PULL_THREE: &str = "9/28/2026 20:30:00.0001  ENCOUNTER_START,3129,\"Plexus Sentinel\",16,20,2810\n\
        9/28/2026 20:30:02.0001  SPELL_DAMAGE,Player-1403-0A000001,\"Player1-TarrenMill-EU\",0x512,0x0,Creature-0-1-2810-1-233814-00001C0001,\"Plexus Sentinel\",0x10a48,0x0,1,2,3\n\
        9/28/2026 20:33:00.0001  ENCOUNTER_END,3129,\"Plexus Sentinel\",16,20,0,180000\n";

    struct Night {
        dir: tempfile::TempDir,
        log: PathBuf,
        tail: PathBuf,
    }

    /// A Logs folder with tonight's log, written just now (so the old rule
    /// would read it from the start), and a place for `tail.json`.
    fn night() -> Night {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("WoWCombatLog-092826_200101.txt");
        std::fs::write(&log, RAID_NIGHT).unwrap();
        let tail = dir.path().join("app").join("tail.json");
        std::fs::create_dir_all(tail.parent().unwrap()).unwrap();
        Night { dir, log, tail }
    }

    fn append(p: &Path, s: &str) {
        let mut f = std::fs::OpenOptions::new().append(true).open(p).unwrap();
        f.write_all(s.as_bytes()).unwrap();
    }

    fn names(p: &Polled) -> Vec<(Kind, Option<String>, Option<bool>)> {
        p.segments
            .iter()
            .map(|(_, s)| (s.kind, s.name.clone(), s.success))
            .collect()
    }

    #[test]
    fn with_live_logging_off_the_log_is_never_opened_and_nothing_is_cut() {
        let n = night();
        let mut f = Follower::new(n.tail.clone());
        for _ in 0..3 {
            assert!(f.tick(false, n.dir.path(), SystemTime::now()).is_none());
            assert!(!f.is_following());
            append(&n.log, PULL_THREE);
        }
        assert!(!n.tail.exists(), "no place saved, so nothing was read");
        assert!(f.splitter().current().is_none());
    }

    #[test]
    fn switched_on_mid_session_it_starts_from_the_end_of_the_file() {
        let n = night();
        let mut f = Follower::new(n.tail.clone());
        assert!(f.tick(false, n.dir.path(), SystemTime::now()).is_none());
        let first = f.tick(true, n.dir.path(), SystemTime::now()).unwrap();
        assert!(f.is_following());
        assert!(
            first.segments.is_empty(),
            "tonight's two pulls came before the switch"
        );
        assert!(!first.activity);
        assert_eq!(first.file.as_deref(), Some(n.log.as_path()));

        append(&n.log, PULL_THREE);
        let next = f.tick(true, n.dir.path(), SystemTime::now()).unwrap();
        assert_eq!(
            names(&next),
            [(Kind::Encounter, Some("Plexus Sentinel".into()), Some(false))]
        );
        let (_, seg) = &next.segments[0];
        assert_eq!(seg.start_offset as usize, RAID_NIGHT.len());
        // Its raid, from the zone change long before the switch.
        assert_eq!(seg.zone_line.as_deref(), raid_zone());
    }

    #[test]
    fn switched_off_and_on_again_it_skips_what_was_written_while_off() {
        let n = night();
        let mut f = Follower::new(n.tail.clone());
        assert!(f.tick(false, n.dir.path(), SystemTime::now()).is_none());
        f.tick(true, n.dir.path(), SystemTime::now()).unwrap();
        // Off in the middle of a pull: it's dropped, and so is the next one.
        append(
            &n.log,
            &PULL_THREE[..PULL_THREE.find("9/28/2026 20:33").unwrap()],
        );
        f.tick(true, n.dir.path(), SystemTime::now()).unwrap();
        assert!(f.tick(false, n.dir.path(), SystemTime::now()).is_none());
        assert!(!f.is_following());
        append(
            &n.log,
            &PULL_THREE[PULL_THREE.find("9/28/2026 20:33").unwrap()..],
        );
        append(&n.log, PULL_THREE);

        let back = f.tick(true, n.dir.path(), SystemTime::now()).unwrap();
        assert!(back.segments.is_empty());
        append(&n.log, PULL_THREE);
        let next = f.tick(true, n.dir.path(), SystemTime::now()).unwrap();
        assert_eq!(next.segments.len(), 1);
    }

    #[test]
    fn on_at_start_up_it_carries_on_from_its_saved_place() {
        let n = night();
        // On when the app starts: a log the game is still writing is read
        // from its start, as before the switch, so a pull under way isn't lost.
        let mut f = Follower::new(n.tail.clone());
        let first = f.tick(true, n.dir.path(), SystemTime::now()).unwrap();
        assert_eq!(first.segments.len(), 2);
        drop(f);

        append(&n.log, PULL_THREE);
        let mut again = Follower::new(n.tail.clone());
        let resumed = again.tick(true, n.dir.path(), SystemTime::now()).unwrap();
        assert_eq!(resumed.segments.len(), 1, "only the pull since the restart");
        // Still named by the raid's zone change, from before the restart.
        let (_, seg) = &resumed.segments[0];
        assert_eq!(seg.zone_line.as_deref(), raid_zone());
    }

    #[test]
    fn a_log_started_after_the_switch_is_read_from_its_start() {
        let n = night();
        let mut f = Follower::new(n.tail.clone());
        f.tick(false, n.dir.path(), SystemTime::now());
        f.tick(true, n.dir.path(), SystemTime::now()).unwrap();
        // /combatlog again: the game starts a new file.
        std::thread::sleep(Duration::from_millis(20));
        let newer = n.dir.path().join("WoWCombatLog-092826_203000.txt");
        std::fs::write(
            &newer,
            format!(
                "{}{}",
                &RAID_NIGHT[..RAID_NIGHT.find('\n').unwrap() + 1],
                PULL_THREE
            ),
        )
        .unwrap();
        let p = f.tick(true, n.dir.path(), SystemTime::now()).unwrap();
        assert_eq!(p.file.as_deref(), Some(newer.as_path()));
        assert_eq!(p.segments.len(), 1);
    }

    #[test]
    fn polls_back_off_to_two_minutes_and_start_again_on_news() {
        let mut gap = next_poll(Duration::ZERO, true);
        assert_eq!(gap, Duration::from_secs(15));
        let mut seen = vec![gap.as_secs()];
        for _ in 0..5 {
            gap = next_poll(gap, false);
            seen.push(gap.as_secs());
        }
        assert_eq!(seen, [15, 30, 60, 120, 120, 120]);
        assert_eq!(next_poll(gap, true), Duration::from_secs(15));
    }
}
