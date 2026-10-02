//! The live report's link (this repository's issue 15; mythics.gg issue
//! 615): while live logging, a page of the log being written that anyone
//! with the link can open, `<site>/shared/<token>/`, updating as pulls
//! arrive. The contract's "Share a log by link".
//!
//! - Made once the log's first pull has uploaded and the server has said
//!   which log (session) it's in, and only when one of the log's pulls went
//!   Public: a link shows Public uploads only (`409 log_not_public`).
//! - The server keeps only the token's hash and gives the token once, so the
//!   app keeps it, for the current log only, in the credential store beside
//!   the app token (`live-share-link`), never in a file and never in its log.
//!   Asking again would make a new link and break the one the raid has.
//! - "Stop sharing" revokes it; the app remembers that for the log, and
//!   makes no new link for it unless the player selects "Make a new link".
//!   A link revoked on the site is noticed when the app checks it works
//!   (every 10 minutes while it's shown).
//! - A site without share links (a 404 with no code) hides the feature,
//!   with a note, until the app next starts.

use crate::state::{AppState, LiveStatus};
use crate::workers::changed;
use mythics_logger_core::api::{share_path, ApiError, ShareLink, UploadRow, Visibility};
use mythics_logger_core::queue::{Item, Origin, State};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::AppHandle;

const SERVICE: &str = "gg.mythics.logger";
const USER: &str = "live-share-link";

/// After an error such as offline: try again this much later.
const RETRY: Duration = Duration::from_secs(60);
/// After `log_not_public` from the server, though a pull went Public here
/// (it was changed on the site): ask again this much later.
const NOT_PUBLIC_RETRY: Duration = Duration::from_secs(5 * 60);
/// How often a shown link is checked to still work.
const CHECK_EVERY: Duration = Duration::from_secs(10 * 60);

/// The current log's link, as kept in the credential store. `token: None`
/// means the player stopped sharing this log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stored {
    pub session_id: String,
    pub token: Option<String>,
}

fn entry() -> Option<keyring::Entry> {
    keyring::Entry::new(SERVICE, USER).ok()
}

fn load() -> Option<Stored> {
    let raw = entry()?.get_password().ok()?;
    serde_json::from_str(&raw).ok()
}

fn save(s: &Stored) {
    let ok = serde_json::to_string(s)
        .ok()
        .and_then(|raw| entry()?.set_password(&raw).ok())
        .is_some();
    if !ok {
        // Kept for this run only: after a restart the app makes a new link.
        log::warn!("couldn't keep the live report link in the credential store");
    }
}

fn clear() {
    if let Some(e) = entry() {
        let _ = e.delete_credential();
    }
}

#[derive(Debug, Default)]
pub struct ShareState {
    /// Read from the credential store yet.
    loaded: bool,
    pub stored: Option<Stored>,
    /// The site has no share links (a 404 with no code).
    pub unavailable: bool,
    pub busy: bool,
    /// The last attempt's error code, and when to try again.
    pub error: Option<(String, Instant)>,
    /// The server said this log has no Public upload, and when.
    pub refused: Option<(String, Instant)>,
    /// When the shown link was last checked to work.
    pub checked: Option<(String, Instant)>,
}

impl ShareState {
    fn ensure_loaded(&mut self) {
        if !self.loaded {
            self.loaded = true;
            self.stored = load();
        }
    }

    /// The current log's stored entry, if it's this one.
    fn for_session(&self, session_id: &str) -> Option<&Stored> {
        self.stored.as_ref().filter(|s| s.session_id == session_id)
    }
}

/// The log being written, as far as sharing goes.
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    /// The server's log (session), once it has said; `None` before.
    pub session_id: Option<String>,
    /// Any pull of the log uploaded by this app.
    pub uploaded: bool,
    /// Any of them uploaded Public.
    pub public: bool,
    /// The newest one's visibility, to say why there's no link.
    pub visibility: Visibility,
}

/// The log being written now, if live logging is on and following one:
/// tonight's live pulls of that file this app has uploaded.
pub fn target(
    live: &LiveStatus,
    live_logging: bool,
    items: &[Item],
    server: &HashMap<String, UploadRow>,
) -> Option<Target> {
    let file = live.file.as_deref().filter(|_| live_logging)?;
    let sent: Vec<&Item> = items
        .iter()
        .filter(|i| {
            i.origin == Origin::Live
                && i.state == State::Done
                && i.upload_id.is_some()
                && i.file_name.as_deref() == Some(file)
        })
        .collect();
    let session_id = sent.iter().rev().find_map(|i| {
        server
            .get(i.upload_id.as_deref()?)
            .and_then(|r| r.session_id.clone())
    });
    Some(Target {
        session_id,
        uploaded: !sent.is_empty(),
        public: sent.iter().any(|i| i.visibility == Visibility::Public),
        visibility: sent.last().map(|i| i.visibility).unwrap_or_default(),
    })
}

