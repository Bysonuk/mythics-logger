//! Measures what a folder of past logs would upload, locally: every pull and
//! key as the Backlog tab would send them, in both of its settings ("Kills
//! and each boss's best wipe", "All pulls"), at the live level (zstd 10) and
//! the backlog's (zstd 19, long-distance matching). Uploads nothing, writes
//! nothing but the optional file below, and prints counts, sizes and times
//! only: never a line or a name from a log.
//!
//! cargo run --release -p mythics-logger-core --example backlog_size -- "<folder>" \
//!     [--threads 16] [--sample 4] [--boss-hp <file.tsv>]
//!
//! - `--sample n`: compress one segment in n at the backlog's level (it's
//!   slow: about 1.3 MB a second a core) and scale up; the live level
//!   compresses every one. `--sample 1` compresses everything.
//! - `--boss-hp <file>`: writes each boss pull's place in its file (path,
//!   byte range, header line) and the app's boss health, for checking
//!   against the server's parser. It names the files, so keep it local.

use mythics_logger_core::backlog::{analyse, find_logs, LogFile};
use mythics_logger_core::chunker::{measure, Profile, Source};
use mythics_logger_core::plan::{plan, BacklogPulls, How};
use mythics_logger_core::splitter::{Kind, Segment};
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Default)]
struct Totals {
    files: u64,
    file_bytes: u64,
    pulls: u64,
    kills: u64,
    wipes: u64,
    keys: u64,
    summarised: u64,
    unsummarisable_wipes: u64,
    // Raw bytes: every uploadable segment, and the ones sent in full with
    // the default setting.
    all_raw: u64,
    best_raw: u64,
    // Compressed at level 10, every segment.
    all_z10: u64,
    best_z10: u64,
    // Compressed at the backlog's level, the sampled segments, with their
    // raw bytes.
    sample_all_raw: u64,
    sample_all_z19: u64,
    sample_best_raw: u64,
    sample_best_z19: u64,
    z10_ns: u64,
    z19_ns: u64,
    z10_raw: u64,
    z19_raw: u64,
}

