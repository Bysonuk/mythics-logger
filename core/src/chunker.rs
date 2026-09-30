//! Compresses a segment into zstd chunks of at most 4 MB each, on disk.
//!
//! The segment is read straight from the log file (its header line, then its
//! byte range), in fixed-size pieces: memory stays flat whatever its size.
//! Each piece of `chunk_size` uncompressed bytes becomes one chunk,
//! compressed on its own, so the server can take them in any order and a
//! failed upload resumes at the chunk it stopped on.
//!
//! Combat logs compress about eighteenfold at level 10, so an 8 MB piece is
//! usually well under 1 MB. If any piece doesn't fit in 4 MB (unusual text), the whole segment
//! is cut again at half the size, down to 2 MB, which always fits.
//!
//! Two profiles (`Profile`): a live pull at level 10 in pieces of 8 MB, to
//! be on the site minutes after the boss dies; a past log at level 19 with
//! long-distance matching in pieces of 32 MB, as small as zstd makes it. On
//! the owner's 36.9 GB of past pulls and keys, level 10 came to 2.04 GB and
//! the backlog's profile to 1.53 GB (`examples/backlog_size.rs`).

use crate::splitter::hex;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// The API's limit on one chunk's body.
pub const MAX_COMPRESSED: usize = 4 * 1024 * 1024;
/// Uncompressed sizes to try for a live pull, largest first. zstd's worst
/// case for 2 MB is a little over 2 MB, so the last always fits.
pub const CHUNK_SIZES: [u64; 3] = [8 << 20, 4 << 20, 2 << 20];
/// zstd level 10 for live pulls (the owner's choice, 29 Sep 2026): on the
/// real 86 MB log, 4.7 MB against 7.0 MB at the default level 3, at about
/// 0.2 s per 8 MB chunk against 25 ms (`examples/measure.rs`). A live pull
/// should be on the site minutes after the boss dies.
pub const LEVEL: i32 = 10;

/// Past logs: the most zstd gives (the owner's decision, 29 Sep 2026: "max
/// compression for backlog"). Nobody waits for a past log, so the app can
/// spend the time, on a low-priority thread (`crate::priority`).
pub const BACKLOG_LEVEL: i32 = 19;
/// Long-distance matching with a window of up to 2^27 bytes (128 MiB). zstd
/// shrinks the window to the chunk when the chunk is smaller, as it always
/// is here; the server decompresses windows up to 2^27 and refuses bigger
/// ones (`docs/specs/logger-api.md`, "Send a chunk").
pub const BACKLOG_WINDOW_LOG: u32 = 27;
/// Past logs' chunks are bigger, so long-distance matching has more to
/// match against: a combat log repeats itself over minutes, not kilobytes.
/// The server takes up to 64 MiB decompressed and 4 MiB compressed.
pub const BACKLOG_CHUNK_SIZES: [u64; 5] = [32 << 20, 16 << 20, 8 << 20, 4 << 20, 2 << 20];

/// How much smaller the backlog's profile makes a past log, for the Backlog
/// tab's estimate before anything is compressed: 24.1 times on the owner's
/// 36.9 GB of pulls and keys, 23.9 for the kills and best wipes alone.
pub const BACKLOG_RATIO: f64 = 24.0;

/// A pull summary's size on the wire, about: a few hundred bytes of JSON.
pub const SUMMARY_BYTES: u64 = 400;

/// How hard to compress: live pulls quickly, past logs as small as zstd
/// can make them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    Live,
    Backlog,
}

impl Profile {
    pub fn level(self) -> i32 {
        match self {
            Profile::Live => LEVEL,
            Profile::Backlog => BACKLOG_LEVEL,
        }
    }

