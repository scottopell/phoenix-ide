use super::wire::EnrichedMessage;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct PersistedMember {
    pub id: String,
    pub slug: Option<String>,
    pub title: Option<String>,
    pub cwd: String,
    pub parent_conversation_id: Option<String>,
    pub user_initiated: bool,
    #[ts(type = "unknown")]
    pub state: serde_json::Value,
    pub state_updated_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub archived: bool,
    pub model: Option<String>,
    pub effort: Option<phoenix_core::domain::llm_types::ModelEffort>,
    pub service_tier: phoenix_core::domain::llm_types::ServiceTier,
    pub project_id: Option<String>,
    pub message_count: i64,
    pub transcript_generation: i64,
    pub seed_parent_id: Option<String>,
    pub seed_label: Option<String>,
    pub continued_in_conv_id: Option<String>,
    pub chain_name: Option<String>,
    #[serde(flatten)]
    pub metadata: PersistedMemberMetadata,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct PersistedMemberMetadata {
    pub conv_mode_label: String,
    pub branch_name: Option<String>,
    pub worktree_path: Option<String>,
    pub base_branch: Option<String>,
    pub task_title: Option<String>,
    pub work_scope_key: String,
    pub seed_parent_slug: Option<String>,
    pub parent_conversation_slug: Option<String>,
    pub cached_pr: Option<crate::runtime::CachedPrSummary>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum MemberObservationSource {
    Persisted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "availability", rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum MemberLiveProcessFacts {
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum PersistedTranscriptCoverage {
    Complete,
    Tail,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct PersistedMemberResponse {
    pub source: MemberObservationSource,
    pub conversation: PersistedMember,
    pub messages: Vec<EnrichedMessage>,
    pub transcript_coverage: PersistedTranscriptCoverage,
    pub server_message_tail: Option<i64>,
    pub persisted_agent_working: bool,
    pub live_process: MemberLiveProcessFacts,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct PersistedMemberQuery {
    pub limit: Option<i64>,
}
