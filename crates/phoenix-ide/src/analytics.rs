//! Phoenix-native analytics projections.
//!
//! The projection reads Phoenix history (`conversations`, `messages`, and
//! `turn_usage`) and produces typed analytics facts without persisting a second
//! transcript or tool I/O store.

use crate::api::usage::{calculate_turn_cost, TurnCost};
use crate::db::{
    ConvMode, Conversation, Database, Message, MessageContent, UsageAnchorRow, UsageTurnRow,
};
use chrono::{DateTime, Utc};
use phoenix_core::domain::llm_types::ContentBlock;
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AnalyticsFidelityValue {
    Native,
    Derived,
    Estimated,
    Unknown,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AnalyticsFidelity {
    pub tokens: AnalyticsFidelityValue,
    pub cost: AnalyticsFidelityValue,
    pub tool_calls: AnalyticsFidelityValue,
    pub first_byte: AnalyticsFidelityValue,
    pub retries: AnalyticsFidelityValue,
    pub outcomes: AnalyticsFidelityValue,
    pub lifecycle: AnalyticsFidelityValue,
}

impl AnalyticsFidelity {
    fn v1(first_byte_available: bool, unknown_cost: bool) -> Self {
        Self {
            tokens: AnalyticsFidelityValue::Native,
            cost: if unknown_cost {
                AnalyticsFidelityValue::Unknown
            } else {
                AnalyticsFidelityValue::Estimated
            },
            tool_calls: AnalyticsFidelityValue::Derived,
            first_byte: if first_byte_available {
                AnalyticsFidelityValue::Native
            } else {
                AnalyticsFidelityValue::Unavailable
            },
            retries: AnalyticsFidelityValue::Unavailable,
            outcomes: AnalyticsFidelityValue::Unavailable,
            lifecycle: AnalyticsFidelityValue::Unavailable,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[allow(clippy::struct_field_names)]
pub struct TokenTotals {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_creation_tokens: i64,
    pub cache_read_tokens: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct AnalyticsUsageTurn {
    pub turn_usage_id: i64,
    pub conversation_id: String,
    pub root_conversation_id: String,
    pub model: String,
    pub created_at: DateTime<Utc>,
    pub first_byte_at: Option<DateTime<Utc>>,
    pub first_byte_latency_ms: Option<u64>,
    pub tokens: TokenTotals,
    pub cost: TurnCost,
}

#[derive(Debug, Clone, Serialize)]
pub struct AnalyticsToolCall {
    pub conversation_id: String,
    pub assistant_message_id: String,
    pub tool_result_message_id: Option<String>,
    pub tool_use_id: String,
    pub tool_name: String,
    pub is_error: bool,
    pub denied: bool,
    pub duration_ms: Option<u64>,
    pub normalized_command: Option<String>,
    pub touched_files: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AnalyticsSession {
    pub session_id: String,
    pub root_session_id: String,
    pub project_id: Option<String>,
    pub cwd: String,
    pub worktree_path: Option<String>,
    pub task_id: Option<String>,
    pub task_title: Option<String>,
    pub branch: Option<String>,
    pub started_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub terminal_status: Option<String>,
    pub turns: Vec<AnalyticsUsageTurn>,
    pub tool_calls: Vec<AnalyticsToolCall>,
    pub fidelity: AnalyticsFidelity,
}

#[derive(Debug, Clone, Serialize)]
pub struct TrajectoryExportPayload {
    pub client: &'static str,
    pub source: &'static str,
    pub session: AnalyticsSession,
}

pub async fn project_session(db: &Database, root_id: &str) -> Result<AnalyticsSession, String> {
    let root = db
        .get_conversation(root_id)
        .await
        .map_err(|e| e.to_string())?;
    let turn_rows = db
        .usage_conversation_turns(root_id)
        .await
        .map_err(|e| e.to_string())?;
    let anchor_rows = db
        .usage_anchor_messages(root_id)
        .await
        .map_err(|e| e.to_string())?;
    let turns = project_usage_turns(&root, &turn_rows, &anchor_rows);

    let conversation_ids = db
        .analytics_conversation_ids_for_root(root_id)
        .await
        .map_err(|e| e.to_string())?;

    let mut messages = Vec::new();
    for id in &conversation_ids {
        let mut conv_messages = db.get_messages(id).await.map_err(|e| e.to_string())?;
        messages.append(&mut conv_messages);
    }
    messages.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| a.sequence_id.cmp(&b.sequence_id))
            .then_with(|| a.message_id.cmp(&b.message_id))
    });

    let tool_calls = project_tool_calls(&messages);
    let first_seen = turns.first().map_or(root.created_at, |t| t.created_at);
    let last_seen = session_last_seen_at(&root, &turns, &messages);
    let first_byte_available = turns.iter().any(|t| t.first_byte_at.is_some());
    let unknown_cost = turns.iter().any(|t| !t.cost.pricing_known);

    Ok(AnalyticsSession {
        session_id: root.id.clone(),
        root_session_id: root.id.clone(),
        project_id: root.project_id.clone(),
        cwd: root.cwd.clone(),
        worktree_path: worktree_path(&root),
        task_id: task_id(&root),
        task_title: task_title(&root),
        branch: branch_name(&root),
        started_at: first_seen.min(root.created_at),
        last_seen_at: last_seen.max(root.updated_at),
        ended_at: terminal_status(&root).map(|_| root.state_updated_at),
        terminal_status: terminal_status(&root),
        turns,
        tool_calls,
        fidelity: AnalyticsFidelity::v1(first_byte_available, unknown_cost),
    })
}

pub(crate) async fn project_usage_turns_for_root(
    db: &Database,
    root_id: &str,
) -> Result<Vec<AnalyticsUsageTurn>, String> {
    let root = db
        .get_conversation(root_id)
        .await
        .map_err(|e| e.to_string())?;
    let turn_rows = db
        .usage_conversation_turns(root_id)
        .await
        .map_err(|e| e.to_string())?;
    let anchor_rows = db
        .usage_anchor_messages(root_id)
        .await
        .map_err(|e| e.to_string())?;
    Ok(project_usage_turns(&root, &turn_rows, &anchor_rows))
}

fn project_usage_turns(
    root: &Conversation,
    turn_rows: &[UsageTurnRow],
    anchor_rows: &[UsageAnchorRow],
) -> Vec<AnalyticsUsageTurn> {
    turn_rows
        .iter()
        .map(|r| {
            let first_byte_at = r
                .first_byte_at
                .as_deref()
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|t| t.with_timezone(&Utc));
            let created_at = DateTime::parse_from_rfc3339(&r.created_at)
                .map_or_else(|_| root.created_at, |t| t.with_timezone(&Utc));
            let first_byte_latency_ms = first_byte_at
                .and_then(|first| {
                    first_byte_anchor(anchor_rows, &r.conversation_id, first)
                        .map(|anchor| (first, anchor))
                })
                .and_then(|(first, anchor)| first.signed_duration_since(anchor).to_std().ok())
                .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
            let tokens = TokenTotals {
                input_tokens: r.input_tokens,
                output_tokens: r.output_tokens,
                cache_creation_tokens: r.cache_creation_tokens,
                cache_read_tokens: r.cache_read_tokens,
            };
            AnalyticsUsageTurn {
                turn_usage_id: r.id,
                conversation_id: r.conversation_id.clone(),
                root_conversation_id: r.root_conversation_id.clone(),
                model: r.model.clone(),
                created_at,
                first_byte_at,
                first_byte_latency_ms,
                tokens,
                cost: calculate_turn_cost(
                    &r.model,
                    r.input_tokens,
                    r.output_tokens,
                    r.cache_creation_tokens,
                    r.cache_read_tokens,
                ),
            }
        })
        .collect()
}

pub async fn trajectory_export(
    db: &Database,
    root_id: &str,
) -> Result<TrajectoryExportPayload, String> {
    let session = project_session(db, root_id).await?;
    Ok(TrajectoryExportPayload {
        client: "phoenix",
        source: "phoenix_conversation_history",
        session,
    })
}

fn session_last_seen_at(
    root: &Conversation,
    turns: &[AnalyticsUsageTurn],
    messages: &[Message],
) -> DateTime<Utc> {
    turns
        .iter()
        .map(|t| t.created_at)
        .max()
        .unwrap_or(root.updated_at)
        .max(root.updated_at)
        .max(
            messages
                .iter()
                .map(|m| m.created_at)
                .max()
                .unwrap_or(root.updated_at),
        )
}

fn project_tool_calls(messages: &[Message]) -> Vec<AnalyticsToolCall> {
    let mut calls = Vec::new();
    for (index, msg) in messages.iter().enumerate() {
        let MessageContent::Agent(blocks) = &msg.content else {
            continue;
        };
        for block in blocks {
            if let ContentBlock::ToolUse { id, name, input } = block {
                let result = following_tool_result(messages, index, &msg.conversation_id, id);
                let (is_error, denied, duration_ms) = result.map_or((false, false, None), |m| {
                    let MessageContent::Tool(content) = &m.content else {
                        return (false, false, None);
                    };
                    (
                        content.is_error,
                        content.is_error
                            && is_deterministic_denial(
                                content.content.as_str(),
                                m.display_data.as_ref(),
                            ),
                        duration_ms(m.display_data.as_ref()),
                    )
                });
                calls.push(AnalyticsToolCall {
                    conversation_id: msg.conversation_id.clone(),
                    assistant_message_id: msg.message_id.clone(),
                    tool_result_message_id: result.map(|m| m.message_id.clone()),
                    tool_use_id: id.clone(),
                    tool_name: name.clone(),
                    is_error,
                    denied,
                    duration_ms,
                    normalized_command: normalized_command(input),
                    touched_files: touched_files(input),
                });
            }
        }
    }
    calls
}

fn following_tool_result<'a>(
    messages: &'a [Message],
    assistant_index: usize,
    conversation_id: &str,
    tool_use_id: &str,
) -> Option<&'a Message> {
    messages.iter().skip(assistant_index + 1).find(|candidate| {
        candidate.conversation_id == conversation_id
            && matches!(
                &candidate.content,
                MessageContent::Tool(content) if content.tool_use_id == tool_use_id
            )
    })
}

