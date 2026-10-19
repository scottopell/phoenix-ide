//! Deterministic `SQLite`-backed policy fixtures: no native adapter, process, or clock.
use std::{
    cell::RefCell,
    collections::BTreeMap,
    num::{NonZeroU32, NonZeroUsize},
    rc::Rc,
};

use phoenix_deployment_controller::{
    AckDisposition, ArtifactDigest, AttemptId, AttemptStore, BirthToken, CandidateDiscovery,
    CandidateVerification, ChildIdentityObservation, ChildSlotId, ChildSlotObservation,
    CleanupOutcome, CommittedTransition, ControllerOs, ControllerSession, ControllerSqliteStore,
    ControllerStoreError, CpuContentionBasisPoints, DeploymentAttempt, DiscoveryGap, DispatchKey,
    DispatchTicket, EffectExecutor, ExecutedEffect, FullCommitSha, Guardian, GuardianError,
    GuardianExecutor, GuardianLoopError, GuardianState, IdentityObservation, LoadedAttempt,
    LoopError, LoopFailure, LoopFenceReason, LoopProgress, LoopStatus, LoopSummary, MemoryPressure,
    MonotonicTick, ObservationFailure, OsEffect, OsQueryError, OwnedIdentityStatus, ParkReason,
    PersistenceRevision, Phase, PolicyLoop, ProcessId, ProcessIdentity, RecoveryDisposition,
    ResourceCapacity, ResourceContention, ResourceObservationFailure, ResourcePolicy,
    ResourceSample, RetainedCandidateIdentity, Signal, SignalOutcome, StableChildEvidence,
    Transition, TransitionError, MAX_OWNED_SIGNAL_BATCH, MAX_RETAINED_RESULTS,
};

// This trusted test store records the evidence supplied to persistence; it never
// obtains a live checkpoint from PolicyLoop's diagnostic interface.
struct TestStore {
    inner: ControllerSqliteStore,
    evidence: Option<DeploymentAttempt>,
    fail: bool,
}

impl TestStore {
    fn in_memory() -> Result<Self, ControllerStoreError> {
        Ok(Self {
            inner: ControllerSqliteStore::in_memory()?,
            evidence: None,
            fail: false,
        })
    }

    fn evidence(&self) -> &DeploymentAttempt {
        self.evidence.as_ref().unwrap()
    }

    fn load_attempt(
        &mut self,
        id: AttemptId,
    ) -> Result<Option<LoadedAttempt>, ControllerStoreError> {
        self.inner.load_attempt(id)
    }
}

impl AttemptStore for TestStore {
    type Error = ControllerStoreError;

    fn persist(
        &mut self,
        checkpoint: &DeploymentAttempt,
    ) -> Result<PersistenceRevision, Self::Error> {
        if self.fail {
            return Err(ControllerStoreError::RevisionConflict);
        }
        let revision = self.inner.persist(checkpoint)?;
        self.evidence = Some(checkpoint.clone());
        Ok(revision)
    }
}

fn identity(pid: u32) -> ProcessIdentity {
    ProcessIdentity::new(
        ProcessId::new(NonZeroU32::new(pid).unwrap()),
        BirthToken::new(u64::from(pid) + 100, 1).unwrap(),
    )
}

fn root() -> ProcessIdentity {
    identity(42)
}

fn policy(count: usize) -> ResourcePolicy {
    ResourcePolicy::new(
        NonZeroUsize::new(count).unwrap(),
        100,
        200,
        CpuContentionBasisPoints::new(5_000).unwrap(),
        MemoryPressure::Warning,
    )
}

fn sample(sequence: u64) -> ResourceSample {
    ResourceSample {
        sequence,
        capacity: ResourceCapacity {
            available_memory_bytes: 10_000,
            available_storage_bytes: 20_000,
        },
        contention: ResourceContention {
            cpu: CpuContentionBasisPoints::new(10).unwrap(),
            memory_pressure: MemoryPressure::Nominal,
        },
    }
}

fn start(id: u128, count: usize, root: ProcessIdentity) -> Transition {
    DeploymentAttempt::start(
        AttemptId::new(id),
        MonotonicTick::new(100),
        policy(count),
        root,
    )
}

fn commit(store: &mut TestStore, transition: Transition) -> DeploymentAttempt {
    transition.persist(store).unwrap().into_parts().0
}

fn discovery(store: &mut TestStore, id: u128, root: ProcessIdentity) -> CommittedTransition {
    let state = commit(store, start(id, 1, root));
    state
        .record_resource_sample(sample(0))
        .unwrap()
        .persist(store)
        .unwrap()
}

fn cleaning_transition(store: &mut TestStore, id: u128, children: &[u32]) -> Transition {
    let state = discovery(store, id, root()).into_parts().0;
    let listed = state
        .record_child_list(
            root(),
            children.iter().map(|pid| identity(*pid).pid()).collect(),
        )
        .unwrap()
        .persist(store)
        .unwrap();
    let discoveries = listed
        .after_persist()
        .iter()
        .map(|effect| {
            let OsEffect::AcquireChildIdentity { discovery } = effect else {
                panic!("expected child-identity fixture effect");
            };
            *discovery
        })
        .collect::<Vec<_>>();
    let mut state = listed.into_parts().0;
    for discovery in discoveries {
        let child = identity(discovery.hinted_pid().get());
        state = commit(
            store,
            state
                .child_observed(
                    discovery,
                    ChildIdentityObservation::Stable(
                        StableChildEvidence::new(discovery, child, root()).unwrap(),
                    ),
                )
                .unwrap(),
        );
        state = commit(store, state.record_child_list(child, Vec::new()).unwrap());
    }
    state.ownership_scan_completed().unwrap()
}

fn ready(store: &mut TestStore, id: u128) -> DeploymentAttempt {
    let transition = cleaning_transition(store, id, &[]);
    let state = commit(store, transition);
    let state = commit(store, state.cleanup_signals_settled().unwrap());
    let state = commit(
        store,
        state
            .identity_observed(root(), IdentityObservation::ExactIdentityAbsent)
            .unwrap(),
    );
    commit(
        store,
        state
            .verification_completed(MonotonicTick::new(99))
            .unwrap(),
    )
}

fn candidate() -> RetainedCandidateIdentity {
    RetainedCandidateIdentity {
        source_commit: FullCommitSha::parse(&"ef".repeat(20)).unwrap(),
        artifact_digest: ArtifactDigest::parse(&"ab".repeat(32)).unwrap(),
    }
}

struct FakeOs {
    resource_failure: Option<ResourceObservationFailure>,
    children: Result<Vec<ProcessId>, OsQueryError>,
    child_failure: Option<ObservationFailure>,
    observation: IdentityObservation,
    signal: SignalOutcome,
    calls: Vec<OsEffect>,
}

impl Default for FakeOs {
    fn default() -> Self {
        Self {
            resource_failure: None,
            children: Ok(Vec::new()),
            child_failure: None,
            observation: IdentityObservation::SameIdentity {
                current_parent: None,
            },
            signal: SignalOutcome::Delivered,
            calls: Vec::new(),
        }
    }
}

impl ControllerOs for FakeOs {
    fn sample_resources(
        &mut self,
        sequence: u64,
    ) -> Result<ResourceSample, ResourceObservationFailure> {
        self.calls.push(OsEffect::SampleResources { sequence });
        self.resource_failure
            .map_or_else(|| Ok(sample(sequence)), Err)
    }

    fn list_children(&mut self, parent: ProcessIdentity) -> Result<Vec<ProcessId>, OsQueryError> {
        self.calls.push(OsEffect::ListChildren { parent });
        self.children.clone()
    }

    fn acquire_child_identity(
        &mut self,
        discovery: CandidateDiscovery,
    ) -> ChildIdentityObservation {
        self.calls
            .push(OsEffect::AcquireChildIdentity { discovery });
        match self.child_failure {
            Some(failure) => ChildIdentityObservation::Unresolved(failure),
            None => ChildIdentityObservation::Stable(
                StableChildEvidence::new(
                    discovery,
                    identity(discovery.hinted_pid().get()),
                    discovery.verified_parent(),
                )
                .unwrap(),
            ),
        }
    }

    fn observe_identity(&mut self, identity: ProcessIdentity) -> IdentityObservation {
        self.calls.push(OsEffect::ObserveIdentity { identity });
        self.observation
    }

    fn signal_exact_identity(
        &mut self,
        identity: ProcessIdentity,
        signal: Signal,
    ) -> SignalOutcome {
        self.calls
            .push(OsEffect::SignalExactIdentity { identity, signal });
        self.signal
    }

    fn reverify_retained_candidate(
        &mut self,
        candidate: RetainedCandidateIdentity,
    ) -> CandidateVerification {
        self.calls
            .push(OsEffect::ReverifyRetainedCandidate { candidate });
        CandidateVerification::Exact
    }
}

fn execute(os: &mut FakeOs, ticket: DispatchTicket) -> ExecutedEffect {
    EffectExecutor::new(os).execute(ticket)
}

fn assert_durable(store: &mut TestStore, policy: &PolicyLoop) {
    let summary = policy.summary();
    let loaded = store.load_attempt(summary.attempt_id).unwrap().unwrap();
    assert_eq!(loaded.persistence_revision(), summary.revision);
    assert_eq!(loaded.phase(), summary.phase);
    assert_eq!(loaded.cleanup_outcome(), store.evidence().cleanup_outcome());
}

fn reload_interrupted(store: &mut TestStore, id: u128) -> DeploymentAttempt {
    let loaded = store.load_attempt(AttemptId::new(id)).unwrap().unwrap();
    let RecoveryDisposition::Checkpoint(transition) = loaded.prepare_recovery().unwrap() else {
        panic!("expected active recovery evidence");
    };
    commit(store, *transition)
}

fn loaded_revision(store: &mut TestStore, id: u128) -> PersistenceRevision {
    store
        .load_attempt(AttemptId::new(id))
        .unwrap()
        .unwrap()
        .persistence_revision()
}

fn commit_root_termination(
    store: &mut TestStore,
    recovered_state: DeploymentAttempt,
) -> CommittedTransition {
    let state = commit(
        store,
        recovered_state.record_resource_sample(sample(0)).unwrap(),
    );
    let state = commit(store, state.child_scan_completed(root()).unwrap());
    let state = commit(store, state.ownership_scan_completed().unwrap());
    let state = commit(
        store,
        state
            .identity_observed(
                root(),
                IdentityObservation::SameIdentity {
                    current_parent: None,
                },
            )
            .unwrap(),
    );
    state
        .request_owned_signals(Signal::Terminate, MonotonicTick::new(1))
        .unwrap()
        .persist(store)
        .unwrap()
}

fn apply_observation_round(
    policy: &mut PolicyLoop,
    store: &mut TestStore,
    os: &mut FakeOs,
    count: usize,
) {
    let tickets = (0..count)
        .map(|_| policy.issue_next().unwrap())
        .collect::<Vec<_>>();
    assert!(policy.issue_next().is_none());
    for ticket in tickets {
        assert!(matches!(ticket.effect(), OsEffect::ObserveIdentity { .. }));
        let completion = execute(os, ticket);
        assert!(matches!(
            policy.apply(completion, store).unwrap(),
            LoopProgress::Applied { .. }
        ));
        assert_durable(store, policy);
    }
}

#[test]
fn child_list_atomic_all_birth_obligations_canonical() {
    let mut store = TestStore::in_memory().unwrap();
    let state = discovery(&mut store, 1, root()).into_parts().0;
    let revision = state.persistence_revision().unwrap();
    assert!(matches!(
        state.clone().record_child_list(identity(999), Vec::new()),
        Err(TransitionError::UnknownChildScan(_))
    ));
    let committed = state
        .record_child_list(
            root(),
            vec![identity(60).pid(), identity(50).pid(), identity(60).pid()],
        )
        .unwrap()
        .persist(&mut store)
        .unwrap();
    assert_eq!(
        committed.checkpoint().persistence_revision(),
        revision.checked_next()
    );
    assert_eq!(committed.checkpoint().phase(), Phase::AwaitingBirthEvidence);
    let tree = committed.checkpoint().ownership_tree().unwrap();
    assert!(tree.pending_child_scans().is_empty());
    assert_eq!(tree.identities().len(), 1);
    assert_eq!(tree.unresolved().len(), 2);
    let expected = [50, 60]
        .into_iter()
        .enumerate()
        .map(|(ordinal, pid)| OsEffect::AcquireChildIdentity {
            discovery: CandidateDiscovery::new(
                phoenix_deployment_controller::DiscoveryId::new(u64::try_from(ordinal).unwrap()),
                identity(pid).pid(),
                root(),
            ),
        })
        .collect::<Vec<_>>();
    assert_eq!(committed.after_persist(), expected);
    assert!(tree
        .unresolved()
        .values()
        .all(|entry| entry.gap == DiscoveryGap::NotYetObserved));
    assert!(matches!(
        committed
            .checkpoint()
            .clone()
            .record_child_list(root(), Vec::new()),
        Err(TransitionError::UnknownChildScan(_))
    ));
    let saved_tree = tree.clone();
    let mut policy = PolicyLoop::from_committed(committed);
    for expected in expected {
        assert_eq!(policy.issue_next().unwrap().effect(), &expected);
    }
    assert!(policy.issue_next().is_none());
    let loaded = reload_interrupted(&mut store, 1);
    assert_eq!(loaded.ownership_tree(), Some(&saved_tree));
}

#[test]
fn atomic_child_list_accepts_only_discovery_phases_and_preserves_recovery() {
    let mut store = TestStore::in_memory().unwrap();
    let sampling = commit(&mut store, start(2, 1, root()));
    assert_eq!(
        sampling.record_child_list(root(), Vec::new()),
        Err(TransitionError::WrongPhase)
    );
    let state = discovery(&mut store, 3, root()).into_parts().0;
    let (transition, epoch) = state.child_discovered(root(), identity(50).pid()).unwrap();
    let state = commit(&mut store, transition);
    let state = commit(
        &mut store,
        state
            .child_observed(
                epoch,
                ChildIdentityObservation::Unresolved(ObservationFailure::NoSuchProcess),
            )
            .unwrap(),
    );
    assert_eq!(state.phase(), Phase::AwaitingBirthEvidence);
    let state = commit(
        &mut store,
        state
            .record_child_list(root(), vec![identity(60).pid()])
            .unwrap(),
    );
    assert_eq!(state.phase(), Phase::AwaitingBirthEvidence);
    assert_eq!(state.ownership_tree().unwrap().unresolved().len(), 2);

    let state = discovery(&mut store, 4, root()).into_parts().0;
    let state = commit(&mut store, state.interrupt().unwrap());
    let state = commit(&mut store, state.recover().unwrap());
    let state = commit(
        &mut store,
        state
            .record_child_list(root(), vec![identity(50).pid()])
            .unwrap(),
    );
    assert_eq!(state.phase(), Phase::Recovering);
    assert_eq!(state.deadline(), MonotonicTick::new(100));
    assert!(state
        .ownership_tree()
        .unwrap()
        .pending_child_scans()
        .is_empty());
    assert_eq!(state.ownership_tree().unwrap().unresolved().len(), 1);

    let transition = cleaning_transition(&mut store, 5, &[]);
    let state = commit(&mut store, transition);
    assert_eq!(
        state.record_child_list(root(), Vec::new()),
        Err(TransitionError::WrongPhase)
    );
}

