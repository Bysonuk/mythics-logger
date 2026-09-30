mod common;

use common::{fixture, write_file};
use mythics_logger_core::backlog::{analyse, describe, find_logs, ReportCache};
use mythics_logger_core::splitter::Kind;
use std::fs::OpenOptions;
use std::time::{Duration, SystemTime};

fn age(p: &std::path::Path, secs: u64) {
    let f = OpenOptions::new().append(true).open(p).unwrap();
    f.set_modified(SystemTime::now() - Duration::from_secs(secs))
        .unwrap();
}

#[test]
fn finds_every_log_including_archived_and_renamed_ones() {
    let tmp = tempfile::tempdir().unwrap();
    let logs = tmp.path();
    let a = write_file(
        logs,
        "WoWCombatLog-092826_200101.txt",
        &fixture("raid_night.txt"),
    );
    let b = write_file(logs, "WoWCombatLog.txt", &fixture("old_version.txt"));
    std::fs::create_dir(logs.join("warcraftlogsarchive")).unwrap();
    let c = write_file(
        &logs.join("warcraftlogsarchive"),
        "WoWCombatLog-archive-092726.txt",
        &fixture("mplus_key.txt"),
    );
    write_file(logs, "Client.log", b"not a combat log");
    write_file(
        logs,
        "WoWCombatLog-092826_200101.zip",
        b"zipped: not read yet",
    );
    age(&a, 10);
    age(&b, 1000);
    age(&c, 100);
    let found = find_logs(logs);
    let names: Vec<_> = found.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "WoWCombatLog-092826_200101.txt",
            "WoWCombatLog-archive-092726.txt",
            "WoWCombatLog.txt"
        ]
    );
}

#[test]
fn reads_a_files_pulls_and_keys_before_sending_anything() {
    let tmp = tempfile::tempdir().unwrap();
    let mut body = fixture("raid_night.txt");
    body.extend(fixture("mplus_key.txt"));
    let p = write_file(tmp.path(), "WoWCombatLog-092826_200101.txt", &body);
    let f = describe(&p).unwrap();
    let mut progress = Vec::new();
    let r = analyse(&f, |n| {
        progress.push(n);
        true
    })
    .unwrap();
    assert!(r.complete);
    assert_eq!(
        r.encounters, 6,
        "two raid pulls plus four bosses in the key"
    );
    assert_eq!(r.keys, 1);
    let kinds: Vec<_> = r.segments.iter().map(|s| s.kind).collect();
    assert_eq!(kinds, [Kind::Encounter, Kind::Encounter, Kind::Key]);
    assert_eq!(
        r.first_time.as_deref(),
        Some("2026-09-28T20:05:00.000+01:00")
    );
    assert_eq!(r.header.as_ref().unwrap().advanced, Some(true));
    assert_eq!(*progress.last().unwrap(), body.len() as u64);
    assert_eq!(r.uploadable().count(), 3);
}

#[test]
fn old_logs_take_their_year_from_the_file_name() {
    let tmp = tempfile::tempdir().unwrap();
    let p = write_file(
        tmp.path(),
        "WoWCombatLog-112120_120130.txt",
        &fixture("old_version.txt"),
    );
    let r = analyse(&describe(&p).unwrap(), |_| true).unwrap();
    assert_eq!(r.first_time.as_deref(), Some("2020-11-21T12:01:34.071"));
    assert!(r.header.is_none());
    assert_eq!(r.segments.len(), 1);
}

#[test]
fn cached_reports_are_used_until_the_file_changes() {
    let tmp = tempfile::tempdir().unwrap();
    let p = write_file(tmp.path(), "WoWCombatLog.txt", &fixture("raid_night.txt"));
    let f = describe(&p).unwrap();
    let mut cache = ReportCache::default();
    cache.put(analyse(&f, |_| true).unwrap());
    let cache_file = tmp.path().join("reports.json");
    cache.save(&cache_file).unwrap();
    let cache = ReportCache::load(&cache_file);
    assert!(cache.get(&f).is_some());
    std::fs::write(&p, fixture("mplus_key.txt")).unwrap();
    assert!(cache.get(&describe(&p).unwrap()).is_none());
}

#[test]
fn reports_cached_by_an_older_app_are_read_again() {
    // Before version 2, a pull's boss health was the app's own guess, not
    // the server's rule, so its best wipe could be the wrong one.
    let tmp = tempfile::tempdir().unwrap();
    let p = write_file(tmp.path(), "WoWCombatLog.txt", &fixture("raid_night.txt"));
    let f = describe(&p).unwrap();
    let mut cache = ReportCache::default();
    cache.put(analyse(&f, |_| true).unwrap());
    let cache_file = tmp.path().join("reports.json");
    cache.save(&cache_file).unwrap();
    let mut old: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&cache_file).unwrap()).unwrap();
    old.as_object_mut().unwrap().remove("version");
    std::fs::write(&cache_file, serde_json::to_vec(&old).unwrap()).unwrap();
    assert!(ReportCache::load(&cache_file).get(&f).is_none());
}

#[test]
fn a_cancelled_read_is_not_cached() {
    let tmp = tempfile::tempdir().unwrap();
    // Over 8 MB, so progress is reported part-way and can stop it.
    let mut body = fixture("raid_night.txt");
    while body.len() < 9 << 20 {
        body.extend(fixture("raid_night.txt"));
    }
    let p = write_file(tmp.path(), "WoWCombatLog.txt", &body);
    let f = describe(&p).unwrap();
    let r = analyse(&f, |_| false).unwrap();
    assert!(!r.complete);
    let mut cache = ReportCache::default();
    cache.put(r);
    assert!(cache.get(&f).is_none());
}
