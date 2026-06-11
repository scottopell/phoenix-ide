//! Authentication middleware and endpoints (REQ-AUTH-001 through REQ-AUTH-003)
//!
//! When `PHOENIX_PASSWORD` is set, all API requests require auth. Browsers
//! authenticate with an opaque random **session token** minted at login and
//! held server-side in [`SessionStore`]; the password itself never travels in a
//! cookie. API clients may still present the password directly via
//! `Authorization: Bearer <password>`. When `PHOENIX_PASSWORD` is unset, auth is
//! bypassed entirely (backward compatible).

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::{
    body::Body,
    extract::{ConnectInfo, State},
    http::{header, Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use base64::Engine;
use rand::Rng;
use serde::{Deserialize, Serialize};

use super::AppState;

/// Server-side set of valid session tokens. A successful login mints a random
/// token, inserts it here, and hands it to the browser in the `phoenix-auth`
/// cookie. Each subsequent request is authenticated by membership, so the
/// password never leaves the server in a cookie and tokens are independently
/// revocable. Tokens are process-lifetime (cleared on restart).
#[derive(Clone, Default)]
pub struct SessionStore {
    tokens: Arc<Mutex<HashSet<String>>>,
}

impl SessionStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Mint a fresh 256-bit random session token and record it as valid.
    fn mint(&self) -> String {
        let mut bytes = [0u8; 32];
        rand::rng().fill_bytes(&mut bytes);
        let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
        self.tokens
            .lock()
            .expect("session store poisoned")
            .insert(token.clone());
        token
    }

    /// Constant-time-ish membership check: a token is valid iff it was minted by
    /// this process and not yet evicted.
    fn is_valid(&self, token: &str) -> bool {
        self.tokens
            .lock()
            .expect("session store poisoned")
            .contains(token)
    }
}

/// In-memory per-client login throttle. Counts consecutive failed logins and
/// locks a client out for a back-off window once the threshold is exceeded. The
/// counter resets on a successful login. Keyed on the connection's real peer IP
/// (`ConnectInfo<SocketAddr>`), so a directly-reachable deployment cannot be
/// induced to mint a fresh bucket per request by varying client-controlled
/// headers. Forwarded headers are trusted only when an operator opts in via
/// `PHOENIX_TRUST_PROXY` (see [`throttle_key`]).
#[derive(Clone, Default)]
pub struct LoginThrottle {
    inner: Arc<Mutex<HashMap<String, AttemptRecord>>>,
}

#[derive(Clone)]
struct AttemptRecord {
    failures: u32,
    locked_until: Option<Instant>,
}

/// Failures allowed before lockout engages.
const MAX_FAILURES: u32 = 5;
/// Lockout window once the threshold is crossed.
const LOCKOUT_WINDOW: Duration = Duration::from_secs(60);

impl LoginThrottle {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `true` if `key` is currently locked out and the attempt should be
    /// rejected without checking the password.
    fn is_locked(&self, key: &str) -> bool {
        let mut guard = self.inner.lock().expect("login throttle poisoned");
        match guard.get_mut(key) {
            Some(rec) => match rec.locked_until {
                Some(until) if Instant::now() < until => true,
                Some(_) => {
                    // Lockout window elapsed — reset and allow a fresh attempt.
                    rec.failures = 0;
                    rec.locked_until = None;
                    false
                }
                None => false,
            },
            None => false,
        }
    }

    /// Record a failed attempt, engaging a lockout once the threshold is crossed.
    fn record_failure(&self, key: &str) {
        let mut guard = self.inner.lock().expect("login throttle poisoned");
        let rec = guard.entry(key.to_string()).or_insert(AttemptRecord {
            failures: 0,
            locked_until: None,
        });
        rec.failures = rec.failures.saturating_add(1);
        if rec.failures >= MAX_FAILURES {
            rec.locked_until = Some(Instant::now() + LOCKOUT_WINDOW);
        }
    }

    /// Clear a client's failure record after a successful login.
    fn record_success(&self, key: &str) {
        self.inner
            .lock()
            .expect("login throttle poisoned")
            .remove(key);
    }
}

/// Whether forwarded client-IP headers (`X-Forwarded-For` / `X-Real-IP`) may be
/// trusted as the throttle identity. Off unless the operator sets
/// `PHOENIX_TRUST_PROXY` to a truthy value (`1`/`true`/`on`/`yes`), asserting
/// that a trusted reverse proxy sets/overwrites those headers. On a
/// directly-reachable deployment these headers are fully client-controlled, so
/// trusting them by default lets an attacker mint a fresh throttle bucket per
/// request and bypass the lockout entirely.
fn trust_forwarded_headers() -> bool {
    matches!(
        std::env::var("PHOENIX_TRUST_PROXY")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "on" | "yes"
    )
}

/// Throttle identity for a login attempt.
///
/// The authoritative key is the connection's real peer IP (`peer`, from
/// `ConnectInfo<SocketAddr>`), which a client cannot spoof on a direct TCP/TLS
/// connection. Forwarded headers are consulted **only** when the operator has
/// opted in via `PHOENIX_TRUST_PROXY` (a trusted proxy is then responsible for
/// setting/overwriting them); in that mode the first `X-Forwarded-For` hop, then
/// `X-Real-IP`, take precedence over the peer (which is the proxy's address).
///
/// `peer` is `None` only if `ConnectInfo` was not injected by the serve loop
/// (it is wired through both the plain and TLS paths); such requests fall back
/// to a shared `"direct"` bucket. Header trust never applies without the opt-in,
/// so an unauthenticated client on a direct deployment cannot vary headers to
/// escape its peer-keyed bucket.
fn throttle_key(req_headers: &header::HeaderMap, peer: Option<SocketAddr>) -> String {
    if trust_forwarded_headers() {
        if let Some(xff) = req_headers.get("x-forwarded-for") {
            if let Ok(s) = xff.to_str() {
                if let Some(first) = s.split(',').next() {
                    let first = first.trim();
                    if first.parse::<IpAddr>().is_ok() {
                        return first.to_string();
                    }
                }
            }
        }
        if let Some(xri) = req_headers.get("x-real-ip") {
            if let Ok(s) = xri.to_str() {
                if s.parse::<IpAddr>().is_ok() {
                    return s.to_string();
                }
            }
        }
    }

    match peer {
        Some(addr) => addr.ip().to_string(),
        None => "direct".to_string(),
    }
}

/// Constant-time string comparison to prevent timing attacks on password checks.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let result = a
        .iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y));
    result == 0
}

