mod common;

use common::{fixture, write_file};
use mythics_logger_core::splitter::{Segment, Splitter};
use mythics_logger_core::tailer::{live_start, Event, Tailer};
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
