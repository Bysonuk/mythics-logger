//! Archiving finished logs (this repository's issue 4, the owner's decisions
//! of 1 Oct 2026): a log the player is done with moves into
//! `Logs\MythicsLogsArchive\` as a `.zip` holding the original `.txt` under its own name, about a tenth of
//! its size. Only when the player turns "Archive logs once uploaded" on, or
//! selects Archive on a log in the Backlog tab.
//!
//! The rules, all checked here:
//!
//! - **Only finished logs.** Never the newest combat log in the folder (the
//!   one the game writes), never one changed in the last 10 minutes, never
//!   one another program has open (on Windows the app first tries to open it
//!   with no sharing at all: if anyone else has it open, that fails and the
//!   log is left alone), and never one with pulls still queued or uploading.
//! - **Only directly in the Logs folder.** Not files in other tools' archive
//!   folders, or files chosen from elsewhere.
//! - **The original goes only after a check.** The `.zip` is written in full
//!   to a temporary name, read back (its one entry's CRC-32 and size against
//!   the original's, decompressing every byte), then renamed; only then is
//!   the original deleted. While it's written the app holds the log open
//!   sharing read only, so nobody can change or delete it under us. Any
//!   failure removes the partial `.zip` and keeps the original.
//! - **A log already gone** (another tool archived it first) is skipped
//!   quietly.
//! - Streamed through one buffer (logs reach 2 GB and more; ZIP64 when
//!   needed), paced, on a background-priority thread (the caller's), so the
//!   game never notices.
//!
//! Clean-up ("Delete archived logs after", off by default) deletes only
//! `.zip` files this app made: named like a combat log, and carrying
//! [`COMMENT`] as the zip's comment.

use crate::tailer::{is_combat_log_name, newest_log};
use serde::Serialize;
use std::fs::File;
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// The archive folder, inside the Logs folder, beside Warcraft Logs'
/// `warcraftlogsarchive` and Raider.IO's `RaiderIOLogsArchive`.
pub const FOLDER: &str = "MythicsLogsArchive";

/// Every archive this app writes carries this as its zip comment: clean-up
/// deletes nothing without it.
pub const COMMENT: &str = "mythics.gg Logger archive";

/// A log changed more recently than this may still be the game's.
pub const QUIET_FOR: Duration = Duration::from_secs(10 * 60);

/// The archive folder for a Logs folder.
pub fn folder(logs_dir: &Path) -> PathBuf {
    logs_dir.join(FOLDER)
}

/// `WoWCombatLog-092826_200101.txt` is archived as
/// `WoWCombatLog-092826_200101.zip` (Windows hides `.zip`, and a name that
/// ended `.txt` would look like the log itself).
pub fn zip_name(log_name: &str) -> String {
    let stem = log_name
        .len()
        .checked_sub(4)
        .filter(|&n| log_name.is_char_boundary(n) && log_name[n..].eq_ignore_ascii_case(".txt"))
        .map_or(log_name, |n| &log_name[..n]);
    format!("{stem}.zip")
}

/// Why a log can't be archived now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NotEligible {
    /// The newest combat log in the folder: the game writes to it.
    Newest,
    /// Changed in the last 10 minutes: the game may still be writing to it.
    Recent,
    /// Some of its pulls are still waiting to upload, or uploading.
    Queued,
    /// Another program has it open.
    InUse,
    /// Not a combat log directly in the Logs folder.
    NotInLogs,
    /// It's gone (moved or deleted by another tool). Skipped quietly.
    Gone,
}

impl NotEligible {
    pub fn code(self) -> &'static str {
        match self {
            Self::Newest => "newest",
            Self::Recent => "recent",
            Self::Queued => "queued",
            Self::InUse => "in_use",
            Self::NotInLogs => "not_in_logs",
            Self::Gone => "gone",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ArchiveError {
    #[error("can't archive this log now: {0:?}")]
    NotEligible(NotEligible),
    /// An archive of that name is there already, and isn't this log.
    #[error("an archive with this name already exists")]
    Exists,
    /// The `.zip` read back didn't match the original.
    #[error("the archive didn't match the log")]
    Verify,
    /// The log changed while it was archived.
    #[error("the log changed while it was archived")]
    Changed,
    #[error("not enough disk space")]
    DiskFull,
    #[error("couldn't write the archive: {0:?}")]
    Io(io::ErrorKind),
}

impl ArchiveError {
    /// A short code for the window, which says it in words.
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotEligible(n) => n.code(),
            Self::Exists => "archive_exists",
            Self::Verify => "archive_verify",
            Self::Changed => "archive_changed",
            Self::DiskFull => "disk_full",
            Self::Io(_) => "archive_io",
        }
    }
}

