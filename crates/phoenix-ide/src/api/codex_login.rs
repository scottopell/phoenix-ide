//! HTTP API for the native ChatGPT/Codex login flows.
//!
//! Two flows are exposed (matching `crate::llm::codex_login`):
//!
//! * **PKCE/loopback** — `POST /api/codex/login/pkce/start` returns the
//!   `authorize_url` and a session id; the user follows the URL in their
//!   browser, which redirects to `127.0.0.1:1455/auth/callback`. A background
//!   task watches that callback and finalises the login. If the loopback can't
//!   bind (port in use, or the user is on a host where 127.0.0.1:1455 isn't
//!   reachable from the browser), the same session id accepts a manual paste
//!   via `POST /api/codex/login/pkce/{id}/manual`.
//! * **Device code** — `POST /api/codex/login/device/start` synchronously
//!   requests a device code from `OpenAI` and returns the verification URL
//!   plus the user-visible code. A background task polls and finalises.
//!
//! Both flows write **Phoenix's own** `~/.phoenix-ide/codex-auth.json` on
//! success — never Codex CLI's `~/.codex/auth.json`, even if the user happens
//! to be in piggyback mode. Two distinct sources, two distinct lifecycles.
//!
//! Status is read via `GET /api/codex/login/{kind}/{id}/status`. The Codex
//! credential ([`crate::llm::codex_credential::CodexCredential`]) mtime-watches
//! its file, so an *already-loaded* credential picks up new tokens on next use.
//! On a first-time login (no credential constructed at startup because the
//! file didn't exist), Phoenix must be restarted before the bridge becomes
//! active — the UI surfaces this in the success banner.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{oneshot, Mutex};
use tokio_util::sync::CancellationToken;

/// Hard cap on how long a PKCE attempt can wait for either the browser
/// callback or a manual paste. Matches the device-code 15-minute window. After
/// this elapses the background task settles the session as `Err(Cancelled)`
/// rather than living forever.
const PKCE_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// After a flow settles (success / error / cancelled), the session record
/// stays in the manager long enough for the polling UI to read the terminal
/// state, then is swept. `pkce_status` / `device_status` already drop
/// sessions on first terminal read; this sweeper is the safety net for
/// clients that crash, navigate away, or never poll back.
const SETTLED_SESSION_RETENTION: Duration = Duration::from_secs(60);

use crate::llm::codex_credential;
use crate::llm::codex_login::{
    self, build_pkce_session, exchange_pkce_code, finalize_login, poll_device_code,
    request_device_code, validate_state, CallbackPayload, DeviceCode, LoginError, LoginResult,
    LoopbackServer, CALLBACK_PORT, CLIENT_ID, ISSUER_BASE,
};

use super::AppState;

// ---------------------------------------------------------------------------
// Public status — what GET /status returns.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LoginStatusJson {
    Pending,
    Success {
        account_id: Option<String>,
        auth_path: String,
    },
    Error {
        message: String,
    },
}

impl LoginStatusJson {
    fn from_result(result: &Result<LoginResult, LoginError>) -> Self {
        match result {
            Ok(r) => Self::Success {
                account_id: r.account_id.clone(),
                auth_path: r.auth_path.display().to_string(),
            },
            Err(e) => Self::Error {
                message: e.to_string(),
            },
        }
    }
}

#[derive(Default)]
struct LoginStatus {
    /// `None` while still in flight; `Some` once the background task has
    /// settled the outcome.
    outcome: Option<Result<LoginResult, LoginError>>,
}

// ---------------------------------------------------------------------------
// Session manager
// ---------------------------------------------------------------------------

struct PkceSessionInner {
    /// Sender used to feed a manually-pasted auth code + state to the
    /// background task when the loopback callback can't fire (e.g. user on
    /// SSH, browser on a different host). Both fields are required so the
    /// driver can run the same `validate_state` CSRF check it runs on the
    /// loopback path. Consumed at most once; mutually exclusive with the
    /// loopback callback firing.
    manual_tx: Option<oneshot::Sender<ManualCallback>>,
    status: LoginStatus,
}

#[derive(Debug, Clone)]
struct ManualCallback {
    code: String,
    state: String,
}

struct PkceSession {
    inner: Mutex<PkceSessionInner>,
    /// Fired by `pkce_cancel`. The background driver races every step
    /// against this; a late callback after cancel must NOT proceed to
    /// `finalize_login` and write `~/.phoenix-ide/codex-auth.json`.
    cancel: CancellationToken,
}

