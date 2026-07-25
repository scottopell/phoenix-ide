use super::WorkflowRepository;
use crate::{DbError, DbResult};
use chrono::{DateTime, Utc};
use phoenix_core::domain::db_schema::{Message, MessageContent};
use phoenix_core::domain::sm_event::{DirectTurnAttemptAuthority, PreparedDirectTurnPayload};
use phoenix_workflow::{
    direct_turn_profile, AcceptedDisposition, AttemptId, AttemptStatus, AuthorityOutcome,
    CanonicalMessageId, ClaimOutcome, ClientTurnKey, ConversationAuthority, DeliveryId,
    DurableTurn, EffectId, EffectRole, EffectStatus, ExecutionCapability, Generation, LeaseExpiry,
    Materialization, PreparedTurn, ProcessIncarnation, ReceiptId, Timestamp, TurnAuthorityId,
    TurnCommand, TurnConflict, TurnLifecycle, TurnOutcome, TurnStep, TurnTerminal, Version,
    WorkflowId, WorkflowStatus,
};
use sqlx::Row;

use super::{
    CommitTransitionPlanCas, CreateWorkflowWithExternalAcceptance, LocalCodec, LocalEffectDecl,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransactionCut {
    None,
    BeforeCommit,
    AfterCommit,
}

const DIRECT_TURN_ACCEPTED_TRANSITION_ID: u64 = 1;
const DIRECT_TURN_TERMINAL_TRANSITION_ID: u64 = 2;
const DIRECT_TURN_EFFECT_ID: u64 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptAuthoritativeTurn {
    pub conversation: ConversationAuthority,
    pub client_key: ClientTurnKey,
    pub prepared: PreparedTurn,
    pub disposition: AcceptedDisposition,
    pub accepted_at: phoenix_workflow::Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimAuthoritativeTurnInput {
    pub turn_id: TurnAuthorityId,
    pub workflow_id: WorkflowId,
    pub process_incarnation: ProcessIncarnation,
    pub now: Timestamp,
    pub lease_until: LeaseExpiry,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimAuthoritativeTurnResult {
    pub outcome: ClaimOutcome,
    pub authority: Option<super::LocalAttemptAuthority>,
    pub attempt: Option<super::LocalAttemptRecord>,
    pub canonical_turn: Option<DurableTurn>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseAuthoritativeTurnInput {
    pub authority: super::LocalAttemptAuthority,
    pub now: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirectTurnDiscoveryCursor {
    pub turn_id: TurnAuthorityId,
    pub workflow_id: WorkflowId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoverableAcceptedTurn {
    pub turn_id: TurnAuthorityId,
    pub workflow_id: WorkflowId,
    pub conversation: ConversationAuthority,
    pub prepared: PreparedTurn,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreflightDirectTurnMaterializationInput {
    pub turn_id: TurnAuthorityId,
    pub authority: super::LocalAttemptAuthority,
    pub prepared: PreparedDirectTurnPayload,
    pub now: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectTurnMaterializationEligibility {
    Fresh,
    ExactReplay,
    StaleAuthority,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterializeAuthoritativeTurnInput {
    pub turn_id: TurnAuthorityId,
    pub authority: super::LocalAttemptAuthority,
    pub prepared: PreparedDirectTurnPayload,
    pub sequence_id: i64,
    pub created_at: Timestamp,
    pub now: Timestamp,
}

#[derive(Debug, Clone)]
pub struct MaterializeAuthoritativeTurnResult {
    pub outcome: TurnOutcome,
    pub authority_outcome: AuthorityOutcome,
    pub receipt: Option<super::LocalReceiptRecord>,
    pub delivery: Option<super::LocalDeliveryRecord>,
    pub canonical_turn: DurableTurn,
    pub message: Option<Message>,
}

fn authority_event(
    authority: &super::LocalAttemptAuthority,
    turn_id: TurnAuthorityId,
) -> DirectTurnAttemptAuthority {
    DirectTurnAttemptAuthority::new(
        authority.workflow_id.0,
        turn_id.0,
        authority.effect_id.0,
        authority.attempt_id.0,
        authority.declared_workflow_version.0,
        authority.generation.0,
        authority.process_incarnation.0,
    )
}

impl WorkflowRepository {
    pub async fn accept_authoritative_turn(
        &self,
        input: &AcceptAuthoritativeTurn,
    ) -> DbResult<TurnStep> {
        self.accept_authoritative_turn_at_cut(input, TransactionCut::None)
            .await
    }

    async fn accept_authoritative_turn_at_cut(
        &self,
        input: &AcceptAuthoritativeTurn,
        cut: TransactionCut,
    ) -> DbResult<TurnStep> {
        let mut tx = self.begin_tx().await?;
        if let Some(existing) =
            load_by_scoped_key(&mut tx.tx, &input.conversation, &input.client_key).await?
        {
            tx.rollback().await?;
            if existing.prepared != input.prepared
                || existing.lifecycle
                    != (TurnLifecycle::Accepted {
                        disposition: input.disposition,
                    })
            {
                return Err(conflict(TurnConflict::PreparedSemanticsChanged));
            }
            return Ok(TurnStep {
                outcome: TurnOutcome::ExactReplay {
                    turn_id: existing.id,
                    disposition: input.disposition,
                },
                owed_effects: Vec::new(),
            });
        }
        if input.disposition == AcceptedDisposition::Runtime {
            if let Some(owner) = sqlx::query_scalar::<_, i64>(
                "SELECT turn_id FROM durable_turns WHERE conversation_id = ?1 AND owns_conversation = 1",
            )
            .bind(&input.conversation.0)
            .fetch_optional(&mut *tx.tx)
            .await?
            {
                tx.rollback().await?;
                return Err(conflict(TurnConflict::ConversationAlreadyOwned {
                    owner: TurnAuthorityId(to_u64(owner, "turn_id")?),
                }));
            }
        }
        let next_id =
            sqlx::query_scalar::<_, i64>("SELECT COALESCE(MAX(turn_id), 0) + 1 FROM durable_turns")
                .fetch_one(&mut *tx.tx)
                .await?;
        let turn_id = TurnAuthorityId(to_u64(next_id, "turn_id")?);
        let workflow_id = next_global_workflow_id_tx(&mut tx).await?;
        insert_direct_turn_workflow_tx(&mut tx, workflow_id, turn_id, input).await?;
        let disposition = disposition_sql(input.disposition);
        sqlx::query(
            "INSERT INTO durable_turns (
                turn_id, conversation_id, client_turn_key, prepared_fingerprint,
                prepared_payload, disposition, generation, terminal_kind,
                terminal_reason, owns_conversation, canonical_message_id, workflow_id
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, NULL, NULL, ?7, NULL, ?8)",
        )
        .bind(to_i64(turn_id.0, "turn_id")?)
        .bind(&input.conversation.0)
        .bind(input.client_key.as_str())
        .bind(input.prepared.fingerprint())
        .bind(input.prepared.payload())
        .bind(disposition)
        .bind(i64::from(input.disposition == AcceptedDisposition::Runtime))
        .bind(to_i64(workflow_id.0, "workflow_id")?)
        .execute(&mut *tx.tx)
        .await
        .map_err(map_constraint)?;
        if cut == TransactionCut::BeforeCommit {
            tx.rollback().await?;
            return Err(injected_cut(cut));
        }
        tx.commit().await?;
        if cut == TransactionCut::AfterCommit {
            return Err(injected_cut(cut));
        }
        let mut model = phoenix_workflow::DurableTurnModel::default();
        model
            .apply(TurnCommand::Accept {
                turn_id,
                conversation: input.conversation.clone(),
                client_key: input.client_key.clone(),
                prepared: input.prepared.clone(),
                disposition: input.disposition,
            })
            .map_err(conflict)
    }

    pub async fn load_authoritative_turn(
        &self,
        turn_id: TurnAuthorityId,
    ) -> DbResult<Option<DurableTurn>> {
        let row = sqlx::query("SELECT * FROM durable_turns WHERE turn_id = ?1")
            .bind(to_i64(turn_id.0, "turn_id")?)
            .fetch_optional(&self.pool)
            .await?;
        row.map(row_to_turn).transpose()
    }

    pub async fn workflow_id_for_turn(
        &self,
        turn_id: TurnAuthorityId,
    ) -> DbResult<Option<WorkflowId>> {
        let workflow_id = sqlx::query_scalar::<_, Option<i64>>(
            "SELECT workflow_id FROM durable_turns WHERE turn_id = ?1",
        )
        .bind(to_i64(turn_id.0, "turn_id")?)
        .fetch_optional(&self.pool)
        .await?
        .flatten();
        workflow_id
            .map(|value| to_u64(value, "workflow_id").map(WorkflowId))
            .transpose()
    }

    pub async fn claim_authoritative_turn(
        &self,
        input: &ClaimAuthoritativeTurnInput,
    ) -> DbResult<ClaimAuthoritativeTurnResult> {
        let mut tx = self.begin_tx().await?;
        let Some(canonical_turn) =
            load_turn_for_workflow_tx(&mut tx.tx, input.turn_id, input.workflow_id).await?
        else {
            tx.rollback().await?;
            return Ok(ClaimAuthoritativeTurnResult {
                outcome: ClaimOutcome::Ineligible,
                authority: None,
                attempt: None,
                canonical_turn: None,
            });
        };
        let Some(existing_live_attempt) =
            load_live_attempt_tx(&mut tx.tx, input.workflow_id, DIRECT_TURN_EFFECT_ID).await?
        else {
            let attempt_id = next_attempt_id_tx(&mut tx).await?;
            let result = tx
                .begin_attempt(&super::BeginAttemptInput {
                    workflow_id: input.workflow_id,
                    effect_id: EffectId(DIRECT_TURN_EFFECT_ID),
                    attempt_id,
                    process_incarnation: input.process_incarnation,
                    now: input.now,
                    lease_until: Some(input.lease_until),
                })
                .await?;
            if result.outcome == ClaimOutcome::Started {
                tx.commit().await?;
            } else {
                tx.rollback().await?;
            }
            return Ok(ClaimAuthoritativeTurnResult {
                outcome: result.outcome,
                authority: result.authority,
                attempt: result.attempt,
                canonical_turn: Some(canonical_turn),
            });
        };
        if let Some(lease_until) = existing_live_attempt
            .lease
            .as_ref()
            .map(|lease| lease.lease_until)
        {
            if lease_until.is_live_at(input.now) {
                tx.rollback().await?;
                return Ok(ClaimAuthoritativeTurnResult {
                    outcome: ClaimOutcome::AuthorityConflict,
                    authority: None,
                    attempt: None,
                    canonical_turn: Some(canonical_turn),
                });
            }
        }
        let expired = expire_direct_turn_lease_in_tx(
            &mut tx,
            &super::ExpireLeaseInput {
                workflow_id: input.workflow_id,
                effect_id: EffectId(DIRECT_TURN_EFFECT_ID),
                attempt_id: existing_live_attempt.id,
                now: input.now,
            },
        )
        .await?;
        if expired != AuthorityOutcome::Authorized {
            tx.rollback().await?;
            return Ok(ClaimAuthoritativeTurnResult {
                outcome: ClaimOutcome::Ineligible,
                authority: None,
                attempt: None,
                canonical_turn: Some(canonical_turn),
            });
        }
        let attempt_id = next_attempt_id_tx(&mut tx).await?;
        let result = tx
            .begin_attempt(&super::BeginAttemptInput {
                workflow_id: input.workflow_id,
                effect_id: EffectId(DIRECT_TURN_EFFECT_ID),
                attempt_id,
                process_incarnation: input.process_incarnation,
                now: input.now,
                lease_until: Some(input.lease_until),
            })
            .await?;
        if result.outcome == ClaimOutcome::Started {
            tx.commit().await?;
        } else {
            tx.rollback().await?;
        }
        Ok(ClaimAuthoritativeTurnResult {
            outcome: result.outcome,
            authority: result.authority,
            attempt: result.attempt,
            canonical_turn: Some(canonical_turn),
        })
    }

    pub async fn release_authoritative_turn_dispatch_failure(
        &self,
        input: &ReleaseAuthoritativeTurnInput,
    ) -> DbResult<AuthorityOutcome> {
        let mut tx = self.begin_tx().await?;
        let updated = sqlx::query(
            "DELETE FROM workflow_reclaimable_leases
             WHERE workflow_id = ?1 AND attempt_id = ?2 AND lease_until > ?3
               AND EXISTS (
                   SELECT 1 FROM workflow_attempts a
                   WHERE a.workflow_id = workflow_reclaimable_leases.workflow_id
                     AND a.attempt_id = workflow_reclaimable_leases.attempt_id
                     AND a.effect_id = ?4
                     AND a.declared_workflow_version = ?5
                     AND a.generation = ?6
                     AND a.process_incarnation = ?7
                     AND a.status IN ('Begun', 'ObservationRecorded')
               )",
        )
        .bind(to_i64(input.authority.workflow_id.0, "workflow_id")?)
        .bind(to_i64(input.authority.attempt_id.0, "attempt_id")?)
        .bind(to_i64(input.now.0, "now")?)
        .bind(to_i64(input.authority.effect_id.0, "effect_id")?)
        .bind(to_i64(
            input.authority.declared_workflow_version.0,
            "declared_workflow_version",
        )?)
        .bind(to_i64(input.authority.generation.0, "generation")?)
        .bind(to_i64(
            input.authority.process_incarnation.0,
            "process_incarnation",
        )?)
        .execute(&mut *tx.tx)
        .await?
        .rows_affected();
        if updated == 0 {
            tx.rollback().await?;
            return Ok(AuthorityOutcome::StaleAuthority);
        }
        let attempts_updated = sqlx::query(
            "UPDATE workflow_attempts
             SET status = 'AuthorityLost'
             WHERE workflow_id = ?1 AND attempt_id = ?2 AND effect_id = ?3
               AND declared_workflow_version = ?4 AND generation = ?5
               AND process_incarnation = ?6 AND status IN ('Begun', 'ObservationRecorded')",
        )
        .bind(to_i64(input.authority.workflow_id.0, "workflow_id")?)
        .bind(to_i64(input.authority.attempt_id.0, "attempt_id")?)
        .bind(to_i64(input.authority.effect_id.0, "effect_id")?)
        .bind(to_i64(
            input.authority.declared_workflow_version.0,
            "declared_workflow_version",
        )?)
        .bind(to_i64(input.authority.generation.0, "generation")?)
        .bind(to_i64(
            input.authority.process_incarnation.0,
            "process_incarnation",
        )?)
        .execute(&mut *tx.tx)
        .await?
        .rows_affected();
        let effects_updated = sqlx::query(
            "UPDATE workflow_effects
             SET status = 'Eligible'
             WHERE workflow_id = ?1 AND effect_id = ?2
               AND declared_workflow_version = ?3 AND generation = ?4
               AND status = 'Executing'",
        )
        .bind(to_i64(input.authority.workflow_id.0, "workflow_id")?)
        .bind(to_i64(input.authority.effect_id.0, "effect_id")?)
        .bind(to_i64(
            input.authority.declared_workflow_version.0,
            "declared_workflow_version",
        )?)
        .bind(to_i64(input.authority.generation.0, "generation")?)
        .execute(&mut *tx.tx)
        .await?
        .rows_affected();
        if attempts_updated != 1 || effects_updated != 1 {
            tx.rollback().await?;
            return Ok(AuthorityOutcome::StaleAuthority);
        }
        tx.commit().await?;
        Ok(AuthorityOutcome::Authorized)
    }

    pub async fn list_discoverable_accepted_turns(
        &self,
        conversation: &ConversationAuthority,
    ) -> DbResult<Vec<(TurnAuthorityId, WorkflowId)>> {
        let rows = sqlx::query(
            "SELECT turn_id, workflow_id FROM durable_turns
             WHERE conversation_id = ?1 AND terminal_kind IS NULL AND workflow_id IS NOT NULL
             ORDER BY turn_id",
        )
        .bind(&conversation.0)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok((
                    TurnAuthorityId(to_u64(row.get("turn_id"), "turn_id")?),
                    WorkflowId(to_u64(row.get("workflow_id"), "workflow_id")?),
                ))
            })
            .collect()
    }

    pub async fn list_discoverable_accepted_runtime_direct_turns(
        &self,
        cursor: Option<DirectTurnDiscoveryCursor>,
        limit: usize,
    ) -> DbResult<Vec<DiscoverableAcceptedTurn>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let capped_limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let (cursor_turn_id, cursor_workflow_id) = cursor.map_or((0_i64, 0_i64), |cursor| {
            (
                i64::try_from(cursor.turn_id.0).unwrap_or(i64::MAX),
                i64::try_from(cursor.workflow_id.0).unwrap_or(i64::MAX),
            )
        });
        let rows = sqlx::query(
            "SELECT turn_id, workflow_id, conversation_id, prepared_fingerprint, prepared_payload
             FROM durable_turns
             WHERE disposition = 'Runtime'
               AND terminal_kind IS NULL
               AND canonical_message_id IS NULL
               AND workflow_id IS NOT NULL
               AND (turn_id > ?1 OR (turn_id = ?1 AND workflow_id > ?2))
             ORDER BY turn_id, workflow_id
             LIMIT ?3",
        )
        .bind(cursor_turn_id)
        .bind(cursor_workflow_id)
        .bind(capped_limit)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(DiscoverableAcceptedTurn {
                    turn_id: TurnAuthorityId(to_u64(row.get("turn_id"), "turn_id")?),
                    workflow_id: WorkflowId(to_u64(row.get("workflow_id"), "workflow_id")?),
                    conversation: ConversationAuthority(row.get("conversation_id")),
                    prepared: PreparedTurn {
                        fingerprint: row.get("prepared_fingerprint"),
                        payload: row.get("prepared_payload"),
                    },
                })
            })
            .collect()
    }

    pub async fn preflight_direct_turn_materialization(
        &self,
        input: &PreflightDirectTurnMaterializationInput,
    ) -> DbResult<DirectTurnMaterializationEligibility> {
        let mut tx = self.begin_tx().await?;
        let eligibility = self
            .preflight_direct_turn_materialization_tx(&mut tx, input)
            .await?;
        tx.rollback().await?;
        Ok(eligibility)
    }

    async fn preflight_direct_turn_materialization_tx(
        &self,
        tx: &mut super::WorkflowTx<'_>,
        input: &PreflightDirectTurnMaterializationInput,
    ) -> DbResult<DirectTurnMaterializationEligibility> {
        let Some(row) = sqlx::query(
            "SELECT * FROM durable_turns
             WHERE turn_id = ?1 AND workflow_id = ?2 AND workflow_id IS NOT NULL",
        )
        .bind(to_i64(input.turn_id.0, "turn_id")?)
        .bind(to_i64(input.authority.workflow_id.0, "workflow_id")?)
        .fetch_optional(&mut *tx.tx)
        .await?
        else {
            return Ok(DirectTurnMaterializationEligibility::StaleAuthority);
        };
        let turn = row_to_turn(row)?;
        verify_prepared_payload(&turn, &input.prepared)?;
        if !matches!(turn.lifecycle, TurnLifecycle::Accepted { .. })
            || turn.generation != input.authority.generation.0
        {
            return Ok(DirectTurnMaterializationEligibility::StaleAuthority);
        }
        if let Materialization::Materialized { message_id } = &turn.materialization {
            let message = load_message_by_id_tx(&mut tx.tx, &message_id.0).await?;
            verify_existing_materialized_message_without_sequence(
                &message,
                &turn,
                &input.prepared,
            )?;
            return Ok(DirectTurnMaterializationEligibility::ExactReplay);
        }

        let Some(authority_row) = sqlx::query(
            "SELECT e.status AS effect_status, a.status AS attempt_status, l.lease_until
             FROM workflow_effects e
             JOIN workflow_attempts a
               ON a.workflow_id = e.workflow_id AND a.attempt_id = ?3
             LEFT JOIN workflow_reclaimable_leases l
               ON l.workflow_id = a.workflow_id AND l.attempt_id = a.attempt_id
             WHERE e.workflow_id = ?1 AND e.effect_id = ?2
               AND e.declared_workflow_version = ?4 AND e.generation = ?5
               AND a.effect_id = ?2 AND a.declared_workflow_version = ?4
               AND a.generation = ?5 AND a.process_incarnation = ?6",
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
        .fetch_optional(&mut *tx.tx)
        .await?
        else {
            return Ok(DirectTurnMaterializationEligibility::StaleAuthority);
        };
        if authority_row.get::<String, _>("effect_status") != "Executing"
            || !matches!(
                authority_row.get::<String, _>("attempt_status").as_str(),
                "Begun" | "ObservationRecorded"
            )
            || authority_row
                .get::<Option<i64>, _>("lease_until")
                .and_then(|value| to_u64(value, "lease_until").ok())
                .is_none_or(|lease_until| !LeaseExpiry(lease_until).is_live_at(input.now))
        {
            return Ok(DirectTurnMaterializationEligibility::StaleAuthority);
        }
        Ok(DirectTurnMaterializationEligibility::Fresh)
    }

    pub async fn materialize_authoritative_turn(
        &self,
        input: &MaterializeAuthoritativeTurnInput,
    ) -> DbResult<MaterializeAuthoritativeTurnResult> {
        self.materialize_authoritative_turn_at_cut(input, TransactionCut::None)
            .await
    }

    async fn materialize_authoritative_turn_at_cut(
        &self,
        input: &MaterializeAuthoritativeTurnInput,
        cut: TransactionCut,
    ) -> DbResult<MaterializeAuthoritativeTurnResult> {
        let mut tx = self.begin_tx().await?;
        let turn_id = input.turn_id;
        let turn = load_turn_for_workflow_tx(&mut tx.tx, turn_id, input.authority.workflow_id)
            .await?
            .ok_or_else(|| conflict(TurnConflict::UnknownTurn))?;
        verify_prepared_payload(&turn, &input.prepared)?;
        let canonical_message_id = CanonicalMessageId(input.prepared.message_id.clone());
        let mut model =
            phoenix_workflow::DurableTurnModel::from_turns([turn.clone()]).map_err(conflict)?;
        let step = model
            .apply(TurnCommand::Materialize {
                turn_id,
                expected_generation: input.authority.generation.0,
                message_id: canonical_message_id.clone(),
            })
            .map_err(conflict)?;
        let receipt_id = next_workflow_sequence_tx(&mut tx, input.authority.workflow_id, "receipt")
            .await
            .map(ReceiptId)?;
        let delivery_id =
            next_workflow_sequence_tx(&mut tx, input.authority.workflow_id, "delivery")
                .await
                .map(DeliveryId)?;
        let acceptance = tx
            .accept_receipt_and_delivery(&super::AcceptReceiptInput {
                authority: input.authority.clone(),
                receipt_id,
                delivery_id,
                attempt_id: Some(input.authority.attempt_id),
                origin: phoenix_workflow::ReceiptOrigin::Execution,
                receipt_codec: local_codec_owned(&direct_turn_profile::receipt_codec()),
                receipt_payload: serde_json::to_vec(&authority_event(&input.authority, turn_id))
                    .map_err(|e| {
                        DbError::Serialization(format!("encode direct-turn receipt: {e}"))
                    })?,
                receipt_event_codec: local_codec_owned(&direct_turn_profile::receipt_event_codec()),
                receipt_event_payload: serde_json::to_vec(&authority_event(
                    &input.authority,
                    turn_id,
                ))
                .map_err(|e| {
                    DbError::Serialization(format!("encode direct-turn receipt event: {e}"))
                })?,
                receipt_event_requires_runtime_acceptance: false,
                request_runtime_acceptance_for_cancellation: false,
            })
            .await?;
        if acceptance.outcome != AuthorityOutcome::Authorized {
            tx.rollback().await?;
            return Ok(MaterializeAuthoritativeTurnResult {
                outcome: step.outcome,
                authority_outcome: acceptance.outcome,
                receipt: None,
                delivery: None,
                canonical_turn: turn,
                message: None,
            });
        }
        let message = if matches!(step.outcome, TurnOutcome::Materialized { .. }) {
            let message = insert_canonical_message_tx(
                &mut tx,
                &turn,
                &canonical_message_id,
                input.sequence_id,
                input.created_at,
                &input.prepared,
            )
            .await?;
            let updated = sqlx::query(
                "UPDATE durable_turns SET canonical_message_id = ?2
                 WHERE turn_id = ?1 AND generation = ?3 AND terminal_kind IS NULL
                   AND canonical_message_id IS NULL AND workflow_id = ?4",
            )
            .bind(to_i64(turn_id.0, "turn_id")?)
            .bind(&message.message_id)
            .bind(to_i64(input.authority.generation.0, "generation")?)
            .bind(to_i64(input.authority.workflow_id.0, "workflow_id")?)
            .execute(&mut *tx.tx)
            .await
            .map_err(map_constraint)?
            .rows_affected();
            if updated != 1 {
                tx.rollback().await?;
                return Err(conflict(TurnConflict::StaleGeneration {
                    actual: turn.generation,
                }));
            }
            Some(message)
        } else {
            let existing = load_message_by_id_tx(&mut tx.tx, &canonical_message_id.0).await?;
            verify_existing_materialized_message_without_sequence(
                &existing,
                &turn,
                &input.prepared,
            )?;
            Some(existing)
        };
        let canonical_turn =
            load_turn_for_workflow_tx(&mut tx.tx, turn_id, input.authority.workflow_id)
                .await?
                .ok_or_else(|| {
                    DbError::Serialization("direct-turn missing after materialization".to_string())
                })?;
        finish_workflow_transaction_at_cut(tx, cut).await?;
        Ok(MaterializeAuthoritativeTurnResult {
            outcome: step.outcome,
            authority_outcome: acceptance.outcome,
            receipt: acceptance.receipt,
            delivery: acceptance.delivery,
            canonical_turn,
            message,
        })
    }

    pub async fn terminate_authoritative_turn(&self, command: TurnCommand) -> DbResult<TurnStep> {
        self.terminate_authoritative_turn_at_cut(command, TransactionCut::None)
            .await
    }

    async fn terminate_authoritative_turn_at_cut(
        &self,
        command: TurnCommand,
        cut: TransactionCut,
    ) -> DbResult<TurnStep> {
        let (turn_id, expected_generation, terminal) = match &command {
            TurnCommand::Complete {
                turn_id,
                expected_generation,
            } => (*turn_id, *expected_generation, TurnTerminal::Completed),
            TurnCommand::Cancel {
                turn_id,
                expected_generation,
            } => (*turn_id, *expected_generation, TurnTerminal::Cancelled),
            TurnCommand::Fail {
                turn_id,
                expected_generation,
                reason,
            } => (
                *turn_id,
                *expected_generation,
                TurnTerminal::Failed {
                    reason: reason.clone(),
                },
            ),
            TurnCommand::Accept { .. } | TurnCommand::Materialize { .. } => {
                return Err(DbError::Serialization(
                    "terminal repository command required".to_string(),
                ));
            }
        };
        let mut tx = self.begin_tx().await?;
        let row = sqlx::query("SELECT * FROM durable_turns WHERE turn_id = ?1")
            .bind(to_i64(turn_id.0, "turn_id")?)
            .fetch_optional(&mut *tx.tx)
            .await?
            .ok_or_else(|| conflict(TurnConflict::UnknownTurn))?;
        let turn = row_to_turn(row)?;
        let workflow_id = workflow_id_for_turn_tx(&mut tx.tx, turn_id).await?;
        let head = tx
            .fetch_workflow_head(workflow_id)
            .await?
            .ok_or_else(|| DbError::Serialization("direct-turn workflow missing".to_string()))?;
        let mut model =
            phoenix_workflow::DurableTurnModel::from_turns([turn.clone()]).map_err(conflict)?;
        let step = model.apply(command).map_err(conflict)?;
        if matches!(step.outcome, TurnOutcome::TerminalReplay { .. }) {
            tx.rollback().await?;
            return Ok(step);
        }
        let (terminal_kind, reason) = terminal_sql(&terminal);
        let next_generation = expected_generation.saturating_add(1);
        let snapshot = direct_turn_profile::DirectTurnSnapshot { turn_id: turn_id.0 };
        let terminal_event = direct_turn_profile::DirectTurnEvent::Terminal(
            direct_turn_profile::DirectTurnTerminalEvent {
                terminal: terminal_event_kind(&terminal),
            },
        );
        let event_codec = local_codec(&direct_turn_profile::event_codec());
        let snapshot_codec = local_codec(&direct_turn_profile::snapshot_codec());
        let event_payload = serde_json::to_vec(&terminal_event).map_err(|e| {
            DbError::Serialization(format!("encode direct-turn terminal event: {e}"))
        })?;
        let snapshot_payload = serde_json::to_vec(&snapshot)
            .map_err(|e| DbError::Serialization(format!("encode direct-turn snapshot: {e}")))?;
        let committed = tx
            .commit_transition_head_cas(
                workflow_id,
                head.version,
                Generation(next_generation),
                workflow_status_for_terminal(&terminal),
                &event_codec,
                &event_payload,
                &snapshot_codec,
                &snapshot_payload,
                phoenix_workflow::TransitionId(DIRECT_TURN_TERMINAL_TRANSITION_ID),
                phoenix_workflow::Timestamp(next_generation),
            )
            .await?;
        if !committed {
            tx.rollback().await?;
            return Err(conflict(TurnConflict::StaleGeneration {
                actual: turn.generation,
            }));
        }
        let updated = sqlx::query(
            "UPDATE durable_turns
             SET generation = generation + 1, terminal_kind = ?3,
                 terminal_reason = ?4, owns_conversation = 0
             WHERE turn_id = ?1 AND generation = ?2 AND terminal_kind IS NULL",
        )
        .bind(to_i64(turn_id.0, "turn_id")?)
        .bind(to_i64(expected_generation, "generation")?)
        .bind(terminal_kind)
        .bind(reason)
        .execute(&mut *tx.tx)
        .await?;
        if updated.rows_affected() != 1 {
            tx.rollback().await?;
            return Err(conflict(TurnConflict::StaleGeneration {
                actual: turn.generation,
            }));
        }
        mark_active_attempts_authority_lost_tx(&mut tx, workflow_id).await?;
        delete_reclaimable_leases_tx(&mut tx, workflow_id).await?;
        tx.invalidate_nonterminal_effects(workflow_id).await?;
        finish_workflow_transaction_at_cut(tx, cut).await?;
        Ok(step)
    }
}

async fn workflow_id_for_turn_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    turn_id: TurnAuthorityId,
) -> DbResult<WorkflowId> {
    let workflow_id =
        sqlx::query_scalar::<_, i64>("SELECT workflow_id FROM durable_turns WHERE turn_id = ?1")
            .bind(to_i64(turn_id.0, "turn_id")?)
            .fetch_one(&mut **tx)
            .await?;
    Ok(WorkflowId(to_u64(workflow_id, "workflow_id")?))
}

async fn next_global_workflow_id_tx(tx: &mut super::WorkflowTx<'_>) -> DbResult<WorkflowId> {
    let next =
        sqlx::query_scalar::<_, i64>("SELECT COALESCE(MAX(workflow_id), 0) + 1 FROM workflows")
            .fetch_one(&mut *tx.tx)
            .await?;
    Ok(WorkflowId(to_u64(next, "workflow_id")?))
}

async fn insert_direct_turn_workflow_tx(
    tx: &mut super::WorkflowTx<'_>,
    workflow_id: WorkflowId,
    turn_id: TurnAuthorityId,
    input: &AcceptAuthoritativeTurn,
) -> DbResult<()> {
    let snapshot = direct_turn_profile::DirectTurnSnapshot { turn_id: turn_id.0 };
    let intent = direct_turn_profile::RuntimeTurnIntent {
        turn_id: turn_id.0,
        conversation_id: input.conversation.0.clone(),
        client_turn_key: input.client_key.as_str().to_string(),
        prepared_fingerprint: input.prepared.fingerprint().to_string(),
    };
    let create = CreateWorkflowWithExternalAcceptance {
        workflow_id,
        profile: direct_turn_profile::profile(),
        acceptance: direct_turn_profile::acceptance_profile().erase(),
        target_scope: phoenix_workflow::ScopeId::new(format!("direct-turn:{}", turn_id.0))
            .ok_or_else(|| DbError::Serialization("empty direct-turn scope".to_string()))?,
        idempotency_key: phoenix_workflow::NonEmptyExternalKey::new(format!(
            "turn:{}:{}",
            input.conversation.0,
            input.client_key.as_str()
        ))
        .ok_or_else(|| DbError::Serialization("empty direct-turn key".to_string()))?,
        intent_fingerprint: input.prepared.fingerprint().to_string(),
        snapshot_codec: direct_turn_profile::snapshot_codec(),
        snapshot_payload: serde_json::to_vec(&snapshot)
            .map_err(|e| DbError::Serialization(format!("encode direct-turn snapshot: {e}")))?,
        receipt_handle: serde_json::to_vec(&turn_id.0).map_err(|e| {
            DbError::Serialization(format!("encode direct-turn receipt handle: {e}"))
        })?,
        disposition_handle: serde_json::to_vec(&authority_event(
            &super::LocalAttemptAuthority {
                workflow_id,
                declared_workflow_version: Version(1),
                generation: Generation(0),
                effect_id: EffectId(DIRECT_TURN_EFFECT_ID),
                attempt_id: AttemptId(0),
                process_incarnation: ProcessIncarnation(0),
            },
            turn_id,
        ))
        .map_err(|e| {
            DbError::Serialization(format!("encode direct-turn disposition handle: {e}"))
        })?,
        now: input.accepted_at,
    };
    tx.insert_workflow(&create).await?;
    let plan = CommitTransitionPlanCas {
        workflow_id,
        expected_version: Version(0),
        transition_id: phoenix_workflow::TransitionId(DIRECT_TURN_ACCEPTED_TRANSITION_ID),
        generation: Generation(0),
        next_status: WorkflowStatus::Active,
        event_codec: local_codec(&direct_turn_profile::event_codec()),
        event_payload: serde_json::to_vec(&direct_turn_profile::DirectTurnEvent::Accepted)
            .map_err(|e| DbError::Serialization(format!("encode direct-turn event: {e}")))?,
        next_snapshot_codec: local_codec(&direct_turn_profile::snapshot_codec()),
        next_snapshot_payload: serde_json::to_vec(&snapshot)
            .map_err(|e| DbError::Serialization(format!("encode direct-turn snapshot: {e}")))?,
        committed_at: input.accepted_at,
        effects: vec![LocalEffectDecl {
            effect_id: phoenix_workflow::EffectId(DIRECT_TURN_EFFECT_ID),
            declared_workflow_version: Version(1),
            family: "direct_turn.delivery".to_string(),
            kind: match input.disposition {
                AcceptedDisposition::Runtime => "deliver_runtime_turn",
                AcceptedDisposition::Steering => "enqueue_steering_turn",
            }
            .to_string(),
            intent_codec: local_codec(&direct_turn_profile::intent_codec()),
            intent_payload: serde_json::to_vec(&intent)
                .map_err(|e| DbError::Serialization(format!("encode direct-turn intent: {e}")))?,
            generation: Generation(0),
            role: EffectRole::Required,
            capability: ExecutionCapability::ReclaimableObservation,
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
        phoenix_workflow::CommitOutcome::Committed => Ok(()),
        other @ (phoenix_workflow::CommitOutcome::VersionConflict
        | phoenix_workflow::CommitOutcome::InvalidPlan
        | phoenix_workflow::CommitOutcome::UnsupportedCodec) => Err(DbError::Serialization(
            format!("direct-turn transition was rejected: {other:?}"),
        )),
    }
}

async fn load_turn_for_workflow_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    turn_id: TurnAuthorityId,
    workflow_id: WorkflowId,
) -> DbResult<Option<DurableTurn>> {
    sqlx::query("SELECT * FROM durable_turns WHERE turn_id = ?1 AND workflow_id = ?2")
        .bind(to_i64(turn_id.0, "turn_id")?)
        .bind(to_i64(workflow_id.0, "workflow_id")?)
        .fetch_optional(&mut **tx)
        .await?
        .map(row_to_turn)
        .transpose()
}

fn local_codec_owned(codec: &phoenix_workflow::CodecRef) -> LocalCodec {
    LocalCodec {
        family: codec.family.to_string(),
        version: codec.version,
    }
}

fn timestamp_to_datetime(timestamp: Timestamp) -> DateTime<Utc> {
    DateTime::from_timestamp(i64::try_from(timestamp.0).unwrap_or(0), 0)
        .unwrap_or(DateTime::<Utc>::UNIX_EPOCH)
}

fn verify_prepared_payload(
    turn: &DurableTurn,
    prepared: &PreparedDirectTurnPayload,
) -> DbResult<()> {
    let stored: PreparedDirectTurnPayload = serde_json::from_slice(&turn.prepared.payload)
        .map_err(|e| DbError::Serialization(format!("decode direct-turn prepared payload: {e}")))?;
    if prepared.message_id != stored.message_id {
        return Err(conflict(TurnConflict::MaterializationIdentityChanged {
            canonical: CanonicalMessageId(stored.message_id),
        }));
    }
    if prepared != &stored {
        return Err(conflict(TurnConflict::PreparedSemanticsChanged));
    }
    Ok(())
}

async fn insert_canonical_message_tx(
    tx: &mut super::WorkflowTx<'_>,
    turn: &DurableTurn,
    canonical_message_id: &CanonicalMessageId,
    sequence_id: i64,
    created_at: Timestamp,
    prepared: &PreparedDirectTurnPayload,
) -> DbResult<Message> {
    if prepared.message_id != canonical_message_id.0 {
        return Err(conflict(TurnConflict::MaterializationIdentityChanged {
            canonical: CanonicalMessageId(prepared.message_id.clone()),
        }));
    }
    let (content, display_data) = prepared.message_content_and_display_data();
    let created_at_dt = timestamp_to_datetime(created_at);
    let content_str = serde_json::to_string(&content.to_stored_json())
        .map_err(|e| DbError::Serialization(e.to_string()))?;
    let display_str = display_data
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|e| DbError::Serialization(e.to_string()))?;
    sqlx::query(
        "INSERT INTO messages (
            message_id, conversation_id, sequence_id, message_type, content, display_data, usage_data, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7)",
    )
    .bind(&canonical_message_id.0)
    .bind(&turn.conversation.0)
    .bind(sequence_id)
    .bind(content.message_type().to_string())
    .bind(&content_str)
    .bind(&display_str)
    .bind(created_at_dt.to_rfc3339())
    .execute(&mut *tx.tx)
    .await
    .map_err(map_constraint)?;
    insert_message_attachments(&mut tx.tx, &canonical_message_id.0, &content).await?;
    sqlx::query("UPDATE conversations SET updated_at = ?1 WHERE id = ?2")
        .bind(created_at_dt.to_rfc3339())
        .bind(&turn.conversation.0)
        .execute(&mut *tx.tx)
        .await?;
    if has_message_fts_tx(tx).await? {
        let message = Message {
            message_id: canonical_message_id.0.clone(),
            conversation_id: turn.conversation.0.clone(),
            sequence_id,
            message_type: content.message_type(),
            content: content.clone(),
            display_data: display_data.clone(),
            usage_data: None,
            created_at: created_at_dt,
        };
        crate::retrieval::fts_upsert_conn(&mut tx.tx, &message).await?;
        Ok(message)
    } else {
        Ok(Message {
            message_id: canonical_message_id.0.clone(),
            conversation_id: turn.conversation.0.clone(),
            sequence_id,
            message_type: content.message_type(),
            content,
            display_data,
            usage_data: None,
            created_at: created_at_dt,
        })
    }
}

async fn load_message_by_id_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    message_id: &str,
) -> DbResult<Message> {
    let row = sqlx::query(
        "SELECT message_id, conversation_id, sequence_id, message_type, content, display_data, usage_data, created_at
         FROM messages WHERE message_id = ?1",
    )
    .bind(message_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(|e| {
        if matches!(e, sqlx::Error::RowNotFound) {
            DbError::MessageNotFound(message_id.to_string())
        } else {
            DbError::Sqlx(e)
        }
    })?;
    let mut message = crate::parse_message_row(row).map_err(DbError::Sqlx)?;
    let files = sqlx::query(
        "SELECT original_name, media_type, size_bytes, stored_path
         FROM message_files WHERE message_id = ?1 ORDER BY ordinal",
    )
    .bind(message_id)
    .map(
        |row: sqlx::sqlite::SqliteRow| phoenix_core::domain::db_schema::FileAttachment {
            original_name: row.get("original_name"),
            media_type: row.get("media_type"),
            size_bytes: u64::try_from(row.get::<i64, _>("size_bytes")).unwrap_or(0),
            stored_path: row.get("stored_path"),
        },
    )
    .fetch_all(&mut **tx)
    .await?;
    let images =
        if matches!(
            message.message_type,
            phoenix_core::domain::db_schema::MessageType::User
        ) {
            sqlx::query(
            "SELECT media_type, data FROM message_images WHERE message_id = ?1 ORDER BY ordinal",
        )
        .bind(message_id)
        .map(|row: sqlx::sqlite::SqliteRow| phoenix_core::domain::db_schema::ImageData {
            data: row.get("data"),
            media_type: row.get("media_type"),
        })
        .fetch_all(&mut **tx)
        .await?
        } else {
            Vec::new()
        };
    message.content.set_attachments(images, files);
    Ok(message)
}

fn verify_existing_materialized_message_without_sequence(
    message: &Message,
    turn: &DurableTurn,
    prepared: &PreparedDirectTurnPayload,
) -> DbResult<()> {
    if message.conversation_id != turn.conversation.0 {
        return Err(conflict(TurnConflict::PreparedSemanticsChanged));
    }
    let (expected_content, expected_display) = prepared.message_content_and_display_data();
    if message.content != expected_content || message.display_data != expected_display {
        return Err(conflict(TurnConflict::PreparedSemanticsChanged));
    }
    Ok(())
}

async fn has_message_fts_tx(tx: &mut super::WorkflowTx<'_>) -> DbResult<bool> {
    let exists = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'message_fts'",
    )
    .fetch_one(&mut *tx.tx)
    .await?;
    Ok(exists > 0)
}

async fn insert_message_attachments(
    conn: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    message_id: &str,
    content: &MessageContent,
) -> DbResult<()> {
    let (images, files) = content.attachments();
    for (ordinal, file) in files.iter().enumerate() {
        sqlx::query(
            "INSERT OR IGNORE INTO message_files
             (message_id, ordinal, original_name, media_type, size_bytes, stored_path)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )
        .bind(message_id)
        .bind(i64::try_from(ordinal).unwrap_or(i64::MAX))
        .bind(&file.original_name)
        .bind(&file.media_type)
        .bind(i64::try_from(file.size_bytes).unwrap_or(i64::MAX))
        .bind(&file.stored_path)
        .execute(&mut **conn)
        .await?;
    }
    for (ordinal, image) in images.iter().enumerate() {
        sqlx::query(
            "INSERT OR IGNORE INTO message_images (message_id, ordinal, media_type, data)
             VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(message_id)
        .bind(i64::try_from(ordinal).unwrap_or(i64::MAX))
        .bind(&image.media_type)
        .bind(&image.data)
        .execute(&mut **conn)
        .await?;
    }
    Ok(())
}

async fn load_live_attempt_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    workflow_id: WorkflowId,
    effect_id: u64,
) -> DbResult<Option<super::LocalAttemptRecord>> {
    let rows = sqlx::query(
        "SELECT a.attempt_id, a.ordinal, a.declared_workflow_version, a.generation,
                a.effect_id, a.process_incarnation, a.status, l.lease_until
         FROM workflow_attempts a
         LEFT JOIN workflow_reclaimable_leases l
           ON l.workflow_id = a.workflow_id AND l.attempt_id = a.attempt_id
         WHERE a.workflow_id = ?1 AND a.effect_id = ?2
           AND a.status IN ('Begun', 'ObservationRecorded')
         ORDER BY a.ordinal
         LIMIT 1",
    )
    .bind(to_i64(workflow_id.0, "workflow_id")?)
    .bind(to_i64(effect_id, "effect_id")?)
    .fetch_optional(&mut **tx)
    .await?;
    rows.map(|row| {
        let authority = super::LocalAttemptAuthority {
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
        Ok(super::LocalAttemptRecord {
            id: authority.attempt_id,
            ordinal: u32::try_from(row.get::<i64, _>("ordinal"))
                .map_err(|_| DbError::Serialization("ordinal exceeds u32".to_string()))?,
            authority: authority.clone(),
            status: parse_attempt_status_local(&row.get::<String, _>("status"))?,
            lease: row
                .get::<Option<i64>, _>("lease_until")
                .map(|value| {
                    to_u64(value, "lease_until").map(|lease_until| super::LocalReclaimableLease {
                        attempt_id: authority.attempt_id,
                        lease_until: LeaseExpiry(lease_until),
                    })
                })
                .transpose()?,
        })
    })
    .transpose()
}

async fn next_attempt_id_tx(tx: &mut super::WorkflowTx<'_>) -> DbResult<AttemptId> {
    sqlx::query(
        "INSERT INTO workflow_global_sequences (sequence_name, next_value)
         VALUES ('attempt', 2)
         ON CONFLICT(sequence_name)
         DO UPDATE SET next_value = workflow_global_sequences.next_value + 1",
    )
    .execute(&mut *tx.tx)
    .await?;
    let next = sqlx::query_scalar::<_, i64>(
        "SELECT next_value - 1 FROM workflow_global_sequences WHERE sequence_name = 'attempt'",
    )
    .fetch_one(&mut *tx.tx)
    .await?;
    Ok(AttemptId(to_u64(next, "attempt_id")?))
}

async fn next_workflow_sequence_tx(
    tx: &mut super::WorkflowTx<'_>,
    workflow_id: WorkflowId,
    sequence_name: &str,
) -> DbResult<u64> {
    sqlx::query(
        "INSERT INTO workflow_sequences (workflow_id, sequence_name, next_value)
         VALUES (?1, ?2, 2)
         ON CONFLICT(workflow_id, sequence_name)
         DO UPDATE SET next_value = workflow_sequences.next_value + 1",
    )
    .bind(to_i64(workflow_id.0, "workflow_id")?)
    .bind(sequence_name)
    .execute(&mut *tx.tx)
    .await?;
    let next = sqlx::query_scalar::<_, i64>(
        "SELECT next_value - 1 FROM workflow_sequences
         WHERE workflow_id = ?1 AND sequence_name = ?2",
    )
    .bind(to_i64(workflow_id.0, "workflow_id")?)
    .bind(sequence_name)
    .fetch_one(&mut *tx.tx)
    .await?;
    to_u64(next, sequence_name)
}

async fn expire_direct_turn_lease_in_tx(
    tx: &mut super::WorkflowTx<'_>,
    input: &super::ExpireLeaseInput,
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
    if !matches!(
        lease.get::<String, _>("attempt_status").as_str(),
        "Begun" | "ObservationRecorded"
    ) {
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
    sqlx::query("UPDATE workflow_effects SET status = 'Eligible' WHERE workflow_id = ?1 AND effect_id = ?2 AND status = 'Executing'")
        .bind(to_i64(input.workflow_id.0, "workflow_id")?)
        .bind(to_i64(input.effect_id.0, "effect_id")?)
        .execute(&mut *tx.tx)
        .await?;
    Ok(AuthorityOutcome::Authorized)
}

fn parse_attempt_status_local(raw: &str) -> DbResult<AttemptStatus> {
    match raw {
        "Begun" => Ok(AttemptStatus::Begun),
        "ObservationRecorded" => Ok(AttemptStatus::ObservationRecorded),
        "ReceiptAccepted" => Ok(AttemptStatus::ReceiptAccepted),
        "AuthorityLost" => Ok(AttemptStatus::AuthorityLost),
        other => Err(DbError::Serialization(format!(
            "unknown attempt status {other}"
        ))),
    }
}

fn local_codec(codec: &phoenix_workflow::CodecRef) -> LocalCodec {
    LocalCodec {
        family: codec.family.to_string(),
        version: codec.version,
    }
}

async fn load_by_scoped_key(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    conversation: &ConversationAuthority,
    client_key: &ClientTurnKey,
) -> DbResult<Option<DurableTurn>> {
    sqlx::query("SELECT * FROM durable_turns WHERE conversation_id = ?1 AND client_turn_key = ?2")
        .bind(&conversation.0)
        .bind(client_key.as_str())
        .fetch_optional(&mut **tx)
        .await?
        .map(row_to_turn)
        .transpose()
}

#[allow(clippy::needless_pass_by_value)]
fn row_to_turn(row: sqlx::sqlite::SqliteRow) -> DbResult<DurableTurn> {
    let disposition = match row.get::<String, _>("disposition").as_str() {
        "Runtime" => AcceptedDisposition::Runtime,
        "Steering" => AcceptedDisposition::Steering,
        other => {
            return Err(DbError::Serialization(format!(
                "unknown disposition {other}"
            )))
        }
    };
    let terminal_kind = row.get::<Option<String>, _>("terminal_kind");
    let terminal_reason = row.get::<Option<String>, _>("terminal_reason");
    let owns_conversation = row.get::<bool, _>("owns_conversation");
    let lifecycle = match terminal_kind.as_deref() {
        None => {
            if terminal_reason.is_some() {
                return Err(DbError::Serialization(
                    "non-terminal turn has terminal reason".to_string(),
                ));
            }
            TurnLifecycle::Accepted { disposition }
        }
        Some("Completed") => {
            if terminal_reason.is_some() {
                return Err(DbError::Serialization(
                    "completed turn must not have terminal reason".to_string(),
                ));
            }
            TurnLifecycle::Terminal {
                terminal: TurnTerminal::Completed,
                disposition,
            }
        }
        Some("Cancelled") => {
            if terminal_reason.is_some() {
                return Err(DbError::Serialization(
                    "cancelled turn must not have terminal reason".to_string(),
                ));
            }
            TurnLifecycle::Terminal {
                terminal: TurnTerminal::Cancelled,
                disposition,
            }
        }
        Some("Failed") => TurnLifecycle::Terminal {
            terminal: TurnTerminal::Failed {
                reason: terminal_reason.ok_or_else(|| {
                    DbError::Serialization("failed turn missing reason".to_string())
                })?,
            },
            disposition,
        },
        Some(other) => {
            return Err(DbError::Serialization(format!(
                "unknown terminal kind {other}"
            )))
        }
    };
    let expected_owns_conversation = matches!(
        lifecycle,
        TurnLifecycle::Accepted {
            disposition: AcceptedDisposition::Runtime
        }
    );
    if owns_conversation != expected_owns_conversation {
        return Err(DbError::Serialization(
            "owns_conversation disagrees with turn lifecycle".to_string(),
        ));
    }
    Ok(DurableTurn {
        id: TurnAuthorityId(to_u64(row.get("turn_id"), "turn_id")?),
        conversation: ConversationAuthority(row.get("conversation_id")),
        client_key: ClientTurnKey::try_from(row.get::<String, _>("client_turn_key"))
            .map_err(|e| DbError::Serialization(e.to_string()))?,
        prepared: PreparedTurn::rehydrate(
            row.get("prepared_fingerprint"),
            row.get("prepared_payload"),
        )
        .map_err(conflict)?,
        generation: to_u64(row.get("generation"), "generation")?,
        lifecycle,
        materialization: row.get::<Option<String>, _>("canonical_message_id").map_or(
            Materialization::Unmaterialized,
            |message_id| Materialization::Materialized {
                message_id: CanonicalMessageId(message_id),
            },
        ),
    })
}

fn disposition_sql(disposition: AcceptedDisposition) -> &'static str {
    match disposition {
        AcceptedDisposition::Runtime => "Runtime",
        AcceptedDisposition::Steering => "Steering",
    }
}

fn terminal_sql(terminal: &TurnTerminal) -> (&'static str, Option<&str>) {
    match terminal {
        TurnTerminal::Completed => ("Completed", None),
        TurnTerminal::Cancelled => ("Cancelled", None),
        TurnTerminal::Failed { reason } => ("Failed", Some(reason.as_str())),
    }
}

fn workflow_status_for_terminal(terminal: &TurnTerminal) -> WorkflowStatus {
    match terminal {
        TurnTerminal::Completed => WorkflowStatus::Completed,
        TurnTerminal::Cancelled => WorkflowStatus::Cancelled,
        TurnTerminal::Failed { .. } => WorkflowStatus::Failed,
    }
}

fn terminal_event_kind(terminal: &TurnTerminal) -> direct_turn_profile::DirectTurnTerminalKind {
    match terminal {
        TurnTerminal::Completed => direct_turn_profile::DirectTurnTerminalKind::Completed,
        TurnTerminal::Cancelled => direct_turn_profile::DirectTurnTerminalKind::Cancelled,
        TurnTerminal::Failed { reason } => direct_turn_profile::DirectTurnTerminalKind::Failed {
            reason: reason.clone(),
        },
    }
}

async fn mark_active_attempts_authority_lost_tx(
    tx: &mut super::WorkflowTx<'_>,
    workflow_id: WorkflowId,
) -> DbResult<()> {
    sqlx::query(
        "UPDATE workflow_attempts
         SET status = 'AuthorityLost'
         WHERE workflow_id = ?1 AND status IN ('Begun', 'ObservationRecorded')",
    )
    .bind(to_i64(workflow_id.0, "workflow_id")?)
    .execute(&mut *tx.tx)
    .await?;
    Ok(())
}

async fn delete_reclaimable_leases_tx(
    tx: &mut super::WorkflowTx<'_>,
    workflow_id: WorkflowId,
) -> DbResult<()> {
    sqlx::query("DELETE FROM workflow_reclaimable_leases WHERE workflow_id = ?1")
        .bind(to_i64(workflow_id.0, "workflow_id")?)
        .execute(&mut *tx.tx)
        .await?;
    Ok(())
}

fn to_i64(value: u64, field: &str) -> DbResult<i64> {
    i64::try_from(value).map_err(|_| DbError::Serialization(format!("{field} exceeds i64")))
}

fn to_u64(value: i64, field: &str) -> DbResult<u64> {
    u64::try_from(value).map_err(|_| DbError::Serialization(format!("negative {field}")))
}

async fn finish_workflow_transaction_at_cut(
    tx: super::WorkflowTx<'_>,
    cut: TransactionCut,
) -> DbResult<()> {
    if cut == TransactionCut::BeforeCommit {
        tx.rollback().await?;
        return Err(injected_cut(cut));
    }
    tx.commit().await?;
    if cut == TransactionCut::AfterCommit {
        return Err(injected_cut(cut));
    }
    Ok(())
}

fn injected_cut(cut: TransactionCut) -> DbError {
    DbError::Serialization(format!("injected transaction cut: {cut:?}"))
}

#[allow(clippy::needless_pass_by_value)]
fn conflict(conflict: TurnConflict) -> DbError {
    DbError::Serialization(format!("direct-turn conflict: {conflict:?}"))
}

#[allow(clippy::wildcard_enum_match_arm)]
fn map_constraint(error: sqlx::Error) -> DbError {
    match error {
        sqlx::Error::Database(database) if database.is_unique_violation() => {
            DbError::Serialization("direct-turn uniqueness conflict".to_string())
        }
        other => DbError::Sqlx(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Database;
    use crate::LocalAttemptAuthority;

    async fn repo() -> WorkflowRepository {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("conv-a", "A", "/tmp", true, None, None)
            .await
            .unwrap();
        db.create_conversation("conv-b", "B", "/tmp", true, None, None)
            .await
            .unwrap();
        db.create_conversation("conv-c", "C", "/tmp", true, None, None)
            .await
            .unwrap();
        db.create_conversation("conv-d", "D", "/tmp", true, None, None)
            .await
            .unwrap();
        WorkflowRepository::new(db.pool().clone())
    }

    fn input(conversation: &str, key: &str, seed: u8) -> AcceptAuthoritativeTurn {
        input_with_disposition(conversation, key, seed, AcceptedDisposition::Runtime)
    }

    fn input_with_disposition(
        conversation: &str,
        key: &str,
        seed: u8,
        disposition: AcceptedDisposition,
    ) -> AcceptAuthoritativeTurn {
        AcceptAuthoritativeTurn {
            conversation: ConversationAuthority(conversation.to_string()),
            client_key: ClientTurnKey::new(key).unwrap(),
            prepared: prepared_turn(&format!("message-{conversation}-{key}")),
            disposition,
            accepted_at: phoenix_workflow::Timestamp(u64::from(seed)),
        }
    }

    async fn created_turn(
        repo: &WorkflowRepository,
        key: &str,
        seed: u8,
    ) -> (TurnAuthorityId, WorkflowId) {
        let created = repo
            .accept_authoritative_turn(&input("conv-a", key, seed))
            .await
            .unwrap();
        let TurnOutcome::Created { turn_id } = created.outcome else {
            panic!("expected created turn")
        };
        let workflow_id = repo.workflow_id_for_turn(turn_id).await.unwrap().unwrap();
        (turn_id, workflow_id)
    }

    async fn workflow_status_version_generation_transition_count(
        repo: &WorkflowRepository,
        workflow_id: WorkflowId,
    ) -> (String, i64, i64, i64) {
        sqlx::query_as(
            "SELECT w.status, w.version, w.generation, COUNT(t.transition_id)
             FROM workflows w
             LEFT JOIN workflow_transitions t ON t.workflow_id = w.workflow_id
             WHERE w.workflow_id = ?1
             GROUP BY w.workflow_id",
        )
        .bind(i64::try_from(workflow_id.0).unwrap())
        .fetch_one(&repo.pool)
        .await
        .unwrap()
    }

    fn claim_input(
        workflow_id: WorkflowId,
        turn_id: TurnAuthorityId,
        now: u64,
    ) -> ClaimAuthoritativeTurnInput {
        ClaimAuthoritativeTurnInput {
            turn_id,
            workflow_id,
            process_incarnation: ProcessIncarnation(now),
            now: Timestamp(now),
            lease_until: LeaseExpiry(now + 10),
        }
    }

    fn prepared_payload(message_id: &str) -> PreparedDirectTurnPayload {
        PreparedDirectTurnPayload {
            text: format!("text-{message_id}"),
            llm_text: None,
            images: Vec::new(),
            files: Vec::new(),
            message_id: message_id.to_string(),
            user_agent: Some("agent/test".to_string()),
            skill_invocation: None,
        }
    }

    fn prepared_turn(message_id: &str) -> PreparedTurn {
        PreparedTurn::from_exact_payload(
            serde_json::to_vec(&prepared_payload(message_id)).unwrap(),
        )
    }

    fn materialize_input(
        turn_id: TurnAuthorityId,
        authority: LocalAttemptAuthority,
        _receipt_id: u64,
        _delivery_id: u64,
        message_id: &str,
        now: u64,
    ) -> MaterializeAuthoritativeTurnInput {
        MaterializeAuthoritativeTurnInput {
            turn_id,
            authority,
            prepared: prepared_payload(message_id),
            sequence_id: i64::try_from(now).unwrap(),
            created_at: Timestamp(now),
            now: Timestamp(now),
        }
    }

    fn preflight_input(
        turn_id: TurnAuthorityId,
        authority: LocalAttemptAuthority,
        message_id: &str,
        now: u64,
    ) -> PreflightDirectTurnMaterializationInput {
        PreflightDirectTurnMaterializationInput {
            turn_id,
            authority,
            prepared: prepared_payload(message_id),
            now: Timestamp(now),
        }
    }

    #[tokio::test]
    async fn accept_refines_before_and_after_commit_crash_cuts() {
        let before_repo = repo().await;
        let before_input = input("conv-a", "before", 1);
        assert!(before_repo
            .accept_authoritative_turn_at_cut(&before_input, TransactionCut::BeforeCommit)
            .await
            .is_err());
        assert!(matches!(
            before_repo
                .accept_authoritative_turn(&before_input)
                .await
                .unwrap()
                .outcome,
            TurnOutcome::Created { .. }
        ));

        let after_repo = repo().await;
        let after_input = input("conv-a", "after", 2);
        assert!(after_repo
            .accept_authoritative_turn_at_cut(&after_input, TransactionCut::AfterCommit)
            .await
            .is_err());
        assert!(matches!(
            after_repo
                .accept_authoritative_turn(&after_input)
                .await
                .unwrap()
                .outcome,
            TurnOutcome::ExactReplay { .. }
        ));
    }

    #[tokio::test]
    async fn materialization_refines_before_and_after_commit_crash_cuts() {
        let before_repo = repo().await;
        let created = before_repo
            .accept_authoritative_turn(&input("conv-a", "materialize-before", 3))
            .await
            .unwrap();
        let TurnOutcome::Created { turn_id } = created.outcome else {
            panic!("expected created turn")
        };
        let workflow_id = before_repo
            .workflow_id_for_turn(turn_id)
            .await
            .unwrap()
            .unwrap();
        let claim = before_repo
            .claim_authoritative_turn(&claim_input(workflow_id, turn_id, 10))
            .await
            .unwrap();
        let authority = claim.authority.unwrap();
        let message = CanonicalMessageId("message-conv-a-materialize-before".to_string());
        assert!(before_repo
            .materialize_authoritative_turn_at_cut(
                &materialize_input(turn_id, authority.clone(), 1, 1, &message.0, 10),
                TransactionCut::BeforeCommit,
            )
            .await
            .is_err());
        assert!(matches!(
            before_repo
                .materialize_authoritative_turn(&materialize_input(
                    turn_id, authority, 1, 1, &message.0, 10
                ))
                .await
                .unwrap()
                .outcome,
            TurnOutcome::Materialized { .. }
        ));

        let after_repo = repo().await;
        let created = after_repo
            .accept_authoritative_turn(&input("conv-a", "materialize-after", 4))
            .await
            .unwrap();
        let TurnOutcome::Created { turn_id } = created.outcome else {
            panic!("expected created turn")
        };
        let workflow_id = after_repo
            .workflow_id_for_turn(turn_id)
            .await
            .unwrap()
            .unwrap();
        let claim = after_repo
            .claim_authoritative_turn(&claim_input(workflow_id, turn_id, 11))
            .await
            .unwrap();
        let authority = claim.authority.unwrap();
        let message = CanonicalMessageId("message-conv-a-materialize-after".to_string());
        assert!(after_repo
            .materialize_authoritative_turn_at_cut(
                &materialize_input(turn_id, authority.clone(), 1, 1, &message.0, 11),
                TransactionCut::AfterCommit,
            )
            .await
            .is_err());
        assert!(matches!(
            after_repo
                .materialize_authoritative_turn(&materialize_input(
                    turn_id, authority, 2, 2, &message.0, 11
                ))
                .await
                .unwrap()
                .outcome,
            TurnOutcome::MaterializationReplay { .. }
        ));
    }

    #[tokio::test]
    async fn terminal_refines_before_and_after_commit_crash_cuts() {
        let before_repo = repo().await;
        let created = before_repo
            .accept_authoritative_turn(&input("conv-a", "terminal-before", 5))
            .await
            .unwrap();
        let TurnOutcome::Created { turn_id } = created.outcome else {
            panic!("expected created turn")
        };
        let command = TurnCommand::Cancel {
            turn_id,
            expected_generation: 0,
        };
        assert!(before_repo
            .terminate_authoritative_turn_at_cut(command.clone(), TransactionCut::BeforeCommit)
            .await
            .is_err());
        assert!(before_repo
            .terminate_authoritative_turn(command)
            .await
            .is_ok());

        let after_repo = repo().await;
        let created = after_repo
            .accept_authoritative_turn(&input("conv-a", "terminal-after", 6))
            .await
            .unwrap();
        let TurnOutcome::Created { turn_id } = created.outcome else {
            panic!("expected created turn")
        };
        assert!(after_repo
            .terminate_authoritative_turn_at_cut(
                TurnCommand::Cancel {
                    turn_id,
                    expected_generation: 0,
                },
                TransactionCut::AfterCommit,
            )
            .await
            .is_err());
        let persisted = after_repo
            .load_authoritative_turn(turn_id)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            persisted.lifecycle,
            TurnLifecycle::Terminal {
                terminal: TurnTerminal::Cancelled,
                ..
            }
        ));
        assert_eq!(persisted.generation, 1);
    }

    #[tokio::test]
    async fn terminal_cas_advances_status_version_generation_and_transition_rows() {
        for (key, command, expected_status, expected_terminal) in [
            (
                "complete-status",
                TurnCommand::Complete {
                    turn_id: TurnAuthorityId(0),
                    expected_generation: 0,
                },
                "Completed",
                TurnTerminal::Completed,
            ),
            (
                "cancel-status",
                TurnCommand::Cancel {
                    turn_id: TurnAuthorityId(0),
                    expected_generation: 0,
                },
                "Cancelled",
                TurnTerminal::Cancelled,
            ),
            (
                "fail-status",
                TurnCommand::Fail {
                    turn_id: TurnAuthorityId(0),
                    expected_generation: 0,
                    reason: "boom".to_string(),
                },
                "Failed",
                TurnTerminal::Failed {
                    reason: "boom".to_string(),
                },
            ),
        ] {
            let repo = repo().await;
            let (turn_id, workflow_id) = created_turn(&repo, key, 12).await;
            let command = match command {
                TurnCommand::Complete { .. } => TurnCommand::Complete {
                    turn_id,
                    expected_generation: 0,
                },
                TurnCommand::Cancel { .. } => TurnCommand::Cancel {
                    turn_id,
                    expected_generation: 0,
                },
                TurnCommand::Fail { reason, .. } => TurnCommand::Fail {
                    turn_id,
                    expected_generation: 0,
                    reason,
                },
                TurnCommand::Accept { .. } | TurnCommand::Materialize { .. } => unreachable!(),
            };
            let step = repo.terminate_authoritative_turn(command).await.unwrap();
            assert_eq!(
                step.outcome,
                TurnOutcome::Terminal {
                    generation: 1,
                    terminal: expected_terminal,
                    disposition: AcceptedDisposition::Runtime,
                }
            );
            assert_eq!(
                workflow_status_version_generation_transition_count(&repo, workflow_id).await,
                (expected_status.to_string(), 2, 1, 2)
            );
        }
    }

    #[tokio::test]
    async fn repository_refines_scoped_accept_replay_and_owner_release() {
        let repo = repo().await;
        let created = repo
            .accept_authoritative_turn(&input("conv-a", "same", 1))
            .await
            .unwrap();
        let TurnOutcome::Created { turn_id } = created.outcome else {
            panic!("expected created turn")
        };
        let workflow_id = repo.workflow_id_for_turn(turn_id).await.unwrap().unwrap();
        assert_eq!(
            repo.list_discoverable_accepted_turns(&ConversationAuthority("conv-a".to_string()))
                .await
                .unwrap(),
            vec![(turn_id, workflow_id)]
        );
        assert!(matches!(
            repo.accept_authoritative_turn(&input("conv-a", "same", 1))
                .await
                .unwrap()
                .outcome,
            TurnOutcome::ExactReplay { .. }
        ));
        assert!(repo
            .accept_authoritative_turn(&input("conv-a", "other", 2))
            .await
            .is_err());
        assert!(repo
            .accept_authoritative_turn(&input("conv-b", "same", 1))
            .await
            .is_ok());
        repo.terminate_authoritative_turn(TurnCommand::Cancel {
            turn_id,
            expected_generation: 0,
        })
        .await
        .unwrap();
        let workflow_id = repo.workflow_id_for_turn(turn_id).await.unwrap().unwrap();
        let (workflow_status, effect_status): (String, String) = sqlx::query_as(
            "SELECT wf.status, e.status
             FROM workflows wf
             JOIN workflow_effects e ON e.workflow_id = wf.workflow_id
             WHERE wf.workflow_id = ?1",
        )
        .bind(i64::try_from(workflow_id.0).unwrap())
        .fetch_one(&repo.pool)
        .await
        .unwrap();
        assert_eq!(workflow_status, "Cancelled");
        assert_eq!(effect_status, "Invalidated");
        assert!(repo
            .accept_authoritative_turn(&input("conv-a", "other", 2))
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn terminal_replay_is_idempotent_and_different_terminal_conflicts() {
        let repo = repo().await;
        let created = repo
            .accept_authoritative_turn(&input("conv-a", "terminal-replay", 10))
            .await
            .unwrap();
        let TurnOutcome::Created { turn_id } = created.outcome else {
            panic!("expected created turn")
        };
        repo.terminate_authoritative_turn(TurnCommand::Cancel {
            turn_id,
            expected_generation: 0,
        })
        .await
        .unwrap();
        let replay = repo
            .terminate_authoritative_turn(TurnCommand::Cancel {
                turn_id,
                expected_generation: 0,
            })
            .await
            .unwrap();
        assert_eq!(
            replay,
            TurnStep {
                outcome: TurnOutcome::TerminalReplay {
                    generation: 1,
                    terminal: TurnTerminal::Cancelled,
                    disposition: AcceptedDisposition::Runtime,
                },
                owed_effects: Vec::new(),
            }
        );
        assert!(repo
            .terminate_authoritative_turn(TurnCommand::Complete {
                turn_id,
                expected_generation: 0,
            })
            .await
            .is_err());
    }

    #[tokio::test]
    async fn failed_terminal_exact_reason_replays_and_different_reason_conflicts() {
        let repo = repo().await;
        let (turn_id, workflow_id) = created_turn(&repo, "failed-replay", 13).await;
        repo.terminate_authoritative_turn(TurnCommand::Fail {
            turn_id,
            expected_generation: 0,
            reason: "exact".to_string(),
        })
        .await
        .unwrap();
        let replay = repo
            .terminate_authoritative_turn(TurnCommand::Fail {
                turn_id,
                expected_generation: 0,
                reason: "exact".to_string(),
            })
            .await
            .unwrap();
        assert_eq!(
            replay.outcome,
            TurnOutcome::TerminalReplay {
                generation: 1,
                terminal: TurnTerminal::Failed {
                    reason: "exact".to_string(),
                },
                disposition: AcceptedDisposition::Runtime,
            }
        );
        assert!(repo
            .terminate_authoritative_turn(TurnCommand::Fail {
                turn_id,
                expected_generation: 0,
                reason: "different".to_string(),
            })
            .await
            .is_err());
        assert_eq!(
            workflow_status_version_generation_transition_count(&repo, workflow_id).await,
            ("Failed".to_string(), 2, 1, 2)
        );
    }

    #[tokio::test]
    async fn after_commit_terminal_retry_replays_without_second_write() {
        let repo = repo().await;
        let (turn_id, workflow_id) = created_turn(&repo, "terminal-after-replay", 14).await;
        assert!(repo
            .terminate_authoritative_turn_at_cut(
                TurnCommand::Complete {
                    turn_id,
                    expected_generation: 0,
                },
                TransactionCut::AfterCommit,
            )
            .await
            .is_err());
        let retry = repo
            .terminate_authoritative_turn(TurnCommand::Complete {
                turn_id,
                expected_generation: 0,
            })
            .await
            .unwrap();
        assert!(matches!(retry.outcome, TurnOutcome::TerminalReplay { .. }));
        assert_eq!(
            workflow_status_version_generation_transition_count(&repo, workflow_id).await,
            ("Completed".to_string(), 2, 1, 2)
        );
    }

    #[tokio::test]
    async fn terminal_cas_marks_active_attempts_authority_lost_removes_lease_and_invalidates_effect(
    ) {
        let repo = repo().await;
        let (turn_id, workflow_id) = created_turn(&repo, "authority-lost", 15).await;
        let attempt = repo
            .begin_attempt(&crate::workflow::BeginAttemptInput {
                workflow_id,
                effect_id: phoenix_workflow::EffectId(DIRECT_TURN_EFFECT_ID),
                attempt_id: phoenix_workflow::AttemptId(77),
                process_incarnation: phoenix_workflow::ProcessIncarnation(1),
                now: phoenix_workflow::Timestamp(1),
                lease_until: Some(phoenix_workflow::LeaseExpiry(99)),
            })
            .await
            .unwrap();
        let authority = attempt.authority.unwrap();
        repo.record_observation(&crate::workflow::RecordObservationInput {
            authority,
            observation_id: 1,
            now: phoenix_workflow::Timestamp(2),
            observed_at: phoenix_workflow::Timestamp(2),
            observation_codec: local_codec(&direct_turn_profile::intent_codec()),
            observation_payload: b"{}".to_vec(),
        })
        .await
        .unwrap();
        repo.terminate_authoritative_turn(TurnCommand::Cancel {
            turn_id,
            expected_generation: 0,
        })
        .await
        .unwrap();
        let (attempt_status, lease_count, effect_status): (String, i64, String) = sqlx::query_as(
            "SELECT a.status,
                    (SELECT COUNT(*) FROM workflow_reclaimable_leases l WHERE l.workflow_id = a.workflow_id),
                    e.status
             FROM workflow_attempts a
             JOIN workflow_effects e ON e.workflow_id = a.workflow_id AND e.effect_id = a.effect_id
             WHERE a.workflow_id = ?1 AND a.attempt_id = ?2",
        )
        .bind(i64::try_from(workflow_id.0).unwrap())
        .bind(77_i64)
        .fetch_one(&repo.pool)
        .await
        .unwrap();
        assert_eq!(attempt_status, "AuthorityLost");
        assert_eq!(lease_count, 0);
        assert_eq!(effect_status, "Invalidated");
    }

    #[tokio::test]
    async fn concurrent_terminal_allows_single_winner_and_loser_replays() {
        let repo = repo().await;
        let (turn_id, workflow_id) = created_turn(&repo, "concurrent-terminal", 16).await;
        let left_repo = repo.clone();
        let right_repo = repo.clone();
        let left = tokio::spawn(async move {
            left_repo
                .terminate_authoritative_turn(TurnCommand::Cancel {
                    turn_id,
                    expected_generation: 0,
                })
                .await
                .unwrap()
        });
        let right = tokio::spawn(async move {
            right_repo
                .terminate_authoritative_turn(TurnCommand::Cancel {
                    turn_id,
                    expected_generation: 0,
                })
                .await
                .unwrap()
        });
        let outcomes = [left.await.unwrap().outcome, right.await.unwrap().outcome];
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| matches!(outcome, TurnOutcome::Terminal { .. }))
                .count(),
            1
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| matches!(outcome, TurnOutcome::TerminalReplay { .. }))
                .count(),
            1
        );
        assert_eq!(
            workflow_status_version_generation_transition_count(&repo, workflow_id).await,
            ("Cancelled".to_string(), 2, 1, 2)
        );
    }

    #[tokio::test]
    async fn schema_rejects_cross_conversation_canonical_message() {
        let repo = repo().await;
        let created = repo
            .accept_authoritative_turn(&input("conv-a", "schema", 11))
            .await
            .unwrap();
        let TurnOutcome::Created { turn_id } = created.outcome else {
            panic!("expected created turn")
        };
        insert_message(&repo, "conv-b", "foreign-message").await;
        assert!(repo
            .materialize_authoritative_turn(
                turn_id,
                0,
                CanonicalMessageId("foreign-message".into())
            )
            .await
            .is_err());
    }

    #[tokio::test]
    async fn deleting_authoritative_turn_deletes_owned_workflow_and_effects() {
        let repo = repo().await;
        let created = repo
            .accept_authoritative_turn(&input("conv-a", "delete", 4))
            .await
            .unwrap();
        let TurnOutcome::Created { turn_id } = created.outcome else {
            panic!("expected created turn")
        };
        let workflow_id = repo.workflow_id_for_turn(turn_id).await.unwrap().unwrap();
        sqlx::query("DELETE FROM durable_turns WHERE turn_id = ?1")
            .bind(i64::try_from(turn_id.0).unwrap())
            .execute(&repo.pool)
            .await
            .unwrap();
        let workflow_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM workflows WHERE workflow_id = ?1")
                .bind(i64::try_from(workflow_id.0).unwrap())
                .fetch_one(&repo.pool)
                .await
                .unwrap();
        assert_eq!(workflow_count, 0);
    }

    #[tokio::test]
    async fn accept_creates_atomic_workflow_child_effect_and_relation() {
        let repo = repo().await;
        let created = repo
            .accept_authoritative_turn(&input("conv-a", "runtime", 9))
            .await
            .unwrap();
        let TurnOutcome::Created { turn_id } = created.outcome else {
            panic!("expected created turn")
        };
        let workflow_id = repo.workflow_id_for_turn(turn_id).await.unwrap().unwrap();

        let effect_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM workflow_effects WHERE workflow_id = ?1 AND effect_id = 1 AND status = 'Eligible'",
        )
        .bind(i64::try_from(workflow_id.0).unwrap())
        .fetch_one(&repo.pool)
        .await
        .unwrap();
        assert_eq!(effect_count, 1);

        let discoverable = repo
            .list_discoverable_accepted_turns(&ConversationAuthority("conv-a".to_string()))
            .await
            .unwrap();
        assert_eq!(discoverable, vec![(turn_id, workflow_id)]);
    }

    #[tokio::test]
    async fn global_discovery_is_bounded_cursor_ordered_and_filters_materialized_terminal_steering()
    {
        let repo = repo().await;
        let first = repo
            .accept_authoritative_turn(&input("conv-a", "first", 21))
            .await
            .unwrap();
        let TurnOutcome::Created { turn_id: first_id } = first.outcome else {
            panic!("expected first runtime turn")
        };
        let first_workflow = repo.workflow_id_for_turn(first_id).await.unwrap().unwrap();
        let second = repo
            .accept_authoritative_turn(&input("conv-b", "second", 22))
            .await
            .unwrap();
        let TurnOutcome::Created { turn_id: second_id } = second.outcome else {
            panic!("expected second runtime turn")
        };
        let second_workflow = repo.workflow_id_for_turn(second_id).await.unwrap().unwrap();
        let materialized = repo
            .accept_authoritative_turn(&input("conv-c", "materialized", 23))
            .await
            .unwrap();
        let TurnOutcome::Created {
            turn_id: materialized_id,
        } = materialized.outcome
        else {
            panic!("expected materialized runtime turn")
        };
        let materialized_workflow = repo
            .workflow_id_for_turn(materialized_id)
            .await
            .unwrap()
            .unwrap();
        let materialized_claim = repo
            .claim_authoritative_turn(&claim_input(materialized_workflow, materialized_id, 100))
            .await
            .unwrap();
        repo.materialize_authoritative_turn(&materialize_input(
            materialized_id,
            materialized_claim.authority.unwrap(),
            100,
            100,
            "message-conv-c-materialized",
            100,
        ))
        .await
        .unwrap();
        let terminal = repo
            .accept_authoritative_turn(&input_with_disposition(
                "conv-a",
                "terminal",
                24,
                AcceptedDisposition::Steering,
            ))
            .await
            .unwrap();
        let TurnOutcome::Created {
            turn_id: terminal_id,
        } = terminal.outcome
        else {
            panic!("expected steering turn")
        };
        repo.terminate_authoritative_turn(TurnCommand::Cancel {
            turn_id: terminal_id,
            expected_generation: 0,
        })
        .await
        .unwrap();

        let first_page = repo
            .list_discoverable_accepted_runtime_direct_turns(None, 1)
            .await
            .unwrap();
        assert_eq!(
            first_page
                .iter()
                .map(|row| (row.turn_id, row.workflow_id, row.conversation.0.as_str()))
                .collect::<Vec<_>>(),
            vec![(first_id, first_workflow, "conv-a")]
        );
        let second_page = repo
            .list_discoverable_accepted_runtime_direct_turns(
                Some(DirectTurnDiscoveryCursor {
                    turn_id: first_id,
                    workflow_id: first_workflow,
                }),
                10,
            )
            .await
            .unwrap();
        assert_eq!(
            second_page
                .iter()
                .map(|row| (row.turn_id, row.workflow_id, row.conversation.0.as_str()))
                .collect::<Vec<_>>(),
            vec![(second_id, second_workflow, "conv-b")]
        );
        assert!(repo
            .list_discoverable_accepted_runtime_direct_turns(None, 0)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn claim_contention_and_expired_reclaim_are_deterministic() {
        let repo = repo().await;
        let created = repo
            .accept_authoritative_turn(&input("conv-a", "claim", 7))
            .await
            .unwrap();
        let TurnOutcome::Created { turn_id } = created.outcome else {
            panic!("expected created turn")
        };
        let workflow_id = repo.workflow_id_for_turn(turn_id).await.unwrap().unwrap();

        let first = repo
            .claim_authoritative_turn(&claim_input(workflow_id, turn_id, 20))
            .await
            .unwrap();
        assert_eq!(first.outcome, ClaimOutcome::Started);
        let second = repo
            .claim_authoritative_turn(&claim_input(workflow_id, turn_id, 21))
            .await
            .unwrap();
        assert_eq!(second.outcome, ClaimOutcome::AuthorityConflict);

        let reclaimed = repo
            .claim_authoritative_turn(&ClaimAuthoritativeTurnInput {
                turn_id,
                workflow_id,
                process_incarnation: ProcessIncarnation(40),
                now: Timestamp(40),
                lease_until: LeaseExpiry(50),
            })
            .await
            .unwrap();
        assert_eq!(reclaimed.outcome, ClaimOutcome::Started);
        let attempts = repo
            .list_attempts(workflow_id, EffectId(DIRECT_TURN_EFFECT_ID))
            .await
            .unwrap();
        assert_eq!(attempts.len(), 2);
        assert_eq!(attempts[0].status, AttemptStatus::AuthorityLost);
        assert_eq!(attempts[1].status, AttemptStatus::Begun);
    }

    #[tokio::test]
    async fn release_dispatch_failure_exactly_releases_and_reclaims() {
        let repo = repo().await;
        let created = repo
            .accept_authoritative_turn(&input("conv-a", "release", 8))
            .await
            .unwrap();
        let TurnOutcome::Created { turn_id } = created.outcome else {
            panic!("expected created turn")
        };
        let workflow_id = repo.workflow_id_for_turn(turn_id).await.unwrap().unwrap();
        let claim = repo
            .claim_authoritative_turn(&claim_input(workflow_id, turn_id, 30))
            .await
            .unwrap();
        let authority = claim.authority.unwrap();
        assert_eq!(
            repo.release_authoritative_turn_dispatch_failure(&ReleaseAuthoritativeTurnInput {
                authority: authority.clone(),
                now: Timestamp(31),
            })
            .await
            .unwrap(),
            AuthorityOutcome::Authorized
        );
        assert_eq!(
            repo.release_authoritative_turn_dispatch_failure(&ReleaseAuthoritativeTurnInput {
                authority,
                now: Timestamp(31),
            })
            .await
            .unwrap(),
            AuthorityOutcome::StaleAuthority
        );
        let reclaimed = repo
            .claim_authoritative_turn(&claim_input(workflow_id, turn_id, 32))
            .await
            .unwrap();
        assert_eq!(reclaimed.outcome, ClaimOutcome::Started);
    }

    #[tokio::test]
    async fn materialization_preflight_fresh_replay_stale_and_wrong_payload() {
        let repo = repo().await;
        let created = repo
            .accept_authoritative_turn(&input("conv-a", "preflight", 31))
            .await
            .unwrap();
        let TurnOutcome::Created { turn_id } = created.outcome else {
            panic!("expected created turn")
        };
        let workflow_id = repo.workflow_id_for_turn(turn_id).await.unwrap().unwrap();
        let claim = repo
            .claim_authoritative_turn(&claim_input(workflow_id, turn_id, 70))
            .await
            .unwrap();
        let authority = claim.authority.unwrap();

        assert!(matches!(
            repo.preflight_direct_turn_materialization(&preflight_input(
                turn_id,
                authority.clone(),
                "message-conv-a-preflight",
                70,
            ))
            .await
            .unwrap(),
            DirectTurnMaterializationEligibility::Fresh
        ));
        assert!(repo
            .preflight_direct_turn_materialization(&preflight_input(
                turn_id,
                authority.clone(),
                "message-other",
                70,
            ))
            .await
            .is_err());

        let stale_authority = LocalAttemptAuthority {
            attempt_id: AttemptId(authority.attempt_id.0 + 100),
            ..authority.clone()
        };
        assert!(matches!(
            repo.preflight_direct_turn_materialization(&preflight_input(
                turn_id,
                stale_authority.clone(),
                "message-conv-a-preflight",
                70,
            ))
            .await
            .unwrap(),
            DirectTurnMaterializationEligibility::StaleAuthority
        ));
        assert!(matches!(
            repo.preflight_direct_turn_materialization(&preflight_input(
                TurnAuthorityId(turn_id.0 + 100),
                authority.clone(),
                "message-conv-a-preflight",
                70,
            ))
            .await
            .unwrap(),
            DirectTurnMaterializationEligibility::StaleAuthority
        ));
        assert!(matches!(
            repo.preflight_direct_turn_materialization(&preflight_input(
                turn_id,
                authority.clone(),
                "message-conv-a-preflight",
                80,
            ))
            .await
            .unwrap(),
            DirectTurnMaterializationEligibility::StaleAuthority
        ));

        let materialized = repo
            .materialize_authoritative_turn(&materialize_input(
                turn_id,
                authority.clone(),
                1,
                1,
                "message-conv-a-preflight",
                70,
            ))
            .await
            .unwrap();
        assert!(materialized.message.is_some());
        let replay = repo
            .preflight_direct_turn_materialization(&preflight_input(
                turn_id,
                authority.clone(),
                "message-conv-a-preflight",
                70,
            ))
            .await
            .unwrap();
        assert_eq!(replay, DirectTurnMaterializationEligibility::ExactReplay);

        let replay_with_stale_attempt = repo
            .preflight_direct_turn_materialization(&preflight_input(
                turn_id,
                stale_authority,
                "message-conv-a-preflight",
                70,
            ))
            .await
            .unwrap();
        assert!(matches!(
            replay_with_stale_attempt,
            DirectTurnMaterializationEligibility::ExactReplay
        ));
    }

    #[tokio::test]
    async fn materialization_exact_replay_conflict_and_terminal_fencing() {
        let repo = repo().await;
        let created = repo
            .accept_authoritative_turn(&input("conv-a", "materialize-phase3", 9))
            .await
            .unwrap();
        let TurnOutcome::Created { turn_id } = created.outcome else {
            panic!("expected created turn")
        };
        let workflow_id = repo.workflow_id_for_turn(turn_id).await.unwrap().unwrap();
        let claim = repo
            .claim_authoritative_turn(&claim_input(workflow_id, turn_id, 50))
            .await
            .unwrap();
        let authority = claim.authority.unwrap();
        let first = repo
            .materialize_authoritative_turn(&materialize_input(
                turn_id,
                authority.clone(),
                10,
                10,
                "message-conv-a-materialize-phase3",
                50,
            ))
            .await
            .unwrap();
        assert_eq!(first.authority_outcome, AuthorityOutcome::Authorized);
        assert!(matches!(first.outcome, TurnOutcome::Materialized { .. }));
        let replay = repo
            .materialize_authoritative_turn(&materialize_input(
                turn_id,
                authority.clone(),
                11,
                11,
                "message-conv-a-materialize-phase3",
                50,
            ))
            .await
            .unwrap();
        assert!(matches!(
            replay.outcome,
            TurnOutcome::MaterializationReplay { .. }
        ));
        let conflict = repo
            .materialize_authoritative_turn(&materialize_input(
                turn_id,
                authority.clone(),
                12,
                12,
                "message-other",
                50,
            ))
            .await;
        assert!(conflict.is_err());

        let terminal_created = repo
            .accept_authoritative_turn(&input_with_disposition(
                "conv-a",
                "steering-terminal",
                12,
                AcceptedDisposition::Steering,
            ))
            .await
            .unwrap();
        let TurnOutcome::Created { turn_id } = terminal_created.outcome else {
            panic!("expected created turn")
        };
        let terminal_workflow_id = repo.workflow_id_for_turn(turn_id).await.unwrap().unwrap();
        let claim = repo
            .claim_authoritative_turn(&claim_input(terminal_workflow_id, turn_id, 60))
            .await
            .unwrap();
        let authority = claim.authority.unwrap();
        repo.terminate_authoritative_turn(TurnCommand::Cancel {
            turn_id,
            expected_generation: 0,
        })
        .await
        .unwrap();
        let terminal = repo
            .materialize_authoritative_turn(&materialize_input(
                turn_id, authority, 20, 20, "late", 60,
            ))
            .await;
        assert!(terminal.is_err());
    }
}
