use std::collections::BTreeMap;

use crate::{
    AuthorityOutcome, ClaimOutcome, CommitOutcome, EffectStatus, LeaseExpiry, ManualChoiceKind,
    ReducerInboxId, ReducerInboxKind, ReducerInboxPayload, Timestamp, Version, WorkflowId,
    WorkflowProfile, WorkflowState,
};

use super::{
    acceptance_owed_decl, authoritative_observation, barrier_events, cancellation_request,
    cancelled_terminal_payload, compare_receipts, continuation_from_snapshot,
    deadline_matches_exactly, evidence_matches_resource, fence_accepts, forgotten_terminal_payload,
    inbox_contains_registration_barrier, lifecycle_fence, manual_choices, profile,
    project_runtime_availability, protocol, registration_decision, registration_fence,
    registration_receipt, shadow_comparison, terminal_codec, terminal_payload_from_evidence,
    transfer_continuation, BashResourceIdentity, BashTerminalEvidence, BashTerminalStatus,
    FenceStatus, ObserveHandleIntent, RuntimeAvailability, RuntimeAvailabilityProjection,
    TmuxResourceIdentity, TmuxTerminalEvidence, TmuxTerminalStatus, WakeCancellationOutcome,
    WakeCancellationReason, WakeForgottenReason, WakeManualPayload, WakeProfile,
    WakeRegistrationIntent, WakeRegistrationReceipt, WakeResourceIdentity,
    WakeShadowComparisonKind, WakeTerminalEvidence, WakeTerminalPayload, WorkScopeIdentity,
    WorkScopeKind, REGISTRATION_BARRIER_ID, REGISTRATION_EFFECT_ID,
};

fn scope() -> WorkScopeIdentity {
    WorkScopeIdentity {
        kind: WorkScopeKind::Conversation,
        stable_key: "conv-7".into(),
    }
}

fn bash_identity(handle_id: impl Into<String>) -> WakeResourceIdentity {
    WakeResourceIdentity::Bash(BashResourceIdentity {
        work_scope: scope(),
        handle_id: handle_id.into(),
    })
}

fn tmux_identity(window_id: impl Into<String>) -> WakeResourceIdentity {
    WakeResourceIdentity::TmuxWindow(TmuxResourceIdentity {
        work_scope: scope(),
        server_generation: "srv-1".into(),
        window_id: window_id.into(),
    })
}

fn registration_intent(resource: WakeResourceIdentity) -> WakeRegistrationIntent {
    WakeRegistrationIntent {
        contract_id: "contract-1".into(),
        conversation_id: "conv-7".into(),
        registration_scope: scope(),
        resource,
        registering_tool_use_id: "tool-9".into(),
        registered_at: Timestamp(5),
        expires_at: Timestamp(20),
    }
}

fn workflow() -> WorkflowState<WakeProfile> {
    let intent = registration_intent(bash_identity("seed"));
    WorkflowState::<WakeProfile>::new_authoritative(
        WorkflowId(7),
        &profile(),
        &protocol("wake-v1", true),
        super::snapshot_codec(),
        super::registration_snapshot(&intent, Version(3)),
    )
    .expect("accepting protocol")
}

#[test]
fn persistence_domain_accepts_runtime_owned_strings() {
    let dynamic = |value: &str| value.to_owned();
    let intent = WakeRegistrationIntent {
        contract_id: dynamic("contract-runtime"),
        conversation_id: dynamic("conversation-runtime"),
        registration_scope: WorkScopeIdentity {
            kind: WorkScopeKind::Worktree,
            stable_key: dynamic("worktree-runtime"),
        },
        resource: WakeResourceIdentity::TmuxWindow(TmuxResourceIdentity {
            work_scope: WorkScopeIdentity {
                kind: WorkScopeKind::Worktree,
                stable_key: dynamic("worktree-runtime"),
            },
            server_generation: dynamic("generation-runtime"),
            window_id: dynamic("window-runtime"),
        }),
        registering_tool_use_id: dynamic("tool-runtime"),
        registered_at: Timestamp(5),
        expires_at: Timestamp(20),
    };

    let snapshot = super::registration_snapshot(&intent, Version(4));
    let receipt = registration_receipt(&intent);
    let continuation = continuation_from_snapshot(&snapshot, vec![], vec![], 9);

    assert_eq!(snapshot.contract_id, "contract-runtime");
    assert_eq!(snapshot.conversation_id, "conversation-runtime");
    assert_eq!(receipt.registering_tool_use_id, "tool-runtime");
    assert_eq!(continuation.pending_contract, "contract-runtime");
    assert_eq!(
        snapshot.resource.work_scope().stable_key,
        "worktree-runtime"
    );
}

