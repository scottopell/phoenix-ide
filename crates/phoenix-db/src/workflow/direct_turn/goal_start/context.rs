use super::super::*;
use phoenix_core::domain::conversation_goal::{
    GoalExecution, GoalRequestCause, GoalRequestId, GoalRequestIntent, GoalRequestPurpose,
    GoalSnapshot,
};

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
    pub async fn goal_start_request_context(
        &self,
        transcript: &str,
        turn_id: TurnAuthorityId,
        generation: u64,
    ) -> DbResult<Option<GoalStartRequestContext>> {
        if self.goal_execution_installation
            == crate::conversation_goals::execution::GoalExecutionInstallation::Disabled
        {
            return Ok(None);
        }
        let mut tx = self.begin_tx().await?;
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
        let witness=sqlx::query("SELECT m.request_id,m.state_updated_at_us,r.generation AS request_generation FROM conversation_goal_start_materializations m JOIN conversation_goal_requests r ON r.request_id=m.request_id WHERE m.start_id=?")
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
        let request = crate::conversation_goals::execution::retained_request_tx(
            &mut tx.tx,
            self.goal_execution_installation,
            &intent,
        )
        .await
        .map_err(|error| DbError::Serialization(error.to_string()))?
        .ok_or_else(|| DbError::Serialization("Start request was not durably charged".into()))?;
        if request.checkpoint.is_some() {
            return Err(DbError::Serialization(
                "Start initial request already checkpointed; continuation admission required"
                    .into(),
            ));
        }
        let state: ConvState = serde_json::from_str(&row.get::<String, _>("state"))
            .map_err(|error| DbError::Serialization(error.to_string()))?;
        let initial_timestamp = DateTime::parse_from_rfc3339(
            &row.get::<String, _>("state_updated_at"),
        )
        .is_ok_and(|timestamp| {
            timestamp.timestamp_micros() == witness.get::<i64, _>("state_updated_at_us")
        });
        let permitted = match state {
            ConvState::LlmRequesting { attempt: 1 } => initial_timestamp,
            ConvState::LlmRequesting { attempt } if attempt > 1 => true,
            ConvState::ServerOverloadRetrying { retry } => {
                retry.logical_request_id == intent.id.as_str()
                    && retry.phase == phoenix_core::domain::sm_state::LlmPhase::Ordinary
            }
            _ => false,
        };
        if !permitted {
            return Err(DbError::Serialization(
                "Start request state advanced without a new admitted request".into(),
            ));
        }
        Ok(Some(GoalStartRequestContext {
            purpose: intent.cause.purpose(),
            request_id: intent.id,
            observation: retained.observation().clone(),
        }))
    }
}
