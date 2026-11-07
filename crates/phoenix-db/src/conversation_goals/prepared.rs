//! Immutable Coordinator Goal actions. Preparing never changes a Goal or dispatches work.
use phoenix_core::domain::conversation_goal::{
    GoalAgentSource, GoalCommandDisposition, GoalExecution, GoalId, GoalPreparationSource,
    GoalState, GoalText, GoalVersion, RemoteGoalAgentSource,
};
use phoenix_core::domain::db_schema::SourceToolCall;
use phoenix_core::domain::instance_identity::InstanceId;
use phoenix_core::domain::product_conversation::ProductConversationId;
use sqlx::{sqlite::SqliteRow, Row};
use std::str::FromStr;
use uuid::Uuid;

use super::{current_tx, GoalDbError, GoalTx, Result};
use crate::Database;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum GoalPreparationIntent {
    Create { objective: GoalText },
    Replace { objective: GoalText },
    State { disposition: GoalCommandDisposition },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedGoalAction {
    pub reference: Uuid,
    pub owner: ProductConversationId,
    pub source: GoalPreparationSource,
    pub expected_version: GoalVersion,
    pub observed_goal_id: Option<GoalId>,
    pub intent: GoalPreparationIntent,
    expires_at_us: Option<i64>,
}

impl PreparedGoalAction {
    #[must_use]
    pub fn expires_at_us(&self) -> Option<i64> {
        self.expires_at_us
    }

    /// Build the owner command represented by this immutable preparation.
    ///
    /// # Errors
    /// Returns an integrity error when a state preparation has no observed Goal.
    pub fn command(&self, principal_id: Uuid) -> Result<super::user_commands::GoalUserCommand> {
        use super::user_commands::{GoalUserChange, GoalUserCommand};
        let change = match &self.intent {
            GoalPreparationIntent::Create { objective }
            | GoalPreparationIntent::Replace { objective } => GoalUserChange::Create {
                objective: objective.clone(),
            },
            GoalPreparationIntent::State { disposition } => GoalUserChange::State {
                goal_id: self
                    .observed_goal_id
                    .clone()
                    .ok_or(GoalDbError::Integrity("prepared state goal"))?,
                disposition: disposition.clone().into(),
            },
        };
        Ok(GoalUserCommand {
            owner: self.owner.clone(),
            principal_id,
            request_id: self.reference,
            expected_version: self.expected_version,
            change,
        })
    }
}

pub(super) async fn require_coordinator(tx: &mut GoalTx<'_>, caller: &str) -> Result<()> {
    let authorized: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM conversations WHERE id=? AND runtime_role='coordinator' AND coordinator_head=1)")
        .bind(caller).fetch_one(&mut **tx).await?;
    if !authorized {
        return Err(GoalDbError::TargetChanged);
    }
    Ok(())
}

fn columns(
    intent: &GoalPreparationIntent,
) -> (&'static str, Option<&str>, Option<&str>, Option<&str>) {
    match intent {
        GoalPreparationIntent::Create { objective } => {
            ("create", Some(objective.as_str()), None, None)
        }
        GoalPreparationIntent::Replace { objective } => {
            ("replace", Some(objective.as_str()), None, None)
        }
        GoalPreparationIntent::State { disposition } => {
            let (state, reason) = match disposition {
                GoalCommandDisposition::Underway => ("underway", None),
                GoalCommandDisposition::Blocked(reason) => ("blocked", Some(reason.as_str())),
                GoalCommandDisposition::Complete(reason) => ("complete", Some(reason.as_str())),
                GoalCommandDisposition::Exhausted(reason) => ("exhausted", Some(reason.as_str())),
            };
            ("state", None, Some(state), reason)
        }
    }
}

