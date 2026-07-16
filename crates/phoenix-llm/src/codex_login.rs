#![allow(clippy::wildcard_enum_match_arm)]
//! First-party `ChatGPT`/Codex `OAuth` login (browser + device code flows).
//!
//! Phoenix's `codex_credential` module reads `~/.codex/auth.json` produced by
//! the Codex CLI. This module produces the same file natively, removing the
//! Codex-CLI installation prerequisite and adding a headless option:
//!
//! - [`build_pkce_session`] + [`exchange_pkce_code`] cover the standard PKCE
//!   flow against `https://auth.openai.com/oauth/authorize`. The caller runs
//!   a loopback HTTP server on `127.0.0.1:1455` to receive the callback (the
//!   redirect URI is registered against the shared `client_id` and cannot be
//!   changed); a manual paste fallback is supported by feeding the code into
//!   [`exchange_pkce_code`] directly.
//! - [`request_device_code`] + [`poll_device_code`] cover `OpenAI`'s *custom*
//!   device code flow under `/api/accounts/deviceauth/`. Note this is **not**
//!   RFC 8628 — the polled token endpoint returns an authorization code plus
//!   a server-generated PKCE pair, which is then exchanged for tokens at the
//!   regular `/oauth/token` endpoint with `redirect_uri = {issuer}/deviceauth/callback`.
//!   Wire format follows Codex CLI's `device_code_auth.rs` exactly.
//!
//! These primitives are storage-agnostic — the caller passes the destination
//! path. Phoenix's API layer (`api/codex_login.rs`) writes to its own
//! `~/.phoenix-ide/codex-auth.json`; piggyback mode (`OPENAI_USE_CODEX_AUTH=1`)
//! reads from Codex CLI's `~/.codex/auth.json` but does not write back to it.
//! Writes are atomic and 0600 on Unix; the on-disk shape is identical to what
//! Codex CLI produces, so a `CodexCredential` constructed against either path
//! reads the same way.
//!
//! # Trade-offs
//!
//! - **PKCE flow** requires a browser reachable from the user's machine. On
//!   SSH'd or container hosts where the loopback bind would only resolve
//!   inside the container, the manual paste fallback receives the auth code
//!   from the user copying it out of the post-redirect URL bar.
//! - **Device code flow** requires no browser on the host running Phoenix —
//!   the user signs in on a separate device. It is gated by the issuer; the
//!   hosted `ChatGPT` issuer at `auth.openai.com` enables it. Self-hosted forks
//!   may return 404 from `/deviceauth/usercode`, surfaced as
//!   [`LoginError::DeviceCodeNotEnabled`].
//! - Both flows share the public Codex client ID
//!   (`app_EMoamEEZ73f0CkXaXp7hrann`); there is no per-app `OpenAI` registration
//!   step. Tokens written here are interchangeable with Codex CLI's.

use base64::Engine;
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const ISSUER_BASE: &str = "https://auth.openai.com";
pub const CALLBACK_PORT: u16 = 1455;
pub const REDIRECT_URI: &str = "http://localhost:1455/auth/callback";
pub const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
/// Identifies Phoenix to the `OAuth` backend. Codex CLI uses `"codex_cli_rs"`;
/// the value is informational and lets `OpenAI` distinguish first-party clients
/// from forks like ours.
pub const ORIGINATOR: &str = "phoenix-ide";

/// Scope set must match what Codex CLI requests, otherwise the access token's
/// granted scopes won't include the connector permissions Phoenix relies on
/// when it routes through the `ChatGPT` backend.
pub const SCOPE: &str =
    "openid profile email offline_access api.connectors.read api.connectors.invoke";

pub const DEVICE_CODE_TIMEOUT_SECS: u64 = 15 * 60;

/// JWT claim namespace under which `OpenAI` ID tokens nest the
/// `chatgpt_account_id` field — used as the `chatgpt-account-id` request
/// header when calling the `ChatGPT` backend.
const JWT_AUTH_CLAIM_NAMESPACE: &str = "https://api.openai.com/auth";

#[derive(Debug, thiserror::Error)]
pub enum LoginError {
    #[error("network error: {0}")]
    Network(String),
    #[error("oauth error: {0}")]
    OAuth(String),
    #[error("state mismatch on PKCE callback (possible CSRF)")]
    StateMismatch,
    #[error("device code flow timed out after 15 minutes")]
    DeviceCodeTimeout,
    #[error("device code login is not enabled for this issuer")]
    DeviceCodeNotEnabled,
    #[error("loopback callback port {0} is already in use")]
    PortInUse(u16),
    #[error("loopback server error: {0}")]
    Loopback(String),
    #[error("auth file write error: {0}")]
    Write(String),
    #[error("login was cancelled")]
    Cancelled,
}

