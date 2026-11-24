//! Read-only evidence for the pre-anchor initial-loss origin, not a retirement proof.

use phoenix_core::domain::close::{
    CapturedWorktreeIdentity, CloseAttemptScope, CloseInspection, CloseInspectionLoss,
    CloseRetirementSnapshot, CloseRunOrdinal, CloseRunRef, CloseStopCertainty, GitPathIdentity,
    LossItemIdentity, OpaqueIdentity, RetiredResourceIdentity, RetiredResourceKind,
    RetirementFailureReason,
};
use phoenix_core::domain::db_schema::SourceToolCall;
use phoenix_core::domain::product_conversation::ProductConversationId;
use phoenix_core::work_scope::WorkScopeId;
use sqlx::{Row, Sqlite, Transaction};

use super::{NativeCloseCommand, NativeCloseIntent, NativeCloseRunRef};
use crate::close_foundation::{
    parse_cleanup_resource_identity, validate_close_retirement_inspection_aggregate,
    CloseCleanupFailureAuthority, CloseCleanupFailureResource, CloseCleanupResourceDisposition,
    TerminalizeInitialCloseCleanupFailureRequest,
};
use crate::{Database, DbError, DbResult};

/// Read-only routing evidence, never retry admission or dispatch authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeCloseRetryOriginKind {
    Anchored,
    CapturedLoss,
}

/// An exact retained initial-loss origin observed in one database snapshot.
/// This is deliberately not `StoppedCloseOriginalRetirementProof`: no original
/// retirement anchor, sealed resource plan, admission, or dispatch authority exists.
/// Later admission must revalidate this evidence and establish its own authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeCloseCapturedLossRetryContext {
    initial_command: NativeCloseCommand,
    failed_run: CloseRunRef,
    snapshot: CloseRetirementSnapshot,
    scopes: Vec<CloseAttemptScope>,
    inspections: Vec<CloseInspection>,
    losses: Vec<CloseInspectionLoss>,
    failure: TerminalizeInitialCloseCleanupFailureRequest,
}

impl NativeCloseCapturedLossRetryContext {
    #[must_use]
    pub fn initial_command(&self) -> &NativeCloseCommand {
        &self.initial_command
    }

    #[must_use]
    pub fn failed_run(&self) -> &CloseRunRef {
        &self.failed_run
    }

    #[must_use]
    pub fn snapshot(&self) -> &CloseRetirementSnapshot {
        &self.snapshot
    }

    #[must_use]
    pub fn scopes(&self) -> &[CloseAttemptScope] {
        &self.scopes
    }

    #[must_use]
    pub fn inspections(&self) -> &[CloseInspection] {
        &self.inspections
    }

    #[must_use]
    pub fn losses(&self) -> &[CloseInspectionLoss] {
        &self.losses
    }

    #[must_use]
    pub fn failure(&self) -> &TerminalizeInitialCloseCleanupFailureRequest {
        &self.failure
    }
}

fn refused(message: &str) -> DbError {
    DbError::CloseFoundationPrecondition(message.to_owned())
}

fn decode_error(error: impl std::fmt::Display) -> DbError {
    DbError::Serialization(error.to_string())
}

fn scope_resource(scope: &WorkScopeId) -> DbResult<RetiredResourceIdentity> {
    RetiredResourceIdentity::parse(
        RetiredResourceKind::WorkScope,
        LossItemIdentity::Opaque(OpaqueIdentity::parse(scope.as_str()).map_err(decode_error)?),
    )
    .map_err(decode_error)
}