#[test]
fn adopted_sibling_and_empty_child_list_preserve_unresolved_birth_phase() {
    let mut store = TestStore::in_memory().unwrap();
    let state = discovery(&mut store, 79, root()).into_parts().0;
    let listed = state
        .record_child_list(root(), vec![identity(50).pid(), identity(60).pid()])
        .unwrap()
        .persist(&mut store)
        .unwrap();
    let discoveries = listed
        .after_persist()
        .iter()
        .map(|effect| {
            let OsEffect::AcquireChildIdentity { discovery } = effect else {
                panic!("birth probe");
            };
            *discovery
        })
        .collect::<Vec<_>>();
    let state = commit(
        &mut store,
        listed
            .into_parts()
            .0
            .child_observed(
                discoveries[0],
                ChildIdentityObservation::Unresolved(ObservationFailure::NoSuchProcess),
            )
            .unwrap(),
    );
    let state = commit(
        &mut store,
        state
            .child_observed(
                discoveries[1],
                ChildIdentityObservation::Stable(
                    StableChildEvidence::new(discoveries[1], identity(60), root()).unwrap(),
                ),
            )
            .unwrap(),
    );
    assert_eq!(state.phase(), Phase::AwaitingBirthEvidence);
    let state = commit(
        &mut store,
        state.record_child_list(identity(60), Vec::new()).unwrap(),
    );
    assert_eq!(state.phase(), Phase::AwaitingBirthEvidence);
    assert_eq!(state.ownership_tree().unwrap().unresolved().len(), 1);
    assert!(state
        .ownership_tree()
        .unwrap()
        .pending_child_scans()
        .is_empty());
}

#[test]
fn explicit_resource_steps_and_unresolved_birth_never_fabricate_cleanup() {
    let mut store = TestStore::in_memory().unwrap();
    let mut policy = PolicyLoop::from_committed(start(6, 2, root()).persist(&mut store).unwrap());
    let mut os = FakeOs::default();
    for sequence in 0..2 {
        let ticket = policy.issue_next().unwrap();
        assert_eq!(ticket.effect(), &OsEffect::SampleResources { sequence });
        let before = policy.summary().revision;
        assert!(policy.issue_next().is_none());
        policy.apply(execute(&mut os, ticket), &mut store).unwrap();
        assert_eq!(Some(policy.summary().revision), before.checked_next());
        assert_durable(&mut store, &policy);
    }
    assert_eq!(policy.summary().phase, Phase::OwnershipDiscovery);
    assert_eq!(
        store.evidence().resource_evidence().unwrap().samples(),
        &[sample(0), sample(1)]
    );
    os.children = Ok(vec![identity(50).pid()]);
    let ticket = policy.issue_next().unwrap();
    assert_eq!(ticket.effect(), &OsEffect::ListChildren { parent: root() });
    policy.apply(execute(&mut os, ticket), &mut store).unwrap();
    let ticket = policy.issue_next().unwrap();
    let OsEffect::AcquireChildIdentity { discovery: epoch } = ticket.effect() else {
        panic!("expected the durably recorded discovery");
    };
    let epoch = *epoch;
    os.child_failure = Some(ObservationFailure::NoSuchProcess);
    policy.apply(execute(&mut os, ticket), &mut store).unwrap();
    assert_eq!(policy.summary().phase, Phase::AwaitingBirthEvidence);
    assert_eq!(
        store.evidence().ownership_tree().unwrap().unresolved()[&epoch.id()].gap,
        DiscoveryGap::Observation(ObservationFailure::NoSuchProcess)
    );
    assert!(
        policy.issue_next().is_none(),
        "no inferred birth-evidence retry"
    );
    let progress = policy
        .deadline_reached(MonotonicTick::new(100), &mut store)
        .unwrap();
    assert!(matches!(
        progress,
        LoopProgress::GuardianMustFence {
            reason: LoopFenceReason::DeadlineReached,
            ..
        }
    ));
    assert_eq!(
        progress.guardian_fence(),
        Some(LoopFenceReason::DeadlineReached)
    );
    assert!(progress.acknowledge_keys().next().is_none());
    assert_eq!(
        policy.summary().phase,
        Phase::AdministrativeCloseoutRequired
    );
    assert_eq!(
        store.evidence().cleanup_outcome(),
        CleanupOutcome::UnresolvedRisk
    );
    assert!(os
        .calls
        .iter()
        .all(|effect| !matches!(effect, OsEffect::SignalExactIdentity { .. })));
    assert_durable(&mut store, &policy);
}

#[test]
fn borrowed_child_list_ack_requires_the_complete_durable_checkpoint() {
    let mut store = TestStore::in_memory().unwrap();
    let mut policy = PolicyLoop::from_committed(discovery(&mut store, 80, root()));
    let mut os = FakeOs {
        children: Ok(vec![identity(50).pid(), identity(60).pid()]),
        ..FakeOs::default()
    };
    let ticket = policy.issue_next().unwrap();
    let retained = execute(&mut os, ticket);
    let before = policy.summary().revision;
    assert_eq!(
        store
            .load_attempt(AttemptId::new(80))
            .unwrap()
            .unwrap()
            .persistence_revision(),
        before
    );
    let progress = policy.apply_retained(&retained, &mut store).unwrap();
    assert!(matches!(
        progress,
        LoopProgress::Applied {
            ack: AckDisposition::AcknowledgeNow,
            ..
        }
    ));
    assert_eq!(
        progress.acknowledge_keys().collect::<Vec<_>>(),
        vec![retained.key()]
    );
    assert_eq!(Some(policy.summary().revision), before.checked_next());
    assert_durable(&mut store, &policy);
    assert_eq!(
        store
            .evidence()
            .ownership_tree()
            .unwrap()
            .unresolved()
            .len(),
        2
    );
    let phoenix_deployment_controller::EffectCompletion::ListChildren {
        result: Ok(pids), ..
    } = retained.completion()
    else {
        panic!("expected retained child listing");
    };
    assert_eq!(pids, &[identity(50).pid(), identity(60).pid()]);
    let summary = policy.summary();
    assert!(matches!(
        policy.apply_retained(&retained, &mut store),
        Err(LoopError::NotIssued(_))
    ));
    assert_eq!(policy.summary(), summary);
}

#[test]
fn held_raw_ticket_survives_interrupt_but_progress_requires_guardian_fencing() {
    let mut store = TestStore::in_memory().unwrap();
    let mut policy = PolicyLoop::from_committed(discovery(&mut store, 81, root()));
    let held = policy.issue_next().unwrap();
    let old_key = held.key();
    let progress = policy.interrupt(&mut store).unwrap();
    assert!(matches!(
        progress,
        LoopProgress::GuardianMustFence {
            phase: Phase::Interrupted,
            reason: LoopFenceReason::Interrupted,
            ..
        }
    ));
    assert_eq!(
        progress.guardian_fence(),
        Some(LoopFenceReason::Interrupted)
    );
    assert!(progress.acknowledge_keys().next().is_none());
    assert_durable(&mut store, &policy);
    assert!(policy.issue_next().is_none());
    // A raw executor can still consume the external ticket; the guardian bridge
    // must fence its session instead of treating loop retirement as revocation.
    let mut os = FakeOs::default();
    let old = execute(&mut os, held);
    assert_eq!(old.key(), old_key);
    assert_eq!(os.calls.len(), 1);
    assert!(matches!(
        policy.apply_retained(&old, &mut store),
        Err(LoopError::NotIssued(_))
    ));
    assert_eq!(policy.classify_superseded_retained(&old).unwrap(), old_key);
}

#[test]
fn old_listing_is_superseded_only_by_committed_interruption_not_fresh_observation() {
    let mut store = TestStore::in_memory().unwrap();
    let mut policy = PolicyLoop::from_committed(discovery(&mut store, 82, root()));
    let mut os = FakeOs {
        children: Ok(vec![identity(50).pid()]),
        ..FakeOs::default()
    };
    let ticket = policy.issue_next().unwrap();
    let old = execute(&mut os, ticket);
    assert!(matches!(
        policy.classify_superseded_retained(&old),
        Err(LoopError::NotSuperseded(_))
    ));
    policy.interrupt(&mut store).unwrap();
    policy.recover(&mut store).unwrap();
    let summary = policy.summary();
    let evidence = store.evidence().clone();
    assert!(matches!(
        policy.apply_retained(&old, &mut store),
        Err(LoopError::NotIssued(_))
    ));
    assert_eq!(
        policy.classify_superseded_retained(&old).unwrap(),
        old.key()
    );
    assert_eq!(
        policy.classify_superseded_retained(&old).unwrap(),
        old.key()
    );
    assert_eq!(policy.summary(), summary);
    assert_eq!(store.evidence(), &evidence);
    let tree = store.evidence().ownership_tree().unwrap();
    assert!(tree.unresolved().is_empty());
    assert!(tree.pending_child_scans().contains(&root()));
    assert_eq!(
        tree.identities()[&root()],
        OwnedIdentityStatus::NeedsFreshObservation
    );
    assert!(matches!(
        policy.finish_recovery(MonotonicTick::new(99), &mut store),
        Err(LoopError::RoundInFlight)
    ));
}

#[test]
fn replacement_loop_uses_committed_interruption_to_classify_only_older_same_attempt_results() {
    let mut store = TestStore::in_memory().unwrap();
    let (state, mut batch) = discovery(&mut store, 90, root()).into_dispatch();
    let old = execute(&mut FakeOs::default(), batch.next().unwrap());
    let committed = state.interrupt().unwrap().persist(&mut store).unwrap();
    let mut policy = PolicyLoop::from_committed(committed);
    assert_eq!(
        policy.classify_superseded_retained(&old).unwrap(),
        old.key()
    );
    policy.recover(&mut store).unwrap();
    let before = policy.summary();
    assert_eq!(
        policy.classify_superseded_retained(&old).unwrap(),
        old.key()
    );
    assert!(matches!(
        policy.classify_superseded_retained(&mirrored_list(91, root())),
        Err(LoopError::NotSuperseded(_))
    ));
    let ticket = policy.issue_next().unwrap();
    let fresh = execute(&mut FakeOs::default(), ticket);
    assert!(matches!(
        policy.classify_superseded_retained(&fresh),
        Err(LoopError::NotSuperseded(_))
    ));
    assert_eq!(policy.summary(), before);
    assert_eq!(
        store.evidence().ownership_tree().unwrap().identities()[&root()],
        OwnedIdentityStatus::NeedsFreshObservation
    );
}

#[test]
fn failed_interruption_cannot_classify_old_results_or_expose_a_live_checkpoint() {
    let mut store = TestStore::in_memory().unwrap();
    let mut policy = PolicyLoop::from_committed(discovery(&mut store, 83, root()));
    let ticket = policy.issue_next().unwrap();
    let old = execute(&mut FakeOs::default(), ticket);
    let before = policy.summary();
    store.fail = true;
    let error = policy.interrupt(&mut store).unwrap_err();
    assert_eq!(
        error.guardian_fence(),
        Some(LoopFenceReason::PersistenceFailure)
    );
    assert!(matches!(
        policy.classify_superseded_retained(&old),
        Err(LoopError::FailStopped)
    ));
    let LoopSummary {
        attempt_id,
        phase,
        revision,
        status,
    } = policy.summary();
    assert_eq!(attempt_id, before.attempt_id);
    assert_eq!(phase, before.phase);
    assert_eq!(revision, before.revision);
    assert_eq!(status, LoopStatus::FailStopped);
    assert_eq!(
        store.load_attempt(attempt_id).unwrap().unwrap().phase(),
        Phase::OwnershipDiscovery
    );
    assert!(policy.issue_next().is_none());
    assert_eq!(old.key().attempt_id(), attempt_id);
}

#[test]
fn sibling_completion_valid_after_revision_advance() {
    let mut store = TestStore::in_memory().unwrap();
    let mut policy = PolicyLoop::from_committed(discovery(&mut store, 10, root()));
    let mut os = FakeOs {
        children: Ok(vec![identity(60).pid(), identity(50).pid()]),
        ..FakeOs::default()
    };
    let ticket = policy.issue_next().unwrap();
    policy.apply(execute(&mut os, ticket), &mut store).unwrap();
    let first = policy.issue_next().unwrap();
    let second = policy.issue_next().unwrap();
    let issuing_revision = first.key().revision();
    assert_eq!(second.key().revision(), issuing_revision);
    assert_eq!((first.key().ordinal(), second.key().ordinal()), (0, 1));
    let first = execute(&mut os, first);
    let second = execute(&mut os, second);
    policy.apply(second, &mut store).unwrap();
    assert!(policy.summary().revision.get() > issuing_revision.get());
    assert!(
        policy.issue_next().is_none(),
        "follow-up rounds wait for the sibling"
    );
    policy.apply(first, &mut store).unwrap();
    assert_eq!(
        store
            .evidence()
            .ownership_tree()
            .unwrap()
            .identities()
            .len(),
        3
    );
    assert!(store
        .evidence()
        .ownership_tree()
        .unwrap()
        .unresolved()
        .is_empty());
    assert_durable(&mut store, &policy);

    os.children = Ok(Vec::new());
    for parent in [identity(60), identity(50)] {
        let ticket = policy.issue_next().unwrap();
        assert_eq!(ticket.effect(), &OsEffect::ListChildren { parent });
        policy.apply(execute(&mut os, ticket), &mut store).unwrap();
    }
    assert!(policy.issue_next().is_none());
    assert_eq!(policy.summary().phase, Phase::OwnershipDiscovery);
    policy.finish_ownership_scan(&mut store).unwrap();
    assert_eq!(policy.summary().phase, Phase::CleaningOwnedTree);
    assert!(matches!(
        policy.issue_next().unwrap().effect(),
        OsEffect::ObserveIdentity { .. }
    ));
}

