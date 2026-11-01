#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RequestCapacity {
    pub(super) context_window: usize,
    pub(super) input_tokens: usize,
    pub(super) output_tokens: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CapacityDecision {
    Ready,
    NeedsCompaction,
    Conflict,
}

impl RequestCapacity {
    pub(super) fn decide(self, mandatory_input_tokens: usize) -> CapacityDecision {
        if self.input_tokens.saturating_add(self.output_tokens) <= self.context_window {
            CapacityDecision::Ready
        } else if mandatory_input_tokens.saturating_add(self.output_tokens) > self.context_window {
            CapacityDecision::Conflict
        } else {
            CapacityDecision::NeedsCompaction
        }
    }
}

pub(super) const SUCCESSOR_WORKING_ROOM_TOKENS: usize = 8192;

#[derive(Debug, Clone, Copy)]
pub(super) struct SuccessorCapacity {
    pub(super) context_window: usize,
    pub(super) ordinary_overhead_tokens: usize,
    pub(super) output_tokens: usize,
}

impl SuccessorCapacity {
    pub(super) fn working_room_tokens(self) -> usize {
        self.context_window
            .saturating_sub(self.ordinary_overhead_tokens)
            .saturating_sub(self.output_tokens)
            .div_ceil(4)
            .max(SUCCESSOR_WORKING_ROOM_TOKENS)
    }

    pub(super) fn handoff_budget(self) -> Result<usize, String> {
        let required = self
            .ordinary_overhead_tokens
            .saturating_add(self.output_tokens)
            .saturating_add(self.working_room_tokens());
        if required >= self.context_window {
            return Err(format!(
                "Required instructions/tools (estimated {} tokens), output reservation ({}), and successor working room ({}) leave no handoff space in the {}-token context. Handoff editing cannot fix required-overhead pressure; select a model with more available input capacity or revise required instructions/tools.",
                self.ordinary_overhead_tokens, self.output_tokens, self.working_room_tokens(), self.context_window,
            ));
        }
        Ok(self.context_window - required)
    }

    pub(super) fn validate(self, handoff_tokens: usize) -> Result<(), String> {
        let target = self.handoff_budget()?;
        if handoff_tokens > target {
            return Err(format!(
                "Generated handoff is estimated at {handoff_tokens} tokens; target is at most {target} after instructions/tools ({}), output reservation ({}), and {} tokens of successor working room. The original transcript is preserved. Review a shorter handoff or explicitly select a model with more input capacity.",
                self.ordinary_overhead_tokens, self.output_tokens, self.working_room_tokens(),
            ));
        }
        Ok(())
    }
}

use super::{LlmClient, LlmRequest};

/// One affine budget for every candidate in a logical planning operation.
pub(super) struct CountingBudget {
    attempted: bool,
}

#[derive(Debug)]
pub(super) struct InputAccounting {
    plan_id: String,
    request: LlmRequest,
    superseded_count: Option<SupersededInputCount>,
    pub(super) local_prediction: usize,
    pub(super) provider: Option<phoenix_llm::ProviderInputCount>,
    pub(super) fallback: Option<phoenix_llm::InputCountFallback>,
}

impl InputAccounting {
    pub(super) fn local(request: LlmRequest, local_prediction: usize) -> Self {
        Self {
            request,
            plan_id: uuid::Uuid::new_v4().to_string(),
            superseded_count: None,
            local_prediction,
            provider: None,
            fallback: Some(phoenix_llm::InputCountFallback::LocalReady),
        }
    }
    #[allow(clippy::needless_pass_by_value)] // consumes the discarded request so its count cannot be dispatched again
    pub(super) fn with_superseded(mut self, previous: InputAccounting) -> Self {
        self.superseded_count = Some(SupersededInputCount {
            local_prediction_tokens: tokens(previous.local_prediction),
            provider_input_tokens: previous.provider.map(|v| tokens(v.tokens())),
            counting_method: count_method(previous.provider),
            count_fallback: previous.fallback.map(fallback_reason),
        });
        self
    }
    pub(super) fn request(&self) -> &LlmRequest {
        &self.request
    }

    pub(super) fn into_request(mut self) -> LlmRequest {
        if let Some(telemetry) = &mut self.request.telemetry {
            telemetry.capacity_plan_id = Some(self.plan_id);
        }
        self.request
    }

