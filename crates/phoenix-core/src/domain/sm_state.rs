//! Conversation state types

use crate::domain::bash_types::BashToolInput;
use crate::domain::db_schema::{ErrorKind, ToolResult, UsageData};
use crate::domain::llm_types::ContentBlock;
use crate::domain::patch_types::PatchInput;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use std::time::Duration;

// ============================================================================
// Tool Input Types - Strongly typed inputs for each tool
// ============================================================================

/// Input for the think tool
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThinkInput {
    pub thoughts: String,
}

/// Input for the `keyword_search` tool
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeywordSearchInput {
    pub query: String,
    pub search_terms: Vec<String>,
}

/// Input for the `read_image` tool
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadImageInput {
    pub path: String,
}

/// Task specification for `spawn_agents` tool
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubAgentTask {
    pub task: String,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub mode: Option<SubAgentMode>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub max_turns: Option<u32>,
    /// Named agent persona to spawn (see `specs/agents/`). Must match a
    /// discovered agent name; supplies the sub-agent's persona and its
    /// default model/mode.
    #[serde(default)]
    pub agent_type: Option<String>,
}

/// Input for the `spawn_agents` tool (parent only)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpawnAgentsInput {
    pub tasks: Vec<SubAgentTask>,
}

/// Input for the `submit_result` tool (sub-agent only)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmitResultInput {
    pub result: String,
}

/// Input for the `submit_error` tool (sub-agent only)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmitErrorInput {
    pub error: String,
}

/// Input for the `commission_review` tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommissionReviewInput {
    pub brief: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus: Option<String>,
    #[serde(default)]
    pub allow_dirty_working_tree: bool,
}

/// Runtime-owned execution payload for an approved `commission_review` request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovedCommissionReviewInput {
    #[serde(flatten)]
    pub request: CommissionReviewInput,
    pub runtime_base_branch: Option<String>,
}

/// User decision for a pending `commission_review` request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommissionReviewApprovalOutcome {
    Approved,
    Rejected,
}

/// Input for the `propose_task` tool (task approval workflow).
///
/// The agent passes a path (relative to the repo root) to an existing task
/// file under the project's tasks directory (typically `tasks/`, discovered
/// per project via `_TEMPLATE.md` — see
/// `taskmd_core::discover::discover_or_default`). Title, priority, status,
/// and plan body are all read from disk by the state-machine interception
/// layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposeTaskInput {
    pub task_file: String,
}

/// A single question presented to the user (REQ-AUQ-001)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserQuestion {
    pub question: String,
    pub header: String,
    pub options: Vec<QuestionOption>,
    #[serde(default, rename = "multiSelect")]
    pub multi_select: bool,
}

/// An option within a user question
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionOption {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
}

/// Annotations the user can attach to an answer
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionAnnotation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

/// Input for the `ask_user_question` tool (REQ-AUQ-001)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskUserQuestionInput {
    pub questions: Vec<UserQuestion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<QuestionMetadata>,
}

/// Optional metadata for an `ask_user_question` invocation
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// Strongly typed tool input enum
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "_tool", rename_all = "snake_case")]
pub enum ToolInput {
    Bash(BashToolInput),
    Think(ThinkInput),
    Patch(PatchInput),
    KeywordSearch(KeywordSearchInput),
    ReadImage(ReadImageInput),
    SpawnAgents(SpawnAgentsInput),
    SubmitResult(SubmitResultInput),
    SubmitError(SubmitErrorInput),
    CommissionReview(CommissionReviewInput),
    ApprovedCommissionReview(ApprovedCommissionReviewInput),
    ProposeTask(ProposeTaskInput),
    AskUserQuestion(AskUserQuestionInput),
    /// The tool name did not match any registered tool. Carries the original
    /// name and payload so the executor can still dispatch by name (e.g. MCP
    /// tools registered at runtime that the state machine does not know about).
    Unknown {
        name: String,
        input: Value,
    },
    /// The tool name matched a registered tool, but the payload failed to
    /// deserialize into the typed input. Structurally distinct from `Unknown`
    /// so callers can tell "the LLM called a tool we don't have" from "the
    /// LLM called a tool we do have but with a bad payload." Captures the
    /// serde error so it can be surfaced to the LLM or logged.
    Malformed {
        name: String,
        input: Value,
        error: String,
    },
}

fn malformed_known_input(name: &str, payload: Value, error: String) -> ToolInput {
    tracing::warn!(
        tool = name,
        error = %error,
        "known tool input failed to deserialize; emitting Malformed"
    );
    ToolInput::Malformed {
        name: name.to_string(),
        input: payload,
        error,
    }
}

fn parse_tool_input_or_malformed<T>(name: &str, payload: Value) -> ToolInput
where
    T: serde::de::DeserializeOwned,
    ToolInput: From<T>,
{
    match serde_json::from_value::<T>(payload.clone()) {
        Ok(value) => ToolInput::from(value),
        Err(err) => malformed_known_input(name, payload, err.to_string()),
    }
}

impl<'de> Deserialize<'de> for ToolInput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let mut value = Value::deserialize(deserializer)?;
        let Some(obj) = value.as_object_mut() else {
            return Err(serde::de::Error::custom("tool input must be an object"));
        };
        let tool_name = obj
            .remove("_tool")
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_else(|| "unknown".to_string());
        let payload = Value::Object(obj.clone());

        if tool_name == "unknown" {
            if let (Some(name), Some(input)) = (
                payload.get("name").and_then(Value::as_str),
                payload.get("input").cloned(),
            ) {
                return Ok(ToolInput::Unknown {
                    name: name.to_string(),
                    input,
                });
            }
        }

        if tool_name == "malformed" {
            if let (Some(name), Some(input), Some(error)) = (
                payload.get("name").and_then(Value::as_str),
                payload.get("input").cloned(),
                payload.get("error").and_then(Value::as_str),
            ) {
                return Ok(ToolInput::Malformed {
                    name: name.to_string(),
                    input,
                    error: error.to_string(),
                });
            }
            // A `_tool: malformed` tag with a missing field is persisted-state
            // corruption — fail loudly rather than masking it as a tool
            // literally named "malformed" (which would reach executor dispatch).
            return Err(serde::de::Error::custom(
                "tool input tagged `_tool: malformed` is missing a required \
                 name/input/error field",
            ));
        }

        Ok(match tool_name.as_str() {
            "bash" => match serde_json::from_value::<BashToolInput>(payload.clone()) {
                Ok(input) => ToolInput::Bash(input),
                Err(err) => payload
                    .get("command")
                    .and_then(Value::as_str)
                    .map(BashToolInput::run)
                    .map_or_else(
                        || malformed_known_input("bash", payload, err.to_string()),
                        ToolInput::Bash,
                    ),
            },
            "think" => parse_tool_input_or_malformed::<ThinkInput>("think", payload),
            "patch" => parse_tool_input_or_malformed::<PatchInput>("patch", payload),
            "keyword_search" => {
                parse_tool_input_or_malformed::<KeywordSearchInput>("keyword_search", payload)
            }
            "read_image" => parse_tool_input_or_malformed::<ReadImageInput>("read_image", payload),
            "spawn_agents" => {
                parse_tool_input_or_malformed::<SpawnAgentsInput>("spawn_agents", payload)
            }
            "submit_result" => {
                parse_tool_input_or_malformed::<SubmitResultInput>("submit_result", payload)
            }
            "submit_error" => {
                parse_tool_input_or_malformed::<SubmitErrorInput>("submit_error", payload)
            }
            "commission_review" => {
                parse_tool_input_or_malformed::<CommissionReviewInput>("commission_review", payload)
            }
            "propose_task" => {
                parse_tool_input_or_malformed::<ProposeTaskInput>("propose_task", payload)
            }
            "ask_user_question" => {
                parse_tool_input_or_malformed::<AskUserQuestionInput>("ask_user_question", payload)
            }
            other => ToolInput::Unknown {
                name: other.to_string(),
                input: payload,
            },
        })
    }
}

