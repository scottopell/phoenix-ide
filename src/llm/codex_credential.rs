//! `ChatGPT` `OAuth` credential source bridged from the local `Codex` CLI.
//!
//! Reads `~/.codex/auth.json` (written by `codex login` against a `ChatGPT`
//! Plus/Pro/Team/Enterprise account), returns the access token for use with
//! the `ChatGPT`-backend Responses API at `https://chatgpt.com/backend-api/codex`,
//! and refreshes the token via the `OpenAI` `OAuth` endpoint when it nears expiry.
//!
//! Modelled after Simon Willison's `llm-openai-via-codex` plugin.
//!
//! Experimental: gated behind `OPENAI_USE_CODEX_AUTH=1`. Removing this file and
//! the env-flag branch in `registry.rs` reverts to standard `OpenAI` API key auth.
//!
//! # Wire details
//! - Refresh URL: `https://auth.openai.com/oauth/token`
//! - Client ID: `app_EMoamEEZ73f0CkXaXp7hrann`
//! - Grant: `refresh_token` with JSON body `{client_id, grant_type, refresh_token}`
//! - Backend URL: `https://chatgpt.com/backend-api/codex/responses`
//! - Per-request header: `chatgpt-account-id: <account_id>`

use crate::llm::registry::CredentialSource;
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};
use tokio::sync::Mutex;

const REFRESH_URL: &str = "https://auth.openai.com/oauth/token";
const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const REFRESH_SKEW_SECS: i64 = 30;
pub const CODEX_BACKEND_URL: &str = "https://chatgpt.com/backend-api/codex/responses";

#[derive(Debug, thiserror::Error)]
pub enum CodexAuthError {
    #[error("codex auth file not found at {0:?} — run `codex login` first")]
    NotFound(PathBuf),
    #[error("expected auth_mode 'chatgpt' in {path:?}, got {found:?} — this provider only supports ChatGPT OAuth tokens")]
    WrongAuthMode {
        path: PathBuf,
        found: Option<String>,
    },
    #[error("no access_token in {0:?}")]
    NoAccessToken(PathBuf),
    #[error("no refresh_token in {0:?} — run `codex login` again")]
    NoRefreshToken(PathBuf),
    #[error("refresh token rejected ({reason}) — run `codex login` to re-authenticate")]
    RefreshTokenInvalid { reason: String },
    #[error("token refresh failed: {0}")]
    RefreshFailed(String),
    #[error("io error on {path:?}: {err}")]
    Io { path: PathBuf, err: std::io::Error },
    #[error("json parse error in {path:?}: {err}")]
    ParseAuthFile {
        path: PathBuf,
        err: serde_json::Error,
    },
}

#[derive(Debug, Deserialize, Serialize)]
struct AuthFile {
    auth_mode: Option<String>,
    tokens: Option<AuthTokens>,
    last_refresh: Option<String>,
    #[serde(flatten)]
    other: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Deserialize, Serialize)]
struct AuthTokens {
    access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    refresh_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    account_id: Option<String>,
    #[serde(flatten)]
    other: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct RefreshRequest<'a> {
    client_id: &'a str,
    grant_type: &'a str,
    refresh_token: &'a str,
}

#[derive(Debug, Deserialize)]
#[allow(clippy::struct_field_names)] // mirrors the OAuth response shape
struct RefreshResponse {
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RefreshError {
    error: Option<String>,
}

/// Resolve the auth.json path: `$CODEX_HOME/auth.json` if set, else `~/.codex/auth.json`.
pub fn default_auth_path() -> PathBuf {
    if let Ok(home) = std::env::var("CODEX_HOME") {
        PathBuf::from(home).join("auth.json")
    } else {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home).join(".codex").join("auth.json")
    }
}

/// Read `auth.json` and validate it's a ChatGPT-mode file.
fn read_auth_file(path: &PathBuf) -> Result<AuthFile, CodexAuthError> {
    if !path.exists() {
        return Err(CodexAuthError::NotFound(path.clone()));
    }
    let bytes = std::fs::read(path).map_err(|err| CodexAuthError::Io {
        path: path.clone(),
        err,
    })?;
    let auth: AuthFile = serde_json::from_slice(&bytes).map_err(|err| {
        CodexAuthError::ParseAuthFile {
            path: path.clone(),
            err,
        }
    })?;
    if auth.auth_mode.as_deref() != Some("chatgpt") {
        return Err(CodexAuthError::WrongAuthMode {
            path: path.clone(),
            found: auth.auth_mode.clone(),
        });
    }
    Ok(auth)
}