#[test]
fn registration_installs_required_observe_handle_without_destructive_lock_or_owed_acceptance() {
    let mut workflow = workflow();
    let intent = registration_intent(bash_identity("b-7"));
    let (decision, events) = registration_decision(Version(0), &intent, Version(9));
    let result = workflow
        .commit_transition(&decision, &events)
        .expect("registration commit succeeds");
    assert_eq!(result.outcome, CommitOutcome::Committed);

    let effect = &workflow.effects[&REGISTRATION_EFFECT_ID];
    assert_eq!(effect.declaration.kind, super::OBSERVE_HANDLE_KIND);
    assert_eq!(effect.declaration.generation, workflow.generation);
    assert_eq!(effect.status, EffectStatus::Eligible);
    assert_eq!(effect.declaration.destructive_resource, None);
    assert!(effect.destructive_lock.is_none());
    assert!(workflow.owed_acceptances.is_empty());
    assert_eq!(workflow.snapshot.registration_fence_version, Version(9));

    let observe = &effect.declaration.intent;
    assert_eq!(
        observe,
        &ObserveHandleIntent {
            contract_id: intent.contract_id,
            resource: intent.resource,
            expires_at: intent.expires_at,
        }
    );
}

#[test]
fn registration_receipt_and_barrier_round_trip_preserve_contract_resource_and_deadline() {
    let receipt = registration_receipt(&registration_intent(tmux_identity("win-8")));
    let events = barrier_events(receipt.clone());
    assert!(events.contains_key(&REGISTRATION_BARRIER_ID));
    assert_eq!(
        super::registration_barrier_event(receipt.clone()),
        super::WakeBarrierEvent::RegistrationObserved {
            receipt: receipt.clone()
        }
    );

    let event = events
        .get(&REGISTRATION_BARRIER_ID)
        .expect("barrier exists");
    assert!(matches!(
        event,
        super::WakeBarrierEvent::RegistrationObserved { receipt: found } if found == &receipt
    ));
}

#[test]
fn bash_and_tmux_evidence_are_typed_and_match_exact_identity() {
    let bash = bash_identity("b-1");
    let bash_evidence = WakeTerminalEvidence::Bash(BashTerminalEvidence {
        identity: match &bash {
            WakeResourceIdentity::Bash(identity) => identity.clone(),
            WakeResourceIdentity::TmuxWindow(_) => unreachable!(),
        },
        status: BashTerminalStatus::Killed,
        occurred_at: Timestamp(10),
        exit_code: None,
        duration_ms: Some(100),
        signal_number: Some(15),
        kill_signal_sent: Some("TERM".into()),
        final_tail: vec!["line 1".into(), "line 2".into()],
    });
    assert!(evidence_matches_resource(&bash_evidence, &bash));

    let tmux = tmux_identity("win-3");
    let tmux_evidence = WakeTerminalEvidence::TmuxWindow(TmuxTerminalEvidence {
        identity: match &tmux {
            WakeResourceIdentity::TmuxWindow(identity) => identity.clone(),
            WakeResourceIdentity::Bash(_) => unreachable!(),
        },
        status: TmuxTerminalStatus::ExitMarkerObserved,
        occurred_at: Timestamp(11),
        exit_code: Some(0),
        duration_ms: Some(77),
        final_tail: vec!["done".into()],
    });
    assert!(evidence_matches_resource(&tmux_evidence, &tmux));
    assert!(!evidence_matches_resource(&tmux_evidence, &bash));
}

