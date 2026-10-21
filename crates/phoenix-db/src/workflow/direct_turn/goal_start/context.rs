use super::super::{
    to_i64, ConvState, DbError, DbResult, Row, TurnAuthorityId, WorkflowRepository,
};
use phoenix_core::domain::conversation_goal::{
    GoalExecution, GoalRequestCause, GoalRequestId, GoalRequestIntent, GoalRequestOperation,
    GoalRequestPurpose, GoalSnapshot,
};

use std::fmt::Write as _;

use sha2::{Digest, Sha256};

#[derive(Debug, Clone)]
pub struct GoalStartRequestContext {
    pub request_id: GoalRequestId,
    pub purpose: GoalRequestPurpose,
    pub observation: GoalSnapshot,
}

impl GoalStartRequestContext {
    #[must_use]
    pub fn system_context(&self) -> String {
        format!("\n\nAdmitted durable goal (request-local context, not a conversation message):\nOwner: {}\nGoal: {}\nObserved version: {}\nState: underway\nObjective:\n{}\nRequest purpose: {:?}\n",
            self.observation.owner.as_str(),self.observation.goal_id.as_str(),
            self.observation.version.get(),self.observation.objective.as_str(),self.purpose)
    }
}

impl WorkflowRepository {
    /// Admit or restore the actual Start request operation and its prompt projection.
    ///
    /// # Errors
    /// Rejects stale authority, changed live fences/state, causal conflicts,
    /// exhausted capacity and storage failures.
    pub async fn admit_goal_start_request_context(
        &self,
        transcript: &str,
        active: Option<(TurnAuthorityId, u64)>,
        operation: GoalRequestOperation,
    ) -> DbResult<Option<GoalStartRequestContext>> {
        if self.goal_execution_installation
            == crate::conversation_goals::execution::GoalExecutionInstallation::Disabled
        {
            return Ok(None);
        }
        let Some((turn_id, generation)) = active else {
            let start: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM durable_turns WHERE conversation_id=?
                 AND input_kind='goal_start' AND owns_conversation=1 AND terminal_kind IS NULL)",
            )
            .bind(transcript)
            .fetch_one(&self.pool)
            .await?;
            if start {
                return Err(DbError::Serialization(
                    "Start request has no active runtime execution".into(),
                ));
            }
            return Ok(None);
        };
        let mut tx = self.begin_immediate_tx().await?;
        let row=sqlx::query("SELECT t.*,c.state,c.state_updated_at FROM durable_turns t JOIN conversations c ON c.id=t.conversation_id WHERE t.turn_id=? AND t.conversation_id=?")
            .bind(to_i64(turn_id.0,"turn_id")?).bind(transcript).fetch_one(&mut *tx.tx).await?;
        if row.get::<String, _>("input_kind") != "goal_start" {
            return Ok(None);
        }
        if self.goal_control_installation
            == crate::conversation_goals::control::GoalControlInstallation::Disabled
            || row.get::<i64, _>("materialized") != 1
            || row.get::<i64, _>("owns_conversation") != 1
            || row.get::<Option<String>, _>("terminal_kind").is_some()
            || row.get::<i64, _>("generation") != to_i64(generation, "generation")?
        {
            return Err(DbError::Serialization(
                "Start request has no live materialized execution".into(),
            ));
        }
        let execution = GoalExecution::parse(row.get("workflow_id"), to_i64(turn_id.0, "turn_id")?)
            .map_err(|error| DbError::Serialization(error.to_string()))?;
        let retained = crate::conversation_goals::start::retained_start_for_execution_tx(
            &mut tx.tx, execution,
        )
        .await
        .map_err(|error| DbError::Serialization(error.to_string()))?
        .ok_or_else(|| DbError::Serialization("Start request lost canonical acceptance".into()))?;
        let witness=sqlx::query("SELECT m.request_id,m.state_updated_at,r.generation AS request_generation FROM conversation_goal_start_materializations m JOIN conversation_goal_requests r ON r.request_id=m.request_id WHERE m.start_id=?")
            .bind(row.get::<i64,_>("goal_start_id")).fetch_one(&mut *tx.tx).await?;
        let intent = GoalRequestIntent::new(
            GoalRequestId::parse(format!("goal-start:{}", row.get::<i64, _>("goal_start_id")))
                .map_err(|error| DbError::Serialization(error.to_string()))?,
            retained.input().goal().clone(),
            execution,
            witness.get("request_generation"),
            GoalRequestCause::Initial,
        )
        .map_err(|error| DbError::Serialization(error.to_string()))?;
        if witness.get::<String, _>("request_id") != intent.id.as_str() {
            return Err(DbError::Serialization(
                "Start request witness identity differs".into(),
            ));
        }
        crate::conversation_goals::execution::retained_request_tx(
            &mut tx.tx,
            self.goal_execution_installation,
            &intent,
        )
        .await
        .map_err(|error| DbError::Serialization(error.to_string()))?
        .ok_or_else(|| DbError::Serialization("Start request was not durably charged".into()))?;
        crate::conversation_goals::execution::require_live_request_tx(
            &mut tx.tx, &intent, transcript,
        )
        .await
        .map_err(|error| DbError::Serialization(error.to_string()))?;
        let state: ConvState = serde_json::from_str(&row.get::<String, _>("state"))
            .map_err(|error| DbError::Serialization(error.to_string()))?;
        let initial_timestamp = row.get::<String, _>("state_updated_at")
            == witness.get::<String, _>("state_updated_at");
        let latest = crate::conversation_goals::execution::latest_request_tx(&mut tx.tx, execution)
            .await
            .map_err(|error| DbError::Serialization(error.to_string()))?
            .ok_or_else(|| DbError::Serialization("Start request lost its latest intent".into()))?;
        let new_intent = latest.checkpoint.is_some();
        let selected = select_intent(&intent, latest, generation, &operation)?;
        let permitted = match (&operation, &selected.cause) {
            (GoalRequestOperation::Normal { attempt }, GoalRequestCause::Initial) => {
                permits_initial_request(&state, &selected.id, initial_timestamp)
                    && permits_normal_request(&state, &selected.id, *attempt, false)
            }
            (GoalRequestOperation::Normal { attempt }, _) => {
                (!new_intent || !initial_timestamp)
                    && permits_normal_request(&state, &selected.id, *attempt, new_intent)
            }
            (GoalRequestOperation::Summary { operation }, _) => {
                permits_summary_request(&state, &selected.id, operation.as_str())
            }
        };
        if !permitted {
            return Err(DbError::Serialization(
                "Start request state advanced without a new admitted request".into(),
            ));
        }
        let admitted = crate::conversation_goals::execution::append_request_tx(
            &mut tx.tx,
            self.goal_execution_installation,
            &selected,
        )
        .await
        .map_err(|error| DbError::Serialization(error.to_string()))?;
        if admitted.checkpoint.is_some() {
            return Err(DbError::Serialization(
                "Start request operation already checkpointed".into(),
            ));
        }
        tx.commit().await?;
        Ok(Some(GoalStartRequestContext {
            purpose: selected.cause.purpose(),
            request_id: selected.id,
            observation: retained.observation().clone(),
        }))
    }
}

