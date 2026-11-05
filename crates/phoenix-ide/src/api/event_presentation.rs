use std::collections::HashMap;

use phoenix_core::domain::{
    db_schema::InputOrigin,
    federation_subscription::{
        GapReason, SubscriptionCurrentState, SubscriptionDelivery, TerminalFactOccurrenceKind,
        TerminalFactOutcome,
    },
    instance_identity::{InstanceId, PeerBaseUrl},
};
use phoenix_db::{
    federation_subscriptions::{
        RemoteNotification, RemoteNotificationDisposition, RemoteSuppressionReason,
    },
    Database, DbError, Message, WatchEventPresentation, WatchEventProvenance, WatchOutcome,
};
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
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum FederatedGapReason {
    EventCount,
    PayloadBytes,
    DisconnectedAge,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum FederatedOccurrenceKind {
    DirectTurn,
    Steering,
    SeededFork,
    InteractionResponse,
    Creation,
    ContinuationSummary,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum FederatedSuppressionReason {
    AuthorizationDenied,
    TrustRevoked,
    SourceUnreachable,
    EvidenceExpired,
    SubscriptionEnded,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(tag = "state", rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum FederatedEventDisposition {
    Pending,
    Authorizing,
    Authorized { authorization_id: String },
    Suppressed { reason: FederatedSuppressionReason },
    Accepted { transcript_id: String },
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(tag = "status", rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum FederatedEventNavigation {
    Available {
        stable_url: String,
        source_url: String,
    },
    Unavailable,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct FederatedFactPresentation {
    pub event_id: String,
    pub source_instance_id: String,
    pub source_product_conversation_id: String,
    pub subscription_id: String,
    pub source_event_id: String,
    pub sequence: i64,
    pub source_occurrence_kind: FederatedOccurrenceKind,
    pub source_occurrence_id: String,
    pub source_generation: i64,
    pub source_transcript_id: String,
    pub outcome: FederatedEventOutcome,
    pub occurred_at_us: i64,
    pub disposition: FederatedEventDisposition,
    pub navigation: FederatedEventNavigation,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct FederatedGapPresentation {
    pub event_id: String,
    pub source_instance_id: String,
    pub source_product_conversation_id: String,
    pub subscription_id: String,
    pub last_acknowledged: i64,
    pub last_produced: i64,
    pub reason: FederatedGapReason,
    pub failed_at_us: i64,
    pub terminal_evidence_expires_at_us: i64,
    pub source_transcript_id: String,
    pub current: FederatedCurrentState,
    pub disposition: FederatedEventDisposition,
    pub navigation: FederatedEventNavigation,
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

fn federated_outcome(outcome: TerminalFactOutcome) -> FederatedEventOutcome {
    match outcome {
        TerminalFactOutcome::Completed => FederatedEventOutcome::Completed,
        TerminalFactOutcome::Failed => FederatedEventOutcome::Failed,
        TerminalFactOutcome::Cancelled => FederatedEventOutcome::Cancelled,
    }
}

fn federated_current(current: SubscriptionCurrentState) -> FederatedCurrentState {
    match current {
        SubscriptionCurrentState::Nonterminal => FederatedCurrentState::Nonterminal,
        SubscriptionCurrentState::Terminal { outcome } => FederatedCurrentState::Terminal {
            outcome: federated_outcome(outcome),
        },
    }
}

fn federated_gap_reason(reason: GapReason) -> FederatedGapReason {
    match reason {
        GapReason::EventCount => FederatedGapReason::EventCount,
        GapReason::PayloadBytes => FederatedGapReason::PayloadBytes,
        GapReason::DisconnectedAge => FederatedGapReason::DisconnectedAge,
    }
}

fn federated_occurrence(kind: TerminalFactOccurrenceKind) -> FederatedOccurrenceKind {
    match kind {
        TerminalFactOccurrenceKind::DirectTurn => FederatedOccurrenceKind::DirectTurn,
        TerminalFactOccurrenceKind::Steering => FederatedOccurrenceKind::Steering,
        TerminalFactOccurrenceKind::SeededFork => FederatedOccurrenceKind::SeededFork,
        TerminalFactOccurrenceKind::InteractionResponse => {
            FederatedOccurrenceKind::InteractionResponse
        }
        TerminalFactOccurrenceKind::Creation => FederatedOccurrenceKind::Creation,
        TerminalFactOccurrenceKind::ContinuationSummary => {
            FederatedOccurrenceKind::ContinuationSummary
        }
    }
}

fn federated_suppression(reason: RemoteSuppressionReason) -> FederatedSuppressionReason {
    match reason {
        RemoteSuppressionReason::AuthorizationDenied => {
            FederatedSuppressionReason::AuthorizationDenied
        }
        RemoteSuppressionReason::TrustRevoked => FederatedSuppressionReason::TrustRevoked,
        RemoteSuppressionReason::SourceUnreachable => FederatedSuppressionReason::SourceUnreachable,
        RemoteSuppressionReason::EvidenceExpired => FederatedSuppressionReason::EvidenceExpired,
        RemoteSuppressionReason::SubscriptionEnded => FederatedSuppressionReason::SubscriptionEnded,
    }
}

fn federated_disposition(disposition: RemoteNotificationDisposition) -> FederatedEventDisposition {
    match disposition {
        RemoteNotificationDisposition::Pending => FederatedEventDisposition::Pending,
        RemoteNotificationDisposition::Authorizing => FederatedEventDisposition::Authorizing,
        RemoteNotificationDisposition::Authorized { authorization_id } => {
            FederatedEventDisposition::Authorized {
                authorization_id: authorization_id.to_string(),
            }
        }
        RemoteNotificationDisposition::Suppressed { reason } => {
            FederatedEventDisposition::Suppressed {
                reason: federated_suppression(reason),
            }
        }
        RemoteNotificationDisposition::Accepted { transcript_id } => {
            FederatedEventDisposition::Accepted {
                transcript_id: transcript_id.to_string(),
            }
        }
    }
}

fn federated_navigation(
    paths: Option<&(String, String)>,
    origin: Option<&PeerBaseUrl>,
) -> FederatedEventNavigation {
    let (Some((stable_path, source_path)), Some(origin)) = (paths, origin) else {
        return FederatedEventNavigation::Unavailable;
    };
    let (Ok(stable_url), Ok(source_url)) = (
        origin.navigation_url(stable_path),
        origin.navigation_url(source_path),
    ) else {
        tracing::error!("stored federation navigation path failed peer-origin validation");
        return FederatedEventNavigation::Unavailable;
    };
    FederatedEventNavigation::Available {
        stable_url: stable_url.as_url().to_string(),
        source_url: source_url.as_url().to_string(),
    }
}

fn federated_presentation(
    notification: RemoteNotification,
    paths: Option<&(String, String)>,
    origin: Option<&PeerBaseUrl>,
) -> Result<EventPresentation, DbError> {
    let RemoteNotification {
        admission_id,
        reference,
        subscription_id,
        content,
        disposition,
    } = notification;
    let source_instance_id = reference.instance_id.to_string();
    let source_product_conversation_id = reference.product_conversation_id.to_string();
    let subscription_id = subscription_id.to_string();
    let disposition = federated_disposition(disposition);
    Ok(match content {
        SubscriptionDelivery::Fact { fact } => {
            let source_generation = i64::try_from(fact.occurrence.generation).map_err(|_| {
                DbError::Serialization("federated occurrence generation exceeds i64".into())
            })?;
            EventPresentation::FederatedFact(FederatedFactPresentation {
                event_id: admission_id,
                source_instance_id,
                source_product_conversation_id,
                subscription_id,
                source_event_id: fact.event_id.to_string(),
                sequence: fact.sequence.get(),
                source_occurrence_kind: federated_occurrence(fact.occurrence.kind),
                source_occurrence_id: fact.occurrence.id,
                source_generation,
                source_transcript_id: fact.source_transcript_id.to_string(),
                outcome: federated_outcome(fact.outcome),
                occurred_at_us: fact.occurred_at_us,
                disposition,
                navigation: federated_navigation(paths, origin),
            })
        }
        SubscriptionDelivery::Gap { gap } => {
            EventPresentation::FederatedGap(FederatedGapPresentation {
                event_id: admission_id,
                source_instance_id,
                source_product_conversation_id,
                subscription_id,
                last_acknowledged: gap.last_acknowledged.get(),
                last_produced: gap.last_produced.get(),
                reason: federated_gap_reason(gap.reason),
                failed_at_us: gap.failed_at_us,
                terminal_evidence_expires_at_us: gap.terminal_evidence_expires_at_us,
                source_transcript_id: gap.snapshot.transcript_id.to_string(),
                current: federated_current(gap.snapshot.state),
                disposition,
                navigation: FederatedEventNavigation::Unavailable,
            })
        }
    })
}

async fn load_federated_presentations(
    db: &Database,
    ids: &[String],
) -> Result<HashMap<String, EventPresentation>, DbError> {
    let notifications = db.remote_notification_presentations(ids).await?;
    let mut origins: HashMap<InstanceId, Option<PeerBaseUrl>> = HashMap::new();
    for (notification, paths) in notifications.values() {
        if paths.is_some()
            && matches!(notification.content, SubscriptionDelivery::Fact { .. })
            && !origins.contains_key(&notification.reference.instance_id)
        {
            let peer = db
                .federation_peer_connection(notification.reference.instance_id)
                .await?;
            origins.insert(
                notification.reference.instance_id,
                peer.map(|connection| connection.base_url),
            );
        }
    }
    notifications
        .into_iter()
        .map(|(id, (notification, paths))| {
            let origin = origins
                .get(&notification.reference.instance_id)
                .and_then(Option::as_ref);
            federated_presentation(notification, paths.as_ref(), origin)
                .map(|presentation| (id, presentation))
        })
        .collect()
}

pub(crate) struct EventPresentations {
    events: HashMap<String, EventPresentation>,
    local_fallback: EventPresentationUnavailableReason,
    federated_fallback: EventPresentationUnavailableReason,
}

impl EventPresentations {
    pub(crate) fn redacted() -> Self {
        Self {
            events: HashMap::new(),
            local_fallback: EventPresentationUnavailableReason::Redacted,
            federated_fallback: EventPresentationUnavailableReason::Redacted,
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
                | InputOrigin::SlackGateway { .. }
                | InputOrigin::RemoteApi { .. }
                | InputOrigin::RemoteConversation { .. }
                | InputOrigin::RemoteCoordinator { .. }
                | InputOrigin::ScheduledReminder { .. }
                | InputOrigin::SystemGenerated => None,
            })
            .collect();
        if ids.is_empty() {
            return Self {
                events: HashMap::new(),
                local_fallback: EventPresentationUnavailableReason::Missing,
                federated_fallback: EventPresentationUnavailableReason::Missing,
            };
        }

        let (mut events, local_fallback) = match db
            .coordinator_watch_event_presentations(&ids)
            .await
        {
            Ok(local) => (
                local
                    .into_iter()
                    .map(|(id, event)| (id, EventPresentation::Available(event.into())))
                    .collect(),
                EventPresentationUnavailableReason::Missing,
            ),
            Err(error) => {
                tracing::warn!(%error, "local event presentation lookup failed; preserving original messages");
                (
                    HashMap::new(),
                    EventPresentationUnavailableReason::LookupFailed,
                )
            }
        };

        let federated_fallback = match load_federated_presentations(db, &ids).await {
            Ok(federated) => {
                for (id, presentation) in federated {
                    if events.insert(id.clone(), presentation).is_some() {
                        tracing::error!(%id, "local and federated event presentation identities collided");
                        events.insert(
                            id.clone(),
                            EventPresentation::Unavailable {
                                event_id: id,
                                reason: EventPresentationUnavailableReason::LookupFailed,
                            },
                        );
                    }
                }
                EventPresentationUnavailableReason::Missing
            }
            Err(error) => {
                tracing::warn!(%error, "federated event presentation lookup failed; preserving original messages");
                EventPresentationUnavailableReason::LookupFailed
            }
        };

        Self {
            events,
            local_fallback,
            federated_fallback,
        }
    }

    pub(crate) fn for_message(&self, message: &Message) -> Option<EventPresentation> {
        match &message.origin {
            InputOrigin::SubscriptionEvent { event_id } => {
                Some(self.events.get(event_id).cloned().unwrap_or_else(|| {
                    EventPresentation::Unavailable {
                        event_id: event_id.clone(),
                        reason: if event_id.starts_with("federation:") {
                            self.federated_fallback
                        } else {
                            self.local_fallback
                        },
                    }
                }))
            }
            InputOrigin::UnknownHistorical
            | InputOrigin::UserApi
            | InputOrigin::InternalConversation { .. }
            | InputOrigin::SlackGateway { .. }
            | InputOrigin::RemoteApi { .. }
            | InputOrigin::RemoteConversation { .. }
            | InputOrigin::RemoteCoordinator { .. }
            | InputOrigin::ScheduledReminder { .. }
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