/// Extract the `phoenix-auth` cookie value from a Cookie header.
fn extract_cookie_value(cookie_header: &str) -> Option<&str> {
    for cookie in cookie_header.split(';') {
        let cookie = cookie.trim();
        if let Some(value) = cookie.strip_prefix("phoenix-auth=") {
            return Some(value);
        }
    }
    None
}

/// Check whether a request carries a valid auth credential.
///
/// The `phoenix-auth` cookie must hold a valid **session token** (minted at
/// login, held in [`SessionStore`]) — never the password. The
/// `Authorization: Bearer` header carries the password directly, for API
/// clients that don't run the cookie login flow.
fn request_is_authenticated(req: &Request<Body>, password: &str, sessions: &SessionStore) -> bool {
    // Cookie carries an opaque session token.
    if let Some(cookie_header) = req.headers().get(header::COOKIE) {
        if let Ok(cookie_str) = cookie_header.to_str() {
            if let Some(cookie_value) = extract_cookie_value(cookie_str) {
                if sessions.is_valid(cookie_value) {
                    return true;
                }
            }
        }
    }

    // Bearer header carries the password directly (API clients).
    if let Some(auth_header) = req.headers().get(header::AUTHORIZATION) {
        if let Ok(auth_str) = auth_header.to_str() {
            if let Some(token) = auth_str.strip_prefix("Bearer ") {
                if constant_time_eq(token.as_bytes(), password.as_bytes()) {
                    return true;
                }
            }
        }
    }

    false
}

/// Returns true if the request path is exempt from auth.
fn is_exempt_path(path: &str) -> bool {
    // Auth endpoints must be accessible without auth
    if path == "/api/auth/status" || path == "/api/auth/login" {
        return true;
    }

    // Static assets: SPA routes, JS/CSS bundles, images, service worker, favicon
    if path == "/"
        || path == "/new"
        || path == "/codex/login"
        || path == "/about"
        || path.starts_with("/c/")
        || path.starts_with("/assets/")
        || path == "/service-worker.js"
        || path == "/phoenix.svg"
        || path == "/version"
    {
        return true;
    }

    // Share routes — exempt so read-only shares work without auth
    // /s/{token} serves the share page, /api/share/{token}/* serves share API
    if path.starts_with("/s/") || path.starts_with("/api/share/") {
        return true;
    }

    // The MCP OAuth redirect arrives as a bare browser GET from the
    // authorization server and cannot carry Phoenix credentials. It is safe
    // unauthenticated: the only meaningful inputs are `code` + `state`, and
    // the unguessable `state` nonce binds the request to a pending flow
    // (REQ-MCP-011) — without it the callback is rejected.
    if path == "/api/mcp/oauth/callback" {
        return true;
    }

    false
}

