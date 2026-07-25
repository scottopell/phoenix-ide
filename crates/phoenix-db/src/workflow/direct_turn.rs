use super::WorkflowRepository;
use crate::{DbError, DbResult};
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterializeAuthoritativeTurnInput {
    pub authority: super::LocalAttemptAuthority,
    pub receipt_id: ReceiptId,
    pub delivery_id: DeliveryId,
    pub message_id: CanonicalMessageId,
    pub now: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterializeAuthoritativeTurnResult {
    pub outcome: TurnOutcome,
    pub authority_outcome: AuthorityOutcome,
    pub receipt: Option<super::LocalReceiptRecord>,
    pub delivery: Option<super::LocalDeliveryRecord>,
    pub canonical_turn: DurableTurn,
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
             WHERE workflow_id = ?1 AND attempt_id = ?2 AND lease_until > ?3",
        )
        .bind(to_i64(input.authority.workflow_id.0, "workflow_id")?)
        .bind(to_i64(input.authority.attempt_id.0, "attempt_id")?)
        .bind(to_i64(input.now.0, "now")?)
        .execute(&mut *tx.tx)
        .await?
        .rows_affected();
        if updated == 0 {
            tx.rollback().await?;
            return Ok(AuthorityOutcome::StaleAuthority);
        }
        sqlx::query(
            "UPDATE workflow_attempts
             SET status = 'AuthorityLost'
             WHERE workflow_id = ?1 AND attempt_id = ?2 AND status IN ('Begun', 'ObservationRecorded')",
        )
        .bind(to_i64(input.authority.workflow_id.0, "workflow_id")?)
        .bind(to_i64(input.authority.attempt_id.0, "attempt_id")?)
        .execute(&mut *tx.tx)
        .await?;
        sqlx::query(
            "UPDATE workflow_effects
             SET status = 'Eligible'
             WHERE workflow_id = ?1 AND effect_id = ?2 AND status = 'Executing'",
        )
        .bind(to_i64(input.authority.workflow_id.0, "workflow_id")?)
        .bind(to_i64(input.authority.effect_id.0, "effect_id")?)
        .execute(&mut *tx.tx)
        .await?;
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
        let row = sqlx::query("SELECT * FROM durable_turns WHERE turn_id = ?1")
            .bind(to_i64(input.authority.workflow_id.0, "workflow_id")?)
            .fetch_optional(&mut *tx.tx)
            .await?;
        let turn_id = decode_turn_id_from_authority(&input.authority)?;
        let turn = match row {
            Some(_) => load_turn_for_workflow_tx(&mut tx.tx, turn_id, input.authority.workflow_id)
                .await?
                .ok_or_else(|| conflict(TurnConflict::UnknownTurn))?,
            None => return Err(conflict(TurnConflict::UnknownTurn)),
        };
        let mut model =
            phoenix_workflow::DurableTurnModel::from_turns([turn.clone()]).map_err(conflict)?;
        let step = model
            .apply(TurnCommand::Materialize {
                turn_id,
                expected_generation: input.authority.generation.0,
                message_id: input.message_id.clone(),
            })
            .map_err(conflict)?;
        let acceptance = tx
            .accept_receipt_and_delivery(&super::AcceptReceiptInput {
                authority: input.authority.clone(),
                receipt_id: input.receipt_id,
                delivery_id: input.delivery_id,
                attempt_id: Some(input.authority.attempt_id),
                origin: phoenix_workflow::ReceiptOrigin::Execution,
                receipt_codec: local_codec_owned(&direct_turn_profile::event_codec()),
                receipt_payload: serde_json::to_vec(&()).map_err(|e| {
                    DbError::Serialization(format!("encode direct-turn receipt: {e}"))
                })?,
                receipt_event_codec: local_codec_owned(&direct_turn_profile::event_codec()),
                receipt_event_payload: serde_json::to_vec(&()).map_err(|e| {
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
            });
        }
        if matches!(step.outcome, TurnOutcome::Materialized { .. }) {
            let updated = sqlx::query(
                "UPDATE durable_turns SET canonical_message_id = ?2
                 WHERE turn_id = ?1 AND generation = ?3 AND terminal_kind IS NULL
                   AND canonical_message_id IS NULL AND workflow_id = ?4",
            )
            .bind(to_i64(turn_id.0, "turn_id")?)
            .bind(&input.message_id.0)
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
        }
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
        receipt_handle: turn_id.0.to_string().into_bytes(),
        disposition_handle: workflow_id.0.to_string().into_bytes(),
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

fn local_codec_owned(codec: &phoenix_workflow::CodecRef) -> LocalCodec {
    LocalCodec {
        family: codec.family.to_string(),
        version: codec.version,
    }
}

fn decode_turn_id_from_authority(
    authority: &super::LocalAttemptAuthority,
) -> DbResult<TurnAuthorityId> {
    if authority.effect_id != EffectId(DIRECT_TURN_EFFECT_ID) {
        return Err(DbError::Serialization(
            "direct-turn authority referenced unexpected effect".to_string(),
        ));
    }
    Ok(TurnAuthorityId(authority.workflow_id.0))
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

async fn finish_raw_transaction_at_cut(
    tx: sqlx::Transaction<'_, sqlx::Sqlite>,
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
        WorkflowRepository::new(db.pool().clone())
    }

    async fn insert_message(repo: &WorkflowRepository, conversation: &str, message_id: &str) {
        sqlx::query(
            "INSERT INTO messages (
                message_id, conversation_id, sequence_id, message_type, content, created_at
             ) VALUES (?1, ?2, 1, 'user', '{}', '2026-01-01T00:00:00Z')",
        )
        .bind(message_id)
        .bind(conversation)
        .execute(&repo.pool)
        .await
        .unwrap();
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
            prepared: PreparedTurn::from_exact_payload(vec![seed]),
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

    fn materialize_input(
        authority: LocalAttemptAuthority,
        receipt_id: u64,
        delivery_id: u64,
        message_id: &str,
        now: u64,
    ) -> MaterializeAuthoritativeTurnInput {
        MaterializeAuthoritativeTurnInput {
            authority,
            receipt_id: ReceiptId(receipt_id),
            delivery_id: DeliveryId(delivery_id),
            message_id: CanonicalMessageId(message_id.to_string()),
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
        let message = CanonicalMessageId("message-before".to_string());
        insert_message(&before_repo, "conv-a", &message.0).await;
        assert!(before_repo
            .materialize_authoritative_turn_at_cut(
                &materialize_input(authority.clone(), 1, 1, &message.0, 10),
                TransactionCut::BeforeCommit,
            )
            .await
            .is_err());
        assert!(matches!(
            before_repo
                .materialize_authoritative_turn(&materialize_input(authority, 1, 1, &message.0, 10))
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
        let message = CanonicalMessageId("message-after".to_string());
        insert_message(&after_repo, "conv-a", &message.0).await;
        assert!(after_repo
            .materialize_authoritative_turn_at_cut(
                &materialize_input(authority.clone(), 1, 1, &message.0, 11),
                TransactionCut::AfterCommit,
            )
            .await
            .is_err());
        assert!(matches!(
            after_repo
                .materialize_authoritative_turn(&materialize_input(authority, 2, 2, &message.0, 11))
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
        insert_message(&repo, "conv-a", "message-phase3").await;
        let first = repo
            .materialize_authoritative_turn(&materialize_input(
                authority.clone(),
                10,
                10,
                "message-phase3",
                50,
            ))
            .await
            .unwrap();
        assert_eq!(first.authority_outcome, AuthorityOutcome::Authorized);
        assert!(matches!(first.outcome, TurnOutcome::Materialized { .. }));
        let replay = repo
            .materialize_authoritative_turn(&materialize_input(
                authority.clone(),
                11,
                11,
                "message-phase3",
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
            .materialize_authoritative_turn(&materialize_input(authority, 20, 20, "late", 60))
            .await;
        assert!(terminal.is_err());
    }
}