fn decode(row: &SqliteRow) -> Result<PreparedGoalAction> {
    let reference = Uuid::from_slice(&row.try_get::<Vec<u8>, _>("reference")?)
        .map_err(|_| GoalDbError::Integrity("preparation reference"))?;
    let kind: String = row.try_get("source_kind")?;
    let source_instance_id = row.try_get::<Option<String>, _>("source_instance_id")?;
    let enrollment_id = row.try_get::<Option<String>, _>("enrollment_id")?;
    let expires_at_us = row.try_get::<Option<i64>, _>("expires_at_us")?;
    let source = GoalAgentSource {
        product_conversation_id: ProductConversationId::parse(
            row.try_get::<String, _>("source_product_id")?,
        )
        .map_err(|_| GoalDbError::Integrity("source product"))?,
        transcript_id: GoalText::parse(row.try_get::<String, _>("caller_conversation_id")?)?,
        execution: GoalExecution::parse(row.try_get("workflow_id")?, row.try_get("turn_id")?)?,
        message_id: GoalText::parse(row.try_get::<String, _>("message_id")?)?,
        tool_use_id: GoalText::parse(row.try_get::<String, _>("tool_use_id")?)?,
    };
    let objective = row
        .try_get::<Option<String>, _>("objective")?
        .map(GoalText::parse)
        .transpose()?;
    let operation: String = row.try_get("operation")?;
    let intent = match operation.as_str() {
        "create" => GoalPreparationIntent::Create {
            objective: objective.ok_or(GoalDbError::Integrity("prepared objective"))?,
        },
        "replace" => GoalPreparationIntent::Replace {
            objective: objective.ok_or(GoalDbError::Integrity("prepared objective"))?,
        },
        "state" => {
            let state: String = row.try_get("disposition")?;
            let reason = row
                .try_get::<Option<String>, _>("reason")?
                .map(GoalText::parse)
                .transpose()?;
            let disposition = match (state.as_str(), reason) {
                ("underway", None) => GoalCommandDisposition::Underway,
                ("blocked", Some(reason)) => GoalCommandDisposition::Blocked(reason),
                ("complete", Some(reason)) => GoalCommandDisposition::Complete(reason),
                ("exhausted", Some(reason)) => GoalCommandDisposition::Exhausted(reason),
                _ => return Err(GoalDbError::Integrity("prepared disposition")),
            };
            GoalPreparationIntent::State { disposition }
        }
        _ => return Err(GoalDbError::Integrity("prepared operation")),
    };
    Ok(PreparedGoalAction {
        reference,
        owner: ProductConversationId::parse(row.try_get::<String, _>("owner_id")?)
            .map_err(|_| GoalDbError::Integrity("prepared owner"))?,
        source: match kind.as_str() {
            "remote"
                if enrollment_id
                    .as_deref()
                    .is_some_and(|enrollment| Uuid::parse_str(enrollment).is_ok())
                    && expires_at_us.is_some() =>
            {
                GoalPreparationSource::Remote(RemoteGoalAgentSource {
                    instance_id: InstanceId::from_str(
                        source_instance_id
                            .as_deref()
                            .ok_or(GoalDbError::Integrity("prepared source instance"))?,
                    )
                    .map_err(|_| GoalDbError::Integrity("prepared source instance"))?,
                    source,
                })
            }
            "local"
                if source_instance_id.is_none()
                    && enrollment_id.is_none()
                    && expires_at_us.is_none() =>
            {
                GoalPreparationSource::Local(source)
            }
            _ => return Err(GoalDbError::Integrity("prepared source kind")),
        },
        expected_version: GoalVersion::parse(row.try_get("expected_version")?)?,
        observed_goal_id: row
            .try_get::<Option<String>, _>("observed_goal_id")?
            .map(GoalId::parse)
            .transpose()?,
        intent,
        expires_at_us,
    })
}

pub(super) async fn require_exact_preparation(
    tx: &mut GoalTx<'_>,
    _caller: &str,
    source: &GoalAgentSource,
    input: &super::user_commands::GoalUserCommand,
) -> Result<()> {
    let row = sqlx::query("SELECT p.*,e.caller_instance_id AS source_instance_id FROM conversation_goal_preparations p LEFT JOIN federation_enrollments e ON e.id=p.enrollment_id WHERE p.reference=?")
        .bind(input.request_id.as_bytes().as_slice())
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(GoalDbError::TargetChanged)?;
    let prepared = decode(&row)?;
    let expected = prepared.command(input.principal_id)?;
    if prepared.source != GoalPreparationSource::Local(source.clone())
        || prepared.owner != input.owner
        || expected.expected_version != input.expected_version
        || expected.change != input.change
    {
        return Err(GoalDbError::RetryConflict);
    }
    Ok(())
}

