use crate::api::{record_pr_auto_fix_context_baseline, validate_submitted_attachments};
use crate::api::{FileAttachment, ImageAttachment};
use crate::db::ConvState;
use crate::runtime::{RuntimeManager, SteeringAcceptanceReceipt};
use crate::state_machine::{check_user_message_acceptable, Event, TransitionError};
use phoenix_core::domain::db_schema::ImageData;
use phoenix_core::domain::skill_invocation::SkillInvocation;
use phoenix_core::domain::sm_event::{
    PreparedDirectTurnDelivery, PreparedDirectTurnPayload, SubmittedDirectTurnExpansionPolicy,
    SubmittedDirectTurnFileAttachment, SubmittedDirectTurnIdentity,
};
use phoenix_db::workflow::{
    AcceptAuthoritativeTurn, ScopedDirectTurnReplayError, ScopedDirectTurnReplayLookup,
    WorkflowRepository,
};
use phoenix_workflow::{
    AcceptedDisposition, ClientTurnKey, ConversationAuthority, Materialization, PreparedTurn,
    Timestamp, TurnConflict, TurnOutcome,
};
use std::fmt::Write as _;
use std::sync::Arc;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SendChatOutcome {
    Delivered,
    AlreadyPersisted,
    QueuedAsSteering,
    Rejected { message: String, code: &'static str },
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
    #[error("conversation is busy accepting another direct turn")]
    Busy,
}

#[derive(Clone)]
pub(crate) struct SendChatApplicationService {
    db: crate::db::Database,
    runtime: Arc<RuntimeManager>,
}

const MAX_STEER_QUEUE_DEPTH: usize = 5;

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
        let conversation = self
            .runtime
            .db()
            .get_conversation(&req.conversation_id)
            .await
            .map_err(map_conversation_load_error)?;
        let submitted = submitted_identity_from_request(&req);
        match lookup_durable_replay(&self.db, &req, &submitted).await? {
            DurableReplayOutcome::Missing => {}
            DurableReplayOutcome::ExactMaterialized => {
                return Ok(SendChatOutcome::AlreadyPersisted);
            }
            DurableReplayOutcome::ExactUnmaterializedTerminal => {
                return Ok(SendChatOutcome::Rejected {
                    message: "The accepted message was cancelled or failed before delivery."
                        .to_string(),
                    code: "turn_terminal",
                });
            }
            DurableReplayOutcome::ExactUnmaterializedLive => {
                self.runtime.kick_direct_turn_worker();
                return Ok(SendChatOutcome::Delivered);
            }
        }
        if let Ok(message) = self
            .db
            .get_message_by_id_in_conversation(&req.conversation_id, &req.message_id)
            .await
        {
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
            if message.conversation_id != req.conversation_id || !persisted_matches {
                return Err(SendChatServiceError::IdempotencyConflict);
            }
            return Ok(SendChatOutcome::AlreadyPersisted);
        }
        if conversation.archived {
            return Ok(SendChatOutcome::Rejected {
                message: "Conversation is archived and unavailable for messaging.".to_string(),
                code: "target_unavailable",
            });
        }
        {
            let mut receipts = self.runtime.lock_steering_acceptance().await;
            let receipt_key = (req.conversation_id.clone(), req.message_id.clone());
            if let Some(receipt) = receipts.get(&receipt_key) {
                return replay_steering_receipt(receipt, &req, &request_fingerprint);
            }
            if let Some((queued_conversation_id, queued_entry)) =
                find_queued_message(&self.db, &req.conversation_id, &req.message_id).await?
            {
                if queued_conversation_id != req.conversation_id
                    || !queued_retry_matches(&queued_entry, &req)
                {
                    return Err(SendChatServiceError::IdempotencyConflict);
                }
                receipts.remove(&receipt_key);
                return Ok(SendChatOutcome::QueuedAsSteering);
            }
        }

        let effective_state = self
            .effective_state(&conversation.id, &conversation.state)
            .await?;
        if let Err(err) = check_user_message_acceptable(&effective_state) {
            if matches!(
                err,
                TransitionError::AgentBusy | TransitionError::CancellationInProgress
            ) {
                let validated_files = validate_files(&req).await?;
                let expanded = expand_request(&self.db, &conversation, &req).await?;
                match lookup_durable_replay(&self.db, &req, &submitted).await? {
                    DurableReplayOutcome::Missing => {}
                    DurableReplayOutcome::ExactMaterialized => {
                        return Ok(SendChatOutcome::AlreadyPersisted);
                    }
                    DurableReplayOutcome::ExactUnmaterializedTerminal => {
                        return Ok(SendChatOutcome::Rejected {
                            message:
                                "The accepted message was cancelled or failed before delivery."
                                    .to_string(),
                            code: "turn_terminal",
                        });
                    }
                    DurableReplayOutcome::ExactUnmaterializedLive => {
                        self.runtime.kick_direct_turn_worker();
                        return Ok(SendChatOutcome::Delivered);
                    }
                }
                let event = Event::SteerMessage {
                    text: expanded.display_text.clone(),
                    llm_text: expanded.llm_text,
                    images: map_images(req.images.clone()),
                    files: validated_files.clone(),
                    message_id: req.message_id.clone(),
                    user_agent: req.user_agent.clone(),
                    skill_invocation: expanded.skill_invocation,
                };
                {
                    let mut receipts = self.runtime.lock_steering_acceptance().await;
                    let receipt_key = (req.conversation_id.clone(), req.message_id.clone());
                    if let Some(receipt) = receipts.get(&receipt_key) {
                        return replay_steering_receipt(receipt, &req, &request_fingerprint);
                    }
                    if let Some((queued_conversation_id, queued_entry)) =
                        find_queued_message(&self.db, &req.conversation_id, &req.message_id).await?
                    {
                        if queued_conversation_id != req.conversation_id
                            || !queued_retry_matches(&queued_entry, &req)
                        {
                            return Err(SendChatServiceError::IdempotencyConflict);
                        }
                        receipts.remove(&receipt_key);
                        return Ok(SendChatOutcome::QueuedAsSteering);
                    }
                    let steering_queue = self
                        .runtime
                        .db()
                        .get_steering_queue(&req.conversation_id)
                        .await
                        .map_err(map_conversation_load_error)?;
                    if steering_queue.len() >= MAX_STEER_QUEUE_DEPTH {
                        return Ok(SendChatOutcome::Rejected {
                            message: "Steering queue is full; try again once a queued message has been delivered."
                                .to_string(),
                            code: "steering_queue_full",
                        });
                    }
                    self.runtime
                        .enqueue_steer_message(&conversation.id, event)
                        .await
                        .map_err(SendChatServiceError::Dispatch)?;
                    insert_transient_steering_receipt(
                        &self.db,
                        &mut receipts,
                        (req.conversation_id.clone(), req.message_id.clone()),
                        SteeringAcceptanceReceipt {
                            conversation_id: conversation.id.clone(),
                            request_fingerprint,
                        },
                    )
                    .await;
                }
                if let Err(error) = record_pr_auto_fix_context_baseline(
                    self.runtime.db(),
                    &conversation.id,
                    &expanded.display_text,
                )
                .await
                {
                    tracing::warn!(conversation_id = %conversation.id, error = ?error, "Message accepted but PR auto-fix baseline recording failed");
                }
                return Ok(SendChatOutcome::QueuedAsSteering);
            }
            return Ok(SendChatOutcome::Rejected {
                message: err.to_string(),
                code: transition_code(&err),
            });
        }

        let validated_files = validate_files(&req).await?;
        let expanded = expand_request(&self.db, &conversation, &req).await?;
        let images = map_images(req.images);
        let delivery = PreparedDirectTurnDelivery {
            text: expanded.display_text.clone(),
            llm_text: expanded.llm_text,
            images,
            files: validated_files,
            user_agent: req.user_agent,
            skill_invocation: expanded.skill_invocation,
        };
        let prepared_payload = PreparedDirectTurnPayload::from_parts(submitted.clone(), delivery);
        let prepared_bytes = prepared_payload
            .to_exact_bytes()
            .map_err(|error| SendChatServiceError::Internal(error.to_string()))?;
        let repo = WorkflowRepository::new(self.db.pool().clone());
        let client_key = ClientTurnKey::new(req.message_id.clone())
            .ok_or(SendChatServiceError::IdempotencyConflict)?;
        let step = match repo
            .accept_authoritative_turn(&AcceptAuthoritativeTurn {
                client_key: client_key.clone(),
                prepared: PreparedTurn::from_exact_payload(
                    &ConversationAuthority(conversation.id.clone()),
                    prepared_bytes,
                ),
                disposition: AcceptedDisposition::Runtime,
                accepted_at: now_timestamp(),
            })
            .await
        {
            Ok(step) => step,
            Err(crate::db::DbError::DirectTurnConflict(
                TurnConflict::PreparedSemanticsChanged { .. },
            )) => {
                match repo
                    .lookup_scoped_direct_turn_replay(
                        &ConversationAuthority(conversation.id.clone()),
                        &client_key,
                        &submitted,
                    )
                    .await
                {
                    Ok(ScopedDirectTurnReplayLookup::Exact { turn, .. }) => {
                        return Ok(match turn.materialization {
                            Materialization::Unmaterialized => {
                                if matches!(
                                    turn.lifecycle,
                                    phoenix_workflow::TurnLifecycle::Terminal { .. }
                                ) {
                                    SendChatOutcome::Rejected {
                                        message: "The accepted message was cancelled or failed before delivery."
                                            .to_string(),
                                        code: "turn_terminal",
                                    }
                                } else {
                                    self.runtime.kick_direct_turn_worker();
                                    SendChatOutcome::Delivered
                                }
                            }
                            Materialization::Materialized { .. } => {
                                SendChatOutcome::AlreadyPersisted
                            }
                        });
                    }
                    Ok(ScopedDirectTurnReplayLookup::Missing)
                    | Err(ScopedDirectTurnReplayError::SubmittedIdentityChanged { .. }) => {
                        return Err(SendChatServiceError::IdempotencyConflict);
                    }
                    Err(ScopedDirectTurnReplayError::Db(error)) => {
                        return Err(map_db_internal_error(&error));
                    }
                }
            }
            Err(error) => return Err(map_direct_turn_accept_error(error)),
        };
        match step.outcome {
            TurnOutcome::Created { .. } | TurnOutcome::ExactReplay { .. } => {
                self.runtime.kick_direct_turn_worker();
            }
            TurnOutcome::TerminalReplay { .. } => {}
            TurnOutcome::Materialized { .. }
            | TurnOutcome::MaterializationReplay { .. }
            | TurnOutcome::Terminal { .. } => {
                return Err(SendChatServiceError::Internal(format!(
                    "unexpected direct-turn accept outcome: {:?}",
                    step.outcome
                )));
            }
        }
        if let Err(error) = record_pr_auto_fix_context_baseline(
            self.runtime.db(),
            &conversation.id,
            &expanded.display_text,
        )
        .await
        {
            tracing::warn!(conversation_id = %conversation.id, error = ?error, "Message accepted but PR auto-fix baseline recording failed");
        }
        Ok(SendChatOutcome::Delivered)
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
        let resolution_root = crate::resolution_root::ResolutionRoot::working_dir(cwd);
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DurableReplayOutcome {
    Missing,
    ExactMaterialized,
    ExactUnmaterializedLive,
    ExactUnmaterializedTerminal,
}

async fn lookup_durable_replay(
    db: &crate::db::Database,
    req: &SendChatRequest,
    submitted: &SubmittedDirectTurnIdentity,
) -> Result<DurableReplayOutcome, SendChatServiceError> {
    let repo = WorkflowRepository::new(db.pool().clone());
    match repo
        .lookup_scoped_direct_turn_replay(
            &ConversationAuthority(req.conversation_id.clone()),
            &ClientTurnKey::new(req.message_id.clone())
                .ok_or(SendChatServiceError::IdempotencyConflict)?,
            submitted,
        )
        .await
    {
        Ok(ScopedDirectTurnReplayLookup::Missing) => Ok(DurableReplayOutcome::Missing),
        Ok(ScopedDirectTurnReplayLookup::Exact { turn, .. }) => match turn.materialization {
            Materialization::Unmaterialized => {
                if matches!(
                    turn.lifecycle,
                    phoenix_workflow::TurnLifecycle::Terminal { .. }
                ) {
                    Ok(DurableReplayOutcome::ExactUnmaterializedTerminal)
                } else {
                    Ok(DurableReplayOutcome::ExactUnmaterializedLive)
                }
            }
            Materialization::Materialized { .. } => Ok(DurableReplayOutcome::ExactMaterialized),
        },
        Err(ScopedDirectTurnReplayError::SubmittedIdentityChanged { .. }) => {
            Err(SendChatServiceError::IdempotencyConflict)
        }
        Err(ScopedDirectTurnReplayError::Db(error)) => Err(map_db_internal_error(&error)),
    }
}

fn submitted_identity_from_request(req: &SendChatRequest) -> SubmittedDirectTurnIdentity {
    SubmittedDirectTurnIdentity {
        text: req.text.clone(),
        images: req
            .images
            .iter()
            .cloned()
            .map(|image| ImageData {
                data: image.data,
                media_type: image.media_type,
            })
            .collect(),
        files: req
            .files
            .iter()
            .cloned()
            .map(|file| SubmittedDirectTurnFileAttachment {
                original_name: file.original_name,
                media_type: file.media_type,
                size_bytes: file.size_bytes,
                stored_path: file.stored_path,
            })
            .collect(),
        message_id: req.message_id.clone(),
        user_agent: req.user_agent.clone(),
        skill_invocation: None,
        expansion_policy: match req.expansion_policy {
            MessageExpansionPolicy::ExpandReferences => {
                SubmittedDirectTurnExpansionPolicy::ExpandReferences
            }
            MessageExpansionPolicy::LiteralText => SubmittedDirectTurnExpansionPolicy::LiteralText,
        },
    }
}

fn now_timestamp() -> Timestamp {
    let now = chrono::Utc::now().timestamp();
    Timestamp(u64::try_from(now).unwrap_or_default())
}

fn map_conversation_load_error(error: crate::db::DbError) -> SendChatServiceError {
    match error {
        crate::db::DbError::ConversationNotFound(message) => {
            SendChatServiceError::NotFound(message)
        }
        crate::db::DbError::Sqlx(sqlx::Error::RowNotFound) => {
            SendChatServiceError::NotFound("conversation not found".to_string())
        }
        other @ (crate::db::DbError::Sqlx(_)
        | crate::db::DbError::MessageNotFound(_)
        | crate::db::DbError::SlugExists(_)
        | crate::db::DbError::ConversationAlreadyExists(_)
        | crate::db::DbError::Serialization(_)
        | crate::db::DbError::ForkProposalConflict(_)
        | crate::db::DbError::DirectTurnConflict(_)) => map_db_internal_error(&other),
    }
}

fn map_direct_turn_accept_error(error: crate::db::DbError) -> SendChatServiceError {
    match error {
        crate::db::DbError::DirectTurnConflict(TurnConflict::PreparedSemanticsChanged {
            ..
        }) => SendChatServiceError::IdempotencyConflict,
        crate::db::DbError::DirectTurnConflict(TurnConflict::ConversationAlreadyOwned {
            ..
        }) => SendChatServiceError::Busy,
        crate::db::DbError::DirectTurnConflict(
            TurnConflict::UnknownTurn
            | TurnConflict::StaleGeneration { .. }
            | TurnConflict::AlreadyTerminal
            | TurnConflict::MaterializationIdentityChanged { .. }
            | TurnConflict::CorruptAggregate(_),
        ) => SendChatServiceError::Internal(error.to_string()),
        other @ (crate::db::DbError::Sqlx(_)
        | crate::db::DbError::ConversationNotFound(_)
        | crate::db::DbError::ConversationAlreadyExists(_)
        | crate::db::DbError::MessageNotFound(_)
        | crate::db::DbError::SlugExists(_)
        | crate::db::DbError::Serialization(_)
        | crate::db::DbError::ForkProposalConflict(_)) => map_db_internal_error(&other),
    }
}

fn map_db_internal_error(error: &crate::db::DbError) -> SendChatServiceError {
    SendChatServiceError::Internal(error.to_string())
}

async fn insert_transient_steering_receipt(
    db: &crate::db::Database,
    receipts: &mut std::collections::HashMap<(String, String), SteeringAcceptanceReceipt>,
    key: (String, String),
    receipt: SteeringAcceptanceReceipt,
) {
    const CLEANUP_THRESHOLD: usize = 1_024;
    if receipts.len() >= CLEANUP_THRESHOLD {
        let candidates = receipts
            .iter()
            .map(|(key, receipt)| (key.clone(), receipt.conversation_id.clone()))
            .collect::<Vec<_>>();
        for (key, conversation_id) in candidates {
            let id = &key.1;
            let persisted = db.message_exists(id).await.unwrap_or(false);
            let queued = db
                .get_steering_queue(&conversation_id)
                .await
                .is_ok_and(|queue| queue.iter().any(|entry| entry.message_id == *id));
            if persisted || queued {
                receipts.remove(&key);
            }
        }
    }
    receipts.insert(key, receipt);
}

async fn find_queued_message(
    db: &crate::db::Database,
    conversation_id: &str,
    message_id: &str,
) -> Result<Option<(String, phoenix_core::domain::sm_event::SteerEntry)>, SendChatServiceError> {
    let entry = db
        .get_steering_queue(conversation_id)
        .await
        .map_err(|error| SendChatServiceError::Internal(error.to_string()))?
        .into_iter()
        .find(|entry| entry.message_id == message_id);
    Ok(entry.map(|entry| (conversation_id.to_string(), entry)))
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
    use sha2::Digest as _;

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
    Ok(sha2::Sha256::digest(canonical).iter().fold(
        String::with_capacity(64),
        |mut output, byte| {
            write!(output, "{byte:02x}").expect("writing to String cannot fail");
            output
        },
    ))
}

fn replay_steering_receipt(
    receipt: &SteeringAcceptanceReceipt,
    req: &SendChatRequest,
    request_fingerprint: &str,
) -> Result<SendChatOutcome, SendChatServiceError> {
    if receipt.conversation_id != req.conversation_id
        || receipt.request_fingerprint != request_fingerprint
    {
        return Err(SendChatServiceError::IdempotencyConflict);
    }
    Ok(SendChatOutcome::QueuedAsSteering)
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
        lookup_durable_replay, map_conversation_load_error, map_direct_turn_accept_error,
        persisted_skill_matches, queued_retry_matches, replay_steering_receipt,
        submitted_identity_from_request, DurableReplayOutcome, MessageExpansionPolicy,
        SendChatRequest, SendChatServiceError,
    };
    use crate::api::{FileAttachment, ImageAttachment};
    use phoenix_core::domain::db_schema::SkillContent;
    use phoenix_core::domain::sm_event::{
        PreparedDirectTurnDelivery, PreparedDirectTurnPayload, SubmittedDirectTurnExpansionPolicy,
        SubmittedDirectTurnFileAttachment,
    };
    use phoenix_db::workflow::{
        AcceptAuthoritativeTurn, ClaimAuthoritativeTurnInput, MaterializeAuthoritativeTurnInput,
        WorkflowRepository,
    };
    use phoenix_workflow::{
        AcceptedDisposition, ClientTurnKey, ConversationAuthority, LeaseExpiry, PreparedTurn,
        ProcessIncarnation, Timestamp, TurnAuthorityId, TurnConflict, TurnOutcome,
    };

    fn request() -> SendChatRequest {
        SendChatRequest {
            conversation_id: "conv-1".to_string(),
            text: "hello".to_string(),
            message_id: "message-1".to_string(),
            images: vec![],
            files: vec![],
            user_agent: None,
            expansion_policy: MessageExpansionPolicy::ExpandReferences,
        }
    }

    fn prepared_payload(req: &SendChatRequest, delivery_text: &str) -> PreparedDirectTurnPayload {
        PreparedDirectTurnPayload::from_parts(
            submitted_identity_from_request(req),
            PreparedDirectTurnDelivery {
                text: delivery_text.to_string(),
                llm_text: Some(format!("expanded {delivery_text}")),
                images: req
                    .images
                    .iter()
                    .cloned()
                    .map(|image| phoenix_core::domain::db_schema::ImageData {
                        data: image.data,
                        media_type: image.media_type,
                    })
                    .collect(),
                files: req
                    .files
                    .iter()
                    .cloned()
                    .map(|file| phoenix_core::domain::db_schema::FileAttachment {
                        original_name: file.original_name,
                        media_type: file.media_type,
                        size_bytes: file.size_bytes,
                        stored_path: file.stored_path,
                    })
                    .collect(),
                user_agent: req.user_agent.clone(),
                skill_invocation: None,
            },
        )
    }

    fn prepared_turn(
        conversation: &ConversationAuthority,
        payload: &PreparedDirectTurnPayload,
    ) -> PreparedTurn {
        PreparedTurn::from_exact_payload(conversation, payload.to_exact_bytes().unwrap())
    }

    async fn db_with_conversation(conversation_id: &str) -> crate::db::Database {
        let db = crate::db::Database::open_in_memory().await.unwrap();
        db.create_conversation(conversation_id, conversation_id, "/tmp", true, None, None)
            .await
            .unwrap();
        db
    }

    #[test]
    fn submitted_identity_tracks_submitted_fields_not_mutable_expansion() {
        let base = SendChatRequest {
            conversation_id: "conv-1".to_string(),
            text: "@file:notes.md".to_string(),
            message_id: "message-1".to_string(),
            images: vec![ImageAttachment {
                data: "image-a".to_string(),
                media_type: "image/png".to_string(),
            }],
            files: vec![FileAttachment {
                original_name: "a.txt".to_string(),
                media_type: "text/plain".to_string(),
                size_bytes: 7,
                stored_path: "/server/a.txt".to_string(),
            }],
            user_agent: Some("agent/a".to_string()),
            expansion_policy: MessageExpansionPolicy::ExpandReferences,
        };
        let identity = submitted_identity_from_request(&base);
        assert_eq!(identity.text, base.text);
        assert_eq!(identity.message_id, base.message_id);
        assert_eq!(identity.user_agent, base.user_agent);
        assert_eq!(
            identity.expansion_policy,
            SubmittedDirectTurnExpansionPolicy::ExpandReferences
        );
        assert_eq!(identity.images[0].data, "image-a");
        assert_eq!(identity.images[0].media_type, "image/png");
        assert_eq!(
            identity.files[0],
            SubmittedDirectTurnFileAttachment {
                original_name: "a.txt".to_string(),
                media_type: "text/plain".to_string(),
                size_bytes: 7,
                stored_path: "/server/a.txt".to_string(),
            }
        );

        let mut changed = base.clone();
        changed.text = "different text".to_string();
        assert_ne!(identity, submitted_identity_from_request(&changed));
        changed = base.clone();
        changed.images[0].data = "image-b".to_string();
        assert_ne!(identity, submitted_identity_from_request(&changed));
        changed = base.clone();
        changed.files[0].stored_path = "/server/b.txt".to_string();
        assert_ne!(identity, submitted_identity_from_request(&changed));
        changed = base.clone();
        changed.expansion_policy = MessageExpansionPolicy::LiteralText;
        assert_ne!(identity, submitted_identity_from_request(&changed));
        changed = base.clone();
        changed.user_agent = Some("agent/b".to_string());
        assert_ne!(identity, submitted_identity_from_request(&changed));

        let exact = PreparedDirectTurnPayload::from_parts(
            identity.clone(),
            PreparedDirectTurnDelivery {
                text: "rendered after expansion".to_string(),
                llm_text: Some("mutable expansion output".to_string()),
                images: vec![],
                files: vec![],
                user_agent: None,
                skill_invocation: None,
            },
        );
        assert_eq!(exact.submitted, identity);
    }

    #[tokio::test]
    async fn lookup_durable_replay_real_db_returns_exact_states_and_pre_expansion_conflict() {
        let mut req = request();
        req.conversation_id = "conv-replay".to_string();
        req.message_id = "client-key".to_string();
        let db = db_with_conversation(&req.conversation_id).await;
        let repo = WorkflowRepository::new(db.pool().clone());
        let payload = prepared_payload(&req, "first expansion");
        let turn = repo
            .accept_authoritative_turn(&AcceptAuthoritativeTurn {
                client_key: ClientTurnKey::new(req.message_id.clone()).unwrap(),
                prepared: prepared_turn(
                    &ConversationAuthority(req.conversation_id.clone()),
                    &payload,
                ),
                disposition: AcceptedDisposition::Runtime,
                accepted_at: Timestamp(1),
            })
            .await
            .unwrap();
        let TurnOutcome::Created { turn_id, .. } = turn.outcome else {
            panic!("expected created turn")
        };
        assert_eq!(
            lookup_durable_replay(&db, &req, &submitted_identity_from_request(&req))
                .await
                .unwrap(),
            DurableReplayOutcome::ExactUnmaterializedLive
        );

        let mut changed_expansion = req.clone();
        changed_expansion.text = "changed before expansion callback".to_string();
        assert!(matches!(
            lookup_durable_replay(
                &db,
                &changed_expansion,
                &submitted_identity_from_request(&changed_expansion)
            )
            .await,
            Err(SendChatServiceError::IdempotencyConflict)
        ));

        let workflow_id = repo.workflow_id_for_turn(turn_id).await.unwrap().unwrap();
        let claim = repo
            .claim_authoritative_turn(&ClaimAuthoritativeTurnInput {
                turn_id,
                workflow_id,
                process_incarnation: ProcessIncarnation(10),
                now: Timestamp(10),
                lease_until: LeaseExpiry(40),
            })
            .await
            .unwrap();
        repo.materialize_authoritative_turn(&MaterializeAuthoritativeTurnInput {
            turn_id,
            authority: claim.authority.unwrap(),
            prepared: payload,
            sequence_id: 7,
            created_at: Timestamp(11),
            accepted_state: crate::db::ConvState::LlmRequesting { attempt: 1 },
            state_updated_at: chrono::DateTime::from_timestamp(11, 0).unwrap(),
            now: Timestamp(11),
        })
        .await
        .unwrap();
        assert_eq!(
            lookup_durable_replay(&db, &req, &submitted_identity_from_request(&req))
                .await
                .unwrap(),
            DurableReplayOutcome::ExactMaterialized
        );
    }

    #[tokio::test]
    async fn lookup_durable_replay_distinguishes_terminal_unmaterialized_turns() {
        let mut req = request();
        req.conversation_id = "conv-terminal-replay".to_string();
        let db = db_with_conversation(&req.conversation_id).await;
        let repo = WorkflowRepository::new(db.pool().clone());
        let payload = prepared_payload(&req, "accepted before terminal");
        let turn = repo
            .accept_authoritative_turn(&AcceptAuthoritativeTurn {
                client_key: ClientTurnKey::new(req.message_id.clone()).unwrap(),
                prepared: prepared_turn(
                    &ConversationAuthority(req.conversation_id.clone()),
                    &payload,
                ),
                disposition: AcceptedDisposition::Runtime,
                accepted_at: Timestamp(1),
            })
            .await
            .unwrap();
        let TurnOutcome::Created { turn_id, .. } = turn.outcome else {
            panic!("expected created turn")
        };
        repo.terminate_authoritative_turn(phoenix_workflow::TurnCommand::Cancel {
            turn_id,
            expected_generation: 0,
        })
        .await
        .unwrap();

        assert_eq!(
            lookup_durable_replay(&db, &req, &submitted_identity_from_request(&req))
                .await
                .unwrap(),
            DurableReplayOutcome::ExactUnmaterializedTerminal
        );
    }

    #[tokio::test]
    async fn lookup_durable_replay_precedes_archived_rejection_policy() {
        let mut req = request();
        req.conversation_id = "conv-archived-replay".to_string();
        let db = db_with_conversation(&req.conversation_id).await;
        let repo = WorkflowRepository::new(db.pool().clone());
        let payload = prepared_payload(&req, "accepted before archive");
        repo.accept_authoritative_turn(&AcceptAuthoritativeTurn {
            client_key: ClientTurnKey::new(req.message_id.clone()).unwrap(),
            prepared: prepared_turn(
                &ConversationAuthority(req.conversation_id.clone()),
                &payload,
            ),
            disposition: AcceptedDisposition::Runtime,
            accepted_at: Timestamp(1),
        })
        .await
        .unwrap();
        db.archive_conversation(&req.conversation_id).await.unwrap();

        assert_eq!(
            lookup_durable_replay(&db, &req, &submitted_identity_from_request(&req))
                .await
                .unwrap(),
            DurableReplayOutcome::ExactUnmaterializedLive
        );
    }

    #[test]
    fn steering_acceptance_receipt_replay_uses_submitted_request_fingerprint() {
        let req = request();
        let fingerprint = super::request_fingerprint(&req).unwrap();
        let receipt = crate::runtime::SteeringAcceptanceReceipt {
            conversation_id: req.conversation_id.clone(),
            request_fingerprint: fingerprint.clone(),
        };
        assert_eq!(
            replay_steering_receipt(&receipt, &req, &fingerprint).unwrap(),
            super::SendChatOutcome::QueuedAsSteering
        );

        let mut changed = req.clone();
        changed.user_agent = Some("changed".to_string());
        let changed_fingerprint = super::request_fingerprint(&changed).unwrap();
        assert!(matches!(
            replay_steering_receipt(&receipt, &changed, &changed_fingerprint),
            Err(SendChatServiceError::IdempotencyConflict)
        ));
    }

    #[test]
    fn typed_db_errors_map_to_distinct_service_errors() {
        assert!(matches!(
            map_conversation_load_error(crate::db::DbError::ConversationNotFound(
                "missing".to_string()
            )),
            SendChatServiceError::NotFound(_)
        ));
        assert!(matches!(
            map_conversation_load_error(crate::db::DbError::Sqlx(sqlx::Error::RowNotFound)),
            SendChatServiceError::NotFound(_)
        ));
        assert!(matches!(
            map_direct_turn_accept_error(crate::db::DbError::DirectTurnConflict(
                TurnConflict::PreparedSemanticsChanged {
                    authoritative_fingerprint: "fp".to_string()
                }
            )),
            SendChatServiceError::IdempotencyConflict
        ));
        assert!(matches!(
            map_direct_turn_accept_error(crate::db::DbError::DirectTurnConflict(
                TurnConflict::ConversationAlreadyOwned {
                    owner: TurnAuthorityId(99)
                }
            )),
            SendChatServiceError::Busy
        ));
        assert!(matches!(
            map_direct_turn_accept_error(crate::db::DbError::DirectTurnConflict(
                TurnConflict::UnknownTurn
            )),
            SendChatServiceError::Internal(_)
        ));
    }

    #[test]
    fn prepared_turn_fingerprint_is_scoped_to_conversation_authority() {
        let req = request();
        let payload = prepared_payload(&req, "first expansion");
        let conv_a = ConversationAuthority("conv-a".to_string());
        let conv_b = ConversationAuthority("conv-b".to_string());

        let prepared_a = prepared_turn(&conv_a, &payload);
        let prepared_b = prepared_turn(&conv_b, &payload);

        assert_ne!(prepared_a.fingerprint(), prepared_b.fingerprint());
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
