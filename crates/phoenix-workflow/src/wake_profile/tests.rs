use std::collections::BTreeMap;

use crate::{
    AuthorityOutcome, ClaimOutcome, CommitOutcome, EffectStatus, LeaseExpiry, ReducerInboxKind,
    ReducerInboxPayload, Timestamp, Version, WorkflowId, WorkflowState,
};

use super::{
    barrier_events, cancellation_request, continuation_from_snapshot, fence_accepts,
    lifecycle_fence, manual_choices, profile, project_runtime_acceptance, protocol,
    receipt_from_evidence, registration_decision, registration_snapshot, shadow_parity,
    transfer_continuation, BusyIdleAcceptance, BusyIdleProjection, RuntimeAvailability,
    TerminalWakeCause, TerminalWakeEvidence, WakeProfile, WakeRegistrationIntent,
    WakeResourceIdentity, REGISTRATION_BARRIER_ID, REGISTRATION_EFFECT_ID,
};

fn workflow() -> WorkflowState<WakeProfile> {
    WorkflowState::<WakeProfile>::new_authoritative(
        WorkflowId(7),
        &profile(),
        &protocol("wake-v1", true),
        super::snapshot_codec(),
        registration_snapshot(
            WakeResourceIdentity::Bash { handle_id: "seed" },
            Timestamp(50),
            BusyIdleAcceptance::Either,
        ),
    )
}

#[test]
fn registration_installs_observe_handle_effect_and_barrier() {
    let mut workflow = workflow();
    let identity = WakeResourceIdentity::Bash { handle_id: "b-7" };
    let (decision, events) = registration_decision(
        Version(0),
        identity,
        Timestamp(20),
        BusyIdleAcceptance::Either,
    );
    let result = workflow
        .commit_transition(&decision, &events)
        .expect("registration commit succeeds");
    assert_eq!(result.outcome, CommitOutcome::Committed);
    let effect = &workflow.effects[&REGISTRATION_EFFECT_ID];
    assert_eq!(effect.declaration.kind, super::OBSERVE_HANDLE_KIND);
    assert_eq!(effect.declaration.generation, workflow.generation);
    assert_eq!(effect.status, EffectStatus::Eligible);
    assert_eq!(effect.declaration.destructive_resource, None);
    assert_eq!(workflow.barriers.len(), 1);
    assert!(workflow.barriers.contains_key(&REGISTRATION_BARRIER_ID));
    assert!(workflow.owed_acceptances.is_empty());
}

#[test]
fn identities_are_stable_for_bash_and_tmux() {
    let bash = WakeResourceIdentity::Bash { handle_id: "b-8" };
    let tmux = WakeResourceIdentity::Tmux {
        session_id: "tmux-8",
    };
    assert_eq!(bash.stable_key(), "b-8");
    assert_eq!(bash.destructive_resource(), "bash:b-8");
    assert_eq!(tmux.stable_key(), "tmux-8");
    assert_eq!(tmux.destructive_resource(), "tmux:tmux-8");
}

#[test]
fn deadline_precedence_accepts_before_equal_and_uses_deadline_after() {
    let identity = WakeResourceIdentity::Bash { handle_id: "b-9" };
    let deadline = Timestamp(10);

    let before = receipt_from_evidence(
        identity,
        &TerminalWakeEvidence::Exited {
            occurred_at: Timestamp(9),
            exit_code: 0,
        },
        deadline,
    );
    assert_eq!(before.cause, TerminalWakeCause::Fired);
    assert_eq!(before.observed_at, Timestamp(9));

    let equal = receipt_from_evidence(
        identity,
        &TerminalWakeEvidence::TmuxFinished {
            occurred_at: Timestamp(10),
        },
        deadline,
    );
    assert_eq!(equal.cause, TerminalWakeCause::Fired);
    assert_eq!(equal.observed_at, Timestamp(10));

    let after = receipt_from_evidence(
        identity,
        &TerminalWakeEvidence::Exited {
            occurred_at: Timestamp(11),
            exit_code: 0,
        },
        deadline,
    );
    assert_eq!(after.cause, TerminalWakeCause::Expired);
    assert_eq!(after.observed_at, Timestamp(10));
}

#[test]
fn cancellation_invalidates_observation_without_kill_compensation() {
    let mut workflow = workflow();
    let identity = WakeResourceIdentity::Bash { handle_id: "b-10" };
    let (decision, events) = registration_decision(
        Version(0),
        identity,
        Timestamp(20),
        BusyIdleAcceptance::Either,
    );
    workflow
        .commit_transition(&decision, &events)
        .expect("registration commit succeeds");

    let request = cancellation_request(&workflow);
    assert_eq!(request.invalidations.len(), 1);
    assert!(request
        .invalidations
        .iter()
        .any(|decl| decl.effect_id == REGISTRATION_EFFECT_ID));
    assert!(request.compensation_plan.effects.is_empty());
    assert!(request.compensation_plan.barriers.is_empty());

    let result = workflow
        .cancel_with_compensation(&request, &BTreeMap::new())
        .expect("pure cancellation succeeds");
    assert_eq!(result.outcome, CommitOutcome::Committed);
    assert_eq!(
        workflow.effects[&REGISTRATION_EFFECT_ID].status,
        EffectStatus::Invalidated
    );
}

