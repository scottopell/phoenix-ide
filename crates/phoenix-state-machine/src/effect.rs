//! Effects produced by state transitions

use crate::state::{
    AssistantMessage, ContinuationSummaryRequest, SubAgentOutcome, SubAgentResult, ToolCall,
};
use chrono::{DateTime, Utc};
use phoenix_bash_display::display_command;
use phoenix_core::domain::db_schema::{
    FileAttachment, ImageData, MessageContent, ToolResult, UsageData,
};
use phoenix_core::domain::llm_error_kind::LlmAttemptReason;
use phoenix_core::domain::llm_types::ContentBlock;
use serde_json::Value;
use std::fmt;
use std::path::Path;
use std::time::Duration;

// ============================================================================
// CheckpointData — atomic persistence gate (REQ-BED-007, FM-2 Prevention)
// ============================================================================

/// Data to persist atomically. The `ToolRound` variant enforces that assistant
/// messages and tool results are always written together — half-written history
/// is structurally unrepresentable.
#[derive(Debug, Clone)]
pub enum CheckpointData {
    /// A complete tool round: assistant message + all tool results.
    /// Constructor enforces matching counts.
    ToolRound {
        assistant_message: AssistantMessage,
        tool_results: Vec<ToolResult>,
    },
}

/// Errors from `CheckpointData` constructors
#[derive(Debug, Clone)]
pub enum PersistError {
    ResultCountMismatch { tool_uses: usize, results: usize },
}

impl fmt::Display for PersistError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PersistError::ResultCountMismatch { tool_uses, results } => {
                write!(
                    f,
                    "tool_use count ({tool_uses}) != tool_result count ({results})"
                )
            }
        }
    }
}

impl CheckpointData {
    /// Construct a `ToolRound` checkpoint, enforcing that the number of
    /// `tool_use` blocks in the assistant message matches the number of
    /// tool results.
    ///
    /// # Errors
    ///
    /// Returns [`PersistError::ResultCountMismatch`] when the `tool_use` count
    /// in the assistant message differs from the number of tool results.
    pub fn tool_round(
        assistant_message: AssistantMessage,
        tool_results: Vec<ToolResult>,
    ) -> Result<Self, PersistError> {
        let tool_use_count = assistant_message.tool_uses().len();
        if tool_use_count != tool_results.len() {
            return Err(PersistError::ResultCountMismatch {
                tool_uses: tool_use_count,
                results: tool_results.len(),
            });
        }
        Ok(Self::ToolRound {
            assistant_message,
            tool_results,
        })
    }
}

/// Derive the durable message ID used to persist one tool result.
///
/// Provider `tool_use_id` values are protocol correlation keys and may be reused
/// across assistant turns. Durable message identity is therefore derived from the
/// Phoenix-owned assistant message plus the tool's ordinal within that turn.
#[must_use]
pub fn tool_result_message_id(assistant_message_id: &str, tool_ordinal: usize) -> String {
    format!("{assistant_message_id}-tool-result-{tool_ordinal}")
}

/// Effects to be executed after state transition
#[derive(Debug, Clone)]
pub enum Effect {
    /// Persist a message to the database
    PersistMessage {
        content: MessageContent,
        display_data: Option<Value>,
        usage_data: Option<UsageData>,
        /// The canonical message identifier (client-generated for user messages,
        /// server-generated for agent/tool messages)
        message_id: String,
        /// If true, skip the insert (and broadcast) when a message with this
        /// `message_id` already exists. Set only by code paths where the same
        /// effect may be re-emitted after crash recovery (e.g., steering-queue
        /// re-drain). Default `false` for normal write paths to avoid the
        /// extra `message_exists` query.
        idempotent: bool,
    },

    /// Persist the new state
    PersistState,

    /// Make an LLM request
    RequestLlm,

    /// Execute a tool (spawns as background task)
    ExecuteTool { tool: ToolCall },

