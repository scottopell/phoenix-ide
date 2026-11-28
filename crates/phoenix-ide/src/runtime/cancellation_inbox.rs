use super::{
    AcknowledgedEventOutcome, AcknowledgedEventRequest, AcknowledgedRuntimeRequest, Event,
};
use phoenix_core::domain::sm_event::CancelCause;
use tokio::sync::{mpsc, oneshot};

pub(crate) struct UserCancellation {
    pub(crate) reason: Option<String>,
    pub(crate) cause: CancelCause,
}

impl UserCancellation {
    pub(crate) fn into_event(self) -> Event {
        Event::UserCancel {
            reason: self.reason,
            cause: self.cause,
        }
    }
}

pub(crate) struct DirectTurnCancellation {
    pub(crate) turn_id: phoenix_workflow::TurnAuthorityId,
    pub(crate) generation: u64,
}

pub(crate) struct NativeCloseSettlementStopOffer {
    db: phoenix_db::Database,
    command: phoenix_db::native_close::NativeCloseCommand,
    receipt: phoenix_db::native_close::NativeCloseReceipt,
    source_entry: super::executor::SourceEntryWorker,
    live_source: phoenix_core::domain::sm_state::ConvState,
    target: phoenix_db::CloseDirectTurnSettlementTarget,
}

impl NativeCloseSettlementStopOffer {
    pub(crate) fn new(
        db: phoenix_db::Database,
        command: phoenix_db::native_close::NativeCloseCommand,
        receipt: phoenix_db::native_close::NativeCloseReceipt,
        source_entry: super::executor::SourceEntryWorker,
        live_source: phoenix_core::domain::sm_state::ConvState,
        target: phoenix_db::CloseDirectTurnSettlementTarget,
    ) -> Self {
        Self {
            db,
            command,
            receipt,
            source_entry,
            live_source,
            target,
        }
    }

    pub(crate) async fn claim(
        self,
        fatal: std::sync::Arc<super::FatalLocalAuthorityFence>,
    ) -> bool {
        use super::executor::WorkerEntryResult;
        use phoenix_db::workflow::LocalAuthorityResult;

        let runtime = tokio::runtime::Handle::current();
        let worker_fatal = fatal.clone();
        let work = tokio::task::spawn_blocking(move || {
            let (result, lease) =
                runtime.block_on(self.db.claim_native_close_initial_settlement_stop(
                    &self.command,
                    &self.receipt,
                    &self.live_source,
                    &self.target,
                    || match self.source_entry.try_acquire_first_effect(&self.command) {
                        WorkerEntryResult::Ran(lease) => Ok(lease),
                        WorkerEntryResult::Busy | WorkerEntryResult::Refused => {
                            Err(phoenix_db::DbError::CloseFoundationPrecondition(
                                "Close settlement source entry is unavailable".into(),
                            ))
                        }
                    },
                ));
            match result {
                LocalAuthorityResult::DurableFactEstablished(result) => {
                    drop(lease);
                    result.unwrap_or_else(|error| {
                        tracing::debug!(%error, "native Close settlement Stop claim refused");
                        false
                    })
                }
                LocalAuthorityResult::DurableFactUnclassified => {
                    worker_fatal.close("native_close_settlement_stop_unknown_writer_release");
                    if let Some(lease) = lease {
                        std::mem::forget(self.source_entry.clone());
                        std::mem::forget(lease);
                    }
                    false
                }
            }
        });
        match work.await {
            Ok(claimed) => claimed,
            Err(error) => {
                tracing::error!(%error, "native Close settlement Stop claim worker failed");
                fatal.close("native_close_settlement_stop_worker_failure");
                false
            }
        }
    }
}

pub(crate) enum CloseSettlementStopOffer {
    Native(Box<NativeCloseSettlementStopOffer>),
    Generic(phoenix_db::Database, phoenix_db::GenericCloseTurnOffer),
}

impl CloseSettlementStopOffer {
    fn target(&self) -> &phoenix_db::CloseDirectTurnSettlementTarget {
        match self {
            Self::Native(offer) => &offer.target,
            Self::Generic(_, offer) => offer.target(),
        }
    }

