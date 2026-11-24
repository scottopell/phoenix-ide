use serde_json::Value;

use super::{
    db_schema::SourceToolCall,
    llm_types::ContentBlock,
    sm_state::ConvState,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ExecutingSourceRefusal {
    #[error("source is not the exact executing tool occurrence")]
    NotExactExecutingSource,
}

/// Verifies that the recorded source is the sole canonical occurrence currently executing.
/// This is a read-only predicate; it does not grant mutation authority.
///
/// # Errors
/// Returns [`ExecutingSourceRefusal::NotExactExecutingSource`] unless the state,
/// source identity, tool name, and payload all identify the same executing occurrence.
pub fn verify_executing_source(
    state: &ConvState,
    source: &SourceToolCall,
    tool_name: &str,
    requested: &Value,
) -> Result<(), ExecutingSourceRefusal> {
    let invalid = ExecutingSourceRefusal::NotExactExecutingSource;
    let ConvState::ToolExecuting {
        current_tool,
        remaining_tools,
        completed_results,
        assistant_message,
        ..
    } = state
    else {
        return Err(invalid);
    };
    if source.message_id.trim().is_empty()
        || source.tool_use_id.trim().is_empty()
        || assistant_message.message_id != source.message_id
        || current_tool.id != source.tool_use_id
        || current_tool.name() != tool_name
        || remaining_tools.iter().any(|tool| tool.id == source.tool_use_id)
        || completed_results
            .iter()
            .any(|result| result.tool_use_id() == source.tool_use_id)
    {
        return Err(invalid);
    }
    let executing = current_tool.input.to_value();
    let mut occurrences = assistant_message.content.iter().filter_map(|block| {
        if let ContentBlock::ToolUse { id, name, input } = block {
            (id == &source.tool_use_id).then_some((name, input))
        } else {
            None
        }
    });
    let Some((name, canonical)) = occurrences.next() else {
        return Err(invalid);
    };
    if occurrences.next().is_some()
        || name != tool_name
        || canonical != &executing
        || canonical != requested
    {
        return Err(invalid);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    use crate::domain::sm_state::{AssistantMessage, ToolCall, ToolInput};

    fn fixture() -> (ConvState, SourceToolCall, Value) {
        let source = SourceToolCall {
            message_id: "assistant-1".into(),
            tool_use_id: "tool-1".into(),
        };
        let input = json!({"target": "conversation-1"});
        let state = ConvState::ToolExecuting {
            current_tool: ToolCall::new(
                "tool-1",
                ToolInput::Unknown {
                    name: "close_conversation".into(),
                    input: input.clone(),
                },
            ),
            remaining_tools: vec![],
            completed_results: vec![],
            pending_sub_agents: vec![],
            assistant_message: AssistantMessage::new(
                "assistant-1".into(),
                vec![ContentBlock::ToolUse {
                    id: "tool-1".into(),
                    name: "close_conversation".into(),
                    input: input.clone(),
                }],
                None,
                None,
            ),
        };
        (state, source, input)
    }

    fn assert_refused(state: &ConvState, source: &SourceToolCall, requested: &Value) {
        assert_eq!(
            verify_executing_source(state, source, "close_conversation", requested),
            Err(ExecutingSourceRefusal::NotExactExecutingSource)
        );
    }

    #[test]
    fn accepts_exact_executing_occurrence() {
        let (state, source, input) = fixture();
        assert_eq!(
            verify_executing_source(&state, &source, "close_conversation", &input),
            Ok(())
        );
    }

    #[test]
    fn rejects_non_executing_state_and_blank_or_wrong_source_identity() {
        let (state, source, input) = fixture();
        assert_refused(&ConvState::Idle, &source, &input);
        assert_refused(
            &state,
            &SourceToolCall {
                message_id: "  ".into(),
                ..source.clone()
            },
            &input,
        );
        assert_refused(
            &state,
            &SourceToolCall {
                tool_use_id: "".into(),
                ..source.clone()
            },
            &input,
        );
        assert_refused(
            &state,
            &SourceToolCall {
                message_id: "other-message".into(),
                ..source.clone()
            },
            &input,
        );
        assert_refused(
            &state,
            &SourceToolCall {
                tool_use_id: "other-tool".into(),
                ..source
            },
            &input,
        );
    }

    #[test]
    fn rejects_wrong_tool_name_and_payload_disagreement() {
        let (state, source, input) = fixture();
        assert_refused(&state, &source, &json!({"target": "other"}));
        assert_refused(&state, &source, &json!({"target": "conversation-1", "extra": true}));
        assert_refused(&state, &source, &json!({}));

        let mut wrong_name = state.clone();
        if let ConvState::ToolExecuting { assistant_message, .. } = &mut wrong_name {
            if let ContentBlock::ToolUse { name, .. } = &mut assistant_message.content[0] {
                *name = "other_tool".into();
            }
        }
        assert_refused(&wrong_name, &source, &input);

        let mut wrong_executing_input = state;
        if let ConvState::ToolExecuting { current_tool, .. } = &mut wrong_executing_input {
            *current_tool = ToolCall::new(
                "tool-1",
                ToolInput::Unknown {
                    name: "close_conversation".into(),
                    input: json!({"target": "other"}),
                },
            );
        }
        assert_refused(&wrong_executing_input, &source, &input);
    }

    #[test]
    fn rejects_missing_duplicate_remaining_and_completed_occurrences() {
        let (state, source, input) = fixture();

        let mut missing = state.clone();
        if let ConvState::ToolExecuting { assistant_message, .. } = &mut missing {
            assistant_message.content.clear();
        }
        assert_refused(&missing, &source, &input);

        let mut duplicate = state.clone();
        if let ConvState::ToolExecuting { assistant_message, .. } = &mut duplicate {
            assistant_message.content.push(assistant_message.content[0].clone());
        }
        assert_refused(&duplicate, &source, &input);

        let mut remaining = state.clone();
        if let ConvState::ToolExecuting { remaining_tools, .. } = &mut remaining {
            remaining_tools.push(ToolCall::new(
                "tool-1",
                ToolInput::Unknown {
                    name: "close_conversation".into(),
                    input: input.clone(),
                },
            ));
        }
        assert_refused(&remaining, &source, &input);

        let mut completed = state;
        if let ConvState::ToolExecuting { completed_results, .. } = &mut completed {
            completed_results.push(
                crate::domain::conversation_goal::ToolRoundResult::from(
                    crate::domain::db_schema::ToolResult::success("tool-1".into(), "done".into()),
                ),
            );
        }
        assert_refused(&completed, &source, &input);
    }
}
