use super::{
    local_codec, parse_attempt_status, parse_delivery_payload_kind, parse_delivery_status,
    parse_receipt_origin, parse_runtime_acceptance_status, parse_suppression_reason, to_i64,
    to_u32, to_u64, AcceptReceiptInput, AttemptId, AuthorityOutcome, BeginAttemptInput,
    BeginAttemptResult, CommitOutcome, CommitTransitionPlanCas,
    CreateWorkflowWithExternalAcceptance, DbError, DbResult, DeliveryId,
    DeliveryResolutionDecision, DeliveryResolutionPlan, EffectId, Generation, LeaseExpiry,
    LocalAttemptAuthority, LocalAttemptRecord, LocalCodec, LocalDeliveryRecord, LocalEffectDecl,
    LocalReceiptRecord, LocalReclaimableLease, ProcessIncarnation, ReceiptId, ReceiptOrigin,
    SuppressionReason, Timestamp, TransitionId, Version, WorkflowId, WorkflowRepository,
    WorkflowStatus, WorkflowTx,
};
use phoenix_workflow::llm_profile;
use phoenix_workflow::llm_profile::{
    CompleteLlmResponse, LlmEffectKey, PreparedLlmRequest, TopLevelLlmSnapshot,
};
use phoenix_workflow::{CodecRef, EffectRole, EffectStatus, ExecutionCapability};
use sqlx::SqlitePool;

