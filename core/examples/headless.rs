//! Runs the app's upload path without the window, against a local server:
//! the same tailer, splitter, chunker, queue and uploader the app uses. For
//! an end-to-end check against the site's API on a developer's computer
//! (`docs/specs/logger-api.md`, section 9: `mt10 logger dev-token`).
//!
//! ```text
//! MT10_TOKEN=mtl_… cargo run -p mythics-logger-core --example headless -- \
//!     --origin http://127.0.0.1:8765 --token-env MT10_TOKEN --data-dir <dir> \
//!     (--backlog <file> [--all-pulls] | --live <Logs folder>) [--visibility private] [--region eu] [--idle-exit 20]
//! ```
//!
//! `--backlog` queues a file's pulls and keys as past logs and sends them: its
//! kills and each boss's best wipe in full and the other wipes as summaries,
//! or with `--all-pulls` every pull in full, as the Backlog tab's setting.
//! `--live` follows the newest log in a folder, as the app does, and sends
//! each pull or key as it ends; it stops once nothing new has been written
//! for `--idle-exit` seconds and the queue is empty. `--data-dir` holds the
//! queue and chunks: point it at the app's own data folder and the app shows
//! what was sent. Prints counts, ids and times only: never a line or a name.
//! Only a local server (http to this computer) or https is accepted.

use mythics_logger_core::api::{Api, Visibility};
use mythics_logger_core::backlog::{analyse, describe};
use mythics_logger_core::plan::{queue_items, BacklogPulls};
use mythics_logger_core::queue::{now_ms, Item, Origin, Queue};
use mythics_logger_core::splitter::{Kind, Splitter};
use mythics_logger_core::tailer::{live_start, Event, Tailer};
use mythics_logger_core::throttle::Throttle;
use mythics_logger_core::uploader::{self, Step};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

struct Args {
    origin: String,
    token: String,
    data_dir: PathBuf,
    backlog: Option<PathBuf>,
    live: Option<PathBuf>,
    visibility: Visibility,
    region: String,
    idle_exit: u64,
    all_pulls: bool,
}

fn args() -> Args {
    let mut it = std::env::args().skip(1);
    let (mut origin, mut token_env, mut data_dir) = (None, None, None);
    let (mut backlog, mut live) = (None, None);
    let mut visibility = Visibility::Public;
    let mut region = "eu".to_string();
    let mut idle_exit = 20;
    let mut all_pulls = false;
    while let Some(a) = it.next() {
        if a == "--all-pulls" {
            all_pulls = true;
            continue;
        }
        let mut v = || {
            it.next()
                .unwrap_or_else(|| die(&format!("{a} needs a value")))
        };
        match a.as_str() {
            "--origin" => origin = Some(v()),
            "--token-env" => token_env = Some(v()),
            "--data-dir" => data_dir = Some(PathBuf::from(v())),
            "--backlog" => backlog = Some(PathBuf::from(v())),
            "--live" => live = Some(PathBuf::from(v())),
            "--region" => region = v(),
            "--idle-exit" => idle_exit = v().parse().unwrap_or_else(|_| die("--idle-exit")),
            "--visibility" => {
                visibility = serde_json::from_value(serde_json::Value::String(v()))
                    .unwrap_or_else(|_| die("--visibility is public, guild or private"))
            }
            other => die(&format!("unknown argument {other}")),
        }
    }
    let origin = origin.unwrap_or_else(|| die("--origin"));
    let local = ["http://127.0.0.1", "http://localhost", "http://[::1]"]
        .iter()
        .any(|p| origin.starts_with(p));
    if !local && !origin.starts_with("https://") {
        die("--origin must be https, or http to this computer");
    }
    let token_env = token_env.unwrap_or_else(|| die("--token-env"));
    let token = std::env::var(&token_env)
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| die("the token variable is empty"));
    if backlog.is_some() == live.is_some() {
        die("one of --backlog or --live");
    }
    Args {
        origin,
        token,
        data_dir: data_dir.unwrap_or_else(|| die("--data-dir")),
        backlog,
        live,
        visibility,
        region,
        idle_exit,
        all_pulls,
    }
}

fn die(msg: &str) -> ! {
    eprintln!("headless: {msg}");
    std::process::exit(2)
}

