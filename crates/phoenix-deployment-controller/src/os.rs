use crate::{
    candidate::RetainedCandidateIdentity,
    identity::{
        CandidateDiscovery, ChildIdentityObservation, IdentityObservation, ProcessId,
        ProcessIdentity,
    },
    resource::ResourceSample,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Signal {
    Terminate,
    Kill,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SignalOutcome {
    Delivered,
    ExactIdentityAlreadyAbsent,
    PidReused,
    PermissionDenied,
    TemporarilyUnavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OsEffect {
    SampleResources {
        sequence: u64,
    },
    ListChildren {
        parent: ProcessIdentity,
    },
    AcquireChildIdentity {
        discovery: CandidateDiscovery,
    },
    ObserveIdentity {
        identity: ProcessIdentity,
    },
    SignalExactIdentity {
        identity: ProcessIdentity,
        signal: Signal,
    },
    ReverifyRetainedCandidate {
        candidate: RetainedCandidateIdentity,
    },
}

pub trait ControllerOs {
    fn sample_resources(&mut self, sequence: u64) -> ResourceSample;

    /// # Errors
    /// Returns a typed query error when enumeration cannot produce trustworthy evidence.
    fn list_children(&mut self, parent: ProcessIdentity) -> Result<Vec<ProcessId>, OsQueryError>;

    fn acquire_child_identity(&mut self, discovery: CandidateDiscovery)
        -> ChildIdentityObservation;

    fn observe_identity(&mut self, identity: ProcessIdentity) -> IdentityObservation;

    fn signal_exact_identity(&mut self, identity: ProcessIdentity, signal: Signal)
        -> SignalOutcome;

    fn reverify_retained_candidate(
        &mut self,
        candidate: RetainedCandidateIdentity,
    ) -> CandidateVerification;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OsQueryError {
    PermissionDenied,
    TemporarilyUnavailable,
    Malformed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CandidateVerification {
    Exact,
    Missing,
    Mismatch,
    Unavailable,
}
