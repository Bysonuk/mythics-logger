//! Installing the mythics.gg in-game addon and keeping it up to date (this
//! repository's issue 7, the owner's decisions of 1 Oct 2026). Only when the
//! player says yes: the app asks once, "Install the mythics.gg addon?", and
//! "Keep the addon up to date" in Settings is off unless they did.
//!
//! **Where it comes from:** mythics.gg itself, never CurseForge.
//! `GET <site>/data/addon/latest.json` says which release is current:
//! `{"version", "published", "zip", "sha256", "size", "folders", "interface"}`,
//! with `zip` relative to `<site>/data/` and `folders` the zip's top-level
//! folders (`Mythics` and each data pack). Until the site publishes it (a
//! 404), the addon "isn't available yet" and nothing else happens.
//!
//! The rules, all checked here:
//!
//! - **Checked before anything is written.** The zip is downloaded to a
//!   temporary file, and its size and SHA-256 checked against `latest.json`
//!   before it's opened. A mismatch is refused, and the file deleted.
//! - **Every entry checked before any is extracted.** Only the listed folders
//!   at the top level, every one of them present with its `.toc`; no
//!   absolute paths, drive letters, `..`, links, encrypted entries or names
//!   Windows can't hold. Then it's extracted to a folder of our own beside
//!   `AddOns` (in `Interface`), and the `.toc` versions read back.
//! - **Each folder replaced whole.** For each listed folder, the old one is
//!   moved aside and the new one moved in (renames on one volume); on any
//!   failure every move is undone, so versions never mix. The old folders
//!   are deleted only once all are in place.
//! - **Nothing else.** Only folders named `Mythics` or `Mythics_…` (a
//!   `latest.json` naming any other is refused), never another addon's
//!   folder, never `WTF` (the player's saved settings), and never a folder
//!   that is a link (a developer's checkout): that is left alone.
//! - **Never while the game runs.** The game reads `AddOns` when it starts;
//!   the caller says whether it's running ([`game_running`]: the names of
//!   running programs only, never a handle to the game's process), and an
//!   update waits for it to close.
//! - **HTTPS only** (plain http only to this computer, for a development
//!   server); redirects only to https. Nothing is sent but the two GETs.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

/// Where the site says which release is current.
pub const LATEST_PATH: &str = "/data/addon/latest.json";
/// The core addon's folder; every listed folder is this or starts `Mythics_`.
pub const CORE: &str = "Mythics";
/// What a source checkout's `.toc` says: the release build writes the
/// version in. Such a copy is a developer's, and isn't updated by itself.
pub const UNBUILT_VERSION: &str = "@project-version@";
/// The release zip's budget is 2 MB (the site's `MAX_ZIP_BYTES`); anything
/// claiming to be far bigger is refused before a byte is fetched.
pub const MAX_ZIP_BYTES: u64 = 32 * 1024 * 1024;
const MAX_LATEST_BYTES: usize = 64 * 1024;
/// Zip bombs: the most the extracted files may hold, and how many entries.
const MAX_UNPACKED_BYTES: u64 = 256 * 1024 * 1024;
const MAX_ENTRIES: usize = 10_000;
/// The app's own working folders in `Interface`, beside `AddOns`. Only
/// folders with this prefix are ever cleaned up there.
const WORK_PREFIX: &str = ".mythics-addon-";
/// The game's programs, any flavour: the addon isn't touched while one runs.
pub const GAME_PROCESSES: &[&str] = &[
    "Wow.exe",
    "WowT.exe",
    "WowB.exe",
    "Wow-64.exe",
    "WowClassic.exe",
    "WowClassicT.exe",
    "WowClassicB.exe",
];

/// The current release, as `latest.json` gives it, checked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Latest {
    pub version: String,
    pub published: Option<String>,
    /// Relative to `<site>/data/`: `addon/<name>.zip`.
    pub zip: String,
    /// Lower-case hex.
    pub sha256: String,
    pub size: u64,
    pub folders: Vec<String>,
    pub interface: Option<String>,
}

