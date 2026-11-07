use super::*;
use phoenix_core::domain::conversation_goal::{SelfGoalOperation, ToolRoundResult};
use phoenix_core::domain::sm_state::AssistantMessage;

fn definitions() -> Vec<phoenix_llm::ToolDefinition> {
    vec![
        phoenix_llm::ToolDefinition {
            name: "create_goal".into(),
            description: "Create a Goal for this conversation only. Requires automatic continuation already enabled and no existing Goal of any disposition. Does not replace, edit or reactivate a Goal; does not start work or change this turn's admitted Goal. Call enable_auto_continuation separately first when needed.".into(),
            input_schema: serde_json::json!({"type":"object","properties":{"objective":{"type":"string","minLength":1}},"required":["objective"],"additionalProperties":false}),
            defer_loading: false,
        },
        phoenix_llm::ToolDefinition {
            name: "enable_auto_continuation".into(),
            description: "Enable automatic continuation for this conversation only. Does not create a Goal, Start, dispatch, retry or resume work. A human disable since this turn was admitted prevents this operation.".into(),
            input_schema: serde_json::json!({"type":"object","properties":{},"additionalProperties":false}),
            defer_loading: false,
        },
    ]
}

pub(super) fn add_definitions(tools: &mut Vec<phoenix_llm::ToolDefinition>, ordinary: bool) {
    tools.retain(|tool| {
        !matches!(
            tool.name.as_str(),
            "create_goal" | "enable_auto_continuation"
        )
    });
    if ordinary {
        tools.extend(definitions());
    }
}

fn apply_self_goal_round_commit(
    state: &mut ConvState,
    state_updated_at: &mut DateTime<Utc>,
    terminal_fact: &mut TerminalFactDurability,
    commit: crate::runtime::traits::SelfGoalRoundCommit,
    terminal_obligation_timestamp: Option<DateTime<Utc>>,
) -> (Vec<crate::db::Message>, bool) {
    match commit {
        crate::runtime::traits::SelfGoalRoundCommit::StateProjected(committed) => {
            *state = committed.state;
            *state_updated_at = committed.state_updated_at;
            (committed.results, committed.replayed)
        }
        crate::runtime::traits::SelfGoalRoundCommit::Terminal(committed) => {
            *terminal_fact = TerminalFactDurability::Durable {
                state_updated_at: terminal_obligation_timestamp
                    .expect("terminal commit requires obligation timestamp"),
            };
            (committed.results, committed.replayed)
        }
    }
}

