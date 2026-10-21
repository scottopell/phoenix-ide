use super::super::{
    finish_workflow_transaction_at_cut, local_codec_owned, next_available_transition_id_tx, to_i64,
    to_u64, ConvState, DbError, DbResult, Generation, Row, Timestamp, TransactionCut,
    TurnAuthorityId, WorkflowRepository, WorkflowStatus,
};
use chrono::{DateTime, Utc};
use phoenix_core::domain::conversation_goal::{GoalExecution, GoalRequestOperation, GoalText};
use phoenix_workflow::{direct_turn_profile, TransitionId};

#[derive(Debug, Clone)]
pub struct GoalStartRecoveryInput {
    transcript: String,
    turn: (TurnAuthorityId, u64),
    slot: (ConvState, DateTime<Utc>),
    reconstruction_id: uuid::Uuid,
    now: Timestamp,
}

impl GoalStartRecoveryInput {
    #[must_use]
    pub fn new(
        transcript: String,
        turn: (TurnAuthorityId, u64),
        slot: (ConvState, DateTime<Utc>),
        now: Timestamp,
    ) -> Self {
        Self {
            transcript,
            turn,
            slot,
            reconstruction_id: uuid::Uuid::new_v4(),
            now,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct RecoveredGoalStartRuntime {
    turn_id: TurnAuthorityId,
    generation: u64,
}

impl RecoveredGoalStartRuntime {
    #[must_use]
    pub const fn turn_id(&self) -> TurnAuthorityId {
        self.turn_id
    }

    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum GoalStartRecoveryOutcome {
    Recovered(RecoveredGoalStartRuntime),
    Refused,
}

impl WorkflowRepository {
    /// Reconstruct an adopted Start at the serialized runtime-creation boundary.
    ///
    /// # Errors
    /// Storage, decode and unclassified commit errors deny reconstruction.
    pub async fn recover_goal_start_runtime(
        &self,
        input: &GoalStartRecoveryInput,
    ) -> DbResult<GoalStartRecoveryOutcome> {
        self.recover_goal_start_runtime_at_cut(input, TransactionCut::None)
            .await
    }

    #[cfg(test)]
    pub(crate) async fn recover_goal_start_runtime_test_cut(
        &self,
        input: &GoalStartRecoveryInput,
        after_commit: bool,
    ) -> DbResult<GoalStartRecoveryOutcome> {
        self.recover_goal_start_runtime_at_cut(
            input,
            if after_commit {
                TransactionCut::AfterCommit
            } else {
                TransactionCut::BeforeCommit
            },
        )
        .await
    }

    #[allow(clippy::too_many_lines)]
    async fn recover_goal_start_runtime_at_cut(
        &self,
        input: &GoalStartRecoveryInput,
        cut: TransactionCut,
    ) -> DbResult<GoalStartRecoveryOutcome> {
        use crate::conversation_goals::{control::GoalControlInstallation, execution};
        if self.goal_execution_installation == execution::GoalExecutionInstallation::Disabled
            || self.goal_control_installation == GoalControlInstallation::Disabled
        {
            return Ok(GoalStartRecoveryOutcome::Refused);
        }
        let mut tx = self.begin_immediate_tx().await?;
        let row = sqlx::query(
            "SELECT t.workflow_id,t.generation,t.input_kind,t.materialized,t.owns_conversation,
                    t.terminal_kind,c.state,c.state_updated_at
             FROM durable_turns t JOIN conversations c ON c.id=t.conversation_id
             WHERE t.turn_id=? AND t.conversation_id=?
               AND NOT EXISTS (SELECT 1 FROM direct_turn_terminal_obligations o WHERE o.turn_id=t.turn_id)
               AND NOT EXISTS (SELECT 1 FROM workflow_attempts a WHERE a.workflow_id=t.workflow_id
                   AND a.status IN ('Begun','ObservationRecorded'))",
        )
        .bind(to_i64(input.turn.0.0, "turn_id")?)
        .bind(&input.transcript)
        .fetch_optional(&mut *tx.tx)
        .await?;
        let Some(row) = row else {
            return Ok(GoalStartRecoveryOutcome::Refused);
        };
        if row.get::<String, _>("input_kind") != "goal_start"
            || row.get::<i64, _>("materialized") != 1
            || row.get::<i64, _>("owns_conversation") != 1
            || row.get::<Option<String>, _>("terminal_kind").is_some()
            || row.get::<i64, _>("generation") != to_i64(input.turn.1, "generation")?
            || row.get::<String, _>("state_updated_at") != input.slot.1.to_rfc3339()
        {
            return Ok(GoalStartRecoveryOutcome::Refused);
        }
        let state: ConvState = serde_json::from_str(&row.get::<String, _>("state"))
            .map_err(|error| DbError::Serialization(error.to_string()))?;
        if state != input.slot.0 {
            return Ok(GoalStartRecoveryOutcome::Refused);
        }
        let operation = if let ConvState::LlmRequesting { attempt } = &state {
            GoalRequestOperation::Normal { attempt: *attempt }
        } else if let ConvState::AwaitingContinuation { request } = &state {
            GoalRequestOperation::Summary {
                operation: GoalText::parse(request.operation_id.clone())
                    .map_err(|error| DbError::Serialization(error.to_string()))?,
            }
        } else {
            return Ok(GoalStartRecoveryOutcome::Refused);
        };
        let execution =
            GoalExecution::parse(row.get("workflow_id"), to_i64(input.turn.0 .0, "turn_id")?)
                .map_err(|error| DbError::Serialization(error.to_string()))?;
        let latest = execution::latest_request_tx(&mut tx.tx, execution)
            .await
            .map_err(|error| DbError::Serialization(error.to_string()))?
            .ok_or_else(|| DbError::Serialization("Start recovery lost admitted request".into()))?;
        if latest.checkpoint.is_some() {
            return Ok(GoalStartRecoveryOutcome::Refused);
        }
        match execution::require_live_request_tx(&mut tx.tx, &latest.intent, &input.transcript)
            .await
        {
            Ok(()) => {}
            Err(error) if recovery_refusal(&error) => return Ok(GoalStartRecoveryOutcome::Refused),
            Err(error) => return Err(DbError::Serialization(error.to_string())),
        }
        let context = self
            .admit_goal_start_request_context_tx(
                &mut tx,
                &input.transcript,
                input.turn.0,
                input.turn.1,
                input.slot.1,
                operation,
            )
            .await?
            .ok_or_else(|| DbError::Serialization("Start recovery lost its context".into()))?;
        if context.request_id() != &latest.intent.id {
            return Err(DbError::Serialization(
                "Start recovery changed admitted intent".into(),
            ));
        }
        let workflow_id =
            phoenix_workflow::WorkflowId(to_u64(execution.workflow_id(), "workflow_id")?);
        let head = tx
            .fetch_workflow_head(workflow_id)
            .await?
            .ok_or_else(|| DbError::Serialization("Start recovery lost workflow".into()))?;
        let snapshot = serde_json::to_vec(&direct_turn_profile::DirectTurnSnapshot {
            turn_id: input.turn.0 .0,
        })
        .map_err(|error| DbError::Serialization(error.to_string()))?;
        let snapshot_codec = local_codec_owned(&direct_turn_profile::snapshot_codec());
        if head.status != WorkflowStatus::Active
            || head.generation.0 != input.turn.1
            || head.binding.profile != direct_turn_profile::profile()
            || head.snapshot_codec != direct_turn_profile::snapshot_codec()
            || head.snapshot_payload != snapshot
        {
            return Ok(GoalStartRecoveryOutcome::Refused);
        }
        let next =
            input.turn.1.checked_add(1).ok_or_else(|| {
                DbError::Serialization("Start recovery generation overflow".into())
            })?;
        let event = direct_turn_profile::DirectTurnEvent::RuntimeReconstructed {
            turn_id: input.turn.0 .0,
            previous_generation: input.turn.1,
            generation: next,
            reconstruction_id: input.reconstruction_id.to_string(),
            request_id: latest.intent.id.to_string(),
            state_updated_at: input.slot.1.to_rfc3339(),
        };
        let payload = serde_json::to_vec(&event)
            .map_err(|error| DbError::Serialization(error.to_string()))?;
        let event_codec = local_codec_owned(&direct_turn_profile::event_codec());
        let transition_id =
            TransitionId(next_available_transition_id_tx(&mut tx.tx, workflow_id, 5).await?);
        if !tx
            .commit_transition_head_cas(
                workflow_id,
                head.version,
                Generation(next),
                WorkflowStatus::Active,
                &event_codec,
                &payload,
                &snapshot_codec,
                &snapshot,
                transition_id,
                input.now,
            )
            .await?
        {
            return Ok(GoalStartRecoveryOutcome::Refused);
        }
        let changed = sqlx::query(
            "UPDATE durable_turns SET generation=?
             WHERE turn_id=? AND generation=? AND input_kind='goal_start' AND materialized=1
               AND owns_conversation=1 AND terminal_kind IS NULL",
        )
        .bind(to_i64(next, "generation")?)
        .bind(to_i64(input.turn.0 .0, "turn_id")?)
        .bind(to_i64(input.turn.1, "generation")?)
        .execute(&mut *tx.tx)
        .await?
        .rows_affected();
        if changed != 1 {
            return Err(DbError::Serialization(
                "Start recovery turn CAS rejected".into(),
            ));
        }
        if let Err(error) = finish_workflow_transaction_at_cut(tx, cut).await {
            if !self
                .probe_goal_start_runtime_recovery(
                    input,
                    workflow_id,
                    head.version.next(),
                    &payload,
                    &snapshot,
                    &latest.intent,
                )
                .await?
            {
                return Err(error);
            }
        }
        Ok(GoalStartRecoveryOutcome::Recovered(
            RecoveredGoalStartRuntime {
                turn_id: input.turn.0,
                generation: next,
            },
        ))
    }

    async fn probe_goal_start_runtime_recovery(
        &self,
        input: &GoalStartRecoveryInput,
        workflow_id: phoenix_workflow::WorkflowId,
        version: phoenix_workflow::Version,
        payload: &[u8],
        snapshot: &[u8],
        intent: &phoenix_core::domain::conversation_goal::GoalRequestIntent,
    ) -> DbResult<bool> {
        let mut tx = self.begin_immediate_tx().await?;
        let exact: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM durable_turns t
             JOIN workflows w ON w.workflow_id=t.workflow_id
             JOIN workflow_transitions r ON r.workflow_id=w.workflow_id AND r.to_version=w.version
             JOIN conversations c ON c.id=t.conversation_id
             WHERE t.turn_id=? AND t.workflow_id=? AND t.conversation_id=?
               AND t.generation=? AND t.generation=w.generation AND r.generation=w.generation
               AND t.input_kind='goal_start' AND t.materialized=1 AND t.owns_conversation=1
               AND t.terminal_kind IS NULL AND w.status='Active' AND w.version=?
               AND w.profile_kind='direct_turn' AND w.profile_version=1
               AND w.snapshot_codec_family='direct_turn.snapshot' AND w.snapshot_codec_version=1
               AND r.event_codec_family='direct_turn.event' AND r.event_codec_version=1
               AND r.event_payload=? AND w.snapshot_payload=?
               AND c.state=? AND c.state_updated_at=?
               AND NOT EXISTS (SELECT 1 FROM direct_turn_terminal_obligations o WHERE o.turn_id=t.turn_id)
               AND NOT EXISTS (SELECT 1 FROM workflow_attempts a WHERE a.workflow_id=t.workflow_id
                   AND a.status IN ('Begun','ObservationRecorded')))",
        )
        .bind(to_i64(input.turn.0.0, "turn_id")?)
        .bind(to_i64(workflow_id.0, "workflow_id")?)
        .bind(&input.transcript)
        .bind(to_i64(input.turn.1 + 1, "generation")?)
        .bind(to_i64(version.0, "version")?)
        .bind(payload).bind(snapshot)
        .bind(serde_json::to_string(&input.slot.0).map_err(|error| DbError::Serialization(error.to_string()))?)
        .bind(input.slot.1.to_rfc3339())
        .fetch_one(&mut *tx.tx).await?;
        if exact {
            let execution = GoalExecution::parse(
                to_i64(workflow_id.0, "workflow_id")?,
                to_i64(input.turn.0 .0, "turn_id")?,
            )
            .map_err(|error| DbError::Serialization(error.to_string()))?;
            let latest =
                crate::conversation_goals::execution::latest_request_tx(&mut tx.tx, execution)
                    .await
                    .map_err(|error| DbError::Serialization(error.to_string()))?
                    .ok_or_else(|| {
                        DbError::Serialization("Start recovery probe lost request".into())
                    })?;
            if latest.checkpoint.is_some() || &latest.intent != intent {
                return Ok(false);
            }
            match crate::conversation_goals::execution::require_live_request_tx(
                &mut tx.tx,
                &latest.intent,
                &input.transcript,
            )
            .await
            {
                Ok(()) => {}
                Err(error) if recovery_refusal(&error) => return Ok(false),
                Err(error) => return Err(DbError::Serialization(error.to_string())),
            }
        }
        Ok(exact)
    }
}

fn recovery_refusal(error: &crate::conversation_goals::execution::GoalRequestError) -> bool {
    use crate::conversation_goals::{
        control::ControlError, execution::GoalRequestError, GoalDbError,
    };
    matches!(
        error,
        GoalRequestError::Disabled
            | GoalRequestError::Ineligible
            | GoalRequestError::Exhausted
            | GoalRequestError::Revoked
            | GoalRequestError::QueuedInput
            | GoalRequestError::Detached
            | GoalRequestError::Goal(
                GoalDbError::TargetChanged
                    | GoalDbError::Control(ControlError::AdmissionStopped)
                    | GoalDbError::Admission(
                        DbError::CloseAdmissionFenced(_)
                            | DbError::ProductConversationUnavailable(_)
                    )
            )
    )
}
