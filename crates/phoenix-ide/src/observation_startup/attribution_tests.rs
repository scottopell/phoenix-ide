use super::{attribution_subscriber, with_attribution_dispatch};
use crate::api::{application_state::ObservationApplication, auth::auth_middleware};
use axum::{
    body::Body,
    extract::{ConnectInfo, Path},
    http::{Request, StatusCode},
    routing::get,
    Json, Router,
};
use serde::{Serialize, Serializer};
use serde_json::Value;
use std::{io::Write, net::SocketAddr, sync::Arc, sync::Mutex, time::Duration};
use tower::ServiceExt;
use tracing::{instrument::WithSubscriber, Instrument};

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Capture {
    fn records(&self) -> Vec<Value> {
        String::from_utf8(self.0.lock().unwrap().clone())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn dispatch(&self) -> tracing::Dispatch {
        let writer = self.clone();
        tracing::Dispatch::new(attribution_subscriber(move || writer.clone()))
    }
}

async fn application() -> crate::api::application_state::ApplicationState {
    let db = crate::db::Database::open_in_memory().await.unwrap();
    let target = db
        .create_conversation("target", "target-slug", "/fixture", true, None, None)
        .await
        .unwrap();
    ObservationApplication::for_target(db, &target.id)
        .await
        .map(crate::api::application_state::ApplicationState::Observation)
        .unwrap()
}

fn request(reference: &str, forwarded: bool) -> Request<Body> {
    let mut builder = Request::builder()
        .uri(format!("/attribution/{reference}?before=CURSOR_SECRET"))
        .extension(ConnectInfo("127.0.0.1:1234".parse::<SocketAddr>().unwrap()))
        .header("authorization", "Bearer AUTH_SECRET");
    if forwarded {
        builder = builder.header("forwarded", "for=FORWARDED_SECRET");
    }
    builder.body(Body::from("REQUEST_BODY_SECRET")).unwrap()
}

struct SlowBody;

impl Serialize for SlowBody {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        std::thread::sleep(Duration::from_millis(20));
        serializer.serialize_str("RESPONSE_BODY_SECRET")
    }
}

async fn handler(Path(reference): Path<String>) -> Result<Json<SlowBody>, StatusCode> {
    tokio::time::sleep(Duration::from_millis(25))
        .instrument(tracing::info_span!("observation.db.read_member"))
        .await;
    tracing::error!(secret = "EVENT_SECRET", "not attribution metadata");
    if reference == "ERROR_SECRET" {
        Err(StatusCode::BAD_REQUEST)
    } else {
        Ok(Json(SlowBody))
    }
}

fn request_record(record: &Value) -> bool {
    record["span"]["name"] == "observation.request"
}

fn seconds(value: &Value) -> f64 {
    let text = value.as_str().unwrap();
    for (suffix, scale) in [("ns", 1e-9), ("µs", 1e-6), ("ms", 1e-3), ("s", 1.0)] {
        if let Some(number) = text.strip_suffix(suffix) {
            return number.parse::<f64>().unwrap() * scale;
        }
    }
    panic!("unexpected fmt duration: {text}");
}

