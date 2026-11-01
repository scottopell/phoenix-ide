use phoenix_core::{
    domain::product_conversation::{ContinuationOpeningAuthority, ProductConversationId},
    work_scope::RuntimeRole,
};
use phoenix_workflow::ClientTurnKey;
use sqlx::{Connection, Row};

use crate::{
    workflow::LocalAuthorityResult, ContinuationDispatchIntent, ContinueOutcome, ConvState,
    Database, DbError, DbResult, MessageContent, MessageType, NewContinuationDispatchIntent,
};

pub struct CurrentCoordinatorContinuationRequest<'a> {
    pub source_transcript_id: &'a str,
    pub product_conversation_id: &'a ProductConversationId,
    pub predecessor_transcript_id: &'a str,
    pub summary_message_id: &'a str,
    pub first_message_id: ClientTurnKey,
}

#[derive(Debug)]
pub enum CoordinatorContinuationOpening {
    Pending(ContinuationDispatchIntent),
    Accepted { accepted_message_id: String },
}

#[derive(Clone, Copy)]
enum ContinuationCommit {
    Acknowledge,
    #[cfg(test)]
    LoseAcknowledgement,
}

impl Database {
    /// Reserve or inspect one ordinary continuation using the current Coordinator source.
    ///
    /// # Errors
    /// Returns established validation/storage errors after acknowledged rollback;
    /// an unacknowledged commit or rollback leaves the durable fact unclassified.
    pub async fn continue_from_current_coordinator(
        &self,
        request: CurrentCoordinatorContinuationRequest<'_>,
    ) -> LocalAuthorityResult<DbResult<(ContinueOutcome, CoordinatorContinuationOpening)>> {
        self.continue_from_current_coordinator_with_commit(request, ContinuationCommit::Acknowledge)
            .await
    }