#[derive(Deserialize)]
struct RawLatest {
    version: String,
    #[serde(default)]
    published: Option<String>,
    zip: String,
    sha256: String,
    size: u64,
    folders: Vec<String>,
    #[serde(default)]
    interface: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AddonError {
    /// `latest.json` isn't there (404): the site doesn't publish the addon yet.
    #[error("the addon isn't available from mythics.gg yet")]
    NotAvailable,
    /// No network, a timeout, or the site is down.
    #[error("couldn't reach mythics.gg")]
    Offline,
    /// `latest.json` is malformed, or names something it mustn't.
    #[error("latest.json isn't usable")]
    BadLatest,
    /// The download's size or SHA-256 isn't what `latest.json` says.
    #[error("the download didn't match its checksum")]
    Checksum,
    /// The zip holds something it mustn't, or lacks a listed folder.
    #[error("the zip isn't a release of the addon")]
    BadZip,
    /// No `_retail_` folder above the Logs folder.
    #[error("no game folder")]
    NoGame,
    /// One of the addon's folders is a link: left alone.
    #[error("an addon folder is a link")]
    Linked,
    /// A file in an addon folder is open in another program.
    #[error("an addon folder is in use")]
    InUse,
    #[error("the disk is full")]
    DiskFull,
    #[error("file error ({0:?})")]
    Io(io::ErrorKind),
}

impl AddonError {
    /// The window's code for it (`src/format.ts`).
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotAvailable => "addon_unavailable",
            Self::Offline => "offline",
            Self::BadLatest => "addon_bad_latest",
            Self::Checksum => "addon_checksum",
            Self::BadZip => "addon_bad_zip",
            Self::NoGame => "addon_no_game",
            Self::Linked => "addon_linked",
            Self::InUse => "addon_in_use",
            Self::DiskFull => "disk_full",
            Self::Io(_) => "addon_io",
        }
    }
}

impl From<io::Error> for AddonError {
    fn from(e: io::Error) -> Self {
        // Windows' "being used by another process" (32), "locked" (33) and
        // "access denied" (5), which renaming a folder with an open file
        // gives.
        if cfg!(windows) && matches!(e.raw_os_error(), Some(5 | 32 | 33)) {
            return Self::InUse;
        }
        match e.kind() {
            io::ErrorKind::StorageFull => Self::DiskFull,
            k => Self::Io(k),
        }
    }
}

impl From<zip::result::ZipError> for AddonError {
    fn from(e: zip::result::ZipError) -> Self {
        match e {
            // Reading our own temporary file failed: a file error. A bad
            // CRC also comes as an I/O error from the reader, but only
            // while extracting, which maps it itself.
            zip::result::ZipError::Io(e) => e.into(),
            _ => Self::BadZip,
        }
    }
}

/// A plain addon folder name we may write: `Mythics` or `Mythics_…`, letters,
/// digits and `_` only.
fn is_our_folder(name: &str) -> bool {
    (name == CORE
        || name
            .strip_prefix("Mythics_")
            .is_some_and(|rest| !rest.is_empty()))
        && name.len() <= 64
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// A version as the `.toc` and `latest.json` write it: `2.1.0`, maybe with a
/// pre-release part.
fn is_version(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 40
        && v.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'+'))
}

/// Reads and checks `latest.json`. Anything unsafe in it is refused whole.
pub fn parse_latest(bytes: &[u8]) -> Result<Latest, AddonError> {
    let raw: RawLatest = serde_json::from_slice(bytes).map_err(|_| AddonError::BadLatest)?;
    let zip_ok = raw.zip.strip_prefix("addon/").is_some_and(|name| {
        name.ends_with(".zip")
            && !name.starts_with('.')
            && name.len() <= 200
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
    });
    let sha_ok = raw.sha256.len() == 64 && raw.sha256.bytes().all(|b| b.is_ascii_hexdigit());
    let folders_ok = raw.folders.iter().any(|f| f == CORE)
        && raw.folders.iter().all(|f| is_our_folder(f))
        && raw.folders.len() <= 64
        && {
            let mut seen: Vec<String> = raw.folders.iter().map(|f| f.to_lowercase()).collect();
            seen.sort();
            seen.dedup();
            seen.len() == raw.folders.len()
        };
    if !(zip_ok
        && sha_ok
        && folders_ok
        && is_version(&raw.version)
        && raw.size > 0
        && raw.size <= MAX_ZIP_BYTES)
    {
        return Err(AddonError::BadLatest);
    }
    Ok(Latest {
        version: raw.version,
        published: raw.published,
        zip: raw.zip,
        sha256: raw.sha256.to_ascii_lowercase(),
        size: raw.size,
        folders: raw.folders,
        interface: raw.interface.and_then(|v| match v {
            serde_json::Value::String(s) => Some(s),
            serde_json::Value::Number(n) => Some(n.to_string()),
            _ => None,
        }),
    })
}