impl From<BashToolInput> for ToolInput {
    fn from(input: BashToolInput) -> Self {
        ToolInput::Bash(input)
    }
}
impl From<ThinkInput> for ToolInput {
    fn from(input: ThinkInput) -> Self {
        ToolInput::Think(input)
    }
}
impl From<PatchInput> for ToolInput {
    fn from(input: PatchInput) -> Self {
        ToolInput::Patch(input)
    }
}
impl From<KeywordSearchInput> for ToolInput {
    fn from(input: KeywordSearchInput) -> Self {
        ToolInput::KeywordSearch(input)
    }
}
impl From<ReadImageInput> for ToolInput {
    fn from(input: ReadImageInput) -> Self {
        ToolInput::ReadImage(input)
    }
}
impl From<SpawnAgentsInput> for ToolInput {
    fn from(input: SpawnAgentsInput) -> Self {
        ToolInput::SpawnAgents(input)
    }
}
impl From<SubmitResultInput> for ToolInput {
    fn from(input: SubmitResultInput) -> Self {
        ToolInput::SubmitResult(input)
    }
}
impl From<SubmitErrorInput> for ToolInput {
    fn from(input: SubmitErrorInput) -> Self {
        ToolInput::SubmitError(input)
    }
}
impl From<CommissionReviewInput> for ToolInput {
    fn from(input: CommissionReviewInput) -> Self {
        ToolInput::CommissionReview(input)
    }
}
impl From<ApprovedCommissionReviewInput> for ToolInput {
    fn from(input: ApprovedCommissionReviewInput) -> Self {
        ToolInput::ApprovedCommissionReview(input)
    }
}
impl From<ProposeTaskInput> for ToolInput {
    fn from(input: ProposeTaskInput) -> Self {
        ToolInput::ProposeTask(input)
    }
}
impl From<AskUserQuestionInput> for ToolInput {
    fn from(input: AskUserQuestionInput) -> Self {
        ToolInput::AskUserQuestion(input)
    }
}

impl ToolInput {
    /// Get the tool name
    #[must_use]
    pub fn tool_name(&self) -> &str {
        match self {
            ToolInput::Bash(_) => "bash",
            ToolInput::Think(_) => "think",
            ToolInput::Patch(_) => "patch",
            ToolInput::KeywordSearch(_) => "keyword_search",
            ToolInput::ReadImage(_) => "read_image",
            ToolInput::SpawnAgents(_) => "spawn_agents",
            ToolInput::SubmitResult(_) => "submit_result",
            ToolInput::SubmitError(_) => "submit_error",
            ToolInput::CommissionReview(_) | ToolInput::ApprovedCommissionReview(_) => {
                "commission_review"
            }
            ToolInput::ProposeTask(_) => "propose_task",
            ToolInput::AskUserQuestion(_) => "ask_user_question",
            ToolInput::Unknown { name, .. } | ToolInput::Malformed { name, .. } => name,
        }
    }

    /// Check if this is a sub-agent terminal tool
    #[must_use]
    pub fn is_terminal_tool(&self) -> bool {
        matches!(self, ToolInput::SubmitResult(_) | ToolInput::SubmitError(_))
    }

    /// Convert to JSON Value for tool execution
    #[must_use]
    pub fn to_value(&self) -> Value {
        match self {
            ToolInput::Bash(input) => serde_json::to_value(input).unwrap_or(Value::Null),
            ToolInput::Think(input) => serde_json::to_value(input).unwrap_or(Value::Null),
            ToolInput::Patch(input) => serde_json::to_value(input).unwrap_or(Value::Null),
            ToolInput::KeywordSearch(input) => serde_json::to_value(input).unwrap_or(Value::Null),
            ToolInput::ReadImage(input) => serde_json::to_value(input).unwrap_or(Value::Null),
            ToolInput::SpawnAgents(input) => serde_json::to_value(input).unwrap_or(Value::Null),
            ToolInput::SubmitResult(input) => serde_json::to_value(input).unwrap_or(Value::Null),
            ToolInput::SubmitError(input) => serde_json::to_value(input).unwrap_or(Value::Null),
            ToolInput::CommissionReview(input) => {
                serde_json::to_value(input).unwrap_or(Value::Null)
            }
            ToolInput::ApprovedCommissionReview(input) => {
                serde_json::to_value(input).unwrap_or(Value::Null)
            }
            ToolInput::ProposeTask(input) => serde_json::to_value(input).unwrap_or(Value::Null),
            ToolInput::AskUserQuestion(input) => serde_json::to_value(input).unwrap_or(Value::Null),
            ToolInput::Unknown { input, .. } | ToolInput::Malformed { input, .. } => input.clone(),
        }
    }