// ---------------------------------------------------------------------------
// PKCE primitives
//
// Verifier is 64 random bytes encoded as URL-safe base64 (no padding) — yields
// a 86-char string within the RFC 7636 43..128 range. Challenge is the SHA-256
// of the verifier, again URL-safe-base64. State is 32 random bytes encoded the
// same way.
//
// Implementation matches /tmp/codex-cli/codex-rs/login/src/pkce.rs:12 and
// .../server.rs:518 (Apache-2.0). Reproduced here to avoid a workspace-deep
// codex-* dependency tree (the upstream login crate isn't published to crates.io
// and pulls 11+ internal codex-* crates for ~500 lines of OAuth logic).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct PkceCodes {
    pub code_verifier: String,
    pub code_challenge: String,
}

#[must_use]
pub fn generate_pkce() -> PkceCodes {
    let mut bytes = [0u8; 64];
    rand::rng().fill_bytes(&mut bytes);
    let code_verifier = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    let digest = Sha256::digest(code_verifier.as_bytes());
    let code_challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
    PkceCodes {
        code_verifier,
        code_challenge,
    }
}

#[must_use]
pub fn generate_state() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Compare the `state` value returned in the OAuth callback against the one we
/// sent in the authorize URL. A mismatch is treated as a CSRF signal; the
/// caller must NOT proceed to exchange the code. Extracted as a helper so the
/// guarantee can be unit-tested without standing up a real loopback server.
///
/// # Errors
/// Returns [`LoginError::StateMismatch`] when `returned` does not equal
/// `expected` (a possible CSRF signal; the caller must not exchange the code).
pub fn validate_state(expected: &str, returned: &str) -> Result<(), LoginError> {
    if expected == returned {
        Ok(())
    } else {
        Err(LoginError::StateMismatch)
    }
}

// ---------------------------------------------------------------------------
// Authorize URL construction
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct PkceSession {
    pub authorize_url: String,
    pub state: String,
    pub pkce: PkceCodes,
    pub redirect_uri: String,
}

#[must_use]
pub fn build_pkce_session() -> PkceSession {
    build_pkce_session_with(ISSUER_BASE, CLIENT_ID, REDIRECT_URI)
}

#[must_use]
pub fn build_pkce_session_with(issuer: &str, client_id: &str, redirect_uri: &str) -> PkceSession {
    let pkce = generate_pkce();
    let state = generate_state();
    let authorize_url = build_authorize_url(issuer, client_id, redirect_uri, &pkce, &state);
    PkceSession {
        authorize_url,
        state,
        pkce,
        redirect_uri: redirect_uri.to_string(),
    }
}

fn build_authorize_url(
    issuer: &str,
    client_id: &str,
    redirect_uri: &str,
    pkce: &PkceCodes,
    state: &str,
) -> String {
    let pairs: &[(&str, &str)] = &[
        ("response_type", "code"),
        ("client_id", client_id),
        ("redirect_uri", redirect_uri),
        ("scope", SCOPE),
        ("code_challenge", &pkce.code_challenge),
        ("code_challenge_method", "S256"),
        ("id_token_add_organizations", "true"),
        ("codex_cli_simplified_flow", "true"),
        ("state", state),
        ("originator", ORIGINATOR),
    ];
    let qs = pairs
        .iter()
        .map(|(k, v)| format!("{k}={}", urlencode(v)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{}/oauth/authorize?{qs}", issuer.trim_end_matches('/'))
}

/// Minimal URL-encoder. We avoid pulling a `url`/`urlencoding` crate — the set
/// of characters we percent-encode is small and well-defined (everything that
/// isn't `unreserved` per RFC 3986).
fn urlencode(s: &str) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            // Writing into a String can't fail.
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Token endpoint exchange
// ---------------------------------------------------------------------------

/// Raw token endpoint response. Fields that downstream callers need are extracted
/// by [`finalize_login`]; the rest are not parsed.
#[derive(Debug, Clone, Deserialize)]
#[allow(clippy::struct_field_names)] // mirrors the OAuth response shape
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub id_token: String,
}

/// Exchange a PKCE authorization code for tokens.
///
/// `redirect_uri` must match the value used in the authorize URL. For the
/// browser flow this is [`REDIRECT_URI`]; for the device-code flow it is
/// `{issuer}/deviceauth/callback`.
///
/// # Errors
/// Returns [`LoginError::Network`] on a transport failure, or
/// [`LoginError::OAuth`] on a non-success token-endpoint status or an
/// unparseable token response.
pub async fn exchange_pkce_code(
    issuer: &str,
    client_id: &str,
    redirect_uri: &str,
    code_verifier: &str,
    code: &str,
) -> Result<TokenResponse, LoginError> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| LoginError::Network(format!("client build failed: {e}")))?;

    let token_endpoint = format!("{}/oauth/token", issuer.trim_end_matches('/'));
    // Form-encoded body matches Codex CLI; OpenAI's token endpoint also
    // accepts JSON (codex_credential.rs uses JSON for refresh) but the
    // authorization_code grant is consistently form-encoded across both Pi
    // and Codex CLI implementations.
    let body = format!(
        "grant_type=authorization_code&code={}&redirect_uri={}&client_id={}&code_verifier={}",
        urlencode(code),
        urlencode(redirect_uri),
        urlencode(client_id),
        urlencode(code_verifier),
    );

    let resp = client
        .post(&token_endpoint)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .map_err(|e| LoginError::Network(format!("token request failed: {e}")))?;

    let status = resp.status();
    let body = resp
        .text()
        .await
        .map_err(|e| LoginError::Network(format!("read body failed: {e}")))?;

    if !status.is_success() {
        tracing::debug!(%status, body_len = body.len(), "codex_login: token endpoint error");
        return Err(LoginError::OAuth(format!("HTTP {status}")));
    }

    serde_json::from_str(&body).map_err(|e| LoginError::OAuth(format!("token response parse: {e}")))
}

