mod common;

use common::{fixture, lines};
use mythics_logger_core::splitter::{hex, Kind, Segment, Splitter};
use sha2::{Digest, Sha256};

fn split(bytes: &[u8]) -> (Vec<Segment>, Splitter) {
    let mut s = Splitter::new();
    let mut out = Vec::new();
    for (off, l) in lines(bytes) {
        out.extend(s.feed(off, l));
    }
    out.extend(s.finish());
    (out, s)
}

/// The segment's SHA-256 must be of its header line plus its exact bytes.
fn check_bytes(bytes: &[u8], seg: &Segment) {
    let mut h = Sha256::new();
    let header = seg.header.as_deref().unwrap_or("").as_bytes();
    h.update(header);
    h.update(&bytes[seg.start_offset as usize..seg.end_offset as usize]);
    assert_eq!(seg.sha256, hex(&h.finalize()));
    assert_eq!(
        seg.size,
        header.len() as u64 + seg.end_offset - seg.start_offset
    );
}

#[test]
fn raid_night_gives_a_wipe_and_a_kill() {
    let bytes = fixture("raid_night.txt");
    let (segs, s) = split(&bytes);
    assert_eq!(segs.len(), 2);
    assert_eq!(s.encounters_seen, 2);
    assert_eq!(s.keys_seen, 0);

    let wipe = &segs[0];
    assert_eq!(wipe.kind, Kind::Encounter);
    assert!(wipe.complete);
    assert_eq!(wipe.encounter_id, Some(3129));
    assert_eq!(wipe.name.as_deref(), Some("Plexus Sentinel"));
    assert_eq!(wipe.difficulty, Some(16));
    assert_eq!(wipe.group_size, Some(20));
    assert_eq!(wipe.success, Some(false));
    assert_eq!(wipe.duration_ms, Some(220_000));
    // The boss (1,000,000,000 health) at 234,000,000, not the add.
    assert_eq!(wipe.boss_hp_pct, Some(23.4));
    assert_eq!(wipe.advanced, Some(true));
    assert_eq!(wipe.start_time, "9/28/2026 20:05:00.0001");
    assert_eq!(wipe.end_time, "9/28/2026 20:08:40.0001");
    assert_eq!(wipe.start_iso(None), "2026-09-28T20:05:00.000+01:00");
    assert!(wipe
        .header
        .as_deref()
        .unwrap()
        .starts_with("9/28/2026 20:01:02.1001  COMBAT_LOG_VERSION,22"));
    // Starts at ENCOUNTER_START and ends with ENCOUNTER_END's line ending.
    let seg_bytes = &bytes[wipe.start_offset as usize..wipe.end_offset as usize];
    assert!(seg_bytes.windows(15).position(|w| w == b"ENCOUNTER_START") == Some(25));
    assert!(seg_bytes.ends_with(b",0,220000\r\n"));
    check_bytes(&bytes, wipe);

    let kill = &segs[1];
    assert_eq!(kill.success, Some(true));
    assert_eq!(kill.duration_ms, Some(312_499));
    // A kill is 0, as the server's parser says.
    assert_eq!(kill.boss_hp_pct, Some(0.0));
    check_bytes(&bytes, kill);
    // Trash between pulls (the resurrect) belongs to neither.
    assert!(kill.start_offset > wipe.end_offset);
}

#[test]
fn a_key_is_one_segment_with_its_bosses_inside() {
    let bytes = fixture("mplus_key.txt");
    let (segs, s) = split(&bytes);
    // The stray CHALLENGE_MODE_END (a reset key) before the start is ignored.
    assert_eq!(segs.len(), 1);
    assert_eq!(s.keys_seen, 1);
    assert_eq!(s.encounters_seen, 4);
    let key = &segs[0];
    assert_eq!(key.kind, Kind::Key);
    assert!(key.complete);
    assert_eq!(key.name.as_deref(), Some("The Blinding Vale"));
    assert_eq!(key.key_level, Some(14));
    assert_eq!(key.map_id, Some(584));
    assert_eq!(key.instance_id, Some(2859));
    assert_eq!(key.affixes, vec![9, 10, 147]);
    assert_eq!(key.success, Some(true));
    assert_eq!(key.duration_ms, Some(1_681_247));
    let names: Vec<_> = key.bosses.iter().map(|b| b.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "Lightblossom Trinity",
            "Ikuzz the Light Hunter",
            "Lightwarden Ruia",
            "Ziekett"
        ]
    );
    assert!(key.bosses.iter().all(|b| b.success == Some(true)));
    check_bytes(&bytes, key);
    // The header the game writes again inside the key stays in the bytes.
    let inner = &bytes[key.start_offset as usize..key.end_offset as usize];
    assert_eq!(
        inner
            .windows(18)
            .filter(|w| w == b"COMBAT_LOG_VERSION")
            .count(),
        1
    );
}

