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

#[derive(Debug, thiserror::Error)]
pub enum GoalDbError {
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
    Ok(GoalSnapshot {
        owner: product(row.try_get("owner_id")?)?,
        goal_id: GoalId::parse(row.try_get::<String, _>("goal_id")?)?,
        version: GoalVersion::parse(row.try_get("version")?)?,
        objective: text(row.try_get("objective")?)?,
        state: read_state(row)?,
    })
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

    /// Read the current immutable fact, including its persisted attribution and reason.
    ///
    /// # Errors
    /// Rejects missing or inconsistent current history and malformed persisted data.
    pub async fn conversation_goal_current_entry(
        &self,
        owner: &ProductConversationId,
    ) -> Result<Option<GoalHistoryEntry>> {
        self.get_ordinary_product_conversation(owner).await?;
        let mut tx = self.pool().begin().await?;
        let Some(snapshot) = current_tx(&mut tx, owner).await? else {
            tx.commit().await?;
            return Ok(None);
        };
        let row = sqlx::query("SELECT h.*, g.objective FROM conversation_goal_history h JOIN conversation_goal_ids g USING (goal_id,owner_id) WHERE h.owner_id=? AND h.version=?")
            .bind(owner.as_str())
            .bind(snapshot.version.get())
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(GoalDbError::Integrity("current goal history missing"))?;
        let entry = history_entry(&mut tx, &row).await?;
        if entry.snapshot != snapshot {
            return Err(GoalDbError::Integrity("current goal history mismatch"));
        }
        tx.commit().await?;
        Ok(Some(entry))
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
            .is_some_and(|binding| binding.execution != execution)
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
        assert_eq!(read_state(&row).unwrap(), GoalState::Blocked(GoalText::parse(" waiting ").unwrap()));

        for (state, reason) in [("underway", "why"), ("blocked", " "), ("unknown", "why")] {
            let row = sqlx::query("SELECT ? AS state, ? AS reason")
                .bind(state)
                .bind(reason)
                .fetch_one(&pool)
                .await
                .unwrap();
            assert!(read_state(&row).is_err(), "accepted {state:?} with {reason:?}");
        }
    }

    #[tokio::test]
    async fn snapshot_decoder_rejects_blank_objective_and_invalid_state() {
        let pool = test_pool().await;
        let row = sqlx::query("SELECT 'owner' AS owner_id, 'goal' AS goal_id, 1 AS version, ' ' AS objective, 'underway' AS state, NULL AS reason")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(matches!(snapshot(&row), Err(GoalDbError::Domain(GoalError::Blank))));

        let row = sqlx::query("SELECT 'owner' AS owner_id, 'goal' AS goal_id, 1 AS version, 'objective' AS objective, 'underway' AS state, 'reason' AS reason")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(matches!(snapshot(&row), Err(GoalDbError::Integrity("state/reason"))));
    }

    #[tokio::test]
    async fn current_and_history_read_absent_and_persisted_facts() {
        let pool = test_pool().await;
        sqlx::query("CREATE TABLE conversation_goal_ids (owner_id TEXT, goal_id TEXT, objective TEXT)")
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
        assert!(history_tx(&mut tx, &owner, GoalVersion::INITIAL).await.unwrap().is_empty());

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
        assert_eq!(current.objective.as_str(), "objective");
        let history = history_tx(&mut tx, &owner, GoalVersion::INITIAL).await.unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].snapshot, current);
        assert_eq!(history[0].recorded_at_us.get(), 100);
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
        assert_eq!(no_goal.owner.as_str(), "owner");
        assert!(matches!(no_goal.observation, GoalAdmissionObservation::NoGoal { observed_version } if observed_version.get() == 0));
        let goal = binding_tx(&mut tx, 11).await.unwrap().unwrap();
        assert!(matches!(goal.observation, GoalAdmissionObservation::Goal(GoalFence { version, .. }) if version.get() == 2));

        sqlx::query("INSERT INTO conversation_goal_bindings VALUES (12, 'owner', 3, NULL, -1), (13, ' ', 3, NULL, 0)")
            .execute(&mut *tx)
            .await
            .unwrap();
        assert!(matches!(binding_tx(&mut tx, 12).await, Err(GoalDbError::Domain(GoalError::Range))));
        assert!(matches!(binding_tx(&mut tx, 13).await, Err(GoalDbError::Integrity("product id"))));
        tx.rollback().await.unwrap();
    }
}
