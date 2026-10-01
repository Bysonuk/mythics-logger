//! The player's settings, in `settings.json` in the app's config folder. No
//! secret goes here: the app token lives in the operating system's credential
//! store (`token.rs`).

use mythics_logger_core::api::{Main, Visibility};
use mythics_logger_core::plan::BacklogPulls;
use mythics_logger_core::DEFAULT_ORIGIN;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// Each upload's visibility unless changed on it.
    pub default_visibility: Visibility,
    /// The Logs folder, found or chosen.
    pub logs_dir: Option<PathBuf>,
    /// mythics.gg, or a local server for development.
    pub site_origin: String,
    /// Off by default.
    pub start_with_windows: bool,
    /// TODO(server): needs the API to tell the app which raids and keys are
    /// the guild's. Stored now; not applied yet.
    pub only_my_guild: bool,
    /// Upload speed limit in kilobytes a second; 0 is no limit.
    pub upload_limit_kbps: u32,
    /// "eu" or "us": sent with each upload. Taken from the main character
    /// when the server says, otherwise this.
    pub region: String,
    /// The signed-in player's main character and guild, for the top right.
    /// Never a BattleTag.
    pub main: Option<Main>,
    pub first_run_done: bool,
    /// Follow the newest combat log and upload pulls as they end. Off until
    /// the player chooses (the owner, 29 Sep 2026: some players won't want
    /// to stream their logs). Off, the app never opens the live file; past
    /// logs still go, but only from the Backlog tab.
    pub live_logging: bool,
    /// The first-run question about live logging has been answered.
    pub live_asked: bool,
    /// Which of a past log's pulls go in full: every kill and each boss's
    /// best wipe (the default, the owner's decision of 29 Sep 2026), the
    /// other wipes as summaries; or all of them. Live logging always sends
    /// every pull.
    pub backlog_pulls: BacklogPulls,
    /// Move a finished log into `Logs\MythicsLogsArchive` as a `.zip` once
    /// every pull of it this app queued is uploaded. Off until the player
    /// turns it on (the owner's decision on mythics-logger issue 4).
    pub archive_uploaded: bool,
    /// Delete this app's archives older than this many days: 0 (never, the
    /// default), 30, 60 or 90.
    pub archive_delete_after_days: u32,
}

/// The choices for "Delete archived logs after", in days; 0 is never.
pub const ARCHIVE_DAYS: [u32; 4] = [0, 30, 60, 90];

impl Default for Settings {
    fn default() -> Self {
        Self {
            default_visibility: Visibility::Public,
            logs_dir: None,
            site_origin: DEFAULT_ORIGIN.to_string(),
            start_with_windows: false,
            only_my_guild: false,
            upload_limit_kbps: 0,
            region: "eu".into(),
            main: None,
            first_run_done: false,
            live_logging: false,
            live_asked: false,
            backlog_pulls: BacklogPulls::KillsAndBestWipe,
            archive_uploaded: false,
            archive_delete_after_days: 0,
        }
    }
}

impl Settings {
    pub fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(tmp, path)
    }

    /// Live logging on or off, from the first-run question, Settings, the
    /// Live tab or the tray: any of them answers the question.
    pub fn choose_live(&mut self, on: bool) {
        self.live_logging = on;
        self.live_asked = true;
    }

    pub fn region(&self) -> String {
        self.main
            .as_ref()
            .and_then(|m| m.region.clone())
            .filter(|r| r == "eu" || r == "us")
            .unwrap_or_else(|| self.region.clone())
    }
}

