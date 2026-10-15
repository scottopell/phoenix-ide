use std::{
    collections::{BTreeMap, BTreeSet},
    num::{NonZeroU32, NonZeroUsize},
};

use phoenix_deployment_controller::{
    AdministrativeRiskAcceptance, ArtifactDigest, AttemptId, AttemptStore, BirthToken,
    CandidateDiscovery, CandidateVerification, ChildIdentityObservation, CleanupOutcome,
    CommittedTransition, CompatibilityBoundary, ControllerOs, CpuContentionBasisPoints,
    DeploymentAttempt, DiscoveryId, FullCommitSha, IdentityObservation, MemoryPressure,
    MonotonicTick, ObservationFailure, OsEffect, OsQueryError, Phase, ProcessId, ProcessIdentity,
    ResourceCapacity, ResourceContention, ResourcePolicy, ResourceSample,
    RetainedCandidateIdentity, Signal, SignalOutcome, StableChildEvidence, Transition,
};

#[derive(Default)]
struct FakeStore {
    checkpoints: Vec<(AttemptId, Phase)>,
}

impl AttemptStore for FakeStore {
    type Error = std::convert::Infallible;

    fn persist(&mut self, checkpoint: &DeploymentAttempt) -> Result<(), Self::Error> {
        self.checkpoints
            .push((checkpoint.attempt_id(), checkpoint.phase()));
        Ok(())
    }
}

struct FailingStore;

impl AttemptStore for FailingStore {
    type Error = &'static str;

    fn persist(&mut self, _checkpoint: &DeploymentAttempt) -> Result<(), Self::Error> {
        Err("durable write failed")
    }
}

trait PersistForTest {
    fn persisted(self) -> CommittedTransition;
}

impl PersistForTest for Transition {
    fn persisted(self) -> CommittedTransition {
        let mut store = FakeStore::default();
        self.persist(&mut store)
            .expect("in-memory checkpoint persistence succeeds")
    }
}

fn persisted_effects(transition: &Transition) -> Vec<OsEffect> {
    transition.clone().persisted().after_persist().to_vec()
}

#[derive(Clone, Copy)]
struct FakeProcess {
    identity: ProcessIdentity,
    parent: Option<ProcessIdentity>,
}

struct FakeOs {
    processes: BTreeMap<ProcessId, FakeProcess>,
    unresolved_discoveries: BTreeSet<DiscoveryId>,
    signalled: Vec<ProcessIdentity>,
}

impl FakeOs {
    fn new(processes: impl IntoIterator<Item = FakeProcess>) -> Self {
        Self {
            processes: processes
                .into_iter()
                .map(|process| (process.identity.pid(), process))
                .collect(),
            unresolved_discoveries: BTreeSet::new(),
            signalled: Vec::new(),
        }
    }

    fn remove(&mut self, identity: ProcessIdentity) {
        if self
            .processes
            .get(&identity.pid())
            .is_some_and(|process| process.identity == identity)
        {
            self.processes.remove(&identity.pid());
        }
    }

    fn replace(&mut self, process: FakeProcess) {
        self.processes.insert(process.identity.pid(), process);
    }
}

impl ControllerOs for FakeOs {
    fn sample_resources(&mut self, sequence: u64) -> ResourceSample {
        ample_sample(sequence)
    }

    fn list_children(&mut self, parent: ProcessIdentity) -> Result<Vec<ProcessId>, OsQueryError> {
        Ok(self
            .processes
            .values()
            .filter(|process| process.parent == Some(parent))
            .map(|process| process.identity.pid())
            .collect())
    }

    fn acquire_child_identity(
        &mut self,
        discovery: CandidateDiscovery,
    ) -> ChildIdentityObservation {
        let Some(process) = self.processes.get(&discovery.hinted_pid()).copied() else {
            self.unresolved_discoveries.insert(discovery.id());
            return ChildIdentityObservation::Unresolved(ObservationFailure::NoSuchProcess);
        };

        if self.unresolved_discoveries.contains(&discovery.id())
            || process.parent != Some(discovery.verified_parent())
        {
            return ChildIdentityObservation::DifferentProcess(process.identity);
        }

        ChildIdentityObservation::Stable(
            StableChildEvidence::new(discovery, process.identity, discovery.verified_parent())
                .expect("fake child evidence is internally consistent"),
        )
    }

    fn observe_identity(&mut self, identity: ProcessIdentity) -> IdentityObservation {
        match self.processes.get(&identity.pid()) {
            Some(process) if process.identity == identity => IdentityObservation::SameIdentity {
                current_parent: process.parent,
            },
            Some(process) => IdentityObservation::PidReused(process.identity),
            None => IdentityObservation::ExactIdentityAbsent,
        }
    }

