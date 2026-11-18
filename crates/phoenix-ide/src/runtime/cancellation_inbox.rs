use super::{AcknowledgedEventOutcome, AcknowledgedEventRequest, Event};
use phoenix_core::domain::sm_event::CancelCause;
use tokio::sync::{mpsc, oneshot};

pub(crate) struct UserCancellation {
    pub(crate) reason: Option<String>,
    pub(crate) cause: CancelCause,
}

impl UserCancellation {
    fn into_event(self) -> Event {
        Event::UserCancel {
            reason: self.reason,
            cause: self.cause,
        }
    }
}

pub(crate) enum CancellationInput {
    Addressed(UserCancellation),
    Acknowledged {
        cancellation: UserCancellation,
        acknowledgement: oneshot::Sender<Result<AcknowledgedEventOutcome, String>>,
        retirement: oneshot::Receiver<()>,
    },
}

pub(crate) enum CancellationReceipt {
    Addressed,
    Acknowledged {
        acknowledgement: oneshot::Sender<Result<AcknowledgedEventOutcome, String>>,
        retirement: oneshot::Receiver<()>,
    },
}

impl CancellationInput {
    pub(crate) fn into_parts(self) -> (Event, CancellationReceipt) {
        match self {
            Self::Addressed(cancellation) => {
                (cancellation.into_event(), CancellationReceipt::Addressed)
            }
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
            ),
        }
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
        ordinary: mpsc::Sender<AcknowledgedEventRequest>,
        cancellation: mpsc::Sender<CancellationInput>,
    },
    #[cfg(test)]
    TestReceiver(mpsc::Sender<AcknowledgedEventRequest>),
}

pub(crate) fn channel(
    events: mpsc::Sender<Event>,
    acknowledgements: mpsc::Sender<AcknowledgedEventRequest>,
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

    #[cfg(test)]
    pub(crate) fn is_closed(&self) -> bool {
        match &self.0 {
            EventDestination::Runtime { ordinary, .. }
            | EventDestination::TestReceiver(ordinary) => ordinary.is_closed(),
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
        request: AcknowledgedEventRequest,
    ) -> Result<(), mpsc::error::SendError<AcknowledgedEventRequest>> {
        match &self.0 {
            AcknowledgedDestination::Runtime {
                ordinary,
                cancellation,
            } => {
                let AcknowledgedEventRequest {
                    event,
                    acknowledgement,
                    retirement,
                } = request;
                match event {
                    Event::UserCancel { reason, cause } => cancellation
                        .send(CancellationInput::Acknowledged {
                            cancellation: UserCancellation { reason, cause },
                            acknowledgement,
                            retirement,
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
                            mpsc::error::SendError(AcknowledgedEventRequest {
                                event: cancellation.into_event(),
                                acknowledgement,
                                retirement,
                            })
                        }),
                    event => {
                        ordinary
                            .send(AcknowledgedEventRequest {
                                event,
                                acknowledgement,
                                retirement,
                            })
                            .await
                    }
                }
            }
            #[cfg(test)]
            AcknowledgedDestination::TestReceiver(sender) => sender.send(request).await,
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test_receiver(sender: mpsc::Sender<AcknowledgedEventRequest>) -> Self {
        Self(AcknowledgedDestination::TestReceiver(sender))
    }
}