fn mirrored_list(id: u128, root: ProcessIdentity) -> ExecutedEffect {
    let mut mirror = TestStore::in_memory().unwrap();
    let (_, mut batch) = discovery(&mut mirror, id, root).into_dispatch();
    execute(&mut FakeOs::default(), batch.next().unwrap())
}

#[test]
fn duplicate_wrong_stale_rejected() {
    let mut store = TestStore::in_memory().unwrap();
    let mut policy = PolicyLoop::from_committed(discovery(&mut store, 20, root()));
    let snapshot = policy.summary();
    assert!(matches!(
        policy.apply(mirrored_list(20, root()), &mut store),
        Err(LoopError::NotIssued(_))
    ));
    assert_eq!(policy.summary(), snapshot);
    let ticket = policy.issue_next().unwrap();
    let key = ticket.key();
    let snapshot = policy.summary();
    assert!(matches!(
        policy.apply(mirrored_list(21, root()), &mut store),
        Err(LoopError::NotIssued(_))
    ));
    assert_eq!(policy.summary(), snapshot);
    assert!(matches!(
        policy.apply(mirrored_list(20, identity(99)), &mut store),
        Err(LoopError::EffectMismatch(_))
    ));
    assert_eq!(policy.summary(), snapshot);

    let mut mirror = TestStore::in_memory().unwrap();
    let state = commit(&mut mirror, start(20, 2, root()));
    let (_, mut batch) = state
        .record_resource_sample(sample(0))
        .unwrap()
        .persist(&mut mirror)
        .unwrap()
        .into_dispatch();
    let wrong_kind = execute(&mut FakeOs::default(), batch.next().unwrap());
    assert_eq!(wrong_kind.key(), key);
    assert!(matches!(
        policy.apply(wrong_kind, &mut store),
        Err(LoopError::EffectMismatch(_))
    ));
    assert_eq!(policy.summary(), snapshot);
    assert_durable(&mut store, &policy);

    let valid = execute(&mut FakeOs::default(), ticket);
    policy.apply(valid, &mut store).unwrap();
    let snapshot = policy.summary();
    let duplicate = mirrored_list(20, root());
    assert_eq!(duplicate.key(), key);
    assert!(matches!(
        policy.apply(duplicate, &mut store),
        Err(LoopError::NotIssued(_))
    ));
    assert_eq!(policy.summary(), snapshot);

    let transition = cleaning_transition(&mut store, 22, &[]);
    let mut policy = PolicyLoop::from_committed(transition.persist(&mut store).unwrap());
    let ticket = policy.issue_next().unwrap();
    let stale_key = ticket.key();
    let stale_result = execute(&mut FakeOs::default(), ticket);
    policy.interrupt(&mut store).unwrap();
    policy.recover(&mut store).unwrap();
    let snapshot = policy.summary();
    assert!(matches!(
        policy.apply(stale_result, &mut store),
        Err(LoopError::NotIssued(_))
    ));
    assert_eq!(policy.summary(), snapshot);
    let fresh = policy.issue_next().unwrap();
    assert!(fresh.key().revision().get() > stale_key.revision().get());
    policy
        .apply(execute(&mut FakeOs::default(), fresh), &mut store)
        .unwrap();
    assert_durable(&mut store, &policy);
}

#[test]
fn failed_resource_query_retains_result_and_obligation_without_evidence() {
    for failure in [
        ResourceObservationFailure::TemporarilyUnavailable,
        ResourceObservationFailure::Unsupported,
        ResourceObservationFailure::Malformed,
    ] {
        let mut store = TestStore::in_memory().unwrap();
        let mut policy =
            PolicyLoop::from_committed(start(30, 2, root()).persist(&mut store).unwrap());
        let before = policy.summary().revision;
        let ticket = policy.issue_next().unwrap();
        let key = ticket.key();
        let mut os = FakeOs {
            resource_failure: Some(failure),
            ..FakeOs::default()
        };
        let retained = execute(&mut os, ticket);
        let progress = policy.apply_retained(&retained, &mut store).unwrap();
        assert!(progress.acknowledge_keys().next().is_none());
        assert_eq!(
            progress.guardian_fence(),
            Some(LoopFenceReason::QueryFailure)
        );
        assert_eq!(retained.key().attempt_id(), policy.summary().attempt_id);
        let reason = ParkReason::ResourceObservationFailure {
            sequence: 0,
            failure,
        };
        assert_eq!(
            progress,
            LoopProgress::Parked {
                key,
                revision: before.checked_next().unwrap(),
                reason,
                ack: AckDisposition::RetainParked,
            }
        );
        assert_eq!(policy.status(), LoopStatus::Parked(reason));
        assert_eq!(policy.summary().phase, Phase::ResourceSampling);
        assert!(store.evidence().resource_evidence().is_none());
        assert!(store.evidence().ownership_tree().is_none());
        assert!(policy.issue_next().is_none());
        assert!(matches!(
            policy.finish_ownership_scan(&mut store),
            Err(LoopError::Parked(_))
        ));
        assert_durable(&mut store, &policy);
        let loaded = reload_interrupted(&mut store, 30);
        let (_, mut batch) = loaded
            .recover()
            .unwrap()
            .persist(&mut store)
            .unwrap()
            .into_dispatch();
        assert_eq!(
            batch.next().unwrap().effect(),
            &OsEffect::SampleResources { sequence: 0 }
        );
        assert!(batch.next().is_none());
    }
}

#[test]
fn failed_child_query_retains_result_and_obligation_without_evidence() {
    for failure in [
        OsQueryError::PermissionDenied,
        OsQueryError::TemporarilyUnavailable,
        OsQueryError::Malformed,
    ] {
        let mut store = TestStore::in_memory().unwrap();
        let mut policy = PolicyLoop::from_committed(discovery(&mut store, 31, root()));
        let tree = store.evidence().ownership_tree().unwrap().clone();
        let ticket = policy.issue_next().unwrap();
        let mut os = FakeOs {
            children: Err(failure),
            ..FakeOs::default()
        };
        let retained = execute(&mut os, ticket);
        let progress = policy.apply_retained(&retained, &mut store).unwrap();
        assert!(progress.acknowledge_keys().next().is_none());
        assert_eq!(
            progress.guardian_fence(),
            Some(LoopFenceReason::QueryFailure)
        );
        assert_eq!(retained.key().attempt_id(), policy.summary().attempt_id);
        let reason = ParkReason::ChildQueryFailure {
            parent: root(),
            failure,
        };
        assert!(matches!(progress, LoopProgress::Parked {
                reason: actual, ack: AckDisposition::RetainParked, ..
            } if actual == reason));
        assert_eq!(policy.status(), LoopStatus::Parked(reason));
        assert_eq!(store.evidence().ownership_tree(), Some(&tree));
        assert!(policy.issue_next().is_none());
        assert_durable(&mut store, &policy);
        let loaded = reload_interrupted(&mut store, 31);
        assert_eq!(loaded.ownership_tree(), Some(&tree));
        let (_, batch) = loaded
            .recover()
            .unwrap()
            .persist(&mut store)
            .unwrap()
            .into_dispatch();
        assert_eq!(
            batch
                .map(|ticket| ticket.effect().clone())
                .collect::<Vec<_>>(),
            vec![
                OsEffect::ObserveIdentity { identity: root() },
                OsEffect::ListChildren { parent: root() },
            ]
        );
    }
}

fn observed_cleaning_state(
    store: &mut TestStore,
    id: u128,
    observation: IdentityObservation,
) -> DeploymentAttempt {
    let transition = cleaning_transition(store, id, &[]);
    let state = commit(store, transition);
    commit(store, state.identity_observed(root(), observation).unwrap())
}

#[test]
fn committed_zero_effect_signal_round_requires_explicit_checked_admission() {
    let mut store = TestStore::in_memory().unwrap();
    let state = observed_cleaning_state(&mut store, 84, IdentityObservation::ExactIdentityAbsent);
    let ordinary = state
        .identity_observed(root(), IdentityObservation::ExactIdentityAbsent)
        .unwrap()
        .persist(&mut store)
        .unwrap();
    assert!(ordinary.after_persist().is_empty());
    assert_eq!(ordinary.checkpoint().phase(), Phase::CleaningOwnedTree);
    assert!(matches!(
        PolicyLoop::from_committed_signal_round(ordinary),
        Err(LoopError::NoSignalRound)
    ));

    let state = observed_cleaning_state(&mut store, 85, IdentityObservation::ExactIdentityAbsent);
    let plan = state
        .request_owned_signals(Signal::Terminate, MonotonicTick::new(1))
        .unwrap()
        .persist(&mut store)
        .unwrap();
    assert!(plan.after_persist().is_empty());
    let mut generic = PolicyLoop::from_committed(plan);
    assert!(generic.signal_results().is_none());
    assert!(matches!(
        generic.finish_signal_round(&mut store),
        Err(LoopError::NoSignalRound)
    ));

    let state = observed_cleaning_state(&mut store, 86, IdentityObservation::ExactIdentityAbsent);
    let plan = state
        .request_owned_signals(Signal::Terminate, MonotonicTick::new(1))
        .unwrap()
        .persist(&mut store)
        .unwrap();
    let mut policy = PolicyLoop::from_committed_signal_round(plan).unwrap();
    assert_eq!(policy.signal_results(), Some([].as_slice()));
    assert!(policy.issue_next().is_none());
    let progress = policy.finish_signal_round(&mut store).unwrap();
    assert!(
        matches!(progress, LoopProgress::SignalRoundFinished { ref results, .. } if results.is_empty())
    );
    assert!(progress.acknowledge_keys().next().is_none());
    assert_eq!(policy.summary().phase, Phase::VerifyingCleanup);
    assert_durable(&mut store, &policy);
    assert_eq!(
        policy.issue_next().unwrap().effect(),
        &OsEffect::ObserveIdentity { identity: root() }
    );
}

fn one_signal_plan(store: &mut TestStore, id: u128) -> CommittedTransition {
    let state = observed_cleaning_state(
        store,
        id,
        IdentityObservation::SameIdentity {
            current_parent: None,
        },
    );
    state
        .request_owned_signals(Signal::Terminate, MonotonicTick::new(1))
        .unwrap()
        .persist(store)
        .unwrap()
}

fn one_signal_round(store: &mut TestStore, id: u128) -> PolicyLoop {
    PolicyLoop::from_committed_signal_round(one_signal_plan(store, id)).unwrap()
}

#[test]
fn failed_signal_settlement_retains_the_borrowed_result_and_requires_fencing() {
    let mut store = TestStore::in_memory().unwrap();
    let mut policy = one_signal_round(&mut store, 87);
    let ticket = policy.issue_next().unwrap();
    let retained = execute(&mut FakeOs::default(), ticket);
    let progress = policy.apply_retained(&retained, &mut store).unwrap();
    assert!(matches!(
        progress,
        LoopProgress::Applied {
            ack: AckDisposition::RetainUntilSignalSettlement,
            ..
        }
    ));
    assert!(progress.acknowledge_keys().next().is_none());
    let before = policy.summary();
    store.fail = true;
    let error = policy.finish_signal_round(&mut store).unwrap_err();
    assert_eq!(
        error.guardian_fence(),
        Some(LoopFenceReason::PersistenceFailure)
    );
    assert_eq!(policy.summary().phase, before.phase);
    assert_eq!(policy.summary().revision, before.revision);
    assert_eq!(policy.status(), LoopStatus::FailStopped);
    assert_eq!(retained.key().attempt_id(), AttemptId::new(87));
    assert!(matches!(
        policy.classify_superseded_retained(&retained),
        Err(LoopError::FailStopped)
    ));
}

#[test]
fn retained_signal_and_query_failures_need_committed_interruption_for_supersession() {
    let mut store = TestStore::in_memory().unwrap();
    let mut policy = one_signal_round(&mut store, 88);
    let ticket = policy.issue_next().unwrap();
    let mut os = FakeOs {
        signal: SignalOutcome::ExactIdentityAlreadyAbsent,
        ..FakeOs::default()
    };
    let retained = execute(&mut os, ticket);
    policy.apply_retained(&retained, &mut store).unwrap();
    assert!(matches!(
        policy.classify_superseded_retained(&retained),
        Err(LoopError::NotSuperseded(_))
    ));
    policy.interrupt(&mut store).unwrap();
    policy.recover(&mut store).unwrap();
    let before = policy.summary();
    assert!(matches!(
        policy.apply_retained(&retained, &mut store),
        Err(LoopError::NotIssued(_))
    ));
    assert_eq!(
        policy.classify_superseded_retained(&retained).unwrap(),
        retained.key()
    );
    assert_eq!(policy.summary(), before);
    assert_eq!(
        store.evidence().ownership_tree().unwrap().identities()[&root()],
        OwnedIdentityStatus::NeedsFreshObservation
    );
    assert!(policy.signal_results().is_none());

    let mut policy = PolicyLoop::from_committed(discovery(&mut store, 89, root()));
    let ticket = policy.issue_next().unwrap();
    os.children = Err(OsQueryError::PermissionDenied);
    let retained = execute(&mut os, ticket);
    policy.apply_retained(&retained, &mut store).unwrap();
    assert!(matches!(
        policy.classify_superseded_retained(&retained),
        Err(LoopError::NotSuperseded(_))
    ));
    policy.interrupt(&mut store).unwrap();
    assert_eq!(
        policy.classify_superseded_retained(&retained).unwrap(),
        retained.key()
    );
    assert_durable(&mut store, &policy);
}