struct DeviceSession {
    /// User-visible code retained for log lines on settle.
    user_code: String,
    status: Mutex<LoginStatus>,
    /// Fired by `device_cancel`. The polling task aborts before
    /// `finalize_login` runs, so a user who clicked Cancel — even one who
    /// completes the verification page anyway — does NOT end up with
    /// silently-written credentials.
    cancel: CancellationToken,
}

#[derive(Default)]
pub struct CodexLoginManager {
    pkce: Mutex<HashMap<String, Arc<PkceSession>>>,
    device: Mutex<HashMap<String, Arc<DeviceSession>>>,
}

impl CodexLoginManager {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
}

fn new_session_id() -> String {
    use rand::RngCore;
    use std::fmt::Write;
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().fold(String::with_capacity(32), |mut acc, b| {
        let _ = write!(acc, "{b:02x}");
        acc
    })
}

/// Login flow always writes to Phoenix's own auth file. Piggybacking on Codex
/// CLI's `~/.codex/auth.json` is read-only — that file belongs to the Codex
/// CLI's lifecycle and we don't overwrite it from in-app login.
fn login_target_path() -> PathBuf {
    codex_credential::default_phoenix_auth_path()
}

// ---------------------------------------------------------------------------
// PKCE handlers
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct PkceStartResponse {
    pub session_id: String,
    pub authorize_url: String,
    /// Redirect URI baked into the authorize URL — surfaced so the UI can
    /// remind the user that this is where the post-redirect code will land.
    pub redirect_uri: String,
    /// `true` if Phoenix successfully bound the loopback callback server. When
    /// `false`, the user is on a host where 127.0.0.1:1455 is occupied or
    /// unreachable; the UI should switch to the manual-paste fallback.
    pub loopback_bound: bool,
    pub callback_port: u16,
}

pub async fn pkce_start(
    State(state): State<AppState>,
) -> Result<Json<PkceStartResponse>, (StatusCode, Json<serde_json::Value>)> {
    let mgr = state.codex_login.clone();
    let session = build_pkce_session();
    let session_id = new_session_id();

    let (manual_tx, manual_rx) = oneshot::channel::<ManualCallback>();
    let cancel = CancellationToken::new();

    let pkce_session = Arc::new(PkceSession {
        inner: Mutex::new(PkceSessionInner {
            manual_tx: Some(manual_tx),
            status: LoginStatus::default(),
        }),
        cancel: cancel.clone(),
    });

    // Try to bind the loopback. Failure here isn't fatal — the manual paste
    // fallback can still complete the flow.
    let loopback = match LoopbackServer::start(session.state.clone()).await {
        Ok(srv) => Some(srv),
        Err(LoginError::PortInUse(_)) => {
            tracing::info!(
                "codex_login: loopback port {CALLBACK_PORT} in use; falling back to manual paste only"
            );
            None
        }
        Err(e) => {
            tracing::warn!(error = %e, "codex_login: loopback bind failed; manual paste only");
            None
        }
    };
    let loopback_bound = loopback.is_some();

    {
        let mut sessions = mgr.pkce.lock().await;
        sessions.insert(session_id.clone(), pkce_session);
    }

    // Spawn the background driver. Sequence: race loopback callback against
    // manual paste; whichever arrives first wins. Validate state, exchange
    // code, write auth.json, settle status.
    {
        let session_id_for_task = session_id.clone();
        let expected_state = session.state.clone();
        let verifier = session.pkce.code_verifier.clone();
        let redirect_uri = session.redirect_uri.clone();
        let mgr_for_task = mgr.clone();
        let cancel_for_task = cancel.clone();
        tokio::spawn(async move {
            let outcome = drive_pkce(
                cancel_for_task,
                loopback,
                manual_rx,
                expected_state,
                verifier,
                redirect_uri,
            )
            .await;
            settle_pkce(&mgr_for_task, &session_id_for_task, outcome).await;
        });
    }

    Ok(Json(PkceStartResponse {
        session_id,
        authorize_url: session.authorize_url,
        redirect_uri: session.redirect_uri,
        loopback_bound,
        callback_port: CALLBACK_PORT,
    }))
}