fn select_intent(
    initial: &GoalRequestIntent,
    latest: crate::conversation_goals::execution::GoalRequestAdmission,
    generation: u64,
    operation: &GoalRequestOperation,
) -> DbResult<GoalRequestIntent> {
    if latest.intent.goal != initial.goal
        || latest.intent.execution != initial.execution
        || latest.live_turn_id != Some(initial.execution.turn_id())
    {
        return Err(DbError::Serialization(
            "Start latest request identity differs".into(),
        ));
    }
    let cause = match operation {
        GoalRequestOperation::Normal { .. } if latest.checkpoint.is_none() => {
            if latest.intent.cause.purpose() != GoalRequestPurpose::Normal {
                return Err(DbError::Serialization(
                    "Start pending Summary cannot authorize Normal".into(),
                ));
            }
            return Ok(latest.intent);
        }
        GoalRequestOperation::Normal { .. } => GoalRequestCause::AfterCheckpoint {
            predecessor: latest.intent.id,
        },
        GoalRequestOperation::Summary { operation } => {
            if let GoalRequestCause::Summary { operation: pending } = &latest.intent.cause {
                if pending == operation {
                    if latest.checkpoint.is_some() {
                        return Err(DbError::Serialization(
                            "Start Summary operation already checkpointed".into(),
                        ));
                    }
                    return Ok(latest.intent);
                }
                if latest.checkpoint.is_none() {
                    return Err(DbError::Serialization(
                        "Start has a different pending Summary operation".into(),
                    ));
                }
            }
            GoalRequestCause::Summary {
                operation: operation.clone(),
            }
        }
    };
    let identity = serde_json::to_vec(&(
        "goal-request-v1",
        initial.execution.workflow_id(),
        initial.execution.turn_id(),
        &cause,
    ))
    .map_err(|error| DbError::Serialization(error.to_string()))?;
    let mut digest = String::with_capacity(64);
    for byte in Sha256::digest(identity) {
        write!(&mut digest, "{byte:02x}").expect("writing to String cannot fail");
    }
    GoalRequestIntent::new(
        GoalRequestId::parse(format!("goal-request:{digest}"))
            .map_err(|error| DbError::Serialization(error.to_string()))?,
        initial.goal.clone(),
        initial.execution,
        to_i64(generation, "generation")?,
        cause,
    )
    .map_err(|error| DbError::Serialization(error.to_string()))
}

