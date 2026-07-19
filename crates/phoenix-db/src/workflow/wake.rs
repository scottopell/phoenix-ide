#![allow(clippy::too_many_arguments, clippy::type_complexity)]

use super::{
    parse_effect_status, to_i64, to_u64, wake_profile, AcceptReceiptInput, BeginAttemptInput,
    BeginAttemptResult, ClaimOutcome, CommitOutcome, CommitTransitionPlanCas,
    CreateWorkflowWithExternalAcceptance, DbError, DbResult, DeliveryResolutionDecision,
    DeliveryResolutionPlan, ExpireLeaseInput, LocalCodec, LocalDeliveryRecord, LocalEffectDecl,
    LocalReceiptRecord, RecordObservationInput, RenewLeaseInput, WorkflowRepository,
    WorkflowSequenceName, WorkflowTx,
};
use chrono::{DateTime, Utc};
use phoenix_core::domain::{
    db_schema::{Message, MessageContent, UserContent},
    sm_state::ConvState,
};
use phoenix_workflow::{
    wake_profile::{
        self as wake_types, BashTerminalEvidence, ObserveHandleIntent, TmuxTerminalEvidence,
        WakeCancellationReason, WakeForgottenReason, WakeRegistrationEvent, WakeRegistrationIntent,
        WakeRegistrationReceipt, WakeRegistrationSnapshot, WakeResourceIdentity,
        WakeTerminalEvidence, WakeTerminalPayload, REGISTRATION_EFFECT_ID,
    },
    AttemptId, AuthorityOutcome, DeliveryId, EffectId, EffectRole, EffectStatus,
    ErasedAcceptanceProfile, Generation, ProcessIncarnation, ProfileRef, ReceiptId, ReceiptOrigin,
    RuntimeAcceptanceStatus, Timestamp, TransitionId, Version, WorkflowId, WorkflowStatus,
};
use serde::Serialize;
use sqlx::Row;
#[cfg(test)]
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Mutex, OnceLock,
};

#[cfg(test)]
type FailpointKey = (u64, u64);

#[cfg(test)]
fn next_failpoint_namespace() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

#[cfg(test)]
fn fail_after_canonical_transition_set() -> &'static Mutex<std::collections::BTreeSet<FailpointKey>>
{
    static SET: OnceLock<Mutex<std::collections::BTreeSet<FailpointKey>>> = OnceLock::new();
    SET.get_or_init(|| Mutex::new(std::collections::BTreeSet::new()))
}
#[cfg(test)]
fn fail_after_canonical_receipt_set() -> &'static Mutex<std::collections::BTreeSet<FailpointKey>> {
    static SET: OnceLock<Mutex<std::collections::BTreeSet<FailpointKey>>> = OnceLock::new();
    SET.get_or_init(|| Mutex::new(std::collections::BTreeSet::new()))
}
#[cfg(test)]
fn fail_after_transfer_binding_update_set(
) -> &'static Mutex<std::collections::BTreeSet<FailpointKey>> {
    static SET: OnceLock<Mutex<std::collections::BTreeSet<FailpointKey>>> = OnceLock::new();
    SET.get_or_init(|| Mutex::new(std::collections::BTreeSet::new()))
}