/// Compares two versions like `2.1.0` and `2.1.0-test.3`: numbers first, and
/// a pre-release before its release. `None` if either isn't one.
pub fn compare_versions(a: &str, b: &str) -> Option<Ordering> {
    fn parse(v: &str) -> Option<(Vec<u64>, Option<&str>)> {
        let v = v.split('+').next()?;
        let (core, pre) = match v.split_once('-') {
            Some((c, p)) => (c, Some(p)),
            None => (v, None),
        };
        let nums = core
            .split('.')
            .map(|p| p.parse().ok())
            .collect::<Option<Vec<u64>>>()?;
        (!nums.is_empty()).then_some((nums, pre))
    }
    let (an, ap) = parse(a)?;
    let (bn, bp) = parse(b)?;
    let len = an.len().max(bn.len());
    let at = |v: &[u64], i: usize| v.get(i).copied().unwrap_or(0);
    for i in 0..len {
        match at(&an, i).cmp(&at(&bn, i)) {
            Ordering::Equal => {}
            o => return Some(o),
        }
    }
    Some(match (ap, bp) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(x), Some(y)) => compare_pre(x, y),
    })
}

fn compare_pre(a: &str, b: &str) -> Ordering {
    let mut ai = a.split('.');
    let mut bi = b.split('.');
    loop {
        match (ai.next(), bi.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => {
                let o = match (x.parse::<u64>(), y.parse::<u64>()) {
                    (Ok(m), Ok(n)) => m.cmp(&n),
                    (Ok(_), Err(_)) => Ordering::Less,
                    (Err(_), Ok(_)) => Ordering::Greater,
                    (Err(_), Err(_)) => x.cmp(y),
                };
                if o != Ordering::Equal {
                    return o;
                }
            }
        }
    }
}

/// The retail game's folder, found from the Logs folder the app already
/// knows (`wowdir`): `<World of Warcraft>\_retail_`. The addon is retail's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Game {
    retail: PathBuf,
}

impl Game {
    /// Only a Logs folder directly in a `_retail_` folder.
    pub fn from_logs_dir(logs_dir: &Path) -> Option<Self> {
        let retail = logs_dir.parent()?;
        let named = retail
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.eq_ignore_ascii_case("_retail_"));
        (named && retail.is_dir()).then(|| Self {
            retail: retail.to_path_buf(),
        })
    }

    /// `_retail_\Interface`.
    pub fn interface(&self) -> PathBuf {
        self.retail.join("Interface")
    }

    /// `_retail_\Interface\AddOns`.
    pub fn addons(&self) -> PathBuf {
        self.interface().join("AddOns")
    }
}

/// The addon as it is on disk.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Installed {
    /// `Mythics.toc`'s `## Version`, if there is one.
    pub version: Option<String>,
    /// The listed folders that are there, and those that aren't.
    pub present: Vec<String>,
    pub missing: Vec<String>,
    /// A data pack's `.toc` says another version than the core's.
    pub mixed: bool,
    /// One of the folders is a link (a developer's checkout).
    pub linked: bool,
}

/// `## Version:` from a `.toc`, if it has a sensible one. Reads at most the
/// first 64 KB.
pub fn toc_version(path: &Path) -> Option<String> {
    let mut buf = Vec::new();
    File::open(path)
        .ok()?
        .take(64 * 1024)
        .read_to_end(&mut buf)
        .ok()?;
    let text = String::from_utf8_lossy(&buf);
    text.lines().find_map(|line| {
        let line = line.trim_start_matches('\u{feff}').trim();
        let rest = line.strip_prefix("##")?.trim_start();
        let (tag, value) = rest.split_once(':')?;
        (tag.trim().eq_ignore_ascii_case("version"))
            .then(|| value.trim().to_string())
            .filter(|v| is_version(v) || v == UNBUILT_VERSION)
    })
}

/// Reads the installed addon: the core's version, and which of `folders`
/// are there. With no `latest.json` yet, pass `&[CORE]`.
pub fn installed(game: &Game, folders: &[String]) -> Installed {
    let addons = game.addons();
    let mut out = Installed::default();
    let mut pack_versions = Vec::new();
    for f in folders {
        let dir = addons.join(f);
        match fs::symlink_metadata(&dir) {
            Ok(m) if m.file_type().is_symlink() => {
                out.linked = true;
                out.present.push(f.clone());
            }
            Ok(m) if m.is_dir() => {
                out.present.push(f.clone());
                let v = toc_version(&dir.join(format!("{f}.toc")));
                if f == CORE {
                    out.version = v;
                } else {
                    pack_versions.push(v);
                }
            }
            _ => out.missing.push(f.clone()),
        }
    }
    out.mixed = out.version.is_some() && pack_versions.iter().any(|v| *v != out.version);
    out
}

