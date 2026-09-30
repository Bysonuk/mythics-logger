//! Sends one queued segment at a time: prepare its chunks, create the upload
//! (idempotent by SHA-256, so a retry or a second copy costs nothing), send
//! the chunks the server hasn't got, then complete it.
//!
//! First it asks: everyone in a raid logs the same pull, so before sending,
//! the waiting pulls' and keys' fingerprints go to the server in one request,
//! and any it already has (a raid member's copy the player may see) are
//! marked done without sending a byte (`docs/specs/logger-api.md`, "Ask
//! before uploading").
//!
//! A past log's wipe that `crate::plan` summarises isn't compressed or sent
//! in full: it goes with the rest of its file's summaries, up to 200 in one
//! request (the contract's "Pull summaries"). Past logs compress at the
//! backlog's level on background-priority threads (`chunker::Profile`).

use crate::api::{Api, ApiError, Fingerprint, NewUpload, PullSummaries, PullSummary};
use crate::chunker::{self, ChunkError, Profile, Source};
use crate::queue::{backoff_ms, now_ms, Already, Item, Origin, PreparedChunks, Queue, State};
use crate::splitter::Kind;
use crate::throttle::Throttle;
use crate::CLIENT_VERSION;
use std::sync::Mutex;

#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// Nothing to send now.
    Idle,
    Uploaded {
        sha256: String,
        id: String,
    },
    /// A network or server problem: it waits and tries again.
    Retrying {
        sha256: String,
    },
    /// Refused, or the file is gone: it won't be tried again.
    Failed {
        sha256: String,
        reason: String,
    },
    /// The token was refused: the player must log in again.
    SignedOut,
    /// Past logs were paused while this one was sending; it resumes later.
    Paused,
    /// The site already had these pulls and keys: nothing was sent.
    AlreadyThere {
        sha256s: Vec<String>,
    },
    /// These past-log wipes went as pull summaries (or the site had them
    /// in full already).
    Summarised {
        sha256s: Vec<String>,
    },
}

/// The API's limit on one pull-summaries request.
pub const MAX_SUMMARIES: usize = 200;

/// A queued wipe as a pull summary, if the server can file it: a boss, a
/// start and a roster hash (`crate::plan::can_summarise`).
pub fn summary_of(item: &Item) -> Option<PullSummary> {
    let s = &item.segment;
    let start_time = s.start_api(item.year_hint);
    let end_time = s.end_api(item.year_hint);
    if start_time.is_empty() || end_time.is_empty() || s.success == Some(true) {
        return None;
    }
    Some(PullSummary {
        encounter_id: s.encounter_id?,
        name: s.name.clone().filter(|n| !n.is_empty()),
        difficulty: s.difficulty,
        group_size: s.group_size,
        instance_id: s.instance_id,
        start_time,
        end_time,
        duration_ms: s.duration_ms,
        boss_hp_pct: s.boss_hp_pct,
        success: s.success,
        roster_hash: s.roster_hash.clone()?,
    })
}