    pub(super) fn occupancy(&self) -> usize {
        tracing::debug!(fallback = ?self.fallback, "capacity evidence");
        self.provider.map_or(
            self.local_prediction,
            phoenix_llm::ProviderInputCount::tokens,
        )
    }
}

impl CountingBudget {
    pub(super) fn new() -> Self {
        Self { attempted: false }
    }

    pub(super) async fn account(
        &mut self,
        client: &dyn LlmClient,
        request: LlmRequest,
        local_prediction: usize,
        input_limit: usize,
    ) -> InputAccounting {
        self.account_before_reduction(client, request, local_prediction, input_limit, false)
            .await
    }

    pub(super) async fn account_before_reduction(
        &mut self,
        client: &dyn LlmClient,
        request: LlmRequest,
        local_prediction: usize,
        input_limit: usize,
        proposed_reduction: bool,
    ) -> InputAccounting {
        let mut accounting = InputAccounting {
            request,
            plan_id: uuid::Uuid::new_v4().to_string(),
            superseded_count: None,
            local_prediction,
            provider: None,
            fallback: None,
        };
        if local_prediction <= input_limit && !proposed_reduction {
            accounting.fallback = Some(phoenix_llm::InputCountFallback::LocalReady);
            return accounting;
        }
        if self.attempted {
            accounting.fallback = Some(phoenix_llm::InputCountFallback::BudgetSpent);
            return accounting;
        }
        self.attempted = true;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        match tokio::time::timeout_at(deadline, client.count_input(&accounting.request)).await {
            Ok(Ok(Some(count))) => {
                tracing::debug!(local_prediction, provider_tokens = count.tokens(), evidence = ?count, "capacity input counted");
                accounting.provider = Some(count);
            }
            Ok(Ok(None)) => {
                accounting.fallback = Some(phoenix_llm::InputCountFallback::Unavailable);
                tracing::debug!(
                    "complete input counting unavailable; using conservative local planning"
                );
            }
            Ok(Err(error)) => {
                accounting.fallback = Some(phoenix_llm::InputCountFallback::Failed);
                tracing::warn!(kind = ?error.kind, "input counting failed; using conservative local planning");
            }
            Err(_) => {
                accounting.fallback = Some(phoenix_llm::InputCountFallback::TimedOut);
                tracing::warn!(
                    "input counting deadline elapsed; using conservative local planning"
                );
            }
        }
        accounting
    }
}

