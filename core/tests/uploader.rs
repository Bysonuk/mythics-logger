//! The uploader against a stub server on 127.0.0.1: never a real one.

mod common;

use common::{fixture, write_file, Request, Stub};
use mythics_logger_core::api::{Api, Visibility};
use mythics_logger_core::backlog::{analyse, describe};
use mythics_logger_core::plan::{queue_items, BacklogPulls};
use mythics_logger_core::queue::{Item, Origin, Queue, State};
use mythics_logger_core::splitter::{split_file, Segment};
use mythics_logger_core::throttle::Throttle;
use mythics_logger_core::uploader::{fingerprint_of, prepare_now, step, Step};
use rand::RngCore;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

fn segments(path: &Path) -> Vec<Segment> {
    let mut out = Vec::new();
    split_file(path, |s| out.push(s), |_| true).unwrap();
    out
}

struct Setup {
    _tmp: tempfile::TempDir,
    data: PathBuf,
    log: PathBuf,
    queue: Mutex<Queue>,
}

fn setup(file: &str, origin: Origin) -> Setup {
    let tmp = tempfile::tempdir().unwrap();
    let log = write_file(tmp.path(), "WoWCombatLog-092826_200101.txt", &fixture(file));
    let data = tmp.path().join("data");
    let mut q = Queue::load(&data);
    for s in segments(&log) {
        q.add(Item::new(origin, log.clone(), s, Visibility::Guild, "eu"));
    }
    Setup {
        _tmp: tmp,
        data,
        log,
        queue: Mutex::new(q),
    }
}

/// "need" for every fingerprint asked about: nobody has uploaded them.
fn need_all(r: &Request) -> (u16, String) {
    let body: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
    let n = body["fingerprints"].as_array().unwrap().len();
    let answers = vec![serde_json::json!({"status": "need"}); n];
    (
        200,
        serde_json::json!({ "fingerprints": answers }).to_string(),
    )
}

const ASK: &str = "/api/logger/fingerprints";