/// What the app would do about the installed addon, given the latest release.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Up to date, every folder there.
    Nothing,
    /// Not installed at all: none of its folders is there.
    Install,
    /// An older version.
    Update,
    /// Some folders missing, a `.toc` without a version, or packs of another
    /// version than the core's.
    Repair,
    /// A newer version than the site's (a test build): left alone.
    Newer,
    /// A source checkout (`@project-version@`): updated only if the player
    /// asks.
    Unbuilt,
    /// A folder is a link: left alone, always.
    Linked,
}

pub fn action(i: &Installed, latest: &Latest) -> Action {
    if i.linked {
        return Action::Linked;
    }
    if i.present.is_empty() {
        return Action::Install;
    }
    let Some(v) = i.version.as_deref() else {
        return Action::Repair;
    };
    if v == UNBUILT_VERSION {
        return Action::Unbuilt;
    }
    if v == latest.version {
        return if i.missing.is_empty() && !i.mixed {
            Action::Nothing
        } else {
            Action::Repair
        };
    }
    match compare_versions(v, &latest.version) {
        Some(Ordering::Greater) => Action::Newer,
        _ => Action::Update,
    }
}

/// Only https, or plain http to this computer (a development server).
fn allowed_url(url: &reqwest::Url) -> bool {
    match url.scheme() {
        "https" => true,
        "http" => matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]")),
        _ => false,
    }
}

/// Fetches the release from the site in Settings.
pub struct Source {
    origin: String,
    http: reqwest::Client,
}

/// A downloaded zip whose size and SHA-256 matched `latest.json`: the only
/// thing [`install`] takes.
#[derive(Debug)]
pub struct Verified {
    path: PathBuf,
    latest: Latest,
}

impl Verified {
    pub fn latest(&self) -> &Latest {
        &self.latest
    }
}

impl Drop for Verified {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

impl Source {
    pub fn new(origin: &str) -> Self {
        // The app's client (user agent, timeouts), following a redirect only
        // to https.
        let http = crate::api::client_builder()
            .redirect(reqwest::redirect::Policy::custom(|a| {
                if a.previous().len() >= 5 {
                    a.error("too many redirects")
                } else if a.url().scheme() == "https" {
                    a.follow()
                } else {
                    a.stop()
                }
            }))
            .build()
            .expect("HTTP client");
        Self {
            origin: origin.trim_end_matches('/').to_string(),
            http,
        }
    }

    fn url(&self, path: &str) -> Result<reqwest::Url, AddonError> {
        let url = reqwest::Url::parse(&format!("{}{}", self.origin, path))
            .map_err(|_| AddonError::BadLatest)?;
        allowed_url(&url)
            .then_some(url)
            .ok_or(AddonError::BadLatest)
    }

    async fn get(&self, path: &str) -> Result<reqwest::Response, AddonError> {
        let resp = self.http.get(self.url(path)?).send().await.map_err(|e| {
            log::info!(
                "addon check failed: {}",
                if e.is_timeout() {
                    "timed out"
                } else {
                    "network error"
                }
            );
            AddonError::Offline
        })?;
        match resp.status().as_u16() {
            200 => Ok(resp),
            // The public bucket answers 403 for a key that isn't there.
            403 | 404 | 410 => Err(AddonError::NotAvailable),
            429 | 500..=599 => Err(AddonError::Offline),
            _ => Err(AddonError::BadLatest),
        }
    }

    /// The current release, or `NotAvailable` until the site publishes one.
    pub async fn latest(&self) -> Result<Latest, AddonError> {
        let mut resp = self.get(LATEST_PATH).await?;
        let mut body = Vec::new();
        while let Some(c) = resp.chunk().await.map_err(|_| AddonError::Offline)? {
            body.extend_from_slice(&c);
            if body.len() > MAX_LATEST_BYTES {
                return Err(AddonError::BadLatest);
            }
        }
        parse_latest(&body)
    }

