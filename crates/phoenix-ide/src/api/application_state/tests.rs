use super::*;
use crate::api::create_router;
use crate::db::Database;
use axum::{body::Body, http::Request, Router};
use phoenix_db::{ContinueOutcome, ConvState};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use tower::ServiceExt;

struct Fixture {
    _dir: tempfile::TempDir,
    path: PathBuf,
    original: Vec<u8>,
    db: Database,
    app: Router,
    product_id: String,
    coordinator_references: Vec<String>,
    coordinator_id: String,
}

impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("observation.db");
        let db = Database::open(path.to_str().unwrap()).await.unwrap();
        phoenix_db::run_pending_migrations(db.pool()).await.unwrap();
        let ordinary = db
            .create_conversation("ordinary", "ordinary-slug", "/fixture", true, None, None)
            .await
            .unwrap();
        let coordinator = db
            .get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
            .await
            .unwrap();
        db.update_conversation_state(
            &coordinator.id,
            &ConvState::ContextExhausted {
                summary: "handoff".into(),
            },
        )
        .await
        .unwrap();
        let successor = match db.continue_conversation(&coordinator.id).await.unwrap() {
            ContinueOutcome::Created(row) => row,
            other @ (ContinueOutcome::AlreadyContinued(_)
            | ContinueOutcome::ParentNotContextExhausted { .. }) => {
                panic!("unexpected continuation: {other:?}")
            }
        };
        let coordinator_references = vec![
            coordinator.id,
            coordinator.slug.unwrap(),
            successor.id.clone(),
            successor.slug.unwrap(),
        ];
        let (busy, frames, checkpointed): (i64, i64, i64) =
            sqlx::query_as("PRAGMA wal_checkpoint(TRUNCATE)")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(busy, 0);
        assert_eq!(frames, checkpointed);
        db.pool().close().await;
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&path, permissions).unwrap();
        let db = Database::open_read_only(path.to_str().unwrap())
            .await
            .unwrap();
        let original = std::fs::read(&path).unwrap();
        let application = ApplicationState::Observation(ObservationApplication {
            routes: ConversationRouteObservations::new(db.clone()),
            access_policy: ObservationAccessPolicy::LoopbackUnauthenticated,
        });
        Self {
            _dir: dir,
            path,
            original,
            db,
            app: create_router(application),
            product_id: ordinary.product_conversation_id.to_string(),
            coordinator_references,
            coordinator_id: successor.id,
        }
    }

    fn assert_unchanged(&self) {
        assert_eq!(std::fs::read(&self.path).unwrap(), self.original);
        for suffix in ["-wal", "-shm", "-journal"] {
            assert!(!Path::new(&format!("{}{suffix}", self.path.display())).exists());
        }
    }

    async fn finish(self) {
        self.assert_unchanged();
        self.db.pool().close().await;
        self.assert_unchanged();
    }
}

fn loopback_request(method: &str, path: &str, body: Body) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .body(body)
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(
        "127.0.0.1:12345".parse::<SocketAddr>().unwrap(),
    ));
    request
}

async fn response_json(app: &Router, request: Request<Body>) -> (StatusCode, Value) {
    let context = format!("{} {}", request.method(), request.uri());
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&body).unwrap_or_else(|error| {
            panic!(
                "{context}: {status}: {error}; body={:?}",
                String::from_utf8_lossy(&body)
            )
        }),
    )
}

#[tokio::test]
async fn full_router_observes_immutable_id_slug_product_and_coordinator_routes() {
    let fixture = Fixture::new().await;
    for path in [
        "/api/conversations/ordinary/route",
        "/api/conversations/by-slug/ordinary-slug/route",
    ] {
        let (status, body) =
            response_json(&fixture.app, loopback_request("GET", path, Body::empty())).await;
        assert_eq!(status, StatusCode::OK, "{path}");
        assert_eq!(body, json!({"id": "ordinary", "slug": "ordinary-slug"}));
    }
    for reference in ["ordinary", "ordinary-slug", &fixture.product_id] {
        let path = format!("/api/product-conversations/{reference}/route");
        let (status, body) =
            response_json(&fixture.app, loopback_request("GET", &path, Body::empty())).await;
        assert_eq!(status, StatusCode::OK, "{path}");
        assert_eq!(body, json!({"transcript_row_id": "ordinary"}));
    }
    for reference in &fixture.coordinator_references {
        let path = format!("/api/global/coordinator/route/{reference}");
        let (status, body) =
            response_json(&fixture.app, loopback_request("GET", &path, Body::empty())).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, json!({"coordinator_id": fixture.coordinator_id}));
    }
    for reference in ["ordinary", "ordinary-slug", "missing"] {
        let path = format!("/api/global/coordinator/route/{reference}");
        let (status, body) =
            response_json(&fixture.app, loopback_request("GET", &path, Body::empty())).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, json!({"coordinator_id": null}));
    }
    for path in [
        "/api/conversations/missing/route",
        "/api/conversations/by-slug/missing/route",
        "/api/product-conversations/missing/route",
    ] {
        let (status, _) =
            response_json(&fixture.app, loopback_request("GET", path, Body::empty())).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
    }
    fixture.finish().await;
}

