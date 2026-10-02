//! The client for mythics.gg's logger API (`docs/specs/logger-api.md`).
//!
//! Every call but the token exchange carries the app token as a bearer header.
//! Nothing here logs a token, a response body or a character name.

use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
    /// On the site and in rankings.
    #[default]
    Public,
    /// The uploader's guild staff and members only.
    Guild,
    /// The uploader only.
    Private,
}

/// The body of `POST /api/logger/uploads`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NewUpload {
    pub sha256: String,
    pub size: u64,
    pub chunk_size: u64,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encounter_id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub difficulty: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_level: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub map_id: Option<u32>,
    pub start_time: String,
    pub end_time: String,
    pub region: String,
    pub visibility: Visibility,
    pub client_version: String,
    /// The log session: the same for every segment of one file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_key: Option<String>,
    /// The file's name, never its path; only with `session_key`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_start_time: Option<String>,
}

/// A pull or key about to be sent, for `POST /api/logger/fingerprints`
/// ("Ask before uploading").
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Fingerprint {
    /// "encounter" or "key".
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encounter_id: Option<u32>,
    /// A key's challenge mode id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub map_id: Option<u32>,
    pub start_time: String,
    pub roster_hash: String,
}

/// The server's answer for one fingerprint: `have` (with the copy's pages)
/// or `need`.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct FingerprintAnswer {
    pub status: String,
    #[serde(default, deserialize_with = "opt_string_or_number")]
    pub fight_id: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub log_url: Option<String>,
    #[serde(default)]
    pub boss_url: Option<String>,
}

impl FingerprintAnswer {
    pub fn have(&self) -> bool {
        self.status == "have"
    }
}

/// One wipe sent without its detail, for `POST /api/logger/pull-summaries`
/// (the contract's "Pull summaries"; `crate::plan` says which).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PullSummary {
    pub encounter_id: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub difficulty: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_size: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance_id: Option<u32>,
    pub start_time: String,
    pub end_time: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    pub boss_hp_pct: Option<f64>,
    /// `false` for a wipe, `None` for a pull the log never finished. Never
    /// `true`: a kill always goes in full.
    pub success: Option<bool>,
    pub roster_hash: String,
}

/// The body of `POST /api/logger/pull-summaries`: one log file's summaries.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PullSummaries {
    pub summaries: Vec<PullSummary>,
    pub region: String,
    pub visibility: Visibility,
    pub client_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_start_time: Option<String>,
}

/// The server's answer for one summary: `stored` (with its pages) or `have`
/// (a full copy the player may see stands for it; the pages are that copy's).
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct SummaryAnswer {
    pub status: String,
    #[serde(default, deserialize_with = "opt_string_or_number")]
    pub upload_id: Option<String>,
    #[serde(default, deserialize_with = "opt_string_or_number")]
    pub fight_id: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub log_url: Option<String>,
    #[serde(default)]
    pub boss_url: Option<String>,
}

/// The answer to create (or resume). The server sends `id` as a number; it's
/// kept as a string here, as the app uses it only in paths.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Created {
    #[serde(deserialize_with = "string_or_number")]
    pub id: String,
    /// `receiving`, `queued`, `parsed` or `failed`: anything but `receiving`
    /// is already complete on the server, so nothing more is sent.
    #[serde(default)]
    pub status: Option<String>,
    /// The chunk size the server holds the upload to. A repeat keeps the
    /// first request's, and the app must use this one.
    #[serde(default)]
    pub chunk_size: Option<u64>,
    #[serde(default)]
    pub received: Vec<u32>,
}

impl Created {
    /// Already complete on the server: queued, parsed or failed.
    pub fn is_complete(&self) -> bool {
        self.status.as_deref().is_some_and(|s| s != "receiving")
    }
}