    /// Eagerly broadcast the assistant message to SSE clients before tools run,
    /// so the UI can render the in-flight `tool_use` blocks during execution.
    /// Pairs with the later `PersistCheckpoint` that performs the DB write —
    /// the UI dedups the duplicate `sse_message` by `message_id`.
    /// Broadcast-only on purpose: persisting eagerly would create half-written
    /// history (`tool_use` without `tool_result`) that the LLM history builder
    /// and crash recovery do not expect.
    BroadcastAssistantMessage { message: AssistantMessage },

    /// Abort the currently running tool
    AbortTool { tool_use_id: String },

    /// Abort the currently running LLM request
    AbortLlm,

    /// Cancel all pending sub-agents
    CancelSubAgents { ids: Vec<String> },

    /// Notify parent of sub-agent completion (sub-agent only)
    NotifyParent { outcome: SubAgentOutcome },

    /// Notify connected clients that conversation state changed. The executor
    /// reconstructs the wire payload from the authoritative `self.state`, so
    /// this variant deliberately carries no payload — a label here would be a
    /// parallel representation of the state the executor already serializes.
    NotifyStateChange,

    /// Notify connected clients that the agent finished its turn (parent only).
    NotifyAgentDone,

    /// Schedule a retry after a retryable LLM error. Carries the
    /// classified `reason` and any quota-reset timestamp from the
    /// upstream error so the executor can emit `SseEvent::LlmAttempt`
    /// (specs/llm-retry-visibility/, REQ-LRV-001) immediately before
    /// spawning the backoff sleep, surfacing retry context to the
    /// client during the otherwise-silent backoff window.
    ScheduleRetry {
        delay: Duration,
        attempt: u32,
        reason: LlmAttemptReason,
        resets_at: Option<DateTime<Utc>>,
    },

    /// Atomically persist a complete checkpoint (REQ-BED-007, FM-2 Prevention)
    PersistCheckpoint { data: CheckpointData },

    /// Persist multiple tool results at once.
    /// Retained for sub-agent result persistence; normal tool rounds use `PersistCheckpoint`.
    #[allow(dead_code)]
    PersistToolResults { results: Vec<ToolResult> },

    /// Persist a UI-hidden system marker.
    ///
    /// System content is ignored by LLM history construction, while the non-empty
    /// display text keeps recovery heuristics from treating the previous tool
    /// result as an interrupted turn after runtime recreation.
    PersistHiddenSystemMarker {
        marker: &'static str,
        message_id: String,
    },

    /// Persist aggregated sub-agent results as a message
    PersistSubAgentResults {
        results: Vec<SubAgentResult>,
        /// Provider `tool_use_id` of `spawn_agents`, retained in the tool-result content.
        spawn_tool_id: Option<String>,
        /// Durable message id for the `spawn_agents` placeholder row to update.
        spawn_tool_result_message_id: Option<String>,
    },

    /// Request continuation summary from LLM (no tools) - REQ-BED-020
    RequestContinuation { request: ContinuationSummaryRequest },

    /// Notify client of context exhaustion - REQ-BED-021
    NotifyContextExhausted { summary: String },

    /// Execute git operations for task approval (REQ-BED-028).
    ///
    /// `task_file` (relative to the conversation cwd) is the canonical
    /// source: the executor reads it from disk to derive task id, slug,
    /// priority, and status, then sets up the branch and worktree. The
    /// remaining fields are the snapshot the user approved and are used for
    /// the user-facing branch announcement message.
    ApproveTask {
        task_file: String,
        title: String,
        priority: phoenix_core::task_source::Priority,
        plan: String,
    },
    ApproveTaskFreshHandoff {
        task_file: String,
        title: String,
        priority: phoenix_core::task_source::Priority,
        plan: String,
    },
    /// Atomically persist a decoupled fork proposal together with the
    /// originating turn's tool round (REQ-PROJ-033). The synthetic success ack
    /// in `checkpoint` and the `fork_proposals` row commit in a single
    /// transaction: the ack ("recorded — pending review") must never be durable
    /// without the row the review/approve surface reads. This replaces the
    /// separate `PersistCheckpoint` on the fork path — the fork arm emits this
    /// instead, never both.
    ///
    /// `task_file` is the working-dir-relative path from `resolve_task_file`;
    /// the executor normalizes it to repository-relative before insert.
    PersistForkProposal {
        proposal_id: String,
        task_file: String,
        title: String,
        priority: phoenix_core::task_source::Priority,
        body: String,
        checkpoint: CheckpointData,
    },
    /// Task completed or abandoned: finalize conversation state, mode, and cwd.
    /// Executor calls `finalize_conversation`, injects system message, broadcasts SSE.
    ResolveTask {
        system_message: String,
        repo_root: String,
    },

