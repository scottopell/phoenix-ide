use super::AppState;
use axum::{extract::State, http::StatusCode, Json};
use serde::Deserialize;

macro_rules! wire_enum {
    ($name:ident { $($variant:ident => $wire:literal),+ $(,)? }) => {
        #[derive(Debug, Deserialize, PartialEq)]
        enum $name {
            $(#[serde(rename = $wire)] $variant),+
        }
        impl $name {
            fn as_str(&self) -> &'static str {
                match self { $(Self::$variant => $wire),+ }
            }
        }
    };
}
wire_enum!(Outcome { Ready => "ready", Failed => "failed", Canceled => "canceled", Incomplete => "incomplete" });
wire_enum!(Route { Global => "global", Ordinary => "ordinary" });
wire_enum!(Trigger { Navigation => "navigation", Reload => "reload", BackForward => "back_forward", PwaResume => "pwa_resume" });
wire_enum!(Platform { Ios => "ios", Android => "android", Desktop => "desktop", Unknown => "unknown" });
wire_enum!(Evidence { Commit => "commit_frame_estimate" });
wire_enum!(Cache { Network => "network", Retained => "retained", Unknown => "unknown" });
wire_enum!(ResponseSizeBucket { Unknown => "unknown", Tiny => "0-10k", Small => "10k-100k", Medium => "100k-1m", Large => "1m+" });
wire_enum!(CountBucket { Unknown => "unknown", Empty => "0", Small => "1-10", Medium => "11-100", Large => "101-1000", Huge => "1001+" });
wire_enum!(AnchorResult { Applied => "applied", TailPinned => "tail_pinned", Empty => "empty", TargetMissing => "target_missing", Superseded => "superseded" });
wire_enum!(CorrelationKind { Request => "request", Sse => "sse", Trace => "trace" });

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Marks {
    shell: Option<f64>,
    snapshot_received: Option<f64>,
    hydration_ready: Option<f64>,
    meaningful_transcript: Option<f64>,
    chapter_data_ready: Option<f64>,
    breadcrumb_presented: Option<f64>,
    breadcrumb_enabled: Option<f64>,
    anchor_settled: Option<f64>,
}

