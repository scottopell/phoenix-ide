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
    request: LlmRequest,
    pub(super) local_prediction: usize,
    pub(super) provider: Option<phoenix_llm::ProviderInputCount>,
    pub(super) fallback: Option<phoenix_llm::InputCountFallback>,
}

impl InputAccounting {
    pub(super) fn into_request(self) -> LlmRequest {
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
                tracing::warn!(kind = ?error.kind, "input counting failed; using conservative local planning")
            }
            Err(_) => {
                accounting.fallback = Some(phoenix_llm::InputCountFallback::TimedOut);
                tracing::warn!("input counting deadline elapsed; using conservative local planning")
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
) {
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
        review.counting_method = match count {
            phoenix_llm::ProviderInputCount::OpenAiCompleteInput { .. } => {
                crate::runtime::CapacityCountingMethod::OpenAiCompleteInput
            }
            phoenix_llm::ProviderInputCount::AnthropicEstimate { .. } => {
                crate::runtime::CapacityCountingMethod::AnthropicEstimate
            }
        };
        if count.tokens() <= input_limit {
            review.status = crate::runtime::HandoffCapacityStatus::Ready {
                target_tokens: review
                    .handoff_tokens
                    .saturating_add(input_limit - count.tokens()),
            };
        }
    }
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
        fn model_id(&self) -> &str {
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
        let counted = budget.account(&counter, request.clone(), 150, 100).await;
        assert_eq!(counted.local_prediction, 150);
        assert_eq!(counted.occupancy(), 10);
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
}
