use super::{model, ScannerWake};
use phoenix_llm::{ContentBlock, LlmConfig, LlmResponse, ModelRegistry};
use serde_json::json;

#[test]
fn scanner_route_rejects_default_proxy_and_unavailable_codex() {
    let expensive_default = ModelRegistry::new(&LlmConfig {
        anthropic_api_key: Some("offline-not-dispatched".into()),
        default_model: Some("claude-opus-5-5".into()),
        ..LlmConfig::default()
    });
    assert!(expensive_default.default().is_some());
    assert!(model::eligible_service(&expensive_default).is_none());

    let proxy = ModelRegistry::new(&LlmConfig {
        openai_api_key: Some("offline-not-dispatched".into()),
        openai_responses_base_url: Some("https://proxy.invalid/v1/responses".into()),
        ..LlmConfig::default()
    });
    assert!(proxy.get(model::MODEL).is_some());
    assert!(model::eligible_service(&proxy).is_none());

    let missing_codex = ModelRegistry::new(&LlmConfig {
        use_codex_auth: true,
        openai_api_key: Some("offline-not-dispatched".into()),
        ..LlmConfig::default()
    });
    assert!(model::eligible_service(&missing_codex).is_none());
}

#[test]
fn scanner_route_accepts_exact_official_low_effort_service() {
    let registry = ModelRegistry::new(&LlmConfig {
        openai_api_key: Some("offline-not-dispatched".into()),
        ..LlmConfig::default()
    });
    let service = model::eligible_service(&registry).unwrap();
    assert_eq!(service.model_id(), model::MODEL);
    assert!(service.uses_official_openai_responses());
}

#[test]
fn scanner_request_bounds_full_serialized_content_and_contains_no_tools() {
    let input = json!({"text":"🦊".repeat(100)});
    let encoded_input_bytes = serde_json::to_vec(&input).unwrap().len();
    let system = "s".repeat(model::MAX_CONTENT_BYTES - encoded_input_bytes);
    let bounded = model::BoundedRequest::new(&system, &input).unwrap();
    assert_eq!(bounded.content_bytes(), model::MAX_CONTENT_BYTES);
    assert!(model::BoundedRequest::new(&(system + "s"), &input).is_err());
    let request = bounded.request();
    assert_eq!(request.max_tokens, 2048);
    assert_eq!(
        request.effective_effort,
        phoenix_llm::EffectiveEffort::explicit(model::EFFORT)
    );
    assert!(request.provider_replay.is_none());
    assert!(request.responses_replay.is_empty());
    assert!(request.tool_availability.declarations().is_empty());
    assert!(request.tool_availability.callable_names().is_empty());
    assert_eq!(
        request.service_tier,
        phoenix_core::domain::llm_types::EffectiveServiceTier::Standard
    );
    assert_eq!(model::TIMEOUT.as_secs(), 30);
}

#[test]
fn scanner_rejects_incomplete_or_nontext_provider_results() {
    let mut complete = LlmResponse::non_streaming(
        vec![ContentBlock::text("{}")],
        true,
        phoenix_llm::Usage::default(),
    );
    complete.usage_observation.output = true;
    assert_eq!(model::complete_text(&complete).unwrap(), "{}");
    let mut incomplete = LlmResponse::non_streaming(
        vec![ContentBlock::text("{}")],
        false,
        phoenix_llm::Usage::default(),
    );
    incomplete.usage_observation.output = true;
    assert!(model::complete_text(&incomplete).is_err());
    let mut tool = LlmResponse::non_streaming(
        vec![ContentBlock::tool_use(
            "unexpected",
            "think",
            json!({"thoughts":"not allowed"}),
        )],
        true,
        phoenix_llm::Usage::default(),
    );
    tool.usage_observation.output = true;
    assert!(model::complete_text(&tool).is_err());
    let mut empty = LlmResponse::non_streaming(vec![], true, phoenix_llm::Usage::default());
    empty.usage_observation.output = true;
    assert!(model::complete_text(&empty).is_err());
}