impl From<io::Error> for ArchiveError {
    fn from(e: io::Error) -> Self {
        if is_sharing_violation(&e) {
            return Self::NotEligible(NotEligible::InUse);
        }
        match e.kind() {
            io::ErrorKind::StorageFull => Self::DiskFull,
            k => Self::Io(k),
        }
    }
}

impl From<zip::result::ZipError> for ArchiveError {
    fn from(e: zip::result::ZipError) -> Self {
        match e {
            zip::result::ZipError::Io(e) => e.into(),
            _ => Self::Verify,
        }
    }
}

/// Windows' "being used by another process" (32) and "locked" (33).
fn is_sharing_violation(e: &io::Error) -> bool {
    cfg!(windows) && matches!(e.raw_os_error(), Some(32 | 33))
}

/// What became of a log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Archived, and the original deleted.
    Archived {
        zip: PathBuf,
        original_size: u64,
        zip_size: u64,
    },
    /// The log was gone before it could be archived: nothing done.
    Vanished,
}

#[derive(Debug, Clone)]
pub struct Options {
    /// Bytes read and written at a time: the most of the log in memory.
    pub chunk_size: usize,
    /// Disk speed limit in bytes a second while archiving; 0 is none.
    pub bytes_per_sec: u64,
    /// Files this big or bigger get ZIP64 sizes (the plain format stops at
    /// 4 GiB; this leaves room for deflate's worst case).
    pub zip64_from: u64,
    /// How long a log must be left alone first.
    pub quiet_for: Duration,
    /// For tests only: damages the new zip before it's checked, to show a
    /// failed check keeps the original.
    #[doc(hidden)]
    pub damage_before_check: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            chunk_size: 1 << 20,
            // Gentle on a disk the game is loading from: a 2 GB log takes
            // about 40 s.
            bytes_per_sec: 50_000_000,
            zip64_from: 0xF000_0000,
            quiet_for: QUIET_FOR,
            damage_before_check: false,
        }
    }
}

/// The quick checks, from what the caller already knows: no file is opened.
/// `newest` is the newest combat log in the Logs folder; `pending` whether
/// any of the log's pulls are queued or uploading.
pub fn blocker(
    file: &Path,
    logs_dir: &Path,
    newest: Option<&Path>,
    modified: SystemTime,
    now: SystemTime,
    quiet_for: Duration,
    pending: bool,
) -> Option<NotEligible> {
    let named = file
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(is_combat_log_name);
    if !named || file.parent() != Some(logs_dir) {
        return Some(NotEligible::NotInLogs);
    }
    if newest == Some(file) {
        return Some(NotEligible::Newest);
    }
    if now.duration_since(modified).map_or(true, |d| d < quiet_for) {
        return Some(NotEligible::Recent);
    }
    if pending {
        return Some(NotEligible::Queued);
    }
    None
}

/// The quick checks against the disk now (the file's time, and the newest
/// log in its folder). Still no file is opened.
pub fn check(
    file: &Path,
    logs_dir: &Path,
    now: SystemTime,
    quiet_for: Duration,
    pending: bool,
) -> Result<(), NotEligible> {
    let meta = match std::fs::metadata(file) {
        Ok(m) if m.is_file() => m,
        Ok(_) => return Err(NotEligible::NotInLogs),
        Err(_) => return Err(NotEligible::Gone),
    };
    let newest = newest_log(logs_dir).ok().flatten().map(|(p, _, _)| p);
    let modified = meta.modified().unwrap_or(now);
    match blocker(
        file,
        logs_dir,
        newest.as_deref(),
        modified,
        now,
        quiet_for,
        pending,
    ) {
        Some(n) => Err(n),
        None => Ok(()),
    }
}

/// Opens the log with no sharing at all: it fails if anyone else (the game,
/// Warcraft Logs' uploader) has it open. Closed again at once.
fn probe_exclusive(path: &Path) -> io::Result<()> {
    let mut o = std::fs::OpenOptions::new();
    o.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        o.share_mode(0);
    }
    o.open(path).map(drop)
}

