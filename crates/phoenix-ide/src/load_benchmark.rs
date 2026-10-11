//! Offline conversation-load measurement: immutable fixture reads, no runtime startup.
//!
//! Auxiliary fixtures are a JSON object mapping `GET /api/path[?query]` to
//! `{ "status": 200, "content_type": "application/json", "body": ... }`.
//! A key without a query matches any query on that exact path. SSE bodies are
//! strings containing sanitized wire frames (`event: init\ndata: {...}\n\n`).
//! Only GET is admitted, including for fixtures. No production router is mounted.

use std::{
    collections::BTreeMap,
    net::Ipv4Addr,
    path::{Path, PathBuf},
    sync::Arc,
};

use axum::{
    extract::{Path as RoutePath, Query, Request, State},
    http::{header, HeaderValue, Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use futures::StreamExt;
use phoenix_db::Database;
use serde::Deserialize;
use serde_json::Value;
use tower_http::services::{ServeDir, ServeFile};

use crate::api::{
    handlers::AppError,
    product_conversations::{
        read_product_conversation_list, read_product_conversation_snapshot, ReadTiming,
        SnapshotQuery,
    },
};

type Error = Box<dyn std::error::Error + Send + Sync>;
type PreparedFixtures = BTreeMap<String, (StatusCode, HeaderValue, Vec<u8>)>;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureResponse {
    #[serde(default = "ok_status")]
    status: u16,
    #[serde(default = "json_content_type")]
    content_type: String,
    body: Value,
}

fn ok_status() -> u16 {
    200
}
fn json_content_type() -> String {
    "application/json".to_owned()
}

#[derive(Clone)]
struct ReadState {
    db: Database,
    fixtures: Arc<PreparedFixtures>,
}

fn standalone_path(path: &Path) -> Result<PathBuf, Error> {
    if !path.is_absolute() {
        return Err("benchmark database path must be absolute".into());
    }
    let canonical = path.canonicalize()?;
    if canonical.components().any(|part| {
        matches!(
            part.as_os_str().to_str(),
            Some(".phoenix-ide" | "prod" | "production")
        )
    }) || canonical
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("prod"))
    {
        return Err("benchmark refuses production database locations".into());
    }
    let metadata = std::fs::metadata(&canonical)?;
    if !metadata.is_file() {
        return Err("benchmark database must be a regular file".into());
    }
    if !metadata.permissions().readonly() {
        return Err("benchmark database fixture must be chmod read-only".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 {
            return Err("benchmark database must not be hard-linked".into());
        }
    }
    for base in [path, canonical.as_path()] {
        for suffix in ["-wal", "-shm", "-journal"] {
            let mut sidecar = base.as_os_str().to_os_string();
            sidecar.push(suffix);
            match std::fs::symlink_metadata(PathBuf::from(sidecar)) {
                Ok(_) => {
                    return Err(
                        "benchmark database has SQLite sidecars; supply a standalone snapshot"
                            .into(),
                    )
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
    Ok(canonical)
}

fn load_fixtures(path: &Path) -> Result<PreparedFixtures, Error> {
    use std::io::Read;
    const MAX_BYTES: u64 = 16 * 1024 * 1024;
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("auxiliary fixture exceeds 16 MiB".into());
    }
    let fixtures: BTreeMap<String, FixtureResponse> = serde_json::from_slice(&bytes)?;
    if fixtures.len() > 256 {
        return Err("auxiliary fixture exceeds 256 entries".into());
    }
    fixtures
        .into_iter()
        .map(|(key, fixture)| {
            let target = key
                .strip_prefix("GET ")
                .ok_or("fixture keys must start with GET ")?;
            let uri: axum::http::Uri = target.parse()?;
            if !target.starts_with("/api/")
                || uri.scheme().is_some()
                || uri.authority().is_some()
                || target.contains('#')
            {
                return Err("fixture keys must be local /api/ paths".into());
            }
            let path = uri.path();
            if real_read_path(path) {
                return Err("fixture must not override real ProductConversation reads".into());
            }
            let status = StatusCode::from_u16(fixture.status)?;
            if !(status.is_success() || status.is_client_error() || status.is_server_error()) {
                return Err("fixture status must not redirect or be informational".into());
            }
            let body = match fixture.content_type.as_str() {
                "application/json" => serde_json::to_vec(&fixture.body)?,
                "text/event-stream" | "text/plain" => fixture
                    .body
                    .as_str()
                    .ok_or("text fixture body must be a string")?
                    .as_bytes()
                    .to_vec(),
                _ => return Err("fixture content type must be JSON, SSE, or plain text".into()),
            };
            Ok((
                key,
                (status, HeaderValue::from_str(&fixture.content_type)?, body),
            ))
        })
        .collect()
}

fn real_read_path(path: &str) -> bool {
    if path == "/api/product-conversations" {
        return true;
    }
    path.strip_prefix("/api/product-conversations/")
        .is_some_and(|reference| {
            !reference.is_empty()
                && !reference.contains('/')
                && !matches!(reference, "events" | "creations")
        })
}

async fn gate(State(state): State<ReadState>, request: Request, next: Next) -> Response {
    if request.method() != Method::GET {
        return denied();
    }
    let path = request.uri().path();
    if path == "/api" || path.starts_with("/api/") {
        let exact = format!(
            "GET {}",
            request
                .uri()
                .path_and_query()
                .map_or(path, |value| value.as_str())
        );
        let simple = format!("GET {path}");
        if let Some((status, content_type, body)) = state
            .fixtures
            .get(&exact)
            .or_else(|| state.fixtures.get(&simple))
        {
            let mut response = (*status, body.clone()).into_response();
            response
                .headers_mut()
                .insert(header::CONTENT_TYPE, content_type.clone());
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            if content_type == "text/event-stream" {
                // Leave the fixed stream open without timers or generated events.
                let stream =
                    futures::stream::once(std::future::ready(Ok::<_, std::convert::Infallible>(
                        body.clone(),
                    )))
                    .chain(futures::stream::pending());
                *response.body_mut() = axum::body::Body::from_stream(stream);
            }
            return response;
        }
        if !real_read_path(path) {
            return denied();
        }
    }
    next.run(request).await
}

fn denied() -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(serde_json::json!({"error": "route denied by read-only benchmark"})),
    )
        .into_response()
}

fn timed(view: impl serde::Serialize, timing: &ReadTiming) -> Response {
    let mut response = Json(view).into_response();
    if let Ok(header) = HeaderValue::from_str(&format!(
        "db;dur={:.3}, projection;dur={:.3}",
        timing.db_ms, timing.projection_ms
    )) {
        response.headers_mut().insert("server-timing", header);
    }
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

async fn list(State(state): State<ReadState>) -> Result<Response, AppError> {
    let (view, timing) = read_product_conversation_list(&state.db).await?;
    Ok(timed(view, &timing))
}

async fn detail(
    State(state): State<ReadState>,
    RoutePath(reference): RoutePath<String>,
    Query(query): Query<SnapshotQuery>,
) -> Result<Response, AppError> {
    let (view, timing) = read_product_conversation_snapshot(&state.db, reference, query).await?;
    Ok(timed(view, &timing))
}

/// Build only the immutable fixture router, without constructing runtime state.
///
/// # Errors
/// Rejects unsafe/non-standalone database paths and invalid auxiliary fixtures.
pub async fn router(
    db_path: &Path,
    fixture_path: &Path,
    ui_dir: Option<&Path>,
) -> Result<Router, Error> {
    let canonical = standalone_path(db_path)?;
    let fixtures = load_fixtures(fixture_path)?;
    let db =
        Database::open_read_only(canonical.to_str().ok_or("database path must be UTF-8")?).await?;
    let state = ReadState {
        db,
        fixtures: Arc::new(fixtures),
    };
    let router = Router::new()
        .route("/api/product-conversations", get(list))
        .route("/api/product-conversations/:reference", get(detail));
    let router = if let Some(dir) = ui_dir {
        let dir = dir.canonicalize()?;
        if !dir.join("index.html").is_file() {
            return Err("UI directory must contain index.html".into());
        }
        router
            .fallback_service(ServeDir::new(&dir).fallback(ServeFile::new(dir.join("index.html"))))
    } else {
        router.fallback(|| async { denied() })
    };
    Ok(router
        .layer(middleware::from_fn_with_state(state.clone(), gate))
        .with_state(state))
}

/// Run a loopback-only, offline fixture server. Port zero requests an ephemeral port.
///
/// # Errors
/// Returns fixture validation, bind, or HTTP serving errors.
pub async fn serve(
    db_path: &Path,
    fixture_path: &Path,
    port: u16,
    ui_dir: Option<&Path>,
) -> Result<(), Error> {
    let router = router(db_path, fixture_path, ui_dir).await?;
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await?;
    eprintln!(
        "load_benchmark listening on http://{} (read-only fixture)",
        listener.local_addr()?
    );
    axum::serve(listener, router)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        http::Request,
    };
    use futures::{FutureExt, StreamExt};
    use phoenix_core::domain::db_schema::MessageContent;
    use tower::ServiceExt;

    fn readonly(path: &Path) {
        let mut permissions = std::fs::metadata(path).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(path, permissions).unwrap();
    }

    async fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf, String, String) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snapshot.db");
        let db = Database::open(path.to_str().unwrap()).await.unwrap();
        phoenix_db::run_pending_migrations(db.pool()).await.unwrap();
        let root = db
            .create_conversation("root", "root-slug", "/fixture", true, None, None)
            .await
            .unwrap();
        db.update_conversation_state(
            &root.id,
            &phoenix_db::ConvState::ContextExhausted {
                summary: "fixture handoff".to_owned(),
            },
        )
        .await
        .unwrap();
        let (outcome, _) = db
            .continue_conversation_with_intent(
                &root.id,
                phoenix_db::NewContinuationDispatchIntent::user_authorized(
                    phoenix_workflow::ClientTurnKey::try_from("opening".to_owned()).unwrap(),
                    "fixture opening".to_owned(),
                    None,
                ),
            )
            .await
            .unwrap();
        let phoenix_db::ContinueOutcome::Created(successor) = outcome else {
            panic!("expected successor");
        };
        for ordinal in 0..55 {
            db.add_message(
                &format!("message-{ordinal}"),
                &successor.id,
                &MessageContent::user(format!("fixture {ordinal}")),
                None,
                None,
            )
            .await
            .unwrap();
        }
        let (busy, wal_frames, checkpointed): (i64, i64, i64) =
            sqlx::query_as("PRAGMA wal_checkpoint(TRUNCATE)")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(busy, 0);
        assert_eq!(wal_frames, checkpointed);
        db.pool().close().await;
        readonly(&path);
        let auxiliary = dir.path().join("auxiliary.json");
        std::fs::write(&auxiliary, serde_json::to_vec(&serde_json::json!({
            "GET /api/models": { "body": {"models": []} },
            "GET /api/models?scope=exact": { "body": {"exact": true} },
            "GET /api/conversations/fixture/stream": { "content_type": "text/event-stream", "body": "event: init\ndata: {\"fixture\":true}\n\n" }
        })).unwrap()).unwrap();
        (
            dir,
            path,
            auxiliary,
            root.product_conversation_id.to_string(),
            successor.id,
        )
    }

    async fn request(router: &Router, method: Method, path: &str) -> Response {
        router
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn json(response: Response) -> Value {
        serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap()).unwrap()
    }

    #[tokio::test]
    async fn fixture_reads_preserve_pagination_alias_and_member_projection_without_writes() {
        let (_dir, path, auxiliary, product, member) = fixture().await;
        let original_db = std::fs::read(&path).unwrap();
        let original_auxiliary = std::fs::read(&auxiliary).unwrap();
        let router = router(&path, &auxiliary, None).await.unwrap();
        let list = request(&router, Method::GET, "/api/product-conversations").await;
        assert_eq!(list.status(), StatusCode::OK);
        assert!(list.headers()["server-timing"]
            .to_str()
            .unwrap()
            .contains("projection;dur="));
        assert_eq!(
            json(list).await["product_conversations"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let first = request(
            &router,
            Method::GET,
            &format!("/api/product-conversations/{member}"),
        )
        .await;
        assert_eq!(first.status(), StatusCode::OK);
        let first = json(first).await;
        assert_eq!(first["product_conversation_id"], product);
        assert_eq!(first["requested_transcript_row_id"], member);
        assert_eq!(
            first["segments"]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|segment| segment["messages"].as_array().unwrap())
                .count(),
            50
        );
        assert_eq!(first["has_older"], true);
        let before = first["before"].as_str().unwrap();
        let next = request(&router, Method::GET, &format!("/api/product-conversations/{product}?before={before}&message_limit=50&open_id=00000000-0000-0000-0000-000000000001")).await;
        assert_eq!(next.status(), StatusCode::OK);
        let next = json(next).await;
        assert_eq!(next["has_older"], false);
        assert_eq!(
            next["segments"]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|segment| segment["messages"].as_array().unwrap())
                .count(),
            5
        );
        let alias = json(
            request(
                &router,
                Method::GET,
                "/api/product-conversations/root-slug?message_limit=2",
            )
            .await,
        )
        .await;
        assert_eq!(alias["requested_transcript_row_id"], "root");
        assert_eq!(
            alias["segments"]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|segment| segment["messages"].as_array().unwrap())
                .count(),
            2
        );
        for query in [
            "message_limit=0",
            "message_limit=201",
            "before=invalid",
            "open_id=invalid",
        ] {
            assert_eq!(
                request(
                    &router,
                    Method::GET,
                    &format!("/api/product-conversations/{member}?{query}")
                )
                .await
                .status(),
                StatusCode::BAD_REQUEST
            );
        }
        let db = Database::open_read_only(path.to_str().unwrap())
            .await
            .unwrap();
        assert!(sqlx::query("DELETE FROM messages")
            .execute(db.pool())
            .await
            .is_err());
        db.pool().close().await;
        assert_eq!(std::fs::read(&path).unwrap(), original_db);
        assert_eq!(std::fs::read(&auxiliary).unwrap(), original_auxiliary);
        assert!(standalone_path(&path).is_ok());
    }

    #[tokio::test]
    async fn writes_unknown_apis_and_runtime_gets_are_denied_even_with_spa_fallback() {
        let (dir, path, auxiliary, _, member) = fixture().await;
        let ui = dir.path().join("ui");
        std::fs::create_dir(&ui).unwrap();
        std::fs::write(ui.join("index.html"), "fixture UI").unwrap();
        let router = router(&path, &auxiliary, Some(&ui)).await.unwrap();
        for method in [
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
            Method::HEAD,
            Method::OPTIONS,
        ] {
            for route in [
                "/api/models".to_owned(),
                format!("/api/product-conversations/{member}"),
                "/api/product-conversations".to_owned(),
                "/c/fixture".to_owned(),
            ] {
                assert_eq!(
                    request(&router, method.clone(), &route).await.status(),
                    StatusCode::FORBIDDEN
                );
            }
        }
        for route in [
            "/api",
            "/api/unknown",
            "/api/conversations/fixture",
            "/api/conversations/fixture/browser",
            "/api/product-conversations/events",
            "/api/product-conversations/creations",
            "/api/product-conversations/fixture/close",
        ] {
            assert_eq!(
                request(&router, Method::GET, route).await.status(),
                StatusCode::FORBIDDEN,
                "{route}"
            );
        }
        assert_eq!(
            request(&router, Method::GET, "/c/fixture").await.status(),
            StatusCode::OK
        );
        assert_eq!(
            json(request(&router, Method::GET, "/api/models?scope=anything").await).await,
            serde_json::json!({"models": []})
        );
        assert_eq!(
            json(request(&router, Method::GET, "/api/models?scope=exact").await).await,
            serde_json::json!({"exact": true})
        );
        let sse = request(
            &router,
            Method::GET,
            "/api/conversations/fixture/stream?after=1",
        )
        .await;
        assert_eq!(sse.headers()[header::CONTENT_TYPE], "text/event-stream");
        let mut stream = sse.into_body().into_data_stream();
        assert_eq!(
            stream.next().await.unwrap().unwrap(),
            "event: init\ndata: {\"fixture\":true}\n\n"
        );
        assert!(stream.next().now_or_never().is_none());
    }

    #[test]
    fn rejects_sidecars_writable_files_production_paths_and_links() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fixture.db");
        std::fs::write(&path, "fixture").unwrap();
        assert!(standalone_path(&path).is_err());
        readonly(&path);
        assert!(standalone_path(&path).is_ok());
        assert!(standalone_path(Path::new("fixture.db")).is_err());
        for suffix in ["-wal", "-shm", "-journal"] {
            let sidecar = dir.path().join(format!("fixture.db{suffix}"));
            std::fs::write(&sidecar, "sidecar").unwrap();
            assert!(standalone_path(&path).is_err());
            std::fs::remove_file(sidecar).unwrap();
        }
        let prod = dir.path().join("prod.db");
        std::fs::copy(&path, &prod).unwrap();
        assert!(standalone_path(&prod).is_err());
        #[cfg(unix)]
        {
            let link = dir.path().join("linked.db");
            std::fs::hard_link(&path, &link).unwrap();
            assert!(standalone_path(&link).is_err());
            assert!(standalone_path(&path).is_err());
            std::fs::remove_file(link).unwrap();
            let link = dir.path().join("alias.db");
            std::os::unix::fs::symlink(&prod, &link).unwrap();
            assert!(standalone_path(&link).is_err());
        }
    }

    #[test]
    fn fixtures_cannot_override_real_reads_enable_writes_or_redirect() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auxiliary.json");
        for fixture in [
            serde_json::json!({"POST /api/models": {"body": {}}}),
            serde_json::json!({"GET /api/product-conversations": {"body": {}}}),
            serde_json::json!({"GET /api/product-conversations/root": {"body": {}}}),
            serde_json::json!({"GET /api/models": {"status": 302, "body": {}}}),
            serde_json::json!({"GET /api/models": {"content_type": "text/event-stream", "body": {}}}),
            serde_json::json!({"GET /api/models": {"body": {}, "headers": {}}}),
        ] {
            std::fs::write(&path, serde_json::to_vec(&fixture).unwrap()).unwrap();
            assert!(load_fixtures(&path).is_err(), "{fixture}");
        }
    }
}
