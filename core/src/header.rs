//! The header line the game writes each time logging starts (and again at a
//! key's start):
//!
//! ```text
//! COMBAT_LOG_VERSION,22,ADVANCED_LOG_ENABLED,1,BUILD_VERSION,12.1.0,PROJECT_ID,1
//! ```
//!
//! Logs from before about 2019 have no header at all; the app still reads them
//! and says it can't tell whether Advanced Combat Logging was on.

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Header {
    pub version: Option<u32>,
    /// Advanced Combat Logging: the server needs it for boss health, gear and
    /// talents, so the app warns when it's off.
    pub advanced: Option<bool>,
    pub build: Option<String>,
    /// 1 is retail.
    pub project_id: Option<u32>,
}

/// Reads a header from a parsed line's event and fields. `None` if the line
/// isn't a header.
pub fn parse(event: &str, rest: &str) -> Option<Header> {
    if event != "COMBAT_LOG_VERSION" {
        return None;
    }
    // The line is key,value pairs, with the first key already taken as the
    // event name: "COMBAT_LOG_VERSION,22,ADVANCED_LOG_ENABLED,1,…".
    let mut parts = rest.split(',').map(str::trim);
    let mut h = Header {
        version: parts.next().and_then(|v| v.parse().ok()),
        advanced: None,
        build: None,
        project_id: None,
    };
    while let (Some(k), Some(v)) = (parts.next(), parts.next()) {
        match k {
            "ADVANCED_LOG_ENABLED" => h.advanced = Some(v == "1"),
            "BUILD_VERSION" => h.build = Some(v.to_string()),
            "PROJECT_ID" => h.project_id = v.parse().ok(),
            _ => {}
        }
    }
    Some(h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_current_retail() {
        let h = parse(
            "COMBAT_LOG_VERSION",
            "22,ADVANCED_LOG_ENABLED,1,BUILD_VERSION,12.1.0,PROJECT_ID,1",
        )
        .unwrap();
        assert_eq!(h.version, Some(22));
        assert_eq!(h.advanced, Some(true));
        assert_eq!(h.build.as_deref(), Some("12.1.0"));
        assert_eq!(h.project_id, Some(1));
    }

    #[test]
    fn reads_advanced_off_and_old_versions() {
        let h = parse(
            "COMBAT_LOG_VERSION",
            "9,ADVANCED_LOG_ENABLED,0,BUILD_VERSION,9.0.5,PROJECT_ID,1",
        )
        .unwrap();
        assert_eq!(h.advanced, Some(false));
        assert_eq!(h.version, Some(9));
        assert!(parse("SPELL_DAMAGE", "x").is_none());
    }
}