#[tokio::test]
async fn observation_denies_live_routes_before_malformed_body_and_websocket_extraction() {
    let fixture = Fixture::new().await;
    for (method, path, content_type) in [
        (
            "POST",
            "/api/conversations/ordinary/chat",
            "application/json",
        ),
        (
            "POST",
            "/api/conversations/ordinary/attachments",
            "multipart/form-data",
        ),
        (
            "POST",
            "/api/conversations/new/with-attachments",
            "multipart/form-data",
        ),
        (
            "POST",
            "/api/product-conversations/new",
            "multipart/form-data",
        ),
        (
            "GET",
            "/api/conversations/ordinary/terminal",
            "application/json",
        ),
        ("GET", "/api/terminal/global", "application/json"),
        (
            "GET",
            "/api/conversations/ordinary/browser-view",
            "application/json",
        ),
        ("GET", "/api/credential-helper/run", "application/json"),
        (
            "POST",
            "/api/credential-helper/invalidate",
            "application/json",
        ),
        (
            "GET",
            "/api/conversations/ordinary/stream",
            "application/json",
        ),
        ("POST", "/api/auth/login", "application/json"),
        ("POST", "/api/files/reveal", "application/json"),
        (
            "POST",
            "/api/telemetry/conversation-open",
            "application/json",
        ),
        ("POST", "/api/federation/enrollments", "application/json"),
        ("GET", "/api/list-directory?path=%FF", "application/json"),
        ("GET", "/api/models", "application/json"),
        ("GET", "/api/version", "application/json"),
        ("GET", "/api/share/invalid/conversation", "application/json"),
        (
            "GET",
            "/api/mcp/oauth/callback?code=invalid",
            "application/json",
        ),
    ] {
        let mut request = loopback_request(method, path, Body::from("malformed"));
        request
            .headers_mut()
            .insert("content-type", content_type.parse().unwrap());
        let (status, body) = response_json(&fixture.app, request).await;
        assert_eq!(
            status,
            StatusCode::SERVICE_UNAVAILABLE,
            "{method} {path}: {body}"
        );
        assert_eq!(
            body,
            json!({
                "error_type": "capability_unavailable",
                "capability": "live_application",
                "error": "Live application capability is unavailable"
            }),
            "{method} {path}"
        );
        fixture.assert_unchanged();
    }
    // A body that panics when polled proves admission does not read uploads/JSON.
    for path in [
        "/api/conversations/ordinary/chat",
        "/api/conversations/ordinary/attachments",
    ] {
        let body = Body::from_stream(futures::stream::poll_fn(
            |_| -> std::task::Poll<Option<Result<axum::body::Bytes, std::io::Error>>> {
                panic!("unavailable route polled request body")
            },
        ));
        let mut request = loopback_request("POST", path, body);
        request
            .headers_mut()
            .insert("content-type", "application/json".parse().unwrap());
        let (status, _) = response_json(&fixture.app, request).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    }
    fixture.finish().await;
}

