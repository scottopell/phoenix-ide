use std::{fmt::Write as _, sync::Arc};

use phoenix_core::domain::db_schema::InputOrigin;
use phoenix_db::{CloseFailureStop, PendingWatchEvent, WatchEventRoute};

use crate::runtime::RuntimeManager;
use crate::send_chat_service::{
    MessageExpansionPolicy, SendChatApplicationService, SendChatOutcome, SendChatRequest,
};

fn notification(event: &PendingWatchEvent) -> String {
    let label = match &event.route {
        WatchEventRoute::Subscription => "Watched conversation terminal event",
        WatchEventRoute::MandatoryCloseFailure { .. } => "Mandatory Close cleanup failure",
    };
    let mut text = format!(
        "{label}. Event ID: {}. Stable ProductConversation: {}. Source transcript: {}. Source occurrence: {} {}. Source generation: {}. Outcome: {}. Occurred at (Unix microseconds): {}.",
        event.event_id,
        event.product_conversation_id.as_str(),
        event.source_transcript_id,
        event.source_occurrence_kind,
        event.source_occurrence_id,
        event.source_generation,
        event.terminal_kind,
        event.occurred_at_us,
    );
    if let Some(reason) = &event.terminal_reason {
        text.push_str(" Reason: ");
        text.push_str(reason);
    }
    if let WatchEventRoute::MandatoryCloseFailure {
        scope,
        resource_kind,
        identity_kind,
        identity_codec,
        identity_value,
        detail,
        stop,
    } = &event.route
    {
        let _ = write!(
            text,
            " Scope: {scope}. Resource kind: {resource_kind}. Identity ({identity_kind}, {identity_codec}): {identity_value}. Detail: {detail}."
        );
        match stop {
            CloseFailureStop::ConversationAndProcessesStopped {
                confirmed_at_unix_us,
            } => {
                let _ = write!(
                    text,
                    " Conversation and processes stopped, confirmed at (Unix microseconds): {confirmed_at_unix_us}. Cleanup needs attention."
                );
            }
            CloseFailureStop::ShutdownUncertain => {
                text.push_str(" Shutdown uncertain. Close incomplete.");
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use phoenix_core::domain::product_conversation::ProductConversationId;

    fn failure(stop: CloseFailureStop) -> PendingWatchEvent {
        PendingWatchEvent {
            route: WatchEventRoute::MandatoryCloseFailure {
                scope: "scope-1".into(),
                resource_kind: "worktree".into(),
                identity_kind: "path".into(),
                identity_codec: "utf8".into(),
                identity_value: "/preserved/worktree".into(),
                detail: "resource preserved".into(),
                stop,
            },
            event_id: "failure-1".into(),
            product_conversation_id: ProductConversationId::parse("product-1").unwrap(),
            source_transcript_id: "root-1".into(),
            source_occurrence_kind: "close_cleanup_failure".into(),
            source_occurrence_id: "failure-1".into(),
            source_generation: 0,
            terminal_kind: "cleanup_failed".into(),
            terminal_reason: Some("remove failed".into()),
            occurred_at_us: 20,
        }
    }

    #[test]
    fn confirmed_cleanup_failure_is_not_an_ordinary_watch_or_success() {
        let text = notification(&failure(
            CloseFailureStop::ConversationAndProcessesStopped {
                confirmed_at_unix_us: 10,
            },
        ));
        for fact in [
            "Mandatory Close cleanup failure",
            "product-1",
            "root-1",
            "failure-1",
            "scope-1",
            "worktree",
            "/preserved/worktree",
            "resource preserved",
            "remove failed",
            "Conversation and processes stopped",
            "Cleanup needs attention",
        ] {
            assert!(text.contains(fact), "missing {fact}: {text}");
        }
        assert!(!text.contains("Watched conversation"));
        assert!(!text.contains("Close incomplete"));
    }

    #[test]
    fn uncertain_shutdown_never_claims_confirmed_stop() {
        let text = notification(&failure(CloseFailureStop::ShutdownUncertain));
        assert!(text.contains("Shutdown uncertain. Close incomplete."));
        assert!(!text.contains("Conversation and processes stopped"));
        assert!(!text.contains("confirmed at"));
    }
}
