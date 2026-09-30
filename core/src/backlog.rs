//! Past logs: finding them, and reading each one's pulls and keys before
//! anything is sent.
//!
//! - Finds every `WoWCombatLog*.txt` in the Logs folder and one level of
//!   folders under it (Warcraft Logs' and Raider.IO's archive folders keep
//!   old logs there), plus any files the player chooses.
//! - Reads each file once, streaming it (flat memory, multi-GB files fine),
//!   and caches the result by path, size and modification time, so the list
//!   comes back instantly next time.
//! - Past logs never count as live: they go in the queue as `Backlog`.
//!
//! Zipped archives aren't read yet.

use crate::header::Header;
use crate::splitter::{self, Kind, Segment};
use crate::tailer::is_combat_log_name;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogFile {
    pub path: PathBuf,
    pub name: String,
    pub size: u64,
    pub modified_ms: u64,
}

/// Every combat log in `logs_dir` and its immediate subfolders, newest first.
pub fn find_logs(logs_dir: &Path) -> Vec<LogFile> {
    let mut out = Vec::new();
    collect(logs_dir, 1, &mut out);
    out.sort_by(|a, b| b.modified_ms.cmp(&a.modified_ms).then(a.name.cmp(&b.name)));
    out
}

fn collect(dir: &Path, depth: u32, out: &mut Vec<LogFile>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let Ok(meta) = e.metadata() else { continue };
        let path = e.path();
        if meta.is_dir() {
            if depth > 0 {
                collect(&path, depth - 1, out);
            }
            continue;
        }
        let name = e.file_name().to_string_lossy().into_owned();
        if !is_combat_log_name(&name) {
            continue;
        }
        out.push(LogFile {
            path,
            name,
            size: meta.len(),
            modified_ms: modified_ms(&meta),
        });
    }
}

fn modified_ms(meta: &std::fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as u64)
}

/// A describable file, ready for the player to tick.
pub fn describe(path: &Path) -> std::io::Result<LogFile> {
    let meta = std::fs::metadata(path)?;
    Ok(LogFile {
        path: path.to_path_buf(),
        name: path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        size: meta.len(),
        modified_ms: modified_ms(&meta),
    })
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileReport {
    pub file: LogFile,
    /// The first timestamp in the file, as ISO 8601 (the year from the file's
    /// name for old logs).
    pub first_time: Option<String>,
    pub header: Option<HeaderInfo>,
    pub segments: Vec<Segment>,
    /// Boss pulls, including those inside keys.
    pub encounters: u32,
    pub keys: u32,
    /// False if reading stopped early (cancelled).
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HeaderInfo {
    pub version: Option<u32>,
    pub advanced: Option<bool>,
    pub build: Option<String>,
}

impl From<&Header> for HeaderInfo {
    fn from(h: &Header) -> Self {
        Self {
            version: h.version,
            advanced: h.advanced,
            build: h.build.clone(),
        }
    }
}

/// Reads a file's pulls and keys. `on_progress(bytes read)` may return false
/// to stop.
pub fn analyse(
    file: &LogFile,
    mut on_progress: impl FnMut(u64) -> bool,
) -> std::io::Result<FileReport> {
    let year_hint = crate::timestamp::year_from_file_name(&file.name);
    let mut segments = Vec::new();
    let mut first_time: Option<String> = None;
    let mut stopped = false;
    let splitter = splitter::split_file(
        &file.path,
        |s| segments.push(s),
        |n| {
            let go = on_progress(n);
            stopped |= !go;
            go
        },
    )?;
    if let Some(s) = segments.first() {
        first_time = Some(s.start_iso(year_hint));
    }
    if first_time.is_none() {
        first_time = first_timestamp(&file.path).map(|t| t.to_iso(year_hint));
    }
    Ok(FileReport {
        file: file.clone(),
        first_time,
        header: splitter.header().map(HeaderInfo::from),
        segments,
        encounters: splitter.encounters_seen,
        keys: splitter.keys_seen,
        complete: !stopped,
    })
}

fn first_timestamp(path: &Path) -> Option<crate::timestamp::Timestamp> {
    use std::io::Read;
    let mut f = crate::tailer::open_shared(path).ok()?;
    let mut head = [0u8; 256];
    let n = f.read(&mut head).ok()?;
    let line = crate::line::parse(&head[..n])?;
    crate::timestamp::parse(line.timestamp)
}

impl FileReport {
    /// Pulls and keys worth sending: complete ones, plus unfinished ones that
    /// ran for a while (a pull cut short by a disconnect still has value).
    pub fn uploadable(&self) -> impl Iterator<Item = &Segment> {
        self.segments
            .iter()
            .filter(|s| s.kind != Kind::Segment || s.size > 64 * 1024)
    }
}

/// What the reports hold changed: 2 has the server's boss health for every
/// pull (`crate::bosshp`), which picks each boss's best wipe; 3 carries each
/// segment's zone line (`crate::splitter`), so a past log read by an older
/// app is read again rather than queued without it.
const CACHE_VERSION: u32 = 3;

/// Reports kept between runs, keyed by path; valid while size and
/// modification time match, and the app reads files the same way.
#[derive(Debug, Serialize, Deserialize)]
pub struct ReportCache {
    #[serde(default)]
    version: u32,
    reports: HashMap<PathBuf, FileReport>,
}

impl Default for ReportCache {
    fn default() -> Self {
        Self {
            version: CACHE_VERSION,
            reports: HashMap::new(),
        }
    }
}

impl ReportCache {
    /// The saved reports, or none if they were made by an older app.
    pub fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Self>(&b).ok())
            .filter(|c| c.version == CACHE_VERSION)
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(self)?)?;
        std::fs::rename(tmp, path)
    }

    pub fn get(&self, f: &LogFile) -> Option<&FileReport> {
        self.reports
            .get(&f.path)
            .filter(|r| r.complete && r.file.size == f.size && r.file.modified_ms == f.modified_ms)
    }

    pub fn put(&mut self, r: FileReport) {
        if r.complete {
            self.reports.insert(r.file.path.clone(), r);
        }
    }

    /// Forgets files that are gone.
    pub fn retain_existing(&mut self) {
        self.reports.retain(|p, _| p.exists());
    }
}
