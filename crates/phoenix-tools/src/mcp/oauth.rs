//! OAuth 2.1 protocol layer for HTTP MCP servers (REQ-MCP-009..012).
//!
//! Pure protocol mechanics: `WWW-Authenticate` challenge parsing, Protected
//! Resource Metadata discovery (RFC 9728), Authorization Server Metadata
//! discovery (RFC 8414 + OpenID Connect Discovery), Dynamic Client
//! Registration (RFC 7591), PKCE, and the token-endpoint grants (code
//! exchange + refresh) with RFC 8707 resource indicators. The connection
//! lifecycle that drives these — when to discover, when to refresh, when to
//! re-prompt — lives in `mcp.rs`; persistence is behind the `OAuthStore`
//! trait so the manager is testable without a database.

use async_trait::async_trait;
use base64::Engine as _;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;

/// HTTP timeout for the OAuth flow's own requests (metadata fetches, DCR,
/// token grants). These are small JSON exchanges, never long-polls.
const OAUTH_HTTP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// `expires_at` for a token response that carries no `expires_in`: such a
/// token lives until revoked, so it is persisted as unexpired far beyond any
/// realistic process lifetime rather than given an invented short expiry that
/// would trigger needless refreshes.
const NO_EXPIRY_LIFETIME_SECS: i64 = 10 * 365 * 24 * 60 * 60;

fn now_unix() -> i64 {
    chrono::Utc::now().timestamp()
}

// ---------------------------------------------------------------------------
// Persistence boundary
// ---------------------------------------------------------------------------

/// A persisted OAuth client registration, keyed by authorization server
/// (REQ-MCP-010): MCP servers sharing one authorization server share it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthRegistrationRecord {
    pub auth_server: String,
    pub client_id: String,
    pub client_secret: Option<String>,
    pub token_endpoint_auth_method: String,
}

/// A persisted OAuth token for one MCP server, audience-bound to `resource`
/// (REQ-MCP-012). `expires_at` is unix seconds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthTokenRecord {
    pub server_name: String,
    pub resource: String,
    pub scopes: Vec<String>,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: i64,
}

impl OAuthTokenRecord {
    #[must_use]
    pub fn is_expired(&self) -> bool {
        now_unix() >= self.expires_at
    }
}

/// Persistence for OAuth registrations and tokens. The production impl is
/// backed by `phoenix-db`; tests (and a manager constructed without a
/// database) use [`MemoryOAuthStore`]. Errors are display strings — OAuth
/// store failures surface as connection errors, not panics.
#[async_trait]
pub trait OAuthStore: Send + Sync {
    async fn registration(
        &self,
        auth_server: &str,
    ) -> Result<Option<OAuthRegistrationRecord>, String>;
    async fn upsert_registration(&self, record: &OAuthRegistrationRecord) -> Result<(), String>;
    async fn token(&self, server_name: &str) -> Result<Option<OAuthTokenRecord>, String>;
    async fn upsert_token(&self, record: &OAuthTokenRecord) -> Result<(), String>;
    async fn delete_token(&self, server_name: &str) -> Result<(), String>;
}

/// In-memory [`OAuthStore`]: the default for a manager constructed without a
/// database, and the store the transport tests script against.
#[derive(Default)]
pub struct MemoryOAuthStore {
    registrations: std::sync::Mutex<HashMap<String, OAuthRegistrationRecord>>,
    tokens: std::sync::Mutex<HashMap<String, OAuthTokenRecord>>,
}

#[async_trait]
impl OAuthStore for MemoryOAuthStore {
    async fn registration(
        &self,
        auth_server: &str,
    ) -> Result<Option<OAuthRegistrationRecord>, String> {
        Ok(self.registrations.lock().unwrap().get(auth_server).cloned())
    }

    async fn upsert_registration(&self, record: &OAuthRegistrationRecord) -> Result<(), String> {
        self.registrations
            .lock()
            .unwrap()
            .insert(record.auth_server.clone(), record.clone());
        Ok(())
    }

    async fn token(&self, server_name: &str) -> Result<Option<OAuthTokenRecord>, String> {
        Ok(self.tokens.lock().unwrap().get(server_name).cloned())
    }

