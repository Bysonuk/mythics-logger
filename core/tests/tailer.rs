mod common;

use common::{fixture, write_file};
use mythics_logger_core::splitter::{Segment, Splitter};
use mythics_logger_core::tailer::{last_zone_change, live_start, Event, Tailer};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

#[derive(Debug, PartialEq)]
enum Seen {
    Opened(String, u64, bool),
    Line(u64, Vec<u8>),
    Restarted(String),
    Closed(String, u64),
}

fn name(p: &Path) -> String {
    p.file_name().unwrap().to_string_lossy().into_owned()
}

fn poll(t: &mut Tailer) -> Vec<Seen> {
    let mut out = Vec::new();
    t.poll(&mut |_, _, _| 0, &mut |e| {
        out.push(match e {
            Event::Opened {
                path,
                offset,
                header,
                ..
            } => Seen::Opened(name(path), offset, header.is_some()),
            Event::Line { offset, bytes } => Seen::Line(offset, bytes.to_vec()),
            Event::Restarted { path } => Seen::Restarted(name(path)),
            Event::Closed { path, end } => Seen::Closed(name(path), end),
        })
    })
    .unwrap();
    out
}

fn append(p: &Path, bytes: &[u8]) {
    let mut f = OpenOptions::new().append(true).open(p).unwrap();
    f.write_all(bytes).unwrap();
}

fn set_mtime(p: &Path, secs_ago: u64) {
    let f = OpenOptions::new().append(true).open(p).unwrap();
    f.set_modified(SystemTime::now() - Duration::from_secs(secs_ago))
        .unwrap();
}

#[test]
fn holds_a_partial_line_until_its_newline_arrives() {
    let dir = tempfile::tempdir().unwrap();
    let p = write_file(
        dir.path(),
        "WoWCombatLog-092826_200000.txt",
        b"line one\r\nline tw",
    );
    let mut t = Tailer::new(dir.path());
    let seen = poll(&mut t);
    assert_eq!(
        seen,
        vec![
            Seen::Opened("WoWCombatLog-092826_200000.txt".into(), 0, false),
            Seen::Line(0, b"line one\r\n".to_vec()),
        ]
    );
    assert_eq!(
        t.position().unwrap().1,
        10,
        "the partial line isn't consumed yet"
    );
    // The game flushes the rest of the line, split across the CR and LF.
    append(&p, b"o\r");
    assert!(poll(&mut t).is_empty());
    append(&p, b"\nline three\r\n");
    assert_eq!(
        poll(&mut t),
        vec![
            Seen::Line(10, b"line two\r\n".to_vec()),
            Seen::Line(20, b"line three\r\n".to_vec()),
        ]
    );
    assert!(poll(&mut t).is_empty(), "a quiet file gives nothing");
}

#[test]
fn a_line_split_across_the_read_buffer() {
    // 1 MB buffer: a line straddling the edge must come out whole, once.
    let dir = tempfile::tempdir().unwrap();
    let mut body = Vec::new();
    let long = vec![b'x'; (1 << 20) - 5];
    body.extend_from_slice(&long);
    body.extend_from_slice(b"\r\nshort line\r\n");
    write_file(dir.path(), "WoWCombatLog.txt", &body);
    let mut t = Tailer::new(dir.path());
    let seen = poll(&mut t);
    assert_eq!(seen.len(), 3);
    assert_eq!(
        seen[2],
        Seen::Line((1 << 20) - 3, b"short line\r\n".to_vec())
    );
}

#[test]
fn moves_to_a_newer_file_after_finishing_the_old_one() {
    let dir = tempfile::tempdir().unwrap();
    let a = write_file(dir.path(), "WoWCombatLog-092826_200000.txt", b"a1\r\n");
    set_mtime(&a, 60);
    let mut t = Tailer::new(dir.path());
    poll(&mut t);
    // The old file gets its last lines (one without a newline) just as
    // logging restarts in a new file.
    append(&a, b"a2\r\na3");
    set_mtime(&a, 30);
    let b = write_file(dir.path(), "WoWCombatLog-092826_210000.txt", b"b1\r\n");
    set_mtime(&b, 0);
    assert_eq!(
        poll(&mut t),
        vec![
            Seen::Line(4, b"a2\r\n".to_vec()),
            Seen::Line(8, b"a3".to_vec()),
            Seen::Closed("WoWCombatLog-092826_200000.txt".into(), 10),
            Seen::Opened("WoWCombatLog-092826_210000.txt".into(), 0, false),
            Seen::Line(0, b"b1\r\n".to_vec()),
        ]
    );
    assert_eq!(
        t.current_path().map(name).as_deref(),
        Some("WoWCombatLog-092826_210000.txt")
    );
}