    /// Remove the specified drained entries from the persisted steering queue.
    /// Emitted by `SteerDrainedUserMessages` transition arms AFTER all
    /// `PersistMessage` + `PersistState` effects so that a crash before this
    /// effect runs leaves the queue intact for re-drain on restart (idempotent
    /// persist guards against double-delivery). Removing only the drained ids
    /// (rather than overwriting with empty) preserves concurrently-enqueued
    /// steers that arrived during the drain window.
    ClearSteeringQueueEntries { message_ids: Vec<String> },
}

impl Effect {
    #[allow(clippy::too_many_arguments)]
    pub fn persist_user_message(
        text: impl Into<String>,
        llm_text: Option<String>,
        images: Vec<ImageData>,
        files: Vec<FileAttachment>,
        message_id: String,
        user_agent: Option<String>,
        skill_invocation: Option<phoenix_core::domain::skill_invocation::SkillInvocation>,
        idempotent: bool,
    ) -> Self {
        let text = text.into();
        let content = if let Some(invocation) = skill_invocation {
            MessageContent::Skill(phoenix_core::domain::db_schema::SkillContent {
                name: invocation.name,
                body: invocation.body,
                trigger: text,
                files,
            })
        } else {
            match llm_text {
                Some(expanded) => MessageContent::User(
                    phoenix_core::domain::db_schema::UserContent::with_expansion(
                        text, expanded, images, files,
                    ),
                ),
                None => {
                    if images.is_empty() && files.is_empty() {
                        MessageContent::user(text)
                    } else {
                        MessageContent::user_with_attachments(text, images, files)
                    }
                }
            }
        };
        // Store user_agent in display_data for UI to show device icon
        let display_data = user_agent.map(|ua| serde_json::json!({ "user_agent": ua }));
        Effect::PersistMessage {
            content,
            display_data,
            usage_data: None,
            message_id,
            idempotent,
        }
    }

    /// Create an agent message effect with display data computed for bash commands.
    ///
    /// The `cwd` parameter is used to determine whether to strip cd prefixes from
    /// bash commands in the display (REQ-BASH-011).
    ///
    /// `final_attempt` is the `attempt` field of the `LlmRequesting` /
    /// `AwaitingContinuation` state being transitioned out of — i.e. how
    /// many tries the LLM took to produce this response. The helper
    /// stamps `retry_count = saturating_sub(1)` into the message's
    /// `display_data` so the UI can render a post-hoc `(retried Nx)`
    /// badge on the persisted assistant message (specs/llm-retry-visibility/
    /// REQ-LRV-006). 1 means "succeeded on first try"; the badge is
    /// hidden in that case.
    #[must_use]
    pub fn persist_agent_message(
        blocks: Vec<ContentBlock>,
        usage: Option<UsageData>,
        cwd: &Path,
        message_id: String,
        final_attempt: u32,
    ) -> Self {
        let mut display_data = compute_bash_display_data(&blocks, cwd);
        let retry_count = final_attempt.saturating_sub(1);
        if retry_count > 0 {
            // Only stamp the key when we actually retried — keeps the
            // persisted JSON minimal for the common no-retry case and
            // lets the UI's `display_data?.retry_count > 0` check
            // double as a `display_data?.retry_count !== undefined`
            // guard.
            let display_obj =
                display_data.get_or_insert_with(|| Value::Object(serde_json::Map::new()));
            if let Some(map) = display_obj.as_object_mut() {
                map.insert(
                    "retry_count".to_string(),
                    Value::Number(serde_json::Number::from(retry_count)),
                );
            }
        }
        Effect::PersistMessage {
            content: MessageContent::agent(blocks),
            display_data,
            usage_data: usage,
            message_id,
            idempotent: false,
        }
    }