    pub(crate) async fn claim(
        self,
        fatal: std::sync::Arc<super::FatalLocalAuthorityFence>,
    ) -> bool {
        use phoenix_db::workflow::LocalAuthorityResult::{
            DurableFactEstablished, DurableFactUnclassified,
        };
        if fatal.is_closed() {
            return false;
        }
        match self {
            Self::Native(offer) => offer.claim(fatal).await,
            Self::Generic(db, offer) => match db.claim_generic_close_turn_offer(offer).await {
                DurableFactEstablished(Ok(accepted)) => accepted,
                DurableFactEstablished(Err(error)) => {
                    tracing::debug!(%error, "generic Close settlement Stop refused");
                    false
                }
                DurableFactUnclassified => {
                    fatal.close("generic_close_settlement_stop_unknown_writer_release");
                    false
                }
            },
        }
    }
}

pub(crate) struct ScopedDirectTurnCancellation {
    pub(crate) scope: DirectTurnCancellation,
    pub(crate) offer: CloseSettlementStopOffer,
    pub(crate) cancellation: UserCancellation,
    pub(crate) applied: oneshot::Sender<ScopedStopHandling>,
}

pub(crate) enum CancellationInput {
    Addressed(UserCancellation),
    DirectTurn(ScopedDirectTurnCancellation),
    Acknowledged {
        cancellation: UserCancellation,
        acknowledgement: oneshot::Sender<Result<AcknowledgedEventOutcome, String>>,
        retirement: oneshot::Receiver<()>,
    },
}

/// A mailbox response describes handling, not release of durable turn ownership.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScopedStopHandling {
    Handled,
    Refused,
}

pub(crate) enum CancellationReceipt {
    Addressed,
    DirectTurn(oneshot::Sender<ScopedStopHandling>),
    Acknowledged {
        acknowledgement: oneshot::Sender<Result<AcknowledgedEventOutcome, String>>,
        retirement: oneshot::Receiver<()>,
    },
}

impl CancellationInput {
    pub(crate) fn into_parts(self) -> (Event, CancellationReceipt, Option<DirectTurnCancellation>) {
        match self {
            Self::Addressed(cancellation) => (
                cancellation.into_event(),
                CancellationReceipt::Addressed,
                None,
            ),
            Self::DirectTurn(scoped) => (
                scoped.cancellation.into_event(),
                CancellationReceipt::DirectTurn(scoped.applied),
                Some(scoped.scope),
            ),
            Self::Acknowledged {
                cancellation,
                acknowledgement,
                retirement,
            } => (
                cancellation.into_event(),
                CancellationReceipt::Acknowledged {
                    acknowledgement,
                    retirement,
                },
                None,
            ),
        }
    }
}

pub(crate) struct DirectTurnCancellationReservation(mpsc::OwnedPermit<CancellationInput>);

impl DirectTurnCancellationReservation {
    pub(crate) fn send(
        self,
        offer: CloseSettlementStopOffer,
    ) -> oneshot::Receiver<ScopedStopHandling> {
        let (applied, receipt) = oneshot::channel();
        let scope = DirectTurnCancellation {
            turn_id: phoenix_workflow::TurnAuthorityId(offer.target().turn_id),
            generation: offer.target().expected_generation,
        };
        self.0.send(CancellationInput::DirectTurn(
            ScopedDirectTurnCancellation {
                scope,
                offer,
                cancellation: UserCancellation {
                    reason: Some("active Close settlement".into()),
                    cause: CancelCause::UserRequested,
                },
                applied,
            },
        ));
        receipt
    }
}

#[derive(Clone)]
pub struct ConversationEventSender(EventDestination);

#[derive(Clone)]
enum EventDestination {
    Runtime {
        ordinary: mpsc::Sender<Event>,
        cancellation: mpsc::Sender<CancellationInput>,
    },
    #[cfg(test)]
    TestReceiver(mpsc::Sender<Event>),
}

#[derive(Clone)]
pub(crate) struct AcknowledgedEventSender(AcknowledgedDestination);

#[derive(Clone)]
enum AcknowledgedDestination {
    Runtime {
        ordinary: mpsc::Sender<AcknowledgedRuntimeRequest>,
        cancellation: mpsc::Sender<CancellationInput>,
    },
    #[cfg(test)]
    TestReceiver(mpsc::Sender<AcknowledgedRuntimeRequest>),
}

