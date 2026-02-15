//! Conversation recovery logic
//!
//! Handles detection of interrupted conversations that need auto-continuation.

use crate::db::{Message, MessageContent, MessageType};
use crate::llm::ContentBlock;
use crate::state_machine::ConvState;

/// Result of analyzing messages for recovery
#[derive(Debug, Clone, PartialEq)]
pub struct RecoveryDecision {
    /// The state to resume with
    pub state: ConvState,
    /// Whether auto-continuation is needed
    pub needs_auto_continue: bool,
    /// Reason for the decision (for debugging)
    pub reason: RecoveryReason,
}

/// Why we made a particular recovery decision
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryReason {
    /// No messages in conversation
    EmptyConversation,
    /// Last message is not a tool result
    LastMessageNotTool,
    /// No agent message found in conversation
    NoAgentMessage,
    /// Last agent message contains text (normal completion)
    AgentHasTextResponse,
    /// Last agent message is `tool_use` only, needs continuation
    InterruptedMidTurn,
}

impl RecoveryDecision {
    fn idle(reason: RecoveryReason) -> Self {
        Self {
            state: ConvState::Idle,
            needs_auto_continue: false,
            reason,
        }
    }

    fn auto_continue() -> Self {
        Self {
            state: ConvState::LlmRequesting { attempt: 1 },
            needs_auto_continue: true,
            reason: RecoveryReason::InterruptedMidTurn,
        }
    }
}