/// What the Live tab shows.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ShareView {
    /// "off" (not live logging), "waiting" (no pull uploaded yet),
    /// "not_public", "creating", "ready", "revoked", "unavailable" or
    /// "error".
    pub status: &'static str,
    /// The link, when ready: the site's address and `/shared/<token>/`.
    pub url: Option<String>,
    /// Why there's no link: the log's pulls go Guild only or Private.
    pub visibility: Option<Visibility>,
    /// The last attempt's error code (`errorText`).
    pub error: Option<String>,
}

pub fn view(s: &ShareState, target: Option<&Target>, origin: &str) -> ShareView {
    let mut v = ShareView {
        status: "off",
        url: None,
        visibility: None,
        error: None,
    };
    let Some(t) = target else { return v };
    if s.unavailable {
        v.status = "unavailable";
        return v;
    }
    let Some(session) = t.session_id.as_deref().filter(|_| t.uploaded) else {
        v.status = "waiting";
        return v;
    };
    if let Some(stored) = s.for_session(session) {
        match stored.token.as_deref().and_then(share_path) {
            Some(path) => {
                v.status = "ready";
                v.url = crate::links::page_url(origin, &path);
            }
            None => v.status = "revoked",
        }
        return v;
    }
    let refused = s.refused.as_ref().is_some_and(|(r, _)| r == session);
    if !t.public || refused {
        v.status = "not_public";
        v.visibility = Some(if t.public {
            Visibility::Guild
        } else {
            t.visibility
        });
        return v;
    }
    if s.busy {
        v.status = "creating";
    } else if let Some((code, _)) = &s.error {
        v.status = "error";
        v.error = Some(code.clone());
    } else {
        v.status = "creating";
    }
    v
}

/// The current log as it stands, from the app's state.
fn current(state: &AppState) -> Option<Target> {
    let live_logging = state.settings.lock().expect("settings").live_logging;
    let live = state.live.lock().expect("live").clone();
    let q = state.queue.lock().expect("queue");
    let server = state.server_uploads.lock().expect("server uploads");
    target(&live, live_logging, q.items(), &server)
}

pub fn snapshot_view(state: &AppState, target: Option<&Target>) -> ShareView {
    let origin = state.settings.lock().expect("settings").site_origin.clone();
    let mut s = state.share.lock().expect("share");
    s.ensure_loaded();
    view(&s, target, &origin)
}

/// What to do now for the current log: make a link, check the one shown,
/// or nothing.
#[derive(Debug, PartialEq, Eq)]
pub enum Next {
    Create(String),
    Check(String),
    Nothing,
}

pub fn next(s: &ShareState, target: Option<&Target>, now: Instant) -> Next {
    let Some(t) = target else {
        return Next::Nothing;
    };
    let Some(session) = t.session_id.as_deref() else {
        return Next::Nothing;
    };
    if s.unavailable || s.busy || !t.public {
        return Next::Nothing;
    }
    if let Some(stored) = s.for_session(session) {
        return match &stored.token {
            Some(token) => {
                let due = s
                    .checked
                    .as_ref()
                    .is_none_or(|(c, at)| c != session || now.duration_since(*at) >= CHECK_EVERY);
                if due {
                    Next::Check(token.clone())
                } else {
                    Next::Nothing
                }
            }
            // The player stopped sharing it: only they make a new one.
            None => Next::Nothing,
        };
    }
    if s.refused
        .as_ref()
        .is_some_and(|(r, at)| r == session && now.duration_since(*at) < NOT_PUBLIC_RETRY)
    {
        return Next::Nothing;
    }
    if s.error
        .as_ref()
        .is_some_and(|(_, at)| now.duration_since(*at) < RETRY)
    {
        return Next::Nothing;
    }
    Next::Create(session.to_string())
}

