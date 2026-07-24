use super::{
    is_sqlite_busy_retryable, local_codec, parse_attempt_status, parse_delivery_payload_kind,
    parse_delivery_status, parse_receipt_origin, parse_runtime_acceptance_status,
    parse_suppression_reason, to_i64, to_u32, to_u64, AcceptReceiptInput, AttemptId,
    AuthorityOutcome, BeginAttemptInput, BeginAttemptResult, CommitOutcome,
    CommitTransitionPlanCas, CreateWorkflowWithExternalAcceptance, DbError, DbResult, DeliveryId,
    DeliveryResolutionDecision, DeliveryResolutionPlan, EffectId, Generation, LeaseExpiry,
    LocalAttemptAuthority, LocalAttemptRecord, LocalCodec, LocalDeliveryRecord, LocalEffectDecl,
    LocalReceiptRecord, LocalReclaimableLease, ProcessIncarnation, ReceiptId, ReceiptOrigin,
    SuppressionReason, Timestamp, TransitionId, Version, WorkflowId, WorkflowRepository,
    WorkflowSequenceName, WorkflowStatus, WorkflowTx,
};
use phoenix_workflow::llm_profile;
use phoenix_workflow::llm_profile::{
    CompleteLlmResponse, LlmEffectKey, PreparedLlmRequest, TopLevelLlmSnapshot, TopLevelTurnRef,
};
use phoenix_workflow::{CodecRef, EffectRole, EffectStatus, ExecutionCapability};
use sqlx::SqlitePool;

#[cfg(test)]
use phoenix_workflow::ClaimOutcome;
use serde::{Deserialize, Serialize};
use sqlx::Row;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DirectTurnCommittedOutcome {
    PendingRuntime,
    RuntimeAccepted,
    QueuedSteering,
    CancelledSteering,
}

#[derive(Debug, Clone)]
pub enum DirectTurnInitialOutcome {
    PendingRuntime,
    RuntimeAccepted,
    QueuedSteering {
        entry: Box<phoenix_core::domain::sm_event::SteerEntry>,
    },
}