    async fn upsert_token(&self, record: &OAuthTokenRecord) -> Result<(), String> {
        self.tokens
            .lock()
            .unwrap()
            .insert(record.server_name.clone(), record.clone());
        Ok(())
    }

    async fn delete_token(&self, server_name: &str) -> Result<(), String> {
        self.tokens.lock().unwrap().remove(server_name);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// WWW-Authenticate challenge
// ---------------------------------------------------------------------------

/// Parse the parameters of a `WWW-Authenticate: Bearer ...` challenge into a
/// key→value map (`resource_metadata`, `scope`, `error`, ...). Handles quoted
/// and unquoted values; keys are lowercased. A non-Bearer or malformed header
/// yields an empty map — discovery then falls back to the well-known
/// locations rather than failing on a header it cannot read.
#[must_use]
pub fn parse_bearer_challenge(header: &str) -> HashMap<String, String> {
    let mut params = HashMap::new();
    let Some(rest) = header
        .trim_start()
        .strip_prefix("Bearer")
        .or_else(|| header.trim_start().strip_prefix("bearer"))
    else {
        return params;
    };

    let mut chars = rest.chars().peekable();
    loop {
        // Skip separators between parameters.
        while matches!(chars.peek(), Some(' ' | ',' | '\t')) {
            chars.next();
        }
        let mut key = String::new();
        while let Some(&c) = chars.peek() {
            if c == '=' {
                break;
            }
            if c == ',' {
                break;
            }
            key.push(c);
            chars.next();
        }
        if chars.peek().is_none() && key.trim().is_empty() {
            break;
        }
        if chars.next() != Some('=') {
            // A bare token without '=' (e.g. another scheme name); skip it.
            continue;
        }
        let mut value = String::new();
        if chars.peek() == Some(&'"') {
            chars.next();
            for c in chars.by_ref() {
                if c == '"' {
                    break;
                }
                value.push(c);
            }
        } else {
            while let Some(&c) = chars.peek() {
                if c == ',' || c == ' ' {
                    break;
                }
                value.push(c);
                chars.next();
            }
        }
        let key = key.trim().to_ascii_lowercase();
        if !key.is_empty() {
            params.insert(key, value);
        }
        if chars.peek().is_none() {
            break;
        }
    }
    params
}

/// Whether a 403 challenge is an `insufficient_scope` step-up request
/// (REQ-MCP-012) rather than a plain authorization denial.
#[must_use]
pub fn is_insufficient_scope_challenge(www_authenticate: &str) -> bool {
    parse_bearer_challenge(www_authenticate)
        .get("error")
        .is_some_and(|e| e == "insufficient_scope")
}

// ---------------------------------------------------------------------------
// Canonical resource URI (RFC 8707)
// ---------------------------------------------------------------------------

/// The canonical resource URI a token is audience-bound to: the MCP server's
/// URL with scheme and host lowercased, the default port and any fragment
/// dropped. Both the `resource` parameter on authorization/token requests and
/// the stored-token match at restore time use this form, so the comparison is
/// stable against cosmetic config edits while a real repoint still mismatches.
#[must_use]
pub fn canonical_resource(url: &str) -> String {
    let Ok(mut parsed) = reqwest::Url::parse(url) else {
        return url.to_string();
    };
    parsed.set_fragment(None);
    let scheme = parsed.scheme().to_ascii_lowercase();
    let host = parsed.host_str().unwrap_or_default().to_ascii_lowercase();
    let port = match (parsed.port(), scheme.as_str()) {
        (Some(443), "https") | (Some(80), "http") | (None, _) => String::new(),
        (Some(p), _) => format!(":{p}"),
    };
    let path = parsed.path().trim_end_matches('/');
    let query = parsed
        .query()
        .map(|q| format!("?{q}"))
        .unwrap_or_default();
    format!("{scheme}://{host}{port}{path}{query}")
}

// ---------------------------------------------------------------------------
// Discovery (RFC 9728 + RFC 8414 / OIDC)
// ---------------------------------------------------------------------------

/// Protected Resource Metadata (RFC 9728): which authorization server(s)
/// protect the MCP endpoint, and what scopes the resource understands.
#[derive(Debug, Clone)]
pub struct ProtectedResourceMetadata {
    pub authorization_servers: Vec<String>,
    pub scopes_supported: Vec<String>,
}

/// Authorization Server Metadata (RFC 8414 / OIDC discovery): the endpoints
/// the flow drives plus the capability flags it dispatches on.
#[derive(Debug, Clone)]
pub struct AuthServerMetadata {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub registration_endpoint: Option<String>,
    pub iss_parameter_supported: bool,
}

/// Build the HTTP client for one OAuth flow's requests (metadata fetches,
/// DCR, token grants).
///
/// # Errors
/// Returns a display string when the TLS backend cannot be initialized.
pub fn oauth_http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(OAUTH_HTTP_TIMEOUT)
        .build()
        .map_err(|e| format!("failed to build OAuth HTTP client: {e}"))
}

async fn fetch_json(client: &reqwest::Client, url: &str) -> Result<Value, String> {
    let response = client
        .get(url)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(|e| format!("GET {url}: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("GET {url}: HTTP {}", response.status()));
    }
    response
        .json()
        .await
        .map_err(|e| format!("GET {url}: invalid JSON: {e}"))
}

fn string_list(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The well-known URL candidates for a resource's Protected Resource
/// Metadata: the path-aware location for the endpoint path, then the host
/// root (RFC 9728 §3).
fn prm_well_known_candidates(mcp_url: &str) -> Vec<String> {
    let Ok(parsed) = reqwest::Url::parse(mcp_url) else {
        return Vec::new();
    };
    let origin = {
        let mut o = parsed.clone();
        o.set_path("");
        o.set_query(None);
        o.set_fragment(None);
        o.to_string().trim_end_matches('/').to_string()
    };
    let path = parsed.path().trim_end_matches('/');
    let mut candidates = Vec::new();
    if !path.is_empty() && path != "/" {
        candidates.push(format!("{origin}/.well-known/oauth-protected-resource{path}"));
    }
    candidates.push(format!("{origin}/.well-known/oauth-protected-resource"));
    candidates
}

/// Locate the Protected Resource Metadata for an MCP endpoint (REQ-MCP-009):
/// the `resource_metadata` URI from the 401's challenge when present,
/// otherwise the well-known locations.
///
/// # Errors
/// Returns a display string when no candidate yields a metadata document
/// naming at least one authorization server.
pub async fn fetch_protected_resource_metadata(
    client: &reqwest::Client,
    mcp_url: &str,
    challenge: &HashMap<String, String>,
) -> Result<ProtectedResourceMetadata, String> {
    let mut candidates = Vec::new();
    if let Some(from_challenge) = challenge.get("resource_metadata") {
        candidates.push(from_challenge.clone());
    }
    candidates.extend(prm_well_known_candidates(mcp_url));

    let mut errors = Vec::new();
    for candidate in &candidates {
        match fetch_json(client, candidate).await {
            Ok(doc) => {
                let authorization_servers = string_list(doc.get("authorization_servers"));
                if authorization_servers.is_empty() {
                    errors.push(format!("{candidate}: no authorization_servers"));
                    continue;
                }
                return Ok(ProtectedResourceMetadata {
                    authorization_servers,
                    scopes_supported: string_list(doc.get("scopes_supported")),
                });
            }
            Err(e) => errors.push(e),
        }
    }
    Err(format!(
        "protected resource metadata not found (RFC 9728): {}",
        errors.join("; ")
    ))
}

/// The metadata URL candidates for an authorization server issuer: RFC 8414
/// path-insertion, then OIDC discovery in both its path-insertion and
/// path-appending forms (REQ-MCP-009 requires trying both families).
fn as_metadata_candidates(issuer: &str) -> Vec<String> {
    let Ok(parsed) = reqwest::Url::parse(issuer) else {
        return Vec::new();
    };
    let origin = {
        let mut o = parsed.clone();
        o.set_path("");
        o.set_query(None);
        o.set_fragment(None);
        o.to_string().trim_end_matches('/').to_string()
    };
    let path = parsed.path().trim_end_matches('/');
    if path.is_empty() || path == "/" {
        vec![
            format!("{origin}/.well-known/oauth-authorization-server"),
            format!("{origin}/.well-known/openid-configuration"),
        ]
    } else {
        vec![
            format!("{origin}/.well-known/oauth-authorization-server{path}"),
            format!("{origin}/.well-known/openid-configuration{path}"),
            format!("{origin}{path}/.well-known/openid-configuration"),
        ]
    }
}

/// Fetch and validate the Authorization Server Metadata for `issuer`
/// (REQ-MCP-009), refusing an authorization server that does not advertise
/// S256 PKCE support (REQ-MCP-011) — better to fail here than after the
/// browser round trip.
///
/// # Errors
/// Returns a display string when no candidate yields usable metadata, the
/// advertised issuer mismatches, or PKCE support is absent.
pub async fn fetch_auth_server_metadata(
    client: &reqwest::Client,
    issuer: &str,
) -> Result<AuthServerMetadata, String> {
    let candidates = as_metadata_candidates(issuer);
    if candidates.is_empty() {
        return Err(format!("invalid authorization server URL '{issuer}'"));
    }

    let mut errors = Vec::new();
    for candidate in &candidates {
        let doc = match fetch_json(client, candidate).await {
            Ok(doc) => doc,
            Err(e) => {
                errors.push(e);
                continue;
            }
        };

        let advertised_issuer = doc.get("issuer").and_then(Value::as_str).unwrap_or("");
        if advertised_issuer.trim_end_matches('/') != issuer.trim_end_matches('/') {
            errors.push(format!(
                "{candidate}: issuer '{advertised_issuer}' does not match '{issuer}'"
            ));
            continue;
        }

        let (Some(authorization_endpoint), Some(token_endpoint)) = (
            doc.get("authorization_endpoint").and_then(Value::as_str),
            doc.get("token_endpoint").and_then(Value::as_str),
        ) else {
            errors.push(format!(
                "{candidate}: missing authorization_endpoint or token_endpoint"
            ));
            continue;
        };

        let pkce_methods = string_list(doc.get("code_challenge_methods_supported"));
        if !pkce_methods.iter().any(|m| m == "S256") {
            return Err(format!(
                "authorization server '{issuer}' does not advertise S256 in \
                 code_challenge_methods_supported; refusing to authorize without PKCE \
                 (OAuth 2.1)"
            ));
        }

        return Ok(AuthServerMetadata {
            issuer: advertised_issuer.to_string(),
            authorization_endpoint: authorization_endpoint.to_string(),
            token_endpoint: token_endpoint.to_string(),
            registration_endpoint: doc
                .get("registration_endpoint")
                .and_then(Value::as_str)
                .map(str::to_string),
            iss_parameter_supported: doc
                .get("authorization_response_iss_parameter_supported")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        });
    }
    Err(format!(
        "authorization server metadata not found for '{issuer}' (RFC 8414 / OIDC): {}",
        errors.join("; ")
    ))
}

// ---------------------------------------------------------------------------
// Client identity (RFC 7591)
// ---------------------------------------------------------------------------

/// Dynamically register a public client at the authorization server's
/// registration endpoint (REQ-MCP-010 fallback).
///
/// # Errors
/// Returns a display string when the endpoint rejects the registration or the
/// response carries no `client_id`.
pub async fn register_client(
    client: &reqwest::Client,
    metadata: &AuthServerMetadata,
    registration_endpoint: &str,
    redirect_uri: &str,
) -> Result<OAuthRegistrationRecord, String> {
    let body = serde_json::json!({
        "client_name": "Phoenix IDE",
        "redirect_uris": [redirect_uri],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none",
    });
    let response = client
        .post(registration_endpoint)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("dynamic client registration failed: {e}"))?;
    let status = response.status();
    let doc: Value = response.json().await.map_err(|e| {
        format!("dynamic client registration: invalid JSON response (HTTP {status}): {e}")
    })?;
    if !status.is_success() {
        return Err(format!(
            "dynamic client registration rejected (HTTP {status}): {}",
            doc.get("error_description")
                .or_else(|| doc.get("error"))
                .and_then(Value::as_str)
                .unwrap_or("unknown error")
        ));
    }
    let client_id = doc
        .get("client_id")
        .and_then(Value::as_str)
        .ok_or("dynamic client registration response missing client_id")?;
    Ok(OAuthRegistrationRecord {
        auth_server: metadata.issuer.clone(),
        client_id: client_id.to_string(),
        client_secret: doc
            .get("client_secret")
            .and_then(Value::as_str)
            .map(str::to_string),
        token_endpoint_auth_method: doc
            .get("token_endpoint_auth_method")
            .and_then(Value::as_str)
            .unwrap_or("none")
            .to_string(),
    })
}

// ---------------------------------------------------------------------------
// PKCE + authorization URL
// ---------------------------------------------------------------------------

pub struct PkcePair {
    pub verifier: String,
    pub challenge: String,
}

fn random_hex_256_bits() -> String {
    // Two v4 UUIDs = 2 × 122 bits of CSPRNG entropy, hex-encoded (valid PKCE
    // charset). uuid is already a dependency; no separate rand needed.
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

/// Generate a PKCE verifier and its S256 challenge (REQ-MCP-011).
#[must_use]
pub fn generate_pkce() -> PkcePair {
    let verifier = random_hex_256_bits();
    let digest = Sha256::digest(verifier.as_bytes());
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
    PkcePair {
        verifier,
        challenge,
    }
}

/// Generate the unguessable `state` nonce binding a callback to its pending
/// flow (REQ-MCP-011).
#[must_use]
pub fn generate_state_nonce() -> String {
    random_hex_256_bits()
}

/// Build the authorization-code URL the operator opens in a browser: PKCE
/// challenge, `state` nonce, RFC 8707 `resource` indicator, and the requested
/// scopes (REQ-MCP-011).
///
/// # Errors
/// Returns a display string when the authorization endpoint is not a valid
/// URL.
pub fn build_authorization_url(
    metadata: &AuthServerMetadata,
    registration: &OAuthRegistrationRecord,
    redirect_uri: &str,
    state_nonce: &str,
    pkce_challenge: &str,
    resource: &str,
    scopes: &[String],
) -> Result<String, String> {
    let mut url = reqwest::Url::parse(&metadata.authorization_endpoint)
        .map_err(|e| format!("invalid authorization_endpoint: {e}"))?;
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("response_type", "code");
        q.append_pair("client_id", &registration.client_id);
        q.append_pair("redirect_uri", redirect_uri);
        q.append_pair("state", state_nonce);
        q.append_pair("code_challenge", pkce_challenge);
        q.append_pair("code_challenge_method", "S256");
        q.append_pair("resource", resource);
        if !scopes.is_empty() {
            q.append_pair("scope", &scopes.join(" "));
        }
    }
    Ok(url.to_string())
}

// ---------------------------------------------------------------------------
// Token endpoint (code exchange + refresh)
// ---------------------------------------------------------------------------

/// A successful token-endpoint response, with `expires_in` already resolved
/// to an absolute expiry.
#[derive(Debug, Clone)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: i64,
    /// The granted scope set when the server returned one (RFC 6749 §5.1
    /// `scope`), otherwise `None` — the grant defaults to the requested set.
    pub scopes: Option<Vec<String>>,
}

/// Failure from a token-endpoint grant. The variant matters to the refresh
/// path: a definitive rejection discards the stored token (REQ-MCP-012),
/// while a transport failure is not evidence the token is stale.
#[derive(Debug)]
pub enum TokenGrantError {
    /// The authorization server answered and rejected the grant.
    Rejected(String),
    /// The request never completed (connect/timeout) or the response was
    /// unreadable.
    Transport(String),
}

impl std::fmt::Display for TokenGrantError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rejected(detail) | Self::Transport(detail) => write!(f, "{detail}"),
        }
    }
}