/// Makes a link for the log and keeps it. Errors are the window's codes.
async fn create(app: &AppHandle, state: &AppState, session_id: &str) -> Result<(), String> {
    {
        let mut s = state.share.lock().expect("share");
        if s.busy {
            return Err("busy".into());
        }
        s.busy = true;
    }
    changed(app);
    let r = state.api().create_share(session_id).await;
    let mut s = state.share.lock().expect("share");
    s.busy = false;
    let out = match r {
        Ok(ShareLink { token, .. }) => {
            let stored = Stored {
                session_id: session_id.to_string(),
                token: Some(token),
            };
            save(&stored);
            s.stored = Some(stored);
            s.error = None;
            s.refused = None;
            s.checked = Some((session_id.to_string(), Instant::now()));
            log::info!("made the live report link");
            Ok(())
        }
        Err(e) => Err(refusal(&mut s, session_id, &e)),
    };
    drop(s);
    changed(app);
    out
}

/// Notes a refusal in the state; the window's code for it.
fn refusal(s: &mut ShareState, session_id: &str, e: &ApiError) -> String {
    match e {
        ApiError::Refused {
            status: 404 | 405,
            code: None,
        } => {
            log::info!("this site has no share links yet");
            s.unavailable = true;
            "share_unavailable".into()
        }
        ApiError::Refused { status: 409, code } if code.as_deref() == Some("log_not_public") => {
            s.refused = Some((session_id.to_string(), Instant::now()));
            "log_not_public".into()
        }
        e => {
            let code = crate::commands::api_code(e);
            log::info!("couldn't make the live report link: {code}");
            s.error = Some((code.clone(), Instant::now()));
            code
        }
    }
}

/// Whether the shown link still works; marks it stopped if not.
async fn check(app: &AppHandle, state: &AppState, session_id: &str, token: &str) {
    state.share.lock().expect("share").checked = Some((session_id.to_string(), Instant::now()));
    if let Ok(false) = state.api().share_works(token).await {
        let mut s = state.share.lock().expect("share");
        if s.for_session(session_id).and_then(|x| x.token.as_deref()) == Some(token) {
            let stopped = Stored {
                session_id: session_id.to_string(),
                token: None,
            };
            save(&stopped);
            s.stored = Some(stopped);
            log::info!("the live report link no longer works");
        }
        drop(s);
        changed(app);
    }
}

/// Makes the current log's link once it can, and checks it now and then.
pub async fn share_forever(app: AppHandle, state: Arc<AppState>) {
    loop {
        let _ = tokio::time::timeout(Duration::from_secs(5), state.share_wake.notified()).await;
        if !state.api().has_token() {
            continue;
        }
        let t = current(&state);
        let step = {
            let mut s = state.share.lock().expect("share");
            s.ensure_loaded();
            next(&s, t.as_ref(), Instant::now())
        };
        let session = t.and_then(|t| t.session_id);
        match (step, session) {
            (Next::Create(id), _) => {
                let _ = create(&app, &state, &id).await;
            }
            (Next::Check(token), Some(id)) => check(&app, &state, &id, &token).await,
            _ => {}
        }
    }
}

/// Logging out, or another site: the link belongs to that account and site.
pub fn forget(state: &AppState) {
    clear();
    let mut s = state.share.lock().expect("share");
    *s = ShareState {
        loaded: true,
        ..Default::default()
    };
}

