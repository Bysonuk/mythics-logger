//! Archiving finished logs (`archive.rs`): made-up logs in temporary
//! folders, never a real one.

mod common;

use mythics_logger_core::archive::{
    self, clean_up, folder, folder_size, is_ours, ArchiveError, NotEligible, Options, Outcome,
    COMMENT,
};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const HOUR: Duration = Duration::from_secs(3600);

/// A Logs folder holding tonight's log (the newest, written now) and an
/// older finished one, last changed an hour ago.
struct Logs {
    dir: tempfile::TempDir,
    old: PathBuf,
    newest: PathBuf,
}

fn logs_with(body: &[u8]) -> Logs {
    let dir = tempfile::tempdir().unwrap();
    let old = dir.path().join("WoWCombatLog-092126_193000.txt");
    std::fs::write(&old, body).unwrap();
    set_age(&old, HOUR);
    let newest = dir.path().join("WoWCombatLog-092826_200101.txt");
    std::fs::write(&newest, common::fixture("raid_night.txt")).unwrap();
    Logs { dir, old, newest }
}

fn logs() -> Logs {
    logs_with(&common::fixture("raid_night.txt"))
}

fn set_age(p: &Path, age: Duration) {
    let f = std::fs::OpenOptions::new().write(true).open(p).unwrap();
    f.set_modified(SystemTime::now() - age).unwrap();
}

fn opts() -> Options {
    Options {
        bytes_per_sec: 0,
        ..Options::default()
    }
}

fn run(l: &Logs, file: &Path, o: &Options, pending: bool) -> Result<Outcome, ArchiveError> {
    archive::archive(file, l.dir.path(), o, pending, &mut |_, _| {})
}

/// The one entry in an archive: its name and contents.
fn unzip(p: &Path) -> (String, Vec<u8>, String) {
    let mut za = zip::ZipArchive::new(File::open(p).unwrap()).unwrap();
    let comment = String::from_utf8(za.comment().to_vec()).unwrap();
    assert_eq!(za.len(), 1);
    let mut e = za.by_index(0).unwrap();
    let mut out = Vec::new();
    e.read_to_end(&mut out).unwrap();
    (e.name().to_string(), out, comment)
}

/// Made-up combat log lines (fake players), `n` bytes or a little more.
fn synthetic(n: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(n + 256);
    out.extend_from_slice(b"9/21/2026 19:30:00.0001  COMBAT_LOG_VERSION,22,ADVANCED_LOG_ENABLED,1,BUILD_VERSION,12.0.0,PROJECT_ID,1\r\n");
    let mut i = 0u64;
    while out.len() < n {
        write!(
            out,
            "9/21/2026 19:{:02}:{:02}.{:04}  SPELL_DAMAGE,Player-1403-0A{:06X},\"Player{}-TarrenMill-EU\",0x512,0x0,Creature-0-1-2810-1-233814-00001C0001,\"Plexus Sentinel\",0x10a48,0x0,{},{},{}\r\n",
            (i / 60_000) % 60,
            (i / 1000) % 60,
            i % 10_000,
            i % 20,
            i % 20 + 1,
            i * 7919 % 100_000,
            i % 3,
            i * 31 % 9973
        )
        .unwrap();
        i += 1;
    }
    out
}

#[test]
fn a_finished_log_is_zipped_checked_and_the_original_deleted() {
    let l = logs();
    let original = std::fs::read(&l.old).unwrap();
    let mut seen = Vec::new();
    let out = archive::archive(&l.old, l.dir.path(), &opts(), false, &mut |d, of| {
        seen.push((d, of))
    })
    .unwrap();
    let zip = folder(l.dir.path()).join("WoWCombatLog-092126_193000.zip");
    match &out {
        Outcome::Archived {
            zip: z,
            original_size,
            zip_size,
        } => {
            assert_eq!(z, &zip);
            assert_eq!(*original_size, original.len() as u64);
            assert!(*zip_size > 0 && *zip_size < *original_size);
        }
        other => panic!("{other:?}"),
    }
    assert!(!l.old.exists(), "the original is deleted");
    assert!(l.newest.exists(), "the newest log is untouched");
    let (name, body, comment) = unzip(&zip);
    assert_eq!(name, "WoWCombatLog-092126_193000.txt");
    assert_eq!(body, original, "the zip holds the log byte for byte");
    assert_eq!(comment, COMMENT);
    assert!(is_ours(&zip));
    // Nothing left behind but the archive.
    let left: Vec<_> = std::fs::read_dir(folder(l.dir.path()))
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(left, ["WoWCombatLog-092126_193000.zip"]);
    // Progress through writing, then checking: twice the log's size.
    let total = 2 * original.len() as u64;
    assert!(seen.iter().all(|&(_, of)| of == total));
    assert_eq!(seen.last().unwrap().0, total);
}

