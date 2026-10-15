//! Standalone deployment policy with checkpoint-before-effect authority.
//!
//! The controller-owned SQLite store supports only its exact schema v1. Each
//! durable write compares and advances the attempt revision atomically. Successful
//! persistence returns a checkpoint carrying that revision plus its OS effects;
//! failures drop the transition and poison the store handle until reopen.
//!
//! Loading returns affine metadata/recovery evidence, not a live attempt. Active
//! evidence must checkpoint interruption before fresh recovery observation. Stable
//! dispositions expose metadata only; migration recovery reverifies the retained
//! candidate through a separately persisted transition.

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
pub use state::sqlite::{ControllerSqliteStore, ControllerStoreError};
pub use state::{
    ActiveAttempt, AdministrativeRiskAcceptance, AttemptId, AttemptStore, CleanupOutcome,
    CommittedTransition, CompatibilityBoundary, DeploymentAttempt, LoadedAttempt, MonotonicTick,
    OwnershipAttempt, PersistenceRevision, Phase, RecoveryDisposition, ResourceSamplingAttempt,
    StableAttempt, Transition, TransitionError,
};
