use chrono::{DateTime, Utc};
use phoenix_core::domain::{
    conversation_goal::{GoalRequestPurpose, GoalText, ToolRoundResult},
    db_schema::{Message, MessageType},
    provider_replay::ProviderReplayUpdate,
    sm_state::ConvState,
};
use sha2::{Digest, Sha256};
use sqlx::Row;
use std::fmt::Write as _;

use super::{execution, management::GoalManagementTarget, round, GoalTx};
use crate::{workflow::AdmittedGoalRequest, Database, DbError, DbResult};

/// Actual-dispatch receipt and the exact source/target state of its canonical commit.
pub struct GoalRequestCheckpoint<'a> {
    pub admitted: &'a AdmittedGoalRequest,
    pub source: &'a ConvState,
    pub source_updated_at: DateTime<Utc>,
    pub assistant: &'a Message,
    pub evidence: GoalRequestEvidence<'a>,
    pub state: &'a ConvState,
    pub state_updated_at: DateTime<Utc>,
    pub provider_replay: Option<&'a ProviderReplayUpdate>,
}

pub enum GoalRequestEvidence<'a> {
    Response,
    Round {
        results: &'a [ToolRoundResult],
        management_targets: &'a [GoalManagementTarget],
    },
}

#[derive(Debug)]
pub enum GoalRequestCheckpointOutcome {
    Committed(round::CommittedGoalRound),
    Refused,
}

#[derive(Clone, Copy)]
enum Cut {
    None,
    #[cfg(test)]
    BeforeCommit,
    #[cfg(test)]
    AfterCommit,
}

impl Database {
    /// Validate a completion before it can publish an in-flight assistant or run tools.
    ///
    /// # Errors
    /// Storage or retained-identity corruption; ordinary lifecycle fences return false.
    pub async fn goal_request_completion_is_current(
        &self,
        admitted: &AdmittedGoalRequest,
        transcript: &str,
        state: &ConvState,
        stamp: DateTime<Utc>,
    ) -> DbResult<bool> {
        if self.goal_execution_installation == execution::GoalExecutionInstallation::Disabled
            || stamp != admitted.state_updated_at()
        {
            return Ok(false);
        }
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        if !live_owner_tx(&mut tx, admitted, transcript).await?
            || !exact_slot_tx(&mut tx, transcript, state, stamp).await?
        {
            return Ok(false);
        }
        match execution::require_live_request_tx(&mut tx, admitted.intent(), transcript).await {
            Ok(()) => Ok(true),
            Err(error) if ordinary_refusal(&error) => Ok(false),
            Err(error) => Err(request_error(error)),
        }
    }

    /// Commit a real admitted response or completed Coordinator round as one unit.
    ///
    /// # Errors
    /// Decode, canonical-payload conflict, or unclassifiable storage failure.
    pub async fn commit_goal_request_checkpoint(
        &self,
        command: &GoalRequestCheckpoint<'_>,
    ) -> DbResult<GoalRequestCheckpointOutcome> {
        self.commit_goal_request_checkpoint_at_cut(command, Cut::None)
            .await
    }

    #[cfg(test)]
    pub(crate) async fn commit_goal_request_checkpoint_test_cut(
        &self,
        command: &GoalRequestCheckpoint<'_>,
        after_commit: bool,
    ) -> DbResult<GoalRequestCheckpointOutcome> {
        self.commit_goal_request_checkpoint_at_cut(
            command,
            if after_commit {
                Cut::AfterCommit
            } else {
                Cut::BeforeCommit
            },
        )
        .await
    }