fn retain_signal_outcomes(
    policy: &mut PolicyLoop,
    store: &mut TestStore,
    os: &mut FakeOs,
) -> [SignalOutcome; 5] {
    let outcomes = [
        SignalOutcome::Delivered,
        SignalOutcome::ExactIdentityAlreadyAbsent,
        SignalOutcome::PidReused,
        SignalOutcome::PermissionDenied,
        SignalOutcome::TemporarilyUnavailable,
    ];
    let mut completions = Vec::new();
    for outcome in outcomes {
        os.signal = outcome;
        let ticket = policy.issue_next().unwrap();
        assert!(matches!(
            ticket.effect(),
            OsEffect::SignalExactIdentity {
                signal: Signal::Terminate,
                ..
            }
        ));
        completions.push(execute(os, ticket));
    }
    assert!(policy.issue_next().is_none());
    assert!(matches!(
        policy.finish_signal_round(store),
        Err(LoopError::SignalRoundIncomplete)
    ));
    for completion in completions.iter().rev() {
        let progress = policy.apply_retained(completion, store).unwrap();
        assert!(matches!(
            progress,
            LoopProgress::Applied {
                ack: AckDisposition::RetainUntilSignalSettlement,
                ..
            }
        ));
        assert!(progress.acknowledge_keys().next().is_none());
        assert_eq!(policy.summary().phase, Phase::CleaningOwnedTree);
        assert_eq!(
            store.evidence().cleanup_outcome(),
            CleanupOutcome::InProgress
        );
        assert!(store
            .evidence()
            .ownership_tree()
            .unwrap()
            .identities()
            .values()
            .all(|status| matches!(status, OwnedIdentityStatus::Present { .. })));
        assert!(policy.issue_next().is_none());
        assert_durable(store, policy);
    }
    outcomes
}

#[test]
fn signal_results_require_explicit_finish_and_fresh_verification() {
    let mut store = TestStore::in_memory().unwrap();
    let transition = cleaning_transition(&mut store, 40, &[50, 60, 70, 80]);
    let mut policy = PolicyLoop::from_committed(transition.persist(&mut store).unwrap());
    let mut os = FakeOs::default();
    apply_observation_round(&mut policy, &mut store, &mut os, 5);
    assert!(policy.issue_next().is_none());
    assert_eq!(policy.summary().phase, Phase::CleaningOwnedTree);
    policy
        .request_owned_signals(Signal::Terminate, MonotonicTick::new(1), &mut store)
        .unwrap();
    assert!(matches!(
        policy.finish_signal_round(&mut store),
        Err(LoopError::SignalRoundIncomplete)
    ));
    let outcomes = retain_signal_outcomes(&mut policy, &mut store, &mut os);
    assert_eq!(
        policy
            .signal_results()
            .unwrap()
            .iter()
            .map(|result| result.outcome)
            .collect::<Vec<_>>(),
        outcomes
    );
    assert!(matches!(
        policy.finish_verification(MonotonicTick::new(99), &mut store),
        Err(LoopError::RoundInFlight)
    ));
    let progress = policy.finish_signal_round(&mut store).unwrap();
    let ack_keys = progress.acknowledge_keys().collect::<Vec<_>>();
    assert_eq!(ack_keys.len(), 5);
    assert_eq!(
        ack_keys.iter().map(|key| key.ordinal()).collect::<Vec<_>>(),
        vec![0, 1, 2, 3, 4]
    );
    assert_durable(&mut store, &policy);
    let LoopProgress::SignalRoundFinished { results, .. } = progress else {
        panic!("expected signal settlement");
    };
    assert_eq!(
        results
            .iter()
            .map(|result| result.key.ordinal())
            .collect::<Vec<_>>(),
        vec![0, 1, 2, 3, 4]
    );
    assert_eq!(
        results
            .iter()
            .map(|result| result.outcome)
            .collect::<Vec<_>>(),
        outcomes
    );
    assert_eq!(policy.summary().phase, Phase::VerifyingCleanup);
    assert!(store
        .evidence()
        .ownership_tree()
        .unwrap()
        .identities()
        .values()
        .all(|status| *status == OwnedIdentityStatus::NeedsFreshObservation));
    assert!(policy.signal_results().is_none());
    assert!(matches!(
        policy.finish_verification(MonotonicTick::new(99), &mut store),
        Err(LoopError::RoundInFlight)
    ));
    os.observation = IdentityObservation::ExactIdentityAbsent;
    apply_observation_round(&mut policy, &mut store, &mut os, 5);
    assert_eq!(policy.summary().phase, Phase::VerifyingCleanup);
    policy
        .finish_verification(MonotonicTick::new(99), &mut store)
        .unwrap();
    assert_eq!(policy.summary().phase, Phase::ReadyForDisruption);
    assert_eq!(
        store.evidence().cleanup_outcome(),
        CleanupOutcome::VerifiedClean
    );
    assert!(policy.issue_next().is_none());
    assert_eq!(
        os.calls
            .iter()
            .filter(|effect| matches!(effect, OsEffect::SignalExactIdentity { .. }))
            .count(),
        5
    );
    assert_durable(&mut store, &policy);
}

#[test]
fn persistence_failure_requires_fencing_remaining_authority() {
    let mut store = TestStore::in_memory().unwrap();
    let transition = cleaning_transition(&mut store, 50, &[50, 60]);
    let committed = transition.persist(&mut store).unwrap();
    let competing = committed.checkpoint().clone();
    let mut policy = PolicyLoop::from_committed(committed);
    let first = policy.issue_next().unwrap();
    let second = policy.issue_next().unwrap();
    let mut os = FakeOs::default();
    let first = execute(&mut os, first);
    let second = execute(&mut os, second);
    let snapshot = policy.summary();
    commit(
        &mut store,
        competing
            .identity_observed(root(), IdentityObservation::ExactIdentityAbsent)
            .unwrap(),
    );
    let error = policy.apply_retained(&first, &mut store).unwrap_err();
    assert!(matches!(
        error,
        LoopError::Persistence(ControllerStoreError::RevisionConflict)
    ));
    assert_eq!(
        error.guardian_fence(),
        Some(LoopFenceReason::PersistenceFailure)
    );
    assert_eq!(policy.status(), LoopStatus::FailStopped);
    assert_eq!(policy.summary().revision, snapshot.revision);
    assert_eq!(policy.summary().phase, snapshot.phase);
    assert_eq!(first.key().attempt_id(), snapshot.attempt_id);
    assert!(
        policy.issue_next().is_none(),
        "unissued third sibling was dropped"
    );
    assert!(matches!(
        policy.apply(second, &mut store),
        Err(LoopError::FailStopped)
    ));
    assert!(matches!(
        policy.interrupt(&mut store),
        Err(LoopError::FailStopped)
    ));
    assert!(matches!(
        policy.recover(&mut store),
        Err(LoopError::FailStopped)
    ));
    assert!(matches!(
        policy.deadline_reached(MonotonicTick::new(100), &mut store),
        Err(LoopError::FailStopped)
    ));
    assert!(matches!(
        policy.finish_signal_round(&mut store),
        Err(LoopError::FailStopped)
    ));
    assert_eq!(os.calls.len(), 2);
}

#[test]
fn candidate_exact_stops_at_forward_recovery_no_activation() {
    let mut store = TestStore::in_memory().unwrap();
    let ready = ready(&mut store, 60);
    let migrated = commit(
        &mut store,
        ready.record_migration_invoked(candidate()).unwrap(),
    );
    let committed = migrated.recover().unwrap().persist(&mut store).unwrap();
    let mut policy = PolicyLoop::from_committed(committed);
    let ticket = policy.issue_next().unwrap();
    assert_eq!(
        ticket.effect(),
        &OsEffect::ReverifyRetainedCandidate {
            candidate: candidate()
        }
    );
    let mut os = FakeOs::default();
    policy.apply(execute(&mut os, ticket), &mut store).unwrap();
    assert_eq!(
        store.evidence().candidate_verification(),
        Some(CandidateVerification::Exact)
    );
    assert_eq!(store.evidence().forward_candidate(), Some(candidate()));
    assert_eq!(policy.summary().phase, Phase::ForwardRecovery);
    assert_eq!(
        store.evidence().cleanup_outcome(),
        CleanupOutcome::UnresolvedRisk
    );
    assert!(policy.issue_next().is_none());
    let snapshot = policy.summary();
    assert!(matches!(
        policy.finish_verification(MonotonicTick::new(99), &mut store),
        Err(LoopError::Transition(TransitionError::WrongPhase))
    ));
    assert_eq!(policy.summary(), snapshot);
    assert_eq!(
        os.calls,
        vec![OsEffect::ReverifyRetainedCandidate {
            candidate: candidate()
        }]
    );
    assert_durable(&mut store, &policy);
}

#[test]
fn recovery_children_and_identity_siblings_remain_behind_explicit_barrier() {
    let mut store = TestStore::in_memory().unwrap();
    let state = discovery(&mut store, 69, root()).into_parts().0;
    let mut policy =
        PolicyLoop::from_committed(state.interrupt().unwrap().persist(&mut store).unwrap());
    policy.recover(&mut store).unwrap();
    let root_observation = policy.issue_next().unwrap();
    let root_scan = policy.issue_next().unwrap();
    let mut os = FakeOs {
        children: Ok(vec![identity(50).pid()]),
        ..FakeOs::default()
    };
    policy
        .apply(execute(&mut os, root_scan), &mut store)
        .unwrap();
    assert_eq!(policy.summary().phase, Phase::Recovering);
    assert!(policy.issue_next().is_none());
    policy
        .apply(execute(&mut os, root_observation), &mut store)
        .unwrap();
    let child = policy.issue_next().unwrap();
    assert!(matches!(
        child.effect(),
        OsEffect::AcquireChildIdentity { .. }
    ));
    policy.apply(execute(&mut os, child), &mut store).unwrap();
    assert_eq!(policy.summary().phase, Phase::Recovering);
    assert!(matches!(
        policy.finish_recovery(MonotonicTick::new(99), &mut store),
        Err(LoopError::RoundInFlight)
    ));
    let child_scan = policy.issue_next().unwrap();
    let child_observation = policy.issue_next().unwrap();
    assert_eq!(
        child_scan.effect(),
        &OsEffect::ListChildren {
            parent: identity(50)
        }
    );
    assert_eq!(
        child_observation.effect(),
        &OsEffect::ObserveIdentity {
            identity: identity(50)
        }
    );
    os.children = Ok(Vec::new());
    policy
        .apply(execute(&mut os, child_scan), &mut store)
        .unwrap();
    policy
        .apply(execute(&mut os, child_observation), &mut store)
        .unwrap();
    assert_eq!(policy.summary().phase, Phase::Recovering);
    assert!(policy.issue_next().is_none());
    policy
        .finish_recovery(MonotonicTick::new(99), &mut store)
        .unwrap();
    assert_eq!(policy.summary().phase, Phase::CleaningOwnedTree);
    assert_eq!(store.evidence().deadline(), MonotonicTick::new(100));
    assert!(
        policy.issue_next().is_none(),
        "recovery never infers a signal"
    );
    assert_durable(&mut store, &policy);
}

#[test]
fn explicit_interrupt_recovery_deadline_preserve_clock_and_fence_old_session() {
    let mut store = TestStore::in_memory().unwrap();
    let mut policy = PolicyLoop::from_committed(discovery(&mut store, 70, root()));
    let ticket = policy.issue_next().unwrap();
    let stale = execute(&mut FakeOs::default(), ticket);
    let snapshot = policy.summary();
    assert!(matches!(
        policy.deadline_reached(MonotonicTick::new(99), &mut store),
        Err(LoopError::Transition(TransitionError::DeadlineNotReached))
    ));
    assert_eq!(policy.summary(), snapshot);
    policy.interrupt(&mut store).unwrap();
    assert_eq!(policy.summary().phase, Phase::Interrupted);
    assert!(policy.issue_next().is_none());
    policy.recover(&mut store).unwrap();
    assert_eq!(policy.summary().phase, Phase::Recovering);
    assert_eq!(store.evidence().deadline(), MonotonicTick::new(100));
    assert!(matches!(
        policy.apply(stale, &mut store),
        Err(LoopError::NotIssued(_))
    ));
    let progress = policy
        .deadline_reached(MonotonicTick::new(100), &mut store)
        .unwrap();
    assert!(matches!(
        progress,
        LoopProgress::GuardianMustFence {
            reason: LoopFenceReason::DeadlineReached,
            ..
        }
    ));
    assert_eq!(
        progress.guardian_fence(),
        Some(LoopFenceReason::DeadlineReached)
    );
    assert!(progress.acknowledge_keys().next().is_none());
    assert_eq!(
        policy.summary().phase,
        Phase::AdministrativeCloseoutRequired
    );
    assert_eq!(store.evidence().deadline(), MonotonicTick::new(100));
    assert_eq!(
        store.evidence().cleanup_outcome(),
        CleanupOutcome::UnresolvedRisk
    );
    assert!(policy.issue_next().is_none());
    assert_durable(&mut store, &policy);
}

struct TestGuardianExecutor {
    os: FakeOs,
    executions: Rc<RefCell<Vec<OsEffect>>>,
}

impl GuardianExecutor for TestGuardianExecutor {
    type ChildSlot = ();

    fn execute(
        &mut self,
        ticket: DispatchTicket,
        _slots: &mut BTreeMap<ChildSlotId, ()>,
    ) -> ExecutedEffect {
        self.executions.borrow_mut().push(ticket.effect().clone());
        execute(&mut self.os, ticket)
    }

    fn poll_observation_only(&self, _slot: &()) -> ChildSlotObservation {
        ChildSlotObservation::Running
    }
}

fn audited_guardian_fixture(
    id: u128,
    os: FakeOs,
) -> (
    Guardian<TestGuardianExecutor>,
    ControllerSession,
    Rc<RefCell<Vec<OsEffect>>>,
) {
    let executions = Rc::new(RefCell::new(Vec::new()));
    let (guardian, session) = Guardian::new(
        AttemptId::new(id),
        MonotonicTick::new(100),
        TestGuardianExecutor {
            os,
            executions: Rc::clone(&executions),
        },
        vec![()],
    )
    .unwrap();
    (guardian, session, executions)
}

fn checkpoint_recovery(loaded: LoadedAttempt) -> Transition {
    let RecoveryDisposition::Checkpoint(transition) = loaded.prepare_recovery().unwrap() else {
        panic!("expected recoverable checkpoint");
    };
    *transition
}

fn begin_loaded_reconciliation(
    store: &mut TestStore,
    guardian: &mut Guardian<TestGuardianExecutor>,
    old: ControllerSession,
) -> ControllerSession {
    let loaded = store.load_attempt(guardian.attempt_id()).unwrap().unwrap();
    assert_eq!(loaded.phase(), Phase::Interrupted);
    let floor = loaded.persistence_revision();
    let mut interruption =
        PolicyLoop::from_loaded_interruption_for_guardian(loaded, guardian, old).unwrap();
    assert_eq!(interruption.summary().revision, floor);
    let slots: Vec<_> = guardian.inventory().collect();
    let current = interruption
        .begin_guardian_reconciliation(guardian, old, &slots)
        .unwrap();
    assert_eq!(guardian.reconciliation_floor(), Some(floor));
    current
}

