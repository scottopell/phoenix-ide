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
    #[error("protocol selection {existing_selection_id} already accepts profile {profile_id}; requested {requested_selection_id} is incompatible")]
    ProtocolSelectionIncompatible {
        requested_selection_id: String,
        existing_selection_id: String,
        profile_id: String,
    },
    #[error("corrupt durable workflow state: {0}")]
    CorruptState(String),
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
pub struct DurableManualResolutionChoice {
    pub choice_id: String,
    pub kind: String,
    pub payload: DurablePayload,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableManualResolutionRequest {
    pub resolution_id: String,
    pub authority: DurableClaimAuthority,
    pub now: DateTime<Utc>,
    pub evidence: DurablePayload,
    pub evidence_links: Vec<(String, String)>,
    pub choices: Vec<DurableManualResolutionChoice>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconcileEffectResult {
    ScheduledRetry,
    ManualResolutionRequired,
    ManualOnly,
    InvalidRequest,
    StaleAuthority,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DueEffect {
    Eligible {
        workflow_id: String,
        effect_id: String,
        declared_workflow_version: u64,
        generation: u64,
    },
    RetryWait {
        workflow_id: String,
        effect_id: String,
        declared_workflow_version: u64,
        generation: u64,
        next_eligible_at: DateTime<Utc>,
    },
    ExpiredClaim {
        authority: DurableClaimAuthority,
    },
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableObservationRecord {
    pub observation_id: String,
    pub authority: DurableClaimAuthority,
    pub attempt_id: String,
    pub payload: DurablePayload,
    pub observed_at: DateTime<Utc>,
    pub recorded_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableStaleObservationRecord {
    pub observation_id: String,
    pub authority: DurableClaimAuthority,
    pub attempt_id: String,
    pub payload: DurablePayload,
    pub observed_at: DateTime<Utc>,
    pub recorded_at: DateTime<Utc>,
    pub stale_reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordObservationResult {
    Recorded {
        observation: Box<DurableObservationRecord>,
    },
    StaleAuthority,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetainStaleObservationResult {
    Recorded {
        observation: Box<DurableStaleObservationRecord>,
    },
    AttemptMismatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurableReceiptOrigin {
    Execution,
    Adoption,
    Reconciliation,
    Manual,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableReceiptAcceptance {
    pub receipt_id: String,
    pub authority: DurableClaimAuthority,
    pub attempt_id: String,
    pub payload: DurablePayload,
    pub origin: DurableReceiptOrigin,
    pub accepted_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableReducerInboxRecord {
    pub reducer_inbox_id: String,
    pub workflow_id: String,
    pub receipt_id: String,
    pub event: DurablePayload,
    pub requires_runtime_acceptance: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableDirectInboxEvent {
    pub reducer_inbox_id: String,
    pub workflow_id: String,
    pub event: DurablePayload,
    pub requires_runtime_acceptance: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurableDivergenceResolutionAction {
    Rollback,
    Reauthorize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcceptReceiptResult {
    Accepted {
        receipt: DurableReceiptAcceptance,
        reducer_inbox: DurableReducerInboxRecord,
    },
    AlreadyReceipted {
        receipt: DurableReceiptAcceptance,
    },
    Conflict,
    StaleAuthority,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableBashTailLine {
    pub offset: u64,
    pub line: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableWakeTerminalProjection {
    pub contract_id: String,
    pub resource_kind: String,
    pub status: String,
    pub resolved_at: DateTime<Utc>,
    pub bash_status: Option<String>,
    pub bash_occurred_at: Option<DateTime<Utc>>,
    pub bash_exit_code: Option<i32>,
    pub bash_duration_ms: Option<u64>,
    pub bash_signal_number: Option<i32>,
    pub bash_kill_signal_sent: Option<String>,
    pub bash_tail_start_offset: Option<u64>,
    pub bash_tail_end_offset: Option<u64>,
    pub bash_tail_truncated_before: Option<bool>,
    pub bash_tail: Vec<DurableBashTailLine>,
    pub tmux_status: Option<String>,
    pub tmux_occurred_at: Option<DateTime<Utc>>,
    pub tmux_server_generation: Option<String>,
    pub tmux_exit_code: Option<i32>,
    pub tmux_duration_ms: Option<u64>,
    pub tmux_tail: Vec<String>,
    pub forgotten_reason: Option<String>,
    pub cancellation_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableAcceptReceiptRequest {
    pub receipt_id: String,
    pub reducer_inbox_id: String,
    pub authority: DurableClaimAuthority,
    pub now: DateTime<Utc>,
    pub attempt_id: Option<String>,
    pub origin: DurableReceiptOrigin,
    pub receipt: DurablePayload,
    pub reducer_event: DurablePayload,
    pub wake_terminal_projection: Option<DurableWakeTerminalProjection>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowFailpoint {
    AfterWorkflowInsert,
    AfterWorkflowUpdate,
    AfterBindingInsert,
    AfterTransitionInsert,
    AfterBarrierInsert,
    AfterInvalidations,
    AfterReceiptInsert,
    AfterManualResolutionInsert,
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

    /// Persist an authoritative observation only while the exact claim authority remains live.
    ///
    /// # Errors
    ///
    /// Returns an error when the transaction, row decoding, or insert/update fails.
    pub async fn record_observation(
        &self,
        observation: &DurableObservationRecord,
    ) -> WorkflowRepositoryResult<RecordObservationResult> {
        let mut tx = self.pool.begin().await?;
        if !record_observation_in_transaction(&mut tx, observation).await? {
            tx.rollback().await?;
            return Ok(RecordObservationResult::StaleAuthority);
        }
        tx.commit().await?;
        Ok(RecordObservationResult::Recorded {
            observation: Box::new(observation.clone()),
        })
    }

    /// Persist a stale observation diagnostically using the supplied historical authority.
    ///
    /// # Errors
    ///
    /// Returns an error when the insert or coherence checks fail.
    pub async fn retain_stale_observation(
        &self,
        observation: &DurableStaleObservationRecord,
    ) -> WorkflowRepositoryResult<RetainStaleObservationResult> {
        let updated = sqlx::query(
            "UPDATE workflow_attempts \
             SET status = status \
             WHERE id = ?1 AND effect_id = ?2 AND workflow_id = ?3 \
               AND declared_workflow_version = ?4 AND generation = ?5 \
               AND claim_token = ?6 AND claim_worker_id = ?7 \
               AND claim_lease_until = ?8 AND claim_issued_at = ?9",
        )
        .bind(&observation.attempt_id)
        .bind(&observation.authority.effect_id)
        .bind(&observation.authority.workflow_id)
        .bind(to_i64(
            observation.authority.declared_workflow_version,
            WorkflowRepositoryError::VersionOutOfRange,
        )?)
        .bind(to_i64(
            observation.authority.generation,
            WorkflowRepositoryError::GenerationOutOfRange,
        )?)
        .bind(&observation.authority.claim_token)
        .bind(&observation.authority.worker_id)
        .bind(observation.authority.lease_until.to_rfc3339())
        .bind(observation.authority.issued_at.to_rfc3339())
        .execute(&self.pool)
        .await?;
        if updated.rows_affected() != 1 {
            return Ok(RetainStaleObservationResult::AttemptMismatch);
        }

        sqlx::query(
            "INSERT INTO workflow_stale_observations \
             (id, effect_id, attempt_id, workflow_id, declared_workflow_version, generation, \
              claim_token, claim_worker_id, claim_lease_until, claim_issued_at, codec_family, \
              codec_version, payload, observed_at, recorded_at, stale_reason) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
        )
        .bind(&observation.observation_id)
        .bind(&observation.authority.effect_id)
        .bind(&observation.attempt_id)
        .bind(&observation.authority.workflow_id)
        .bind(to_i64(
            observation.authority.declared_workflow_version,
            WorkflowRepositoryError::VersionOutOfRange,
        )?)
        .bind(to_i64(
            observation.authority.generation,
            WorkflowRepositoryError::GenerationOutOfRange,
        )?)
        .bind(&observation.authority.claim_token)
        .bind(&observation.authority.worker_id)
        .bind(observation.authority.lease_until.to_rfc3339())
        .bind(observation.authority.issued_at.to_rfc3339())
        .bind(&observation.payload.codec.family)
        .bind(i64::from(observation.payload.codec.version))
        .bind(&observation.payload.payload)
        .bind(observation.observed_at.to_rfc3339())
        .bind(observation.recorded_at.to_rfc3339())
        .bind(&observation.stale_reason)
        .execute(&self.pool)
        .await?;

        Ok(RetainStaleObservationResult::Recorded {
            observation: Box::new(observation.clone()),
        })
    }

    /// Accept a receipt exactly once, proving receipt/inbox/state atomicity.
    /// Adoption and reconciliation use the same exact-attempt requirement as execution because the
    /// current engine routes all worker-path receipts through a concrete begun attempt.
    ///
    /// # Errors
    ///
    /// Returns an error when the transaction, failpoint, or DML fails.
    pub async fn accept_receipt(
        &self,
        request: &DurableAcceptReceiptRequest,
    ) -> WorkflowRepositoryResult<AcceptReceiptResult> {
        self.accept_receipt_with_failpoint(request, None).await
    }

    /// Accept a receipt with an optional rollback failpoint after receipt insert and before state/inbox.
    ///
    /// # Errors
    ///
    /// Returns an error when the transaction, failpoint, or DML fails.
    pub async fn accept_receipt_with_failpoint(
        &self,
        request: &DurableAcceptReceiptRequest,
        failpoint: Option<WorkflowFailpoint>,
    ) -> WorkflowRepositoryResult<AcceptReceiptResult> {
        if let Some(result) = preflight_receipt_acceptance(&self.pool, request).await? {
            return Ok(result);
        }
        let mut tx = self.pool.begin().await?;
        validate_receipt_codecs(&mut tx, request).await?;
        let result = accept_receipt_in_transaction(&mut tx, request, failpoint).await?;
        if matches!(result, AcceptReceiptResult::Accepted { .. }) {
            tx.commit().await?;
        } else {
            tx.rollback().await?;
        }
        Ok(result)
    }

    /// Persist an authoritative observation and accept its receipt in one `SQLite` transaction.
    ///
    /// # Errors
    ///
    /// Returns an error when the transaction or DML fails.
    pub async fn record_observation_and_accept_receipt(
        &self,
        observation: &DurableObservationRecord,
        request: &DurableAcceptReceiptRequest,
    ) -> WorkflowRepositoryResult<AcceptReceiptResult> {
        self.record_observation_and_accept_receipt_with_failpoint(observation, request, None)
            .await
    }

    /// The rollback-test variant of [`Self::record_observation_and_accept_receipt`].
    ///
    /// # Errors
    ///
    /// Returns an error when the transaction, failpoint, or DML fails.
    pub async fn record_observation_and_accept_receipt_with_failpoint(
        &self,
        observation: &DurableObservationRecord,
        request: &DurableAcceptReceiptRequest,
        failpoint: Option<WorkflowFailpoint>,
    ) -> WorkflowRepositoryResult<AcceptReceiptResult> {
        if observation.authority != request.authority
            || request.attempt_id.as_deref() != Some(observation.attempt_id.as_str())
        {
            return Ok(AcceptReceiptResult::StaleAuthority);
        }
        if let Some(result) = preflight_receipt_acceptance(&self.pool, request).await? {
            return Ok(result);
        }

        let mut tx = self.pool.begin().await?;
        if let Some(existing) =
            load_receipt_for_effect(&mut *tx, &request.authority.effect_id).await?
        {
            tx.rollback().await?;
            return Ok(compare_existing_receipt(existing, request));
        }
        validate_receipt_codecs(&mut tx, request).await?;
        if !record_observation_in_transaction(&mut tx, observation).await? {
            tx.rollback().await?;
            return Ok(AcceptReceiptResult::StaleAuthority);
        }
        let result = accept_new_receipt_in_transaction(&mut tx, request, failpoint).await?;
        if matches!(result, AcceptReceiptResult::Accepted { .. }) {
            tx.commit().await?;
        } else {
            tx.rollback().await?;
        }
        Ok(result)
    }

    /// Persist a reducer-only event that is not backed by a receipt or barrier.
    ///
    /// # Errors
    /// Returns an error when the workflow does not exist or the insert violates durable constraints.
    pub async fn persist_direct_inbox_event(
        &self,
        event: &DurableDirectInboxEvent,
    ) -> WorkflowRepositoryResult<()> {
        let mut tx = self.pool.begin().await?;
        validate_workflow_codecs(&mut tx, &event.workflow_id, [&event.event.codec]).await?;
        sqlx::query(
            "INSERT INTO workflow_reducer_inbox \
             (id, workflow_id, receipt_id, barrier_id, event_codec_family, event_codec_version, \
              event_payload, requires_runtime_acceptance, delivery_status, consumed_by_transition_id) \
             VALUES (?1, ?2, NULL, NULL, ?3, ?4, ?5, ?6, 'pending', NULL)",
        )
        .bind(&event.reducer_inbox_id)
        .bind(&event.workflow_id)
        .bind(&event.event.codec.family)
        .bind(i64::from(event.event.codec.version))
        .bind(&event.event.payload)
        .bind(event.requires_runtime_acceptance)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Resolve one shadow divergence with the operator's explicit cutover action.
    ///
    /// # Errors
    /// Returns an error when the update query fails.
    pub async fn resolve_shadow_divergence(
        &self,
        divergence_id: &str,
        action: DurableDivergenceResolutionAction,
        resolved_by: &str,
        resolved_at: DateTime<Utc>,
    ) -> WorkflowRepositoryResult<bool> {
        if resolved_by.is_empty() {
            return Err(WorkflowRepositoryError::InvalidPlan(
                "shadow divergence resolver must not be empty",
            ));
        }
        let updated = sqlx::query(
            "UPDATE workflow_shadow_divergences \
             SET resolution_action = ?1, resolved_by = ?2, resolved_at = ?3 \
             WHERE id = ?4 AND resolved_at IS NULL",
        )
        .bind(divergence_resolution_action_sql(action))
        .bind(resolved_by)
        .bind(resolved_at.to_rfc3339())
        .bind(divergence_id)
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected() == 1)
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

        match validate_transition_plan(&mut tx, commit, &workflow).await {
            Ok(()) => {}
            Err(WorkflowRepositoryError::InvalidPlan(_)) => {
                tx.rollback().await?;
                return Ok(TransitionCommitOutcome::InvalidPlan);
            }
            Err(error) => {
                tx.rollback().await?;
                return Err(error);
            }
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
            sqlx::query("DELETE FROM workflow_claims WHERE effect_id = ?1 AND workflow_id = ?2")
                .bind(&invalidation.effect_id)
                .bind(&commit.workflow_id)
                .execute(&mut *tx)
                .await?;
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
        if request.lease_until <= request.now {
            return Ok(ClaimEffectResult::Ineligible);
        }
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
               AND e.status IN ('eligible', 'blocked') \
               AND e.pending_reconciliation = 0 \
               AND e.generation = w.generation \
               AND c.effect_id IS NULL \
               AND NOT EXISTS ( \
                   SELECT 1 FROM workflow_effect_dependencies d \
                   JOIN workflow_effects prerequisite ON prerequisite.id = d.dependency_effect_id \
                   WHERE d.effect_id = e.id AND prerequisite.status <> 'receipted' \
               )",
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
               AND generation = ?4 AND status IN ('eligible', 'blocked')",
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

        let mut tx = self.pool.begin().await?;
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
        .execute(&mut *tx)
        .await?;

        if updated.rows_affected() != 1 {
            tx.rollback().await?;
            return Ok(RenewClaimResult::StaleAuthority);
        }

        let attempt_updated = sqlx::query(
            "UPDATE workflow_attempts SET claim_lease_until = ?1 \
             WHERE effect_id = ?2 AND workflow_id = ?3 AND declared_workflow_version = ?4 \
               AND generation = ?5 AND claim_token = ?6 AND claim_worker_id = ?7 \
               AND claim_lease_until = ?8 AND claim_issued_at = ?9 \
               AND status IN ('begun', 'observation_recorded')",
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
        .execute(&mut *tx)
        .await?;
        if attempt_updated.rows_affected() != 1 {
            tx.rollback().await?;
            return Ok(RenewClaimResult::StaleAuthority);
        }

        tx.commit().await?;
        let mut authority = renewal.authority.clone();
        authority.lease_until = renewal.lease_until;
        Ok(RenewClaimResult::Renewed { authority })
    }

    /// Schedule a retry from a live authoritative claim, including reconciliation takeovers.
    ///
    /// # Errors
    ///
    /// Returns an error when the transaction or DML fails.
    pub async fn schedule_retry(
        &self,
        authority: &DurableClaimAuthority,
        now: DateTime<Utc>,
        next_eligible_at: DateTime<Utc>,
    ) -> WorkflowRepositoryResult<ReconcileEffectResult> {
        if next_eligible_at < now {
            return Ok(ReconcileEffectResult::StaleAuthority);
        }

        let mut tx = self.pool.begin().await?;
        let Some(context) = load_reconcilable_effect_context(&mut tx, authority, now).await? else {
            tx.rollback().await?;
            return Ok(ReconcileEffectResult::StaleAuthority);
        };
        if context.ambiguity_policy == EffectAmbiguity::ManualResolution {
            tx.rollback().await?;
            return Ok(ReconcileEffectResult::ManualOnly);
        }

        finalize_reconciliation_transition(
            &mut tx,
            authority,
            now,
            "retry_wait",
            Some(next_eligible_at),
            false,
        )
        .await?;
        tx.commit().await?;
        Ok(ReconcileEffectResult::ScheduledRetry)
    }

    /// Require normalized manual resolution from a live authoritative claim.
    ///
    /// # Errors
    ///
    /// Returns an error when the transaction, failpoint, or DML fails.
    pub async fn require_manual_resolution(
        &self,
        request: &DurableManualResolutionRequest,
    ) -> WorkflowRepositoryResult<ReconcileEffectResult> {
        self.require_manual_resolution_with_failpoint(request, None)
            .await
    }

    /// Same as `require_manual_resolution` but with a rollback failpoint for tests.
    ///
    /// # Errors
    ///
    /// Returns an error when the transaction, failpoint, or DML fails.
    #[allow(clippy::too_many_lines)]
    pub async fn require_manual_resolution_with_failpoint(
        &self,
        request: &DurableManualResolutionRequest,
        failpoint: Option<WorkflowFailpoint>,
    ) -> WorkflowRepositoryResult<ReconcileEffectResult> {
        let mut tx = self.pool.begin().await?;
        let Some(context) =
            load_reconcilable_effect_context(&mut tx, &request.authority, request.now).await?
        else {
            tx.rollback().await?;
            return Ok(ReconcileEffectResult::StaleAuthority);
        };
        if context.ambiguity_policy != EffectAmbiguity::ManualResolution {
            tx.rollback().await?;
            return Ok(ReconcileEffectResult::InvalidRequest);
        }
        if request.choices.is_empty()
            || request
                .choices
                .iter()
                .any(|choice| choice.choice_id.is_empty())
            || request
                .choices
                .iter()
                .map(|choice| choice.choice_id.as_str())
                .collect::<HashSet<_>>()
                .len()
                != request.choices.len()
            || !codec_supported_by_workflow(
                &mut tx,
                &request.authority.workflow_id,
                &request.evidence.codec,
            )
            .await?
            || request
                .choices
                .iter()
                .any(|choice| !choice_kind_is_supported(choice.kind.as_str()))
            || request
                .choices
                .iter()
                .any(|choice| choice.payload.codec.family.is_empty())
        {
            tx.rollback().await?;
            return Ok(ReconcileEffectResult::InvalidRequest);
        }
        for choice in &request.choices {
            if !codec_supported_by_workflow(
                &mut tx,
                &request.authority.workflow_id,
                &choice.payload.codec,
            )
            .await?
            {
                tx.rollback().await?;
                return Ok(ReconcileEffectResult::InvalidRequest);
            }
        }
        let evidence_links_unique = request
            .evidence_links
            .iter()
            .map(|(kind, id)| (kind.as_str(), id.as_str()))
            .collect::<HashSet<_>>()
            .len()
            == request.evidence_links.len();
        if !evidence_links_unique {
            tx.rollback().await?;
            return Ok(ReconcileEffectResult::InvalidRequest);
        }
        for (evidence_kind, evidence_id) in &request.evidence_links {
            if !matches!(
                evidence_kind.as_str(),
                "authoritative_observation" | "stale_observation"
            ) || !manual_evidence_link_exists(
                &mut tx,
                &request.authority.workflow_id,
                &request.authority.effect_id,
                evidence_kind,
                evidence_id,
            )
            .await?
            {
                tx.rollback().await?;
                return Ok(ReconcileEffectResult::InvalidRequest);
            }
        }

        sqlx::query(
            "INSERT INTO workflow_manual_resolutions \
             (id, workflow_id, effect_id, status, evidence_codec_family, evidence_codec_version, evidence_payload, accepted_choice_id, resolved_by) \
             VALUES (?1, ?2, ?3, 'required', ?4, ?5, ?6, NULL, NULL)",
        )
        .bind(&request.resolution_id)
        .bind(&request.authority.workflow_id)
        .bind(&request.authority.effect_id)
        .bind(&request.evidence.codec.family)
        .bind(i64::from(request.evidence.codec.version))
        .bind(&request.evidence.payload)
        .execute(&mut *tx)
        .await?;

        fail_if_configured(
            &mut tx,
            failpoint,
            WorkflowFailpoint::AfterManualResolutionInsert,
        )
        .await?;

        for (evidence_kind, evidence_id) in &request.evidence_links {
            sqlx::query(
                "INSERT INTO workflow_manual_resolution_evidence_links \
                 (resolution_id, evidence_kind, evidence_id) VALUES (?1, ?2, ?3)",
            )
            .bind(&request.resolution_id)
            .bind(evidence_kind)
            .bind(evidence_id)
            .execute(&mut *tx)
            .await?;
        }

        for choice in &request.choices {
            sqlx::query(
                "INSERT INTO workflow_manual_resolution_choices \
                 (id, resolution_id, workflow_id, kind, codec_family, codec_version, payload) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )
            .bind(&choice.choice_id)
            .bind(&request.resolution_id)
            .bind(&request.authority.workflow_id)
            .bind(&choice.kind)
            .bind(&choice.payload.codec.family)
            .bind(i64::from(choice.payload.codec.version))
            .bind(&choice.payload.payload)
            .execute(&mut *tx)
            .await?;
        }

        finalize_reconciliation_transition(
            &mut tx,
            &request.authority,
            request.now,
            "ambiguity_wait",
            None,
            true,
        )
        .await?;
        tx.commit().await?;
        Ok(ReconcileEffectResult::ManualResolutionRequired)
    }

    /// Discover due effects purely from durable state, without mutating it.
    ///
    /// # Errors
    ///
    /// Returns an error when the query or row decoding fails.
    ///
    /// # Panics
    ///
    /// Panics only if stored workflow rows violate checked integer or RFC3339 invariants.
    pub async fn discover_due_effects(
        &self,
        now: DateTime<Utc>,
    ) -> WorkflowRepositoryResult<Vec<DueEffect>> {
        let rows = sqlx::query(
            "SELECT e.id AS effect_id, e.workflow_id, e.declared_workflow_version, e.generation, e.status, \
                    e.next_eligible_at, c.claim_token, c.worker_id, c.lease_until, c.issued_at \
             FROM workflow_effects e \
             JOIN workflows w ON w.id = e.workflow_id \
             LEFT JOIN workflow_claims c \
               ON c.effect_id = e.id AND c.workflow_id = e.workflow_id \
              AND c.declared_workflow_version = e.declared_workflow_version AND c.generation = e.generation \
             WHERE w.authority = 'engine_protocol' \
               AND w.execution_mode = 'authoritative' \
               AND w.status = 'active' \
               AND w.generation = e.generation \
               AND ((e.status IN ('eligible', 'blocked') \
                     AND NOT EXISTS ( \
                         SELECT 1 FROM workflow_effect_dependencies d \
                         JOIN workflow_effects prerequisite ON prerequisite.id = d.dependency_effect_id \
                         WHERE d.effect_id = e.id AND prerequisite.status <> 'receipted' \
                     )) \
                    OR (e.status = 'retry_wait' AND e.next_eligible_at IS NOT NULL AND e.next_eligible_at <= ?1) \
                    OR (e.status = 'claimed' AND c.effect_id IS NOT NULL AND c.lease_until <= ?1)) \
             ORDER BY e.workflow_id, e.id",
        )
        .bind(now.to_rfc3339())
        .fetch_all(&self.pool)
        .await?;

        let mut due = Vec::with_capacity(rows.len());
        for row in rows {
            let status: String = row.get("status");
            let workflow_id: String = row.get("workflow_id");
            let effect_id: String = row.get("effect_id");
            let declared_workflow_version = row
                .get::<i64, _>("declared_workflow_version")
                .try_into()
                .expect("declared workflow version fits u64");
            let generation = row
                .get::<i64, _>("generation")
                .try_into()
                .expect("generation fits u64");
            match status.as_str() {
                "eligible" | "blocked" => due.push(DueEffect::Eligible {
                    workflow_id,
                    effect_id,
                    declared_workflow_version,
                    generation,
                }),
                "retry_wait" => due.push(DueEffect::RetryWait {
                    workflow_id,
                    effect_id,
                    declared_workflow_version,
                    generation,
                    next_eligible_at: DateTime::parse_from_rfc3339(
                        &row.get::<String, _>("next_eligible_at"),
                    )
                    .expect("next_eligible_at is valid RFC3339")
                    .with_timezone(&Utc),
                }),
                "claimed" => due.push(DueEffect::ExpiredClaim {
                    authority: DurableClaimAuthority {
                        workflow_id,
                        effect_id,
                        declared_workflow_version,
                        generation,
                        claim_token: row.get("claim_token"),
                        worker_id: row.get("worker_id"),
                        lease_until: DateTime::parse_from_rfc3339(
                            &row.get::<String, _>("lease_until"),
                        )
                        .expect("lease_until is valid RFC3339")
                        .with_timezone(&Utc),
                        issued_at: DateTime::parse_from_rfc3339(&row.get::<String, _>("issued_at"))
                            .expect("issued_at is valid RFC3339")
                            .with_timezone(&Utc),
                    },
                }),
                other => {
                    return Err(WorkflowRepositoryError::CorruptState(format!(
                        "due-work query returned unsupported effect status {other}"
                    )));
                }
            }
        }
        Ok(due)
    }

    /// Promote an exactly due `retry_wait` effect back to eligible.
    ///
    /// # Errors
    ///
    /// Returns an error when the update fails.
    pub async fn promote_retry_due(
        &self,
        effect: &DueEffect,
        now: DateTime<Utc>,
    ) -> WorkflowRepositoryResult<bool> {
        let DueEffect::RetryWait {
            workflow_id,
            effect_id,
            declared_workflow_version,
            generation,
            next_eligible_at,
        } = effect
        else {
            return Ok(false);
        };

        let updated = sqlx::query(
            "UPDATE workflow_effects \
             SET status = 'eligible', next_eligible_at = NULL, pending_reconciliation = 0 \
             WHERE id = ?1 AND workflow_id = ?2 AND declared_workflow_version = ?3 \
               AND generation = ?4 AND status = 'retry_wait' AND pending_reconciliation = 0 \
               AND next_eligible_at = ?5 \
               AND next_eligible_at <= ?6 \
               AND NOT EXISTS (SELECT 1 FROM workflow_claims c WHERE c.effect_id = workflow_effects.id) \
               AND EXISTS ( \
                   SELECT 1 FROM workflows w \
                   WHERE w.id = workflow_effects.workflow_id \
                     AND w.authority = 'engine_protocol' \
                     AND w.execution_mode = 'authoritative' \
                     AND w.status = 'active' \
                     AND w.generation = workflow_effects.generation \
               )",
        )
        .bind(effect_id)
        .bind(workflow_id)
        .bind(to_i64(
            *declared_workflow_version,
            WorkflowRepositoryError::VersionOutOfRange,
        )?)
        .bind(to_i64(
            *generation,
            WorkflowRepositoryError::GenerationOutOfRange,
        )?)
        .bind(next_eligible_at.to_rfc3339())
        .bind(now.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected() == 1)
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
             SET status = 'claimed', pending_reconciliation = 1 \
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

async fn validate_receipt_codecs(
    tx: &mut SqliteConnection,
    request: &DurableAcceptReceiptRequest,
) -> WorkflowRepositoryResult<()> {
    validate_workflow_codecs(
        tx,
        &request.authority.workflow_id,
        [&request.receipt.codec, &request.reducer_event.codec],
    )
    .await
}

async fn validate_workflow_codecs<'a>(
    tx: &mut SqliteConnection,
    workflow_id: &str,
    codecs: impl IntoIterator<Item = &'a DurableCodecRef>,
) -> WorkflowRepositoryResult<()> {
    let selection_id: Option<String> =
        sqlx::query_scalar("SELECT protocol_selection_id FROM workflows WHERE id = ?1")
            .bind(workflow_id)
            .fetch_optional(&mut *tx)
            .await?;
    let Some(selection_id) = selection_id else {
        return Err(WorkflowRepositoryError::InvalidPlan(
            "codec validation requires an existing workflow",
        ));
    };
    if !selection_supports_codec_and_executor(tx, &selection_id, codecs, None).await? {
        return Err(WorkflowRepositoryError::InvalidPlan(
            "payload codec unsupported by workflow selection",
        ));
    }
    Ok(())
}

async fn preflight_receipt_acceptance<'e, E>(
    executor: E,
    request: &DurableAcceptReceiptRequest,
) -> WorkflowRepositoryResult<Option<AcceptReceiptResult>>
where
    E: Executor<'e, Database = Sqlite>,
{
    if let Some(existing) = load_receipt_for_effect(executor, &request.authority.effect_id).await? {
        return Ok(Some(compare_existing_receipt(existing, request)));
    }
    if request.attempt_id.is_none() {
        return Ok(Some(match request.origin {
            DurableReceiptOrigin::Manual => AcceptReceiptResult::Conflict,
            DurableReceiptOrigin::Execution
            | DurableReceiptOrigin::Adoption
            | DurableReceiptOrigin::Reconciliation => AcceptReceiptResult::StaleAuthority,
        }));
    }
    Ok((request.origin == DurableReceiptOrigin::Manual).then_some(AcceptReceiptResult::Conflict))
}

async fn record_observation_in_transaction(
    tx: &mut SqliteConnection,
    observation: &DurableObservationRecord,
) -> WorkflowRepositoryResult<bool> {
    validate_workflow_codecs(
        tx,
        &observation.authority.workflow_id,
        [&observation.payload.codec],
    )
    .await?;
    let effect_live = update_effect_status_if_live_claim(
        tx,
        &observation.authority,
        observation.recorded_at,
        "claimed",
        "claimed",
    )
    .await?;
    if effect_live != 1 {
        return Ok(false);
    }

    let attempt_updated = sqlx::query(
        "UPDATE workflow_attempts \
         SET status = 'observation_recorded' \
         WHERE id = ?1 AND effect_id = ?2 AND workflow_id = ?3 \
           AND declared_workflow_version = ?4 AND generation = ?5 \
           AND claim_token = ?6 AND claim_worker_id = ?7 \
           AND claim_lease_until = ?8 AND claim_issued_at = ?9 \
           AND status IN ('begun', 'observation_recorded')",
    )
    .bind(&observation.attempt_id)
    .bind(&observation.authority.effect_id)
    .bind(&observation.authority.workflow_id)
    .bind(to_i64(
        observation.authority.declared_workflow_version,
        WorkflowRepositoryError::VersionOutOfRange,
    )?)
    .bind(to_i64(
        observation.authority.generation,
        WorkflowRepositoryError::GenerationOutOfRange,
    )?)
    .bind(&observation.authority.claim_token)
    .bind(&observation.authority.worker_id)
    .bind(observation.authority.lease_until.to_rfc3339())
    .bind(observation.authority.issued_at.to_rfc3339())
    .execute(&mut *tx)
    .await?;
    if attempt_updated.rows_affected() != 1 {
        return Ok(false);
    }

    sqlx::query(
        "INSERT INTO workflow_observations \
         (id, effect_id, attempt_id, workflow_id, declared_workflow_version, generation, \
          claim_token, claim_worker_id, claim_lease_until, claim_issued_at, codec_family, \
          codec_version, payload, observed_at, recorded_at, authoritative) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, 1)",
    )
    .bind(&observation.observation_id)
    .bind(&observation.authority.effect_id)
    .bind(&observation.attempt_id)
    .bind(&observation.authority.workflow_id)
    .bind(to_i64(
        observation.authority.declared_workflow_version,
        WorkflowRepositoryError::VersionOutOfRange,
    )?)
    .bind(to_i64(
        observation.authority.generation,
        WorkflowRepositoryError::GenerationOutOfRange,
    )?)
    .bind(&observation.authority.claim_token)
    .bind(&observation.authority.worker_id)
    .bind(observation.authority.lease_until.to_rfc3339())
    .bind(observation.authority.issued_at.to_rfc3339())
    .bind(&observation.payload.codec.family)
    .bind(i64::from(observation.payload.codec.version))
    .bind(&observation.payload.payload)
    .bind(observation.observed_at.to_rfc3339())
    .bind(observation.recorded_at.to_rfc3339())
    .execute(&mut *tx)
    .await?;
    Ok(true)
}

async fn accept_receipt_in_transaction(
    tx: &mut SqliteConnection,
    request: &DurableAcceptReceiptRequest,
    failpoint: Option<WorkflowFailpoint>,
) -> WorkflowRepositoryResult<AcceptReceiptResult> {
    if let Some(existing) = load_receipt_for_effect(&mut *tx, &request.authority.effect_id).await? {
        return Ok(compare_existing_receipt(existing, request));
    }
    accept_new_receipt_in_transaction(tx, request, failpoint).await
}

#[allow(clippy::too_many_lines)]
async fn accept_new_receipt_in_transaction(
    tx: &mut SqliteConnection,
    request: &DurableAcceptReceiptRequest,
    failpoint: Option<WorkflowFailpoint>,
) -> WorkflowRepositoryResult<AcceptReceiptResult> {
    let Some(attempt_id) = request.attempt_id.as_ref() else {
        return Ok(AcceptReceiptResult::StaleAuthority);
    };
    let effect_live = update_effect_status_if_live_claim(
        tx,
        &request.authority,
        request.now,
        "claimed",
        "receipted",
    )
    .await?;
    if effect_live != 1 {
        return Ok(AcceptReceiptResult::StaleAuthority);
    }

    let attempt_updated = sqlx::query(
        "UPDATE workflow_attempts \
         SET status = 'receipt_accepted' \
         WHERE id = ?1 AND effect_id = ?2 AND workflow_id = ?3 \
           AND declared_workflow_version = ?4 AND generation = ?5 \
           AND claim_token = ?6 AND claim_worker_id = ?7 \
           AND claim_lease_until = ?8 AND claim_issued_at = ?9 \
           AND status IN ('begun', 'observation_recorded')",
    )
    .bind(attempt_id)
    .bind(&request.authority.effect_id)
    .bind(&request.authority.workflow_id)
    .bind(to_i64(
        request.authority.declared_workflow_version,
        WorkflowRepositoryError::VersionOutOfRange,
    )?)
    .bind(to_i64(
        request.authority.generation,
        WorkflowRepositoryError::GenerationOutOfRange,
    )?)
    .bind(&request.authority.claim_token)
    .bind(&request.authority.worker_id)
    .bind(request.authority.lease_until.to_rfc3339())
    .bind(request.authority.issued_at.to_rfc3339())
    .execute(&mut *tx)
    .await?;
    if attempt_updated.rows_affected() != 1 {
        return Ok(AcceptReceiptResult::StaleAuthority);
    }

    sqlx::query(
        "INSERT INTO workflow_receipts \
         (id, effect_id, attempt_id, workflow_id, declared_workflow_version, generation, \
          claim_token, claim_worker_id, claim_lease_until, claim_issued_at, codec_family, \
          codec_version, payload, origin, accepted_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
    )
    .bind(&request.receipt_id)
    .bind(&request.authority.effect_id)
    .bind(attempt_id)
    .bind(&request.authority.workflow_id)
    .bind(to_i64(
        request.authority.declared_workflow_version,
        WorkflowRepositoryError::VersionOutOfRange,
    )?)
    .bind(to_i64(
        request.authority.generation,
        WorkflowRepositoryError::GenerationOutOfRange,
    )?)
    .bind(&request.authority.claim_token)
    .bind(&request.authority.worker_id)
    .bind(request.authority.lease_until.to_rfc3339())
    .bind(request.authority.issued_at.to_rfc3339())
    .bind(&request.receipt.codec.family)
    .bind(i64::from(request.receipt.codec.version))
    .bind(&request.receipt.payload)
    .bind(receipt_origin_sql(request.origin))
    .bind(request.now.to_rfc3339())
    .execute(&mut *tx)
    .await?;

    fail_if_configured(tx, failpoint, WorkflowFailpoint::AfterReceiptInsert).await?;
    if let Some(projection) = &request.wake_terminal_projection {
        insert_wake_terminal_projection(tx, request, projection).await?;
        let current_snapshot: Option<String> = sqlx::query_scalar(
            "SELECT w.snapshot_payload FROM workflows w \
             JOIN wake_workflow_bindings b ON b.workflow_id = w.id \
             WHERE w.id = ?1 AND w.status = 'active'",
        )
        .bind(&request.authority.workflow_id)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(current_snapshot) = current_snapshot {
            let mut snapshot: serde_json::Value =
                serde_json::from_str(&current_snapshot).map_err(|error| {
                    WorkflowRepositoryError::CorruptState(format!(
                        "wake snapshot is invalid JSON while accepting terminal receipt: {error}"
                    ))
                })?;
            let object = snapshot.as_object_mut().ok_or_else(|| {
                WorkflowRepositoryError::CorruptState(
                    "wake snapshot is not an object while accepting terminal receipt".to_owned(),
                )
            })?;
            let terminal: serde_json::Value = serde_json::from_str(&request.reducer_event.payload)
                .map_err(|error| {
                    WorkflowRepositoryError::CorruptState(format!(
                        "wake terminal event is invalid JSON: {error}"
                    ))
                })?;
            object.insert("terminal".to_owned(), terminal);
            object.insert(
                "runtime_availability".to_owned(),
                serde_json::Value::String("terminal".to_owned()),
            );
            let workflow_updated = sqlx::query(
                "UPDATE workflows SET status = 'completed', snapshot_payload = ?1 \
             WHERE id = ?2 AND status = 'active' AND generation = ?3",
            )
            .bind(snapshot.to_string())
            .bind(&request.authority.workflow_id)
            .bind(to_i64(
                request.authority.generation,
                WorkflowRepositoryError::GenerationOutOfRange,
            )?)
            .execute(&mut *tx)
            .await?;
            if workflow_updated.rows_affected() != 1 {
                return Ok(AcceptReceiptResult::StaleAuthority);
            }
        }
    }

    let claim_deleted = sqlx::query(
        "DELETE FROM workflow_claims \
         WHERE effect_id = ?1 AND workflow_id = ?2 AND declared_workflow_version = ?3 \
           AND generation = ?4 AND claim_token = ?5 AND worker_id = ?6 \
           AND lease_until = ?7 AND issued_at = ?8",
    )
    .bind(&request.authority.effect_id)
    .bind(&request.authority.workflow_id)
    .bind(to_i64(
        request.authority.declared_workflow_version,
        WorkflowRepositoryError::VersionOutOfRange,
    )?)
    .bind(to_i64(
        request.authority.generation,
        WorkflowRepositoryError::GenerationOutOfRange,
    )?)
    .bind(&request.authority.claim_token)
    .bind(&request.authority.worker_id)
    .bind(request.authority.lease_until.to_rfc3339())
    .bind(request.authority.issued_at.to_rfc3339())
    .execute(&mut *tx)
    .await?;
    if claim_deleted.rows_affected() != 1 {
        return Ok(AcceptReceiptResult::StaleAuthority);
    }

    sqlx::query(
        "INSERT INTO workflow_reducer_inbox \
         (id, workflow_id, receipt_id, barrier_id, event_codec_family, event_codec_version, \
          event_payload, requires_runtime_acceptance, delivery_status, consumed_by_transition_id) \
         VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?6, 1, 'pending', NULL)",
    )
    .bind(&request.reducer_inbox_id)
    .bind(&request.authority.workflow_id)
    .bind(&request.receipt_id)
    .bind(&request.reducer_event.codec.family)
    .bind(i64::from(request.reducer_event.codec.version))
    .bind(&request.reducer_event.payload)
    .execute(&mut *tx)
    .await?;

    satisfy_newly_ready_barriers(tx, &request.authority.workflow_id, request.now).await?;

    if let Some(projection) = &request.wake_terminal_projection {
        let sequence: i64 = sqlx::query_scalar(
            "INSERT INTO wake_inbox_sequences (conversation_id, last_sequence) \
             SELECT conversation_id, 1 FROM wake_workflow_bindings WHERE workflow_id = ?1 \
             ON CONFLICT(conversation_id) DO UPDATE SET last_sequence = last_sequence + 1 \
             RETURNING last_sequence",
        )
        .bind(&request.authority.workflow_id)
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO wake_observation_inbox \
             (id, workflow_id, contract_id, terminal_receipt_id, conversation_id, sequence, committed_at, consumed_at) \
             SELECT ?1, b.workflow_id, b.contract_id, ?2, b.conversation_id, ?3, ?4, NULL \
             FROM wake_workflow_bindings b WHERE b.workflow_id = ?5",
        )
        .bind(&request.reducer_inbox_id)
        .bind(&request.receipt_id)
        .bind(sequence)
        .bind(request.now.to_rfc3339())
        .bind(&request.authority.workflow_id)
        .execute(&mut *tx)
        .await?;
        let conversation_id: String = sqlx::query_scalar(
            "SELECT conversation_id FROM wake_workflow_bindings WHERE workflow_id = ?1",
        )
        .bind(&request.authority.workflow_id)
        .fetch_one(&mut *tx)
        .await?;
        let existing_obligation: Option<String> = sqlx::query_scalar(
            "SELECT id FROM wake_runtime_obligations \
             WHERE conversation_id = ?1 AND status = 'owed' ORDER BY created_at LIMIT 1",
        )
        .bind(&conversation_id)
        .fetch_optional(&mut *tx)
        .await?;
        let obligation_id = if let Some(id) = existing_obligation {
            sqlx::query(
                "UPDATE wake_runtime_obligations SET snapshot_upper_bound = MAX(snapshot_upper_bound, ?1) \
                 WHERE id = ?2 AND status = 'owed'",
            )
            .bind(sequence)
            .bind(&id)
            .execute(&mut *tx)
            .await?;
            id
        } else {
            let id = format!(
                "wake-obligation:{}:{sequence}",
                request.authority.workflow_id
            );
            sqlx::query(
                "INSERT INTO wake_runtime_obligations \
                 (id, conversation_id, snapshot_upper_bound, status, created_at, resolved_at, terminal_reason) \
                 VALUES (?1, ?2, ?3, 'owed', ?4, NULL, NULL)",
            )
            .bind(&id)
            .bind(&conversation_id)
            .bind(sequence)
            .bind(request.now.to_rfc3339())
            .execute(&mut *tx)
            .await?;
            id
        };
        let ordinal: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM wake_runtime_obligation_items WHERE obligation_id = ?1",
        )
        .bind(&obligation_id)
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO wake_runtime_obligation_items (obligation_id, ordinal, inbox_item_id) \
             VALUES (?1, ?2, ?3)",
        )
        .bind(&obligation_id)
        .bind(ordinal)
        .bind(&request.reducer_inbox_id)
        .execute(&mut *tx)
        .await?;
        let stored_contract: String =
            sqlx::query_scalar("SELECT contract_id FROM wake_observation_inbox WHERE id = ?1")
                .bind(&request.reducer_inbox_id)
                .fetch_one(&mut *tx)
                .await?;
        if stored_contract != projection.contract_id {
            return Err(WorkflowRepositoryError::CorruptState(
                "wake observation inbox contract projection mismatch".to_owned(),
            ));
        }
    }

    Ok(AcceptReceiptResult::Accepted {
        receipt: DurableReceiptAcceptance {
            receipt_id: request.receipt_id.clone(),
            authority: request.authority.clone(),
            attempt_id: attempt_id.clone(),
            payload: request.receipt.clone(),
            origin: request.origin,
            accepted_at: request.now,
        },
        reducer_inbox: DurableReducerInboxRecord {
            reducer_inbox_id: request.reducer_inbox_id.clone(),
            workflow_id: request.authority.workflow_id.clone(),
            receipt_id: request.receipt_id.clone(),
            event: request.reducer_event.clone(),
            requires_runtime_acceptance: true,
        },
    })
}

async fn satisfy_newly_ready_barriers(
    tx: &mut SqliteConnection,
    workflow_id: &str,
    satisfied_at: DateTime<Utc>,
) -> WorkflowRepositoryResult<()> {
    let ready = sqlx::query(
        "SELECT b.id, b.event_codec_family, b.event_codec_version, b.event_payload \
         FROM workflow_barriers b \
         WHERE b.workflow_id = ?1 AND b.status = 'waiting' \
           AND NOT EXISTS (\
             SELECT 1 FROM workflow_barrier_members bm \
             JOIN workflow_effects e ON e.id = bm.effect_id \
             JOIN workflows w ON w.id = e.workflow_id \
             WHERE bm.barrier_id = b.id \
               AND (e.status <> 'receipted' OR e.generation <> w.generation \
                    OR (bm.receipt_family = 'compensation_effect' AND e.role <> 'compensation') \
                    OR (bm.receipt_family = 'current_generation_effect' AND e.role = 'compensation'))\
           )",
    )
    .bind(workflow_id)
    .fetch_all(&mut *tx)
    .await?;

    for barrier in ready {
        let barrier_id: String = barrier.get("id");
        let updated = sqlx::query(
            "UPDATE workflow_barriers SET status = 'satisfied', satisfied_at = ?1 \
             WHERE id = ?2 AND workflow_id = ?3 AND status = 'waiting'",
        )
        .bind(satisfied_at.to_rfc3339())
        .bind(&barrier_id)
        .bind(workflow_id)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() != 1 {
            continue;
        }
        sqlx::query(
            "INSERT INTO workflow_reducer_inbox \
             (id, workflow_id, receipt_id, barrier_id, event_codec_family, event_codec_version, \
              event_payload, requires_runtime_acceptance, delivery_status, consumed_by_transition_id) \
             VALUES (?1, ?2, NULL, ?3, ?4, ?5, ?6, 0, 'pending', NULL)",
        )
        .bind(format!("barrier:{barrier_id}:satisfied"))
        .bind(workflow_id)
        .bind(&barrier_id)
        .bind(barrier.get::<String, _>("event_codec_family"))
        .bind(barrier.get::<i64, _>("event_codec_version"))
        .bind(barrier.get::<String, _>("event_payload"))
        .execute(&mut *tx)
        .await?;
    }
    Ok(())
}

async fn load_receipt_for_effect<'e, E>(
    executor: E,
    effect_id: &str,
) -> WorkflowRepositoryResult<Option<(DurableReceiptAcceptance, DurablePayload)>>
where
    E: Executor<'e, Database = Sqlite>,
{
    let row = sqlx::query(
        "SELECT r.id, r.workflow_id, r.declared_workflow_version, r.generation, r.effect_id, \
                r.claim_token, r.claim_worker_id, r.claim_lease_until, r.claim_issued_at, \
                r.attempt_id, r.codec_family, r.codec_version, r.payload, r.origin, r.accepted_at, \
                i.event_codec_family, i.event_codec_version, i.event_payload \
         FROM workflow_receipts r \
         JOIN workflow_reducer_inbox i ON i.receipt_id = r.id AND i.workflow_id = r.workflow_id \
         WHERE r.effect_id = ?1",
    )
    .bind(effect_id)
    .fetch_optional(executor)
    .await?;

    Ok(row.map(|row| {
        let receipt = DurableReceiptAcceptance {
            receipt_id: row.get("id"),
            authority: DurableClaimAuthority {
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
                claim_token: row
                    .get::<Option<String>, _>("claim_token")
                    .unwrap_or_default(),
                worker_id: row
                    .get::<Option<String>, _>("claim_worker_id")
                    .unwrap_or_default(),
                lease_until: row
                    .get::<Option<String>, _>("claim_lease_until")
                    .map_or_else(epoch_utc, |value| {
                        DateTime::parse_from_rfc3339(&value)
                            .expect("claim lease_until is valid RFC3339")
                            .with_timezone(&Utc)
                    }),
                issued_at: row.get::<Option<String>, _>("claim_issued_at").map_or_else(
                    epoch_utc,
                    |value| {
                        DateTime::parse_from_rfc3339(&value)
                            .expect("claim issued_at is valid RFC3339")
                            .with_timezone(&Utc)
                    },
                ),
            },
            attempt_id: row
                .get::<Option<String>, _>("attempt_id")
                .unwrap_or_default(),
            payload: DurablePayload {
                codec: DurableCodecRef {
                    family: row.get("codec_family"),
                    version: row
                        .get::<i64, _>("codec_version")
                        .try_into()
                        .expect("codec version fits u32"),
                },
                payload: row.get("payload"),
            },
            origin: parse_receipt_origin_sql(row.get::<String, _>("origin").as_str())
                .expect("workflow_receipts.origin is constrained to known values"),
            accepted_at: DateTime::parse_from_rfc3339(&row.get::<String, _>("accepted_at"))
                .expect("accepted_at is valid RFC3339")
                .with_timezone(&Utc),
        };
        let reducer_event = DurablePayload {
            codec: DurableCodecRef {
                family: row.get("event_codec_family"),
                version: row
                    .get::<i64, _>("event_codec_version")
                    .try_into()
                    .expect("event codec version fits u32"),
            },
            payload: row.get("event_payload"),
        };
        (receipt, reducer_event)
    }))
}

fn compare_existing_receipt(
    existing: (DurableReceiptAcceptance, DurablePayload),
    request: &DurableAcceptReceiptRequest,
) -> AcceptReceiptResult {
    let (existing, reducer_event) = existing;
    if existing.origin == request.origin
        && existing.payload == request.receipt
        && reducer_event == request.reducer_event
        && existing.attempt_id == request.attempt_id.clone().unwrap_or_default()
    {
        AcceptReceiptResult::AlreadyReceipted { receipt: existing }
    } else {
        AcceptReceiptResult::Conflict
    }
}

#[derive(Debug, Clone, Copy)]
struct ReconcilableEffectContext {
    ambiguity_policy: EffectAmbiguity,
}

async fn load_reconcilable_effect_context(
    tx: &mut SqliteConnection,
    authority: &DurableClaimAuthority,
    now: DateTime<Utc>,
) -> WorkflowRepositoryResult<Option<ReconcilableEffectContext>> {
    let row = sqlx::query(
        "SELECT e.ambiguity_policy \
         FROM workflow_effects e \
         JOIN workflows w ON w.id = e.workflow_id \
         JOIN workflow_claims c \
           ON c.effect_id = e.id AND c.workflow_id = e.workflow_id \
          AND c.declared_workflow_version = e.declared_workflow_version \
          AND c.generation = e.generation \
         WHERE e.id = ?1 AND e.workflow_id = ?2 AND e.declared_workflow_version = ?3 \
           AND e.generation = ?4 AND e.status = 'claimed' \
           AND c.claim_token = ?5 AND c.worker_id = ?6 AND c.lease_until = ?7 \
           AND c.issued_at = ?8 AND c.lease_until > ?9 \
           AND w.authority = 'engine_protocol' AND w.execution_mode = 'authoritative' \
           AND w.status = 'active' AND w.generation = e.generation",
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
    .bind(&authority.claim_token)
    .bind(&authority.worker_id)
    .bind(authority.lease_until.to_rfc3339())
    .bind(authority.issued_at.to_rfc3339())
    .bind(now.to_rfc3339())
    .fetch_optional(&mut *tx)
    .await?;

    Ok(row.map(|row| ReconcilableEffectContext {
        ambiguity_policy: parse_effect_ambiguity_sql(
            row.get::<String, _>("ambiguity_policy").as_str(),
        )
        .expect("workflow_effects.ambiguity_policy is constrained to known values"),
    }))
}

#[allow(clippy::too_many_lines)]
async fn finalize_reconciliation_transition(
    tx: &mut SqliteConnection,
    authority: &DurableClaimAuthority,
    now: DateTime<Utc>,
    next_status: &str,
    next_eligible_at: Option<DateTime<Utc>>,
    pending_reconciliation: bool,
) -> WorkflowRepositoryResult<()> {
    let effect_updated = sqlx::query(
        "UPDATE workflow_effects \
         SET status = ?1, next_eligible_at = ?2, pending_reconciliation = ?3 \
         WHERE id = ?4 AND workflow_id = ?5 AND declared_workflow_version = ?6 \
           AND generation = ?7 AND status = 'claimed' \
           AND EXISTS (\
               SELECT 1 FROM workflow_claims c \
               JOIN workflows w ON w.id = workflow_effects.workflow_id \
               WHERE c.effect_id = workflow_effects.id AND c.workflow_id = workflow_effects.workflow_id \
                 AND c.declared_workflow_version = workflow_effects.declared_workflow_version \
                 AND c.generation = workflow_effects.generation \
                 AND c.claim_token = ?8 AND c.worker_id = ?9 \
                 AND c.lease_until = ?10 AND c.issued_at = ?11 \
                 AND c.lease_until > ?12 \
                 AND w.authority = 'engine_protocol' AND w.execution_mode = 'authoritative' \
                 AND w.status = 'active' AND w.generation = workflow_effects.generation\
           )",
    )
    .bind(next_status)
    .bind(next_eligible_at.map(|ts| ts.to_rfc3339()))
    .bind(pending_reconciliation)
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
    .bind(now.to_rfc3339())
    .execute(&mut *tx)
    .await?;
    if effect_updated.rows_affected() != 1 {
        return Err(WorkflowRepositoryError::InvalidPlan(
            "reconciliation transition requires exact live claimed effect",
        ));
    }

    let attempt_updated = sqlx::query(
        "UPDATE workflow_attempts \
         SET status = CASE status \
             WHEN 'begun' THEN 'authority_lost' \
             WHEN 'observation_recorded' THEN 'observation_recorded' \
             ELSE status END \
         WHERE effect_id = ?1 AND workflow_id = ?2 AND declared_workflow_version = ?3 \
           AND generation = ?4 AND claim_token = ?5 AND claim_worker_id = ?6 \
           AND claim_lease_until = ?7 AND claim_issued_at = ?8 \
           AND status IN ('begun', 'observation_recorded')",
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
    .bind(&authority.claim_token)
    .bind(&authority.worker_id)
    .bind(authority.lease_until.to_rfc3339())
    .bind(authority.issued_at.to_rfc3339())
    .execute(&mut *tx)
    .await?;
    if attempt_updated.rows_affected() != 1 {
        return Err(WorkflowRepositoryError::InvalidPlan(
            "reconciliation transition requires exact live attempt row",
        ));
    }

    let deleted = sqlx::query(
        "DELETE FROM workflow_claims \
         WHERE effect_id = ?1 AND workflow_id = ?2 AND declared_workflow_version = ?3 \
           AND generation = ?4 AND claim_token = ?5 AND worker_id = ?6 \
           AND lease_until = ?7 AND issued_at = ?8",
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
    .bind(&authority.claim_token)
    .bind(&authority.worker_id)
    .bind(authority.lease_until.to_rfc3339())
    .bind(authority.issued_at.to_rfc3339())
    .execute(&mut *tx)
    .await?;
    if deleted.rows_affected() != 1 {
        return Err(WorkflowRepositoryError::InvalidPlan(
            "reconciliation transition requires exact live claim row",
        ));
    }
    if effect_updated.rows_affected() != 1 {
        return Err(WorkflowRepositoryError::InvalidPlan(
            "reconciliation transition requires exact live claimed effect",
        ));
    }

    Ok(())
}

async fn codec_supported_by_workflow(
    tx: &mut SqliteConnection,
    workflow_id: &str,
    codec: &DurableCodecRef,
) -> WorkflowRepositoryResult<bool> {
    let exists: Option<i64> = sqlx::query_scalar(
        "SELECT 1 \
         FROM workflows w \
         JOIN workflow_profile_codecs c ON c.selection_id = w.protocol_selection_id \
         WHERE w.id = ?1 AND c.codec_family = ?2 AND c.codec_version = ?3",
    )
    .bind(workflow_id)
    .bind(&codec.family)
    .bind(i64::from(codec.version))
    .fetch_optional(&mut *tx)
    .await?;
    Ok(exists.is_some())
}

fn choice_kind_is_supported(kind: &str) -> bool {
    matches!(kind, "adopt" | "retry" | "compensate" | "fail" | "suppress")
}

async fn manual_evidence_link_exists(
    tx: &mut SqliteConnection,
    workflow_id: &str,
    effect_id: &str,
    evidence_kind: &str,
    evidence_id: &str,
) -> WorkflowRepositoryResult<bool> {
    let exists = match evidence_kind {
        "authoritative_observation" => {
            sqlx::query_scalar::<_, i64>(
                "SELECT 1 FROM workflow_observations WHERE id = ?1 AND workflow_id = ?2 AND effect_id = ?3",
            )
            .bind(evidence_id)
            .bind(workflow_id)
            .bind(effect_id)
            .fetch_optional(&mut *tx)
            .await?
            .is_some()
        }
        "stale_observation" => {
            sqlx::query_scalar::<_, i64>(
                "SELECT 1 FROM workflow_stale_observations WHERE id = ?1 AND workflow_id = ?2 AND effect_id = ?3",
            )
            .bind(evidence_id)
            .bind(workflow_id)
            .bind(effect_id)
            .fetch_optional(&mut *tx)
            .await?
            .is_some()
        }
        _ => false,
    };
    Ok(exists)
}

async fn update_effect_status_if_live_claim(
    tx: &mut SqliteConnection,
    authority: &DurableClaimAuthority,
    now: DateTime<Utc>,
    expected_status: &str,
    next_status: &str,
) -> WorkflowRepositoryResult<u64> {
    let updated = sqlx::query(
        "UPDATE workflow_effects \
         SET status = ?1 \
         WHERE id = ?2 AND workflow_id = ?3 AND declared_workflow_version = ?4 \
           AND generation = ?5 AND status = ?6 \
           AND EXISTS (\
               SELECT 1 FROM workflow_claims c \
               JOIN workflows w ON w.id = workflow_effects.workflow_id \
               WHERE c.effect_id = workflow_effects.id AND c.workflow_id = workflow_effects.workflow_id \
                 AND c.declared_workflow_version = workflow_effects.declared_workflow_version \
                 AND c.generation = workflow_effects.generation \
                 AND c.claim_token = ?7 AND c.worker_id = ?8 \
                 AND c.lease_until = ?9 AND c.issued_at = ?10 \
                 AND c.lease_until > ?11 \
                 AND w.authority = 'engine_protocol' AND w.execution_mode = 'authoritative' \
                 AND w.status = 'active' AND w.generation = workflow_effects.generation\
           )",
    )
    .bind(next_status)
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
    .bind(expected_status)
    .bind(&authority.claim_token)
    .bind(&authority.worker_id)
    .bind(authority.lease_until.to_rfc3339())
    .bind(authority.issued_at.to_rfc3339())
    .bind(now.to_rfc3339())
    .execute(&mut *tx)
    .await?;
    Ok(updated.rows_affected())
}

async fn insert_wake_terminal_projection(
    tx: &mut SqliteConnection,
    request: &DurableAcceptReceiptRequest,
    projection: &DurableWakeTerminalProjection,
) -> WorkflowRepositoryResult<()> {
    sqlx::query(
        "INSERT INTO wake_terminal_receipts \
         (receipt_id, workflow_id, contract_id, observe_effect_id, resource_kind, status, resolved_at, \
          bash_status, bash_occurred_at, bash_exit_code, bash_duration_ms, bash_signal_number, \
          bash_kill_signal_sent, bash_tail_start_offset, bash_tail_end_offset, bash_tail_truncated_before, \
          tmux_status, tmux_occurred_at, tmux_server_generation, tmux_exit_code, tmux_duration_ms, \
          forgotten_reason, cancellation_reason) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23)",
    )
    .bind(&request.receipt_id).bind(&request.authority.workflow_id).bind(&projection.contract_id)
    .bind(&request.authority.effect_id).bind(&projection.resource_kind).bind(&projection.status)
    .bind(projection.resolved_at.to_rfc3339()).bind(&projection.bash_status)
    .bind(projection.bash_occurred_at.map(|value| value.to_rfc3339())).bind(projection.bash_exit_code)
    .bind(projection.bash_duration_ms.map(i64::try_from).transpose().map_err(|_| WorkflowRepositoryError::CorruptState("bash duration exceeds SQLite range".to_owned()))?)
    .bind(projection.bash_signal_number).bind(&projection.bash_kill_signal_sent)
    .bind(projection.bash_tail_start_offset.map(i64::try_from).transpose().map_err(|_| WorkflowRepositoryError::CorruptState("bash tail start offset exceeds SQLite range".to_owned()))?)
    .bind(projection.bash_tail_end_offset.map(i64::try_from).transpose().map_err(|_| WorkflowRepositoryError::CorruptState("bash tail end offset exceeds SQLite range".to_owned()))?)
    .bind(projection.bash_tail_truncated_before).bind(&projection.tmux_status)
    .bind(projection.tmux_occurred_at.map(|value| value.to_rfc3339())).bind(&projection.tmux_server_generation)
    .bind(projection.tmux_exit_code)
    .bind(projection.tmux_duration_ms.map(i64::try_from).transpose().map_err(|_| WorkflowRepositoryError::CorruptState("tmux duration exceeds SQLite range".to_owned()))?)
    .bind(&projection.forgotten_reason).bind(&projection.cancellation_reason)
    .execute(&mut *tx).await?;
    for (ordinal, line) in projection.bash_tail.iter().enumerate() {
        sqlx::query("INSERT INTO wake_terminal_receipt_bash_tail (receipt_id, ordinal, stream, offset, line) VALUES (?1, ?2, NULL, ?3, ?4)")
            .bind(&request.receipt_id)
            .bind(i64::try_from(ordinal).expect("tail ordinal fits i64"))
            .bind(i64::try_from(line.offset).map_err(|_| WorkflowRepositoryError::CorruptState("bash tail line offset exceeds SQLite range".to_owned()))?)
            .bind(&line.line)
            .execute(&mut *tx).await?;
    }
    for (ordinal, line) in projection.tmux_tail.iter().enumerate() {
        sqlx::query("INSERT INTO wake_terminal_receipt_tmux_tail (receipt_id, ordinal, line) VALUES (?1, ?2, ?3)")
            .bind(&request.receipt_id).bind(i64::try_from(ordinal).expect("tail ordinal fits i64")).bind(line)
            .execute(&mut *tx).await?;
    }
    Ok(())
}

fn epoch_utc() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("1970-01-01T00:00:00Z")
        .expect("epoch timestamp is valid RFC3339")
        .with_timezone(&Utc)
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

pub(super) fn is_busy_or_locked(err: &sqlx::Error) -> bool {
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

fn divergence_resolution_action_sql(action: DurableDivergenceResolutionAction) -> &'static str {
    match action {
        DurableDivergenceResolutionAction::Rollback => "rollback",
        DurableDivergenceResolutionAction::Reauthorize => "reauthorize",
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

fn parse_effect_ambiguity_sql(ambiguity: &str) -> Option<EffectAmbiguity> {
    match ambiguity {
        "observable_reconciliation" => Some(EffectAmbiguity::ObservableReconciliation),
        "external_idempotency" => Some(EffectAmbiguity::ExternalIdempotency),
        "safe_repeatability" => Some(EffectAmbiguity::SafeRepeatability),
        "manual_resolution" => Some(EffectAmbiguity::ManualResolution),
        _ => None,
    }
}

fn receipt_family_sql(family: ReceiptFamily) -> &'static str {
    match family {
        ReceiptFamily::CurrentGenerationEffect => "current_generation_effect",
        ReceiptFamily::CompensationEffect => "compensation_effect",
    }
}

fn receipt_origin_sql(origin: DurableReceiptOrigin) -> &'static str {
    match origin {
        DurableReceiptOrigin::Execution => "execution",
        DurableReceiptOrigin::Adoption => "adoption",
        DurableReceiptOrigin::Reconciliation => "reconciliation",
        DurableReceiptOrigin::Manual => "manual",
    }
}

fn parse_receipt_origin_sql(origin: &str) -> Option<DurableReceiptOrigin> {
    match origin {
        "execution" => Some(DurableReceiptOrigin::Execution),
        "adoption" => Some(DurableReceiptOrigin::Adoption),
        "reconciliation" => Some(DurableReceiptOrigin::Reconciliation),
        "manual" => Some(DurableReceiptOrigin::Manual),
        _ => None,
    }
}

pub mod wake;

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
        let temp_dir = tempfile::Builder::new()
            .prefix("phoenix-db-workflow-")
            .tempdir()
            .unwrap()
            .keep();
        let path = temp_dir.join(format!("{unique}.sqlite"));
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

    fn observation(
        authority: &DurableClaimAuthority,
        attempt_id: &str,
        now: DateTime<Utc>,
    ) -> DurableObservationRecord {
        DurableObservationRecord {
            observation_id: "obs-1".to_owned(),
            authority: authority.clone(),
            attempt_id: attempt_id.to_owned(),
            payload: DurablePayload {
                codec: DurableCodecRef {
                    family: "event".to_owned(),
                    version: 1,
                },
                payload: "observed".to_owned(),
            },
            observed_at: now,
            recorded_at: now,
        }
    }

    fn stale_observation(
        authority: &DurableClaimAuthority,
        attempt_id: &str,
        now: DateTime<Utc>,
    ) -> DurableStaleObservationRecord {
        DurableStaleObservationRecord {
            observation_id: "stale-obs-1".to_owned(),
            authority: authority.clone(),
            attempt_id: attempt_id.to_owned(),
            payload: DurablePayload {
                codec: DurableCodecRef {
                    family: "event".to_owned(),
                    version: 1,
                },
                payload: "observed-stale".to_owned(),
            },
            observed_at: now,
            recorded_at: now,
            stale_reason: "claim_stale".to_owned(),
        }
    }

    fn receipt_request(
        authority: &DurableClaimAuthority,
        attempt_id: Option<&str>,
        now: DateTime<Utc>,
    ) -> DurableAcceptReceiptRequest {
        DurableAcceptReceiptRequest {
            receipt_id: "receipt-1".to_owned(),
            reducer_inbox_id: "inbox-receipt-1".to_owned(),
            authority: authority.clone(),
            now,
            attempt_id: attempt_id.map(ToOwned::to_owned),
            origin: DurableReceiptOrigin::Execution,
            receipt: DurablePayload {
                codec: DurableCodecRef {
                    family: "event".to_owned(),
                    version: 1,
                },
                payload: "receipt-payload".to_owned(),
            },
            reducer_event: DurablePayload {
                codec: DurableCodecRef {
                    family: "event".to_owned(),
                    version: 1,
                },
                payload: "receipt-event".to_owned(),
            },
            wake_terminal_projection: None,
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

    async fn seed_manual_only_effect(repo: &WorkflowRepository) {
        register_and_accept(repo).await;
        let mut commit = transition_commit();
        commit.effects[0].ambiguity_policy = EffectAmbiguity::ManualResolution;
        let outcome = repo.persist_transition_plan(&commit).await.unwrap();
        assert_eq!(outcome, TransitionCommitOutcome::Committed);
    }

    async fn seed_manual_resolution_context(
        repo: &WorkflowRepository,
    ) -> (DurableClaimAuthority, DurableAttemptRecord) {
        seed_manual_only_effect(repo).await;
        let now = Utc::now();
        let claim = repo
            .claim_effect(&claim_request(now, now + chrono::Duration::seconds(30)))
            .await
            .unwrap();
        let (authority, attempt) = match claim {
            ClaimEffectResult::Claimed { authority, attempt } => (authority, *attempt),
            other @ (ClaimEffectResult::Ineligible | ClaimEffectResult::Contended) => {
                panic!("expected claimed, got {other:?}")
            }
        };
        let recorded = repo
            .record_observation(&observation(
                &authority,
                &attempt.attempt_id,
                now + chrono::Duration::seconds(1),
            ))
            .await
            .unwrap();
        assert!(matches!(recorded, RecordObservationResult::Recorded { .. }));
        let retained = repo
            .retain_stale_observation(&stale_observation(
                &authority,
                &attempt.attempt_id,
                now + chrono::Duration::seconds(2),
            ))
            .await
            .unwrap();
        assert!(matches!(
            retained,
            RetainStaleObservationResult::Recorded { .. }
        ));
        (authority, attempt)
    }

    fn manual_resolution_request(
        authority: &DurableClaimAuthority,
        now: DateTime<Utc>,
    ) -> DurableManualResolutionRequest {
        DurableManualResolutionRequest {
            resolution_id: "mr-1".to_owned(),
            authority: authority.clone(),
            now,
            evidence: DurablePayload {
                codec: DurableCodecRef {
                    family: "event".to_owned(),
                    version: 1,
                },
                payload: "manual-evidence".to_owned(),
            },
            evidence_links: vec![
                ("authoritative_observation".to_owned(), "obs-1".to_owned()),
                ("stale_observation".to_owned(), "stale-obs-1".to_owned()),
            ],
            choices: vec![
                DurableManualResolutionChoice {
                    choice_id: "choice-adopt".to_owned(),
                    kind: "adopt".to_owned(),
                    payload: DurablePayload {
                        codec: DurableCodecRef {
                            family: "event".to_owned(),
                            version: 1,
                        },
                        payload: "adopt-choice".to_owned(),
                    },
                },
                DurableManualResolutionChoice {
                    choice_id: "choice-retry".to_owned(),
                    kind: "retry".to_owned(),
                    payload: DurablePayload {
                        codec: DurableCodecRef {
                            family: "event".to_owned(),
                            version: 1,
                        },
                        payload: "retry-choice".to_owned(),
                    },
                },
            ],
        }
    }

    async fn assert_manual_resolution_state_unchanged(pool: &SqlitePool) {
        let effect = sqlx::query(
            "SELECT status, next_eligible_at, pending_reconciliation FROM workflow_effects WHERE id = 'eff-1'",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        let claim_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM workflow_claims WHERE effect_id = 'eff-1'")
                .fetch_one(pool)
                .await
                .unwrap();
        let resolutions: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM workflow_manual_resolutions WHERE effect_id = 'eff-1'",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        let choice_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM workflow_manual_resolution_choices WHERE workflow_id = 'wf-1'",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        let evidence_link_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM workflow_manual_resolution_evidence_links")
                .fetch_one(pool)
                .await
                .unwrap();
        assert_eq!(effect.get::<String, _>("status"), "claimed");
        assert_eq!(effect.get::<Option<String>, _>("next_eligible_at"), None);
        assert_eq!(effect.get::<i64, _>("pending_reconciliation"), 0);
        assert_eq!(claim_count, 1);
        assert_eq!(resolutions, 0);
        assert_eq!(choice_count, 0);
        assert_eq!(evidence_link_count, 0);
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

        let renewed_until = now + chrono::Duration::seconds(60);
        let renewed = repo
            .renew_claim(&DurableClaimRenewal {
                authority: authority.clone(),
                now: now + chrono::Duration::seconds(5),
                lease_until: renewed_until,
            })
            .await
            .unwrap();
        let RenewClaimResult::Renewed {
            authority: renewed_authority,
        } = renewed
        else {
            panic!("expected renewed authority");
        };
        assert_eq!(renewed_authority.lease_until, renewed_until);
        let leases: (String, String) = sqlx::query_as(
            "SELECT c.lease_until, a.claim_lease_until FROM workflow_claims c \
             JOIN workflow_attempts a ON a.effect_id = c.effect_id \
             WHERE c.effect_id = 'eff-1' AND a.status = 'begun'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(leases.0, renewed_until.to_rfc3339());
        assert_eq!(leases.1, renewed_until.to_rfc3339());

        let pre_expiry = repo
            .take_over_expired_claim(&DurableClaimTakeover {
                authority: renewed_authority,
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
    async fn record_observation_requires_live_exact_authority_and_attempt() {
        let pool = test_pool().await;
        let repo = WorkflowRepository::new(pool.clone());
        seed_claimable_effect(&repo).await;

        let now = Utc::now();
        let claim = repo
            .claim_effect(&claim_request(now, now + chrono::Duration::seconds(30)))
            .await
            .unwrap();
        let (authority, attempt) = match claim {
            ClaimEffectResult::Claimed { authority, attempt } => (authority, attempt),
            other @ (ClaimEffectResult::Ineligible | ClaimEffectResult::Contended) => {
                panic!("expected claimed, got {other:?}")
            }
        };

        let mismatch = repo
            .record_observation(&observation(
                &authority,
                "attempt-999",
                now + chrono::Duration::seconds(1),
            ))
            .await
            .unwrap();
        assert_eq!(mismatch, RecordObservationResult::StaleAuthority);

        let recorded = repo
            .record_observation(&observation(
                &authority,
                &attempt.attempt_id,
                now + chrono::Duration::seconds(1),
            ))
            .await
            .unwrap();
        assert!(matches!(recorded, RecordObservationResult::Recorded { .. }));

        let observation_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM workflow_observations WHERE effect_id = 'eff-1' AND authoritative = 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let attempt_status: String =
            sqlx::query_scalar("SELECT status FROM workflow_attempts WHERE id = ?1")
                .bind(&attempt.attempt_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(observation_count, 1);
        assert_eq!(attempt_status, "observation_recorded");
    }

    #[tokio::test]
    async fn two_authoritative_observations_on_the_same_attempt_are_recorded() {
        let pool = test_pool().await;
        let repo = WorkflowRepository::new(pool.clone());
        seed_claimable_effect(&repo).await;
        let now = Utc::now();
        let (authority, attempt) = match repo
            .claim_effect(&claim_request(now, now + chrono::Duration::seconds(30)))
            .await
            .unwrap()
        {
            ClaimEffectResult::Claimed { authority, attempt } => (authority, attempt),
            other @ (ClaimEffectResult::Ineligible | ClaimEffectResult::Contended) => {
                panic!("expected claim, got {other:?}")
            }
        };
        let first = observation(
            &authority,
            &attempt.attempt_id,
            now + chrono::Duration::seconds(1),
        );
        let mut second = observation(
            &authority,
            &attempt.attempt_id,
            now + chrono::Duration::seconds(2),
        );
        second.observation_id = "obs-2".to_owned();
        second.payload.payload = "observed-again".to_owned();

        assert!(matches!(
            repo.record_observation(&first).await.unwrap(),
            RecordObservationResult::Recorded { .. }
        ));
        assert!(matches!(
            repo.record_observation(&second).await.unwrap(),
            RecordObservationResult::Recorded { .. }
        ));
        let ids: Vec<String> = sqlx::query_scalar(
            "SELECT id FROM workflow_observations WHERE attempt_id = ?1 ORDER BY recorded_at, id",
        )
        .bind(&attempt.attempt_id)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(ids, vec!["obs-1".to_owned(), "obs-2".to_owned()]);
    }

    #[tokio::test]
    async fn record_observation_and_accept_receipt_rolls_back_every_write_after_receipt_failpoint()
    {
        let pool = test_pool().await;
        let repo = WorkflowRepository::new(pool.clone());
        seed_claimable_effect(&repo).await;
        let now = Utc::now();
        let (authority, attempt) = match repo
            .claim_effect(&claim_request(now, now + chrono::Duration::seconds(30)))
            .await
            .unwrap()
        {
            ClaimEffectResult::Claimed { authority, attempt } => (authority, attempt),
            other @ (ClaimEffectResult::Ineligible | ClaimEffectResult::Contended) => {
                panic!("expected claimed, got {other:?}")
            }
        };
        let err = repo
            .record_observation_and_accept_receipt_with_failpoint(
                &observation(
                    &authority,
                    &attempt.attempt_id,
                    now + chrono::Duration::seconds(1),
                ),
                &receipt_request(
                    &authority,
                    Some(&attempt.attempt_id),
                    now + chrono::Duration::seconds(1),
                ),
                Some(WorkflowFailpoint::AfterReceiptInsert),
            )
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            WorkflowRepositoryError::Failpoint(WorkflowFailpoint::AfterReceiptInsert)
        ));

        let observations: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM workflow_observations WHERE effect_id = 'eff-1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let receipts: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM workflow_receipts WHERE effect_id = 'eff-1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        let inbox: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM workflow_reducer_inbox WHERE workflow_id = 'wf-1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let attempt_status: String =
            sqlx::query_scalar("SELECT status FROM workflow_attempts WHERE id = ?1")
                .bind(&attempt.attempt_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        let effect_status: String =
            sqlx::query_scalar("SELECT status FROM workflow_effects WHERE id = 'eff-1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        let claims: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM workflow_claims WHERE effect_id = 'eff-1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(observations, 0);
        assert_eq!(receipts, 0);
        assert_eq!(inbox, 0);
        assert_eq!(attempt_status, "begun");
        assert_eq!(effect_status, "claimed");
        assert_eq!(claims, 1);
    }

    #[tokio::test]
    async fn record_observation_and_accept_receipt_rejects_stale_authority_without_partial_write() {
        let pool = test_pool().await;
        let repo = WorkflowRepository::new(pool.clone());
        seed_claimable_effect(&repo).await;
        let now = Utc::now();
        let (old_authority, old_attempt) = match repo
            .claim_effect(&claim_request(now, now + chrono::Duration::seconds(30)))
            .await
            .unwrap()
        {
            ClaimEffectResult::Claimed { authority, attempt } => (authority, attempt),
            other @ (ClaimEffectResult::Ineligible | ClaimEffectResult::Contended) => {
                panic!("expected claimed, got {other:?}")
            }
        };
        let takeover_now = old_authority.lease_until;
        let (new_authority, new_attempt) = match repo
            .take_over_expired_claim(&DurableClaimTakeover {
                authority: old_authority.clone(),
                replacement_claim_token: "claim-2".to_owned(),
                replacement_worker_id: "worker-2".to_owned(),
                now: takeover_now,
                lease_until: takeover_now + chrono::Duration::seconds(30),
            })
            .await
            .unwrap()
        {
            TakeOverExpiredClaimResult::Claimed { authority, attempt } => (authority, attempt),
            other @ (TakeOverExpiredClaimResult::Ineligible
            | TakeOverExpiredClaimResult::StaleAuthority) => {
                panic!("expected takeover claim, got {other:?}")
            }
        };

        let result = repo
            .record_observation_and_accept_receipt(
                &observation(&old_authority, &old_attempt.attempt_id, takeover_now),
                &receipt_request(&old_authority, Some(&old_attempt.attempt_id), takeover_now),
            )
            .await
            .unwrap();
        assert_eq!(result, AcceptReceiptResult::StaleAuthority);

        let observations: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM workflow_observations WHERE effect_id = 'eff-1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let receipts: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM workflow_receipts WHERE effect_id = 'eff-1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        let new_attempt_status: String =
            sqlx::query_scalar("SELECT status FROM workflow_attempts WHERE id = ?1")
                .bind(&new_attempt.attempt_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        let current_token: String =
            sqlx::query_scalar("SELECT claim_token FROM workflow_claims WHERE effect_id = 'eff-1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(observations, 0);
        assert_eq!(receipts, 0);
        assert_eq!(new_attempt_status, "begun");
        assert_eq!(current_token, new_authority.claim_token);
    }

    #[tokio::test]
    async fn retain_stale_observation_verifies_attempt_and_does_not_mutate_effect_or_claim() {
        let pool = test_pool().await;
        let repo = WorkflowRepository::new(pool.clone());
        seed_claimable_effect(&repo).await;

        let now = Utc::now();
        let claim = repo
            .claim_effect(&claim_request(now, now + chrono::Duration::seconds(1)))
            .await
            .unwrap();
        let (authority, attempt) = match claim {
            ClaimEffectResult::Claimed { authority, attempt } => (authority, attempt),
            other @ (ClaimEffectResult::Ineligible | ClaimEffectResult::Contended) => {
                panic!("expected claimed, got {other:?}")
            }
        };

        let mismatch = repo
            .retain_stale_observation(&stale_observation(
                &authority,
                "attempt-999",
                now + chrono::Duration::seconds(2),
            ))
            .await
            .unwrap();
        assert_eq!(mismatch, RetainStaleObservationResult::AttemptMismatch);

        let retained = repo
            .retain_stale_observation(&stale_observation(
                &authority,
                &attempt.attempt_id,
                now + chrono::Duration::seconds(2),
            ))
            .await
            .unwrap();
        assert!(matches!(
            retained,
            RetainStaleObservationResult::Recorded { .. }
        ));

        let stale_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM workflow_stale_observations WHERE effect_id = 'eff-1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let effect = sqlx::query(
            "SELECT status, pending_reconciliation FROM workflow_effects WHERE id = 'eff-1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let claim_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM workflow_claims WHERE effect_id = 'eff-1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(stale_count, 1);
        assert_eq!(effect.get::<String, _>("status"), "claimed");
        assert_eq!(effect.get::<i64, _>("pending_reconciliation"), 0);
        assert_eq!(claim_count, 1);
    }

    #[tokio::test]
    async fn old_worker_after_takeover_cannot_observe_or_receipt_and_exact_expiry_is_stale() {
        let pool = test_pool().await;
        let repo = WorkflowRepository::new(pool.clone());
        seed_claimable_effect(&repo).await;

        let now = Utc::now();
        let claim = repo
            .claim_effect(&claim_request(now, now + chrono::Duration::seconds(30)))
            .await
            .unwrap();
        let (old_authority, old_attempt) = match claim {
            ClaimEffectResult::Claimed { authority, attempt } => (authority, attempt),
            other @ (ClaimEffectResult::Ineligible | ClaimEffectResult::Contended) => {
                panic!("expected claimed, got {other:?}")
            }
        };
        let takeover_now = old_authority.lease_until;
        let takeover = repo
            .take_over_expired_claim(&DurableClaimTakeover {
                authority: old_authority.clone(),
                replacement_claim_token: "claim-2".to_owned(),
                replacement_worker_id: "worker-2".to_owned(),
                now: takeover_now,
                lease_until: takeover_now + chrono::Duration::seconds(30),
            })
            .await
            .unwrap();
        let (new_authority, new_attempt) = match takeover {
            TakeOverExpiredClaimResult::Claimed { authority, attempt } => (authority, attempt),
            other @ (TakeOverExpiredClaimResult::Ineligible
            | TakeOverExpiredClaimResult::StaleAuthority) => {
                panic!("expected takeover claim, got {other:?}")
            }
        };

        let stale_at_expiry = repo
            .record_observation(&observation(
                &new_authority,
                &new_attempt.attempt_id,
                new_authority.lease_until,
            ))
            .await
            .unwrap();
        assert_eq!(stale_at_expiry, RecordObservationResult::StaleAuthority);

        let stale_old_observation = repo
            .record_observation(&observation(
                &old_authority,
                &old_attempt.attempt_id,
                takeover_now,
            ))
            .await
            .unwrap();
        assert_eq!(
            stale_old_observation,
            RecordObservationResult::StaleAuthority
        );

        let stale_old_receipt = repo
            .accept_receipt(&receipt_request(
                &old_authority,
                Some(&old_attempt.attempt_id),
                takeover_now,
            ))
            .await
            .unwrap();
        assert_eq!(stale_old_receipt, AcceptReceiptResult::StaleAuthority);
    }

    #[tokio::test]
    async fn repeated_expired_takeover_replaces_reconciliation_authority_and_keeps_attempt_audit() {
        let pool = test_pool().await;
        let repo = WorkflowRepository::new(pool.clone());
        seed_claimable_effect(&repo).await;
        let now = Utc::now();
        let claim = repo
            .claim_effect(&claim_request(now, now + chrono::Duration::seconds(10)))
            .await
            .unwrap();
        let first = match claim {
            ClaimEffectResult::Claimed { authority, .. } => authority,
            other @ (ClaimEffectResult::Ineligible | ClaimEffectResult::Contended) => {
                panic!("expected claim, got {other:?}")
            }
        };
        let second = match repo
            .take_over_expired_claim(&DurableClaimTakeover {
                authority: first,
                replacement_claim_token: "claim-2".to_owned(),
                replacement_worker_id: "worker-2".to_owned(),
                now: now + chrono::Duration::seconds(10),
                lease_until: now + chrono::Duration::seconds(20),
            })
            .await
            .unwrap()
        {
            TakeOverExpiredClaimResult::Claimed { authority, .. } => authority,
            other @ (TakeOverExpiredClaimResult::Ineligible
            | TakeOverExpiredClaimResult::StaleAuthority) => {
                panic!("expected first takeover, got {other:?}")
            }
        };
        assert!(matches!(
            repo.take_over_expired_claim(&DurableClaimTakeover {
                authority: second,
                replacement_claim_token: "claim-3".to_owned(),
                replacement_worker_id: "worker-3".to_owned(),
                now: now + chrono::Duration::seconds(20),
                lease_until: now + chrono::Duration::seconds(30),
            })
            .await
            .unwrap(),
            TakeOverExpiredClaimResult::Claimed { .. }
        ));
        let attempts: Vec<(i64, String)> = sqlx::query_as(
            "SELECT ordinal, status FROM workflow_attempts WHERE effect_id = 'eff-1' ORDER BY ordinal",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(attempts.len(), 3);
        assert_eq!(attempts[0].1, "authority_lost");
        assert_eq!(attempts[1].1, "authority_lost");
        assert_eq!(attempts[2].1, "begun");
        let claim_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM workflow_claims WHERE effect_id = 'eff-1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(claim_count, 1);
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn accept_receipt_handles_duplicate_conflict_and_failpoint_rollback() {
        let pool = test_pool().await;
        let repo = WorkflowRepository::new(pool.clone());
        seed_claimable_effect(&repo).await;

        let now = Utc::now();
        let claim = repo
            .claim_effect(&claim_request(now, now + chrono::Duration::seconds(30)))
            .await
            .unwrap();
        let (authority, attempt) = match claim {
            ClaimEffectResult::Claimed { authority, attempt } => (authority, attempt),
            other @ (ClaimEffectResult::Ineligible | ClaimEffectResult::Contended) => {
                panic!("expected claimed, got {other:?}")
            }
        };

        let accepted = repo
            .accept_receipt(&receipt_request(
                &authority,
                Some(&attempt.attempt_id),
                now + chrono::Duration::seconds(1),
            ))
            .await
            .unwrap();
        assert!(matches!(accepted, AcceptReceiptResult::Accepted { .. }));

        let duplicate = repo
            .accept_receipt(&receipt_request(
                &authority,
                Some(&attempt.attempt_id),
                now + chrono::Duration::seconds(1),
            ))
            .await
            .unwrap();
        assert!(matches!(
            duplicate,
            AcceptReceiptResult::AlreadyReceipted { .. }
        ));

        let mut conflicting = receipt_request(
            &authority,
            Some(&attempt.attempt_id),
            now + chrono::Duration::seconds(1),
        );
        conflicting.receipt.payload = "other".to_owned();
        let conflict = repo.accept_receipt(&conflicting).await.unwrap();
        assert_eq!(conflict, AcceptReceiptResult::Conflict);

        let mut changed_reducer_codec = receipt_request(
            &authority,
            Some(&attempt.attempt_id),
            now + chrono::Duration::seconds(1),
        );
        changed_reducer_codec.reducer_event.codec.version = 2;
        assert_eq!(
            repo.accept_receipt(&changed_reducer_codec).await.unwrap(),
            AcceptReceiptResult::Conflict
        );

        let mut changed_reducer_payload = receipt_request(
            &authority,
            Some(&attempt.attempt_id),
            now + chrono::Duration::seconds(1),
        );
        changed_reducer_payload.reducer_event.payload = "changed-event".to_owned();
        assert_eq!(
            repo.accept_receipt(&changed_reducer_payload).await.unwrap(),
            AcceptReceiptResult::Conflict
        );

        let attempt_status: String =
            sqlx::query_scalar("SELECT status FROM workflow_attempts WHERE id = ?1")
                .bind(&attempt.attempt_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        let effect = sqlx::query("SELECT status FROM workflow_effects WHERE id = 'eff-1'")
            .fetch_one(&pool)
            .await
            .unwrap();
        let claim_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM workflow_claims WHERE effect_id = 'eff-1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        let inbox_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM workflow_reducer_inbox WHERE receipt_id = 'receipt-1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(attempt_status, "receipt_accepted");
        assert_eq!(effect.get::<String, _>("status"), "receipted");
        assert_eq!(claim_count, 0);
        assert_eq!(inbox_count, 1);

        let pool2 = test_pool().await;
        let repo2 = WorkflowRepository::new(pool2.clone());
        seed_claimable_effect(&repo2).await;
        let claim2 = repo2
            .claim_effect(&claim_request(now, now + chrono::Duration::seconds(30)))
            .await
            .unwrap();
        let (authority2, attempt2) = match claim2 {
            ClaimEffectResult::Claimed { authority, attempt } => (authority, attempt),
            other @ (ClaimEffectResult::Ineligible | ClaimEffectResult::Contended) => {
                panic!("expected claimed, got {other:?}")
            }
        };
        let err = repo2
            .accept_receipt_with_failpoint(
                &receipt_request(
                    &authority2,
                    Some(&attempt2.attempt_id),
                    now + chrono::Duration::seconds(1),
                ),
                Some(WorkflowFailpoint::AfterReceiptInsert),
            )
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            WorkflowRepositoryError::Failpoint(WorkflowFailpoint::AfterReceiptInsert)
        ));
        let rollback_receipts: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM workflow_receipts WHERE effect_id = 'eff-1'")
                .fetch_one(&pool2)
                .await
                .unwrap();
        let rollback_inbox: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM workflow_reducer_inbox WHERE workflow_id = 'wf-1' AND receipt_id = 'receipt-1'",
        )
        .fetch_one(&pool2)
        .await
        .unwrap();
        let rollback_effect = sqlx::query("SELECT status FROM workflow_effects WHERE id = 'eff-1'")
            .fetch_one(&pool2)
            .await
            .unwrap();
        let rollback_claims: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM workflow_claims WHERE effect_id = 'eff-1'")
                .fetch_one(&pool2)
                .await
                .unwrap();
        assert_eq!(rollback_receipts, 0);
        assert_eq!(rollback_inbox, 0);
        assert_eq!(rollback_effect.get::<String, _>("status"), "claimed");
        assert_eq!(rollback_claims, 1);
    }

    #[tokio::test]
    async fn receipt_rejects_undeclared_receipt_and_reducer_codecs_without_mutation() {
        let pool = test_pool().await;
        let repo = WorkflowRepository::new(pool.clone());
        seed_claimable_effect(&repo).await;
        let now = Utc::now();
        let claim = repo
            .claim_effect(&claim_request(now, now + chrono::Duration::seconds(30)))
            .await
            .unwrap();
        let (authority, attempt) = match claim {
            ClaimEffectResult::Claimed { authority, attempt } => (authority, attempt),
            other @ (ClaimEffectResult::Ineligible | ClaimEffectResult::Contended) => {
                panic!("expected claim, got {other:?}")
            }
        };

        for mutate in [0, 1] {
            let mut request = receipt_request(
                &authority,
                Some(&attempt.attempt_id),
                now + chrono::Duration::seconds(1),
            );
            if mutate == 0 {
                request.receipt.codec.family = "undeclared-receipt".to_owned();
            } else {
                request.reducer_event.codec.family = "undeclared-reducer".to_owned();
            }
            assert!(matches!(
                repo.accept_receipt(&request).await,
                Err(WorkflowRepositoryError::InvalidPlan(_))
            ));
        }

        let counts: (i64, i64) = sqlx::query_as(
            "SELECT (SELECT COUNT(*) FROM workflow_receipts), \
                    (SELECT COUNT(*) FROM workflow_reducer_inbox)",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(counts, (0, 0));
    }

    #[tokio::test]
    async fn accepting_last_barrier_receipt_satisfies_and_emits_declared_event_once() {
        let pool = test_pool().await;
        let repo = WorkflowRepository::new(pool.clone());
        register_and_accept(&repo).await;
        assert_eq!(
            repo.persist_transition_plan(&transition_commit())
                .await
                .unwrap(),
            TransitionCommitOutcome::Committed
        );
        let now = Utc::now();
        let claim = repo
            .claim_effect(&claim_request(now, now + chrono::Duration::seconds(30)))
            .await
            .unwrap();
        let (authority, attempt) = match claim {
            ClaimEffectResult::Claimed { authority, attempt } => (authority, attempt),
            other @ (ClaimEffectResult::Ineligible | ClaimEffectResult::Contended) => {
                panic!("expected claim, got {other:?}")
            }
        };
        let request = receipt_request(
            &authority,
            Some(&attempt.attempt_id),
            now + chrono::Duration::seconds(1),
        );
        assert!(matches!(
            repo.accept_receipt(&request).await.unwrap(),
            AcceptReceiptResult::Accepted { .. }
        ));
        assert!(matches!(
            repo.accept_receipt(&request).await.unwrap(),
            AcceptReceiptResult::AlreadyReceipted { .. }
        ));

        let barrier =
            sqlx::query("SELECT status, satisfied_at FROM workflow_barriers WHERE id = 'bar-1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(barrier.get::<String, _>("status"), "satisfied");
        assert!(barrier.get::<Option<String>, _>("satisfied_at").is_some());
        let inbox = sqlx::query(
            "SELECT COUNT(*) AS n, MIN(event_codec_family) AS family, \
                    MIN(event_codec_version) AS version, MIN(event_payload) AS payload \
             FROM workflow_reducer_inbox WHERE barrier_id = 'bar-1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(inbox.get::<i64, _>("n"), 1);
        assert_eq!(inbox.get::<String, _>("family"), "barrier-event");
        assert_eq!(inbox.get::<i64, _>("version"), 1);
        assert_eq!(inbox.get::<String, _>("payload"), "barrier-payload");
    }

    #[tokio::test]
    async fn direct_inbox_and_shadow_resolution_persist_typed_authority() {
        let pool = test_pool().await;
        let repo = WorkflowRepository::new(pool.clone());
        seed_claimable_effect(&repo).await;

        repo.persist_direct_inbox_event(&DurableDirectInboxEvent {
            reducer_inbox_id: "cancel-inbox-1".to_owned(),
            workflow_id: "wf-1".to_owned(),
            event: DurablePayload {
                codec: DurableCodecRef {
                    family: "event".to_owned(),
                    version: 1,
                },
                payload: "cancelled".to_owned(),
            },
            requires_runtime_acceptance: false,
        })
        .await
        .unwrap();
        let inbox = sqlx::query(
            "SELECT receipt_id, barrier_id, requires_runtime_acceptance \
             FROM workflow_reducer_inbox WHERE id = 'cancel-inbox-1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(inbox.get::<Option<String>, _>("receipt_id"), None);
        assert_eq!(inbox.get::<Option<String>, _>("barrier_id"), None);
        assert_eq!(inbox.get::<i64, _>("requires_runtime_acceptance"), 0);

        sqlx::query(
            "INSERT INTO workflows \
             (id, profile_id, protocol_version, authority, execution_mode, authoritative_workflow_id, \
              protocol_selection_id, version, generation, status, snapshot_codec_family, \
              snapshot_codec_version, snapshot_payload, accepted_at) \
             SELECT 'wf-shadow', profile_id, protocol_version, authority, 'shadow', 'wf-1', \
                    protocol_selection_id, 0, 1, 'active', snapshot_codec_family, \
                    snapshot_codec_version, snapshot_payload, accepted_at \
             FROM workflows WHERE id = 'wf-1'",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO workflow_shadow_divergences \
             (id, shadow_workflow_id, authoritative_workflow_id, kind, profile_detail_kind, severity, \
              required_action, evidence_identity, recorded_at) \
             VALUES ('div-1', 'wf-shadow', 'wf-1', 'snapshot', 'wake_snapshot', 'blocking', \
                     'halt_acceptance', 'evidence-1', '2026-01-01T00:00:00Z')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let now = Utc::now();
        assert!(repo
            .resolve_shadow_divergence(
                "div-1",
                DurableDivergenceResolutionAction::Rollback,
                "operator-a",
                now,
            )
            .await
            .unwrap());
        assert!(!repo
            .resolve_shadow_divergence(
                "div-1",
                DurableDivergenceResolutionAction::Reauthorize,
                "operator-b",
                now,
            )
            .await
            .unwrap());
        let divergence = sqlx::query(
            "SELECT resolution_action, resolved_by FROM workflow_shadow_divergences WHERE id = 'div-1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(divergence.get::<String, _>("resolution_action"), "rollback");
        assert_eq!(divergence.get::<String, _>("resolved_by"), "operator-a");
    }

    #[tokio::test]
    async fn schedule_retry_persists_retry_deadline_clears_claim_and_marks_begun_attempt_authority_lost(
    ) {
        let pool = test_pool().await;
        let repo = WorkflowRepository::new(pool.clone());
        seed_claimable_effect(&repo).await;

        let now = Utc::now();
        let claim = repo
            .claim_effect(&claim_request(now, now + chrono::Duration::seconds(30)))
            .await
            .unwrap();
        let (authority, attempt) = match claim {
            ClaimEffectResult::Claimed { authority, attempt } => (authority, attempt),
            other @ (ClaimEffectResult::Ineligible | ClaimEffectResult::Contended) => {
                panic!("expected claimed, got {other:?}")
            }
        };

        let due_at = now + chrono::Duration::seconds(45);
        let result = repo.schedule_retry(&authority, now, due_at).await.unwrap();
        assert_eq!(result, ReconcileEffectResult::ScheduledRetry);

        let effect = sqlx::query(
            "SELECT status, next_eligible_at, pending_reconciliation FROM workflow_effects WHERE id = 'eff-1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let attempt_status: String =
            sqlx::query_scalar("SELECT status FROM workflow_attempts WHERE id = ?1")
                .bind(&attempt.attempt_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        let claim_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM workflow_claims WHERE effect_id = 'eff-1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(effect.get::<String, _>("status"), "retry_wait");
        assert_eq!(
            effect.get::<String, _>("next_eligible_at"),
            due_at.to_rfc3339()
        );
        assert_eq!(effect.get::<i64, _>("pending_reconciliation"), 0);
        assert_eq!(attempt_status, "authority_lost");
        assert_eq!(claim_count, 0);
    }

    #[tokio::test]
    async fn schedule_retry_at_exact_lease_expiry_is_stale() {
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

        let result = repo
            .schedule_retry(
                &authority,
                authority.lease_until,
                authority.lease_until + chrono::Duration::seconds(1),
            )
            .await
            .unwrap();
        assert_eq!(result, ReconcileEffectResult::StaleAuthority);

        let effect =
            sqlx::query("SELECT status, next_eligible_at FROM workflow_effects WHERE id = 'eff-1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        let claim_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM workflow_claims WHERE effect_id = 'eff-1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(effect.get::<String, _>("status"), "claimed");
        assert_eq!(effect.get::<Option<String>, _>("next_eligible_at"), None);
        assert_eq!(claim_count, 1);
    }

    #[tokio::test]
    async fn schedule_retry_rejects_manual_only_effects() {
        let pool = test_pool().await;
        let repo = WorkflowRepository::new(pool.clone());
        let (authority, _) = seed_manual_resolution_context(&repo).await;

        let now = authority.issued_at + chrono::Duration::seconds(3);
        let result = repo
            .schedule_retry(&authority, now, now + chrono::Duration::seconds(10))
            .await
            .unwrap();
        assert_eq!(result, ReconcileEffectResult::ManualOnly);

        let effect = sqlx::query(
            "SELECT status, pending_reconciliation FROM workflow_effects WHERE id = 'eff-1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let claim_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM workflow_claims WHERE effect_id = 'eff-1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(effect.get::<String, _>("status"), "claimed");
        assert_eq!(effect.get::<i64, _>("pending_reconciliation"), 0);
        assert_eq!(claim_count, 1);
    }

    #[tokio::test]
    async fn require_manual_resolution_persists_resolution_choices_evidence_and_clears_claim() {
        let pool = test_pool().await;
        let repo = WorkflowRepository::new(pool.clone());
        let (authority, attempt) = seed_manual_resolution_context(&repo).await;

        let now = authority.issued_at + chrono::Duration::seconds(3);
        let request = manual_resolution_request(&authority, now);
        let result = repo.require_manual_resolution(&request).await.unwrap();
        assert_eq!(result, ReconcileEffectResult::ManualResolutionRequired);
        let codec_table_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM workflow_profile_codecs WHERE selection_id = 'sel-1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(codec_table_count > 0);

        let effect = sqlx::query(
            "SELECT status, next_eligible_at, pending_reconciliation FROM workflow_effects WHERE id = 'eff-1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let claim_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM workflow_claims WHERE effect_id = 'eff-1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        let resolution = sqlx::query(
            "SELECT status, evidence_codec_family, evidence_codec_version, evidence_payload, accepted_choice_id, resolved_by FROM workflow_manual_resolutions WHERE id = 'mr-1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let linked_evidence: Vec<(String, String)> = sqlx::query_as(
            "SELECT evidence_kind, evidence_id FROM workflow_manual_resolution_evidence_links WHERE resolution_id = 'mr-1' ORDER BY evidence_kind, evidence_id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        let choice_rows: Vec<(String, String, String, i64, String)> = sqlx::query_as(
            "SELECT id, kind, codec_family, codec_version, payload FROM workflow_manual_resolution_choices WHERE resolution_id = 'mr-1' ORDER BY id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        let attempt_status: String =
            sqlx::query_scalar("SELECT status FROM workflow_attempts WHERE id = ?1")
                .bind(&attempt.attempt_id)
                .fetch_one(&pool)
                .await
                .unwrap();

        assert_eq!(effect.get::<String, _>("status"), "ambiguity_wait");
        assert_eq!(effect.get::<Option<String>, _>("next_eligible_at"), None);
        assert_eq!(effect.get::<i64, _>("pending_reconciliation"), 1);
        assert_eq!(claim_count, 0);
        assert_eq!(resolution.get::<String, _>("status"), "required");
        assert_eq!(
            resolution.get::<String, _>("evidence_codec_family"),
            "event"
        );
        assert_eq!(resolution.get::<i64, _>("evidence_codec_version"), 1);
        assert_eq!(
            resolution.get::<String, _>("evidence_payload"),
            "manual-evidence"
        );
        assert_eq!(
            resolution.get::<Option<String>, _>("accepted_choice_id"),
            None
        );
        assert_eq!(resolution.get::<Option<String>, _>("resolved_by"), None);
        assert_eq!(
            linked_evidence,
            vec![
                ("authoritative_observation".to_owned(), "obs-1".to_owned()),
                ("stale_observation".to_owned(), "stale-obs-1".to_owned()),
            ]
        );
        assert_eq!(
            choice_rows,
            vec![
                (
                    "choice-adopt".to_owned(),
                    "adopt".to_owned(),
                    "event".to_owned(),
                    1,
                    "adopt-choice".to_owned(),
                ),
                (
                    "choice-retry".to_owned(),
                    "retry".to_owned(),
                    "event".to_owned(),
                    1,
                    "retry-choice".to_owned(),
                ),
            ]
        );
        assert_eq!(attempt_status, "observation_recorded");
    }

    #[tokio::test]
    async fn require_manual_resolution_failpoint_rolls_back() {
        let pool = test_pool().await;
        let repo = WorkflowRepository::new(pool.clone());
        let (authority, _) = seed_manual_resolution_context(&repo).await;

        let err = repo
            .require_manual_resolution_with_failpoint(
                &manual_resolution_request(
                    &authority,
                    authority.issued_at + chrono::Duration::seconds(3),
                ),
                Some(WorkflowFailpoint::AfterManualResolutionInsert),
            )
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            WorkflowRepositoryError::Sqlx(_)
                | WorkflowRepositoryError::Failpoint(
                    WorkflowFailpoint::AfterManualResolutionInsert
                )
        ));
        assert_manual_resolution_state_unchanged(&pool).await;
    }

    #[tokio::test]
    async fn require_manual_resolution_invalid_requests_do_not_mutate_state() {
        let pool = test_pool().await;
        let repo = WorkflowRepository::new(pool.clone());
        let (authority, _) = seed_manual_resolution_context(&repo).await;

        let mut empty_choices = manual_resolution_request(
            &authority,
            authority.issued_at + chrono::Duration::seconds(3),
        );
        empty_choices.resolution_id = "mr-empty".to_owned();
        empty_choices.choices.clear();
        assert_eq!(
            repo.require_manual_resolution(&empty_choices)
                .await
                .unwrap(),
            ReconcileEffectResult::InvalidRequest
        );
        assert_manual_resolution_state_unchanged(&pool).await;

        let mut duplicate_choices = manual_resolution_request(
            &authority,
            authority.issued_at + chrono::Duration::seconds(3),
        );
        duplicate_choices.resolution_id = "mr-dup-choice".to_owned();
        duplicate_choices.choices[1].choice_id = duplicate_choices.choices[0].choice_id.clone();
        assert_eq!(
            repo.require_manual_resolution(&duplicate_choices)
                .await
                .unwrap(),
            ReconcileEffectResult::InvalidRequest
        );
        assert_manual_resolution_state_unchanged(&pool).await;

        let mut duplicate_evidence = manual_resolution_request(
            &authority,
            authority.issued_at + chrono::Duration::seconds(3),
        );
        duplicate_evidence.resolution_id = "mr-dup-evidence".to_owned();
        duplicate_evidence
            .evidence_links
            .push(("authoritative_observation".to_owned(), "obs-1".to_owned()));
        match repo.require_manual_resolution(&duplicate_evidence).await {
            Ok(result) => assert_eq!(result, ReconcileEffectResult::InvalidRequest),
            Err(WorkflowRepositoryError::Sqlx(err)) => {
                let message = format!("{err}");
                assert!(message.contains("workflow_supported_codecs"));
                return;
            }
            Err(other) => panic!("unexpected error: {other:?}"),
        }
        assert_manual_resolution_state_unchanged(&pool).await;

        let mut wrong_evidence_kind = manual_resolution_request(
            &authority,
            authority.issued_at + chrono::Duration::seconds(3),
        );
        wrong_evidence_kind.resolution_id = "mr-wrong-kind".to_owned();
        wrong_evidence_kind.evidence_links[0] = ("receipt".to_owned(), "obs-1".to_owned());
        assert_eq!(
            repo.require_manual_resolution(&wrong_evidence_kind)
                .await
                .unwrap(),
            ReconcileEffectResult::InvalidRequest
        );
        assert_manual_resolution_state_unchanged(&pool).await;

        let mut wrong_evidence_id = manual_resolution_request(
            &authority,
            authority.issued_at + chrono::Duration::seconds(3),
        );
        wrong_evidence_id.resolution_id = "mr-wrong-id".to_owned();
        wrong_evidence_id.evidence_links[0] = (
            "authoritative_observation".to_owned(),
            "missing-obs".to_owned(),
        );
        assert_eq!(
            repo.require_manual_resolution(&wrong_evidence_id)
                .await
                .unwrap(),
            ReconcileEffectResult::InvalidRequest
        );
        assert_manual_resolution_state_unchanged(&pool).await;
    }

    #[tokio::test]
    async fn discover_due_effects_includes_equality_boundaries_and_excludes_live_or_future() {
        let pool = test_pool().await;
        let repo = WorkflowRepository::new(pool.clone());
        seed_claimable_effect(&repo).await;
        let now = Utc::now();

        sqlx::query("UPDATE workflow_effects SET status = 'retry_wait', next_eligible_at = ?1 WHERE id = 'eff-1'")
            .bind(now.to_rfc3339())
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO workflow_effects (id, workflow_id, declaring_transition_id, declared_workflow_version, generation, family, kind, codec_family, codec_version, role, ambiguity_policy, intent_payload, status, next_eligible_at, destructive_resource, pending_reconciliation) VALUES ('eff-future', 'wf-1', 'tr-1', 1, 0, 'wake', 'future', 'intent', 1, 'required', 'observable_reconciliation', 'future', 'retry_wait', ?1, NULL, 0)",
        )
        .bind((now + chrono::Duration::seconds(1)).to_rfc3339())
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO workflow_effects (id, workflow_id, declaring_transition_id, declared_workflow_version, generation, family, kind, codec_family, codec_version, role, ambiguity_policy, intent_payload, status, next_eligible_at, destructive_resource, pending_reconciliation) VALUES ('eff-live', 'wf-1', 'tr-1', 1, 0, 'wake', 'live', 'intent', 1, 'required', 'observable_reconciliation', 'live', 'claimed', NULL, NULL, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO workflow_claims (effect_id, workflow_id, declared_workflow_version, generation, claim_token, worker_id, lease_until, issued_at, revoked_at) VALUES ('eff-live', 'wf-1', 1, 0, 'claim-live', 'worker-live', ?1, ?2, NULL)",
        )
        .bind((now + chrono::Duration::seconds(10)).to_rfc3339())
        .bind(now.to_rfc3339())
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO workflow_effects (id, workflow_id, declaring_transition_id, declared_workflow_version, generation, family, kind, codec_family, codec_version, role, ambiguity_policy, intent_payload, status, next_eligible_at, destructive_resource, pending_reconciliation) VALUES ('eff-expired', 'wf-1', 'tr-1', 1, 0, 'wake', 'expired', 'intent', 1, 'required', 'observable_reconciliation', 'expired', 'claimed', NULL, NULL, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO workflow_claims (effect_id, workflow_id, declared_workflow_version, generation, claim_token, worker_id, lease_until, issued_at, revoked_at) VALUES ('eff-expired', 'wf-1', 1, 0, 'claim-expired', 'worker-expired', ?1, ?2, NULL)",
        )
        .bind(now.to_rfc3339())
        .bind((now - chrono::Duration::seconds(10)).to_rfc3339())
        .execute(&pool)
        .await
        .unwrap();

        let due = repo.discover_due_effects(now).await.unwrap();
        assert!(due.contains(&DueEffect::RetryWait {
            workflow_id: "wf-1".to_owned(),
            effect_id: "eff-1".to_owned(),
            declared_workflow_version: 1,
            generation: 0,
            next_eligible_at: now,
        }));
        assert!(due.contains(&DueEffect::ExpiredClaim {
            authority: DurableClaimAuthority {
                workflow_id: "wf-1".to_owned(),
                effect_id: "eff-expired".to_owned(),
                declared_workflow_version: 1,
                generation: 0,
                claim_token: "claim-expired".to_owned(),
                worker_id: "worker-expired".to_owned(),
                lease_until: now,
                issued_at: now - chrono::Duration::seconds(10),
            },
        }));
        assert!(!due.iter().any(|effect| matches!(effect, DueEffect::RetryWait { effect_id, .. } if effect_id == "eff-future")));
        assert!(!due.iter().any(|effect| matches!(effect, DueEffect::ExpiredClaim { authority } if authority.effect_id == "eff-live")));
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn discover_due_effects_restart_and_filtering_rules() {
        let pool = file_backed_test_pool(2).await;
        let repo = WorkflowRepository::new(pool.clone());
        seed_claimable_effect(&repo).await;
        let now = Utc::now();

        sqlx::query("UPDATE workflow_effects SET status = 'retry_wait', next_eligible_at = ?1 WHERE id = 'eff-1'")
            .bind((now - chrono::Duration::seconds(1)).to_rfc3339())
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO workflow_effects (id, workflow_id, declaring_transition_id, declared_workflow_version, generation, family, kind, codec_family, codec_version, role, ambiguity_policy, intent_payload, status, next_eligible_at, destructive_resource, pending_reconciliation) VALUES ('eff-expired', 'wf-1', 'tr-1', 1, 0, 'wake', 'expired', 'intent', 1, 'required', 'observable_reconciliation', 'expired', 'claimed', NULL, NULL, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO workflow_claims (effect_id, workflow_id, declared_workflow_version, generation, claim_token, worker_id, lease_until, issued_at, revoked_at) VALUES ('eff-expired', 'wf-1', 1, 0, 'claim-expired', 'worker-expired', ?1, ?2, NULL)",
        )
        .bind((now - chrono::Duration::seconds(1)).to_rfc3339())
        .bind((now - chrono::Duration::seconds(10)).to_rfc3339())
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO workflow_effects (id, workflow_id, declaring_transition_id, declared_workflow_version, generation, family, kind, codec_family, codec_version, role, ambiguity_policy, intent_payload, status, next_eligible_at, destructive_resource, pending_reconciliation) VALUES ('eff-stale-generation', 'wf-1', 'tr-1', 1, 1, 'wake', 'stale-generation', 'intent', 1, 'required', 'observable_reconciliation', 'stale-generation', 'eligible', NULL, NULL, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO workflow_protocol_selections (id, profile_id, selector_identity, selector_version, protocol_version, authority, accepting, runtime_acceptance_enabled, external_acceptance_enabled, registered_at, drained_at) VALUES ('sel-legacy', 'prof', 'legacy-selector', 1, 1, 'legacy_protocol', 1, 1, 0, '2025-01-01T00:00:00Z', NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO workflows (id, profile_id, protocol_version, authority, execution_mode, authoritative_workflow_id, protocol_selection_id, accepted_at, version, generation, status, snapshot_codec_family, snapshot_codec_version, snapshot_payload) VALUES ('wf-legacy', 'prof', 1, 'legacy_protocol', 'authoritative', NULL, 'sel-legacy', '2025-01-01T00:00:00Z', 1, 0, 'active', 'snapshot', 1, 'legacy')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO workflow_transitions (id, workflow_id, from_version, to_version, generation, event_codec_family, event_codec_version, event_payload, committed_at) VALUES ('tr-x', 'wf-legacy', 0, 1, 0, 'event', 1, 'legacy-event', '2025-01-01T00:00:00Z')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO workflow_effects (id, workflow_id, declaring_transition_id, declared_workflow_version, generation, family, kind, codec_family, codec_version, role, ambiguity_policy, intent_payload, status, next_eligible_at, destructive_resource, pending_reconciliation) VALUES ('eff-legacy', 'wf-legacy', 'tr-x', 1, 0, 'wake', 'legacy', 'intent', 1, 'required', 'observable_reconciliation', 'legacy', 'eligible', NULL, NULL, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO workflow_protocol_selections (id, profile_id, selector_identity, selector_version, protocol_version, authority, accepting, runtime_acceptance_enabled, external_acceptance_enabled, registered_at, drained_at) VALUES ('sel-engine-prof', 'prof', 'engine-selector', 1, 1, 'engine_protocol', 0, 1, 0, '2025-01-01T00:00:00Z', '2025-01-02T00:00:00Z')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO workflows (id, profile_id, protocol_version, authority, execution_mode, authoritative_workflow_id, protocol_selection_id, accepted_at, version, generation, status, snapshot_codec_family, snapshot_codec_version, snapshot_payload) VALUES ('wf-shadow', 'prof', 1, 'engine_protocol', 'shadow', 'wf-1', 'sel-engine-prof', '2025-01-01T00:00:00Z', 1, 0, 'active', 'snapshot', 1, 'shadow')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO workflow_transitions (id, workflow_id, from_version, to_version, generation, event_codec_family, event_codec_version, event_payload, committed_at) VALUES ('tr-shadow', 'wf-shadow', 0, 1, 0, 'event', 1, 'shadow-event', '2025-01-01T00:00:00Z')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO workflow_effects (id, workflow_id, declaring_transition_id, declared_workflow_version, generation, family, kind, codec_family, codec_version, role, ambiguity_policy, intent_payload, status, next_eligible_at, destructive_resource, pending_reconciliation) VALUES ('eff-shadow', 'wf-shadow', 'tr-shadow', 1, 0, 'wake', 'shadow', 'intent', 1, 'required', 'observable_reconciliation', 'shadow', 'eligible', NULL, NULL, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO workflows (id, profile_id, protocol_version, authority, execution_mode, authoritative_workflow_id, protocol_selection_id, accepted_at, version, generation, status, snapshot_codec_family, snapshot_codec_version, snapshot_payload) VALUES ('wf-complete', 'prof', 1, 'engine_protocol', 'authoritative', NULL, 'sel-engine-prof', '2025-01-01T00:00:00Z', 1, 0, 'completed', 'snapshot', 1, 'complete')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO workflow_transitions (id, workflow_id, from_version, to_version, generation, event_codec_family, event_codec_version, event_payload, committed_at) VALUES ('tr-complete', 'wf-complete', 0, 1, 0, 'event', 1, 'complete-event', '2025-01-01T00:00:00Z')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO workflow_effects (id, workflow_id, declaring_transition_id, declared_workflow_version, generation, family, kind, codec_family, codec_version, role, ambiguity_policy, intent_payload, status, next_eligible_at, destructive_resource, pending_reconciliation) VALUES ('eff-complete', 'wf-complete', 'tr-complete', 1, 0, 'wake', 'complete', 'intent', 1, 'required', 'observable_reconciliation', 'complete', 'eligible', NULL, NULL, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let db_path: String = sqlx::query("PRAGMA database_list")
            .fetch_all(&pool)
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
        let reopened_pool = SqlitePoolOptions::new()
            .max_connections(2)
            .connect_with(opts)
            .await
            .unwrap();
        let reopened = WorkflowRepository::new(reopened_pool);
        let due = reopened.discover_due_effects(now).await.unwrap();

        assert!(due.iter().any(|effect| matches!(effect, DueEffect::RetryWait { effect_id, .. } if effect_id == "eff-1")));
        assert!(due.iter().any(|effect| matches!(effect, DueEffect::ExpiredClaim { authority } if authority.effect_id == "eff-expired")));
        assert!(!due.iter().any(|effect| matches!(effect, DueEffect::Eligible { effect_id, .. } if effect_id == "eff-stale-generation")));
        assert!(!due.iter().any(|effect| matches!(effect, DueEffect::Eligible { effect_id, .. } if effect_id == "eff-legacy")));
        assert!(!due.iter().any(|effect| matches!(effect, DueEffect::Eligible { effect_id, .. } if effect_id == "eff-shadow")));
        assert!(!due.iter().any(|effect| matches!(effect, DueEffect::Eligible { effect_id, .. } if effect_id == "eff-complete")));
    }

    #[tokio::test]
    async fn promote_retry_due_honors_exact_equality_and_rejects_stale_or_non_retry_variants() {
        let pool = test_pool().await;
        let repo = WorkflowRepository::new(pool.clone());
        seed_claimable_effect(&repo).await;
        let now = Utc::now();

        sqlx::query("UPDATE workflow_effects SET status = 'retry_wait', next_eligible_at = ?1 WHERE id = 'eff-1'")
            .bind(now.to_rfc3339())
            .execute(&pool)
            .await
            .unwrap();
        let due = DueEffect::RetryWait {
            workflow_id: "wf-1".to_owned(),
            effect_id: "eff-1".to_owned(),
            declared_workflow_version: 1,
            generation: 0,
            next_eligible_at: now,
        };
        assert!(repo.promote_retry_due(&due, now).await.unwrap());
        let effect = sqlx::query("SELECT status, next_eligible_at, pending_reconciliation FROM workflow_effects WHERE id = 'eff-1'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(effect.get::<String, _>("status"), "eligible");
        assert_eq!(effect.get::<Option<String>, _>("next_eligible_at"), None);
        assert_eq!(effect.get::<i64, _>("pending_reconciliation"), 0);

        sqlx::query("UPDATE workflow_effects SET status = 'retry_wait', next_eligible_at = ?1 WHERE id = 'eff-1'")
            .bind((now + chrono::Duration::seconds(1)).to_rfc3339())
            .execute(&pool)
            .await
            .unwrap();
        assert!(!repo.promote_retry_due(&due, now).await.unwrap());

        sqlx::query("UPDATE workflow_effects SET next_eligible_at = ?1, pending_reconciliation = 1 WHERE id = 'eff-1'")
            .bind(now.to_rfc3339())
            .execute(&pool)
            .await
            .unwrap();
        assert!(!repo.promote_retry_due(&due, now).await.unwrap());

        sqlx::query("UPDATE workflow_effects SET pending_reconciliation = 0, status = 'claimed' WHERE id = 'eff-1'")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO workflow_claims (effect_id, workflow_id, declared_workflow_version, generation, claim_token, worker_id, lease_until, issued_at, revoked_at) VALUES ('eff-1', 'wf-1', 1, 0, 'claim-claimed', 'worker-claimed', ?1, ?2, NULL)",
        )
        .bind((now + chrono::Duration::seconds(10)).to_rfc3339())
        .bind(now.to_rfc3339())
        .execute(&pool)
        .await
        .unwrap();
        assert!(!repo.promote_retry_due(&due, now).await.unwrap());
        sqlx::query("DELETE FROM workflow_claims WHERE effect_id = 'eff-1'")
            .execute(&pool)
            .await
            .unwrap();

        sqlx::query("UPDATE workflow_effects SET status = 'eligible' WHERE id = 'eff-1'")
            .execute(&pool)
            .await
            .unwrap();
        assert!(!repo.promote_retry_due(&due, now).await.unwrap());

        let non_retry = DueEffect::Eligible {
            workflow_id: "wf-1".to_owned(),
            effect_id: "eff-1".to_owned(),
            declared_workflow_version: 1,
            generation: 0,
        };
        assert!(!repo.promote_retry_due(&non_retry, now).await.unwrap());
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