/// The signed-in player, as the app shows them: their main character and
/// guild. The API also sends account details; the app keeps none of them
/// (and never a BattleTag).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Main {
    pub name: Option<String>,
    pub realm: Option<String>,
    pub region: Option<String>,
    pub guild: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SignedIn {
    pub token: String,
    pub main: Main,
}

/// One row of `GET /api/logger/uploads`. Fields the server adds later are
/// ignored; fields it leaves out are empty.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UploadRow {
    #[serde(deserialize_with = "string_or_number")]
    pub id: String,
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub encounter_id: Option<u32>,
    #[serde(default)]
    pub difficulty: Option<u32>,
    #[serde(default)]
    pub key_level: Option<u32>,
    #[serde(default)]
    pub map_id: Option<u32>,
    #[serde(default)]
    pub start_time: Option<String>,
    #[serde(default)]
    pub end_time: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub visibility: Option<Visibility>,
    #[serde(default)]
    pub status: Option<String>,
    /// The log session it's in: one per combat log file.
    #[serde(default, deserialize_with = "opt_string_or_number")]
    pub session_id: Option<String>,
    /// Its log's page on the site: a path, `null` until the server has parsed it.
    #[serde(default)]
    pub log_url: Option<String>,
    /// The same as `log_url`, from servers before sessions.
    #[serde(default)]
    pub url: Option<String>,
    /// What the parse found: empty until it's parsed.
    #[serde(default)]
    pub fights: Vec<FightRow>,
}

impl UploadRow {
    /// Its log's page: `log_url`, or `url` from an older server.
    pub fn log_page(&self) -> Option<&str> {
        self.log_url.as_deref().or(self.url.as_deref())
    }

    /// Sent, and the server is still receiving or reading it.
    pub fn is_processing(&self) -> bool {
        matches!(self.status.as_deref(), Some("receiving" | "queued"))
    }
}

/// A boss pull or a key the server found in an upload (the contract's
/// "One upload"), with the path of its page on the site.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FightRow {
    #[serde(deserialize_with = "string_or_number")]
    pub id: String,
    /// "encounter" or "key".
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub encounter_id: Option<u32>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub difficulty: Option<u32>,
    #[serde(default)]
    pub key_level: Option<u32>,
    /// The boss died, or the key was completed.
    #[serde(default)]
    pub kill: Option<bool>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    /// The boss's health at the end, in percent.
    #[serde(default)]
    pub boss_hp_pct: Option<f64>,
    /// A pull summary: the wipe's result only, its detail never uploaded.
    #[serde(default)]
    pub summary: bool,
    /// The key a pull was part of.
    #[serde(default, deserialize_with = "opt_string_or_number")]
    pub parent_id: Option<String>,
    /// Where it goes in its log: "raid", "mplus" (a key, or a boss inside
    /// one) or "dungeon" (a dungeon boss outside any key).
    #[serde(default)]
    pub section: Option<String>,
    /// The key a boss was fought in.
    #[serde(default, deserialize_with = "opt_string_or_number")]
    pub in_key: Option<String>,
    /// Its page: a pull's, or a key's.
    #[serde(default)]
    pub url: Option<String>,
    /// A raid pull's boss page: every pull of that boss in the log.
    #[serde(default)]
    pub boss_url: Option<String>,
}

/// `GET /api/logger/sessions/{id}`: one log's raid bosses and keys.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LogSession {
    #[serde(deserialize_with = "string_or_number")]
    pub id: String,
    #[serde(default)]
    pub log_url: Option<String>,
    #[serde(default)]
    pub raid: Vec<RaidBoss>,
    #[serde(default)]
    pub mplus: Vec<KeyRun>,
    #[serde(default)]
    pub dungeon: Vec<FightRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RaidBoss {
    pub encounter_id: u32,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub boss_url: Option<String>,
    #[serde(default)]
    pub killed: bool,
    #[serde(default)]
    pub pulls: Vec<FightRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct KeyRun {
    #[serde(flatten)]
    pub key: FightRow,
    #[serde(default)]
    pub bosses: Vec<FightRow>,
}

fn opt_string_or_number<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<String>, D::Error> {
    let v = Option::<serde_json::Value>::deserialize(d)?;
    Ok(match v {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(s)) => Some(s),
        Some(other) => Some(other.to_string()),
    })
}

