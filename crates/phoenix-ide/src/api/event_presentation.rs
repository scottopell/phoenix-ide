use std::collections::HashMap;

use phoenix_core::domain::{db_schema::InputOrigin, federation_subscription::SubscriptionDelivery};
use phoenix_db::{Database, Message, WatchEventPresentation, WatchEventProvenance, WatchOutcome};
use serde::Serialize;
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, TS)]
#[serde(tag = "status", rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum EventPresentation {
    Available(EventPresentationPayload),
    FederatedFact(FederatedFactPresentation),
    FederatedGap(FederatedGapPresentation),
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
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum EventProvenance {
    Subscription,
    MandatoryCloseFailure,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct FederatedFactPresentation {
    pub event_id: String,
    pub instance_id: String,
    pub product_conversation_id: String,
    pub source_transcript_id: String,
    pub outcome: FederatedEventOutcome,
    pub occurred_at_us: i64,
    pub navigation: FederatedEventNavigation,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct FederatedGapPresentation {
    pub event_id: String,
    pub instance_id: String,
    pub product_conversation_id: String,
    pub requested_after: i64,
    pub resume_after: i64,
    pub current_transcript_id: String,
    pub current: FederatedCurrentState,
    pub navigation: FederatedEventNavigation,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum FederatedEventOutcome {
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum FederatedCurrentState {
    Nonterminal,
    Terminal { outcome: FederatedEventOutcome },
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(tag = "status", rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum FederatedEventNavigation {
    Available {
        stable_url: String,
        source_url: String,
    },
    Unavailable {
        reason: FederatedNavigationUnavailableReason,
    },
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum FederatedNavigationUnavailableReason {
    MissingConnection,
    MissingLocator,
    InvalidLocator,
}

impl From<phoenix_core::domain::federation_subscription::TerminalFactOutcome>
    for FederatedEventOutcome
{
    fn from(outcome: phoenix_core::domain::federation_subscription::TerminalFactOutcome) -> Self {
        use phoenix_core::domain::federation_subscription::TerminalFactOutcome;
        match outcome {
            TerminalFactOutcome::Completed => Self::Completed,
            TerminalFactOutcome::Failed => Self::Failed,
            TerminalFactOutcome::Cancelled => Self::Cancelled,
        }
    }
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
    events: HashMap<String, EventPresentation>,
    fallback: EventPresentationUnavailableReason,
}

impl EventPresentations {
    pub(crate) fn redacted() -> Self {
        Self {
            events: HashMap::new(),
            fallback: EventPresentationUnavailableReason::Redacted,
        }
    }

    #[allow(clippy::too_many_lines)]
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
                | InputOrigin::RemoteApi { .. }
                | InputOrigin::RemoteConversation { .. }
                | InputOrigin::RemoteCoordinator { .. }
                | InputOrigin::SystemGenerated => None,
            })
            .collect();
        if ids.is_empty() {
            return Self {
                events: HashMap::new(),
                fallback: EventPresentationUnavailableReason::Missing,
            };
        }
        let (remote_ids, local_ids): (Vec<_>, Vec<_>) = ids
            .into_iter()
            .partition(|id| id.starts_with("federation:"));
        let mut result = Self {
            events: HashMap::new(),
            fallback: EventPresentationUnavailableReason::Missing,
        };
        if !local_ids.is_empty() {
            match db.coordinator_watch_event_presentations(&local_ids).await {
                Ok(events) => result.events.extend(
                    events
                        .into_iter()
                        .map(|(id, event)| (id, EventPresentation::Available(event.into()))),
                ),
                Err(error) => {
                    tracing::warn!(%error, "local event presentation lookup failed; preserving original messages");
                    result.fallback = EventPresentationUnavailableReason::LookupFailed;
                }
            }
        }
        if !remote_ids.is_empty() {
            match db.remote_notification_presentations(&remote_ids).await {
                Ok(events) => {
                    for (id, (notification, locator)) in events {
                        let navigation =
                            federated_navigation(db, &notification.reference, locator).await;
                        let event = match notification.content {
                            SubscriptionDelivery::Fact { fact } => EventPresentation::FederatedFact(FederatedFactPresentation {
                                event_id: id.clone(), instance_id: notification.reference.instance_id.to_string(),
                                product_conversation_id: notification.reference.product_conversation_id.to_string(),
                                source_transcript_id: fact.source_transcript_id.to_string(), outcome: fact.outcome.into(),
                                occurred_at_us: fact.occurred_at_us, navigation,
                            }),
                            SubscriptionDelivery::Gap { gap } => EventPresentation::FederatedGap(FederatedGapPresentation {
                                event_id: id.clone(), instance_id: notification.reference.instance_id.to_string(),
                                product_conversation_id: notification.reference.product_conversation_id.to_string(),
                                requested_after: gap.requested_after.get(), resume_after: gap.resume_after.get(),
                                current_transcript_id: gap.snapshot.transcript_id.to_string(),
                                current: match gap.snapshot.state {
                                    phoenix_core::domain::federation_subscription::SubscriptionCurrentState::Nonterminal => FederatedCurrentState::Nonterminal,
                                    phoenix_core::domain::federation_subscription::SubscriptionCurrentState::Terminal { outcome } => FederatedCurrentState::Terminal { outcome: outcome.into() },
                                }, navigation,
                            }),
                        };
                        result.events.insert(id, event);
                    }
                }
                Err(error) => {
                    tracing::warn!(%error, "federated event presentation lookup failed; preserving original messages");
                    result.fallback = EventPresentationUnavailableReason::LookupFailed;
                }
            }
        }
        result
    }

    pub(crate) fn for_message(&self, message: &Message) -> Option<EventPresentation> {
        match &message.origin {
            InputOrigin::SubscriptionEvent { event_id } => {
                Some(self.events.get(event_id).map_or_else(
                    || EventPresentation::Unavailable {
                        event_id: event_id.clone(),
                        reason: self.fallback,
                    },
                    Clone::clone,
                ))
            }
            InputOrigin::UnknownHistorical
            | InputOrigin::UserApi
            | InputOrigin::InternalConversation { .. }
            | InputOrigin::RemoteApi { .. }
            | InputOrigin::RemoteConversation { .. }
            | InputOrigin::RemoteCoordinator { .. }
            | InputOrigin::SystemGenerated => None,
        }
    }

    pub(crate) fn enrich(&self, message: &Message) -> super::wire::EnrichedMessage {
        super::wire::EnrichedMessage::new(message, self)
    }
}

async fn federated_navigation(
    db: &Database,
    reference: &phoenix_core::domain::federation_subscription::SubscriptionReference,
    locator: Option<(String, String)>,
) -> FederatedEventNavigation {
    let Ok(Some(peer)) = db.federation_peer_connection(reference.instance_id).await else {
        return FederatedEventNavigation::Unavailable {
            reason: FederatedNavigationUnavailableReason::MissingConnection,
        };
    };
    let Some((stable_path, source_path)) = locator else {
        return FederatedEventNavigation::Unavailable {
            reason: FederatedNavigationUnavailableReason::MissingLocator,
        };
    };
    match (
        peer.base_url.navigation_url(&stable_path),
        peer.base_url.navigation_url(&source_path),
    ) {
        (Ok(stable), Ok(source)) => FederatedEventNavigation::Available {
            stable_url: stable.as_url().to_string(),
            source_url: source.as_url().to_string(),
        },
        _ => FederatedEventNavigation::Unavailable {
            reason: FederatedNavigationUnavailableReason::InvalidLocator,
        },
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
