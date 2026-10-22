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

use super::{execution, round, GoalTx};
use crate::{workflow::AdmittedGoalRequest, Database, DbError, DbResult};

/// Actual-dispatch receipt and the exact source/target state of its canonical commit.
pub(crate) struct GoalAccounting<'a> {
    pub admitted: &'a AdmittedGoalRequest,
    pub source: &'a ConvState,
    pub source_updated_at: chrono::DateTime<chrono::Utc>,
}

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
    Round { results: &'a [ToolRoundResult] },
}

#[derive(Debug)]
pub enum GoalRequestCheckpointOutcome {
    Committed(round::CommittedGoalRound),
    Refused,
}

pub(crate) use crate::provider_replay::CanonicalCommitCut as Cut;

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

    async fn commit_goal_request_checkpoint_at_cut(
        &self,
        command: &GoalRequestCheckpoint<'_>,
        cut: Cut,
    ) -> DbResult<GoalRequestCheckpointOutcome> {
        match self.persist_canonical_checkpoint(command, cut).await? {
            crate::provider_replay::CanonicalWriteOutcome::Committed(round) => {
                Ok(GoalRequestCheckpointOutcome::Committed(round))
            }
            crate::provider_replay::CanonicalWriteOutcome::Superseded => {
                Ok(GoalRequestCheckpointOutcome::Refused)
            }
        }
    }
}

pub(crate) enum AccountingAdmission {
    Ready(GoalText),
    Retained(round::CommittedGoalRound),
    Refused,
}

pub(crate) async fn admit_accounting_tx(
    tx: &mut GoalTx<'_>,
    command: &GoalRequestCheckpoint<'_>,
    installation: execution::GoalExecutionInstallation,
) -> DbResult<AccountingAdmission> {
    use AccountingAdmission::{Ready, Refused, Retained};
    if installation == execution::GoalExecutionInstallation::Disabled {
        return Ok(Refused);
    }
    validate_shape(command)?;
    let key = checkpoint_key(command)?;
    let retained = execution::retained_request_tx(tx, installation, command.admitted.intent())
        .await
        .map_err(request_error)?
        .ok_or_else(|| {
            DbError::Serialization("canonical checkpoint lost admitted request".into())
        })?;
    if retained.checkpoint.is_some() {
        return Ok(
            match retained_bundle_tx(tx, command, &key, installation).await? {
                Some(round) => Retained(round),
                None => Refused,
            },
        );
    }
    if !live_owner_tx(tx, command.admitted, &command.assistant.conversation_id).await? {
        return Ok(Refused);
    }
    if !exact_slot_tx(
        tx,
        &command.assistant.conversation_id,
        command.source,
        command.source_updated_at,
    )
    .await?
    {
        return Ok(Refused);
    }
    match execution::require_live_request_tx(
        tx,
        command.admitted.intent(),
        &command.assistant.conversation_id,
    )
    .await
    {
        Ok(()) => {}
        Err(error) if ordinary_refusal(&error) => return Ok(Refused),
        Err(error) => return Err(request_error(error)),
    }
    // The explicit report may close the Goal. Charge settlement precedes that write in this transaction.
    execution::checkpoint_request_tx(
        tx,
        installation,
        command.admitted.intent(),
        command.admitted.claim_generation(),
        &key,
    )
    .await
    .map_err(request_error)?;
    Ok(Ready(key))
}

pub(crate) async fn classify_accounting_tx(
    tx: &mut GoalTx<'_>,
    command: &GoalRequestCheckpoint<'_>,
    key: &GoalText,
    installation: execution::GoalExecutionInstallation,
) -> DbResult<Option<round::CommittedGoalRound>> {
    retained_bundle_tx(tx, command, key, installation).await
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
    if !same_claim_tx(tx, admitted, transcript).await? {
        return Ok(false);
    }
    let terminal: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM direct_turn_terminal_obligations WHERE turn_id=?)",
    )
    .bind(admitted.intent().execution.turn_id())
    .fetch_one(&mut **tx)
    .await?;
    Ok(!terminal)
}