/// Opens the log to read, letting others read it too but not write, rename
/// or delete it while it's archived.
fn open_held(path: &Path) -> io::Result<File> {
    let mut o = std::fs::OpenOptions::new();
    o.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_SHARE_READ only.
        o.share_mode(0x1);
    }
    o.open(path)
}

/// Removes the partial `.zip` unless the archive succeeded.
struct Partial(Option<PathBuf>);

impl Drop for Partial {
    fn drop(&mut self) {
        if let Some(p) = self.0.take() {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// Keeps reads to `bytes_per_sec`.
struct Pace {
    rate: u64,
    start: Instant,
    bytes: u64,
}

impl Pace {
    fn new(rate: u64) -> Self {
        Self {
            rate,
            start: Instant::now(),
            bytes: 0,
        }
    }

    fn take(&mut self, n: u64) {
        if self.rate == 0 {
            return;
        }
        self.bytes += n;
        let due = Duration::from_secs_f64(self.bytes as f64 / self.rate as f64);
        if let Some(wait) = due.checked_sub(self.start.elapsed()) {
            std::thread::sleep(wait);
        }
    }
}

/// Archives one finished log from `logs_dir` into its `MythicsLogsArchive`
/// folder, then deletes the original. `pending`: some of its pulls are still
/// queued or uploading. `on_progress(bytes done, of)` is called as it goes
/// (writing, then checking: twice the log's size in all).
pub fn archive(
    file: &Path,
    logs_dir: &Path,
    opts: &Options,
    pending: bool,
    on_progress: &mut dyn FnMut(u64, u64),
) -> Result<Outcome, ArchiveError> {
    match check(file, logs_dir, SystemTime::now(), opts.quiet_for, pending) {
        Ok(()) => {}
        Err(NotEligible::Gone) => return Ok(Outcome::Vanished),
        Err(n) => return Err(ArchiveError::NotEligible(n)),
    }
    match probe_exclusive(file) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Outcome::Vanished),
        Err(e) if is_sharing_violation(&e) || e.kind() == io::ErrorKind::PermissionDenied => {
            return Err(ArchiveError::NotEligible(NotEligible::InUse))
        }
        Err(e) => return Err(e.into()),
    }
    let mut held = match open_held(file) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Outcome::Vanished),
        Err(e) => return Err(e.into()),
    };
    let before = held.metadata()?;
    let size = before.len();
    let name = file
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or(ArchiveError::NotEligible(NotEligible::NotInLogs))?
        .to_string();

    let dir = folder(logs_dir);
    std::fs::create_dir_all(&dir)?;
    let dest = dir.join(zip_name(&name));
    let total = size.saturating_mul(2);

    if dest.exists() {
        // Archived before, and the app stopped before deleting the original?
        // Then the archive there is this log, checked, and the original goes.
        let (crc, n) = crc_of(&mut held, opts, &mut |d| on_progress(d, total))?;
        if n != size {
            return Err(ArchiveError::Changed);
        }
        if !is_ours(&dest) || verify(&dest, &name, crc, size, opts, &mut |_| {}).is_err() {
            return Err(ArchiveError::Exists);
        }
        drop(held);
        return finish(file, &dest, size, None);
    }

    let tmp = dir.join(format!("{}.partial", zip_name(&name)));
    let mut partial = Partial(Some(tmp.clone()));
    let crc = write_zip(&mut held, &tmp, &name, &before, opts, &mut |d| {
        on_progress(d, total)
    })?;
    if opts.damage_before_check {
        damage(&tmp)?;
    }
    verify(&tmp, &name, crc, size, opts, &mut |d| {
        on_progress(size + d, total)
    })?;

    // Nobody could write to it while we held it; make sure.
    let after = held.metadata()?;
    if after.len() != size || after.modified().ok() != before.modified().ok() {
        return Err(ArchiveError::Changed);
    }
    std::fs::rename(&tmp, &dest)?;
    partial.0 = None;
    drop(held);
    finish(file, &dest, size, Some(&dest))
}