impl DirectTurnInitialOutcome {
    fn committed_outcome(&self) -> DirectTurnCommittedOutcome {
        match self {
            Self::PendingRuntime => DirectTurnCommittedOutcome::PendingRuntime,
            Self::RuntimeAccepted => DirectTurnCommittedOutcome::RuntimeAccepted,
            Self::QueuedSteering { .. } => DirectTurnCommittedOutcome::QueuedSteering,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DirectTurnAcceptanceInput {
    pub initial_outcome: DirectTurnInitialOutcome,

    pub conversation_id: String,
    pub client_message_id: String,
    pub prepared_fingerprint: String,
    pub prepared_payload: String,
    pub accepted_at: Timestamp,
    pub snapshot: TopLevelLlmSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectTurnAcceptanceRecord {
    pub workflow_id: WorkflowId,
    pub conversation_id: String,
    pub client_message_id: String,
    pub prepared_fingerprint: String,
    pub prepared_payload: String,
    pub committed_outcome: DirectTurnCommittedOutcome,
    pub accepted_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirectTurnAcceptanceOutcome {
    Created(DirectTurnAcceptanceRecord),
    Replayed(DirectTurnAcceptanceRecord),
    RetryablePersistence,
    Conflict,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectTurnRuntimeAdmissionInput {
    pub workflow_id: WorkflowId,
    pub conversation_id: String,
    pub client_message_id: String,
    pub generation: Generation,
    pub disposition: DirectTurnCommittedOutcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectTurnRuntimeAdmissionOutcome {
    Committed,
    ExactReplay,
    Conflict,
    RetryablePersistence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingDirectTurnRuntimeAdmission {
    pub workflow_id: WorkflowId,
    pub conversation_id: String,
    pub client_message_id: String,
    pub generation: Generation,
}

#[derive(Debug, Clone)]
pub struct PersistDirectTurnRuntimeAcceptanceInput {
    pub admission: DirectTurnRuntimeAdmissionInput,
    pub message: phoenix_core::domain::db_schema::Message,
    pub next_state: phoenix_core::domain::sm_state::ConvState,
    pub state_updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopLevelLlmWorkflowRecord {
    pub workflow_id: WorkflowId,
    pub conversation_id: String,
    pub accepted_turn_id: String,
    pub turn_generation: Generation,
    pub accepted_assistant_message_id: Option<String>,
    pub stopped_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopLevelLlmPreparedRequestRecord {
    pub workflow_id: WorkflowId,
    pub effect_id: EffectId,
    pub call_ordinal: u64,
    pub codec_version: u32,
    pub request_fingerprint: String,
    pub provider: String,
    pub model: String,
    pub backend: String,
    pub request_aggregate: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrepareTopLevelLlmRequestInput {
    pub workflow_id: WorkflowId,
    pub effect_id: EffectId,
    pub expected_version: Version,
    pub transition_id: TransitionId,
    pub generation: Generation,
    pub committed_at: Timestamp,
    pub snapshot: TopLevelLlmSnapshot,
    pub prepared_request: PreparedLlmRequest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrepareAndBeginTopLevelLlmInput {
    pub workflow_id: WorkflowId,
    pub committed_at: Timestamp,
    pub process_incarnation: ProcessIncarnation,
    pub prepared_request: PreparedLlmRequest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedTopLevelLlmAttempt {
    pub prepared_request: TopLevelLlmPreparedRequestRecord,
    pub authority: LocalAttemptAuthority,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordTopLevelLlmFailureInput {
    pub authority: LocalAttemptAuthority,
    pub observed_at: Timestamp,
    pub outcome_payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoverTopLevelLlmAttempt {
    pub workflow: TopLevelLlmWorkflowRecord,
    pub prepared_request: TopLevelLlmPreparedRequestRecord,
    pub attempt: LocalAttemptRecord,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptCompleteLlmResponseInput {
    pub authority: LocalAttemptAuthority,
    pub delivery_id: Option<DeliveryId>,
    pub receipt_id: Option<ReceiptId>,
    pub response: CompleteLlmResponse,
    pub provider_request_id: Option<String>,
    pub tool_intents: Vec<ToolIntentRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmResponseReceiptRecord {
    pub workflow_id: WorkflowId,
    pub receipt_id: ReceiptId,
    pub effect_id: EffectId,
    pub codec_version: u32,
    pub response_fingerprint: String,
    pub response_aggregate: String,
    pub provider_request_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompleteLlmResponsePersistenceOutcome {
    Accepted,
    ExactReplay,
    StaleAuthority,
    RetryablePersistence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptCompleteLlmResponseResult {
    pub outcome: CompleteLlmResponsePersistenceOutcome,
    pub receipt: Option<LocalReceiptRecord>,
    pub delivery: Option<LocalDeliveryRecord>,
    pub llm_receipt: Option<LlmResponseReceiptRecord>,
    pub tool_intents: Vec<ToolIntentRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolKindRecord {
    Function,
    Custom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolIntentStatus {
    PendingAcceptance,
    Owed,
    ExecutionMayHaveBegun,
    Completed,
    Interrupted,
    Suppressed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolIntentRecord {
    pub intent_ordinal: u32,
    pub status: ToolIntentStatus,
    pub tool_name: String,
    pub tool_kind: ToolKindRecord,
    pub tool_use_id: String,
    pub arguments_json: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolIntentTransitionInput {
    pub workflow_id: WorkflowId,
    pub receipt_id: ReceiptId,
    pub intent_ordinal: u32,
    pub generation: Generation,
    pub from: ToolIntentStatus,
    pub to: ToolIntentStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolIntentTransitionOutcome {
    Committed,
    ExactReplay,
    Conflict,
    RetryablePersistence,
}
#[derive(Debug, Clone)]
pub enum PersistDirectTurnRuntimeAcceptanceOutcome {
    Committed(Box<crate::Message>),
    ExactReplay,
    Conflict,
    RetryablePersistence,
}

#[derive(Debug, Clone)]
pub enum PersistQueuedSteeringMessageOutcome {
    Committed(Box<crate::Message>),
    ExactReplay,
    LegacyQueueEntry,
    Conflict,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwedTopLevelLlmReceipt {
    pub workflow: TopLevelLlmWorkflowRecord,
    pub prepared_request: TopLevelLlmPreparedRequestRecord,
    pub receipt: LocalReceiptRecord,
    pub llm_receipt: LlmResponseReceiptRecord,
    pub delivery: LocalDeliveryRecord,
    pub tool_intents: Vec<ToolIntentRecord>,
}

#[derive(Debug, Clone)]
pub enum AcceptedTopLevelLlmProduct {
    PersistedAssistant(Box<phoenix_core::domain::db_schema::Message>),
    PersistedCheckpoint {
        assistant: Box<phoenix_core::domain::db_schema::Message>,
        tool_results: Vec<phoenix_core::domain::db_schema::Message>,
    },
    StateCheckpoint {
        conversation_id: String,
    },
}

#[derive(Debug, Clone)]
pub struct AcceptTopLevelLlmProductInput {
    pub workflow_id: WorkflowId,
    pub delivery_id: DeliveryId,
    pub receipt_id: ReceiptId,
    pub product: AcceptedTopLevelLlmProduct,
    pub next_state: phoenix_core::domain::sm_state::ConvState,
    pub state_updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcceptTopLevelLlmProductOutcome {
    Committed,
    ExactReplay,
    StaleAuthority,
    RetryablePersistence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StopTopLevelLlmInput {
    pub workflow_id: WorkflowId,
    pub stopped_at: Timestamp,
    pub expected_version: Version,
    pub transition_id: TransitionId,
    pub generation: Generation,
    pub next_status: WorkflowStatus,
    pub event_payload: Vec<u8>,
    pub next_snapshot: TopLevelLlmSnapshot,
    pub suppression_reason: SuppressionReason,
}

impl WorkflowRepository {
    pub async fn cancel_queued_steering(
        &self,
        conversation_id: &str,
        message_id: &str,
    ) -> DbResult<bool> {
        let mut tx = self.pool.begin().await?;
        let updated = sqlx::query(
            "INSERT OR IGNORE INTO direct_turn_steering_cancellations (workflow_id, cancelled_at)
             SELECT workflow_id, unixepoch('subsec') * 1000
             FROM direct_turn_acceptances
             WHERE conversation_id = ?1 AND client_message_id = ?2
               AND committed_outcome = 'QueuedSteering'",
        )
        .bind(conversation_id)
        .bind(message_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        sqlx::query(
            "DELETE FROM steering_messages
             WHERE conversation_id = ?1 AND message_id = ?2",
        )
        .bind(conversation_id)
        .bind(message_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(updated > 0)
    }

    pub async fn load_direct_turn_materialized_message_id(
        &self,
        conversation_id: &str,
        client_message_id: &str,
    ) -> DbResult<Option<String>> {
        Ok(sqlx::query_scalar(
            "SELECT materialized_message_id FROM direct_turn_acceptances
             WHERE conversation_id = ?1 AND client_message_id = ?2",
        )
        .bind(conversation_id)
        .bind(client_message_id)
        .fetch_optional(&self.pool)
        .await?
        .flatten())
    }

    pub async fn load_direct_turn_acceptance(
        &self,
        conversation_id: &str,
        client_message_id: &str,
    ) -> DbResult<Option<DirectTurnAcceptanceRecord>> {
        load_direct_turn_acceptance(&self.pool, conversation_id, client_message_id).await
    }

    pub async fn accept_direct_turn(
        &self,
        input: &DirectTurnAcceptanceInput,
    ) -> DbResult<DirectTurnAcceptanceOutcome> {
        match self.accept_direct_turn_once(input).await {
            Err(DbError::Sqlx(sqlx::Error::Database(error)))
                if is_sqlite_busy_retryable(error.as_ref()) =>
            {
                Ok(DirectTurnAcceptanceOutcome::RetryablePersistence)
            }
            result => result,
        }
    }

    async fn accept_direct_turn_once(
        &self,
        input: &DirectTurnAcceptanceInput,
    ) -> DbResult<DirectTurnAcceptanceOutcome> {
        let mut tx = self.begin_tx().await?;
        let existing = tx
            .fetch_direct_turn_acceptance(&input.conversation_id, &input.client_message_id)
            .await?;
        if let Some(existing) = existing {
            tx.commit().await?;
            return Ok(classify_direct_turn_replay(existing, input));
        }
        let workflow_id = allocate_global_workflow_id(&mut tx).await?;
        let create = CreateWorkflowWithExternalAcceptance {
            workflow_id,
            profile: llm_profile::profile(),
            acceptance: llm_profile::acceptance_profile().erase(),
            target_scope: phoenix_workflow::ScopeId::new(format!(
                "conversation:{}",
                input.conversation_id
            ))
            .ok_or_else(|| DbError::Serialization("empty conversation scope".to_string()))?,
            idempotency_key: phoenix_workflow::NonEmptyExternalKey::new(
                input.client_message_id.clone(),
            )
            .ok_or_else(|| DbError::Serialization("empty client_message_id".to_string()))?,
            intent_fingerprint: input.prepared_fingerprint.clone(),
            snapshot_codec: llm_profile::snapshot_codec(),
            snapshot_payload: serde_json::to_vec(&input.snapshot)
                .map_err(|e| DbError::Serialization(e.to_string()))?,
            receipt_handle: input.client_message_id.as_bytes().to_vec(),
            disposition_handle: input.client_message_id.as_bytes().to_vec(),
            now: input.accepted_at,
        };
        tx.insert_workflow(&create).await?;
        let acceptance_insert = sqlx::query("INSERT INTO direct_turn_acceptances (conversation_id, client_message_id, workflow_id, prepared_fingerprint, prepared_payload, committed_outcome, accepted_at, live_slot) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)")
            .bind(&input.conversation_id)
            .bind(&input.client_message_id)
            .bind(to_i64(workflow_id.0, "workflow_id")?)
            .bind(&input.prepared_fingerprint)
            .bind(&input.prepared_payload)
            .bind(direct_turn_outcome_to_str(
                &input.initial_outcome.committed_outcome(),
            ))
            .bind(to_i64(input.accepted_at.0, "accepted_at")?)
            .bind(if matches!(
                input.initial_outcome,
                DirectTurnInitialOutcome::PendingRuntime
                    | DirectTurnInitialOutcome::RuntimeAccepted
            ) {
                Some(1_i64)
            } else {
                None
            })
            .execute(&mut *tx.tx)
            .await;
        if let Err(error) = acceptance_insert {
            tx.rollback().await?;
            if error.as_database_error().is_some() {
                if let Some(existing) = load_direct_turn_acceptance(
                    &self.pool,
                    &input.conversation_id,
                    &input.client_message_id,
                )
                .await?
                {
                    return Ok(classify_direct_turn_replay(existing, input));
                }
                let live_slot_taken = sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*) FROM direct_turn_acceptances
                     WHERE conversation_id = ?1
                       AND committed_outcome IN ('PendingRuntime', 'RuntimeAccepted')",
                )
                .bind(&input.conversation_id)
                .fetch_one(&self.pool)
                .await?
                    != 0;
                if live_slot_taken {
                    return Ok(DirectTurnAcceptanceOutcome::Conflict);
                }
            }
            return Err(error.into());
        }
        sqlx::query("INSERT INTO top_level_llm_workflows (workflow_id, turn_generation, accepted_assistant_message_id, stopped_at) VALUES (?1, ?2, ?3, ?4)")
            .bind(to_i64(workflow_id.0, "workflow_id")?)
            .bind(to_i64(input.snapshot.turn_ref.generation, "turn_generation")?)
            .bind(input.snapshot.accepted_assistant_message_id.as_deref())
            .bind(input.snapshot.stopped_at.map(|v| i64::try_from(v).unwrap()))
            .execute(&mut *tx.tx)
            .await?;
        if let DirectTurnInitialOutcome::QueuedSteering { entry } = &input.initial_outcome {
            let next_ordinal = sqlx::query_scalar::<_, i64>(
                "SELECT COALESCE(MAX(ordinal), -1) + 1 FROM steering_messages
                 WHERE conversation_id = ?1",
            )
            .bind(&input.conversation_id)
            .fetch_one(&mut *tx.tx)
            .await?;
            super::super::insert_steering_entry_tx(
                &mut tx.tx,
                &input.conversation_id,
                next_ordinal,
                entry,
            )
            .await?;
        }
        tx.commit().await?;
        Ok(DirectTurnAcceptanceOutcome::Created(
            DirectTurnAcceptanceRecord {
                workflow_id,
                conversation_id: input.conversation_id.clone(),
                client_message_id: input.client_message_id.clone(),
                prepared_fingerprint: input.prepared_fingerprint.clone(),
                prepared_payload: input.prepared_payload.clone(),
                committed_outcome: input.initial_outcome.committed_outcome(),
                accepted_at: input.accepted_at,
            },
        ))
    }

    pub async fn commit_direct_turn_runtime_admission(
        &self,
        input: &DirectTurnRuntimeAdmissionInput,
    ) -> DbResult<DirectTurnRuntimeAdmissionOutcome> {
        if input.disposition == DirectTurnCommittedOutcome::PendingRuntime {
            return Ok(DirectTurnRuntimeAdmissionOutcome::Conflict);
        }
        match self.commit_direct_turn_runtime_admission_once(input).await {
            Err(DbError::Sqlx(sqlx::Error::Database(error)))
                if is_sqlite_busy_retryable(error.as_ref()) =>
            {
                Ok(DirectTurnRuntimeAdmissionOutcome::RetryablePersistence)
            }
            result => result,
        }
    }

    async fn commit_direct_turn_runtime_admission_once(
        &self,
        input: &DirectTurnRuntimeAdmissionInput,
    ) -> DbResult<DirectTurnRuntimeAdmissionOutcome> {
        let mut tx = self.begin_tx().await?;
        let updated = sqlx::query(
            "UPDATE direct_turn_acceptances
             SET committed_outcome = ?5,
                 live_slot = CASE WHEN ?5 = 'RuntimeAccepted' THEN 1 ELSE NULL END
             WHERE workflow_id = ?1
               AND conversation_id = ?2
               AND client_message_id = ?3
               AND committed_outcome = 'PendingRuntime'
               AND EXISTS (
                   SELECT 1 FROM top_level_llm_workflows w
                   WHERE w.workflow_id = direct_turn_acceptances.workflow_id
                     AND w.turn_generation = ?4
                     AND w.stopped_at IS NULL
               )",
        )
        .bind(to_i64(input.workflow_id.0, "workflow_id")?)
        .bind(&input.conversation_id)
        .bind(&input.client_message_id)
        .bind(to_i64(input.generation.0, "generation")?)
        .bind(direct_turn_outcome_to_str(&input.disposition))
        .execute(&mut *tx.tx)
        .await?;
        if updated.rows_affected() == 1 {
            tx.commit().await?;
            return Ok(DirectTurnRuntimeAdmissionOutcome::Committed);
        }
        let existing = tx
            .fetch_direct_turn_acceptance(&input.conversation_id, &input.client_message_id)
            .await?;
        let generation = sqlx::query_scalar::<_, i64>(
            "SELECT turn_generation FROM top_level_llm_workflows WHERE workflow_id = ?1",
        )
        .bind(to_i64(input.workflow_id.0, "workflow_id")?)
        .fetch_optional(&mut *tx.tx)
        .await?;
        tx.rollback().await?;
        let exact_replay = existing.is_some_and(|existing| {
            existing.workflow_id == input.workflow_id
                && existing.committed_outcome == input.disposition
        }) && generation
            .and_then(|value| u64::try_from(value).ok())
            .is_some_and(|generation| generation == input.generation.0);
        Ok(if exact_replay {
            DirectTurnRuntimeAdmissionOutcome::ExactReplay
        } else {
            DirectTurnRuntimeAdmissionOutcome::Conflict
        })
    }

    pub async fn consume_queued_steering_batch(
        &self,
        conversation_id: &str,
        owner_message_id: &str,
        drained_message_ids: &[String],
    ) -> DbResult<()> {
        let mut tx = self.begin_tx().await?;
        let updated = sqlx::query(
            "UPDATE direct_turn_acceptances
             SET committed_outcome = 'RuntimeAccepted', live_slot = 1
             WHERE conversation_id = ?1 AND client_message_id = ?2
               AND committed_outcome = 'QueuedSteering'
               AND NOT EXISTS (
                   SELECT 1 FROM direct_turn_steering_cancellations c
                   WHERE c.workflow_id = direct_turn_acceptances.workflow_id
               )
               AND EXISTS (
                   SELECT 1 FROM top_level_llm_workflows w
                   WHERE w.workflow_id = direct_turn_acceptances.workflow_id
                     AND w.stopped_at IS NULL
               )",
        )
        .bind(conversation_id)
        .bind(owner_message_id)
        .execute(&mut *tx.tx)
        .await?;
        if updated.rows_affected() == 0 {
            let replay = sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM direct_turn_acceptances a
                 JOIN top_level_llm_workflows w ON w.workflow_id = a.workflow_id
                 WHERE a.conversation_id = ?1 AND a.client_message_id = ?2
                   AND a.committed_outcome = 'RuntimeAccepted'
                   AND NOT EXISTS (
                       SELECT 1 FROM direct_turn_steering_cancellations c
                       WHERE c.workflow_id = a.workflow_id
                   )
                   AND w.stopped_at IS NULL",
            )
            .bind(conversation_id)
            .bind(owner_message_id)
            .fetch_one(&mut *tx.tx)
            .await?
                == 1;
            if !replay {
                let legacy_without_acceptance = sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*) FROM direct_turn_acceptances
                     WHERE conversation_id = ?1 AND client_message_id = ?2",
                )
                .bind(conversation_id)
                .bind(owner_message_id)
                .fetch_one(&mut *tx.tx)
                .await?
                    == 0;
                if !legacy_without_acceptance {
                    tx.rollback().await?;
                    return Err(DbError::Serialization(format!(
                        "queued steering turn {owner_message_id} cannot own the drained batch"
                    )));
                }
            }
        }
        for message_id in drained_message_ids {
            if message_id != owner_message_id {
                sqlx::query(
                    "UPDATE direct_turn_acceptances
                     SET committed_outcome = 'RuntimeAccepted'
                     WHERE conversation_id = ?1 AND client_message_id = ?2
                       AND committed_outcome = 'QueuedSteering'
                       AND NOT EXISTS (
                           SELECT 1 FROM direct_turn_steering_cancellations c
                           WHERE c.workflow_id = direct_turn_acceptances.workflow_id
                       )",
                )
                .bind(conversation_id)
                .bind(message_id)
                .execute(&mut *tx.tx)
                .await?;
            }
            sqlx::query(
                "DELETE FROM steering_messages
                 WHERE conversation_id = ?1 AND message_id = ?2",
            )
            .bind(conversation_id)
            .bind(message_id)
            .execute(&mut *tx.tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn load_pending_direct_turn_runtime_admission(
        &self,
        conversation_id: &str,
        client_message_id: &str,
    ) -> DbResult<Option<PendingDirectTurnRuntimeAdmission>> {
        let row = sqlx::query(
            "SELECT a.workflow_id, a.conversation_id, a.client_message_id, w.turn_generation
             FROM direct_turn_acceptances a
             JOIN top_level_llm_workflows w ON w.workflow_id = a.workflow_id
             WHERE a.conversation_id = ?1 AND a.client_message_id = ?2
               AND a.committed_outcome = 'PendingRuntime'
               AND w.stopped_at IS NULL",
        )
        .bind(conversation_id)
        .bind(client_message_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            Ok(PendingDirectTurnRuntimeAdmission {
                workflow_id: WorkflowId(to_u64(row.get("workflow_id"), "workflow_id")?),
                conversation_id: row.get("conversation_id"),
                client_message_id: row.get("client_message_id"),
                generation: Generation(to_u64(row.get("turn_generation"), "turn_generation")?),
            })
        })
        .transpose()
    }

    pub async fn claim_direct_turn_runtime_delivery(
        &self,
        conversation_id: &str,
        client_message_id: &str,
        process_incarnation: ProcessIncarnation,
    ) -> DbResult<bool> {
        let claimed = sqlx::query(
            "UPDATE direct_turn_acceptances
             SET runtime_delivery_incarnation = ?3
             WHERE conversation_id = ?1 AND client_message_id = ?2
               AND committed_outcome IN ('PendingRuntime', 'RuntimeAccepted', 'QueuedSteering')
               AND NOT EXISTS (
                   SELECT 1 FROM direct_turn_steering_cancellations c
                   WHERE c.workflow_id = direct_turn_acceptances.workflow_id
               )
               AND (runtime_delivery_incarnation IS NULL OR runtime_delivery_incarnation <> ?3)",
        )
        .bind(conversation_id)
        .bind(client_message_id)
        .bind(to_i64(process_incarnation.0, "process_incarnation")?)
        .execute(&self.pool)
        .await?;
        Ok(claimed.rows_affected() == 1)
    }

    pub async fn release_direct_turn_runtime_delivery(
        &self,
        conversation_id: &str,
        client_message_id: &str,
        process_incarnation: ProcessIncarnation,
    ) -> DbResult<()> {
        sqlx::query(
            "UPDATE direct_turn_acceptances
             SET runtime_delivery_incarnation = NULL
             WHERE conversation_id = ?1 AND client_message_id = ?2
               AND runtime_delivery_incarnation = ?3",
        )
        .bind(conversation_id)
        .bind(client_message_id)
        .bind(to_i64(process_incarnation.0, "process_incarnation")?)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn claim_recoverable_direct_turns(
        &self,
        process_incarnation: ProcessIncarnation,
    ) -> DbResult<Vec<DirectTurnAcceptanceRecord>> {
        let incarnation = to_i64(process_incarnation.0, "process_incarnation")?;
        let rows = sqlx::query(
            "UPDATE direct_turn_acceptances AS a
             SET runtime_delivery_incarnation = ?1
             WHERE a.committed_outcome IN ('PendingRuntime', 'RuntimeAccepted')
               AND (a.runtime_delivery_incarnation IS NULL OR a.runtime_delivery_incarnation <> ?1)
               AND NOT EXISTS (
                   SELECT 1 FROM conversation_creation_jobs cj
                   WHERE cj.conversation_id = a.conversation_id
                     AND cj.message_id = a.client_message_id
               )
               AND EXISTS (
                   SELECT 1 FROM top_level_llm_workflows w
                   WHERE w.workflow_id = a.workflow_id AND w.stopped_at IS NULL
               )
               AND (
                   a.committed_outcome = 'PendingRuntime'
                   OR (
                       a.live_slot = 1
                       AND a.materialized_message_id IS NOT NULL
                       AND NOT EXISTS (
                           SELECT 1 FROM top_level_llm_effects e
                           WHERE e.workflow_id = a.workflow_id
                       )
                   )
               )
             RETURNING conversation_id, client_message_id, workflow_id,
                       prepared_fingerprint, prepared_payload, committed_outcome, accepted_at",
        )
        .bind(incarnation)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| direct_turn_record_from_row(&row))
            .collect()
    }

    pub async fn load_pending_direct_turns(&self) -> DbResult<Vec<DirectTurnAcceptanceRecord>> {
        let rows = sqlx::query(
            "SELECT a.conversation_id, a.client_message_id, a.workflow_id,
                    a.prepared_fingerprint, a.prepared_payload, a.committed_outcome, a.accepted_at
             FROM direct_turn_acceptances a
             JOIN top_level_llm_workflows w ON w.workflow_id = a.workflow_id
             WHERE a.committed_outcome = 'PendingRuntime'
               AND NOT EXISTS (
                   SELECT 1 FROM conversation_creation_jobs cj
                   WHERE cj.conversation_id = a.conversation_id
                     AND cj.message_id = a.client_message_id
               )
               AND w.stopped_at IS NULL
             ORDER BY a.accepted_at, a.conversation_id, a.client_message_id",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| direct_turn_record_from_row(&row))
            .collect()
    }

    pub async fn load_active_top_level_llm_workflow(
        &self,
        conversation_id: &str,
    ) -> DbResult<Option<TopLevelLlmWorkflowRecord>> {
        let row = sqlx::query(
            "SELECT w.workflow_id, dta.conversation_id, dta.client_message_id AS accepted_turn_id,
                    w.turn_generation, w.accepted_assistant_message_id, w.stopped_at
             FROM top_level_llm_workflows w
             JOIN direct_turn_acceptances dta ON dta.workflow_id = w.workflow_id
             JOIN workflows wf ON wf.workflow_id = w.workflow_id
             WHERE dta.conversation_id = ?1 AND dta.committed_outcome = 'RuntimeAccepted'
               AND w.stopped_at IS NULL AND wf.status = 'Active'
             ORDER BY dta.accepted_at DESC LIMIT 1",
        )
        .bind(conversation_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| workflow_row_from_row(&row)).transpose()
    }

    pub async fn prepare_and_begin_top_level_llm_attempt(
        &self,
        input: &PrepareAndBeginTopLevelLlmInput,
    ) -> DbResult<PreparedTopLevelLlmAttempt> {
        let mut tx = self.begin_tx().await?;
        let workflow = sqlx::query(
            "SELECT version, generation, snapshot_payload
             FROM workflows
             WHERE workflow_id = ?1 AND status = 'Active'",
        )
        .bind(to_i64(input.workflow_id.0, "workflow_id")?)
        .fetch_optional(&mut *tx.tx)
        .await?
        .ok_or_else(|| {
            DbError::Serialization("active top-level LLM workflow missing".to_string())
        })?;
        let version = Version(to_u64(workflow.get("version"), "version")?);
        let generation = Generation(to_u64(workflow.get("generation"), "generation")?);
        if let Some(existing) = sqlx::query(
            "SELECT e.effect_id, e.call_ordinal, pr.codec_version,
                    pr.request_fingerprint, pr.provider, pr.model, pr.backend,
                    pr.request_aggregate
             FROM top_level_llm_effects e
             JOIN top_level_llm_prepared_requests pr
               ON pr.workflow_id = e.workflow_id AND pr.effect_id = e.effect_id
             JOIN workflow_attempts a
               ON a.workflow_id = e.workflow_id AND a.effect_id = e.effect_id
             WHERE e.workflow_id = ?1
               AND a.status IN ('AuthorityLost', 'ObservationRecorded', 'ReceiptAccepted')
               AND NOT EXISTS (
                   SELECT 1 FROM top_level_llm_response_receipts rr
                   WHERE rr.workflow_id = e.workflow_id AND rr.effect_id = e.effect_id
               )
               AND e.call_ordinal = (
                   SELECT MAX(call_ordinal) FROM top_level_llm_effects
                   WHERE workflow_id = ?1
               )
             ORDER BY a.attempt_id DESC LIMIT 1",
        )
        .bind(to_i64(input.workflow_id.0, "workflow_id")?)
        .fetch_optional(&mut *tx.tx)
        .await?
        {
            let effect_id = EffectId(to_u64(existing.get("effect_id"), "effect_id")?);
            let attempt_id = AttemptId(
                tx.allocate_sequence_value(input.workflow_id, WorkflowSequenceName::Attempt)
                    .await?,
            );
            sqlx::query(
                "UPDATE workflow_attempts SET status = 'ReceiptAccepted'
                 WHERE workflow_id = ?1 AND effect_id = ?2
                   AND status = 'ObservationRecorded'",
            )
            .bind(to_i64(input.workflow_id.0, "workflow_id")?)
            .bind(to_i64(effect_id.0, "effect_id")?)
            .execute(&mut *tx.tx)
            .await?;
            sqlx::query(
                "UPDATE workflow_effects SET status = 'Eligible'
                 WHERE workflow_id = ?1 AND effect_id = ?2 AND status = 'Executing'",
            )
            .bind(to_i64(input.workflow_id.0, "workflow_id")?)
            .bind(to_i64(effect_id.0, "effect_id")?)
            .execute(&mut *tx.tx)
            .await?;
            let begun = tx
                .begin_attempt(&BeginAttemptInput {
                    workflow_id: input.workflow_id,
                    effect_id,
                    attempt_id,
                    process_incarnation: input.process_incarnation,
                    now: input.committed_at,
                    lease_until: None,
                })
                .await?;
            let authority = begun.authority.ok_or_else(|| {
                DbError::Serialization("retryable LLM effect was not claimable".to_string())
            })?;
            let prepared_request = TopLevelLlmPreparedRequestRecord {
                workflow_id: input.workflow_id,
                effect_id,
                call_ordinal: to_u64(existing.get("call_ordinal"), "call_ordinal")?,
                codec_version: to_u32(existing.get("codec_version"), "codec_version")?,
                request_fingerprint: existing.get("request_fingerprint"),
                provider: existing.get("provider"),
                model: existing.get("model"),
                backend: existing.get("backend"),
                request_aggregate: existing.get("request_aggregate"),
            };
            let result = PreparedTopLevelLlmAttempt {
                prepared_request,
                authority,
            };
            tx.commit().await?;
            return Ok(result);
        }
        let snapshot: TopLevelLlmSnapshot =
            serde_json::from_slice(&workflow.get::<Vec<u8>, _>("snapshot_payload"))
                .map_err(|error| DbError::Serialization(error.to_string()))?;
        let transition_id = TransitionId(
            tx.allocate_sequence_value(input.workflow_id, WorkflowSequenceName::Transition)
                .await?,
        );
        let effect_id = EffectId(
            tx.allocate_sequence_value(input.workflow_id, WorkflowSequenceName::Effect)
                .await?,
        );
        let attempt_id = AttemptId(
            tx.allocate_sequence_value(input.workflow_id, WorkflowSequenceName::Attempt)
                .await?,
        );
        let call_ordinal = prepare_top_level_llm_request_tx(
            &mut tx,
            &PrepareTopLevelLlmRequestInput {
                workflow_id: input.workflow_id,
                effect_id,
                expected_version: version,
                transition_id,
                generation,
                committed_at: input.committed_at,
                snapshot,
                prepared_request: input.prepared_request.clone(),
            },
        )
        .await?;
        let begun = tx
            .begin_attempt(&BeginAttemptInput {
                workflow_id: input.workflow_id,
                effect_id,
                attempt_id,
                process_incarnation: input.process_incarnation,
                now: input.committed_at,
                lease_until: None,
            })
            .await?;
        let authority = begun.authority.ok_or_else(|| {
            DbError::Serialization("newly prepared LLM effect was not claimable".to_string())
        })?;
        tx.commit().await?;
        Ok(PreparedTopLevelLlmAttempt {
            prepared_request: TopLevelLlmPreparedRequestRecord {
                workflow_id: input.workflow_id,
                effect_id,
                call_ordinal,
                codec_version: input.prepared_request.codec_version,
                request_fingerprint: input.prepared_request.request_fingerprint.clone(),
                provider: input.prepared_request.provider.clone(),
                model: input.prepared_request.model.clone(),
                backend: input.prepared_request.backend.clone(),
                request_aggregate: input.prepared_request.request_aggregate.clone(),
            },
            authority,
        })
    }

    pub async fn prepare_top_level_llm_request(
        &self,
        input: &PrepareTopLevelLlmRequestInput,
    ) -> DbResult<CommitOutcome> {
        let mut tx = self.begin_tx().await?;
        prepare_top_level_llm_request_tx(&mut tx, input).await?;
        tx.commit().await?;
        Ok(CommitOutcome::Committed)
    }

    pub async fn begin_top_level_llm_attempt(
        &self,
        input: &BeginAttemptInput,
    ) -> DbResult<BeginAttemptResult> {
        self.begin_attempt(input).await
    }

    pub async fn record_top_level_llm_failure(
        &self,
        input: &RecordTopLevelLlmFailureInput,
    ) -> DbResult<AuthorityOutcome> {
        let mut tx = self.begin_tx().await?;
        let observation_id = tx
            .allocate_sequence_value(
                input.authority.workflow_id,
                WorkflowSequenceName::Observation,
            )
            .await?;
        let result = tx
            .record_observation(&super::RecordObservationInput {
                authority: input.authority.clone(),
                observation_id,
                now: input.observed_at,
                observed_at: input.observed_at,
                observation_codec: LocalCodec {
                    family: "llm.failure".to_string(),
                    version: 1,
                },
                observation_payload: input.outcome_payload.clone(),
            })
            .await?;
        if result.outcome == AuthorityOutcome::Authorized {
            tx.commit().await?;
        } else {
            tx.rollback().await?;
        }
        Ok(result.outcome)
    }

    pub async fn begin_recovered_top_level_llm_attempt(
        &self,
        workflow_id: WorkflowId,
        effect_id: EffectId,
        process_incarnation: ProcessIncarnation,
        now: Timestamp,
    ) -> DbResult<BeginAttemptResult> {
        let mut tx = self.begin_tx().await?;
        sqlx::query(
            "INSERT INTO workflow_sequences (workflow_id, sequence_name, next_value)
             SELECT ?1, 'attempt', COALESCE(MAX(attempt_id), 0) + 1
             FROM workflow_attempts WHERE workflow_id = ?1
             ON CONFLICT(workflow_id, sequence_name) DO UPDATE SET
               next_value = MAX(workflow_sequences.next_value, excluded.next_value)",
        )
        .bind(to_i64(workflow_id.0, "workflow_id")?)
        .execute(&mut *tx.tx)
        .await?;
        let attempt_id = AttemptId(
            tx.allocate_sequence_value(workflow_id, WorkflowSequenceName::Attempt)
                .await?,
        );
        sqlx::query(
            "UPDATE workflow_attempts SET status = 'AuthorityLost'
             WHERE workflow_id = ?1 AND effect_id = ?2
               AND status IN ('Begun', 'ObservationRecorded')",
        )
        .bind(to_i64(workflow_id.0, "workflow_id")?)
        .bind(to_i64(effect_id.0, "effect_id")?)
        .execute(&mut *tx.tx)
        .await?;
        sqlx::query(
            "UPDATE workflow_effects SET status = 'Eligible'
             WHERE workflow_id = ?1 AND effect_id = ?2 AND status = 'Executing'",
        )
        .bind(to_i64(workflow_id.0, "workflow_id")?)
        .bind(to_i64(effect_id.0, "effect_id")?)
        .execute(&mut *tx.tx)
        .await?;
        let result = tx
            .begin_attempt(&BeginAttemptInput {
                workflow_id,
                effect_id,
                attempt_id,
                process_incarnation,
                now,
                lease_until: None,
            })
            .await?;
        tx.commit().await?;
        Ok(result)
    }

    pub async fn recover_top_level_llm_attempts(
        &self,
        current_process_incarnation: ProcessIncarnation,
    ) -> DbResult<Vec<RecoverTopLevelLlmAttempt>> {
        let rows = sqlx::query(
            "SELECT w.workflow_id, dta.conversation_id, dta.client_message_id AS accepted_turn_id, w.turn_generation,
                    w.accepted_assistant_message_id, w.stopped_at,
                    p.codec_version, p.request_fingerprint, p.provider, p.model, p.backend, p.request_aggregate,
                    e.effect_id, e.call_ordinal,
                    a.attempt_id, a.ordinal, a.declared_workflow_version, a.generation,
                    a.process_incarnation, a.status, l.lease_until
             FROM top_level_llm_workflows w
             JOIN direct_turn_acceptances dta ON dta.workflow_id = w.workflow_id
             JOIN top_level_llm_effects e ON e.workflow_id = w.workflow_id
             JOIN top_level_llm_prepared_requests p ON p.workflow_id = e.workflow_id AND p.effect_id = e.effect_id
             JOIN workflow_attempts a ON a.workflow_id = e.workflow_id AND a.effect_id = e.effect_id
             LEFT JOIN workflow_reclaimable_leases l ON l.workflow_id = a.workflow_id AND l.attempt_id = a.attempt_id
             WHERE a.status IN ('Begun', 'ObservationRecorded')
               AND a.process_incarnation <> ?1
               AND w.stopped_at IS NULL
             ORDER BY w.workflow_id, a.attempt_id"
        )
        .bind(to_i64(
            current_process_incarnation.0,
            "current_process_incarnation",
        )?)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                let workflow_id =
                    WorkflowId(to_u64(row.get::<i64, _>("workflow_id"), "workflow_id")?);
                let effect_id = EffectId(to_u64(row.get::<i64, _>("effect_id"), "effect_id")?);
                let attempt_id = AttemptId(to_u64(row.get::<i64, _>("attempt_id"), "attempt_id")?);
                let authority = LocalAttemptAuthority {
                    workflow_id,
                    declared_workflow_version: Version(to_u64(
                        row.get::<i64, _>("declared_workflow_version"),
                        "declared_workflow_version",
                    )?),
                    generation: Generation(to_u64(row.get::<i64, _>("generation"), "generation")?),
                    effect_id,
                    attempt_id,
                    process_incarnation: ProcessIncarnation(to_u64(
                        row.get::<i64, _>("process_incarnation"),
                        "process_incarnation",
                    )?),
                };
                Ok(RecoverTopLevelLlmAttempt {
                    workflow: workflow_row_from_row(&row)?,
                    prepared_request: prepared_row_from_row(&row)?,
                    attempt: LocalAttemptRecord {
                        id: attempt_id,
                        ordinal: to_u32(row.get::<i64, _>("ordinal"), "ordinal")?,
                        authority: authority.clone(),
                        status: parse_attempt_status(&row.get::<String, _>("status"))?,
                        lease: row
                            .get::<Option<i64>, _>("lease_until")
                            .map(|v| {
                                to_u64(v, "lease_until").map(|lease_until| LocalReclaimableLease {
                                    attempt_id,
                                    lease_until: LeaseExpiry(lease_until),
                                })
                            })
                            .transpose()?,
                    },
                })
            })
            .collect()
    }

    pub async fn accept_complete_top_level_llm_response(
        &self,
        input: &AcceptCompleteLlmResponseInput,
    ) -> DbResult<AcceptCompleteLlmResponseResult> {
        match self
            .accept_complete_top_level_llm_response_once(input)
            .await
        {
            Err(DbError::Sqlx(sqlx::Error::Database(error)))
                if is_sqlite_busy_retryable(error.as_ref()) =>
            {
                Ok(AcceptCompleteLlmResponseResult {
                    outcome: CompleteLlmResponsePersistenceOutcome::RetryablePersistence,
                    receipt: None,
                    delivery: None,
                    llm_receipt: None,
                    tool_intents: vec![],
                })
            }
            result => result,
        }
    }

    async fn accept_complete_top_level_llm_response_once(
        &self,
        input: &AcceptCompleteLlmResponseInput,
    ) -> DbResult<AcceptCompleteLlmResponseResult> {
        let mut tx = self.begin_tx().await?;
        let turn = sqlx::query(
            "SELECT dta.client_message_id, w.turn_generation, w.stopped_at, e.call_ordinal
             FROM top_level_llm_workflows w
             JOIN direct_turn_acceptances dta ON dta.workflow_id = w.workflow_id
             JOIN top_level_llm_effects e ON e.workflow_id = w.workflow_id
             WHERE w.workflow_id = ?1 AND e.effect_id = ?2",
        )
        .bind(to_i64(input.authority.workflow_id.0, "workflow_id")?)
        .bind(to_i64(input.authority.effect_id.0, "effect_id")?)
        .fetch_optional(&mut *tx.tx)
        .await?;
        let Some(turn) = turn else {
            tx.rollback().await?;
            return Ok(stale_complete_llm_response_result());
        };
        if turn.get::<Option<i64>, _>("stopped_at").is_some() {
            tx.rollback().await?;
            return Ok(stale_complete_llm_response_result());
        }
        let accepted_turn_id: String = turn.get("client_message_id");
        let turn_generation = to_u64(turn.get::<i64, _>("turn_generation"), "turn_generation")?;
        let call_ordinal = to_u64(turn.get::<i64, _>("call_ordinal"), "call_ordinal")?;
        let (receipt_id, delivery_id) = match (input.receipt_id, input.delivery_id) {
            (Some(receipt_id), Some(delivery_id)) => (receipt_id, delivery_id),
            (None, None) => {
                let receipt_id = ReceiptId(
                    tx.allocate_sequence_value(
                        input.authority.workflow_id,
                        WorkflowSequenceName::Receipt,
                    )
                    .await?,
                );
                let delivery_id = DeliveryId(
                    tx.allocate_sequence_value(
                        input.authority.workflow_id,
                        WorkflowSequenceName::Delivery,
                    )
                    .await?,
                );
                (receipt_id, delivery_id)
            }
            _ => {
                return Err(DbError::Serialization(
                    "receipt and delivery identities must be supplied together".to_string(),
                ));
            }
        };
        let receipt_payload = serde_json::to_vec(&llm_profile::LlmResponseReceipt {
            key: LlmEffectKey {
                accepted_turn_id: accepted_turn_id.clone(),
                generation: turn_generation,
                call_ordinal,
            },
            response: input.response.clone(),
            generation: input.authority.generation.0,
        })
        .map_err(|e| DbError::Serialization(e.to_string()))?;
        let receipt_input = AcceptReceiptInput {
            authority: input.authority.clone(),
            receipt_id,
            delivery_id,
            attempt_id: Some(input.authority.attempt_id),
            origin: ReceiptOrigin::Execution,
            receipt_codec: local_codec_ref_to_owned(&llm_profile::receipt_codec()),
            receipt_payload,
            receipt_event_codec: local_codec_ref_to_owned(&llm_profile::receipt_codec()),
            receipt_event_payload: serde_json::to_vec(&llm_profile::LlmResponseReceipt {
                key: LlmEffectKey {
                    accepted_turn_id,
                    generation: turn_generation,
                    call_ordinal,
                },
                response: input.response.clone(),
                generation: input.authority.generation.0,
            })
            .map_err(|e| DbError::Serialization(e.to_string()))?,
            receipt_event_requires_runtime_acceptance: true,
            request_runtime_acceptance_for_cancellation: false,
        };
        let generic = tx.accept_receipt_and_delivery(&receipt_input).await?;
        if generic.outcome != AuthorityOutcome::Authorized {
            tx.rollback().await?;
            let existing = self
                .load_llm_response_receipt(input.authority.workflow_id, input.authority.effect_id)
                .await?;
            if let Some(llm_receipt) = existing {
                if llm_receipt.response_fingerprint != input.response.response_fingerprint
                    || llm_receipt.response_aggregate != input.response.response_aggregate
                {
                    return Ok(AcceptCompleteLlmResponseResult {
                        outcome: CompleteLlmResponsePersistenceOutcome::StaleAuthority,
                        receipt: None,
                        delivery: None,
                        llm_receipt: Some(llm_receipt),
                        tool_intents: vec![],
                    });
                }
                let intents = self
                    .load_tool_intents(input.authority.workflow_id, llm_receipt.receipt_id)
                    .await?;
                return Ok(AcceptCompleteLlmResponseResult {
                    outcome: CompleteLlmResponsePersistenceOutcome::ExactReplay,
                    receipt: self
                        .list_receipts(input.authority.workflow_id)
                        .await?
                        .into_iter()
                        .find(|r| r.receipt_id == llm_receipt.receipt_id),
                    delivery: self
                        .list_deliveries(input.authority.workflow_id)
                        .await?
                        .into_iter()
                        .find(|d| {
                            d.delivery_id == delivery_id
                                || d.effect_id == Some(input.authority.effect_id)
                        }),
                    llm_receipt: Some(llm_receipt),
                    tool_intents: intents,
                });
            }
            return Ok(AcceptCompleteLlmResponseResult {
                outcome: CompleteLlmResponsePersistenceOutcome::StaleAuthority,
                receipt: None,
                delivery: None,
                llm_receipt: None,
                tool_intents: vec![],
            });
        }
        sqlx::query("INSERT INTO top_level_llm_response_receipts (workflow_id, receipt_id, effect_id, codec_version, response_fingerprint, response_aggregate, provider_request_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)")
            .bind(to_i64(input.authority.workflow_id.0, "workflow_id")?)
            .bind(to_i64(receipt_id.0, "receipt_id")?)
            .bind(to_i64(input.authority.effect_id.0, "effect_id")?)
            .bind(i64::from(input.response.codec_version))
            .bind(&input.response.response_fingerprint)
            .bind(&input.response.response_aggregate)
            .bind(input.provider_request_id.as_deref())
            .execute(&mut *tx.tx)
            .await?;
        for intent in &input.tool_intents {
            sqlx::query("INSERT INTO top_level_llm_tool_intents (workflow_id, receipt_id, intent_ordinal, tool_name, tool_kind, tool_use_id, arguments_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)")
                .bind(to_i64(input.authority.workflow_id.0, "workflow_id")?)
                .bind(to_i64(receipt_id.0, "receipt_id")?)
                .bind(i64::from(intent.intent_ordinal))
                .bind(&intent.tool_name)
                .bind(tool_kind_to_str(&intent.tool_kind))
                .bind(&intent.tool_use_id)
                .bind(&intent.arguments_json)
                .execute(&mut *tx.tx)
                .await?;
        }
        tx.commit().await?;
        Ok(AcceptCompleteLlmResponseResult {
            outcome: CompleteLlmResponsePersistenceOutcome::Accepted,
            receipt: generic.receipt,
            delivery: generic.delivery,
            llm_receipt: Some(LlmResponseReceiptRecord {
                workflow_id: input.authority.workflow_id,
                receipt_id,
                effect_id: input.authority.effect_id,
                codec_version: input.response.codec_version,
                response_fingerprint: input.response.response_fingerprint.clone(),
                response_aggregate: input.response.response_aggregate.clone(),
                provider_request_id: input.provider_request_id.clone(),
            }),
            tool_intents: input.tool_intents.clone(),
        })
    }

    pub async fn load_owed_top_level_llm_receipts(&self) -> DbResult<Vec<OwedTopLevelLlmReceipt>> {
        let rows = sqlx::query(
            "SELECT w.workflow_id, dta.conversation_id, dta.client_message_id AS accepted_turn_id, w.turn_generation,
                    w.accepted_assistant_message_id, w.stopped_at,
                    pr.effect_id, e.call_ordinal,
                    pr.codec_version AS request_codec_version, pr.request_fingerprint, pr.provider, pr.model, pr.backend, pr.request_aggregate,
                    r.receipt_id, wr.generation, wr.declared_workflow_version, wr.process_incarnation, wr.attempt_id, wr.origin,
                    wr.receipt_codec_family, wr.receipt_codec_version, wr.receipt_payload,
                    d.delivery_id, d.consumer_kind, d.event_codec_family, d.event_codec_version, d.payload_kind, d.payload_blob,
                    d.requires_runtime_acceptance, d.status, d.runtime_acceptance_status, d.suppression_reason, d.accepted_by_transition_id,
                    rr.codec_version AS response_codec_version, rr.response_fingerprint, rr.response_aggregate, rr.provider_request_id
             FROM workflow_deliveries d
             JOIN workflow_receipts wr ON wr.workflow_id = d.workflow_id AND wr.effect_id = d.effect_id
             JOIN top_level_llm_response_receipts rr ON rr.workflow_id = wr.workflow_id AND rr.receipt_id = wr.receipt_id
             JOIN top_level_llm_prepared_requests pr ON pr.workflow_id = wr.workflow_id AND pr.effect_id = wr.effect_id
             JOIN top_level_llm_effects e ON e.workflow_id = pr.workflow_id AND e.effect_id = pr.effect_id
             JOIN top_level_llm_workflows w ON w.workflow_id = wr.workflow_id
             JOIN direct_turn_acceptances dta ON dta.workflow_id = w.workflow_id
             JOIN workflow_receipts r ON r.workflow_id = wr.workflow_id AND r.receipt_id = wr.receipt_id
             WHERE (
                 d.runtime_acceptance_status = 'Owed'
                 OR (
                     d.runtime_acceptance_status = 'Accepted'
                     AND EXISTS (
                         SELECT 1 FROM top_level_llm_tool_intents ti
                         WHERE ti.workflow_id = wr.workflow_id
                           AND ti.receipt_id = wr.receipt_id
                           AND ti.status = 'Owed'
                     )
                 )
             )
               AND w.stopped_at IS NULL
             ORDER BY w.workflow_id, wr.receipt_id"
        )
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let workflow_id = WorkflowId(to_u64(row.get::<i64, _>("workflow_id"), "workflow_id")?);
            let receipt_id = ReceiptId(to_u64(row.get::<i64, _>("receipt_id"), "receipt_id")?);
            out.push(OwedTopLevelLlmReceipt {
                workflow: workflow_row_from_row(&row)?,
                prepared_request: TopLevelLlmPreparedRequestRecord {
                    workflow_id,
                    effect_id: EffectId(to_u64(row.get::<i64, _>("effect_id"), "effect_id")?),
                    call_ordinal: to_u64(row.get::<i64, _>("call_ordinal"), "call_ordinal")?,
                    codec_version: to_u32(
                        row.get::<i64, _>("request_codec_version"),
                        "request_codec_version",
                    )?,
                    request_fingerprint: row.get("request_fingerprint"),
                    provider: row.get("provider"),
                    model: row.get("model"),
                    backend: row.get("backend"),
                    request_aggregate: row.get("request_aggregate"),
                },
                receipt: LocalReceiptRecord {
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
                        .map(|v| to_u64(v, "attempt_id").map(AttemptId))
                        .transpose()?,
                    origin: parse_receipt_origin(&row.get::<String, _>("origin"))?,
                    receipt_codec: local_codec(
                        row.get("receipt_codec_family"),
                        row.get("receipt_codec_version"),
                        "receipt_codec_version",
                    )?,
                    receipt_payload: row.get("receipt_payload"),
                },
                llm_receipt: LlmResponseReceiptRecord {
                    workflow_id,
                    receipt_id,
                    effect_id: EffectId(to_u64(row.get::<i64, _>("effect_id"), "effect_id")?),
                    codec_version: to_u32(
                        row.get::<i64, _>("response_codec_version"),
                        "response_codec_version",
                    )?,
                    response_fingerprint: row.get("response_fingerprint"),
                    response_aggregate: row.get("response_aggregate"),
                    provider_request_id: row.get("provider_request_id"),
                },
                delivery: LocalDeliveryRecord {
                    delivery_id: DeliveryId(to_u64(
                        row.get::<i64, _>("delivery_id"),
                        "delivery_id",
                    )?),
                    workflow_id,
                    effect_id: row
                        .get::<Option<i64>, _>("effect_id")
                        .map(|v| to_u64(v, "effect_id").map(EffectId))
                        .transpose()?,
                    barrier_id: None,
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
                },
                tool_intents: self.load_tool_intents(workflow_id, receipt_id).await?,
            });
        }
        Ok(out)
    }

    pub async fn load_owed_top_level_llm_tool_intent(
        &self,
        conversation_id: &str,
        tool_use_id: &str,
    ) -> DbResult<Option<(WorkflowId, ReceiptId, ToolIntentRecord, Generation)>> {
        self.load_top_level_llm_tool_intent(conversation_id, tool_use_id, ToolIntentStatus::Owed)
            .await
    }

    pub async fn load_top_level_llm_tool_intent(
        &self,
        conversation_id: &str,
        tool_use_id: &str,
        status: ToolIntentStatus,
    ) -> DbResult<Option<(WorkflowId, ReceiptId, ToolIntentRecord, Generation)>> {
        let row = sqlx::query(
            "SELECT ti.workflow_id, ti.receipt_id, ti.intent_ordinal, ti.status,
                    ti.tool_name, ti.tool_kind, ti.tool_use_id, ti.arguments_json,
                    r.generation
             FROM top_level_llm_tool_intents ti
             JOIN direct_turn_acceptances dta ON dta.workflow_id = ti.workflow_id
             JOIN workflow_receipts r ON r.workflow_id = ti.workflow_id
               AND r.receipt_id = ti.receipt_id
             JOIN top_level_llm_workflows w ON w.workflow_id = ti.workflow_id
             WHERE dta.conversation_id = ?1 AND ti.tool_use_id = ?2
               AND ti.status = ?3 AND w.stopped_at IS NULL",
        )
        .bind(conversation_id)
        .bind(tool_use_id)
        .bind(tool_intent_status_to_str(status))
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            Ok((
                WorkflowId(to_u64(row.get("workflow_id"), "workflow_id")?),
                ReceiptId(to_u64(row.get("receipt_id"), "receipt_id")?),
                ToolIntentRecord {
                    intent_ordinal: to_u32(row.get::<i64, _>("intent_ordinal"), "intent_ordinal")?,
                    status: parse_tool_intent_status(&row.get::<String, _>("status"))?,
                    tool_name: row.get("tool_name"),
                    tool_kind: parse_tool_kind(&row.get::<String, _>("tool_kind"))?,
                    tool_use_id: row.get("tool_use_id"),
                    arguments_json: row.get("arguments_json"),
                },
                Generation(to_u64(row.get("generation"), "generation")?),
            ))
        })
        .transpose()
    }

    pub async fn transition_top_level_llm_tool_intent(
        &self,
        input: &ToolIntentTransitionInput,
    ) -> DbResult<ToolIntentTransitionOutcome> {
        if !valid_tool_intent_transition(input.from, input.to) {
            return Ok(ToolIntentTransitionOutcome::Conflict);
        }
        match self.transition_top_level_llm_tool_intent_once(input).await {
            Err(DbError::Sqlx(sqlx::Error::Database(error)))
                if is_sqlite_busy_retryable(error.as_ref()) =>
            {
                Ok(ToolIntentTransitionOutcome::RetryablePersistence)
            }
            result => result,
        }
    }

    async fn transition_top_level_llm_tool_intent_once(
        &self,
        input: &ToolIntentTransitionInput,
    ) -> DbResult<ToolIntentTransitionOutcome> {
        let mut tx = self.begin_tx().await?;
        let updated = sqlx::query(
            "UPDATE top_level_llm_tool_intents
             SET status = ?6
             WHERE workflow_id = ?1 AND receipt_id = ?2 AND intent_ordinal = ?3
               AND status = ?5
               AND EXISTS (
                   SELECT 1 FROM workflow_receipts r
                   WHERE r.workflow_id = top_level_llm_tool_intents.workflow_id
                     AND r.receipt_id = top_level_llm_tool_intents.receipt_id
                     AND r.generation = ?4
               )",
        )
        .bind(to_i64(input.workflow_id.0, "workflow_id")?)
        .bind(to_i64(input.receipt_id.0, "receipt_id")?)
        .bind(i64::from(input.intent_ordinal))
        .bind(to_i64(input.generation.0, "generation")?)
        .bind(tool_intent_status_to_str(input.from))
        .bind(tool_intent_status_to_str(input.to))
        .execute(&mut *tx.tx)
        .await?;
        if updated.rows_affected() == 1 {
            tx.commit().await?;
            return Ok(ToolIntentTransitionOutcome::Committed);
        }
        let status = sqlx::query_scalar::<_, String>(
            "SELECT status FROM top_level_llm_tool_intents WHERE workflow_id = ?1 AND receipt_id = ?2 AND intent_ordinal = ?3",
        )
        .bind(to_i64(input.workflow_id.0, "workflow_id")?)
        .bind(to_i64(input.receipt_id.0, "receipt_id")?)
        .bind(i64::from(input.intent_ordinal))
        .fetch_optional(&mut *tx.tx)
        .await?;
        tx.rollback().await?;
        Ok(
            if status.as_deref() == Some(tool_intent_status_to_str(input.to)) {
                ToolIntentTransitionOutcome::ExactReplay
            } else {
                ToolIntentTransitionOutcome::Conflict
            },
        )
    }

    pub async fn claim_workflow_delivery(
        &self,
        workflow_id: WorkflowId,
        delivery_id: DeliveryId,
        process_incarnation: ProcessIncarnation,
        claimed_at: Timestamp,
    ) -> DbResult<bool> {
        let claimed = sqlx::query(
            "INSERT INTO workflow_delivery_claims (
                 workflow_id, delivery_id, process_incarnation, claimed_at
             )
             SELECT ?1, ?2, ?3, ?4
             WHERE EXISTS (
                 SELECT 1 FROM workflow_deliveries
                 WHERE workflow_id = ?1 AND delivery_id = ?2
                   AND (
                       runtime_acceptance_status = 'Owed'
                       OR (
                           runtime_acceptance_status = 'Accepted'
                           AND EXISTS (
                               SELECT 1 FROM top_level_llm_tool_intents ti
                               JOIN workflow_receipts r
                                 ON r.workflow_id = ti.workflow_id
                                AND r.receipt_id = ti.receipt_id
                               WHERE ti.workflow_id = workflow_deliveries.workflow_id
                                 AND r.effect_id = workflow_deliveries.effect_id
                                 AND ti.status = 'Owed'
                           )
                       )
                   )
             )
             ON CONFLICT(workflow_id, delivery_id) DO NOTHING",
        )
        .bind(to_i64(workflow_id.0, "workflow_id")?)
        .bind(to_i64(delivery_id.0, "delivery_id")?)
        .bind(to_i64(process_incarnation.0, "process_incarnation")?)
        .bind(to_i64(claimed_at.0, "claimed_at")?)
        .execute(&self.pool)
        .await?;
        Ok(claimed.rows_affected() == 1)
    }

    pub async fn release_workflow_delivery_claim(
        &self,
        workflow_id: WorkflowId,
        delivery_id: DeliveryId,
        process_incarnation: ProcessIncarnation,
    ) -> DbResult<()> {
        sqlx::query(
            "DELETE FROM workflow_delivery_claims
             WHERE workflow_id = ?1 AND delivery_id = ?2 AND process_incarnation = ?3",
        )
        .bind(to_i64(workflow_id.0, "workflow_id")?)
        .bind(to_i64(delivery_id.0, "delivery_id")?)
        .bind(to_i64(process_incarnation.0, "process_incarnation")?)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn reclaim_workflow_delivery_claims(
        &self,
        process_incarnation: ProcessIncarnation,
    ) -> DbResult<u64> {
        let deleted =
            sqlx::query("DELETE FROM workflow_delivery_claims WHERE process_incarnation <> ?1")
                .bind(to_i64(process_incarnation.0, "process_incarnation")?)
                .execute(&self.pool)
                .await?;
        Ok(deleted.rows_affected())
    }

    pub async fn interrupt_begun_top_level_llm_tools(
        &self,
        process_incarnation: ProcessIncarnation,
    ) -> DbResult<u64> {
        let updated = sqlx::query(
            "UPDATE top_level_llm_tool_intents AS ti
             SET status = 'Interrupted'
             WHERE ti.status = 'ExecutionMayHaveBegun'
               AND EXISTS (
                   SELECT 1
                   FROM workflow_receipts r
                   JOIN workflow_attempts a
                     ON a.workflow_id = r.workflow_id
                    AND a.attempt_id = r.attempt_id
                   WHERE r.workflow_id = ti.workflow_id
                     AND r.receipt_id = ti.receipt_id
                     AND a.process_incarnation <> ?1
               )",
        )
        .bind(to_i64(process_incarnation.0, "process_incarnation")?)
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected())
    }

    pub async fn load_owed_top_level_llm_receipt_for_conversation(
        &self,
        conversation_id: &str,
    ) -> DbResult<Option<OwedTopLevelLlmReceipt>> {
        Ok(self
            .load_owed_top_level_llm_receipts()
            .await?
            .into_iter()
            .find(|owed| owed.workflow.conversation_id == conversation_id))
    }

    pub async fn stop_active_top_level_llm_for_conversation(
        &self,
        conversation_id: &str,
        stopped_at: Timestamp,
    ) -> DbResult<Option<CommitOutcome>> {
        let row = sqlx::query(
            "SELECT wf.workflow_id, wf.version, wf.generation,
                    dta.client_message_id AS accepted_turn_id,
                    w.turn_generation, w.accepted_assistant_message_id,
                    COALESCE((
                        SELECT MAX(e.call_ordinal) FROM top_level_llm_effects e
                        WHERE e.workflow_id = wf.workflow_id
                    ), 0) AS call_ordinal
             FROM workflows wf
             JOIN top_level_llm_workflows w ON w.workflow_id = wf.workflow_id
             JOIN direct_turn_acceptances dta ON dta.workflow_id = wf.workflow_id
             WHERE dta.conversation_id = ?1
               AND dta.committed_outcome IN ('PendingRuntime', 'RuntimeAccepted')
               AND dta.live_slot = 1
               AND wf.status = 'Active' AND w.stopped_at IS NULL
             ORDER BY dta.accepted_at DESC LIMIT 1",
        )
        .bind(conversation_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let workflow_id = WorkflowId(to_u64(row.get("workflow_id"), "workflow_id")?);
        let accepted_turn_id: String = row.get("accepted_turn_id");
        let turn_generation = to_u64(row.get("turn_generation"), "turn_generation")?;
        let next_snapshot = TopLevelLlmSnapshot {
            turn_ref: TopLevelTurnRef {
                conversation_id: conversation_id.to_string(),
                accepted_turn_id: accepted_turn_id.clone(),
                generation: turn_generation,
            },
            accepted_assistant_message_id: row.get("accepted_assistant_message_id"),
            stopped_at: Some(stopped_at.0),
        };
        let mut tx = self.begin_tx().await?;
        let transition_id = TransitionId(
            tx.allocate_sequence_value(workflow_id, WorkflowSequenceName::Transition)
                .await?,
        );
        tx.commit().await?;
        self.stop_top_level_llm_and_suppress_pending_delivery(&StopTopLevelLlmInput {
            workflow_id,
            stopped_at,
            expected_version: Version(to_u64(row.get("version"), "version")?),
            transition_id,
            generation: Generation(to_u64(row.get("generation"), "generation")?),
            next_status: WorkflowStatus::Cancelled,
            event_payload: serde_json::to_vec(&llm_profile::TopLevelLlmEvent::ResponseCancelled {
                key: LlmEffectKey {
                    accepted_turn_id,
                    generation: turn_generation,
                    call_ordinal: to_u64(row.get("call_ordinal"), "call_ordinal")?,
                },
                reason: "stop".to_string(),
            })
            .map_err(|error| DbError::Serialization(error.to_string()))?,
            next_snapshot,
            suppression_reason: SuppressionReason::Cancelled,
        })
        .await
        .map(Some)
    }

    pub async fn stop_top_level_llm_and_suppress_pending_delivery(
        &self,
        input: &StopTopLevelLlmInput,
    ) -> DbResult<CommitOutcome> {
        let mut tx = self.begin_tx().await?;
        let deliveries = sqlx::query_scalar::<_, i64>(
            "SELECT delivery_id FROM workflow_deliveries WHERE workflow_id = ?1 AND status = 'Pending' ORDER BY delivery_id"
        )
        .bind(to_i64(input.workflow_id.0, "workflow_id")?)
        .fetch_all(&mut *tx.tx)
        .await?
        .into_iter()
        .map(|v| to_u64(v, "delivery_id").map(DeliveryId))
        .collect::<DbResult<Vec<_>>>()?;
        if deliveries.is_empty() {
            let event_codec = local_codec_ref_to_owned(&llm_profile::event_codec());
            let snapshot_codec = local_codec_ref_to_owned(&llm_profile::snapshot_codec());
            let snapshot_payload = serde_json::to_vec(&input.next_snapshot)
                .map_err(|e| DbError::Serialization(e.to_string()))?;
            let committed = tx
                .commit_transition_head_cas(
                    input.workflow_id,
                    input.expected_version,
                    input.generation,
                    input.next_status,
                    &event_codec,
                    &input.event_payload,
                    &snapshot_codec,
                    &snapshot_payload,
                    input.transition_id,
                    input.stopped_at,
                )
                .await?;
            let outcome = if committed {
                CommitOutcome::Committed
            } else {
                CommitOutcome::VersionConflict
            };
            if outcome != CommitOutcome::Committed {
                tx.rollback().await?;
                return Ok(outcome);
            }
            sqlx::query(
                "UPDATE top_level_llm_workflows SET stopped_at = ?2 WHERE workflow_id = ?1",
            )
            .bind(to_i64(input.workflow_id.0, "workflow_id")?)
            .bind(to_i64(input.stopped_at.0, "stopped_at")?)
            .execute(&mut *tx.tx)
            .await?;
            fence_tool_intents_for_stop(&mut tx, input.workflow_id).await?;
            sqlx::query(
                "UPDATE direct_turn_acceptances SET live_slot = NULL WHERE workflow_id = ?1",
            )
            .bind(to_i64(input.workflow_id.0, "workflow_id")?)
            .execute(&mut *tx.tx)
            .await?;
            tx.commit().await?;
            return Ok(CommitOutcome::Committed);
        }
        let event_codec = local_codec_ref_to_owned(&llm_profile::event_codec());
        let snapshot_codec = local_codec_ref_to_owned(&llm_profile::snapshot_codec());
        let snapshot_payload = serde_json::to_vec(&input.next_snapshot)
            .map_err(|e| DbError::Serialization(e.to_string()))?;
        let outcome = tx
            .resolve_deliveries_exact(DeliveryResolutionPlan {
                workflow_id: input.workflow_id,
                expected_version: input.expected_version,
                transition_id: input.transition_id,
                generation: input.generation,
                next_status: input.next_status,
                event_codec: &event_codec,
                event_payload: &input.event_payload,
                next_snapshot_codec: &snapshot_codec,
                next_snapshot_payload: &snapshot_payload,
                committed_at: input.stopped_at,
                exact_delivery_ids: &deliveries,
                decision: DeliveryResolutionDecision::Suppress {
                    reason: input.suppression_reason,
                },
            })
            .await?;
        if outcome != CommitOutcome::Committed {
            tx.rollback().await?;
            return Ok(outcome);
        }
        sqlx::query("UPDATE top_level_llm_workflows SET stopped_at = ?2 WHERE workflow_id = ?1")
            .bind(to_i64(input.workflow_id.0, "workflow_id")?)
            .bind(to_i64(input.stopped_at.0, "stopped_at")?)
            .execute(&mut *tx.tx)
            .await?;
        fence_tool_intents_for_stop(&mut tx, input.workflow_id).await?;
        sqlx::query("UPDATE direct_turn_acceptances SET live_slot = NULL WHERE workflow_id = ?1")
            .bind(to_i64(input.workflow_id.0, "workflow_id")?)
            .execute(&mut *tx.tx)
            .await?;
        tx.commit().await?;
        Ok(CommitOutcome::Committed)
    }

    async fn load_llm_response_receipt(
        &self,
        workflow_id: WorkflowId,
        effect_id: EffectId,
    ) -> DbResult<Option<LlmResponseReceiptRecord>> {
        let row = sqlx::query("SELECT workflow_id, receipt_id, effect_id, codec_version, response_fingerprint, response_aggregate, provider_request_id FROM top_level_llm_response_receipts WHERE workflow_id = ?1 AND effect_id = ?2")
            .bind(to_i64(workflow_id.0, "workflow_id")?)
            .bind(to_i64(effect_id.0, "effect_id")?)
            .fetch_optional(&self.pool)
            .await?;
        row.map(|row| {
            Ok(LlmResponseReceiptRecord {
                workflow_id,
                receipt_id: ReceiptId(to_u64(row.get::<i64, _>("receipt_id"), "receipt_id")?),
                effect_id,
                codec_version: to_u32(row.get::<i64, _>("codec_version"), "codec_version")?,
                response_fingerprint: row.get("response_fingerprint"),
                response_aggregate: row.get("response_aggregate"),
                provider_request_id: row.get("provider_request_id"),
            })
        })
        .transpose()
    }

    async fn load_tool_intents(
        &self,
        workflow_id: WorkflowId,
        receipt_id: ReceiptId,
    ) -> DbResult<Vec<ToolIntentRecord>> {
        let rows = sqlx::query("SELECT intent_ordinal, tool_name, tool_kind, tool_use_id, arguments_json, status FROM top_level_llm_tool_intents WHERE workflow_id = ?1 AND receipt_id = ?2 ORDER BY intent_ordinal")
            .bind(to_i64(workflow_id.0, "workflow_id")?)
            .bind(to_i64(receipt_id.0, "receipt_id")?)
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter()
            .map(|row| {
                Ok(ToolIntentRecord {
                    intent_ordinal: to_u32(row.get::<i64, _>("intent_ordinal"), "intent_ordinal")?,
                    status: parse_tool_intent_status(&row.get::<String, _>("status"))?,
                    tool_name: row.get("tool_name"),
                    tool_kind: parse_tool_kind(&row.get::<String, _>("tool_kind"))?,
                    tool_use_id: row.get("tool_use_id"),
                    arguments_json: row.get("arguments_json"),
                })
            })
            .collect()
    }
}

async fn fence_tool_intents_for_stop(
    tx: &mut WorkflowTx<'_>,
    workflow_id: WorkflowId,
) -> DbResult<()> {
    sqlx::query(
        "UPDATE top_level_llm_tool_intents
         SET status = CASE
             WHEN status = 'ExecutionMayHaveBegun' THEN 'Interrupted'
             ELSE 'Suppressed'
         END
         WHERE workflow_id = ?1
           AND status IN ('PendingAcceptance', 'Owed', 'ExecutionMayHaveBegun')",
    )
    .bind(to_i64(workflow_id.0, "workflow_id")?)
    .execute(&mut *tx.tx)
    .await?;
    Ok(())
}

impl WorkflowTx<'_> {
    async fn fetch_direct_turn_acceptance(
        &mut self,
        conversation_id: &str,
        client_message_id: &str,
    ) -> DbResult<Option<DirectTurnAcceptanceRecord>> {
        let row = sqlx::query("SELECT workflow_id, prepared_fingerprint, prepared_payload, committed_outcome, accepted_at FROM direct_turn_acceptances WHERE conversation_id = ?1 AND client_message_id = ?2")
            .bind(conversation_id)
            .bind(client_message_id)
            .fetch_optional(&mut *self.tx)
            .await?;
        row.map(|row| {
            Ok(DirectTurnAcceptanceRecord {
                workflow_id: WorkflowId(to_u64(row.get::<i64, _>("workflow_id"), "workflow_id")?),
                conversation_id: conversation_id.to_string(),
                client_message_id: client_message_id.to_string(),
                prepared_fingerprint: row.get("prepared_fingerprint"),
                prepared_payload: row.get("prepared_payload"),
                committed_outcome: parse_direct_turn_outcome(
                    &row.get::<String, _>("committed_outcome"),
                )?,
                accepted_at: Timestamp(to_u64(row.get::<i64, _>("accepted_at"), "accepted_at")?),
            })
        })
        .transpose()
    }
}

fn classify_direct_turn_replay(
    existing: DirectTurnAcceptanceRecord,
    input: &DirectTurnAcceptanceInput,
) -> DirectTurnAcceptanceOutcome {
    if existing.prepared_fingerprint == input.prepared_fingerprint {
        DirectTurnAcceptanceOutcome::Replayed(existing)
    } else {
        DirectTurnAcceptanceOutcome::Conflict
    }
}

async fn load_direct_turn_acceptance(
    pool: &SqlitePool,
    conversation_id: &str,
    client_message_id: &str,
) -> DbResult<Option<DirectTurnAcceptanceRecord>> {
    let row = sqlx::query(
        "SELECT a.conversation_id, a.client_message_id, a.workflow_id,
                a.prepared_fingerprint, a.prepared_payload,
                CASE WHEN c.workflow_id IS NULL THEN a.committed_outcome
                     ELSE 'CancelledSteering' END AS committed_outcome,
                a.accepted_at
         FROM direct_turn_acceptances a
         LEFT JOIN direct_turn_steering_cancellations c ON c.workflow_id = a.workflow_id
         WHERE a.conversation_id = ?1 AND a.client_message_id = ?2",
    )
    .bind(conversation_id)
    .bind(client_message_id)
    .fetch_optional(pool)
    .await?;
    row.map(|row| direct_turn_record_from_row(&row)).transpose()
}

async fn allocate_global_workflow_id(tx: &mut WorkflowTx<'_>) -> DbResult<WorkflowId> {
    sqlx::query(
        "INSERT INTO workflow_global_sequences (sequence_name, next_value) VALUES ('workflow', 2) ON CONFLICT(sequence_name) DO UPDATE SET next_value = workflow_global_sequences.next_value + 1",
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

fn local_codec_ref_to_owned(codec: &CodecRef) -> LocalCodec {
    LocalCodec {
        family: codec.family.to_string(),
        version: codec.version,
    }
}

fn workflow_row_from_row(row: &sqlx::sqlite::SqliteRow) -> DbResult<TopLevelLlmWorkflowRecord> {
    Ok(TopLevelLlmWorkflowRecord {
        workflow_id: WorkflowId(to_u64(row.get::<i64, _>("workflow_id"), "workflow_id")?),
        conversation_id: row.get("conversation_id"),
        accepted_turn_id: row.get("accepted_turn_id"),
        turn_generation: Generation(to_u64(
            row.get::<i64, _>("turn_generation"),
            "turn_generation",
        )?),
        accepted_assistant_message_id: row.get("accepted_assistant_message_id"),
        stopped_at: row
            .get::<Option<i64>, _>("stopped_at")
            .map(|v| to_u64(v, "stopped_at").map(Timestamp))
            .transpose()?,
    })
}

fn prepared_row_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> DbResult<TopLevelLlmPreparedRequestRecord> {
    Ok(TopLevelLlmPreparedRequestRecord {
        workflow_id: WorkflowId(to_u64(row.get::<i64, _>("workflow_id"), "workflow_id")?),
        effect_id: EffectId(to_u64(row.get::<i64, _>("effect_id"), "effect_id")?),
        call_ordinal: to_u64(row.get::<i64, _>("call_ordinal"), "call_ordinal")?,
        codec_version: to_u32(row.get::<i64, _>("codec_version"), "codec_version")?,
        request_fingerprint: row.get("request_fingerprint"),
        provider: row.get("provider"),
        model: row.get("model"),
        backend: row.get("backend"),
        request_aggregate: row.get("request_aggregate"),
    })
}

fn direct_turn_record_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> DbResult<DirectTurnAcceptanceRecord> {
    Ok(DirectTurnAcceptanceRecord {
        workflow_id: WorkflowId(to_u64(row.get::<i64, _>("workflow_id"), "workflow_id")?),
        conversation_id: row.get("conversation_id"),
        client_message_id: row.get("client_message_id"),
        prepared_fingerprint: row.get("prepared_fingerprint"),
        prepared_payload: row.get("prepared_payload"),
        committed_outcome: parse_direct_turn_outcome(&row.get::<String, _>("committed_outcome"))?,
        accepted_at: Timestamp(to_u64(row.get::<i64, _>("accepted_at"), "accepted_at")?),
    })
}

async fn prepare_top_level_llm_request_tx(
    tx: &mut WorkflowTx<'_>,
    input: &PrepareTopLevelLlmRequestInput,
) -> DbResult<u64> {
    let call_ordinal = sqlx::query_scalar::<_, i64>(
            "UPDATE top_level_llm_workflows SET next_call_ordinal = next_call_ordinal + 1 WHERE workflow_id = ?1 AND stopped_at IS NULL RETURNING next_call_ordinal - 1",
        )
        .bind(to_i64(input.workflow_id.0, "workflow_id")?)
        .fetch_optional(&mut *tx.tx)
        .await?
        .ok_or_else(|| DbError::Serialization("top-level LLM turn is stopped or missing".to_string()))?;
    let call_ordinal = to_u64(call_ordinal, "call_ordinal")?;
    let turn = sqlx::query(
        "SELECT dta.client_message_id, w.turn_generation FROM top_level_llm_workflows w JOIN direct_turn_acceptances dta ON dta.workflow_id = w.workflow_id WHERE w.workflow_id = ?1",
    )
    .bind(to_i64(input.workflow_id.0, "workflow_id")?)
    .fetch_one(&mut *tx.tx)
    .await?;
    let key = LlmEffectKey {
        accepted_turn_id: turn.get("client_message_id"),
        generation: to_u64(turn.get::<i64, _>("turn_generation"), "turn_generation")?,
        call_ordinal,
    };
    let intent = llm_profile::LlmIntent {
        key: key.clone(),
        prepared_request: input.prepared_request.clone(),
    };
    let event = llm_profile::TopLevelLlmEvent::Prepared { key };
    let outcome = tx
        .commit_transition_plan(&CommitTransitionPlanCas {
            workflow_id: input.workflow_id,
            expected_version: input.expected_version,
            transition_id: input.transition_id,
            generation: input.generation,
            next_status: WorkflowStatus::Active,
            event_codec: local_codec_ref_to_owned(&llm_profile::event_codec()),
            event_payload: serde_json::to_vec(&event)
                .map_err(|error| DbError::Serialization(error.to_string()))?,
            next_snapshot_codec: local_codec_ref_to_owned(&llm_profile::snapshot_codec()),
            next_snapshot_payload: serde_json::to_vec(&input.snapshot)
                .map_err(|error| DbError::Serialization(error.to_string()))?,
            committed_at: input.committed_at,
            effects: vec![LocalEffectDecl {
                effect_id: input.effect_id,
                declared_workflow_version: input.expected_version.next(),
                family: "llm.call".to_string(),
                kind: "top_level_call".to_string(),
                intent_codec: local_codec_ref_to_owned(&llm_profile::intent_codec()),
                intent_payload: serde_json::to_vec(&intent)
                    .map_err(|error| DbError::Serialization(error.to_string()))?,
                generation: input.generation,
                role: EffectRole::Required,
                capability: ExecutionCapability::SafelyRepeatable,
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
        .await?;
    if outcome != CommitOutcome::Committed {
        return Err(DbError::Serialization(format!(
            "top-level LLM preparation lost authority: {outcome:?}"
        )));
    }
    sqlx::query("INSERT INTO top_level_llm_effects (workflow_id, effect_id, call_ordinal) VALUES (?1, ?2, ?3)")
        .bind(to_i64(input.workflow_id.0, "workflow_id")?)
        .bind(to_i64(input.effect_id.0, "effect_id")?)
        .bind(to_i64(call_ordinal, "call_ordinal")?)
        .execute(&mut *tx.tx)
        .await?;
    sqlx::query("INSERT INTO top_level_llm_prepared_requests (workflow_id, effect_id, codec_version, request_fingerprint, provider, model, backend, request_aggregate) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)")
        .bind(to_i64(input.workflow_id.0, "workflow_id")?)
        .bind(to_i64(input.effect_id.0, "effect_id")?)
        .bind(i64::from(input.prepared_request.codec_version))
        .bind(&input.prepared_request.request_fingerprint)
        .bind(&input.prepared_request.provider)
        .bind(&input.prepared_request.model)
        .bind(&input.prepared_request.backend)
        .bind(&input.prepared_request.request_aggregate)
        .execute(&mut *tx.tx)
        .await?;
    Ok(call_ordinal)
}

fn direct_turn_outcome_to_str(value: &DirectTurnCommittedOutcome) -> &'static str {
    match value {
        DirectTurnCommittedOutcome::PendingRuntime => "PendingRuntime",
        DirectTurnCommittedOutcome::RuntimeAccepted => "RuntimeAccepted",
        DirectTurnCommittedOutcome::QueuedSteering => "QueuedSteering",
        DirectTurnCommittedOutcome::CancelledSteering => {
            unreachable!("cancelled steering is represented by its typed tombstone")
        }
    }
}

fn parse_direct_turn_outcome(value: &str) -> DbResult<DirectTurnCommittedOutcome> {
    match value {
        "PendingRuntime" => Ok(DirectTurnCommittedOutcome::PendingRuntime),
        "RuntimeAccepted" => Ok(DirectTurnCommittedOutcome::RuntimeAccepted),
        "QueuedSteering" => Ok(DirectTurnCommittedOutcome::QueuedSteering),
        "CancelledSteering" => Ok(DirectTurnCommittedOutcome::CancelledSteering),
        other => Err(DbError::Serialization(format!(
            "unknown direct-turn outcome: {other}"
        ))),
    }
}

fn stale_complete_llm_response_result() -> AcceptCompleteLlmResponseResult {
    AcceptCompleteLlmResponseResult {
        outcome: CompleteLlmResponsePersistenceOutcome::StaleAuthority,
        receipt: None,
        delivery: None,
        llm_receipt: None,
        tool_intents: vec![],
    }
}

fn valid_tool_intent_transition(from: ToolIntentStatus, to: ToolIntentStatus) -> bool {
    matches!(
        (from, to),
        (
            ToolIntentStatus::PendingAcceptance,
            ToolIntentStatus::Owed | ToolIntentStatus::Suppressed
        ) | (
            ToolIntentStatus::Owed,
            ToolIntentStatus::ExecutionMayHaveBegun | ToolIntentStatus::Suppressed
        ) | (
            ToolIntentStatus::ExecutionMayHaveBegun,
            ToolIntentStatus::Completed | ToolIntentStatus::Interrupted
        )
    )
}

fn tool_intent_status_to_str(value: ToolIntentStatus) -> &'static str {
    match value {
        ToolIntentStatus::PendingAcceptance => "PendingAcceptance",
        ToolIntentStatus::Owed => "Owed",
        ToolIntentStatus::ExecutionMayHaveBegun => "ExecutionMayHaveBegun",
        ToolIntentStatus::Completed => "Completed",
        ToolIntentStatus::Interrupted => "Interrupted",
        ToolIntentStatus::Suppressed => "Suppressed",
    }
}

fn parse_tool_intent_status(value: &str) -> DbResult<ToolIntentStatus> {
    match value {
        "PendingAcceptance" => Ok(ToolIntentStatus::PendingAcceptance),
        "Owed" => Ok(ToolIntentStatus::Owed),
        "ExecutionMayHaveBegun" => Ok(ToolIntentStatus::ExecutionMayHaveBegun),
        "Completed" => Ok(ToolIntentStatus::Completed),
        "Interrupted" => Ok(ToolIntentStatus::Interrupted),
        "Suppressed" => Ok(ToolIntentStatus::Suppressed),
        other => Err(DbError::Serialization(format!(
            "unknown tool intent status: {other}"
        ))),
    }
}

fn tool_kind_to_str(value: &ToolKindRecord) -> &'static str {
    match value {
        ToolKindRecord::Function => "Function",
        ToolKindRecord::Custom => "Custom",
    }
}

fn parse_tool_kind(value: &str) -> DbResult<ToolKindRecord> {
    match value {
        "Function" => Ok(ToolKindRecord::Function),
        "Custom" => Ok(ToolKindRecord::Custom),
        other => Err(DbError::Serialization(format!(
            "unknown tool kind: {other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrations::run_pending_migrations;
    use phoenix_workflow::AttemptStatus;
    use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
    use std::str::FromStr;

    async fn setup_repo_schema(pool: &SqlitePool) {
        sqlx::query("CREATE TABLE conversations (id TEXT PRIMARY KEY, conv_mode TEXT NOT NULL DEFAULT '{\"mode\":\"Explore\"}', state TEXT NOT NULL DEFAULT '{\"type\":\"idle\"}', cwd TEXT NOT NULL DEFAULT '/tmp', parent_conversation_id TEXT, user_initiated BOOLEAN NOT NULL DEFAULT 1, archived BOOLEAN NOT NULL DEFAULT 0, model TEXT, steering_queue TEXT NOT NULL DEFAULT '[]', state_updated_at TEXT NOT NULL DEFAULT '2025-01-01', created_at TEXT NOT NULL DEFAULT '2025-01-01', updated_at TEXT NOT NULL DEFAULT '2025-01-01')").execute(pool).await.unwrap();
        sqlx::query("CREATE TABLE messages (id INTEGER PRIMARY KEY AUTOINCREMENT, message_id TEXT UNIQUE, conversation_id TEXT NOT NULL, message_type TEXT NOT NULL, content TEXT NOT NULL, sequence_id INTEGER NOT NULL DEFAULT 1, created_at TEXT NOT NULL DEFAULT '2025-01-01')").execute(pool).await.unwrap();
        sqlx::query("INSERT INTO conversations (id) VALUES ('conv-1')")
            .execute(pool)
            .await
            .unwrap();
        run_pending_migrations(pool).await.unwrap();
    }

    async fn open_repo() -> WorkflowRepository {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workflow.db");
        let url = format!("sqlite://{}", path.display());
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
        setup_repo_schema(&pool).await;
        std::mem::forget(dir);
        WorkflowRepository::new(pool)
    }

    async fn open_repo_with_lock_pool() -> (tempfile::TempDir, WorkflowRepository, SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workflow.db");
        let url = format!("sqlite://{}", path.display());
        let options = || {
            SqliteConnectOptions::from_str(&url)
                .unwrap()
                .create_if_missing(true)
                .journal_mode(SqliteJournalMode::Wal)
                .busy_timeout(std::time::Duration::from_millis(1))
        };
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options())
            .await
            .unwrap();
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&pool)
            .await
            .unwrap();
        setup_repo_schema(&pool).await;
        let lock_pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options())
            .await
            .unwrap();
        (dir, WorkflowRepository::new(pool), lock_pool)
    }

    async fn open_independent_repos() -> (tempfile::TempDir, WorkflowRepository, WorkflowRepository)
    {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workflow.db");
        let url = format!("sqlite://{}", path.display());
        let options = || {
            SqliteConnectOptions::from_str(&url)
                .unwrap()
                .create_if_missing(true)
                .journal_mode(SqliteJournalMode::Wal)
                .busy_timeout(std::time::Duration::from_secs(5))
        };
        let first_pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options())
            .await
            .unwrap();
        setup_repo_schema(&first_pool).await;
        let second_pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options())
            .await
            .unwrap();
        (
            dir,
            WorkflowRepository::new(first_pool),
            WorkflowRepository::new(second_pool),
        )
    }

    fn snapshot() -> TopLevelLlmSnapshot {
        TopLevelLlmSnapshot {
            turn_ref: TopLevelTurnRef {
                conversation_id: "conv-1".to_string(),
                accepted_turn_id: "msg-1".to_string(),
                generation: 4,
            },
            accepted_assistant_message_id: None,
            stopped_at: None,
        }
    }

    #[tokio::test]
    async fn queued_steering_acceptance_atomically_persists_queue_entry() {
        let repo = open_repo().await;
        let entry = phoenix_core::domain::sm_event::SteerEntry {
            text: "steer".to_string(),
            llm_text: Some("expanded steer".to_string()),
            images: Vec::new(),
            files: Vec::new(),
            message_id: "msg-steer".to_string(),
            user_agent: Some("ios".to_string()),
            skill_invocation: None,
        };
        let outcome = repo
            .accept_direct_turn(&DirectTurnAcceptanceInput {
                initial_outcome: DirectTurnInitialOutcome::QueuedSteering {
                    entry: Box::new(entry.clone()),
                },
                conversation_id: "conv-1".to_string(),
                client_message_id: "msg-steer".to_string(),
                prepared_fingerprint: "fp-steer".to_string(),
                prepared_payload: "{}".to_string(),
                accepted_at: Timestamp(1),
                snapshot: snapshot(),
            })
            .await
            .unwrap();
        assert!(matches!(outcome, DirectTurnAcceptanceOutcome::Created(_)));

        let persisted = sqlx::query(
            "SELECT message_id, text, llm_text, user_agent FROM steering_messages
             WHERE conversation_id = 'conv-1' AND ordinal = 0",
        )
        .fetch_one(&repo.pool)
        .await
        .unwrap();
        assert_eq!(persisted.get::<String, _>("message_id"), entry.message_id);
        assert_eq!(persisted.get::<String, _>("text"), entry.text);
        assert_eq!(
            persisted.get::<Option<String>, _>("llm_text"),
            entry.llm_text
        );
        assert_eq!(
            persisted.get::<Option<String>, _>("user_agent"),
            entry.user_agent
        );
    }

    #[tokio::test]
    async fn cancelled_steering_is_tombstoned_and_cannot_replay_as_queued() {
        let repo = open_repo().await;
        let input = DirectTurnAcceptanceInput {
            initial_outcome: DirectTurnInitialOutcome::QueuedSteering {
                entry: Box::new(phoenix_core::domain::sm_event::SteerEntry {
                    text: "steer".to_string(),
                    llm_text: None,
                    images: Vec::new(),
                    files: Vec::new(),
                    message_id: "msg-steer".to_string(),
                    user_agent: None,
                    skill_invocation: None,
                }),
            },
            conversation_id: "conv-1".to_string(),
            client_message_id: "msg-steer".to_string(),
            prepared_fingerprint: "fp-steer".to_string(),
            prepared_payload: "{}".to_string(),
            accepted_at: Timestamp(1),
            snapshot: snapshot(),
        };
        repo.accept_direct_turn(&input).await.unwrap();
        assert!(repo
            .cancel_queued_steering("conv-1", "msg-steer")
            .await
            .unwrap());
        assert!(!repo
            .cancel_queued_steering("conv-1", "msg-steer")
            .await
            .unwrap());
        assert_eq!(
            repo.load_direct_turn_acceptance("conv-1", "msg-steer")
                .await
                .unwrap()
                .unwrap()
                .committed_outcome,
            DirectTurnCommittedOutcome::CancelledSteering
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM steering_messages WHERE message_id = 'msg-steer'"
            )
            .fetch_one(&repo.pool)
            .await
            .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn cancelled_steering_cannot_be_claimed_for_runtime_delivery() {
        let repo = open_repo().await;
        repo.accept_direct_turn(&DirectTurnAcceptanceInput {
            initial_outcome: DirectTurnInitialOutcome::QueuedSteering {
                entry: Box::new(phoenix_core::domain::sm_event::SteerEntry {
                    text: "steer".to_string(),
                    llm_text: None,
                    images: Vec::new(),
                    files: Vec::new(),
                    message_id: "msg-steer".to_string(),
                    user_agent: None,
                    skill_invocation: None,
                }),
            },
            conversation_id: "conv-1".to_string(),
            client_message_id: "msg-steer".to_string(),
            prepared_fingerprint: "fp-steer".to_string(),
            prepared_payload: "{}".to_string(),
            accepted_at: Timestamp(1),
            snapshot: snapshot(),
        })
        .await
        .unwrap();
        assert!(repo
            .cancel_queued_steering("conv-1", "msg-steer")
            .await
            .unwrap());

        assert!(!repo
            .claim_direct_turn_runtime_delivery("conv-1", "msg-steer", ProcessIncarnation(7),)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn stale_drain_cannot_promote_cancelled_steering() {
        let repo = open_repo().await;
        repo.accept_direct_turn(&DirectTurnAcceptanceInput {
            initial_outcome: DirectTurnInitialOutcome::QueuedSteering {
                entry: Box::new(phoenix_core::domain::sm_event::SteerEntry {
                    text: "steer".to_string(),
                    llm_text: None,
                    images: Vec::new(),
                    files: Vec::new(),
                    message_id: "msg-steer".to_string(),
                    user_agent: None,
                    skill_invocation: None,
                }),
            },
            conversation_id: "conv-1".to_string(),
            client_message_id: "msg-steer".to_string(),
            prepared_fingerprint: "fp-steer".to_string(),
            prepared_payload: "{}".to_string(),
            accepted_at: Timestamp(1),
            snapshot: snapshot(),
        })
        .await
        .unwrap();
        let staged = vec!["msg-steer".to_string()];
        assert!(repo
            .cancel_queued_steering("conv-1", "msg-steer")
            .await
            .unwrap());

        assert!(repo
            .consume_queued_steering_batch("conv-1", "msg-steer", &staged)
            .await
            .is_err());
        assert_eq!(
            repo.load_direct_turn_acceptance("conv-1", "msg-steer")
                .await
                .unwrap()
                .unwrap()
                .committed_outcome,
            DirectTurnCommittedOutcome::CancelledSteering
        );
    }

    #[tokio::test]
    async fn direct_turn_accept_replay_conflict() {
        let repo = open_repo().await;
        let input = DirectTurnAcceptanceInput {
            initial_outcome: DirectTurnInitialOutcome::PendingRuntime,
            conversation_id: "conv-1".to_string(),
            client_message_id: "msg-1".to_string(),
            prepared_fingerprint: "fp-1".to_string(),
            prepared_payload: "{}".to_string(),
            accepted_at: Timestamp(1),
            snapshot: snapshot(),
        };
        assert!(matches!(
            repo.accept_direct_turn(&input).await.unwrap(),
            DirectTurnAcceptanceOutcome::Created(_)
        ));
        assert!(matches!(
            repo.accept_direct_turn(&input).await.unwrap(),
            DirectTurnAcceptanceOutcome::Replayed(_)
        ));
        let mut equivalent_encoding = input.clone();
        equivalent_encoding.prepared_payload = "{ \"same\": true }".to_string();
        assert!(matches!(
            repo.accept_direct_turn(&equivalent_encoding).await.unwrap(),
            DirectTurnAcceptanceOutcome::Replayed(_)
        ));
        let pending = repo.load_pending_direct_turns().await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(
            repo.commit_direct_turn_runtime_admission(&DirectTurnRuntimeAdmissionInput {
                workflow_id: WorkflowId(1),
                conversation_id: "conv-1".to_string(),
                client_message_id: "msg-1".to_string(),
                generation: Generation(5),
                disposition: DirectTurnCommittedOutcome::RuntimeAccepted,
            })
            .await
            .unwrap(),
            DirectTurnRuntimeAdmissionOutcome::Conflict
        );
        assert_eq!(repo.load_pending_direct_turns().await.unwrap().len(), 1);
        let admission = DirectTurnRuntimeAdmissionInput {
            workflow_id: WorkflowId(1),
            conversation_id: "conv-1".to_string(),
            client_message_id: "msg-1".to_string(),
            generation: Generation(4),
            disposition: DirectTurnCommittedOutcome::RuntimeAccepted,
        };
        assert_eq!(
            repo.commit_direct_turn_runtime_admission(&admission)
                .await
                .unwrap(),
            DirectTurnRuntimeAdmissionOutcome::Committed
        );
        assert_eq!(
            repo.commit_direct_turn_runtime_admission(&admission)
                .await
                .unwrap(),
            DirectTurnRuntimeAdmissionOutcome::ExactReplay
        );
        assert_eq!(
            repo.commit_direct_turn_runtime_admission(&DirectTurnRuntimeAdmissionInput {
                disposition: DirectTurnCommittedOutcome::QueuedSteering,
                ..admission.clone()
            })
            .await
            .unwrap(),
            DirectTurnRuntimeAdmissionOutcome::Conflict
        );
        assert!(repo.load_pending_direct_turns().await.unwrap().is_empty());
        assert!(repo
            .load_pending_direct_turn_runtime_admission("conv-1", "msg-1")
            .await
            .unwrap()
            .is_none());
        repo.prepare_and_begin_top_level_llm_attempt(&PrepareAndBeginTopLevelLlmInput {
            workflow_id: WorkflowId(1),
            committed_at: Timestamp(3),
            process_incarnation: ProcessIncarnation(7),
            prepared_request: PreparedLlmRequest {
                codec_version: 1,
                request_fingerprint: "req-1".to_string(),
                provider: "mock".to_string(),
                model: "mock".to_string(),
                backend: "mock".to_string(),
                request_aggregate: "{}".to_string(),
            },
        })
        .await
        .unwrap();
        assert!(repo.load_pending_direct_turns().await.unwrap().is_empty());
        sqlx::query("INSERT INTO conversations (id) VALUES ('conv-2')")
            .execute(&repo.pool)
            .await
            .unwrap();
        let mut other_conversation = input.clone();
        other_conversation.conversation_id = "conv-2".to_string();
        assert!(matches!(
            repo.accept_direct_turn(&other_conversation).await.unwrap(),
            DirectTurnAcceptanceOutcome::Created(_)
        ));
        let mut conflict = input.clone();
        conflict.prepared_fingerprint = "fp-2".to_string();
        assert_eq!(
            repo.accept_direct_turn(&conflict).await.unwrap(),
            DirectTurnAcceptanceOutcome::Conflict
        );
    }

    #[tokio::test]
    async fn direct_turn_acceptance_has_one_winner_across_independent_connections() {
        let (_dir, first, second) = open_independent_repos().await;
        let input = DirectTurnAcceptanceInput {
            initial_outcome: DirectTurnInitialOutcome::PendingRuntime,
            conversation_id: "conv-1".to_string(),
            client_message_id: "msg-race".to_string(),
            prepared_fingerprint: "fp-race".to_string(),
            prepared_payload: "{}".to_string(),
            accepted_at: Timestamp(1),
            snapshot: snapshot(),
        };
        let (left, right) = tokio::join!(
            first.accept_direct_turn(&input),
            second.accept_direct_turn(&input),
        );
        let mut outcomes = [left.unwrap(), right.unwrap()];
        if matches!(
            outcomes[0],
            DirectTurnAcceptanceOutcome::RetryablePersistence
        ) {
            outcomes[0] = first.accept_direct_turn(&input).await.unwrap();
        }
        if matches!(
            outcomes[1],
            DirectTurnAcceptanceOutcome::RetryablePersistence
        ) {
            outcomes[1] = second.accept_direct_turn(&input).await.unwrap();
        }
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| matches!(outcome, DirectTurnAcceptanceOutcome::Created(_)))
                .count(),
            1
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| matches!(outcome, DirectTurnAcceptanceOutcome::Replayed(_)))
                .count(),
            1
        );
        let workflow_ids: Vec<_> = outcomes
            .iter()
            .filter_map(|outcome| match outcome {
                DirectTurnAcceptanceOutcome::Created(record)
                | DirectTurnAcceptanceOutcome::Replayed(record) => Some(record.workflow_id),
                DirectTurnAcceptanceOutcome::Conflict
                | DirectTurnAcceptanceOutcome::RetryablePersistence => None,
            })
            .collect();
        assert_eq!(workflow_ids, vec![workflow_ids[0], workflow_ids[0]]);
    }

    #[tokio::test]
    async fn distinct_direct_turns_race_for_one_conversation_slot() {
        let (_dir, first, second) = open_independent_repos().await;
        let left_input = DirectTurnAcceptanceInput {
            initial_outcome: DirectTurnInitialOutcome::PendingRuntime,
            conversation_id: "conv-1".to_string(),
            client_message_id: "msg-left".to_string(),
            prepared_fingerprint: "fp-left".to_string(),
            prepared_payload: "{}".to_string(),
            accepted_at: Timestamp(1),
            snapshot: snapshot(),
        };
        let mut right_input = left_input.clone();
        right_input.client_message_id = "msg-right".to_string();
        right_input.prepared_fingerprint = "fp-right".to_string();
        let (left, right) = tokio::join!(
            first.accept_direct_turn(&left_input),
            second.accept_direct_turn(&right_input),
        );
        let mut outcomes = [left.unwrap(), right.unwrap()];
        for (index, repo, input) in [(0, &first, &left_input), (1, &second, &right_input)] {
            if matches!(
                outcomes[index],
                DirectTurnAcceptanceOutcome::RetryablePersistence
            ) {
                outcomes[index] = repo.accept_direct_turn(input).await.unwrap();
            }
        }
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| matches!(outcome, DirectTurnAcceptanceOutcome::Created(_)))
                .count(),
            1
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| matches!(outcome, DirectTurnAcceptanceOutcome::Conflict))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn queued_steering_batch_consumption_is_atomic_and_replayable() {
        let repo = open_repo().await;
        let input = DirectTurnAcceptanceInput {
            initial_outcome: DirectTurnInitialOutcome::PendingRuntime,
            conversation_id: "conv-1".to_string(),
            client_message_id: "msg-1".to_string(),
            prepared_fingerprint: "fp-1".to_string(),
            prepared_payload: "{}".to_string(),
            accepted_at: Timestamp(1),
            snapshot: snapshot(),
        };
        let DirectTurnAcceptanceOutcome::Created(record) =
            repo.accept_direct_turn(&input).await.unwrap()
        else {
            panic!("direct turn was not created");
        };
        repo.commit_direct_turn_runtime_admission(&DirectTurnRuntimeAdmissionInput {
            workflow_id: record.workflow_id,
            conversation_id: input.conversation_id.clone(),
            client_message_id: input.client_message_id.clone(),
            generation: Generation(4),
            disposition: DirectTurnCommittedOutcome::QueuedSteering,
        })
        .await
        .unwrap();

        let message_ids = vec![input.client_message_id.clone()];
        repo.consume_queued_steering_batch(
            &input.conversation_id,
            &input.client_message_id,
            &message_ids,
        )
        .await
        .unwrap();
        repo.consume_queued_steering_batch(
            &input.conversation_id,
            &input.client_message_id,
            &message_ids,
        )
        .await
        .unwrap();
        assert_eq!(
            repo.load_active_top_level_llm_workflow(&input.conversation_id)
                .await
                .unwrap()
                .unwrap()
                .workflow_id,
            record.workflow_id
        );
    }

    #[tokio::test]
    async fn queued_steering_batch_finalizes_every_drained_acceptance() {
        let repo = open_repo().await;
        for (message_id, accepted_at) in [("msg-1", 1), ("msg-2", 2)] {
            repo.accept_direct_turn(&DirectTurnAcceptanceInput {
                initial_outcome: DirectTurnInitialOutcome::QueuedSteering {
                    entry: Box::new(phoenix_core::domain::sm_event::SteerEntry {
                        text: message_id.to_string(),
                        llm_text: None,
                        images: Vec::new(),
                        files: Vec::new(),
                        message_id: message_id.to_string(),
                        user_agent: None,
                        skill_invocation: None,
                    }),
                },
                conversation_id: "conv-1".to_string(),
                client_message_id: message_id.to_string(),
                prepared_fingerprint: format!("fp-{message_id}"),
                prepared_payload: "{}".to_string(),
                accepted_at: Timestamp(accepted_at),
                snapshot: snapshot(),
            })
            .await
            .unwrap();
        }
        let drained = vec!["msg-1".to_string(), "msg-2".to_string()];

        repo.consume_queued_steering_batch("conv-1", "msg-2", &drained)
            .await
            .unwrap();

        for message_id in drained {
            assert_eq!(
                repo.load_direct_turn_acceptance("conv-1", &message_id)
                    .await
                    .unwrap()
                    .unwrap()
                    .committed_outcome,
                DirectTurnCommittedOutcome::RuntimeAccepted
            );
        }
    }

    #[tokio::test]
    async fn recovery_claims_only_the_materialized_steering_batch_owner() {
        let repo = open_repo().await;
        for (message_id, accepted_at) in [("msg-1", 1), ("msg-2", 2)] {
            repo.accept_direct_turn(&DirectTurnAcceptanceInput {
                initial_outcome: DirectTurnInitialOutcome::QueuedSteering {
                    entry: Box::new(phoenix_core::domain::sm_event::SteerEntry {
                        text: message_id.to_string(),
                        llm_text: None,
                        images: Vec::new(),
                        files: Vec::new(),
                        message_id: message_id.to_string(),
                        user_agent: None,
                        skill_invocation: None,
                    }),
                },
                conversation_id: "conv-1".to_string(),
                client_message_id: message_id.to_string(),
                prepared_fingerprint: format!("fp-{message_id}"),
                prepared_payload: "{}".to_string(),
                accepted_at: Timestamp(accepted_at),
                snapshot: snapshot(),
            })
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO messages
                 (message_id, conversation_id, message_type, content, sequence_id)
                 VALUES (?1, 'conv-1', 'user', '{}', ?2)",
            )
            .bind(message_id)
            .bind(i64::try_from(accepted_at).unwrap())
            .execute(&repo.pool)
            .await
            .unwrap();
            sqlx::query(
                "UPDATE direct_turn_acceptances SET materialized_message_id = ?1
                 WHERE conversation_id = 'conv-1' AND client_message_id = ?1",
            )
            .bind(message_id)
            .execute(&repo.pool)
            .await
            .unwrap();
        }
        let drained = vec!["msg-1".to_string(), "msg-2".to_string()];
        repo.consume_queued_steering_batch("conv-1", "msg-2", &drained)
            .await
            .unwrap();

        let claimed = repo
            .claim_recoverable_direct_turns(ProcessIncarnation(9))
            .await
            .unwrap();
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].client_message_id, "msg-2");
    }

    #[tokio::test]
    async fn prepare_and_begin_allocates_authoritative_effect_and_attempt_identity() {
        let repo = open_repo().await;
        repo.accept_direct_turn(&DirectTurnAcceptanceInput {
            initial_outcome: DirectTurnInitialOutcome::PendingRuntime,
            conversation_id: "conv-1".to_string(),
            client_message_id: "msg-1".to_string(),
            prepared_fingerprint: "fp-1".to_string(),
            prepared_payload: "{}".to_string(),
            accepted_at: Timestamp(1),
            snapshot: snapshot(),
        })
        .await
        .unwrap();
        repo.commit_direct_turn_runtime_admission(&DirectTurnRuntimeAdmissionInput {
            workflow_id: WorkflowId(1),
            conversation_id: "conv-1".to_string(),
            client_message_id: "msg-1".to_string(),
            generation: Generation(4),
            disposition: DirectTurnCommittedOutcome::RuntimeAccepted,
        })
        .await
        .unwrap();

        let input = PrepareAndBeginTopLevelLlmInput {
            workflow_id: WorkflowId(1),
            committed_at: Timestamp(2),
            process_incarnation: ProcessIncarnation(9),
            prepared_request: PreparedLlmRequest {
                codec_version: 1,
                request_fingerprint: "request-fp".to_string(),
                provider: "configured".to_string(),
                model: "model".to_string(),
                backend: "llm_client".to_string(),
                request_aggregate: "{}".to_string(),
            },
        };
        let prepared = repo
            .prepare_and_begin_top_level_llm_attempt(&input)
            .await
            .unwrap();

        assert_eq!(prepared.prepared_request.effect_id, EffectId(1));
        assert_eq!(prepared.prepared_request.call_ordinal, 0);
        assert_eq!(prepared.authority.attempt_id, AttemptId(1));
        assert_eq!(
            repo.record_top_level_llm_failure(&RecordTopLevelLlmFailureInput {
                authority: prepared.authority,
                observed_at: Timestamp(3),
                outcome_payload: b"network error".to_vec(),
            })
            .await
            .unwrap(),
            AuthorityOutcome::Authorized
        );
        let retry = repo
            .prepare_and_begin_top_level_llm_attempt(&PrepareAndBeginTopLevelLlmInput {
                committed_at: Timestamp(4),
                ..input
            })
            .await
            .unwrap();
        assert_eq!(retry.prepared_request.effect_id, EffectId(1));
        assert_eq!(retry.prepared_request.call_ordinal, 0);
        assert_eq!(retry.authority.effect_id, EffectId(1));
        assert_eq!(retry.authority.attempt_id, AttemptId(2));
        assert!(repo
            .recover_top_level_llm_attempts(ProcessIncarnation(9))
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn recovery_claims_once_and_includes_runtime_accepted_without_effect() {
        let repo = open_repo().await;
        let input = DirectTurnAcceptanceInput {
            initial_outcome: DirectTurnInitialOutcome::PendingRuntime,
            conversation_id: "conv-1".to_string(),
            client_message_id: "msg-1".to_string(),
            prepared_fingerprint: "fp-1".to_string(),
            prepared_payload: "{}".to_string(),
            accepted_at: Timestamp(1),
            snapshot: snapshot(),
        };
        let DirectTurnAcceptanceOutcome::Created(record) =
            repo.accept_direct_turn(&input).await.unwrap()
        else {
            panic!("direct turn was not created");
        };
        repo.commit_direct_turn_runtime_admission(&DirectTurnRuntimeAdmissionInput {
            workflow_id: record.workflow_id,
            conversation_id: input.conversation_id.clone(),
            client_message_id: input.client_message_id.clone(),
            generation: Generation(4),
            disposition: DirectTurnCommittedOutcome::RuntimeAccepted,
        })
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO messages
             (message_id, conversation_id, message_type, content, sequence_id)
             VALUES ('msg-1', 'conv-1', 'user', '{}', 1)",
        )
        .execute(&repo.pool)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE direct_turn_acceptances SET materialized_message_id = 'msg-1'
             WHERE workflow_id = ?1",
        )
        .bind(i64::try_from(record.workflow_id.0).unwrap())
        .execute(&repo.pool)
        .await
        .unwrap();

        assert_eq!(
            repo.claim_recoverable_direct_turns(ProcessIncarnation(7))
                .await
                .unwrap()
                .len(),
            1
        );
        assert!(repo
            .claim_recoverable_direct_turns(ProcessIncarnation(7))
            .await
            .unwrap()
            .is_empty());
        assert_eq!(
            repo.claim_recoverable_direct_turns(ProcessIncarnation(8))
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn startup_reclamation_reuses_effect_before_pending_delivery() {
        let repo = open_repo().await;
        repo.accept_direct_turn(&DirectTurnAcceptanceInput {
            initial_outcome: DirectTurnInitialOutcome::RuntimeAccepted,
            conversation_id: "conv-1".to_string(),
            client_message_id: "msg-1".to_string(),
            prepared_fingerprint: "fp-1".to_string(),
            prepared_payload: "{}".to_string(),
            accepted_at: Timestamp(1),
            snapshot: snapshot(),
        })
        .await
        .unwrap();
        let old_incarnation = PrepareAndBeginTopLevelLlmInput {
            workflow_id: WorkflowId(1),
            committed_at: Timestamp(2),
            process_incarnation: ProcessIncarnation(7),
            prepared_request: PreparedLlmRequest {
                codec_version: 1,
                request_fingerprint: "request-fp".to_string(),
                provider: "configured".to_string(),
                model: "model".to_string(),
                backend: "llm_client".to_string(),
                request_aggregate: "{}".to_string(),
            },
        };
        let first = repo
            .prepare_and_begin_top_level_llm_attempt(&old_incarnation)
            .await
            .unwrap();
        assert_eq!(first.prepared_request.effect_id, EffectId(1));
        assert_eq!(first.prepared_request.call_ordinal, 0);

        let recovered = repo
            .recover_top_level_llm_attempts(ProcessIncarnation(8))
            .await
            .unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].prepared_request.effect_id, EffectId(1));
        assert_eq!(recovered[0].prepared_request.call_ordinal, 0);
        let resumed = repo
            .begin_recovered_top_level_llm_attempt(
                WorkflowId(1),
                EffectId(1),
                ProcessIncarnation(8),
                Timestamp(3),
            )
            .await
            .unwrap();
        let resumed_authority = resumed.authority.expect("reclaimed attempt authority");

        assert_eq!(resumed_authority.effect_id, EffectId(1));
        assert_eq!(resumed_authority.attempt_id, AttemptId(2));
        assert_eq!(
            repo.list_attempts(WorkflowId(1), EffectId(1))
                .await
                .unwrap()
                .len(),
            2
        );
    }

    #[tokio::test]
    async fn runtime_admission_reports_retryable_persistence_while_writer_is_locked() {
        let (_dir, repo, lock_pool) = open_repo_with_lock_pool().await;
        repo.accept_direct_turn(&DirectTurnAcceptanceInput {
            initial_outcome: DirectTurnInitialOutcome::PendingRuntime,
            conversation_id: "conv-1".to_string(),
            client_message_id: "msg-1".to_string(),
            prepared_fingerprint: "fp-1".to_string(),
            prepared_payload: "{}".to_string(),
            accepted_at: Timestamp(1),
            snapshot: snapshot(),
        })
        .await
        .unwrap();
        let input = DirectTurnRuntimeAdmissionInput {
            workflow_id: WorkflowId(1),
            conversation_id: "conv-1".to_string(),
            client_message_id: "msg-1".to_string(),
            generation: Generation(4),
            disposition: DirectTurnCommittedOutcome::RuntimeAccepted,
        };
        let mut writer = lock_pool.acquire().await.unwrap();
        sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut *writer)
            .await
            .unwrap();
        assert_eq!(
            repo.commit_direct_turn_runtime_admission(&input)
                .await
                .unwrap(),
            DirectTurnRuntimeAdmissionOutcome::RetryablePersistence
        );
        assert_eq!(repo.load_pending_direct_turns().await.unwrap().len(), 1);
        sqlx::query("ROLLBACK").execute(&mut *writer).await.unwrap();
        assert_eq!(
            repo.commit_direct_turn_runtime_admission(&input)
                .await
                .unwrap(),
            DirectTurnRuntimeAdmissionOutcome::Committed
        );
    }

    #[tokio::test]
    async fn completed_response_reports_retryable_persistence_while_writer_is_locked() {
        let (_dir, repo, lock_pool) = open_repo_with_lock_pool().await;
        repo.accept_direct_turn(&DirectTurnAcceptanceInput {
            initial_outcome: DirectTurnInitialOutcome::PendingRuntime,
            conversation_id: "conv-1".to_string(),
            client_message_id: "msg-1".to_string(),
            prepared_fingerprint: "fp-1".to_string(),
            prepared_payload: "{}".to_string(),
            accepted_at: Timestamp(1),
            snapshot: snapshot(),
        })
        .await
        .unwrap();
        repo.prepare_top_level_llm_request(&PrepareTopLevelLlmRequestInput {
            workflow_id: WorkflowId(1),
            effect_id: EffectId(2),
            expected_version: Version(0),
            transition_id: TransitionId(1),
            generation: Generation(0),
            committed_at: Timestamp(2),
            snapshot: snapshot(),
            prepared_request: PreparedLlmRequest {
                codec_version: 1,
                request_fingerprint: "req-1".to_string(),
                provider: "openai".to_string(),
                model: "gpt-5".to_string(),
                backend: "responses".to_string(),
                request_aggregate: "{\"messages\":[]}".to_string(),
            },
        })
        .await
        .unwrap();
        let authority = repo
            .begin_top_level_llm_attempt(&BeginAttemptInput {
                workflow_id: WorkflowId(1),
                effect_id: EffectId(2),
                attempt_id: AttemptId(1),
                process_incarnation: ProcessIncarnation(7),
                now: Timestamp(3),
                lease_until: None,
            })
            .await
            .unwrap()
            .authority
            .unwrap();
        let input = AcceptCompleteLlmResponseInput {
            authority,
            delivery_id: Some(DeliveryId(1)),
            receipt_id: Some(ReceiptId(1)),
            response: CompleteLlmResponse {
                codec_version: 1,
                response_fingerprint: "resp-1".to_string(),
                response_aggregate: "{\"output\":[]}".to_string(),
            },
            provider_request_id: Some("provider-1".to_string()),
            tool_intents: vec![],
        };

        let mut writer = lock_pool.acquire().await.unwrap();
        sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut *writer)
            .await
            .unwrap();
        let locked = repo
            .accept_complete_top_level_llm_response(&input)
            .await
            .unwrap();
        assert_eq!(
            locked.outcome,
            CompleteLlmResponsePersistenceOutcome::RetryablePersistence
        );
        assert!(repo.list_receipts(WorkflowId(1)).await.unwrap().is_empty());
        sqlx::query("ROLLBACK").execute(&mut *writer).await.unwrap();

        let accepted = repo
            .accept_complete_top_level_llm_response(&input)
            .await
            .unwrap();
        assert_eq!(
            accepted.outcome,
            CompleteLlmResponsePersistenceOutcome::Accepted
        );
        assert_eq!(repo.list_receipts(WorkflowId(1)).await.unwrap().len(), 1);
        let replay = repo
            .accept_complete_top_level_llm_response(&input)
            .await
            .unwrap();
        assert_eq!(
            replay.outcome,
            CompleteLlmResponsePersistenceOutcome::ExactReplay
        );
        assert_eq!(repo.list_receipts(WorkflowId(1)).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn stop_pending_runtime_before_first_effect_commits_and_releases_live_slot() {
        let repo = open_repo().await;
        let input = DirectTurnAcceptanceInput {
            initial_outcome: DirectTurnInitialOutcome::PendingRuntime,
            conversation_id: "conv-1".to_string(),
            client_message_id: "msg-1".to_string(),
            prepared_fingerprint: "fp-1".to_string(),
            prepared_payload: "{}".to_string(),
            accepted_at: Timestamp(1),
            snapshot: snapshot(),
        };
        assert!(matches!(
            repo.accept_direct_turn(&input).await.unwrap(),
            DirectTurnAcceptanceOutcome::Created(_)
        ));

        assert_eq!(
            repo.stop_active_top_level_llm_for_conversation("conv-1", Timestamp(2))
                .await
                .unwrap(),
            Some(CommitOutcome::Committed)
        );
        let cancelled_payload: Vec<u8> = sqlx::query_scalar(
            "SELECT event_payload FROM workflow_transitions
             WHERE workflow_id = 1 ORDER BY transition_id DESC LIMIT 1",
        )
        .fetch_one(&repo.pool)
        .await
        .unwrap();
        let cancelled: llm_profile::TopLevelLlmEvent =
            serde_json::from_slice(&cancelled_payload).unwrap();
        assert!(matches!(
            cancelled,
            llm_profile::TopLevelLlmEvent::ResponseCancelled {
                key: LlmEffectKey {
                    call_ordinal: 0,
                    ..
                },
                ..
            }
        ));

        let next = DirectTurnAcceptanceInput {
            client_message_id: "msg-2".to_string(),
            prepared_fingerprint: "fp-2".to_string(),
            accepted_at: Timestamp(3),
            snapshot: TopLevelLlmSnapshot {
                turn_ref: TopLevelTurnRef {
                    accepted_turn_id: "msg-2".to_string(),
                    ..snapshot().turn_ref
                },
                ..snapshot()
            },
            ..input
        };
        assert!(matches!(
            repo.accept_direct_turn(&next).await.unwrap(),
            DirectTurnAcceptanceOutcome::Created(_)
        ));
    }

    #[tokio::test]
    async fn stop_cancels_the_latest_prepared_call_ordinal() {
        let repo = open_repo().await;
        repo.accept_direct_turn(&DirectTurnAcceptanceInput {
            initial_outcome: DirectTurnInitialOutcome::RuntimeAccepted,
            conversation_id: "conv-1".to_string(),
            client_message_id: "msg-1".to_string(),
            prepared_fingerprint: "fp-1".to_string(),
            prepared_payload: "{}".to_string(),
            accepted_at: Timestamp(1),
            snapshot: snapshot(),
        })
        .await
        .unwrap();
        sqlx::query(
            "UPDATE top_level_llm_workflows SET next_call_ordinal = 3 WHERE workflow_id = 1",
        )
        .execute(&repo.pool)
        .await
        .unwrap();
        repo.prepare_top_level_llm_request(&PrepareTopLevelLlmRequestInput {
            workflow_id: WorkflowId(1),
            effect_id: EffectId(1),
            expected_version: Version(0),
            transition_id: TransitionId(2),
            generation: Generation(0),
            committed_at: Timestamp(2),
            snapshot: snapshot(),
            prepared_request: PreparedLlmRequest {
                codec_version: 1,
                request_fingerprint: "req-3".to_string(),
                provider: "openai".to_string(),
                model: "gpt-5".to_string(),
                backend: "responses".to_string(),
                request_aggregate: "{\"messages\":[]}".to_string(),
            },
        })
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO workflow_sequences (workflow_id, sequence_name, next_value)
             VALUES (1, 'transition', 3)
             ON CONFLICT(workflow_id, sequence_name) DO UPDATE SET next_value = 3",
        )
        .execute(&repo.pool)
        .await
        .unwrap();

        repo.stop_active_top_level_llm_for_conversation("conv-1", Timestamp(3))
            .await
            .unwrap();
        let payload: Vec<u8> = sqlx::query_scalar(
            "SELECT event_payload FROM workflow_transitions
             WHERE workflow_id = 1 ORDER BY transition_id DESC LIMIT 1",
        )
        .fetch_one(&repo.pool)
        .await
        .unwrap();
        let event: llm_profile::TopLevelLlmEvent = serde_json::from_slice(&payload).unwrap();
        assert!(matches!(
            event,
            llm_profile::TopLevelLlmEvent::ResponseCancelled {
                key: LlmEffectKey {
                    call_ordinal: 3,
                    ..
                },
                ..
            }
        ));
    }

    #[tokio::test]
    async fn prepare_begin_recover_accept_owed_and_stop_flow() {
        let repo = open_repo().await;
        repo.accept_direct_turn(&DirectTurnAcceptanceInput {
            initial_outcome: DirectTurnInitialOutcome::PendingRuntime,
            conversation_id: "conv-1".to_string(),
            client_message_id: "msg-1".to_string(),
            prepared_fingerprint: "fp-1".to_string(),
            prepared_payload: "{}".to_string(),
            accepted_at: Timestamp(1),
            snapshot: snapshot(),
        })
        .await
        .unwrap();
        assert_eq!(
            repo.commit_direct_turn_runtime_admission(&DirectTurnRuntimeAdmissionInput {
                workflow_id: WorkflowId(1),
                conversation_id: "conv-1".to_string(),
                client_message_id: "msg-1".to_string(),
                generation: Generation(4),
                disposition: DirectTurnCommittedOutcome::RuntimeAccepted,
            })
            .await
            .unwrap(),
            DirectTurnRuntimeAdmissionOutcome::Committed
        );
        assert_eq!(
            repo.prepare_top_level_llm_request(&PrepareTopLevelLlmRequestInput {
                workflow_id: WorkflowId(1),
                effect_id: EffectId(2),
                expected_version: Version(0),
                transition_id: TransitionId(1),
                generation: Generation(0),
                committed_at: Timestamp(2),
                snapshot: snapshot(),
                prepared_request: PreparedLlmRequest {
                    codec_version: 1,
                    request_fingerprint: "req-1".to_string(),
                    provider: "openai".to_string(),
                    model: "gpt-5".to_string(),
                    backend: "responses".to_string(),
                    request_aggregate: "{\"messages\":[]}".to_string()
                },
            })
            .await
            .unwrap(),
            CommitOutcome::Committed
        );
        let begun = repo
            .begin_top_level_llm_attempt(&BeginAttemptInput {
                workflow_id: WorkflowId(1),
                effect_id: EffectId(2),
                attempt_id: AttemptId(1),
                process_incarnation: ProcessIncarnation(7),
                now: Timestamp(3),
                lease_until: None,
            })
            .await
            .unwrap();
        assert_eq!(begun.outcome, ClaimOutcome::Started);
        assert!(repo
            .recover_top_level_llm_attempts(ProcessIncarnation(7))
            .await
            .unwrap()
            .is_empty());
        let recoverable = repo
            .recover_top_level_llm_attempts(ProcessIncarnation(8))
            .await
            .unwrap();
        assert_eq!(recoverable.len(), 1);
        let recovered = repo
            .begin_recovered_top_level_llm_attempt(
                WorkflowId(1),
                EffectId(2),
                ProcessIncarnation(8),
                Timestamp(4),
            )
            .await
            .unwrap();
        assert_eq!(recovered.outcome, ClaimOutcome::Started);
        let recovered_authority = recovered.authority.unwrap();
        let attempts = repo
            .list_attempts(WorkflowId(1), EffectId(2))
            .await
            .unwrap();
        assert_eq!(attempts.len(), 2);
        assert_eq!(attempts[0].status, AttemptStatus::AuthorityLost);
        assert_eq!(attempts[1].status, AttemptStatus::Begun);
        assert_eq!(
            repo.recover_top_level_llm_attempts(ProcessIncarnation(9))
                .await
                .unwrap()
                .len(),
            1
        );
        let result = repo
            .accept_complete_top_level_llm_response(&AcceptCompleteLlmResponseInput {
                authority: recovered_authority,
                delivery_id: Some(DeliveryId(1)),
                receipt_id: Some(ReceiptId(1)),
                response: CompleteLlmResponse {
                    codec_version: 1,
                    response_fingerprint: "resp-1".to_string(),
                    response_aggregate: "{\"output\":[]}".to_string(),
                },
                provider_request_id: Some("provider-1".to_string()),
                tool_intents: vec![ToolIntentRecord {
                    intent_ordinal: 0,
                    status: ToolIntentStatus::PendingAcceptance,
                    tool_name: "submit_result".to_string(),
                    tool_kind: ToolKindRecord::Function,
                    tool_use_id: "tool-1".to_string(),
                    arguments_json: "{\"result\":\"ok\"}".to_string(),
                }],
            })
            .await
            .unwrap();
        assert_eq!(
            result.outcome,
            CompleteLlmResponsePersistenceOutcome::Accepted
        );
        assert_eq!(
            repo.load_owed_top_level_llm_receipts().await.unwrap().len(),
            1
        );
        assert!(repo
            .claim_workflow_delivery(
                WorkflowId(1),
                DeliveryId(1),
                ProcessIncarnation(7),
                Timestamp(6),
            )
            .await
            .unwrap());
        assert!(!repo
            .claim_workflow_delivery(
                WorkflowId(1),
                DeliveryId(1),
                ProcessIncarnation(7),
                Timestamp(7),
            )
            .await
            .unwrap());
        repo.release_workflow_delivery_claim(WorkflowId(1), DeliveryId(1), ProcessIncarnation(7))
            .await
            .unwrap();
        assert!(repo
            .claim_workflow_delivery(
                WorkflowId(1),
                DeliveryId(1),
                ProcessIncarnation(7),
                Timestamp(8),
            )
            .await
            .unwrap());
        let tool_transition = ToolIntentTransitionInput {
            workflow_id: WorkflowId(1),
            receipt_id: ReceiptId(1),
            intent_ordinal: 0,
            generation: Generation(0),
            from: ToolIntentStatus::PendingAcceptance,
            to: ToolIntentStatus::Owed,
        };
        assert_eq!(
            repo.transition_top_level_llm_tool_intent(&tool_transition)
                .await
                .unwrap(),
            ToolIntentTransitionOutcome::Committed
        );
        assert_eq!(
            repo.transition_top_level_llm_tool_intent(&tool_transition)
                .await
                .unwrap(),
            ToolIntentTransitionOutcome::ExactReplay
        );
        assert_eq!(
            repo.transition_top_level_llm_tool_intent(&ToolIntentTransitionInput {
                from: ToolIntentStatus::Owed,
                to: ToolIntentStatus::ExecutionMayHaveBegun,
                ..tool_transition.clone()
            })
            .await
            .unwrap(),
            ToolIntentTransitionOutcome::Committed
        );
        assert_eq!(
            repo.accept_complete_top_level_llm_response(&AcceptCompleteLlmResponseInput {
                authority: LocalAttemptAuthority {
                    workflow_id: WorkflowId(1),
                    declared_workflow_version: Version(1),
                    generation: Generation(0),
                    effect_id: EffectId(2),
                    attempt_id: AttemptId(1),
                    process_incarnation: ProcessIncarnation(7)
                },
                delivery_id: Some(DeliveryId(1)),
                receipt_id: Some(ReceiptId(1)),
                response: CompleteLlmResponse {
                    codec_version: 1,
                    response_fingerprint: "resp-1".to_string(),
                    response_aggregate: "{\"output\":[]}".to_string()
                },
                provider_request_id: Some("provider-1".to_string()),
                tool_intents: vec![],
            })
            .await
            .unwrap()
            .outcome,
            CompleteLlmResponsePersistenceOutcome::ExactReplay
        );
        let stopped = repo
            .stop_top_level_llm_and_suppress_pending_delivery(&StopTopLevelLlmInput {
                workflow_id: WorkflowId(1),
                stopped_at: Timestamp(10),
                expected_version: Version(1),
                transition_id: TransitionId(2),
                generation: Generation(0),
                next_status: WorkflowStatus::Cancelled,
                event_payload: serde_json::to_vec(
                    &llm_profile::TopLevelLlmEvent::ResponseCancelled {
                        key: LlmEffectKey {
                            accepted_turn_id: "msg-1".to_string(),
                            generation: 4,
                            call_ordinal: 2,
                        },
                        reason: "stop".to_string(),
                    },
                )
                .unwrap(),
                next_snapshot: TopLevelLlmSnapshot {
                    stopped_at: Some(10),
                    ..snapshot()
                },
                suppression_reason: SuppressionReason::Cancelled,
            })
            .await
            .unwrap();
        assert_eq!(stopped, CommitOutcome::Committed);
        assert_eq!(
            repo.accept_complete_top_level_llm_response(&AcceptCompleteLlmResponseInput {
                authority: LocalAttemptAuthority {
                    workflow_id: WorkflowId(1),
                    declared_workflow_version: Version(1),
                    generation: Generation(0),
                    effect_id: EffectId(2),
                    attempt_id: AttemptId(1),
                    process_incarnation: ProcessIncarnation(7),
                },
                delivery_id: None,
                receipt_id: None,
                response: CompleteLlmResponse {
                    codec_version: 1,
                    response_fingerprint: "late".to_string(),
                    response_aggregate: "{\"late\":true}".to_string(),
                },
                provider_request_id: Some("provider-late".to_string()),
                tool_intents: vec![],
            })
            .await
            .unwrap()
            .outcome,
            CompleteLlmResponsePersistenceOutcome::StaleAuthority
        );
        assert!(repo
            .load_owed_top_level_llm_receipts()
            .await
            .unwrap()
            .is_empty());
        assert!(repo
            .recover_top_level_llm_attempts(ProcessIncarnation(10))
            .await
            .unwrap()
            .is_empty());
        assert_eq!(
            repo.load_tool_intents(WorkflowId(1), ReceiptId(1))
                .await
                .unwrap()[0]
                .status,
            ToolIntentStatus::Interrupted
        );
    }
}