async fn same_claim_tx(
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
           AND t.terminal_kind IS NULL)",
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
    if !same_claim_tx(tx, command.admitted, &command.assistant.conversation_id).await? {
        return Ok(None);
    }
    let selected = settle_request_boundary_tx(tx, command, None).await?;
    let terminal = terminal_obligation(command, &selected)?;
    let slot_matches = if let Some(obligation) = terminal.as_ref() {
        crate::workflow::WorkflowRepository::terminal_obligation_matches_tx(tx, obligation).await?
            && exact_slot_tx(
                tx,
                &command.assistant.conversation_id,
                command.source,
                command.source_updated_at,
            )
            .await?
    } else {
        live_owner_tx(tx, command.admitted, &command.assistant.conversation_id).await?
            && exact_slot_tx(
                tx,
                &command.assistant.conversation_id,
                &selected,
                command.state_updated_at,
            )
            .await?
    };
    let retained = execution::retained_request_tx(tx, installation, command.admitted.intent())
        .await
        .map_err(request_error)?;
    if !retained.is_some_and(|retained| retained.checkpoint.as_ref() == Some(key)) || !slot_matches
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
                    state: &selected,
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
        state: selected,
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

pub(crate) fn terminal_obligation(
    command: &GoalRequestCheckpoint<'_>,
    selected: &ConvState,
) -> DbResult<Option<crate::workflow::DirectTurnTerminalObligationInput>> {
    if !matches!(selected, ConvState::Idle) {
        return Ok(None);
    }
    let turn = u64::try_from(command.admitted.intent().execution.turn_id())
        .map_err(|error| DbError::Serialization(error.to_string()))?;
    let generation = u64::try_from(command.admitted.claim_generation())
        .map_err(|error| DbError::Serialization(error.to_string()))?;
    Ok(Some(crate::workflow::DirectTurnTerminalObligationInput {
        turn_id: phoenix_workflow::TurnAuthorityId(turn),
        expected_generation: generation,
        terminal: phoenix_workflow::TurnTerminal::Completed,
        projection: crate::workflow::PersistedConversationProjection {
            state: selected.clone(),
            state_updated_at: command.state_updated_at,
        },
        response_message_id: None,
    }))
}

pub(crate) async fn settle_request_boundary_tx(
    tx: &mut GoalTx<'_>,
    command: &GoalRequestCheckpoint<'_>,
    policies: Option<execution::GoalWritePolicies>,
) -> DbResult<ConvState> {
    use phoenix_core::domain::conversation_goal::{
        GoalActor, GoalAttribution, GoalChange, GoalDisposition, GoalMutation, GoalState,
        GoalTimestampUs,
    };
    if !matches!(
        command.state,
        ConvState::Idle | ConvState::LlmRequesting { .. }
    ) {
        return Ok(command.state.clone());
    }
    let intent = command.admitted.intent();
    let Some(goal) = super::snapshot_tx(tx, &intent.goal.owner)
        .await
        .map_err(round::db_error)?
    else {
        return Ok(ConvState::Idle);
    };
    if goal.goal_id != intent.goal.goal_id || !matches!(goal.state, GoalState::Underway) {
        return Ok(ConvState::Idle);
    }
    let (used, limit): (i64, i64) = sqlx::query_as(
        "SELECT
        (SELECT count(*) FROM conversation_goal_requests WHERE goal_id=?),request_limit
        FROM conversation_goal_limits WHERE goal_id=?",
    )
    .bind(goal.goal_id.as_str())
    .bind(goal.goal_id.as_str())
    .fetch_one(&mut **tx)
    .await?;
    if used >= limit {
        if let Some(policies) = policies {
            let action = GoalText::parse(format!("{}:allowance-exhausted", intent.id.as_str()))
                .map_err(|error| DbError::Serialization(error.to_string()))?;
            let actor = GoalActor::GeneratedHost {
                action_id: action.clone(),
            };
            super::apply_tx(
                tx,
                &GoalMutation {
                    owner: goal.owner,
                    expected_version: goal.version,
                    action_id: action,
                    attribution: GoalAttribution {
                        requested_by: actor.clone(),
                        applied_by: actor,
                    },
                    change: GoalChange::SetDisposition {
                        goal_id: goal.goal_id,
                        disposition: GoalDisposition::Exhausted(
                            GoalText::parse("request allowance exhausted")
                                .map_err(|error| DbError::Serialization(error.to_string()))?,
                        ),
                    },
                },
                GoalTimestampUs::parse(command.state_updated_at.timestamp_micros())
                    .map_err(|error| DbError::Serialization(error.to_string()))?,
                policies,
            )
            .await
            .map_err(round::db_error)?;
        }
        return Ok(ConvState::Idle);
    }
    Ok(ConvState::LlmRequesting { attempt: 1 })
}
