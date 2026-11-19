//! Inputs the runtime hands to the Coordinator on its own: watched-conversation
//! events and replies to the Coordinator's questions (REQ-AUQ-011). Both go to
//! the current Coordinator transcript through the ordinary send-chat path.

use std::sync::Arc;

use phoenix_core::domain::db_schema::InputOrigin;

use crate::db::Database;
use crate::runtime::RuntimeManager;
use crate::send_chat_service::{
    MessageExpansionPolicy, SendChatApplicationService, SendChatOutcome, SendChatRequest,
    SendChatServiceError,
};

/// Deliver pending watch events and question replies to the current
/// Coordinator transcript.
pub(crate) async fn deliver_pass(runtime: &Arc<RuntimeManager>) {
    let db = runtime.db();
    let events = db
        .pending_coordinator_watch_events(16)
        .await
        .unwrap_or_else(|error| {
            tracing::warn!(%error, "coordinator watch event discovery failed; retrying");
            Vec::new()
        });
    let replies = db
        .undelivered_coordinator_question_replies(16)
        .await
        .unwrap_or_else(|error| {
            tracing::warn!(%error, "coordinator question reply discovery failed; retrying");
            Vec::new()
        });
    if events.is_empty() && replies.is_empty() {
        return;
    }
    let target = match db.coordinator_watch_target().await {
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
    let sender = CoordinatorSender {
        service: SendChatApplicationService::new(db.clone(), runtime.clone()),
        db: db.clone(),
        target,
    };
    crate::coordinator_watch_delivery::deliver(&sender, events).await;
    crate::coordinator_question_delivery::deliver(&sender, replies).await;
}

/// Sends literal-text inputs to one Coordinator transcript.
pub(crate) struct CoordinatorSender {
    service: SendChatApplicationService,
    db: Database,
    target: String,
}

impl CoordinatorSender {
    pub(crate) fn db(&self) -> &Database {
        &self.db
    }

    pub(crate) async fn send(
        &self,
        origin: InputOrigin,
        text: String,
        message_id: String,
    ) -> Result<SendChatOutcome, SendChatServiceError> {
        self.service
            .send(SendChatRequest {
                conversation_id: self.target.clone(),
                origin,
                text,
                message_id,
                images: Vec::new(),
                files: Vec::new(),
                user_agent: None,
                expansion_policy: MessageExpansionPolicy::LiteralText,
            })
            .await
    }
}