fn guardian_fixture(id: u128, os: FakeOs) -> (Guardian<TestGuardianExecutor>, ControllerSession) {
    let (guardian, session, _) = audited_guardian_fixture(id, os);
    (guardian, session)
}

fn coordinated_step(
    policy: &mut PolicyLoop,
    guardian: &mut Guardian<TestGuardianExecutor>,
    session: ControllerSession,
    store: &mut TestStore,
) -> DispatchKey {
    let key = policy
        .dispatch_next_to_guardian(guardian, session, MonotonicTick::new(1))
        .unwrap()
        .unwrap();
    assert!(guardian.retained_result(key).is_some());
    policy
        .apply_guardian_result(guardian, session, key, store)
        .unwrap();
    assert_durable(store, policy);
    key
}

#[test]
fn guardian_policy_loop_persists_then_acks() {
    let mut store = TestStore::in_memory().unwrap();
    let committed = start(100, 1, root()).persist(&mut store).unwrap();
    let (mut guardian, session) = guardian_fixture(
        100,
        FakeOs {
            observation: IdentityObservation::ExactIdentityAbsent,
            ..FakeOs::default()
        },
    );
    let mut policy =
        PolicyLoop::from_committed_for_guardian(committed, &guardian, session).unwrap();
    let revision = policy.summary().revision;
    let sample_key = coordinated_step(&mut policy, &mut guardian, session, &mut store);
    assert_eq!(policy.summary().revision, revision.checked_next().unwrap());
    assert!(guardian.retained_result(sample_key).is_none());
    assert_eq!(policy.summary().phase, Phase::OwnershipDiscovery);
    let list = coordinated_step(&mut policy, &mut guardian, session, &mut store);
    assert!(guardian.retained_result(list).is_none());
    assert!(store
        .evidence()
        .ownership_tree()
        .unwrap()
        .pending_child_scans()
        .is_empty());
    policy.finish_ownership_scan(&mut store).unwrap();
    coordinated_step(&mut policy, &mut guardian, session, &mut store);
    policy
        .request_owned_signals(Signal::Terminate, MonotonicTick::new(1), &mut store)
        .unwrap();
    assert_eq!(policy.signal_results(), Some([].as_slice()));
    policy
        .finish_guardian_signal_round(&mut guardian, session, &mut store)
        .unwrap();
    coordinated_step(&mut policy, &mut guardian, session, &mut store);
    policy
        .finish_verification(MonotonicTick::new(99), &mut store)
        .unwrap();
    assert_eq!(policy.summary().phase, Phase::ReadyForDisruption);
    assert_eq!(
        store.evidence().cleanup_outcome(),
        CleanupOutcome::VerifiedClean
    );
    assert_eq!(guardian.retained_results().len(), 0);
    assert_eq!(guardian.state(), GuardianState::Active);
    assert_durable(&mut store, &policy);
}

#[test]
fn coordinated_persistence_failure_retains_result_and_loses_authority() {
    let mut store = TestStore::in_memory().unwrap();
    let committed = start(101, 1, root()).persist(&mut store).unwrap();
    let (mut guardian, session) = guardian_fixture(101, FakeOs::default());
    let mut policy =
        PolicyLoop::from_committed_for_guardian(committed, &guardian, session).unwrap();
    let key = policy
        .dispatch_next_to_guardian(&mut guardian, session, MonotonicTick::new(1))
        .unwrap()
        .unwrap();
    let durable = policy.summary();
    store.fail = true;
    assert!(matches!(
        policy.apply_guardian_result(&mut guardian, session, key, &mut store),
        Err(GuardianLoopError::Persistence(
            ControllerStoreError::RevisionConflict
        ))
    ));
    assert_eq!(guardian.state(), GuardianState::AuthorityLost);
    assert!(guardian.retained_result(key).is_some());
    assert_eq!(policy.summary().revision, durable.revision);
    assert_eq!(policy.status(), LoopStatus::FailStopped);
    assert!(policy
        .dispatch_next_to_guardian(&mut guardian, session, MonotonicTick::new(1))
        .is_err());
}

#[test]
fn same_guardian_inventory_and_committed_fresh_barrier_activate() {
    let mut store = TestStore::in_memory().unwrap();
    let committed = discovery(&mut store, 102, root());
    let (mut guardian, old) = guardian_fixture(102, FakeOs::default());
    let mut policy = PolicyLoop::from_committed_for_guardian(committed, &guardian, old).unwrap();
    let old_key = policy
        .dispatch_next_to_guardian(&mut guardian, old, MonotonicTick::new(1))
        .unwrap()
        .unwrap();
    assert!(policy
        .classify_superseded_with_guardian(&mut guardian, old, old_key)
        .is_err());
    policy
        .interrupt_with_guardian(&mut guardian, old, &mut store)
        .unwrap();
    assert_eq!(guardian.state(), GuardianState::DetachedFenced);
    let slots: Vec<_> = guardian.inventory().collect();
    let current = policy
        .begin_guardian_reconciliation(&mut guardian, old, &slots)
        .unwrap();
    assert_eq!(
        guardian.reconciliation_floor(),
        Some(policy.summary().revision)
    );
    assert!(matches!(
        policy.apply_guardian_result(&mut guardian, old, old_key, &mut store),
        Err(GuardianLoopError::Guardian(GuardianError::StaleSession))
    ));
    policy
        .recover_with_guardian(&mut guardian, current, &mut store)
        .unwrap();
    let revision = policy.summary().revision;
    assert!(matches!(
        policy.apply_guardian_result(&mut guardian, current, old_key, &mut store),
        Err(GuardianLoopError::Policy(LoopFailure::NotIssued))
    ));
    assert_eq!(
        policy
            .classify_superseded_with_guardian(&mut guardian, current, old_key)
            .unwrap(),
        old_key
    );
    assert_eq!(policy.summary().revision, revision);
    assert!(guardian.retained_result(old_key).is_none());
    assert!(!store
        .evidence()
        .ownership_tree()
        .unwrap()
        .reconciliation_complete());
    for _ in 0..2 {
        let fresh = coordinated_step(&mut policy, &mut guardian, current, &mut store);
        assert!(fresh.revision().get() > old_key.revision().get());
        assert!(guardian.retained_result(fresh).is_none());
        assert_eq!(guardian.state(), GuardianState::Reconciling);
    }
    assert!(policy
        .dispatch_next_to_guardian(&mut guardian, current, MonotonicTick::new(1))
        .unwrap()
        .is_none());
    let barrier_revision = policy.summary().revision;
    policy
        .finish_recovery_with_guardian(&mut guardian, current, MonotonicTick::new(99), &mut store)
        .unwrap();
    assert_eq!(
        policy.summary().revision,
        barrier_revision.checked_next().unwrap()
    );
    assert_eq!(guardian.state(), GuardianState::Active);
    assert_eq!(guardian.inventory().collect::<Vec<_>>(), slots);
    assert_eq!(guardian.original_deadline(), MonotonicTick::new(100));
    assert_eq!(policy.summary().phase, Phase::CleaningOwnedTree);
    assert_durable(&mut store, &policy);
}

#[test]
fn old_signal_loop_cannot_first_bind_replacement_session() {
    let mut store = TestStore::in_memory().unwrap();
    let committed = one_signal_plan(&mut store, 103);
    let (mut guardian, old, audit) = audited_guardian_fixture(103, FakeOs::default());
    let mut policy = PolicyLoop::from_committed_for_guardian(committed, &guardian, old).unwrap();
    let revision = policy.summary().revision;
    guardian.controller_lost(old).unwrap();
    let loaded = store.load_attempt(AttemptId::new(103)).unwrap().unwrap();
    let interrupted = checkpoint_recovery(loaded).persist(&mut store).unwrap();
    let interrupted_revision = interrupted.checkpoint().persistence_revision().unwrap();
    let current = begin_loaded_reconciliation(&mut store, &mut guardian, old);
    let recovery = interrupted
        .into_parts()
        .0
        .recover()
        .unwrap()
        .persist(&mut store)
        .unwrap();
    let mut replacement =
        PolicyLoop::from_committed_recovery_for_guardian(recovery, &guardian, current).unwrap();
    coordinated_step(&mut replacement, &mut guardian, current, &mut store);
    replacement
        .finish_recovery_with_guardian(&mut guardian, current, MonotonicTick::new(99), &mut store)
        .unwrap();
    assert_eq!(guardian.state(), GuardianState::Active);
    assert!(replacement.summary().revision.get() > interrupted_revision.get());
    let executions_before = audit.borrow().len();
    for _ in 0..3 {
        assert!(matches!(
            policy.dispatch_next_to_guardian(&mut guardian, current, MonotonicTick::new(1)),
            Err(GuardianLoopError::Guardian(GuardianError::StaleSession))
        ));
        assert_eq!(guardian.retained_results().len(), 0);
        assert_eq!(policy.summary().revision, revision);
    }
    let returned = policy.issue_next().unwrap();
    assert_eq!(returned.key().revision(), revision);
    assert_eq!(returned.key().ordinal(), 0);
    assert_eq!(
        returned.effect(),
        &OsEffect::SignalExactIdentity {
            identity: root(),
            signal: Signal::Terminate,
        }
    );
    assert!(policy.issue_next().is_none());
    assert_eq!(audit.borrow().len(), executions_before);
    assert!(!audit
        .borrow()
        .iter()
        .any(|effect| matches!(effect, OsEffect::SignalExactIdentity { .. })));
    assert!(policy.signal_results().unwrap().is_empty());
    assert!(matches!(
        policy.finish_signal_round(&mut store),
        Err(LoopError::SignalRoundIncomplete)
    ));
    assert_eq!(guardian.state(), GuardianState::Active);
}

#[test]
fn unconsumed_unbound_commits_cannot_dispatch_below_reconciled_revision_floor() {
    for (id, resources) in [(122, false), (123, true)] {
        let mut store = TestStore::in_memory().unwrap();
        let held = if resources {
            start(id, 2, root()).persist(&mut store).unwrap()
        } else {
            one_signal_plan(&mut store, id)
        };
        let held_revision = held.checkpoint().persistence_revision().unwrap();
        assert_eq!(held.after_persist().len(), 1);
        let held_effect = held.after_persist()[0].clone();
        let (mut guardian, old, audit) = audited_guardian_fixture(id, FakeOs::default());
        guardian.controller_lost(old).unwrap();
        let loaded = store.load_attempt(AttemptId::new(id)).unwrap().unwrap();
        let interrupted = checkpoint_recovery(loaded).persist(&mut store).unwrap();
        let current = begin_loaded_reconciliation(&mut store, &mut guardian, old);
        assert_eq!(current.instance_id(), old.instance_id());
        assert_eq!(current.generation(), old.generation() + 1);
        let recovery = interrupted
            .into_parts()
            .0
            .recover()
            .unwrap()
            .persist(&mut store)
            .unwrap();
        let mut replacement =
            PolicyLoop::from_committed_recovery_for_guardian(recovery, &guardian, current).unwrap();
        if !resources {
            coordinated_step(&mut replacement, &mut guardian, current, &mut store);
        }
        let before_barrier = replacement.summary().revision;
        replacement
            .finish_recovery_with_guardian(
                &mut guardian,
                current,
                MonotonicTick::new(99),
                &mut store,
            )
            .unwrap();
        let barrier_revision = replacement.summary().revision;
        assert_eq!(
            barrier_revision,
            if resources {
                before_barrier
            } else {
                before_barrier.checked_next().unwrap()
            }
        );
        assert!(held_revision < barrier_revision);
        assert_eq!(guardian.state(), GuardianState::Active);
        assert_durable(&mut store, &replacement);
        let executions_before = audit.borrow().len();
        let mut stale = PolicyLoop::from_committed_for_guardian(held, &guardian, current).unwrap();
        for _ in 0..3 {
            assert!(matches!(
                stale.dispatch_next_to_guardian(&mut guardian, current, MonotonicTick::new(99)),
                Err(GuardianLoopError::Guardian(
                    GuardianError::StaleDispatchRevision
                ))
            ));
            assert_eq!(stale.summary().revision, held_revision);
            assert_eq!(stale.status(), LoopStatus::Running);
            assert_eq!(guardian.retained_results().len(), 0);
            assert_eq!(guardian.state(), GuardianState::Active);
            assert_eq!(audit.borrow().len(), executions_before);
        }
        let returned = stale.issue_next().unwrap();
        assert_eq!(returned.key().attempt_id(), AttemptId::new(id));
        assert_eq!(returned.key().revision(), held_revision);
        assert_eq!(returned.key().ordinal(), 0);
        assert_eq!(returned.effect(), &held_effect);
        assert!(stale.issue_next().is_none());
        assert!(!audit
            .borrow()
            .iter()
            .any(|effect| matches!(effect, OsEffect::SignalExactIdentity { .. })));
        assert_durable(&mut store, &replacement);
    }
}

#[test]
fn coordinated_signal_retains_until_durable_settlement() {
    let mut store = TestStore::in_memory().unwrap();
    let committed = one_signal_plan(&mut store, 104);
    let (mut guardian, session) = guardian_fixture(104, FakeOs::default());
    let mut policy =
        PolicyLoop::from_committed_for_guardian(committed, &guardian, session).unwrap();
    let key = coordinated_step(&mut policy, &mut guardian, session, &mut store);
    assert!(guardian.retained_result(key).is_some());
    let progress = policy
        .finish_guardian_signal_round(&mut guardian, session, &mut store)
        .unwrap();
    assert_eq!(progress.acknowledge_keys().collect::<Vec<_>>(), vec![key]);
    assert!(guardian.retained_result(key).is_none());
    assert_eq!(policy.summary().phase, Phase::VerifyingCleanup);
    assert_durable(&mut store, &policy);
}

#[test]
fn coordinated_failed_signal_settlement_keeps_guardian_evidence() {
    let mut store = TestStore::in_memory().unwrap();
    let committed = one_signal_plan(&mut store, 105);
    let (mut guardian, session) = guardian_fixture(105, FakeOs::default());
    let mut policy =
        PolicyLoop::from_committed_for_guardian(committed, &guardian, session).unwrap();
    let key = coordinated_step(&mut policy, &mut guardian, session, &mut store);
    store.fail = true;
    assert!(matches!(
        policy.finish_guardian_signal_round(&mut guardian, session, &mut store),
        Err(GuardianLoopError::Persistence(_))
    ));
    assert_eq!(guardian.state(), GuardianState::AuthorityLost);
    assert!(guardian.retained_result(key).is_some());
}