/// Sends the ready summaries of `first`'s log file (same visibility and
/// region), up to 200, in one request. A server without the route, or one
/// that refuses them, gets those wipes in full instead: nothing is lost.
async fn send_summaries(queue: &Mutex<Queue>, api: &Api, first: &Item) -> Step {
    let now = now_ms();
    let batch: Vec<(String, PullSummary)> = {
        let mut q = queue.lock().expect("queue");
        let shas: Vec<String> = q
            .items()
            .iter()
            .filter(|i| {
                i.summary
                    && i.state == State::Waiting
                    && i.next_try_ms <= now
                    && i.session_key == first.session_key
                    && i.file == first.file
                    && i.visibility == first.visibility
                    && i.region == first.region
            })
            .map(|i| i.sha256.clone())
            .collect();
        let mut out = Vec::new();
        for sha in shas {
            let Some(i) = q.get_mut(&sha) else { continue };
            match summary_of(i) {
                Some(s) if out.len() < MAX_SUMMARIES => {
                    i.state = State::Uploading;
                    out.push((sha, s));
                }
                Some(_) => {}
                // Nothing to file it by: it goes in full.
                None => i.summary = false,
            }
        }
        let _ = q.save();
        out
    };
    if batch.is_empty() {
        return Step::Summarised {
            sha256s: Vec::new(),
        };
    }
    let body = PullSummaries {
        summaries: batch.iter().map(|(_, s)| s.clone()).collect(),
        region: first.region.clone(),
        visibility: first.visibility,
        client_version: CLIENT_VERSION.to_string(),
        session_key: first.session_key.clone(),
        file_name: first.session_key.as_ref().and(first.file_name.clone()),
        file_start_time: first.session_key.as_ref().and(first.file_start.clone()),
    };
    let result = api.pull_summaries(&body).await;
    let mut q = queue.lock().expect("queue");
    let step = match result {
        Ok(answers) => {
            for ((sha, _), a) in batch.iter().zip(answers) {
                let Some(i) = q.get_mut(sha) else { continue };
                let links = Already {
                    fight_id: a.fight_id,
                    url: a.url,
                    log_url: a.log_url,
                    boss_url: a.boss_url,
                };
                i.state = State::Done;
                i.done_ms = Some(now_ms());
                i.error = None;
                i.segment.header = None;
                if a.status == "have" {
                    i.already = Some(links);
                } else {
                    i.upload_id = a.upload_id;
                    i.summarised = Some(links);
                }
            }
            log::info!("sent {} pull summaries", batch.len());
            Step::Summarised {
                sha256s: batch.into_iter().map(|(s, _)| s).collect(),
            }
        }
        Err(ApiError::Unauthorized) => {
            for (sha, _) in &batch {
                if let Some(i) = q.get_mut(sha) {
                    i.state = State::Waiting;
                }
            }
            Step::SignedOut
        }
        Err(ApiError::Refused { status, .. }) => {
            log::info!("pull summaries refused ({status}); sending those wipes in full");
            for (sha, _) in &batch {
                if let Some(i) = q.get_mut(sha) {
                    i.state = State::Waiting;
                    i.summary = false;
                }
            }
            Step::Summarised {
                sha256s: Vec::new(),
            }
        }
        Err(e) => {
            for (sha, _) in &batch {
                if let Some(i) = q.get_mut(sha) {
                    i.state = State::Waiting;
                    i.attempts += 1;
                    let wait = match &e {
                        ApiError::Busy {
                            retry_after_s: Some(s),
                            ..
                        } => backoff_ms(i.attempts).max(s.saturating_mul(1000)),
                        _ => backoff_ms(i.attempts),
                    };
                    i.next_try_ms = now_ms() + wait;
                    i.error = Some(match e {
                        ApiError::Offline => "offline".into(),
                        _ => "server_busy".into(),
                    });
                }
            }
            log::info!("pull summaries will be tried again: {e}");
            Step::Retrying {
                sha256: first.sha256.clone(),
            }
        }
    };
    let _ = q.save();
    step
}

/// The API's limit on one fingerprints request.
pub const MAX_FINGERPRINTS: usize = 200;

/// A queued pull's or key's fingerprint, if it has one: a whole pull or key
/// with a roster. A segment that never ended is always sent.
pub fn fingerprint_of(item: &Item) -> Option<Fingerprint> {
    let seg = &item.segment;
    if !seg.complete {
        return None;
    }
    let roster_hash = seg.roster_hash.clone()?;
    let start_time = seg.start_api(item.year_hint);
    if start_time.is_empty() {
        return None;
    }
    match seg.kind {
        Kind::Encounter => Some(Fingerprint {
            kind: "encounter",
            encounter_id: Some(seg.encounter_id?),
            map_id: None,
            start_time,
            roster_hash,
        }),
        Kind::Key => Some(Fingerprint {
            kind: "key",
            encounter_id: None,
            map_id: Some(seg.map_id?),
            start_time,
            roster_hash,
        }),
        Kind::Segment => None,
    }
}