#[test]
fn streams_a_big_log_through_a_small_buffer_with_zip64() {
    // 8 MB through a 4 KB buffer: the streaming path, two thousand times
    // round. ZIP64 from 1 MB, so its sizes are written as they would be for
    // a log over 4 GB.
    let body = synthetic(8 << 20);
    let l = logs_with(&body);
    let o = Options {
        chunk_size: 4096,
        zip64_from: 1 << 20,
        ..opts()
    };
    let mut calls = 0u32;
    archive::archive(&l.old, l.dir.path(), &o, false, &mut |_, _| calls += 1).unwrap();
    assert!(calls >= 2 * (8 << 20) / 4096, "{calls} progress calls");
    let zip = folder(l.dir.path()).join("WoWCombatLog-092126_193000.zip");
    let (_, back, _) = unzip(&zip);
    assert!(back == body, "the zip holds the log byte for byte");
    let ratio = body.len() as f64 / std::fs::metadata(&zip).unwrap().len() as f64;
    assert!(ratio > 5.0, "combat logs shrink a lot: {ratio:.1}:1");
}

#[test]
fn a_bad_archive_keeps_the_original_and_removes_the_zip() {
    let l = logs();
    let original = std::fs::read(&l.old).unwrap();
    let crc = crc32fast::hash(&original);
    // Archive a copy, then damage its stored data.
    let copy = tempfile::tempdir().unwrap();
    let logs2 = copy.path();
    let f = logs2.join("WoWCombatLog-092126_193000.txt");
    std::fs::write(&f, &original).unwrap();
    set_age(&f, HOUR);
    std::fs::write(logs2.join("WoWCombatLog-092826_200101.txt"), b"x").unwrap();
    archive::archive(&f, logs2, &opts(), false, &mut |_, _| {}).unwrap();
    let good = folder(logs2).join("WoWCombatLog-092126_193000.zip");
    let mut bytes = std::fs::read(&good).unwrap();
    let mid = bytes.len() / 2;
    bytes[mid] ^= 0xFF;
    let bad = copy.path().join("bad.zip");
    std::fs::write(&bad, &bytes).unwrap();
    let r = archive::verify(
        &bad,
        "WoWCombatLog-092126_193000.txt",
        crc,
        original.len() as u64,
        &opts(),
        &mut |_| {},
    );
    assert!(matches!(r, Err(ArchiveError::Verify)), "{r:?}");
    // A good archive, but of other bytes: refused too.
    let r = archive::verify(
        &good,
        "WoWCombatLog-092126_193000.txt",
        crc ^ 1,
        original.len() as u64,
        &opts(),
        &mut |_| {},
    );
    assert!(matches!(r, Err(ArchiveError::Verify)), "{r:?}");

    // And through `archive`: an archive of that name that isn't this log
    // (the damaged one) means the original stays, and no new zip is left.
    let dest = folder(l.dir.path());
    std::fs::create_dir_all(&dest).unwrap();
    std::fs::write(dest.join("WoWCombatLog-092126_193000.zip"), &bytes).unwrap();
    let r = run(&l, &l.old, &opts(), false);
    assert!(matches!(r, Err(ArchiveError::Exists)), "{r:?}");
    assert_eq!(
        std::fs::read(&l.old).unwrap(),
        original,
        "the original stays"
    );
    let left: Vec<_> = std::fs::read_dir(&dest)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(left, ["WoWCombatLog-092126_193000.zip"], "no partial zip");
}

#[test]
fn a_failed_check_keeps_the_original_and_leaves_no_zip() {
    let l = logs();
    let original = std::fs::read(&l.old).unwrap();
    let o = Options {
        damage_before_check: true,
        ..opts()
    };
    let r = run(&l, &l.old, &o, false);
    assert!(matches!(r, Err(ArchiveError::Verify)), "{r:?}");
    assert_eq!(r.unwrap_err().code(), "archive_verify");
    assert_eq!(
        std::fs::read(&l.old).unwrap(),
        original,
        "the original stays"
    );
    let left = std::fs::read_dir(folder(l.dir.path())).unwrap().count();
    assert_eq!(left, 0, "the partial zip is removed");
    // And it goes fine the next time.
    assert!(matches!(
        run(&l, &l.old, &opts(), false),
        Ok(Outcome::Archived { .. })
    ));
}

#[test]
fn an_archive_left_by_a_stop_before_the_delete_finishes_the_job() {
    let l = logs();
    let original = std::fs::read(&l.old).unwrap();
    run(&l, &l.old, &opts(), false).unwrap();
    // The app stopped between the rename and the delete: the log is back.
    std::fs::write(&l.old, &original).unwrap();
    set_age(&l.old, HOUR);
    let out = run(&l, &l.old, &opts(), false).unwrap();
    assert!(matches!(out, Outcome::Archived { .. }));
    assert!(!l.old.exists());
}