/// Axum middleware that enforces password auth when `PHOENIX_PASSWORD` is set.
pub async fn auth_middleware(
    State(state): State<AppState>,
    req: Request<Body>,
    next: Next,
) -> Response {
    // No password configured — pass through (no auth required)
    let Some(password) = &state.password else {
        return next.run(req).await;
    };

    // Exempt paths don't require auth
    if is_exempt_path(req.uri().path()) {
        return next.run(req).await;
    }

    // Check credentials
    if request_is_authenticated(&req, password, &state.sessions) {
        return next.run(req).await;
    }

    // Reject unauthenticated request
    (
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({ "error": "Authentication required" })),
    )
        .into_response()
}

// ---- Auth endpoints ----

#[derive(Serialize)]
pub struct AuthStatusResponse {
    pub auth_required: bool,
    pub authenticated: bool,
}

#[derive(Deserialize)]
pub struct LoginRequest {
    pub password: String,
}

/// `GET /api/auth/status` — report whether auth is required and whether the
/// caller is currently authenticated.
pub async fn auth_status(
    State(state): State<AppState>,
    req: Request<Body>,
) -> Json<AuthStatusResponse> {
    match &state.password {
        None => Json(AuthStatusResponse {
            auth_required: false,
            authenticated: true,
        }),
        Some(password) => {
            let authenticated = request_is_authenticated(&req, password, &state.sessions);
            Json(AuthStatusResponse {
                auth_required: true,
                authenticated,
            })
        }
    }
}

