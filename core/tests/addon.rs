//! The addon updater against a fake game folder (`_retail_` with its
//! `Interface\AddOns`, another addon and `WTF`) and a local stub standing in
//! for mythics.gg, serving a generated release zip and its `latest.json`.

mod common;

use common::Stub;
use mythics_logger_core::addon::{self, Action, AddonError, Game, Outcome, Source, Want};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

const FOLDERS: [&str; 3] = ["Mythics", "Mythics_Data_EU", "Mythics_Data_US"];
const ZIP_PATH: &str = "/data/addon/Mythics-2.1.0-ab12.zip";

struct Fake {
    _tmp: tempfile::TempDir,
    retail: PathBuf,
    downloads: PathBuf,
    game: Game,
}

impl Fake {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let retail = tmp.path().join("World of Warcraft").join("_retail_");
        let logs = retail.join("Logs");
        std::fs::create_dir_all(&logs).unwrap();
        // Another addon, and the player's saved settings: never touched.
        let dbm = retail.join("Interface/AddOns/DBM-Core");
        std::fs::create_dir_all(&dbm).unwrap();
        std::fs::write(dbm.join("DBM-Core.toc"), "## Version: 12.0.1\n").unwrap();
        let sv = retail.join("WTF/Account/ACCOUNT1/SavedVariables");
        std::fs::create_dir_all(&sv).unwrap();
        std::fs::write(sv.join("Mythics.lua"), "MythicsDB = { shown = true }\n").unwrap();
        std::fs::write(retail.join("WTF/Config.wtf"), "SET locale \"enGB\"\n").unwrap();
        let downloads = tmp.path().join("app-data");
        Self {
            game: Game::from_logs_dir(&logs).unwrap(),
            _tmp: tmp,
            retail,
            downloads,
        }
    }

    fn addons(&self) -> PathBuf {
        self.retail.join("Interface").join("AddOns")
    }

    /// Puts an installed copy of the addon in place, as files.
    fn put(&self, version: &str, folders: &[&str]) {
        for f in folders {
            let dir = self.addons().join(f);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(format!("{f}.toc")), toc(f, version)).unwrap();
            std::fs::write(dir.join("Old.lua"), format!("-- {version}\n")).unwrap();
        }
    }

    /// Every file under `_retail_` and its bytes.
    fn tree(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(root, &p, out);
                } else {
                    out.insert(
                        p.strip_prefix(root).unwrap().to_path_buf(),
                        std::fs::read(&p).unwrap(),
                    );
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(&self.retail, &self.retail, &mut out);
        out
    }

    /// Everything but the addon's own folders: other addons and WTF.
    fn others(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        self.tree()
            .into_iter()
            .filter(|(p, _)| {
                !FOLDERS
                    .iter()
                    .any(|f| p.starts_with(Path::new("Interface/AddOns").join(f)))
            })
            .collect()
    }

    fn version_of(&self, folder: &str) -> Option<String> {
        addon::toc_version(&self.addons().join(folder).join(format!("{folder}.toc")))
    }

    /// Nothing of ours left behind: no work folder in Interface, no download.
    fn assert_clean(&self) {
        let work: Vec<_> = std::fs::read_dir(self.retail.join("Interface"))
            .map(|r| {
                r.flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|n| n != "AddOns")
                    .collect()
            })
            .unwrap_or_default();
        assert!(work.is_empty(), "left in Interface: {work:?}");
        let downloads: Vec<_> = std::fs::read_dir(&self.downloads)
            .map(|r| r.flatten().map(|e| e.path()).collect())
            .unwrap_or_default();
        assert!(downloads.is_empty(), "downloads left: {downloads:?}");
    }
}

fn toc(folder: &str, version: &str) -> String {
    format!("## Interface: 120100, 120105\r\n## Title: {folder}\r\n## Version: {version}\r\n\r\nCore.lua\r\n")
}

/// A zip of these entries; a name ending `/` is a folder.
fn zip_of(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut zw = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, bytes) in entries {
        if let Some(dir) = name.strip_suffix('/') {
            zw.add_directory(dir, opts).unwrap();
        } else {
            zw.start_file(*name, opts).unwrap();
            zw.write_all(bytes).unwrap();
        }
    }
    zw.finish().unwrap().into_inner()
}