    #[must_use]
    pub fn notify_state_change() -> Self {
        Effect::NotifyStateChange
    }

    #[must_use]
    pub fn notify_agent_done() -> Self {
        Effect::NotifyAgentDone
    }

    #[must_use]
    pub fn execute_tool(tool: ToolCall) -> Self {
        Effect::ExecuteTool { tool }
    }

    /// Create a continuation message effect
    pub fn persist_continuation_message(summary: impl Into<String>) -> Self {
        let summary = summary.into();
        Effect::PersistMessage {
            content: MessageContent::continuation(summary.clone()),
            display_data: Some(serde_json::json!({ "summary": summary })),
            usage_data: None,
            message_id: uuid::Uuid::new_v4().to_string(),
            idempotent: false,
        }
    }
}

/// Compute display data for bash commands in content blocks.
///
/// For each bash `tool_use` block, computes a simplified display string
/// using `display_command()` which strips cd prefixes when they match cwd.
///
/// Returns `Some(json)` with display info if there are bash commands,
/// `None` otherwise.
#[must_use]
pub fn compute_bash_display_data(blocks: &[ContentBlock], cwd: &Path) -> Option<Value> {
    let cwd_str = cwd.to_string_lossy();
    let mut bash_displays: Vec<Value> = Vec::new();

    for block in blocks {
        if let ContentBlock::ToolUse { id, name, input } = block {
            if name == "bash" {
                let display = serde_json::from_value::<
                    phoenix_core::domain::bash_types::BashToolInput,
                >(input.clone())
                .ok()
                .and_then(|input| bash_input_display(&input, &cwd_str))
                .or_else(|| legacy_bash_input_display(input, &cwd_str));
                if let Some(display) = display {
                    bash_displays.push(serde_json::json!({
                        "tool_use_id": id,
                        "display": display
                    }));
                }
            }
        }
    }

    if bash_displays.is_empty() {
        None
    } else {
        Some(serde_json::json!({ "bash": bash_displays }))
    }
}

fn bash_input_display(
    input: &phoenix_core::domain::bash_types::BashToolInput,
    cwd: &str,
) -> Option<String> {
    match input.op {
        phoenix_core::domain::bash_types::BashOp::Run => {
            input.cmd.as_deref().map(|cmd| display_command(cmd, cwd))
        }
        phoenix_core::domain::bash_types::BashOp::Peek => input
            .handle
            .as_deref()
            .map(|handle| format!("peek {handle}")),
        phoenix_core::domain::bash_types::BashOp::Wait => input.handle.as_deref().map(|handle| {
            let suffix = input
                .wait_seconds
                .map(|seconds| format!(" (up to {seconds}s)"))
                .unwrap_or_default();
            format!("wait {handle}{suffix}")
        }),
        phoenix_core::domain::bash_types::BashOp::Kill => input.handle.as_deref().map(|handle| {
            let signal = input.signal.map_or(
                "TERM",
                phoenix_core::domain::kill_signal::KillSignal::as_str,
            );
            format!("kill {handle} ({signal})")
        }),
    }
}
fn legacy_bash_input_display(input: &Value, cwd: &str) -> Option<String> {
    input
        .get("command")
        .or_else(|| input.get("cmd"))
        .and_then(Value::as_str)
        .map(|cmd| display_command(cmd, cwd))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_bash_display_data_strips_cwd_for_legacy_command() {
        let blocks = vec![ContentBlock::ToolUse {
            id: "tool-1".to_string(),
            name: "bash".to_string(),
            input: serde_json::json!({ "command": "cd /repo && cargo test" }),
        }];

        let display = compute_bash_display_data(&blocks, Path::new("/repo")).unwrap();
        assert_eq!(display["bash"][0]["display"], "cargo test");
    }
}