    /// Parse from tool name and JSON value.
    ///
    /// Returns `ToolInput::Malformed` (carrying the serde error) when the
    /// tool name matches a registered tool but the payload fails to parse,
    /// and `ToolInput::Unknown` only when the tool name itself is not
    /// registered. The two cases are structurally distinct so callers can
    /// surface a malformed-known input separately from an unsupported tool.
    #[must_use]
    pub fn from_name_and_value(name: &str, value: Value) -> Self {
        fn parse<T>(name: &str, value: Value) -> ToolInput
        where
            T: serde::de::DeserializeOwned,
            ToolInput: From<T>,
        {
            match serde_json::from_value::<T>(value.clone()) {
                Ok(parsed) => ToolInput::from(parsed),
                Err(err) => malformed_known_input(name, value, err.to_string()),
            }
        }
        match name {
            "bash" => parse::<BashToolInput>(name, value),
            "think" => parse::<ThinkInput>(name, value),
            "patch" => parse::<PatchInput>(name, value),
            "keyword_search" => parse::<KeywordSearchInput>(name, value),
            "read_image" => parse::<ReadImageInput>(name, value),
            "spawn_agents" => parse::<SpawnAgentsInput>(name, value),
            "submit_result" => parse::<SubmitResultInput>(name, value),
            "submit_error" => parse::<SubmitErrorInput>(name, value),
            "commission_review" => parse::<CommissionReviewInput>(name, value),
            "propose_task" => parse::<ProposeTaskInput>(name, value),
            "ask_user_question" => parse::<AskUserQuestionInput>(name, value),
            _ => ToolInput::Unknown {
                name: name.to_string(),
                input: value,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_bash_tool_input_deserializes_as_modern_run() {
        let input: ToolInput = serde_json::from_value(serde_json::json!({
            "_tool": "bash",
            "command": "echo legacy",
            "mode": "default"
        }))
        .expect("legacy bash state should deserialize");

        let ToolInput::Bash(input) = input else {
            panic!("expected bash input");
        };
        assert_eq!(input.op, crate::domain::bash_types::BashOp::Run);
        assert_eq!(input.cmd.as_deref(), Some("echo legacy"));
    }

    #[test]
    fn unknown_tool_input_preserves_original_name_and_payload() {
        let input: ToolInput = serde_json::from_value(serde_json::json!({
            "_tool": "unknown",
            "name": "bash",
            "input": { "op": "run", "command": "bad" }
        }))
        .expect("unknown tool state should deserialize");

        let ToolInput::Unknown { name, input } = input else {
            panic!("expected unknown input");
        };
        assert_eq!(name, "bash");
        assert_eq!(input["op"], "run");
        assert_eq!(input["command"], "bad");
    }

    /// A known tool name with a payload that fails to deserialize must produce
    /// `ToolInput::Malformed` — structurally distinct from `Unknown` (which is
    /// reserved for unsupported tool names). This is the regression guard for
    /// task 13018: previously both cases collapsed into the same `Unknown`
    /// variant, hiding "schema drift / bad LLM output" inside "tool we don't
    /// have."
    #[test]
    fn malformed_known_tool_is_distinct_from_unknown() {
        // `think` is a registered tool; `thoughts` is required as a string, but
        // here it's an integer — deserialisation fails.
        let bad_payload = serde_json::json!({ "thoughts": 42 });
        let parsed = ToolInput::from_name_and_value("think", bad_payload.clone());
        // reason: this assertion only cares about Malformed vs. everything else;
        // enumerating all 10+ typed-tool variants would obscure intent.
        #[allow(clippy::wildcard_enum_match_arm)]
        match parsed {
            ToolInput::Malformed { name, input, error } => {
                assert_eq!(name, "think");
                assert_eq!(input, bad_payload);
                assert!(
                    !error.is_empty(),
                    "Malformed must carry the serde error string"
                );
            }
            other => panic!("expected Malformed for malformed known tool, got {other:?}"),
        }

        // A genuinely-unknown tool name still produces `Unknown` — the two
        // cases must not collapse in either direction.
        let parsed =
            ToolInput::from_name_and_value("tool_we_do_not_have", serde_json::json!({"x": 1}));
        assert!(
            matches!(parsed, ToolInput::Unknown { .. }),
            "expected Unknown for unregistered name, got {parsed:?}"
        );
    }

    /// `ToolInput::Malformed` must round-trip through the persisted
    /// `_tool`-tagged JSON used for `ConvState` serialisation, so a tool call
    /// captured mid-execution can survive a server restart with its serde
    /// error attached for surfacing on resume.
    #[test]
    fn malformed_tool_input_round_trips() {
        let original = ToolInput::Malformed {
            name: "propose_task".to_string(),
            input: serde_json::json!({"unexpected": "shape"}),
            error: "missing field `task_file`".to_string(),
        };
        let encoded = serde_json::to_value(&original).unwrap();
        assert_eq!(encoded["_tool"], "malformed");
        let decoded: ToolInput = serde_json::from_value(encoded).unwrap();
        assert_eq!(decoded, original);
    }

    /// A `_tool: malformed` payload missing a required field is persisted-state
    /// corruption — deserialization must fail loudly rather than silently
    /// degrade to `Unknown { name: "malformed" }` (which would reach executor
    /// dispatch as a tool literally named "malformed").
    #[test]
    fn malformed_tool_input_with_missing_field_is_rejected() {
        for partial in [
            serde_json::json!({"_tool": "malformed", "input": {}, "error": "e"}),
            serde_json::json!({"_tool": "malformed", "name": "bash", "error": "e"}),
            serde_json::json!({"_tool": "malformed", "name": "bash", "input": {}}),
        ] {
            assert!(
                serde_json::from_value::<ToolInput>(partial.clone()).is_err(),
                "corrupt malformed payload must fail to deserialize: {partial}"
            );
        }
    }

    /// Task 13014: `ConvState::ToolExecuting` is strict-deserialized — a
    /// persisted row missing a field is corruption, not a defaultable absence
    /// (`reset_all_to_idle` wipes this transient state on startup, so a
    /// cross-version row can never legitimately reach this code path). This
    /// test locks the strictness: re-adding `#[serde(default)]` would silently
    /// regress it.
    #[test]
    fn tool_executing_rejects_rows_missing_fields() {
        // A complete row deserializes fine.
        let complete = serde_json::json!({
            "type": "tool_executing",
            "current_tool": { "id": "t1", "name": "think",
                "input": { "_tool": "think", "thoughts": "" } },
            "remaining_tools": [],
            "completed_results": [],
            "pending_sub_agents": [],
            "assistant_message": AssistantMessage::default(),
        });
        assert!(
            serde_json::from_value::<ConvState>(complete.clone()).is_ok(),
            "a complete tool_executing row must deserialize"
        );

        // Dropping any field is now a loud error, not a silent default.
        for field in [
            "completed_results",
            "pending_sub_agents",
            "assistant_message",
        ] {
            let mut partial = complete.clone();
            partial.as_object_mut().unwrap().remove(field);
            assert!(
                serde_json::from_value::<ConvState>(partial).is_err(),
                "tool_executing missing `{field}` must fail to deserialize"
            );
        }
    }

    /// Task 02713: model change is allowed from `Idle` and `Error` only.
    ///
    /// The `match` below is intentionally wildcard-free: adding a new
    /// `ConvState` variant breaks this test's compilation, forcing an
    /// explicit decision about whether a mid-state model swap is safe
    /// for that variant (correct-by-construction guard).
    #[test]
    // reason: exhaustive per-variant ConvState construction is inherently long;
    // splitting it would scatter the correct-by-construction guard described above.
    #[allow(clippy::too_many_lines)]
    fn allows_model_change_only_from_idle_and_error() {
        fn err() -> ConvState {
            ConvState::Error {
                message: "overloaded".into(),
                error_kind: ErrorKind::ServerOverloaded,
                resets_at: None,
            }
        }
        fn tool_call() -> ToolCall {
            ToolCall::new(
                "t1",
                ToolInput::Think(ThinkInput {
                    thoughts: String::new(),
                }),
            )
        }

        let all = [
            ConvState::Idle,
            ConvState::LlmRequesting { attempt: 1 },
            ConvState::ToolExecuting {
                current_tool: tool_call(),
                remaining_tools: vec![],
                completed_results: vec![],
                pending_sub_agents: vec![],
                assistant_message: AssistantMessage::default(),
            },
            ConvState::CancellingTool {
                tool_use_id: "t1".into(),
                skipped_tools: vec![],
                completed_results: vec![],
                assistant_message: AssistantMessage::default(),
                pending_sub_agents: vec![],
            },
            ConvState::AwaitingSubAgents {
                pending: vec![],
                completed_results: vec![],
                spawn_tool_id: None,
            },
            ConvState::CancellingSubAgents {
                pending: vec![],
                completed_results: vec![],
                cause: crate::domain::sm_event::CancelCause::UserRequested,
                spawn_tool_id: None,
            },
            ConvState::Completed {
                result: "ok".into(),
            },
            ConvState::Failed {
                error: "boom".into(),
                error_kind: ErrorKind::SubAgentError,
            },
            err(),
            ConvState::AwaitingRecovery {
                message: "auth".into(),
                error_kind: ErrorKind::Auth,
                recovery_kind: RecoveryKind::Credential,
                resume: RecoveryResumeTarget::ConversationTurn,
            },
            ConvState::AwaitingContinuation {
                rejected_tool_calls: vec![],
                attempt: 1,
            },
            ConvState::AwaitingTaskApproval {
                task_file: "tasks/x.md".into(),
                title: "t".into(),
                priority: crate::task_source::Priority::P1,
                plan: "plan".into(),
            },
            ConvState::AwaitingUserResponse {
                questions: vec![],
                tool_use_id: "t1".into(),
            },
            ConvState::ContextExhausted {
                summary: "s".into(),
            },
            ConvState::HandedOff {
                successor_conv_id: "next".into(),
            },
            ConvState::SeededLlmRequesting {
                seed_message_id: "seed".into(),
                attempt: 1,
            },
            ConvState::Terminal,
        ];

        for state in &all {
            // Independent restatement of the predicate. Exhaustive (no
            // `_` arm) so a new variant fails to compile here.
            let expected = match state {
                ConvState::Idle | ConvState::Error { .. } => true,
                ConvState::LlmRequesting { .. }
                | ConvState::SeededLlmRequesting { .. }
                | ConvState::ToolExecuting { .. }
                | ConvState::CancellingTool { .. }
                | ConvState::AwaitingSubAgents { .. }
                | ConvState::CancellingSubAgents { .. }
                | ConvState::Completed { .. }
                | ConvState::Failed { .. }
                | ConvState::AwaitingRecovery { .. }
                | ConvState::AwaitingContinuation { .. }
                | ConvState::AwaitingTaskApproval { .. }
                | ConvState::AwaitingUserResponse { .. }
                | ConvState::AwaitingCommissionReviewApproval { .. }
                | ConvState::ContextExhausted { .. }
                | ConvState::HandedOff { .. }
                | ConvState::Terminal => false,
            };
            assert_eq!(
                state.allows_model_change(),
                expected,
                "allows_model_change mismatch for {}",
                state.variant_name()
            );
        }
    }
}

/// A tool call from the LLM with typed input.
///
/// On the wire we expose `name` at the top level (not buried inside the
/// `_tool` serde-tag of `ToolInput`). The `_tool` tag is an internal
/// encoding for the typed enum — for `ToolInput::Unknown` it serializes
/// as the literal string `"unknown"`, which is useless to consumers. The
/// authoritative tool name lives at `ToolCall.name` and is emitted by the
/// custom `Serialize` impl below. Derived `Deserialize` ignores the extra
/// `name` field on read-back (we reconstruct it from the inner variant via
/// `input.tool_name()` whenever it is needed).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub input: ToolInput,
}

impl Serialize for ToolCall {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(3))?;
        map.serialize_entry("id", &self.id)?;
        map.serialize_entry("name", self.input.tool_name())?;
        map.serialize_entry("input", &self.input)?;
        map.end()
    }
}

impl ToolCall {
    pub fn new(id: impl Into<String>, input: ToolInput) -> Self {
        Self {
            id: id.into(),
            input,
        }
    }

    /// Get the tool name
    #[must_use]
    pub fn name(&self) -> &str {
        self.input.tool_name()
    }
}

// ============================================================================
// Assistant Message — bundled representation for atomic persistence
// ============================================================================

/// An LLM assistant message held in state until persistence.
/// Bundles content, display metadata, usage stats, and message ID so they
/// cannot be partially threaded or forgotten.
///
/// `created_at` is captured once at construction and threaded through BOTH
/// the eager SSE broadcast (`Effect::BroadcastAssistantMessage`) and the
/// eventual DB persist at `persist_checkpoint`. Keeping them in lockstep
/// prevents a user-visible timestamp jump: without this, the eager copy
/// carries one `Utc::now()` and the persisted DB row carries a later one,
/// so reconnecting clients would see the message timestamp shift when init
/// merges the DB row in. `#[serde(default = "chrono::Utc::now")]` lets old
/// `ConvState` JSON rows without the field deserialise cleanly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssistantMessage {
    pub message_id: String,
    pub content: Vec<ContentBlock>,
    pub usage: Option<UsageData>,
    pub display_data: Option<Value>,
    #[serde(default = "chrono::Utc::now")]
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl Default for AssistantMessage {
    fn default() -> Self {
        Self {
            message_id: String::new(),
            content: Vec::new(),
            usage: None,
            display_data: None,
            created_at: chrono::Utc::now(),
        }
    }
}