    fn signal_exact_identity(
        &mut self,
        identity: ProcessIdentity,
        _signal: Signal,
    ) -> SignalOutcome {
        match self.processes.get(&identity.pid()) {
            Some(process) if process.identity == identity => {
                self.signalled.push(identity);
                self.processes.remove(&identity.pid());
                SignalOutcome::Delivered
            }
            Some(_) => SignalOutcome::PidReused,
            None => SignalOutcome::ExactIdentityAlreadyAbsent,
        }
    }

    fn reverify_retained_candidate(
        &mut self,
        _candidate: RetainedCandidateIdentity,
    ) -> CandidateVerification {
        CandidateVerification::Exact
    }
}

#[test]
fn failed_checkpoint_write_does_not_produce_a_committed_transition() {
    let root = identity(5_000, 1_792_031_100, 1);
    let transition =
        DeploymentAttempt::start(AttemptId::new(90), MonotonicTick::new(30), policy(), root);
    let error = transition
        .persist(&mut FailingStore)
        .expect_err("OS effects require a successful checkpoint write");
    assert_eq!(error, "durable write failed");
}

#[test]
fn transient_child_esrch_before_birth_stays_unresolved_until_administrative_closeout() {
    let root = identity(5_188, 1_792_031_138, 344_344);
    let child = identity(5_189, 1_792_031_138, 400_001);
    let mut os = FakeOs::new([
        FakeProcess {
            identity: root,
            parent: None,
        },
        FakeProcess {
            identity: child,
            parent: Some(root),
        },
    ]);
    let state = admitted_attempt(root);

    let hinted = os.list_children(root).expect("fake enumeration succeeds");
    assert_eq!(hinted, vec![child.pid()]);
    let (transition, discovery) = state
        .child_discovered(root, child.pid())
        .expect("verified parent admits a discovery epoch");
    assert_eq!(
        persisted_effects(&transition),
        vec![OsEffect::AcquireChildIdentity { discovery }]
    );
    let (state, _) = transition.persisted().into_parts();

    os.remove(child);
    let observation = os.acquire_child_identity(discovery);
    assert_eq!(
        observation,
        ChildIdentityObservation::Unresolved(ObservationFailure::NoSuchProcess)
    );
    let (state, _) = state
        .child_observed(discovery, observation)
        .expect("ESRCH remains typed evidence")
        .persisted()
        .into_parts();
    assert_eq!(state.phase(), Phase::AwaitingBirthEvidence);

    let reused = identity(5_189, 1_792_031_140, 99);
    os.replace(FakeProcess {
        identity: reused,
        parent: Some(root),
    });
    let retry = state
        .retry_unresolved_discoveries()
        .expect("bounded reconciliation can retry");
    assert_eq!(
        persisted_effects(&retry),
        vec![OsEffect::AcquireChildIdentity { discovery }]
    );
    let (state, _) = retry.persisted().into_parts();
    let replacement_observation = os.acquire_child_identity(discovery);
    assert_eq!(
        replacement_observation,
        ChildIdentityObservation::DifferentProcess(reused)
    );
    let (state, _) = state
        .child_observed(discovery, replacement_observation)
        .expect("PID reuse cannot resolve the old discovery")
        .persisted()
        .into_parts();

    os.remove(root);
    let transition = state
        .deadline_reached(MonotonicTick::new(30))
        .expect("the absolute deadline ends reconciliation without claiming cleanup");
    assert_eq!(
        transition.checkpoint().phase(),
        Phase::AdministrativeCloseoutRequired
    );
    assert_eq!(
        transition.checkpoint().cleanup_outcome(),
        CleanupOutcome::UnresolvedRisk
    );
    assert!(persisted_effects(&transition).is_empty());
    let (state, _) = transition.persisted().into_parts();

    let closed = state
        .accept_administrative_risk(AdministrativeRiskAcceptance::for_attempt(AttemptId::new(1)))
        .expect("explicit administrative closeout is separate from cleanup proof");
    assert_eq!(closed.checkpoint().phase(), Phase::AdministrativelyClosed);
    assert_eq!(
        closed.checkpoint().cleanup_outcome(),
        CleanupOutcome::AdministrativeRiskAccepted
    );
    assert!(persisted_effects(&closed).is_empty());
    assert!(os.signalled.is_empty());
    assert_eq!(
        closed
            .checkpoint()
            .ownership_tree()
            .expect("closeout retains evidence")
            .unresolved()
            .len(),
        1
    );
}