    pub fn chunk_sizes(self) -> &'static [u64] {
        match self {
            Profile::Live => &CHUNK_SIZES,
            Profile::Backlog => &BACKLOG_CHUNK_SIZES,
        }
    }

    pub fn compressor(self) -> io::Result<zstd::bulk::Compressor<'static>> {
        use zstd::zstd_safe::CParameter;
        let mut c = zstd::bulk::Compressor::new(self.level())?;
        if self == Profile::Backlog {
            c.set_parameter(CParameter::EnableLongDistanceMatching(true))?;
            c.set_parameter(CParameter::WindowLog(BACKLOG_WINDOW_LOG))?;
        }
        Ok(c)
    }

    /// How many chunks are compressed at once: one for a live pull; for a
    /// past log, `BACKLOG_THREADS` (fewer on a small computer).
    pub fn threads(self) -> usize {
        match self {
            Profile::Live => 1,
            Profile::Backlog => {
                let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
                if cores <= 4 {
                    1
                } else {
                    BACKLOG_THREADS
                }
            }
        }
    }

    fn compressors(self, threads: usize) -> io::Result<Vec<zstd::bulk::Compressor<'static>>> {
        (0..threads.max(1)).map(|_| self.compressor()).collect()
    }
}

/// Past logs' chunks are compressed two at a time, each on a
/// background-priority thread: level 19 does about 1.3 MB of log a second a
/// thread, and each compressor holds about 140 MB while it works (measured
/// on the owner's logs, `examples/backlog_size.rs`), so two halve the wait
/// for under 300 MB, beside a game that comes first.
pub const BACKLOG_THREADS: usize = 2;

/// Where a segment's bytes are: its header line, then a range of a file.
#[derive(Debug, Clone)]
pub struct Source {
    pub path: PathBuf,
    pub header: Option<Vec<u8>>,
    pub start: u64,
    pub end: u64,
}

