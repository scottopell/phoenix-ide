use super::WorkflowRepository;
use crate::sqlite_telemetry::{SqliteOperation, SqlitePhase, SqliteTelemetry};
use crate::{DbError, DbResult};
use chrono::{DateTime, Utc};
use phoenix_core::domain::db_schema::{
    ConvState, FileAttachment, ImageData, Message, MessageContent,
};
use phoenix_core::domain::sm_event::{
    DirectTurnAttemptAuthority, PreparedDirectTurnPayload, SubmittedDirectTurnFileAttachment,
    SubmittedDirectTurnIdentity,
};
use phoenix_workflow::{
    direct_turn_profile, AcceptedDisposition, AttemptId, AttemptStatus, AuthorityOutcome,
    CanonicalMessageId, ClaimOutcome, ClientTurnKey, ConversationAuthority, DeliveryId,
    DurableTurn, EffectId, EffectRole, EffectStatus, ExecutionCapability, Generation, LeaseExpiry,
    Materialization, PreparedTurn, ProcessIncarnation, ReceiptId, Timestamp, TurnAuthorityId,
    TurnCommand, TurnConflict, TurnLifecycle, TurnOutcome, TurnStep, TurnTerminal, Version,
    WorkflowId, WorkflowStatus,
};
use sqlx::{Acquire, Connection, Row};

