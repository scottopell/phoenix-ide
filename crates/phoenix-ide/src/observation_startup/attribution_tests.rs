use super::{attribution_subscriber, complete_stage, with_attribution_dispatch, Stage};
use crate::api::{
    application_state::{ApplicationState, ObservationApplication},
    auth::auth_middleware,
};
use axum::{
    body::Body,
    extract::{ConnectInfo, State},
    http::{Request, StatusCode},
    middleware::Next,
    routing::get,
    Router,
};
use serde_json::Value;
use std::{io::Write, net::SocketAddr, sync::Arc, sync::Mutex};
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

fn completions(records: &[Value]) -> Vec<&Value> {
    records
        .iter()
        .filter(|record| record["target"] == "phoenix::observation::completion")
        .collect()
}

fn terminal<'a>(events: &[&'a Value]) -> &'a Value {
    let roots: Vec<_> = events
        .iter()
        .copied()
        .filter(|event| event["fields"]["stage"] == "observation.request")
        .collect();
    assert_eq!(roots.len(), 1, "terminal events: {events:#?}");
    roots[0]
}

fn assert_private(records: &[Value]) {
    let text = serde_json::to_string(records).unwrap();
    assert!(!text.contains("SECRET"), "private input leaked: {text}");
    for event in completions(records) {
        let fields = event["fields"].as_object().unwrap();
        assert!(fields["elapsed_us"].as_u64().is_some());
        assert!(matches!(
            fields["outcome"].as_str(),
            Some("success" | "error" | "cancel")
        ));
        for key in fields.keys() {
            assert!(
                [
                    "stage",
                    "request_id",
                    "outcome",
                    "elapsed_us",
                    "status",
                    "response_ready"
                ]
                .contains(&key.as_str()),
                "unexpected event field: {key}"
            );
        }
    }
}

fn assert_stage(events: &[&Value], stage: &str, count: usize, outcome: &str, request_id: u64) {
    let stages: Vec<_> = events
        .iter()
        .filter(|event| event["fields"]["stage"] == stage)
        .collect();
    assert_eq!(stages.len(), count, "{stage}: {events:#?}");
    for event in stages {
        assert_eq!(event["fields"]["outcome"], outcome);
        assert!(
            event["spans"]
                .as_array()
                .unwrap()
                .iter()
                .any(|span| span["name"] == "observation.request"
                    && span["request_id"] == request_id),
            "uncorrelated stage: {event}"
        );
    }
}

fn request(uri: String, forwarded: bool) -> Request<Body> {
    let mut builder = Request::builder()
        .uri(uri)
        .extension(ConnectInfo("127.0.0.1:1234".parse::<SocketAddr>().unwrap()))
        .header("authorization", "Bearer AUTH_SECRET");
    if forwarded {
        builder = builder.header("forwarded", "for=FORWARDED_SECRET");
    }
    builder.body(Body::from("REQUEST_BODY_SECRET")).unwrap()
}

async fn application() -> (ApplicationState, String) {
    let db = crate::db::Database::open_in_memory().await.unwrap();
    let target = db
        .create_conversation("target", "target-slug", "/fixture", true, None, None)
        .await
        .unwrap();
    let product_id = target.product_conversation_id.to_string();
    let application = ObservationApplication::for_target(db, &target.id)
        .await
        .unwrap();
    (ApplicationState::Observation(application), product_id)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(clippy::too_many_lines)]
