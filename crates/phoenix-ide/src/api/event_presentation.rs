use std::collections::HashMap;

use phoenix_core::domain::db_schema::InputOrigin;
use phoenix_db::{Database, Message, WatchEventPresentation, WatchEventProvenance, WatchOutcome};
use serde::Serialize;
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, TS)]
#[serde(tag = "status", rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum EventPresentation {
    Available(EventPresentationPayload),
    Unavailable {
        event_id: String,
        reason: EventPresentationUnavailableReason,
    },
}

#[derive(Debug, Clone, Copy, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum EventPresentationUnavailableReason {
    Missing,
    LookupFailed,
    Redacted,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct EventPresentationPayload {
    pub event_id: String,
    pub product_conversation_id: String,
    pub display_name: Option<String>,
    pub source_transcript_id: String,
    pub source_occurrence_kind: String,
    pub source_occurrence_id: String,
    pub source_generation: i64,
    pub outcome: EventOutcome,
    pub occurred_at_us: i64,
    pub provenance: EventProvenance,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum EventOutcome {
    TurnFinished,
    Failed { reason: String },
    CleanupFailed { reason: String },
    Cancelled,
    AwaitingUserResponse,
    AwaitingTaskApproval,
    GoalChanged,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum EventProvenance {
    Subscription,
    MandatoryCloseFailure,
}

impl From<WatchEventPresentation> for EventPresentationPayload {
    fn from(event: WatchEventPresentation) -> Self {
        Self {
            event_id: event.event_id,
            product_conversation_id: event.product_conversation_id.to_string(),
            display_name: event.display_name,
            source_transcript_id: event.source_transcript_id,
            source_occurrence_kind: event.source_occurrence_kind,
            source_occurrence_id: event.source_occurrence_id,
            source_generation: event.source_generation,
            occurred_at_us: event.occurred_at_us,
            outcome: match event.outcome {
                WatchOutcome::Completed => EventOutcome::TurnFinished,
                WatchOutcome::Failed { reason } => EventOutcome::Failed { reason },
                WatchOutcome::CleanupFailed { reason } => EventOutcome::CleanupFailed { reason },
                WatchOutcome::Cancelled => EventOutcome::Cancelled,
                WatchOutcome::AwaitingUserResponse => EventOutcome::AwaitingUserResponse,
                WatchOutcome::AwaitingTaskApproval => EventOutcome::AwaitingTaskApproval,
                WatchOutcome::GoalChanged => EventOutcome::GoalChanged,
            },
            provenance: match event.provenance {
                WatchEventProvenance::Subscription => EventProvenance::Subscription,
                WatchEventProvenance::MandatoryCloseFailure => {
                    EventProvenance::MandatoryCloseFailure
                }
            },
        }
    }
}

pub(crate) struct EventPresentations {
    events: HashMap<String, EventPresentationPayload>,
    fallback: EventPresentationUnavailableReason,
}

impl EventPresentations {
    pub(crate) fn redacted() -> Self {
        Self {
            events: HashMap::new(),
            fallback: EventPresentationUnavailableReason::Redacted,
        }
    }

    pub(crate) async fn load<'a>(
        db: &Database,
        messages: impl IntoIterator<Item = &'a Message>,
    ) -> Self {
        let ids: Vec<_> = messages
            .into_iter()
            .filter_map(|message| match &message.origin {
                InputOrigin::SubscriptionEvent { event_id } => Some(event_id.clone()),
                InputOrigin::UnknownHistorical
                | InputOrigin::UserApi
                | InputOrigin::InternalConversation { .. }
                | InputOrigin::SystemGenerated => None,
            })
            .collect();
        match db.coordinator_watch_event_presentations(&ids).await {
            Ok(events) => Self {
                events: events
                    .into_iter()
                    .map(|(id, event)| (id, event.into()))
                    .collect(),
                fallback: EventPresentationUnavailableReason::Missing,
            },
            Err(error) => {
                tracing::warn!(%error, "event presentation lookup failed; preserving original messages");
                Self {
                    events: HashMap::new(),
                    fallback: EventPresentationUnavailableReason::LookupFailed,
                }
            }
        }
    }

    pub(crate) fn for_message(&self, message: &Message) -> Option<EventPresentation> {
        match &message.origin {
            InputOrigin::SubscriptionEvent { event_id } => {
                Some(self.events.get(event_id).map_or_else(
                    || EventPresentation::Unavailable {
                        event_id: event_id.clone(),
                        reason: self.fallback,
                    },
                    |event| EventPresentation::Available(event.clone()),
                ))
            }
            InputOrigin::UnknownHistorical
            | InputOrigin::UserApi
            | InputOrigin::InternalConversation { .. }
            | InputOrigin::SystemGenerated => None,
        }
    }

    pub(crate) fn enrich(&self, message: &Message) -> super::wire::EnrichedMessage {
        super::wire::EnrichedMessage::new(message, self)
    }
}

pub(crate) async fn enrich_messages(
    db: &Database,
    messages: &[Message],
) -> Vec<super::wire::EnrichedMessage> {
    let presentations = EventPresentations::load(db, messages).await;
    messages
        .iter()
        .map(|message| presentations.enrich(message))
        .collect()
}

pub(crate) fn event_messages(event: &crate::runtime::SseEvent) -> Vec<&Message> {
    use crate::runtime::SseEvent;
    match event {
        SseEvent::Init {
            transcript,
            pending_events,
            ..
        } => {
            let messages: &[Message] = match transcript {
                crate::runtime::InitTranscript::Complete(messages)
                | crate::runtime::InitTranscript::Tail(messages) => messages,
                crate::runtime::InitTranscript::Preserve => &[],
            };
            messages
                .iter()
                .chain(pending_events.iter().flat_map(event_messages))
                .collect()
        }
        SseEvent::Message { message, .. } => vec![message],
        SseEvent::MessageUpdated { .. }
        | SseEvent::StateChange { .. }
        | SseEvent::Token { .. }
        | SseEvent::LlmFirstByte { .. }
        | SseEvent::LlmAttempt { .. }
        | SseEvent::AgentDone { .. }
        | SseEvent::ConversationBecameTerminal { .. }
        | SseEvent::ConversationUpdate { .. }
        | SseEvent::Error { .. }
        | SseEvent::ConversationHardDeleted { .. }
        | SseEvent::BrowserSessionState { .. }
        | SseEvent::BashToolProgress { .. }
        | SseEvent::SteerMessageQueued { .. }
        | SseEvent::SteerMessageCancelled { .. }
        | SseEvent::RateLimitSnapshot { .. }
        | SseEvent::WorkScopeUpdate { .. } => Vec::new(),
    }
}

#[cfg(test)]
pub(crate) mod tests;