use super::{
    CommitTransitionPlanCas, CreateWorkflowWithExternalAcceptance, DeliveryResolutionDecision,
    DeliveryResolutionPlan, LocalCodec, LocalEffectDecl,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransactionCut {
    None,
    BeforeCommit,
    AfterCommit,
}

const DIRECT_TURN_ACCEPTED_TRANSITION_ID: u64 = 1;
const DIRECT_TURN_MATERIALIZED_TRANSITION_ID: u64 = 2;
const DIRECT_TURN_TERMINAL_TRANSITION_ID: u64 = 3;
const DIRECT_TURN_EFFECT_ID: u64 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptAuthoritativeTurn {
    pub client_key: ClientTurnKey,
    pub prepared: PreparedTurn,
    pub disposition: AcceptedDisposition,
    pub accepted_at: phoenix_workflow::Timestamp,
}

impl AcceptAuthoritativeTurn {
    #[must_use]
    pub fn conversation(&self) -> &ConversationAuthority {
        self.prepared.target()
    }
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
pub enum ClaimAuthoritativeTurnEstablishment {
    Established(Box<ClaimAuthoritativeTurnResult>),
    KnownNotCommitted(String),
    Unclassifiable(String),
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
pub struct DiscoverableAcceptedTurnPage {
    pub candidates: Vec<DiscoverableAcceptedTurn>,
    pub next_cursor: Option<DirectTurnDiscoveryCursor>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoverableTerminalObligation {
    pub turn_id: TurnAuthorityId,
    pub conversation: ConversationAuthority,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmittedIdentityChanged {
    pub turn: DurableTurn,
    pub stored: SubmittedDirectTurnIdentity,
}

#[derive(Debug, thiserror::Error)]
pub enum ScopedDirectTurnReplayError {
    #[error(transparent)]
    Db(#[from] DbError),
    #[error("submitted direct-turn identity changed for turn {turn_id:?}")]
    SubmittedIdentityChanged {
        turn_id: TurnAuthorityId,
        changed: Box<SubmittedIdentityChanged>,
    },
}

impl From<SubmittedIdentityChanged> for ScopedDirectTurnReplayError {
    fn from(changed: SubmittedIdentityChanged) -> Self {
        Self::SubmittedIdentityChanged {
            turn_id: changed.turn.id,
            changed: Box::new(changed),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopedDirectTurnReplayLookup {
    Missing,
    Exact {
        turn: DurableTurn,
        prepared: Box<PreparedDirectTurnPayload>,
    },
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

#[derive(Debug, Clone, PartialEq)]
pub struct MaterializeAuthoritativeTurnInput {
    pub turn_id: TurnAuthorityId,
    pub authority: super::LocalAttemptAuthority,
    pub prepared: PreparedDirectTurnPayload,
    pub sequence_id: i64,
    pub created_at: Timestamp,
    pub accepted_state: ConvState,
    pub state_updated_at: DateTime<Utc>,
    pub now: Timestamp,
}

#[derive(Debug, Clone)]
pub struct AuthoritativeTurnMaterialization {
    pub message: Box<Message>,
    pub turn_id: TurnAuthorityId,
    pub generation: u64,
}

#[derive(Debug, Clone)]
pub enum MaterializeAuthoritativeTurnOutcome {
    Materialized(AuthoritativeTurnMaterialization),
    ExactReplay(AuthoritativeTurnMaterialization),
    ClassifiedCommitted(AuthoritativeTurnMaterialization),
    NotCommitted,
    StaleAuthority,
    CommandRejected(TurnConflict),
}

pub type MaterializeAuthoritativeTurnResult =
    super::LocalAuthorityResult<MaterializeAuthoritativeTurnOutcome>;

#[derive(Debug, Clone, PartialEq)]
pub struct PersistedConversationProjection {
    pub state: ConvState,
    pub state_updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TerminalizeAuthoritativeTurnInput {
    pub command: TurnCommand,
    pub projection: Option<PersistedConversationProjection>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DirectTurnTerminalObligationInput {
    pub turn_id: TurnAuthorityId,
    pub expected_generation: u64,
    pub terminal: TurnTerminal,
    pub projection: PersistedConversationProjection,
    pub response_message_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalEvidenceProbe {
    Established { transcript_generation: Option<i64> },
    KnownNotCommitted,
    Incomplete,
    Retired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalProjectionProbe {
    Missing,
    Current,
    Superseded,
    StillOwed,
    Unclassifiable,
}

#[derive(Debug, Clone)]
pub enum TerminalEvidenceExpectation {
    ObligationOnly {
        conversation_id: String,
    },
    Messages(Vec<Message>),
    MessageMutation {
        conversation_id: String,
        message_id: String,
        content: MessageContent,
        display_data: serde_json::Value,
    },
}

impl TerminalEvidenceExpectation {
    #[must_use]
    pub fn conversation_id(&self) -> &str {
        expected_conversation_id(self)
    }

    #[must_use]
    pub fn is_message_mutation(&self) -> bool {
        matches!(self, Self::MessageMutation { .. })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DirectTurnTerminalObligation {
    pub turn_id: TurnAuthorityId,
    pub expected_generation: u64,
    pub terminal: TurnTerminal,
    pub projection: PersistedConversationProjection,
    pub response_message_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AtomicContinuationSettlementInput {
    pub conversation_id: String,
    pub operation_id: String,
    pub message: Message,
    pub completed_state: ConvState,
    pub state_updated_at: DateTime<Utc>,
    pub command: TurnCommand,
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
        let mut tx = self.begin_immediate_tx().await?;
        if let Some(existing) = load_by_scoped_key(
            &self.pool,
            &mut tx.tx,
            input.conversation(),
            &input.client_key,
        )
        .await?
        {
            tx.rollback().await?;
            if existing.prepared != input.prepared
                || existing.lifecycle
                    != (TurnLifecycle::Accepted {
                        disposition: input.disposition,
                    })
            {
                return Err(prepared_semantics_changed(&existing.prepared));
            }
            return Ok(TurnStep {
                outcome: TurnOutcome::ExactReplay {
                    turn_id: existing.id,
                    disposition: input.disposition,
                },
                owed_effects: Vec::new(),
            });
        }
        let submitted_message_id =
            PreparedDirectTurnPayload::from_exact_bytes(input.prepared.payload())
                .map_err(|_| {
                    conflict(TurnConflict::CorruptAggregate(
                        "prepared payload decode failed",
                    ))
                })?
                .message_id()
                .to_string();
        let legacy_message_exists = sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS(
                 SELECT 1 FROM messages
                 WHERE conversation_id = ?1 AND message_id = ?2
             )",
        )
        .bind(&input.conversation().0)
        .bind(&submitted_message_id)
        .fetch_one(&mut *tx.tx)
        .await?
            != 0;
        if legacy_message_exists {
            tx.rollback().await?;
            return Err(prepared_semantics_changed(&input.prepared));
        }
        if input.disposition == AcceptedDisposition::Runtime {
            if let Some(owner) = sqlx::query_scalar::<_, i64>(
                "SELECT turn_id FROM durable_turns WHERE conversation_id = ?1 AND owns_conversation = 1",
            )
            .bind(&input.conversation().0)
            .fetch_optional(&mut *tx.tx)
            .await?
            {
                tx.rollback().await?;
                return Err(conflict(TurnConflict::ConversationAlreadyOwned {
                    owner: TurnAuthorityId(to_u64(owner, "turn_id")?),
                }));
            }
        } else if let Some(owner) =
            load_active_runtime_turn_tx(&self.pool, &mut tx.tx, input.conversation()).await?
        {
            if owner.conversation != *input.conversation() {
                tx.rollback().await?;
                return Err(DbError::Serialization(
                    "active runtime owner rehydrated under wrong conversation".to_string(),
                ));
            }
        }
        let turn_id = next_direct_turn_id_tx(&mut tx).await?;
        let workflow_id = super::next_global_workflow_id_tx(&mut tx).await?;
        insert_direct_turn_workflow_tx(&mut tx, workflow_id, turn_id, input).await?;
        let disposition = disposition_sql(input.disposition);
        let prepared_payload =
            PreparedDirectTurnPayload::from_exact_bytes(input.prepared.payload())
                .map_err(|error| DbError::Serialization(error.to_string()))?;
        sqlx::query(
            "INSERT INTO durable_turns (
                turn_id, conversation_id, client_turn_key, prepared_fingerprint,
                prepared_payload, disposition, generation, terminal_kind,
                terminal_reason, owns_conversation, canonical_message_id, workflow_id
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, NULL, NULL, ?7, NULL, ?8)",
        )
        .bind(to_i64(turn_id.0, "turn_id")?)
        .bind(&input.conversation().0)
        .bind(input.client_key.as_str())
        .bind(input.prepared.fingerprint())
        .bind(
            prepared_payload
                .to_normalized_bytes_without_attachments()
                .map_err(|error| DbError::Serialization(error.to_string()))?,
        )
        .bind(disposition)
        .bind(i64::from(input.disposition == AcceptedDisposition::Runtime))
        .bind(to_i64(workflow_id.0, "workflow_id")?)
        .execute(&mut *tx.tx)
        .await
        .map_err(map_constraint)?;
        insert_prepared_turn_attachments_tx(&mut tx.tx, turn_id, &prepared_payload).await?;
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
                conversation: input.conversation().clone(),
                client_key: input.client_key.clone(),
                prepared: input.prepared.clone(),
                disposition: input.disposition,
            })
            .map_err(conflict)
    }

    pub async fn exact_turn_retired(
        &self,
        turn_id: TurnAuthorityId,
        conversation_id: &str,
    ) -> DbResult<bool> {
        Ok(sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS(
                SELECT 1 FROM direct_turn_retirements
                WHERE turn_id = ?1 AND conversation_id = ?2
             )",
        )
        .bind(to_i64(turn_id.0, "turn_id")?)
        .bind(conversation_id)
        .fetch_one(&self.pool)
        .await?
            != 0)
    }

    pub async fn load_owning_authoritative_turn(
        &self,
        conversation: &ConversationAuthority,
    ) -> DbResult<Option<DurableTurn>> {
        let turn_id: Option<i64> = sqlx::query_scalar(
            "SELECT turn_id FROM durable_turns
             WHERE conversation_id = ?1 AND owns_conversation = 1
               AND terminal_kind IS NULL
             ORDER BY turn_id DESC LIMIT 1",
        )
        .bind(&conversation.0)
        .fetch_optional(&self.pool)
        .await?;
        let Some(turn_id) = turn_id else {
            return Ok(None);
        };
        self.load_authoritative_turn(TurnAuthorityId(to_u64(turn_id, "turn_id")?))
            .await
    }

    pub async fn load_authoritative_turn(
        &self,
        turn_id: TurnAuthorityId,
    ) -> DbResult<Option<DurableTurn>> {
        let row = sqlx::query("SELECT * FROM durable_turns WHERE turn_id = ?1")
            .bind(to_i64(turn_id.0, "turn_id")?)
            .fetch_optional(&self.pool)
            .await?;
        match row {
            Some(row) => Ok(Some(row_to_turn_pool(&self.pool, row).await?)),
            None => Ok(None),
        }
    }

    pub async fn lookup_scoped_direct_turn_replay(
        &self,
        conversation: &ConversationAuthority,
        client_key: &ClientTurnKey,
        submitted: &SubmittedDirectTurnIdentity,
    ) -> Result<ScopedDirectTurnReplayLookup, ScopedDirectTurnReplayError> {
        let Some(turn) = load_by_scoped_key_pool(&self.pool, conversation, client_key).await?
        else {
            return Ok(ScopedDirectTurnReplayLookup::Missing);
        };
        let stored = load_prepared_payload_pool(&self.pool, turn.id).await?;
        PreparedTurn::rehydrate(
            &turn.conversation,
            turn.prepared.fingerprint().to_string(),
            turn.prepared.payload().to_vec(),
        )
        .map_err(DbError::DirectTurnConflict)?;
        if !stored.submitted_identity_matches(submitted) {
            return Err(SubmittedIdentityChanged {
                turn,
                stored: stored.submitted,
            }
            .into());
        }
        Ok(ScopedDirectTurnReplayLookup::Exact {
            turn,
            prepared: Box::new(stored),
        })
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

    #[cfg(test)]
    async fn establish_authoritative_turn_claim_at_cut(
        &self,
        input: &ClaimAuthoritativeTurnInput,
        cut: TransactionCut,
    ) -> ClaimAuthoritativeTurnEstablishment {
        self.establish_authoritative_turn_claim_with_cut(input, cut)
            .await
    }

    pub async fn establish_authoritative_turn_claim(
        &self,
        input: &ClaimAuthoritativeTurnInput,
    ) -> ClaimAuthoritativeTurnEstablishment {
        self.establish_authoritative_turn_claim_with_cut(input, TransactionCut::None)
            .await
    }

    async fn establish_authoritative_turn_claim_with_cut(
        &self,
        input: &ClaimAuthoritativeTurnInput,
        cut: TransactionCut,
    ) -> ClaimAuthoritativeTurnEstablishment {
        let mut tx = match self.begin_tx().await {
            Ok(tx) => tx,
            Err(error) => {
                return ClaimAuthoritativeTurnEstablishment::KnownNotCommitted(error.to_string());
            }
        };
        let result = match self.claim_authoritative_turn_in_tx(&mut tx, input).await {
            Ok(result) => result,
            Err(error) => {
                return ClaimAuthoritativeTurnEstablishment::KnownNotCommitted(error.to_string());
            }
        };
        if result.outcome != ClaimOutcome::Started {
            return match tx.rollback().await {
                Ok(()) => ClaimAuthoritativeTurnEstablishment::Established(Box::new(result)),
                Err(error) => {
                    ClaimAuthoritativeTurnEstablishment::KnownNotCommitted(error.to_string())
                }
            };
        }
        if cut == TransactionCut::BeforeCommit {
            let _ = tx.rollback().await;
            return ClaimAuthoritativeTurnEstablishment::KnownNotCommitted(
                "injected claim before-commit cut".to_string(),
            );
        }
        let commit = tx.commit().await;
        let commit = if cut == TransactionCut::AfterCommit {
            Err(DbError::Serialization(
                "injected claim after-commit acknowledgement loss".to_string(),
            ))
        } else {
            commit
        };
        match commit {
            Ok(()) => ClaimAuthoritativeTurnEstablishment::Established(Box::new(result)),
            Err(error) => match self.probe_authoritative_turn_claim(input).await {
                Ok(Some(result)) => {
                    ClaimAuthoritativeTurnEstablishment::Established(Box::new(result))
                }
                Ok(None) => {
                    ClaimAuthoritativeTurnEstablishment::KnownNotCommitted(error.to_string())
                }
                Err(probe_error) => ClaimAuthoritativeTurnEstablishment::Unclassifiable(format!(
                    "direct-turn claim commit acknowledgement failed: {error}; exact claim probe failed: {probe_error}"
                )),
            },
        }
    }

    async fn probe_authoritative_turn_claim(
        &self,
        input: &ClaimAuthoritativeTurnInput,
    ) -> DbResult<Option<ClaimAuthoritativeTurnResult>> {
        let mut tx = self.pool.begin().await?;
        let canonical_turn =
            load_turn_for_workflow_tx(&self.pool, &mut tx, input.turn_id, input.workflow_id)
                .await?;
        let attempt =
            load_live_attempt_tx(&mut tx, input.workflow_id, DIRECT_TURN_EFFECT_ID).await?;
        tx.commit().await?;
        let Some(attempt) = attempt else {
            return Ok(None);
        };
        let exact = attempt.authority.process_incarnation == input.process_incarnation
            && attempt
                .lease
                .as_ref()
                .is_some_and(|lease| lease.lease_until == input.lease_until);
        if !exact {
            return Ok(None);
        }
        let Some(canonical_turn) = canonical_turn else {
            return Err(DbError::Serialization(
                "established direct-turn claim is missing its canonical turn".to_string(),
            ));
        };
        Ok(Some(ClaimAuthoritativeTurnResult {
            outcome: ClaimOutcome::Started,
            authority: Some(attempt.authority.clone()),
            attempt: Some(attempt),
            canonical_turn: Some(canonical_turn),
        }))
    }

    pub async fn claim_authoritative_turn(
        &self,
        input: &ClaimAuthoritativeTurnInput,
    ) -> DbResult<ClaimAuthoritativeTurnResult> {
        let mut tx = self.begin_tx().await?;
        let result = self.claim_authoritative_turn_in_tx(&mut tx, input).await?;
        if result.outcome == ClaimOutcome::Started {
            tx.commit().await?;
        } else {
            tx.rollback().await?;
        }
        Ok(result)
    }

    async fn claim_authoritative_turn_in_tx(
        &self,
        tx: &mut super::WorkflowTx<'_>,
        input: &ClaimAuthoritativeTurnInput,
    ) -> DbResult<ClaimAuthoritativeTurnResult> {
        let Some(canonical_turn) =
            load_turn_for_workflow_tx(&self.pool, &mut tx.tx, input.turn_id, input.workflow_id)
                .await?
        else {
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
            let attempt_id = next_attempt_id_tx(tx).await?;
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
                return Ok(ClaimAuthoritativeTurnResult {
                    outcome: ClaimOutcome::AuthorityConflict,
                    authority: None,
                    attempt: None,
                    canonical_turn: Some(canonical_turn),
                });
            }
        }
        let expired = expire_direct_turn_lease_in_tx(
            tx,
            &super::ExpireLeaseInput {
                workflow_id: input.workflow_id,
                effect_id: EffectId(DIRECT_TURN_EFFECT_ID),
                attempt_id: existing_live_attempt.id,
                now: input.now,
            },
        )
        .await?;
        if expired != AuthorityOutcome::Authorized {
            return Ok(ClaimAuthoritativeTurnResult {
                outcome: ClaimOutcome::Ineligible,
                authority: None,
                attempt: None,
                canonical_turn: Some(canonical_turn),
            });
        }
        let attempt_id = next_attempt_id_tx(tx).await?;
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

    pub async fn load_active_runtime_turn(
        &self,
        conversation: &ConversationAuthority,
    ) -> DbResult<Option<DurableTurn>> {
        let mut tx = self.pool.begin().await?;
        let turn = load_active_runtime_turn_tx(&self.pool, &mut tx, conversation).await?;
        tx.rollback().await?;
        Ok(turn)
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
    ) -> DbResult<DiscoverableAcceptedTurnPage> {
        if limit == 0 {
            return Ok(DiscoverableAcceptedTurnPage {
                candidates: Vec::new(),
                next_cursor: cursor,
            });
        }
        let capped_limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let (cursor_turn_id, cursor_workflow_id) = cursor.map_or((0_i64, 0_i64), |cursor| {
            (
                i64::try_from(cursor.turn_id.0).unwrap_or(i64::MAX),
                i64::try_from(cursor.workflow_id.0).unwrap_or(i64::MAX),
            )
        });
        let rows = sqlx::query(
            "SELECT durable_turns.turn_id, durable_turns.workflow_id,
                    durable_turns.conversation_id, durable_turns.prepared_fingerprint,
                    durable_turns.prepared_payload
             FROM durable_turns
             JOIN conversations ON conversations.id = durable_turns.conversation_id
             WHERE disposition = 'Runtime'
               AND conversations.archived = 0
               AND terminal_kind IS NULL
               AND canonical_message_id IS NULL
               AND workflow_id IS NOT NULL
               AND (durable_turns.turn_id > ?1 OR (durable_turns.turn_id = ?1 AND durable_turns.workflow_id > ?2))
             ORDER BY durable_turns.turn_id, durable_turns.workflow_id
             LIMIT ?3",
        )
        .bind(cursor_turn_id)
        .bind(cursor_workflow_id)
        .bind(capped_limit)
        .fetch_all(&self.pool)
        .await?;
        let next_cursor = rows
            .last()
            .map(|row| {
                Ok::<DirectTurnDiscoveryCursor, DbError>(DirectTurnDiscoveryCursor {
                    turn_id: TurnAuthorityId(to_u64(row.get("turn_id"), "turn_id")?),
                    workflow_id: WorkflowId(to_u64(row.get("workflow_id"), "workflow_id")?),
                })
            })
            .transpose()?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let turn_id = TurnAuthorityId(to_u64(row.get("turn_id"), "turn_id")?);
            let workflow_id = WorkflowId(to_u64(row.get("workflow_id"), "workflow_id")?);
            match load_prepared_turn_from_row(&self.pool, &row).await {
                Ok(prepared) => out.push(DiscoverableAcceptedTurn {
                    turn_id,
                    workflow_id,
                    conversation: ConversationAuthority(row.get("conversation_id")),
                    prepared,
                }),
                Err(error) => {
                    tracing::error!(turn_id = turn_id.0, workflow_id = workflow_id.0, error = %error, "quarantining corrupt direct-turn payload during discovery");
                    match self
                        .quarantine_corrupt_direct_turn(
                            turn_id,
                            format!("prepared payload cannot be decoded: {error}"),
                        )
                        .await
                    {
                        Ok(_) => {}
                        Err(quarantine_error) => tracing::error!(
                            turn_id = turn_id.0,
                            workflow_id = workflow_id.0,
                            error = %quarantine_error,
                            "failed to quarantine corrupt direct-turn payload"
                        ),
                    }
                }
            }
        }
        Ok(DiscoverableAcceptedTurnPage {
            candidates: out,
            next_cursor,
        })
    }

    pub async fn quarantine_corrupt_direct_turn(
        &self,
        turn_id: TurnAuthorityId,
        reason: String,
    ) -> DbResult<TurnStep> {
        let generation =
            sqlx::query_scalar::<_, i64>("SELECT generation FROM durable_turns WHERE turn_id = ?1")
                .bind(
                    i64::try_from(turn_id.0)
                        .map_err(|_| DbError::Serialization("turn id overflow".to_string()))?,
                )
                .fetch_one(&self.pool)
                .await?;
        self.terminate_authoritative_turn(TurnCommand::Fail {
            turn_id,
            expected_generation: u64::try_from(generation).map_err(|_| {
                DbError::Serialization("negative direct-turn generation".to_string())
            })?,
            reason,
        })
        .await
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
        let turn = row_to_turn_tx(&mut tx.tx, row).await?;
        let stored_prepared = load_prepared_payload_tx(&mut tx.tx, turn.id).await?;
        verify_prepared_payload(&turn, &stored_prepared, &input.prepared)?;
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
    ) -> MaterializeAuthoritativeTurnResult {
        self.materialize_authoritative_turn_at_cut(input, TransactionCut::None)
            .await
    }

    async fn materialize_authoritative_turn_at_cut(
        &self,
        input: &MaterializeAuthoritativeTurnInput,
        cut: TransactionCut,
    ) -> MaterializeAuthoritativeTurnResult {
        match self
            .materialize_authoritative_turn_command(input, cut)
            .await
        {
            Ok(outcome) => crate::workflow::LocalAuthorityResult::DurableFactEstablished(outcome),
            Err(DbError::DirectTurnConflict(conflict)) => {
                crate::workflow::LocalAuthorityResult::DurableFactEstablished(
                    MaterializeAuthoritativeTurnOutcome::CommandRejected(conflict),
                )
            }
            Err(command_error) => {
                tracing::error!(
                    turn_id = input.turn_id.0,
                    workflow_id = input.authority.workflow_id.0,
                    error = %command_error,
                    "direct-turn materialization returned no typed result; classifying once"
                );
                match self
                    .classify_authoritative_turn_materialization(input)
                    .await
                {
                    Ok(outcome) => {
                        crate::workflow::LocalAuthorityResult::DurableFactEstablished(outcome)
                    }
                    Err(classification_error) => {
                        tracing::error!(
                            turn_id = input.turn_id.0,
                            workflow_id = input.authority.workflow_id.0,
                            error = %classification_error,
                            "direct-turn materialization durable fact remains unclassified"
                        );
                        crate::workflow::LocalAuthorityResult::DurableFactUnclassified
                    }
                }
            }
        }
    }

    async fn classify_authoritative_turn_materialization(
        &self,
        input: &MaterializeAuthoritativeTurnInput,
    ) -> DbResult<MaterializeAuthoritativeTurnOutcome> {
        let mut tx = self.begin_tx().await?;
        let outcome = self
            .classify_authoritative_turn_materialization_tx(&mut tx, input)
            .await;
        tx.rollback().await?;
        outcome
    }

    async fn classify_authoritative_turn_materialization_tx(
        &self,
        tx: &mut super::WorkflowTx<'_>,
        input: &MaterializeAuthoritativeTurnInput,
    ) -> DbResult<MaterializeAuthoritativeTurnOutcome> {
        let row = sqlx::query(
            "SELECT dt.conversation_id, dt.prepared_fingerprint, dt.generation,
                    dt.terminal_kind, dt.canonical_message_id,
                    c.state AS conversation_state,
                    EXISTS (
                        SELECT 1
                        FROM workflow_effects e
                        JOIN workflow_attempts a
                          ON a.workflow_id = e.workflow_id AND a.attempt_id = ?4
                        LEFT JOIN workflow_reclaimable_leases l
                          ON l.workflow_id = a.workflow_id AND l.attempt_id = a.attempt_id
                        WHERE e.workflow_id = ?2 AND e.effect_id = ?3
                          AND e.declared_workflow_version = ?5 AND e.generation = ?6
                          AND e.status = 'Executing'
                          AND a.effect_id = ?3 AND a.declared_workflow_version = ?5
                          AND a.generation = ?6 AND a.process_incarnation = ?7
                          AND a.status IN ('Begun', 'ObservationRecorded')
                          AND l.lease_until > ?8
                    ) AS authority_live,
                    EXISTS (
                        SELECT 1 FROM workflow_receipts r
                        JOIN workflow_deliveries d
                          ON d.workflow_id = r.workflow_id AND d.effect_id = r.effect_id
                        JOIN workflow_transitions t
                          ON t.workflow_id = r.workflow_id
                         AND t.transition_id = d.accepted_by_transition_id
                        WHERE r.workflow_id = ?2 AND r.effect_id = ?3
                          AND r.attempt_id = ?4 AND r.declared_workflow_version = ?5
                          AND r.generation = ?6 AND r.process_incarnation = ?7
                          AND d.status = 'Accepted'
                          AND d.runtime_acceptance_status = 'Accepted'
                          AND t.transition_id = ?9
                    ) AS materialization_committed,
                    m.message_id, m.sequence_id, m.message_type, m.content,
                    m.display_data, m.usage_data, m.created_at
             FROM durable_turns dt
             JOIN conversations c ON c.id = dt.conversation_id
             LEFT JOIN messages m ON m.message_id = dt.canonical_message_id
             WHERE dt.turn_id = ?1 AND dt.workflow_id = ?2",
        )
        .bind(to_i64(input.turn_id.0, "turn_id")?)
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
        .bind(to_i64(input.now.0, "now")?)
        .bind(to_i64(
            DIRECT_TURN_MATERIALIZED_TRANSITION_ID,
            "direct_turn_materialized_transition_id",
        )?)
        .fetch_optional(&mut *tx.tx)
        .await?;
        let Some(row) = row else {
            return Ok(MaterializeAuthoritativeTurnOutcome::StaleAuthority);
        };
        let conversation = ConversationAuthority(row.get("conversation_id"));
        let expected_prepared = PreparedTurn::from_exact_payload(
            &conversation,
            input
                .prepared
                .to_exact_bytes()
                .map_err(|error| DbError::Serialization(error.to_string()))?,
        );
        if row.get::<String, _>("prepared_fingerprint") != expected_prepared.fingerprint() {
            return Ok(MaterializeAuthoritativeTurnOutcome::CommandRejected(
                TurnConflict::PreparedSemanticsChanged {
                    authoritative_fingerprint: row.get("prepared_fingerprint"),
                },
            ));
        }
        let generation = to_u64(row.get("generation"), "generation")?;
        let canonical_message_id = row.get::<Option<String>, _>("canonical_message_id");
        if let Some(message_id) = canonical_message_id {
            if generation != input.authority.generation.0
                || row.get::<Option<String>, _>("terminal_kind").is_some()
                || row.get::<i64, _>("materialization_committed") == 0
            {
                return Err(DbError::Serialization(
                    "canonical direct-turn message exists without exact committed authority facts"
                        .to_string(),
                ));
            }
            let persisted_state: ConvState =
                serde_json::from_str(&row.get::<String, _>("conversation_state"))
                    .map_err(|error| DbError::Serialization(error.to_string()))?;
            if persisted_state != input.accepted_state {
                return Err(DbError::Serialization(
                    "materialized direct-turn projection does not match proposed state".to_string(),
                ));
            }
            let mut message = crate::parse_message_row(row).map_err(DbError::Sqlx)?;
            if message.message_id != message_id {
                return Err(DbError::Serialization(
                    "materialized direct-turn message identity mismatch".to_string(),
                ));
            }
            let files = sqlx::query(
                "SELECT original_name, media_type, size_bytes, stored_path
                 FROM message_files WHERE message_id = ?1 ORDER BY ordinal",
            )
            .bind(&message_id)
            .map(
                |row: sqlx::sqlite::SqliteRow| phoenix_core::domain::db_schema::FileAttachment {
                    original_name: row.get("original_name"),
                    media_type: row.get("media_type"),
                    size_bytes: u64::try_from(row.get::<i64, _>("size_bytes")).unwrap_or(0),
                    stored_path: row.get("stored_path"),
                },
            )
            .fetch_all(&mut *tx.tx)
            .await?;
            let images = sqlx::query(
                "SELECT media_type, data FROM message_images WHERE message_id = ?1 ORDER BY ordinal",
            )
            .bind(&message_id)
            .map(|row: sqlx::sqlite::SqliteRow| phoenix_core::domain::db_schema::ImageData {
                data: row.get("data"),
                media_type: row.get("media_type"),
            })
            .fetch_all(&mut *tx.tx)
            .await?;
            message.content.set_attachments(images, files);
            let expected_content = input.prepared.message_content_and_display_data();
            if message.conversation_id != conversation.0
                || message.content != expected_content.0
                || message.display_data != expected_content.1
            {
                return Err(DbError::Serialization(
                    "materialized direct-turn canonical message payload mismatch".to_string(),
                ));
            }
            return Ok(MaterializeAuthoritativeTurnOutcome::ClassifiedCommitted(
                AuthoritativeTurnMaterialization {
                    message: Box::new(message),
                    turn_id: input.turn_id,
                    generation,
                },
            ));
        }
        let persisted_state: ConvState =
            serde_json::from_str(&row.get::<String, _>("conversation_state"))
                .map_err(|error| DbError::Serialization(error.to_string()))?;
        if persisted_state == input.accepted_state {
            return Err(DbError::Serialization(
                "proposed direct-turn state exists without canonical materialization".to_string(),
            ));
        }
        if generation == input.authority.generation.0
            && row.get::<Option<String>, _>("terminal_kind").is_none()
            && row.get::<i64, _>("authority_live") != 0
        {
            Ok(MaterializeAuthoritativeTurnOutcome::NotCommitted)
        } else {
            Ok(MaterializeAuthoritativeTurnOutcome::StaleAuthority)
        }
    }

    async fn materialize_authoritative_turn_command(
        &self,
        input: &MaterializeAuthoritativeTurnInput,
        cut: TransactionCut,
    ) -> DbResult<MaterializeAuthoritativeTurnOutcome> {
        let mut tx = self.begin_immediate_tx().await?;
        let turn_id = input.turn_id;
        let turn =
            load_turn_for_workflow_tx(&self.pool, &mut tx.tx, turn_id, input.authority.workflow_id)
                .await?
                .ok_or_else(|| conflict(TurnConflict::UnknownTurn))?;
        let stored_prepared = load_prepared_payload_tx(&mut tx.tx, turn.id).await?;
        verify_prepared_payload(&turn, &stored_prepared, &input.prepared)?;
        let canonical_message_id = canonical_message_id_for_turn(&turn, &input.prepared);
        let mut model =
            phoenix_workflow::DurableTurnModel::from_turns([turn.clone()]).map_err(conflict)?;
        let step = model
            .apply(TurnCommand::Materialize {
                turn_id,
                expected_generation: input.authority.generation.0,
                message_id: canonical_message_id.clone(),
            })
            .map_err(conflict)?;
        if matches!(step.outcome, TurnOutcome::MaterializationReplay { .. }) {
            let existing = load_message_by_id_tx(&mut tx.tx, &canonical_message_id.0).await?;
            verify_existing_materialized_message_without_sequence(
                &existing,
                &turn,
                &input.prepared,
            )?;
            tx.rollback().await?;
            return Ok(MaterializeAuthoritativeTurnOutcome::ExactReplay(
                AuthoritativeTurnMaterialization {
                    message: Box::new(existing),
                    turn_id: turn.id,
                    generation: turn.generation,
                },
            ));
        }
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
                receipt_payload: serde_json::to_vec(&direct_turn_profile::DirectTurnReceipt {
                    turn_id: turn_id.0,
                    canonical_message_id: canonical_message_id.0.clone(),
                })
                .map_err(|e| DbError::Serialization(format!("encode direct-turn receipt: {e}")))?,
                receipt_event_codec: local_codec_owned(&direct_turn_profile::receipt_event_codec()),
                receipt_event_payload: serde_json::to_vec(
                    &direct_turn_profile::DirectTurnReceiptEvent::Materialized {
                        canonical_message_id: canonical_message_id.0.clone(),
                    },
                )
                .map_err(|e| {
                    DbError::Serialization(format!("encode direct-turn receipt event: {e}"))
                })?,
                receipt_event_requires_runtime_acceptance: true,
                request_runtime_acceptance_for_cancellation: false,
            })
            .await?;
        if acceptance.outcome != AuthorityOutcome::Authorized {
            tx.rollback().await?;
            return Ok(MaterializeAuthoritativeTurnOutcome::StaleAuthority);
        }
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
        update_conversation_state_for_adoption_tx(&mut tx, &turn.conversation, input).await?;
        if matches!(step.outcome, TurnOutcome::Materialized { .. }) {
            let snapshot = direct_turn_profile::DirectTurnSnapshot { turn_id: turn_id.0 };
            let delivered = direct_turn_profile::DirectTurnReceiptEvent::Materialized {
                canonical_message_id: canonical_message_id.0.clone(),
            };
            let event = direct_turn_profile::DirectTurnEvent::Delivered(delivered);
            let event_codec = local_codec_owned(&direct_turn_profile::event_codec());
            let snapshot_codec = local_codec_owned(&direct_turn_profile::snapshot_codec());
            let event_payload = serde_json::to_vec(&event).map_err(|error| {
                DbError::Serialization(format!("encode direct-turn delivered event: {error}"))
            })?;
            let snapshot_payload = serde_json::to_vec(&snapshot).map_err(|error| {
                DbError::Serialization(format!("encode direct-turn snapshot: {error}"))
            })?;
            let outcome = tx
                .resolve_deliveries_exact(DeliveryResolutionPlan {
                    workflow_id: input.authority.workflow_id,
                    expected_version: input.authority.declared_workflow_version,
                    transition_id: phoenix_workflow::TransitionId(
                        DIRECT_TURN_MATERIALIZED_TRANSITION_ID,
                    ),
                    generation: input.authority.generation,
                    next_status: WorkflowStatus::Active,
                    event_codec: &event_codec,
                    event_payload: &event_payload,
                    next_snapshot_codec: &snapshot_codec,
                    next_snapshot_payload: &snapshot_payload,
                    committed_at: input.now,
                    exact_delivery_ids: &[delivery_id],
                    decision: DeliveryResolutionDecision::Accept,
                })
                .await?;
            if outcome != phoenix_workflow::CommitOutcome::Committed {
                tx.rollback().await?;
                return Err(DbError::Serialization(format!(
                    "direct-turn runtime acceptance was rejected: {outcome:?}"
                )));
            }
        }
        let canonical_turn =
            load_turn_for_workflow_tx(&self.pool, &mut tx.tx, turn_id, input.authority.workflow_id)
                .await?
                .ok_or_else(|| {
                    DbError::Serialization("direct-turn missing after materialization".to_string())
                })?;
        let outcome =
            MaterializeAuthoritativeTurnOutcome::Materialized(AuthoritativeTurnMaterialization {
                message: Box::new(message),
                turn_id: canonical_turn.id,
                generation: canonical_turn.generation,
            });
        finish_workflow_transaction_at_cut(tx, cut).await?;
        Ok(outcome)
    }

    pub async fn terminate_authoritative_turn(&self, command: TurnCommand) -> DbResult<TurnStep> {
        self.terminalize_authoritative_turn(&TerminalizeAuthoritativeTurnInput {
            command,
            projection: None,
        })
        .await
    }

    pub async fn terminalize_authoritative_turn(
        &self,
        input: &TerminalizeAuthoritativeTurnInput,
    ) -> DbResult<TurnStep> {
        self.terminalize_authoritative_turn_at_cut(input, TransactionCut::None)
            .await
    }

    pub async fn settle_failed_continuation_start_atomically(
        &self,
        input: &AtomicContinuationSettlementInput,
    ) -> DbResult<crate::ContinuationCommitOutcome> {
        let telemetry = SqliteTelemetry::new(SqliteOperation::DirectTurnTerminalSettlement);
        let (mut connection, pool_timing) = telemetry
            .observe_pool_acquisition_sqlx(self.pool.acquire())
            .await?;
        let mut tx = telemetry
            .observe_db(SqlitePhase::TransactionAcquisition, async {
                Ok(super::WorkflowTx::new(connection.begin().await?))
            })
            .await?;
        let transaction_timing = pool_timing.transaction_started();
        let outcome = telemetry
            .observe_db(SqlitePhase::Statement, async {
                let outcome = crate::persist_continuation_start_tx(
                    &mut tx.tx,
                    &input.conversation_id,
                    &input.operation_id,
                    &input.message,
                    &input.completed_state,
                    input.state_updated_at,
                )
                .await?;
                if outcome == crate::ContinuationCommitOutcome::Applied {
                    self.terminalize_authoritative_turn_in_tx(
                        &mut tx,
                        &TerminalizeAuthoritativeTurnInput {
                            command: input.command.clone(),
                            projection: Some(PersistedConversationProjection {
                                state: input.completed_state.clone(),
                                state_updated_at: input.state_updated_at,
                            }),
                        },
                    )
                    .await?;
                }
                Ok(outcome)
            })
            .await?;
        match outcome {
            crate::ContinuationCommitOutcome::Applied => {
                telemetry
                    .observe_commit_db(transaction_timing, tx.commit())
                    .await?;
            }
            crate::ContinuationCommitOutcome::Duplicate
            | crate::ContinuationCommitOutcome::Stale => {
                telemetry
                    .observe_rollback_db(transaction_timing, tx.rollback())
                    .await?;
            }
        }
        Ok(outcome)
    }

    pub async fn reconcile_legacy_continuation_atomically(
        &self,
        conversation_id: &str,
        state_updated_at: DateTime<Utc>,
    ) -> DbResult<Option<String>> {
        let telemetry = SqliteTelemetry::new(SqliteOperation::DirectTurnTerminalSettlement);
        let (mut connection, pool_timing) = telemetry
            .observe_pool_acquisition_sqlx(self.pool.acquire())
            .await?;
        let mut tx = telemetry
            .observe_db(SqlitePhase::TransactionAcquisition, async {
                Ok(super::WorkflowTx::new(connection.begin().await?))
            })
            .await?;
        let transaction_timing = pool_timing.transaction_started();
        let summary = telemetry
            .observe_db(SqlitePhase::Statement, async {
                let summary = crate::reconcile_legacy_half_committed_continuation_tx(
                    &mut tx.tx,
                    conversation_id,
                    state_updated_at,
                )
                .await?;
                let Some(summary) = summary else {
                    return Ok(None);
                };

                let conversation = ConversationAuthority(conversation_id.to_string());
                if let Some(turn) =
                    load_active_runtime_turn_tx(&self.pool, &mut tx.tx, &conversation).await?
                {
                    self.terminalize_authoritative_turn_in_tx(
                        &mut tx,
                        &TerminalizeAuthoritativeTurnInput {
                            command: TurnCommand::Fail {
                                turn_id: turn.id,
                                expected_generation: turn.generation,
                                reason: "legacy continuation operation interrupted after summary persistence"
                                    .to_string(),
                            },
                            projection: None,
                        },
                    )
                    .await?;
                }
                Ok(Some(summary))
            })
            .await?;
        let Some(summary) = summary else {
            telemetry
                .observe_rollback_db(transaction_timing, tx.rollback())
                .await?;
            return Ok(None);
        };
        telemetry
            .observe_commit_db(transaction_timing, tx.commit())
            .await?;
        Ok(Some(summary))
    }

    pub async fn settle_continuation_direct_turn_atomically(
        &self,
        input: &AtomicContinuationSettlementInput,
    ) -> DbResult<crate::ContinuationCommitOutcome> {
        let telemetry = SqliteTelemetry::new(SqliteOperation::DirectTurnTerminalSettlement);
        let (mut connection, pool_timing) = telemetry
            .observe_pool_acquisition_sqlx(self.pool.acquire())
            .await?;
        let mut tx = telemetry
            .observe_db(SqlitePhase::TransactionAcquisition, async {
                Ok(super::WorkflowTx::new(connection.begin().await?))
            })
            .await?;
        let transaction_timing = pool_timing.transaction_started();
        let outcome = telemetry
            .observe_db(SqlitePhase::Statement, async {
                let outcome = crate::commit_continuation_tx(
                    &mut tx.tx,
                    &input.conversation_id,
                    &input.operation_id,
                    &input.message,
                    &input.completed_state,
                    input.state_updated_at,
                )
                .await?;
                let projection = match outcome {
                    crate::ContinuationCommitOutcome::Applied => {
                        Some(PersistedConversationProjection {
                            state: input.completed_state.clone(),
                            state_updated_at: input.state_updated_at,
                        })
                    }
                    crate::ContinuationCommitOutcome::Duplicate => None,
                    crate::ContinuationCommitOutcome::Stale => return Ok(outcome),
                };
                self.terminalize_authoritative_turn_in_tx(
                    &mut tx,
                    &TerminalizeAuthoritativeTurnInput {
                        command: input.command.clone(),
                        projection,
                    },
                )
                .await?;
                Ok(outcome)
            })
            .await?;
        if outcome == crate::ContinuationCommitOutcome::Stale {
            telemetry
                .observe_rollback_db(transaction_timing, tx.rollback())
                .await?;
        } else {
            telemetry
                .observe_commit_db(transaction_timing, tx.commit())
                .await?;
        }
        Ok(outcome)
    }

    #[cfg(test)]
    async fn terminate_authoritative_turn_at_cut(
        &self,
        command: TurnCommand,
        cut: TransactionCut,
    ) -> DbResult<TurnStep> {
        self.terminalize_authoritative_turn_at_cut(
            &TerminalizeAuthoritativeTurnInput {
                command,
                projection: None,
            },
            cut,
        )
        .await
    }

    pub async fn list_discoverable_terminal_obligations(
        &self,
        limit: usize,
    ) -> DbResult<Vec<DiscoverableTerminalObligation>> {
        let limit = i64::try_from(limit)
            .map_err(|_| DbError::Serialization("terminal discovery limit overflow".to_string()))?;
        let rows: Vec<(i64, String)> = sqlx::query_as(
            "SELECT o.turn_id, t.conversation_id
             FROM direct_turn_terminal_obligations AS o
             JOIN durable_turns AS t ON t.turn_id = o.turn_id
             WHERE t.disposition = 'Runtime'
               AND t.terminal_kind IS NULL
               AND t.owns_conversation = 1
               AND t.generation = o.expected_generation
             ORDER BY o.turn_id ASC
             LIMIT ?1",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|(turn_id, conversation_id)| {
                Ok(DiscoverableTerminalObligation {
                    turn_id: TurnAuthorityId(to_u64(turn_id, "turn_id")?),
                    conversation: ConversationAuthority(conversation_id),
                })
            })
            .collect()
    }

    pub async fn persist_terminal_obligation(
        &self,
        input: &DirectTurnTerminalObligationInput,
    ) -> DbResult<()> {
        let mut tx = self.pool.begin().await?;
        persist_terminal_obligation_tx(&mut tx, input).await?;
        tx.commit().await?;
        Ok(())
    }

    pub(crate) async fn persist_terminal_obligation_tx(
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        input: &DirectTurnTerminalObligationInput,
    ) -> DbResult<()> {
        persist_terminal_obligation_tx(tx, input).await
    }

    pub async fn load_active_terminal_obligation(
        &self,
        conversation: &ConversationAuthority,
    ) -> DbResult<Option<DirectTurnTerminalObligation>> {
        let row = sqlx::query(
            "SELECT o.turn_id, o.expected_generation, o.terminal_kind, o.terminal_reason,
                    o.target_state, o.target_state_updated_at_us, o.response_message_id
             FROM direct_turn_terminal_obligations AS o
             JOIN durable_turns AS t ON t.turn_id = o.turn_id
             WHERE t.conversation_id = ?1 AND t.disposition = 'Runtime'
               AND t.terminal_kind IS NULL AND t.owns_conversation = 1
               AND t.generation = o.expected_generation",
        )
        .bind(&conversation.0)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| parse_terminal_obligation_row(&row))
            .transpose()
    }

    pub async fn probe_terminal_evidence(
        &self,
        conversation_id: &str,
        message_id: &str,
        expected: &DirectTurnTerminalObligationInput,
    ) -> DbResult<TerminalEvidenceProbe> {
        self.probe_terminal_evidence_expectation(
            &TerminalEvidenceExpectation::Messages(vec![Message {
                message_id: message_id.to_string(),
                conversation_id: conversation_id.to_string(),
                sequence_id: 0,
                message_type: phoenix_core::domain::db_schema::MessageType::System,
                content: MessageContent::system("probe-by-identity"),
                display_data: None,
                usage_data: None,
                created_at: DateTime::<Utc>::UNIX_EPOCH,
            }]),
            expected,
            true,
        )
        .await
    }

    pub async fn probe_exact_terminal_evidence(
        &self,
        evidence: &TerminalEvidenceExpectation,
        expected: &DirectTurnTerminalObligationInput,
    ) -> DbResult<TerminalEvidenceProbe> {
        self.probe_terminal_evidence_expectation(evidence, expected, false)
            .await
    }

    async fn probe_terminal_evidence_expectation(
        &self,
        evidence: &TerminalEvidenceExpectation,
        expected: &DirectTurnTerminalObligationInput,
        identity_only: bool,
    ) -> DbResult<TerminalEvidenceProbe> {
        let mut tx = self.pool.begin().await?;
        let evidence_matches = match evidence {
            TerminalEvidenceExpectation::ObligationOnly { .. } => true,
            TerminalEvidenceExpectation::Messages(messages) => {
                let mut matches = true;
                for expected_message in messages {
                    let actual =
                        load_optional_message_by_id_tx(&mut tx, &expected_message.message_id)
                            .await?;
                    matches &= actual.as_ref().is_some_and(|actual| {
                        actual.conversation_id == expected_message.conversation_id
                            && (identity_only
                                || (actual.sequence_id == expected_message.sequence_id
                                    && actual.message_type == expected_message.message_type
                                    && actual.content == expected_message.content
                                    && actual.display_data == expected_message.display_data
                                    && actual.usage_data == expected_message.usage_data
                                    && actual.created_at == expected_message.created_at))
                    });
                }
                matches
            }
            TerminalEvidenceExpectation::MessageMutation {
                conversation_id,
                message_id,
                content,
                display_data,
            } => load_optional_message_by_id_tx(&mut tx, message_id)
                .await?
                .is_some_and(|message| {
                    message.conversation_id == *conversation_id
                        && message.content == *content
                        && message.display_data.as_ref() == Some(display_data)
                }),
        };
        let conversation_exists: i64 =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM conversations WHERE id = ?1)")
                .bind(expected_conversation_id(evidence))
                .fetch_one(&mut *tx)
                .await?;
        let row = sqlx::query(
            "SELECT turn_id, expected_generation, terminal_kind, terminal_reason,
                    target_state, target_state_updated_at_us, response_message_id
             FROM direct_turn_terminal_obligations WHERE turn_id = ?1",
        )
        .bind(to_i64(expected.turn_id.0, "turn_id")?)
        .fetch_optional(&mut *tx)
        .await?;
        let obligation = row
            .map(|row| parse_terminal_obligation_row(&row))
            .transpose()?;
        let settled_row = sqlx::query(
            "SELECT t.generation, t.terminal_kind, t.terminal_reason, t.owns_conversation,
                    c.state, c.state_updated_at, c.transcript_generation
             FROM durable_turns AS t
             JOIN conversations AS c ON c.id = t.conversation_id
             WHERE t.turn_id = ?1",
        )
        .bind(to_i64(expected.turn_id.0, "turn_id")?)
        .fetch_optional(&mut *tx)
        .await?;
        let exact_retirement_exists: i64 = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM direct_turn_retirements
                WHERE turn_id = ?1 AND conversation_id = ?2
             )",
        )
        .bind(to_i64(expected.turn_id.0, "turn_id")?)
        .bind(expected_conversation_id(evidence))
        .fetch_one(&mut *tx)
        .await?;
        if conversation_exists == 0
            && settled_row.is_none()
            && obligation.is_none()
            && exact_retirement_exists != 0
        {
            tx.commit().await?;
            return Ok(TerminalEvidenceProbe::Retired);
        }
        let settled_row = settled_row.ok_or_else(|| {
            DbError::Serialization("terminal evidence lost its authoritative turn".to_string())
        })?;
        tx.commit().await?;
        let obligation_matches = obligation.as_ref().is_some_and(|obligation| {
            obligation.turn_id == expected.turn_id
                && obligation.expected_generation == expected.expected_generation
                && obligation.terminal == expected.terminal
                && projections_match(&obligation.projection, &expected.projection)
                && obligation.response_message_id == expected.response_message_id
        });
        let settled_terminal_kind: Option<String> = settled_row.try_get("terminal_kind")?;
        let settled_terminal = terminal_from_sql(
            settled_terminal_kind.as_deref(),
            settled_row.try_get("terminal_reason")?,
        )?;
        let settled_state_json: String = settled_row.try_get("state")?;
        let settled_state = serde_json::from_str(&settled_state_json).map_err(|error| {
            DbError::Serialization(format!(
                "decode settled direct-turn conversation state: {error}"
            ))
        })?;
        let settled_state_updated_at =
            DateTime::parse_from_rfc3339(&settled_row.try_get::<String, _>("state_updated_at")?)
                .map_err(|error| {
                    DbError::Serialization(format!(
                        "decode settled direct-turn conversation timestamp: {error}"
                    ))
                })?
                .with_timezone(&Utc);
        let settled_matches = to_u64(settled_row.try_get("generation")?, "generation")?
            == expected.expected_generation.saturating_add(1)
            && settled_terminal.as_ref() == Some(&expected.terminal)
            && settled_row.try_get::<i64, _>("owns_conversation")? == 0
            && projections_match(
                &PersistedConversationProjection {
                    state: settled_state,
                    state_updated_at: settled_state_updated_at,
                },
                &expected.projection,
            );
        Ok(
            match (
                evidence_matches,
                obligation_matches || settled_matches,
                obligation.is_some(),
                settled_matches,
            ) {
                (true, true, _, _) => TerminalEvidenceProbe::Established {
                    transcript_generation: evidence
                        .is_message_mutation()
                        .then(|| settled_row.try_get("transcript_generation"))
                        .transpose()?,
                },
                (false, false, false, false) => TerminalEvidenceProbe::KnownNotCommitted,
                (true, false, false, false)
                    if matches!(evidence, TerminalEvidenceExpectation::ObligationOnly { .. }) =>
                {
                    TerminalEvidenceProbe::KnownNotCommitted
                }
                _ => TerminalEvidenceProbe::Incomplete,
            },
        )
    }

    pub async fn probe_terminal_projection(
        &self,
        expected: &DirectTurnTerminalObligation,
    ) -> DbResult<TerminalProjectionProbe> {
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query(
            "SELECT t.generation, t.disposition, t.terminal_kind, t.terminal_reason,
                    t.owns_conversation, t.conversation_id,
                    c.state, c.state_updated_at,
                    EXISTS(
                        SELECT 1 FROM durable_turns AS current
                        WHERE current.conversation_id = t.conversation_id
                          AND current.owns_conversation = 1
                          AND current.terminal_kind IS NULL
                    ) AS has_current_owner
             FROM durable_turns AS t
             JOIN conversations AS c ON c.id = t.conversation_id
             WHERE t.turn_id = ?1",
        )
        .bind(to_i64(expected.turn_id.0, "turn_id")?)
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        let Some(row) = row else {
            return Ok(TerminalProjectionProbe::Missing);
        };
        let generation = to_u64(row.try_get("generation")?, "generation")?;
        let disposition: String = row.try_get("disposition")?;
        let owns_conversation: i64 = row.try_get("owns_conversation")?;
        let terminal = terminal_from_sql(
            row.try_get::<Option<String>, _>("terminal_kind")?
                .as_deref(),
            row.try_get("terminal_reason")?,
        )?;
        if generation == expected.expected_generation
            && disposition == "Runtime"
            && owns_conversation == 1
            && terminal.is_none()
        {
            return Ok(TerminalProjectionProbe::StillOwed);
        }
        if generation != expected.expected_generation.saturating_add(1)
            || owns_conversation != 0
            || terminal.as_ref() != Some(&expected.terminal)
        {
            return Ok(TerminalProjectionProbe::Unclassifiable);
        }
        let state_json: String = row.try_get("state")?;
        let state: ConvState = serde_json::from_str(&state_json).map_err(|error| {
            DbError::Serialization(format!("decode terminal projection state: {error}"))
        })?;
        let state_updated_at =
            DateTime::parse_from_rfc3339(&row.try_get::<String, _>("state_updated_at")?)
                .map_err(|error| {
                    DbError::Serialization(format!("decode terminal projection timestamp: {error}"))
                })?
                .with_timezone(&Utc);
        let current = row.try_get::<i64, _>("has_current_owner")? == 0
            && projections_match(
                &PersistedConversationProjection {
                    state,
                    state_updated_at,
                },
                &expected.projection,
            );
        Ok(if current {
            TerminalProjectionProbe::Current
        } else {
            TerminalProjectionProbe::Superseded
        })
    }

    async fn terminalize_authoritative_turn_at_cut(
        &self,
        input: &TerminalizeAuthoritativeTurnInput,
        cut: TransactionCut,
    ) -> DbResult<TurnStep> {
        let telemetry = SqliteTelemetry::new(SqliteOperation::DirectTurnTerminalSettlement);
        let (mut connection, pool_timing) = telemetry
            .observe_pool_acquisition_sqlx(self.pool.acquire())
            .await?;
        let mut tx = telemetry
            .observe_db(SqlitePhase::TransactionAcquisition, async {
                Ok(super::WorkflowTx::new(
                    connection.begin_with("BEGIN IMMEDIATE").await?,
                ))
            })
            .await?;
        let transaction_timing = pool_timing.transaction_started();
        let step = telemetry
            .observe_db(
                SqlitePhase::Statement,
                self.terminalize_authoritative_turn_in_tx(&mut tx, input),
            )
            .await?;
        if cut == TransactionCut::BeforeCommit {
            telemetry
                .observe_rollback_db(transaction_timing, tx.rollback())
                .await?;
            return Err(injected_cut(cut));
        }
        telemetry
            .observe_commit_db(transaction_timing, tx.commit())
            .await?;
        if cut == TransactionCut::AfterCommit {
            return Err(injected_cut(cut));
        }
        Ok(step)
    }

    async fn terminalize_authoritative_turn_in_tx(
        &self,
        tx: &mut super::WorkflowTx<'_>,
        input: &TerminalizeAuthoritativeTurnInput,
    ) -> DbResult<TurnStep> {
        let command = input.command.clone();
        let (turn_id, expected_generation, terminal) = terminal_command_parts(&command)?;
        let row = sqlx::query("SELECT * FROM durable_turns WHERE turn_id = ?1")
            .bind(to_i64(turn_id.0, "turn_id")?)
            .fetch_optional(&mut *tx.tx)
            .await?
            .ok_or_else(|| conflict(TurnConflict::UnknownTurn))?;
        let turn = row_to_turn_tx(&mut tx.tx, row).await?;
        let workflow_id = workflow_id_for_turn_tx(&mut tx.tx, turn_id).await?;
        let head = tx
            .fetch_workflow_head(workflow_id)
            .await?
            .ok_or_else(|| DbError::Serialization("direct-turn workflow missing".to_string()))?;
        let mut model =
            phoenix_workflow::DurableTurnModel::from_turns([turn.clone()]).map_err(conflict)?;
        let step = model.apply(command).map_err(conflict)?;
        if matches!(step.outcome, TurnOutcome::TerminalReplay { .. }) {
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
                terminal_commit_timestamp(),
            )
            .await?;
        if !committed {
            return Err(conflict(TurnConflict::StaleGeneration {
                actual: turn.generation,
            }));
        }
        if input.projection.is_none() {
            let obligation_exists: i64 = sqlx::query_scalar(
                "SELECT EXISTS(
                    SELECT 1 FROM direct_turn_terminal_obligations WHERE turn_id = ?1
                 )",
            )
            .bind(to_i64(turn_id.0, "turn_id")?)
            .fetch_one(&mut *tx.tx)
            .await?;
            if obligation_exists != 0 {
                return Err(DbError::Serialization(
                    "projection-less terminalization cannot consume an established obligation"
                        .to_string(),
                ));
            }
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
            return Err(conflict(TurnConflict::StaleGeneration {
                actual: turn.generation,
            }));
        }
        if let Some(projection) = &input.projection {
            update_conversation_projection_tx(tx, &turn.conversation, projection).await?;
        }
        sqlx::query("DELETE FROM direct_turn_terminal_obligations WHERE turn_id = ?1")
            .bind(to_i64(turn_id.0, "turn_id")?)
            .execute(&mut *tx.tx)
            .await?;
        if let Some(parent_id) = sqlx::query_scalar::<_, String>(
            "SELECT parent_conversation_id FROM conversations
             WHERE id = ?1 AND parent_conversation_id IS NOT NULL",
        )
        .bind(&turn.conversation.0)
        .fetch_optional(&mut *tx.tx)
        .await?
        {
            sqlx::query(
                "INSERT OR REPLACE INTO startup_parent_actions
                     (conversation_id, action, transcript_generation,
                      turn_id, turn_generation, created_at)
                 SELECT c.id, 'Reconcile', c.transcript_generation,
                        CASE WHEN a.conversation_id IS NULL THEN t.turn_id ELSE a.turn_id END,
                        CASE WHEN a.conversation_id IS NULL THEN t.generation ELSE a.turn_generation END,
                        ?2
                 FROM conversations AS c
                 LEFT JOIN startup_parent_actions AS a ON a.conversation_id = c.id
                 LEFT JOIN durable_turns AS t ON t.conversation_id = c.id
                     AND t.owns_conversation = 1 AND t.terminal_kind IS NULL
                 WHERE c.id = ?1",
            )
            .bind(parent_id)
            .bind(Utc::now().to_rfc3339())
            .execute(&mut *tx.tx)
            .await?;
        }
        mark_active_attempts_authority_lost_tx(tx, workflow_id).await?;
        delete_reclaimable_leases_tx(tx, workflow_id).await?;
        tx.invalidate_nonterminal_effects(workflow_id).await?;
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

async fn next_direct_turn_id_tx(tx: &mut super::WorkflowTx<'_>) -> DbResult<TurnAuthorityId> {
    Ok(TurnAuthorityId(
        super::next_global_sequence_value_tx(tx, "direct_turn", "turn_id").await?,
    ))
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
        conversation_id: input.conversation().0.clone(),
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
            input.conversation().0,
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

fn canonical_message_id_for_turn(
    turn: &DurableTurn,
    prepared: &PreparedDirectTurnPayload,
) -> CanonicalMessageId {
    CanonicalMessageId(format!("{}:{}", turn.conversation.0, prepared.message_id()))
}

async fn persist_terminal_obligation_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    input: &DirectTurnTerminalObligationInput,
) -> DbResult<()> {
    if input.projection.state_updated_at.timestamp_micros() < 0 {
        return Err(DbError::Serialization(
            "direct-turn terminal target timestamp must be nonnegative".to_string(),
        ));
    }
    let (terminal_kind, terminal_reason) = terminal_sql(&input.terminal);
    let target_state = serde_json::to_string(&input.projection.state).map_err(|error| {
        DbError::Serialization(format!("encode direct-turn terminal target state: {error}"))
    })?;
    let updated = sqlx::query(
        "INSERT INTO direct_turn_terminal_obligations
         (turn_id, expected_generation, terminal_kind, terminal_reason, target_state,
          target_state_updated_at_us, response_message_id)
         SELECT turn_id, generation, ?3, ?4, ?5, ?6, ?7
         FROM durable_turns
         WHERE turn_id = ?1 AND generation = ?2 AND terminal_kind IS NULL
           AND owns_conversation = 1
         ON CONFLICT(turn_id) DO NOTHING",
    )
    .bind(to_i64(input.turn_id.0, "turn_id")?)
    .bind(to_i64(input.expected_generation, "generation")?)
    .bind(terminal_kind)
    .bind(terminal_reason)
    .bind(target_state)
    .bind(input.projection.state_updated_at.timestamp_micros())
    .bind(&input.response_message_id)
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        let row = sqlx::query(
            "SELECT turn_id, expected_generation, terminal_kind, terminal_reason,
                    target_state, target_state_updated_at_us, response_message_id
             FROM direct_turn_terminal_obligations WHERE turn_id = ?1",
        )
        .bind(to_i64(input.turn_id.0, "turn_id")?)
        .fetch_optional(&mut **tx)
        .await?;
        let Some(existing) = row else {
            return Err(conflict(TurnConflict::StaleGeneration {
                actual: input.expected_generation,
            }));
        };
        let existing = parse_terminal_obligation_row(&existing)?;
        if existing.expected_generation != input.expected_generation
            || existing.terminal != input.terminal
            || !projections_match(&existing.projection, &input.projection)
            || existing.response_message_id != input.response_message_id
        {
            return Err(DbError::Serialization(
                "direct-turn terminal obligation conflicts with first durable payload".to_string(),
            ));
        }
    }
    Ok(())
}

fn expected_conversation_id(evidence: &TerminalEvidenceExpectation) -> &str {
    match evidence {
        TerminalEvidenceExpectation::Messages(messages) => messages
            .first()
            .map_or("", |message| message.conversation_id.as_str()),
        TerminalEvidenceExpectation::ObligationOnly { conversation_id }
        | TerminalEvidenceExpectation::MessageMutation {
            conversation_id, ..
        } => conversation_id,
    }
}

fn projections_match(
    actual: &PersistedConversationProjection,
    expected: &PersistedConversationProjection,
) -> bool {
    actual.state == expected.state
        && actual.state_updated_at.timestamp_micros()
            == expected.state_updated_at.timestamp_micros()
}

fn terminal_from_sql(
    terminal_kind: Option<&str>,
    terminal_reason: Option<String>,
) -> DbResult<Option<TurnTerminal>> {
    match terminal_kind {
        None => Ok(None),
        Some("Completed") => Ok(Some(TurnTerminal::Completed)),
        Some("Cancelled") => Ok(Some(TurnTerminal::Cancelled)),
        Some("Failed") => Ok(Some(TurnTerminal::Failed {
            reason: terminal_reason.ok_or_else(|| {
                DbError::Serialization("failed terminal turn missing reason".to_string())
            })?,
        })),
        Some(other) => Err(DbError::Serialization(format!(
            "unknown direct-turn terminal kind: {other}"
        ))),
    }
}

fn terminal_command_parts(command: &TurnCommand) -> DbResult<(TurnAuthorityId, u64, TurnTerminal)> {
    match command {
        TurnCommand::Complete {
            turn_id,
            expected_generation,
        } => Ok((*turn_id, *expected_generation, TurnTerminal::Completed)),
        TurnCommand::Cancel {
            turn_id,
            expected_generation,
        } => Ok((*turn_id, *expected_generation, TurnTerminal::Cancelled)),
        TurnCommand::Fail {
            turn_id,
            expected_generation,
            reason,
        } => Ok((
            *turn_id,
            *expected_generation,
            TurnTerminal::Failed {
                reason: reason.clone(),
            },
        )),
        TurnCommand::Accept { .. } | TurnCommand::Materialize { .. } => Err(
            DbError::Serialization("terminal repository command required".to_string()),
        ),
    }
}

fn parse_terminal_obligation_row(
    row: &sqlx::sqlite::SqliteRow,
) -> DbResult<DirectTurnTerminalObligation> {
    let terminal_kind: String = row.try_get("terminal_kind")?;
    let terminal = terminal_from_sql(Some(&terminal_kind), row.try_get("terminal_reason")?)?
        .ok_or_else(|| {
            DbError::Serialization("terminal obligation is missing terminal kind".to_string())
        })?;
    let target_state_json: String = row.try_get("target_state")?;
    let state = serde_json::from_str(&target_state_json).map_err(|error| {
        DbError::Serialization(format!("decode direct-turn terminal target state: {error}"))
    })?;
    let timestamp_us: i64 = row.try_get("target_state_updated_at_us")?;
    let state_updated_at =
        DateTime::<Utc>::from_timestamp_micros(timestamp_us).ok_or_else(|| {
            DbError::Serialization(format!(
                "direct-turn terminal target timestamp is out of range: {timestamp_us}"
            ))
        })?;
    Ok(DirectTurnTerminalObligation {
        turn_id: TurnAuthorityId(to_u64(row.try_get("turn_id")?, "turn_id")?),
        expected_generation: to_u64(row.try_get("expected_generation")?, "expected_generation")?,
        terminal,
        projection: PersistedConversationProjection {
            state,
            state_updated_at,
        },
        response_message_id: row.try_get("response_message_id")?,
    })
}

fn terminal_commit_timestamp() -> Timestamp {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    Timestamp(seconds)
}

async fn load_active_runtime_turn_tx(
    _pool: &sqlx::SqlitePool,
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    conversation: &ConversationAuthority,
) -> DbResult<Option<DurableTurn>> {
    let row = sqlx::query(
        "SELECT * FROM durable_turns
         WHERE conversation_id = ?1
           AND disposition = 'Runtime'
           AND terminal_kind IS NULL
           AND owns_conversation = 1",
    )
    .bind(&conversation.0)
    .fetch_optional(&mut **tx)
    .await?;
    match row {
        Some(row) => Ok(Some(row_to_turn_tx(tx, row).await?)),
        None => Ok(None),
    }
}

async fn load_turn_for_workflow_tx(
    _pool: &sqlx::SqlitePool,
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    turn_id: TurnAuthorityId,
    workflow_id: WorkflowId,
) -> DbResult<Option<DurableTurn>> {
    let row = sqlx::query("SELECT * FROM durable_turns WHERE turn_id = ?1 AND workflow_id = ?2")
        .bind(to_i64(turn_id.0, "turn_id")?)
        .bind(to_i64(workflow_id.0, "workflow_id")?)
        .fetch_optional(&mut **tx)
        .await?;
    match row {
        Some(row) => Ok(Some(row_to_turn_tx(tx, row).await?)),
        None => Ok(None),
    }
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

fn rehydrate_prepared_from_parts(
    fingerprint: String,
    conversation: &ConversationAuthority,
    payload: &PreparedDirectTurnPayload,
) -> DbResult<PreparedTurn> {
    let exact = payload
        .to_exact_bytes()
        .map_err(|error| DbError::Serialization(error.to_string()))?;
    PreparedTurn::rehydrate(conversation, fingerprint, exact).map_err(conflict)
}

async fn load_prepared_turn_from_row(
    pool: &sqlx::SqlitePool,
    row: &sqlx::sqlite::SqliteRow,
) -> DbResult<PreparedTurn> {
    let conversation = ConversationAuthority(row.get("conversation_id"));
    let payload = load_prepared_payload_pool(
        pool,
        TurnAuthorityId(to_u64(row.get("turn_id"), "turn_id")?),
    )
    .await?;
    rehydrate_prepared_from_parts(row.get("prepared_fingerprint"), &conversation, &payload)
}

fn verify_prepared_payload(
    turn: &DurableTurn,
    stored: &PreparedDirectTurnPayload,
    prepared: &PreparedDirectTurnPayload,
) -> DbResult<()> {
    if prepared != stored {
        return Err(prepared_semantics_changed(&turn.prepared));
    }
    Ok(())
}

fn prepared_semantics_changed(prepared: &PreparedTurn) -> DbError {
    conflict(TurnConflict::PreparedSemanticsChanged {
        authoritative_fingerprint: prepared.fingerprint().to_string(),
    })
}

fn decode_prepared_payload_with_normalized_attachments(
    payload: &[u8],
    submitted_images: Vec<ImageData>,
    submitted_files: Vec<SubmittedDirectTurnFileAttachment>,
    delivery_images: Vec<ImageData>,
    delivery_files: Vec<FileAttachment>,
) -> DbResult<PreparedDirectTurnPayload> {
    let has_normalized_attachments = !submitted_images.is_empty()
        || !submitted_files.is_empty()
        || !delivery_images.is_empty()
        || !delivery_files.is_empty();
    if !has_normalized_attachments {
        if let Ok(legacy) = PreparedDirectTurnPayload::from_exact_bytes(payload) {
            return Ok(legacy);
        }
    }
    PreparedDirectTurnPayload::rehydrate_from_normalized_bytes(
        payload,
        submitted_images,
        submitted_files,
        delivery_images,
        delivery_files,
    )
    .map_err(|error| DbError::Serialization(error.to_string()))
}

async fn load_prepared_payload_pool(
    pool: &sqlx::SqlitePool,
    turn_id: TurnAuthorityId,
) -> DbResult<PreparedDirectTurnPayload> {
    let payload: Vec<u8> =
        sqlx::query_scalar("SELECT prepared_payload FROM durable_turns WHERE turn_id = ?1")
            .bind(to_i64(turn_id.0, "turn_id")?)
            .fetch_one(pool)
            .await?;
    let submitted_images = load_prepared_turn_submitted_images(pool, turn_id).await?;
    let submitted_files = load_prepared_turn_submitted_files(pool, turn_id).await?;
    let delivery_images = load_prepared_turn_delivery_images(pool, turn_id).await?;
    let delivery_files = load_prepared_turn_delivery_files(pool, turn_id).await?;
    decode_prepared_payload_with_normalized_attachments(
        &payload,
        submitted_images,
        submitted_files,
        delivery_images,
        delivery_files,
    )
}

async fn load_prepared_payload_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    turn_id: TurnAuthorityId,
) -> DbResult<PreparedDirectTurnPayload> {
    let payload: Vec<u8> =
        sqlx::query_scalar("SELECT prepared_payload FROM durable_turns WHERE turn_id = ?1")
            .bind(to_i64(turn_id.0, "turn_id")?)
            .fetch_one(&mut **tx)
            .await?;
    let submitted_images = load_prepared_turn_submitted_images(tx.as_mut(), turn_id).await?;
    let submitted_files = load_prepared_turn_submitted_files(tx.as_mut(), turn_id).await?;
    let delivery_images = load_prepared_turn_delivery_images(tx.as_mut(), turn_id).await?;
    let delivery_files = load_prepared_turn_delivery_files(tx.as_mut(), turn_id).await?;
    decode_prepared_payload_with_normalized_attachments(
        &payload,
        submitted_images,
        submitted_files,
        delivery_images,
        delivery_files,
    )
}

async fn load_prepared_turn_submitted_images<'c, E>(
    executor: E,
    turn_id: TurnAuthorityId,
) -> DbResult<Vec<ImageData>>
where
    E: sqlx::Executor<'c, Database = sqlx::Sqlite>,
{
    sqlx::query("SELECT media_type, data FROM durable_turn_submitted_images WHERE turn_id = ?1 ORDER BY ordinal")
        .bind(to_i64(turn_id.0, "turn_id")?)
        .map(|row: sqlx::sqlite::SqliteRow| ImageData {
            data: row.get("data"),
            media_type: row.get("media_type"),
        })
        .fetch_all(executor)
        .await
        .map_err(DbError::Sqlx)
}

async fn load_prepared_turn_submitted_files<'c, E>(
    executor: E,
    turn_id: TurnAuthorityId,
) -> DbResult<Vec<SubmittedDirectTurnFileAttachment>>
where
    E: sqlx::Executor<'c, Database = sqlx::Sqlite>,
{
    sqlx::query(
        "SELECT original_name, media_type, size_bytes, stored_path
         FROM durable_turn_submitted_files WHERE turn_id = ?1 ORDER BY ordinal",
    )
    .bind(to_i64(turn_id.0, "turn_id")?)
    .map(
        |row: sqlx::sqlite::SqliteRow| SubmittedDirectTurnFileAttachment {
            original_name: row.get("original_name"),
            media_type: row.get("media_type"),
            size_bytes: u64::try_from(row.get::<i64, _>("size_bytes")).unwrap_or(0),
            stored_path: row.get("stored_path"),
        },
    )
    .fetch_all(executor)
    .await
    .map_err(DbError::Sqlx)
}

async fn load_prepared_turn_delivery_images<'c, E>(
    executor: E,
    turn_id: TurnAuthorityId,
) -> DbResult<Vec<ImageData>>
where
    E: sqlx::Executor<'c, Database = sqlx::Sqlite>,
{
    sqlx::query("SELECT media_type, data FROM durable_turn_delivery_images WHERE turn_id = ?1 ORDER BY ordinal")
        .bind(to_i64(turn_id.0, "turn_id")?)
        .map(|row: sqlx::sqlite::SqliteRow| ImageData {
            data: row.get("data"),
            media_type: row.get("media_type"),
        })
        .fetch_all(executor)
        .await
        .map_err(DbError::Sqlx)
}

async fn load_prepared_turn_delivery_files<'c, E>(
    executor: E,
    turn_id: TurnAuthorityId,
) -> DbResult<Vec<FileAttachment>>
where
    E: sqlx::Executor<'c, Database = sqlx::Sqlite>,
{
    sqlx::query(
        "SELECT original_name, media_type, size_bytes, stored_path
         FROM durable_turn_delivery_files WHERE turn_id = ?1 ORDER BY ordinal",
    )
    .bind(to_i64(turn_id.0, "turn_id")?)
    .map(|row: sqlx::sqlite::SqliteRow| FileAttachment {
        original_name: row.get("original_name"),
        media_type: row.get("media_type"),
        size_bytes: u64::try_from(row.get::<i64, _>("size_bytes")).unwrap_or(0),
        stored_path: row.get("stored_path"),
    })
    .fetch_all(executor)
    .await
    .map_err(DbError::Sqlx)
}

