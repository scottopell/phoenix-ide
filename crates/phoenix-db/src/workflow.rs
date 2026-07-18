use crate::{DbError, DbResult};
use phoenix_workflow::{
    CodecRef, CommitOutcome, ErasedAcceptanceProfile, ExternalAcceptanceBinding,
    ExternalAcceptanceDisposition, ExternalAcceptanceOutcome, ExternalAcceptanceReceipt,
    ProfileRef, SupportedCodecRegistry, Version, WorkflowBinding, WorkflowId, WorkflowStatus,
};
use sqlx::{Row, SqlitePool};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowHead {
    pub binding: WorkflowBinding,
    pub version: Version,
    pub generation: phoenix_workflow::Generation,
    pub status: WorkflowStatus,
    pub snapshot_codec: CodecRef,
    pub snapshot_payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateWorkflowWithExternalAcceptance {
    pub workflow_id: WorkflowId,
    pub profile: ProfileRef,
    pub acceptance: ErasedAcceptanceProfile,
    pub target_scope: phoenix_workflow::ScopeId,
    pub idempotency_key: phoenix_workflow::NonEmptyExternalKey,
    pub intent_fingerprint: String,
    pub snapshot_codec: CodecRef,
    pub snapshot_payload: Vec<u8>,
    pub receipt_handle: Vec<u8>,
    pub disposition_handle: Vec<u8>,
    pub now: phoenix_workflow::Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitTransitionHeadCas {
    pub workflow_id: WorkflowId,
    pub expected_version: Version,
    pub transition_id: phoenix_workflow::TransitionId,
    pub generation: phoenix_workflow::Generation,
    pub next_status: WorkflowStatus,
    pub event_codec: CodecRef,
    pub event_payload: Vec<u8>,
    pub next_snapshot_codec: CodecRef,
    pub next_snapshot_payload: Vec<u8>,
    pub committed_at: phoenix_workflow::Timestamp,
}

#[derive(Debug, Clone)]
pub struct WorkflowRepository {
    pool: SqlitePool,
}

fn external_binding_from_input(
    input: &CreateWorkflowWithExternalAcceptance,
) -> ExternalAcceptanceBinding<Vec<u8>> {
    ExternalAcceptanceBinding {
        profile: input.profile.clone(),
        target_scope: input.target_scope.clone(),
        idempotency_key: input.idempotency_key.clone(),
        intent_fingerprint: input.intent_fingerprint.clone(),
        receipt: ExternalAcceptanceReceipt {
            idempotency_key: input.idempotency_key.clone(),
            workflow_id: input.workflow_id,
            handle: input.receipt_handle.clone(),
        },
        disposition: ExternalAcceptanceDisposition {
            workflow_id: input.workflow_id,
            handle: input.disposition_handle.clone(),
        },
    }
}

fn replay_binding_from_row(
    input: &CreateWorkflowWithExternalAcceptance,
    row: &sqlx::sqlite::SqliteRow,
) -> DbResult<ExternalAcceptanceBinding<Vec<u8>>> {
    let workflow_id = WorkflowId(to_u64(row.get::<i64, _>("workflow_id"), "workflow_id")?);
    Ok(ExternalAcceptanceBinding {
        profile: input.profile.clone(),
        target_scope: input.target_scope.clone(),
        idempotency_key: input.idempotency_key.clone(),
        intent_fingerprint: row.get("intent_fingerprint"),
        receipt: ExternalAcceptanceReceipt {
            idempotency_key: input.idempotency_key.clone(),
            workflow_id,
            handle: row.get("receipt_handle"),
        },
        disposition: ExternalAcceptanceDisposition {
            workflow_id,
            handle: row.get("disposition_handle"),
        },
    })
}

async fn insert_workflow_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    input: &CreateWorkflowWithExternalAcceptance,
) -> DbResult<()> {
    sqlx::query(
        "INSERT INTO workflows (
            workflow_id, profile_kind, profile_version,
            runtime_acceptance_enabled, external_acceptance_enabled,
            version, generation, status,
            snapshot_codec_family, snapshot_codec_version, snapshot_payload,
            created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, 0, 0, 'Active', ?6, ?7, ?8, ?9, ?9)",
    )
    .bind(to_i64(input.workflow_id.0, "workflow_id")?)
    .bind(input.profile.profile_kind)
    .bind(i64::from(input.profile.profile_version))
    .bind(input.acceptance.runtime_acceptance_enabled())
    .bind(input.acceptance.external_acceptance_enabled())
    .bind(input.snapshot_codec.family)
    .bind(i64::from(input.snapshot_codec.version))
    .bind(&input.snapshot_payload)
    .bind(to_i64(input.now.0, "timestamp")?)
    .execute(&mut **tx)
    .await?;

    for codec in input.acceptance.supported_codecs.iter() {
        sqlx::query(
            "INSERT INTO workflow_supported_codecs (workflow_id, codec_family, codec_version)
             VALUES (?1, ?2, ?3)",
        )
        .bind(to_i64(input.workflow_id.0, "workflow_id")?)
        .bind(codec.family)
        .bind(i64::from(codec.version))
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

async fn insert_external_acceptance_binding_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    input: &CreateWorkflowWithExternalAcceptance,
) -> DbResult<()> {
    sqlx::query(
        "INSERT INTO workflow_external_acceptance_bindings (
            profile_kind, profile_version, target_scope, idempotency_key,
            intent_fingerprint, workflow_id, receipt_handle,
            disposition_handle, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
    )
    .bind(input.profile.profile_kind)
    .bind(i64::from(input.profile.profile_version))
    .bind(input.target_scope.as_str())
    .bind(input.idempotency_key.as_str())
    .bind(&input.intent_fingerprint)
    .bind(to_i64(input.workflow_id.0, "workflow_id")?)
    .bind(&input.receipt_handle)
    .bind(&input.disposition_handle)
    .bind(to_i64(input.now.0, "timestamp")?)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

impl WorkflowRepository {
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// # Errors
    ///
    /// Returns an error when the transaction cannot read or persist the workflow foundation rows.
    pub async fn create_workflow_with_external_acceptance(
        &self,
        input: &CreateWorkflowWithExternalAcceptance,
    ) -> DbResult<ExternalAcceptanceOutcome<Vec<u8>>> {
        let mut tx = self.pool.begin().await?;
        let existing = sqlx::query(
            "SELECT workflow_id, intent_fingerprint, receipt_handle, disposition_handle
             FROM workflow_external_acceptance_bindings
             WHERE profile_kind = ?1 AND profile_version = ?2 AND target_scope = ?3 AND idempotency_key = ?4",
        )
        .bind(input.profile.profile_kind)
        .bind(i64::from(input.profile.profile_version))
        .bind(input.target_scope.as_str())
        .bind(input.idempotency_key.as_str())
        .fetch_optional(&mut *tx)
        .await?;

        if let Some(row) = existing {
            let replay = replay_binding_from_row(input, &row)?;
            tx.commit().await?;
            return Ok(
                if replay.intent_fingerprint == input.intent_fingerprint
                    && replay.receipt.handle == input.receipt_handle
                    && replay.disposition.handle == input.disposition_handle
                    && replay.receipt.workflow_id == input.workflow_id
                {
                    ExternalAcceptanceOutcome::Replayed(replay)
                } else {
                    ExternalAcceptanceOutcome::Conflict
                },
            );
        }

        insert_workflow_tx(&mut tx, input).await?;
        insert_external_acceptance_binding_tx(&mut tx, input).await?;
        tx.commit().await?;
        Ok(ExternalAcceptanceOutcome::Created(
            external_binding_from_input(input),
        ))
    }

    /// # Errors
    ///
    /// Returns an error when the compare-and-swap transaction cannot update or append the transition row.
    pub async fn commit_transition_head_cas(
        &self,
        input: &CommitTransitionHeadCas,
    ) -> DbResult<CommitOutcome> {
        let mut tx = self.pool.begin().await?;
        let updated = sqlx::query(
            "UPDATE workflows
             SET version = version + 1,
                 generation = ?3,
                 status = ?4,
                 snapshot_codec_family = ?5,
                 snapshot_codec_version = ?6,
                 snapshot_payload = ?7,
                 updated_at = ?8
             WHERE workflow_id = ?1 AND version = ?2",
        )
        .bind(to_i64(input.workflow_id.0, "workflow_id")?)
        .bind(to_i64(input.expected_version.0, "expected_version")?)
        .bind(to_i64(input.generation.0, "generation")?)
        .bind(workflow_status_to_str(input.next_status))
        .bind(input.next_snapshot_codec.family)
        .bind(i64::from(input.next_snapshot_codec.version))
        .bind(&input.next_snapshot_payload)
        .bind(to_i64(input.committed_at.0, "committed_at")?)
        .execute(&mut *tx)
        .await?
        .rows_affected();

        if updated == 0 {
            tx.rollback().await?;
            return Ok(CommitOutcome::VersionConflict);
        }

        sqlx::query(
            "INSERT INTO workflow_transitions (
                workflow_id, transition_id, from_version, to_version, generation,
                event_codec_family, event_codec_version, event_payload, committed_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        )
        .bind(to_i64(input.workflow_id.0, "workflow_id")?)
        .bind(to_i64(input.transition_id.0, "transition_id")?)
        .bind(to_i64(input.expected_version.0, "expected_version")?)
        .bind(to_i64(input.expected_version.next().0, "next_version")?)
        .bind(to_i64(input.generation.0, "generation")?)
        .bind(input.event_codec.family)
        .bind(i64::from(input.event_codec.version))
        .bind(&input.event_payload)
        .bind(to_i64(input.committed_at.0, "committed_at")?)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(CommitOutcome::Committed)
    }

    /// # Errors
    ///
    /// Returns an error when reading the workflow head or decoding persisted numeric fields fails.
    pub async fn fetch_workflow_head(
        &self,
        workflow_id: WorkflowId,
    ) -> DbResult<Option<WorkflowHead>> {
        let row = sqlx::query(
            "SELECT
                workflow_id, profile_kind, profile_version,
                runtime_acceptance_enabled, external_acceptance_enabled,
                version, generation, status,
                snapshot_codec_family, snapshot_codec_version, snapshot_payload
             FROM workflows
             WHERE workflow_id = ?1",
        )
        .bind(to_i64(workflow_id.0, "workflow_id")?)
        .fetch_optional(&self.pool)
        .await?;

        let Some(row) = row else {
            return Ok(None);
        };

        let codec_rows = sqlx::query(
            "SELECT codec_family, codec_version
             FROM workflow_supported_codecs
             WHERE workflow_id = ?1
             ORDER BY codec_family, codec_version",
        )
        .bind(to_i64(workflow_id.0, "workflow_id")?)
        .fetch_all(&self.pool)
        .await?;
        let codecs = codec_rows
            .into_iter()
            .map(|codec_row| {
                let family: String = codec_row.get("codec_family");
                Ok(CodecRef {
                    family: Box::leak(family.into_boxed_str()),
                    version: to_u32(codec_row.get::<i64, _>("codec_version"), "codec_version")?,
                })
            })
            .collect::<DbResult<Vec<_>>>()?;
        let supported_codecs = SupportedCodecRegistry::new(codecs).ok_or_else(|| {
            DbError::Serialization("workflow_supported_codecs cannot be empty".to_string())
        })?;
        let profile_kind: String = row.get("profile_kind");
        let profile = ProfileRef {
            profile_kind: Box::leak(profile_kind.into_boxed_str()),
            profile_version: to_u32(row.get::<i64, _>("profile_version"), "profile_version")?,
        };
        let acceptance = ErasedAcceptanceProfile::from_parts(
            profile.clone(),
            supported_codecs,
            row.get::<bool, _>("runtime_acceptance_enabled"),
            row.get::<bool, _>("external_acceptance_enabled"),
        );
        Ok(Some(WorkflowHead {
            binding: WorkflowBinding {
                workflow_id,
                profile,
                acceptance,
            },
            version: Version(to_u64(row.get::<i64, _>("version"), "version")?),
            generation: phoenix_workflow::Generation(to_u64(
                row.get::<i64, _>("generation"),
                "generation",
            )?),
            status: parse_workflow_status(&row.get::<String, _>("status"))?,
            snapshot_codec: CodecRef {
                family: Box::leak(
                    row.get::<String, _>("snapshot_codec_family")
                        .into_boxed_str(),
                ),
                version: to_u32(
                    row.get::<i64, _>("snapshot_codec_version"),
                    "snapshot_codec_version",
                )?,
            },
            snapshot_payload: row.get("snapshot_payload"),
        }))
    }
}

fn to_i64(value: u64, field: &str) -> DbResult<i64> {
    i64::try_from(value).map_err(|_| DbError::Serialization(format!("{field} exceeds i64")))
}

fn to_u64(value: i64, field: &str) -> DbResult<u64> {
    u64::try_from(value).map_err(|_| DbError::Serialization(format!("{field} is negative")))
}

fn to_u32(value: i64, field: &str) -> DbResult<u32> {
    u32::try_from(value).map_err(|_| DbError::Serialization(format!("{field} out of range")))
}

fn parse_workflow_status(value: &str) -> DbResult<WorkflowStatus> {
    match value {
        "Active" => Ok(WorkflowStatus::Active),
        "Cancelling" => Ok(WorkflowStatus::Cancelling),
        "ManualResolution" => Ok(WorkflowStatus::ManualResolution),
        "Incompatible" => Ok(WorkflowStatus::Incompatible),
        "Cancelled" => Ok(WorkflowStatus::Cancelled),
        "DeletionPending" => Ok(WorkflowStatus::DeletionPending),
        "Deleted" => Ok(WorkflowStatus::Deleted),
        "Completed" => Ok(WorkflowStatus::Completed),
        "Failed" => Ok(WorkflowStatus::Failed),
        other => Err(DbError::Serialization(format!(
            "unknown workflow status: {other}"
        ))),
    }
}

fn workflow_status_to_str(status: WorkflowStatus) -> &'static str {
    match status {
        WorkflowStatus::Active => "Active",
        WorkflowStatus::Cancelling => "Cancelling",
        WorkflowStatus::ManualResolution => "ManualResolution",
        WorkflowStatus::Incompatible => "Incompatible",
        WorkflowStatus::Cancelled => "Cancelled",
        WorkflowStatus::DeletionPending => "DeletionPending",
        WorkflowStatus::Deleted => "Deleted",
        WorkflowStatus::Completed => "Completed",
        WorkflowStatus::Failed => "Failed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrations::run_pending_migrations;
    use phoenix_workflow::{
        Generation, NonEmptyExternalKey, ScopeId, SupportedCodecRegistry, Timestamp, TransitionId,
    };
    use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
    use std::str::FromStr;

    fn profile() -> ProfileRef {
        ProfileRef {
            profile_kind: "test",
            profile_version: 1,
        }
    }

    fn codec(family: &'static str) -> CodecRef {
        CodecRef { family, version: 1 }
    }

    fn acceptance() -> ErasedAcceptanceProfile {
        ErasedAcceptanceProfile::from_parts(
            profile(),
            SupportedCodecRegistry::new([codec("snapshot"), codec("event")]).unwrap(),
            true,
            true,
        )
    }

    async fn setup_repo_schema(pool: &SqlitePool) {
        sqlx::query("CREATE TABLE conversations (id TEXT PRIMARY KEY, conv_mode TEXT NOT NULL DEFAULT '{\"mode\":\"Explore\"}', state TEXT NOT NULL DEFAULT '{\"type\":\"idle\"}', cwd TEXT NOT NULL DEFAULT '/tmp', parent_conversation_id TEXT, user_initiated BOOLEAN NOT NULL DEFAULT 1, archived BOOLEAN NOT NULL DEFAULT 0, model TEXT, steering_queue TEXT NOT NULL DEFAULT '[]', state_updated_at TEXT NOT NULL DEFAULT '2025-01-01', created_at TEXT NOT NULL DEFAULT '2025-01-01', updated_at TEXT NOT NULL DEFAULT '2025-01-01')")
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("CREATE TABLE messages (id INTEGER PRIMARY KEY AUTOINCREMENT, message_id TEXT UNIQUE, conversation_id TEXT NOT NULL, message_type TEXT NOT NULL, content TEXT NOT NULL, sequence_id INTEGER NOT NULL DEFAULT 1, created_at TEXT NOT NULL DEFAULT '2025-01-01')")
            .execute(pool)
            .await
            .unwrap();
        run_pending_migrations(pool).await.unwrap();
    }

    async fn open_repo_pair() -> (tempfile::TempDir, WorkflowRepository, WorkflowRepository) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workflow.db");
        let url = format!("sqlite://{}", path.display());
        let open = || async {
            let opts = SqliteConnectOptions::from_str(&url)
                .unwrap()
                .create_if_missing(true)
                .journal_mode(SqliteJournalMode::Wal)
                .busy_timeout(std::time::Duration::from_secs(5));
            let pool = SqlitePoolOptions::new()
                .max_connections(1)
                .connect_with(opts)
                .await
                .unwrap();
            sqlx::query("PRAGMA foreign_keys = ON")
                .execute(&pool)
                .await
                .unwrap();
            if sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='conversations'",
            )
            .fetch_one(&pool)
            .await
            .unwrap()
                == 0
            {
                setup_repo_schema(&pool).await;
            }
            WorkflowRepository::new(pool)
        };
        (dir, open().await, open().await)
    }

    #[tokio::test]
    async fn create_workflow_with_external_acceptance_replays_and_conflicts() {
        let (_dir, first, second) = open_repo_pair().await;
        let input = CreateWorkflowWithExternalAcceptance {
            workflow_id: WorkflowId(11),
            profile: profile(),
            acceptance: acceptance(),
            target_scope: ScopeId::new("scope-a").unwrap(),
            idempotency_key: NonEmptyExternalKey::new("key-a").unwrap(),
            intent_fingerprint: "fp-1".to_string(),
            snapshot_codec: codec("snapshot"),
            snapshot_payload: vec![1],
            receipt_handle: vec![2],
            disposition_handle: vec![3],
            now: Timestamp(10),
        };

        let created = first
            .create_workflow_with_external_acceptance(&input)
            .await
            .unwrap();
        assert!(matches!(created, ExternalAcceptanceOutcome::Created(_)));

        let replay = second
            .create_workflow_with_external_acceptance(&input)
            .await
            .unwrap();
        assert!(matches!(replay, ExternalAcceptanceOutcome::Replayed(_)));

        let mut conflict_input = input.clone();
        conflict_input.workflow_id = WorkflowId(12);
        conflict_input.intent_fingerprint = "fp-2".to_string();
        let conflict = second
            .create_workflow_with_external_acceptance(&conflict_input)
            .await
            .unwrap();
        assert_eq!(conflict, ExternalAcceptanceOutcome::Conflict);
    }

    #[tokio::test]
    async fn commit_transition_head_cas_allows_single_winner_across_connections() {
        let (_dir, first, second) = open_repo_pair().await;
        let created = CreateWorkflowWithExternalAcceptance {
            workflow_id: WorkflowId(21),
            profile: profile(),
            acceptance: acceptance(),
            target_scope: ScopeId::new("scope-b").unwrap(),
            idempotency_key: NonEmptyExternalKey::new("key-b").unwrap(),
            intent_fingerprint: "fp-b".to_string(),
            snapshot_codec: codec("snapshot"),
            snapshot_payload: vec![7],
            receipt_handle: vec![8],
            disposition_handle: vec![9],
            now: Timestamp(20),
        };
        first
            .create_workflow_with_external_acceptance(&created)
            .await
            .unwrap();

        let cas1 = CommitTransitionHeadCas {
            workflow_id: WorkflowId(21),
            expected_version: Version(0),
            transition_id: TransitionId(1),
            generation: Generation(1),
            next_status: WorkflowStatus::Active,
            event_codec: codec("event"),
            event_payload: vec![1],
            next_snapshot_codec: codec("snapshot"),
            next_snapshot_payload: vec![2],
            committed_at: Timestamp(21),
        };
        let mut cas2 = cas1.clone();
        cas2.transition_id = TransitionId(2);
        cas2.event_payload = vec![3];
        cas2.next_snapshot_payload = vec![4];

        let (left, right) = tokio::join!(
            first.commit_transition_head_cas(&cas1),
            second.commit_transition_head_cas(&cas2)
        );
        let outcomes = [left.unwrap(), right.unwrap()];
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| **outcome == CommitOutcome::Committed)
                .count(),
            1
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| **outcome == CommitOutcome::VersionConflict)
                .count(),
            1
        );

        let head = first
            .fetch_workflow_head(WorkflowId(21))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(head.version, Version(1));
        assert_eq!(head.generation, Generation(1));
        assert!(head.snapshot_payload == vec![2] || head.snapshot_payload == vec![4]);
    }
}
