//! The upload queue, kept on disk so nothing is lost when the network drops,
//! mythics.gg is down, or the app is closed mid-upload.
//!
//! `queue.json` in the app's data folder holds one item per segment, keyed by
//! its SHA-256; a segment already in the queue (or already uploaded) is never
//! added twice. Compressed chunks wait in `chunks/<sha256>/` until their
//! upload completes. A live pull's chunks are written as soon as the pull
//! ends, so they survive another program clearing the log; a past log's are
//! written just before it uploads, so a big backlog doesn't fill the disk.
//!
//! Live pulls always go first. Past logs go in the order they were added,
//! and can be paused.

use crate::api::Visibility;
use crate::splitter::Segment;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Origin {
    Live,
    /// A past log from the Backlog tab. Never counts as live.
    Backlog,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Waiting,
    Uploading,
    Done,
    /// Refused by the server, or the file is gone. Kept so it isn't re-added.
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedChunks {
    pub chunk_size: u64,
    pub chunks: u32,
}

/// The site already had this pull or key, uploaded by a raid member: the app
/// asked with its fingerprint and sent nothing (`docs/specs/logger-api.md`,
/// "Ask before uploading"). The pages are that copy's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Already {
    pub fight_id: Option<String>,
    pub url: Option<String>,
    pub log_url: Option<String>,
    pub boss_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub sha256: String,
    pub origin: Origin,
    pub file: PathBuf,
    pub segment: Segment,
    /// For old logs whose lines have no year.
    pub year_hint: Option<i32>,
    pub visibility: Visibility,
    pub region: String,
    pub state: State,
    pub prepared: Option<PreparedChunks>,
    pub upload_id: Option<String>,
    pub chunks_sent: u32,
    pub attempts: u32,
    pub next_try_ms: u64,
    /// A refusal code or a short reason in our words; never a server message.
    pub error: Option<String>,
    pub added_ms: u64,
    pub done_ms: Option<u64>,
    /// The log session: one key per file (`crate::session`). Items saved
    /// before sessions have none, and go as logs of their own.
    #[serde(default)]
    pub session_key: Option<String>,
    /// The file's name, never its path.
    #[serde(default)]
    pub file_name: Option<String>,
    #[serde(default)]
    pub file_start: Option<String>,
    /// The server was asked whether it has this pull already.
    #[serde(default)]
    pub fingerprint_asked: bool,
    /// It had: nothing was sent (state `Done`).
    #[serde(default)]
    pub already: Option<Already>,
    /// A past log's wipe sent as a pull summary, not in full (`crate::plan`).
    #[serde(default)]
    pub summary: bool,
    /// The summary's pages, once the server has stored it (state `Done`).
    #[serde(default)]
    pub summarised: Option<Already>,
}

impl Item {
    pub fn new(
        origin: Origin,
        file: PathBuf,
        segment: Segment,
        visibility: Visibility,
        region: &str,
    ) -> Self {
        let year_hint = file
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(crate::timestamp::year_from_file_name);
        let session = crate::session::of_file(&file, year_hint);
        Self {
            sha256: segment.sha256.clone(),
            origin,
            file,
            segment,
            year_hint,
            visibility,
            region: region.to_string(),
            state: State::Waiting,
            prepared: None,
            upload_id: None,
            chunks_sent: 0,
            attempts: 0,
            next_try_ms: 0,
            error: None,
            added_ms: now_ms(),
            done_ms: None,
            session_key: session.as_ref().map(|s| s.key.clone()),
            file_name: session.as_ref().map(|s| s.file_name.clone()),
            file_start: session.and_then(|s| s.start),
            fingerprint_asked: false,
            already: None,
            summary: false,
            summarised: None,
        }
    }

