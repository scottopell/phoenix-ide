//! Content-free facts tied to one immutable prepared request.
use serde::{Deserialize, Serialize};

macro_rules! fact_enum {
    ($name:ident { $($variant:ident),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
        #[serde(rename_all = "snake_case")]
        #[ts(export, export_to = "../../../ui/src/generated/")]
        pub enum $name { $($variant),+ }
    };
}
fact_enum!(RequestPlanKind {
    Ordinary,
    Compaction,
    HandoffShortening,
    SuccessorProjection
});
fact_enum!(RequestPlanOutcome {
    Ready,
    NeedsCompaction,
    CapacityConflict
});
fact_enum!(InputCountingMethod {
    ConservativeLocalEstimate,
    OpenAiCompleteInput,
    AnthropicEstimate
});
fact_enum!(CountFallbackReason {
    LocalReady,
    BudgetSpent,
    Unavailable,
    Failed,
    TimedOut
});
fact_enum!(InputReductionAction {
    None,
    Clearing,
    BoundedHistory
});
fact_enum!(PrefixChangeCause {
    Clearing,
    Compaction,
    GuidanceRefresh,
    ModelRouteSwitch,
    ToolPolicyChange,
    ReadableProjection
});
fact_enum!(PrefixComparison {
    Unknown,
    Unchanged,
    Changed
});
fact_enum!(SafeRouteClass {
    Unknown,
    AnthropicDirect,
    AnthropicCompatible,
    OpenAiResponsesDirect,
    CodexAuth,
    OpenAiResponsesCompatible,
    ChatCompletionsDirect,
    ChatCompletionsCompatible
});

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct RequestCapacityPlan {
    pub plan_id: String,
    pub request_id: String,
    pub retry_attempt: u32,
    pub conversation_id: String,
    pub root_conversation_id: String,
    pub kind: RequestPlanKind,
    pub model: String,
    pub route_class: SafeRouteClass,
    pub connection_id: Option<String>,
    pub context_window_tokens: u32,
    pub output_reservation_tokens: u32,
    pub local_prediction_tokens: u32,
    pub provider_input_tokens: Option<u32>,
    pub superseded_count: Option<SupersededInputCount>,
    pub counting_method: InputCountingMethod,
    pub count_fallback: Option<CountFallbackReason>,
    pub reduction_action: InputReductionAction,
    pub reduced_local_tokens: u32,
    pub outcome: RequestPlanOutcome,
    pub planning_duration_ms: u32,
    pub prefix_comparison: PrefixComparison,
    pub prefix_change_causes: Vec<PrefixChangeCause>,
}

impl RequestCapacityPlan {
    #[must_use]
    pub fn retained_input_tokens(&self) -> u32 {
        self.provider_input_tokens
            .unwrap_or(self.local_prediction_tokens)
    }
    #[must_use]
    pub fn count_is_estimated(&self) -> bool {
        self.counting_method != InputCountingMethod::OpenAiCompleteInput
    }
}

/// Counting evidence for an earlier candidate, never compared with dispatched usage.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct SupersededInputCount {
    pub local_prediction_tokens: u32,
    pub provider_input_tokens: Option<u32>,
    pub counting_method: InputCountingMethod,
    pub count_fallback: Option<CountFallbackReason>,
}