fn assert_private(records: &[Value]) {
    let text = serde_json::to_string(records).unwrap();
    assert!(!text.contains("SECRET"), "private input leaked: {text}");
    for record in records {
        assert!(record["timestamp"].as_str().is_some());
        assert_eq!(record["fields"]["message"], "close");
        assert!(record["fields"]["time.busy"].as_str().is_some());
        assert!(record["fields"]["time.idle"].as_str().is_some());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closes_associated_success_error_and_denial_before_body_drain() {
    let capture = Capture::default();
    let dispatch = capture.dispatch();
    let app = Router::new()
        .route("/attribution/:reference", get(handler))
        .layer(axum::middleware::from_fn_with_state(
            application().await,
            auth_middleware,
        ));
    let app = with_attribution_dispatch(app, dispatch);
    let mut previous_id = 0;
    for (reference, forwarded, status) in [
        ("SUCCESS_SECRET", false, StatusCode::OK),
        ("ERROR_SECRET", false, StatusCode::BAD_REQUEST),
        ("DENIED_SECRET", true, StatusCode::FORBIDDEN),
    ] {
        let response = tokio::spawn(app.clone().oneshot(request(reference, forwarded)))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.status(), status);
        let records = capture.records();
        assert_private(&records);
        let request = records
            .iter()
            .filter(|record| request_record(record))
            .next_back()
            .unwrap();
        let id = request["span"]["request_id"].as_u64().unwrap();
        assert!(id > previous_id);
        previous_id = id;
        assert_eq!(request["span"]["route"], "/attribution/:reference");
        assert_eq!(request["span"]["status"], status.as_u16());
        assert_eq!(request["span"]["response_ready"], true);
        if status == StatusCode::OK {
            let wall =
                seconds(&request["fields"]["time.busy"]) + seconds(&request["fields"]["time.idle"]);
            assert!(
                wall >= 0.040,
                "await and serialization must be included: {wall}"
            );
        }
        assert!(records.iter().any(|record| {
            record["span"]["name"] == "observation.authorization"
                && record["spans"]
                    .as_array()
                    .is_some_and(|spans| spans.iter().any(|span| span["request_id"] == id))
        }));
        let count = records.len();
        let _ = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        assert_eq!(capture.records().len(), count);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_closes_request_and_child_without_claiming_response_ready() {
    let capture = Capture::default();
    let started = Arc::new(tokio::sync::Notify::new());
    let notify = started.clone();
    let app = Router::new()
        .route(
            "/attribution/:reference",
            get(move || async move {
                async {
                    notify.notify_one();
                    std::future::pending::<StatusCode>().await
                }
                .instrument(tracing::info_span!("observation.db.read_member"))
                .await
            }),
        )
        .layer(axum::middleware::from_fn_with_state(
            application().await,
            auth_middleware,
        ));
    let app = with_attribution_dispatch(app, capture.dispatch());
    let task = tokio::spawn(app.oneshot(request("CANCEL_SECRET", false)));
    tokio::time::timeout(Duration::from_secs(2), started.notified())
        .await
        .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    let records = capture.records();
    assert_private(&records);
    let request = records
        .iter()
        .find(|record| request_record(record))
        .unwrap();
    assert_eq!(request["span"]["response_ready"], false);
    assert!(request["span"]["status"].is_null());
    let id = request["span"]["request_id"].as_u64().unwrap();
    assert!(records.iter().any(|record| {
        record["span"]["name"] == "observation.db.read_member"
            && record["spans"]
                .as_array()
                .is_some_and(|spans| spans.iter().any(|span| span["request_id"] == id))
    }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(clippy::too_many_lines)]
async fn startup_dispatch_captures_actual_product_detail_without_inheriting_default() {
    let db = crate::db::Database::open_in_memory().await.unwrap();
    let target = db
        .create_conversation("target", "target-slug", "/fixture", true, None, None)
        .await
        .unwrap();
    let application = ObservationApplication::for_target(db, &target.id)
        .await
        .unwrap();
    let existing = Capture::default();
    for outer in [tracing::Dispatch::none(), existing.dispatch()] {
        let emit_sample = outer.is_none();
        let capture = Capture::default();
        let app = with_attribution_dispatch(
            crate::api::create_router(
                crate::api::application_state::ApplicationState::Observation(application.clone()),
            )
            .layer(
                tower_http::compression::CompressionLayer::new()
                    .gzip(true)
                    .br(true),
            ),
            capture.dispatch(),
        );
        let request = Request::builder()
            .uri(format!(
                "/api/product-conversations/{}",
                target.product_conversation_id
            ))
            .extension(ConnectInfo("127.0.0.1:1234".parse::<SocketAddr>().unwrap()))
            .header("authorization", "Bearer AUTH_SECRET")
            .body(Body::empty())
            .unwrap();
        let response = tokio::spawn(
            async move {
                let response = app.oneshot(request).await.unwrap();
                drop(tracing::info_span!("observation.authorization"));
                response
            }
            .with_subscriber(outer),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let records = capture.records();
        assert_private(&records);
        let roots: Vec<_> = records
            .iter()
            .filter(|record| request_record(record))
            .collect();
        assert_eq!(roots.len(), 1);
        let request = roots[0];
        let id = request["span"]["request_id"].as_u64().unwrap();
        assert_eq!(
            request["span"]["route"],
            "/api/product-conversations/:reference"
        );
        assert_eq!(request["span"]["status"], StatusCode::OK.as_u16());
        assert_eq!(request["span"]["response_ready"], true);
        for name in [
            "observation.authorization",
            "product_conversation.detail_get",
            "product_conversation.post_snapshot_projection",
            "product_conversation.pool_wait",
            "product_conversation.begin_read",
            "product_conversation.resolve",
            "product_conversation.aggregate",
            "product_conversation.close",
            "product_conversation.message_page",
            "product_conversation.snapshot_read",
            "product_conversation.rollback_read",
            "observation.db.product",
            "observation.db.topology",
            "observation.db.read_member",
            "observation.db.source",
        ] {
            let children: Vec<_> = records
                .iter()
                .filter(|record| record["span"]["name"] == name)
                .collect();
            assert!(!children.is_empty(), "missing close record: {name}");
            for child in children {
                assert!(
                    child["spans"].as_array().unwrap().iter().any(|span| {
                        span["name"] == "observation.request" && span["request_id"] == id
                    }),
                    "unassociated child: {child}"
                );
            }
        }
        assert!(records.iter().any(|record| {
            record["span"]["name"] == "product_conversation.snapshot_read"
                && record["spans"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|span| span["name"] == "product_conversation.detail_get")
        }));
        for name in ["product", "topology", "read_member", "source"] {
            assert!(
                records.iter().any(|record| {
                    record["span"]["name"] == format!("observation.db.{name}")
                        && record["spans"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .any(|span| span["name"] == "product_conversation.aggregate")
                }),
                "missing snapshot aggregate child: {name}"
            );
        }
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let detail: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            detail["product_conversation_id"],
            target.product_conversation_id.as_str()
        );
        assert_eq!(capture.records().len(), records.len());
        if emit_sample {
            println!(
                "observation_attribution_sample={}",
                serde_json::to_string(&records).unwrap()
            );
        }
    }
    let records = existing.records();
    assert_private(&records);
    assert_eq!(
        records.len(),
        1,
        "request spans escaped the startup dispatch"
    );
    assert_eq!(records[0]["span"]["name"], "observation.authorization");
    assert!(records[0]["span"]["request_id"].is_null());
}

#[test]
fn scoped_subscriber_does_not_replace_existing_default_or_log_unmatched_path() {
    let capture = Capture::default();
    let existing = Capture::default();
    tracing::dispatcher::with_default(&existing.dispatch(), || {
        tracing::dispatcher::with_default(&capture.dispatch(), || {
            let request = request("UNMATCHED_SECRET", false);
            drop(crate::api::auth::observation_request_span(&request));
            drop(tracing::info_span!("unrelated", secret = "SPAN_SECRET"));
        });
        drop(tracing::info_span!("observation.authorization"));
    });
    let records = capture.records();
    assert_private(&records);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["span"]["route"], "unmatched");
    assert_eq!(existing.records().len(), 1);
}