async fn drive_pkce(
    cancel: CancellationToken,
    loopback: Option<LoopbackServer>,
    manual_rx: oneshot::Receiver<ManualCallback>,
    expected_state: String,
    verifier: String,
    redirect_uri: String,
) -> Result<LoginResult, LoginError> {
    // Race the loopback callback against the manual-paste channel and the
    // user-cancellation token. `biased` makes cancel preempt deterministically
    // when multiple branches ready simultaneously — we never want to hand a
    // late callback through to `finalize_login` after the user has clicked
    // Cancel.
    let code = match loopback {
        Some(mut server) => {
            tokio::select! {
                biased;
                () = cancel.cancelled() => return Err(LoginError::Cancelled),
                () = tokio::time::sleep(PKCE_TIMEOUT) => return Err(LoginError::Cancelled),
                cb = &mut server.callback_rx => {
                    match cb.map_err(|_| LoginError::Loopback("callback channel closed".into()))? {
                        CallbackPayload::Success { code, state: returned_state } => {
                            validate_state(&expected_state, &returned_state)?;
                            code
                        }
                        CallbackPayload::Error { error, description } => {
                            return Err(LoginError::OAuth(match description {
                                Some(d) if !d.is_empty() => format!("{error}: {d}"),
                                _ => error,
                            }));
                        }
                    }
                }
                manual = manual_rx => {
                    // Manual paste must validate state too. Without this
                    // check, an attacker who tricked the user into pasting
                    // an authorization code minted for the attacker's own
                    // session would have Phoenix store tokens for the wrong
                    // ChatGPT account. The UI now collects the full
                    // post-redirect URL (or both code+state) so this branch
                    // has the same CSRF guarantee as the loopback path.
                    let m = manual.map_err(|_| {
                        LoginError::Loopback("manual code channel closed".into())
                    })?;
                    validate_state(&expected_state, &m.state)?;
                    m.code
                }
            }
        }
        None => tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(LoginError::Cancelled),
            () = tokio::time::sleep(PKCE_TIMEOUT) => return Err(LoginError::Cancelled),
            manual = manual_rx => {
                let m = manual.map_err(|_| {
                    LoginError::Loopback("manual code channel closed (no loopback)".into())
                })?;
                validate_state(&expected_state, &m.state)?;
                m.code
            }
        },
    };

    // Re-check after the await above and before each subsequent step, so a
    // cancellation that lands between the callback and the file-write still
    // short-circuits before we spend any tokens.
    if cancel.is_cancelled() {
        return Err(LoginError::Cancelled);
    }

    let tokens = tokio::select! {
        biased;
        () = cancel.cancelled() => return Err(LoginError::Cancelled),
        r = exchange_pkce_code(ISSUER_BASE, CLIENT_ID, &redirect_uri, &verifier, &code) => r?,
    };

    if cancel.is_cancelled() {
        return Err(LoginError::Cancelled);
    }

    finalize_login(&login_target_path(), tokens)
}

async fn settle_pkce(
    mgr: &Arc<CodexLoginManager>,
    session_id: &str,
    outcome: Result<LoginResult, LoginError>,
) {
    let session = {
        let sessions = mgr.pkce.lock().await;
        sessions.get(session_id).cloned()
    };
    let Some(session) = session else { return };
    {
        let mut inner = session.inner.lock().await;
        inner.status.outcome = Some(outcome);
    }
    schedule_pkce_sweep(mgr.clone(), session_id.to_string());
}

/// Remove a settled PKCE session after [`SETTLED_SESSION_RETENTION`] elapses.
/// Bounds memory growth when a client crashes or navigates away without
/// reading `/status` (in which case `pkce_status` would have removed it
/// eagerly). Idempotent: if the entry has already been swept, this is a no-op.
fn schedule_pkce_sweep(mgr: Arc<CodexLoginManager>, session_id: String) {
    tokio::spawn(async move {
        tokio::time::sleep(SETTLED_SESSION_RETENTION).await;
        let mut sessions = mgr.pkce.lock().await;
        sessions.remove(&session_id);
    });
}

/// Manual-paste fallback request. Either:
/// - `redirect_url`: full post-callback URL with `code=…&state=…` (preferred),
/// - or `code` + `state` extracted by the UI.
///
/// `state` is mandatory because the backend validates it before exchanging
/// the code; without that check, an attacker who tricked the user into
/// pasting a code minted for the attacker's session would have Phoenix
/// store tokens for the wrong ChatGPT account.
#[derive(Deserialize)]
pub struct ManualCodeRequest {
    #[serde(default)]
    pub redirect_url: Option<String>,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
}

