//! Public custody API: no native adapters, processes, clocks, or privileged permits.
use std::{cell::Cell, collections::BTreeMap, rc::Rc};

use phoenix_deployment_controller::{
    guardian::{
        ChildSlotId, ChildSlotObservation, Guardian, GuardianError, GuardianExecutor,
        GuardianState, MAX_CHILD_SLOTS,
    },
    AttemptId, DispatchTicket, ExecutedEffect, MonotonicTick, PersistenceRevision,
};

struct Slot {
    observation: ChildSlotObservation,
    drops: Rc<Cell<usize>>,
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.drops.set(self.drops.get() + 1);
    }
}

struct Observer {
    polls: Rc<Cell<usize>>,
    drops: Rc<Cell<usize>>,
}

impl Drop for Observer {
    fn drop(&mut self) {
        self.drops.set(self.drops.get() + 1);
    }
}

impl GuardianExecutor for Observer {
    type ChildSlot = Slot;

    fn execute(
        &mut self,
        _ticket: DispatchTicket,
        _slots: &mut BTreeMap<ChildSlotId, Slot>,
    ) -> ExecutedEffect {
        panic!("public observation fixtures never receive bound dispatch authority")
    }

    fn poll_observation_only(&self, slot: &Slot) -> ChildSlotObservation {
        self.polls.set(self.polls.get() + 1);
        slot.observation
    }
}

#[test]
fn public_reconnect_fences_stale_controllers_and_forfeit_preserves_custody() {
    let polls = Rc::new(Cell::new(0));
    let executor_drops = Rc::new(Cell::new(0));
    let slot_drops = Rc::new(Cell::new(0));
    let executor = Observer {
        polls: Rc::clone(&polls),
        drops: Rc::clone(&executor_drops),
    };
    let slots = [
        ChildSlotObservation::Exited,
        ChildSlotObservation::Unavailable,
    ]
    .into_iter()
    .map(|observation| Slot {
        observation,
        drops: Rc::clone(&slot_drops),
    })
    .collect();
    let (mut guardian, old) =
        Guardian::new(AttemptId::new(1), MonotonicTick::new(100), executor, slots).unwrap();
    let inventory: Vec<_> = guardian.inventory().collect();
    for (index, slot) in inventory.iter().enumerate() {
        assert_eq!(slot.instance_id(), old.instance_id());
        assert_eq!(slot.ordinal(), index + 1);
    }
    assert!(guardian.can_retain_result());
    guardian.controller_lost(old).unwrap();
    assert_eq!(
        guardian.poll_observation_only(inventory[0]),
        Ok(ChildSlotObservation::Exited)
    );
    assert_eq!(
        guardian.poll_observation_only(inventory[1]),
        Ok(ChildSlotObservation::Unavailable)
    );
    assert_eq!(polls.get(), 2);
    assert_eq!(guardian.state(), GuardianState::DetachedFenced);
    assert_eq!(
        guardian.begin_reconciliation(guardian.instance_id(), &[], PersistenceRevision::first()),
        Err(GuardianError::InventoryMismatch)
    );
    let current = guardian
        .begin_reconciliation(
            guardian.instance_id(),
            &inventory,
            PersistenceRevision::first(),
        )
        .unwrap();
    assert_eq!(current.generation(), old.generation() + 1);
    assert_eq!(
        guardian.controller_lost(old),
        Err(GuardianError::StaleSession)
    );
    assert_eq!(
        guardian.authority_lost(old),
        Err(GuardianError::StaleSession)
    );
    assert_eq!(guardian.state(), GuardianState::Reconciling);
    assert_eq!(
        guardian.reconciliation_floor(),
        Some(PersistenceRevision::first())
    );
    guardian.authority_lost(current).unwrap();
    assert_eq!(guardian.reconciliation_floor(), None);
    assert_eq!(
        guardian.controller_lost(current),
        Err(GuardianError::AuthorityLost)
    );
    assert_eq!(
        guardian.begin_reconciliation(
            guardian.instance_id(),
            &inventory,
            PersistenceRevision::first()
        ),
        Err(GuardianError::AuthorityLost)
    );
    assert_eq!(
        guardian.poll_observation_only(inventory[0]),
        Ok(ChildSlotObservation::Exited)
    );
    let summary = guardian.summary();
    let custody = guardian.forfeit();
    assert_eq!(custody.summary(), summary);
    assert_eq!(custody.inventory().collect::<Vec<_>>(), inventory);
    assert_eq!(custody.retained_results().len(), 0);
    assert!(custody.quarantined_result().is_none());
    assert_eq!(executor_drops.get(), 0);
    assert_eq!(slot_drops.get(), 0);
}

#[test]
fn repeated_rejected_construction_returns_the_same_owned_inputs() {
    let executor_drops = Rc::new(Cell::new(0));
    let slot_drops = Rc::new(Cell::new(0));
    let mut executor = Observer {
        polls: Rc::new(Cell::new(0)),
        drops: Rc::clone(&executor_drops),
    };
    let mut slots: Vec<_> = (0..=MAX_CHILD_SLOTS)
        .map(|_| Slot {
            observation: ChildSlotObservation::Running,
            drops: Rc::clone(&slot_drops),
        })
        .collect();
    for _ in 0..4 {
        let failure = Guardian::new(AttemptId::new(2), MonotonicTick::new(100), executor, slots)
            .err()
            .expect("oversized inventory must be returned");
        assert_eq!(failure.error(), GuardianError::ChildSlotCapacityExceeded);
        let (error, returned_executor, returned_slots) = failure.into_parts();
        assert_eq!(error, GuardianError::ChildSlotCapacityExceeded);
        assert!(Rc::ptr_eq(&returned_executor.drops, &executor_drops));
        assert_eq!(returned_slots.len(), MAX_CHILD_SLOTS + 1);
        assert!(returned_slots
            .iter()
            .all(|slot| Rc::ptr_eq(&slot.drops, &slot_drops)));
        assert_eq!(executor_drops.get(), 0);
        assert_eq!(slot_drops.get(), 0);
        executor = returned_executor;
        slots = returned_slots;
    }
    // Only the recovered owner explicitly destroys these non-native test inputs.
    drop(executor);
    drop(slots);
    assert_eq!(executor_drops.get(), 1);
    assert_eq!(slot_drops.get(), MAX_CHILD_SLOTS + 1);
}

#[test]
fn empty_inventory_is_exact_and_foreign_instances_cannot_reconnect() {
    let make = || {
        Guardian::new(
            AttemptId::new(3),
            MonotonicTick::new(0),
            Observer {
                polls: Rc::new(Cell::new(0)),
                drops: Rc::new(Cell::new(0)),
            },
            Vec::new(),
        )
        .unwrap()
    };
    let (mut guardian, session) = make();
    let (other, foreign) = make();
    assert_ne!(session.instance_id(), foreign.instance_id());
    guardian.controller_lost(session).unwrap();
    assert_eq!(
        guardian.begin_reconciliation(other.instance_id(), &[], PersistenceRevision::first()),
        Err(GuardianError::InstanceMismatch)
    );
    guardian
        .begin_reconciliation(guardian.instance_id(), &[], PersistenceRevision::first())
        .unwrap();
    assert_eq!(guardian.state(), GuardianState::Reconciling);
    assert_eq!(guardian.inventory().len(), 0);
    let custody = guardian.forfeit();
    let other_custody = other.forfeit();
    assert_eq!(custody.summary().original_deadline, MonotonicTick::new(0));
    assert_eq!(other_custody.summary().state, GuardianState::AuthorityLost);
}
