use super::*;
use proptest::prelude::*;

fn subject() -> WakeSubject {
    WakeSubject {
        profile_kind: "test".to_string(),
        profile_version: 1,
        resource_key: b"resource".to_vec(),
    }
}

fn registered(deadline: u64) -> WakeState {
    transition(
        &WakeState::Absent,
        WakeCommand::Register {
            id: WakeContractId::new("contract").unwrap(),
            owner: WakeOwner("owner".to_string()),
            subject: subject(),
            condition: WakeCondition::Terminal,
            registered_at: Timestamp(0),
            deadline: Timestamp(deadline),
        },
    )
    .new_state
}

fn evidence(occurred_at: u64, payload: u8) -> TerminalEvidence {
    TerminalEvidence {
        occurred_at: Timestamp(occurred_at),
        profile_kind: "test".to_string(),
        profile_version: 1,
        payload: vec![payload],
    }
}

proptest! {
    #[test]
    fn occurrence_wins_cancel_observe_interleavings(
        cancel_at in 1u64..1_000,
        observed_at in 0u64..1_000,
    ) {
        let state = registered(2_000);
        let proposed = transition(
            &state,
            WakeCommand::Cancel {
                expected_generation: Generation(0),
                cause: CancellationCause::UserRequested,
                occurred_at: Timestamp(cancel_at),
            },
        );
        let after_observation = transition(
            &proposed.new_state,
            WakeCommand::ObserveTerminal {
                expected_generation: Generation(0),
                evidence: evidence(observed_at, 1),
            },
        );
        if observed_at < cancel_at {
            let fired = matches!(
                after_observation.new_state,
                WakeState::Present(WakeContract {
                    lifecycle: WakeLifecycle::Closed(CanonicalTerminal::Fired { .. }),
                    ..
                })
            );
            prop_assert!(fired, "earlier evidence must win");
        } else {
            prop_assert!(matches!(
                after_observation.disposition,
                WakeDisposition::Rejected(WakeConflict::EvidenceDidNotPrecedeProposal)
            ));
            let finalized = transition(
                &after_observation.new_state,
                WakeCommand::FinalizeProposedTerminal {
                    expected_generation: Generation(0),
                },
            );
            let cancelled = matches!(
                finalized.new_state,
                WakeState::Present(WakeContract {
                    lifecycle: WakeLifecycle::Closed(CanonicalTerminal::Cancelled { .. }),
                    ..
                })
            );
            prop_assert!(cancelled, "cancellation must finalize after drain");
        }
    }

    #[test]
    fn closed_states_are_immutable(
        terminal_at in 0u64..1_000,
        later_at in 1_000u64..2_000,
    ) {
        let state = registered(2_000);
        let closed = transition(
            &state,
            WakeCommand::ObserveTerminal {
                expected_generation: Generation(0),
                evidence: evidence(terminal_at, 7),
            },
        );
        let replay = transition(
            &closed.new_state,
            WakeCommand::Cancel {
                expected_generation: Generation(0),
                cause: CancellationCause::UserRequested,
                occurred_at: Timestamp(later_at),
            },
        );
        prop_assert_eq!(replay.new_state, closed.new_state);
        prop_assert!(matches!(replay.disposition, WakeDisposition::Rejected(WakeConflict::AlreadyClosed)));
        prop_assert!(replay.owed_effects.is_empty());
    }

    #[test]
    fn stale_generation_never_changes_state(
        stale in 1u64..u64::MAX,
        at in any::<u64>(),
    ) {
        let state = registered(u64::MAX);
        let result = transition(
            &state,
            WakeCommand::Cancel {
                expected_generation: Generation(stale),
                cause: CancellationCause::UserRequested,
                occurred_at: Timestamp(at),
            },
        );
        prop_assert_eq!(result.new_state, state);
        prop_assert!(result.owed_effects.is_empty());
        prop_assert!(matches!(result.disposition, WakeDisposition::Rejected(WakeConflict::StaleGeneration { .. })), "stale generation must be rejected");
    }

    #[test]
    fn event_boundary_adds_contract_identity_once(
        at in any::<u64>(),
        payload in prop::collection::vec(any::<u8>(), 0..64),
    ) {
        let state = registered(u64::MAX);
        let raw = TerminalEvidence {
            occurred_at: Timestamp(at),
            profile_kind: "test".to_string(),
            profile_version: 1,
            payload,
        };
        let result = transition(
            &state,
            WakeCommand::ObserveTerminal {
                expected_generation: Generation(0),
                evidence: raw.clone(),
            },
        );
        let WakeDisposition::Applied { event } = result.disposition else {
            prop_assert!(false, "observation must apply");
            return Ok(());
        };
        prop_assert_eq!(event.contract_id, WakeContractId("contract".to_string()));
        let WakeEventKind::Terminalized { terminal: CanonicalTerminal::Fired { evidence } } = event.kind else {
            prop_assert!(false, "expected fired event");
            return Ok(());
        };
        prop_assert_eq!(evidence, raw);
    }
}

#[test]
fn cancellation_is_observation_only() {
    let state = registered(10);
    let result = transition(
        &state,
        WakeCommand::Cancel {
            expected_generation: Generation(0),
            cause: CancellationCause::UserRequested,
            occurred_at: Timestamp(5),
        },
    );
    assert!(result
        .owed_effects
        .iter()
        .any(|effect| matches!(effect, WakeOwedEffect::StopObservation { .. })));
    assert!(!result
        .owed_effects
        .iter()
        .any(|effect| matches!(effect, WakeOwedEffect::BeginObservation { .. })));
}

#[test]
fn owner_transfer_keeps_contract_and_resource_identity() {
    let state = registered(10);
    let WakeState::Present(before) = &state else {
        panic!("registered state");
    };
    let result = transition(
        &state,
        WakeCommand::TransferOwner {
            expected_generation: Generation(0),
            new_owner: WakeOwner("successor".to_string()),
        },
    );
    let WakeState::Present(after) = result.new_state else {
        panic!("transferred state");
    };
    assert_eq!(after.id, before.id);
    assert_eq!(after.subject, before.subject);
    assert_eq!(after.generation, before.generation);
    assert_eq!(after.owner, WakeOwner("successor".to_string()));
}

#[test]
fn zero_duration_registration_is_valid_and_immediately_expirable() {
    let state = registered(0);
    let result = transition(
        &state,
        WakeCommand::DeadlineElapsed {
            expected_generation: Generation(0),
            observed_at: Timestamp(0),
        },
    );
    assert!(matches!(
        result.new_state,
        WakeState::Present(WakeContract {
            lifecycle: WakeLifecycle::Open(OpenWakeLifecycle::TerminalProposed(
                ProposedTerminal::Expired {
                    deadline: Timestamp(0)
                }
            )),
            ..
        })
    ));
}