/// The entries of a release, as the site's build makes it.
fn release_entries(version: &str, folders: &[&str]) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    for f in folders {
        out.push((format!("{f}/"), Vec::new()));
        out.push((format!("{f}/{f}.toc"), toc(f, version).into_bytes()));
        out.push((
            format!("{f}/Core.lua"),
            format!("-- {f} {version}\n").into_bytes(),
        ));
    }
    out
}

fn release(version: &str, folders: &[&str]) -> Vec<u8> {
    let e = release_entries(version, folders);
    zip_of(
        &e.iter()
            .map(|(n, b)| (n.as_str(), b.clone()))
            .collect::<Vec<_>>(),
    )
}

fn sha_hex(b: &[u8]) -> String {
    Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

fn latest_for(zip: &[u8], version: &str) -> String {
    serde_json::json!({
        "version": version,
        "published": "2026-10-01T12:00:00Z",
        "zip": ZIP_PATH.trim_start_matches("/data/"),
        "sha256": sha_hex(zip),
        "size": zip.len(),
        "folders": FOLDERS,
        "interface": "120100, 120105",
    })
    .to_string()
}

/// mythics.gg: `latest.json` (or a 404) and the zip.
fn site(latest: Option<String>, zip: Vec<u8>) -> Stub {
    Stub::start_bytes(move |r| match r.path.as_str() {
        "/data/addon/latest.json" => match &latest {
            Some(l) => (200, l.clone().into_bytes()),
            None => (404, b"{}".to_vec()),
        },
        p if p == ZIP_PATH => (200, zip.clone()),
        _ => (404, Vec::new()),
    })
}

fn paths(stub: &Stub) -> Vec<String> {
    stub.taken().into_iter().map(|r| r.path).collect()
}

async fn run(fake: &Fake, stub: &Stub, want: Want, running: bool) -> Result<Outcome, AddonError> {
    addon::sync(
        &Source::new(&stub.origin),
        &fake.game,
        want,
        &|| running,
        &fake.downloads,
    )
    .await
}

fn action_of(o: &Outcome) -> Action {
    match o {
        Outcome::Checked { action, .. }
        | Outcome::WaitingForGame { action, .. }
        | Outcome::Installed { action, .. } => *action,
    }
}

#[tokio::test]
async fn no_addon_is_offered_then_installed_when_the_player_says_yes() {
    let fake = Fake::new();
    let zip = release("2.1.0", &FOLDERS);
    let stub = site(Some(latest_for(&zip, "2.1.0")), zip);
    let before = fake.tree();

    // Keeping it up to date never installs one that isn't there.
    let o = run(&fake, &stub, Want::Auto, false).await.unwrap();
    assert!(
        matches!(
            o,
            Outcome::Checked {
                action: Action::Install,
                ..
            }
        ),
        "{o:?}"
    );
    assert_eq!(paths(&stub), ["/data/addon/latest.json"]);
    assert_eq!(fake.tree(), before);

    let o = run(&fake, &stub, Want::Install, false).await.unwrap();
    assert!(
        matches!(
            o,
            Outcome::Installed {
                action: Action::Install,
                ..
            }
        ),
        "{o:?}"
    );
    assert_eq!(paths(&stub), ["/data/addon/latest.json", ZIP_PATH]);
    for f in FOLDERS {
        assert_eq!(fake.version_of(f).as_deref(), Some("2.1.0"), "{f}");
    }
    assert_eq!(fake.others(), before, "other addons and WTF untouched");
    fake.assert_clean();
}

#[tokio::test]
async fn an_older_version_is_replaced_whole() {
    let fake = Fake::new();
    fake.put("2.0.0", &FOLDERS);
    let others = fake.others();
    let zip = release("2.1.0", &FOLDERS);
    let stub = site(Some(latest_for(&zip, "2.1.0")), zip);

    let o = run(&fake, &stub, Want::Auto, false).await.unwrap();
    assert!(
        matches!(
            o,
            Outcome::Installed {
                action: Action::Update,
                ..
            }
        ),
        "{o:?}"
    );
    for f in FOLDERS {
        assert_eq!(fake.version_of(f).as_deref(), Some("2.1.0"));
        // Replaced, not added to: the old version's file is gone.
        assert!(!fake.addons().join(f).join("Old.lua").exists());
        assert!(fake.addons().join(f).join("Core.lua").exists());
    }
    assert_eq!(fake.others(), others, "other addons and WTF untouched");
    fake.assert_clean();
}

#[tokio::test]
async fn the_same_version_does_nothing() {
    let fake = Fake::new();
    fake.put("2.1.0", &FOLDERS);
    let before = fake.tree();
    let zip = release("2.1.0", &FOLDERS);
    let stub = site(Some(latest_for(&zip, "2.1.0")), zip);
    for want in [Want::Auto, Want::Install] {
        let o = run(&fake, &stub, want, false).await.unwrap();
        assert!(
            matches!(
                o,
                Outcome::Checked {
                    action: Action::Nothing,
                    ..
                }
            ),
            "{o:?}"
        );
    }
    assert_eq!(
        paths(&stub),
        ["/data/addon/latest.json", "/data/addon/latest.json"]
    );
    assert_eq!(fake.tree(), before);
}

#[tokio::test]
async fn a_partial_or_mixed_install_is_repaired() {
    let fake = Fake::new();
    // A data pack missing.
    fake.put("2.1.0", &FOLDERS[..2]);
    let zip = release("2.1.0", &FOLDERS);
    let stub = site(Some(latest_for(&zip, "2.1.0")), zip);
    let o = run(&fake, &stub, Want::Auto, false).await.unwrap();
    assert!(
        matches!(
            o,
            Outcome::Installed {
                action: Action::Repair,
                ..
            }
        ),
        "{o:?}"
    );
    for f in FOLDERS {
        assert_eq!(fake.version_of(f).as_deref(), Some("2.1.0"));
    }

    // A data pack of another version than the core's.
    fake.put("2.0.0", &FOLDERS[2..]);
    let i = addon::installed(&fake.game, &FOLDERS.map(String::from));
    assert!(i.mixed);
    let o = run(&fake, &stub, Want::Auto, false).await.unwrap();
    assert!(
        matches!(
            o,
            Outcome::Installed {
                action: Action::Repair,
                ..
            }
        ),
        "{o:?}"
    );
    assert_eq!(fake.version_of("Mythics_Data_US").as_deref(), Some("2.1.0"));
    fake.assert_clean();
}

#[tokio::test]
async fn while_the_game_runs_it_waits_then_installs_once_it_closes() {
    let fake = Fake::new();
    fake.put("2.0.0", &FOLDERS);
    let before = fake.tree();
    let zip = release("2.1.0", &FOLDERS);
    let stub = site(Some(latest_for(&zip, "2.1.0")), zip);
    let running = Arc::new(AtomicBool::new(true));
    let check = {
        let r = running.clone();
        move || r.load(Ordering::SeqCst)
    };
    let source = Source::new(&stub.origin);

    let o = addon::sync(&source, &fake.game, Want::Auto, &check, &fake.downloads)
        .await
        .unwrap();
    assert!(
        matches!(
            o,
            Outcome::WaitingForGame {
                action: Action::Update,
                ..
            }
        ),
        "{o:?}"
    );
    assert_eq!(
        paths(&stub),
        ["/data/addon/latest.json"],
        "nothing downloaded"
    );
    assert_eq!(fake.tree(), before, "nothing written");

    running.store(false, Ordering::SeqCst);
    let o = addon::sync(&source, &fake.game, Want::Auto, &check, &fake.downloads)
        .await
        .unwrap();
    assert!(matches!(o, Outcome::Installed { .. }), "{o:?}");
    assert_eq!(fake.version_of("Mythics").as_deref(), Some("2.1.0"));
    fake.assert_clean();
}

#[tokio::test]
async fn a_bad_checksum_or_size_is_refused_and_nothing_written() {
    let fake = Fake::new();
    fake.put("2.0.0", &FOLDERS);
    let before = fake.tree();
    let zip = release("2.1.0", &FOLDERS);
    let good: serde_json::Value = serde_json::from_str(&latest_for(&zip, "2.1.0")).unwrap();

    let mut wrong_sha = good.clone();
    wrong_sha["sha256"] = serde_json::json!(sha_hex(b"something else"));
    let mut wrong_size = good.clone();
    wrong_size["size"] = serde_json::json!(zip.len() - 1);
    // The bytes served aren't the release's: one byte changed.
    let mut tampered = zip.clone();
    let last = tampered.len() - 30;
    tampered[last] ^= 0xff;

    for (latest, served) in [
        (wrong_sha, zip.clone()),
        (wrong_size, zip.clone()),
        (good.clone(), tampered),
    ] {
        let stub = site(Some(latest.to_string()), served);
        let r = run(&fake, &stub, Want::Auto, false).await;
        assert_eq!(r, Err(AddonError::Checksum));
        assert_eq!(fake.tree(), before, "nothing written");
        fake.assert_clean();
    }
}

#[tokio::test]
async fn zip_slip_entries_are_refused() {
    let fake = Fake::new();
    fake.put("2.0.0", &FOLDERS);
    let before = fake.tree();
    let evil_names = [
        "../evil.lua",
        "Mythics/../../../WTF/Config.wtf",
        "/Interface/AddOns/evil.lua",
        "C:/Windows/evil.dll",
        "Mythics\\..\\..\\evil.lua",
        "evil.lua",
    ];
    for evil in evil_names {
        let mut e = release_entries("2.1.0", &FOLDERS);
        e.push((evil.to_string(), b"-- evil\n".to_vec()));
        let zip = zip_of(
            &e.iter()
                .map(|(n, b)| (n.as_str(), b.clone()))
                .collect::<Vec<_>>(),
        );
        // The name went into the zip as written: the updater is what refuses it.
        let za = zip::ZipArchive::new(std::io::Cursor::new(zip.clone())).unwrap();
        assert!(za.file_names().any(|n| n == evil), "{evil}");
        let stub = site(Some(latest_for(&zip, "2.1.0")), zip);
        let r = run(&fake, &stub, Want::Auto, false).await;
        assert_eq!(r, Err(AddonError::BadZip), "{evil}");
        assert_eq!(fake.tree(), before, "nothing written for {evil}");
        fake.assert_clean();
    }

    // A link inside the zip.
    let mut zw = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default();
    for (n, b) in release_entries("2.1.0", &FOLDERS) {
        if let Some(d) = n.strip_suffix('/') {
            zw.add_directory(d, opts).unwrap();
        } else {
            zw.start_file(n, opts).unwrap();
            zw.write_all(&b).unwrap();
        }
    }
    zw.add_symlink("Mythics/WTF", "../../../WTF", opts).unwrap();
    let zip = zw.finish().unwrap().into_inner();
    let stub = site(Some(latest_for(&zip, "2.1.0")), zip);
    assert_eq!(
        run(&fake, &stub, Want::Auto, false).await,
        Err(AddonError::BadZip)
    );
    assert_eq!(fake.tree(), before);
    fake.assert_clean();
}

#[tokio::test]
async fn a_folder_not_listed_or_a_listed_one_missing_is_refused() {
    let fake = Fake::new();
    fake.put("2.0.0", &FOLDERS);
    let before = fake.tree();

    // Another addon's folder in the zip: refused, and it's left as it was.
    let mut e = release_entries("2.1.0", &FOLDERS);
    e.push(("DBM-Core/DBM-Core.toc".into(), b"## Version: 0\n".to_vec()));
    let with_other = zip_of(
        &e.iter()
            .map(|(n, b)| (n.as_str(), b.clone()))
            .collect::<Vec<_>>(),
    );
    // A listed data pack missing from the zip.
    let missing = release("2.1.0", &FOLDERS[..2]);
    // The zip's .toc says another version than latest.json.
    let wrong_version = release("2.0.9", &FOLDERS);

    for zip in [with_other, missing, wrong_version] {
        let stub = site(Some(latest_for(&zip, "2.1.0")), zip);
        assert_eq!(
            run(&fake, &stub, Want::Auto, false).await,
            Err(AddonError::BadZip)
        );
        assert_eq!(fake.tree(), before);
        fake.assert_clean();
    }
}

#[tokio::test]
async fn no_latest_json_says_the_addon_isnt_available_yet() {
    let fake = Fake::new();
    let before = fake.tree();
    let stub = site(None, Vec::new());
    for want in [Want::Check, Want::Auto, Want::Install] {
        assert_eq!(
            run(&fake, &stub, want, false).await,
            Err(AddonError::NotAvailable)
        );
    }
    assert_eq!(fake.tree(), before);

    // Something that isn't latest.json at all.
    let stub = Stub::start(|_| (200, "<html>maintenance</html>".into()));
    assert_eq!(
        run(&fake, &stub, Want::Install, false).await,
        Err(AddonError::BadLatest)
    );
    assert_eq!(fake.tree(), before);
}

#[tokio::test]
async fn a_newer_or_unbuilt_copy_is_left_alone_unless_asked() {
    let fake = Fake::new();
    let zip = release("2.1.0", &FOLDERS);
    let stub = site(Some(latest_for(&zip, "2.1.0")), zip);

    fake.put("2.2.0-test.1", &FOLDERS);
    let before = fake.tree();
    for want in [Want::Auto, Want::Install] {
        let o = run(&fake, &stub, want, false).await.unwrap();
        assert_eq!(action_of(&o), Action::Newer);
        assert!(matches!(o, Outcome::Checked { .. }));
    }
    assert_eq!(fake.tree(), before);

    // A source checkout copied in: not by itself, only if the player asks.
    fake.put(addon::UNBUILT_VERSION, &FOLDERS);
    let o = run(&fake, &stub, Want::Auto, false).await.unwrap();
    assert!(matches!(
        o,
        Outcome::Checked {
            action: Action::Unbuilt,
            ..
        }
    ));
    let o = run(&fake, &stub, Want::Install, false).await.unwrap();
    assert!(matches!(o, Outcome::Installed { .. }));
    assert_eq!(fake.version_of("Mythics").as_deref(), Some("2.1.0"));
}

/// A file open in a data pack (no sharing, as an editor or the game might)
/// stops the swap part-way: every folder goes back as it was.
#[cfg(windows)]
#[tokio::test]
async fn a_folder_in_use_rolls_everything_back() {
    use std::os::windows::fs::OpenOptionsExt;
    let fake = Fake::new();
    fake.put("2.0.0", &FOLDERS);
    let before = fake.tree();
    let zip = release("2.1.0", &FOLDERS);
    let stub = site(Some(latest_for(&zip, "2.1.0")), zip);
    let held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(fake.addons().join("Mythics_Data_US/Old.lua"))
        .unwrap();
    let r = run(&fake, &stub, Want::Auto, false).await;
    assert_eq!(r, Err(AddonError::InUse));
    drop(held);
    assert_eq!(fake.tree(), before, "every folder back as it was");
    fake.assert_clean();

    // Once it's closed, the next try works.
    let o = run(&fake, &stub, Want::Auto, false).await.unwrap();
    assert!(matches!(o, Outcome::Installed { .. }));
}

/// A linked addon folder (a developer's checkout) is never written through.
#[cfg(windows)]
#[tokio::test]
async fn a_linked_folder_is_left_alone() {
    let fake = Fake::new();
    let checkout = fake._tmp.path().join("checkout").join("Mythics");
    std::fs::create_dir_all(&checkout).unwrap();
    std::fs::write(checkout.join("Mythics.toc"), toc("Mythics", "2.0.0")).unwrap();
    std::fs::create_dir_all(fake.addons()).unwrap();
    // A junction needs no special rights, unlike a symbolic link.
    let made = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(fake.addons().join("Mythics"))
        .arg(&checkout)
        .output()
        .is_ok_and(|o| o.status.success());
    if !made {
        eprintln!("couldn't make a junction here; skipped");
        return;
    }
    let zip = release("2.1.0", &FOLDERS);
    let stub = site(Some(latest_for(&zip, "2.1.0")), zip);
    let o = run(&fake, &stub, Want::Install, false).await.unwrap();
    assert!(
        matches!(
            o,
            Outcome::Checked {
                action: Action::Linked,
                ..
            }
        ),
        "{o:?}"
    );
    assert_eq!(
        addon::toc_version(&checkout.join("Mythics.toc")).as_deref(),
        Some("2.0.0")
    );
    assert!(!fake.addons().join("Mythics_Data_EU").exists());
}
