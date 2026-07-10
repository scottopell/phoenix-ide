//! Deterministic operation-sequence tests for the durable creation protocol.

#![allow(clippy::too_many_lines, clippy::wildcard_enum_match_arm)]

use crate::creation_protocol::{
    decide_creation, CreationProtocolEffect, CreationProtocolError, CreationProtocolEvent,
    MAX_CREATION_ATTEMPTS,
};
use phoenix_core::domain::creation_protocol::{
    CreationClaim, CreationClaimToken, CreationError, CreationKind, CreationProtocolState,
    CreationStage, CreationStatus, CreationTime, CreationWorkerId,
};
use proptest::prelude::*;

const LEASE_DURATION: CreationTime = 100;

#[derive(Clone, Debug)]
enum SimOp {
    Claim { worker: u8 },
    AdvanceTime(u8),
    RenewCurrent,
    SucceedCurrent,
    RetryableFailure,
    PermanentFailure,
    ReplayStaleSuccess,
    ReplayStaleFailure,
    Cancel,
    Delete,
    ReconcileSuccess,
    ReconcileFailure,
}

fn arb_op() -> impl Strategy<Value = SimOp> {
    prop_oneof![
        4 => (0_u8..3).prop_map(|worker| SimOp::Claim { worker }),
        3 => (0_u8..=150).prop_map(SimOp::AdvanceTime),
        2 => Just(SimOp::RenewCurrent),
        5 => Just(SimOp::SucceedCurrent),
        3 => Just(SimOp::RetryableFailure),
        1 => Just(SimOp::PermanentFailure),
        2 => Just(SimOp::ReplayStaleSuccess),
        2 => Just(SimOp::ReplayStaleFailure),
        1 => Just(SimOp::Cancel),
        1 => Just(SimOp::Delete),
        2 => Just(SimOp::ReconcileSuccess),
        1 => Just(SimOp::ReconcileFailure),
    ]
}

#[derive(Debug)]
struct Simulation {
    now: CreationTime,
    state: CreationProtocolState,
    claims: Vec<CreationClaim>,
    provisioning_terminal_seen: Option<CreationStatus>,
    trace: Vec<String>,
}

impl Simulation {
    fn new() -> Self {
        Self {
            now: 0,
            state: CreationProtocolState::accepted(CreationKind::InitialTurn {
                message_id: "message-0".to_string(),
            }),
            claims: Vec::new(),
            provisioning_terminal_seen: None,
            trace: Vec::new(),
        }
    }

    fn current_claim(&self) -> Option<CreationClaim> {
        match &self.state.status {
            CreationStatus::Claimed(claim) => Some(claim.clone()),
            _ => None,
        }
    }

    fn stale_claim(&self) -> Option<CreationClaim> {
        let current_generation = self.current_claim().map(|claim| claim.generation);
        self.claims
            .iter()
            .rev()
            .find(|claim| Some(claim.generation) != current_generation)
            .cloned()
    }

    fn error(label: &str) -> CreationError {
        CreationError {
            kind: label.to_string(),
            message: format!("simulated {label}"),
        }
    }