/// Deletes the original, now that its archive is checked. If it can't be
/// deleted (someone opened it in the moment between), the new archive goes
/// instead (`remove_on_fail`), so the log isn't kept twice.
fn finish(
    file: &Path,
    dest: &Path,
    size: u64,
    remove_on_fail: Option<&Path>,
) -> Result<Outcome, ArchiveError> {
    match std::fs::remove_file(file) {
        Ok(()) => {}
        // Gone already: its archive is checked, so nothing is lost.
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => {
            if let Some(d) = remove_on_fail {
                let _ = std::fs::remove_file(d);
            }
            return Err(
                if is_sharing_violation(&e) || e.kind() == io::ErrorKind::PermissionDenied {
                    ArchiveError::NotEligible(NotEligible::InUse)
                } else {
                    e.into()
                },
            );
        }
    }
    let zip_size = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
    log::info!("archived a log: {size} bytes to {zip_size}");
    Ok(Outcome::Archived {
        zip: dest.to_path_buf(),
        original_size: size,
        zip_size,
    })
}

/// Flips a byte in the middle of a file (`Options::damage_before_check`).
fn damage(p: &Path) -> io::Result<()> {
    let mut b = std::fs::read(p)?;
    let mid = b.len() / 2;
    b[mid] ^= 0xFF;
    std::fs::write(p, b)
}

/// The log's time, for its entry in the zip (local time, as Explorer shows).
fn zip_time(meta: &std::fs::Metadata) -> zip::DateTime {
    use chrono::{Datelike, Timelike};
    let Ok(m) = meta.modified() else {
        return zip::DateTime::default();
    };
    let t = chrono::DateTime::<chrono::Local>::from(m);
    zip::DateTime::from_date_and_time(
        u16::try_from(t.year()).unwrap_or(1980),
        t.month() as u8,
        t.day() as u8,
        t.hour() as u8,
        t.minute() as u8,
        t.second() as u8,
    )
    .unwrap_or_default()
}

/// Streams the log into a new zip at `tmp`; returns the log's CRC-32.
fn write_zip(
    src: &mut File,
    tmp: &Path,
    name: &str,
    meta: &std::fs::Metadata,
    opts: &Options,
    on_progress: &mut dyn FnMut(u64),
) -> Result<u32, ArchiveError> {
    let size = meta.len();
    let out = File::create(tmp)?;
    let mut zw = zip::ZipWriter::new(BufWriter::with_capacity(1 << 16, out));
    zw.set_comment(COMMENT)?;
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .last_modified_time(zip_time(meta))
        .large_file(size >= opts.zip64_from);
    zw.start_file(name, options)?;
    let mut buf = vec![0u8; opts.chunk_size.max(1)];
    let mut crc = crc32fast::Hasher::new();
    let mut pace = Pace::new(opts.bytes_per_sec);
    let mut done = 0u64;
    loop {
        let n = match src.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        };
        crc.update(&buf[..n]);
        zw.write_all(&buf[..n])?;
        done += n as u64;
        on_progress(done);
        pace.take(n as u64);
    }
    if done != size {
        return Err(ArchiveError::Changed);
    }
    let out = zw.finish()?;
    let file = out.into_inner().map_err(|e| e.into_error())?;
    file.sync_all()?;
    Ok(crc.finalize())
}

/// The CRC-32 and length of what's left to read.
fn crc_of(
    src: &mut File,
    opts: &Options,
    on_progress: &mut dyn FnMut(u64),
) -> Result<(u32, u64), ArchiveError> {
    let mut buf = vec![0u8; opts.chunk_size.max(1)];
    let mut crc = crc32fast::Hasher::new();
    let mut pace = Pace::new(opts.bytes_per_sec);
    let mut n_total = 0u64;
    loop {
        let n = match src.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        };
        crc.update(&buf[..n]);
        n_total += n as u64;
        on_progress(n_total);
        pace.take(n as u64);
    }
    Ok((crc.finalize(), n_total))
}

/// Reads the zip back: our comment, one entry with the log's name, its
/// recorded CRC-32 and size equal to the original's, and every byte
/// decompressed again to the same CRC-32 and size.
pub fn verify(
    zip_path: &Path,
    name: &str,
    crc: u32,
    size: u64,
    opts: &Options,
    on_progress: &mut dyn FnMut(u64),
) -> Result<(), ArchiveError> {
    let mut za = zip::ZipArchive::new(File::open(zip_path)?)?;
    if za.comment() != COMMENT.as_bytes() || za.len() != 1 {
        return Err(ArchiveError::Verify);
    }
    let mut entry = za.by_index(0)?;
    if entry.name() != name || entry.size() != size || entry.crc32() != crc {
        return Err(ArchiveError::Verify);
    }
    let mut buf = vec![0u8; opts.chunk_size.max(1)];
    let mut again = crc32fast::Hasher::new();
    let mut n_total = 0u64;
    loop {
        let n = match entry.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            // A bad CRC or broken deflate data.
            Err(e) if e.kind() == io::ErrorKind::InvalidData => return Err(ArchiveError::Verify),
            Err(e) => return Err(e.into()),
        };
        again.update(&buf[..n]);
        n_total += n as u64;
        on_progress(n_total);
    }
    if n_total != size || again.finalize() != crc {
        return Err(ArchiveError::Verify);
    }
    Ok(())
}

