use super::*;
use proptest::prelude::*;
use std::collections::BTreeSet;

fn codec() -> WakeCodecRef {
    WakeCodecRef {
        family: WakeCodecFamily("test".to_string()),
        version: WakeCodecVersion(1),
    }
}

fn encoded(bytes: Vec<u8>) -> EncodedWakeValue {
    EncodedWakeValue {
        codec: codec(),
        payload: WakePayload(bytes),
    }
}

fn subject() -> WakeSubject {
    WakeSubject {
        profile: WakeProfileRef {
            kind: WakeProfileKind("test".to_string()),
            version: WakeProfileVersion(1),
        },
        resource: encoded(b"resource".to_vec()),
    }
}

fn register_command(deadline: u64) -> WakeCommand {
    WakeCommand {
        transition_id: TransitionId(1),
        kind: WakeCommandKind::Register {
            id: WakeContractId::new("contract").unwrap(),
            registration_owner: WakeOwner("registrant".to_string()),
            subject: subject(),
            condition: WakeCondition::Terminal,
            registered_at: Timestamp(0),
            deadline: Timestamp(deadline),
        },
    }
}

fn registered(deadline: u64) -> WakeState {
    transition(&WakeState::Absent, register_command(deadline)).new_state
}

fn contract(state: &WakeState) -> &WakeContract {
    let WakeState::Present(contract) = state else {
        panic!("expected present contract")
    };
    contract
}

fn evidence(occurred_at: u64, payload: u8) -> TerminalEvidence {
    TerminalEvidence {
        occurred_at: Timestamp(occurred_at),
        value: encoded(vec![payload]),
    }
}

fn command(transition_id: u64, kind: WakeCommandKind) -> WakeCommand {
    WakeCommand {
        transition_id: TransitionId(transition_id),
        kind,
    }
}

fn cancel(state: &WakeState, transition_id: u64, occurred_at: u64) -> WakeTransition {
    transition(
        state,
        command(
            transition_id,
            WakeCommandKind::Cancel {
                expected_head: contract(state).head(),
                cause: CancellationCause::UserRequested,
                occurred_at: Timestamp(occurred_at),
            },
        ),
    )
}

fn proposal_proof(result: &WakeTransition) -> ObservationFenceProof {
    let [WakeOwedEffect {
        kind: WakeOwedEffectKind::FenceObservationAuthority { proof },
        ..
    }] = result.owed_effects.as_slice()
    else {
        panic!("proposal must owe one observation fence")
    };
    proof.clone()
}

fn finalize(state: &WakeState, transition_id: u64, proof: ObservationFenceProof) -> WakeTransition {
    transition(
        state,
        command(
            transition_id,
            WakeCommandKind::Reconcile {
                expected_head: contract(state).head(),
                observation: ReconcileObservation::ObservationAuthorityFenced(proof),
            },
        ),
    )
}

#[derive(Debug, Clone, Copy)]
enum GeneratedAction {
    Observe { at: u16, payload: u8 },
    Cancel { at: u16 },
    Deadline { at: u16 },
    Transfer { owner: u8 },
    Forget { at: u16 },
    Fail { at: u16 },
    FenceWithCurrentProof,
    ReplayHeadTransition,
    StaleCancel { at: u16 },
}

fn action_strategy() -> impl Strategy<Value = GeneratedAction> {
    prop_oneof![
        (any::<u16>(), any::<u8>())
            .prop_map(|(at, payload)| GeneratedAction::Observe { at, payload }),
        any::<u16>().prop_map(|at| GeneratedAction::Cancel { at }),
        any::<u16>().prop_map(|at| GeneratedAction::Deadline { at }),
        any::<u8>().prop_map(|owner| GeneratedAction::Transfer { owner }),
        any::<u16>().prop_map(|at| GeneratedAction::Forget { at }),
        any::<u16>().prop_map(|at| GeneratedAction::Fail { at }),
        Just(GeneratedAction::FenceWithCurrentProof),
        Just(GeneratedAction::ReplayHeadTransition),
        any::<u16>().prop_map(|at| GeneratedAction::StaleCancel { at }),
    ]
}

