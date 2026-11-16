//! Bounded own-Coordinator transcript capture. REQ-SCAN-001 through REQ-SCAN-008.
use crate::coordinator_tracking::{TrackingDisposition, TrackingKind};
use phoenix_core::domain::db_schema::InputOrigin;
use serde::{Deserialize, Serialize};

mod apply;
mod source;
mod storage;

pub const SCAN_MODEL: &str = "gpt-6-luna";
pub const SCAN_MAX_INPUT_BYTES: usize = 24 * 1024;
pub const SCAN_MAX_OUTPUT_TOKENS: u32 = 2048;
pub const SCAN_MAX_OUTPUT_BYTES: usize = 16 * 1024;
pub const SCAN_TIMEOUT_SECONDS: u64 = 30;
pub const SCAN_RETRY_BACKOFF_US: i64 = 300_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanConnection {
    Codex,
    #[serde(rename = "openai_responses")]
    OpenAiResponses,
}
impl ScanConnection {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::OpenAiResponses => "openai_responses",
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanProfile {
    pub connection: ScanConnection,
    pub endpoint_hash: [u8; 32],
    pub prompt_hash: [u8; 32],
    pub instruction_bytes: usize,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanLane {
    Live,
    Historical,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanRole {
    User,
    Assistant,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ScanPosition {
    Boundary {
        sequence_id: i64,
    },
    Fragment {
        sequence_id: i64,
        message_id: String,
        part_ordinal: usize,
        utf8_offset: usize,
        prose_digest: String,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanContext {
    pub lane: ScanLane,
    pub new_capture_freshness: ScanFreshness,
    pub captured_at_us: i64,
    pub transcript_id: String,
    pub source_generation: i64,
    pub registry_revision: i64,
    pub omitted_active_candidates: usize,
    pub omitted_retired_candidates: usize,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanFreshness {
    Unverified,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanFragment {
    pub fragment_id: String,
    pub message_id: String,
    pub role: ScanRole,
    pub origin: InputOrigin,
    pub is_meta: bool,
    pub created_at_us: i64,
    pub part_ordinal: usize,
    pub utf8_start: usize,
    pub utf8_end: usize,
    pub complete_message: bool,
    pub prose_digest: String,
    pub text: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanCandidate {
    pub id: String,
    pub kind: TrackingKind,
    pub disposition: TrackingDisposition,
    pub title: String,
    pub title_truncated: bool,
    pub context_hint: String,
    pub context_hint_truncated: bool,
    pub retired_at_us: Option<i64>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanInput {
    pub scan_context: ScanContext,
    pub fragments: Vec<ScanFragment>,
    pub candidates: Vec<ScanCandidate>,
}
#[derive(Debug)]
pub struct PreparedScanBatch {
    pub(crate) input: ScanInput,
    pub(crate) binding_id: String,
    pub(crate) profile: ScanProfile,
    pub(crate) start: ScanPosition,
    pub(crate) end: ScanPosition,
    pub(crate) frontier_revision: i64,
    pub(crate) source_digest: String,
    pub(crate) owner_product_id: String,
}
impl PreparedScanBatch {
    #[must_use]
    pub fn input(&self) -> &ScanInput {
        &self.input
    }
    #[must_use]
    pub fn binding_id(&self) -> &str {
        &self.binding_id
    }
}
#[derive(Debug)]
pub struct ReservedScanAttempt {
    pub(crate) id: i64,
    pub(crate) batch: PreparedScanBatch,
}
impl ReservedScanAttempt {
    #[must_use]
    pub const fn attempt_id(&self) -> i64 {
        self.id
    }
    #[must_use]
    pub fn input(&self) -> &ScanInput {
        self.batch.input()
    }
    #[must_use]
    pub fn binding_id(&self) -> &str {
        self.batch.binding_id()
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanBlocker {
    RouteUnavailable,
    GlobalBudgetPaused,
    HistoricalBudgetPaused,
    RetryBackoff,
    InvalidOutput,
    SourceOverflow,
    SourceUnavailable,
    InferenceFailed,
}
#[derive(Debug)]
pub enum ScanPreparation {
    Ready(PreparedScanBatch),
    Progress,
    Wait {
        reason: ScanBlocker,
        next_at_us: Option<i64>,
    },
    Idle,
}
#[derive(Debug)]
pub enum ScanReservation {
    Reserved(ReservedScanAttempt),
    Paused {
        reason: ScanBlocker,
        next_at_us: Option<i64>,
    },
    Conflict,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScanQuote {
    pub fragment_id: String,
    pub quotation: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScanAction {
    New {
        kind: TrackingKind,
        title: String,
        note: String,
        resumption: String,
        quotes: Vec<ScanQuote>,
    },
    Append {
        item_id: String,
        quotes: Vec<ScanQuote>,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScanResult {
    Complete { actions: Vec<ScanAction> },
}
#[derive(Debug, Clone, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum ScanRejection {
    #[error("invalid scanner output: {detail}")]
    InvalidOutput { detail: String },
    #[error("invalid scanner quote: {detail}")]
    InvalidQuote { detail: String },
    #[error("invalid scanner capture: {detail}")]
    InvalidCapture { detail: String },
}
#[derive(Debug)]
pub enum ScanApplyOutcome {
    Applied {
        created_item_ids: Vec<String>,
        changed_item_count: usize,
        registry_revision: i64,
    },
    Replayed,
    Conflict,
    Rejected(ScanRejection),
}
#[derive(Debug, Clone)]
pub enum ScanFailure {
    Provider(String),
    Timeout,
    InvalidOutput(String),
    Storage(String),
}
#[derive(Debug, Clone)]
pub struct ScanUsage {
    pub input_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_creation_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
}
#[derive(Debug, Clone)]
pub struct ScanMetrics {
    pub elapsed_ms: u64,
    pub usage: Option<ScanUsage>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanCoverageState {
    PendingInitial,
    GenerationStale,
    Incomplete,
    Covered,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanMemberCoverage {
    pub transcript_id: String,
    pub continuation_ordinal: i64,
    pub source_generation: i64,
    pub covered_generation: Option<i64>,
    pub live_boundary: i64,
    pub historical_boundary: Option<i64>,
    pub live_frontier: Option<ScanPosition>,
    pub historical_frontier: Option<ScanPosition>,
    pub state: ScanCoverageState,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanCoverageSummary {
    pub enrolled_members: usize,
    pub pending_live_members: usize,
    pub pending_historical_members: usize,
    pub generation_stale_members: usize,
    pub blocker: Option<ScanBlocker>,
    pub next_resume_at_us: Option<i64>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanCoveragePage {
    pub captured_at_us: i64,
    pub summary: ScanCoverageSummary,
    pub members: Vec<ScanMemberCoverage>,
    pub has_more: bool,
    pub next_member: Option<i64>,
    pub omitted_members: usize,
}

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanOriginKind {
    UnknownHistorical,
    UserApi,
    InternalConversation,
    SystemGenerated,
    SlackGateway,
    RemoteApi,
    RemoteConversation,
    RemoteCoordinator,
    SubscriptionEvent,
    ScheduledReminder,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanEvidenceProvenance {
    pub evidence_ordinal: i64,
    pub origin_kind: ScanOriginKind,
    pub source_role: ScanRole,
    pub source_generation: i64,
    pub utf8_start: i64,
    pub utf8_end: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanRecordingSource {
    pub reserved_at_us: i64,
    pub lane: ScanLane,
    pub source_generation: i64,
    pub model: String,
    pub connection: ScanConnection,
    pub prompt_hash: String,
    pub endpoint_hash: String,
}