    /// Downloads the release's zip into `dir`, checking its size and
    /// SHA-256 as it goes; a mismatch deletes the file and is refused.
    pub async fn download(&self, latest: &Latest, dir: &Path) -> Result<Verified, AddonError> {
        fs::create_dir_all(dir)?;
        // A download cut short when the app last closed.
        for e in fs::read_dir(dir)?.flatten() {
            if e.file_name()
                .to_str()
                .is_some_and(|n| n.starts_with("addon-download-"))
            {
                let _ = fs::remove_file(e.path());
            }
        }
        let path = dir.join(format!("addon-download-{}.zip", std::process::id()));
        let r = self.download_to(latest, &path).await;
        match r {
            Ok(()) => Ok(Verified {
                path,
                latest: latest.clone(),
            }),
            Err(e) => {
                let _ = fs::remove_file(&path);
                Err(e)
            }
        }
    }

    async fn download_to(&self, latest: &Latest, path: &Path) -> Result<(), AddonError> {
        let mut resp = self.get(&format!("/data/{}", latest.zip)).await?;
        if resp.content_length().is_some_and(|n| n != latest.size) {
            return Err(AddonError::Checksum);
        }
        let mut out = File::create(path)?;
        let mut hash = Sha256::new();
        let mut got = 0u64;
        while let Some(c) = resp.chunk().await.map_err(|_| AddonError::Offline)? {
            got += c.len() as u64;
            if got > latest.size {
                return Err(AddonError::Checksum);
            }
            hash.update(&c);
            out.write_all(&c)?;
        }
        out.sync_all()?;
        let sha: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
        if got != latest.size || sha != latest.sha256 {
            log::warn!("the addon's download didn't match its checksum: refused");
            return Err(AddonError::Checksum);
        }
        Ok(())
    }
}

/// Windows' reserved device names, which no file may take.
fn is_reserved(component: &str) -> bool {
    let stem = component
        .split('.')
        .next()
        .unwrap_or("")
        .to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && stem.as_bytes()[3].is_ascii_digit())
}

/// One path component a Windows folder can hold, and nothing special.
fn is_plain_component(c: &str) -> bool {
    !c.is_empty()
        && c != "."
        && c != ".."
        && c.len() <= 128
        && !c.ends_with(['.', ' '])
        && c.bytes().all(|b| {
            (0x20..0x7f).contains(&b)
                && !matches!(b, b'<' | b'>' | b':' | b'"' | b'|' | b'?' | b'*')
        })
        && !is_reserved(c)
}

/// Checks one entry's name: relative, `/`-separated, plain components only,
/// under one of `folders`. Returns its components (a directory's without
/// the trailing `/`).
pub fn check_entry_name<'a>(name: &'a str, folders: &[String]) -> Result<Vec<&'a str>, AddonError> {
    if name.is_empty() || name.starts_with('/') || name.contains('\\') || name.contains('\0') {
        return Err(AddonError::BadZip);
    }
    let trimmed = name.strip_suffix('/').unwrap_or(name);
    let parts: Vec<&str> = trimmed.split('/').collect();
    if !parts.iter().all(|p| is_plain_component(p)) {
        return Err(AddonError::BadZip);
    }
    // Only inside a listed folder: no file at the top level, no other folder.
    if !folders.iter().any(|f| f == parts[0]) || (parts.len() == 1 && !name.ends_with('/')) {
        return Err(AddonError::BadZip);
    }
    Ok(parts)
}

struct Entry {
    index: usize,
    parts: Vec<String>,
    dir: bool,
    size: u64,
}

/// Every entry checked, before anything is extracted.
fn check_zip(za: &mut zip::ZipArchive<File>, latest: &Latest) -> Result<Vec<Entry>, AddonError> {
    if za.len() > MAX_ENTRIES {
        return Err(AddonError::BadZip);
    }
    let mut out = Vec::with_capacity(za.len());
    let mut seen = std::collections::HashSet::new();
    let mut total = 0u64;
    for index in 0..za.len() {
        let f = za.by_index_raw(index)?;
        if !f.name_raw().is_ascii() || f.is_symlink() || f.encrypted() {
            return Err(AddonError::BadZip);
        }
        let dir = f.name().ends_with('/');
        let parts = check_entry_name(f.name(), &latest.folders)?;
        // Windows names ignore case: two entries one file is a trick.
        if !seen.insert(parts.join("/").to_ascii_lowercase()) {
            return Err(AddonError::BadZip);
        }
        total = total.saturating_add(f.size());
        if total > MAX_UNPACKED_BYTES {
            return Err(AddonError::BadZip);
        }
        out.push(Entry {
            index,
            parts: parts.iter().map(|p| p.to_string()).collect(),
            dir,
            size: f.size(),
        });
    }
    // Every listed folder, each with its own `.toc`.
    for folder in &latest.folders {
        let toc = format!("{folder}/{folder}.toc").to_ascii_lowercase();
        if !seen.contains(&toc) {
            return Err(AddonError::BadZip);
        }
    }
    Ok(out)
}