async fn token_grant(
    client: &reqwest::Client,
    token_endpoint: &str,
    registration: &OAuthRegistrationRecord,
    params: Vec<(&str, &str)>,
) -> Result<TokenResponse, TokenGrantError> {
    let mut form: Vec<(&str, &str)> = params;
    form.push(("client_id", &registration.client_id));
    let mut request = client.post(token_endpoint);
    if let Some(secret) = &registration.client_secret {
        if registration.token_endpoint_auth_method == "client_secret_basic" {
            request = request.basic_auth(&registration.client_id, Some(secret));
        } else {
            // client_secret_post and any unrecognized method carrying a
            // secret: send it in the body, the most widely accepted form.
            form.push(("client_secret", secret));
        }
    }
    let response = request
        .form(&form)
        .send()
        .await
        .map_err(|e| TokenGrantError::Transport(format!("token request failed: {e}")))?;
    let status = response.status();
    let doc: Value = response.json().await.map_err(|e| {
        TokenGrantError::Transport(format!(
            "token response unreadable (HTTP {status}): {e}"
        ))
    })?;
    if !status.is_success() {
        return Err(TokenGrantError::Rejected(format!(
            "token endpoint rejected the grant (HTTP {status}): {}",
            doc.get("error_description")
                .or_else(|| doc.get("error"))
                .and_then(Value::as_str)
                .unwrap_or("unknown error")
        )));
    }
    let access_token = doc
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            TokenGrantError::Rejected("token response missing access_token".to_string())
        })?;
    let expires_at = doc
        .get("expires_in")
        .and_then(Value::as_i64)
        .map_or(now_unix() + NO_EXPIRY_LIFETIME_SECS, |s| now_unix() + s);
    Ok(TokenResponse {
        access_token: access_token.to_string(),
        refresh_token: doc
            .get("refresh_token")
            .and_then(Value::as_str)
            .map(str::to_string),
        expires_at,
        scopes: doc
            .get("scope")
            .and_then(Value::as_str)
            .map(|s| s.split_whitespace().map(str::to_string).collect()),
    })
}

