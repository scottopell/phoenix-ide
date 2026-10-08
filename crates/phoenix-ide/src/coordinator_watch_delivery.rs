use std::sync::Arc;

use phoenix_core::domain::db_schema::InputOrigin;
use phoenix_db::PendingWatchEvent;

use crate::runtime::RuntimeManager;
use crate::send_chat_service::{
    MessageExpansionPolicy, SendChatApplicationService, SendChatOutcome, SendChatRequest,
};

fn notification(event: &PendingWatchEvent) -> String {
    let mut text = format!(
        "Watched conversation event. Event ID: {}. Stable ProductConversation: {}. Source transcript: {}. Source occurrence: {} {}. Source generation: {}. Outcome: {}. Occurred at (Unix microseconds): {}.",
        event.event_id,
        event.product_conversation_id.as_str(),
        event.source_transcript_id,
        event.source_occurrence_kind,
        event.source_occurrence_id,
        event.source_generation,
        event.outcome.label(),
        event.occurred_at_us,
    );
    if let Some(reason) = event.outcome.reason() {
        text.push_str(" Reason: ");
        text.push_str(reason);
    }
    text
}

pub(crate) async fn deliver_pass(runtime: &Arc<RuntimeManager>) {
    let db = runtime.db();
    let events = match db.pending_coordinator_watch_events(16).await {
        Ok(events) => events,
        Err(error) => {
            tracing::warn!(%error, "coordinator watch event discovery failed; retrying");
            return;
        }
    };
    if events.is_empty() {
        return;
    }
    let Some(target) = (match db.coordinator_watch_target().await {
        Ok(target) => target,
        Err(error) => {
            tracing::warn!(%error, "coordinator watch target lookup failed; retrying");
            return;
        }
    }) else {
        tracing::debug!("coordinator watch delivery awaiting coordinator transcript");
        return;
    };
    let service = SendChatApplicationService::new(db.clone(), runtime.clone());
    for event in events {
        let request = SendChatRequest {
            conversation_id: target.clone(),
            origin: InputOrigin::SubscriptionEvent {
                event_id: event.event_id.clone(),
            },
            text: notification(&event),
            message_id: event.event_id.clone(),
            images: Vec::new(),
            files: Vec::new(),
            user_agent: None,
            expansion_policy: MessageExpansionPolicy::LiteralText,
        };
        match service.send(request).await {
            Ok(
                SendChatOutcome::Delivered
                | SendChatOutcome::AlreadyPersisted
                | SendChatOutcome::QueuedAsSteering,
            ) => {}
            Ok(SendChatOutcome::Rejected { message, code }) => {
                tracing::warn!(event_id = %event.event_id, code, %message, "coordinator watch event not accepted; retrying");
            }
            Err(error) => {
                tracing::warn!(event_id = %event.event_id, %error, "coordinator watch event delivery failed; retrying");
                if let Err(suppression_error) = db.suppress_stale_watch_event(&event.event_id).await
                {
                    tracing::warn!(%suppression_error, "watch stale-event suppression failed");
                }
            }
        }
    }
}