fn extract(
    za: &mut zip::ZipArchive<File>,
    entries: &[Entry],
    into: &Path,
) -> Result<(), AddonError> {
    for e in entries {
        let target = e.parts.iter().fold(into.to_path_buf(), |p, c| p.join(c));
        if e.dir {
            fs::create_dir_all(&target)?;
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut f = za.by_index(e.index)?;
        let mut out = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)?;
        // The zip crate checks each entry's CRC-32 as it reaches the end.
        let n = io::copy(&mut (&mut f).take(e.size + 1), &mut out).map_err(|e| {
            if e.kind() == io::ErrorKind::InvalidData {
                AddonError::BadZip
            } else {
                e.into()
            }
        })?;
        if n != e.size {
            return Err(AddonError::BadZip);
        }
    }
    Ok(())
}

/// A moved folder, to undo.
enum Moved {
    /// The installed folder, moved aside to `old`.
    Aside(String),
    /// The new folder, moved into `AddOns`.
    Placed(String),
}

/// Removes this app's own leftovers in `Interface` from an install that was
/// cut short (the app closed mid-way): only folders named with our prefix.
fn clean_leftovers(interface: &Path) {
    let Ok(entries) = fs::read_dir(interface) else {
        return;
    };
    for e in entries.flatten() {
        let ours = e
            .file_name()
            .to_str()
            .is_some_and(|n| n.starts_with(WORK_PREFIX));
        let real_dir = e.file_type().is_ok_and(|t| t.is_dir() && !t.is_symlink());
        if ours && real_dir && fs::remove_dir_all(e.path()).is_err() {
            log::warn!("couldn't remove an old addon work folder");
        }
    }
}

/// Installs a verified release into the game's `AddOns`: every listed folder
/// replaced whole, nothing else touched. The caller has checked the game
/// isn't running.
pub fn install(v: &Verified, game: &Game) -> Result<(), AddonError> {
    let latest = &v.latest;
    let interface = game.interface();
    let addons = game.addons();
    // Refuse before anything is written: a linked folder is a developer's.
    for f in &latest.folders {
        if fs::symlink_metadata(addons.join(f)).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(AddonError::Linked);
        }
    }
    let mut za = zip::ZipArchive::new(File::open(&v.path)?)?;
    let entries = check_zip(&mut za, latest)?;

    fs::create_dir_all(&addons)?;
    clean_leftovers(&interface);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let work = interface.join(format!("{WORK_PREFIX}{}-{nanos}", std::process::id()));
    fs::create_dir(&work)?;
    let r = install_from(&mut za, &entries, latest, &addons, &work);
    if fs::remove_dir_all(&work).is_err() {
        log::warn!("couldn't remove the addon work folder; removed next time");
    }
    r
}

fn install_from(
    za: &mut zip::ZipArchive<File>,
    entries: &[Entry],
    latest: &Latest,
    addons: &Path,
    work: &Path,
) -> Result<(), AddonError> {
    let new = work.join("new");
    let old = work.join("old");
    fs::create_dir(&new)?;
    fs::create_dir(&old)?;
    extract(za, entries, &new)?;
    // What was extracted says the version latest.json does, every folder.
    for f in &latest.folders {
        if toc_version(&new.join(f).join(format!("{f}.toc"))).as_deref()
            != Some(latest.version.as_str())
        {
            return Err(AddonError::BadZip);
        }
    }

    let mut done: Vec<Moved> = Vec::new();
    let r = (|| -> Result<(), AddonError> {
        for f in &latest.folders {
            let dst = addons.join(f);
            if fs::symlink_metadata(&dst).is_ok() {
                fs::rename(&dst, old.join(f))?;
                done.push(Moved::Aside(f.clone()));
            }
            fs::rename(new.join(f), &dst)?;
            done.push(Moved::Placed(f.clone()));
        }
        Ok(())
    })();
    if let Err(e) = r {
        log::warn!("couldn't put the addon in place: {}; undoing", e.code());
        for m in done.iter().rev() {
            let undone = match m {
                Moved::Placed(f) => fs::rename(addons.join(f), new.join(f)),
                Moved::Aside(f) => fs::rename(old.join(f), addons.join(f)),
            };
            if undone.is_err() {
                log::warn!("couldn't undo a step of the addon install");
            }
        }
        return Err(e);
    }
    Ok(())
}

