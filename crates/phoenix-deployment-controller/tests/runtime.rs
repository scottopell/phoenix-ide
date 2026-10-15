//! Isolated runtime/CLI regression fixtures: never invoke a native OS adapter.
use std::{
    num::{NonZeroU32, NonZeroUsize},
    path::Path,
    process::{Command, Output},
};

use phoenix_deployment_controller::{
    ArtifactDigest, AttemptId, AttemptStore, BirthToken, CandidateDiscovery, CandidateVerification,
    ChildIdentityObservation, ControllerOs, ControllerSqliteStore, ControllerStoreError,
    CpuContentionBasisPoints, DeploymentAttempt, EffectCompletion, EffectExecutor, FullCommitSha,
    IdentityObservation, MemoryPressure, MonotonicTick, ObservationFailure, OsEffect, OsQueryError,
    PersistenceRevision, Phase, ProcessId, ProcessIdentity, ResourceCapacity, ResourceContention,
    ResourceObservationFailure, ResourcePolicy, ResourceSample, RetainedCandidateIdentity, Signal,
    SignalOutcome, Transition,
};
use serde_json::{json, Value};

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

fn start(id: u128) -> Transition {
    DeploymentAttempt::start(
        AttemptId::new(id),
        MonotonicTick::new(100),
        policy(1),
        identity(42, 10),
    )
}

fn commit(store: &mut ControllerSqliteStore, transition: Transition) -> DeploymentAttempt {
    transition.persist(store).unwrap().into_parts().0
}

fn admitted(store: &mut ControllerSqliteStore, id: u128) -> DeploymentAttempt {
    let state = commit(store, start(id));
    commit(store, state.record_resource_sample(sample(0)).unwrap())
}

fn cleaning(store: &mut ControllerSqliteStore, id: u128) -> DeploymentAttempt {
    let state = admitted(store, id);
    let state = commit(store, state.child_scan_completed(identity(42, 10)).unwrap());
    commit(store, state.ownership_scan_completed().unwrap())
}

