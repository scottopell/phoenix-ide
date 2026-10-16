//! Disposable database binding evidence, never a guardian process or executable permit.
use phoenix_deployment_controller::{
    AttemptId, BirthToken, ControllerSqliteStore, ControllerStoreError, CpuContentionBasisPoints,
    DeploymentAttempt, GuardianLifetimeNonce, MemoryPressure, MonotonicTick,
    NativeGuardianIdentity, NativeGuardianSlot, NativeSessionState, NativeSlotLiveness,
    PersistenceRevision, ProcessId, ProcessIdentity, ResourcePolicy,
};
use rusqlite::{params, Connection};
use std::num::{NonZeroU32, NonZeroUsize};

fn process(pid: u32) -> ProcessIdentity {
    ProcessIdentity::new(
        ProcessId::new(NonZeroU32::new(pid).unwrap()),
        BirthToken::new(12, 34).unwrap(),
    )
}
fn start(
    store: &mut ControllerSqliteStore,
    id: AttemptId,
) -> phoenix_deployment_controller::DeploymentAttempt {
    let policy = ResourcePolicy::new(
        NonZeroUsize::new(2).unwrap(),
        0,
        0,
        CpuContentionBasisPoints::new(0).unwrap(),
        MemoryPressure::Critical,
    );
    DeploymentAttempt::start(id, MonotonicTick::new(100), policy, process(50))
        .persist(store)
        .unwrap()
        .into_parts()
        .0
}
fn guardian() -> NativeGuardianIdentity {
    NativeGuardianIdentity {
        nonce: GuardianLifetimeNonce::new([1; 16]).unwrap(),
        process: process(100),
    }
}
fn slots() -> Vec<NativeGuardianSlot> {
    [process(101), process(102)]
        .into_iter()
        .map(|process| NativeGuardianSlot {
            process,
            liveness: NativeSlotLiveness::Running,
        })
        .collect()
}

#[test]
fn binding_survives_policy_checkpoint_advance_and_reopen_in_original_order() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("binding.sqlite");
    let id = AttemptId::new(44);
    let mut store = ControllerSqliteStore::open(&path).unwrap();
    let state = start(&mut store, id);
    let binding = store
        .commit_initial_binding(
            id,
            state.deadline(),
            PersistenceRevision::first(),
            guardian(),
            &slots(),
        )
        .unwrap();
    let state = state
        .interrupt()
        .unwrap()
        .persist(&mut store)
        .unwrap()
        .into_parts()
        .0;
    assert_eq!(state.deadline(), binding.deadline());
    assert_eq!(
        store.load_native_binding(id).unwrap(),
        Some(binding.clone())
    );
    let detached = store
        .commit_detached_binding(&binding, binding.slots())
        .unwrap();
    assert_eq!(
        detached.current_session().state,
        NativeSessionState::Detached
    );
    let fresh = store
        .commit_fresh_interruption_binding(&detached, state.persistence_revision().unwrap())
        .unwrap();
    assert_eq!(fresh.current_session().generation.get(), 2);
    assert_eq!(
        fresh.current_session().state,
        NativeSessionState::Reconciling
    );
    assert_eq!(fresh.slots(), slots().as_slice());
    assert!(matches!(
        store.commit_fresh_interruption_binding(&fresh, state.persistence_revision().unwrap()),
        Err(ControllerStoreError::RevisionConflict)
    ));
    drop(store);
    let mut reopened = ControllerSqliteStore::open_existing(&path).unwrap();
    assert_eq!(reopened.load_native_binding(id).unwrap(), Some(fresh));
}

#[test]
fn initial_binding_requires_fresh_resource_sampling_checkpoint() {
    let mut store = ControllerSqliteStore::in_memory().unwrap();
    let id = AttemptId::new(45);
    let interrupted = start(&mut store, id)
        .interrupt()
        .unwrap()
        .persist(&mut store)
        .unwrap()
        .into_parts()
        .0;
    assert!(matches!(
        store.commit_initial_binding(
            id,
            interrupted.deadline(),
            interrupted.persistence_revision().unwrap(),
            guardian(),
            &slots()
        ),
        Err(ControllerStoreError::RevisionConflict)
    ));
    assert!(matches!(
        store.load_native_binding(id),
        Err(ControllerStoreError::Poisoned)
    ));
}