#[test]
fn the_newest_a_recent_a_queued_and_an_elsewhere_log_are_refused() {
    let l = logs();
    let refused = |r: Result<Outcome, ArchiveError>| match r {
        Err(ArchiveError::NotEligible(n)) => n,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        refused(run(&l, &l.newest, &opts(), false)),
        NotEligible::Newest
    );
    // Even when it's been quiet a while: the game may log to it again.
    set_age(&l.newest, HOUR / 2);
    assert_eq!(
        refused(run(&l, &l.newest, &opts(), false)),
        NotEligible::Newest
    );
    set_age(&l.newest, Duration::ZERO);

    set_age(&l.old, Duration::from_secs(5 * 60));
    assert_eq!(
        refused(run(&l, &l.old, &opts(), false)),
        NotEligible::Recent
    );
    set_age(&l.old, HOUR);
    assert_eq!(refused(run(&l, &l.old, &opts(), true)), NotEligible::Queued);

    let other = l.dir.path().join("warcraftlogsarchive");
    std::fs::create_dir_all(&other).unwrap();
    let theirs = other.join("WoWCombatLog-091426_190000.txt");
    std::fs::write(&theirs, b"x").unwrap();
    set_age(&theirs, HOUR);
    assert_eq!(
        refused(run(&l, &theirs, &opts(), false)),
        NotEligible::NotInLogs
    );
    assert!(l.old.exists() && l.newest.exists() && theirs.exists());
    assert!(!folder(l.dir.path()).exists(), "nothing was written");
}

#[cfg(windows)]
#[test]
fn a_log_another_program_has_open_is_refused() {
    let l = logs();
    // Another program reading it, sharing everything (as this app and the
    // Warcraft Logs uploader do).
    let held = mythics_logger_core::tailer::open_shared(&l.old).unwrap();
    match run(&l, &l.old, &opts(), false) {
        Err(ArchiveError::NotEligible(NotEligible::InUse)) => {}
        other => panic!("{other:?}"),
    }
    assert!(l.old.exists());
    drop(held);
    assert!(matches!(
        run(&l, &l.old, &opts(), false),
        Ok(Outcome::Archived { .. })
    ));
}

#[test]
fn a_log_already_moved_is_skipped_quietly() {
    let l = logs();
    std::fs::remove_file(&l.old).unwrap();
    assert_eq!(run(&l, &l.old, &opts(), false).unwrap(), Outcome::Vanished);
    assert!(!folder(l.dir.path()).exists());
}

#[test]
fn clean_up_deletes_only_our_old_archives() {
    let l = logs();
    let original = std::fs::read(&l.old).unwrap();
    run(&l, &l.old, &opts(), false).unwrap();
    let dir = folder(l.dir.path());
    let ours_old = dir.join("WoWCombatLog-092126_193000.zip");
    // A second archive of ours, recent.
    let recent_log = l.dir.path().join("WoWCombatLog-092726_190000.txt");
    std::fs::write(&recent_log, &original).unwrap();
    set_age(&recent_log, HOUR);
    run(&l, &recent_log, &opts(), false).unwrap();
    let ours_new = dir.join("WoWCombatLog-092726_190000.zip");
    // Someone else's zip with a combat log's name, a stray file, and a zip
    // with our comment but not our name: none of them ours.
    let theirs = dir.join("WoWCombatLog-010126_120000.zip");
    {
        let mut zw = zip::ZipWriter::new(File::create(&theirs).unwrap());
        zw.start_file("x.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        zw.write_all(b"x").unwrap();
        zw.finish().unwrap();
    }
    let renamed = dir.join("holiday.zip");
    std::fs::copy(&ours_old, &renamed).unwrap();
    let notes = dir.join("WoWCombatLog-notes.zip");
    std::fs::write(&notes, b"not a zip").unwrap();
    for p in [&ours_old, &theirs, &renamed, &notes] {
        set_age(p, Duration::from_secs(40 * 86_400));
    }
    set_age(&ours_new, Duration::from_secs(10 * 86_400));

    let before = folder_size(l.dir.path());
    assert_eq!(before.files, 5);
    let freed = std::fs::metadata(&ours_old).unwrap().len();
    let r = clean_up(
        l.dir.path(),
        Duration::from_secs(30 * 86_400),
        SystemTime::now(),
    );
    assert_eq!((r.deleted, r.freed), (1, freed));
    assert!(!ours_old.exists());
    assert!(ours_new.exists(), "not old enough");
    assert!(theirs.exists() && renamed.exists() && notes.exists());
    let after = folder_size(l.dir.path());
    assert_eq!((after.files, after.bytes), (4, before.bytes - freed));
}

#[test]
fn the_folder_size_counts_every_file_in_it() {
    let l = logs();
    assert_eq!(folder_size(l.dir.path()), archive::FolderSize::default());
    let dir = folder(l.dir.path());
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("a.zip"), vec![0u8; 1000]).unwrap();
    std::fs::write(dir.join("b.zip"), vec![0u8; 234]).unwrap();
    let s = folder_size(l.dir.path());
    assert_eq!((s.files, s.bytes), (2, 1234));
}

/// A few hundred megabytes, in release builds only (debug-mode deflate is
/// slow): `cargo test --release --test archive -- --ignored`.
#[test]
#[ignore]
fn a_few_hundred_megabytes() {
    let body = synthetic(300 << 20);
    let l = logs_with(&body);
    drop(body);
    let out = run(&l, &l.old, &Options::default(), false).unwrap();
    assert!(matches!(out, Outcome::Archived { .. }));
}