async fn insert_prepared_turn_attachments_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    turn_id: TurnAuthorityId,
    prepared: &PreparedDirectTurnPayload,
) -> DbResult<()> {
    for (ordinal, image) in prepared.submitted.images.iter().enumerate() {
        sqlx::query(
            "INSERT INTO durable_turn_submitted_images (turn_id, ordinal, media_type, data)
             VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(to_i64(turn_id.0, "turn_id")?)
        .bind(i64::try_from(ordinal).unwrap_or(i64::MAX))
        .bind(&image.media_type)
        .bind(&image.data)
        .execute(&mut **tx)
        .await?;
    }
    for (ordinal, file) in prepared.submitted.files.iter().enumerate() {
        sqlx::query(
            "INSERT INTO durable_turn_submitted_files
             (turn_id, ordinal, original_name, media_type, size_bytes, stored_path)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )
        .bind(to_i64(turn_id.0, "turn_id")?)
        .bind(i64::try_from(ordinal).unwrap_or(i64::MAX))
        .bind(&file.original_name)
        .bind(&file.media_type)
        .bind(i64::try_from(file.size_bytes).unwrap_or(i64::MAX))
        .bind(&file.stored_path)
        .execute(&mut **tx)
        .await?;
    }
    for (ordinal, image) in prepared.delivery.images.iter().enumerate() {
        sqlx::query(
            "INSERT INTO durable_turn_delivery_images (turn_id, ordinal, media_type, data)
             VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(to_i64(turn_id.0, "turn_id")?)
        .bind(i64::try_from(ordinal).unwrap_or(i64::MAX))
        .bind(&image.media_type)
        .bind(&image.data)
        .execute(&mut **tx)
        .await?;
    }
    for (ordinal, file) in prepared.delivery.files.iter().enumerate() {
        sqlx::query(
            "INSERT INTO durable_turn_delivery_files
             (turn_id, ordinal, original_name, media_type, size_bytes, stored_path)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )
        .bind(to_i64(turn_id.0, "turn_id")?)
        .bind(i64::try_from(ordinal).unwrap_or(i64::MAX))
        .bind(&file.original_name)
        .bind(&file.media_type)
        .bind(i64::try_from(file.size_bytes).unwrap_or(i64::MAX))
        .bind(&file.stored_path)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

async fn update_conversation_projection_tx(
    tx: &mut super::WorkflowTx<'_>,
    conversation: &ConversationAuthority,
    projection: &PersistedConversationProjection,
) -> DbResult<()> {
    let state_json = serde_json::to_string(&projection.state)
        .map_err(|error| DbError::Serialization(error.to_string()))?;
    let updated = sqlx::query(
        "UPDATE conversations
         SET state = ?1, state_kind = ?2, state_updated_at = ?3, updated_at = ?3
         WHERE id = ?4",
    )
    .bind(state_json)
    .bind(crate::conv_state_kind(&projection.state))
    .bind(projection.state_updated_at.to_rfc3339())
    .bind(&conversation.0)
    .execute(&mut *tx.tx)
    .await?
    .rows_affected();
    if updated != 1 {
        return Err(DbError::ConversationNotFound(conversation.0.clone()));
    }
    Ok(())
}

async fn update_conversation_state_for_adoption_tx(
    tx: &mut super::WorkflowTx<'_>,
    conversation: &ConversationAuthority,
    input: &MaterializeAuthoritativeTurnInput,
) -> DbResult<()> {
    let state_json = serde_json::to_string(&input.accepted_state)
        .map_err(|error| DbError::Serialization(error.to_string()))?;
    let updated = sqlx::query(
        "UPDATE conversations
         SET state = ?1, state_kind = ?2, state_updated_at = ?3, updated_at = ?3
         WHERE id = ?4",
    )
    .bind(state_json)
    .bind(crate::conv_state_kind(&input.accepted_state))
    .bind(input.state_updated_at.to_rfc3339())
    .bind(&conversation.0)
    .execute(&mut *tx.tx)
    .await?
    .rows_affected();
    if updated != 1 {
        return Err(DbError::ConversationNotFound(conversation.0.clone()));
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

async fn load_optional_message_by_id_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    message_id: &str,
) -> DbResult<Option<Message>> {
    let exists: i64 =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM messages WHERE message_id = ?1)")
            .bind(message_id)
            .fetch_one(&mut **tx)
            .await?;
    if exists == 0 {
        return Ok(None);
    }
    load_message_by_id_tx(tx, message_id).await.map(Some)
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
        return Err(prepared_semantics_changed(&turn.prepared));
    }
    let (expected_content, expected_display) = prepared.message_content_and_display_data();
    if message.content != expected_content || message.display_data != expected_display {
        return Err(prepared_semantics_changed(&turn.prepared));
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
    _pool: &sqlx::SqlitePool,
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    conversation: &ConversationAuthority,
    client_key: &ClientTurnKey,
) -> DbResult<Option<DurableTurn>> {
    let row = sqlx::query(
        "SELECT * FROM durable_turns WHERE conversation_id = ?1 AND client_turn_key = ?2",
    )
    .bind(&conversation.0)
    .bind(client_key.as_str())
    .fetch_optional(&mut **tx)
    .await?;
    match row {
        Some(row) => Ok(Some(row_to_turn_tx(tx, row).await?)),
        None => Ok(None),
    }
}

async fn load_by_scoped_key_pool(
    pool: &sqlx::SqlitePool,
    conversation: &ConversationAuthority,
    client_key: &ClientTurnKey,
) -> DbResult<Option<DurableTurn>> {
    let row = sqlx::query(
        "SELECT * FROM durable_turns WHERE conversation_id = ?1 AND client_turn_key = ?2",
    )
    .bind(&conversation.0)
    .bind(client_key.as_str())
    .fetch_optional(pool)
    .await?;
    match row {
        Some(row) => Ok(Some(row_to_turn_pool(pool, row).await?)),
        None => Ok(None),
    }
}

#[allow(clippy::needless_pass_by_value)]
fn row_to_turn_stub(
    row: sqlx::sqlite::SqliteRow,
) -> DbResult<(
    TurnAuthorityId,
    ConversationAuthority,
    ClientTurnKey,
    u64,
    TurnLifecycle,
    Materialization,
)> {
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
    Ok((
        TurnAuthorityId(to_u64(row.get("turn_id"), "turn_id")?),
        ConversationAuthority(row.get("conversation_id")),
        ClientTurnKey::try_from(row.get::<String, _>("client_turn_key"))
            .map_err(|e| DbError::Serialization(e.to_string()))?,
        to_u64(row.get("generation"), "generation")?,
        lifecycle,
        row.get::<Option<String>, _>("canonical_message_id").map_or(
            Materialization::Unmaterialized,
            |message_id| Materialization::Materialized {
                message_id: CanonicalMessageId(message_id),
            },
        ),
    ))
}

async fn row_to_turn_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    row: sqlx::sqlite::SqliteRow,
) -> DbResult<DurableTurn> {
    let fingerprint: String = row.get("prepared_fingerprint");
    let (id, conversation, client_key, generation, lifecycle, materialization) =
        row_to_turn_stub(row)?;
    let prepared = load_prepared_payload_tx(tx, id).await?;
    Ok(DurableTurn {
        id,
        conversation: conversation.clone(),
        client_key,
        prepared: rehydrate_prepared_from_parts(fingerprint, &conversation, &prepared)?,
        generation,
        lifecycle,
        materialization,
    })
}

async fn row_to_turn_pool(
    pool: &sqlx::SqlitePool,
    row: sqlx::sqlite::SqliteRow,
) -> DbResult<DurableTurn> {
    let (id, conversation, client_key, generation, lifecycle, materialization) =
        row_to_turn_stub(row)?;
    let fingerprint_row =
        sqlx::query("SELECT prepared_fingerprint FROM durable_turns WHERE turn_id = ?1")
            .bind(to_i64(id.0, "turn_id")?)
            .fetch_one(pool)
            .await?;
    let prepared = load_prepared_payload_pool(pool, id).await?;
    Ok(DurableTurn {
        id,
        conversation: conversation.clone(),
        client_key,
        prepared: rehydrate_prepared_from_parts(
            fingerprint_row.get("prepared_fingerprint"),
            &conversation,
            &prepared,
        )?,
        generation,
        lifecycle,
        materialization,
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
    DbError::DirectTurnConflict(conflict)
}

#[allow(clippy::wildcard_enum_match_arm)]
fn map_constraint(error: sqlx::Error) -> DbError {
    match error {
        sqlx::Error::Database(database) if database.is_unique_violation() => {
            DbError::DirectTurnConflict(TurnConflict::PreparedSemanticsChanged {
                authoritative_fingerprint: "sqlite constraint".to_string(),
            })
        }
        other => DbError::Sqlx(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrations::run_pending_migrations;
    use crate::sqlite_telemetry::test_support::EventCapture;
    use crate::workflow::wake::{WakeRegistrationOutcome, WakeRepository};
    use crate::Database;
    use crate::LocalAttemptAuthority;
    use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
    use std::str::FromStr;
    use tracing_subscriber::prelude::*;

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
        db.create_conversation("conv-replay", "Replay", "/tmp", true, None, None)
            .await
            .unwrap();
        db.create_conversation("conv-a-scope", "A scope", "/tmp", true, None, None)
            .await
            .unwrap();
        db.create_conversation("conv-b-scope", "B scope", "/tmp", true, None, None)
            .await
            .unwrap();
        WorkflowRepository::new(db.pool().clone())
    }

    async fn setup_repo_schema(pool: &sqlx::SqlitePool) {
        sqlx::query("CREATE TABLE conversations (id TEXT PRIMARY KEY, slug TEXT, title TEXT NOT NULL DEFAULT '', conv_mode TEXT NOT NULL DEFAULT '{\"mode\":\"Explore\"}', state TEXT NOT NULL DEFAULT '{\"type\":\"idle\"}', cwd TEXT NOT NULL DEFAULT '/tmp', parent_conversation_id TEXT, project_id TEXT, user_initiated BOOLEAN NOT NULL DEFAULT 1, archived BOOLEAN NOT NULL DEFAULT 0, model TEXT, steering_queue TEXT NOT NULL DEFAULT '[]', state_updated_at TEXT NOT NULL DEFAULT '2025-01-01', created_at TEXT NOT NULL DEFAULT '2025-01-01', updated_at TEXT NOT NULL DEFAULT '2025-01-01')").execute(pool).await.unwrap();
        sqlx::query("CREATE TABLE projects (id TEXT PRIMARY KEY, canonical_path TEXT UNIQUE NOT NULL, main_ref TEXT NOT NULL DEFAULT 'main', created_at TEXT NOT NULL DEFAULT '2025-01-01')").execute(pool).await.unwrap();
        sqlx::query("CREATE TABLE messages (message_id TEXT PRIMARY KEY, conversation_id TEXT NOT NULL, sequence_id INTEGER NOT NULL DEFAULT 1, message_type TEXT NOT NULL, content TEXT NOT NULL, display_data TEXT, usage_data TEXT, created_at TEXT NOT NULL DEFAULT '2025-01-01')").execute(pool).await.unwrap();
        run_pending_migrations(pool).await.unwrap();
        for conversation_id in [
            "conv-a",
            "conv-b",
            "conv-c",
            "conv-d",
            "conv-replay",
            "conv-a-scope",
            "conv-b-scope",
        ] {
            let work_scope_id = format!("scope-{conversation_id}");
            sqlx::query(
                "INSERT OR IGNORE INTO work_scopes
                 (id, authority_kind, lifecycle, created_at, updated_at, environment_kind, cwd)
                 VALUES (?1, 'restricted_explore', 'active', '2025-01-01', '2025-01-01', 'unowned_cwd', '/tmp')",
            )
            .bind(&work_scope_id)
            .execute(pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT OR IGNORE INTO conversations
                 (id, runtime_role, work_scope_id) VALUES (?1, 'user', ?2)",
            )
            .bind(conversation_id)
            .bind(work_scope_id)
            .execute(pool)
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn deferred_settlement_snapshot_fails_at_first_write_after_competing_commit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("busy-snapshot.db");
        let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))
            .unwrap()
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(std::time::Duration::from_secs(1));
        let mut settlement = sqlx::SqliteConnection::connect_with(&options)
            .await
            .unwrap();
        let mut competitor = sqlx::SqliteConnection::connect_with(&options)
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE workflow_heads (id INTEGER PRIMARY KEY, version INTEGER NOT NULL)",
        )
        .execute(&mut settlement)
        .await
        .unwrap();
        sqlx::query("INSERT INTO workflow_heads (id, version) VALUES (1, 0)")
            .execute(&mut settlement)
            .await
            .unwrap();

        let mut stale_settlement = sqlx::Connection::begin(&mut settlement).await.unwrap();
        let version: i64 = sqlx::query_scalar("SELECT version FROM workflow_heads WHERE id = 1")
            .fetch_one(&mut *stale_settlement)
            .await
            .unwrap();
        assert_eq!(version, 0);
        sqlx::query("UPDATE workflow_heads SET version = version + 1 WHERE id = 1")
            .execute(&mut competitor)
            .await
            .unwrap();

        let error = sqlx::query(
            "UPDATE workflow_heads SET version = version + 1 WHERE id = 1 AND version = 0",
        )
        .execute(&mut *stale_settlement)
        .await
        .unwrap_err();
        let database_error = error.as_database_error().unwrap();
        assert_eq!(database_error.code().as_deref(), Some("517"));
    }

    #[tokio::test]
    async fn terminal_evidence_probe_classifies_absent_established_and_incomplete() {
        let repo = repo().await;
        let (turn_id, _) = created_turn(&repo, "terminal-evidence-probe", 71).await;
        let expected = DirectTurnTerminalObligationInput {
            turn_id,
            expected_generation: 0,
            terminal: TurnTerminal::Completed,
            projection: PersistedConversationProjection {
                state: ConvState::Idle,
                state_updated_at: chrono::Utc::now(),
            },
            response_message_id: Some("response-terminal-evidence".to_string()),
        };
        assert_eq!(
            repo.probe_terminal_evidence("conv-a", "response-terminal-evidence", &expected)
                .await
                .unwrap(),
            TerminalEvidenceProbe::KnownNotCommitted
        );

        let mut tx = repo.pool.begin().await.unwrap();
        WorkflowRepository::persist_terminal_obligation_tx(&mut tx, &expected)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        assert_eq!(
            repo.probe_terminal_evidence("conv-a", "response-terminal-evidence", &expected)
                .await
                .unwrap(),
            TerminalEvidenceProbe::Incomplete
        );

        sqlx::query(
            "INSERT INTO messages
             (message_id, conversation_id, sequence_id, message_type, content, created_at)
             VALUES (?1, 'conv-a', 9001, 'agent', '[]', '2025-01-01')",
        )
        .bind("response-terminal-evidence")
        .execute(&repo.pool)
        .await
        .unwrap();
        assert_eq!(
            repo.probe_terminal_evidence("conv-a", "response-terminal-evidence", &expected)
                .await
                .unwrap(),
            TerminalEvidenceProbe::Established {
                transcript_generation: None
            }
        );
    }

    #[tokio::test]
    async fn terminal_evidence_probe_classifies_exact_hard_delete_as_retired() {
        let repo = repo().await;
        let (turn_id, _) = created_turn(&repo, "terminal-evidence-retired", 73).await;
        let expected = DirectTurnTerminalObligationInput {
            turn_id,
            expected_generation: 0,
            terminal: TurnTerminal::Cancelled,
            projection: PersistedConversationProjection {
                state: ConvState::Idle,
                state_updated_at: chrono::Utc::now(),
            },
            response_message_id: Some("response-terminal-retired".to_string()),
        };
        sqlx::query(
            "INSERT INTO direct_turn_retirements (turn_id, conversation_id)
             VALUES (?1, 'conv-a')",
        )
        .bind(i64::try_from(turn_id.0).unwrap())
        .execute(&repo.pool)
        .await
        .unwrap();
        sqlx::query("DELETE FROM conversations WHERE id = 'conv-a'")
            .execute(&repo.pool)
            .await
            .unwrap();

        assert_eq!(
            repo.probe_terminal_evidence("conv-a", "response-terminal-retired", &expected)
                .await
                .unwrap(),
            TerminalEvidenceProbe::Retired
        );

        let unrelated = DirectTurnTerminalObligationInput {
            turn_id: TurnAuthorityId(turn_id.0 + 999),
            ..expected
        };
        assert!(repo
            .probe_terminal_evidence("conv-a", "response-terminal-retired", &unrelated)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn terminal_obligation_replay_cannot_mutate_first_payload() {
        let repo = repo().await;
        let (turn_id, _) = created_turn(&repo, "obligation-immutable", 75).await;
        let original = DirectTurnTerminalObligationInput {
            turn_id,
            expected_generation: 0,
            terminal: TurnTerminal::Failed {
                reason: "first".to_string(),
            },
            projection: PersistedConversationProjection {
                state: ConvState::Idle,
                state_updated_at: DateTime::<Utc>::from_timestamp_micros(1).unwrap(),
            },
            response_message_id: Some("first-response".to_string()),
        };
        repo.persist_terminal_obligation(&original).await.unwrap();
        repo.persist_terminal_obligation(&original).await.unwrap();
        let differing = DirectTurnTerminalObligationInput {
            terminal: TurnTerminal::Failed {
                reason: "second".to_string(),
            },
            projection: PersistedConversationProjection {
                state: ConvState::Idle,
                state_updated_at: DateTime::<Utc>::from_timestamp_micros(2).unwrap(),
            },
            response_message_id: Some("second-response".to_string()),
            ..original.clone()
        };
        assert!(repo.persist_terminal_obligation(&differing).await.is_err());
        let row = sqlx::query(
            "SELECT turn_id, expected_generation, terminal_kind, terminal_reason,
                    target_state, target_state_updated_at_us, response_message_id
             FROM direct_turn_terminal_obligations WHERE turn_id = ?1",
        )
        .bind(i64::try_from(turn_id.0).unwrap())
        .fetch_one(&repo.pool)
        .await
        .unwrap();
        let stored = parse_terminal_obligation_row(&row).unwrap();
        assert_eq!(stored.terminal, original.terminal);
        assert!(projections_match(&stored.projection, &original.projection));
        assert_eq!(stored.response_message_id, original.response_message_id);
    }

    #[tokio::test]
    async fn negative_terminal_obligation_timestamp_is_rejected() {
        let repo = repo().await;
        let (turn_id, _) = created_turn(&repo, "negative-terminal-time", 74).await;
        let input = DirectTurnTerminalObligationInput {
            turn_id,
            expected_generation: 0,
            terminal: TurnTerminal::Completed,
            projection: PersistedConversationProjection {
                state: ConvState::Idle,
                state_updated_at: DateTime::<Utc>::from_timestamp_micros(-1).unwrap(),
            },
            response_message_id: None,
        };
        let error = repo.persist_terminal_obligation(&input).await.unwrap_err();
        assert!(error.to_string().contains("must be nonnegative"));
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM direct_turn_terminal_obligations WHERE turn_id = ?1",
        )
        .bind(i64::try_from(turn_id.0).unwrap())
        .fetch_one(&repo.pool)
        .await
        .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn terminal_evidence_probe_recognizes_concurrently_settled_exact_turn() {
        let repo = repo().await;
        let (turn_id, _) = created_turn(&repo, "terminal-evidence-settled", 72).await;
        let expected = DirectTurnTerminalObligationInput {
            turn_id,
            expected_generation: 0,
            terminal: TurnTerminal::Cancelled,
            projection: PersistedConversationProjection {
                state: ConvState::Idle,
                state_updated_at: chrono::Utc::now(),
            },
            response_message_id: Some("response-terminal-settled".to_string()),
        };
        let mut tx = repo.pool.begin().await.unwrap();
        sqlx::query(
            "INSERT INTO messages
             (message_id, conversation_id, sequence_id, message_type, content, created_at)
             VALUES (?1, 'conv-a', 9002, 'agent', '[]', '2025-01-01')",
        )
        .bind("response-terminal-settled")
        .execute(&mut *tx)
        .await
        .unwrap();
        WorkflowRepository::persist_terminal_obligation_tx(&mut tx, &expected)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        repo.terminalize_authoritative_turn(&TerminalizeAuthoritativeTurnInput {
            command: TurnCommand::Cancel {
                turn_id,
                expected_generation: 0,
            },
            projection: Some(expected.projection.clone()),
        })
        .await
        .unwrap();

        assert_eq!(
            repo.probe_terminal_evidence("conv-a", "response-terminal-settled", &expected)
                .await
                .unwrap(),
            TerminalEvidenceProbe::Established {
                transcript_generation: None
            }
        );
    }

    #[tokio::test]
    async fn immediate_settlement_transaction_prevents_snapshot_invalidation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("immediate-settlement.db");
        let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))
            .unwrap()
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(std::time::Duration::ZERO);
        let mut settlement = sqlx::SqliteConnection::connect_with(&options)
            .await
            .unwrap();
        let mut competitor = sqlx::SqliteConnection::connect_with(&options)
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE workflow_heads (id INTEGER PRIMARY KEY, version INTEGER NOT NULL)",
        )
        .execute(&mut settlement)
        .await
        .unwrap();
        sqlx::query("INSERT INTO workflow_heads (id, version) VALUES (1, 0)")
            .execute(&mut settlement)
            .await
            .unwrap();

        let mut reserved_settlement = settlement.begin_with("BEGIN IMMEDIATE").await.unwrap();
        let version: i64 = sqlx::query_scalar("SELECT version FROM workflow_heads WHERE id = 1")
            .fetch_one(&mut *reserved_settlement)
            .await
            .unwrap();
        assert_eq!(version, 0);
        let competitor_error =
            sqlx::query("UPDATE workflow_heads SET version = version + 1 WHERE id = 1")
                .execute(&mut competitor)
                .await
                .unwrap_err();
        assert_eq!(
            competitor_error
                .as_database_error()
                .unwrap()
                .code()
                .as_deref(),
            Some("5")
        );
        sqlx::query("UPDATE workflow_heads SET version = version + 1 WHERE id = 1")
            .execute(&mut *reserved_settlement)
            .await
            .unwrap();
        reserved_settlement.commit().await.unwrap();
    }

    async fn open_workflow_repo_pair() -> (tempfile::TempDir, WorkflowRepository, WorkflowRepository)
    {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("direct-turn.db");
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

    fn input(conversation: &str, key: &str, seed: u8) -> AcceptAuthoritativeTurn {
        input_with_disposition(conversation, key, seed, AcceptedDisposition::Runtime)
    }

    fn input_with_disposition(
        conversation: &str,
        key: &str,
        seed: u8,
        disposition: AcceptedDisposition,
    ) -> AcceptAuthoritativeTurn {
        let conversation = ConversationAuthority(conversation.to_string());
        AcceptAuthoritativeTurn {
            client_key: ClientTurnKey::new(key).unwrap(),
            prepared: prepared_turn(&conversation, &format!("message-{}-{key}", conversation.0)),
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
        let TurnOutcome::Created { turn_id, .. } = created.outcome else {
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
        PreparedDirectTurnPayload::from_parts(
            phoenix_core::domain::sm_event::SubmittedDirectTurnIdentity {
                text: format!("text-{message_id}"),
                images: Vec::new(),
                files: Vec::new(),
                message_id: message_id.to_string(),
                user_agent: Some("agent/test".to_string()),
                skill_invocation: None,
                expansion_policy: phoenix_core::domain::sm_event::SubmittedDirectTurnExpansionPolicy::ExpandReferences,
            },
            phoenix_core::domain::sm_event::PreparedDirectTurnDelivery {
                text: format!("text-{message_id}"),
                llm_text: None,
                images: Vec::new(),
                files: Vec::new(),
                user_agent: Some("agent/test".to_string()),
                skill_invocation: None,
            },
        )
    }

    fn prepared_payload_with_attachments(message_id: &str) -> PreparedDirectTurnPayload {
        PreparedDirectTurnPayload::from_parts(
            phoenix_core::domain::sm_event::SubmittedDirectTurnIdentity {
                text: format!("submitted-{message_id}"),
                images: vec![ImageData {
                    data: "SUBMITTED_IMAGE".to_string(),
                    media_type: "image/png".to_string(),
                }],
                files: vec![SubmittedDirectTurnFileAttachment {
                    original_name: "submitted.txt".to_string(),
                    media_type: "text/plain".to_string(),
                    size_bytes: 3,
                    stored_path: "/tmp/submitted.txt".to_string(),
                }],
                message_id: message_id.to_string(),
                user_agent: Some("agent/test".to_string()),
                skill_invocation: None,
                expansion_policy: phoenix_core::domain::sm_event::SubmittedDirectTurnExpansionPolicy::ExpandReferences,
            },
            phoenix_core::domain::sm_event::PreparedDirectTurnDelivery {
                text: format!("delivery-{message_id}"),
                llm_text: Some(format!("expanded-{message_id}")),
                images: vec![ImageData {
                    data: "DELIVERY_IMAGE".to_string(),
                    media_type: "image/jpeg".to_string(),
                }],
                files: vec![FileAttachment {
                    original_name: "delivery.txt".to_string(),
                    media_type: "text/plain".to_string(),
                    size_bytes: 5,
                    stored_path: "/tmp/delivery.txt".to_string(),
                }],
                user_agent: Some("agent/test".to_string()),
                skill_invocation: None,
            },
        )
    }

    fn prepared_turn(target: &ConversationAuthority, message_id: &str) -> PreparedTurn {
        PreparedTurn::from_exact_payload(
            target,
            prepared_payload(message_id).to_exact_bytes().unwrap(),
        )
    }

    fn canonical_message_id(conversation: &str, submitted_message_id: &str) -> String {
        format!("{conversation}:{submitted_message_id}")
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
            accepted_state: ConvState::LlmRequesting { attempt: 1 },
            state_updated_at: timestamp_to_datetime(Timestamp(now)),
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
    async fn replacement_repository_reconstructs_only_committed_direct_turn_state() {
        let (_dir, writer, replacement) = open_workflow_repo_pair().await;
        let (turn_id, workflow_id) = created_turn(&writer, "replacement", 17).await;
        let authority = writer
            .claim_authoritative_turn(&claim_input(workflow_id, turn_id, 10))
            .await
            .unwrap()
            .authority
            .unwrap();
        assert!(matches!(
            writer
                .materialize_authoritative_turn_at_cut(
                    &materialize_input(turn_id, authority, 1, 1, "message-conv-a-replacement", 10,),
                    TransactionCut::BeforeCommit,
                )
                .await,
            crate::workflow::LocalAuthorityResult::DurableFactEstablished(
                MaterializeAuthoritativeTurnOutcome::NotCommitted
            )
        ));

        let persisted_state: String =
            sqlx::query_scalar("SELECT state FROM conversations WHERE id = 'conv-a'")
                .fetch_one(replacement.pool())
                .await
                .unwrap();
        let message_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE conversation_id = 'conv-a'")
                .fetch_one(replacement.pool())
                .await
                .unwrap();
        let obligations: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM durable_turns
             WHERE conversation_id = 'conv-a' AND terminal_kind IS NULL",
        )
        .fetch_one(replacement.pool())
        .await
        .unwrap();

        assert_eq!(
            serde_json::from_str::<ConvState>(&persisted_state).unwrap(),
            ConvState::Idle
        );
        assert_eq!(message_count, 0);
        assert_eq!(obligations, 1);
    }

    #[tokio::test]
    async fn unavailable_authority_database_returns_unclassified() {
        let repo = repo().await;
        let (turn_id, workflow_id) = created_turn(&repo, "unclassified", 18).await;
        let authority = repo
            .claim_authoritative_turn(&claim_input(workflow_id, turn_id, 10))
            .await
            .unwrap()
            .authority
            .unwrap();
        repo.pool.close().await;

        assert!(matches!(
            repo.materialize_authoritative_turn(&materialize_input(
                turn_id,
                authority,
                1,
                1,
                "message-conv-a-unclassified",
                10,
            ))
            .await,
            crate::workflow::LocalAuthorityResult::DurableFactUnclassified
        ));
    }

    #[tokio::test]
    async fn committed_canonical_payload_mismatch_is_classification_error() {
        let repo = repo().await;
        let (turn_id, workflow_id) = created_turn(&repo, "payload-drift", 28).await;
        let authority = repo
            .claim_authoritative_turn(&claim_input(workflow_id, turn_id, 10))
            .await
            .unwrap()
            .authority
            .unwrap();
        let input = materialize_input(
            turn_id,
            authority.clone(),
            1,
            1,
            "message-conv-a-payload-drift",
            10,
        );
        assert!(matches!(
            repo.materialize_authoritative_turn(&input).await,
            crate::workflow::LocalAuthorityResult::DurableFactEstablished(
                MaterializeAuthoritativeTurnOutcome::Materialized(_)
            )
        ));
        sqlx::query("UPDATE messages SET content = ?1 WHERE message_id = ?2")
            .bind(serde_json::to_string(&MessageContent::user("drifted payload")).unwrap())
            .bind(canonical_message_id(
                "conv-a",
                "message-conv-a-payload-drift",
            ))
            .execute(repo.pool())
            .await
            .unwrap();

        assert!(matches!(
            repo.classify_authoritative_turn_materialization(&input).await,
            Err(DbError::Serialization(message))
                if message.contains("canonical message payload mismatch")
        ));
    }

    #[tokio::test]
    async fn materialization_classifier_uses_one_read_snapshot_for_canonical_rows() {
        let (_dir, repo, second) = open_workflow_repo_pair().await;
        let conversation = ConversationAuthority("conv-a".to_string());
        let payload = prepared_payload_with_attachments("message-conv-a-snapshot");
        let created = repo
            .accept_authoritative_turn(&AcceptAuthoritativeTurn {
                client_key: ClientTurnKey::new("classifier-snapshot").unwrap(),
                prepared: PreparedTurn::from_exact_payload(
                    &conversation,
                    payload.to_exact_bytes().unwrap(),
                ),
                disposition: AcceptedDisposition::Runtime,
                accepted_at: Timestamp(29),
            })
            .await
            .unwrap();
        let TurnOutcome::Created { turn_id, .. } = created.outcome else {
            panic!("expected created turn")
        };
        let workflow_id = repo.workflow_id_for_turn(turn_id).await.unwrap().unwrap();
        let authority = repo
            .claim_authoritative_turn(&claim_input(workflow_id, turn_id, 30))
            .await
            .unwrap()
            .authority
            .unwrap();
        let input = MaterializeAuthoritativeTurnInput {
            turn_id,
            authority,
            prepared: payload,
            sequence_id: 30,
            created_at: Timestamp(30),
            accepted_state: ConvState::LlmRequesting { attempt: 1 },
            state_updated_at: timestamp_to_datetime(Timestamp(30)),
            now: Timestamp(30),
        };
        assert!(matches!(
            repo.materialize_authoritative_turn(&input).await,
            crate::workflow::LocalAuthorityResult::DurableFactEstablished(
                MaterializeAuthoritativeTurnOutcome::Materialized(_)
            )
        ));

        let mut snapshot = repo.begin_tx().await.unwrap();
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM durable_turns")
            .fetch_one(&mut *snapshot.tx)
            .await
            .unwrap();
        sqlx::query("DELETE FROM message_files WHERE message_id = ?1")
            .bind(canonical_message_id("conv-a", "message-conv-a-snapshot"))
            .execute(second.pool())
            .await
            .unwrap();

        assert!(matches!(
            repo.classify_authoritative_turn_materialization_tx(&mut snapshot, &input)
                .await
                .unwrap(),
            MaterializeAuthoritativeTurnOutcome::ClassifiedCommitted(_)
        ));
        snapshot.rollback().await.unwrap();
    }

    #[tokio::test]
    async fn committed_canonical_attachment_drift_is_classification_error() {
        let repo = repo().await;
        let conversation = ConversationAuthority("conv-a".to_string());
        let payload = prepared_payload_with_attachments("message-conv-a-attachment-drift");
        let created = repo
            .accept_authoritative_turn(&AcceptAuthoritativeTurn {
                client_key: ClientTurnKey::new("attachment-drift").unwrap(),
                prepared: PreparedTurn::from_exact_payload(
                    &conversation,
                    payload.to_exact_bytes().unwrap(),
                ),
                disposition: AcceptedDisposition::Runtime,
                accepted_at: Timestamp(29),
            })
            .await
            .unwrap();
        let TurnOutcome::Created { turn_id, .. } = created.outcome else {
            panic!("expected created turn")
        };
        let workflow_id = repo.workflow_id_for_turn(turn_id).await.unwrap().unwrap();
        let authority = repo
            .claim_authoritative_turn(&claim_input(workflow_id, turn_id, 30))
            .await
            .unwrap()
            .authority
            .unwrap();
        let input = MaterializeAuthoritativeTurnInput {
            turn_id,
            authority,
            prepared: payload,
            sequence_id: 30,
            created_at: Timestamp(30),
            accepted_state: ConvState::LlmRequesting { attempt: 1 },
            state_updated_at: timestamp_to_datetime(Timestamp(30)),
            now: Timestamp(30),
        };
        assert!(matches!(
            repo.materialize_authoritative_turn(&input).await,
            crate::workflow::LocalAuthorityResult::DurableFactEstablished(
                MaterializeAuthoritativeTurnOutcome::Materialized(_)
            )
        ));
        let message_id = canonical_message_id("conv-a", "message-conv-a-attachment-drift");
        sqlx::query("DELETE FROM message_files WHERE message_id = ?1")
            .bind(message_id)
            .execute(repo.pool())
            .await
            .unwrap();

        assert!(matches!(
            repo.classify_authoritative_turn_materialization(&input).await,
            Err(DbError::Serialization(message))
                if message.contains("canonical message payload mismatch")
        ));
    }

    #[tokio::test]
    async fn materialization_persists_typed_receipt_payloads() {
        let repo = repo().await;
        let (turn_id, workflow_id) = created_turn(&repo, "typed-receipt", 19).await;
        let claim = repo
            .claim_authoritative_turn(&claim_input(workflow_id, turn_id, 10))
            .await
            .unwrap()
            .authority
            .unwrap();
        let attempt_id = claim.attempt_id;
        let effect_id = claim.effect_id;
        assert!(matches!(
            repo.materialize_authoritative_turn(&materialize_input(
                turn_id,
                claim,
                1,
                1,
                "message-conv-a-typed-receipt",
                10,
            ))
            .await,
            crate::workflow::LocalAuthorityResult::DurableFactEstablished(
                MaterializeAuthoritativeTurnOutcome::Materialized(_)
            )
        ));
        let result = sqlx::query(
            "SELECT r.receipt_payload, d.delivery_id, d.payload_blob
             FROM workflow_receipts r
             JOIN workflow_deliveries d
               ON d.workflow_id = r.workflow_id AND d.effect_id = r.effect_id
             WHERE r.workflow_id = ?1 AND r.effect_id = ?2 AND r.attempt_id = ?3",
        )
        .bind(i64::try_from(workflow_id.0).unwrap())
        .bind(i64::try_from(effect_id.0).unwrap())
        .bind(i64::try_from(attempt_id.0).unwrap())
        .fetch_one(&repo.pool)
        .await
        .unwrap();
        let receipt: direct_turn_profile::DirectTurnReceipt =
            serde_json::from_slice(&result.get::<Vec<u8>, _>("receipt_payload")).unwrap();
        assert_eq!(receipt.turn_id, turn_id.0);
        assert_eq!(
            receipt.canonical_message_id,
            canonical_message_id("conv-a", "message-conv-a-typed-receipt")
        );
        let event: direct_turn_profile::DirectTurnReceiptEvent =
            serde_json::from_slice(&result.get::<Vec<u8>, _>("payload_blob")).unwrap();
        assert_eq!(
            event,
            direct_turn_profile::DirectTurnReceiptEvent::Materialized {
                canonical_message_id: canonical_message_id(
                    "conv-a",
                    "message-conv-a-typed-receipt"
                ),
            }
        );
        let delivery = sqlx::query(
            "SELECT status, runtime_acceptance_status, accepted_by_transition_id
             FROM workflow_deliveries WHERE workflow_id = ?1 AND delivery_id = ?2",
        )
        .bind(i64::try_from(workflow_id.0).unwrap())
        .bind(result.get::<i64, _>("delivery_id"))
        .fetch_one(&repo.pool)
        .await
        .unwrap();
        assert_eq!(delivery.get::<String, _>("status"), "Accepted");
        assert_eq!(
            delivery.get::<Option<String>, _>("runtime_acceptance_status"),
            Some("Accepted".to_string())
        );
        assert_eq!(
            delivery.get::<Option<i64>, _>("accepted_by_transition_id"),
            Some(i64::try_from(DIRECT_TURN_MATERIALIZED_TRANSITION_ID).unwrap())
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn materialization_acquires_write_intent_before_authority_reads() {
        let (_dir, blocker, contender) = open_workflow_repo_pair().await;
        let (turn_id, workflow_id) = created_turn(&blocker, "write-intent", 20).await;
        let authority = blocker
            .claim_authoritative_turn(&claim_input(workflow_id, turn_id, 10))
            .await
            .unwrap()
            .authority
            .unwrap();
        let input = materialize_input(turn_id, authority, 1, 1, "message-conv-a-write-intent", 10);
        let mut write_lock = blocker.pool.acquire().await.unwrap();
        sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut *write_lock)
            .await
            .unwrap();

        let materialization =
            tokio::spawn(async move { contender.materialize_authoritative_turn(&input).await });
        tokio::task::yield_now().await;
        assert!(!materialization.is_finished());

        sqlx::query("ROLLBACK")
            .execute(&mut *write_lock)
            .await
            .unwrap();
        assert!(matches!(
            materialization.await.unwrap(),
            crate::workflow::LocalAuthorityResult::DurableFactEstablished(
                MaterializeAuthoritativeTurnOutcome::Materialized(_)
            )
        ));
    }

    #[tokio::test]
    async fn materialization_refines_before_and_after_commit_crash_cuts() {
        let before_repo = repo().await;
        let created = before_repo
            .accept_authoritative_turn(&input("conv-a", "materialize-before", 3))
            .await
            .unwrap();
        let TurnOutcome::Created { turn_id, .. } = created.outcome else {
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
        assert!(matches!(
            before_repo
                .materialize_authoritative_turn_at_cut(
                    &materialize_input(
                        turn_id,
                        authority.clone(),
                        1,
                        1,
                        "message-conv-a-materialize-before",
                        10,
                    ),
                    TransactionCut::BeforeCommit,
                )
                .await,
            crate::workflow::LocalAuthorityResult::DurableFactEstablished(
                MaterializeAuthoritativeTurnOutcome::NotCommitted
            )
        ));
        assert!(matches!(
            before_repo
                .materialize_authoritative_turn(&materialize_input(
                    turn_id,
                    authority,
                    1,
                    1,
                    "message-conv-a-materialize-before",
                    10,
                ))
                .await,
            crate::workflow::LocalAuthorityResult::DurableFactEstablished(
                MaterializeAuthoritativeTurnOutcome::Materialized(_)
            )
        ));

        let after_repo = repo().await;
        let created = after_repo
            .accept_authoritative_turn(&input("conv-a", "materialize-after", 4))
            .await
            .unwrap();
        let TurnOutcome::Created { turn_id, .. } = created.outcome else {
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
        let after_commit = after_repo
            .materialize_authoritative_turn_at_cut(
                &materialize_input(
                    turn_id,
                    authority.clone(),
                    1,
                    1,
                    "message-conv-a-materialize-after",
                    11,
                ),
                TransactionCut::AfterCommit,
            )
            .await;
        assert!(
            matches!(
                after_commit,
                crate::workflow::LocalAuthorityResult::DurableFactEstablished(
                    MaterializeAuthoritativeTurnOutcome::ClassifiedCommitted(_)
                )
            ),
            "unexpected classified after-commit outcome: {after_commit:?}"
        );
        assert!(matches!(
            after_repo
                .materialize_authoritative_turn(&materialize_input(
                    turn_id,
                    authority,
                    2,
                    2,
                    "message-conv-a-materialize-after",
                    11,
                ))
                .await,
            crate::workflow::LocalAuthorityResult::DurableFactEstablished(
                MaterializeAuthoritativeTurnOutcome::ExactReplay(_)
            )
        ));
    }

    #[tokio::test]
    async fn terminal_refines_before_and_after_commit_crash_cuts() {
        let before_repo = repo().await;
        let created = before_repo
            .accept_authoritative_turn(&input("conv-a", "terminal-before", 5))
            .await
            .unwrap();
        let TurnOutcome::Created { turn_id, .. } = created.outcome else {
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
        let TurnOutcome::Created { turn_id, .. } = created.outcome else {
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
    async fn terminal_projection_and_owner_release_commit_atomically() {
        let repo = repo().await;
        let created = repo
            .accept_authoritative_turn(&input("conv-a", "terminal-projection", 7))
            .await
            .unwrap();
        let TurnOutcome::Created { turn_id, .. } = created.outcome else {
            panic!("expected created turn")
        };
        sqlx::query(
            "UPDATE conversations SET state = ?1, state_kind = 'llm_requesting' WHERE id = 'conv-a'",
        )
            .bind(serde_json::to_string(&ConvState::LlmRequesting { attempt: 0 }).unwrap())
            .execute(&repo.pool)
            .await
            .unwrap();
        let projection = PersistedConversationProjection {
            state: ConvState::Idle,
            state_updated_at: Utc::now(),
        };
        let input = TerminalizeAuthoritativeTurnInput {
            command: TurnCommand::Complete {
                turn_id,
                expected_generation: 0,
            },
            projection: Some(projection.clone()),
        };

        assert!(repo
            .terminalize_authoritative_turn_at_cut(&input, TransactionCut::BeforeCommit)
            .await
            .is_err());
        let after_cut = repo
            .load_authoritative_turn(turn_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(after_cut.generation, 0);
        assert!(after_cut.owns_conversation());
        let state_after_cut: String =
            sqlx::query_scalar("SELECT state FROM conversations WHERE id = 'conv-a'")
                .fetch_one(&repo.pool)
                .await
                .unwrap();
        assert!(matches!(
            serde_json::from_str::<ConvState>(&state_after_cut).unwrap(),
            ConvState::LlmRequesting { .. }
        ));

        repo.terminalize_authoritative_turn(&input).await.unwrap();
        let committed = repo
            .load_authoritative_turn(turn_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(committed.generation, 1);
        assert!(!committed.owns_conversation());
        let state_after_commit: String =
            sqlx::query_scalar("SELECT state FROM conversations WHERE id = 'conv-a'")
                .fetch_one(&repo.pool)
                .await
                .unwrap();
        assert_eq!(
            serde_json::from_str::<ConvState>(&state_after_commit).unwrap(),
            projection.state
        );
    }

    #[tokio::test]
    async fn child_terminal_reconcile_rotates_id_and_preserves_originating_authority() {
        let repo = repo().await;
        sqlx::query(
            "UPDATE conversations SET parent_conversation_id = 'conv-b' WHERE id = 'conv-a'",
        )
        .execute(&repo.pool)
        .await
        .unwrap();
        let original_parent = repo
            .accept_authoritative_turn(&input("conv-b", "original-parent-turn", 6))
            .await
            .unwrap();
        let TurnOutcome::Created {
            turn_id: original_parent_turn_id,
            ..
        } = original_parent.outcome
        else {
            panic!("expected original parent turn")
        };
        sqlx::query(
            "INSERT INTO startup_parent_actions
                 (conversation_id, action, transcript_generation,
                  turn_id, turn_generation, created_at)
             SELECT c.id, 'Resume', c.transcript_generation,
                    t.turn_id, t.generation, '2025-01-01'
             FROM conversations AS c
             JOIN durable_turns AS t ON t.conversation_id = c.id
                 AND t.owns_conversation = 1 AND t.terminal_kind IS NULL
             WHERE c.id = 'conv-b'",
        )
        .execute(&repo.pool)
        .await
        .unwrap();
        let original_id: i64 = sqlx::query_scalar(
            "SELECT action_id FROM startup_parent_actions WHERE conversation_id = 'conv-b'",
        )
        .fetch_one(&repo.pool)
        .await
        .unwrap();
        repo.terminalize_authoritative_turn(&TerminalizeAuthoritativeTurnInput {
            command: TurnCommand::Complete {
                turn_id: original_parent_turn_id,
                expected_generation: 0,
            },
            projection: Some(PersistedConversationProjection {
                state: ConvState::Idle,
                state_updated_at: Utc::now(),
            }),
        })
        .await
        .unwrap();
        let later_parent = repo
            .accept_authoritative_turn(&input("conv-b", "later-parent-turn", 7))
            .await
            .unwrap();
        let TurnOutcome::Created {
            turn_id: later_parent_turn_id,
            ..
        } = later_parent.outcome
        else {
            panic!("expected later parent turn")
        };
        assert_ne!(later_parent_turn_id, original_parent_turn_id);

        let created = repo
            .accept_authoritative_turn(&input("conv-a", "child-terminal-action", 8))
            .await
            .unwrap();
        let TurnOutcome::Created { turn_id, .. } = created.outcome else {
            panic!("expected created turn")
        };

        repo.terminalize_authoritative_turn(&TerminalizeAuthoritativeTurnInput {
            command: TurnCommand::Complete {
                turn_id,
                expected_generation: 0,
            },
            projection: Some(PersistedConversationProjection {
                state: ConvState::Idle,
                state_updated_at: Utc::now(),
            }),
        })
        .await
        .unwrap();

        let replacement: (i64, String, Option<i64>, Option<i64>) = sqlx::query_as(
            "SELECT action_id, action, turn_id, turn_generation
             FROM startup_parent_actions WHERE conversation_id = 'conv-b'",
        )
        .fetch_one(&repo.pool)
        .await
        .unwrap();
        assert_eq!(replacement.1, "Reconcile");
        assert_ne!(replacement.0, original_id);
        assert_eq!(
            replacement.2,
            Some(to_i64(original_parent_turn_id.0, "turn_id").unwrap())
        );
        assert_eq!(replacement.3, Some(0));
        assert_ne!(
            replacement.2,
            Some(to_i64(later_parent_turn_id.0, "turn_id").unwrap())
        );
    }

    #[tokio::test]
    async fn terminal_projection_probe_suppresses_a_superseded_projection() {
        let repo = repo().await;
        let created = repo
            .accept_authoritative_turn(&input("conv-a", "terminal-probe-old", 9))
            .await
            .unwrap();
        let TurnOutcome::Created { turn_id, .. } = created.outcome else {
            panic!("expected created turn")
        };
        let projection = PersistedConversationProjection {
            state: ConvState::Idle,
            state_updated_at: Utc::now(),
        };
        repo.terminalize_authoritative_turn(&TerminalizeAuthoritativeTurnInput {
            command: TurnCommand::Complete {
                turn_id,
                expected_generation: 0,
            },
            projection: Some(projection.clone()),
        })
        .await
        .unwrap();
        let expected = DirectTurnTerminalObligation {
            turn_id,
            expected_generation: 0,
            terminal: TurnTerminal::Completed,
            projection,
            response_message_id: None,
        };
        assert_eq!(
            repo.probe_terminal_projection(&expected).await.unwrap(),
            TerminalProjectionProbe::Current
        );

        repo.accept_authoritative_turn(&input("conv-a", "terminal-probe-new", 10))
            .await
            .unwrap();
        assert_eq!(
            repo.probe_terminal_projection(&expected).await.unwrap(),
            TerminalProjectionProbe::Superseded
        );
    }

    #[tokio::test]
    async fn continuation_message_projection_and_owner_release_commit_atomically() {
        let repo = repo().await;
        let created = repo
            .accept_authoritative_turn(&input("conv-a", "continuation-terminal", 8))
            .await
            .unwrap();
        let TurnOutcome::Created { turn_id, .. } = created.outcome else {
            panic!("expected created turn")
        };
        let operation_id = "continuation-operation";
        let awaiting = ConvState::AwaitingContinuation {
            request: phoenix_core::domain::sm_state::ContinuationSummaryRequest {
                operation_id: operation_id.to_string(),
                rejected_tool_calls: Vec::new(),
                attempt: 1,
            },
        };
        sqlx::query("UPDATE conversations SET state = ?1, state_kind = ?2 WHERE id = 'conv-a'")
            .bind(serde_json::to_string(&awaiting).unwrap())
            .bind("awaiting_continuation")
            .execute(&repo.pool)
            .await
            .unwrap();
        let completed = ConvState::ContextExhausted {
            summary: "durable summary".to_string(),
        };
        let content = crate::MessageContent::continuation("durable summary");
        let message = crate::Message {
            message_id: format!("continuation-conv-a-{operation_id}"),
            conversation_id: "conv-a".to_string(),
            sequence_id: 1,
            message_type: content.message_type(),
            content,
            display_data: None,
            usage_data: None,
            created_at: Utc::now(),
        };
        let input = AtomicContinuationSettlementInput {
            conversation_id: "conv-a".to_string(),
            operation_id: operation_id.to_string(),
            message,
            completed_state: completed.clone(),
            state_updated_at: Utc::now(),
            command: TurnCommand::Complete {
                turn_id,
                expected_generation: 0,
            },
        };

        assert_eq!(
            repo.settle_continuation_direct_turn_atomically(&input)
                .await
                .unwrap(),
            crate::ContinuationCommitOutcome::Applied
        );
        let turn = repo
            .load_authoritative_turn(turn_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(turn.generation, 1);
        assert!(!turn.owns_conversation());
        let (state_json, state_kind): (String, String) =
            sqlx::query_as("SELECT state, state_kind FROM conversations WHERE id = 'conv-a'")
                .fetch_one(&repo.pool)
                .await
                .unwrap();
        assert_eq!(
            serde_json::from_str::<ConvState>(&state_json).unwrap(),
            completed
        );
        assert_eq!(state_kind, "context_exhausted");
        let message_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE conversation_id = 'conv-a'")
                .fetch_one(&repo.pool)
                .await
                .unwrap();
        assert_eq!(message_count, 1);

        assert_eq!(
            repo.settle_continuation_direct_turn_atomically(&input)
                .await
                .unwrap(),
            crate::ContinuationCommitOutcome::Duplicate
        );
        assert_eq!(
            repo.load_authoritative_turn(turn_id)
                .await
                .unwrap()
                .unwrap()
                .generation,
            1
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn continuation_settlement_contention_records_statement_failure() {
        let (directory, setup_repo, _) = open_workflow_repo_pair().await;
        let created = setup_repo
            .accept_authoritative_turn(&input("conv-a", "continuation-contention", 9))
            .await
            .unwrap();
        let TurnOutcome::Created { turn_id, .. } = created.outcome else {
            panic!("expected created turn")
        };
        let operation_id = "continuation-contention-operation";
        let awaiting = ConvState::AwaitingContinuation {
            request: phoenix_core::domain::sm_state::ContinuationSummaryRequest {
                operation_id: operation_id.to_string(),
                rejected_tool_calls: Vec::new(),
                attempt: 1,
            },
        };
        sqlx::query("UPDATE conversations SET state = ?1, state_kind = ?2 WHERE id = 'conv-a'")
            .bind(serde_json::to_string(&awaiting).unwrap())
            .bind("awaiting_continuation")
            .execute(&setup_repo.pool)
            .await
            .unwrap();
        let completed = ConvState::ContextExhausted {
            summary: "durable summary".to_string(),
        };
        let content = crate::MessageContent::continuation("durable summary");
        let settlement = AtomicContinuationSettlementInput {
            conversation_id: "conv-a".to_string(),
            operation_id: operation_id.to_string(),
            message: crate::Message {
                message_id: format!("continuation-conv-a-{operation_id}"),
                conversation_id: "conv-a".to_string(),
                sequence_id: 1,
                message_type: content.message_type(),
                content,
                display_data: None,
                usage_data: None,
                created_at: Utc::now(),
            },
            completed_state: completed,
            state_updated_at: Utc::now(),
            command: TurnCommand::Complete {
                turn_id,
                expected_generation: 0,
            },
        };

        let path = directory.path().join("direct-turn.db");
        let contending_pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))
                    .unwrap()
                    .journal_mode(SqliteJournalMode::Wal)
                    .busy_timeout(std::time::Duration::ZERO),
            )
            .await
            .unwrap();
        let contending_repo = WorkflowRepository::new(contending_pool);
        let mut writer = setup_repo.pool.acquire().await.unwrap();
        sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut *writer)
            .await
            .unwrap();

        let capture = EventCapture::default();
        let subscriber = tracing_subscriber::registry().with(capture.clone());
        let _subscriber_guard = tracing::subscriber::set_default(subscriber);
        let error = contending_repo
            .settle_continuation_direct_turn_atomically(&settlement)
            .await
            .unwrap_err();
        sqlx::query("ROLLBACK").execute(&mut *writer).await.unwrap();

        assert!(matches!(error, DbError::Sqlx(_)));
        let events = capture.events();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].get("db_operation").map(String::as_str),
            Some("direct_turn.terminal_settlement")
        );
        assert_eq!(
            events[0].get("db_phase").map(String::as_str),
            Some("statement")
        );
        assert_eq!(
            events[0].get("db_sqlite_primary_code").map(String::as_str),
            Some("5")
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn legacy_continuation_reconciliation_records_statement_failure() {
        let (directory, setup_repo, _) = open_workflow_repo_pair().await;
        let awaiting = ConvState::AwaitingContinuation {
            request: phoenix_core::domain::sm_state::ContinuationSummaryRequest {
                operation_id: phoenix_core::domain::sm_state::LEGACY_CONTINUATION_OPERATION_ID
                    .to_string(),
                rejected_tool_calls: Vec::new(),
                attempt: 1,
            },
        };
        sqlx::query("UPDATE conversations SET state = ?1, state_kind = ?2 WHERE id = 'conv-a'")
            .bind(serde_json::to_string(&awaiting).unwrap())
            .bind("awaiting_continuation")
            .execute(&setup_repo.pool)
            .await
            .unwrap();
        let content = crate::MessageContent::continuation("legacy durable summary");
        sqlx::query(
            "INSERT INTO messages
             (message_id, conversation_id, sequence_id, message_type, content, created_at)
             VALUES ('legacy-continuation', 'conv-a', 1, 'continuation', ?1, ?2)",
        )
        .bind(serde_json::to_string(&content.to_stored_json()).unwrap())
        .bind(Utc::now().to_rfc3339())
        .execute(&setup_repo.pool)
        .await
        .unwrap();

        let path = directory.path().join("direct-turn.db");
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))
                    .unwrap()
                    .journal_mode(SqliteJournalMode::Wal)
                    .busy_timeout(std::time::Duration::ZERO),
            )
            .await
            .unwrap();
        let contending_repo = WorkflowRepository::new(pool);
        let mut writer = setup_repo.pool.acquire().await.unwrap();
        sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut *writer)
            .await
            .unwrap();

        let capture = EventCapture::default();
        let subscriber = tracing_subscriber::registry().with(capture.clone());
        let _subscriber_guard = tracing::subscriber::set_default(subscriber);
        let error = contending_repo
            .reconcile_legacy_continuation_atomically("conv-a", Utc::now())
            .await
            .unwrap_err();
        sqlx::query("ROLLBACK").execute(&mut *writer).await.unwrap();

        assert!(matches!(error, DbError::Sqlx(_)));
        let events = capture.events();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].get("db_operation").map(String::as_str),
            Some("direct_turn.terminal_settlement")
        );
        assert_eq!(
            events[0].get("db_phase").map(String::as_str),
            Some("statement")
        );
        assert_eq!(
            events[0].get("db_sqlite_primary_code").map(String::as_str),
            Some("5")
        );
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
        let TurnOutcome::Created { turn_id, .. } = created.outcome else {
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
        let TurnOutcome::Created { turn_id, .. } = created.outcome else {
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
                    turn_id,
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
                turn_id,
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
    async fn acceptance_allows_same_client_message_identity_in_other_conversation() {
        let repo = repo().await;
        sqlx::query(
            "INSERT INTO messages
             (message_id, conversation_id, sequence_id, message_type, content, created_at)
             VALUES (?1, 'conv-b', 1, 'user', '[]', '2025-01-01T00:00:00Z')",
        )
        .bind(canonical_message_id(
            "conv-b",
            "message-conv-a-cross-conversation",
        ))
        .execute(&repo.pool)
        .await
        .unwrap();

        let step = repo
            .accept_authoritative_turn(&input("conv-a", "cross-conversation", 11))
            .await
            .unwrap();
        assert!(matches!(step.outcome, TurnOutcome::Created { .. }));
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM durable_turns")
                .fetch_one(&repo.pool)
                .await
                .unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn schema_rejects_cross_conversation_canonical_message() {
        let repo = repo().await;
        let created = repo
            .accept_authoritative_turn(&input("conv-a", "schema", 11))
            .await
            .unwrap();
        let TurnOutcome::Created { turn_id, .. } = created.outcome else {
            panic!("expected created turn")
        };
        sqlx::query(
            "INSERT INTO messages
             (message_id, conversation_id, sequence_id, message_type, content, created_at)
             VALUES (?1, 'conv-b', 1, 'user', '[]', '2025-01-01T00:00:00Z')",
        )
        .bind(canonical_message_id("conv-b", "foreign-message"))
        .execute(&repo.pool)
        .await
        .unwrap();
        assert!(sqlx::query(
            "UPDATE durable_turns SET canonical_message_id = ?2 WHERE turn_id = ?1",
        )
        .bind(i64::try_from(turn_id.0).unwrap())
        .bind(canonical_message_id("conv-b", "foreign-message"))
        .execute(&repo.pool)
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
        let TurnOutcome::Created { turn_id, .. } = created.outcome else {
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
        let TurnOutcome::Created { turn_id, .. } = created.outcome else {
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
        let TurnOutcome::Created {
            turn_id: first_id, ..
        } = first.outcome
        else {
            panic!("expected first runtime turn")
        };
        let first_workflow = repo.workflow_id_for_turn(first_id).await.unwrap().unwrap();
        let second = repo
            .accept_authoritative_turn(&input("conv-b", "second", 22))
            .await
            .unwrap();
        let TurnOutcome::Created {
            turn_id: second_id, ..
        } = second.outcome
        else {
            panic!("expected second runtime turn")
        };
        let second_workflow = repo.workflow_id_for_turn(second_id).await.unwrap().unwrap();
        let materialized = repo
            .accept_authoritative_turn(&input("conv-c", "materialized", 23))
            .await
            .unwrap();
        let TurnOutcome::Created {
            turn_id: materialized_id,
            ..
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
        .established()
        .expect("classified direct-turn materialization");

        let first_page = repo
            .list_discoverable_accepted_runtime_direct_turns(None, 1)
            .await
            .unwrap();
        assert_eq!(
            first_page
                .candidates
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
                .candidates
                .iter()
                .map(|row| (row.turn_id, row.workflow_id, row.conversation.0.as_str()))
                .collect::<Vec<_>>(),
            vec![(second_id, second_workflow, "conv-b")]
        );
        assert!(repo
            .list_discoverable_accepted_runtime_direct_turns(None, 0)
            .await
            .unwrap()
            .candidates
            .is_empty());
    }

    #[tokio::test]
    async fn direct_turn_and_wake_allocate_distinct_global_workflow_ids() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("conv-a", "A", "/tmp", true, None, None)
            .await
            .unwrap();
        let workflow_repo = WorkflowRepository::new(db.pool().clone());
        let wake_repo = WakeRepository::new(db.pool().clone());

        let created = workflow_repo
            .accept_authoritative_turn(&input("conv-a", "direct-first", 41))
            .await
            .unwrap();
        let TurnOutcome::Created { turn_id, .. } = created.outcome else {
            panic!("expected direct turn creation")
        };
        let direct_workflow_id = workflow_repo
            .workflow_id_for_turn(turn_id)
            .await
            .unwrap()
            .unwrap();
        let work_scope_id: String =
            sqlx::query_scalar("SELECT work_scope_id FROM conversations WHERE id = 'conv-a'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let wake_intent = phoenix_workflow::wake_profile::WakeRegistrationIntent {
            contract_id: "contract-direct-cross-profile".to_string(),
            conversation_id: "conv-a".to_string(),
            root_conversation_id: "conv-a".to_string(),
            registration_scope: phoenix_workflow::wake_profile::WorkScopeIdentity(
                work_scope_id.clone(),
            ),
            resource: phoenix_workflow::wake_profile::WakeResourceIdentity::TmuxWindow(
                phoenix_workflow::wake_profile::TmuxResourceIdentity {
                    work_scope: phoenix_workflow::wake_profile::WorkScopeIdentity(work_scope_id),
                    server_token: "server-direct-cross-profile".to_string(),
                    window_id: "@direct-cross-profile".to_string(),
                    completion_policy:
                        phoenix_workflow::wake_profile::TmuxCompletionPolicy::KeepOpen,
                },
            ),
            registering_tool_use_id: "tool-direct-cross-profile".to_string(),
            registered_at: Timestamp(42),
            expires_at: Timestamp(100),
        };
        let wake_workflow_id = match wake_repo
            .register(&wake_intent, "wake-after-direct", Timestamp(42))
            .await
            .unwrap()
        {
            WakeRegistrationOutcome::Registered { workflow_id, .. } => workflow_id,
            other @ (WakeRegistrationOutcome::Replayed { .. }
            | WakeRegistrationOutcome::Conflict) => {
                panic!("expected wake registration, got {other:?}")
            }
        };

        assert_ne!(direct_workflow_id, wake_workflow_id);
        let workflow_ids =
            sqlx::query_scalar::<_, i64>("SELECT COUNT(DISTINCT workflow_id) FROM workflows")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(workflow_ids, 2);
    }

    #[tokio::test]
    async fn concurrent_direct_turns_on_different_scoped_keys_allocate_distinct_ids() {
        let (_dir, first, second) = open_workflow_repo_pair().await;
        let left_input = input("conv-a", "left", 51);
        let right_input = input("conv-b", "right", 52);
        let (left, right) = tokio::join!(
            first.accept_authoritative_turn(&left_input),
            second.accept_authoritative_turn(&right_input)
        );
        let TurnOutcome::Created {
            turn_id: left_id, ..
        } = left.unwrap().outcome
        else {
            panic!("expected left creation")
        };
        let TurnOutcome::Created {
            turn_id: right_id, ..
        } = right.unwrap().outcome
        else {
            panic!("expected right creation")
        };
        let left_workflow = first.workflow_id_for_turn(left_id).await.unwrap().unwrap();
        let right_workflow = first.workflow_id_for_turn(right_id).await.unwrap().unwrap();

        assert_ne!(left_id, right_id);
        assert_ne!(left_workflow, right_workflow);
    }

    #[tokio::test]
    async fn concurrent_direct_turns_on_same_scoped_key_converge_to_created_and_exact_replay() {
        let (_dir, first, second) = open_workflow_repo_pair().await;
        let direct_input = input("conv-a", "same-concurrent", 61);
        let (left, right) = tokio::join!(
            first.accept_authoritative_turn(&direct_input),
            second.accept_authoritative_turn(&direct_input)
        );
        let left = left.unwrap().outcome;
        let right = right.unwrap().outcome;
        let created = [left.clone(), right.clone()]
            .into_iter()
            .filter(|outcome| matches!(outcome, TurnOutcome::Created { .. }))
            .count();
        let replay = [left, right]
            .into_iter()
            .filter(|outcome| matches!(outcome, TurnOutcome::ExactReplay { .. }))
            .count();

        assert_eq!(created, 1);
        assert_eq!(replay, 1);
    }

    #[tokio::test]
    async fn claim_establishment_classifies_before_and_after_commit_cuts() {
        let repo = repo().await;
        let created = repo
            .accept_authoritative_turn(&input("conv-a", "claim-cuts", 7))
            .await
            .unwrap();
        let TurnOutcome::Created { turn_id, .. } = created.outcome else {
            panic!("expected created turn")
        };
        let workflow_id = repo.workflow_id_for_turn(turn_id).await.unwrap().unwrap();
        let claim = claim_input(workflow_id, turn_id, 20);

        assert!(matches!(
            repo.establish_authoritative_turn_claim_at_cut(&claim, TransactionCut::BeforeCommit)
                .await,
            ClaimAuthoritativeTurnEstablishment::KnownNotCommitted(_)
        ));
        assert!(repo
            .list_attempts(workflow_id, EffectId(DIRECT_TURN_EFFECT_ID))
            .await
            .unwrap()
            .is_empty());

        let established = repo
            .establish_authoritative_turn_claim_at_cut(&claim, TransactionCut::AfterCommit)
            .await;
        let ClaimAuthoritativeTurnEstablishment::Established(established) = established else {
            panic!("after-commit acknowledgement loss must adopt exact claim")
        };
        assert_eq!(established.outcome, ClaimOutcome::Started);
        let attempts = repo
            .list_attempts(workflow_id, EffectId(DIRECT_TURN_EFFECT_ID))
            .await
            .unwrap();
        assert_eq!(attempts.len(), 1);
        assert_eq!(attempts[0].authority, established.authority.unwrap());
    }

    #[tokio::test]
    async fn claim_contention_and_expired_reclaim_are_deterministic() {
        let repo = repo().await;
        let created = repo
            .accept_authoritative_turn(&input("conv-a", "claim", 7))
            .await
            .unwrap();
        let TurnOutcome::Created { turn_id, .. } = created.outcome else {
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
        let TurnOutcome::Created { turn_id, .. } = created.outcome else {
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
        let TurnOutcome::Created { turn_id, .. } = created.outcome else {
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

        assert!(matches!(
            repo.materialize_authoritative_turn(&materialize_input(
                turn_id,
                authority.clone(),
                1,
                1,
                "message-conv-a-preflight",
                70,
            ))
            .await,
            crate::workflow::LocalAuthorityResult::DurableFactEstablished(
                MaterializeAuthoritativeTurnOutcome::Materialized(_)
            )
        ));
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
        let TurnOutcome::Created { turn_id, .. } = created.outcome else {
            panic!("expected created turn")
        };
        let workflow_id = repo.workflow_id_for_turn(turn_id).await.unwrap().unwrap();
        let claim = repo
            .claim_authoritative_turn(&claim_input(workflow_id, turn_id, 50))
            .await
            .unwrap();
        let authority = claim.authority.unwrap();
        assert!(matches!(
            repo.materialize_authoritative_turn(&materialize_input(
                turn_id,
                authority.clone(),
                10,
                10,
                "message-conv-a-materialize-phase3",
                50,
            ))
            .await,
            crate::workflow::LocalAuthorityResult::DurableFactEstablished(
                MaterializeAuthoritativeTurnOutcome::Materialized(_)
            )
        ));
        let replay = repo
            .materialize_authoritative_turn(&materialize_input(
                turn_id,
                authority.clone(),
                11,
                11,
                "message-conv-a-materialize-phase3",
                50,
            ))
            .await;
        assert!(
            matches!(
                replay,
                crate::workflow::LocalAuthorityResult::DurableFactEstablished(
                    MaterializeAuthoritativeTurnOutcome::ExactReplay(_)
                )
            ),
            "unexpected replay outcome: {replay:?}"
        );
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
        assert!(matches!(
            conflict,
            crate::workflow::LocalAuthorityResult::DurableFactEstablished(
                MaterializeAuthoritativeTurnOutcome::CommandRejected(
                    TurnConflict::PreparedSemanticsChanged { .. }
                )
            )
        ));

        let terminal_created = repo
            .accept_authoritative_turn(&input("conv-d", "steering-terminal", 12))
            .await
            .unwrap();
        let TurnOutcome::Created { turn_id, .. } = terminal_created.outcome else {
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
        assert!(
            matches!(
                terminal,
                crate::workflow::LocalAuthorityResult::DurableFactEstablished(
                    MaterializeAuthoritativeTurnOutcome::CommandRejected(
                        TurnConflict::PreparedSemanticsChanged { .. }
                    )
                )
            ),
            "unexpected terminal outcome: {terminal:?}"
        );
    }
    #[tokio::test]
    async fn materialization_scopes_canonical_internal_message_id_by_conversation() {
        let repo = repo().await;
        let input_a = input("conv-a-scope", "same-key", 33);
        let input_b = input("conv-b-scope", "same-key", 34);
        let step_a = repo.accept_authoritative_turn(&input_a).await.unwrap();
        let step_b = repo.accept_authoritative_turn(&input_b).await.unwrap();
        let TurnOutcome::Created {
            turn_id: turn_a, ..
        } = step_a.outcome
        else {
            panic!("expected created turn a")
        };
        let TurnOutcome::Created {
            turn_id: turn_b, ..
        } = step_b.outcome
        else {
            panic!("expected created turn b")
        };
        let workflow_a = repo.workflow_id_for_turn(turn_a).await.unwrap().unwrap();
        let workflow_b = repo.workflow_id_for_turn(turn_b).await.unwrap().unwrap();
        let authority_a = repo
            .claim_authoritative_turn(&claim_input(workflow_a, turn_a, 70))
            .await
            .unwrap()
            .authority
            .unwrap();
        let authority_b = repo
            .claim_authoritative_turn(&claim_input(workflow_b, turn_b, 71))
            .await
            .unwrap()
            .authority
            .unwrap();

        let crate::workflow::LocalAuthorityResult::DurableFactEstablished(
            MaterializeAuthoritativeTurnOutcome::Materialized(materialization_a),
        ) = repo
            .materialize_authoritative_turn(&materialize_input(
                turn_a,
                authority_a,
                70,
                70,
                "message-conv-a-scope-same-key",
                70,
            ))
            .await
        else {
            panic!("expected materialized turn a")
        };
        let message_a = materialization_a.message;
        let crate::workflow::LocalAuthorityResult::DurableFactEstablished(
            MaterializeAuthoritativeTurnOutcome::Materialized(materialization_b),
        ) = repo
            .materialize_authoritative_turn(&materialize_input(
                turn_b,
                authority_b,
                71,
                71,
                "message-conv-b-scope-same-key",
                71,
            ))
            .await
        else {
            panic!("expected materialized turn b")
        };
        let message_b = materialization_b.message;

        assert_eq!(
            message_a.message_id,
            canonical_message_id("conv-a-scope", "message-conv-a-scope-same-key")
        );
        assert_eq!(
            message_b.message_id,
            canonical_message_id("conv-b-scope", "message-conv-b-scope-same-key")
        );
        assert_ne!(message_a.message_id, message_b.message_id);
    }

    #[tokio::test]
    async fn prepared_turn_attachments_round_trip_via_normalized_tables() {
        let repo = repo().await;
        let conversation = ConversationAuthority("conv-a".to_string());
        let payload = prepared_payload_with_attachments("message-conv-a-attachments");
        let prepared =
            PreparedTurn::from_exact_payload(&conversation, payload.to_exact_bytes().unwrap());
        let input = AcceptAuthoritativeTurn {
            client_key: ClientTurnKey::new("attachments").unwrap(),
            prepared: prepared.clone(),
            disposition: AcceptedDisposition::Runtime,
            accepted_at: Timestamp(77),
        };
        let created = repo.accept_authoritative_turn(&input).await.unwrap();
        let TurnOutcome::Created { turn_id, .. } = created.outcome else {
            panic!("expected created turn")
        };

        let stored_payload: Vec<u8> =
            sqlx::query_scalar("SELECT prepared_payload FROM durable_turns WHERE turn_id = ?1")
                .bind(i64::try_from(turn_id.0).unwrap())
                .fetch_one(&repo.pool)
                .await
                .unwrap();
        let stored_json: serde_json::Value = serde_json::from_slice(&stored_payload).unwrap();
        assert!(stored_json["submitted"].get("images").is_none());
        assert!(stored_json["submitted"].get("files").is_none());
        assert!(stored_json["delivery"].get("images").is_none());
        assert!(stored_json["delivery"].get("files").is_none());

        let loaded = repo
            .load_authoritative_turn(turn_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded.prepared, prepared);

        let submitted_images: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM durable_turn_submitted_images WHERE turn_id = ?1",
        )
        .bind(i64::try_from(turn_id.0).unwrap())
        .fetch_one(&repo.pool)
        .await
        .unwrap();
        let submitted_files: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM durable_turn_submitted_files WHERE turn_id = ?1",
        )
        .bind(i64::try_from(turn_id.0).unwrap())
        .fetch_one(&repo.pool)
        .await
        .unwrap();
        let delivery_images: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM durable_turn_delivery_images WHERE turn_id = ?1",
        )
        .bind(i64::try_from(turn_id.0).unwrap())
        .fetch_one(&repo.pool)
        .await
        .unwrap();
        let delivery_files: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM durable_turn_delivery_files WHERE turn_id = ?1",
        )
        .bind(i64::try_from(turn_id.0).unwrap())
        .fetch_one(&repo.pool)
        .await
        .unwrap();
        assert_eq!(
            (
                submitted_images,
                submitted_files,
                delivery_images,
                delivery_files
            ),
            (1, 1, 1, 1)
        );
    }

    #[tokio::test]
    async fn legacy_embedded_attachments_survive_normalized_schema_upgrade() {
        let repo = repo().await;
        let conversation = ConversationAuthority("conv-a".to_string());
        let payload = prepared_payload_with_attachments("message-conv-a-legacy-attachments");
        let prepared =
            PreparedTurn::from_exact_payload(&conversation, payload.to_exact_bytes().unwrap());
        let created = repo
            .accept_authoritative_turn(&AcceptAuthoritativeTurn {
                client_key: ClientTurnKey::new("legacy-attachments").unwrap(),
                prepared: prepared.clone(),
                disposition: AcceptedDisposition::Runtime,
                accepted_at: Timestamp(78),
            })
            .await
            .unwrap();
        let TurnOutcome::Created { turn_id, .. } = created.outcome else {
            panic!("expected created turn")
        };

        let turn_id = i64::try_from(turn_id.0).unwrap();
        sqlx::query("DELETE FROM durable_turn_submitted_images WHERE turn_id = ?1")
            .bind(turn_id)
            .execute(&repo.pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM durable_turn_submitted_files WHERE turn_id = ?1")
            .bind(turn_id)
            .execute(&repo.pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM durable_turn_delivery_images WHERE turn_id = ?1")
            .bind(turn_id)
            .execute(&repo.pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM durable_turn_delivery_files WHERE turn_id = ?1")
            .bind(turn_id)
            .execute(&repo.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE durable_turns SET prepared_payload = ?1 WHERE turn_id = ?2")
            .bind(payload.to_exact_bytes().unwrap())
            .bind(turn_id)
            .execute(&repo.pool)
            .await
            .unwrap();

        let loaded = repo
            .load_authoritative_turn(TurnAuthorityId(u64::try_from(turn_id).unwrap()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded.prepared, prepared);
    }

    #[tokio::test]
    async fn materialize_rehydrates_normalized_delivery_attachments_into_message() {
        let repo = repo().await;
        let conversation = ConversationAuthority("conv-a".to_string());
        let payload = prepared_payload_with_attachments("message-conv-a-materialized-attachments");
        let prepared =
            PreparedTurn::from_exact_payload(&conversation, payload.to_exact_bytes().unwrap());
        let created = repo
            .accept_authoritative_turn(&AcceptAuthoritativeTurn {
                client_key: ClientTurnKey::new("materialized-attachments").unwrap(),
                prepared,
                disposition: AcceptedDisposition::Runtime,
                accepted_at: Timestamp(80),
            })
            .await
            .unwrap();
        let TurnOutcome::Created { turn_id, .. } = created.outcome else {
            panic!("expected created turn")
        };
        let workflow_id = repo.workflow_id_for_turn(turn_id).await.unwrap().unwrap();
        let claim = repo
            .claim_authoritative_turn(&claim_input(workflow_id, turn_id, 81))
            .await
            .unwrap();
        let crate::workflow::LocalAuthorityResult::DurableFactEstablished(
            MaterializeAuthoritativeTurnOutcome::Materialized(materialization),
        ) = repo
            .materialize_authoritative_turn(&MaterializeAuthoritativeTurnInput {
                turn_id,
                authority: claim.authority.unwrap(),
                prepared: payload.clone(),
                sequence_id: 81,
                created_at: Timestamp(81),
                accepted_state: ConvState::LlmRequesting { attempt: 1 },
                state_updated_at: timestamp_to_datetime(Timestamp(81)),
                now: Timestamp(81),
            })
            .await
        else {
            panic!("expected materialized attachment turn")
        };
        let message = materialization.message;
        let (images, files) = message.content.attachments();
        assert_eq!(images, payload.delivery.images);
        assert_eq!(files, payload.delivery.files);
    }

    #[tokio::test]
    async fn scoped_replay_rehydrates_normalized_submitted_attachments() {
        let repo = repo().await;
        let conversation = ConversationAuthority("conv-replay".to_string());
        let payload = prepared_payload_with_attachments("message-conv-replay-attachments");
        let prepared =
            PreparedTurn::from_exact_payload(&conversation, payload.to_exact_bytes().unwrap());
        let input = AcceptAuthoritativeTurn {
            client_key: ClientTurnKey::new("replay-attachments").unwrap(),
            prepared,
            disposition: AcceptedDisposition::Runtime,
            accepted_at: Timestamp(90),
        };
        repo.accept_authoritative_turn(&input).await.unwrap();
        let replay = repo
            .lookup_scoped_direct_turn_replay(&conversation, &input.client_key, &payload.submitted)
            .await
            .unwrap();
        let ScopedDirectTurnReplayLookup::Exact { prepared, .. } = replay else {
            panic!("expected exact replay")
        };
        assert_eq!(*prepared, payload);
    }

    #[tokio::test]
    async fn steering_accept_rehydrates_active_owner_before_commit() {
        let repo = repo().await;
        let runtime = input("conv-a", "runtime-owner", 80);
        repo.accept_authoritative_turn(&runtime).await.unwrap();
        sqlx::query(
            "UPDATE durable_turns SET prepared_fingerprint = 'corrupt-owner' WHERE conversation_id = ?1 AND client_turn_key = ?2",
        )
        .bind("conv-a")
        .bind("runtime-owner")
        .execute(&repo.pool)
        .await
        .unwrap();

        let steering = input_with_disposition(
            "conv-a",
            "steering-corrupt-owner",
            81,
            AcceptedDisposition::Steering,
        );
        let err = repo.accept_authoritative_turn(&steering).await.unwrap_err();
        assert!(matches!(
            err,
            DbError::DirectTurnConflict(TurnConflict::CorruptAggregate(_))
        ));
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM durable_turns WHERE client_turn_key = 'steering-corrupt-owner'"
            )
            .fetch_one(&repo.pool)
            .await
            .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn scoped_replay_returns_exact_before_expansion_changes() {
        let repo = repo().await;
        let input = input("conv-replay", "client", 31);
        repo.accept_authoritative_turn(&input).await.unwrap();
        let mut changed_delivery =
            PreparedDirectTurnPayload::from_exact_bytes(input.prepared.payload()).unwrap();
        changed_delivery.delivery.llm_text = Some("changed expansion".to_string());
        assert!(matches!(
            repo.lookup_scoped_direct_turn_replay(
                input.conversation(),
                &input.client_key,
                &changed_delivery.submitted,
            )
            .await
            .unwrap(),
            ScopedDirectTurnReplayLookup::Exact { .. }
        ));
    }

    #[tokio::test]
    async fn scoped_replay_reports_submitted_identity_changed() {
        let repo = repo().await;
        let input = input("conv-replay", "conflict", 32);
        repo.accept_authoritative_turn(&input).await.unwrap();
        let mut submitted = PreparedDirectTurnPayload::from_exact_bytes(input.prepared.payload())
            .unwrap()
            .submitted;
        submitted.text.push_str(" changed");
        let err = repo
            .lookup_scoped_direct_turn_replay(input.conversation(), &input.client_key, &submitted)
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            ScopedDirectTurnReplayError::SubmittedIdentityChanged { .. }
        ));
    }

    #[tokio::test]
    async fn scoped_replay_client_key_is_conversation_scoped() {
        let repo = repo().await;
        let input_a = input("conv-a-scope", "same-key", 33);
        let input_b = input("conv-b-scope", "same-key", 34);
        repo.accept_authoritative_turn(&input_a).await.unwrap();
        repo.accept_authoritative_turn(&input_b).await.unwrap();
        let submitted_a = PreparedDirectTurnPayload::from_exact_bytes(input_a.prepared.payload())
            .unwrap()
            .submitted;
        let submitted_b = PreparedDirectTurnPayload::from_exact_bytes(input_b.prepared.payload())
            .unwrap()
            .submitted;
        assert!(matches!(
            repo.lookup_scoped_direct_turn_replay(
                input_a.conversation(),
                &input_a.client_key,
                &submitted_a
            )
            .await
            .unwrap(),
            ScopedDirectTurnReplayLookup::Exact { .. }
        ));
        assert!(matches!(
            repo.lookup_scoped_direct_turn_replay(
                input_b.conversation(),
                &input_b.client_key,
                &submitted_b
            )
            .await
            .unwrap(),
            ScopedDirectTurnReplayLookup::Exact { .. }
        ));
        assert!(matches!(
            repo.lookup_scoped_direct_turn_replay(
                input_a.conversation(),
                &input_a.client_key,
                &submitted_b
            )
            .await
            .unwrap_err(),
            ScopedDirectTurnReplayError::SubmittedIdentityChanged { .. }
        ));
    }

    #[tokio::test]
    async fn scoped_replay_rejects_corrupt_persisted_fingerprint() {
        let repo = repo().await;
        let input = input("conv-replay", "corrupt", 35);
        repo.accept_authoritative_turn(&input).await.unwrap();
        sqlx::query("UPDATE durable_turns SET prepared_fingerprint = 'not-the-exact-fingerprint' WHERE conversation_id = ?1 AND client_turn_key = ?2")
            .bind(&input.conversation().0)
            .bind(input.client_key.as_str())
            .execute(&repo.pool)
            .await
            .unwrap();
        let submitted = PreparedDirectTurnPayload::from_exact_bytes(input.prepared.payload())
            .unwrap()
            .submitted;
        let err = repo
            .lookup_scoped_direct_turn_replay(input.conversation(), &input.client_key, &submitted)
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            ScopedDirectTurnReplayError::Db(DbError::DirectTurnConflict(
                TurnConflict::CorruptAggregate(_)
            ))
        ));
    }
}