pub(crate) fn channel(
    events: mpsc::Sender<Event>,
    acknowledgements: mpsc::Sender<AcknowledgedRuntimeRequest>,
) -> (
    ConversationEventSender,
    AcknowledgedEventSender,
    mpsc::Receiver<CancellationInput>,
) {
    let (cancellation, receiver) = mpsc::channel(1);
    (
        ConversationEventSender(EventDestination::Runtime {
            ordinary: events,
            cancellation: cancellation.clone(),
        }),
        AcknowledgedEventSender(AcknowledgedDestination::Runtime {
            ordinary: acknowledgements,
            cancellation,
        }),
        receiver,
    )
}

impl ConversationEventSender {
    pub fn try_send(&self, event: Event) -> Result<(), Box<mpsc::error::TrySendError<Event>>> {
        let result = match &self.0 {
            EventDestination::Runtime {
                ordinary,
                cancellation,
            } => match event {
                Event::UserCancel { reason, cause } => cancellation
                    .try_send(CancellationInput::Addressed(UserCancellation {
                        reason,
                        cause,
                    }))
                    .map_err(|error| match error {
                        mpsc::error::TrySendError::Full(CancellationInput::Addressed(input)) => {
                            mpsc::error::TrySendError::Full(input.into_event())
                        }
                        mpsc::error::TrySendError::Closed(CancellationInput::Addressed(input)) => {
                            mpsc::error::TrySendError::Closed(input.into_event())
                        }
                        _ => unreachable!("try_send returns the same owned input"),
                    }),
                event => ordinary.try_send(event),
            },
            #[cfg(test)]
            EventDestination::TestReceiver(sender) => sender.try_send(event),
        };
        result.map_err(Box::new)
    }

    pub async fn send(&self, event: Event) -> Result<(), mpsc::error::SendError<Event>> {
        match &self.0 {
            EventDestination::Runtime {
                ordinary,
                cancellation,
            } => match event {
                Event::UserCancel { reason, cause } => cancellation
                    .send(CancellationInput::Addressed(UserCancellation {
                        reason,
                        cause,
                    }))
                    .await
                    .map_err(|mpsc::error::SendError(input)| {
                        let CancellationInput::Addressed(cancellation) = input else {
                            unreachable!("send returns the same owned input")
                        };
                        mpsc::error::SendError(cancellation.into_event())
                    }),
                event => ordinary.send(event).await,
            },
            #[cfg(test)]
            EventDestination::TestReceiver(sender) => sender.send(event).await,
        }
    }

    pub(crate) fn try_reserve_direct_turn_cancellation(
        &self,
    ) -> Result<DirectTurnCancellationReservation, mpsc::error::TrySendError<()>> {
        match &self.0 {
            EventDestination::Runtime { cancellation, .. } => cancellation
                .clone()
                .try_reserve_owned()
                .map(DirectTurnCancellationReservation)
                .map_err(|error| match error {
                    mpsc::error::TrySendError::Full(_) => mpsc::error::TrySendError::Full(()),
                    mpsc::error::TrySendError::Closed(_) => mpsc::error::TrySendError::Closed(()),
                }),
            #[cfg(test)]
            EventDestination::TestReceiver(_) => Err(mpsc::error::TrySendError::Closed(())),
        }
    }

    pub(crate) fn is_cancellation_closed(&self) -> bool {
        match &self.0 {
            EventDestination::Runtime { cancellation, .. } => cancellation.is_closed(),
            #[cfg(test)]
            EventDestination::TestReceiver(sender) => sender.is_closed(),
        }
    }

    pub(crate) fn is_closed(&self) -> bool {
        match &self.0 {
            EventDestination::Runtime { ordinary, .. } => ordinary.is_closed(),
            #[cfg(test)]
            EventDestination::TestReceiver(ordinary) => ordinary.is_closed(),
        }
    }

    #[cfg(test)]
    pub(crate) async fn closed(&self) {
        match &self.0 {
            EventDestination::Runtime { ordinary, .. } => ordinary.closed().await,
            EventDestination::TestReceiver(sender) => sender.closed().await,
        }
    }