#[test]
fn scanner_runner_can_be_started_only_once() {
    let wake = ScannerWake::default();
    assert!(wake.begin_once().is_some());
    assert!(wake.begin_once().is_none());
}

#[test]
fn scanner_usage_distinguishes_unknown_from_reported_zero() {
    let mut response = LlmResponse::non_streaming(
        vec![ContentBlock::text("{}")],
        true,
        phoenix_llm::Usage {
            input_tokens: 11,
            output_tokens: 3,
            ..Default::default()
        },
    );
    assert!(super::metrics(&response, 7).usage.is_none());
    response.usage_observation.output = true;
    let partial = super::metrics(&response, 7).usage.unwrap();
    assert_eq!(partial.output_tokens, Some(3));
    assert_eq!(partial.input_tokens, None);
    assert_eq!(partial.cache_read_tokens, None);
    assert_eq!(partial.cache_creation_tokens, None);
    response.usage_observation = phoenix_llm::UsageObservation {
        input_total: true,
        output: true,
        cache_read: true,
        cache_write: true,
    };
    let metrics = super::metrics(&response, 7);
    let usage = metrics.usage.unwrap();
    assert_eq!(metrics.elapsed_ms, 7);
    assert_eq!(usage.input_tokens, Some(11));
    assert_eq!(usage.cache_read_tokens, Some(0));
    assert_eq!(usage.cache_creation_tokens, Some(0));
    assert_eq!(usage.output_tokens, Some(3));
}

async fn manager() -> std::sync::Arc<crate::runtime::RuntimeManager> {
    std::sync::Arc::new(crate::runtime::RuntimeManager::new(
        phoenix_db::Database::open_in_memory().await.unwrap(),
        std::sync::Arc::new(ModelRegistry::new(&LlmConfig::default())),
        crate::platform::PlatformCapability::None {
            details: "isolated scanner authority test".into(),
        },
        std::sync::Arc::new(phoenix_tools::mcp::McpClientManager::new()),
        None,
    ))
}

#[tokio::test]
async fn scanner_unclassified_write_closes_real_runtime_authority() {
    let runtime = manager().await;
    let known = super::WritePass::begin(&runtime, "scanner-test-known").unwrap();
    let result = known.complete::<()>(
        phoenix_db::workflow::LocalAuthorityResult::DurableFactEstablished(Err(
            phoenix_db::DbError::Serialization("known rollback".into()),
        )),
    );
    assert!(matches!(result, Some(Err(_))));
    assert!(!runtime.local_authority_is_closed());
    let unknown = super::WritePass::begin(&runtime, "scanner-test-unknown").unwrap();
    assert!(unknown
        .complete::<()>(phoenix_db::workflow::LocalAuthorityResult::DurableFactUnclassified)
        .is_none());
    assert!(runtime.local_authority_is_closed());
    assert_eq!(
        runtime.observed_local_authority_failure_boundary(),
        Some("scanner-test-unknown")
    );
}

#[tokio::test]
async fn scanner_dropped_write_owner_closes_real_runtime_authority() {
    let runtime = manager().await;
    let write = super::WritePass::begin(&runtime, "scanner-test-dropped").unwrap();
    drop(write);
    assert!(runtime.local_authority_is_closed());
    assert_eq!(
        runtime.observed_local_authority_failure_boundary(),
        Some("scanner-test-dropped")
    );
}

enum ScriptedOutcome {
    Response(Box<LlmResponse>),
    Pending,
    Panic,
}