/// Asks about every waiting item not asked about yet, in one request, and
/// marks the ones the site has as done. Returns their hashes.
pub async fn ask_first(queue: &Mutex<Queue>, api: &Api) -> Result<Vec<String>, ApiError> {
    let now = now_ms();
    let asking: Vec<(String, Fingerprint)> = {
        let mut q = queue.lock().expect("queue");
        let paused = q.backlog_paused;
        let waiting: Vec<String> = q
            .items()
            .iter()
            .filter(|i| {
                i.state == State::Waiting
                    // A summary asks as it's sent (`send_summaries`).
                    && !i.summary
                    && !i.fingerprint_asked
                    && i.upload_id.is_none()
                    && i.next_try_ms <= now
                    && (i.origin == Origin::Live || !paused)
            })
            .map(|i| i.sha256.clone())
            .collect();
        let mut out = Vec::new();
        for sha in waiting {
            let Some(item) = q.get_mut(&sha) else {
                continue;
            };
            match fingerprint_of(item) {
                Some(f) if out.len() < MAX_FINGERPRINTS => out.push((sha, f)),
                Some(_) => {}
                // Nothing to ask about: it's sent.
                None => item.fingerprint_asked = true,
            }
        }
        out
    };
    if asking.is_empty() {
        return Ok(Vec::new());
    }
    let prints: Vec<Fingerprint> = asking.iter().map(|(_, f)| f.clone()).collect();
    let answers = match api.fingerprints(&prints).await {
        Ok(a) => a,
        Err(ApiError::Refused { status, .. }) => {
            // A server without the route (or one that won't take these):
            // send them, as before there was asking.
            log::info!("fingerprints refused ({status}); uploading as usual");
            let mut q = queue.lock().expect("queue");
            for (sha, _) in &asking {
                if let Some(i) = q.get_mut(sha) {
                    i.fingerprint_asked = true;
                }
            }
            let _ = q.save();
            return Ok(Vec::new());
        }
        Err(e) => return Err(e),
    };
    let mut skipped = Vec::new();
    let mut q = queue.lock().expect("queue");
    for ((sha, _), a) in asking.iter().zip(answers) {
        let dir = q.chunk_dir(sha);
        let Some(i) = q.get_mut(sha) else {
            continue;
        };
        i.fingerprint_asked = true;
        // Only if nothing started meanwhile (the player can't, but be sure).
        if !a.have() || i.state != State::Waiting {
            continue;
        }
        i.state = State::Done;
        i.done_ms = Some(now_ms());
        i.error = None;
        i.segment.header = None;
        i.already = Some(Already {
            fight_id: a.fight_id,
            url: a.url,
            log_url: a.log_url,
            boss_url: a.boss_url,
        });
        let _ = chunker::clear_dir(&dir);
        skipped.push(sha.clone());
    }
    let _ = q.save();
    if !skipped.is_empty() {
        log::info!(
            "{} pulls or keys already on the site; not sent",
            skipped.len()
        );
    }
    Ok(skipped)
}

/// Where a queued item's bytes come from.
pub fn source_of(item: &Item) -> Source {
    Source {
        path: item.file.clone(),
        header: item.segment.header.clone().map(String::into_bytes),
        start: item.segment.start_offset,
        end: item.segment.end_offset,
    }
}

/// Live pulls compress quickly; past logs as small as zstd can make them
/// (`chunker::Profile`).
pub fn profile_of(item: &Item) -> Profile {
    match item.origin {
        Origin::Live => Profile::Live,
        Origin::Backlog => Profile::Backlog,
    }
}