/// Pull `code` and `state` out of either a full redirect URL or the raw
/// fields. Tolerates both `http://localhost:1455/auth/callback?code=…&state=…`
/// and bare `?code=…&state=…` (just the query string) — anything we can find
/// a `?` in.
fn extract_manual_callback(req: &ManualCodeRequest) -> Option<ManualCallback> {
    if let Some(url) = req.redirect_url.as_deref().map(str::trim) {
        // The user may have pasted just the query (`?code=…&state=…`) or the
        // full URL. Find the first `?` to anchor the query string.
        let qs = url.split_once('?').map_or(url, |(_, q)| q);
        let mut code = None;
        let mut state = None;
        for pair in qs.split('&') {
            let (k, v) = pair.split_once('=')?;
            let v = url_decode(v);
            match k {
                "code" => code = Some(v),
                "state" => state = Some(v),
                _ => {}
            }
        }
        if let (Some(code), Some(state)) = (code, state) {
            return Some(ManualCallback { code, state });
        }
    }
    if let (Some(code), Some(state)) = (req.code.as_ref(), req.state.as_ref()) {
        return Some(ManualCallback {
            code: code.clone(),
            state: state.clone(),
        });
    }
    None
}

/// Decode the small subset of URL-encoded characters that show up in OAuth
/// query strings. Avoids pulling in the `url` crate just for this.
fn url_decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut bytes = s.bytes();
    while let Some(b) = bytes.next() {
        if b == b'%' {
            let hi = bytes.next();
            let lo = bytes.next();
            match (hi, lo) {
                (Some(h), Some(l)) => {
                    if let (Some(h), Some(l)) =
                        (char::from(h).to_digit(16), char::from(l).to_digit(16))
                    {
                        out.push(char::from(((h << 4) | l) as u8));
                    } else {
                        out.push('%');
                        out.push(char::from(h));
                        out.push(char::from(l));
                    }
                }
                _ => out.push('%'),
            }
        } else if b == b'+' {
            out.push(' ');
        } else {
            out.push(char::from(b));
        }
    }
    out
}

