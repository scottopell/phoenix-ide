use crate as phoenix_deployment_controller;
use crate::{
    AttemptId, BirthToken, ControllerSqliteStore, ControllerStoreError, CpuContentionBasisPoints,
    DeploymentAttempt, GuardianLifetimeNonce, LoadedAttempt, MemoryPressure, MonotonicTick,
    NativeGuardianBinding, NativeGuardianIdentity, OsEffect, PersistenceRevision, Phase,
    PreparedNativeReconciliation, ProcessId, ProcessIdentity, ResourcePolicy,
};
use rusqlite::Connection;
use std::{
    num::{NonZeroU32, NonZeroUsize},
    path::Path,
};

fn prepared(path: &Path) -> (ControllerSqliteStore, LoadedAttempt, NativeGuardianBinding) {
    let mut store = ControllerSqliteStore::open(path).unwrap();
    let identity = ProcessIdentity::new(
        ProcessId::new(NonZeroU32::new(42).unwrap()),
        BirthToken::new(1, 2).unwrap(),
    );
    let committed = DeploymentAttempt::start(
        AttemptId::new(1),
        MonotonicTick::new(500),
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
            MonotonicTick::new(500),
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
#[allow(clippy::too_many_lines)]
fn activation_is_atomic_and_retains_prepared_completion_as_immutable_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("activation.sqlite");
    let (mut store, loaded, binding) = prepared(&path);
    let committed = store
        .commit_native_sampling_recovery_at(loaded, &binding, crate::MonotonicTick::new(0))
        .unwrap();
    let generation = binding.current_session().generation;
    let (_, _, completion) = store
        .read_native_reconciliation_evidence(binding.attempt_id(), generation)
        .unwrap()
        .unwrap();
    let (active, activation, committed_activation) = store
        .commit_native_reconciliation_activation_at(
            &binding,
            completion,
            PreparedNativeReconciliation::for_test(&binding, completion),
            MonotonicTick::new(499),
        )
        .unwrap();
    assert_eq!(
        active.current_session().state,
        phoenix_deployment_controller::NativeSessionState::Active
    );
    assert_eq!(
        active.revision(),
        binding.revision().checked_next().unwrap()
    );
    let (locator, id, recovery, active_revision) = committed_activation.into_locator();
    assert_eq!(locator.generation, generation.get());
    assert_eq!(id, binding.attempt_id());
    assert_eq!(recovery, completion.recovery_revision());
    assert_eq!(active_revision, active.revision());
    let (checkpoint, diagnostic_binding, diagnostic_activation) = store
        .read_native_reconciliation_activation_evidence(binding.attempt_id(), generation)
        .unwrap()
        .unwrap();
    assert_eq!(checkpoint.persistence_revision(), recovery);
    assert_eq!(diagnostic_binding, active);
    assert_eq!(diagnostic_activation, activation);
    assert_eq!(active.dispatch_floor(), completion.recovery_revision());
    assert_eq!(active.identity(), binding.identity());
    assert_eq!(active.slots(), binding.slots());
    assert_eq!(activation.completion(), completion);
    assert_eq!(activation.binding_revision(), active.revision());
    drop(committed);
    drop(store);
    let mut reopened = ControllerSqliteStore::open_existing(&path).unwrap();
    assert_eq!(
        reopened
            .load_native_binding(binding.attempt_id())
            .unwrap()
            .unwrap(),
        active
    );
    assert_eq!(
        reopened
            .load_attempt(binding.attempt_id())
            .unwrap()
            .unwrap()
            .persistence_revision(),
        completion.recovery_revision()
    );
    assert_eq!(
        reopened
            .load_native_reconciliation_activation(binding.attempt_id(), generation)
            .unwrap(),
        Some(activation)
    );
    let raw = Connection::open(&path).unwrap();
    for sql in [
        "UPDATE native_reconciliation_completions SET recovery_revision=x'0000000000000004'",
        "UPDATE native_reconciliation_completions SET binding_revision=x'0000000000000001'",
        "DELETE FROM native_reconciliation_completions",
        "UPDATE native_reconciliation_activations SET binding_revision=x'0000000000000005'",
        "DELETE FROM native_reconciliation_activations",
    ] {
        let before = durable_snapshot(&path);
        let error = raw.execute_batch(sql).unwrap_err();
        let expected = if sql.contains("completions") {
            "prepared completion is immutable"
        } else {
            "activation is immutable"
        };
        assert!(
            matches!(error, rusqlite::Error::SqliteFailure(ref code, Some(ref message)) if code.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_TRIGGER && message == expected),
            "{error:?}"
        );
        assert_eq!(durable_snapshot(&path), before);
    }
    let detached = reopened.commit_detached_binding(&active, &[]).unwrap();
    assert_eq!(
        detached.current_session().state,
        phoenix_deployment_controller::NativeSessionState::Detached
    );
    assert_eq!(detached.dispatch_floor(), completion.recovery_revision());
    assert_eq!(
        reopened
            .load_native_reconciliation_activation(binding.attempt_id(), generation)
            .unwrap(),
        Some(activation)
    );
    commit_interruption_for_test(&mut reopened, binding.attempt_id());
    let interrupted = reopened
        .load_attempt(binding.attempt_id())
        .unwrap()
        .unwrap();
    assert_eq!(interrupted.phase(), Phase::Interrupted);
    assert_eq!(interrupted.deadline(), binding.deadline());
    assert!(interrupted.persistence_revision() > completion.recovery_revision());
    let next = reopened
        .commit_fresh_interruption_binding(&detached, interrupted.persistence_revision())
        .unwrap();
    assert!(next.current_session().generation > generation);
    let recovered = reopened
        .commit_native_sampling_recovery_at(interrupted, &next, crate::MonotonicTick::new(0))
        .unwrap();
    assert_eq!(
        recovered.after_persist(),
        &[OsEffect::SampleResources { sequence: 0 }]
    );
    assert_eq!(
        raw.query_row("SELECT count(*) FROM native_sampling_issues", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn activation_mismatch_replay_and_insert_failure_leave_every_durable_row_unchanged() {
    for case in [
        "binding",
        "session",
        "checkpoint",
        "completion",
        "replay",
        "rollback",
        "expired",
        "late",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("activation.sqlite");
        let (mut store, loaded, binding) = prepared(&path);
        store
            .commit_native_sampling_recovery_at(loaded, &binding, crate::MonotonicTick::new(0))
            .unwrap();
        let (_, _, mut completion) = store
            .read_native_reconciliation_evidence(
                binding.attempt_id(),
                binding.current_session().generation,
            )
            .unwrap()
            .unwrap();
        let raw = Connection::open(&path).unwrap();
        match case {
            "binding" => {
                store.commit_detached_binding(&binding, &[]).unwrap();
            }
            "session" => {
                raw.execute_batch("UPDATE guardian_sessions SET state='detached'")
                    .unwrap();
            }
            "checkpoint" => {
                commit_interruption_for_test(&mut store, binding.attempt_id());
            }
            "completion" => {
                let other_path = dir.path().join("other.sqlite");
                let (mut other, loaded, other_binding) = prepared(&other_path);
                let interrupted = loaded
                    .refresh_interruption()
                    .unwrap()
                    .persist(&mut other)
                    .unwrap();
                let other_binding = other
                    .commit_fresh_interruption_binding(
                        &other_binding,
                        interrupted.checkpoint().persistence_revision().unwrap(),
                    )
                    .unwrap();
                let loaded = other
                    .load_attempt(other_binding.attempt_id())
                    .unwrap()
                    .unwrap();
                other
                    .commit_native_sampling_recovery_at(
                        loaded,
                        &other_binding,
                        crate::MonotonicTick::new(0),
                    )
                    .unwrap();
                completion = other
                    .read_native_reconciliation_evidence(
                        other_binding.attempt_id(),
                        other_binding.current_session().generation,
                    )
                    .unwrap()
                    .unwrap()
                    .2;
            }
            "replay" => {
                store
                    .commit_native_reconciliation_activation_at(
                        &binding,
                        completion,
                        PreparedNativeReconciliation::for_test(&binding, completion),
                        MonotonicTick::new(499),
                    )
                    .unwrap();
            }
            "rollback" => {
                raw.execute_batch("CREATE TRIGGER reject_activation BEFORE INSERT ON native_reconciliation_activations BEGIN SELECT RAISE(ABORT, 'fixture'); END;").unwrap();
            }
            "expired" | "late" => {}
            _ => unreachable!(),
        }
        let before = durable_snapshot(&path);
        assert!(
            store
                .commit_native_reconciliation_activation_at(
                    &binding,
                    completion,
                    PreparedNativeReconciliation::for_test(&binding, completion),
                    MonotonicTick::new(match case {
                        "expired" => 500,
                        "late" => 501,
                        _ => 499,
                    })
                )
                .is_err(),
            "{case}"
        );
        assert!(matches!(
            store.load_native_binding(binding.attempt_id()),
            Err(ControllerStoreError::Poisoned)
        ));
        drop(store);
        assert_reopen_unchanged(&path, &before);
    }
}

fn commit_interruption_for_test(store: &mut ControllerSqliteStore, id: AttemptId) {
    let loaded = store.load_attempt(id).unwrap().unwrap();
    let phoenix_deployment_controller::RecoveryDisposition::Checkpoint(transition) =
        loaded.prepare_recovery().unwrap()
    else {
        panic!("expected interruption");
    };
    transition.persist(store).unwrap();
}

#[test]
fn prepared_capability_must_match_exact_activation_binding_and_completion() {
    for mismatch in ["attempt", "session", "identity", "deadline", "revision"] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("prepared.sqlite");
        let (mut store, loaded, binding) = prepared(&path);
        store
            .commit_native_sampling_recovery_at(loaded, &binding, MonotonicTick::new(499))
            .unwrap();
        let (_, _, completion) = store
            .read_native_reconciliation_evidence(
                binding.attempt_id(),
                binding.current_session().generation,
            )
            .unwrap()
            .unwrap();
        let mut proof = PreparedNativeReconciliation::for_test(&binding, completion);
        match mismatch {
            "attempt" => proof.attempt = AttemptId::new(2),
            "session" => proof.session.generation += 1,
            "identity" => proof.identity.nonce = GuardianLifetimeNonce::new([9; 16]).unwrap(),
            "deadline" => proof.deadline = MonotonicTick::new(501),
            "revision" => proof.revision = completion.interruption_revision(),
            _ => unreachable!(),
        }
        let before = durable_snapshot(&path);
        assert!(
            store
                .commit_native_reconciliation_activation_at(
                    &binding,
                    completion,
                    proof,
                    MonotonicTick::new(499)
                )
                .is_err(),
            "{mismatch}"
        );
        assert_reopen_unchanged(&path, &before);
    }
}