#[test]
fn fired_vs_expired_vs_forgotten_payloads_are_structurally_distinct() {
    let resource = bash_identity("b-9");
    let fired_evidence = WakeTerminalEvidence::Bash(BashTerminalEvidence {
        identity: match &resource {
            WakeResourceIdentity::Bash(identity) => identity.clone(),
            WakeResourceIdentity::TmuxWindow(_) => unreachable!(),
        },
        status: BashTerminalStatus::Exited,
        occurred_at: Timestamp(19),
        exit_code: Some(0),
        duration_ms: Some(44),
        signal_number: None,
        kill_signal_sent: None,
        final_tail: vec!["ok".into()],
    });
    let fired = terminal_payload_from_evidence(
        "contract-1",
        resource.clone(),
        fired_evidence.clone(),
        Timestamp(20),
    )
    .expect("matching evidence");
    assert!(matches!(
        fired,
        WakeTerminalPayload::Fired {
            contract_id,
            resource: found,
            evidence,
            resolved_at: Timestamp(19)
        } if contract_id == "contract-1" && found == resource && evidence == fired_evidence
    ));

    let expired = terminal_payload_from_evidence(
        "contract-1",
        resource.clone(),
        WakeTerminalEvidence::Bash(BashTerminalEvidence {
            identity: match &resource {
                WakeResourceIdentity::Bash(identity) => identity.clone(),
                WakeResourceIdentity::TmuxWindow(_) => unreachable!(),
            },
            status: BashTerminalStatus::Exited,
            occurred_at: Timestamp(21),
            exit_code: Some(0),
            duration_ms: None,
            signal_number: None,
            kill_signal_sent: None,
            final_tail: vec!["late".into()],
        }),
        Timestamp(20),
    )
    .expect("matching evidence");
    assert!(matches!(
        expired,
        WakeTerminalPayload::Expired {
            contract_id,
            resource: found,
            resolved_at: Timestamp(20)
        } if contract_id == "contract-1" && found == resource
    ));

    let forgotten = forgotten_terminal_payload(
        "contract-1",
        resource.clone(),
        WakeForgottenReason::HandleMissing,
        Timestamp(20),
    );
    assert!(matches!(
        forgotten,
        WakeTerminalPayload::Forgotten {
            reason: WakeForgottenReason::HandleMissing,
            ..
        }
    ));
}

#[test]
fn receipt_comparison_requires_exact_identity_and_exact_deadline_equality() {
    let expected = WakeRegistrationReceipt {
        contract_id: "contract-1".into(),
        resource: bash_identity("b-12"),
        expires_at: Timestamp(20),
        registering_tool_use_id: "tool-9".into(),
    };
    let equal = compare_receipts(&expected, &expected.clone());
    assert!(equal.equal);
    assert!(equal.exact_identity_match);
    assert!(equal.exact_deadline_match);
    assert!(deadline_matches_exactly(Timestamp(20), Timestamp(20)));

    let wrong_deadline = WakeRegistrationReceipt {
        expires_at: Timestamp(21),
        ..expected.clone()
    };
    let compare_deadline = compare_receipts(&expected, &wrong_deadline);
    assert!(!compare_deadline.equal);
    assert!(compare_deadline.exact_identity_match);
    assert!(!compare_deadline.exact_deadline_match);

    let wrong_identity = WakeRegistrationReceipt {
        resource: bash_identity("b-13"),
        ..expected.clone()
    };
    let compare_identity = compare_receipts(&expected, &wrong_identity);
    assert!(!compare_identity.equal);
    assert!(!compare_identity.exact_identity_match);
}