#[test]
fn stale_duplicate_and_inexact_bindings_fail_stopped() {
    let mut store = ControllerSqliteStore::in_memory().unwrap();
    let id = AttemptId::new(3);
    let state = start(&mut store, id);
    assert!(matches!(
        store.commit_initial_binding(
            id,
            MonotonicTick::new(101),
            PersistenceRevision::first(),
            guardian(),
            &slots()
        ),
        Err(ControllerStoreError::RevisionConflict)
    ));
    assert!(matches!(
        store.load_native_binding(id),
        Err(ControllerStoreError::Poisoned)
    ));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("duplicate.sqlite");
    let mut store = ControllerSqliteStore::open(&path).unwrap();
    start(&mut store, id);
    let binding = store
        .commit_initial_binding(
            id,
            state.deadline(),
            PersistenceRevision::first(),
            guardian(),
            &slots(),
        )
        .unwrap();
    assert!(matches!(
        store.commit_initial_binding(
            id,
            state.deadline(),
            PersistenceRevision::first(),
            guardian(),
            &slots()
        ),
        Err(ControllerStoreError::RevisionConflict)
    ));
    drop(store);
    let mut store = ControllerSqliteStore::open_existing(&path).unwrap();
    let detached = store
        .commit_detached_binding(&binding, binding.slots())
        .unwrap();
    assert!(matches!(
        store.commit_detached_binding(&binding, binding.slots()),
        Err(ControllerStoreError::RevisionConflict)
    ));
    assert!(matches!(
        store.load_native_binding(id),
        Err(ControllerStoreError::Poisoned)
    ));
    drop(detached);
}

#[test]
fn schema_refuses_invalid_native_identity_inventory_and_unfinished_session_generation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("checks.sqlite");
    let mut store = ControllerSqliteStore::open(&path).unwrap();
    let id = AttemptId::new(1);
    start(&mut store, id);
    let binding = store
        .commit_initial_binding(
            id,
            MonotonicTick::new(100),
            PersistenceRevision::first(),
            guardian(),
            &slots(),
        )
        .unwrap();
    let raw = Connection::open(&path).unwrap();
    raw.pragma_update(None, "foreign_keys", true).unwrap();
    let key = id.get().to_be_bytes();
    for (sql, values) in [
        (
            "UPDATE guardian_bindings SET nonce=?2 WHERE attempt_id=?1",
            vec![0_u8; 16],
        ),
        (
            "UPDATE guardian_bindings SET nonce=?2 WHERE attempt_id=?1",
            vec![1_u8; 15],
        ),
        (
            "UPDATE guardian_bindings SET revision=?2 WHERE attempt_id=?1",
            vec![0_u8; 8],
        ),
    ] {
        assert!(raw.execute(sql, params![key.as_slice(), values]).is_err());
    }
    assert!(raw
        .execute(
            "UPDATE guardian_bindings SET guardian_pid=0 WHERE attempt_id=?1",
            [key.as_slice()]
        )
        .is_err());
    assert!(raw
        .execute(
            "UPDATE guardian_bindings SET guardian_birth_nanos=1000000000 WHERE attempt_id=?1",
            [key.as_slice()]
        )
        .is_err());
    assert!(raw
        .execute(
            "UPDATE guardian_bindings SET guardian_birth_seconds=?2 WHERE attempt_id=?1",
            params![key.as_slice(), [1u8; 7].as_slice()]
        )
        .is_err());
    assert!(raw
        .execute(
            "INSERT INTO guardian_sessions VALUES (?1,?2,?3,'detached')",
            params![
                key.as_slice(),
                [0u8; 8].as_slice(),
                2u64.to_be_bytes().as_slice()
            ]
        )
        .is_err());
    assert!(raw
        .execute(
            "INSERT INTO guardian_sessions VALUES (?1,?2,?3,'active')",
            params![
                key.as_slice(),
                2u64.to_be_bytes().as_slice(),
                2u64.to_be_bytes().as_slice()
            ]
        )
        .is_err());
    assert!(raw
        .execute(
            "INSERT INTO guardian_slots VALUES (?1, 2, 101, ?2, 34, 'running')",
            params![key.as_slice(), 12u64.to_be_bytes().as_slice()]
        )
        .is_err());
    raw.execute(
        "DELETE FROM guardian_slots WHERE attempt_id=?1 AND ordinal=1",
        [key.as_slice()],
    )
    .unwrap();
    assert!(matches!(
        store.load_native_binding(id),
        Err(ControllerStoreError::Malformed(_))
    ));
    assert!(matches!(
        store.load_native_binding(id),
        Err(ControllerStoreError::Poisoned)
    ));
    assert_eq!(binding.slots().len(), 2);
}