impl<S: Storage + 'static, L: LlmClient + 'static, T: ToolExecutor + 'static>
    ConversationRuntime<S, L, T>
{
    pub(super) async fn prepare_self_goal(&self, tool: ToolCall) -> Result<Option<Event>, String> {
        let result = if self.context.is_coordinator || self.context.is_sub_agent {
            ToolResult::error(tool.id.clone(), "ordinary_role_required".into()).into()
        } else {
            let input = tool.input.to_value();
            match SelfGoalOperation::parse(tool.name(), input) {
                Err(error) => {
                    ToolResult::error(tool.id.clone(), format!("Invalid input: {error}")).into()
                }
                Ok(operation) => {
                    let turn = self
                        .active_direct_turn
                        .as_ref()
                        .ok_or("self-goal tool has no active turn authority")?;
                    let assistant_message = match &self.state {
                        ConvState::ToolExecuting {
                            assistant_message, ..
                        } => assistant_message,
                        _ => return Err("self-goal tool outside executing state".into()),
                    };
                    let pending = self
                        .storage
                        .prepare_self_goal_tool(
                            &self.context.conversation_id,
                            turn,
                            &assistant_message.message_id,
                            &tool.id,
                            operation,
                        )
                        .await?;
                    ToolRoundResult::PendingSelfGoal {
                        pending,
                        duration_ms: None,
                    }
                }
            }
        };
        Ok(Some(match result {
            ToolRoundResult::Finalized(result) => Event::ToolComplete {
                tool_use_id: tool.id,
                result,
            },
            pending => Event::ToolRoundComplete {
                tool_use_id: tool.id,
                result: pending,
            },
        }))
    }

    pub(super) async fn persist_self_goal_checkpoint(
        &mut self,
        assistant_message: AssistantMessage,
        tool_results: Vec<ToolRoundResult>,
        settlement: Option<&ActiveDirectTurnSettlement>,
        proposal: Option<&crate::db::ForkProposal>,
        admitted: &mut crate::runtime::AdmittedOperation,
    ) -> Result<Option<Event>, String> {
        let execution = tool_results
            .iter()
            .find_map(|result| match result {
                ToolRoundResult::PendingSelfGoal { pending, .. } => Some(pending.execution),
                _ => None,
            })
            .ok_or("missing self-goal pending effect")?;
        let new_trusted_results = trusted_tool_results(
            &assistant_message.message_id,
            tool_results.iter().filter_map(ToolRoundResult::finalized),
        );
        let (reserved_range, sequences) = self
            .broadcast_tx
            .persisted_message_reservation_authority()
            .await
            .reserve_next_range(1 + tool_results.len())
            .expect("executor owns persisted message reservation authority");
        let content = MessageContent::agent(assistant_message.content);
        let assistant = crate::db::Message {
            origin: phoenix_core::domain::db_schema::InputOrigin::SystemGenerated,
            message_id: assistant_message.message_id,
            conversation_id: self.context.conversation_id.clone(),
            sequence_id: sequences[0],
            message_type: content.message_type(),
            content,
            display_data: assistant_message.display_data,
            usage_data: assistant_message.usage,
            created_at: assistant_message.created_at,
        };
        let intents = tool_results;
        let checkpoint = phoenix_db::conversation_goals::round::GoalRoundCheckpoint {
            assistant: &assistant,
            execution,
            results: &intents,
            state: &self.state,
            state_updated_at: self.state_updated_at,
        };
        let committed = self
            .storage
            .persist_self_goal_round(
                &checkpoint,
                self.pending_provider_replay_update.as_ref(),
                settlement,
                proposal,
            )
            .await
            .map_err(|error| {
                self.recovery_disposition = RuntimeRecoveryDisposition::RecreateFromDatabase;
                error
            })?;
        self.pending_provider_replay_update = None;
        let (results, replayed) = apply_self_goal_round_commit(
            &mut self.state,
            &mut self.state_updated_at,
            &mut self.direct_turn_terminal_fact,
            committed,
            settlement.map(|settlement| settlement.state_updated_at),
        );
        merge_trusted_tool_results(&mut self.pending_trusted_tool_results, new_trusted_results);
        if replayed {
            return Ok(None);
        }
        let _ = self
            .broadcast_tx
            .admitted_publication(admitted)
            .persisted_message(assistant);
        for message in results {
            let _ = self
                .broadcast_tx
                .admitted_publication(admitted)
                .persisted_message(message);
        }
        drop(reserved_range);
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn terminal_commit_preserves_reducer_state_and_uses_obligation_timestamp() {
        let mut state = ConvState::Idle;
        let proposed_timestamp = Utc::now();
        let obligation_timestamp = proposed_timestamp + chrono::Duration::seconds(1);
        let mut state_updated_at = proposed_timestamp;
        let mut terminal_fact = TerminalFactDurability::ProcessOnly;

        let (_, replayed) = apply_self_goal_round_commit(
            &mut state,
            &mut state_updated_at,
            &mut terminal_fact,
            crate::runtime::traits::SelfGoalRoundCommit::Terminal(
                phoenix_db::conversation_goals::round::CommittedGoalTerminalRound {
                    results: Vec::new(),
                    replayed: false,
                },
            ),
            Some(obligation_timestamp),
        );

        assert!(!replayed);
        assert!(matches!(state, ConvState::Idle));
        assert_eq!(state_updated_at, proposed_timestamp);
        assert_eq!(
            terminal_fact,
            TerminalFactDurability::Durable {
                state_updated_at: obligation_timestamp,
            }
        );
    }

    #[test]
    fn discovery_is_ordinary_only_and_has_no_identity_arguments() {
        let mut tools = vec![];
        add_definitions(&mut tools, true);
        assert_eq!(tools.len(), 2);
        for tool in &tools {
            assert_eq!(tool.input_schema["additionalProperties"], false);
            for forbidden in ["owner", "conversation_id", "target", "version", "goal_id"] {
                assert!(tool.input_schema["properties"].get(forbidden).is_none());
            }
        }
        add_definitions(&mut tools, false);
        assert!(tools.is_empty());
    }
}
