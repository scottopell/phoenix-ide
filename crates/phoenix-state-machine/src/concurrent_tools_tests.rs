use crate::state::{AssistantMessage, ToolCall, ToolInput};
use crate::{transition, ConvContext, ConvState, Effect, Event};
use phoenix_core::domain::db_schema::ToolResult;
use phoenix_core::domain::llm_types::ContentBlock;
use std::path::PathBuf;

fn batch() -> ConvState {
    let call = |id: &str| {
        ToolCall::new(
            id,
            ToolInput::from_name_and_value("bash", serde_json::json!({"op":"run","cmd":"pwd"})),
        )
    };
    ConvState::ToolExecuting {
        current_tool: call("slow"),
        remaining_tools: vec![call("fast")],
        completed_results: vec![],
        pending_sub_agents: vec![],
        assistant_message: AssistantMessage::new(
            "batch-assistant".into(),
            ["slow", "fast"]
                .into_iter()
                .map(|id| {
                    ContentBlock::tool_use(id, "bash", serde_json::json!({"op":"run","cmd":"pwd"}))
                })
                .collect(),
            None,
            None,
        ),
    }
}

fn context() -> ConvContext {
    ConvContext::new(
        "batch",
        PathBuf::from("/tmp"),
        "test-model",
        200_000,
        16_384,
    )
}

#[test]
fn ordinary_sibling_result_is_accepted_before_the_representative_finishes() {
    let result = ToolResult::error("fast".into(), "individual sibling failure".into());
    let next = transition(
        &batch(),
        &context(),
        Event::ToolComplete {
            tool_use_id: "fast".into(),
            result: result.clone(),
        },
    )
    .expect("an exact unresolved ordinary sibling must settle independently");
    let ConvState::ToolExecuting {
        current_tool,
        remaining_tools,
        completed_results,
        ..
    } = next.new_state
    else {
        panic!("the slow sibling still owns the foreground barrier")
    };
    assert_eq!(current_tool.id, "slow");
    assert!(remaining_tools.is_empty());
    assert_eq!(completed_results, vec![result.into()]);
    assert!(!next
        .effects
        .iter()
        .any(|effect| matches!(effect, Effect::RequestLlm)));
}

#[test]
fn child_cancellation_waits_for_every_ordinary_sibling_before_notifying_parent() {
    let mut context = context();
    context.is_sub_agent = true;
    let cancelling = transition(
        &batch(),
        &context,
        Event::UserCancel {
            reason: None,
            cause: crate::event::CancelCause::UserRequested,
        },
    )
    .unwrap();
    let partial = transition(
        &cancelling.new_state,
        &context,
        Event::ToolAborted {
            tool_use_id: "slow".into(),
        },
    )
    .unwrap();
    assert!(
        matches!(partial.new_state, ConvState::CancellingTool { .. }),
        "one sibling must not terminate the child while another is unresolved"
    );
    assert!(!partial
        .effects
        .iter()
        .any(|effect| matches!(effect, Effect::NotifyParent { .. })));
}
