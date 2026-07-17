use crate::api::{record_pr_auto_fix_context_baseline, validate_submitted_attachments};
use crate::api::{FileAttachment, ImageAttachment};
use crate::db::ConvState;
use crate::runtime::{ChatAcceptanceReceipt, RuntimeManager};
use crate::state_machine::{check_user_message_acceptable, Event, TransitionError};
use phoenix_core::domain::db_schema::ImageData;
use phoenix_core::domain::skill_invocation::SkillInvocation;
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
        if let Ok(message) = self.db.get_message_by_id(&req.message_id).await {
            let persisted_matches = match &message.content {
                phoenix_core::domain::db_schema::MessageContent::Skill(skill) => {
                    let expanded = expand_request(&self.db, &conversation, &req).await?;
                    persisted_skill_matches(skill, &req, &expanded)
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
            receipts.remove(&req.message_id);
            return Ok(SendChatOutcome::AlreadyPersisted);
        }
        if let Some(receipt) = receipts.get(&req.message_id) {
            return replay_receipt(receipt, &req, &request_fingerprint);
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
        if let Some(entry) = steering_queue
            .iter()
            .find(|entry| entry.message_id == req.message_id)
        {
            let validated_files = validate_files(&req).await?;
            let expanded = expand_request(&self.db, &conversation, &req).await?;
            if !queued_entry_matches(entry, &req, &expanded, &validated_files) {
                return Err(SendChatServiceError::IdempotencyConflict);
            }
            receipts.remove(&req.message_id);
            return Ok(SendChatOutcome::QueuedAsSteering);
        }

        let effective_state = self
            .effective_state(&conversation.id, &conversation.state)
            .await?;
        if let Err(err) = check_user_message_acceptable(&effective_state) {
            if matches!(
                err,
                TransitionError::AgentBusy | TransitionError::CancellationInProgress
            ) {
                const MAX_STEER_QUEUE_DEPTH: usize = 5;
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
                let event = Event::SteerMessage {
                    text: expanded.display_text.clone(),
                    llm_text: expanded.llm_text,
                    images: map_images(req.images),
                    files: validated_files.clone(),
                    message_id: req.message_id.clone(),
                    user_agent: req.user_agent,
                    skill_invocation: expanded.skill_invocation,
                };
                self.runtime
                    .enqueue_steer_message(&conversation.id, event)
                    .await
                    .map_err(SendChatServiceError::Dispatch)?;
                insert_bounded_receipt(
                    &mut receipts,
                    req.message_id.clone(),
                    ChatAcceptanceReceipt {
                        conversation_id: conversation.id.clone(),
                        request_fingerprint,
                        steering: true,
                    },
                );
                record_pr_auto_fix_context_baseline(
                    self.runtime.db(),
                    &conversation.id,
                    &expanded.display_text,
                )
                .await
                .map_err(|e| SendChatServiceError::Internal(format!("{e:?}")))?;
                return Ok(SendChatOutcome::QueuedAsSteering);
            }
            return Ok(SendChatOutcome::Rejected {
                message: err.to_string(),
                code: transition_code(&err),
            });
        }

        let validated_files = validate_files(&req).await?;
        let expanded = expand_request(&self.db, &conversation, &req).await?;
        let event = Event::UserMessage {
            text: expanded.display_text.clone(),
            llm_text: expanded.llm_text,
            images: map_images(req.images),
            files: validated_files,
            message_id: req.message_id.clone(),
            user_agent: req.user_agent,
            skill_invocation: expanded.skill_invocation,
        };
        self.runtime
            .send_event(&conversation.id, event)
            .await
            .map_err(SendChatServiceError::Dispatch)?;
        insert_bounded_receipt(
            &mut receipts,
            req.message_id.clone(),
            ChatAcceptanceReceipt {
                conversation_id: conversation.id.clone(),
                request_fingerprint,
                steering: false,
            },
        );
        record_pr_auto_fix_context_baseline(
            self.runtime.db(),
            &conversation.id,
            &expanded.display_text,
        )
        .await
        .map_err(|e| SendChatServiceError::Internal(format!("{e:?}")))?;
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

fn insert_bounded_receipt(
    receipts: &mut std::collections::HashMap<String, ChatAcceptanceReceipt>,
    message_id: String,
    receipt: ChatAcceptanceReceipt,
) {
    const MAX_TRANSIENT_RECEIPTS: usize = 1_024;
    if receipts.len() >= MAX_TRANSIENT_RECEIPTS {
        if let Some(expired_id) = receipts.keys().next().cloned() {
            receipts.remove(&expired_id);
        }
    }
    receipts.insert(message_id, receipt);
}

fn queued_entry_matches(
    entry: &phoenix_core::domain::sm_event::SteerEntry,
    req: &SendChatRequest,
    expanded: &ExpandedDispatchMessage,
    validated_files: &[phoenix_core::domain::db_schema::FileAttachment],
) -> bool {
    entry.text == expanded.display_text
        && entry.llm_text == expanded.llm_text
        && entry.images.len() == req.images.len()
        && entry
            .images
            .iter()
            .zip(&req.images)
            .all(|(stored, requested)| {
                stored.data == requested.data && stored.media_type == requested.media_type
            })
        && entry.files == validated_files
        && entry.user_agent == req.user_agent
        && entry.skill_invocation == expanded.skill_invocation
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
    expanded: &ExpandedDispatchMessage,
) -> bool {
    let Some(invocation) = expanded.skill_invocation.as_ref() else {
        return false;
    };
    skill.name == invocation.name
        && skill.body == invocation.body
        && skill.trigger == req.text
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
    Ok(if receipt.steering {
        SendChatOutcome::QueuedAsSteering
    } else {
        SendChatOutcome::Delivered
    })
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
        persisted_skill_matches, ExpandedDispatchMessage, MessageExpansionPolicy, SendChatRequest,
    };
    use phoenix_core::domain::db_schema::SkillContent;
    use phoenix_core::domain::skill_invocation::SkillInvocation;

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
        let expanded = ExpandedDispatchMessage {
            display_text: request.text.clone(),
            llm_text: None,
            skill_invocation: Some(SkillInvocation {
                name: "build".to_string(),
                body: "expanded body".to_string(),
                skill_dir: "/skills/build".to_string(),
            }),
        };
        let persisted = SkillContent {
            name: "build".to_string(),
            body: "expanded body".to_string(),
            trigger: request.text.clone(),
            files: vec![],
        };

        assert!(persisted_skill_matches(&persisted, &request, &expanded));
    }
}