/// What the caller wants of [`sync`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Want {
    /// Only find out the latest version ("Check now" with updates off).
    Check,
    /// "Keep the addon up to date": update or repair an installed addon;
    /// never install one the player removed, and never replace a test
    /// build, a checkout or a link.
    Auto,
    /// The player asked: install, update or repair (never over a link or a
    /// newer version).
    Install,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Looked, and did nothing: `action` is what would be done.
    Checked { latest: Latest, action: Action },
    /// Something to do, but the game is running: try again once it's closed.
    WaitingForGame { latest: Latest, action: Action },
    /// Installed `latest`.
    Installed { latest: Latest, action: Action },
}

/// One pass: ask the site for the latest release, compare it with what's
/// installed, and install it if `want` says so and the game isn't running
/// (asked just before downloading and again just before installing).
pub async fn sync(
    source: &Source,
    game: &Game,
    want: Want,
    game_running: &(dyn Fn() -> bool + Sync),
    download_dir: &Path,
) -> Result<Outcome, AddonError> {
    let latest = source.latest().await?;
    let now = installed(game, &latest.folders);
    let action = action(&now, &latest);
    let go = match want {
        Want::Check => false,
        Want::Auto => matches!(action, Action::Update | Action::Repair),
        Want::Install => matches!(
            action,
            Action::Install | Action::Update | Action::Repair | Action::Unbuilt
        ),
    };
    if !go {
        return Ok(Outcome::Checked { latest, action });
    }
    if game_running() {
        return Ok(Outcome::WaitingForGame { latest, action });
    }
    let verified = source.download(&latest, download_dir).await?;
    if game_running() {
        return Ok(Outcome::WaitingForGame { latest, action });
    }
    install(&verified, game)?;
    log::info!("installed the addon ({action:?})");
    Ok(Outcome::Installed { latest, action })
}

/// Whether a running program's name is the game's.
pub fn is_game_process(name: &str) -> bool {
    GAME_PROCESSES.iter().any(|g| g.eq_ignore_ascii_case(name))
}

/// Whether World of Warcraft is running, any flavour. Reads only the list
/// of running programs' names (a Toolhelp snapshot); it never opens the
/// game's process. If the list can't be read, it says yes, so nothing is
/// replaced under a running game.
#[cfg(windows)]
pub fn game_running() -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    // SAFETY: a snapshot handle checked before use and closed once; the
    // entry is plain data sized as the API requires.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            log::warn!("couldn't list running programs; assuming the game is running");
            return true;
        }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut found = false;
        let mut ok = Process32FirstW(snap, &mut e) != 0;
        while ok {
            let len = e
                .szExeFile
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(e.szExeFile.len());
            if is_game_process(&String::from_utf16_lossy(&e.szExeFile[..len])) {
                found = true;
                break;
            }
            ok = Process32NextW(snap, &mut e) != 0;
        }
        CloseHandle(snap);
        found
    }
}