    #[cfg(test)]
    pub(crate) fn same_channel(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (
                EventDestination::Runtime {
                    ordinary,
                    cancellation,
                },
                EventDestination::Runtime {
                    ordinary: other,
                    cancellation: other_cancellation,
                },
            ) => ordinary.same_channel(other) && cancellation.same_channel(other_cancellation),
            (EventDestination::TestReceiver(sender), EventDestination::TestReceiver(other)) => {
                sender.same_channel(other)
            }
            _ => false,
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test_receiver(sender: mpsc::Sender<Event>) -> Self {
        Self(EventDestination::TestReceiver(sender))
    }
}

impl AcknowledgedEventSender {
    pub(crate) async fn send(
        &self,
        request: impl Into<AcknowledgedRuntimeRequest>,
    ) -> Result<(), mpsc::error::SendError<AcknowledgedRuntimeRequest>> {
        let request = request.into();
        match &self.0 {
            AcknowledgedDestination::Runtime {
                ordinary,
                cancellation,
            } => {
                let AcknowledgedRuntimeRequest::Event(request) = request else {
                    return ordinary.send(request).await;
                };
                let Event::UserCancel { reason, cause } = request.event else {
                    return ordinary
                        .send(AcknowledgedRuntimeRequest::Event(request))
                        .await;
                };
                cancellation
                    .send(CancellationInput::Acknowledged {
                        cancellation: UserCancellation { reason, cause },
                        acknowledgement: request.acknowledgement,
                        retirement: request.retirement,
                    })
                    .await
                    .map_err(|mpsc::error::SendError(input)| {
                        let CancellationInput::Acknowledged {
                            cancellation,
                            acknowledgement,
                            retirement,
                        } = input
                        else {
                            unreachable!("send returns the same owned request")
                        };
                        mpsc::error::SendError(AcknowledgedRuntimeRequest::from(
                            AcknowledgedEventRequest {
                                event: cancellation.into_event(),
                                acknowledgement,
                                retirement,
                            },
                        ))
                    })
            }
            #[cfg(test)]
            AcknowledgedDestination::TestReceiver(sender) => sender.send(request).await,
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test_receiver(sender: mpsc::Sender<AcknowledgedRuntimeRequest>) -> Self {
        Self(AcknowledgedDestination::TestReceiver(sender))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::native_close_application::{
        NativeCloseApplicationError, NativeCloseInitialRequest, NativeCloseObserveRequest,
        NativeCloseRetryRequest,
    };
    use crate::runtime::scope_liveness_tests::test_manager;
    use phoenix_core::domain::close::{CloseAttemptId, CloseRunOrdinal};
    use phoenix_core::domain::db_schema::SourceToolCall;
    use phoenix_core::domain::product_conversation::ProductConversationId;
    use phoenix_db::native_close::{
        NativeCloseInitialResult, NativeCloseReceipt, NativeCloseRunRef,
    };
    use std::sync::Arc;
    use tokio::sync::Mutex;

    struct EventReceiptPeer {
        outcome: oneshot::Receiver<Result<AcknowledgedEventOutcome, String>>,
        retired: oneshot::Sender<()>,
    }

    impl EventReceiptPeer {
        fn request(event: Event) -> (AcknowledgedEventRequest, Self) {
            let (acknowledgement, outcome) = oneshot::channel();
            let (retired, retirement) = oneshot::channel();
            (
                AcknowledgedEventRequest {
                    event,
                    acknowledgement,
                    retirement,
                },
                Self { outcome, retired },
            )
        }

        fn assert_round_trip(
            mut self,
            acknowledgement: oneshot::Sender<Result<AcknowledgedEventOutcome, String>>,
            mut retirement: oneshot::Receiver<()>,
        ) {
            let outcome = AcknowledgedEventOutcome::StaleAuthority;
            acknowledgement.send(Ok(outcome)).unwrap();
            assert_eq!(self.outcome.try_recv().unwrap(), Ok(outcome));
            self.retired.send(()).unwrap();
            retirement.try_recv().unwrap();
        }
    }

    #[tokio::test]
    async fn native_direct_turn_cancellation_reservation_does_not_enqueue() {
        let (events, _events_rx) = mpsc::channel(1);
        let (ordinary, _ordinary_rx) = mpsc::channel(1);
        let (sender, _, mut cancellation_rx) = channel(events, ordinary);
        let reservation = sender.try_reserve_direct_turn_cancellation().unwrap();
        assert!(matches!(
            cancellation_rx.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
        drop(reservation);
        let _reservation = sender.try_reserve_direct_turn_cancellation().unwrap();
        assert!(matches!(
            cancellation_rx.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
    }

    #[tokio::test]
    async fn acknowledged_stop_routes_to_cancellation_with_original_receipt() {
        let (events, _events_rx) = mpsc::channel(1);
        let (ordinary, mut ordinary_rx) = mpsc::channel(1);
        let (_events, sender, mut cancellation_rx) = channel(events, ordinary);
        let (request, peer) = EventReceiptPeer::request(Event::UserCancel {
            reason: Some("stop".into()),
            cause: CancelCause::UserRequested,
        });

        sender
            .send(AcknowledgedRuntimeRequest::from(request))
            .await
            .unwrap();

        assert!(matches!(
            ordinary_rx.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
        let (event, receipt, scope) = cancellation_rx.try_recv().unwrap().into_parts();
        assert!(scope.is_none());
        assert!(matches!(
            event,
            Event::UserCancel { reason: Some(reason), cause: CancelCause::UserRequested }
                if reason == "stop"
        ));
        let CancellationReceipt::Acknowledged {
            acknowledgement,
            retirement,
        } = receipt
        else {
            panic!("acknowledged Stop must retain its receipt");
        };
        peer.assert_round_trip(acknowledgement, retirement);
    }

    #[tokio::test]
    async fn ordinary_acknowledged_event_keeps_its_envelope() {
        let (events, _events_rx) = mpsc::channel(1);
        let (ordinary, mut ordinary_rx) = mpsc::channel(1);
        let (_events, sender, mut cancellation_rx) = channel(events, ordinary);
        let (request, peer) = EventReceiptPeer::request(Event::DismissError);
        let request = Box::new(request);
        let original = std::ptr::from_ref(request.as_ref());

        sender
            .send(AcknowledgedRuntimeRequest::Event(request))
            .await
            .unwrap();

        assert!(matches!(
            cancellation_rx.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
        let AcknowledgedRuntimeRequest::Event(request) = ordinary_rx.try_recv().unwrap() else {
            panic!("ordinary event must reach the ordinary acknowledged inbox");
        };
        assert!(std::ptr::eq(request.as_ref(), original));
        assert!(matches!(request.event, Event::DismissError));
        peer.assert_round_trip(request.acknowledgement, request.retirement);
    }

    #[tokio::test]
    async fn closed_cancellation_inbox_returns_stop_envelope_with_original_receipt() {
        let (events, _events_rx) = mpsc::channel(1);
        let (ordinary, mut ordinary_rx) = mpsc::channel(1);
        let (events, sender, mut cancellation_rx) = channel(events, ordinary);
        cancellation_rx.close();
        assert!(!events.is_closed());
        assert!(events.is_cancellation_closed());
        let (request, peer) = EventReceiptPeer::request(Event::UserCancel {
            reason: Some("close race".into()),
            cause: CancelCause::Timeout,
        });

        let AcknowledgedRuntimeRequest::Event(request) = sender.send(request).await.unwrap_err().0
        else {
            panic!("send failure must return the Stop envelope");
        };
        assert!(matches!(
            ordinary_rx.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
        assert!(matches!(
            request.event,
            Event::UserCancel { reason: Some(reason), cause: CancelCause::Timeout }
                if reason == "close race"
        ));
        peer.assert_round_trip(request.acknowledgement, request.retirement);
    }

    #[tokio::test]
    async fn test_receiver_accepts_raw_acknowledged_stop_without_splitting_it() {
        let (sender, mut receiver) = mpsc::channel(1);
        let sender = AcknowledgedEventSender::for_test_receiver(sender);
        let (request, peer) = EventReceiptPeer::request(Event::UserCancel {
            reason: None,
            cause: CancelCause::UserRequested,
        });

        sender.send(request).await.unwrap();

        let AcknowledgedRuntimeRequest::Event(request) = receiver.try_recv().unwrap() else {
            panic!("test receiver must still see the submitted Stop event");
        };
        assert!(matches!(
            request.event,
            Event::UserCancel {
                reason: None,
                cause: CancelCause::UserRequested
            }
        ));
        peer.assert_round_trip(request.acknowledgement, request.retirement);
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)] // Keep all three envelope/reply ownership cases together.
    async fn native_close_requests_keep_their_envelopes_and_replies() {
        let backend = Arc::new(test_manager().await);
        let source = SourceToolCall {
            message_id: "close-message".into(),
            tool_use_id: "close-tool".into(),
        };
        let referenced = NativeCloseRunRef {
            target: ProductConversationId::parse("close-target").unwrap(),
            attempt_id: CloseAttemptId::parse("close-attempt").unwrap(),
            run_ordinal: CloseRunOrdinal::INITIAL,
        };
        let (events, _events_rx) = mpsc::channel(1);
        let (ordinary, mut ordinary_rx) = mpsc::channel(1);
        let (_events, sender, mut cancellation_rx) = channel(events, ordinary);

        let (response, mut initial_reply) = oneshot::channel();
        let initial = Box::new(NativeCloseInitialRequest {
            source: source.clone(),
            backend: Arc::clone(&backend),
            target: referenced.target.clone(),
            admitted: backend.acquire_local_authority_pass().unwrap(),
            target_gate: Arc::new(Mutex::new(())).try_lock_owned().unwrap(),
            response,
        });
        let original = std::ptr::from_ref(initial.as_ref());
        sender
            .send(AcknowledgedRuntimeRequest::ExecuteNativeCloseInitial(
                initial,
            ))
            .await
            .unwrap();
        let AcknowledgedRuntimeRequest::ExecuteNativeCloseInitial(initial) =
            ordinary_rx.try_recv().unwrap()
        else {
            panic!("Close Initial must reach the ordinary acknowledged inbox");
        };
        assert!(std::ptr::eq(initial.as_ref(), original));
        let receipt = NativeCloseInitialResult::Replay(NativeCloseReceipt {
            request_id: uuid::Uuid::nil(),
            admitted: referenced.clone(),
        });
        initial.response.send(Ok(receipt.clone())).unwrap();
        assert_eq!(initial_reply.try_recv().unwrap(), Ok(receipt));

        let (response, mut observe_reply) = oneshot::channel();
        let observe = Box::new(NativeCloseObserveRequest {
            source: source.clone(),
            referenced: referenced.clone(),
            admitted: backend.acquire_local_authority_pass().unwrap(),
            target_gate: Arc::new(Mutex::new(())).try_lock_owned().unwrap(),
            response,
        });
        let original = std::ptr::from_ref(observe.as_ref());
        sender
            .send(AcknowledgedRuntimeRequest::ExecuteNativeCloseObserve(
                observe,
            ))
            .await
            .unwrap();
        let AcknowledgedRuntimeRequest::ExecuteNativeCloseObserve(observe) =
            ordinary_rx.try_recv().unwrap()
        else {
            panic!("Close Observe must reach the ordinary acknowledged inbox");
        };
        assert!(std::ptr::eq(observe.as_ref(), original));
        observe
            .response
            .send(Err(NativeCloseApplicationError::SourceRefused))
            .unwrap();
        assert!(matches!(
            observe_reply.try_recv(),
            Ok(Err(NativeCloseApplicationError::SourceRefused))
        ));

        let (response, mut retry_reply) = oneshot::channel();
        let retry = Box::new(NativeCloseRetryRequest {
            source,
            target: referenced.target,
            attempt_id: referenced.attempt_id,
            failed_run_ordinal: referenced.run_ordinal,
            backend: Arc::clone(&backend),
            admitted: backend.acquire_local_authority_pass().unwrap(),
            target_gate: Arc::new(Mutex::new(())).try_lock_owned().unwrap(),
            attempt_gate: Arc::new(Mutex::new(())).try_lock_owned().unwrap(),
            response,
        });
        let original = std::ptr::from_ref(retry.as_ref());
        sender
            .send(AcknowledgedRuntimeRequest::ExecuteNativeCloseRetry(retry))
            .await
            .unwrap();
        let AcknowledgedRuntimeRequest::ExecuteNativeCloseRetry(retry) =
            ordinary_rx.try_recv().unwrap()
        else {
            panic!("Close Retry must reach the ordinary acknowledged inbox");
        };
        assert!(std::ptr::eq(retry.as_ref(), original));

        ordinary_rx.close();
        let AcknowledgedRuntimeRequest::ExecuteNativeCloseRetry(retry) = sender
            .send(AcknowledgedRuntimeRequest::ExecuteNativeCloseRetry(retry))
            .await
            .unwrap_err()
            .0
        else {
            panic!("send failure must return the Close Retry envelope");
        };
        assert!(std::ptr::eq(retry.as_ref(), original));
        retry
            .response
            .send(Err(NativeCloseApplicationError::SourceRefused))
            .unwrap();
        assert!(matches!(
            retry_reply.try_recv(),
            Ok(Err(NativeCloseApplicationError::SourceRefused))
        ));
        assert!(matches!(
            cancellation_rx.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
    }
}
