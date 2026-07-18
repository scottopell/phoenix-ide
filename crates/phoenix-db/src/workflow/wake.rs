#![allow(clippy::too_many_arguments, clippy::type_complexity)]

use super::{
    to_i64, to_u64, wake_profile, AcceptReceiptInput, BeginAttemptInput, BeginAttemptResult,
    ClaimOutcome, CommitOutcome, CommitTransitionPlanCas, CreateWorkflowWithExternalAcceptance,
    DbError, DbResult, DeliveryResolutionDecision, DeliveryResolutionPlan, LocalCodec,
    LocalDeliveryRecord, LocalEffectDecl, LocalReceiptRecord, RecordObservationInput,
    WorkflowRepository, WorkflowTx,
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
    Timestamp, TransitionId, Version, WorkflowId, WorkflowStatus,
};
use serde::Serialize;
use sqlx::Row;
#[cfg(test)]
use std::sync::{Mutex, OnceLock};

#[cfg(test)]
fn fail_after_canonical_transition_set() -> &'static Mutex<std::collections::BTreeSet<u64>> {
    static SET: OnceLock<Mutex<std::collections::BTreeSet<u64>>> = OnceLock::new();
    SET.get_or_init(|| Mutex::new(std::collections::BTreeSet::new()))
}
#[cfg(test)]
fn fail_after_canonical_receipt_set() -> &'static Mutex<std::collections::BTreeSet<u64>> {
    static SET: OnceLock<Mutex<std::collections::BTreeSet<u64>>> = OnceLock::new();
    SET.get_or_init(|| Mutex::new(std::collections::BTreeSet::new()))
}
#[cfg(test)]
fn fail_after_transfer_binding_update_set() -> &'static Mutex<std::collections::BTreeSet<u64>> {
    static SET: OnceLock<Mutex<std::collections::BTreeSet<u64>>> = OnceLock::new();
    SET.get_or_init(|| Mutex::new(std::collections::BTreeSet::new()))
}

#[cfg(test)]
pub fn fail_after_canonical_transition_once(workflow_id: WorkflowId) {
    fail_after_canonical_transition_set()
        .lock()
        .unwrap()
        .insert(workflow_id.0);
}

#[cfg(test)]
pub fn fail_after_canonical_receipt_once(workflow_id: WorkflowId) {
    fail_after_canonical_receipt_set()
        .lock()
        .unwrap()
        .insert(workflow_id.0);
}

#[cfg(test)]
pub fn fail_after_resolve_transition_once(workflow_id: WorkflowId) {
    fail_after_canonical_transition_set()
        .lock()
        .unwrap()
        .insert(workflow_id.0);
}

#[cfg(test)]
pub fn fail_after_transfer_binding_update_once(workflow_id: WorkflowId) {
    fail_after_transfer_binding_update_set()
        .lock()
        .unwrap()
        .insert(workflow_id.0);
}