fn generated_command(state: &WakeState, action: GeneratedAction, next_id: u64) -> WakeCommand {
    let current = contract(state);
    let current_head = current.head();
    let replay = matches!(action, GeneratedAction::ReplayHeadTransition);
    let kind = match action {
        GeneratedAction::Observe { at, payload } => WakeCommandKind::ObserveTerminal {
            expected_head: current_head,
            evidence: evidence(u64::from(at), payload),
        },
        GeneratedAction::Cancel { at } => WakeCommandKind::Cancel {
            expected_head: current_head,
            cause: CancellationCause::UserRequested,
            occurred_at: Timestamp(u64::from(at)),
        },
        GeneratedAction::Deadline { at } => WakeCommandKind::DeadlineElapsed {
            expected_head: current_head,
            observed_at: Timestamp(u64::from(at)),
        },
        GeneratedAction::Transfer { owner } => WakeCommandKind::TransferDeliveryOwner {
            expected_head: current_head,
            new_owner: WakeOwner(format!("owner-{owner}")),
        },
        GeneratedAction::Forget { at } => WakeCommandKind::Reconcile {
            expected_head: current_head,
            observation: ReconcileObservation::ResourceUnavailable {
                cause: ForgottenCause::ResourceLostAfterRestart,
                occurred_at: Timestamp(u64::from(at)),
            },
        },
        GeneratedAction::Fail { at } => WakeCommandKind::Reconcile {
            expected_head: current_head,
            observation: ReconcileObservation::ProtocolFailure {
                occurred_at: Timestamp(u64::from(at)),
            },
        },
        GeneratedAction::FenceWithCurrentProof => WakeCommandKind::Reconcile {
            expected_head: current_head,
            observation: ReconcileObservation::ObservationAuthorityFenced(
                match &current.lifecycle {
                    WakeLifecycle::Open(OpenWakeLifecycle::TerminalProposed(proposal)) => {
                        ObservationFenceProof {
                            contract_id: current.id.clone(),
                            proposed_head: current.head(),
                            proposal_transition_id: proposal.transition_id,
                        }
                    }
                    WakeLifecycle::Open(_) | WakeLifecycle::Closed(_) => ObservationFenceProof {
                        contract_id: current.id.clone(),
                        proposed_head: current.head(),
                        proposal_transition_id: TransitionId(0),
                    },
                },
            ),
        },
        GeneratedAction::ReplayHeadTransition => WakeCommandKind::Cancel {
            expected_head: current_head,
            cause: CancellationCause::Superseded,
            occurred_at: Timestamp(0),
        },
        GeneratedAction::StaleCancel { at } => WakeCommandKind::Cancel {
            expected_head: WakeHeadToken {
                generation: current.generation,
                version: Version(current.version.0.saturating_sub(1)),
            },
            cause: CancellationCause::UserRequested,
            occurred_at: Timestamp(u64::from(at)),
        },
    };
    let transition_id = if replay {
        current.head_transition_id
    } else {
        TransitionId(next_id)
    };
    command(transition_id.0, kind)
}