    fn apply(&mut self, op: &SimOp) -> Result<(), TestCaseError> {
        self.trace
            .push(format!("t={} op={op:?} before={:?}", self.now, self.state));
        match op {
            SimOp::AdvanceTime(delta) => {
                self.now = self.now.saturating_add(u64::from(*delta));
                return self.check_invariants(op);
            }
            SimOp::ReplayStaleSuccess => {
                if let Some(claim) = self.stale_claim() {
                    self.assert_rejected_without_mutation(CreationProtocolEvent::StageSucceeded {
                        claim,
                        now: self.now,
                    })?;
                }
                return self.check_invariants(op);
            }
            SimOp::ReplayStaleFailure => {
                if let Some(claim) = self.stale_claim() {
                    self.assert_rejected_without_mutation(
                        CreationProtocolEvent::StageFailedPermanent {
                            claim,
                            now: self.now,
                            error: Self::error("stale"),
                        },
                    )?;
                }
                return self.check_invariants(op);
            }
            _ => {}
        }

        let event = match op {
            SimOp::Claim { worker } => CreationProtocolEvent::ClaimRequested {
                worker_id: CreationWorkerId(format!("worker-{worker}")),
                token: CreationClaimToken(format!(
                    "token-{worker}-{}-{}",
                    self.state.generation + 1,
                    self.now
                )),
                now: self.now,
                lease_duration: LEASE_DURATION,
            },
            SimOp::RenewCurrent => {
                let Some(claim) = self.current_claim() else {
                    return self.check_invariants(op);
                };
                CreationProtocolEvent::LeaseRenewed {
                    claim,
                    now: self.now,
                    lease_duration: LEASE_DURATION,
                }
            }
            SimOp::SucceedCurrent => {
                let Some(claim) = self.current_claim() else {
                    return self.check_invariants(op);
                };
                CreationProtocolEvent::StageSucceeded {
                    claim,
                    now: self.now,
                }
            }
            SimOp::RetryableFailure => {
                let Some(claim) = self.current_claim() else {
                    return self.check_invariants(op);
                };
                CreationProtocolEvent::StageFailedRetryable {
                    claim,
                    now: self.now,
                    error: Self::error("transient"),
                }
            }
            SimOp::PermanentFailure => {
                let Some(claim) = self.current_claim() else {
                    return self.check_invariants(op);
                };
                CreationProtocolEvent::StageFailedPermanent {
                    claim,
                    now: self.now,
                    error: Self::error("permanent"),
                }
            }
            SimOp::Cancel => CreationProtocolEvent::CancelRequested,
            SimOp::Delete => CreationProtocolEvent::DeleteRequested,
            SimOp::ReconcileSuccess => CreationProtocolEvent::ReconciliationSucceeded,
            SimOp::ReconcileFailure => CreationProtocolEvent::ReconciliationFailedRetryable {
                error: Self::error("cleanup"),
            },
            SimOp::AdvanceTime(_) | SimOp::ReplayStaleSuccess | SimOp::ReplayStaleFailure => {
                unreachable!("handled above")
            }
        };

        match decide_creation(&self.state, event) {
            Ok(decision) => {
                if let CreationStatus::Claimed(claim) = &decision.state.status {
                    if !self
                        .claims
                        .iter()
                        .any(|known| known.generation == claim.generation)
                    {
                        self.claims.push(claim.clone());
                    }
                }
                self.state = decision.state;
            }
            Err(
                CreationProtocolError::InvalidStatus(_)
                | CreationProtocolError::ClaimLost
                | CreationProtocolError::LeaseExpired,
            ) => {}
            Err(error) => {
                prop_assert!(
                    false,
                    "unexpected protocol error {error:?}; trace={:#?}",
                    self.trace
                );
            }
        }
        self.check_invariants(op)
    }

    fn assert_rejected_without_mutation(
        &self,
        event: CreationProtocolEvent,
    ) -> Result<(), TestCaseError> {
        let result = decide_creation(&self.state, event);
        prop_assert!(
            matches!(
                result,
                Err(CreationProtocolError::ClaimLost | CreationProtocolError::LeaseExpired)
            ),
            "stale result was not fenced: {result:?}; trace={:#?}",
            self.trace
        );
        Ok(())
    }