fn string_or_number<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(match v {
        serde_json::Value::String(s) => s,
        other => other.to_string(),
    })
}

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    /// No network, a timeout, or mythics.gg is down: try again later.
    #[error("couldn't reach mythics.gg")]
    Offline,
    /// The token was revoked or expired: log in again.
    #[error("signed out")]
    Unauthorized,
    /// The server refused this request; `code` is its refusal code, if any.
    #[error("refused ({status})")]
    Refused { status: u16, code: Option<String> },
    /// A 5xx or 429: try again later, not before `retry_after_s` seconds if
    /// the server said (`Retry-After`: a rate limit, or the daily quota).
    #[error("server busy ({status})")]
    Busy {
        status: u16,
        retry_after_s: Option<u64>,
    },
    #[error("unexpected reply")]
    BadReply,
}

impl ApiError {
    /// Worth trying again later, as opposed to a refusal.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            ApiError::Offline | ApiError::Busy { .. } | ApiError::BadReply
        )
    }
}

/// 100 uploads a page: enough for the History tab and the Backlog tab's
/// "already uploaded" check without walking a huge account forever.
const MAX_LIST_PAGES: usize = 50;

#[derive(Clone)]
pub struct Api {
    origin: String,
    http: reqwest::Client,
    token: Option<String>,
}