#[test]
fn coordinated_query_failure_is_retained_through_interruption() {
    let mut store = TestStore::in_memory().unwrap();
    let committed = discovery(&mut store, 106, root());
    let (mut guardian, session) = guardian_fixture(
        106,
        FakeOs {
            children: Err(OsQueryError::PermissionDenied),
            ..FakeOs::default()
        },
    );
    let mut policy =
        PolicyLoop::from_committed_for_guardian(committed, &guardian, session).unwrap();
    let key = coordinated_step(&mut policy, &mut guardian, session, &mut store);
    assert!(guardian.retained_result(key).is_some());
    assert_eq!(guardian.state(), GuardianState::DetachedFenced);
    policy
        .interrupt_with_guardian(&mut guardian, session, &mut store)
        .unwrap();
    assert!(guardian.retained_result(key).is_some());
    let slots: Vec<_> = guardian.inventory().collect();
    let current = policy
        .begin_guardian_reconciliation(&mut guardian, session, &slots)
        .unwrap();
    policy
        .classify_superseded_with_guardian(&mut guardian, current, key)
        .unwrap();
    assert!(guardian.retained_result(key).is_none());
    assert_eq!(policy.summary().phase, Phase::Interrupted);
}

#[test]
fn coordinated_failed_interruption_and_deadline_never_restore_authority() {
    for deadline in [false, true] {
        let mut store = TestStore::in_memory().unwrap();
        let committed = one_signal_plan(&mut store, 107);
        let (mut guardian, session) = guardian_fixture(107, FakeOs::default());
        let mut policy =
            PolicyLoop::from_committed_for_guardian(committed, &guardian, session).unwrap();
        let held = policy.issue_next().unwrap();
        store.fail = true;
        let result = if deadline {
            policy.deadline_with_guardian(
                &mut guardian,
                session,
                MonotonicTick::new(100),
                &mut store,
            )
        } else {
            policy.interrupt_with_guardian(&mut guardian, session, &mut store)
        };
        assert!(matches!(result, Err(GuardianLoopError::Persistence(_))));
        assert_eq!(guardian.state(), GuardianState::AuthorityLost);
        assert_eq!(policy.status(), LoopStatus::FailStopped);
        assert!(matches!(
            held.effect(),
            OsEffect::SignalExactIdentity { .. }
        ));
        assert_eq!(guardian.retained_results().len(), 0);
    }
}

#[test]
fn recovery_barrier_failure_is_fail_stop_not_activation() {
    for persist_failure in [false, true] {
        let mut store = TestStore::in_memory().unwrap();
        let committed = discovery(&mut store, 108, root());
        let (mut guardian, old) = guardian_fixture(108, FakeOs::default());
        let mut policy =
            PolicyLoop::from_committed_for_guardian(committed, &guardian, old).unwrap();
        policy
            .interrupt_with_guardian(&mut guardian, old, &mut store)
            .unwrap();
        let slots: Vec<_> = guardian.inventory().collect();
        let current = policy
            .begin_guardian_reconciliation(&mut guardian, old, &slots)
            .unwrap();
        policy
            .recover_with_guardian(&mut guardian, current, &mut store)
            .unwrap();
        if persist_failure {
            for _ in 0..2 {
                coordinated_step(&mut policy, &mut guardian, current, &mut store);
            }
            store.fail = true;
        }
        assert!(policy
            .finish_recovery_with_guardian(
                &mut guardian,
                current,
                MonotonicTick::new(99),
                &mut store
            )
            .is_err());
        assert_eq!(guardian.state(), GuardianState::AuthorityLost);
        assert_eq!(policy.status(), LoopStatus::FailStopped);
    }
}

struct WrongEffectExecutor {
    actual: Option<ExecutedEffect>,
    received: Option<DispatchTicket>,
}

impl GuardianExecutor for WrongEffectExecutor {
    type ChildSlot = ();

    fn execute(
        &mut self,
        ticket: DispatchTicket,
        _slots: &mut BTreeMap<ChildSlotId, ()>,
    ) -> ExecutedEffect {
        self.received = Some(ticket);
        self.actual.take().unwrap()
    }

    fn poll_observation_only(&self, _slot: &()) -> ChildSlotObservation {
        ChildSlotObservation::Running
    }
}

#[test]
fn coordinated_wrong_result_is_quarantined_and_never_restored() {
    let mut store = TestStore::in_memory().unwrap();
    let (state, mut batch) = start(109, 1, root())
        .persist(&mut store)
        .unwrap()
        .into_dispatch();
    let actual = execute(&mut FakeOs::default(), batch.next().unwrap());
    let committed = state
        .record_resource_sample(sample(0))
        .unwrap()
        .persist(&mut store)
        .unwrap();
    let (mut guardian, session) = Guardian::new(
        AttemptId::new(109),
        MonotonicTick::new(100),
        WrongEffectExecutor {
            actual: Some(actual),
            received: None,
        },
        Vec::new(),
    )
    .unwrap();
    let mut policy =
        PolicyLoop::from_committed_for_guardian(committed, &guardian, session).unwrap();
    assert!(matches!(
        policy.dispatch_next_to_guardian(&mut guardian, session, MonotonicTick::new(1)),
        Err(GuardianLoopError::Guardian(
            GuardianError::ExecutorCorrelationMismatch
        ))
    ));
    assert_eq!(guardian.state(), GuardianState::AuthorityLost);
    assert_eq!(policy.status(), LoopStatus::FailStopped);
    assert!(policy.issue_next().is_none());
    assert_eq!(guardian.retained_results().len(), 0);
    let quarantined = guardian.quarantined_result().unwrap();
    assert_ne!(
        quarantined.actual_result().key(),
        quarantined.requested_key()
    );
    assert_ne!(
        quarantined.requested_effect(),
        &quarantined.actual_result().completion().effect()
    );
}

#[test]
fn stale_recovery_session_reports_combined_fail_stop_and_blocks_dispatch() {
    let mut store = TestStore::in_memory().unwrap();
    let committed = discovery(&mut store, 110, root());
    let (mut guardian, old) = guardian_fixture(110, FakeOs::default());
    let mut policy = PolicyLoop::from_committed_for_guardian(committed, &guardian, old).unwrap();
    policy
        .interrupt_with_guardian(&mut guardian, old, &mut store)
        .unwrap();
    let slots: Vec<_> = guardian.inventory().collect();
    let current = policy
        .begin_guardian_reconciliation(&mut guardian, old, &slots)
        .unwrap();
    policy
        .recover_with_guardian(&mut guardian, current, &mut store)
        .unwrap();
    assert!(matches!(
        policy.finish_recovery_with_guardian(
            &mut guardian,
            old,
            MonotonicTick::new(99),
            &mut store
        ),
        Err(GuardianLoopError::FencingFailed {
            fencing: GuardianError::StaleSession,
            ..
        })
    ));
    assert_eq!(policy.status(), LoopStatus::FailStopped);
    assert!(policy.issue_next().is_none());
    assert_eq!(guardian.state(), GuardianState::Reconciling);
}

#[test]
fn pure_loop_never_lazily_binds_to_a_guardian() {
    let mut store = TestStore::in_memory().unwrap();
    let mut policy = PolicyLoop::from_committed(one_signal_plan(&mut store, 111));
    let (mut guardian, session, audit) = audited_guardian_fixture(111, FakeOs::default());
    let before = policy.summary();
    assert!(matches!(
        policy.dispatch_next_to_guardian(&mut guardian, session, MonotonicTick::new(1)),
        Err(GuardianLoopError::Policy(LoopFailure::GuardianNotBound))
    ));
    assert!(matches!(
        policy.interrupt_with_guardian(&mut guardian, session, &mut store),
        Err(GuardianLoopError::Policy(LoopFailure::GuardianNotBound))
    ));
    assert!(matches!(
        policy.recover_with_guardian(&mut guardian, session, &mut store),
        Err(GuardianLoopError::Policy(LoopFailure::GuardianNotBound))
    ));
    assert!(matches!(
        policy.finish_recovery_with_guardian(
            &mut guardian,
            session,
            MonotonicTick::new(100),
            &mut store
        ),
        Err(GuardianLoopError::Policy(LoopFailure::GuardianNotBound))
    ));
    assert_eq!(policy.summary(), before);
    assert_eq!(guardian.state(), GuardianState::Active);
    assert!(audit.borrow().is_empty());
    assert!(policy.issue_next().is_some());
    assert_durable(&mut store, &policy);
}

#[test]
fn recovery_rejects_guardian_substitution_before_persistence_or_rebinding() {
    let mut store = TestStore::in_memory().unwrap();
    let committed = discovery(&mut store, 112, root());
    let (mut guardian, old, audit) = audited_guardian_fixture(112, FakeOs::default());
    let mut policy = PolicyLoop::from_committed_for_guardian(committed, &guardian, old).unwrap();
    policy
        .interrupt_with_guardian(&mut guardian, old, &mut store)
        .unwrap();
    let slots: Vec<_> = guardian.inventory().collect();
    let current = policy
        .begin_guardian_reconciliation(&mut guardian, old, &slots)
        .unwrap();
    let (mut other, other_old, other_audit) = audited_guardian_fixture(112, FakeOs::default());
    let other_slots: Vec<_> = other.inventory().collect();
    other.controller_lost(other_old).unwrap();
    let other_current = other
        .begin_reconciliation(other.instance_id(), &other_slots, policy.summary().revision)
        .unwrap();
    let before = policy.summary();
    assert!(matches!(
        policy.recover_with_guardian(&mut other, other_current, &mut store),
        Err(GuardianLoopError::Guardian(GuardianError::InstanceMismatch))
    ));
    assert_eq!(policy.summary(), before);
    assert_durable(&mut store, &policy);
    assert_eq!(other.state(), GuardianState::Reconciling);
    assert_eq!(other.inventory().collect::<Vec<_>>(), other_slots);
    policy
        .recover_with_guardian(&mut guardian, current, &mut store)
        .unwrap();
    assert_eq!(policy.summary().phase, Phase::Recovering);
    assert_eq!(guardian.inventory().collect::<Vec<_>>(), slots);
    assert_eq!(guardian.original_deadline(), MonotonicTick::new(100));
    assert!(audit.borrow().is_empty());
    assert!(other_audit.borrow().is_empty());
    for _ in 0..2 {
        coordinated_step(&mut policy, &mut guardian, current, &mut store);
    }
    policy
        .finish_recovery_with_guardian(&mut guardian, current, MonotonicTick::new(99), &mut store)
        .unwrap();
    assert_eq!(guardian.state(), GuardianState::Active);
    let recovered = policy.summary();
    let other_before = other.summary();
    let executions_before = audit.borrow().len();
    assert!(matches!(
        policy.dispatch_next_to_guardian(&mut other, other_current, MonotonicTick::new(99)),
        Err(GuardianLoopError::Guardian(GuardianError::InstanceMismatch))
    ));
    assert!(matches!(
        policy.finish_recovery_with_guardian(
            &mut other,
            other_current,
            MonotonicTick::new(99),
            &mut store
        ),
        Err(GuardianLoopError::Guardian(GuardianError::InstanceMismatch))
    ));
    assert_eq!(policy.summary(), recovered);
    assert_eq!(other.summary(), other_before);
    assert_eq!(guardian.state(), GuardianState::Active);
    assert_eq!(audit.borrow().len(), executions_before);
    assert!(other_audit.borrow().is_empty());
    assert_durable(&mut store, &policy);
}

#[test]
fn loaded_interrupted_recovery_binds_origin_and_commits_fresh_barrier() {
    let mut store = TestStore::in_memory().unwrap();
    let committed = discovery(&mut store, 113, root());
    let (mut guardian, old) = guardian_fixture(113, FakeOs::default());
    let mut original = PolicyLoop::from_committed_for_guardian(committed, &guardian, old).unwrap();
    let old_key = original
        .dispatch_next_to_guardian(&mut guardian, old, MonotonicTick::new(1))
        .unwrap()
        .unwrap();
    original
        .interrupt_with_guardian(&mut guardian, old, &mut store)
        .unwrap();
    let interrupted_revision = original.summary().revision;
    let slots: Vec<_> = guardian.inventory().collect();
    drop(original);
    let current = begin_loaded_reconciliation(&mut store, &mut guardian, old);
    let loaded = store.load_attempt(AttemptId::new(113)).unwrap().unwrap();
    assert_eq!(loaded.phase(), Phase::Interrupted);
    assert_eq!(loaded.persistence_revision(), interrupted_revision);
    let recovery = checkpoint_recovery(loaded).persist(&mut store).unwrap();
    assert_eq!(recovery.checkpoint().phase(), Phase::Recovering);
    let mut policy =
        PolicyLoop::from_committed_recovery_for_guardian(recovery, &guardian, current).unwrap();
    policy
        .classify_superseded_with_guardian(&mut guardian, current, old_key)
        .unwrap();
    assert!(guardian.retained_result(old_key).is_none());
    assert!(matches!(
        policy.finish_recovery(MonotonicTick::new(1), &mut store),
        Err(LoopError::RoundInFlight)
    ));
    for _ in 0..2 {
        coordinated_step(&mut policy, &mut guardian, current, &mut store);
        assert_eq!(guardian.state(), GuardianState::Reconciling);
    }
    let before = policy.summary().revision;
    policy
        .finish_recovery_with_guardian(&mut guardian, current, MonotonicTick::new(99), &mut store)
        .unwrap();
    assert_eq!(policy.summary().revision, before.checked_next().unwrap());
    assert_eq!(guardian.state(), GuardianState::Active);
    assert_eq!(guardian.inventory().collect::<Vec<_>>(), slots);
    assert_eq!(guardian.original_deadline(), MonotonicTick::new(100));
    assert_durable(&mut store, &policy);
}

