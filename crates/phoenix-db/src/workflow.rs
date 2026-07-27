#![allow(
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::too_many_lines
)]

use crate::{DbError, DbResult};
use phoenix_workflow::{
    AttemptId, AttemptStatus, AuthorityOutcome, BarrierId, BarrierStatus, ClaimOutcome, CodecRef,
    CommitOutcome, DeliveryId, DeliveryStatus, EffectId, EffectRole, EffectStatus,
    ErasedAcceptanceProfile, ExecutionCapability, ExternalAcceptanceBinding,
    ExternalAcceptanceDisposition, ExternalAcceptanceOutcome, ExternalAcceptanceReceipt,
    Generation, LeaseExpiry, ManualChoiceKind, ManualResolutionId, ProcessIncarnation, ProfileRef,
    ReceiptFamily, ReceiptId, ReceiptOrigin, ResolutionStatus, RuntimeAcceptanceStatus, ScheduleId,
    ScheduleOccurrence, ScheduleOccurrenceId, SchedulePolicy, ScheduleStatus,
    SupportedCodecRegistry, SuppressionReason, Timestamp, TransitionId, Version, WorkflowBinding,
    WorkflowId, WorkflowStatus,
};
use sqlx::{error::DatabaseError, Row, SqlitePool};
use std::collections::BTreeSet;

pub mod direct_turn;
pub mod wake;

