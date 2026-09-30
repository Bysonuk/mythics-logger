//! Follows the newest `WoWCombatLog*.txt` in the Logs folder as the game
//! writes it.
//!
//! - Reads from the last offset through one fixed 1 MB buffer, so memory
//!   stays flat whatever the file's size.
//! - Holds back a line until its newline arrives: the game flushes in blocks,
//!   so the end of a read is often mid-line.
//! - When a newer file appears (logging turned on again), reads the old one
//!   to its end, then moves to the new one.
//! - When the file shrinks, or its first bytes change (another uploader
//!   cleared it), starts again from 0.
//! - Opens files read-only and shares read, write and delete, so the game,
//!   Warcraft Logs' uploader and anyone else carry on as if we weren't there.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

const BUF_SIZE: usize = 1 << 20;
const HEAD_SIZE: usize = 4096;
/// A "line" longer than this is garbage (a corrupt file): it's passed on in
/// pieces rather than held in memory.
const MAX_PARTIAL: usize = 16 << 20;

/// Opens a log read-only, sharing read, write and delete with everyone else.
pub fn open_shared(path: &Path) -> io::Result<File> {
    let mut o = std::fs::OpenOptions::new();
    o.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE
        o.share_mode(0x1 | 0x2 | 0x4);
    }
    o.open(path)
}

/// Whether a file name is a combat log: `WoWCombatLog.txt`,
/// `WoWCombatLog-092726_213545.txt`, or a renamed or archived copy that keeps
/// the prefix.
pub fn is_combat_log_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.starts_with("wowcombatlog") && lower.ends_with(".txt")
}

/// The newest combat log directly in `dir`, by modification time (then name).
/// The game's file names aren't trusted for ordering.
pub fn newest_log(dir: &Path) -> io::Result<Option<(PathBuf, SystemTime, u64)>> {
    let mut best: Option<(PathBuf, SystemTime, u64)> = None;
    for entry in std::fs::read_dir(dir)? {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !is_combat_log_name(name) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let path = entry.path();
        let newer = match &best {
            None => true,
            Some((bp, bt, _)) => (mtime, path.as_path()) > (*bt, bp.as_path()),
        };
        if newer {
            best = Some((path, mtime, meta.len()));
        }
    }
    Ok(best)
}

