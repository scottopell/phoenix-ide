#![cfg(unix)]
use phoenix_deployment_controller::{
    guardian_ipc::{
        WireIdentity, WireNativeBinding, WireNativeSlot, WireObservation, WireSession, WireSlot,
        WireSnapshot, WireState,
    },
    native_guardian::{verify_native_binding, VerifiedBindingStatus},
    AttemptId, BirthToken, ControllerSqliteStore, CpuContentionBasisPoints, DeploymentAttempt,
    MemoryPressure, MonotonicTick, NativeSessionState, ProcessId, ProcessIdentity, ResourcePolicy,
};
use std::num::{NonZeroU32, NonZeroUsize};

fn fixture() -> (ControllerSqliteStore, WireSnapshot) {
    let mut store = ControllerSqliteStore::in_memory().unwrap();
    let id = AttemptId::new(71);
    let root = ProcessIdentity::new(
        ProcessId::new(NonZeroU32::new(8).unwrap()),
        BirthToken::new(10, 0).unwrap(),
    );
    let policy = ResourcePolicy::new(
        NonZeroUsize::new(2).unwrap(),
        0,
        0,
        CpuContentionBasisPoints::new(0).unwrap(),
        MemoryPressure::Critical,
    );
    DeploymentAttempt::start(id, MonotonicTick::new(500), policy, root)
        .persist(&mut store)
        .unwrap();
    let snapshot = WireSnapshot {
        instance: 9,
        attempt: id.get(),
        deadline: 500,
        state: WireState::Active,
        session: WireSession {
            instance: 9,
            generation: 1,
        },
        slots: vec![WireSlot {
            instance: 9,
            ordinal: 1,
        }],
        retained: vec![],
        native_binding: Some(WireNativeBinding {
            attempt: id.get(),
            deadline: 500,
            nonce: [7; 16],
            guardian: WireIdentity {
                pid: 100,
                birth_seconds: 11,
                birth_nanos: 200,
            },
            slots: vec![WireNativeSlot {
                identity: WireIdentity {
                    pid: 101,
                    birth_seconds: 12,
                    birth_nanos: 300,
                },
                liveness: WireObservation::Running,
            }],
        }),
    };
    (store, snapshot)
}

#[test]
fn initial_and_detached_reconnect_are_exact_and_idempotent() {
    let (mut store, mut wire) = fixture();
    let id = AttemptId::new(71);
    assert_eq!(
        verify_native_binding(&mut store, id, &wire).unwrap(),
        VerifiedBindingStatus::InitialActive
    );
    let bound = store.load_native_binding(id).unwrap().unwrap();
    assert_eq!(bound.identity().nonce.bytes(), [7; 16]);
    assert_eq!(bound.identity().process.pid().get(), 100);
    assert_eq!(bound.slots()[0].process.birth().nanos(), 300);
    wire.state = WireState::DetachedFenced;
    wire.native_binding.as_mut().unwrap().slots[0].liveness = WireObservation::Exited;
    assert_eq!(
        verify_native_binding(&mut store, id, &wire).unwrap(),
        VerifiedBindingStatus::Detached
    );
    let detached = store.load_native_binding(id).unwrap().unwrap();
    assert_eq!(
        detached.current_session().state,
        NativeSessionState::Detached
    );
    assert_eq!(
        verify_native_binding(&mut store, id, &wire).unwrap(),
        VerifiedBindingStatus::Detached
    );
    assert_eq!(
        store.load_native_binding(id).unwrap().unwrap().revision(),
        detached.revision()
    );
}

#[test]
fn mismatched_attempt_store_nonce_guardian_deadline_and_inventory_are_refused() {
    let (mut store, wire) = fixture();
    assert!(verify_native_binding(&mut store, AttemptId::new(72), &wire).is_err());
    let mut wrong_store = ControllerSqliteStore::in_memory().unwrap();
    assert!(verify_native_binding(&mut wrong_store, AttemptId::new(71), &wire).is_err());
    let (mut deadline_store, mut wrong_deadline) = fixture();
    wrong_deadline.deadline += 1;
    wrong_deadline.native_binding.as_mut().unwrap().deadline += 1;
    assert!(
        verify_native_binding(&mut deadline_store, AttemptId::new(71), &wrong_deadline).is_err()
    );
    assert_eq!(
        verify_native_binding(&mut store, AttemptId::new(71), &wire).unwrap(),
        VerifiedBindingStatus::InitialActive
    );
    let mut detached = wire;
    detached.state = WireState::DetachedFenced;
    let good = detached.clone();
    detached.native_binding.as_mut().unwrap().nonce[0] ^= 1;
    assert!(verify_native_binding(&mut store, AttemptId::new(71), &detached).is_err());
    detached = good.clone();
    detached
        .native_binding
        .as_mut()
        .unwrap()
        .guardian
        .birth_nanos += 1;
    assert!(verify_native_binding(&mut store, AttemptId::new(71), &detached).is_err());
    detached = good.clone();
    detached.native_binding.as_mut().unwrap().slots[0]
        .identity
        .pid += 1;
    assert!(verify_native_binding(&mut store, AttemptId::new(71), &detached).is_err());
    detached = good.clone();
    detached.native_binding.as_mut().unwrap().slots.clear();
    assert!(verify_native_binding(&mut store, AttemptId::new(71), &detached).is_err());
    detached = good.clone();
    detached.native_binding = None;
    assert!(verify_native_binding(&mut store, AttemptId::new(71), &detached).is_err());
    assert_eq!(
        verify_native_binding(&mut store, AttemptId::new(71), &good).unwrap(),
        VerifiedBindingStatus::Detached
    );
}

#[test]
fn generic_transport_does_not_invent_native_binding() {
    let (mut store, mut wire) = fixture();
    wire.native_binding = None;
    assert!(verify_native_binding(&mut store, AttemptId::new(71), &wire).is_err());
}