pub(super) async fn require_retained_preparation(
    tx: &mut GoalTx<'_>,
    _caller: &str,
    input: &super::user_commands::GoalUserCommand,
) -> Result<()> {
    let row = sqlx::query("SELECT p.*,e.caller_instance_id AS source_instance_id FROM conversation_goal_preparations p LEFT JOIN federation_enrollments e ON e.id=p.enrollment_id WHERE p.reference=?")
        .bind(input.request_id.as_bytes().as_slice())
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(GoalDbError::TargetChanged)?;
    let prepared = decode(&row)?;
    let expected = prepared.command(input.principal_id)?;
    if !matches!(&prepared.source, GoalPreparationSource::Local(_))
        || prepared.owner != input.owner
        || expected.expected_version != input.expected_version
        || expected.change != input.change
    {
        return Err(GoalDbError::RetryConflict);
    }
    Ok(())
}

async fn require_source_tool_provenance(
    tx: &mut GoalTx<'_>,
    caller: &str,
    source_call: &SourceToolCall,
) -> Result<()> {
    let message: Option<String> = sqlx::query_scalar(
        "SELECT content FROM messages WHERE conversation_id=? AND message_id=? AND message_type='agent'",
    )
    .bind(caller)
    .bind(&source_call.message_id)
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(message) = message {
        let blocks: Vec<phoenix_core::domain::llm_types::ContentBlock> =
            serde_json::from_str(&message)
                .map_err(|_| GoalDbError::Integrity("source tool message content"))?;
        if blocks.iter().any(|block| {
            matches!(block, phoenix_core::domain::llm_types::ContentBlock::ToolUse { id, .. } if id == &source_call.tool_use_id)
        }) {
            return Ok(());
        }
        return Err(GoalDbError::TargetChanged);
    }

    let state: Option<String> = sqlx::query_scalar("SELECT state FROM conversations WHERE id=?")
        .bind(caller)
        .fetch_optional(&mut **tx)
        .await?;
    let state: crate::ConvState =
        serde_json::from_str(&state.ok_or(GoalDbError::TargetChanged)?)
            .map_err(|_| GoalDbError::Integrity("source tool conversation state"))?;
    let crate::ConvState::ToolExecuting {
        current_tool,
        assistant_message,
        ..
    } = state
    else {
        return Err(GoalDbError::TargetChanged);
    };
    if current_tool.name() != "prepare_goal"
        || current_tool.id != source_call.tool_use_id
        || assistant_message.message_id != source_call.message_id
        || !assistant_message.content.iter().any(|block| {
            matches!(block, phoenix_core::domain::llm_types::ContentBlock::ToolUse { id, name, .. }
                if id == &source_call.tool_use_id && name == "prepare_goal")
        })
    {
        return Err(GoalDbError::TargetChanged);
    }
    Ok(())
}

pub(super) async fn require_remote_peer(
    tx: &mut GoalTx<'_>,
    authority: &crate::federation_enrollment::FederationSendAuthority,
    instance_id: InstanceId,
) -> Result<Uuid> {
    let enrollment = crate::federation_enrollment::federation_send_enrollment_id_tx(tx, authority)
        .await
        .map_err(|_| GoalDbError::TargetChanged)?;
    let enrolled_instance: String = sqlx::query_scalar(
        "SELECT caller_instance_id FROM federation_enrollments WHERE id=? AND revoked_at_us IS NULL",
    )
    .bind(enrollment.to_string())
    .fetch_one(&mut **tx)
    .await?;
    if enrolled_instance != instance_id.to_string() {
        return Err(GoalDbError::TargetChanged);
    }
    Ok(enrollment)
}

pub(super) async fn remote_preparation_tx(
    tx: &mut GoalTx<'_>,
    authority: &crate::federation_enrollment::FederationSendAuthority,
    reference: Uuid,
) -> Result<PreparedGoalAction> {
    let row = sqlx::query("SELECT p.*,e.caller_instance_id AS source_instance_id FROM conversation_goal_preparations p LEFT JOIN federation_enrollments e ON e.id=p.enrollment_id WHERE p.reference=?")
        .bind(reference.as_bytes().as_slice())
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(GoalDbError::TargetChanged)?;
    let prepared = decode(&row)?;
    let GoalPreparationSource::Remote(source) = &prepared.source else {
        return Err(GoalDbError::TargetChanged);
    };
    let enrollment_id = require_remote_peer(tx, authority, source.instance_id).await?;
    if row
        .try_get::<Option<String>, _>("enrollment_id")?
        .as_deref()
        != Some(enrollment_id.to_string().as_str())
    {
        return Err(GoalDbError::TargetChanged);
    }
    Ok(prepared)
}