pub use direct_turn::*;
use phoenix_workflow::wake_profile;
use sqlx::{Sqlite, Transaction};

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalCodec {
    pub family: String,
    pub version: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalEffectDecl {
    pub effect_id: EffectId,
    pub declared_workflow_version: Version,
    pub family: String,
    pub kind: String,
    pub intent_codec: LocalCodec,
    pub intent_payload: Vec<u8>,
    pub generation: Generation,
    pub role: EffectRole,
    pub capability: ExecutionCapability,
    pub next_eligible_at: Option<Timestamp>,
    pub destructive_resource: Option<String>,
    pub status: EffectStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalDependencyDecl {
    pub effect_id: EffectId,
    pub depends_on_effect_id: EffectId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalBarrierDecl {
    pub barrier_id: BarrierId,
    pub status: BarrierStatus,
    pub reducer_event_codec: LocalCodec,
    pub reducer_event_payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalBarrierMemberDecl {
    pub barrier_id: BarrierId,
    pub effect_id: EffectId,
    pub receipt_family: ReceiptFamily,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalDeliveryDecl {
    pub delivery_id: DeliveryId,
    pub effect_id: Option<EffectId>,
    pub barrier_id: Option<BarrierId>,
    pub consumer_kind: String,
    pub event_codec: LocalCodec,
    pub payload_kind: LocalDeliveryPayloadKind,
    pub payload_blob: Vec<u8>,
    pub requires_runtime_acceptance: bool,
    pub runtime_acceptance_status: Option<RuntimeAcceptanceStatus>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalDeliveryPayloadKind {
    Receipt,
    Barrier,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalScheduleDecl {
    pub schedule_id: ScheduleId,
    pub policy: SchedulePolicy,
    pub key: String,
    pub status: ScheduleStatus,
    pub next_eligible_at: Timestamp,
    pub active_effect_id: Option<EffectId>,
    pub due_occurrence: Option<ScheduleOccurrence>,
    pub active_occurrence: Option<ScheduleOccurrence>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitTransitionPlanCas {
    pub workflow_id: WorkflowId,
    pub expected_version: Version,
    pub transition_id: TransitionId,
    pub generation: Generation,
    pub next_status: WorkflowStatus,
    pub event_codec: LocalCodec,
    pub event_payload: Vec<u8>,
    pub next_snapshot_codec: LocalCodec,
    pub next_snapshot_payload: Vec<u8>,
    pub committed_at: Timestamp,
    pub effects: Vec<LocalEffectDecl>,
    pub dependencies: Vec<LocalDependencyDecl>,
    pub barriers: Vec<LocalBarrierDecl>,
    pub barrier_members: Vec<LocalBarrierMemberDecl>,
    pub deliveries: Vec<LocalDeliveryDecl>,
    pub schedules: Vec<LocalScheduleDecl>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BeginAttemptInput {
    pub workflow_id: WorkflowId,
    pub effect_id: EffectId,
    pub attempt_id: AttemptId,
    pub process_incarnation: ProcessIncarnation,
    pub now: Timestamp,
    pub lease_until: Option<LeaseExpiry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BeginAttemptResult {
    pub outcome: ClaimOutcome,
    pub authority: Option<LocalAttemptAuthority>,
    pub attempt: Option<LocalAttemptRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalAttemptAuthority {
    pub workflow_id: WorkflowId,
    pub declared_workflow_version: Version,
    pub generation: Generation,
    pub effect_id: EffectId,
    pub attempt_id: AttemptId,
    pub process_incarnation: ProcessIncarnation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalReclaimableLease {
    pub attempt_id: AttemptId,
    pub lease_until: LeaseExpiry,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalAttemptRecord {
    pub id: AttemptId,
    pub ordinal: u32,
    pub authority: LocalAttemptAuthority,
    pub status: AttemptStatus,
    pub lease: Option<LocalReclaimableLease>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkflowSequenceName {
    Observation,
    Receipt,
    Delivery,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenewLeaseInput {
    pub authority: LocalAttemptAuthority,
    pub now: Timestamp,
    pub new_lease_until: LeaseExpiry,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpireLeaseInput {
    pub workflow_id: WorkflowId,
    pub effect_id: EffectId,
    pub attempt_id: AttemptId,
    pub now: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalObservationRecord {
    pub observation_id: u64,
    pub workflow_id: WorkflowId,
    pub effect_id: EffectId,
    pub attempt_id: AttemptId,
    pub declared_workflow_version: Version,
    pub generation: Generation,
    pub process_incarnation: ProcessIncarnation,
    pub observation_codec: LocalCodec,
    pub observation_payload: Vec<u8>,
    pub observed_at: Timestamp,
    pub recorded_at: Timestamp,
    pub authoritative: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordObservationInput {
    pub authority: LocalAttemptAuthority,
    pub observation_id: u64,
    pub now: Timestamp,
    pub observed_at: Timestamp,
    pub observation_codec: LocalCodec,
    pub observation_payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordObservationResult {
    pub outcome: AuthorityOutcome,
    pub observation: Option<LocalObservationRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptReceiptInput {
    pub authority: LocalAttemptAuthority,
    pub receipt_id: ReceiptId,
    pub delivery_id: DeliveryId,
    pub attempt_id: Option<AttemptId>,
    pub origin: ReceiptOrigin,
    pub receipt_codec: LocalCodec,
    pub receipt_payload: Vec<u8>,
    pub receipt_event_codec: LocalCodec,
    pub receipt_event_payload: Vec<u8>,
    pub receipt_event_requires_runtime_acceptance: bool,
    pub request_runtime_acceptance_for_cancellation: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalReceiptRecord {
    pub receipt_id: ReceiptId,
    pub workflow_id: WorkflowId,
    pub effect_id: EffectId,
    pub generation: Generation,
    pub declared_workflow_version: Version,
    pub process_incarnation: ProcessIncarnation,
    pub attempt_id: Option<AttemptId>,
    pub origin: ReceiptOrigin,
    pub receipt_codec: LocalCodec,
    pub receipt_payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiptAcceptanceResult {
    pub outcome: AuthorityOutcome,
    pub receipt: Option<LocalReceiptRecord>,
    pub delivery: Option<LocalDeliveryRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalDeliveryRecord {
    pub delivery_id: DeliveryId,
    pub workflow_id: WorkflowId,
    pub effect_id: Option<EffectId>,
    pub barrier_id: Option<BarrierId>,
    pub consumer_kind: String,
    pub event_codec: LocalCodec,
    pub payload_kind: LocalDeliveryPayloadKind,
    pub payload_blob: Vec<u8>,
    pub requires_runtime_acceptance: bool,
    pub status: DeliveryStatus,
    pub runtime_acceptance_status: Option<RuntimeAcceptanceStatus>,
    pub suppression_reason: Option<SuppressionReason>,
    pub accepted_by_transition_id: Option<TransitionId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptOrSuppressDeliveryInput {
    pub workflow_id: WorkflowId,
    pub expected_version: Version,
    pub transition_id: TransitionId,
    pub generation: Generation,
    pub next_status: WorkflowStatus,
    pub event_codec: LocalCodec,
    pub event_payload: Vec<u8>,
    pub next_snapshot_codec: LocalCodec,
    pub next_snapshot_payload: Vec<u8>,
    pub committed_at: Timestamp,
    pub accept_delivery_ids: Vec<DeliveryId>,
    pub suppress_delivery_ids: Vec<DeliveryId>,
    pub suppression_reason: SuppressionReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryResolutionDecision {
    Accept,
    Suppress { reason: SuppressionReason },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryResolutionPlan<'a> {
    pub workflow_id: WorkflowId,
    pub expected_version: Version,
    pub transition_id: TransitionId,
    pub generation: Generation,
    pub next_status: WorkflowStatus,
    pub event_codec: &'a LocalCodec,
    pub event_payload: &'a [u8],
    pub next_snapshot_codec: &'a LocalCodec,
    pub next_snapshot_payload: &'a [u8],
    pub committed_at: Timestamp,
    pub exact_delivery_ids: &'a [DeliveryId],
    pub decision: DeliveryResolutionDecision,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManualResolutionRecordRow {
    pub manual_resolution_id: ManualResolutionId,
    pub workflow_id: WorkflowId,
    pub workflow_version: Version,
    pub effect_id: EffectId,
    pub status: ResolutionStatus,
    pub accepted_choice_ordinal: Option<u32>,
    pub resolved_by: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManualResolutionChoiceRow {
    pub manual_resolution_id: ManualResolutionId,
    pub ordinal: u32,
    pub kind: ManualChoiceKind,
    pub payload_codec: LocalCodec,
    pub payload_blob: Vec<u8>,
    pub receipt_codec: LocalCodec,
    pub receipt_blob: Vec<u8>,
    pub receipt_event_codec: LocalCodec,
    pub receipt_event_blob: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolveManualInput {
    pub workflow_id: WorkflowId,
    pub expected_version: Version,
    pub transition_id: TransitionId,
    pub generation: Generation,
    pub manual_resolution_id: ManualResolutionId,
    pub choice_ordinal: u32,
    pub resolved_by: String,
    pub next_status: WorkflowStatus,
    pub transition_codec: LocalCodec,
    pub transition_payload: Vec<u8>,
    pub next_snapshot_codec: LocalCodec,
    pub next_snapshot_payload: Vec<u8>,
    pub committed_at: Timestamp,
    pub retry_at: Option<Timestamp>,
    pub manual_receipt_id: Option<ReceiptId>,
    pub manual_delivery_id: Option<DeliveryId>,
    pub manual_receipt_requires_runtime_acceptance: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconcileScheduleDueInput {
    pub workflow_id: WorkflowId,
    pub schedule_id: ScheduleId,
    pub now: Timestamp,
    pub new_occurrence_id: ScheduleOccurrenceId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartScheduleOccurrenceInput {
    pub workflow_id: WorkflowId,
    pub occurrence: ScheduleOccurrence,
    pub active_effect_id: Option<EffectId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompleteScheduleOccurrenceInput {
    pub workflow_id: WorkflowId,
    pub occurrence: ScheduleOccurrence,
    pub next_eligible_at: Timestamp,
}

#[derive(Debug, Clone)]
pub struct WorkflowRepository {
    pool: SqlitePool,
}

impl WorkflowRepository {
    pub(crate) async fn begin_tx(&self) -> DbResult<WorkflowTx<'_>> {
        Ok(WorkflowTx::new(self.pool.begin().await?))
    }

    pub(crate) async fn begin_immediate_tx(&self) -> DbResult<WorkflowTx<'_>> {
        Ok(WorkflowTx::new(
            self.pool.begin_with("BEGIN IMMEDIATE").await?,
        ))
    }
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

type SqliteTx<'a> = Transaction<'a, Sqlite>;

pub(crate) struct WorkflowTx<'a> {
    pub(crate) tx: SqliteTx<'a>,
}

impl<'a> WorkflowTx<'a> {
    fn new(tx: SqliteTx<'a>) -> Self {
        Self { tx }
    }

    async fn commit(self) -> DbResult<()> {
        self.tx.commit().await?;
        Ok(())
    }

    async fn rollback(self) -> DbResult<()> {
        self.tx.rollback().await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn commit_transition_head_cas(
        &mut self,
        workflow_id: WorkflowId,
        expected_version: Version,
        generation: Generation,
        next_status: WorkflowStatus,
        event_codec: &LocalCodec,
        event_payload: &[u8],
        next_snapshot_codec: &LocalCodec,
        next_snapshot_payload: &[u8],
        transition_id: TransitionId,
        committed_at: Timestamp,
    ) -> DbResult<bool> {
        let updated = sqlx::query("UPDATE workflows SET version = version + 1, generation = ?3, status = ?4, snapshot_codec_family = ?5, snapshot_codec_version = ?6, snapshot_payload = ?7, updated_at = ?8 WHERE workflow_id = ?1 AND version = ?2")
            .bind(to_i64(workflow_id.0, "workflow_id")?)
            .bind(to_i64(expected_version.0, "expected_version")?)
            .bind(to_i64(generation.0, "generation")?)
            .bind(workflow_status_to_str(next_status))
            .bind(&next_snapshot_codec.family)
            .bind(i64::from(next_snapshot_codec.version))
            .bind(next_snapshot_payload)
            .bind(to_i64(committed_at.0, "committed_at")?)
            .execute(&mut *self.tx).await?.rows_affected();
        if updated == 0 {
            return Ok(false);
        }
        sqlx::query("INSERT INTO workflow_transitions (workflow_id, transition_id, from_version, to_version, generation, event_codec_family, event_codec_version, event_payload, committed_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)")
            .bind(to_i64(workflow_id.0, "workflow_id")?)
            .bind(to_i64(transition_id.0, "transition_id")?)
            .bind(to_i64(expected_version.0, "expected_version")?)
            .bind(to_i64(expected_version.next().0, "to_version")?)
            .bind(to_i64(generation.0, "generation")?)
            .bind(&event_codec.family)
            .bind(i64::from(event_codec.version))
            .bind(event_payload)
            .bind(to_i64(committed_at.0, "committed_at")?)
            .execute(&mut *self.tx).await?;
        Ok(true)
    }

    pub(crate) async fn resolve_deliveries_exact(
        &mut self,
        plan: DeliveryResolutionPlan<'_>,
    ) -> DbResult<CommitOutcome> {
        let mut exact = BTreeSet::new();
        for &delivery_id in plan.exact_delivery_ids {
            if !exact.insert(delivery_id) {
                return Ok(CommitOutcome::InvalidPlan);
            }
        }
        let committed = self
            .commit_transition_head_cas(
                plan.workflow_id,
                plan.expected_version,
                plan.generation,
                plan.next_status,
                plan.event_codec,
                plan.event_payload,
                plan.next_snapshot_codec,
                plan.next_snapshot_payload,
                plan.transition_id,
                plan.committed_at,
            )
            .await?;
        if !committed {
            return Ok(CommitOutcome::VersionConflict);
        }
        let rows = sqlx::query(
            "SELECT delivery_id, status FROM workflow_deliveries WHERE workflow_id = ?1 ORDER BY delivery_id",
        )
        .bind(to_i64(plan.workflow_id.0, "workflow_id")?)
        .fetch_all(&mut *self.tx)
        .await?;
        let matched: Vec<_> = rows
            .into_iter()
            .filter(|row| {
                exact.contains(&DeliveryId(
                    to_u64(row.get::<i64, _>("delivery_id"), "delivery_id").unwrap(),
                ))
            })
            .collect();
        if matched.len() != exact.len()
            || matched
                .iter()
                .any(|r| r.get::<String, _>("status") != "Pending")
        {
            return Ok(CommitOutcome::InvalidPlan);
        }
        let (status, runtime_acceptance_status, suppression_reason) = match plan.decision {
            DeliveryResolutionDecision::Accept => ("Accepted", "Accepted", None),
            DeliveryResolutionDecision::Suppress { reason } => (
                "Suppressed",
                "Suppressed",
                Some(suppression_reason_to_str(reason)),
            ),
        };
        for &delivery_id in plan.exact_delivery_ids {
            sqlx::query("UPDATE workflow_deliveries SET status = ?3, suppression_reason = ?4, accepted_by_transition_id = ?5, runtime_acceptance_status = CASE WHEN requires_runtime_acceptance = 1 THEN ?6 ELSE runtime_acceptance_status END WHERE workflow_id = ?1 AND delivery_id = ?2")
                .bind(to_i64(plan.workflow_id.0, "workflow_id")?)
                .bind(to_i64(delivery_id.0, "delivery_id")?)
                .bind(status)
                .bind(suppression_reason)
                .bind(match plan.decision {
                    DeliveryResolutionDecision::Accept => Some(to_i64(plan.transition_id.0, "transition_id")?),
                    DeliveryResolutionDecision::Suppress { .. } => None,
                })
                .bind(runtime_acceptance_status)
                .execute(&mut *self.tx).await?;
        }
        Ok(CommitOutcome::Committed)
    }

    pub(crate) async fn fetch_workflow_head(
        &mut self,
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
        .fetch_optional(&mut *self.tx)
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
        .fetch_all(&mut *self.tx)
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
            profile_kind,
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

    pub(crate) async fn allocate_sequence_value(
        &mut self,
        workflow_id: WorkflowId,
        sequence: WorkflowSequenceName,
    ) -> DbResult<u64> {
        let name = workflow_sequence_name_str(sequence);
        sqlx::query(
            "INSERT INTO workflow_sequences (workflow_id, sequence_name, next_value)
             VALUES (?1, ?2, 2)
             ON CONFLICT(workflow_id, sequence_name)
             DO UPDATE SET next_value = workflow_sequences.next_value + 1",
        )
        .bind(to_i64(workflow_id.0, "workflow_id")?)
        .bind(name)
        .execute(&mut *self.tx)
        .await?;
        let allocated = sqlx::query_scalar::<_, i64>(
            "SELECT next_value - 1 FROM workflow_sequences WHERE workflow_id = ?1 AND sequence_name = ?2",
        )
        .bind(to_i64(workflow_id.0, "workflow_id")?)
        .bind(name)
        .fetch_one(&mut *self.tx)
        .await?;
        to_u64(allocated, "allocated_sequence")
    }

    pub(crate) async fn begin_attempt(
        &mut self,
        input: &BeginAttemptInput,
    ) -> DbResult<BeginAttemptResult> {
        let effect = sqlx::query(
            "SELECT e.declared_workflow_version, e.generation, e.capability_kind,
                    e.stable_command_id, e.status
             FROM workflow_effects e
             JOIN workflows w ON w.workflow_id = e.workflow_id
             WHERE e.workflow_id = ?1 AND e.effect_id = ?2
               AND w.status IN ('Active', 'Cancelling')",
        )
        .bind(to_i64(input.workflow_id.0, "workflow_id")?)
        .bind(to_i64(input.effect_id.0, "effect_id")?)
        .fetch_optional(&mut *self.tx)
        .await?;
        let Some(effect) = effect else {
            return Ok(BeginAttemptResult {
                outcome: ClaimOutcome::Ineligible,
                authority: None,
                attempt: None,
            });
        };
        let effect_status = parse_effect_status(&effect.get::<String, _>("status"))?;
        if effect_status != EffectStatus::Eligible {
            return Ok(BeginAttemptResult {
                outcome: if effect_status == EffectStatus::Executing {
                    ClaimOutcome::AuthorityConflict
                } else {
                    ClaimOutcome::Ineligible
                },
                authority: None,
                attempt: None,
            });
        }
        let capability = parse_capability(
            &effect.get::<String, _>("capability_kind"),
            effect.get::<Option<i64>, _>("stable_command_id"),
        )?;
        match (&capability, input.lease_until) {
            (ExecutionCapability::ReclaimableObservation, Some(lease_until))
                if lease_until.is_live_at(input.now) => {}
            (ExecutionCapability::ReclaimableObservation, _) | (_, Some(_)) => {
                return Ok(BeginAttemptResult {
                    outcome: ClaimOutcome::AuthorityConflict,
                    authority: None,
                    attempt: None,
                });
            }
            _ => {}
        }
        let claimed = sqlx::query(
            "UPDATE workflow_effects
             SET status = 'Executing'
             WHERE workflow_id = ?1 AND effect_id = ?2 AND status = 'Eligible'
               AND EXISTS (
                   SELECT 1 FROM workflows w
                   WHERE w.workflow_id = workflow_effects.workflow_id
                     AND w.status IN ('Active', 'Cancelling')
               )",
        )
        .bind(to_i64(input.workflow_id.0, "workflow_id")?)
        .bind(to_i64(input.effect_id.0, "effect_id")?)
        .execute(&mut *self.tx)
        .await?
        .rows_affected();
        if claimed == 0 {
            return Ok(BeginAttemptResult {
                outcome: ClaimOutcome::AuthorityConflict,
                authority: None,
                attempt: None,
            });
        }
        let ordinal = to_u32(
            sqlx::query_scalar::<_, i64>(
                "SELECT COALESCE(MAX(ordinal), -1) + 1 FROM workflow_attempts WHERE workflow_id = ?1 AND effect_id = ?2",
            )
            .bind(to_i64(input.workflow_id.0, "workflow_id")?)
            .bind(to_i64(input.effect_id.0, "effect_id")?)
            .fetch_one(&mut *self.tx)
            .await?,
            "ordinal",
        )?;
        let declared_workflow_version = Version(to_u64(
            effect.get::<i64, _>("declared_workflow_version"),
            "declared_workflow_version",
        )?);
        let generation = Generation(to_u64(effect.get::<i64, _>("generation"), "generation")?);
        let insert_attempt = sqlx::query(
            "INSERT INTO workflow_attempts (
                workflow_id, effect_id, attempt_id, ordinal, declared_workflow_version,
                generation, process_incarnation, status, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'Begun', ?8)",
        )
        .bind(to_i64(input.workflow_id.0, "workflow_id")?)
        .bind(to_i64(input.effect_id.0, "effect_id")?)
        .bind(to_i64(input.attempt_id.0, "attempt_id")?)
        .bind(i64::from(ordinal))
        .bind(to_i64(
            declared_workflow_version.0,
            "declared_workflow_version",
        )?)
        .bind(to_i64(generation.0, "generation")?)
        .bind(to_i64(input.process_incarnation.0, "process_incarnation")?)
        .bind(to_i64(input.now.0, "created_at")?)
        .execute(&mut *self.tx)
        .await;
        match insert_attempt {
            Ok(_) => {}
            Err(sqlx::Error::Database(error))
                if is_sqlite_busy_retryable(error.as_ref())
                    || is_sqlite_unique_constraint(error.as_ref())
                    || is_sqlite_primary_key_constraint(error.as_ref()) =>
            {
                return Ok(BeginAttemptResult {
                    outcome: ClaimOutcome::AuthorityConflict,
                    authority: None,
                    attempt: None,
                });
            }
            Err(error) => return Err(DbError::Sqlx(error)),
        }
        if let Some(lease_until) = input.lease_until {
            sqlx::query("INSERT INTO workflow_reclaimable_leases (workflow_id, attempt_id, lease_until) VALUES (?1, ?2, ?3)")
                .bind(to_i64(input.workflow_id.0, "workflow_id")?)
                .bind(to_i64(input.attempt_id.0, "attempt_id")?)
                .bind(to_i64(lease_until.0, "lease_until")?)
                .execute(&mut *self.tx)
                .await?;
        }
        let authority = LocalAttemptAuthority {
            workflow_id: input.workflow_id,
            declared_workflow_version,
            generation,
            effect_id: input.effect_id,
            attempt_id: input.attempt_id,
            process_incarnation: input.process_incarnation,
        };
        let attempt = LocalAttemptRecord {
            id: input.attempt_id,
            ordinal,
            authority: authority.clone(),
            status: AttemptStatus::Begun,
            lease: input.lease_until.map(|lease_until| LocalReclaimableLease {
                attempt_id: input.attempt_id,
                lease_until,
            }),
        };
        Ok(BeginAttemptResult {
            outcome: ClaimOutcome::Started,
            authority: Some(authority),
            attempt: Some(attempt),
        })
    }

    pub(crate) async fn record_observation(
        &mut self,
        input: &RecordObservationInput,
    ) -> DbResult<RecordObservationResult> {
        let row = sqlx::query("SELECT e.declared_workflow_version, e.generation, a.process_incarnation, a.status, l.lease_until FROM workflow_effects e JOIN workflow_attempts a ON a.workflow_id = e.workflow_id AND a.effect_id = e.effect_id LEFT JOIN workflow_reclaimable_leases l ON l.workflow_id = a.workflow_id AND l.attempt_id = a.attempt_id WHERE e.workflow_id = ?1 AND e.effect_id = ?2 AND a.attempt_id = ?3")
            .bind(to_i64(input.authority.workflow_id.0, "workflow_id")?)
            .bind(to_i64(input.authority.effect_id.0, "effect_id")?)
            .bind(to_i64(input.authority.attempt_id.0, "attempt_id")?)
            .fetch_optional(&mut *self.tx).await?;
        let authoritative = row.as_ref().is_some_and(|row| {
            row.get::<i64, _>("declared_workflow_version")
                == i64::try_from(input.authority.declared_workflow_version.0).unwrap()
                && row.get::<i64, _>("generation")
                    == i64::try_from(input.authority.generation.0).unwrap()
                && row.get::<i64, _>("process_incarnation")
                    == i64::try_from(input.authority.process_incarnation.0).unwrap()
                && matches!(
                    row.get::<String, _>("status").as_str(),
                    "Begun" | "ObservationRecorded"
                )
                && row
                    .get::<Option<i64>, _>("lease_until")
                    .is_none_or(|lease| lease > i64::try_from(input.now.0).unwrap())
        });
        let insert_sql = if authoritative {
            "INSERT INTO workflow_authoritative_observations (workflow_id, observation_id, effect_id, attempt_id, declared_workflow_version, generation, process_incarnation, observation_codec_family, observation_codec_version, observation_payload, observed_at, recorded_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)"
        } else {
            "INSERT INTO workflow_stale_observations (workflow_id, observation_id, effect_id, attempt_id, declared_workflow_version, generation, process_incarnation, observation_codec_family, observation_codec_version, observation_payload, observed_at, recorded_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)"
        };
        sqlx::query(insert_sql)
            .bind(to_i64(input.authority.workflow_id.0, "workflow_id")?)
            .bind(to_i64(input.observation_id, "observation_id")?)
            .bind(to_i64(input.authority.effect_id.0, "effect_id")?)
            .bind(to_i64(input.authority.attempt_id.0, "attempt_id")?)
            .bind(to_i64(
                input.authority.declared_workflow_version.0,
                "declared_workflow_version",
            )?)
            .bind(to_i64(input.authority.generation.0, "generation")?)
            .bind(to_i64(
                input.authority.process_incarnation.0,
                "process_incarnation",
            )?)
            .bind(&input.observation_codec.family)
            .bind(i64::from(input.observation_codec.version))
            .bind(&input.observation_payload)
            .bind(to_i64(input.observed_at.0, "observed_at")?)
            .bind(to_i64(input.now.0, "recorded_at")?)
            .execute(&mut *self.tx)
            .await?;
        if authoritative {
            sqlx::query("UPDATE workflow_attempts SET status = 'ObservationRecorded' WHERE workflow_id = ?1 AND attempt_id = ?2")
                .bind(to_i64(input.authority.workflow_id.0, "workflow_id")?)
                .bind(to_i64(input.authority.attempt_id.0, "attempt_id")?)
                .execute(&mut *self.tx).await?;
        }
        Ok(RecordObservationResult {
            outcome: if authoritative {
                AuthorityOutcome::Authorized
            } else {
                AuthorityOutcome::StaleAuthority
            },
            observation: Some(LocalObservationRecord {
                observation_id: input.observation_id,
                workflow_id: input.authority.workflow_id,
                effect_id: input.authority.effect_id,
                attempt_id: input.authority.attempt_id,
                declared_workflow_version: input.authority.declared_workflow_version,
                generation: input.authority.generation,
                process_incarnation: input.authority.process_incarnation,
                observation_codec: input.observation_codec.clone(),
                observation_payload: input.observation_payload.clone(),
                observed_at: input.observed_at,
                recorded_at: input.now,
                authoritative,
            }),
        })
    }

    pub(crate) async fn accept_receipt_and_delivery(
        &mut self,
        input: &AcceptReceiptInput,
    ) -> DbResult<ReceiptAcceptanceResult> {
        let effect = sqlx::query(
            "SELECT e.status, e.generation, e.declared_workflow_version,
                    EXISTS (
                        SELECT 1 FROM workflow_attempts a
                        WHERE a.workflow_id = e.workflow_id
                          AND a.effect_id = e.effect_id
                          AND a.attempt_id = ?3
                          AND a.declared_workflow_version = ?4
                          AND a.generation = ?5
                          AND a.process_incarnation = ?6
                          AND a.status IN ('Begun', 'ObservationRecorded')
                    ) AS owns_effect,
                    EXISTS (
                        SELECT 1 FROM workflow_supported_codecs c
                        WHERE c.workflow_id = e.workflow_id
                          AND c.codec_family = ?7 AND c.codec_version = ?8
                    ) AS supports_receipt_codec,
                    EXISTS (
                        SELECT 1 FROM workflow_supported_codecs c
                        WHERE c.workflow_id = e.workflow_id
                          AND c.codec_family = ?9 AND c.codec_version = ?10
                    ) AS supports_receipt_event_codec
             FROM workflow_effects e
             WHERE e.workflow_id = ?1 AND e.effect_id = ?2",
        )
        .bind(to_i64(input.authority.workflow_id.0, "workflow_id")?)
        .bind(to_i64(input.authority.effect_id.0, "effect_id")?)
        .bind(to_i64(input.authority.attempt_id.0, "attempt_id")?)
        .bind(to_i64(
            input.authority.declared_workflow_version.0,
            "declared_workflow_version",
        )?)
        .bind(to_i64(input.authority.generation.0, "generation")?)
        .bind(to_i64(
            input.authority.process_incarnation.0,
            "process_incarnation",
        )?)
        .bind(&input.receipt_codec.family)
        .bind(i64::from(input.receipt_codec.version))
        .bind(&input.receipt_event_codec.family)
        .bind(i64::from(input.receipt_event_codec.version))
        .fetch_optional(&mut *self.tx)
        .await?;
        let Some(effect) = effect else {
            return Ok(ReceiptAcceptanceResult {
                outcome: AuthorityOutcome::StaleAuthority,
                receipt: None,
                delivery: None,
            });
        };
        if effect.get::<String, _>("status") != "Executing"
            || input.attempt_id != Some(input.authority.attempt_id)
            || effect.get::<i64, _>("owns_effect") == 0
            || effect.get::<i64, _>("supports_receipt_codec") == 0
            || effect.get::<i64, _>("supports_receipt_event_codec") == 0
        {
            return Ok(ReceiptAcceptanceResult {
                outcome: AuthorityOutcome::StaleAuthority,
                receipt: None,
                delivery: None,
            });
        }
        let mut requires_runtime = input.receipt_event_requires_runtime_acceptance;
        if input.origin == ReceiptOrigin::CancellationArbitration
            && !input.request_runtime_acceptance_for_cancellation
        {
            requires_runtime = false;
        }
        let effect_updated = sqlx::query("UPDATE workflow_effects SET status = 'Receipted', pending_reconciliation = 0 WHERE workflow_id = ?1 AND effect_id = ?2 AND status = 'Executing'")
            .bind(to_i64(input.authority.workflow_id.0, "workflow_id")?)
            .bind(to_i64(input.authority.effect_id.0, "effect_id")?)
            .execute(&mut *self.tx).await?.rows_affected();
        if effect_updated == 0 {
            return Ok(ReceiptAcceptanceResult {
                outcome: AuthorityOutcome::StaleAuthority,
                receipt: None,
                delivery: None,
            });
        }
        let receipt_insert = sqlx::query("INSERT INTO workflow_receipts (workflow_id, receipt_id, effect_id, generation, declared_workflow_version, process_incarnation, attempt_id, origin, receipt_codec_family, receipt_codec_version, receipt_payload) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)")
            .bind(to_i64(input.authority.workflow_id.0, "workflow_id")?)
            .bind(to_i64(input.receipt_id.0, "receipt_id")?)
            .bind(to_i64(input.authority.effect_id.0, "effect_id")?)
            .bind(to_i64(input.authority.generation.0, "generation")?)
            .bind(to_i64(input.authority.declared_workflow_version.0, "declared_workflow_version")?)
            .bind(to_i64(input.authority.process_incarnation.0, "process_incarnation")?)
            .bind(input.attempt_id.and_then(|id| i64::try_from(id.0).ok()))
            .bind(receipt_origin_to_str(input.origin))
            .bind(&input.receipt_codec.family)
            .bind(i64::from(input.receipt_codec.version))
            .bind(&input.receipt_payload)
            .execute(&mut *self.tx).await;
        match receipt_insert {
            Ok(_) => {}
            Err(sqlx::Error::Database(error))
                if is_sqlite_busy_retryable(error.as_ref())
                    || is_sqlite_unique_constraint(error.as_ref())
                    || is_sqlite_primary_key_constraint(error.as_ref()) =>
            {
                return Ok(ReceiptAcceptanceResult {
                    outcome: AuthorityOutcome::StaleAuthority,
                    receipt: None,
                    delivery: None,
                });
            }
            Err(error) => return Err(DbError::Sqlx(error)),
        }
        sqlx::query("INSERT INTO workflow_deliveries (workflow_id, delivery_id, effect_id, barrier_id, consumer_kind, event_codec_family, event_codec_version, payload_kind, payload_blob, requires_runtime_acceptance, status, runtime_acceptance_status, suppression_reason, accepted_by_transition_id) VALUES (?1, ?2, ?3, NULL, 'reducer', ?4, ?5, 'Receipt', ?6, ?7, 'Pending', ?8, NULL, NULL)")
            .bind(to_i64(input.authority.workflow_id.0, "workflow_id")?)
            .bind(to_i64(input.delivery_id.0, "delivery_id")?)
            .bind(to_i64(input.authority.effect_id.0, "effect_id")?)
            .bind(&input.receipt_event_codec.family)
            .bind(i64::from(input.receipt_event_codec.version))
            .bind(&input.receipt_event_payload)
            .bind(requires_runtime)
            .bind(if requires_runtime { Some("Owed") } else { None })
            .execute(&mut *self.tx).await?;
        sqlx::query("UPDATE workflow_attempts SET status = 'ReceiptAccepted' WHERE workflow_id = ?1 AND attempt_id = ?2")
            .bind(to_i64(input.authority.workflow_id.0, "workflow_id")?)
            .bind(to_i64(input.authority.attempt_id.0, "attempt_id")?)
            .execute(&mut *self.tx).await?;
        sqlx::query(
            "DELETE FROM workflow_reclaimable_leases WHERE workflow_id = ?1 AND attempt_id = ?2",
        )
        .bind(to_i64(input.authority.workflow_id.0, "workflow_id")?)
        .bind(to_i64(input.authority.attempt_id.0, "attempt_id")?)
        .execute(&mut *self.tx)
        .await?;
        Ok(ReceiptAcceptanceResult {
            outcome: AuthorityOutcome::Authorized,
            receipt: Some(LocalReceiptRecord {
                receipt_id: input.receipt_id,
                workflow_id: input.authority.workflow_id,
                effect_id: input.authority.effect_id,
                generation: input.authority.generation,
                declared_workflow_version: input.authority.declared_workflow_version,
                process_incarnation: input.authority.process_incarnation,
                attempt_id: input.attempt_id,
                origin: input.origin,
                receipt_codec: input.receipt_codec.clone(),
                receipt_payload: input.receipt_payload.clone(),
            }),
            delivery: Some(LocalDeliveryRecord {
                delivery_id: input.delivery_id,
                workflow_id: input.authority.workflow_id,
                effect_id: Some(input.authority.effect_id),
                barrier_id: None,
                consumer_kind: "reducer".to_string(),
                event_codec: input.receipt_event_codec.clone(),
                payload_kind: LocalDeliveryPayloadKind::Receipt,
                payload_blob: input.receipt_event_payload.clone(),
                requires_runtime_acceptance: requires_runtime,
                status: DeliveryStatus::Pending,
                runtime_acceptance_status: requires_runtime
                    .then_some(RuntimeAcceptanceStatus::Owed),
                suppression_reason: None,
                accepted_by_transition_id: None,
            }),
        })
    }

    pub(crate) async fn fetch_external_acceptance_binding(
        &mut self,
        input: &CreateWorkflowWithExternalAcceptance,
    ) -> DbResult<Option<ExternalAcceptanceBinding<Vec<u8>>>> {
        let existing = sqlx::query(
            "SELECT workflow_id, intent_fingerprint, receipt_handle, disposition_handle
             FROM workflow_external_acceptance_bindings
             WHERE profile_kind = ?1 AND profile_version = ?2 AND target_scope = ?3 AND idempotency_key = ?4",
        )
        .bind(&input.profile.profile_kind)
        .bind(i64::from(input.profile.profile_version))
        .bind(input.target_scope.as_str())
        .bind(input.idempotency_key.as_str())
        .fetch_optional(&mut *self.tx)
        .await?;
        existing
            .map(|row| replay_binding_from_row(input, &row))
            .transpose()
    }

    pub(crate) async fn insert_workflow(
        &mut self,
        input: &CreateWorkflowWithExternalAcceptance,
    ) -> DbResult<()> {
        insert_workflow_tx(&mut self.tx, input).await
    }

    pub(crate) async fn insert_external_acceptance_binding(
        &mut self,
        input: &CreateWorkflowWithExternalAcceptance,
    ) -> DbResult<()> {
        insert_external_acceptance_binding_tx(&mut self.tx, input).await
    }

    pub(crate) async fn invalidate_nonterminal_effects(
        &mut self,
        workflow_id: WorkflowId,
    ) -> DbResult<()> {
        sqlx::query(
            "UPDATE workflow_effects
             SET status = 'Invalidated', next_eligible_at = NULL,
                 pending_reconciliation = 0
             WHERE workflow_id = ?1
               AND status IN ('Blocked', 'Eligible', 'Executing', 'RetryWait', 'AmbiguityWait')",
        )
        .bind(to_i64(workflow_id.0, "workflow_id")?)
        .execute(&mut *self.tx)
        .await?;
        Ok(())
    }

    pub(crate) async fn commit_transition_plan(
        &mut self,
        input: &CommitTransitionPlanCas,
    ) -> DbResult<CommitOutcome> {
        WorkflowRepository::commit_transition_plan_tx(&mut self.tx, input).await
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
    .bind(&input.profile.profile_kind)
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
    .bind(&input.profile.profile_kind)
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

const SQLITE_CONSTRAINT_UNIQUE: &str = "2067";
const SQLITE_CONSTRAINT_PRIMARYKEY: &str = "1555";
const SQLITE_BUSY: &str = "5";
const DIRECT_TURN_PROFILE_KIND: &str = "direct_turn";

fn sqlite_error_code_is(error: &dyn DatabaseError, expected: &str) -> bool {
    error.code().as_deref() == Some(expected)
}

fn is_sqlite_unique_constraint(error: &dyn DatabaseError) -> bool {
    sqlite_error_code_is(error, SQLITE_CONSTRAINT_UNIQUE)
}

fn is_sqlite_primary_key_constraint(error: &dyn DatabaseError) -> bool {
    sqlite_error_code_is(error, SQLITE_CONSTRAINT_PRIMARYKEY)
}

fn is_sqlite_busy_retryable(error: &dyn DatabaseError) -> bool {
    sqlite_error_code_is(error, SQLITE_BUSY) || error.code().as_deref() == Some("517")
}

async fn workflow_profile_kind(
    pool: &SqlitePool,
    workflow_id: WorkflowId,
) -> DbResult<Option<String>> {
    sqlx::query_scalar::<_, String>("SELECT profile_kind FROM workflows WHERE workflow_id = ?1")
        .bind(to_i64(workflow_id.0, "workflow_id")?)
        .fetch_optional(pool)
        .await
        .map_err(Into::into)
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

    /// # Errors
    ///
    /// Returns an error when the transaction cannot read or persist the workflow foundation rows.
    pub async fn create_workflow_with_external_acceptance(
        &self,
        input: &CreateWorkflowWithExternalAcceptance,
    ) -> DbResult<ExternalAcceptanceOutcome<Vec<u8>>> {
        if input.profile.profile_kind == DIRECT_TURN_PROFILE_KIND {
            return Err(DbError::Serialization(
                "direct-turn workflows require accept_authoritative_turn".to_string(),
            ));
        }
        let mut tx = self.begin_tx().await?;
        if let Some(replay) = tx.fetch_external_acceptance_binding(input).await? {
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

        tx.insert_workflow(input).await?;
        tx.insert_external_acceptance_binding(input).await?;
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
        if workflow_profile_kind(&self.pool, input.workflow_id)
            .await?
            .as_deref()
            == Some(DIRECT_TURN_PROFILE_KIND)
        {
            return Ok(CommitOutcome::InvalidPlan);
        }
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

    async fn commit_transition_plan_tx(
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        input: &CommitTransitionPlanCas,
    ) -> DbResult<CommitOutcome> {
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
        .bind(&input.next_snapshot_codec.family)
        .bind(i64::from(input.next_snapshot_codec.version))
        .bind(&input.next_snapshot_payload)
        .bind(to_i64(input.committed_at.0, "committed_at")?)
        .execute(&mut **tx)
        .await?
        .rows_affected();
        if updated == 0 {
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
        .bind(to_i64(input.expected_version.next().0, "to_version")?)
        .bind(to_i64(input.generation.0, "generation")?)
        .bind(&input.event_codec.family)
        .bind(i64::from(input.event_codec.version))
        .bind(&input.event_payload)
        .bind(to_i64(input.committed_at.0, "committed_at")?)
        .execute(&mut **tx)
        .await?;
        for effect in &input.effects {
            let (capability_kind, stable_command_id) = capability_to_parts(&effect.capability);
            sqlx::query(
                "INSERT INTO workflow_effects (
                workflow_id, effect_id, declared_workflow_version, family, kind,
                intent_codec_family, intent_codec_version, intent_payload, generation,
                role, capability_kind, stable_command_id, next_eligible_at,
                destructive_resource, status, pending_reconciliation
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, 0)",
            )
            .bind(to_i64(input.workflow_id.0, "workflow_id")?)
            .bind(to_i64(effect.effect_id.0, "effect_id")?)
            .bind(to_i64(
                effect.declared_workflow_version.0,
                "declared_workflow_version",
            )?)
            .bind(&effect.family)
            .bind(&effect.kind)
            .bind(&effect.intent_codec.family)
            .bind(i64::from(effect.intent_codec.version))
            .bind(&effect.intent_payload)
            .bind(to_i64(effect.generation.0, "generation")?)
            .bind(effect_role_to_str(effect.role))
            .bind(capability_kind)
            .bind(stable_command_id.and_then(|value| i64::try_from(value).ok()))
            .bind(
                effect
                    .next_eligible_at
                    .and_then(|ts| i64::try_from(ts.0).ok()),
            )
            .bind(&effect.destructive_resource)
            .bind(effect_status_to_str(effect.status))
            .execute(&mut **tx)
            .await?;
        }
        for dep in &input.dependencies {
            sqlx::query(
            "INSERT INTO workflow_effect_dependencies (workflow_id, effect_id, depends_on_effect_id)
             VALUES (?1, ?2, ?3)",
        )
        .bind(to_i64(input.workflow_id.0, "workflow_id")?)
        .bind(to_i64(dep.effect_id.0, "effect_id")?)
        .bind(to_i64(dep.depends_on_effect_id.0, "depends_on_effect_id")?)
        .execute(&mut **tx)
        .await?;
        }
        for barrier in &input.barriers {
            sqlx::query(
                "INSERT INTO workflow_barriers (
                workflow_id, barrier_id, status, reducer_event_codec_family,
                reducer_event_codec_version, reducer_event_payload
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )
            .bind(to_i64(input.workflow_id.0, "workflow_id")?)
            .bind(to_i64(barrier.barrier_id.0, "barrier_id")?)
            .bind(barrier_status_to_str(barrier.status))
            .bind(&barrier.reducer_event_codec.family)
            .bind(i64::from(barrier.reducer_event_codec.version))
            .bind(&barrier.reducer_event_payload)
            .execute(&mut **tx)
            .await?;
        }
        for member in &input.barrier_members {
            sqlx::query(
            "INSERT INTO workflow_barrier_members (workflow_id, barrier_id, effect_id, receipt_family)
             VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(to_i64(input.workflow_id.0, "workflow_id")?)
        .bind(to_i64(member.barrier_id.0, "barrier_id")?)
        .bind(to_i64(member.effect_id.0, "effect_id")?)
        .bind(receipt_family_to_str(member.receipt_family))
        .execute(&mut **tx)
        .await?;
        }
        for delivery in &input.deliveries {
            sqlx::query(
                "INSERT INTO workflow_deliveries (
                workflow_id, delivery_id, effect_id, barrier_id, consumer_kind,
                event_codec_family, event_codec_version, payload_kind, payload_blob,
                requires_runtime_acceptance, status, runtime_acceptance_status,
                suppression_reason, accepted_by_transition_id
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'Pending', ?11, NULL, NULL)",
            )
            .bind(to_i64(input.workflow_id.0, "workflow_id")?)
            .bind(to_i64(delivery.delivery_id.0, "delivery_id")?)
            .bind(delivery.effect_id.and_then(|v| i64::try_from(v.0).ok()))
            .bind(delivery.barrier_id.and_then(|v| i64::try_from(v.0).ok()))
            .bind(&delivery.consumer_kind)
            .bind(&delivery.event_codec.family)
            .bind(i64::from(delivery.event_codec.version))
            .bind(delivery_payload_kind_to_str(delivery.payload_kind))
            .bind(&delivery.payload_blob)
            .bind(delivery.requires_runtime_acceptance)
            .bind(
                delivery
                    .runtime_acceptance_status
                    .map(runtime_acceptance_status_to_str),
            )
            .execute(&mut **tx)
            .await?;
        }
        for schedule in &input.schedules {
            sqlx::query(
                "INSERT INTO workflow_schedules (
                workflow_id, schedule_id, policy, schedule_key, status, next_eligible_at,
                active_effect_id, due_occurrence_id, due_generation, due_at,
                active_occurrence_id, active_generation, active_due_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            )
            .bind(to_i64(input.workflow_id.0, "workflow_id")?)
            .bind(to_i64(schedule.schedule_id.0, "schedule_id")?)
            .bind(schedule_policy_to_str(schedule.policy))
            .bind(&schedule.key)
            .bind(schedule_status_to_str(schedule.status))
            .bind(to_i64(schedule.next_eligible_at.0, "next_eligible_at")?)
            .bind(
                schedule
                    .active_effect_id
                    .and_then(|v| i64::try_from(v.0).ok()),
            )
            .bind(
                schedule
                    .due_occurrence
                    .and_then(|o| i64::try_from(o.occurrence_id.0).ok()),
            )
            .bind(
                schedule
                    .due_occurrence
                    .and_then(|o| i64::try_from(o.generation.0).ok()),
            )
            .bind(
                schedule
                    .due_occurrence
                    .and_then(|o| i64::try_from(o.due_at.0).ok()),
            )
            .bind(
                schedule
                    .active_occurrence
                    .and_then(|o| i64::try_from(o.occurrence_id.0).ok()),
            )
            .bind(
                schedule
                    .active_occurrence
                    .and_then(|o| i64::try_from(o.generation.0).ok()),
            )
            .bind(
                schedule
                    .active_occurrence
                    .and_then(|o| i64::try_from(o.due_at.0).ok()),
            )
            .execute(&mut **tx)
            .await?;
        }
        Ok(CommitOutcome::Committed)
    }

    /// # Errors
    ///
    /// Returns an error when the CAS transaction or any child insert fails.
    pub async fn commit_transition_plan(
        &self,
        input: &CommitTransitionPlanCas,
    ) -> DbResult<CommitOutcome> {
        if workflow_profile_kind(&self.pool, input.workflow_id)
            .await?
            .as_deref()
            == Some(DIRECT_TURN_PROFILE_KIND)
        {
            return Ok(CommitOutcome::InvalidPlan);
        }
        let mut tx = self.pool.begin().await?;
        let outcome = Self::commit_transition_plan_tx(&mut tx, input).await?;
        if outcome == CommitOutcome::Committed {
            tx.commit().await?;
        } else {
            tx.rollback().await?;
        }
        Ok(outcome)
    }

    pub async fn begin_attempt(&self, input: &BeginAttemptInput) -> DbResult<BeginAttemptResult> {
        for _ in 0..5 {
            match self.begin_attempt_once(input).await {
                Err(DbError::Sqlx(sqlx::Error::Database(error)))
                    if is_sqlite_busy_retryable(error.as_ref()) =>
                {
                    std::thread::yield_now();
                }
                result => return result,
            }
        }
        Ok(BeginAttemptResult {
            outcome: ClaimOutcome::AuthorityConflict,
            authority: None,
            attempt: None,
        })
    }

    async fn begin_attempt_once(&self, input: &BeginAttemptInput) -> DbResult<BeginAttemptResult> {
        let mut tx = self.begin_tx().await?;
        let result = tx.begin_attempt(input).await?;
        if result.outcome == ClaimOutcome::Started {
            tx.commit().await?;
        } else {
            tx.rollback().await?;
        }
        Ok(result)
    }

    pub async fn renew_lease_exact(&self, input: &RenewLeaseInput) -> DbResult<AuthorityOutcome> {
        let mut tx = self.pool.begin().await?;
        let updated = sqlx::query(
            "UPDATE workflow_reclaimable_leases
             SET lease_until = ?4
             WHERE workflow_id = ?1 AND attempt_id = ?2 AND lease_until < ?4 AND lease_until >= ?3",
        )
        .bind(to_i64(input.authority.workflow_id.0, "workflow_id")?)
        .bind(to_i64(input.authority.attempt_id.0, "attempt_id")?)
        .bind(to_i64(input.now.0, "now")?)
        .bind(to_i64(input.new_lease_until.0, "new_lease_until")?)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if updated == 0 {
            tx.rollback().await?;
            return Ok(AuthorityOutcome::StaleAuthority);
        }
        tx.commit().await?;
        Ok(AuthorityOutcome::Authorized)
    }

    pub async fn expire_lease_exact(&self, input: &ExpireLeaseInput) -> DbResult<AuthorityOutcome> {
        let mut tx = self.pool.begin().await?;
        let lease = sqlx::query("SELECT a.status as attempt_status, e.capability_kind FROM workflow_reclaimable_leases l JOIN workflow_attempts a ON a.workflow_id = l.workflow_id AND a.attempt_id = l.attempt_id JOIN workflow_effects e ON e.workflow_id = a.workflow_id AND e.effect_id = a.effect_id WHERE l.workflow_id = ?1 AND l.attempt_id = ?2 AND a.effect_id = ?3 AND l.lease_until <= ?4")
            .bind(to_i64(input.workflow_id.0, "workflow_id")?)
            .bind(to_i64(input.attempt_id.0, "attempt_id")?)
            .bind(to_i64(input.effect_id.0, "effect_id")?)
            .bind(to_i64(input.now.0, "now")?)
            .fetch_optional(&mut *tx)
            .await?;
        let Some(lease) = lease else {
            tx.rollback().await?;
            return Ok(AuthorityOutcome::StaleAuthority);
        };
        if !matches!(
            parse_attempt_status(&lease.get::<String, _>("attempt_status"))?,
            AttemptStatus::Begun | AttemptStatus::ObservationRecorded
        ) {
            tx.rollback().await?;
            return Ok(AuthorityOutcome::StaleAuthority);
        }
        sqlx::query(
            "DELETE FROM workflow_reclaimable_leases WHERE workflow_id = ?1 AND attempt_id = ?2",
        )
        .bind(to_i64(input.workflow_id.0, "workflow_id")?)
        .bind(to_i64(input.attempt_id.0, "attempt_id")?)
        .execute(&mut *tx)
        .await?;
        sqlx::query("UPDATE workflow_attempts SET status = 'AuthorityLost' WHERE workflow_id = ?1 AND attempt_id = ?2")
            .bind(to_i64(input.workflow_id.0, "workflow_id")?)
            .bind(to_i64(input.attempt_id.0, "attempt_id")?)
            .execute(&mut *tx).await?;
        let next_status = match lease.get::<String, _>("capability_kind").as_str() {
            "ReclaimableObservation" => "Eligible",
            "SafelyRepeatable" => "RetryWait",
            _ => "AmbiguityWait",
        };
        sqlx::query("UPDATE workflow_effects SET status = ?3 WHERE workflow_id = ?1 AND effect_id = ?2 AND status = 'Executing'")
            .bind(to_i64(input.workflow_id.0, "workflow_id")?)
            .bind(to_i64(input.effect_id.0, "effect_id")?)
            .bind(next_status)
            .execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(AuthorityOutcome::Authorized)
    }

    pub async fn record_observation(
        &self,
        input: &RecordObservationInput,
    ) -> DbResult<RecordObservationResult> {
        let mut tx = self.begin_tx().await?;
        let result = tx.record_observation(input).await?;
        tx.commit().await?;
        Ok(result)
    }

    pub async fn accept_receipt(
        &self,
        input: &AcceptReceiptInput,
    ) -> DbResult<ReceiptAcceptanceResult> {
        for _ in 0..5 {
            match self.accept_receipt_once(input).await {
                Err(DbError::Sqlx(sqlx::Error::Database(error)))
                    if is_sqlite_busy_retryable(error.as_ref()) =>
                {
                    std::thread::yield_now();
                }
                result => return result,
            }
        }
        Ok(ReceiptAcceptanceResult {
            outcome: AuthorityOutcome::StaleAuthority,
            receipt: None,
            delivery: None,
        })
    }

    async fn accept_receipt_once(
        &self,
        input: &AcceptReceiptInput,
    ) -> DbResult<ReceiptAcceptanceResult> {
        let mut tx = self.begin_tx().await?;
        let result = tx.accept_receipt_and_delivery(input).await?;
        if result.outcome == AuthorityOutcome::Authorized {
            tx.commit().await?;
        } else {
            tx.rollback().await?;
        }
        Ok(result)
    }

    pub async fn accept_or_suppress_deliveries_exact(
        &self,
        input: &AcceptOrSuppressDeliveryInput,
    ) -> DbResult<CommitOutcome> {
        if workflow_profile_kind(&self.pool, input.workflow_id)
            .await?
            .as_deref()
            == Some(DIRECT_TURN_PROFILE_KIND)
        {
            return Ok(CommitOutcome::InvalidPlan);
        }
        let mut tx = self.begin_tx().await?;
        let mut exact_delivery_ids = input.accept_delivery_ids.clone();
        exact_delivery_ids.extend(input.suppress_delivery_ids.iter().copied());
        let decision = if input.suppress_delivery_ids.is_empty() {
            DeliveryResolutionDecision::Accept
        } else if input.accept_delivery_ids.is_empty() {
            DeliveryResolutionDecision::Suppress {
                reason: input.suppression_reason,
            }
        } else {
            tx.rollback().await?;
            return Ok(CommitOutcome::InvalidPlan);
        };
        let outcome = tx
            .resolve_deliveries_exact(DeliveryResolutionPlan {
                workflow_id: input.workflow_id,
                expected_version: input.expected_version,
                transition_id: input.transition_id,
                generation: input.generation,
                next_status: input.next_status,
                event_codec: &input.event_codec,
                event_payload: &input.event_payload,
                next_snapshot_codec: &input.next_snapshot_codec,
                next_snapshot_payload: &input.next_snapshot_payload,
                committed_at: input.committed_at,
                exact_delivery_ids: &exact_delivery_ids,
                decision,
            })
            .await?;
        match outcome {
            CommitOutcome::Committed => tx.commit().await?,
            CommitOutcome::VersionConflict
            | CommitOutcome::InvalidPlan
            | CommitOutcome::UnsupportedCodec => tx.rollback().await?,
        }
        Ok(outcome)
    }

    pub async fn resolve_manual_choice(
        &self,
        input: &ResolveManualInput,
    ) -> DbResult<CommitOutcome> {
        let mut tx = self.pool.begin().await?;
        let resolution = sqlx::query("SELECT effect_id, status FROM workflow_manual_resolutions WHERE workflow_id = ?1 AND manual_resolution_id = ?2 AND workflow_version = ?3")
            .bind(to_i64(input.workflow_id.0, "workflow_id")?)
            .bind(to_i64(input.manual_resolution_id.0, "manual_resolution_id")?)
            .bind(to_i64(input.expected_version.0, "workflow_version")?)
            .fetch_optional(&mut *tx).await?;
        let Some(resolution) = resolution else {
            tx.rollback().await?;
            return Ok(CommitOutcome::InvalidPlan);
        };
        if resolution.get::<String, _>("status") != "Required" {
            tx.rollback().await?;
            return Ok(CommitOutcome::InvalidPlan);
        }
        let choice = sqlx::query("SELECT * FROM workflow_manual_resolution_choices WHERE workflow_id = ?1 AND manual_resolution_id = ?2 AND ordinal = ?3")
            .bind(to_i64(input.workflow_id.0, "workflow_id")?)
            .bind(to_i64(input.manual_resolution_id.0, "manual_resolution_id")?)
            .bind(i64::from(input.choice_ordinal))
            .fetch_optional(&mut *tx).await?;
        let Some(choice) = choice else {
            tx.rollback().await?;
            return Ok(CommitOutcome::InvalidPlan);
        };
        let kind = parse_manual_choice_kind(&choice.get::<String, _>("kind"))?;
        if kind == ManualChoiceKind::Retry && input.retry_at.is_none() {
            tx.rollback().await?;
            return Ok(CommitOutcome::InvalidPlan);
        }
        let updated = sqlx::query("UPDATE workflows SET version = version + 1, generation = ?3, status = ?4, snapshot_codec_family = ?5, snapshot_codec_version = ?6, snapshot_payload = ?7, updated_at = ?8 WHERE workflow_id = ?1 AND version = ?2")
            .bind(to_i64(input.workflow_id.0, "workflow_id")?)
            .bind(to_i64(input.expected_version.0, "expected_version")?)
            .bind(to_i64(input.generation.0, "generation")?)
            .bind(workflow_status_to_str(input.next_status))
            .bind(&input.next_snapshot_codec.family)
            .bind(i64::from(input.next_snapshot_codec.version))
            .bind(&input.next_snapshot_payload)
            .bind(to_i64(input.committed_at.0, "committed_at")?)
            .execute(&mut *tx).await?.rows_affected();
        if updated == 0 {
            tx.rollback().await?;
            return Ok(CommitOutcome::VersionConflict);
        }
        sqlx::query("INSERT INTO workflow_transitions (workflow_id, transition_id, from_version, to_version, generation, event_codec_family, event_codec_version, event_payload, committed_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)")
            .bind(to_i64(input.workflow_id.0, "workflow_id")?)
            .bind(to_i64(input.transition_id.0, "transition_id")?)
            .bind(to_i64(input.expected_version.0, "expected_version")?)
            .bind(to_i64(input.expected_version.next().0, "to_version")?)
            .bind(to_i64(input.generation.0, "generation")?)
            .bind(&input.transition_codec.family)
            .bind(i64::from(input.transition_codec.version))
            .bind(&input.transition_payload)
            .bind(to_i64(input.committed_at.0, "committed_at")?)
            .execute(&mut *tx).await?;
        let effect_id = EffectId(to_u64(resolution.get::<i64, _>("effect_id"), "effect_id")?);
        sqlx::query("UPDATE workflow_manual_resolutions SET status = 'Resolved', accepted_choice_ordinal = ?3, resolved_by = ?4 WHERE workflow_id = ?1 AND manual_resolution_id = ?2")
            .bind(to_i64(input.workflow_id.0, "workflow_id")?)
            .bind(to_i64(input.manual_resolution_id.0, "manual_resolution_id")?)
            .bind(i64::from(input.choice_ordinal))
            .bind(&input.resolved_by)
            .execute(&mut *tx).await?;
        match kind {
            ManualChoiceKind::Retry => {
                sqlx::query("UPDATE workflow_effects SET status = 'RetryWait', next_eligible_at = ?3 WHERE workflow_id = ?1 AND effect_id = ?2")
                    .bind(to_i64(input.workflow_id.0, "workflow_id")?)
                    .bind(to_i64(effect_id.0, "effect_id")?)
                    .bind(input.retry_at.and_then(|ts| i64::try_from(ts.0).ok()))
                    .execute(&mut *tx).await?;
            }
            ManualChoiceKind::Compensate | ManualChoiceKind::Suppress => {
                sqlx::query("UPDATE workflow_effects SET status = 'Invalidated' WHERE workflow_id = ?1 AND effect_id = ?2")
                    .bind(to_i64(input.workflow_id.0, "workflow_id")?)
                    .bind(to_i64(effect_id.0, "effect_id")?)
                    .execute(&mut *tx).await?;
            }
            ManualChoiceKind::AcceptAsTerminal => {
                sqlx::query("UPDATE workflow_effects SET status = 'Receipted' WHERE workflow_id = ?1 AND effect_id = ?2")
                    .bind(to_i64(input.workflow_id.0, "workflow_id")?)
                    .bind(to_i64(effect_id.0, "effect_id")?)
                    .execute(&mut *tx).await?;
                sqlx::query("INSERT INTO workflow_receipts (workflow_id, receipt_id, effect_id, generation, declared_workflow_version, process_incarnation, attempt_id, origin, receipt_codec_family, receipt_codec_version, receipt_payload) SELECT ?1, ?2, effect_id, generation, declared_workflow_version, 0, NULL, 'Manual', ?3, ?4, ?5 FROM workflow_effects WHERE workflow_id = ?1 AND effect_id = ?6")
                    .bind(to_i64(input.workflow_id.0, "workflow_id")?)
                    .bind(to_i64(input.manual_receipt_id.ok_or_else(|| DbError::Serialization("manual_receipt_id required".to_string()))?.0, "receipt_id")?)
                    .bind(choice.get::<String, _>("receipt_codec_family"))
                    .bind(choice.get::<i64, _>("receipt_codec_version"))
                    .bind(choice.get::<Vec<u8>, _>("receipt_blob"))
                    .bind(to_i64(effect_id.0, "effect_id")?)
                    .execute(&mut *tx).await?;
                sqlx::query("INSERT INTO workflow_deliveries (workflow_id, delivery_id, effect_id, barrier_id, consumer_kind, event_codec_family, event_codec_version, payload_kind, payload_blob, requires_runtime_acceptance, status, runtime_acceptance_status, suppression_reason, accepted_by_transition_id) VALUES (?1, ?2, ?3, NULL, 'reducer', ?4, ?5, 'Receipt', ?6, ?7, 'Pending', ?8, NULL, NULL)")
                    .bind(to_i64(input.workflow_id.0, "workflow_id")?)
                    .bind(to_i64(input.manual_delivery_id.ok_or_else(|| DbError::Serialization("manual_delivery_id required".to_string()))?.0, "delivery_id")?)
                    .bind(to_i64(effect_id.0, "effect_id")?)
                    .bind(choice.get::<String, _>("receipt_event_codec_family"))
                    .bind(choice.get::<i64, _>("receipt_event_codec_version"))
                    .bind(choice.get::<Vec<u8>, _>("receipt_event_blob"))
                    .bind(input.manual_receipt_requires_runtime_acceptance)
                    .bind(if input.manual_receipt_requires_runtime_acceptance { Some("Owed") } else { None })
                    .execute(&mut *tx).await?;
            }
        }
        tx.commit().await?;
        Ok(CommitOutcome::Committed)
    }

    pub async fn reconcile_schedule_due_exact(
        &self,
        input: &ReconcileScheduleDueInput,
    ) -> DbResult<Option<ScheduleOccurrence>> {
        let mut tx = self.pool.begin().await?;
        let updated = sqlx::query("UPDATE workflow_schedules SET status = 'Due', due_occurrence_id = ?3, due_generation = (SELECT generation FROM workflows WHERE workflow_id = ?1), due_at = next_eligible_at, active_effect_id = NULL, active_occurrence_id = NULL, active_generation = NULL, active_due_at = NULL WHERE workflow_id = ?1 AND schedule_id = ?2 AND status = 'Idle' AND next_eligible_at <= ?4")
            .bind(to_i64(input.workflow_id.0, "workflow_id")?)
            .bind(to_i64(input.schedule_id.0, "schedule_id")?)
            .bind(to_i64(input.new_occurrence_id.0, "occurrence_id")?)
            .bind(to_i64(input.now.0, "now")?)
            .execute(&mut *tx).await?.rows_affected();
        if updated == 0 {
            tx.rollback().await?;
            return Ok(None);
        }
        let row = sqlx::query("SELECT due_generation, due_at FROM workflow_schedules WHERE workflow_id = ?1 AND schedule_id = ?2")
            .bind(to_i64(input.workflow_id.0, "workflow_id")?)
            .bind(to_i64(input.schedule_id.0, "schedule_id")?)
            .fetch_one(&mut *tx).await?;
        tx.commit().await?;
        Ok(Some(ScheduleOccurrence {
            schedule_id: input.schedule_id,
            occurrence_id: input.new_occurrence_id,
            generation: Generation(to_u64(
                row.get::<i64, _>("due_generation"),
                "due_generation",
            )?),
            due_at: Timestamp(to_u64(row.get::<i64, _>("due_at"), "due_at")?),
        }))
    }

    pub async fn start_schedule_occurrence_exact(
        &self,
        input: &StartScheduleOccurrenceInput,
    ) -> DbResult<bool> {
        let mut tx = self.pool.begin().await?;
        let updated = sqlx::query("UPDATE workflow_schedules SET status = 'Active', active_effect_id = ?3, active_occurrence_id = ?4, active_generation = ?5, active_due_at = ?6, due_occurrence_id = NULL, due_generation = NULL, due_at = NULL WHERE workflow_id = ?1 AND schedule_id = ?2 AND status = 'Due' AND due_occurrence_id = ?4 AND due_generation = ?5 AND due_at = ?6")
            .bind(to_i64(input.workflow_id.0, "workflow_id")?)
            .bind(to_i64(input.occurrence.schedule_id.0, "schedule_id")?)
            .bind(input.active_effect_id.and_then(|id| i64::try_from(id.0).ok()))
            .bind(to_i64(input.occurrence.occurrence_id.0, "occurrence_id")?)
            .bind(to_i64(input.occurrence.generation.0, "generation")?)
            .bind(to_i64(input.occurrence.due_at.0, "due_at")?)
            .execute(&mut *tx).await?.rows_affected();
        if updated == 0 {
            tx.rollback().await?;
            return Ok(false);
        }
        tx.commit().await?;
        Ok(true)
    }

    pub async fn complete_schedule_occurrence_exact(
        &self,
        input: &CompleteScheduleOccurrenceInput,
    ) -> DbResult<bool> {
        let mut tx = self.pool.begin().await?;
        let updated = sqlx::query("UPDATE workflow_schedules SET status = 'Idle', active_effect_id = NULL, active_occurrence_id = NULL, active_generation = NULL, active_due_at = NULL, due_occurrence_id = NULL, due_generation = NULL, due_at = NULL, next_eligible_at = ?3 WHERE workflow_id = ?1 AND schedule_id = ?2 AND status = 'Active' AND active_occurrence_id = ?4 AND active_generation = ?5 AND active_due_at = ?6")
            .bind(to_i64(input.workflow_id.0, "workflow_id")?)
            .bind(to_i64(input.occurrence.schedule_id.0, "schedule_id")?)
            .bind(to_i64(input.next_eligible_at.0, "next_eligible_at")?)
            .bind(to_i64(input.occurrence.occurrence_id.0, "occurrence_id")?)
            .bind(to_i64(input.occurrence.generation.0, "generation")?)
            .bind(to_i64(input.occurrence.due_at.0, "due_at")?)
            .execute(&mut *tx).await?.rows_affected();
        if updated == 0 {
            tx.rollback().await?;
            return Ok(false);
        }
        tx.commit().await?;
        Ok(true)
    }

    pub async fn list_attempts(
        &self,
        workflow_id: WorkflowId,
        effect_id: EffectId,
    ) -> DbResult<Vec<LocalAttemptRecord>> {
        let rows = sqlx::query(
            "SELECT a.attempt_id, a.ordinal, a.declared_workflow_version, a.generation,
                    a.effect_id, a.process_incarnation, a.status, l.lease_until
             FROM workflow_attempts a
             LEFT JOIN workflow_reclaimable_leases l
               ON l.workflow_id = a.workflow_id AND l.attempt_id = a.attempt_id
             WHERE a.workflow_id = ?1 AND a.effect_id = ?2
             ORDER BY a.ordinal",
        )
        .bind(to_i64(workflow_id.0, "workflow_id")?)
        .bind(to_i64(effect_id.0, "effect_id")?)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                let authority = LocalAttemptAuthority {
                    workflow_id,
                    declared_workflow_version: Version(to_u64(
                        row.get::<i64, _>("declared_workflow_version"),
                        "declared_workflow_version",
                    )?),
                    generation: Generation(to_u64(row.get::<i64, _>("generation"), "generation")?),
                    effect_id: EffectId(to_u64(row.get::<i64, _>("effect_id"), "effect_id")?),
                    attempt_id: AttemptId(to_u64(row.get::<i64, _>("attempt_id"), "attempt_id")?),
                    process_incarnation: ProcessIncarnation(to_u64(
                        row.get::<i64, _>("process_incarnation"),
                        "process_incarnation",
                    )?),
                };
                Ok(LocalAttemptRecord {
                    id: authority.attempt_id,
                    ordinal: to_u32(row.get::<i64, _>("ordinal"), "ordinal")?,
                    authority: authority.clone(),
                    status: parse_attempt_status(&row.get::<String, _>("status"))?,
                    lease: row
                        .get::<Option<i64>, _>("lease_until")
                        .map(|value| {
                            to_u64(value, "lease_until").map(|lease_until| LocalReclaimableLease {
                                attempt_id: authority.attempt_id,
                                lease_until: LeaseExpiry(lease_until),
                            })
                        })
                        .transpose()?,
                })
            })
            .collect()
    }

    pub async fn list_deliveries(
        &self,
        workflow_id: WorkflowId,
    ) -> DbResult<Vec<LocalDeliveryRecord>> {
        let rows = sqlx::query(
            "SELECT * FROM workflow_deliveries WHERE workflow_id = ?1 ORDER BY delivery_id",
        )
        .bind(to_i64(workflow_id.0, "workflow_id")?)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(LocalDeliveryRecord {
                    delivery_id: DeliveryId(to_u64(
                        row.get::<i64, _>("delivery_id"),
                        "delivery_id",
                    )?),
                    workflow_id,
                    effect_id: row
                        .get::<Option<i64>, _>("effect_id")
                        .map(|v| to_u64(v, "effect_id").map(EffectId))
                        .transpose()?,
                    barrier_id: row
                        .get::<Option<i64>, _>("barrier_id")
                        .map(|v| to_u64(v, "barrier_id").map(BarrierId))
                        .transpose()?,
                    consumer_kind: row.get("consumer_kind"),
                    event_codec: local_codec(
                        row.get("event_codec_family"),
                        row.get("event_codec_version"),
                        "event_codec_version",
                    )?,
                    payload_kind: parse_delivery_payload_kind(
                        &row.get::<String, _>("payload_kind"),
                    )?,
                    payload_blob: row.get("payload_blob"),
                    requires_runtime_acceptance: row.get("requires_runtime_acceptance"),
                    status: parse_delivery_status(&row.get::<String, _>("status"))?,
                    runtime_acceptance_status: parse_runtime_acceptance_status(
                        row.get::<Option<String>, _>("runtime_acceptance_status"),
                    )?,
                    suppression_reason: parse_suppression_reason(
                        row.get::<Option<String>, _>("suppression_reason"),
                    )?,
                    accepted_by_transition_id: row
                        .get::<Option<i64>, _>("accepted_by_transition_id")
                        .map(|v| to_u64(v, "accepted_by_transition_id").map(TransitionId))
                        .transpose()?,
                })
            })
            .collect()
    }

    pub async fn list_receipts(
        &self,
        workflow_id: WorkflowId,
    ) -> DbResult<Vec<LocalReceiptRecord>> {
        let rows = sqlx::query(
            "SELECT * FROM workflow_receipts WHERE workflow_id = ?1 ORDER BY receipt_id",
        )
        .bind(to_i64(workflow_id.0, "workflow_id")?)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(LocalReceiptRecord {
                    receipt_id: ReceiptId(to_u64(row.get::<i64, _>("receipt_id"), "receipt_id")?),
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
                        .map(|v| to_u64(v, "attempt_id").map(AttemptId))
                        .transpose()?,
                    origin: parse_receipt_origin(&row.get::<String, _>("origin"))?,
                    receipt_codec: local_codec(
                        row.get("receipt_codec_family"),
                        row.get("receipt_codec_version"),
                        "receipt_codec_version",
                    )?,
                    receipt_payload: row.get("receipt_payload"),
                })
            })
            .collect()
    }

    pub async fn list_authoritative_observations(
        &self,
        workflow_id: WorkflowId,
        effect_id: EffectId,
    ) -> DbResult<Vec<LocalObservationRecord>> {
        self.list_observations_table(
            "workflow_authoritative_observations",
            workflow_id,
            effect_id,
            true,
        )
        .await
    }

    pub async fn list_stale_observations(
        &self,
        workflow_id: WorkflowId,
        effect_id: EffectId,
    ) -> DbResult<Vec<LocalObservationRecord>> {
        self.list_observations_table("workflow_stale_observations", workflow_id, effect_id, false)
            .await
    }

    async fn list_observations_table(
        &self,
        _table: &str,
        workflow_id: WorkflowId,
        effect_id: EffectId,
        authoritative: bool,
    ) -> DbResult<Vec<LocalObservationRecord>> {
        let sql = if authoritative {
            "SELECT * FROM workflow_authoritative_observations WHERE workflow_id = ?1 AND effect_id = ?2 ORDER BY observation_id"
        } else {
            "SELECT * FROM workflow_stale_observations WHERE workflow_id = ?1 AND effect_id = ?2 ORDER BY observation_id"
        };
        let rows = sqlx::query(sql)
            .bind(to_i64(workflow_id.0, "workflow_id")?)
            .bind(to_i64(effect_id.0, "effect_id")?)
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter()
            .map(|row| {
                Ok(LocalObservationRecord {
                    observation_id: to_u64(row.get::<i64, _>("observation_id"), "observation_id")?,
                    workflow_id,
                    effect_id,
                    attempt_id: AttemptId(to_u64(row.get::<i64, _>("attempt_id"), "attempt_id")?),
                    declared_workflow_version: Version(to_u64(
                        row.get::<i64, _>("declared_workflow_version"),
                        "declared_workflow_version",
                    )?),
                    generation: Generation(to_u64(row.get::<i64, _>("generation"), "generation")?),
                    process_incarnation: ProcessIncarnation(to_u64(
                        row.get::<i64, _>("process_incarnation"),
                        "process_incarnation",
                    )?),
                    observation_codec: local_codec(
                        row.get("observation_codec_family"),
                        row.get("observation_codec_version"),
                        "observation_codec_version",
                    )?,
                    observation_payload: row.get("observation_payload"),
                    observed_at: Timestamp(to_u64(
                        row.get::<i64, _>("observed_at"),
                        "observed_at",
                    )?),
                    recorded_at: Timestamp(to_u64(
                        row.get::<i64, _>("recorded_at"),
                        "recorded_at",
                    )?),
                    authoritative,
                })
            })
            .collect()
    }

    pub async fn get_schedule(
        &self,
        workflow_id: WorkflowId,
        schedule_id: ScheduleId,
    ) -> DbResult<Option<LocalScheduleDecl>> {
        let row = sqlx::query(
            "SELECT * FROM workflow_schedules WHERE workflow_id = ?1 AND schedule_id = ?2",
        )
        .bind(to_i64(workflow_id.0, "workflow_id")?)
        .bind(to_i64(schedule_id.0, "schedule_id")?)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            Ok(LocalScheduleDecl {
                schedule_id,
                policy: parse_schedule_policy(&row.get::<String, _>("policy"))?,
                key: row.get("schedule_key"),
                status: parse_schedule_status(&row.get::<String, _>("status"))?,
                next_eligible_at: Timestamp(to_u64(
                    row.get::<i64, _>("next_eligible_at"),
                    "next_eligible_at",
                )?),
                active_effect_id: row
                    .get::<Option<i64>, _>("active_effect_id")
                    .map(|v| to_u64(v, "active_effect_id").map(EffectId))
                    .transpose()?,
                due_occurrence: match row.get::<Option<i64>, _>("due_occurrence_id") {
                    Some(id) => Some(ScheduleOccurrence {
                        schedule_id,
                        occurrence_id: ScheduleOccurrenceId(to_u64(id, "due_occurrence_id")?),
                        generation: Generation(to_u64(
                            row.get::<i64, _>("due_generation"),
                            "due_generation",
                        )?),
                        due_at: Timestamp(to_u64(row.get::<i64, _>("due_at"), "due_at")?),
                    }),
                    None => None,
                },
                active_occurrence: match row.get::<Option<i64>, _>("active_occurrence_id") {
                    Some(id) => Some(ScheduleOccurrence {
                        schedule_id,
                        occurrence_id: ScheduleOccurrenceId(to_u64(id, "active_occurrence_id")?),
                        generation: Generation(to_u64(
                            row.get::<i64, _>("active_generation"),
                            "active_generation",
                        )?),
                        due_at: Timestamp(to_u64(
                            row.get::<i64, _>("active_due_at"),
                            "active_due_at",
                        )?),
                    }),
                    None => None,
                },
            })
        })
        .transpose()
    }

    pub async fn get_manual_resolution(
        &self,
        workflow_id: WorkflowId,
        manual_resolution_id: ManualResolutionId,
    ) -> DbResult<Option<ManualResolutionRecordRow>> {
        let row = sqlx::query("SELECT * FROM workflow_manual_resolutions WHERE workflow_id = ?1 AND manual_resolution_id = ?2")
            .bind(to_i64(workflow_id.0, "workflow_id")?)
            .bind(to_i64(manual_resolution_id.0, "manual_resolution_id")?)
            .fetch_optional(&self.pool).await?;
        row.map(|row| {
            Ok(ManualResolutionRecordRow {
                manual_resolution_id,
                workflow_id,
                workflow_version: Version(to_u64(
                    row.get::<i64, _>("workflow_version"),
                    "workflow_version",
                )?),
                effect_id: EffectId(to_u64(row.get::<i64, _>("effect_id"), "effect_id")?),
                status: parse_resolution_status(&row.get::<String, _>("status"))?,
                accepted_choice_ordinal: row
                    .get::<Option<i64>, _>("accepted_choice_ordinal")
                    .map(|v| to_u32(v, "accepted_choice_ordinal"))
                    .transpose()?,
                resolved_by: row.get("resolved_by"),
            })
        })
        .transpose()
    }

    pub async fn list_manual_resolution_choices(
        &self,
        workflow_id: WorkflowId,
        manual_resolution_id: ManualResolutionId,
    ) -> DbResult<Vec<ManualResolutionChoiceRow>> {
        let rows = sqlx::query("SELECT * FROM workflow_manual_resolution_choices WHERE workflow_id = ?1 AND manual_resolution_id = ?2 ORDER BY ordinal")
            .bind(to_i64(workflow_id.0, "workflow_id")?)
            .bind(to_i64(manual_resolution_id.0, "manual_resolution_id")?)
            .fetch_all(&self.pool).await?;
        rows.into_iter()
            .map(|row| {
                Ok(ManualResolutionChoiceRow {
                    manual_resolution_id,
                    ordinal: to_u32(row.get::<i64, _>("ordinal"), "ordinal")?,
                    kind: parse_manual_choice_kind(&row.get::<String, _>("kind"))?,
                    payload_codec: local_codec(
                        row.get("payload_codec_family"),
                        row.get("payload_codec_version"),
                        "payload_codec_version",
                    )?,
                    payload_blob: row.get("payload_blob"),
                    receipt_codec: local_codec(
                        row.get("receipt_codec_family"),
                        row.get("receipt_codec_version"),
                        "receipt_codec_version",
                    )?,
                    receipt_blob: row.get("receipt_blob"),
                    receipt_event_codec: local_codec(
                        row.get("receipt_event_codec_family"),
                        row.get("receipt_event_codec_version"),
                        "receipt_event_codec_version",
                    )?,
                    receipt_event_blob: row.get("receipt_event_blob"),
                })
            })
            .collect()
    }

    pub async fn fetch_workflow_head(
        &self,
        workflow_id: WorkflowId,
    ) -> DbResult<Option<WorkflowHead>> {
        let mut tx = self.begin_tx().await?;
        let head = tx.fetch_workflow_head(workflow_id).await?;
        tx.commit().await?;
        Ok(head)
    }
}

pub(super) async fn next_global_sequence_value_tx(
    tx: &mut WorkflowTx<'_>,
    sequence_name: &str,
    field: &str,
) -> DbResult<u64> {
    sqlx::query(
        "INSERT INTO workflow_global_sequences (sequence_name, next_value)
         VALUES (?1, 2)
         ON CONFLICT(sequence_name)
         DO UPDATE SET next_value = workflow_global_sequences.next_value + 1",
    )
    .bind(sequence_name)
    .execute(&mut *tx.tx)
    .await?;
    let allocated = sqlx::query_scalar::<_, i64>(
        "SELECT next_value - 1 FROM workflow_global_sequences WHERE sequence_name = ?1",
    )
    .bind(sequence_name)
    .fetch_one(&mut *tx.tx)
    .await?;
    to_u64(allocated, field)
}

pub(super) async fn next_global_workflow_id_tx(tx: &mut WorkflowTx<'_>) -> DbResult<WorkflowId> {
    Ok(WorkflowId(
        next_global_sequence_value_tx(tx, "workflow", "workflow_id").await?,
    ))
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

fn workflow_sequence_name_str(sequence: WorkflowSequenceName) -> &'static str {
    match sequence {
        WorkflowSequenceName::Observation => "observation",
        WorkflowSequenceName::Receipt => "receipt",
        WorkflowSequenceName::Delivery => "delivery",
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

fn effect_role_to_str(role: EffectRole) -> &'static str {
    match role {
        EffectRole::Required => "Required",
        EffectRole::Optional => "Optional",
        EffectRole::Compensation => "Compensation",
    }
}

fn effect_status_to_str(status: EffectStatus) -> &'static str {
    match status {
        EffectStatus::Blocked => "Blocked",
        EffectStatus::Eligible => "Eligible",
        EffectStatus::Executing => "Executing",
        EffectStatus::RetryWait => "RetryWait",
        EffectStatus::AmbiguityWait => "AmbiguityWait",
        EffectStatus::Receipted => "Receipted",
        EffectStatus::Invalidated => "Invalidated",
    }
}

fn barrier_status_to_str(status: BarrierStatus) -> &'static str {
    match status {
        BarrierStatus::Waiting => "Waiting",
        BarrierStatus::Satisfied => "Satisfied",
    }
}

fn receipt_family_to_str(family: ReceiptFamily) -> &'static str {
    match family {
        ReceiptFamily::CurrentGenerationEffect => "CurrentGenerationEffect",
        ReceiptFamily::CompensationEffect => "CompensationEffect",
    }
}

fn receipt_origin_to_str(origin: ReceiptOrigin) -> &'static str {
    match origin {
        ReceiptOrigin::Execution => "Execution",
        ReceiptOrigin::Adoption => "Adoption",
        ReceiptOrigin::Reconciliation => "Reconciliation",
        ReceiptOrigin::Manual => "Manual",
        ReceiptOrigin::CancellationArbitration => "CancellationArbitration",
        ReceiptOrigin::DeadlineExpiration => "DeadlineExpiration",
        ReceiptOrigin::ForgottenInterruption => "ForgottenInterruption",
        ReceiptOrigin::ScheduleCollapse => "ScheduleCollapse",
    }
}

fn runtime_acceptance_status_to_str(status: RuntimeAcceptanceStatus) -> &'static str {
    match status {
        RuntimeAcceptanceStatus::Owed => "Owed",
        RuntimeAcceptanceStatus::Accepted => "Accepted",
        RuntimeAcceptanceStatus::Suppressed => "Suppressed",
    }
}

fn suppression_reason_to_str(reason: SuppressionReason) -> &'static str {
    match reason {
        SuppressionReason::Cancelled => "Cancelled",
        SuppressionReason::Superseded => "Superseded",
        SuppressionReason::LifecycleTerminal => "LifecycleTerminal",
        SuppressionReason::ReducerTerminal => "ReducerTerminal",
    }
}

fn schedule_policy_to_str(policy: SchedulePolicy) -> &'static str {
    match policy {
        SchedulePolicy::CoalesceLatest => "CoalesceLatest",
    }
}

fn schedule_status_to_str(status: ScheduleStatus) -> &'static str {
    match status {
        ScheduleStatus::Idle => "Idle",
        ScheduleStatus::Due => "Due",
        ScheduleStatus::Active => "Active",
    }
}

fn delivery_payload_kind_to_str(kind: LocalDeliveryPayloadKind) -> &'static str {
    match kind {
        LocalDeliveryPayloadKind::Receipt => "Receipt",
        LocalDeliveryPayloadKind::Barrier => "Barrier",
    }
}

fn capability_to_parts(capability: &ExecutionCapability) -> (&'static str, Option<u64>) {
    match capability {
        ExecutionCapability::ReclaimableObservation => ("ReclaimableObservation", None),
        ExecutionCapability::IdempotentSubmission { stable_command_id } => {
            ("IdempotentSubmission", Some(stable_command_id.0))
        }
        ExecutionCapability::ObservableSubmission { stable_command_id } => {
            ("ObservableSubmission", Some(stable_command_id.0))
        }
        ExecutionCapability::SafelyRepeatable => ("SafelyRepeatable", None),
        ExecutionCapability::ManualOnAmbiguity => ("ManualOnAmbiguity", None),
    }
}

fn parse_effect_status(value: &str) -> DbResult<EffectStatus> {
    match value {
        "Blocked" => Ok(EffectStatus::Blocked),
        "Eligible" => Ok(EffectStatus::Eligible),
        "Executing" => Ok(EffectStatus::Executing),
        "RetryWait" => Ok(EffectStatus::RetryWait),
        "AmbiguityWait" => Ok(EffectStatus::AmbiguityWait),
        "Receipted" => Ok(EffectStatus::Receipted),
        "Invalidated" => Ok(EffectStatus::Invalidated),
        other => Err(DbError::Serialization(format!(
            "unknown effect status: {other}"
        ))),
    }
}

fn parse_attempt_status(value: &str) -> DbResult<AttemptStatus> {
    match value {
        "Begun" => Ok(AttemptStatus::Begun),
        "ObservationRecorded" => Ok(AttemptStatus::ObservationRecorded),
        "ReceiptAccepted" => Ok(AttemptStatus::ReceiptAccepted),
        "AuthorityLost" => Ok(AttemptStatus::AuthorityLost),
        other => Err(DbError::Serialization(format!(
            "unknown attempt status: {other}"
        ))),
    }
}

fn parse_receipt_origin(value: &str) -> DbResult<ReceiptOrigin> {
    match value {
        "Execution" => Ok(ReceiptOrigin::Execution),
        "Adoption" => Ok(ReceiptOrigin::Adoption),
        "Reconciliation" => Ok(ReceiptOrigin::Reconciliation),
        "Manual" => Ok(ReceiptOrigin::Manual),
        "CancellationArbitration" => Ok(ReceiptOrigin::CancellationArbitration),
        "DeadlineExpiration" => Ok(ReceiptOrigin::DeadlineExpiration),
        "ForgottenInterruption" => Ok(ReceiptOrigin::ForgottenInterruption),
        "ScheduleCollapse" => Ok(ReceiptOrigin::ScheduleCollapse),
        other => Err(DbError::Serialization(format!(
            "unknown receipt origin: {other}"
        ))),
    }
}

fn parse_delivery_status(value: &str) -> DbResult<DeliveryStatus> {
    match value {
        "Pending" => Ok(DeliveryStatus::Pending),
        "Accepted" => Ok(DeliveryStatus::Accepted),
        "Deferred" => Ok(DeliveryStatus::Deferred),
        "Suppressed" => Ok(DeliveryStatus::Suppressed),
        other => Err(DbError::Serialization(format!(
            "unknown delivery status: {other}"
        ))),
    }
}

fn parse_runtime_acceptance_status(
    value: Option<String>,
) -> DbResult<Option<RuntimeAcceptanceStatus>> {
    value
        .map(|value| match value.as_str() {
            "Owed" => Ok(RuntimeAcceptanceStatus::Owed),
            "Accepted" => Ok(RuntimeAcceptanceStatus::Accepted),
            "Suppressed" => Ok(RuntimeAcceptanceStatus::Suppressed),
            other => Err(DbError::Serialization(format!(
                "unknown runtime acceptance status: {other}"
            ))),
        })
        .transpose()
}

fn parse_suppression_reason(value: Option<String>) -> DbResult<Option<SuppressionReason>> {
    value
        .map(|value| match value.as_str() {
            "Cancelled" => Ok(SuppressionReason::Cancelled),
            "Superseded" => Ok(SuppressionReason::Superseded),
            "LifecycleTerminal" => Ok(SuppressionReason::LifecycleTerminal),
            "ReducerTerminal" => Ok(SuppressionReason::ReducerTerminal),
            other => Err(DbError::Serialization(format!(
                "unknown suppression reason: {other}"
            ))),
        })
        .transpose()
}

fn parse_schedule_policy(value: &str) -> DbResult<SchedulePolicy> {
    match value {
        "CoalesceLatest" => Ok(SchedulePolicy::CoalesceLatest),
        other => Err(DbError::Serialization(format!(
            "unknown schedule policy: {other}"
        ))),
    }
}

fn parse_schedule_status(value: &str) -> DbResult<ScheduleStatus> {
    match value {
        "Idle" => Ok(ScheduleStatus::Idle),
        "Due" => Ok(ScheduleStatus::Due),
        "Active" => Ok(ScheduleStatus::Active),
        other => Err(DbError::Serialization(format!(
            "unknown schedule status: {other}"
        ))),
    }
}

fn parse_resolution_status(value: &str) -> DbResult<ResolutionStatus> {
    match value {
        "Required" => Ok(ResolutionStatus::Required),
        "Resolved" => Ok(ResolutionStatus::Resolved),
        other => Err(DbError::Serialization(format!(
            "unknown resolution status: {other}"
        ))),
    }
}

fn parse_manual_choice_kind(value: &str) -> DbResult<ManualChoiceKind> {
    match value {
        "Retry" => Ok(ManualChoiceKind::Retry),
        "Compensate" => Ok(ManualChoiceKind::Compensate),
        "Suppress" => Ok(ManualChoiceKind::Suppress),
        "AcceptAsTerminal" => Ok(ManualChoiceKind::AcceptAsTerminal),
        other => Err(DbError::Serialization(format!(
            "unknown manual choice kind: {other}"
        ))),
    }
}

fn parse_delivery_payload_kind(value: &str) -> DbResult<LocalDeliveryPayloadKind> {
    match value {
        "Receipt" => Ok(LocalDeliveryPayloadKind::Receipt),
        "Barrier" => Ok(LocalDeliveryPayloadKind::Barrier),
        other => Err(DbError::Serialization(format!(
            "unknown delivery payload kind: {other}"
        ))),
    }
}

fn parse_capability(kind: &str, stable_command_id: Option<i64>) -> DbResult<ExecutionCapability> {
    Ok(match kind {
        "ReclaimableObservation" => ExecutionCapability::ReclaimableObservation,
        "IdempotentSubmission" => ExecutionCapability::IdempotentSubmission {
            stable_command_id: phoenix_workflow::StableCommandId(to_u64(
                stable_command_id.ok_or_else(|| {
                    DbError::Serialization("stable_command_id missing".to_string())
                })?,
                "stable_command_id",
            )?),
        },
        "ObservableSubmission" => ExecutionCapability::ObservableSubmission {
            stable_command_id: phoenix_workflow::StableCommandId(to_u64(
                stable_command_id.ok_or_else(|| {
                    DbError::Serialization("stable_command_id missing".to_string())
                })?,
                "stable_command_id",
            )?),
        },
        "SafelyRepeatable" => ExecutionCapability::SafelyRepeatable,
        "ManualOnAmbiguity" => ExecutionCapability::ManualOnAmbiguity,
        other => {
            return Err(DbError::Serialization(format!(
                "unknown capability kind: {other}"
            )))
        }
    })
}

fn local_codec(family: String, version: i64, field: &str) -> DbResult<LocalCodec> {
    Ok(LocalCodec {
        family,
        version: to_u32(version, field)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrations::run_pending_migrations;
    use phoenix_workflow::{
        AcceptedDisposition, ClientTurnKey, ConversationAuthority, Generation, NonEmptyExternalKey,
        PreparedTurn, ScopeId, SupportedCodecRegistry, Timestamp, TransitionId, TurnOutcome,
    };
    use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
    use std::str::FromStr;

    fn profile() -> ProfileRef {
        ProfileRef {
            profile_kind: "test".to_string(),
            profile_version: 1,
        }
    }

    fn codec(family: &'static str) -> CodecRef {
        CodecRef { family, version: 1 }
    }

    fn acceptance() -> ErasedAcceptanceProfile {
        ErasedAcceptanceProfile::from_parts(
            profile(),
            SupportedCodecRegistry::new([codec("snapshot"), codec("event"), codec("receipt")])
                .unwrap(),
            true,
            true,
        )
    }

    async fn setup_repo_schema(pool: &SqlitePool) {
        sqlx::raw_sql(crate::ddl::SCHEMA)
            .execute(pool)
            .await
            .unwrap();
        crate::Database {
            pool: pool.clone(),
            path: String::new(),
        }
        .run_migrations()
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

    fn local_codec_owned(family: &str) -> LocalCodec {
        LocalCodec {
            family: family.to_string(),
            version: 1,
        }
    }

    async fn create_workflow(repo: &WorkflowRepository, workflow_id: WorkflowId) {
        repo.create_workflow_with_external_acceptance(&CreateWorkflowWithExternalAcceptance {
            workflow_id,
            profile: profile(),
            acceptance: acceptance(),
            target_scope: ScopeId::new(format!("scope-{workflow_id:?}")).unwrap(),
            idempotency_key: NonEmptyExternalKey::new(format!("key-{workflow_id:?}")).unwrap(),
            intent_fingerprint: "fp".to_string(),
            snapshot_codec: codec("snapshot"),
            snapshot_payload: vec![1],
            receipt_handle: vec![2],
            disposition_handle: vec![3],
            now: Timestamp(1),
        })
        .await
        .unwrap();
    }

    async fn install_effect_plan(
        repo: &WorkflowRepository,
        workflow_id: WorkflowId,
        effect_id: EffectId,
        capability: ExecutionCapability,
    ) {
        repo.commit_transition_plan(&CommitTransitionPlanCas {
            workflow_id,
            expected_version: Version(0),
            transition_id: TransitionId(1),
            generation: Generation(0),
            next_status: WorkflowStatus::Active,
            event_codec: local_codec_owned("event"),
            event_payload: vec![1],
            next_snapshot_codec: local_codec_owned("snapshot"),
            next_snapshot_payload: vec![2],
            committed_at: Timestamp(2),
            effects: vec![LocalEffectDecl {
                effect_id,
                declared_workflow_version: Version(1),
                family: "fam".to_string(),
                kind: "kind".to_string(),
                intent_codec: local_codec_owned("intent"),
                intent_payload: vec![9],
                generation: Generation(0),
                role: EffectRole::Required,
                capability,
                next_eligible_at: None,
                destructive_resource: None,
                status: EffectStatus::Eligible,
            }],
            dependencies: vec![],
            barriers: vec![],
            barrier_members: vec![],
            deliveries: vec![],
            schedules: vec![],
        })
        .await
        .unwrap();
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

    #[tokio::test]
    async fn begin_attempt_allows_single_winner_across_connections() {
        let (_dir, first, second) = open_repo_pair().await;
        create_workflow(&first, WorkflowId(31)).await;
        install_effect_plan(
            &first,
            WorkflowId(31),
            EffectId(1),
            ExecutionCapability::ReclaimableObservation,
        )
        .await;
        let a = BeginAttemptInput {
            workflow_id: WorkflowId(31),
            effect_id: EffectId(1),
            attempt_id: AttemptId(1),
            process_incarnation: ProcessIncarnation(7),
            now: Timestamp(3),
            lease_until: Some(LeaseExpiry(10)),
        };
        let b = BeginAttemptInput {
            attempt_id: AttemptId(2),
            ..a.clone()
        };
        let (left, right) = tokio::join!(first.begin_attempt(&a), second.begin_attempt(&b));
        let outcomes = [left.unwrap().outcome, right.unwrap().outcome];
        assert_eq!(
            outcomes
                .iter()
                .filter(|o| **o == ClaimOutcome::Started)
                .count(),
            1
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|o| **o == ClaimOutcome::AuthorityConflict)
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn begin_attempt_rejects_eligible_effect_on_terminal_workflow() {
        let (_dir, repo, _) = open_repo_pair().await;
        create_workflow(&repo, WorkflowId(311)).await;
        install_effect_plan(
            &repo,
            WorkflowId(311),
            EffectId(1),
            ExecutionCapability::SafelyRepeatable,
        )
        .await;
        sqlx::query("UPDATE workflows SET status = 'Completed' WHERE workflow_id = ?1")
            .bind(311_i64)
            .execute(&repo.pool)
            .await
            .unwrap();

        let result = repo
            .begin_attempt(&BeginAttemptInput {
                workflow_id: WorkflowId(311),
                effect_id: EffectId(1),
                attempt_id: AttemptId(1),
                process_incarnation: ProcessIncarnation(7),
                now: Timestamp(3),
                lease_until: None,
            })
            .await
            .unwrap();

        assert_eq!(result.outcome, ClaimOutcome::Ineligible);
        assert!(repo
            .list_attempts(WorkflowId(311), EffectId(1))
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn accept_receipt_allows_single_winner_across_connections() {
        let (_dir, first, second) = open_repo_pair().await;
        create_workflow(&first, WorkflowId(32)).await;
        install_effect_plan(
            &first,
            WorkflowId(32),
            EffectId(1),
            ExecutionCapability::SafelyRepeatable,
        )
        .await;
        let begun = first
            .begin_attempt(&BeginAttemptInput {
                workflow_id: WorkflowId(32),
                effect_id: EffectId(1),
                attempt_id: AttemptId(1),
                process_incarnation: ProcessIncarnation(1),
                now: Timestamp(3),
                lease_until: None,
            })
            .await
            .unwrap();
        let authority = begun.authority.unwrap();
        let a = AcceptReceiptInput {
            authority: authority.clone(),
            receipt_id: ReceiptId(1),
            delivery_id: DeliveryId(1),
            attempt_id: Some(AttemptId(1)),
            origin: ReceiptOrigin::Execution,
            receipt_codec: local_codec_owned("receipt"),
            receipt_payload: vec![1],
            receipt_event_codec: local_codec_owned("event"),
            receipt_event_payload: vec![2],
            receipt_event_requires_runtime_acceptance: false,
            request_runtime_acceptance_for_cancellation: false,
        };
        let b = AcceptReceiptInput {
            receipt_id: ReceiptId(2),
            delivery_id: DeliveryId(2),
            ..a.clone()
        };
        let (left, right) = tokio::join!(first.accept_receipt(&a), second.accept_receipt(&b));
        let outcomes = [left.unwrap().outcome, right.unwrap().outcome];
        assert_eq!(
            outcomes
                .iter()
                .filter(|o| **o == AuthorityOutcome::Authorized)
                .count(),
            1
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|o| **o == AuthorityOutcome::StaleAuthority)
                .count(),
            1
        );
        assert_eq!(first.list_receipts(WorkflowId(32)).await.unwrap().len(), 1);
        assert_eq!(
            first.list_deliveries(WorkflowId(32)).await.unwrap().len(),
            1
        );
    }

    #[tokio::test]
    async fn accept_receipt_rejects_unsupported_persisted_codec() {
        let (_dir, repo, _) = open_repo_pair().await;
        create_workflow(&repo, WorkflowId(323)).await;
        install_effect_plan(
            &repo,
            WorkflowId(323),
            EffectId(1),
            ExecutionCapability::SafelyRepeatable,
        )
        .await;
        let authority = repo
            .begin_attempt(&BeginAttemptInput {
                workflow_id: WorkflowId(323),
                effect_id: EffectId(1),
                attempt_id: AttemptId(1),
                process_incarnation: ProcessIncarnation(1),
                now: Timestamp(3),
                lease_until: None,
            })
            .await
            .unwrap()
            .authority
            .unwrap();

        let result = repo
            .accept_receipt(&AcceptReceiptInput {
                authority,
                receipt_id: ReceiptId(1),
                delivery_id: DeliveryId(1),
                attempt_id: Some(AttemptId(1)),
                origin: ReceiptOrigin::Execution,
                receipt_codec: local_codec_owned("unsupported"),
                receipt_payload: vec![1],
                receipt_event_codec: local_codec_owned("event"),
                receipt_event_payload: vec![2],
                receipt_event_requires_runtime_acceptance: false,
                request_runtime_acceptance_for_cancellation: false,
            })
            .await
            .unwrap();

        assert_eq!(result.outcome, AuthorityOutcome::StaleAuthority);
        assert!(repo
            .list_receipts(WorkflowId(323))
            .await
            .unwrap()
            .is_empty());
        assert!(repo
            .list_deliveries(WorkflowId(323))
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn record_observation_rejects_exact_lease_expiry_boundary() {
        let (_dir, repo, _) = open_repo_pair().await;
        create_workflow(&repo, WorkflowId(321)).await;
        install_effect_plan(
            &repo,
            WorkflowId(321),
            EffectId(1),
            ExecutionCapability::ReclaimableObservation,
        )
        .await;
        let begun = repo
            .begin_attempt(&BeginAttemptInput {
                workflow_id: WorkflowId(321),
                effect_id: EffectId(1),
                attempt_id: AttemptId(1),
                process_incarnation: ProcessIncarnation(9),
                now: Timestamp(3),
                lease_until: Some(LeaseExpiry(10)),
            })
            .await
            .unwrap();
        let authority = begun.authority.unwrap();

        let result = repo
            .record_observation(&RecordObservationInput {
                authority: authority.clone(),
                observation_id: 1,
                now: Timestamp(10),
                observed_at: Timestamp(9),
                observation_codec: local_codec_owned("observation"),
                observation_payload: vec![1],
            })
            .await
            .unwrap();
        assert_eq!(result.outcome, AuthorityOutcome::StaleAuthority);

        assert!(repo
            .list_authoritative_observations(WorkflowId(321), EffectId(1))
            .await
            .unwrap()
            .is_empty());
        assert_eq!(
            repo.list_stale_observations(WorkflowId(321), EffectId(1))
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn record_observation_accepts_before_lease_expiry_boundary() {
        let (_dir, repo, _) = open_repo_pair().await;
        create_workflow(&repo, WorkflowId(322)).await;
        install_effect_plan(
            &repo,
            WorkflowId(322),
            EffectId(1),
            ExecutionCapability::ReclaimableObservation,
        )
        .await;
        let begun = repo
            .begin_attempt(&BeginAttemptInput {
                workflow_id: WorkflowId(322),
                effect_id: EffectId(1),
                attempt_id: AttemptId(1),
                process_incarnation: ProcessIncarnation(9),
                now: Timestamp(3),
                lease_until: Some(LeaseExpiry(10)),
            })
            .await
            .unwrap();
        let authority = begun.authority.unwrap();

        let result = repo
            .record_observation(&RecordObservationInput {
                authority: authority.clone(),
                observation_id: 1,
                now: Timestamp(9),
                observed_at: Timestamp(9),
                observation_codec: local_codec_owned("observation"),
                observation_payload: vec![1],
            })
            .await
            .unwrap();
        assert_eq!(result.outcome, AuthorityOutcome::Authorized);
        assert!(result
            .observation
            .as_ref()
            .is_some_and(|obs| obs.authoritative));
    }

    #[tokio::test]
    async fn exact_delivery_acceptance_is_atomic_and_duplicate_safe() {
        let (_dir, repo, second) = open_repo_pair().await;
        create_workflow(&repo, WorkflowId(33)).await;
        repo.commit_transition_plan(&CommitTransitionPlanCas {
            workflow_id: WorkflowId(33),
            expected_version: Version(0),
            transition_id: TransitionId(1),
            generation: Generation(0),
            next_status: WorkflowStatus::Active,
            event_codec: local_codec_owned("event"),
            event_payload: vec![1],
            next_snapshot_codec: local_codec_owned("snapshot"),
            next_snapshot_payload: vec![2],
            committed_at: Timestamp(2),
            effects: vec![],
            dependencies: vec![],
            barriers: vec![LocalBarrierDecl {
                barrier_id: BarrierId(1),
                status: BarrierStatus::Waiting,
                reducer_event_codec: local_codec_owned("event"),
                reducer_event_payload: vec![0],
            }],
            barrier_members: vec![],
            deliveries: vec![LocalDeliveryDecl {
                delivery_id: DeliveryId(1),
                effect_id: None,
                barrier_id: Some(BarrierId(1)),
                consumer_kind: "reducer".to_string(),
                event_codec: local_codec_owned("event"),
                payload_kind: LocalDeliveryPayloadKind::Barrier,
                payload_blob: vec![1],
                requires_runtime_acceptance: false,
                runtime_acceptance_status: None,
            }],
            schedules: vec![],
        })
        .await
        .unwrap();
        let first = AcceptOrSuppressDeliveryInput {
            workflow_id: WorkflowId(33),
            expected_version: Version(1),
            transition_id: TransitionId(2),
            generation: Generation(0),
            next_status: WorkflowStatus::Active,
            event_codec: local_codec_owned("event"),
            event_payload: vec![3],
            next_snapshot_codec: local_codec_owned("snapshot"),
            next_snapshot_payload: vec![4],
            committed_at: Timestamp(4),
            accept_delivery_ids: vec![DeliveryId(1)],
            suppress_delivery_ids: vec![],
            suppression_reason: SuppressionReason::ReducerTerminal,
        };
        assert_eq!(
            repo.accept_or_suppress_deliveries_exact(&first)
                .await
                .unwrap(),
            CommitOutcome::Committed
        );
        assert_eq!(
            second
                .accept_or_suppress_deliveries_exact(&AcceptOrSuppressDeliveryInput {
                    transition_id: TransitionId(3),
                    ..first.clone()
                })
                .await
                .unwrap(),
            CommitOutcome::VersionConflict
        );
    }

    fn valid_direct_turn_payload(message_id: &str) -> Vec<u8> {
        phoenix_core::domain::sm_event::PreparedDirectTurnPayload::from_parts(
            phoenix_core::domain::sm_event::SubmittedDirectTurnIdentity {
                text: "gate".to_string(),
                images: Vec::new(),
                files: Vec::new(),
                message_id: message_id.to_string(),
                user_agent: None,
                skill_invocation: None,
                expansion_policy: phoenix_core::domain::sm_event::SubmittedDirectTurnExpansionPolicy::ExpandReferences,
            },
            phoenix_core::domain::sm_event::PreparedDirectTurnDelivery {
                text: "gate".to_string(),
                llm_text: None,
                images: Vec::new(),
                files: Vec::new(),
                user_agent: None,
                skill_invocation: None,
            },
        )
        .to_exact_bytes()
        .unwrap()
    }

    #[tokio::test]
    async fn generic_delivery_resolution_rejects_direct_turn_workflow() {
        let (_dir, repo, _) = open_repo_pair().await;
        crate::Database {
            pool: repo.pool.clone(),
            path: String::new(),
        }
        .create_conversation("conv-a", "A", "/tmp", true, None, None)
        .await
        .unwrap();
        let accepted_at = Timestamp(7);
        let created = repo
            .accept_authoritative_turn(&AcceptAuthoritativeTurn {
                client_key: ClientTurnKey::new("gate-delivery").unwrap(),
                prepared: PreparedTurn::from_exact_payload(
                    &ConversationAuthority("conv-a".to_string()),
                    valid_direct_turn_payload("gate-delivery"),
                ),
                disposition: AcceptedDisposition::Runtime,
                accepted_at,
            })
            .await
            .unwrap();
        let TurnOutcome::Created { turn_id, .. } = created.outcome else {
            panic!("expected created turn")
        };
        let workflow_id = repo.workflow_id_for_turn(turn_id).await.unwrap().unwrap();
        assert_eq!(
            repo.accept_or_suppress_deliveries_exact(&AcceptOrSuppressDeliveryInput {
                workflow_id,
                expected_version: Version(1),
                transition_id: TransitionId(2),
                generation: Generation(0),
                next_status: WorkflowStatus::Active,
                event_codec: local_codec_owned("event"),
                event_payload: vec![1],
                next_snapshot_codec: local_codec_owned("snapshot"),
                next_snapshot_payload: vec![2],
                committed_at: Timestamp(8),
                accept_delivery_ids: vec![],
                suppress_delivery_ids: vec![DeliveryId(1)],
                suppression_reason: SuppressionReason::ReducerTerminal,
            })
            .await
            .unwrap(),
            CommitOutcome::InvalidPlan
        );
    }

    #[tokio::test]
    async fn generic_head_cas_rejects_direct_turn_workflow() {
        let (_dir, repo, _) = open_repo_pair().await;
        crate::Database {
            pool: repo.pool.clone(),
            path: String::new(),
        }
        .create_conversation("conv-a", "A", "/tmp", true, None, None)
        .await
        .unwrap();
        let accepted_at = Timestamp(7);
        let created = repo
            .accept_authoritative_turn(&AcceptAuthoritativeTurn {
                client_key: ClientTurnKey::new("gate-head").unwrap(),
                prepared: PreparedTurn::from_exact_payload(
                    &ConversationAuthority("conv-a".to_string()),
                    valid_direct_turn_payload("gate-head"),
                ),
                disposition: AcceptedDisposition::Runtime,
                accepted_at,
            })
            .await
            .unwrap();
        let TurnOutcome::Created { turn_id, .. } = created.outcome else {
            panic!("expected created turn")
        };
        let workflow_id = repo.workflow_id_for_turn(turn_id).await.unwrap().unwrap();
        assert_eq!(
            repo.commit_transition_head_cas(&CommitTransitionHeadCas {
                workflow_id,
                expected_version: Version(1),
                transition_id: TransitionId(99),
                generation: Generation(0),
                next_status: WorkflowStatus::Active,
                event_codec: codec("event"),
                event_payload: vec![1],
                next_snapshot_codec: codec("snapshot"),
                next_snapshot_payload: vec![2],
                committed_at: Timestamp(9),
            })
            .await
            .unwrap(),
            CommitOutcome::InvalidPlan
        );
    }

    #[tokio::test]
    async fn schedule_stale_completion_is_noop() {
        let (_dir, repo, _) = open_repo_pair().await;
        create_workflow(&repo, WorkflowId(34)).await;
        repo.commit_transition_plan(&CommitTransitionPlanCas {
            workflow_id: WorkflowId(34),
            expected_version: Version(0),
            transition_id: TransitionId(1),
            generation: Generation(0),
            next_status: WorkflowStatus::Active,
            event_codec: local_codec_owned("event"),
            event_payload: vec![1],
            next_snapshot_codec: local_codec_owned("snapshot"),
            next_snapshot_payload: vec![2],
            committed_at: Timestamp(2),
            effects: vec![],
            dependencies: vec![],
            barriers: vec![],
            barrier_members: vec![],
            deliveries: vec![],
            schedules: vec![LocalScheduleDecl {
                schedule_id: ScheduleId(1),
                policy: SchedulePolicy::CoalesceLatest,
                key: "cron".to_string(),
                status: ScheduleStatus::Idle,
                next_eligible_at: Timestamp(5),
                active_effect_id: None,
                due_occurrence: None,
                active_occurrence: None,
            }],
        })
        .await
        .unwrap();
        let occ = repo
            .reconcile_schedule_due_exact(&ReconcileScheduleDueInput {
                workflow_id: WorkflowId(34),
                schedule_id: ScheduleId(1),
                now: Timestamp(10),
                new_occurrence_id: ScheduleOccurrenceId(1),
            })
            .await
            .unwrap()
            .unwrap();
        assert!(!repo
            .start_schedule_occurrence_exact(&StartScheduleOccurrenceInput {
                workflow_id: WorkflowId(34),
                occurrence: ScheduleOccurrence {
                    generation: Generation(occ.generation.0 + 1),
                    ..occ
                },
                active_effect_id: None
            })
            .await
            .unwrap());
        assert!(!repo
            .start_schedule_occurrence_exact(&StartScheduleOccurrenceInput {
                workflow_id: WorkflowId(34),
                occurrence: ScheduleOccurrence {
                    due_at: Timestamp(occ.due_at.0 + 1),
                    ..occ
                },
                active_effect_id: None
            })
            .await
            .unwrap());
        assert!(repo
            .start_schedule_occurrence_exact(&StartScheduleOccurrenceInput {
                workflow_id: WorkflowId(34),
                occurrence: occ,
                active_effect_id: None
            })
            .await
            .unwrap());
        assert!(repo
            .complete_schedule_occurrence_exact(&CompleteScheduleOccurrenceInput {
                workflow_id: WorkflowId(34),
                occurrence: occ,
                next_eligible_at: Timestamp(20)
            })
            .await
            .unwrap());
        assert!(!repo
            .complete_schedule_occurrence_exact(&CompleteScheduleOccurrenceInput {
                workflow_id: WorkflowId(34),
                occurrence: occ,
                next_eligible_at: Timestamp(30)
            })
            .await
            .unwrap());
        assert_eq!(
            repo.get_schedule(WorkflowId(34), ScheduleId(1))
                .await
                .unwrap()
                .unwrap()
                .next_eligible_at,
            Timestamp(20)
        );
    }
}