/// Analyze messages to determine if a conversation needs auto-continuation.
///
/// A conversation needs auto-continuation when:
/// 1. The last message is a tool result
/// 2. The last agent message contains only `tool_use` blocks (no text)
///
/// This indicates the conversation was interrupted after tools completed
/// but before the LLM could provide a text response.
pub fn should_auto_continue(messages: &[Message]) -> RecoveryDecision {
    // Empty conversation -> idle
    if messages.is_empty() {
        return RecoveryDecision::idle(RecoveryReason::EmptyConversation);
    }

    // Last message must be a tool result
    let last_msg = messages.last().unwrap();
    if !matches!(last_msg.message_type, MessageType::Tool) {
        return RecoveryDecision::idle(RecoveryReason::LastMessageNotTool);
    }

    // Find the last agent message
    let last_agent = messages
        .iter()
        .rev()
        .find(|m| matches!(m.message_type, MessageType::Agent));

    let Some(last_agent) = last_agent else {
        return RecoveryDecision::idle(RecoveryReason::NoAgentMessage);
    };

    // Check if the agent message has any text content
    let agent_has_text = match &last_agent.content {
        MessageContent::Agent(blocks) => blocks
            .iter()
            .any(|b| matches!(b, ContentBlock::Text { .. })),
        // Non-agent content in agent message is unexpected, treat as having text (safe default)
        _ => true,
    };

    if agent_has_text {
        RecoveryDecision::idle(RecoveryReason::AgentHasTextResponse)
    } else {
        RecoveryDecision::auto_continue()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{ToolContent, UserContent};
    use chrono::Utc;
    use serde_json::json;

    // Helper to create a user message
    fn user_msg(seq: i64, text: &str) -> Message {
        Message {
            message_id: format!("user-{seq}"),
            conversation_id: "test-conv".to_string(),
            sequence_id: seq,
            message_type: MessageType::User,
            content: MessageContent::User(UserContent {
                text: text.to_string(),
                images: vec![],
            }),
            display_data: None,
            usage_data: None,
            created_at: Utc::now(),
        }
    }

    // Helper to create an agent message with only tool_use blocks
    fn agent_tool_use_only(seq: i64, tool_names: &[&str]) -> Message {
        let blocks: Vec<ContentBlock> = tool_names
            .iter()
            .enumerate()
            .map(|(i, name)| ContentBlock::ToolUse {
                id: format!("tool-{seq}-{i}"),
                name: (*name).to_string(),
                input: json!({}),
            })
            .collect();

        Message {
            message_id: format!("agent-{seq}"),
            conversation_id: "test-conv".to_string(),
            sequence_id: seq,
            message_type: MessageType::Agent,
            content: MessageContent::Agent(blocks),
            display_data: None,
            usage_data: None,
            created_at: Utc::now(),
        }
    }

    // Helper to create an agent message with text
    fn agent_with_text(seq: i64, text: &str) -> Message {
        Message {
            message_id: format!("agent-{seq}"),
            conversation_id: "test-conv".to_string(),
            sequence_id: seq,
            message_type: MessageType::Agent,
            content: MessageContent::Agent(vec![ContentBlock::Text {
                text: text.to_string(),
            }]),
            display_data: None,
            usage_data: None,
            created_at: Utc::now(),
        }
    }

    // Helper to create an agent message with text AND tool_use
    fn agent_with_text_and_tools(seq: i64, text: &str, tool_names: &[&str]) -> Message {
        let mut blocks = vec![ContentBlock::Text {
            text: text.to_string(),
        }];
        for (i, name) in tool_names.iter().enumerate() {
            blocks.push(ContentBlock::ToolUse {
                id: format!("tool-{seq}-{i}"),
                name: (*name).to_string(),
                input: json!({}),
            });
        }

        Message {
            message_id: format!("agent-{seq}"),
            conversation_id: "test-conv".to_string(),
            sequence_id: seq,
            message_type: MessageType::Agent,
            content: MessageContent::Agent(blocks),
            display_data: None,
            usage_data: None,
            created_at: Utc::now(),
        }
    }

    // Helper to create a tool result message
    fn tool_result(seq: i64, tool_use_id: &str, output: &str) -> Message {
        Message {
            message_id: format!("tool-{seq}"),
            conversation_id: "test-conv".to_string(),
            sequence_id: seq,
            message_type: MessageType::Tool,
            content: MessageContent::Tool(ToolContent {
                tool_use_id: tool_use_id.to_string(),
                content: output.to_string(),
                is_error: false,
            }),
            display_data: None,
            usage_data: None,
            created_at: Utc::now(),
        }
    }

    // =========================================================================
    // Basic cases
    // =========================================================================

    #[test]
    fn test_empty_conversation() {
        let decision = should_auto_continue(&[]);
        assert_eq!(decision.state, ConvState::Idle);
        assert!(!decision.needs_auto_continue);
        assert_eq!(decision.reason, RecoveryReason::EmptyConversation);
    }

    #[test]
    fn test_only_user_message() {
        let messages = vec![user_msg(1, "Hello")];
        let decision = should_auto_continue(&messages);
        assert_eq!(decision.state, ConvState::Idle);
        assert!(!decision.needs_auto_continue);
        assert_eq!(decision.reason, RecoveryReason::LastMessageNotTool);
    }

    #[test]
    fn test_user_then_agent_text() {
        // Normal completion: user asks, agent responds with text
        let messages = vec![user_msg(1, "Hello"), agent_with_text(2, "Hi there!")];
        let decision = should_auto_continue(&messages);
        assert_eq!(decision.state, ConvState::Idle);
        assert!(!decision.needs_auto_continue);
        assert_eq!(decision.reason, RecoveryReason::LastMessageNotTool);
    }

    // =========================================================================
    // Interrupted mid-turn cases (SHOULD auto-continue)
    // =========================================================================

    #[test]
    fn test_interrupted_single_tool() {
        // User -> Agent(tool_use) -> Tool result
        // Server crashed before LLM could respond to tool result
        let messages = vec![
            user_msg(1, "List files"),
            agent_tool_use_only(2, &["bash"]),
            tool_result(3, "tool-2-0", "file1.txt\nfile2.txt"),
        ];
        let decision = should_auto_continue(&messages);
        assert_eq!(decision.state, ConvState::LlmRequesting { attempt: 1 });
        assert!(decision.needs_auto_continue);
        assert_eq!(decision.reason, RecoveryReason::InterruptedMidTurn);
    }

    #[test]
    fn test_interrupted_multiple_tools() {
        // User -> Agent(multiple tool_use) -> Tool results
        let messages = vec![
            user_msg(1, "Check status"),
            agent_tool_use_only(2, &["bash", "bash"]),
            tool_result(3, "tool-2-0", "output1"),
            tool_result(4, "tool-2-1", "output2"),
        ];
        let decision = should_auto_continue(&messages);
        assert!(decision.needs_auto_continue);
        assert_eq!(decision.reason, RecoveryReason::InterruptedMidTurn);
    }

    #[test]
    fn test_interrupted_multi_turn_conversation() {
        // A longer conversation that was interrupted mid-turn
        let messages = vec![
            user_msg(1, "Hello"),
            agent_with_text(2, "Hi! How can I help?"),
            user_msg(3, "List files"),
            agent_tool_use_only(4, &["bash"]),
            tool_result(5, "tool-4-0", "file1.txt"),
        ];
        let decision = should_auto_continue(&messages);
        assert!(decision.needs_auto_continue);
        assert_eq!(decision.reason, RecoveryReason::InterruptedMidTurn);
    }

    // =========================================================================
    // Normal completion cases (should NOT auto-continue)
    // =========================================================================

    #[test]
    fn test_completed_tool_cycle() {
        // Full cycle: User -> Agent(tool) -> Tool -> Agent(text)
        let messages = vec![
            user_msg(1, "List files"),
            agent_tool_use_only(2, &["bash"]),
            tool_result(3, "tool-2-0", "file1.txt"),
            agent_with_text(4, "I found file1.txt"),
        ];
        let decision = should_auto_continue(&messages);
        assert_eq!(decision.state, ConvState::Idle);
        assert!(!decision.needs_auto_continue);
        // Last message is agent text, not tool
        assert_eq!(decision.reason, RecoveryReason::LastMessageNotTool);
    }

    #[test]
    fn test_agent_text_with_tools_completed() {
        // Agent responds with text AND tools, tools complete, agent responds
        let messages = vec![
            user_msg(1, "Help me"),
            agent_with_text_and_tools(2, "Let me check...", &["bash"]),
            tool_result(3, "tool-2-0", "done"),
            agent_with_text(4, "All done!"),
        ];
        let decision = should_auto_continue(&messages);
        assert!(!decision.needs_auto_continue);
    }

    #[test]
    fn test_tool_result_followed_by_user() {
        // User interrupted while agent was working
        let messages = vec![
            user_msg(1, "Do something"),
            agent_tool_use_only(2, &["bash"]),
            tool_result(3, "tool-2-0", "output"),
            user_msg(4, "Actually, cancel that"),
        ];
        let decision = should_auto_continue(&messages);
        assert!(!decision.needs_auto_continue);
        assert_eq!(decision.reason, RecoveryReason::LastMessageNotTool);
    }

    // =========================================================================
    // Edge cases
    // =========================================================================

    #[test]
    fn test_tool_result_no_agent_message() {
        // Weird state: tool result but no agent message
        // This shouldn't happen in practice but we handle it safely
        let messages = vec![
            user_msg(1, "Hello"),
            tool_result(2, "orphan-tool", "output"),
        ];
        let decision = should_auto_continue(&messages);
        assert!(!decision.needs_auto_continue);
        assert_eq!(decision.reason, RecoveryReason::NoAgentMessage);
    }

    #[test]
    fn test_agent_with_empty_blocks() {
        // Agent message with no content blocks at all
        let messages = vec![
            user_msg(1, "Hello"),
            Message {
                message_id: "agent-2".to_string(),
                conversation_id: "test-conv".to_string(),
                sequence_id: 2,
                message_type: MessageType::Agent,
                content: MessageContent::Agent(vec![]),
                display_data: None,
                usage_data: None,
                created_at: Utc::now(),
            },
            tool_result(3, "some-tool", "output"),
        ];
        let decision = should_auto_continue(&messages);
        // Empty blocks = no text, should auto-continue
        assert!(decision.needs_auto_continue);
    }

    #[test]
    fn test_multiple_agent_messages_last_has_no_text() {
        // Multiple agent messages, only looking at the LAST one
        let messages = vec![
            user_msg(1, "Hello"),
            agent_with_text(2, "Hi!"),
            user_msg(3, "Do task"),
            agent_tool_use_only(4, &["bash"]), // This is the last agent msg
            tool_result(5, "tool-4-0", "done"),
        ];
        let decision = should_auto_continue(&messages);
        assert!(decision.needs_auto_continue);
    }

    #[test]
    fn test_multiple_agent_messages_last_has_text() {
        // Multiple agent messages, last one has text
        let messages = vec![
            user_msg(1, "Hello"),
            agent_tool_use_only(2, &["bash"]),
            tool_result(3, "tool-2-0", "output"),
            agent_with_text(4, "Done!"), // This is the last agent msg
            user_msg(5, "Thanks"),
            agent_with_text(6, "You're welcome!"),
            tool_result(7, "orphan", "???"), // Orphan tool result
        ];
        let decision = should_auto_continue(&messages);
        // Last agent msg (6) has text, so even though last msg is tool, don't auto-continue
        assert!(!decision.needs_auto_continue);
        assert_eq!(decision.reason, RecoveryReason::AgentHasTextResponse);
    }

    #[test]
    fn test_text_before_tool_use_in_same_message() {
        // Agent message contains text BEFORE tool_use (common pattern)
        // "Let me check that for you" + tool_use
        let messages = vec![
            user_msg(1, "Check files"),
            agent_with_text_and_tools(2, "Let me check...", &["bash"]),
            tool_result(3, "tool-2-0", "files"),
        ];
        let decision = should_auto_continue(&messages);
        // The agent DID provide text, so this is normal - don't auto-continue
        // The agent will see the tool result and provide more text
        // Wait - actually this IS an interrupted case! The tool completed but LLM didn't respond.
        // Hmm, let me think about this...
        //
        // Actually NO - if the agent said "Let me check..." AND requested a tool,
        // and the tool completed, the agent should still respond with the result.
        // So this IS an interrupted case.
        //
        // But wait - the spec says we check if the last agent message has text.
        // It does ("Let me check..."). So we DON'T auto-continue.
        //
        // This is a design decision: if the agent already provided some text in
        // the same message as the tool_use, we consider that a "partial response"
        // and the agent can continue when the user sends a new message.
        //
        // This is safer than auto-continuing because the agent might have actually
        // finished their thought with "Let me check..." and then the user can
        // see the tool result and decide what to do.
        assert!(!decision.needs_auto_continue);
        assert_eq!(decision.reason, RecoveryReason::AgentHasTextResponse);
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use crate::db::{ToolContent, UserContent};
    use chrono::Utc;
    use proptest::prelude::*;
    use serde_json::json;

    // Strategy to generate a random message type
    fn message_type_strategy() -> impl Strategy<Value = MessageType> {
        prop_oneof![
            Just(MessageType::User),
            Just(MessageType::Agent),
            Just(MessageType::Tool),
        ]
    }

    // Strategy to generate content blocks for agent messages
    fn content_blocks_strategy() -> impl Strategy<Value = Vec<ContentBlock>> {
        prop_oneof![
            // Only tool_use
            (1..=3usize).prop_flat_map(|n| {
                proptest::collection::vec(
                    ("[a-z]{3,8}".prop_map(|s| s.to_string())).prop_map(|name| {
                        ContentBlock::ToolUse {
                            id: format!("tool-{}", name),
                            name,
                            input: json!({}),
                        }
                    }),
                    n,
                )
            }),
            // Only text
            "[a-zA-Z ]{1,50}".prop_map(|text| vec![ContentBlock::Text { text }]),
            // Text and tool_use
            ("[a-zA-Z ]{1,50}", "[a-z]{3,8}").prop_map(|(text, name)| {
                vec![
                    ContentBlock::Text { text },
                    ContentBlock::ToolUse {
                        id: format!("tool-{name}"),
                        name,
                        input: json!({}),
                    },
                ]
            }),
            // Empty (edge case)
            Just(vec![]),
        ]
    }

    // Helper to create messages for property tests
    fn make_message(seq: i64, msg_type: MessageType, has_text: bool) -> Message {
        let content = match msg_type {
            MessageType::User => MessageContent::User(UserContent {
                text: "user message".to_string(),
                images: vec![],
            }),
            MessageType::Agent => {
                if has_text {
                    MessageContent::Agent(vec![ContentBlock::Text {
                        text: "agent response".to_string(),
                    }])
                } else {
                    MessageContent::Agent(vec![ContentBlock::ToolUse {
                        id: format!("tool-{seq}"),
                        name: "bash".to_string(),
                        input: json!({}),
                    }])
                }
            }
            MessageType::Tool => MessageContent::Tool(ToolContent {
                tool_use_id: format!("tool-{}", seq - 1),
                content: "tool output".to_string(),
                is_error: false,
            }),
            _ => MessageContent::User(UserContent {
                text: "fallback".to_string(),
                images: vec![],
            }),
        };

        Message {
            message_id: format!("msg-{seq}"),
            conversation_id: "test".to_string(),
            sequence_id: seq,
            message_type: msg_type,
            content,
            display_data: None,
            usage_data: None,
            created_at: Utc::now(),
        }
    }

    // =========================================================================
    // Property: Result is always valid
    // =========================================================================

    proptest! {
        #[test]
        fn prop_always_returns_valid_state(msg_count in 0..20usize) {
            // Generate random sequence of messages
            let messages: Vec<Message> = (0..msg_count)
                .map(|i| {
                    let seq = i as i64 + 1;
                    let msg_type = match i % 3 {
                        0 => MessageType::User,
                        1 => MessageType::Agent,
                        _ => MessageType::Tool,
                    };
                    make_message(seq, msg_type, i % 2 == 0)
                })
                .collect();

            let decision = should_auto_continue(&messages);

            // State must be either Idle or LlmRequesting
            prop_assert!(
                matches!(decision.state, ConvState::Idle | ConvState::LlmRequesting { .. }),
                "Unexpected state: {:?}",
                decision.state
            );

            // If auto-continue, state must be LlmRequesting
            if decision.needs_auto_continue {
                prop_assert!(
                    matches!(decision.state, ConvState::LlmRequesting { .. }),
                    "Expected LlmRequesting when needs_auto_continue"
                );
            } else {
                prop_assert!(
                    matches!(decision.state, ConvState::Idle),
                    "Expected Idle when not needs_auto_continue"
                );
            }
        }
    }

    // =========================================================================
    // Property: Auto-continue requires tool as last message
    // =========================================================================

    proptest! {
        #[test]
        fn prop_auto_continue_requires_last_tool(
            prefix_len in 0..10usize,
            last_type in message_type_strategy()
        ) {
            // Build a message list with controlled last message
            let mut messages: Vec<Message> = (0..prefix_len)
                .map(|i| {
                    let seq = i as i64 + 1;
                    make_message(seq, MessageType::User, true)
                })
                .collect();

            // Add agent with tool_use
            if prefix_len > 0 {
                messages.push(make_message(prefix_len as i64 + 1, MessageType::Agent, false));
            }

            // Add last message with specified type
            let last_seq = messages.len() as i64 + 1;
            messages.push(make_message(last_seq, last_type.clone(), true));

            let decision = should_auto_continue(&messages);

            // If last message is NOT tool, cannot auto-continue
            if !matches!(last_type, MessageType::Tool) {
                prop_assert!(
                    !decision.needs_auto_continue,
                    "Auto-continued with last message type {:?}",
                    last_type
                );
            }
        }
    }

    // =========================================================================
    // Property: Auto-continue requires agent without text
    // =========================================================================

    proptest! {
        #[test]
        fn prop_auto_continue_requires_textless_agent(agent_has_text: bool) {
            let messages = vec![
                make_message(1, MessageType::User, true),
                make_message(2, MessageType::Agent, agent_has_text),
                make_message(3, MessageType::Tool, true),
            ];

            let decision = should_auto_continue(&messages);

            if agent_has_text {
                // Agent has text -> should NOT auto-continue
                prop_assert!(
                    !decision.needs_auto_continue,
                    "Auto-continued when agent had text"
                );
            } else {
                // Agent has no text -> SHOULD auto-continue
                prop_assert!(
                    decision.needs_auto_continue,
                    "Did not auto-continue when agent had no text"
                );
            }
        }
    }

    // =========================================================================
    // Property: Empty messages never auto-continues
    // =========================================================================

    #[test]
    fn prop_empty_never_auto_continues() {
        let decision = should_auto_continue(&[]);
        assert!(!decision.needs_auto_continue);
        assert_eq!(decision.reason, RecoveryReason::EmptyConversation);
    }

    // =========================================================================
    // Property: Reason matches behavior
    // =========================================================================

    proptest! {
        #[test]
        fn prop_reason_matches_behavior(msg_count in 1..10usize) {
            let messages: Vec<Message> = (0..msg_count)
                .map(|i| {
                    let seq = i as i64 + 1;
                    let msg_type = match i % 3 {
                        0 => MessageType::User,
                        1 => MessageType::Agent,
                        _ => MessageType::Tool,
                    };
                    make_message(seq, msg_type, i % 4 != 0) // Some agents have text, some don't
                })
                .collect();

            let decision = should_auto_continue(&messages);

            // Verify reason matches actual state
            match decision.reason {
                RecoveryReason::EmptyConversation => {
                    prop_assert!(messages.is_empty());
                }
                RecoveryReason::LastMessageNotTool => {
                    prop_assert!(!matches!(
                        messages.last().unwrap().message_type,
                        MessageType::Tool
                    ));
                }
                RecoveryReason::NoAgentMessage => {
                    prop_assert!(!messages
                        .iter()
                        .any(|m| matches!(m.message_type, MessageType::Agent)));
                }
                RecoveryReason::AgentHasTextResponse => {
                    // Last agent has text
                    let last_agent = messages
                        .iter()
                        .rev()
                        .find(|m| matches!(m.message_type, MessageType::Agent));
                    if let Some(agent) = last_agent {
                        if let MessageContent::Agent(blocks) = &agent.content {
                            let has_text = blocks
                                .iter()
                                .any(|b| matches!(b, ContentBlock::Text { .. }));
                            prop_assert!(has_text, "Agent should have text for AgentHasTextResponse");
                        }
                    }
                }
                RecoveryReason::InterruptedMidTurn => {
                    prop_assert!(decision.needs_auto_continue);
                }
            }
        }
    }
}
