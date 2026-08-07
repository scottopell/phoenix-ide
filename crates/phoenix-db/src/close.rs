use chrono::{DateTime, Utc};
use phoenix_core::domain::close::{
    CloseInspection, CloseInspectionLoss, CloseObligation, ClosePhase, CloseRetiredResource,
    CloseTombstoneKind, LossCategory, ProductConversationId, RetiredResourceKind,
    RetirementFailureReason, RetirementOutcome,
};
use phoenix_core::domain::db_schema::{Message, MessageContent};
use phoenix_core::work_scope::{RuntimeRole, WorkScopeId, WorkScopeRetirementBlocker};
use sqlx::{Row, Sqlite, Transaction};

use crate::{insert_message_tx, retrieval, ConvState, Database, DbError, DbResult, MessageType};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductConversationTopology {
    pub root_conversation_id: ProductConversationId,
    pub latest_conversation_id: ProductConversationId,
    pub member_conversation_ids: Vec<ProductConversationId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BeginCloseOutcome {
    Started(CloseObligation),
    AlreadyStarted(CloseObligation),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmInspectionOutcome {
    Confirmed(CloseObligation),
    Mismatch { obligation: CloseObligation },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordRetirementResourceOutcome {
    Inserted(CloseRetiredResource),
    Unchanged(CloseRetiredResource),
    Updated(CloseRetiredResource),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeleteHistoryAggregateOutcome {
    Deleted {
        topology: ProductConversationTopology,
    },
    AlreadyDeleted {
        root_conversation_id: ProductConversationId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeInspectionInput {
    pub scope: WorkScopeId,
    pub generation: Option<String>,
    pub fingerprint: Option<String>,
    pub losses: Vec<LossRowInput>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LossRowInput {
    pub category: LossCategory,
    pub item_identity: String,
}

impl Database {
    pub async fn product_conversation_topology(
        &self,
        conversation_id: &ProductConversationId,
    ) -> DbResult<ProductConversationTopology> {
        let mut tx = self.pool.begin().await?;
        let topology = product_conversation_topology_tx(&mut tx, conversation_id).await?;
        tx.rollback().await?;
        Ok(topology)
    }

    pub async fn begin_close(
        &self,
        conversation_id: &ProductConversationId,
        attempt_id: &str,
    ) -> DbResult<BeginCloseOutcome> {
        let mut tx = self.pool.begin().await?;
        let topology = product_conversation_topology_tx(&mut tx, conversation_id).await?;
        let root_id = topology.root_conversation_id.as_str();
        let root = sqlx::query(
            "SELECT runtime_role, user_initiated, archived FROM conversations WHERE id = ?1",
        )
        .bind(root_id)
        .fetch_one(&mut *tx)
        .await?;
        let runtime_role = parse_runtime_role(root.try_get("runtime_role")?)?;
        let user_initiated: bool = root.try_get("user_initiated")?;
        let archived: bool = root.try_get("archived")?;
        if runtime_role != RuntimeRole::User || !user_initiated {
            return Err(DbError::CloseAttemptConflict(format!(
                "close only allowed for ordinary user conversations: {root_id}"
            )));
        }
        if archived {
            return Err(DbError::CloseAttemptConflict(format!(
                "close not allowed for archived root conversation: {root_id}"
            )));
        }
        if topology.latest_conversation_id.as_str() != conversation_id.as_str() {
            return Err(DbError::CloseAttemptConflict(format!(
                "close only allowed from latest conversation {} (got {})",
                topology.latest_conversation_id, conversation_id
            )));
        }
        if let Some(existing) = get_active_close_obligation_for_root_tx(&mut tx, root_id).await? {
            if existing.attempt_id == attempt_id {
                tx.rollback().await?;
                return Ok(BeginCloseOutcome::AlreadyStarted(existing));
            }
            return Err(DbError::CloseAttemptConflict(format!(
                "active attempt {} already exists for root {root_id}",
                existing.attempt_id
            )));
        }
        let now = Utc::now();
        sqlx::query(
            "INSERT INTO close_obligations (
                attempt_id, root_conversation_id, phase, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?4)",
        )
        .bind(attempt_id)
        .bind(root_id)
        .bind(ClosePhase::AwaitingBlockerResolution.as_str())
        .bind(now.to_rfc3339())
        .execute(&mut *tx)
        .await?;
        let obligation = get_close_obligation_tx(&mut tx, attempt_id).await?;
        tx.commit().await?;
        Ok(BeginCloseOutcome::Started(obligation))
    }

    pub async fn get_close_obligation(
        &self,
        attempt_id: &str,
    ) -> DbResult<Option<CloseObligation>> {
        let mut tx = self.pool.begin().await?;
        let row = get_close_obligation_optional_tx(&mut tx, attempt_id).await?;
        tx.rollback().await?;
        Ok(row)
    }

    pub async fn latest_close_obligation_for_product(
        &self,
        product_conversation_id: &ProductConversationId,
    ) -> DbResult<Option<CloseObligation>> {
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query(
            "SELECT attempt_id, root_conversation_id, phase,
                    inspection_generation, inspection_fingerprint,
                    created_at, updated_at, completed_at
             FROM close_obligations
             WHERE root_conversation_id = ?1
             ORDER BY created_at DESC, attempt_id DESC
             LIMIT 1",
        )
        .bind(product_conversation_id.as_str())
        .fetch_optional(&mut *tx)
        .await?;
        row.map(parse_close_obligation_row).transpose()
    }

    pub async fn list_pending_close_restart_attempts(&self) -> DbResult<Vec<CloseObligation>> {
        let rows = sqlx::query(
            "SELECT attempt_id, root_conversation_id, phase, inspection_generation, inspection_fingerprint, created_at, updated_at, completed_at
             FROM close_obligations
             WHERE phase <> 'completed'
             ORDER BY created_at, attempt_id",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(parse_close_obligation_row).collect()
    }

    pub async fn transition_close_phase(
        &self,
        attempt_id: &str,
        current_phase: ClosePhase,
        next_phase: ClosePhase,
    ) -> DbResult<CloseObligation> {
        if !current_phase.can_transition_to(next_phase) {
            return Err(DbError::InvalidCloseTransition {
                attempt_id: attempt_id.to_string(),
                from: current_phase.as_str().to_string(),
                to: next_phase.as_str().to_string(),
            });
        }
        let mut tx = self.pool.begin().await?;
        let stored = get_close_obligation_tx(&mut tx, attempt_id).await?;
        if stored.phase != current_phase {
            return Err(DbError::ClosePhaseConflict {
                attempt_id: attempt_id.to_string(),
                expected: current_phase.as_str().to_string(),
                actual: stored.phase.as_str().to_string(),
            });
        }
        let completed_at = (next_phase == ClosePhase::Completed).then(|| Utc::now().to_rfc3339());
        let updated = sqlx::query(
            "UPDATE close_obligations
             SET phase = ?1, completed_at = ?2, updated_at = ?3
             WHERE attempt_id = ?4 AND phase = ?5",
        )
        .bind(next_phase.as_str())
        .bind(completed_at)
        .bind(Utc::now().to_rfc3339())
        .bind(attempt_id)
        .bind(current_phase.as_str())
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() != 1 {
            let actual = get_close_obligation_tx(&mut tx, attempt_id).await?;
            return Err(DbError::ClosePhaseConflict {
                attempt_id: attempt_id.to_string(),
                expected: current_phase.as_str().to_string(),
                actual: actual.phase.as_str().to_string(),
            });
        }
        let obligation = get_close_obligation_tx(&mut tx, attempt_id).await?;
        tx.commit().await?;
        Ok(obligation)
    }

    pub async fn replace_inspection(
        &self,
        attempt_id: &str,
        phase_after_replace: ClosePhase,
        aggregate_generation: Option<&str>,
        aggregate_fingerprint: Option<&str>,
        inspections: Vec<ScopeInspectionInput>,
    ) -> DbResult<CloseObligation> {
        let mut tx = self.pool.begin().await?;
        let obligation = get_close_obligation_tx(&mut tx, attempt_id).await?;
        if obligation.phase != ClosePhase::AwaitingRetirementInspection {
            return Err(DbError::ClosePhaseConflict {
                attempt_id: attempt_id.to_string(),
                expected: ClosePhase::AwaitingRetirementInspection
                    .as_str()
                    .to_string(),
                actual: obligation.phase.as_str().to_string(),
            });
        }
        if !matches!(
            phase_after_replace,
            ClosePhase::AwaitingLossConfirmation | ClosePhase::RetirementRequested
        ) {
            return Err(DbError::InvalidCloseTransition {
                attempt_id: attempt_id.to_string(),
                from: obligation.phase.as_str().to_string(),
                to: phase_after_replace.as_str().to_string(),
            });
        }
        if aggregate_generation.is_some_and(str::is_empty)
            || aggregate_fingerprint.is_some_and(str::is_empty)
        {
            return Err(DbError::CloseAttemptConflict(
                "inspection aggregate generation/fingerprint must be non-empty when supplied"
                    .to_string(),
            ));
        }
        if aggregate_generation.is_some() != aggregate_fingerprint.is_some() {
            return Err(DbError::CloseAttemptConflict(
                "inspection aggregate generation/fingerprint must both be null or both nonnull"
                    .to_string(),
            ));
        }
        validate_inspection_shape(aggregate_generation, aggregate_fingerprint, &inspections)?;
        validate_inspection_belongs_to_aggregate(
            &mut tx,
            obligation.product_conversation_id.as_str(),
            attempt_id,
            &inspections,
        )
        .await?;
        sqlx::query("DELETE FROM close_retirement_losses WHERE attempt_id = ?1")
            .bind(attempt_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM close_retirement_inspections WHERE attempt_id = ?1")
            .bind(attempt_id)
            .execute(&mut *tx)
            .await?;
        for inspection in &inspections {
            sqlx::query(
                "INSERT INTO close_retirement_inspections (attempt_id, scope, generation, fingerprint)
                 VALUES (?1, ?2, ?3, ?4)",
            )
            .bind(attempt_id)
            .bind(inspection.scope.as_str())
            .bind(inspection.generation.as_deref())
            .bind(inspection.fingerprint.as_deref())
            .execute(&mut *tx)
            .await?;
            if let Some(generation) = inspection.generation.as_deref() {
                for loss in &inspection.losses {
                    sqlx::query(
                        "INSERT INTO close_retirement_losses (attempt_id, scope, generation, category, item_identity)
                         VALUES (?1, ?2, ?3, ?4, ?5)",
                    )
                    .bind(attempt_id)
                    .bind(inspection.scope.as_str())
                    .bind(generation)
                    .bind(loss.category.as_str())
                    .bind(&loss.item_identity)
                    .execute(&mut *tx)
                    .await?;
                }
            }
        }
        sqlx::query(
            "UPDATE close_obligations
             SET phase = ?1, inspection_generation = ?2, inspection_fingerprint = ?3, updated_at = ?4
             WHERE attempt_id = ?5",
        )
        .bind(phase_after_replace.as_str())
        .bind(aggregate_generation)
        .bind(aggregate_fingerprint)
        .bind(Utc::now().to_rfc3339())
        .bind(attempt_id)
        .execute(&mut *tx)
        .await?;
        let updated = get_close_obligation_tx(&mut tx, attempt_id).await?;
        tx.commit().await?;
        Ok(updated)
    }

    pub async fn list_close_inspections(&self, attempt_id: &str) -> DbResult<Vec<CloseInspection>> {
        let rows = sqlx::query(
            "SELECT attempt_id, scope, generation, fingerprint
             FROM close_retirement_inspections
             WHERE attempt_id = ?1
             ORDER BY scope",
        )
        .bind(attempt_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(parse_close_inspection_row).collect()
    }

    pub async fn list_close_inspection_losses(
        &self,
        attempt_id: &str,
    ) -> DbResult<Vec<CloseInspectionLoss>> {
        let rows = sqlx::query(
            "SELECT attempt_id, scope, generation, category, item_identity
             FROM close_retirement_losses
             WHERE attempt_id = ?1
             ORDER BY scope, generation, category, item_identity",
        )
        .bind(attempt_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(parse_close_inspection_loss_row)
            .collect()
    }

    pub async fn confirm_inspection(
        &self,
        attempt_id: &str,
        confirmed_generation: &str,
        confirmed_fingerprint: &str,
        fresh_generation: &str,
        fresh_fingerprint: &str,
    ) -> DbResult<ConfirmInspectionOutcome> {
        let mut tx = self.pool.begin().await?;
        let stored = get_close_obligation_tx(&mut tx, attempt_id).await?;
        if stored.phase != ClosePhase::AwaitingLossConfirmation {
            return Err(DbError::ClosePhaseConflict {
                attempt_id: attempt_id.to_string(),
                expected: ClosePhase::AwaitingLossConfirmation.as_str().to_string(),
                actual: stored.phase.as_str().to_string(),
            });
        }
        let confirmed_matches_fresh =
            confirmed_generation == fresh_generation && confirmed_fingerprint == fresh_fingerprint;
        let stored_matches_confirmed = stored.inspection_generation.as_deref()
            == Some(confirmed_generation)
            && stored.inspection_fingerprint.as_deref() == Some(confirmed_fingerprint);
        let matches = confirmed_matches_fresh && stored_matches_confirmed;
        let (next_phase, next_generation, next_fingerprint) = if matches {
            (
                ClosePhase::RetirementRequested,
                stored.inspection_generation.clone(),
                stored.inspection_fingerprint.clone(),
            )
        } else {
            (ClosePhase::AwaitingRetirementInspection, None, None)
        };
        sqlx::query(
            "UPDATE close_obligations
             SET phase = ?1,
                 inspection_generation = ?2,
                 inspection_fingerprint = ?3,
                 updated_at = ?4
             WHERE attempt_id = ?5",
        )
        .bind(next_phase.as_str())
        .bind(next_generation)
        .bind(next_fingerprint)
        .bind(Utc::now().to_rfc3339())
        .bind(attempt_id)
        .execute(&mut *tx)
        .await?;
        let updated = get_close_obligation_tx(&mut tx, attempt_id).await?;
        tx.commit().await?;
        Ok(if matches {
            ConfirmInspectionOutcome::Confirmed(updated)
        } else {
            ConfirmInspectionOutcome::Mismatch {
                obligation: updated,
            }
        })
    }

    pub async fn record_retirement_resource(
        &self,
        attempt_id: &str,
        scope: &WorkScopeId,
        resource_kind: RetiredResourceKind,
        resource_identity: &str,
        outcome: RetirementOutcome,
        detail: Option<&str>,
    ) -> DbResult<RecordRetirementResourceOutcome> {
        let mut tx = self.pool.begin().await?;
        let obligation = get_close_obligation_tx(&mut tx, attempt_id).await?;
        validate_scope_membership(
            &mut tx,
            obligation.product_conversation_id.as_str(),
            attempt_id,
            scope,
        )
        .await?;
        let existing = sqlx::query(
            "SELECT attempt_id, scope, resource_kind, resource_identity, outcome, failure_reason, detail, created_at, updated_at
             FROM close_retirement_resources
             WHERE attempt_id = ?1 AND scope = ?2 AND resource_kind = ?3 AND resource_identity = ?4",
        )
        .bind(attempt_id)
        .bind(scope.as_str())
        .bind(resource_kind.as_str())
        .bind(resource_identity)
        .fetch_optional(&mut *tx)
        .await?;
        let (outcome_str, failure_reason) = encode_retirement_outcome(&outcome);
        let detail_owned = detail.map(ToOwned::to_owned);
        let result = if let Some(row) = existing {
            let existing_resource = parse_close_retired_resource_row(row)?;
            if existing_resource.outcome == outcome && existing_resource.detail == detail_owned {
                RecordRetirementResourceOutcome::Unchanged(existing_resource)
            } else {
                sqlx::query(
                    "UPDATE close_retirement_resources
                     SET outcome = ?1, failure_reason = ?2, detail = ?3, updated_at = ?4
                     WHERE attempt_id = ?5 AND scope = ?6 AND resource_kind = ?7 AND resource_identity = ?8",
                )
                .bind(outcome_str)
                .bind(failure_reason)
                .bind(detail)
                .bind(Utc::now().to_rfc3339())
                .bind(attempt_id)
                .bind(scope.as_str())
                .bind(resource_kind.as_str())
                .bind(resource_identity)
                .execute(&mut *tx)
                .await?;
                RecordRetirementResourceOutcome::Updated(
                    get_close_retired_resource_tx(
                        &mut tx,
                        attempt_id,
                        scope,
                        resource_kind,
                        resource_identity,
                    )
                    .await?,
                )
            }
        } else {
            let now = Utc::now().to_rfc3339();
            sqlx::query(
                "INSERT INTO close_retirement_resources (
                    attempt_id, scope, resource_kind, resource_identity, outcome, failure_reason, detail, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
            )
            .bind(attempt_id)
            .bind(scope.as_str())
            .bind(resource_kind.as_str())
            .bind(resource_identity)
            .bind(outcome_str)
            .bind(failure_reason)
            .bind(detail)
            .bind(&now)
            .execute(&mut *tx)
            .await?;
            RecordRetirementResourceOutcome::Inserted(
                get_close_retired_resource_tx(
                    &mut tx,
                    attempt_id,
                    scope,
                    resource_kind,
                    resource_identity,
                )
                .await?,
            )
        };
        tx.commit().await?;
        Ok(result)
    }

    pub async fn list_retirement_evidence(
        &self,
        attempt_id: &str,
    ) -> DbResult<Vec<CloseRetiredResource>> {
        let rows = sqlx::query(
            "SELECT attempt_id, scope, resource_kind, resource_identity, outcome, failure_reason, detail, created_at, updated_at
             FROM close_retirement_resources
             WHERE attempt_id = ?1
             ORDER BY scope, resource_kind, resource_identity",
        )
        .bind(attempt_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(parse_close_retired_resource_row)
            .collect()
    }

    pub async fn finalize_history(
        &self,
        attempt_id: &str,
        message_id: &str,
        text: &str,
    ) -> DbResult<ProductConversationTopology> {
        let mut tx = self.pool.begin().await?;
        let obligation = get_close_obligation_tx(&mut tx, attempt_id).await?;
        if obligation.phase != ClosePhase::RetirementRequested {
            return Err(DbError::ClosePhaseConflict {
                attempt_id: attempt_id.to_string(),
                expected: ClosePhase::RetirementRequested.as_str().to_string(),
                actual: obligation.phase.as_str().to_string(),
            });
        }
        let topology =
            product_conversation_topology_tx(&mut tx, &obligation.product_conversation_id).await?;
        let latest_id = topology.latest_conversation_id.as_str();
        let duplicate_message_id: i64 =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM messages WHERE message_id = ?1)")
                .bind(message_id)
                .fetch_one(&mut *tx)
                .await?;
        if duplicate_message_id != 0 {
            return Err(DbError::CloseAttemptConflict(format!(
                "finalization message id already exists: {message_id}"
            )));
        }
        let next_seq: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(sequence_id), 0) + 1 FROM messages WHERE conversation_id = ?1",
        )
        .bind(latest_id)
        .fetch_one(&mut *tx)
        .await?;
        let message = Message {
            message_id: message_id.to_string(),
            conversation_id: latest_id.to_string(),
            sequence_id: next_seq,
            message_type: MessageType::System,
            content: MessageContent::system(text),
            display_data: None,
            usage_data: None,
            created_at: Utc::now(),
        };
        insert_message_tx(&mut tx, &message).await?;
        for member in &topology.member_conversation_ids {
            sqlx::query("UPDATE conversations SET archived = 1, updated_at = ?1 WHERE id = ?2")
                .bind(Utc::now().to_rfc3339())
                .bind(member.as_str())
                .execute(&mut *tx)
                .await?;
        }
        let completed_at = Utc::now().to_rfc3339();
        sqlx::query(
            "UPDATE close_obligations
             SET phase = 'completed', completed_at = ?1, updated_at = ?1
             WHERE attempt_id = ?2",
        )
        .bind(&completed_at)
        .bind(attempt_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(topology)
    }

    pub async fn delete_history_aggregate(
        &self,
        conversation_id: &ProductConversationId,
    ) -> DbResult<DeleteHistoryAggregateOutcome> {
        if let Some(root) = sqlx::query_scalar::<_, String>(
            "SELECT root_conversation_id FROM close_tombstones WHERE conversation_id = ?1",
        )
        .bind(conversation_id.as_str())
        .fetch_optional(&self.pool)
        .await?
        {
            return Ok(DeleteHistoryAggregateOutcome::AlreadyDeleted {
                root_conversation_id: ProductConversationId::parse(root)
                    .map_err(|e| DbError::Serialization(e.to_string()))?,
            });
        }
        let mut tx = self.pool.begin().await?;
        let topology = product_conversation_topology_tx(&mut tx, conversation_id).await?;
        let root_id = topology.root_conversation_id.as_str();
        let root_archived: bool =
            sqlx::query_scalar("SELECT archived FROM conversations WHERE id = ?1")
                .bind(root_id)
                .fetch_one(&mut *tx)
                .await?;
        if !root_archived {
            return Err(DbError::CloseDeleteBlocked(
                "root conversation is not archived".to_string(),
            ));
        }
        let has_active_obligation: i64 = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM close_obligations WHERE root_conversation_id = ?1 AND phase <> 'completed')",
        )
        .bind(root_id)
        .fetch_one(&mut *tx)
        .await?;
        if has_active_obligation != 0 {
            return Err(DbError::CloseDeleteBlocked(
                "active close obligation exists".to_string(),
            ));
        }
        let blocker = delete_history_busy_blocker(&mut tx, &topology).await?;
        if let Some(blocker) = blocker {
            return Err(DbError::CloseDeleteBlocked(format!(
                "busy member blocks delete: {blocker:?}"
            )));
        }
        let deleted_at = Utc::now().to_rfc3339();
        for member in topology.member_conversation_ids.iter().rev() {
            let kind = if member == &topology.root_conversation_id {
                CloseTombstoneKind::Root
            } else {
                CloseTombstoneKind::Continuation
            };
            sqlx::query(
                "INSERT INTO close_tombstones (conversation_id, root_conversation_id, tombstone_kind, deleted_at)
                 VALUES (?1, ?2, ?3, ?4)",
            )
            .bind(member.as_str())
            .bind(root_id)
            .bind(close_tombstone_kind_str(kind))
            .bind(&deleted_at)
            .execute(&mut *tx)
            .await?;
        }
        for member in &topology.member_conversation_ids {
            sqlx::query(
                "DELETE FROM workflows
                 WHERE workflow_id IN (
                    SELECT workflow_id FROM wake_bindings WHERE conversation_id = ?1
                 )",
            )
            .bind(member.as_str())
            .execute(&mut *tx)
            .await?;
            retrieval::fts_delete_conversation_conn(&mut tx, member.as_str()).await?;
        }
        sqlx::query(
            "UPDATE conversations
             SET continued_in_conv_id = NULL
             WHERE id IN (
                 WITH RECURSIVE chain(id, next_id) AS (
                     SELECT id, continued_in_conv_id FROM conversations WHERE id = ?1
                     UNION ALL
                     SELECT c.id, c.continued_in_conv_id
                     FROM conversations c JOIN chain ON c.id = chain.next_id
                 )
                 SELECT id FROM chain
             )",
        )
        .bind(root_id)
        .execute(&mut *tx)
        .await?;
        for member in topology.member_conversation_ids.iter().rev() {
            sqlx::query("DELETE FROM conversations WHERE id = ?1")
                .bind(member.as_str())
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(DeleteHistoryAggregateOutcome::Deleted { topology })
    }
}

async fn product_conversation_topology_tx(
    tx: &mut Transaction<'_, Sqlite>,
    conversation_id: &ProductConversationId,
) -> DbResult<ProductConversationTopology> {
    let Some(root_id) = sqlx::query_scalar::<_, String>(
        "WITH RECURSIVE chain(id, depth) AS (
            SELECT id, 0 FROM conversations WHERE id = ?1
            UNION ALL
            SELECT p.id, chain.depth + 1
            FROM conversations p
            JOIN chain ON p.continued_in_conv_id = chain.id
         )
         SELECT id FROM chain ORDER BY depth DESC LIMIT 1",
    )
    .bind(conversation_id.as_str())
    .fetch_optional(&mut **tx)
    .await?
    else {
        return Err(DbError::CloseConversationNotFound(
            conversation_id.as_str().to_string(),
        ));
    };
    let rows = sqlx::query(
        "WITH RECURSIVE chain(id, next_id, depth) AS (
            SELECT id, continued_in_conv_id, 0 FROM conversations WHERE id = ?1
            UNION ALL
            SELECT c.id, c.continued_in_conv_id, chain.depth + 1
            FROM conversations c JOIN chain ON c.id = chain.next_id
         )
         SELECT id FROM chain ORDER BY depth",
    )
    .bind(&root_id)
    .fetch_all(&mut **tx)
    .await?;
    let members: Vec<ProductConversationId> = rows
        .into_iter()
        .map(|row| {
            ProductConversationId::parse(row.get::<String, _>("id"))
                .map_err(|e| DbError::Serialization(e.to_string()))
        })
        .collect::<DbResult<_>>()?;
    let latest = members
        .last()
        .cloned()
        .ok_or_else(|| DbError::CloseConversationNotFound(conversation_id.as_str().to_string()))?;
    Ok(ProductConversationTopology {
        root_conversation_id: ProductConversationId::parse(root_id)
            .map_err(|e| DbError::Serialization(e.to_string()))?,
        latest_conversation_id: latest,
        member_conversation_ids: members,
    })
}

async fn get_active_close_obligation_for_root_tx(
    tx: &mut Transaction<'_, Sqlite>,
    root_id: &str,
) -> DbResult<Option<CloseObligation>> {
    let row = sqlx::query(
        "SELECT attempt_id, root_conversation_id, phase, inspection_generation, inspection_fingerprint, created_at, updated_at, completed_at
         FROM close_obligations WHERE root_conversation_id = ?1 AND phase <> 'completed'",
    )
    .bind(root_id)
    .fetch_optional(&mut **tx)
    .await?;
    row.map(parse_close_obligation_row).transpose()
}

async fn get_close_obligation_optional_tx(
    tx: &mut Transaction<'_, Sqlite>,
    attempt_id: &str,
) -> DbResult<Option<CloseObligation>> {
    let row = sqlx::query(
        "SELECT attempt_id, root_conversation_id, phase, inspection_generation, inspection_fingerprint, created_at, updated_at, completed_at
         FROM close_obligations WHERE attempt_id = ?1",
    )
    .bind(attempt_id)
    .fetch_optional(&mut **tx)
    .await?;
    row.map(parse_close_obligation_row).transpose()
}

async fn get_close_obligation_tx(
    tx: &mut Transaction<'_, Sqlite>,
    attempt_id: &str,
) -> DbResult<CloseObligation> {
    get_close_obligation_optional_tx(tx, attempt_id)
        .await?
        .ok_or_else(|| DbError::CloseAttemptNotFound(attempt_id.to_string()))
}

async fn validate_inspection_belongs_to_aggregate(
    tx: &mut Transaction<'_, Sqlite>,
    root_id: &str,
    attempt_id: &str,
    inspections: &[ScopeInspectionInput],
) -> DbResult<()> {
    for inspection in inspections {
        validate_scope_membership(tx, root_id, attempt_id, &inspection.scope).await?;
    }
    Ok(())
}

fn validate_inspection_shape(
    aggregate_generation: Option<&str>,
    aggregate_fingerprint: Option<&str>,
    inspections: &[ScopeInspectionInput],
) -> DbResult<()> {
    match (aggregate_generation, aggregate_fingerprint) {
        (Some(generation), Some(fingerprint)) => {
            if inspections.is_empty() {
                if generation == no_worktree_generation()
                    && fingerprint == no_worktree_fingerprint()
                {
                    return Ok(());
                }
                return Err(DbError::CloseAttemptConflict(
                    "inspection aggregate cannot be non-empty without per-scope inspections"
                        .to_string(),
                ));
            }

            for inspection in inspections {
                if inspection.generation.as_deref() != Some(generation) {
                    return Err(DbError::CloseAttemptConflict(format!(
                        "scope {} generation must exactly match aggregate generation {generation}",
                        inspection.scope
                    )));
                }
                if inspection.fingerprint.as_deref().is_none() {
                    return Err(DbError::CloseAttemptConflict(format!(
                        "scope {} fingerprint must be present when aggregate inspection exists",
                        inspection.scope
                    )));
                }
            }
        }
        (None, None) => {
            if !inspections.is_empty() {
                return Err(DbError::CloseAttemptConflict(
                    "no-worktree inspection must not include per-scope inspections".to_string(),
                ));
            }
        }
        _ => unreachable!("pairing checked by caller"),
    }
    Ok(())
}

async fn validate_scope_membership(
    tx: &mut Transaction<'_, Sqlite>,
    root_id: &str,
    attempt_id: &str,
    scope: &WorkScopeId,
) -> DbResult<()> {
    let belongs: i64 = sqlx::query_scalar(
        "WITH RECURSIVE chain(id, next_id) AS (
            SELECT id, continued_in_conv_id FROM conversations WHERE id = ?1
            UNION ALL
            SELECT c.id, c.continued_in_conv_id FROM conversations c JOIN chain ON c.id = chain.next_id
         )
         SELECT EXISTS(SELECT 1 FROM chain JOIN conversations c ON c.id = chain.id WHERE c.work_scope_id = ?2)",
    )
    .bind(root_id)
    .bind(scope.as_str())
    .fetch_one(&mut **tx)
    .await?;
    if belongs == 0 {
        return Err(DbError::CloseScopeOutsideAggregate {
            attempt_id: attempt_id.to_string(),
            scope: scope.as_str().to_string(),
        });
    }
    Ok(())
}

fn no_worktree_generation() -> &'static str {
    "no-worktree"
}

fn no_worktree_fingerprint() -> &'static str {
    "no-worktree"
}

fn parse_close_obligation_row(row: sqlx::sqlite::SqliteRow) -> DbResult<CloseObligation> {
    let phase_raw: String = row.try_get("phase")?;
    let phase = ClosePhase::from_db_str(&phase_raw)
        .ok_or_else(|| DbError::Serialization(format!("unknown close phase {phase_raw}")))?;
    Ok(CloseObligation {
        attempt_id: row.try_get("attempt_id")?,
        product_conversation_id: ProductConversationId::parse(
            row.try_get::<String, _>("root_conversation_id")?,
        )
        .map_err(|e| DbError::Serialization(e.to_string()))?,
        phase,
        inspection_generation: row.try_get("inspection_generation")?,
        inspection_fingerprint: row.try_get("inspection_fingerprint")?,
        created_at: parse_dt(row.try_get("created_at")?)?,
        updated_at: parse_dt(row.try_get("updated_at")?)?,
        completed_at: row
            .try_get::<Option<String>, _>("completed_at")?
            .map(parse_dt)
            .transpose()?,
    })
}

fn parse_close_inspection_row(row: sqlx::sqlite::SqliteRow) -> DbResult<CloseInspection> {
    Ok(CloseInspection {
        attempt_id: row.try_get("attempt_id")?,
        scope: WorkScopeId::parse(row.try_get::<String, _>("scope")?)
            .map_err(|e| DbError::Serialization(e.to_string()))?,
        generation: row.try_get("generation")?,
        fingerprint: row.try_get("fingerprint")?,
    })
}

fn parse_close_inspection_loss_row(row: sqlx::sqlite::SqliteRow) -> DbResult<CloseInspectionLoss> {
    let category_raw: String = row.try_get("category")?;
    let category = match category_raw.as_str() {
        "staged_tracked_paths" => LossCategory::StagedTrackedPaths,
        "unstaged_tracked_paths" => LossCategory::UnstagedTrackedPaths,
        "untracked_non_ignored_paths" => LossCategory::UntrackedNonIgnoredPaths,
        "initialized_submodule_state" => LossCategory::InitializedSubmoduleState,
        "detached_unreachable_commits" => LossCategory::DetachedUnreachableCommits,
        _ => {
            return Err(DbError::Serialization(format!(
                "unknown loss category {category_raw}"
            )))
        }
    };
    Ok(CloseInspectionLoss {
        attempt_id: row.try_get("attempt_id")?,
        scope: WorkScopeId::parse(row.try_get::<String, _>("scope")?)
            .map_err(|e| DbError::Serialization(e.to_string()))?,
        generation: row.try_get("generation")?,
        category,
        item_identity: row.try_get("item_identity")?,
    })
}

fn parse_close_retired_resource_row(
    row: sqlx::sqlite::SqliteRow,
) -> DbResult<CloseRetiredResource> {
    let kind_raw: String = row.try_get("resource_kind")?;
    let resource_kind = match kind_raw.as_str() {
        "worktree" => RetiredResourceKind::Worktree,
        "bash_process_group" => RetiredResourceKind::BashProcessGroup,
        "tmux_server" => RetiredResourceKind::TmuxServer,
        "pty_session" => RetiredResourceKind::PtySession,
        "browser_session" => RetiredResourceKind::BrowserSession,
        "equivalent_live_resource" => RetiredResourceKind::EquivalentLiveResource,
        _ => {
            return Err(DbError::Serialization(format!(
                "unknown resource kind {kind_raw}"
            )))
        }
    };
    let outcome_raw: String = row.try_get("outcome")?;
    let failure_reason_raw: Option<String> = row.try_get("failure_reason")?;
    let outcome = match (outcome_raw.as_str(), failure_reason_raw.as_deref()) {
        ("retired", None) => RetirementOutcome::Retired,
        ("absence_adopted", None) => RetirementOutcome::AbsenceAdopted,
        ("residual", Some(reason)) => RetirementOutcome::Residual(parse_failure_reason(reason)?),
        _ => {
            return Err(DbError::Serialization(format!(
                "unknown retirement outcome {outcome_raw}"
            )))
        }
    };
    Ok(CloseRetiredResource {
        attempt_id: row.try_get("attempt_id")?,
        scope: WorkScopeId::parse(row.try_get::<String, _>("scope")?)
            .map_err(|e| DbError::Serialization(e.to_string()))?,
        resource_kind,
        resource_identity: row.try_get("resource_identity")?,
        outcome,
        detail: row.try_get("detail")?,
        created_at: parse_dt(row.try_get("created_at")?)?,
        updated_at: parse_dt(row.try_get("updated_at")?)?,
    })
}

async fn get_close_retired_resource_tx(
    tx: &mut Transaction<'_, Sqlite>,
    attempt_id: &str,
    scope: &WorkScopeId,
    resource_kind: RetiredResourceKind,
    resource_identity: &str,
) -> DbResult<CloseRetiredResource> {
    let row = sqlx::query(
        "SELECT attempt_id, scope, resource_kind, resource_identity, outcome, failure_reason, detail, created_at, updated_at
         FROM close_retirement_resources
         WHERE attempt_id = ?1 AND scope = ?2 AND resource_kind = ?3 AND resource_identity = ?4",
    )
    .bind(attempt_id)
    .bind(scope.as_str())
    .bind(resource_kind.as_str())
    .bind(resource_identity)
    .fetch_one(&mut **tx)
    .await?;
    parse_close_retired_resource_row(row)
}

fn parse_runtime_role(raw: String) -> DbResult<RuntimeRole> {
    RuntimeRole::from_db_str(&raw)
        .ok_or_else(|| DbError::Serialization(format!("unknown runtime role {raw}")))
}

fn encode_retirement_outcome(outcome: &RetirementOutcome) -> (&'static str, Option<&'static str>) {
    match outcome {
        RetirementOutcome::Retired => ("retired", None),
        RetirementOutcome::AbsenceAdopted => ("absence_adopted", None),
        RetirementOutcome::Residual(reason) => ("residual", Some(reason.as_str())),
    }
}

fn parse_failure_reason(reason: &str) -> DbResult<RetirementFailureReason> {
    Ok(match reason {
        "removal_failed" => RetirementFailureReason::RemovalFailed,
        "still_shared_by_live_owner" => RetirementFailureReason::StillSharedByLiveOwner,
        "residual_process_alive" => RetirementFailureReason::ResidualProcessAlive,
        "identity_not_proven" => RetirementFailureReason::IdentityNotProven,
        "manual_repair_required" => RetirementFailureReason::ManualRepairRequired,
        _ => {
            return Err(DbError::Serialization(format!(
                "unknown retirement failure reason {reason}"
            )))
        }
    })
}

fn parse_dt(raw: String) -> DbResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&raw)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| DbError::Serialization(e.to_string()))
}

