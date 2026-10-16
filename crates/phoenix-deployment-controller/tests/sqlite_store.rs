//! Isolated `SQLite` fixtures only: no processes, services, or application database.
use std::num::{NonZeroU32, NonZeroUsize};

use phoenix_deployment_controller::{
    AdministrativeRiskAcceptance, ArtifactDigest, AttemptId, AttemptStore, BirthToken,
    CandidateVerification, ChildIdentityObservation, CleanupOutcome, CompatibilityBoundary,
    ControllerSqliteStore, ControllerStoreError, CpuContentionBasisPoints, DeploymentAttempt,
    DiscoveryGap, FullCommitSha, IdentityObservation, MemoryPressure, MonotonicTick,
    ObservationFailure, OsEffect, OwnedIdentityStatus, Phase, ProcessId, ProcessIdentity,
    RecoveryDisposition, ResourceCapacity, ResourceContention, ResourcePolicy, ResourceSample,
    RetainedCandidateIdentity, Signal, StableChildEvidence, Transition, TransitionError,
};
use rusqlite::Connection;

fn identity(pid: u32, seconds: u64) -> ProcessIdentity {
    ProcessIdentity::new(
        ProcessId::new(NonZeroU32::new(pid).unwrap()),
        BirthToken::new(seconds, 999_999_999).unwrap(),
    )
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
            available_memory_bytes: u64::MAX,
            available_storage_bytes: u64::MAX,
        },
        contention: ResourceContention {
            cpu: CpuContentionBasisPoints::new(42).unwrap(),
            memory_pressure: MemoryPressure::Nominal,
        },
    }
}
fn roundtrip(store: &mut ControllerSqliteStore, transition: Transition) -> DeploymentAttempt {
    let committed = transition.persist(store).unwrap();
    let expected = committed.checkpoint();
    let loaded = store.load_attempt(expected.attempt_id()).unwrap().unwrap();
    assert_eq!(loaded.attempt_id(), expected.attempt_id());
    assert_eq!(loaded.phase(), expected.phase());
    committed.into_parts().0
}
fn recovery_checkpoint(disposition: RecoveryDisposition) -> Transition {
    match disposition {
        RecoveryDisposition::Checkpoint(transition) => *transition,
        RecoveryDisposition::Stable(stable) => {
            panic!("unexpected stable phase {:?}", stable.phase())
        }
    }
}

