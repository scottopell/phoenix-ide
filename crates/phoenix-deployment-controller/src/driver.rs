//! Affine dispatch of committed effects. This boundary does not apply results.
use crate::{
    AttemptId, CandidateDiscovery, CandidateVerification, ChildIdentityObservation,
    CommittedTransition, ControllerOs, DeploymentAttempt, IdentityObservation, OsEffect,
    OsQueryError, PersistenceRevision, ProcessId, ProcessIdentity, ResourceObservationFailure,
    ResourceSample, RetainedCandidateIdentity, Signal, SignalOutcome,
};

/// Correlation only, not executable authority. Ordinals are zero-based plan positions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DispatchKey {
    attempt_id: AttemptId,
    revision: PersistenceRevision,
    ordinal: usize,
}

impl DispatchKey {
    pub(crate) const fn from_committed_parts(
        attempt_id: AttemptId,
        revision: PersistenceRevision,
        ordinal: usize,
    ) -> Self {
        Self {
            attempt_id,
            revision,
            ordinal,
        }
    }

    #[must_use]
    pub const fn attempt_id(self) -> AttemptId {
        self.attempt_id
    }

    #[must_use]
    pub const fn revision(self) -> PersistenceRevision {
        self.revision
    }

    #[must_use]
    pub const fn ordinal(self) -> usize {
        self.ordinal
    }
}

/// Admission to `EffectExecutor` for one committed effect. Dropping it performs no call.
///
/// ```compile_fail
/// use phoenix_deployment_controller::DispatchTicket;
/// fn duplicate(ticket: DispatchTicket) { let _ = ticket.clone(); }
/// ```
///
/// ```compile_fail
/// use phoenix_deployment_controller::{DispatchKey, DispatchTicket, OsEffect};
/// fn fabricate(key: DispatchKey, effect: OsEffect) -> DispatchTicket {
///     DispatchTicket { key, effect }
/// }
/// ```
#[derive(Debug)]
pub struct DispatchTicket {
    key: DispatchKey,
    effect: OsEffect,
}

impl DispatchTicket {
    pub(crate) const fn from_committed_parts(key: DispatchKey, effect: OsEffect) -> Self {
        Self { key, effect }
    }

    #[must_use]
    pub const fn key(&self) -> DispatchKey {
        self.key
    }

    #[must_use]
    pub const fn effect(&self) -> &OsEffect {
        &self.effect
    }
}

/// An affine iterator of sibling tickets; dropping unissued siblings has no effect.
/// This is not an exactly-once-across-crash guarantee.
///
/// ```compile_fail
/// use phoenix_deployment_controller::DispatchBatch;
/// fn duplicate(batch: DispatchBatch) { let _ = batch.clone(); }
/// ```
///
/// ```compile_fail
/// use phoenix_deployment_controller::{DispatchBatch, DispatchTicket};
/// fn fabricate(tickets: Vec<DispatchTicket>) -> DispatchBatch {
///     DispatchBatch { tickets: tickets.into_iter() }
/// }
/// ```
#[derive(Debug)]
pub struct DispatchBatch {
    tickets: std::vec::IntoIter<DispatchTicket>,
}

impl DispatchBatch {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tickets.as_slice().is_empty()
    }
}

impl Iterator for DispatchBatch {
    type Item = DispatchTicket;

    fn next(&mut self) -> Option<Self::Item> {
        self.tickets.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.tickets.size_hint()
    }
}

impl ExactSizeIterator for DispatchBatch {}

impl CommittedTransition {
    /// Consumes the sole committed plan to issue a batch; diagnostic effects alone
    /// cannot be converted into tickets.
    ///
    /// # Panics
    /// Panics if the private committed checkpoint lacks its persisted revision.
    #[must_use]
    pub fn into_dispatch(self) -> (DeploymentAttempt, DispatchBatch) {
        let (checkpoint, effects) = self.into_parts();
        let revision = checkpoint
            .persistence_revision()
            .expect("committed revision");
        let attempt_id = checkpoint.attempt_id();
        let tickets = effects
            .into_iter()
            .enumerate()
            .map(|(ordinal, effect)| DispatchTicket {
                key: DispatchKey {
                    attempt_id,
                    revision,
                    ordinal,
                },
                effect,
            })
            .collect::<Vec<_>>()
            .into_iter();
        (checkpoint, DispatchBatch { tickets })
    }
}