#[test]
fn cancellation_invalidates_observation_and_uses_pure_cancelled_terminal_payload_helper() {
    let mut workflow = workflow();
    let intent = registration_intent(bash_identity("b-10"));
    let (decision, events) = registration_decision(Version(0), &intent, Version(4));
    workflow
        .commit_transition(&decision, &events)
        .expect("registration commit succeeds");

    let WakeCancellationOutcome::Request(request) = cancellation_request(&workflow, Timestamp(12))
    else {
        panic!("pending observation is cancellable");
    };
    assert_eq!(request.invalidations.len(), 1);
    assert!(request
        .invalidations
        .iter()
        .any(|decl| decl.effect_id == REGISTRATION_EFFECT_ID));
    assert!(request.compensation_plan.effects.is_empty());
    assert!(request.compensation_plan.barriers.is_empty());
    assert!(matches!(
        cancelled_terminal_payload(
            "contract-1",
            bash_identity("b-10"),
            WakeCancellationReason::ExplicitCancel,
            Timestamp(5)
        ),
        WakeTerminalPayload::Cancelled {
            reason: WakeCancellationReason::ExplicitCancel,
            ..
        }
    ));

    let result = workflow
        .cancel_with_compensation(&request, &BTreeMap::new())
        .expect("pure cancellation succeeds");
    assert_eq!(result.outcome, CommitOutcome::Committed);
    assert_eq!(workflow.status, crate::WorkflowStatus::Cancelled);
    assert!(workflow.snapshot.cancelled);
    assert!(matches!(
        workflow.snapshot.terminal,
        Some(WakeTerminalPayload::Cancelled { .. })
    ));
    assert_eq!(result.reducer_events.len(), 1);
    assert_eq!(workflow.reducer_inbox.len(), 1);
    assert_eq!(result.reducer_events[0].event_codec, terminal_codec());
    assert!(matches!(
        result.reducer_events[0].payload,
        crate::ReducerInboxPayload::Receipt(WakeTerminalPayload::Cancelled { .. })
    ));
    assert!(workflow.owed_acceptances.is_empty());
    assert_eq!(
        workflow.effects[&REGISTRATION_EFFECT_ID].status,
        EffectStatus::Invalidated
    );
    assert_eq!(
        workflow.effects[&REGISTRATION_EFFECT_ID]
            .declaration
            .intent
            .resource,
        bash_identity("b-10")
    );
}

#[test]
fn cancellation_preserves_receipted_terminal_winner_before_snapshot_projection() {
    let intent = registration_intent(bash_identity("seed"));
    let mut workflow = workflow();
    let (decision, barriers) = registration_decision(Version(0), &intent, Version(3));
    workflow
        .commit_transition(&decision, &barriers)
        .expect("registration commit");
    let claim = workflow.claim_effect(
        REGISTRATION_EFFECT_ID,
        "worker",
        Timestamp(1),
        LeaseExpiry(10),
    );
    let authority = claim.authority.expect("authority");
    let attempt = claim.attempt.expect("attempt");
    let terminal = terminal_payload_from_evidence(
        intent.contract_id.clone(),
        intent.resource.clone(),
        WakeTerminalEvidence::Bash(BashTerminalEvidence {
            identity: match &intent.resource {
                WakeResourceIdentity::Bash(identity) => identity.clone(),
                WakeResourceIdentity::TmuxWindow(_) => unreachable!(),
            },
            status: BashTerminalStatus::Exited,
            occurred_at: Timestamp(5),
            exit_code: Some(0),
            duration_ms: Some(5),
            signal_number: None,
            kill_signal_sent: None,
            final_tail: vec![],
        }),
        intent.expires_at,
    )
    .expect("matching terminal");
    workflow.accept_receipt(
        &authority,
        Timestamp(5),
        Some(attempt.id),
        crate::ReceiptOrigin::Execution,
        terminal_codec(),
        terminal.clone(),
        terminal_codec(),
        terminal.clone(),
    );
    assert_eq!(workflow.snapshot.terminal, None);
    assert_eq!(
        cancellation_request(&workflow, Timestamp(6)),
        WakeCancellationOutcome::AlreadyTerminal(Box::new(terminal))
    );
}

#[test]
fn runtime_availability_projection_is_explicit_accept_defer_suppress() {
    assert_eq!(
        project_runtime_availability(RuntimeAvailability::Idle),
        RuntimeAvailabilityProjection::Accept
    );
    assert_eq!(
        project_runtime_availability(RuntimeAvailability::Busy),
        RuntimeAvailabilityProjection::Defer
    );
    assert_eq!(
        project_runtime_availability(RuntimeAvailability::Terminal),
        RuntimeAvailabilityProjection::Suppress
    );
}