struct ScriptedService {
    response: ScriptedOutcome,
    calls: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl phoenix_llm::LlmService for ScriptedService {
    async fn complete(
        &self,
        request: &phoenix_llm::LlmRequest,
    ) -> Result<LlmResponse, phoenix_llm::LlmError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        assert!(request.tool_availability.declarations().is_empty());
        match &self.response {
            ScriptedOutcome::Response(response) => Ok(response.as_ref().clone()),
            ScriptedOutcome::Pending => std::future::pending().await,
            ScriptedOutcome::Panic => panic!("isolated provider panic"),
        }
    }
    fn model_id(&self) -> &str {
        model::MODEL
    }
}

#[tokio::test]
async fn scanner_inference_enforces_closed_schema_and_keeps_reported_usage_on_rejection() {
    let mut response = LlmResponse::non_streaming(
        vec![ContentBlock::text(
            r#"{"outcome":"complete","actions":[],"extra":true}"#,
        )],
        true,
        phoenix_llm::Usage {
            input_tokens: 11,
            output_tokens: 3,
            ..Default::default()
        },
    );
    response.usage_observation = phoenix_llm::UsageObservation {
        input_total: true,
        output: true,
        cache_read: true,
        cache_write: true,
    };
    let service = ScriptedService {
        response: ScriptedOutcome::Response(Box::new(response)),
        calls: std::sync::atomic::AtomicUsize::default(),
    };
    let request =
        model::BoundedRequest::new(super::prompt::INSTRUCTIONS, &json!({"fragments":[]})).unwrap();
    let (result, metrics) = super::infer(&service, &request).await;
    assert!(matches!(
        result,
        Err(phoenix_db::coordinator_scanner::ScanFailure::InvalidOutput(
            _
        ))
    ));
    assert_eq!(metrics.usage.unwrap().input_tokens, Some(11));
    assert_eq!(service.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn scanner_inference_times_out_without_unbudgeted_retry_or_fabricated_usage() {
    let service = ScriptedService {
        response: ScriptedOutcome::Pending,
        calls: std::sync::atomic::AtomicUsize::default(),
    };
    let request =
        model::BoundedRequest::new(super::prompt::INSTRUCTIONS, &json!({"fragments":[]})).unwrap();
    let (result, metrics) = super::infer(&service, &request).await;
    assert!(matches!(
        result,
        Err(phoenix_db::coordinator_scanner::ScanFailure::Timeout)
    ));
    assert!(metrics.usage.is_none());
    assert_eq!(service.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test]
async fn scanner_provider_panic_returns_failure_without_hidden_retry() {
    let service = ScriptedService {
        response: ScriptedOutcome::Panic,
        calls: std::sync::atomic::AtomicUsize::default(),
    };
    let request =
        model::BoundedRequest::new(super::prompt::INSTRUCTIONS, &json!({"fragments":[]})).unwrap();
    let (result, metrics) = super::infer(&service, &request).await;
    assert!(matches!(
        result,
        Err(phoenix_db::coordinator_scanner::ScanFailure::Provider(_))
    ));
    assert!(metrics.usage.is_none());
    assert_eq!(service.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
}

struct OutboundNameService(Option<&'static str>);

#[async_trait::async_trait]
impl phoenix_llm::LlmService for OutboundNameService {
    async fn complete(
        &self,
        _request: &phoenix_llm::LlmRequest,
    ) -> Result<LlmResponse, phoenix_llm::LlmError> {
        panic!("route inspection must not dispatch")
    }
    fn model_id(&self) -> &str {
        model::MODEL
    }
    fn outbound_model_name(&self) -> Option<&str> {
        self.0
    }
    fn uses_official_openai_responses(&self) -> bool {
        true
    }
}

#[test]
fn scanner_rejects_expensive_or_unknown_outbound_spelling_behind_luna_catalog_id() {
    assert!(!model::eligible_identity(&OutboundNameService(Some(
        "gpt-6-astra"
    ))));
    assert!(!model::eligible_identity(&OutboundNameService(None)));
    assert!(model::eligible_identity(&OutboundNameService(Some(
        model::MODEL
    ))));
}

#[test]
fn scanner_acceptance_requires_reported_output_usage_and_both_independent_bounds() {
    let mut response = LlmResponse::non_streaming(
        vec![ContentBlock::text("{}")],
        true,
        phoenix_llm::Usage {
            output_tokens: 2048,
            ..Default::default()
        },
    );
    assert!(model::complete_text(&response)
        .unwrap_err()
        .contains("unavailable"));
    response.usage_observation.output = true;
    assert_eq!(model::complete_text(&response).unwrap(), "{}");
    response.usage.output_tokens = 2049;
    assert!(model::complete_text(&response)
        .unwrap_err()
        .contains("reported-token"));
    response.usage.output_tokens = 2048;
    response.content = vec![ContentBlock::text("🦊".repeat(model::MAX_OUTPUT_BYTES / 4))];
    assert_eq!(
        model::complete_text(&response).unwrap().len(),
        model::MAX_OUTPUT_BYTES
    );
    response.content.push(ContentBlock::text("x"));
    assert!(model::complete_text(&response)
        .unwrap_err()
        .contains("UTF8-byte"));
}

#[tokio::test]
async fn scanner_oversized_complete_result_is_rejected_before_closed_json_acceptance() {
    let mut response = LlmResponse::non_streaming(
        vec![ContentBlock::text(r#"{"outcome":"complete","actions":[]}"#)],
        true,
        phoenix_llm::Usage {
            output_tokens: 2049,
            ..Default::default()
        },
    );
    response.usage_observation.output = true;
    let service = ScriptedService {
        response: ScriptedOutcome::Response(Box::new(response)),
        calls: std::sync::atomic::AtomicUsize::default(),
    };
    let request =
        model::BoundedRequest::new(super::prompt::INSTRUCTIONS, &json!({"fragments":[]})).unwrap();
    let (result, _) = super::infer(&service, &request).await;
    assert!(
        matches!(result, Err(phoenix_db::coordinator_scanner::ScanFailure::InvalidOutput(reason)) if reason.contains("reported-token"))
    );
}

struct CaptureService {
    calls: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl phoenix_llm::LlmService for CaptureService {
    async fn complete(
        &self,
        request: &phoenix_llm::LlmRequest,
    ) -> Result<LlmResponse, phoenix_llm::LlmError> {
        use phoenix_db::coordinator_scanner::{ScanAction, ScanInput, ScanQuote, ScanResult};
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let ContentBlock::Text { text } = &request.messages[0].content[0] else {
            panic!("expected actual encoded ScanInput")
        };
        let input: ScanInput = serde_json::from_str(text).unwrap();
        let source = input.fragments.first().unwrap();
        let result = ScanResult::Complete {
            actions: vec![ScanAction::New {
                kind: phoenix_db::coordinator_tracking::TrackingKind::RequestedWork,
                title: "Indigo status report".into(),
                note: "User requested the report without sending it.".into(),
                resumption: "Reconcile the request; do not send without permission.".into(),
                quotes: vec![ScanQuote {
                    fragment_id: source.fragment_id.clone(),
                    quotation: source.text.clone(),
                }],
            }],
        };
        let mut response = LlmResponse::non_streaming(
            vec![ContentBlock::text(serde_json::to_string(&result).unwrap())],
            true,
            phoenix_llm::Usage {
                output_tokens: 7,
                ..Default::default()
            },
        );
        response.usage_observation.output = true;
        Ok(response)
    }
    fn model_id(&self) -> &str {
        model::MODEL
    }
    fn outbound_model_name(&self) -> Option<&str> {
        Some(model::MODEL)
    }
    fn uses_official_openai_responses(&self) -> bool {
        true
    }
}

async fn whole_pass_fixture() -> (std::sync::Arc<crate::runtime::RuntimeManager>, String) {
    let runtime = manager().await;
    let coordinator = runtime
        .db()
        .get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
        .await
        .unwrap()
        .id;
    runtime
        .db()
        .add_message_with_seq_and_origin(
            "scanner-real-input",
            &coordinator,
            1,
            &phoenix_db::MessageContent::user(
                "Please prepare the Indigo status report; do not send it.",
            ),
            None,
            None,
            &phoenix_core::domain::db_schema::InputOrigin::UserApi,
        )
        .await
        .unwrap();
    (runtime, coordinator)
}

async fn dispatch_one_prepared_pass(
    runtime: &std::sync::Arc<crate::runtime::RuntimeManager>,
    service: &std::sync::Arc<CaptureService>,
) {
    let profile = super::profile(
        service.as_ref(),
        phoenix_db::coordinator_scanner::ScanConnection::OpenAiResponses,
    );
    for _ in 0..8 {
        let selected = (
            profile.clone(),
            service.clone() as std::sync::Arc<dyn phoenix_llm::LlmService>,
        );
        super::run_pass_with_selected(runtime, Some(selected)).await;
        if service.calls.load(std::sync::atomic::Ordering::SeqCst) > 0 {
            return;
        }
    }
    panic!("bounded host preparation never reached inference");
}

#[tokio::test]
async fn scanner_whole_pass_uses_real_prepare_reserve_apply_without_foreground_wake() {
    let (runtime, coordinator) = whole_pass_fixture().await;
    let before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages")
        .fetch_one(runtime.db().pool())
        .await
        .unwrap();
    let service = std::sync::Arc::new(CaptureService {
        calls: std::sync::atomic::AtomicUsize::default(),
    });
    dispatch_one_prepared_pass(&runtime, &service).await;
    let items: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM coordinator_tracking_items")
        .fetch_one(runtime.db().pool())
        .await
        .unwrap();
    assert_eq!(items, 1);
    let attempt: (String, Option<i64>, Option<i64>) =
        sqlx::query_as("SELECT state,output_tokens,input_tokens FROM coordinator_scan_attempts")
            .fetch_one(runtime.db().pool())
            .await
            .unwrap();
    assert_eq!(attempt, ("applied".into(), Some(7), None));
    let after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages")
        .fetch_one(runtime.db().pool())
        .await
        .unwrap();
    assert_eq!(after, before);
    assert!(runtime.try_get_handle(&coordinator).await.is_none());
    assert!(!runtime.local_authority_is_closed());
}

#[tokio::test]
async fn scanner_whole_pass_classifies_known_storage_failure_and_preserves_partial_usage() {
    let (runtime, coordinator) = whole_pass_fixture().await;
    sqlx::query("CREATE TRIGGER scanner_test_known_storage_failure BEFORE INSERT ON coordinator_tracking_items BEGIN SELECT RAISE(ABORT,'isolated scanner storage failure'); END;").execute(runtime.db().pool()).await.unwrap();
    let service = std::sync::Arc::new(CaptureService {
        calls: std::sync::atomic::AtomicUsize::default(),
    });
    dispatch_one_prepared_pass(&runtime, &service).await;
    let items: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM coordinator_tracking_items")
        .fetch_one(runtime.db().pool())
        .await
        .unwrap();
    assert_eq!(items, 0);
    let attempt: (String, Option<String>, Option<i64>, Option<i64>) = sqlx::query_as(
        "SELECT state,failure_kind,output_tokens,input_tokens FROM coordinator_scan_attempts",
    )
    .fetch_one(runtime.db().pool())
    .await
    .unwrap();
    assert_eq!(
        attempt,
        ("failed".into(), Some("storage".into()), Some(7), None)
    );
    let applied: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM coordinator_scan_applications")
        .fetch_one(runtime.db().pool())
        .await
        .unwrap();
    assert_eq!(applied, 0);
    assert!(runtime.try_get_handle(&coordinator).await.is_none());
    assert!(!runtime.local_authority_is_closed());
}
