//! Cuts the stream of lines into segments: one boss pull, or one Mythic+ key.
//!
//! A segment is the file's header line (`COMBAT_LOG_VERSION…`), then every
//! line from `ENCOUNTER_START` to `ENCOUNTER_END`, or from
//! `CHALLENGE_MODE_START` to `CHALLENGE_MODE_END`, byte for byte. Its SHA-256
//! is of exactly those bytes, uncompressed, which is what the server
//! de-duplicates on.
//!
//! - Bosses inside a key belong to the key's segment: they aren't cut out
//!   again, so no line is sent twice.
//! - A pull or key that never ends (logging stopped, the game closed, a new
//!   one began) is closed where the next one starts, or at the end of the
//!   file, and sent as kind `segment`.
//! - An end line with no start (the game writes a stray
//!   `CHALLENGE_MODE_END,…,0,0,0` when a key is reset) is ignored.
//! - Trash between pulls isn't sent yet. The scope suggests timed segments for
//!   it; the server's contract for this MVP takes pulls and keys.
//!
//! Privacy: names in the lines are hashed and passed through, never kept or
//! logged. The only names kept are the boss's and the dungeon's. A pull's
//! player GUIDs are held only until it ends, for its roster hash (below).
//!
//! The roster hash is the fingerprint's (`docs/specs/logger-api.md`, "Ask
//! before uploading"): the SHA-256 of the GUIDs of the pull's
//! `COMBATANT_INFO` lines, sorted, de-duplicated and joined with `\n`. For a
//! key, only those outside the boss pulls inside it (the ones after
//! `CHALLENGE_MODE_START`). The server's parser stores the same roster.

use crate::bosshp::BossHp;
use crate::header::{self, Header};
use crate::line::{self, fields};
use crate::timestamp;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Encounter,
    Key,
    /// A pull or key without its end line.
    Segment,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Encounter => "encounter",
            Kind::Key => "key",
            Kind::Segment => "segment",
        }
    }
}

/// A boss fought inside a key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Boss {
    pub encounter_id: u32,
    pub name: String,
    pub success: Option<bool>,
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    pub kind: Kind,
    /// What the segment was opened as, even when it never ended.
    pub opened_as: Kind,
    pub complete: bool,
    pub encounter_id: Option<u32>,
    /// The boss's name, or the dungeon's for a key.
    pub name: Option<String>,
    pub difficulty: Option<u32>,
    pub group_size: Option<u32>,
    pub instance_id: Option<u32>,
    pub key_level: Option<u32>,
    /// The key's challenge-mode map id (`CHALLENGE_MODE_START`'s third field).
    /// A pull sends its `instance_id` as the API's `map_id` instead.
    pub map_id: Option<u32>,
    pub affixes: Vec<u32>,
    /// A kill (or a timed key).
    pub success: Option<bool>,
    pub duration_ms: Option<u64>,
    /// The boss's health when the pull ended, in percent, by the server's
    /// own rule (`crate::bosshp`), so the app can pick each boss's best wipe
    /// before sending anything. 0 for a kill; `None` without Advanced
    /// Combat Logging. Only a boss pull's own segment has one.
    pub boss_hp_pct: Option<f64>,
    pub bosses: Vec<Boss>,
    pub start_time: String,
    pub end_time: String,
    /// The header line, with its line ending, as the file has it.
    pub header: Option<String>,
    pub advanced: Option<bool>,
    /// Byte range of the segment's lines in the file: [start, end).
    pub start_offset: u64,
    pub end_offset: u64,
    /// Header plus range: the uncompressed size sent.
    pub size: u64,
    pub sha256: String,
    /// The fingerprint's roster hash (above); `None` when the segment had no
    /// `COMBATANT_INFO`, or came from a queue saved before there was one.
    #[serde(default)]
    pub roster_hash: Option<String>,
}

impl Segment {
    pub fn start_iso(&self, year_hint: Option<i32>) -> String {
        iso(&self.start_time, year_hint)
    }
    pub fn end_iso(&self, year_hint: Option<i32>) -> String {
        iso(&self.end_time, year_hint)
    }
    /// As sent to the API: always with an offset (see `to_api_iso`).
    pub fn start_api(&self, year_hint: Option<i32>) -> String {
        api_iso(&self.start_time, year_hint)
    }
    pub fn end_api(&self, year_hint: Option<i32>) -> String {
        api_iso(&self.end_time, year_hint)
    }
}

fn api_iso(raw: &str, year_hint: Option<i32>) -> String {
    timestamp::parse(raw)
        .map(|t| t.to_api_iso(year_hint))
        .unwrap_or_default()
}