#[test]
fn resource_sampling_recovery_activates_before_resume_and_requires_new_discovery() {
    let mut store = TestStore::in_memory().unwrap();
    let committed = start(114, 2, root()).persist(&mut store).unwrap();
    let (mut guardian, old, audit) = audited_guardian_fixture(114, FakeOs::default());
    let mut policy = PolicyLoop::from_committed_for_guardian(committed, &guardian, old).unwrap();
    coordinated_step(&mut policy, &mut guardian, old, &mut store);
    policy
        .interrupt_with_guardian(&mut guardian, old, &mut store)
        .unwrap();
    let slots: Vec<_> = guardian.inventory().collect();
    let current = policy
        .begin_guardian_reconciliation(&mut guardian, old, &slots)
        .unwrap();
    policy
        .recover_with_guardian(&mut guardian, current, &mut store)
        .unwrap();
    assert_eq!(policy.summary().phase, Phase::ResourceSampling);
    let recovery_revision = policy.summary().revision;
    policy
        .finish_recovery_with_guardian(&mut guardian, current, MonotonicTick::new(1), &mut store)
        .unwrap();
    assert_eq!(policy.summary().revision, recovery_revision);
    assert_eq!(guardian.state(), GuardianState::Active);
    let resumed = coordinated_step(&mut policy, &mut guardian, current, &mut store);
    assert_eq!(resumed.revision(), recovery_revision);
    assert_eq!(
        audit.borrow().as_slice(),
        [
            OsEffect::SampleResources { sequence: 0 },
            OsEffect::SampleResources { sequence: 1 }
        ]
    );
    assert!(policy
        .request_owned_signals(Signal::Kill, MonotonicTick::new(1), &mut store)
        .is_err());
    let discovery = coordinated_step(&mut policy, &mut guardian, current, &mut store);
    assert!(discovery.revision() > recovery_revision);
    policy.finish_ownership_scan(&mut store).unwrap();
    coordinated_step(&mut policy, &mut guardian, current, &mut store);
    assert_eq!(audit.borrow()[2], OsEffect::ListChildren { parent: root() });
    assert_eq!(
        audit.borrow()[3],
        OsEffect::ObserveIdentity { identity: root() }
    );
    policy
        .request_owned_signals(Signal::Terminate, MonotonicTick::new(1), &mut store)
        .unwrap();
    coordinated_step(&mut policy, &mut guardian, current, &mut store);
    assert_eq!(
        audit.borrow()[4],
        OsEffect::SignalExactIdentity {
            identity: root(),
            signal: Signal::Terminate
        }
    );
    assert_eq!(guardian.inventory().collect::<Vec<_>>(), slots);
    assert_eq!(store.evidence().deadline(), MonotonicTick::new(100));
    assert_durable(&mut store, &policy);
}

#[test]
fn expired_recovery_finish_never_activates_or_dispatches() {
    for (id, resources) in [(115, false), (116, true)] {
        for expired in [100, 101] {
            let mut store = TestStore::in_memory().unwrap();
            let committed = if resources {
                start(id, 2, root()).persist(&mut store).unwrap()
            } else {
                discovery(&mut store, id, root())
            };
            let (mut guardian, old, audit) = audited_guardian_fixture(id, FakeOs::default());
            let instance = guardian.instance_id();
            let mut policy =
                PolicyLoop::from_committed_for_guardian(committed, &guardian, old).unwrap();
            policy
                .interrupt_with_guardian(&mut guardian, old, &mut store)
                .unwrap();
            let slots: Vec<_> = guardian.inventory().collect();
            let current = policy
                .begin_guardian_reconciliation(&mut guardian, old, &slots)
                .unwrap();
            policy
                .recover_with_guardian(&mut guardian, current, &mut store)
                .unwrap();
            if !resources {
                for _ in 0..2 {
                    coordinated_step(&mut policy, &mut guardian, current, &mut store);
                }
            }
            let before = policy.summary().revision;
            let executions_before = audit.borrow().len();
            assert!(matches!(
                policy.finish_recovery_with_guardian(
                    &mut guardian,
                    current,
                    MonotonicTick::new(expired),
                    &mut store
                ),
                Err(GuardianLoopError::Guardian(GuardianError::DeadlineExpired))
            ));
            assert_eq!(guardian.state(), GuardianState::AuthorityLost);
            assert_eq!(policy.status(), LoopStatus::FailStopped);
            assert_eq!(policy.summary().revision, before);
            assert!(policy
                .dispatch_next_to_guardian(&mut guardian, current, MonotonicTick::new(expired))
                .is_err());
            assert_eq!(audit.borrow().len(), executions_before);
            assert_eq!(guardian.instance_id(), instance);
            assert_eq!(guardian.inventory().collect::<Vec<_>>(), slots);
            assert_eq!(guardian.original_deadline(), MonotonicTick::new(100));
            assert_eq!(store.evidence().deadline(), MonotonicTick::new(100));
            assert_durable(&mut store, &policy);
        }
    }
}

#[test]
fn oversized_owned_signal_batch_closes_out_without_partial_dispatch() {
    assert_eq!(MAX_OWNED_SIGNAL_BATCH, MAX_RETAINED_RESULTS);
    let mut store = TestStore::in_memory().unwrap();
    let children: Vec<_> = (1_000..1_128).collect();
    let transition = cleaning_transition(&mut store, 117, &children);
    let mut state = commit(&mut store, transition);
    let identities: Vec<_> = state
        .ownership_tree()
        .unwrap()
        .identities()
        .keys()
        .copied()
        .collect();
    assert_eq!(identities.len(), MAX_OWNED_SIGNAL_BATCH + 1);
    for identity in identities {
        state = commit(
            &mut store,
            state
                .identity_observed(
                    identity,
                    IdentityObservation::SameIdentity {
                        current_parent: None,
                    },
                )
                .unwrap(),
        );
    }
    let tree = state.ownership_tree().unwrap().clone();
    let resources = state.resource_evidence().cloned();
    let committed = state
        .identity_observed(
            root(),
            IdentityObservation::SameIdentity {
                current_parent: None,
            },
        )
        .unwrap()
        .persist(&mut store)
        .unwrap();
    assert!(committed.after_persist().is_empty());
    let (mut guardian, session, audit) = audited_guardian_fixture(117, FakeOs::default());
    let mut policy =
        PolicyLoop::from_committed_for_guardian(committed, &guardian, session).unwrap();
    let progress = policy
        .request_owned_signals(Signal::Terminate, MonotonicTick::new(1), &mut store)
        .unwrap();
    assert_eq!(
        progress.guardian_fence(),
        Some(LoopFenceReason::SignalCapacityExceeded)
    );
    assert_eq!(
        policy.summary().phase,
        Phase::AdministrativeCloseoutRequired
    );
    assert_eq!(
        store.evidence().cleanup_outcome(),
        CleanupOutcome::UnresolvedRisk
    );
    assert_eq!(store.evidence().ownership_tree(), Some(&tree));
    assert_eq!(store.evidence().resource_evidence().cloned(), resources);
    assert_eq!(store.evidence().deadline(), MonotonicTick::new(100));
    assert!(policy.issue_next().is_none());
    assert!(policy
        .dispatch_next_to_guardian(&mut guardian, session, MonotonicTick::new(1))
        .unwrap()
        .is_none());
    assert!(audit.borrow().is_empty());
    assert_eq!(guardian.retained_results().len(), 0);
    let loaded = store.load_attempt(AttemptId::new(117)).unwrap().unwrap();
    assert_eq!(loaded.phase(), Phase::AdministrativeCloseoutRequired);
    assert_eq!(loaded.cleanup_outcome(), CleanupOutcome::UnresolvedRisk);
}

#[test]
fn loaded_sampling_recovery_uses_committed_origin_without_an_owned_tree() {
    let mut store = TestStore::in_memory().unwrap();
    let committed = start(118, 2, root()).persist(&mut store).unwrap();
    let (mut guardian, old, audit) = audited_guardian_fixture(118, FakeOs::default());
    let mut original = PolicyLoop::from_committed_for_guardian(committed, &guardian, old).unwrap();
    original
        .interrupt_with_guardian(&mut guardian, old, &mut store)
        .unwrap();
    drop(original);
    let current = begin_loaded_reconciliation(&mut store, &mut guardian, old);
    let loaded = store.load_attempt(AttemptId::new(118)).unwrap().unwrap();
    let committed = checkpoint_recovery(loaded).persist(&mut store).unwrap();
    assert_eq!(committed.checkpoint().phase(), Phase::ResourceSampling);
    assert!(committed.checkpoint().ownership_tree().is_none());
    let mut policy =
        PolicyLoop::from_committed_recovery_for_guardian(committed, &guardian, current).unwrap();
    policy
        .finish_recovery_with_guardian(&mut guardian, current, MonotonicTick::new(1), &mut store)
        .unwrap();
    assert_eq!(guardian.state(), GuardianState::Active);
    assert!(audit.borrow().is_empty());
    coordinated_step(&mut policy, &mut guardian, current, &mut store);
    assert_eq!(
        audit.borrow().as_slice(),
        [OsEffect::SampleResources { sequence: 0 }]
    );
    assert_eq!(policy.summary().phase, Phase::ResourceSampling);
    assert_eq!(guardian.original_deadline(), MonotonicTick::new(100));
    assert_durable(&mut store, &policy);
}

#[test]
fn reconciliation_begin_requires_a_committed_interruption_bound_to_the_same_guardian() {
    let mut store = TestStore::in_memory().unwrap();
    let committed = start(126, 1, root()).persist(&mut store).unwrap();
    let (mut guardian, old, audit) = audited_guardian_fixture(126, FakeOs::default());
    let mut policy = PolicyLoop::from_committed_for_guardian(committed, &guardian, old).unwrap();
    guardian.controller_lost(old).unwrap();
    let slots: Vec<_> = guardian.inventory().collect();
    let before = policy.summary();
    assert_eq!(
        policy.begin_guardian_reconciliation(&mut guardian, old, &slots),
        Err(GuardianLoopError::Policy(LoopFailure::RecoveryNotAdmitted))
    );
    let loaded = store.load_attempt(AttemptId::new(126)).unwrap().unwrap();
    assert!(matches!(
        PolicyLoop::from_loaded_interruption_for_guardian(loaded, &guardian, old),
        Err(GuardianLoopError::Policy(LoopFailure::RecoveryNotAdmitted))
    ));
    assert_eq!(policy.summary(), before);
    assert_eq!(guardian.state(), GuardianState::DetachedFenced);
    assert_eq!(guardian.reconciliation_floor(), None);
    policy
        .interrupt_with_guardian(&mut guardian, old, &mut store)
        .unwrap();
    let floor = policy.summary().revision;
    let (mut other, other_session, other_audit) = audited_guardian_fixture(126, FakeOs::default());
    other.controller_lost(other_session).unwrap();
    assert_eq!(
        policy.begin_guardian_reconciliation(&mut other, other_session, &slots),
        Err(GuardianLoopError::Guardian(GuardianError::InstanceMismatch))
    );
    assert_eq!(other.state(), GuardianState::DetachedFenced);
    let current = policy
        .begin_guardian_reconciliation(&mut guardian, old, &slots)
        .unwrap();
    assert_eq!(guardian.reconciliation_floor(), Some(floor));
    assert_eq!(policy.summary().revision, floor);
    assert_eq!(current.generation(), old.generation() + 1);
    assert!(audit.borrow().is_empty());
    assert!(other_audit.borrow().is_empty());
    assert_durable(&mut store, &policy);
}

#[test]
fn recovery_cannot_use_a_different_committed_interruption_than_the_guardian_floor() {
    let mut store = TestStore::in_memory().unwrap();
    let committed = start(127, 1, root()).persist(&mut store).unwrap();
    let (mut guardian, old, audit) = audited_guardian_fixture(127, FakeOs::default());
    let mut policy = PolicyLoop::from_committed_for_guardian(committed, &guardian, old).unwrap();
    policy
        .interrupt_with_guardian(&mut guardian, old, &mut store)
        .unwrap();
    let before = policy.summary();
    let slots: Vec<_> = guardian.inventory().collect();
    let floor = before.revision.checked_next().unwrap();
    let current = guardian
        .begin_reconciliation(guardian.instance_id(), &slots, floor)
        .unwrap();
    assert!(matches!(
        policy.recover_with_guardian(&mut guardian, current, &mut store),
        Err(GuardianLoopError::Policy(LoopFailure::RecoveryNotAdmitted))
    ));
    assert_eq!(policy.summary(), before);
    assert_eq!(guardian.state(), GuardianState::Reconciling);
    assert_eq!(guardian.reconciliation_floor(), Some(floor));
    assert!(audit.borrow().is_empty());
    assert_durable(&mut store, &policy);
}

#[test]
fn held_interruption_cannot_restart_after_unfinished_reconciliation() {
    let mut store = TestStore::in_memory().unwrap();
    let committed = start(128, 1, root()).persist(&mut store).unwrap();
    let (mut guardian, old, audit) = audited_guardian_fixture(128, FakeOs::default());
    let mut policy = PolicyLoop::from_committed_for_guardian(committed, &guardian, old).unwrap();
    policy
        .interrupt_with_guardian(&mut guardian, old, &mut store)
        .unwrap();
    let held = store.load_attempt(AttemptId::new(128)).unwrap().unwrap();
    let held_revision = held.persistence_revision();
    let slots: Vec<_> = guardian.inventory().collect();
    let first = policy
        .begin_guardian_reconciliation(&mut guardian, old, &slots)
        .unwrap();
    policy
        .recover_with_guardian(&mut guardian, first, &mut store)
        .unwrap();
    policy
        .interrupt_with_guardian(&mut guardian, first, &mut store)
        .unwrap();
    let current_revision = loaded_revision(&mut store, 128);
    assert!(current_revision > held_revision);

    let mut historical =
        PolicyLoop::from_loaded_interruption_for_guardian(held, &guardian, first).unwrap();
    assert!(matches!(
        historical.begin_guardian_reconciliation(&mut guardian, first, &slots),
        Err(GuardianLoopError::Guardian(
            GuardianError::StaleDispatchRevision
        ))
    ));
    assert_eq!(guardian.state(), GuardianState::DetachedFenced);
    assert_eq!(guardian.reconciliation_floor(), None);

    let loaded = store.load_attempt(AttemptId::new(128)).unwrap().unwrap();
    let mut current =
        PolicyLoop::from_loaded_interruption_for_guardian(loaded, &guardian, first).unwrap();
    let second = current
        .begin_guardian_reconciliation(&mut guardian, first, &slots)
        .unwrap();
    assert_eq!(second.generation(), first.generation() + 1);
    assert_eq!(guardian.reconciliation_floor(), Some(current_revision));
    assert!(audit.borrow().is_empty());
}