impl AssistantMessage {
    /// Construct with an explicit `message_id`. Production code threads the
    /// LLM dispatch's `request_id` through so the streaming `Token` events
    /// (which already carry it) share identity with this finalized message —
    /// the UI keys its in-flight streaming render unit by `request_id`, the
    /// eventual `agent_turn` render unit by `message_id`, and the two match
    /// exactly because they're the same string. Tests can pass a fresh
    /// `Uuid::new_v4().to_string()` if they don't care about the wire trace.
    #[must_use]
    pub fn new(
        message_id: String,
        content: Vec<ContentBlock>,
        usage: Option<UsageData>,
        display_data: Option<Value>,
    ) -> Self {
        Self {
            message_id,
            content,
            usage,
            display_data,
            created_at: chrono::Utc::now(),
        }
    }

    /// Returns references to the `ToolUse` blocks in content.
    /// Used by `CheckpointData::tool_round()` to enforce the matching-count invariant.
    #[must_use]
    pub fn tool_uses(&self) -> Vec<&ContentBlock> {
        self.content
            .iter()
            .filter(|b| matches!(b, ContentBlock::ToolUse { .. }))
            .collect()
    }
}

// ============================================================================
// Conversation State
// ============================================================================

/// Active recovery mechanism in flight (REQ-BED-030).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryKind {
    /// Credential helper subprocess is running (OIDC flow in progress).
    Credential,
}

/// Operation-specific inputs for a continuation summary request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContinuationSummaryRequest {
    pub rejected_tool_calls: Vec<ToolCall>,
}

/// LLM operation suspended while an external recovery mechanism runs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RecoveryResumeTarget {
    ConversationTurn,
    ContinuationSummary { request: ContinuationSummaryRequest },
}
/// Conversation state
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
#[derive(Default)]
pub enum ConvState {
    /// Ready for user input, no pending operations
    #[default]
    Idle,

    /// LLM request in flight, with retry tracking
    LlmRequesting { attempt: u32 },

    /// Fresh handoff successor whose approved-plan seed is already durable.
    /// Runtime resume treats this as `LlmRequesting` so the first request can
    /// be replayed after a crash without reinserting or reconstructing the seed.
    SeededLlmRequesting {
        seed_message_id: String,
        attempt: u32,
    },

    /// Executing tools serially.
    /// The assistant message is held here (NOT yet persisted) — persistence is atomic
    /// at the end of the tool round via `CheckpointData::ToolRound` (REQ-BED-007).
    ///
    /// Fields are strict-deserialized (no `serde(default)`): `reset_all_to_idle`
    /// resets this transient state to `Idle` on startup before any row is read,
    /// so a cross-version row missing a field is never observed by a recovered
    /// conversation — a missing field instead signals corruption and must fail
    /// loudly rather than default silently (task 13014).
    ToolExecuting {
        /// The current tool being executed
        current_tool: ToolCall,
        /// Remaining tools to execute after current completes
        remaining_tools: Vec<ToolCall>,
        /// Completed tool results — single source of truth (FM-4 Prevention).
        /// No parallel `persisted_tool_ids` tracking set.
        completed_results: Vec<ToolResult>,
        /// Sub-agents spawned during this tool execution phase
        pending_sub_agents: Vec<PendingSubAgent>,
        /// Assistant message held until all tools complete (not yet persisted)
        assistant_message: AssistantMessage,
    },

    /// User requested cancellation of tool execution, waiting for abort confirmation.
    /// Carries the assistant message and completed results so the checkpoint can
    /// be persisted atomically on abort.
    CancellingTool {
        /// The tool being aborted
        tool_use_id: String,
        /// Tools that were skipped
        skipped_tools: Vec<ToolCall>,
        /// Tool results completed before cancellation
        completed_results: Vec<ToolResult>,
        /// Assistant message held for atomic persistence
        assistant_message: AssistantMessage,
        /// Sub-agents spawned earlier in this tool round, awaiting cancellation.
        /// Empty when no `spawn_agents` ran before the cancel.
        pending_sub_agents: Vec<PendingSubAgent>,
    },

    /// Waiting for sub-agents to complete.
    ///
    /// Strict-deserialized — see `ToolExecuting` (task 13014).
    AwaitingSubAgents {
        /// Sub-agents still running (id + task co-located)
        pending: Vec<PendingSubAgent>,
        completed_results: Vec<SubAgentResult>,
        /// `tool_use_id` of the `spawn_agents` call (to update `display_data` when done)
        spawn_tool_id: Option<String>,
    },

    /// User requested cancellation while waiting for sub-agents.
    ///
    /// Strict-deserialized — see `ToolExecuting` (task 13014).
    CancellingSubAgents {
        /// Sub-agents still running (id + task co-located)
        pending: Vec<PendingSubAgent>,
        completed_results: Vec<SubAgentResult>,
        /// Why teardown was initiated — maps the recorded outcome of draining
        /// sub-agents (task 61004). Transient/owned: this state is reset to Idle
        /// on startup, so it is strict-deserialized like its siblings (no
        /// `serde(default)`), and no migration is owed.
        cause: crate::domain::sm_event::CancelCause,
        /// `tool_use_id` of the originating `spawn_agents` call, when teardown
        /// began from `AwaitingSubAgents`. `None` for the `CancellingTool`-origin
        /// path (no spawn id exists), which suppresses result persistence to
        /// avoid an orphaned `tool_result`. Transient/owned like `cause`.
        spawn_tool_id: Option<String>,
    },

    /// Sub-agent completed successfully (terminal state, sub-agent only)
    Completed { result: String },

    /// Sub-agent failed (terminal state, sub-agent only)
    Failed {
        error: String,
        error_kind: ErrorKind,
    },

    /// Error occurred - UI displays this state directly
    Error {
        message: String,
        error_kind: ErrorKind,
        /// Upstream quota-window reset time, when known. Populated only for an
        /// `error_kind == UsageLimitReached` whose 429 carried a `resets_at`;
        /// the auto-clear sweep returns the conversation to Idle once this
        /// instant has passed. `None` for every other error.
        // owned: pre-feature error rows had no reset time; None is correct,
        // no migration owed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resets_at: Option<chrono::DateTime<chrono::Utc>>,
    },

    /// Recovery mechanism active — waiting for external resolution (REQ-BED-030).
    /// Distinct from `Error`: something is in flight to fix the problem.
    AwaitingRecovery {
        message: String,
        error_kind: ErrorKind,
        recovery_kind: RecoveryKind,
        resume: RecoveryResumeTarget,
    },

    /// Awaiting continuation summary from LLM (tool-less request in flight)
    AwaitingContinuation {
        /// Tool calls that were requested but not executed
        rejected_tool_calls: Vec<ToolCall>,
        /// Retry attempt for the continuation request
        attempt: u32,
    },

    /// Awaiting user approval of a proposed task plan (REQ-BED-028).
    ///
    /// `task_file` is the canonical source — the path (relative to the
    /// conversation cwd) to a file under `tasks/`. The other fields are a
    /// snapshot derived from that file at the moment `propose_task` was
    /// called and exist for UI display only; the executor re-reads
    /// `task_file` on approval.
    ///
    /// `serde(default)` on `task_file` is a rollout shim for pre-1.0
    /// conversations persisted before this field existed. Such rows
    /// deserialise with an empty `task_file`, which the executor surfaces
    /// as a clear "reject and re-propose" error rather than silently
    /// resetting the conversation to `Idle`.
    AwaitingTaskApproval {
        #[serde(default)]
        task_file: String,
        title: String,
        priority: crate::task_source::Priority,
        plan: String,
    },

    /// Awaiting user answers to clarifying questions (REQ-AUQ-001).
    /// `ask_user_question` must be the sole tool in a response, so there are
    /// no remaining tools or persisted tool IDs to carry.
    AwaitingUserResponse {
        questions: Vec<UserQuestion>,
        tool_use_id: String,
    },

    /// Awaiting human approval before spending review tokens for `commission_review`.
    AwaitingCommissionReviewApproval {
        tool_call: ToolCall,
        brief: String,
        focus: Option<String>,
        allow_dirty_working_tree: bool,
        assistant_message: AssistantMessage,
    },