#[cfg(test)]
fn maybe_fail_after_canonical_transition(namespace: u64, workflow_id: WorkflowId) -> DbResult<()> {
    if fail_after_canonical_transition_set()
        .lock()
        .unwrap()
        .remove(&(namespace, workflow_id.0))
    {
        return Err(DbError::Serialization(
            "test failpoint after canonical transition".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
fn maybe_fail_after_canonical_receipt(namespace: u64, workflow_id: WorkflowId) -> DbResult<()> {
    if fail_after_canonical_receipt_set()
        .lock()
        .unwrap()
        .remove(&(namespace, workflow_id.0))
    {
        return Err(DbError::Serialization(
            "test failpoint after canonical receipt".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
fn maybe_fail_after_transfer_binding_update(
    namespace: u64,
    workflow_id: WorkflowId,
) -> DbResult<()> {
    if fail_after_transfer_binding_update_set()
        .lock()
        .unwrap()
        .remove(&(namespace, workflow_id.0))
    {
        return Err(DbError::Serialization(
            "test failpoint after transfer binding update".to_string(),
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeBindingRecord {
    pub workflow_id: WorkflowId,
    pub conversation_id: String,
    pub contract_id: String,
    pub profile: ProfileRef,
    pub registration_scope: wake_types::WorkScopeIdentity,
    pub resource: WakeResourceIdentity,
    pub registering_tool_use_id: String,
    pub expires_at: Timestamp,
    pub prepared_fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeRegistrationOutcome {
    Registered {
        workflow_id: WorkflowId,
        receipt: WakeRegistrationReceipt,
    },
    Replayed {
        workflow_id: WorkflowId,
        receipt: WakeRegistrationReceipt,
    },
    Conflict,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakePendingDelivery {
    pub workflow_id: WorkflowId,
    pub conversation_id: String,
    pub receipt: WakeTerminalReceiptProjection,
    pub canonical_delivery: LocalDeliveryRecord,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterializePendingDeliveryMessageInput {
    pub workflow_id: WorkflowId,
    pub delivery_id: DeliveryId,
    pub conversation_id: String,
    pub rendered_content: String,
    pub display_data: Option<serde_json::Value>,
    pub auto_resume: bool,
    pub created_at: Timestamp,
    pub sequence_id: Option<i64>,
}

#[derive(Debug, Clone)]
pub enum MaterializePendingDeliveryMessageOutcome {
    Materialized(WakeDeliveryMessageLink),
    AlreadyMaterialized(WakeDeliveryMessageLink),
    WrongOwnerOrIneligible,
}

#[derive(Debug, Clone)]
pub struct WakeDeliveryLinkedMessage {
    pub message: Message,
}

#[derive(Debug, Clone)]
pub struct WakeDeliveryMessageLink {
    pub workflow_id: WorkflowId,
    pub delivery_id: DeliveryId,
    pub conversation_id: String,
    pub message_id: String,
    pub registering_tool_use_id: String,
    pub terminal_kind: String,
    pub auto_resume: bool,
    pub created_at: Timestamp,
    pub linked_message: WakeDeliveryLinkedMessage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeTerminalReceiptProjection {
    pub workflow_id: WorkflowId,
    pub receipt_id: ReceiptId,
    pub delivery_id: DeliveryId,
    pub conversation_id: String,
    pub contract_id: String,
    pub terminal: WakeTerminalPayload,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeObservationOutcome {
    Started {
        canonical: BeginAttemptResult,
    },
    Busy {
        lease_until: phoenix_workflow::LeaseExpiry,
    },
    Ineligible,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeObservationLease {
    pub workflow_id: WorkflowId,
    pub process_incarnation: ProcessIncarnation,
    pub now: Timestamp,
    pub lease_until: phoenix_workflow::LeaseExpiry,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeLeaseRenewalOutcome {
    Renewed,
    Stale,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeActiveUnresolvedRow {
    pub workflow_id: WorkflowId,
    pub conversation_id: String,
    pub contract_id: String,
    pub expires_at: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeObservationCandidateReason {
    NoLiveAttempt,
    ExpiredLease,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeObservationCandidateRow {
    pub workflow_id: WorkflowId,
    pub conversation_id: String,
    pub contract_id: String,
    pub reason: WakeObservationCandidateReason,
    pub expires_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeExpiredUnresolvedRow {
    pub workflow_id: WorkflowId,
    pub conversation_id: String,
    pub contract_id: String,
    pub expires_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakePendingGlobalRow {
    pub workflow_id: WorkflowId,
    pub conversation_id: String,
    pub contract_id: String,
    pub delivery_id: DeliveryId,
    pub receipt_id: ReceiptId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WakePendingGlobalCursor {
    pub workflow_id: WorkflowId,
    pub delivery_id: DeliveryId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeTerminalEvidenceInput {
    pub workflow_id: WorkflowId,
    pub authority: super::LocalAttemptAuthority,
    pub observation_time: Timestamp,
    pub evidence: WakeTerminalEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeExpireIfUnresolvedInput {
    pub workflow_id: WorkflowId,
    pub now: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeExpireIfUnresolvedOutcome {
    Expired {
        receipt: LocalReceiptRecord,
        delivery: WakePendingDelivery,
    },
    Replayed {
        receipt: LocalReceiptRecord,
        delivery: WakePendingDelivery,
    },
    NotDue,
    Stale,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeForgetIfUnresolvedInput {
    pub workflow_id: WorkflowId,
    pub now: Timestamp,
    pub reason: WakeForgottenReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeForgetIfUnresolvedOutcome {
    Forgotten {
        receipt: LocalReceiptRecord,
        delivery: WakePendingDelivery,
    },
    Replayed {
        receipt: LocalReceiptRecord,
        delivery: WakePendingDelivery,
    },
    Stale,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeTerminalEvidenceOutcome {
    Recorded {
        receipt: LocalReceiptRecord,
        delivery: WakePendingDelivery,
    },
    Replayed {
        receipt: LocalReceiptRecord,
        delivery: WakePendingDelivery,
    },
    StaleAttempt,
    WrongResource,
    EvidenceAfterObservation,
    EvidenceAfterExpiry,
}

pub struct WakeCancellationInput {
    pub workflow_id: WorkflowId,
    pub expected_version: Version,
    pub expected_generation: Generation,
    pub receipt_id: ReceiptId,
    pub delivery_id: DeliveryId,
    pub timestamp: Timestamp,
    pub reason: WakeCancellationReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeCancellationOutcome {
    Cancelled {
        receipt: LocalReceiptRecord,
        delivery: WakePendingDelivery,
    },
    Replayed {
        receipt: LocalReceiptRecord,
        delivery: WakePendingDelivery,
    },
    Stale,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeCancelIfUnresolvedInput {
    pub workflow_id: WorkflowId,
    pub expected_conversation_id: Option<String>,
    pub expected_contract_id: Option<String>,
    pub timestamp: Timestamp,
    pub reason: WakeCancellationReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeResolveDecision {
    Accept,
    Suppress,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeResolvePendingInput {
    pub workflow_id: WorkflowId,
    pub expected_version: Version,
    pub delivery_ids: Vec<DeliveryId>,
    pub decision: WakeResolveDecision,
    pub transition_id: TransitionId,
    pub timestamp: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeResolvePendingOutcome {
    Resolved,
    VersionConflict,
    SetMismatch,
    AlreadyResolved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeResolveMaterializedDecision {
    Accept,
    Suppress,
}

#[derive(Debug, Clone)]
pub struct WakeMaterializedPendingDelivery {
    pub pending: WakePendingDelivery,
    pub link: WakeDeliveryMessageLink,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeResolveMaterializedPendingOutcome {
    Resolved {
        delivery_ids: Vec<DeliveryId>,
        auto_resume: bool,
    },
    AlreadyResolved,
    NothingPending,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeResolveMaterializedPendingError {
    NotFullyMaterialized { delivery_ids: Vec<DeliveryId> },
}

#[derive(Debug, Clone)]
pub struct WakeAdoptedMaterializedPending {
    pub links: Vec<WakeDeliveryMessageLink>,
    pub auto_resume: bool,
}

#[derive(Debug, Clone)]
pub enum WakeAdoptMaterializedPendingOutcome {
    Adopted(WakeAdoptedMaterializedPending),
    Busy(Box<ConvState>),
    NothingPending,
    NotFullyMaterialized { delivery_ids: Vec<DeliveryId> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeTransferInput {
    pub workflow_id: WorkflowId,
    pub from_conversation_id: String,
    pub to_conversation_id: String,
    pub expected_version: Version,
    pub exact_pending_delivery_ids: Vec<DeliveryId>,
    pub transition_id: TransitionId,
    pub timestamp: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeTransferOutcome {
    Transferred,
    VersionConflict,
    OwnerMismatch,
    SetMismatch,
}

#[derive(Debug, Clone)]
pub struct WakeRepository {
    workflow_repo: WorkflowRepository,
    #[cfg(test)]
    failpoint_namespace: u64,
}

impl WakeRepository {
    #[must_use]
    pub fn new(pool: sqlx::SqlitePool) -> Self {
        Self {
            workflow_repo: WorkflowRepository::new(pool),
            #[cfg(test)]
            failpoint_namespace: next_failpoint_namespace(),
        }
    }

    #[cfg(test)]
    fn fail_after_canonical_transition_once(&self, workflow_id: WorkflowId) {
        fail_after_canonical_transition_set()
            .lock()
            .unwrap()
            .insert((self.failpoint_namespace, workflow_id.0));
    }

    #[cfg(test)]
    fn fail_after_canonical_receipt_once(&self, workflow_id: WorkflowId) {
        fail_after_canonical_receipt_set()
            .lock()
            .unwrap()
            .insert((self.failpoint_namespace, workflow_id.0));
    }

    #[cfg(test)]
    fn fail_after_resolve_transition_once(&self, workflow_id: WorkflowId) {
        self.fail_after_canonical_transition_once(workflow_id);
    }

    #[cfg(test)]
    fn fail_after_transfer_binding_update_once(&self, workflow_id: WorkflowId) {
        fail_after_transfer_binding_update_set()
            .lock()
            .unwrap()
            .insert((self.failpoint_namespace, workflow_id.0));
    }

    pub async fn register(
        &self,
        input: &WakeRegistrationIntent,
        prepared_fingerprint: &str,
        now: Timestamp,
    ) -> DbResult<WakeRegistrationOutcome> {
        for _ in 0..20 {
            match self
                .register_once_with_id(None, input, prepared_fingerprint, now)
                .await
            {
                Err(DbError::Sqlx(sqlx::Error::Database(error)))
                    if super::is_sqlite_busy_retryable(error.as_ref()) =>
                {
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
                result => return result,
            }
        }
        self.register_once_with_id(None, input, prepared_fingerprint, now)
            .await
    }

    pub async fn register_allocated(
        &self,
        workflow_id: WorkflowId,
        input: &WakeRegistrationIntent,
        prepared_fingerprint: &str,
        now: Timestamp,
    ) -> DbResult<WakeRegistrationOutcome> {
        for _ in 0..20 {
            match self
                .register_once_with_id(Some(workflow_id), input, prepared_fingerprint, now)
                .await
            {
                Err(DbError::Sqlx(sqlx::Error::Database(error)))
                    if super::is_sqlite_busy_retryable(error.as_ref()) =>
                {
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
                result => return result,
            }
        }
        self.register_once_with_id(Some(workflow_id), input, prepared_fingerprint, now)
            .await
    }

    async fn register_once_with_id(
        &self,
        allocated_workflow_id: Option<WorkflowId>,
        input: &WakeRegistrationIntent,
        prepared_fingerprint: &str,
        now: Timestamp,
    ) -> DbResult<WakeRegistrationOutcome> {
        let mut tx = self.workflow_repo.begin_tx().await?;
        let existing = fetch_existing_binding_tx(&mut tx, input).await?;
        if let Some(existing) = existing {
            tx.commit().await?;
            return Ok(if existing.prepared_fingerprint == prepared_fingerprint {
                WakeRegistrationOutcome::Replayed {
                    workflow_id: existing.workflow_id,
                    receipt: replay_receipt(&existing),
                }
            } else {
                WakeRegistrationOutcome::Conflict
            });
        }

        let workflow_id = match allocated_workflow_id {
            Some(workflow_id) => workflow_id,
            None => next_global_workflow_id_tx(&mut tx).await?,
        };
        let snapshot = WakeRegistrationSnapshot {
            contract_id: input.contract_id.clone(),
            resource: input.resource.clone(),
            registered: true,
            terminal: None,
            runtime_availability: wake_profile::RuntimeAvailability::Idle,
        };
        let observe_intent = ObserveHandleIntent {
            contract_id: input.contract_id.clone(),
            resource: input.resource.clone(),
            expires_at: input.expires_at,
        };
        let acceptance = ErasedAcceptanceProfile::from_parts(
            wake_profile::profile(),
            wake_profile::acceptance_profile().supported_codecs.clone(),
            true,
            false,
        );
        let create = CreateWorkflowWithExternalAcceptance {
            workflow_id,
            profile: wake_profile::profile(),
            acceptance,
            target_scope: phoenix_workflow::ScopeId::new(format!(
                "wake:{}:{}",
                input.conversation_id, input.contract_id
            ))
            .ok_or_else(|| DbError::Serialization("empty wake acceptance identity".to_string()))?,
            idempotency_key: phoenix_workflow::NonEmptyExternalKey::new(format!(
                "wake:{}:{}",
                input.conversation_id,
                resource_key(&input.resource)
            ))
            .ok_or_else(|| DbError::Serialization("empty wake acceptance identity".to_string()))?,
            intent_fingerprint: prepared_fingerprint.to_string(),
            snapshot_codec: wake_profile::snapshot_codec(),
            snapshot_payload: json_blob(&snapshot)?,
            receipt_handle: resource_key(&input.resource).into_bytes(),
            disposition_handle: input.contract_id.clone().into_bytes(),
            now,
        };
        tx.insert_workflow(&create).await?;

        let receipt = WakeRegistrationReceipt {
            contract_id: input.contract_id.clone(),
            resource: input.resource.clone(),
            expires_at: input.expires_at,
            registering_tool_use_id: input.registering_tool_use_id.clone(),
        };
        let plan = CommitTransitionPlanCas {
            workflow_id,
            expected_version: Version(0),
            transition_id: TransitionId(1),
            generation: Generation(0),
            next_status: WorkflowStatus::Active,
            event_codec: local_codec(&wake_profile::event_codec()),
            event_payload: json_blob(&WakeRegistrationEvent::Registered)?,
            next_snapshot_codec: local_codec(&wake_profile::snapshot_codec()),
            next_snapshot_payload: json_blob(&snapshot)?,
            committed_at: now,
            effects: vec![LocalEffectDecl {
                effect_id: REGISTRATION_EFFECT_ID,
                declared_workflow_version: Version(1),
                family: "wake.observe".to_string(),
                kind: "observe_handle".to_string(),
                intent_codec: local_codec(&wake_profile::intent_codec()),
                intent_payload: json_blob(&observe_intent)?,
                generation: Generation(0),
                role: EffectRole::Required,
                capability: phoenix_workflow::ExecutionCapability::ReclaimableObservation,
                next_eligible_at: None,
                destructive_resource: None,
                status: EffectStatus::Eligible,
            }],
            dependencies: vec![],
            barriers: vec![],
            barrier_members: vec![],
            deliveries: vec![],
            schedules: vec![],
        };
        match tx.commit_transition_plan(&plan).await? {
            CommitOutcome::Committed => {}
            CommitOutcome::VersionConflict
            | CommitOutcome::InvalidPlan
            | CommitOutcome::UnsupportedCodec => {
                tx.rollback().await?;
                return Err(DbError::Serialization(
                    "wake registration transition was rejected".to_string(),
                ));
            }
        }
        #[cfg(test)]
        maybe_fail_after_canonical_transition(self.failpoint_namespace, workflow_id)?;
        insert_binding_tx(&mut tx, workflow_id, input, prepared_fingerprint, now).await?;
        tx.commit().await?;
        Ok(WakeRegistrationOutcome::Registered {
            workflow_id,
            receipt,
        })
    }

    pub async fn fetch_binding(
        &self,
        workflow_id: WorkflowId,
    ) -> DbResult<Option<WakeBindingRecord>> {
        let mut tx = self.workflow_repo.begin_tx().await?;
        let row = fetch_binding_by_workflow_tx(&mut tx, workflow_id).await?;
        tx.commit().await?;
        Ok(row)
    }

    pub async fn reload_binding(
        &self,
        workflow_id: WorkflowId,
    ) -> DbResult<Option<WakeBindingRecord>> {
        self.fetch_binding(workflow_id).await
    }

    pub async fn fetch_binding_for_conversation_contract(
        &self,
        conversation_id: &str,
        contract_id: &str,
    ) -> DbResult<Option<WakeBindingRecord>> {
        let mut tx = self.workflow_repo.begin_tx().await?;
        let row = sqlx::query(
            "SELECT workflow_id, conversation_id, contract_id, profile_kind, profile_version,
                    scope_kind, scope_stable_key, resource_kind, bash_handle_id,
                    tmux_server_token, tmux_window_id, tmux_completion_policy, registering_tool_use_id,
                    expires_at, prepared_fingerprint
             FROM wake_bindings
             WHERE conversation_id = ?1 AND contract_id = ?2
             LIMIT 1",
        )
        .bind(conversation_id)
        .bind(contract_id)
        .fetch_optional(&mut *tx.tx)
        .await?;
        let out = row.as_ref().map(binding_from_row).transpose()?;
        tx.commit().await?;
        Ok(out)
    }

    pub async fn claim_observation_if_eligible(
        &self,
        workflow_id: WorkflowId,
        process_incarnation: ProcessIncarnation,
        now: Timestamp,
        lease_until: phoenix_workflow::LeaseExpiry,
    ) -> DbResult<WakeObservationOutcome> {
        for _ in 0..20 {
            match self
                .claim_observation_if_eligible_once(
                    workflow_id,
                    process_incarnation,
                    now,
                    lease_until,
                )
                .await
            {
                Err(DbError::Sqlx(sqlx::Error::Database(error)))
                    if super::is_sqlite_busy_retryable(error.as_ref()) =>
                {
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
                result => return result,
            }
        }
        self.claim_observation_if_eligible_once(workflow_id, process_incarnation, now, lease_until)
            .await
    }

    async fn claim_observation_if_eligible_once(
        &self,
        workflow_id: WorkflowId,
        process_incarnation: ProcessIncarnation,
        now: Timestamp,
        lease_until: phoenix_workflow::LeaseExpiry,
    ) -> DbResult<WakeObservationOutcome> {
        let mut tx = self.workflow_repo.begin_tx().await?;
        let effect = sqlx::query(
            "SELECT status FROM workflow_effects WHERE workflow_id = ?1 AND effect_id = ?2",
        )
        .bind(to_i64(workflow_id.0, "workflow_id")?)
        .bind(to_i64(REGISTRATION_EFFECT_ID.0, "effect_id")?)
        .fetch_optional(&mut *tx.tx)
        .await?;
        let Some(effect) = effect else {
            tx.rollback().await?;
            return Ok(WakeObservationOutcome::Ineligible);
        };
        let initial_effect_status = parse_effect_status(&effect.get::<String, _>("status"))?;

        let live_attempt = sqlx::query(
            "SELECT a.attempt_id, l.lease_until
             FROM workflow_attempts a
             LEFT JOIN workflow_reclaimable_leases l
               ON l.workflow_id = a.workflow_id AND l.attempt_id = a.attempt_id
             WHERE a.workflow_id = ?1
               AND a.effect_id = ?2
               AND a.status IN ('Begun', 'ObservationRecorded')
             ORDER BY a.attempt_id
             LIMIT 1",
        )
        .bind(to_i64(workflow_id.0, "workflow_id")?)
        .bind(to_i64(REGISTRATION_EFFECT_ID.0, "effect_id")?)
        .fetch_optional(&mut *tx.tx)
        .await?;
        if let Some(live_attempt) = live_attempt {
            let existing_lease_until = phoenix_workflow::LeaseExpiry(to_u64(
                live_attempt.get::<i64, _>("lease_until"),
                "lease_until",
            )?);
            if existing_lease_until.is_live_at(now) {
                tx.rollback().await?;
                return Ok(WakeObservationOutcome::Busy {
                    lease_until: existing_lease_until,
                });
            }
            let attempt_id = AttemptId(to_u64(
                live_attempt.get::<i64, _>("attempt_id"),
                "attempt_id",
            )?);
            let expired = expire_observation_lease_in_tx(
                &mut tx,
                &ExpireLeaseInput {
                    workflow_id,
                    effect_id: REGISTRATION_EFFECT_ID,
                    attempt_id,
                    now,
                },
            )
            .await?;
            if expired != AuthorityOutcome::Authorized {
                tx.rollback().await?;
                return Ok(WakeObservationOutcome::Ineligible);
            }
        } else if initial_effect_status != EffectStatus::Eligible {
            tx.rollback().await?;
            return Ok(WakeObservationOutcome::Ineligible);
        }

        let attempt_id = AttemptId(
            sqlx::query_scalar::<_, i64>(
                "SELECT COALESCE(MAX(attempt_id), 0) + 1 FROM workflow_attempts WHERE workflow_id = ?1",
            )
            .bind(to_i64(workflow_id.0, "workflow_id")?)
            .fetch_one(&mut *tx.tx)
            .await?
            .try_into()
            .map_err(|error| DbError::Serialization(format!("attempt_id: {error}")))?,
        );
        let result = tx
            .begin_attempt(&BeginAttemptInput {
                workflow_id,
                effect_id: REGISTRATION_EFFECT_ID,
                attempt_id,
                process_incarnation,
                now,
                lease_until: Some(lease_until),
            })
            .await?;
        match result.outcome {
            ClaimOutcome::Started => {
                tx.commit().await?;
                Ok(WakeObservationOutcome::Started { canonical: result })
            }
            ClaimOutcome::AuthorityConflict => {
                tx.rollback().await?;
                let lease = sqlx::query_scalar::<_, i64>(
                    "SELECT l.lease_until
                     FROM workflow_attempts a
                     JOIN workflow_reclaimable_leases l
                       ON l.workflow_id = a.workflow_id AND l.attempt_id = a.attempt_id
                     WHERE a.workflow_id = ?1
                       AND a.effect_id = ?2
                       AND a.status IN ('Begun', 'ObservationRecorded')
                     ORDER BY a.attempt_id
                     LIMIT 1",
                )
                .bind(to_i64(workflow_id.0, "workflow_id")?)
                .bind(to_i64(REGISTRATION_EFFECT_ID.0, "effect_id")?)
                .fetch_optional(&self.workflow_repo.pool)
                .await?;
                Ok(match lease {
                    Some(lease_until) => WakeObservationOutcome::Busy {
                        lease_until: phoenix_workflow::LeaseExpiry(to_u64(
                            lease_until,
                            "lease_until",
                        )?),
                    },
                    None => WakeObservationOutcome::Ineligible,
                })
            }
            ClaimOutcome::Ineligible | ClaimOutcome::UnsupportedCodec => {
                tx.rollback().await?;
                Ok(WakeObservationOutcome::Ineligible)
            }
        }
    }

    pub async fn renew_observation_lease(
        &self,
        authority: &super::LocalAttemptAuthority,
        now: Timestamp,
        new_lease_until: phoenix_workflow::LeaseExpiry,
    ) -> DbResult<WakeLeaseRenewalOutcome> {
        Ok(
            match self
                .workflow_repo
                .renew_lease_exact(&RenewLeaseInput {
                    authority: authority.clone(),
                    now,
                    new_lease_until,
                })
                .await?
            {
                AuthorityOutcome::Authorized => WakeLeaseRenewalOutcome::Renewed,
                AuthorityOutcome::StaleAuthority => WakeLeaseRenewalOutcome::Stale,
            },
        )
    }

    pub async fn record_terminal_evidence(
        &self,
        workflow_id: WorkflowId,
        authority: &super::LocalAttemptAuthority,
        observation_id: u64,
        receipt_id: ReceiptId,
        delivery_id: DeliveryId,
        observation_time: Timestamp,
        evidence: &WakeTerminalEvidence,
    ) -> DbResult<WakeTerminalEvidenceOutcome> {
        for _ in 0..20 {
            match self
                .record_terminal_evidence_once(
                    workflow_id,
                    authority,
                    observation_id,
                    receipt_id,
                    delivery_id,
                    observation_time,
                    evidence,
                )
                .await
            {
                Err(DbError::Sqlx(sqlx::Error::Database(error)))
                    if super::is_sqlite_busy_retryable(error.as_ref()) =>
                {
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
                result => return result,
            }
        }
        self.record_terminal_evidence_once(
            workflow_id,
            authority,
            observation_id,
            receipt_id,
            delivery_id,
            observation_time,
            evidence,
        )
        .await
    }

    async fn record_terminal_evidence_once(
        &self,
        workflow_id: WorkflowId,
        authority: &super::LocalAttemptAuthority,
        observation_id: u64,
        receipt_id: ReceiptId,
        delivery_id: DeliveryId,
        observation_time: Timestamp,
        evidence: &WakeTerminalEvidence,
    ) -> DbResult<WakeTerminalEvidenceOutcome> {
        let mut tx = self.workflow_repo.begin_tx().await?;
        let Some(binding) = fetch_binding_by_workflow_tx(&mut tx, workflow_id).await? else {
            tx.rollback().await?;
            return Ok(WakeTerminalEvidenceOutcome::StaleAttempt);
        };
        if !resource_matches_evidence(&binding.resource, evidence) {
            tx.rollback().await?;
            return Ok(WakeTerminalEvidenceOutcome::WrongResource);
        }
        let evidence_time = evidence_occurred_at(evidence);
        if evidence_time.0 > observation_time.0 {
            tx.rollback().await?;
            return Ok(WakeTerminalEvidenceOutcome::EvidenceAfterObservation);
        }
        if evidence_time.0 > binding.expires_at.0 {
            tx.rollback().await?;
            return Ok(WakeTerminalEvidenceOutcome::EvidenceAfterExpiry);
        }
        if let Some(existing) =
            fetch_projection_by_receipt_tx(&mut tx, workflow_id, receipt_id).await?
        {
            let delivery = fetch_pending_delivery_by_delivery_id_tx(
                &mut tx,
                workflow_id,
                existing.delivery_id,
            )
            .await?
            .ok_or_else(|| {
                DbError::Serialization("wake projection missing canonical delivery".to_string())
            })?;
            let receipt = fetch_receipt_tx(&mut tx, workflow_id, receipt_id)
                .await?
                .ok_or_else(|| {
                    DbError::Serialization("wake projection missing canonical receipt".to_string())
                })?;
            tx.commit().await?;
            return Ok(WakeTerminalEvidenceOutcome::Replayed {
                receipt,
                delivery: WakePendingDelivery {
                    workflow_id,
                    conversation_id: existing.conversation_id.clone(),
                    receipt: existing,
                    canonical_delivery: delivery,
                },
            });
        }

        let observation = tx
            .record_observation(&RecordObservationInput {
                authority: authority.clone(),
                observation_id,
                now: observation_time,
                observed_at: evidence_time,
                observation_codec: local_codec(&wake_profile::terminal_codec()),
                observation_payload: json_blob(evidence)?,
            })
            .await?;
        if observation.outcome != AuthorityOutcome::Authorized {
            if let Some(existing) =
                fetch_projection_for_attempt_tx(&mut tx, workflow_id, authority.attempt_id).await?
            {
                let delivery = fetch_pending_delivery_by_delivery_id_tx(
                    &mut tx,
                    workflow_id,
                    existing.delivery_id,
                )
                .await?
                .ok_or_else(|| {
                    DbError::Serialization("wake replay missing canonical delivery".to_string())
                })?;
                let receipt = fetch_receipt_tx(&mut tx, workflow_id, existing.receipt_id)
                    .await?
                    .ok_or_else(|| {
                        DbError::Serialization("wake replay missing canonical receipt".to_string())
                    })?;
                tx.commit().await?;
                return Ok(WakeTerminalEvidenceOutcome::Replayed {
                    receipt,
                    delivery: WakePendingDelivery {
                        workflow_id,
                        conversation_id: existing.conversation_id.clone(),
                        receipt: existing,
                        canonical_delivery: delivery,
                    },
                });
            }
            tx.rollback().await?;
            return Ok(WakeTerminalEvidenceOutcome::StaleAttempt);
        }

        let terminal = WakeTerminalPayload::Fired {
            contract_id: binding.contract_id.clone(),
            resource: binding.resource.clone(),
            evidence: evidence.clone(),
            resolved_at: observation_time,
        };
        let canonical = tx
            .accept_receipt_and_delivery(&AcceptReceiptInput {
                authority: authority.clone(),
                receipt_id,
                delivery_id,
                attempt_id: Some(authority.attempt_id),
                origin: ReceiptOrigin::Execution,
                receipt_codec: local_codec(&wake_profile::terminal_codec()),
                receipt_payload: json_blob(&terminal)?,
                receipt_event_codec: local_codec(&wake_profile::terminal_codec()),
                receipt_event_payload: json_blob(&terminal)?,
                receipt_event_requires_runtime_acceptance: false,
                request_runtime_acceptance_for_cancellation: false,
            })
            .await?;
        if canonical.outcome != AuthorityOutcome::Authorized {
            if let Some(existing) =
                fetch_projection_for_attempt_tx(&mut tx, workflow_id, authority.attempt_id).await?
            {
                let delivery = fetch_pending_delivery_by_delivery_id_tx(
                    &mut tx,
                    workflow_id,
                    existing.delivery_id,
                )
                .await?
                .ok_or_else(|| {
                    DbError::Serialization("wake replay missing canonical delivery".to_string())
                })?;
                let receipt = fetch_receipt_tx(&mut tx, workflow_id, existing.receipt_id)
                    .await?
                    .ok_or_else(|| {
                        DbError::Serialization("wake replay missing canonical receipt".to_string())
                    })?;
                tx.commit().await?;
                return Ok(WakeTerminalEvidenceOutcome::Replayed {
                    receipt,
                    delivery: WakePendingDelivery {
                        workflow_id,
                        conversation_id: existing.conversation_id.clone(),
                        receipt: existing,
                        canonical_delivery: delivery,
                    },
                });
            }
            tx.rollback().await?;
            return Ok(WakeTerminalEvidenceOutcome::StaleAttempt);
        }
        let head = tx.fetch_workflow_head(workflow_id).await?.ok_or_else(|| {
            DbError::Serialization("wake workflow head missing after receipt".to_string())
        })?;
        let next_snapshot = WakeRegistrationSnapshot {
            contract_id: binding.contract_id.clone(),
            resource: binding.resource.clone(),
            registered: true,
            terminal: Some(terminal.clone()),
            runtime_availability: wake_profile::RuntimeAvailability::Idle,
        };
        let event = WakeRegistrationEvent::TerminalProjected {
            terminal: Box::new(terminal.clone()),
        };
        if !tx
            .commit_transition_head_cas(
                workflow_id,
                head.version,
                head.generation,
                WorkflowStatus::Completed,
                &local_codec(&wake_profile::event_codec()),
                &json_blob(&event)?,
                &local_codec(&wake_profile::snapshot_codec()),
                &json_blob(&next_snapshot)?,
                TransitionId(head.version.next().0),
                observation_time,
            )
            .await?
        {
            tx.rollback().await?;
            return Ok(WakeTerminalEvidenceOutcome::StaleAttempt);
        }
        #[cfg(test)]
        maybe_fail_after_canonical_receipt(self.failpoint_namespace, workflow_id)?;
        insert_terminal_receipt_projection_tx(
            &mut tx,
            &binding,
            canonical.receipt.as_ref().expect("authorized receipt"),
            canonical.delivery.as_ref().expect("authorized delivery"),
            &terminal,
        )
        .await?;
        let projection = fetch_projection_by_receipt_tx(&mut tx, workflow_id, receipt_id)
            .await?
            .ok_or_else(|| {
                DbError::Serialization("wake projection missing after insert".to_string())
            })?;
        tx.commit().await?;
        Ok(WakeTerminalEvidenceOutcome::Recorded {
            receipt: canonical.receipt.expect("authorized receipt"),
            delivery: WakePendingDelivery {
                workflow_id,
                conversation_id: binding.conversation_id.clone(),
                receipt: projection,
                canonical_delivery: canonical.delivery.expect("authorized delivery"),
            },
        })
    }

    pub async fn record_terminal_allocated(
        &self,
        input: &WakeTerminalEvidenceInput,
    ) -> DbResult<WakeTerminalEvidenceOutcome> {
        let mut tx = self.workflow_repo.begin_tx().await?;
        let observation_id = tx
            .allocate_sequence_value(input.workflow_id, WorkflowSequenceName::Observation)
            .await?;
        let receipt_id = ReceiptId(
            tx.allocate_sequence_value(input.workflow_id, WorkflowSequenceName::Receipt)
                .await?,
        );
        let delivery_id = DeliveryId(
            tx.allocate_sequence_value(input.workflow_id, WorkflowSequenceName::Delivery)
                .await?,
        );
        tx.commit().await?;
        self.record_terminal_evidence(
            input.workflow_id,
            &input.authority,
            observation_id,
            receipt_id,
            delivery_id,
            input.observation_time,
            &input.evidence,
        )
        .await
    }

    pub async fn forget_if_unresolved_allocated(
        &self,
        input: &WakeForgetIfUnresolvedInput,
    ) -> DbResult<WakeForgetIfUnresolvedOutcome> {
        for _ in 0..20 {
            match self.forget_if_unresolved_allocated_once(input).await {
                Err(DbError::Sqlx(sqlx::Error::Database(error)))
                    if super::is_sqlite_busy_retryable(error.as_ref()) =>
                {
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
                result => return result,
            }
        }
        self.forget_if_unresolved_allocated_once(input).await
    }

    async fn forget_if_unresolved_allocated_once(
        &self,
        input: &WakeForgetIfUnresolvedInput,
    ) -> DbResult<WakeForgetIfUnresolvedOutcome> {
        let mut tx = self.workflow_repo.begin_tx().await?;
        let Some(binding) = fetch_binding_by_workflow_tx(&mut tx, input.workflow_id).await? else {
            tx.rollback().await?;
            return Ok(WakeForgetIfUnresolvedOutcome::Stale);
        };
        if let Some(existing) = fetch_any_terminal_projection_tx(&mut tx, input.workflow_id).await?
        {
            let outcome = replay_terminal_projection(&mut tx, input.workflow_id, existing).await?;
            tx.commit().await?;
            return Ok(WakeForgetIfUnresolvedOutcome::Replayed {
                receipt: outcome.0,
                delivery: outcome.1,
            });
        }
        let Some(head) = tx.fetch_workflow_head(input.workflow_id).await? else {
            tx.rollback().await?;
            return Ok(WakeForgetIfUnresolvedOutcome::Stale);
        };

        let receipt_id = ReceiptId(
            tx.allocate_sequence_value(input.workflow_id, WorkflowSequenceName::Receipt)
                .await?,
        );
        let delivery_id = DeliveryId(
            tx.allocate_sequence_value(input.workflow_id, WorkflowSequenceName::Delivery)
                .await?,
        );
        let terminal = WakeTerminalPayload::Forgotten {
            contract_id: binding.contract_id.clone(),
            resource: binding.resource.clone(),
            reason: input.reason,
            resolved_at: input.now,
        };
        let next_snapshot = WakeRegistrationSnapshot {
            contract_id: binding.contract_id.clone(),
            resource: binding.resource.clone(),
            registered: true,
            terminal: Some(terminal.clone()),
            runtime_availability: wake_profile::RuntimeAvailability::Idle,
        };
        let event = WakeRegistrationEvent::TerminalProjected {
            terminal: Box::new(terminal.clone()),
        };
        let committed = tx
            .commit_transition_head_cas(
                input.workflow_id,
                head.version,
                head.generation,
                WorkflowStatus::Completed,
                &local_codec(&wake_profile::event_codec()),
                &json_blob(&event)?,
                &local_codec(&wake_profile::snapshot_codec()),
                &json_blob(&next_snapshot)?,
                TransitionId(head.version.next().0),
                input.now,
            )
            .await?;
        if !committed {
            if let Some(existing) =
                fetch_any_terminal_projection_tx(&mut tx, input.workflow_id).await?
            {
                let outcome =
                    replay_terminal_projection(&mut tx, input.workflow_id, existing).await?;
                tx.commit().await?;
                return Ok(WakeForgetIfUnresolvedOutcome::Replayed {
                    receipt: outcome.0,
                    delivery: outcome.1,
                });
            }
            tx.rollback().await?;
            return Ok(WakeForgetIfUnresolvedOutcome::Stale);
        }
        sqlx::query(
            "UPDATE workflow_attempts
             SET status = 'AuthorityLost'
             WHERE workflow_id = ?1 AND status IN ('Begun', 'ObservationRecorded')",
        )
        .bind(to_i64(input.workflow_id.0, "workflow_id")?)
        .execute(&mut *tx.tx)
        .await?;
        sqlx::query("DELETE FROM workflow_reclaimable_leases WHERE workflow_id = ?1")
            .bind(to_i64(input.workflow_id.0, "workflow_id")?)
            .execute(&mut *tx.tx)
            .await?;
        sqlx::query("INSERT INTO workflow_receipts (workflow_id, receipt_id, effect_id, generation, declared_workflow_version, process_incarnation, attempt_id, origin, receipt_codec_family, receipt_codec_version, receipt_payload) VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7, ?8, ?9, ?10)")
            .bind(to_i64(input.workflow_id.0, "workflow_id")?)
            .bind(to_i64(receipt_id.0, "receipt_id")?)
            .bind(to_i64(REGISTRATION_EFFECT_ID.0, "effect_id")?)
            .bind(to_i64(head.generation.0, "generation")?)
            .bind(to_i64(head.version.next().0, "declared_workflow_version")?)
            .bind(0_i64)
            .bind("ForgottenInterruption")
            .bind(wake_profile::terminal_codec().family)
            .bind(i64::from(wake_profile::terminal_codec().version))
            .bind(json_blob(&terminal)?)
            .execute(&mut *tx.tx)
            .await?;
        sqlx::query("INSERT INTO workflow_deliveries (workflow_id, delivery_id, effect_id, barrier_id, consumer_kind, event_codec_family, event_codec_version, payload_kind, payload_blob, requires_runtime_acceptance, status, runtime_acceptance_status, suppression_reason, accepted_by_transition_id) VALUES (?1, ?2, ?3, NULL, 'reducer', ?4, ?5, 'Receipt', ?6, 1, 'Pending', 'Owed', NULL, NULL)")
            .bind(to_i64(input.workflow_id.0, "workflow_id")?)
            .bind(to_i64(delivery_id.0, "delivery_id")?)
            .bind(to_i64(REGISTRATION_EFFECT_ID.0, "effect_id")?)
            .bind(wake_profile::terminal_codec().family)
            .bind(i64::from(wake_profile::terminal_codec().version))
            .bind(json_blob(&terminal)?)
            .execute(&mut *tx.tx)
            .await?;
        insert_terminal_receipt_projection_tx(
            &mut tx,
            &binding,
            &LocalReceiptRecord {
                receipt_id,
                workflow_id: input.workflow_id,
                effect_id: REGISTRATION_EFFECT_ID,
                generation: head.generation,
                declared_workflow_version: head.version.next(),
                process_incarnation: ProcessIncarnation(0),
                attempt_id: None,
                origin: ReceiptOrigin::ForgottenInterruption,
                receipt_codec: local_codec(&wake_profile::terminal_codec()),
                receipt_payload: json_blob(&terminal)?,
            },
            &LocalDeliveryRecord {
                delivery_id,
                workflow_id: input.workflow_id,
                effect_id: Some(REGISTRATION_EFFECT_ID),
                barrier_id: None,
                consumer_kind: "reducer".to_string(),
                event_codec: local_codec(&wake_profile::terminal_codec()),
                payload_kind: super::LocalDeliveryPayloadKind::Receipt,
                payload_blob: json_blob(&terminal)?,
                requires_runtime_acceptance: true,
                status: phoenix_workflow::DeliveryStatus::Pending,
                runtime_acceptance_status: Some(RuntimeAcceptanceStatus::Owed),
                suppression_reason: None,
                accepted_by_transition_id: None,
            },
            &terminal,
        )
        .await?;
        let projection = fetch_any_terminal_projection_tx(&mut tx, input.workflow_id)
            .await?
            .ok_or_else(|| {
                DbError::Serialization("wake forgotten projection missing after insert".to_string())
            })?;
        let outcome = replay_terminal_projection(&mut tx, input.workflow_id, projection).await?;
        tx.commit().await?;
        Ok(WakeForgetIfUnresolvedOutcome::Forgotten {
            receipt: outcome.0,
            delivery: outcome.1,
        })
    }

    pub async fn cancel_allocated(
        &self,
        input: &WakeCancelIfUnresolvedInput,
    ) -> DbResult<WakeCancellationOutcome> {
        let mut tx = self.workflow_repo.begin_tx().await?;
        let Some(head) = tx.fetch_workflow_head(input.workflow_id).await? else {
            tx.rollback().await?;
            return Ok(WakeCancellationOutcome::Stale);
        };
        let Some(binding) = fetch_binding_by_workflow_tx(&mut tx, input.workflow_id).await? else {
            tx.rollback().await?;
            return Ok(WakeCancellationOutcome::Stale);
        };
        if input
            .expected_conversation_id
            .as_deref()
            .is_some_and(|expected| expected != binding.conversation_id)
            || input
                .expected_contract_id
                .as_deref()
                .is_some_and(|expected| expected != binding.contract_id)
        {
            tx.rollback().await?;
            return Ok(WakeCancellationOutcome::Stale);
        }
        let receipt_id = ReceiptId(
            tx.allocate_sequence_value(input.workflow_id, WorkflowSequenceName::Receipt)
                .await?,
        );
        let delivery_id = DeliveryId(
            tx.allocate_sequence_value(input.workflow_id, WorkflowSequenceName::Delivery)
                .await?,
        );
        tx.commit().await?;
        self.cancel(&WakeCancellationInput {
            workflow_id: input.workflow_id,
            expected_version: head.version,
            expected_generation: head.generation,
            receipt_id,
            delivery_id,
            timestamp: input.timestamp,
            reason: input.reason,
        })
        .await
    }

    pub async fn cancel(&self, input: &WakeCancellationInput) -> DbResult<WakeCancellationOutcome> {
        for _ in 0..20 {
            match self.cancel_once(input).await {
                Err(DbError::Sqlx(sqlx::Error::Database(error)))
                    if super::is_sqlite_busy_retryable(error.as_ref()) =>
                {
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
                result => return result,
            }
        }
        self.cancel_once(input).await
    }

    pub async fn expire_if_unresolved(
        &self,
        workflow_id: WorkflowId,
        now: Timestamp,
    ) -> DbResult<WakeExpireIfUnresolvedOutcome> {
        for _ in 0..20 {
            match self.expire_if_unresolved_once(workflow_id, now).await {
                Err(DbError::Sqlx(sqlx::Error::Database(error)))
                    if super::is_sqlite_busy_retryable(error.as_ref()) =>
                {
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
                result => return result,
            }
        }
        self.expire_if_unresolved_once(workflow_id, now).await
    }

    async fn expire_if_unresolved_once(
        &self,
        workflow_id: WorkflowId,
        now: Timestamp,
    ) -> DbResult<WakeExpireIfUnresolvedOutcome> {
        let mut tx = self.workflow_repo.begin_tx().await?;
        let Some(binding) = fetch_binding_by_workflow_tx(&mut tx, workflow_id).await? else {
            tx.rollback().await?;
            return Ok(WakeExpireIfUnresolvedOutcome::Stale);
        };
        if binding.expires_at.0 > now.0 {
            tx.rollback().await?;
            return Ok(WakeExpireIfUnresolvedOutcome::NotDue);
        }
        let live_attempt = sqlx::query(
            "SELECT a.attempt_id, l.lease_until
             FROM workflow_attempts a
             JOIN workflow_reclaimable_leases l
               ON l.workflow_id = a.workflow_id AND l.attempt_id = a.attempt_id
             WHERE a.workflow_id = ?1 AND a.effect_id = ?2
               AND a.status IN ('Begun', 'ObservationRecorded')
             ORDER BY a.attempt_id LIMIT 1",
        )
        .bind(to_i64(workflow_id.0, "workflow_id")?)
        .bind(to_i64(REGISTRATION_EFFECT_ID.0, "effect_id")?)
        .fetch_optional(&mut *tx.tx)
        .await?;
        if let Some(lease) = live_attempt {
            let lease_until = phoenix_workflow::LeaseExpiry(to_u64(
                lease.get::<i64, _>("lease_until"),
                "lease_until",
            )?);
            if lease_until.is_live_at(now) {
                tx.rollback().await?;
                return Ok(WakeExpireIfUnresolvedOutcome::NotDue);
            }
            let expired = expire_observation_lease_in_tx(
                &mut tx,
                &ExpireLeaseInput {
                    workflow_id,
                    effect_id: REGISTRATION_EFFECT_ID,
                    attempt_id: AttemptId(to_u64(lease.get::<i64, _>("attempt_id"), "attempt_id")?),
                    now,
                },
            )
            .await?;
            if expired != AuthorityOutcome::Authorized {
                tx.rollback().await?;
                return Ok(WakeExpireIfUnresolvedOutcome::Stale);
            }
        }
        if let Some(existing) = fetch_any_terminal_projection_tx(&mut tx, workflow_id).await? {
            let outcome = replay_terminal_projection(&mut tx, workflow_id, existing).await?;
            tx.commit().await?;
            return Ok(WakeExpireIfUnresolvedOutcome::Replayed {
                receipt: outcome.0,
                delivery: outcome.1,
            });
        }
        let Some(head) = tx.fetch_workflow_head(workflow_id).await? else {
            tx.rollback().await?;
            return Ok(WakeExpireIfUnresolvedOutcome::Stale);
        };

        let receipt_id = ReceiptId(
            tx.allocate_sequence_value(workflow_id, WorkflowSequenceName::Receipt)
                .await?,
        );
        let delivery_id = DeliveryId(
            tx.allocate_sequence_value(workflow_id, WorkflowSequenceName::Delivery)
                .await?,
        );
        let next_snapshot = WakeRegistrationSnapshot {
            contract_id: binding.contract_id.clone(),
            resource: binding.resource.clone(),
            registered: true,
            terminal: Some(WakeTerminalPayload::Expired {
                contract_id: binding.contract_id.clone(),
                resource: binding.resource.clone(),
                resolved_at: now,
            }),
            runtime_availability: wake_profile::RuntimeAvailability::Idle,
        };
        let next_snapshot_payload = json_blob(&next_snapshot)?;
        let next_snapshot_codec = LocalCodec {
            family: wake_profile::snapshot_codec().family.to_string(),
            version: wake_profile::snapshot_codec().version,
        };
        let event = WakeRegistrationEvent::TerminalProjected {
            terminal: Box::new(next_snapshot.terminal.clone().expect("terminal")),
        };
        let event_payload = json_blob(&event)?;
        let event_codec = local_codec(&wake_profile::event_codec());
        let transition_id = TransitionId(head.version.next().0);
        let committed = tx
            .commit_transition_head_cas(
                workflow_id,
                head.version,
                head.generation,
                WorkflowStatus::Completed,
                &event_codec,
                &event_payload,
                &next_snapshot_codec,
                &next_snapshot_payload,
                transition_id,
                now,
            )
            .await?;
        if !committed {
            if let Some(existing) = fetch_any_terminal_projection_tx(&mut tx, workflow_id).await? {
                let outcome = replay_terminal_projection(&mut tx, workflow_id, existing).await?;
                tx.commit().await?;
                return Ok(WakeExpireIfUnresolvedOutcome::Replayed {
                    receipt: outcome.0,
                    delivery: outcome.1,
                });
            }
            tx.rollback().await?;
            return Ok(WakeExpireIfUnresolvedOutcome::Stale);
        }
        sqlx::query(
            "UPDATE workflow_attempts
             SET status = 'AuthorityLost'
             WHERE workflow_id = ?1 AND status IN ('Begun', 'ObservationRecorded')",
        )
        .bind(to_i64(workflow_id.0, "workflow_id")?)
        .execute(&mut *tx.tx)
        .await?;
        sqlx::query("DELETE FROM workflow_reclaimable_leases WHERE workflow_id = ?1")
            .bind(to_i64(workflow_id.0, "workflow_id")?)
            .execute(&mut *tx.tx)
            .await?;
        let terminal = match next_snapshot.terminal.clone().expect("terminal") {
            WakeTerminalPayload::Expired {
                contract_id,
                resource,
                resolved_at,
            } => WakeTerminalPayload::Expired {
                contract_id,
                resource,
                resolved_at,
            },
            WakeTerminalPayload::Fired { .. }
            | WakeTerminalPayload::Cancelled { .. }
            | WakeTerminalPayload::Forgotten { .. } => unreachable!(),
        };
        sqlx::query("INSERT INTO workflow_receipts (workflow_id, receipt_id, effect_id, generation, declared_workflow_version, process_incarnation, attempt_id, origin, receipt_codec_family, receipt_codec_version, receipt_payload) VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7, ?8, ?9, ?10)")
            .bind(to_i64(workflow_id.0, "workflow_id")?)
            .bind(to_i64(receipt_id.0, "receipt_id")?)
            .bind(to_i64(REGISTRATION_EFFECT_ID.0, "effect_id")?)
            .bind(to_i64(head.generation.0, "generation")?)
            .bind(to_i64(head.version.next().0, "declared_workflow_version")?)
            .bind(0_i64)
            .bind("DeadlineExpiration")
            .bind(wake_profile::terminal_codec().family)
            .bind(i64::from(wake_profile::terminal_codec().version))
            .bind(json_blob(&terminal)?)
            .execute(&mut *tx.tx)
            .await?;
        sqlx::query("INSERT INTO workflow_deliveries (workflow_id, delivery_id, effect_id, barrier_id, consumer_kind, event_codec_family, event_codec_version, payload_kind, payload_blob, requires_runtime_acceptance, status, runtime_acceptance_status, suppression_reason, accepted_by_transition_id) VALUES (?1, ?2, ?3, NULL, 'reducer', ?4, ?5, 'Receipt', ?6, 1, 'Pending', 'Owed', NULL, NULL)")
            .bind(to_i64(workflow_id.0, "workflow_id")?)
            .bind(to_i64(delivery_id.0, "delivery_id")?)
            .bind(to_i64(REGISTRATION_EFFECT_ID.0, "effect_id")?)
            .bind(wake_profile::terminal_codec().family)
            .bind(i64::from(wake_profile::terminal_codec().version))
            .bind(json_blob(&terminal)?)
            .execute(&mut *tx.tx)
            .await?;
        insert_terminal_receipt_projection_tx(
            &mut tx,
            &binding,
            &LocalReceiptRecord {
                receipt_id,
                workflow_id,
                effect_id: REGISTRATION_EFFECT_ID,
                generation: head.generation,
                declared_workflow_version: head.version.next(),
                process_incarnation: ProcessIncarnation(0),
                attempt_id: None,
                origin: ReceiptOrigin::DeadlineExpiration,
                receipt_codec: local_codec(&wake_profile::terminal_codec()),
                receipt_payload: json_blob(&terminal)?,
            },
            &LocalDeliveryRecord {
                delivery_id,
                workflow_id,
                effect_id: Some(REGISTRATION_EFFECT_ID),
                barrier_id: None,
                consumer_kind: "reducer".to_string(),
                event_codec: local_codec(&wake_profile::terminal_codec()),
                payload_kind: super::LocalDeliveryPayloadKind::Receipt,
                payload_blob: json_blob(&terminal)?,
                requires_runtime_acceptance: true,
                status: phoenix_workflow::DeliveryStatus::Pending,
                runtime_acceptance_status: Some(RuntimeAcceptanceStatus::Owed),
                suppression_reason: None,
                accepted_by_transition_id: None,
            },
            &terminal,
        )
        .await?;
        let projection = fetch_any_terminal_projection_tx(&mut tx, workflow_id)
            .await?
            .ok_or_else(|| {
                DbError::Serialization("wake expiry projection missing after insert".to_string())
            })?;
        let outcome = replay_terminal_projection(&mut tx, workflow_id, projection).await?;
        tx.commit().await?;
        Ok(WakeExpireIfUnresolvedOutcome::Expired {
            receipt: outcome.0,
            delivery: outcome.1,
        })
    }

    async fn cancel_once(
        &self,
        input: &WakeCancellationInput,
    ) -> DbResult<WakeCancellationOutcome> {
        let mut tx = self.workflow_repo.begin_tx().await?;
        let Some(binding) = fetch_binding_by_workflow_tx(&mut tx, input.workflow_id).await? else {
            tx.rollback().await?;
            return Ok(WakeCancellationOutcome::Stale);
        };
        let Some(head) = tx.fetch_workflow_head(input.workflow_id).await? else {
            tx.rollback().await?;
            return Ok(WakeCancellationOutcome::Stale);
        };
        if let Some(existing) = fetch_any_terminal_projection_tx(&mut tx, input.workflow_id).await?
        {
            let outcome = replay_terminal_projection(&mut tx, input.workflow_id, existing).await?;
            tx.commit().await?;
            return Ok(WakeCancellationOutcome::Replayed {
                receipt: outcome.0,
                delivery: outcome.1,
            });
        }
        if head.version != input.expected_version || head.generation != input.expected_generation {
            tx.rollback().await?;
            return Ok(WakeCancellationOutcome::Stale);
        }
        let terminal = WakeTerminalPayload::Cancelled {
            contract_id: binding.contract_id.clone(),
            resource: binding.resource.clone(),
            reason: input.reason,
            resolved_at: input.timestamp,
        };
        let snapshot = WakeRegistrationSnapshot {
            contract_id: binding.contract_id.clone(),
            resource: binding.resource.clone(),
            registered: true,
            terminal: Some(terminal.clone()),
            runtime_availability: wake_profile::RuntimeAvailability::Idle,
        };
        let updated = sqlx::query(
            "UPDATE workflows
             SET version = version + 1,
                 generation = ?3,
                 status = 'Cancelled',
                 snapshot_codec_family = ?4,
                 snapshot_codec_version = ?5,
                 snapshot_payload = ?6,
                 updated_at = ?7
             WHERE workflow_id = ?1 AND version = ?2 AND generation = ?8",
        )
        .bind(to_i64(input.workflow_id.0, "workflow_id")?)
        .bind(to_i64(input.expected_version.0, "expected_version")?)
        .bind(to_i64(input.expected_generation.next().0, "generation")?)
        .bind(wake_profile::snapshot_codec().family)
        .bind(i64::from(wake_profile::snapshot_codec().version))
        .bind(json_blob(&snapshot)?)
        .bind(to_i64(input.timestamp.0, "timestamp")?)
        .bind(to_i64(input.expected_generation.0, "expected_generation")?)
        .execute(&mut *tx.tx)
        .await?
        .rows_affected();
        if updated == 0 {
            if let Some(existing) =
                fetch_any_terminal_projection_tx(&mut tx, input.workflow_id).await?
            {
                let outcome =
                    replay_terminal_projection(&mut tx, input.workflow_id, existing).await?;
                tx.commit().await?;
                return Ok(WakeCancellationOutcome::Replayed {
                    receipt: outcome.0,
                    delivery: outcome.1,
                });
            }
            tx.rollback().await?;
            return Ok(WakeCancellationOutcome::Stale);
        }
        sqlx::query(
            "INSERT INTO workflow_transitions (
                workflow_id, transition_id, from_version, to_version, generation,
                event_codec_family, event_codec_version, event_payload, committed_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        )
        .bind(to_i64(input.workflow_id.0, "workflow_id")?)
        .bind(to_i64(input.expected_version.next().0, "transition_id")?)
        .bind(to_i64(input.expected_version.0, "from_version")?)
        .bind(to_i64(input.expected_version.next().0, "to_version")?)
        .bind(to_i64(input.expected_generation.next().0, "generation")?)
        .bind(wake_profile::event_codec().family)
        .bind(i64::from(wake_profile::event_codec().version))
        .bind(json_blob(&WakeRegistrationEvent::CancelRequested)?)
        .bind(to_i64(input.timestamp.0, "timestamp")?)
        .execute(&mut *tx.tx)
        .await?;
        sqlx::query(
            "UPDATE workflow_attempts
             SET status = 'AuthorityLost'
             WHERE workflow_id = ?1 AND status IN ('Begun', 'ObservationRecorded')",
        )
        .bind(to_i64(input.workflow_id.0, "workflow_id")?)
        .execute(&mut *tx.tx)
        .await?;
        sqlx::query("DELETE FROM workflow_reclaimable_leases WHERE workflow_id = ?1")
            .bind(to_i64(input.workflow_id.0, "workflow_id")?)
            .execute(&mut *tx.tx)
            .await?;
        #[cfg(test)]
        maybe_fail_after_canonical_transition(self.failpoint_namespace, input.workflow_id)?;
        sqlx::query("INSERT INTO workflow_receipts (workflow_id, receipt_id, effect_id, generation, declared_workflow_version, process_incarnation, attempt_id, origin, receipt_codec_family, receipt_codec_version, receipt_payload) VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7, ?8, ?9, ?10)")
            .bind(to_i64(input.workflow_id.0, "workflow_id")?)
            .bind(to_i64(input.receipt_id.0, "receipt_id")?)
            .bind(to_i64(REGISTRATION_EFFECT_ID.0, "effect_id")?)
            .bind(to_i64(input.expected_generation.next().0, "generation")?)
            .bind(to_i64(input.expected_version.next().0, "declared_workflow_version")?)
            .bind(0_i64)
            .bind("CancellationArbitration")
            .bind(wake_profile::terminal_codec().family)
            .bind(i64::from(wake_profile::terminal_codec().version))
            .bind(json_blob(&terminal)?)
            .execute(&mut *tx.tx)
            .await?;
        sqlx::query("INSERT INTO workflow_deliveries (workflow_id, delivery_id, effect_id, barrier_id, consumer_kind, event_codec_family, event_codec_version, payload_kind, payload_blob, requires_runtime_acceptance, status, runtime_acceptance_status, suppression_reason, accepted_by_transition_id) VALUES (?1, ?2, ?3, NULL, 'reducer', ?4, ?5, 'Receipt', ?6, 0, 'Pending', NULL, NULL, NULL)")
            .bind(to_i64(input.workflow_id.0, "workflow_id")?)
            .bind(to_i64(input.delivery_id.0, "delivery_id")?)
            .bind(to_i64(REGISTRATION_EFFECT_ID.0, "effect_id")?)
            .bind(wake_profile::terminal_codec().family)
            .bind(i64::from(wake_profile::terminal_codec().version))
            .bind(json_blob(&terminal)?)
            .execute(&mut *tx.tx)
            .await?;
        insert_terminal_receipt_projection_tx(
            &mut tx,
            &binding,
            &LocalReceiptRecord {
                receipt_id: input.receipt_id,
                workflow_id: input.workflow_id,
                effect_id: REGISTRATION_EFFECT_ID,
                generation: input.expected_generation.next(),
                declared_workflow_version: input.expected_version.next(),
                process_incarnation: ProcessIncarnation(0),
                attempt_id: None,
                origin: ReceiptOrigin::CancellationArbitration,
                receipt_codec: local_codec(&wake_profile::terminal_codec()),
                receipt_payload: json_blob(&terminal)?,
            },
            &LocalDeliveryRecord {
                delivery_id: input.delivery_id,
                workflow_id: input.workflow_id,
                effect_id: Some(REGISTRATION_EFFECT_ID),
                barrier_id: None,
                consumer_kind: "reducer".to_string(),
                event_codec: local_codec(&wake_profile::terminal_codec()),
                payload_kind: super::LocalDeliveryPayloadKind::Receipt,
                payload_blob: json_blob(&terminal)?,
                requires_runtime_acceptance: false,
                status: phoenix_workflow::DeliveryStatus::Pending,
                runtime_acceptance_status: None,
                suppression_reason: None,
                accepted_by_transition_id: None,
            },
            &terminal,
        )
        .await?;
        let projection = fetch_any_terminal_projection_tx(&mut tx, input.workflow_id)
            .await?
            .ok_or_else(|| {
                DbError::Serialization(
                    "wake cancellation projection missing after insert".to_string(),
                )
            })?;
        let outcome = replay_terminal_projection(&mut tx, input.workflow_id, projection).await?;
        tx.commit().await?;
        Ok(WakeCancellationOutcome::Cancelled {
            receipt: outcome.0,
            delivery: outcome.1,
        })
    }

    pub async fn list_pending_global(
        &self,
        after: Option<WakePendingGlobalCursor>,
        limit: usize,
    ) -> DbResult<Vec<WakePendingGlobalRow>> {
        let mut tx = self.workflow_repo.begin_tx().await?;
        let rows = match after {
            Some(after) => {
                sqlx::query(
                    "SELECT p.workflow_id, p.conversation_id, p.contract_id, p.delivery_id, p.receipt_id
                     FROM wake_terminal_receipts p
                     JOIN workflow_deliveries d
                       ON d.workflow_id = p.workflow_id AND d.delivery_id = p.delivery_id
                     WHERE d.status = 'Pending'
                       AND (
                           p.workflow_id > ?1
                           OR (p.workflow_id = ?1 AND p.delivery_id > ?2)
                       )
                     ORDER BY p.workflow_id, p.delivery_id
                     LIMIT ?3",
                )
                .bind(to_i64(after.workflow_id.0, "workflow_id")?)
                .bind(to_i64(after.delivery_id.0, "delivery_id")?)
                .bind(to_i64(limit as u64, "limit")?)
                .fetch_all(&mut *tx.tx)
                .await?
            }
            None => {
                sqlx::query(
                    "SELECT p.workflow_id, p.conversation_id, p.contract_id, p.delivery_id, p.receipt_id
                     FROM wake_terminal_receipts p
                     JOIN workflow_deliveries d
                       ON d.workflow_id = p.workflow_id AND d.delivery_id = p.delivery_id
                     WHERE d.status = 'Pending'
                     ORDER BY p.workflow_id, p.delivery_id
                     LIMIT ?1",
                )
                .bind(to_i64(limit as u64, "limit")?)
                .fetch_all(&mut *tx.tx)
                .await?
            }
        };
        let out = rows
            .into_iter()
            .map(|row| {
                Ok(WakePendingGlobalRow {
                    workflow_id: WorkflowId(to_u64(
                        row.get::<i64, _>("workflow_id"),
                        "workflow_id",
                    )?),
                    conversation_id: row.get("conversation_id"),
                    contract_id: row.get("contract_id"),
                    delivery_id: DeliveryId(to_u64(
                        row.get::<i64, _>("delivery_id"),
                        "delivery_id",
                    )?),
                    receipt_id: ReceiptId(to_u64(row.get::<i64, _>("receipt_id"), "receipt_id")?),
                })
            })
            .collect::<DbResult<Vec<_>>>()?;
        tx.commit().await?;
        Ok(out)
    }

    pub async fn list_active_unresolved(
        &self,
        limit: usize,
    ) -> DbResult<Vec<WakeActiveUnresolvedRow>> {
        let mut tx = self.workflow_repo.begin_tx().await?;
        let rows = sqlx::query(
            "SELECT b.workflow_id, b.conversation_id, b.contract_id, b.expires_at
             FROM wake_bindings b
             JOIN workflows w ON w.workflow_id = b.workflow_id
             WHERE w.status = 'Active'
               AND NOT EXISTS (SELECT 1 FROM wake_terminal_receipts p WHERE p.workflow_id = b.workflow_id)
             ORDER BY b.workflow_id
             LIMIT ?1",
        )
        .bind(to_i64(limit as u64, "limit")?)
        .fetch_all(&mut *tx.tx)
        .await?;
        let out = rows
            .into_iter()
            .map(|row| parse_active_unresolved_row(&row))
            .collect::<DbResult<Vec<_>>>()?;
        tx.commit().await?;
        Ok(out)
    }

    pub async fn list_active_unresolved_for_conversation(
        &self,
        conversation_id: &str,
    ) -> DbResult<Vec<WakeActiveUnresolvedRow>> {
        let mut tx = self.workflow_repo.begin_tx().await?;
        let rows = sqlx::query(
            "SELECT b.workflow_id, b.conversation_id, b.contract_id, b.expires_at
             FROM wake_bindings b
             JOIN workflows w ON w.workflow_id = b.workflow_id
             WHERE b.conversation_id = ?1
               AND w.status = 'Active'
               AND NOT EXISTS (SELECT 1 FROM wake_terminal_receipts p WHERE p.workflow_id = b.workflow_id)
             ORDER BY b.workflow_id",
        )
        .bind(conversation_id)
        .fetch_all(&mut *tx.tx)
        .await?;
        let out = rows
            .into_iter()
            .map(|row| parse_active_unresolved_row(&row))
            .collect::<DbResult<Vec<_>>>()?;
        tx.commit().await?;
        Ok(out)
    }

    pub async fn has_owed_work_for_conversation(&self, conversation_id: &str) -> DbResult<bool> {
        let mut tx = self.workflow_repo.begin_tx().await?;
        let owed = sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS (
                SELECT 1 FROM wake_bindings b
                JOIN workflows w ON w.workflow_id = b.workflow_id
                WHERE b.conversation_id = ?1
                  AND (
                    (w.status = 'Active' AND b.resolved_at IS NULL)
                    OR EXISTS (
                        SELECT 1 FROM workflow_deliveries d
                        WHERE d.workflow_id = b.workflow_id
                          AND (d.status = 'Pending' OR d.runtime_acceptance_status = 'Owed')
                    )
                  )
             )",
        )
        .bind(conversation_id)
        .fetch_one(&mut *tx.tx)
        .await?
            != 0;
        tx.commit().await?;
        Ok(owed)
    }

    pub async fn list_observation_candidates(
        &self,
        now: Timestamp,
        after_workflow_id: Option<WorkflowId>,
        limit: usize,
    ) -> DbResult<Vec<WakeObservationCandidateRow>> {
        let mut tx = self.workflow_repo.begin_tx().await?;
        let rows = sqlx::query(
            "SELECT b.workflow_id, b.conversation_id, b.contract_id, b.expires_at,
                    CASE
                      WHEN EXISTS (
                        SELECT 1 FROM workflow_attempts a
                        WHERE a.workflow_id = b.workflow_id
                          AND a.status IN ('Begun', 'ObservationRecorded')
                      ) THEN 'ExpiredLease'
                      ELSE 'NoLiveAttempt'
                    END AS candidate_reason
             FROM wake_bindings b
             JOIN workflows w ON w.workflow_id = b.workflow_id
             WHERE w.status = 'Active'
               AND NOT EXISTS (SELECT 1 FROM wake_terminal_receipts p WHERE p.workflow_id = b.workflow_id)
               AND (
                    NOT EXISTS (
                        SELECT 1 FROM workflow_attempts a
                        WHERE a.workflow_id = b.workflow_id
                          AND a.status IN ('Begun', 'ObservationRecorded')
                    )
                    OR EXISTS (
                        SELECT 1
                        FROM workflow_attempts a
                        JOIN workflow_reclaimable_leases l
                          ON l.workflow_id = a.workflow_id AND l.attempt_id = a.attempt_id
                        WHERE a.workflow_id = b.workflow_id
                          AND a.status IN ('Begun', 'ObservationRecorded')
                          AND l.lease_until <= ?1
                    )
               )
               AND b.workflow_id > ?2
             ORDER BY b.workflow_id
             LIMIT ?3",
        )
        .bind(to_i64(now.0, "now")?)
        .bind(to_i64(after_workflow_id.map_or(0, |id| id.0), "after_workflow_id")?)
        .bind(to_i64(limit as u64, "limit")?)
        .fetch_all(&mut *tx.tx)
        .await?;
        let out = rows
            .into_iter()
            .map(|row| {
                let reason = match row.get::<String, _>("candidate_reason").as_str() {
                    "NoLiveAttempt" => WakeObservationCandidateReason::NoLiveAttempt,
                    "ExpiredLease" => WakeObservationCandidateReason::ExpiredLease,
                    other => {
                        return Err(DbError::Serialization(format!(
                            "unknown wake observation candidate reason: {other}"
                        )))
                    }
                };
                Ok(WakeObservationCandidateRow {
                    workflow_id: WorkflowId(to_u64(
                        row.get::<i64, _>("workflow_id"),
                        "workflow_id",
                    )?),
                    conversation_id: row.get("conversation_id"),
                    contract_id: row.get("contract_id"),
                    reason,
                    expires_at: Timestamp(to_u64(row.get::<i64, _>("expires_at"), "expires_at")?),
                })
            })
            .collect::<DbResult<Vec<_>>>()?;
        tx.commit().await?;
        Ok(out)
    }

    pub async fn list_expired_unresolved(
        &self,
        now: Timestamp,
        limit: usize,
    ) -> DbResult<Vec<WakeExpiredUnresolvedRow>> {
        let mut tx = self.workflow_repo.begin_tx().await?;
        let rows = sqlx::query(
            "SELECT b.workflow_id, b.conversation_id, b.contract_id, b.expires_at
             FROM wake_bindings b
             JOIN workflows w ON w.workflow_id = b.workflow_id
             WHERE w.status = 'Active'
               AND b.expires_at <= ?1
               AND NOT EXISTS (SELECT 1 FROM wake_terminal_receipts p WHERE p.workflow_id = b.workflow_id)
             ORDER BY b.workflow_id
             LIMIT ?2",
        )
        .bind(to_i64(now.0, "now")?)
        .bind(to_i64(limit as u64, "limit")?)
        .fetch_all(&mut *tx.tx)
        .await?;
        let out = rows
            .into_iter()
            .map(|row| {
                Ok(WakeExpiredUnresolvedRow {
                    workflow_id: WorkflowId(to_u64(
                        row.get::<i64, _>("workflow_id"),
                        "workflow_id",
                    )?),
                    conversation_id: row.get("conversation_id"),
                    contract_id: row.get("contract_id"),
                    expires_at: Timestamp(to_u64(row.get::<i64, _>("expires_at"), "expires_at")?),
                })
            })
            .collect::<DbResult<Vec<_>>>()?;
        tx.commit().await?;
        Ok(out)
    }

    pub async fn list_pending(&self, conversation_id: &str) -> DbResult<Vec<WakePendingDelivery>> {
        let mut tx = self.workflow_repo.begin_tx().await?;
        let out = fetch_pending_deliveries_for_conversation_tx(&mut tx, conversation_id).await?;
        tx.commit().await?;
        Ok(out)
    }

    pub async fn get_pending_exact(
        &self,
        workflow_id: WorkflowId,
        delivery_id: DeliveryId,
        conversation_id: &str,
    ) -> DbResult<Option<WakePendingDelivery>> {
        let mut tx = self.workflow_repo.begin_tx().await?;
        let out =
            fetch_pending_delivery_exact_tx(&mut tx, workflow_id, delivery_id, conversation_id)
                .await?;
        tx.commit().await?;
        Ok(out)
    }
    pub async fn materialize_pending_delivery_message(
        &self,
        input: &MaterializePendingDeliveryMessageInput,
    ) -> DbResult<MaterializePendingDeliveryMessageOutcome> {
        for _ in 0..20 {
            match self.materialize_pending_delivery_message_once(input).await {
                Err(DbError::Sqlx(sqlx::Error::Database(error)))
                    if super::is_sqlite_busy_retryable(error.as_ref()) =>
                {
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
                Err(DbError::Sqlx(sqlx::Error::Database(error)))
                    if is_unique_or_primary_constraint(error.as_ref()) =>
                {
                    if let Some(existing) = self
                        .get_delivery_message_link(input.workflow_id, input.delivery_id)
                        .await?
                    {
                        return Ok(
                            MaterializePendingDeliveryMessageOutcome::AlreadyMaterialized(existing),
                        );
                    }
                }
                result => return result,
            }
        }
        self.materialize_pending_delivery_message_once(input).await
    }

    async fn materialize_pending_delivery_message_once(
        &self,
        input: &MaterializePendingDeliveryMessageInput,
    ) -> DbResult<MaterializePendingDeliveryMessageOutcome> {
        let mut eligibility_tx = self.workflow_repo.begin_tx().await?;
        let eligible = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM wake_bindings b
             JOIN conversations c ON c.id = b.conversation_id
             WHERE b.workflow_id = ?1 AND b.conversation_id = ?2 AND c.archived = 0",
        )
        .bind(to_i64(input.workflow_id.0, "workflow_id")?)
        .bind(&input.conversation_id)
        .fetch_one(&mut *eligibility_tx.tx)
        .await?
            > 0;
        eligibility_tx.commit().await?;
        if !eligible {
            return Ok(MaterializePendingDeliveryMessageOutcome::WrongOwnerOrIneligible);
        }

        if let Some(existing) = self
            .get_delivery_message_link(input.workflow_id, input.delivery_id)
            .await?
        {
            return Ok(MaterializePendingDeliveryMessageOutcome::AlreadyMaterialized(existing));
        }

        let mut tx = self.workflow_repo.begin_tx().await?;
        if let Some(existing) =
            fetch_delivery_message_link_tx(&mut tx, input.workflow_id, input.delivery_id).await?
        {
            tx.commit().await?;
            return Ok(MaterializePendingDeliveryMessageOutcome::AlreadyMaterialized(existing));
        }

        let candidate = sqlx::query(
            "SELECT d.workflow_id, d.delivery_id, d.effect_id, d.barrier_id, d.consumer_kind,
                    d.event_codec_family, d.event_codec_version, d.payload_kind, d.payload_blob,
                    d.requires_runtime_acceptance, d.status, d.runtime_acceptance_status,
                    d.suppression_reason, d.accepted_by_transition_id,
                    p.receipt_id, p.conversation_id, p.contract_id, p.resource_kind, p.terminal_kind,
                    p.resolved_at, p.bash_handle_id, p.tmux_server_token, p.tmux_window_id,
                    p.bash_status, p.tmux_status, p.occurred_at, p.exit_code, p.duration_ms,
                    p.signal_number, p.kill_signal_sent, p.forgotten_reason, p.cancelled_reason,
                    p.cancelled_at, b.scope_kind, b.scope_stable_key, b.tmux_completion_policy, b.registering_tool_use_id
             FROM workflow_deliveries d
             JOIN wake_terminal_receipts p
               ON p.workflow_id = d.workflow_id AND p.delivery_id = d.delivery_id
             JOIN wake_bindings b ON b.workflow_id = p.workflow_id
             JOIN conversations c ON c.id = p.conversation_id
             WHERE d.workflow_id = ?1 AND d.delivery_id = ?2 AND d.status = 'Pending'
               AND p.conversation_id = ?3 AND c.archived = 0"
        )
        .bind(to_i64(input.workflow_id.0, "workflow_id")?)
        .bind(to_i64(input.delivery_id.0, "delivery_id")?)
        .bind(&input.conversation_id)
        .fetch_optional(&mut *tx.tx)
        .await?;
        let Some(row) = candidate else {
            tx.rollback().await?;
            return Ok(MaterializePendingDeliveryMessageOutcome::WrongOwnerOrIneligible);
        };

        let registering_tool_use_id: String = row.get("registering_tool_use_id");
        let terminal_kind: String = row.get("terminal_kind");
        let message_id = wake_delivery_message_id(input.workflow_id, input.delivery_id);
        let sequence_id = match input.sequence_id {
            Some(sequence_id) => sequence_id,
            None => sqlx::query_scalar::<_, i64>(
                "SELECT COALESCE(MAX(sequence_id), 0) + 1 FROM messages WHERE conversation_id = ?1",
            )
            .bind(&input.conversation_id)
            .fetch_one(&mut *tx.tx)
            .await?,
        };
        let content = MessageContent::User(UserContent::meta(input.rendered_content.clone()));
        let content_str = serde_json::to_string(&content.to_stored_json())
            .map_err(|e| DbError::Serialization(e.to_string()))?;
        let display_str = input
            .display_data
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| DbError::Serialization(e.to_string()))?;
        let created_at = timestamp_to_datetime(input.created_at);
        sqlx::query(
            "INSERT INTO messages (message_id, conversation_id, sequence_id, message_type, content, display_data, usage_data, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7)",
        )
        .bind(&message_id)
        .bind(&input.conversation_id)
        .bind(sequence_id)
        .bind(content.message_type().to_string())
        .bind(&content_str)
        .bind(&display_str)
        .bind(created_at.to_rfc3339())
        .execute(&mut *tx.tx)
        .await?;
        sqlx::query("UPDATE conversations SET updated_at = ?1 WHERE id = ?2")
            .bind(created_at.to_rfc3339())
            .bind(&input.conversation_id)
            .execute(&mut *tx.tx)
            .await?;
        let message = Message {
            message_id: message_id.clone(),
            conversation_id: input.conversation_id.clone(),
            sequence_id,
            message_type: content.message_type(),
            content,
            display_data: input.display_data.clone(),
            usage_data: None,
            created_at,
        };
        if has_message_fts_tx(&mut tx).await? {
            crate::retrieval::fts_upsert_conn(&mut tx.tx, &message).await?;
        }
        sqlx::query(
            "INSERT INTO wake_delivery_messages (
                workflow_id, delivery_id, conversation_id, message_id, registering_tool_use_id,
                terminal_kind, auto_resume, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )
        .bind(to_i64(input.workflow_id.0, "workflow_id")?)
        .bind(to_i64(input.delivery_id.0, "delivery_id")?)
        .bind(&input.conversation_id)
        .bind(&message_id)
        .bind(&registering_tool_use_id)
        .bind(&terminal_kind)
        .bind(input.auto_resume)
        .bind(to_i64(input.created_at.0, "created_at")?)
        .execute(&mut *tx.tx)
        .await?;
        let linked = fetch_delivery_message_link_tx(&mut tx, input.workflow_id, input.delivery_id)
            .await?
            .ok_or_else(|| {
                DbError::Serialization(
                    "wake delivery message link missing after insert".to_string(),
                )
            })?;
        tx.commit().await?;
        Ok(MaterializePendingDeliveryMessageOutcome::Materialized(
            linked,
        ))
    }

    pub async fn get_delivery_message_link(
        &self,
        workflow_id: WorkflowId,
        delivery_id: DeliveryId,
    ) -> DbResult<Option<WakeDeliveryMessageLink>> {
        let mut tx = self.workflow_repo.begin_tx().await?;
        let out = fetch_delivery_message_link_tx(&mut tx, workflow_id, delivery_id).await?;
        tx.commit().await?;
        Ok(out)
    }

    pub async fn list_linked_pending_delivery_messages(
        &self,
        conversation_id: &str,
    ) -> DbResult<Vec<WakeDeliveryMessageLink>> {
        let mut tx = self.workflow_repo.begin_tx().await?;
        let rows = sqlx::query(
            "SELECT l.workflow_id, l.delivery_id, l.conversation_id AS link_conversation_id,
                    l.message_id AS link_message_id, l.registering_tool_use_id, l.terminal_kind,
                    l.auto_resume, l.created_at AS link_created_at,
                    m.message_id, m.conversation_id, m.sequence_id, m.message_type, m.content,
                    m.display_data, m.usage_data, m.created_at
             FROM wake_delivery_messages l
             JOIN workflow_deliveries d
               ON d.workflow_id = l.workflow_id AND d.delivery_id = l.delivery_id
             JOIN messages m ON m.message_id = l.message_id
             WHERE l.conversation_id = ?1 AND d.status = 'Pending'
             ORDER BY l.delivery_id",
        )
        .bind(conversation_id)
        .fetch_all(&mut *tx.tx)
        .await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            out.push(delivery_message_link_from_join_row(&row)?);
        }
        tx.commit().await?;
        Ok(out)
    }

    pub async fn list_materialized_pending_for_workflow(
        &self,
        workflow_id: WorkflowId,
    ) -> DbResult<Vec<WakeMaterializedPendingDelivery>> {
        let mut tx = self.workflow_repo.begin_tx().await?;
        let out = fetch_materialized_pending_deliveries_tx(&mut tx, workflow_id).await?;
        tx.commit().await?;
        Ok(out)
    }

    async fn list_workflows_owed_to_conversation(
        &self,
        conversation_id: &str,
    ) -> DbResult<Vec<WorkflowId>> {
        let mut tx = self.workflow_repo.begin_tx().await?;
        let rows = sqlx::query_scalar::<_, i64>(
            "SELECT DISTINCT b.workflow_id
             FROM wake_bindings b
             LEFT JOIN workflow_deliveries d ON d.workflow_id = b.workflow_id
             WHERE b.conversation_id = ?1
               AND (b.resolved_at IS NULL OR d.status = 'Pending')
             ORDER BY b.workflow_id",
        )
        .bind(conversation_id)
        .fetch_all(&mut *tx.tx)
        .await?;
        tx.commit().await?;
        rows.into_iter()
            .map(|value| Ok(WorkflowId(to_u64(value, "workflow_id")?)))
            .collect()
    }

    pub async fn reconcile_continuation_transfers(&self, timestamp: Timestamp) -> DbResult<usize> {
        let mut tx = self.workflow_repo.begin_tx().await?;
        let predecessors = sqlx::query_scalar::<_, String>(
            "SELECT c.id
             FROM conversations c
             WHERE c.continued_in_conv_id IS NOT NULL
               AND EXISTS (
                   SELECT 1 FROM wake_bindings b
                   LEFT JOIN workflow_deliveries d ON d.workflow_id = b.workflow_id
                   WHERE b.conversation_id = c.id
                     AND (b.resolved_at IS NULL OR d.status = 'Pending')
               )
             ORDER BY c.id",
        )
        .fetch_all(&mut *tx.tx)
        .await?;
        tx.commit().await?;

        let mut repaired = 0;
        for predecessor in predecessors {
            if self
                .recover_continuation_transfer(&predecessor, timestamp)
                .await?
            {
                repaired += 1;
            }
        }
        Ok(repaired)
    }

    pub async fn recover_continuation_transfer(
        &self,
        from_conversation_id: &str,
        timestamp: Timestamp,
    ) -> DbResult<bool> {
        let mut tx = self.workflow_repo.begin_tx().await?;
        let continuation = sqlx::query_scalar::<_, String>(
            "WITH RECURSIVE continuation_chain(id, continued_in_conv_id, depth) AS (
                 SELECT id, continued_in_conv_id, 0 FROM conversations WHERE id = ?1
                 UNION ALL
                 SELECT c.id, c.continued_in_conv_id, chain.depth + 1
                 FROM conversations c
                 JOIN continuation_chain chain ON c.id = chain.continued_in_conv_id
                 WHERE chain.depth < 100
             )
             SELECT id FROM continuation_chain
             WHERE continued_in_conv_id IS NULL AND depth > 0
             ORDER BY depth DESC LIMIT 1",
        )
        .bind(from_conversation_id)
        .fetch_optional(&mut *tx.tx)
        .await?;
        tx.commit().await?;
        let Some(continuation) = continuation else {
            return Ok(false);
        };
        self.transfer_active_for_continuation(from_conversation_id, &continuation, timestamp)
            .await?;
        Ok(true)
    }

    pub async fn transfer_active_for_continuation(
        &self,
        from_conversation_id: &str,
        to_conversation_id: &str,
        timestamp: Timestamp,
    ) -> DbResult<()> {
        let owed = self
            .list_workflows_owed_to_conversation(from_conversation_id)
            .await?;
        for workflow_id in owed {
            for _ in 0..20 {
                let mut tx = self.workflow_repo.begin_tx().await?;
                let Some(head) = tx.fetch_workflow_head(workflow_id).await? else {
                    tx.rollback().await?;
                    break;
                };
                let pending_delivery_ids =
                    fetch_pending_terminal_delivery_ids_tx(&mut tx, workflow_id).await?;
                tx.rollback().await?;
                let input = WakeTransferInput {
                    workflow_id,
                    from_conversation_id: from_conversation_id.to_string(),
                    to_conversation_id: to_conversation_id.to_string(),
                    expected_version: head.version,
                    exact_pending_delivery_ids: pending_delivery_ids,
                    transition_id: TransitionId(head.version.next().0),
                    timestamp,
                };
                match self.transfer(&input).await? {
                    WakeTransferOutcome::Transferred | WakeTransferOutcome::OwnerMismatch => break,
                    WakeTransferOutcome::VersionConflict | WakeTransferOutcome::SetMismatch => {}
                }
            }
        }
        Ok(())
    }

    pub async fn transfer(&self, input: &WakeTransferInput) -> DbResult<WakeTransferOutcome> {
        for _ in 0..5 {
            match self.transfer_once(input).await {
                Err(DbError::Sqlx(sqlx::Error::Database(error)))
                    if super::is_sqlite_busy_retryable(error.as_ref()) =>
                {
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
                result => return result,
            }
        }
        self.transfer_once(input).await
    }

    async fn transfer_once(&self, input: &WakeTransferInput) -> DbResult<WakeTransferOutcome> {
        let mut tx = self.workflow_repo.begin_tx().await?;
        let to_exists =
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM conversations WHERE id = ?1")
                .bind(&input.to_conversation_id)
                .fetch_one(&mut *tx.tx)
                .await?;
        if to_exists == 0 {
            tx.rollback().await?;
            return Ok(WakeTransferOutcome::OwnerMismatch);
        }
        let Some(binding) = fetch_binding_by_workflow_tx(&mut tx, input.workflow_id).await? else {
            tx.rollback().await?;
            return Ok(WakeTransferOutcome::OwnerMismatch);
        };
        if binding.conversation_id != input.from_conversation_id {
            tx.rollback().await?;
            return Ok(WakeTransferOutcome::OwnerMismatch);
        }
        let Some(head) = tx.fetch_workflow_head(input.workflow_id).await? else {
            tx.rollback().await?;
            return Ok(WakeTransferOutcome::OwnerMismatch);
        };
        if head.version != input.expected_version {
            tx.rollback().await?;
            return Ok(WakeTransferOutcome::VersionConflict);
        }
        let current_pending_ids =
            fetch_pending_terminal_delivery_ids_tx(&mut tx, input.workflow_id).await?;
        if current_pending_ids != input.exact_pending_delivery_ids {
            tx.rollback().await?;
            return Ok(WakeTransferOutcome::SetMismatch);
        }

        let event = WakeRegistrationEvent::OwnershipTransferred {
            from_conversation_id: input.from_conversation_id.clone(),
            to_conversation_id: input.to_conversation_id.clone(),
            pending_delivery_ids: input.exact_pending_delivery_ids.clone(),
        };
        let event_codec = local_codec(&wake_profile::event_codec());
        let event_payload = json_blob(&event)?;
        let next_snapshot_codec = LocalCodec {
            family: wake_profile::snapshot_codec().family.to_string(),
            version: wake_profile::snapshot_codec().version,
        };
        let committed = tx
            .commit_transition_head_cas(
                input.workflow_id,
                input.expected_version,
                head.generation,
                head.status,
                &event_codec,
                &event_payload,
                &next_snapshot_codec,
                &head.snapshot_payload,
                input.transition_id,
                input.timestamp,
            )
            .await?;
        if !committed {
            tx.rollback().await?;
            return Ok(WakeTransferOutcome::VersionConflict);
        }

        sqlx::query(
            "UPDATE wake_bindings SET conversation_id = ?2 WHERE workflow_id = ?1 AND conversation_id = ?3",
        )
        .bind(to_i64(input.workflow_id.0, "workflow_id")?)
        .bind(&input.to_conversation_id)
        .bind(&input.from_conversation_id)
        .execute(&mut *tx.tx)
        .await?;

        if binding.registration_scope.kind == wake_types::WorkScopeKind::Conversation
            && binding.registration_scope.stable_key
                == format!("conversation:{}", input.from_conversation_id)
        {
            sqlx::query("UPDATE wake_bindings SET scope_stable_key = ?2 WHERE workflow_id = ?1")
                .bind(to_i64(input.workflow_id.0, "workflow_id")?)
                .bind(format!("conversation:{}", input.to_conversation_id))
                .execute(&mut *tx.tx)
                .await?;
        }

        #[cfg(test)]
        maybe_fail_after_transfer_binding_update(self.failpoint_namespace, input.workflow_id)?;

        for delivery_id in &input.exact_pending_delivery_ids {
            sqlx::query(
                "UPDATE wake_terminal_receipts
                 SET conversation_id = ?3
                 WHERE workflow_id = ?1 AND delivery_id = ?2 AND conversation_id = ?4
                   AND EXISTS (
                       SELECT 1 FROM workflow_deliveries d
                       WHERE d.workflow_id = wake_terminal_receipts.workflow_id
                         AND d.delivery_id = wake_terminal_receipts.delivery_id
                         AND d.status = 'Pending'
                   )",
            )
            .bind(to_i64(input.workflow_id.0, "workflow_id")?)
            .bind(to_i64(delivery_id.0, "delivery_id")?)
            .bind(&input.to_conversation_id)
            .bind(&input.from_conversation_id)
            .execute(&mut *tx.tx)
            .await?;

            sqlx::query(
                "UPDATE wake_delivery_messages
                 SET conversation_id = ?3
                 WHERE workflow_id = ?1 AND delivery_id = ?2 AND conversation_id = ?4",
            )
            .bind(to_i64(input.workflow_id.0, "workflow_id")?)
            .bind(to_i64(delivery_id.0, "delivery_id")?)
            .bind(&input.to_conversation_id)
            .bind(&input.from_conversation_id)
            .execute(&mut *tx.tx)
            .await?;

            sqlx::query(
                "UPDATE messages
                 SET conversation_id = ?3
                 WHERE message_id IN (
                     SELECT l.message_id
                     FROM wake_delivery_messages l
                     WHERE l.workflow_id = ?1 AND l.delivery_id = ?2 AND l.conversation_id = ?3
                 )
                   AND conversation_id = ?4",
            )
            .bind(to_i64(input.workflow_id.0, "workflow_id")?)
            .bind(to_i64(delivery_id.0, "delivery_id")?)
            .bind(&input.to_conversation_id)
            .bind(&input.from_conversation_id)
            .execute(&mut *tx.tx)
            .await?;

            if has_message_fts_tx(&mut tx).await? {
                sqlx::query(
                    "UPDATE message_fts
                     SET conversation_id = ?3
                     WHERE message_id IN (
                         SELECT l.message_id FROM wake_delivery_messages l
                         WHERE l.workflow_id = ?1 AND l.delivery_id = ?2 AND l.conversation_id = ?3
                     )",
                )
                .bind(to_i64(input.workflow_id.0, "workflow_id")?)
                .bind(to_i64(delivery_id.0, "delivery_id")?)
                .bind(&input.to_conversation_id)
                .execute(&mut *tx.tx)
                .await?;
            }
        }

        tx.commit().await?;
        Ok(WakeTransferOutcome::Transferred)
    }

    pub async fn resolve_materialized_pending_for_workflow(
        &self,
        workflow_id: WorkflowId,
        decision: WakeResolveMaterializedDecision,
        timestamp: Timestamp,
    ) -> DbResult<Result<WakeResolveMaterializedPendingOutcome, WakeResolveMaterializedPendingError>>
    {
        for _ in 0..20 {
            match self
                .resolve_materialized_pending_for_workflow_once(workflow_id, decision, timestamp)
                .await
            {
                Err(DbError::Sqlx(sqlx::Error::Database(error)))
                    if super::is_sqlite_busy_retryable(error.as_ref()) =>
                {
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
                Ok(ResolveMaterializedPendingAttempt::RetryVersionConflict) => {}
                Ok(ResolveMaterializedPendingAttempt::Done(outcome)) => return Ok(outcome),
                Err(error) => return Err(error),
            }
        }
        Err(DbError::Serialization(
            "wake materialized delivery resolution exhausted concurrent retry budget".to_string(),
        ))
    }

    pub async fn adopt_materialized_pending_for_conversation(
        &self,
        conversation_id: &str,
        timestamp: Timestamp,
    ) -> DbResult<WakeAdoptMaterializedPendingOutcome> {
        for _ in 0..20 {
            match self
                .adopt_materialized_pending_for_conversation_once(conversation_id, timestamp)
                .await
            {
                Err(DbError::Sqlx(sqlx::Error::Database(error)))
                    if super::is_sqlite_busy_retryable(error.as_ref()) =>
                {
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
                result => return result,
            }
        }
        self.adopt_materialized_pending_for_conversation_once(conversation_id, timestamp)
            .await
    }

    pub async fn suppress_pending_for_archived_conversation(
        &self,
        pending: &WakePendingDelivery,
        timestamp: Timestamp,
    ) -> DbResult<()> {
        let mut tx = self.workflow_repo.begin_tx().await?;
        let archived =
            sqlx::query_scalar::<_, i64>("SELECT archived FROM conversations WHERE id = ?1")
                .bind(&pending.conversation_id)
                .fetch_optional(&mut *tx.tx)
                .await?
                .unwrap_or_default()
                != 0;
        let head = tx.fetch_workflow_head(pending.workflow_id).await?;
        tx.rollback().await?;
        if archived {
            if let Some(head) = head {
                let _ = self
                    .resolve_pending_exact(&WakeResolvePendingInput {
                        workflow_id: pending.workflow_id,
                        expected_version: head.version,
                        delivery_ids: vec![pending.receipt.delivery_id],
                        decision: WakeResolveDecision::Suppress,
                        transition_id: TransitionId(head.version.next().0),
                        timestamp,
                    })
                    .await?;
            }
        }
        Ok(())
    }

    pub async fn resolve_pending_exact(
        &self,
        input: &WakeResolvePendingInput,
    ) -> DbResult<WakeResolvePendingOutcome> {
        for _ in 0..20 {
            match self.resolve_pending_exact_once(input).await {
                Err(DbError::Sqlx(sqlx::Error::Database(error)))
                    if super::is_sqlite_busy_retryable(error.as_ref()) =>
                {
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
                result => return result,
            }
        }
        self.resolve_pending_exact_once(input).await
    }

    async fn resolve_pending_exact_once(
        &self,
        input: &WakeResolvePendingInput,
    ) -> DbResult<WakeResolvePendingOutcome> {
        let mut tx = self.workflow_repo.begin_tx().await?;
        let Some(_binding) = fetch_binding_by_workflow_tx(&mut tx, input.workflow_id).await? else {
            tx.rollback().await?;
            return Ok(WakeResolvePendingOutcome::SetMismatch);
        };
        let Some(head) = tx.fetch_workflow_head(input.workflow_id).await? else {
            tx.rollback().await?;
            return Ok(WakeResolvePendingOutcome::SetMismatch);
        };
        if head.version != input.expected_version {
            tx.rollback().await?;
            return Ok(WakeResolvePendingOutcome::VersionConflict);
        }

        let current = sqlx::query(
            "SELECT d.delivery_id, d.status
             FROM workflow_deliveries d
             JOIN wake_terminal_receipts p
               ON p.workflow_id = d.workflow_id AND p.delivery_id = d.delivery_id
             WHERE d.workflow_id = ?1
             ORDER BY d.delivery_id",
        )
        .bind(to_i64(input.workflow_id.0, "workflow_id")?)
        .fetch_all(&mut *tx.tx)
        .await?;

        let mut pending_ids = Vec::new();
        let mut resolved_requested = false;
        let mut requested_counts = std::collections::BTreeMap::new();
        for &delivery_id in &input.delivery_ids {
            *requested_counts.entry(delivery_id).or_insert(0_usize) += 1;
        }
        if requested_counts.values().any(|count| *count > 1) {
            tx.rollback().await?;
            return Ok(WakeResolvePendingOutcome::SetMismatch);
        }
        for row in &current {
            let delivery_id = DeliveryId(to_u64(row.get::<i64, _>("delivery_id"), "delivery_id")?);
            let status = row.get::<String, _>("status");
            if status == "Pending" {
                pending_ids.push(delivery_id);
            } else if requested_counts.contains_key(&delivery_id) {
                resolved_requested = true;
            }
        }
        if resolved_requested {
            tx.rollback().await?;
            return Ok(WakeResolvePendingOutcome::AlreadyResolved);
        }
        if pending_ids != input.delivery_ids {
            tx.rollback().await?;
            return Ok(WakeResolvePendingOutcome::SetMismatch);
        }

        let projection = fetch_any_terminal_projection_tx(&mut tx, input.workflow_id)
            .await?
            .ok_or_else(|| {
                DbError::Serialization("wake pending delivery set missing projection".to_string())
            })?;
        let terminal = projection.terminal;
        let event = match input.decision {
            WakeResolveDecision::Accept => WakeRegistrationEvent::RuntimeAccepted {
                terminal: Box::new(terminal.clone()),
            },
            WakeResolveDecision::Suppress => WakeRegistrationEvent::RuntimeSuppressed {
                terminal: Box::new(terminal.clone()),
            },
        };
        let decision = match input.decision {
            WakeResolveDecision::Accept => DeliveryResolutionDecision::Accept,
            WakeResolveDecision::Suppress => DeliveryResolutionDecision::Suppress {
                reason: phoenix_workflow::SuppressionReason::ReducerTerminal,
            },
        };
        let event_codec = local_codec(&wake_profile::event_codec());
        let event_payload = json_blob(&event)?;
        let mut snapshot: WakeRegistrationSnapshot = serde_json::from_slice(&head.snapshot_payload)
            .map_err(|e| DbError::Serialization(e.to_string()))?;
        snapshot.runtime_availability = match input.decision {
            WakeResolveDecision::Accept => wake_profile::RuntimeAvailability::Accepted,
            WakeResolveDecision::Suppress => wake_profile::RuntimeAvailability::Suppressed,
        };
        let next_snapshot_payload = json_blob(&snapshot)?;
        let next_snapshot_codec = LocalCodec {
            family: wake_profile::snapshot_codec().family.to_string(),
            version: wake_profile::snapshot_codec().version,
        };
        let outcome = tx
            .resolve_deliveries_exact(DeliveryResolutionPlan {
                workflow_id: input.workflow_id,
                expected_version: input.expected_version,
                transition_id: input.transition_id,
                generation: head.generation,
                next_status: head.status,
                event_codec: &event_codec,
                event_payload: &event_payload,
                next_snapshot_codec: &next_snapshot_codec,
                next_snapshot_payload: &next_snapshot_payload,
                committed_at: input.timestamp,
                exact_delivery_ids: &input.delivery_ids,
                decision,
            })
            .await?;
        match outcome {
            CommitOutcome::Committed => {
                #[cfg(test)]
                maybe_fail_after_canonical_transition(self.failpoint_namespace, input.workflow_id)?;
                tx.commit().await?;
                Ok(WakeResolvePendingOutcome::Resolved)
            }
            CommitOutcome::VersionConflict => {
                tx.rollback().await?;
                Ok(WakeResolvePendingOutcome::VersionConflict)
            }
            CommitOutcome::InvalidPlan => {
                tx.rollback().await?;
                let head = self
                    .workflow_repo
                    .fetch_workflow_head(input.workflow_id)
                    .await?
                    .ok_or_else(|| {
                        DbError::Serialization(
                            "wake resolve missing workflow after invalid plan".to_string(),
                        )
                    })?;
                if head.version != input.expected_version {
                    return Ok(WakeResolvePendingOutcome::VersionConflict);
                }
                let current = sqlx::query(
                    "SELECT d.delivery_id, d.status
                     FROM workflow_deliveries d
                     JOIN wake_terminal_receipts p
                       ON p.workflow_id = d.workflow_id AND p.delivery_id = d.delivery_id
                     WHERE d.workflow_id = ?1
                     ORDER BY d.delivery_id",
                )
                .bind(to_i64(input.workflow_id.0, "workflow_id")?)
                .fetch_all(&self.workflow_repo.pool)
                .await?;
                if current.iter().any(|row| {
                    let delivery_id = DeliveryId(
                        to_u64(row.get::<i64, _>("delivery_id"), "delivery_id").unwrap(),
                    );
                    row.get::<String, _>("status") != "Pending"
                        && input.delivery_ids.contains(&delivery_id)
                }) {
                    Ok(WakeResolvePendingOutcome::AlreadyResolved)
                } else {
                    Ok(WakeResolvePendingOutcome::SetMismatch)
                }
            }
            CommitOutcome::UnsupportedCodec => {
                tx.rollback().await?;
                Err(DbError::Serialization(
                    "wake resolve returned unexpected codec error".to_string(),
                ))
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ResolveMaterializedPendingAttempt {
    RetryVersionConflict,
    Done(Result<WakeResolveMaterializedPendingOutcome, WakeResolveMaterializedPendingError>),
}

impl WakeRepository {
    async fn adopt_materialized_pending_for_conversation_once(
        &self,
        conversation_id: &str,
        timestamp: Timestamp,
    ) -> DbResult<WakeAdoptMaterializedPendingOutcome> {
        let mut tx = self.workflow_repo.begin_tx().await?;
        let Some(conversation_row) =
            sqlx::query("SELECT state, archived FROM conversations WHERE id = ?1")
                .bind(conversation_id)
                .fetch_optional(&mut *tx.tx)
                .await?
        else {
            tx.rollback().await?;
            return Ok(WakeAdoptMaterializedPendingOutcome::NothingPending);
        };
        if conversation_row.get::<i64, _>("archived") != 0 {
            tx.rollback().await?;
            return Ok(WakeAdoptMaterializedPendingOutcome::NothingPending);
        }
        let state_json = conversation_row.get::<String, _>("state");
        let state: ConvState = serde_json::from_str(&state_json)
            .map_err(|error| DbError::Serialization(error.to_string()))?;
        if !matches!(state, ConvState::Idle) {
            tx.rollback().await?;
            return Ok(WakeAdoptMaterializedPendingOutcome::Busy(Box::new(state)));
        }

        let batches =
            fetch_materialized_pending_batches_for_conversation_tx(&mut tx, conversation_id)
                .await?;
        if batches.is_empty() {
            tx.rollback().await?;
            return Ok(WakeAdoptMaterializedPendingOutcome::NothingPending);
        }

        let mut missing_ids = Vec::new();
        for (_workflow_id, pending_ids, materialized) in &batches {
            let materialized_ids: std::collections::BTreeSet<_> = materialized
                .iter()
                .map(|item| item.pending.canonical_delivery.delivery_id)
                .collect();
            missing_ids.extend(
                pending_ids
                    .iter()
                    .copied()
                    .filter(|delivery_id| !materialized_ids.contains(delivery_id)),
            );
        }
        if !missing_ids.is_empty() {
            tx.rollback().await?;
            return Ok(WakeAdoptMaterializedPendingOutcome::NotFullyMaterialized {
                delivery_ids: missing_ids,
            });
        }

        let mut links = Vec::new();
        let mut auto_resume = false;
        for (workflow_id, pending_ids, materialized) in batches {
            let Some(head) = tx.fetch_workflow_head(workflow_id).await? else {
                return Err(DbError::Serialization(
                    "wake materialized batch missing workflow head".to_string(),
                ));
            };
            let projection = materialized
                .first()
                .map(|item| &item.pending.receipt)
                .ok_or_else(|| {
                    DbError::Serialization(
                        "wake materialized batch missing terminal projection".to_string(),
                    )
                })?;
            let cancellation_only = materialized.iter().all(|item| {
                matches!(
                    item.pending.receipt.terminal,
                    WakeTerminalPayload::Cancelled { .. }
                )
            });
            let decision = if cancellation_only {
                WakeResolveMaterializedDecision::Suppress
            } else {
                WakeResolveMaterializedDecision::Accept
            };
            let event = match decision {
                WakeResolveMaterializedDecision::Accept => WakeRegistrationEvent::RuntimeAccepted {
                    terminal: Box::new(projection.terminal.clone()),
                },
                WakeResolveMaterializedDecision::Suppress => {
                    WakeRegistrationEvent::RuntimeSuppressed {
                        terminal: Box::new(projection.terminal.clone()),
                    }
                }
            };
            let resolution_decision = match decision {
                WakeResolveMaterializedDecision::Accept => DeliveryResolutionDecision::Accept,
                WakeResolveMaterializedDecision::Suppress => DeliveryResolutionDecision::Suppress {
                    reason: phoenix_workflow::SuppressionReason::ReducerTerminal,
                },
            };
            let mut snapshot: WakeRegistrationSnapshot =
                serde_json::from_slice(&head.snapshot_payload)
                    .map_err(|error| DbError::Serialization(error.to_string()))?;
            snapshot.runtime_availability = match decision {
                WakeResolveMaterializedDecision::Accept => {
                    wake_profile::RuntimeAvailability::Accepted
                }
                WakeResolveMaterializedDecision::Suppress => {
                    wake_profile::RuntimeAvailability::Suppressed
                }
            };
            let event_codec = local_codec(&wake_profile::event_codec());
            let event_payload = json_blob(&event)?;
            let snapshot_codec = LocalCodec {
                family: wake_profile::snapshot_codec().family.to_string(),
                version: wake_profile::snapshot_codec().version,
            };
            let snapshot_payload = json_blob(&snapshot)?;
            match tx
                .resolve_deliveries_exact(DeliveryResolutionPlan {
                    workflow_id,
                    expected_version: head.version,
                    transition_id: TransitionId(head.version.next().0),
                    generation: head.generation,
                    next_status: head.status,
                    event_codec: &event_codec,
                    event_payload: &event_payload,
                    next_snapshot_codec: &snapshot_codec,
                    next_snapshot_payload: &snapshot_payload,
                    committed_at: timestamp,
                    exact_delivery_ids: &pending_ids,
                    decision: resolution_decision,
                })
                .await?
            {
                CommitOutcome::Committed => {}
                CommitOutcome::VersionConflict | CommitOutcome::InvalidPlan => {
                    tx.rollback().await?;
                    return Ok(WakeAdoptMaterializedPendingOutcome::NothingPending);
                }
                CommitOutcome::UnsupportedCodec => {
                    return Err(DbError::Serialization(
                        "wake batch adoption returned unexpected codec error".to_string(),
                    ));
                }
            }
            auto_resume |= materialized.iter().any(|item| item.link.auto_resume);
            links.extend(materialized.into_iter().map(|item| item.link));
        }

        if auto_resume {
            let next_state = serde_json::to_string(&ConvState::LlmRequesting { attempt: 1 })
                .map_err(|error| DbError::Serialization(error.to_string()))?;
            let idle_state = serde_json::to_string(&ConvState::Idle)
                .map_err(|error| DbError::Serialization(error.to_string()))?;
            let updated_at = timestamp_to_datetime(timestamp).to_rfc3339();
            let updated = sqlx::query(
                "UPDATE conversations
                 SET state = ?1, state_updated_at = ?2, updated_at = ?2
                 WHERE id = ?3 AND state = ?4",
            )
            .bind(next_state)
            .bind(updated_at)
            .bind(conversation_id)
            .bind(idle_state)
            .execute(&mut *tx.tx)
            .await?;
            if updated.rows_affected() != 1 {
                tx.rollback().await?;
                return Ok(WakeAdoptMaterializedPendingOutcome::NothingPending);
            }
        }

        for link in &links {
            let mut display_data = link
                .linked_message
                .message
                .display_data
                .clone()
                .unwrap_or_else(|| serde_json::json!({}));
            if let serde_json::Value::Object(object) = &mut display_data {
                object.insert("adopted".to_string(), serde_json::Value::Bool(true));
            }
            sqlx::query("UPDATE messages SET display_data = ?1 WHERE message_id = ?2")
                .bind(
                    serde_json::to_string(&display_data)
                        .map_err(|error| DbError::Serialization(error.to_string()))?,
                )
                .bind(&link.linked_message.message.message_id)
                .execute(&mut *tx.tx)
                .await?;
        }

        tx.commit().await?;
        Ok(WakeAdoptMaterializedPendingOutcome::Adopted(
            WakeAdoptedMaterializedPending { links, auto_resume },
        ))
    }

    async fn resolve_materialized_pending_for_workflow_once(
        &self,
        workflow_id: WorkflowId,
        decision: WakeResolveMaterializedDecision,
        timestamp: Timestamp,
    ) -> DbResult<ResolveMaterializedPendingAttempt> {
        let mut tx = self.workflow_repo.begin_tx().await?;
        let Some(_binding) = fetch_binding_by_workflow_tx(&mut tx, workflow_id).await? else {
            tx.rollback().await?;
            return Ok(ResolveMaterializedPendingAttempt::Done(Ok(
                WakeResolveMaterializedPendingOutcome::NothingPending,
            )));
        };
        let Some(head) = tx.fetch_workflow_head(workflow_id).await? else {
            tx.rollback().await?;
            return Ok(ResolveMaterializedPendingAttempt::Done(Ok(
                WakeResolveMaterializedPendingOutcome::NothingPending,
            )));
        };
        let pending_ids = fetch_pending_terminal_delivery_ids_tx(&mut tx, workflow_id).await?;
        if pending_ids.is_empty() {
            let terminal_rows = sqlx::query(
                "SELECT d.status
                 FROM workflow_deliveries d
                 JOIN wake_terminal_receipts p
                   ON p.workflow_id = d.workflow_id AND p.delivery_id = d.delivery_id
                 WHERE d.workflow_id = ?1",
            )
            .bind(to_i64(workflow_id.0, "workflow_id")?)
            .fetch_all(&mut *tx.tx)
            .await?;
            tx.rollback().await?;
            let outcome = if terminal_rows.is_empty() {
                WakeResolveMaterializedPendingOutcome::NothingPending
            } else {
                WakeResolveMaterializedPendingOutcome::AlreadyResolved
            };
            return Ok(ResolveMaterializedPendingAttempt::Done(Ok(outcome)));
        }
        let projection = fetch_any_terminal_projection_tx(&mut tx, workflow_id)
            .await?
            .ok_or_else(|| {
                DbError::Serialization("wake pending delivery set missing projection".to_string())
            })?;
        let materialized = fetch_materialized_pending_deliveries_tx(&mut tx, workflow_id).await?;
        let materialized_ids: std::collections::BTreeSet<_> = materialized
            .iter()
            .map(|item| item.pending.canonical_delivery.delivery_id)
            .collect();
        let missing_ids: Vec<_> = pending_ids
            .iter()
            .copied()
            .filter(|delivery_id| !materialized_ids.contains(delivery_id))
            .collect();
        if !missing_ids.is_empty() {
            tx.rollback().await?;
            return Ok(ResolveMaterializedPendingAttempt::Done(Err(
                WakeResolveMaterializedPendingError::NotFullyMaterialized {
                    delivery_ids: missing_ids,
                },
            )));
        }
        let event = match decision {
            WakeResolveMaterializedDecision::Accept => WakeRegistrationEvent::RuntimeAccepted {
                terminal: Box::new(projection.terminal.clone()),
            },
            WakeResolveMaterializedDecision::Suppress => WakeRegistrationEvent::RuntimeSuppressed {
                terminal: Box::new(projection.terminal.clone()),
            },
        };
        let resolution_decision = match decision {
            WakeResolveMaterializedDecision::Accept => DeliveryResolutionDecision::Accept,
            WakeResolveMaterializedDecision::Suppress => DeliveryResolutionDecision::Suppress {
                reason: phoenix_workflow::SuppressionReason::ReducerTerminal,
            },
        };
        let event_codec = local_codec(&wake_profile::event_codec());
        let event_payload = json_blob(&event)?;
        let mut snapshot: WakeRegistrationSnapshot = serde_json::from_slice(&head.snapshot_payload)
            .map_err(|e| DbError::Serialization(e.to_string()))?;
        snapshot.runtime_availability = match decision {
            WakeResolveMaterializedDecision::Accept => wake_profile::RuntimeAvailability::Accepted,
            WakeResolveMaterializedDecision::Suppress => {
                wake_profile::RuntimeAvailability::Suppressed
            }
        };
        let next_snapshot_payload = json_blob(&snapshot)?;
        let next_snapshot_codec = LocalCodec {
            family: wake_profile::snapshot_codec().family.to_string(),
            version: wake_profile::snapshot_codec().version,
        };
        let transition_id = TransitionId(head.version.next().0);
        let outcome = tx
            .resolve_deliveries_exact(DeliveryResolutionPlan {
                workflow_id,
                expected_version: head.version,
                transition_id,
                generation: head.generation,
                next_status: head.status,
                event_codec: &event_codec,
                event_payload: &event_payload,
                next_snapshot_codec: &next_snapshot_codec,
                next_snapshot_payload: &next_snapshot_payload,
                committed_at: timestamp,
                exact_delivery_ids: &pending_ids,
                decision: resolution_decision,
            })
            .await?;
        match outcome {
            CommitOutcome::Committed => {
                #[cfg(test)]
                maybe_fail_after_canonical_transition(self.failpoint_namespace, workflow_id)?;
                let auto_resume = matches!(decision, WakeResolveMaterializedDecision::Accept)
                    && materialized.iter().any(|item| item.link.auto_resume);
                tx.commit().await?;
                Ok(ResolveMaterializedPendingAttempt::Done(Ok(
                    WakeResolveMaterializedPendingOutcome::Resolved {
                        delivery_ids: pending_ids,
                        auto_resume,
                    },
                )))
            }
            CommitOutcome::VersionConflict => {
                tx.rollback().await?;
                Ok(ResolveMaterializedPendingAttempt::RetryVersionConflict)
            }
            CommitOutcome::InvalidPlan => {
                tx.rollback().await?;
                let current_pending = self
                    .list_materialized_pending_for_workflow(workflow_id)
                    .await?;
                let outcome = if current_pending.is_empty() {
                    WakeResolveMaterializedPendingOutcome::AlreadyResolved
                } else {
                    WakeResolveMaterializedPendingOutcome::NothingPending
                };
                Ok(ResolveMaterializedPendingAttempt::Done(Ok(outcome)))
            }
            CommitOutcome::UnsupportedCodec => {
                tx.rollback().await?;
                Err(DbError::Serialization(
                    "wake materialized resolve returned unexpected codec error".to_string(),
                ))
            }
        }
    }
}

async fn expire_observation_lease_in_tx(
    tx: &mut WorkflowTx<'_>,
    input: &ExpireLeaseInput,
) -> DbResult<AuthorityOutcome> {
    let lease = sqlx::query("SELECT a.status as attempt_status, e.capability_kind FROM workflow_reclaimable_leases l JOIN workflow_attempts a ON a.workflow_id = l.workflow_id AND a.attempt_id = l.attempt_id JOIN workflow_effects e ON e.workflow_id = a.workflow_id AND e.effect_id = a.effect_id WHERE l.workflow_id = ?1 AND l.attempt_id = ?2 AND a.effect_id = ?3 AND l.lease_until <= ?4")
        .bind(to_i64(input.workflow_id.0, "workflow_id")?)
        .bind(to_i64(input.attempt_id.0, "attempt_id")?)
        .bind(to_i64(input.effect_id.0, "effect_id")?)
        .bind(to_i64(input.now.0, "now")?)
        .fetch_optional(&mut *tx.tx)
        .await?;
    let Some(lease) = lease else {
        return Ok(AuthorityOutcome::StaleAuthority);
    };
    let attempt_status = lease.get::<String, _>("attempt_status");
    if !matches!(attempt_status.as_str(), "Begun" | "ObservationRecorded") {
        return Ok(AuthorityOutcome::StaleAuthority);
    }
    sqlx::query(
        "DELETE FROM workflow_reclaimable_leases WHERE workflow_id = ?1 AND attempt_id = ?2",
    )
    .bind(to_i64(input.workflow_id.0, "workflow_id")?)
    .bind(to_i64(input.attempt_id.0, "attempt_id")?)
    .execute(&mut *tx.tx)
    .await?;
    sqlx::query("UPDATE workflow_attempts SET status = 'AuthorityLost' WHERE workflow_id = ?1 AND attempt_id = ?2")
        .bind(to_i64(input.workflow_id.0, "workflow_id")?)
        .bind(to_i64(input.attempt_id.0, "attempt_id")?)
        .execute(&mut *tx.tx)
        .await?;
    let next_status = match lease.get::<String, _>("capability_kind").as_str() {
        "ReclaimableObservation" => "Eligible",
        "SafelyRepeatable" => "RetryWait",
        _ => "AmbiguityWait",
    };
    sqlx::query("UPDATE workflow_effects SET status = ?3 WHERE workflow_id = ?1 AND effect_id = ?2 AND status = 'Executing'")
        .bind(to_i64(input.workflow_id.0, "workflow_id")?)
        .bind(to_i64(input.effect_id.0, "effect_id")?)
        .bind(next_status)
        .execute(&mut *tx.tx)
        .await?;
    Ok(AuthorityOutcome::Authorized)
}

async fn next_global_workflow_id_tx(tx: &mut WorkflowTx<'_>) -> DbResult<WorkflowId> {
    sqlx::query(
        "INSERT INTO workflow_global_sequences (sequence_name, next_value)
         VALUES ('workflow', 2)
         ON CONFLICT(sequence_name)
         DO UPDATE SET next_value = workflow_global_sequences.next_value + 1",
    )
    .execute(&mut *tx.tx)
    .await?;
    let allocated = sqlx::query_scalar::<_, i64>(
        "SELECT next_value - 1 FROM workflow_global_sequences WHERE sequence_name = 'workflow'",
    )
    .fetch_one(&mut *tx.tx)
    .await?;
    Ok(WorkflowId(to_u64(allocated, "workflow_id")?))
}

#[cfg(test)]
async fn next_test_workflow_id(repo: &WakeRepository) -> DbResult<WorkflowId> {
    let mut tx = repo.workflow_repo.begin_tx().await?;
    let workflow_id = next_global_workflow_id_tx(&mut tx).await?;
    tx.commit().await?;
    Ok(workflow_id)
}

fn local_codec(codec: &phoenix_workflow::CodecRef) -> LocalCodec {
    LocalCodec {
        family: codec.family.to_string(),
        version: codec.version,
    }
}

fn json_blob<T: Serialize>(value: &T) -> DbResult<Vec<u8>> {
    serde_json::to_vec(value).map_err(|e| DbError::Serialization(e.to_string()))
}

fn resource_key(resource: &WakeResourceIdentity) -> String {
    match resource {
        WakeResourceIdentity::Bash(identity) => format!("bash:{}", identity.handle_id),
        WakeResourceIdentity::TmuxWindow(identity) => {
            format!("tmux:{}:{}", identity.server_token, identity.window_id)
        }
        WakeResourceIdentity::Subagent(_) => unreachable!("subagent wake bindings not implemented"),
    }
}

fn replay_receipt(existing: &WakeBindingRecord) -> WakeRegistrationReceipt {
    WakeRegistrationReceipt {
        contract_id: existing.contract_id.clone(),
        resource: existing.resource.clone(),
        expires_at: existing.expires_at,
        registering_tool_use_id: existing.registering_tool_use_id.clone(),
    }
}

async fn fetch_existing_binding_tx(
    tx: &mut WorkflowTx<'_>,
    input: &WakeRegistrationIntent,
) -> DbResult<Option<WakeBindingRecord>> {
    let row = sqlx::query(
        "SELECT workflow_id, conversation_id, contract_id, profile_kind, profile_version,
                scope_kind, scope_stable_key, resource_kind, bash_handle_id,
                tmux_server_token, tmux_window_id, tmux_completion_policy, registering_tool_use_id,
                expires_at, prepared_fingerprint
         FROM wake_bindings
         WHERE profile_kind = 'wake' AND profile_version = ?1 AND conversation_id = ?2
           AND contract_id = ?3 AND resource_kind = ?4
           AND COALESCE(bash_handle_id, '') = ?5
           AND COALESCE(tmux_server_token, '') = ?6
           AND COALESCE(tmux_window_id, '') = ?7",
    )
    .bind(i64::from(wake_profile::PROTOCOL_VERSION))
    .bind(&input.conversation_id)
    .bind(&input.contract_id)
    .bind(resource_kind_str(&input.resource))
    .bind(bash_handle_id(&input.resource).unwrap_or_default())
    .bind(tmux_server_token(&input.resource).unwrap_or_default())
    .bind(tmux_window_id(&input.resource).unwrap_or_default())
    .fetch_optional(&mut *tx.tx)
    .await?;
    row.as_ref().map(binding_from_row).transpose()
}

async fn fetch_binding_by_workflow_tx(
    tx: &mut WorkflowTx<'_>,
    workflow_id: WorkflowId,
) -> DbResult<Option<WakeBindingRecord>> {
    let row = sqlx::query(
        "SELECT workflow_id, conversation_id, contract_id, profile_kind, profile_version,
                scope_kind, scope_stable_key, resource_kind, bash_handle_id,
                tmux_server_token, tmux_window_id, tmux_completion_policy, registering_tool_use_id,
                expires_at, prepared_fingerprint
         FROM wake_bindings WHERE workflow_id = ?1",
    )
    .bind(i64::try_from(workflow_id.0).map_err(|e| DbError::Serialization(e.to_string()))?)
    .fetch_optional(&mut *tx.tx)
    .await?;
    row.as_ref().map(binding_from_row).transpose()
}

async fn insert_binding_tx(
    tx: &mut WorkflowTx<'_>,
    workflow_id: WorkflowId,
    input: &WakeRegistrationIntent,
    prepared_fingerprint: &str,
    now: Timestamp,
) -> DbResult<()> {
    sqlx::query(
        "INSERT INTO wake_bindings (
            workflow_id, conversation_id, contract_id, profile_kind, profile_version,
            scope_kind, scope_stable_key, resource_kind, bash_handle_id,
            tmux_server_token, tmux_window_id, tmux_completion_policy, registering_tool_use_id,
            expires_at, prepared_fingerprint, observe_effect_id, created_at
         ) VALUES (?1, ?2, ?3, 'wake', ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
    )
    .bind(i64::try_from(workflow_id.0).map_err(|e| DbError::Serialization(e.to_string()))?)
    .bind(&input.conversation_id)
    .bind(&input.contract_id)
    .bind(i64::from(wake_profile::PROTOCOL_VERSION))
    .bind(scope_kind_str(&input.registration_scope))
    .bind(&input.registration_scope.stable_key)
    .bind(resource_kind_str(&input.resource))
    .bind(bash_handle_id(&input.resource))
    .bind(tmux_server_token(&input.resource))
    .bind(tmux_window_id(&input.resource))
    .bind(tmux_completion_policy(&input.resource))
    .bind(&input.registering_tool_use_id)
    .bind(i64::try_from(input.expires_at.0).map_err(|e| DbError::Serialization(e.to_string()))?)
    .bind(prepared_fingerprint)
    .bind(
        i64::try_from(REGISTRATION_EFFECT_ID.0)
            .map_err(|e| DbError::Serialization(e.to_string()))?,
    )
    .bind(i64::try_from(now.0).map_err(|e| DbError::Serialization(e.to_string()))?)
    .execute(&mut *tx.tx)
    .await?;
    Ok(())
}

fn parse_active_unresolved_row(row: &sqlx::sqlite::SqliteRow) -> DbResult<WakeActiveUnresolvedRow> {
    Ok(WakeActiveUnresolvedRow {
        workflow_id: WorkflowId(to_u64(row.get::<i64, _>("workflow_id"), "workflow_id")?),
        conversation_id: row.get("conversation_id"),
        contract_id: row.get("contract_id"),
        expires_at: Timestamp(to_u64(row.get::<i64, _>("expires_at"), "expires_at")?),
    })
}

fn binding_from_row(row: &sqlx::sqlite::SqliteRow) -> DbResult<WakeBindingRecord> {
    Ok(WakeBindingRecord {
        workflow_id: WorkflowId(
            u64::try_from(row.get::<i64, _>("workflow_id"))
                .map_err(|e| DbError::Serialization(e.to_string()))?,
        ),
        conversation_id: row.get("conversation_id"),
        contract_id: row.get("contract_id"),
        profile: ProfileRef {
            profile_kind: row.get("profile_kind"),
            profile_version: u32::try_from(row.get::<i64, _>("profile_version"))
                .map_err(|e| DbError::Serialization(e.to_string()))?,
        },
        registration_scope: wake_types::WorkScopeIdentity {
            kind: match row.get::<String, _>("scope_kind").as_str() {
                "Conversation" => wake_types::WorkScopeKind::Conversation,
                "Worktree" => wake_types::WorkScopeKind::Worktree,
                other => {
                    return Err(DbError::Serialization(format!(
                        "unknown scope kind: {other}"
                    )))
                }
            },
            stable_key: row.get("scope_stable_key"),
        },
        resource: resource_from_row(row)?,
        registering_tool_use_id: row.get("registering_tool_use_id"),
        expires_at: Timestamp(
            u64::try_from(row.get::<i64, _>("expires_at"))
                .map_err(|e| DbError::Serialization(e.to_string()))?,
        ),
        prepared_fingerprint: row.get("prepared_fingerprint"),
    })
}

fn resource_from_row(row: &sqlx::sqlite::SqliteRow) -> DbResult<WakeResourceIdentity> {
    match row.get::<String, _>("resource_kind").as_str() {
        "Bash" => Ok(WakeResourceIdentity::Bash(
            wake_types::BashResourceIdentity {
                work_scope: wake_types::WorkScopeIdentity {
                    kind: match row.get::<String, _>("scope_kind").as_str() {
                        "Conversation" => wake_types::WorkScopeKind::Conversation,
                        "Worktree" => wake_types::WorkScopeKind::Worktree,
                        other => {
                            return Err(DbError::Serialization(format!(
                                "unknown scope kind: {other}"
                            )))
                        }
                    },
                    stable_key: row.get("scope_stable_key"),
                },
                handle_id: row.get::<String, _>("bash_handle_id"),
            },
        )),
        "TmuxWindow" => Ok(WakeResourceIdentity::TmuxWindow(
            wake_types::TmuxResourceIdentity {
                work_scope: wake_types::WorkScopeIdentity {
                    kind: match row.get::<String, _>("scope_kind").as_str() {
                        "Conversation" => wake_types::WorkScopeKind::Conversation,
                        "Worktree" => wake_types::WorkScopeKind::Worktree,
                        other => {
                            return Err(DbError::Serialization(format!(
                                "unknown scope kind: {other}"
                            )))
                        }
                    },
                    stable_key: row.get("scope_stable_key"),
                },
                server_token: row.get::<String, _>("tmux_server_token"),
                window_id: row.get::<String, _>("tmux_window_id"),
                completion_policy: parse_tmux_completion_policy(
                    &row.get::<String, _>("tmux_completion_policy"),
                )?,
            },
        )),
        other => Err(DbError::Serialization(format!(
            "unknown resource kind: {other}"
        ))),
    }
}

fn scope_kind_str(scope: &wake_types::WorkScopeIdentity) -> &'static str {
    match scope.kind {
        wake_types::WorkScopeKind::Conversation => "Conversation",
        wake_types::WorkScopeKind::Worktree => "Worktree",
    }
}

fn resource_kind_str(resource: &WakeResourceIdentity) -> &'static str {
    match resource {
        WakeResourceIdentity::Bash(_) => "Bash",
        WakeResourceIdentity::TmuxWindow(_) => "TmuxWindow",
        WakeResourceIdentity::Subagent(_) => unreachable!("subagent wake bindings not implemented"),
    }
}

fn bash_handle_id(resource: &WakeResourceIdentity) -> Option<String> {
    match resource {
        WakeResourceIdentity::Bash(identity) => Some(identity.handle_id.clone()),
        WakeResourceIdentity::TmuxWindow(_) => None,
        WakeResourceIdentity::Subagent(_) => unreachable!("subagent wake bindings not implemented"),
    }
}

fn tmux_server_token(resource: &WakeResourceIdentity) -> Option<String> {
    match resource {
        WakeResourceIdentity::TmuxWindow(identity) => Some(identity.server_token.clone()),
        WakeResourceIdentity::Bash(_) => None,
        WakeResourceIdentity::Subagent(_) => unreachable!("subagent wake bindings not implemented"),
    }
}

fn tmux_completion_policy(resource: &WakeResourceIdentity) -> &'static str {
    match resource {
        WakeResourceIdentity::TmuxWindow(identity) => match identity.completion_policy {
            wake_types::TmuxCompletionPolicy::KeepOpen => "KeepOpen",
            wake_types::TmuxCompletionPolicy::CloseAfterCompletion => "CloseAfterCompletion",
        },
        WakeResourceIdentity::Bash(_) => "KeepOpen",
        WakeResourceIdentity::Subagent(_) => unreachable!("subagent wake bindings not implemented"),
    }
}

fn parse_tmux_completion_policy(value: &str) -> DbResult<wake_types::TmuxCompletionPolicy> {
    match value {
        "KeepOpen" => Ok(wake_types::TmuxCompletionPolicy::KeepOpen),
        "CloseAfterCompletion" => Ok(wake_types::TmuxCompletionPolicy::CloseAfterCompletion),
        other => Err(DbError::Serialization(format!(
            "unknown tmux completion policy: {other}"
        ))),
    }
}

fn tmux_window_id(resource: &WakeResourceIdentity) -> Option<String> {
    match resource {
        WakeResourceIdentity::TmuxWindow(identity) => Some(identity.window_id.clone()),
        WakeResourceIdentity::Bash(_) => None,
        WakeResourceIdentity::Subagent(_) => unreachable!("subagent wake bindings not implemented"),
    }
}

fn evidence_occurred_at(evidence: &WakeTerminalEvidence) -> Timestamp {
    match evidence {
        WakeTerminalEvidence::Bash(BashTerminalEvidence { occurred_at, .. })
        | WakeTerminalEvidence::TmuxWindow(TmuxTerminalEvidence { occurred_at, .. }) => {
            *occurred_at
        }
        WakeTerminalEvidence::Subagent(_) => unreachable!("subagent wake bindings not implemented"),
    }
}

fn resource_matches_evidence(
    resource: &WakeResourceIdentity,
    evidence: &WakeTerminalEvidence,
) -> bool {
    match (resource, evidence) {
        (WakeResourceIdentity::Bash(expected), WakeTerminalEvidence::Bash(actual)) => {
            &actual.identity == expected
        }
        (WakeResourceIdentity::TmuxWindow(expected), WakeTerminalEvidence::TmuxWindow(actual)) => {
            &actual.identity == expected
        }
        (WakeResourceIdentity::Subagent(_), WakeTerminalEvidence::Subagent(_)) => {
            unreachable!("subagent wake bindings not implemented")
        }
        _ => false,
    }
}

async fn insert_terminal_receipt_projection_tx(
    tx: &mut WorkflowTx<'_>,
    binding: &WakeBindingRecord,
    receipt: &LocalReceiptRecord,
    delivery: &LocalDeliveryRecord,
    terminal: &WakeTerminalPayload,
) -> DbResult<()> {
    let (
        resource_kind,
        terminal_kind,
        resolved_at,
        bash_handle_id,
        tmux_server_token,
        tmux_window_id,
        bash_status,
        tmux_status,
        occurred_at,
        exit_code,
        duration_ms,
        signal_number,
        kill_signal_sent,
        forgotten_reason,
        cancelled_reason,
        cancelled_at,
        tail,
    ) = projection_parts(terminal);
    sqlx::query(
        "UPDATE wake_bindings SET resolved_at = ?2 WHERE workflow_id = ?1 AND resolved_at IS NULL",
    )
    .bind(to_i64(binding.workflow_id.0, "workflow_id")?)
    .bind(to_i64(resolved_at.0, "resolved_at")?)
    .execute(&mut *tx.tx)
    .await?;
    sqlx::query(
        "INSERT INTO wake_terminal_receipts (
            workflow_id, receipt_id, delivery_id, conversation_id, contract_id, resource_kind,
            terminal_kind, resolved_at, bash_handle_id, tmux_server_token, tmux_window_id,
            bash_status, tmux_status, occurred_at, exit_code, duration_ms, signal_number,
            kill_signal_sent, forgotten_reason, cancelled_reason, cancelled_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21)"
    )
    .bind(to_i64(binding.workflow_id.0, "workflow_id")?)
    .bind(to_i64(receipt.receipt_id.0, "receipt_id")?)
    .bind(to_i64(delivery.delivery_id.0, "delivery_id")?)
    .bind(&binding.conversation_id)
    .bind(&binding.contract_id)
    .bind(resource_kind)
    .bind(terminal_kind)
    .bind(to_i64(resolved_at.0, "resolved_at")?)
    .bind(bash_handle_id)
    .bind(tmux_server_token)
    .bind(tmux_window_id)
    .bind(bash_status)
    .bind(tmux_status)
    .bind(occurred_at.map(|ts| i64::try_from(ts.0)).transpose().map_err(|e| DbError::Serialization(e.to_string()))?)
    .bind(exit_code.map(i64::from))
    .bind(duration_ms.map(i64::try_from).transpose().map_err(|e| DbError::Serialization(e.to_string()))?)
    .bind(signal_number.map(i64::from))
    .bind(kill_signal_sent)
    .bind(forgotten_reason)
    .bind(cancelled_reason)
    .bind(cancelled_at.map(|ts| i64::try_from(ts.0)).transpose().map_err(|e| DbError::Serialization(e.to_string()))?)
    .execute(&mut *tx.tx)
    .await?;
    for (ordinal, line) in tail.into_iter().enumerate() {
        sqlx::query(
            "INSERT INTO wake_terminal_receipt_tails (workflow_id, receipt_id, ordinal, line)
             VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(to_i64(binding.workflow_id.0, "workflow_id")?)
        .bind(to_i64(receipt.receipt_id.0, "receipt_id")?)
        .bind(i64::try_from(ordinal).map_err(|e| DbError::Serialization(e.to_string()))?)
        .bind(line)
        .execute(&mut *tx.tx)
        .await?;
    }
    Ok(())
}

fn projection_parts(
    terminal: &WakeTerminalPayload,
) -> (
    &'static str,
    &'static str,
    Timestamp,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<&'static str>,
    Option<&'static str>,
    Option<Timestamp>,
    Option<i32>,
    Option<u64>,
    Option<i32>,
    Option<String>,
    Option<&'static str>,
    Option<&'static str>,
    Option<Timestamp>,
    Vec<String>,
) {
    match terminal {
        WakeTerminalPayload::Fired {
            resource,
            evidence,
            resolved_at,
            ..
        } => match (resource, evidence) {
            (WakeResourceIdentity::Bash(identity), WakeTerminalEvidence::Bash(ev)) => (
                "Bash",
                "Fired",
                *resolved_at,
                Some(identity.handle_id.clone()),
                None,
                None,
                Some(match ev.status {
                    wake_types::BashTerminalStatus::Exited => "Exited",
                    wake_types::BashTerminalStatus::Killed => "Killed",
                    wake_types::BashTerminalStatus::KillPendingKernel => "KillPendingKernel",
                }),
                None,
                Some(ev.occurred_at),
                ev.exit_code,
                ev.duration_ms,
                ev.signal_number,
                ev.kill_signal_sent.clone(),
                None,
                None,
                None,
                ev.final_tail.clone(),
            ),
            (WakeResourceIdentity::TmuxWindow(identity), WakeTerminalEvidence::TmuxWindow(ev)) => (
                "TmuxWindow",
                "Fired",
                *resolved_at,
                None,
                Some(identity.server_token.clone()),
                Some(identity.window_id.clone()),
                None,
                Some(match ev.status {
                    wake_types::TmuxTerminalStatus::ExitMarkerObserved => "ExitMarkerObserved",
                    wake_types::TmuxTerminalStatus::WindowKilled => "WindowKilled",
                }),
                Some(ev.occurred_at),
                ev.exit_code,
                ev.duration_ms,
                None,
                None,
                None,
                None,
                None,
                ev.final_tail.clone(),
            ),
            _ => unreachable!("resource/evidence mismatch guarded earlier"),
        },
        WakeTerminalPayload::Expired {
            resource,
            resolved_at,
            ..
        } => match resource {
            WakeResourceIdentity::Bash(identity) => (
                "Bash",
                "Expired",
                *resolved_at,
                Some(identity.handle_id.clone()),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                vec![],
            ),
            WakeResourceIdentity::TmuxWindow(identity) => (
                "TmuxWindow",
                "Expired",
                *resolved_at,
                None,
                Some(identity.server_token.clone()),
                Some(identity.window_id.clone()),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                vec![],
            ),
            WakeResourceIdentity::Subagent(_) => {
                unreachable!("subagent wake bindings not implemented")
            }
        },
        WakeTerminalPayload::Forgotten {
            resource,
            reason,
            resolved_at,
            ..
        } => {
            let reason = match reason {
                WakeForgottenReason::PhoenixRestart => "PhoenixRestart",
                WakeForgottenReason::CascadeDestroyedHandle => "CascadeDestroyedHandle",
                WakeForgottenReason::TmuxHandleMissing => "TmuxHandleMissing",
                WakeForgottenReason::SubagentHandleMissing => {
                    unreachable!("subagent wake bindings not implemented")
                }
            };
            match resource {
                WakeResourceIdentity::Bash(identity) => (
                    "Bash",
                    "Forgotten",
                    *resolved_at,
                    Some(identity.handle_id.clone()),
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    Some(reason),
                    None,
                    None,
                    vec![],
                ),
                WakeResourceIdentity::TmuxWindow(identity) => (
                    "TmuxWindow",
                    "Forgotten",
                    *resolved_at,
                    None,
                    Some(identity.server_token.clone()),
                    Some(identity.window_id.clone()),
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    Some(reason),
                    None,
                    None,
                    vec![],
                ),
                WakeResourceIdentity::Subagent(_) => {
                    unreachable!("subagent wake bindings not implemented")
                }
            }
        }
        WakeTerminalPayload::Cancelled {
            resource,
            reason,
            resolved_at,
            ..
        } => {
            let reason = match reason {
                WakeCancellationReason::ExplicitCancel => "ExplicitCancel",
            };
            match resource {
                WakeResourceIdentity::Bash(identity) => (
                    "Bash",
                    "Cancelled",
                    *resolved_at,
                    Some(identity.handle_id.clone()),
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    Some(reason),
                    Some(*resolved_at),
                    vec![],
                ),
                WakeResourceIdentity::TmuxWindow(identity) => (
                    "TmuxWindow",
                    "Cancelled",
                    *resolved_at,
                    None,
                    Some(identity.server_token.clone()),
                    Some(identity.window_id.clone()),
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    Some(reason),
                    Some(*resolved_at),
                    vec![],
                ),
                WakeResourceIdentity::Subagent(_) => {
                    unreachable!("subagent wake bindings not implemented")
                }
            }
        }
    }
}

async fn fetch_pending_terminal_delivery_ids_tx(
    tx: &mut WorkflowTx<'_>,
    workflow_id: WorkflowId,
) -> DbResult<Vec<DeliveryId>> {
    let rows = sqlx::query(
        "SELECT d.delivery_id
         FROM workflow_deliveries d
         JOIN wake_terminal_receipts p
           ON p.workflow_id = d.workflow_id AND p.delivery_id = d.delivery_id
         WHERE d.workflow_id = ?1 AND d.status = 'Pending'
         ORDER BY d.delivery_id",
    )
    .bind(to_i64(workflow_id.0, "workflow_id")?)
    .fetch_all(&mut *tx.tx)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(DeliveryId(to_u64(
                row.get::<i64, _>("delivery_id"),
                "delivery_id",
            )?))
        })
        .collect()
}
async fn fetch_pending_delivery_exact_tx(
    tx: &mut WorkflowTx<'_>,
    workflow_id: WorkflowId,
    delivery_id: DeliveryId,
    conversation_id: &str,
) -> DbResult<Option<WakePendingDelivery>> {
    let row = sqlx::query(
        "SELECT d.workflow_id, d.delivery_id, d.effect_id, d.barrier_id, d.consumer_kind,
                d.event_codec_family, d.event_codec_version, d.payload_kind, d.payload_blob,
                d.requires_runtime_acceptance, d.status, d.runtime_acceptance_status,
                d.suppression_reason, d.accepted_by_transition_id,
                p.receipt_id, p.conversation_id, p.contract_id, p.resource_kind, p.terminal_kind,
                p.resolved_at, p.bash_handle_id, p.tmux_server_token, p.tmux_window_id,
                p.bash_status, p.tmux_status, p.occurred_at, p.exit_code, p.duration_ms,
                p.signal_number, p.kill_signal_sent, p.forgotten_reason, p.cancelled_reason,
                p.cancelled_at, b.scope_kind, b.scope_stable_key, b.tmux_completion_policy
         FROM workflow_deliveries d
         JOIN wake_terminal_receipts p
           ON p.workflow_id = d.workflow_id AND p.delivery_id = d.delivery_id
         JOIN wake_bindings b ON b.workflow_id = p.workflow_id
         WHERE d.workflow_id = ?1 AND d.delivery_id = ?2 AND p.conversation_id = ?3 AND d.status = 'Pending'"
    )
    .bind(to_i64(workflow_id.0, "workflow_id")?)
    .bind(to_i64(delivery_id.0, "delivery_id")?)
    .bind(conversation_id)
    .fetch_optional(&mut *tx.tx)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let receipt_id = ReceiptId(to_u64(row.get::<i64, _>("receipt_id"), "receipt_id")?);
    let projection = projection_from_row(
        &row,
        fetch_tail_lines_tx(tx, workflow_id, receipt_id).await?,
    )?;
    Ok(Some(WakePendingDelivery {
        workflow_id,
        conversation_id: projection.conversation_id.clone(),
        receipt: projection,
        canonical_delivery: delivery_from_join_row(&row)?,
    }))
}

async fn fetch_pending_deliveries_for_conversation_tx(
    tx: &mut WorkflowTx<'_>,
    conversation_id: &str,
) -> DbResult<Vec<WakePendingDelivery>> {
    let rows = sqlx::query(
        "SELECT d.workflow_id, d.delivery_id, d.effect_id, d.barrier_id, d.consumer_kind,
                d.event_codec_family, d.event_codec_version, d.payload_kind, d.payload_blob,
                d.requires_runtime_acceptance, d.status, d.runtime_acceptance_status,
                d.suppression_reason, d.accepted_by_transition_id,
                p.receipt_id, p.conversation_id, p.contract_id, p.resource_kind, p.terminal_kind,
                p.resolved_at, p.bash_handle_id, p.tmux_server_token, p.tmux_window_id,
                p.bash_status, p.tmux_status, p.occurred_at, p.exit_code, p.duration_ms,
                p.signal_number, p.kill_signal_sent, p.forgotten_reason, p.cancelled_reason,
                p.cancelled_at, b.scope_kind, b.scope_stable_key, b.tmux_completion_policy
         FROM workflow_deliveries d
         JOIN wake_terminal_receipts p
           ON p.workflow_id = d.workflow_id AND p.delivery_id = d.delivery_id
         JOIN wake_bindings b ON b.workflow_id = p.workflow_id
         WHERE p.conversation_id = ?1 AND d.status = 'Pending'
         ORDER BY d.delivery_id",
    )
    .bind(conversation_id)
    .fetch_all(&mut *tx.tx)
    .await?;
    let mut pending = Vec::with_capacity(rows.len());
    for row in rows {
        let workflow_id = WorkflowId(to_u64(row.get::<i64, _>("workflow_id"), "workflow_id")?);
        let receipt_id = ReceiptId(to_u64(row.get::<i64, _>("receipt_id"), "receipt_id")?);
        let projection = projection_from_row(
            &row,
            fetch_tail_lines_tx(tx, workflow_id, receipt_id).await?,
        )?;
        pending.push(WakePendingDelivery {
            workflow_id,
            conversation_id: projection.conversation_id.clone(),
            receipt: projection,
            canonical_delivery: delivery_from_join_row(&row)?,
        });
    }
    Ok(pending)
}

async fn fetch_materialized_pending_batches_for_conversation_tx(
    tx: &mut WorkflowTx<'_>,
    conversation_id: &str,
) -> DbResult<
    Vec<(
        WorkflowId,
        Vec<DeliveryId>,
        Vec<WakeMaterializedPendingDelivery>,
    )>,
> {
    let rows = sqlx::query(
        "SELECT d.*, p.receipt_id, p.conversation_id, p.contract_id, p.resource_kind,
                p.terminal_kind, p.resolved_at, p.bash_handle_id, p.tmux_server_token,
                p.tmux_window_id, p.bash_status, p.tmux_status, p.occurred_at, p.exit_code,
                p.duration_ms, p.signal_number, p.kill_signal_sent, p.forgotten_reason,
                p.cancelled_reason, p.cancelled_at, b.scope_kind, b.scope_stable_key, b.tmux_completion_policy,
                l.workflow_id AS link_workflow_id, l.delivery_id AS link_delivery_id,
                l.conversation_id AS link_conversation_id, l.message_id AS link_message_id,
                l.registering_tool_use_id, l.terminal_kind, l.auto_resume,
                l.created_at AS link_created_at,
                m.message_id, m.conversation_id, m.sequence_id, m.message_type, m.content,
                m.display_data, m.usage_data, m.created_at
         FROM workflow_deliveries d
         JOIN wake_terminal_receipts p
           ON p.workflow_id = d.workflow_id AND p.delivery_id = d.delivery_id
         JOIN wake_bindings b ON b.workflow_id = d.workflow_id
         LEFT JOIN wake_delivery_messages l
           ON l.workflow_id = d.workflow_id AND l.delivery_id = d.delivery_id
         LEFT JOIN messages m ON m.message_id = l.message_id
         WHERE p.conversation_id = ?1 AND d.status = 'Pending'
         ORDER BY d.workflow_id, d.delivery_id",
    )
    .bind(conversation_id)
    .fetch_all(&mut *tx.tx)
    .await?;
    let mut batches: Vec<(
        WorkflowId,
        Vec<DeliveryId>,
        Vec<WakeMaterializedPendingDelivery>,
    )> = Vec::new();
    for row in rows {
        let workflow_id = WorkflowId(to_u64(row.get::<i64, _>("workflow_id"), "workflow_id")?);
        let delivery_id = DeliveryId(to_u64(row.get::<i64, _>("delivery_id"), "delivery_id")?);
        let batch = match batches.last_mut() {
            Some(batch) if batch.0 == workflow_id => batch,
            _ => {
                batches.push((workflow_id, Vec::new(), Vec::new()));
                batches.last_mut().expect("just pushed")
            }
        };
        batch.1.push(delivery_id);
        if row.get::<Option<String>, _>("link_message_id").is_some() {
            let receipt_id = ReceiptId(to_u64(row.get::<i64, _>("receipt_id"), "receipt_id")?);
            let tail = fetch_tail_lines_tx(tx, workflow_id, receipt_id).await?;
            let pending = WakePendingDelivery {
                workflow_id,
                conversation_id: row.get("conversation_id"),
                receipt: projection_from_row(&row, tail)?,
                canonical_delivery: delivery_from_join_row(&row)?,
            };
            batch.2.push(WakeMaterializedPendingDelivery {
                pending,
                link: delivery_message_link_from_join_row(&row)?,
            });
        }
    }
    Ok(batches)
}

async fn fetch_materialized_pending_deliveries_tx(
    tx: &mut WorkflowTx<'_>,
    workflow_id: WorkflowId,
) -> DbResult<Vec<WakeMaterializedPendingDelivery>> {
    let rows = sqlx::query(
        "SELECT d.*, p.receipt_id, p.conversation_id, p.contract_id, p.resource_kind,
                p.terminal_kind, p.resolved_at, p.bash_handle_id, p.tmux_server_token,
                p.tmux_window_id, p.bash_status, p.tmux_status, p.occurred_at, p.exit_code,
                p.duration_ms, p.signal_number, p.kill_signal_sent, p.forgotten_reason,
                p.cancelled_reason, p.cancelled_at, b.scope_kind, b.scope_stable_key, b.tmux_completion_policy,
                l.workflow_id AS link_workflow_id, l.delivery_id AS link_delivery_id,
                l.conversation_id AS link_conversation_id, l.message_id AS link_message_id,
                l.registering_tool_use_id, l.terminal_kind, l.auto_resume,
                l.created_at AS link_created_at,
                m.message_id, m.conversation_id, m.sequence_id, m.message_type, m.content,
                m.display_data, m.usage_data, m.created_at
         FROM workflow_deliveries d
         JOIN wake_terminal_receipts p
           ON p.workflow_id = d.workflow_id AND p.delivery_id = d.delivery_id
         JOIN wake_bindings b ON b.workflow_id = d.workflow_id
         JOIN wake_delivery_messages l
           ON l.workflow_id = d.workflow_id AND l.delivery_id = d.delivery_id
         JOIN messages m ON m.message_id = l.message_id
         WHERE d.workflow_id = ?1 AND d.status = 'Pending'
         ORDER BY d.delivery_id",
    )
    .bind(to_i64(workflow_id.0, "workflow_id")?)
    .fetch_all(&mut *tx.tx)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let receipt_id = ReceiptId(to_u64(row.get::<i64, _>("receipt_id"), "receipt_id")?);
        let tail = fetch_tail_lines_tx(tx, workflow_id, receipt_id).await?;
        let pending = WakePendingDelivery {
            workflow_id,
            conversation_id: row.get("conversation_id"),
            receipt: projection_from_row(&row, tail)?,
            canonical_delivery: delivery_from_join_row(&row)?,
        };
        out.push(WakeMaterializedPendingDelivery {
            pending,
            link: delivery_message_link_from_join_row(&row)?,
        });
    }
    Ok(out)
}

async fn fetch_any_terminal_projection_tx(
    tx: &mut WorkflowTx<'_>,
    workflow_id: WorkflowId,
) -> DbResult<Option<WakeTerminalReceiptProjection>> {
    let row = sqlx::query(
        "SELECT p.*, b.scope_kind, b.scope_stable_key, b.tmux_completion_policy
         FROM wake_terminal_receipts p
         JOIN wake_bindings b ON b.workflow_id = p.workflow_id
         WHERE p.workflow_id = ?1
         ORDER BY p.receipt_id
         LIMIT 1",
    )
    .bind(to_i64(workflow_id.0, "workflow_id")?)
    .fetch_optional(&mut *tx.tx)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let receipt_id = ReceiptId(to_u64(row.get::<i64, _>("receipt_id"), "receipt_id")?);
    let tail = fetch_tail_lines_tx(tx, workflow_id, receipt_id).await?;
    Ok(Some(projection_from_row(&row, tail)?))
}

async fn replay_terminal_projection(
    tx: &mut WorkflowTx<'_>,
    workflow_id: WorkflowId,
    existing: WakeTerminalReceiptProjection,
) -> DbResult<(LocalReceiptRecord, WakePendingDelivery)> {
    let delivery =
        fetch_pending_delivery_by_delivery_id_tx(&mut *tx, workflow_id, existing.delivery_id)
            .await?
            .ok_or_else(|| {
                DbError::Serialization("wake projection missing canonical delivery".to_string())
            })?;
    let receipt = fetch_receipt_tx(&mut *tx, workflow_id, existing.receipt_id)
        .await?
        .ok_or_else(|| {
            DbError::Serialization("wake projection missing canonical receipt".to_string())
        })?;
    Ok((
        receipt,
        WakePendingDelivery {
            workflow_id,
            conversation_id: existing.conversation_id.clone(),
            receipt: existing,
            canonical_delivery: delivery,
        },
    ))
}

async fn fetch_projection_by_receipt_tx(
    tx: &mut WorkflowTx<'_>,
    workflow_id: WorkflowId,
    receipt_id: ReceiptId,
) -> DbResult<Option<WakeTerminalReceiptProjection>> {
    let row = sqlx::query(
        "SELECT p.*, b.scope_kind, b.scope_stable_key, b.tmux_completion_policy
         FROM wake_terminal_receipts p
         JOIN wake_bindings b ON b.workflow_id = p.workflow_id
         WHERE p.workflow_id = ?1 AND p.receipt_id = ?2",
    )
    .bind(to_i64(workflow_id.0, "workflow_id")?)
    .bind(to_i64(receipt_id.0, "receipt_id")?)
    .fetch_optional(&mut *tx.tx)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let tail = fetch_tail_lines_tx(tx, workflow_id, receipt_id).await?;
    Ok(Some(projection_from_row(&row, tail)?))
}

async fn fetch_projection_for_attempt_tx(
    tx: &mut WorkflowTx<'_>,
    workflow_id: WorkflowId,
    attempt_id: AttemptId,
) -> DbResult<Option<WakeTerminalReceiptProjection>> {
    let row = sqlx::query(
        "SELECT p.*, b.scope_kind, b.scope_stable_key, b.tmux_completion_policy
         FROM wake_terminal_receipts p
         JOIN workflow_receipts r
           ON r.workflow_id = p.workflow_id AND r.receipt_id = p.receipt_id
         JOIN wake_bindings b ON b.workflow_id = p.workflow_id
         WHERE p.workflow_id = ?1 AND r.attempt_id = ?2",
    )
    .bind(to_i64(workflow_id.0, "workflow_id")?)
    .bind(to_i64(attempt_id.0, "attempt_id")?)
    .fetch_optional(&mut *tx.tx)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let receipt_id = ReceiptId(to_u64(row.get::<i64, _>("receipt_id"), "receipt_id")?);
    let tail = fetch_tail_lines_tx(tx, workflow_id, receipt_id).await?;
    Ok(Some(projection_from_row(&row, tail)?))
}

async fn fetch_tail_lines_tx(
    tx: &mut WorkflowTx<'_>,
    workflow_id: WorkflowId,
    receipt_id: ReceiptId,
) -> DbResult<Vec<String>> {
    Ok(sqlx::query_scalar::<_, String>(
        "SELECT line FROM wake_terminal_receipt_tails
         WHERE workflow_id = ?1 AND receipt_id = ?2
         ORDER BY ordinal",
    )
    .bind(to_i64(workflow_id.0, "workflow_id")?)
    .bind(to_i64(receipt_id.0, "receipt_id")?)
    .fetch_all(&mut *tx.tx)
    .await?)
}

fn projection_from_row(
    row: &sqlx::sqlite::SqliteRow,
    tail: Vec<String>,
) -> DbResult<WakeTerminalReceiptProjection> {
    let workflow_id = WorkflowId(to_u64(row.get::<i64, _>("workflow_id"), "workflow_id")?);
    let receipt_id = ReceiptId(to_u64(row.get::<i64, _>("receipt_id"), "receipt_id")?);
    let delivery_id = DeliveryId(to_u64(row.get::<i64, _>("delivery_id"), "delivery_id")?);
    let contract_id: String = row.get("contract_id");
    let conversation_id: String = row.get("conversation_id");
    let resource = resource_from_projection_row(row)?;
    let resolved_at = Timestamp(to_u64(row.get::<i64, _>("resolved_at"), "resolved_at")?);
    let terminal = match row.get::<String, _>("terminal_kind").as_str() {
        "Fired" => WakeTerminalPayload::Fired {
            contract_id: contract_id.clone(),
            resource: resource.clone(),
            evidence: evidence_from_projection_row(row, resource.clone(), tail)?,
            resolved_at,
        },
        "Expired" => WakeTerminalPayload::Expired {
            contract_id: contract_id.clone(),
            resource: resource.clone(),
            resolved_at,
        },
        "Forgotten" => WakeTerminalPayload::Forgotten {
            contract_id: contract_id.clone(),
            resource: resource.clone(),
            reason: match row.get::<String, _>("forgotten_reason").as_str() {
                "PhoenixRestart" => WakeForgottenReason::PhoenixRestart,
                "CascadeDestroyedHandle" => WakeForgottenReason::CascadeDestroyedHandle,
                "TmuxHandleMissing" => WakeForgottenReason::TmuxHandleMissing,
                other => {
                    return Err(DbError::Serialization(format!(
                        "unknown forgotten reason: {other}"
                    )))
                }
            },
            resolved_at,
        },
        "Cancelled" => WakeTerminalPayload::Cancelled {
            contract_id: contract_id.clone(),
            resource: resource.clone(),
            reason: match row.get::<String, _>("cancelled_reason").as_str() {
                "ExplicitCancel" => WakeCancellationReason::ExplicitCancel,
                other => {
                    return Err(DbError::Serialization(format!(
                        "unknown cancelled reason: {other}"
                    )))
                }
            },
            resolved_at: Timestamp(to_u64(row.get::<i64, _>("cancelled_at"), "cancelled_at")?),
        },
        other => {
            return Err(DbError::Serialization(format!(
                "unknown wake terminal kind: {other}"
            )))
        }
    };
    Ok(WakeTerminalReceiptProjection {
        workflow_id,
        receipt_id,
        delivery_id,
        conversation_id,
        contract_id,
        terminal,
    })
}

fn resource_from_projection_row(row: &sqlx::sqlite::SqliteRow) -> DbResult<WakeResourceIdentity> {
    let scope_kind = match row.get::<String, _>("scope_kind").as_str() {
        "Conversation" => wake_types::WorkScopeKind::Conversation,
        "Worktree" => wake_types::WorkScopeKind::Worktree,
        other => {
            return Err(DbError::Serialization(format!(
                "unknown projection scope kind: {other}"
            )))
        }
    };
    let scope_stable_key: String = row.get("scope_stable_key");
    match row.get::<String, _>("resource_kind").as_str() {
        "Bash" => Ok(WakeResourceIdentity::Bash(
            wake_types::BashResourceIdentity {
                work_scope: wake_types::WorkScopeIdentity {
                    kind: scope_kind,
                    stable_key: scope_stable_key,
                },
                handle_id: row.get("bash_handle_id"),
            },
        )),
        "TmuxWindow" => Ok(WakeResourceIdentity::TmuxWindow(
            wake_types::TmuxResourceIdentity {
                work_scope: wake_types::WorkScopeIdentity {
                    kind: scope_kind,
                    stable_key: scope_stable_key,
                },
                server_token: row.get("tmux_server_token"),
                window_id: row.get("tmux_window_id"),
                completion_policy: parse_tmux_completion_policy(
                    &row.get::<String, _>("tmux_completion_policy"),
                )?,
            },
        )),
        other => Err(DbError::Serialization(format!(
            "unknown wake resource kind: {other}"
        ))),
    }
}

fn evidence_from_projection_row(
    row: &sqlx::sqlite::SqliteRow,
    resource: WakeResourceIdentity,
    tail: Vec<String>,
) -> DbResult<WakeTerminalEvidence> {
    let occurred_at = Timestamp(to_u64(row.get::<i64, _>("occurred_at"), "occurred_at")?);
    Ok(match resource {
        WakeResourceIdentity::Bash(identity) => WakeTerminalEvidence::Bash(BashTerminalEvidence {
            identity,
            status: match row.get::<String, _>("bash_status").as_str() {
                "Exited" => wake_types::BashTerminalStatus::Exited,
                "Killed" => wake_types::BashTerminalStatus::Killed,
                "KillPendingKernel" => wake_types::BashTerminalStatus::KillPendingKernel,
                other => {
                    return Err(DbError::Serialization(format!(
                        "unknown bash status: {other}"
                    )))
                }
            },
            occurred_at,
            exit_code: row
                .get::<Option<i64>, _>("exit_code")
                .map(|v| i32::try_from(v).map_err(|e| DbError::Serialization(e.to_string())))
                .transpose()?,
            duration_ms: row
                .get::<Option<i64>, _>("duration_ms")
                .map(|v| u64::try_from(v).map_err(|e| DbError::Serialization(e.to_string())))
                .transpose()?,
            signal_number: row
                .get::<Option<i64>, _>("signal_number")
                .map(|v| i32::try_from(v).map_err(|e| DbError::Serialization(e.to_string())))
                .transpose()?,
            kill_signal_sent: row.get("kill_signal_sent"),
            final_tail: tail,
        }),
        WakeResourceIdentity::TmuxWindow(identity) => {
            WakeTerminalEvidence::TmuxWindow(TmuxTerminalEvidence {
                identity,
                status: match row.get::<String, _>("tmux_status").as_str() {
                    "ExitMarkerObserved" => wake_types::TmuxTerminalStatus::ExitMarkerObserved,
                    "WindowKilled" => wake_types::TmuxTerminalStatus::WindowKilled,
                    other => {
                        return Err(DbError::Serialization(format!(
                            "unknown tmux status: {other}"
                        )))
                    }
                },
                occurred_at,
                exit_code: row
                    .get::<Option<i64>, _>("exit_code")
                    .map(|v| i32::try_from(v).map_err(|e| DbError::Serialization(e.to_string())))
                    .transpose()?,
                duration_ms: row
                    .get::<Option<i64>, _>("duration_ms")
                    .map(|v| u64::try_from(v).map_err(|e| DbError::Serialization(e.to_string())))
                    .transpose()?,
                final_tail: tail,
            })
        }
        WakeResourceIdentity::Subagent(_) => unreachable!("subagent wake bindings not implemented"),
    })
}

async fn fetch_receipt_tx(
    tx: &mut WorkflowTx<'_>,
    workflow_id: WorkflowId,
    receipt_id: ReceiptId,
) -> DbResult<Option<LocalReceiptRecord>> {
    let row =
        sqlx::query("SELECT * FROM workflow_receipts WHERE workflow_id = ?1 AND receipt_id = ?2")
            .bind(to_i64(workflow_id.0, "workflow_id")?)
            .bind(to_i64(receipt_id.0, "receipt_id")?)
            .fetch_optional(&mut *tx.tx)
            .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    Ok(Some(LocalReceiptRecord {
        receipt_id,
        workflow_id,
        effect_id: EffectId(to_u64(row.get::<i64, _>("effect_id"), "effect_id")?),
        generation: Generation(to_u64(row.get::<i64, _>("generation"), "generation")?),
        declared_workflow_version: Version(to_u64(
            row.get::<i64, _>("declared_workflow_version"),
            "declared_workflow_version",
        )?),
        process_incarnation: ProcessIncarnation(to_u64(
            row.get::<i64, _>("process_incarnation"),
            "process_incarnation",
        )?),
        attempt_id: row
            .get::<Option<i64>, _>("attempt_id")
            .map(|v| AttemptId(to_u64(v, "attempt_id").unwrap())),
        origin: match row.get::<String, _>("origin").as_str() {
            "Execution" => ReceiptOrigin::Execution,
            "Adoption" => ReceiptOrigin::Adoption,
            "Reconciliation" => ReceiptOrigin::Reconciliation,
            "Manual" => ReceiptOrigin::Manual,
            "CancellationArbitration" => ReceiptOrigin::CancellationArbitration,
            "DeadlineExpiration" => ReceiptOrigin::DeadlineExpiration,
            "ForgottenInterruption" => ReceiptOrigin::ForgottenInterruption,
            "ScheduleCollapse" => ReceiptOrigin::ScheduleCollapse,
            other => {
                return Err(DbError::Serialization(format!(
                    "unknown receipt origin: {other}"
                )))
            }
        },
        receipt_codec: LocalCodec {
            family: row.get("receipt_codec_family"),
            version: u32::try_from(row.get::<i64, _>("receipt_codec_version"))
                .map_err(|e| DbError::Serialization(e.to_string()))?,
        },
        receipt_payload: row.get("receipt_payload"),
    }))
}

async fn fetch_pending_delivery_by_delivery_id_tx(
    tx: &mut WorkflowTx<'_>,
    workflow_id: WorkflowId,
    delivery_id: DeliveryId,
) -> DbResult<Option<LocalDeliveryRecord>> {
    let row = sqlx::query(
        "SELECT * FROM workflow_deliveries WHERE workflow_id = ?1 AND delivery_id = ?2",
    )
    .bind(to_i64(workflow_id.0, "workflow_id")?)
    .bind(to_i64(delivery_id.0, "delivery_id")?)
    .fetch_optional(&mut *tx.tx)
    .await?;
    row.as_ref().map(delivery_from_join_row).transpose()
}

fn delivery_from_join_row(row: &sqlx::sqlite::SqliteRow) -> DbResult<LocalDeliveryRecord> {
    Ok(LocalDeliveryRecord {
        delivery_id: DeliveryId(to_u64(row.get::<i64, _>("delivery_id"), "delivery_id")?),
        workflow_id: WorkflowId(to_u64(row.get::<i64, _>("workflow_id"), "workflow_id")?),
        effect_id: row
            .get::<Option<i64>, _>("effect_id")
            .map(|v| EffectId(to_u64(v, "effect_id").unwrap())),
        barrier_id: None,
        consumer_kind: row.get("consumer_kind"),
        event_codec: LocalCodec {
            family: row.get("event_codec_family"),
            version: u32::try_from(row.get::<i64, _>("event_codec_version"))
                .map_err(|e| DbError::Serialization(e.to_string()))?,
        },
        payload_kind: super::LocalDeliveryPayloadKind::Receipt,
        payload_blob: row.get("payload_blob"),
        requires_runtime_acceptance: row.get("requires_runtime_acceptance"),
        status: match row.get::<String, _>("status").as_str() {
            "Pending" => phoenix_workflow::DeliveryStatus::Pending,
            "Deferred" => phoenix_workflow::DeliveryStatus::Deferred,
            "Accepted" => phoenix_workflow::DeliveryStatus::Accepted,
            "Suppressed" => phoenix_workflow::DeliveryStatus::Suppressed,
            other => {
                return Err(DbError::Serialization(format!(
                    "unknown delivery status: {other}"
                )))
            }
        },
        runtime_acceptance_status: match row
            .get::<Option<String>, _>("runtime_acceptance_status")
            .as_deref()
        {
            Some("Owed") => Some(phoenix_workflow::RuntimeAcceptanceStatus::Owed),
            Some("Accepted") => Some(phoenix_workflow::RuntimeAcceptanceStatus::Accepted),
            Some("Suppressed") => Some(phoenix_workflow::RuntimeAcceptanceStatus::Suppressed),
            None => None,
            Some(other) => {
                return Err(DbError::Serialization(format!(
                    "unknown runtime acceptance status: {other}"
                )))
            }
        },
        suppression_reason: None,
        accepted_by_transition_id: None,
    })
}

fn wake_delivery_message_id(workflow_id: WorkflowId, delivery_id: DeliveryId) -> String {
    format!("wake-{}-{}-result", workflow_id.0, delivery_id.0)
}

fn timestamp_to_datetime(timestamp: Timestamp) -> DateTime<Utc> {
    DateTime::from_timestamp(i64::try_from(timestamp.0).unwrap_or(0), 0)
        .unwrap_or(DateTime::<Utc>::UNIX_EPOCH)
}

fn message_from_join_row(row: &sqlx::sqlite::SqliteRow) -> Result<Message, sqlx::Error> {
    let msg_type = parse_message_type_local(&row.try_get::<String, _>("message_type")?);
    let content_str: String = row.try_get("content")?;
    let content_value: serde_json::Value = serde_json::from_str(&content_str).unwrap_or_default();
    let content = MessageContent::from_stored_json(msg_type, content_value)
        .unwrap_or_else(|_| MessageContent::error(format!("Failed to parse {msg_type} message")));
    Ok(Message {
        message_id: row.try_get("message_id")?,
        conversation_id: row.try_get("conversation_id")?,
        sequence_id: row.try_get("sequence_id")?,
        message_type: msg_type,
        content,
        display_data: row
            .try_get::<Option<String>, _>("display_data")?
            .map(|s| serde_json::from_str(&s).unwrap_or_default()),
        usage_data: row
            .try_get::<Option<String>, _>("usage_data")?
            .and_then(|s| serde_json::from_str(&s).ok()),
        created_at: DateTime::parse_from_rfc3339(&row.try_get::<String, _>("created_at")?)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or(DateTime::<Utc>::UNIX_EPOCH),
    })
}

fn parse_message_type_local(s: &str) -> phoenix_core::domain::db_schema::MessageType {
    serde_json::from_value(serde_json::Value::String(s.to_string()))
        .unwrap_or(phoenix_core::domain::db_schema::MessageType::System)
}

fn is_unique_or_primary_constraint(error: &dyn sqlx::error::DatabaseError) -> bool {
    let message = error.message();
    message.contains("UNIQUE constraint failed")
        || message.contains("PRIMARY KEY constraint failed")
}

async fn has_message_fts_tx(tx: &mut WorkflowTx<'_>) -> DbResult<bool> {
    let exists = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'message_fts'",
    )
    .fetch_one(&mut *tx.tx)
    .await?;
    Ok(exists > 0)
}

async fn fetch_delivery_message_link_tx(
    tx: &mut WorkflowTx<'_>,
    workflow_id: WorkflowId,
    delivery_id: DeliveryId,
) -> DbResult<Option<WakeDeliveryMessageLink>> {
    let row = sqlx::query(
        "SELECT l.workflow_id, l.delivery_id, l.conversation_id AS link_conversation_id,
                l.message_id AS link_message_id, l.registering_tool_use_id, l.terminal_kind,
                l.auto_resume, l.created_at AS link_created_at,
                m.message_id, m.conversation_id, m.sequence_id, m.message_type, m.content,
                m.display_data, m.usage_data, m.created_at
         FROM wake_delivery_messages l
         JOIN messages m ON m.message_id = l.message_id
         WHERE l.workflow_id = ?1 AND l.delivery_id = ?2",
    )
    .bind(to_i64(workflow_id.0, "workflow_id")?)
    .bind(to_i64(delivery_id.0, "delivery_id")?)
    .fetch_optional(&mut *tx.tx)
    .await?;
    row.as_ref()
        .map(delivery_message_link_from_join_row)
        .transpose()
}

fn delivery_message_link_from_join_row(
    row: &sqlx::sqlite::SqliteRow,
) -> DbResult<WakeDeliveryMessageLink> {
    let message = message_from_join_row(row)?;
    Ok(WakeDeliveryMessageLink {
        workflow_id: WorkflowId(to_u64(row.get::<i64, _>("workflow_id"), "workflow_id")?),
        delivery_id: DeliveryId(to_u64(row.get::<i64, _>("delivery_id"), "delivery_id")?),
        conversation_id: row.get("link_conversation_id"),
        message_id: row.get("link_message_id"),
        registering_tool_use_id: row.get("registering_tool_use_id"),
        terminal_kind: row.get("terminal_kind"),
        auto_resume: row.get::<i64, _>("auto_resume") != 0,
        created_at: Timestamp(to_u64(
            row.get::<i64, _>("link_created_at"),
            "link_created_at",
        )?),
        linked_message: WakeDeliveryLinkedMessage { message },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrations::run_pending_migrations;
    use crate::workflow::WorkflowHead;
    use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
    use std::str::FromStr;

    async fn setup_repo_schema(pool: &sqlx::SqlitePool) {
        sqlx::query("CREATE TABLE conversations (id TEXT PRIMARY KEY, conv_mode TEXT NOT NULL DEFAULT '{\"mode\":\"Explore\"}', state TEXT NOT NULL DEFAULT '{\"type\":\"idle\"}', cwd TEXT NOT NULL DEFAULT '/tmp', parent_conversation_id TEXT, user_initiated BOOLEAN NOT NULL DEFAULT 1, archived BOOLEAN NOT NULL DEFAULT 0, model TEXT, steering_queue TEXT NOT NULL DEFAULT '[]', state_updated_at TEXT NOT NULL DEFAULT '2025-01-01', created_at TEXT NOT NULL DEFAULT '2025-01-01', updated_at TEXT NOT NULL DEFAULT '2025-01-01')").execute(pool).await.unwrap();
        sqlx::query("CREATE TABLE messages (message_id TEXT PRIMARY KEY, conversation_id TEXT NOT NULL, sequence_id INTEGER NOT NULL DEFAULT 1, message_type TEXT NOT NULL, content TEXT NOT NULL, display_data TEXT, usage_data TEXT, created_at TEXT NOT NULL DEFAULT '2025-01-01')").execute(pool).await.unwrap();
        run_pending_migrations(pool).await.unwrap();
        sqlx::query("INSERT OR IGNORE INTO conversations (id) VALUES ('conv-1')")
            .execute(pool)
            .await
            .unwrap();
    }

    async fn open_repo_pair() -> (tempfile::TempDir, WakeRepository, WakeRepository) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wake.db");
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
            WakeRepository::new(pool)
        };
        (dir, open().await, open().await)
    }

    fn tmux_intent() -> WakeRegistrationIntent {
        WakeRegistrationIntent {
            contract_id: "contract-2".into(),
            conversation_id: "conv-1".into(),
            registration_scope: wake_types::WorkScopeIdentity {
                kind: wake_types::WorkScopeKind::Conversation,
                stable_key: "conv-1".into(),
            },
            resource: WakeResourceIdentity::TmuxWindow(wake_types::TmuxResourceIdentity {
                work_scope: wake_types::WorkScopeIdentity {
                    kind: wake_types::WorkScopeKind::Conversation,
                    stable_key: "conv-1".into(),
                },
                server_token: "srv-1".into(),
                window_id: "win-1".into(),
                completion_policy: wake_types::TmuxCompletionPolicy::KeepOpen,
            }),
            registering_tool_use_id: "tool-2".into(),
            registered_at: Timestamp(10),
            expires_at: Timestamp(100),
        }
    }

    fn bash_evidence(occurred_at: u64) -> WakeTerminalEvidence {
        WakeTerminalEvidence::Bash(BashTerminalEvidence {
            identity: wake_types::BashResourceIdentity {
                work_scope: wake_types::WorkScopeIdentity {
                    kind: wake_types::WorkScopeKind::Conversation,
                    stable_key: "conv-1".into(),
                },
                handle_id: "b-1".into(),
            },
            status: wake_types::BashTerminalStatus::Exited,
            occurred_at: Timestamp(occurred_at),
            exit_code: Some(0),
            duration_ms: Some(12),
            signal_number: None,
            kill_signal_sent: None,
            final_tail: vec!["done".into(), "ok".into()],
        })
    }

    fn tmux_evidence(occurred_at: u64) -> WakeTerminalEvidence {
        WakeTerminalEvidence::TmuxWindow(TmuxTerminalEvidence {
            identity: wake_types::TmuxResourceIdentity {
                work_scope: wake_types::WorkScopeIdentity {
                    kind: wake_types::WorkScopeKind::Conversation,
                    stable_key: "conv-1".into(),
                },
                server_token: "srv-1".into(),
                window_id: "win-1".into(),
                completion_policy: wake_types::TmuxCompletionPolicy::KeepOpen,
            },
            status: wake_types::TmuxTerminalStatus::ExitMarkerObserved,
            occurred_at: Timestamp(occurred_at),
            exit_code: Some(7),
            duration_ms: Some(33),
            final_tail: vec!["tail-1".into()],
        })
    }

    fn cancel_input(workflow_id: WorkflowId) -> WakeCancellationInput {
        WakeCancellationInput {
            workflow_id,
            expected_version: Version(1),
            expected_generation: Generation(0),
            receipt_id: ReceiptId(1),
            delivery_id: DeliveryId(1),
            timestamp: Timestamp(20),
            reason: WakeCancellationReason::ExplicitCancel,
        }
    }

    async fn register_and_begin(
        repo: &WakeRepository,
        workflow_id: WorkflowId,
    ) -> super::WakeObservationOutcome {
        let input = intent();
        assert!(matches!(
            repo.register_allocated(workflow_id, &input, "fp-1", Timestamp(10))
                .await
                .unwrap(),
            WakeRegistrationOutcome::Registered { .. }
        ));
        repo.claim_observation_if_eligible(
            workflow_id,
            ProcessIncarnation(1),
            Timestamp(20),
            phoenix_workflow::LeaseExpiry(30),
        )
        .await
        .unwrap()
    }

    fn unwrap_started(outcome: super::WakeObservationOutcome) -> super::BeginAttemptResult {
        match outcome {
            super::WakeObservationOutcome::Started { canonical } => canonical,
            other @ (WakeObservationOutcome::Busy { .. } | WakeObservationOutcome::Ineligible) => {
                panic!("expected started, got {other:?}")
            }
        }
    }

    fn intent() -> WakeRegistrationIntent {
        WakeRegistrationIntent {
            contract_id: "contract-1".into(),
            conversation_id: "conv-1".into(),
            registration_scope: wake_types::WorkScopeIdentity {
                kind: wake_types::WorkScopeKind::Conversation,
                stable_key: "conv-1".into(),
            },
            resource: WakeResourceIdentity::Bash(wake_types::BashResourceIdentity {
                work_scope: wake_types::WorkScopeIdentity {
                    kind: wake_types::WorkScopeKind::Conversation,
                    stable_key: "conv-1".into(),
                },
                handle_id: "b-1".into(),
            }),
            registering_tool_use_id: "tool-1".into(),
            registered_at: Timestamp(10),
            expires_at: Timestamp(100),
        }
    }

    fn resolve_input(
        workflow_id: WorkflowId,
        expected_version: Version,
        transition_id: TransitionId,
        delivery_ids: Vec<DeliveryId>,
    ) -> WakeResolvePendingInput {
        WakeResolvePendingInput {
            workflow_id,
            expected_version,
            delivery_ids,
            decision: WakeResolveDecision::Accept,
            transition_id,
            timestamp: Timestamp(30),
        }
    }

    fn decode_snapshot(head: &WorkflowHead) -> WakeRegistrationSnapshot {
        serde_json::from_slice(&head.snapshot_payload).expect("snapshot decodes")
    }

    async fn head_snapshot(
        repo: &WakeRepository,
        workflow_id: WorkflowId,
    ) -> (WorkflowHead, WakeRegistrationSnapshot) {
        let head = repo
            .workflow_repo
            .fetch_workflow_head(workflow_id)
            .await
            .unwrap()
            .unwrap();
        let snapshot = decode_snapshot(&head);
        (head, snapshot)
    }

    async fn assert_snapshot_projection_parity(
        repo: &WakeRepository,
        workflow_id: WorkflowId,
        conversation_id: &str,
        expected_runtime: wake_profile::RuntimeAvailability,
        expect_pending_delivery: bool,
        expected_terminal: fn(&WakeTerminalPayload) -> bool,
    ) {
        let (_head, snapshot) = head_snapshot(repo, workflow_id).await;
        assert!(expected_terminal(
            snapshot.terminal.as_ref().expect("terminal snapshot")
        ));
        assert_eq!(snapshot.runtime_availability, expected_runtime);

        let pending = repo.list_pending(conversation_id).await.unwrap();
        if expect_pending_delivery {
            assert_eq!(pending.len(), 1);
            let item = pending
                .iter()
                .find(|item| item.workflow_id == workflow_id)
                .expect("pending delivery for workflow");
            assert!(expected_terminal(&item.receipt.terminal));
        } else {
            assert!(pending.iter().all(|item| item.workflow_id != workflow_id));
        }

        let projection = fetch_any_terminal_projection_tx(
            &mut repo.workflow_repo.begin_tx().await.unwrap(),
            workflow_id,
        )
        .await
        .unwrap()
        .unwrap();
        assert!(expected_terminal(&projection.terminal));
        assert_eq!(projection.conversation_id, conversation_id);

        let deliveries = repo
            .workflow_repo
            .list_deliveries(workflow_id)
            .await
            .unwrap();
        let has_pending = deliveries
            .iter()
            .any(|d| d.status == phoenix_workflow::DeliveryStatus::Pending);
        assert_eq!(has_pending, expect_pending_delivery);
    }

    fn is_fired(terminal: &WakeTerminalPayload) -> bool {
        matches!(terminal, WakeTerminalPayload::Fired { .. })
    }

    fn is_cancelled(terminal: &WakeTerminalPayload) -> bool {
        matches!(terminal, WakeTerminalPayload::Cancelled { .. })
    }

    fn is_expired(terminal: &WakeTerminalPayload) -> bool {
        matches!(terminal, WakeTerminalPayload::Expired { .. })
    }

    async fn fetch_receipt_origin(
        repo: &WakeRepository,
        workflow_id: WorkflowId,
        receipt_id: ReceiptId,
    ) -> ReceiptOrigin {
        let mut tx = repo.workflow_repo.begin_tx().await.unwrap();
        fetch_receipt_tx(&mut tx, workflow_id, receipt_id)
            .await
            .unwrap()
            .unwrap()
            .origin
    }

    fn transfer_input(
        workflow_id: WorkflowId,
        from_conversation_id: &str,
        to_conversation_id: &str,
        expected_version: Version,
        exact_pending_delivery_ids: Vec<DeliveryId>,
        transition_id: TransitionId,
    ) -> WakeTransferInput {
        WakeTransferInput {
            workflow_id,
            from_conversation_id: from_conversation_id.into(),
            to_conversation_id: to_conversation_id.into(),
            expected_version,
            exact_pending_delivery_ids,
            transition_id,
            timestamp: Timestamp(30),
        }
    }

    async fn insert_conversation(pool: &sqlx::SqlitePool, id: &str) {
        sqlx::query("INSERT OR IGNORE INTO conversations (id) VALUES (?1)")
            .bind(id)
            .execute(pool)
            .await
            .unwrap();
    }

    async fn external_acceptance_identity(
        repo: &WakeRepository,
        workflow_id: WorkflowId,
    ) -> Option<(String, String)> {
        sqlx::query(
            "SELECT target_scope, idempotency_key
             FROM workflow_external_acceptance_bindings
             WHERE workflow_id = ?1",
        )
        .bind(to_i64(workflow_id.0, "workflow_id").unwrap())
        .fetch_optional(&repo.workflow_repo.pool)
        .await
        .unwrap()
        .map(|row| (row.get("target_scope"), row.get("idempotency_key")))
    }

    async fn registered_workflow_id(
        repo: &WakeRepository,
        input: &WakeRegistrationIntent,
        fingerprint: &str,
    ) -> WorkflowId {
        match repo
            .register(input, fingerprint, Timestamp(10))
            .await
            .unwrap()
        {
            WakeRegistrationOutcome::Registered { workflow_id, .. }
            | WakeRegistrationOutcome::Replayed { workflow_id, .. } => workflow_id,
            WakeRegistrationOutcome::Conflict => panic!("expected registered or replayed"),
        }
    }

    async fn create_pending_terminal_delivery(
        repo: &WakeRepository,
        workflow_id: WorkflowId,
    ) -> WakePendingDelivery {
        let canonical = unwrap_started(register_and_begin(repo, workflow_id).await);
        match repo
            .record_terminal_evidence(
                workflow_id,
                canonical.authority.as_ref().expect("authority"),
                1,
                ReceiptId(1),
                DeliveryId(1),
                Timestamp(20),
                &bash_evidence(19),
            )
            .await
            .unwrap()
        {
            WakeTerminalEvidenceOutcome::Recorded { delivery, .. }
            | WakeTerminalEvidenceOutcome::Replayed { delivery, .. } => delivery,
            other @ (WakeTerminalEvidenceOutcome::StaleAttempt
            | WakeTerminalEvidenceOutcome::WrongResource
            | WakeTerminalEvidenceOutcome::EvidenceAfterObservation
            | WakeTerminalEvidenceOutcome::EvidenceAfterExpiry) => {
                panic!("expected recorded/replayed pending delivery, got {other:?}")
            }
        }
    }

    fn materialize_input(
        pending: &WakePendingDelivery,
        rendered_content: &str,
        display_data: Option<serde_json::Value>,
        auto_resume: bool,
        created_at: Timestamp,
    ) -> MaterializePendingDeliveryMessageInput {
        MaterializePendingDeliveryMessageInput {
            workflow_id: pending.workflow_id,
            delivery_id: pending.canonical_delivery.delivery_id,
            conversation_id: pending.conversation_id.clone(),
            rendered_content: rendered_content.to_string(),
            display_data,
            auto_resume,
            created_at,
            sequence_id: None,
        }
    }

    async fn materialize_pending(
        repo: &WakeRepository,
        pending: &WakePendingDelivery,
        rendered_content: &str,
        display_data: Option<serde_json::Value>,
        auto_resume: bool,
        created_at: Timestamp,
    ) -> MaterializePendingDeliveryMessageOutcome {
        repo.materialize_pending_delivery_message(&materialize_input(
            pending,
            rendered_content,
            display_data,
            auto_resume,
            created_at,
        ))
        .await
        .unwrap()
    }

    async fn count_conversation_messages(repo: &WakeRepository, conversation_id: &str) -> i64 {
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM messages WHERE conversation_id = ?1")
            .bind(conversation_id)
            .fetch_one(&repo.workflow_repo.pool)
            .await
            .unwrap()
    }

    async fn count_delivery_message_links(repo: &WakeRepository) -> i64 {
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM wake_delivery_messages")
            .fetch_one(&repo.workflow_repo.pool)
            .await
            .unwrap()
    }

    async fn insert_unrelated_message(
        repo: &WakeRepository,
        conversation_id: &str,
        message_id: &str,
    ) {
        let content = MessageContent::system("unrelated");
        sqlx::query(
            "INSERT INTO messages (message_id, conversation_id, sequence_id, message_type, content, display_data, usage_data, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, NULL, NULL, ?6)",
        )
        .bind(message_id)
        .bind(conversation_id)
        .bind(1_i64)
        .bind(content.message_type().to_string())
        .bind(serde_json::to_string(&content.to_stored_json()).unwrap())
        .bind(timestamp_to_datetime(Timestamp(11)).to_rfc3339())
        .execute(&repo.workflow_repo.pool)
        .await
        .unwrap();
    }

    async fn fetch_conversation_messages(
        repo: &WakeRepository,
        conversation_id: &str,
    ) -> Vec<Message> {
        sqlx::query(
            "SELECT message_id, conversation_id, sequence_id, message_type, content, display_data, usage_data, created_at
             FROM messages WHERE conversation_id = ?1 ORDER BY sequence_id ASC",
        )
        .bind(conversation_id)
        .fetch_all(&repo.workflow_repo.pool)
        .await
        .unwrap()
        .iter()
        .map(|row| message_from_join_row(row).unwrap())
        .collect()
    }

    fn materialized_outcome_link(
        outcome: MaterializePendingDeliveryMessageOutcome,
    ) -> WakeDeliveryMessageLink {
        match outcome {
            MaterializePendingDeliveryMessageOutcome::Materialized(link)
            | MaterializePendingDeliveryMessageOutcome::AlreadyMaterialized(link) => link,
            MaterializePendingDeliveryMessageOutcome::WrongOwnerOrIneligible => {
                panic!("expected materialized/already-materialized link")
            }
        }
    }

    #[tokio::test]
    async fn materialize_pending_delivery_message_creates_meta_user_message_with_optional_sequence()
    {
        let (_dir, repo, _) = open_repo_pair().await;
        let workflow_id = WorkflowId(800);
        let pending = create_pending_terminal_delivery(&repo, workflow_id).await;
        insert_unrelated_message(&repo, &pending.conversation_id, "existing-msg").await;
        let display_data = serde_json::json!({
            "terminal_kind": "bash",
            "final_tail": ["done", "ok"],
            "exit_code": 0,
            "duration_ms": 12
        });

        let outcome = repo
            .materialize_pending_delivery_message(&MaterializePendingDeliveryMessageInput {
                workflow_id: pending.workflow_id,
                delivery_id: pending.canonical_delivery.delivery_id,
                conversation_id: pending.conversation_id.clone(),
                rendered_content: "bash finished normally".to_string(),
                display_data: Some(display_data.clone()),
                auto_resume: true,
                created_at: Timestamp(42),
                sequence_id: Some(17),
            })
            .await
            .unwrap();

        let link = match outcome {
            MaterializePendingDeliveryMessageOutcome::Materialized(link) => link,
            other @ (MaterializePendingDeliveryMessageOutcome::AlreadyMaterialized(_)
            | MaterializePendingDeliveryMessageOutcome::WrongOwnerOrIneligible) => {
                panic!("expected materialized, got {other:?}")
            }
        };
        let expected_message_id = wake_delivery_message_id(workflow_id, DeliveryId(1));
        assert_eq!(link.workflow_id, workflow_id);
        assert_eq!(link.delivery_id, DeliveryId(1));
        assert_eq!(link.conversation_id, pending.conversation_id);
        assert_eq!(link.message_id, expected_message_id);
        assert_eq!(link.registering_tool_use_id, "tool-1");
        assert_eq!(link.terminal_kind, "Fired");
        assert!(link.auto_resume);
        assert_eq!(link.created_at, Timestamp(42));

        let message = &link.linked_message.message;
        assert_eq!(message.message_id, expected_message_id);
        assert_eq!(message.conversation_id, "conv-1");
        assert_eq!(message.sequence_id, 17);
        assert_eq!(message.display_data, Some(display_data.clone()));
        assert_eq!(message.created_at, timestamp_to_datetime(Timestamp(42)));
        match &message.content {
            MessageContent::User(user) => {
                assert_eq!(user.text, "bash finished normally");
                assert!(user.is_meta);
            }
            other @ (MessageContent::Tool(_)
            | MessageContent::Agent(_)
            | MessageContent::System(_)
            | MessageContent::Error(_)
            | MessageContent::Continuation(_)
            | MessageContent::Skill(_)) => {
                panic!("expected meta user content, got {other:?}")
            }
        }

        let messages = fetch_conversation_messages(&repo, "conv-1").await;
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1].message_id, expected_message_id);
        assert_eq!(messages[1].conversation_id, message.conversation_id);
        assert_eq!(messages[1].sequence_id, message.sequence_id);
        assert_eq!(messages[1].message_type, message.message_type);
        assert_eq!(messages[1].display_data, message.display_data);
    }

    #[tokio::test]
    async fn materialize_pending_delivery_message_is_idempotent_across_two_repos() {
        let (_dir, first, second) = open_repo_pair().await;
        let workflow_id = WorkflowId(801);
        let pending = create_pending_terminal_delivery(&first, workflow_id).await;
        let input = materialize_input(
            &pending,
            "same wake result",
            Some(serde_json::json!({"kind": "wake"})),
            false,
            Timestamp(43),
        );

        let (left, right) = tokio::join!(
            first.materialize_pending_delivery_message(&input),
            second.materialize_pending_delivery_message(&input)
        );
        let outcomes = [left.unwrap(), right.unwrap()];
        assert_eq!(
            outcomes
                .iter()
                .filter(|o| matches!(o, MaterializePendingDeliveryMessageOutcome::Materialized(_)))
                .count(),
            1
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|o| matches!(
                    o,
                    MaterializePendingDeliveryMessageOutcome::AlreadyMaterialized(_)
                ))
                .count(),
            1
        );

        let first_link = materialized_outcome_link(outcomes[0].clone());
        let second_link = materialized_outcome_link(outcomes[1].clone());
        assert_eq!(first_link.message_id, second_link.message_id);
        assert_eq!(count_conversation_messages(&first, "conv-1").await, 1);
        assert_eq!(count_delivery_message_links(&first).await, 1);
    }

    #[tokio::test]
    async fn materialize_pending_delivery_message_rejects_wrong_owner_missing_and_resolved() {
        let (_dir, repo, _) = open_repo_pair().await;
        insert_conversation(&repo.workflow_repo.pool, "conv-2").await;
        let workflow_id = WorkflowId(802);
        let pending = create_pending_terminal_delivery(&repo, workflow_id).await;

        let mut wrong_owner = materialize_input(&pending, "ignored", None, false, Timestamp(44));
        wrong_owner.conversation_id = "conv-2".into();
        assert!(matches!(
            repo.materialize_pending_delivery_message(&wrong_owner)
                .await
                .unwrap(),
            MaterializePendingDeliveryMessageOutcome::WrongOwnerOrIneligible
        ));
        assert_eq!(count_conversation_messages(&repo, "conv-1").await, 0);
        assert_eq!(count_delivery_message_links(&repo).await, 0);

        let missing = MaterializePendingDeliveryMessageInput {
            workflow_id,
            delivery_id: DeliveryId(999),
            conversation_id: "conv-1".into(),
            rendered_content: "ignored".into(),
            display_data: None,
            auto_resume: false,
            created_at: Timestamp(45),
            sequence_id: None,
        };
        assert!(matches!(
            repo.materialize_pending_delivery_message(&missing)
                .await
                .unwrap(),
            MaterializePendingDeliveryMessageOutcome::WrongOwnerOrIneligible
        ));
        assert_eq!(count_conversation_messages(&repo, "conv-1").await, 0);
        assert_eq!(count_delivery_message_links(&repo).await, 0);

        assert_eq!(
            repo.resolve_pending_exact(&resolve_input(
                workflow_id,
                Version(2),
                TransitionId(3),
                vec![pending.canonical_delivery.delivery_id],
            ))
            .await
            .unwrap(),
            WakeResolvePendingOutcome::Resolved
        );
        assert!(matches!(
            materialize_pending(&repo, &pending, "ignored", None, false, Timestamp(46)).await,
            MaterializePendingDeliveryMessageOutcome::WrongOwnerOrIneligible
        ));
        assert_eq!(count_conversation_messages(&repo, "conv-1").await, 0);
        assert_eq!(count_delivery_message_links(&repo).await, 0);
    }

    #[tokio::test]
    async fn restart_repo_get_delivery_message_link_returns_materialized_message() {
        let (_dir, repo, restarted) = open_repo_pair().await;
        let workflow_id = WorkflowId(803);
        let pending = create_pending_terminal_delivery(&repo, workflow_id).await;
        let created = materialized_outcome_link(
            materialize_pending(
                &repo,
                &pending,
                "restart lookup",
                Some(serde_json::json!({"source": "wake"})),
                false,
                Timestamp(47),
            )
            .await,
        );

        let loaded = restarted
            .get_delivery_message_link(workflow_id, pending.canonical_delivery.delivery_id)
            .await
            .unwrap()
            .expect("linked message after restart");
        assert_eq!(loaded.message_id, created.message_id);
        assert_eq!(
            loaded.registering_tool_use_id,
            created.registering_tool_use_id
        );
        assert_eq!(
            loaded.linked_message.message.message_id,
            created.linked_message.message.message_id
        );
        assert_eq!(
            loaded.linked_message.message.sequence_id,
            created.linked_message.message.sequence_id
        );
        assert_eq!(
            loaded.linked_message.message.display_data,
            created.linked_message.message.display_data
        );
    }

    #[tokio::test]
    async fn list_linked_pending_delivery_messages_tracks_materialized_until_resolved() {
        let (_dir, repo, _) = open_repo_pair().await;
        let workflow_id = WorkflowId(804);
        let pending = create_pending_terminal_delivery(&repo, workflow_id).await;
        let created = materialized_outcome_link(
            materialize_pending(
                &repo,
                &pending,
                "list pending",
                Some(serde_json::json!({"state": "pending"})),
                true,
                Timestamp(48),
            )
            .await,
        );

        let listed = repo
            .list_linked_pending_delivery_messages("conv-1")
            .await
            .unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].message_id, created.message_id);
        assert_eq!(
            listed[0].delivery_id,
            pending.canonical_delivery.delivery_id
        );

        assert_eq!(
            repo.resolve_pending_exact(&resolve_input(
                workflow_id,
                Version(2),
                TransitionId(3),
                vec![pending.canonical_delivery.delivery_id],
            ))
            .await
            .unwrap(),
            WakeResolvePendingOutcome::Resolved
        );
        assert!(repo
            .list_linked_pending_delivery_messages("conv-1")
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn resolve_materialized_pending_updates_snapshot_and_preserves_link() {
        let (_dir, repo, _) = open_repo_pair().await;
        let workflow_id = WorkflowId(807);
        let pending = create_pending_terminal_delivery(&repo, workflow_id).await;
        let link = materialized_outcome_link(
            materialize_pending(&repo, &pending, "wake complete", None, true, Timestamp(50)).await,
        );

        let outcome = repo
            .resolve_materialized_pending_for_workflow(
                workflow_id,
                WakeResolveMaterializedDecision::Accept,
                Timestamp(51),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            outcome,
            WakeResolveMaterializedPendingOutcome::Resolved {
                delivery_ids: vec![pending.canonical_delivery.delivery_id],
                auto_resume: true,
            }
        );
        assert_snapshot_projection_parity(
            &repo,
            workflow_id,
            "conv-1",
            wake_profile::RuntimeAvailability::Accepted,
            false,
            is_fired,
        )
        .await;
        assert_eq!(
            repo.get_delivery_message_link(workflow_id, pending.canonical_delivery.delivery_id)
                .await
                .unwrap()
                .unwrap()
                .message_id,
            link.message_id
        );
    }

    #[tokio::test]
    async fn resolve_materialized_pending_rejects_incomplete_set_without_mutation() {
        let (_dir, repo, _) = open_repo_pair().await;
        let workflow_id = WorkflowId(808);
        let pending = create_pending_terminal_delivery(&repo, workflow_id).await;
        let (head_before, snapshot_before) = head_snapshot(&repo, workflow_id).await;

        let outcome = repo
            .resolve_materialized_pending_for_workflow(
                workflow_id,
                WakeResolveMaterializedDecision::Accept,
                Timestamp(51),
            )
            .await
            .unwrap();
        assert_eq!(
            outcome,
            Err(WakeResolveMaterializedPendingError::NotFullyMaterialized {
                delivery_ids: vec![pending.canonical_delivery.delivery_id],
            })
        );
        let (head_after, snapshot_after) = head_snapshot(&repo, workflow_id).await;
        assert_eq!(head_after.version, head_before.version);
        assert_eq!(snapshot_after, snapshot_before);
        assert_eq!(repo.list_pending("conv-1").await.unwrap().len(), 1);
        assert_eq!(count_conversation_messages(&repo, "conv-1").await, 0);
    }

    #[tokio::test]
    async fn resolve_materialized_pending_is_idempotent_across_two_repos() {
        let (_dir, first, second) = open_repo_pair().await;
        let workflow_id = WorkflowId(809);
        let pending = create_pending_terminal_delivery(&first, workflow_id).await;
        materialize_pending(&first, &pending, "wake complete", None, true, Timestamp(50)).await;

        let (left, right) = tokio::join!(
            first.resolve_materialized_pending_for_workflow(
                workflow_id,
                WakeResolveMaterializedDecision::Accept,
                Timestamp(51),
            ),
            second.resolve_materialized_pending_for_workflow(
                workflow_id,
                WakeResolveMaterializedDecision::Accept,
                Timestamp(51),
            )
        );
        let outcomes = [left.unwrap().unwrap(), right.unwrap().unwrap()];
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| matches!(
                    outcome,
                    WakeResolveMaterializedPendingOutcome::Resolved { .. }
                ))
                .count(),
            1
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| matches!(
                    outcome,
                    WakeResolveMaterializedPendingOutcome::AlreadyResolved
                ))
                .count(),
            1
        );
        assert_eq!(count_conversation_messages(&first, "conv-1").await, 1);
        assert!(first.list_pending("conv-1").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn cancelled_materialized_delivery_never_requests_auto_resume() {
        let (_dir, repo, _) = open_repo_pair().await;
        let workflow_id = WorkflowId(810);
        assert!(matches!(
            repo.register_allocated(workflow_id, &intent(), "fp-1", Timestamp(10))
                .await
                .unwrap(),
            WakeRegistrationOutcome::Registered { .. }
        ));
        let pending = match repo
            .cancel_allocated(&WakeCancelIfUnresolvedInput {
                workflow_id,
                expected_conversation_id: None,
                expected_contract_id: None,
                timestamp: Timestamp(20),
                reason: WakeCancellationReason::ExplicitCancel,
            })
            .await
            .unwrap()
        {
            WakeCancellationOutcome::Cancelled { delivery, .. }
            | WakeCancellationOutcome::Replayed { delivery, .. } => delivery,
            WakeCancellationOutcome::Stale => panic!("expected cancellation delivery"),
        };
        let link = materialized_outcome_link(
            materialize_pending(
                &repo,
                &pending,
                "wake cancelled",
                None,
                false,
                Timestamp(50),
            )
            .await,
        );
        assert!(!link.auto_resume);

        let outcome = repo
            .resolve_materialized_pending_for_workflow(
                workflow_id,
                WakeResolveMaterializedDecision::Suppress,
                Timestamp(51),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            outcome,
            WakeResolveMaterializedPendingOutcome::Resolved {
                delivery_ids: vec![pending.canonical_delivery.delivery_id],
                auto_resume: false,
            }
        );
        assert_snapshot_projection_parity(
            &repo,
            workflow_id,
            "conv-1",
            wake_profile::RuntimeAvailability::Suppressed,
            false,
            is_cancelled,
        )
        .await;
    }

    #[tokio::test]
    async fn restarted_repo_finalizes_materialized_pending_delivery() {
        let (_dir, repo, restarted) = open_repo_pair().await;
        let workflow_id = WorkflowId(811);
        let pending = create_pending_terminal_delivery(&repo, workflow_id).await;
        materialize_pending(&repo, &pending, "wake complete", None, true, Timestamp(50)).await;

        assert!(matches!(
            restarted
                .resolve_materialized_pending_for_workflow(
                    workflow_id,
                    WakeResolveMaterializedDecision::Accept,
                    Timestamp(51),
                )
                .await
                .unwrap()
                .unwrap(),
            WakeResolveMaterializedPendingOutcome::Resolved {
                auto_resume: true,
                ..
            }
        ));
        assert_eq!(count_conversation_messages(&restarted, "conv-1").await, 1);
        assert!(restarted.list_pending("conv-1").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn conversation_adoption_atomically_accepts_delivery_and_requests_one_turn() {
        let (_dir, repo, _) = open_repo_pair().await;
        let workflow_id = WorkflowId(812);
        let pending = create_pending_terminal_delivery(&repo, workflow_id).await;
        materialize_pending(&repo, &pending, "wake complete", None, true, Timestamp(50)).await;

        let outcome = repo
            .adopt_materialized_pending_for_conversation("conv-1", Timestamp(51))
            .await
            .unwrap();
        let WakeAdoptMaterializedPendingOutcome::Adopted(adopted) = outcome else {
            panic!("expected adopted wake batch");
        };
        assert!(adopted.auto_resume);
        assert_eq!(adopted.links.len(), 1);
        let state_json =
            sqlx::query_scalar::<_, String>("SELECT state FROM conversations WHERE id = 'conv-1'")
                .fetch_one(&repo.workflow_repo.pool)
                .await
                .unwrap();
        assert_eq!(
            serde_json::from_str::<ConvState>(&state_json).unwrap(),
            ConvState::LlmRequesting { attempt: 1 }
        );
        assert_snapshot_projection_parity(
            &repo,
            workflow_id,
            "conv-1",
            wake_profile::RuntimeAvailability::Accepted,
            false,
            is_fired,
        )
        .await;
    }

    #[tokio::test]
    async fn conversation_adoption_leaves_busy_delivery_owed() {
        let (_dir, repo, _) = open_repo_pair().await;
        let workflow_id = WorkflowId(813);
        let pending = create_pending_terminal_delivery(&repo, workflow_id).await;
        materialize_pending(&repo, &pending, "wake complete", None, true, Timestamp(50)).await;
        let busy = ConvState::LlmRequesting { attempt: 1 };
        sqlx::query("UPDATE conversations SET state = ?1 WHERE id = 'conv-1'")
            .bind(serde_json::to_string(&busy).unwrap())
            .execute(&repo.workflow_repo.pool)
            .await
            .unwrap();

        assert!(matches!(
            repo.adopt_materialized_pending_for_conversation("conv-1", Timestamp(51))
                .await
                .unwrap(),
            WakeAdoptMaterializedPendingOutcome::Busy(_)
        ));
        assert_eq!(repo.list_pending("conv-1").await.unwrap().len(), 1);
        let (head, snapshot) = head_snapshot(&repo, workflow_id).await;
        assert_eq!(head.version, Version(2));
        assert_eq!(
            snapshot.runtime_availability,
            wake_profile::RuntimeAvailability::Idle
        );
    }

    #[tokio::test]
    async fn conversation_adoption_refuses_archived_conversation_transactionally() {
        let (_dir, repo, _) = open_repo_pair().await;
        let workflow_id = WorkflowId(8131);
        let pending = create_pending_terminal_delivery(&repo, workflow_id).await;
        materialize_pending(&repo, &pending, "wake complete", None, true, Timestamp(50)).await;
        sqlx::query("UPDATE conversations SET archived = 1 WHERE id = 'conv-1'")
            .execute(&repo.workflow_repo.pool)
            .await
            .unwrap();

        assert!(matches!(
            repo.adopt_materialized_pending_for_conversation("conv-1", Timestamp(51))
                .await
                .unwrap(),
            WakeAdoptMaterializedPendingOutcome::NothingPending
        ));
        assert_eq!(repo.list_pending("conv-1").await.unwrap().len(), 1);
        let (head, snapshot) = head_snapshot(&repo, workflow_id).await;
        assert_eq!(head.version, Version(2));
        assert_eq!(
            snapshot.runtime_availability,
            wake_profile::RuntimeAvailability::Idle
        );
        let state_json =
            sqlx::query_scalar::<_, String>("SELECT state FROM conversations WHERE id = 'conv-1'")
                .fetch_one(&repo.workflow_repo.pool)
                .await
                .unwrap();
        assert_eq!(
            serde_json::from_str::<ConvState>(&state_json).unwrap(),
            ConvState::Idle
        );
    }

    #[tokio::test]
    async fn conversation_adoption_suppresses_cancellation_without_requesting_turn() {
        let (_dir, repo, _) = open_repo_pair().await;
        let workflow_id = WorkflowId(814);
        assert!(matches!(
            repo.register_allocated(workflow_id, &intent(), "fp-1", Timestamp(10))
                .await
                .unwrap(),
            WakeRegistrationOutcome::Registered { .. }
        ));
        let pending = match repo
            .cancel_allocated(&WakeCancelIfUnresolvedInput {
                workflow_id,
                expected_conversation_id: None,
                expected_contract_id: None,
                timestamp: Timestamp(20),
                reason: WakeCancellationReason::ExplicitCancel,
            })
            .await
            .unwrap()
        {
            WakeCancellationOutcome::Cancelled { delivery, .. }
            | WakeCancellationOutcome::Replayed { delivery, .. } => delivery,
            WakeCancellationOutcome::Stale => panic!("expected cancellation delivery"),
        };
        materialize_pending(
            &repo,
            &pending,
            "wake cancelled",
            None,
            false,
            Timestamp(50),
        )
        .await;

        let WakeAdoptMaterializedPendingOutcome::Adopted(adopted) = repo
            .adopt_materialized_pending_for_conversation("conv-1", Timestamp(51))
            .await
            .unwrap()
        else {
            panic!("expected adopted cancellation");
        };
        assert!(!adopted.auto_resume);
        let state_json =
            sqlx::query_scalar::<_, String>("SELECT state FROM conversations WHERE id = 'conv-1'")
                .fetch_one(&repo.workflow_repo.pool)
                .await
                .unwrap();
        assert_eq!(
            serde_json::from_str::<ConvState>(&state_json).unwrap(),
            ConvState::Idle
        );
        assert_snapshot_projection_parity(
            &repo,
            workflow_id,
            "conv-1",
            wake_profile::RuntimeAvailability::Suppressed,
            false,
            is_cancelled,
        )
        .await;
    }

    #[tokio::test]
    async fn concurrent_conversation_adoption_has_one_winner() {
        let (_dir, first, second) = open_repo_pair().await;
        let workflow_id = WorkflowId(815);
        let pending = create_pending_terminal_delivery(&first, workflow_id).await;
        materialize_pending(&first, &pending, "wake complete", None, true, Timestamp(50)).await;

        let (left, right) = tokio::join!(
            first.adopt_materialized_pending_for_conversation("conv-1", Timestamp(51)),
            second.adopt_materialized_pending_for_conversation("conv-1", Timestamp(51))
        );
        let outcomes = [left.unwrap(), right.unwrap()];
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| matches!(
                    outcome,
                    WakeAdoptMaterializedPendingOutcome::Adopted(_)
                ))
                .count(),
            1
        );
        assert_eq!(count_conversation_messages(&first, "conv-1").await, 1);
        assert!(first.list_pending("conv-1").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn identical_delivery_ids_across_workflows_get_distinct_terminal_receipts() {
        let (_dir, repo, _) = open_repo_pair().await;
        let first = create_pending_terminal_delivery(&repo, WorkflowId(8051)).await;
        let mut second_intent = intent();
        second_intent.contract_id = "contract-second-workflow".into();
        second_intent.registering_tool_use_id = "tool-second-workflow".into();
        second_intent.resource = WakeResourceIdentity::Bash(wake_types::BashResourceIdentity {
            work_scope: second_intent.registration_scope.clone(),
            handle_id: "b-second-workflow".into(),
        });
        assert!(matches!(
            repo.register_allocated(WorkflowId(8052), &second_intent, "fp-second", Timestamp(10))
                .await
                .unwrap(),
            WakeRegistrationOutcome::Registered { .. }
        ));
        let started = unwrap_started(
            repo.claim_observation_if_eligible(
                WorkflowId(8052),
                ProcessIncarnation(1),
                Timestamp(20),
                phoenix_workflow::LeaseExpiry(30),
            )
            .await
            .unwrap(),
        );
        let second = match repo
            .record_terminal_evidence(
                WorkflowId(8052),
                started.authority.as_ref().unwrap(),
                1,
                ReceiptId(1),
                DeliveryId(1),
                Timestamp(20),
                &WakeTerminalEvidence::Bash(BashTerminalEvidence {
                    identity: wake_types::BashResourceIdentity {
                        work_scope: second_intent.registration_scope.clone(),
                        handle_id: "b-second-workflow".into(),
                    },
                    status: wake_types::BashTerminalStatus::Exited,
                    occurred_at: Timestamp(19),
                    exit_code: Some(0),
                    duration_ms: Some(1),
                    signal_number: None,
                    kill_signal_sent: None,
                    final_tail: vec![],
                }),
            )
            .await
            .unwrap()
        {
            WakeTerminalEvidenceOutcome::Recorded { delivery, .. }
            | WakeTerminalEvidenceOutcome::Replayed { delivery, .. } => delivery,
            WakeTerminalEvidenceOutcome::StaleAttempt
            | WakeTerminalEvidenceOutcome::WrongResource
            | WakeTerminalEvidenceOutcome::EvidenceAfterObservation
            | WakeTerminalEvidenceOutcome::EvidenceAfterExpiry => {
                panic!("expected terminal receipt")
            }
        };

        assert_eq!(first.canonical_delivery.delivery_id, DeliveryId(1));
        assert_eq!(second.canonical_delivery.delivery_id, DeliveryId(1));

        let count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM wake_terminal_receipts WHERE delivery_id = 1",
        )
        .fetch_one(&repo.workflow_repo.pool)
        .await
        .unwrap();
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn distinct_workflow_delivery_pairs_get_distinct_stable_message_ids_and_links() {
        let (_dir, repo, _) = open_repo_pair().await;
        let first = create_pending_terminal_delivery(&repo, WorkflowId(805)).await;
        let mut second_intent = intent();
        second_intent.registering_tool_use_id = "tool-2".into();
        second_intent.resource = WakeResourceIdentity::Bash(wake_types::BashResourceIdentity {
            work_scope: wake_types::WorkScopeIdentity {
                kind: wake_types::WorkScopeKind::Conversation,
                stable_key: "conv-1".into(),
            },
            handle_id: "b-2".into(),
        });
        let second_workflow_id = WorkflowId(806);
        assert!(matches!(
            repo.register_allocated(second_workflow_id, &second_intent, "fp-2", Timestamp(10))
                .await
                .unwrap(),
            WakeRegistrationOutcome::Registered { .. }
        ));
        let second_started = unwrap_started(
            repo.claim_observation_if_eligible(
                second_workflow_id,
                ProcessIncarnation(1),
                Timestamp(20),
                phoenix_workflow::LeaseExpiry(30),
            )
            .await
            .unwrap(),
        );
        let second = match repo
            .record_terminal_evidence(
                second_workflow_id,
                second_started.authority.as_ref().expect("authority"),
                1,
                ReceiptId(2),
                DeliveryId(2),
                Timestamp(20),
                &WakeTerminalEvidence::Bash(BashTerminalEvidence {
                    identity: wake_types::BashResourceIdentity {
                        work_scope: wake_types::WorkScopeIdentity {
                            kind: wake_types::WorkScopeKind::Conversation,
                            stable_key: "conv-1".into(),
                        },
                        handle_id: "b-2".into(),
                    },
                    status: wake_types::BashTerminalStatus::Exited,
                    occurred_at: Timestamp(19),
                    exit_code: Some(0),
                    duration_ms: Some(12),
                    signal_number: None,
                    kill_signal_sent: None,
                    final_tail: vec!["done".into(), "ok".into()],
                }),
            )
            .await
            .unwrap()
        {
            WakeTerminalEvidenceOutcome::Recorded { delivery, .. }
            | WakeTerminalEvidenceOutcome::Replayed { delivery, .. } => delivery,
            other @ (WakeTerminalEvidenceOutcome::StaleAttempt
            | WakeTerminalEvidenceOutcome::WrongResource
            | WakeTerminalEvidenceOutcome::EvidenceAfterObservation
            | WakeTerminalEvidenceOutcome::EvidenceAfterExpiry) => {
                panic!("expected recorded/replayed second delivery, got {other:?}")
            }
        };

        let first_link = materialized_outcome_link(
            materialize_pending(&repo, &first, "first", None, false, Timestamp(49)).await,
        );
        let second_link = materialized_outcome_link(
            materialize_pending(&repo, &second, "second", None, false, Timestamp(50)).await,
        );

        assert_ne!(first_link.workflow_id, second_link.workflow_id);
        assert_ne!(first_link.message_id, second_link.message_id);
        assert_eq!(
            first_link.message_id,
            wake_delivery_message_id(WorkflowId(805), DeliveryId(1))
        );
        assert_eq!(
            second_link.message_id,
            wake_delivery_message_id(second_workflow_id, DeliveryId(2))
        );
        assert_eq!(count_delivery_message_links(&repo).await, 2);
    }

    #[tokio::test]
    async fn duplicate_concurrent_registration_replays_single_winner() {
        let (_dir, first, second) = open_repo_pair().await;
        let input = intent();
        let (left, right) = tokio::join!(
            first.register(&input, "fp-1", Timestamp(10)),
            second.register(&input, "fp-1", Timestamp(10))
        );
        let outcomes = [left.unwrap(), right.unwrap()];
        assert_eq!(
            outcomes
                .iter()
                .filter(|o| matches!(o, WakeRegistrationOutcome::Registered { .. }))
                .count(),
            1
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|o| matches!(o, WakeRegistrationOutcome::Replayed { .. }))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn mutable_input_conflict() {
        let (_dir, repo, _) = open_repo_pair().await;
        let input = intent();
        assert!(matches!(
            repo.register(&input, "fp-1", Timestamp(10)).await.unwrap(),
            WakeRegistrationOutcome::Registered { .. }
        ));
        assert_eq!(
            repo.register(&input, "fp-2", Timestamp(10)).await.unwrap(),
            WakeRegistrationOutcome::Conflict
        );
    }

    #[tokio::test]
    async fn failpoint_rolls_back_everything() {
        let (_dir, repo, _) = open_repo_pair().await;
        let input = intent();
        let workflow_id = next_test_workflow_id(&repo).await.unwrap();
        repo.fail_after_canonical_transition_once(workflow_id);
        let err = repo
            .register_allocated(workflow_id, &input, "fp-1", Timestamp(10))
            .await;
        assert!(err.is_err());
        assert!(repo.fetch_binding(workflow_id).await.unwrap().is_none());
        assert_eq!(
            repo.workflow_repo
                .fetch_workflow_head(workflow_id)
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn restart_reload_finds_binding() {
        let (_dir, first, second) = open_repo_pair().await;
        let input = intent();
        let registered = first.register(&input, "fp-1", Timestamp(10)).await.unwrap();
        assert!(matches!(
            registered,
            WakeRegistrationOutcome::Registered { .. }
        ));
        let workflow_id = match registered {
            WakeRegistrationOutcome::Registered { workflow_id, .. } => workflow_id,
            other @ (WakeRegistrationOutcome::Replayed { .. }
            | WakeRegistrationOutcome::Conflict) => {
                panic!("expected registered, got {other:?}")
            }
        };
        let binding = second.reload_binding(workflow_id).await.unwrap().unwrap();
        assert_eq!(binding.contract_id, "contract-1");
        assert_eq!(binding.prepared_fingerprint, "fp-1");
    }

    #[tokio::test]
    async fn terminal_receipt_failpoint_rolls_back_canonical_and_projection() {
        let (_dir, repo, _) = open_repo_pair().await;
        let canonical = unwrap_started(register_and_begin(&repo, WorkflowId(106)).await);
        repo.fail_after_canonical_receipt_once(canonical.authority.as_ref().unwrap().workflow_id);
        let err = repo
            .record_terminal_evidence(
                WorkflowId(106),
                canonical.authority.as_ref().unwrap(),
                1,
                ReceiptId(1),
                DeliveryId(1),
                Timestamp(20),
                &bash_evidence(19),
            )
            .await;
        assert!(err.is_err());
        assert!(repo.list_pending("conv-1").await.unwrap().is_empty());
        let head = repo
            .workflow_repo
            .fetch_workflow_head(WorkflowId(106))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(head.version, Version(1));
    }

    #[tokio::test]
    async fn cancel_failpoint_rolls_back_everything() {
        let (_dir, repo, _) = open_repo_pair().await;
        let input = intent();
        let workflow_id = next_test_workflow_id(&repo).await.unwrap();
        assert!(matches!(
            repo.register_allocated(workflow_id, &input, "fp-1", Timestamp(10))
                .await
                .unwrap(),
            WakeRegistrationOutcome::Registered { .. }
        ));
        repo.fail_after_canonical_transition_once(workflow_id);
        let err = repo.cancel(&cancel_input(workflow_id)).await;
        assert!(err.is_err());
        assert!(repo.list_pending("conv-1").await.unwrap().is_empty());
        let head = repo
            .workflow_repo
            .fetch_workflow_head(workflow_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(head.version, Version(1));
        assert_eq!(head.generation, Generation(0));
        assert_eq!(head.status, WorkflowStatus::Active);
    }

    #[tokio::test]
    async fn cancel_replays_after_cancelled_projection_exists() {
        let (_dir, repo, _) = open_repo_pair().await;
        let input = intent();
        let workflow_id = registered_workflow_id(&repo, &input, "fp-1").await;
        let first = repo.cancel(&cancel_input(workflow_id)).await.unwrap();
        let second = repo.cancel(&cancel_input(workflow_id)).await.unwrap();
        assert!(matches!(first, WakeCancellationOutcome::Cancelled { .. }));
        match second {
            WakeCancellationOutcome::Replayed { receipt, delivery } => {
                assert_eq!(receipt.receipt_id, ReceiptId(1));
                assert_eq!(delivery.canonical_delivery.delivery_id, DeliveryId(1));
                assert!(!delivery.canonical_delivery.requires_runtime_acceptance);
                assert!(matches!(
                    delivery.receipt.terminal,
                    WakeTerminalPayload::Cancelled {
                        reason: WakeCancellationReason::ExplicitCancel,
                        ..
                    }
                ));
            }
            other
            @ (WakeCancellationOutcome::Cancelled { .. } | WakeCancellationOutcome::Stale) => {
                panic!("expected replayed, got {other:?}")
            }
        }
    }

    #[tokio::test]
    async fn cancel_vs_terminal_race_repeated_has_single_winner() {
        for run in 0..10 {
            let (_dir, first, second) = open_repo_pair().await;
            let workflow_id = WorkflowId(300 + run);
            let canonical = unwrap_started(register_and_begin(&first, workflow_id).await);
            let authority = canonical.authority.unwrap();
            let evidence = bash_evidence(19);
            let cancel = cancel_input(workflow_id);
            let (left, right) = tokio::join!(
                first.record_terminal_evidence(
                    workflow_id,
                    &authority,
                    1,
                    ReceiptId(1),
                    DeliveryId(1),
                    Timestamp(20),
                    &evidence
                ),
                second.cancel(&cancel)
            );
            let pending = first.list_pending("conv-1").await.unwrap();
            assert_eq!(pending.len(), 1);
            let terminal = &pending[0].receipt.terminal;
            match (left.unwrap(), right.unwrap(), terminal) {
                (
                    WakeTerminalEvidenceOutcome::Recorded { .. }
                    | WakeTerminalEvidenceOutcome::Replayed { .. },
                    WakeCancellationOutcome::Stale | WakeCancellationOutcome::Replayed { .. },
                    WakeTerminalPayload::Fired { .. },
                )
                | (
                    WakeTerminalEvidenceOutcome::StaleAttempt
                    | WakeTerminalEvidenceOutcome::Replayed { .. },
                    WakeCancellationOutcome::Cancelled { .. }
                    | WakeCancellationOutcome::Replayed { .. },
                    WakeTerminalPayload::Cancelled { .. },
                ) => {}
                other => panic!("unexpected race outcome: {other:?}"),
            }
            let deliveries = first
                .workflow_repo
                .list_deliveries(workflow_id)
                .await
                .unwrap();
            assert_eq!(deliveries.len(), 1);
            assert_eq!(
                deliveries
                    .iter()
                    .filter(|d| d.status == phoenix_workflow::DeliveryStatus::Pending)
                    .count(),
                1
            );
        }
    }

    #[tokio::test]
    async fn expiry_vs_terminal_race_repeated_has_single_winner_with_typed_losers() {
        for run in 0..10 {
            let (_dir, first, second) = open_repo_pair().await;
            let workflow_id = WorkflowId(800 + run);
            let canonical = unwrap_started(register_and_begin(&first, workflow_id).await);
            let authority = canonical.authority.unwrap();
            let evidence = bash_evidence(19);
            let (left, right) = tokio::join!(
                first.record_terminal_evidence(
                    workflow_id,
                    &authority,
                    1,
                    ReceiptId(1),
                    DeliveryId(1),
                    Timestamp(20),
                    &evidence
                ),
                second.expire_if_unresolved(workflow_id, Timestamp(100))
            );
            let pending = first.list_pending("conv-1").await.unwrap();
            assert_eq!(pending.len(), 1);
            let terminal = &pending[0].receipt.terminal;
            match (left.unwrap(), right.unwrap(), terminal) {
                (
                    WakeTerminalEvidenceOutcome::Recorded { .. }
                    | WakeTerminalEvidenceOutcome::Replayed { .. },
                    WakeExpireIfUnresolvedOutcome::Stale
                    | WakeExpireIfUnresolvedOutcome::Replayed { .. },
                    WakeTerminalPayload::Fired { .. },
                )
                | (
                    WakeTerminalEvidenceOutcome::StaleAttempt
                    | WakeTerminalEvidenceOutcome::Replayed { .. },
                    WakeExpireIfUnresolvedOutcome::Expired { .. }
                    | WakeExpireIfUnresolvedOutcome::Replayed { .. },
                    WakeTerminalPayload::Expired { .. },
                ) => {}
                other => panic!("unexpected expiry race outcome: {other:?}"),
            }
            let deliveries = first
                .workflow_repo
                .list_deliveries(workflow_id)
                .await
                .unwrap();
            assert_eq!(deliveries.len(), 1);
            assert_eq!(
                deliveries
                    .iter()
                    .filter(|d| d.status == phoenix_workflow::DeliveryStatus::Pending)
                    .count(),
                1
            );
        }
    }

    #[tokio::test]
    async fn expiry_vs_cancel_race_repeated_has_single_winner_with_typed_losers() {
        for run in 0..10 {
            let (_dir, first, second) = open_repo_pair().await;
            let workflow_id = WorkflowId(900 + run);
            register_and_begin(&first, workflow_id).await;
            let cancel = cancel_input(workflow_id);
            let (left, right) = tokio::join!(
                first.expire_if_unresolved(workflow_id, Timestamp(100)),
                second.cancel(&cancel)
            );
            let pending = first.list_pending("conv-1").await.unwrap();
            assert_eq!(pending.len(), 1);
            let terminal = &pending[0].receipt.terminal;
            match (left.unwrap(), right.unwrap(), terminal) {
                (
                    WakeExpireIfUnresolvedOutcome::Expired { .. }
                    | WakeExpireIfUnresolvedOutcome::Replayed { .. },
                    WakeCancellationOutcome::Stale | WakeCancellationOutcome::Replayed { .. },
                    WakeTerminalPayload::Expired { .. },
                )
                | (
                    WakeExpireIfUnresolvedOutcome::Stale
                    | WakeExpireIfUnresolvedOutcome::Replayed { .. },
                    WakeCancellationOutcome::Cancelled { .. }
                    | WakeCancellationOutcome::Replayed { .. },
                    WakeTerminalPayload::Cancelled { .. },
                ) => {}
                other => panic!("unexpected cancel/expiry race outcome: {other:?}"),
            }
            let deliveries = first
                .workflow_repo
                .list_deliveries(workflow_id)
                .await
                .unwrap();
            assert_eq!(deliveries.len(), 1);
            assert_eq!(
                deliveries
                    .iter()
                    .filter(|d| d.status == phoenix_workflow::DeliveryStatus::Pending)
                    .count(),
                1
            );
        }
    }

    #[tokio::test]
    async fn restart_reload_lists_pending_cancellation_projection() {
        let (_dir, first, second) = open_repo_pair().await;
        let input = intent();
        let workflow_id = registered_workflow_id(&first, &input, "fp-1").await;
        first.cancel(&cancel_input(workflow_id)).await.unwrap();
        let pending = second.list_pending("conv-1").await.unwrap();
        assert_eq!(pending.len(), 1);
        assert!(matches!(
            pending[0].receipt.terminal,
            WakeTerminalPayload::Cancelled {
                reason: WakeCancellationReason::ExplicitCancel,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn cancellation_has_no_runtime_acceptance() {
        let (_dir, repo, _) = open_repo_pair().await;
        let input = intent();
        let workflow_id = registered_workflow_id(&repo, &input, "fp-1").await;
        let outcome = repo.cancel(&cancel_input(workflow_id)).await.unwrap();
        let delivery = match outcome {
            WakeCancellationOutcome::Cancelled { delivery, .. }
            | WakeCancellationOutcome::Replayed { delivery, .. } => delivery,
            WakeCancellationOutcome::Stale => panic!("expected cancelled delivery"),
        };
        assert!(!delivery.canonical_delivery.requires_runtime_acceptance);
        assert_eq!(delivery.canonical_delivery.runtime_acceptance_status, None);
    }

    #[tokio::test]
    async fn cancellation_snapshot_projection_parity_before_and_after_accept_restart() {
        let (_dir, repo, restarted) = open_repo_pair().await;
        let workflow_id = registered_workflow_id(&repo, &intent(), "fp-1").await;
        let outcome = repo.cancel(&cancel_input(workflow_id)).await.unwrap();
        let receipt_id = match outcome {
            WakeCancellationOutcome::Cancelled { receipt, delivery }
            | WakeCancellationOutcome::Replayed { receipt, delivery } => {
                assert!(!delivery.canonical_delivery.requires_runtime_acceptance);
                assert_eq!(delivery.canonical_delivery.runtime_acceptance_status, None);
                receipt.receipt_id
            }
            WakeCancellationOutcome::Stale => panic!("expected cancelled delivery"),
        };
        assert_eq!(
            fetch_receipt_origin(&repo, workflow_id, receipt_id).await,
            ReceiptOrigin::CancellationArbitration
        );
        assert_snapshot_projection_parity(
            &repo,
            workflow_id,
            "conv-1",
            wake_profile::RuntimeAvailability::Idle,
            true,
            is_cancelled,
        )
        .await;
        assert_snapshot_projection_parity(
            &restarted,
            workflow_id,
            "conv-1",
            wake_profile::RuntimeAvailability::Idle,
            true,
            is_cancelled,
        )
        .await;

        assert_eq!(
            restarted
                .resolve_pending_exact(&resolve_input(
                    workflow_id,
                    Version(2),
                    TransitionId(3),
                    vec![DeliveryId(1)],
                ))
                .await
                .unwrap(),
            WakeResolvePendingOutcome::Resolved
        );
        assert_snapshot_projection_parity(
            &restarted,
            workflow_id,
            "conv-1",
            wake_profile::RuntimeAvailability::Accepted,
            false,
            is_cancelled,
        )
        .await;
    }

    #[tokio::test]
    async fn duplicate_concurrent_receipt_has_single_winner_and_replay() {
        let (_dir, first, second) = open_repo_pair().await;
        let canonical = unwrap_started(register_and_begin(&first, WorkflowId(107)).await);
        let authority = canonical.authority.unwrap();
        let evidence = bash_evidence(19);
        let (left, right) = tokio::join!(
            first.record_terminal_evidence(
                WorkflowId(107),
                &authority,
                1,
                ReceiptId(1),
                DeliveryId(1),
                Timestamp(20),
                &evidence
            ),
            second.record_terminal_evidence(
                WorkflowId(107),
                &authority,
                2,
                ReceiptId(1),
                DeliveryId(1),
                Timestamp(20),
                &evidence
            )
        );
        let outcomes = [left, right];
        assert_eq!(
            outcomes
                .iter()
                .filter(|o| matches!(o, Ok(WakeTerminalEvidenceOutcome::Recorded { .. })))
                .count(),
            1
        );
        assert!(outcomes.iter().any(|outcome| matches!(
            outcome,
            Ok(WakeTerminalEvidenceOutcome::Replayed { .. }
                | WakeTerminalEvidenceOutcome::StaleAttempt)
        )));
        let pending = first.list_pending("conv-1").await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].receipt.receipt_id, ReceiptId(1));
        let head = first
            .workflow_repo
            .fetch_workflow_head(WorkflowId(107))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(head.version, Version(2));
        let transition_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM workflow_transitions WHERE workflow_id = ?1",
        )
        .bind(107_i64)
        .fetch_one(&first.workflow_repo.pool)
        .await
        .unwrap();
        assert_eq!(transition_count, 2);
    }

    #[tokio::test]
    async fn wrong_resource_is_rejected() {
        let (_dir, repo, _) = open_repo_pair().await;
        let canonical = unwrap_started(register_and_begin(&repo, WorkflowId(108)).await);
        let outcome = repo
            .record_terminal_evidence(
                WorkflowId(108),
                canonical.authority.as_ref().unwrap(),
                1,
                ReceiptId(1),
                DeliveryId(1),
                Timestamp(20),
                &tmux_evidence(19),
            )
            .await
            .unwrap();
        assert_eq!(outcome, WakeTerminalEvidenceOutcome::WrongResource);
    }

    #[tokio::test]
    async fn evidence_before_deadline_wins() {
        let (_dir, repo, _) = open_repo_pair().await;
        let canonical = unwrap_started(register_and_begin(&repo, WorkflowId(109)).await);
        let outcome = repo
            .record_terminal_evidence(
                WorkflowId(109),
                canonical.authority.as_ref().unwrap(),
                1,
                ReceiptId(1),
                DeliveryId(1),
                Timestamp(20),
                &bash_evidence(20),
            )
            .await
            .unwrap();
        match outcome {
            WakeTerminalEvidenceOutcome::Recorded { delivery, .. } => {
                assert_eq!(delivery.receipt.delivery_id, DeliveryId(1));
            }
            other @ (WakeTerminalEvidenceOutcome::Replayed { .. }
            | WakeTerminalEvidenceOutcome::StaleAttempt
            | WakeTerminalEvidenceOutcome::WrongResource
            | WakeTerminalEvidenceOutcome::EvidenceAfterObservation
            | WakeTerminalEvidenceOutcome::EvidenceAfterExpiry) => {
                panic!("expected recorded, got {other:?}")
            }
        }
    }

    #[tokio::test]
    async fn empty_transfer_before_terminal_then_terminal_targets_new_owner() {
        let (_dir, repo, restarted) = open_repo_pair().await;
        insert_conversation(&repo.workflow_repo.pool, "conv-2").await;
        let workflow_id = WorkflowId(118);
        let started = unwrap_started(register_and_begin(&repo, workflow_id).await);
        let identity_before = external_acceptance_identity(&repo, workflow_id).await;

        assert_eq!(
            repo.transfer(&transfer_input(
                workflow_id,
                "conv-1",
                "conv-2",
                Version(1),
                vec![],
                TransitionId(2),
            ))
            .await
            .unwrap(),
            WakeTransferOutcome::Transferred
        );
        assert!(repo.list_pending("conv-1").await.unwrap().is_empty());
        assert!(repo.list_pending("conv-2").await.unwrap().is_empty());
        assert_eq!(
            restarted
                .fetch_binding(workflow_id)
                .await
                .unwrap()
                .unwrap()
                .conversation_id,
            "conv-2"
        );
        assert_eq!(
            external_acceptance_identity(&repo, workflow_id).await,
            identity_before
        );

        repo.record_terminal_evidence(
            workflow_id,
            started.authority.as_ref().unwrap(),
            1,
            ReceiptId(1),
            DeliveryId(1),
            Timestamp(20),
            &bash_evidence(19),
        )
        .await
        .unwrap();
        assert!(repo.list_pending("conv-1").await.unwrap().is_empty());
        let pending = repo.list_pending("conv-2").await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].conversation_id, "conv-2");
    }

    #[tokio::test]
    async fn pending_transfer_moves_list_pending_old_to_new() {
        let (_dir, repo, _) = open_repo_pair().await;
        insert_conversation(&repo.workflow_repo.pool, "conv-2").await;
        let workflow_id = WorkflowId(119);
        let canonical = unwrap_started(register_and_begin(&repo, workflow_id).await);
        repo.record_terminal_evidence(
            workflow_id,
            canonical.authority.as_ref().unwrap(),
            1,
            ReceiptId(1),
            DeliveryId(1),
            Timestamp(20),
            &bash_evidence(19),
        )
        .await
        .unwrap();
        assert_eq!(repo.list_pending("conv-1").await.unwrap().len(), 1);

        assert_eq!(
            repo.transfer(&transfer_input(
                workflow_id,
                "conv-1",
                "conv-2",
                Version(2),
                vec![DeliveryId(1)],
                TransitionId(3),
            ))
            .await
            .unwrap(),
            WakeTransferOutcome::Transferred
        );
        assert!(repo.list_pending("conv-1").await.unwrap().is_empty());
        let pending = repo.list_pending("conv-2").await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].conversation_id, "conv-2");
        assert_eq!(pending[0].receipt.conversation_id, "conv-2");
    }

    #[tokio::test]
    async fn startup_reconciliation_repairs_continuation_transfer_after_restart() {
        let (_dir, repo, restarted) = open_repo_pair().await;
        insert_conversation(&repo.workflow_repo.pool, "conv-2").await;
        insert_conversation(&repo.workflow_repo.pool, "conv-3").await;
        let workflow_id = WorkflowId(1191);
        create_pending_terminal_delivery(&repo, workflow_id).await;
        sqlx::query("UPDATE conversations SET continued_in_conv_id = 'conv-2' WHERE id = 'conv-1'")
            .execute(&repo.workflow_repo.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE conversations SET continued_in_conv_id = 'conv-3' WHERE id = 'conv-2'")
            .execute(&repo.workflow_repo.pool)
            .await
            .unwrap();
        assert_eq!(restarted.list_pending("conv-1").await.unwrap().len(), 1);

        assert_eq!(
            restarted
                .reconcile_continuation_transfers(Timestamp(30))
                .await
                .unwrap(),
            1
        );

        assert!(restarted.list_pending("conv-1").await.unwrap().is_empty());
        assert!(restarted.list_pending("conv-2").await.unwrap().is_empty());
        let pending = restarted.list_pending("conv-3").await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].conversation_id, "conv-3");
        assert_eq!(
            restarted
                .fetch_binding(workflow_id)
                .await
                .unwrap()
                .unwrap()
                .conversation_id,
            "conv-3"
        );
    }

    #[tokio::test]
    async fn resolved_transfer_leaves_historical_projection_old_but_binding_new() {
        let (_dir, repo, restarted) = open_repo_pair().await;
        insert_conversation(&repo.workflow_repo.pool, "conv-2").await;
        let workflow_id = WorkflowId(120);
        let canonical = unwrap_started(register_and_begin(&repo, workflow_id).await);
        repo.record_terminal_evidence(
            workflow_id,
            canonical.authority.as_ref().unwrap(),
            1,
            ReceiptId(1),
            DeliveryId(1),
            Timestamp(20),
            &bash_evidence(19),
        )
        .await
        .unwrap();
        assert_eq!(
            repo.resolve_pending_exact(&resolve_input(
                workflow_id,
                Version(2),
                TransitionId(3),
                vec![DeliveryId(1)],
            ))
            .await
            .unwrap(),
            WakeResolvePendingOutcome::Resolved
        );

        assert_eq!(
            repo.transfer(&transfer_input(
                workflow_id,
                "conv-1",
                "conv-2",
                Version(3),
                vec![],
                TransitionId(4),
            ))
            .await
            .unwrap(),
            WakeTransferOutcome::Transferred
        );
        assert!(repo.list_pending("conv-1").await.unwrap().is_empty());
        assert!(repo.list_pending("conv-2").await.unwrap().is_empty());
        assert_eq!(
            restarted
                .fetch_binding(workflow_id)
                .await
                .unwrap()
                .unwrap()
                .conversation_id,
            "conv-2"
        );
        let projection = fetch_any_terminal_projection_tx(
            &mut restarted.workflow_repo.begin_tx().await.unwrap(),
            workflow_id,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(projection.conversation_id, "conv-1");
    }

    #[tokio::test]
    async fn transfer_owner_set_and_version_mismatch_do_not_mutate() {
        let (_dir, repo, restarted) = open_repo_pair().await;
        insert_conversation(&repo.workflow_repo.pool, "conv-2").await;
        let workflow_id = WorkflowId(121);
        let canonical = unwrap_started(register_and_begin(&repo, workflow_id).await);
        repo.record_terminal_evidence(
            workflow_id,
            canonical.authority.as_ref().unwrap(),
            1,
            ReceiptId(1),
            DeliveryId(1),
            Timestamp(20),
            &bash_evidence(19),
        )
        .await
        .unwrap();
        let before = repo
            .workflow_repo
            .fetch_workflow_head(workflow_id)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(
            repo.transfer(&transfer_input(
                workflow_id,
                "conv-x",
                "conv-2",
                Version(2),
                vec![DeliveryId(1)],
                TransitionId(3),
            ))
            .await
            .unwrap(),
            WakeTransferOutcome::OwnerMismatch
        );
        assert_eq!(
            repo.transfer(&transfer_input(
                workflow_id,
                "conv-1",
                "conv-2",
                Version(3),
                vec![DeliveryId(1)],
                TransitionId(4),
            ))
            .await
            .unwrap(),
            WakeTransferOutcome::VersionConflict
        );
        assert_eq!(
            repo.transfer(&transfer_input(
                workflow_id,
                "conv-1",
                "conv-2",
                Version(2),
                vec![],
                TransitionId(3),
            ))
            .await
            .unwrap(),
            WakeTransferOutcome::SetMismatch
        );

        let after = restarted
            .workflow_repo
            .fetch_workflow_head(workflow_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(before.version, after.version);
        assert_eq!(
            restarted
                .fetch_binding(workflow_id)
                .await
                .unwrap()
                .unwrap()
                .conversation_id,
            "conv-1"
        );
        assert_eq!(restarted.list_pending("conv-1").await.unwrap().len(), 1);
        assert!(restarted.list_pending("conv-2").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn transfer_failpoint_rolls_back_binding_and_projection() {
        let (_dir, repo, restarted) = open_repo_pair().await;
        insert_conversation(&repo.workflow_repo.pool, "conv-2").await;
        let workflow_id = WorkflowId(122);
        let canonical = unwrap_started(register_and_begin(&repo, workflow_id).await);
        repo.record_terminal_evidence(
            workflow_id,
            canonical.authority.as_ref().unwrap(),
            1,
            ReceiptId(1),
            DeliveryId(1),
            Timestamp(20),
            &bash_evidence(19),
        )
        .await
        .unwrap();
        restarted.fail_after_transfer_binding_update_once(workflow_id);

        let err = restarted
            .transfer(&transfer_input(
                workflow_id,
                "conv-1",
                "conv-2",
                Version(2),
                vec![DeliveryId(1)],
                TransitionId(3),
            ))
            .await;
        assert!(err.is_err());
        assert_eq!(
            restarted
                .fetch_binding(workflow_id)
                .await
                .unwrap()
                .unwrap()
                .conversation_id,
            "conv-1"
        );
        assert_eq!(
            restarted
                .workflow_repo
                .fetch_workflow_head(workflow_id)
                .await
                .unwrap()
                .unwrap()
                .version,
            Version(2)
        );
        assert_eq!(restarted.list_pending("conv-1").await.unwrap().len(), 1);
        assert!(restarted.list_pending("conv-2").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn transfer_restart_reload_sees_new_owner() {
        let (_dir, repo, restarted) = open_repo_pair().await;
        insert_conversation(&repo.workflow_repo.pool, "conv-2").await;
        let workflow_id = next_test_workflow_id(&repo).await.unwrap();
        assert!(matches!(
            repo.register_allocated(workflow_id, &intent(), "fp-1", Timestamp(10))
                .await
                .unwrap(),
            WakeRegistrationOutcome::Registered { .. }
        ));
        assert_eq!(
            repo.transfer(&transfer_input(
                workflow_id,
                "conv-1",
                "conv-2",
                Version(1),
                vec![],
                TransitionId(2),
            ))
            .await
            .unwrap(),
            WakeTransferOutcome::Transferred
        );
        assert_eq!(
            restarted
                .reload_binding(workflow_id)
                .await
                .unwrap()
                .unwrap()
                .conversation_id,
            "conv-2"
        );
    }

    #[tokio::test]
    async fn transfer_vs_terminal_race_repeated_has_one_coherent_owner() {
        for run in 0..10 {
            let (_dir, first, second) = open_repo_pair().await;
            insert_conversation(&first.workflow_repo.pool, "conv-2").await;
            let workflow_id = WorkflowId(400 + run);
            let canonical = unwrap_started(register_and_begin(&first, workflow_id).await);
            let authority = canonical.authority.unwrap();
            let transfer = transfer_input(
                workflow_id,
                "conv-1",
                "conv-2",
                Version(1),
                vec![],
                TransitionId(2),
            );
            let evidence = bash_evidence(19);
            let (left, right) = tokio::join!(
                first.transfer(&transfer),
                second.record_terminal_evidence(
                    workflow_id,
                    &authority,
                    1,
                    ReceiptId(1),
                    DeliveryId(1),
                    Timestamp(20),
                    &evidence
                )
            );
            match left.unwrap() {
                WakeTransferOutcome::Transferred
                | WakeTransferOutcome::VersionConflict
                | WakeTransferOutcome::SetMismatch => {}
                other @ WakeTransferOutcome::OwnerMismatch => {
                    panic!("unexpected transfer race outcome: {other:?}")
                }
            }
            match right.unwrap() {
                WakeTerminalEvidenceOutcome::Recorded { .. }
                | WakeTerminalEvidenceOutcome::Replayed { .. } => {
                    let binding_owner = first
                        .fetch_binding(workflow_id)
                        .await
                        .unwrap()
                        .unwrap()
                        .conversation_id;
                    let old_pending = first.list_pending("conv-1").await.unwrap();
                    let new_pending = first.list_pending("conv-2").await.unwrap();
                    assert!(old_pending.len() + new_pending.len() <= 1);
                    if let Some(item) = old_pending.first() {
                        assert_eq!(binding_owner, item.conversation_id);
                    }
                    if let Some(item) = new_pending.first() {
                        assert_eq!(binding_owner, item.conversation_id);
                    }
                }
                WakeTerminalEvidenceOutcome::StaleAttempt => {
                    let binding_owner = first
                        .fetch_binding(workflow_id)
                        .await
                        .unwrap()
                        .unwrap()
                        .conversation_id;
                    assert_eq!(binding_owner, "conv-2");
                    assert!(first.list_pending("conv-1").await.unwrap().is_empty());
                    assert!(first.list_pending("conv-2").await.unwrap().is_empty());
                }
                other @ (WakeTerminalEvidenceOutcome::WrongResource
                | WakeTerminalEvidenceOutcome::EvidenceAfterObservation
                | WakeTerminalEvidenceOutcome::EvidenceAfterExpiry) => {
                    panic!("unexpected terminal race outcome: {other:?}")
                }
            }
        }
    }

    #[tokio::test]
    async fn transfer_vs_cancel_race_repeated_has_one_coherent_owner() {
        for run in 0..10 {
            let (_dir, first, second) = open_repo_pair().await;
            insert_conversation(&first.workflow_repo.pool, "conv-2").await;
            let workflow_id = WorkflowId(500 + run);
            register_and_begin(&first, workflow_id).await;
            let transfer = transfer_input(
                workflow_id,
                "conv-1",
                "conv-2",
                Version(1),
                vec![],
                TransitionId(2),
            );
            let cancel = cancel_input(workflow_id);
            let (left, right) = tokio::join!(first.transfer(&transfer), second.cancel(&cancel));
            match left.unwrap() {
                WakeTransferOutcome::Transferred
                | WakeTransferOutcome::VersionConflict
                | WakeTransferOutcome::SetMismatch => {}
                other @ WakeTransferOutcome::OwnerMismatch => {
                    panic!("unexpected transfer race outcome: {other:?}")
                }
            }
            match right.unwrap() {
                WakeCancellationOutcome::Cancelled { .. }
                | WakeCancellationOutcome::Replayed { .. } => {
                    let binding_owner = first
                        .fetch_binding(workflow_id)
                        .await
                        .unwrap()
                        .unwrap()
                        .conversation_id;
                    let old_pending = first.list_pending("conv-1").await.unwrap();
                    let new_pending = first.list_pending("conv-2").await.unwrap();
                    assert!(old_pending.len() + new_pending.len() <= 1);
                    if let Some(item) = old_pending.first() {
                        assert_eq!(binding_owner, item.conversation_id);
                    }
                    if let Some(item) = new_pending.first() {
                        assert_eq!(binding_owner, item.conversation_id);
                    }
                }
                WakeCancellationOutcome::Stale => {
                    let binding_owner = first
                        .fetch_binding(workflow_id)
                        .await
                        .unwrap()
                        .unwrap()
                        .conversation_id;
                    assert_eq!(binding_owner, "conv-2");
                    assert!(first.list_pending("conv-1").await.unwrap().is_empty());
                    assert!(first.list_pending("conv-2").await.unwrap().is_empty());
                }
            }
        }
    }

    #[tokio::test]
    async fn resolve_pending_accept_hides_from_list_and_preserves_no_runtime_acceptance() {
        let (_dir, repo, _) = open_repo_pair().await;
        let canonical = unwrap_started(register_and_begin(&repo, WorkflowId(111)).await);
        repo.record_terminal_evidence(
            WorkflowId(111),
            canonical.authority.as_ref().unwrap(),
            1,
            ReceiptId(1),
            DeliveryId(1),
            Timestamp(20),
            &bash_evidence(19),
        )
        .await
        .unwrap();

        assert_eq!(repo.list_pending("conv-1").await.unwrap().len(), 1);
        assert_eq!(
            repo.resolve_pending_exact(&resolve_input(
                WorkflowId(111),
                Version(2),
                TransitionId(3),
                vec![DeliveryId(1)]
            ))
            .await
            .unwrap(),
            WakeResolvePendingOutcome::Resolved
        );
        assert!(repo.list_pending("conv-1").await.unwrap().is_empty());
        let deliveries = repo
            .workflow_repo
            .list_deliveries(WorkflowId(111))
            .await
            .unwrap();
        assert_eq!(deliveries.len(), 1);
        assert_eq!(
            deliveries[0].status,
            phoenix_workflow::DeliveryStatus::Accepted
        );
        assert_eq!(deliveries[0].runtime_acceptance_status, None);
    }

    #[tokio::test]
    async fn resolve_pending_suppress_sets_structural_reason() {
        let (_dir, repo, _) = open_repo_pair().await;
        let canonical = unwrap_started(register_and_begin(&repo, WorkflowId(112)).await);
        repo.record_terminal_evidence(
            WorkflowId(112),
            canonical.authority.as_ref().unwrap(),
            1,
            ReceiptId(1),
            DeliveryId(1),
            Timestamp(20),
            &bash_evidence(19),
        )
        .await
        .unwrap();
        let mut input = resolve_input(
            WorkflowId(112),
            Version(2),
            TransitionId(3),
            vec![DeliveryId(1)],
        );
        input.decision = WakeResolveDecision::Suppress;
        assert_eq!(
            repo.resolve_pending_exact(&input).await.unwrap(),
            WakeResolvePendingOutcome::Resolved
        );
        let deliveries = repo
            .workflow_repo
            .list_deliveries(WorkflowId(112))
            .await
            .unwrap();
        assert_eq!(
            deliveries[0].status,
            phoenix_workflow::DeliveryStatus::Suppressed
        );
        assert_eq!(
            deliveries[0].suppression_reason,
            Some(phoenix_workflow::SuppressionReason::ReducerTerminal)
        );
        assert_eq!(deliveries[0].runtime_acceptance_status, None);
    }

    #[tokio::test]
    async fn resolve_pending_failpoint_rolls_back_transition_and_deliveries() {
        let (_dir, repo, restarted) = open_repo_pair().await;
        let canonical = unwrap_started(register_and_begin(&repo, WorkflowId(113)).await);
        repo.record_terminal_evidence(
            WorkflowId(113),
            canonical.authority.as_ref().unwrap(),
            1,
            ReceiptId(1),
            DeliveryId(1),
            Timestamp(20),
            &bash_evidence(19),
        )
        .await
        .unwrap();
        restarted.fail_after_resolve_transition_once(WorkflowId(113));
        let input = resolve_input(
            WorkflowId(113),
            Version(2),
            TransitionId(3),
            vec![DeliveryId(1)],
        );
        let err = restarted.resolve_pending_exact(&input).await;
        assert!(err.is_err());
        let head = restarted
            .workflow_repo
            .fetch_workflow_head(WorkflowId(113))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(head.version, Version(2));
        let deliveries = restarted
            .workflow_repo
            .list_deliveries(WorkflowId(113))
            .await
            .unwrap();
        assert_eq!(
            deliveries[0].status,
            phoenix_workflow::DeliveryStatus::Pending
        );
        assert_eq!(restarted.list_pending("conv-1").await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn resolve_pending_restart_reload_sees_resolved_state() {
        let (_dir, repo, restarted) = open_repo_pair().await;
        let canonical = unwrap_started(register_and_begin(&repo, WorkflowId(114)).await);
        repo.record_terminal_evidence(
            WorkflowId(114),
            canonical.authority.as_ref().unwrap(),
            1,
            ReceiptId(1),
            DeliveryId(1),
            Timestamp(20),
            &bash_evidence(19),
        )
        .await
        .unwrap();
        assert_eq!(
            repo.resolve_pending_exact(&resolve_input(
                WorkflowId(114),
                Version(2),
                TransitionId(3),
                vec![DeliveryId(1)]
            ))
            .await
            .unwrap(),
            WakeResolvePendingOutcome::Resolved
        );
        assert!(restarted.list_pending("conv-1").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn concurrent_resolve_has_single_winner_and_version_conflict() {
        let (_dir, first, second) = open_repo_pair().await;
        let canonical = unwrap_started(register_and_begin(&first, WorkflowId(115)).await);
        first
            .record_terminal_evidence(
                WorkflowId(115),
                canonical.authority.as_ref().unwrap(),
                1,
                ReceiptId(1),
                DeliveryId(1),
                Timestamp(20),
                &bash_evidence(19),
            )
            .await
            .unwrap();
        let input = resolve_input(
            WorkflowId(115),
            Version(2),
            TransitionId(3),
            vec![DeliveryId(1)],
        );
        let (left, right) = tokio::join!(
            first.resolve_pending_exact(&input),
            second.resolve_pending_exact(&input)
        );
        let outcomes = [left.unwrap(), right.unwrap()];
        assert_eq!(
            outcomes
                .iter()
                .filter(|o| matches!(o, WakeResolvePendingOutcome::Resolved))
                .count(),
            1
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|o| matches!(o, WakeResolvePendingOutcome::VersionConflict))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn resolve_pending_rejects_missing_extra_and_duplicate_sets() {
        let (_dir, repo, _) = open_repo_pair().await;
        let canonical = unwrap_started(register_and_begin(&repo, WorkflowId(116)).await);
        repo.record_terminal_evidence(
            WorkflowId(116),
            canonical.authority.as_ref().unwrap(),
            1,
            ReceiptId(1),
            DeliveryId(1),
            Timestamp(20),
            &bash_evidence(19),
        )
        .await
        .unwrap();

        assert_eq!(
            repo.resolve_pending_exact(&resolve_input(
                WorkflowId(116),
                Version(2),
                TransitionId(3),
                vec![]
            ))
            .await
            .unwrap(),
            WakeResolvePendingOutcome::SetMismatch
        );
        assert_eq!(
            repo.resolve_pending_exact(&resolve_input(
                WorkflowId(116),
                Version(2),
                TransitionId(3),
                vec![DeliveryId(1), DeliveryId(999)]
            ))
            .await
            .unwrap(),
            WakeResolvePendingOutcome::SetMismatch
        );
        assert_eq!(
            repo.resolve_pending_exact(&resolve_input(
                WorkflowId(116),
                Version(2),
                TransitionId(3),
                vec![DeliveryId(1), DeliveryId(1)]
            ))
            .await
            .unwrap(),
            WakeResolvePendingOutcome::SetMismatch
        );
    }

    #[tokio::test]
    async fn resolve_pending_already_resolved_and_set_mismatch_do_not_mutate() {
        let (_dir, repo, _) = open_repo_pair().await;
        let canonical = unwrap_started(register_and_begin(&repo, WorkflowId(117)).await);
        repo.record_terminal_evidence(
            WorkflowId(117),
            canonical.authority.as_ref().unwrap(),
            1,
            ReceiptId(1),
            DeliveryId(1),
            Timestamp(20),
            &bash_evidence(19),
        )
        .await
        .unwrap();
        let first = resolve_input(
            WorkflowId(117),
            Version(2),
            TransitionId(3),
            vec![DeliveryId(1)],
        );
        assert_eq!(
            repo.resolve_pending_exact(&first).await.unwrap(),
            WakeResolvePendingOutcome::Resolved
        );

        let before = repo
            .workflow_repo
            .fetch_workflow_head(WorkflowId(117))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            repo.resolve_pending_exact(&resolve_input(
                WorkflowId(117),
                Version(3),
                TransitionId(4),
                vec![DeliveryId(1)]
            ))
            .await
            .unwrap(),
            WakeResolvePendingOutcome::AlreadyResolved
        );
        assert_eq!(
            repo.resolve_pending_exact(&resolve_input(
                WorkflowId(117),
                Version(3),
                TransitionId(4),
                vec![DeliveryId(999)]
            ))
            .await
            .unwrap(),
            WakeResolvePendingOutcome::SetMismatch
        );
        let after = repo
            .workflow_repo
            .fetch_workflow_head(WorkflowId(117))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(before.version, after.version);
        let deliveries = repo
            .workflow_repo
            .list_deliveries(WorkflowId(117))
            .await
            .unwrap();
        assert_eq!(deliveries.len(), 1);
        assert_eq!(
            deliveries[0].status,
            phoenix_workflow::DeliveryStatus::Accepted
        );
    }

    #[tokio::test]
    async fn concurrent_public_register_allocates_distinct_workflow_ids_for_distinct_intents() {
        let (_dir, first, second) = open_repo_pair().await;
        let left_input = intent();
        let mut right_input = intent();
        right_input.contract_id = "contract-2".into();
        right_input.registering_tool_use_id = "tool-2".into();
        right_input.resource = WakeResourceIdentity::Bash(wake_types::BashResourceIdentity {
            work_scope: wake_types::WorkScopeIdentity {
                kind: wake_types::WorkScopeKind::Conversation,
                stable_key: "conv-1".into(),
            },
            handle_id: "b-2".into(),
        });

        let (left, right) = tokio::join!(
            first.register(&left_input, "fp-1", Timestamp(10)),
            second.register(&right_input, "fp-2", Timestamp(10))
        );

        let left_id = match left.unwrap() {
            WakeRegistrationOutcome::Registered { workflow_id, .. }
            | WakeRegistrationOutcome::Replayed { workflow_id, .. } => workflow_id,
            WakeRegistrationOutcome::Conflict => panic!("expected registration success"),
        };
        let right_id = match right.unwrap() {
            WakeRegistrationOutcome::Registered { workflow_id, .. }
            | WakeRegistrationOutcome::Replayed { workflow_id, .. } => workflow_id,
            WakeRegistrationOutcome::Conflict => panic!("expected registration success"),
        };
        assert_ne!(left_id, right_id);
    }

    #[tokio::test]
    async fn replay_register_does_not_allocate_or_create_extra_workflow() {
        let (_dir, repo, _) = open_repo_pair().await;
        let input = intent();

        let first = repo.register(&input, "fp-1", Timestamp(10)).await.unwrap();
        let first_id = match first {
            WakeRegistrationOutcome::Registered { workflow_id, .. } => workflow_id,
            other @ (WakeRegistrationOutcome::Replayed { .. }
            | WakeRegistrationOutcome::Conflict) => {
                panic!("expected registered, got {other:?}")
            }
        };
        let before_next = next_test_workflow_id(&repo).await.unwrap();
        assert_eq!(before_next.0, first_id.0 + 1);

        let replay = repo.register(&input, "fp-1", Timestamp(10)).await.unwrap();
        match replay {
            WakeRegistrationOutcome::Replayed { workflow_id, .. } => {
                assert_eq!(workflow_id, first_id);
            }
            other @ (WakeRegistrationOutcome::Registered { .. }
            | WakeRegistrationOutcome::Conflict) => {
                panic!("expected replay, got {other:?}")
            }
        }

        let rows = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM workflows")
            .fetch_one(&repo.workflow_repo.pool)
            .await
            .unwrap();
        assert_eq!(rows, 1);
        let after_next = next_test_workflow_id(&repo).await.unwrap();
        assert_eq!(after_next.0, before_next.0 + 1);
    }

    #[tokio::test]
    async fn per_workflow_sequence_values_are_unique_under_concurrent_transactions() {
        let (_dir, first, second) = open_repo_pair().await;
        let workflow_id = registered_workflow_id(&first, &intent(), "fp-1").await;

        let (left, right) = tokio::join!(
            async {
                let mut tx = first.workflow_repo.begin_tx().await.unwrap();
                let seq = tx
                    .allocate_sequence_value(workflow_id, WorkflowSequenceName::Receipt)
                    .await
                    .unwrap();
                tx.commit().await.unwrap();
                seq
            },
            async {
                let mut tx = second.workflow_repo.begin_tx().await.unwrap();
                let seq = tx
                    .allocate_sequence_value(workflow_id, WorkflowSequenceName::Receipt)
                    .await
                    .unwrap();
                tx.commit().await.unwrap();
                seq
            }
        );

        assert_ne!(left, right);
        let mut values = [left, right];
        values.sort_unstable();
        assert_eq!(values, [1, 2]);
    }

    #[tokio::test]
    async fn discovery_queries_are_bounded_ordered_and_restart_consistent() {
        let (_dir, repo, restarted) = open_repo_pair().await;
        insert_conversation(&repo.workflow_repo.pool, "conv-2").await;

        let active_a = WorkflowId(600);
        let active_b = WorkflowId(601);
        let pending = WorkflowId(602);
        let expired = WorkflowId(603);
        let leased = WorkflowId(604);

        assert!(matches!(
            repo.register_allocated(active_a, &intent(), "fp-a", Timestamp(10))
                .await
                .unwrap(),
            WakeRegistrationOutcome::Registered { .. }
        ));

        let mut intent_b = intent();
        intent_b.contract_id = "contract-b".into();
        intent_b.registering_tool_use_id = "tool-b".into();
        intent_b.resource = WakeResourceIdentity::Bash(wake_types::BashResourceIdentity {
            work_scope: wake_types::WorkScopeIdentity {
                kind: wake_types::WorkScopeKind::Conversation,
                stable_key: "conv-1".into(),
            },
            handle_id: "b-active-b".into(),
        });
        assert!(matches!(
            repo.register_allocated(active_b, &intent_b, "fp-b", Timestamp(10))
                .await
                .unwrap(),
            WakeRegistrationOutcome::Registered { .. }
        ));

        let pending_conv = "conv-0";
        insert_conversation(&repo.workflow_repo.pool, pending_conv).await;
        let pending_input = WakeRegistrationIntent {
            contract_id: "contract-p".into(),
            conversation_id: pending_conv.into(),
            registration_scope: wake_types::WorkScopeIdentity {
                kind: wake_types::WorkScopeKind::Conversation,
                stable_key: pending_conv.into(),
            },
            resource: WakeResourceIdentity::Bash(wake_types::BashResourceIdentity {
                work_scope: wake_types::WorkScopeIdentity {
                    kind: wake_types::WorkScopeKind::Conversation,
                    stable_key: pending_conv.into(),
                },
                handle_id: "b-pending".into(),
            }),
            registering_tool_use_id: "tool-p".into(),
            registered_at: Timestamp(10),
            expires_at: Timestamp(100),
        };
        assert!(matches!(
            repo.register_allocated(pending, &pending_input, "fp-p", Timestamp(10))
                .await
                .unwrap(),
            WakeRegistrationOutcome::Registered { .. }
        ));
        let pending_started = unwrap_started(
            repo.claim_observation_if_eligible(
                pending,
                ProcessIncarnation(1),
                Timestamp(20),
                phoenix_workflow::LeaseExpiry(30),
            )
            .await
            .unwrap(),
        );
        assert_eq!(pending_started.attempt.as_ref().unwrap().id, AttemptId(1));
        repo.record_terminal_evidence(
            pending,
            pending_started.authority.as_ref().unwrap(),
            1,
            ReceiptId(1),
            DeliveryId(1),
            Timestamp(20),
            &bash_evidence(19),
        )
        .await
        .unwrap();

        let mut expired_intent = intent();
        expired_intent.contract_id = "contract-e".into();
        expired_intent.registering_tool_use_id = "tool-e".into();
        expired_intent.expires_at = Timestamp(15);
        expired_intent.resource = WakeResourceIdentity::Bash(wake_types::BashResourceIdentity {
            work_scope: wake_types::WorkScopeIdentity {
                kind: wake_types::WorkScopeKind::Conversation,
                stable_key: "conv-1".into(),
            },
            handle_id: "b-expired".into(),
        });
        assert!(matches!(
            repo.register_allocated(expired, &expired_intent, "fp-e", Timestamp(10))
                .await
                .unwrap(),
            WakeRegistrationOutcome::Registered { .. }
        ));

        let mut leased_intent = intent();
        leased_intent.contract_id = "contract-l".into();
        leased_intent.registering_tool_use_id = "tool-l".into();
        leased_intent.resource = WakeResourceIdentity::Bash(wake_types::BashResourceIdentity {
            work_scope: wake_types::WorkScopeIdentity {
                kind: wake_types::WorkScopeKind::Conversation,
                stable_key: "conv-1".into(),
            },
            handle_id: "b-leased".into(),
        });
        assert!(matches!(
            repo.register_allocated(leased, &leased_intent, "fp-l", Timestamp(10))
                .await
                .unwrap(),
            WakeRegistrationOutcome::Registered { .. }
        ));
        unwrap_started(
            repo.claim_observation_if_eligible(
                leased,
                ProcessIncarnation(1),
                Timestamp(20),
                phoenix_workflow::LeaseExpiry(25),
            )
            .await
            .unwrap(),
        );

        let active = repo.list_active_unresolved(2).await.unwrap();
        assert_eq!(
            active.iter().map(|row| row.workflow_id).collect::<Vec<_>>(),
            vec![active_a, active_b]
        );

        let conv_active = repo
            .list_active_unresolved_for_conversation("conv-1")
            .await
            .unwrap();
        assert_eq!(
            conv_active
                .iter()
                .map(|row| row.workflow_id)
                .collect::<Vec<_>>(),
            vec![active_a, active_b, expired, leased]
        );
        let exact = repo
            .fetch_binding_for_conversation_contract("conv-1", "contract-b")
            .await
            .unwrap()
            .expect("exact binding");
        assert_eq!(exact.workflow_id, active_b);
        assert!(repo
            .fetch_binding_for_conversation_contract("conv-2", "contract-b")
            .await
            .unwrap()
            .is_none());

        let observation = repo
            .list_observation_candidates(Timestamp(30), None, 2)
            .await
            .unwrap();
        assert_eq!(observation.len(), 2);
        assert_eq!(observation[0].workflow_id, active_a);
        assert_eq!(
            observation[0].reason,
            WakeObservationCandidateReason::NoLiveAttempt
        );
        assert_eq!(observation[1].workflow_id, active_b);
        assert_eq!(
            observation[1].reason,
            WakeObservationCandidateReason::NoLiveAttempt
        );

        let observation_restarted = restarted
            .list_observation_candidates(Timestamp(30), None, 5)
            .await
            .unwrap();
        assert_eq!(
            observation_restarted
                .iter()
                .map(|row| (row.workflow_id, row.reason))
                .collect::<Vec<_>>(),
            vec![
                (active_a, WakeObservationCandidateReason::NoLiveAttempt),
                (active_b, WakeObservationCandidateReason::NoLiveAttempt),
                (pending, WakeObservationCandidateReason::ExpiredLease),
                (expired, WakeObservationCandidateReason::NoLiveAttempt),
                (leased, WakeObservationCandidateReason::ExpiredLease),
            ]
        );

        let expired_rows = repo
            .list_expired_unresolved(Timestamp(30), 1)
            .await
            .unwrap();
        assert_eq!(expired_rows.len(), 1);
        assert_eq!(expired_rows[0].workflow_id, expired);

        let expired_restarted = restarted
            .list_expired_unresolved(Timestamp(30), 5)
            .await
            .unwrap();
        assert_eq!(
            expired_restarted
                .iter()
                .map(|row| row.workflow_id)
                .collect::<Vec<_>>(),
            vec![expired]
        );

        let pending_rows = repo.list_pending_global(None, 1).await.unwrap();
        assert!(pending_rows.is_empty());
        let pending_restarted = restarted.list_pending_global(None, 5).await.unwrap();
        assert!(pending_restarted.is_empty());
        let pending_local = repo.list_pending(pending_conv).await.unwrap();
        assert!(pending_local.is_empty());
        let pending_local_restarted = restarted.list_pending(pending_conv).await.unwrap();
        assert!(pending_local_restarted.is_empty());
        let pending_original = repo.list_pending("conv-1").await.unwrap();
        assert!(pending_original.is_empty());
        let pending_original_restarted = restarted.list_pending("conv-1").await.unwrap();
        assert!(pending_original_restarted.is_empty());
    }

    #[tokio::test]
    async fn pending_global_cursor_pages_past_an_unchanged_first_page() {
        let (_dir, repo, _) = open_repo_pair().await;
        for workflow_id in 8000..8003 {
            let mut input = intent();
            input.contract_id = format!("contract-{workflow_id}");
            input.registering_tool_use_id = format!("tool-{workflow_id}");
            input.resource = WakeResourceIdentity::Bash(wake_types::BashResourceIdentity {
                work_scope: input.registration_scope.clone(),
                handle_id: format!("b-{workflow_id}"),
            });
            assert!(matches!(
                repo.register_allocated(
                    WorkflowId(workflow_id),
                    &input,
                    &format!("fp-{workflow_id}"),
                    Timestamp(10),
                )
                .await
                .unwrap(),
                WakeRegistrationOutcome::Registered { .. }
            ));
            let started = unwrap_started(
                repo.claim_observation_if_eligible(
                    WorkflowId(workflow_id),
                    ProcessIncarnation(1),
                    Timestamp(20),
                    phoenix_workflow::LeaseExpiry(30),
                )
                .await
                .unwrap(),
            );
            let evidence = WakeTerminalEvidence::Bash(BashTerminalEvidence {
                identity: wake_types::BashResourceIdentity {
                    work_scope: input.registration_scope,
                    handle_id: format!("b-{workflow_id}"),
                },
                status: wake_types::BashTerminalStatus::Exited,
                occurred_at: Timestamp(19),
                exit_code: Some(0),
                duration_ms: Some(1),
                signal_number: None,
                kill_signal_sent: None,
                final_tail: vec![],
            });
            assert!(matches!(
                repo.record_terminal_evidence(
                    WorkflowId(workflow_id),
                    started.authority.as_ref().unwrap(),
                    1,
                    ReceiptId(1),
                    DeliveryId(1),
                    Timestamp(20),
                    &evidence,
                )
                .await
                .unwrap(),
                WakeTerminalEvidenceOutcome::Recorded { .. }
            ));
        }

        let first = repo.list_pending_global(None, 2).await.unwrap();
        assert_eq!(first.len(), 2);
        let cursor = WakePendingGlobalCursor {
            workflow_id: first[1].workflow_id,
            delivery_id: first[1].delivery_id,
        };
        let second = repo.list_pending_global(Some(cursor), 2).await.unwrap();
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].workflow_id, WorkflowId(8002));
    }

    #[tokio::test]
    async fn claim_observation_reclaims_expired_lease_and_starts_fresh_attempt() {
        let (_dir, repo, restarted) = open_repo_pair().await;
        let workflow_id = WorkflowId(700);
        let started = unwrap_started(register_and_begin(&repo, workflow_id).await);
        let first_authority = started.authority.expect("authority");
        assert_eq!(first_authority.attempt_id, AttemptId(1));

        let reclaimed = restarted
            .claim_observation_if_eligible(
                workflow_id,
                ProcessIncarnation(2),
                Timestamp(31),
                phoenix_workflow::LeaseExpiry(40),
            )
            .await
            .unwrap();
        let reclaimed = unwrap_started(reclaimed);
        let authority = reclaimed.authority.expect("authority");
        assert!(authority.attempt_id.0 > first_authority.attempt_id.0);

        let stale = repo
            .renew_observation_lease(
                &first_authority,
                Timestamp(31),
                phoenix_workflow::LeaseExpiry(35),
            )
            .await
            .unwrap();
        assert_eq!(stale, WakeLeaseRenewalOutcome::Stale);
    }

    #[tokio::test]
    async fn renew_observation_lease_fences_stale_and_requires_monotonic_future() {
        let (_dir, repo, restarted) = open_repo_pair().await;
        let workflow_id = WorkflowId(701);
        let started = unwrap_started(register_and_begin(&repo, workflow_id).await);
        let authority = started.authority.expect("authority");
        assert_eq!(
            repo.renew_observation_lease(
                &authority,
                Timestamp(21),
                phoenix_workflow::LeaseExpiry(35),
            )
            .await
            .unwrap(),
            WakeLeaseRenewalOutcome::Renewed
        );
        assert_eq!(
            repo.renew_observation_lease(
                &authority,
                Timestamp(21),
                phoenix_workflow::LeaseExpiry(34),
            )
            .await
            .unwrap(),
            WakeLeaseRenewalOutcome::Stale
        );
        let replacement = unwrap_started(
            restarted
                .claim_observation_if_eligible(
                    workflow_id,
                    ProcessIncarnation(2),
                    Timestamp(36),
                    phoenix_workflow::LeaseExpiry(50),
                )
                .await
                .unwrap(),
        );
        assert!(replacement.authority.expect("authority").attempt_id.0 > authority.attempt_id.0);
        assert_eq!(
            repo.renew_observation_lease(
                &authority,
                Timestamp(36),
                phoenix_workflow::LeaseExpiry(45),
            )
            .await
            .unwrap(),
            WakeLeaseRenewalOutcome::Stale
        );
    }

    #[tokio::test]
    async fn expire_if_unresolved_not_due_returns_typed_not_due() {
        let (_dir, repo, _) = open_repo_pair().await;
        let workflow_id = registered_workflow_id(&repo, &intent(), "fp-1").await;
        assert_eq!(
            repo.expire_if_unresolved(workflow_id, Timestamp(99))
                .await
                .unwrap(),
            WakeExpireIfUnresolvedOutcome::NotDue
        );
    }

    #[tokio::test]
    async fn record_terminal_evidence_rejects_evidence_after_binding_expiry() {
        let (_dir, repo, restarted) = open_repo_pair().await;
        let workflow_id = WorkflowId(1702);
        let input = intent();
        assert!(matches!(
            repo.register_allocated(workflow_id, &input, "fp-expired-evidence", Timestamp(10))
                .await
                .unwrap(),
            WakeRegistrationOutcome::Registered { .. }
        ));
        let started = unwrap_started(
            repo.claim_observation_if_eligible(
                workflow_id,
                ProcessIncarnation(1),
                Timestamp(101),
                phoenix_workflow::LeaseExpiry(130),
            )
            .await
            .unwrap(),
        );
        let authority = started.authority.expect("authority");

        let outcome = repo
            .record_terminal_evidence(
                workflow_id,
                &authority,
                1,
                ReceiptId(1),
                DeliveryId(1),
                Timestamp(101),
                &bash_evidence(101),
            )
            .await
            .unwrap();
        assert_eq!(outcome, WakeTerminalEvidenceOutcome::EvidenceAfterExpiry);
        assert!(repo.list_pending("conv-1").await.unwrap().is_empty());
        let (_, snapshot) = head_snapshot(&restarted, workflow_id).await;
        assert!(snapshot.terminal.is_none());
    }

    #[tokio::test]
    async fn record_terminal_evidence_allows_occurrence_at_expiry_even_when_observed_later() {
        let (_dir, repo, restarted) = open_repo_pair().await;
        let workflow_id = WorkflowId(1703);
        let input = intent();
        assert!(matches!(
            repo.register_allocated(workflow_id, &input, "fp-expiry-edge", Timestamp(10))
                .await
                .unwrap(),
            WakeRegistrationOutcome::Registered { .. }
        ));
        let started = unwrap_started(
            repo.claim_observation_if_eligible(
                workflow_id,
                ProcessIncarnation(1),
                Timestamp(101),
                phoenix_workflow::LeaseExpiry(130),
            )
            .await
            .unwrap(),
        );
        let authority = started.authority.expect("authority");

        let outcome = repo
            .record_terminal_evidence(
                workflow_id,
                &authority,
                1,
                ReceiptId(1),
                DeliveryId(1),
                Timestamp(101),
                &bash_evidence(100),
            )
            .await
            .unwrap();
        let delivery = match outcome {
            WakeTerminalEvidenceOutcome::Recorded { delivery, .. }
            | WakeTerminalEvidenceOutcome::Replayed { delivery, .. } => delivery,
            other @ (WakeTerminalEvidenceOutcome::StaleAttempt
            | WakeTerminalEvidenceOutcome::WrongResource
            | WakeTerminalEvidenceOutcome::EvidenceAfterObservation
            | WakeTerminalEvidenceOutcome::EvidenceAfterExpiry) => {
                panic!("expected recorded/replayed, got {other:?}")
            }
        };
        assert!(matches!(
            delivery.receipt.terminal,
            WakeTerminalPayload::Fired { .. }
        ));
        let pending = restarted.list_pending("conv-1").await.unwrap();
        assert_eq!(pending.len(), 1);
        assert!(matches!(
            pending[0].receipt.terminal,
            WakeTerminalPayload::Fired { .. }
        ));
    }

    #[tokio::test]
    async fn expire_if_unresolved_records_expired_and_updates_snapshot() {
        let (_dir, repo, restarted) = open_repo_pair().await;
        let workflow_id = WorkflowId(702);
        let started = unwrap_started(register_and_begin(&repo, workflow_id).await);
        let authority = started.authority.expect("authority");
        let expired = repo
            .expire_if_unresolved(workflow_id, Timestamp(100))
            .await
            .unwrap();
        let (receipt_id, delivery_id) = match expired {
            WakeExpireIfUnresolvedOutcome::Expired { receipt, delivery } => {
                assert!(matches!(
                    delivery.receipt.terminal,
                    WakeTerminalPayload::Expired { .. }
                ));
                (receipt.receipt_id, delivery.canonical_delivery.delivery_id)
            }
            other @ (WakeExpireIfUnresolvedOutcome::Replayed { .. }
            | WakeExpireIfUnresolvedOutcome::NotDue
            | WakeExpireIfUnresolvedOutcome::Stale) => {
                panic!("expected expired, got {other:?}")
            }
        };
        assert_eq!(
            fetch_receipt_origin(&repo, workflow_id, receipt_id).await,
            ReceiptOrigin::DeadlineExpiration
        );
        assert_eq!(
            repo.renew_observation_lease(
                &authority,
                Timestamp(100),
                phoenix_workflow::LeaseExpiry(110),
            )
            .await
            .unwrap(),
            WakeLeaseRenewalOutcome::Stale
        );
        assert_snapshot_projection_parity(
            &restarted,
            workflow_id,
            "conv-1",
            wake_profile::RuntimeAvailability::Idle,
            true,
            is_expired,
        )
        .await;
        assert_eq!(
            restarted
                .resolve_pending_exact(&resolve_input(
                    workflow_id,
                    Version(2),
                    TransitionId(3),
                    vec![delivery_id],
                ))
                .await
                .unwrap(),
            WakeResolvePendingOutcome::Resolved
        );
        assert_snapshot_projection_parity(
            &restarted,
            workflow_id,
            "conv-1",
            wake_profile::RuntimeAvailability::Accepted,
            false,
            is_expired,
        )
        .await;
    }

    #[tokio::test]
    async fn fired_snapshot_projection_parity_before_and_after_suppress_restart() {
        let (_dir, repo, restarted) = open_repo_pair().await;
        let workflow_id = WorkflowId(704);
        let canonical = unwrap_started(register_and_begin(&repo, workflow_id).await);
        repo.record_terminal_evidence(
            workflow_id,
            canonical.authority.as_ref().unwrap(),
            1,
            ReceiptId(1),
            DeliveryId(1),
            Timestamp(20),
            &bash_evidence(19),
        )
        .await
        .unwrap();
        assert_snapshot_projection_parity(
            &restarted,
            workflow_id,
            "conv-1",
            wake_profile::RuntimeAvailability::Idle,
            true,
            is_fired,
        )
        .await;

        let mut input = resolve_input(
            workflow_id,
            Version(2),
            TransitionId(3),
            vec![DeliveryId(1)],
        );
        input.decision = WakeResolveDecision::Suppress;
        assert_eq!(
            restarted.resolve_pending_exact(&input).await.unwrap(),
            WakeResolvePendingOutcome::Resolved
        );
        assert_snapshot_projection_parity(
            &restarted,
            workflow_id,
            "conv-1",
            wake_profile::RuntimeAvailability::Suppressed,
            false,
            is_fired,
        )
        .await;
    }

    #[tokio::test]
    async fn transfer_preserves_snapshot_bytes_and_semantics() {
        let (_dir, repo, restarted) = open_repo_pair().await;
        insert_conversation(&repo.workflow_repo.pool, "conv-2").await;
        let workflow_id = WorkflowId(705);
        let canonical = unwrap_started(register_and_begin(&repo, workflow_id).await);
        repo.record_terminal_evidence(
            workflow_id,
            canonical.authority.as_ref().unwrap(),
            1,
            ReceiptId(1),
            DeliveryId(1),
            Timestamp(20),
            &bash_evidence(19),
        )
        .await
        .unwrap();
        let (before_head, before_snapshot) = head_snapshot(&repo, workflow_id).await;
        assert_eq!(
            repo.transfer(&transfer_input(
                workflow_id,
                "conv-1",
                "conv-2",
                Version(2),
                vec![DeliveryId(1)],
                TransitionId(3),
            ))
            .await
            .unwrap(),
            WakeTransferOutcome::Transferred
        );
        let (after_head, after_snapshot) = head_snapshot(&restarted, workflow_id).await;
        assert_eq!(before_snapshot, after_snapshot);
        assert_eq!(before_head.snapshot_payload, after_head.snapshot_payload);
        assert_eq!(after_head.version, Version(3));
        assert_snapshot_projection_parity(
            &restarted,
            workflow_id,
            "conv-2",
            wake_profile::RuntimeAvailability::Idle,
            true,
            is_fired,
        )
        .await;
    }

    #[tokio::test]
    async fn transfer_moves_materialized_link_and_message_to_new_owner_pending_delivery() {
        let (_dir, repo, restarted) = open_repo_pair().await;
        insert_conversation(&repo.workflow_repo.pool, "conv-2").await;
        let workflow_id = WorkflowId(7051);
        let pending = create_pending_terminal_delivery(&repo, workflow_id).await;
        let materialized = materialize_pending(
            &repo,
            &pending,
            "wake complete",
            Some(serde_json::json!({"kind": "wake"})),
            true,
            Timestamp(50),
        )
        .await;
        let linked = match materialized {
            MaterializePendingDeliveryMessageOutcome::Materialized(link)
            | MaterializePendingDeliveryMessageOutcome::AlreadyMaterialized(link) => link,
            MaterializePendingDeliveryMessageOutcome::WrongOwnerOrIneligible => {
                panic!("expected link")
            }
        };

        assert_eq!(
            repo.transfer(&transfer_input(
                workflow_id,
                "conv-1",
                "conv-2",
                Version(2),
                vec![DeliveryId(1)],
                TransitionId(3),
            ))
            .await
            .unwrap(),
            WakeTransferOutcome::Transferred
        );
        assert!(repo.list_pending("conv-1").await.unwrap().is_empty());
        let pending = restarted.list_pending("conv-2").await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(
            restarted
                .get_delivery_message_link(workflow_id, DeliveryId(1))
                .await
                .unwrap()
                .unwrap()
                .conversation_id,
            "conv-2"
        );
        let link_owner: String = sqlx::query_scalar(
            "SELECT conversation_id FROM wake_delivery_messages WHERE workflow_id = ?1 AND delivery_id = 1",
        )
        .bind(7051_i64)
        .fetch_one(&repo.workflow_repo.pool)
        .await
        .unwrap();
        let message_owner: String =
            sqlx::query_scalar("SELECT conversation_id FROM messages WHERE message_id = ?1")
                .bind(&linked.linked_message.message.message_id)
                .fetch_one(&repo.workflow_repo.pool)
                .await
                .unwrap();
        assert_eq!(link_owner, "conv-2");
        assert_eq!(message_owner, "conv-2");
    }

    #[tokio::test]
    async fn worktree_scope_projection_reload_preserves_exact_scope() {
        let (_dir, first, second) = open_repo_pair().await;
        let workflow_id = WorkflowId(703);
        let mut input = intent();
        input.conversation_id = "conv-wt".into();
        insert_conversation(&first.workflow_repo.pool, "conv-wt").await;

        input.registration_scope = wake_types::WorkScopeIdentity {
            kind: wake_types::WorkScopeKind::Worktree,
            stable_key: "worktree:/tmp/demo".into(),
        };
        input.resource = WakeResourceIdentity::Bash(wake_types::BashResourceIdentity {
            work_scope: input.registration_scope.clone(),
            handle_id: "b-wt".into(),
        });
        assert!(matches!(
            first
                .register_allocated(workflow_id, &input, "fp-wt", Timestamp(10))
                .await
                .unwrap(),
            WakeRegistrationOutcome::Registered { .. }
        ));
        let started = unwrap_started(
            first
                .claim_observation_if_eligible(
                    workflow_id,
                    ProcessIncarnation(1),
                    Timestamp(20),
                    phoenix_workflow::LeaseExpiry(30),
                )
                .await
                .unwrap(),
        );
        first
            .record_terminal_evidence(
                workflow_id,
                started.authority.as_ref().unwrap(),
                1,
                ReceiptId(1),
                DeliveryId(1),
                Timestamp(20),
                &WakeTerminalEvidence::Bash(BashTerminalEvidence {
                    identity: wake_types::BashResourceIdentity {
                        work_scope: input.registration_scope.clone(),
                        handle_id: "b-wt".into(),
                    },
                    status: wake_types::BashTerminalStatus::Exited,
                    occurred_at: Timestamp(19),
                    exit_code: Some(0),
                    duration_ms: Some(1),
                    signal_number: None,
                    kill_signal_sent: None,
                    final_tail: vec![],
                }),
            )
            .await
            .unwrap();
        let pending = second.list_pending("conv-wt").await.unwrap();
        assert_eq!(pending.len(), 1);
        match &pending[0].receipt.terminal {
            WakeTerminalPayload::Fired {
                resource: WakeResourceIdentity::Bash(identity),
                ..
            } => {
                assert_eq!(
                    identity.work_scope.kind,
                    wake_types::WorkScopeKind::Worktree
                );
                assert_eq!(identity.work_scope.stable_key, "worktree:/tmp/demo");
            }
            other @ (WakeTerminalPayload::Fired { .. }
            | WakeTerminalPayload::Cancelled { .. }
            | WakeTerminalPayload::Expired { .. }
            | WakeTerminalPayload::Forgotten { .. }) => {
                panic!("expected fired bash receipt, got {other:?}")
            }
        }
    }

    #[tokio::test]
    async fn pending_exact_lookup_matches_owner_and_preserves_tail_projection() {
        let (_dir, first, second) = open_repo_pair().await;
        let input = tmux_intent();
        let workflow_id = next_test_workflow_id(&first).await.unwrap();
        first
            .register_allocated(workflow_id, &input, "fp-exact", Timestamp(10))
            .await
            .unwrap();
        assert_eq!(
            second
                .fetch_binding(workflow_id)
                .await
                .unwrap()
                .unwrap()
                .resource,
            input.resource
        );
        let started = first
            .claim_observation_if_eligible(
                workflow_id,
                ProcessIncarnation(1),
                Timestamp(20),
                phoenix_workflow::LeaseExpiry(30),
            )
            .await
            .unwrap();
        let canonical = unwrap_started(started);
        first
            .record_terminal_evidence(
                workflow_id,
                canonical.authority.as_ref().unwrap(),
                1,
                ReceiptId(1),
                DeliveryId(1),
                Timestamp(20),
                &tmux_evidence(18),
            )
            .await
            .unwrap();

        let exact = second
            .get_pending_exact(workflow_id, DeliveryId(1), "conv-1")
            .await
            .unwrap()
            .expect("owned pending delivery");
        assert_eq!(exact.workflow_id, workflow_id);
        assert_eq!(exact.canonical_delivery.delivery_id, DeliveryId(1));
        match exact.receipt.terminal {
            WakeTerminalPayload::Fired {
                evidence: WakeTerminalEvidence::TmuxWindow(ref ev),
                ..
            } => {
                assert_eq!(ev.final_tail, vec!["tail-1"]);
            }
            other @ (WakeTerminalPayload::Fired { .. }
            | WakeTerminalPayload::Cancelled { .. }
            | WakeTerminalPayload::Expired { .. }
            | WakeTerminalPayload::Forgotten { .. }) => {
                panic!("expected fired tmux receipt, got {other:?}")
            }
        }

        assert!(second
            .get_pending_exact(workflow_id, DeliveryId(1), "conv-2")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn conversation_adoption_detects_missing_materialization_from_single_conversation_scan() {
        let (_dir, repo, _) = open_repo_pair().await;
        let workflow_id = next_test_workflow_id(&repo).await.unwrap();
        repo.register_allocated(
            workflow_id,
            &intent(),
            "fp-missing-materialized",
            Timestamp(10),
        )
        .await
        .unwrap();
        let started = unwrap_started(
            repo.claim_observation_if_eligible(
                workflow_id,
                ProcessIncarnation(1),
                Timestamp(20),
                phoenix_workflow::LeaseExpiry(30),
            )
            .await
            .unwrap(),
        );
        assert!(matches!(
            repo.record_terminal_evidence(
                workflow_id,
                started.authority.as_ref().unwrap(),
                1,
                ReceiptId(1),
                DeliveryId(1),
                Timestamp(20),
                &bash_evidence(19),
            )
            .await
            .unwrap(),
            WakeTerminalEvidenceOutcome::Recorded { .. }
        ));

        let outcome = repo
            .adopt_materialized_pending_for_conversation("conv-1", Timestamp(21))
            .await
            .unwrap();
        match outcome {
            WakeAdoptMaterializedPendingOutcome::NotFullyMaterialized { delivery_ids } => {
                assert_eq!(delivery_ids, vec![DeliveryId(1)]);
            }
            other @ (WakeAdoptMaterializedPendingOutcome::Adopted(_)
            | WakeAdoptMaterializedPendingOutcome::Busy(_)
            | WakeAdoptMaterializedPendingOutcome::NothingPending) => {
                panic!("expected not fully materialized, got {other:?}")
            }
        }
    }

    #[tokio::test]
    async fn restart_reload_lists_pending_projection() {
        let (_dir, first, second) = open_repo_pair().await;
        let input = tmux_intent();
        let workflow_id = next_test_workflow_id(&first).await.unwrap();
        first
            .register_allocated(workflow_id, &input, "fp-2", Timestamp(10))
            .await
            .unwrap();
        let started = first
            .claim_observation_if_eligible(
                workflow_id,
                ProcessIncarnation(1),
                Timestamp(20),
                phoenix_workflow::LeaseExpiry(30),
            )
            .await
            .unwrap();
        let canonical = unwrap_started(started);
        first
            .record_terminal_evidence(
                workflow_id,
                canonical.authority.as_ref().unwrap(),
                1,
                ReceiptId(1),
                DeliveryId(1),
                Timestamp(20),
                &tmux_evidence(18),
            )
            .await
            .unwrap();
        let pending = second.list_pending("conv-1").await.unwrap();
        assert_eq!(pending.len(), 1);
        assert!(matches!(
            pending[0].receipt.terminal,
            WakeTerminalPayload::Fired { .. }
        ));
    }
}