/// Sends everything ready now. Returns false if the token was refused.
async fn drain(queue: &Mutex<Queue>, api: &Api, throttle: &Throttle, started: Instant) -> bool {
    loop {
        let step = uploader::step(queue, api, throttle, &|_, _, _| {}).await;
        let t = started.elapsed().as_millis();
        match step {
            Step::Idle | Step::Paused => return true,
            Step::Uploaded { sha256, id } => {
                println!(
                    "uploaded id={id} sha={} at_unix_ms={} t+{t}ms",
                    &sha256[..12],
                    now_ms()
                );
            }
            Step::AlreadyThere { sha256s } => {
                let q = queue.lock().expect("queue");
                for sha in sha256s {
                    let url = q
                        .get(&sha)
                        .and_then(|i| i.already.as_ref())
                        .and_then(|a| a.url.clone())
                        .unwrap_or_default();
                    println!("already_there sha={} url={url} t+{t}ms", &sha[..12]);
                }
            }
            Step::Summarised { sha256s } => {
                println!("summarised {} wipes t+{t}ms", sha256s.len());
            }
            Step::Failed { sha256, reason } => {
                println!("failed sha={} reason={reason} t+{t}ms", &sha256[..12])
            }
            Step::Retrying { sha256 } => {
                let (why, wake) = {
                    let q = queue.lock().expect("queue");
                    let why = q.get(&sha256).and_then(|i| i.error.clone());
                    (why, q.next_wake_ms())
                };
                println!("retrying sha={} ({why:?}) t+{t}ms", &sha256[..12]);
                let wait = wake.map(|w| w.saturating_sub(now_ms())).unwrap_or(1000);
                tokio::time::sleep(Duration::from_millis(wait.clamp(250, 60_000))).await;
            }
            Step::SignedOut => {
                println!("signed_out: the token was refused");
                return false;
            }
        }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let a = args();
    std::fs::create_dir_all(&a.data_dir).unwrap_or_else(|_| die("can't make --data-dir"));
    let api = Api::new(&a.origin).with_token(Some(a.token.clone()));
    let queue = Mutex::new(Queue::load(&a.data_dir));
    let throttle = Throttle::new(0);
    let started = Instant::now();

    if let Some(file) = &a.backlog {
        let lf = describe(file).unwrap_or_else(|_| die("can't read the file"));
        let report = analyse(&lf, |_| true).unwrap_or_else(|_| die("can't read the file"));
        let mut added = 0;
        {
            let mut q = queue.lock().expect("queue");
            let mode = if a.all_pulls {
                BacklogPulls::All
            } else {
                BacklogPulls::KillsAndBestWipe
            };
            for item in queue_items(&report, mode, a.visibility, &a.region, |_| false) {
                added += q.add(item) as u32;
            }
            let _ = q.save();
        }
        println!(
            "backlog: {} bytes, {} segments ({} pulls, {} keys), {added} newly queued, read in {}ms",
            lf.size,
            report.segments.len(),
            report.encounters,
            report.keys,
            started.elapsed().as_millis()
        );
        let ok = drain(&queue, &api, &throttle, started).await;
        let c = queue.lock().expect("queue").counts();
        println!(
            "done: {} sent, {} failed, {} waiting, {}ms",
            c.done,
            c.failed,
            c.backlog_waiting + c.live_waiting,
            started.elapsed().as_millis()
        );
        std::process::exit(if ok && c.failed == 0 { 0 } else { 1 });
    }

    let dir = a.live.clone().expect("live");
    let mut tailer = Tailer::new(&dir);
    let mut splitter = Splitter::new();
    let mut reading: Option<PathBuf> = None;
    let mut last_bytes = Instant::now();
    let mut total = 0u64;
    loop {
        let mut segs = Vec::new();
        let n = tailer
            .poll(
                &mut |_, len, mtime| live_start(len, mtime, SystemTime::now()),
                &mut |e| match e {
                    Event::Opened { path, header, .. } => {
                        reading = Some(path.to_path_buf());
                        splitter = match header {
                            Some(h) => Splitter::with_header(&h),
                            None => Splitter::new(),
                        };
                    }
                    Event::Line { offset, bytes } => {
                        if let (Some(s), Some(p)) = (splitter.feed(offset, bytes), &reading) {
                            segs.push((p.clone(), s));
                        }
                    }
                    Event::Restarted { .. } => splitter = Splitter::new(),
                    Event::Closed { path, .. } => {
                        if let Some(s) = splitter.finish() {
                            segs.push((path.to_path_buf(), s));
                        }
                        splitter = Splitter::new();
                    }
                },
            )
            .unwrap_or(0);
        total += n;
        if n > 0 {
            last_bytes = Instant::now();
        }
        for (file, seg) in segs {
            // As the app does: a scrap of an unfinished pull isn't worth sending.
            if seg.kind == Kind::Segment && seg.size <= 64 * 1024 {
                continue;
            }
            println!(
                "ended kind={} bytes={} sha={} seen_at_unix_ms={} t+{}ms",
                seg.kind.as_str(),
                seg.size,
                &seg.sha256[..12],
                now_ms(),
                started.elapsed().as_millis()
            );
            let sha = seg.sha256.clone();
            let added = {
                let mut q = queue.lock().expect("queue");
                let added = q.add(Item::new(Origin::Live, file, seg, a.visibility, &a.region));
                let _ = q.save();
                added
            };
            if added {
                if let Err(e) = uploader::prepare_now(&queue, &sha) {
                    println!("couldn't compress: {e}");
                }
            }
        }
        if !drain(&queue, &api, &throttle, started).await {
            std::process::exit(1);
        }
        if last_bytes.elapsed() > Duration::from_secs(a.idle_exit) {
            let c = queue.lock().expect("queue").counts();
            println!(
                "idle: read {total} bytes; {} sent, {} failed, {} waiting",
                c.done, c.failed, c.live_waiting
            );
            std::process::exit(if c.failed == 0 { 0 } else { 1 });
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}