impl Database {
    /// Classifies an exact retained origin without requiring captured losses to
    /// possess a sealed inventory. Each branch validates its own provenance in
    /// the same snapshot; assessment and admission must still revalidate it.
    ///
    /// # Errors
    /// Refuses a mismatched target, invalid anchored proof, or invalid captured-loss
    /// context. Database errors are never interpreted as an alternate origin.
    pub async fn read_native_close_retry_origin_kind(
        &self,
        target: &ProductConversationId,
        failed_run: &CloseRunRef,
    ) -> DbResult<NativeCloseRetryOriginKind> {
        let mut tx = self.pool.begin().await?;
        let (target_matches, anchored): (bool, bool) = sqlx::query_as(
            "SELECT EXISTS(SELECT 1 FROM close_obligations
                           WHERE attempt_id = ?1 AND product_conversation_id = ?2),
                    EXISTS(SELECT 1 FROM close_original_retirement_anchors
                           WHERE attempt_id = ?1)",
        )
        .bind(failed_run.attempt_id.as_str())
        .bind(target.as_str())
        .fetch_one(&mut *tx)
        .await?;
        if !target_matches {
            return Err(refused(
                "retry origin target differs from the retained obligation",
            ));
        }
        let kind = if anchored {
            Self::read_stopped_close_original_retirement_proof_tx(&mut tx, failed_run).await?;
            NativeCloseRetryOriginKind::Anchored
        } else {
            Self::read_native_close_captured_loss_retry_context_tx(&mut tx, target, failed_run)
                .await?;
            NativeCloseRetryOriginKind::CapturedLoss
        };
        tx.commit().await?;
        Ok(kind)
    }

    /// Reads only the exact latest native initial run stopped by retained loss inspection.
    /// No current scope is recaptured, no allowance is consumed, and no authority is granted.
    /// The historical initial source need not still be executing. This does not validate
    /// a new explicit-retry source; admission must do so separately in its writer transaction.
    ///
    /// # Errors
    /// Refuses cancellation, retained suppression, later runs, anchored or dispatched work,
    /// non-loss failures, and incomplete or inconsistent captured evidence.
    pub async fn read_native_close_captured_loss_retry_context(
        &self,
        target: &ProductConversationId,
        failed_run: &CloseRunRef,
    ) -> DbResult<NativeCloseCapturedLossRetryContext> {
        let mut tx = self.pool.begin().await?;
        let context =
            Self::read_native_close_captured_loss_retry_context_tx(&mut tx, target, failed_run)
                .await?;
        tx.commit().await?;
        Ok(context)
    }

    #[allow(clippy::too_many_lines)]
    pub(super) async fn read_native_close_captured_loss_retry_context_tx(
        tx: &mut Transaction<'_, Sqlite>,
        target: &ProductConversationId,
        failed_run: &CloseRunRef,
    ) -> DbResult<NativeCloseCapturedLossRetryContext> {
        if failed_run.ordinal != CloseRunOrdinal::INITIAL {
            return Err(refused("captured loss retry requires the initial run"));
        }
        let eligible: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM close_obligations obligation
             JOIN close_runs run ON run.attempt_id = obligation.attempt_id
             WHERE obligation.attempt_id = ?1 AND obligation.product_conversation_id = ?2
               AND obligation.native_managed = 1 AND obligation.phase = 'completed'
               AND obligation.close_outcome = 'close_incomplete'
               AND run.run_ordinal = 1 AND run.status = 'stopped' AND run.ended_at_us IS NOT NULL
               AND run.run_ordinal = (SELECT MAX(run_ordinal) FROM close_runs WHERE attempt_id = ?1))
             AND NOT EXISTS(SELECT 1 FROM close_original_retirement_anchors WHERE attempt_id = ?1)
             AND NOT EXISTS(SELECT 1 FROM close_retirement_resources WHERE attempt_id = ?1)
             AND NOT EXISTS(SELECT 1 FROM close_retirement_resource_history WHERE attempt_id = ?1)
             AND NOT EXISTS(SELECT 1 FROM close_retirement_resource_dispatches WHERE attempt_id = ?1)
             AND NOT EXISTS(SELECT 1 FROM close_process_step_successes WHERE attempt_id = ?1)
             AND NOT EXISTS(SELECT 1 FROM close_run_retry_successes WHERE attempt_id = ?1)
             AND NOT EXISTS(SELECT 1 FROM native_close_retry_plans WHERE attempt_id = ?1)
             AND EXISTS(SELECT 1 FROM close_completion_retry_allowances
                        WHERE attempt_id = ?1 AND suppressed = 0)
             AND NOT EXISTS(SELECT 1 FROM close_retirement_anchor_exclusions WHERE attempt_id = ?1)
             AND NOT EXISTS(SELECT 1 FROM close_retirement_inventories WHERE attempt_id = ?1)
             AND NOT EXISTS(SELECT 1 FROM close_expected_retirement_resources WHERE attempt_id = ?1)
             AND NOT EXISTS(SELECT 1 FROM close_cleanup_failures
                            WHERE attempt_id = ?1 AND authority_kind = 'explicitly_suppressed')",
        )
        .bind(failed_run.attempt_id.as_str())
        .bind(target.as_str())
        .fetch_one(&mut **tx)
        .await?;
        if !eligible {
            return Err(refused(
                "captured loss retry requires an effect-free latest stopped native initial loss",
            ));
        }
        let receipts = sqlx::query(
            "SELECT * FROM native_close_initial_receipts
             WHERE attempt_id = ?1 AND run_ordinal = 1 AND intent = 'initial'",
        )
        .bind(failed_run.attempt_id.as_str())
        .fetch_all(&mut **tx)
        .await?;
        let [receipt] = receipts.as_slice() else {
            return Err(refused(
                "captured loss origin requires exactly one initial receipt",
            ));
        };
        let initial_command = NativeCloseCommand::new(
            receipt.try_get("source_transcript_id")?,
            SourceToolCall {
                message_id: receipt.try_get("source_message_id")?,
                tool_use_id: receipt.try_get("source_tool_use_id")?,
            },
            NativeCloseIntent::Initial {
                target: target.clone(),
            },
        )
        .map_err(decode_error)?;
        if receipt.try_get::<String, _>("request_id")? != initial_command.request_id().to_string()
            || super::receipts::read_receipt_tx(tx, &initial_command).await?
                != Some(NativeCloseRunRef {
                    target: target.clone(),
                    attempt_id: failed_run.attempt_id.clone(),
                    run_ordinal: failed_run.ordinal,
                })
        {
            return Err(refused("captured loss initial receipt provenance differs"));
        }
        let projection = Self::get_active_close_projection_for_product_on(tx, target)
            .await?
            .ok_or_else(|| refused("captured loss projection is absent"))?;
        if projection.obligation.attempt_id() != &failed_run.attempt_id
            || projection.losses.is_empty()
        {
            return Err(refused(
                "captured loss origin is not the active retained loss attempt",
            ));
        }
        let snapshot = projection
            .obligation
            .snapshot()
            .cloned()
            .ok_or_else(|| refused("captured loss inspection snapshot is absent"))?;
        validate_close_retirement_inspection_aggregate(&snapshot, &projection.inspections)?;
        let scopes = read_captured_scopes_tx(tx, failed_run).await?;
        let inspected: Vec<_> = projection
            .inspections
            .iter()
            .map(|inspection| &inspection.target.scope)
            .collect();
        let captured: Vec<_> = scopes
            .iter()
            .filter(|scope| scope.captured_worktree.is_some())
            .map(|scope| &scope.scope)
            .collect();
        if scopes.is_empty()
            || inspected != captured
            || projection.losses.iter().any(|loss| {
                !projection.inspections.iter().any(|inspection| {
                    loss.attempt_id == inspection.attempt_id
                        && loss.scope == inspection.target.scope
                        && loss.snapshot == inspection.snapshot
                })
            })
        {
            return Err(refused(
                "captured loss inspection does not cover the original worktree scopes",
            ));
        }
        let failure_row = sqlx::query(
            "SELECT failure.* FROM close_cleanup_failures failure
             JOIN close_runs run ON run.attempt_id = failure.attempt_id
               AND run.run_ordinal = failure.cleanup_run_ordinal
             WHERE failure.attempt_id = ?1 AND failure.cleanup_run_ordinal = 1
               AND failure.authority_kind = 'captured_scope'
               AND failure.reason = 'manual_repair_required'
               AND failure.stop_certainty = 'shutdown_uncertain'
               AND failure.occurred_at_us = run.ended_at_us",
        )
        .bind(failed_run.attempt_id.as_str())
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| refused("captured loss initial failure occurrence is absent"))?;
        let authority_scope =
            WorkScopeId::parse(failure_row.try_get::<String, _>("scope")?).map_err(decode_error)?;
        if !projection
            .losses
            .iter()
            .any(|loss| loss.scope == authority_scope)
        {
            return Err(refused(
                "captured failure authority does not identify a retained loss scope",
            ));
        }
        let mut remaining_resources = Vec::new();
        for scope in &scopes {
            remaining_resources.push(CloseCleanupFailureResource {
                scope: scope.scope.clone(),
                resource: scope_resource(&scope.scope)?,
                disposition: CloseCleanupResourceDisposition::Unknown,
            });
            if let Some(CapturedWorktreeIdentity::Resolved(identity)) = &scope.captured_worktree {
                remaining_resources.push(CloseCleanupFailureResource {
                    scope: scope.scope.clone(),
                    resource: RetiredResourceIdentity::parse(
                        RetiredResourceKind::Worktree,
                        LossItemIdentity::Worktree(identity.clone()),
                    )
                    .map_err(decode_error)?,
                    disposition: CloseCleanupResourceDisposition::Unknown,
                });
            }
        }
        let failure = TerminalizeInitialCloseCleanupFailureRequest {
            failure_occurrence_id: failed_run.failure_occurrence_id(),
            attempt_id: failed_run.attempt_id.clone(),
            source_product_conversation_id: target.clone(),
            authority: CloseCleanupFailureAuthority::CapturedScope {
                resource: scope_resource(&authority_scope)?,
                scope: authority_scope,
            },
            remaining_resources,
            reason: RetirementFailureReason::ManualRepairRequired,
            detail: failure_row.try_get("detail")?,
            stop_certainty: CloseStopCertainty::ShutdownUncertain,
            occurred_at_us: failure_row.try_get("occurred_at_us")?,
        };
        if failure.detail.trim().is_empty()
            || !Self::retained_close_cleanup_failure_matches_tx(tx, failed_run, &failure).await?
        {
            return Err(refused(
                "captured loss failure differs from its full original captured identities",
            ));
        }
        Ok(NativeCloseCapturedLossRetryContext {
            initial_command,
            failed_run: failed_run.clone(),
            snapshot,
            scopes,
            inspections: projection.inspections,
            losses: projection.losses,
            failure,
        })
    }
}