#[cfg(test)]
use phoenix_workflow::llm_profile::TopLevelTurnRef;
#[cfg(test)]
use phoenix_workflow::ClaimOutcome;
use serde::{Deserialize, Serialize};
use sqlx::Row;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DirectTurnCommittedOutcome {
    PendingRuntime,
    RuntimeAccepted,
    QueuedSteering,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectTurnAcceptanceInput {
    pub conversation_id: String,
    pub client_message_id: String,
    pub prepared_fingerprint: String,
    pub prepared_payload: String,
    pub committed_outcome: DirectTurnCommittedOutcome,
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
    Conflict,
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
pub struct RecoverTopLevelLlmAttempt {
    pub workflow: TopLevelLlmWorkflowRecord,
    pub prepared_request: TopLevelLlmPreparedRequestRecord,
    pub attempt: LocalAttemptRecord,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptCompleteLlmResponseInput {
    pub authority: LocalAttemptAuthority,
    pub delivery_id: DeliveryId,
    pub receipt_id: ReceiptId,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptCompleteLlmResponseResult {
    pub outcome: AuthorityOutcome,
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
pub struct OwedTopLevelLlmReceipt {
    pub workflow: TopLevelLlmWorkflowRecord,
    pub prepared_request: TopLevelLlmPreparedRequestRecord,
    pub receipt: LocalReceiptRecord,
    pub llm_receipt: LlmResponseReceiptRecord,
    pub delivery: LocalDeliveryRecord,
    pub tool_intents: Vec<ToolIntentRecord>,
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
    pub async fn accept_direct_turn(
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
        let acceptance_insert = sqlx::query("INSERT INTO direct_turn_acceptances (conversation_id, client_message_id, workflow_id, prepared_fingerprint, prepared_payload, committed_outcome, accepted_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)")
            .bind(&input.conversation_id)
            .bind(&input.client_message_id)
            .bind(to_i64(workflow_id.0, "workflow_id")?)
            .bind(&input.prepared_fingerprint)
            .bind(&input.prepared_payload)
            .bind(direct_turn_outcome_to_str(&input.committed_outcome))
            .bind(to_i64(input.accepted_at.0, "accepted_at")?)
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
        tx.commit().await?;
        Ok(DirectTurnAcceptanceOutcome::Created(
            DirectTurnAcceptanceRecord {
                workflow_id,
                conversation_id: input.conversation_id.clone(),
                client_message_id: input.client_message_id.clone(),
                prepared_fingerprint: input.prepared_fingerprint.clone(),
                prepared_payload: input.prepared_payload.clone(),
                committed_outcome: input.committed_outcome.clone(),
                accepted_at: input.accepted_at,
            },
        ))
    }

    pub async fn mark_direct_turn_runtime_accepted(
        &self,
        conversation_id: &str,
        client_message_id: &str,
    ) -> DbResult<bool> {
        let updated = sqlx::query(
            "UPDATE direct_turn_acceptances SET committed_outcome = 'RuntimeAccepted' WHERE conversation_id = ?1 AND client_message_id = ?2 AND committed_outcome = 'PendingRuntime'",
        )
        .bind(conversation_id)
        .bind(client_message_id)
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected() == 1)
    }

    pub async fn load_pending_direct_turns(&self) -> DbResult<Vec<DirectTurnAcceptanceRecord>> {
        let rows = sqlx::query(
            "SELECT conversation_id, client_message_id, workflow_id, prepared_fingerprint, prepared_payload, committed_outcome, accepted_at FROM direct_turn_acceptances WHERE committed_outcome = 'PendingRuntime' ORDER BY accepted_at, conversation_id, client_message_id",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| direct_turn_record_from_row(&row))
            .collect()
    }

    pub async fn prepare_top_level_llm_request(
        &self,
        input: &PrepareTopLevelLlmRequestInput,
    ) -> DbResult<CommitOutcome> {
        let mut tx = self.begin_tx().await?;
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
                    .map_err(|e| DbError::Serialization(e.to_string()))?,
                next_snapshot_codec: local_codec_ref_to_owned(&llm_profile::snapshot_codec()),
                next_snapshot_payload: serde_json::to_vec(&input.snapshot)
                    .map_err(|e| DbError::Serialization(e.to_string()))?,
                committed_at: input.committed_at,
                effects: vec![LocalEffectDecl {
                    effect_id: input.effect_id,
                    declared_workflow_version: input.expected_version.next(),
                    family: "llm.call".to_string(),
                    kind: "top_level_call".to_string(),
                    intent_codec: local_codec_ref_to_owned(&llm_profile::intent_codec()),
                    intent_payload: serde_json::to_vec(&intent)
                        .map_err(|e| DbError::Serialization(e.to_string()))?,
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
            tx.rollback().await?;
            return Ok(outcome);
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
        tx.commit().await?;
        Ok(CommitOutcome::Committed)
    }

    pub async fn begin_top_level_llm_attempt(
        &self,
        input: &BeginAttemptInput,
    ) -> DbResult<BeginAttemptResult> {
        self.begin_attempt(input).await
    }

    pub async fn recover_top_level_llm_attempts(&self) -> DbResult<Vec<RecoverTopLevelLlmAttempt>> {
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
             ORDER BY w.workflow_id, a.attempt_id"
        )
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
        let receipt_payload = serde_json::to_vec(&llm_profile::LlmResponseReceipt {
            key: LlmEffectKey {
                accepted_turn_id: load_accepted_turn_id(&self.pool, input.authority.workflow_id)
                    .await?,
                generation: input.authority.generation.0,
                call_ordinal: load_call_ordinal(
                    &self.pool,
                    input.authority.workflow_id,
                    input.authority.effect_id,
                )
                .await?,
            },
            response: input.response.clone(),
            generation: input.authority.generation.0,
        })
        .map_err(|e| DbError::Serialization(e.to_string()))?;
        let receipt_input = AcceptReceiptInput {
            authority: input.authority.clone(),
            receipt_id: input.receipt_id,
            delivery_id: input.delivery_id,
            attempt_id: Some(input.authority.attempt_id),
            origin: ReceiptOrigin::Execution,
            receipt_codec: local_codec_ref_to_owned(&llm_profile::receipt_codec()),
            receipt_payload,
            receipt_event_codec: local_codec_ref_to_owned(&llm_profile::receipt_codec()),
            receipt_event_payload: serde_json::to_vec(&llm_profile::LlmResponseReceipt {
                key: LlmEffectKey {
                    accepted_turn_id: load_accepted_turn_id(
                        &self.pool,
                        input.authority.workflow_id,
                    )
                    .await?,
                    generation: input.authority.generation.0,
                    call_ordinal: load_call_ordinal(
                        &self.pool,
                        input.authority.workflow_id,
                        input.authority.effect_id,
                    )
                    .await?,
                },
                response: input.response.clone(),
                generation: input.authority.generation.0,
            })
            .map_err(|e| DbError::Serialization(e.to_string()))?,
            receipt_event_requires_runtime_acceptance: true,
            request_runtime_acceptance_for_cancellation: false,
        };
        let mut tx = self.begin_tx().await?;
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
                        outcome: generic.outcome,
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
                    outcome: AuthorityOutcome::Authorized,
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
                            d.delivery_id == input.delivery_id
                                || d.effect_id == Some(input.authority.effect_id)
                        }),
                    llm_receipt: Some(llm_receipt),
                    tool_intents: intents,
                });
            }
            return Ok(AcceptCompleteLlmResponseResult {
                outcome: generic.outcome,
                receipt: None,
                delivery: None,
                llm_receipt: None,
                tool_intents: vec![],
            });
        }
        sqlx::query("INSERT INTO top_level_llm_response_receipts (workflow_id, receipt_id, effect_id, codec_version, response_fingerprint, response_aggregate, provider_request_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)")
            .bind(to_i64(input.authority.workflow_id.0, "workflow_id")?)
            .bind(to_i64(input.receipt_id.0, "receipt_id")?)
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
                .bind(to_i64(input.receipt_id.0, "receipt_id")?)
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
            outcome: AuthorityOutcome::Authorized,
            receipt: generic.receipt,
            delivery: generic.delivery,
            llm_receipt: Some(LlmResponseReceiptRecord {
                workflow_id: input.authority.workflow_id,
                receipt_id: input.receipt_id,
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
             WHERE d.runtime_acceptance_status = 'Owed'
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
    if existing.prepared_fingerprint == input.prepared_fingerprint
        && existing.prepared_payload == input.prepared_payload
        && existing.committed_outcome == input.committed_outcome
    {
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
        "SELECT conversation_id, client_message_id, workflow_id, prepared_fingerprint, prepared_payload, committed_outcome, accepted_at FROM direct_turn_acceptances WHERE conversation_id = ?1 AND client_message_id = ?2",
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

async fn load_accepted_turn_id(pool: &SqlitePool, workflow_id: WorkflowId) -> DbResult<String> {
    sqlx::query_scalar::<_, String>(
        "SELECT client_message_id FROM direct_turn_acceptances WHERE workflow_id = ?1",
    )
    .bind(to_i64(workflow_id.0, "workflow_id")?)
    .fetch_one(pool)
    .await
    .map_err(DbError::from)
}

async fn load_call_ordinal(
    pool: &SqlitePool,
    workflow_id: WorkflowId,
    effect_id: EffectId,
) -> DbResult<u64> {
    let value = sqlx::query_scalar::<_, i64>(
        "SELECT call_ordinal FROM top_level_llm_effects WHERE workflow_id = ?1 AND effect_id = ?2",
    )
    .bind(to_i64(workflow_id.0, "workflow_id")?)
    .bind(to_i64(effect_id.0, "effect_id")?)
    .fetch_one(pool)
    .await?;
    to_u64(value, "call_ordinal")
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

fn direct_turn_outcome_to_str(value: &DirectTurnCommittedOutcome) -> &'static str {
    match value {
        DirectTurnCommittedOutcome::PendingRuntime => "PendingRuntime",
        DirectTurnCommittedOutcome::RuntimeAccepted => "RuntimeAccepted",
        DirectTurnCommittedOutcome::QueuedSteering => "QueuedSteering",
    }
}

fn parse_direct_turn_outcome(value: &str) -> DbResult<DirectTurnCommittedOutcome> {
    match value {
        "PendingRuntime" => Ok(DirectTurnCommittedOutcome::PendingRuntime),
        "RuntimeAccepted" => Ok(DirectTurnCommittedOutcome::RuntimeAccepted),
        "QueuedSteering" => Ok(DirectTurnCommittedOutcome::QueuedSteering),
        other => Err(DbError::Serialization(format!(
            "unknown direct-turn outcome: {other}"
        ))),
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
    async fn direct_turn_accept_replay_conflict() {
        let repo = open_repo().await;
        let input = DirectTurnAcceptanceInput {
            conversation_id: "conv-1".to_string(),
            client_message_id: "msg-1".to_string(),
            prepared_fingerprint: "fp-1".to_string(),
            prepared_payload: "{}".to_string(),
            committed_outcome: DirectTurnCommittedOutcome::PendingRuntime,
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
        let pending = repo.load_pending_direct_turns().await.unwrap();
        assert_eq!(pending.len(), 1);
        assert!(repo
            .mark_direct_turn_runtime_accepted("conv-1", "msg-1")
            .await
            .unwrap());
        assert!(repo.load_pending_direct_turns().await.unwrap().is_empty());
        let mut conflict = input.clone();
        conflict.prepared_fingerprint = "fp-2".to_string();
        assert_eq!(
            repo.accept_direct_turn(&conflict).await.unwrap(),
            DirectTurnAcceptanceOutcome::Conflict
        );
    }

    #[tokio::test]
    async fn prepare_begin_recover_accept_owed_and_stop_flow() {
        let repo = open_repo().await;
        repo.accept_direct_turn(&DirectTurnAcceptanceInput {
            conversation_id: "conv-1".to_string(),
            client_message_id: "msg-1".to_string(),
            prepared_fingerprint: "fp-1".to_string(),
            prepared_payload: "{}".to_string(),
            committed_outcome: DirectTurnCommittedOutcome::PendingRuntime,
            accepted_at: Timestamp(1),
            snapshot: snapshot(),
        })
        .await
        .unwrap();
        repo.mark_direct_turn_runtime_accepted("conv-1", "msg-1")
            .await
            .unwrap();
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
        assert_eq!(
            repo.recover_top_level_llm_attempts().await.unwrap().len(),
            1
        );
        let result = repo
            .accept_complete_top_level_llm_response(&AcceptCompleteLlmResponseInput {
                authority: begun.authority.unwrap(),
                delivery_id: DeliveryId(1),
                receipt_id: ReceiptId(1),
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
        assert_eq!(result.outcome, AuthorityOutcome::Authorized);
        assert_eq!(
            repo.load_owed_top_level_llm_receipts().await.unwrap().len(),
            1
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
                delivery_id: DeliveryId(1),
                receipt_id: ReceiptId(1),
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
            AuthorityOutcome::Authorized
        );
        let stopped = repo
            .stop_top_level_llm_and_suppress_pending_delivery(&StopTopLevelLlmInput {
                workflow_id: WorkflowId(1),
                stopped_at: Timestamp(10),
                expected_version: Version(999),
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
        assert_eq!(stopped, CommitOutcome::VersionConflict);
    }
}
