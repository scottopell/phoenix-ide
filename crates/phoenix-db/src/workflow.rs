use chrono::{DateTime, Utc};
use phoenix_workflow::{
    BarrierStatus, EffectAmbiguity, EffectRole, EffectStatus, ReceiptFamily, SemanticAuthority,
    WorkflowStatus,
};
use sqlx::{
    error::DatabaseError,
    sqlite::{SqliteConnection, SqliteError},
    Executor, Row, Sqlite, SqlitePool, Transaction,
};
use std::collections::{HashMap, HashSet};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum WorkflowRepositoryError {
    #[error("database error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("invalid workflow version for SQLite i64: {0}")]
    VersionOutOfRange(u64),
    #[error("invalid generation for SQLite i64: {0}")]
    GenerationOutOfRange(u64),
    #[error("invalid workflow plan: {0}")]
    InvalidPlan(&'static str),
    #[error("rollback test failpoint triggered at {0:?}")]
    Failpoint(WorkflowFailpoint),
}

pub type WorkflowRepositoryResult<T> = Result<T, WorkflowRepositoryError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableCodecRef {
    pub family: String,
    pub version: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableProtocolSelectionRegistration {
    pub selection_id: String,
    pub profile_id: String,
    pub selector_identity: String,
    pub selector_version: u32,
    pub protocol_version: u32,
    pub authority: SemanticAuthority,
    pub accepting: bool,
    pub runtime_acceptance_enabled: bool,
    pub external_acceptance_enabled: bool,
    pub registered_at: DateTime<Utc>,
    pub drained_at: Option<DateTime<Utc>>,
    pub supported_codecs: Vec<DurableCodecRef>,
    pub executor_kinds: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableExternalAuthority {
    pub authority: SemanticAuthority,
    pub scope: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurablePayload {
    pub codec: DurableCodecRef,
    pub payload: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableWorkflowAcceptance {
    pub selection_id: String,
    pub profile_id: String,
    pub protocol_version: u32,
    pub authority: DurableExternalAuthority,
    pub idempotency_key: String,
    pub intent_fingerprint: String,
    pub binding_id: String,
    pub workflow_id: String,
    pub accepted_at: DateTime<Utc>,
    pub workflow_snapshot: DurablePayload,
    pub handle_receipt: DurablePayload,
    pub executor_kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternalAcceptanceResult {
    New {
        workflow_id: String,
        handle_receipt: DurablePayload,
    },
    Replay {
        workflow_id: String,
        handle_receipt: DurablePayload,
    },
    Conflict,
    Retryable,
    NotAccepting,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableWorkflowTransitionCommit {
    pub transition_id: String,
    pub workflow_id: String,
    pub expected_from_version: u64,
    pub next_version: u64,
    pub next_generation: u64,
    pub committed_at: DateTime<Utc>,
    pub workflow_status: WorkflowStatus,
    pub snapshot: DurablePayload,
    pub event: DurablePayload,
    pub effects: Vec<DurableEffectRecord>,
    pub dependencies: Vec<DurableEffectDependencyRecord>,
    pub barriers: Vec<DurableBarrierRecord>,
    pub barrier_members: Vec<DurableBarrierMemberRecord>,
    pub invalidations: Vec<DurableInvalidationRecord>,
    pub owed_acceptances: Vec<DurableOwedAcceptanceRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableEffectRecord {
    pub effect_id: String,
    pub family: String,
    pub kind: String,
    pub codec: DurableCodecRef,
    pub role: EffectRole,
    pub ambiguity_policy: EffectAmbiguity,
    pub intent_payload: String,
    pub next_eligible_at: Option<DateTime<Utc>>,
    pub destructive_resource: Option<String>,
    pub generation: u64,
    pub status: EffectStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableEffectDependencyRecord {
    pub effect_id: String,
    pub dependency_effect_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableBarrierRecord {
    pub barrier_id: String,
    pub status: BarrierStatus,
    pub satisfied_at: Option<DateTime<Utc>>,
    pub barrier_event: DurablePayload,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableBarrierMemberRecord {
    pub barrier_id: String,
    pub effect_id: String,
    pub receipt_family: ReceiptFamily,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableInvalidationRecord {
    pub effect_id: String,
    pub expected_declared_workflow_version: u64,
    pub expected_generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableOwedAcceptanceRecord {
    pub owed_acceptance_id: String,
    pub reducer_inbox_id: String,
    pub source_kind: String,
    pub event: DurablePayload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionCommitOutcome {
    Committed,
    VersionConflict,
    InvalidPlan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableClaimAuthority {
    pub workflow_id: String,
    pub declared_workflow_version: u64,
    pub generation: u64,
    pub effect_id: String,
    pub claim_token: String,
    pub worker_id: String,
    pub lease_until: DateTime<Utc>,
    pub issued_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableAttemptRecord {
    pub attempt_id: String,
    pub effect_id: String,
    pub workflow_id: String,
    pub declared_workflow_version: u64,
    pub generation: u64,
    pub ordinal: u64,
    pub claim: DurableClaimAuthority,
    pub status: String,
    pub begun_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimEffectResult {
    Claimed {
        authority: DurableClaimAuthority,
        attempt: Box<DurableAttemptRecord>,
    },
    Ineligible,
    Contended,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenewClaimResult {
    Renewed { authority: DurableClaimAuthority },
    StaleAuthority,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TakeOverExpiredClaimResult {
    Claimed {
        authority: DurableClaimAuthority,
        attempt: Box<DurableAttemptRecord>,
    },
    Ineligible,
    StaleAuthority,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableClaimRequest {
    pub workflow_id: String,
    pub effect_id: String,
    pub claim_token: String,
    pub worker_id: String,
    pub lease_until: DateTime<Utc>,
    pub now: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableClaimRenewal {
    pub authority: DurableClaimAuthority,
    pub now: DateTime<Utc>,
    pub lease_until: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableClaimTakeover {
    pub authority: DurableClaimAuthority,
    pub replacement_claim_token: String,
    pub replacement_worker_id: String,
    pub now: DateTime<Utc>,
    pub lease_until: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowFailpoint {
    AfterWorkflowInsert,
    AfterWorkflowUpdate,
    AfterBindingInsert,
    AfterTransitionInsert,
    AfterBarrierInsert,
    AfterInvalidations,
}

#[derive(Debug)]
pub struct WorkflowRepository {
    pool: SqlitePool,
}

impl WorkflowRepository {
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    #[must_use]
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Persist a protocol selection plus its supported codecs and executors in one transaction.
    ///
    /// # Errors
    ///
    /// Returns an error when the transaction fails, a constraint rejects the data,
    /// or a test failpoint is configured by the delegated variant.
    pub async fn register_protocol_selection(
        &self,
        registration: &DurableProtocolSelectionRegistration,
    ) -> WorkflowRepositoryResult<()> {
        self.register_protocol_selection_with_failpoint(registration, None)
            .await
    }

    /// Persist a protocol selection plus its supported codecs and executors in one transaction,
    /// optionally aborting at a test failpoint to prove rollback.
    ///
    /// # Errors
    ///
    /// Returns an error when the transaction fails, a constraint rejects the data,
    /// or the requested failpoint intentionally aborts the transaction.
    pub async fn register_protocol_selection_with_failpoint(
        &self,
        registration: &DurableProtocolSelectionRegistration,
        failpoint: Option<WorkflowFailpoint>,
    ) -> WorkflowRepositoryResult<()> {
        let mut tx = self.pool.begin().await?;
        insert_protocol_selection(&mut tx, registration).await?;
        maybe_fail(failpoint, WorkflowFailpoint::AfterWorkflowInsert)?;
        tx.commit().await?;
        Ok(())
    }

    /// Atomically accept a new external workflow or replay a prior accepted binding.
    ///
    /// # Errors
    ///
    /// Returns an error when lookup or insert queries fail, a constraint rejects the data,
    /// or a delegated test failpoint aborts the transaction.
    pub async fn accept_external_workflow(
        &self,
        acceptance: &DurableWorkflowAcceptance,
    ) -> WorkflowRepositoryResult<ExternalAcceptanceResult> {
        self.accept_external_workflow_with_failpoint(acceptance, None)
            .await
    }

    /// Atomically accept a new external workflow or replay a prior accepted binding,
    /// optionally aborting at a test failpoint to prove rollback.
    ///
    /// # Errors
    ///
    /// Returns an error when lookup or insert queries fail, a constraint rejects the data,
    /// or the requested failpoint intentionally aborts the transaction.
    pub async fn accept_external_workflow_with_failpoint(
        &self,
        acceptance: &DurableWorkflowAcceptance,
        failpoint: Option<WorkflowFailpoint>,
    ) -> WorkflowRepositoryResult<ExternalAcceptanceResult> {
        if let Some(existing) = lookup_existing_binding(&self.pool, acceptance).await? {
            return Ok(existing);
        }

        let mut tx = self.pool.begin().await?;

        if let Some(existing) = lookup_existing_binding(&mut *tx, acceptance).await? {
            tx.rollback().await?;
            return Ok(existing);
        }

        if !selection_accepts_external(&mut tx, acceptance).await? {
            tx.rollback().await?;
            return Ok(ExternalAcceptanceResult::NotAccepting);
        }

        let workflow_insert = sqlx::query(
            "INSERT INTO workflows \
             (id, profile_id, protocol_version, authority, execution_mode, authoritative_workflow_id, \
              protocol_selection_id, version, generation, status, snapshot_codec_family, \
              snapshot_codec_version, snapshot_payload, accepted_at) \
             VALUES (?1, ?2, ?3, ?4, 'authoritative', NULL, ?5, 0, 0, 'active', ?6, ?7, ?8, ?9)",
        )
        .bind(&acceptance.workflow_id)
        .bind(&acceptance.profile_id)
        .bind(i64::from(acceptance.protocol_version))
        .bind(authority_sql(acceptance.authority.authority))
        .bind(&acceptance.selection_id)
        .bind(&acceptance.workflow_snapshot.codec.family)
        .bind(i64::from(acceptance.workflow_snapshot.codec.version))
        .bind(&acceptance.workflow_snapshot.payload)
        .bind(acceptance.accepted_at.to_rfc3339())
        .execute(&mut *tx)
        .await;

        if let Err(err) = workflow_insert {
            tx.rollback().await?;
            return resolve_external_acceptance_race(err, &self.pool, acceptance).await;
        }

        fail_if_configured(&mut tx, failpoint, WorkflowFailpoint::AfterWorkflowInsert).await?;

        let binding_insert = sqlx::query(
            "INSERT INTO external_acceptance_bindings \
             (id, selection_id, profile_id, protocol_version, authority, authority_scope, \
              idempotency_key, intent_fingerprint, workflow_id, receipt_codec_family, \
              receipt_codec_version, receipt_payload, accepted_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        )
        .bind(&acceptance.binding_id)
        .bind(&acceptance.selection_id)
        .bind(&acceptance.profile_id)
        .bind(i64::from(acceptance.protocol_version))
        .bind(authority_sql(acceptance.authority.authority))
        .bind(&acceptance.authority.scope)
        .bind(&acceptance.idempotency_key)
        .bind(&acceptance.intent_fingerprint)
        .bind(&acceptance.workflow_id)
        .bind(&acceptance.handle_receipt.codec.family)
        .bind(i64::from(acceptance.handle_receipt.codec.version))
        .bind(&acceptance.handle_receipt.payload)
        .bind(acceptance.accepted_at.to_rfc3339())
        .execute(&mut *tx)
        .await;

        if let Err(err) = binding_insert {
            tx.rollback().await?;
            return resolve_external_acceptance_race(err, &self.pool, acceptance).await;
        }

        fail_if_configured(&mut tx, failpoint, WorkflowFailpoint::AfterBindingInsert).await?;

        tx.commit().await?;
        Ok(ExternalAcceptanceResult::New {
            workflow_id: acceptance.workflow_id.clone(),
            handle_receipt: acceptance.handle_receipt.clone(),
        })
    }

    /// Persist a validated reducer transition plan under workflow version compare-and-swap.
    ///
    /// # Errors
    ///
    /// Returns an error when the transaction fails, a constraint rejects the DAG rows,
    /// integer conversion exceeds `SQLite` storage, or a delegated failpoint aborts the transaction.
    pub async fn persist_transition_plan(
        &self,
        commit: &DurableWorkflowTransitionCommit,
    ) -> WorkflowRepositoryResult<TransitionCommitOutcome> {
        self.persist_transition_plan_with_failpoint(commit, None)
            .await
    }

    /// Persist a validated reducer transition plan under workflow version compare-and-swap,
    /// optionally aborting after workflow update or mid-DAG to prove rollback.
    ///
    /// # Errors
    ///
    /// Returns an error when the transaction fails, a constraint rejects the DAG rows,
    /// integer conversion exceeds `SQLite` storage, or the requested failpoint intentionally aborts.
    #[allow(clippy::too_many_lines)]
    pub async fn persist_transition_plan_with_failpoint(
        &self,
        commit: &DurableWorkflowTransitionCommit,
        failpoint: Option<WorkflowFailpoint>,
    ) -> WorkflowRepositoryResult<TransitionCommitOutcome> {
        let mut tx = self.pool.begin().await?;
        let Some(workflow) = load_workflow_for_commit(&mut tx, &commit.workflow_id).await? else {
            tx.rollback().await?;
            return Ok(TransitionCommitOutcome::VersionConflict);
        };

        if let Err(WorkflowRepositoryError::InvalidPlan(_)) =
            validate_transition_plan(&mut tx, commit, &workflow).await
        {
            tx.rollback().await?;
            return Ok(TransitionCommitOutcome::InvalidPlan);
        }

        let updated = sqlx::query(
            "UPDATE workflows SET version = ?1, generation = ?2, status = ?3, \
             snapshot_codec_family = ?4, snapshot_codec_version = ?5, snapshot_payload = ?6 \
             WHERE id = ?7 AND version = ?8 AND authority = ?9 AND execution_mode = ?10 \
               AND protocol_selection_id = ?11 AND generation = ?12 AND status = ?13",
        )
        .bind(to_i64(
            commit.next_version,
            WorkflowRepositoryError::VersionOutOfRange,
        )?)
        .bind(to_i64(
            commit.next_generation,
            WorkflowRepositoryError::GenerationOutOfRange,
        )?)
        .bind(workflow_status_sql(commit.workflow_status))
        .bind(&commit.snapshot.codec.family)
        .bind(i64::from(commit.snapshot.codec.version))
        .bind(&commit.snapshot.payload)
        .bind(&commit.workflow_id)
        .bind(to_i64(
            commit.expected_from_version,
            WorkflowRepositoryError::VersionOutOfRange,
        )?)
        .bind(authority_sql(workflow.authority))
        .bind(workflow_execution_mode_sql(workflow.execution_mode))
        .bind(&workflow.selection_id)
        .bind(to_i64(
            workflow.generation,
            WorkflowRepositoryError::GenerationOutOfRange,
        )?)
        .bind(workflow_status_sql(workflow.status))
        .execute(&mut *tx)
        .await?;

        if updated.rows_affected() != 1 {
            tx.rollback().await?;
            return Ok(TransitionCommitOutcome::VersionConflict);
        }

        maybe_fail(failpoint, WorkflowFailpoint::AfterWorkflowUpdate)?;

        sqlx::query(
            "INSERT INTO workflow_transitions \
             (id, workflow_id, from_version, to_version, generation, event_codec_family, \
              event_codec_version, event_payload, committed_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        )
        .bind(&commit.transition_id)
        .bind(&commit.workflow_id)
        .bind(to_i64(
            commit.expected_from_version,
            WorkflowRepositoryError::VersionOutOfRange,
        )?)
        .bind(to_i64(
            commit.next_version,
            WorkflowRepositoryError::VersionOutOfRange,
        )?)
        .bind(to_i64(
            commit.next_generation,
            WorkflowRepositoryError::GenerationOutOfRange,
        )?)
        .bind(&commit.event.codec.family)
        .bind(i64::from(commit.event.codec.version))
        .bind(&commit.event.payload)
        .bind(commit.committed_at.to_rfc3339())
        .execute(&mut *tx)
        .await?;

        maybe_fail(failpoint, WorkflowFailpoint::AfterTransitionInsert)?;

        for effect in &commit.effects {
            sqlx::query(
                "INSERT INTO workflow_effects \
                 (id, workflow_id, declaring_transition_id, declared_workflow_version, generation, \
                  family, kind, codec_family, codec_version, role, ambiguity_policy, intent_payload, \
                  status, pending_reconciliation, next_eligible_at, destructive_resource) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, 0, ?14, ?15)",
            )
            .bind(&effect.effect_id)
            .bind(&commit.workflow_id)
            .bind(&commit.transition_id)
            .bind(to_i64(commit.next_version, WorkflowRepositoryError::VersionOutOfRange)?)
            .bind(to_i64(effect.generation, WorkflowRepositoryError::GenerationOutOfRange)?)
            .bind(&effect.family)
            .bind(&effect.kind)
            .bind(&effect.codec.family)
            .bind(i64::from(effect.codec.version))
            .bind(effect_role_sql(effect.role))
            .bind(effect_ambiguity_sql(effect.ambiguity_policy))
            .bind(&effect.intent_payload)
            .bind(effect_status_sql(effect.status))
            .bind(effect.next_eligible_at.map(|ts| ts.to_rfc3339()))
            .bind(effect.destructive_resource.as_deref())
            .execute(&mut *tx)
            .await?;
        }

        for dependency in &commit.dependencies {
            sqlx::query(
                "INSERT INTO workflow_effect_dependencies (effect_id, dependency_effect_id) \
                 VALUES (?1, ?2)",
            )
            .bind(&dependency.effect_id)
            .bind(&dependency.dependency_effect_id)
            .execute(&mut *tx)
            .await?;
        }

        for barrier in &commit.barriers {
            sqlx::query(
                "INSERT INTO workflow_barriers \
                 (id, workflow_id, declaring_transition_id, declaring_workflow_version, status, satisfied_at, \
                  event_codec_family, event_codec_version, event_payload) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )
            .bind(&barrier.barrier_id)
            .bind(&commit.workflow_id)
            .bind(&commit.transition_id)
            .bind(to_i64(commit.next_version, WorkflowRepositoryError::VersionOutOfRange)?)
            .bind(barrier_status_sql(barrier.status))
            .bind(barrier.satisfied_at.map(|ts| ts.to_rfc3339()))
            .bind(&barrier.barrier_event.codec.family)
            .bind(i64::from(barrier.barrier_event.codec.version))
            .bind(&barrier.barrier_event.payload)
            .execute(&mut *tx)
            .await?;
        }

        maybe_fail(failpoint, WorkflowFailpoint::AfterBarrierInsert)?;

        for member in &commit.barrier_members {
            sqlx::query(
                "INSERT INTO workflow_barrier_members (barrier_id, effect_id, receipt_family) \
                 VALUES (?1, ?2, ?3)",
            )
            .bind(&member.barrier_id)
            .bind(&member.effect_id)
            .bind(receipt_family_sql(member.receipt_family))
            .execute(&mut *tx)
            .await?;
        }

        for owed in &commit.owed_acceptances {
            let updated = sqlx::query(
                "UPDATE workflow_reducer_inbox \
                 SET delivery_status = 'consumed', consumed_by_transition_id = ?1 \
                 WHERE id = ?2 AND workflow_id = ?3 AND delivery_status = 'pending' \
                   AND consumed_by_transition_id IS NULL",
            )
            .bind(&commit.transition_id)
            .bind(&owed.reducer_inbox_id)
            .bind(&commit.workflow_id)
            .execute(&mut *tx)
            .await?;
            if updated.rows_affected() != 1 {
                tx.rollback().await?;
                return Ok(TransitionCommitOutcome::InvalidPlan);
            }

            sqlx::query(
                "INSERT INTO workflow_owed_acceptance \
                 (id, workflow_id, reducer_inbox_id, source_kind, event_codec_family, \
                  event_codec_version, event_payload, status, resolving_transition_id, suppression_reason) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'owed', NULL, NULL)",
            )
            .bind(&owed.owed_acceptance_id)
            .bind(&commit.workflow_id)
            .bind(&owed.reducer_inbox_id)
            .bind(&owed.source_kind)
            .bind(&owed.event.codec.family)
            .bind(i64::from(owed.event.codec.version))
            .bind(&owed.event.payload)
            .execute(&mut *tx)
            .await?;
        }

        for invalidation in &commit.invalidations {
            let updated = sqlx::query(
                "UPDATE workflow_effects SET status = ?1 \
                 WHERE id = ?2 AND workflow_id = ?3 AND declared_workflow_version = ?4 \
                   AND generation = ?5 AND status <> ?6",
            )
            .bind(effect_status_sql(EffectStatus::Invalidated))
            .bind(&invalidation.effect_id)
            .bind(&commit.workflow_id)
            .bind(to_i64(
                invalidation.expected_declared_workflow_version,
                WorkflowRepositoryError::VersionOutOfRange,
            )?)
            .bind(to_i64(
                invalidation.expected_generation,
                WorkflowRepositoryError::GenerationOutOfRange,
            )?)
            .bind(effect_status_sql(EffectStatus::Receipted))
            .execute(&mut *tx)
            .await?;
            if updated.rows_affected() != 1 {
                tx.rollback().await?;
                return Ok(TransitionCommitOutcome::InvalidPlan);
            }
        }

        maybe_fail(failpoint, WorkflowFailpoint::AfterInvalidations)?;

        tx.commit().await?;
        Ok(TransitionCommitOutcome::Committed)
    }

    /// Atomically claim one eligible effect exactly once.
    ///
    /// # Errors
    ///
    /// Returns an error when the transaction or row decoding fails.
    ///
    /// # Panics
    ///
    /// Panics only if the just-inserted claim row disappears before the same transaction reads it back.
    pub async fn claim_effect(
        &self,
        request: &DurableClaimRequest,
    ) -> WorkflowRepositoryResult<ClaimEffectResult> {
        let mut tx = self.pool.begin().await?;

        let insert_claim = sqlx::query(
            "INSERT INTO workflow_claims \
             (effect_id, workflow_id, declared_workflow_version, generation, claim_token, worker_id, lease_until, issued_at, revoked_at) \
             SELECT e.id, e.workflow_id, e.declared_workflow_version, e.generation, ?1, ?2, ?3, ?4, NULL \
             FROM workflow_effects e \
             JOIN workflows w ON w.id = e.workflow_id \
             LEFT JOIN workflow_claims c ON c.effect_id = e.id \
             WHERE e.workflow_id = ?5 AND e.id = ?6 \
               AND w.authority = 'engine_protocol' \
               AND w.execution_mode = 'authoritative' \
               AND w.status = 'active' \
               AND e.status = 'eligible' \
               AND e.pending_reconciliation = 0 \
               AND e.generation = w.generation \
               AND c.effect_id IS NULL",
        )
        .bind(&request.claim_token)
        .bind(&request.worker_id)
        .bind(request.lease_until.to_rfc3339())
        .bind(request.now.to_rfc3339())
        .bind(&request.workflow_id)
        .bind(&request.effect_id)
        .execute(&mut *tx)
        .await;

        match insert_claim {
            Ok(done) if done.rows_affected() == 1 => {}
            Ok(_) => {
                tx.rollback().await?;
                return Ok(ClaimEffectResult::Ineligible);
            }
            Err(err) if is_unique_constraint(&err) || is_busy_or_locked(&err) => {
                tx.rollback().await?;
                return Ok(ClaimEffectResult::Contended);
            }
            Err(err) => {
                tx.rollback().await?;
                return Err(err.into());
            }
        }

        let authority = load_claim_authority(&mut tx, &request.workflow_id, &request.effect_id)
            .await?
            .expect("inserted claim must be readable");
        let attempt = insert_attempt_for_claim(&mut tx, &authority).await?;

        let updated = sqlx::query(
            "UPDATE workflow_effects \
             SET status = 'claimed' \
             WHERE id = ?1 AND workflow_id = ?2 AND declared_workflow_version = ?3 \
               AND generation = ?4 AND status = 'eligible'",
        )
        .bind(&authority.effect_id)
        .bind(&authority.workflow_id)
        .bind(to_i64(
            authority.declared_workflow_version,
            WorkflowRepositoryError::VersionOutOfRange,
        )?)
        .bind(to_i64(
            authority.generation,
            WorkflowRepositoryError::GenerationOutOfRange,
        )?)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() != 1 {
            tx.rollback().await?;
            return Ok(ClaimEffectResult::Contended);
        }

        tx.commit().await?;
        Ok(ClaimEffectResult::Claimed {
            authority,
            attempt: Box::new(attempt),
        })
    }

    /// Renew an exact live claim authority while strictly extending its lease.
    ///
    /// # Errors
    ///
    /// Returns an error when the update fails.
    pub async fn renew_claim(
        &self,
        renewal: &DurableClaimRenewal,
    ) -> WorkflowRepositoryResult<RenewClaimResult> {
        if renewal.lease_until <= renewal.now
            || renewal.lease_until <= renewal.authority.lease_until
        {
            return Ok(RenewClaimResult::StaleAuthority);
        }

        let updated = sqlx::query(
            "UPDATE workflow_claims \
             SET lease_until = ?1 \
             WHERE effect_id = ?2 AND workflow_id = ?3 AND declared_workflow_version = ?4 \
               AND generation = ?5 AND claim_token = ?6 AND worker_id = ?7 \
               AND lease_until = ?8 AND issued_at = ?9 AND lease_until > ?10",
        )
        .bind(renewal.lease_until.to_rfc3339())
        .bind(&renewal.authority.effect_id)
        .bind(&renewal.authority.workflow_id)
        .bind(to_i64(
            renewal.authority.declared_workflow_version,
            WorkflowRepositoryError::VersionOutOfRange,
        )?)
        .bind(to_i64(
            renewal.authority.generation,
            WorkflowRepositoryError::GenerationOutOfRange,
        )?)
        .bind(&renewal.authority.claim_token)
        .bind(&renewal.authority.worker_id)
        .bind(renewal.authority.lease_until.to_rfc3339())
        .bind(renewal.authority.issued_at.to_rfc3339())
        .bind(renewal.now.to_rfc3339())
        .execute(&self.pool)
        .await?;

        if updated.rows_affected() != 1 {
            return Ok(RenewClaimResult::StaleAuthority);
        }

        let mut authority = renewal.authority.clone();
        authority.lease_until = renewal.lease_until;
        Ok(RenewClaimResult::Renewed { authority })
    }

    /// Replace an expired exact claim authority with a fresh authority and new attempt.
    ///
    /// # Errors
    ///
    /// Returns an error when the transaction or row decoding fails.
    ///
    /// # Panics
    ///
    /// Panics only if the just-updated claim row disappears before the same transaction reads it back.
    #[allow(clippy::too_many_lines)]
    pub async fn take_over_expired_claim(
        &self,
        takeover: &DurableClaimTakeover,
    ) -> WorkflowRepositoryResult<TakeOverExpiredClaimResult> {
        if takeover.lease_until <= takeover.now {
            return Ok(TakeOverExpiredClaimResult::StaleAuthority);
        }

        let mut tx = self.pool.begin().await?;

        let updated_claim = sqlx::query(
            "UPDATE workflow_claims \
             SET claim_token = ?1, worker_id = ?2, lease_until = ?3, issued_at = ?4, revoked_at = NULL \
             WHERE effect_id = ?5 AND workflow_id = ?6 AND declared_workflow_version = ?7 \
               AND generation = ?8 AND claim_token = ?9 AND worker_id = ?10 \
               AND lease_until = ?11 AND issued_at = ?12 AND lease_until <= ?13 \
               AND EXISTS (\
                   SELECT 1 FROM workflow_effects e \
                   JOIN workflows w ON w.id = e.workflow_id \
                   WHERE e.id = workflow_claims.effect_id AND e.workflow_id = workflow_claims.workflow_id \
                     AND e.declared_workflow_version = workflow_claims.declared_workflow_version \
                     AND e.generation = workflow_claims.generation \
                     AND w.authority = 'engine_protocol' \
                     AND w.execution_mode = 'authoritative' \
                     AND w.status = 'active' \
                     AND e.status = 'claimed'\
               )",
        )
        .bind(&takeover.replacement_claim_token)
        .bind(&takeover.replacement_worker_id)
        .bind(takeover.lease_until.to_rfc3339())
        .bind(takeover.now.to_rfc3339())
        .bind(&takeover.authority.effect_id)
        .bind(&takeover.authority.workflow_id)
        .bind(to_i64(
            takeover.authority.declared_workflow_version,
            WorkflowRepositoryError::VersionOutOfRange,
        )?)
        .bind(to_i64(
            takeover.authority.generation,
            WorkflowRepositoryError::GenerationOutOfRange,
        )?)
        .bind(&takeover.authority.claim_token)
        .bind(&takeover.authority.worker_id)
        .bind(takeover.authority.lease_until.to_rfc3339())
        .bind(takeover.authority.issued_at.to_rfc3339())
        .bind(takeover.now.to_rfc3339())
        .execute(&mut *tx)
        .await?;

        if updated_claim.rows_affected() != 1 {
            tx.rollback().await?;
            return Ok(TakeOverExpiredClaimResult::StaleAuthority);
        }

        let authority = load_claim_authority(
            &mut tx,
            &takeover.authority.workflow_id,
            &takeover.authority.effect_id,
        )
        .await?
        .expect("updated claim must be readable");

        sqlx::query(
            "UPDATE workflow_attempts \
             SET status = 'authority_lost' \
             WHERE effect_id = ?1 AND workflow_id = ?2 AND declared_workflow_version = ?3 \
               AND generation = ?4 AND claim_token = ?5 AND claim_worker_id = ?6 \
               AND claim_lease_until = ?7 AND claim_issued_at = ?8 AND status = 'begun'",
        )
        .bind(&takeover.authority.effect_id)
        .bind(&takeover.authority.workflow_id)
        .bind(to_i64(
            takeover.authority.declared_workflow_version,
            WorkflowRepositoryError::VersionOutOfRange,
        )?)
        .bind(to_i64(
            takeover.authority.generation,
            WorkflowRepositoryError::GenerationOutOfRange,
        )?)
        .bind(&takeover.authority.claim_token)
        .bind(&takeover.authority.worker_id)
        .bind(takeover.authority.lease_until.to_rfc3339())
        .bind(takeover.authority.issued_at.to_rfc3339())
        .execute(&mut *tx)
        .await?;

        let attempt = insert_attempt_for_claim(&mut tx, &authority).await?;

        let updated_effect = sqlx::query(
            "UPDATE workflow_effects \
             SET status = 'claimed' \
             WHERE id = ?1 AND workflow_id = ?2 AND declared_workflow_version = ?3 \
               AND generation = ?4 AND status = 'claimed'",
        )
        .bind(&authority.effect_id)
        .bind(&authority.workflow_id)
        .bind(to_i64(
            authority.declared_workflow_version,
            WorkflowRepositoryError::VersionOutOfRange,
        )?)
        .bind(to_i64(
            authority.generation,
            WorkflowRepositoryError::GenerationOutOfRange,
        )?)
        .execute(&mut *tx)
        .await?;
        if updated_effect.rows_affected() != 1 {
            tx.rollback().await?;
            return Ok(TakeOverExpiredClaimResult::Ineligible);
        }

        let flagged = sqlx::query(
            "UPDATE workflow_effects \
             SET pending_reconciliation = 1 \
             WHERE id = ?1 AND workflow_id = ?2 AND declared_workflow_version = ?3 \
               AND generation = ?4 AND status = 'claimed' AND pending_reconciliation = 0",
        )
        .bind(&authority.effect_id)
        .bind(&authority.workflow_id)
        .bind(to_i64(
            authority.declared_workflow_version,
            WorkflowRepositoryError::VersionOutOfRange,
        )?)
        .bind(to_i64(
            authority.generation,
            WorkflowRepositoryError::GenerationOutOfRange,
        )?)
        .execute(&mut *tx)
        .await?;
        if flagged.rows_affected() != 1 {
            tx.rollback().await?;
            return Ok(TakeOverExpiredClaimResult::Ineligible);
        }

        tx.commit().await?;
        Ok(TakeOverExpiredClaimResult::Claimed {
            authority,
            attempt: Box::new(attempt),
        })
    }
}

#[derive(Debug)]
struct WorkflowCommitContext {
    authority: SemanticAuthority,
    execution_mode: WorkflowExecutionMode,
    selection_id: String,
    generation: u64,
    status: WorkflowStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkflowExecutionMode {
    Authoritative,
    Shadow,
}

async fn selection_accepts_external(
    tx: &mut sqlx::SqliteConnection,
    acceptance: &DurableWorkflowAcceptance,
) -> WorkflowRepositoryResult<bool> {
    let Some(gate) = sqlx::query(
        "SELECT accepting, external_acceptance_enabled \
         FROM workflow_protocol_selections WHERE id = ?1 AND profile_id = ?2 \
         AND protocol_version = ?3 AND authority = ?4",
    )
    .bind(&acceptance.selection_id)
    .bind(&acceptance.profile_id)
    .bind(i64::from(acceptance.protocol_version))
    .bind(authority_sql(acceptance.authority.authority))
    .fetch_optional(&mut *tx)
    .await?
    else {
        return Ok(false);
    };

    let accepting: i64 = gate.get("accepting");
    let external_enabled: i64 = gate.get("external_acceptance_enabled");
    if accepting == 0 || external_enabled == 0 {
        return Ok(false);
    }

    selection_supports_codec_and_executor(
        &mut *tx,
        &acceptance.selection_id,
        [
            &acceptance.workflow_snapshot.codec,
            &acceptance.handle_receipt.codec,
        ],
        Some(&acceptance.executor_kind),
    )
    .await
}

async fn resolve_external_acceptance_race(
    err: sqlx::Error,
    pool: &SqlitePool,
    acceptance: &DurableWorkflowAcceptance,
) -> WorkflowRepositoryResult<ExternalAcceptanceResult> {
    if !is_unique_constraint(&err) && !is_busy_or_locked(&err) {
        return Err(WorkflowRepositoryError::Sqlx(err));
    }

    for _ in 0..20 {
        if let Some(existing) = lookup_existing_binding(pool, acceptance).await? {
            return Ok(existing);
        }
    }

    Ok(ExternalAcceptanceResult::Retryable)
}

async fn load_workflow_for_commit(
    tx: &mut sqlx::SqliteConnection,
    workflow_id: &str,
) -> WorkflowRepositoryResult<Option<WorkflowCommitContext>> {
    let row = sqlx::query(
        "SELECT authority, execution_mode, authoritative_workflow_id, protocol_selection_id, generation, status \
         FROM workflows WHERE id = ?1",
    )
    .bind(workflow_id)
    .fetch_optional(&mut *tx)
    .await?;

    Ok(row.map(|row| WorkflowCommitContext {
        authority: parse_authority_sql(row.get::<String, _>("authority").as_str())
            .expect("workflows.authority is constrained to known values"),
        execution_mode: parse_workflow_execution_mode_sql(
            row.get::<String, _>("execution_mode").as_str(),
        )
        .expect("workflows.execution_mode is constrained to known values"),
        selection_id: row.get("protocol_selection_id"),
        generation: row
            .get::<i64, _>("generation")
            .try_into()
            .expect("workflow generation fits u64"),
        status: parse_workflow_status_sql(row.get::<String, _>("status").as_str())
            .expect("workflows.status is constrained to known values"),
    }))
}

#[allow(clippy::too_many_lines)]
async fn validate_transition_plan(
    tx: &mut Transaction<'_, Sqlite>,
    commit: &DurableWorkflowTransitionCommit,
    workflow: &WorkflowCommitContext,
) -> WorkflowRepositoryResult<()> {
    if commit.next_version != commit.expected_from_version + 1 {
        return Err(WorkflowRepositoryError::InvalidPlan(
            "transition must advance by exactly one version",
        ));
    }
    if workflow.authority != SemanticAuthority::EngineProtocol {
        return Err(WorkflowRepositoryError::InvalidPlan(
            "transition workflow must use engine authority",
        ));
    }
    if workflow.execution_mode != WorkflowExecutionMode::Authoritative {
        return Err(WorkflowRepositoryError::InvalidPlan(
            "transition workflow must be authoritative",
        ));
    }
    if workflow.status != WorkflowStatus::Active {
        return Err(WorkflowRepositoryError::InvalidPlan(
            "ordinary transition requires active workflow status",
        ));
    }
    if commit.next_generation != workflow.generation {
        return Err(WorkflowRepositoryError::InvalidPlan(
            "ordinary transition must preserve workflow generation",
        ));
    }

    for effect in &commit.effects {
        if effect.generation != workflow.generation {
            return Err(WorkflowRepositoryError::InvalidPlan(
                "ordinary transition effects must use current generation",
            ));
        }
    }

    let mut effect_ids = HashSet::new();
    let mut barrier_ids = HashSet::new();
    let mut invalidation_ids = HashSet::new();
    let mut owed_ids = HashSet::new();
    let mut owed_inbox_ids = HashSet::new();
    let mut dependency_pairs = HashSet::new();
    let mut member_pairs = HashSet::new();

    for effect in &commit.effects {
        if !effect_ids.insert(effect.effect_id.as_str()) {
            return Err(WorkflowRepositoryError::InvalidPlan("duplicate effect id"));
        }
    }
    for barrier in &commit.barriers {
        if !barrier_ids.insert(barrier.barrier_id.as_str()) {
            return Err(WorkflowRepositoryError::InvalidPlan("duplicate barrier id"));
        }
        if matches!(barrier.status, BarrierStatus::Waiting) != barrier.satisfied_at.is_none() {
            return Err(WorkflowRepositoryError::InvalidPlan(
                "barrier satisfied timestamp must match barrier status",
            ));
        }
    }
    for dependency in &commit.dependencies {
        if !dependency_pairs.insert((
            dependency.effect_id.as_str(),
            dependency.dependency_effect_id.as_str(),
        )) {
            return Err(WorkflowRepositoryError::InvalidPlan(
                "duplicate dependency ref",
            ));
        }
        if dependency.effect_id == dependency.dependency_effect_id {
            return Err(WorkflowRepositoryError::InvalidPlan(
                "self dependency is invalid",
            ));
        }
        if !effect_ids.contains(dependency.effect_id.as_str())
            || !effect_ids.contains(dependency.dependency_effect_id.as_str())
        {
            return Err(WorkflowRepositoryError::InvalidPlan(
                "dependency must reference declared effects",
            ));
        }
    }

    validate_dependency_dag_acyclic(&commit.effects, &commit.dependencies)?;

    let barrier_map: HashMap<&str, &DurableBarrierRecord> = commit
        .barriers
        .iter()
        .map(|b| (b.barrier_id.as_str(), b))
        .collect();
    let effect_map: HashMap<&str, &DurableEffectRecord> = commit
        .effects
        .iter()
        .map(|e| (e.effect_id.as_str(), e))
        .collect();
    let mut barrier_member_counts: HashMap<&str, usize> = HashMap::new();
    for member in &commit.barrier_members {
        if !member_pairs.insert((member.barrier_id.as_str(), member.effect_id.as_str())) {
            return Err(WorkflowRepositoryError::InvalidPlan(
                "duplicate barrier member",
            ));
        }
        let Some(barrier) = barrier_map.get(member.barrier_id.as_str()) else {
            return Err(WorkflowRepositoryError::InvalidPlan(
                "barrier member references unknown barrier",
            ));
        };
        let Some(effect) = effect_map.get(member.effect_id.as_str()) else {
            return Err(WorkflowRepositoryError::InvalidPlan(
                "barrier member references unknown effect",
            ));
        };
        if barrier.status != BarrierStatus::Waiting {
            return Err(WorkflowRepositoryError::InvalidPlan(
                "ordinary transition barriers must begin waiting",
            ));
        }
        if effect.role != EffectRole::Required {
            return Err(WorkflowRepositoryError::InvalidPlan(
                "barrier members must target required effects",
            ));
        }
        if effect.status == EffectStatus::Receipted || effect.status == EffectStatus::Invalidated {
            return Err(WorkflowRepositoryError::InvalidPlan(
                "barrier members must target live required effects",
            ));
        }
        if !receipt_family_matches_role(member.receipt_family, effect.role) {
            return Err(WorkflowRepositoryError::InvalidPlan(
                "barrier receipt family incompatible with effect role",
            ));
        }
        *barrier_member_counts
            .entry(member.barrier_id.as_str())
            .or_default() += 1;
    }
    for barrier in &commit.barriers {
        if barrier_member_counts
            .get(barrier.barrier_id.as_str())
            .copied()
            .unwrap_or(0)
            == 0
        {
            return Err(WorkflowRepositoryError::InvalidPlan(
                "barrier must have at least one member",
            ));
        }
    }

    for invalidation in &commit.invalidations {
        if !invalidation_ids.insert(invalidation.effect_id.as_str()) {
            return Err(WorkflowRepositoryError::InvalidPlan(
                "duplicate invalidation target",
            ));
        }
        let Some(row) = sqlx::query(
            "SELECT workflow_id, declared_workflow_version, generation, status, ambiguity_policy \
             FROM workflow_effects WHERE id = ?1",
        )
        .bind(&invalidation.effect_id)
        .fetch_optional(&mut **tx)
        .await?
        else {
            return Err(WorkflowRepositoryError::InvalidPlan(
                "invalidation target does not exist",
            ));
        };
        let workflow_id: String = row.get("workflow_id");
        if workflow_id != commit.workflow_id {
            return Err(WorkflowRepositoryError::InvalidPlan(
                "invalidation target belongs to different workflow",
            ));
        }
        let declared_workflow_version: u64 = row
            .get::<i64, _>("declared_workflow_version")
            .try_into()
            .expect("declared workflow version fits u64");
        if declared_workflow_version != invalidation.expected_declared_workflow_version {
            return Err(WorkflowRepositoryError::InvalidPlan(
                "invalidation target declared workflow version mismatch",
            ));
        }
        let generation: u64 = row
            .get::<i64, _>("generation")
            .try_into()
            .expect("generation fits u64");
        if generation != invalidation.expected_generation {
            return Err(WorkflowRepositoryError::InvalidPlan(
                "invalidation target generation mismatch",
            ));
        }
        let status: String = row.get("status");
        if status == effect_status_sql(EffectStatus::Receipted) {
            return Err(WorkflowRepositoryError::InvalidPlan(
                "receipted effect cannot be invalidated",
            ));
        }
        let ambiguity_policy: String = row.get("ambiguity_policy");
        if ambiguity_policy == effect_ambiguity_sql(EffectAmbiguity::ManualResolution) {
            let manual_resolution_required: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM workflow_manual_resolutions \
                 WHERE workflow_id = ?1 AND effect_id = ?2 AND status = 'required'",
            )
            .bind(&commit.workflow_id)
            .bind(&invalidation.effect_id)
            .fetch_one(&mut **tx)
            .await?;
            if manual_resolution_required > 0 {
                return Err(WorkflowRepositoryError::InvalidPlan(
                    "manual-resolution effect with required resolution cannot be invalidated",
                ));
            }
        }
    }

    for owed in &commit.owed_acceptances {
        if !owed_ids.insert(owed.owed_acceptance_id.as_str()) {
            return Err(WorkflowRepositoryError::InvalidPlan(
                "duplicate owed acceptance id",
            ));
        }
        if !owed_inbox_ids.insert(owed.reducer_inbox_id.as_str()) {
            return Err(WorkflowRepositoryError::InvalidPlan(
                "duplicate owed acceptance reducer inbox id",
            ));
        }
        let inbox_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM workflow_reducer_inbox \
             WHERE id = ?1 AND workflow_id = ?2 AND delivery_status = 'pending'",
        )
        .bind(&owed.reducer_inbox_id)
        .bind(&commit.workflow_id)
        .fetch_one(&mut **tx)
        .await?;
        if inbox_count != 1 {
            return Err(WorkflowRepositoryError::InvalidPlan(
                "owed acceptance must reference existing pending reducer inbox entry",
            ));
        }
    }

    let mut codecs = vec![&commit.snapshot.codec, &commit.event.codec];
    codecs.extend(commit.effects.iter().map(|effect| &effect.codec));
    codecs.extend(
        commit
            .barriers
            .iter()
            .map(|barrier| &barrier.barrier_event.codec),
    );
    codecs.extend(commit.owed_acceptances.iter().map(|owed| &owed.event.codec));
    if !selection_supports_codec_and_executor(tx, &workflow.selection_id, codecs, None).await? {
        return Err(WorkflowRepositoryError::InvalidPlan(
            "commit references codec unsupported by workflow selection",
        ));
    }

    Ok(())
}

async fn selection_supports_codec_and_executor(
    tx: &mut sqlx::SqliteConnection,
    selection_id: &str,
    codecs: impl IntoIterator<Item = &DurableCodecRef>,
    executor_kind: Option<&str>,
) -> WorkflowRepositoryResult<bool> {
    for codec in codecs {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM workflow_profile_codecs \
             WHERE selection_id = ?1 AND codec_family = ?2 AND codec_version = ?3",
        )
        .bind(selection_id)
        .bind(&codec.family)
        .bind(i64::from(codec.version))
        .fetch_one(&mut *tx)
        .await?;
        if count != 1 {
            return Ok(false);
        }
    }

    if let Some(kind) = executor_kind {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM workflow_profile_executors \
             WHERE selection_id = ?1 AND executor_kind = ?2",
        )
        .bind(selection_id)
        .bind(kind)
        .fetch_one(&mut *tx)
        .await?;
        if count != 1 {
            return Ok(false);
        }
    }

    Ok(true)
}

async fn load_claim_authority(
    tx: &mut SqliteConnection,
    workflow_id: &str,
    effect_id: &str,
) -> WorkflowRepositoryResult<Option<DurableClaimAuthority>> {
    let row = sqlx::query(
        "SELECT workflow_id, declared_workflow_version, generation, effect_id, claim_token, worker_id, lease_until, issued_at \
         FROM workflow_claims WHERE workflow_id = ?1 AND effect_id = ?2",
    )
    .bind(workflow_id)
    .bind(effect_id)
    .fetch_optional(&mut *tx)
    .await?;

    Ok(row.map(|row| DurableClaimAuthority {
        workflow_id: row.get("workflow_id"),
        declared_workflow_version: row
            .get::<i64, _>("declared_workflow_version")
            .try_into()
            .expect("declared workflow version fits u64"),
        generation: row
            .get::<i64, _>("generation")
            .try_into()
            .expect("generation fits u64"),
        effect_id: row.get("effect_id"),
        claim_token: row.get("claim_token"),
        worker_id: row.get("worker_id"),
        lease_until: DateTime::parse_from_rfc3339(&row.get::<String, _>("lease_until"))
            .expect("claim lease_until is valid RFC3339")
            .with_timezone(&Utc),
        issued_at: DateTime::parse_from_rfc3339(&row.get::<String, _>("issued_at"))
            .expect("claim issued_at is valid RFC3339")
            .with_timezone(&Utc),
    }))
}

async fn insert_attempt_for_claim(
    tx: &mut SqliteConnection,
    authority: &DurableClaimAuthority,
) -> WorkflowRepositoryResult<DurableAttemptRecord> {
    let ordinal: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(ordinal), -1) + 1 FROM workflow_attempts WHERE effect_id = ?1",
    )
    .bind(&authority.effect_id)
    .fetch_one(&mut *tx)
    .await?;
    let begun_at = authority.issued_at;
    let attempt_id = format!("attempt-{}", uuid::Uuid::new_v4());

    sqlx::query(
        "INSERT INTO workflow_attempts \
         (id, effect_id, workflow_id, declared_workflow_version, generation, claim_token, \
          claim_worker_id, claim_lease_until, claim_issued_at, ordinal, status, begun_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'begun', ?11)",
    )
    .bind(&attempt_id)
    .bind(&authority.effect_id)
    .bind(&authority.workflow_id)
    .bind(to_i64(
        authority.declared_workflow_version,
        WorkflowRepositoryError::VersionOutOfRange,
    )?)
    .bind(to_i64(
        authority.generation,
        WorkflowRepositoryError::GenerationOutOfRange,
    )?)
    .bind(&authority.claim_token)
    .bind(&authority.worker_id)
    .bind(authority.lease_until.to_rfc3339())
    .bind(authority.issued_at.to_rfc3339())
    .bind(ordinal)
    .bind(begun_at.to_rfc3339())
    .execute(&mut *tx)
    .await?;

    Ok(DurableAttemptRecord {
        attempt_id,
        effect_id: authority.effect_id.clone(),
        workflow_id: authority.workflow_id.clone(),
        declared_workflow_version: authority.declared_workflow_version,
        generation: authority.generation,
        ordinal: ordinal.try_into().expect("ordinal fits u64"),
        claim: authority.clone(),
        status: "begun".to_owned(),
        begun_at,
    })
}

fn validate_dependency_dag_acyclic(
    effects: &[DurableEffectRecord],
    dependencies: &[DurableEffectDependencyRecord],
) -> WorkflowRepositoryResult<()> {
    let mut indegree: HashMap<&str, usize> = effects
        .iter()
        .map(|effect| (effect.effect_id.as_str(), 0_usize))
        .collect();
    let mut outgoing: HashMap<&str, Vec<&str>> = HashMap::new();

    for dependency in dependencies {
        outgoing
            .entry(dependency.dependency_effect_id.as_str())
            .or_default()
            .push(dependency.effect_id.as_str());
        *indegree
            .get_mut(dependency.effect_id.as_str())
            .expect("validated dependencies reference declared effects") += 1;
    }

    let mut ready: Vec<&str> = indegree
        .iter()
        .filter_map(|(effect_id, degree)| (*degree == 0).then_some(*effect_id))
        .collect();
    let mut visited = 0usize;

    while let Some(effect_id) = ready.pop() {
        visited += 1;
        if let Some(children) = outgoing.get(effect_id) {
            for child in children {
                let degree = indegree
                    .get_mut(child)
                    .expect("outgoing child references declared effect");
                *degree -= 1;
                if *degree == 0 {
                    ready.push(child);
                }
            }
        }
    }

    if visited != effects.len() {
        return Err(WorkflowRepositoryError::InvalidPlan(
            "effect dependency graph must be acyclic",
        ));
    }

    Ok(())
}

fn receipt_family_matches_role(receipt_family: ReceiptFamily, role: EffectRole) -> bool {
    match receipt_family {
        ReceiptFamily::CurrentGenerationEffect => role == EffectRole::Required,
        ReceiptFamily::CompensationEffect => role == EffectRole::Compensation,
    }
}

fn is_unique_constraint(err: &sqlx::Error) -> bool {
    if let sqlx::Error::Database(db_err) = err {
        return db_err
            .try_downcast_ref::<SqliteError>()
            .is_some_and(DatabaseError::is_unique_violation);
    }
    false
}

fn is_busy_or_locked(err: &sqlx::Error) -> bool {
    if let sqlx::Error::Database(db_err) = err {
        return db_err
            .try_downcast_ref::<SqliteError>()
            .is_some_and(|sqlite_err| {
                sqlite_err
                    .code()
                    .as_deref()
                    .is_some_and(|code| matches!(code, "5" | "6" | "517"))
            });
    }
    false
}

async fn fail_if_configured(
    tx: &mut SqliteConnection,
    configured: Option<WorkflowFailpoint>,
    here: WorkflowFailpoint,
) -> WorkflowRepositoryResult<()> {
    if configured == Some(here) {
        tx.execute("ROLLBACK").await?;
        return Err(WorkflowRepositoryError::Failpoint(here));
    }
    Ok(())
}

async fn insert_protocol_selection(
    tx: &mut Transaction<'_, Sqlite>,
    registration: &DurableProtocolSelectionRegistration,
) -> WorkflowRepositoryResult<()> {
    sqlx::query(
        "INSERT INTO workflow_protocol_selections \
         (id, profile_id, selector_identity, selector_version, protocol_version, authority, \
          accepting, runtime_acceptance_enabled, external_acceptance_enabled, registered_at, drained_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
    )
    .bind(&registration.selection_id)
    .bind(&registration.profile_id)
    .bind(&registration.selector_identity)
    .bind(i64::from(registration.selector_version))
    .bind(i64::from(registration.protocol_version))
    .bind(authority_sql(registration.authority))
    .bind(registration.accepting)
    .bind(registration.runtime_acceptance_enabled)
    .bind(registration.external_acceptance_enabled)
    .bind(registration.registered_at.to_rfc3339())
    .bind(registration.drained_at.map(|ts| ts.to_rfc3339()))
    .execute(&mut **tx)
    .await?;

    for codec in &registration.supported_codecs {
        sqlx::query(
            "INSERT INTO workflow_profile_codecs (selection_id, codec_family, codec_version) \
             VALUES (?1, ?2, ?3)",
        )
        .bind(&registration.selection_id)
        .bind(&codec.family)
        .bind(i64::from(codec.version))
        .execute(&mut **tx)
        .await?;
    }

    for executor_kind in &registration.executor_kinds {
        sqlx::query(
            "INSERT INTO workflow_profile_executors (selection_id, executor_kind) \
             VALUES (?1, ?2)",
        )
        .bind(&registration.selection_id)
        .bind(executor_kind)
        .execute(&mut **tx)
        .await?;
    }

    Ok(())
}

async fn lookup_existing_binding<'e, E>(
    executor: E,
    acceptance: &DurableWorkflowAcceptance,
) -> WorkflowRepositoryResult<Option<ExternalAcceptanceResult>>
where
    E: Executor<'e, Database = Sqlite>,
{
    let existing = sqlx::query(
        "SELECT intent_fingerprint, workflow_id, receipt_codec_family, receipt_codec_version, receipt_payload \
         FROM external_acceptance_bindings WHERE profile_id = ?1 AND protocol_version = ?2 \
         AND authority = ?3 AND authority_scope = ?4 AND idempotency_key = ?5",
    )
    .bind(&acceptance.profile_id)
    .bind(i64::from(acceptance.protocol_version))
    .bind(authority_sql(acceptance.authority.authority))
    .bind(&acceptance.authority.scope)
    .bind(&acceptance.idempotency_key)
    .fetch_optional(executor)
    .await?;

    Ok(existing.map(|row| {
        let existing_fingerprint: String = row.get("intent_fingerprint");
        if existing_fingerprint == acceptance.intent_fingerprint {
            ExternalAcceptanceResult::Replay {
                workflow_id: row.get("workflow_id"),
                handle_receipt: DurablePayload {
                    codec: DurableCodecRef {
                        family: row.get("receipt_codec_family"),
                        version: row
                            .get::<i64, _>("receipt_codec_version")
                            .try_into()
                            .expect("receipt codec version fits u32"),
                    },
                    payload: row.get("receipt_payload"),
                },
            }
        } else {
            ExternalAcceptanceResult::Conflict
        }
    }))
}

fn maybe_fail(
    configured: Option<WorkflowFailpoint>,
    here: WorkflowFailpoint,
) -> WorkflowRepositoryResult<()> {
    if configured == Some(here) {
        return Err(WorkflowRepositoryError::Failpoint(here));
    }
    Ok(())
}

fn to_i64(
    value: u64,
    err: impl FnOnce(u64) -> WorkflowRepositoryError,
) -> WorkflowRepositoryResult<i64> {
    i64::try_from(value).map_err(|_| err(value))
}

fn parse_authority_sql(authority: &str) -> Option<SemanticAuthority> {
    match authority {
        "legacy_protocol" => Some(SemanticAuthority::LegacyProtocol),
        "engine_protocol" => Some(SemanticAuthority::EngineProtocol),
        _ => None,
    }
}

fn parse_workflow_execution_mode_sql(mode: &str) -> Option<WorkflowExecutionMode> {
    match mode {
        "authoritative" => Some(WorkflowExecutionMode::Authoritative),
        "shadow" => Some(WorkflowExecutionMode::Shadow),
        _ => None,
    }
}

fn parse_workflow_status_sql(status: &str) -> Option<WorkflowStatus> {
    match status {
        "active" => Some(WorkflowStatus::Active),
        "cancelling" => Some(WorkflowStatus::Cancelling),
        "cancelled" => Some(WorkflowStatus::Cancelled),
        "deletion_pending" => Some(WorkflowStatus::DeletionPending),
        "completed" => Some(WorkflowStatus::Completed),
        "failed" => Some(WorkflowStatus::Failed),
        _ => None,
    }
}

fn workflow_execution_mode_sql(mode: WorkflowExecutionMode) -> &'static str {
    match mode {
        WorkflowExecutionMode::Authoritative => "authoritative",
        WorkflowExecutionMode::Shadow => "shadow",
    }
}

fn authority_sql(authority: SemanticAuthority) -> &'static str {
    match authority {
        SemanticAuthority::LegacyProtocol => "legacy_protocol",
        SemanticAuthority::EngineProtocol => "engine_protocol",
    }
}

fn workflow_status_sql(status: WorkflowStatus) -> &'static str {
    match status {
        WorkflowStatus::Active => "active",
        WorkflowStatus::Cancelling => "cancelling",
        WorkflowStatus::Cancelled => "cancelled",
        WorkflowStatus::DeletionPending => "deletion_pending",
        WorkflowStatus::Completed => "completed",
        WorkflowStatus::Failed => "failed",
    }
}

fn effect_status_sql(status: EffectStatus) -> &'static str {
    match status {
        EffectStatus::Blocked => "blocked",
        EffectStatus::Eligible => "eligible",
        EffectStatus::Claimed => "claimed",
        EffectStatus::RetryWait => "retry_wait",
        EffectStatus::AmbiguityWait => "ambiguity_wait",
        EffectStatus::Receipted => "receipted",
        EffectStatus::Invalidated => "invalidated",
    }
}

fn barrier_status_sql(status: BarrierStatus) -> &'static str {
    match status {
        BarrierStatus::Waiting => "waiting",
        BarrierStatus::Satisfied => "satisfied",
    }
}

fn effect_role_sql(role: EffectRole) -> &'static str {
    match role {
        EffectRole::Required => "required",
        EffectRole::Optional => "optional",
        EffectRole::Compensation => "compensation",
    }
}

fn effect_ambiguity_sql(ambiguity: EffectAmbiguity) -> &'static str {
    match ambiguity {
        EffectAmbiguity::ObservableReconciliation => "observable_reconciliation",
        EffectAmbiguity::ExternalIdempotency => "external_idempotency",
        EffectAmbiguity::SafeRepeatability => "safe_repeatability",
        EffectAmbiguity::ManualResolution => "manual_resolution",
    }
}

fn receipt_family_sql(family: ReceiptFamily) -> &'static str {
    match family {
        ReceiptFamily::CurrentGenerationEffect => "current_generation_effect",
        ReceiptFamily::CompensationEffect => "compensation_effect",
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::run_pending_migrations;
    use sqlx::sqlite::{
        SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous,
    };
    use std::{path::Path, str::FromStr, sync::Arc};
    use tokio::sync::Barrier;

    async fn setup_conversations_table(pool: &SqlitePool) {
        sqlx::raw_sql(
            "CREATE TABLE IF NOT EXISTS conversations (\
                id TEXT PRIMARY KEY, \
                slug TEXT UNIQUE, \
                cwd TEXT NOT NULL DEFAULT '/tmp', \
                parent_conversation_id TEXT, \
                user_initiated BOOLEAN NOT NULL DEFAULT 1, \
                state TEXT NOT NULL DEFAULT '{\"type\":\"idle\"}', \
                state_updated_at TEXT NOT NULL DEFAULT '2025-01-01', \
                created_at TEXT NOT NULL DEFAULT '2025-01-01', \
                updated_at TEXT NOT NULL DEFAULT '2025-01-01', \
                archived BOOLEAN NOT NULL DEFAULT 0, \
                model TEXT, \
                conv_mode TEXT NOT NULL DEFAULT '{\"mode\":\"Explore\"}', \
                steering_queue TEXT NOT NULL DEFAULT '[]'\
            );\n             CREATE INDEX IF NOT EXISTS idx_conversations_slug ON conversations(slug);\n             CREATE INDEX IF NOT EXISTS idx_conversations_parent ON conversations(parent_conversation_id);\n             CREATE INDEX IF NOT EXISTS idx_conversations_updated ON conversations(updated_at DESC);\n             CREATE TABLE IF NOT EXISTS messages (\
                message_id TEXT PRIMARY KEY, \
                conversation_id TEXT NOT NULL, \
                sequence_id INTEGER NOT NULL, \
                message_type TEXT NOT NULL, \
                content TEXT NOT NULL, \
                display_data TEXT, \
                usage_data TEXT, \
                created_at TEXT NOT NULL, \
                FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE\
            );\n             CREATE INDEX IF NOT EXISTS idx_messages_conversation ON messages(conversation_id, sequence_id);",
        )
        .execute(pool)
        .await
        .unwrap();
    }

    async fn test_pool() -> SqlitePool {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(Duration::from_secs(5))
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await
            .unwrap();
        setup_conversations_table(&pool).await;
        run_pending_migrations(&pool).await.unwrap();
        pool
    }

    async fn file_backed_test_pool(max_connections: u32) -> SqlitePool {
        let unique = format!(
            "{}-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("unnamed"),
            uuid::Uuid::new_v4()
        );
        let path = std::env::temp_dir().join(format!("phoenix-db-workflow-{unique}.sqlite"));
        let url = sqlite_file_url(&path);
        let opts = SqliteConnectOptions::from_str(&url)
            .unwrap()
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(Duration::from_secs(5))
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(max_connections)
            .connect_with(opts)
            .await
            .unwrap();
        setup_conversations_table(&pool).await;
        run_pending_migrations(&pool).await.unwrap();
        pool
    }

    fn sqlite_file_url(path: &Path) -> String {
        format!("sqlite:{}", path.display())
    }

    fn registration(accepting: bool, external: bool) -> DurableProtocolSelectionRegistration {
        DurableProtocolSelectionRegistration {
            selection_id: "sel-1".to_owned(),
            profile_id: "wake".to_owned(),
            selector_identity: "wake-selector".to_owned(),
            selector_version: 1,
            protocol_version: 1,
            authority: SemanticAuthority::EngineProtocol,
            accepting,
            runtime_acceptance_enabled: true,
            external_acceptance_enabled: external,
            registered_at: Utc::now(),
            drained_at: (!accepting).then(Utc::now),
            supported_codecs: vec![
                DurableCodecRef {
                    family: "snapshot".to_owned(),
                    version: 1,
                },
                DurableCodecRef {
                    family: "snapshot".to_owned(),
                    version: 2,
                },
                DurableCodecRef {
                    family: "handle".to_owned(),
                    version: 1,
                },
                DurableCodecRef {
                    family: "event".to_owned(),
                    version: 1,
                },
                DurableCodecRef {
                    family: "intent".to_owned(),
                    version: 1,
                },
                DurableCodecRef {
                    family: "barrier-event".to_owned(),
                    version: 1,
                },
                DurableCodecRef {
                    family: "owed-event".to_owned(),
                    version: 1,
                },
            ],
            executor_kinds: vec!["wake".to_owned()],
        }
    }

    fn acceptance() -> DurableWorkflowAcceptance {
        DurableWorkflowAcceptance {
            selection_id: "sel-1".to_owned(),
            profile_id: "wake".to_owned(),
            protocol_version: 1,
            authority: DurableExternalAuthority {
                authority: SemanticAuthority::EngineProtocol,
                scope: "repo:owner/name".to_owned(),
            },
            idempotency_key: "idem-1".to_owned(),
            intent_fingerprint: "fp-1".to_owned(),
            binding_id: "binding-1".to_owned(),
            workflow_id: "wf-1".to_owned(),
            accepted_at: Utc::now(),
            workflow_snapshot: DurablePayload {
                codec: DurableCodecRef {
                    family: "snapshot".to_owned(),
                    version: 1,
                },
                payload: "snapshot-payload".to_owned(),
            },
            handle_receipt: DurablePayload {
                codec: DurableCodecRef {
                    family: "handle".to_owned(),
                    version: 1,
                },
                payload: "handle-payload".to_owned(),
            },
            executor_kind: "wake".to_owned(),
        }
    }

    async fn assert_no_orphan_workflows(pool: &SqlitePool) {
        let orphaned: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM workflows w \
             LEFT JOIN external_acceptance_bindings b ON b.workflow_id = w.id \
             WHERE b.workflow_id IS NULL",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        assert_eq!(orphaned, 0);
    }

    async fn run_concurrent_acceptances<F>(
        first: DurableWorkflowAcceptance,
        second: DurableWorkflowAcceptance,
        assert_results: F,
    ) where
        F: Fn(ExternalAcceptanceResult, ExternalAcceptanceResult) + Send + Sync,
    {
        let pool = file_backed_test_pool(4).await;
        let repo = Arc::new(WorkflowRepository::new(pool.clone()));
        repo.register_protocol_selection(&registration(true, true))
            .await
            .unwrap();

        let barrier = Arc::new(Barrier::new(2));
        let repo_a = Arc::clone(&repo);
        let barrier_a = Arc::clone(&barrier);
        let first_task = tokio::spawn(async move {
            barrier_a.wait().await;
            repo_a.accept_external_workflow(&first).await
        });

        let repo_b = Arc::clone(&repo);
        let barrier_b = Arc::clone(&barrier);
        let second_task = tokio::spawn(async move {
            barrier_b.wait().await;
            repo_b.accept_external_workflow(&second).await
        });

        let first_result = first_task.await.unwrap();
        let second_result = second_task.await.unwrap();
        let first_result = first_result.unwrap_or_else(|err| panic!("first task failed: {err:?}"));
        let second_result =
            second_result.unwrap_or_else(|err| panic!("second task failed: {err:?}"));
        assert_results(first_result, second_result);
        assert_no_orphan_workflows(&pool).await;
    }

    fn claim_request(now: DateTime<Utc>, lease_until: DateTime<Utc>) -> DurableClaimRequest {
        DurableClaimRequest {
            workflow_id: "wf-1".to_owned(),
            effect_id: "eff-1".to_owned(),
            claim_token: "claim-1".to_owned(),
            worker_id: "worker-1".to_owned(),
            lease_until,
            now,
        }
    }

    async fn seed_claimable_effect(repo: &WorkflowRepository) {
        register_and_accept(repo).await;
        let outcome = repo
            .persist_transition_plan(&transition_commit())
            .await
            .unwrap();
        assert_eq!(outcome, TransitionCommitOutcome::Committed);
    }

    fn transition_commit() -> DurableWorkflowTransitionCommit {
        DurableWorkflowTransitionCommit {
            transition_id: "tr-1".to_owned(),
            workflow_id: "wf-1".to_owned(),
            expected_from_version: 0,
            next_version: 1,
            next_generation: 0,
            committed_at: Utc::now(),
            workflow_status: WorkflowStatus::Active,
            snapshot: DurablePayload {
                codec: DurableCodecRef {
                    family: "snapshot".to_owned(),
                    version: 2,
                },
                payload: "snapshot-v1".to_owned(),
            },
            event: DurablePayload {
                codec: DurableCodecRef {
                    family: "event".to_owned(),
                    version: 1,
                },
                payload: "event-payload".to_owned(),
            },
            effects: vec![
                DurableEffectRecord {
                    effect_id: "eff-1".to_owned(),
                    family: "wake".to_owned(),
                    kind: "register".to_owned(),
                    codec: DurableCodecRef {
                        family: "intent".to_owned(),
                        version: 1,
                    },
                    role: EffectRole::Required,
                    ambiguity_policy: EffectAmbiguity::ObservableReconciliation,
                    intent_payload: "intent-1".to_owned(),
                    next_eligible_at: None,
                    destructive_resource: None,
                    generation: 0,
                    status: EffectStatus::Eligible,
                },
                DurableEffectRecord {
                    effect_id: "eff-2".to_owned(),
                    family: "wake".to_owned(),
                    kind: "observe_handle".to_owned(),
                    codec: DurableCodecRef {
                        family: "intent".to_owned(),
                        version: 1,
                    },
                    role: EffectRole::Required,
                    ambiguity_policy: EffectAmbiguity::ObservableReconciliation,
                    intent_payload: "intent-2".to_owned(),
                    next_eligible_at: None,
                    destructive_resource: Some("repo:owner/name".to_owned()),
                    generation: 0,
                    status: EffectStatus::Blocked,
                },
            ],
            dependencies: vec![DurableEffectDependencyRecord {
                effect_id: "eff-2".to_owned(),
                dependency_effect_id: "eff-1".to_owned(),
            }],
            barriers: vec![DurableBarrierRecord {
                barrier_id: "bar-1".to_owned(),
                status: BarrierStatus::Waiting,
                satisfied_at: None,
                barrier_event: DurablePayload {
                    codec: DurableCodecRef {
                        family: "barrier-event".to_owned(),
                        version: 1,
                    },
                    payload: "barrier-payload".to_owned(),
                },
            }],
            barrier_members: vec![DurableBarrierMemberRecord {
                barrier_id: "bar-1".to_owned(),
                effect_id: "eff-1".to_owned(),
                receipt_family: ReceiptFamily::CurrentGenerationEffect,
            }],
            invalidations: vec![],
            owed_acceptances: vec![],
        }
    }

    async fn register_and_accept(repo: &WorkflowRepository) {
        repo.register_protocol_selection(&registration(true, true))
            .await
            .unwrap();
        let result = repo.accept_external_workflow(&acceptance()).await.unwrap();
        assert!(matches!(result, ExternalAcceptanceResult::New { .. }));
    }

    #[tokio::test]
    async fn claim_effect_has_one_winner_under_competition() {
        for _ in 0..10 {
            let pool_a = file_backed_test_pool(2).await;
            let repo_a = Arc::new(WorkflowRepository::new(pool_a.clone()));
            seed_claimable_effect(&repo_a).await;

            let db_path: String = sqlx::query("PRAGMA database_list")
                .fetch_all(&pool_a)
                .await
                .unwrap()
                .into_iter()
                .find_map(|row| {
                    let name: String = row.get(1);
                    (name == "main").then(|| row.get::<String, _>(2))
                })
                .unwrap();
            let opts = SqliteConnectOptions::from_str(&sqlite_file_url(Path::new(&db_path)))
                .unwrap()
                .journal_mode(SqliteJournalMode::Wal)
                .synchronous(SqliteSynchronous::Normal)
                .busy_timeout(Duration::from_secs(5))
                .foreign_keys(true);
            let pool_b = SqlitePoolOptions::new()
                .max_connections(2)
                .connect_with(opts)
                .await
                .unwrap();
            let repo_b = Arc::new(WorkflowRepository::new(pool_b));

            let now = Utc::now();
            let barrier = Arc::new(Barrier::new(2));
            let mut req_b = claim_request(now, now + chrono::Duration::seconds(30));
            req_b.claim_token = "claim-2".to_owned();
            req_b.worker_id = "worker-2".to_owned();

            let t1 = {
                let repo = Arc::clone(&repo_a);
                let barrier = Arc::clone(&barrier);
                let req = claim_request(now, now + chrono::Duration::seconds(30));
                tokio::spawn(async move {
                    barrier.wait().await;
                    repo.claim_effect(&req).await.unwrap()
                })
            };
            let t2 = {
                let repo = Arc::clone(&repo_b);
                let barrier = Arc::clone(&barrier);
                tokio::spawn(async move {
                    barrier.wait().await;
                    repo.claim_effect(&req_b).await.unwrap()
                })
            };

            let left = t1.await.unwrap();
            let right = t2.await.unwrap();
            match (left, right) {
                (
                    ClaimEffectResult::Claimed { authority, attempt },
                    ClaimEffectResult::Contended | ClaimEffectResult::Ineligible,
                )
                | (
                    ClaimEffectResult::Contended | ClaimEffectResult::Ineligible,
                    ClaimEffectResult::Claimed { authority, attempt },
                ) => {
                    assert_eq!(authority.effect_id, "eff-1");
                    assert_eq!(attempt.ordinal, 0);
                }
                other => panic!("expected one winner, got {other:?}"),
            }

            let claims: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM workflow_claims WHERE effect_id = 'eff-1'",
            )
            .fetch_one(&pool_a)
            .await
            .unwrap();
            let attempts: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM workflow_attempts WHERE effect_id = 'eff-1'",
            )
            .fetch_one(&pool_a)
            .await
            .unwrap();
            assert_eq!(claims, 1);
            assert_eq!(attempts, 1);
        }
    }

    #[tokio::test]
    async fn renew_rejects_shorten_and_takeover_rejects_pre_expiry() {
        let pool = test_pool().await;
        let repo = WorkflowRepository::new(pool.clone());
        seed_claimable_effect(&repo).await;

        let now = Utc::now();
        let claim = repo
            .claim_effect(&claim_request(now, now + chrono::Duration::seconds(30)))
            .await
            .unwrap();
        let (authority, _) = match claim {
            ClaimEffectResult::Claimed { authority, attempt } => (authority, attempt),
            other @ (ClaimEffectResult::Ineligible | ClaimEffectResult::Contended) => {
                panic!("expected claimed, got {other:?}")
            }
        };

        let shortened = repo
            .renew_claim(&DurableClaimRenewal {
                authority: authority.clone(),
                now: now + chrono::Duration::seconds(5),
                lease_until: now + chrono::Duration::seconds(20),
            })
            .await
            .unwrap();
        assert_eq!(shortened, RenewClaimResult::StaleAuthority);

        let pre_expiry = repo
            .take_over_expired_claim(&DurableClaimTakeover {
                authority: authority.clone(),
                replacement_claim_token: "claim-2".to_owned(),
                replacement_worker_id: "worker-2".to_owned(),
                now: now + chrono::Duration::seconds(29),
                lease_until: now + chrono::Duration::seconds(60),
            })
            .await
            .unwrap();
        assert_eq!(pre_expiry, TakeOverExpiredClaimResult::StaleAuthority);
    }

    #[tokio::test]
    async fn takeover_allows_exact_expiry_and_old_authority_no_longer_matches() {
        let pool = test_pool().await;
        let repo = WorkflowRepository::new(pool.clone());
        seed_claimable_effect(&repo).await;

        let now = Utc::now();
        let claim = repo
            .claim_effect(&claim_request(now, now + chrono::Duration::seconds(30)))
            .await
            .unwrap();
        let authority = match claim {
            ClaimEffectResult::Claimed { authority, .. } => authority,
            other @ (ClaimEffectResult::Ineligible | ClaimEffectResult::Contended) => {
                panic!("expected claimed, got {other:?}")
            }
        };

        let takeover_now = authority.lease_until;
        let takeover = repo
            .take_over_expired_claim(&DurableClaimTakeover {
                authority: authority.clone(),
                replacement_claim_token: "claim-2".to_owned(),
                replacement_worker_id: "worker-2".to_owned(),
                now: takeover_now,
                lease_until: takeover_now + chrono::Duration::seconds(30),
            })
            .await
            .unwrap();
        let (replacement, attempt) = match takeover {
            TakeOverExpiredClaimResult::Claimed { authority, attempt } => (authority, attempt),
            other @ (TakeOverExpiredClaimResult::Ineligible
            | TakeOverExpiredClaimResult::StaleAuthority) => {
                panic!("expected takeover claim, got {other:?}")
            }
        };
        assert_eq!(attempt.ordinal, 1);
        assert_eq!(replacement.claim_token, "claim-2");

        let renewed_old = repo
            .renew_claim(&DurableClaimRenewal {
                authority: authority.clone(),
                now: takeover_now,
                lease_until: takeover_now + chrono::Duration::seconds(60),
            })
            .await
            .unwrap();
        assert_eq!(renewed_old, RenewClaimResult::StaleAuthority);

        let current_claim = sqlx::query(
            "SELECT claim_token, worker_id, lease_until FROM workflow_claims WHERE effect_id = 'eff-1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(current_claim.get::<String, _>("claim_token"), "claim-2");
        assert_eq!(current_claim.get::<String, _>("worker_id"), "worker-2");

        let pending_reconciliation: i64 = sqlx::query_scalar(
            "SELECT pending_reconciliation FROM workflow_effects WHERE id = 'eff-1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(pending_reconciliation, 1);

        let claim_after_takeover = repo
            .claim_effect(&DurableClaimRequest {
                workflow_id: "wf-1".to_owned(),
                effect_id: "eff-1".to_owned(),
                claim_token: "claim-3".to_owned(),
                worker_id: "worker-3".to_owned(),
                lease_until: takeover_now + chrono::Duration::seconds(90),
                now: takeover_now + chrono::Duration::seconds(61),
            })
            .await
            .unwrap();
        assert_eq!(claim_after_takeover, ClaimEffectResult::Ineligible);

        let last_attempt_status: String = sqlx::query_scalar(
            "SELECT status FROM workflow_attempts WHERE effect_id = 'eff-1' AND ordinal = 0",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(last_attempt_status, "authority_lost");
    }

    #[tokio::test]
    async fn acceptance_replay_conflict_and_drained_replay() {
        let pool = test_pool().await;
        let repo = WorkflowRepository::new(pool.clone());
        repo.register_protocol_selection(&registration(true, true))
            .await
            .unwrap();

        let first = repo.accept_external_workflow(&acceptance()).await.unwrap();
        assert!(matches!(first, ExternalAcceptanceResult::New { .. }));

        let replay = repo.accept_external_workflow(&acceptance()).await.unwrap();
        assert!(matches!(replay, ExternalAcceptanceResult::Replay { .. }));

        let mut conflicting = acceptance();
        conflicting.intent_fingerprint = "fp-2".to_owned();
        let conflict = repo.accept_external_workflow(&conflicting).await.unwrap();
        assert_eq!(conflict, ExternalAcceptanceResult::Conflict);

        sqlx::query("UPDATE workflow_protocol_selections SET accepting = 0, drained_at = 'later' WHERE id = 'sel-1'")
            .execute(&pool)
            .await
            .unwrap();

        let drained_replay = repo.accept_external_workflow(&acceptance()).await.unwrap();
        assert!(matches!(
            drained_replay,
            ExternalAcceptanceResult::Replay { .. }
        ));
    }

    #[tokio::test]
    async fn acceptance_rollback_leaves_no_workflow_or_key() {
        let pool = test_pool().await;
        let repo = WorkflowRepository::new(pool.clone());
        repo.register_protocol_selection(&registration(true, true))
            .await
            .unwrap();

        let err = repo
            .accept_external_workflow_with_failpoint(
                &acceptance(),
                Some(WorkflowFailpoint::AfterWorkflowInsert),
            )
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            WorkflowRepositoryError::Failpoint(WorkflowFailpoint::AfterWorkflowInsert)
        ));

        let workflows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM workflows WHERE id = 'wf-1'")
            .fetch_one(&pool)
            .await
            .unwrap();
        let bindings: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM external_acceptance_bindings WHERE workflow_id = 'wf-1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(workflows, 0);
        assert_eq!(bindings, 0);
    }

    #[tokio::test]
    async fn acceptance_failpoint_after_binding_insert_rolls_back_workflow_and_binding() {
        let pool = test_pool().await;
        let repo = WorkflowRepository::new(pool.clone());
        repo.register_protocol_selection(&registration(true, true))
            .await
            .unwrap();

        let err = repo
            .accept_external_workflow_with_failpoint(
                &acceptance(),
                Some(WorkflowFailpoint::AfterBindingInsert),
            )
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            WorkflowRepositoryError::Failpoint(WorkflowFailpoint::AfterBindingInsert)
        ));

        let workflows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM workflows WHERE id = 'wf-1'")
            .fetch_one(&pool)
            .await
            .unwrap();
        let bindings: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM external_acceptance_bindings WHERE workflow_id = 'wf-1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(workflows, 0);
        assert_eq!(bindings, 0);
    }

    #[tokio::test]
    async fn concurrent_same_key_same_fingerprint_returns_new_and_replay_without_orphans() {
        for _ in 0..10 {
            let first = acceptance();
            let mut second = acceptance();
            second.binding_id = "binding-2".to_owned();
            second.workflow_id = "wf-2".to_owned();
            second.handle_receipt.payload = "handle-payload-2".to_owned();

            run_concurrent_acceptances(first, second, |left, right| {
                let (new_result, replay_result) = match (left, right) {
                    (
                        ExternalAcceptanceResult::New {
                            workflow_id: new_workflow_id,
                            handle_receipt: new_receipt,
                        },
                        ExternalAcceptanceResult::Replay {
                            workflow_id: replay_workflow_id,
                            handle_receipt: replay_receipt,
                        },
                    )
                    | (
                        ExternalAcceptanceResult::Replay {
                            workflow_id: replay_workflow_id,
                            handle_receipt: replay_receipt,
                        },
                        ExternalAcceptanceResult::New {
                            workflow_id: new_workflow_id,
                            handle_receipt: new_receipt,
                        },
                    ) => (
                        (new_workflow_id, new_receipt),
                        (replay_workflow_id, replay_receipt),
                    ),
                    other => panic!("expected one New and one Replay, got {other:?}"),
                };
                assert_eq!(new_result.0, replay_result.0);
                assert_eq!(new_result.1, replay_result.1);
            })
            .await;
        }
    }

    #[tokio::test]
    async fn concurrent_same_key_different_fingerprint_returns_new_and_conflict_without_orphans() {
        for _ in 0..10 {
            let first = acceptance();
            let mut second = acceptance();
            second.binding_id = "binding-2".to_owned();
            second.workflow_id = "wf-2".to_owned();
            second.intent_fingerprint = "fp-2".to_owned();
            second.handle_receipt.payload = "handle-payload-2".to_owned();

            run_concurrent_acceptances(first, second, |left, right| match (left, right) {
                (ExternalAcceptanceResult::New { .. }, ExternalAcceptanceResult::Conflict)
                | (ExternalAcceptanceResult::Conflict, ExternalAcceptanceResult::New { .. }) => {}
                other => panic!("expected one New and one Conflict, got {other:?}"),
            })
            .await;
        }
    }

    #[tokio::test]
    async fn acceptance_busy_without_visible_binding_returns_retryable() {
        let pool = file_backed_test_pool(2).await;
        let repo = WorkflowRepository::new(pool.clone());
        repo.register_protocol_selection(&registration(true, true))
            .await
            .unwrap();

        let mut conn = pool.acquire().await.unwrap();
        sqlx::query("BEGIN EXCLUSIVE")
            .execute(&mut *conn)
            .await
            .unwrap();

        let result = repo.accept_external_workflow(&acceptance()).await.unwrap();
        assert_eq!(result, ExternalAcceptanceResult::Retryable);

        sqlx::query("ROLLBACK").execute(&mut *conn).await.unwrap();
        assert_no_orphan_workflows(&pool).await;
    }

    #[tokio::test]
    async fn drained_non_replay_returns_not_accepting() {
        let pool = test_pool().await;
        let repo = WorkflowRepository::new(pool.clone());
        repo.register_protocol_selection(&registration(false, true))
            .await
            .unwrap();

        let result = repo.accept_external_workflow(&acceptance()).await.unwrap();
        assert_eq!(result, ExternalAcceptanceResult::NotAccepting);
    }

    #[tokio::test]
    async fn transition_cas_loser_has_no_mutation() {
        let pool = test_pool().await;
        let repo = WorkflowRepository::new(pool.clone());
        register_and_accept(&repo).await;

        let mut commit = transition_commit();
        commit.expected_from_version = 9;
        commit.next_version = 10;
        let outcome = repo.persist_transition_plan(&commit).await.unwrap();
        assert_eq!(outcome, TransitionCommitOutcome::VersionConflict);

        let version: i64 = sqlx::query_scalar("SELECT version FROM workflows WHERE id = 'wf-1'")
            .fetch_one(&pool)
            .await
            .unwrap();
        let transitions: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM workflow_transitions WHERE workflow_id = 'wf-1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let effects: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM workflow_effects WHERE workflow_id = 'wf-1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(version, 0);
        assert_eq!(transitions, 0);
        assert_eq!(effects, 0);
    }

    #[tokio::test]
    async fn transition_insert_failure_rolls_back_version_transition_and_effects() {
        let pool = test_pool().await;
        let repo = WorkflowRepository::new(pool.clone());
        register_and_accept(&repo).await;

        let err = repo
            .persist_transition_plan_with_failpoint(
                &transition_commit(),
                Some(WorkflowFailpoint::AfterTransitionInsert),
            )
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            WorkflowRepositoryError::Failpoint(WorkflowFailpoint::AfterTransitionInsert)
        ));

        let version: i64 = sqlx::query_scalar("SELECT version FROM workflows WHERE id = 'wf-1'")
            .fetch_one(&pool)
            .await
            .unwrap();
        let transitions: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM workflow_transitions WHERE workflow_id = 'wf-1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let effects: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM workflow_effects WHERE workflow_id = 'wf-1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(version, 0);
        assert_eq!(transitions, 0);
        assert_eq!(effects, 0);
    }

    #[tokio::test]
    async fn successful_dag_rows_all_present_and_generation_matches_version_generation_rule() {
        let pool = test_pool().await;
        let repo = WorkflowRepository::new(pool.clone());
        register_and_accept(&repo).await;

        let outcome = repo
            .persist_transition_plan(&transition_commit())
            .await
            .unwrap();
        assert_eq!(outcome, TransitionCommitOutcome::Committed);

        let workflow = sqlx::query(
            "SELECT version, generation, snapshot_codec_family, snapshot_payload FROM workflows WHERE id = 'wf-1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(workflow.get::<i64, _>("version"), 1);
        assert_eq!(workflow.get::<i64, _>("generation"), 0);
        assert_eq!(
            workflow.get::<String, _>("snapshot_codec_family"),
            "snapshot"
        );
        assert_eq!(workflow.get::<String, _>("snapshot_payload"), "snapshot-v1");

        let transition_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM workflow_transitions WHERE workflow_id = 'wf-1' AND from_version = 0 AND to_version = 1")
            .fetch_one(&pool)
            .await
            .unwrap();
        let effect_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM workflow_effects WHERE workflow_id = 'wf-1' AND declared_workflow_version = 1")
            .fetch_one(&pool)
            .await
            .unwrap();
        let dependency_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM workflow_effect_dependencies WHERE effect_id = 'eff-2' AND dependency_effect_id = 'eff-1'")
            .fetch_one(&pool)
            .await
            .unwrap();
        let barrier = sqlx::query(
            "SELECT workflow_id, event_codec_family, event_codec_version, event_payload \
             FROM workflow_barriers WHERE id = 'bar-1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let barrier_member_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM workflow_barrier_members WHERE barrier_id = 'bar-1' AND effect_id = 'eff-1'")
            .fetch_one(&pool)
            .await
            .unwrap();
        let inbox_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM workflow_reducer_inbox WHERE barrier_id = 'bar-1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let ordinary_effect_generations: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM workflow_effects WHERE workflow_id = 'wf-1' AND generation = 0",
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        assert_eq!(transition_count, 1);
        assert_eq!(effect_count, 2);
        assert_eq!(dependency_count, 1);
        assert_eq!(barrier.get::<String, _>("workflow_id"), "wf-1");
        assert_eq!(
            barrier.get::<String, _>("event_codec_family"),
            "barrier-event"
        );
        assert_eq!(barrier.get::<i64, _>("event_codec_version"), 1);
        assert_eq!(barrier.get::<String, _>("event_payload"), "barrier-payload");
        assert_eq!(barrier_member_count, 1);
        assert_eq!(inbox_count, 0);
        assert_eq!(ordinary_effect_generations, 2);
    }

    #[tokio::test]
    async fn unsupported_acceptance_codec_or_executor_returns_not_accepting() {
        let pool = test_pool().await;
        let repo = WorkflowRepository::new(pool);
        repo.register_protocol_selection(&registration(true, true))
            .await
            .unwrap();

        let mut unsupported_codec = acceptance();
        unsupported_codec.workflow_snapshot.codec.family = "unknown-snapshot".to_owned();
        let result = repo
            .accept_external_workflow(&unsupported_codec)
            .await
            .unwrap();
        assert_eq!(result, ExternalAcceptanceResult::NotAccepting);

        let mut unsupported_executor = acceptance();
        unsupported_executor.binding_id = "binding-2".to_owned();
        unsupported_executor.idempotency_key = "idem-2".to_owned();
        unsupported_executor.workflow_id = "wf-2".to_owned();
        unsupported_executor.executor_kind = "other".to_owned();
        let result = repo
            .accept_external_workflow(&unsupported_executor)
            .await
            .unwrap();
        assert_eq!(result, ExternalAcceptanceResult::NotAccepting);
    }

    #[tokio::test]
    async fn invalid_transition_generation_and_barrier_semantics_return_invalid_plan() {
        let pool = test_pool().await;
        let repo = WorkflowRepository::new(pool.clone());
        register_and_accept(&repo).await;

        let mut bad_generation = transition_commit();
        bad_generation.next_generation = 1;
        let outcome = repo.persist_transition_plan(&bad_generation).await.unwrap();
        assert_eq!(outcome, TransitionCommitOutcome::InvalidPlan);

        let mut bad_barrier = transition_commit();
        bad_barrier.barrier_members.clear();
        let outcome = repo.persist_transition_plan(&bad_barrier).await.unwrap();
        assert_eq!(outcome, TransitionCommitOutcome::InvalidPlan);

        let mut satisfied_barrier = transition_commit();
        satisfied_barrier.barriers[0].status = BarrierStatus::Satisfied;
        satisfied_barrier.barriers[0].satisfied_at = Some(Utc::now());
        let outcome = repo
            .persist_transition_plan(&satisfied_barrier)
            .await
            .unwrap();
        assert_eq!(outcome, TransitionCommitOutcome::InvalidPlan);

        let mut receipted_member = transition_commit();
        receipted_member.effects[0].status = EffectStatus::Receipted;
        let outcome = repo
            .persist_transition_plan(&receipted_member)
            .await
            .unwrap();
        assert_eq!(outcome, TransitionCommitOutcome::InvalidPlan);
    }

    #[tokio::test]
    async fn transition_dependency_cycle_returns_invalid_plan_without_mutation() {
        let pool = test_pool().await;
        let repo = WorkflowRepository::new(pool.clone());
        register_and_accept(&repo).await;

        let mut cyclic = transition_commit();
        cyclic.dependencies = vec![
            DurableEffectDependencyRecord {
                effect_id: "eff-2".to_owned(),
                dependency_effect_id: "eff-1".to_owned(),
            },
            DurableEffectDependencyRecord {
                effect_id: "eff-1".to_owned(),
                dependency_effect_id: "eff-2".to_owned(),
            },
        ];

        let outcome = repo.persist_transition_plan(&cyclic).await.unwrap();
        assert_eq!(outcome, TransitionCommitOutcome::InvalidPlan);

        let version: i64 = sqlx::query_scalar("SELECT version FROM workflows WHERE id = 'wf-1'")
            .fetch_one(&pool)
            .await
            .unwrap();
        let transitions: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM workflow_transitions WHERE workflow_id = 'wf-1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let dependencies: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM workflow_effect_dependencies WHERE effect_id IN ('eff-1','eff-2')",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(version, 0);
        assert_eq!(transitions, 0);
        assert_eq!(dependencies, 0);
    }

    #[tokio::test]
    async fn invalidation_validation_rejects_unknown_receipted_and_manual_required_without_mutation(
    ) {
        let pool = test_pool().await;
        let repo = WorkflowRepository::new(pool.clone());
        register_and_accept(&repo).await;
        assert_eq!(
            repo.persist_transition_plan(&transition_commit())
                .await
                .unwrap(),
            TransitionCommitOutcome::Committed
        );

        sqlx::query(
            "INSERT INTO workflow_effects \
             (id, workflow_id, declaring_transition_id, declared_workflow_version, generation, family, kind, codec_family, codec_version, role, ambiguity_policy, intent_payload, status, next_eligible_at, destructive_resource) \
             VALUES ('eff-manual', 'wf-1', 'tr-1', 1, 0, 'wake', 'manual', 'intent', 1, 'required', 'manual_resolution', 'manual', 'ambiguity_wait', NULL, NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO workflow_manual_resolutions \
             (id, workflow_id, effect_id, status, evidence_codec_family, evidence_codec_version, evidence_payload, accepted_choice_id, resolved_by) \
             VALUES ('mr-1', 'wf-1', 'eff-manual', 'required', 'evidence', 1, 'payload', NULL, NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let mut missing = transition_commit();
        missing.transition_id = "tr-invalid-missing".to_owned();
        missing.expected_from_version = 1;
        missing.next_version = 2;
        missing.effects.clear();
        missing.dependencies.clear();
        missing.barriers.clear();
        missing.barrier_members.clear();
        missing.invalidations = vec![DurableInvalidationRecord {
            effect_id: "missing".to_owned(),
            expected_declared_workflow_version: 1,
            expected_generation: 0,
        }];
        assert_eq!(
            repo.persist_transition_plan(&missing).await.unwrap(),
            TransitionCommitOutcome::InvalidPlan
        );

        let mut manual_required = transition_commit();
        manual_required.transition_id = "tr-invalid-manual".to_owned();
        manual_required.expected_from_version = 1;
        manual_required.next_version = 2;
        manual_required.effects.clear();
        manual_required.dependencies.clear();
        manual_required.barriers.clear();
        manual_required.barrier_members.clear();
        manual_required.invalidations = vec![DurableInvalidationRecord {
            effect_id: "eff-manual".to_owned(),
            expected_declared_workflow_version: 1,
            expected_generation: 0,
        }];
        assert_eq!(
            repo.persist_transition_plan(&manual_required)
                .await
                .unwrap(),
            TransitionCommitOutcome::InvalidPlan
        );

        let version: i64 = sqlx::query_scalar("SELECT version FROM workflows WHERE id = 'wf-1'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(version, 1);
    }

    #[tokio::test]
    async fn invalidation_validation_rejects_version_or_generation_mismatch() {
        let pool = test_pool().await;
        let repo = WorkflowRepository::new(pool.clone());
        register_and_accept(&repo).await;
        assert_eq!(
            repo.persist_transition_plan(&transition_commit())
                .await
                .unwrap(),
            TransitionCommitOutcome::Committed
        );

        let mut wrong_version = transition_commit();
        wrong_version.transition_id = "tr-invalid-version".to_owned();
        wrong_version.expected_from_version = 1;
        wrong_version.next_version = 2;
        wrong_version.effects.clear();
        wrong_version.dependencies.clear();
        wrong_version.barriers.clear();
        wrong_version.barrier_members.clear();
        wrong_version.invalidations = vec![DurableInvalidationRecord {
            effect_id: "eff-2".to_owned(),
            expected_declared_workflow_version: 2,
            expected_generation: 0,
        }];
        assert_eq!(
            repo.persist_transition_plan(&wrong_version).await.unwrap(),
            TransitionCommitOutcome::InvalidPlan
        );

        let mut wrong_generation = transition_commit();
        wrong_generation.transition_id = "tr-invalid-generation".to_owned();
        wrong_generation.expected_from_version = 1;
        wrong_generation.next_version = 2;
        wrong_generation.effects.clear();
        wrong_generation.dependencies.clear();
        wrong_generation.barriers.clear();
        wrong_generation.barrier_members.clear();
        wrong_generation.invalidations = vec![DurableInvalidationRecord {
            effect_id: "eff-2".to_owned(),
            expected_declared_workflow_version: 1,
            expected_generation: 99,
        }];
        assert_eq!(
            repo.persist_transition_plan(&wrong_generation)
                .await
                .unwrap(),
            TransitionCommitOutcome::InvalidPlan
        );
    }

    #[tokio::test]
    async fn transition_failpoints_after_barrier_and_invalidation_roll_back_all_rows() {
        let pool = test_pool().await;
        let repo = WorkflowRepository::new(pool.clone());
        register_and_accept(&repo).await;

        let err = repo
            .persist_transition_plan_with_failpoint(
                &transition_commit(),
                Some(WorkflowFailpoint::AfterBarrierInsert),
            )
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            WorkflowRepositoryError::Failpoint(WorkflowFailpoint::AfterBarrierInsert)
        ));

        let version: i64 = sqlx::query_scalar("SELECT version FROM workflows WHERE id = 'wf-1'")
            .fetch_one(&pool)
            .await
            .unwrap();
        let transitions: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM workflow_transitions WHERE workflow_id = 'wf-1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let barriers: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM workflow_barriers WHERE workflow_id = 'wf-1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(version, 0);
        assert_eq!(transitions, 0);
        assert_eq!(barriers, 0);

        let committed = repo
            .persist_transition_plan(&transition_commit())
            .await
            .unwrap();
        assert_eq!(committed, TransitionCommitOutcome::Committed);

        let mut commit = transition_commit();
        commit.transition_id = "tr-2".to_owned();
        commit.expected_from_version = 1;
        commit.next_version = 2;
        commit.effects.clear();
        commit.dependencies.clear();
        commit.barriers.clear();
        commit.barrier_members.clear();
        commit.invalidations = vec![DurableInvalidationRecord {
            effect_id: "eff-2".to_owned(),
            expected_declared_workflow_version: 1,
            expected_generation: 0,
        }];

        let err = repo
            .persist_transition_plan_with_failpoint(
                &commit,
                Some(WorkflowFailpoint::AfterInvalidations),
            )
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            WorkflowRepositoryError::Failpoint(WorkflowFailpoint::AfterInvalidations)
        ));

        let version: i64 = sqlx::query_scalar("SELECT version FROM workflows WHERE id = 'wf-1'")
            .fetch_one(&pool)
            .await
            .unwrap();
        let invalidated: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM workflow_effects WHERE workflow_id = 'wf-1' AND status = 'invalidated'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let transition_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM workflow_transitions WHERE workflow_id = 'wf-1' AND to_version = 2",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(version, 1);
        assert_eq!(invalidated, 0);
        assert_eq!(transition_count, 0);
    }

    #[tokio::test]
    async fn transition_rejects_non_active_non_engine_or_shadow_workflow() {
        let pool = test_pool().await;
        let repo = WorkflowRepository::new(pool.clone());
        register_and_accept(&repo).await;

        sqlx::query("UPDATE workflows SET status = 'completed' WHERE id = 'wf-1'")
            .execute(&pool)
            .await
            .unwrap();
        let outcome = repo
            .persist_transition_plan(&transition_commit())
            .await
            .unwrap();
        assert_eq!(outcome, TransitionCommitOutcome::InvalidPlan);

        sqlx::query(
            "UPDATE workflows SET authority = 'engine_protocol', execution_mode = 'shadow', authoritative_workflow_id = 'wf-1' WHERE id = 'wf-1'",
        )
        .execute(&pool)
        .await
        .unwrap();
        let outcome = repo
            .persist_transition_plan(&transition_commit())
            .await
            .unwrap();
        assert_eq!(outcome, TransitionCommitOutcome::InvalidPlan);
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn owed_acceptance_persists_atomically_and_waiting_barrier_creates_no_inbox() {
        let pool = test_pool().await;
        let repo = WorkflowRepository::new(pool.clone());
        register_and_accept(&repo).await;

        let outcome = repo
            .persist_transition_plan(&transition_commit())
            .await
            .unwrap();
        assert_eq!(outcome, TransitionCommitOutcome::Committed);

        sqlx::query(
            "INSERT INTO workflow_receipts \
             (id, attempt_id, effect_id, workflow_id, declared_workflow_version, generation, claim_token, claim_worker_id, claim_lease_until, claim_issued_at, codec_family, codec_version, payload, origin, accepted_at) \
             VALUES ('receipt-1', NULL, 'eff-1', 'wf-1', 1, 0, NULL, NULL, NULL, NULL, 'owed-event', 1, 'owed', 'manual', '2025-01-01T00:00:00Z')",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO workflow_reducer_inbox \
             (id, workflow_id, receipt_id, barrier_id, event_codec_family, event_codec_version, event_payload, delivery_status, consumed_by_transition_id) \
             VALUES ('inbox-1', 'wf-1', 'receipt-1', NULL, 'owed-event', 1, 'owed', 'pending', NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let mut commit = transition_commit();
        commit.transition_id = "tr-2".to_owned();
        commit.expected_from_version = 1;
        commit.next_version = 2;
        commit.effects.clear();
        commit.dependencies.clear();
        commit.barriers.clear();
        commit.barrier_members.clear();
        commit.owed_acceptances = vec![DurableOwedAcceptanceRecord {
            owed_acceptance_id: "owed-1".to_owned(),
            reducer_inbox_id: "inbox-1".to_owned(),
            source_kind: "receipt".to_owned(),
            event: DurablePayload {
                codec: DurableCodecRef {
                    family: "owed-event".to_owned(),
                    version: 1,
                },
                payload: "owed-payload".to_owned(),
            },
        }];

        let outcome = repo.persist_transition_plan(&commit).await.unwrap();
        assert_eq!(outcome, TransitionCommitOutcome::Committed);

        let owed = sqlx::query(
            "SELECT reducer_inbox_id, source_kind, event_codec_family, event_payload, status \
             FROM workflow_owed_acceptance WHERE id = 'owed-1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(owed.get::<String, _>("reducer_inbox_id"), "inbox-1");
        assert_eq!(owed.get::<String, _>("source_kind"), "receipt");
        assert_eq!(owed.get::<String, _>("event_codec_family"), "owed-event");
        assert_eq!(owed.get::<String, _>("event_payload"), "owed-payload");
        assert_eq!(owed.get::<String, _>("status"), "owed");

        let inbox = sqlx::query(
            "SELECT delivery_status, consumed_by_transition_id \
             FROM workflow_reducer_inbox WHERE id = 'inbox-1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(inbox.get::<String, _>("delivery_status"), "consumed");
        assert_eq!(inbox.get::<String, _>("consumed_by_transition_id"), "tr-2");

        let inbox_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM workflow_reducer_inbox WHERE barrier_id = 'bar-1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(inbox_count, 0);

        let mut rollback_commit = transition_commit();
        rollback_commit.transition_id = "tr-rollback-owed".to_owned();
        rollback_commit.expected_from_version = 2;
        rollback_commit.next_version = 3;
        rollback_commit.effects.clear();
        rollback_commit.dependencies.clear();
        rollback_commit.barriers.clear();
        rollback_commit.barrier_members.clear();

        sqlx::query(
            "INSERT INTO workflow_reducer_inbox \
             (id, workflow_id, receipt_id, barrier_id, event_codec_family, event_codec_version, event_payload, delivery_status, consumed_by_transition_id) \
             VALUES ('inbox-rollback', 'wf-1', 'receipt-1', NULL, 'owed-event', 1, 'owed', 'pending', NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();
        rollback_commit.owed_acceptances = vec![DurableOwedAcceptanceRecord {
            owed_acceptance_id: "owed-rollback".to_owned(),
            reducer_inbox_id: "inbox-rollback".to_owned(),
            source_kind: "receipt".to_owned(),
            event: DurablePayload {
                codec: DurableCodecRef {
                    family: "owed-event".to_owned(),
                    version: 1,
                },
                payload: "owed-rollback-payload".to_owned(),
            },
        }];

        let err = repo
            .persist_transition_plan_with_failpoint(
                &rollback_commit,
                Some(WorkflowFailpoint::AfterInvalidations),
            )
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            WorkflowRepositoryError::Failpoint(WorkflowFailpoint::AfterInvalidations)
        ));

        let rollback_inbox = sqlx::query(
            "SELECT delivery_status, consumed_by_transition_id \
             FROM workflow_reducer_inbox WHERE id = 'inbox-rollback'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            rollback_inbox.get::<String, _>("delivery_status"),
            "pending"
        );
        assert!(rollback_inbox
            .get::<Option<String>, _>("consumed_by_transition_id")
            .is_none());
    }
}