#[test]
fn starts_again_when_the_file_is_cleared() {
    let dir = tempfile::tempdir().unwrap();
    let p = write_file(dir.path(), "WoWCombatLog.txt", b"first\r\nsecond\r\n");
    let mut t = Tailer::new(dir.path());
    poll(&mut t);
    // Another uploader's "clear logs after upload".
    std::fs::write(&p, b"new\r\n").unwrap();
    assert_eq!(
        poll(&mut t),
        vec![
            Seen::Restarted("WoWCombatLog.txt".into()),
            Seen::Line(0, b"new\r\n".to_vec()),
        ]
    );
}

#[test]
fn ignores_files_that_are_not_combat_logs() {
    let dir = tempfile::tempdir().unwrap();
    write_file(dir.path(), "Client.log", b"x\r\n");
    write_file(dir.path(), "WoWCombatLog.txt.bak", b"x\r\n");
    let mut t = Tailer::new(dir.path());
    assert!(poll(&mut t).is_empty());
}

#[test]
fn resuming_part_way_hands_over_the_header() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = fixture("raid_night.txt");
    write_file(dir.path(), "WoWCombatLog-092826_200101.txt", &bytes);
    let mut t = Tailer::new(dir.path());
    let mut header = None;
    t.poll(&mut |_, _, _| 200, &mut |e| {
        if let Event::Opened { header: h, .. } = e {
            header = h;
        }
    })
    .unwrap();
    assert!(header.unwrap().ends_with(b"PROJECT_ID,1\r\n"));
}

/// The whole path, as the app runs it: the game writes the raid fixture in
/// awkward pieces (mid-line, mid-CRLF), the tailer follows, the splitter cuts.
/// The pulls must be exactly those of reading the finished file at once.
#[test]
fn tailing_in_pieces_gives_the_same_segments() {
    let bytes = fixture("raid_night.txt");
    let dir = tempfile::tempdir().unwrap();
    let p: PathBuf = write_file(dir.path(), "WoWCombatLog-092826_200101.txt", b"");
    let mut t = Tailer::new(dir.path());
    let mut splitter = Splitter::new();
    let mut live: Vec<Segment> = Vec::new();
    let mut pos = 0;
    for step in [7usize, 100, 1, 333, 2, 64, 999, 5, 4096] {
        let end = (pos + step).min(bytes.len());
        append(&p, &bytes[pos..end]);
        pos = end;
        t.poll(&mut |_, _, _| 0, &mut |e| {
            if let Event::Line { offset, bytes } = e {
                live.extend(splitter.feed(offset, bytes));
            }
        })
        .unwrap();
    }
    assert_eq!(pos, bytes.len());
    let whole = mythics_logger_core::splitter::split_file(&p, |_| {}, |_| true).unwrap();
    assert_eq!(whole.encounters_seen, 2);
    let mut expected = Vec::new();
    mythics_logger_core::splitter::split_file(&p, |s| expected.push(s), |_| true).unwrap();
    assert_eq!(live, expected);
}

#[test]
fn live_start_reads_fresh_files_and_leaves_old_ones() {
    let now = SystemTime::now();
    assert_eq!(live_start(5000, now - Duration::from_secs(30), now), 0);
    assert_eq!(live_start(5000, now - Duration::from_secs(3600), now), 5000);
}

#[test]
fn resuming_part_way_hands_over_the_latest_zone_change() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = fixture("raid_night.txt");
    write_file(dir.path(), "WoWCombatLog-092826_200101.txt", &bytes);
    let mut t = Tailer::new(dir.path());
    let mut zone = None;
    let second_pull = bytes
        .windows(21)
        .position(|w| w == b"20:15:00.0001  ENCOUN")
        .unwrap()
        - 10;
    t.poll(&mut |_, _, _| second_pull as u64, &mut |e| {
        if let Event::Opened { zone: z, .. } = e {
            zone = z;
        }
    })
    .unwrap();
    assert_eq!(
        zone.as_deref(),
        Some(&b"9/28/2026 20:01:02.1021  ZONE_CHANGE,2810,\"Manaforge Omega\",16\r\n"[..])
    );
    // Read from the start: nothing to look back for.
    let mut t = Tailer::new(dir.path());
    t.poll(&mut |_, _, _| 0, &mut |e| {
        if let Event::Opened { zone: z, .. } = e {
            assert_eq!(z, None);
        }
    })
    .unwrap();
}