    /// Context window exhausted - conversation is read-only
    ContextExhausted {
        /// The continuation summary
        summary: String,
    },

    /// This conversation handed live work to a successor conversation.
    HandedOff { successor_conv_id: String },

    /// Task lifecycle completed or abandoned — conversation is permanently read-only.
    /// Rejects all events. Preserved on server restart (not reset to Idle).
    Terminal,
}

// ============================================================================
// Split State Types — CoreState, ParentState, SubAgentState
//
// CoreState holds behavior shared between parent and sub-agent conversations.
// ParentState wraps CoreState and adds parent-only variants.
// SubAgentState wraps CoreState and adds sub-agent-only variants.
//
// ConvState remains as the DB serialization format. From/TryFrom conversions
// bridge the split types to/from the flat ConvState.
// ============================================================================

/// Shared state variants common to both parent and sub-agent conversations.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(tag = "type", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)] // Wrapping in Box would add indirection for marginal gain
pub enum CoreState {
    #[default]
    Idle,
    LlmRequesting {
        attempt: u32,
    },
    ToolExecuting {
        current_tool: ToolCall,
        remaining_tools: Vec<ToolCall>,
        completed_results: Vec<ToolResult>,
        pending_sub_agents: Vec<PendingSubAgent>,
        assistant_message: AssistantMessage,
    },
    CancellingTool {
        tool_use_id: String,
        skipped_tools: Vec<ToolCall>,
        completed_results: Vec<ToolResult>,
        assistant_message: AssistantMessage,
        pending_sub_agents: Vec<PendingSubAgent>,
    },
    AwaitingSubAgents {
        pending: Vec<PendingSubAgent>,
        completed_results: Vec<SubAgentResult>,
        spawn_tool_id: Option<String>,
    },
    CancellingSubAgents {
        pending: Vec<PendingSubAgent>,
        completed_results: Vec<SubAgentResult>,
        /// Why teardown was initiated — see `ConvState::CancellingSubAgents`.
        cause: crate::domain::sm_event::CancelCause,
        /// See `ConvState::CancellingSubAgents::spawn_tool_id`.
        spawn_tool_id: Option<String>,
    },
    Error {
        message: String,
        error_kind: ErrorKind,
        /// See `ConvState::Error::resets_at`. Threaded through the
        /// Core↔Conv mappings so the persisted state carries the
        /// usage-limit reset time the auto-clear sweep reads.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resets_at: Option<chrono::DateTime<chrono::Utc>>,
    },
    AwaitingContinuation {
        rejected_tool_calls: Vec<ToolCall>,
        attempt: u32,
    },
}

/// Parent conversation state. Wraps `CoreState` for shared behavior and adds
/// parent-only variants that are structurally excluded from sub-agent transitions.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)] // Core variant is large but dominant path
pub enum ParentState {
    Core(CoreState),
    AwaitingRecovery {
        message: String,
        error_kind: ErrorKind,
        recovery_kind: RecoveryKind,
        resume: RecoveryResumeTarget,
    },
    AwaitingTaskApproval {
        task_file: String,
        title: String,
        priority: crate::task_source::Priority,
        plan: String,
    },
    AwaitingUserResponse {
        questions: Vec<UserQuestion>,
        tool_use_id: String,
    },
    AwaitingCommissionReviewApproval {
        tool_call: ToolCall,
        brief: String,
        focus: Option<String>,
        allow_dirty_working_tree: bool,
        assistant_message: AssistantMessage,
    },
    ContextExhausted {
        summary: String,
    },
    HandedOff {
        successor_conv_id: String,
    },
    Terminal,
}

/// Sub-agent conversation state. Wraps `CoreState` for shared behavior and adds
/// sub-agent-only terminal variants that are structurally excluded from parent transitions.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)] // Core variant is large but dominant path
pub enum SubAgentState {
    Core(CoreState),
    Completed {
        result: String,
    },
    Failed {
        error: String,
        error_kind: ErrorKind,
    },
}

// ============================================================================
// From/TryFrom: ConvState <-> ParentState / SubAgentState
// ============================================================================

impl From<ParentState> for ConvState {
    fn from(ps: ParentState) -> Self {
        match ps {
            ParentState::Core(core) => core.into(),
            ParentState::AwaitingRecovery {
                message,
                error_kind,
                recovery_kind,
                resume,
            } => ConvState::AwaitingRecovery {
                message,
                error_kind,
                recovery_kind,
                resume,
            },
            ParentState::AwaitingTaskApproval {
                task_file,
                title,
                priority,
                plan,
            } => ConvState::AwaitingTaskApproval {
                task_file,
                title,
                priority,
                plan,
            },
            ParentState::AwaitingUserResponse {
                questions,
                tool_use_id,
            } => ConvState::AwaitingUserResponse {
                questions,
                tool_use_id,
            },
            ParentState::AwaitingCommissionReviewApproval {
                tool_call,
                brief,
                focus,
                allow_dirty_working_tree,
                assistant_message,
            } => ConvState::AwaitingCommissionReviewApproval {
                tool_call,
                brief,
                focus,
                allow_dirty_working_tree,
                assistant_message,
            },
            ParentState::ContextExhausted { summary } => ConvState::ContextExhausted { summary },
            ParentState::HandedOff { successor_conv_id } => {
                ConvState::HandedOff { successor_conv_id }
            }
            ParentState::Terminal => ConvState::Terminal,
        }
    }
}

impl From<SubAgentState> for ConvState {
    fn from(ss: SubAgentState) -> Self {
        match ss {
            SubAgentState::Core(core) => core.into(),
            SubAgentState::Completed { result } => ConvState::Completed { result },
            SubAgentState::Failed { error, error_kind } => ConvState::Failed { error, error_kind },
        }
    }
}

impl From<CoreState> for ConvState {
    fn from(cs: CoreState) -> Self {
        match cs {
            CoreState::Idle => ConvState::Idle,
            CoreState::LlmRequesting { attempt } => ConvState::LlmRequesting { attempt },
            CoreState::ToolExecuting {
                current_tool,
                remaining_tools,
                completed_results,
                pending_sub_agents,
                assistant_message,
            } => ConvState::ToolExecuting {
                current_tool,
                remaining_tools,
                completed_results,
                pending_sub_agents,
                assistant_message,
            },
            CoreState::CancellingTool {
                tool_use_id,
                skipped_tools,
                completed_results,
                assistant_message,
                pending_sub_agents,
            } => ConvState::CancellingTool {
                tool_use_id,
                skipped_tools,
                completed_results,
                assistant_message,
                pending_sub_agents,
            },
            CoreState::AwaitingSubAgents {
                pending,
                completed_results,
                spawn_tool_id,
            } => ConvState::AwaitingSubAgents {
                pending,
                completed_results,
                spawn_tool_id,
            },
            CoreState::CancellingSubAgents {
                pending,
                completed_results,
                cause,
                spawn_tool_id,
            } => ConvState::CancellingSubAgents {
                pending,
                completed_results,
                cause,
                spawn_tool_id,
            },
            CoreState::Error {
                message,
                error_kind,
                resets_at,
            } => ConvState::Error {
                message,
                error_kind,
                resets_at,
            },
            CoreState::AwaitingContinuation {
                rejected_tool_calls,
                attempt,
            } => ConvState::AwaitingContinuation {
                rejected_tool_calls,
                attempt,
            },
        }
    }
}

/// Error returned when a `ConvState` cannot be converted to the requested split type.
#[derive(Debug, Clone)]
pub struct StateConversionError {
    pub from_variant: &'static str,
    pub target_type: &'static str,
}

impl std::fmt::Display for StateConversionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "cannot convert ConvState::{} to {}",
            self.from_variant, self.target_type
        )
    }
}

impl std::error::Error for StateConversionError {}

impl TryFrom<ConvState> for ParentState {
    type Error = StateConversionError;