fn ready(store: &mut ControllerSqliteStore, id: u128) -> DeploymentAttempt {
    let state = cleaning(store, id);
    let state = commit(store, state.cleanup_signals_settled().unwrap());
    let state = commit(
        store,
        state
            .identity_observed(identity(42, 10), IdentityObservation::ExactIdentityAbsent)
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

struct RecordingOs {
    calls: Vec<OsEffect>,
    resources: Result<ResourceSample, ResourceObservationFailure>,
    children: Result<Vec<ProcessId>, OsQueryError>,
    child: ChildIdentityObservation,
    observation: IdentityObservation,
    signal: SignalOutcome,
    verification: CandidateVerification,
}

impl ControllerOs for RecordingOs {
    fn sample_resources(
        &mut self,
        sequence: u64,
    ) -> Result<ResourceSample, ResourceObservationFailure> {
        self.calls.push(OsEffect::SampleResources { sequence });
        self.resources
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
        self.child
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
        self.verification
    }
}

fn recording_os() -> RecordingOs {
    RecordingOs {
        calls: Vec::new(),
        resources: Ok(sample(0)),
        children: Ok(vec![identity(43, 11).pid()]),
        child: ChildIdentityObservation::Unresolved(ObservationFailure::NoSuchProcess),
        observation: IdentityObservation::ExactIdentityAbsent,
        signal: SignalOutcome::Delivered,
        verification: CandidateVerification::Exact,
    }
}

fn execute_one(
    store: &mut ControllerSqliteStore,
    transition: Transition,
    os: &mut RecordingOs,
    expected: EffectCompletion,
) -> DeploymentAttempt {
    let committed = transition.persist(store).unwrap();
    let revision = committed.checkpoint().persistence_revision().unwrap();
    let effect = expected.effect();
    assert_eq!(committed.after_persist(), std::slice::from_ref(&effect));
    let (checkpoint, mut batch) = committed.into_dispatch();
    assert_eq!(batch.len(), 1);
    let ticket = batch.next().unwrap();
    assert!(batch.next().is_none());
    assert_eq!(ticket.effect(), &effect);
    let key = ticket.key();
    assert_eq!(key.attempt_id(), checkpoint.attempt_id());
    assert_eq!(key.revision(), revision);
    assert_eq!(key.ordinal(), 0);
    let before = os.calls.len();
    let executed = EffectExecutor::new(os).execute(ticket);
    assert_eq!(&os.calls[before..], &[effect]);
    assert_eq!(executed.key(), key);
    assert_eq!(executed.completion().effect(), expected.effect());
    assert_eq!(executed.into_parts(), (key, expected));
    checkpoint
}

fn configure_outcomes(os: &mut RecordingOs, index: usize) {
    os.resources = match index {
        0 => Ok(sample(0)),
        1 => Err(ResourceObservationFailure::TemporarilyUnavailable),
        2 => Err(ResourceObservationFailure::Unsupported),
        _ => Err(ResourceObservationFailure::Malformed),
    };
    os.children = match index {
        0 => Ok(vec![identity(43, 11).pid()]),
        1 => Err(OsQueryError::PermissionDenied),
        2 => Err(OsQueryError::TemporarilyUnavailable),
        _ => Err(OsQueryError::Malformed),
    };
    let failures = [
        ObservationFailure::NoSuchProcess,
        ObservationFailure::PermissionDenied,
        ObservationFailure::TemporarilyUnavailable,
        ObservationFailure::Malformed,
    ];
    os.child = ChildIdentityObservation::Unresolved(failures[index % 4]);
    os.observation = if index == 0 {
        IdentityObservation::ExactIdentityAbsent
    } else {
        IdentityObservation::Unresolved(failures[index % 4])
    };
    os.signal = [
        SignalOutcome::Delivered,
        SignalOutcome::ExactIdentityAlreadyAbsent,
        SignalOutcome::PidReused,
        SignalOutcome::PermissionDenied,
        SignalOutcome::TemporarilyUnavailable,
    ][index];
    os.verification = [
        CandidateVerification::Exact,
        CandidateVerification::Missing,
        CandidateVerification::Mismatch,
        CandidateVerification::Unavailable,
        CandidateVerification::Exact,
    ][index];
}

fn assert_six_effect_dispatches(index: usize) {
    let mut store = ControllerSqliteStore::in_memory().unwrap();
    let mut os = recording_os();
    configure_outcomes(&mut os, index);
    let resources = os.resources;
    let state = execute_one(
        &mut store,
        start(1),
        &mut os,
        EffectCompletion::SampleResources {
            sequence: 0,
            result: resources,
        },
    );
    let children = os.children.clone();
    let state = execute_one(
        &mut store,
        state.record_resource_sample(sample(0)).unwrap(),
        &mut os,
        EffectCompletion::ListChildren {
            parent: identity(42, 10),
            result: children,
        },
    );
    let (transition, discovery) = state
        .child_discovered(identity(42, 10), identity(43, 11).pid())
        .unwrap();
    let child = os.child;
    execute_one(
        &mut store,
        transition,
        &mut os,
        EffectCompletion::AcquireChildIdentity {
            discovery,
            observation: child,
        },
    );

    let state = admitted(&mut store, 2);
    let state = commit(
        &mut store,
        state.child_scan_completed(identity(42, 10)).unwrap(),
    );
    let observation = os.observation;
    let state = execute_one(
        &mut store,
        state.ownership_scan_completed().unwrap(),
        &mut os,
        EffectCompletion::ObserveIdentity {
            identity: identity(42, 10),
            observation,
        },
    );
    let state = commit(
        &mut store,
        state
            .identity_observed(
                identity(42, 10),
                IdentityObservation::SameIdentity {
                    current_parent: None,
                },
            )
            .unwrap(),
    );
    let signal = if index.is_multiple_of(2) {
        Signal::Terminate
    } else {
        Signal::Kill
    };
    let outcome = os.signal;
    execute_one(
        &mut store,
        state
            .request_owned_signals(signal, MonotonicTick::new(99))
            .unwrap(),
        &mut os,
        EffectCompletion::SignalExactIdentity {
            identity: identity(42, 10),
            signal,
            outcome,
        },
    );

    let state = ready(&mut store, 3);
    let state = commit(
        &mut store,
        state.record_migration_invoked(candidate()).unwrap(),
    );
    let verification = os.verification;
    let state = execute_one(
        &mut store,
        state.recover().unwrap(),
        &mut os,
        EffectCompletion::ReverifyRetainedCandidate {
            candidate: candidate(),
            verification,
        },
    );
    assert_eq!(state.phase(), Phase::ForwardRecovery);
    assert_eq!(os.calls.len(), 6); // Exact verification did not activate anything.
}

#[test]
fn six_effects_are_correlated_one_call_each_and_keep_typed_outcomes() {
    // Each pass covers different native outcomes without applying them as policy.
    for index in 0..5 {
        assert_six_effect_dispatches(index);
    }
}

#[test]
fn committed_revision_and_sibling_ordinals_survive_partial_issuance() {
    let mut store = ControllerSqliteStore::in_memory().unwrap();
    let os = recording_os();
    let state = admitted(&mut store, 9);
    let (transition, _) = state
        .child_discovered(identity(42, 10), identity(43, 11).pid())
        .unwrap();
    let state = commit(&mut store, transition);
    let state = commit(&mut store, state.interrupt().unwrap());
    // Recovery emits identity observation, pending enumeration, and a birth probe.
    let committed = state.recover().unwrap().persist(&mut store).unwrap();
    let expected = committed.after_persist().to_vec();
    let revision = committed.checkpoint().persistence_revision().unwrap();
    assert!(revision.get() > 1);
    let (_, mut batch) = committed.into_dispatch();
    assert_eq!(batch.len(), expected.len());
    let first = batch.next().unwrap();
    assert_eq!(first.key().ordinal(), 0);
    assert_eq!(first.key().revision(), revision);
    assert_eq!(first.key().attempt_id(), AttemptId::new(9));
    assert_eq!(first.effect(), &expected[0]);
    for (ordinal, ticket) in batch.enumerate() {
        assert_eq!(ticket.key().attempt_id(), AttemptId::new(9));
        assert_eq!(ticket.key().revision(), revision);
        assert_eq!(ticket.key().ordinal(), ordinal + 1);
        assert_eq!(ticket.effect(), &expected[ordinal + 1]);
    }
    assert!(expected.len() >= 3);
    assert!(os.calls.is_empty());
    let (_, batch) = start(10).persist(&mut store).unwrap().into_dispatch();
    drop(batch);
    assert!(os.calls.is_empty());
}

#[test]
fn persistence_failure_never_exposes_dispatch_authority() {
    struct Reject;
    impl AttemptStore for Reject {
        type Error = &'static str;
        fn persist(&mut self, _: &DeploymentAttempt) -> Result<PersistenceRevision, Self::Error> {
            Err("rejected")
        }
    }
    let os = recording_os();
    let pending = start(1);
    assert!(os.calls.is_empty());
    assert_eq!(pending.persist(&mut Reject).unwrap_err(), "rejected");
    assert!(os.calls.is_empty());
}

fn cli(command: &str, store: &Path, attempt: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_phoenix-deployment-controller"))
        .args([command, "--store"])
        .arg(store)
        .args(["--attempt", attempt])
        .output()
        .unwrap()
}

fn summary(output: &Output) -> Value {
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());
    let payload = output
        .stdout
        .strip_suffix(b"\n")
        .expect("summary ends with one newline");
    assert!(!payload.contains(&b'\n'));
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value.as_object().unwrap().len(), 4); // No checkpoint, effects, or native metadata.
    value
}

