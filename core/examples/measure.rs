//! Measures the live path on a real combat log, locally: tails the newest log
//! in a Logs folder from its start, splits it, compresses each segment into a
//! temporary folder (deleted afterwards), and prints counts, sizes, time and
//! peak memory. Uploads nothing, and prints no line or name from the log.
//!
//! cargo run --release -p mythics-logger-core --example measure -- "<Logs folder>"

use mythics_logger_core::chunker::{prepare, Profile, Source};
use mythics_logger_core::splitter::{Kind, Splitter};
use mythics_logger_core::tailer::{Event, Tailer};
use std::time::Instant;

fn main() {
    let dir = std::env::args().nth(1).expect("a Logs folder");
    // `--split-only`: skip compression (for huge synthetic files with a
    // million tiny segments, where file creation would dominate).
    let split_only = std::env::args().any(|a| a == "--split-only");
    let out = std::env::temp_dir().join("mythics-measure");
    let started = Instant::now();
    let mut tailer = Tailer::new(&dir);
    let mut splitter = Splitter::new();
    let mut segs = Vec::new();
    let mut kinds = [0usize; 3];
    let mut bytes = 0u64;
    let mut file = None;
    loop {
        let n = tailer
            .poll(&mut |_, _, _| 0, &mut |e| match e {
                Event::Line { offset, bytes } => {
                    if let Some(s) = splitter.feed(offset, bytes) {
                        if split_only {
                            kinds[s.kind as usize] += 1;
                        } else {
                            segs.push(s);
                        }
                    }
                }
                Event::Opened { path, header, .. } => {
                    file = Some(path.to_path_buf());
                    if let Some(h) = header {
                        splitter = Splitter::with_header(&h);
                    }
                }
                _ => {}
            })
            .expect("poll");
        bytes += n;
        if n == 0 {
            break;
        }
    }
    segs.extend(splitter.finish());
    let read_time = started.elapsed();
    let file = file.expect("a combat log");

    // The whole file in 8 MB pieces, streamed, at zstd's default level and
    // at the app's (chunker::LEVEL): sizes, and the time per piece.
    for level in [3, mythics_logger_core::chunker::LEVEL] {
        use std::io::Read;
        let mut f = std::fs::File::open(&file).expect("open");
        let mut piece = vec![0u8; 8 << 20];
        let (mut in_bytes, mut out_bytes, mut pieces) = (0usize, 0usize, 0u32);
        let mut spent = std::time::Duration::ZERO;
        loop {
            let mut got = 0;
            while got < piece.len() {
                match f.read(&mut piece[got..]).expect("read") {
                    0 => break,
                    n => got += n,
                }
            }
            if got == 0 {
                break;
            }
            let t = Instant::now();
            out_bytes += zstd::bulk::compress(&piece[..got], level)
                .expect("zstd")
                .len();
            spent += t.elapsed();
            in_bytes += got;
            pieces += 1;
        }
        println!(
            "zstd level {level}: {:.1} MB -> {:.2} MB ({:.1}x), {:.0} ms per 8 MB piece",
            in_bytes as f64 / 1e6,
            out_bytes as f64 / 1e6,
            in_bytes as f64 / out_bytes.max(1) as f64,
            spent.as_secs_f64() * 1000.0 / pieces.max(1) as f64
        );
    }

    let mut compressed = 0u64;
    let mut raw = 0u64;
    let mut chunks = 0u32;
    for (i, s) in segs.iter().enumerate() {
        let src = Source {
            path: file.clone(),
            header: s.header.clone().map(String::into_bytes),
            start: s.start_offset,
            end: s.end_offset,
        };
        let d = out.join(i.to_string());
        let p = prepare(&src, &d, Some(&s.sha256), Profile::Live).expect("prepare");
        chunks += p.chunks;
        raw += p.size;
        for n in 0..p.chunks {
            compressed += std::fs::metadata(d.join(format!("{n}.zst"))).unwrap().len();
        }
    }
    let _ = std::fs::remove_dir_all(&out);

    let count = |k: Kind| kinds[k as usize] + segs.iter().filter(|s| s.kind == k).count();
    println!("read {:.1} MB in {:.2?}", bytes as f64 / 1e6, read_time);
    println!(
        "encounters seen {} (inside keys too), keys seen {}",
        splitter.encounters_seen, splitter.keys_seen
    );
    println!(
        "segments: {} encounter, {} key, {} unfinished",
        count(Kind::Encounter),
        count(Kind::Key),
        count(Kind::Segment)
    );
    println!(
        "segments' bytes {:.1} MB -> {:.1} MB zstd in {chunks} chunks ({:.1}x); total {:.2?}",
        raw as f64 / 1e6,
        compressed as f64 / 1e6,
        raw as f64 / compressed.max(1) as f64,
        started.elapsed()
    );
    if let Some((peak, now)) = memory() {
        println!(
            "peak working set {:.1} MB, now {:.1} MB",
            peak as f64 / 1e6,
            now as f64 / 1e6
        );
    }
}

#[cfg(windows)]
fn memory() -> Option<(usize, usize)> {
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let mut c: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
    c.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    let ok = unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) };
    (ok != 0).then_some((c.PeakWorkingSetSize, c.WorkingSetSize))
}

#[cfg(not(windows))]
fn memory() -> Option<(usize, usize)> {
    None
}
