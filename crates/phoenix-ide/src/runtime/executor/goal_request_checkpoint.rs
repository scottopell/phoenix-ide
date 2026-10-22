use super::*;
use phoenix_db::{
    conversation_goals::{
        request_checkpoint::{
            GoalRequestCheckpoint, GoalRequestCheckpointOutcome, GoalRequestEvidence,
        },
        round::CommittedGoalRound,
    },
    workflow::AdmittedGoalRequest,
};

pub(super) struct LlmCompletion {
    pub generation: u64,
    pub outcome: LlmOutcome,
    pub admitted: Option<AdmittedGoalRequest>,
}

pub(super) struct PendingGoalRequestCheckpoint {
    pub admitted: AdmittedGoalRequest,
    pub source: ConvState,
    pub source_updated_at: DateTime<Utc>,
}

impl<S, L, T> ConversationRuntime<S, L, T>
where
    S: Storage + Clone + 'static,
    L: LlmClient + 'static,
    T: ToolExecutor + 'static,
{
    pub(super) async fn prepare_management_request(
        &self,
        service: &crate::goal_service::management::GoalManagementService,
        assistant_id: &str,
        tool_use: &str,
        request: phoenix_core::domain::conversation_goal::GoalManagementRequest,
    ) -> Result<phoenix_core::domain::conversation_goal::GoalPendingManagement, String> {
        let turn = self
            .active_direct_turn
            .as_deref()
            .ok_or("goal management has no active direct turn")?;
        service
            .prepare(
                &self.context.conversation_id,
                turn,
                assistant_id,
                tool_use,
                request,
            )
            .await
    }

    pub(super) async fn process_llm_completion(
        &mut self,
        completion: LlmCompletion,
    ) -> Result<(), String> {
        if self.llm_outcome_is_stale(completion.generation) {
            return Ok(());
        }
        if let Some(receipt) = completion.admitted {
            // A consumed task has no handle; a duplicate envelope cannot retire its successor.
            if self.llm_task_handle.is_none() || self.pending_goal_request_checkpoint.is_some() {
                return Ok(());
            }
            let current_slot = matches!(
                self.state,
                ConvState::LlmRequesting { .. } | ConvState::ServerOverloadRetrying { .. }
            ) && self.state_updated_at == receipt.state_updated_at();
            if !current_slot
                || !self
                    .storage
                    .goal_request_completion_is_current(
                        &receipt,
                        &self.context.conversation_id,
                        &self.state,
                        self.state_updated_at,
                    )
                    .await?
            {
                return Ok(());
            }
            if let LlmOutcome::Response {
                request_id,
                content,
                tool_calls,
                ..
            } = &completion.outcome
            {
                if request_id != receipt.intent().id.as_str()
                    || (content.is_empty() && tool_calls.is_empty())
                {
                    return Err("Goal response identity differs from its admitted request".into());
                }
                self.pending_goal_request_checkpoint = Some(PendingGoalRequestCheckpoint {
                    admitted: receipt,
                    source: self.state.clone(),
                    source_updated_at: self.state_updated_at,
                });
            }
        }
        self.llm_task_handle = None;
        self.active_llm_attempt = None;
        self.process_outcome(EffectOutcome::Llm(completion.outcome))
            .await
    }

    pub(super) fn is_goal_checkpoint_transition(
        &self,
        result: &crate::state_machine::transition::TransitionResult,
    ) -> bool {
        let Some(pending) = &self.pending_goal_request_checkpoint else {
            return false;
        };
        result.effects.iter().any(|effect| match effect {
            Effect::PersistMessage { message_id, .. } => {
                message_id == pending.admitted.intent().id.as_str()
            }
            Effect::PersistCheckpoint {
                data:
                    CheckpointData::ToolRound {
                        assistant_message, ..
                    },
            } => assistant_message.message_id == pending.admitted.intent().id.as_str(),
            _ => false,
        })
    }

    pub(super) async fn commit_admitted_goal_request(
        &mut self,
        assistant: &crate::db::Message,
        evidence: GoalRequestEvidence<'_>,
    ) -> Result<CommittedGoalRound, String> {
        let pending = self
            .pending_goal_request_checkpoint
            .as_ref()
            .ok_or("canonical checkpoint has no actual dispatch receipt")?;
        let logical_turn_boundary = matches!(evidence, GoalRequestEvidence::Response);
        let result = self
            .storage
            .commit_goal_request_checkpoint(&GoalRequestCheckpoint {
                admitted: &pending.admitted,
                source: &pending.source,
                source_updated_at: pending.source_updated_at,
                assistant,
                evidence,
                state: &self.state,
                state_updated_at: self.state_updated_at,
                provider_replay: self.pending_provider_replay_update.as_ref(),
            })
            .await;
        match result {
            Ok(GoalRequestCheckpointOutcome::Committed(committed)) => {
                self.install_live_state(
                    committed.state.clone(),
                    committed.state_updated_at,
                    false,
                )?;
                if matches!(committed.state, ConvState::Idle) {
                    self.classify_active_direct_turn_state_terminal(&committed.state);
                    self.direct_turn_terminal_fact = TerminalFactDurability::Durable;
                    let turn = self
                        .active_direct_turn
                        .as_deref()
                        .cloned()
                        .ok_or("committed Goal settlement lost its active terminal owner")?;
                    let terminal = self
                        .pending_direct_turn_terminal
                        .clone()
                        .ok_or("committed Goal settlement omitted terminal classification")?;
                    if let Err(error) = self.settle_pending_direct_turn(turn, terminal).await {
                        self.recovery_disposition =
                            RuntimeRecoveryDisposition::RecreateFromDatabase;
                        return Err(error);
                    }
                } else {
                    self.pending_direct_turn_terminal = None;
                }
                self.pending_goal_request_checkpoint = None;
                if logical_turn_boundary {
                    self.ordinary_turn_context = None;
                }
                self.pending_provider_replay_update = None;
                Ok(committed)
            }
            Ok(GoalRequestCheckpointOutcome::Refused) => {
                self.recovery_disposition = RuntimeRecoveryDisposition::RecreateFromDatabase;
                Err("canonical Goal response/checkpoint lost its owner".into())
            }
            Err(error) => {
                self.local_terminal_authority = LocalTerminalAuthority::Fatal;
                Err(error)
            }
        }
    }
}
