use crate::api::{record_pr_auto_fix_context_baseline, validate_submitted_attachments};
use crate::api::{FileAttachment, ImageAttachment};
use crate::db::ConvState;
use crate::runtime::RuntimeManager;
use crate::state_machine::{check_user_message_acceptable, Event, TransitionError};
use phoenix_core::domain::db_schema::ImageData;
use phoenix_core::domain::skill_invocation::SkillInvocation;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub(crate) struct SendChatRequest {
    pub conversation_id: String,
    pub text: String,
    pub message_id: String,
    pub images: Vec<ImageAttachment>,
    pub files: Vec<FileAttachment>,
    pub user_agent: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SendChatOutcome {
    Delivered,
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
        if self
            .db
            .message_exists(&req.message_id)
            .await
            .map_err(|e| SendChatServiceError::Internal(e.to_string()))?
        {
            return Ok(SendChatOutcome::Delivered);
        }

        let conversation = self
            .runtime
            .db()
            .get_conversation(&req.conversation_id)
            .await
            .map_err(|e| SendChatServiceError::NotFound(e.to_string()))?;
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
        if steering_queue
            .iter()
            .any(|e| e.message_id == req.message_id)
        {
            return Ok(SendChatOutcome::QueuedAsSteering);
        }

        let validated_files = validate_submitted_attachments(&req.conversation_id, &req.files)
            .await
            .map_err(|e| SendChatServiceError::AttachmentValidation(format!("{e:?}")))?;

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
                let expanded =
                    expand_message(&self.db, &conversation.id, &conversation.cwd, &req.text)
                        .await?;
                let event = Event::SteerMessage {
                    text: expanded.display_text.clone(),
                    llm_text: expanded.llm_text,
                    images: map_images(req.images),
                    files: validated_files.clone(),
                    message_id: req.message_id,
                    user_agent: req.user_agent,
                    skill_invocation: expanded.skill_invocation,
                };
                self.runtime
                    .enqueue_steer_message(&conversation.id, event)
                    .await
                    .map_err(SendChatServiceError::Dispatch)?;
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

        let expanded =
            expand_message(&self.db, &conversation.id, &conversation.cwd, &req.text).await?;
        let event = Event::UserMessage {
            text: expanded.display_text.clone(),
            llm_text: expanded.llm_text,
            images: map_images(req.images),
            files: validated_files,
            message_id: req.message_id,
            user_agent: req.user_agent,
            skill_invocation: expanded.skill_invocation,
        };
        self.runtime
            .send_event(&conversation.id, event)
            .await
            .map_err(SendChatServiceError::Dispatch)?;
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

async fn expand_message(
    db: &crate::db::Database,
    conversation_id: &str,
    cwd: &str,
    text: &str,
) -> Result<ExpandedDispatchMessage, SendChatServiceError> {
    let resolution_root = crate::resolution_root::ResolutionRoot::working_dir(cwd);
    let expanded = if db
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