pub(super) async fn read_captured_scopes_tx(
    tx: &mut Transaction<'_, Sqlite>,
    run: &CloseRunRef,
) -> DbResult<Vec<CloseAttemptScope>> {
    let rows = sqlx::query(
        "SELECT *, 'worktree' AS resource_kind, 'worktree' AS identity_kind,
                'worktree_id_v1' AS identity_codec, captured_worktree_identity AS identity_value
         FROM close_attempt_scopes WHERE attempt_id = ?1 ORDER BY scope",
    )
    .bind(run.attempt_id.as_str())
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter()
        .map(|row| {
            let captured_worktree = match (
                row.try_get::<Option<String>, _>("captured_worktree_identity")?,
                row.try_get::<Option<String>, _>("captured_worktree_fingerprint")?,
                row.try_get::<Option<String>, _>("captured_worktree_locator")?,
            ) {
                (Some(_), Some(_), Some(_)) => {
                    let resource = parse_cleanup_resource_identity(&row)?;
                    let LossItemIdentity::Worktree(identity) = resource.identity() else {
                        return Err(refused(
                            "captured worktree decoder returned a different identity kind",
                        ));
                    };
                    Some(CapturedWorktreeIdentity::Resolved(identity.clone()))
                }
                (None, None, Some(locator)) => Some(CapturedWorktreeIdentity::Unresolved {
                    locator: GitPathIdentity::decode_exact(&locator).map_err(decode_error)?,
                }),
                (None, None, None) => None,
                _ => {
                    return Err(refused(
                        "captured loss scope contains a partial worktree identity",
                    ))
                }
            };
            let captured_at: String = row.try_get("captured_at")?;
            Ok(CloseAttemptScope {
                attempt_id: run.attempt_id.clone(),
                scope: WorkScopeId::parse(row.try_get::<String, _>("scope")?)
                    .map_err(decode_error)?,
                captured_worktree,
                captured_at: chrono::DateTime::parse_from_rfc3339(&captured_at)
                    .map_err(decode_error)?
                    .with_timezone(&chrono::Utc),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::native_close::NativeCloseReceipt;
    use crate::workflow::LocalAuthorityResult;
    use phoenix_core::domain::close::CloseLossItem;
    use phoenix_core::domain::llm_types::ContentBlock;
    use phoenix_core::domain::sm_state::{AssistantMessage, ConvState, ToolCall, ToolInput};
    use phoenix_core::llm_language::LlmLanguage;

    fn established<T: std::fmt::Debug>(result: LocalAuthorityResult<DbResult<T>>) -> T {
        match result {
            LocalAuthorityResult::DurableFactEstablished(Ok(value)) => value,
            other @ (LocalAuthorityResult::DurableFactEstablished(Err(_))
            | LocalAuthorityResult::DurableFactUnclassified) => {
                panic!("expected established success: {other:?}");
            }
        }
    }

    fn retained_loss_run(receipt: &NativeCloseReceipt) -> CloseRunRef {
        CloseRunRef {
            attempt_id: receipt.admitted.attempt_id.clone(),
            ordinal: CloseRunOrdinal::INITIAL,
        }
    }

    async fn retained_loss_origin() -> (Database, NativeCloseCommand, CloseRunRef) {
        let database = Database::open_in_memory().await.unwrap();
        let owner = database
            .get_or_create_coordinator(None, LlmLanguage::default())
            .await
            .unwrap();
        let product = database
            .create_conversation(
                "captured-loss-target",
                "captured-loss-target",
                "/tmp",
                true,
                None,
                None,
            )
            .await
            .unwrap();
        let work_scope =
            crate::close_foundation::allocate_scope_worktree(&database, &product.id).await;
        let command = NativeCloseCommand::new(
            owner.id.clone(),
            SourceToolCall {
                message_id: "loss-message".into(),
                tool_use_id: "loss-call".into(),
            },
            NativeCloseIntent::Initial {
                target: product.product_conversation_id,
            },
        )
        .unwrap();
        let input = command.intent().source_input();
        let live = ConvState::ToolExecuting {
            current_tool: ToolCall::new(
                "loss-call",
                ToolInput::Unknown {
                    name: "close_conversation".into(),
                    input: input.clone(),
                },
            ),
            remaining_tools: Vec::new(),
            completed_results: Vec::new(),
            pending_sub_agents: Vec::new(),
            assistant_message: AssistantMessage::new(
                "loss-message".into(),
                vec![ContentBlock::ToolUse {
                    id: "loss-call".into(),
                    name: "close_conversation".into(),
                    input,
                }],
                None,
                None,
            ),
        };
        database
            .update_conversation_state(&owner.id, &live)
            .await
            .unwrap();
        let super::super::NativeCloseInitialResult::NewlyAdmitted(receipt) =
            established(database.apply_native_close_initial(&command).await)
        else {
            panic!("expected initial admission")
        };
        established(
            database
                .prepare_native_close_initial_execution(&command, &receipt, &live)
                .await,
        );
        established(
            database
                .settle_native_close_initial(&command, &receipt, &live)
                .await,
        );
        let run = retained_loss_run(&receipt);
        let inspection = crate::ReplaceCloseInspectionRequest {
            attempt_id: run.attempt_id.clone(),
            scopes: vec![crate::ReplaceCloseInspectionScopeRequest {
                scope: work_scope,
                snapshot: CloseRetirementSnapshot::parse("loss-generation", "loss-fingerprint")
                    .unwrap(),
                losses: vec![CloseLossItem::UntrackedNonIgnoredPath(
                    GitPathIdentity::from_bytes(b"dirty-file".to_vec()),
                )],
            }],
        };
        established(
            database
                .terminalize_native_close_initial_losses(
                    &command,
                    &receipt,
                    &live,
                    inspection,
                    None,
                    "unconfirmed dirty-file loss",
                )
                .await,
        );
        (database, command, run)
    }

    #[tokio::test]
    async fn exact_loss_origin_is_read_only_and_keeps_original_captured_identity() {
        let (db, command, run) = retained_loss_origin().await;
        let before = db
            .list_close_cleanup_failures(run.attempt_id.as_str())
            .await
            .unwrap();
        let context = db
            .read_native_close_captured_loss_retry_context(command.intent().target(), &run)
            .await
            .unwrap();
        assert_eq!(context.initial_command(), &command);
        assert_eq!(context.failed_run(), &run);
        assert_eq!(context.losses().len(), 1);
        assert_eq!(context.inspections().len(), 1);
        assert_eq!(
            context.scopes(),
            db.list_close_attempt_scopes(run.attempt_id.as_str())
                .await
                .unwrap()
        );
        assert_eq!(context.failure(), &before[0].occurrence);
        let scope = &context.scopes()[0].scope;
        sqlx::query("UPDATE work_scopes SET worktree_fingerprint = 'replacement' WHERE id = ?1")
            .bind(scope.as_str())
            .execute(db.pool())
            .await
            .unwrap();
        let reread = db
            .read_native_close_captured_loss_retry_context(command.intent().target(), &run)
            .await
            .unwrap();
        assert_eq!(reread, context);
        assert_eq!(
            db.list_close_cleanup_failures(run.attempt_id.as_str())
                .await
                .unwrap(),
            before
        );
        let runs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM close_runs WHERE attempt_id = ?1")
            .bind(run.attempt_id.as_str())
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(runs, 1);
    }

    #[tokio::test]
    async fn retained_observe_receipt_does_not_replace_initial_loss_provenance() {
        let (db, command, run) = retained_loss_origin().await;
        let before = db
            .read_native_close_captured_loss_retry_context(command.intent().target(), &run)
            .await
            .unwrap();
        let failures = db
            .list_close_cleanup_failures(run.attempt_id.as_str())
            .await
            .unwrap();
        let receipt_counts: (i64, i64) = sqlx::query_as(
            "SELECT COUNT(*), COUNT(CASE WHEN intent = 'initial' THEN 1 END)
             FROM native_close_initial_receipts WHERE attempt_id = ?1 AND run_ordinal = 1",
        )
        .bind(run.attempt_id.as_str())
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(receipt_counts, (1, 1));

        let observe = NativeCloseCommand::new(
            command.source_transcript_id().to_owned(),
            SourceToolCall {
                message_id: "observe-loss-message".into(),
                tool_use_id: "observe-loss-call".into(),
            },
            NativeCloseIntent::Observe {
                target: command.intent().target().clone(),
                attempt_id: run.attempt_id.clone(),
                run_ordinal: run.ordinal,
            },
        )
        .unwrap();
        let referenced = NativeCloseRunRef {
            target: command.intent().target().clone(),
            attempt_id: run.attempt_id.clone(),
            run_ordinal: run.ordinal,
        };
        // Exercise the reader with an additional schema-valid retained receipt.
        // This lower-level fixture does not assert Observe source admission or
        // replace the original command: ordinary replacement must suppress it
        // (covered separately below). Receipt lookup must select the initial
        // provenance, not accidentally decode this Observe as a second origin.
        let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await.unwrap();
        super::super::receipts::adopt_and_insert_receipt_tx(&mut tx, &observe, &referenced)
            .await
            .unwrap();
        assert_eq!(
            super::super::receipts::read_receipt_tx(&mut tx, &observe)
                .await
                .unwrap(),
            Some(referenced)
        );
        tx.commit().await.unwrap();

        let after = db
            .read_native_close_captured_loss_retry_context(command.intent().target(), &run)
            .await
            .unwrap();
        assert_eq!(after, before);
        assert_eq!(after.initial_command(), &command);
        assert_ne!(after.initial_command(), &observe);
        assert_eq!(
            db.list_close_cleanup_failures(run.attempt_id.as_str())
                .await
                .unwrap(),
            failures
        );
        let counts: (i64, i64, i64, i64) = sqlx::query_as(
            "SELECT COUNT(*), COUNT(CASE WHEN intent = 'initial' THEN 1 END),
                    COUNT(CASE WHEN intent = 'observe' THEN 1 END),
                    (SELECT COUNT(*) FROM close_runs WHERE attempt_id = ?1)
             FROM native_close_initial_receipts WHERE attempt_id = ?1 AND run_ordinal = 1",
        )
        .bind(run.attempt_id.as_str())
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(counts, (2, 1, 1, 1));
    }

    #[tokio::test]
    async fn original_source_replacement_suppresses_captured_loss_origin() {
        let (db, command, run) = retained_loss_origin().await;
        let before = db
            .list_close_cleanup_failures(run.attempt_id.as_str())
            .await
            .unwrap();
        db.read_native_close_captured_loss_retry_context(command.intent().target(), &run)
            .await
            .unwrap();
        db.update_conversation_state(command.source_transcript_id(), &ConvState::Idle)
            .await
            .unwrap();
        assert!(matches!(
            db.read_native_close_captured_loss_retry_context(command.intent().target(), &run)
                .await,
            Err(DbError::CloseFoundationPrecondition(_))
        ));
        assert_eq!(
            db.list_close_cleanup_failures(run.attempt_id.as_str())
                .await
                .unwrap(),
            before
        );
        let counts: (i64, i64, i64) = sqlx::query_as(
            "SELECT
                (SELECT COUNT(*) FROM close_runs WHERE attempt_id = ?1),
                (SELECT COUNT(*) FROM close_run_retry_effects WHERE attempt_id = ?1),
                (SELECT COUNT(*) FROM close_completion_retry_allowances
                 WHERE attempt_id = ?1 AND suppressed = 1)",
        )
        .bind(run.attempt_id.as_str())
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(counts, (1, 0, 1));
    }

    #[tokio::test]
    async fn wrong_target_and_durable_suppression_refuse_loss_origin() {
        let (db, command, run) = retained_loss_origin().await;
        assert!(matches!(db.read_native_close_captured_loss_retry_context(
            &ProductConversationId::parse("wrong-target").unwrap(), &run,
        ).await, Err(DbError::CloseFoundationPrecondition(message))
            if message == "captured loss retry requires an effect-free latest stopped native initial loss"));
        sqlx::query(
            "UPDATE close_completion_retry_allowances SET suppressed = 1 WHERE attempt_id = ?1",
        )
        .bind(run.attempt_id.as_str())
        .execute(db.pool())
        .await
        .unwrap();
        assert!(matches!(db.read_native_close_captured_loss_retry_context(
            command.intent().target(), &run,
        ).await, Err(DbError::CloseFoundationPrecondition(message))
            if message == "captured loss retry requires an effect-free latest stopped native initial loss"));
    }

    #[tokio::test]
    async fn missing_origin_and_noninitial_runs_are_refused_read_only() {
        let db = Database::open_in_memory().await.unwrap();
        let target = ProductConversationId::parse("missing-target").unwrap();
        let attempt_id =
            phoenix_core::domain::close::CloseAttemptId::parse("missing-attempt").unwrap();
        for ordinal in [CloseRunOrdinal::INITIAL, CloseRunOrdinal::parse(2).unwrap()] {
            let run = CloseRunRef {
                attempt_id: attempt_id.clone(),
                ordinal,
            };
            assert!(db
                .read_native_close_captured_loss_retry_context(&target, &run)
                .await
                .is_err());
        }
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM close_runs")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(count, 0);
    }

    include!("initial_result_acknowledgement_tests.rs");
    include!("captured_loss_admission_tests.rs");
}
