use super::*;
use crate::{
    CpuContentionBasisPoints, EffectCompletion, ExecutedEffect, GuardianLifetimeNonce,
    LoopProgress, MemoryPressure, NativeChildListCompletion, NativeChildListDispatch,
    NativeGuardianSlot, NativeSlotLiveness, PolicyLoop, ResourceCapacity, ResourceContention,
    ResourcePolicy, ResourceSample,
};
use std::num::NonZeroUsize;

fn identity(pid: u32) -> ProcessIdentity {
    ProcessIdentity::new(
        ProcessId::new(NonZeroU32::new(pid).unwrap()),
        BirthToken::new(12, 3).unwrap(),
    )
}

fn setup_cleaning(
    deadline: MonotonicTick,
) -> (ControllerSqliteStore, PolicyLoop, NativeGuardianBinding) {
    let mut store = ControllerSqliteStore::in_memory().unwrap();
    let root = identity(42);
    let committed = DeploymentAttempt::start(
        crate::AttemptId::new(19),
        deadline,
        ResourcePolicy::new(
            NonZeroUsize::new(1).unwrap(),
            0,
            0,
            CpuContentionBasisPoints::new(10_000).unwrap(),
            MemoryPressure::Critical,
        ),
        root,
    )
    .persist(&mut store)
    .unwrap();
    let binding = store
        .commit_initial_binding(
            crate::AttemptId::new(19),
            deadline,
            PersistenceRevision::first(),
            NativeGuardianIdentity {
                nonce: GuardianLifetimeNonce::new([9; 16]).unwrap(),
                process: identity(100),
            },
            &[NativeGuardianSlot {
                process: root,
                liveness: NativeSlotLiveness::Running,
            }],
        )
        .unwrap();
    let mut policy = PolicyLoop::from_committed(committed);
    let ticket = policy.issue_next().unwrap();
    let (key, _) = ticket.into_parts();
    policy
        .apply(
            ExecutedEffect::from_parts(
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
            ),
            &mut store,
        )
        .unwrap();
    let child_ticket = policy.issue_next().unwrap();
    let child_issue = store
        .commit_native_child_list_issue_at(
            &child_ticket,
            &binding,
            MonotonicTick::new(deadline.get() - 1),
        )
        .unwrap();
    let child_verified = crate::VerifiedNativeChildListCompletion::new(
        NativeChildListDispatch::new(child_ticket, child_issue),
        NativeChildListCompletion::Children(Vec::new()),
    );
    let mut child_ticks = [
        MonotonicTick::new(deadline.get() - 1),
        MonotonicTick::new(deadline.get() - 1),
    ]
    .into_iter();
    assert!(matches!(
        policy
            .apply_native_child_list_at(child_verified, &mut store, &mut || {
                Ok(child_ticks.next().unwrap())
            })
            .unwrap(),
        LoopProgress::Applied { .. }
    ));
    assert!(matches!(
        policy.finish_ownership_scan(&mut store).unwrap(),
        LoopProgress::Advanced { .. }
    ));
    assert_eq!(
        store
            .load_attempt(crate::AttemptId::new(19))
            .unwrap()
            .unwrap()
            .phase(),
        Phase::CleaningOwnedTree
    );
    (store, policy, binding)
}

#[test]
fn exact_root_observation_persists_before_ack_and_round_trips() {
    let deadline = MonotonicTick::new(500);
    let (mut store, mut policy, binding) = setup_cleaning(deadline);
    let ticket = policy.issue_next().unwrap();
    let issue = store
        .commit_native_identity_observation_issue_at(&ticket, &binding, MonotonicTick::new(499))
        .unwrap();
    assert_eq!(
        store
            .load_native_identity_observation_status(issue.key())
            .unwrap(),
        Some(NativeIdentityObservationStatus::IssuedPendingApplication { issue })
    );
    let verified = VerifiedNativeIdentityObservationCompletion::new(
        NativeIdentityObservationDispatch::new(ticket, issue),
        IdentityObservation::ExactIdentityAbsent,
    );
    let mut ticks = [MonotonicTick::new(499), MonotonicTick::new(499)].into_iter();
    let progress = policy
        .apply_native_identity_observation_at(verified, &mut store, &mut || {
            Ok(ticks.next().unwrap())
        })
        .unwrap();
    let LoopProgress::Applied {
        revision: applying_revision,
        ..
    } = progress
    else {
        panic!("identity application")
    };
    let pending = NativeIdentityObservationStatus::AppliedPendingAck {
        issue,
        observation: IdentityObservation::ExactIdentityAbsent,
        applying_revision,
    };
    assert_eq!(
        store
            .load_native_identity_observation_status(issue.key())
            .unwrap(),
        Some(pending.clone())
    );
    assert_eq!(
        store
            .pending_native_identity_observation_ack(issue.key().attempt_id())
            .unwrap(),
        Some((issue.key(), pending))
    );
    let confirmed = store
        .confirm_native_identity_observation_ack(
            VerifiedNativeIdentityObservationAckConfirmation::new(
                issue,
                IdentityObservation::ExactIdentityAbsent,
                applying_revision,
                binding.current_session().generation,
            ),
        )
        .unwrap();
    assert!(matches!(
        confirmed,
        NativeIdentityObservationStatus::AckConfirmed {
            observation: IdentityObservation::ExactIdentityAbsent,
            ..
        }
    ));
}

#[test]
fn identity_application_expiry_rolls_back_checkpoint_and_evidence() {
    let deadline = MonotonicTick::new(500);
    let (mut store, mut policy, binding) = setup_cleaning(deadline);
    let ticket = policy.issue_next().unwrap();
    let issue = store
        .commit_native_identity_observation_issue_at(&ticket, &binding, MonotonicTick::new(499))
        .unwrap();
    let before = store
        .load_attempt(issue.key().attempt_id())
        .unwrap()
        .unwrap();
    let verified = VerifiedNativeIdentityObservationCompletion::new(
        NativeIdentityObservationDispatch::new(ticket, issue),
        IdentityObservation::ExactIdentityAbsent,
    );
    let mut ticks = [MonotonicTick::new(499), deadline].into_iter();
    assert!(matches!(
        policy.apply_native_identity_observation_at(verified, &mut store, &mut || {
            Ok(ticks.next().unwrap())
        }),
        Err(crate::LoopError::Persistence(
            ControllerStoreError::RevisionConflict
        ))
    ));
    let durable = read_attempt(&store.connection, issue.key().attempt_id())
        .unwrap()
        .unwrap();
    assert_eq!(
        durable.persistence_revision(),
        Some(before.persistence_revision())
    );
    assert_eq!(
        read_status(&store.connection, issue.key()).unwrap(),
        Some(NativeIdentityObservationStatus::IssuedPendingApplication { issue })
    );
}

#[test]
fn issue_rejects_non_root_and_expired_authority() {
    let deadline = MonotonicTick::new(500);
    let (mut store, mut policy, binding) = setup_cleaning(deadline);
    let ticket = policy.issue_next().unwrap();
    assert!(matches!(
        store.commit_native_identity_observation_issue_at(&ticket, &binding, deadline),
        Err(ControllerStoreError::RevisionConflict)
    ));
}
