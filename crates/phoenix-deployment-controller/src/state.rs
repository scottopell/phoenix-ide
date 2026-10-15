use crate::{
    candidate::RetainedCandidateIdentity,
    identity::{
        CandidateDiscovery, ChildIdentityObservation, IdentityObservation, ProcessId,
        ProcessIdentity,
    },
    os::{CandidateVerification, OsEffect, Signal},
    ownership::{
        CleanupReadiness, OwnedProcessTree, UnknownDiscovery, UnknownOwnedIdentity, UnknownParent,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CleanupOutcome {
    NotStarted,
    InProgress,
    VerifiedClean,
    UnresolvedRisk,
    AdministrativeRiskAccepted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct AttemptContext {
    id: AttemptId,
    deadline: MonotonicTick,
    compatibility: CompatibilityBoundary,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourceSamplingAttempt {
    context: AttemptContext,
    root: ProcessIdentity,
    next_sample_sequence: u64,
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OwnershipAttempt {
    context: AttemptContext,
    resources: SustainedResourceEvidence,
    tree: OwnedProcessTree,
    phase: OwnershipPhase,
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Transition {
    checkpoint: DeploymentAttempt,
    after_persist: Vec<OsEffect>,
}

impl Transition {
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
                deadline,
                compatibility: CompatibilityBoundary::PreMigration,
            },
            root,
            next_sample_sequence: 1,
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
        let assessment = attempt
            .window
            .record(sample)
            .map_err(TransitionError::NonIncreasingSample)?;
        match assessment {
            ResourceAssessment::NeedMoreSamples { .. } => {
                let sequence = attempt.next_sample_sequence;
                attempt.next_sample_sequence = attempt.next_sample_sequence.saturating_add(1);
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
        let stable_identity = match observation {
            ChildIdentityObservation::Stable(evidence) => Some(evidence.identity()),
            ChildIdentityObservation::Unresolved(_)
            | ChildIdentityObservation::DifferentProcess(_) => None,
        };
        attempt
            .tree
            .record_child_observation(discovery, observation)
            .map_err(TransitionError::UnknownDiscovery)?;
        let after_persist = if let Some(identity) = stable_identity {
            attempt.phase = OwnershipPhase::Discovery;
            vec![OsEffect::ListChildren { parent: identity }]
        } else {
            attempt.phase = OwnershipPhase::AwaitingBirthEvidence;
            Vec::new()
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
    /// Returns [`TransitionError::WrongPhase`] unless ownership discovery is active.
    pub fn ownership_scan_completed(self) -> Result<Transition, TransitionError> {
        let mut attempt = self.into_ownership()?;
        if !matches!(
            attempt.phase,
            OwnershipPhase::Discovery | OwnershipPhase::AwaitingBirthEvidence
        ) {
            return Err(TransitionError::WrongPhase);
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
    pub fn request_owned_signals(self, signal: Signal) -> Result<Transition, TransitionError> {
        let attempt = self.into_ownership()?;
        if !matches!(
            attempt.phase,
            OwnershipPhase::Cleaning | OwnershipPhase::Recovering
        ) {
            return Err(TransitionError::WrongPhase);
        }
        if !attempt.tree.unresolved().is_empty() {
            return Err(TransitionError::UnresolvedOwnership);
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
    /// Returns [`TransitionError::WrongPhase`] unless verification or recovery is active.
    pub fn verification_completed(self, now: MonotonicTick) -> Result<Transition, TransitionError> {
        let mut attempt = self.into_ownership()?;
        if !matches!(
            attempt.phase,
            OwnershipPhase::Verifying | OwnershipPhase::Recovering
        ) {
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
    /// Returns [`TransitionError::WrongPhase`] unless the attempt is interrupted.
    pub fn recover(self) -> Result<Transition, TransitionError> {
        let Self::Interrupted(interrupted) = self else {
            return Err(TransitionError::WrongPhase);
        };
        match interrupted.context.compatibility {
            CompatibilityBoundary::CandidateMigrationInvoked(candidate) => Ok(Transition {
                checkpoint: Self::ForwardRecovery(ForwardRecoveryAttempt {
                    context: interrupted.context,
                    candidate,
                    verification: None,
                }),
                after_persist: vec![OsEffect::ReverifyRetainedCandidate { candidate }],
            }),
            CompatibilityBoundary::PreMigration => match interrupted.resume {
                ActiveAttempt::ResourceSampling(attempt) => {
                    let sequence = attempt.next_sample_sequence;
                    Ok(Transition {
                        checkpoint: Self::Active(ActiveAttempt::ResourceSampling(attempt)),
                        after_persist: vec![OsEffect::SampleResources { sequence }],
                    })
                }
                ActiveAttempt::Ownership(mut attempt) => {
                    attempt.phase = OwnershipPhase::Recovering;
                    attempt.tree.mark_all_for_reconciliation();
                    let effects = observe_all(&attempt.tree);
                    Ok(Transition {
                        checkpoint: Self::Active(ActiveAttempt::Ownership(attempt)),
                        after_persist: effects,
                    })
                }
            },
        }
    }

    /// # Errors
    /// Returns [`TransitionError::WrongPhase`] when invocation cannot be recorded safely.
    pub fn record_migration_invoked(
        mut self,
        candidate: RetainedCandidateIdentity,
    ) -> Result<Transition, TransitionError> {
        match &mut self {
            Self::Active(active) => {
                active.context_mut().compatibility =
                    CompatibilityBoundary::CandidateMigrationInvoked(candidate);
            }
            Self::Ready(attempt) => {
                attempt.context.compatibility =
                    CompatibilityBoundary::CandidateMigrationInvoked(candidate);
            }
            Self::Interrupted(_)
            | Self::ResourceBlocked(_)
            | Self::ForwardRecovery(_)
            | Self::AdministrativeCloseoutRequired(_)
            | Self::AdministrativelyClosed(_) => return Err(TransitionError::WrongPhase),
        }
        Ok(Transition {
            checkpoint: self,
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
    const fn context(&self) -> &AttemptContext {
        match self {
            Self::ResourceSampling(attempt) => &attempt.context,
            Self::Ownership(attempt) => &attempt.context,
        }
    }

    const fn context_mut(&mut self) -> &mut AttemptContext {
        match self {
            Self::ResourceSampling(attempt) => &mut attempt.context,
            Self::Ownership(attempt) => &mut attempt.context,
        }
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
    UnknownParent(UnknownParent),
    UnknownDiscovery(UnknownDiscovery),
    UnknownOwnedIdentity(UnknownOwnedIdentity),
    UnresolvedOwnership,
    DeadlineNotReached,
    NoUnresolvedRisk,
    AcceptanceAttemptMismatch,
}