/// Exchange an authorization code (plus PKCE verifier and RFC 8707 resource
/// indicator) for tokens (REQ-MCP-011).
///
/// # Errors
/// Returns a [`TokenGrantError`] when the grant fails.
pub async fn exchange_code(
    client: &reqwest::Client,
    token_endpoint: &str,
    registration: &OAuthRegistrationRecord,
    redirect_uri: &str,
    code: &str,
    pkce_verifier: &str,
    resource: &str,
) -> Result<TokenResponse, TokenGrantError> {
    token_grant(
        client,
        token_endpoint,
        registration,
        vec![
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("code_verifier", pkce_verifier),
            ("resource", resource),
        ],
    )
    .await
}

/// Exchange a refresh token for a new access token (REQ-MCP-012), with the
/// RFC 8707 resource indicator keeping the replacement audience-bound.
///
/// # Errors
/// Returns a [`TokenGrantError`] when the grant fails.
pub async fn refresh_grant(
    client: &reqwest::Client,
    token_endpoint: &str,
    registration: &OAuthRegistrationRecord,
    refresh_token: &str,
    resource: &str,
) -> Result<TokenResponse, TokenGrantError> {
    token_grant(
        client,
        token_endpoint,
        registration,
        vec![
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("resource", resource),
        ],
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearer_challenge_parses_quoted_and_unquoted_params() {
        let params = parse_bearer_challenge(
            "Bearer realm=\"mcp\", resource_metadata=\"https://h/.well-known/oauth-protected-resource/mcp\", scope=\"a b\", error=insufficient_scope",
        );
        assert_eq!(params.get("realm").map(String::as_str), Some("mcp"));
        assert_eq!(
            params.get("resource_metadata").map(String::as_str),
            Some("https://h/.well-known/oauth-protected-resource/mcp")
        );
        assert_eq!(params.get("scope").map(String::as_str), Some("a b"));
        assert_eq!(
            params.get("error").map(String::as_str),
            Some("insufficient_scope")
        );
        assert!(is_insufficient_scope_challenge(
            "Bearer error=insufficient_scope, scope=\"x\""
        ));
        assert!(!is_insufficient_scope_challenge("Bearer realm=\"mcp\""));
    }

    #[test]
    fn bearer_challenge_tolerates_other_schemes_and_garbage() {
        assert!(parse_bearer_challenge("Basic realm=\"x\"").is_empty());
        assert!(parse_bearer_challenge("").is_empty());
        let params = parse_bearer_challenge("Bearer");
        assert!(params.is_empty());
    }

    #[test]
    fn canonical_resource_normalizes_case_port_and_fragment() {
        assert_eq!(
            canonical_resource("HTTPS://Example.COM:443/MCP/#frag"),
            "https://example.com/MCP"
        );
        assert_eq!(
            canonical_resource("http://host:8080/mcp"),
            "http://host:8080/mcp"
        );
        assert_eq!(canonical_resource("https://host"), "https://host");
        // Path case is preserved — only scheme/host are case-insensitive.
        assert_eq!(
            canonical_resource("https://host/Path?x=1"),
            "https://host/Path?x=1"
        );
    }

    #[test]
    fn prm_candidates_prefer_path_aware_location() {
        assert_eq!(
            prm_well_known_candidates("https://h.example/mcp"),
            vec![
                "https://h.example/.well-known/oauth-protected-resource/mcp",
                "https://h.example/.well-known/oauth-protected-resource",
            ]
        );
        assert_eq!(
            prm_well_known_candidates("https://h.example/"),
            vec!["https://h.example/.well-known/oauth-protected-resource"]
        );
    }

    #[test]
    fn as_metadata_candidates_cover_8414_and_oidc_forms() {
        assert_eq!(
            as_metadata_candidates("https://as.example"),
            vec![
                "https://as.example/.well-known/oauth-authorization-server",
                "https://as.example/.well-known/openid-configuration",
            ]
        );
        assert_eq!(
            as_metadata_candidates("https://as.example/tenant"),
            vec![
                "https://as.example/.well-known/oauth-authorization-server/tenant",
                "https://as.example/.well-known/openid-configuration/tenant",
                "https://as.example/tenant/.well-known/openid-configuration",
            ]
        );
    }

    #[test]
    fn pkce_challenge_is_s256_of_verifier() {
        let pair = generate_pkce();
        assert!(pair.verifier.len() >= 43 && pair.verifier.len() <= 128);
        let digest = Sha256::digest(pair.verifier.as_bytes());
        let expected = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
        assert_eq!(pair.challenge, expected);
        // Two flows never share a verifier or nonce.
        assert_ne!(generate_pkce().verifier, pair.verifier);
        assert_ne!(generate_state_nonce(), generate_state_nonce());
    }

    #[test]
    fn authorization_url_carries_pkce_state_resource_and_scopes() {
        let metadata = AuthServerMetadata {
            issuer: "https://as.example".to_string(),
            authorization_endpoint: "https://as.example/authorize".to_string(),
            token_endpoint: "https://as.example/token".to_string(),
            registration_endpoint: None,
            iss_parameter_supported: true,
        };
        let registration = OAuthRegistrationRecord {
            auth_server: "https://as.example".to_string(),
            client_id: "cid".to_string(),
            client_secret: None,
            token_endpoint_auth_method: "none".to_string(),
        };
        let url = build_authorization_url(
            &metadata,
            &registration,
            "http://localhost:8031/api/mcp/oauth/callback",
            "nonce",
            "challenge",
            "https://mcp.example/mcp",
            &["read".to_string(), "write".to_string()],
        )
        .expect("url");
        let parsed = reqwest::Url::parse(&url).expect("valid url");
        let q: HashMap<_, _> = parsed.query_pairs().into_owned().collect();
        assert_eq!(q.get("response_type").map(String::as_str), Some("code"));
        assert_eq!(q.get("client_id").map(String::as_str), Some("cid"));
        assert_eq!(q.get("state").map(String::as_str), Some("nonce"));
        assert_eq!(
            q.get("code_challenge").map(String::as_str),
            Some("challenge")
        );
        assert_eq!(
            q.get("code_challenge_method").map(String::as_str),
            Some("S256")
        );
        assert_eq!(
            q.get("resource").map(String::as_str),
            Some("https://mcp.example/mcp")
        );
        assert_eq!(q.get("scope").map(String::as_str), Some("read write"));
    }
}