fn close_tombstone_kind_str(kind: CloseTombstoneKind) -> &'static str {
    match kind {
        CloseTombstoneKind::Root => "root",
        CloseTombstoneKind::Continuation => "continuation",
    }
}

async fn delete_history_busy_blocker(
    tx: &mut Transaction<'_, Sqlite>,
    topology: &ProductConversationTopology,
) -> DbResult<Option<WorkScopeRetirementBlocker>> {
    for member in &topology.member_conversation_ids {
        let row = sqlx::query("SELECT runtime_role, state FROM conversations WHERE id = ?1")
            .bind(member.as_str())
            .fetch_one(&mut **tx)
            .await?;
        let runtime_role = parse_runtime_role(row.try_get("runtime_role")?)?;
        let state: ConvState = serde_json::from_str(&row.try_get::<String, _>("state")?)
            .map_err(|error| DbError::Serialization(error.to_string()))?;
        match runtime_role {
            RuntimeRole::User if state.is_busy() => {
                return Ok(Some(WorkScopeRetirementBlocker::CurrentUserOwner));
            }
            RuntimeRole::SubAgent if state.is_busy() => {
                return Ok(Some(WorkScopeRetirementBlocker::ActiveSubAgent));
            }
            RuntimeRole::User | RuntimeRole::SubAgent | RuntimeRole::Coordinator => {}
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Database;
    use sqlx::Row;

    async fn mk_conv(db: &Database, id: &str) -> WorkScopeId {
        db.create_conversation(id, id, "/tmp", true, None, None)
            .await
            .unwrap()
            .attached_work_scope_id
            .unwrap()
    }

    async fn set_continuation(db: &Database, from: &str, to: &str) {
        sqlx::query("UPDATE conversations SET continued_in_conv_id = ?1 WHERE id = ?2")
            .bind(to)
            .bind(from)
            .execute(db.pool())
            .await
            .unwrap();
    }

    async fn set_archived(db: &Database, id: &str, archived: bool) {
        sqlx::query("UPDATE conversations SET archived = ?1, updated_at = STRFTIME('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ?2")
            .bind(archived)
            .bind(id)
            .execute(db.pool())
            .await
            .unwrap();
    }

    async fn set_state(db: &Database, id: &str, state: &ConvState) {
        let state_json = serde_json::to_string(state).unwrap();
        let state_kind = crate::conv_state_kind(state);
        sqlx::query("UPDATE conversations SET state = ?1, state_kind = ?2, updated_at = STRFTIME('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ?3")
            .bind(state_json)
            .bind(state_kind)
            .bind(id)
            .execute(db.pool())
            .await
            .unwrap();
    }

    async fn obligation_phase(db: &Database, attempt: &str) -> String {
        sqlx::query_scalar("SELECT phase FROM close_obligations WHERE attempt_id = ?1")
            .bind(attempt)
            .fetch_one(db.pool())
            .await
            .unwrap()
    }

    async fn start_attempt(db: &Database, id: &str, attempt: &str) {
        db.begin_close(&ProductConversationId::parse(id).unwrap(), attempt)
            .await
            .unwrap();
    }

    async fn move_to_inspection(db: &Database, attempt: &str) {
        db.transition_close_phase(
            attempt,
            ClosePhase::AwaitingBlockerResolution,
            ClosePhase::SettlingActiveWork,
        )
        .await
        .unwrap();
        db.transition_close_phase(
            attempt,
            ClosePhase::SettlingActiveWork,
            ClosePhase::AwaitingRetirementInspection,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn topology_derives_root_latest_and_members() {
        let db = Database::open_in_memory().await.unwrap();
        let _root_scope = mk_conv(&db, "root").await;
        let _mid_scope = mk_conv(&db, "mid").await;
        let _leaf_scope = mk_conv(&db, "leaf").await;
        set_continuation(&db, "root", "mid").await;
        set_continuation(&db, "mid", "leaf").await;
        set_archived(&db, "root", true).await;
        let topology = db
            .product_conversation_topology(&ProductConversationId::parse("mid").unwrap())
            .await
            .unwrap();
        assert_eq!(topology.root_conversation_id.as_str(), "root");
        assert_eq!(topology.latest_conversation_id.as_str(), "leaf");
        assert_eq!(
            topology
                .member_conversation_ids
                .iter()
                .map(ProductConversationId::as_str)
                .collect::<Vec<_>>(),
            vec!["root", "mid", "leaf"]
        );
    }

    #[tokio::test]
    async fn begin_close_enforces_uniqueness_and_restart_listing() {
        let db = Database::open_in_memory().await.unwrap();
        let _scope = mk_conv(&db, "root").await;
        set_state(&db, "root", &ConvState::Idle).await;
        let started = db
            .begin_close(&ProductConversationId::parse("root").unwrap(), "a1")
            .await
            .unwrap();
        assert!(matches!(started, BeginCloseOutcome::Started(_)));
        let again = db
            .begin_close(&ProductConversationId::parse("root").unwrap(), "a1")
            .await
            .unwrap();
        assert!(matches!(again, BeginCloseOutcome::AlreadyStarted(_)));
        let err = db
            .begin_close(&ProductConversationId::parse("root").unwrap(), "a2")
            .await
            .unwrap_err();
        assert!(matches!(err, DbError::CloseAttemptConflict(_)));
        let pending = db.list_pending_close_restart_attempts().await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].attempt_id, "a1");
    }

    #[tokio::test]
    async fn exact_transitions_require_matching_current_phase() {
        let db = Database::open_in_memory().await.unwrap();
        let _scope = mk_conv(&db, "root").await;
        set_state(&db, "root", &ConvState::Idle).await;
        db.begin_close(&ProductConversationId::parse("root").unwrap(), "a1")
            .await
            .unwrap();
        let moved = db
            .transition_close_phase(
                "a1",
                ClosePhase::AwaitingBlockerResolution,
                ClosePhase::AwaitingStopWorkConfirmation,
            )
            .await
            .unwrap();
        assert_eq!(moved.phase, ClosePhase::AwaitingStopWorkConfirmation);
        let err = db
            .transition_close_phase(
                "a1",
                ClosePhase::AwaitingBlockerResolution,
                ClosePhase::SettlingActiveWork,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, DbError::ClosePhaseConflict { .. }));
    }

    #[tokio::test]
    async fn stale_confirmation_moves_back_to_inspection() {
        let db = Database::open_in_memory().await.unwrap();
        let scope = mk_conv(&db, "root").await;
        set_state(&db, "root", &ConvState::Idle).await;
        start_attempt(&db, "root", "a1").await;
        move_to_inspection(&db, "a1").await;
        db.replace_inspection(
            "a1",
            ClosePhase::AwaitingLossConfirmation,
            Some("g1"),
            Some("fp1"),
            vec![ScopeInspectionInput {
                scope: scope.clone(),
                generation: Some("g1".into()),
                fingerprint: Some("fp1".into()),
                losses: vec![],
            }],
        )
        .await
        .unwrap();
        let outcome = db
            .confirm_inspection("a1", "g2", "fp1", "g2", "fp1")
            .await
            .unwrap();
        assert!(matches!(outcome, ConfirmInspectionOutcome::Mismatch { .. }));
        assert_eq!(
            obligation_phase(&db, "a1").await,
            "awaiting_retirement_inspection"
        );
    }

    #[tokio::test]
    async fn inventory_replacement_deletes_stale_rows() {
        let db = Database::open_in_memory().await.unwrap();
        let scope = mk_conv(&db, "root").await;
        set_state(&db, "root", &ConvState::Idle).await;
        start_attempt(&db, "root", "a1").await;
        move_to_inspection(&db, "a1").await;
        db.replace_inspection(
            "a1",
            ClosePhase::AwaitingLossConfirmation,
            Some("g1"),
            Some("fp1"),
            vec![ScopeInspectionInput {
                scope: scope.clone(),
                generation: Some("g1".into()),
                fingerprint: Some("fp1".into()),
                losses: vec![LossRowInput {
                    category: LossCategory::StagedTrackedPaths,
                    item_identity: "x".into(),
                }],
            }],
        )
        .await
        .unwrap();
        let mismatch = db
            .confirm_inspection("a1", "stale", "fp1", "g1", "fp1")
            .await
            .unwrap();
        assert!(matches!(
            mismatch,
            ConfirmInspectionOutcome::Mismatch { .. }
        ));
        db.replace_inspection(
            "a1",
            ClosePhase::AwaitingLossConfirmation,
            Some("g2"),
            Some("fp2"),
            vec![ScopeInspectionInput {
                scope: scope.clone(),
                generation: Some("g2".into()),
                fingerprint: Some("fp2".into()),
                losses: vec![],
            }],
        )
        .await
        .unwrap();
        let losses = db.list_close_inspection_losses("a1").await.unwrap();
        assert!(losses.is_empty());
        let inspections = db.list_close_inspections("a1").await.unwrap();
        assert_eq!(inspections[0].generation.as_deref(), Some("g2"));
    }

    #[tokio::test]
    async fn resource_idempotency_is_typed() {
        let db = Database::open_in_memory().await.unwrap();
        let scope = mk_conv(&db, "root").await;
        set_state(&db, "root", &ConvState::Idle).await;
        start_attempt(&db, "root", "a1").await;
        let inserted = db
            .record_retirement_resource(
                "a1",
                &scope,
                RetiredResourceKind::Worktree,
                "wt",
                RetirementOutcome::Retired,
                None,
            )
            .await
            .unwrap();
        assert!(matches!(
            inserted,
            RecordRetirementResourceOutcome::Inserted(_)
        ));
        let unchanged = db
            .record_retirement_resource(
                "a1",
                &scope,
                RetiredResourceKind::Worktree,
                "wt",
                RetirementOutcome::Retired,
                None,
            )
            .await
            .unwrap();
        assert!(matches!(
            unchanged,
            RecordRetirementResourceOutcome::Unchanged(_)
        ));
    }

    #[tokio::test]
    async fn begin_close_rejects_archived_or_non_latest_and_keeps_state_out_of_band() {
        let db = Database::open_in_memory().await.unwrap();
        let _root_scope = mk_conv(&db, "root").await;
        let _leaf_scope = mk_conv(&db, "leaf").await;
        set_continuation(&db, "root", "leaf").await;
        set_archived(&db, "root", true).await;
        let err = db
            .begin_close(&ProductConversationId::parse("leaf").unwrap(), "archived")
            .await
            .unwrap_err();
        assert!(matches!(err, DbError::CloseAttemptConflict(_)));
        set_archived(&db, "root", false).await;
        let err = db
            .begin_close(&ProductConversationId::parse("root").unwrap(), "not-latest")
            .await
            .unwrap_err();
        assert!(matches!(err, DbError::CloseAttemptConflict(_)));
        start_attempt(&db, "leaf", "a1").await;
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM close_obligations WHERE phase = 'awaiting_blocker_resolution'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        let state_kind: String =
            sqlx::query_scalar("SELECT state_kind FROM conversations WHERE id = 'root'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(count, 1);
        assert_eq!(state_kind, crate::conv_state_kind(&ConvState::Idle));
    }

    #[tokio::test]
    async fn replace_inspection_enforces_exact_inventory_and_no_worktree_shape() {
        let db = Database::open_in_memory().await.unwrap();
        let scope = mk_conv(&db, "root").await;
        start_attempt(&db, "root", "a1").await;
        move_to_inspection(&db, "a1").await;
        let err = db
            .replace_inspection(
                "a1",
                ClosePhase::AwaitingLossConfirmation,
                Some("g1"),
                Some("agg"),
                vec![ScopeInspectionInput {
                    scope: scope.clone(),
                    generation: Some("g2".into()),
                    fingerprint: Some("fp1".into()),
                    losses: vec![],
                }],
            )
            .await
            .unwrap_err();
        assert!(matches!(err, DbError::CloseAttemptConflict(_)));

        let db = Database::open_in_memory().await.unwrap();
        let _scope = mk_conv(&db, "root").await;
        start_attempt(&db, "root", "a2").await;
        move_to_inspection(&db, "a2").await;
        let no_worktree = db
            .replace_inspection(
                "a2",
                ClosePhase::RetirementRequested,
                Some(no_worktree_generation()),
                Some(no_worktree_fingerprint()),
                vec![],
            )
            .await
            .unwrap();
        assert_eq!(no_worktree.phase, ClosePhase::RetirementRequested);
        assert_eq!(
            no_worktree.inspection_generation.as_deref(),
            Some(no_worktree_generation())
        );
        assert_eq!(
            no_worktree.inspection_fingerprint.as_deref(),
            Some(no_worktree_fingerprint())
        );

        let db = Database::open_in_memory().await.unwrap();
        let _scope = mk_conv(&db, "root").await;
        start_attempt(&db, "root", "b1").await;
        move_to_inspection(&db, "b1").await;
        let err = db
            .replace_inspection(
                "b1",
                ClosePhase::AwaitingLossConfirmation,
                None,
                None,
                vec![ScopeInspectionInput {
                    scope: WorkScopeId::parse("orphan").unwrap(),
                    generation: Some("g1".into()),
                    fingerprint: Some("fp1".into()),
                    losses: vec![],
                }],
            )
            .await
            .unwrap_err();
        assert!(matches!(err, DbError::CloseAttemptConflict(_)));

        let db = Database::open_in_memory().await.unwrap();
        let scope = mk_conv(&db, "root").await;
        start_attempt(&db, "root", "c1").await;
        move_to_inspection(&db, "c1").await;
        let err = db
            .replace_inspection(
                "c1",
                ClosePhase::AwaitingLossConfirmation,
                Some("g1"),
                Some("fp-agg"),
                vec![ScopeInspectionInput {
                    scope,
                    generation: Some("g1".into()),
                    fingerprint: None,
                    losses: vec![],
                }],
            )
            .await
            .unwrap_err();
        assert!(matches!(err, DbError::CloseAttemptConflict(_)));
    }

    #[tokio::test]
    async fn confirmation_requires_supplied_stored_and_recomputed_match() {
        let db = Database::open_in_memory().await.unwrap();
        let scope = mk_conv(&db, "root").await;
        start_attempt(&db, "root", "a1").await;
        move_to_inspection(&db, "a1").await;
        db.replace_inspection(
            "a1",
            ClosePhase::AwaitingLossConfirmation,
            Some("g1"),
            Some("fp1"),
            vec![ScopeInspectionInput {
                scope: scope.clone(),
                generation: Some("g1".into()),
                fingerprint: Some("fp1".into()),
                losses: vec![],
            }],
        )
        .await
        .unwrap();
        sqlx::query(
            "UPDATE close_retirement_inspections SET fingerprint = 'fp2' WHERE attempt_id = 'a1' AND scope = ?1",
        )
        .bind(scope.as_str())
        .execute(db.pool())
        .await
        .unwrap();
        let outcome = db
            .confirm_inspection("a1", "g1", "fp1", "g1", "fp2")
            .await
            .unwrap();
        assert!(matches!(outcome, ConfirmInspectionOutcome::Mismatch { .. }));
        let obligation = db.get_close_obligation("a1").await.unwrap().unwrap();
        assert_eq!(obligation.phase, ClosePhase::AwaitingRetirementInspection);
        assert_eq!(obligation.inspection_generation, None);
        assert_eq!(obligation.inspection_fingerprint, None);
    }

    #[tokio::test]
    async fn finalization_rolls_back_on_duplicate_message() {
        let db = Database::open_in_memory().await.unwrap();
        let scope = mk_conv(&db, "root").await;
        db.add_message(
            "dup",
            "root",
            &MessageContent::system("existing"),
            None,
            None,
        )
        .await
        .unwrap();
        set_state(&db, "root", &ConvState::Idle).await;
        start_attempt(&db, "root", "a1").await;
        move_to_inspection(&db, "a1").await;
        db.replace_inspection(
            "a1",
            ClosePhase::RetirementRequested,
            Some("g1"),
            Some("fp1"),
            vec![ScopeInspectionInput {
                scope: scope.clone(),
                generation: Some("g1".into()),
                fingerprint: Some("fp1".into()),
                losses: vec![],
            }],
        )
        .await
        .unwrap();
        let err = db.finalize_history("a1", "dup", "final").await.unwrap_err();
        assert!(matches!(err, DbError::CloseAttemptConflict(_)));
        let archived: bool =
            sqlx::query_scalar("SELECT archived FROM conversations WHERE id = 'root'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert!(
            !archived,
            "duplicate message id must leave archive unchanged"
        );
        assert_eq!(obligation_phase(&db, "a1").await, "retirement_requested");
    }

    #[tokio::test]
    async fn whole_delete_cleans_workflows_fts_and_tombstones_and_is_idempotent_and_rejects_open() {
        let db = Database::open_in_memory().await.unwrap();
        let _scope = mk_conv(&db, "root").await;
        let _leaf_scope = mk_conv(&db, "leaf").await;
        set_continuation(&db, "root", "leaf").await;
        db.add_message("m1", "root", &MessageContent::system("x"), None, None)
            .await
            .unwrap();
        db.add_message("m2", "leaf", &MessageContent::system("y"), None, None)
            .await
            .unwrap();
        let open_err = db
            .delete_history_aggregate(&ProductConversationId::parse("root").unwrap())
            .await
            .unwrap_err();
        let DbError::CloseDeleteBlocked(msg) = open_err else {
            panic!("expected CloseDeleteBlocked, got {open_err:?}");
        };
        assert_eq!(msg, "root conversation is not archived");

        let db = Database::open_in_memory().await.unwrap();
        let scope = mk_conv(&db, "root").await;
        let _leaf_scope = mk_conv(&db, "leaf").await;
        set_continuation(&db, "root", "leaf").await;
        db.add_message("m1", "root", &MessageContent::system("x"), None, None)
            .await
            .unwrap();
        db.add_message("m2", "leaf", &MessageContent::system("y"), None, None)
            .await
            .unwrap();
        sqlx::query("INSERT INTO workflows (workflow_id, profile_kind, profile_version, runtime_acceptance_enabled, external_acceptance_enabled, version, generation, status, snapshot_codec_family, snapshot_codec_version, snapshot_payload, created_at, updated_at) VALUES (901, 'wake', 1, 1, 0, 0, 0, 'Active', 'wake', 1, X'00', 1, 1)").execute(db.pool()).await.unwrap();
        sqlx::query("INSERT INTO workflow_effects (workflow_id, effect_id, declared_workflow_version, family, kind, intent_codec_family, intent_codec_version, intent_payload, generation, role, capability_kind, status) VALUES (901, 1, 0, 'wake', 'observe', 'wake', 1, X'00', 0, 'Required', 'ReclaimableObservation', 'Eligible')").execute(db.pool()).await.unwrap();
        sqlx::query("INSERT INTO wake_bindings (workflow_id, conversation_id, contract_id, profile_kind, profile_version, work_scope_id, resource_kind, bash_handle_id, registering_tool_use_id, expires_at, prepared_fingerprint, observe_effect_id, created_at) VALUES (901, 'leaf', 'contract', 'wake', 1, ?1, 'Bash', 'b-901', 'tool', 100, 'fp', 1, 1)").bind(scope.as_str()).execute(db.pool()).await.unwrap();
        set_archived(&db, "root", true).await;
        set_archived(&db, "leaf", true).await;
        set_state(
            &db,
            "root",
            &ConvState::ContextExhausted {
                summary: "done".into(),
            },
        )
        .await;
        set_state(
            &db,
            "leaf",
            &ConvState::ContextExhausted {
                summary: "done".into(),
            },
        )
        .await;
        let active_before: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM workflows WHERE workflow_id = 901")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(active_before, 1);
        let deleted = db
            .delete_history_aggregate(&ProductConversationId::parse("leaf").unwrap())
            .await
            .unwrap();
        assert!(matches!(
            deleted,
            DeleteHistoryAggregateOutcome::Deleted { .. }
        ));
        let wf_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM workflows WHERE workflow_id = 901")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(wf_count, 0);
        let fts_rows: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM message_fts_rows WHERE conversation_id IN ('root', 'leaf')",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(fts_rows, 0);
        let tombstones: Vec<(String, String)> = sqlx::query(
            "SELECT conversation_id, tombstone_kind FROM close_tombstones ORDER BY conversation_id",
        )
        .fetch_all(db.pool())
        .await
        .unwrap()
        .into_iter()
        .map(|row| (row.get("conversation_id"), row.get("tombstone_kind")))
        .collect();
        assert_eq!(
            tombstones,
            vec![
                ("leaf".into(), "continuation".into()),
                ("root".into(), "root".into())
            ]
        );
        let again = db
            .delete_history_aggregate(&ProductConversationId::parse("leaf").unwrap())
            .await
            .unwrap();
        assert!(matches!(
            again,
            DeleteHistoryAggregateOutcome::AlreadyDeleted { .. }
        ));
    }
}