#[tokio::test]
async fn observation_requires_real_loopback_peer_even_for_auth_and_public_ui() {
    let fixture = Fixture::new().await;
    for path in [
        "/api/auth/status",
        "/",
        "/api/conversations/ordinary/route",
        "/api/credential-helper/run",
    ] {
        for peer in [None, Some("192.0.2.1:12345"), Some("[2001:db8::1]:12345")] {
            let mut request = Request::builder()
                .uri(path)
                .header("x-forwarded-for", "127.0.0.1")
                .body(Body::empty())
                .unwrap();
            if let Some(peer) = peer {
                request
                    .extensions_mut()
                    .insert(ConnectInfo(peer.parse::<SocketAddr>().unwrap()));
            }
            let (status, body) = response_json(&fixture.app, request).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{path} {peer:?}");
            assert_eq!(body["error_type"], "observation_access_denied");
        }
    }
    for path in [
        "/api/auth/status",
        "/",
        "/assets/index.js",
        "/api/conversations/ordinary/route",
        "/api/credential-helper/run",
    ] {
        for (header, value) in [
            ("x-forwarded-for", "192.0.2.1"),
            ("x-forwarded-for", ""),
            ("x-forwarded-for", "invalid"),
            ("x-forwarded-for", "127.0.0.1, 192.0.2.1"),
            ("x-forwarded-for", "127.0.0.1"),
            ("forwarded", "for=192.0.2.1"),
        ] {
            let mut request = loopback_request("GET", path, Body::empty());
            request.headers_mut().insert(
                axum::http::HeaderName::from_static(header),
                value.parse().unwrap(),
            );
            let (status, body) = response_json(&fixture.app, request).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{path} {header}: {value}");
            assert_eq!(body["error_type"], "observation_access_denied");
        }
    }
    for peer in ["127.0.0.1:12345", "[::1]:12345"] {
        let mut request = Request::builder()
            .uri("/api/auth/status")
            .body(Body::empty())
            .unwrap();
        request
            .extensions_mut()
            .insert(ConnectInfo(peer.parse::<SocketAddr>().unwrap()));
        let (status, body) = response_json(&fixture.app, request).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, json!({"auth_required": false, "authenticated": true}));
    }
    let response = fixture
        .app
        .clone()
        .oneshot(loopback_request("GET", "/", Body::empty()))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!response.headers().contains_key("set-cookie"));
    fixture.finish().await;
}

struct LiveAuthFixture {
    _dir: tempfile::TempDir,
    state: AppState,
    app: Router,
}

impl LiveAuthFixture {
    async fn new(password: Option<&str>) -> Self {
        use phoenix_core::runtime_env::PhoenixRuntimeEnvironment;
        use phoenix_llm::{LlmConfig, ModelRegistry};
        use std::sync::Arc;

        let dir = tempfile::tempdir().unwrap();
        let runtime_env = Arc::new(PhoenixRuntimeEnvironment::with_root(dir.path()));
        let db = Database::open_in_memory().await.unwrap();
        phoenix_db::run_pending_migrations(db.pool()).await.unwrap();
        let registry = Arc::new(ModelRegistry::new(&LlmConfig {
            anthropic_api_key: Some(String::new()),
            runtime_env: runtime_env.clone(),
            ..LlmConfig::default()
        }));
        let state = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            AppState::new(
                db,
                registry,
                crate::platform::PlatformCapability::None {
                    details: "live auth regression".into(),
                },
                Arc::new(crate::tools::mcp::McpClientManager::new()),
                None,
                password.map(str::to_owned),
                Arc::new(crate::api::DeploymentConfig::for_tests()),
                runtime_env,
                String::new(),
            ),
        )
        .await
        .expect("bounded normal live startup")
        .unwrap();
        state
            .db
            .create_conversation(
                "live-auth-row",
                "live-auth-slug",
                dir.path().to_str().unwrap(),
                true,
                None,
                None,
            )
            .await
            .unwrap();
        let app = create_router(ApplicationState::Live(state.clone()));
        Self {
            _dir: dir,
            state,
            app,
        }
    }

    async fn login_cookie(&self, password: &str) -> String {
        let mut request = loopback_request(
            "POST",
            "/api/auth/login",
            Body::from(json!({"password": password}).to_string()),
        );
        request
            .headers_mut()
            .insert("content-type", "application/json".parse().unwrap());
        let response = self.app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let set_cookie = response.headers()["set-cookie"].to_str().unwrap();
        assert!(set_cookie.contains("; Path=/"));
        assert!(set_cookie.contains("; HttpOnly"));
        assert!(set_cookie.contains("; SameSite=Lax"));
        assert!(set_cookie.contains("; Max-Age="));
        let cookie = set_cookie.split(';').next().unwrap().to_owned();
        let token = cookie.strip_prefix("phoenix-auth=").unwrap();
        assert!(!token.is_empty());
        assert_ne!(token, password);
        assert!(self
            .state
            .db
            .is_auth_session_valid(token, &crate::api::auth::password_fingerprint(password))
            .await
            .unwrap());
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap(),
            json!({"ok": true})
        );
        cookie
    }

    async fn finish(self) {
        self.state.runtime.begin_process_shutdown();
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            self.state.runtime.drain_process_shutdown(),
        )
        .await
        .expect("bounded fixture shutdown");
        self.state.db.pool().close().await;
    }
}