#[test]
fn acquired_child_remains_owned_after_reparenting() {
    let root = identity(100, 10, 1);
    let child = identity(101, 10, 2);
    let unrelated_parent = identity(200, 9, 8);
    let mut os = FakeOs::new([
        FakeProcess {
            identity: root,
            parent: None,
        },
        FakeProcess {
            identity: child,
            parent: Some(root),
        },
    ]);
    let state = acquire_child(admitted_attempt(root), &mut os, root, child);
    os.replace(FakeProcess {
        identity: child,
        parent: Some(unrelated_parent),
    });

    let (mut state, effects) = state
        .ownership_scan_completed()
        .expect("all discoveries have stable identities")
        .persisted()
        .into_parts();
    state = apply_identity_observations(state, &effects, &mut os);
    let signals = state
        .request_owned_signals(Signal::Terminate)
        .expect("stable identities authorize bounded signals");
    assert!(
        persisted_effects(&signals).contains(&OsEffect::SignalExactIdentity {
            identity: child,
            signal: Signal::Terminate,
        })
    );

    let (mut state, effects) = signals.persisted().into_parts();
    apply_signal_effects(&effects, &mut os);
    let settled = state
        .cleanup_signals_settled()
        .expect("TERM phase settles into verification");
    (state, _) = settled.persisted().into_parts();
    let effects: Vec<_> = state
        .ownership_tree()
        .expect("tree remains available")
        .identities()
        .keys()
        .copied()
        .map(|identity| OsEffect::ObserveIdentity { identity })
        .collect();
    state = apply_identity_observations(state, &effects, &mut os);
    let ready = state
        .verification_completed(MonotonicTick::new(5))
        .expect("all exact identities are absent");
    assert_eq!(ready.checkpoint().phase(), Phase::ReadyForDisruption);
    assert!(os.signalled.contains(&child));
}

#[test]
fn pid_reuse_is_absence_of_owned_identity_not_signal_authority() {
    let root = identity(300, 20, 1);
    let child = identity(301, 20, 2);
    let replacement = identity(301, 21, 1);
    let mut os = FakeOs::new([
        FakeProcess {
            identity: root,
            parent: None,
        },
        FakeProcess {
            identity: child,
            parent: Some(root),
        },
    ]);
    let state = acquire_child(admitted_attempt(root), &mut os, root, child);
    os.remove(root);
    os.replace(FakeProcess {
        identity: replacement,
        parent: None,
    });

    let (mut state, effects) = state
        .ownership_scan_completed()
        .expect("stable ownership discovery completes")
        .persisted()
        .into_parts();
    state = apply_identity_observations(state, &effects, &mut os);
    let signals = state
        .request_owned_signals(Signal::Terminate)
        .expect("only currently matching identities can be signalled");
    assert!(!persisted_effects(&signals).iter().any(|effect| {
        matches!(
            effect,
            OsEffect::SignalExactIdentity { identity, .. } if *identity == replacement
        )
    }));
    assert!(persisted_effects(&signals).is_empty());
}

#[test]
fn malformed_pid_reuse_observation_cannot_launder_owned_identity_absence() {
    let root = identity(350, 25, 1);
    let unrelated = identity(999, 26, 1);
    let state = admitted_attempt(root);
    let (state, _) = state
        .ownership_scan_completed()
        .expect("empty child discovery enters cleanup")
        .persisted()
        .into_parts();
    let (state, _) = state
        .identity_observed(root, IdentityObservation::PidReused(unrelated))
        .expect("malformed adapter evidence remains typed uncertainty")
        .persisted()
        .into_parts();
    let closeout = state
        .deadline_reached(MonotonicTick::new(30))
        .expect("malformed evidence cannot yield verified cleanup");
    assert_eq!(
        closeout.checkpoint().phase(),
        Phase::AdministrativeCloseoutRequired
    );
}

#[test]
fn candidate_identities_accept_complete_uppercase_hex() {
    assert!(FullCommitSha::parse(&"A".repeat(40)).is_ok());
    assert!(ArtifactDigest::parse(&"F".repeat(64)).is_ok());
}