async fn actual_router_completes_success_error_and_denial_before_response_handoff() {
    let (application, product_id) = application().await;
    let existing = Capture::default();
    let mut sample_emitted = false;
    for outer in [tracing::Dispatch::none(), existing.dispatch()] {
        for (suffix, forwarded, status) in [
            ("", false, StatusCode::OK),
            ("?before=CURSOR_SECRET", false, StatusCode::BAD_REQUEST),
            ("", true, StatusCode::FORBIDDEN),
        ] {
            let capture = Capture::default();
            let app = with_attribution_dispatch(
                crate::api::create_router(application.clone()).layer(
                    tower_http::compression::CompressionLayer::new()
                        .gzip(true)
                        .br(true),
                ),
                capture.dispatch(),
            );
            let response = app
                .oneshot(request(
                    format!("/api/product-conversations/{product_id}{suffix}"),
                    forwarded,
                ))
                .with_subscriber(outer.clone())
                .await
                .unwrap();
            assert_eq!(response.status(), status);
            let records = capture.records();
            assert_private(&records);
            let events = completions(&records);
            let root = terminal(&events);
            let id = root["fields"]["request_id"].as_u64().unwrap();
            assert_eq!(root["fields"]["status"], status.as_u16());
            assert_eq!(root["fields"]["response_ready"], true);
            assert_eq!(
                root["fields"]["outcome"],
                if status == StatusCode::OK {
                    "success"
                } else {
                    "error"
                }
            );
            assert_eq!(
                events.last().copied(),
                Some(root),
                "request terminal must follow stage completion"
            );
            assert_stage(
                &events,
                "observation.authorization",
                1,
                if forwarded { "error" } else { "success" },
                id,
            );
            if forwarded {
                assert_eq!(events.len(), 2);
            } else {
                for stage in [
                    "replay.admission",
                    "observation.resolve_ordinary",
                    "observation.get_aggregate",
                    "observation.require_member",
                    "observation.member.resolve_ordinary",
                    "observation.member.get_aggregate",
                ] {
                    assert_stage(&events, stage, 1, "success", id);
                }
                assert_stage(
                    &events,
                    "product_conversation.detail_get",
                    1,
                    if status == StatusCode::OK {
                        "success"
                    } else {
                        "error"
                    },
                    id,
                );
                for stage in [
                    "product_conversation.pool_wait",
                    "product_conversation.begin_read",
                    "product_conversation.resolve",
                    "product_conversation.aggregate",
                    "product_conversation.close",
                    "product_conversation.message_page",
                    "product_conversation.snapshot_read",
                    "product_conversation.rollback_read",
                    "product_conversation.post_snapshot_projection",
                ] {
                    assert_stage(
                        &events,
                        stage,
                        usize::from(status == StatusCode::OK),
                        "success",
                        id,
                    );
                }
                for stage in ["product", "topology", "read_member", "source"] {
                    assert_stage(
                        &events,
                        &format!("observation.db.{stage}"),
                        if status == StatusCode::OK { 3 } else { 2 },
                        "success",
                        id,
                    );
                }
                assert_stage(&events, "observation.db.handoff", 0, "success", id);
                assert_eq!(events.len(), if status == StatusCode::OK { 30 } else { 17 });
                if status == StatusCode::OK {
                    for stage in ["product", "topology", "read_member", "source"] {
                        assert!(events.iter().any(|event| event["fields"]["stage"]
                            == format!("observation.db.{stage}")
                            && event["spans"]
                                .as_array()
                                .unwrap()
                                .iter()
                                .any(|span| span["name"] == "product_conversation.aggregate")));
                    }
                }
            }
            if status == StatusCode::OK && !sample_emitted {
                println!(
                    "observation_attribution_sample={}",
                    serde_json::to_string(&records).unwrap()
                );
                sample_emitted = true;
            }
            let count = events.len();
            let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
                .await
                .unwrap();
            if status == StatusCode::OK {
                let detail: Value = serde_json::from_slice(&body).unwrap();
                assert_eq!(detail["product_conversation_id"], product_id);
            }
            assert_eq!(
                completions(&capture.records()).len(),
                count,
                "terminal must precede body drain"
            );
        }
    }
    assert!(
        existing.records().is_empty(),
        "completion escaped private dispatcher"
    );
}

#[tokio::test]
async fn actual_router_admission_error_completes_only_started_stages() {
    let (application, _) = application().await;
    let existing = Capture::default();
    for outer in [tracing::Dispatch::none(), existing.dispatch()] {
        let capture = Capture::default();
        let app = with_attribution_dispatch(
            crate::api::create_router(application.clone()),
            capture.dispatch(),
        );
        let response = app
            .oneshot(request(
                "/api/product-conversations/UNKNOWN_SECRET".into(),
                false,
            ))
            .with_subscriber(outer)
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let records = capture.records();
        assert_private(&records);
        let events = completions(&records);
        let root = terminal(&events);
        let id = root["fields"]["request_id"].as_u64().unwrap();
        assert_eq!(root["fields"]["outcome"], "error");
        assert_eq!(root["fields"]["status"], StatusCode::NOT_FOUND.as_u16());
        assert_eq!(root["fields"]["response_ready"], true);
        assert_stage(&events, "observation.authorization", 1, "success", id);
        assert_stage(&events, "observation.resolve_ordinary", 1, "error", id);
        assert_stage(&events, "replay.admission", 1, "error", id);
        assert_eq!(events.len(), 4);
    }
    assert!(existing.records().is_empty());
}

#[test]
fn request_terminal_is_available_with_root_span_still_retained() {
    let capture = Capture::default();
    tracing::dispatcher::with_default(&capture.dispatch(), || {
        let (id, span) =
            crate::api::auth::observation_request_span(&request("/UNMATCHED_SECRET".into(), false));
        span.in_scope(|| super::RequestCompletion::new(id).response_ready(StatusCode::OK));
        let records = capture.records();
        let events = completions(&records);
        assert_eq!(terminal(&events)["fields"]["request_id"], id);
        assert_eq!(events.len(), 1);
        assert_private(&records);
        drop(span);
    });
}

