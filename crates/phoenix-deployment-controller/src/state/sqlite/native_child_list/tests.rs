use super::*;
use crate::{
    CpuContentionBasisPoints, EffectCompletion, ExecutedEffect, GuardianLifetimeNonce,
    LoopProgress, MemoryPressure, NativeGuardianSlot, NativeSlotLiveness, PolicyLoop,
    ResourceCapacity, ResourceContention, ResourcePolicy, ResourceSample,
};
use std::num::NonZeroUsize;

fn identity(pid: u32) -> ProcessIdentity {
    ProcessIdentity::new(
        ProcessId::new(NonZeroU32::new(pid).unwrap()),
        BirthToken::new(12, 3).unwrap(),
    )
}

fn setup(
    slots: &[NativeGuardianSlot],
) -> (ControllerSqliteStore, PolicyLoop, NativeGuardianBinding) {
    let mut store = ControllerSqliteStore::in_memory().unwrap();
    let committed = DeploymentAttempt::start(
        crate::AttemptId::new(9),
        MonotonicTick::new(100),
        ResourcePolicy::new(
            NonZeroUsize::new(1).unwrap(),
            0,
            0,
            CpuContentionBasisPoints::new(10_000).unwrap(),
            MemoryPressure::Critical,
        ),
        identity(42),
    )
    .persist(&mut store)
    .unwrap();
    let binding = store
        .commit_initial_binding(
            crate::AttemptId::new(9),
            MonotonicTick::new(100),
            PersistenceRevision::first(),
            NativeGuardianIdentity {
                nonce: GuardianLifetimeNonce::new([5; 16]).unwrap(),
                process: identity(100),
            },
            slots,
        )
        .unwrap();
    let mut policy = PolicyLoop::from_committed(committed);
    let ticket = policy.issue_next().unwrap();
    let (key, _) = ticket.into_parts();
    let executed = ExecutedEffect::from_parts(
        key,
        EffectCompletion::SampleResources {
            sequence: 0,
            result: Ok(ResourceSample {
                sequence: 0,
                capacity: ResourceCapacity {
                    available_memory_bytes: 10,
                    available_storage_bytes: 10,
                },
                contention: ResourceContention {
                    cpu: CpuContentionBasisPoints::new(0).unwrap(),
                    memory_pressure: MemoryPressure::Nominal,
                },
            }),
        },
    );
    policy.apply(executed, &mut store).unwrap();
    assert_eq!(
        store
            .load_attempt(crate::AttemptId::new(9))
            .unwrap()
            .unwrap()
            .phase(),
        Phase::OwnershipDiscovery
    );
    (store, policy, binding)
}
fn root_slot() -> NativeGuardianSlot {
    NativeGuardianSlot {
        process: identity(42),
        liveness: NativeSlotLiveness::Running,
    }
}

