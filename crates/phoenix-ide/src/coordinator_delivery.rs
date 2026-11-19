//! Inputs the runtime hands to the Coordinator on its own: watched-conversation
//! events and replies to the Coordinator's questions (REQ-AUQ-011). Both go to
//! the current Coordinator transcript through the ordinary send-chat path.

use std::sync::Arc;

use phoenix_core::domain::db_schema::InputOrigin;

use crate::runtime::RuntimeManager;
use crate::send_chat_service::{
    MessageExpansionPolicy, SendChatApplicationService, SendChatOutcome, SendChatRequest,
    SendChatServiceError,
};

/// Run every Coordinator input pass against one resolved Coordinator transcript.
pub(crate) async fn deliver_pass(runtime: &Arc<RuntimeManager>) {
    let target = match runtime.db().coordinator_watch_target().await {
        Ok(Some(target)) => target,
        Ok(None) => {
            tracing::debug!("coordinator inputs awaiting a coordinator transcript");
            return;
        }
        Err(error) => {
            tracing::warn!(%error, "coordinator transcript lookup failed; retrying");
            return;
        }
    };
    let sender = CoordinatorSender::new(runtime, target);
    crate::coordinator_watch_delivery::deliver_pass(runtime, &sender).await;
    crate::coordinator_question_delivery::deliver_pass(runtime, &sender).await;
}

/// How the Coordinator transcript answered one input.
#[derive(Debug)]
pub(crate) enum CoordinatorSend {
    /// Accepted directly, queued as steering, or already present.
    Accepted,
    /// Refused by admission; the input stays pending.
    Rejected { code: &'static str, message: String },
    /// The send itself failed.
    Failed(SendChatServiceError),
}

/// Sends literal-text inputs to one Coordinator transcript.
pub(crate) struct CoordinatorSender {
    service: SendChatApplicationService,
    target: String,
}

impl CoordinatorSender {
    fn new(runtime: &Arc<RuntimeManager>, target: String) -> Self {
        Self {
            service: SendChatApplicationService::new(runtime.db().clone(), runtime.clone()),
            target,
        }
    }

    pub(crate) async fn send(
        &self,
        origin: InputOrigin,
        text: String,
        message_id: String,
    ) -> CoordinatorSend {
        let request = SendChatRequest {
            conversation_id: self.target.clone(),
            origin,
            text,
            message_id,
            images: Vec::new(),
            files: Vec::new(),
            user_agent: None,
            expansion_policy: MessageExpansionPolicy::LiteralText,
        };
        match self.service.send(request).await {
            Ok(
                SendChatOutcome::Delivered
                | SendChatOutcome::AlreadyPersisted
                | SendChatOutcome::QueuedAsSteering,
            ) => CoordinatorSend::Accepted,
            Ok(SendChatOutcome::Rejected { message, code }) => {
                CoordinatorSend::Rejected { code, message }
            }
            Err(error) => CoordinatorSend::Failed(error),
        }
    }
}