fn failure(output: &Output, code: i32, category: &str) {
    assert_eq!(output.status.code(), Some(code));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr.clone()).unwrap(),
        format!("{category}\n")
    );
}

#[test]
fn missing_stores_are_never_created_and_errors_do_not_echo_inputs() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("secret-store-name.sqlite");
    assert!(ControllerSqliteStore::open_existing(&path).is_err());
    for command in ["status", "closeout"] {
        failure(&cli(command, &path, "1"), 3, "store_error");
        assert!(!path.exists());
    }
    for malformed in [
        "",
        "-1",
        "+1",
        "0x10",
        "abc-secret",
        "340282366920938463463374607431768211456",
    ] {
        failure(&cli("status", &path, malformed), 2, "usage_error");
        assert!(!path.exists());
    }
    for unsupported in ["run", "reconcile", "deploy"] {
        failure(&cli(unsupported, &path, "1"), 2, "usage_error");
        assert!(!path.exists());
    }
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[test]
fn open_existing_never_initializes_an_empty_database() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("unversioned.sqlite");
    std::fs::write(&path, []).unwrap();
    assert!(matches!(
        ControllerSqliteStore::open_existing(&path),
        Err(ControllerStoreError::UnsupportedSchema(0))
    ));
    assert!(std::fs::read(&path).unwrap().is_empty());
}