fn first_byte_anchor(
    anchors: &[UsageAnchorRow],
    conversation_id: &str,
    first_byte_at: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    anchors
        .iter()
        .filter(|anchor| anchor.conversation_id == conversation_id)
        .filter_map(|anchor| {
            DateTime::parse_from_rfc3339(&anchor.created_at)
                .ok()
                .map(|t| t.with_timezone(&Utc))
        })
        .filter(|created_at| *created_at <= first_byte_at)
        .max()
}

fn duration_ms(display_data: Option<&Value>) -> Option<u64> {
    display_data
        .and_then(|v| v.get("duration_ms"))
        .and_then(Value::as_u64)
}

fn is_deterministic_denial(content: &str, display_data: Option<&Value>) -> bool {
    fn has_marker(v: &Value) -> bool {
        v.get("error")
            .and_then(Value::as_str)
            .is_some_and(|e| e == "command_safety_rejected")
    }

    display_data.is_some_and(has_marker)
        || serde_json::from_str::<Value>(content).is_ok_and(|v| has_marker(&v))
}

fn normalized_command(input: &Value) -> Option<String> {
    input
        .get("cmd")
        .or_else(|| input.get("command"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn touched_files(input: &Value) -> Vec<String> {
    let mut files = Vec::new();
    for key in ["path", "file", "file_path"] {
        if let Some(value) = input.get(key).and_then(Value::as_str) {
            files.push(value.to_string());
        }
    }
    files.sort();
    files.dedup();
    files
}

fn worktree_path(conv: &Conversation) -> Option<String> {
    match &conv.conv_mode {
        ConvMode::Explore { worktree_path, .. } => worktree_path.as_ref().map(ToString::to_string),
        ConvMode::Work { worktree_path, .. } | ConvMode::Branch { worktree_path, .. } => {
            Some(worktree_path.to_string())
        }
        ConvMode::Direct => None,
    }
}

fn branch_name(conv: &Conversation) -> Option<String> {
    match &conv.conv_mode {
        ConvMode::Work { branch_name, .. } | ConvMode::Branch { branch_name, .. } => {
            Some(branch_name.to_string())
        }
        ConvMode::Explore { .. } | ConvMode::Direct => None,
    }
}

fn task_id(conv: &Conversation) -> Option<String> {
    match &conv.conv_mode {
        ConvMode::Work { task_id, .. } => Some(task_id.to_string()),
        ConvMode::Explore { .. } | ConvMode::Direct | ConvMode::Branch { .. } => None,
    }
}

fn task_title(conv: &Conversation) -> Option<String> {
    match &conv.conv_mode {
        ConvMode::Work { task_title, .. } => Some(task_title.to_string()),
        ConvMode::Explore { .. } | ConvMode::Direct | ConvMode::Branch { .. } => None,
    }
}

fn terminal_status(conv: &Conversation) -> Option<String> {
    match &conv.state {
        crate::state_machine::ConvState::Completed { .. } => Some("completed".to_string()),
        crate::state_machine::ConvState::Failed { .. } => Some("failed".to_string()),
        crate::state_machine::ConvState::CreationFailed { .. } => {
            Some("creation_failed".to_string())
        }
        crate::state_machine::ConvState::CreationCancelled { .. } => {
            Some("creation_cancelled".to_string())
        }
        crate::state_machine::ConvState::Error { .. } => Some("error".to_string()),
        crate::state_machine::ConvState::ContextExhausted { .. } => {
            Some("context_exhausted".to_string())
        }
        crate::state_machine::ConvState::HandedOff { .. } => Some("handed_off".to_string()),
        crate::state_machine::ConvState::Terminal => Some("terminal".to_string()),
        crate::state_machine::ConvState::Idle
        | crate::state_machine::ConvState::LlmRequesting { .. }
        | crate::state_machine::ConvState::SeededLlmRequesting { .. }
        | crate::state_machine::ConvState::Provisioning { .. }
        | crate::state_machine::ConvState::ToolExecuting { .. }
        | crate::state_machine::ConvState::CancellingTool { .. }
        | crate::state_machine::ConvState::AwaitingSubAgents { .. }
        | crate::state_machine::ConvState::CancellingSubAgents { .. }
        | crate::state_machine::ConvState::AwaitingRecovery { .. }
        | crate::state_machine::ConvState::AwaitingContinuation { .. }
        | crate::state_machine::ConvState::AwaitingTaskApproval { .. }
        | crate::state_machine::ConvState::AwaitingUserResponse { .. }
        | crate::state_machine::ConvState::AwaitingCommissionReviewApproval { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Message, MessageContent, MessageType, ToolContent};
    use serde_json::json;

    fn msg(id: &str, seq: i64, content: MessageContent, display_data: Option<Value>) -> Message {
        msg_in("c", id, seq, Utc::now(), content, display_data)
    }

    fn msg_in(
        conversation_id: &str,
        id: &str,
        seq: i64,
        created_at: DateTime<Utc>,
        content: MessageContent,
        display_data: Option<Value>,
    ) -> Message {
        let message_type = match &content {
            MessageContent::Agent(_) => MessageType::Agent,
            MessageContent::Tool(_) => MessageType::Tool,
            MessageContent::User(_) => MessageType::User,
            MessageContent::System(_) => MessageType::System,
            MessageContent::Error(_) => MessageType::Error,
            MessageContent::Continuation(_) => MessageType::Continuation,
            MessageContent::Skill(_) => MessageType::Skill,
        };
        Message {
            message_id: id.to_string(),
            conversation_id: conversation_id.to_string(),
            sequence_id: seq,
            message_type,
            content,
            display_data,
            usage_data: None,
            created_at,
        }
    }

    #[test]
    fn project_tool_calls_pairs_results_and_denials() {
        let messages = vec![
            msg(
                "a1",
                1,
                MessageContent::Agent(vec![ContentBlock::ToolUse {
                    id: "tu1".to_string(),
                    name: "bash".to_string(),
                    input: json!({ "cmd": "rm -rf /tmp/nope" }),
                }]),
                None,
            ),
            msg(
                "t1",
                2,
                MessageContent::Tool(ToolContent::new(
                    "tu1",
                    r#"{"error":"command_safety_rejected"}"#,
                    true,
                )),
                Some(json!({ "duration_ms": 7 })),
            ),
        ];

        let calls = project_tool_calls(&messages);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].tool_result_message_id.as_deref(), Some("t1"));
        assert!(calls[0].is_error);
        assert!(calls[0].denied);
        assert_eq!(calls[0].duration_ms, Some(7));
        assert_eq!(
            calls[0].normalized_command.as_deref(),
            Some("rm -rf /tmp/nope")
        );
    }

    #[test]
    fn project_tool_calls_pairs_duplicate_ids_by_following_conversation_result() {
        let messages = vec![
            msg_in(
                "parent",
                "a-parent",
                1,
                Utc::now(),
                MessageContent::Agent(vec![ContentBlock::ToolUse {
                    id: "reused".to_string(),
                    name: "bash".to_string(),
                    input: json!({ "cmd": "echo parent" }),
                }]),
                None,
            ),
            msg_in(
                "parent",
                "t-parent",
                2,
                Utc::now(),
                MessageContent::Tool(ToolContent::new("reused", "parent result", false)),
                Some(json!({ "duration_ms": 11 })),
            ),
            msg_in(
                "sub",
                "a-sub",
                1,
                Utc::now(),
                MessageContent::Agent(vec![ContentBlock::ToolUse {
                    id: "reused".to_string(),
                    name: "bash".to_string(),
                    input: json!({ "cmd": "echo sub" }),
                }]),
                None,
            ),
            msg_in(
                "sub",
                "t-sub",
                2,
                Utc::now(),
                MessageContent::Tool(ToolContent::new("reused", "sub result", true)),
                Some(json!({ "duration_ms": 22 })),
            ),
        ];

        let calls = project_tool_calls(&messages);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].assistant_message_id, "a-parent");
        assert_eq!(calls[0].tool_result_message_id.as_deref(), Some("t-parent"));
        assert_eq!(calls[0].duration_ms, Some(11));
        assert!(!calls[0].is_error);
        assert_eq!(calls[1].assistant_message_id, "a-sub");
        assert_eq!(calls[1].tool_result_message_id.as_deref(), Some("t-sub"));
        assert_eq!(calls[1].duration_ms, Some(22));
        assert!(calls[1].is_error);
    }

    #[test]
    fn successful_tool_output_with_error_marker_is_not_denied() {
        let messages = vec![
            msg(
                "a1",
                1,
                MessageContent::Agent(vec![ContentBlock::ToolUse {
                    id: "tu1".to_string(),
                    name: "bash".to_string(),
                    input: json!({ "cmd": "echo fixture" }),
                }]),
                None,
            ),
            msg(
                "t1",
                2,
                MessageContent::Tool(ToolContent::new(
                    "tu1",
                    r#"{"error":"command_safety_rejected"}"#,
                    false,
                )),
                Some(json!({ "error": "command_safety_rejected" })),
            ),
        ];

        let calls = project_tool_calls(&messages);
        assert_eq!(calls.len(), 1);
        assert!(!calls[0].is_error);
        assert!(!calls[0].denied);
    }

    #[test]
    fn session_last_seen_includes_message_activity_after_usage() {
        let root = Conversation {
            id: "root".to_string(),
            slug: Some("root".to_string()),
            title: Some("root".to_string()),
            cwd: "/tmp".to_string(),
            parent_conversation_id: None,
            user_initiated: true,
            state: crate::state_machine::ConvState::Idle,
            state_updated_at: Utc::now(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            archived: false,
            transcript_generation: 1,
            model: Some("mock".to_string()),
            project_id: None,
            conv_mode: ConvMode::Direct,
            desired_base_branch: None,
            message_count: 0,
            seed_parent_id: None,
            seed_label: None,
            continued_in_conv_id: None,
            chain_name: None,
            llm_language: crate::llm_language::LlmLanguage::default(),
            spawned_from_conversation_id: None,
        };
        let later = root.updated_at + chrono::Duration::seconds(30);
        let messages = vec![msg_in(
            "sub",
            "sub-tool",
            1,
            later,
            MessageContent::Tool(ToolContent::new("tu", "ok", false)),
            None,
        )];

        let last_seen = session_last_seen_at(&root, &[], &messages);
        assert_eq!(last_seen, later);
    }

    #[test]
    fn first_byte_anchor_uses_source_conversation_preceding_message() {
        let start = Utc::now();
        let anchors = vec![
            UsageAnchorRow {
                conversation_id: "parent".to_string(),
                created_at: start.to_rfc3339(),
            },
            UsageAnchorRow {
                conversation_id: "sub".to_string(),
                created_at: (start + chrono::Duration::milliseconds(5)).to_rfc3339(),
            },
        ];

        assert_eq!(
            first_byte_anchor(&anchors, "sub", start + chrono::Duration::milliseconds(25)),
            Some(start + chrono::Duration::milliseconds(5))
        );
        assert_eq!(
            first_byte_anchor(
                &anchors,
                "parent",
                start + chrono::Duration::milliseconds(25)
            ),
            Some(start)
        );
    }
}