fn permits_normal_request(
    state: &ConvState,
    request_id: &GoalRequestId,
    requested_attempt: u32,
    new_intent: bool,
) -> bool {
    match state {
        ConvState::LlmRequesting { attempt } => {
            *attempt >= 1 && *attempt == requested_attempt && (!new_intent || *attempt == 1)
        }
        ConvState::ServerOverloadRetrying { retry } => {
            !new_intent
                && retry.attempt == requested_attempt
                && retry.logical_request_id == request_id.as_str()
                && retry.target == phoenix_core::domain::sm_state::ServerOverloadTarget::Ordinary
                && retry.phase == phoenix_core::domain::sm_state::ServerOverloadPhase::InFlight
        }
        ConvState::Idle
        | ConvState::SeededLlmRequesting { .. }
        | ConvState::Provisioning { .. }
        | ConvState::CreationCancelled { .. }
        | ConvState::Completed { .. }
        | ConvState::Failed { .. }
        | ConvState::CreationFailed { .. }
        | ConvState::AwaitingRecovery { .. }
        | ConvState::AwaitingContinuation { .. }
        | ConvState::RecoverableContinuationFailure { .. }
        | ConvState::ToolExecuting { .. }
        | ConvState::CancellingTool { .. }
        | ConvState::CancellingSubAgents { .. }
        | ConvState::AwaitingSubAgents { .. }
        | ConvState::Error { .. }
        | ConvState::Terminal
        | ConvState::AwaitingTaskApproval { .. }
        | ConvState::AwaitingUserResponse { .. }
        | ConvState::ContextExhausted { .. }
        | ConvState::HandedOff { .. } => false,
    }
}

fn permits_summary_request(state: &ConvState, request_id: &GoalRequestId, operation: &str) -> bool {
    match state {
        ConvState::AwaitingContinuation { request } => request.operation_id == operation,
        ConvState::ServerOverloadRetrying { retry } => {
            retry.logical_request_id == request_id.as_str()
                && retry.phase == phoenix_core::domain::sm_state::ServerOverloadPhase::InFlight
                && matches!(&retry.target,
                    phoenix_core::domain::sm_state::ServerOverloadTarget::Continuation { operation_id, .. }
                    if operation_id == operation)
        }
        ConvState::Idle
        | ConvState::SeededLlmRequesting { .. }
        | ConvState::Provisioning { .. }
        | ConvState::CreationCancelled { .. }
        | ConvState::Completed { .. }
        | ConvState::Failed { .. }
        | ConvState::CreationFailed { .. }
        | ConvState::AwaitingRecovery { .. }
        | ConvState::RecoverableContinuationFailure { .. }
        | ConvState::LlmRequesting { .. }
        | ConvState::ToolExecuting { .. }
        | ConvState::CancellingTool { .. }
        | ConvState::CancellingSubAgents { .. }
        | ConvState::AwaitingSubAgents { .. }
        | ConvState::Error { .. }
        | ConvState::Terminal
        | ConvState::AwaitingTaskApproval { .. }
        | ConvState::AwaitingUserResponse { .. }
        | ConvState::ContextExhausted { .. }
        | ConvState::HandedOff { .. } => false,
    }
}