    #[allow(clippy::too_many_lines)] // Ordered validation and commit classification share one transaction.
    async fn continue_from_current_coordinator_with_commit(
        &self,
        request: CurrentCoordinatorContinuationRequest<'_>,
        commit: ContinuationCommit,
    ) -> LocalAuthorityResult<DbResult<(ContinueOutcome, CoordinatorContinuationOpening)>> {
        use LocalAuthorityResult::{
            DurableFactEstablished as Established, DurableFactUnclassified as Unclassified,
        };

        let mut connection = match self.pool.acquire().await {
            Ok(connection) => connection,
            Err(error) => return Established(Err(error.into())),
        };
        let mut tx = match connection.begin_with("BEGIN IMMEDIATE").await {
            Ok(tx) => tx,
            Err(error) => return Established(Err(error.into())),
        };

        let result = async {
            crate::product_conversation_rename::require_current_gc_tx(
                &mut tx,
                request.source_transcript_id,
            )
            .await?;

            let resolved = Self::resolve_ordinary_product_conversation_on(
                &mut tx,
                request.product_conversation_id.as_str(),
            )
            .await?;
            if resolved.product_conversation_id != *request.product_conversation_id {
                return Err(DbError::ContinuationPrecondition(
                    "requested identifier did not resolve to the requested ordinary product conversation".to_string(),
                ));
            }

            crate::close_foundation::require_product_conversation_admission_tx(
                &mut tx,
                request.predecessor_transcript_id,
            )
            .await?;

            let parent = Self::get_conversation_on(&mut *tx, request.predecessor_transcript_id).await?;
            if parent.product_conversation_id != *request.product_conversation_id
                || parent.runtime_role != RuntimeRole::User
                || !parent.user_initiated
                || parent.parent_conversation_id.is_some()
            {
                return Err(DbError::ContinuationPrecondition(
                    "predecessor is not a user-initiated root transcript of the requested product conversation".to_string(),
                ));
            }
            let expected_summary = match &parent.state {
                ConvState::ContextExhausted { summary } if !summary.trim().is_empty() => summary,
                ConvState::Idle
                | ConvState::LlmRequesting { .. }
                | ConvState::ServerOverloadRetrying { .. }
                | ConvState::SeededLlmRequesting { .. }
                | ConvState::Provisioning { .. }
                | ConvState::CreationCancelled { .. }
                | ConvState::ToolExecuting { .. }
                | ConvState::CancellingTool { .. }
                | ConvState::AwaitingSubAgents { .. }
                | ConvState::CancellingSubAgents { .. }
                | ConvState::Completed { .. }
                | ConvState::Failed { .. }
                | ConvState::CreationFailed { .. }
                | ConvState::Error { .. }
                | ConvState::AwaitingRecovery { .. }
                | ConvState::AwaitingContinuation { .. }
                | ConvState::RecoverableContinuationFailure { .. }
                | ConvState::AwaitingTaskApproval { .. }
                | ConvState::AwaitingUserResponse { .. }
                | ConvState::ContextExhausted { .. }
                | ConvState::HandedOff { .. }
                | ConvState::Terminal => {
                    return Err(DbError::ContinuationPrecondition(
                        "predecessor must be strictly ContextExhausted with a nonempty summary".to_string(),
                    ));
                }
            };

            let summary_row = sqlx::query(
                "SELECT message_type, content, origin_kind FROM messages
                 WHERE conversation_id = ?1 AND message_id = ?2",
            )
            .bind(request.predecessor_transcript_id)
            .bind(request.summary_message_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| DbError::ContinuationPrecondition(
                "summary message does not belong to the predecessor transcript".to_string(),
            ))?;
            let message_type: String = summary_row.try_get("message_type")?;
            let origin_kind: String = summary_row.try_get("origin_kind")?;
            if message_type != MessageType::Continuation.to_string()
                || origin_kind != "system_generated"
            {
                return Err(DbError::ContinuationPrecondition(
                    "summary message is not a system-generated continuation message".to_string(),
                ));
            }
            let stored_content: String = summary_row.try_get("content")?;
            let stored_json = serde_json::from_str(&stored_content)
                .map_err(|error| DbError::Serialization(error.to_string()))?;
            let MessageContent::Continuation(summary) = MessageContent::from_stored_json(
                MessageType::Continuation,
                stored_json,
            )
            .map_err(DbError::Serialization)? else {
                return Err(DbError::ContinuationPrecondition(
                    "summary message is not a system-generated continuation message".to_string(),
                ));
            };
            if summary.summary.trim().is_empty()
                || summary.summary.as_str() != expected_summary.as_str()
            {
                return Err(DbError::ContinuationPrecondition(
                    "persisted continuation summary does not match the predecessor state".to_string(),
                ));
            }

            let generated = NewContinuationDispatchIntent::generated_predecessor_context(
                request.first_message_id.clone(),
                summary.summary,
            );
            let outcome = self
                .continue_conversation_tx(&mut tx, request.predecessor_transcript_id, Some(&generated))
                .await?;
            let winning = Self::continuation_dispatch_intent_on(
                &mut *tx,
                request.predecessor_transcript_id,
            )
            .await?;

            let successor_id = match &outcome {
                ContinueOutcome::Created(successor) | ContinueOutcome::AlreadyContinued(successor) => &successor.id,
                ContinueOutcome::ParentNotContextExhausted { .. } => {
                    return Err(DbError::ContinuationPrecondition(
                        "predecessor ceased to be ContextExhausted during continuation".to_string(),
                    ));
                }
            };
            if let Some(winning) = &winning {
                if winning.successor_conversation_id != *successor_id
                    || winning.opening_authority
                        != ContinuationOpeningAuthority::GeneratedPredecessorContext
                    || winning.handoff != *expected_summary
                    || winning.user_agent.is_some()
                {
                    return Err(DbError::ContinuationPrecondition(
                        "winning intent is not the exact generated-context opening".to_string(),
                    ));
                }
            }
            let read = Self::read_continuation_opening_on(&mut tx, successor_id).await?;
            let (edge, opening) = match read {
                crate::ContinuationOpeningRead::Pending {
                    edge,
                    client_turn_key,
                    opening_authority: ContinuationOpeningAuthority::GeneratedPredecessorContext,
                } => {
                    let winning = winning.ok_or_else(|| DbError::ContinuationPrecondition(
                        "pending opening lacks its dispatch intent".to_string(),
                    ))?;
                    if winning.message_id != client_turn_key {
                        return Err(DbError::ContinuationPrecondition(
                            "pending opening key differs from the dispatch intent".to_string(),
                        ));
                    }
                    (edge, CoordinatorContinuationOpening::Pending(winning))
                }
                crate::ContinuationOpeningRead::Accepted {
                    edge,
                    accepted_message_id,
                    opening_authority: ContinuationOpeningAuthority::GeneratedPredecessorContext,
                    presentation: _,
                } => (edge, CoordinatorContinuationOpening::Accepted { accepted_message_id }),
                crate::ContinuationOpeningRead::None
                | crate::ContinuationOpeningRead::Pending { .. }
                | crate::ContinuationOpeningRead::Accepted { .. } => {
                    return Err(DbError::ContinuationPrecondition(
                        "continuation lacks an exact generated-context opening".to_string(),
                    ));
                }
            };
            if edge.product_conversation_id != *request.product_conversation_id
                || edge.predecessor_transcript_row_id != request.predecessor_transcript_id
                || edge.successor_transcript_row_id != *successor_id
                || edge.continuation_message_id != request.summary_message_id
            {
                return Err(DbError::ContinuationPrecondition(
                    "opening is bound to a different continuation boundary".to_string(),
                ));
            }
            Ok((outcome, opening))
        }
        .await;

        match result {
            Ok(value) => {
                let committed = tx.commit().await;
                let acknowledged = match commit {
                    ContinuationCommit::Acknowledge => committed,
                    #[cfg(test)]
                    ContinuationCommit::LoseAcknowledgement => {
                        committed.and(Err(sqlx::Error::WorkerCrashed))
                    }
                };
                match acknowledged {
                    Ok(()) => Established(Ok(value)),
                    Err(_) => Unclassified,
                }
            }
            Err(error) => match tx.rollback().await {
                Ok(()) => Established(Err(error)),
                Err(_) => Unclassified,
            },
        }
    }
}

#[cfg(test)]
mod tests;