proptest! {
    #[test]
    fn occurrence_wins_cancel_observe_interleavings(
        cancel_at in 1u64..1_000,
        observed_at in 0u64..1_000,
    ) {
        let state = registered(1_800);
        let proposed = cancel(&state, 2, cancel_at);
        let after_observation = transition(
            &proposed.new_state,
            command(
                3,
                WakeCommandKind::ObserveTerminal {
                    expected_head: contract(&proposed.new_state).head(),
                    evidence: evidence(observed_at, 1),
                },
            ),
        );
        if observed_at < cancel_at {
            prop_assert!(
                matches!(
                    contract(&after_observation.new_state).lifecycle,
                    WakeLifecycle::Closed(CanonicalTerminal::Fired { .. })
                ),
                "earlier evidence must close fired"
            );
        } else {
            prop_assert!(matches!(
                after_observation.disposition,
                WakeDisposition::Rejected(WakeRejection::EvidenceDidNotPrecedeProposal)
            ));
            let finalized = finalize(&after_observation.new_state, 4, proposal_proof(&proposed));
            prop_assert!(
                matches!(
                    contract(&finalized.new_state).lifecycle,
                    WakeLifecycle::Closed(CanonicalTerminal::Cancelled { .. })
                ),
                "valid proof must close cancelled"
            );
        }
    }

    #[test]
    fn expiry_admits_evidence_at_deadline_but_not_after(
        deadline in 1u64..=1_800,
        delta in 1u64..10_000,
    ) {
        let state = registered(deadline);
        let proposed = transition(
            &state,
            command(2, WakeCommandKind::DeadlineElapsed {
                expected_head: contract(&state).head(),
                observed_at: Timestamp(deadline.saturating_add(delta)),
            }),
        );
        let at_deadline = transition(
            &proposed.new_state,
            command(3, WakeCommandKind::ObserveTerminal {
                expected_head: contract(&proposed.new_state).head(),
                evidence: evidence(deadline, 1),
            }),
        );
        prop_assert!(
            matches!(
                contract(&at_deadline.new_state).lifecycle,
                WakeLifecycle::Closed(CanonicalTerminal::Fired { .. })
            ),
            "evidence at the deadline must win"
        );

        let proposed = transition(
            &state,
            command(4, WakeCommandKind::DeadlineElapsed {
                expected_head: contract(&state).head(),
                observed_at: Timestamp(deadline.saturating_add(delta)),
            }),
        );
        let after_deadline = transition(
            &proposed.new_state,
            command(5, WakeCommandKind::ObserveTerminal {
                expected_head: contract(&proposed.new_state).head(),
                evidence: evidence(deadline.saturating_add(1), 2),
            }),
        );
        prop_assert!(matches!(
            after_deadline.disposition,
            WakeDisposition::Rejected(WakeRejection::EvidenceDidNotPrecedeProposal)
        ));
    }

    #[test]
    fn stale_composite_heads_never_mutate(
        at in any::<u64>(),
        stale_version in 0u64..100,
        stale_generation in 1u64..100,
    ) {
        let state = registered(1_800);
        for expected_head in [
            WakeHeadToken { generation: Generation(0), version: Version(stale_version) },
            WakeHeadToken { generation: Generation(stale_generation), version: Version(1) },
        ] {
            if expected_head == contract(&state).head() {
                continue;
            }
            let result = transition(
                &state,
                command(2, WakeCommandKind::Cancel {
                    expected_head,
                    cause: CancellationCause::UserRequested,
                    occurred_at: Timestamp(at),
                }),
            );
            prop_assert_eq!(&result.new_state, &state);
            prop_assert!(result.owed_effects.is_empty());
            prop_assert!(
                matches!(result.disposition, WakeDisposition::Rejected(WakeRejection::StaleHead { .. })),
                "a stale composite head must be rejected"
            );
        }
    }

    #[test]
    fn arbitrary_command_sequences_preserve_transition_invariants(
        actions in prop::collection::vec(action_strategy(), 0..100),
    ) {
        let mut state = registered(1_800);
        let registration_owner = contract(&state).registration_owner.clone();
        let identity = contract(&state).id.clone();
        let subject = contract(&state).subject.clone();
        let mut closed_terminal: Option<CanonicalTerminal> = None;
        let mut applied_transition_ids = BTreeSet::from([TransitionId(1)]);

        for (index, action) in actions.into_iter().enumerate() {
            let before = state.clone();
            let before_contract = contract(&before).clone();
            let command = generated_command(&before, action, index as u64 + 2);
            let command_transition_id = command.transition_id;
            let result = transition(&before, command);

            match &result.disposition {
                WakeDisposition::Applied { event } => {
                    prop_assert_eq!(contract(&result.new_state).version, before_contract.version.next());
                    prop_assert_eq!(event.transition_id, command_transition_id);
                    prop_assert!(applied_transition_ids.insert(command_transition_id));
                    for effect in &result.owed_effects {
                        prop_assert_eq!(effect.key.contract_id.clone(), identity.clone());
                        prop_assert_eq!(effect.key.generation, before_contract.generation);
                        prop_assert_eq!(effect.key.transition_id, command_transition_id);
                    }
                }
                WakeDisposition::Replayed { transition_id, .. } => {
                    prop_assert_eq!(*transition_id, before_contract.head_transition_id);
                    prop_assert_eq!(&result.new_state, &before);
                    prop_assert!(result.owed_effects.is_empty());
                }
                WakeDisposition::Rejected(_) => {
                    prop_assert_eq!(&result.new_state, &before);
                    prop_assert!(result.owed_effects.is_empty());
                }
            }

            let after = contract(&result.new_state);
            prop_assert_eq!(&after.id, &identity);
            prop_assert_eq!(&after.subject, &subject);
            prop_assert_eq!(&after.registration_owner, &registration_owner);

            if let Some(terminal) = &closed_terminal {
                prop_assert_eq!(&after.lifecycle, &WakeLifecycle::Closed(terminal.clone()));
            }
            if let WakeLifecycle::Closed(terminal) = &after.lifecycle {
                closed_terminal.get_or_insert_with(|| terminal.clone());
            }

            let has_terminal_effect = result.owed_effects.iter().any(|effect| {
                matches!(effect.kind, WakeOwedEffectKind::CommitTerminalization { .. })
            });
            prop_assert_eq!(
                has_terminal_effect,
                matches!(
                    (&before_contract.lifecycle, &after.lifecycle),
                    (WakeLifecycle::Open(_), WakeLifecycle::Closed(_))
                )
            );
            state = result.new_state;
        }
    }
}

