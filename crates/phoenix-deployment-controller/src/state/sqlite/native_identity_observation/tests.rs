use super::*;
use crate::{
    ActiveAttempt, CpuContentionBasisPoints, EffectCompletion, ExecutedEffect,
    GuardianLifetimeNonce, LoopProgress, MemoryPressure, NativeChildListCompletion,
    NativeChildListDispatch, NativeGuardianSlot, NativeSlotLiveness, OwnedIdentityStatus,
    PolicyLoop, ResourceCapacity, ResourceContention, ResourcePolicy, ResourceSample,
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

fn recovery_fixture() -> (
    ControllerSqliteStore,
    LoadedAttempt,
    NativeGuardianBinding,
    NativeIdentityObservationIssue,
) {
    let deadline = MonotonicTick::new(500);
    let (mut store, mut policy, binding) = setup_cleaning(deadline);
    let ticket = policy.issue_next().unwrap();
    let issue = store
        .commit_native_identity_observation_issue_at(&ticket, &binding, MonotonicTick::new(499))
        .unwrap();
    let checkpoint = store
        .load_attempt(issue.key().attempt_id())
        .unwrap()
        .unwrap()
        .checkpoint;
    let detached = store
        .commit_detached_binding(&binding, binding.slots())
        .unwrap();
    let interrupted = checkpoint.interrupt().unwrap().persist(&mut store).unwrap();
    let binding = store
        .commit_fresh_interruption_binding(
            &detached,
            interrupted.checkpoint().persistence_revision().unwrap(),
        )
        .unwrap();
    let loaded = store
        .load_attempt(issue.key().attempt_id())
        .unwrap()
        .unwrap();
    (store, loaded, binding, issue)
}

#[test]
fn recovered_identity_folds_only_the_root_without_activating_cleanup() {
    for observation in [
        IdentityObservation::ExactIdentityAbsent,
        IdentityObservation::PidReused(ProcessIdentity::new(
            identity(42).pid(),
            BirthToken::new(13, 0).unwrap(),
        )),
    ] {
        let (mut store, loaded, _, issue) = recovery_fixture();
        let original = loaded.checkpoint.clone();
        let candidate = store
            .prepare_native_identity_observation_recovery_at(loaded, MonotonicTick::new(499))
            .unwrap();
        let applied = store
            .apply_recovered_native_identity_observation_at(
                VerifiedRecoveredNativeIdentityObservation::after_live_verification(
                    candidate,
                    observation,
                ),
                MonotonicTick::new(499),
            )
            .unwrap();
        assert!(
            matches!(applied, NativeIdentityObservationStatus::AppliedPendingAck { issue: saved, observation: value, .. } if saved == issue && value == observation)
        );
        let saved = store
            .load_attempt(issue.key().attempt_id())
            .unwrap()
            .unwrap();
        assert_eq!(saved.phase(), Phase::Interrupted);
        assert_ne!(saved.checkpoint, original);
        let DeploymentAttempt::Interrupted(interrupted) = &saved.checkpoint else {
            panic!("recovered identity application must remain interrupted")
        };
        let ActiveAttempt::Ownership(resume) = &interrupted.resume else {
            panic!("identity recovery must resume owned-tree cleanup")
        };
        assert_eq!(
            resume.tree.identities().get(&issue.target()),
            Some(&OwnedIdentityStatus::VerifiedAbsent)
        );
        assert_eq!(
            store
                .pending_native_identity_observation_ack(issue.key().attempt_id())
                .unwrap()
                .unwrap()
                .0,
            issue.key()
        );
        assert!(matches!(
            store.prepare_native_identity_observation_recovery_at(saved, MonotonicTick::new(499)),
            Err(ControllerStoreError::RevisionConflict)
        ));
    }
}

#[test]
fn recovered_identity_expiry_rolls_back_checkpoint_and_application() {
    let (mut store, loaded, _, issue) = recovery_fixture();
    let original = loaded.checkpoint.clone();
    let candidate = store
        .prepare_native_identity_observation_recovery_at(loaded, MonotonicTick::new(499))
        .unwrap();
    let mut ticks = [
        MonotonicTick::new(499),
        MonotonicTick::new(499),
        MonotonicTick::new(500),
    ]
    .into_iter();
    assert!(matches!(
        store.apply_recovered_native_identity_observation_with_clock(
            VerifiedRecoveredNativeIdentityObservation::after_live_verification(
                candidate,
                IdentityObservation::ExactIdentityAbsent
            ),
            || Ok(ticks.next().unwrap_or(MonotonicTick::new(500)))
        ),
        Err(ControllerStoreError::RevisionConflict)
    ));
    assert!(ticks.next().is_none());
    assert_eq!(
        read_attempt(&store.connection, issue.key().attempt_id()).unwrap(),
        Some(original)
    );
    assert_eq!(
        read_status(&store.connection, issue.key()).unwrap(),
        Some(NativeIdentityObservationStatus::IssuedPendingApplication { issue })
    );
}