/// Atomically write `auth.json` with mode 0600.
///
/// On Unix, the temp file is opened with mode 0600 from the start so the
/// access/refresh tokens never sit briefly readable to other local users
/// under the process umask. Falls back to a write-then-chmod sequence on
/// non-Unix.
fn write_auth_file(path: &PathBuf, auth: &AuthFile) -> Result<(), CodexAuthError> {
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(auth).map_err(|err| CodexAuthError::ParseAuthFile {
        path: path.clone(),
        err,
    })?;

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
            .map_err(|err| CodexAuthError::Io {
                path: tmp.clone(),
                err,
            })?;
        file.write_all(&bytes).map_err(|err| CodexAuthError::Io {
            path: tmp.clone(),
            err,
        })?;
        file.sync_all().map_err(|err| CodexAuthError::Io {
            path: tmp.clone(),
            err,
        })?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(&tmp, &bytes).map_err(|err| CodexAuthError::Io {
            path: tmp.clone(),
            err,
        })?;
    }

    std::fs::rename(&tmp, path).map_err(|err| CodexAuthError::Io {
        path: path.clone(),
        err,
    })?;
    Ok(())
}

/// Decode a JWT's `exp` claim (seconds since epoch). Returns `None` for any parse failure.
fn jwt_exp(token: &str) -> Option<i64> {
    let payload_b64 = token.split('.').nth(1)?;
    // JWTs use URL-safe base64 without padding.
    let payload =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload_b64.as_bytes()).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&payload).ok()?;
    value.get("exp").and_then(serde_json::Value::as_i64)
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(0))
        .unwrap_or(0)
}

fn now_iso8601_utc() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S+00:00").to_string()
}

/// Call `OpenAI`'s `OAuth` token endpoint to refresh.
async fn refresh_tokens(refresh_token: &str) -> Result<RefreshResponse, CodexAuthError> {
    let body = RefreshRequest {
        client_id: CLIENT_ID,
        grant_type: "refresh_token",
        refresh_token,
    };
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| CodexAuthError::RefreshFailed(format!("client build failed: {e}")))?;

    let response = client
        .post(REFRESH_URL)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| CodexAuthError::RefreshFailed(format!("network error: {e}")))?;

    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|e| CodexAuthError::RefreshFailed(format!("read body failed: {e}")))?;

    if !status.is_success() {
        // Distinguish "user must re-login" from transient failures.
        let reason = serde_json::from_str::<RefreshError>(&text)
            .ok()
            .and_then(|e| e.error)
            .unwrap_or_default();
        if matches!(
            reason.as_str(),
            "refresh_token_expired" | "refresh_token_reused" | "refresh_token_invalidated"
        ) {
            return Err(CodexAuthError::RefreshTokenInvalid { reason });
        }
        // Don't put the response body in the error: the OAuth endpoint may
        // emit token-bearing payloads under malformed-success conditions, and
        // this error string ends up in `tracing::warn!` output. Log only what
        // is safe to log.
        return Err(CodexAuthError::RefreshFailed(format!(
            "HTTP {status} (body redacted)"
        )));
    }

    let parsed: RefreshResponse = serde_json::from_str(&text)
        .map_err(|e| CodexAuthError::RefreshFailed(format!("response parse: {e}")))?;
    // Reject success responses missing an access_token. Without this check,
    // `apply_refresh_response` would preserve the old (expired) token and the
    // service would happily keep using it.
    if parsed.access_token.as_deref().is_none_or(str::is_empty) {
        return Err(CodexAuthError::RefreshFailed(
            "OAuth endpoint returned success with empty/missing access_token".to_string(),
        ));
    }
    Ok(parsed)
}

#[derive(Debug, Clone)]
struct CachedToken {
    access_token: String,
    /// `exp` from the JWT, in seconds since epoch. May be `None` if unparseable —
    /// in that case we always treat it as expired and re-read the file.
    exp: Option<i64>,
    /// Mtime of `auth.json` when this entry was cached. On the next fetch we
    /// stat the file and force a re-read if the mtime has changed, so a
    /// `codex login` against a different account mid-session is picked up
    /// even if the previously-cached token is still within its `exp` window.
    file_mtime: Option<std::time::SystemTime>,
}