// ---------------------------------------------------------------------------
// Device code flow — OpenAI's custom dialect (NOT RFC 8628).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct DeviceCode {
    pub verification_url: String,
    pub user_code: String,
    pub interval: Duration,
    pub expires_at: Instant,
    /// Server-issued opaque handle. Never displayed to the user.
    pub device_auth_id: String,
    /// The issuer this device code was minted against — needed when polling
    /// and exchanging because callers may override the default for testing.
    pub issuer: String,
    pub client_id: String,
}

#[derive(Deserialize)]
struct UserCodeResponse {
    device_auth_id: String,
    #[serde(alias = "user_code", alias = "usercode")]
    user_code: String,
    /// `OpenAI`'s response sometimes encodes this as a string-numeric. Codex CLI
    /// has a custom deserializer; we accept either by deserializing into a
    /// `serde_json::Value` and coercing here.
    #[serde(default)]
    interval: serde_json::Value,
}

/// Request a device code from the issuer. Returns immediately with the
/// `user_code` (to display to the user) and the polling parameters.
///
/// # Errors
/// Returns [`LoginError::DeviceCodeNotEnabled`] when the issuer has no
/// device-code endpoint, [`LoginError::Network`] on a transport failure, or
/// [`LoginError::OAuth`] on a non-success status or unparseable response.
pub async fn request_device_code() -> Result<DeviceCode, LoginError> {
    request_device_code_with(ISSUER_BASE, CLIENT_ID).await
}

/// Request a device code from an explicit `issuer` / `client_id`.
///
/// # Errors
/// Returns [`LoginError::DeviceCodeNotEnabled`] when the issuer has no
/// device-code endpoint, [`LoginError::Network`] on a transport failure, or
/// [`LoginError::OAuth`] on a non-success status or unparseable response.
pub async fn request_device_code_with(
    issuer: &str,
    client_id: &str,
) -> Result<DeviceCode, LoginError> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| LoginError::Network(format!("client build failed: {e}")))?;
    let base_url = issuer.trim_end_matches('/');
    let url = format!("{base_url}/api/accounts/deviceauth/usercode");
    let body = serde_json::json!({ "client_id": client_id });

    let resp = client
        .post(&url)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| LoginError::Network(format!("usercode request failed: {e}")))?;

    let status = resp.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(LoginError::DeviceCodeNotEnabled);
    }
    if !status.is_success() {
        return Err(LoginError::OAuth(format!("usercode HTTP {status}")));
    }

    let body_text = resp
        .text()
        .await
        .map_err(|e| LoginError::Network(format!("read usercode body: {e}")))?;
    let parsed: UserCodeResponse = serde_json::from_str(&body_text)
        .map_err(|e| LoginError::OAuth(format!("usercode parse: {e}")))?;

    let interval_secs = coerce_interval(&parsed.interval).unwrap_or(5);
    let interval = Duration::from_secs(interval_secs);
    let expires_at = Instant::now() + Duration::from_secs(DEVICE_CODE_TIMEOUT_SECS);

    Ok(DeviceCode {
        verification_url: format!("{base_url}/codex/device"),
        user_code: parsed.user_code,
        interval,
        expires_at,
        device_auth_id: parsed.device_auth_id,
        issuer: base_url.to_string(),
        client_id: client_id.to_string(),
    })
}

fn coerce_interval(v: &serde_json::Value) -> Option<u64> {
    match v {
        serde_json::Value::Number(n) => n.as_u64(),
        serde_json::Value::String(s) => s.trim().parse::<u64>().ok(),
        _ => None,
    }
}

#[derive(Deserialize)]
struct DevicePollResponse {
    authorization_code: String,
    /// Server-generated PKCE challenge — sent for parity with what the user
    /// would see in a browser address bar, but we don't validate it on this
    /// path (we trust the verifier the same response provided).
    #[allow(dead_code)]
    code_challenge: String,
    code_verifier: String,
}