/// Filler: one line of trash damage (fake players), about 200 bytes.
fn filler(n: usize) -> String {
    format!(
        "9/29/2026 19:{:02}:{:02}.{:04}  SPELL_DAMAGE,Player-1403-0A000001,\"Player1-TarrenMill-EU\",0x512,0x0,Creature-0-1-2810-1-240001-00002A0001,\"Trash\",0xa48,0x0,1,2,3\r\n",
        (n / 600) % 60,
        (n / 10) % 60,
        n % 10_000
    )
}

#[test]
fn last_zone_change_looks_back_across_blocks() {
    let dir = tempfile::tempdir().unwrap();
    let header = "9/29/2026 19:00:00.0001  COMBAT_LOG_VERSION,22,ADVANCED_LOG_ENABLED,1,BUILD_VERSION,12.1.0,PROJECT_ID,1\r\n";
    let city = "9/29/2026 19:00:00.0002  ZONE_CHANGE,2552,\"Khaz Algar (Surface)\",0\r\n";
    let raid = "9/29/2026 19:30:00.0001  ZONE_CHANGE,2810,\"Manaforge Omega\",16\r\n";
    let mut text = String::from(header);
    text.push_str(city);
    let mut n = 0;
    // Put the raid's line across the first 1 MB boundary back from the
    // end, then 3 MB of trash after it.
    while text.len() < (1 << 20) - 20 {
        text.push_str(&filler(n));
        n += 1;
    }
    let raid_at = text.len();
    text.push_str(raid);
    let after_raid = text.len();
    while text.len() < raid_at + (3 << 20) {
        text.push_str(&filler(n));
        n += 1;
    }
    let pull_at = text.len() as u64;
    text.push_str(
        "9/29/2026 20:05:00.0001  ENCOUNTER_START,3129,\"Plexus Sentinel\",16,20,2810\r\n",
    );
    let p = write_file(
        dir.path(),
        "WoWCombatLog-092926_190000.txt",
        text.as_bytes(),
    );

    let found = last_zone_change(&p, pull_at).unwrap();
    assert_eq!(found.as_deref(), Some(raid.as_bytes()));
    // Every boundary near the raid's line: before it the city, after it the raid.
    for back in [raid_at - 1, raid_at, raid_at + 1, after_raid - 1] {
        assert_eq!(
            last_zone_change(&p, back as u64).unwrap().as_deref(),
            Some(city.as_bytes()),
            "reading from {back}"
        );
    }
    assert_eq!(
        last_zone_change(&p, after_raid as u64).unwrap().as_deref(),
        Some(raid.as_bytes())
    );
    // From anywhere in the file, the same line splitting from the start finds.
    for cut in [pull_at - 7, pull_at - 1_000_003, pull_at - 2_500_001] {
        assert_eq!(
            last_zone_change(&p, cut).unwrap().as_deref(),
            Some(raid.as_bytes())
        );
    }
    // Before any zone change: none.
    assert_eq!(last_zone_change(&p, header.len() as u64).unwrap(), None);
    assert_eq!(last_zone_change(&p, 0).unwrap(), None);
}

/// Live, after a restart part-way through a raid: the pull that was open
/// goes with exactly the bytes, and so the SHA-256, of reading the file whole.
#[test]
fn a_pull_resumed_after_a_restart_matches_the_whole_file() {
    let bytes = fixture("raid_night.txt");
    let dir = tempfile::tempdir().unwrap();
    let p = write_file(dir.path(), "WoWCombatLog-092826_200101.txt", &bytes);
    let mut whole = Vec::new();
    mythics_logger_core::splitter::split_file(&p, |s| whole.push(s), |_| true).unwrap();
    for seg in &whole {
        let mut t = Tailer::new(dir.path());
        let mut splitter = Splitter::new();
        let mut live = Vec::new();
        t.poll(&mut |_, _, _| seg.start_offset, &mut |e| match e {
            Event::Opened { header, zone, .. } => {
                splitter = Splitter::resuming(header.as_deref(), zone.as_deref());
            }
            Event::Line { offset, bytes } => live.extend(splitter.feed(offset, bytes)),
            _ => {}
        })
        .unwrap();
        live.extend(splitter.finish());
        assert_eq!(&live[0], seg);
    }
}
