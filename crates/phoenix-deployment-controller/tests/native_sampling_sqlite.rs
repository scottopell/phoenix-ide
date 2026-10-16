//! Native sample rows are evidence, not a dispatch or ACK reconstruction route.
use phoenix_deployment_controller::{
    AttemptId, BirthToken, ControllerSqliteStore, ControllerStoreError, CpuContentionBasisPoints,
    DeploymentAttempt, GuardianLifetimeNonce, MemoryPressure, MonotonicTick,
    NativeGuardianIdentity, NativeSamplingStatus, PersistenceRevision, PolicyLoop, ProcessId,
    ProcessIdentity, ResourcePolicy,
};
use rusqlite::Connection;
use std::num::{NonZeroU32, NonZeroUsize};

fn identity(pid: u32) -> ProcessIdentity {
    ProcessIdentity::new(
        ProcessId::new(NonZeroU32::new(pid).unwrap()),
        BirthToken::new(12, 3).unwrap(),
    )
}

fn setup(path: &std::path::Path) -> (ControllerSqliteStore, PolicyLoop) {
    let mut store = ControllerSqliteStore::open(path).unwrap();
    let policy = ResourcePolicy::new(
        NonZeroUsize::new(2).unwrap(),
        0,
        0,
        CpuContentionBasisPoints::new(100).unwrap(),
        MemoryPressure::Critical,
    );
    let committed = DeploymentAttempt::start(
        AttemptId::new(9),
        MonotonicTick::new(100),
        policy,
        identity(42),
    )
    .persist(&mut store)
    .unwrap();
    store
        .commit_initial_binding(
            AttemptId::new(9),
            MonotonicTick::new(100),
            PersistenceRevision::first(),
            NativeGuardianIdentity {
                nonce: GuardianLifetimeNonce::new([5; 16]).unwrap(),
                process: identity(100),
            },
            &[],
        )
        .unwrap();
    (store, PolicyLoop::from_committed(committed))
}

#[test]
fn schema_v3_is_empty_and_v2_is_rejected_without_mutating_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fresh.sqlite");
    let store = ControllerSqliteStore::open(&path).unwrap();
    drop(store);
    let raw = Connection::open(&path).unwrap();
    assert_eq!(
        raw.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        3
    );
    for table in ["native_sampling_issues", "native_sampling_applications"] {
        assert_eq!(
            raw.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
    let old = dir.path().join("old.sqlite");
    let raw = Connection::open(&old).unwrap();
    raw.execute_batch("CREATE TABLE marker (sentinel TEXT); INSERT INTO marker VALUES ('untouched'); PRAGMA user_version=2;").unwrap();
    drop(raw);
    assert!(matches!(
        ControllerSqliteStore::open(&old),
        Err(ControllerStoreError::UnsupportedSchema(2))
    ));
    let raw = Connection::open(&old).unwrap();
    assert_eq!(
        raw.query_row("SELECT sentinel FROM marker", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "untouched"
    );
    assert_eq!(
        raw.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
}

#[test]
fn duplicate_issue_and_stale_binding_poison_without_extra_rows() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("duplicate.sqlite");
    let (mut store, mut loop_) = setup(&path);
    let binding = store
        .load_native_binding(AttemptId::new(9))
        .unwrap()
        .unwrap();
    let ticket = loop_.issue_next().unwrap();
    let issue = store
        .commit_native_sampling_issue(&ticket, &binding)
        .unwrap();
    assert_eq!(issue.guardian_identity(), binding.identity());
    assert!(store
        .commit_native_sampling_issue(&ticket, &binding)
        .is_err());
    assert!(matches!(
        store.load_native_sampling_status(issue.key()),
        Err(ControllerStoreError::Poisoned)
    ));
    drop(store);
    let mut store = ControllerSqliteStore::open_existing(&path).unwrap();
    assert_eq!(
        store.load_native_sampling_status(issue.key()).unwrap(),
        Some(NativeSamplingStatus::IssuedPendingApplication { issue })
    );
    store.commit_detached_binding(&binding, &[]).unwrap();
    assert!(matches!(
        store.commit_native_sampling_issue(&ticket, &binding),
        Err(ControllerStoreError::RevisionConflict)
    ));
}

#[test]
fn stale_dispatch_revision_and_detached_session_cannot_issue() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("stale.sqlite");
    let (mut store, mut loop_) = setup(&path);
    let binding = store
        .load_native_binding(AttemptId::new(9))
        .unwrap()
        .unwrap();
    let ticket = loop_.issue_next().unwrap();
    loop_.interrupt(&mut store).unwrap();
    assert!(matches!(
        store.commit_native_sampling_issue(&ticket, &binding),
        Err(ControllerStoreError::RevisionConflict)
    ));
    assert!(matches!(
        store.load_attempt(AttemptId::new(9)),
        Err(ControllerStoreError::Poisoned)
    ));
}

#[test]
fn sqlite_enforces_native_issue_identity_and_total_outcome_shape() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shape.sqlite");
    let (mut store, mut loop_) = setup(&path);
    let binding = store
        .load_native_binding(AttemptId::new(9))
        .unwrap()
        .unwrap();
    let ticket = loop_.issue_next().unwrap();
    let issue = store
        .commit_native_sampling_issue(&ticket, &binding)
        .unwrap();
    let raw = Connection::open(&path).unwrap();
    raw.pragma_update(None, "foreign_keys", true).unwrap();
    let id = 9_u128.to_be_bytes();
    let revision = 1_u64.to_be_bytes();
    for (ordinal, generation, sequence) in [(1, 1_u64, 0_u64), (0, 2, 1), (0, 1, 0)] {
        assert!(raw
            .execute(
                "INSERT INTO native_sampling_issues VALUES (?1,?2,?3,?4,?5)",
                rusqlite::params![
                    id.as_slice(),
                    revision.as_slice(),
                    ordinal,
                    generation.to_be_bytes().as_slice(),
                    sequence.to_be_bytes().as_slice()
                ],
            )
            .is_err());
    }
    assert!(raw.execute(
        "INSERT INTO native_sampling_applications (attempt_id,dispatch_revision,ordinal,applying_revision,outcome,available_memory) VALUES (?1,?2,0,?3,'sample',?4)",
        rusqlite::params![id.as_slice(), revision.as_slice(), 2_u64.to_be_bytes().as_slice(),
            10_u64.to_be_bytes().as_slice()],
    ).is_err());
    assert!(raw.execute(
        "INSERT INTO native_sampling_applications (attempt_id,dispatch_revision,ordinal,applying_revision,outcome,cpu) VALUES (?1,?2,0,?3,'unsupported',1)",
        rusqlite::params![id.as_slice(), revision.as_slice(), 2_u64.to_be_bytes().as_slice()],
    ).is_err());
    assert_eq!(
        store.load_native_sampling_status(issue.key()).unwrap(),
        Some(NativeSamplingStatus::IssuedPendingApplication { issue })
    );
}