#[derive(Debug)]
pub enum Event<'a> {
    /// Started following a file at `offset`. For a file opened part-way
    /// through: `header` is its first line when it's a combat log header, and
    /// `zone` its latest `ZONE_CHANGE` line before `offset`
    /// (`Splitter::resuming`).
    Opened {
        path: &'a Path,
        offset: u64,
        header: Option<Vec<u8>>,
        zone: Option<Vec<u8>>,
    },
    /// One whole line, with its line ending, starting at `offset`.
    Line { offset: u64, bytes: &'a [u8] },
    /// The file shrank or was replaced: reading starts again from 0.
    Restarted { path: &'a Path },
    /// Finished with a file: a newer one appeared, or it was deleted.
    Closed { path: &'a Path, end: u64 },
}

struct Current {
    path: PathBuf,
    file: File,
    /// The next byte to read.
    pos: u64,
    /// An incomplete last line, and where it starts.
    partial: Vec<u8>,
    partial_start: u64,
    head: Vec<u8>,
}

pub struct Tailer {
    dir: PathBuf,
    cur: Option<Current>,
    buf: Vec<u8>,
    /// Stop after this many bytes in one poll, so a big file read from 0
    /// still lets the app show progress and save its place.
    pub max_per_poll: u64,
}

impl Tailer {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            cur: None,
            buf: vec![0; BUF_SIZE],
            max_per_poll: 64 << 20,
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn current_path(&self) -> Option<&Path> {
        self.cur.as_ref().map(|c| c.path.as_path())
    }

    /// The file and the offset of the first byte not yet handed on as a whole line.
    pub fn position(&self) -> Option<(&Path, u64)> {
        self.cur
            .as_ref()
            .map(|c| (c.path.as_path(), c.pos - c.partial.len() as u64))
    }

    /// Reads whatever is new. `start_at` picks where to begin in a file seen
    /// for the first time (given its length and modification time). Returns
    /// the bytes read.
    pub fn poll(
        &mut self,
        start_at: &mut dyn FnMut(&Path, u64, SystemTime) -> u64,
        sink: &mut dyn FnMut(Event<'_>),
    ) -> io::Result<u64> {
        let newest = newest_log(&self.dir)?;
        let mut read = 0;

        let switch = match (&self.cur, &newest) {
            (Some(c), Some((p, _, _))) => c.path != *p,
            (Some(c), None) => !c.path.exists(),
            _ => false,
        };
        if switch {
            // Finish the old file first: its last pull may still be arriving.
            read += self.read_available(u64::MAX, sink)?;
            let mut c = self.cur.take().expect("current file");
            if !c.partial.is_empty() {
                let bytes = std::mem::take(&mut c.partial);
                sink(Event::Line {
                    offset: c.partial_start,
                    bytes: &bytes,
                });
            }
            log::info!("finished reading a combat log at byte {}", c.pos);
            sink(Event::Closed {
                path: &c.path,
                end: c.pos,
            });
        }

        if self.cur.is_none() {
            let Some((path, mtime, len)) = newest else {
                return Ok(read);
            };
            let file = open_shared(&path)?;
            let offset = start_at(&path, len, mtime).min(len);
            let head = read_head(&path).unwrap_or_default();
            let header = if offset > 0 { header_line(&head) } else { None };
            let zone = if offset > 0 {
                last_zone_change(&path, offset).unwrap_or_else(|e| {
                    log::warn!("couldn't look back for the zone: {}", e.kind());
                    None
                })
            } else {
                None
            };
            log::info!("following a combat log from byte {offset} of {len}");
            sink(Event::Opened {
                path: &path,
                offset,
                header,
                zone,
            });
            self.cur = Some(Current {
                path,
                file,
                pos: offset,
                partial: Vec::new(),
                partial_start: offset,
                head,
            });
        }

        self.check_restarted(sink)?;
        read += self.read_available(self.max_per_poll, sink)?;
        Ok(read)
    }

    fn check_restarted(&mut self, sink: &mut dyn FnMut(Event<'_>)) -> io::Result<()> {
        let Some(c) = self.cur.as_mut() else {
            return Ok(());
        };
        let len = match std::fs::metadata(&c.path) {
            Ok(m) => m.len(),
            Err(_) => return Ok(()),
        };
        let head_now = read_head(&c.path).unwrap_or_default();
        let n = head_now.len().min(c.head.len());
        let replaced = n > 0 && head_now[..n] != c.head[..n];
        if len < c.pos || replaced {
            log::info!("a combat log shrank or was replaced; reading it again from the start");
            c.file = open_shared(&c.path)?;
            c.pos = 0;
            c.partial.clear();
            c.partial_start = 0;
            c.head = head_now;
            sink(Event::Restarted { path: &c.path });
        } else if head_now.len() > c.head.len() {
            c.head = head_now;
        }
        Ok(())
    }

    fn read_available(&mut self, cap: u64, sink: &mut dyn FnMut(Event<'_>)) -> io::Result<u64> {
        let Some(c) = self.cur.as_mut() else {
            return Ok(0);
        };
        c.file.seek(SeekFrom::Start(c.pos))?;
        let mut total = 0u64;
        while total < cap {
            let n = match c.file.read(&mut self.buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            };
            let data = &self.buf[..n];
            let mut start = 0usize;
            while let Some(rel) = data[start..].iter().position(|&b| b == b'\n') {
                let end = start + rel + 1;
                if c.partial.is_empty() {
                    sink(Event::Line {
                        offset: c.pos + start as u64,
                        bytes: &data[start..end],
                    });
                } else {
                    c.partial.extend_from_slice(&data[start..end]);
                    let bytes = std::mem::take(&mut c.partial);
                    sink(Event::Line {
                        offset: c.partial_start,
                        bytes: &bytes,
                    });
                    // Keep the allocation for the next partial line.
                    c.partial = bytes;
                    c.partial.clear();
                }
                start = end;
            }
            if start < n {
                if c.partial.is_empty() {
                    c.partial_start = c.pos + start as u64;
                }
                c.partial.extend_from_slice(&data[start..]);
                if c.partial.len() > MAX_PARTIAL {
                    let bytes = std::mem::take(&mut c.partial);
                    sink(Event::Line {
                        offset: c.partial_start,
                        bytes: &bytes,
                    });
                }
            }
            c.pos += n as u64;
            total += n as u64;
        }
        if c.partial.capacity() > BUF_SIZE && c.partial.is_empty() {
            c.partial = Vec::new();
        }
        Ok(total)
    }
}

fn read_head(path: &Path) -> io::Result<Vec<u8>> {
    let mut f = open_shared(path)?;
    let mut head = vec![0; HEAD_SIZE];
    let mut n = 0;
    while n < HEAD_SIZE {
        match f.read(&mut head[n..])? {
            0 => break,
            k => n += k,
        }
    }
    head.truncate(n);
    Ok(head)
}

/// The last whole `ZONE_CHANGE` line that ends at or before byte `before` of
/// a file, with its line ending: what the splitter would be carrying had it
/// read the file from its start, so a pull resumed after a restart goes with
/// the same bytes. Reads backwards through one 1 MB buffer, so it costs a
/// little when the zone changed recently (it usually has) and a single pass
/// of the file at worst, with flat memory either way.
pub fn last_zone_change(path: &Path, before: u64) -> io::Result<Option<Vec<u8>>> {
    const BLOCK: u64 = BUF_SIZE as u64;
    let mut f = open_shared(path)?;
    let mut end = before.min(f.metadata()?.len());
    // The start of a line whose beginning is further back, from the block
    // read before this one.
    let mut tail: Vec<u8> = Vec::new();
    let mut data: Vec<u8> = Vec::with_capacity(BUF_SIZE);
    while end > 0 {
        let start = end.saturating_sub(BLOCK);
        data.clear();
        data.resize((end - start) as usize, 0);
        f.seek(SeekFrom::Start(start))?;
        f.read_exact(&mut data)?;
        data.extend_from_slice(&tail);
        // Before the first newline is part of a line that began in an
        // earlier block, unless this block is the file's start.
        let whole_from = if start == 0 {
            0
        } else {
            data.iter()
                .position(|&b| b == b'\n')
                .map_or(data.len(), |i| i + 1)
        };
        let mut line_end = data.len();
        while line_end > whole_from {
            let line_start = data[whole_from..line_end - 1]
                .iter()
                .rposition(|&b| b == b'\n')
                .map_or(whole_from, |i| whole_from + i + 1);
            let l = &data[line_start..line_end];
            if l.ends_with(b"\n") && crate::line::event_name(l) == Some("ZONE_CHANGE") {
                return Ok(Some(l.to_vec()));
            }
            line_end = line_start;
        }
        tail.clear();
        // A "line" longer than the tailer would ever hand over is garbage.
        if whole_from <= MAX_PARTIAL {
            tail.extend_from_slice(&data[..whole_from]);
        }
        end = start;
    }
    Ok(None)
}

/// The first line of a file, if it's a combat log header.
fn header_line(head: &[u8]) -> Option<Vec<u8>> {
    let end = head.iter().position(|&b| b == b'\n')? + 1;
    let line = &head[..end];
    (crate::line::event_name(line) == Some("COMBAT_LOG_VERSION")).then(|| line.to_vec())
}

/// Where to start in a file the app hasn't seen before, when tailing live.
/// A file the game is still writing (changed in the last 10 minutes) is read
/// from its start, so a pull already under way isn't lost; an older one is
/// left for the Backlog tab, since past logs never count as live.
pub fn live_start(len: u64, mtime: SystemTime, now: SystemTime) -> u64 {
    let fresh = now
        .duration_since(mtime)
        .map(|d| d.as_secs() < 600)
        .unwrap_or(true);
    if fresh {
        0
    } else {
        len
    }
}
