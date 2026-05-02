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
fn write_auth_file(path: &PathBuf, auth: &AuthFile) -> Result<(), CodexAuthError> {
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(auth).map_err(|err| CodexAuthError::ParseAuthFile {
        path: path.clone(),
        err,
    })?;
    std::fs::write(&tmp, bytes).map_err(|err| CodexAuthError::Io {
        path: tmp.clone(),
        err,
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(&tmp, perms).map_err(|err| CodexAuthError::Io {
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
        return Err(CodexAuthError::RefreshFailed(format!(
            "HTTP {status}: {text}"
        )));
    }

    serde_json::from_str(&text)
        .map_err(|e| CodexAuthError::RefreshFailed(format!("response parse: {e} body={text}")))
}

#[derive(Debug, Clone)]
struct CachedToken {
    access_token: String,
    /// `exp` from the JWT, in seconds since epoch. May be `None` if unparseable —
    /// in that case we always treat it as expired and re-read the file.
    exp: Option<i64>,
}

/// `CredentialSource` impl for `ChatGPT` `OAuth` tokens borrowed from the `Codex` CLI.
pub struct CodexCredential {
    auth_path: PathBuf,
    cached: Mutex<Option<CachedToken>>,
    /// `account_id` cached from the most recent file read. Read once at construction
    /// and refreshed lazily; the Codex CLI writes it during `codex login` and it
    /// rarely changes during a session.
    account_id: StdMutex<Option<String>>,
}

impl std::fmt::Debug for CodexCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodexCredential")
            .field("auth_path", &self.auth_path)
            .field("cached", &"[redacted]")
            .field(
                "account_id",
                &self.account_id.lock().ok().map(|g| g.is_some()),
            )
            .finish()
    }
}

impl CodexCredential {
    pub fn new(auth_path: PathBuf) -> Self {
        Self {
            auth_path,
            cached: Mutex::new(None),
            account_id: StdMutex::new(None),
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

    /// Acquire a valid access token, refreshing if needed. This is the all-in-one
    /// path that `get()` calls; broken out so it can return a typed error for
    /// logging while `get()` itself returns `Option<String>` per trait.
    async fn fetch(&self) -> Result<String, CodexAuthError> {
        // Fast path: cached token still good.
        {
            let cached = self.cached.lock().await;
            if let Some(c) = cached.as_ref() {
                if let Some(exp) = c.exp {
                    if now_unix() < exp - REFRESH_SKEW_SECS {
                        return Ok(c.access_token.clone());
                    }
                }
            }
        }

        // Slow path: re-read file. The CLI may have refreshed for us; if so, use what's there.
        let mut auth = read_auth_file(&self.auth_path)?;
        let tokens = auth
            .tokens
            .as_mut()
            .ok_or_else(|| CodexAuthError::NoAccessToken(self.auth_path.clone()))?;

        // Update cached account_id from file (covers re-login during session).
        if let Ok(mut guard) = self.account_id.lock() {
            (*guard).clone_from(&tokens.account_id);
        }

        let exp = jwt_exp(&tokens.access_token);
        if let Some(exp_unix) = exp {
            if now_unix() < exp_unix - REFRESH_SKEW_SECS {
                let token = tokens.access_token.clone();
                *self.cached.lock().await = Some(CachedToken {
                    access_token: token.clone(),
                    exp: Some(exp_unix),
                });
                return Ok(token);
            }
        }

        // Refresh.
        let refresh_token = tokens
            .refresh_token
            .clone()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| CodexAuthError::NoRefreshToken(self.auth_path.clone()))?;

        tracing::info!("codex_credential: refreshing access token");
        let new_tokens = refresh_tokens(&refresh_token).await?;

        if let Some(at) = new_tokens.access_token {
            tokens.access_token = at;
        }
        if let Some(it) = new_tokens.id_token {
            tokens.id_token = Some(it);
        }
        if let Some(rt) = new_tokens.refresh_token {
            tokens.refresh_token = Some(rt);
        }
        auth.last_refresh = Some(now_iso8601_utc());

        write_auth_file(&self.auth_path, &auth)?;

        let new_token = auth
            .tokens
            .as_ref()
            .map(|t| t.access_token.clone())
            .unwrap_or_default();
        let new_exp = jwt_exp(&new_token);
        *self.cached.lock().await = Some(CachedToken {
            access_token: new_token.clone(),
            exp: new_exp,
        });
        Ok(new_token)
    }
}

#[async_trait::async_trait]
impl CredentialSource for CodexCredential {
    async fn get(&self) -> Option<String> {
        match self.fetch().await {
            Ok(token) => Some(token),
            Err(e) => {
                tracing::warn!(error = %e, "codex_credential: get() failed");
                None
            }
        }
    }

    async fn invalidate(&self) -> bool {
        let mut cached = self.cached.lock().await;
        if cached.is_some() {
            *cached = None;
            true
        } else {
            false
        }
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
        assert!(!cred.invalidate().await); // already cleared
    }
}