/// State protected by the inner mutex. Holding the mutex through the entire
/// `fetch()` body serialises concurrent refreshes — the OAuth server rotates
/// refresh tokens and rejects replays as `refresh_token_reused`, so racing two
/// refreshes for the same expired token would fail one of them.
#[derive(Default)]
struct InnerState {
    cached: Option<CachedToken>,
    /// Set by `invalidate()` after a 401; forces the next `fetch()` to skip
    /// the "JWT exp still in the future" shortcut and rotate via the refresh
    /// token, so a server-side revocation actually replaces the token.
    force_refresh: bool,
}

/// Return the file mtime, or `None` if it can't be read (the caller treats
/// `None` as "file changed" so a stat failure forces a re-read rather than
/// silently serving a stale cache).
fn file_mtime(path: &PathBuf) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).ok().and_then(|m| m.modified().ok())
}

/// `CredentialSource` impl for `ChatGPT` `OAuth` tokens borrowed from the `Codex` CLI.
pub struct CodexCredential {
    auth_path: PathBuf,
    inner: Mutex<InnerState>,
    /// `account_id` mirrored from the most recent file read. Updated by
    /// `fetch()` so per-request callers (`account_id()`) see the current
    /// account after a `codex login` mid-session.
    account_id: StdMutex<Option<String>>,
    /// Most recent fetch failure message, surfaced via the `CredentialSource`
    /// `last_error_hint()` trait method so the UI shows actionable recovery
    /// guidance ("run `codex login`") instead of the generic auth-failure
    /// message.
    last_error: StdMutex<Option<String>>,
}

impl std::fmt::Debug for CodexCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodexCredential")
            .field("auth_path", &self.auth_path)
            .field("inner", &"[redacted]")
            .field(
                "account_id",
                &self.account_id.lock().ok().map(|g| g.is_some()),
            )
            .field(
                "last_error",
                &self.last_error.lock().ok().map(|g| g.is_some()),
            )
            .finish()
    }
}

impl CodexCredential {
    pub fn new(auth_path: PathBuf) -> Self {
        Self {
            auth_path,
            inner: Mutex::new(InnerState::default()),
            account_id: StdMutex::new(None),
            last_error: StdMutex::new(None),
        }
    }

    /// Read `auth.json` once at construction and pull the `account_id` for header injection.
    /// Returns the credential plus the `account_id` (caller wires it into `chatgpt-account-id`).
    /// Errors if the file is missing or wrong-mode at startup time.
    pub fn load(auth_path: PathBuf) -> Result<(Arc<Self>, Option<String>), CodexAuthError> {
        let auth = read_auth_file(&auth_path)?;
        let tokens = auth
            .tokens
            .as_ref()
            .ok_or_else(|| CodexAuthError::NoAccessToken(auth_path.clone()))?;
        if tokens.access_token.is_empty() {
            return Err(CodexAuthError::NoAccessToken(auth_path.clone()));
        }
        let account_id = tokens.account_id.clone();
        let cred = Arc::new(Self::new(auth_path));
        if let Ok(mut guard) = cred.account_id.lock() {
            (*guard).clone_from(&account_id);
        }
        Ok((cred, account_id))
    }

    /// Snapshot the current `account_id` without forcing a file read.
    pub fn account_id(&self) -> Option<String> {
        self.account_id.lock().ok().and_then(|g| g.clone())
    }