/// Poll the device-auth token endpoint until either the user authorizes the
/// session (success) or the 15-minute window expires.
///
/// On success, returns a [`TokenResponse`] obtained by completing the second
/// leg — the device-code endpoint returns `{authorization_code, code_challenge,
/// code_verifier}` rather than tokens directly, and the caller still has to
/// exchange the code at `/oauth/token` with the special device-code redirect
/// URI. We bundle that step here so callers get a single `Result<TokenResponse>`.
///
/// # Errors
/// Returns [`LoginError::DeviceCodeTimeout`] when the authorization window
/// elapses, [`LoginError::Network`] on a transport failure, or
/// [`LoginError::OAuth`] on an unexpected poll status or unparseable response.
/// Errors from the final code exchange are propagated from
/// [`exchange_pkce_code`].
pub async fn poll_device_code(device: &DeviceCode) -> Result<TokenResponse, LoginError> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| LoginError::Network(format!("client build failed: {e}")))?;
    let base_url = device.issuer.trim_end_matches('/');
    let token_url = format!("{base_url}/api/accounts/deviceauth/token");

    let auth_code_response = loop {
        if Instant::now() >= device.expires_at {
            return Err(LoginError::DeviceCodeTimeout);
        }
        let body = serde_json::json!({
            "device_auth_id": device.device_auth_id,
            "user_code": device.user_code,
        });
        let resp = client
            .post(&token_url)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| LoginError::Network(format!("device poll failed: {e}")))?;
        let status = resp.status();
        if status.is_success() {
            let body_text = resp
                .text()
                .await
                .map_err(|e| LoginError::Network(format!("read poll body: {e}")))?;
            let parsed: DevicePollResponse = serde_json::from_str(&body_text)
                .map_err(|e| LoginError::OAuth(format!("device poll parse: {e}")))?;
            break parsed;
        }
        if status == reqwest::StatusCode::FORBIDDEN || status == reqwest::StatusCode::NOT_FOUND {
            // Pending. Sleep at most until the expiry.
            let remaining = device.expires_at.saturating_duration_since(Instant::now());
            let sleep_for = device.interval.min(remaining);
            if sleep_for.is_zero() {
                return Err(LoginError::DeviceCodeTimeout);
            }
            tokio::time::sleep(sleep_for).await;
            continue;
        }
        return Err(LoginError::OAuth(format!("device poll HTTP {status}")));
    };

    let device_redirect_uri = format!("{base_url}/deviceauth/callback");
    exchange_pkce_code(
        base_url,
        &device.client_id,
        &device_redirect_uri,
        &auth_code_response.code_verifier,
        &auth_code_response.authorization_code,
    )
    .await
}

// ---------------------------------------------------------------------------
// JWT claim extraction
// ---------------------------------------------------------------------------

/// Pull `chatgpt_account_id` out of a JWT's `https://api.openai.com/auth`
/// claim. Returns `None` for any parse failure, an unexpected JWT shape, or a
/// payload missing the expected claim — caller treats `None` as "no account
/// id available," same as a `null` field on disk.
pub fn extract_account_id(jwt: &str) -> Option<String> {
    let payload_b64 = jwt.split('.').nth(1)?;
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload_b64.as_bytes())
        .ok()?;
    let value: serde_json::Value = serde_json::from_slice(&payload).ok()?;
    let auth = value.get(JWT_AUTH_CLAIM_NAMESPACE)?;
    auth.get("chatgpt_account_id")?.as_str().map(str::to_string)
}

/// Pull the standard OIDC `email` claim out of an ID token's payload.
/// Surfaced by the in-app login preflight so the sidebar account chip can
/// show "alice@example.com" instead of the opaque `chatgpt_account_id` UUID.
/// `None` on any parse failure or missing claim — caller falls back to the
/// `account_id` form.
pub fn extract_email(jwt: &str) -> Option<String> {
    let payload_b64 = jwt.split('.').nth(1)?;
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload_b64.as_bytes())
        .ok()?;
    let value: serde_json::Value = serde_json::from_slice(&payload).ok()?;
    value.get("email")?.as_str().map(str::to_string)
}

// ---------------------------------------------------------------------------
// Persistence — write to ~/.codex/auth.json in the format
// codex_credential::CodexCredential::load() reads.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct LoginResult {
    pub access_token: String,
    pub refresh_token: String,
    pub id_token: String,
    pub account_id: Option<String>,
    pub auth_path: PathBuf,
}

#[derive(Serialize)]
struct OnDiskTokens<'a> {
    id_token: &'a str,
    access_token: &'a str,
    refresh_token: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    account_id: Option<&'a str>,
}

#[derive(Serialize)]
struct OnDiskAuthFile<'a> {
    #[serde(rename = "OPENAI_API_KEY", skip_serializing_if = "Option::is_none")]
    openai_api_key: Option<&'a str>,
    auth_mode: &'static str,
    tokens: OnDiskTokens<'a>,
    last_refresh: String,
}

