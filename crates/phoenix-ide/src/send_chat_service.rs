use crate::api::{record_pr_auto_fix_context_baseline, validate_submitted_attachments};
use crate::api::{FileAttachment, ImageAttachment};
use crate::db::ConvState;
use crate::runtime::{ChatAcceptanceReceipt, RuntimeManager};
use crate::state_machine::{check_user_message_acceptable, Event, TransitionError};
use phoenix_core::domain::db_schema::ImageData;
use phoenix_core::domain::skill_invocation::SkillInvocation;
use std::fmt::Write as _;
use std::sync::Arc;

const MAX_STEER_QUEUE_DEPTH: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MessageExpansionPolicy {
    ExpandReferences,
    LiteralText,
}

#[derive(Debug, Clone)]
pub(crate) struct SendChatRequest {
    pub conversation_id: String,
    pub text: String,
    pub message_id: String,
    pub images: Vec<ImageAttachment>,
    pub files: Vec<FileAttachment>,
    pub user_agent: Option<String>,
    pub expansion_policy: MessageExpansionPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SendChatRequestResult {
    Created,
    Replayed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SendChatDisposition {
    PendingRuntime,
    RuntimeAccepted,
    QueuedSteering,
    CancelledSteering,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SendChatOutcome {
    Accepted {
        message_id: String,
        request_result: SendChatRequestResult,
        disposition: SendChatDisposition,
    },
    Rejected {
        message: String,
        code: &'static str,
    },
}

impl SendChatOutcome {
    fn accepted(
        message_id: String,
        request_result: SendChatRequestResult,
        disposition: SendChatDisposition,
    ) -> Self {
        Self::Accepted {
            message_id,
            request_result,
            disposition,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum SendChatServiceError {
    #[error("conversation not found: {0}")]
    NotFound(String),
    #[error("attachment validation failed: {0}")]
    AttachmentValidation(String),
    #[error("message expansion failed: {message}")]
    Expansion {
        message: String,
        error_type: &'static str,
        reference: String,
    },
    #[error("internal error: {0}")]
    Internal(String),
    #[error("dispatch failed: {0}")]
    Dispatch(String),
    #[error("message_id was already used for a different target or payload")]
    IdempotencyConflict,
}

#[derive(Clone)]
pub(crate) struct SendChatApplicationService {
    db: crate::db::Database,
    runtime: Arc<RuntimeManager>,
}

impl SendChatApplicationService {
    pub(crate) fn new(db: crate::db::Database, runtime: Arc<RuntimeManager>) -> Self {
        Self { db, runtime }
    }

    #[allow(clippy::too_many_lines)]
    pub(crate) async fn send(
        &self,
        req: SendChatRequest,
    ) -> Result<SendChatOutcome, SendChatServiceError> {
        let request_fingerprint = request_fingerprint(&req)?;
        let mut receipts = self.runtime.lock_chat_acceptance().await;
        let conversation = self
            .runtime
            .db()
            .get_conversation(&req.conversation_id)
            .await
            .map_err(|e| SendChatServiceError::NotFound(e.to_string()))?;
        if let Some(accepted) = phoenix_db::WorkflowRepository::new(self.db.pool().clone())
            .load_direct_turn_acceptance(&conversation.id, &req.message_id)
            .await
            .map_err(|error| SendChatServiceError::Internal(error.to_string()))?
        {
            let prepared: phoenix_core::domain::sm_event::PreparedDirectTurn =
                serde_json::from_str(&accepted.prepared_payload)
                    .map_err(|error| SendChatServiceError::Internal(error.to_string()))?;
            if !prepared_retry_matches_request(&prepared, &req) {
                return Err(SendChatServiceError::IdempotencyConflict);
            }
            return self
                .replay_durable_acceptance(
                    &conversation,
                    &req,
                    &request_fingerprint,
                    accepted,
                    &mut receipts,
                )
                .await;
        }
        if let Ok(message) = self.db.get_message_by_id(&req.message_id).await {
            if message.conversation_id == conversation.id {
                let persisted_matches = match &message.content {
                    phoenix_core::domain::db_schema::MessageContent::Skill(skill) => {
                        persisted_skill_matches(skill, &req)
                    }
                    content @ phoenix_core::domain::db_schema::MessageContent::User(_) => {
                        persisted_user_message_matches(content, &req)
                    }
                    phoenix_core::domain::db_schema::MessageContent::Agent(_)
                    | phoenix_core::domain::db_schema::MessageContent::Tool(_)
                    | phoenix_core::domain::db_schema::MessageContent::System(_)
                    | phoenix_core::domain::db_schema::MessageContent::Error(_)
                    | phoenix_core::domain::db_schema::MessageContent::Continuation(_) => false,
                };
                if !persisted_matches {
                    return Err(SendChatServiceError::IdempotencyConflict);
                }
                receipts.remove(&(conversation.id.clone(), req.message_id.clone()));
                return Ok(SendChatOutcome::accepted(
                    req.message_id,
                    SendChatRequestResult::Replayed,
                    SendChatDisposition::RuntimeAccepted,
                ));
            }
        }
        if let Some(receipt) = receipts.get(&(conversation.id.clone(), req.message_id.clone())) {
            return replay_receipt(receipt, &req, &request_fingerprint);
        }
        if let Some(queued_entry) =
            find_queued_message(&self.db, &req.conversation_id, &req.message_id).await?
        {
            if !queued_retry_matches(&queued_entry, &req) {
                return Err(SendChatServiceError::IdempotencyConflict);
            }
            receipts.remove(&(conversation.id.clone(), req.message_id.clone()));
            return Ok(SendChatOutcome::accepted(
                req.message_id,
                SendChatRequestResult::Replayed,
                SendChatDisposition::QueuedSteering,
            ));
        }
        if conversation.archived {
            return Ok(SendChatOutcome::Rejected {
                message: "Conversation is archived and unavailable for messaging.".to_string(),
                code: "target_unavailable",
            });
        }
        let steering_queue = self
            .runtime
            .db()
            .get_steering_queue(&req.conversation_id)
            .await
            .map_err(|e| SendChatServiceError::NotFound(e.to_string()))?;

        let effective_state = self
            .effective_state(&conversation.id, &conversation.state)
            .await?;
        if let Err(err) = check_user_message_acceptable(&effective_state) {
            if matches!(
                err,
                TransitionError::AgentBusy | TransitionError::CancellationInProgress
            ) {
                if steering_queue.len() >= MAX_STEER_QUEUE_DEPTH {
                    return Ok(SendChatOutcome::Rejected {
                        message:
                            "Steering queue is full; try again once a queued message has been delivered."
                                .to_string(),
                        code: "steering_queue_full",
                    });
                }
                let validated_files = validate_files(&req).await?;
                let expanded = expand_request(&self.db, &conversation, &req).await?;
                let images = map_images(req.images.clone());
                let prepared = phoenix_core::domain::sm_event::PreparedDirectTurn {
                    codec_version:
                        phoenix_core::domain::sm_event::PREPARED_DIRECT_TURN_CODEC_VERSION,
                    expand_references: req.expansion_policy
                        == MessageExpansionPolicy::ExpandReferences,
                    text: expanded.display_text.clone(),
                    llm_text: expanded.llm_text.clone(),
                    images: images.clone(),
                    files: validated_files.clone(),
                    message_id: req.message_id.clone(),
                    user_agent: req.user_agent.clone(),
                    skill_invocation: expanded.skill_invocation.clone(),
                };
                let prepared_payload = serde_json::to_string(&prepared)
                    .map_err(|error| SendChatServiceError::Internal(error.to_string()))?;
                let prepared_fingerprint = prepared_direct_turn_fingerprint(&prepared_payload);
                let steering_entry = crate::state_machine::event::SteerEntry {
                    text: expanded.display_text.clone(),
                    llm_text: expanded.llm_text.clone(),
                    images: images.clone(),
                    files: validated_files.clone(),
                    message_id: req.message_id.clone(),
                    user_agent: req.user_agent.clone(),
                    skill_invocation: expanded.skill_invocation.clone(),
                };
                let acceptance = phoenix_db::WorkflowRepository::new(self.db.pool().clone())
                    .accept_direct_turn(&phoenix_db::DirectTurnAcceptanceInput {
                        initial_outcome: phoenix_db::DirectTurnInitialOutcome::QueuedSteering {
                            entry: Box::new(steering_entry),
                        },
                        conversation_id: conversation.id.clone(),
                        client_message_id: req.message_id.clone(),
                        prepared_fingerprint,
                        prepared_payload,
                        accepted_at: phoenix_workflow::Timestamp(
                            u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or(0),
                        ),
                        snapshot: phoenix_workflow::llm_profile::TopLevelLlmSnapshot {
                            turn_ref: phoenix_workflow::llm_profile::TopLevelTurnRef {
                                conversation_id: conversation.id.clone(),
                                accepted_turn_id: req.message_id.clone(),
                                generation: 0,
                            },
                            accepted_assistant_message_id: None,
                            stopped_at: None,
                        },
                    })
                    .await
                    .map_err(|error| SendChatServiceError::Internal(error.to_string()))?;
                let (acceptance, request_result) = match acceptance {
                    phoenix_db::DirectTurnAcceptanceOutcome::Created(record) => {
                        (record, SendChatRequestResult::Created)
                    }
                    phoenix_db::DirectTurnAcceptanceOutcome::Replayed(record) => {
                        (record, SendChatRequestResult::Replayed)
                    }
                    phoenix_db::DirectTurnAcceptanceOutcome::RetryablePersistence => {
                        return Err(SendChatServiceError::Dispatch(
                            "direct turn persistence is temporarily busy".to_string(),
                        ));
                    }
                    phoenix_db::DirectTurnAcceptanceOutcome::Conflict => {
                        return Err(SendChatServiceError::IdempotencyConflict);
                    }
                };
                debug_assert_eq!(
                    acceptance.committed_outcome,
                    phoenix_db::DirectTurnCommittedOutcome::QueuedSteering
                );
                let event = Event::SteerMessage {
                    text: expanded.display_text.clone(),
                    llm_text: expanded.llm_text,
                    images,
                    files: validated_files.clone(),
                    message_id: req.message_id.clone(),
                    user_agent: req.user_agent,
                    skill_invocation: expanded.skill_invocation,
                };
                kick_runtime_delivery(self.runtime.clone(), conversation.id.clone(), event);
                insert_transient_receipt(
                    &self.db,
                    &mut receipts,
                    (conversation.id.clone(), req.message_id.clone()),
                    ChatAcceptanceReceipt {
                        conversation_id: conversation.id.clone(),
                        request_fingerprint,
                        steering: true,
                    },
                )
                .await;
                if let Err(error) = record_pr_auto_fix_context_baseline(
                    self.runtime.db(),
                    &conversation.id,
                    &expanded.display_text,
                )
                .await
                {
                    tracing::warn!(conversation_id = %conversation.id, error = ?error, "Message accepted but PR auto-fix baseline recording failed");
                }
                return Ok(SendChatOutcome::accepted(
                    req.message_id,
                    request_result,
                    SendChatDisposition::QueuedSteering,
                ));
            }
            return Ok(SendChatOutcome::Rejected {
                message: err.to_string(),
                code: transition_code(&err),
            });
        }

        let validated_files = validate_files(&req).await?;
        let expanded = expand_request(&self.db, &conversation, &req).await?;
        let images = map_images(req.images);
        let prepared = phoenix_core::domain::sm_event::PreparedDirectTurn {
            codec_version: phoenix_core::domain::sm_event::PREPARED_DIRECT_TURN_CODEC_VERSION,
            expand_references: req.expansion_policy == MessageExpansionPolicy::ExpandReferences,
            text: expanded.display_text.clone(),
            llm_text: expanded.llm_text.clone(),
            images: images.clone(),
            files: validated_files.clone(),
            message_id: req.message_id.clone(),
            user_agent: req.user_agent.clone(),
            skill_invocation: expanded.skill_invocation.clone(),
        };
        let prepared_payload = serde_json::to_string(&prepared)
            .map_err(|error| SendChatServiceError::Internal(error.to_string()))?;
        let prepared_fingerprint = prepared_direct_turn_fingerprint(&prepared_payload);
        let event = Event::UserMessage {
            text: expanded.display_text.clone(),
            llm_text: expanded.llm_text,
            images,
            files: validated_files,
            message_id: req.message_id.clone(),
            user_agent: req.user_agent,
            skill_invocation: expanded.skill_invocation,
        };
        let acceptance = phoenix_db::WorkflowRepository::new(self.db.pool().clone())
            .accept_direct_turn(&phoenix_db::DirectTurnAcceptanceInput {
                initial_outcome: phoenix_db::DirectTurnInitialOutcome::PendingRuntime,
                conversation_id: conversation.id.clone(),
                client_message_id: req.message_id.clone(),
                prepared_fingerprint,
                prepared_payload: prepared_payload.clone(),
                accepted_at: phoenix_workflow::Timestamp(
                    u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or(0),
                ),
                snapshot: phoenix_workflow::llm_profile::TopLevelLlmSnapshot {
                    turn_ref: phoenix_workflow::llm_profile::TopLevelTurnRef {
                        conversation_id: conversation.id.clone(),
                        accepted_turn_id: req.message_id.clone(),
                        generation: 0,
                    },
                    accepted_assistant_message_id: None,
                    stopped_at: None,
                },
            })
            .await
            .map_err(|error| SendChatServiceError::Internal(error.to_string()))?;
        let request_result = match acceptance {
            phoenix_db::DirectTurnAcceptanceOutcome::Created(_) => SendChatRequestResult::Created,
            phoenix_db::DirectTurnAcceptanceOutcome::Replayed(_) => SendChatRequestResult::Replayed,
            phoenix_db::DirectTurnAcceptanceOutcome::RetryablePersistence => {
                return Err(SendChatServiceError::Dispatch(
                    "direct turn persistence is temporarily busy".to_string(),
                ));
            }
            phoenix_db::DirectTurnAcceptanceOutcome::Conflict => {
                if steering_queue.len() >= MAX_STEER_QUEUE_DEPTH {
                    return Ok(SendChatOutcome::Rejected {
                        message:
                            "Steering queue is full; try again once a queued message has been delivered."
                                .to_string(),
                        code: "steering_queue_full",
                    });
                }
                let queued = phoenix_db::WorkflowRepository::new(self.db.pool().clone())
                    .accept_direct_turn(&phoenix_db::DirectTurnAcceptanceInput {
                        initial_outcome: phoenix_db::DirectTurnInitialOutcome::QueuedSteering {
                            entry: Box::new(steer_entry_from_prepared(&prepared)),
                        },
                        conversation_id: conversation.id.clone(),
                        client_message_id: req.message_id.clone(),
                        prepared_fingerprint: prepared_direct_turn_fingerprint(&prepared_payload),
                        prepared_payload,
                        accepted_at: phoenix_workflow::Timestamp(
                            u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or(0),
                        ),
                        snapshot: phoenix_workflow::llm_profile::TopLevelLlmSnapshot {
                            turn_ref: phoenix_workflow::llm_profile::TopLevelTurnRef {
                                conversation_id: conversation.id.clone(),
                                accepted_turn_id: req.message_id.clone(),
                                generation: 0,
                            },
                            accepted_assistant_message_id: None,
                            stopped_at: None,
                        },
                    })
                    .await
                    .map_err(|error| SendChatServiceError::Internal(error.to_string()))?;
                let request_result = match queued {
                    phoenix_db::DirectTurnAcceptanceOutcome::Created(_) => {
                        SendChatRequestResult::Created
                    }
                    phoenix_db::DirectTurnAcceptanceOutcome::Replayed(_) => {
                        SendChatRequestResult::Replayed
                    }
                    phoenix_db::DirectTurnAcceptanceOutcome::RetryablePersistence => {
                        return Err(SendChatServiceError::Dispatch(
                            "queued steering persistence is temporarily busy".to_string(),
                        ));
                    }
                    phoenix_db::DirectTurnAcceptanceOutcome::Conflict => {
                        return Err(SendChatServiceError::IdempotencyConflict);
                    }
                };
                let steer = steer_entry_from_prepared(&prepared);
                kick_runtime_delivery(
                    self.runtime.clone(),
                    conversation.id.clone(),
                    Event::SteerMessage {
                        text: steer.text,
                        llm_text: steer.llm_text,
                        images: steer.images,
                        files: steer.files,
                        message_id: steer.message_id,
                        user_agent: steer.user_agent,
                        skill_invocation: steer.skill_invocation,
                    },
                );
                return Ok(SendChatOutcome::accepted(
                    req.message_id,
                    request_result,
                    SendChatDisposition::QueuedSteering,
                ));
            }
        };
        kick_runtime_delivery(self.runtime.clone(), conversation.id.clone(), event);
        insert_transient_receipt(
            &self.db,
            &mut receipts,
            (conversation.id.clone(), req.message_id.clone()),
            ChatAcceptanceReceipt {
                conversation_id: conversation.id.clone(),
                request_fingerprint,
                steering: false,
            },
        )
        .await;
        if let Err(error) = record_pr_auto_fix_context_baseline(
            self.runtime.db(),
            &conversation.id,
            &expanded.display_text,
        )
        .await
        {
            tracing::warn!(conversation_id = %conversation.id, error = ?error, "Message accepted but PR auto-fix baseline recording failed");
        }
        Ok(SendChatOutcome::accepted(
            req.message_id,
            request_result,
            SendChatDisposition::PendingRuntime,
        ))
    }

    async fn replay_durable_acceptance(
        &self,
        conversation: &crate::db::Conversation,
        req: &SendChatRequest,
        request_fingerprint: &str,
        accepted: phoenix_db::DirectTurnAcceptanceRecord,
        receipts: &mut std::collections::HashMap<(String, String), ChatAcceptanceReceipt>,
    ) -> Result<SendChatOutcome, SendChatServiceError> {
        let prepared: phoenix_core::domain::sm_event::PreparedDirectTurn =
            serde_json::from_str(&accepted.prepared_payload)
                .map_err(|error| SendChatServiceError::Internal(error.to_string()))?;
        let message_id = req.message_id.clone();
        Ok(match accepted.committed_outcome {
            phoenix_db::DirectTurnCommittedOutcome::QueuedSteering => {
                let Event::UserMessage {
                    text,
                    llm_text,
                    images,
                    files,
                    message_id,
                    user_agent,
                    skill_invocation,
                } = prepared.into_event()
                else {
                    return Err(SendChatServiceError::Internal(
                        "accepted direct turn did not decode to a user message".to_string(),
                    ));
                };
                kick_runtime_delivery(
                    self.runtime.clone(),
                    conversation.id.clone(),
                    Event::SteerMessage {
                        text,
                        llm_text,
                        images,
                        files,
                        message_id: message_id.clone(),
                        user_agent,
                        skill_invocation,
                    },
                );
                SendChatOutcome::accepted(
                    message_id,
                    SendChatRequestResult::Replayed,
                    SendChatDisposition::QueuedSteering,
                )
            }
            phoenix_db::DirectTurnCommittedOutcome::CancelledSteering => SendChatOutcome::accepted(
                message_id,
                SendChatRequestResult::Replayed,
                SendChatDisposition::CancelledSteering,
            ),
            phoenix_db::DirectTurnCommittedOutcome::PendingRuntime
            | phoenix_db::DirectTurnCommittedOutcome::RuntimeAccepted => {
                let should_deliver =
                    matches!(
                        accepted.committed_outcome,
                        phoenix_db::DirectTurnCommittedOutcome::PendingRuntime
                    ) || phoenix_db::WorkflowRepository::new(self.db.pool().clone())
                        .load_active_top_level_llm_workflow(&conversation.id)
                        .await
                        .map_err(|error| SendChatServiceError::Internal(error.to_string()))?
                        .is_some();
                if should_deliver {
                    kick_runtime_delivery(
                        self.runtime.clone(),
                        conversation.id.clone(),
                        prepared.into_event(),
                    );
                }
                insert_transient_receipt(
                    &self.db,
                    receipts,
                    (conversation.id.clone(), req.message_id.clone()),
                    ChatAcceptanceReceipt {
                        conversation_id: conversation.id.clone(),
                        request_fingerprint: request_fingerprint.to_string(),
                        steering: false,
                    },
                )
                .await;
                let disposition = match accepted.committed_outcome {
                    phoenix_db::DirectTurnCommittedOutcome::PendingRuntime => {
                        SendChatDisposition::PendingRuntime
                    }
                    phoenix_db::DirectTurnCommittedOutcome::RuntimeAccepted => {
                        SendChatDisposition::RuntimeAccepted
                    }
                    phoenix_db::DirectTurnCommittedOutcome::QueuedSteering
                    | phoenix_db::DirectTurnCommittedOutcome::CancelledSteering => unreachable!(),
                };
                SendChatOutcome::accepted(message_id, SendChatRequestResult::Replayed, disposition)
            }
        })
    }

    async fn effective_state(
        &self,
        conversation_id: &str,
        persisted_state: &ConvState,
    ) -> Result<ConvState, SendChatServiceError> {
        if let Some(live_state) = self
            .runtime
            .effective_conversation_state(conversation_id)
            .await
        {
            return Ok(live_state);
        }
        if let Err(stable_err) = check_user_message_acceptable(persisted_state) {
            if !matches!(
                stable_err,
                TransitionError::AgentBusy | TransitionError::CancellationInProgress
            ) {
                return Ok(persisted_state.clone());
            }
        }
        self.runtime
            .get_or_create(conversation_id)
            .await
            .map_err(SendChatServiceError::Dispatch)?;
        Ok(self
            .runtime
            .effective_conversation_state(conversation_id)
            .await
            .unwrap_or_else(|| persisted_state.clone()))
    }
}

struct ExpandedDispatchMessage {
    display_text: String,
    llm_text: Option<String>,
    skill_invocation: Option<SkillInvocation>,
}

async fn validate_files(
    req: &SendChatRequest,
) -> Result<Vec<phoenix_core::domain::db_schema::FileAttachment>, SendChatServiceError> {
    validate_submitted_attachments(&req.conversation_id, &req.files)
        .await
        .map_err(|error| SendChatServiceError::AttachmentValidation(format!("{error:?}")))
}

async fn expand_request(
    db: &crate::db::Database,
    conversation: &crate::db::Conversation,
    req: &SendChatRequest,
) -> Result<ExpandedDispatchMessage, SendChatServiceError> {
    expand_message(
        db,
        &conversation.id,
        &conversation.cwd,
        &req.text,
        req.expansion_policy,
    )
    .await
}

async fn expand_message(
    db: &crate::db::Database,
    conversation_id: &str,
    cwd: &str,
    text: &str,
    policy: MessageExpansionPolicy,
) -> Result<ExpandedDispatchMessage, SendChatServiceError> {
    let resolution_root = crate::resolution_root::ResolutionRoot::working_dir(cwd);
    let expanded = if policy == MessageExpansionPolicy::LiteralText
        || db
            .is_coordinator_conversation(conversation_id)
            .await
            .map_err(|e| SendChatServiceError::Internal(e.to_string()))?
    {
        crate::message_expander::ExpandedMessage {
            display_text: text.to_string(),
            llm_text: text.to_string(),
            skill_invocation: None,
        }
    } else {
        crate::message_expander::expand(text, &resolution_root).map_err(|error| {
            SendChatServiceError::Expansion {
                message: error.to_string(),
                error_type: error.error_type(),
                reference: error.reference(),
            }
        })?
    };
    let llm_text = (expanded.llm_text != expanded.display_text).then_some(expanded.llm_text);
    Ok(ExpandedDispatchMessage {
        display_text: expanded.display_text,
        llm_text,
        skill_invocation: expanded.skill_invocation,
    })
}

fn kick_runtime_delivery(runtime: Arc<RuntimeManager>, conversation_id: String, event: Event) {
    runtime.kick_wake_worker();
    tokio::spawn(async move {
        let message_id = match &event {
            Event::UserMessage { message_id, .. } | Event::SteerMessage { message_id, .. } => {
                message_id.clone()
            }
            Event::CreationProvisioned { .. }
            | Event::CreationRequestResume { .. }
            | Event::UserCancel { .. }
            | Event::LlmResponse { .. }
            | Event::LlmError { .. }
            | Event::RetryTimeout { .. }
            | Event::ToolComplete { .. }
            | Event::ToolAborted { .. }
            | Event::SpawnAgentsComplete { .. }
            | Event::SubAgentResult { .. }
            | Event::ContinuationResponse { .. }
            | Event::ContinuationFailed { .. }
            | Event::UserTriggerContinuation
            | Event::TaskApprovalDecided { .. }
            | Event::CommissionReviewApprovalDecided { .. }
            | Event::TaskHandoffComplete { .. }
            | Event::UserQuestionResponse { .. }
            | Event::UserQuestionDismissed
            | Event::DismissError
            | Event::GraceTurnExhausted { .. }
            | Event::CredentialBecameAvailable
            | Event::CredentialHelperFailed { .. }
            | Event::TaskResolved { .. }
            | Event::CancelSteerMessage { .. }
            | Event::SteerDrainedUserMessages { .. }
            | Event::WakeBatchAdopted
            | Event::ResumeDurableLlmRequest
            | Event::ResumeDurableToolExecution
            | Event::ResumeDurableLlmFailure { .. }
            | Event::Shutdown => return,
        };
        let repo = phoenix_db::WorkflowRepository::new(runtime.db().pool().clone());
        match repo
            .claim_direct_turn_runtime_delivery(
                &conversation_id,
                &message_id,
                crate::runtime::process_incarnation(),
            )
            .await
        {
            Ok(true) => {}
            Ok(false) => return,
            Err(error) => {
                tracing::warn!(%conversation_id, %error, "failed to claim direct-turn runtime delivery");
                return;
            }
        }
        if matches!(event, Event::SteerMessage { .. })
            && repo
                .load_direct_turn_acceptance(&conversation_id, &message_id)
                .await
                .is_ok_and(|acceptance| {
                    acceptance.is_some_and(|record| {
                        record.committed_outcome
                            == phoenix_db::DirectTurnCommittedOutcome::CancelledSteering
                    })
                })
        {
            return;
        }
        if let Err(error) = runtime.send_event(&conversation_id, event).await {
            if let Err(release_error) = repo
                .release_direct_turn_runtime_delivery(
                    &conversation_id,
                    &message_id,
                    crate::runtime::process_incarnation(),
                )
                .await
            {
                tracing::warn!(%conversation_id, %release_error, "failed to release direct-turn delivery claim");
            }
            tracing::warn!(
                conversation_id,
                error = %error,
                "durably accepted message runtime kick failed; recovery remains owed"
            );
            runtime.kick_wake_worker();
        }
    });
}

async fn insert_transient_receipt(
    db: &crate::db::Database,
    receipts: &mut std::collections::HashMap<(String, String), ChatAcceptanceReceipt>,
    key: (String, String),
    receipt: ChatAcceptanceReceipt,
) {
    const CLEANUP_THRESHOLD: usize = 1_024;
    if receipts.len() >= CLEANUP_THRESHOLD {
        let candidates = receipts.keys().cloned().collect::<Vec<_>>();
        for (conversation_id, message_id) in candidates {
            let persisted = db.message_exists(&message_id).await.unwrap_or(false);
            let queued = db
                .get_steering_queue(&conversation_id)
                .await
                .is_ok_and(|queue| queue.iter().any(|entry| entry.message_id == message_id));
            if persisted || queued {
                receipts.remove(&(conversation_id, message_id));
            }
        }
    }
    receipts.insert(key, receipt);
}

async fn find_queued_message(
    db: &crate::db::Database,
    conversation_id: &str,
    message_id: &str,
) -> Result<Option<phoenix_core::domain::sm_event::SteerEntry>, SendChatServiceError> {
    db.get_steering_queue(conversation_id)
        .await
        .map_err(|error| SendChatServiceError::Internal(error.to_string()))
        .map(|queue| {
            queue
                .into_iter()
                .find(|entry| entry.message_id == message_id)
        })
}

fn queued_retry_matches(
    entry: &phoenix_core::domain::sm_event::SteerEntry,
    req: &SendChatRequest,
) -> bool {
    entry.text == req.text
        && entry.images.len() == req.images.len()
        && entry
            .images
            .iter()
            .zip(&req.images)
            .all(|(stored, requested)| {
                stored.data == requested.data && stored.media_type == requested.media_type
            })
        && entry.files.len() == req.files.len()
        && entry
            .files
            .iter()
            .zip(&req.files)
            .all(|(stored, requested)| {
                stored.original_name == requested.original_name
                    && stored.media_type == requested.media_type
                    && stored.size_bytes == requested.size_bytes
                    && stored.stored_path == requested.stored_path
            })
        && entry.user_agent == req.user_agent
}

fn steer_entry_from_prepared(
    prepared: &phoenix_core::domain::sm_event::PreparedDirectTurn,
) -> phoenix_core::domain::sm_event::SteerEntry {
    phoenix_core::domain::sm_event::SteerEntry {
        text: prepared.text.clone(),
        llm_text: prepared.llm_text.clone(),
        images: prepared.images.clone(),
        files: prepared.files.clone(),
        message_id: prepared.message_id.clone(),
        user_agent: prepared.user_agent.clone(),
        skill_invocation: prepared.skill_invocation.clone(),
    }
}

fn prepared_retry_matches_request(
    prepared: &phoenix_core::domain::sm_event::PreparedDirectTurn,
    req: &SendChatRequest,
) -> bool {
    prepared.text == req.text
        && prepared.expand_references
            == (req.expansion_policy == MessageExpansionPolicy::ExpandReferences)
        && prepared.message_id == req.message_id
        && prepared.user_agent == req.user_agent
        && prepared.images == map_images(req.images.clone())
        && prepared.files
            == req
                .files
                .clone()
                .into_iter()
                .map(Into::into)
                .collect::<Vec<_>>()
}

fn persisted_user_message_matches(
    content: &phoenix_core::domain::db_schema::MessageContent,
    req: &SendChatRequest,
) -> bool {
    let phoenix_core::domain::db_schema::MessageContent::User(user) = content else {
        return false;
    };
    user.text == req.text
        && user.images.len() == req.images.len()
        && user
            .images
            .iter()
            .zip(&req.images)
            .all(|(stored, requested)| {
                stored.data == requested.data && stored.media_type == requested.media_type
            })
        && user.files.len() == req.files.len()
        && user
            .files
            .iter()
            .zip(&req.files)
            .all(|(stored, requested)| {
                stored.original_name == requested.original_name
                    && stored.media_type == requested.media_type
                    && stored.size_bytes == requested.size_bytes
                    && stored.stored_path == requested.stored_path
            })
}

fn persisted_skill_matches(
    skill: &phoenix_core::domain::db_schema::SkillContent,
    req: &SendChatRequest,
) -> bool {
    skill.trigger == req.text
        && skill.files.len() == req.files.len()
        && skill
            .files
            .iter()
            .zip(&req.files)
            .all(|(stored, requested)| {
                stored.original_name == requested.original_name
                    && stored.media_type == requested.media_type
                    && stored.size_bytes == requested.size_bytes
                    && stored.stored_path == requested.stored_path
            })
}

fn request_fingerprint(req: &SendChatRequest) -> Result<String, SendChatServiceError> {
    let canonical = serde_json::to_vec(&serde_json::json!({
        "conversation_id": req.conversation_id,
        "text": req.text,
        "images": req.images.iter().map(|image| serde_json::json!({
            "data": image.data,
            "media_type": image.media_type,
        })).collect::<Vec<_>>(),
        "files": req.files,
        "user_agent": req.user_agent,
        "expansion_policy": match req.expansion_policy {
            MessageExpansionPolicy::ExpandReferences => "expand_references",
            MessageExpansionPolicy::LiteralText => "literal_text",
        },
    }))
    .map_err(|error| SendChatServiceError::Internal(error.to_string()))?;
    Ok(sha256_hex(&canonical))
}

fn prepared_direct_turn_fingerprint(prepared_payload: &str) -> String {
    sha256_hex(prepared_payload.as_bytes())
}

fn sha256_hex(value: &[u8]) -> String {
    use sha2::Digest as _;
    sha2::Sha256::digest(value)
        .iter()
        .fold(String::with_capacity(64), |mut output, byte| {
            write!(output, "{byte:02x}").expect("writing to String cannot fail");
            output
        })
}

fn replay_receipt(
    receipt: &ChatAcceptanceReceipt,
    req: &SendChatRequest,
    request_fingerprint: &str,
) -> Result<SendChatOutcome, SendChatServiceError> {
    if receipt.conversation_id != req.conversation_id
        || receipt.request_fingerprint != request_fingerprint
    {
        return Err(SendChatServiceError::IdempotencyConflict);
    }
    Ok(SendChatOutcome::accepted(
        req.message_id.clone(),
        SendChatRequestResult::Replayed,
        if receipt.steering {
            SendChatDisposition::QueuedSteering
        } else {
            SendChatDisposition::PendingRuntime
        },
    ))
}

fn map_images(images: Vec<ImageAttachment>) -> Vec<ImageData> {
    images
        .into_iter()
        .map(|img| ImageData {
            data: img.data,
            media_type: img.media_type,
        })
        .collect()
}

fn transition_code(err: &TransitionError) -> &'static str {
    match err {
        TransitionError::ContextExhausted => "context_exhausted",
        TransitionError::ConversationTerminal => "conversation_terminal",
        TransitionError::AwaitingTaskApproval => "awaiting_task_approval",
        TransitionError::AwaitingUserResponse => "awaiting_user_response",
        TransitionError::AgentBusy => "agent_busy",
        TransitionError::CancellationInProgress => "cancellation_in_progress",
        TransitionError::InvalidTransition { .. } => "invalid_state_for_message",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        find_queued_message, persisted_skill_matches, prepared_direct_turn_fingerprint,
        queued_retry_matches, MessageExpansionPolicy, SendChatApplicationService, SendChatRequest,
    };
    use phoenix_core::domain::db_schema::SkillContent;

    #[tokio::test]
    async fn durable_replay_ignores_reference_changes_after_acceptance() {
        let state = crate::api::handlers::hard_delete_cascade_tests::make_test_state().await;
        let cwd = tempfile::tempdir().expect("create temp cwd");
        let reference = cwd.path().join("notes.md");
        std::fs::write(&reference, "first expansion").expect("write initial reference");
        state
            .db
            .create_conversation(
                "conv-prepared-replay",
                "prepared replay",
                cwd.path().to_str().expect("utf-8 cwd"),
                true,
                None,
                None,
            )
            .await
            .expect("create conversation");
        let service = SendChatApplicationService::new(state.db.clone(), state.runtime.clone());
        let request = SendChatRequest {
            conversation_id: "conv-prepared-replay".to_string(),
            text: "Review @notes.md".to_string(),
            message_id: "message-prepared-replay".to_string(),
            images: Vec::new(),
            files: Vec::new(),
            user_agent: None,
            expansion_policy: MessageExpansionPolicy::ExpandReferences,
        };

        service
            .send(request.clone())
            .await
            .expect("accept initial prepared turn");
        std::fs::write(&reference, "changed expansion").expect("mutate reference");

        assert!(matches!(
            service.send(request).await,
            Ok(super::SendChatOutcome::Accepted {
                request_result: super::SendChatRequestResult::Replayed,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn persisted_message_replay_is_scoped_to_the_target_conversation() {
        let state = crate::api::handlers::hard_delete_cascade_tests::make_test_state().await;
        for conversation_id in ["conv-owner", "conv-target"] {
            state
                .db
                .create_conversation(conversation_id, conversation_id, "/tmp", true, None, None)
                .await
                .unwrap();
        }
        state
            .db
            .add_message(
                "shared-client-id",
                "conv-owner",
                &phoenix_core::domain::db_schema::MessageContent::user("owner payload"),
                None,
                None,
            )
            .await
            .unwrap();
        let service = SendChatApplicationService::new(state.db.clone(), state.runtime.clone());

        let outcome = service
            .send(SendChatRequest {
                conversation_id: "conv-target".to_string(),
                text: "target payload".to_string(),
                message_id: "shared-client-id".to_string(),
                images: Vec::new(),
                files: Vec::new(),
                user_agent: None,
                expansion_policy: MessageExpansionPolicy::LiteralText,
            })
            .await
            .expect("the same client id is independent in another conversation");

        assert!(matches!(
            outcome,
            super::SendChatOutcome::Accepted {
                request_result: super::SendChatRequestResult::Created,
                ..
            }
        ));
        assert!(phoenix_db::WorkflowRepository::new(state.db.pool().clone())
            .load_direct_turn_acceptance("conv-target", "shared-client-id")
            .await
            .unwrap()
            .is_some());
    }

    #[tokio::test]
    async fn queued_replay_lookup_is_scoped_to_the_target_conversation() {
        let db = crate::db::Database::open_in_memory().await.unwrap();
        for conversation_id in ["conv-a", "conv-b"] {
            db.create_conversation(conversation_id, conversation_id, "/tmp", true, None, None)
                .await
                .unwrap();
        }
        db.update_steering_queue(
            "conv-a",
            &[phoenix_core::domain::sm_event::SteerEntry {
                text: "first target".to_string(),
                llm_text: None,
                images: Vec::new(),
                files: Vec::new(),
                message_id: "shared-id".to_string(),
                user_agent: None,
                skill_invocation: None,
            }],
        )
        .await
        .unwrap();

        assert!(find_queued_message(&db, "conv-b", "shared-id")
            .await
            .unwrap()
            .is_none());
        assert_eq!(
            find_queued_message(&db, "conv-a", "shared-id")
                .await
                .unwrap()
                .map(|entry| entry.text),
            Some("first target".to_string())
        );
    }

    #[tokio::test]
    async fn live_slot_conflict_does_not_overfill_steering_queue() {
        let state = crate::api::handlers::hard_delete_cascade_tests::make_test_state().await;
        state
            .db
            .create_conversation("conv-full", "full", "/tmp", true, None, None)
            .await
            .unwrap();
        let repo = phoenix_db::WorkflowRepository::new(state.db.pool().clone());
        repo.accept_direct_turn(&phoenix_db::DirectTurnAcceptanceInput {
            initial_outcome: phoenix_db::DirectTurnInitialOutcome::RuntimeAccepted,
            conversation_id: "conv-full".to_string(),
            client_message_id: "active-turn".to_string(),
            prepared_fingerprint: "active".to_string(),
            prepared_payload: "{}".to_string(),
            accepted_at: phoenix_workflow::Timestamp(1),
            snapshot: phoenix_workflow::llm_profile::TopLevelLlmSnapshot {
                turn_ref: phoenix_workflow::llm_profile::TopLevelTurnRef {
                    conversation_id: "conv-full".to_string(),
                    accepted_turn_id: "active-turn".to_string(),
                    generation: 0,
                },
                accepted_assistant_message_id: None,
                stopped_at: None,
            },
        })
        .await
        .unwrap();
        let queue = (0..super::MAX_STEER_QUEUE_DEPTH)
            .map(|index| phoenix_core::domain::sm_event::SteerEntry {
                text: format!("queued {index}"),
                llm_text: None,
                images: Vec::new(),
                files: Vec::new(),
                message_id: format!("queued-{index}"),
                user_agent: None,
                skill_invocation: None,
            })
            .collect::<Vec<_>>();
        state
            .db
            .update_steering_queue("conv-full", &queue)
            .await
            .unwrap();

        let outcome = SendChatApplicationService::new(state.db.clone(), state.runtime.clone())
            .send(SendChatRequest {
                conversation_id: "conv-full".to_string(),
                text: "overflow".to_string(),
                message_id: "overflow".to_string(),
                images: Vec::new(),
                files: Vec::new(),
                user_agent: None,
                expansion_policy: MessageExpansionPolicy::LiteralText,
            })
            .await
            .unwrap();

        assert!(matches!(
            outcome,
            super::SendChatOutcome::Rejected {
                code: "steering_queue_full",
                ..
            }
        ));
        assert_eq!(
            state
                .db
                .get_steering_queue("conv-full")
                .await
                .unwrap()
                .len(),
            super::MAX_STEER_QUEUE_DEPTH
        );
    }

    #[tokio::test]
    async fn materialized_queued_replay_preserves_queued_disposition() {
        let state = crate::api::handlers::hard_delete_cascade_tests::make_test_state().await;
        state
            .db
            .create_conversation(
                "conv-materialized-queued",
                "queued",
                "/tmp",
                true,
                None,
                None,
            )
            .await
            .unwrap();
        let prepared = phoenix_core::domain::sm_event::PreparedDirectTurn {
            codec_version: phoenix_core::domain::sm_event::PREPARED_DIRECT_TURN_CODEC_VERSION,
            expand_references: true,
            text: "queued".to_string(),
            llm_text: None,
            images: Vec::new(),
            files: Vec::new(),
            message_id: "queued-id".to_string(),
            user_agent: None,
            skill_invocation: None,
        };
        let repo = phoenix_db::WorkflowRepository::new(state.db.pool().clone());
        repo.accept_direct_turn(&phoenix_db::DirectTurnAcceptanceInput {
            initial_outcome: phoenix_db::DirectTurnInitialOutcome::QueuedSteering {
                entry: Box::new(phoenix_core::domain::sm_event::SteerEntry {
                    text: prepared.text.clone(),
                    llm_text: None,
                    images: Vec::new(),
                    files: Vec::new(),
                    message_id: prepared.message_id.clone(),
                    user_agent: None,
                    skill_invocation: None,
                }),
            },
            conversation_id: "conv-materialized-queued".to_string(),
            client_message_id: "queued-id".to_string(),
            prepared_fingerprint: "fingerprint".to_string(),
            prepared_payload: serde_json::to_string(&prepared).unwrap(),
            accepted_at: phoenix_workflow::Timestamp(1),
            snapshot: phoenix_workflow::llm_profile::TopLevelLlmSnapshot {
                turn_ref: phoenix_workflow::llm_profile::TopLevelTurnRef {
                    conversation_id: "conv-materialized-queued".to_string(),
                    accepted_turn_id: "queued-id".to_string(),
                    generation: 0,
                },
                accepted_assistant_message_id: None,
                stopped_at: None,
            },
        })
        .await
        .unwrap();
        let proposed = crate::db::Message {
            message_id: "queued-id".to_string(),
            conversation_id: "conv-materialized-queued".to_string(),
            sequence_id: 1,
            message_type: crate::db::MessageType::User,
            content: crate::db::MessageContent::user("queued"),
            display_data: None,
            usage_data: None,
            created_at: chrono::Utc::now(),
        };
        state
            .db
            .persist_queued_steering_message("conv-materialized-queued", "queued-id", &proposed)
            .await
            .unwrap();

        let outcome = SendChatApplicationService::new(state.db.clone(), state.runtime.clone())
            .send(SendChatRequest {
                conversation_id: "conv-materialized-queued".to_string(),
                text: "queued".to_string(),
                message_id: "queued-id".to_string(),
                images: Vec::new(),
                files: Vec::new(),
                user_agent: None,
                expansion_policy: MessageExpansionPolicy::ExpandReferences,
            })
            .await
            .unwrap();
        assert!(matches!(
            outcome,
            super::SendChatOutcome::Accepted {
                disposition: super::SendChatDisposition::QueuedSteering,
                ..
            }
        ));
    }

    #[test]
    fn prepared_fingerprint_changes_when_expansion_changes() {
        let base = phoenix_core::domain::sm_event::PreparedDirectTurn {
            codec_version: phoenix_core::domain::sm_event::PREPARED_DIRECT_TURN_CODEC_VERSION,
            expand_references: true,
            text: "@ref".to_string(),
            llm_text: Some("first expansion".to_string()),
            images: Vec::new(),
            files: Vec::new(),
            message_id: "msg".to_string(),
            user_agent: None,
            skill_invocation: None,
        };
        let mut changed = base.clone();
        changed.llm_text = Some("changed expansion".to_string());
        let base = serde_json::to_string(&base).unwrap();
        let changed = serde_json::to_string(&changed).unwrap();
        assert_ne!(
            prepared_direct_turn_fingerprint(&base),
            prepared_direct_turn_fingerprint(&changed)
        );
    }

    #[test]
    fn queued_retry_compares_submitted_payload_not_mutable_expansion() {
        let request = SendChatRequest {
            conversation_id: "conv-1".to_string(),
            text: "@file:notes.md".to_string(),
            message_id: "message-1".to_string(),
            images: vec![],
            files: vec![],
            user_agent: None,
            expansion_policy: MessageExpansionPolicy::ExpandReferences,
        };
        let entry = phoenix_core::domain::sm_event::SteerEntry {
            text: request.text.clone(),
            llm_text: Some("old expanded file contents".to_string()),
            images: vec![],
            files: vec![],
            message_id: request.message_id.clone(),
            user_agent: None,
            skill_invocation: None,
        };

        assert!(queued_retry_matches(&entry, &request));
        let mut conflicting = request.clone();
        conflicting.text = "different submission".to_string();
        assert!(!queued_retry_matches(&entry, &conflicting));
    }

    #[test]
    fn persisted_skill_retry_matches_expanded_invocation() {
        let request = SendChatRequest {
            conversation_id: "conv-1".to_string(),
            text: "/build now".to_string(),
            message_id: "message-1".to_string(),
            images: vec![],
            files: vec![],
            user_agent: None,
            expansion_policy: MessageExpansionPolicy::ExpandReferences,
        };
        let persisted = SkillContent {
            name: "build".to_string(),
            body: "expanded body".to_string(),
            trigger: request.text.clone(),
            files: vec![],
        };

        assert!(persisted_skill_matches(&persisted, &request));
        let mut changed_definition = persisted.clone();
        changed_definition.body = "definition changed after commit".to_string();
        assert!(persisted_skill_matches(&changed_definition, &request));
    }
}