    fn check_invariants(&mut self, op: &SimOp) -> Result<(), TestCaseError> {
        prop_assert!(
            self.state.attempt <= MAX_CREATION_ATTEMPTS,
            "attempt budget exceeded after {op:?}; trace={:#?}",
            self.trace
        );
        prop_assert!(
            self.state.generation >= u64::from(self.state.attempt),
            "attempt cannot exceed generation after {op:?}; trace={:#?}",
            self.trace
        );
        if let CreationStatus::Claimed(claim) = &self.state.status {
            prop_assert_eq!(
                claim.generation,
                self.state.generation,
                "claim generation diverged after {:?}; trace={:#?}",
                op,
                self.trace
            );
            prop_assert!(
                claim.lease_until > 0,
                "claimed job has no lease after {op:?}; trace={:#?}",
                self.trace
            );
        }
        if let CreationStatus::RetryScheduled {
            next_attempt_at, ..
        } = self.state.status
        {
            prop_assert!(
                next_attempt_at >= self.now,
                "retry was scheduled in the past after {op:?}; trace={:#?}",
                self.trace
            );
        }
        if matches!(
            self.state.status,
            CreationStatus::Ready | CreationStatus::Failed(_)
        ) {
            if self.provisioning_terminal_seen.is_none() {
                self.provisioning_terminal_seen = Some(self.state.status.clone());
            }
        } else if self.provisioning_terminal_seen.is_some() {
            prop_assert!(
                matches!(self.state.status, CreationStatus::DeletionPending),
                "ready/failed creation resumed provisioning after {op:?}; trace={:#?}",
                self.trace
            );
        }

        Ok(())
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn prop_creation_protocol_invariants_hold_after_every_operation(
        operations in proptest::collection::vec(arb_op(), 1..=50),
    ) {
        let mut simulation = Simulation::new();
        for operation in &operations {
            simulation.apply(operation)?;
        }
    }
}

#[test]
fn expired_worker_is_fenced_after_replacement_completes() {
    let mut simulation = Simulation::new();
    simulation.apply(&SimOp::Claim { worker: 0 }).unwrap();
    let stale = simulation.current_claim().unwrap();
    simulation
        .apply(&SimOp::AdvanceTime(
            u8::try_from(LEASE_DURATION).expect("test lease fits in u8"),
        ))
        .unwrap();
    simulation.apply(&SimOp::Claim { worker: 1 }).unwrap();

    while simulation.state.stage != CreationStage::Finalize {
        simulation.apply(&SimOp::SucceedCurrent).unwrap();
    }
    simulation.apply(&SimOp::SucceedCurrent).unwrap();
    assert_eq!(simulation.state.status, CreationStatus::Ready);

    let before = simulation.state.clone();
    let result = decide_creation(
        &simulation.state,
        CreationProtocolEvent::StageFailedPermanent {
            claim: stale,
            now: simulation.now,
            error: Simulation::error("late worker"),
        },
    );
    assert_eq!(result, Err(CreationProtocolError::ClaimLost));
    assert_eq!(simulation.state, before);
}

#[test]
fn cancellation_revokes_claim_and_preserves_cancelled_record() {
    let mut simulation = Simulation::new();
    simulation.apply(&SimOp::Claim { worker: 0 }).unwrap();
    let stale = simulation.current_claim().unwrap();
    simulation.apply(&SimOp::Cancel).unwrap();
    assert_eq!(simulation.state.status, CreationStatus::Cancelling);

    let late = decide_creation(
        &simulation.state,
        CreationProtocolEvent::StageSucceeded {
            claim: stale,
            now: simulation.now,
        },
    );
    assert_eq!(late, Err(CreationProtocolError::ClaimLost));

    simulation.apply(&SimOp::ReconcileSuccess).unwrap();
    assert_eq!(simulation.state.status, CreationStatus::Cancelled);
    assert!(!simulation.state.status.is_hidden());
}

#[test]
fn delete_hides_immediately_and_requires_tombstone_cleanup() {
    let state = CreationProtocolState::accepted(CreationKind::SeededEmpty);
    let decision = decide_creation(&state, CreationProtocolEvent::DeleteRequested).unwrap();
    assert_eq!(decision.state.status, CreationStatus::DeletionPending);
    assert!(decision.state.status.is_hidden());
    assert_eq!(
        decision.effects,
        vec![CreationProtocolEffect::ReconcileForDeletion]
    );

    let reconciled = decide_creation(
        &decision.state,
        CreationProtocolEvent::ReconciliationSucceeded,
    )
    .unwrap();
    assert_eq!(reconciled.state.status, CreationStatus::DeletionPending);
    assert_eq!(
        reconciled.effects,
        vec![CreationProtocolEffect::DeleteTombstone]
    );
}