#[tokio::test]
async fn live_full_router_without_password_reports_authenticated_and_resolves_id() {
    let fixture = LiveAuthFixture::new(None).await;
    let (status, body) = response_json(
        &fixture.app,
        loopback_request("GET", "/api/auth/status", Body::empty()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({"auth_required": false, "authenticated": true}));
    let (status, body) = response_json(
        &fixture.app,
        loopback_request(
            "GET",
            "/api/conversations/live-auth-row/route",
            Body::empty(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!({"id": "live-auth-row", "slug": "live-auth-slug"})
    );
    fixture.finish().await;
}

#[tokio::test]
async fn live_full_router_password_login_persists_cookie_and_authenticates_status_and_id() {
    let fixture = LiveAuthFixture::new(Some("live-owner-password")).await;
    let (status, body) = response_json(
        &fixture.app,
        loopback_request("GET", "/api/auth/status", Body::empty()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({"auth_required": true, "authenticated": false}));
    for cookie in [
        None,
        Some("phoenix-auth=live-owner-password"),
        Some("phoenix-auth=invalid"),
    ] {
        let mut request = loopback_request(
            "GET",
            "/api/conversations/live-auth-row/route",
            Body::empty(),
        );
        if let Some(cookie) = cookie {
            request
                .headers_mut()
                .insert("cookie", cookie.parse().unwrap());
        }
        let (status, _) = response_json(&fixture.app, request).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
    let cookie = fixture.login_cookie("live-owner-password").await;
    for (path, expected) in [
        (
            "/api/auth/status",
            json!({"auth_required": true, "authenticated": true}),
        ),
        (
            "/api/conversations/live-auth-row/route",
            json!({"id": "live-auth-row", "slug": "live-auth-slug"}),
        ),
    ] {
        let mut request = loopback_request("GET", path, Body::empty());
        request
            .headers_mut()
            .insert("cookie", cookie.parse().unwrap());
        let (status, body) = response_json(&fixture.app, request).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, expected);
    }
    fixture.finish().await;
}

#[tokio::test]
async fn live_full_router_bearer_lockout_does_not_lock_out_or_reset_valid_cookie() {
    let fixture = LiveAuthFixture::new(Some("live-owner-password")).await;
    let cookie = fixture.login_cookie("live-owner-password").await;
    let route = "/api/conversations/live-auth-row/route";
    let mut request = loopback_request("GET", route, Body::empty());
    request.headers_mut().insert(
        "authorization",
        "Bearer live-owner-password".parse().unwrap(),
    );
    let (status, _) = response_json(&fixture.app, request).await;
    assert_eq!(status, StatusCode::OK);
    for _ in 0..5 {
        let mut request = loopback_request("GET", route, Body::empty());
        request
            .headers_mut()
            .insert("authorization", "Bearer wrong".parse().unwrap());
        let (status, _) = response_json(&fixture.app, request).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
    for bearer in ["Bearer wrong", "Bearer live-owner-password"] {
        let mut request = loopback_request("GET", route, Body::empty());
        request
            .headers_mut()
            .insert("authorization", bearer.parse().unwrap());
        let (status, _) = response_json(&fixture.app, request).await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    }
    let mut request = loopback_request("GET", "/api/auth/status", Body::empty());
    request.headers_mut().insert(
        "authorization",
        "Bearer live-owner-password".parse().unwrap(),
    );
    let (status, body) = response_json(&fixture.app, request).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({"auth_required": true, "authenticated": false}));
    for (path, expected) in [
        (
            route,
            json!({"id": "live-auth-row", "slug": "live-auth-slug"}),
        ),
        (
            "/api/auth/status",
            json!({"auth_required": true, "authenticated": true}),
        ),
    ] {
        let mut request = loopback_request("GET", path, Body::empty());
        request
            .headers_mut()
            .insert("cookie", cookie.parse().unwrap());
        request
            .headers_mut()
            .insert("authorization", "Bearer wrong".parse().unwrap());
        let (status, body) = response_json(&fixture.app, request).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, expected);
    }
    let mut request = loopback_request("GET", route, Body::empty());
    request.headers_mut().insert(
        "authorization",
        "Bearer live-owner-password".parse().unwrap(),
    );
    let (status, _) = response_json(&fixture.app, request).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    fixture.finish().await;
}