fn happy(r: &Request) -> (u16, String) {
    match (r.method.as_str(), r.path.as_str()) {
        ("POST", ASK) => need_all(r),
        ("POST", "/api/logger/uploads") => (201, r#"{"id":"u1","received":[]}"#.into()),
        ("PUT", _) => (204, String::new()),
        ("POST", p) if p.ends_with("/complete") => (200, "{}".into()),
        _ => (404, "{}".into()),
    }
}

fn api(stub: &Stub) -> Api {
    Api::new(&stub.origin).with_token(Some("test-token".into()))
}

const NO_PROGRESS: &(dyn Fn(&str, u32, u32) + Send + Sync) = &|_, _, _| {};

#[tokio::test]
async fn uploads_a_pull_to_the_contract() {
    let s = setup("raid_night.txt", Origin::Live);
    let stub = Stub::start(happy);
    let throttle = Throttle::new(0);
    let first = s.queue.lock().unwrap().items()[0].clone();

    let r = step(&s.queue, &api(&stub), &throttle, NO_PROGRESS).await;
    assert_eq!(
        r,
        Step::Uploaded {
            sha256: first.sha256.clone(),
            id: "u1".into()
        }
    );
    // First it asks about both of tonight's pulls, in one request.
    let (asks, reqs): (Vec<_>, Vec<_>) = stub.taken().into_iter().partition(|r| r.path == ASK);
    assert_eq!(asks.len(), 1);
    let asked: serde_json::Value = serde_json::from_slice(&asks[0].body).unwrap();
    assert_eq!(asked["fingerprints"].as_array().unwrap().len(), 2);
    let paths: Vec<_> = reqs
        .iter()
        .map(|r| format!("{} {}", r.method, r.path))
        .collect();
    assert_eq!(
        paths,
        [
            "POST /api/logger/uploads",
            "PUT /api/logger/uploads/u1/chunks/0",
            "POST /api/logger/uploads/u1/complete"
        ]
    );
    assert!(reqs
        .iter()
        .all(|r| r.authorization.as_deref() == Some("Bearer test-token")));

    let body: serde_json::Value = serde_json::from_slice(&reqs[0].body).unwrap();
    assert_eq!(body["sha256"], first.sha256);
    assert_eq!(body["size"], first.segment.size);
    assert_eq!(body["chunk_size"], 8 << 20);
    assert_eq!(body["kind"], "encounter");
    assert_eq!(body["encounter_id"], 3129);
    assert_eq!(body["difficulty"], 16);
    assert!(body.get("key_level").is_none());
    assert_eq!(body["start_time"], "2026-09-28T20:05:00.000+01:00");
    assert_eq!(body["end_time"], "2026-09-28T20:08:40.000+01:00");
    assert_eq!(body["region"], "eu");
    assert_eq!(body["visibility"], "guild");
    assert_eq!(body["client_version"], env!("CARGO_PKG_VERSION"));

    // The chunk is zstd, and unpacks to the header, the zone line carried
    // from before the pull, then the pull's bytes: each line as the file has
    // it, and all of it what `sha256` is of.
    assert_eq!(reqs[1].content_type.as_deref(), Some("application/zstd"));
    let raw = zstd::decode_all(&reqs[1].body[..]).unwrap();
    let file = std::fs::read(&s.log).unwrap();
    let seg = &first.segment;
    let mut expected = seg.header.clone().unwrap().into_bytes();
    expected
        .extend_from_slice(b"9/28/2026 20:01:02.1021  ZONE_CHANGE,2810,\"Manaforge Omega\",16\r\n");
    expected.extend_from_slice(&file[seg.start_offset as usize..seg.end_offset as usize]);
    assert_eq!(raw, expected);
    let lines: Vec<&[u8]> = file.split_inclusive(|&b| b == b'\n').collect();
    assert!(lines.contains(&seg.zone_line.as_deref().unwrap().as_bytes()));
    assert_eq!(
        mythics_logger_core::splitter::hex(&sha2::Digest::finalize(
            <sha2::Sha256 as sha2::Digest>::new_with_prefix(&raw)
        )),
        first.sha256
    );

    let q = s.queue.lock().unwrap();
    assert_eq!(q.get(&first.sha256).unwrap().state, State::Done);
    assert!(
        !q.chunk_dir(&first.sha256).exists(),
        "chunks are cleaned up"
    );
}

#[tokio::test]
async fn resumes_with_only_the_chunks_the_server_lacks() {
    // 9 MB of noise: five 2 MB chunks; the server already has 0, 1 and 2.
    let tmp = tempfile::tempdir().unwrap();
    let mut body = vec![0u8; 9 << 20];
    rand::thread_rng().fill_bytes(&mut body);
    let head = b"9/28/2026 20:05:00.0001  ENCOUNTER_START,3129,\"Plexus Sentinel\",16,20,2810\r\n";
    let tail =
        b"\r\n9/28/2026 20:08:40.0001  ENCOUNTER_END,3129,\"Plexus Sentinel\",16,20,1,220000\r\n";
    let mut file = head.to_vec();
    file.extend(body.iter().map(|b| if *b == b'\n' { b'.' } else { *b }));
    file.extend_from_slice(tail);
    let log = write_file(tmp.path(), "WoWCombatLog.txt", &file);
    let seg = segments(&log).remove(0);
    let mut q = Queue::load(&tmp.path().join("data"));
    q.add(Item::new(
        Origin::Backlog,
        log,
        seg,
        Visibility::Public,
        "us",
    ));
    let queue = Mutex::new(q);

    let stub = Stub::start(|r| match (r.method.as_str(), r.path.as_str()) {
        ("POST", "/api/logger/uploads") => (200, r#"{"id":"u9","received":[0,1,2]}"#.into()),
        ("PUT", _) => (204, String::new()),
        _ => (200, "{}".into()),
    });
    let progress = Mutex::new(Vec::new());
    let on = |_: &str, a: u32, b: u32| progress.lock().unwrap().push((a, b));
    let r = step(&queue, &api(&stub), &Throttle::new(0), &on).await;
    assert!(matches!(r, Step::Uploaded { .. }));
    let puts: Vec<_> = stub
        .taken()
        .into_iter()
        .filter(|r| r.method == "PUT")
        .map(|r| r.path)
        .collect();
    assert_eq!(
        puts,
        [
            "/api/logger/uploads/u9/chunks/3",
            "/api/logger/uploads/u9/chunks/4"
        ]
    );
    assert_eq!(*progress.lock().unwrap(), [(3, 5), (4, 5), (5, 5)]);
}

#[tokio::test]
async fn waits_and_retries_when_the_server_is_busy() {
    let s = setup("raid_night.txt", Origin::Live);
    let stub = Stub::start(|_| (503, "{}".into()));
    let throttle = Throttle::new(0);
    let r = step(&s.queue, &api(&stub), &throttle, NO_PROGRESS).await;
    assert!(matches!(r, Step::Retrying { .. }));
    let sha = {
        let q = s.queue.lock().unwrap();
        let it = &q.items()[0];
        assert_eq!(it.state, State::Waiting);
        assert_eq!(it.attempts, 1);
        assert_eq!(it.error.as_deref(), Some("server_busy"));
        assert!(it.next_try_ms > mythics_logger_core::queue::now_ms());
        it.sha256.clone()
    };
    // Backing off: the other pull goes meanwhile.
    stub.set_handler(happy);
    let r = step(&s.queue, &api(&stub), &throttle, NO_PROGRESS).await;
    assert!(matches!(r, Step::Uploaded { ref sha256, .. } if *sha256 != sha));
    // After the wait, the first one goes too.
    s.queue.lock().unwrap().get_mut(&sha).unwrap().next_try_ms = 0;
    let r = step(&s.queue, &api(&stub), &throttle, NO_PROGRESS).await;
    assert!(matches!(r, Step::Uploaded { ref sha256, .. } if *sha256 == sha));
    assert_eq!(
        step(&s.queue, &api(&stub), &throttle, NO_PROGRESS).await,
        Step::Idle
    );
}

#[tokio::test]
async fn offline_is_retried_later() {
    let s = setup("raid_night.txt", Origin::Live);
    // Nothing listens on this port.
    let dead = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = format!("http://{}", dead.local_addr().unwrap());
    drop(dead);
    let api = Api::new(&origin).with_token(Some("t".into()));
    let r = step(&s.queue, &api, &Throttle::new(0), NO_PROGRESS).await;
    assert!(matches!(r, Step::Retrying { .. }));
    assert_eq!(
        s.queue.lock().unwrap().items()[0].error.as_deref(),
        Some("offline")
    );
}

#[tokio::test]
async fn a_refused_token_signs_out_and_keeps_the_item() {
    let s = setup("raid_night.txt", Origin::Live);
    let stub = Stub::start(|_| (401, "{}".into()));
    let r = step(&s.queue, &api(&stub), &Throttle::new(0), NO_PROGRESS).await;
    assert_eq!(r, Step::SignedOut);
    assert_eq!(s.queue.lock().unwrap().items()[0].state, State::Waiting);
    // And with no token at all, nothing is sent.
    let none = Api::new(&stub.origin);
    stub.taken();
    assert_eq!(
        step(&s.queue, &none, &Throttle::new(0), NO_PROGRESS).await,
        Step::SignedOut
    );
    assert!(stub.taken().is_empty());
}

#[tokio::test]
async fn a_refusal_fails_the_item_with_its_code() {
    let s = setup("raid_night.txt", Origin::Live);
    let stub = Stub::start(|_| (422, r#"{"code":"log_too_new"}"#.into()));
    let r = step(&s.queue, &api(&stub), &Throttle::new(0), NO_PROGRESS).await;
    assert!(matches!(r, Step::Failed { ref reason, .. } if reason == "log_too_new"));
    assert_eq!(s.queue.lock().unwrap().items()[0].state, State::Failed);
}

#[tokio::test]
async fn live_pulls_go_before_past_logs_and_past_logs_can_pause() {
    let s = setup("mplus_key.txt", Origin::Backlog);
    let raid = write_file(
        s.log.parent().unwrap(),
        "WoWCombatLog-092826_210000.txt",
        &fixture("raid_night.txt"),
    );
    {
        let mut q = s.queue.lock().unwrap();
        for seg in segments(&raid) {
            q.add(Item::new(
                Origin::Live,
                raid.clone(),
                seg,
                Visibility::Public,
                "eu",
            ));
        }
    }
    let stub = Stub::start(happy);
    let t = Throttle::new(0);
    for _ in 0..2 {
        step(&s.queue, &api(&stub), &t, NO_PROGRESS).await;
    }
    let bodies: Vec<serde_json::Value> = stub
        .taken()
        .iter()
        .filter(|r| r.path == "/api/logger/uploads")
        .map(|r| serde_json::from_slice(&r.body).unwrap())
        .collect();
    assert_eq!(
        bodies
            .iter()
            .map(|b| b["kind"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["encounter", "encounter"]
    );

    s.queue.lock().unwrap().backlog_paused = true;
    assert_eq!(
        step(&s.queue, &api(&stub), &t, NO_PROGRESS).await,
        Step::Idle
    );
    s.queue.lock().unwrap().backlog_paused = false;
    let r = step(&s.queue, &api(&stub), &t, NO_PROGRESS).await;
    assert!(matches!(r, Step::Uploaded { .. }));
    let body: serde_json::Value = serde_json::from_slice(&stub.taken()[0].body).unwrap();
    assert_eq!(body["kind"], "key");
    assert_eq!(body["key_level"], 14);
    assert_eq!(body["map_id"], 584);
    assert!(body.get("encounter_id").is_none());
    let c = s.queue.lock().unwrap().counts();
    assert_eq!((c.backlog_done, c.backlog_total, c.live_waiting), (1, 1, 0));
}

#[test]
fn the_queue_survives_a_restart_and_never_adds_a_pull_twice() {
    let s = setup("raid_night.txt", Origin::Live);
    let sha = {
        let mut q = s.queue.lock().unwrap();
        let sha = q.items()[0].sha256.clone();
        q.get_mut(&sha).unwrap().state = State::Uploading; // as if the app crashed mid-upload
        q.backlog_paused = true;
        q.save().unwrap();
        sha
    };
    // A live pull's chunks are written as soon as it ends.
    prepare_now(&s.queue, &sha).unwrap();
    let mut q = Queue::load(&s.data);
    assert_eq!(q.items().len(), 2);
    assert!(q.backlog_paused);
    let it = q.get(&sha).unwrap();
    assert_eq!(it.state, State::Waiting);
    assert_eq!(it.prepared.as_ref().unwrap().chunks, 1);
    assert!(q.chunk_dir(&sha).join("0.zst").is_file());
    let dup = Item::new(
        Origin::Backlog,
        s.log.clone(),
        it.segment.clone(),
        Visibility::Private,
        "eu",
    );
    assert!(
        !q.add(dup),
        "the same pull, found again in the backlog, is skipped"
    );
}

#[tokio::test]
async fn a_log_deleted_before_upload_fails_cleanly() {
    let s = setup("raid_night.txt", Origin::Backlog);
    std::fs::remove_file(&s.log).unwrap();
    let stub = Stub::start(happy);
    let r = step(&s.queue, &api(&stub), &Throttle::new(0), NO_PROGRESS).await;
    assert!(matches!(r, Step::Failed { ref reason, .. } if reason == "file_missing"));
    // Only the question: nothing was created.
    assert!(stub.taken().iter().all(|r| r.path == ASK));
}

/// A 9 MB pull of noise (no line breaks inside), queued as a past log.
fn noise_pull(tmp: &Path) -> Mutex<Queue> {
    let mut body = vec![0u8; 9 << 20];
    rand::thread_rng().fill_bytes(&mut body);
    let head = b"9/28/2026 20:05:00.0001  ENCOUNTER_START,3129,\"Plexus Sentinel\",16,20,2810\r\n";
    let tail =
        b"\r\n9/28/2026 20:08:40.0001  ENCOUNTER_END,3129,\"Plexus Sentinel\",16,20,1,220000\r\n";
    let mut file = head.to_vec();
    file.extend(body.iter().map(|b| if *b == b'\n' { b'.' } else { *b }));
    file.extend_from_slice(tail);
    let log = write_file(tmp, "WoWCombatLog.txt", &file);
    let seg = segments(&log).remove(0);
    let mut q = Queue::load(&tmp.join("data"));
    q.add(Item::new(
        Origin::Backlog,
        log,
        seg,
        Visibility::Public,
        "eu",
    ));
    Mutex::new(q)
}

#[tokio::test]
async fn uses_the_servers_numeric_id_and_its_chunk_size() {
    // The server answers as the contract says: a numeric id, and a repeat
    // keeps the first request's chunk size (here 3 MiB, not the app's 2 MiB
    // for noise), which the chunks must be cut at.
    let tmp = tempfile::tempdir().unwrap();
    let queue = noise_pull(tmp.path());
    let stub = Stub::start(|r| match (r.method.as_str(), r.path.as_str()) {
        ("POST", "/api/logger/uploads") => (
            200,
            r#"{"id":41,"status":"receiving","received":[],"chunk_size":3145728,"chunk_count":4}"#
                .into(),
        ),
        ("PUT", _) => (204, String::new()),
        _ => (200, r#"{"id":41,"status":"queued"}"#.into()),
    });
    let r = step(&queue, &api(&stub), &Throttle::new(0), NO_PROGRESS).await;
    assert!(
        matches!(r, Step::Uploaded { ref id, .. } if id == "41"),
        "{r:?}"
    );
    let reqs = stub.taken();
    let paths: Vec<_> = reqs.iter().map(|r| r.path.as_str()).collect();
    assert_eq!(
        paths,
        [
            "/api/logger/uploads",
            "/api/logger/uploads/41/chunks/0",
            "/api/logger/uploads/41/chunks/1",
            "/api/logger/uploads/41/chunks/2",
            "/api/logger/uploads/41/chunks/3",
            "/api/logger/uploads/41/complete"
        ]
    );
    let sizes: Vec<_> = reqs[1..5]
        .iter()
        .map(|r| zstd::decode_all(&r.body[..]).unwrap().len())
        .collect();
    let total = queue.lock().unwrap().items()[0].segment.size as usize;
    assert_eq!(sizes, [3 << 20, 3 << 20, 3 << 20, total - (9 << 20)]);
}

#[tokio::test]
async fn an_upload_the_server_already_has_sends_nothing_more() {
    let s = setup("raid_night.txt", Origin::Live);
    let stub = Stub::start(|r| {
        if r.path == ASK {
            return need_all(r);
        }
        (
            200,
            r#"{"id":7,"status":"parsed","received":[0],"chunk_size":8388608}"#.into(),
        )
    });
    let r = step(&s.queue, &api(&stub), &Throttle::new(0), NO_PROGRESS).await;
    assert!(matches!(r, Step::Uploaded { ref id, .. } if id == "7"));
    let reqs: Vec<_> = stub.taken().into_iter().filter(|r| r.path != ASK).collect();
    assert_eq!(reqs.len(), 1, "only the create");
    assert_eq!(s.queue.lock().unwrap().items()[0].state, State::Done);
}

#[tokio::test]
async fn refusals_that_clear_up_are_tried_again() {
    for code in ["upload_busy", "chunks_missing", "hash_mismatch"] {
        let s = setup("raid_night.txt", Origin::Live);
        let stub = Stub::start(move |r| match (r.method.as_str(), r.path.as_str()) {
            ("POST", "/api/logger/uploads") => (200, r#"{"id":5,"received":[]}"#.into()),
            ("PUT", _) => (204, String::new()),
            _ => (
                if code == "hash_mismatch" { 422 } else { 409 },
                format!(r#"{{"code":"{code}"}}"#),
            ),
        });
        let r = step(&s.queue, &api(&stub), &Throttle::new(0), NO_PROGRESS).await;
        assert!(matches!(r, Step::Retrying { .. }), "{code}: {r:?}");
        let q = s.queue.lock().unwrap();
        let it = &q.items()[0];
        assert_eq!(it.state, State::Waiting, "{code}");
        if code == "hash_mismatch" {
            // Cut and sent again from scratch.
            assert!(it.prepared.is_none());
            assert!(!q.chunk_dir(&it.sha256).exists());
        }
    }
}

#[tokio::test]
async fn every_segment_of_one_file_carries_its_session() {
    let s = setup("raid_night.txt", Origin::Live);
    let stub = Stub::start(happy);
    let throttle = Throttle::new(0);
    let api = api(&stub);
    for _ in 0..2 {
        let r = step(&s.queue, &api, &throttle, NO_PROGRESS).await;
        assert!(matches!(r, Step::Uploaded { .. }), "{r:?}");
    }
    let bodies: Vec<serde_json::Value> = stub
        .taken()
        .iter()
        .filter(|r| r.method == "POST" && r.path == "/api/logger/uploads")
        .map(|r| serde_json::from_slice(&r.body).unwrap())
        .collect();
    assert_eq!(bodies.len(), 2);
    let key = bodies[0]["session_key"].as_str().unwrap().to_string();
    assert_eq!(key.len(), 64);
    assert!(bodies.iter().all(|b| b["session_key"] == key.as_str()));
    // The file's name and its first line's time; never its folder.
    assert!(bodies
        .iter()
        .all(|b| b["file_name"] == "WoWCombatLog-092826_200101.txt"));
    assert_eq!(
        bodies[0]["file_start_time"],
        "2026-09-28T20:01:02.100+01:00"
    );
    let folder = s.log.parent().unwrap().to_string_lossy().into_owned();
    for b in &bodies {
        assert!(!b.to_string().contains(&folder.replace('\\', "\\\\")));
    }

    // Another file is another log.
    let other = write_file(
        s.log.parent().unwrap(),
        "WoWCombatLog-092926_200101.txt",
        &fixture("mplus_key.txt"),
    );
    let seg = segments(&other).remove(0);
    let item = Item::new(Origin::Live, other, seg, Visibility::Guild, "eu");
    assert!(item.session_key.is_some());
    assert_ne!(item.session_key.as_deref(), Some(key.as_str()));
}

#[tokio::test]
async fn a_pull_a_raid_member_already_uploaded_is_not_sent() {
    let s = setup("raid_night.txt", Origin::Live);
    let (wipe, kill) = {
        let q = s.queue.lock().unwrap();
        (q.items()[0].clone(), q.items()[1].clone())
    };
    // Chunks written when the pull ended, as for every live pull.
    prepare_now(&s.queue, &wipe.sha256).unwrap();
    let stub = Stub::start(|r| match r.path.as_str() {
        ASK => (
            200,
            r#"{"fingerprints":[
                {"status":"have","fight_id":31,"url":"/logs/3/pulls/31/","log_url":"/logs/3/",
                 "boss_url":"/logs/3/bosses/3129/"},
                {"status":"need"}]}"#
                .into(),
        ),
        _ => happy(r),
    });
    let api = api(&stub);
    let throttle = Throttle::new(0);

    let r = step(&s.queue, &api, &throttle, NO_PROGRESS).await;
    assert_eq!(
        r,
        Step::AlreadyThere {
            sha256s: vec![wipe.sha256.clone()]
        }
    );
    // What was asked: each pull's boss, start and roster hash.
    let asks = stub.taken();
    assert_eq!(asks.len(), 1);
    let asked: serde_json::Value = serde_json::from_slice(&asks[0].body).unwrap();
    assert_eq!(
        asked["fingerprints"][0],
        serde_json::json!({
            "kind": "encounter",
            "encounter_id": 3129,
            "start_time": "2026-09-28T20:05:00.000+01:00",
            "roster_hash": wipe.segment.roster_hash.clone().unwrap(),
        })
    );
    {
        let q = s.queue.lock().unwrap();
        let it = q.get(&wipe.sha256).unwrap();
        assert_eq!(it.state, State::Done);
        assert_eq!(it.upload_id, None, "nothing was uploaded");
        let already = it.already.as_ref().unwrap();
        assert_eq!(already.url.as_deref(), Some("/logs/3/pulls/31/"));
        assert_eq!(already.log_url.as_deref(), Some("/logs/3/"));
        assert!(!q.chunk_dir(&wipe.sha256).exists(), "its chunks are gone");
    }

    // The other is sent as usual, and nobody is asked twice.
    let r = step(&s.queue, &api, &throttle, NO_PROGRESS).await;
    assert!(matches!(r, Step::Uploaded { ref sha256, .. } if *sha256 == kill.sha256));
    let reqs = stub.taken();
    assert!(reqs.iter().all(|r| r.path != ASK));
    let created: serde_json::Value = serde_json::from_slice(&reqs[0].body).unwrap();
    assert_eq!(created["sha256"], kill.sha256);
    assert_eq!(
        step(&s.queue, &api, &throttle, NO_PROGRESS).await,
        Step::Idle
    );
    assert!(stub.taken().is_empty());
}

#[tokio::test]
async fn a_server_without_fingerprints_gets_every_upload() {
    let s = setup("raid_night.txt", Origin::Live);
    let stub = Stub::start(|r| match r.path.as_str() {
        ASK => (404, r#"{"code":"not_found"}"#.into()),
        _ => happy(r),
    });
    let api = api(&stub);
    let throttle = Throttle::new(0);
    for _ in 0..2 {
        let r = step(&s.queue, &api, &throttle, NO_PROGRESS).await;
        assert!(matches!(r, Step::Uploaded { .. }), "{r:?}");
    }
    let asks = stub.taken().iter().filter(|r| r.path == ASK).count();
    assert_eq!(asks, 1, "asked once, then sent as before");
}

#[tokio::test]
async fn offline_asking_backs_off_with_the_upload() {
    let s = setup("raid_night.txt", Origin::Live);
    let stub = Stub::start(|_| (503, "{}".into()));
    let r = step(&s.queue, &api(&stub), &Throttle::new(0), NO_PROGRESS).await;
    assert!(matches!(r, Step::Retrying { .. }), "{r:?}");
    let q = s.queue.lock().unwrap();
    let first = &q.items()[0];
    assert!(
        !first.fingerprint_asked,
        "asked again when it's tried again"
    );
    assert!(first.next_try_ms > 0);
}

// -- past logs: kills and each boss's best wipe ------------------------------------------

/// A raid night of made-up pulls on one boss: each a wipe (with the boss's
/// health left, from a swing's advanced fields) or a kill.
fn raid_night(pulls: &[Option<u64>]) -> String {
    let mut s = String::from(
        "9/28/2026 20:01:02.1001  COMBAT_LOG_VERSION,22,ADVANCED_LOG_ENABLED,1,BUILD_VERSION,12.1.0,PROJECT_ID,1\n",
    );
    for (i, hp) in pulls.iter().enumerate() {
        let m = 5 + i * 5;
        s.push_str(&format!(
            "9/28/2026 20:{m:02}:00.0001  ENCOUNTER_START,3129,\"Plexus Sentinel\",16,20,2810\n\
             9/28/2026 20:{m:02}:00.0101  COMBATANT_INFO,Player-1403-0A000001,1,100,200,300,400\n\
             9/28/2026 20:{m:02}:00.0102  COMBATANT_INFO,Player-1403-0A000002,1,100,200,300,400\n\
             9/28/2026 20:{m:02}:30.0001  SWING_DAMAGE,Player-1403-0A000002,\"Player2-TarrenMill-EU\",0x512,0x0,Creature-0-1-2810-1-233814-0000{i:02}0001,\"Plexus Sentinel\",0x10a48,0x0,Creature-0-1-2810-1-233814-0000{i:02}0001,0000000000000000,{},1000000000\n\
             9/28/2026 20:{:02}:00.0001  ENCOUNTER_END,3129,\"Plexus Sentinel\",16,20,{},{}\n",
            hp.unwrap_or(0),
            m + 3,
            u8::from(hp.is_none()),
            180_000 + i * 1000,
        ));
    }
    s
}

fn past_log(text: &str, mode: BacklogPulls) -> (tempfile::TempDir, Mutex<Queue>, Vec<Item>) {
    let tmp = tempfile::tempdir().unwrap();
    let log = write_file(
        tmp.path(),
        "WoWCombatLog-092826_200101.txt",
        &common::to_crlf(text.as_bytes()),
    );
    let report = analyse(&describe(&log).unwrap(), |_| true).unwrap();
    let items = queue_items(&report, mode, Visibility::Public, "eu", |_| false);
    let mut q = Queue::load(&tmp.path().join("data"));
    for i in &items {
        q.add(i.clone());
    }
    (tmp, Mutex::new(q), items)
}

const SUMMARIES: &str = "/api/logger/pull-summaries";

fn stored_all(r: &Request) -> (u16, String) {
    let body: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
    let answers: Vec<_> = body["summaries"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .map(|(n, _)| {
            serde_json::json!({"status": "stored", "upload_id": 70 + n, "fight_id": 90 + n,
                "url": format!("/logs/5/pulls/{}/", 90 + n), "log_url": "/logs/5/",
                "boss_url": "/logs/5/bosses/3129/"})
        })
        .collect();
    (200, serde_json::json!({ "summaries": answers }).to_string())
}

#[tokio::test]
async fn a_past_logs_other_wipes_go_as_summaries_in_one_request() {
    // Wipes at 80%, 23.4% and 51%, then the kill.
    let text = raid_night(&[
        Some(800_000_000),
        Some(234_000_000),
        Some(510_000_000),
        None,
    ]);
    let (_tmp, queue, items) = past_log(&text, BacklogPulls::KillsAndBestWipe);
    let summarised: Vec<bool> = items.iter().map(|i| i.summary).collect();
    assert_eq!(summarised, [true, false, true, false]);
    let stub = Stub::start(|r| match r.path.as_str() {
        SUMMARIES => stored_all(r),
        _ => happy(r),
    });
    let (api, throttle) = (api(&stub), Throttle::new(0));

    let mut steps = Vec::new();
    loop {
        match step(&queue, &api, &throttle, NO_PROGRESS).await {
            Step::Idle => break,
            s => steps.push(s),
        }
    }
    assert_eq!(
        steps[0],
        Step::Summarised {
            sha256s: vec![items[0].sha256.clone(), items[2].sha256.clone()]
        }
    );
    let taken = stub.taken();
    // Only the full ones are asked about, and sent: the best wipe and the kill.
    let asked: serde_json::Value =
        serde_json::from_slice(&taken.iter().find(|r| r.path == ASK).unwrap().body).unwrap();
    assert_eq!(asked["fingerprints"].as_array().unwrap().len(), 2);
    let creates = taken
        .iter()
        .filter(|r| r.path == "/api/logger/uploads")
        .count();
    assert_eq!(creates, 2);
    let sent: Vec<_> = taken.iter().filter(|r| r.path == SUMMARIES).collect();
    assert_eq!(sent.len(), 1, "one request for the file's summaries");
    let body: serde_json::Value = serde_json::from_slice(&sent[0].body).unwrap();
    assert_eq!(body["visibility"], "public");
    assert_eq!(body["region"], "eu");
    assert_eq!(body["file_name"], "WoWCombatLog-092826_200101.txt");
    assert_eq!(body["session_key"], items[0].session_key.clone().unwrap());
    assert_eq!(
        body["summaries"][0],
        serde_json::json!({
            "encounter_id": 3129,
            "name": "Plexus Sentinel",
            "difficulty": 16,
            "group_size": 20,
            "instance_id": 2810,
            "start_time": "2026-09-28T20:05:00.000+01:00",
            "end_time": "2026-09-28T20:08:00.000+01:00",
            "duration_ms": 180_000,
            "boss_hp_pct": 80.0,
            "success": false,
            "roster_hash": items[0].segment.roster_hash.clone().unwrap(),
        })
    );
    assert_eq!(body["summaries"][1]["boss_hp_pct"], 51.0);
    let q = queue.lock().unwrap();
    let first = q.get(&items[0].sha256).unwrap();
    assert_eq!(first.state, State::Done);
    assert_eq!(first.upload_id.as_deref(), Some("70"));
    assert_eq!(
        first.summarised.as_ref().unwrap().url.as_deref(),
        Some("/logs/5/pulls/90/")
    );
    assert!(!q.chunk_dir(&items[0].sha256).exists(), "never compressed");
    assert_eq!(q.counts().backlog_done, 4);
}

#[tokio::test]
async fn all_pulls_sends_every_wipe_in_full() {
    let text = raid_night(&[Some(800_000_000), Some(234_000_000), None]);
    let (_tmp, queue, items) = past_log(&text, BacklogPulls::All);
    assert!(items.iter().all(|i| !i.summary));
    let stub = Stub::start(happy);
    let (api, throttle) = (api(&stub), Throttle::new(0));
    while step(&queue, &api, &throttle, NO_PROGRESS).await != Step::Idle {}
    let taken = stub.taken();
    assert!(taken.iter().all(|r| r.path != SUMMARIES));
    assert_eq!(
        taken
            .iter()
            .filter(|r| r.path == "/api/logger/uploads")
            .count(),
        3
    );
}

#[tokio::test]
async fn a_summary_the_site_has_in_full_links_that_copy() {
    let text = raid_night(&[Some(800_000_000), Some(234_000_000)]);
    let (_tmp, queue, items) = past_log(&text, BacklogPulls::KillsAndBestWipe);
    let stub = Stub::start(|r| {
        match r.path.as_str() {
        SUMMARIES => (
            200,
            r#"{"summaries":[{"status":"have","upload_id":null,"fight_id":31,
                "url":"/logs/3/pulls/31/","log_url":"/logs/3/","boss_url":"/logs/3/bosses/3129/"}]}"#
                .into(),
        ),
        _ => happy(r),
    }
    });
    let r = step(&queue, &api(&stub), &Throttle::new(0), NO_PROGRESS).await;
    assert!(matches!(r, Step::Summarised { .. }), "{r:?}");
    let q = queue.lock().unwrap();
    let it = q.get(&items[0].sha256).unwrap();
    assert_eq!(it.state, State::Done);
    assert_eq!(it.upload_id, None);
    assert_eq!(
        it.already.as_ref().unwrap().url.as_deref(),
        Some("/logs/3/pulls/31/")
    );
}

#[tokio::test]
async fn a_server_without_summaries_gets_those_wipes_in_full() {
    let text = raid_night(&[Some(800_000_000), Some(234_000_000)]);
    let (_tmp, queue, items) = past_log(&text, BacklogPulls::KillsAndBestWipe);
    assert!(items[0].summary);
    let stub = Stub::start(|r| match r.path.as_str() {
        SUMMARIES => (404, r#"{"detail":"Not Found"}"#.into()),
        _ => happy(r),
    });
    let (api, throttle) = (api(&stub), Throttle::new(0));
    while step(&queue, &api, &throttle, NO_PROGRESS).await != Step::Idle {}
    let q = queue.lock().unwrap();
    assert!(q
        .items()
        .iter()
        .all(|i| i.state == State::Done && !i.summary));
    let creates = stub
        .taken()
        .iter()
        .filter(|r| r.path == "/api/logger/uploads")
        .count();
    assert_eq!(creates, 2, "both wipes, in full");
}

#[tokio::test]
async fn busy_summaries_wait_and_are_sent_again() {
    let text = raid_night(&[Some(800_000_000), Some(234_000_000)]);
    let (_tmp, queue, items) = past_log(&text, BacklogPulls::KillsAndBestWipe);
    let stub = Stub::start(|r| match r.path.as_str() {
        SUMMARIES => (503, "{}".into()),
        _ => happy(r),
    });
    let r = step(&queue, &api(&stub), &Throttle::new(0), NO_PROGRESS).await;
    assert!(matches!(r, Step::Retrying { .. }), "{r:?}");
    let q = queue.lock().unwrap();
    let it = q.get(&items[0].sha256).unwrap();
    assert!(it.summary && it.state == State::Waiting && it.next_try_ms > 0);
    assert_eq!(it.attempts, 1);
}

/// The same pull, as an app from before zone lines split it (no ZONE_CHANGE
/// carried) and as this one does: different bytes, so a different SHA-256,
/// but the same fingerprint, so the site still answers "have" for a copy the
/// older app sent, and the parser groups the two as one pull.
#[test]
fn a_carried_zone_line_leaves_the_fingerprint_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let bytes = fixture("raid_night.txt");
    let name = "WoWCombatLog-092826_200101.txt";
    let new_log = write_file(tmp.path(), name, &bytes);
    let old_dir = tmp.path().join("old");
    std::fs::create_dir_all(&old_dir).unwrap();
    // What an older app sent: header and range, no zone line.
    let mut old_segs = segments(&new_log);
    for seg in &mut old_segs {
        seg.zone_line = None;
    }
    let new_segs = segments(&new_log);
    assert_eq!(new_segs.len(), 2);
    for (old, new) in old_segs.iter().zip(&new_segs) {
        assert!(new.zone_line.is_some());
        assert_ne!(old.prefix(), new.prefix());
        let old_item = Item::new(
            Origin::Live,
            old_dir.join(name),
            old.clone(),
            Visibility::Public,
            "eu",
        );
        let new_item = Item::new(
            Origin::Live,
            new_log.clone(),
            new.clone(),
            Visibility::Public,
            "eu",
        );
        let print = fingerprint_of(&new_item).expect("a whole pull with a roster");
        assert_eq!(fingerprint_of(&old_item), Some(print));
        assert_eq!(old.start_time, new.start_time);
        assert_eq!(old.end_time, new.end_time);
    }
    // And a file with no ZONE_CHANGE at all gives the same fingerprints too.
    let text = String::from_utf8(bytes.clone()).unwrap();
    let without: String = text
        .split_inclusive('\n')
        .filter(|l| !l.contains("  ZONE_CHANGE,"))
        .collect();
    let bare_log = write_file(&old_dir, name, without.as_bytes());
    let bare_segs = segments(&bare_log);
    for (bare, new) in bare_segs.iter().zip(&new_segs) {
        assert_eq!(bare.zone_line, None);
        assert_ne!(bare.sha256, new.sha256);
        let a = Item::new(
            Origin::Backlog,
            bare_log.clone(),
            bare.clone(),
            Visibility::Public,
            "eu",
        );
        let b = Item::new(
            Origin::Backlog,
            new_log.clone(),
            new.clone(),
            Visibility::Public,
            "eu",
        );
        assert_eq!(fingerprint_of(&a), fingerprint_of(&b));
    }
    // A queue saved before zone lines still loads: the segment has none.
    let mut saved = serde_json::to_value(&new_segs[0]).unwrap();
    saved.as_object_mut().unwrap().remove("zone_line");
    let loaded: Segment = serde_json::from_value(saved).unwrap();
    assert_eq!(loaded.zone_line, None);
}