#[test]
fn cli_status_is_read_only_and_closeout_is_phase_fenced() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("controller.sqlite");
    let mut store = ControllerSqliteStore::open(&path).unwrap();
    let state = commit(&mut store, start(u128::MAX));
    let state = commit(&mut store, state.record_resource_sample(sample(0)).unwrap());
    let id = u128::MAX.to_string();
    let expected = json!({
        "attempt_id": id, "persistence_revision": 2,
        "phase": "ownership_discovery", "cleanup_outcome": "not_started"
    });
    let before = std::fs::read(&path).unwrap();
    assert_eq!(summary(&cli("status", &path, &id)), expected);
    failure(&cli("closeout", &path, &id), 5, "wrong_phase");
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(
        store
            .load_attempt(AttemptId::new(u128::MAX))
            .unwrap()
            .unwrap()
            .persistence_revision()
            .get(),
        2
    );
    failure(&cli("status", &path, "42"), 4, "attempt_not_found");

    let state = commit(
        &mut store,
        state.deadline_reached(MonotonicTick::new(100)).unwrap(),
    );
    assert_eq!(
        summary(&cli("status", &path, &id))["phase"],
        "administrative_closeout_required"
    );
    let revision = state.persistence_revision().unwrap().get();
    let closed = summary(&cli("closeout", &path, &id));
    assert_eq!(
        closed,
        json!({
            "attempt_id": id, "persistence_revision": revision + 1,
            "phase": "administratively_closed", "cleanup_outcome": "administrative_risk_accepted"
        })
    );
    assert_eq!(summary(&cli("status", &path, &id)), closed);
    let before = std::fs::read(&path).unwrap();
    failure(&cli("closeout", &path, &id), 5, "wrong_phase");
    assert_eq!(std::fs::read(&path).unwrap(), before);
    // A pre-closeout live checkpoint cannot overwrite CLI closeout (CAS preserved).
    assert!(matches!(
        state
            .accept_administrative_risk(
                phoenix_deployment_controller::AdministrativeRiskAcceptance::for_attempt(
                    AttemptId::new(u128::MAX)
                )
            )
            .unwrap()
            .persist(&mut store),
        Err(ControllerStoreError::RevisionConflict)
    ));
    assert!(matches!(
        store.load_attempt(AttemptId::new(u128::MAX)),
        Err(ControllerStoreError::Poisoned)
    ));
}

