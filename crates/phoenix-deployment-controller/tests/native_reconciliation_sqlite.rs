use phoenix_deployment_controller::{
    AttemptId, BirthToken, ControllerSqliteStore, ControllerStoreError, CpuContentionBasisPoints,
    DeploymentAttempt, GuardianLifetimeNonce, LoadedAttempt, MemoryPressure, MonotonicTick,
    NativeGuardianBinding, NativeGuardianIdentity, OsEffect, PersistenceRevision, Phase,
    PolicyLoop, ProcessId, ProcessIdentity, ResourceCapacity, ResourceContention, ResourcePolicy,
    ResourceSample,
};
use rusqlite::Connection;
use std::{
    num::{NonZeroU32, NonZeroU64, NonZeroUsize},
    path::Path,
};

fn prepared(path: &Path) -> (ControllerSqliteStore, LoadedAttempt, NativeGuardianBinding) {
    let deadline = MonotonicTick::native_now()
        .unwrap()
        .checked_add(std::time::Duration::from_secs(60))
        .unwrap();
    prepared_until(path, deadline)
}

fn prepared_until(
    path: &Path,
    deadline: MonotonicTick,
) -> (ControllerSqliteStore, LoadedAttempt, NativeGuardianBinding) {
    let mut store = ControllerSqliteStore::open(path).unwrap();
    let identity = ProcessIdentity::new(
        ProcessId::new(NonZeroU32::new(42).unwrap()),
        BirthToken::new(1, 2).unwrap(),
    );
    let committed = DeploymentAttempt::start(
        AttemptId::new(1),
        deadline,
        ResourcePolicy::new(
            NonZeroUsize::new(2).unwrap(),
            0,
            0,
            CpuContentionBasisPoints::new(100).unwrap(),
            MemoryPressure::Critical,
        ),
        identity,
    )
    .persist(&mut store)
    .unwrap();
    let binding = store
        .commit_initial_binding(
            AttemptId::new(1),
            deadline,
            PersistenceRevision::first(),
            NativeGuardianIdentity {
                nonce: GuardianLifetimeNonce::new([1; 16]).unwrap(),
                process: identity,
            },
            &[],
        )
        .unwrap();
    let binding = store.commit_detached_binding(&binding, &[]).unwrap();
    let interruption = committed
        .into_parts()
        .0
        .interrupt()
        .unwrap()
        .persist(&mut store)
        .unwrap();
    let revision = interruption.checkpoint().persistence_revision().unwrap();
    let binding = store
        .commit_fresh_interruption_binding(&binding, revision)
        .unwrap();
    let loaded = store.load_attempt(AttemptId::new(1)).unwrap().unwrap();
    (store, loaded, binding)
}

type DurableRows = Vec<(String, Vec<Vec<rusqlite::types::Value>>)>;