/// "Make a new link": for the current log, now, even after Stop sharing.
#[tauri::command]
pub async fn live_share_new(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<(), String> {
    let id = current(&state).and_then(|t| t.session_id).ok_or("no_log")?;
    {
        let mut s = state.share.lock().expect("share");
        s.ensure_loaded();
        s.error = None;
        s.refused = None;
        if s.for_session(&id).is_some() {
            s.stored = None;
        }
    }
    create(&app, &state, &id).await
}

/// "Stop sharing": the link stops working at once.
#[tauri::command]
pub async fn live_share_revoke(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<(), String> {
    let id = current(&state).and_then(|t| t.session_id).ok_or("no_log")?;
    let r = state.api().revoke_share(&id).await;
    let mut s = state.share.lock().expect("share");
    let out = match r {
        // Already gone (revoked on the site): the same to the player.
        Ok(()) => Ok(()),
        Err(ApiError::Refused { status: 404, code })
            if code.as_deref() == Some("share_not_found") =>
        {
            Ok(())
        }
        Err(e) => Err(refusal(&mut s, &id, &e)),
    };
    if out.is_ok() {
        let stopped = Stored {
            session_id: id,
            token: None,
        };
        save(&stopped);
        s.stored = Some(stopped);
        log::info!("stopped sharing the live report");
    }
    drop(s);
    changed(&app);
    out
}

/// "Open in browser": the link's page, on the site in Settings only.
#[tauri::command]
pub fn live_share_open(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<(), String> {
    let id = current(&state).and_then(|t| t.session_id).ok_or("no_log")?;
    let path = {
        let s = state.share.lock().expect("share");
        s.for_session(&id)
            .and_then(|x| x.token.as_deref())
            .and_then(share_path)
            .ok_or("no_link")?
    };
    crate::commands::open_page(&app, &state, &path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mythics_logger_core::splitter::split_file;

    const RAID: &str = include_str!("../../core/tests/fixtures/raid_night.txt");
    const FILE: &str = "WoWCombatLog-092826_200101.txt";
    const TOKEN: &str = "AbCdEfGhIjKlMnOpQr_-12";

    /// Tonight's two raid pulls from the live log, as queued.
    fn items(visibility: Visibility) -> (tempfile::TempDir, Vec<Item>) {
        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join(FILE);
        std::fs::write(&log, RAID).unwrap();
        let mut segs = Vec::new();
        split_file(&log, |s| segs.push(s), |_| true).unwrap();
        let items = segs
            .into_iter()
            .map(|s| Item::new(Origin::Live, log.clone(), s, visibility, "eu"))
            .collect();
        (tmp, items)
    }

    fn live() -> LiveStatus {
        LiveStatus {
            status: "live",
            file: Some(FILE.into()),
            ..Default::default()
        }
    }

    fn uploaded(i: &mut Item, id: &str) {
        i.state = State::Done;
        i.upload_id = Some(id.into());
    }

    fn server(ids: &[&str]) -> HashMap<String, UploadRow> {
        ids.iter()
            .map(|id| {
                let r: UploadRow = serde_json::from_value(
                    serde_json::json!({"id": id, "status": "queued", "session_id": 12}),
                )
                .unwrap();
                (id.to_string(), r)
            })
            .collect()
    }

    const SITE: &str = "https://mythics.gg";

    #[test]
    fn not_logging_shows_nothing() {
        let (_t, items) = items(Visibility::Public);
        assert_eq!(target(&live(), false, &items, &HashMap::new()), None);
        let idle = LiveStatus {
            file: None,
            ..live()
        };
        assert_eq!(target(&idle, true, &items, &HashMap::new()), None);
        let v = view(&ShareState::default(), None, SITE);
        assert_eq!(v.status, "off");
        assert_eq!(
            next(&ShareState::default(), None, Instant::now()),
            Next::Nothing
        );
    }

    #[test]
    fn before_the_first_pull_uploads_it_waits() {
        let (_t, mut items) = items(Visibility::Public);
        let s = ShareState::default();
        // Nothing uploaded yet.
        let t = target(&live(), true, &items, &HashMap::new()).unwrap();
        assert!(!t.uploaded && t.session_id.is_none());
        assert_eq!(view(&s, Some(&t), SITE).status, "waiting");
        assert_eq!(next(&s, Some(&t), Instant::now()), Next::Nothing);
        // Uploaded, but the server hasn't said which log yet.
        uploaded(&mut items[0], "41");
        let t = target(&live(), true, &items, &HashMap::new()).unwrap();
        assert!(t.uploaded && t.session_id.is_none());
        assert_eq!(view(&s, Some(&t), SITE).status, "waiting");
        assert_eq!(next(&s, Some(&t), Instant::now()), Next::Nothing);
        // Another file's pulls aren't this log's.
        let other = LiveStatus {
            file: Some("WoWCombatLog-092926_200101.txt".into()),
            ..live()
        };
        assert!(
            !target(&other, true, &items, &server(&["41"]))
                .unwrap()
                .uploaded
        );
    }

    #[test]
    fn after_the_first_public_pull_it_makes_the_link_and_shows_it() {
        let (_t, mut items) = items(Visibility::Public);
        uploaded(&mut items[0], "41");
        let t = target(&live(), true, &items, &server(&["41"])).unwrap();
        assert_eq!(t.session_id.as_deref(), Some("12"));
        let mut s = ShareState::default();
        assert_eq!(
            next(&s, Some(&t), Instant::now()),
            Next::Create("12".into())
        );
        assert_eq!(view(&s, Some(&t), SITE).status, "creating");
        s.stored = Some(Stored {
            session_id: "12".into(),
            token: Some(TOKEN.into()),
        });
        let v = view(&s, Some(&t), SITE);
        assert_eq!(v.status, "ready");
        assert_eq!(
            v.url.as_deref(),
            Some("https://mythics.gg/shared/AbCdEfGhIjKlMnOpQr_-12/")
        );
        // Made once: never again for the same log, so the raid's link
        // keeps working; only checked now and then.
        let now = Instant::now();
        assert_eq!(next(&s, Some(&t), now), Next::Check(TOKEN.into()));
        s.checked = Some(("12".into(), now));
        assert_eq!(next(&s, Some(&t), now), Next::Nothing);
        assert_eq!(
            next(&s, Some(&t), now + CHECK_EVERY),
            Next::Check(TOKEN.into())
        );
        // The next log gets its own link.
        let mut rows = server(&["41"]);
        rows.get_mut("41").unwrap().session_id = Some("13".into());
        let t2 = target(&live(), true, &items, &rows).unwrap();
        assert_eq!(view(&s, Some(&t2), SITE).status, "creating");
        assert_eq!(next(&s, Some(&t2), now), Next::Create("13".into()));
    }

    #[test]
    fn a_private_or_guild_only_log_has_no_link_and_says_why() {
        for vis in [Visibility::Private, Visibility::Guild] {
            let (_t, mut items) = items(vis);
            uploaded(&mut items[0], "41");
            let t = target(&live(), true, &items, &server(&["41"])).unwrap();
            let s = ShareState::default();
            let v = view(&s, Some(&t), SITE);
            assert_eq!(v.status, "not_public");
            assert_eq!(v.visibility, Some(vis));
            assert_eq!(v.url, None);
            assert_eq!(next(&s, Some(&t), Instant::now()), Next::Nothing);
        }
        // The server says no (made Guild only on the site): asked again later.
        let (_t, mut items) = items(Visibility::Public);
        uploaded(&mut items[0], "41");
        let t = target(&live(), true, &items, &server(&["41"])).unwrap();
        let mut s = ShareState::default();
        let code = refusal(
            &mut s,
            "12",
            &ApiError::Refused {
                status: 409,
                code: Some("log_not_public".into()),
            },
        );
        assert_eq!(code, "log_not_public");
        let now = Instant::now();
        assert_eq!(view(&s, Some(&t), SITE).status, "not_public");
        assert_eq!(next(&s, Some(&t), now), Next::Nothing);
        assert_eq!(
            next(&s, Some(&t), now + NOT_PUBLIC_RETRY),
            Next::Create("12".into())
        );
    }

    #[test]
    fn a_revoked_link_stays_revoked_until_the_player_makes_a_new_one() {
        let (_t, mut items) = items(Visibility::Public);
        uploaded(&mut items[0], "41");
        let t = target(&live(), true, &items, &server(&["41"])).unwrap();
        let s = ShareState {
            stored: Some(Stored {
                session_id: "12".into(),
                token: None,
            }),
            ..Default::default()
        };
        let v = view(&s, Some(&t), SITE);
        assert_eq!(v.status, "revoked");
        assert_eq!(v.url, None);
        assert_eq!(next(&s, Some(&t), Instant::now()), Next::Nothing);
    }

    #[test]
    fn a_site_without_share_links_hides_the_feature() {
        let (_t, mut items) = items(Visibility::Public);
        uploaded(&mut items[0], "41");
        let t = target(&live(), true, &items, &server(&["41"])).unwrap();
        let mut s = ShareState::default();
        let code = refusal(
            &mut s,
            "12",
            &ApiError::Refused {
                status: 404,
                code: None,
            },
        );
        assert_eq!(code, "share_unavailable");
        assert_eq!(view(&s, Some(&t), SITE).status, "unavailable");
        assert_eq!(next(&s, Some(&t), Instant::now()), Next::Nothing);
    }

    #[test]
    fn an_error_is_shown_and_tried_again_a_minute_later() {
        let (_t, mut items) = items(Visibility::Public);
        uploaded(&mut items[0], "41");
        let t = target(&live(), true, &items, &server(&["41"])).unwrap();
        let mut s = ShareState::default();
        assert_eq!(refusal(&mut s, "12", &ApiError::Offline), "offline");
        // Later than the error, as the job's clock always is.
        let now = Instant::now();
        let v = view(&s, Some(&t), SITE);
        assert_eq!((v.status, v.error.as_deref()), ("error", Some("offline")));
        assert_eq!(next(&s, Some(&t), now), Next::Nothing);
        assert_eq!(next(&s, Some(&t), now + RETRY), Next::Create("12".into()));
    }

    #[test]
    fn the_stored_link_never_holds_more_than_the_log_and_token() {
        let s = Stored {
            session_id: "12".into(),
            token: Some(TOKEN.into()),
        };
        let raw = serde_json::to_string(&s).unwrap();
        assert_eq!(raw, format!(r#"{{"session_id":"12","token":"{TOKEN}"}}"#));
        // Well under Windows Credential Manager's 2,560-byte limit.
        assert!(raw.len() < 200);
    }
}