/// The app's HTTP client settings: its user agent and timeouts. The addon's
/// updater (`addon.rs`) builds on the same.
pub fn client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .user_agent(concat!("mythics.gg-logger/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(120))
}

impl Api {
    pub fn new(origin: &str) -> Self {
        let http = client_builder().build().expect("HTTP client");
        Self {
            origin: origin.trim_end_matches('/').to_string(),
            http,
            token: None,
        }
    }

    pub fn with_token(mut self, token: Option<String>) -> Self {
        self.token = token;
        self
    }

    pub fn origin(&self) -> &str {
        &self.origin
    }

    pub fn has_token(&self) -> bool {
        self.token.is_some()
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.origin, path)
    }

    /// Where the browser goes to log in with Battle.net.
    pub fn auth_start_url(&self, port: u16, state: &str, challenge: &str) -> String {
        format!(
            "{}/api/logger/auth/start?port={port}&state={state}&code_challenge={challenge}",
            self.origin
        )
    }

    fn authed(&self, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.token {
            Some(t) => rb.bearer_auth(t),
            None => rb,
        }
    }

    async fn send(&self, rb: reqwest::RequestBuilder) -> Result<reqwest::Response, ApiError> {
        let resp = self.authed(rb).send().await.map_err(|e| {
            log::info!("request failed: {}", describe(&e));
            ApiError::Offline
        })?;
        let status = resp.status().as_u16();
        if resp.status().is_success() {
            return Ok(resp);
        }
        Err(match status {
            401 => ApiError::Unauthorized,
            429 | 500..=599 => ApiError::Busy {
                status,
                retry_after_s: resp
                    .headers()
                    .get(reqwest::header::RETRY_AFTER)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.trim().parse().ok()),
            },
            _ => {
                let code = resp.json::<serde_json::Value>().await.ok().and_then(|v| {
                    v.get("code")
                        .or_else(|| v.get("error"))
                        .and_then(|c| c.as_str().map(str::to_string))
                });
                ApiError::Refused { status, code }
            }
        })
    }

    /// Swaps the one-time code from the browser for an app token.
    pub async fn exchange(&self, code: &str, verifier: &str) -> Result<SignedIn, ApiError> {
        #[derive(Deserialize)]
        struct Reply {
            token: String,
            #[serde(default)]
            main: Option<serde_json::Value>,
        }
        let body = serde_json::json!({ "code": code, "code_verifier": verifier });
        let resp = self
            .send(
                self.http
                    .post(self.url("/api/logger/auth/token"))
                    .json(&body),
            )
            .await?;
        let r: Reply = resp.json().await.map_err(|_| ApiError::BadReply)?;
        Ok(SignedIn {
            token: r.token,
            main: r.main.as_ref().map(main_from).unwrap_or_default(),
        })
    }

    pub async fn revoke(&self) -> Result<(), ApiError> {
        self.send(self.http.post(self.url("/api/logger/auth/revoke")))
            .await
            .map(|_| ())
    }

    pub async fn create_upload(&self, u: &NewUpload) -> Result<Created, ApiError> {
        let resp = self
            .send(self.http.post(self.url("/api/logger/uploads")).json(u))
            .await?;
        resp.json().await.map_err(|_| ApiError::BadReply)
    }

    /// Asks whether the site already has these pulls and keys: one answer
    /// each, in order. At most 200 a call.
    pub async fn fingerprints(
        &self,
        prints: &[Fingerprint],
    ) -> Result<Vec<FingerprintAnswer>, ApiError> {
        #[derive(Deserialize)]
        struct Reply {
            fingerprints: Vec<FingerprintAnswer>,
        }
        let body = serde_json::json!({ "fingerprints": prints });
        let resp = self
            .send(
                self.http
                    .post(self.url("/api/logger/fingerprints"))
                    .json(&body),
            )
            .await?;
        let r: Reply = resp.json().await.map_err(|_| ApiError::BadReply)?;
        if r.fingerprints.len() != prints.len() {
            return Err(ApiError::BadReply);
        }
        Ok(r.fingerprints)
    }

    /// Sends one log file's wipes as summaries: one answer each, in order.
    /// At most 200 a call.
    pub async fn pull_summaries(
        &self,
        body: &PullSummaries,
    ) -> Result<Vec<SummaryAnswer>, ApiError> {
        #[derive(Deserialize)]
        struct Reply {
            summaries: Vec<SummaryAnswer>,
        }
        let resp = self
            .send(
                self.http
                    .post(self.url("/api/logger/pull-summaries"))
                    .json(body),
            )
            .await?;
        let r: Reply = resp.json().await.map_err(|_| ApiError::BadReply)?;
        if r.summaries.len() != body.summaries.len() {
            return Err(ApiError::BadReply);
        }
        Ok(r.summaries)
    }

    /// One log session: its raid bosses with their pulls, and its keys.
    pub async fn session(&self, id: &str) -> Result<LogSession, ApiError> {
        if !is_id(id) {
            return Err(ApiError::BadReply);
        }
        let resp = self
            .send(
                self.http
                    .get(self.url(&format!("/api/logger/sessions/{id}"))),
            )
            .await?;
        resp.json().await.map_err(|_| ApiError::BadReply)
    }

    pub async fn put_chunk(&self, id: &str, n: u32, body: Vec<u8>) -> Result<(), ApiError> {
        let rb = self
            .http
            .put(self.url(&format!("/api/logger/uploads/{id}/chunks/{n}")))
            .header(reqwest::header::CONTENT_TYPE, "application/zstd")
            .body(body);
        self.send(rb).await.map(|_| ())
    }

    pub async fn complete(&self, id: &str) -> Result<(), ApiError> {
        self.send(
            self.http
                .post(self.url(&format!("/api/logger/uploads/{id}/complete"))),
        )
        .await
        .map(|_| ())
    }

    /// The account's uploads, newest first: every page of
    /// `{"uploads": [...], "next_cursor": ...}`, up to `MAX_LIST_PAGES`.
    pub async fn list_uploads(&self) -> Result<Vec<UploadRow>, ApiError> {
        #[derive(Deserialize)]
        struct Page {
            uploads: Vec<UploadRow>,
            #[serde(default)]
            next_cursor: Option<String>,
        }
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_LIST_PAGES {
            let mut path = "/api/logger/uploads?limit=100".to_string();
            if let Some(c) = &cursor {
                path.push_str(&format!("&cursor={c}"));
            }
            let resp = self.send(self.http.get(self.url(&path))).await?;
            let page: Page = resp.json().await.map_err(|_| ApiError::BadReply)?;
            out.extend(page.uploads);
            // Cursors are ids; anything else isn't ours to put in a URL.
            match page.next_cursor {
                Some(c) if !c.is_empty() && c.bytes().all(|b| b.is_ascii_digit()) => {
                    cursor = Some(c)
                }
                _ => break,
            }
        }
        Ok(out)
    }

    /// The newest uploads, one page: enough to see whether the latest have
    /// been parsed without walking the whole account.
    pub async fn recent_uploads(&self, limit: u8) -> Result<Vec<UploadRow>, ApiError> {
        #[derive(Deserialize)]
        struct Page {
            uploads: Vec<UploadRow>,
        }
        let path = format!("/api/logger/uploads?limit={}", limit.clamp(1, 100));
        let resp = self.send(self.http.get(self.url(&path))).await?;
        let page: Page = resp.json().await.map_err(|_| ApiError::BadReply)?;
        Ok(page.uploads)
    }

    /// `id` comes from the window: only a numeric upload id goes into the
    /// path, so nothing else can steer the request elsewhere on the site.
    pub async fn set_visibility(&self, id: &str, visibility: Visibility) -> Result<(), ApiError> {
        if !is_id(id) {
            return Err(ApiError::BadReply);
        }
        let body = serde_json::json!({ "visibility": visibility });
        self.send(
            self.http
                .patch(self.url(&format!("/api/logger/uploads/{id}")))
                .json(&body),
        )
        .await
        .map(|_| ())
    }

    /// As `set_visibility`: a numeric upload id only.
    pub async fn delete_upload(&self, id: &str) -> Result<(), ApiError> {
        if !is_id(id) {
            return Err(ApiError::BadReply);
        }
        self.send(
            self.http
                .delete(self.url(&format!("/api/logger/uploads/{id}"))),
        )
        .await
        .map(|_| ())
    }
}