fn iso(raw: &str, year_hint: Option<i32>) -> String {
    timestamp::parse(raw)
        .map(|t| t.to_iso(year_hint))
        .unwrap_or_default()
}

/// What's open right now, for the Live tab.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Current {
    pub kind: Kind,
    pub name: Option<String>,
    pub difficulty: Option<u32>,
    pub key_level: Option<u32>,
    pub start_time: String,
}

struct Open {
    kind: Kind,
    seg: Segment,
    hasher: Sha256,
    last_time: String,
    /// Creatures' health, for a boss pull's boss health (`crate::bosshp`).
    hp: BossHp,
    /// Player GUIDs from `COMBATANT_INFO`, for the roster hash.
    roster: Vec<String>,
    /// Inside a boss pull inside this key.
    in_boss: bool,
}

/// The fingerprint's roster hash of these GUIDs (see the module's notes).
pub fn roster_hash<S: AsRef<str>>(guids: &[S]) -> Option<String> {
    let mut sorted: Vec<&str> = guids.iter().map(AsRef::as_ref).collect();
    sorted.sort_unstable();
    sorted.dedup();
    if sorted.is_empty() {
        return None;
    }
    Some(hex(&Sha256::digest(sorted.join("\n").as_bytes())))
}

#[derive(Default)]
pub struct Splitter {
    header_line: Option<Vec<u8>>,
    header: Option<Header>,
    open: Option<Open>,
    zone: Option<(String, u32)>,
    /// Pulls and keys seen, including ones inside keys: for counts.
    pub encounters_seen: u32,
    pub keys_seen: u32,
}

impl Splitter {
    pub fn new() -> Self {
        Self::default()
    }

    /// For a file opened part-way through: the header comes from its first line.
    pub fn with_header(raw_header_line: &[u8]) -> Self {
        let mut s = Self::new();
        s.set_header(raw_header_line);
        s
    }

    pub fn header(&self) -> Option<&Header> {
        self.header.as_ref()
    }

    pub fn zone(&self) -> Option<(&str, u32)> {
        self.zone.as_ref().map(|(n, d)| (n.as_str(), *d))
    }

    pub fn current(&self) -> Option<Current> {
        self.open.as_ref().map(|o| Current {
            kind: o.kind,
            name: o.seg.name.clone(),
            difficulty: o.seg.difficulty,
            key_level: o.seg.key_level,
            start_time: o.seg.start_time.clone(),
        })
    }

    /// Where to resume reading after a restart without losing the open
    /// segment: its first byte, if one is open.
    pub fn open_start(&self) -> Option<u64> {
        self.open.as_ref().map(|o| o.seg.start_offset)
    }

    fn set_header(&mut self, raw: &[u8]) {
        if let Some(l) = line::parse(raw) {
            if let Some(h) = header::parse(l.event, l.rest) {
                self.header = Some(h);
                self.header_line = Some(raw.to_vec());
            }
        }
    }

