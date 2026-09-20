use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubAgentExecutionAuthority {
    ReadOnly,
    WriteCapable,
}

impl SubAgentExecutionAuthority {
    const fn as_db_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read_only",
            Self::WriteCapable => "write_capable",
        }
    }

    fn from_db_str(value: &str) -> DbResult<Self> {
        match value {
            "read_only" => Ok(Self::ReadOnly),
            "write_capable" => Ok(Self::WriteCapable),
            other => Err(DbError::Serialization(format!(
                "unknown sub-agent execution authority {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubAgentTerminalCause {
    SubmitResult,
    SubmitError,
    TimedOut,
    Cancelled,
    TurnLimit,
    ImplicitCompletion,
    RuntimeFailure,
    ContextExhausted,
}

impl SubAgentTerminalCause {
    const fn as_db_str(self) -> &'static str {
        match self {
            Self::SubmitResult => "submit_result",
            Self::SubmitError => "submit_error",
            Self::TimedOut => "timed_out",
            Self::Cancelled => "cancelled",
            Self::TurnLimit => "turn_limit",
            Self::ImplicitCompletion => "implicit_completion",
            Self::RuntimeFailure => "runtime_failure",
            Self::ContextExhausted => "context_exhausted",
        }
    }

    fn from_db_str(value: &str) -> DbResult<Self> {
        match value {
            "submit_result" => Ok(Self::SubmitResult),
            "submit_error" => Ok(Self::SubmitError),
            "timed_out" => Ok(Self::TimedOut),
            "cancelled" => Ok(Self::Cancelled),
            "turn_limit" => Ok(Self::TurnLimit),
            "implicit_completion" => Ok(Self::ImplicitCompletion),
            "runtime_failure" => Ok(Self::RuntimeFailure),
            "context_exhausted" => Ok(Self::ContextExhausted),
            other => Err(DbError::Serialization(format!(
                "unknown sub-agent terminal cause {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubAgentRunAdmission {
    pub child_conversation_id: String,
    pub execution_authority: SubAgentExecutionAuthority,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubAgentBatchAdmission {
    pub batch_id: String,
    pub parent_conversation_id: String,
    pub parallel_work_qualified: bool,
    pub runs: Vec<SubAgentRunAdmission>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubAgentBatchAdmissionOutcome {
    Admitted,
    Replayed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubAgentInitialDispatchOutcome {
    Claimed,
    AlreadyClaimed,
    CancelledBeforeDispatch,
    AlreadyTerminal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubAgentCancellationOutcome {
    CancelledBeforeDispatch,
    DeliverToRuntime,
    AlreadyTerminal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubAgentTerminalOutcome {
    Recorded,
    Replayed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubAgentParentAcceptanceOutcome {
    Accepted,
    Replayed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwedSubAgentTerminal {
    pub parent_conversation_id: String,
    pub child_conversation_id: String,
    pub cause: SubAgentTerminalCause,
    pub terminal_at: DateTime<Utc>,
}

fn unix_micros_to_datetime(value: i64) -> DbResult<DateTime<Utc>> {
    DateTime::<Utc>::from_timestamp_micros(value).ok_or_else(|| {
        DbError::Serialization(format!("invalid sub-agent lifecycle timestamp {value}"))
    })
}

fn lifecycle_conflict(message: impl Into<String>) -> DbError {
    DbError::SubAgentLifecycleConflict(message.into())
}

fn validate_batch_input(batch: &SubAgentBatchAdmission) -> DbResult<()> {
    if batch.batch_id.trim().is_empty() || batch.parent_conversation_id.trim().is_empty() {
        return Err(lifecycle_conflict("batch and parent ids must be non-empty"));
    }
    if batch.runs.is_empty() {
        return Err(lifecycle_conflict(
            "a sub-agent batch must contain at least one run",
        ));
    }
    let mut child_ids = std::collections::HashSet::with_capacity(batch.runs.len());
    for run in &batch.runs {
        if run.child_conversation_id.trim().is_empty() {
            return Err(lifecycle_conflict(
                "child conversation id must be non-empty",
            ));
        }
        if !child_ids.insert(run.child_conversation_id.as_str()) {
            return Err(lifecycle_conflict(format!(
                "duplicate child conversation {}",
                run.child_conversation_id
            )));
        }
    }
    if !batch.parallel_work_qualified
        && batch
            .runs
            .iter()
            .filter(|run| run.execution_authority == SubAgentExecutionAuthority::WriteCapable)
            .count()
            > 1
    {
        return Err(lifecycle_conflict(
            "unqualified parent cannot admit multiple write-capable runs",
        ));
    }
    Ok(())
}

async fn existing_batch_matches(
    tx: &mut Transaction<'_, Sqlite>,
    batch: &SubAgentBatchAdmission,
) -> DbResult<Option<bool>> {
    let Some(header) = sqlx::query(
        "SELECT parent_conversation_id, parallel_work_qualified
         FROM sub_agent_batches WHERE batch_id = ?1",
    )
    .bind(&batch.batch_id)
    .fetch_optional(&mut **tx)
    .await?
    else {
        return Ok(None);
    };
    if header.try_get::<String, _>("parent_conversation_id")? != batch.parent_conversation_id
        || header.try_get::<bool, _>("parallel_work_qualified")? != batch.parallel_work_qualified
    {
        return Ok(Some(false));
    }
    let rows = sqlx::query(
        "SELECT child_conversation_id, execution_authority
         FROM sub_agent_runs WHERE batch_id = ?1 ORDER BY ordinal",
    )
    .bind(&batch.batch_id)
    .fetch_all(&mut **tx)
    .await?;
    if rows.len() != batch.runs.len() {
        return Ok(Some(false));
    }
    for (row, expected) in rows.iter().zip(&batch.runs) {
        if row.try_get::<String, _>("child_conversation_id")? != expected.child_conversation_id
            || SubAgentExecutionAuthority::from_db_str(
                &row.try_get::<String, _>("execution_authority")?,
            )? != expected.execution_authority
        {
            return Ok(Some(false));
        }
    }
    Ok(Some(true))
}

#[allow(clippy::missing_errors_doc)]
impl Database {
    pub async fn admit_sub_agent_batch(
        &self,
        batch: &SubAgentBatchAdmission,
    ) -> DbResult<SubAgentBatchAdmissionOutcome> {
        validate_batch_input(batch)?;
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        if let Some(matches) = existing_batch_matches(&mut tx, batch).await? {
            tx.commit().await?;
            return if matches {
                Ok(SubAgentBatchAdmissionOutcome::Replayed)
            } else {
                Err(lifecycle_conflict(format!(
                    "batch id {} was already admitted with different membership",
                    batch.batch_id
                )))
            };
        }

        let admitted_at = Utc::now().timestamp_micros();
        sqlx::query(
            "INSERT INTO sub_agent_batches (
                 batch_id, parent_conversation_id, parallel_work_qualified,
                 admitted_at_unix_micros
             ) VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(&batch.batch_id)
        .bind(&batch.parent_conversation_id)
        .bind(batch.parallel_work_qualified)
        .bind(admitted_at)
        .execute(&mut *tx)
        .await?;

        for (ordinal, run) in batch.runs.iter().enumerate() {
            let ordinal = i64::try_from(ordinal)
                .map_err(|_| lifecycle_conflict("sub-agent ordinal exceeds i64"))?;
            sqlx::query(
                "INSERT INTO sub_agent_runs (
                     child_conversation_id, batch_id, ordinal, execution_authority
                 ) VALUES (?1, ?2, ?3, ?4)",
            )
            .bind(&run.child_conversation_id)
            .bind(&batch.batch_id)
            .bind(ordinal)
            .bind(run.execution_authority.as_db_str())
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(SubAgentBatchAdmissionOutcome::Admitted)
    }

    pub async fn claim_sub_agent_initial_dispatch(
        &self,
        child_conversation_id: &str,
        claimed_at: DateTime<Utc>,
    ) -> DbResult<SubAgentInitialDispatchOutcome> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let changed = sqlx::query(
            "UPDATE sub_agent_runs
             SET initial_dispatch_claimed_at_unix_micros = ?2
             WHERE child_conversation_id = ?1
               AND cancellation_requested_at_unix_micros IS NULL
               AND initial_dispatch_claimed_at_unix_micros IS NULL
               AND terminal_at_unix_micros IS NULL",
        )
        .bind(child_conversation_id)
        .bind(claimed_at.timestamp_micros())
        .execute(&mut *tx)
        .await?
        .rows_affected();
        let outcome = if changed == 1 {
            SubAgentInitialDispatchOutcome::Claimed
        } else {
            let row = sqlx::query(
                "SELECT cancellation_requested_at_unix_micros,
                        initial_dispatch_claimed_at_unix_micros, terminal_at_unix_micros
                 FROM sub_agent_runs WHERE child_conversation_id = ?1",
            )
            .bind(child_conversation_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| lifecycle_conflict(format!("unknown child {child_conversation_id}")))?;
            if row
                .try_get::<Option<i64>, _>("terminal_at_unix_micros")?
                .is_some()
            {
                SubAgentInitialDispatchOutcome::AlreadyTerminal
            } else if row
                .try_get::<Option<i64>, _>("initial_dispatch_claimed_at_unix_micros")?
                .is_some()
            {
                SubAgentInitialDispatchOutcome::AlreadyClaimed
            } else {
                SubAgentInitialDispatchOutcome::CancelledBeforeDispatch
            }
        };
        tx.commit().await?;
        Ok(outcome)
    }

    pub async fn request_sub_agent_cancellation(
        &self,
        child_conversation_id: &str,
        requested_at: DateTime<Utc>,
    ) -> DbResult<SubAgentCancellationOutcome> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let at = requested_at.timestamp_micros();
        let cancelled_before_dispatch = sqlx::query(
            "UPDATE sub_agent_runs
             SET cancellation_requested_at_unix_micros = ?2,
                 terminal_cause = 'cancelled', terminal_at_unix_micros = ?2
             WHERE child_conversation_id = ?1
               AND cancellation_requested_at_unix_micros IS NULL
               AND initial_dispatch_claimed_at_unix_micros IS NULL
               AND terminal_at_unix_micros IS NULL",
        )
        .bind(child_conversation_id)
        .bind(at)
        .execute(&mut *tx)
        .await?
        .rows_affected()
            == 1;
        let outcome = if cancelled_before_dispatch {
            SubAgentCancellationOutcome::CancelledBeforeDispatch
        } else {
            sqlx::query(
                "UPDATE sub_agent_runs
                 SET cancellation_requested_at_unix_micros = COALESCE(
                     cancellation_requested_at_unix_micros, ?2)
                 WHERE child_conversation_id = ?1 AND terminal_at_unix_micros IS NULL",
            )
            .bind(child_conversation_id)
            .bind(at)
            .execute(&mut *tx)
            .await?;
            let row = sqlx::query(
                "SELECT initial_dispatch_claimed_at_unix_micros, terminal_at_unix_micros
                 FROM sub_agent_runs WHERE child_conversation_id = ?1",
            )
            .bind(child_conversation_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| lifecycle_conflict(format!("unknown child {child_conversation_id}")))?;
            if row
                .try_get::<Option<i64>, _>("terminal_at_unix_micros")?
                .is_some()
            {
                SubAgentCancellationOutcome::AlreadyTerminal
            } else {
                SubAgentCancellationOutcome::DeliverToRuntime
            }
        };
        tx.commit().await?;
        Ok(outcome)
    }

    pub async fn record_sub_agent_terminal(
        &self,
        child_conversation_id: &str,
        cause: SubAgentTerminalCause,
        terminal_at: DateTime<Utc>,
    ) -> DbResult<SubAgentTerminalOutcome> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let at = terminal_at.timestamp_micros();
        let changed = sqlx::query(
            "UPDATE sub_agent_runs SET terminal_cause = ?2, terminal_at_unix_micros = ?3
             WHERE child_conversation_id = ?1 AND terminal_at_unix_micros IS NULL",
        )
        .bind(child_conversation_id)
        .bind(cause.as_db_str())
        .bind(at)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        let outcome = if changed == 1 {
            SubAgentTerminalOutcome::Recorded
        } else {
            let row = sqlx::query(
                "SELECT terminal_cause, terminal_at_unix_micros
                 FROM sub_agent_runs WHERE child_conversation_id = ?1",
            )
            .bind(child_conversation_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| lifecycle_conflict(format!("unknown child {child_conversation_id}")))?;
            let existing_cause = row.try_get::<Option<String>, _>("terminal_cause")?;
            let existing_at = row.try_get::<Option<i64>, _>("terminal_at_unix_micros")?;
            if existing_cause.as_deref() == Some(cause.as_db_str()) && existing_at == Some(at) {
                SubAgentTerminalOutcome::Replayed
            } else {
                return Err(lifecycle_conflict(format!(
                    "child {child_conversation_id} already has different terminal evidence"
                )));
            }
        };
        tx.commit().await?;
        Ok(outcome)
    }

    pub async fn accept_sub_agent_terminal(
        &self,
        child_conversation_id: &str,
        accepted_at: DateTime<Utc>,
    ) -> DbResult<SubAgentParentAcceptanceOutcome> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let changed = sqlx::query(
            "UPDATE sub_agent_runs SET parent_accepted_at_unix_micros = ?2
             WHERE child_conversation_id = ?1
               AND terminal_at_unix_micros IS NOT NULL
               AND parent_accepted_at_unix_micros IS NULL",
        )
        .bind(child_conversation_id)
        .bind(accepted_at.timestamp_micros())
        .execute(&mut *tx)
        .await?
        .rows_affected();
        let outcome = if changed == 1 {
            SubAgentParentAcceptanceOutcome::Accepted
        } else {
            let row: Option<(Option<i64>, Option<i64>)> = sqlx::query_as(
                "SELECT terminal_at_unix_micros, parent_accepted_at_unix_micros
                 FROM sub_agent_runs WHERE child_conversation_id = ?1",
            )
            .bind(child_conversation_id)
            .fetch_optional(&mut *tx)
            .await?;
            match row {
                Some((Some(_), Some(_))) => SubAgentParentAcceptanceOutcome::Replayed,
                Some((None, _)) => {
                    return Err(lifecycle_conflict(format!(
                        "child {child_conversation_id} is not terminal"
                    )))
                }
                None => {
                    return Err(lifecycle_conflict(format!(
                        "unknown child {child_conversation_id}"
                    )))
                }
                Some((Some(_), None)) => {
                    unreachable!("matching update must accept an unaccepted terminal")
                }
            }
        };
        tx.commit().await?;
        Ok(outcome)
    }

    pub async fn owed_sub_agent_terminals(
        &self,
        parent_conversation_id: &str,
    ) -> DbResult<Vec<OwedSubAgentTerminal>> {
        let rows = sqlx::query(
            "SELECT batch.parent_conversation_id, run.child_conversation_id,
                    run.terminal_cause, run.terminal_at_unix_micros
             FROM sub_agent_runs run
             JOIN sub_agent_batches batch ON batch.batch_id = run.batch_id
             WHERE batch.parent_conversation_id = ?1
               AND run.terminal_at_unix_micros IS NOT NULL
               AND run.parent_accepted_at_unix_micros IS NULL
             ORDER BY run.terminal_at_unix_micros, run.child_conversation_id",
        )
        .bind(parent_conversation_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(OwedSubAgentTerminal {
                    parent_conversation_id: row.try_get("parent_conversation_id")?,
                    child_conversation_id: row.try_get("child_conversation_id")?,
                    cause: SubAgentTerminalCause::from_db_str(
                        &row.try_get::<String, _>("terminal_cause")?,
                    )?,
                    terminal_at: unix_micros_to_datetime(row.try_get("terminal_at_unix_micros")?)?,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn parent_with_children(db: &Database, parent: &str, children: &[&str]) {
        db.create_conversation(parent, parent, "/tmp", true, None, Some("gpt-5.6-sol"))
            .await
            .unwrap();
        for child in children {
            db.create_conversation(
                child,
                child,
                "/tmp",
                false,
                Some(parent),
                Some("gpt-5.6-luna"),
            )
            .await
            .unwrap();
        }
    }

    fn batch(
        batch_id: &str,
        parent: &str,
        parallel_work_qualified: bool,
        children: &[(&str, SubAgentExecutionAuthority)],
    ) -> SubAgentBatchAdmission {
        SubAgentBatchAdmission {
            batch_id: batch_id.to_string(),
            parent_conversation_id: parent.to_string(),
            parallel_work_qualified,
            runs: children
                .iter()
                .map(|(child, authority)| SubAgentRunAdmission {
                    child_conversation_id: (*child).to_string(),
                    execution_authority: *authority,
                })
                .collect(),
        }
    }

    async fn open_pair(name: &str) -> (tempfile::TempDir, Database, Database) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(name);
        let first = Database::open(path.to_str().unwrap()).await.unwrap();
        migrations::run_pending_migrations(first.pool())
            .await
            .unwrap();
        let second = Database::open(path.to_str().unwrap()).await.unwrap();
        migrations::run_pending_migrations(second.pool())
            .await
            .unwrap();
        (dir, first, second)
    }

    #[tokio::test]
    async fn batch_admission_is_atomic_and_exactly_idempotent() {
        let db = Database::open_in_memory().await.unwrap();
        parent_with_children(&db, "parent", &["child-a", "child-b"]).await;
        let admission = batch(
            "batch",
            "parent",
            true,
            &[
                ("child-a", SubAgentExecutionAuthority::ReadOnly),
                ("child-b", SubAgentExecutionAuthority::WriteCapable),
            ],
        );
        assert_eq!(
            db.admit_sub_agent_batch(&admission).await.unwrap(),
            SubAgentBatchAdmissionOutcome::Admitted
        );
        assert_eq!(
            db.admit_sub_agent_batch(&admission).await.unwrap(),
            SubAgentBatchAdmissionOutcome::Replayed
        );
        let mut conflict = admission.clone();
        conflict.runs.reverse();
        assert!(matches!(
            db.admit_sub_agent_batch(&conflict).await,
            Err(DbError::SubAgentLifecycleConflict(_))
        ));

        let invalid = batch(
            "invalid",
            "parent",
            true,
            &[
                ("child-a", SubAgentExecutionAuthority::ReadOnly),
                ("missing", SubAgentExecutionAuthority::ReadOnly),
            ],
        );
        assert!(db.admit_sub_agent_batch(&invalid).await.is_err());
        let rows: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM sub_agent_runs WHERE batch_id = 'invalid'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(rows, 0);
    }

    #[tokio::test]
    async fn concurrent_unqualified_work_admission_has_one_winner() {
        let (_dir, first, second) = open_pair("unqualified-work.sqlite").await;
        parent_with_children(&first, "parent", &["child-a", "child-b"]).await;
        let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
        let a = batch(
            "batch-a",
            "parent",
            false,
            &[("child-a", SubAgentExecutionAuthority::WriteCapable)],
        );
        let b = batch(
            "batch-b",
            "parent",
            false,
            &[("child-b", SubAgentExecutionAuthority::WriteCapable)],
        );
        let barrier_a = barrier.clone();
        let first_task = tokio::spawn(async move {
            barrier_a.wait().await;
            first.admit_sub_agent_batch(&a).await
        });
        let second_task = tokio::spawn(async move {
            barrier.wait().await;
            second.admit_sub_agent_batch(&b).await
        });
        let outcomes = [first_task.await.unwrap(), second_task.await.unwrap()];
        assert_eq!(outcomes.iter().filter(|outcome| outcome.is_ok()).count(), 1);
    }

    #[tokio::test]
    async fn qualified_parent_admits_multiple_work_runs() {
        let db = Database::open_in_memory().await.unwrap();
        parent_with_children(&db, "parent", &["child-a", "child-b"]).await;
        let admission = batch(
            "batch",
            "parent",
            true,
            &[
                ("child-a", SubAgentExecutionAuthority::WriteCapable),
                ("child-b", SubAgentExecutionAuthority::WriteCapable),
            ],
        );
        assert_eq!(
            db.admit_sub_agent_batch(&admission).await.unwrap(),
            SubAgentBatchAdmissionOutcome::Admitted
        );
    }

    #[tokio::test]
    async fn claim_and_cancel_choose_one_atomic_winner() {
        let (_dir, first, second) = open_pair("claim-cancel.sqlite").await;
        parent_with_children(&first, "parent", &["child"]).await;
        first
            .admit_sub_agent_batch(&batch(
                "batch",
                "parent",
                false,
                &[("child", SubAgentExecutionAuthority::WriteCapable)],
            ))
            .await
            .unwrap();
        let at = Utc::now();
        let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
        let claim_barrier = barrier.clone();
        let claim = tokio::spawn(async move {
            claim_barrier.wait().await;
            first.claim_sub_agent_initial_dispatch("child", at).await
        });
        let cancel = tokio::spawn(async move {
            barrier.wait().await;
            second.request_sub_agent_cancellation("child", at).await
        });
        let claim = claim.await.unwrap().unwrap();
        let cancel = cancel.await.unwrap().unwrap();
        assert!(matches!(
            (claim, cancel),
            (
                SubAgentInitialDispatchOutcome::Claimed,
                SubAgentCancellationOutcome::DeliverToRuntime
            ) | (
                SubAgentInitialDispatchOutcome::AlreadyTerminal,
                SubAgentCancellationOutcome::CancelledBeforeDispatch
            ) | (
                SubAgentInitialDispatchOutcome::CancelledBeforeDispatch,
                SubAgentCancellationOutcome::CancelledBeforeDispatch
            )
        ));
    }

    #[tokio::test]
    async fn terminal_acceptance_is_monotonic_and_owed_until_accepted() {
        let db = Database::open_in_memory().await.unwrap();
        parent_with_children(&db, "parent", &["child"]).await;
        db.admit_sub_agent_batch(&batch(
            "batch",
            "parent",
            false,
            &[("child", SubAgentExecutionAuthority::ReadOnly)],
        ))
        .await
        .unwrap();
        let terminal_at = Utc::now();
        assert!(db
            .accept_sub_agent_terminal("child", terminal_at)
            .await
            .is_err());
        assert_eq!(
            db.record_sub_agent_terminal(
                "child",
                SubAgentTerminalCause::SubmitResult,
                terminal_at,
            )
            .await
            .unwrap(),
            SubAgentTerminalOutcome::Recorded
        );
        assert_eq!(
            db.owed_sub_agent_terminals("parent").await.unwrap().len(),
            1
        );
        assert_eq!(
            db.record_sub_agent_terminal(
                "child",
                SubAgentTerminalCause::SubmitResult,
                terminal_at,
            )
            .await
            .unwrap(),
            SubAgentTerminalOutcome::Replayed
        );
        assert!(db
            .record_sub_agent_terminal("child", SubAgentTerminalCause::TimedOut, terminal_at)
            .await
            .is_err());
        let accepted_at = Utc::now();
        assert_eq!(
            db.accept_sub_agent_terminal("child", accepted_at)
                .await
                .unwrap(),
            SubAgentParentAcceptanceOutcome::Accepted
        );
        assert_eq!(
            db.accept_sub_agent_terminal("child", accepted_at + chrono::Duration::seconds(1))
                .await
                .unwrap(),
            SubAgentParentAcceptanceOutcome::Replayed
        );
        let stored: (i64, i64) = sqlx::query_as(
            "SELECT terminal_at_unix_micros, parent_accepted_at_unix_micros
             FROM sub_agent_runs WHERE child_conversation_id = 'child'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(
            stored,
            (
                terminal_at.timestamp_micros(),
                accepted_at.timestamp_micros()
            )
        );
        assert!(db
            .owed_sub_agent_terminals("parent")
            .await
            .unwrap()
            .is_empty());
    }
}