/// A log's share link (`POST /api/logger/sessions/{id}/share`): the page
/// anyone with it can open, `/shared/<token>/`. The server keeps only the
/// token's hash, so this answer is the only time it's given. Never logged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareLink {
    pub token: String,
    pub path: String,
    #[serde(default, rename = "createdAt")]
    pub created_at: Option<String>,
}

/// A share link's token as the server makes it: 22 URL-safe characters
/// (128 random bits). Nothing else goes into a path or an address.
pub fn is_share_token(s: &str) -> bool {
    s.len() == 22
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The share page's path for a token, `/shared/<token>/`.
pub fn share_path(token: &str) -> Option<String> {
    is_share_token(token).then(|| format!("/shared/{token}/"))
}

impl Api {
    /// Makes a share link for one of the account's logs (a session),
    /// replacing any it had: the old one stops working. `409
    /// log_not_public` when the log has no Public upload.
    pub async fn create_share(&self, session_id: &str) -> Result<ShareLink, ApiError> {
        if !is_id(session_id) {
            return Err(ApiError::BadReply);
        }
        let resp = self
            .send(
                self.http
                    .post(self.url(&format!("/api/logger/sessions/{session_id}/share"))),
            )
            .await?;
        let link: ShareLink = resp.json().await.map_err(|_| ApiError::BadReply)?;
        // The page is the server's; but only its one shape is kept.
        match share_path(&link.token) {
            Some(p) if p == link.path => Ok(link),
            _ => Err(ApiError::BadReply),
        }
    }

    /// Revokes the log's share link: it stops working at once. `404
    /// share_not_found` when the log has none.
    pub async fn revoke_share(&self, session_id: &str) -> Result<(), ApiError> {
        if !is_id(session_id) {
            return Err(ApiError::BadReply);
        }
        self.send(
            self.http
                .delete(self.url(&format!("/api/logger/sessions/{session_id}/share"))),
        )
        .await
        .map(|_| ())
    }

    /// Whether a share link still opens its log: `false` once it was
    /// revoked (here or on the site), or the log has no Public upload left
    /// (`404 share_not_found`). Reads the shared log with no sign-in, as
    /// anyone with the link would.
    pub async fn share_works(&self, token: &str) -> Result<bool, ApiError> {
        if !is_share_token(token) {
            return Err(ApiError::BadReply);
        }
        let rb = self.http.get(self.url(&format!("/api/shared/{token}")));
        // Not `send`: the link is read as a stranger would, with no token.
        let resp = rb.send().await.map_err(|e| {
            log::info!("request failed: {}", describe(&e));
            ApiError::Offline
        })?;
        match resp.status().as_u16() {
            200..=299 => Ok(true),
            404 => {
                let code = resp
                    .json::<serde_json::Value>()
                    .await
                    .ok()
                    .and_then(|v| v.get("code").and_then(|c| c.as_str().map(str::to_string)));
                if code.as_deref() == Some("share_not_found") {
                    Ok(false)
                } else {
                    Err(ApiError::Refused { status: 404, code })
                }
            }
            status @ (429 | 500..=599) => Err(ApiError::Busy {
                status,
                retry_after_s: None,
            }),
            status => Err(ApiError::Refused { status, code: None }),
        }
    }
}

/// A server id as the API gives it: digits only, and short enough for a
/// 64-bit number. Nothing else is put in a request's path.
fn is_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 19 && s.bytes().all(|b| b.is_ascii_digit())
}