    #[allow(clippy::too_many_lines)]
    async fn commit_goal_request_checkpoint_at_cut(
        &self,
        command: &GoalRequestCheckpoint<'_>,
        cut: Cut,
    ) -> DbResult<GoalRequestCheckpointOutcome> {
        use GoalRequestCheckpointOutcome::{Committed, Refused};
        if self.goal_execution_installation == execution::GoalExecutionInstallation::Disabled {
            return Ok(Refused);
        }
        validate_shape(command)?;
        let key = checkpoint_key(command)?;
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        if !live_owner_tx(
            &mut tx,
            command.admitted,
            &command.assistant.conversation_id,
        )
        .await?
        {
            return Ok(Refused);
        }
        let retained = execution::retained_request_tx(
            &mut tx,
            self.goal_execution_installation,
            command.admitted.intent(),
        )
        .await
        .map_err(request_error)?
        .ok_or_else(|| {
            DbError::Serialization("canonical checkpoint lost admitted request".into())
        })?;
        if retained.checkpoint.is_some() {
            return Ok(
                match retained_bundle_tx(&mut tx, command, &key, self.goal_execution_installation)
                    .await?
                {
                    Some(committed) => Committed(committed),
                    None => Refused,
                },
            );
        }
        if !exact_slot_tx(
            &mut tx,
            &command.assistant.conversation_id,
            command.source,
            command.source_updated_at,
        )
        .await?
        {
            return Ok(Refused);
        }
        match execution::require_live_request_tx(
            &mut tx,
            command.admitted.intent(),
            &command.assistant.conversation_id,
        )
        .await
        {
            Ok(()) => {}
            Err(error) if ordinary_refusal(&error) => return Ok(Refused),
            Err(error) => return Err(request_error(error)),
        }
        execution::checkpoint_request_tx(
            &mut tx,
            self.goal_execution_installation,
            command.admitted.intent(),
            command.admitted.claim_generation(),
            &key,
        )
        .await
        .map_err(request_error)?;
        let results = match &command.evidence {
            GoalRequestEvidence::Response => {
                crate::insert_message_tx(&mut tx, command.assistant).await?;
                Vec::new()
            }
            GoalRequestEvidence::Round {
                results,
                management_targets,
            } => {
                let (messages, replayed) = round::write_round_tx(
                    &mut tx,
                    &command.assistant.conversation_id,
                    command.assistant,
                    round::ToolRoundWrite::Management {
                        execution: command.admitted.intent().execution,
                        results,
                        targets: management_targets,
                    },
                    command.state,
                    command.state_updated_at,
                    self.goal_write_policies(),
                )
                .await?;
                if replayed {
                    return Err(DbError::Serialization(
                        "canonical round exists without request checkpoint".into(),
                    ));
                }
                messages
            }
        };
        if let Some(update) = command.provider_replay {
            crate::provider_replay::validate_goal_round_replay(command.assistant, update)?;
            crate::provider_replay::apply_replay_tx(
                &mut tx,
                &command.assistant.conversation_id,
                update,
            )
            .await?;
        }
        let (state, state_updated_at) = round::selected_state_tx(
            &mut tx,
            &command.assistant.conversation_id,
            command.state,
            command.state_updated_at,
            false,
            self.goal_control_installation,
        )
        .await
        .map_err(round::db_error)?;
        let committed = round::CommittedGoalRound {
            results,
            state,
            state_updated_at,
            replayed: false,
        };
        let result = match cut {
            Cut::None => tx.commit().await.map_err(DbError::from),
            #[cfg(test)]
            Cut::BeforeCommit => {
                tx.rollback().await?;
                Err(DbError::Serialization(
                    "injected before canonical request commit".into(),
                ))
            }
            #[cfg(test)]
            Cut::AfterCommit => {
                tx.commit().await?;
                Err(DbError::Serialization(
                    "injected after canonical request commit".into(),
                ))
            }
        };
        if let Err(error) = result {
            let mut probe = self.pool.begin_with("BEGIN IMMEDIATE").await?;
            if !live_owner_tx(
                &mut probe,
                command.admitted,
                &command.assistant.conversation_id,
            )
            .await?
            {
                return Err(error);
            }
            return match retained_bundle_tx(
                &mut probe,
                command,
                &key,
                self.goal_execution_installation,
            )
            .await?
            {
                Some(committed) => Ok(Committed(committed)),
                None => Err(error),
            };
        }
        Ok(Committed(committed))
    }
}