fn durable_snapshot(path: &Path) -> DurableRows {
    let raw = Connection::open(path).unwrap();
    let tables: Vec<String> = raw
        .prepare("SELECT name FROM sqlite_schema WHERE type='table' ORDER BY name")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    tables
        .into_iter()
        .map(|table| {
            let mut statement = raw
                .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
                .unwrap();
            let columns = statement.column_count();
            let rows = statement
                .query_map([], |r| {
                    (0..columns)
                        .map(|i| r.get(i))
                        .collect::<Result<Vec<rusqlite::types::Value>, _>>()
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            (table, rows)
        })
        .collect()
}

fn assert_reopen_unchanged(path: &Path, before: &DurableRows) {
    let mut reopened = ControllerSqliteStore::open_existing(path).unwrap();
    assert!(reopened.load_attempt(AttemptId::new(1)).unwrap().is_some());
    assert!(reopened
        .load_native_binding(AttemptId::new(1))
        .unwrap()
        .is_some());
    assert_eq!(&durable_snapshot(path), before);
}

#[test]
fn recovery_retains_original_affine_plan_and_atomic_exact_completion() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("controller.sqlite");
    let (mut store, loaded, binding) = prepared(&path);
    store
        .preflight_native_sampling_recovery(binding.attempt_id())
        .unwrap();
    let mut reader = ControllerSqliteStore::open_read_only(&path).unwrap();
    let generation = binding.current_session().generation;
    assert!(reader
        .read_native_reconciliation_evidence(binding.attempt_id(), generation)
        .unwrap()
        .is_none());
    let committed = store
        .commit_native_sampling_recovery_before_deadline(loaded, &binding)
        .unwrap();
    let revision = committed.checkpoint().persistence_revision().unwrap();
    let (checkpoint, durable_binding, completion) = reader
        .read_native_reconciliation_evidence(binding.attempt_id(), generation)
        .unwrap()
        .unwrap();
    assert_eq!(checkpoint.phase(), Phase::ResourceSampling);
    assert_eq!(checkpoint.persistence_revision(), revision);
    assert_eq!(durable_binding, binding);
    assert_eq!(
        completion.interruption_revision(),
        binding.current_session().interruption_revision
    );
    assert_eq!(completion.recovery_revision(), revision);
    assert_eq!(completion.binding_revision(), binding.revision());
    let mut policy = PolicyLoop::from_committed(committed);
    {
        let ticket = policy.issue_next().unwrap();
        assert_eq!(ticket.key().revision(), revision);
        assert_eq!(*ticket.effect(), OsEffect::SampleResources { sequence: 0 });
        assert!(policy.issue_next().is_none());
    }
    let raw = Connection::open(&path).unwrap();
    assert_eq!(
        raw.query_row("SELECT count(*) FROM native_sampling_issues", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        raw.query_row(
            "SELECT count(*) FROM native_reconciliation_completions",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
}

#[test]
fn completion_insert_failure_rolls_back_checkpoint_and_poisons_store() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("controller.sqlite");
    let (mut store, loaded, binding) = prepared(&path);
    let raw = Connection::open(&path).unwrap();
    raw.execute_batch("CREATE TRIGGER reject_completion BEFORE INSERT ON native_reconciliation_completions BEGIN SELECT RAISE(ABORT, 'fixture'); END;").unwrap();
    assert!(store
        .commit_native_sampling_recovery_before_deadline(loaded, &binding)
        .is_err());
    assert!(matches!(
        store.load_attempt(binding.attempt_id()),
        Err(ControllerStoreError::Poisoned)
    ));
    let mut reopened = ControllerSqliteStore::open_existing(&path).unwrap();
    let durable = reopened
        .load_attempt(binding.attempt_id())
        .unwrap()
        .unwrap();
    assert_eq!(durable.phase(), Phase::Interrupted);
    assert_eq!(
        durable.persistence_revision(),
        binding.current_session().interruption_revision
    );
    assert_eq!(
        raw.query_row(
            "SELECT count(*) FROM native_reconciliation_completions",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}

#[test]
fn stale_loaded_interruption_wrong_binding_state_and_replay_are_fail_stop() {
    for mismatch in ["loaded", "binding", "state", "replay"] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("controller.sqlite");
        let (mut store, loaded, binding) = prepared(&path);
        let (loaded, supplied) = match mismatch {
            "loaded" => {
                store
                    .load_attempt(binding.attempt_id())
                    .unwrap()
                    .unwrap()
                    .refresh_interruption()
                    .unwrap()
                    .persist(&mut store)
                    .unwrap();
                (loaded, binding.clone())
            }
            "binding" => {
                store.commit_detached_binding(&binding, &[]).unwrap();
                (loaded, binding.clone())
            }
            "state" => {
                let detached = store.commit_detached_binding(&binding, &[]).unwrap();
                (loaded, detached)
            }
            "replay" => {
                let second = store.load_attempt(binding.attempt_id()).unwrap().unwrap();
                store
                    .commit_native_sampling_recovery_before_deadline(loaded, &binding)
                    .unwrap();
                (second, binding.clone())
            }
            _ => unreachable!(),
        };
        let before = durable_snapshot(&path);
        assert!(store
            .commit_native_sampling_recovery_before_deadline(loaded, &supplied)
            .is_err());
        assert!(matches!(
            store.load_attempt(binding.attempt_id()),
            Err(ControllerStoreError::Poisoned)
        ));
        drop(store);
        assert_reopen_unchanged(&path, &before);
    }
}

#[test]
fn malformed_completion_and_later_fencing_cannot_restore_authority() {
    for corruption in [
        "UPDATE guardian_sessions SET state='detached' WHERE generation=x'0000000000000002'",
        "UPDATE guardian_bindings SET revision=x'0000000000000004'",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("controller.sqlite");
        let (mut store, loaded, binding) = prepared(&path);
        store
            .commit_native_sampling_recovery_before_deadline(loaded, &binding)
            .unwrap();
        Connection::open(&path)
            .unwrap()
            .execute_batch(corruption)
            .unwrap();
        let mut reader = ControllerSqliteStore::open_read_only(&path).unwrap();
        assert!(reader
            .read_native_reconciliation_evidence(
                binding.attempt_id(),
                binding.current_session().generation
            )
            .is_err());
        assert!(matches!(
            reader.load_attempt(binding.attempt_id()),
            Err(ControllerStoreError::Poisoned)
        ));
    }
}

#[test]
fn unsupported_source_phase_and_unsettled_issue_refuse_before_recovery() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("controller.sqlite");
    let (mut store, loaded, binding) = prepared(&path);
    store
        .commit_native_sampling_recovery_before_deadline(loaded, &binding)
        .unwrap();
    let active = store.load_attempt(binding.attempt_id()).unwrap().unwrap();
    assert!(store
        .commit_native_sampling_recovery_before_deadline(active, &binding)
        .is_err());

    let path = dir.path().join("pending.sqlite");
    let (mut store, _, binding) = prepared(&path);
    Connection::open(&path).unwrap().execute_batch("INSERT INTO native_sampling_issues VALUES (x'00000000000000000000000000000001',x'0000000000000001',0,x'0000000000000001',zeroblob(8))").unwrap();
    let before = durable_snapshot(&path);
    assert!(store
        .preflight_native_sampling_recovery(binding.attempt_id())
        .is_err());
    drop(store);
    assert_reopen_unchanged(&path, &before);
    let mut store = ControllerSqliteStore::open_existing(&path).unwrap();
    let loaded = store.load_attempt(binding.attempt_id()).unwrap().unwrap();
    assert!(store
        .commit_native_sampling_recovery_before_deadline(loaded, &binding)
        .is_err());
    assert_reopen_unchanged(&path, &before);
    assert!(matches!(
        store.load_attempt(binding.attempt_id()),
        Err(ControllerStoreError::Poisoned)
    ));
}

#[test]
fn interrupted_ownership_cannot_use_sampling_reconciliation_completion() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ownership.sqlite");
    let mut store = ControllerSqliteStore::open(&path).unwrap();
    let attempt = AttemptId::new(2);
    let identity = ProcessIdentity::new(
        ProcessId::new(NonZeroU32::new(43).unwrap()),
        BirthToken::new(2, 3).unwrap(),
    );
    let committed = DeploymentAttempt::start(
        attempt,
        MonotonicTick::new(500),
        ResourcePolicy::new(
            NonZeroUsize::new(1).unwrap(),
            0,
            0,
            CpuContentionBasisPoints::new(100).unwrap(),
            MemoryPressure::Critical,
        ),
        identity,
    )
    .persist(&mut store)
    .unwrap();
    let binding = store
        .commit_initial_binding(
            attempt,
            MonotonicTick::new(500),
            PersistenceRevision::first(),
            NativeGuardianIdentity {
                nonce: GuardianLifetimeNonce::new([2; 16]).unwrap(),
                process: identity,
            },
            &[],
        )
        .unwrap();
    let active = committed
        .into_parts()
        .0
        .record_resource_sample(ResourceSample {
            sequence: 0,
            capacity: ResourceCapacity {
                available_memory_bytes: u64::MAX,
                available_storage_bytes: u64::MAX,
            },
            contention: ResourceContention {
                cpu: CpuContentionBasisPoints::new(0).unwrap(),
                memory_pressure: MemoryPressure::Nominal,
            },
        })
        .unwrap()
        .persist(&mut store)
        .unwrap()
        .into_parts()
        .0;
    assert_eq!(active.phase(), Phase::OwnershipDiscovery);
    let binding = store.commit_detached_binding(&binding, &[]).unwrap();
    let interruption = active.interrupt().unwrap().persist(&mut store).unwrap();
    let binding = store
        .commit_fresh_interruption_binding(
            &binding,
            interruption.checkpoint().persistence_revision().unwrap(),
        )
        .unwrap();
    assert!(store.preflight_native_sampling_recovery(attempt).is_err());
    let mut reopened = ControllerSqliteStore::open_existing(&path).unwrap();
    let loaded = reopened.load_attempt(attempt).unwrap().unwrap();
    assert_eq!(loaded.phase(), Phase::Interrupted);
    assert!(reopened
        .commit_native_sampling_recovery_before_deadline(loaded, &binding)
        .is_err());
}

#[test]
fn missing_completion_is_not_an_error_or_a_permit() {
    let dir = tempfile::tempdir().unwrap();
    let (mut store, _, binding) = prepared(&dir.path().join("controller.sqlite"));
    assert!(store
        .read_native_reconciliation_evidence(binding.attempt_id(), NonZeroU64::new(9).unwrap())
        .unwrap()
        .is_none());
}

#[test]
fn direct_sql_completion_requires_exact_checkpoint_binding_and_current_reconciling_session() {
    for contradiction in [
        "",
        "UPDATE attempts SET revision=x'0000000000000004',phase='sampling',interrupted_phase=NULL",
        "UPDATE attempts SET revision=x'0000000000000003'",
        "UPDATE attempts SET revision=x'0000000000000003',phase='sampling',interrupted_phase=NULL; UPDATE guardian_bindings SET revision=x'0000000000000004'",
        "UPDATE attempts SET revision=x'0000000000000003',phase='sampling',interrupted_phase=NULL; UPDATE guardian_bindings SET dispatch_floor=x'0000000000000001'",
        "UPDATE attempts SET revision=x'0000000000000003',phase='sampling',interrupted_phase=NULL; UPDATE guardian_sessions SET state='detached'",
        "UPDATE attempts SET revision=x'0000000000000003',phase='sampling',interrupted_phase=NULL; UPDATE guardian_sessions SET interruption_revision=x'0000000000000003' WHERE generation=x'0000000000000002'",
        "UPDATE attempts SET revision=x'0000000000000003',phase='sampling',interrupted_phase=NULL; UPDATE guardian_sessions SET state='detached'; INSERT INTO guardian_sessions VALUES (x'00000000000000000000000000000001',x'0000000000000003',x'0000000000000003','reconciling')",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("constraint.sqlite");
        let (_store, _, _) = prepared(&path);
        let raw = Connection::open(&path).unwrap();
        raw.execute_batch("PRAGMA foreign_keys=ON").unwrap();
        raw.execute_batch(contradiction).unwrap();
        assert_eq!(raw.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
        let before = durable_snapshot(&path);
        let error = raw.execute_batch("INSERT INTO native_reconciliation_completions VALUES (x'00000000000000000000000000000001',x'0000000000000002',x'0000000000000002',x'0000000000000003',x'0000000000000003')").unwrap_err();
        assert_trigger_abort(&error, "completion requires exact prepared checkpoint and session");
        assert_eq!(durable_snapshot(&path), before);
    }
}

#[test]
fn direct_sql_activation_requires_exact_completion_checkpoint_binding_and_active_session() {
    for contradiction in [
        "",
        "UPDATE guardian_sessions SET state='active' WHERE generation=x'0000000000000002'",
        "UPDATE guardian_bindings SET revision=x'0000000000000004',dispatch_floor=x'0000000000000003'",
        "UPDATE guardian_sessions SET state='active' WHERE generation=x'0000000000000002'; UPDATE guardian_bindings SET revision=x'0000000000000005',dispatch_floor=x'0000000000000003'",
        "UPDATE guardian_sessions SET state='active' WHERE generation=x'0000000000000002'; UPDATE guardian_bindings SET revision=x'0000000000000004',dispatch_floor=x'0000000000000002'",
        "UPDATE guardian_sessions SET state='active' WHERE generation=x'0000000000000002'; UPDATE guardian_bindings SET revision=x'0000000000000004',dispatch_floor=x'0000000000000003'; UPDATE attempts SET revision=x'0000000000000004'",
        "UPDATE guardian_sessions SET state='active' WHERE generation=x'0000000000000002'; UPDATE guardian_bindings SET revision=x'0000000000000004',dispatch_floor=x'0000000000000003'; UPDATE attempts SET phase='interrupted',interrupted_phase='sampling'",
        "UPDATE guardian_sessions SET state='detached' WHERE generation=x'0000000000000002'; UPDATE guardian_bindings SET revision=x'0000000000000004',dispatch_floor=x'0000000000000003'; INSERT INTO guardian_sessions VALUES (x'00000000000000000000000000000001',x'0000000000000003',x'0000000000000003','reconciling')",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("constraint.sqlite");
        let (mut store, loaded, binding) = prepared(&path);
        store.commit_native_sampling_recovery_before_deadline(loaded, &binding).unwrap();
        let raw = Connection::open(&path).unwrap();
        raw.execute_batch("PRAGMA foreign_keys=ON").unwrap();
        raw.execute_batch(contradiction).unwrap();
        assert_eq!(raw.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
        let before = durable_snapshot(&path);
        let error = raw.execute_batch("INSERT INTO native_reconciliation_activations VALUES (x'00000000000000000000000000000001',x'0000000000000002',x'0000000000000004')").unwrap_err();
        assert_trigger_abort(&error, "activation requires exact active checkpoint and session");
        assert_eq!(durable_snapshot(&path), before);
    }
}

fn assert_trigger_abort(error: &rusqlite::Error, expected: &str) {
    assert!(
        matches!(error, rusqlite::Error::SqliteFailure(ref code, Some(ref message)) if code.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_TRIGGER && message == expected),
        "{error:?}"
    );
}

#[test]
fn expired_after_begin_cannot_prepare_completion_or_change_checkpoint() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("expired.sqlite");
    let (mut store, loaded, binding) = prepared_until(&path, MonotonicTick::new(1));
    store
        .preflight_native_sampling_recovery(binding.attempt_id())
        .unwrap();
    let before = durable_snapshot(&path);
    assert!(matches!(
        store.commit_native_sampling_recovery_before_deadline(loaded, &binding),
        Err(ControllerStoreError::RevisionConflict)
    ));
    assert!(matches!(
        store.load_attempt(binding.attempt_id()),
        Err(ControllerStoreError::Poisoned)
    ));
    assert_reopen_unchanged(&path, &before);
    let raw = Connection::open(&path).unwrap();
    assert_eq!(
        raw.query_row(
            "SELECT count(*) FROM native_reconciliation_completions",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}
