//! Finding World of Warcraft's `Logs` folder.
//!
//! In order, stopping at the first that has a `Logs` folder: the folder the
//! player chose before (the app passes it in), the registry's install path,
//! the uninstall entry, then the default install folders. Otherwise the
//! player picks one, and `resolve_pick` says what's wrong in plain words.
//!
//! The app reads nothing else in the game's folder: not `WTF`, not saved
//! variables, not the game's files.

use std::path::{Path, PathBuf};

/// Every place worth looking, most likely first. Paths may not exist.
pub fn candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    #[cfg(windows)]
    out.extend(registry_candidates());
    #[cfg(windows)]
    {
        for drive in ["C", "D", "E", "F"] {
            for base in [
                r"Program Files (x86)\World of Warcraft",
                r"Program Files\World of Warcraft",
                r"World of Warcraft",
                r"Games\World of Warcraft",
                r"Battle.net\World of Warcraft",
            ] {
                out.push(PathBuf::from(format!(r"{drive}:\{base}")));
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        // TODO(macOS, Phase 5): confirm on a Mac.
        out.push(PathBuf::from("/Applications/World of Warcraft"));
    }
    // TODO(Phase 1): the Battle.net agent's install list under
    // C:\ProgramData\Battle.net\Agent\ (product.db, an undocumented protobuf).
    out
}

#[cfg(windows)]
fn registry_candidates() -> Vec<PathBuf> {
    use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY};
    use winreg::RegKey;
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let mut out = Vec::new();
    // The launcher writes the 32-bit view: a 64-bit app must ask for it.
    for (key, value) in [
        (
            r"SOFTWARE\Blizzard Entertainment\World of Warcraft",
            "InstallPath",
        ),
        (
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\World of Warcraft",
            "InstallLocation",
        ),
    ] {
        if let Ok(k) = hklm.open_subkey_with_flags(key, KEY_READ | KEY_WOW64_32KEY) {
            if let Ok(v) = k.get_value::<String, _>(value) {
                if let Some(p) = normalise_install_path(&v) {
                    out.push(p);
                }
            }
        }
    }
    out
}

/// The registry's `InstallPath` points at `_retail_`, sometimes with a
/// trailing backslash or without a drive letter.
pub fn normalise_install_path(raw: &str) -> Option<PathBuf> {
    let t = raw.trim().trim_matches('"').trim_end_matches(['\\', '/']);
    if t.is_empty() {
        return None;
    }
    let t = if t.starts_with('\\') && !t.starts_with(r"\\") {
        format!("C:{t}")
    } else {
        t.to_string()
    };
    Some(PathBuf::from(t))
}

/// The first candidate with a `Logs` folder, as that `Logs` folder.
pub fn find(previous: Option<&Path>) -> Option<PathBuf> {
    previous
        .map(Path::to_path_buf)
        .into_iter()
        .chain(candidates())
        .find_map(|p| resolve_pick(&p).ok())
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PickError {
    #[error("That folder doesn't exist.")]
    Missing,
    #[error("That folder has no Logs folder in it. Choose the World of Warcraft folder, or the _retail_ folder inside it.")]
    NoLogs,
}

/// Accepts the World of Warcraft folder, its `_retail_` folder, or the `Logs`
/// folder itself, and returns the `Logs` folder.
pub fn resolve_pick(p: &Path) -> Result<PathBuf, PickError> {
    if !p.is_dir() {
        return Err(PickError::Missing);
    }
    let is_logs = p
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.eq_ignore_ascii_case("logs"));
    let tries = [
        is_logs.then(|| p.to_path_buf()),
        Some(p.join("Logs")),
        Some(p.join("_retail_").join("Logs")),
    ];
    tries
        .into_iter()
        .flatten()
        .find(|t| t.is_dir())
        .ok_or(PickError::NoLogs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalises_registry_paths() {
        assert_eq!(
            normalise_install_path(r"C:\Program Files (x86)\World of Warcraft\_retail_\"),
            Some(PathBuf::from(
                r"C:\Program Files (x86)\World of Warcraft\_retail_"
            ))
        );
        assert_eq!(
            normalise_install_path(r"\Games\World of Warcraft\_retail_"),
            Some(PathBuf::from(r"C:\Games\World of Warcraft\_retail_"))
        );
        assert_eq!(normalise_install_path("  "), None);
    }

    #[test]
    fn accepts_the_game_retail_or_logs_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let logs = tmp
            .path()
            .join("World of Warcraft")
            .join("_retail_")
            .join("Logs");
        std::fs::create_dir_all(&logs).unwrap();
        let wow = tmp.path().join("World of Warcraft");
        assert_eq!(resolve_pick(&wow).unwrap(), logs);
        assert_eq!(resolve_pick(&wow.join("_retail_")).unwrap(), logs);
        assert_eq!(resolve_pick(&logs).unwrap(), logs);
        assert_eq!(resolve_pick(tmp.path()), Err(PickError::NoLogs));
        assert_eq!(
            resolve_pick(&tmp.path().join("nope")),
            Err(PickError::Missing)
        );
        assert_eq!(find(Some(&wow)), Some(logs));
    }
}
