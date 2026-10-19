//! Dormant goal-specific persistence. No runtime, HTTP or tool caller is installed.

use phoenix_core::domain::conversation_goal::{
    GoalActor, GoalAdmissionObservation, GoalAgentSource, GoalAttribution, GoalChange,
    GoalDisposition, GoalError, GoalExecution, GoalExecutionBinding, GoalFence, GoalHistoryEntry,
    GoalId, GoalMutation, GoalSnapshot, GoalState, GoalText, GoalTimestampUs, GoalVersion,
};
use phoenix_core::domain::product_conversation::ProductConversationId;
use sqlx::{sqlite::SqliteRow, Row, Sqlite, Transaction};

use crate::Database;

type GoalTx<'a> = Transaction<'a, Sqlite>;

#[derive(Debug, thiserror::Error)]
pub enum GoalDbError {
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
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

    /// Internal persistence only. Trusted callers supply actors; this is not an authorization API.
    ///
    /// # Errors
    /// Rejects stale/conflicting changes, invalid attribution or binding, and database failures.
    pub async fn mutate_conversation_goal(
        &self,
        mutation: &GoalMutation,
        at: GoalTimestampUs,
    ) -> Result<GoalHistoryEntry> {
        let mut tx = self.pool().begin_with("BEGIN IMMEDIATE").await?;
        if let Some(row) =
            sqlx::query("SELECT h.*, g.objective FROM conversation_goal_history h JOIN conversation_goal_ids g USING (goal_id,owner_id) WHERE owner_id=? AND action_id=?")
                .bind(mutation.owner.as_str())
                .bind(mutation.action_id.as_str())
                .fetch_optional(&mut *tx)
                .await?
        {
            let previous = history_entry(&mut tx, &row).await?;
            if previous.mutation != *mutation {
                return Err(GoalDbError::RetryConflict);
            }
            tx.commit().await?;
            return Ok(previous);
        }
        let current = current_tx(&mut tx, &mutation.owner).await?;
        let next = mutation.advance(current.as_ref())?;
        let (kind, report_turn) = match &mutation.change {
            GoalChange::Create { goal_id, objective } => {
                sqlx::query(
                    "INSERT INTO conversation_goal_ids (goal_id,owner_id,objective) VALUES (?,?,?)",
                )
                .bind(goal_id.as_str())
                .bind(mutation.owner.as_str())
                .bind(objective.as_str())
                .execute(&mut *tx)
                .await?;
                ("create", None)
            }
            GoalChange::SetDisposition { .. } => ("state", None),
            GoalChange::Report { binding, .. } => {
                if binding_tx(&mut tx, binding.execution.turn_id())
                    .await?
                    .as_ref()
                    != Some(binding)
                {
                    return Err(GoalDbError::BindingConflict);
                }
                ("report", Some(binding.execution.turn_id()))
            }
        };
        let requester =
            insert_actor(&mut tx, &mutation.owner, &mutation.attribution.requested_by).await?;
        let applier =
            insert_actor(&mut tx, &mutation.owner, &mutation.attribution.applied_by).await?;
        let (state, reason) = state_columns(&next.state);
        sqlx::query("INSERT INTO conversation_goal_history (owner_id,version,goal_id,state,reason,action_id,change_kind,report_turn_id,requested_actor_id,applied_actor_id,recorded_at_us) VALUES (?,?,?,?,?,?,?,?,?,?,?)")
            .bind(next.owner.as_str()).bind(next.version.get()).bind(next.goal_id.as_str())
            .bind(state).bind(reason)
            .bind(mutation.action_id.as_str()).bind(kind).bind(report_turn).bind(requester).bind(applier)
            .bind(at.get()).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(GoalHistoryEntry {
            snapshot: next,
            mutation: mutation.clone(),
            recorded_at_us: at,
        })
    }
}

/// Dormant storage primitive; this cannot prove transaction co-creation of its turn.
/// Atomic capture by the actual admission owner is not installed here.
#[allow(dead_code)]
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
