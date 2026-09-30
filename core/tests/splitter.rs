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

/// The segment's SHA-256 must be of its header line, its zone line and its
/// exact bytes, and each carried line a line of the file, byte for byte.
fn check_bytes(bytes: &[u8], seg: &Segment) {
    let mut h = Sha256::new();
    let prefix = seg.prefix().unwrap_or_default();
    let header = seg.header.as_deref().unwrap_or("").as_bytes();
    let zone = seg.zone_line.as_deref().unwrap_or("").as_bytes();
    assert_eq!(prefix, [header, zone].concat());
    for carried in [header, zone].into_iter().filter(|l| !l.is_empty()) {
        assert!(lines(bytes).iter().any(|(_, l)| *l == carried));
    }
    h.update(&prefix);
    h.update(&bytes[seg.start_offset as usize..seg.end_offset as usize]);
    assert_eq!(seg.sha256, hex(&h.finalize()));
    assert_eq!(
        seg.size,
        prefix.len() as u64 + seg.end_offset - seg.start_offset
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
    // The zone change on entering the raid, from before the pull.
    assert_eq!(
        wipe.zone_line.as_deref(),
        Some("9/28/2026 20:01:02.1021  ZONE_CHANGE,2810,\"Manaforge Omega\",16\r\n")
    );
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
    assert_eq!(kill.zone_line, wipe.zone_line);
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
    // The zone change on entering the dungeon, before the key started (the
    // one the game writes again inside the key is in its bytes already).
    assert_eq!(
        key.zone_line.as_deref(),
        Some("9/27/2026 21:35:45.1251  ZONE_CHANGE,2859,\"The Blinding Vale\",23\r\n")
    );
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
    // No header, but its zone change still goes.
    assert!(seg
        .zone_line
        .as_deref()
        .unwrap()
        .contains("ZONE_CHANGE,2296,\"Castle Nathria\",16"));
    assert_eq!(seg.advanced, None);
    assert_eq!(seg.success, Some(true));
    assert_eq!(seg.start_iso(Some(2020)), "2020-11-21T12:01:34.071");
    check_bytes(&bytes, seg);
}

#[test]
fn resuming_mid_file_takes_the_header_from_the_first_line() {
    let bytes = fixture("raid_night.txt");
    let all = lines(&bytes);
    // The tailer finds the zone line by looking back (`last_zone_change`).
    let mut s = Splitter::resuming(Some(all[0].1), Some(all[1].1));
    assert_eq!(s.zone(), Some(("Manaforge Omega", 16)));
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

/// A night out of synthetic lines (fake players, real instance and boss
/// names): a city, one raid, out to the city, another raid, then a key.
const NIGHT: &str = "9/29/2026 19:00:00.0001  COMBAT_LOG_VERSION,22,ADVANCED_LOG_ENABLED,1,BUILD_VERSION,12.1.0,PROJECT_ID,1\r\n\
9/29/2026 19:00:00.0002  ZONE_CHANGE,2552,\"Khaz Algar (Surface)\",0\r\n\
9/29/2026 19:00:00.0003  MAP_CHANGE,2339,\"Dornogal\",-2000.0,-3000.0,1500.0,2500.0\r\n\
9/29/2026 19:30:00.0001  ZONE_CHANGE,2810,\"Manaforge Omega\",16\r\n\
9/29/2026 19:30:00.0002  MAP_CHANGE,2460,\"Manaforge Omega\",100.0,-100.0,200.0,-200.0\r\n\
9/29/2026 19:31:00.0001  SPELL_AURA_APPLIED,Player-1403-0A000001,\"Player1-TarrenMill-EU\",0x512,0x0,Player-1403-0A000001,\"Player1-TarrenMill-EU\",0x512,0x0,1459,\"Arcane Intellect\",0x40,BUFF\r\n\
9/29/2026 19:35:00.0001  ENCOUNTER_START,3129,\"Plexus Sentinel\",16,20,2810\r\n\
9/29/2026 19:35:00.0101  COMBATANT_INFO,Player-1403-0A000001,1,100\r\n\
9/29/2026 19:35:00.0102  COMBATANT_INFO,Player-1403-0A000002,1,100\r\n\
9/29/2026 19:38:00.0001  ENCOUNTER_END,3129,\"Plexus Sentinel\",16,20,1,180000\r\n\
9/29/2026 20:00:00.0001  ZONE_CHANGE,2552,\"Khaz Algar (Surface)\",0\r\n\
9/29/2026 20:10:00.0001  ZONE_CHANGE,2769,\"Liberation of Undermine\",15\r\n\
9/29/2026 20:15:00.0001  ENCOUNTER_START,3009,\"Vexie and the Geargrinders\",15,20,2769\r\n\
9/29/2026 20:15:00.0101  COMBATANT_INFO,Player-1403-0A000001,1,100\r\n\
9/29/2026 20:19:00.0001  ENCOUNTER_END,3009,\"Vexie and the Geargrinders\",15,20,0,240000\r\n\
9/29/2026 21:00:00.0001  ZONE_CHANGE,2859,\"The Blinding Vale\",23\r\n\
9/29/2026 21:01:00.0001  CHALLENGE_MODE_START,\"The Blinding Vale\",2859,584,12,[9,10,147]\r\n\
9/29/2026 21:01:00.0002  COMBAT_LOG_VERSION,22,ADVANCED_LOG_ENABLED,1,BUILD_VERSION,12.1.0,PROJECT_ID,1\r\n\
9/29/2026 21:01:00.0003  ZONE_CHANGE,2859,\"The Blinding Vale\",8\r\n\
9/29/2026 21:01:01.0001  COMBATANT_INFO,Player-1403-0A000001,1,100\r\n\
9/29/2026 21:30:00.0001  CHALLENGE_MODE_END,2859,1,12,1740000,300.0,3000.0\r\n\
9/29/2026 21:40:00.0001  ZONE_CHANGE,2769,\"Liberation of Undermine\",15\r\n\
9/29/2026 21:45:00.0001  ENCOUNTER_START,3009,\"Vexie and the Geargrinders\",15,20,2769\r\n\
9/29/2026 21:45:00.0101  COMBATANT_INFO,Player-1403-0A000001,1,100\r\n";

fn zone_of(seg: &Segment) -> &str {
    let l = seg.zone_line.as_deref().expect("a zone line");
    l.split("  ").nth(1).unwrap().trim_end()
}

#[test]
fn each_segment_carries_the_latest_zone_change_before_it() {
    let bytes = NIGHT.as_bytes();
    let (segs, s) = split(bytes);
    assert_eq!(segs.len(), 4);
    // Each raid pull is named by the zone change on entering its raid, not
    // the file's first, nor the city in between.
    assert_eq!(segs[0].name.as_deref(), Some("Plexus Sentinel"));
    assert_eq!(zone_of(&segs[0]), "ZONE_CHANGE,2810,\"Manaforge Omega\",16");
    assert_eq!(
        zone_of(&segs[1]),
        "ZONE_CHANGE,2769,\"Liberation of Undermine\",15"
    );
    // A key: the zone change before CHALLENGE_MODE_START, not the ones the
    // game writes inside it (those are in its bytes).
    assert_eq!(segs[2].kind, Kind::Key);
    assert_eq!(
        zone_of(&segs[2]),
        "ZONE_CHANGE,2859,\"The Blinding Vale\",23"
    );
    // After the key: the raid again, entered after it ended.
    assert_eq!(segs[3].kind, Kind::Segment);
    assert_eq!(
        segs[3].zone_line.as_deref().unwrap(),
        "9/29/2026 21:40:00.0001  ZONE_CHANGE,2769,\"Liberation of Undermine\",15\r\n"
    );
    for seg in &segs {
        check_bytes(bytes, seg);
        // Header first, then the zone line; MAP_CHANGE isn't carried (the
        // server's parser doesn't read it).
        let prefix = String::from_utf8(seg.prefix().unwrap()).unwrap();
        assert!(prefix.contains("  COMBAT_LOG_VERSION,22,"));
        assert_eq!(prefix.lines().count(), 2);
        assert!(prefix.lines().nth(1).unwrap().contains("  ZONE_CHANGE,"));
        assert!(!prefix.contains("MAP_CHANGE"));
        // The carried lines change neither end of the segment's time.
        let first = &bytes[seg.start_offset as usize..];
        assert!(first.starts_with(seg.start_time.as_bytes()));
    }
    assert_eq!(s.zone(), Some(("Liberation of Undermine", 15)));
}

#[test]
fn a_pull_with_no_zone_change_before_it_carries_none() {
    // Logging turned on inside the raid, mid-way: no ZONE_CHANGE before the
    // pull. The segment is as it was before zone lines: header and range.
    let log = "9/29/2026 19:35:00.0000  COMBAT_LOG_VERSION,22,ADVANCED_LOG_ENABLED,1,BUILD_VERSION,12.1.0,PROJECT_ID,1\r\n\
9/29/2026 19:35:00.0001  ENCOUNTER_START,3129,\"Plexus Sentinel\",16,20,2810\r\n\
9/29/2026 19:35:00.0101  COMBATANT_INFO,Player-1403-0A000001,1,100\r\n\
9/29/2026 19:36:00.0001  ZONE_CHANGE,2810,\"Manaforge Omega\",16\r\n\
9/29/2026 19:38:00.0001  ENCOUNTER_END,3129,\"Plexus Sentinel\",16,20,1,180000\r\n\
9/29/2026 19:45:00.0001  ENCOUNTER_START,3129,\"Plexus Sentinel\",16,20,2810\r\n\
9/29/2026 19:48:00.0001  ENCOUNTER_END,3129,\"Plexus Sentinel\",16,20,1,180000\r\n";
    let (segs, _) = split(log.as_bytes());
    assert_eq!(segs.len(), 2);
    assert_eq!(segs[0].zone_line, None);
    assert_eq!(
        segs[0].prefix().unwrap(),
        segs[0].header.as_deref().unwrap().as_bytes()
    );
    check_bytes(log.as_bytes(), &segs[0]);
    // One written during the first pull is in its bytes, and carried to the
    // next.
    assert_eq!(zone_of(&segs[1]), "ZONE_CHANGE,2810,\"Manaforge Omega\",16");
    check_bytes(log.as_bytes(), &segs[1]);

    // Neither header nor zone change: nothing before the range at all.
    let bare = &log[log.find('\n').unwrap() + 1..];
    let (segs, _) = split(bare.as_bytes());
    assert_eq!(segs[0].prefix(), None);
    check_bytes(bare.as_bytes(), &segs[0]);
}

#[test]
fn only_a_whole_zone_change_line_is_carried() {
    // A ZONE_CHANGE cut off by the end of what's written (no line ending)
    // isn't carried: it wouldn't be the line the file ends up with.
    let mut s = Splitter::resuming(
        None,
        Some(b"9/29/2026 19:30:00.0001  ZONE_CHANGE,2810,\"Mana"),
    );
    let seg = s
        .feed(
            0,
            b"9/29/2026 19:35:00.0001  ENCOUNTER_START,3129,\"Plexus Sentinel\",16,20,2810\r\n",
        )
        .or_else(|| s.finish())
        .unwrap();
    assert_eq!(seg.zone_line, None);
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