#[allow(clippy::too_many_lines)]
#[test]
fn complete_empty_and_nonempty_result_ack_and_replay() {
    for pids in [
        vec![],
        vec![
            ProcessId::new(NonZeroU32::new(31).unwrap()),
            ProcessId::new(NonZeroU32::new(32).unwrap()),
        ],
    ] {
        let (mut store, mut policy, binding) = setup(&[root_slot()]);
        let ticket = policy.issue_next().unwrap();
        let issue = store
            .commit_native_child_list_issue_at(&ticket, &binding, MonotonicTick::new(99))
            .unwrap();
        let dispatch = NativeChildListDispatch::new(ticket, issue);
        let progress = policy
            .apply_native_child_list(
                VerifiedNativeChildListCompletion::new(
                    dispatch,
                    NativeChildListCompletion::Children(pids.clone()),
                ),
                &mut store,
            )
            .unwrap();
        assert_eq!(
            progress.acknowledge_keys().collect::<Vec<_>>(),
            vec![issue.key()]
        );
        let status = store
            .load_native_child_list_status(issue.key())
            .unwrap()
            .unwrap();
        let NativeChildListStatus::AppliedPendingAck {
            children,
            applying_revision,
            ..
        } = status
        else {
            panic!("missing child result")
        };
        assert_eq!(children, pids);
        assert_eq!(
            store
                .pending_native_child_list_ack(issue.key().attempt_id())
                .unwrap()
                .unwrap()
                .1,
            NativeChildListStatus::AppliedPendingAck {
                issue,
                children: children.clone(),
                applying_revision
            }
        );
        let checkpoint = store
            .load_attempt(issue.key().attempt_id())
            .unwrap()
            .unwrap();
        assert_eq!(
            checkpoint.checkpoint.persistence_revision(),
            Some(applying_revision)
        );
        assert_eq!(
            checkpoint
                .checkpoint
                .ownership_tree()
                .unwrap()
                .pending_child_scans()
                .len(),
            0
        );
        assert_eq!(
            checkpoint
                .checkpoint
                .ownership_tree()
                .unwrap()
                .unresolved()
                .len(),
            children.len()
        );
        let ack = VerifiedNativeChildListAckConfirmation::new(
            issue,
            children.clone(),
            applying_revision,
            issue.session_generation(),
        );
        assert_eq!(
            store.confirm_native_child_list_ack(ack).unwrap(),
            NativeChildListStatus::ChildrenAckConfirmed {
                issue,
                children: children.clone(),
                applying_revision,
                confirming_session_generation: issue.session_generation()
            }
        );
        assert!(store
            .pending_native_child_list_ack(issue.key().attempt_id())
            .unwrap()
            .is_none());
        assert!(store
            .confirm_native_child_list_ack(VerifiedNativeChildListAckConfirmation::new(
                issue,
                children,
                applying_revision,
                issue.session_generation()
            ))
            .is_err());
        assert!(matches!(
            store.load_native_child_list_status(issue.key()),
            Err(ControllerStoreError::Poisoned)
        ));
    }
}

#[test]
fn failures_park_and_never_offer_ack() {
    for failure in [
        OsQueryError::PermissionDenied,
        OsQueryError::TemporarilyUnavailable,
        OsQueryError::Malformed,
    ] {
        let (mut store, mut policy, binding) = setup(&[root_slot()]);
        let ticket = policy.issue_next().unwrap();
        let issue = store
            .commit_native_child_list_issue_at(&ticket, &binding, MonotonicTick::new(1))
            .unwrap();
        let progress = policy
            .apply_native_child_list(
                VerifiedNativeChildListCompletion::new(
                    NativeChildListDispatch::new(ticket, issue),
                    NativeChildListCompletion::Failure(failure),
                ),
                &mut store,
            )
            .unwrap();
        assert!(matches!(&progress, LoopProgress::Parked { .. }));
        assert!(progress.acknowledge_keys().next().is_none());
        assert!(
            matches!(store.load_native_child_list_status(issue.key()).unwrap(),Some(NativeChildListStatus::ParkedRetained {failure:f,..}) if f==failure)
        );
        assert!(store
            .pending_native_child_list_ack(issue.key().attempt_id())
            .unwrap()
            .is_none());
    }
}

#[test]
fn rejection_poison_and_deadline_and_root_slot() {
    for bad in ["deadline", "wrong_parent", "missing_slot", "wrong_birth"] {
        let slots: Vec<_> = match bad {
            "missing_slot" => vec![],
            "wrong_birth" => vec![NativeGuardianSlot {
                process: identity(43),
                liveness: NativeSlotLiveness::Running,
            }],
            _ => vec![root_slot()],
        };
        let (mut store, mut policy, binding) = setup(&slots);
        let ticket = policy.issue_next().unwrap();
        let forged = if bad == "wrong_parent" {
            DispatchTicket::from_committed_parts(
                ticket.key(),
                OsEffect::ListChildren {
                    parent: identity(78),
                },
            )
        } else {
            DispatchTicket::from_committed_parts(ticket.key(), ticket.effect().clone())
        };
        let now = if bad == "deadline" {
            MonotonicTick::new(100)
        } else {
            MonotonicTick::new(1)
        };
        assert!(
            store
                .commit_native_child_list_issue_at(&forged, &binding, now)
                .is_err(),
            "{bad}"
        );
        assert!(matches!(
            store.load_native_child_list_status(ticket.key()),
            Err(ControllerStoreError::Poisoned)
        ));
    }
}