#[test]
fn continuation_transfer_preserves_pending_contract_resource_and_deadline() {
    let snapshot =
        super::registration_snapshot(&registration_intent(tmux_identity("win-11")), Version(8));
    let continuation = continuation_from_snapshot(
        &snapshot,
        vec![ReducerInboxId(4), ReducerInboxId(5)],
        vec![7, 8],
        99,
    );
    assert_eq!(continuation.pending_contract, snapshot.contract_id);
    assert_eq!(continuation.resource, snapshot.resource);
    assert_eq!(continuation.expires_at, snapshot.expires_at);
    assert_eq!(
        continuation.inbox_ids,
        vec![ReducerInboxId(4), ReducerInboxId(5)]
    );
    assert_eq!(continuation.owed_ids, vec![7, 8]);
    assert_eq!(continuation.successor_workflow_id, 99);

    let transferred = transfer_continuation(&snapshot, vec![ReducerInboxId(4)], vec![7], 100);
    assert_eq!(transferred.resource, snapshot.resource);
    assert_eq!(transferred.expires_at, snapshot.expires_at);
    let stored = transferred.continuation.expect("continuation stored");
    assert_eq!(stored.pending_contract, snapshot.contract_id);
    assert_eq!(stored.successor_workflow_id, 100);
}

#[test]
fn conversation_registration_and_lifecycle_fences_share_version_gate() {
    let workflow = workflow();
    let registration = registration_fence(Version(3), FenceStatus::Open);
    let lifecycle = lifecycle_fence(Version(3), FenceStatus::Open);
    assert!(fence_accepts(&workflow, &registration, &lifecycle));

    let closed = lifecycle_fence(Version(3), FenceStatus::Closed);
    assert!(!fence_accepts(&workflow, &registration, &closed));
    let wrong = registration_fence(Version(4), FenceStatus::Open);
    assert!(!fence_accepts(&workflow, &wrong, &lifecycle));
}

#[test]
fn shadow_comparison_maps_all_specified_kinds() {
    let cases = [
        WakeShadowComparisonKind::Registration,
        WakeShadowComparisonKind::Observation,
        WakeShadowComparisonKind::TerminalReceipt,
        WakeShadowComparisonKind::Inbox,
        WakeShadowComparisonKind::Acceptance,
        WakeShadowComparisonKind::Lifecycle,
        WakeShadowComparisonKind::Capability,
        WakeShadowComparisonKind::UserProjection,
    ];
    let mapped = cases.map(shadow_comparison);
    assert_eq!(
        mapped[0].generic_kind,
        crate::ShadowDivergenceKind::Snapshot
    );
    assert_eq!(
        mapped[1].generic_kind,
        crate::ShadowDivergenceKind::Observation
    );
    assert_eq!(mapped[2].generic_kind, crate::ShadowDivergenceKind::Receipt);
    assert_eq!(
        mapped[3].generic_kind,
        crate::ShadowDivergenceKind::ReducerEvent
    );
    assert_eq!(
        mapped[4].generic_kind,
        crate::ShadowDivergenceKind::Transition
    );
    assert_eq!(
        mapped[5].generic_kind,
        crate::ShadowDivergenceKind::EffectPlan
    );
    assert_eq!(
        mapped[6].generic_kind,
        crate::ShadowDivergenceKind::Capability
    );
    assert_eq!(
        mapped[7].generic_kind,
        crate::ShadowDivergenceKind::UserProjection
    );
}