#[test]
fn historical_sampling_recovery_cannot_reactivate_a_later_interruption() {
    for retain_bound_loop in [false, true] {
        let mut store = TestStore::in_memory().unwrap();
        let committed = start(124, 1, root()).persist(&mut store).unwrap();
        let (mut guardian, old, audit) = audited_guardian_fixture(124, FakeOs::default());
        let mut original =
            PolicyLoop::from_committed_for_guardian(committed, &guardian, old).unwrap();
        original
            .interrupt_with_guardian(&mut guardian, old, &mut store)
            .unwrap();
        let slots: Vec<_> = guardian.inventory().collect();
        let first = original
            .begin_guardian_reconciliation(&mut guardian, old, &slots)
            .unwrap();
        let loaded = store.load_attempt(AttemptId::new(124)).unwrap().unwrap();
        let historical = checkpoint_recovery(loaded).persist(&mut store).unwrap();
        let historical_revision = historical.checkpoint().persistence_revision().unwrap();
        let old_state = historical.checkpoint().clone();
        let (held_recovery, held_loop) = if retain_bound_loop {
            (
                None,
                Some(
                    PolicyLoop::from_committed_recovery_for_guardian(historical, &guardian, first)
                        .unwrap(),
                ),
            )
        } else {
            (Some(historical), None)
        };

        guardian.controller_lost(first).unwrap();
        commit(&mut store, old_state.interrupt().unwrap());
        let second = begin_loaded_reconciliation(&mut store, &mut guardian, first);
        let loaded = store.load_attempt(AttemptId::new(124)).unwrap().unwrap();
        let recovery = checkpoint_recovery(loaded).persist(&mut store).unwrap();
        let recovered_state = recovery.checkpoint().clone();
        let mut newer =
            PolicyLoop::from_committed_recovery_for_guardian(recovery, &guardian, second).unwrap();
        newer
            .finish_recovery_with_guardian(&mut guardian, second, MonotonicTick::new(1), &mut store)
            .unwrap();
        let installed_revision = newer.summary().revision;
        assert!(installed_revision > historical_revision);
        assert_eq!(guardian.state(), GuardianState::Active);
        assert_eq!(guardian.reconciliation_floor(), None);

        let unbound_signal = commit_root_termination(&mut store, recovered_state);
        let signal_revision = unbound_signal.checkpoint().persistence_revision().unwrap();
        assert!(signal_revision > installed_revision);
        assert!(matches!(
            unbound_signal.after_persist(),
            [OsEffect::SignalExactIdentity { .. }]
        ));
        guardian.controller_lost(second).unwrap();
        let loaded = store.load_attempt(AttemptId::new(124)).unwrap().unwrap();
        let interruption = checkpoint_recovery(loaded).persist(&mut store).unwrap();
        let floor = interruption.checkpoint().persistence_revision().unwrap();
        assert!(floor > signal_revision);
        let current = begin_loaded_reconciliation(&mut store, &mut guardian, second);
        assert_eq!(guardian.reconciliation_floor(), Some(floor));
        let durable_revision = loaded_revision(&mut store, 124);

        if let Some(historical) = held_recovery {
            assert!(matches!(
                PolicyLoop::from_committed_recovery_for_guardian(historical, &guardian, current),
                Err(GuardianLoopError::Policy(LoopFailure::RecoveryNotAdmitted))
            ));
            assert_eq!(guardian.state(), GuardianState::Reconciling);
            assert_eq!(guardian.reconciliation_floor(), Some(floor));
        }
        if let Some(mut historical) = held_loop {
            assert!(historical
                .finish_recovery_with_guardian(
                    &mut guardian,
                    current,
                    MonotonicTick::new(1),
                    &mut store
                )
                .is_err());
            assert_eq!(guardian.state(), GuardianState::AuthorityLost);
            assert_eq!(guardian.reconciliation_floor(), None);
            assert_eq!(historical.status(), LoopStatus::FailStopped);
            assert!(historical.issue_next().is_none());
        }
        assert!(
            PolicyLoop::from_committed_for_guardian(unbound_signal, &guardian, current).is_err()
        );
        assert_eq!(loaded_revision(&mut store, 124), durable_revision);
        assert!(audit.borrow().is_empty());
        assert_eq!(guardian.retained_results().len(), 0);
        assert_eq!(guardian.inventory().collect::<Vec<_>>(), slots);
        assert_eq!(guardian.original_deadline(), MonotonicTick::new(100));
    }
}

#[test]
fn crash_loaded_interruption_refresh_requires_interrupted_phase() {
    let mut store = TestStore::in_memory().unwrap();
    let active = start(126, 1, root()).persist(&mut store).unwrap();
    assert_eq!(
        store
            .load_attempt(AttemptId::new(126))
            .unwrap()
            .unwrap()
            .refresh_interruption()
            .unwrap_err(),
        TransitionError::WrongPhase
    );
    let interrupted = active.checkpoint().clone().interrupt().unwrap();
    interrupted.persist(&mut store).unwrap();
    let loaded = store.load_attempt(AttemptId::new(126)).unwrap().unwrap();
    assert_eq!(loaded.phase(), Phase::Interrupted);
    let refreshed = loaded.refresh_interruption().unwrap();
    assert!(refreshed
        .persist(&mut store)
        .unwrap()
        .after_persist()
        .is_empty());

    let mut terminal_store = TestStore::in_memory().unwrap();
    let stable = ready(&mut terminal_store, 127);
    assert_eq!(stable.phase(), Phase::ReadyForDisruption);
    assert_eq!(
        terminal_store
            .load_attempt(AttemptId::new(127))
            .unwrap()
            .unwrap()
            .refresh_interruption()
            .unwrap_err(),
        TransitionError::WrongPhase
    );
}

#[test]
fn fresh_interruption_refresh_preserves_evidence_and_outpaces_unfinished_reconciliation() {
    let mut store = TestStore::in_memory().unwrap();
    let cleaning = cleaning_transition(&mut store, 128, &[43]);
    let active = commit(&mut store, cleaning);
    let interrupted = commit(&mut store, active.interrupt().unwrap());
    let original = interrupted.clone();
    let original_revision = original.persistence_revision().unwrap();
    let (mut guardian, old, audit) = audited_guardian_fixture(128, FakeOs::default());
    guardian.controller_lost(old).unwrap();
    let first = begin_loaded_reconciliation(&mut store, &mut guardian, old);
    assert_eq!(guardian.reconciliation_floor(), Some(original_revision));
    guardian.controller_lost(first).unwrap();

    let loaded = store.load_attempt(AttemptId::new(128)).unwrap().unwrap();
    let stale = loaded.refresh_interruption().unwrap();
    let stale_copy = stale.clone();
    let refreshed = stale.persist(&mut store).unwrap();
    let next = refreshed.checkpoint();
    assert!(refreshed.after_persist().is_empty());
    assert_eq!(next.phase(), Phase::Interrupted);
    assert_eq!(next.deadline(), original.deadline());
    assert_eq!(
        next.compatibility_boundary(),
        original.compatibility_boundary()
    );
    assert_eq!(next.cleanup_outcome(), original.cleanup_outcome());
    assert_eq!(next.ownership_tree(), original.ownership_tree());
    assert_eq!(
        next.persistence_revision(),
        original_revision.checked_next()
    );
    let refreshed_revision = next.persistence_revision().unwrap();
    let current = begin_loaded_reconciliation(&mut store, &mut guardian, first);
    assert_eq!(guardian.reconciliation_floor(), Some(refreshed_revision));
    assert_eq!(guardian.state(), GuardianState::Reconciling);
    assert!(audit.borrow().is_empty());

    let loaded = store.load_attempt(AttemptId::new(128)).unwrap().unwrap();
    assert!(matches!(
        PolicyLoop::from_committed_recovery_for_guardian(refreshed, &guardian, current),
        Err(GuardianLoopError::Policy(LoopFailure::RecoveryNotAdmitted))
    ));

    let recovery = checkpoint_recovery(loaded).persist(&mut store).unwrap();
    assert_eq!(recovery.checkpoint().phase(), Phase::Recovering);
    assert!(matches!(
        recovery.after_persist().first(),
        Some(OsEffect::ObserveIdentity { .. })
    ));
    assert!(PolicyLoop::from_committed_recovery_for_guardian(recovery, &guardian, current).is_ok());
    assert!(matches!(
        stale_copy.persist(&mut store),
        Err(ControllerStoreError::RevisionConflict)
    ));
}

#[test]
fn repeated_refresh_requires_fresh_loaded_revision() {
    let mut store = TestStore::in_memory().unwrap();
    let initial = start(129, 1, root()).persist(&mut store).unwrap();
    let original = commit(
        &mut store,
        initial.checkpoint().clone().interrupt().unwrap(),
    );
    let historical_recovery =
        checkpoint_recovery(store.load_attempt(AttemptId::new(129)).unwrap().unwrap());
    let first = store
        .load_attempt(AttemptId::new(129))
        .unwrap()
        .unwrap()
        .refresh_interruption()
        .unwrap()
        .persist(&mut store)
        .unwrap();
    let second = store
        .load_attempt(AttemptId::new(129))
        .unwrap()
        .unwrap()
        .refresh_interruption()
        .unwrap()
        .persist(&mut store)
        .unwrap();
    assert_eq!(
        first.checkpoint().persistence_revision(),
        original.persistence_revision().unwrap().checked_next()
    );
    assert_eq!(
        second.checkpoint().persistence_revision(),
        first
            .checkpoint()
            .persistence_revision()
            .unwrap()
            .checked_next()
    );
    assert_eq!(second.checkpoint().deadline(), original.deadline());
    assert!(first.after_persist().is_empty() && second.after_persist().is_empty());
    assert!(matches!(
        checkpoint_recovery(store.load_attempt(AttemptId::new(129)).unwrap().unwrap())
            .persist(&mut store)
            .unwrap()
            .after_persist(),
        [OsEffect::SampleResources { sequence: 0 }]
    ));
    assert!(matches!(
        historical_recovery.persist(&mut store),
        Err(ControllerStoreError::RevisionConflict)
    ));
}

#[test]
fn historical_ownership_recovery_origin_cannot_bind_a_new_interruption_floor() {
    let mut store = TestStore::in_memory().unwrap();
    let committed = discovery(&mut store, 125, root());
    let (mut guardian, old, audit) = audited_guardian_fixture(125, FakeOs::default());
    let mut original = PolicyLoop::from_committed_for_guardian(committed, &guardian, old).unwrap();
    original
        .interrupt_with_guardian(&mut guardian, old, &mut store)
        .unwrap();
    let first = begin_loaded_reconciliation(&mut store, &mut guardian, old);
    let loaded = store.load_attempt(AttemptId::new(125)).unwrap().unwrap();
    let historical = checkpoint_recovery(loaded).persist(&mut store).unwrap();
    assert_eq!(historical.checkpoint().phase(), Phase::Recovering);
    let latest = commit(
        &mut store,
        historical.checkpoint().clone().interrupt().unwrap(),
    );
    guardian.controller_lost(first).unwrap();
    let current = begin_loaded_reconciliation(&mut store, &mut guardian, first);
    let floor = latest.persistence_revision().unwrap();
    assert_eq!(guardian.reconciliation_floor(), Some(floor));
    assert!(matches!(
        PolicyLoop::from_committed_recovery_for_guardian(historical, &guardian, current),
        Err(GuardianLoopError::Policy(LoopFailure::RecoveryNotAdmitted))
    ));
    assert_eq!(guardian.state(), GuardianState::Reconciling);
    assert_eq!(guardian.reconciliation_floor(), Some(floor));
    assert!(audit.borrow().is_empty());
}

#[test]
fn ordinary_sampling_commit_is_not_recovery_activation_authority() {
    let mut store = TestStore::in_memory().unwrap();
    let committed = start(119, 2, root()).persist(&mut store).unwrap();
    let revision = committed.checkpoint().persistence_revision().unwrap();
    let (mut guardian, old, audit) = audited_guardian_fixture(119, FakeOs::default());
    guardian.controller_lost(old).unwrap();
    let loaded = store.load_attempt(AttemptId::new(119)).unwrap().unwrap();
    let interrupted = checkpoint_recovery(loaded).persist(&mut store).unwrap();
    let interruption_revision = interrupted.checkpoint().persistence_revision().unwrap();
    assert!(interruption_revision > revision);
    let current = begin_loaded_reconciliation(&mut store, &mut guardian, old);
    assert!(matches!(
        PolicyLoop::from_committed_recovery_for_guardian(committed, &guardian, current),
        Err(GuardianLoopError::Policy(LoopFailure::RecoveryNotAdmitted))
    ));
    assert_eq!(guardian.state(), GuardianState::Reconciling);
    assert_eq!(
        store
            .load_attempt(AttemptId::new(119))
            .unwrap()
            .unwrap()
            .persistence_revision(),
        interruption_revision
    );
    assert!(audit.borrow().is_empty());
}

#[test]
fn initial_guardian_binding_checks_attempt_deadline_and_active_session() {
    for (guardian_attempt, deadline, expected) in [
        (121, 100, GuardianError::AttemptMismatch),
        (120, 101, GuardianError::PermitMismatch),
    ] {
        let mut store = TestStore::in_memory().unwrap();
        let committed = start(120, 1, root()).persist(&mut store).unwrap();
        let audit = Rc::new(RefCell::new(Vec::new()));
        let (guardian, session) = Guardian::new(
            AttemptId::new(guardian_attempt),
            MonotonicTick::new(deadline),
            TestGuardianExecutor {
                os: FakeOs::default(),
                executions: Rc::clone(&audit),
            },
            vec![()],
        )
        .unwrap();
        assert!(matches!(
            PolicyLoop::from_committed_for_guardian(committed, &guardian, session),
            Err(GuardianLoopError::Guardian(error)) if error == expected
        ));
        assert_eq!(guardian.state(), GuardianState::Active);
        assert!(audit.borrow().is_empty());
    }
    let mut store = TestStore::in_memory().unwrap();
    let committed = start(120, 1, root()).persist(&mut store).unwrap();
    let (mut guardian, old, audit) = audited_guardian_fixture(120, FakeOs::default());
    guardian.controller_lost(old).unwrap();
    let loaded = store.load_attempt(AttemptId::new(120)).unwrap().unwrap();
    checkpoint_recovery(loaded).persist(&mut store).unwrap();
    let current = begin_loaded_reconciliation(&mut store, &mut guardian, old);
    assert!(matches!(
        PolicyLoop::from_committed_for_guardian(committed, &guardian, current),
        Err(GuardianLoopError::Policy(LoopFailure::RecoveryNotAdmitted))
    ));
    assert!(audit.borrow().is_empty());
}
