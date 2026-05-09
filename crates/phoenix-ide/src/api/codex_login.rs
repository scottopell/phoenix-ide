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
use tokio::sync::{oneshot, Mutex};

use crate::llm::codex_credential;
use crate::llm::codex_login::{
    self, build_pkce_session, exchange_pkce_code, finalize_login, poll_device_code,
    request_device_code, CallbackPayload, DeviceCode, LoginError, LoginResult, LoopbackServer,
    CALLBACK_PORT, CLIENT_ID, ISSUER_BASE,
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
    /// Sender used to feed a manually-pasted auth code to the background task
    /// when the loopback callback can't fire (e.g. user on SSH, browser on a
    /// different host). Consumed at most once; mutually exclusive with the
    /// loopback callback firing.
    manual_tx: Option<oneshot::Sender<String>>,
    status: LoginStatus,
}

struct PkceSession {
    inner: Mutex<PkceSessionInner>,
}

struct DeviceSession {
    /// User-visible code retained for log lines on settle.
    user_code: String,
    status: Mutex<LoginStatus>,
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

    let (manual_tx, manual_rx) = oneshot::channel::<String>();

    let pkce_session = Arc::new(PkceSession {
        inner: Mutex::new(PkceSessionInner {
            manual_tx: Some(manual_tx),
            status: LoginStatus::default(),
        }),
    });

    // Try to bind the loopback. Failure here isn't fatal — the manual paste
    // fallback can still complete the flow.
    let loopback = match LoopbackServer::start().await {
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
        tokio::spawn(async move {
            let outcome =
                drive_pkce(loopback, manual_rx, expected_state, verifier, redirect_uri).await;
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
    loopback: Option<LoopbackServer>,
    manual_rx: oneshot::Receiver<String>,
    expected_state: String,
    verifier: String,
    redirect_uri: String,
) -> Result<LoginResult, LoginError> {
    // Race the loopback callback against the manual-paste channel. If we
    // didn't manage to bind the loopback, only the manual channel is in play.
    let code = match loopback {
        Some(mut server) => {
            tokio::select! {
                cb = &mut server.callback_rx => {
                    match cb.map_err(|_| LoginError::Loopback("callback channel closed".into()))? {
                        CallbackPayload::Success { code, state: returned_state } => {
                            if returned_state != expected_state {
                                return Err(LoginError::StateMismatch);
                            }
                            code
                        }
                        CallbackPayload::Error { error, description } => {
                            let detail = description.unwrap_or_default();
                            return Err(LoginError::OAuth(format!("{error}: {detail}")));
                        }
                    }
                }
                code = manual_rx => {
                    // Manual paste pre-empts the browser callback. We
                    // intentionally do NOT validate state here: the manual
                    // path doesn't carry a state value, the user pasted only
                    // the code. The CSRF concern that motivates state is
                    // about a malicious page sending a callback to our
                    // loopback; a hand-pasted code from the user's own
                    // browser bar isn't subject to that attack.
                    code.map_err(|_| LoginError::Loopback("manual code channel closed".into()))?
                }
            }
        }
        None => manual_rx
            .await
            .map_err(|_| LoginError::Loopback("manual code channel closed (no loopback)".into()))?,
    };

    let tokens =
        exchange_pkce_code(ISSUER_BASE, CLIENT_ID, &redirect_uri, &verifier, &code).await?;
    finalize_login(&login_target_path(), tokens)
}

async fn settle_pkce(
    mgr: &CodexLoginManager,
    session_id: &str,
    outcome: Result<LoginResult, LoginError>,
) {
    let session = {
        let sessions = mgr.pkce.lock().await;
        sessions.get(session_id).cloned()
    };
    let Some(session) = session else { return };
    let mut inner = session.inner.lock().await;
    inner.status.outcome = Some(outcome);
}

#[derive(Deserialize)]
pub struct ManualCodeRequest {
    pub code: String,
}

pub async fn pkce_manual(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(body): Json<ManualCodeRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let mgr = state.codex_login.clone();
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

    if tx.send(body.code).is_err() {
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
    let inner = session.inner.lock().await;
    Ok(Json(match &inner.status.outcome {
        None => LoginStatusJson::Pending,
        Some(r) => LoginStatusJson::from_result(r),
    }))
}

pub async fn pkce_cancel(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Json<serde_json::Value> {
    let mgr = state.codex_login.clone();
    let mut sessions = mgr.pkce.lock().await;
    sessions.remove(&session_id);
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
            }),
        );
    }

    {
        let mgr_for_task = mgr.clone();
        let session_id_for_task = session_id.clone();
        tokio::spawn(async move {
            let outcome = drive_device_code(device).await;
            settle_device(&mgr_for_task, &session_id_for_task, outcome).await;
        });
    }

    Ok(Json(response))
}

async fn drive_device_code(device: DeviceCode) -> Result<LoginResult, LoginError> {
    let tokens = poll_device_code(&device).await?;
    finalize_login(&login_target_path(), tokens)
}

async fn settle_device(
    mgr: &CodexLoginManager,
    session_id: &str,
    outcome: Result<LoginResult, LoginError>,
) {
    let session = {
        let sessions = mgr.device.lock().await;
        sessions.get(session_id).cloned()
    };
    let Some(session) = session else { return };
    if outcome.is_err() {
        tracing::warn!(
            user_code = %session.user_code,
            "codex_login: device code flow ended in error"
        );
    }
    let mut status = session.status.lock().await;
    status.outcome = Some(outcome);
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
    let status = session.status.lock().await;
    Ok(Json(match &status.outcome {
        None => LoginStatusJson::Pending,
        Some(r) => LoginStatusJson::from_result(r),
    }))
}

pub async fn device_cancel(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Json<serde_json::Value> {
    let mgr = state.codex_login.clone();
    let mut sessions = mgr.device.lock().await;
    sessions.remove(&session_id);
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
    /// Whether the active credential was constructed at startup. When `false`
    /// after a successful login, Phoenix must be restarted before the bridge
    /// activates.
    pub bridge_loaded_at_startup: bool,
    /// Whether `OPENAI_USE_CODEX_AUTH=1` is set in the current environment.
    /// Informational; the env-var only governs piggyback mode, not whether
    /// in-app login works.
    pub piggyback_env_set: bool,
}

pub async fn login_preflight(State(_state): State<AppState>) -> Json<LoginPreflight> {
    let auth_path = codex_credential::default_phoenix_auth_path();
    let piggyback_path = codex_credential::default_auth_path();
    let already_signed_in = codex_credential::CodexCredential::load(auth_path.clone()).is_ok();
    let bridge_loaded_at_startup = codex_credential::resolve_active_auth_path().is_some();
    let piggyback_env_set = std::env::var("OPENAI_USE_CODEX_AUTH")
        .ok()
        .is_some_and(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"));
    Json(LoginPreflight {
        auth_path: auth_path.display().to_string(),
        piggyback_path: piggyback_path.display().to_string(),
        already_signed_in,
        bridge_loaded_at_startup,
        piggyback_env_set,
    })
}