#[test]
fn exact_transition_replay_is_typed_and_side_effect_free() {
    let state = registered(10);
    let replay = transition(&state, register_command(10));
    assert_eq!(replay.new_state, state);
    assert!(replay.owed_effects.is_empty());
    assert_eq!(
        replay.disposition,
        WakeDisposition::Replayed {
            transition_id: TransitionId(1),
            head: WakeHeadToken {
                generation: Generation(0),
                version: Version(1),
            },
        }
    );
}

#[test]
fn transition_id_reuse_requires_the_exact_semantic_command() {
    let state = registered(10);
    let conflicting_registration = transition(&state, register_command(11));
    assert_eq!(conflicting_registration.new_state, state);
    assert_eq!(
        conflicting_registration.disposition,
        WakeDisposition::Rejected(WakeRejection::ConflictingTransitionReuse)
    );

    let proposed = cancel(&state, 2, 5);
    let conflicting_kind = transition(
        &proposed.new_state,
        command(
            2,
            WakeCommandKind::ObserveTerminal {
                expected_head: contract(&proposed.new_state).head(),
                evidence: evidence(4, 1),
            },
        ),
    );
    assert_eq!(conflicting_kind.new_state, proposed.new_state);
    assert_eq!(
        conflicting_kind.disposition,
        WakeDisposition::Rejected(WakeRejection::ConflictingTransitionReuse)
    );
}

#[test]
fn registration_event_is_rebuildable_and_registry_is_exhaustive() {
    let result = transition(&WakeState::Absent, register_command(10));
    let WakeDisposition::Applied { event } = result.disposition else {
        panic!("registration should apply")
    };
    let WakeEventKind::Registered {
        registration_owner,
        subject: registered_subject,
        condition,
        registered_at,
        deadline,
    } = event.kind
    else {
        panic!("expected registration event")
    };
    assert_eq!(registration_owner, WakeOwner("registrant".to_string()));
    assert_eq!(registered_subject, subject());
    assert_eq!(condition, WakeCondition::Terminal);
    assert_eq!(registered_at, Timestamp(0));
    assert_eq!(deadline, Timestamp(10));
    assert_eq!(
        WakePublicEventRegistry::ALL,
        [
            WakePublicEventType::Registered,
            WakePublicEventType::DeliveryOwnerTransferred,
            WakePublicEventType::TerminalProposed,
            WakePublicEventType::Terminalized,
        ]
    );
}

#[test]
fn prior_transition_ids_cannot_be_reused_after_the_head_advances() {
    let state = registered(10);
    let transferred = transition(
        &state,
        command(
            2,
            WakeCommandKind::TransferDeliveryOwner {
                expected_head: contract(&state).head(),
                new_owner: WakeOwner("successor".into()),
            },
        ),
    );
    let reused = transition(&transferred.new_state, register_command(10));
    assert_eq!(reused.new_state, transferred.new_state);
    assert_eq!(
        reused.disposition,
        WakeDisposition::Rejected(WakeRejection::NonMonotonicTransitionId)
    );
}

#[test]
fn registration_rejects_deadlines_beyond_the_profile_bound() {
    let result = transition(&WakeState::Absent, register_command(1_801));
    assert_eq!(result.new_state, WakeState::Absent);
    assert!(matches!(
        result.disposition,
        WakeDisposition::Rejected(WakeRejection::InvalidDeadline)
    ));
}

#[test]
fn earlier_resource_loss_wins_while_cancellation_is_proposed() {
    let state = registered(10);
    let proposed = cancel(&state, 2, 5);
    let reconciled = transition(
        &proposed.new_state,
        command(
            3,
            WakeCommandKind::Reconcile {
                expected_head: contract(&proposed.new_state).head(),
                observation: ReconcileObservation::ResourceUnavailable {
                    cause: ForgottenCause::ResourceDestroyed,
                    occurred_at: Timestamp(4),
                },
            },
        ),
    );
    assert!(matches!(
        contract(&reconciled.new_state).lifecycle,
        WakeLifecycle::Closed(CanonicalTerminal::Forgotten {
            occurred_at: Timestamp(4),
            ..
        })
    ));
}

#[test]
fn delivery_owner_can_transfer_during_terminal_arbitration() {
    let state = registered(10);
    let proposed = cancel(&state, 2, 5);
    let transferred = transition(
        &proposed.new_state,
        command(
            3,
            WakeCommandKind::TransferDeliveryOwner {
                expected_head: contract(&proposed.new_state).head(),
                new_owner: WakeOwner("successor".into()),
            },
        ),
    );
    assert_eq!(
        contract(&transferred.new_state).delivery_owner,
        WakeOwner("successor".into())
    );
    assert!(matches!(
        contract(&transferred.new_state).lifecycle,
        WakeLifecycle::Open(OpenWakeLifecycle::TerminalProposed(_))
    ));
}