fn prepare_observation(
    intent: &GoalPreparationIntent,
    current: Option<&super::GoalSnapshot>,
) -> Result<(GoalVersion, Option<GoalId>)> {
    let observed = match (intent, current) {
        (GoalPreparationIntent::Create { .. }, None) => (GoalVersion::INITIAL, None),
        (GoalPreparationIntent::Replace { .. }, Some(current)) => {
            (current.version(), Some(current.goal_id().clone()))
        }
        (GoalPreparationIntent::State { disposition }, Some(current)) => {
            if matches!(disposition, GoalCommandDisposition::Underway)
                && matches!(current.state(), GoalState::Suspended(_))
            {
                return Err(GoalDbError::Invalid("Suspended Goal requires explicit Start; disposition tools cannot reactivate it"));
            }
            (current.version(), Some(current.goal_id().clone()))
        }
        (GoalPreparationIntent::Create { .. }, Some(_)) => {
            return Err(GoalDbError::Invalid(
                "Goal exists; use explicit replace after reading current Goal/history",
            ));
        }
        _ => return Err(GoalDbError::Invalid("No saved Goal; use create first")),
    };
    Ok(observed)
}

pub(super) enum RemoteCoordinatorGoalClaim {
    Claimed(Box<RemoteClaimedGoalAction>),
    Replay(super::user_commands::GoalUserReceipt),
    Unclassified,
}

pub(super) struct RemoteClaimedGoalAction {
    pub(super) command: super::user_commands::GoalUserCommand,
    pub(super) source: RemoteGoalAgentSource,
}

enum RemoteCoordinatorGoalClaimState {
    Prepared(Box<RemoteClaimedGoalAction>),
    Claimed,
    Replay(super::user_commands::GoalUserReceipt),
}

async fn remote_claim_state_tx(
    tx: &mut GoalTx<'_>,
    authority: &crate::federation_enrollment::FederationSendAuthority,
    reference: Uuid,
) -> Result<RemoteCoordinatorGoalClaimState> {
    let prepared = remote_preparation_tx(tx, authority, reference).await?;
    let input = prepared.command(Uuid::nil())?;
    let GoalPreparationSource::Remote(source) = prepared.source else {
        return Err(GoalDbError::Integrity("remote preparation source"));
    };
    if let Some(receipt) = super::user_commands::retained_tx(
        tx,
        &input,
        super::user_commands::GoalReceiptStore::Coordinator,
    )
    .await?
    {
        return Ok(RemoteCoordinatorGoalClaimState::Replay(receipt));
    }
    let state: String = sqlx::query_scalar(
        "SELECT apply_state FROM conversation_goal_preparations WHERE reference=?",
    )
    .bind(reference.as_bytes().as_slice())
    .fetch_one(&mut **tx)
    .await?;
    match state.as_str() {
        "prepared" => Ok(RemoteCoordinatorGoalClaimState::Prepared(Box::new(
            RemoteClaimedGoalAction {
                command: input,
                source,
            },
        ))),
        "claimed" => Ok(RemoteCoordinatorGoalClaimState::Claimed),
        _ => Err(GoalDbError::Integrity("remote Goal claim state")),
    }
}

#[cfg(test)]
#[derive(Clone, Copy)]
pub(super) enum RemoteGoalClaimCommitCut {
    None,
    LostAcknowledgement,
    RolledBack,
}

#[cfg(test)]
#[derive(Clone)]
pub(super) struct RemoteGoalClaimPause {
    pub(super) claimed: std::sync::Arc<tokio::sync::Notify>,
    pub(super) resume: std::sync::Arc<tokio::sync::Notify>,
}

#[cfg(test)]
impl RemoteGoalClaimPause {
    pub(super) async fn after_claim(&self) {
        self.claimed.notify_one();
        tokio::time::timeout(std::time::Duration::from_secs(30), self.resume.notified())
            .await
            .expect("remote Goal claim test did not resume within 30 seconds");
    }
}