/// TODO(macOS): the game's process name there.
#[cfg(not(windows))]
pub fn game_running() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn latest_json(folders: &[&str]) -> String {
        serde_json::json!({
            "version": "2.1.0",
            "published": "2026-10-01T12:00:00Z",
            "zip": "addon/Mythics-2.1.0-0123abcd.zip",
            "sha256": "a".repeat(64),
            "size": 1_650_000,
            "folders": folders,
            "interface": "120100, 120105",
        })
        .to_string()
    }

    #[test]
    fn latest_json_is_read_and_checked() {
        let l = parse_latest(latest_json(&["Mythics", "Mythics_Data_EU"]).as_bytes()).unwrap();
        assert_eq!(l.version, "2.1.0");
        assert_eq!(l.folders, ["Mythics", "Mythics_Data_EU"]);
        assert_eq!(l.interface.as_deref(), Some("120100, 120105"));
        // Never another addon's folder, nor one that leaves AddOns.
        for bad in [
            &["Mythics", "DBM-Core"][..],
            &["Mythics", "..\\WTF"],
            &["Mythics", "Mythics_/x"],
            &["Mythics_Data_EU"],
            &["Mythics", "mythics"],
            &[],
        ] {
            assert_eq!(
                parse_latest(latest_json(bad).as_bytes()),
                Err(AddonError::BadLatest),
                "{bad:?}"
            );
        }
        for (field, value) in [
            ("zip", serde_json::json!("../addon/x.zip")),
            ("zip", serde_json::json!("https://evil.example/x.zip")),
            ("zip", serde_json::json!("addon/../../x.zip")),
            ("zip", serde_json::json!("addon/x.exe")),
            ("sha256", serde_json::json!("nothex")),
            ("size", serde_json::json!(0)),
            ("size", serde_json::json!(MAX_ZIP_BYTES + 1)),
            ("version", serde_json::json!("2.1.0; rm")),
        ] {
            let mut v: serde_json::Value =
                serde_json::from_str(&latest_json(&["Mythics"])).unwrap();
            v[field] = value;
            assert_eq!(
                parse_latest(v.to_string().as_bytes()),
                Err(AddonError::BadLatest),
                "{field}"
            );
        }
        assert_eq!(parse_latest(b"<html>"), Err(AddonError::BadLatest));
    }

    #[test]
    fn versions_compare_as_numbers() {
        use Ordering::*;
        assert_eq!(compare_versions("2.1.0", "2.1.0"), Some(Equal));
        assert_eq!(compare_versions("2.1.0", "2.10.0"), Some(Less));
        assert_eq!(compare_versions("2.1.0-test.3", "2.1.0"), Some(Less));
        assert_eq!(
            compare_versions("2.1.0-test.10", "2.1.0-test.9"),
            Some(Greater)
        );
        assert_eq!(compare_versions("3.0.0", "2.9.9"), Some(Greater));
        assert_eq!(compare_versions("@project-version@", "2.1.0"), None);
    }

    #[test]
    fn entry_names_must_stay_inside_a_listed_folder() {
        let folders = vec!["Mythics".to_string(), "Mythics_Data_EU".to_string()];
        assert_eq!(
            check_entry_name("Mythics/Core.lua", &folders).unwrap(),
            ["Mythics", "Core.lua"]
        );
        assert!(check_entry_name("Mythics/", &folders).is_ok());
        assert!(check_entry_name("Mythics_Data_EU/Data.lua", &folders).is_ok());
        for bad in [
            "../evil.lua",
            "Mythics/../../WTF/Config.wtf",
            "Mythics/./Core.lua",
            "/Mythics/Core.lua",
            "C:/Windows/evil.dll",
            "C:evil",
            "Mythics\\..\\..\\evil",
            "Mythics//Core.lua",
            "DBM-Core/DBM-Core.toc",
            "WTF/Config.wtf",
            "readme.txt",
            "Mythics",
            "Mythics/CON.lua",
            "Mythics/Core.lua.",
            "Mythics/a:b",
        ] {
            assert_eq!(
                check_entry_name(bad, &folders),
                Err(AddonError::BadZip),
                "{bad}"
            );
        }
    }

    #[test]
    fn the_game_is_found_from_retails_logs_folder_only() {
        let tmp = tempfile::tempdir().unwrap();
        let logs = tmp.path().join("World of Warcraft/_retail_/Logs");
        fs::create_dir_all(&logs).unwrap();
        let g = Game::from_logs_dir(&logs).unwrap();
        assert_eq!(
            g.addons(),
            tmp.path()
                .join("World of Warcraft/_retail_/Interface/AddOns")
        );
        let classic = tmp.path().join("World of Warcraft/_classic_/Logs");
        fs::create_dir_all(&classic).unwrap();
        assert_eq!(Game::from_logs_dir(&classic), None);
    }

    #[test]
    fn toc_versions() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("Mythics.toc");
        fs::write(
            &p,
            "\u{feff}## Interface: 120100, 120105\r\n## Title: mythics\r\n## Version: 2.1.0\r\n\r\nCore.lua\r\n",
        )
        .unwrap();
        assert_eq!(toc_version(&p).as_deref(), Some("2.1.0"));
        fs::write(&p, "## Version: @project-version@\n").unwrap();
        assert_eq!(toc_version(&p).as_deref(), Some(UNBUILT_VERSION));
        fs::write(&p, "## Title: x\n").unwrap();
        assert_eq!(toc_version(&p), None);
        assert_eq!(toc_version(&tmp.path().join("none.toc")), None);
    }

    #[test]
    fn game_processes() {
        assert!(is_game_process("Wow.exe"));
        assert!(is_game_process("WOWCLASSIC.EXE"));
        // An addon manager isn't the game.
        assert!(!is_game_process("WowUp.exe"));
        assert!(!is_game_process("Battle.net.exe"));
    }
}