/// Atomically persist login result to `auth_path`. Format is byte-compatible
/// with what `CodexCredential::load()` (and Codex CLI) reads.
///
/// # Errors
/// Returns [`LoginError::Write`] when serialization fails, the parent
/// directory cannot be created, or the temp-file write/sync/rename fails.
pub fn write_auth_file(auth_path: &Path, result: &LoginResult) -> Result<(), LoginError> {
    let now = chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S+00:00")
        .to_string();
    let on_disk = OnDiskAuthFile {
        openai_api_key: None,
        auth_mode: "chatgpt",
        tokens: OnDiskTokens {
            id_token: &result.id_token,
            access_token: &result.access_token,
            refresh_token: &result.refresh_token,
            account_id: result.account_id.as_deref(),
        },
        last_refresh: now,
    };
    let bytes = serde_json::to_vec_pretty(&on_disk)
        .map_err(|e| LoginError::Write(format!("serialize: {e}")))?;

    if let Some(parent) = auth_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| LoginError::Write(format!("mkdir {}: {e}", parent.display())))?;
    }
    let tmp = auth_path.with_extension("json.tmp");

    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)
            .map_err(|e| LoginError::Write(format!("open {}: {e}", tmp.display())))?;
        file.write_all(&bytes)
            .map_err(|e| LoginError::Write(format!("write {}: {e}", tmp.display())))?;
        file.sync_all()
            .map_err(|e| LoginError::Write(format!("sync {}: {e}", tmp.display())))?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(&tmp, &bytes)
            .map_err(|e| LoginError::Write(format!("write {}: {e}", tmp.display())))?;
    }

    std::fs::rename(&tmp, auth_path)
        .map_err(|e| LoginError::Write(format!("rename to {}: {e}", auth_path.display())))?;
    Ok(())
}

/// Convenience: take a [`TokenResponse`] from either flow, extract the
/// account id, write the file, and return a [`LoginResult`].
///
/// # Errors
/// Returns [`LoginError::Write`] when persisting the auth file fails
/// (serialization, directory creation, or the atomic write/rename).
pub fn finalize_login(auth_path: &Path, tokens: TokenResponse) -> Result<LoginResult, LoginError> {
    let account_id = extract_account_id(&tokens.id_token);
    let result = LoginResult {
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        id_token: tokens.id_token,
        account_id,
        auth_path: auth_path.to_path_buf(),
    };
    write_auth_file(auth_path, &result)?;
    Ok(result)
}

// ---------------------------------------------------------------------------
// Loopback callback server.
//
// Runs a single-purpose HTTP server on `127.0.0.1:1455` to receive the
// post-authorize redirect from the user's browser. The server runs only for
// the duration of a single login attempt; it is shut down when the caller
// drops the `LoopbackServer` handle.
// ---------------------------------------------------------------------------

use axum::{
    extract::{Query, State},
    response::Html,
    routing::get,
    Router,
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::oneshot;
use tokio::sync::Mutex as TokioMutex;

#[derive(Clone)]
struct CallbackState {
    sender: Arc<TokioMutex<Option<oneshot::Sender<CallbackPayload>>>>,
    /// State we generated and embedded in the authorize URL. The handler
    /// validates this **before** consuming the sender, so a malicious local
    /// process that hits `/auth/callback` with garbage cannot `DoS` the login
    /// session by burning the one-shot before the real browser callback
    /// arrives.
    expected_state: Arc<str>,
}

/// Either a successful auth code or an error reported by the OAuth provider.
#[derive(Debug, Clone)]
pub enum CallbackPayload {
    Success {
        code: String,
        state: String,
    },
    Error {
        error: String,
        description: Option<String>,
    },
}

pub struct LoopbackServer {
    /// Receiver for the single callback. May be consumed at most once.
    pub callback_rx: oneshot::Receiver<CallbackPayload>,
    /// One sender per bound listener (typically two: 127.0.0.1 + `::1`).
    /// Dropping all of them initiates graceful shutdown.
    shutdown_txs: Vec<oneshot::Sender<()>>,
    handles: Vec<tokio::task::JoinHandle<()>>,
}

impl LoopbackServer {
    /// Bind to the loopback callback port and start serving.
    ///
    /// Tries both `127.0.0.1:port` and `[::1]:port`. The redirect URI is
    /// registered as `localhost`, which on dual-stack systems can resolve to
    /// either family depending on the browser; binding both makes the
    /// callback robust regardless of resolution order.
    ///
    /// Bind to at least one address is required. If the requested port is in
    /// use on either loopback (typical: a previous Codex CLI login left
    /// something listening), returns `PortInUse`. If only one family is
    /// unavailable on the host (e.g. no IPv6 stack), the other is enough.
    ///
    /// # Errors
    /// Returns [`LoginError::PortInUse`] when the callback port is already
    /// bound, or [`LoginError::Loopback`] when neither loopback family could
    /// be bound.
    pub async fn start(expected_state: String) -> Result<Self, LoginError> {
        Self::start_on_port(CALLBACK_PORT, expected_state).await
    }

    /// Bind the loopback callback server to an explicit `port`.
    ///
    /// # Errors
    /// Returns [`LoginError::PortInUse`] when `port` is already bound on a
    /// loopback family, or [`LoginError::Loopback`] when neither `127.0.0.1`
    /// nor `[::1]` could be bound at all.
    pub async fn start_on_port(port: u16, expected_state: String) -> Result<Self, LoginError> {
        let v4_addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
        let v6_addr = std::net::SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], port));

        let v4 = match tokio::net::TcpListener::bind(v4_addr).await {
            Ok(l) => Some(l),
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                return Err(LoginError::PortInUse(port));
            }
            Err(e) => {
                tracing::debug!(addr = %v4_addr, error = %e,
                    "codex_login: IPv4 loopback bind failed");
                None
            }
        };
        let v6 = match tokio::net::TcpListener::bind(v6_addr).await {
            Ok(l) => Some(l),
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                if v4.is_none() {
                    return Err(LoginError::PortInUse(port));
                }
                tracing::debug!(addr = %v6_addr, error = %e,
                    "codex_login: IPv6 loopback in use, continuing with IPv4 only");
                None
            }
            Err(e) => {
                tracing::debug!(addr = %v6_addr, error = %e,
                    "codex_login: IPv6 loopback bind failed (no v6 stack?)");
                None
            }
        };

        if v4.is_none() && v6.is_none() {
            return Err(LoginError::Loopback(format!(
                "neither 127.0.0.1:{port} nor [::1]:{port} could be bound"
            )));
        }

        let (callback_tx, callback_rx) = oneshot::channel();
        let state = CallbackState {
            sender: Arc::new(TokioMutex::new(Some(callback_tx))),
            expected_state: Arc::from(expected_state),
        };

        let mut shutdown_txs = Vec::new();
        let mut handles = Vec::new();
        for (listener, label) in [(v4, "127.0.0.1"), (v6, "::1")]
            .into_iter()
            .filter_map(|(l, label)| l.map(|l| (l, label)))
        {
            let app = Router::new()
                .route("/auth/callback", get(callback_handler))
                .with_state(state.clone());
            let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
            let handle = tokio::spawn(async move {
                let server = axum::serve(listener, app).with_graceful_shutdown(async move {
                    let _ = shutdown_rx.await;
                });
                if let Err(e) = server.await {
                    tracing::warn!(addr = %label, error = %e,
                        "codex_login: loopback server exited with error");
                }
            });
            shutdown_txs.push(shutdown_tx);
            handles.push(handle);
        }

        Ok(Self {
            callback_rx,
            shutdown_txs,
            handles,
        })
    }

    /// Initiate graceful shutdown of every bound listener. Idempotent.
    pub fn shutdown(&mut self) {
        for tx in self.shutdown_txs.drain(..) {
            let _ = tx.send(());
        }
    }
}