impl Database {
    /// Read the current Goal and history after the supplied version.
    ///
    /// # Errors
    /// Rejects a non-current Coordinator, an unavailable ordinary target,
    /// malformed retained Goal data, or a storage failure.
    pub async fn read_coordinator_goals(
        &self,
        caller: &str,
        owner: &ProductConversationId,
        after: GoalVersion,
    ) -> Result<(
        Option<super::GoalHistoryEntry>,
        Vec<super::GoalHistoryEntry>,
    )> {
        let mut tx = self.pool().begin().await?;
        require_coordinator(&mut tx, caller).await?;
        let live = Self::get_ordinary_product_conversation_on(&mut tx, owner).await?;
        super::admission::require_target_tx(&mut tx, owner, &live.latest_transcript_row_id).await?;
        let row = sqlx::query("SELECT h.*,g.objective FROM conversation_goal_history h JOIN conversation_goal_ids g USING(goal_id,owner_id) WHERE owner_id=? ORDER BY version DESC LIMIT 1")
            .bind(owner.as_str()).fetch_optional(&mut *tx).await?;
        let current = match row {
            Some(row) => Some(super::history_entry(&mut tx, &row).await?),
            None => None,
        };
        let history = super::history_tx(&mut tx, owner, after).await?;
        tx.rollback().await?;
        Ok((current, history))
    }

    /// Check that the caller is the current Coordinator head.
    ///
    /// # Errors
    /// Rejects a non-current Coordinator or a storage failure.
    pub async fn require_current_coordinator(&self, caller: &str) -> Result<()> {
        let mut tx = self.pool().begin().await?;
        require_coordinator(&mut tx, caller).await?;
        tx.rollback().await?;
        Ok(())
    }

    /// Retain an immutable action attributed to the Coordinator's live tool call.
    ///
    /// # Errors
    /// Rejects invalid source provenance, unavailable callers or targets,
    /// conflicting retries, inapplicable intents, malformed data, or storage failures.
    pub async fn prepare_coordinator_goal_action(
        &self,
        caller: &str,
        owner: &ProductConversationId,
        source_call: &SourceToolCall,
        turn_id: phoenix_core::domain::sm_event::DirectTurnTurnId,
        intent: GoalPreparationIntent,
    ) -> crate::workflow::LocalAuthorityResult<Result<PreparedGoalAction>> {
        self.prepare_coordinator_goal_action_with_commit(
            caller,
            owner,
            source_call,
            turn_id,
            intent,
            PreparationCommit::Normal,
        )
        .await
    }