    /// Feeds one line, with its line ending, starting at `offset` in the file.
    /// Returns a segment when this line closes one.
    pub fn feed(&mut self, offset: u64, raw: &[u8]) -> Option<Segment> {
        let event = line::event_name(raw).unwrap_or("");
        match event {
            "COMBAT_LOG_VERSION" => {
                self.set_header(raw);
                self.append(raw);
                None
            }
            "ZONE_CHANGE" => {
                if let Some(l) = line::parse(raw) {
                    let f = fields(l.rest);
                    if let (Some(name), Some(diff)) =
                        (f.get(1), f.get(2).and_then(|d| d.parse().ok()))
                    {
                        self.zone = Some((name.to_string(), diff));
                    }
                }
                self.append(raw);
                None
            }
            "CHALLENGE_MODE_START" => {
                let closed = self.close_incomplete();
                self.keys_seen += 1;
                let l = line::parse(raw)?;
                let f = fields(l.rest);
                let mut o = self.open_segment(Kind::Key, offset, l.timestamp);
                o.seg.name = f.first().map(|s| s.to_string());
                o.seg.instance_id = f.get(1).and_then(|v| v.parse().ok());
                o.seg.map_id = f.get(2).and_then(|v| v.parse().ok());
                o.seg.key_level = f.get(3).and_then(|v| v.parse().ok());
                o.seg.affixes = f
                    .get(4)
                    .map(|a| {
                        a.trim_matches(|c| c == '[' || c == ']')
                            .split(',')
                            .filter_map(|x| x.trim().parse().ok())
                            .collect()
                    })
                    .unwrap_or_default();
                self.open = Some(o);
                self.append(raw);
                closed
            }
            "CHALLENGE_MODE_END" => {
                let is_key = matches!(self.open.as_ref(), Some(o) if o.kind == Kind::Key);
                if !is_key {
                    // A stray end (a reset key) outside a key: not part of anything.
                    self.append(raw);
                    return None;
                }
                self.append(raw);
                let l = line::parse(raw)?;
                let f = fields(l.rest);
                let o = self.open.as_mut()?;
                o.seg.success = f.get(1).map(|v| *v == "1");
                o.seg.duration_ms = f.get(3).and_then(|v| v.parse().ok());
                self.close(true)
            }
            "ENCOUNTER_START" => {
                self.encounters_seen += 1;
                let l = line::parse(raw)?;
                let f = fields(l.rest);
                let id = f.first().and_then(|v| v.parse().ok());
                let name = f.get(1).map(|s| s.to_string());
                if let Some(o) = self.open.as_mut().filter(|o| o.kind == Kind::Key) {
                    o.in_boss = true;
                    o.seg.bosses.push(Boss {
                        encounter_id: id.unwrap_or(0),
                        name: name.unwrap_or_default(),
                        success: None,
                        duration_ms: None,
                    });
                    self.append(raw);
                    return None;
                }
                let closed = self.close_incomplete();
                let mut o = self.open_segment(Kind::Encounter, offset, l.timestamp);
                o.seg.encounter_id = id;
                o.seg.name = name;
                o.seg.difficulty = f.get(2).and_then(|v| v.parse().ok());
                o.seg.group_size = f.get(3).and_then(|v| v.parse().ok());
                o.seg.instance_id = f.get(4).and_then(|v| v.parse().ok());
                self.open = Some(o);
                self.append(raw);
                closed
            }
            "ENCOUNTER_END" => {
                // An end with nothing open is ignored.
                let kind = self.open.as_ref().map(|o| o.kind)?;
                self.append(raw);
                let l = line::parse(raw)?;
                let f = fields(l.rest);
                let id: Option<u32> = f.first().and_then(|v| v.parse().ok());
                let success = f.get(4).map(|v| *v == "1");
                let duration = f.get(5).and_then(|v| v.parse().ok());
                let o = self.open.as_mut()?;
                if kind == Kind::Encounter && id.is_some() && id != o.seg.encounter_id {
                    // Another boss's end: not this pull's. Keep reading.
                    return None;
                }
                if kind == Kind::Key {
                    o.in_boss = false;
                    if let Some(b) = o.seg.bosses.last_mut() {
                        b.success = success;
                        b.duration_ms = duration;
                    }
                    return None;
                }
                o.seg.success = success;
                o.seg.duration_ms = duration;
                self.close(true)
            }
            "SPELL_DAMAGE"
            | "SPELL_PERIODIC_DAMAGE"
            | "RANGE_DAMAGE"
            | "SPELL_BUILDING_DAMAGE"
            | "SWING_DAMAGE"
            | "SWING_DAMAGE_LANDED"
            | "ENVIRONMENTAL_DAMAGE"
            | "SPELL_HEAL"
            | "SPELL_PERIODIC_HEAL"
            | "SPELL_CAST_SUCCESS" => {
                self.track_hp(event, raw);
                self.append(raw);
                None
            }
            "COMBATANT_INFO" => {
                if let Some(o) = self.open.as_mut().filter(|o| !o.in_boss) {
                    // The GUID is the first field, as the server's parser reads it.
                    let guid = line::parse(raw)
                        .and_then(|l| l.rest.split(',').next())
                        .filter(|g| !g.is_empty())
                        .map(str::to_string);
                    o.roster.extend(guid);
                }
                self.append(raw);
                None
            }
            _ => {
                self.append(raw);
                None
            }
        }
    }

    /// Closes whatever is open at the end of a file (or when the app moves to
    /// a new file): it never ended, so it goes as kind `segment`.
    pub fn finish(&mut self) -> Option<Segment> {
        self.close_incomplete()
    }

    fn open_segment(&self, kind: Kind, offset: u64, ts: &str) -> Open {
        let mut hasher = Sha256::new();
        let header = self.header_line.clone();
        let mut size = 0u64;
        if let Some(h) = &header {
            hasher.update(h);
            size += h.len() as u64;
        }
        Open {
            kind,
            hasher,
            last_time: ts.to_string(),
            hp: BossHp::new(
                self.header.as_ref().and_then(|h| h.advanced),
                self.header.as_ref().and_then(|h| h.version),
            ),
            roster: Vec::new(),
            in_boss: false,
            seg: Segment {
                kind,
                opened_as: kind,
                complete: false,
                encounter_id: None,
                name: None,
                difficulty: None,
                group_size: None,
                instance_id: None,
                key_level: None,
                map_id: None,
                affixes: Vec::new(),
                success: None,
                duration_ms: None,
                boss_hp_pct: None,
                bosses: Vec::new(),
                start_time: ts.to_string(),
                end_time: ts.to_string(),
                header: header.map(|h| String::from_utf8_lossy(&h).into_owned()),
                advanced: self.header.as_ref().and_then(|h| h.advanced),
                start_offset: offset,
                end_offset: offset,
                size,
                sha256: String::new(),
                roster_hash: None,
            },
        }
    }

