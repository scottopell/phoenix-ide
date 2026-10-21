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

pub(super) struct GoalRequestCompletion {
    pub generation: u64,
    pub admitted: AdmittedGoalRequest,
    pub outcome: LlmOutcome,
}

pub(super) struct GoalResponseMessage {
    pub content: MessageContent,
    pub display_data: Option<serde_json::Value>,
    pub usage_data: Option<crate::db::UsageData>,
    pub message_id: String,
    pub origin: phoenix_core::domain::db_schema::InputOrigin,
}

pub(super) struct GoalRequestDispatchSlot {
    generation: u64,
    source: ConvState,
    request_id: phoenix_core::domain::conversation_goal::GoalRequestId,
    state_updated_at: DateTime<Utc>,
}

impl GoalRequestDispatchSlot {
    pub(super) fn spawned(
        generation: u64,
        admitted: &AdmittedGoalRequest,
        source: ConvState,
    ) -> Self {
        Self {
            generation,
            source,
            request_id: admitted.intent().id.clone(),
            state_updated_at: admitted.state_updated_at(),
        }
    }

    fn owns(&self, completion: &GoalRequestCompletion, source: &ConvState) -> bool {
        &self.source == source
            && self.generation == completion.generation
            && self.request_id == completion.admitted.intent().id
            && self.state_updated_at == completion.admitted.state_updated_at()
    }
}

pub(super) struct PendingGoalRequestCheckpoint {
    pub admitted: AdmittedGoalRequest,
    pub generation: u64,
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
        if let Some(pending) = &self.pending_goal_request_checkpoint {
            if pending.generation != self.llm_request_generation
                || pending.admitted.intent().id.as_str() != assistant_id
            {
                return Err("Goal management does not own the actual dispatch generation".into());
            }
            service
                .prepare_admitted_request(
                    &self.context.conversation_id,
                    &pending.admitted,
                    (&self.state, self.state_updated_at),
                    assistant_id,
                    tool_use,
                    request,
                )
                .await
        } else {
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
    }

    pub(super) async fn process_goal_request_completion(
        &mut self,
        completion: GoalRequestCompletion,
    ) -> Result<(), String> {
        if self.llm_outcome_is_stale(completion.generation)
            || !self
                .active_goal_request_dispatch
                .as_ref()
                .is_some_and(|slot| slot.owns(&completion, &self.state))
        {
            return Ok(());
        }
        let receipt = completion.admitted;
        let owned = self.active_direct_turn.as_deref().is_some_and(|active| {
            i64::try_from(active.turn_id.0).ok() == Some(receipt.intent().execution.turn_id())
                && i64::try_from(active.generation).ok() == Some(receipt.claim_generation())
        });
        if !owned || self.state_updated_at != receipt.state_updated_at() {
            return Err("Goal response lost its exact actual-dispatch owner/slot".into());
        }
        if !self
            .storage
            .goal_request_completion_is_current(
                &receipt,
                &self.context.conversation_id,
                &self.state,
                self.state_updated_at,
            )
            .await?
        {
            return Err("Goal provider completion is obsolete before response effects".into());
        }
        if let LlmOutcome::Response {
            request_id,
            content,
            tool_calls,
            ..
        } = &completion.outcome
        {
            if content.is_empty() && tool_calls.is_empty() {
                return Err("empty Start response has no canonical checkpoint payload".into());
            }
            if request_id != receipt.intent().id.as_str()
                || self.pending_goal_request_checkpoint.is_some()
            {
                return Err("Goal response does not own this checkpoint receipt".into());
            }
            self.pending_goal_request_checkpoint = Some(PendingGoalRequestCheckpoint {
                admitted: receipt,
                generation: completion.generation,
                source: self.state.clone(),
                source_updated_at: self.state_updated_at,
            });
        }
        self.active_goal_request_dispatch = None;
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

    pub(super) async fn persist_goal_response_message(
        &mut self,
        response: GoalResponseMessage,
        admitted: &mut crate::runtime::AdmittedOperation,
    ) -> Result<Option<Event>, String> {
        let GoalResponseMessage {
            content,
            display_data,
            usage_data,
            message_id,
            origin,
        } = response;
        let (reservation, sequences) = self
            .broadcast_tx
            .persisted_message_reservation_authority()
            .await
            .reserve_next_range(1)
            .expect("runtime owns canonical response reservation");
        let _reservation = reservation;
        let assistant = crate::db::Message {
            message_id,
            conversation_id: self.context.conversation_id.clone(),
            sequence_id: sequences[0],
            message_type: content.message_type(),
            content,
            display_data,
            usage_data,
            origin,
            created_at: Utc::now(),
        };
        let committed = self
            .commit_admitted_goal_request(&assistant, GoalRequestEvidence::Response)
            .await?;
        self.install_live_state(committed.state, committed.state_updated_at, false)?;
        self.publish_goal_request_rows(&assistant, &committed.results, admitted);
        Ok(None)
    }

    pub(super) async fn persist_admitted_goal_tool_round(
        &mut self,
        assistant: crate::db::Message,
        results: Vec<phoenix_core::domain::conversation_goal::ToolRoundResult>,
        admitted: &mut crate::runtime::AdmittedOperation,
    ) -> Result<Option<Event>, String> {
        if results.iter().any(|result| matches!(result,
            phoenix_core::domain::conversation_goal::ToolRoundResult::PendingGoalManagement { .. })) {
            return Box::pin(self.persist_pending_management_round(assistant, results, None, None, admitted)).await;
        }
        let trusted = results
            .iter()
            .filter_map(|result| result.finalized().cloned())
            .collect::<Vec<_>>();
        let trusted = trusted_tool_results(&assistant.message_id, &trusted);
        let committed = self
            .commit_admitted_goal_request(
                &assistant,
                GoalRequestEvidence::Round {
                    results: &results,
                    management_targets: &[],
                },
            )
            .await?;
        self.install_live_state(committed.state, committed.state_updated_at, false)?;
        merge_trusted_tool_results(&mut self.pending_trusted_tool_results, trusted);
        self.publish_goal_request_rows(&assistant, &committed.results, admitted);
        Ok(None)
    }

    async fn commit_admitted_goal_request(
        &mut self,
        assistant: &crate::db::Message,
        evidence: GoalRequestEvidence<'_>,
    ) -> Result<CommittedGoalRound, String> {
        let pending = self
            .pending_goal_request_checkpoint
            .as_ref()
            .ok_or("canonical checkpoint has no actual dispatch receipt")?;
        if pending.generation != self.llm_request_generation {
            self.local_terminal_authority = LocalTerminalAuthority::Fatal;
            return Err("admitted Goal checkpoint generation changed before commit".into());
        }
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
                self.pending_goal_request_checkpoint = None;
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

    fn publish_goal_request_rows(
        &self,
        assistant: &crate::db::Message,
        results: &[crate::db::Message],
        admitted: &mut crate::runtime::AdmittedOperation,
    ) {
        self.publish_live_state_admitted();
        let _ = self
            .broadcast_tx
            .admitted_publication(admitted)
            .persisted_message(assistant.clone());
        for result in results {
            let _ = self
                .broadcast_tx
                .admitted_publication(admitted)
                .persisted_message(result.clone());
        }
    }
}
