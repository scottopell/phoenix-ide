//! Public guardian boundary regression alongside the private IPC adapter tests.
use phoenix_deployment_controller::{
    AttemptId, ChildSlotId, ChildSlotObservation, DispatchTicket, ExecutedEffect, Guardian,
    GuardianError, GuardianExecutor, MonotonicTick, PersistenceRevision,
};
use std::collections::BTreeMap;
struct Observer;
impl GuardianExecutor for Observer {
    type ChildSlot = ();
    fn execute(&mut self, _: DispatchTicket, _: &mut BTreeMap<ChildSlotId, ()>) -> ExecutedEffect {
        panic!("observation-only fixture never executes")
    }
    fn poll_observation_only(&self, _: &()) -> ChildSlotObservation {
        ChildSlotObservation::Running
    }
}
#[test]
fn reconnect_fences_the_old_session_and_preserves_inventory() {
    let (mut guardian, old) = Guardian::new(
        AttemptId::new(8),
        MonotonicTick::new(100),
        Observer,
        vec![()],
    )
    .unwrap();
    let slots: Vec<_> = guardian.inventory().collect();
    guardian.controller_lost(old).unwrap();
    let fresh = guardian
        .begin_reconciliation(guardian.instance_id(), &slots, PersistenceRevision::first())
        .unwrap();
    assert_eq!(fresh.generation(), old.generation() + 1);
    assert_eq!(
        guardian.controller_lost(old),
        Err(GuardianError::StaleSession)
    );
    assert_eq!(guardian.inventory().collect::<Vec<_>>(), slots);
    assert_eq!(
        guardian.poll_observation_only(slots[0]),
        Ok(ChildSlotObservation::Running)
    );
}
