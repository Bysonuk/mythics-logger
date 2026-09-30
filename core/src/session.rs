//! The log session a segment belongs to: one per combat log file
//! (`docs/specs/logger-api.md`, "Sessions").
//!
//! Every segment cut from one file carries the same `session_key`, so the
//! site shows the file as one log, with its raid bosses and its keys. The
//! key is the SHA-256 (hex) of the file's name, a newline, and the
//! timestamp of the file's first line as the file has it: the same across
//! restarts and re-imports of that file, and new when the game starts a new
//! file. It's read from the file's first line, not a segment's header: the
//! game writes `COMBAT_LOG_VERSION` again mid-file (a reload, a new zone).
//!
//! Only the file's name is sent, never its folder: a path can hold the
//! player's Windows user name.

use crate::splitter::hex;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSession {
    pub key: String,
    pub file_name: String,
    /// The file's first timestamp, as the API takes it; `None` if the first
    /// line couldn't be read.
    pub start: Option<String>,
}

/// The session of the file at `path`, or `None` when it has no usable name.
pub fn of_file(path: &Path, year_hint: Option<i32>) -> Option<FileSession> {
    let file_name = path.file_name()?.to_str()?.to_string();
    if file_name.is_empty()
        || file_name.len() > 255
        || file_name.contains(['/', '\\', ':'])
        || file_name.chars().any(char::is_control)
    {
        return None;
    }
    let first = first_timestamp(path);
    let mut h = Sha256::new();
    h.update(file_name.as_bytes());
    h.update(b"\n");
    h.update(first.as_deref().unwrap_or("").as_bytes());
    let start = first
        .as_deref()
        .and_then(crate::timestamp::parse)
        .map(|t| t.to_api_iso(year_hint));
    Some(FileSession {
        key: hex(&h.finalize()),
        file_name,
        start,
    })
}

/// The first line's timestamp, as written.
fn first_timestamp(path: &Path) -> Option<String> {
    let mut f = crate::tailer::open_shared(path).ok()?;
    let mut head = [0u8; 512];
    let mut n = 0;
    while n < head.len() {
        match f.read(&mut head[n..]).ok()? {
            0 => break,
            k => n += k,
        }
    }
    let line = crate::line::parse(&head[..n])?;
    Some(line.timestamp.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEAD: &str = "9/27/2026 21:35:45.1241  COMBAT_LOG_VERSION,22,ADVANCED_LOG_ENABLED,1,BUILD_VERSION,12.1.0,PROJECT_ID,1\r\n";

    #[test]
    fn one_key_per_file_from_its_name_and_first_line() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("WoWCombatLog-092726_213545.txt");
        std::fs::write(
            &a,
            format!("{HEAD}9/27/2026 21:35:45.1251  ZONE_CHANGE,2859,\"The Blinding Vale\",23\r\n"),
        )
        .unwrap();
        let s = of_file(&a, None).unwrap();
        assert_eq!(s.file_name, "WoWCombatLog-092726_213545.txt");
        let want = hex(&Sha256::digest(
            b"WoWCombatLog-092726_213545.txt\n9/27/2026 21:35:45.1241",
        ));
        assert_eq!(s.key, want);
        assert_eq!(s.start.as_deref(), Some("2026-09-27T21:35:45.124+01:00"));
        // The same file, grown: the same session.
        std::fs::write(&a, format!("{HEAD}{HEAD}")).unwrap();
        assert_eq!(of_file(&a, None).unwrap().key, s.key);
        // Another file (a new night): another session.
        let b = tmp.path().join("WoWCombatLog-092826_200101.txt");
        std::fs::write(&b, HEAD).unwrap();
        assert_ne!(of_file(&b, None).unwrap().key, s.key);
        // Never the folder.
        assert!(!s.file_name.contains(std::path::MAIN_SEPARATOR));
    }

    #[test]
    fn a_file_that_cant_be_read_still_has_a_session_by_name() {
        let tmp = tempfile::tempdir().unwrap();
        let gone = tmp.path().join("WoWCombatLog.txt");
        let s = of_file(&gone, None).unwrap();
        assert_eq!(s.start, None);
        assert_eq!(s.key, hex(&Sha256::digest(b"WoWCombatLog.txt\n")));
    }
}
