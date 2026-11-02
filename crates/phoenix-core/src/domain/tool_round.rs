use chrono::{DateTime, Utc};
use serde_json::Value;
use std::collections::HashSet;
use std::fmt;

use super::db_schema::{
    InputOrigin, Message, MessageContent, MessageType, ToolContent, ToolResult,
};
use super::llm_types::ContentBlock;
use super::sm_state::{AssistantMessage, ToolCall};
use super::tool_result_identity::tool_result_message_id;

/// An assistant/result payload with unique, paired identities and supplied order.
#[derive(Debug, Clone)]
pub struct ToolRoundAssembly {
    assistant_message: AssistantMessage,
    tool_results: Vec<ToolResult>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolRoundError {
    ResultCountMismatch { tool_uses: usize, results: usize },
    DuplicateCall { tool_use_id: String },
    UnknownResult { tool_use_id: String },
    DuplicateResult { tool_use_id: String },
}

impl fmt::Display for ToolRoundError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ResultCountMismatch { tool_uses, results } => write!(
                f,
                "tool_use count ({tool_uses}) != tool_result count ({results})"
            ),
            Self::DuplicateCall { .. } => {
                write!(f, "duplicate tool_use identity")
            }
            Self::UnknownResult { .. } => {
                write!(f, "result has no owning tool_use")
            }
            Self::DuplicateResult { .. } => {
                write!(f, "duplicate tool_result identity")
            }
        }
    }
}

impl std::error::Error for ToolRoundError {}

/// Validate one-to-one tool identity correspondence without imposing order.
///
/// # Errors
/// Returns a count mismatch, duplicate call/result, or unknown result identity.
pub fn validate_tool_round_ids<'a>(
    tool_uses: impl IntoIterator<Item = &'a str>,
    results: impl IntoIterator<Item = &'a str>,
) -> Result<(), ToolRoundError> {
    let calls: Vec<_> = tool_uses.into_iter().collect();
    let results: Vec<_> = results.into_iter().collect();
    if calls.len() != results.len() {
        return Err(ToolRoundError::ResultCountMismatch {
            tool_uses: calls.len(),
            results: results.len(),
        });
    }
    let mut call_ids = HashSet::with_capacity(calls.len());
    for id in calls {
        if !call_ids.insert(id) {
            return Err(ToolRoundError::DuplicateCall {
                tool_use_id: id.into(),
            });
        }
    }
    let mut seen = HashSet::with_capacity(results.len());
    for id in results {
        if !call_ids.contains(id) {
            return Err(ToolRoundError::UnknownResult {
                tool_use_id: id.into(),
            });
        }
        if !seen.insert(id) {
            return Err(ToolRoundError::DuplicateResult {
                tool_use_id: id.into(),
            });
        }
    }
    Ok(())
}

/// Check Phoenix-managed calls against the assistant's Phoenix tool blocks.
///
/// # Errors
/// Returns the identity correspondence error for a malformed response schedule.
pub fn validate_response_tool_calls(
    content: &[ContentBlock],
    tool_calls: &[ToolCall],
) -> Result<(), ToolRoundError> {
    validate_tool_round_ids(
        phoenix_tool_ids(content),
        tool_calls.iter().map(|call| call.id.as_str()),
    )
}

fn phoenix_tool_ids(content: &[ContentBlock]) -> impl Iterator<Item = &str> {
    content.iter().filter_map(|block| {
        if let ContentBlock::ToolUse { id, .. } = block {
            Some(id.as_str())
        } else {
            None
        }
    })
}

/// Reject an output that does not belong to the invocation reporting completion.
#[must_use]
pub fn result_for_invocation(result: ToolResult, expected_id: &str) -> ToolResult {
    if result.tool_use_id == expected_id {
        result
    } else {
        tracing::warn!(expected_id, actual_id = %result.tool_use_id, "rejecting tool result for a different invocation");
        ToolResult::error(
            expected_id.to_owned(),
            "Tool returned a result for a different invocation".into(),
        )
    }
}

impl ToolRoundAssembly {
    /// Append a policy-produced tail to completed results without reordering.
    ///
    /// # Errors
    /// Returns an error when call and result identities do not correspond one-to-one.
    pub fn new(
        assistant_message: AssistantMessage,
        mut completed_results: Vec<ToolResult>,
        remaining_results: impl IntoIterator<Item = ToolResult>,
    ) -> Result<Self, ToolRoundError> {
        completed_results.extend(remaining_results);
        validate_tool_round_ids(
            phoenix_tool_ids(&assistant_message.content),
            completed_results
                .iter()
                .map(|result| result.tool_use_id.as_str()),
        )?;
        Ok(Self {
            assistant_message,
            tool_results: completed_results,
        })
    }

    #[must_use]
    pub fn assistant_message(&self) -> &AssistantMessage {
        &self.assistant_message
    }

    #[must_use]
    pub fn results(&self) -> &[ToolResult] {
        &self.tool_results
    }

    #[must_use]
    pub fn into_parts(self) -> (AssistantMessage, Vec<ToolResult>) {
        (self.assistant_message, self.tool_results)
    }
}