/// Only https, or plain http to this computer (a local development server).
pub fn valid_origin(s: &str) -> Option<String> {
    let s = s.trim().trim_end_matches('/');
    let rest = s.strip_prefix("https://").or_else(|| {
        s.strip_prefix("http://").filter(|r| {
            let host = r.split([':', '/']).next().unwrap_or("");
            host == "127.0.0.1" || host == "localhost" || host == "[::1]"
        })
    })?;
    let host = rest.split('/').next().unwrap_or("");
    // `@` would make what's before it a user name and what's after the host.
    (!host.is_empty() && !rest.contains(['?', '#', ' ', '@', '\\'])).then(|| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origins() {
        assert_eq!(
            valid_origin("https://mythics.gg/").as_deref(),
            Some("https://mythics.gg")
        );
        assert_eq!(
            valid_origin("http://127.0.0.1:8000").as_deref(),
            Some("http://127.0.0.1:8000")
        );
        assert_eq!(valid_origin("http://example.com"), None);
        assert_eq!(valid_origin("ftp://x"), None);
        assert_eq!(valid_origin("https://"), None);
        assert_eq!(valid_origin("http://127.0.0.1@evil.example"), None);
        assert_eq!(valid_origin("https://mythics.gg@evil.example"), None);
    }

    #[test]
    fn defaults_and_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("settings.json");
        let s = Settings::load(&p);
        assert_eq!(s.default_visibility, Visibility::Public);
        assert!(!s.start_with_windows);
        assert_eq!(s.site_origin, "https://mythics.gg");
        assert!(
            !s.live_logging,
            "live logging is off until the player chooses"
        );
        assert!(!s.live_asked);
        let mut s2 = s.clone();
        s2.default_visibility = Visibility::Private;
        s2.live_logging = true;
        s2.live_asked = true;
        s2.save(&p).unwrap();
        assert_eq!(Settings::load(&p), s2);
        let raw = std::fs::read_to_string(&p).unwrap();
        assert!(!raw.contains("token"));
    }

    #[test]
    fn settings_from_before_the_switch_start_with_live_logging_off() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("settings.json");
        std::fs::write(&p, r#"{"first_run_done":true,"region":"us"}"#).unwrap();
        let s = Settings::load(&p);
        assert_eq!(s.region, "us");
        assert!(!s.live_logging);
        assert!(!s.live_asked, "so the app asks");
        assert_eq!(
            s.backlog_pulls,
            BacklogPulls::KillsAndBestWipe,
            "past logs send kills and each boss's best wipe unless the player chooses"
        );
    }

    #[test]
    fn archiving_is_off_until_the_player_turns_it_on_and_is_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("settings.json");
        // Settings saved before archiving existed.
        std::fs::write(&p, r#"{"first_run_done":true,"live_logging":true}"#).unwrap();
        let s = Settings::load(&p);
        assert!(!s.archive_uploaded, "off by default");
        assert_eq!(s.archive_delete_after_days, 0, "never deleted by default");
        let s = Settings {
            archive_uploaded: true,
            archive_delete_after_days: 60,
            ..s
        };
        s.save(&p).unwrap();
        let back = Settings::load(&p);
        assert!(back.archive_uploaded);
        assert_eq!(back.archive_delete_after_days, 60);
    }

    #[test]
    fn the_backlog_setting_is_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("settings.json");
        let s = Settings {
            backlog_pulls: BacklogPulls::All,
            ..Settings::default()
        };
        s.save(&p).unwrap();
        let raw = std::fs::read_to_string(&p).unwrap();
        assert!(raw.contains(r#""backlog_pulls": "all""#), "{raw}");
        assert_eq!(Settings::load(&p).backlog_pulls, BacklogPulls::All);
    }

    #[test]
    fn the_first_run_answer_is_kept_either_way() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("settings.json");
        for on in [false, true] {
            let mut s = Settings::load(&p);
            s.choose_live(on);
            s.save(&p).unwrap();
            let back = Settings::load(&p);
            assert_eq!(back.live_logging, on);
            assert!(back.live_asked, "not asked again");
        }
    }

    #[test]
    fn region_prefers_the_main_character() {
        let mut s = Settings::default();
        assert_eq!(s.region(), "eu");
        s.main = Some(Main {
            region: Some("us".into()),
            ..Main::default()
        });
        assert_eq!(s.region(), "us");
    }
}