impl Drop for LoopbackServer {
    fn drop(&mut self) {
        self.shutdown();
        for h in self.handles.drain(..) {
            h.abort();
        }
    }
}

async fn callback_handler(
    State(state): State<CallbackState>,
    Query(params): Query<HashMap<String, String>>,
) -> Html<String> {
    // RFC 6749 §4.1.2.1: the OAuth provider includes `state` in both the
    // success and error redirect. Validate it BEFORE consuming the one-shot.
    // Any caller — including a malicious local process trying to DoS the
    // login by burning the channel with garbage — that doesn't supply our
    // exact state value sees an error page but does NOT settle the session.
    // The real browser callback can still arrive afterwards.
    let returned_state = params.get("state").map(String::as_str);
    if returned_state != Some(&*state.expected_state) {
        tracing::debug!(
            "codex_login: rejecting callback with missing/mismatched state — \
             not consuming session sender"
        );
        return Html(ERROR_HTML.to_string());
    }

    let payload = if let Some(error) = params.get("error") {
        CallbackPayload::Error {
            error: error.clone(),
            description: params.get("error_description").cloned(),
        }
    } else if let Some(code) = params.get("code") {
        CallbackPayload::Success {
            code: code.clone(),
            // We've already validated state above, but pass it through for
            // the defense-in-depth check in `drive_pkce`.
            state: state.expected_state.to_string(),
        }
    } else {
        // State validated but neither error nor code present. Treat as
        // garbage; do NOT consume the sender.
        return Html(ERROR_HTML.to_string());
    };

    let mut guard = state.sender.lock().await;
    if let Some(sender) = guard.take() {
        let _ = sender.send(payload.clone());
    }
    drop(guard);

    let body = match payload {
        CallbackPayload::Success { .. } => SUCCESS_HTML,
        CallbackPayload::Error { .. } => ERROR_HTML,
    };
    Html(body.to_string())
}