#[test]
fn unfinished_pulls_go_as_segments() {
    let bytes = fixture("unfinished.txt");
    let (segs, _) = split(&bytes);
    assert_eq!(segs.len(), 2);
    for s in &segs {
        assert_eq!(s.kind, Kind::Segment);
        assert_eq!(s.opened_as, Kind::Encounter);
        assert!(!s.complete);
        assert_eq!(s.advanced, Some(false));
        check_bytes(&bytes, s);
    }
    // The first ends where the second begins; another boss's end line
    // doesn't close the second.
    assert_eq!(segs[0].end_offset, segs[1].start_offset);
    assert_eq!(segs[1].end_offset as usize, bytes.len());
}

#[test]
fn old_logs_without_a_header_or_year() {
    let bytes = fixture("old_version.txt");
    let (segs, s) = split(&bytes);
    assert!(s.header().is_none());
    assert_eq!(segs.len(), 1);
    let seg = &segs[0];
    assert_eq!(seg.header, None);
    assert_eq!(seg.advanced, None);
    assert_eq!(seg.success, Some(true));
    assert_eq!(seg.start_iso(Some(2020)), "2020-11-21T12:01:34.071");
    check_bytes(&bytes, seg);
}

#[test]
fn resuming_mid_file_takes_the_header_from_the_first_line() {
    let bytes = fixture("raid_night.txt");
    let all = lines(&bytes);
    let mut s = Splitter::with_header(all[0].1);
    let mut segs = Vec::new();
    // Start at the second pull.
    for (off, l) in all.iter().skip(12) {
        segs.extend(s.feed(*off, l));
    }
    let (whole, _) = split(&bytes);
    assert_eq!(segs.len(), 1);
    assert_eq!(segs[0], whole[1]);
}

#[test]
fn lf_only_files_split_the_same_way() {
    let crlf = fixture("raid_night.txt");
    let lf: Vec<u8> = crlf.iter().copied().filter(|&b| b != b'\r').collect();
    let (segs, _) = split(&lf);
    assert_eq!(segs.len(), 2);
    check_bytes(&lf, &segs[0]);
}

fn hash_of(guids: &[&str]) -> String {
    hex(&Sha256::digest(guids.join("\n").as_bytes()))
}

#[test]
fn a_pulls_roster_hash_is_its_combatant_info_guids() {
    let (segs, _) = split(&fixture("raid_night.txt"));
    assert_eq!(
        segs[0].roster_hash.as_deref(),
        Some(hash_of(&["Player-1403-0A000001", "Player-1403-0A000002"]).as_str())
    );
    assert_eq!(
        segs[1].roster_hash.as_deref(),
        Some(hash_of(&["Player-1403-0A000001"]).as_str())
    );
}

#[test]
fn a_keys_roster_is_its_own_not_its_bosses() {
    // As the server's parser reads it: a COMBATANT_INFO inside a boss pull
    // in the key is that pull's, not the key's. Sorted and de-duplicated.
    let log = "9/27/2026 21:35:45.1241  COMBAT_LOG_VERSION,22,ADVANCED_LOG_ENABLED,1,BUILD_VERSION,12.1.0,PROJECT_ID,1\r\n\
9/27/2026 21:39:19.1871  CHALLENGE_MODE_START,\"The Blinding Vale\",2859,584,14,[9,10,147]\r\n\
9/27/2026 21:39:21.0001  COMBATANT_INFO,Player-1403-0A000009,1,100\r\n\
9/27/2026 21:39:21.0002  COMBATANT_INFO,Player-1403-0A000003,1,100\r\n\
9/27/2026 21:39:21.0003  COMBATANT_INFO,Player-1403-0A000003,1,100\r\n\
9/27/2026 21:45:04.5521  ENCOUNTER_START,3199,\"Lightblossom Trinity\",8,5,2859\r\n\
9/27/2026 21:45:04.5601  COMBATANT_INFO,Player-1403-0A000004,1,100\r\n\
9/27/2026 21:47:43.3081  ENCOUNTER_END,3199,\"Lightblossom Trinity\",8,5,1,158794\r\n\
9/27/2026 22:05:58.7421  CHALLENGE_MODE_END,2859,1,14,1681247,397.474030,3279.437256\r\n";
    let (segs, _) = split(log.as_bytes());
    assert_eq!(segs.len(), 1);
    assert_eq!(
        segs[0].roster_hash.as_deref(),
        Some(hash_of(&["Player-1403-0A000003", "Player-1403-0A000009"]).as_str())
    );
    // No COMBATANT_INFO at all: no hash, so nothing to ask; it's sent.
    let bare =
        "9/27/2026 21:45:04.5521  ENCOUNTER_START,3199,\"Lightblossom Trinity\",8,5,2859\r\n\
9/27/2026 21:47:43.3081  ENCOUNTER_END,3199,\"Lightblossom Trinity\",8,5,1,158794\r\n";
    let (segs, _) = split(bare.as_bytes());
    assert_eq!(segs[0].roster_hash, None);
}
