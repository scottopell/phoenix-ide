use phoenix_core::{
    domain::product_conversation::{ContinuationOpeningAuthority, ProductConversationId},
    work_scope::RuntimeRole,
};
use phoenix_workflow::ClientTurnKey;
use sha2::{Digest, Sha256};
use sqlx::{Connection, Row};
use std::fmt::Write as _;

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

#[derive(Clone, Debug)]
pub struct FederatedContinuationSource {
    pub destination_instance_id: phoenix_core::domain::instance_identity::InstanceId,
    pub product_conversation_id: ProductConversationId,
    pub transcript_id: phoenix_core::domain::close::TranscriptConversationId,
    pub message_id: String,
    pub tool_use_id: String,
}

pub struct FederatedCoordinatorContinuationRequest<'a> {
    pub authority: &'a crate::FederationSendAuthority,
    pub source: &'a FederatedContinuationSource,
    pub product_conversation_id: &'a ProductConversationId,
    pub predecessor_transcript_id: &'a str,
    pub summary_message_id: &'a str,
    pub first_message_id: ClientTurnKey,
}

enum ContinuationAdmission<'a> {
    LocalCurrentCoordinator(&'a str),
    FederatedPeer {
        authority: &'a crate::FederationSendAuthority,
        source: &'a FederatedContinuationSource,
    },
}