    /// Acquire a valid access token, refreshing if needed. The mutex is held
    /// for the full body so concurrent callers that arrive with an expired
    /// token serialise behind a single refresh — the second caller sees the
    /// freshly-cached token and returns without issuing its own refresh.
    ///
    /// `force_refresh` is only cleared after a successful refresh, so a
    /// transient refresh failure (network blip) does not silently demote the
    /// next call back to "JWT looks fine, return cached token."
    async fn fetch(&self) -> Result<String, CodexAuthError> {
        let mut state = self.inner.lock().await;
        let forced = state.force_refresh;
        let current_mtime = file_mtime(&self.auth_path);

        // Fast path: in-memory cache still good AND auth.json hasn't changed
        // since we cached. The mtime check picks up `codex login` against a
        // different account mid-session — without it the in-memory cache
        // serves the previous user's token until expiry.
        if !forced {
            if let Some(c) = state.cached.as_ref() {
                if c.file_mtime == current_mtime {
                    if let Some(exp) = c.exp {
                        if now_unix() < exp - REFRESH_SKEW_SECS {
                            return Ok(c.access_token.clone());
                        }
                    }
                }
            }
        }

        // Re-read the file. The Codex CLI may have refreshed for us, in which
        // case we adopt its token without doing our own refresh.
        let mut auth = read_auth_file(&self.auth_path)?;
        let tokens = auth
            .tokens
            .as_mut()
            .ok_or_else(|| CodexAuthError::NoAccessToken(self.auth_path.clone()))?;

        if let Ok(mut guard) = self.account_id.lock() {
            (*guard).clone_from(&tokens.account_id);
        }

        // Re-stat after read so we cache the mtime that matches what we
        // actually loaded.
        let post_read_mtime = file_mtime(&self.auth_path);

        // Skip the file-fresh shortcut when forced — invalidate() is called
        // after a 401, so even a JWT whose `exp` claim is still future is now
        // server-side revoked. Going straight to refresh is the only way to
        // actually rotate.
        if !forced {
            if let Some(exp_unix) = jwt_exp(&tokens.access_token) {
                if now_unix() < exp_unix - REFRESH_SKEW_SECS {
                    let token = tokens.access_token.clone();
                    state.cached = Some(CachedToken {
                        access_token: token.clone(),
                        exp: Some(exp_unix),
                        file_mtime: post_read_mtime,
                    });
                    return Ok(token);
                }
            }
        }

        let refresh_token = tokens
            .refresh_token
            .clone()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| CodexAuthError::NoRefreshToken(self.auth_path.clone()))?;

        tracing::info!("codex_credential: refreshing access token");
        let new_tokens = refresh_tokens(&refresh_token).await?;
        apply_refresh_response(&mut auth, new_tokens);
        write_auth_file(&self.auth_path, &auth)?;

        let new_token = auth
            .tokens
            .as_ref()
            .map(|t| t.access_token.clone())
            .unwrap_or_default();
        let new_exp = jwt_exp(&new_token);
        state.cached = Some(CachedToken {
            access_token: new_token.clone(),
            exp: new_exp,
            file_mtime: file_mtime(&self.auth_path),
        });
        // Successful refresh — clear the force_refresh flag set by invalidate().
        state.force_refresh = false;
        Ok(new_token)
    }
}

/// Apply a refresh response to an in-memory `AuthFile`, preserving any
/// non-rotated token fields and stamping `last_refresh`. Pure so it can be
/// unit-tested without a live OAuth endpoint.
fn apply_refresh_response(auth: &mut AuthFile, response: RefreshResponse) {
    if let Some(tokens) = auth.tokens.as_mut() {
        if let Some(at) = response.access_token {
            tokens.access_token = at;
        }
        if let Some(it) = response.id_token {
            tokens.id_token = Some(it);
        }
        if let Some(rt) = response.refresh_token {
            tokens.refresh_token = Some(rt);
        }
    }
    auth.last_refresh = Some(now_iso8601_utc());
}

/// Convert a `CodexAuthError` into a short, user-actionable hint string for
/// `LlmAuth::resolve()` to display. Pure mapping — keeps the trait method
/// non-async and avoids holding any locks while formatting.
fn error_hint(err: &CodexAuthError) -> String {
    match err {
        CodexAuthError::NotFound(_) => {
            "ChatGPT credentials not found at ~/.codex/auth.json — run `codex login`"
                .to_string()
        }
        CodexAuthError::WrongAuthMode { .. } => {
            "~/.codex/auth.json is in API-key mode, not ChatGPT — run `codex login` to switch"
                .to_string()
        }
        CodexAuthError::NoAccessToken(_) | CodexAuthError::NoRefreshToken(_) => {
            "Codex credentials are missing fields — run `codex login` to refresh".to_string()
        }
        CodexAuthError::RefreshTokenInvalid { reason } => {
            format!("ChatGPT refresh token rejected ({reason}) — run `codex login` to re-authenticate")
        }
        CodexAuthError::RefreshFailed(msg) => {
            format!("ChatGPT token refresh failed: {msg}")
        }
        CodexAuthError::Io { err, .. } => {
            format!("Could not read ~/.codex/auth.json: {err}")
        }
        CodexAuthError::ParseAuthFile { err, .. } => {
            format!("~/.codex/auth.json is malformed: {err}")
        }
    }
}