#[test]
fn registration_barrier_receipt_round_trip_is_deterministic() {
    let mut workflow = workflow();
    let receipt = registration_receipt(&registration_intent(tmux_identity("win-14")));
    let (decision, events) = registration_decision(
        Version(0),
        &registration_intent(tmux_identity("win-14")),
        Version(3),
    );
    workflow
        .commit_transition(&decision, &events)
        .expect("registration commit succeeds");
    let claim = workflow.claim_effect(
        REGISTRATION_EFFECT_ID,
        "wake-worker",
        Timestamp(0),
        LeaseExpiry(50),
    );
    assert_eq!(claim.outcome, ClaimOutcome::Claimed);
    let authority = claim.authority.unwrap();
    let attempt = claim.attempt.unwrap();
    assert_eq!(
        workflow.record_observation(
            &authority,
            Timestamp(5),
            attempt.id,
            terminal_codec(),
            WakeTerminalEvidence::TmuxWindow(TmuxTerminalEvidence {
                identity: TmuxResourceIdentity {
                    work_scope: scope(),
                    server_generation: "srv-1".into(),
                    window_id: "win-14".into(),
                },
                status: TmuxTerminalStatus::ExitMarkerObserved,
                occurred_at: Timestamp(5),
                exit_code: Some(0),
                duration_ms: Some(10),
                final_tail: vec!["done".into()],
            }),
        ),
        AuthorityOutcome::Authorized
    );
    let terminal = terminal_payload_from_evidence(
        "contract-1",
        tmux_identity("win-14"),
        WakeTerminalEvidence::TmuxWindow(TmuxTerminalEvidence {
            identity: TmuxResourceIdentity {
                work_scope: scope(),
                server_generation: "srv-1".into(),
                window_id: "win-14".into(),
            },
            status: TmuxTerminalStatus::ExitMarkerObserved,
            occurred_at: Timestamp(5),
            exit_code: Some(0),
            duration_ms: Some(10),
            final_tail: vec!["done".into()],
        }),
        Timestamp(20),
    )
    .expect("matching evidence");
    let accepted = workflow.accept_receipt(
        &authority,
        Timestamp(5),
        Some(attempt.id),
        crate::ReceiptOrigin::Execution,
        terminal_codec(),
        terminal.clone(),
        terminal_codec(),
        terminal,
    );
    assert_eq!(accepted.outcome, AuthorityOutcome::Authorized);
    assert_eq!(accepted.reducer_events.len(), 1);
    let barrier = &accepted.reducer_events[0];
    assert_eq!(barrier.kind, ReducerInboxKind::BarrierSatisfied);
    assert_eq!(
        barrier.payload,
        ReducerInboxPayload::Barrier(super::WakeBarrierEvent::RegistrationObserved {
            receipt: receipt.clone()
        })
    );
    assert!(inbox_contains_registration_barrier(barrier, &receipt));
}

#[test]
fn authoritative_observation_helper_and_acceptance_decl_are_typed() {
    let mut workflow = workflow();
    let (decision, events) = registration_decision(
        Version(0),
        &registration_intent(bash_identity("b-15")),
        Version(3),
    );
    workflow
        .commit_transition(&decision, &events)
        .expect("registration commit succeeds");
    let claim = workflow.claim_effect(
        REGISTRATION_EFFECT_ID,
        "wake-worker",
        Timestamp(0),
        LeaseExpiry(50),
    );
    let authority = claim.authority.unwrap();
    let attempt = claim.attempt.unwrap();
    let evidence = WakeTerminalEvidence::Bash(BashTerminalEvidence {
        identity: BashResourceIdentity {
            work_scope: scope(),
            handle_id: "b-15".into(),
        },
        status: BashTerminalStatus::KillPendingKernel,
        occurred_at: Timestamp(6),
        exit_code: None,
        duration_ms: Some(20),
        signal_number: Some(9),
        kill_signal_sent: Some("KILL".into()),
        final_tail: vec!["hung".into()],
    });
    assert_eq!(
        workflow.record_observation(
            &authority,
            Timestamp(6),
            attempt.id,
            terminal_codec(),
            evidence
        ),
        AuthorityOutcome::Authorized
    );
    let effect = &workflow.effects[&REGISTRATION_EFFECT_ID];
    let observed =
        authoritative_observation(&authority, effect).expect("authoritative observation");
    assert!(matches!(
        &observed.observation,
        WakeTerminalEvidence::Bash(BashTerminalEvidence {
            status: BashTerminalStatus::KillPendingKernel,
            ..
        })
    ));

    let terminal = terminal_payload_from_evidence(
        "contract-1",
        bash_identity("b-15"),
        observed.observation.clone(),
        Timestamp(20),
    )
    .expect("matching evidence");
    let owed = acceptance_owed_decl(ReducerInboxId(9), &terminal)
        .expect("non-cancelled terminal receipt can auto-resume");
    assert_eq!(owed.reducer_inbox_id, ReducerInboxId(9));
    assert_eq!(owed.source_kind, "wake_terminal_receipt");
    assert_eq!(owed.event, terminal);

    let cancelled = cancelled_terminal_payload(
        "contract-1",
        bash_identity("b-15"),
        WakeCancellationReason::ExplicitCancel,
        Timestamp(7),
    );
    assert_eq!(acceptance_owed_decl(ReducerInboxId(10), &cancelled), None);
}