#[tokio::test]
async fn pending_drop_emits_one_cancel_per_started_stage_under_owner_not_ambient() {
    fn assert_send<T: Send>(_: &T) {}
    let (application, _) = application().await;
    let existing = Capture::default();
    for outer in [tracing::Dispatch::none(), existing.dispatch()] {
        let capture = Capture::default();
        let app = Router::new()
            .route(
                "/attribution",
                get(|| async {
                    complete_stage(
                        Stage::Projection,
                        std::future::pending::<Result<StatusCode, StatusCode>>(),
                    )
                    .instrument(tracing::info_span!(
                        "product_conversation.post_snapshot_projection"
                    ))
                    .await
                }),
            )
            .layer(axum::middleware::from_fn_with_state(
                application.clone(),
                |state: State<ApplicationState>, req: Request<Body>, next: Next| async move {
                    let future = auth_middleware(state, req, next);
                    assert_send(&future);
                    future.await
                },
            ));
        let app = with_attribution_dispatch(app, capture.dispatch());
        tracing::dispatcher::with_default(&outer, || {
            let mut future =
                Box::pin(app.oneshot(request("/attribution?before=CURSOR_SECRET".into(), false)));
            let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
            assert!(std::future::Future::poll(future.as_mut(), &mut cx).is_pending());
            drop(future);
            drop(tracing::info_span!("observation.authorization"));
        });
        let records = capture.records();
        assert_private(&records);
        let events = completions(&records);
        let root = terminal(&events);
        let id = root["fields"]["request_id"].as_u64().unwrap();
        assert_eq!(root["fields"]["outcome"], "cancel");
        assert_eq!(root["fields"]["response_ready"], false);
        assert!(root["fields"]["status"].is_null());
        assert_stage(&events, "observation.authorization", 1, "success", id);
        assert_stage(
            &events,
            "product_conversation.post_snapshot_projection",
            1,
            "cancel",
            id,
        );
        assert_eq!(events.len(), 3);
        assert_eq!(events.last().copied(), Some(root));
        assert_eq!(
            records
                .iter()
                .filter(
                    |record| record["span"]["name"] == "observation.authorization"
                        && record["fields"]["message"] == "close"
                )
                .count(),
            1,
            "sentinel leaked into private dispatch"
        );
    }
    let records = existing.records();
    assert_eq!(
        records.len(),
        1,
        "outer sentinel not restored: {records:#?}"
    );
    assert_eq!(records[0]["span"]["name"], "observation.authorization");
    assert!(records[0]["span"]["request_id"].is_null());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn abort_is_observed_at_await_drop_not_a_sqlite_worker_completion() {
    let (application, _) = application().await;
    let existing = Capture::default();
    for outer in [tracing::Dispatch::none(), existing.dispatch()] {
        let capture = Capture::default();
        let started = Arc::new(tokio::sync::Notify::new());
        let notify = started.clone();
        let app = Router::new()
            .route(
                "/attribution",
                get(move || {
                    let notify = notify.clone();
                    async move {
                        complete_stage(Stage::Projection, async {
                            notify.notify_one();
                            std::future::pending::<Result<StatusCode, StatusCode>>().await
                        })
                        .instrument(tracing::info_span!(
                            "product_conversation.post_snapshot_projection"
                        ))
                        .await
                    }
                }),
            )
            .layer(axum::middleware::from_fn_with_state(
                application.clone(),
                auth_middleware,
            ));
        let app = with_attribution_dispatch(app, capture.dispatch());
        let task = tokio::spawn(async move {
            app.oneshot(request("/attribution".into(), false))
                .with_subscriber(outer)
                .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), started.notified())
            .await
            .unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        let records = capture.records();
        assert_private(&records);
        let events = completions(&records);
        let root = terminal(&events);
        let id = root["fields"]["request_id"].as_u64().unwrap();
        assert_eq!(root["fields"]["outcome"], "cancel");
        assert_eq!(root["fields"]["response_ready"], false);
        assert!(root["fields"]["status"].is_null());
        assert_stage(
            &events,
            "product_conversation.post_snapshot_projection",
            1,
            "cancel",
            id,
        );
        assert_eq!(events.len(), 3);
    }
    assert!(existing.records().is_empty());
}

#[test]
fn private_filter_rejects_unrelated_events_and_unmatched_path_data() {
    let capture = Capture::default();
    tracing::dispatcher::with_default(&capture.dispatch(), || {
        let (_, span) =
            crate::api::auth::observation_request_span(&request("/UNMATCHED_SECRET".into(), false));
        span.in_scope(|| {
            tracing::error!(secret = "EVENT_SECRET", "not attribution metadata");
            tracing::event!(name: "unreviewed", target: "phoenix::observation::completion", tracing::Level::INFO, secret = "EVENT_SECRET");
            drop(tracing::info_span!("unrelated", secret = "SPAN_SECRET"));
        });
    });
    let records = capture.records();
    assert_private(&records);
    assert!(
        completions(&records).is_empty(),
        "diagnostic CLOSE is not a terminal event"
    );
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["span"]["route"], "unmatched");
}