#[async_trait::async_trait]
impl CredentialSource for CodexCredential {
    async fn get(&self) -> Option<String> {
        match self.fetch().await {
            Ok(token) => {
                if let Ok(mut guard) = self.last_error.lock() {
                    *guard = None;
                }
                Some(token)
            }
            Err(e) => {
                let hint = error_hint(&e);
                tracing::warn!(error = %e, "codex_credential: get() failed");
                if let Ok(mut guard) = self.last_error.lock() {
                    *guard = Some(hint);
                }
                None
            }
        }
    }

    async fn invalidate(&self) -> bool {
        // Always set force_refresh so the next fetch() rotates via the refresh
        // token, even if `cached` was empty (e.g. the previous fetch failed
        // before populating it) or the JWT `exp` is still in the future. The
        // caller relies on a true return for retry; we return true whenever
        // force_refresh was newly set OR a cache existed.
        let mut state = self.inner.lock().await;
        let had_cache = state.cached.take().is_some();
        let was_already_forced = state.force_refresh;
        state.force_refresh = true;
        had_cache || !was_already_forced
    }

    async fn last_error_hint(&self) -> Option<String> {
        self.last_error.lock().ok().and_then(|g| g.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;

    fn fake_jwt(exp: i64) -> String {
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"none"}"#);
        let payload = URL_SAFE_NO_PAD.encode(format!(r#"{{"exp":{exp}}}"#).as_bytes());
        format!("{header}.{payload}.")
    }

    #[test]
    fn jwt_exp_extracts_exp_claim() {
        assert_eq!(jwt_exp(&fake_jwt(1_700_000_000)), Some(1_700_000_000));
    }

    #[test]
    fn jwt_exp_returns_none_for_garbage() {
        assert_eq!(jwt_exp("not.a.jwt"), None);
        assert_eq!(jwt_exp("nodot"), None);
    }

    #[test]
    fn read_auth_file_rejects_wrong_auth_mode() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        std::fs::write(&path, br#"{"auth_mode":"apikey","tokens":{"access_token":"x"}}"#).unwrap();
        let err = read_auth_file(&path).unwrap_err();
        assert!(matches!(err, CodexAuthError::WrongAuthMode { .. }));
    }

    #[test]
    fn read_auth_file_accepts_chatgpt_mode() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        std::fs::write(
            &path,
            br#"{"auth_mode":"chatgpt","tokens":{"access_token":"x","refresh_token":"r","account_id":"acc-1"}}"#,
        )
        .unwrap();
        let auth = read_auth_file(&path).unwrap();
        assert_eq!(auth.auth_mode.as_deref(), Some("chatgpt"));
        let tokens = auth.tokens.unwrap();
        assert_eq!(tokens.access_token, "x");
        assert_eq!(tokens.refresh_token.as_deref(), Some("r"));
        assert_eq!(tokens.account_id.as_deref(), Some("acc-1"));
    }

    #[test]
    fn write_auth_file_round_trips_and_preserves_unknown_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        let original = br#"{"auth_mode":"chatgpt","extra_field":"keep_me","tokens":{"access_token":"x","refresh_token":"r","account_id":"acc-1","custom":"k"}}"#;
        std::fs::write(&path, original).unwrap();
        let auth = read_auth_file(&path).unwrap();
        write_auth_file(&path, &auth).unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.contains("extra_field"));
        assert!(written.contains("keep_me"));
        assert!(written.contains("custom"));
    }