#[test]
fn cli_summaries_cover_sampling_ready_and_forward_recovery_without_running_them() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("controller.sqlite");
    let mut store = ControllerSqliteStore::open(&path).unwrap();
    commit(&mut store, start(1));
    let state = ready(&mut store, 2);
    let forward = ready(&mut store, 3);
    let forward = commit(
        &mut store,
        forward.record_migration_invoked(candidate()).unwrap(),
    );
    commit(&mut store, forward.recover().unwrap());
    assert_eq!(state.phase(), Phase::ReadyForDisruption);
    let interrupted = commit(&mut store, start(4));
    commit(&mut store, interrupted.interrupt().unwrap());
    let recovering = admitted(&mut store, 5);
    let recovering = commit(&mut store, recovering.interrupt().unwrap());
    commit(&mut store, recovering.recover().unwrap());
    cleaning(&mut store, 6);
    let verifying = cleaning(&mut store, 7);
    commit(&mut store, verifying.cleanup_signals_settled().unwrap());
    let awaiting = admitted(&mut store, 8);
    let (transition, discovery) = awaiting
        .child_discovered(identity(42, 10), identity(43, 11).pid())
        .unwrap();
    let awaiting = commit(&mut store, transition);
    commit(
        &mut store,
        awaiting
            .child_observed(
                discovery,
                ChildIdentityObservation::Unresolved(ObservationFailure::NoSuchProcess),
            )
            .unwrap(),
    );
    let blocked = commit(&mut store, start(9));
    let mut insufficient = sample(0);
    insufficient.capacity.available_memory_bytes = 0;
    commit(
        &mut store,
        blocked.record_resource_sample(insufficient).unwrap(),
    );
    for (id, phase, cleanup) in [
        ("1", "resource_sampling", "not_started"),
        ("2", "ready_for_disruption", "verified_clean"),
        ("3", "forward_recovery", "unresolved_risk"),
        ("4", "interrupted", "not_started"),
        ("5", "recovering", "not_started"),
        ("6", "cleaning_owned_tree", "in_progress"),
        ("7", "verifying_cleanup", "in_progress"),
        ("8", "awaiting_birth_evidence", "not_started"),
        ("9", "resource_blocked", "not_started"),
    ] {
        let before = std::fs::read(&path).unwrap();
        let value = summary(&cli("status", &path, id));
        assert_eq!(value["phase"], phase);
        assert_eq!(value["cleanup_outcome"], cleanup);
        failure(&cli("closeout", &path, id), 5, "wrong_phase");
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
}

#[test]
fn malformed_load_is_category_only_and_poisons_existing_store_handle() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("controller.sqlite");
    let mut store = ControllerSqliteStore::open(&path).unwrap();
    admitted(&mut store, 1);
    drop(store);
    let raw = rusqlite::Connection::open(&path).unwrap();
    raw.execute("DELETE FROM resource_samples", []).unwrap();
    drop(raw);
    let mut store = ControllerSqliteStore::open_existing(&path).unwrap();
    let before = std::fs::read(&path).unwrap();
    failure(&cli("status", &path, "1"), 3, "store_error");
    failure(&cli("closeout", &path, "1"), 3, "store_error");
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(matches!(
        store.load_attempt(AttemptId::new(1)),
        Err(ControllerStoreError::Malformed(_))
    ));
    assert!(matches!(
        store.load_attempt(AttemptId::new(1)),
        Err(ControllerStoreError::Poisoned)
    ));
}

#[test]
fn cli_rejects_extra_or_duplicate_arguments_before_store_access() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("absent.sqlite");
    for suffix in [
        vec!["--unknown"],
        vec!["--attempt", "2"],
        vec!["--store", "other.sqlite"],
        vec!["trailing"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_phoenix-deployment-controller"))
            .args(["closeout", "--store"])
            .arg(&path)
            .args(["--attempt", "1"])
            .args(suffix)
            .output()
            .unwrap();
        failure(&output, 2, "usage_error");
    }
    let mut store = ControllerSqliteStore::open(&path).unwrap();
    commit(&mut store, start(1));
    let output = Command::new(env!("CARGO_BIN_EXE_phoenix-deployment-controller"))
        .args(["status", "--attempt", "0001", "--store"])
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(summary(&output)["attempt_id"], "1");
}