fn validate_shape(command: &GoalRequestCheckpoint<'_>) -> DbResult<()> {
    let intent = command.admitted.intent();
    if intent.cause.purpose() != GoalRequestPurpose::Normal
        || command.assistant.message_id != intent.id.as_str()
        || command.assistant.message_type != MessageType::Agent
    {
        return Err(DbError::Serialization(
            "canonical response differs from its actual dispatch receipt".into(),
        ));
    }
    let valid_source = match (command.source, &command.evidence) {
        (
            ConvState::LlmRequesting { .. } | ConvState::ServerOverloadRetrying { .. },
            GoalRequestEvidence::Response,
        ) => command.source_updated_at == command.admitted.state_updated_at(),
        (
            ConvState::ToolExecuting {
                assistant_message, ..
            },
            GoalRequestEvidence::Round { .. },
        ) => {
            assistant_message.message_id == intent.id.as_str()
                && assistant_message.created_at == command.assistant.created_at
                && assistant_message.display_data == command.assistant.display_data
                && assistant_message.usage == command.assistant.usage_data
                && assistant_message.content
                    == match &command.assistant.content {
                        crate::MessageContent::Agent(content) => content.clone(),
                        _ => {
                            return Err(DbError::Serialization(
                                "canonical round is not assistant content".into(),
                            ))
                        }
                    }
        }
        _ => false,
    };
    if !valid_source {
        return Err(DbError::Serialization(
            "canonical checkpoint source is not its admitted response/round".into(),
        ));
    }
    Ok(())
}

fn checkpoint_key(command: &GoalRequestCheckpoint<'_>) -> DbResult<GoalText> {
    let results = match &command.evidence {
        GoalRequestEvidence::Response => None,
        GoalRequestEvidence::Round { results, .. } => Some(*results),
    };
    let replay = match command.provider_replay {
        Some(ProviderReplayUpdate::Anthropic(set)) => {
            Some(("anthropic", serde_json::to_value(set)))
        }
        Some(ProviderReplayUpdate::Responses(set)) => {
            Some(("responses", serde_json::to_value(set)))
        }
        Some(ProviderReplayUpdate::Clear) => Some(("clear", Ok(serde_json::Value::Null))),
        None => None,
    }
    .map(|(kind, value)| value.map(|value| (kind, value)))
    .transpose()
    .map_err(|error| DbError::Serialization(error.to_string()))?;
    let bytes = serde_json::to_vec(&(
        command.admitted.intent(),
        command.admitted.claim_generation(),
        command.admitted.state_updated_at(),
        command.source,
        command.source_updated_at,
        command.assistant,
        results,
        command.state,
        command.state_updated_at,
        replay,
    ))
    .map_err(|error| DbError::Serialization(error.to_string()))?;
    let mut key = String::from("canonical-v1:");
    for byte in Sha256::digest(bytes) {
        write!(&mut key, "{byte:02x}").expect("writing to String cannot fail");
    }
    GoalText::parse(key).map_err(|error| DbError::Serialization(error.to_string()))
}

async fn live_owner_tx(
    tx: &mut GoalTx<'_>,
    admitted: &AdmittedGoalRequest,
    transcript: &str,
) -> DbResult<bool> {
    let intent = admitted.intent();
    let live: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM durable_turns t JOIN workflows w ON w.workflow_id=t.workflow_id
         WHERE t.turn_id=? AND t.workflow_id=? AND t.conversation_id=? AND t.generation=?
           AND w.generation=t.generation AND w.status='Active' AND t.input_kind='goal_start'
           AND t.disposition='Runtime' AND t.materialized=1 AND t.owns_conversation=1
           AND t.terminal_kind IS NULL AND NOT EXISTS(
             SELECT 1 FROM direct_turn_terminal_obligations o WHERE o.turn_id=t.turn_id))",
    ).bind(intent.execution.turn_id()).bind(intent.execution.workflow_id())
        .bind(transcript).bind(admitted.claim_generation())
        .fetch_one(&mut **tx).await?;
    Ok(live)
}