/// Named as this app names archives: a combat log's name, as a `.zip`.
pub fn is_ours_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.starts_with("wowcombatlog") && lower.ends_with(".zip")
}

/// A `.zip` this app made: our name pattern and our comment.
pub fn is_ours(path: &Path) -> bool {
    let named = path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(is_ours_name);
    named
        && File::open(path)
            .ok()
            .and_then(|f| zip::ZipArchive::new(f).ok())
            .is_some_and(|za| za.comment() == COMMENT.as_bytes())
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct FolderSize {
    pub bytes: u64,
    pub files: u32,
}

/// The archive folder's size: every file directly in it.
pub fn folder_size(logs_dir: &Path) -> FolderSize {
    let mut out = FolderSize::default();
    let Ok(rd) = std::fs::read_dir(folder(logs_dir)) else {
        return out;
    };
    for e in rd.flatten() {
        if let Ok(m) = e.metadata() {
            if m.is_file() {
                out.bytes += m.len();
                out.files += 1;
            }
        }
    }
    out
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CleanUp {
    pub deleted: u32,
    pub freed: u64,
}

/// Deletes this app's archives (`is_ours`) last changed more than
/// `older_than` ago: nothing else in the folder, ever.
pub fn clean_up(logs_dir: &Path, older_than: Duration, now: SystemTime) -> CleanUp {
    let mut out = CleanUp::default();
    let Ok(rd) = std::fs::read_dir(folder(logs_dir)) else {
        return out;
    };
    for e in rd.flatten() {
        let Ok(m) = e.metadata() else { continue };
        if !m.is_file() {
            continue;
        }
        let old = m
            .modified()
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|age| age > older_than);
        let path = e.path();
        if old && is_ours(&path) && std::fs::remove_file(&path).is_ok() {
            out.deleted += 1;
            out.freed += m.len();
        }
    }
    if out.deleted > 0 {
        log::info!(
            "deleted {} old archived logs ({} bytes)",
            out.deleted,
            out.freed
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(
            zip_name("WoWCombatLog-092826_200101.txt"),
            "WoWCombatLog-092826_200101.zip"
        );
        assert_eq!(zip_name("WoWCombatLog.TXT"), "WoWCombatLog.zip");
        assert_eq!(zip_name("WoWCombatLog"), "WoWCombatLog.zip");
        assert!(is_ours_name("WoWCombatLog-092826_200101.zip"));
        assert!(!is_ours_name("WoWCombatLog-092826_200101.zip.partial"));
        assert!(!is_ours_name("holiday.zip"));
    }

    #[test]
    fn the_quick_checks_in_order() {
        let logs = Path::new("C:/WoW/Logs");
        let f = logs.join("WoWCombatLog-092126_193000.txt");
        let now = SystemTime::now();
        let old = now - Duration::from_secs(3600);
        let fresh = now - Duration::from_secs(60);
        let b = |file: &Path, newest: Option<&Path>, m, pending| {
            blocker(file, logs, newest, m, now, QUIET_FOR, pending)
        };
        assert_eq!(b(&f, None, old, false), None);
        assert_eq!(b(&f, Some(&f), old, false), Some(NotEligible::Newest));
        assert_eq!(b(&f, None, fresh, false), Some(NotEligible::Recent));
        assert_eq!(b(&f, None, old, true), Some(NotEligible::Queued));
        let elsewhere = logs.join("RaiderIOLogsArchive").join("WoWCombatLog.txt");
        assert_eq!(
            b(&elsewhere, None, old, false),
            Some(NotEligible::NotInLogs)
        );
        assert_eq!(
            b(&logs.join("notes.txt"), None, old, false),
            Some(NotEligible::NotInLogs)
        );
        // A time in the future (a clock change) counts as recent.
        assert_eq!(
            b(&f, None, now + Duration::from_secs(60), false),
            Some(NotEligible::Recent)
        );
    }
}