    #[tokio::test]
    async fn load_returns_account_id() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        let jwt = fake_jwt(now_unix() + 3600);
        let body = format!(
            r#"{{"auth_mode":"chatgpt","tokens":{{"access_token":"{jwt}","refresh_token":"r","account_id":"acc-xyz"}}}}"#
        );
        std::fs::write(&path, body).unwrap();
        let (_cred, account_id) = CodexCredential::load(path).unwrap();
        assert_eq!(account_id.as_deref(), Some("acc-xyz"));
    }

    #[tokio::test]
    async fn fetch_returns_cached_token_when_unexpired() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        let jwt = fake_jwt(now_unix() + 3600);
        let body = format!(
            r#"{{"auth_mode":"chatgpt","tokens":{{"access_token":"{jwt}","refresh_token":"r","account_id":"a"}}}}"#
        );
        std::fs::write(&path, body).unwrap();
        let (cred, _) = CodexCredential::load(path).unwrap();
        let token = cred.get().await.unwrap();
        assert_eq!(token, jwt);
    }

    #[tokio::test]
    async fn invalidate_clears_cache() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        let jwt = fake_jwt(now_unix() + 3600);
        let body = format!(
            r#"{{"auth_mode":"chatgpt","tokens":{{"access_token":"{jwt}","refresh_token":"r","account_id":"a"}}}}"#
        );
        std::fs::write(&path, body).unwrap();
        let (cred, _) = CodexCredential::load(path).unwrap();
        let _ = cred.get().await;
        assert!(cred.invalidate().await);
        // Subsequent invalidate is a no-op once cache is cleared and force_refresh already set.
        assert!(!cred.invalidate().await);
    }

    /// After `invalidate()` (e.g. on a 401), the next fetch must rotate via the
    /// refresh token even if the cached JWT's `exp` claim is still in the
    /// future — otherwise a server-side revocation would never get replaced.
    /// We observe the rotation attempt by removing the refresh_token from the
    /// file: the second `get()` should fail with `NoRefreshToken`, proving we
    /// reached the refresh branch instead of returning the (revoked) token.
    #[tokio::test]
    async fn invalidate_forces_refresh_even_if_jwt_still_fresh() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        let jwt = fake_jwt(now_unix() + 3600);
        let body = format!(
            r#"{{"auth_mode":"chatgpt","tokens":{{"access_token":"{jwt}","account_id":"a"}}}}"#
        );
        std::fs::write(&path, body).unwrap();
        let (cred, _) = CodexCredential::load(path).unwrap();

        // First call succeeds via the JWT-still-fresh shortcut.
        assert_eq!(cred.get().await.as_deref(), Some(jwt.as_str()));

        // After invalidate, force_refresh skips the shortcut and reaches the
        // refresh path, which fails because no refresh_token is present.
        cred.invalidate().await;
        assert!(cred.get().await.is_none());
        let err = cred.fetch().await.unwrap_err();
        assert!(matches!(err, CodexAuthError::NoRefreshToken(_)));
    }

    #[test]
    fn apply_refresh_response_rotates_all_three_tokens() {
        let mut auth = AuthFile {
            auth_mode: Some("chatgpt".into()),
            tokens: Some(AuthTokens {
                access_token: "old_at".into(),
                id_token: Some("old_it".into()),
                refresh_token: Some("old_rt".into()),
                account_id: Some("acc".into()),
                other: Default::default(),
            }),
            last_refresh: None,
            other: Default::default(),
        };
        let response = RefreshResponse {
            access_token: Some("new_at".into()),
            id_token: Some("new_it".into()),
            refresh_token: Some("new_rt".into()),
        };
        apply_refresh_response(&mut auth, response);
        let tokens = auth.tokens.unwrap();
        assert_eq!(tokens.access_token, "new_at");
        assert_eq!(tokens.id_token.as_deref(), Some("new_it"));
        assert_eq!(tokens.refresh_token.as_deref(), Some("new_rt"));
        // account_id is untouched by refresh.
        assert_eq!(tokens.account_id.as_deref(), Some("acc"));
        assert!(auth.last_refresh.is_some());
    }

    #[test]
    fn apply_refresh_response_preserves_omitted_fields() {
        let mut auth = AuthFile {
            auth_mode: Some("chatgpt".into()),
            tokens: Some(AuthTokens {
                access_token: "old_at".into(),
                id_token: Some("old_it".into()),
                refresh_token: Some("old_rt".into()),
                account_id: Some("acc".into()),
                other: Default::default(),
            }),
            last_refresh: None,
            other: Default::default(),
        };
        // Only access_token rotates; id_token and refresh_token preserved.
        let response = RefreshResponse {
            access_token: Some("new_at".into()),
            id_token: None,
            refresh_token: None,
        };
        apply_refresh_response(&mut auth, response);
        let tokens = auth.tokens.unwrap();
        assert_eq!(tokens.access_token, "new_at");
        assert_eq!(tokens.id_token.as_deref(), Some("old_it"));
        assert_eq!(tokens.refresh_token.as_deref(), Some("old_rt"));
    }

    #[test]
    fn apply_refresh_response_stamps_last_refresh_even_with_empty_response() {
        let mut auth = AuthFile {
            auth_mode: Some("chatgpt".into()),
            tokens: Some(AuthTokens {
                access_token: "x".into(),
                id_token: None,
                refresh_token: None,
                account_id: None,
                other: Default::default(),
            }),
            last_refresh: None,
            other: Default::default(),
        };
        apply_refresh_response(
            &mut auth,
            RefreshResponse {
                access_token: None,
                id_token: None,
                refresh_token: None,
            },
        );
        assert!(auth.last_refresh.is_some());
    }
}
