//! Authentication middleware and endpoints (REQ-AUTH-001 through REQ-AUTH-003)
//!
//! When `PHOENIX_PASSWORD` is set, all API requests require auth via cookie or
//! Bearer token. When unset, auth is bypassed entirely (backward compatible).

use axum::{
    body::Body,
    extract::State,
    http::{header, Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};

use super::AppState;

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
fn request_is_authenticated(req: &Request<Body>, password: &str) -> bool {
    // Check cookie first
    if let Some(cookie_header) = req.headers().get(header::COOKIE) {
        if let Ok(cookie_str) = cookie_header.to_str() {
            if let Some(cookie_value) = extract_cookie_value(cookie_str) {
                if constant_time_eq(cookie_value.as_bytes(), password.as_bytes()) {
                    return true;
                }
            }
        }
    }

    // Check Authorization: Bearer header
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
        || path.starts_with("/c/")
        || path.starts_with("/assets/")
        || path == "/service-worker.js"
        || path == "/phoenix.svg"
        || path == "/version"
    {
        return true;
    }

    // Share routes (Phase 2) — exempt so read-only shares work without auth
    if path.starts_with("/s/") {
        return true;
    }

    // Preview files (served by the static handler)
    if path.starts_with("/preview/") {
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
    if request_is_authenticated(&req, password) {
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
            let authenticated = request_is_authenticated(&req, password);
            Json(AuthStatusResponse {
                auth_required: true,
                authenticated,
            })
        }
    }
}

/// `POST /api/auth/login` — validate password and set an auth cookie on success.
pub async fn auth_login(State(state): State<AppState>, Json(body): Json<LoginRequest>) -> Response {
    let Some(password) = &state.password else {
        // Auth not required — login is a no-op success
        return (StatusCode::OK, Json(serde_json::json!({ "ok": true }))).into_response();
    };

    if !constant_time_eq(body.password.as_bytes(), password.as_bytes()) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": "Invalid password" })),
        )
            .into_response();
    }

    // Set cookie: HttpOnly, SameSite=Lax, 1-year expiry
    let cookie_value =
        format!("phoenix-auth={password}; Path=/; HttpOnly; SameSite=Lax; Max-Age=31536000");

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
        assert!(is_exempt_path("/c/some-slug"));
        assert!(is_exempt_path("/assets/index-abc.js"));
        assert!(is_exempt_path("/service-worker.js"));
        assert!(is_exempt_path("/phoenix.svg"));
        assert!(is_exempt_path("/version"));
        assert!(is_exempt_path("/api/auth/status"));
        assert!(is_exempt_path("/api/auth/login"));
        assert!(is_exempt_path("/s/share-token"));
        assert!(is_exempt_path("/preview/some/file.html"));

        assert!(!is_exempt_path("/api/conversations"));
        assert!(!is_exempt_path("/api/conversations/new"));
        assert!(!is_exempt_path("/api/models"));
        assert!(!is_exempt_path("/api/env"));
    }
}