    async fn prepare_coordinator_goal_action_with_commit(
        &self,
        caller: &str,
        owner: &ProductConversationId,
        source_call: &SourceToolCall,
        turn_id: phoenix_core::domain::sm_event::DirectTurnTurnId,
        intent: GoalPreparationIntent,
        commit: PreparationCommit,
    ) -> crate::workflow::LocalAuthorityResult<Result<PreparedGoalAction>> {
        use crate::workflow::LocalAuthorityResult;

        let Ok(turn_id) = i64::try_from(turn_id.0) else {
            return LocalAuthorityResult::DurableFactEstablished(Err(GoalDbError::Invalid(
                "durable turn ID is out of range",
            )));
        };
        let mut connection = match self.pool().acquire().await {
            Ok(connection) => connection,
            Err(error) => return LocalAuthorityResult::DurableFactEstablished(Err(error.into())),
        };
        let mut tx = match sqlx::Connection::begin_with(&mut *connection, "BEGIN IMMEDIATE").await {
            Ok(tx) => tx,
            Err(error) => return LocalAuthorityResult::DurableFactEstablished(Err(error.into())),
        };
        let result = async {
            require_coordinator(&mut tx, caller).await?;
            if let Some(row) = sqlx::query("SELECT p.*,NULL AS source_instance_id FROM conversation_goal_preparations p WHERE p.source_kind='local' AND p.caller_conversation_id=? AND p.message_id=? AND p.tool_use_id=?")
                .bind(caller).bind(&source_call.message_id).bind(&source_call.tool_use_id).fetch_optional(&mut *tx).await? {
                let previous = decode(&row)?;
                if previous.owner != *owner || previous.intent != intent
                    || !matches!(&previous.source, GoalPreparationSource::Local(source) if source.execution.turn_id() == turn_id)
                { return Err(GoalDbError::RetryConflict); }
                return Ok(previous);
            }
            let source_row = sqlx::query("SELECT c.product_conversation_id,t.workflow_id,t.turn_id FROM conversations c JOIN durable_turns t ON t.conversation_id=c.id WHERE c.id=? AND t.turn_id=? AND t.disposition='Runtime' AND t.terminal_kind IS NULL AND t.owns_conversation=1")
                .bind(caller).bind(turn_id).fetch_optional(&mut *tx).await?.ok_or(GoalDbError::TargetChanged)?;
            let source = GoalAgentSource {
                product_conversation_id: ProductConversationId::parse(
                    source_row.try_get::<String, _>("product_conversation_id")?,
                )
                .map_err(|_| GoalDbError::Integrity("source product"))?,
                transcript_id: GoalText::parse(caller)?,
                execution: GoalExecution::parse(
                    source_row.try_get("workflow_id")?,
                    source_row.try_get("turn_id")?,
                )?,
                message_id: GoalText::parse(source_call.message_id.clone())?,
                tool_use_id: GoalText::parse(source_call.tool_use_id.clone())?,
            };
            require_source_tool_provenance(&mut tx, caller, source_call).await?;
            let live = Self::get_ordinary_product_conversation_on(&mut tx, owner).await?;
            super::admission::require_target_tx(&mut tx, owner, &live.latest_transcript_row_id).await?;
            let current = current_tx(&mut tx, owner).await?;
            let (expected_version, observed_goal_id) = prepare_observation(&intent, current.as_ref())?;

            let reference = Uuid::new_v4();
            let (operation, objective, disposition, reason) = columns(&intent);
            sqlx::query("INSERT INTO conversation_goal_preparations(reference,owner_id,source_kind,caller_conversation_id,source_product_id,workflow_id,turn_id,message_id,tool_use_id,operation,expected_version,observed_goal_id,objective,disposition,reason) VALUES(?,?,'local',?,?,?,?,?,?,?,?,?,?,?,?)")
                .bind(reference.as_bytes().as_slice()).bind(owner.as_str()).bind(caller).bind(source.product_conversation_id.as_str())
                .bind(source.execution.workflow_id()).bind(source.execution.turn_id()).bind(source.message_id.as_str()).bind(source.tool_use_id.as_str())
                .bind(operation).bind(expected_version.get()).bind(observed_goal_id.as_ref().map(GoalId::as_str)).bind(objective).bind(disposition).bind(reason).execute(&mut *tx).await?;
            Ok(PreparedGoalAction {
                reference,
                owner: owner.clone(),
                source: GoalPreparationSource::Local(source),
                expected_version,
                observed_goal_id,
                intent,
                expires_at_us: None,
            })
        }
        .await;
        match result {
            Ok(action) => match commit.execute(tx).await {
                Ok(()) => LocalAuthorityResult::DurableFactEstablished(Ok(action)),
                Err(_) => LocalAuthorityResult::DurableFactUnclassified,
            },
            Err(error) => match tx.rollback().await {
                Ok(()) => LocalAuthorityResult::DurableFactEstablished(Err(error)),
                Err(_) => LocalAuthorityResult::DurableFactUnclassified,
            },
        }
    }