    #[allow(clippy::too_many_lines)]
    fn try_from(cs: ConvState) -> Result<Self, Self::Error> {
        match cs {
            // Core states
            ConvState::Idle => Ok(ParentState::Core(CoreState::Idle)),
            ConvState::LlmRequesting { attempt }
            | ConvState::SeededLlmRequesting { attempt, .. } => {
                Ok(ParentState::Core(CoreState::LlmRequesting { attempt }))
            }
            ConvState::ToolExecuting {
                current_tool,
                remaining_tools,
                completed_results,
                pending_sub_agents,
                assistant_message,
            } => Ok(ParentState::Core(CoreState::ToolExecuting {
                current_tool,
                remaining_tools,
                completed_results,
                pending_sub_agents,
                assistant_message,
            })),
            ConvState::CancellingTool {
                tool_use_id,
                skipped_tools,
                completed_results,
                assistant_message,
                pending_sub_agents,
            } => Ok(ParentState::Core(CoreState::CancellingTool {
                tool_use_id,
                skipped_tools,
                completed_results,
                assistant_message,
                pending_sub_agents,
            })),
            ConvState::AwaitingSubAgents {
                pending,
                completed_results,
                spawn_tool_id,
            } => Ok(ParentState::Core(CoreState::AwaitingSubAgents {
                pending,
                completed_results,
                spawn_tool_id,
            })),
            ConvState::CancellingSubAgents {
                pending,
                completed_results,
                cause,
                spawn_tool_id,
            } => Ok(ParentState::Core(CoreState::CancellingSubAgents {
                pending,
                completed_results,
                cause,
                spawn_tool_id,
            })),
            ConvState::Error {
                message,
                error_kind,
                resets_at,
            } => Ok(ParentState::Core(CoreState::Error {
                message,
                error_kind,
                resets_at,
            })),
            ConvState::AwaitingContinuation {
                rejected_tool_calls,
                attempt,
            } => Ok(ParentState::Core(CoreState::AwaitingContinuation {
                rejected_tool_calls,
                attempt,
            })),
            // Parent-only states
            ConvState::AwaitingRecovery {
                message,
                error_kind,
                recovery_kind,
                resume,
            } => Ok(ParentState::AwaitingRecovery {
                message,
                error_kind,
                recovery_kind,
                resume,
            }),
            ConvState::AwaitingTaskApproval {
                task_file,
                title,
                priority,
                plan,
            } => Ok(ParentState::AwaitingTaskApproval {
                task_file,
                title,
                priority,
                plan,
            }),
            ConvState::AwaitingUserResponse {
                questions,
                tool_use_id,
            } => Ok(ParentState::AwaitingUserResponse {
                questions,
                tool_use_id,
            }),
            ConvState::AwaitingCommissionReviewApproval {
                tool_call,
                brief,
                focus,
                allow_dirty_working_tree,
                assistant_message,
            } => Ok(ParentState::AwaitingCommissionReviewApproval {
                tool_call,
                brief,
                focus,
                allow_dirty_working_tree,
                assistant_message,
            }),
            ConvState::ContextExhausted { summary } => {
                Ok(ParentState::ContextExhausted { summary })
            }
            ConvState::HandedOff { successor_conv_id } => {
                Ok(ParentState::HandedOff { successor_conv_id })
            }
            ConvState::Terminal => Ok(ParentState::Terminal),
            // Sub-agent-only states are invalid for parent
            ConvState::Completed { .. } | ConvState::Failed { .. } => Err(StateConversionError {
                from_variant: cs.variant_name(),
                target_type: "ParentState",
            }),
        }
    }
}

impl TryFrom<ConvState> for SubAgentState {
    type Error = StateConversionError;

    fn try_from(cs: ConvState) -> Result<Self, Self::Error> {
        match cs {
            // Core states
            ConvState::Idle => Ok(SubAgentState::Core(CoreState::Idle)),
            ConvState::LlmRequesting { attempt }
            | ConvState::SeededLlmRequesting { attempt, .. } => {
                Ok(SubAgentState::Core(CoreState::LlmRequesting { attempt }))
            }
            ConvState::ToolExecuting {
                current_tool,
                remaining_tools,
                completed_results,
                pending_sub_agents,
                assistant_message,
            } => Ok(SubAgentState::Core(CoreState::ToolExecuting {
                current_tool,
                remaining_tools,
                completed_results,
                pending_sub_agents,
                assistant_message,
            })),
            ConvState::CancellingTool {
                tool_use_id,
                skipped_tools,
                completed_results,
                assistant_message,
                pending_sub_agents,
            } => Ok(SubAgentState::Core(CoreState::CancellingTool {
                tool_use_id,
                skipped_tools,
                completed_results,
                assistant_message,
                pending_sub_agents,
            })),
            ConvState::AwaitingSubAgents {
                pending,
                completed_results,
                spawn_tool_id,
            } => Ok(SubAgentState::Core(CoreState::AwaitingSubAgents {
                pending,
                completed_results,
                spawn_tool_id,
            })),
            ConvState::CancellingSubAgents {
                pending,
                completed_results,
                cause,
                spawn_tool_id,
            } => Ok(SubAgentState::Core(CoreState::CancellingSubAgents {
                pending,
                completed_results,
                cause,
                spawn_tool_id,
            })),
            ConvState::Error {
                message,
                error_kind,
                resets_at,
            } => Ok(SubAgentState::Core(CoreState::Error {
                message,
                error_kind,
                resets_at,
            })),
            ConvState::AwaitingContinuation {
                rejected_tool_calls,
                attempt,
            } => Ok(SubAgentState::Core(CoreState::AwaitingContinuation {
                rejected_tool_calls,
                attempt,
            })),
            // Sub-agent-only states
            ConvState::Completed { result } => Ok(SubAgentState::Completed { result }),
            ConvState::Failed { error, error_kind } => {
                Ok(SubAgentState::Failed { error, error_kind })
            }
            // Parent-only states are invalid for sub-agent
            ConvState::AwaitingRecovery { .. }
            | ConvState::AwaitingTaskApproval { .. }
            | ConvState::AwaitingUserResponse { .. }
            | ConvState::AwaitingCommissionReviewApproval { .. }
            | ConvState::ContextExhausted { .. }
            | ConvState::HandedOff { .. }
            | ConvState::Terminal => Err(StateConversionError {
                from_variant: cs.variant_name(),
                target_type: "SubAgentState",
            }),
        }
    }
}

impl CoreState {
    /// Stable variant name (mirrors `ConvState::variant_name`)
    #[must_use]
    pub fn variant_name(&self) -> &'static str {
        match self {
            CoreState::Idle => "Idle",
            CoreState::LlmRequesting { .. } => "LlmRequesting",
            CoreState::ToolExecuting { .. } => "ToolExecuting",
            CoreState::CancellingTool { .. } => "CancellingTool",
            CoreState::AwaitingSubAgents { .. } => "AwaitingSubAgents",
            CoreState::CancellingSubAgents { .. } => "CancellingSubAgents",
            CoreState::Error { .. } => "Error",
            CoreState::AwaitingContinuation { .. } => "AwaitingContinuation",
        }
    }
}

impl ParentState {
    /// Stable variant name
    #[must_use]
    pub fn variant_name(&self) -> &'static str {
        match self {
            ParentState::Core(c) => c.variant_name(),
            ParentState::AwaitingRecovery { .. } => "AwaitingRecovery",
            ParentState::AwaitingTaskApproval { .. } => "AwaitingTaskApproval",
            ParentState::AwaitingUserResponse { .. } => "AwaitingUserResponse",
            ParentState::AwaitingCommissionReviewApproval { .. } => {
                "AwaitingCommissionReviewApproval"
            }
            ParentState::ContextExhausted { .. } => "ContextExhausted",
            ParentState::HandedOff { .. } => "HandedOff",
            ParentState::Terminal => "Terminal",
        }
    }

    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            ParentState::ContextExhausted { .. }
                | ParentState::HandedOff { .. }
                | ParentState::Terminal
        )
    }
}

impl SubAgentState {
    /// Stable variant name
    #[allow(dead_code)] // Will be used when callers migrate to split types
    #[must_use]
    pub fn variant_name(&self) -> &'static str {
        match self {
            SubAgentState::Core(c) => c.variant_name(),
            SubAgentState::Completed { .. } => "Completed",
            SubAgentState::Failed { .. } => "Failed",
        }
    }

    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            SubAgentState::Completed { .. } | SubAgentState::Failed { .. }
        )
    }

    /// Get reference to core state if this is a Core variant
    #[allow(dead_code)] // Will be used when callers migrate to split types
    #[must_use]
    pub fn as_core(&self) -> Option<&CoreState> {
        match self {
            SubAgentState::Core(c) => Some(c),
            SubAgentState::Completed { .. } | SubAgentState::Failed { .. } => None,
        }
    }
}

/// Outcome of user's decision on a proposed task plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TaskApprovalOutcome {
    Approved {
        #[serde(default)]
        handoff: TaskApprovalHandoff,
    },
    Rejected,
    FeedbackProvided {
        annotations: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TaskApprovalHandoff {
    #[default]
    ContinueInCurrentConversation,
    StartFreshWorkConversation,
}

/// Semantic state category for UI display.
///
/// Single source of truth for how conversation states map to visual indicators.
/// The API serializes this so the UI never re-derives state categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayState {
    /// Ready for user input (green dot, static)
    Idle,
    /// Agent is processing (yellow dot, pulsing)
    Working,
    /// Retryable error occurred (red dot)
    Error,
    /// Conversation cannot continue — context exhausted, completed, or failed (gray dot, static)
    Terminal,
    /// Awaiting user action on a proposed task plan (REQ-BED-028)
    AwaitingApproval,
}

