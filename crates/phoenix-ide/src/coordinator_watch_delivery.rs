use std::{fmt::Write as _, sync::Arc};

use phoenix_core::domain::db_schema::InputOrigin;
use phoenix_db::{
    CloseFailureStop, MandatoryCloseFailureSubject, PendingWatchEvent, WatchEventRoute,
};

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
        run_ordinal,
        remaining_resources,
        subject,
        detail,
        stop,
    } = &event.route
    {
        let _ = write!(text, " Cleanup run ordinal: {}.", run_ordinal.get());
        match subject {
            MandatoryCloseFailureSubject::Attempt => {
                text.push_str(
                    " Failure subject: Close attempt interruption with no captured resource.",
                );
            }
            MandatoryCloseFailureSubject::Resource {
                scope,
                resource_kind,
                identity_kind,
                identity_codec,
                identity_value,
            } => {
                let _ = write!(
                    text,
                    " Failed resource — Scope: {scope}. Resource kind: {resource_kind}. Identity ({identity_kind}, {identity_codec}): {identity_value}."
                );
            }
        }
        let _ = write!(text, " Detail: {detail}.");
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
        text.push_str("\nRemaining resources:");
        for resource in remaining_resources {
            let _ = write!(
                text,
                "\n- Scope: {}. Resource kind: {}. Identity ({}, {}): {}. Disposition: {}.",
                resource.scope.as_str(),
                resource.resource_kind,
                resource.identity_kind,
                resource.identity_codec,
                resource.identity_value,
                resource.disposition.as_str(),
            );
        }
        text.push_str("\nRead-only investigation is allowed. Delivery acceptance is not cleanup success or repair approval.");
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
                run_ordinal: phoenix_core::domain::close::CloseRunOrdinal::parse(2).unwrap(),
                remaining_resources: Vec::new(),
                subject: MandatoryCloseFailureSubject::Resource {
                    scope: "scope-1".into(),
                    resource_kind: "worktree".into(),
                    identity_kind: "path".into(),
                    identity_codec: "utf8".into(),
                    identity_value: "/preserved/worktree".into(),
                },
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
            "Cleanup run ordinal: 2",
            "Failed resource",
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
    fn attempt_interruption_message_does_not_invent_a_resource() {
        let mut event = failure(CloseFailureStop::ShutdownUncertain);
        let WatchEventRoute::MandatoryCloseFailure { subject, .. } = &mut event.route else {
            unreachable!();
        };
        *subject = MandatoryCloseFailureSubject::Attempt;
        let text = notification(&event);
        assert!(text.contains("Close attempt interruption with no captured resource"));
        assert!(!text.contains("Failed resource"));
    }

    #[test]
    fn mandatory_message_renders_every_remaining_resource_and_disposition() {
        use phoenix_core::{
            domain::close::{
                LossItemIdentity, OpaqueIdentity, RetiredResourceIdentity, RetiredResourceKind,
            },
            work_scope::WorkScopeId,
        };
        use phoenix_db::{
            CloseCleanupFailureResource, CloseCleanupResourceDisposition as Disposition,
        };

        let mut event = failure(CloseFailureStop::ShutdownUncertain);
        let WatchEventRoute::MandatoryCloseFailure {
            remaining_resources,
            ..
        } = &mut event.route
        else {
            unreachable!();
        };
        let targets = [
            (
                "scope-1",
                RetiredResourceKind::BashProcessGroup,
                "epoch:failed",
                Disposition::Failed,
            ),
            (
                "scope-2",
                RetiredResourceKind::BrowserSession,
                "epoch:residual",
                Disposition::Residual,
            ),
            (
                "scope-1",
                RetiredResourceKind::PtySession,
                "epoch:unattempted",
                Disposition::Unattempted,
            ),
            (
                "scope-2",
                RetiredResourceKind::EquivalentLiveResource,
                "epoch:unknown",
                Disposition::Unknown,
            ),
        ];
        *remaining_resources = targets
            .iter()
            .map(|(scope, kind, identity, disposition)| {
                CloseCleanupFailureResource {
                    scope: WorkScopeId::parse(*scope).unwrap(),
                    resource: RetiredResourceIdentity::parse(
                        *kind,
                        LossItemIdentity::Opaque(OpaqueIdentity::parse(*identity).unwrap()),
                    )
                    .unwrap(),
                    disposition: *disposition,
                }
                .into()
            })
            .collect();
        let text = notification(&event);
        let lines = text
            .lines()
            .filter(|line| line.starts_with("- Scope:"))
            .collect::<Vec<_>>();
        assert_eq!(lines.len(), targets.len());
        for (line, (scope, kind, identity, disposition)) in lines.into_iter().zip(targets) {
            assert_eq!(line, format!("- Scope: {scope}. Resource kind: {}. Identity (opaque, opaque_string_v1): {identity}. Disposition: {}.", kind.as_str(), disposition.as_str()));
        }
        assert_eq!(text.matches("Mandatory Close cleanup failure").count(), 1);
        assert!(text.contains("Cleanup run ordinal: 2"));
        assert!(text.contains("Failed resource"));
        assert!(text.contains("Shutdown uncertain. Close incomplete."));
        assert!(text.contains("Read-only investigation is allowed"));
        assert!(text.contains("Delivery acceptance is not cleanup success or repair approval"));
    }

    #[test]
    fn subscription_message_has_no_cleanup_evidence() {
        let mut event = failure(CloseFailureStop::ShutdownUncertain);
        event.route = WatchEventRoute::Subscription;
        let text = notification(&event);
        assert!(text.starts_with("Watched conversation terminal event."));
        assert!(!text.contains("Cleanup run ordinal"));
        assert!(!text.contains("Remaining resources"));
        assert!(!text.contains("Shutdown uncertain"));
    }

    #[test]
    fn uncertain_shutdown_never_claims_confirmed_stop() {
        let text = notification(&failure(CloseFailureStop::ShutdownUncertain));
        assert!(text.contains("Shutdown uncertain. Close incomplete."));
        assert!(!text.contains("Conversation and processes stopped"));
        assert!(!text.contains("confirmed at"));
    }
}