/// `POST /api/auth/login` — validate the password, mint a session token, and
/// set it in an auth cookie on success. Rate-limited per client (see
/// [`LoginThrottle`]).
pub async fn auth_login(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    req_headers: header::HeaderMap,
    Json(body): Json<LoginRequest>,
) -> Response {
    let Some(password) = &state.password else {
        // Auth not required — login is a no-op success
        return (StatusCode::OK, Json(serde_json::json!({ "ok": true }))).into_response();
    };

    let key = throttle_key(&req_headers, Some(peer));

    // Reject locked-out clients before touching the password.
    if state.login_throttle.is_locked(&key) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({ "error": "Too many login attempts; try again later" })),
        )
            .into_response();
    }

    if !constant_time_eq(body.password.as_bytes(), password.as_bytes()) {
        state.login_throttle.record_failure(&key);
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": "Invalid password" })),
        )
            .into_response();
    }

    state.login_throttle.record_success(&key);

    // Mint an opaque session token; the password never enters the cookie.
    let token = state.sessions.mint();

    // `Secure` only when the server terminates TLS — sending it over plain HTTP
    // would make the cookie undeliverable and silently break login.
    let secure = if state.deployment.tls.enabled {
        "; Secure"
    } else {
        ""
    };
    let cookie_value =
        format!("phoenix-auth={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age=31536000{secure}");

    (
        StatusCode::OK,
        [(header::SET_COOKIE, cookie_value)],
        Json(serde_json::json!({ "ok": true })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_eq_works() {
        assert!(constant_time_eq(b"hello", b"hello"));
        assert!(!constant_time_eq(b"hello", b"world"));
        assert!(!constant_time_eq(b"hello", b"hell"));
        assert!(!constant_time_eq(b"", b"a"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn extract_cookie_parses_correctly() {
        assert_eq!(
            extract_cookie_value("phoenix-auth=secret123"),
            Some("secret123")
        );
        assert_eq!(
            extract_cookie_value("other=val; phoenix-auth=mypass; more=stuff"),
            Some("mypass")
        );
        assert_eq!(extract_cookie_value("other=val; unrelated=x"), None);
        assert_eq!(extract_cookie_value(""), None);
    }

    #[test]
    fn exempt_paths_are_correct() {
        assert!(is_exempt_path("/"));
        assert!(is_exempt_path("/new"));
        assert!(is_exempt_path("/codex/login"));
        assert!(is_exempt_path("/about"));
        assert!(is_exempt_path("/c/some-slug"));
        assert!(is_exempt_path("/assets/index-abc.js"));
        assert!(is_exempt_path("/service-worker.js"));
        assert!(is_exempt_path("/phoenix.svg"));
        assert!(is_exempt_path("/version"));
        assert!(is_exempt_path("/api/auth/status"));
        assert!(is_exempt_path("/api/auth/login"));
        assert!(is_exempt_path("/s/share-token"));

        assert!(!is_exempt_path("/api/conversations"));
        assert!(!is_exempt_path("/api/conversations/new"));
        assert!(!is_exempt_path("/api/models"));
        assert!(!is_exempt_path("/api/env"));
        // Preview is NOT auth-exempt: it serves on-disk files and must sit
        // behind auth. The same-origin sandboxed iframe carries the cookie.
        assert!(!is_exempt_path("/preview/some/file.html"));
    }

    #[test]
    fn session_token_is_opaque_and_never_the_password() {
        let store = SessionStore::new();
        let password = "hunter2";
        let token = store.mint();

        // The minted token must not be the password — the whole point of the
        // scheme is that the cookie carries an opaque credential.
        assert_ne!(token, password);
        // A minted token validates; an arbitrary string (e.g. the password) does
        // not, because only minted tokens are members of the store.
        assert!(store.is_valid(&token));
        assert!(!store.is_valid(password));
        assert!(!store.is_valid("not-a-real-token"));
    }

    #[test]
    fn session_tokens_are_unique_per_mint() {
        let store = SessionStore::new();
        let a = store.mint();
        let b = store.mint();
        assert_ne!(a, b);
        assert!(store.is_valid(&a));
        assert!(store.is_valid(&b));
    }

    #[test]
    fn cookie_holding_a_valid_session_token_authenticates() {
        let sessions = SessionStore::new();
        let token = sessions.mint();
        let req = Request::builder()
            .header(header::COOKIE, format!("phoenix-auth={token}"))
            .body(Body::empty())
            .unwrap();
        assert!(request_is_authenticated(&req, "the-password", &sessions));
    }

    #[test]
    fn cookie_holding_the_raw_password_is_rejected() {
        // Old scheme set the cookie to the password itself. The new scheme must
        // reject a cookie that carries the password — only session tokens count.
        let sessions = SessionStore::new();
        let req = Request::builder()
            .header(header::COOKIE, "phoenix-auth=the-password")
            .body(Body::empty())
            .unwrap();
        assert!(!request_is_authenticated(&req, "the-password", &sessions));
    }

    #[test]
    fn bearer_password_still_authenticates_for_api_clients() {
        let sessions = SessionStore::new();
        let req = Request::builder()
            .header(header::AUTHORIZATION, "Bearer the-password")
            .body(Body::empty())
            .unwrap();
        assert!(request_is_authenticated(&req, "the-password", &sessions));
    }

    #[test]
    fn login_throttle_locks_out_after_threshold() {
        let throttle = LoginThrottle::new();
        let key = "1.2.3.4";
        assert!(!throttle.is_locked(key));
        for _ in 0..MAX_FAILURES {
            assert!(!throttle.is_locked(key));
            throttle.record_failure(key);
        }
        // Threshold crossed — the client is now locked out.
        assert!(throttle.is_locked(key));
        // A successful login clears the lockout.
        throttle.record_success(key);
        assert!(!throttle.is_locked(key));
    }

    /// Serialize tests that mutate the process-global `PHOENIX_TRUST_PROXY`
    /// env var so they don't race each other under the parallel test runner.
    static TRUST_PROXY_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn peer(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    /// By default (no trusted-proxy opt-in) the throttle key is the real peer
    /// IP and forwarded headers are ignored entirely. This is the security
    /// property: on a directly-reachable deployment a client cannot vary
    /// `X-Forwarded-For` / `X-Real-IP` to escape its peer-keyed bucket.
    #[test]
    fn throttle_key_uses_peer_and_ignores_forwarded_headers_by_default() {
        let _guard = TRUST_PROXY_ENV_LOCK.lock().unwrap();
        // SAFETY: env mutation guarded by TRUST_PROXY_ENV_LOCK for this test.
        unsafe { std::env::remove_var("PHOENIX_TRUST_PROXY") };

        // A spoofed XFF is ignored; the key is the peer IP.
        let mut headers = header::HeaderMap::new();
        headers.insert("x-forwarded-for", "203.0.113.7, 10.0.0.1".parse().unwrap());
        assert_eq!(
            throttle_key(&headers, Some(peer("198.51.100.9:54321"))),
            "198.51.100.9"
        );

        // X-Real-IP is likewise ignored without the opt-in.
        let mut headers = header::HeaderMap::new();
        headers.insert("x-real-ip", "203.0.113.7".parse().unwrap());
        assert_eq!(
            throttle_key(&headers, Some(peer("198.51.100.9:1234"))),
            "198.51.100.9"
        );

        // No peer info (ConnectInfo absent) falls back to the shared bucket.
        assert_eq!(throttle_key(&header::HeaderMap::new(), None), "direct");
    }

    /// The bypass-is-closed regression: two login attempts that present
    /// DIFFERENT spoofed `X-Forwarded-For` values from the SAME peer must land
    /// in the SAME throttle bucket (both count toward lockout). Before the fix
    /// each spoofed header minted a fresh bucket, defeating the lockout.
    #[test]
    fn spoofed_forwarded_headers_share_one_bucket_by_default() {
        let _guard = TRUST_PROXY_ENV_LOCK.lock().unwrap();
        // SAFETY: env mutation guarded by TRUST_PROXY_ENV_LOCK for this test.
        unsafe { std::env::remove_var("PHOENIX_TRUST_PROXY") };

        let attacker = peer("198.51.100.42:5555");

        let mut h1 = header::HeaderMap::new();
        h1.insert("x-forwarded-for", "1.1.1.1".parse().unwrap());
        let mut h2 = header::HeaderMap::new();
        h2.insert("x-forwarded-for", "2.2.2.2".parse().unwrap());

        let k1 = throttle_key(&h1, Some(attacker));
        let k2 = throttle_key(&h2, Some(attacker));
        assert_eq!(k1, k2, "varying XFF must not mint a fresh bucket");
        assert_eq!(k1, "198.51.100.42");

        // And drive it through the throttle to prove both count toward lockout.
        let throttle = LoginThrottle::new();
        for _ in 0..MAX_FAILURES {
            assert!(!throttle.is_locked(&k1));
            // Alternate the spoofed header each attempt — same peer, same key.
            throttle.record_failure(&throttle_key(
                if rand::random() { &h1 } else { &h2 },
                Some(attacker),
            ));
        }
        assert!(
            throttle.is_locked(&k2),
            "attacker is locked out despite varying X-Forwarded-For"
        );
    }

    /// With `PHOENIX_TRUST_PROXY` set, forwarded headers regain precedence over
    /// the peer (which is now the trusted proxy's address): first XFF hop, then
    /// X-Real-IP, then peer fallback.
    #[test]
    fn throttle_key_trusts_forwarded_headers_when_opted_in() {
        let _guard = TRUST_PROXY_ENV_LOCK.lock().unwrap();
        // SAFETY: env mutation guarded by TRUST_PROXY_ENV_LOCK for this test.
        unsafe { std::env::set_var("PHOENIX_TRUST_PROXY", "1") };

        let proxy = Some(peer("10.0.0.1:443"));

        let mut headers = header::HeaderMap::new();
        headers.insert("x-forwarded-for", "203.0.113.7, 10.0.0.1".parse().unwrap());
        assert_eq!(throttle_key(&headers, proxy), "203.0.113.7");

        let mut headers = header::HeaderMap::new();
        headers.insert("x-real-ip", "198.51.100.4".parse().unwrap());
        assert_eq!(throttle_key(&headers, proxy), "198.51.100.4");

        // A non-IP forwarding value is ignored; fall back to the peer (proxy) IP.
        let mut headers = header::HeaderMap::new();
        headers.insert("x-forwarded-for", "not-an-ip".parse().unwrap());
        assert_eq!(throttle_key(&headers, proxy), "10.0.0.1");

        // No forwarding header at all — peer (proxy) IP.
        assert_eq!(throttle_key(&header::HeaderMap::new(), proxy), "10.0.0.1");

        // SAFETY: restore default for other tests; guarded by the lock.
        unsafe { std::env::remove_var("PHOENIX_TRUST_PROXY") };
    }
}