#[cfg(test)]
fn maybe_fail_after_canonical_transition(workflow_id: WorkflowId) -> DbResult<()> {
    if fail_after_canonical_transition_set()
        .lock()
        .unwrap()
        .remove(&workflow_id.0)
    {
        return Err(DbError::Serialization(
            "test failpoint after canonical transition".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
fn maybe_fail_after_canonical_receipt(workflow_id: WorkflowId) -> DbResult<()> {
    if fail_after_canonical_receipt_set()
        .lock()
        .unwrap()
        .remove(&workflow_id.0)
    {
        return Err(DbError::Serialization(
            "test failpoint after canonical receipt".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
fn maybe_fail_after_transfer_binding_update(workflow_id: WorkflowId) -> DbResult<()> {
    if fail_after_transfer_binding_update_set()
        .lock()
        .unwrap()
        .remove(&workflow_id.0)
    {
        return Err(DbError::Serialization(
            "test failpoint after transfer binding update".to_string(),
        ));
    }
    Ok(())
}

#[cfg(not(test))]
fn maybe_fail_after_canonical_transition(_workflow_id: WorkflowId) {}
#[cfg(not(test))]
fn maybe_fail_after_canonical_receipt(_workflow_id: WorkflowId) {}
#[cfg(not(test))]
fn maybe_fail_after_transfer_binding_update(_workflow_id: WorkflowId) {}

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
    Started { canonical: BeginAttemptResult },
    StaleAttempt,
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
}

impl WakeRepository {
    #[must_use]
    pub fn new(pool: sqlx::SqlitePool) -> Self {
        Self {
            workflow_repo: WorkflowRepository::new(pool),
        }
    }

    pub async fn register(
        &self,
        input: &WakeRegistrationIntent,
        prepared_fingerprint: &str,
        workflow_id: WorkflowId,
        now: Timestamp,
    ) -> DbResult<WakeRegistrationOutcome> {
        for _ in 0..20 {
            match self
                .register_once(input, prepared_fingerprint, workflow_id, now)
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
        self.register_once(input, prepared_fingerprint, workflow_id, now)
            .await
    }

    async fn register_once(
        &self,
        input: &WakeRegistrationIntent,
        prepared_fingerprint: &str,
        workflow_id: WorkflowId,
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

        let snapshot = WakeRegistrationSnapshot {
            contract_id: input.contract_id.clone(),
            resource: input.resource.clone(),
            registered: true,
            terminal: None,
            runtime_availability: wake_profile::RuntimeAvailability::Pending,
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
        maybe_fail_after_canonical_transition(workflow_id)?;
        #[cfg(not(test))]
        maybe_fail_after_canonical_transition(workflow_id);
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

    pub async fn begin_observation(
        &self,
        workflow_id: WorkflowId,
        attempt_id: AttemptId,
        process_incarnation: ProcessIncarnation,
        now: Timestamp,
        lease_until: phoenix_workflow::LeaseExpiry,
    ) -> DbResult<WakeObservationOutcome> {
        let result = self
            .workflow_repo
            .begin_attempt(&BeginAttemptInput {
                workflow_id,
                effect_id: REGISTRATION_EFFECT_ID,
                attempt_id,
                process_incarnation,
                now,
                lease_until: Some(lease_until),
            })
            .await?;
        Ok(match result.outcome {
            ClaimOutcome::Started => WakeObservationOutcome::Started { canonical: result },
            ClaimOutcome::Ineligible
            | ClaimOutcome::AuthorityConflict
            | ClaimOutcome::UnsupportedCodec => WakeObservationOutcome::StaleAttempt,
        })
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
        #[cfg(test)]
        maybe_fail_after_canonical_receipt(workflow_id)?;
        #[cfg(not(test))]
        maybe_fail_after_canonical_receipt(workflow_id);
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
            runtime_availability: wake_profile::RuntimeAvailability::Pending,
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
        maybe_fail_after_canonical_transition(input.workflow_id)?;
        #[cfg(not(test))]
        maybe_fail_after_canonical_transition(input.workflow_id);
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

    pub async fn list_pending(&self, conversation_id: &str) -> DbResult<Vec<WakePendingDelivery>> {
        let mut tx = self.workflow_repo.begin_tx().await?;
        let rows = sqlx::query(
            "SELECT d.workflow_id, d.delivery_id, d.effect_id, d.barrier_id, d.consumer_kind,
                    d.event_codec_family, d.event_codec_version, d.payload_kind, d.payload_blob,
                    d.requires_runtime_acceptance, d.status, d.runtime_acceptance_status,
                    d.suppression_reason, d.accepted_by_transition_id,
                    p.receipt_id, p.conversation_id, p.contract_id, p.resource_kind, p.terminal_kind,
                    p.resolved_at, p.bash_handle_id, p.tmux_server_generation, p.tmux_window_id,
                    p.bash_status, p.tmux_status, p.occurred_at, p.exit_code, p.duration_ms,
                    p.signal_number, p.kill_signal_sent, p.forgotten_reason, p.cancelled_reason,
                    p.cancelled_at
             FROM workflow_deliveries d
             JOIN wake_terminal_receipts p
               ON p.workflow_id = d.workflow_id AND p.delivery_id = d.delivery_id
             WHERE p.conversation_id = ?1 AND d.status = 'Pending'
             ORDER BY d.delivery_id"
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
                fetch_tail_lines_tx(&mut tx, workflow_id, receipt_id).await?,
            )?;
            let delivery = delivery_from_join_row(&row)?;
            pending.push(WakePendingDelivery {
                workflow_id,
                conversation_id: projection.conversation_id.clone(),
                receipt: projection,
                canonical_delivery: delivery,
            });
        }
        tx.commit().await?;
        Ok(pending)
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
            family: head.snapshot_codec.family.to_string(),
            version: head.snapshot_codec.version,
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

        #[cfg(test)]
        maybe_fail_after_transfer_binding_update(input.workflow_id)?;
        #[cfg(not(test))]
        maybe_fail_after_transfer_binding_update(input.workflow_id);

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
        }

        tx.commit().await?;
        Ok(WakeTransferOutcome::Transferred)
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

        let terminal = fetch_any_terminal_projection_tx(&mut tx, input.workflow_id)
            .await?
            .ok_or_else(|| {
                DbError::Serialization("wake pending delivery set missing projection".to_string())
            })?
            .terminal;
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
        let next_snapshot_codec = LocalCodec {
            family: head.snapshot_codec.family.to_string(),
            version: head.snapshot_codec.version,
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
                next_snapshot_payload: &head.snapshot_payload,
                committed_at: input.timestamp,
                exact_delivery_ids: &input.delivery_ids,
                decision,
            })
            .await?;
        match outcome {
            CommitOutcome::Committed => {
                #[cfg(test)]
                maybe_fail_after_canonical_transition(input.workflow_id)?;
                #[cfg(not(test))]
                maybe_fail_after_canonical_transition(input.workflow_id);
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
            format!("tmux:{}:{}", identity.server_generation, identity.window_id)
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
                tmux_server_generation, tmux_window_id, registering_tool_use_id,
                expires_at, prepared_fingerprint
         FROM wake_bindings
         WHERE profile_kind = 'wake' AND profile_version = ?1 AND conversation_id = ?2
           AND contract_id = ?3 AND resource_kind = ?4
           AND COALESCE(bash_handle_id, '') = ?5
           AND COALESCE(tmux_server_generation, '') = ?6
           AND COALESCE(tmux_window_id, '') = ?7",
    )
    .bind(i64::from(wake_profile::PROTOCOL_VERSION))
    .bind(&input.conversation_id)
    .bind(&input.contract_id)
    .bind(resource_kind_str(&input.resource))
    .bind(bash_handle_id(&input.resource).unwrap_or_default())
    .bind(tmux_server_generation(&input.resource).unwrap_or_default())
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
                tmux_server_generation, tmux_window_id, registering_tool_use_id,
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
            tmux_server_generation, tmux_window_id, registering_tool_use_id,
            expires_at, prepared_fingerprint, observe_effect_id, created_at
         ) VALUES (?1, ?2, ?3, 'wake', ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
    )
    .bind(i64::try_from(workflow_id.0).map_err(|e| DbError::Serialization(e.to_string()))?)
    .bind(&input.conversation_id)
    .bind(&input.contract_id)
    .bind(i64::from(wake_profile::PROTOCOL_VERSION))
    .bind(scope_kind_str(&input.registration_scope))
    .bind(&input.registration_scope.stable_key)
    .bind(resource_kind_str(&input.resource))
    .bind(bash_handle_id(&input.resource))
    .bind(tmux_server_generation(&input.resource))
    .bind(tmux_window_id(&input.resource))
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
                server_generation: row.get::<String, _>("tmux_server_generation"),
                window_id: row.get::<String, _>("tmux_window_id"),
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

fn tmux_server_generation(resource: &WakeResourceIdentity) -> Option<String> {
    match resource {
        WakeResourceIdentity::TmuxWindow(identity) => Some(identity.server_generation.clone()),
        WakeResourceIdentity::Bash(_) => None,
        WakeResourceIdentity::Subagent(_) => unreachable!("subagent wake bindings not implemented"),
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
        tmux_server_generation,
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
        "INSERT INTO wake_terminal_receipts (
            workflow_id, receipt_id, delivery_id, conversation_id, contract_id, resource_kind,
            terminal_kind, resolved_at, bash_handle_id, tmux_server_generation, tmux_window_id,
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
    .bind(tmux_server_generation)
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
                Some(identity.server_generation.clone()),
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
                Some(identity.server_generation.clone()),
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
                    Some(identity.server_generation.clone()),
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
                    Some(identity.server_generation.clone()),
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

async fn fetch_any_terminal_projection_tx(
    tx: &mut WorkflowTx<'_>,
    workflow_id: WorkflowId,
) -> DbResult<Option<WakeTerminalReceiptProjection>> {
    let row = sqlx::query(
        "SELECT * FROM wake_terminal_receipts WHERE workflow_id = ?1 ORDER BY receipt_id LIMIT 1",
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
        "SELECT * FROM wake_terminal_receipts WHERE workflow_id = ?1 AND receipt_id = ?2",
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
        "SELECT p.*
         FROM wake_terminal_receipts p
         JOIN workflow_receipts r
           ON r.workflow_id = p.workflow_id AND r.receipt_id = p.receipt_id
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
    match row.get::<String, _>("resource_kind").as_str() {
        "Bash" => Ok(WakeResourceIdentity::Bash(
            wake_types::BashResourceIdentity {
                work_scope: wake_types::WorkScopeIdentity {
                    kind: wake_types::WorkScopeKind::Conversation,
                    stable_key: row.get::<String, _>("conversation_id"),
                },
                handle_id: row.get("bash_handle_id"),
            },
        )),
        "TmuxWindow" => Ok(WakeResourceIdentity::TmuxWindow(
            wake_types::TmuxResourceIdentity {
                work_scope: wake_types::WorkScopeIdentity {
                    kind: wake_types::WorkScopeKind::Conversation,
                    stable_key: row.get::<String, _>("conversation_id"),
                },
                server_generation: row.get("tmux_server_generation"),
                window_id: row.get("tmux_window_id"),
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
    let row = sqlx::query("SELECT * FROM workflow_deliveries WHERE workflow_id = ?1 AND delivery_id = ?2 AND status = 'Pending'")
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
        status: phoenix_workflow::DeliveryStatus::Pending,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrations::run_pending_migrations;
    use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
    use std::str::FromStr;

    async fn setup_repo_schema(pool: &sqlx::SqlitePool) {
        sqlx::query("CREATE TABLE conversations (id TEXT PRIMARY KEY, conv_mode TEXT NOT NULL DEFAULT '{\"mode\":\"Explore\"}', state TEXT NOT NULL DEFAULT '{\"type\":\"idle\"}', cwd TEXT NOT NULL DEFAULT '/tmp', parent_conversation_id TEXT, user_initiated BOOLEAN NOT NULL DEFAULT 1, archived BOOLEAN NOT NULL DEFAULT 0, model TEXT, steering_queue TEXT NOT NULL DEFAULT '[]', state_updated_at TEXT NOT NULL DEFAULT '2025-01-01', created_at TEXT NOT NULL DEFAULT '2025-01-01', updated_at TEXT NOT NULL DEFAULT '2025-01-01')").execute(pool).await.unwrap();
        sqlx::query("CREATE TABLE messages (id INTEGER PRIMARY KEY AUTOINCREMENT, message_id TEXT UNIQUE, conversation_id TEXT NOT NULL, message_type TEXT NOT NULL, content TEXT NOT NULL, sequence_id INTEGER NOT NULL DEFAULT 1, created_at TEXT NOT NULL DEFAULT '2025-01-01')").execute(pool).await.unwrap();
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
                server_generation: "srv-1".into(),
                window_id: "win-1".into(),
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
                server_generation: "srv-1".into(),
                window_id: "win-1".into(),
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
            repo.register(&input, "fp-1", workflow_id, Timestamp(10))
                .await
                .unwrap(),
            WakeRegistrationOutcome::Registered { .. }
        ));
        repo.begin_observation(
            workflow_id,
            AttemptId(1),
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
            other @ WakeObservationOutcome::StaleAttempt => {
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

    #[tokio::test]
    async fn duplicate_concurrent_registration_replays_single_winner() {
        let (_dir, first, second) = open_repo_pair().await;
        let input = intent();
        let (left, right) = tokio::join!(
            first.register(&input, "fp-1", WorkflowId(100), Timestamp(10)),
            second.register(&input, "fp-1", WorkflowId(101), Timestamp(10))
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
            repo.register(&input, "fp-1", WorkflowId(102), Timestamp(10))
                .await
                .unwrap(),
            WakeRegistrationOutcome::Registered { .. }
        ));
        assert_eq!(
            repo.register(&input, "fp-2", WorkflowId(103), Timestamp(10))
                .await
                .unwrap(),
            WakeRegistrationOutcome::Conflict
        );
    }

    #[tokio::test]
    async fn failpoint_rolls_back_everything() {
        let (_dir, repo, _) = open_repo_pair().await;
        let input = intent();
        fail_after_canonical_transition_once(WorkflowId(104));
        let err = repo
            .register(&input, "fp-1", WorkflowId(104), Timestamp(10))
            .await;
        assert!(err.is_err());
        assert!(repo.fetch_binding(WorkflowId(104)).await.unwrap().is_none());
        assert_eq!(
            repo.workflow_repo
                .fetch_workflow_head(WorkflowId(104))
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn restart_reload_finds_binding() {
        let (_dir, first, second) = open_repo_pair().await;
        let input = intent();
        let registered = first
            .register(&input, "fp-1", WorkflowId(105), Timestamp(10))
            .await
            .unwrap();
        assert!(matches!(
            registered,
            WakeRegistrationOutcome::Registered { .. }
        ));
        let binding = second
            .reload_binding(WorkflowId(105))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(binding.contract_id, "contract-1");
        assert_eq!(binding.prepared_fingerprint, "fp-1");
    }

    #[tokio::test]
    async fn terminal_receipt_failpoint_rolls_back_canonical_and_projection() {
        let (_dir, repo, _) = open_repo_pair().await;
        let canonical = unwrap_started(register_and_begin(&repo, WorkflowId(106)).await);
        fail_after_canonical_receipt_once(WorkflowId(106));
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
        repo.register(&input, "fp-1", WorkflowId(107), Timestamp(10))
            .await
            .unwrap();
        fail_after_canonical_transition_once(WorkflowId(107));
        let err = repo.cancel(&cancel_input(WorkflowId(107))).await;
        assert!(err.is_err());
        assert!(repo.list_pending("conv-1").await.unwrap().is_empty());
        let head = repo
            .workflow_repo
            .fetch_workflow_head(WorkflowId(107))
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
        repo.register(&input, "fp-1", WorkflowId(108), Timestamp(10))
            .await
            .unwrap();
        let first = repo.cancel(&cancel_input(WorkflowId(108))).await.unwrap();
        let second = repo.cancel(&cancel_input(WorkflowId(108))).await.unwrap();
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
        }
    }

    #[tokio::test]
    async fn restart_reload_lists_pending_cancellation_projection() {
        let (_dir, first, second) = open_repo_pair().await;
        let input = intent();
        first
            .register(&input, "fp-1", WorkflowId(109), Timestamp(10))
            .await
            .unwrap();
        first.cancel(&cancel_input(WorkflowId(109))).await.unwrap();
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
        repo.register(&input, "fp-1", WorkflowId(110), Timestamp(10))
            .await
            .unwrap();
        let outcome = repo.cancel(&cancel_input(WorkflowId(110))).await.unwrap();
        let delivery = match outcome {
            WakeCancellationOutcome::Cancelled { delivery, .. }
            | WakeCancellationOutcome::Replayed { delivery, .. } => delivery,
            WakeCancellationOutcome::Stale => panic!("expected cancelled delivery"),
        };
        assert!(!delivery.canonical_delivery.requires_runtime_acceptance);
        assert_eq!(delivery.canonical_delivery.runtime_acceptance_status, None);
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
            | WakeTerminalEvidenceOutcome::EvidenceAfterObservation) => {
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
                Version(1),
                vec![DeliveryId(1)],
                TransitionId(2),
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
                Version(1),
                TransitionId(2),
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
                Version(2),
                vec![],
                TransitionId(3),
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
                Version(1),
                vec![DeliveryId(1)],
                TransitionId(2),
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
                Version(2),
                vec![DeliveryId(1)],
                TransitionId(2),
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
                Version(1),
                vec![],
                TransitionId(2),
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
        fail_after_transfer_binding_update_once(workflow_id);

        let err = restarted
            .transfer(&transfer_input(
                workflow_id,
                "conv-1",
                "conv-2",
                Version(1),
                vec![DeliveryId(1)],
                TransitionId(2),
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
            Version(1)
        );
        assert_eq!(restarted.list_pending("conv-1").await.unwrap().len(), 1);
        assert!(restarted.list_pending("conv-2").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn transfer_restart_reload_sees_new_owner() {
        let (_dir, repo, restarted) = open_repo_pair().await;
        insert_conversation(&repo.workflow_repo.pool, "conv-2").await;
        let workflow_id = WorkflowId(123);
        assert!(matches!(
            repo.register(&intent(), "fp-1", workflow_id, Timestamp(10))
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
                | WakeTerminalEvidenceOutcome::EvidenceAfterObservation) => {
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
                Version(1),
                TransitionId(2),
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
            Version(1),
            TransitionId(2),
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
        fail_after_resolve_transition_once(WorkflowId(113));
        let input = resolve_input(
            WorkflowId(113),
            Version(1),
            TransitionId(2),
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
        assert_eq!(head.version, Version(1));
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
                Version(1),
                TransitionId(2),
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
            Version(1),
            TransitionId(2),
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
                Version(1),
                TransitionId(2),
                vec![]
            ))
            .await
            .unwrap(),
            WakeResolvePendingOutcome::SetMismatch
        );
        assert_eq!(
            repo.resolve_pending_exact(&resolve_input(
                WorkflowId(116),
                Version(1),
                TransitionId(2),
                vec![DeliveryId(1), DeliveryId(999)]
            ))
            .await
            .unwrap(),
            WakeResolvePendingOutcome::SetMismatch
        );
        assert_eq!(
            repo.resolve_pending_exact(&resolve_input(
                WorkflowId(116),
                Version(1),
                TransitionId(2),
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
            Version(1),
            TransitionId(2),
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
                Version(2),
                TransitionId(3),
                vec![DeliveryId(1)]
            ))
            .await
            .unwrap(),
            WakeResolvePendingOutcome::AlreadyResolved
        );
        assert_eq!(
            repo.resolve_pending_exact(&resolve_input(
                WorkflowId(117),
                Version(2),
                TransitionId(3),
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
    async fn restart_reload_lists_pending_projection() {
        let (_dir, first, second) = open_repo_pair().await;
        let input = tmux_intent();
        first
            .register(&input, "fp-2", WorkflowId(110), Timestamp(10))
            .await
            .unwrap();
        let started = first
            .begin_observation(
                WorkflowId(110),
                AttemptId(1),
                ProcessIncarnation(1),
                Timestamp(20),
                phoenix_workflow::LeaseExpiry(30),
            )
            .await
            .unwrap();
        let canonical = unwrap_started(started);
        first
            .record_terminal_evidence(
                WorkflowId(110),
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
