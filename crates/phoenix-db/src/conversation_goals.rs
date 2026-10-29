//! Read-only persistence access for conversation goals.

use phoenix_core::domain::conversation_goal::{
    GoalActor, GoalAdmissionObservation, GoalAgentSource, GoalAttribution, GoalChange,
    GoalDisposition, GoalError, GoalExecution, GoalExecutionBinding, GoalFence, GoalHistoryEntry,
    GoalId, GoalMutation, GoalSnapshot, GoalState, GoalText, GoalTimestampUs, GoalUserCommandKind,
    GoalVersion,
};
use phoenix_core::domain::product_conversation::ProductConversationId;
use sqlx::{sqlite::SqliteRow, Row, Sqlite, Transaction};

use crate::Database;

type GoalTx<'a> = Transaction<'a, Sqlite>;

mod admission;
pub mod checkpoint;
pub mod control;
pub mod execution;
pub mod prepared;
pub mod round;
pub mod user_commands;

pub use round::{CommittedGoalRound, GoalRoundCheckpoint};

#[cfg(test)]
mod snapshot_tests;

#[cfg(test)]
mod writer_tests;

#[derive(Debug, thiserror::Error)]
pub enum GoalDbError {
    #[error("{0}")]
    Invalid(&'static str),
    #[error("goal execution binding conflicts with its retained admission")]
    BindingConflict,
    #[error("goal mutation requires a settled turn boundary")]
    Busy,
    #[error("goal owner or current transcript changed during admission")]
    TargetChanged,
    #[error(transparent)]
    Control(#[from] control::ControlError),
    #[error("goal action identity was already used for another mutation")]
    RetryConflict,
    #[error("goal identity is already retained")]
    DuplicateGoalId,
    #[error(transparent)]
    Admission(#[from] crate::DbError),
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
    #[error(transparent)]
    Domain(#[from] GoalError),
    #[error("invalid persisted goal data: {0}")]
    Integrity(&'static str),
}

type Result<T> = std::result::Result<T, GoalDbError>;

fn text(value: String) -> Result<GoalText> {
    Ok(GoalText::parse(value)?)
}

fn product(value: String) -> Result<ProductConversationId> {
    ProductConversationId::parse(value).map_err(|_| GoalDbError::Integrity("product id"))
}

fn read_state(row: &SqliteRow) -> Result<GoalState> {
    let kind: String = row.try_get("state")?;
    let reason: Option<String> = row.try_get("reason")?;
    match (kind.as_str(), reason) {
        ("underway", None) => Ok(GoalState::Underway),
        ("blocked", Some(reason)) => Ok(GoalState::Blocked(text(reason)?)),
        ("suspended", Some(reason)) => Ok(GoalState::Suspended(text(reason)?)),
        ("complete", Some(reason)) => Ok(GoalState::Complete(text(reason)?)),
        ("exhausted", Some(reason)) => Ok(GoalState::Exhausted(text(reason)?)),
        _ => Err(GoalDbError::Integrity("state/reason")),
    }
}

fn disposition(state: &GoalState) -> GoalDisposition {
    match state {
        GoalState::Underway => GoalDisposition::Underway,
        GoalState::Blocked(reason) => GoalDisposition::Blocked(reason.clone()),
        GoalState::Suspended(reason) => GoalDisposition::Suspended(reason.clone()),
        GoalState::Complete(reason) => GoalDisposition::Complete(reason.clone()),
        GoalState::Exhausted(reason) => GoalDisposition::Exhausted(reason.clone()),
    }
}

fn snapshot(row: &SqliteRow) -> Result<GoalSnapshot> {
    Ok(GoalSnapshot::new(
        product(row.try_get("owner_id")?)?,
        GoalId::parse(row.try_get::<String, _>("goal_id")?)?,
        GoalVersion::parse(row.try_get("version")?)?,
        text(row.try_get("objective")?)?,
        read_state(row)?,
    )?)
}

async fn history_tx(
    tx: &mut GoalTx<'_>,
    owner: &ProductConversationId,
    after: GoalVersion,
) -> Result<Vec<GoalHistoryEntry>> {
    let rows = sqlx::query("SELECT h.*, g.objective FROM conversation_goal_history h JOIN conversation_goal_ids g USING (goal_id,owner_id) WHERE owner_id=? AND version>? ORDER BY version LIMIT 100")
        .bind(owner.as_str()).bind(after.get()).fetch_all(&mut **tx).await?;
    let mut entries = Vec::with_capacity(rows.len());
    for row in &rows {
        entries.push(history_entry(tx, row).await?);
    }
    Ok(entries)
}

async fn current_tx(
    tx: &mut GoalTx<'_>,
    owner: &ProductConversationId,
) -> Result<Option<GoalSnapshot>> {
    sqlx::query(
        "SELECT h.*, g.objective FROM conversation_goal_history h JOIN conversation_goal_ids g USING (goal_id,owner_id) WHERE owner_id=? ORDER BY version DESC LIMIT 1",
    )
    .bind(owner.as_str())
    .fetch_optional(&mut **tx)
    .await?
    .as_ref()
    .map(snapshot)
    .transpose()
}

async fn binding_tx(tx: &mut GoalTx<'_>, turn_id: i64) -> Result<Option<GoalExecutionBinding>> {
    let Some(row) = sqlx::query("SELECT * FROM conversation_goal_bindings WHERE turn_id=?")
        .bind(turn_id)
        .fetch_optional(&mut **tx)
        .await?
    else {
        return Ok(None);
    };
    let owner = product(row.try_get("owner_id")?)?;
    let version = GoalVersion::parse(
        row.try_get::<Option<i64>, _>("observed_version")?
            .unwrap_or(0),
    )?;
    let goal_id: Option<String> = row.try_get("goal_id")?;
    let observation = match goal_id {
        Some(goal_id) => GoalAdmissionObservation::Goal(GoalFence::new(
            owner.clone(),
            GoalId::parse(goal_id)?,
            version,
        )?),
        None => GoalAdmissionObservation::NoGoal {
            observed_version: version,
        },
    };
    Ok(Some(GoalExecutionBinding::new(
        owner,
        GoalExecution::parse(row.try_get("workflow_id")?, turn_id)?,
        observation,
    )?))
}

async fn read_actor(tx: &mut GoalTx<'_>, actor_id: i64) -> Result<GoalActor> {
    let row = sqlx::query("SELECT actor_id,owner_id,kind,source_product_id,transcript_id,workflow_id,turn_id,message_id,tool_use_id,host_action_id FROM conversation_goal_actors WHERE actor_id=?")
        .bind(actor_id)
        .fetch_one(&mut **tx)
        .await?;
    let kind: String = row.try_get("kind")?;
    match kind.as_str() {
        "user_control" => {
            let control = sqlx::query("SELECT control_principal_id,control_request_id,control_command FROM conversation_goal_actors WHERE actor_id=?")
                .bind(actor_id).fetch_one(&mut **tx).await?;
            Ok(GoalActor::UserControl {
                principal_id: uuid::Uuid::from_slice(
                    &control.try_get::<Vec<u8>, _>("control_principal_id")?,
                )
                .map_err(|_| GoalDbError::Integrity("goal control principal identity"))?,
                request_id: uuid::Uuid::from_slice(
                    &control.try_get::<Vec<u8>, _>("control_request_id")?,
                )
                .map_err(|_| GoalDbError::Integrity("goal control request identity"))?,
                command: match control.try_get::<String, _>("control_command")?.as_str() {
                    "create" => GoalUserCommandKind::Create,
                    "state" => GoalUserCommandKind::State,
                    _ => return Err(GoalDbError::Integrity("goal control command kind")),
                },
            })
        }
        "user_api" => Ok(GoalActor::UserApi {
            transcript_id: text(row.try_get("transcript_id")?)?,
            message_id: text(row.try_get("message_id")?)?,
        }),
        "generated_host" => Ok(GoalActor::GeneratedHost {
            action_id: text(row.try_get("host_action_id")?)?,
        }),
        "owning_agent" | "coordinator" => {
            let source = GoalAgentSource {
                product_conversation_id: product(row.try_get("source_product_id")?)?,
                transcript_id: text(row.try_get("transcript_id")?)?,
                execution: GoalExecution::parse(
                    row.try_get("workflow_id")?,
                    row.try_get("turn_id")?,
                )?,
                message_id: text(row.try_get("message_id")?)?,
                tool_use_id: text(row.try_get("tool_use_id")?)?,
            };
            Ok(if kind == "owning_agent" {
                GoalActor::OwningAgent(source)
            } else {
                GoalActor::Coordinator(source)
            })
        }
        _ => Err(GoalDbError::Integrity("actor kind")),
    }
}

async fn history_entry(tx: &mut GoalTx<'_>, row: &SqliteRow) -> Result<GoalHistoryEntry> {
    let snapshot = snapshot(row)?;
    let kind: String = row.try_get("change_kind")?;
    let change = match kind.as_str() {
        "create" => GoalChange::Create {
            goal_id: snapshot.goal_id().clone(),
            objective: snapshot.objective().clone(),
        },
        "state" => GoalChange::SetDisposition {
            goal_id: snapshot.goal_id().clone(),
            disposition: disposition(snapshot.state()),
        },
        "report" => GoalChange::Report {
            binding: binding_tx(tx, row.try_get("report_turn_id")?)
                .await?
                .ok_or(GoalDbError::Integrity("report binding"))?,
            disposition: disposition(snapshot.state()).try_into()?,
        },
        _ => return Err(GoalDbError::Integrity("change kind")),
    };
    Ok(GoalHistoryEntry::new(
        snapshot.clone(),
        GoalMutation {
            owner: snapshot.owner().clone(),
            expected_version: GoalVersion::parse(snapshot.version().get() - 1)?,
            action_id: text(row.try_get("action_id")?)?,
            attribution: GoalAttribution {
                requested_by: read_actor(tx, row.try_get("requested_actor_id")?).await?,
                applied_by: read_actor(tx, row.try_get("applied_actor_id")?).await?,
            },
            change,
        },
        GoalTimestampUs::parse(row.try_get("recorded_at_us")?)?,
    )?)
}

fn state_columns(state: &GoalState) -> (&'static str, Option<&str>) {
    match state {
        GoalState::Underway => ("underway", None),
        GoalState::Blocked(reason) => ("blocked", Some(reason.as_str())),
        GoalState::Suspended(reason) => ("suspended", Some(reason.as_str())),
        GoalState::Complete(reason) => ("complete", Some(reason.as_str())),
        GoalState::Exhausted(reason) => ("exhausted", Some(reason.as_str())),
    }
}

async fn insert_actor(
    tx: &mut GoalTx<'_>,
    owner: &ProductConversationId,
    actor: &GoalActor,
    result_version: GoalVersion,
) -> Result<i64> {
    if let GoalActor::OwningAgent(source) | GoalActor::Coordinator(source) = actor {
        let row = sqlx::query(
            "SELECT message_type,content FROM messages WHERE conversation_id=? AND message_id=?",
        )
        .bind(source.transcript_id.as_str())
        .bind(source.message_id.as_str())
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(GoalDbError::Integrity("source message missing"))?;
        let kind: String = row.try_get("message_type")?;
        let blocks: Vec<phoenix_core::domain::llm_types::ContentBlock> =
            serde_json::from_str(&row.try_get::<String, _>("content")?)
                .map_err(|_| GoalDbError::Integrity("source tool message content"))?;
        if kind != "agent" || !blocks.iter().any(|block| matches!(block,
            phoenix_core::domain::llm_types::ContentBlock::ToolUse { id, .. } if id == source.tool_use_id.as_str())) {
            return Err(GoalDbError::Integrity("source tool call mismatch"));
        }
    }
    let (kind, source_product, transcript, workflow, turn, message, tool, host) = match actor {
        GoalActor::UserControl {
            principal_id,
            request_id,
            command,
        } => {
            let command = match command {
                GoalUserCommandKind::Create => "create",
                GoalUserCommandKind::State => "state",
            };
            let existing: Option<i64> = sqlx::query_scalar("SELECT actor_id FROM conversation_goal_actors WHERE kind='user_control' AND owner_id=?1 AND control_principal_id=?2 AND control_request_id=?3 AND control_command=?4 AND control_result_version=?5")
                .bind(owner.as_str()).bind(principal_id.as_bytes().as_slice())
                .bind(request_id.as_bytes().as_slice()).bind(command).bind(result_version.get())
                .fetch_optional(&mut **tx).await?;
            if let Some(existing) = existing {
                return Ok(existing);
            }
            return Ok(sqlx::query("INSERT INTO conversation_goal_actors (owner_id,kind,control_principal_id,control_request_id,control_command,control_result_version) VALUES (?1,'user_control',?2,?3,?4,?5)")
                .bind(owner.as_str()).bind(principal_id.as_bytes().as_slice())
                .bind(request_id.as_bytes().as_slice()).bind(command).bind(result_version.get())
                .execute(&mut **tx).await?.last_insert_rowid());
        }
        GoalActor::UserApi {
            transcript_id,
            message_id,
        } => (
            "user_api",
            None,
            Some(transcript_id.as_str()),
            None,
            None,
            Some(message_id.as_str()),
            None,
            None,
        ),
        GoalActor::GeneratedHost { action_id } => (
            "generated_host",
            None,
            None,
            None,
            None,
            None,
            None,
            Some(action_id.as_str()),
        ),
        GoalActor::OwningAgent(source) | GoalActor::Coordinator(source) => (
            if matches!(actor, GoalActor::OwningAgent(_)) {
                "owning_agent"
            } else {
                "coordinator"
            },
            Some(source.product_conversation_id.as_str()),
            Some(source.transcript_id.as_str()),
            Some(source.execution.workflow_id()),
            Some(source.execution.turn_id()),
            Some(source.message_id.as_str()),
            Some(source.tool_use_id.as_str()),
            None,
        ),
    };
    Ok(sqlx::query("INSERT INTO conversation_goal_actors (owner_id,kind,source_product_id,transcript_id,workflow_id,turn_id,message_id,tool_use_id,host_action_id) VALUES (?,?,?,?,?,?,?,?,?)")
        .bind(owner.as_str()).bind(kind).bind(source_product).bind(transcript)
        .bind(workflow).bind(turn).bind(message).bind(tool).bind(host)
        .execute(&mut **tx).await?.last_insert_rowid())
}

async fn retry_tx(
    tx: &mut GoalTx<'_>,
    mutation: &GoalMutation,
) -> Result<Option<GoalHistoryEntry>> {
    if let Some(row) = sqlx::query("SELECT h.*, g.objective FROM conversation_goal_history h JOIN conversation_goal_ids g USING (goal_id,owner_id) WHERE owner_id=? AND action_id=?")
        .bind(mutation.owner.as_str()).bind(mutation.action_id.as_str())
        .fetch_optional(&mut **tx).await? {
        let previous = history_entry(tx, &row).await?;
        if previous.mutation() != mutation { return Err(GoalDbError::RetryConflict); }
        return Ok(Some(previous));
    }
    Ok(None)
}

async fn action_is_claimed_tx(tx: &mut GoalTx<'_>, mutation: &GoalMutation) -> Result<bool> {
    for actor in [
        &mutation.attribution.requested_by,
        &mutation.attribution.applied_by,
    ] {
        if let GoalActor::OwningAgent(source) | GoalActor::Coordinator(source) = actor {
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM conversation_goal_actors
                 WHERE kind IN ('owning_agent','coordinator') AND source_product_id=?
                   AND transcript_id=? AND workflow_id=? AND turn_id=? AND message_id=? AND tool_use_id=?)",
            )
            .bind(source.product_conversation_id.as_str()).bind(source.transcript_id.as_str())
            .bind(source.execution.workflow_id()).bind(source.execution.turn_id())
            .bind(source.message_id.as_str()).bind(source.tool_use_id.as_str())
            .fetch_one(&mut **tx).await?;
            if exists {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

async fn apply_goal_tx(
    tx: &mut GoalTx<'_>,
    mutation: &GoalMutation,
    expected_transcript: Option<&str>,
    at: GoalTimestampUs,
    policies: execution::GoalWritePolicies,
) -> Result<GoalHistoryEntry> {
    if let Some(previous) = retry_tx(tx, mutation).await? {
        return Ok(previous);
    }
    if action_is_claimed_tx(tx, mutation).await? {
        return Err(GoalDbError::RetryConflict);
    }
    if let GoalChange::Create { goal_id, .. } = &mutation.change {
        let retained: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM conversation_goal_ids WHERE goal_id=?)",
        )
        .bind(goal_id.as_str())
        .fetch_one(&mut **tx)
        .await?;
        if retained {
            return Err(GoalDbError::DuplicateGoalId);
        }
    }
    if let (control::GoalControlInstallation::Installed, GoalChange::Report { binding, .. }) =
        (policies.control, &mutation.change)
    {
        control::require_current_control_tx(tx, binding.owner(), binding.execution()).await?;
    }
    if let Some(transcript) = expected_transcript {
        match &mutation.change {
            GoalChange::Report { binding, .. } => {
                admission::require_live_report_tx(tx, binding, transcript).await?;
            }
            GoalChange::Create { .. } | GoalChange::SetDisposition { .. } => {
                admission::require_settled_tx(tx, mutation, transcript).await?;
            }
        }
    }
    let current = current_tx(tx, &mutation.owner).await?;
    let next = mutation.advance(current.as_ref())?;
    let (kind, report_turn) = match &mutation.change {
        GoalChange::Create { goal_id, objective } => {
            sqlx::query(
                "INSERT INTO conversation_goal_ids (goal_id,owner_id,objective) VALUES (?,?,?)",
            )
            .bind(goal_id.as_str())
            .bind(mutation.owner.as_str())
            .bind(objective.as_str())
            .execute(&mut **tx)
            .await
            .map_err(|error| {
                if matches!(&error, sqlx::Error::Database(db) if db.is_unique_violation()) {
                    GoalDbError::DuplicateGoalId
                } else {
                    GoalDbError::Sqlx(error)
                }
            })?;
            execution::capture_created_limit_tx(tx, policies.execution, goal_id).await?;
            ("create", None)
        }
        GoalChange::SetDisposition { .. } => ("state", None),
        GoalChange::Report { binding, .. } => {
            if binding_tx(tx, binding.execution().turn_id())
                .await?
                .as_ref()
                != Some(binding)
            {
                return Err(GoalDbError::BindingConflict);
            }
            ("report", Some(binding.execution().turn_id()))
        }
    };
    let requester = insert_actor(
        tx,
        &mutation.owner,
        &mutation.attribution.requested_by,
        next.version(),
    )
    .await?;
    let applier = insert_actor(
        tx,
        &mutation.owner,
        &mutation.attribution.applied_by,
        next.version(),
    )
    .await?;
    let (state, reason) = state_columns(next.state());
    sqlx::query("INSERT INTO conversation_goal_history (owner_id,version,goal_id,state,reason,action_id,change_kind,report_turn_id,requested_actor_id,applied_actor_id,recorded_at_us) VALUES (?,?,?,?,?,?,?,?,?,?,?)")
        .bind(next.owner().as_str()).bind(next.version().get()).bind(next.goal_id().as_str())
        .bind(state).bind(reason).bind(mutation.action_id.as_str()).bind(kind).bind(report_turn)
        .bind(requester).bind(applier).bind(at.get()).execute(&mut **tx).await?;
    Ok(GoalHistoryEntry::new(next, mutation.clone(), at)?)
}

pub(crate) async fn capture_goal_at_new_turn_tx(
    tx: &mut GoalTx<'_>,
    execution: GoalExecution,
) -> std::result::Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO conversation_goal_bindings (turn_id,workflow_id,owner_id,observed_version,goal_id)
         SELECT t.turn_id,t.workflow_id,p.id,h.version,
             CASE WHEN h.state='underway' THEN h.goal_id ELSE NULL END
         FROM durable_turns t JOIN conversations c ON c.id=t.conversation_id
         JOIN product_conversations p ON p.id=c.product_conversation_id
         LEFT JOIN conversation_goal_history h ON h.owner_id=p.id
             AND h.version=(SELECT MAX(version) FROM conversation_goal_history WHERE owner_id=p.id)
         WHERE t.turn_id=? AND t.workflow_id=? AND p.kind='ordinary'",
    ).bind(execution.turn_id()).bind(execution.workflow_id()).execute(&mut **tx).await?;
    Ok(())
}

impl Database {
    /// # Errors
    /// Rejects conflicting action reuse and malformed persisted data or database failures.
    pub async fn accepted_conversation_goal_mutation(
        &self,
        mutation: &GoalMutation,
    ) -> Result<Option<GoalHistoryEntry>> {
        let mut tx = self.pool().begin().await?;
        let accepted = retry_tx(&mut tx, mutation).await?;
        tx.commit().await?;
        Ok(accepted)
    }
    /// # Errors
    /// Rejects busy/changed targets, stale versions, retained IDs, invalid attribution or storage failures.
    pub async fn apply_conversation_goal(
        &self,
        mutation: &GoalMutation,
        expected_transcript: &str,
        at: GoalTimestampUs,
    ) -> Result<GoalHistoryEntry> {
        let mut tx = self.pool().begin_with("BEGIN IMMEDIATE").await?;
        if let Some(previous) = retry_tx(&mut tx, mutation).await? {
            tx.commit().await?;
            return Ok(previous);
        }
        let accepted = apply_goal_tx(
            &mut tx,
            mutation,
            Some(expected_transcript),
            at,
            self.goal_write_policies(),
        )
        .await?;
        if !matches!(&mutation.change, GoalChange::Report { .. }) {
            crate::coordinator_watches::goal_facts::settled_change_tx(
                &mut tx,
                self.goal_control_installation,
                &accepted,
                expected_transcript,
            )
            .await?;
        }
        tx.commit().await?;
        Ok(accepted)
    }
}

impl Database {
    /// # Errors
    /// Returns a database error or rejects malformed persisted goal data.
    pub async fn conversation_goal(
        &self,
        owner: &ProductConversationId,
    ) -> Result<Option<GoalSnapshot>> {
        let mut tx = self.pool().begin().await?;
        let snapshot = current_tx(&mut tx, owner).await?;
        tx.commit().await?;
        Ok(snapshot)
    }

    /// Read the current immutable fact, including its persisted attribution and reason.
    ///
    /// # Errors
    /// Rejects missing or inconsistent current history and malformed persisted data.
    pub async fn conversation_goal_current_entry(
        &self,
        owner: &ProductConversationId,
    ) -> Result<Option<GoalHistoryEntry>> {
        let mut tx = self.pool().begin().await?;
        Self::get_ordinary_product_conversation_on(&mut tx, owner).await?;
        let Some(snapshot) = current_tx(&mut tx, owner).await? else {
            tx.commit().await?;
            return Ok(None);
        };
        let row = sqlx::query("SELECT h.*, g.objective FROM conversation_goal_history h JOIN conversation_goal_ids g USING (goal_id,owner_id) WHERE h.owner_id=? AND h.version=?")
            .bind(owner.as_str())
            .bind(snapshot.version().get())
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(GoalDbError::Integrity("current goal history missing"))?;
        let entry = history_entry(&mut tx, &row).await?;
        if entry.snapshot() != &snapshot {
            return Err(GoalDbError::Integrity("current goal history mismatch"));
        }
        tx.commit().await?;
        Ok(Some(entry))
    }

    /// Returns at most 100 immutable changes, oldest first, after the supplied owner version.
    ///
    /// # Errors
    /// Rejects missing/non-ordinary owners, database errors, or malformed history/actor/binding data.
    pub async fn conversation_goal_history(
        &self,
        owner: &ProductConversationId,
        after: GoalVersion,
    ) -> Result<Vec<GoalHistoryEntry>> {
        self.conversation_goal_history_with_cut(
            owner,
            after,
            #[cfg(test)]
            None,
        )
        .await
    }

    async fn conversation_goal_history_with_cut(
        &self,
        owner: &ProductConversationId,
        after: GoalVersion,
        #[cfg(test)] cut: Option<snapshot_tests::HistoryReadCut>,
    ) -> Result<Vec<GoalHistoryEntry>> {
        let mut tx = self.pool().begin().await?;
        Self::get_ordinary_product_conversation_on(&mut tx, owner).await?;
        #[cfg(test)]
        if let Some(cut) = cut {
            cut.pause().await;
        }
        let entries = history_tx(&mut tx, owner, after).await?;
        tx.commit().await?;
        Ok(entries)
    }

    /// Read the immutable observation for one exact execution; never capture a missing one here.
    ///
    /// # Errors
    /// Rejects a mismatched workflow or malformed persisted binding and database failures.
    pub async fn conversation_goal_execution_binding(
        &self,
        execution: GoalExecution,
    ) -> Result<Option<GoalExecutionBinding>> {
        let mut tx = self.pool().begin().await?;
        let binding = binding_tx(&mut tx, execution.turn_id()).await?;
        if binding
            .as_ref()
            .is_some_and(|binding| binding.execution() != execution)
        {
            return Err(GoalDbError::Integrity("execution binding"));
        }
        tx.commit().await?;
        Ok(binding)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn test_pool() -> sqlx::SqlitePool {
        SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("in-memory SQLite pool")
    }

    async fn migrated_db() -> Database {
        let db = Database::open_in_memory().await.unwrap();
        db
    }

    #[tokio::test]
    async fn public_current_history_reads_use_migrated_database_and_ordinary_eligibility() {
        let db = migrated_db().await;
        let ordinary = db
            .create_conversation("goal-owner", "goal-owner", "/tmp", true, None, None)
            .await
            .unwrap();
        let owner = ordinary.product_conversation_id.clone();
        assert!(db.conversation_goal(&owner).await.unwrap().is_none());
        assert!(db
            .conversation_goal_current_entry(&owner)
            .await
            .unwrap()
            .is_none());
        assert!(db
            .conversation_goal_history(&owner, GoalVersion::INITIAL)
            .await
            .unwrap()
            .is_empty());

        let coordinator = db
            .get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
            .await
            .unwrap();
        assert!(db
            .conversation_goal(&coordinator.product_conversation_id)
            .await
            .unwrap()
            .is_none());
        assert!(db
            .conversation_goal_current_entry(&coordinator.product_conversation_id)
            .await
            .is_err());
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM conversation_goal_ids")
                .fetch_one(db.pool())
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM conversation_goal_history")
                .fetch_one(db.pool())
                .await
                .unwrap(),
            0
        );

        sqlx::query("INSERT INTO conversation_goal_ids(goal_id,objective,owner_id) VALUES ('goal-1','objective',?)")
            .bind(owner.as_str()).execute(db.pool()).await.unwrap();
        sqlx::query("INSERT INTO conversation_goal_actors(actor_id,owner_id,kind,host_action_id) VALUES (1,?,'generated_host','fixture-create')")
            .bind(owner.as_str()).execute(db.pool()).await.unwrap();
        sqlx::query("INSERT INTO conversation_goal_history(owner_id,version,goal_id,state,reason,action_id,change_kind,report_turn_id,requested_actor_id,applied_actor_id,recorded_at_us) VALUES (?,1,'goal-1','underway',NULL,'action-1','create',NULL,1,1,1234)")
            .bind(owner.as_str()).execute(db.pool()).await.unwrap();

        let current = db.conversation_goal(&owner).await.unwrap().unwrap();
        assert_eq!(current.objective().as_str(), "objective");
        let entry = db
            .conversation_goal_current_entry(&owner)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(entry.snapshot(), &current);
        assert_eq!(entry.recorded_at_us().get(), 1234);
        assert!(
            matches!(entry.mutation().attribution.requested_by, GoalActor::GeneratedHost { ref action_id } if action_id.as_str() == "fixture-create")
        );
        assert!(
            matches!(entry.mutation().attribution.applied_by, GoalActor::GeneratedHost { ref action_id } if action_id.as_str() == "fixture-create")
        );
        let history = db
            .conversation_goal_history(&owner, GoalVersion::INITIAL)
            .await
            .unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].snapshot(), &current);
        assert_eq!(history[0].mutation().action_id.as_str(), "action-1");
    }

    async fn seed_execution_turn(
        db: &Database,
        conversation_id: &str,
        workflow_id: i64,
        turn_id: i64,
    ) {
        sqlx::query("INSERT INTO workflows(workflow_id,profile_kind,profile_version,runtime_acceptance_enabled,external_acceptance_enabled,version,generation,status,snapshot_codec_family,snapshot_codec_version,snapshot_payload,created_at,updated_at) VALUES (?, 'direct_turn', 1, 0, 1, 1, 0, 'Active', 'direct_turn.snapshot', 1, X'01', 1, 1)")
            .bind(workflow_id)
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO durable_turns(turn_id,workflow_id,conversation_id,client_turn_key,prepared_fingerprint,prepared_payload,disposition,generation,owns_conversation,origin_kind,canonical_message_id,materialized) VALUES (?, ?, ?, ?, 'goal-test-fingerprint', X'01', 'Runtime', 0, 1, 'user_api', NULL, 0)")
            .bind(turn_id)
            .bind(workflow_id)
            .bind(conversation_id)
            .bind(format!("goal-turn-key-{turn_id}"))
            .execute(db.pool())
            .await
            .unwrap();
    }

    async fn seed_goal(db: &Database, owner: &ProductConversationId, goal_id: &str) {
        sqlx::query("INSERT INTO conversation_goal_ids(goal_id,objective,owner_id) VALUES (?, 'objective', ?)")
            .bind(goal_id)
            .bind(owner.as_str())
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO conversation_goal_actors(actor_id,owner_id,kind,host_action_id) VALUES (1,?,'generated_host','fixture-create')")
            .bind(owner.as_str())
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO conversation_goal_history(owner_id,version,goal_id,state,reason,action_id,change_kind,report_turn_id,requested_actor_id,applied_actor_id,recorded_at_us) VALUES (?,1,?,'underway',NULL,'fixture-create','create',NULL,1,1,1)")
            .bind(owner.as_str())
            .bind(goal_id)
            .execute(db.pool())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn public_binding_reads_saved_goal_and_no_goal_without_capturing_or_writing() {
        let db = migrated_db().await;
        let saved = db
            .create_conversation("goal-saved", "goal-saved", "/tmp", true, None, None)
            .await
            .unwrap();
        let no_goal = db
            .create_conversation("goal-none", "goal-none", "/tmp", true, None, None)
            .await
            .unwrap();
        let saved_owner = saved.product_conversation_id.clone();
        let no_goal_owner = no_goal.product_conversation_id.clone();
        seed_goal(&db, &saved_owner, "saved-goal").await;
        seed_execution_turn(&db, &saved.id, 9001, 7001).await;
        seed_execution_turn(&db, &no_goal.id, 9002, 7002).await;
        sqlx::query("INSERT INTO conversation_goal_bindings(turn_id,workflow_id,owner_id,observed_version,goal_id) VALUES (7001,9001,?,1,'saved-goal'), (7002,9002,?,NULL,NULL)")
            .bind(saved_owner.as_str())
            .bind(no_goal_owner.as_str())
            .execute(db.pool())
            .await
            .unwrap();

        let before =
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM conversation_goal_bindings")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let saved_binding = db
            .conversation_goal_execution_binding(GoalExecution::parse(9001, 7001).unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(saved_binding.owner(), &saved_owner);
        assert_eq!(
            saved_binding.observation().clone(),
            GoalAdmissionObservation::Goal(
                GoalFence::new(
                    saved_owner,
                    GoalId::parse("saved-goal".to_string()).unwrap(),
                    GoalVersion::parse(1).unwrap(),
                )
                .unwrap(),
            )
        );
        let no_goal_binding = db
            .conversation_goal_execution_binding(GoalExecution::parse(9002, 7002).unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(no_goal_binding.owner(), &no_goal_owner);
        assert_eq!(
            no_goal_binding.observation().clone(),
            GoalAdmissionObservation::NoGoal {
                observed_version: GoalVersion::INITIAL
            }
        );
        assert!(matches!(
            db.conversation_goal_execution_binding(GoalExecution::parse(9999, 7001).unwrap())
                .await,
            Err(GoalDbError::Integrity("execution binding"))
        ));
        assert!(db
            .conversation_goal_execution_binding(GoalExecution::parse(9001, 7999).unwrap())
            .await
            .unwrap()
            .is_none());
        let after = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM conversation_goal_bindings")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(before, 2);
        assert_eq!(after, before);
    }

    #[tokio::test]
    async fn public_history_accepts_nonreport_underway_and_suspended_dispositions() {
        let db = migrated_db().await;
        let conversation = db
            .create_conversation(
                "goal-dispositions",
                "goal-dispositions",
                "/tmp",
                true,
                None,
                None,
            )
            .await
            .unwrap();
        let owner = conversation.product_conversation_id;
        seed_goal(&db, &owner, "disposition-goal").await;

        sqlx::query(
            "INSERT INTO conversation_goal_history(owner_id,version,goal_id,state,reason,action_id,change_kind,report_turn_id,requested_actor_id,applied_actor_id,recorded_at_us) VALUES (?,2,'disposition-goal','blocked','waiting','fixture-block','state',NULL,1,1,2), (?,3,'disposition-goal','underway',NULL,'fixture-underway','state',NULL,1,1,3), (?,4,'disposition-goal','suspended','waiting','fixture-suspend','state',NULL,1,1,4)",
        )
            .bind(owner.as_str())
            .bind(owner.as_str())
            .bind(owner.as_str())
            .execute(db.pool())
            .await
            .unwrap();

        let history = db
            .conversation_goal_history(&owner, GoalVersion::INITIAL)
            .await
            .unwrap();
        assert_eq!(history.len(), 4);
        assert!(matches!(
            &history[2].mutation().change,
            GoalChange::SetDisposition {
                disposition: GoalDisposition::Underway,
                ..
            }
        ));
        assert!(matches!(
            &history[3].mutation().change,
            GoalChange::SetDisposition {
                disposition: GoalDisposition::Suspended(reason),
                ..
            } if reason.as_str() == "waiting"
        ));
    }

    #[tokio::test]
    async fn report_history_rejects_nonreport_disposition_in_corrupt_decoder_fixture() {
        // This private schema intentionally omits production CHECK constraints to model a
        // malformed persisted row; it is not a legal canonical-database history fixture.
        let pool = test_pool().await;
        sqlx::query(
            "CREATE TABLE conversation_goal_ids (goal_id TEXT, objective TEXT, owner_id TEXT)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("CREATE TABLE conversation_goal_history (owner_id TEXT, version INTEGER, goal_id TEXT, state TEXT, reason TEXT, action_id TEXT, change_kind TEXT, report_turn_id INTEGER, requested_actor_id INTEGER, applied_actor_id INTEGER, recorded_at_us INTEGER)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("CREATE TABLE conversation_goal_actors (actor_id INTEGER, owner_id TEXT, kind TEXT, host_action_id TEXT)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("CREATE TABLE conversation_goal_bindings (turn_id INTEGER, workflow_id INTEGER, owner_id TEXT, observed_version INTEGER, goal_id TEXT)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO conversation_goal_ids VALUES ('goal','objective','owner')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO conversation_goal_actors VALUES (1,'owner','generated_host','fixture-action')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO conversation_goal_bindings VALUES (7,8,'owner',1,'goal')")
            .execute(&pool)
            .await
            .unwrap();
        let owner = ProductConversationId::parse("owner").unwrap();
        for (state, reason) in [("underway", None), ("suspended", Some("waiting"))] {
            let mut tx = pool.begin().await.unwrap();
            sqlx::query("INSERT INTO conversation_goal_history VALUES ('owner',2,'goal',?,?,'fixture-report','report',7,1,1,2)")
                .bind(state)
                .bind(reason)
                .execute(&mut *tx)
                .await
                .unwrap();
            let error = history_tx(&mut tx, &owner, GoalVersion::INITIAL)
                .await
                .unwrap_err();
            assert!(matches!(
                error,
                GoalDbError::Domain(GoalError::Inapplicable)
            ));
            tx.rollback().await.unwrap();
        }
    }

    #[tokio::test]
    async fn public_history_paginates_legal_history_and_current_entry_matches_latest() {
        let db = migrated_db().await;
        let conversation = db
            .create_conversation("goal-history", "goal-history", "/tmp", true, None, None)
            .await
            .unwrap();
        let owner = conversation.product_conversation_id;
        sqlx::query("INSERT INTO conversation_goal_actors(actor_id,owner_id,kind,host_action_id) VALUES (1,?,'generated_host','fixture-history')")
            .bind(owner.as_str())
            .execute(db.pool())
            .await
            .unwrap();

        let mut version = 0_i64;
        for goal_number in 0..34 {
            let goal_id = format!("history-goal-{goal_number}");
            sqlx::query(
                "INSERT INTO conversation_goal_ids(goal_id,objective,owner_id) VALUES (?, ?, ?)",
            )
            .bind(&goal_id)
            .bind(format!("objective {goal_number}"))
            .bind(owner.as_str())
            .execute(db.pool())
            .await
            .unwrap();
            version += 1;
            sqlx::query("INSERT INTO conversation_goal_history(owner_id,version,goal_id,state,reason,action_id,change_kind,report_turn_id,requested_actor_id,applied_actor_id,recorded_at_us) VALUES (?, ?, ?, 'underway', NULL, ?, 'create', NULL, 1, 1, ?)")
                .bind(owner.as_str())
                .bind(version)
                .bind(&goal_id)
                .bind(format!("history-action-{version}"))
                .bind(version)
                .execute(db.pool())
                .await
                .unwrap();
            for (state, reason) in [("blocked", Some("waiting")), ("underway", None)] {
                version += 1;
                sqlx::query("INSERT INTO conversation_goal_history(owner_id,version,goal_id,state,reason,action_id,change_kind,report_turn_id,requested_actor_id,applied_actor_id,recorded_at_us) VALUES (?, ?, ?, ?, ?, ?, 'state', NULL, 1, 1, ?)")
                    .bind(owner.as_str())
                    .bind(version)
                    .bind(&goal_id)
                    .bind(state)
                    .bind(reason)
                    .bind(format!("history-action-{version}"))
                    .bind(version)
                    .execute(db.pool())
                    .await
                    .unwrap();
            }
        }
        assert_eq!(version, 102);

        let first_page = db
            .conversation_goal_history(&owner, GoalVersion::INITIAL)
            .await
            .unwrap();
        assert_eq!(first_page.len(), 100);
        assert_eq!(first_page.first().unwrap().snapshot().version().get(), 1);
        assert_eq!(first_page.last().unwrap().snapshot().version().get(), 100);
        let second_page = db
            .conversation_goal_history(&owner, GoalVersion::parse(100).unwrap())
            .await
            .unwrap();
        assert_eq!(second_page.len(), 2);
        assert_eq!(second_page[0].snapshot().version().get(), 101);
        assert_eq!(second_page[1].snapshot().version().get(), 102);

        let current = db
            .conversation_goal_current_entry(&owner)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(current.snapshot(), second_page.last().unwrap().snapshot());
        assert_eq!(current, second_page[1]);
    }

    #[tokio::test]
    async fn public_binding_read_of_absent_execution_does_not_capture_or_write() {
        let db = migrated_db().await;
        let before =
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM conversation_goal_bindings")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert!(db
            .conversation_goal_execution_binding(GoalExecution::parse(9001, 7001).unwrap())
            .await
            .unwrap()
            .is_none());
        let after = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM conversation_goal_bindings")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(before, after);
        assert_eq!(after, 0);
    }

    #[tokio::test]
    async fn state_decoder_requires_consistent_kind_and_nonblank_reason() {
        let pool = test_pool().await;
        let row = sqlx::query("SELECT 'underway' AS state, NULL AS reason")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(read_state(&row).unwrap(), GoalState::Underway);

        let row = sqlx::query("SELECT 'blocked' AS state, ' waiting ' AS reason")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            read_state(&row).unwrap(),
            GoalState::Blocked(GoalText::parse(" waiting ").unwrap())
        );

        for (state, reason) in [("underway", "why"), ("blocked", " "), ("unknown", "why")] {
            let row = sqlx::query("SELECT ? AS state, ? AS reason")
                .bind(state)
                .bind(reason)
                .fetch_one(&pool)
                .await
                .unwrap();
            assert!(
                read_state(&row).is_err(),
                "accepted {state:?} with {reason:?}"
            );
        }
    }

    #[tokio::test]
    async fn snapshot_decoder_rejects_blank_objective_and_invalid_state() {
        let pool = test_pool().await;
        let row = sqlx::query("SELECT 'owner' AS owner_id, 'goal' AS goal_id, 1 AS version, ' ' AS objective, 'underway' AS state, NULL AS reason")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(matches!(
            snapshot(&row),
            Err(GoalDbError::Domain(GoalError::Blank))
        ));

        let row = sqlx::query("SELECT 'owner' AS owner_id, 'goal' AS goal_id, 1 AS version, 'objective' AS objective, 'underway' AS state, 'reason' AS reason")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(matches!(
            snapshot(&row),
            Err(GoalDbError::Integrity("state/reason"))
        ));
    }

    #[tokio::test]
    async fn current_and_history_read_absent_and_persisted_facts() {
        let pool = test_pool().await;
        sqlx::query(
            "CREATE TABLE conversation_goal_ids (owner_id TEXT, goal_id TEXT, objective TEXT)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("CREATE TABLE conversation_goal_history (owner_id TEXT, version INTEGER, goal_id TEXT, state TEXT, reason TEXT, change_kind TEXT, action_id TEXT, requested_actor_id INTEGER, applied_actor_id INTEGER, recorded_at_us INTEGER, report_turn_id INTEGER)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("CREATE TABLE conversation_goal_actors (actor_id INTEGER, owner_id TEXT, kind TEXT, source_product_id TEXT, transcript_id TEXT, workflow_id INTEGER, turn_id INTEGER, message_id TEXT, tool_use_id TEXT, host_action_id TEXT, control_principal_id BLOB, control_request_id BLOB, control_command TEXT)")
            .execute(&pool)
            .await
            .unwrap();

        let owner = ProductConversationId::parse("owner").unwrap();
        let mut tx = pool.begin().await.unwrap();
        assert!(current_tx(&mut tx, &owner).await.unwrap().is_none());
        assert!(history_tx(&mut tx, &owner, GoalVersion::INITIAL)
            .await
            .unwrap()
            .is_empty());

        sqlx::query("INSERT INTO conversation_goal_ids VALUES ('owner', 'goal', 'objective')")
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("INSERT INTO conversation_goal_actors VALUES (1, 'owner', 'generated_host', NULL, NULL, NULL, NULL, NULL, NULL, 'host-action', NULL, NULL, NULL)")
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("INSERT INTO conversation_goal_history VALUES ('owner', 1, 'goal', 'underway', NULL, 'create', 'action', 1, 1, 100, NULL)")
            .execute(&mut *tx)
            .await
            .unwrap();

        let current = current_tx(&mut tx, &owner).await.unwrap().unwrap();
        assert_eq!(current.objective().as_str(), "objective");
        let history = history_tx(&mut tx, &owner, GoalVersion::INITIAL)
            .await
            .unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].snapshot(), &current);
        assert_eq!(history[0].recorded_at_us().get(), 100);
        tx.rollback().await.unwrap();
    }

    #[tokio::test]
    async fn history_reads_are_oldest_first_and_capped_at_one_hundred() {
        let pool = test_pool().await;
        sqlx::query(
            "CREATE TABLE conversation_goal_ids (goal_id TEXT, owner_id TEXT, objective TEXT)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("CREATE TABLE conversation_goal_history (owner_id TEXT, version INTEGER, goal_id TEXT, state TEXT, reason TEXT, change_kind TEXT, action_id TEXT, requested_actor_id INTEGER, applied_actor_id INTEGER, recorded_at_us INTEGER, report_turn_id INTEGER)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("CREATE TABLE conversation_goal_actors (actor_id INTEGER, owner_id TEXT, kind TEXT, source_product_id TEXT, transcript_id TEXT, workflow_id INTEGER, turn_id INTEGER, message_id TEXT, tool_use_id TEXT, host_action_id TEXT, control_principal_id BLOB, control_request_id BLOB, control_command TEXT)")
            .execute(&pool)
            .await
            .unwrap();

        sqlx::query("INSERT INTO conversation_goal_ids VALUES ('goal', 'owner', 'objective')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO conversation_goal_actors(actor_id,owner_id,kind,host_action_id) VALUES (1, 'owner', 'generated_host', 'host-action')")
            .execute(&pool)
            .await
            .unwrap();
        // Private decoder LIMIT/order fixture, not a canonical aggregate journey.
        // The public pagination test above covers legal retained history.
        for version in 1..=101_i64 {
            sqlx::query("INSERT INTO conversation_goal_history VALUES ('owner', ?, 'goal', 'underway', NULL, 'create', ?, 1, 1, ?, NULL)")
                .bind(version)
                .bind(format!("action-{version}"))
                .bind(version)
                .execute(&pool)
                .await
                .unwrap();
        }

        let owner = ProductConversationId::parse("owner").unwrap();
        let mut tx = pool.begin().await.unwrap();
        let entries = history_tx(&mut tx, &owner, GoalVersion::INITIAL)
            .await
            .unwrap();
        assert_eq!(entries.len(), 100);
        assert_eq!(entries.first().unwrap().snapshot().version().get(), 1);
        assert_eq!(entries.last().unwrap().snapshot().version().get(), 100);
        tx.rollback().await.unwrap();
    }

    #[tokio::test]
    async fn binding_decoder_reads_no_goal_and_goal_observations_and_rejects_invalid_rows() {
        let pool = test_pool().await;
        sqlx::query("CREATE TABLE conversation_goal_bindings (turn_id INTEGER, owner_id TEXT, workflow_id INTEGER, goal_id TEXT, observed_version INTEGER)")
            .execute(&pool)
            .await
            .unwrap();
        let mut tx = pool.begin().await.unwrap();

        assert!(binding_tx(&mut tx, 10).await.unwrap().is_none());
        sqlx::query("INSERT INTO conversation_goal_bindings VALUES (10, 'owner', 3, NULL, 0), (11, 'owner', 3, 'goal', 2)")
            .execute(&mut *tx)
            .await
            .unwrap();
        let no_goal = binding_tx(&mut tx, 10).await.unwrap().unwrap();
        assert_eq!(no_goal.owner().as_str(), "owner");
        assert!(
            matches!(no_goal.observation(), GoalAdmissionObservation::NoGoal { observed_version } if observed_version.get() == 0)
        );
        let goal = binding_tx(&mut tx, 11).await.unwrap().unwrap();
        assert!(
            matches!(goal.observation(), GoalAdmissionObservation::Goal(fence) if fence.version().get() == 2)
        );

        sqlx::query("INSERT INTO conversation_goal_bindings VALUES (12, 'owner', 3, NULL, -1), (13, ' ', 3, NULL, 0)")
            .execute(&mut *tx)
            .await
            .unwrap();
        assert!(matches!(
            binding_tx(&mut tx, 12).await,
            Err(GoalDbError::Domain(GoalError::Range))
        ));
        assert!(matches!(
            binding_tx(&mut tx, 13).await,
            Err(GoalDbError::Integrity("product id"))
        ));
        sqlx::query("INSERT INTO conversation_goal_bindings VALUES (14, 'owner', 3, 'goal', 0)")
            .execute(&mut *tx)
            .await
            .unwrap();
        assert!(matches!(
            binding_tx(&mut tx, 14).await,
            Err(GoalDbError::Domain(GoalError::Range))
        ));
        tx.rollback().await.unwrap();
    }
}
