use chrono::{DateTime, Utc};
use serde_json::Value;
use std::fmt;

use super::db_schema::{
    InputOrigin, Message, MessageContent, MessageType, ToolContent, ToolResult,
};
use super::sm_state::AssistantMessage;
use super::tool_result_identity::tool_result_message_id;

/// An assistant turn and its complete, caller-ordered result sequence.
#[derive(Debug, Clone)]
pub struct CompletedToolRound {
    assistant_message: AssistantMessage,
    tool_results: Vec<ToolResult>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolRoundError {
    ResultCountMismatch { tool_uses: usize, results: usize },
}

impl fmt::Display for ToolRoundError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ResultCountMismatch { tool_uses, results } => write!(
                f,
                "tool_use count ({tool_uses}) != tool_result count ({results})"
            ),
        }
    }
}

impl CompletedToolRound {
    /// Append a policy-produced tail to completed results without reordering.
    ///
    /// # Errors
    /// Returns a count mismatch when the assistant and results differ in length.
    pub fn new(
        assistant_message: AssistantMessage,
        mut completed_results: Vec<ToolResult>,
        remaining_results: impl IntoIterator<Item = ToolResult>,
    ) -> Result<Self, ToolRoundError> {
        completed_results.extend(remaining_results);
        let tool_uses = assistant_message.tool_uses().len();
        if tool_uses != completed_results.len() {
            return Err(ToolRoundError::ResultCountMismatch {
                tool_uses,
                results: completed_results.len(),
            });
        }
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
        let round = CompletedToolRound::new(
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
    fn assembly_preserves_supplied_order_and_existing_identifier_acceptance() {
        let round = CompletedToolRound::new(
            assistant(&["a", "b", "c"]),
            vec![ToolResult::success("c".into(), "recovered".into())],
            [
                ToolResult::error("duplicate".into(), "first".into()),
                ToolResult::error("duplicate".into(), "second".into()),
            ],
        )
        .unwrap();
        assert_eq!(
            round
                .results()
                .iter()
                .map(|result| result.tool_use_id.as_str())
                .collect::<Vec<_>>(),
            ["c", "duplicate", "duplicate"]
        );
        assert_eq!(round.results()[1].output(), "first");
        assert_eq!(round.results()[2].output(), "second");
    }

    #[test]
    fn incomplete_and_overfull_rounds_return_the_existing_count_error() {
        for count in [0, 2] {
            let error = CompletedToolRound::new(
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
}