/// Construct a tool row from caller-projected content and display metadata.
#[must_use]
pub fn tool_result_message(
    assistant_message_id: &str,
    conversation_id: &str,
    sequence_id: i64,
    created_at: DateTime<Utc>,
    content: ToolContent,
    display_data: Option<Value>,
) -> Message {
    Message {
        message_id: tool_result_message_id(assistant_message_id, &content.tool_use_id),
        conversation_id: conversation_id.to_owned(),
        sequence_id,
        message_type: MessageType::Tool,
        content: MessageContent::Tool(content),
        created_at,
        display_data,
        usage_data: None,
        origin: InputOrigin::SystemGenerated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::db_schema::{ToolContentImage, ToolOutcome, UsageData};
    use crate::domain::llm_types::ContentBlock;

    fn assistant(ids: &[&str]) -> AssistantMessage {
        let mut assistant = AssistantMessage::new(
            "round:1".into(),
            ids.iter()
                .map(|id| ContentBlock::tool_use(*id, "think", serde_json::json!({})))
                .collect(),
            Some(UsageData::default()),
            Some(serde_json::json!({"annotation": ["retained", 42]})),
        );
        assistant.created_at = DateTime::from_timestamp(100, 123).unwrap();
        assistant
    }

    #[test]
    fn assembly_preserves_all_assistant_and_result_fields() {
        let assistant = assistant(&["a", "b", "c", "d"]);
        let results = vec![
            ToolResult {
                tool_use_id: "a".into(),
                outcome: ToolOutcome::Success {
                    output: "image result".into(),
                    display_data: Some(serde_json::json!({"duration_ms": 999, "caption": "a"})),
                    images: vec![ToolContentImage {
                        media_type: "image/png".into(),
                        data: "image-bytes".into(),
                    }],
                },
                duration_ms: Some(17),
            },
            ToolResult {
                tool_use_id: "b".into(),
                outcome: ToolOutcome::TrustedInstructions {
                    output: "live instructions".into(),
                },
                duration_ms: Some(0),
            },
            ToolResult::error("c".into(), "concrete failure".into()),
            ToolResult::cancelled("d".into(), "cancelled"),
        ];
        let round = ToolRoundAssembly::new(
            assistant.clone(),
            results[..2].to_vec(),
            results[2..].iter().cloned(),
        )
        .unwrap();
        assert_eq!(round.assistant_message(), &assistant);
        assert_eq!(round.results(), results);
        let (actual_assistant, actual_results) = round.into_parts();
        assert_eq!(actual_assistant, assistant);
        assert_eq!(actual_results, results);
    }

    #[test]
    fn assembly_preserves_completed_prefix_and_recovered_result_order() {
        let round = ToolRoundAssembly::new(
            assistant(&["a", "b", "c"]),
            vec![
                ToolResult::success("a".into(), "completed".into()),
                ToolResult::success("c".into(), "recovered".into()),
            ],
            [ToolResult::error("b".into(), "interrupted".into())],
        )
        .unwrap();
        assert_eq!(
            round
                .results()
                .iter()
                .map(|result| result.tool_use_id.as_str())
                .collect::<Vec<_>>(),
            ["a", "c", "b"]
        );
        assert_eq!(round.results()[0].output(), "completed");
        assert_eq!(round.results()[1].output(), "recovered");
        assert_eq!(round.results()[2].output(), "interrupted");
    }

    #[test]
    fn incomplete_and_overfull_rounds_return_the_existing_count_error() {
        for count in [0, 2] {
            let error = ToolRoundAssembly::new(
                assistant(&["a"]),
                vec![],
                (0..count).map(|_| ToolResult::error("a".into(), "error".into())),
            )
            .unwrap_err();
            assert_eq!(
                error,
                ToolRoundError::ResultCountMismatch {
                    tool_uses: 1,
                    results: count
                }
            );
            assert_eq!(
                error.to_string(),
                format!("tool_use count (1) != tool_result count ({count})")
            );
        }
    }

    #[test]
    fn assembly_rejects_equal_count_identity_mismatches() {
        for (calls, results) in [
            (vec!["a", "b", "c"], vec!["a", "a", "c"]),
            (vec!["a", "b", "c"], vec!["a", "b", "unknown"]),
            (vec!["a", "a"], vec!["a", "a"]),
        ] {
            assert!(ToolRoundAssembly::new(
                assistant(&calls),
                results
                    .into_iter()
                    .map(|id| ToolResult::success(id.into(), "retained output".into()))
                    .collect(),
                [],
            )
            .is_err());
        }
    }

    #[test]
    fn response_validation_pairs_only_phoenix_managed_tool_blocks() {
        let call = ToolCall::new(
            "phoenix",
            super::super::sm_state::ToolInput::Think(super::super::sm_state::ThinkInput {
                thoughts: "reason".into(),
            }),
        );
        let phoenix = ContentBlock::tool_use("phoenix", "think", serde_json::json!({}));
        let server: ContentBlock = serde_json::from_value(serde_json::json!({
            "type": "server_tool_use", "id": "server", "name": "web_search", "input": {}
        }))
        .unwrap();
        assert!(validate_response_tool_calls(&[server.clone()], &[]).is_ok());
        assert!(validate_response_tool_calls(
            &[server, phoenix.clone()],
            std::slice::from_ref(&call)
        )
        .is_ok());
        assert!(validate_response_tool_calls(&[phoenix], &[]).is_err());
        assert!(validate_response_tool_calls(&[], std::slice::from_ref(&call)).is_err());
        assert!(validate_response_tool_calls(
            &[ContentBlock::tool_use(
                "other",
                "think",
                serde_json::json!({})
            )],
            &[call],
        )
        .is_err());
    }
}