#[test]
fn recovery_reobserves_owned_identities_before_repeating_mutation() {
    let root = identity(400, 30, 1);
    let mut os = FakeOs::new([FakeProcess {
        identity: root,
        parent: None,
    }]);
    let state = admitted_attempt(root);
    let (state, _) = state
        .ownership_scan_completed()
        .expect("empty child discovery can enter cleanup")
        .persisted()
        .into_parts();
    let deadline = state.deadline();
    let (state, _) = state
        .interrupt()
        .expect("active cleanup interruption is durable")
        .persisted()
        .into_parts();
    assert_eq!(state.phase(), Phase::Interrupted);

    let recovered = state.recover().expect("pre-migration recovery reconciles");
    assert_eq!(recovered.checkpoint().phase(), Phase::Recovering);
    assert_eq!(recovered.checkpoint().deadline(), deadline);
    assert_eq!(
        persisted_effects(&recovered),
        vec![OsEffect::ObserveIdentity { identity: root }]
    );
    assert!(!persisted_effects(&recovered)
        .iter()
        .any(|effect| matches!(effect, OsEffect::SignalExactIdentity { .. })));

    let (state, effects) = recovered.persisted().into_parts();
    let state = apply_identity_observations(state, &effects, &mut os);
    let signals = state
        .request_owned_signals(Signal::Terminate)
        .expect("fresh identity match restores signal authority");
    assert_eq!(
        persisted_effects(&signals),
        vec![OsEffect::SignalExactIdentity {
            identity: root,
            signal: Signal::Terminate,
        }]
    );
}

#[test]
fn migration_invocation_recovery_is_exact_candidate_forward_only() {
    let root = identity(500, 40, 1);
    let candidate = RetainedCandidateIdentity {
        source_commit: FullCommitSha::parse(&"a".repeat(40)).expect("valid full SHA"),
        artifact_digest: ArtifactDigest::parse(&"b".repeat(64)).expect("valid digest"),
    };
    let state = admitted_attempt(root);
    let (state, _) = state
        .record_migration_invoked(candidate)
        .expect("migration invocation is durably fenced")
        .persisted()
        .into_parts();
    assert_eq!(
        state.compatibility_boundary(),
        CompatibilityBoundary::CandidateMigrationInvoked(candidate)
    );
    let (state, _) = state
        .interrupt()
        .expect("post-invocation interruption persists")
        .persisted()
        .into_parts();
    let recovered = state.recover().expect("forward recovery remains available");
    assert_eq!(recovered.checkpoint().phase(), Phase::ForwardRecovery);
    assert_eq!(
        persisted_effects(&recovered),
        vec![OsEffect::ReverifyRetainedCandidate { candidate }]
    );
}

#[test]
fn permission_uncertainty_reaches_bounded_closeout_without_signalling() {
    let root = identity(550, 45, 1);
    let state = admitted_attempt(root);
    let (state, _) = state
        .ownership_scan_completed()
        .expect("empty discovery enters cleanup")
        .persisted()
        .into_parts();
    let (state, _) = state
        .identity_observed(
            root,
            IdentityObservation::Unresolved(ObservationFailure::PermissionDenied),
        )
        .expect("permission uncertainty remains typed")
        .persisted()
        .into_parts();
    let transition = state
        .deadline_reached(MonotonicTick::new(30))
        .expect("uncertainty stops at the absolute deadline");
    assert_eq!(
        transition.checkpoint().phase(),
        Phase::AdministrativeCloseoutRequired
    );
    assert!(persisted_effects(&transition).is_empty());
}

#[test]
fn administrative_acceptance_is_bound_to_the_attempt() {
    let root = identity(575, 47, 1);
    let state = admitted_attempt(root);
    let (state, _) = state
        .ownership_scan_completed()
        .expect("empty discovery enters cleanup")
        .persisted()
        .into_parts();
    let (state, _) = state
        .identity_observed(
            root,
            IdentityObservation::Unresolved(ObservationFailure::PermissionDenied),
        )
        .expect("permission uncertainty remains typed")
        .persisted()
        .into_parts();
    let (state, _) = state
        .deadline_reached(MonotonicTick::new(30))
        .expect("uncertainty requires closeout")
        .persisted()
        .into_parts();
    let error = state
        .accept_administrative_risk(AdministrativeRiskAcceptance::for_attempt(AttemptId::new(
            999,
        )))
        .expect_err("another attempt's acceptance cannot close this evidence");
    assert_eq!(
        error,
        phoenix_deployment_controller::TransitionError::AcceptanceAttemptMismatch
    );
}