/// Compression off the upload task: a live pull on tokio's blocking pool, a
/// past log on a background-priority thread of its own.
async fn off_thread<T: Send + 'static>(
    profile: Profile,
    work: impl FnOnce() -> Result<T, ChunkError> + Send + 'static,
) -> Result<T, ChunkError> {
    let panicked = || Err(ChunkError::Io(std::io::Error::other("prepare panicked")));
    match profile {
        Profile::Live => tokio::task::spawn_blocking(work)
            .await
            .unwrap_or_else(|_| panicked()),
        Profile::Backlog => crate::priority::spawn_low(work)
            .await
            .unwrap_or_else(|_| panicked()),
    }
}

/// Writes a live pull's chunks straight away (see the queue's notes).
pub fn prepare_now(queue: &Mutex<Queue>, sha: &str) -> Result<(), ChunkError> {
    let (src, dir) = {
        let q = queue.lock().expect("queue");
        let Some(item) = q.get(sha) else {
            return Ok(());
        };
        (source_of(item), q.chunk_dir(sha))
    };
    let p = chunker::prepare(&src, &dir, Some(sha), Profile::Live)?;
    let mut q = queue.lock().expect("queue");
    if let Some(item) = q.get_mut(sha) {
        item.prepared = Some(PreparedChunks {
            chunk_size: p.chunk_size,
            chunks: p.chunks,
        });
    }
    let _ = q.save();
    Ok(())
}

fn chunks_present(q: &Queue, item: &Item) -> bool {
    match &item.prepared {
        Some(p) => {
            let dir = q.chunk_dir(&item.sha256);
            (0..p.chunks).all(|n| chunker::chunk_path(&dir, n).is_file())
        }
        None => false,
    }
}