impl Source {
    pub fn len(&self) -> u64 {
        self.header.as_ref().map_or(0, |h| h.len() as u64) + (self.end - self.start)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// A reader over the segment's bytes, uncompressed.
    pub fn reader(&self) -> io::Result<impl Read> {
        let mut f = crate::tailer::open_shared(&self.path)?;
        f.seek(SeekFrom::Start(self.start))?;
        let header = io::Cursor::new(self.header.clone().unwrap_or_default());
        Ok(header.chain(f.take(self.end - self.start)))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prepared {
    pub chunk_size: u64,
    pub chunks: u32,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ChunkError {
    #[error("the log file changed since this pull was read")]
    Changed,
    #[error(transparent)]
    Io(#[from] io::Error),
}

pub fn chunk_path(dir: &Path, n: u32) -> PathBuf {
    dir.join(format!("{n}.zst"))
}

/// Writes `0.zst`, `1.zst`, … into `dir`. `expected_sha` guards against a
/// file that changed under us (cleared by another uploader, say).
pub fn prepare(
    src: &Source,
    dir: &Path,
    expected_sha: Option<&str>,
    profile: Profile,
) -> Result<Prepared, ChunkError> {
    let mut compressors = profile.compressors(profile.threads())?;
    for &chunk_size in profile.chunk_sizes() {
        clear_dir(dir)?;
        std::fs::create_dir_all(dir)?;
        match try_prepare(src, dir, chunk_size, &mut compressors)? {
            Some(p) => {
                if expected_sha.is_some_and(|s| s != p.sha256) || p.size != src.len() {
                    clear_dir(dir)?;
                    return Err(ChunkError::Changed);
                }
                return Ok(p);
            }
            None => log::info!("a chunk didn't fit in 4 MB at {chunk_size} bytes; trying smaller"),
        }
    }
    unreachable!("2 MB always compresses to under 4 MB")
}

/// Writes the chunks at one given size: the one the server holds an upload
/// to when it already has it (a repeat keeps the first request's size).
/// `Ok(None)` if a chunk doesn't fit in 4 MB at that size.
pub fn prepare_at(
    src: &Source,
    dir: &Path,
    chunk_size: u64,
    expected_sha: Option<&str>,
    profile: Profile,
) -> Result<Option<Prepared>, ChunkError> {
    let mut compressors = profile.compressors(profile.threads())?;
    clear_dir(dir)?;
    std::fs::create_dir_all(dir)?;
    let p = try_prepare(src, dir, chunk_size, &mut compressors)?;
    if let Some(p) = &p {
        if expected_sha.is_some_and(|s| s != p.sha256) || p.size != src.len() {
            clear_dir(dir)?;
            return Err(ChunkError::Changed);
        }
    }
    if p.is_none() {
        clear_dir(dir)?;
    }
    Ok(p)
}

fn try_prepare(
    src: &Source,
    dir: &Path,
    chunk_size: u64,
    compressors: &mut [zstd::bulk::Compressor<'static>],
) -> io::Result<Option<Prepared>> {
    compress_each(src, chunk_size, compressors, |n, out| {
        let mut w = BufWriter::new(File::create(chunk_path(dir, n))?);
        w.write_all(out)?;
        w.flush()
    })
}

/// What a segment would come to, compressed, without writing anything, on
/// one thread: for `examples/backlog_size.rs`. Returns the chunk size used
/// and the compressed bytes.
pub fn measure(src: &Source, profile: Profile) -> io::Result<(u64, u64)> {
    let mut compressors = profile.compressors(1)?;
    for &chunk_size in profile.chunk_sizes() {
        let mut total = 0u64;
        let fits = compress_each(src, chunk_size, &mut compressors, |_, out| {
            total += out.len() as u64;
            Ok(())
        })?;
        if fits.is_some() {
            return Ok((chunk_size, total));
        }
    }
    unreachable!("2 MB always compresses to under 4 MB")
}

/// Compresses the segment in pieces of `chunk_size`, as many at once as
/// there are compressors (each on a background-priority thread when there's
/// more than one), handing each chunk to `sink` in order. `Ok(None)` as soon
/// as one doesn't fit in 4 MB.
fn compress_each(
    src: &Source,
    chunk_size: u64,
    compressors: &mut [zstd::bulk::Compressor<'static>],
    mut sink: impl FnMut(u32, &[u8]) -> io::Result<()>,
) -> io::Result<Option<Prepared>> {
    let mut reader = src.reader()?;
    let bound = zstd::zstd_safe::compress_bound(chunk_size as usize);
    // Pieces of the segment and their compressed chunks, one per compressor.
    let mut work: Vec<(Vec<u8>, usize, Vec<u8>)> = Vec::new();
    let mut hasher = Sha256::new();
    let mut n = 0u32;
    let mut size = 0u64;
    let mut done = false;
    while !done {
        let mut filled = 0;
        while filled < compressors.len().max(1) {
            if work.len() == filled {
                work.push((vec![0u8; chunk_size as usize], 0, Vec::with_capacity(bound)));
            }
            let (buf, got, _) = &mut work[filled];
            *got = read_full(&mut reader, buf)?;
            if *got == 0 {
                done = true;
                break;
            }
            hasher.update(&buf[..*got]);
            size += *got as u64;
            filled += 1;
            if *got < buf.len() {
                done = true;
                break;
            }
        }
        let pieces = &mut work[..filled];
        if let [(buf, got, out)] = pieces {
            out.clear();
            compressors[0].compress_to_buffer(&buf[..*got], out)?;
        } else if !pieces.is_empty() {
            std::thread::scope(|scope| {
                let running: Vec<_> = pieces
                    .iter_mut()
                    .zip(compressors.iter_mut())
                    .map(|((buf, got, out), c)| {
                        scope.spawn(move || {
                            crate::priority::lower_this_thread();
                            out.clear();
                            c.compress_to_buffer(&buf[..*got], out).map(|_| ())
                        })
                    })
                    .collect();
                running.into_iter().try_for_each(|t| {
                    t.join()
                        .unwrap_or_else(|_| Err(io::Error::other("panicked")))
                })
            })?;
        }
        for (_, _, out) in pieces.iter() {
            if out.len() > MAX_COMPRESSED {
                return Ok(None);
            }
            sink(n, out)?;
            n += 1;
        }
    }
    Ok(Some(Prepared {
        chunk_size,
        chunks: n,
        size,
        sha256: hex(&hasher.finalize()),
    }))
}

fn read_full(r: &mut impl Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}

pub fn clear_dir(dir: &Path) -> io::Result<()> {
    match std::fs::remove_dir_all(dir) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// The SHA-256 of a segment's bytes, streamed: the backlog check uses it when
/// a file's report is cached but the hash needs confirming.
pub fn sha256_of(src: &Source) -> io::Result<String> {
    let mut r = src.reader()?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = r.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}
