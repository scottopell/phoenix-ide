//! Standalone deployment policy with checkpoint-before-effect authority.
//!
//! The controller-owned `SQLite` store supports only its exact schema v4. Each
//! durable write compares and advances the attempt revision atomically. Successful
//! persistence returns a checkpoint carrying that revision plus its OS effects;
//! failures drop the transition and poison the store handle until reopen.
//!
//! Loading returns affine metadata/recovery evidence, not a live attempt. Active
//! evidence must checkpoint interruption before fresh recovery observation. Stable
//! dispositions expose metadata only; migration recovery reverifies the retained
//! candidate through a separately persisted transition.

mod candidate;
mod driver;
pub mod guardian;
pub mod guardian_ipc;
mod identity;
#[cfg(target_os = "macos")]
mod macos;
pub mod native_binding;
#[cfg(unix)]
pub mod native_guardian;
mod os;
mod ownership;
mod resource;
pub mod runtime;
mod state;

pub use candidate::{ArtifactDigest, FullCommitSha, InvalidHexIdentity, RetainedCandidateIdentity};
pub use driver::{
    DispatchBatch, DispatchKey, DispatchTicket, EffectCompletion, EffectExecutor, ExecutedEffect,
};
pub use guardian::{
    AppliedResultAck, BoundSubmissionError, BoundSubmissionRefusal, ChildSlotId,
    ChildSlotObservation, ControllerSession, CustodySummary, ForfeitedCustody, Guardian,
    GuardianBuildFailure, GuardianError, GuardianExecutor, GuardianInstanceId, GuardianState,
    QuarantinedResult, ReconciledSessionPermit, SessionDispatchTicket, TicketBindingRefusal,
    MAX_CHILD_SLOTS, MAX_RETAINED_RESULTS,
};
pub use identity::{
    BirthToken, CandidateDiscovery, ChildEvidenceMismatch, ChildIdentityObservation, DiscoveryId,
    IdentityObservation, InvalidBirthToken, ObservationFailure, ProcessId, ProcessIdentity,
    StableChildEvidence,
};
#[cfg(target_os = "macos")]
pub use macos::{
    MacOsControllerOs, MacOsError, MacOsGuardianExecutor, SupervisedChild, SupervisedFinalization,
    SupervisionRisk,
};
pub use native_binding::{
    GuardianLifetimeNonce, NativeGuardianBinding, NativeGuardianIdentity, NativeGuardianSession,
    NativeGuardianSlot, NativeSessionState, NativeSlotLiveness,
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
pub use runtime::{
    AckDisposition, EffectMismatchDetail, GuardianLoopError, LoopError, LoopFailure,
    LoopFenceReason, LoopProgress, LoopStatus, LoopSummary, ParkReason, PolicyLoop, SignalResult,
};
pub use state::sqlite::{
    ControllerSqliteStore, ControllerStoreError, NativeSamplingCompletion, NativeSamplingDispatch,
    NativeSamplingIssue, NativeSamplingStatus, VerifiedNativeSamplingAckConfirmation,
    VerifiedNativeSamplingCompletion,
};
pub use state::{
    ActiveAttempt, AdministrativeRiskAcceptance, AttemptId, AttemptStore, CleanupOutcome,
    CommittedTransition, CompatibilityBoundary, DeploymentAttempt, LoadedAttempt, MonotonicTick,
    OwnershipAttempt, PersistenceRevision, Phase, RecoveryDisposition, ResourceSamplingAttempt,
    StableAttempt, Transition, TransitionError, MAX_OWNED_SIGNAL_BATCH,
};