#[test]
fn contention_blocks_only_when_sustained_across_the_measurement_window() {
    let root = identity(590, 48, 1);
    let (mut state, _) =
        DeploymentAttempt::start(AttemptId::new(3), MonotonicTick::new(30), policy(), root)
            .persisted()
            .into_parts();
    let mut congested = ample_sample(0);
    congested.contention.cpu =
        CpuContentionBasisPoints::new(9_000).expect("valid contention measurement");
    (state, _) = state
        .record_resource_sample(congested)
        .expect("first measurement remains nonterminal")
        .persisted()
        .into_parts();
    for sequence in 1..3 {
        (state, _) = state
            .record_resource_sample(ample_sample(sequence))
            .expect("ordered measurement")
            .persisted()
            .into_parts();
    }
    assert_eq!(state.phase(), Phase::OwnershipDiscovery);

    let (mut state, _) =
        DeploymentAttempt::start(AttemptId::new(4), MonotonicTick::new(30), policy(), root)
            .persisted()
            .into_parts();
    for sequence in 0..3 {
        let mut congested = ample_sample(sequence);
        congested.contention.cpu =
            CpuContentionBasisPoints::new(9_000).expect("valid contention measurement");
        (state, _) = state
            .record_resource_sample(congested)
            .expect("three sustained congested measurements")
            .persisted()
            .into_parts();
    }
    assert_eq!(state.phase(), Phase::ResourceBlocked);
}

#[test]
fn admission_requires_consecutive_capacity_and_contention_measurements() {
    let root = identity(600, 50, 1);
    let start = DeploymentAttempt::start(AttemptId::new(2), MonotonicTick::new(30), policy(), root);
    let (mut state, _) = start.persisted().into_parts();

    for sequence in 0..2 {
        let transition = state
            .record_resource_sample(ample_sample(sequence))
            .expect("ordered sample");
        assert_eq!(transition.checkpoint().phase(), Phase::ResourceSampling);
        (state, _) = transition.persisted().into_parts();
    }
    let admitted = state
        .record_resource_sample(ample_sample(2))
        .expect("third sustained sample admits");
    assert_eq!(admitted.checkpoint().phase(), Phase::OwnershipDiscovery);
}

fn admitted_attempt(root: ProcessIdentity) -> DeploymentAttempt {
    let (mut state, _) =
        DeploymentAttempt::start(AttemptId::new(1), MonotonicTick::new(30), policy(), root)
            .persisted()
            .into_parts();
    for sequence in 0..3 {
        (state, _) = state
            .record_resource_sample(ample_sample(sequence))
            .expect("ample ordered resource sample")
            .persisted()
            .into_parts();
    }
    assert_eq!(state.phase(), Phase::OwnershipDiscovery);
    state
}

fn acquire_child(
    state: DeploymentAttempt,
    os: &mut FakeOs,
    parent: ProcessIdentity,
    child: ProcessIdentity,
) -> DeploymentAttempt {
    let (transition, discovery) = state
        .child_discovered(parent, child.pid())
        .expect("known parent can discover child");
    let (state, _) = transition.persisted().into_parts();
    let observation = os.acquire_child_identity(discovery);
    let (state, _) = state
        .child_observed(discovery, observation)
        .expect("stable child observation is accepted")
        .persisted()
        .into_parts();
    state
}

fn apply_identity_observations(
    mut state: DeploymentAttempt,
    effects: &[OsEffect],
    os: &mut FakeOs,
) -> DeploymentAttempt {
    for effect in effects {
        if let OsEffect::ObserveIdentity { identity } = effect {
            let observation = os.observe_identity(*identity);
            (state, _) = state
                .identity_observed(*identity, observation)
                .expect("effect names an owned identity")
                .persisted()
                .into_parts();
        }
    }
    state
}

fn apply_signal_effects(effects: &[OsEffect], os: &mut FakeOs) {
    for effect in effects {
        if let OsEffect::SignalExactIdentity { identity, signal } = effect {
            os.signal_exact_identity(*identity, *signal);
        }
    }
}

fn policy() -> ResourcePolicy {
    ResourcePolicy::new(
        NonZeroUsize::new(3).expect("nonzero"),
        16 * 1024 * 1024 * 1024,
        40 * 1024 * 1024 * 1024,
        CpuContentionBasisPoints::new(2_500).expect("valid basis points"),
        MemoryPressure::Nominal,
    )
}

fn ample_sample(sequence: u64) -> ResourceSample {
    ResourceSample {
        sequence,
        capacity: ResourceCapacity {
            available_memory_bytes: 24 * 1024 * 1024 * 1024,
            available_storage_bytes: 80 * 1024 * 1024 * 1024,
        },
        contention: ResourceContention {
            cpu: CpuContentionBasisPoints::new(1_000).expect("valid basis points"),
            memory_pressure: MemoryPressure::Nominal,
        },
    }
}

fn identity(pid: u32, seconds: u64, nanos: u32) -> ProcessIdentity {
    ProcessIdentity::new(
        ProcessId::new(NonZeroU32::new(pid).expect("test PID is nonzero")),
        BirthToken::new(seconds, nanos).expect("valid birth token"),
    )
}