#[test]
fn runtime_acceptance_decision_names_the_exact_owed_terminal() {
    let owed = forgotten_terminal_payload(
        "contract-1",
        bash_identity("b-15"),
        WakeForgottenReason::HandleMissing,
        Timestamp(20),
    );
    let other = forgotten_terminal_payload(
        "contract-2",
        bash_identity("b-16"),
        WakeForgottenReason::HandleMissing,
        Timestamp(21),
    );

    assert!(WakeProfile::decision_handles_owed_acceptance(
        &owed,
        &super::WakeRegistrationEvent::RuntimeAccepted {
            terminal: Box::new(owed.clone()),
        }
    ));
    assert!(!WakeProfile::decision_handles_owed_acceptance(
        &owed,
        &super::WakeRegistrationEvent::RuntimeAccepted {
            terminal: Box::new(other),
        }
    ));
    assert!(WakeProfile::decision_handles_owed_acceptance_suppression(
        &owed,
        &super::WakeRegistrationEvent::RuntimeSuppressed {
            terminal: Box::new(owed.clone()),
        }
    ));
    assert!(!WakeProfile::decision_handles_owed_acceptance_suppression(
        &owed,
        &super::WakeRegistrationEvent::RuntimeAccepted {
            terminal: Box::new(owed.clone()),
        }
    ));
}

#[test]
fn manual_choices_export_expected_codec_payloads_and_terminal_receipts() {
    let terminal = forgotten_terminal_payload(
        "contract-1",
        bash_identity("b-15"),
        WakeForgottenReason::HandleMissing,
        Timestamp(20),
    );
    let choices = manual_choices(&terminal);
    assert_eq!(choices.len(), 3);
    assert!(choices
        .iter()
        .all(|choice| choice.codec.family == super::MANUAL_CODEC_FAMILY));
    assert_eq!(choices[0].payload, WakeManualPayload::Accept);
    assert_eq!(choices[1].payload, WakeManualPayload::Defer);
    assert_eq!(choices[2].payload, WakeManualPayload::Suppress);
    assert!(choices.iter().all(|choice| choice.receipt == terminal));
    assert!(choices
        .iter()
        .all(|choice| choice.receipt_event == terminal));
}

#[test]
fn terminal_receipt_requires_exact_terminal_projection_event() {
    let terminal = forgotten_terminal_payload(
        "contract-1",
        bash_identity("b-terminal"),
        WakeForgottenReason::HandleMissing,
        Timestamp(12),
    );
    let inbox = ReducerInboxPayload::Receipt(terminal.clone());

    assert!(!WakeProfile::decision_handles_inbox(
        &inbox,
        &super::WakeRegistrationEvent::Registered,
    ));
    assert!(WakeProfile::decision_handles_inbox(
        &inbox,
        &super::WakeRegistrationEvent::TerminalProjected {
            terminal: Box::new(terminal),
        },
    ));
}

#[test]
fn suppress_manual_choice_is_typed_as_suppress() {
    let terminal = forgotten_terminal_payload(
        "contract-1",
        bash_identity("b-suppress"),
        WakeForgottenReason::HandleMissing,
        Timestamp(12),
    );
    let choices = manual_choices(&terminal);
    let suppress = choices
        .iter()
        .find(|choice| choice.payload == WakeManualPayload::Suppress)
        .expect("suppress choice");

    assert_eq!(suppress.kind, ManualChoiceKind::Suppress);
}