#[test]
fn duplicate_children_roll_back_checkpoint_and_poison() {
    let (mut store, mut policy, binding) = setup(&[root_slot()]);
    let ticket = policy.issue_next().unwrap();
    let issue = store
        .commit_native_child_list_issue_at(&ticket, &binding, MonotonicTick::new(1))
        .unwrap();
    let before = store
        .load_attempt(issue.key().attempt_id())
        .unwrap()
        .unwrap()
        .checkpoint;
    let same = ProcessId::new(NonZeroU32::new(31).unwrap());
    assert!(policy
        .apply_native_child_list(
            VerifiedNativeChildListCompletion::new(
                NativeChildListDispatch::new(ticket, issue),
                NativeChildListCompletion::Children(vec![same, same])
            ),
            &mut store
        )
        .is_err());
    assert!(matches!(
        store.load_attempt(issue.key().attempt_id()),
        Err(ControllerStoreError::Poisoned)
    ));
    assert_eq!(
        read_attempt(&store.connection, issue.key().attempt_id()).unwrap(),
        Some(before)
    );
    assert_eq!(
        read_status(&store.connection, issue.key()).unwrap(),
        Some(NativeChildListStatus::IssuedPendingApplication { issue })
    );
}

#[test]
fn issued_child_list_evidence_cannot_be_deleted() {
    let (mut store, mut policy, binding) = setup(&[root_slot()]);
    let ticket = policy.issue_next().unwrap();
    let issue = store
        .commit_native_child_list_issue_at(&ticket, &binding, MonotonicTick::new(1))
        .unwrap();
    let (id, revision, ordinal) = parts(issue);

    store
        .connection
        .execute(
            "DELETE FROM native_child_list_issues WHERE attempt_id=?1 AND dispatch_revision=?2 AND ordinal=?3",
            params![id, revision, ordinal],
        )
        .unwrap_err();
    assert_eq!(
        read_status(&store.connection, issue.key()).unwrap(),
        Some(NativeChildListStatus::IssuedPendingApplication { issue })
    );
}

#[test]
fn malformed_child_ordinals_are_rejected() {
    let (mut store, mut policy, binding) = setup(&[root_slot()]);
    let ticket = policy.issue_next().unwrap();
    let issue = store
        .commit_native_child_list_issue_at(&ticket, &binding, MonotonicTick::new(1))
        .unwrap();
    let children = vec![ProcessId::new(NonZeroU32::new(31).unwrap())];
    policy
        .apply_native_child_list(
            VerifiedNativeChildListCompletion::new(
                NativeChildListDispatch::new(ticket, issue),
                NativeChildListCompletion::Children(children),
            ),
            &mut store,
        )
        .unwrap();
    let (id, revision, ordinal) = parts(issue);
    store.connection.execute("UPDATE native_child_list_result_children SET child_ordinal=3 WHERE attempt_id=?1 AND dispatch_revision=?2 AND issue_ordinal=?3",params![id,revision,ordinal]).unwrap_err();
    // Malformed rows inserted by a damaged or foreign-key-disabled writer must still fail closed.
    store
        .connection
        .execute_batch("DROP TRIGGER native_child_list_child_immutable")
        .unwrap();
    store.connection.execute("UPDATE native_child_list_result_children SET child_ordinal=3 WHERE attempt_id=?1 AND dispatch_revision=?2 AND issue_ordinal=?3",params![id,revision,ordinal]).unwrap();
    assert!(matches!(
        store.load_native_child_list_status(issue.key()),
        Err(ControllerStoreError::Malformed("noncontiguous child list"))
    ));
}