/// Sends the next ready item, if any. `on_progress(sha, chunks sent, chunks)`
/// is called after each chunk.
pub async fn step(
    queue: &Mutex<Queue>,
    api: &Api,
    throttle: &Throttle,
    on_progress: &(dyn Fn(&str, u32, u32) + Send + Sync),
) -> Step {
    if !api.has_token() {
        return Step::SignedOut;
    }
    match ask_first(queue, api).await {
        Ok(skipped) if !skipped.is_empty() => return Step::AlreadyThere { sha256s: skipped },
        Err(ApiError::Unauthorized) => return Step::SignedOut,
        // Offline or busy: the upload below meets the same and backs off,
        // and nothing backing off is asked about again until it's ready.
        _ => {}
    }
    let next = queue.lock().expect("queue").next_ready(now_ms()).cloned();
    match next {
        None => return Step::Idle,
        Some(item) if item.summary => return send_summaries(queue, api, &item).await,
        Some(_) => {}
    }
    let (item, need_prepare, dir) = {
        let mut q = queue.lock().expect("queue");
        let Some(item) = q.next_ready(now_ms()).cloned() else {
            return Step::Idle;
        };
        let need = !chunks_present(&q, &item);
        let dir = q.chunk_dir(&item.sha256);
        if let Some(i) = q.get_mut(&item.sha256) {
            i.state = State::Uploading;
        }
        (item, need, dir)
    };
    let sha = item.sha256.clone();

    let profile = profile_of(&item);
    let prepared = if need_prepare {
        let src = source_of(&item);
        let d = dir.clone();
        let s = sha.clone();
        let r = off_thread(profile, move || {
            chunker::prepare(&src, &d, Some(&s), profile)
        })
        .await;
        match r {
            Ok(p) => PreparedChunks {
                chunk_size: p.chunk_size,
                chunks: p.chunks,
            },
            Err(e) => {
                let reason = match &e {
                    ChunkError::Changed => "file_changed",
                    ChunkError::Io(io) if io.kind() == std::io::ErrorKind::NotFound => {
                        "file_missing"
                    }
                    ChunkError::Io(_) => "file_unreadable",
                };
                log::warn!("couldn't prepare an upload: {reason}");
                return fail(queue, &sha, reason);
            }
        }
    } else {
        item.prepared.clone().expect("prepared")
    };
    {
        let mut q = queue.lock().expect("queue");
        if let Some(i) = q.get_mut(&sha) {
            i.prepared = Some(prepared.clone());
        }
        let _ = q.save();
    }

    let seg = &item.segment;
    let body = NewUpload {
        sha256: sha.clone(),
        size: seg.size,
        chunk_size: prepared.chunk_size,
        kind: seg.kind.as_str().to_string(),
        encounter_id: seg.encounter_id,
        difficulty: seg.difficulty,
        key_level: seg.key_level,
        // The contract's map_id: a key's challenge-mode id, a pull's instance.
        map_id: seg.map_id.or(seg.instance_id),
        start_time: seg.start_api(item.year_hint),
        end_time: seg.end_api(item.year_hint),
        region: item.region.clone(),
        visibility: item.visibility,
        client_version: CLIENT_VERSION.to_string(),
        session_key: item.session_key.clone(),
        file_name: item.session_key.as_ref().and(item.file_name.clone()),
        file_start_time: item.session_key.as_ref().and(item.file_start.clone()),
    };

    let created = match api.create_upload(&body).await {
        Ok(c) => c,
        Err(e) => return api_failed(queue, &sha, e),
    };
    {
        let mut q = queue.lock().expect("queue");
        if let Some(i) = q.get_mut(&sha) {
            i.upload_id = Some(created.id.clone());
        }
    }

    // Already complete on the server (sent before, from here or another
    // copy of the queue): nothing more to send.
    if created.is_complete() {
        return done(queue, &sha, &dir, &created.id, &item, prepared.chunks);
    }

    // A repeat keeps the first request's chunk size, and chunks must be cut
    // at it: cut them again if ours differ.
    let prepared = match created.chunk_size {
        Some(cs) if cs != prepared.chunk_size => {
            let src = source_of(&item);
            let d = dir.clone();
            let s = sha.clone();
            let r = off_thread(profile, move || {
                chunker::prepare_at(&src, &d, cs, Some(&s), profile)
            })
            .await;
            match r {
                Ok(Some(p)) => {
                    let p = PreparedChunks {
                        chunk_size: p.chunk_size,
                        chunks: p.chunks,
                    };
                    let mut q = queue.lock().expect("queue");
                    if let Some(i) = q.get_mut(&sha) {
                        i.prepared = Some(p.clone());
                    }
                    let _ = q.save();
                    p
                }
                Ok(None) => return fail(queue, &sha, "chunk_size_unusable"),
                Err(ChunkError::Changed) => return fail(queue, &sha, "file_changed"),
                Err(_) => return fail(queue, &sha, "file_unreadable"),
            }
        }
        _ => prepared,
    };

    let mut sent = created
        .received
        .iter()
        .filter(|&&n| n < prepared.chunks)
        .count() as u32;
    on_progress(&sha, sent, prepared.chunks);
    for n in 0..prepared.chunks {
        if created.received.contains(&n) {
            continue;
        }
        if item.origin == Origin::Backlog && queue.lock().expect("queue").backlog_paused {
            let mut q = queue.lock().expect("queue");
            if let Some(i) = q.get_mut(&sha) {
                i.state = State::Waiting;
            }
            let _ = q.save();
            return Step::Paused;
        }
        let bytes = match std::fs::read(chunker::chunk_path(&dir, n)) {
            Ok(b) => b,
            Err(_) => {
                // Chunks vanished (disk cleaned?): prepare again next time.
                let mut q = queue.lock().expect("queue");
                if let Some(i) = q.get_mut(&sha) {
                    i.prepared = None;
                    i.state = State::Waiting;
                }
                let _ = q.save();
                return Step::Retrying { sha256: sha };
            }
        };
        throttle.take(bytes.len() as u64).await;
        if let Err(e) = api.put_chunk(&created.id, n, bytes).await {
            return api_failed(queue, &sha, e);
        }
        sent += 1;
        {
            let mut q = queue.lock().expect("queue");
            if let Some(i) = q.get_mut(&sha) {
                i.chunks_sent = sent;
            }
        }
        on_progress(&sha, sent, prepared.chunks);
    }

    if let Err(e) = api.complete(&created.id).await {
        return api_failed(queue, &sha, e);
    }
    done(queue, &sha, &dir, &created.id, &item, prepared.chunks)
}