/// Executor lifecycle signal — forces explicit handling of terminal states (FM-5 prevention).
///
/// The executor loop checks this after every transition. `Terminal` means the loop
/// must exit — no reliance on channel-drop semantics.
#[derive(Debug, Clone, PartialEq)]
pub enum StepResult {
    Continue,
    Terminal(TerminalOutcome),
}

/// Why the executor is exiting.
#[derive(Debug, Clone, PartialEq)]
pub enum TerminalOutcome {
    /// Sub-agent completed successfully
    Completed(String),
    /// Sub-agent or conversation failed
    Failed(String, ErrorKind),
    /// Context window exhausted — conversation is read-only
    ContextExhausted { summary: String },
    /// Task lifecycle ended (complete or abandon) — conversation is permanently read-only
    TaskResolved,
}

impl ConvState {
    /// Check if this is a terminal state — cannot transition out.
    /// `Completed`/`Failed` are sub-agent specific; `Terminal` is the
    /// user-facing lifecycle end state (complete/abandon).
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            ConvState::Completed { .. }
                | ConvState::Failed { .. }
                | ConvState::ContextExhausted { .. }
                | ConvState::HandedOff { .. }
                | ConvState::Terminal
        )
    }

    /// Mirror of the Allium-defined `is_busy` derivation in
    /// `specs/bedrock/bedrock.allium`:
    ///
    /// > `is_busy: core_status in { llm_requesting, executing_tools,
    /// >                            awaiting_sub_agents, cancelling_tool,
    /// >                            cancelling_sub_agents }`
    ///
    /// Used by REQ-BED-032's `RejectHardDeleteWhileBusy` rule. The
    /// hard-delete cascade refuses to fire while busy because the cleanup
    /// would race the in-flight tool execution's own teardown.
    #[must_use]
    pub fn is_busy(&self) -> bool {
        matches!(
            self,
            ConvState::LlmRequesting { .. }
                | ConvState::SeededLlmRequesting { .. }
                | ConvState::ToolExecuting { .. }
                | ConvState::CancellingTool { .. }
                | ConvState::AwaitingSubAgents { .. }
                | ConvState::CancellingSubAgents { .. }
        )
    }

    /// True when `UserCancel` is a supported user action for this state.
    #[must_use]
    pub fn allows_user_cancel(&self) -> bool {
        matches!(
            self,
            ConvState::LlmRequesting { .. }
                | ConvState::SeededLlmRequesting { .. }
                | ConvState::ToolExecuting { .. }
                | ConvState::AwaitingSubAgents { .. }
                | ConvState::AwaitingTaskApproval { .. }
                | ConvState::AwaitingCommissionReviewApproval { .. }
                | ConvState::AwaitingRecovery { .. }
        )
    }

    /// True only for `Idle` and `Error` — the states with nothing in
    /// flight that a model swap would race. Error-state recovery
    /// ("pick another model") is specified by REQ-LLM-006.
    #[must_use]
    pub fn allows_model_change(&self) -> bool {
        matches!(self, ConvState::Idle | ConvState::Error { .. })
    }

    #[must_use]
    pub fn error_kind(&self) -> Option<&ErrorKind> {
        match self {
            Self::Error { error_kind, .. }
            | Self::Failed { error_kind, .. }
            | Self::AwaitingRecovery { error_kind, .. } => Some(error_kind),
            Self::Idle
            | Self::LlmRequesting { .. }
            | Self::SeededLlmRequesting { .. }
            | Self::ToolExecuting { .. }
            | Self::CancellingTool { .. }
            | Self::AwaitingSubAgents { .. }
            | Self::CancellingSubAgents { .. }
            | Self::Completed { .. }
            | Self::AwaitingContinuation { .. }
            | Self::AwaitingTaskApproval { .. }
            | Self::AwaitingUserResponse { .. }
            | Self::AwaitingCommissionReviewApproval { .. }
            | Self::ContextExhausted { .. }
            | Self::HandedOff { .. }
            | Self::Terminal => None,
        }
    }

    /// Stable, payload-free name of this variant. Used by structured
    /// error types (e.g. `TransitionError::InvalidTransition`) and
    /// tracing so they can carry a state discriminator without the
    /// `Debug` format of the variant's payloads — task 24682 follow-up.
    /// This is the single source of truth; do not inline another
    /// `match self { ... => "Name" }` elsewhere.
    #[must_use]
    pub fn variant_name(&self) -> &'static str {
        match self {
            ConvState::Idle => "Idle",
            ConvState::LlmRequesting { .. } => "LlmRequesting",
            ConvState::SeededLlmRequesting { .. } => "SeededLlmRequesting",
            ConvState::ToolExecuting { .. } => "ToolExecuting",
            ConvState::CancellingTool { .. } => "CancellingTool",
            ConvState::AwaitingSubAgents { .. } => "AwaitingSubAgents",
            ConvState::CancellingSubAgents { .. } => "CancellingSubAgents",
            ConvState::Completed { .. } => "Completed",
            ConvState::Failed { .. } => "Failed",
            ConvState::Error { .. } => "Error",
            ConvState::AwaitingRecovery { .. } => "AwaitingRecovery",
            ConvState::AwaitingContinuation { .. } => "AwaitingContinuation",
            ConvState::ContextExhausted { .. } => "ContextExhausted",
            ConvState::HandedOff { .. } => "HandedOff",
            ConvState::AwaitingTaskApproval { .. } => "AwaitingTaskApproval",
            ConvState::AwaitingUserResponse { .. } => "AwaitingUserResponse",
            ConvState::AwaitingCommissionReviewApproval { .. } => {
                "AwaitingCommissionReviewApproval"
            }
            ConvState::Terminal => "Terminal",
        }
    }

    /// Structural terminal-state check for the executor loop.
    ///
    /// Returns `StepResult::Terminal` for states that cannot produce further transitions,
    /// forcing the executor to exit explicitly rather than relying on channel lifecycle.
    #[must_use]
    pub fn step_result(&self) -> StepResult {
        match self {
            ConvState::Completed { result } => {
                StepResult::Terminal(TerminalOutcome::Completed(result.clone()))
            }
            ConvState::Failed { error, error_kind } => {
                StepResult::Terminal(TerminalOutcome::Failed(error.clone(), error_kind.clone()))
            }
            ConvState::ContextExhausted { summary, .. } => {
                StepResult::Terminal(TerminalOutcome::ContextExhausted {
                    summary: summary.clone(),
                })
            }
            ConvState::Terminal | ConvState::HandedOff { .. } => {
                StepResult::Terminal(TerminalOutcome::TaskResolved)
            }
            ConvState::Idle
            | ConvState::LlmRequesting { .. }
            | ConvState::SeededLlmRequesting { .. }
            | ConvState::ToolExecuting { .. }
            | ConvState::CancellingTool { .. }
            | ConvState::AwaitingSubAgents { .. }
            | ConvState::CancellingSubAgents { .. }
            | ConvState::Error { .. }
            | ConvState::AwaitingRecovery { .. }
            | ConvState::AwaitingContinuation { .. }
            | ConvState::AwaitingTaskApproval { .. }
            | ConvState::AwaitingUserResponse { .. }
            | ConvState::AwaitingCommissionReviewApproval { .. } => StepResult::Continue,
        }
    }

    /// Typed presentation mode for the frontend.
    ///
    /// Maps states to the 5 presentation variants the UI renders.
    /// Note: `ContextExhausted` always returns `"needs_action"` here.
    /// Callers that have a full `Conversation` and want the `"done"` variant
    /// for the case where `continued_in_conv_id.is_some()` must override this.
    #[must_use]
    pub fn presentation_mode(&self) -> &'static str {
        match self {
            ConvState::Idle => "idle",
            ConvState::Error { .. } => "error",
            ConvState::AwaitingTaskApproval { .. }
            | ConvState::AwaitingUserResponse { .. }
            | ConvState::AwaitingCommissionReviewApproval { .. }
            | ConvState::ContextExhausted { .. } => "needs_action",
            ConvState::HandedOff { .. }
            | ConvState::Terminal
            | ConvState::Completed { .. }
            | ConvState::Failed { .. } => "done",
            ConvState::LlmRequesting { .. }
            | ConvState::SeededLlmRequesting { .. }
            | ConvState::ToolExecuting { .. }
            | ConvState::CancellingTool { .. }
            | ConvState::AwaitingSubAgents { .. }
            | ConvState::CancellingSubAgents { .. }
            | ConvState::AwaitingRecovery { .. }
            | ConvState::AwaitingContinuation { .. } => "working",
        }
    }

    /// Semantic category for UI display. This is the single source of truth
    /// for mapping raw conversation states to visual indicators.
    #[must_use]
    pub fn display_state(&self) -> DisplayState {
        match self {
            ConvState::Idle => DisplayState::Idle,
            ConvState::Error { .. } => DisplayState::Error,
            ConvState::AwaitingTaskApproval { .. }
            | ConvState::AwaitingUserResponse { .. }
            | ConvState::AwaitingCommissionReviewApproval { .. } => DisplayState::AwaitingApproval,
            ConvState::ContextExhausted { .. }
            | ConvState::HandedOff { .. }
            | ConvState::Completed { .. }
            | ConvState::Failed { .. }
            | ConvState::Terminal => DisplayState::Terminal,
            ConvState::LlmRequesting { .. }
            | ConvState::SeededLlmRequesting { .. }
            | ConvState::ToolExecuting { .. }
            | ConvState::CancellingTool { .. }
            | ConvState::AwaitingSubAgents { .. }
            | ConvState::CancellingSubAgents { .. }
            | ConvState::AwaitingRecovery { .. }
            | ConvState::AwaitingContinuation { .. } => DisplayState::Working,
        }
    }
}