    fn append(&mut self, raw: &[u8]) {
        let Some(o) = self.open.as_mut() else { return };
        o.hasher.update(raw);
        o.seg.size += raw.len() as u64;
        o.seg.end_offset += raw.len() as u64;
        if let Some(sep) = raw.iter().take(64).position(|&b| b == b' ') {
            // Timestamps are "date time"; the second space ends it.
            let end = raw[sep + 1..]
                .iter()
                .position(|&b| b == b' ')
                .map(|p| sep + 1 + p)
                .unwrap_or(sep);
            if let Ok(ts) = std::str::from_utf8(&raw[..end]) {
                o.last_time.clear();
                o.last_time.push_str(ts);
            }
        }
    }

    fn track_hp(&mut self, event: &str, raw: &[u8]) {
        // Only a boss pull's own segment needs it: a key's bosses go with
        // the key whatever their health.
        let Some(o) = self.open.as_mut().filter(|o| o.kind == Kind::Encounter) else {
            return;
        };
        if let Some(l) = line::parse(raw) {
            o.hp.feed(event, l.rest);
        }
    }

    fn close_incomplete(&mut self) -> Option<Segment> {
        self.close(false)
    }

    fn close(&mut self, complete: bool) -> Option<Segment> {
        let o = self.open.take()?;
        let mut seg = o.seg;
        seg.complete = complete;
        if !complete {
            seg.kind = Kind::Segment;
        }
        seg.end_time = o.last_time;
        if o.kind == Kind::Encounter {
            seg.boss_hp_pct = o.hp.pct(seg.success);
        }
        seg.sha256 = hex(&o.hasher.finalize());
        seg.roster_hash = roster_hash(&o.roster);
        Some(seg)
    }
}

pub fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 15) as usize] as char);
    }
    s
}

/// Splits a whole file from start to end, streaming it through a fixed
/// buffer: memory stays flat whatever the file's size. `on_progress` gets the
/// bytes read so far; returning `false` from it stops early.
pub fn split_file(
    path: &std::path::Path,
    mut on_segment: impl FnMut(Segment),
    mut on_progress: impl FnMut(u64) -> bool,
) -> std::io::Result<Splitter> {
    let file = crate::tailer::open_shared(path)?;
    let mut reader = std::io::BufReader::with_capacity(1 << 20, file);
    let mut splitter = Splitter::new();
    let mut line_buf = Vec::with_capacity(4096);
    let mut offset = 0u64;
    let mut since_progress = 0u64;
    loop {
        line_buf.clear();
        let n = read_line_capped(&mut reader, &mut line_buf)?;
        if n == 0 {
            break;
        }
        if let Some(seg) = splitter.feed(offset, &line_buf) {
            on_segment(seg);
        }
        offset += n as u64;
        since_progress += n as u64;
        if since_progress >= 8 << 20 {
            since_progress = 0;
            if !on_progress(offset) {
                return Ok(splitter);
            }
        }
        // Don't let one huge line (a corrupt file) keep its buffer.
        if line_buf.capacity() > 1 << 20 {
            line_buf = Vec::with_capacity(4096);
        }
    }
    if let Some(seg) = splitter.finish() {
        on_segment(seg);
    }
    on_progress(offset);
    Ok(splitter)
}

/// `read_until(b'\n')`, but a line longer than 16 MB (a corrupt file) is
/// handed over in pieces instead of growing the buffer without end. The bytes
/// are the same either way; only boundary detection sees the pieces.
fn read_line_capped<R: std::io::BufRead>(r: &mut R, out: &mut Vec<u8>) -> std::io::Result<usize> {
    const CAP: usize = 16 << 20;
    let mut total = 0;
    loop {
        let (done, used) = {
            let buf = r.fill_buf()?;
            if buf.is_empty() {
                return Ok(total);
            }
            match buf.iter().position(|&b| b == b'\n') {
                Some(i) => {
                    out.extend_from_slice(&buf[..=i]);
                    (true, i + 1)
                }
                None => {
                    out.extend_from_slice(buf);
                    (out.len() >= CAP, buf.len())
                }
            }
        };
        r.consume(used);
        total += used;
        if done {
            return Ok(total);
        }
    }
}