struct ContinuationReservation<'a> {
    admission: ContinuationAdmission<'a>,
    product_conversation_id: &'a ProductConversationId,
    predecessor_transcript_id: &'a str,
    summary_message_id: &'a str,
    first_message_id: ClientTurnKey,
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
        self.continue_from_coordinator_transaction(
            ContinuationReservation {
                admission: ContinuationAdmission::LocalCurrentCoordinator(
                    request.source_transcript_id,
                ),
                product_conversation_id: request.product_conversation_id,
                predecessor_transcript_id: request.predecessor_transcript_id,
                summary_message_id: request.summary_message_id,
                first_message_id: request.first_message_id,
            },
            ContinuationCommit::Acknowledge,
        )
        .await
    }

    /// # Errors
    /// Returns an inactive-authority error when enrollment is revoked before mutation.
    pub async fn continue_from_federated_coordinator(
        &self,
        request: FederatedCoordinatorContinuationRequest<'_>,
    ) -> LocalAuthorityResult<DbResult<(ContinueOutcome, CoordinatorContinuationOpening)>> {
        self.continue_from_coordinator_transaction(
            ContinuationReservation {
                admission: ContinuationAdmission::FederatedPeer {
                    authority: request.authority,
                    source: request.source,
                },
                product_conversation_id: request.product_conversation_id,
                predecessor_transcript_id: request.predecessor_transcript_id,
                summary_message_id: request.summary_message_id,
                first_message_id: request.first_message_id,
            },
            ContinuationCommit::Acknowledge,
        )
        .await
    }

    #[allow(clippy::too_many_lines)] // Ordered validation and commit classification share one transaction.
    async fn continue_from_coordinator_transaction(
        &self,
        request: ContinuationReservation<'_>,
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

        let federated = matches!(
            &request.admission,
            ContinuationAdmission::FederatedPeer { .. }
        );
        let result = async {
            match request.admission {
                ContinuationAdmission::LocalCurrentCoordinator(source) => {
                    crate::product_conversation_rename::require_current_gc_tx(&mut tx, source).await?;
                }
                ContinuationAdmission::FederatedPeer { authority, .. } => {
                    crate::federation_enrollment::require_federation_send_authority_tx(&mut tx, authority).await?;
                }
            }

            let existing_remote_receipt = if let ContinuationAdmission::FederatedPeer {
                authority,
                ..
            } = &request.admission
            {
                let receipt: Option<(String, String, String, String, String)> = sqlx::query_as(
                    "SELECT request_digest, product_conversation_id,
                            predecessor_transcript_id, summary_message_id,
                            successor_transcript_id
                     FROM federated_continuation_receipts
                     WHERE caller_instance_id = ?1 AND first_message_id = ?2",
                )
                .bind(authority.caller_instance_id().to_string())
                .bind(request.first_message_id.as_str())
                .fetch_optional(&mut *tx)
                .await?;
                if let Some((_, product, predecessor, summary, _)) = &receipt {
                    if product.as_str() != request.product_conversation_id.as_str()
                        || predecessor.as_str() != request.predecessor_transcript_id
                        || summary.as_str() != request.summary_message_id
                    {
                        return Err(DbError::FederationSendIdentityConflict);
                    }
                }
                receipt
            } else {
                None
            };

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

            let remote_reservation = if let ContinuationAdmission::FederatedPeer { authority, source } = &request.admission {
                let payload = serde_json::to_vec(&(
                    "federated-continuation-v1",
                    authority.caller_instance_id().to_string(),
                    source.destination_instance_id.to_string(),
                    source.product_conversation_id.as_str(),
                    source.transcript_id.as_str(),
                    &source.message_id,
                    &source.tool_use_id,
                    request.product_conversation_id.as_str(),
                    request.predecessor_transcript_id,
                    request.summary_message_id,
                    request.first_message_id.as_str(),
                    &summary,
                )).map_err(|error| DbError::Serialization(error.to_string()))?;

                let mut digest = String::with_capacity(64);
                for byte in Sha256::digest(payload) {
                    write!(&mut digest, "{byte:02x}").expect("writing to a String cannot fail");
                }
                let prior_successor = if let Some((
                    existing_digest,
                    _,
                    _,
                    _,
                    successor_transcript_id,
                )) = existing_remote_receipt
                {
                    if existing_digest != digest {
                        return Err(DbError::FederationSendIdentityConflict);
                    }
                    Some(successor_transcript_id)
                } else {
                    crate::federation_enrollment::require_federation_send_identity_owner_tx(
                        &mut tx,
                        request.first_message_id.as_str(),
                        "",
                        crate::federation_enrollment::FederationSendIdentityJournal::Continuation,
                    ).await?;
                    None
                };
                Some((authority.caller_instance_id(), digest, prior_successor))
            } else {
                let conflict: bool = sqlx::query_scalar(
                    "SELECT EXISTS(
                         SELECT 1 FROM continuation_dispatch_intents
                         WHERE message_id = ?1 AND parent_conversation_id <> ?2
                         UNION ALL
                         SELECT 1 FROM federated_continuation_receipts
                         WHERE first_message_id = ?1
                         UNION ALL
                         SELECT 1 FROM completed_continuation_handoffs
                         WHERE (accepted_successor_message_id = ?1
                            OR accepted_successor_message_id = successor_conversation_id || ':' || ?1)
                           AND predecessor_conversation_id <> ?2
                     )",
                )
                .bind(request.first_message_id.as_str())
                .bind(request.predecessor_transcript_id)
                .fetch_one(&mut *tx)
                .await?;
                if conflict {
                    return Err(DbError::FederationSendIdentityConflict);
                }
                None
            };

            let generated = NewContinuationDispatchIntent::generated_predecessor_context(
                request.first_message_id.clone(),
                summary.summary,
            );
            let outcome = self
                .continue_conversation_tx(
                    &mut tx,
                    request.predecessor_transcript_id,
                    (!federated).then_some(&generated),
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
            if federated {
                let completed_elsewhere: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM completed_continuation_handoffs
                     WHERE successor_conversation_id <> ?2
                       AND (accepted_successor_message_id = ?1
                         OR accepted_successor_message_id = successor_conversation_id || ':' || ?1))",
                )
                .bind(request.first_message_id.as_str())
                .bind(successor_id)
                .fetch_one(&mut *tx)
                .await?;
                if completed_elsewhere {
                    return Err(DbError::FederationSendIdentityConflict);
                }
                if matches!(&outcome, ContinueOutcome::Created(_)) {
                    sqlx::query(
                        "INSERT INTO continuation_dispatch_intents (
                             parent_conversation_id, successor_conversation_id, message_id,
                             handoff, user_agent, opening_authority, created_at
                         ) VALUES (?1, ?2, ?3, ?4, NULL, ?5, CURRENT_TIMESTAMP)",
                    )
                    .bind(request.predecessor_transcript_id)
                    .bind(successor_id)
                    .bind(generated.message_id.as_str())
                    .bind(&generated.handoff)
                    .bind(generated.opening_authority.as_str())
                    .execute(&mut *tx)
                    .await?;
                }
            }
            let winning = Self::continuation_dispatch_intent_on(
                &mut *tx,
                request.predecessor_transcript_id,
            )
            .await?;

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
                    if federated && client_turn_key != request.first_message_id {
                        return Err(DbError::ContinuationPrecondition(
                            "opening identity differs from the requested first message".to_string(),
                        ));
                    }
                    (edge, CoordinatorContinuationOpening::Pending(winning))
                }
                crate::ContinuationOpeningRead::Accepted {
                    edge,
                    accepted_message_id,
                    opening_authority: ContinuationOpeningAuthority::GeneratedPredecessorContext,
                    presentation: _,
                } => {
                    let key = request.first_message_id.as_str();
                    if federated
                        && accepted_message_id != key
                        && accepted_message_id != format!("{successor_id}:{key}")
                    {
                        return Err(DbError::ContinuationPrecondition(
                            "opening identity differs from the requested first message".to_string(),
                        ));
                    }
                    (edge, CoordinatorContinuationOpening::Accepted { accepted_message_id })
                },
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
            if let Some((caller, digest, prior_successor)) = remote_reservation {
                if let Some(prior_successor) = prior_successor {
                    if prior_successor != *successor_id {
                        return Err(DbError::FederationSendIdentityConflict);
                    }
                } else {
                    sqlx::query(
                        "INSERT INTO federated_continuation_receipts
                         (caller_instance_id, first_message_id, request_digest,
                          product_conversation_id, predecessor_transcript_id,
                          summary_message_id, successor_transcript_id)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    )
                    .bind(caller.to_string())
                    .bind(request.first_message_id.as_str())
                    .bind(digest)
                    .bind(request.product_conversation_id.as_str())
                    .bind(request.predecessor_transcript_id)
                    .bind(request.summary_message_id)
                    .bind(successor_id)
                    .execute(&mut *tx)
                    .await?;
                }
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