    /// Retain a remote peer's immutable tool-call preparation on this destination.
    pub async fn prepare_remote_coordinator_goal_action(
        &self,
        authority: &crate::federation_enrollment::FederationSendAuthority,
        source: &RemoteGoalAgentSource,
        owner: &ProductConversationId,
        intent: GoalPreparationIntent,
    ) -> crate::workflow::LocalAuthorityResult<Result<PreparedGoalAction>> {
        use crate::workflow::LocalAuthorityResult;
        let mut connection = match self.pool().acquire().await {
            Ok(connection) => connection,
            Err(error) => return LocalAuthorityResult::DurableFactEstablished(Err(error.into())),
        };
        let mut tx = match sqlx::Connection::begin_with(&mut *connection, "BEGIN IMMEDIATE").await {
            Ok(tx) => tx,
            Err(error) => return LocalAuthorityResult::DurableFactEstablished(Err(error.into())),
        };
        let result = async {
            let enrollment_id = require_remote_peer(&mut tx, authority, source.instance_id).await?;
            let call = &source.source;
            if let Some(row) = sqlx::query(
                "SELECT p.*,e.caller_instance_id AS source_instance_id
                 FROM conversation_goal_preparations p
                 JOIN federation_enrollments e ON e.id=p.enrollment_id
                 WHERE p.source_kind='remote' AND p.enrollment_id=?
                   AND source_product_id=? AND workflow_id=? AND turn_id=?
                   AND message_id=? AND tool_use_id=?",
            )
            .bind(enrollment_id.to_string())
            .bind(call.product_conversation_id.as_str())
            .bind(call.execution.workflow_id())
            .bind(call.execution.turn_id())
            .bind(call.message_id.as_str())
            .bind(call.tool_use_id.as_str())
            .fetch_optional(&mut *tx)
            .await?
            {
                let previous = decode(&row)?;
                if previous.owner != *owner || previous.intent != intent || previous.source != GoalPreparationSource::Remote(source.clone()) {
                    return Err(GoalDbError::RetryConflict);
                }
                return Ok(previous);
            }
            let live = Self::get_ordinary_product_conversation_on(&mut tx, owner).await?;
            super::admission::require_target_tx(&mut tx, owner, &live.latest_transcript_row_id).await?;
            let current = current_tx(&mut tx, owner).await?;
            let (expected_version, observed_goal_id) = prepare_observation(&intent, current.as_ref())?;
            let reference = Uuid::new_v4();
            let expires_at_us = chrono::Utc::now()
                .timestamp_micros()
                .checked_add(15 * 60 * 1_000_000)
                .ok_or(GoalDbError::Integrity("remote Goal preparation expiry"))?;
            let (operation, objective, disposition, reason) = columns(&intent);
            sqlx::query(
                "INSERT INTO conversation_goal_preparations(
                    reference,owner_id,source_kind,enrollment_id,expires_at_us,
                    caller_conversation_id,source_product_id,workflow_id,turn_id,message_id,tool_use_id,
                    operation,expected_version,observed_goal_id,objective,disposition,reason
                 ) VALUES(?,?,'remote',?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            )
            .bind(reference.as_bytes().as_slice())
            .bind(owner.as_str())
            .bind(enrollment_id.to_string())
            .bind(expires_at_us)
            .bind(call.transcript_id.as_str())
            .bind(call.product_conversation_id.as_str())
            .bind(call.execution.workflow_id())
            .bind(call.execution.turn_id())
            .bind(call.message_id.as_str())
            .bind(call.tool_use_id.as_str())
            .bind(operation)
            .bind(expected_version.get())
            .bind(observed_goal_id.as_ref().map(GoalId::as_str))
            .bind(objective)
            .bind(disposition)
            .bind(reason)
            .execute(&mut *tx)
            .await?;
            Ok(PreparedGoalAction {
                reference,
                owner: owner.clone(),
                source: GoalPreparationSource::Remote(source.clone()),
                expected_version,
                observed_goal_id,
                intent,
                expires_at_us: Some(expires_at_us),
            })
        }.await;
        match result {
            Ok(prepared) => match tx.commit().await {
                Ok(()) => LocalAuthorityResult::DurableFactEstablished(Ok(prepared)),
                Err(_) => LocalAuthorityResult::DurableFactUnclassified,
            },
            Err(error) => match tx.rollback().await {
                Ok(()) => LocalAuthorityResult::DurableFactEstablished(Err(error)),
                Err(_) => LocalAuthorityResult::DurableFactUnclassified,
            },
        }
    }

    /// Resolve a remote preparation only for its authenticated, enrolled peer.
    ///
    /// # Errors
    /// Rejects a non-current peer, a missing or changed preparation, malformed
    /// retained data, or a storage failure.
    pub async fn read_remote_coordinator_goal_preparation(
        &self,
        authority: &crate::federation_enrollment::FederationSendAuthority,
        reference: Uuid,
    ) -> Result<PreparedGoalAction> {
        let mut tx = self.pool().begin().await?;
        let result = remote_preparation_tx(&mut tx, authority, reference).await?;
        tx.rollback().await?;
        Ok(result)
    }