fn admitted(
    store: &mut ControllerSqliteStore,
    id: AttemptId,
    root: ProcessIdentity,
) -> DeploymentAttempt {
    let mut state = roundtrip(
        store,
        DeploymentAttempt::start(id, MonotonicTick::new(100), policy(2), root),
    );
    for sequence in 0..2 {
        state = roundtrip(
            store,
            state.record_resource_sample(sample(sequence)).unwrap(),
        );
    }
    assert_eq!(state.phase(), Phase::OwnershipDiscovery);
    state
}
fn cleaning(
    store: &mut ControllerSqliteStore,
    id: AttemptId,
    root: ProcessIdentity,
) -> DeploymentAttempt {
    let state = admitted(store, id, root);
    let state = roundtrip(store, state.child_scan_completed(root).unwrap());
    roundtrip(store, state.ownership_scan_completed().unwrap())
}
fn ready(
    store: &mut ControllerSqliteStore,
    id: AttemptId,
    root: ProcessIdentity,
) -> DeploymentAttempt {
    let state = cleaning(store, id, root);
    let state = roundtrip(store, state.cleanup_signals_settled().unwrap());
    let state = roundtrip(
        store,
        state
            .identity_observed(root, IdentityObservation::ExactIdentityAbsent)
            .unwrap(),
    );
    roundtrip(
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

fn add_failed_discoveries(
    store: &mut ControllerSqliteStore,
    mut state: DeploymentAttempt,
    root: ProcessIdentity,
) -> DeploymentAttempt {
    for (offset, failure) in [
        ObservationFailure::NoSuchProcess,
        ObservationFailure::PermissionDenied,
        ObservationFailure::TemporarilyUnavailable,
        ObservationFailure::Malformed,
    ]
    .into_iter()
    .enumerate()
    {
        let (transition, discovery) = state
            .child_discovered(
                root,
                identity(100 + u32::try_from(offset).unwrap(), 12).pid(),
            )
            .unwrap();
        state = roundtrip(store, transition);
        state = roundtrip(
            store,
            state
                .child_observed(discovery, ChildIdentityObservation::Unresolved(failure))
                .unwrap(),
        );
    }
    state
}

#[test]
fn sampling_roundtrip_reopen_and_unsigned_extremes() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("controller.sqlite");
    let mut store = ControllerSqliteStore::open(&path).unwrap();
    assert!(store.load_attempt(AttemptId::new(0)).unwrap().is_none());
    let id = AttemptId::new(u128::MAX);
    let root = identity(u32::MAX, u64::MAX);
    let policy = ResourcePolicy::new(
        NonZeroUsize::new(3).unwrap(),
        u64::MAX,
        u64::MAX,
        CpuContentionBasisPoints::new(10_000).unwrap(),
        MemoryPressure::Critical,
    );
    let state = roundtrip(
        &mut store,
        DeploymentAttempt::start(id, MonotonicTick::new(u64::MAX), policy, root),
    );
    let state = roundtrip(&mut store, state.record_resource_sample(sample(0)).unwrap());
    drop(store);
    let mut store = ControllerSqliteStore::open(&path).unwrap();
    let loaded = store.load_attempt(id).unwrap().unwrap();
    assert_eq!(loaded.attempt_id(), state.attempt_id());
    assert_eq!(loaded.phase(), state.phase());
    drop(state);
    let interruption = recovery_checkpoint(loaded.prepare_recovery().unwrap())
        .persist(&mut store)
        .unwrap();
    assert!(interruption.after_persist().is_empty());
    let state = interruption.into_parts().0;
    let transition = state.recover().unwrap().persist(&mut store).unwrap();
    assert_eq!(
        transition.after_persist(),
        &[OsEffect::SampleResources { sequence: 1 }]
    );
    let (state, _) = transition.into_parts();
    assert_eq!(state.deadline().get(), u64::MAX);
    let state = roundtrip(&mut store, state.record_resource_sample(sample(1)).unwrap());
    let state = roundtrip(&mut store, state.record_resource_sample(sample(2)).unwrap());
    assert_eq!(state.resource_evidence().unwrap().samples().len(), 3);
    let raw = Connection::open(path).unwrap();
    let (id_size, deadline_size): (i64, i64) = raw
        .query_row(
            "SELECT length(attempt_id),length(deadline) FROM attempts",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!((id_size, deadline_size), (16, 8));
    assert_eq!(
        raw.query_row(
            "SELECT hex(available_memory) FROM resource_samples LIMIT 1",
            [],
            |row| row.get::<_, String>(0)
        )
        .unwrap(),
        "FFFFFFFFFFFFFFFF"
    );
}

#[test]
fn owned_tree_pending_scans_observation_status_and_durable_discovery_gaps() {
    let mut store = ControllerSqliteStore::in_memory().unwrap();
    let root = identity(42, 10);
    let child = identity(43, 11);
    let external_parent = identity(99, u64::MAX);
    let mut state = admitted(&mut store, AttemptId::new(2), root);
    let (transition, discovery) = state.child_discovered(root, child.pid()).unwrap();
    state = roundtrip(&mut store, transition);
    state = roundtrip(
        &mut store,
        state
            .child_observed(
                discovery,
                ChildIdentityObservation::Stable(
                    StableChildEvidence::new(discovery, child, root).unwrap(),
                ),
            )
            .unwrap(),
    );
    state = add_failed_discoveries(&mut store, state, root);
    let (transition, gap) = state
        .child_discovered(child, identity(200, 12).pid())
        .unwrap();
    state = roundtrip(&mut store, transition);
    state = roundtrip(
        &mut store,
        state
            .child_observed(
                gap,
                ChildIdentityObservation::DifferentProcess(identity(200, 13)),
            )
            .unwrap(),
    );
    let (transition, unobserved) = state
        .child_discovered(child, identity(201, 12).pid())
        .unwrap();
    state = roundtrip(&mut store, transition);
    state = roundtrip(&mut store, state.interrupt().unwrap());
    state = roundtrip(&mut store, state.recover().unwrap());
    state = roundtrip(
        &mut store,
        state
            .identity_observed(
                root,
                IdentityObservation::SameIdentity {
                    current_parent: Some(external_parent),
                },
            )
            .unwrap(),
    );
    state = roundtrip(
        &mut store,
        state
            .identity_observed(
                child,
                IdentityObservation::Unresolved(ObservationFailure::PermissionDenied),
            )
            .unwrap(),
    );
    let tree = state.ownership_tree().unwrap();
    assert_eq!(tree.pending_child_scans().len(), 2);
    assert_eq!(tree.unresolved().len(), 6);
    assert_eq!(
        tree.unresolved()[&gap.id()].gap,
        DiscoveryGap::DifferentProcess(identity(200, 13))
    );
    assert_eq!(
        tree.unresolved()[&unobserved.id()].gap,
        DiscoveryGap::NotYetObserved
    );
    assert_eq!(
        tree.identities()[&root],
        OwnedIdentityStatus::Present {
            current_parent: Some(external_parent)
        }
    );
    assert_eq!(
        tree.identities()[&child],
        OwnedIdentityStatus::Uncertain(ObservationFailure::PermissionDenied)
    );
    // A fresh adapter cannot turn a previously failed epoch into ownership.
    state = roundtrip(
        &mut store,
        state
            .child_observed(
                gap,
                ChildIdentityObservation::Stable(
                    StableChildEvidence::new(gap, identity(200, 14), child).unwrap(),
                ),
            )
            .unwrap(),
    );
    assert!(!state
        .ownership_tree()
        .unwrap()
        .identities()
        .contains_key(&identity(200, 14)));
    let (transition, next) = state
        .child_discovered(root, identity(202, 15).pid())
        .unwrap();
    roundtrip(&mut store, transition);
    assert_eq!(next.id().get(), unobserved.id().get() + 1);
}

#[test]
fn interrupted_recovery_preserves_original_verifying_resume_and_deadline() {
    let mut store = ControllerSqliteStore::in_memory().unwrap();
    let root = identity(44, 20);
    let state = cleaning(&mut store, AttemptId::new(3), root);
    let mut state = roundtrip(&mut store, state.cleanup_signals_settled().unwrap());
    for _ in 0..2 {
        state = roundtrip(&mut store, state.interrupt().unwrap());
        state = roundtrip(&mut store, state.recover().unwrap());
        assert_eq!(state.phase(), Phase::Recovering);
        assert_eq!(state.deadline(), MonotonicTick::new(100));
    }
    state = roundtrip(
        &mut store,
        state
            .identity_observed(root, IdentityObservation::ExactIdentityAbsent)
            .unwrap(),
    );
    state = roundtrip(
        &mut store,
        state
            .recovery_reconciliation_completed(MonotonicTick::new(99))
            .unwrap(),
    );
    assert_eq!(state.phase(), Phase::VerifyingCleanup);
    state = roundtrip(
        &mut store,
        state
            .verification_completed(MonotonicTick::new(99))
            .unwrap(),
    );
    assert_eq!(state.cleanup_outcome(), CleanupOutcome::VerifiedClean);
}

#[test]
fn ready_retained_candidate_forward_recovery_and_every_verification_status() {
    let mut store = ControllerSqliteStore::in_memory().unwrap();
    let state = ready(&mut store, AttemptId::new(4), identity(45, 21));
    assert_eq!(
        state.compatibility_boundary(),
        CompatibilityBoundary::PreMigration
    );
    let state = roundtrip(
        &mut store,
        state.record_migration_invoked(candidate()).unwrap(),
    );
    assert_eq!(
        state.compatibility_boundary(),
        CompatibilityBoundary::CandidateMigrationInvoked(candidate())
    );
    let committed = state.recover().unwrap().persist(&mut store).unwrap();
    assert_eq!(
        committed.after_persist(),
        &[OsEffect::ReverifyRetainedCandidate {
            candidate: candidate()
        }]
    );
    let (mut state, _) = committed.into_parts();
    for verification in [
        CandidateVerification::Exact,
        CandidateVerification::Missing,
        CandidateVerification::Mismatch,
        CandidateVerification::Unavailable,
    ] {
        state = roundtrip(
            &mut store,
            state.record_candidate_verification(verification).unwrap(),
        );
        assert_eq!(state.forward_candidate(), Some(candidate()));
        state = roundtrip(&mut store, state.recover().unwrap());
        assert_eq!(state.candidate_verification(), None);
    }
}

#[test]
fn administrative_closeout_retains_risk_without_clean_claim_or_effects() {
    let mut store = ControllerSqliteStore::in_memory().unwrap();
    let id = AttemptId::new(5);
    let root = identity(46, 22);
    let state = admitted(&mut store, id, root);
    let (transition, discovery) = state
        .child_discovered(root, identity(47, 23).pid())
        .unwrap();
    let state = roundtrip(&mut store, transition);
    let state = roundtrip(
        &mut store,
        state
            .child_observed(
                discovery,
                ChildIdentityObservation::Unresolved(ObservationFailure::NoSuchProcess),
            )
            .unwrap(),
    );
    let state = roundtrip(
        &mut store,
        state.deadline_reached(MonotonicTick::new(100)).unwrap(),
    );
    let tree = state.ownership_tree().unwrap().clone();
    assert_eq!(state.phase(), Phase::AdministrativeCloseoutRequired);
    let RecoveryDisposition::Stable(stable) = store
        .load_attempt(id)
        .unwrap()
        .unwrap()
        .prepare_recovery()
        .unwrap()
    else {
        panic!("loaded closeout evidence cannot grant live mutation authority");
    };
    assert_eq!(stable.phase(), Phase::AdministrativeCloseoutRequired);
    let committed = stable
        .accept_administrative_risk(AdministrativeRiskAcceptance::for_attempt(id))
        .unwrap()
        .persist(&mut store)
        .unwrap();
    assert!(committed.after_persist().is_empty());
    let loaded = store.load_attempt(id).unwrap().unwrap();
    assert_eq!(loaded.phase(), Phase::AdministrativelyClosed);
    let RecoveryDisposition::Stable(stable) = loaded.prepare_recovery().unwrap() else {
        panic!("administrative closure must not grant recovery effects");
    };
    assert_eq!(stable.attempt_id(), id);
    assert_eq!(stable.phase(), Phase::AdministrativelyClosed);
    let state = committed.into_parts().0;
    assert_eq!(state.phase(), Phase::AdministrativelyClosed);
    assert_eq!(
        state.cleanup_outcome(),
        CleanupOutcome::AdministrativeRiskAccepted
    );
    assert_eq!(state.ownership_tree(), Some(&tree));
    assert!(state.recover().is_err());
}

#[test]
fn resource_blocked_roundtrips_without_fabricated_evidence() {
    let mut store = ControllerSqliteStore::in_memory().unwrap();
    let mut state = roundtrip(
        &mut store,
        DeploymentAttempt::start(
            AttemptId::new(6),
            MonotonicTick::new(100),
            policy(2),
            identity(48, 24),
        ),
    );
    for sequence in 0..2 {
        let mut measured = sample(sequence);
        measured.capacity.available_memory_bytes = 0;
        state = roundtrip(&mut store, state.record_resource_sample(measured).unwrap());
    }
    assert_eq!(state.phase(), Phase::ResourceBlocked);
    assert!(state.resource_evidence().is_none());
    assert!(state.ownership_tree().is_none());
    let RecoveryDisposition::Stable(stable) = store
        .load_attempt(state.attempt_id())
        .unwrap()
        .unwrap()
        .prepare_recovery()
        .unwrap()
    else {
        panic!("resource-blocked evidence cannot restart active work");
    };
    assert_eq!(stable.phase(), Phase::ResourceBlocked);
}

#[test]
fn failed_write_and_failed_commit_drop_capabilities_and_poison_until_reopen() {
    for fail_at_commit in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("controller.sqlite");
        let mut store = ControllerSqliteStore::open(&path).unwrap();
        let id = AttemptId::new(7);
        let before = roundtrip(
            &mut store,
            DeploymentAttempt::start(id, MonotonicTick::new(100), policy(2), identity(49, 25)),
        );
        let raw = Connection::open(&path).unwrap();
        if fail_at_commit {
            raw.execute_batch("CREATE TABLE commit_poison (attempt_id BLOB NOT NULL REFERENCES attempts(attempt_id) DEFERRABLE INITIALLY DEFERRED);
                CREATE TRIGGER fail_checkpoint AFTER INSERT ON resource_samples BEGIN
                    INSERT INTO commit_poison VALUES (zeroblob(16)); END;").unwrap();
        } else {
            raw.execute_batch(
                "CREATE TRIGGER fail_checkpoint BEFORE INSERT ON resource_samples BEGIN
                SELECT RAISE(ABORT, 'injected checkpoint failure'); END;",
            )
            .unwrap();
        }
        let revision = before.persistence_revision().unwrap().get();
        let transition = before.record_resource_sample(sample(0)).unwrap();
        let cloned_before_failure = transition.clone();
        let error = transition
            .persist(&mut store)
            .expect_err("no CommittedTransition on write/commit failure");
        assert!(matches!(error, ControllerStoreError::Sqlite(_)));
        assert!(matches!(
            store.load_attempt(id),
            Err(ControllerStoreError::Poisoned)
        ));
        raw.execute_batch("DROP TRIGGER fail_checkpoint").unwrap();
        assert!(matches!(
            cloned_before_failure.persist(&mut store),
            Err(ControllerStoreError::Poisoned)
        ));
        let (stored_revision, samples): (Vec<u8>, i64) = raw
            .query_row(
                "SELECT revision,(SELECT count(*) FROM resource_samples) FROM attempts",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(stored_revision, revision.to_be_bytes());
        assert_eq!(samples, 0);
        drop(store);
        let mut reopened = ControllerSqliteStore::open(&path).unwrap();
        let loaded = reopened.load_attempt(id).unwrap().unwrap();
        assert_eq!(loaded.phase(), Phase::ResourceSampling);
        let interrupted = recovery_checkpoint(loaded.prepare_recovery().unwrap())
            .persist(&mut reopened)
            .unwrap();
        assert!(interrupted.after_persist().is_empty());
        let committed = interrupted
            .into_parts()
            .0
            .recover()
            .unwrap()
            .persist(&mut reopened)
            .unwrap();
        assert_eq!(
            committed.after_persist(),
            &[OsEffect::SampleResources { sequence: 0 }]
        );
    }
}

#[test]
fn two_connections_reject_same_id_restart_and_stale_clone_without_losing_winner() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("controller.sqlite");
    let mut writer = ControllerSqliteStore::open(&path).unwrap();
    let mut other = ControllerSqliteStore::open(&path).unwrap();
    let id = AttemptId::new(12);
    let start = DeploymentAttempt::start(id, MonotonicTick::new(100), policy(3), identity(55, 31));
    let stale_start = start.clone();
    let first = start.persist(&mut writer).unwrap().into_parts().0;
    assert_eq!(first.persistence_revision().unwrap().get(), 1);
    assert!(matches!(
        stale_start.persist(&mut other),
        Err(ControllerStoreError::RevisionConflict)
    ));
    assert!(matches!(
        other.load_attempt(id),
        Err(ControllerStoreError::Poisoned)
    ));
    drop(other);
    let mut other = ControllerSqliteStore::open(&path).unwrap();
    let restart =
        DeploymentAttempt::start(id, MonotonicTick::new(999), policy(2), identity(99, 99));
    assert!(matches!(
        restart.persist(&mut other),
        Err(ControllerStoreError::RevisionConflict)
    ));
    drop(other);
    let mut other = ControllerSqliteStore::open(&path).unwrap();
    let stale_loaded = other.load_attempt(id).unwrap().unwrap();
    let stale = first.clone();
    let winner = first
        .record_resource_sample(sample(0))
        .unwrap()
        .persist(&mut writer)
        .unwrap()
        .into_parts()
        .0;
    assert_eq!(winner.persistence_revision().unwrap().get(), 2);
    assert!(matches!(
        stale
            .record_resource_sample(sample(0))
            .unwrap()
            .persist(&mut other),
        Err(ControllerStoreError::RevisionConflict)
    ));
    let winner = winner
        .record_resource_sample(sample(1))
        .unwrap()
        .persist(&mut writer)
        .unwrap()
        .into_parts()
        .0;
    assert_eq!(winner.persistence_revision().unwrap().get(), 3);
    drop(other);
    let mut other = ControllerSqliteStore::open(&path).unwrap();
    assert!(matches!(
        recovery_checkpoint(stale_loaded.prepare_recovery().unwrap()).persist(&mut other),
        Err(ControllerStoreError::RevisionConflict)
    ));
    let raw = Connection::open(&path).unwrap();
    let (revision, samples): (Vec<u8>, i64) = raw
        .query_row(
            "SELECT revision,(SELECT count(*) FROM resource_samples) FROM attempts",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(revision, 3_u64.to_be_bytes());
    assert_eq!(samples, 2);
}

#[test]
fn persisted_revision_cannot_recreate_a_missing_durable_row() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("controller.sqlite");
    let mut writer = ControllerSqliteStore::open(&path).unwrap();
    let mut other = ControllerSqliteStore::open(&path).unwrap();
    let id = AttemptId::new(20);
    let state = roundtrip(
        &mut writer,
        DeploymentAttempt::start(id, MonotonicTick::new(100), policy(2), identity(63, 39)),
    );
    Connection::open(&path)
        .unwrap()
        .execute_batch("PRAGMA foreign_keys=ON; DELETE FROM attempts")
        .unwrap();
    assert!(matches!(
        state.interrupt().unwrap().persist(&mut other),
        Err(ControllerStoreError::RevisionConflict)
    ));
    assert!(writer.load_attempt(id).unwrap().is_none());
}

#[test]
fn readback_revision_mismatch_rolls_back_and_poisons_without_effects() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("controller.sqlite");
    let mut store = ControllerSqliteStore::open(&path).unwrap();
    let id = AttemptId::new(21);
    let state = roundtrip(
        &mut store,
        DeploymentAttempt::start(id, MonotonicTick::new(100), policy(2), identity(64, 40)),
    );
    let raw = Connection::open(&path).unwrap();
    raw.execute_batch("CREATE TRIGGER corrupt_checkpoint AFTER INSERT ON resource_samples BEGIN UPDATE attempts SET revision=x'0000000000000001'; END").unwrap();
    assert!(matches!(
        state
            .record_resource_sample(sample(0))
            .unwrap()
            .persist(&mut store),
        Err(ControllerStoreError::Malformed(
            "checkpoint did not round-trip losslessly"
        ))
    ));
    assert!(matches!(
        store.load_attempt(id),
        Err(ControllerStoreError::Poisoned)
    ));
    assert_eq!(
        raw.query_row("SELECT count(*) FROM resource_samples", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        raw.query_row("SELECT hex(revision) FROM attempts", [], |row| row
            .get::<_, String>(0))
            .unwrap(),
        "0000000000000001"
    );
}

#[test]
fn two_connections_reject_stale_candidate_b_and_pre_migration_regression() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("controller.sqlite");
    let mut writer = ControllerSqliteStore::open(&path).unwrap();
    let mut other = ControllerSqliteStore::open(&path).unwrap();
    let id = AttemptId::new(13);
    let pre_migration = ready(&mut writer, id, identity(56, 32));
    let candidate_b = RetainedCandidateIdentity {
        source_commit: FullCommitSha::parse(&"12".repeat(20)).unwrap(),
        artifact_digest: ArtifactDigest::parse(&"34".repeat(32)).unwrap(),
    };
    let stale_candidate_b = pre_migration
        .clone()
        .record_migration_invoked(candidate_b)
        .unwrap();
    let committed_a = pre_migration
        .clone()
        .record_migration_invoked(candidate())
        .unwrap()
        .persist(&mut writer)
        .unwrap()
        .into_parts()
        .0;
    assert!(matches!(
        stale_candidate_b.persist(&mut other),
        Err(ControllerStoreError::RevisionConflict)
    ));
    drop(other);
    let mut other = ControllerSqliteStore::open(&path).unwrap();
    assert!(matches!(
        other.persist(&pre_migration),
        Err(ControllerStoreError::RevisionConflict)
    ));
    let raw = Connection::open(&path).unwrap();
    let (source, digest): (Vec<u8>, Vec<u8>) = raw
        .query_row(
            "SELECT source_commit,artifact_digest FROM retained_candidates",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(source, vec![0xef; 20]);
    assert_eq!(digest, vec![0xab; 32]);
    let recovered = committed_a.recover().unwrap().persist(&mut writer).unwrap();
    assert_eq!(
        recovered.after_persist(),
        &[OsEffect::ReverifyRetainedCandidate {
            candidate: candidate()
        }]
    );
}

#[test]
fn reopening_cleaning_or_recovering_restarts_observation_before_signal_capability() {
    for crash_in_recovery in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("controller.sqlite");
        let mut store = ControllerSqliteStore::open(&path).unwrap();
        let id = AttemptId::new(14);
        let root = identity(57, 33);
        let mut state = cleaning(&mut store, id, root);
        if crash_in_recovery {
            state = roundtrip(&mut store, state.interrupt().unwrap());
            state = roundtrip(&mut store, state.recover().unwrap());
        }
        state = roundtrip(
            &mut store,
            state
                .identity_observed(
                    root,
                    IdentityObservation::SameIdentity {
                        current_parent: None,
                    },
                )
                .unwrap(),
        );
        assert!(state.ownership_tree().unwrap().reconciliation_complete());
        drop(state);
        drop(store);
        let mut store = ControllerSqliteStore::open(&path).unwrap();
        let loaded = store.load_attempt(id).unwrap().unwrap();
        assert_eq!(
            loaded.phase(),
            if crash_in_recovery {
                Phase::Recovering
            } else {
                Phase::CleaningOwnedTree
            }
        );
        let interrupted = recovery_checkpoint(loaded.prepare_recovery().unwrap())
            .persist(&mut store)
            .unwrap();
        assert!(interrupted.after_persist().is_empty());
        let interrupted = interrupted.into_parts().0;
        assert_recovery_barrier(&mut store, interrupted, root);
    }
}

fn assert_recovery_barrier(
    store: &mut ControllerSqliteStore,
    interrupted: DeploymentAttempt,
    root: ProcessIdentity,
) {
    assert_eq!(interrupted.phase(), Phase::Interrupted);
    assert!(matches!(
        interrupted
            .clone()
            .request_owned_signals(Signal::Terminate, MonotonicTick::new(1)),
        Err(TransitionError::WrongPhase)
    ));
    let recovering = interrupted.recover().unwrap().persist(store).unwrap();
    assert_eq!(
        recovering.after_persist(),
        &[OsEffect::ObserveIdentity { identity: root }]
    );
    let state = recovering.into_parts().0;
    assert_eq!(state.phase(), Phase::Recovering);
    assert_eq!(
        state.ownership_tree().unwrap().identities()[&root],
        OwnedIdentityStatus::NeedsFreshObservation
    );
    assert!(matches!(
        state
            .clone()
            .request_owned_signals(Signal::Kill, MonotonicTick::new(1)),
        Err(TransitionError::WrongPhase)
    ));
    assert!(matches!(
        state
            .clone()
            .recovery_reconciliation_completed(MonotonicTick::new(1)),
        Err(TransitionError::IncompleteReconciliation)
    ));
    let state = roundtrip(
        store,
        state
            .identity_observed(
                root,
                IdentityObservation::SameIdentity {
                    current_parent: None,
                },
            )
            .unwrap(),
    );
    let resumed = state
        .recovery_reconciliation_completed(MonotonicTick::new(1))
        .unwrap()
        .persist(store)
        .unwrap();
    assert!(resumed.after_persist().is_empty());
    let state = resumed.into_parts().0;
    assert_eq!(state.phase(), Phase::CleaningOwnedTree);
    let signals = state
        .request_owned_signals(Signal::Terminate, MonotonicTick::new(1))
        .unwrap()
        .persist(store)
        .unwrap();
    assert_eq!(
        signals.after_persist(),
        &[OsEffect::SignalExactIdentity {
            identity: root,
            signal: Signal::Terminate
        }]
    );
}

#[test]
fn loaded_interrupted_and_migration_checkpoints_only_offer_gated_recovery() {
    let mut store = ControllerSqliteStore::in_memory().unwrap();
    let id = AttemptId::new(15);
    let root = identity(58, 34);
    let state = cleaning(&mut store, id, root);
    roundtrip(&mut store, state.interrupt().unwrap());
    let loaded = store.load_attempt(id).unwrap().unwrap();
    assert_eq!(loaded.phase(), Phase::Interrupted);
    let committed = recovery_checkpoint(loaded.prepare_recovery().unwrap())
        .persist(&mut store)
        .unwrap();
    assert_eq!(committed.checkpoint().phase(), Phase::Recovering);
    assert_eq!(
        committed.after_persist(),
        &[OsEffect::ObserveIdentity { identity: root }]
    );

    let id = AttemptId::new(16);
    ready(&mut store, id, identity(59, 35));
    let RecoveryDisposition::Stable(stable) = store
        .load_attempt(id)
        .unwrap()
        .unwrap()
        .prepare_recovery()
        .unwrap()
    else {
        panic!("pre-migration ready is metadata only after load");
    };
    assert_eq!(stable.phase(), Phase::ReadyForDisruption);
    assert_eq!(stable.attempt_id(), id);
    roundtrip(
        &mut store,
        stable.record_migration_invoked(candidate()).unwrap(),
    );
    for phase in [Phase::ReadyForDisruption, Phase::ForwardRecovery] {
        let loaded = store.load_attempt(id).unwrap().unwrap();
        assert_eq!(loaded.phase(), phase);
        let committed = recovery_checkpoint(loaded.prepare_recovery().unwrap())
            .persist(&mut store)
            .unwrap();
        assert_eq!(committed.checkpoint().phase(), Phase::ForwardRecovery);
        assert_eq!(
            committed.after_persist(),
            &[OsEffect::ReverifyRetainedCandidate {
                candidate: candidate()
            }]
        );
    }
}

#[test]
fn nonempty_sqlitex_database_is_detected_and_not_modified() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("unversioned.sqlite");
    let raw = Connection::open(&path).unwrap();
    raw.execute_batch("CREATE TABLE sqlitex_user_data (value TEXT NOT NULL); INSERT INTO sqlitex_user_data VALUES ('keep me')").unwrap();
    assert!(matches!(
        ControllerSqliteStore::open(&path),
        Err(ControllerStoreError::NonemptyUnversionedDatabase)
    ));
    assert_eq!(
        raw.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        raw.query_row("SELECT value FROM sqlitex_user_data", [], |row| row
            .get::<_, String>(0))
            .unwrap(),
        "keep me"
    );
    assert_eq!(
        raw.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE type='table'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
}

#[test]
fn revision_is_fixed_width_unsigned_and_exhaustion_is_fail_stop() {
    for revision in [
        i64::MAX.unsigned_abs(),
        i64::MAX.unsigned_abs() + 1,
        u64::MAX,
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("controller.sqlite");
        let mut store = ControllerSqliteStore::open(&path).unwrap();
        let id = AttemptId::new(17);
        roundtrip(
            &mut store,
            DeploymentAttempt::start(id, MonotonicTick::new(100), policy(2), identity(60, 36)),
        );
        let raw = Connection::open(&path).unwrap();
        raw.execute(
            "UPDATE attempts SET revision=?1",
            [revision.to_be_bytes().as_slice()],
        )
        .unwrap();
        let loaded = store.load_attempt(id).unwrap().unwrap();
        let transition = recovery_checkpoint(loaded.prepare_recovery().unwrap());
        let result = transition.persist(&mut store);
        if let Some(next) = revision.checked_add(1) {
            let committed = result.unwrap();
            assert_eq!(
                committed.checkpoint().persistence_revision().unwrap().get(),
                next
            );
            let stored: Vec<u8> = raw
                .query_row("SELECT revision FROM attempts", [], |row| row.get(0))
                .unwrap();
            assert_eq!(stored, next.to_be_bytes());
        } else {
            assert!(matches!(
                result,
                Err(ControllerStoreError::RevisionExhausted)
            ));
            assert!(matches!(
                store.load_attempt(id),
                Err(ControllerStoreError::Poisoned)
            ));
            assert_eq!(
                raw.query_row("SELECT hex(revision) FROM attempts", [], |row| row
                    .get::<_, String>(0))
                    .unwrap(),
                "FFFFFFFFFFFFFFFF"
            );
        }
    }
}

#[test]
fn schema_is_normalized_and_unknown_versions_and_unversioned_contents_are_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("controller.sqlite");
    let mut store = ControllerSqliteStore::open(&path).unwrap();
    admitted(&mut store, AttemptId::new(8), identity(50, 26));
    let raw = Connection::open(&path).unwrap();
    let tables: Vec<String> = raw
        .prepare("SELECT name FROM sqlite_schema WHERE type='table' ORDER BY name")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        tables,
        [
            "attempts",
            "discovery_epochs",
            "guardian_bindings",
            "guardian_sessions",
            "guardian_slots",
            "native_sampling_applications",
            "native_sampling_issues",
            "owned_identities",
            "ownership",
            "pending_child_scans",
            "process_identities",
            "resource_policies",
            "resource_samples",
            "retained_candidates",
            "sampling_obligations"
        ]
    );
    for table in &tables {
        if table != "attempts" {
            let count: i64 = raw
                .query_row(
                    "SELECT count(*) FROM pragma_foreign_key_list(?1)",
                    [table],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(count > 0, "{table} must have foreign keys");
        }
    }
    assert_eq!(
        raw.query_row("SELECT count(*) FROM resource_samples", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        raw.query_row("SELECT count(*) FROM owned_identities", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        raw.query_row("SELECT count(*) FROM pending_child_scans", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(raw.query_row("SELECT count(*) FROM pragma_table_info('resource_samples') WHERE name='ordinal' AND [notnull]=1", [], |row| row.get::<_, i64>(0)).unwrap(), 1);
    assert_eq!(
        raw.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        3
    );
    raw.pragma_update(None, "user_version", 4).unwrap();
    assert!(matches!(
        store.load_attempt(AttemptId::new(8)),
        Err(ControllerStoreError::UnsupportedSchema(4))
    ));
    drop(store);
    assert!(matches!(
        ControllerSqliteStore::open(&path),
        Err(ControllerStoreError::UnsupportedSchema(4))
    ));
    let other = directory.path().join("unversioned.sqlite");
    Connection::open(&other)
        .unwrap()
        .execute_batch("CREATE TABLE unrelated (id INTEGER)")
        .unwrap();
    assert!(matches!(
        ControllerSqliteStore::open(other),
        Err(ControllerStoreError::NonemptyUnversionedDatabase)
    ));
}

#[test]
fn incomplete_sampling_and_unknown_metadata_fail_closed() {
    for corruption in [
        "UPDATE attempts SET revision=zeroblob(8)",
        "UPDATE attempts SET revision=zeroblob(7)",
        "UPDATE attempts SET revision=zeroblob(9)",
        "UPDATE attempts SET interrupted_phase='cleaning'",
        "UPDATE attempts SET recovery_resume='cleaning'",
        "UPDATE attempts SET ownership_cleanup='not_started'",
        "UPDATE attempts SET block_reason='memory'",
        "UPDATE attempts SET verification='exact'",
        "DELETE FROM sampling_obligations",
        "DELETE FROM resource_policies",
        "DELETE FROM resource_samples",
        "UPDATE resource_samples SET ordinal=2",
        "UPDATE sampling_obligations SET pending_sequence=x'FFFFFFFFFFFFFFFF'",
        "UPDATE attempts SET phase='future_phase'",
        "UPDATE attempts SET compatibility='future_boundary'",
        "UPDATE resource_policies SET maximum_pressure='future_pressure'",
        "INSERT INTO process_identities VALUES (x'00000000000000000000000000000009',1,99,x'0000000000000001',0)",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("controller.sqlite");
        let mut store = ControllerSqliteStore::open(&path).unwrap();
        let id = AttemptId::new(9);
        let state = roundtrip(&mut store, DeploymentAttempt::start(id, MonotonicTick::new(100), policy(3), identity(51, 27)));
        roundtrip(&mut store, state.record_resource_sample(sample(0)).unwrap());
        let raw = Connection::open(&path).unwrap();
        raw.execute_batch("PRAGMA ignore_check_constraints=ON").unwrap();
        raw.execute_batch(corruption).unwrap();
        assert!(store.load_attempt(id).is_err(), "must reject {corruption}");
    }
}

#[test]
fn optional_rows_must_match_the_phase_even_with_valid_foreign_keys() {
    for (phase, corruption) in [
        (Phase::ResourceSampling, "INSERT INTO ownership SELECT attempt_id,x'0000000000000000' FROM attempts; INSERT INTO owned_identities SELECT attempt_id,0,'fresh',NULL,NULL FROM attempts"),
        (Phase::ResourceSampling, "INSERT INTO retained_candidates SELECT attempt_id,zeroblob(20),zeroblob(32) FROM attempts"),
        (Phase::ReadyForDisruption, "INSERT INTO sampling_obligations SELECT attempt_id,0,zeroblob(8) FROM attempts"),
        (Phase::ReadyForDisruption, "UPDATE attempts SET recovery_resume='cleaning'"),
        (Phase::ReadyForDisruption, "UPDATE attempts SET verification='exact'"),
        (Phase::ForwardRecovery, "INSERT INTO resource_policies SELECT attempt_id,x'0000000000000002',zeroblob(8),zeroblob(8),0,'nominal' FROM attempts"),
        (Phase::ForwardRecovery, "INSERT INTO resource_samples SELECT attempt_id,0,zeroblob(8),zeroblob(8),zeroblob(8),0,'nominal' FROM attempts"),
        (Phase::ForwardRecovery, "INSERT INTO process_identities SELECT attempt_id,0,42,zeroblob(8),0 FROM attempts"),
        (Phase::ForwardRecovery, "UPDATE attempts SET compatibility='pre_migration'; DELETE FROM retained_candidates"),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("controller.sqlite");
        let mut store = ControllerSqliteStore::open(&path).unwrap();
        let id = AttemptId::new(18);
        let root = identity(61, 37);
        match phase {
            Phase::ResourceSampling => {
                roundtrip(&mut store, DeploymentAttempt::start(id, MonotonicTick::new(100), policy(2), root));
            }
            Phase::ReadyForDisruption => { ready(&mut store, id, root); }
            Phase::ForwardRecovery => {
                let state = ready(&mut store, id, root);
                let state = roundtrip(&mut store, state.record_migration_invoked(candidate()).unwrap());
                roundtrip(&mut store, state.recover().unwrap());
            }
            Phase::OwnershipDiscovery
            | Phase::AwaitingBirthEvidence
            | Phase::CleaningOwnedTree
            | Phase::VerifyingCleanup
            | Phase::ResourceBlocked
            | Phase::Interrupted
            | Phase::Recovering
            | Phase::AdministrativeCloseoutRequired
            | Phase::AdministrativelyClosed => unreachable!("fixture phases are explicit"),
        }
        let raw = Connection::open(&path).unwrap();
        raw.execute_batch("PRAGMA foreign_keys=ON; PRAGMA ignore_check_constraints=ON").unwrap();
        raw.execute_batch(corruption).unwrap();
        assert!(raw.prepare("PRAGMA foreign_key_check").unwrap().query([]).unwrap().next().unwrap().is_none());
        assert!(store.load_attempt(id).is_err(), "{phase:?} must reject {corruption}");
    }
}

#[test]
fn revision_schema_rejects_zero_wrong_width_and_non_blob_encodings() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("controller.sqlite");
    let mut store = ControllerSqliteStore::open(&path).unwrap();
    roundtrip(
        &mut store,
        DeploymentAttempt::start(
            AttemptId::new(19),
            MonotonicTick::new(100),
            policy(2),
            identity(62, 38),
        ),
    );
    let raw = Connection::open(&path).unwrap();
    for invalid in [
        "zeroblob(8)",
        "zeroblob(7)",
        "zeroblob(9)",
        "1",
        "'00000001'",
        "NULL",
    ] {
        assert!(
            raw.execute_batch(&format!("UPDATE attempts SET revision={invalid}"))
                .is_err(),
            "must reject {invalid}"
        );
    }
    assert_eq!(
        raw.query_row("SELECT hex(revision) FROM attempts", [], |row| row
            .get::<_, String>(0))
            .unwrap(),
        "0000000000000001"
    );
}

#[test]
fn malformed_ownership_evidence_and_missing_retained_candidate_fail_closed() {
    for corruption in [
        "DELETE FROM resource_samples WHERE ordinal=1",
        "UPDATE owned_identities SET status='future_status'",
        "UPDATE ownership SET next_discovery=x'0000000000000000'",
        "DELETE FROM ownership",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("controller.sqlite");
        let mut store = ControllerSqliteStore::open(&path).unwrap();
        let id = AttemptId::new(10);
        let root = identity(52, 28);
        let state = admitted(&mut store, id, root);
        let (transition, _) = state
            .child_discovered(root, identity(53, 29).pid())
            .unwrap();
        roundtrip(&mut store, transition);
        let raw = Connection::open(&path).unwrap();
        raw.execute_batch("PRAGMA ignore_check_constraints=ON")
            .unwrap();
        raw.execute_batch(corruption).unwrap();
        assert!(store.load_attempt(id).is_err(), "must reject {corruption}");
        assert!(matches!(
            store.load_attempt(id),
            Err(ControllerStoreError::Poisoned)
        ));
        assert!(matches!(
            DeploymentAttempt::start(
                AttemptId::new(10_000),
                MonotonicTick::new(100),
                policy(1),
                identity(10_000, 10_000),
            )
            .persist(&mut store),
            Err(ControllerStoreError::Poisoned)
        ));
    }
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("controller.sqlite");
    let mut store = ControllerSqliteStore::open(&path).unwrap();
    let id = AttemptId::new(11);
    let state = ready(&mut store, id, identity(54, 30));
    roundtrip(
        &mut store,
        state.record_migration_invoked(candidate()).unwrap(),
    );
    Connection::open(&path)
        .unwrap()
        .execute_batch("DELETE FROM retained_candidates")
        .unwrap();
    assert!(
        store.load_attempt(id).is_err(),
        "candidate loss must never restore pre-migration authority"
    );
    assert!(matches!(
        store.load_attempt(id),
        Err(ControllerStoreError::Poisoned)
    ));
    assert!(matches!(
        DeploymentAttempt::start(
            AttemptId::new(11_000),
            MonotonicTick::new(100),
            policy(1),
            identity(11_000, 11_000),
        )
        .persist(&mut store),
        Err(ControllerStoreError::Poisoned)
    ));
}