fn arg(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

fn sampled(s: &Segment, n: u64) -> bool {
    n <= 1
        || u64::from_str_radix(&s.sha256[..8], 16)
            .unwrap_or(0)
            .is_multiple_of(n)
}

fn main() {
    let dir = std::env::args().nth(1).expect("a folder of combat logs");
    let threads: usize = arg("--threads").map_or(8, |t| t.parse().expect("threads"));
    let sample: u64 = arg("--sample").map_or(4, |t| t.parse().expect("sample"));
    let hp_out =
        arg("--boss-hp").map(|p| Mutex::new(std::fs::File::create(p).expect("boss-hp file")));
    let started = Instant::now();
    let mut files = find_logs(std::path::Path::new(&dir));
    // Biggest last, so they're taken first and no thread is left with one
    // big file at the end.
    files.sort_by_key(|f| f.size);
    let work: Arc<Mutex<Vec<(usize, LogFile)>>> =
        Arc::new(Mutex::new(files.into_iter().enumerate().collect()));
    let totals = Arc::new(Mutex::new(Totals::default()));
    let split_ns = Arc::new(AtomicU64::new(0));
    let hp_out = Arc::new(hp_out);
    std::thread::scope(|scope| {
        for _ in 0..threads {
            let work = work.clone();
            let totals = totals.clone();
            let split_ns = split_ns.clone();
            let hp_out = hp_out.clone();
            scope.spawn(move || loop {
                let Some((_, file)) = work.lock().unwrap().pop() else {
                    return;
                };
                let t = Instant::now();
                let Ok(report) = analyse(&file, |_| true) else {
                    continue;
                };
                split_ns.fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
                let segs: Vec<&Segment> = report.uploadable().collect();
                let how = plan(&segs, BacklogPulls::KillsAndBestWipe);
                let mut local = Totals {
                    files: 1,
                    file_bytes: file.size,
                    ..Totals::default()
                };
                for (s, how) in segs.iter().zip(&how) {
                    if s.opened_as == Kind::Encounter {
                        local.pulls += 1;
                        if s.success == Some(true) {
                            local.kills += 1;
                        } else {
                            local.wipes += 1;
                            if !mythics_logger_core::plan::can_summarise(s) {
                                local.unsummarisable_wipes += 1;
                            }
                        }
                        if let Some(out) = hp_out.as_ref() {
                            let hp = s.boss_hp_pct.map_or("".into(), |v| format!("{v:.2}"));
                            let line = format!(
                                "{}\t{}\t{}\t{}\t{}\t{}\n",
                                file.path.display(),
                                s.start_offset,
                                s.end_offset,
                                s.header.as_deref().unwrap_or("").trim_end(),
                                hp,
                                match s.success {
                                    Some(true) => "1",
                                    Some(false) => "0",
                                    None => "",
                                }
                            );
                            out.lock().unwrap().write_all(line.as_bytes()).unwrap();
                        }
                    } else if s.opened_as == Kind::Key {
                        local.keys += 1;
                    }
                    local.all_raw += s.size;
                    let full = *how == How::Full;
                    if full {
                        local.best_raw += s.size;
                    } else {
                        local.summarised += 1;
                    }
                    let src = Source {
                        path: file.path.clone(),
                        header: s.header.clone().map(String::into_bytes),
                        start: s.start_offset,
                        end: s.end_offset,
                    };
                    let t = Instant::now();
                    let Ok((_, z10)) = measure(&src, Profile::Live) else {
                        continue;
                    };
                    local.z10_ns += t.elapsed().as_nanos() as u64;
                    local.z10_raw += s.size;
                    local.all_z10 += z10;
                    if full {
                        local.best_z10 += z10;
                    }
                    if sampled(s, sample) {
                        let t = Instant::now();
                        let Ok((_, z19)) = measure(&src, Profile::Backlog) else {
                            continue;
                        };
                        local.z19_ns += t.elapsed().as_nanos() as u64;
                        local.z19_raw += s.size;
                        local.sample_all_raw += s.size;
                        local.sample_all_z19 += z19;
                        if full {
                            local.sample_best_raw += s.size;
                            local.sample_best_z19 += z19;
                        }
                    }
                }
                let mut t = totals.lock().unwrap();
                t.files += local.files;
                t.file_bytes += local.file_bytes;
                t.pulls += local.pulls;
                t.kills += local.kills;
                t.wipes += local.wipes;
                t.keys += local.keys;
                t.summarised += local.summarised;
                t.unsummarisable_wipes += local.unsummarisable_wipes;
                t.all_raw += local.all_raw;
                t.best_raw += local.best_raw;
                t.all_z10 += local.all_z10;
                t.best_z10 += local.best_z10;
                t.sample_all_raw += local.sample_all_raw;
                t.sample_all_z19 += local.sample_all_z19;
                t.sample_best_raw += local.sample_best_raw;
                t.sample_best_z19 += local.sample_best_z19;
                t.z10_ns += local.z10_ns;
                t.z19_ns += local.z19_ns;
                t.z10_raw += local.z10_raw;
                t.z19_raw += local.z19_raw;
                eprintln!("{} files done", t.files);
            });
        }
    });
    let t = totals.lock().unwrap();
    let gb = |b: f64| b / 1e9;
    let rate = |raw: u64, ns: u64| raw as f64 / 1e6 / (ns as f64 / 1e9).max(1e-9);
    let z19_all = t.all_raw as f64 * t.sample_all_z19 as f64 / t.sample_all_raw.max(1) as f64;
    let z19_best = t.best_raw as f64 * t.sample_best_z19 as f64 / t.sample_best_raw.max(1) as f64;
    // A summary is a few hundred bytes of JSON, before HTTP's own compression.
    let summaries = t.summarised as f64 * 400.0;
    println!("files {}, {:.2} GB", t.files, gb(t.file_bytes as f64));
    println!(
        "pulls {} (kills {}, wipes {}, of which {} can't be summarised), keys {}",
        t.pulls, t.kills, t.wipes, t.unsummarisable_wipes, t.keys
    );
    println!(
        "split: {:.1} s of thread time ({:.0} MB/s a thread)",
        split_ns.load(Ordering::Relaxed) as f64 / 1e9,
        rate(t.file_bytes, split_ns.load(Ordering::Relaxed))
    );
    println!(
        "all pulls:   raw {:.2} GB -> level 10 {:.3} GB, level 19 long {:.3} GB (est. from {:.2} GB sampled)",
        gb(t.all_raw as f64),
        gb(t.all_z10 as f64),
        gb(z19_all),
        gb(t.sample_all_raw as f64)
    );
    println!(
        "kills+best:  raw {:.2} GB -> level 10 {:.3} GB, level 19 long {:.3} GB, plus {} summaries (~{:.1} MB)",
        gb(t.best_raw as f64),
        gb(t.best_z10 as f64),
        gb(z19_best),
        t.summarised,
        summaries / 1e6
    );
    println!(
        "ratio level 19 long: all {:.1}x, kills+best {:.1}x",
        t.sample_all_raw as f64 / t.sample_all_z19.max(1) as f64,
        t.sample_best_raw as f64 / t.sample_best_z19.max(1) as f64
    );
    println!(
        "speed a thread: level 10 {:.1} MB/s, level 19 long {:.2} MB/s",
        rate(t.z10_raw, t.z10_ns),
        rate(t.z19_raw, t.z19_ns)
    );
    let one =
        |raw: u64| Duration::from_secs_f64(raw as f64 / 1e6 / rate(t.z19_raw, t.z19_ns).max(1e-9));
    println!(
        "level 19 long on one thread: all pulls {:.1} h, kills+best {:.1} h",
        one(t.all_raw).as_secs_f64() / 3600.0,
        one(t.best_raw).as_secs_f64() / 3600.0
    );
    println!("took {:.1?}", started.elapsed());
}