#[test]
fn version_one_is_rejected_without_schema_changes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v1.sqlite");
    let db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE legacy (value TEXT); INSERT INTO legacy VALUES ('unchanged'); PRAGMA user_version=1;").unwrap();
    assert!(matches!(
        ControllerSqliteStore::open(&path),
        Err(ControllerStoreError::UnsupportedSchema(1))
    ));
    assert!(matches!(
        ControllerSqliteStore::open_existing(&path),
        Err(ControllerStoreError::UnsupportedSchema(1))
    ));
    assert_eq!(
        db.query_row("SELECT value FROM legacy", [], |row| row
            .get::<_, String>(0))
            .unwrap(),
        "unchanged"
    );
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE name='guardian_bindings'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}

#[test]
fn duplicate_nonce_and_native_identity_cannot_replace_another_attempt() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("unique.sqlite");
    let mut store = ControllerSqliteStore::open(&path).unwrap();
    let first = AttemptId::new(7);
    let second = AttemptId::new(8);
    start(&mut store, first);
    start(&mut store, second);
    store
        .commit_initial_binding(
            first,
            MonotonicTick::new(100),
            PersistenceRevision::first(),
            guardian(),
            &slots(),
        )
        .unwrap();
    let mut different_guardian = guardian();
    different_guardian.process = process(110);
    assert!(matches!(
        store.commit_initial_binding(
            second,
            MonotonicTick::new(100),
            PersistenceRevision::first(),
            different_guardian,
            &[]
        ),
        Err(ControllerStoreError::Sqlite(_))
    ));
    drop(store);
    let mut store = ControllerSqliteStore::open_existing(&path).unwrap();
    different_guardian.nonce = GuardianLifetimeNonce::new([2; 16]).unwrap();
    different_guardian.process = guardian().process;
    assert!(matches!(
        store.commit_initial_binding(
            second,
            MonotonicTick::new(100),
            PersistenceRevision::first(),
            different_guardian,
            &[]
        ),
        Err(ControllerStoreError::Sqlite(_))
    ));
    drop(store);
    let mut store = ControllerSqliteStore::open_existing(&path).unwrap();
    different_guardian.process = process(110);
    assert!(matches!(
        store.commit_initial_binding(
            second,
            MonotonicTick::new(100),
            PersistenceRevision::first(),
            different_guardian,
            &slots()
        ),
        Err(ControllerStoreError::Sqlite(_))
    ));
}

#[test]
fn malformed_session_progression_poisons_binding_reader() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("generation.sqlite");
    let mut store = ControllerSqliteStore::open(&path).unwrap();
    let id = AttemptId::new(9);
    start(&mut store, id);
    store
        .commit_initial_binding(
            id,
            MonotonicTick::new(100),
            PersistenceRevision::first(),
            guardian(),
            &[],
        )
        .unwrap();
    let raw = Connection::open(&path).unwrap();
    let key = id.get().to_be_bytes();
    raw.execute(
        "INSERT INTO guardian_sessions VALUES (?1,?2,?3,'detached')",
        params![
            key.as_slice(),
            3u64.to_be_bytes().as_slice(),
            2u64.to_be_bytes().as_slice()
        ],
    )
    .unwrap();
    assert!(matches!(
        store.load_native_binding(id),
        Err(ControllerStoreError::Malformed(_))
    ));
    assert!(matches!(
        store.load_native_binding(id),
        Err(ControllerStoreError::Poisoned)
    ));
}
