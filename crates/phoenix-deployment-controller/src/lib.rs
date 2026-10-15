mod candidate;
mod identity;
#[cfg(target_os = "macos")]
mod macos;
mod os;
mod ownership;
mod resource;
mod state;

pub use candidate::{ArtifactDigest, FullCommitSha, InvalidHexIdentity, RetainedCandidateIdentity};
pub use identity::{
    BirthToken, CandidateDiscovery, ChildEvidenceMismatch, ChildIdentityObservation, DiscoveryId,
    IdentityObservation, InvalidBirthToken, ObservationFailure, ProcessId, ProcessIdentity,
    StableChildEvidence,
};
#[cfg(target_os = "macos")]
pub use macos::{
    MacOsControllerOs, MacOsError, SupervisedChild, SupervisedFinalization, SupervisionRisk,
};
pub use os::{
    CandidateVerification, ControllerOs, OsEffect, OsQueryError, ResourceObservationFailure,
    Signal, SignalOutcome,
};
pub use ownership::{
    ChildObservationOutcome, CleanupReadiness, DiscoveryGap, OwnedIdentityStatus, OwnedProcessTree,
    UnknownChildScan, UnknownDiscovery, UnknownOwnedIdentity, UnknownParent, UnresolvedDiscovery,
};
pub use resource::{
    CpuContentionBasisPoints, InvalidCpuContention, MemoryPressure, NonIncreasingSampleSequence,
    ResourceAssessment, ResourceBlock, ResourceCapacity, ResourceContention, ResourcePolicy,
    ResourceSample, ResourceWindow, SustainedResourceEvidence,
};
pub use state::{
    ActiveAttempt, AdministrativeRiskAcceptance, AttemptId, AttemptStore, CleanupOutcome,
    CommittedTransition, CompatibilityBoundary, DeploymentAttempt, MonotonicTick, OwnershipAttempt,
    Phase, ResourceSamplingAttempt, Transition, TransitionError,
};