#[test]
fn finalization_requires_matching_observation_fence_proof() {
    let state = registered(10);
    let proposed = cancel(&state, 2, 5);
    let proposed_state = proposed.new_state.clone();

    let missing = transition(
        &proposed_state,
        command(
            3,
            WakeCommandKind::Reconcile {
                expected_head: contract(&proposed_state).head(),
                observation: ReconcileObservation::ResourceUnavailable {
                    cause: ForgottenCause::AdapterLostAuthority,
                    occurred_at: Timestamp(6),
                },
            },
        ),
    );
    assert!(matches!(
        missing.disposition,
        WakeDisposition::Rejected(WakeRejection::ObservationFenceProofRequired)
    ));

    let wrong = ObservationFenceProof {
        proposal_transition_id: TransitionId(999),
        ..proposal_proof(&proposed)
    };
    let mismatch = finalize(&proposed_state, 4, wrong);
    assert!(matches!(
        mismatch.disposition,
        WakeDisposition::Rejected(WakeRejection::ObservationFenceProofMismatch)
    ));

    let finalized = finalize(&proposed_state, 5, proposal_proof(&proposed));
    assert!(matches!(
        contract(&finalized.new_state).lifecycle,
        WakeLifecycle::Closed(CanonicalTerminal::Cancelled { .. })
    ));
}

#[test]
fn proposal_is_semantic_arbitration_and_owes_no_terminal_delivery() {
    let state = registered(10);
    let proposed = cancel(&state, 2, 5);
    assert!(matches!(
        contract(&proposed.new_state).lifecycle,
        WakeLifecycle::Open(OpenWakeLifecycle::TerminalProposed(_))
    ));
    assert!(proposed.owed_effects.iter().all(|effect| !matches!(
        effect.kind,
        WakeOwedEffectKind::CommitTerminalization { .. }
    )));
    assert!(proposed
        .owed_effects
        .iter()
        .all(|effect| !matches!(effect.kind, WakeOwedEffectKind::BeginObservation { .. })));
}

#[test]
fn cancellation_is_observation_only() {
    let state = registered(10);
    let result = cancel(&state, 2, 5);
    assert_eq!(result.owed_effects.len(), 1);
    assert!(matches!(
        result.owed_effects[0].kind,
        WakeOwedEffectKind::FenceObservationAuthority { .. }
    ));
}

#[test]
fn delivery_transfer_preserves_registration_and_resource_identity() {
    let state = registered(10);
    let before = contract(&state).clone();
    let result = transition(
        &state,
        command(
            2,
            WakeCommandKind::TransferDeliveryOwner {
                expected_head: before.head(),
                new_owner: WakeOwner("successor".to_string()),
            },
        ),
    );
    let after = contract(&result.new_state);
    assert_eq!(after.id, before.id);
    assert_eq!(after.subject, before.subject);
    assert_eq!(after.generation, before.generation);
    assert_eq!(after.registration_owner, before.registration_owner);
    assert_eq!(after.delivery_owner, WakeOwner("successor".to_string()));
}

#[test]
fn event_boundary_adds_contract_and_transition_identity_once() {
    let state = registered(10);
    let result = transition(
        &state,
        command(
            2,
            WakeCommandKind::ObserveTerminal {
                expected_head: contract(&state).head(),
                evidence: evidence(5, 7),
            },
        ),
    );
    let WakeDisposition::Applied { event } = &result.disposition else {
        panic!("observation should apply")
    };
    assert_eq!(event.contract_id, WakeContractId::new("contract").unwrap());
    assert_eq!(event.transition_id, TransitionId(2));
    assert!(matches!(
        event.kind,
        WakeEventKind::Terminalized {
            terminal: CanonicalTerminal::Fired { .. },
            ..
        }
    ));
    assert!(result
        .owed_effects
        .iter()
        .all(|effect| effect.key.transition_id == event.transition_id));
}

#[test]
fn zero_duration_registration_is_rejected_by_bounded_deadline_policy() {
    let result = transition(&WakeState::Absent, register_command(0));
    assert_eq!(result.new_state, WakeState::Absent);
    assert!(matches!(
        result.disposition,
        WakeDisposition::Rejected(WakeRejection::InvalidDeadline)
    ));
}