// ============================================================================
// Sub-Agent Types
// ============================================================================

/// Mode for sub-agent execution (REQ-PROJ-008)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SubAgentMode {
    /// Read-only tools, cheaper model default (haiku)
    #[default]
    Explore,
    /// Full tool suite, inherits parent model
    Work,
}

/// Outcome of a sub-agent execution - pit of success design
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SubAgentOutcome {
    Success {
        result: String,
    },
    Failure {
        error: String,
        error_kind: ErrorKind,
    },
    /// Sub-agent exceeded its time limit (REQ-SA-006)
    TimedOut,
}

/// A sub-agent that is still running.
///
/// Only ever held inside transient states (`ToolExecuting`, `CancellingTool`,
/// `AwaitingSubAgents`, `CancellingSubAgents`), all of which `reset_all_to_idle`
/// resets on startup — so `mode` is strict-deserialized with no `serde(default)`
/// (task 13014).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PendingSubAgent {
    pub agent_id: String,
    pub task: String,
    pub mode: SubAgentMode,
}

/// Result from a completed sub-agent
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubAgentResult {
    pub agent_id: String,
    pub task: String,
    pub outcome: SubAgentOutcome,
}

/// Specification for spawning a sub-agent (used in effects)
#[derive(Debug, Clone, PartialEq)]
pub struct SubAgentSpec {
    pub agent_id: String,
    pub task: String,
    pub cwd: String,
    /// Mandatory timeout — caller must make a conscious decision (REQ-SA-006)
    pub timeout: Duration,
    /// Sub-agent execution mode (REQ-PROJ-008)
    pub mode: SubAgentMode,
    /// Resolved model ID for this sub-agent
    pub model_id: String,
    /// Maximum LLM turns before forced completion
    pub max_turns: u32,
    /// Named agent that produced this spec, if any (see `specs/agents/`).
    pub agent_name: Option<String>,
    /// Resolved agent persona; replaces the base preamble in the sub-agent's
    /// system prompt (REQ-AG-006). `None` for anonymous spawns.
    pub persona: Option<String>,
}

/// How a conversation handles approaching context limits
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ContextExhaustionBehavior {
    /// Normal conversations: trigger continuation at 90% threshold
    #[default]
    ThresholdBasedContinuation,
    /// Sub-agents: fail immediately (no continuation flow)
    IntentionallyUnhandled,
}

/// Simplified mode identifier for state machine guards.
/// The full `ConvMode` (with branch names, worktree paths, etc.) is not needed --
/// only which category matters for transition-level defense.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeKind {
    Direct,
    Managed, // Explore or Work
    Branch,
}

/// Context for a conversation (immutable configuration)
#[derive(Debug, Clone)]
pub struct ConvContext {
    pub conversation_id: String,
    /// The top-level conversation that owns this work tree.
    /// For root conversations this equals `conversation_id`.
    /// For sub-agents it is the root ancestor's id.
    pub root_conversation_id: String,
    pub working_dir: PathBuf,
    #[allow(dead_code)] // Used by LLM client selection
    pub model_id: String,
    /// Whether this is a sub-agent conversation
    pub is_sub_agent: bool,
    /// Model's context window size in tokens
    pub context_window: usize,
    /// How this conversation handles context exhaustion
    pub context_exhaustion_behavior: ContextExhaustionBehavior,
    /// Conversation mode context for system prompt (stable per mode, updated on Explore->Work)
    pub mode_context: Option<crate::domain::mode_context::ModeContext>,
    /// Maximum LLM turns for this conversation (0 = unlimited, for parent conversations)
    pub max_turns: u32,
    /// Desired base branch for Managed mode (set at creation, consumed at task approval)
    pub desired_base_branch: Option<String>,
    /// Mode category for transition-level guards (defense-in-depth behind tool registry)
    pub mode: ModeKind,
    /// The worktree path that defines this conversation's `WorkScope`, taken
    /// verbatim from the persisted `ConvMode::worktree_path()`. `Some` for
    /// Work/Branch and top-level Explore conversations (which own a worktree),
    /// `None` for Direct conversations and sub-agent Explore conversations
    /// (which share the parent's working directory but have no worktree of
    /// their own). The executor resolves `ToolContext.work_scope` from this so
    /// the scope keying matches every DB-facing path that derives scope from
    /// `WorkScope::resolve(conv.id, conv.conv_mode.worktree_path())`.
    pub work_scope_worktree: Option<PathBuf>,
    /// Relative name of the project's tasks directory (e.g. `"tasks"` or
    /// `"taskmds"`). Discovered at conversation startup via
    /// `taskmd_core::discover::discover_or_default` and cached here so the
    /// state machine, executor, patch-tool registration, and system prompt
    /// all agree on the same name without re-walking the worktree on every
    /// reference.
    pub tasks_dir_name: String,
    /// LLM-facing prose language for this conversation. Drives the system
    /// prompt builder and tool description selection.
    pub llm_language: crate::llm_language::LlmLanguage,
    /// Named-agent persona for this conversation, when spawned from a named
    /// agent (see `specs/agents/`). Replaces the base preamble in the system
    /// prompt (REQ-AG-006). Only ever set for sub-agents.
    pub persona: Option<String>,
}

/// Default context window for unknown models (conservative)
pub const DEFAULT_CONTEXT_WINDOW: usize = 128_000;

impl ConvContext {
    pub fn new(
        conversation_id: impl Into<String>,
        working_dir: PathBuf,
        model_id: impl Into<String>,
        context_window: usize,
    ) -> Self {
        let id = conversation_id.into();
        Self {
            root_conversation_id: id.clone(),
            conversation_id: id,
            working_dir,
            model_id: model_id.into(),
            is_sub_agent: false,
            context_window,
            context_exhaustion_behavior: ContextExhaustionBehavior::ThresholdBasedContinuation,
            mode_context: None,
            max_turns: 0,
            desired_base_branch: None,
            mode: ModeKind::Managed,
            work_scope_worktree: None,
            tasks_dir_name: taskmd_core::constants::DEFAULT_TASKS_DIR_NAME.to_string(),
            llm_language: crate::llm_language::LlmLanguage::default(),
            persona: None,
        }
    }

    /// Create a sub-agent context
    pub fn sub_agent(
        conversation_id: impl Into<String>,
        working_dir: PathBuf,
        model_id: impl Into<String>,
        context_window: usize,
        root_conversation_id: impl Into<String>,
    ) -> Self {
        Self {
            conversation_id: conversation_id.into(),
            root_conversation_id: root_conversation_id.into(),
            working_dir,
            model_id: model_id.into(),
            is_sub_agent: true,
            context_window,
            context_exhaustion_behavior: ContextExhaustionBehavior::IntentionallyUnhandled,
            mode_context: None,
            max_turns: 0,
            desired_base_branch: None,
            mode: ModeKind::Managed,
            work_scope_worktree: None,
            tasks_dir_name: taskmd_core::constants::DEFAULT_TASKS_DIR_NAME.to_string(),
            llm_language: crate::llm_language::LlmLanguage::default(),
            persona: None,
        }
    }
}