#[test]
fn busy_deferral_projection_is_pure() {
    assert_eq!(
        project_runtime_acceptance(RuntimeAvailability::Busy, TerminalWakeCause::Fired),
        BusyIdleProjection::Defer
    );
    assert_eq!(
        project_runtime_acceptance(RuntimeAvailability::Idle, TerminalWakeCause::Fired),
        BusyIdleProjection::AcceptNow(TerminalWakeCause::Fired)
    );
    assert_eq!(
        project_runtime_acceptance(RuntimeAvailability::Terminal, TerminalWakeCause::Fired),
        BusyIdleProjection::Suppress
    );
}

#[test]
fn continuation_transfer_preserves_identity_and_deadline() {
    let snapshot = registration_snapshot(
        WakeResourceIdentity::Tmux {
            session_id: "tmux-11",
        },
        Timestamp(33),
        BusyIdleAcceptance::Busy,
    );
    let continuation = continuation_from_snapshot(&snapshot);
    assert_eq!(continuation.identity, snapshot.identity);
    assert_eq!(continuation.deadline, snapshot.deadline);

    let transferred = transfer_continuation(&snapshot, BusyIdleAcceptance::Idle);
    assert_eq!(transferred.identity, snapshot.identity);
    assert_eq!(transferred.deadline, snapshot.deadline);
    assert_eq!(transferred.accepted, BusyIdleAcceptance::Idle);
    let prior = transferred.continuation.expect("continuation retained");
    assert_eq!(prior.identity, snapshot.identity);
    assert_eq!(prior.deadline, snapshot.deadline);
    assert_eq!(prior.accepted, BusyIdleAcceptance::Busy);
}

#[test]
fn lifecycle_fencing_rejects_post_cancellation_state() {
    let mut workflow = workflow();
    let identity = WakeResourceIdentity::Bash { handle_id: "b-12" };
    let (decision, events) = registration_decision(
        Version(0),
        identity,
        Timestamp(20),
        BusyIdleAcceptance::Either,
    );
    workflow
        .commit_transition(&decision, &events)
        .expect("registration commit succeeds");

    let fence = lifecycle_fence(&workflow, REGISTRATION_EFFECT_ID).expect("fence exists");
    assert!(fence_accepts(&workflow, &fence));

    let request = cancellation_request(&workflow);
    workflow
        .cancel_with_compensation(&request, &BTreeMap::new())
        .expect("cancel succeeds");
    assert!(!fence_accepts(&workflow, &fence));
}

#[test]
fn shadow_selection_reports_parity_and_divergence() {
    let identity = WakeResourceIdentity::Tmux {
        session_id: "tmux-13",
    };
    let authoritative = super::TerminalWakeReceipt {
        identity,
        cause: TerminalWakeCause::Fired,
        observed_at: Timestamp(9),
    };
    let equal_shadow = authoritative.clone();
    let equal = shadow_parity(&authoritative, &equal_shadow);
    assert_eq!(equal.selected_cause, TerminalWakeCause::Fired);
    assert_eq!(equal.divergence_kind, None);

    let diverged_shadow = super::TerminalWakeReceipt {
        identity,
        cause: TerminalWakeCause::Expired,
        observed_at: Timestamp(10),
    };
    let diverged = shadow_parity(&authoritative, &diverged_shadow);
    assert_eq!(diverged.selected_cause, TerminalWakeCause::Fired);
    assert_eq!(
        diverged.divergence_kind,
        Some(crate::ShadowDivergenceKind::Receipt)
    );
}

#[test]
fn registration_barrier_receipt_round_trip_is_deterministic() {
    let mut workflow = workflow();
    let identity = WakeResourceIdentity::Bash { handle_id: "b-14" };
    let (decision, events) = registration_decision(
        Version(0),
        identity,
        Timestamp(20),
        BusyIdleAcceptance::Either,
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
            TerminalWakeEvidence::TmuxFinished {
                occurred_at: Timestamp(5)
            },
            true,
        ),
        AuthorityOutcome::Authorized
    );
    let receipt = receipt_from_evidence(
        identity,
        &TerminalWakeEvidence::TmuxFinished {
            occurred_at: Timestamp(5),
        },
        Timestamp(20),
    );
    let accepted = workflow.accept_receipt(
        &authority,
        Timestamp(5),
        Some(attempt.id),
        crate::ReceiptOrigin::Execution,
        receipt.clone(),
        receipt,
    );
    assert_eq!(accepted.outcome, AuthorityOutcome::Authorized);
    assert_eq!(accepted.reducer_events.len(), 1);
    let barrier = &accepted.reducer_events[0];
    assert_eq!(barrier.kind, ReducerInboxKind::BarrierSatisfied);
    assert!(matches!(
        barrier.payload,
        ReducerInboxPayload::Barrier(super::WakeBarrierEvent::RegistrationObserved { identity: found }) if found == identity
    ));
}

#[test]
fn helper_exports_are_shaped_as_expected() {
    let events = barrier_events();
    assert!(events.contains_key(&REGISTRATION_BARRIER_ID));
    let choices = manual_choices();
    assert_eq!(choices.len(), 3);
    assert!(choices
        .iter()
        .all(|choice| choice.codec.family == super::MANUAL_CODEC_FAMILY));
    let (decision, _) = registration_decision(
        Version(0),
        WakeResourceIdentity::Bash { handle_id: "b-15" },
        Timestamp(2),
        BusyIdleAcceptance::Either,
    );
    assert!(matches!(
        decision.plan.effects[0].intent,
        WakeRegistrationIntent::ObserveHandle { .. }
    ));
}