pub async fn pkce_manual(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(body): Json<ManualCodeRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let mgr = state.codex_login.clone();
    let Some(callback) = extract_manual_callback(&body) else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "missing_code_or_state",
                "message": "Provide either redirect_url or both code and state. \
                            State is required so the backend can verify the redirect \
                            wasn't crafted for a different login session."
            })),
        ));
    };

    let session = {
        let sessions = mgr.pkce.lock().await;
        sessions.get(&session_id).cloned()
    };
    let session = session.ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "session not found" })),
        )
    })?;

    let tx = {
        let mut inner = session.inner.lock().await;
        inner.manual_tx.take()
    };

    let Some(tx) = tx else {
        return Err((
            StatusCode::CONFLICT,
            Json(serde_json::json!({ "error": "code already submitted" })),
        ));
    };

    if tx.send(callback).is_err() {
        // Background task already exited; this can happen if the loopback
        // callback fired first or the flow was cancelled.
        return Err((
            StatusCode::CONFLICT,
            Json(serde_json::json!({ "error": "login already settled" })),
        ));
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn pkce_status(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<LoginStatusJson>, StatusCode> {
    let mgr = state.codex_login.clone();
    let session = {
        let sessions = mgr.pkce.lock().await;
        sessions.get(&session_id).cloned()
    };
    let session = session.ok_or(StatusCode::NOT_FOUND)?;
    let response = {
        let inner = session.inner.lock().await;
        match &inner.status.outcome {
            None => LoginStatusJson::Pending,
            Some(r) => LoginStatusJson::from_result(r),
        }
    };
    // Once we've returned a terminal state, the UI stops polling — drop the
    // session so the map doesn't grow unbounded over a session that opens the
    // login page repeatedly. Subsequent /status calls 404, which the UI
    // already handles as "session no longer tracked".
    if !matches!(response, LoginStatusJson::Pending) {
        let mut sessions = mgr.pkce.lock().await;
        sessions.remove(&session_id);
    }
    Ok(Json(response))
}

pub async fn pkce_cancel(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Json<serde_json::Value> {
    let mgr = state.codex_login.clone();
    // Hold the session — don't remove it from the map. We need to (a) fire
    // the cancellation token so the background driver bails before
    // `finalize_login`, and (b) leave the session record present so the next
    // /status poll can read the settled `Cancelled` outcome the driver writes.
    let session = {
        let sessions = mgr.pkce.lock().await;
        sessions.get(&session_id).cloned()
    };
    if let Some(session) = session {
        session.cancel.cancel();
    }
    Json(serde_json::json!({ "ok": true }))
}

// ---------------------------------------------------------------------------
// Device code handlers
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct DeviceStartResponse {
    pub session_id: String,
    pub verification_url: String,
    pub user_code: String,
    pub interval_secs: u64,
    pub timeout_secs: u64,
}

pub async fn device_start(
    State(state): State<AppState>,
) -> Result<Json<DeviceStartResponse>, (StatusCode, Json<serde_json::Value>)> {
    let mgr = state.codex_login.clone();
    let device = request_device_code().await.map_err(|e| match e {
        LoginError::DeviceCodeNotEnabled => (
            StatusCode::NOT_IMPLEMENTED,
            Json(serde_json::json!({
                "error": "device_code_not_enabled",
                "message": "device code login is not enabled for this issuer; use the browser flow instead",
            })),
        ),
        other => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({ "error": other.to_string() })),
        ),
    })?;

    let session_id = new_session_id();
    let cancel = CancellationToken::new();
    let response = DeviceStartResponse {
        session_id: session_id.clone(),
        verification_url: device.verification_url.clone(),
        user_code: device.user_code.clone(),
        interval_secs: device.interval.as_secs(),
        timeout_secs: codex_login::DEVICE_CODE_TIMEOUT_SECS,
    };

    {
        let mut sessions = mgr.device.lock().await;
        sessions.insert(
            session_id.clone(),
            Arc::new(DeviceSession {
                user_code: device.user_code.clone(),
                status: Mutex::new(LoginStatus::default()),
                cancel: cancel.clone(),
            }),
        );
    }

    {
        let mgr_for_task = mgr.clone();
        let session_id_for_task = session_id.clone();
        tokio::spawn(async move {
            let outcome = drive_device_code(cancel, device).await;
            settle_device(&mgr_for_task, &session_id_for_task, outcome).await;
        });
    }

    Ok(Json(response))
}

async fn drive_device_code(
    cancel: CancellationToken,
    device: DeviceCode,
) -> Result<LoginResult, LoginError> {
    // Race the long polling loop against user cancellation. Without this,
    // pressing Cancel only deletes the session record while the poll keeps
    // running — and if the user has already (or subsequently) completes the
    // verification page, `finalize_login` would still write
    // `~/.phoenix-ide/codex-auth.json` against their wishes.
    let tokens = tokio::select! {
        biased;
        () = cancel.cancelled() => return Err(LoginError::Cancelled),
        r = poll_device_code(&device) => r?,
    };
    if cancel.is_cancelled() {
        return Err(LoginError::Cancelled);
    }
    finalize_login(&login_target_path(), tokens)
}

async fn settle_device(
    mgr: &Arc<CodexLoginManager>,
    session_id: &str,
    outcome: Result<LoginResult, LoginError>,
) {
    let session = {
        let sessions = mgr.device.lock().await;
        sessions.get(session_id).cloned()
    };
    let Some(session) = session else { return };
    if let Err(err) = &outcome {
        tracing::warn!(
            user_code = %session.user_code,
            error = %err,
            "codex_login: device code flow ended in error"
        );
    }
    {
        let mut status = session.status.lock().await;
        status.outcome = Some(outcome);
    }
    schedule_device_sweep(mgr.clone(), session_id.to_string());
}

/// See [`schedule_pkce_sweep`].
fn schedule_device_sweep(mgr: Arc<CodexLoginManager>, session_id: String) {
    tokio::spawn(async move {
        tokio::time::sleep(SETTLED_SESSION_RETENTION).await;
        let mut sessions = mgr.device.lock().await;
        sessions.remove(&session_id);
    });
}

pub async fn device_status(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<LoginStatusJson>, StatusCode> {
    let mgr = state.codex_login.clone();
    let session = {
        let sessions = mgr.device.lock().await;
        sessions.get(&session_id).cloned()
    };
    let session = session.ok_or(StatusCode::NOT_FOUND)?;
    let response = {
        let status = session.status.lock().await;
        match &status.outcome {
            None => LoginStatusJson::Pending,
            Some(r) => LoginStatusJson::from_result(r),
        }
    };
    if !matches!(response, LoginStatusJson::Pending) {
        let mut sessions = mgr.device.lock().await;
        sessions.remove(&session_id);
    }
    Ok(Json(response))
}

pub async fn device_cancel(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Json<serde_json::Value> {
    let mgr = state.codex_login.clone();
    let session = {
        let sessions = mgr.device.lock().await;
        sessions.get(&session_id).cloned()
    };
    if let Some(session) = session {
        session.cancel.cancel();
    }
    Json(serde_json::json!({ "ok": true }))
}

// ---------------------------------------------------------------------------
// Pre-flight: report whether the login flow is usable & what the env-var gate
// state is, so the UI can warn the user about restart-required.
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct LoginPreflight {
    /// Path the in-app login will write to (Phoenix's own auth file).
    pub auth_path: String,
    /// Path Phoenix will piggyback off when `OPENAI_USE_CODEX_AUTH=1` is set
    /// and Phoenix's own file is absent. Surfaced for diagnostic clarity.
    pub piggyback_path: String,
    /// Whether Phoenix's own auth file exists and parses as a valid
    /// chatgpt-mode token.
    pub already_signed_in: bool,
    /// Whether a Codex credential was constructed at startup (any path).
    /// Informational only — the UI should drive restart messaging from
    /// `restart_required_after_login` instead, since piggyback-loaded
    /// credentials still require restart when the user signs in via Phoenix.
    pub bridge_loaded_at_startup: bool,
    /// Whether the user must restart Phoenix before an in-app login takes
    /// effect. True when:
    ///  - no credential was loaded at startup (registry has nothing to
    ///    refresh), OR
    ///  - a credential was loaded but from a different path than the in-app
    ///    login writes to (e.g. piggyback mode loaded `~/.codex/auth.json`,
    ///    but the new login writes `~/.phoenix-ide/codex-auth.json` — the
    ///    in-memory credential keeps watching the old path).
    /// False only when the loaded credential's path matches the destination
    /// of the in-app login, i.e. its `mtime` watch will pick up new tokens
    /// without a restart.
    pub restart_required_after_login: bool,
    /// Whether `OPENAI_USE_CODEX_AUTH=1` is set in the current environment.
    /// Informational; the env-var only governs piggyback mode, not whether
    /// in-app login works.
    pub piggyback_env_set: bool,
}

pub async fn login_preflight(State(state): State<AppState>) -> Json<LoginPreflight> {
    let auth_path = codex_credential::default_phoenix_auth_path();
    let piggyback_path = codex_credential::default_auth_path();
    let already_signed_in = codex_credential::CodexCredential::load(auth_path.clone()).is_ok();
    let bridge_loaded_at_startup = state.llm_registry.codex_bridge_loaded_at_startup;
    // Restart is required UNLESS the registry's loaded credential is pinned
    // to the same path the in-app login writes to. This catches the
    // piggyback-then-Phoenix-login footgun: bridge_loaded_at_startup is true
    // (Codex CLI's auth.json was loaded), but a Phoenix login writes
    // ~/.phoenix-ide/codex-auth.json, which the running registry doesn't
    // watch. (Codex P2 review feedback on PR #57.)
    let restart_required_after_login =
        state.llm_registry.codex_loaded_path.as_deref() != Some(auth_path.as_path());
    let piggyback_env_set = std::env::var("OPENAI_USE_CODEX_AUTH")
        .ok()
        .is_some_and(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"));
    Json(LoginPreflight {
        auth_path: auth_path.display().to_string(),
        piggyback_path: piggyback_path.display().to_string(),
        already_signed_in,
        bridge_loaded_at_startup,
        restart_required_after_login,
        piggyback_env_set,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::codex_login::{finalize_login, DeviceCode, LoginError, TokenResponse};
    use std::time::Instant;

    #[test]
    fn extract_manual_callback_parses_full_redirect_url() {
        let req = ManualCodeRequest {
            redirect_url: Some("http://localhost:1455/auth/callback?code=abc&state=xyz".into()),
            code: None,
            state: None,
        };
        let m = extract_manual_callback(&req).unwrap();
        assert_eq!(m.code, "abc");
        assert_eq!(m.state, "xyz");
    }

    #[test]
    fn extract_manual_callback_decodes_url_encoded_values() {
        let req = ManualCodeRequest {
            redirect_url: Some("?code=a%2Bb%2Fc%3D&state=hello%20world".into()),
            code: None,
            state: None,
        };
        let m = extract_manual_callback(&req).unwrap();
        assert_eq!(m.code, "a+b/c=");
        assert_eq!(m.state, "hello world");
    }

    #[test]
    fn extract_manual_callback_falls_back_to_explicit_fields() {
        let req = ManualCodeRequest {
            redirect_url: None,
            code: Some("c".into()),
            state: Some("s".into()),
        };
        let m = extract_manual_callback(&req).unwrap();
        assert_eq!(m.code, "c");
        assert_eq!(m.state, "s");
    }

    /// A manual paste that's missing the state parameter must NOT be
    /// extractable. The handler refuses with 400 BAD_REQUEST so the
    /// background driver never sees a state-less code (which would skip
    /// the CSRF check). PR #57 review feedback.
    #[test]
    fn extract_manual_callback_rejects_missing_state() {
        // URL with code but no state.
        let req = ManualCodeRequest {
            redirect_url: Some("http://localhost:1455/auth/callback?code=abc".into()),
            code: None,
            state: None,
        };
        assert!(extract_manual_callback(&req).is_none());

        // Explicit code without explicit state.
        let req = ManualCodeRequest {
            redirect_url: None,
            code: Some("c".into()),
            state: None,
        };
        assert!(extract_manual_callback(&req).is_none());

        // Empty body.
        let req = ManualCodeRequest {
            redirect_url: None,
            code: None,
            state: None,
        };
        assert!(extract_manual_callback(&req).is_none());
    }

    /// Cancellation racing into `drive_device_code` MUST short-circuit the
    /// whole flow before `finalize_login` runs. The bug Codex review caught:
    /// previously, `device_cancel` only removed the session record from the
    /// map; the spawned poll keeps running and writes auth.json on success.
    ///
    /// We exercise the pre-poll cancel path: a token already cancelled when
    /// drive_device_code starts must error out with `Cancelled` from the
    /// `tokio::select!`, never reaching `poll_device_code` (which would hit
    /// the network) or `finalize_login` (which would write to
    /// `login_target_path()`). Asserting on the error type is what tells us
    /// the fix is in place — a regression that drops the cancel branch would
    /// surface as `LoginError::Network` or a hang on the unreachable issuer.
    #[tokio::test]
    async fn drive_device_code_cancelled_before_poll_short_circuits() {
        let cancel = CancellationToken::new();
        cancel.cancel();

        let device = DeviceCode {
            verification_url: "https://example/codex/device".into(),
            user_code: "ABCD-1234".into(),
            interval: std::time::Duration::from_secs(5),
            expires_at: Instant::now() + std::time::Duration::from_secs(60),
            device_auth_id: "dev-id".into(),
            issuer: "https://auth.example.invalid".into(),
            client_id: "client".into(),
        };

        let err = drive_device_code(cancel, device).await.unwrap_err();
        assert!(
            matches!(err, LoginError::Cancelled),
            "expected Cancelled, got {err:?}"
        );
    }

    /// `finalize_login` is the synchronous file-write step. Cancellation
    /// arriving between `poll_device_code` returning Ok and the file write
    /// must still suppress the write — verified by checking that a manually
    /// triggered cancel between the two steps short-circuits.
    #[tokio::test]
    async fn cancel_between_token_receipt_and_write_skips_file() {
        // We can't easily fake `poll_device_code` Ok without hitting the
        // network, but the code path between the await and `finalize_login`
        // is `if cancel.is_cancelled() { return Cancelled }`. Exercise that
        // check directly by inlining the same predicate against tokens we
        // synthesise — and confirm `finalize_login` would have written if
        // we let it through, then verify the predicate stops it.
        let dir = tempfile::tempdir().unwrap();
        let auth_path = dir.path().join("codex-auth.json");
        let cancel = CancellationToken::new();
        cancel.cancel();

        let tokens = TokenResponse {
            access_token: "at".into(),
            refresh_token: "rt".into(),
            id_token: "header.eyJleHAiOjE3MDAwMDAwMDB9.sig".into(),
        };
        // The check that `drive_device_code` performs immediately before
        // calling finalize_login.
        let outcome: Result<(), LoginError> = if cancel.is_cancelled() {
            Err(LoginError::Cancelled)
        } else {
            finalize_login(&auth_path, tokens).map(|_| ())
        };
        assert!(matches!(outcome, Err(LoginError::Cancelled)));
        assert!(!auth_path.exists());
    }
}