impl Marks {
    fn values(&self) -> [Option<f64>; 8] {
        [
            self.shell,
            self.snapshot_received,
            self.hydration_ready,
            self.meaningful_transcript,
            self.chapter_data_ready,
            self.breadcrumb_presented,
            self.breadcrumb_enabled,
            self.anchor_settled,
        ]
    }
    fn delta(&self) -> Option<f64> {
        Some(self.breadcrumb_presented?.max(self.breadcrumb_enabled?) - self.meaningful_transcript?)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Correlation {
    kind: CorrelationKind,
    id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "wire flags describe independent environment and censoring observations, not mutually exclusive states"
)]
pub(super) struct Report {
    open_id: uuid::Uuid,
    reporter_id: uuid::Uuid,
    counters_saturated: bool,
    product_conversation_id: Option<uuid::Uuid>,
    segment_id: Option<uuid::Uuid>,
    route: Route,
    trigger: Trigger,
    standalone: bool,
    platform: Platform,
    frontend_source: Option<String>,
    frontend_source_dirty: bool,
    cache: Cache,
    message_count_bucket: CountBucket,
    chapter_count_bucket: CountBucket,
    response_size_bucket: ResponseSizeBucket,
    reconnect_count: u32,
    visible: bool,
    marks: Marks,
    outcome: Outcome,
    total_ms: f64,
    background_ms: f64,
    background_interval_count: u32,
    background_open: bool,
    observer_inspection_count: u32,
    observer_scan_limited: bool,
    anchor_result: Option<AnchorResult>,
    timeline_saturated: bool,
    breadcrumb_ready_minus_meaningful_ms: Option<f64>,
    presentation_evidence: Evidence,
    correlations: Vec<Correlation>,
    dropped_correlations: u32,
    sample_rate: f64,
    admitted_count: u32,
    sampled_count: u32,
    transport_dropped_count: u32,
    transport_failed_count: u32,
}

impl Report {
    fn valid(&self) -> bool {
        let bounded = |n: f64| n.is_finite() && (0.0..=300_000.0).contains(&n);
        let hex = |s: &str| s.bytes().all(|b| b.is_ascii_hexdigit());
        let counters = [
            self.admitted_count,
            self.sampled_count,
            self.transport_dropped_count,
            self.transport_failed_count,
            self.dropped_correlations,
            self.background_interval_count,
            self.reconnect_count,
            self.observer_inspection_count,
        ];
        bounded(self.total_ms)
            && bounded(self.background_ms)
            && self.background_ms <= self.total_ms
            && self
                .marks
                .values()
                .into_iter()
                .flatten()
                .all(|n| bounded(n) && n <= self.total_ms)
            && (!self.timeline_saturated || self.total_ms.to_bits() == 300_000.0_f64.to_bits())
            && self.breadcrumb_ready_minus_meaningful_ms
                == if self.timeline_saturated {
                    None
                } else {
                    self.marks.delta()
                }
            && (self.outcome != Outcome::Ready
                || (!self.timeline_saturated
                    && self.marks.delta().is_some()
                    && self.marks.anchor_settled.is_some()))
            && self
                .frontend_source
                .as_deref()
                .is_none_or(|s| (7..=40).contains(&s.len()) && hex(s))
            && self.reconnect_count <= 10_000
            && self.marks.anchor_settled.is_some()
                == matches!(
                    self.anchor_result.as_ref(),
                    Some(AnchorResult::Applied | AnchorResult::TailPinned | AnchorResult::Empty)
                )
            && self.correlations.len() <= 16
            && self.correlations.iter().all(|c| match c.kind {
                CorrelationKind::Trace => c.id.len() == 32 && hex(&c.id),
                CorrelationKind::Request | CorrelationKind::Sse => {
                    uuid::Uuid::parse_str(&c.id).is_ok()
                }
            })
            && self.sample_rate.is_finite()
            && self.sample_rate > 0.0
            && self.sample_rate <= 1.0
            && self.sampled_count <= self.admitted_count
            && self.counters_saturated == counters.contains(&10_000)
            && counters.into_iter().all(|n| n <= 10_000)
            && (self.background_interval_count > 0
                || (self.background_ms == 0.0 && !self.background_open))
    }
}

pub(super) async fn report(
    State(state): State<AppState>,
    Json(report): Json<Report>,
) -> StatusCode {
    if !report.valid() {
        return StatusCode::BAD_REQUEST;
    }
    let Ok(instance_id) = state.db.instance_id().await else {
        tracing::warn!("view telemetry instance identity unavailable");
        return StatusCode::SERVICE_UNAVAILABLE;
    };
    let span = tracing::info_span!(target: "phoenix_ide::otel", "browser.conversation_view",
        "open.id" = %report.open_id,
        "browser.reporter_id" = %report.reporter_id,
        "browser.counters_saturated" = report.counters_saturated,
        "product_conversation.id" = report.product_conversation_id.as_ref().map(uuid::Uuid::to_string),
        "segment.id" = report.segment_id.as_ref().map(uuid::Uuid::to_string),
        "browser.route" = report.route.as_str(), "browser.trigger" = report.trigger.as_str(),
        "browser.outcome" = report.outcome.as_str(), "browser.platform" = report.platform.as_str(),
        "browser.standalone" = report.standalone, "browser.visible" = report.visible,
        "browser.frontend_source" = report.frontend_source.as_deref(),
        "browser.frontend_source_dirty" = report.frontend_source_dirty,
        "browser.cache" = report.cache.as_str(),
        "browser.message_count_bucket" = report.message_count_bucket.as_str(),
        "browser.chapter_count_bucket" = report.chapter_count_bucket.as_str(),
        "browser.response_size_bucket" = report.response_size_bucket.as_str(),
        "browser.reconnect_count" = i64::from(report.reconnect_count),
        "browser.anchor_result" = report.anchor_result.as_ref().map(AnchorResult::as_str),
        "server.backend_source" = env!("PHOENIX_GIT_SHA"),
        "server.instance_id" = %instance_id,
        "browser.presentation_evidence" = report.presentation_evidence.as_str(),
        "browser.total_ms" = report.total_ms, "browser.background_ms" = report.background_ms,
        "browser.background_interval_count" = i64::from(report.background_interval_count),
        "browser.background_open" = report.background_open,
        "browser.observer_inspection_count" = i64::from(report.observer_inspection_count),
        "browser.observer_scan_limited" = report.observer_scan_limited,
        "browser.timeline_saturated" = report.timeline_saturated,
        "browser.shell_ms" = report.marks.shell,
        "browser.snapshot_received_ms" = report.marks.snapshot_received,
        "browser.hydration_ready_ms" = report.marks.hydration_ready,
        "browser.meaningful_transcript_ms" = report.marks.meaningful_transcript,
        "browser.chapter_data_ready_ms" = report.marks.chapter_data_ready,
        "browser.breadcrumb_presented_ms" = report.marks.breadcrumb_presented,
        "browser.breadcrumb_enabled_ms" = report.marks.breadcrumb_enabled,
        "browser.anchor_settled_ms" = report.marks.anchor_settled,
        "browser.breadcrumb_ready_minus_meaningful_ms" = report.breadcrumb_ready_minus_meaningful_ms,
        "browser.sample_rate" = report.sample_rate,
        "browser.admitted_count" = i64::from(report.admitted_count), "browser.sampled_count" = i64::from(report.sampled_count),
        "browser.dropped_correlations" = i64::from(report.dropped_correlations),
        "browser.transport_dropped_count" = i64::from(report.transport_dropped_count),
        "browser.transport_failed_count" = i64::from(report.transport_failed_count),
    );
    let _entered = span.enter();
    for correlation in report.correlations {
        let child = tracing::info_span!(target: "phoenix_ide::otel", "browser.view_correlation",
            "open.id" = %report.open_id, "correlation.kind" = correlation.kind.as_str(),
            "correlation.id" = correlation.id);
        drop(child.enter());
    }
    tracing::info!(target: "phoenix_ide::view_journey_acceptance",
        schema_version = 1_u32,
        open_id = %report.open_id,
        server_instance_id = %instance_id,
        backend_source = env!("PHOENIX_GIT_SHA"),
        status = 204_u16,
        "conversation_view_accepted"
    );
    StatusCode::NO_CONTENT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Default)]
    struct WitnessWriter(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for WitnessWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for WitnessWriter {
        type Writer = Self;
        fn make_writer(&'a self) -> Self {
            self.clone()
        }
    }

    async fn collector_test_state() -> AppState {
        use std::sync::Arc;
        let db = phoenix_db::Database::open_in_memory().await.unwrap();
        let models = Arc::new(phoenix_llm::ModelRegistry::new_empty());
        let platform = crate::platform::PlatformCapability::None {
            details: "collector-test".into(),
        };
        let mcp = Arc::new(crate::tools::mcp::McpClientManager::new());
        let runtime = Arc::new(crate::runtime::RuntimeManager::new(
            db.clone(),
            models.clone(),
            platform.clone(),
            mcp.clone(),
            None,
        ));
        let retriever: Arc<dyn crate::db::MessageRetriever> = Arc::new(db.fts_retriever());
        AppState {
            terminals: runtime.terminals.clone(),
            runtime,
            llm_registry: models.clone(),
            platform,
            mcp_manager: mcp,
            chain_qa: crate::chain_qa::ChainQa::new(db.clone(), models, retriever.clone()),
            message_retriever: retriever,
            sessions: crate::api::auth::SessionStore::new(
                db.clone(),
                "collector-test-password".into(),
            ),
            db,
            federation_tls_trust:
                phoenix_core::domain::instance_identity::PeerTlsTrust::PlatformRoots,
            credential_helper: None,
            password: Some(
                phoenix_core::domain::instance_identity::OwnerPassword::parse(
                    "collector-test-password".into(),
                )
                .unwrap(),
            ),
            login_throttle: crate::api::auth::LoginThrottle::new(),
            codex_login: crate::api::codex_login::CodexLoginManager::new(),
            deployment: Arc::new(crate::api::deployment::DeploymentConfig::for_tests()),
            runtime_env: Arc::new(phoenix_core::runtime_env::PhoenixRuntimeEnvironment::detect()),
            suggest_token: String::new(),
            discovery: crate::discovery::start(crate::discovery::DiscoveryConfig {
                enabled: false,
                ..crate::discovery::DiscoveryConfig::from_env()
            }),
            resource_monitor: crate::api::resource_monitor::ResourceMonitor::new(),
        }
    }

    #[tokio::test]
    async fn collector_witness_requires_real_auth_and_validation_and_counts_each_acceptance() {
        use axum::{body::Body, http::Request};
        use tower::ServiceExt;
        use tracing::instrument::WithSubscriber;
        let state = collector_test_state().await;
        let instance = state.db.instance_id().await.unwrap().to_string();
        let router = crate::api::create_router(state);
        let writer = WitnessWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .json()
            .with_writer(writer.clone())
            .finish();
        let dispatch = tracing::Dispatch::new(subscriber);
        let valid = payload();
        let mut invalid = valid.clone();
        invalid["total_ms"] = (-1).into();
        for (authenticated, body, status) in [
            (false, valid.clone(), StatusCode::UNAUTHORIZED),
            (true, invalid, StatusCode::BAD_REQUEST),
            (
                true,
                serde_json::json!({"private": "NEVER_CAPTURE"}),
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
            (true, valid.clone(), StatusCode::NO_CONTENT),
            (true, valid.clone(), StatusCode::NO_CONTENT),
        ] {
            let mut request = Request::post("/api/telemetry/conversation-view")
                .header("Content-Type", "application/json");
            if authenticated {
                request = request.header("Authorization", "Bearer collector-test-password");
            }
            let response = router
                .clone()
                .oneshot(request.body(Body::from(body.to_string())).unwrap())
                .with_subscriber(dispatch.clone())
                .await
                .unwrap();
            assert_eq!(response.status(), status);
            let bytes = writer.0.lock().unwrap().clone();
            let witnesses: Vec<serde_json::Value> = String::from_utf8(bytes)
                .unwrap()
                .lines()
                .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
                .filter(|line| line["target"] == "phoenix_ide::view_journey_acceptance")
                .collect();
            if status != StatusCode::NO_CONTENT {
                assert!(witnesses.is_empty());
            }
        }
        let bytes = writer.0.lock().unwrap().clone();
        let witnesses: Vec<serde_json::Value> = String::from_utf8(bytes)
            .unwrap()
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .filter(|line| line["target"] == "phoenix_ide::view_journey_acceptance")
            .collect();
        assert_eq!(witnesses.len(), 2);
        for witness in witnesses {
            assert_eq!(
                witness["fields"],
                serde_json::json!({
                    "message": "conversation_view_accepted", "schema_version": 1,
                    "open_id": valid["open_id"], "server_instance_id": instance,
                    "backend_source": env!("PHOENIX_GIT_SHA"), "status": 204,
                })
            );
        }
    }

    #[test]
    fn pinned_default_layer_emits_typed_sdk_metadata_separately_from_journey_fields() {
        use opentelemetry::trace::TracerProvider as _;
        use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};
        use tracing_subscriber::prelude::*;
        let exporter = InMemorySpanExporter::default();
        let provider = SdkTracerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        let layer = tracing_opentelemetry::layer()
            .with_tracer(provider.tracer("collector-metadata-contract"));
        tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), || {
            let parent = tracing::info_span!(target: "phoenix_ide::otel", "browser.conversation_view", "open.id" = "test-open", "view.outcome" = "incomplete");
            let entered = parent.enter();
            let child = tracing::info_span!(target: "phoenix_ide::otel", "browser.view_correlation", "open.id" = "test-open", "correlation.kind" = "request", "correlation.id" = "test-request");
            drop(child.enter());
            drop(child);
            drop(entered);
            drop(parent);
        });
        provider.force_flush().unwrap();
        let spans = exporter.get_finished_spans().unwrap();
        assert_eq!(spans.len(), 2);
        let strings = [
            "code.file.path",
            "code.module.name",
            "thread.name",
            "target",
        ];
        let integers = ["code.line.number", "thread.id", "busy_ns", "idle_ns"];
        for span in &spans {
            for key in [
                "code.file.path",
                "code.module.name",
                "code.line.number",
                "thread.id",
                "target",
                "busy_ns",
                "idle_ns",
            ] {
                assert!(
                    span.attributes
                        .iter()
                        .any(|attribute| attribute.key.as_str() == key),
                    "missing default metadata {key}"
                );
            }
            for attribute in &span.attributes {
                if strings.contains(&attribute.key.as_str()) {
                    assert!(matches!(attribute.value, opentelemetry::Value::String(_)));
                } else if integers.contains(&attribute.key.as_str()) {
                    assert!(
                        matches!(attribute.value, opentelemetry::Value::I64(value) if value >= 0)
                    );
                } else {
                    assert!(
                        [
                            "open.id",
                            "view.outcome",
                            "correlation.kind",
                            "correlation.id"
                        ]
                        .contains(&attribute.key.as_str()),
                        "unexpected pinned layer field"
                    );
                }
            }
            assert!(!span
                .attributes
                .iter()
                .any(|attribute| attribute.key.as_str() == "level"));
            assert!(span
                .attributes
                .iter()
                .any(|attribute| attribute.key.as_str() == "open.id"));
        }
        let view = spans
            .iter()
            .find(|span| span.name == "browser.conversation_view")
            .unwrap();
        let child = spans
            .iter()
            .find(|span| span.name == "browser.view_correlation")
            .unwrap();
        assert_eq!(child.parent_span_id, view.span_context.span_id());
        assert_eq!(child.span_context.trace_id(), view.span_context.trace_id());
    }

    #[tokio::test]
    async fn production_attribute_capacity_preserves_complete_ready_collector_schema() {
        use opentelemetry::trace::TracerProvider as _;
        use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};
        use prost::Message as _;
        use tracing::instrument::WithSubscriber;
        use tracing_subscriber::prelude::*;
        let state = collector_test_state().await;
        let instance = state.db.instance_id().await.unwrap().to_string();
        let mut value = payload();
        value["frontend_source"] = "f".repeat(40).into();
        for key in [
            "admitted_count",
            "sampled_count",
            "transport_dropped_count",
            "transport_failed_count",
            "dropped_correlations",
            "background_interval_count",
            "reconnect_count",
            "observer_inspection_count",
        ] {
            value[key] = 10_000.into();
        }
        value["counters_saturated"] = true.into();
        value["correlations"] = serde_json::json!([{
            "kind": "request", "id": uuid::Uuid::new_v4().to_string()
        }]);
        value["outcome"] = "ready".into();
        value["anchor_result"] = "applied".into();
        value["product_conversation_id"] = serde_json::json!(uuid::Uuid::new_v4());
        value["segment_id"] = serde_json::json!(uuid::Uuid::new_v4());
        value["marks"]["snapshot_received"] = 5.into();
        value["marks"]["hydration_ready"] = 6.into();
        value["marks"]["chapter_data_ready"] = 8.into();
        value["marks"]["anchor_settled"] = 75.into();
        let parsed: Report = serde_json::from_value(value.clone()).unwrap();
        assert!(parsed.valid());
        let expected: std::collections::BTreeMap<&str, serde_json::Value> = [
            ("open.id", value["open_id"].clone()),
            ("browser.reporter_id", value["reporter_id"].clone()),
            (
                "browser.counters_saturated",
                value["counters_saturated"].clone(),
            ),
            (
                "product_conversation.id",
                value["product_conversation_id"].clone(),
            ),
            ("segment.id", value["segment_id"].clone()),
            ("browser.route", value["route"].clone()),
            ("browser.trigger", value["trigger"].clone()),
            ("browser.outcome", value["outcome"].clone()),
            ("browser.platform", value["platform"].clone()),
            ("browser.standalone", value["standalone"].clone()),
            ("browser.visible", value["visible"].clone()),
            ("browser.frontend_source", value["frontend_source"].clone()),
            (
                "browser.frontend_source_dirty",
                value["frontend_source_dirty"].clone(),
            ),
            ("browser.cache", value["cache"].clone()),
            (
                "browser.message_count_bucket",
                value["message_count_bucket"].clone(),
            ),
            (
                "browser.chapter_count_bucket",
                value["chapter_count_bucket"].clone(),
            ),
            (
                "browser.response_size_bucket",
                value["response_size_bucket"].clone(),
            ),
            ("browser.reconnect_count", value["reconnect_count"].clone()),
            ("browser.anchor_result", value["anchor_result"].clone()),
            (
                "server.backend_source",
                serde_json::json!(env!("PHOENIX_GIT_SHA")),
            ),
            ("server.instance_id", serde_json::json!(instance)),
            (
                "browser.presentation_evidence",
                value["presentation_evidence"].clone(),
            ),
            ("browser.total_ms", value["total_ms"].clone()),
            ("browser.background_ms", value["background_ms"].clone()),
            (
                "browser.background_interval_count",
                value["background_interval_count"].clone(),
            ),
            ("browser.background_open", value["background_open"].clone()),
            (
                "browser.observer_inspection_count",
                value["observer_inspection_count"].clone(),
            ),
            (
                "browser.observer_scan_limited",
                value["observer_scan_limited"].clone(),
            ),
            (
                "browser.timeline_saturated",
                value["timeline_saturated"].clone(),
            ),
            ("browser.shell_ms", value["marks"]["shell"].clone()),
            (
                "browser.snapshot_received_ms",
                value["marks"]["snapshot_received"].clone(),
            ),
            (
                "browser.hydration_ready_ms",
                value["marks"]["hydration_ready"].clone(),
            ),
            (
                "browser.meaningful_transcript_ms",
                value["marks"]["meaningful_transcript"].clone(),
            ),
            (
                "browser.chapter_data_ready_ms",
                value["marks"]["chapter_data_ready"].clone(),
            ),
            (
                "browser.breadcrumb_presented_ms",
                value["marks"]["breadcrumb_presented"].clone(),
            ),
            (
                "browser.breadcrumb_enabled_ms",
                value["marks"]["breadcrumb_enabled"].clone(),
            ),
            (
                "browser.anchor_settled_ms",
                value["marks"]["anchor_settled"].clone(),
            ),
            (
                "browser.breadcrumb_ready_minus_meaningful_ms",
                value["breadcrumb_ready_minus_meaningful_ms"].clone(),
            ),
            ("browser.sample_rate", value["sample_rate"].clone()),
            ("browser.admitted_count", value["admitted_count"].clone()),
            ("browser.sampled_count", value["sampled_count"].clone()),
            (
                "browser.dropped_correlations",
                value["dropped_correlations"].clone(),
            ),
            (
                "browser.transport_dropped_count",
                value["transport_dropped_count"].clone(),
            ),
            (
                "browser.transport_failed_count",
                value["transport_failed_count"].clone(),
            ),
        ]
        .into_iter()
        .collect();
        assert_eq!(expected.len(), 44);
        assert_eq!(expected.keys().map(|key| key.len()).sum::<usize>(), 1030);
        assert_eq!(
            crate::logging::phoenix_span_limits().max_attributes_per_span,
            52
        );
        for cap in [
            32,
            crate::logging::phoenix_span_limits().max_attributes_per_span,
        ] {
            let exporter = InMemorySpanExporter::default();
            let provider = SdkTracerProvider::builder()
                .with_simple_exporter(exporter.clone())
                .with_span_limits({
                    let mut limits = crate::logging::phoenix_span_limits();
                    limits.max_attributes_per_span = cap;
                    limits
                })
                .build();
            let subscriber = tracing_subscriber::registry().with(
                tracing_opentelemetry::layer()
                    .with_tracer(provider.tracer("real-ready-collector-budget")),
            );
            let status = report(
                State(state.clone()),
                Json(serde_json::from_value(value.clone()).unwrap()),
            )
            .with_subscriber(subscriber)
            .await;
            assert_eq!(status, StatusCode::NO_CONTENT);
            provider.force_flush().unwrap();
            let spans = exporter.get_finished_spans().unwrap();
            let view = spans
                .iter()
                .find(|span| span.name == "browser.conversation_view")
                .unwrap();
            assert!(view.events.is_empty());
            let proto: opentelemetry_proto::tonic::trace::v1::Span = view.clone().into();
            let semantic_bytes: usize = proto
                .attributes
                .iter()
                .filter(|attribute| expected.contains_key(attribute.key.as_str()))
                .map(|attribute| attribute.encoded_len())
                .sum();
            let metadata_bytes: usize = proto
                .attributes
                .iter()
                .filter(|attribute| !expected.contains_key(attribute.key.as_str()))
                .map(|attribute| attribute.encoded_len())
                .sum();
            println!("collector_capacity cap={cap} attributes={} dropped={} otlp_span_bytes={} semantic_kv_bytes={semantic_bytes} sdk_kv_bytes={metadata_bytes}", view.attributes.len(), view.dropped_attributes_count, proto.encoded_len());
            let required = [
                "browser.meaningful_transcript_ms",
                "browser.breadcrumb_presented_ms",
                "browser.breadcrumb_enabled_ms",
                "browser.anchor_settled_ms",
                "browser.breadcrumb_ready_minus_meaningful_ms",
            ];
            if cap == 32 {
                assert!(view.dropped_attributes_count > 0);
                assert!(required.iter().all(|key| !view
                    .attributes
                    .iter()
                    .any(|attribute| attribute.key.as_str() == *key)));
            } else {
                assert_eq!(view.dropped_attributes_count, 0);
                for key in required {
                    assert!(
                        view.attributes
                            .iter()
                            .any(|attribute| attribute.key.as_str() == key
                                && matches!(attribute.value, opentelemetry::Value::F64(_))),
                        "missing typed mark {key}"
                    );
                }
                assert!(view.attributes.len() <= 52);
                for (key, expected_value) in &expected {
                    let attribute = view
                        .attributes
                        .iter()
                        .find(|attribute| attribute.key.as_str() == *key)
                        .unwrap_or_else(|| panic!("missing field {key}"));
                    match (&attribute.value, expected_value) {
                        (
                            opentelemetry::Value::String(actual),
                            serde_json::Value::String(expected),
                        ) => assert_eq!(actual.as_str(), expected.as_str()),
                        (opentelemetry::Value::Bool(actual), serde_json::Value::Bool(expected)) => {
                            assert_eq!(actual, expected)
                        }
                        (
                            opentelemetry::Value::I64(actual),
                            serde_json::Value::Number(expected),
                        ) => assert_eq!(Some(*actual), expected.as_i64()),
                        (
                            opentelemetry::Value::F64(actual),
                            serde_json::Value::Number(expected),
                        ) => assert_eq!(Some(*actual), expected.as_f64()),
                        _ => panic!("wrong type {key}"),
                    }
                }
                for key in [
                    "browser.reconnect_count",
                    "browser.background_interval_count",
                    "browser.observer_inspection_count",
                    "browser.admitted_count",
                    "browser.sampled_count",
                    "browser.dropped_correlations",
                    "browser.transport_dropped_count",
                    "browser.transport_failed_count",
                ] {
                    assert!(
                        view.attributes
                            .iter()
                            .any(|attribute| attribute.key.as_str() == key
                                && matches!(attribute.value, opentelemetry::Value::I64(10_000))),
                        "counter must export as signed integer {key}"
                    );
                }
                let metadata = [
                    "code.file.path",
                    "code.module.name",
                    "code.line.number",
                    "thread.id",
                    "thread.name",
                    "target",
                    "busy_ns",
                    "idle_ns",
                ];
                assert!(view
                    .attributes
                    .iter()
                    .all(|attribute| expected.contains_key(attribute.key.as_str())
                        || metadata.contains(&attribute.key.as_str())));
                assert_eq!(spans.len(), 2);
                let child = spans
                    .iter()
                    .find(|span| span.name == "browser.view_correlation")
                    .unwrap();
                assert_eq!(child.parent_span_id, view.span_context.span_id());
                assert_eq!(child.span_context.trace_id(), view.span_context.trace_id());
                assert_eq!(child.dropped_attributes_count, 0);
                for (key, expected) in [
                    ("open.id", value["open_id"].as_str().unwrap()),
                    ("correlation.kind", "request"),
                    (
                        "correlation.id",
                        value["correlations"][0]["id"].as_str().unwrap(),
                    ),
                ] {
                    assert!(child.attributes.iter().any(|attribute| attribute.key.as_str() == key && matches!(&attribute.value, opentelemetry::Value::String(actual) if actual.as_str() == expected)));
                }
            }
        }
    }

    fn payload() -> serde_json::Value {
        serde_json::json!({
            "open_id": uuid::Uuid::new_v4(), "product_conversation_id": null, "segment_id": null,
            "reporter_id": uuid::Uuid::new_v4(), "counters_saturated": false,
            "route": "global", "trigger": "pwa_resume", "standalone": true, "platform": "ios",
            "frontend_source": "b8fc23f", "visible": true,
            "frontend_source_dirty": false, "cache": "unknown", "message_count_bucket": "unknown", "chapter_count_bucket": "unknown", "reconnect_count": 0, "anchor_result": null,
            "response_size_bucket": "unknown", "background_interval_count": 1, "background_open": false,
            "observer_inspection_count": 0, "observer_scan_limited": false,
            "marks": { "shell": 0, "snapshot_received": null, "hydration_ready": null,
                "meaningful_transcript": 50, "chapter_data_ready": null, "breadcrumb_presented": 10,
                "breadcrumb_enabled": 10, "anchor_settled": null },
            "outcome": "canceled", "total_ms": 100, "background_ms": 40, "timeline_saturated": false,
            "breadcrumb_ready_minus_meaningful_ms": -40, "presentation_evidence": "commit_frame_estimate",
            "correlations": [], "dropped_correlations": 0, "sample_rate": 0.25,
            "admitted_count": 4, "sampled_count": 1, "transport_dropped_count": 0, "transport_failed_count": 0
        })
    }

    #[test]
    fn accepts_missing_canceled_reordered_marks_but_not_fabricated_success() {
        let value = payload();
        assert!(serde_json::from_value::<Report>(value.clone())
            .unwrap()
            .valid());
        let mut success = value;
        success["outcome"] = "ready".into();
        assert!(!serde_json::from_value::<Report>(success).unwrap().valid());
    }

    #[test]
    fn rejects_content_and_forged_delta_and_excess_correlations() {
        let mut value = payload();
        value["message_text"] = "private".into();
        assert!(serde_json::from_value::<Report>(value).is_err());
        let mut value = payload();
        value["breadcrumb_ready_minus_meaningful_ms"] = 0.into();
        assert!(!serde_json::from_value::<Report>(value).unwrap().valid());
        let mut value = payload();
        value["correlations"] = serde_json::json!(vec![
            serde_json::json!({ "kind": "request", "id": uuid::Uuid::new_v4() });
            17
        ]);
        assert!(!serde_json::from_value::<Report>(value).unwrap().valid());
    }

    #[test]
    fn saturated_timeline_requires_censored_delta() {
        let mut value = payload();
        value["timeline_saturated"] = true.into();
        value["total_ms"] = 300_000.into();
        assert!(!serde_json::from_value::<Report>(value.clone())
            .unwrap()
            .valid());
        value["breadcrumb_ready_minus_meaningful_ms"] = serde_json::Value::Null;
        let mut report = serde_json::from_value::<Report>(value).unwrap();
        assert!(report.valid());
        report.total_ms = f64::from_bits(300_000.0_f64.to_bits() - 1);
        assert!(!report.valid());
        report.total_ms = f64::from_bits(300_000.0_f64.to_bits() + 1);
        assert!(!report.valid());
    }
}
