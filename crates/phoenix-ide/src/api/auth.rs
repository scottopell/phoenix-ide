//! Authentication middleware and endpoints (REQ-AUTH-001 through REQ-AUTH-003)
//!
//! When `PHOENIX_PASSWORD` is set, all API requests require auth. Browsers
//! authenticate with an opaque random **session token** minted at login and
//! held server-side in [`SessionStore`]; the password itself never travels in a
//! cookie. API clients may still present the password directly via
//! `Authorization: Bearer <password>`. When `PHOENIX_PASSWORD` is unset, auth is
//! bypassed entirely (backward compatible).

use std::collections::HashMap;
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

/// Lifetime of a minted session token. Drives both the `expires_at` persisted
/// with the token and the `Max-Age` advertised on the `phoenix-auth` cookie, so
/// the two can never disagree.
const SESSION_TTL_SECS: i64 = 31_536_000; // 1 year

/// Non-reversible fingerprint of a configured password, stored alongside each
/// session token so that rotating `PHOENIX_PASSWORD` invalidates every session
/// minted under the old one. SHA-256, base64-encoded; the password is never
/// stored.
pub fn password_fingerprint(password: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(password.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

/// Server-side store of valid session tokens, backed by the `auth_sessions`
/// table. A successful login mints a random token, persists it here, and hands
/// it to the browser in the `phoenix-auth` cookie. Each subsequent request is
/// authenticated by membership, so the password never leaves the server in a
/// cookie and tokens are independently revocable. Persistence means tokens
/// survive a server restart — a redeploy no longer logs everyone out.
///
/// Each token is bound to [`password_fingerprint`] of the password it was
/// minted under; validation requires it to match the current password's
/// fingerprint, so a password rotation invalidates all prior sessions.
#[derive(Clone)]
pub struct SessionStore {
    db: crate::db::Database,
    password_fingerprint: String,
}

impl SessionStore {
    pub fn new(db: crate::db::Database, password_fingerprint: String) -> Self {
        Self {
            db,
            password_fingerprint,
        }
    }

    /// Mint a fresh 256-bit random session token and persist it as valid.
    /// Opportunistically reaps expired rows so the table cannot grow unbounded.
    async fn mint(&self) -> Result<String, crate::db::DbError> {
        let mut bytes = [0u8; 32];
        rand::rng().fill_bytes(&mut bytes);
        let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
        if let Err(e) = self.db.delete_expired_auth_sessions().await {
            tracing::warn!(error = %e, "failed to reap expired auth sessions; continuing");
        }
        self.db
            .insert_auth_session(
                &token,
                &self.password_fingerprint,
                chrono::Duration::seconds(SESSION_TTL_SECS),
            )
            .await?;
        Ok(token)
    }

    /// A token is valid iff it was minted previously, has not expired, and was
    /// minted under the current password. A DB error fails closed (treated as
    /// invalid) and is logged — an unreachable store must never silently
    /// authenticate.
    async fn is_valid(&self, token: &str) -> bool {
        match self
            .db
            .is_auth_session_valid(token, &self.password_fingerprint)
            .await
        {
            Ok(valid) => valid,
            Err(e) => {
                tracing::warn!(error = %e, "auth session lookup failed; treating token as invalid");
                false
            }
        }
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

/// Whether the request carries a valid **session-cookie** credential.
///
/// The `phoenix-auth` cookie must hold a valid session token (minted at login,
/// held in [`SessionStore`]) — never the password. Cookie auth is **never**
/// throttled: it proves prior possession of the password, so a legitimate
/// browser user must never be locked out by an attacker brute-forcing Bearer
/// guesses from the same peer IP.
/// Takes `&HeaderMap` rather than `&Request<Body>` deliberately: `Body` is not
/// `Sync`, so a `&Request<Body>` borrow held across the `.await` below would make
/// the enclosing handler future non-`Send` and fail axum's `Handler` bound.
/// `HeaderMap` is `Sync`, so a borrow of it is safe to hold across the await.
async fn cookie_is_valid(headers: &header::HeaderMap, sessions: &SessionStore) -> bool {
    let token = headers
        .get(header::COOKIE)
        .and_then(|h| h.to_str().ok())
        .and_then(extract_cookie_value);
    match token {
        Some(token) => sessions.is_valid(token).await,
        None => false,
    }
}

/// Extract the `Authorization: Bearer <token>` value, if present and parseable.
///
/// A `Some` result means the client is presenting a Bearer **password guess** —
/// the unit subject to throttling. `None` means no Bearer credential was offered
/// (anonymous traffic), which must never consume throttle budget.
fn bearer_token(req: &Request<Body>) -> Option<&str> {
    req.headers()
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

/// Outcome of validating a Bearer-password credential against the throttle.
enum BearerCheck {
    /// No Bearer credential was offered — throttle budget untouched.
    Absent,
    /// Bearer matched the password — counter cleared (like a login success).
    Valid,
    /// Bearer was wrong — a failure was recorded against `key`.
    Invalid,
    /// Peer is locked out; the guess was rejected without comparison.
    LockedOut,
}

/// Validate a Bearer-password guess against the shared per-peer [`LoginThrottle`].
///
/// This is the throttled counterpart to [`cookie_is_valid`]: the password-equality
/// brute-force oracle exposed by `auth_status` and `auth_middleware` is closed by
/// routing every Bearer check through the same per-IP budget that gates
/// `/api/auth/login`. A correct Bearer clears the counter; a wrong one records a
/// failure; once locked out, further guesses are rejected without comparing.
fn check_bearer_password(
    req: &Request<Body>,
    password: &str,
    throttle: &LoginThrottle,
    key: &str,
) -> BearerCheck {
    let Some(token) = bearer_token(req) else {
        return BearerCheck::Absent;
    };

    if throttle.is_locked(key) {
        return BearerCheck::LockedOut;
    }

    if constant_time_eq(token.as_bytes(), password.as_bytes()) {
        throttle.record_success(key);
        BearerCheck::Valid
    } else {
        throttle.record_failure(key);
        BearerCheck::Invalid
    }
}

/// Read the connection's real peer IP from request extensions, where the serve
/// loop injects it (`into_make_service_with_connect_info` on the plain path, an
/// `Extension(ConnectInfo(..))` layer on the TLS path). `None` when absent —
/// [`throttle_key`] then falls back to the shared `"direct"` bucket.
fn peer_from_extensions(req: &Request<Body>) -> Option<SocketAddr> {
    req.extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ci| ci.0)
}

/// Returns true if the request path is exempt from auth.
fn is_exempt_path(path: &str) -> bool {
    // Auth endpoints must be accessible without auth
    if path == "/api/auth/status" || path == "/api/auth/login" {
        return true;
    }

    // Public static assets — the same bundle for every user, no secrets.
    if path.starts_with("/assets/")
        || path == "/service-worker.js"
        || path == "/phoenix.svg"
        || path == "/version"
    {
        return true;
    }

    // SPA client routes (`/`, `/new`, `/c/:slug`, …). Exempting them serves the
    // public shell so a direct load / refresh / bookmark renders the in-app
    // login screen instead of 401ing before React mounts. The set comes from
    // the same `SPA_ROUTES` the router registers (see `api::spa_routes`), so the
    // router and this exemption cannot drift — the recurring 404/401 bug class.
    if super::spa_routes::is_spa_route(path) {
        return true;
    }

    // Share routes — exempt so read-only shares work without auth
    // /s/{token} serves the share page, /api/share/{token}/* serves share API
    if path.starts_with("/s/") || path.starts_with("/api/share/") {
        return true;
    }

    // Command suggestion is gated by its own scoped capability token
    // (PHOENIX_SUGGEST_TOKEN), checked inside the handler — not the master
    // password. Exempt it from the password middleware so the in-terminal
    // `phx`, which holds the token but not the password, can reach it.
    if path == "/api/suggest" {
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

    // Session cookie wins and is never throttled — a legitimate browser must
    // not be locked out by Bearer brute-force from the same peer IP.
    if cookie_is_valid(req.headers(), &state.sessions).await {
        return next.run(req).await;
    }

    // Bearer-password validation is throttled per peer IP, sharing the single
    // login budget, so this 200/401 oracle cannot be used for unlimited guesses.
    let key = throttle_key(req.headers(), peer_from_extensions(&req));
    match check_bearer_password(&req, password, &state.login_throttle, &key) {
        BearerCheck::Valid => next.run(req).await,
        BearerCheck::LockedOut => (
            StatusCode::TOO_MANY_REQUESTS,
            Json(
                serde_json::json!({ "error": "Too many authentication attempts; try again later" }),
            ),
        )
            .into_response(),
        BearerCheck::Invalid | BearerCheck::Absent => (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": "Authentication required" })),
        )
            .into_response(),
    }
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
            // Cookie auth is authoritative and never throttled.
            let authenticated = if cookie_is_valid(req.headers(), &state.sessions).await {
                true
            } else {
                // This endpoint is auth-exempt, so its Bearer check is an
                // unauthenticated oracle for password equality. Route it through
                // the shared per-peer throttle: once locked out, further guesses
                // report `authenticated: false` without comparing.
                let key = throttle_key(req.headers(), peer_from_extensions(&req));
                matches!(
                    check_bearer_password(&req, password, &state.login_throttle, &key),
                    BearerCheck::Valid
                )
            };
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
    let token = match state.sessions.mint().await {
        Ok(token) => token,
        Err(e) => {
            tracing::error!(error = %e, "failed to persist session token at login");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "Could not create session" })),
            )
                .into_response();
        }
    };

    // `Secure` only when the server terminates TLS — sending it over plain HTTP
    // would make the cookie undeliverable and silently break login.
    let secure = if state.deployment.tls.enabled {
        "; Secure"
    } else {
        ""
    };
    let cookie_value = format!(
        "phoenix-auth={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={SESSION_TTL_SECS}{secure}"
    );

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
        // Direct loads of the home terminal and chain deep links must serve the
        // SPA shell, not 401 — these mirror the serve_spa routes in handlers.rs.
        assert!(is_exempt_path("/terminal"));
        assert!(is_exempt_path("/chains/root-conv-id"));
        assert!(is_exempt_path("/codex/login"));
        assert!(is_exempt_path("/about"));
        assert!(is_exempt_path("/usage"));
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

    /// CBC guard: every route the router serves as the SPA shell must also be
    /// auth-exempt, so a direct load renders the login screen rather than
    /// 401ing. Both derive from `SPA_ROUTES`; this pins that the auth side
    /// actually consumes it (a refactor dropping the `is_spa_route` call fails
    /// here, not in production behind a password).
    #[test]
    fn every_spa_route_is_auth_exempt() {
        use super::super::spa_routes::{SpaRoute, SPA_ROUTES};
        for route in SPA_ROUTES {
            let sample = match route {
                SpaRoute::Exact(p) => (*p).to_string(),
                SpaRoute::Param { prefix, .. } => format!("{prefix}sample-param"),
            };
            assert!(
                is_exempt_path(&sample),
                "SPA route {sample} is served by the router but not auth-exempt"
            );
        }
    }

    async fn test_store() -> SessionStore {
        let db = crate::db::Database::open_in_memory()
            .await
            .expect("in-memory db");
        SessionStore::new(db, password_fingerprint("test-password"))
    }

    #[tokio::test]
    async fn session_token_is_opaque_and_never_the_password() {
        let store = test_store().await;
        let password = "hunter2";
        let token = store.mint().await.unwrap();

        // The minted token must not be the password — the whole point of the
        // scheme is that the cookie carries an opaque credential.
        assert_ne!(token, password);
        // A minted token validates; an arbitrary string (e.g. the password) does
        // not, because only minted tokens are members of the store.
        assert!(store.is_valid(&token).await);
        assert!(!store.is_valid(password).await);
        assert!(!store.is_valid("not-a-real-token").await);
    }

    #[tokio::test]
    async fn session_tokens_are_unique_per_mint() {
        let store = test_store().await;
        let a = store.mint().await.unwrap();
        let b = store.mint().await.unwrap();
        assert_ne!(a, b);
        assert!(store.is_valid(&a).await);
        assert!(store.is_valid(&b).await);
    }

    /// A token minted by one `SessionStore` validates through a second store
    /// over the *same* database — the persistence guarantee that makes sessions
    /// survive a process restart (a redeploy no longer logs everyone out).
    #[tokio::test]
    async fn session_token_survives_a_fresh_store_over_the_same_db() {
        let db = crate::db::Database::open_in_memory()
            .await
            .expect("in-memory db");
        let fp = password_fingerprint("pw");
        let token = SessionStore::new(db.clone(), fp.clone())
            .mint()
            .await
            .unwrap();
        // A brand-new store (as if the process restarted) under the same
        // password sees the same token.
        assert!(SessionStore::new(db, fp).is_valid(&token).await);
    }

    /// Rotating the configured password invalidates tokens minted under the old
    /// one: a fresh store with a different password fingerprint over the same DB
    /// rejects the prior token. This restores the pre-persistence behaviour
    /// where a restart cleared the in-memory store.
    #[tokio::test]
    async fn session_token_is_rejected_after_password_rotation() {
        let db = crate::db::Database::open_in_memory()
            .await
            .expect("in-memory db");
        let token = SessionStore::new(db.clone(), password_fingerprint("old"))
            .mint()
            .await
            .unwrap();
        let rotated = SessionStore::new(db, password_fingerprint("new"));
        assert!(!rotated.is_valid(&token).await);
    }

    #[tokio::test]
    async fn cookie_holding_a_valid_session_token_authenticates() {
        let sessions = test_store().await;
        let token = sessions.mint().await.unwrap();
        let req = Request::builder()
            .header(header::COOKIE, format!("phoenix-auth={token}"))
            .body(Body::empty())
            .unwrap();
        assert!(cookie_is_valid(req.headers(), &sessions).await);
    }

    #[tokio::test]
    async fn cookie_holding_the_raw_password_is_rejected() {
        // Old scheme set the cookie to the password itself. The new scheme must
        // reject a cookie that carries the password — only session tokens count.
        let sessions = test_store().await;
        let req = Request::builder()
            .header(header::COOKIE, "phoenix-auth=the-password")
            .body(Body::empty())
            .unwrap();
        assert!(!cookie_is_valid(req.headers(), &sessions).await);
    }

    /// A correct Bearer password authenticates an API client and, as a side
    /// effect, clears any accumulated failures for that peer.
    #[test]
    fn bearer_password_still_authenticates_for_api_clients() {
        let throttle = LoginThrottle::new();
        let req = Request::builder()
            .header(header::AUTHORIZATION, "Bearer the-password")
            .body(Body::empty())
            .unwrap();
        assert!(matches!(
            check_bearer_password(&req, "the-password", &throttle, "1.2.3.4"),
            BearerCheck::Valid
        ));
    }

    fn bearer_req(token: &str) -> Request<Body> {
        Request::builder()
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap()
    }

    /// (a) Repeated wrong-Bearer guesses from one peer lock that peer out, and a
    /// subsequent *correct* Bearer is rejected (without comparison) while the
    /// lockout window is active — closing the unlimited brute-force oracle.
    #[test]
    fn wrong_bearer_locks_out_and_correct_bearer_rejected_while_locked() {
        let throttle = LoginThrottle::new();
        let key = "10.0.0.5";

        for _ in 0..MAX_FAILURES {
            assert!(matches!(
                check_bearer_password(&bearer_req("wrong"), "the-password", &throttle, key),
                BearerCheck::Invalid
            ));
        }

        // Even the correct password is now rejected without comparison.
        assert!(matches!(
            check_bearer_password(&bearer_req("the-password"), "the-password", &throttle, key),
            BearerCheck::LockedOut
        ));
    }

    /// (b) A valid session cookie authenticates regardless of throttle state:
    /// cookie auth never consults the throttle, so a locked-out peer's browser
    /// session still works.
    #[tokio::test]
    async fn valid_cookie_authenticates_during_lockout() {
        let throttle = LoginThrottle::new();
        let sessions = test_store().await;
        let key = "10.0.0.6";

        // Drive the peer into lockout via Bearer guesses.
        for _ in 0..MAX_FAILURES {
            check_bearer_password(&bearer_req("wrong"), "the-password", &throttle, key);
        }
        assert!(throttle.is_locked(key));

        // A valid cookie from that same peer still authenticates — cookie auth
        // is independent of the throttle.
        let token = sessions.mint().await.unwrap();
        let req = Request::builder()
            .header(header::COOKIE, format!("phoenix-auth={token}"))
            .body(Body::empty())
            .unwrap();
        assert!(cookie_is_valid(req.headers(), &sessions).await);
    }

    /// (c) A correct Bearer *before* the threshold clears the failure counter,
    /// so accumulated near-misses don't strand a legitimate API client.
    #[test]
    fn correct_bearer_clears_counter_before_lockout() {
        let throttle = LoginThrottle::new();
        let key = "10.0.0.7";

        // One short of lockout.
        for _ in 0..(MAX_FAILURES - 1) {
            check_bearer_password(&bearer_req("wrong"), "the-password", &throttle, key);
        }
        assert!(!throttle.is_locked(key));

        // A correct Bearer clears the counter.
        assert!(matches!(
            check_bearer_password(&bearer_req("the-password"), "the-password", &throttle, key),
            BearerCheck::Valid
        ));

        // The budget is fully reset: a fresh full run of failures is needed to
        // lock out again, proving the counter was cleared rather than merely not
        // incremented.
        for _ in 0..(MAX_FAILURES - 1) {
            assert!(matches!(
                check_bearer_password(&bearer_req("wrong"), "the-password", &throttle, key),
                BearerCheck::Invalid
            ));
        }
        assert!(!throttle.is_locked(key));
    }

    /// (d) Anonymous / no-credential traffic never consumes throttle budget:
    /// a request with neither cookie nor Bearer leaves the failure counter at
    /// zero, so unauthenticated noise can't help (or hurt) lockout state.
    #[test]
    fn anonymous_traffic_does_not_consume_throttle_budget() {
        let throttle = LoginThrottle::new();
        let key = "10.0.0.8";

        // Far more than MAX_FAILURES anonymous (no-Bearer) requests.
        for _ in 0..(MAX_FAILURES * 4) {
            let req = Request::builder().body(Body::empty()).unwrap();
            assert!(matches!(
                check_bearer_password(&req, "the-password", &throttle, key),
                BearerCheck::Absent
            ));
        }

        // Never locked out — anonymous traffic recorded no failures.
        assert!(!throttle.is_locked(key));

        // And the full Bearer budget is still available afterward.
        for _ in 0..(MAX_FAILURES - 1) {
            assert!(matches!(
                check_bearer_password(&bearer_req("wrong"), "the-password", &throttle, key),
                BearerCheck::Invalid
            ));
        }
        assert!(!throttle.is_locked(key));
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