    pub(super) async fn claim_remote_coordinator_goal_preparation(
        &self,
        authority: &crate::federation_enrollment::FederationSendAuthority,
        reference: Uuid,
        #[cfg(test)] commit: RemoteGoalClaimCommitCut,
    ) -> Result<RemoteCoordinatorGoalClaim> {
        let mut tx = self.pool().begin_with("BEGIN IMMEDIATE").await?;
        let action = match remote_claim_state_tx(&mut tx, authority, reference).await? {
            RemoteCoordinatorGoalClaimState::Replay(receipt) => {
                tx.rollback().await?;
                return Ok(RemoteCoordinatorGoalClaim::Replay(receipt));
            }
            RemoteCoordinatorGoalClaimState::Claimed => {
                let _ = tx.rollback().await;
                return Ok(RemoteCoordinatorGoalClaim::Unclassified);
            }
            RemoteCoordinatorGoalClaimState::Prepared(action) => action,
        };
        let changed = sqlx::query(
            "UPDATE conversation_goal_preparations SET apply_state='claimed'
             WHERE reference=? AND apply_state='prepared'
               AND expires_at_us > (unixepoch('now') * 1000000 + CAST(substr(strftime('%f','now'),4,3) AS INTEGER) * 1000)",
        )
        .bind(reference.as_bytes().as_slice())
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if changed != 1 {
            return Err(GoalDbError::TargetChanged);
        }
        #[cfg(not(test))]
        let committed = tx.commit().await;
        #[cfg(test)]
        let committed = match commit {
            RemoteGoalClaimCommitCut::None => tx.commit().await,
            RemoteGoalClaimCommitCut::LostAcknowledgement => {
                tx.commit().await.and(Err(sqlx::Error::WorkerCrashed))
            }
            RemoteGoalClaimCommitCut::RolledBack => {
                tx.rollback().await.and(Err(sqlx::Error::WorkerCrashed))
            }
        };
        match committed {
            Ok(()) => Ok(RemoteCoordinatorGoalClaim::Claimed(action)),
            Err(error) => {
                self.classify_remote_coordinator_goal_claim(authority, reference, error.into())
                    .await
            }
        }
    }

    async fn classify_remote_coordinator_goal_claim(
        &self,
        authority: &crate::federation_enrollment::FederationSendAuthority,
        reference: Uuid,
        attempted: GoalDbError,
    ) -> Result<RemoteCoordinatorGoalClaim> {
        let classified = async {
            let mut lookup = self.pool().begin_with("BEGIN IMMEDIATE").await?;
            let state = remote_claim_state_tx(&mut lookup, authority, reference).await;
            lookup.rollback().await?;
            state
        }
        .await;
        match classified {
            Ok(RemoteCoordinatorGoalClaimState::Replay(receipt)) => {
                Ok(RemoteCoordinatorGoalClaim::Replay(receipt))
            }
            Ok(RemoteCoordinatorGoalClaimState::Prepared(_)) => Err(attempted),
            Ok(RemoteCoordinatorGoalClaimState::Claimed) | Err(_) => {
                Ok(RemoteCoordinatorGoalClaim::Unclassified)
            }
        }
    }

    /// Read a retained action by its preparation reference.
    ///
    /// # Errors
    /// Rejects a non-current Coordinator, an unknown reference,
    /// malformed preparation data, or a storage failure.
    pub async fn read_coordinator_goal_preparation(
        &self,
        caller: &str,
        reference: Uuid,
    ) -> Result<PreparedGoalAction> {
        let mut tx = self.pool().begin().await?;
        require_coordinator(&mut tx, caller).await?;
        let row = sqlx::query("SELECT p.*,e.caller_instance_id AS source_instance_id FROM conversation_goal_preparations p LEFT JOIN federation_enrollments e ON e.id=p.enrollment_id WHERE p.reference=?")
            .bind(reference.as_bytes().as_slice())
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(GoalDbError::Invalid(
                "Unknown Goal action reference; prepare an explicit new action",
            ))?;
        let result = decode(&row)?;
        if !matches!(&result.source, GoalPreparationSource::Local(_)) {
            return Err(GoalDbError::TargetChanged);
        }
        tx.rollback().await?;
        Ok(result)
    }
}

enum PreparationCommit {
    Normal,
    #[cfg(test)]
    LoseAcknowledgement,
}

impl PreparationCommit {
    async fn execute(self, tx: sqlx::Transaction<'_, sqlx::Sqlite>) -> Result<()> {
        tx.commit().await?;
        match self {
            Self::Normal => Ok(()),
            #[cfg(test)]
            Self::LoseAcknowledgement => Err(sqlx::Error::WorkerCrashed.into()),
        }
    }
}

#[cfg(test)]
mod tests;