    /// A past log's wipe, to go as a pull summary.
    pub fn summarised_wipe(
        file: PathBuf,
        segment: Segment,
        visibility: Visibility,
        region: &str,
    ) -> Self {
        Self {
            summary: true,
            ..Self::new(Origin::Backlog, file, segment, visibility, region)
        }
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct OnDisk {
    #[serde(default)]
    items: Vec<Item>,
    #[serde(default)]
    backlog_paused: bool,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct Counts {
    pub live_waiting: u32,
    pub backlog_waiting: u32,
    pub backlog_done: u32,
    pub backlog_total: u32,
    pub failed: u32,
    pub done: u32,
}

pub struct Queue {
    path: PathBuf,
    chunks_root: PathBuf,
    items: Vec<Item>,
    pub backlog_paused: bool,
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis() as u64
}

/// Waits between tries: 5 s, 10 s, 20 s … up to 10 minutes, with a little
/// jitter so a crowd of apps doesn't come back at once after an outage.
pub fn backoff_ms(attempts: u32) -> u64 {
    let base = 5_000u64.saturating_mul(1 << attempts.min(10)).min(600_000);
    let jitter = rand::random::<u64>() % (base / 5 + 1);
    base + jitter
}

impl Queue {
    /// Loads the queue from `dir` (the app's data folder), or starts empty.
    /// Anything left "uploading" by a crash goes back to waiting.
    pub fn load(dir: &Path) -> Self {
        let path = dir.join("queue.json");
        let on_disk: OnDisk = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let mut items = on_disk.items;
        for it in &mut items {
            if it.state == State::Uploading {
                it.state = State::Waiting;
            }
        }
        Self {
            path,
            chunks_root: dir.join("chunks"),
            items,
            backlog_paused: on_disk.backlog_paused,
        }
    }

    /// Writes to a temporary file and renames it over the old one, so a crash
    /// mid-write leaves the last good queue.
    pub fn save(&self) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        let body = serde_json::to_vec(&OnDisk {
            items: self.items.clone(),
            backlog_paused: self.backlog_paused,
        })?;
        std::fs::write(&tmp, body)?;
        std::fs::rename(&tmp, &self.path)
    }

    pub fn items(&self) -> &[Item] {
        &self.items
    }

    pub fn get(&self, sha: &str) -> Option<&Item> {
        self.items.iter().find(|i| i.sha256 == sha)
    }

    pub fn get_mut(&mut self, sha: &str) -> Option<&mut Item> {
        self.items.iter_mut().find(|i| i.sha256 == sha)
    }

    /// Whether any pull or key from `file` is still waiting or uploading:
    /// such a log isn't archived (`crate::archive`).
    pub fn has_pending(&self, file: &Path) -> bool {
        self.items
            .iter()
            .any(|i| i.file == file && matches!(i.state, State::Waiting | State::Uploading))
    }

    /// Every log file with pulls in the queue, in any state: the logs
    /// "Archive logs once uploaded" looks at (`crate::archive::ready_to_archive`).
    pub fn files(&self) -> Vec<PathBuf> {
        let mut files: Vec<PathBuf> = self.items.iter().map(|i| i.file.clone()).collect();
        files.sort();
        files.dedup();
        files
    }

    pub fn contains(&self, sha: &str) -> bool {
        self.get(sha).is_some()
    }

    pub fn chunk_dir(&self, sha: &str) -> PathBuf {
        self.chunks_root.join(sha)
    }

    /// Adds an item unless one with the same SHA-256 is already there.
    pub fn add(&mut self, item: Item) -> bool {
        if self.contains(&item.sha256) {
            return false;
        }
        self.items.push(item);
        true
    }

    /// The next item to send: live first, then past logs (unless paused),
    /// each in the order added, skipping any waiting out a backoff.
    pub fn next_ready(&self, now_ms: u64) -> Option<&Item> {
        let ready = |i: &&Item| {
            matches!(i.state, State::Waiting | State::Uploading) && i.next_try_ms <= now_ms
        };
        self.items
            .iter()
            .filter(ready)
            .find(|i| i.origin == Origin::Live)
            .or_else(|| {
                if self.backlog_paused {
                    None
                } else {
                    self.items
                        .iter()
                        .filter(ready)
                        .find(|i| i.origin == Origin::Backlog)
                }
            })
    }

    /// When the next backed-off item will be ready, if nothing is ready now.
    pub fn next_wake_ms(&self) -> Option<u64> {
        self.items
            .iter()
            .filter(|i| i.state == State::Waiting)
            .filter(|i| i.origin == Origin::Live || !self.backlog_paused)
            .map(|i| i.next_try_ms)
            .min()
    }

    pub fn counts(&self) -> Counts {
        let mut c = Counts::default();
        for i in &self.items {
            let waiting = matches!(i.state, State::Waiting | State::Uploading);
            match (i.origin, waiting) {
                (Origin::Live, true) => c.live_waiting += 1,
                (Origin::Backlog, true) => c.backlog_waiting += 1,
                _ => {}
            }
            if i.origin == Origin::Backlog {
                c.backlog_total += 1;
                if i.state == State::Done {
                    c.backlog_done += 1;
                }
            }
            match i.state {
                State::Failed => c.failed += 1,
                State::Done => c.done += 1,
                _ => {}
            }
        }
        c
    }

    /// Drops the chunks of anything finished, and done items older than
    /// `keep` from the list (their hashes are still known to the server, which
    /// de-duplicates on them).
    pub fn tidy(&mut self, keep: Duration) {
        let cutoff = now_ms().saturating_sub(keep.as_millis() as u64);
        let root = self.chunks_root.clone();
        self.items.retain(|i| {
            if matches!(i.state, State::Done | State::Failed) {
                let _ = crate::chunker::clear_dir(&root.join(&i.sha256));
            }
            !(i.state == State::Done && i.done_ms.is_some_and(|d| d < cutoff))
        });
    }

    /// Forgets a log's waiting past-log pulls: the player skipped the rest of
    /// it. One already uploading finishes. Returns how many went.
    pub fn remove_waiting_of(&mut self, file: &Path) -> u32 {
        let shas: Vec<String> = self
            .items
            .iter()
            .filter(|i| i.file == file && i.state == State::Waiting && i.origin == Origin::Backlog)
            .map(|i| i.sha256.clone())
            .collect();
        shas.iter().filter(|s| self.remove_waiting(s)).count() as u32
    }

    /// Forgets waiting past-log items the player took out of the list.
    pub fn remove_waiting(&mut self, sha: &str) -> bool {
        let before = self.items.len();
        let root = self.chunks_root.clone();
        self.items.retain(|i| {
            let drop = i.sha256 == sha && i.state == State::Waiting && i.origin == Origin::Backlog;
            if drop {
                let _ = crate::chunker::clear_dir(&root.join(&i.sha256));
            }
            !drop
        });
        self.items.len() != before
    }
}
