use super::super::{
    direct_turn_profile, finish_workflow_transaction_at_cut, local_codec_owned,
    next_workflow_sequence_tx, row_to_turn_tx, to_i64, to_u64, AuthorityOutcome, ConvState,
    ConversationAuthority, DateTime, DbError, DbResult, DeliveryId, DeliveryResolutionDecision,
    DeliveryResolutionPlan, DirectTurnMaterializationEligibility, GoalStartReceiptId,
    LocalMaterializationCheck, Materialization, PreparedTurn, ReceiptId, Row, Timestamp,
    TransactionCut, TurnAuthorityId, TurnLifecycle, Utc, WorkflowRepository, WorkflowStatus,
    DIRECT_TURN_MATERIALIZED_TRANSITION_ID,
};
use phoenix_core::domain::conversation_goal::{
    GoalRequestCause, GoalRequestId, GoalRequestIntent, GoalStartReceipt,
};

#[derive(Debug, Clone)]
pub struct GoalStartAdoptionInput {
    pub authority: crate::workflow::LocalAttemptAuthority,
    pub start_id: GoalStartReceiptId,
    pub receipt: GoalStartReceipt,
    pub state_updated_at: DateTime<Utc>,
    pub now: Timestamp,
}

impl GoalStartAdoptionInput {
    pub fn intent(&self) -> DbResult<GoalRequestIntent> {
        GoalRequestIntent::new(
            GoalRequestId::parse(format!("goal-start:{}", self.start_id.get()))
                .map_err(|error| DbError::Serialization(error.to_string()))?,
            self.receipt.input().goal().clone(),
            self.receipt.execution(),
            to_i64(self.authority.generation.0, "generation")?,
            GoalRequestCause::Initial,
        )
        .map_err(|error| DbError::Serialization(error.to_string()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GoalStartAdoptionOutcome {
    Committed {
        turn_id: TurnAuthorityId,
        generation: u64,
    },
    ClassifiedCommitted {
        turn_id: TurnAuthorityId,
        generation: u64,
    },
    CommittedElsewhere,
    ExactReplay,
    NotCommitted,
    StaleAuthority,
    Rejected,
}

impl WorkflowRepository {
    pub async fn goal_start_id_for_receipt(
        &self,
        receipt: &GoalStartReceipt,
    ) -> DbResult<Option<GoalStartReceiptId>> {
        let id: Option<i64> = sqlx::query_scalar(
            "SELECT start_id FROM conversation_goal_starts WHERE owner_id=? AND request_id=?",
        )
        .bind(receipt.input().goal().owner.as_str())
        .bind(receipt.input().request_id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        id.map(|id| {
            GoalStartReceiptId::new(to_u64(id, "Start receipt")?)
                .ok_or_else(|| DbError::Serialization("Start receipt id out of range".into()))
        })
        .transpose()
    }

    async fn preflight_goal_start_tx(
        &self,
        tx: &mut crate::workflow::WorkflowTx<'_>,
        input: &GoalStartAdoptionInput,
    ) -> DbResult<DirectTurnMaterializationEligibility> {
        let row = sqlx::query("SELECT * FROM durable_turns WHERE turn_id=? AND workflow_id=?")
            .bind(input.receipt.execution().turn_id())
            .bind(to_i64(input.authority.workflow_id.0, "workflow_id")?)
            .fetch_optional(&mut *tx.tx)
            .await?;
        let Some(row) = row else {
            return Ok(DirectTurnMaterializationEligibility::StaleAuthority);
        };
        let turn = row_to_turn_tx(&mut tx.tx, row).await?;
        let expected = PreparedTurn::from_goal_start(
            &ConversationAuthority(input.receipt.historical_transcript().as_str().to_owned()),
            input.start_id,
        );
        if turn.prepared.fingerprint() != expected.fingerprint()
            || turn.conversation != *expected.target()
        {
            return Err(DbError::Serialization(
                "Start input/fingerprint differs".into(),
            ));
        }
        if !matches!(turn.lifecycle, TurnLifecycle::Accepted { .. })
            || turn.generation != input.authority.generation.0
        {
            return Ok(DirectTurnMaterializationEligibility::StaleAuthority);
        }
        match turn.materialization {
            Materialization::MaterializedGoalStart { receipt } if receipt == input.start_id => {
                return Ok(DirectTurnMaterializationEligibility::ExactReplay)
            }
            Materialization::UnmaterializedGoalStart { receipt } if receipt == input.start_id => {}
            Materialization::Unmaterialized
            | Materialization::Materialized { .. }
            | Materialization::UnmaterializedGoalStart { .. }
            | Materialization::MaterializedGoalStart { .. } => {
                return Err(DbError::Serialization(
                    "Start input kind/identity differs".into(),
                ));
            }
        }
        Self::preflight_local_materialization_authority_tx(
            tx,
            &LocalMaterializationCheck {
                authority: &input.authority,
                now: input.now,
            },
        )
        .await
    }

    pub async fn materialize_goal_start(
        &self,
        input: &GoalStartAdoptionInput,
    ) -> super::super::super::LocalAuthorityResult<GoalStartAdoptionOutcome> {
        self.materialize_goal_start_at_cut(input, TransactionCut::None)
            .await
    }

    #[cfg(test)]
    pub(crate) async fn materialize_goal_start_test_cut(
        &self,
        input: &GoalStartAdoptionInput,
        after_commit: bool,
    ) -> super::super::super::LocalAuthorityResult<GoalStartAdoptionOutcome> {
        self.materialize_goal_start_at_cut(
            input,
            if after_commit {
                TransactionCut::AfterCommit
            } else {
                TransactionCut::BeforeCommit
            },
        )
        .await
    }

    async fn materialize_goal_start_at_cut(
        &self,
        input: &GoalStartAdoptionInput,
        cut: TransactionCut,
    ) -> super::super::super::LocalAuthorityResult<GoalStartAdoptionOutcome> {
        use super::super::super::LocalAuthorityResult;
        match self.materialize_goal_start_command(input, cut).await {
            Ok(outcome) => LocalAuthorityResult::DurableFactEstablished(outcome),
            Err(error) => {
                tracing::warn!(%error, start_id=input.start_id.get(),
                    "Start materialization requires durable full-bundle classification");
                match self.classify_goal_start_materialization(input).await {
                    Ok(outcome) => LocalAuthorityResult::DurableFactEstablished(outcome),
                    Err(error) => {
                        tracing::error!(%error, start_id=input.start_id.get(),
                            "Start materialization remains unclassified");
                        LocalAuthorityResult::DurableFactUnclassified
                    }
                }
            }
        }
    }

    async fn materialize_goal_start_command(
        &self,
        input: &GoalStartAdoptionInput,
        cut: TransactionCut,
    ) -> DbResult<GoalStartAdoptionOutcome> {
        if self.goal_execution_installation
            == crate::conversation_goals::execution::GoalExecutionInstallation::Disabled
            || self.goal_control_installation
                == crate::conversation_goals::control::GoalControlInstallation::Disabled
        {
            return Ok(GoalStartAdoptionOutcome::Rejected);
        }
        let mut tx = self.begin_immediate_tx().await?;
        let retained =
            crate::conversation_goals::start::retained_start_tx(&mut tx.tx, input.receipt.input())
                .await
                .map_err(|error| DbError::Serialization(error.to_string()))?;
        if retained.as_ref() != Some(&input.receipt) {
            return Err(DbError::Serialization(
                "Start acceptance differs before materialization".into(),
            ));
        }
        let preflight = self.preflight_goal_start_tx(&mut tx, input).await?;
        match preflight {
            DirectTurnMaterializationEligibility::Fresh => {}
            DirectTurnMaterializationEligibility::ExactReplay => {
                return Ok(GoalStartAdoptionOutcome::ExactReplay)
            }
            DirectTurnMaterializationEligibility::StaleAuthority => {
                return Ok(GoalStartAdoptionOutcome::StaleAuthority)
            }
        }
        let eligible: i64 = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM conversations c JOIN product_conversations p ON p.id=c.product_conversation_id JOIN conversation_goal_controls g ON g.owner_id=p.id JOIN conversation_goal_control_bindings b ON b.turn_id=? WHERE c.id=? AND c.state_kind='idle' AND p.kind='ordinary' AND p.closed_at IS NULL AND p.latest_transcript_id=c.id AND g.stopped=0 AND b.control_version=g.control_version AND NOT EXISTS(SELECT 1 FROM steering_messages q WHERE q.conversation_id=c.id))")
            .bind(input.receipt.execution().turn_id()).bind(input.receipt.historical_transcript().as_str())
            .fetch_one(&mut *tx.tx).await?;
        if eligible != 1 {
            return Ok(GoalStartAdoptionOutcome::Rejected);
        }
        let intent = input.intent()?;
        let accepted_state = ConvState::LlmRequesting { attempt: 1 };
        let changed = sqlx::query("UPDATE durable_turns SET materialized=1,updated_at=? WHERE turn_id=? AND input_kind='goal_start' AND materialized=0 AND goal_start_id=? AND owns_conversation=1 AND terminal_kind IS NULL AND generation=?")
            .bind(to_i64(input.now.0, "updated_at")?).bind(intent.execution.turn_id())
            .bind(to_i64(input.start_id.get(), "Start receipt")?).bind(intent.generation())
            .execute(&mut *tx.tx).await?.rows_affected();
        if changed == 0 {
            return Ok(GoalStartAdoptionOutcome::ExactReplay);
        }
        let canonical = direct_turn_profile::CanonicalTurnReceipt::GoalStart(
            direct_turn_profile::GoalStartTurnReceipt {
                turn_id: to_u64(intent.execution.turn_id(), "turn_id")?,
                goal_start_receipt_id: input.start_id,
            },
        );
        let bytes = serde_json::to_vec(&canonical)
            .map_err(|error| DbError::Serialization(error.to_string()))?;
        let event = direct_turn_profile::DirectTurnReceiptEvent::GoalStartMaterialized {
            receipt: input.start_id,
        };
        let receipt_id = ReceiptId(
            next_workflow_sequence_tx(&mut tx, input.authority.workflow_id, "receipt").await?,
        );
        let delivery_id = DeliveryId(
            next_workflow_sequence_tx(&mut tx, input.authority.workflow_id, "delivery").await?,
        );
        let accepted = tx
            .accept_receipt_and_delivery(&crate::workflow::AcceptReceiptInput {
                authority: input.authority.clone(),
                receipt_id,
                delivery_id,
                attempt_id: Some(input.authority.attempt_id),
                origin: phoenix_workflow::ReceiptOrigin::Execution,
                receipt_codec: local_codec_owned(&direct_turn_profile::receipt_codec()),
                receipt_payload: bytes,
                receipt_event_codec: local_codec_owned(&direct_turn_profile::receipt_event_codec()),
                receipt_event_payload: serde_json::to_vec(&event)
                    .map_err(|error| DbError::Serialization(error.to_string()))?,
                receipt_event_requires_runtime_acceptance: true,
                request_runtime_acceptance_for_cancellation: false,
            })
            .await?;
        if accepted.outcome != AuthorityOutcome::Authorized {
            return Err(DbError::Serialization(
                "Start receipt authority changed".into(),
            ));
        }
        let delivered = direct_turn_profile::DirectTurnEvent::Delivered(event);
        let event_payload = serde_json::to_vec(&delivered)
            .map_err(|error| DbError::Serialization(error.to_string()))?;
        let snapshot_payload = serde_json::to_vec(&direct_turn_profile::DirectTurnSnapshot {
            turn_id: to_u64(intent.execution.turn_id(), "turn_id")?,
        })
        .map_err(|error| DbError::Serialization(error.to_string()))?;
        let event_codec = local_codec_owned(&direct_turn_profile::event_codec());
        let snapshot_codec = local_codec_owned(&direct_turn_profile::snapshot_codec());
        let committed = tx
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
        if committed != phoenix_workflow::CommitOutcome::Committed {
            return Err(DbError::Serialization(
                "Start workflow transition rejected".into(),
            ));
        }
        crate::conversation_goals::execution::append_request_tx(
            &mut tx.tx,
            self.goal_execution_installation,
            &intent,
        )
        .await
        .map_err(|error| DbError::Serialization(error.to_string()))?;
        let state_change = sqlx::query("UPDATE conversations SET state=?,state_kind=?,state_updated_at=?,updated_at=? WHERE id=? AND state_kind='idle'")
            .bind(serde_json::to_string(&accepted_state).map_err(|error| DbError::Serialization(error.to_string()))?)
            .bind(crate::conv_state_kind(&accepted_state)).bind(input.state_updated_at.to_rfc3339())
            .bind(input.state_updated_at.to_rfc3339())
            .bind(input.receipt.historical_transcript().as_str()).execute(&mut *tx.tx).await?;
        if state_change.rows_affected() != 1 {
            return Err(DbError::Serialization(
                "Start proposed request state was not installed".into(),
            ));
        }
        sqlx::query("INSERT INTO conversation_goal_start_materializations (start_id,request_id,effect_id,attempt_id,process_incarnation,state_updated_at) VALUES (?,?,?,?,?,?)")
            .bind(to_i64(input.start_id.get(), "Start receipt")?).bind(intent.id.as_str())
            .bind(to_i64(input.authority.effect_id.0, "effect_id")?)
            .bind(to_i64(input.authority.attempt_id.0, "attempt_id")?)
            .bind(to_i64(input.authority.process_incarnation.0, "process_incarnation")?)
            .bind(input.state_updated_at.to_rfc3339()).execute(&mut *tx.tx).await?;
        finish_workflow_transaction_at_cut(tx, cut).await?;
        Ok(GoalStartAdoptionOutcome::Committed {
            turn_id: TurnAuthorityId(to_u64(intent.execution.turn_id(), "turn_id")?),
            generation: input.authority.generation.0,
        })
    }

    async fn goal_start_workflow_head_tx(
        tx: &mut crate::workflow::WorkflowTx<'_>,
        input: &GoalStartAdoptionInput,
    ) -> DbResult<sqlx::sqlite::SqliteRow> {
        let head = sqlx::query("SELECT profile_kind,profile_version,status,version,generation,snapshot_codec_family,snapshot_codec_version,snapshot_payload FROM workflows WHERE workflow_id=?")
            .bind(input.receipt.execution().workflow_id()).fetch_one(&mut *tx.tx).await?;
        let snapshot = serde_json::to_vec(&direct_turn_profile::DirectTurnSnapshot {
            turn_id: to_u64(input.receipt.execution().turn_id(), "turn_id")?,
        })
        .map_err(|error| DbError::Serialization(error.to_string()))?;
        if head.get::<String, _>("profile_kind") != direct_turn_profile::PROFILE_ID
            || head.get::<i64, _>("profile_version")
                != i64::from(direct_turn_profile::PROTOCOL_VERSION)
            || head.get::<String, _>("snapshot_codec_family")
                != direct_turn_profile::SNAPSHOT_CODEC_FAMILY
            || head.get::<i64, _>("snapshot_codec_version")
                != i64::from(direct_turn_profile::PROTOCOL_VERSION)
            || head.get::<Vec<u8>, _>("snapshot_payload") != snapshot
        {
            return Err(DbError::Serialization(
                "Start workflow head profile/snapshot diverged".into(),
            ));
        }
        Ok(head)
    }

    pub(crate) async fn classify_goal_start_materialization(
        &self,
        input: &GoalStartAdoptionInput,
    ) -> DbResult<GoalStartAdoptionOutcome> {
        let mut tx = self.begin_tx().await?;
        let retained =
            crate::conversation_goals::start::retained_start_tx(&mut tx.tx, input.receipt.input())
                .await
                .map_err(|error| DbError::Serialization(error.to_string()))?
                .ok_or_else(|| {
                    DbError::Serialization("Start acceptance missing during classification".into())
                })?;
        // A live link may detach after admission; immutable facts must still be exact.
        if retained.input() != input.receipt.input()
            || retained.execution() != input.receipt.execution()
            || retained.historical_transcript().as_str()
                != input.receipt.historical_transcript().as_str()
        {
            return Err(DbError::Serialization(
                "Start history conflicts during classification".into(),
            ));
        }
        let actual_start: i64 = sqlx::query_scalar(
            "SELECT start_id FROM conversation_goal_starts WHERE owner_id=? AND request_id=?",
        )
        .bind(input.receipt.input().goal().owner.as_str())
        .bind(input.receipt.input().request_id.to_string())
        .fetch_one(&mut *tx.tx)
        .await?;
        if actual_start != to_i64(input.start_id.get(), "Start receipt")? {
            return Err(DbError::Serialization(
                "Start receipt identity conflicts during classification".into(),
            ));
        }
        let intent = input.intent()?;
        let witness = sqlx::query("SELECT request_id,effect_id,attempt_id,process_incarnation,state_updated_at FROM conversation_goal_start_materializations WHERE start_id=?")
            .bind(to_i64(input.start_id.get(), "Start receipt")?).fetch_optional(&mut *tx.tx).await?;
        let live = sqlx::query("SELECT t.*,c.state AS conversation_state,c.state_updated_at AS conversation_updated_at FROM durable_turns t JOIN conversations c ON c.id=t.conversation_id WHERE t.turn_id=?")
            .bind(intent.execution.turn_id()).fetch_optional(&mut *tx.tx).await?;
        if let Some(witness) = witness {
            if witness.get::<String, _>("request_id") != intent.id.as_str() {
                return Err(DbError::Serialization(
                    "Start materialization request conflicts".into(),
                ));
            }
            let request = crate::conversation_goals::execution::retained_request_tx(
                &mut tx.tx,
                self.goal_execution_installation,
                &intent,
            )
            .await
            .map_err(|error| DbError::Serialization(error.to_string()))?
            .ok_or_else(|| {
                DbError::Serialization("Start witness has no exact request intent".into())
            })?;
            let same_authority = witness.get::<i64, _>("effect_id")
                == to_i64(input.authority.effect_id.0, "effect_id")?
                && witness.get::<i64, _>("attempt_id")
                    == to_i64(input.authority.attempt_id.0, "attempt_id")?
                && witness.get::<i64, _>("process_incarnation")
                    == to_i64(input.authority.process_incarnation.0, "incarnation")?
                && witness.get::<String, _>("state_updated_at")
                    == input.state_updated_at.to_rfc3339();
            let Some(live) = live else {
                return Ok(GoalStartAdoptionOutcome::CommittedElsewhere);
            };
            let proof = sqlx::query("SELECT r.receipt_payload,r.receipt_codec_family,r.receipt_codec_version,r.declared_workflow_version,r.generation,r.process_incarnation,r.origin,d.payload_kind,d.consumer_kind,d.requires_runtime_acceptance,d.suppression_reason,d.payload_blob,d.event_codec_family AS delivery_codec_family,d.event_codec_version AS delivery_codec_version,d.status,d.runtime_acceptance_status,t.event_codec_family AS transition_codec_family,t.event_codec_version AS transition_codec_version,t.event_payload,t.from_version,t.to_version,t.generation AS transition_generation FROM workflow_receipts r JOIN workflow_deliveries d ON d.workflow_id=r.workflow_id AND d.effect_id=r.effect_id AND d.accepted_by_transition_id=? JOIN workflow_transitions t ON t.workflow_id=r.workflow_id AND t.transition_id=d.accepted_by_transition_id WHERE r.workflow_id=? AND r.effect_id=? AND r.attempt_id=?")
                .bind(to_i64(DIRECT_TURN_MATERIALIZED_TRANSITION_ID,"transition_id")?)
                .bind(intent.execution.workflow_id()).bind(witness.get::<i64,_>("effect_id"))
                .bind(witness.get::<i64,_>("attempt_id")).fetch_one(&mut *tx.tx).await?;
            let canonical: direct_turn_profile::CanonicalTurnReceipt =
                serde_json::from_slice(&proof.get::<Vec<u8>, _>("receipt_payload"))
                    .map_err(|error| DbError::Serialization(error.to_string()))?;
            let expected = direct_turn_profile::CanonicalTurnReceipt::GoalStart(
                direct_turn_profile::GoalStartTurnReceipt {
                    turn_id: to_u64(intent.execution.turn_id(), "turn_id")?,
                    goal_start_receipt_id: input.start_id,
                },
            );
            let delivery: direct_turn_profile::DirectTurnReceiptEvent =
                serde_json::from_slice(&proof.get::<Vec<u8>, _>("payload_blob"))
                    .map_err(|error| DbError::Serialization(error.to_string()))?;
            let transition: direct_turn_profile::DirectTurnEvent =
                serde_json::from_slice(&proof.get::<Vec<u8>, _>("event_payload"))
                    .map_err(|error| DbError::Serialization(error.to_string()))?;
            let expected_delivery =
                direct_turn_profile::DirectTurnReceiptEvent::GoalStartMaterialized {
                    receipt: input.start_id,
                };
            let declared = proof.get::<i64, _>("declared_workflow_version");
            if canonical != expected
                || delivery != expected_delivery
                || transition != direct_turn_profile::DirectTurnEvent::Delivered(expected_delivery)
                || proof.get::<String, _>("receipt_codec_family")
                    != direct_turn_profile::RECEIPT_CODEC_FAMILY
                || proof.get::<i64, _>("receipt_codec_version")
                    != i64::from(direct_turn_profile::PROTOCOL_VERSION)
                || proof.get::<String, _>("delivery_codec_family")
                    != direct_turn_profile::RECEIPT_EVENT_CODEC_FAMILY
                || proof.get::<i64, _>("delivery_codec_version")
                    != i64::from(direct_turn_profile::PROTOCOL_VERSION)
                || proof.get::<String, _>("transition_codec_family")
                    != direct_turn_profile::EVENT_CODEC_FAMILY
                || proof.get::<i64, _>("transition_codec_version")
                    != i64::from(direct_turn_profile::PROTOCOL_VERSION)
                || proof.get::<String, _>("status") != "Accepted"
                || proof.get::<String, _>("origin") != "Execution"
                || proof.get::<String, _>("payload_kind") != "Receipt"
                || proof.get::<String, _>("consumer_kind") != "reducer"
                || proof.get::<i64, _>("requires_runtime_acceptance") != 1
                || proof
                    .get::<Option<String>, _>("suppression_reason")
                    .is_some()
                || proof.get::<String, _>("runtime_acceptance_status") != "Accepted"
                || proof.get::<i64, _>("generation") != intent.generation()
                || proof.get::<i64, _>("transition_generation") != intent.generation()
                || proof.get::<i64, _>("process_incarnation")
                    != witness.get::<i64, _>("process_incarnation")
                || proof.get::<i64, _>("from_version") != declared
                || declared.checked_add(1) != Some(proof.get::<i64, _>("to_version"))
            {
                return Err(DbError::Serialization(
                    "Start materialization workflow bundle diverged".into(),
                ));
            }
            let head = Self::goal_start_workflow_head_tx(&mut tx, input).await?;
            if head.get::<i64, _>("version") < proof.get::<i64, _>("to_version")
                || head.get::<i64, _>("generation") < intent.generation()
            {
                return Err(DbError::Serialization(
                    "Start workflow head precedes materialization proof".into(),
                ));
            }
            let local_workflow = head.get::<i64, _>("version") == proof.get::<i64, _>("to_version")
                && head.get::<i64, _>("generation") == intent.generation();
            if local_workflow && head.get::<String, _>("status") != "Active" {
                return Err(DbError::Serialization(
                    "Start materialized workflow head status diverged".into(),
                ));
            }
            let local_workflow = local_workflow
                && declared
                    == to_i64(
                        input.authority.declared_workflow_version.0,
                        "declared version",
                    )?;
            if live.get::<i64, _>("materialized") != 1
                || live
                    .get::<Option<String>, _>("canonical_message_id")
                    .is_some()
                || live.get::<i64, _>("goal_start_id")
                    != to_i64(input.start_id.get(), "Start receipt")?
            {
                return Err(DbError::Serialization(
                    "Start witness/live input diverged".into(),
                ));
            }
            let state: ConvState =
                serde_json::from_str(&live.get::<String, _>("conversation_state"))
                    .map_err(|error| DbError::Serialization(error.to_string()))?;
            if same_authority
                && local_workflow
                && request.checkpoint.is_none()
                && live.get::<i64, _>("generation") == intent.generation()
                && live.get::<i64, _>("owns_conversation") == 1
                && live.get::<Option<String>, _>("terminal_kind").is_none()
                && state == (ConvState::LlmRequesting { attempt: 1 })
                && live.get::<String, _>("conversation_updated_at")
                    == witness.get::<String, _>("state_updated_at")
            {
                return Ok(GoalStartAdoptionOutcome::ClassifiedCommitted {
                    turn_id: TurnAuthorityId(to_u64(intent.execution.turn_id(), "turn_id")?),
                    generation: input.authority.generation.0,
                });
            }
            return Ok(GoalStartAdoptionOutcome::CommittedElsewhere);
        }
        if let Some(live) = live {
            if live.get::<i64, _>("materialized") != 0 {
                return Err(DbError::Serialization(
                    "materialized Start lacks retained witness".into(),
                ));
            }
            if crate::conversation_goals::execution::retained_request_tx(
                &mut tx.tx,
                self.goal_execution_installation,
                &intent,
            )
            .await
            .map_err(|error| DbError::Serialization(error.to_string()))?
            .is_some()
            {
                return Err(DbError::Serialization(
                    "Start initial charge exists without witness".into(),
                ));
            }
            let state: ConvState =
                serde_json::from_str(&live.get::<String, _>("conversation_state"))
                    .map_err(|error| DbError::Serialization(error.to_string()))?;
            let transitions: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM workflow_transitions WHERE workflow_id=? AND transition_id=?",
            )
            .bind(intent.execution.workflow_id())
            .bind(to_i64(
                DIRECT_TURN_MATERIALIZED_TRANSITION_ID,
                "transition_id",
            )?)
            .fetch_one(&mut *tx.tx)
            .await?;
            if state != ConvState::Idle || transitions != 0 {
                return Err(DbError::Serialization(
                    "unmaterialized Start advanced without retained witness".into(),
                ));
            }
            let partial: i64 = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM workflow_receipts WHERE workflow_id=?1) OR EXISTS(SELECT 1 FROM workflow_deliveries WHERE workflow_id=?1)")
                .bind(intent.execution.workflow_id()).fetch_one(&mut *tx.tx).await?;
            if partial != 0 {
                return Err(DbError::Serialization(
                    "unmaterialized Start has partial receipt/delivery evidence".into(),
                ));
            }
            let head = Self::goal_start_workflow_head_tx(&mut tx, input).await?;
            if head.get::<i64, _>("version")
                != to_i64(
                    input.authority.declared_workflow_version.0,
                    "declared version",
                )?
                || head.get::<i64, _>("generation") != intent.generation()
                || head.get::<String, _>("status") != "Active"
            {
                return Err(DbError::Serialization(
                    "unmaterialized Start workflow head changed".into(),
                ));
            }
            let eligible = self.preflight_goal_start_tx(&mut tx, input).await?;
            return Ok(if eligible == DirectTurnMaterializationEligibility::Fresh {
                GoalStartAdoptionOutcome::NotCommitted
            } else {
                GoalStartAdoptionOutcome::StaleAuthority
            });
        }
        Err(DbError::Serialization(
            "Start live input and retained materialization witness both absent".into(),
        ))
    }
}