/// Input identity and typed output share a variant, so an observation cannot be
/// paired with another kind of effect. `Exact` verification is observation only.
#[derive(Debug, Eq, PartialEq)]
pub enum EffectCompletion {
    SampleResources {
        sequence: u64,
        result: Result<ResourceSample, ResourceObservationFailure>,
    },
    ListChildren {
        parent: ProcessIdentity,
        result: Result<Vec<ProcessId>, OsQueryError>,
    },
    AcquireChildIdentity {
        discovery: CandidateDiscovery,
        observation: ChildIdentityObservation,
    },
    ObserveIdentity {
        identity: ProcessIdentity,
        observation: IdentityObservation,
    },
    SignalExactIdentity {
        identity: ProcessIdentity,
        signal: Signal,
        outcome: SignalOutcome,
    },
    ReverifyRetainedCandidate {
        candidate: RetainedCandidateIdentity,
        verification: CandidateVerification,
    },
}

impl EffectCompletion {
    #[must_use]
    pub const fn effect(&self) -> OsEffect {
        match *self {
            Self::SampleResources { sequence, .. } => OsEffect::SampleResources { sequence },
            Self::ListChildren { parent, .. } => OsEffect::ListChildren { parent },
            Self::AcquireChildIdentity { discovery, .. } => {
                OsEffect::AcquireChildIdentity { discovery }
            }
            Self::ObserveIdentity { identity, .. } => OsEffect::ObserveIdentity { identity },
            Self::SignalExactIdentity {
                identity, signal, ..
            } => OsEffect::SignalExactIdentity { identity, signal },
            Self::ReverifyRetainedCandidate { candidate, .. } => {
                OsEffect::ReverifyRetainedCandidate { candidate }
            }
        }
    }
}

/// Only the executor can pair a dispatch key with its actual completion.
///
/// ```compile_fail
/// use phoenix_deployment_controller::{DispatchKey, EffectCompletion, ExecutedEffect};
/// fn fabricate(key: DispatchKey, completion: EffectCompletion) -> ExecutedEffect {
///     ExecutedEffect { key, completion }
/// }
/// ```
#[derive(Debug, Eq, PartialEq)]
pub struct ExecutedEffect {
    key: DispatchKey,
    completion: EffectCompletion,
}

impl ExecutedEffect {
    #[must_use]
    pub const fn key(&self) -> DispatchKey {
        self.key
    }

    #[must_use]
    pub const fn completion(&self) -> &EffectCompletion {
        &self.completion
    }

    #[must_use]
    pub fn into_parts(self) -> (DispatchKey, EffectCompletion) {
        (self.key, self.completion)
    }
}

pub struct EffectExecutor<'a, O: ControllerOs + ?Sized> {
    os: &'a mut O,
}

impl<'a, O: ControllerOs + ?Sized> EffectExecutor<'a, O> {
    #[must_use]
    pub fn new(os: &'a mut O) -> Self {
        Self { os }
    }

    /// Performs one call, without retry, phase interpretation, or result application.
    #[must_use]
    pub fn execute(&mut self, ticket: DispatchTicket) -> ExecutedEffect {
        let DispatchTicket { key, effect } = ticket;
        let completion = match effect {
            OsEffect::SampleResources { sequence } => EffectCompletion::SampleResources {
                sequence,
                result: self.os.sample_resources(sequence),
            },
            OsEffect::ListChildren { parent } => EffectCompletion::ListChildren {
                parent,
                result: self.os.list_children(parent),
            },
            OsEffect::AcquireChildIdentity { discovery } => {
                EffectCompletion::AcquireChildIdentity {
                    discovery,
                    observation: self.os.acquire_child_identity(discovery),
                }
            }
            OsEffect::ObserveIdentity { identity } => EffectCompletion::ObserveIdentity {
                identity,
                observation: self.os.observe_identity(identity),
            },
            OsEffect::SignalExactIdentity { identity, signal } => {
                EffectCompletion::SignalExactIdentity {
                    identity,
                    signal,
                    outcome: self.os.signal_exact_identity(identity, signal),
                }
            }
            OsEffect::ReverifyRetainedCandidate { candidate } => {
                EffectCompletion::ReverifyRetainedCandidate {
                    candidate,
                    verification: self.os.reverify_retained_candidate(candidate),
                }
            }
        };
        ExecutedEffect { key, completion }
    }
}