async fn exact_slot_tx(
    tx: &mut GoalTx<'_>,
    transcript: &str,
    state: &ConvState,
    stamp: DateTime<Utc>,
) -> DbResult<bool> {
    let row = sqlx::query("SELECT state,state_updated_at FROM conversations WHERE id=?")
        .bind(transcript)
        .fetch_one(&mut **tx)
        .await?;
    let actual: ConvState = serde_json::from_str(&row.get::<String, _>("state"))
        .map_err(|error| DbError::Serialization(error.to_string()))?;
    Ok(&actual == state && row.get::<String, _>("state_updated_at") == stamp.to_rfc3339())
}

async fn retained_bundle_tx(
    tx: &mut GoalTx<'_>,
    command: &GoalRequestCheckpoint<'_>,
    key: &GoalText,
    installation: execution::GoalExecutionInstallation,
) -> DbResult<Option<round::CommittedGoalRound>> {
    let retained = execution::retained_request_tx(tx, installation, command.admitted.intent())
        .await
        .map_err(request_error)?;
    if !retained.is_some_and(|retained| retained.checkpoint.as_ref() == Some(key))
        || !exact_slot_tx(
            tx,
            &command.assistant.conversation_id,
            command.state,
            command.state_updated_at,
        )
        .await?
    {
        return Ok(None);
    }
    super::admission::require_target_tx(
        tx,
        &command.admitted.intent().goal.owner,
        &command.assistant.conversation_id,
    )
    .await
    .map_err(round::db_error)?;
    super::control::require_current_control_tx(
        tx,
        &command.admitted.intent().goal.owner,
        command.admitted.intent().execution,
    )
    .await
    .map_err(|error| DbError::Serialization(error.to_string()))?;
    let queued: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM steering_messages WHERE conversation_id=?)",
    )
    .bind(&command.assistant.conversation_id)
    .fetch_one(&mut **tx)
    .await?;
    if queued {
        return Ok(None);
    }
    let results = match &command.evidence {
        GoalRequestEvidence::Response => {
            let actual = sqlx::query("SELECT * FROM messages WHERE message_id=?")
                .bind(&command.assistant.message_id)
                .try_map(crate::parse_message_row)
                .fetch_optional(&mut **tx)
                .await?;
            if !actual
                .as_ref()
                .is_some_and(|actual| crate::message_payload_matches(actual, command.assistant))
            {
                return Ok(None);
            }
            Vec::new()
        }
        GoalRequestEvidence::Round { results, .. } => {
            let Some(messages) = round::retained_round_tx(
                tx,
                &round::GoalRoundCheckpoint {
                    assistant: command.assistant,
                    execution: command.admitted.intent().execution,
                    results,
                    state: command.state,
                    state_updated_at: command.state_updated_at,
                },
            )
            .await
            .map_err(round::db_error)?
            else {
                return Ok(None);
            };
            messages
        }
    };
    if let Some(update) = command.provider_replay {
        if !crate::provider_replay::update_is_retained_tx(
            tx,
            &command.assistant.conversation_id,
            update,
        )
        .await?
        {
            return Ok(None);
        }
    }
    Ok(Some(round::CommittedGoalRound {
        results,
        state: command.state.clone(),
        state_updated_at: command.state_updated_at,
        replayed: true,
    }))
}

fn request_error(error: execution::GoalRequestError) -> DbError {
    match error {
        execution::GoalRequestError::Sqlx(error) => DbError::Sqlx(error),
        other => DbError::Serialization(other.to_string()),
    }
}

fn ordinary_refusal(error: &execution::GoalRequestError) -> bool {
    use super::{control::ControlError, GoalDbError};
    use execution::GoalRequestError;
    matches!(
        error,
        GoalRequestError::Revoked
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
