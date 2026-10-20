//! Goal-specific persistence and owning-transaction checkpoint operations.

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
pub(crate) mod checkpoint;
pub mod control;
pub mod round;

#[derive(Debug, thiserror::Error)]
pub enum GoalDbError {
    #[error(transparent)]
    Admission(#[from] crate::DbError),
    #[error("goal mutation requires a settled turn boundary")]
    Busy,
    #[error("goal owner or current transcript changed during admission")]
    TargetChanged,
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
    #[error(transparent)]
    Control(#[from] control::ControlError),
    #[error(transparent)]
    Domain(#[from] GoalError),
    #[error("goal action identity was already used for another mutation")]
    RetryConflict,
    #[error("goal admission binding conflicts with its first observation")]
    BindingConflict,
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

fn state_columns(state: &GoalState) -> (&'static str, Option<&str>) {
    match state {
        GoalState::Underway => ("underway", None),
        GoalState::Blocked(reason) => ("blocked", Some(reason.as_str())),
        GoalState::Suspended(reason) => ("suspended", Some(reason.as_str())),
        GoalState::Complete(reason) => ("complete", Some(reason.as_str())),
        GoalState::Exhausted(reason) => ("exhausted", Some(reason.as_str())),
    }
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
    Ok(GoalSnapshot {
        owner: product(row.try_get("owner_id")?)?,
        goal_id: GoalId::parse(row.try_get::<String, _>("goal_id")?)?,
        version: GoalVersion::parse(row.try_get("version")?)?,
        objective: text(row.try_get("objective")?)?,
        state: read_state(row)?,
    })
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
        Some(goal_id) => GoalAdmissionObservation::Goal(GoalFence {
            owner: owner.clone(),
            goal_id: GoalId::parse(goal_id)?,
            version,
        }),
        None => GoalAdmissionObservation::NoGoal {
            observed_version: version,
        },
    };
    Ok(Some(GoalExecutionBinding {
        owner,
        execution: GoalExecution::parse(row.try_get("workflow_id")?, turn_id)?,
        observation,
    }))
}

async fn read_actor(tx: &mut GoalTx<'_>, actor_id: i64) -> Result<GoalActor> {
    let row = sqlx::query("SELECT * FROM conversation_goal_actors WHERE actor_id=?")
        .bind(actor_id)
        .fetch_one(&mut **tx)
        .await?;
    let kind: String = row.try_get("kind")?;
    match kind.as_str() {
        "user_control" => Ok(GoalActor::UserControl {
            principal_id: uuid::Uuid::from_slice(
                &row.try_get::<Vec<u8>, _>("control_principal_id")?,
            )
            .map_err(|_| GoalDbError::Integrity("goal control principal identity"))?,
            request_id: uuid::Uuid::from_slice(&row.try_get::<Vec<u8>, _>("control_request_id")?)
                .map_err(|_| GoalDbError::Integrity("goal control request identity"))?,
            command: match row.try_get::<String, _>("control_command")?.as_str() {
                "create" => GoalUserCommandKind::Create,
                "state" => GoalUserCommandKind::State,
                _ => return Err(GoalDbError::Integrity("goal control command kind")),
            },
        }),
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

async fn history_entry(tx: &mut GoalTx<'_>, row: &SqliteRow) -> Result<GoalHistoryEntry> {
    let snapshot = snapshot(row)?;
    let kind: String = row.try_get("change_kind")?;
    let change = match kind.as_str() {
        "create" => GoalChange::Create {
            goal_id: snapshot.goal_id.clone(),
            objective: snapshot.objective.clone(),
        },
        "state" => GoalChange::SetDisposition {
            goal_id: snapshot.goal_id.clone(),
            disposition: disposition(&snapshot.state),
        },
        "report" => GoalChange::Report {
            binding: binding_tx(tx, row.try_get("report_turn_id")?)
                .await?
                .ok_or(GoalDbError::Integrity("report binding"))?,
            disposition: disposition(&snapshot.state),
        },
        _ => return Err(GoalDbError::Integrity("change kind")),
    };
    Ok(GoalHistoryEntry {
        mutation: GoalMutation {
            owner: snapshot.owner.clone(),
            expected_version: GoalVersion::parse(snapshot.version.get() - 1)?,
            action_id: text(row.try_get("action_id")?)?,
            attribution: GoalAttribution {
                requested_by: read_actor(tx, row.try_get("requested_actor_id")?).await?,
                applied_by: read_actor(tx, row.try_get("applied_actor_id")?).await?,
            },
            change,
        },
        snapshot,
        recorded_at_us: GoalTimestampUs::parse(row.try_get("recorded_at_us")?)?,
    })
}

async fn retry_tx(
    tx: &mut GoalTx<'_>,
    mutation: &GoalMutation,
) -> Result<Option<GoalHistoryEntry>> {
    if let Some(row) =
        sqlx::query("SELECT h.*, g.objective FROM conversation_goal_history h JOIN conversation_goal_ids g USING (goal_id,owner_id) WHERE owner_id=? AND action_id=?")
            .bind(mutation.owner.as_str())
            .bind(mutation.action_id.as_str())
            .fetch_optional(&mut **tx)
            .await?
    {
        let previous = history_entry(tx, &row).await?;
        if previous.mutation != *mutation {
            return Err(GoalDbError::RetryConflict);
        }
        return Ok(Some(previous));
    }
    Ok(None)
}

async fn apply_goal_tx(
    tx: &mut GoalTx<'_>,
    mutation: &GoalMutation,
    expected_transcript: Option<&str>,
    at: GoalTimestampUs,
    control_installation: control::GoalControlInstallation,
) -> Result<GoalHistoryEntry> {
    if let Some(previous) = retry_tx(tx, mutation).await? {
        return Ok(previous);
    }
    if let (control::GoalControlInstallation::Installed, GoalChange::Report { binding, .. }) =
        (control_installation, &mutation.change)
    {
        control::require_current_control_tx(tx, &binding.owner, binding.execution).await?;
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
            .await?;
            ("create", None)
        }
        GoalChange::SetDisposition { .. } => ("state", None),
        GoalChange::Report { binding, .. } => {
            if binding_tx(tx, binding.execution.turn_id()).await?.as_ref() != Some(binding) {
                return Err(GoalDbError::BindingConflict);
            }
            ("report", Some(binding.execution.turn_id()))
        }
    };
    let requester = insert_actor(
        tx,
        &mutation.owner,
        &mutation.attribution.requested_by,
        next.version,
    )
    .await?;
    let applier = insert_actor(
        tx,
        &mutation.owner,
        &mutation.attribution.applied_by,
        next.version,
    )
    .await?;
    let (state, reason) = state_columns(&next.state);
    sqlx::query("INSERT INTO conversation_goal_history (owner_id,version,goal_id,state,reason,action_id,change_kind,report_turn_id,requested_actor_id,applied_actor_id,recorded_at_us) VALUES (?,?,?,?,?,?,?,?,?,?,?)")
        .bind(next.owner.as_str()).bind(next.version.get()).bind(next.goal_id.as_str())
        .bind(state).bind(reason)
        .bind(mutation.action_id.as_str()).bind(kind).bind(report_turn).bind(requester).bind(applier)
        .bind(at.get()).execute(&mut **tx).await?;
    Ok(GoalHistoryEntry {
        snapshot: next,
        mutation: mutation.clone(),
        recorded_at_us: at,
    })
}

impl Database {
    /// Read an exact accepted retry without admitting another mutation.
    ///
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

    /// Returns at most 100 immutable changes, oldest first, after the supplied owner version.
    ///
    /// # Errors
    /// Returns a database error or rejects malformed persisted history/actor/binding data.
    pub async fn conversation_goal_history(
        &self,
        owner: &ProductConversationId,
        after: GoalVersion,
    ) -> Result<Vec<GoalHistoryEntry>> {
        let mut tx = self.pool().begin().await?;
        let rows = sqlx::query("SELECT h.*, g.objective FROM conversation_goal_history h JOIN conversation_goal_ids g USING (goal_id,owner_id) WHERE owner_id=? AND version>? ORDER BY version LIMIT 100")
            .bind(owner.as_str()).bind(after.get()).fetch_all(&mut *tx).await?;
        let mut entries = Vec::with_capacity(rows.len());
        for row in &rows {
            entries.push(history_entry(&mut tx, row).await?);
        }
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
            .is_some_and(|binding| binding.execution != execution)
        {
            return Err(GoalDbError::BindingConflict);
        }
        tx.commit().await?;
        Ok(binding)
    }

    /// Apply a trusted host command after the application holds product-message then
    /// transcript-acceptance gates, or an exact in-flight report without those wait gates.
    /// No public adapter is installed by this method.
    ///
    /// # Errors
    /// Rejects busy or changed targets, stale versions, invalid attribution/bindings and DB failures.
    pub async fn apply_conversation_goal(
        &self,
        mutation: &GoalMutation,
        expected_transcript: &str,
        at: GoalTimestampUs,
    ) -> Result<GoalHistoryEntry> {
        self.mutate_goal_with_admission(mutation, Some(expected_transcript), at)
            .await
    }

    #[cfg(test)]
    async fn mutate_conversation_goal(
        &self,
        mutation: &GoalMutation,
        at: GoalTimestampUs,
    ) -> Result<GoalHistoryEntry> {
        self.mutate_goal_with_admission(mutation, None, at).await
    }

    async fn mutate_goal_with_admission(
        &self,
        mutation: &GoalMutation,
        expected_transcript: Option<&str>,
        at: GoalTimestampUs,
    ) -> Result<GoalHistoryEntry> {
        let mut tx = self.pool().begin_with("BEGIN IMMEDIATE").await?;
        let accepted = apply_goal_tx(
            &mut tx,
            mutation,
            expected_transcript,
            at,
            self.goal_control_installation,
        )
        .await?;
        tx.commit().await?;
        Ok(accepted)
    }
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
    )
    .bind(execution.turn_id()).bind(execution.workflow_id())
    .execute(&mut **tx).await?;
    Ok(())
}

#[cfg(test)]
pub(crate) async fn bind_goal_at_admission_tx(
    tx: &mut GoalTx<'_>,
    binding: &GoalExecutionBinding,
) -> Result<()> {
    if let Some(existing) = binding_tx(tx, binding.execution.turn_id()).await? {
        return if existing == *binding {
            Ok(())
        } else {
            Err(GoalDbError::BindingConflict)
        };
    }
    let (observed_version, goal_id) = match &binding.observation {
        GoalAdmissionObservation::NoGoal { observed_version } => (*observed_version, None),
        GoalAdmissionObservation::Goal(fence) => {
            if fence.owner != binding.owner {
                return Err(GoalDbError::BindingConflict);
            }
            (fence.version, Some(fence.goal_id.as_str()))
        }
    };
    sqlx::query("INSERT INTO conversation_goal_bindings (turn_id,workflow_id,owner_id,observed_version,goal_id) VALUES (?,?,?,?,?)")
        .bind(binding.execution.turn_id()).bind(binding.execution.workflow_id()).bind(binding.owner.as_str())
        .bind(if observed_version == GoalVersion::INITIAL { None } else { Some(observed_version.get()) })
        .bind(goal_id).execute(&mut **tx).await?;
    Ok(())
}

#[cfg(test)]
mod tests;
