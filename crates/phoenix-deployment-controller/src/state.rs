pub(crate) mod sqlite;

use crate::{
    candidate::RetainedCandidateIdentity,
    identity::{
        CandidateDiscovery, ChildIdentityObservation, IdentityObservation, ProcessId,
        ProcessIdentity,
    },
    os::{CandidateVerification, OsEffect, Signal},
    ownership::{
        ChildObservationOutcome, CleanupReadiness, OwnedProcessTree, UnknownChildScan,
        UnknownDiscovery, UnknownOwnedIdentity, UnknownParent,
    },
    resource::{
        NonIncreasingSampleSequence, ResourceAssessment, ResourceBlock, ResourcePolicy,
        ResourceSample, ResourceWindow, SustainedResourceEvidence,
    },
};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct AttemptId(u128);

impl AttemptId {
    #[must_use]
    pub const fn new(value: u128) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn get(self) -> u128 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AdministrativeRiskAcceptance {
    attempt_id: AttemptId,
}

impl AdministrativeRiskAcceptance {
    #[must_use]
    pub const fn for_attempt(attempt_id: AttemptId) -> Self {
        Self { attempt_id }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct MonotonicTick(u64);

impl MonotonicTick {
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompatibilityBoundary {
    PreMigration,
    CandidateMigrationInvoked(RetainedCandidateIdentity),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CleanupOutcome {
    NotStarted,
    InProgress,
    VerifiedClean,
    UnresolvedRisk,
    AdministrativeRiskAccepted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    ResourceSampling,
    OwnershipDiscovery,
    AwaitingBirthEvidence,
    CleaningOwnedTree,
    VerifyingCleanup,
    ReadyForDisruption,
    ResourceBlocked,
    Interrupted,
    Recovering,
    ForwardRecovery,
    AdministrativeCloseoutRequired,
    AdministrativelyClosed,
}

/// A durable, nonzero checkpoint revision. Absence means never persisted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PersistenceRevision(std::num::NonZeroU64);

impl PersistenceRevision {
    #[must_use]
    pub const fn new(value: std::num::NonZeroU64) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn first() -> Self {
        Self(std::num::NonZeroU64::MIN)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0.get()
    }

    #[must_use]
    pub fn checked_next(self) -> Option<Self> {
        self.get()
            .checked_add(1)
            .and_then(std::num::NonZeroU64::new)
            .map(Self)
    }
}

/// Crash-loaded evidence has no live mutation or signal capability.
///
/// ```compile_fail
/// use phoenix_deployment_controller::{LoadedAttempt, MonotonicTick, Signal};
/// fn signal(loaded: LoadedAttempt) {
///     loaded.request_owned_signals(Signal::Terminate, MonotonicTick::new(0));
/// }
/// ```
///
/// ```compile_fail
/// use phoenix_deployment_controller::LoadedAttempt;
/// fn duplicate(loaded: LoadedAttempt) {
///     let _ = loaded.clone();
/// }
/// ```
///
/// ```compile_fail
/// use phoenix_deployment_controller::{DeploymentAttempt, LoadedAttempt};
/// fn checkpoint(loaded: LoadedAttempt) -> DeploymentAttempt {
///     loaded.checkpoint().clone()
/// }
/// ```
pub struct LoadedAttempt {
    checkpoint: DeploymentAttempt,
}

/// Classification after consuming crash-loaded evidence. Effects in a checkpoint
/// remain private until its persistence succeeds.
#[derive(Debug)]
pub enum RecoveryDisposition {
    Checkpoint(Box<Transition>),
    Stable(StableAttempt),
}

/// Crash-loaded stable evidence exposes only phase-appropriate, persistence-gated
/// continuation. It cannot request process mutation.
#[derive(Debug)]
pub struct StableAttempt {
    checkpoint: Box<DeploymentAttempt>,
}

impl StableAttempt {
    #[must_use]
    pub const fn attempt_id(&self) -> AttemptId {
        self.checkpoint.attempt_id()
    }

    #[must_use]
    pub const fn phase(&self) -> Phase {
        self.checkpoint.phase()
    }

    #[must_use]
    pub const fn compatibility_boundary(&self) -> CompatibilityBoundary {
        self.checkpoint.compatibility_boundary()
    }

    #[must_use]
    pub const fn cleanup_outcome(&self) -> CleanupOutcome {
        self.checkpoint.cleanup_outcome()
    }

    #[must_use]
    pub const fn resource_block_reason(&self) -> Option<ResourceBlock> {
        self.checkpoint.resource_block_reason()
    }

    /// # Errors
    /// Returns an error unless the loaded checkpoint is pre-migration ready.
    pub fn record_migration_invoked(
        self,
        candidate: RetainedCandidateIdentity,
    ) -> Result<Transition, TransitionError> {
        (*self.checkpoint).record_migration_invoked(candidate)
    }

    /// # Errors
    /// Returns an error unless the loaded checkpoint requires administrative closeout
    /// and the acceptance is bound to the same attempt.
    pub fn accept_administrative_risk(
        self,
        acceptance: AdministrativeRiskAcceptance,
    ) -> Result<Transition, TransitionError> {
        (*self.checkpoint).accept_administrative_risk(acceptance)
    }
}

impl LoadedAttempt {
    #[must_use]
    pub const fn attempt_id(&self) -> AttemptId {
        self.checkpoint.attempt_id()
    }

    #[must_use]
    pub const fn phase(&self) -> Phase {
        self.checkpoint.phase()
    }

    /// # Panics
    /// Panics if private loaded evidence lacks its store-validated revision.
    #[must_use]
    pub fn persistence_revision(&self) -> PersistenceRevision {
        // Only the store constructs loaded evidence, after validating its revision.
        self.checkpoint
            .persistence_revision()
            .expect("loaded revision")
    }

    #[must_use]
    pub const fn cleanup_outcome(&self) -> CleanupOutcome {
        self.checkpoint.cleanup_outcome()
    }

    /// Active work first checkpoints interruption without OS effects. A further
    /// committed recovery transition restarts the fresh-observation barrier.
    ///
    /// # Errors
    /// Returns an error if the stored phase cannot prepare recovery.
    pub fn prepare_recovery(self) -> Result<RecoveryDisposition, TransitionError> {
        match self.checkpoint {
            attempt @ DeploymentAttempt::Active(_) => attempt
                .interrupt()
                .map(Box::new)
                .map(RecoveryDisposition::Checkpoint),
            attempt @ (DeploymentAttempt::Interrupted(_)
            | DeploymentAttempt::ForwardRecovery(_)) => attempt
                .recover()
                .map(Box::new)
                .map(RecoveryDisposition::Checkpoint),
            DeploymentAttempt::Ready(attempt)
                if matches!(
                    attempt.context.compatibility,
                    CompatibilityBoundary::CandidateMigrationInvoked(_)
                ) =>
            {
                DeploymentAttempt::Ready(attempt)
                    .recover()
                    .map(Box::new)
                    .map(RecoveryDisposition::Checkpoint)
            }
            attempt @ (DeploymentAttempt::Ready(_)
            | DeploymentAttempt::ResourceBlocked(_)
            | DeploymentAttempt::AdministrativeCloseoutRequired(_)
            | DeploymentAttempt::AdministrativelyClosed(_)) => {
                Ok(RecoveryDisposition::Stable(StableAttempt {
                    checkpoint: Box::new(attempt),
                }))
            }
        }
    }
}

// Debug exposes the same safe metadata as the public accessors.
impl std::fmt::Debug for LoadedAttempt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoadedAttempt")
            .field("id", &self.attempt_id())
            .field("phase", &self.phase())
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct AttemptContext {
    id: AttemptId,
    revision: Option<PersistenceRevision>,
    deadline: MonotonicTick,
    compatibility: CompatibilityBoundary,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourceSamplingAttempt {
    context: AttemptContext,
    root: ProcessIdentity,
    pending_sample_sequence: u64,
    window: ResourceWindow,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OwnershipPhase {
    Discovery,
    AwaitingBirthEvidence,
    Cleaning,
    Verifying,
    Recovering,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RecoveryResume {
    Discovery,
    AwaitingBirthEvidence,
    Cleaning,
    Verifying,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OwnershipAttempt {
    context: AttemptContext,
    resources: SustainedResourceEvidence,
    tree: OwnedProcessTree,
    phase: OwnershipPhase,
    recovery_resume: Option<RecoveryResume>,
    cleanup: CleanupOutcome,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ActiveAttempt {
    ResourceSampling(ResourceSamplingAttempt),
    Ownership(OwnershipAttempt),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadyAttempt {
    context: AttemptContext,
    resources: SustainedResourceEvidence,
    tree: OwnedProcessTree,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResourceBlockedAttempt {
    context: AttemptContext,
    reason: ResourceBlock,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InterruptedAttempt {
    context: AttemptContext,
    resume: ActiveAttempt,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdministrativeCloseoutRequired {
    context: AttemptContext,
    resources: SustainedResourceEvidence,
    tree: OwnedProcessTree,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdministrativelyClosed {
    context: AttemptContext,
    resources: SustainedResourceEvidence,
    tree: OwnedProcessTree,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ForwardRecoveryAttempt {
    context: AttemptContext,
    candidate: RetainedCandidateIdentity,
    verification: Option<CandidateVerification>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeploymentAttempt {
    Active(ActiveAttempt),
    Ready(ReadyAttempt),
    ResourceBlocked(ResourceBlockedAttempt),
    Interrupted(InterruptedAttempt),
    ForwardRecovery(ForwardRecoveryAttempt),
    AdministrativeCloseoutRequired(AdministrativeCloseoutRequired),
    AdministrativelyClosed(AdministrativelyClosed),
}

/// An uncommitted plan cannot issue tickets.
///
/// ```compile_fail
/// use phoenix_deployment_controller::Transition;
/// fn dispatch_before_persistence(pending: Transition) {
///     let _ = pending.into_dispatch();
/// }
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Transition {
    checkpoint: DeploymentAttempt,
    after_persist: Vec<OsEffect>,
}

pub trait AttemptStore {
    type Error;

    /// A successful return is trusted as durable issuance authority. Implementations
    /// must not return a revision before the complete checkpoint commits.
    ///
    /// Atomically compares `checkpoint.persistence_revision()` with the durable
    /// row, stores the complete checkpoint with its checked next revision, and
    /// returns that committed revision. An unpersisted checkpoint requires no row.
    ///
    /// # Errors
    /// Returns the store's typed error on conflict, exhaustion, or storage failure.
    fn persist(
        &mut self,
        checkpoint: &DeploymentAttempt,
    ) -> Result<PersistenceRevision, Self::Error>;
}

/// Persisted checkpoint and effect plan; consuming it can issue executor admission.
///
/// ```compile_fail
/// use phoenix_deployment_controller::CommittedTransition;
/// fn duplicate(committed: CommittedTransition) {
///     let _ = committed.clone();
/// }
/// ```
#[derive(Debug, Eq, PartialEq)]
pub struct CommittedTransition {
    checkpoint: DeploymentAttempt,
    after_persist: Vec<OsEffect>,
}

impl Transition {
    /// # Errors
    /// Drops the transition and all effect capabilities on failure. An ambiguous
    /// commit requires reopening and classifying durable evidence, not retrying.
    pub fn persist<S: AttemptStore>(
        mut self,
        store: &mut S,
    ) -> Result<CommittedTransition, S::Error> {
        let revision = store.persist(&self.checkpoint)?;
        self.checkpoint.set_revision(revision);
        Ok(CommittedTransition {
            checkpoint: self.checkpoint,
            after_persist: self.after_persist,
        })
    }
}

impl CommittedTransition {
    #[must_use]
    pub fn checkpoint(&self) -> &DeploymentAttempt {
        &self.checkpoint
    }

    #[must_use]
    pub fn after_persist(&self) -> &[OsEffect] {
        &self.after_persist
    }

    #[must_use]
    pub fn into_parts(self) -> (DeploymentAttempt, Vec<OsEffect>) {
        (self.checkpoint, self.after_persist)
    }
}

impl DeploymentAttempt {
    #[must_use]
    pub fn start(
        id: AttemptId,
        deadline: MonotonicTick,
        policy: ResourcePolicy,
        root: ProcessIdentity,
    ) -> Transition {
        let attempt = Self::Active(ActiveAttempt::ResourceSampling(ResourceSamplingAttempt {
            context: AttemptContext {
                id,
                revision: None,
                deadline,
                compatibility: CompatibilityBoundary::PreMigration,
            },
            root,
            pending_sample_sequence: 0,
            window: ResourceWindow::new(policy),
        }));
        Transition {
            checkpoint: attempt,
            after_persist: vec![OsEffect::SampleResources { sequence: 0 }],
        }
    }

    #[must_use]
    pub const fn phase(&self) -> Phase {
        match self {
            Self::Active(ActiveAttempt::ResourceSampling(_)) => Phase::ResourceSampling,
            Self::Active(ActiveAttempt::Ownership(attempt)) => match attempt.phase {
                OwnershipPhase::Discovery => Phase::OwnershipDiscovery,
                OwnershipPhase::AwaitingBirthEvidence => Phase::AwaitingBirthEvidence,
                OwnershipPhase::Cleaning => Phase::CleaningOwnedTree,
                OwnershipPhase::Verifying => Phase::VerifyingCleanup,
                OwnershipPhase::Recovering => Phase::Recovering,
            },
            Self::Ready(_) => Phase::ReadyForDisruption,
            Self::ResourceBlocked(_) => Phase::ResourceBlocked,
            Self::Interrupted(_) => Phase::Interrupted,
            Self::ForwardRecovery(_) => Phase::ForwardRecovery,
            Self::AdministrativeCloseoutRequired(_) => Phase::AdministrativeCloseoutRequired,
            Self::AdministrativelyClosed(_) => Phase::AdministrativelyClosed,
        }
    }

    #[must_use]
    pub const fn deadline(&self) -> MonotonicTick {
        self.context().deadline
    }

    #[must_use]
    pub const fn attempt_id(&self) -> AttemptId {
        self.context().id
    }

    #[must_use]
    pub const fn persistence_revision(&self) -> Option<PersistenceRevision> {
        self.context().revision
    }

    #[must_use]
    pub const fn compatibility_boundary(&self) -> CompatibilityBoundary {
        self.context().compatibility
    }

    #[must_use]
    pub const fn cleanup_outcome(&self) -> CleanupOutcome {
        match self {
            Self::Active(ActiveAttempt::ResourceSampling(_))
            | Self::ResourceBlocked(_)
            | Self::Interrupted(InterruptedAttempt {
                resume: ActiveAttempt::ResourceSampling(_),
                ..
            }) => CleanupOutcome::NotStarted,
            Self::Active(ActiveAttempt::Ownership(attempt))
            | Self::Interrupted(InterruptedAttempt {
                resume: ActiveAttempt::Ownership(attempt),
                ..
            }) => attempt.cleanup,
            Self::Ready(_) => CleanupOutcome::VerifiedClean,
            Self::ForwardRecovery(_) | Self::AdministrativeCloseoutRequired(_) => {
                CleanupOutcome::UnresolvedRisk
            }
            Self::AdministrativelyClosed(_) => CleanupOutcome::AdministrativeRiskAccepted,
        }
    }

    /// # Errors
    /// Returns an error outside resource sampling or for a non-increasing sample sequence.
    pub fn record_resource_sample(
        self,
        sample: ResourceSample,
    ) -> Result<Transition, TransitionError> {
        let Self::Active(ActiveAttempt::ResourceSampling(mut attempt)) = self else {
            return Err(TransitionError::WrongPhase);
        };
        if sample.sequence != attempt.pending_sample_sequence {
            return Err(TransitionError::UnexpectedSampleSequence {
                expected: attempt.pending_sample_sequence,
                actual: sample.sequence,
            });
        }
        let assessment = attempt
            .window
            .record(sample)
            .map_err(TransitionError::NonIncreasingSample)?;
        match assessment {
            ResourceAssessment::NeedMoreSamples { .. } => {
                attempt.pending_sample_sequence = attempt.pending_sample_sequence.saturating_add(1);
                let sequence = attempt.pending_sample_sequence;
                Ok(Transition {
                    checkpoint: Self::Active(ActiveAttempt::ResourceSampling(attempt)),
                    after_persist: vec![OsEffect::SampleResources { sequence }],
                })
            }
            ResourceAssessment::Admitted(resources) => {
                let root = attempt.root;
                let ownership = OwnershipAttempt {
                    context: attempt.context,
                    resources,
                    tree: OwnedProcessTree::new(root),
                    phase: OwnershipPhase::Discovery,
                    recovery_resume: None,
                    cleanup: CleanupOutcome::NotStarted,
                };
                Ok(Transition {
                    checkpoint: Self::Active(ActiveAttempt::Ownership(ownership)),
                    after_persist: vec![OsEffect::ListChildren { parent: root }],
                })
            }
            ResourceAssessment::Blocked(reason) => Ok(Transition {
                checkpoint: Self::ResourceBlocked(ResourceBlockedAttempt {
                    context: attempt.context,
                    reason,
                }),
                after_persist: Vec::new(),
            }),
        }
    }

    /// # Errors
    /// Returns an error outside ownership discovery or when the parent is not owned.
    pub fn child_discovered(
        self,
        parent: ProcessIdentity,
        pid: ProcessId,
    ) -> Result<(Transition, CandidateDiscovery), TransitionError> {
        let mut attempt = self.into_ownership()?;
        if !matches!(
            attempt.phase,
            OwnershipPhase::Discovery
                | OwnershipPhase::AwaitingBirthEvidence
                | OwnershipPhase::Recovering
        ) {
            return Err(TransitionError::WrongPhase);
        }
        let discovery = attempt
            .tree
            .discover_child(parent, pid)
            .map_err(TransitionError::UnknownParent)?;
        Ok((
            Transition {
                checkpoint: Self::Active(ActiveAttempt::Ownership(attempt)),
                after_persist: vec![OsEffect::AcquireChildIdentity { discovery }],
            },
            discovery,
        ))
    }

    /// # Errors
    /// Returns an error when the phase cannot accept evidence or the discovery is unknown.
    pub fn child_observed(
        self,
        discovery: CandidateDiscovery,
        observation: ChildIdentityObservation,
    ) -> Result<Transition, TransitionError> {
        let mut attempt = self.into_ownership()?;
        let recovering = attempt.phase == OwnershipPhase::Recovering;
        let outcome = attempt
            .tree
            .record_child_observation(discovery, observation)
            .map_err(TransitionError::UnknownDiscovery)?;
        let after_persist = match outcome {
            ChildObservationOutcome::Adopted(identity) => {
                if !recovering {
                    attempt.phase = OwnershipPhase::Discovery;
                }
                let mut effects = vec![OsEffect::ListChildren { parent: identity }];
                if recovering {
                    effects.push(OsEffect::ObserveIdentity { identity });
                }
                effects
            }
            ChildObservationOutcome::RemainsUnresolved => {
                if !recovering {
                    attempt.phase = OwnershipPhase::AwaitingBirthEvidence;
                }
                Vec::new()
            }
        };
        Ok(Transition {
            checkpoint: Self::Active(ActiveAttempt::Ownership(attempt)),
            after_persist,
        })
    }

    /// # Errors
    /// Returns [`TransitionError::WrongPhase`] unless the attempt awaits birth evidence.
    pub fn retry_unresolved_discoveries(self) -> Result<Transition, TransitionError> {
        let attempt = self.into_ownership()?;
        if attempt.phase != OwnershipPhase::AwaitingBirthEvidence {
            return Err(TransitionError::WrongPhase);
        }
        let effects = attempt
            .tree
            .unresolved()
            .values()
            .map(|unresolved| OsEffect::AcquireChildIdentity {
                discovery: unresolved.discovery,
            })
            .collect();
        Ok(Transition {
            checkpoint: Self::Active(ActiveAttempt::Ownership(attempt)),
            after_persist: effects,
        })
    }

    /// # Errors
    /// Returns an error unless `parent` has an unfinished scan in an ownership-discovery phase.
    pub fn child_scan_completed(
        self,
        parent: ProcessIdentity,
    ) -> Result<Transition, TransitionError> {
        let mut attempt = self.into_ownership()?;
        if !matches!(
            attempt.phase,
            OwnershipPhase::Discovery
                | OwnershipPhase::AwaitingBirthEvidence
                | OwnershipPhase::Recovering
        ) {
            return Err(TransitionError::WrongPhase);
        }
        attempt
            .tree
            .complete_child_scan(parent)
            .map_err(TransitionError::UnknownChildScan)?;
        if attempt.phase != OwnershipPhase::Recovering {
            attempt.phase = if attempt.tree.unresolved().is_empty() {
                OwnershipPhase::Discovery
            } else {
                OwnershipPhase::AwaitingBirthEvidence
            };
        }
        Ok(Transition {
            checkpoint: Self::Active(ActiveAttempt::Ownership(attempt)),
            after_persist: Vec::new(),
        })
    }

    /// # Errors
    /// Returns [`TransitionError::WrongPhase`] unless ownership discovery is active.
    pub fn ownership_scan_completed(self) -> Result<Transition, TransitionError> {
        let mut attempt = self.into_ownership()?;
        if !matches!(
            attempt.phase,
            OwnershipPhase::Discovery | OwnershipPhase::AwaitingBirthEvidence
        ) {
            return Err(TransitionError::WrongPhase);
        }
        if !attempt.tree.pending_child_scans().is_empty() {
            return Err(TransitionError::PendingChildScans);
        }
        if !attempt.tree.unresolved().is_empty() {
            attempt.phase = OwnershipPhase::AwaitingBirthEvidence;
            return Ok(Transition {
                checkpoint: Self::Active(ActiveAttempt::Ownership(attempt)),
                after_persist: Vec::new(),
            });
        }
        attempt.phase = OwnershipPhase::Cleaning;
        attempt.cleanup = CleanupOutcome::InProgress;
        let effects = observe_all(&attempt.tree);
        Ok(Transition {
            checkpoint: Self::Active(ActiveAttempt::Ownership(attempt)),
            after_persist: effects,
        })
    }

    /// # Errors
    /// Returns an error when the phase cannot accept evidence or the identity is not owned.
    pub fn identity_observed(
        self,
        identity: ProcessIdentity,
        observation: IdentityObservation,
    ) -> Result<Transition, TransitionError> {
        let mut attempt = self.into_ownership()?;
        if !matches!(
            attempt.phase,
            OwnershipPhase::Cleaning | OwnershipPhase::Verifying | OwnershipPhase::Recovering
        ) {
            return Err(TransitionError::WrongPhase);
        }
        attempt
            .tree
            .record_identity_observation(identity, observation)
            .map_err(TransitionError::UnknownOwnedIdentity)?;
        Ok(Transition {
            checkpoint: Self::Active(ActiveAttempt::Ownership(attempt)),
            after_persist: Vec::new(),
        })
    }

    /// # Errors
    /// Returns an error unless cleanup is active and every discovery has stable evidence.
    pub fn request_owned_signals(
        self,
        signal: Signal,
        now: MonotonicTick,
    ) -> Result<Transition, TransitionError> {
        let attempt = self.into_ownership()?;
        if attempt.phase != OwnershipPhase::Cleaning {
            return Err(TransitionError::WrongPhase);
        }
        if !attempt.tree.unresolved().is_empty() || !attempt.tree.pending_child_scans().is_empty() {
            return Err(TransitionError::UnresolvedOwnership);
        }
        if !attempt.tree.reconciliation_complete() {
            return Err(TransitionError::IncompleteReconciliation);
        }
        if now >= attempt.context.deadline {
            return match attempt.tree.cleanup_readiness() {
                CleanupReadiness::VerifiedClean => Ok(Transition {
                    checkpoint: Self::Ready(ReadyAttempt {
                        context: attempt.context,
                        resources: attempt.resources,
                        tree: attempt.tree,
                    }),
                    after_persist: Vec::new(),
                }),
                CleanupReadiness::OwnedIdentityMayRemain | CleanupReadiness::UnresolvedEvidence => {
                    Ok(to_administrative_closeout(attempt))
                }
            };
        }
        let effects = attempt
            .tree
            .exact_signal_candidates()
            .into_iter()
            .map(|identity| OsEffect::SignalExactIdentity { identity, signal })
            .collect();
        Ok(Transition {
            checkpoint: Self::Active(ActiveAttempt::Ownership(attempt)),
            after_persist: effects,
        })
    }

    /// # Errors
    /// Returns [`TransitionError::WrongPhase`] unless owned-tree cleanup is active.
    pub fn cleanup_signals_settled(self) -> Result<Transition, TransitionError> {
        let mut attempt = self.into_ownership()?;
        if attempt.phase != OwnershipPhase::Cleaning {
            return Err(TransitionError::WrongPhase);
        }
        attempt.phase = OwnershipPhase::Verifying;
        attempt.tree.mark_all_for_reconciliation();
        let effects = observe_all(&attempt.tree);
        Ok(Transition {
            checkpoint: Self::Active(ActiveAttempt::Ownership(attempt)),
            after_persist: effects,
        })
    }

    /// # Errors
    /// Returns an error unless recovery has freshly reconciled every acquired identity.
    pub fn recovery_reconciliation_completed(
        self,
        now: MonotonicTick,
    ) -> Result<Transition, TransitionError> {
        let mut attempt = self.into_ownership()?;
        if attempt.phase != OwnershipPhase::Recovering {
            return Err(TransitionError::WrongPhase);
        }
        if !attempt.tree.reconciliation_complete() {
            return Err(TransitionError::IncompleteReconciliation);
        }
        if now >= attempt.context.deadline {
            return match attempt.tree.cleanup_readiness() {
                CleanupReadiness::VerifiedClean => Ok(Transition {
                    checkpoint: Self::Ready(ReadyAttempt {
                        context: attempt.context,
                        resources: attempt.resources,
                        tree: attempt.tree,
                    }),
                    after_persist: Vec::new(),
                }),
                CleanupReadiness::OwnedIdentityMayRemain | CleanupReadiness::UnresolvedEvidence => {
                    Ok(to_administrative_closeout(attempt))
                }
            };
        }
        if !attempt.tree.pending_child_scans().is_empty() || !attempt.tree.unresolved().is_empty() {
            attempt.phase = if attempt.tree.unresolved().is_empty() {
                OwnershipPhase::Discovery
            } else {
                OwnershipPhase::AwaitingBirthEvidence
            };
            attempt.recovery_resume = None;
            return Ok(Transition {
                checkpoint: Self::Active(ActiveAttempt::Ownership(attempt)),
                after_persist: Vec::new(),
            });
        }

        let resume = attempt
            .recovery_resume
            .take()
            .ok_or(TransitionError::MissingRecoveryResume)?;
        attempt.phase = match resume {
            RecoveryResume::Discovery | RecoveryResume::AwaitingBirthEvidence => {
                attempt.cleanup = CleanupOutcome::InProgress;
                OwnershipPhase::Cleaning
            }
            RecoveryResume::Cleaning => OwnershipPhase::Cleaning,
            RecoveryResume::Verifying => OwnershipPhase::Verifying,
        };
        Ok(Transition {
            checkpoint: Self::Active(ActiveAttempt::Ownership(attempt)),
            after_persist: Vec::new(),
        })
    }

    /// # Errors
    /// Returns [`TransitionError::WrongPhase`] unless verification or recovery is active.
    pub fn verification_completed(self, now: MonotonicTick) -> Result<Transition, TransitionError> {
        let mut attempt = self.into_ownership()?;
        if attempt.phase != OwnershipPhase::Verifying {
            return Err(TransitionError::WrongPhase);
        }
        match attempt.tree.cleanup_readiness() {
            CleanupReadiness::VerifiedClean => Ok(Transition {
                checkpoint: Self::Ready(ReadyAttempt {
                    context: attempt.context,
                    resources: attempt.resources,
                    tree: attempt.tree,
                }),
                after_persist: Vec::new(),
            }),
            CleanupReadiness::OwnedIdentityMayRemain if now < attempt.context.deadline => {
                attempt.phase = OwnershipPhase::Cleaning;
                Ok(Transition {
                    checkpoint: Self::Active(ActiveAttempt::Ownership(attempt)),
                    after_persist: Vec::new(),
                })
            }
            CleanupReadiness::UnresolvedEvidence if now < attempt.context.deadline => {
                attempt.phase = OwnershipPhase::AwaitingBirthEvidence;
                Ok(Transition {
                    checkpoint: Self::Active(ActiveAttempt::Ownership(attempt)),
                    after_persist: Vec::new(),
                })
            }
            CleanupReadiness::OwnedIdentityMayRemain | CleanupReadiness::UnresolvedEvidence => {
                Ok(to_administrative_closeout(attempt))
            }
        }
    }

    /// # Errors
    /// Returns an error before the deadline, without unresolved risk, or outside ownership work.
    pub fn deadline_reached(self, now: MonotonicTick) -> Result<Transition, TransitionError> {
        let attempt = self.into_ownership()?;
        if now < attempt.context.deadline {
            return Err(TransitionError::DeadlineNotReached);
        }
        if attempt.tree.cleanup_readiness() == CleanupReadiness::VerifiedClean {
            return Err(TransitionError::NoUnresolvedRisk);
        }
        Ok(to_administrative_closeout(attempt))
    }

    /// # Errors
    /// Returns [`TransitionError::WrongPhase`] unless the attempt has active work.
    pub fn interrupt(self) -> Result<Transition, TransitionError> {
        let Self::Active(active) = self else {
            return Err(TransitionError::WrongPhase);
        };
        let context = *active.context();
        Ok(Transition {
            checkpoint: Self::Interrupted(InterruptedAttempt {
                context,
                resume: active,
            }),
            after_persist: Vec::new(),
        })
    }

    /// # Errors
    /// Returns [`TransitionError::WrongPhase`] unless the checkpoint has recoverable work.
    pub fn recover(self) -> Result<Transition, TransitionError> {
        match self {
            Self::Interrupted(interrupted) => match interrupted.context.compatibility {
                CompatibilityBoundary::CandidateMigrationInvoked(candidate) => {
                    Ok(forward_recovery_transition(interrupted.context, candidate))
                }
                CompatibilityBoundary::PreMigration => match interrupted.resume {
                    ActiveAttempt::ResourceSampling(attempt) => {
                        let sequence = attempt.pending_sample_sequence;
                        Ok(Transition {
                            checkpoint: Self::Active(ActiveAttempt::ResourceSampling(attempt)),
                            after_persist: vec![OsEffect::SampleResources { sequence }],
                        })
                    }
                    ActiveAttempt::Ownership(mut attempt) => {
                        let resume = if attempt.phase == OwnershipPhase::Recovering {
                            attempt
                                .recovery_resume
                                .ok_or(TransitionError::MissingRecoveryResume)?
                        } else {
                            recovery_resume_for(attempt.phase)?
                        };
                        attempt.phase = OwnershipPhase::Recovering;
                        attempt.recovery_resume = Some(resume);
                        attempt.tree.mark_all_for_reconciliation();
                        let effects = recovery_effects(&attempt.tree);
                        Ok(Transition {
                            checkpoint: Self::Active(ActiveAttempt::Ownership(attempt)),
                            after_persist: effects,
                        })
                    }
                },
            },
            Self::Ready(attempt) => match attempt.context.compatibility {
                CompatibilityBoundary::CandidateMigrationInvoked(candidate) => {
                    Ok(forward_recovery_transition(attempt.context, candidate))
                }
                CompatibilityBoundary::PreMigration => Err(TransitionError::WrongPhase),
            },
            Self::ForwardRecovery(mut attempt) => {
                attempt.verification = None;
                let candidate = attempt.candidate;
                Ok(Transition {
                    checkpoint: Self::ForwardRecovery(attempt),
                    after_persist: vec![OsEffect::ReverifyRetainedCandidate { candidate }],
                })
            }
            Self::Active(_)
            | Self::ResourceBlocked(_)
            | Self::AdministrativeCloseoutRequired(_)
            | Self::AdministrativelyClosed(_) => Err(TransitionError::WrongPhase),
        }
    }

    /// # Errors
    /// Returns an error unless verified cleanup is ready or a different candidate was already frozen.
    pub fn record_migration_invoked(
        self,
        candidate: RetainedCandidateIdentity,
    ) -> Result<Transition, TransitionError> {
        let Self::Ready(mut attempt) = self else {
            return Err(TransitionError::WrongPhase);
        };
        match attempt.context.compatibility {
            CompatibilityBoundary::PreMigration => {
                attempt.context.compatibility =
                    CompatibilityBoundary::CandidateMigrationInvoked(candidate);
            }
            CompatibilityBoundary::CandidateMigrationInvoked(existing) if existing == candidate => {
            }
            CompatibilityBoundary::CandidateMigrationInvoked(existing) => {
                return Err(TransitionError::CandidateMismatch {
                    expected: existing,
                    actual: candidate,
                });
            }
        }
        Ok(Transition {
            checkpoint: Self::Ready(attempt),
            after_persist: Vec::new(),
        })
    }

    /// # Errors
    /// Returns [`TransitionError::WrongPhase`] unless forward recovery is active.
    pub fn record_candidate_verification(
        self,
        verification: CandidateVerification,
    ) -> Result<Transition, TransitionError> {
        let Self::ForwardRecovery(mut attempt) = self else {
            return Err(TransitionError::WrongPhase);
        };
        attempt.verification = Some(verification);
        Ok(Transition {
            checkpoint: Self::ForwardRecovery(attempt),
            after_persist: Vec::new(),
        })
    }

    /// # Errors
    /// Returns an error unless closeout is required and acceptance names this attempt.
    pub fn accept_administrative_risk(
        self,
        acceptance: AdministrativeRiskAcceptance,
    ) -> Result<Transition, TransitionError> {
        let Self::AdministrativeCloseoutRequired(attempt) = self else {
            return Err(TransitionError::WrongPhase);
        };
        if acceptance.attempt_id != attempt.context.id {
            return Err(TransitionError::AcceptanceAttemptMismatch);
        }
        Ok(Transition {
            checkpoint: Self::AdministrativelyClosed(AdministrativelyClosed {
                context: attempt.context,
                resources: attempt.resources,
                tree: attempt.tree,
            }),
            after_persist: Vec::new(),
        })
    }

    #[must_use]
    pub const fn resource_block_reason(&self) -> Option<ResourceBlock> {
        match self {
            Self::ResourceBlocked(attempt) => Some(attempt.reason),
            Self::Active(_)
            | Self::Ready(_)
            | Self::Interrupted(_)
            | Self::ForwardRecovery(_)
            | Self::AdministrativeCloseoutRequired(_)
            | Self::AdministrativelyClosed(_) => None,
        }
    }

    #[must_use]
    pub const fn forward_candidate(&self) -> Option<RetainedCandidateIdentity> {
        match self {
            Self::ForwardRecovery(attempt) => Some(attempt.candidate),
            Self::Active(_)
            | Self::Ready(_)
            | Self::ResourceBlocked(_)
            | Self::Interrupted(_)
            | Self::AdministrativeCloseoutRequired(_)
            | Self::AdministrativelyClosed(_) => None,
        }
    }

    #[must_use]
    pub const fn candidate_verification(&self) -> Option<CandidateVerification> {
        match self {
            Self::ForwardRecovery(attempt) => attempt.verification,
            Self::Active(_)
            | Self::Ready(_)
            | Self::ResourceBlocked(_)
            | Self::Interrupted(_)
            | Self::AdministrativeCloseoutRequired(_)
            | Self::AdministrativelyClosed(_) => None,
        }
    }

    #[must_use]
    pub fn resource_evidence(&self) -> Option<&SustainedResourceEvidence> {
        match self {
            Self::Active(ActiveAttempt::Ownership(attempt))
            | Self::Interrupted(InterruptedAttempt {
                resume: ActiveAttempt::Ownership(attempt),
                ..
            }) => Some(&attempt.resources),
            Self::Ready(attempt) => Some(&attempt.resources),
            Self::AdministrativeCloseoutRequired(attempt) => Some(&attempt.resources),
            Self::AdministrativelyClosed(attempt) => Some(&attempt.resources),
            Self::Active(ActiveAttempt::ResourceSampling(_))
            | Self::Interrupted(InterruptedAttempt {
                resume: ActiveAttempt::ResourceSampling(_),
                ..
            })
            | Self::ResourceBlocked(_)
            | Self::ForwardRecovery(_) => None,
        }
    }

    #[must_use]
    pub fn ownership_tree(&self) -> Option<&OwnedProcessTree> {
        match self {
            Self::Active(ActiveAttempt::Ownership(attempt))
            | Self::Interrupted(InterruptedAttempt {
                resume: ActiveAttempt::Ownership(attempt),
                ..
            }) => Some(&attempt.tree),
            Self::Ready(attempt) => Some(&attempt.tree),
            Self::AdministrativeCloseoutRequired(attempt) => Some(&attempt.tree),
            Self::AdministrativelyClosed(attempt) => Some(&attempt.tree),
            Self::Active(ActiveAttempt::ResourceSampling(_))
            | Self::Interrupted(InterruptedAttempt {
                resume: ActiveAttempt::ResourceSampling(_),
                ..
            })
            | Self::ResourceBlocked(_)
            | Self::ForwardRecovery(_) => None,
        }
    }

    const fn context(&self) -> &AttemptContext {
        match self {
            Self::Active(active) => active.context(),
            Self::Ready(attempt) => &attempt.context,
            Self::ResourceBlocked(attempt) => &attempt.context,
            Self::Interrupted(attempt) => &attempt.context,
            Self::ForwardRecovery(attempt) => &attempt.context,
            Self::AdministrativeCloseoutRequired(attempt) => &attempt.context,
            Self::AdministrativelyClosed(attempt) => &attempt.context,
        }
    }

    fn set_revision(&mut self, revision: PersistenceRevision) {
        match self {
            Self::Active(active) => active.set_revision(revision),
            Self::Ready(attempt) => attempt.context.revision = Some(revision),
            Self::ResourceBlocked(attempt) => attempt.context.revision = Some(revision),
            Self::Interrupted(attempt) => {
                attempt.context.revision = Some(revision);
                attempt.resume.set_revision(revision);
            }
            Self::ForwardRecovery(attempt) => attempt.context.revision = Some(revision),
            Self::AdministrativeCloseoutRequired(attempt) => {
                attempt.context.revision = Some(revision);
            }
            Self::AdministrativelyClosed(attempt) => attempt.context.revision = Some(revision),
        }
    }

    fn into_ownership(self) -> Result<OwnershipAttempt, TransitionError> {
        match self {
            Self::Active(ActiveAttempt::Ownership(attempt)) => Ok(attempt),
            Self::Active(ActiveAttempt::ResourceSampling(_))
            | Self::Ready(_)
            | Self::ResourceBlocked(_)
            | Self::Interrupted(_)
            | Self::ForwardRecovery(_)
            | Self::AdministrativeCloseoutRequired(_)
            | Self::AdministrativelyClosed(_) => Err(TransitionError::WrongPhase),
        }
    }
}

impl ActiveAttempt {
    fn set_revision(&mut self, revision: PersistenceRevision) {
        match self {
            Self::ResourceSampling(attempt) => attempt.context.revision = Some(revision),
            Self::Ownership(attempt) => attempt.context.revision = Some(revision),
        }
    }

    const fn context(&self) -> &AttemptContext {
        match self {
            Self::ResourceSampling(attempt) => &attempt.context,
            Self::Ownership(attempt) => &attempt.context,
        }
    }
}

fn recovery_resume_for(phase: OwnershipPhase) -> Result<RecoveryResume, TransitionError> {
    match phase {
        OwnershipPhase::Discovery => Ok(RecoveryResume::Discovery),
        OwnershipPhase::AwaitingBirthEvidence => Ok(RecoveryResume::AwaitingBirthEvidence),
        OwnershipPhase::Cleaning => Ok(RecoveryResume::Cleaning),
        OwnershipPhase::Verifying => Ok(RecoveryResume::Verifying),
        OwnershipPhase::Recovering => Err(TransitionError::WrongPhase),
    }
}

fn recovery_effects(tree: &OwnedProcessTree) -> Vec<OsEffect> {
    let mut effects = observe_all(tree);
    effects.extend(
        tree.pending_child_scans()
            .iter()
            .copied()
            .map(|parent| OsEffect::ListChildren { parent }),
    );
    effects.extend(
        tree.unresolved()
            .values()
            .map(|unresolved| OsEffect::AcquireChildIdentity {
                discovery: unresolved.discovery,
            }),
    );
    effects
}

fn forward_recovery_transition(
    context: AttemptContext,
    candidate: RetainedCandidateIdentity,
) -> Transition {
    Transition {
        checkpoint: DeploymentAttempt::ForwardRecovery(ForwardRecoveryAttempt {
            context,
            candidate,
            verification: None,
        }),
        after_persist: vec![OsEffect::ReverifyRetainedCandidate { candidate }],
    }
}

fn observe_all(tree: &OwnedProcessTree) -> Vec<OsEffect> {
    tree.identities()
        .keys()
        .copied()
        .map(|identity| OsEffect::ObserveIdentity { identity })
        .collect()
}

fn to_administrative_closeout(attempt: OwnershipAttempt) -> Transition {
    Transition {
        checkpoint: DeploymentAttempt::AdministrativeCloseoutRequired(
            AdministrativeCloseoutRequired {
                context: attempt.context,
                resources: attempt.resources,
                tree: attempt.tree,
            },
        ),
        after_persist: Vec::new(),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransitionError {
    WrongPhase,
    NonIncreasingSample(NonIncreasingSampleSequence),
    UnexpectedSampleSequence {
        expected: u64,
        actual: u64,
    },
    UnknownParent(UnknownParent),
    UnknownChildScan(UnknownChildScan),
    UnknownDiscovery(UnknownDiscovery),
    UnknownOwnedIdentity(UnknownOwnedIdentity),
    UnresolvedOwnership,
    PendingChildScans,
    IncompleteReconciliation,
    MissingRecoveryResume,
    CandidateMismatch {
        expected: RetainedCandidateIdentity,
        actual: RetainedCandidateIdentity,
    },
    DeadlineNotReached,
    NoUnresolvedRisk,
    AcceptanceAttemptMismatch,
}