fn done(
    queue: &Mutex<Queue>,
    sha: &str,
    dir: &std::path::Path,
    id: &str,
    item: &Item,
    chunks: u32,
) -> Step {
    let mut q = queue.lock().expect("queue");
    if let Some(i) = q.get_mut(sha) {
        i.state = State::Done;
        i.done_ms = Some(now_ms());
        i.error = None;
        // A finished item needs no header copy; keeps queue.json small.
        i.segment.header = None;
    }
    let _ = chunker::clear_dir(dir);
    let _ = q.save();
    log::info!(
        "uploaded a {} segment ({} chunks)",
        item.segment.kind.as_str(),
        chunks
    );
    Step::Uploaded {
        sha256: sha.to_string(),
        id: id.to_string(),
    }
}

fn fail(queue: &Mutex<Queue>, sha: &str, reason: &str) -> Step {
    let mut q = queue.lock().expect("queue");
    let dir = q.chunk_dir(sha);
    if let Some(i) = q.get_mut(sha) {
        i.state = State::Failed;
        i.error = Some(reason.to_string());
    }
    let _ = chunker::clear_dir(&dir);
    let _ = q.save();
    Step::Failed {
        sha256: sha.to_string(),
        reason: reason.to_string(),
    }
}

/// Refusals that clear up by themselves (`docs/specs/logger-api.md`,
/// section 4): a create racing another, chunks the server lost track of, or
/// an upload completed meanwhile. The next try creates (resumes) again.
const TRY_AGAIN: [&str; 3] = ["upload_busy", "chunks_missing", "upload_not_receiving"];

fn api_failed(queue: &Mutex<Queue>, sha: &str, e: ApiError) -> Step {
    // The joined chunks didn't match: the server dropped them all, and the
    // app can't know which was wrong, so it cuts and sends them all again.
    let e = match e {
        ApiError::Refused { code: Some(c), .. } if c == "hash_mismatch" => {
            let mut q = queue.lock().expect("queue");
            let dir = q.chunk_dir(sha);
            let _ = chunker::clear_dir(&dir);
            if let Some(i) = q.get_mut(sha) {
                i.prepared = None;
            }
            drop(q);
            ApiError::Busy {
                status: 422,
                retry_after_s: None,
            }
        }
        ApiError::Refused {
            status,
            code: Some(c),
        } if TRY_AGAIN.contains(&c.as_str()) => ApiError::Busy {
            status,
            retry_after_s: None,
        },
        e => e,
    };
    match e {
        ApiError::Unauthorized => {
            let mut q = queue.lock().expect("queue");
            if let Some(i) = q.get_mut(sha) {
                i.state = State::Waiting;
            }
            let _ = q.save();
            Step::SignedOut
        }
        e if e.is_retryable() => {
            let mut q = queue.lock().expect("queue");
            if let Some(i) = q.get_mut(sha) {
                i.state = State::Waiting;
                i.attempts += 1;
                let wait = match &e {
                    // Never sooner than the server asked (a quota can be hours).
                    ApiError::Busy {
                        retry_after_s: Some(s),
                        ..
                    } => backoff_ms(i.attempts).max(s.saturating_mul(1000)),
                    _ => backoff_ms(i.attempts),
                };
                i.next_try_ms = now_ms() + wait;
                i.error = Some(match e {
                    ApiError::Offline => "offline".into(),
                    _ => "server_busy".into(),
                });
            }
            let _ = q.save();
            log::info!("upload will be tried again: {e}");
            Step::Retrying {
                sha256: sha.to_string(),
            }
        }
        ApiError::Refused { status, code } => {
            log::warn!("upload refused ({status})");
            fail(
                queue,
                sha,
                &code.unwrap_or_else(|| format!("refused_{status}")),
            )
        }
        _ => fail(queue, sha, "refused"),
    }
}