pub(super) async fn account_handoff(
    budget: &mut CountingBudget,
    client: &dyn LlmClient,
    mut request: LlmRequest,
    text: &str,
    authority: phoenix_core::domain::product_conversation::ContinuationOpeningAuthority,
    review: &mut crate::runtime::HandoffCapacityReview,
) -> InputAccounting {
    let rendered = if matches!(authority, phoenix_core::domain::product_conversation::ContinuationOpeningAuthority::GeneratedPredecessorContext) {
        format!(
            "[System-generated input]\n{}",
            crate::send_chat_service::generated_predecessor_context_projection(text)
        )
    } else {
        text.to_owned()
    };
    request.messages = vec![phoenix_llm::LlmMessage {
        source_message_id: None,
        role: phoenix_llm::MessageRole::User,
        content: vec![phoenix_llm::ContentBlock::text(rendered)],
    }];
    let input_limit = review
        .context_window_tokens
        .saturating_sub(review.output_reservation_tokens)
        .saturating_sub(review.successor_working_room_tokens);
    let evidence = budget
        .account(
            client,
            request,
            review
                .required_overhead_tokens
                .saturating_add(review.handoff_tokens),
            input_limit,
        )
        .await;
    if let Some(count) = evidence.provider {
        review.provider_input_tokens = Some(count.tokens());
        review.counting_method = count_method(Some(count));
        if count.tokens() <= input_limit {
            review.status = crate::runtime::HandoffCapacityStatus::Ready {
                target_tokens: review
                    .handoff_tokens
                    .saturating_add(input_limit - count.tokens()),
            };
        }
    }
    evidence
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    struct Counter {
        calls: AtomicUsize,
        answer: u8,
    }
    #[async_trait::async_trait]
    impl LlmClient for Counter {
        fn freeze_for_request(
            self: Arc<Self>,
        ) -> Result<Arc<dyn LlmClient>, phoenix_llm::LlmError> {
            Ok(self)
        }
        fn model_id(&self) -> &'static str {
            "counter"
        }
        async fn complete(
            &self,
            _: &LlmRequest,
        ) -> Result<phoenix_llm::LlmResponse, phoenix_llm::LlmError> {
            panic!("counting must not generate")
        }
        async fn count_input(
            &self,
            request: &LlmRequest,
        ) -> Result<Option<phoenix_llm::ProviderInputCount>, phoenix_llm::LlmError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            assert_eq!(request.max_tokens, 128_000);
            assert!(request.telemetry.is_none());
            match self.answer {
                0 => Ok(Some(phoenix_llm::ProviderInputCount::OpenAiCompleteInput {
                    tokens: 10,
                })),
                1 => Ok(None),
                2 => Err(phoenix_llm::LlmError::network("test")),
                _ => std::future::pending().await,
            }
        }
    }

    fn request() -> LlmRequest {
        LlmRequest {
            system: vec![phoenix_llm::SystemContent::cached("instructions")],
            messages: vec![],
            provider_replay: None,
            responses_replay: vec![],
            tool_availability: phoenix_core::domain::tool_availability::ToolAvailability::all(
                vec![],
            ),
            max_tokens: 128_000,
            effective_effort: phoenix_core::domain::llm_types::EffectiveEffort::native_unknown(),
            service_tier: phoenix_core::domain::llm_types::EffectiveServiceTier::Standard,
            telemetry: None,
            cache_key: phoenix_llm::PromptCacheKey::stable("count-test"),
        }
    }

    #[tokio::test]
    async fn counting_only_under_pressure_and_never_reuses_changed_candidate() {
        let counter = Counter {
            calls: AtomicUsize::new(0),
            answer: 0,
        };
        let mut budget = CountingBudget::new();
        let mut request = request();
        assert_eq!(
            budget
                .account(&counter, request.clone(), 50, 100)
                .await
                .occupancy(),
            50
        );
        assert_eq!(counter.calls.load(Ordering::SeqCst), 0);
        let evidence = budget.account(&counter, request.clone(), 150, 100).await;
        assert_eq!(evidence.local_prediction, 150);
        assert_eq!(evidence.occupancy(), 10);
        request
            .system
            .push(phoenix_llm::SystemContent::new("changed"));
        assert_eq!(
            budget
                .account(&counter, request.clone(), 175, 100)
                .await
                .occupancy(),
            175
        );
        assert_eq!(counter.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn counting_unavailable_failure_and_timeout_fallback_have_one_attempt() {
        for answer in 1..=3 {
            let counter = Counter {
                calls: AtomicUsize::new(0),
                answer,
            };
            let mut budget = CountingBudget::new();
            let request = request();
            assert_eq!(
                budget
                    .account(&counter, request.clone(), 150, 100)
                    .await
                    .occupancy(),
                150
            );
            assert_eq!(
                budget
                    .account(&counter, request.clone(), 120, 100)
                    .await
                    .occupancy(),
                120
            );
            assert_eq!(counter.calls.load(Ordering::SeqCst), 1);
        }
    }

    #[tokio::test]
    async fn provider_complete_count_can_rescue_local_required_overhead_conflict() {
        let counter = Counter {
            calls: AtomicUsize::new(0),
            answer: 0,
        };
        let mut budget = CountingBudget::new();
        let mut review = crate::runtime::HandoffCapacityReview {
            counting_method: crate::runtime::CapacityCountingMethod::ConservativeLocalEstimate,
            context_window_tokens: 150_000,
            required_overhead_tokens: 100_000,
            output_reservation_tokens: 128_000,
            successor_working_room_tokens: 8192,
            handoff_tokens: 20_000,
            provider_input_tokens: None,
            status: crate::runtime::HandoffCapacityStatus::RequiredOverhead,
        };
        account_handoff(&mut budget, &counter, request(), "protected source",
            phoenix_core::domain::product_conversation::ContinuationOpeningAuthority::GeneratedPredecessorContext, &mut review).await;
        assert!(review.fits());
        assert_eq!(review.provider_input_tokens, Some(10));
        assert_eq!(review.required_overhead_tokens, 100_000);
        assert_eq!(review.handoff_tokens, 20_000);
        assert_eq!(review.output_reservation_tokens, 128_000);
        assert_eq!(counter.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn full_output_reservation_distinguishes_reducible_history_from_fixed_conflict() {
        let capacity = RequestCapacity {
            context_window: 272_000,
            input_tokens: 150_000,
            output_tokens: 128_000,
        };
        assert_eq!(capacity.decide(10_000), CapacityDecision::NeedsCompaction);
        assert_eq!(capacity.decide(145_000), CapacityDecision::Conflict);
        assert_eq!(
            RequestCapacity {
                input_tokens: 144_000,
                ..capacity
            }
            .decide(10_000),
            CapacityDecision::Ready
        );
    }

    #[test]
    fn handoff_that_barely_fits_does_not_leave_working_room() {
        let successor = SuccessorCapacity {
            context_window: 272_000,
            ordinary_overhead_tokens: 20_000,
            output_tokens: 128_000,
        };
        assert_eq!(successor.working_room_tokens(), 31_000);
        assert_eq!(successor.handoff_budget().unwrap(), 93_000);
        assert!(successor.validate(124_000).is_err());
        assert!(successor.validate(115_808).is_err());
        assert!(successor.validate(93_000).is_ok());
        assert!(SuccessorCapacity {
            ordinary_overhead_tokens: usize::MAX,
            ..successor
        }
        .handoff_budget()
        .is_err());
    }
    #[test]
    #[allow(clippy::too_many_lines)] // one cold/warm/change/order/privacy sequence
    fn capacity_diagnostics_prefix_causes_are_multiple_and_content_free() {
        let mut request = request();
        request.messages.push(phoenix_llm::LlmMessage {
            source_message_id: Some("old".into()),
            role: phoenix_llm::MessageRole::User,
            content: vec![phoenix_llm::ContentBlock::text("private body")],
        });
        let counter = Counter {
            calls: AtomicUsize::new(0),
            answer: 1,
        };
        let identity = phoenix_llm::LlmRequestTelemetry {
            capacity_plan_id: None,
            conversation_id: "test".into(),
            root_conversation_id: "test".into(),
            request_id: "plan".into(),
            retry_attempt: 1,
            attempt_capture: phoenix_llm::LlmAttemptCapture::new(),
        };
        let accounting = InputAccounting::local(request.clone(), 100);
        let mut plan = accounting.plan(
            &identity,
            &counter,
            RequestPlanKind::Ordinary,
            272_000,
            100,
            InputReductionAction::None,
            RequestPlanOutcome::Ready,
            std::time::Instant::now(),
        );
        let mut switched_tracker = PrefixTracker::after_model_switch(true);
        switched_tracker.observe(&request, "new route", &mut plan, false, false, false);
        assert_eq!(plan.prefix_comparison, PrefixComparison::Unknown);
        assert_eq!(
            plan.prefix_change_causes,
            vec![PrefixChangeCause::ModelRouteSwitch]
        );
        switched_tracker.observe(&request, "new route", &mut plan, false, false, false);
        assert_eq!(plan.prefix_comparison, PrefixComparison::Unchanged);
        assert!(plan.prefix_change_causes.is_empty());
        plan.prefix_comparison = PrefixComparison::Unknown;
        let mut tracker = PrefixTracker::default();
        tracker.observe(
            &request,
            "https://secret:credential@private.example/account",
            &mut plan,
            false,
            false,
            false,
        );
        assert_eq!(plan.prefix_comparison, PrefixComparison::Unknown);
        request.messages.push(phoenix_llm::LlmMessage {
            source_message_id: Some("new".into()),
            role: phoenix_llm::MessageRole::User,
            content: vec![phoenix_llm::ContentBlock::text("new activity")],
        });
        tracker.observe(
            &request,
            "https://secret:credential@private.example/account",
            &mut plan,
            false,
            false,
            false,
        );
        assert_eq!(plan.prefix_comparison, PrefixComparison::Unchanged);
        assert!(plan.prefix_change_causes.is_empty());
        request.system.push(phoenix_llm::SystemContent::new(
            "refreshed private guidance",
        ));
        request.tool_availability =
            phoenix_core::domain::tool_availability::ToolAvailability::all(vec![
                phoenix_llm::ToolDefinition {
                    name: "tool".into(),
                    description: "private tool description".into(),
                    input_schema: serde_json::json!({"type":"object"}),
                    defer_loading: false,
                },
            ]);
        tracker.observe(
            &request,
            "different route secret",
            &mut plan,
            true,
            false,
            false,
        );
        assert_eq!(plan.prefix_comparison, PrefixComparison::Changed);
        for cause in [
            PrefixChangeCause::Clearing,
            PrefixChangeCause::GuidanceRefresh,
            PrefixChangeCause::ToolPolicyChange,
            PrefixChangeCause::ModelRouteSwitch,
        ] {
            assert!(plan.prefix_change_causes.contains(&cause));
        }
        request.messages.swap(0, 1);
        tracker.observe(
            &request,
            "different route secret",
            &mut plan,
            false,
            false,
            false,
        );
        assert_eq!(plan.prefix_comparison, PrefixComparison::Changed);
        request.messages.push(phoenix_llm::LlmMessage {
            source_message_id: None,
            role: phoenix_llm::MessageRole::User,
            content: vec![phoenix_llm::ContentBlock::text("source-less")],
        });
        tracker.observe(
            &request,
            "different route secret",
            &mut plan,
            false,
            false,
            false,
        );
        tracker.observe(
            &request,
            "different route secret",
            &mut plan,
            false,
            false,
            false,
        );
        assert_eq!(plan.prefix_comparison, PrefixComparison::Unknown);
        let persisted = serde_json::to_string(&plan).unwrap();
        for secret in [
            "private body",
            "credential",
            "private.example",
            "private guidance",
            "private tool description",
            "different route secret",
        ] {
            assert!(!persisted.contains(secret));
        }
    }
}

use phoenix_core::domain::request_capacity::{
    CountFallbackReason, InputCountingMethod, InputReductionAction, PrefixChangeCause,
    PrefixComparison, RequestCapacityPlan, RequestPlanKind, RequestPlanOutcome,
    SupersededInputCount,
};

fn tokens(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

impl InputAccounting {
    #[allow(clippy::too_many_arguments)] // one immutable request accounting projection
    pub(super) fn plan(
        &self,
        telemetry: &phoenix_llm::LlmRequestTelemetry,
        client: &dyn LlmClient,
        kind: RequestPlanKind,
        context_window: usize,
        initial_local: usize,
        reduction_action: InputReductionAction,
        outcome: RequestPlanOutcome,
        started: std::time::Instant,
    ) -> RequestCapacityPlan {
        let (route_class, connection_id) = client.diagnostic_route();
        RequestCapacityPlan {
            plan_id: self.plan_id.clone(),
            request_id: telemetry.request_id.clone(),
            retry_attempt: telemetry.retry_attempt,
            conversation_id: telemetry.conversation_id.clone(),
            root_conversation_id: telemetry.root_conversation_id.clone(),
            kind,
            model: client.model_id().to_owned(),
            route_class,
            connection_id,
            context_window_tokens: tokens(context_window),
            output_reservation_tokens: self.request.max_tokens,
            local_prediction_tokens: tokens(self.local_prediction),
            provider_input_tokens: self.provider.map(|v| tokens(v.tokens())),
            superseded_count: self.superseded_count.clone(),
            counting_method: count_method(self.provider),
            count_fallback: self.fallback.map(fallback_reason),
            reduction_action,
            reduced_local_tokens: tokens(initial_local.saturating_sub(self.local_prediction)),
            outcome,
            planning_duration_ms: tokens(
                started
                    .elapsed()
                    .as_millis()
                    .try_into()
                    .unwrap_or(usize::MAX),
            ),
            prefix_comparison: PrefixComparison::Unknown,
            prefix_change_causes: vec![],
        }
    }
}

#[derive(Default)]
pub(super) struct PrefixTracker {
    previous: Option<PrefixSnapshot>,
    pending_model_switch: bool,
}
struct PrefixSnapshot {
    system: u64,
    tools: u64,
    route: u64,
    messages: Vec<(String, u64)>,
    complete_public_prefix: bool,
}
fn fingerprint(value: &impl serde::Serialize) -> u64 {
    use std::hash::Hasher;
    struct HashWriter(std::collections::hash_map::DefaultHasher);
    impl std::io::Write for HashWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.write(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = HashWriter(std::collections::hash_map::DefaultHasher::new());
    serde_json::to_writer(&mut writer, value).expect("request component serializes");
    writer.0.finish()
}

impl PrefixTracker {
    pub(super) fn after_model_switch(changed: bool) -> Self {
        Self {
            previous: None,
            pending_model_switch: changed,
        }
    }

    pub(super) fn observe(
        &mut self,
        request: &LlmRequest,
        route: &str,
        plan: &mut RequestCapacityPlan,
        clearing: bool,
        compaction: bool,
        readable_projection: bool,
    ) {
        let next = PrefixSnapshot {
            system: fingerprint(
                &request
                    .system
                    .iter()
                    .map(|s| (&s.text, s.cache))
                    .collect::<Vec<_>>(),
            ),
            tools: fingerprint(&(
                request.tool_availability.declarations(),
                request.tool_availability.callable_names(),
                format!("{:?}", request.tool_availability.anthropic_changes()),
            )),
            route: fingerprint(&route),
            complete_public_prefix: request
                .messages
                .iter()
                .all(|m| m.source_message_id.is_some())
                && request.provider_replay.is_none()
                && request.responses_replay.is_empty(),
            messages: request
                .messages
                .iter()
                .filter_map(|m| {
                    m.source_message_id
                        .as_ref()
                        .map(|id| (id.clone(), fingerprint(&m.content)))
                })
                .collect(),
        };
        let mut causes = vec![];
        if std::mem::take(&mut self.pending_model_switch) {
            causes.push(PrefixChangeCause::ModelRouteSwitch);
        }
        if clearing {
            causes.push(PrefixChangeCause::Clearing);
        }
        if compaction && self.previous.is_none() {
            causes.push(PrefixChangeCause::Compaction);
            if readable_projection {
                causes.push(PrefixChangeCause::ReadableProjection);
            }
        }
        if let Some(previous) = &self.previous {
            if previous.system != next.system {
                causes.push(PrefixChangeCause::GuidanceRefresh);
            }
            if previous.tools != next.tools {
                causes.push(PrefixChangeCause::ToolPolicyChange);
            }
            if previous.route != next.route {
                causes.push(PrefixChangeCause::ModelRouteSwitch);
            }
            let changed = previous.system != next.system
                || previous.tools != next.tools
                || previous.route != next.route
                || next.messages.get(..previous.messages.len())
                    != Some(previous.messages.as_slice());
            plan.prefix_comparison = if changed {
                PrefixComparison::Changed
            } else if previous.complete_public_prefix && next.complete_public_prefix {
                PrefixComparison::Unchanged
            } else {
                PrefixComparison::Unknown
            };
        }
        plan.prefix_change_causes = causes;
        self.previous = Some(next);
    }
}

impl InputAccounting {
    pub(super) fn projection_plan(
        &self,
        conversation_id: &str,
        root_conversation_id: &str,
        client: &dyn LlmClient,
        context_window: usize,
        fits: bool,
        started: std::time::Instant,
    ) -> RequestCapacityPlan {
        self.plan(
            &phoenix_llm::LlmRequestTelemetry {
                capacity_plan_id: None,
                conversation_id: conversation_id.to_owned(),
                root_conversation_id: root_conversation_id.to_owned(),
                request_id: format!("successor-projection:{}", uuid::Uuid::new_v4()),
                retry_attempt: 1,
                attempt_capture: phoenix_llm::LlmAttemptCapture::new(),
            },
            client,
            RequestPlanKind::SuccessorProjection,
            context_window,
            self.local_prediction,
            InputReductionAction::None,
            if fits {
                RequestPlanOutcome::Ready
            } else {
                RequestPlanOutcome::CapacityConflict
            },
            started,
        )
    }
}

fn count_method(count: Option<phoenix_llm::ProviderInputCount>) -> InputCountingMethod {
    match count {
        None => InputCountingMethod::ConservativeLocalEstimate,
        Some(phoenix_llm::ProviderInputCount::OpenAiCompleteInput { .. }) => {
            InputCountingMethod::OpenAiCompleteInput
        }
        Some(phoenix_llm::ProviderInputCount::AnthropicEstimate { .. }) => {
            InputCountingMethod::AnthropicEstimate
        }
    }
}
fn fallback_reason(reason: phoenix_llm::InputCountFallback) -> CountFallbackReason {
    match reason {
        phoenix_llm::InputCountFallback::LocalReady => CountFallbackReason::LocalReady,
        phoenix_llm::InputCountFallback::BudgetSpent => CountFallbackReason::BudgetSpent,
        phoenix_llm::InputCountFallback::Unavailable => CountFallbackReason::Unavailable,
        phoenix_llm::InputCountFallback::Failed => CountFallbackReason::Failed,
        phoenix_llm::InputCountFallback::TimedOut => CountFallbackReason::TimedOut,
    }
}