fn permits_initial_request(
    state: &ConvState,
    request_id: &GoalRequestId,
    initial_timestamp: bool,
) -> bool {
    match state {
        ConvState::LlmRequesting { attempt: 1 } => initial_timestamp,
        ConvState::LlmRequesting { attempt } if *attempt > 1 => true,
        ConvState::ServerOverloadRetrying { retry } => {
            retry.logical_request_id == request_id.as_str()
                && retry.target == phoenix_core::domain::sm_state::ServerOverloadTarget::Ordinary
                && retry.phase == phoenix_core::domain::sm_state::ServerOverloadPhase::InFlight
        }
        ConvState::Idle
        | ConvState::SeededLlmRequesting { .. }
        | ConvState::Provisioning { .. }
        | ConvState::CreationCancelled { .. }
        | ConvState::Completed { .. }
        | ConvState::Failed { .. }
        | ConvState::CreationFailed { .. }
        | ConvState::AwaitingRecovery { .. }
        | ConvState::AwaitingContinuation { .. }
        | ConvState::RecoverableContinuationFailure { .. }
        | ConvState::LlmRequesting { .. }
        | ConvState::ToolExecuting { .. }
        | ConvState::CancellingTool { .. }
        | ConvState::CancellingSubAgents { .. }
        | ConvState::AwaitingSubAgents { .. }
        | ConvState::Error { .. }
        | ConvState::Terminal
        | ConvState::AwaitingTaskApproval { .. }
        | ConvState::AwaitingUserResponse { .. }
        | ConvState::ContextExhausted { .. }
        | ConvState::HandedOff { .. } => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use phoenix_core::domain::conversation_goal::{GoalFence, GoalId, GoalText, GoalVersion};

    #[test]
    fn conversation_goal_request_identity_is_bounded_and_delimiter_unambiguous() {
        let initial = GoalRequestIntent::new(
            GoalRequestId::parse("initial:request").unwrap(),
            GoalFence {
                owner: phoenix_core::domain::product_conversation::ProductConversationId::new(),
                goal_id: GoalId::parse("goal:with:delimiters").unwrap(),
                version: GoalVersion::parse(1).unwrap(),
            },
            GoalExecution::parse(12, 34).unwrap(),
            1,
            GoalRequestCause::Initial,
        )
        .unwrap();
        let retained = crate::conversation_goals::execution::GoalRequestAdmission {
            intent: initial.clone(),
            ordinal: 1,
            live_turn_id: Some(34),
            checkpoint: None,
        };
        let left = select_intent(
            &initial,
            retained.clone(),
            1,
            &GoalRequestOperation::Summary {
                operation: GoalText::parse("a:\"b:c").unwrap(),
            },
        )
        .unwrap();
        let right = select_intent(
            &initial,
            retained.clone(),
            1,
            &GoalRequestOperation::Summary {
                operation: GoalText::parse("a:b:\"c").unwrap(),
            },
        )
        .unwrap();
        assert_ne!(left.id, right.id);
        assert_eq!(left.id.as_str().len(), "goal-request:".len() + 64);
        let mut predecessor = retained;
        for ordinal in 1..=100 {
            predecessor.checkpoint = Some(GoalText::parse("durable checkpoint").unwrap());
            let next = select_intent(
                &initial,
                predecessor,
                ordinal,
                &GoalRequestOperation::Normal { attempt: 1 },
            )
            .unwrap();
            assert_eq!(next.id.as_str().len(), "goal-request:".len() + 64);
            predecessor = crate::conversation_goals::execution::GoalRequestAdmission {
                intent: next,
                ordinal: i64::try_from(ordinal).unwrap(),
                live_turn_id: Some(34),
                checkpoint: None,
            };
        }
    }
}