const SUCCESS_HTML: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><title>Phoenix — signed in</title>
<style>body{font-family:system-ui,-apple-system,sans-serif;max-width:480px;margin:80px auto;padding:0 16px;color:#111}
h1{margin:0 0 8px}p{color:#555;line-height:1.5}</style></head>
<body><h1>You're signed in.</h1>
<p>You can close this tab and return to Phoenix IDE.</p></body></html>
"#;

const ERROR_HTML: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><title>Phoenix — sign-in failed</title>
<style>body{font-family:system-ui,-apple-system,sans-serif;max-width:480px;margin:80px auto;padding:0 16px;color:#111}
h1{margin:0 0 8px}p{color:#555;line-height:1.5}</style></head>
<body><h1>Sign-in didn't complete.</h1>
<p>Return to Phoenix IDE for details. You can close this tab.</p></body></html>
"#;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;

    fn jwt_with_claims(claims: &serde_json::Value) -> String {
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"none"}"#);
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims).unwrap());
        format!("{header}.{payload}.")
    }

    #[test]
    fn pkce_codes_are_well_formed() {
        let p = generate_pkce();
        // Verifier: 86 chars (64 bytes -> base64url-no-pad).
        assert_eq!(p.code_verifier.len(), 86);
        // Challenge: SHA-256 -> 32 bytes -> 43 chars.
        assert_eq!(p.code_challenge.len(), 43);
        // Recompute the challenge and compare.
        let expected = URL_SAFE_NO_PAD.encode(Sha256::digest(p.code_verifier.as_bytes()));
        assert_eq!(p.code_challenge, expected);
    }

    #[test]
    fn pkce_codes_are_unique_per_call() {
        let a = generate_pkce();
        let b = generate_pkce();
        assert_ne!(a.code_verifier, b.code_verifier);
        assert_ne!(a.code_challenge, b.code_challenge);
    }

    #[test]
    fn build_authorize_url_has_required_params() {
        let session = build_pkce_session();
        let url = &session.authorize_url;
        assert!(url.starts_with("https://auth.openai.com/oauth/authorize?"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains(&format!("client_id={CLIENT_ID}")));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains(&format!("code_challenge={}", session.pkce.code_challenge)));
        assert!(url.contains(&format!("state={}", session.state)));
        assert!(url.contains("id_token_add_organizations=true"));
        assert!(url.contains("codex_cli_simplified_flow=true"));
        assert!(url.contains(&format!("originator={ORIGINATOR}")));
        // Scope is space-separated -> URL-encoded as %20.
        assert!(url.contains("openid%20profile%20email%20offline_access"));
    }

    #[test]
    fn extract_account_id_pulls_from_namespaced_claim() {
        let jwt = jwt_with_claims(&serde_json::json!({
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "acc-abc"
            }
        }));
        assert_eq!(extract_account_id(&jwt).as_deref(), Some("acc-abc"));
    }

    #[test]
    fn extract_account_id_returns_none_when_claim_missing() {
        let jwt = jwt_with_claims(&serde_json::json!({
            "https://api.openai.com/auth": { "other_field": "x" }
        }));
        assert_eq!(extract_account_id(&jwt), None);

        let jwt = jwt_with_claims(&serde_json::json!({ "sub": "user" }));
        assert_eq!(extract_account_id(&jwt), None);

        assert_eq!(extract_account_id("not.a.jwt"), None);
        assert_eq!(extract_account_id("no-dots"), None);
    }

    #[test]
    fn write_auth_file_produces_codex_credential_compatible_shape() {
        // Round-trip via codex_credential's reader.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        let result = LoginResult {
            access_token: "at".into(),
            refresh_token: "rt".into(),
            id_token: "it".into(),
            account_id: Some("acc-1".into()),
            auth_path: path.clone(),
        };
        write_auth_file(&path, &result).unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed["auth_mode"], "chatgpt");
        assert_eq!(parsed["tokens"]["access_token"], "at");
        assert_eq!(parsed["tokens"]["refresh_token"], "rt");
        assert_eq!(parsed["tokens"]["id_token"], "it");
        assert_eq!(parsed["tokens"]["account_id"], "acc-1");
        assert!(parsed["last_refresh"].is_string());

        // CodexCredential::load() must accept this file.
        let (_cred, account_id) = crate::codex_credential::CodexCredential::load(path).unwrap();
        assert_eq!(account_id.as_deref(), Some("acc-1"));
    }

    #[cfg(unix)]
    #[test]
    fn write_auth_file_sets_0600_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        write_auth_file(
            &path,
            &LoginResult {
                access_token: "a".into(),
                refresh_token: "r".into(),
                id_token: "i".into(),
                account_id: None,
                auth_path: path.clone(),
            },
        )
        .unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn finalize_login_extracts_account_id_from_id_token() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        let id_token = jwt_with_claims(&serde_json::json!({
            "https://api.openai.com/auth": { "chatgpt_account_id": "acc-zzz" }
        }));
        let tokens = TokenResponse {
            access_token: "at".into(),
            refresh_token: "rt".into(),
            id_token,
        };
        let result = finalize_login(&path, tokens).unwrap();
        assert_eq!(result.account_id.as_deref(), Some("acc-zzz"));

        let parsed: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(parsed["tokens"]["account_id"], "acc-zzz");
    }

    #[test]
    fn coerce_interval_handles_numeric_and_string() {
        assert_eq!(coerce_interval(&serde_json::json!(5)), Some(5));
        assert_eq!(coerce_interval(&serde_json::json!("5")), Some(5));
        assert_eq!(coerce_interval(&serde_json::json!("  7 ")), Some(7));
        assert_eq!(coerce_interval(&serde_json::json!(null)), None);
        assert_eq!(coerce_interval(&serde_json::json!("nope")), None);
    }

    #[test]
    fn urlencode_handles_oauth_special_chars() {
        // OAuth scopes have spaces; redirect_uris have `:` `/` and `?`.
        assert_eq!(urlencode("openid profile"), "openid%20profile");
        assert_eq!(
            urlencode("http://localhost:1455/auth/callback"),
            "http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback"
        );
        assert_eq!(urlencode("a-b_c.d~e"), "a-b_c.d~e");
    }

    /// PKCE state validation: when the redirect callback's `state` doesn't
    /// match the value we generated, we treat it as a CSRF attempt and abort
    /// before exchanging the code. Exercises [`validate_state`], the function
    /// `drive_pkce` actually calls — so removing or weakening the comparison
    /// in production code would fail this test.
    #[test]
    fn validate_state_rejects_mismatch_and_accepts_match() {
        let session = build_pkce_session();

        assert!(validate_state(&session.state, &session.state).is_ok());

        // Substring of the real state still mismatches — guards against a
        // future regression to a permissive `starts_with` / prefix check.
        let prefix = session
            .state
            .get(..session.state.len() - 1)
            .expect("state has at least one char");
        assert!(matches!(
            validate_state(&session.state, prefix),
            Err(LoginError::StateMismatch)
        ));

        // Empty returned state.
        assert!(matches!(
            validate_state(&session.state, ""),
            Err(LoginError::StateMismatch)
        ));

        // Attacker-controlled value bearing no relation to ours.
        assert!(matches!(
            validate_state(&session.state, "attacker-supplied-state"),
            Err(LoginError::StateMismatch)
        ));
    }

    /// Loopback callback `DoS` protection (PR #57 review): a request without
    /// a matching `state` parameter must NOT consume the one-shot sender.
    /// This guards against a local malicious process firing one bogus GET
    /// to `:1455/auth/callback` and stranding the real browser callback.
    /// Exercises `callback_handler` directly with a hand-built
    /// `CallbackState`, since spinning up a real listener on a fixed port
    /// risks conflict with an actual login flow.
    #[tokio::test]
    async fn callback_handler_rejects_wrong_state_without_consuming_sender() {
        let (tx, rx) = oneshot::channel::<CallbackPayload>();
        let state = CallbackState {
            sender: Arc::new(TokioMutex::new(Some(tx))),
            expected_state: Arc::from("real-state"),
        };

        // (1) Mismatched state — handler rejects, sender preserved.
        let mut bad_params = HashMap::new();
        bad_params.insert("state".to_string(), "attacker-supplied".to_string());
        bad_params.insert("code".to_string(), "some-code".to_string());
        let _ = callback_handler(
            axum::extract::State(state.clone()),
            axum::extract::Query(bad_params),
        )
        .await;
        assert!(
            state.sender.lock().await.is_some(),
            "wrong-state callback must not consume the one-shot sender"
        );

        // (2) Missing state parameter entirely — same protection.
        let mut missing_params = HashMap::new();
        missing_params.insert("code".to_string(), "some-code".to_string());
        let _ = callback_handler(
            axum::extract::State(state.clone()),
            axum::extract::Query(missing_params),
        )
        .await;
        assert!(
            state.sender.lock().await.is_some(),
            "callback without state must not consume the one-shot sender"
        );

        // (3) Real callback now arrives with matching state — settles.
        let mut good_params = HashMap::new();
        good_params.insert("state".to_string(), "real-state".to_string());
        good_params.insert("code".to_string(), "real-code".to_string());
        let _ = callback_handler(
            axum::extract::State(state.clone()),
            axum::extract::Query(good_params),
        )
        .await;
        let payload = rx.await.expect("real callback must settle the session");
        match payload {
            CallbackPayload::Success {
                code,
                state: returned_state,
            } => {
                assert_eq!(code, "real-code");
                assert_eq!(returned_state, "real-state");
            }
            other @ CallbackPayload::Error { .. } => panic!("expected Success, got {other:?}"),
        }
    }

    /// Device code timeout: when `expires_at` is in the past, `poll_device_code`
    /// returns immediately with `DeviceCodeTimeout` instead of issuing an HTTP
    /// request. This is the loop-bound that prevents a silently-stuck poller.
    #[tokio::test]
    async fn poll_device_code_returns_timeout_when_expired() {
        let device = DeviceCode {
            verification_url: "https://example/codex/device".into(),
            user_code: "ABCD-1234".into(),
            interval: Duration::from_secs(5),
            expires_at: Instant::now()
                .checked_sub(Duration::from_secs(1))
                .expect("Instant in the past"),
            device_auth_id: "dev-id".into(),
            issuer: "https://auth.example.invalid".into(), // unreachable; we shouldn't hit it
            client_id: "client".into(),
        };
        let err = poll_device_code(&device).await.unwrap_err();
        assert!(matches!(err, LoginError::DeviceCodeTimeout));
    }
}