/// Takes only the character and guild from the server's `main`; anything
/// else in it is dropped.
fn main_from(v: &serde_json::Value) -> Main {
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
    let guild = match v.get("guild") {
        Some(serde_json::Value::String(g)) => Some(g.clone()),
        Some(g) => g.get("name").and_then(|n| n.as_str()).map(str::to_string),
        None => None,
    };
    Main {
        name: s("name"),
        realm: s("realm").or_else(|| s("realm_name")),
        region: s("region"),
        guild,
    }
}

/// A connection error in words, without the URL (it may carry a code).
fn describe(e: &reqwest::Error) -> &'static str {
    if e.is_timeout() {
        "timed out"
    } else if e.is_connect() {
        "couldn't connect"
    } else {
        "network error"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn main_keeps_only_character_and_guild() {
        let v = serde_json::json!({"name": "Player1", "realm": "Tarren Mill", "region": "eu",
            "guild": {"name": "Guild One", "id": 7}, "battletag": "never#1234"});
        let m = main_from(&v);
        assert_eq!(m.name.as_deref(), Some("Player1"));
        assert_eq!(m.guild.as_deref(), Some("Guild One"));
        let json = serde_json::to_string(&m).unwrap();
        assert!(!json.contains("never"));
    }

    #[test]
    fn created_reads_the_servers_numeric_id_and_chunk_size() {
        let c: Created = serde_json::from_value(serde_json::json!({
            "id": 41, "status": "receiving", "received": [0, 2], "chunk_size": 4194304,
            "chunk_count": 3, "historical": false
        }))
        .unwrap();
        assert_eq!(c.id, "41");
        assert_eq!(c.chunk_size, Some(4 << 20));
        assert!(!c.is_complete());
        let done: Created =
            serde_json::from_value(serde_json::json!({"id": 41, "status": "parsed"})).unwrap();
        assert!(done.is_complete());
    }

    #[test]
    fn upload_rows_read_the_fights_and_page_links() {
        let rows: Vec<UploadRow> = serde_json::from_value(serde_json::json!([
            {"id": 41, "status": "parsed", "url": "/logs/41/", "fights": [
                {"id": 7, "kind": "key", "name": "The Blinding Vale", "difficulty": null,
                 "key_level": 14, "kill": true, "duration_ms": 1681247, "parent_id": null,
                 "url": "/logs/41/pulls/7/", "affixes": [9, 10, 147]},
                {"id": 8, "kind": "encounter", "name": "Lightblossom Trinity", "difficulty": 8,
                 "kill": true, "duration_ms": 90000, "parent_id": 7, "url": "/logs/41/pulls/8/"}
            ]},
            {"id": 42, "status": "queued", "url": null},
            {"id": 43, "status": "receiving"}
        ]))
        .unwrap();
        assert_eq!(rows[0].url.as_deref(), Some("/logs/41/"));
        assert_eq!(rows[0].fights.len(), 2);
        assert_eq!(rows[0].fights[0].id, "7");
        assert_eq!(rows[0].fights[0].key_level, Some(14));
        assert_eq!(rows[0].fights[0].parent_id, None);
        assert_eq!(rows[0].fights[1].parent_id.as_deref(), Some("7"));
        assert_eq!(rows[0].fights[1].kill, Some(true));
        assert!(!rows[0].is_processing());
        assert!(rows[1].fights.is_empty() && rows[1].is_processing());
        assert!(rows[2].is_processing());
        // A server from before sessions: `url` is the log's page.
        assert_eq!(rows[0].log_page(), Some("/logs/41/"));
    }

    #[test]
    fn upload_rows_read_their_session_and_its_page_links() {
        let row: UploadRow = serde_json::from_value(serde_json::json!(
            {"id": 44, "status": "parsed", "session_id": 12, "log_url": "/logs/12/",
             "url": "/logs/12/", "fights": [
                {"id": 9, "kind": "encounter", "encounter_id": 3129, "name": "Plexus Sentinel",
                 "difficulty": 16, "kill": false, "section": "raid", "in_key": null,
                 "url": "/logs/12/pulls/9/", "boss_url": "/logs/12/bosses/3129/"},
                {"id": 10, "kind": "encounter", "encounter_id": 3202, "difficulty": 8,
                 "section": "mplus", "in_key": 7, "url": "/logs/12/pulls/10/", "boss_url": null}
            ]}
        ))
        .unwrap();
        assert_eq!(row.session_id.as_deref(), Some("12"));
        assert_eq!(row.log_page(), Some("/logs/12/"));
        let (raid, boss) = (&row.fights[0], &row.fights[1]);
        assert_eq!(raid.section.as_deref(), Some("raid"));
        assert_eq!(raid.encounter_id, Some(3129));
        assert_eq!(raid.boss_url.as_deref(), Some("/logs/12/bosses/3129/"));
        assert_eq!(boss.in_key.as_deref(), Some("7"));
        assert_eq!(boss.boss_url, None);
    }

    #[test]
    fn a_session_reads_its_raid_bosses_and_keys() {
        let s: LogSession = serde_json::from_value(serde_json::json!({
            "id": 12, "log_url": "/logs/12/", "file_name": "WoWCombatLog-092826_200101.txt",
            "raid": [{"encounter_id": 3129, "name": "Plexus Sentinel",
                      "boss_url": "/logs/12/bosses/3129/", "killed": true,
                      "pulls": [{"id": 9, "kill": false, "url": "/logs/12/pulls/9/"},
                                {"id": 11, "kill": true, "url": "/logs/12/pulls/11/"}]}],
            "mplus": [{"id": 7, "kind": "key", "key_level": 14, "url": "/logs/12/keys/7/",
                       "bosses": [{"id": 10, "in_key": 7, "url": "/logs/12/pulls/10/"}]}],
            "dungeon": []
        }))
        .unwrap();
        assert_eq!(s.id, "12");
        assert!(s.raid[0].killed);
        assert_eq!(s.raid[0].pulls.len(), 2);
        assert_eq!(s.mplus[0].key.url.as_deref(), Some("/logs/12/keys/7/"));
        assert_eq!(s.mplus[0].bosses[0].in_key.as_deref(), Some("7"));
    }

    #[tokio::test]
    async fn only_a_numeric_upload_id_goes_in_the_path() {
        // Nothing listens on this port: a request that's sent comes back
        // Offline, one refused before sending comes back BadReply.
        let dead = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", dead.local_addr().unwrap());
        drop(dead);
        let api = Api::new(&origin).with_token(Some("t".into()));
        for bad in [
            "",
            "u1",
            "../tokens",
            "41/chunks/0",
            "41?x=1",
            "41#",
            "%34%31",
            " 41",
            "12345678901234567890",
        ] {
            assert!(
                matches!(
                    api.set_visibility(bad, Visibility::Private).await,
                    Err(ApiError::BadReply)
                ),
                "{bad:?}"
            );
            assert!(
                matches!(api.delete_upload(bad).await, Err(ApiError::BadReply)),
                "{bad:?}"
            );
        }
        // A real id is sent (and finds nobody there).
        assert!(matches!(
            api.set_visibility("41", Visibility::Private).await,
            Err(ApiError::Offline)
        ));
        assert!(matches!(
            api.delete_upload("41").await,
            Err(ApiError::Offline)
        ));
    }

    #[test]
    fn share_tokens_and_paths_have_one_shape() {
        let t = "AbCdEfGhIjKlMnOpQr_-12";
        assert!(is_share_token(t));
        assert_eq!(
            share_path(t).as_deref(),
            Some("/shared/AbCdEfGhIjKlMnOpQr_-12/")
        );
        for bad in [
            "",
            "short",
            "AbCdEfGhIjKlMnOpQr_-123",
            "AbCdEfGhIjKlMnOpQr/-12",
            "AbCdEfGhIjKlMnOpQr.-12",
            "../../../account/logs/",
        ] {
            assert!(!is_share_token(bad), "{bad:?}");
            assert_eq!(share_path(bad), None);
        }
        let link: ShareLink = serde_json::from_value(serde_json::json!({
            "id": 3, "token": t, "path": "/shared/AbCdEfGhIjKlMnOpQr_-12/",
            "createdAt": "2026-10-02T20:10:00+00:00"
        }))
        .unwrap();
        assert_eq!(
            link.created_at.as_deref(),
            Some("2026-10-02T20:10:00+00:00")
        );
    }

    #[tokio::test]
    async fn only_a_numeric_log_id_goes_in_a_share_path() {
        let dead = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", dead.local_addr().unwrap());
        drop(dead);
        let api = Api::new(&origin).with_token(Some("t".into()));
        for bad in ["", "../uploads", "12/share", "12?x"] {
            assert!(matches!(
                api.create_share(bad).await,
                Err(ApiError::BadReply)
            ));
            assert!(matches!(
                api.revoke_share(bad).await,
                Err(ApiError::BadReply)
            ));
        }
        assert!(matches!(
            api.share_works("../x").await,
            Err(ApiError::BadReply)
        ));
        assert!(matches!(
            api.create_share("12").await,
            Err(ApiError::Offline)
        ));
    }

    #[test]
    fn upload_body_leaves_out_empty_ids() {
        let u = NewUpload {
            sha256: "ab".into(),
            size: 10,
            chunk_size: 8,
            kind: "key".into(),
            encounter_id: None,
            difficulty: None,
            key_level: Some(14),
            map_id: Some(584),
            start_time: "t0".into(),
            end_time: "t1".into(),
            region: "eu".into(),
            visibility: Visibility::Guild,
            client_version: "0.1.0".into(),
            session_key: None,
            file_name: None,
            file_start_time: None,
        };
        let v = serde_json::to_value(&u).unwrap();
        assert!(v.get("encounter_id").is_none());
        assert!(v.get("session_key").is_none() && v.get("file_name").is_none());
        assert_eq!(v["visibility"], "guild");
        assert_eq!(v["key_level"], 14);
    }
}
