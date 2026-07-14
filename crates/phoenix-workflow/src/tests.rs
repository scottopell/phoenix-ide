use std::collections::BTreeMap;

use proptest::prelude::*;

use crate::*;

#[derive(Debug, Clone, PartialEq, Eq)]
struct TestProfile;

impl WorkflowProfile for TestProfile {
    type Snapshot = &'static str;
    type Event = &'static str;
    type Intent = &'static str;
    type Observation = &'static str;
    type Receipt = &'static str;
    type ReceiptReducerEvent = &'static str;
    type BarrierEvent = &'static str;
    type OwedAcceptanceEvent = &'static str;
    type ManualPayload = &'static str;

    fn runtime_start_allowed(snapshot: &Self::Snapshot) -> bool {
        *snapshot != "runtime-blocked"
    }

    fn receipt_requires_runtime_acceptance(event: &Self::ReceiptReducerEvent) -> bool {
        *event != "reducer-only"
    }

    fn decision_handles_inbox(
        event: &ReducerInboxPayload<Self>,
        decision_event: &Self::Event,
    ) -> bool {
        !matches!(
            (event, *decision_event),
            (ReducerInboxPayload::Receipt("event-a"), "event-b")
        )
    }

    fn owed_acceptance_matches_inbox(
        event: &Self::OwedAcceptanceEvent,
        inbox_payload: &ReducerInboxPayload<Self>,
    ) -> bool {
        matches!(inbox_payload, ReducerInboxPayload::Receipt(payload) if payload == event)
    }

    fn decision_handles_owed_acceptance(
        event: &Self::OwedAcceptanceEvent,
        decision_event: &Self::Event,
    ) -> bool {
        !matches!((*event, *decision_event), ("event-a", "event-b"))
    }

    fn decision_handles_owed_acceptance_suppression(
        event: &Self::OwedAcceptanceEvent,
        decision_event: &Self::Event,
    ) -> bool {
        matches!(
            (*event, *decision_event),
            ("event-a", "suppress-event-a") | ("receipt-event", "suppress-receipt-event")
        )
    }
}

fn profile() -> ProfileRef {
    ProfileRef {
        profile_id: "test",
        protocol_version: 1,
    }
}

fn codec(name: &'static str) -> CodecRef {
    CodecRef {
        family: name,
        version: 1,
    }
}

fn protocol() -> ProtocolSelection {
    ProtocolSelection {
        profile: profile(),
        authority: SemanticAuthority::EngineProtocol,
        accepting: true,
        runtime_acceptance_enabled: true,
        external_acceptance_enabled: true,
        selector: "selector-v1",
    }
}

fn legacy_protocol() -> ProtocolSelection {
    let mut selection = protocol();
    selection.authority = SemanticAuthority::LegacyProtocol;
    selection
}

fn effect(effect_id: u64, role: EffectRole, generation: Generation) -> EffectDecl<&'static str> {
    effect_with_ambiguity(
        effect_id,
        role,
        generation,
        EffectAmbiguity::SafeRepeatability,
    )
}

fn effect_with_ambiguity(
    effect_id: u64,
    role: EffectRole,
    generation: Generation,
    ambiguity: EffectAmbiguity,
) -> EffectDecl<&'static str> {
    EffectDecl {
        effect_id: EffectId(effect_id),
        family: "test",
        kind: "step",
        codec: codec("intent"),
        generation,
        role,
        ambiguity,
        intent: "intent",
        next_eligible_at: None,
        destructive_resource: None,
    }
}

fn plan() -> TransitionPlan<TestProfile> {
    TransitionPlan {
        next_status: WorkflowStatus::Active,
        snapshot: "next",
        snapshot_codec: codec("snapshot"),
        event: "evt",
        event_codec: codec("event"),
        effects: vec![
            effect(1, EffectRole::Required, Generation(0)),
            effect(2, EffectRole::Required, Generation(0)),
        ],
        dependencies: vec![DependencyDecl {
            effect_id: EffectId(2),
            depends_on_effect_id: EffectId(1),
        }],
        barriers: vec![BarrierDecl {
            barrier_id: BarrierId(10),
            reducer_event_codec: codec("barrier"),
        }],
        barrier_members: vec![
            BarrierMemberDecl {
                barrier_id: BarrierId(10),
                effect_id: EffectId(1),
                receipt_family: ReceiptFamily::CurrentGenerationEffect,
            },
            BarrierMemberDecl {
                barrier_id: BarrierId(10),
                effect_id: EffectId(2),
                receipt_family: ReceiptFamily::CurrentGenerationEffect,
            },
        ],
        invalidations: vec![],
        owed_acceptances: None,
    }
}

fn barrier_events() -> BTreeMap<BarrierId, &'static str> {
    BTreeMap::from([(BarrierId(10), "barrier-event")])
}

fn workflow() -> WorkflowState<TestProfile> {
    let profile = profile();
    WorkflowState::<TestProfile>::new_authoritative(
        WorkflowId(1),
        &profile,
        &protocol(),
        codec("snapshot"),
        "initial",
    )
    .expect("accepting protocol")
}

#[test]
fn validates_happy_path_plan() {
    assert_eq!(
        validate_plan(WorkflowStatus::Active, &plan(), &barrier_events()),
        Ok(())
    );
}

#[test]
fn rejects_duplicate_effect_ids() {
    let mut plan = plan();
    plan.effects
        .push(effect(1, EffectRole::Required, Generation(0)));
    assert_eq!(
        validate_plan(WorkflowStatus::Active, &plan, &barrier_events()),
        Err(PlanError::DuplicateEffectId(EffectId(1)))
    );
}

#[test]
fn rejects_dependency_cycles() {
    let mut plan = plan();
    plan.dependencies.push(DependencyDecl {
        effect_id: EffectId(1),
        depends_on_effect_id: EffectId(2),
    });
    assert_eq!(
        validate_plan(WorkflowStatus::Active, &plan, &barrier_events()),
        Err(PlanError::DependencyCycle)
    );
}

#[test]
fn external_acceptance_replays_same_receipt_and_rejects_conflicting_intent() {
    let selection = protocol();
    let mut registry = ExternalAcceptanceRegistry::new();
    let first = registry.accept(
        &selection,
        "account:a",
        "client-key",
        "intent:v1",
        WorkflowId(10),
        "conversation-10",
    );
    let ExternalAcceptanceOutcome::New(first_receipt) = first else {
        panic!("first acceptance must be new");
    };
    let replay = registry.accept(
        &selection,
        "account:a",
        "client-key",
        "intent:v1",
        WorkflowId(99),
        "conversation-99",
    );
    assert_eq!(replay, ExternalAcceptanceOutcome::Replay(first_receipt));
    for distinct_selection in [
        ProtocolSelection {
            selector: "selector-v2",
            ..selection.clone()
        },
        ProtocolSelection {
            authority: SemanticAuthority::LegacyProtocol,
            ..selection.clone()
        },
        ProtocolSelection {
            profile: ProfileRef {
                profile_id: "test",
                protocol_version: 2,
            },
            ..selection.clone()
        },
    ] {
        assert!(matches!(
            registry.accept(
                &distinct_selection,
                "account:a",
                "client-key",
                "intent:v1",
                WorkflowId(99),
                "conversation-99",
            ),
            ExternalAcceptanceOutcome::New(_)
        ));
    }
    assert_eq!(
        registry.accept(
            &selection,
            "account:a",
            "client-key",
            "intent:v2",
            WorkflowId(11),
            "conversation-11",
        ),
        ExternalAcceptanceOutcome::Conflict
    );
}

#[test]
fn external_acceptance_key_is_scoped_and_capability_is_structural() {
    let selection = protocol();
    let mut registry = ExternalAcceptanceRegistry::new();
    assert!(matches!(
        registry.accept(
            &selection,
            "account:a",
            "same-key",
            "intent",
            WorkflowId(1),
            "conversation-a",
        ),
        ExternalAcceptanceOutcome::New(_)
    ));
    assert!(matches!(
        registry.accept(
            &selection,
            "account:b",
            "same-key",
            "intent",
            WorkflowId(2),
            "conversation-b",
        ),
        ExternalAcceptanceOutcome::New(_)
    ));

    let mut unsupported = protocol();
    unsupported.external_acceptance_enabled = false;
    let mut unsupported_registry = ExternalAcceptanceRegistry::new();
    assert_eq!(
        unsupported_registry.accept(
            &unsupported,
            "account:a",
            "key",
            "intent",
            WorkflowId(3),
            "conversation-c",
        ),
        ExternalAcceptanceOutcome::Unsupported
    );
    assert!(unsupported_registry.is_empty());
}

#[test]
fn closed_protocol_cannot_accept_authoritative_or_shadow_workflows() {
    let mut closed = protocol();
    closed.accepting = false;
    assert_eq!(
        WorkflowState::<TestProfile>::new_authoritative(
            WorkflowId(1),
            &profile(),
            &closed,
            codec("snapshot"),
            "initial",
        ),
        Err(EngineError::ProtocolNotAccepting)
    );
    assert_eq!(
        WorkflowState::<TestProfile>::new_shadow(
            WorkflowId(2),
            WorkflowId(1),
            &profile(),
            &closed,
            codec("snapshot"),
            "initial",
        ),
        Err(EngineError::ProtocolNotAccepting)
    );
}

#[test]
fn stale_cas_does_not_mutate_state() {
    let mut workflow = workflow();
    let result = workflow
        .commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(7),
                plan: plan(),
            },
            &barrier_events(),
        )
        .expect("cas handled");
    assert_eq!(result.outcome, CommitOutcome::VersionConflict);
    assert!(workflow.effects.is_empty());
}

#[test]
fn shadow_workflow_cannot_execute_or_claim() {
    let profile = profile();
    let mut workflow = WorkflowState::<TestProfile>::new_shadow(
        WorkflowId(2),
        WorkflowId(1),
        &profile,
        &protocol(),
        codec("snapshot"),
        "initial",
    )
    .expect("accepting protocol");
    assert_eq!(
        workflow.commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(0),
                plan: plan()
            },
            &barrier_events()
        ),
        Err(EngineError::ShadowCannotExecute)
    );
    assert_eq!(
        workflow
            .claim_effect(EffectId(1), "worker-a", Timestamp(0), LeaseExpiry(10))
            .outcome,
        ClaimOutcome::AuthorityConflict
    );
}

#[test]
fn terminal_workflow_rejects_claim_even_when_effect_remains_eligible() {
    for terminal_status in [WorkflowStatus::Completed, WorkflowStatus::Failed] {
        let mut workflow = workflow();
        workflow
            .commit_transition(
                &ReducerDecision {
                    expected_workflow_version: Version(0),
                    plan: plan(),
                },
                &barrier_events(),
            )
            .expect("effect plan commits");
        assert_eq!(
            workflow.effects[&EffectId(1)].status,
            EffectStatus::Eligible
        );
        workflow
            .commit_transition(
                &ReducerDecision {
                    expected_workflow_version: Version(1),
                    plan: TransitionPlan {
                        next_status: terminal_status,
                        snapshot: "terminal",
                        snapshot_codec: codec("snapshot"),
                        event: "terminal",
                        event_codec: codec("event"),
                        effects: vec![],
                        dependencies: vec![],
                        barriers: vec![],
                        barrier_members: vec![],
                        invalidations: vec![],
                        owed_acceptances: None,
                    },
                },
                &BTreeMap::new(),
            )
            .expect("terminal transition commits");

        assert_eq!(
            workflow
                .claim_effect(EffectId(1), "worker-a", Timestamp(0), LeaseExpiry(10))
                .outcome,
            ClaimOutcome::Ineligible
        );
        assert!(workflow.effects[&EffectId(1)].attempts.is_empty());
    }
}

#[test]
fn claim_requires_future_lease_and_takeover_requires_expiry() {
    let mut workflow = workflow();
    workflow
        .commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(0),
                plan: plan(),
            },
            &barrier_events(),
        )
        .expect("commit succeeds");
    assert_eq!(
        workflow
            .claim_effect(EffectId(1), "worker-a", Timestamp(5), LeaseExpiry(5))
            .outcome,
        ClaimOutcome::Ineligible
    );
    let claim = workflow.claim_effect(EffectId(1), "worker-a", Timestamp(0), LeaseExpiry(10));
    let authority = claim.authority.expect("authority issued");
    assert_eq!(
        workflow
            .take_over_expired_claim(
                EffectId(1),
                &authority,
                "worker-b",
                Timestamp(9),
                LeaseExpiry(20)
            )
            .outcome,
        ClaimOutcome::AuthorityConflict
    );
    assert_eq!(
        workflow
            .take_over_expired_claim(
                EffectId(1),
                &authority,
                "worker-b",
                Timestamp(10),
                LeaseExpiry(20)
            )
            .outcome,
        ClaimOutcome::Claimed
    );
    assert!(workflow.effects[&EffectId(1)].pending_reconciliation);
}

#[test]
fn observations_and_receipts_require_matching_attempt() {
    let mut workflow = workflow();
    workflow
        .commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(0),
                plan: plan(),
            },
            &barrier_events(),
        )
        .expect("commit succeeds");
    let claim = workflow.claim_effect(EffectId(1), "worker-a", Timestamp(0), LeaseExpiry(10));
    let authority = claim.authority.expect("authority issued");
    let attempt = claim.attempt.expect("attempt created");
    assert_eq!(
        workflow.record_observation(
            &authority,
            Timestamp(1),
            AttemptId(999),
            codec("observation"),
            "saw"
        ),
        AuthorityOutcome::StaleAuthority
    );
    assert_eq!(
        workflow.record_observation(
            &authority,
            Timestamp(1),
            attempt.id,
            codec("observation"),
            "saw"
        ),
        AuthorityOutcome::Authorized
    );
    let accepted = workflow.accept_receipt(
        &authority,
        Timestamp(2),
        Some(AttemptId(999)),
        ReceiptOrigin::Execution,
        codec("receipt"),
        "done",
        codec("receipt-event"),
        "receipt-event",
    );
    assert_eq!(accepted.outcome, AuthorityOutcome::StaleAuthority);
}

#[test]
fn receipt_and_reducer_event_codecs_are_persisted_independently() {
    let mut workflow = workflow();
    workflow
        .commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(0),
                plan: plan(),
            },
            &barrier_events(),
        )
        .expect("commit succeeds");
    let claim = workflow.claim_effect(EffectId(1), "worker-a", Timestamp(0), LeaseExpiry(10));
    let authority = claim.authority.expect("authority issued");
    let attempt = claim.attempt.expect("attempt created");
    let accepted = workflow.accept_receipt(
        &authority,
        Timestamp(1),
        Some(attempt.id),
        ReceiptOrigin::Execution,
        codec("receipt"),
        "done",
        codec("receipt-event"),
        "receipt-event",
    );

    assert_eq!(
        accepted.receipt.expect("receipt persisted").receipt_codec,
        codec("receipt")
    );
    assert_eq!(
        workflow.reducer_inbox[&accepted.receipt_inbox_ids[0]].event_codec,
        codec("receipt-event")
    );
}

#[test]
fn stale_observations_are_retained_diagnostically() {
    let mut workflow = workflow();
    workflow
        .commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(0),
                plan: plan(),
            },
            &barrier_events(),
        )
        .expect("commit succeeds");
    let claim = workflow.claim_effect(EffectId(1), "worker-a", Timestamp(0), LeaseExpiry(1));
    let authority = claim.authority.expect("authority issued");
    let attempt = claim.attempt.expect("attempt created");
    assert_eq!(
        workflow.record_observation(
            &authority,
            Timestamp(2),
            attempt.id,
            codec("observation"),
            "late"
        ),
        AuthorityOutcome::StaleAuthority
    );
    assert_eq!(workflow.effects[&EffectId(1)].stale_observations.len(), 1);
}

#[test]
fn retry_wait_becomes_eligible_only_after_deadline() {
    let mut workflow = workflow();
    workflow
        .commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(0),
                plan: plan(),
            },
            &barrier_events(),
        )
        .expect("commit succeeds");
    let claim = workflow.claim_effect(EffectId(1), "worker-a", Timestamp(0), LeaseExpiry(10));
    let authority = claim.authority.expect("authority issued");
    let _ = workflow.schedule_retry(&authority, Timestamp(1), Timestamp(5));
    assert_eq!(
        workflow.effects[&EffectId(1)].status,
        EffectStatus::RetryWait
    );
    workflow.refresh_eligibility(Timestamp(4));
    assert_eq!(
        workflow.effects[&EffectId(1)].status,
        EffectStatus::RetryWait
    );
    workflow.refresh_eligibility(Timestamp(5));
    assert_eq!(
        workflow.effects[&EffectId(1)].status,
        EffectStatus::Eligible
    );
}

#[test]
fn barrier_satisfaction_returns_all_events() {
    let mut workflow = workflow();
    workflow
        .commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(0),
                plan: plan(),
            },
            &barrier_events(),
        )
        .expect("commit succeeds");
    let first = workflow.claim_effect(EffectId(1), "worker-a", Timestamp(0), LeaseExpiry(10));
    let first_authority = first.authority.expect("authority issued");
    let first_attempt = first.attempt.expect("attempt created");
    let _ = workflow.accept_receipt(
        &first_authority,
        Timestamp(1),
        Some(first_attempt.id),
        ReceiptOrigin::Execution,
        codec("receipt"),
        "done-1",
        codec("receipt-event"),
        "receipt-event-1",
    );
    let second = workflow.claim_effect(EffectId(2), "worker-b", Timestamp(1), LeaseExpiry(10));
    let second_authority = second.authority.expect("authority issued");
    let second_attempt = second.attempt.expect("attempt created");
    let accepted = workflow.accept_receipt(
        &second_authority,
        Timestamp(2),
        Some(second_attempt.id),
        ReceiptOrigin::Execution,
        codec("receipt"),
        "done-2",
        codec("receipt-event"),
        "receipt-event-2",
    );
    assert_eq!(accepted.reducer_events.len(), 1);
    assert_eq!(accepted.reducer_events[0].barrier_id, Some(BarrierId(10)));
    assert!(matches!(
        accepted.reducer_events[0].payload,
        ReducerInboxPayload::Barrier("barrier-event")
    ));
}

#[test]
#[allow(clippy::too_many_lines)]
fn manual_resolution_requires_permitted_choice_and_cas() {
    let mut workflow = workflow();
    workflow
        .commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(0),
                plan: plan(),
            },
            &barrier_events(),
        )
        .expect("commit succeeds");
    let claim = workflow.claim_effect(EffectId(1), "worker-a", Timestamp(0), LeaseExpiry(10));
    let authority = claim.authority.expect("authority issued");
    let resolution = workflow
        .require_manual_resolution(
            &authority,
            Timestamp(1),
            vec![ManualChoice {
                kind: ManualChoiceKind::Adopt,
                codec: codec("manual"),
                payload: "adopt",
                receipt_codec: codec("receipt"),
                receipt: "receipt",
                receipt_event_codec: codec("receipt-event"),
                receipt_event: "manual-receipt-event",
            }],
        )
        .manual_resolution
        .expect("resolution persisted");
    let stale = workflow.resolve_manual(
        resolution.id,
        Version(99),
        "operator-a",
        &ManualChoice {
            kind: ManualChoiceKind::Adopt,
            codec: codec("manual"),
            payload: "adopt",
            receipt_codec: codec("receipt"),
            receipt: "receipt",
            receipt_event_codec: codec("receipt-event"),
            receipt_event: "manual-receipt-event",
        },
        ManualResolutionCommit {
            transition_codec: codec("manual-transition"),
            transition_event: "manual-transition",
            next_status: WorkflowStatus::Active,
        },
    );
    assert_eq!(stale.outcome, CommitOutcome::VersionConflict);
    let invalid = workflow.resolve_manual(
        resolution.id,
        Version(1),
        "operator-a",
        &ManualChoice {
            kind: ManualChoiceKind::Retry,
            codec: codec("manual"),
            payload: "retry",
            receipt_codec: codec("receipt"),
            receipt: "retry-receipt",
            receipt_event_codec: codec("receipt-event"),
            receipt_event: "retry-receipt-event",
        },
        ManualResolutionCommit {
            transition_codec: codec("manual-transition"),
            transition_event: "manual-transition",
            next_status: WorkflowStatus::Active,
        },
    );
    assert_eq!(invalid.outcome, CommitOutcome::InvalidPlan);

    let committed = workflow.resolve_manual(
        resolution.id,
        Version(1),
        "operator-a",
        &ManualChoice {
            kind: ManualChoiceKind::Adopt,
            codec: codec("manual"),
            payload: "adopt",
            receipt_codec: codec("receipt"),
            receipt: "receipt",
            receipt_event_codec: codec("receipt-event"),
            receipt_event: "manual-receipt-event",
        },
        ManualResolutionCommit {
            transition_codec: codec("manual-transition"),
            transition_event: "manual-transition",
            next_status: WorkflowStatus::Active,
        },
    );
    assert_eq!(committed.outcome, CommitOutcome::Committed);
    let ManualEffectOutcome::Receipt { receipt, .. } =
        committed.effect_outcome.expect("manual outcome")
    else {
        panic!("adoption must produce a receipt");
    };
    assert_eq!(receipt.origin, ReceiptOrigin::Manual);
    assert_eq!(receipt.authority.worker_id, "manual");
    assert_eq!(receipt.authority.declared_workflow_version, Version(1));
    assert_eq!(receipt.receipt, "receipt");
    assert_eq!(receipt.receipt_codec, codec("receipt"));
    let inbox = workflow
        .reducer_inbox
        .values()
        .find(|event| event.effect_id == Some(EffectId(1)))
        .expect("manual receipt inbox recorded");
    assert!(matches!(
        inbox.payload,
        ReducerInboxPayload::Receipt("manual-receipt-event")
    ));
    assert_eq!(workflow.version, Version(2));
}

#[test]
fn manual_resolution_version_has_matching_transition_history() {
    let mut workflow = workflow();
    workflow
        .commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(0),
                plan: plan(),
            },
            &barrier_events(),
        )
        .expect("commit succeeds");
    let claim = workflow.claim_effect(EffectId(1), "worker-a", Timestamp(0), LeaseExpiry(10));
    let authority = claim.authority.expect("authority issued");
    let resolution = workflow
        .require_manual_resolution(
            &authority,
            Timestamp(1),
            vec![ManualChoice {
                kind: ManualChoiceKind::Adopt,
                codec: codec("manual"),
                payload: "adopt",
                receipt_codec: codec("receipt"),
                receipt: "receipt",
                receipt_event_codec: codec("receipt-event"),
                receipt_event: "manual-receipt-event",
            }],
        )
        .manual_resolution
        .expect("resolution persisted");
    let before = workflow.transition_log.len();
    let outcome = workflow.resolve_manual(
        resolution.id,
        Version(1),
        "operator-a",
        &ManualChoice {
            kind: ManualChoiceKind::Adopt,
            codec: codec("manual"),
            payload: "adopt",
            receipt_codec: codec("receipt"),
            receipt: "receipt",
            receipt_event_codec: codec("receipt-event"),
            receipt_event: "manual-receipt-event",
        },
        ManualResolutionCommit {
            transition_codec: codec("manual-transition"),
            transition_event: "manual-transition",
            next_status: WorkflowStatus::Active,
        },
    );
    assert_eq!(outcome.outcome, CommitOutcome::Committed);
    assert_eq!(workflow.transition_log.len(), before + 1);
    let transition = workflow.transition_log.last().expect("transition appended");
    assert_eq!(transition.from_version, Version(1));
    assert_eq!(transition.to_version, Version(2));
    assert_eq!(transition.event, "manual-transition");
}

#[test]
#[allow(clippy::too_many_lines)]
fn owed_acceptance_requires_exact_consumed_inbox_link() {
    let mut state = workflow();
    state
        .commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(0),
                plan: plan(),
            },
            &barrier_events(),
        )
        .expect("commit succeeds");
    let claim = state.claim_effect(EffectId(1), "worker-a", Timestamp(0), LeaseExpiry(10));
    let authority = claim.authority.expect("authority issued");
    let attempt = claim.attempt.expect("attempt created");
    let accepted = state.accept_receipt(
        &authority,
        Timestamp(1),
        Some(attempt.id),
        ReceiptOrigin::Execution,
        codec("receipt"),
        "done",
        codec("receipt-event"),
        "receipt-event",
    );
    let inbox_id = accepted.receipt_inbox_ids[0];
    let mut consume_plan = TransitionPlan {
        next_status: WorkflowStatus::Active,
        snapshot: "accepted",
        snapshot_codec: codec("snapshot"),
        event: "consume",
        event_codec: codec("event"),
        effects: vec![],
        dependencies: vec![],
        barriers: vec![],
        barrier_members: vec![],
        invalidations: vec![],
        owed_acceptances: Some(vec![OwedAcceptanceDecl {
            reducer_inbox_id: inbox_id,
            source_kind: "wake",
            event_codec: codec("receipt-event"),
            event: "receipt-event",
        }]),
    };
    let result = state
        .consume_reducer_inbox_atomically(
            &InboxDecisionBinding {
                inbox: [inbox_id]
                    .iter()
                    .map(|id| state.reducer_inbox[id].clone())
                    .collect(),
                decision: ReducerDecision {
                    expected_workflow_version: Version(1),
                    plan: consume_plan.clone(),
                },
            },
            &BTreeMap::new(),
        )
        .expect("consume succeeds");
    assert_eq!(result.outcome, CommitOutcome::Committed);
    let owed = state
        .owed_acceptances
        .values()
        .next()
        .expect("owed created");
    assert_eq!(owed.reducer_inbox_id, inbox_id);

    let mut payload_mismatch = workflow();
    payload_mismatch.reducer_inbox.insert(
        ReducerInboxId(88),
        ReducerInboxEvent {
            id: ReducerInboxId(88),
            effect_id: None,
            barrier_id: None,
            kind: ReducerInboxKind::ReceiptAccepted,
            event_codec: codec("terminal"),
            requires_runtime_acceptance: true,
            payload: ReducerInboxPayload::Receipt("terminal-a"),
            delivery_status: DeliveryStatus::Pending,
            consumed_by: None,
        },
    );
    let linked = payload_mismatch.reducer_inbox[&ReducerInboxId(88)].clone();
    let rejected = payload_mismatch.consume_reducer_inbox_atomically(
        &InboxDecisionBinding {
            inbox: vec![linked],
            decision: ReducerDecision {
                expected_workflow_version: Version(0),
                plan: TransitionPlan {
                    next_status: WorkflowStatus::Active,
                    snapshot: "wrong-terminal",
                    snapshot_codec: codec("snapshot"),
                    event: "consume",
                    event_codec: codec("event"),
                    effects: vec![],
                    dependencies: vec![],
                    barriers: vec![],
                    barrier_members: vec![],
                    invalidations: vec![],
                    owed_acceptances: Some(vec![OwedAcceptanceDecl {
                        reducer_inbox_id: ReducerInboxId(88),
                        source_kind: "wake",
                        event_codec: codec("terminal"),
                        event: "terminal-b",
                    }]),
                },
            },
        },
        &BTreeMap::new(),
    );
    assert_eq!(rejected, Err(EngineError::InvalidInbox));
    assert_eq!(payload_mismatch.version, Version(0));
    assert_eq!(
        payload_mismatch.reducer_inbox[&ReducerInboxId(88)].delivery_status,
        DeliveryStatus::Pending
    );

    consume_plan.owed_acceptances = Some(vec![OwedAcceptanceDecl {
        reducer_inbox_id: ReducerInboxId(999),
        source_kind: "wake",
        event_codec: codec("runtime-event"),
        event: "runtime-event",
    }]);
    let rejected = state.consume_reducer_inbox_atomically(
        &InboxDecisionBinding {
            inbox: []
                .iter()
                .map(|id| state.reducer_inbox[id].clone())
                .collect(),
            decision: ReducerDecision {
                expected_workflow_version: Version(2),
                plan: consume_plan,
            },
        },
        &BTreeMap::new(),
    );
    assert_eq!(rejected, Err(EngineError::InvalidInbox));
}

#[test]
fn cancellation_bumps_generation_and_revokes_prior_claims() {
    let mut workflow = workflow();
    workflow
        .commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(0),
                plan: plan(),
            },
            &barrier_events(),
        )
        .expect("commit succeeds");
    let claim = workflow.claim_effect(EffectId(1), "worker-a", Timestamp(0), LeaseExpiry(10));
    let authority = claim.authority.expect("authority issued");
    let compensation_plan = TransitionPlan {
        next_status: WorkflowStatus::Active,
        snapshot: "cancelled",
        snapshot_codec: codec("snapshot"),
        event: "cancel",
        event_codec: codec("event"),
        effects: vec![effect(20, EffectRole::Compensation, Generation(1))],
        dependencies: vec![],
        barriers: vec![BarrierDecl {
            barrier_id: BarrierId(11),
            reducer_event_codec: codec("barrier"),
        }],
        barrier_members: vec![BarrierMemberDecl {
            barrier_id: BarrierId(11),
            effect_id: EffectId(20),
            receipt_family: ReceiptFamily::CompensationEffect,
        }],
        invalidations: vec![],
        owed_acceptances: None,
    };
    let cancel_events = BTreeMap::from([(BarrierId(11), "compensated")]);
    let _ = workflow
        .cancel_with_compensation(
            &CancellationRequest {
                expected_workflow_version: Version(1),
                next_snapshot: "cancelled",
                next_snapshot_codec: codec("snapshot"),
                event: "cancel",
                event_codec: codec("event"),
                invalidations: vec![EffectInvalidationDecl {
                    effect_id: EffectId(1),
                }],
                reducer_inbox_events: vec![],
                compensation_plan,
            },
            &cancel_events,
        )
        .expect("cancel succeeds");
    assert_eq!(workflow.generation, Generation(1));
    assert_eq!(
        workflow
            .renew_claim(&authority, Timestamp(1), LeaseExpiry(30))
            .outcome,
        AuthorityOutcome::StaleAuthority
    );
}

#[test]
fn invalid_commit_plan_does_not_mutate_state() {
    let mut workflow = workflow();
    let before = workflow.clone();
    let mut invalid_plan = plan();
    invalid_plan.barrier_members.clear();
    let result = workflow.commit_transition(
        &ReducerDecision {
            expected_workflow_version: Version(0),
            plan: invalid_plan,
        },
        &barrier_events(),
    );
    assert_eq!(
        result,
        Err(EngineError::InvalidPlan(PlanError::BarrierHasNoMembers(
            BarrierId(10)
        )))
    );
    assert_eq!(workflow, before);
}

#[test]
fn invalid_cancellation_plan_does_not_mutate_state() {
    let mut workflow = workflow();
    workflow
        .commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(0),
                plan: plan(),
            },
            &barrier_events(),
        )
        .expect("commit succeeds");
    let before = workflow.clone();
    let result = workflow.cancel_with_compensation(
        &CancellationRequest {
            expected_workflow_version: Version(1),
            next_snapshot: "cancelled",
            next_snapshot_codec: codec("snapshot"),
            event: "cancel",
            event_codec: codec("event"),
            invalidations: vec![EffectInvalidationDecl {
                effect_id: EffectId(1),
            }],
            reducer_inbox_events: vec![],
            compensation_plan: TransitionPlan {
                next_status: WorkflowStatus::Active,
                snapshot: "cancelled",
                snapshot_codec: codec("snapshot"),
                event: "cancel",
                event_codec: codec("event"),
                effects: vec![effect(20, EffectRole::Compensation, Generation(1))],
                dependencies: vec![],
                barriers: vec![BarrierDecl {
                    barrier_id: BarrierId(11),
                    reducer_event_codec: codec("barrier"),
                }],
                barrier_members: vec![],
                invalidations: vec![],
                owed_acceptances: None,
            },
        },
        &BTreeMap::from([(BarrierId(11), "compensated")]),
    );
    assert_eq!(
        result,
        Err(EngineError::InvalidPlan(PlanError::BarrierHasNoMembers(
            BarrierId(11)
        )))
    );
    assert_eq!(workflow, before);
}

#[test]
fn stale_cancellation_cas_does_not_mutate_state() {
    let mut workflow = workflow();
    workflow
        .commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(0),
                plan: plan(),
            },
            &barrier_events(),
        )
        .expect("commit succeeds");
    let before = workflow.clone();
    let result = workflow
        .cancel_with_compensation(
            &CancellationRequest {
                expected_workflow_version: Version(99),
                next_snapshot: "cancelled",
                next_snapshot_codec: codec("snapshot"),
                event: "cancel",
                event_codec: codec("event"),
                invalidations: vec![EffectInvalidationDecl {
                    effect_id: EffectId(1),
                }],
                reducer_inbox_events: vec![],
                compensation_plan: TransitionPlan {
                    next_status: WorkflowStatus::Active,
                    snapshot: "cancelled",
                    snapshot_codec: codec("snapshot"),
                    event: "cancel",
                    event_codec: codec("event"),
                    effects: vec![effect(20, EffectRole::Compensation, Generation(1))],
                    dependencies: vec![],
                    barriers: vec![BarrierDecl {
                        barrier_id: BarrierId(11),
                        reducer_event_codec: codec("barrier"),
                    }],
                    barrier_members: vec![BarrierMemberDecl {
                        barrier_id: BarrierId(11),
                        effect_id: EffectId(20),
                        receipt_family: ReceiptFamily::CompensationEffect,
                    }],
                    invalidations: vec![],
                    owed_acceptances: None,
                },
            },
            &BTreeMap::from([(BarrierId(11), "compensated")]),
        )
        .expect("cancellation returns cas outcome");
    assert_eq!(result.outcome, CommitOutcome::VersionConflict);
    assert_eq!(workflow, before);
}

#[test]
fn invalid_manual_resolution_does_not_mutate_state() {
    let mut workflow = workflow();
    workflow
        .commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(0),
                plan: plan(),
            },
            &barrier_events(),
        )
        .expect("commit succeeds");
    let claim = workflow.claim_effect(EffectId(1), "worker-a", Timestamp(0), LeaseExpiry(10));
    let authority = claim.authority.expect("authority issued");
    let resolution = workflow
        .require_manual_resolution(
            &authority,
            Timestamp(1),
            vec![ManualChoice {
                kind: ManualChoiceKind::Adopt,
                codec: codec("manual"),
                payload: "adopt",
                receipt_codec: codec("receipt"),
                receipt: "receipt",
                receipt_event_codec: codec("receipt-event"),
                receipt_event: "manual-receipt-event",
            }],
        )
        .manual_resolution
        .expect("resolution persisted");
    let before = workflow.clone();
    let outcome = workflow.resolve_manual(
        resolution.id,
        Version(1),
        "operator-a",
        &ManualChoice {
            kind: ManualChoiceKind::Retry,
            codec: codec("manual"),
            payload: "retry",
            receipt_codec: codec("receipt"),
            receipt: "retry-receipt",
            receipt_event_codec: codec("receipt-event"),
            receipt_event: "retry-receipt-event",
        },
        ManualResolutionCommit {
            transition_codec: codec("manual-transition"),
            transition_event: "manual-transition",
            next_status: WorkflowStatus::Active,
        },
    );
    assert_eq!(outcome.outcome, CommitOutcome::InvalidPlan);
    assert_eq!(workflow, before);
}

#[test]
fn claim_rejects_pending_reconciliation_and_preserves_flag() {
    let mut workflow = workflow();
    workflow
        .commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(0),
                plan: plan(),
            },
            &barrier_events(),
        )
        .expect("commit succeeds");
    let claim = workflow.claim_effect(EffectId(1), "worker-a", Timestamp(0), LeaseExpiry(10));
    let authority = claim.authority.expect("authority issued");
    assert_eq!(
        workflow
            .take_over_expired_claim(
                EffectId(1),
                &authority,
                "worker-b",
                Timestamp(10),
                LeaseExpiry(20)
            )
            .outcome,
        ClaimOutcome::Claimed
    );
    assert!(workflow.effects[&EffectId(1)].pending_reconciliation);
    let retry_claim =
        workflow.claim_effect(EffectId(1), "worker-b", Timestamp(10), LeaseExpiry(20));
    assert_eq!(retry_claim.outcome, ClaimOutcome::Ineligible);
    assert!(workflow.effects[&EffectId(1)].pending_reconciliation);
}

#[test]
fn manual_accept_receipt_is_rejected_but_manual_resolution_still_persists_manual_origin() {
    let mut workflow = workflow();
    workflow
        .commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(0),
                plan: plan(),
            },
            &barrier_events(),
        )
        .expect("commit succeeds");
    let claim = workflow.claim_effect(EffectId(1), "worker-a", Timestamp(0), LeaseExpiry(10));
    let authority = claim.authority.expect("authority issued");
    let attempt = claim.attempt.expect("attempt created");
    let rejected = workflow.accept_receipt(
        &authority,
        Timestamp(1),
        Some(attempt.id),
        ReceiptOrigin::Manual,
        codec("receipt"),
        "receipt",
        codec("receipt-event"),
        "manual-receipt-event",
    );
    assert_eq!(rejected.outcome, AuthorityOutcome::StaleAuthority);
    assert!(workflow.effects[&EffectId(1)].receipt.is_none());

    let resolution = workflow
        .require_manual_resolution(
            &authority,
            Timestamp(1),
            vec![ManualChoice {
                kind: ManualChoiceKind::Adopt,
                codec: codec("manual"),
                payload: "adopt",
                receipt_codec: codec("receipt"),
                receipt: "receipt",
                receipt_event_codec: codec("receipt-event"),
                receipt_event: "manual-receipt-event",
            }],
        )
        .manual_resolution
        .expect("resolution persisted");
    let committed = workflow.resolve_manual(
        resolution.id,
        Version(1),
        "operator-a",
        &ManualChoice {
            kind: ManualChoiceKind::Adopt,
            codec: codec("manual"),
            payload: "adopt",
            receipt_codec: codec("receipt"),
            receipt: "receipt",
            receipt_event_codec: codec("receipt-event"),
            receipt_event: "manual-receipt-event",
        },
        ManualResolutionCommit {
            transition_codec: codec("manual-transition"),
            transition_event: "manual-transition",
            next_status: WorkflowStatus::Active,
        },
    );
    assert_eq!(
        match committed.effect_outcome.expect("manual outcome") {
            ManualEffectOutcome::Receipt { receipt, .. } => receipt.origin,
            outcome @ (ManualEffectOutcome::Retry
            | ManualEffectOutcome::Compensate
            | ManualEffectOutcome::Failed
            | ManualEffectOutcome::Suppressed) => {
                panic!("unexpected manual outcome: {outcome:?}")
            }
        },
        ReceiptOrigin::Manual
    );
}

#[test]
fn runtime_acceptance_requires_exact_one_to_one_inbox_mapping() {
    let mut workflow = workflow();
    workflow
        .commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(0),
                plan: plan(),
            },
            &barrier_events(),
        )
        .expect("commit succeeds");
    let claim = workflow.claim_effect(EffectId(1), "worker-a", Timestamp(0), LeaseExpiry(10));
    let authority = claim.authority.expect("authority issued");
    let attempt = claim.attempt.expect("attempt created");
    let accepted = workflow.accept_receipt(
        &authority,
        Timestamp(1),
        Some(attempt.id),
        ReceiptOrigin::Execution,
        codec("receipt"),
        "done",
        codec("receipt-event"),
        "receipt-event",
    );
    let inbox_id = accepted.receipt_inbox_ids[0];

    let duplicate = workflow.consume_reducer_inbox_atomically(
        &InboxDecisionBinding {
            inbox: [inbox_id, inbox_id]
                .iter()
                .map(|id| workflow.reducer_inbox[id].clone())
                .collect(),
            decision: ReducerDecision {
                expected_workflow_version: Version(1),
                plan: TransitionPlan {
                    next_status: WorkflowStatus::Active,
                    snapshot: "accepted",
                    snapshot_codec: codec("snapshot"),
                    event: "consume",
                    event_codec: codec("event"),
                    effects: vec![],
                    dependencies: vec![],
                    barriers: vec![],
                    barrier_members: vec![],
                    invalidations: vec![],
                    owed_acceptances: Some(vec![
                        OwedAcceptanceDecl {
                            reducer_inbox_id: inbox_id,
                            source_kind: "wake",
                            event_codec: codec("runtime-event"),
                            event: "runtime-event-a",
                        },
                        OwedAcceptanceDecl {
                            reducer_inbox_id: inbox_id,
                            source_kind: "wake",
                            event_codec: codec("runtime-event"),
                            event: "runtime-event-b",
                        },
                    ]),
                },
            },
        },
        &BTreeMap::new(),
    );
    assert_eq!(duplicate, Err(EngineError::InvalidInbox));
    assert_eq!(workflow.version, Version(1));
    assert!(workflow.owed_acceptances.is_empty());

    let missing = workflow.consume_reducer_inbox_atomically(
        &InboxDecisionBinding {
            inbox: [inbox_id]
                .iter()
                .map(|id| workflow.reducer_inbox[id].clone())
                .collect(),
            decision: ReducerDecision {
                expected_workflow_version: Version(1),
                plan: TransitionPlan {
                    next_status: WorkflowStatus::Active,
                    snapshot: "accepted",
                    snapshot_codec: codec("snapshot"),
                    event: "consume",
                    event_codec: codec("event"),
                    effects: vec![],
                    dependencies: vec![],
                    barriers: vec![],
                    barrier_members: vec![],
                    invalidations: vec![],
                    owed_acceptances: None,
                },
            },
        },
        &BTreeMap::new(),
    );
    assert_eq!(missing, Err(EngineError::InvalidInbox));
    assert_eq!(workflow.version, Version(1));
    assert!(workflow.owed_acceptances.is_empty());
}

#[test]
fn reducer_only_inbox_ids_cannot_be_consumed_twice() {
    let mut workflow = workflow();
    let inbox_id = ReducerInboxId(90);
    workflow.reducer_inbox.insert(
        inbox_id,
        ReducerInboxEvent {
            id: inbox_id,
            effect_id: None,
            barrier_id: None,
            kind: ReducerInboxKind::BarrierSatisfied,
            event_codec: codec("barrier"),
            requires_runtime_acceptance: false,
            payload: ReducerInboxPayload::Barrier("barrier-event"),
            delivery_status: DeliveryStatus::Pending,
            consumed_by: None,
        },
    );
    let event = workflow.reducer_inbox[&inbox_id].clone();
    let result = workflow.consume_reducer_inbox_atomically(
        &InboxDecisionBinding {
            inbox: vec![event.clone(), event],
            decision: ReducerDecision {
                expected_workflow_version: Version(0),
                plan: TransitionPlan {
                    next_status: WorkflowStatus::Active,
                    snapshot: "consumed",
                    snapshot_codec: codec("snapshot"),
                    event: "consume",
                    event_codec: codec("event"),
                    effects: vec![],
                    dependencies: vec![],
                    barriers: vec![],
                    barrier_members: vec![],
                    invalidations: vec![],
                    owed_acceptances: None,
                },
            },
        },
        &BTreeMap::new(),
    );

    assert_eq!(result, Err(EngineError::InvalidInbox));
    assert_eq!(workflow.version, Version(0));
    assert_eq!(
        workflow.reducer_inbox[&inbox_id].delivery_status,
        DeliveryStatus::Pending
    );
}

#[test]
fn commit_rejects_effect_and_barrier_id_collisions() {
    let mut workflow = workflow();
    workflow
        .commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(0),
                plan: plan(),
            },
            &barrier_events(),
        )
        .expect("commit succeeds");
    let result = workflow.commit_transition(
        &ReducerDecision {
            expected_workflow_version: Version(1),
            plan: plan(),
        },
        &barrier_events(),
    );
    assert_eq!(
        result,
        Err(EngineError::InvalidPlan(PlanError::EffectIdCollision(
            EffectId(1)
        )))
    );
}

#[test]
fn commit_rejects_barrier_id_collision_independently() {
    let mut workflow = workflow();
    workflow
        .commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(0),
                plan: plan(),
            },
            &barrier_events(),
        )
        .expect("commit succeeds");
    let mut collision = plan();
    collision.effects[0].effect_id = EffectId(3);
    collision.effects[1].effect_id = EffectId(4);
    collision.dependencies[0] = DependencyDecl {
        effect_id: EffectId(4),
        depends_on_effect_id: EffectId(3),
    };
    collision.barrier_members[0].effect_id = EffectId(3);
    collision.barrier_members[1].effect_id = EffectId(4);
    let result = workflow.commit_transition(
        &ReducerDecision {
            expected_workflow_version: Version(1),
            plan: collision,
        },
        &barrier_events(),
    );
    assert_eq!(
        result,
        Err(EngineError::InvalidPlan(PlanError::BarrierIdCollision(
            BarrierId(10)
        )))
    );
}

#[test]
fn runtime_acceptance_capability_is_independent_from_external_acceptance() {
    let mut selection = protocol();
    selection.runtime_acceptance_enabled = false;
    assert!(selection.external_acceptance_enabled);
    let mut workflow = WorkflowState::<TestProfile>::new_authoritative(
        WorkflowId(20),
        &profile(),
        &selection,
        codec("snapshot"),
        "initial",
    )
    .expect("accepting protocol");
    workflow
        .commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(0),
                plan: plan(),
            },
            &barrier_events(),
        )
        .expect("ordinary transition succeeds");
    let claim = workflow.claim_effect(EffectId(1), "worker-a", Timestamp(0), LeaseExpiry(10));
    let authority = claim.authority.expect("claim");
    let attempt = claim.attempt.expect("attempt");
    let receipt = workflow.accept_receipt(
        &authority,
        Timestamp(1),
        Some(attempt.id),
        ReceiptOrigin::Execution,
        codec("receipt"),
        "done",
        codec("receipt-event"),
        "receipt-event",
    );
    let inbox_id = receipt.receipt_inbox_ids[0];
    let consume = workflow.consume_reducer_inbox_atomically(
        &InboxDecisionBinding {
            inbox: [inbox_id]
                .iter()
                .map(|id| workflow.reducer_inbox[id].clone())
                .collect(),
            decision: ReducerDecision {
                expected_workflow_version: Version(1),
                plan: TransitionPlan {
                    next_status: WorkflowStatus::Active,
                    snapshot: "consumed",
                    snapshot_codec: codec("snapshot"),
                    event: "consume",
                    event_codec: codec("event"),
                    effects: vec![],
                    dependencies: vec![],
                    barriers: vec![],
                    barrier_members: vec![],
                    invalidations: vec![],
                    owed_acceptances: Some(vec![OwedAcceptanceDecl {
                        reducer_inbox_id: inbox_id,
                        source_kind: "wake",
                        event_codec: codec("runtime-event"),
                        event: "runtime-event",
                    }]),
                },
            },
        },
        &BTreeMap::new(),
    );
    assert_eq!(consume, Err(EngineError::InvalidInbox));
    assert_eq!(workflow.version, Version(1));
}

#[test]
fn runtime_required_inbox_is_rejected_when_protocol_runtime_acceptance_is_disabled() {
    let mut selection = protocol();
    selection.runtime_acceptance_enabled = false;
    let mut workflow = WorkflowState::<TestProfile>::new_authoritative(
        WorkflowId(21),
        &profile(),
        &selection,
        codec("snapshot"),
        "initial",
    )
    .expect("accepting protocol");
    let inbox_id = ReducerInboxId(1);
    workflow.reducer_inbox.insert(
        inbox_id,
        ReducerInboxEvent {
            id: inbox_id,
            effect_id: None,
            barrier_id: None,
            kind: ReducerInboxKind::ReceiptAccepted,
            event_codec: codec("receipt-event"),
            requires_runtime_acceptance: true,
            payload: ReducerInboxPayload::Receipt("receipt-event"),
            delivery_status: DeliveryStatus::Pending,
            consumed_by: None,
        },
    );
    let result = workflow.consume_reducer_inbox_atomically(
        &InboxDecisionBinding {
            inbox: vec![workflow.reducer_inbox[&inbox_id].clone()],
            decision: ReducerDecision {
                expected_workflow_version: Version(0),
                plan: TransitionPlan {
                    next_status: WorkflowStatus::Active,
                    snapshot: "consumed",
                    snapshot_codec: codec("snapshot"),
                    event: "consume",
                    event_codec: codec("event"),
                    effects: vec![],
                    dependencies: vec![],
                    barriers: vec![],
                    barrier_members: vec![],
                    invalidations: vec![],
                    owed_acceptances: None,
                },
            },
        },
        &BTreeMap::new(),
    );

    assert_eq!(result, Err(EngineError::InvalidInbox));
    assert_eq!(workflow.version, Version(0));
    assert_eq!(
        workflow.reducer_inbox[&inbox_id].delivery_status,
        DeliveryStatus::Pending
    );
}

#[test]
fn cas_conflict_wins_before_plan_validation_in_versioned_paths() {
    let mut workflow = workflow();
    let mut invalid = plan();
    invalid.barrier_members.clear();
    let commit = workflow
        .commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(99),
                plan: invalid.clone(),
            },
            &barrier_events(),
        )
        .expect("cas handled");
    assert_eq!(commit.outcome, CommitOutcome::VersionConflict);

    let consume = workflow
        .consume_reducer_inbox_atomically(
            &InboxDecisionBinding {
                inbox: []
                    .iter()
                    .map(|id| workflow.reducer_inbox[id].clone())
                    .collect(),
                decision: ReducerDecision {
                    expected_workflow_version: Version(99),
                    plan: invalid.clone(),
                },
            },
            &barrier_events(),
        )
        .expect("cas handled");
    assert_eq!(consume.outcome, CommitOutcome::VersionConflict);

    let runtime = workflow
        .runtime_accept_atomically(
            &OwedAcceptanceDecisionBinding {
                owed: OwedAcceptanceRecord {
                    id: OwedAcceptanceId(1),
                    reducer_inbox_id: ReducerInboxId(1),
                    source_kind: "missing",
                    event_codec: codec("owed"),
                    event: "missing",
                    disposition: OwedAcceptanceDisposition::Owed,
                },
                decision: ReducerDecision {
                    expected_workflow_version: Version(99),
                    plan: invalid.clone(),
                },
            },
            &barrier_events(),
        )
        .expect("missing obligation handled");
    assert_eq!(runtime.outcome, CommitOutcome::InvalidPlan);

    let cancel = workflow
        .cancel_with_compensation(
            &CancellationRequest {
                expected_workflow_version: Version(99),
                next_snapshot: "cancelled",
                next_snapshot_codec: codec("snapshot"),
                event: "cancel",
                event_codec: codec("event"),
                invalidations: vec![],
                reducer_inbox_events: vec![],
                compensation_plan: invalid,
            },
            &barrier_events(),
        )
        .expect("cas handled");
    assert_eq!(cancel.outcome, CommitOutcome::VersionConflict);
}

#[test]
fn terminal_runtime_acceptance_retry_is_idempotent_before_cas() {
    let mut workflow = workflow();
    workflow
        .commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(0),
                plan: plan(),
            },
            &barrier_events(),
        )
        .expect("commit succeeds");
    let claim = workflow.claim_effect(EffectId(1), "worker-a", Timestamp(0), LeaseExpiry(10));
    let authority = claim.authority.expect("authority");
    let attempt = claim.attempt.expect("attempt");
    let receipt = workflow.accept_receipt(
        &authority,
        Timestamp(1),
        Some(attempt.id),
        ReceiptOrigin::Execution,
        codec("receipt"),
        "done",
        codec("receipt-event"),
        "receipt-event",
    );
    let inbox_id = receipt.receipt_inbox_ids[0];
    workflow
        .consume_reducer_inbox_atomically(
            &InboxDecisionBinding {
                inbox: [inbox_id]
                    .iter()
                    .map(|id| workflow.reducer_inbox[id].clone())
                    .collect(),
                decision: ReducerDecision {
                    expected_workflow_version: Version(1),
                    plan: TransitionPlan {
                        next_status: WorkflowStatus::Active,
                        snapshot: "accepted",
                        snapshot_codec: codec("snapshot"),
                        event: "consume",
                        event_codec: codec("event"),
                        effects: vec![],
                        dependencies: vec![],
                        barriers: vec![],
                        barrier_members: vec![],
                        invalidations: vec![],
                        owed_acceptances: Some(vec![OwedAcceptanceDecl {
                            reducer_inbox_id: inbox_id,
                            source_kind: "wake",
                            event_codec: codec("receipt-event"),
                            event: "receipt-event",
                        }]),
                    },
                },
            },
            &BTreeMap::new(),
        )
        .expect("owed recorded");
    let owed_id = *workflow.owed_acceptances.keys().next().expect("owed");
    let stale_owed_binding = workflow.owed_acceptances[&owed_id].clone();
    let decision = ReducerDecision {
        expected_workflow_version: workflow.version,
        plan: TransitionPlan {
            next_status: WorkflowStatus::Active,
            snapshot: "runtime-accepted",
            snapshot_codec: codec("snapshot"),
            event: "accept",
            event_codec: codec("event"),
            effects: vec![],
            dependencies: vec![],
            barriers: vec![],
            barrier_members: vec![],
            invalidations: vec![],
            owed_acceptances: None,
        },
    };
    let first = workflow
        .runtime_accept_atomically(
            &OwedAcceptanceDecisionBinding {
                owed: workflow.owed_acceptances[&owed_id].clone(),
                decision: decision.clone(),
            },
            &BTreeMap::new(),
        )
        .expect("first acceptance commits");
    assert_eq!(first.outcome, CommitOutcome::Committed);
    let accepted = first.owed_acceptance.expect("accepted obligation");
    let retry = workflow
        .runtime_accept_atomically(
            &OwedAcceptanceDecisionBinding {
                owed: stale_owed_binding,
                decision: decision.clone(),
            },
            &BTreeMap::new(),
        )
        .expect("stale owed retry is idempotent");
    assert_eq!(retry.outcome, CommitOutcome::Committed);
    assert_eq!(retry.transition, None);
    assert_eq!(retry.owed_acceptance, Some(accepted));
}

#[test]
fn suppression_persists_typed_disposition() {
    let mut workflow = workflow();
    workflow
        .commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(0),
                plan: plan(),
            },
            &barrier_events(),
        )
        .expect("commit succeeds");
    let claim = workflow.claim_effect(EffectId(1), "worker-a", Timestamp(0), LeaseExpiry(10));
    let authority = claim.authority.expect("authority issued");
    let attempt = claim.attempt.expect("attempt created");
    let accepted = workflow.accept_receipt(
        &authority,
        Timestamp(1),
        Some(attempt.id),
        ReceiptOrigin::Execution,
        codec("receipt"),
        "done",
        codec("receipt-event"),
        "receipt-event",
    );
    let inbox_id = accepted.receipt_inbox_ids[0];
    workflow
        .consume_reducer_inbox_atomically(
            &InboxDecisionBinding {
                inbox: [inbox_id]
                    .iter()
                    .map(|id| workflow.reducer_inbox[id].clone())
                    .collect(),
                decision: ReducerDecision {
                    expected_workflow_version: Version(1),
                    plan: TransitionPlan {
                        next_status: WorkflowStatus::Active,
                        snapshot: "accepted",
                        snapshot_codec: codec("snapshot"),
                        event: "consume",
                        event_codec: codec("event"),
                        effects: vec![],
                        dependencies: vec![],
                        barriers: vec![],
                        barrier_members: vec![],
                        invalidations: vec![],
                        owed_acceptances: Some(vec![OwedAcceptanceDecl {
                            reducer_inbox_id: inbox_id,
                            source_kind: "wake",
                            event_codec: codec("receipt-event"),
                            event: "receipt-event",
                        }]),
                    },
                },
            },
            &BTreeMap::new(),
        )
        .expect("owed recorded");
    let owed_id = *workflow.owed_acceptances.keys().next().expect("owed id");
    let suppressed = workflow
        .suppress_runtime_acceptance_atomically(
            &OwedAcceptanceDecisionBinding {
                owed: workflow.owed_acceptances[&owed_id].clone(),
                decision: ReducerDecision {
                    expected_workflow_version: Version(2),
                    plan: TransitionPlan {
                        next_status: WorkflowStatus::Active,
                        snapshot: "suppressed",
                        snapshot_codec: codec("snapshot"),
                        event: "suppress-receipt-event",
                        event_codec: codec("event"),
                        effects: vec![],
                        dependencies: vec![],
                        barriers: vec![],
                        barrier_members: vec![],
                        invalidations: vec![],
                        owed_acceptances: None,
                    },
                },
            },
            &BTreeMap::new(),
            SuppressionReason::OperatorRejected,
        )
        .expect("suppression succeeds");
    assert_eq!(suppressed.outcome, CommitOutcome::Committed);
    assert_eq!(
        suppressed
            .owed_acceptance
            .expect("owed returned")
            .disposition,
        OwedAcceptanceDisposition::Suppressed {
            transition: TransitionId(3),
            reason: SuppressionReason::OperatorRejected,
        }
    );
}

#[test]
fn reject_unknown_and_receipted_invalidations() {
    let mut workflow = workflow();
    workflow
        .commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(0),
                plan: plan(),
            },
            &barrier_events(),
        )
        .expect("commit succeeds");
    assert_eq!(
        workflow.commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(1),
                plan: TransitionPlan {
                    next_status: WorkflowStatus::Active,
                    snapshot: "next",
                    snapshot_codec: codec("snapshot"),
                    event: "evt",
                    event_codec: codec("event"),
                    effects: vec![],
                    dependencies: vec![],
                    barriers: vec![],
                    barrier_members: vec![],
                    invalidations: vec![EffectInvalidationDecl {
                        effect_id: EffectId(999),
                    }],
                    owed_acceptances: None,
                },
            },
            &BTreeMap::new(),
        ),
        Err(EngineError::InvalidPlan(
            PlanError::UnknownInvalidationTarget(EffectId(999),)
        ))
    );

    let claim = workflow.claim_effect(EffectId(1), "worker-a", Timestamp(0), LeaseExpiry(10));
    let authority = claim.authority.expect("authority issued");
    let attempt = claim.attempt.expect("attempt created");
    let _ = workflow.accept_receipt(
        &authority,
        Timestamp(1),
        Some(attempt.id),
        ReceiptOrigin::Execution,
        codec("receipt"),
        "done",
        codec("receipt-event"),
        "receipt-event",
    );
    assert_eq!(
        workflow.commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(1),
                plan: TransitionPlan {
                    next_status: WorkflowStatus::Active,
                    snapshot: "next",
                    snapshot_codec: codec("snapshot"),
                    event: "evt",
                    event_codec: codec("event"),
                    effects: vec![],
                    dependencies: vec![],
                    barriers: vec![],
                    barrier_members: vec![],
                    invalidations: vec![EffectInvalidationDecl {
                        effect_id: EffectId(1),
                    }],
                    owed_acceptances: None,
                },
            },
            &BTreeMap::new(),
        ),
        Err(EngineError::InvalidPlan(
            PlanError::InvalidatesReceiptedEffect(EffectId(1),)
        ))
    );
}

#[test]
fn legacy_authority_cannot_execute() {
    let profile = profile();
    let mut workflow = WorkflowState::<TestProfile>::new_authoritative(
        WorkflowId(3),
        &profile,
        &legacy_protocol(),
        codec("snapshot"),
        "initial",
    )
    .expect("accepting protocol");
    assert_eq!(
        workflow.commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(0),
                plan: plan(),
            },
            &barrier_events(),
        ),
        Err(EngineError::ShadowCannotExecute)
    );
}

#[test]
fn manual_only_ambiguity_cannot_schedule_retry() {
    let mut workflow = workflow();
    let mut manual_plan = plan();
    manual_plan.effects = vec![effect_with_ambiguity(
        1,
        EffectRole::Required,
        Generation(0),
        EffectAmbiguity::ManualResolution,
    )];
    manual_plan.dependencies.clear();
    manual_plan.barriers.clear();
    manual_plan.barrier_members.clear();
    workflow
        .commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(0),
                plan: manual_plan,
            },
            &BTreeMap::new(),
        )
        .expect("commit succeeds");
    let claim = workflow.claim_effect(EffectId(1), "worker-a", Timestamp(0), LeaseExpiry(10));
    let authority = claim.authority.expect("authority issued");
    let outcome = workflow.schedule_retry(&authority, Timestamp(1), Timestamp(5));
    assert_eq!(outcome.outcome, AuthorityOutcome::StaleAuthority);
    assert_eq!(outcome.decision, None);
    assert_eq!(workflow.effects[&EffectId(1)].status, EffectStatus::Claimed);
}

#[test]
fn transitions_persist_event_codec_across_commit_cancel_and_manual_paths() {
    let mut workflow = workflow();
    workflow
        .commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(0),
                plan: plan(),
            },
            &barrier_events(),
        )
        .expect("commit succeeds");
    assert_eq!(
        workflow
            .transition_log
            .last()
            .expect("transition")
            .event_codec,
        codec("event")
    );

    let claim = workflow.claim_effect(EffectId(1), "worker-a", Timestamp(0), LeaseExpiry(10));
    let authority = claim.authority.expect("authority issued");
    let resolution = workflow
        .require_manual_resolution(
            &authority,
            Timestamp(1),
            vec![ManualChoice {
                kind: ManualChoiceKind::Adopt,
                codec: codec("manual"),
                payload: "adopt",
                receipt_codec: codec("receipt"),
                receipt: "receipt",
                receipt_event_codec: codec("receipt-event"),
                receipt_event: "manual-receipt-event",
            }],
        )
        .manual_resolution
        .expect("resolution persisted");
    let _ = workflow.resolve_manual(
        resolution.id,
        Version(1),
        "operator-a",
        &ManualChoice {
            kind: ManualChoiceKind::Adopt,
            codec: codec("manual"),
            payload: "adopt",
            receipt_codec: codec("receipt"),
            receipt: "receipt",
            receipt_event_codec: codec("receipt-event"),
            receipt_event: "manual-receipt-event",
        },
        ManualResolutionCommit {
            transition_codec: codec("manual-transition"),
            transition_event: "manual-transition",
            next_status: WorkflowStatus::Active,
        },
    );
    assert_eq!(
        workflow
            .transition_log
            .last()
            .expect("manual transition")
            .event_codec,
        codec("manual-transition")
    );
}

#[test]
fn shadow_mutation_apis_reject_manual_resolution() {
    let profile = profile();
    let mut workflow = WorkflowState::<TestProfile>::new_shadow(
        WorkflowId(4),
        WorkflowId(1),
        &profile,
        &protocol(),
        codec("snapshot"),
        "initial",
    )
    .expect("accepting protocol");
    let outcome = workflow.resolve_manual(
        ManualResolutionId(1),
        Version(0),
        "operator-a",
        &ManualChoice {
            kind: ManualChoiceKind::Adopt,
            codec: codec("manual"),
            payload: "adopt",
            receipt_codec: codec("receipt"),
            receipt: "receipt",
            receipt_event_codec: codec("receipt-event"),
            receipt_event: "manual-receipt-event",
        },
        ManualResolutionCommit {
            transition_codec: codec("manual-transition"),
            transition_event: "manual-transition",
            next_status: WorkflowStatus::Active,
        },
    );
    assert_eq!(outcome.outcome, CommitOutcome::InvalidPlan);
}

#[test]
fn cancel_only_invalidates_active_like_effects() {
    let mut workflow = workflow();
    workflow
        .commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(0),
                plan: plan(),
            },
            &barrier_events(),
        )
        .expect("commit succeeds");
    let claim = workflow.claim_effect(EffectId(1), "worker-a", Timestamp(0), LeaseExpiry(10));
    let authority = claim.authority.expect("authority issued");
    let attempt = claim.attempt.expect("attempt created");
    let _ = workflow.accept_receipt(
        &authority,
        Timestamp(1),
        Some(attempt.id),
        ReceiptOrigin::Execution,
        codec("receipt"),
        "done",
        codec("receipt-event"),
        "receipt-event",
    );
    let _ = workflow
        .cancel_with_compensation(
            &CancellationRequest {
                expected_workflow_version: Version(1),
                next_snapshot: "cancelled",
                next_snapshot_codec: codec("snapshot"),
                event: "cancel",
                event_codec: codec("cancel-event"),
                invalidations: vec![EffectInvalidationDecl {
                    effect_id: EffectId(1),
                }],
                reducer_inbox_events: vec![],
                compensation_plan: TransitionPlan {
                    next_status: WorkflowStatus::Active,
                    snapshot: "cancelled",
                    snapshot_codec: codec("snapshot"),
                    event: "cancel",
                    event_codec: codec("cancel-event"),
                    effects: vec![],
                    dependencies: vec![],
                    barriers: vec![],
                    barrier_members: vec![],
                    invalidations: vec![],
                    owed_acceptances: None,
                },
            },
            &BTreeMap::new(),
        )
        .expect("cancel succeeds");
    assert_eq!(
        workflow.effects[&EffectId(1)].status,
        EffectStatus::Receipted
    );
    assert_eq!(
        workflow
            .transition_log
            .last()
            .expect("cancel transition")
            .event_codec,
        codec("cancel-event")
    );
}

#[test]
fn drain_proof_uses_exact_categories() {
    let workflow = workflow();
    let proof = drain_proof(workflow.binding.accepted_protocol(), [&workflow]);
    let categories = exact_drain_categories();
    for category in categories {
        assert!(proof.categories.contains_key(category));
    }
    assert_eq!(proof.selector, "selector-v1");
    assert_eq!(proof.query_identity, "phoenix.workflow.drain");
    assert_eq!(proof.query_version, 1);
    assert!(proof.complete);
    assert_eq!(proof.categories.len(), exact_drain_categories().len());
}

#[test]
fn shadow_divergence_is_recorded_without_authority() {
    let profile = profile();
    let mut workflow = WorkflowState::<TestProfile>::new_shadow(
        WorkflowId(2),
        WorkflowId(1),
        &profile,
        &protocol(),
        codec("snapshot"),
        "initial",
    )
    .expect("accepting protocol");
    workflow.record_shadow_divergence(
        ShadowDivergenceKind::Snapshot,
        "snap-1".to_string(),
        ShadowComparisonEvidence {
            profile_detail_kind: "snapshot".to_string(),
            expected_codec: None,
            expected_payload: None,
            actual_codec: None,
            actual_payload: None,
        },
    );
    assert_eq!(workflow.shadow_divergences.len(), 1);
    assert_eq!(
        workflow.shadow_divergences[0].authoritative_workflow_id,
        WorkflowId(1)
    );
    assert_eq!(
        workflow.shadow_divergences[0].shadow_workflow_id,
        WorkflowId(2)
    );
    assert_eq!(
        workflow.shadow_divergences[0].action,
        DivergenceAction::HaltAcceptance
    );
    assert_eq!(
        drain_proof(workflow.binding.accepted_protocol(), [&workflow]).categories
            ["blocking_divergences"]
            .count,
        1
    );
    let divergence_id = workflow.shadow_divergences[0].id;
    assert!(workflow.resolve_shadow_divergence(
        divergence_id,
        DivergenceResolutionAction::Rollback,
        "operator-a",
    ));
    assert!(!workflow.resolve_shadow_divergence(
        divergence_id,
        DivergenceResolutionAction::Reauthorize,
        "operator-b",
    ));
    assert!(matches!(
        workflow.shadow_divergences[0].resolution,
        ShadowDivergenceResolution::Resolved {
            action: DivergenceResolutionAction::Rollback,
            resolved_by: "operator-a"
        }
    ));
    assert_eq!(
        drain_proof(workflow.binding.accepted_protocol(), [&workflow]).categories
            ["blocking_divergences"]
            .count,
        0
    );
}

#[test]
fn simulator_preserves_claim_loss_and_deadline_progress() {
    let mut workflow = workflow();
    workflow
        .commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(0),
                plan: plan(),
            },
            &barrier_events(),
        )
        .expect("commit succeeds");
    let mut sim = Simulator::new(workflow);
    sim.apply(SimOp::Claim {
        effect_id: EffectId(1),
        worker_id: "worker-a",
        lease_until: LeaseExpiry(10),
    });
    sim.apply(SimOp::CrashWorker {
        worker_id: "worker-a",
    });
    let authority = sim.workflow.effects[&EffectId(1)]
        .claim
        .clone()
        .expect("crash preserves durable claim");
    assert!(!sim.workflow.effects[&EffectId(1)].pending_reconciliation);
    assert_eq!(
        sim.workflow
            .take_over_expired_claim(
                EffectId(1),
                &authority,
                "worker-b",
                Timestamp(9),
                LeaseExpiry(20)
            )
            .outcome,
        ClaimOutcome::AuthorityConflict
    );
    let takeover = sim.workflow.take_over_expired_claim(
        EffectId(1),
        &authority,
        "worker-b",
        Timestamp(10),
        LeaseExpiry(20),
    );
    assert_eq!(takeover.outcome, ClaimOutcome::Claimed);
    let reconciliation = takeover.authority.expect("reconciliation authority issued");
    assert!(sim.workflow.effects[&EffectId(1)].pending_reconciliation);
    assert_eq!(
        sim.workflow.effects[&EffectId(1)].status,
        EffectStatus::Claimed
    );
    assert_eq!(
        sim.workflow
            .schedule_retry(&authority, Timestamp(10), Timestamp(15))
            .outcome,
        AuthorityOutcome::StaleAuthority
    );
    let reconciliation_attempt = takeover.attempt.expect("reconciliation attempt issued");
    let mut receipt_path = sim.workflow.clone();
    assert_eq!(
        receipt_path
            .accept_receipt(
                &reconciliation,
                Timestamp(10),
                Some(reconciliation_attempt.id),
                ReceiptOrigin::Execution,
                codec("receipt"),
                "done",
                codec("receipt-event"),
                "receipt-event",
            )
            .outcome,
        AuthorityOutcome::StaleAuthority
    );
    assert_eq!(
        receipt_path
            .accept_receipt(
                &reconciliation,
                Timestamp(10),
                Some(reconciliation_attempt.id),
                ReceiptOrigin::Reconciliation,
                codec("receipt"),
                "done",
                codec("receipt-event"),
                "receipt-event",
            )
            .outcome,
        AuthorityOutcome::Authorized
    );
    assert_eq!(
        sim.workflow
            .schedule_retry(&reconciliation, Timestamp(10), Timestamp(15))
            .outcome,
        AuthorityOutcome::Authorized
    );
    assert!(!sim.workflow.effects[&EffectId(1)].pending_reconciliation);
}

#[test]
fn simulator_restart_preserves_durable_claim_and_recovers_worker_runtime() {
    let mut workflow = workflow();
    workflow
        .commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(0),
                plan: plan(),
            },
            &barrier_events(),
        )
        .expect("commit succeeds");
    let mut sim = Simulator::new(workflow);
    sim.apply(SimOp::Claim {
        effect_id: EffectId(1),
        worker_id: "worker-a",
        lease_until: LeaseExpiry(10),
    });
    let durable_claim = sim.workflow.effects[&EffectId(1)]
        .claim
        .clone()
        .expect("claim persisted");
    sim.apply(SimOp::CrashWorker {
        worker_id: "worker-a",
    });
    assert!(sim.workflow.crashed_workers.contains("worker-a"));
    sim.apply(SimOp::Restart);
    assert!(sim.workflow.crashed_workers.is_empty());
    assert_eq!(
        sim.workflow.effects[&EffectId(1)].claim.as_ref(),
        Some(&durable_claim)
    );
    assert_eq!(
        sim.workflow
            .take_over_expired_claim(
                EffectId(1),
                &durable_claim,
                "worker-b",
                Timestamp(9),
                LeaseExpiry(20)
            )
            .outcome,
        ClaimOutcome::AuthorityConflict
    );
    assert_eq!(
        sim.workflow
            .take_over_expired_claim(
                EffectId(1),
                &durable_claim,
                "worker-b",
                Timestamp(10),
                LeaseExpiry(20)
            )
            .outcome,
        ClaimOutcome::Claimed
    );
}

#[test]
fn active_allows_completed_and_failed_but_terminals_do_not_reopen() {
    let workflow = workflow();
    for next_status in [WorkflowStatus::Completed, WorkflowStatus::Failed] {
        let mut candidate = workflow.clone();
        let committed = candidate
            .commit_transition(
                &ReducerDecision {
                    expected_workflow_version: Version(0),
                    plan: TransitionPlan {
                        next_status,
                        snapshot: "terminal",
                        snapshot_codec: codec("snapshot"),
                        event: "terminalize",
                        event_codec: codec("event"),
                        effects: vec![],
                        dependencies: vec![],
                        barriers: vec![],
                        barrier_members: vec![],
                        invalidations: vec![],
                        owed_acceptances: None,
                    },
                },
                &BTreeMap::new(),
            )
            .expect("terminal transition succeeds from active");
        assert_eq!(committed.outcome, CommitOutcome::Committed);
        assert_eq!(candidate.status, next_status);
        let reopen = candidate.commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(1),
                plan: TransitionPlan {
                    next_status: WorkflowStatus::Active,
                    snapshot: "reopen",
                    snapshot_codec: codec("snapshot"),
                    event: "reopen",
                    event_codec: codec("event"),
                    effects: vec![],
                    dependencies: vec![],
                    barriers: vec![],
                    barrier_members: vec![],
                    invalidations: vec![],
                    owed_acceptances: None,
                },
            },
            &BTreeMap::new(),
        );
        assert_eq!(
            reopen,
            Err(EngineError::InvalidPlan(
                PlanError::InvalidStatusTransition {
                    current: next_status,
                    next: WorkflowStatus::Active,
                }
            ))
        );
    }
}

#[test]
fn generic_commits_reserve_generation_bump_edges_and_reject_terminal_self_loops() {
    let workflow = workflow();
    for reserved in [
        WorkflowStatus::Cancelling,
        WorkflowStatus::DeletionPending,
        WorkflowStatus::Cancelled,
    ] {
        let mut candidate = workflow.clone();
        let result = candidate.commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(0),
                plan: TransitionPlan {
                    next_status: reserved,
                    snapshot: "reserved",
                    snapshot_codec: codec("snapshot"),
                    event: "reserved",
                    event_codec: codec("event"),
                    effects: vec![],
                    dependencies: vec![],
                    barriers: vec![],
                    barrier_members: vec![],
                    invalidations: vec![],
                    owed_acceptances: None,
                },
            },
            &BTreeMap::new(),
        );
        assert!(matches!(
            result,
            Err(EngineError::InvalidPlan(
                PlanError::InvalidStatusTransition { current, next }
            )) if current == WorkflowStatus::Active && next == reserved
        ));
    }

    for terminal in [
        WorkflowStatus::Cancelled,
        WorkflowStatus::Completed,
        WorkflowStatus::Failed,
    ] {
        let mut candidate = workflow.clone();
        candidate.status = terminal;
        let result = candidate.commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(0),
                plan: TransitionPlan {
                    next_status: terminal,
                    snapshot: "terminal-mutation",
                    snapshot_codec: codec("snapshot"),
                    event: "terminal-self-loop",
                    event_codec: codec("event"),
                    effects: vec![],
                    dependencies: vec![],
                    barriers: vec![],
                    barrier_members: vec![],
                    invalidations: vec![],
                    owed_acceptances: None,
                },
            },
            &BTreeMap::new(),
        );
        assert!(matches!(
            result,
            Err(EngineError::InvalidPlan(
                PlanError::InvalidStatusTransition { current, next }
            )) if current == terminal && next == terminal
        ));
    }
}

#[test]
fn cancellation_active_compensation_cannot_reopen_and_stays_cancelling() {
    let mut workflow = workflow();
    workflow
        .commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(0),
                plan: plan(),
            },
            &barrier_events(),
        )
        .expect("commit succeeds");
    let result = workflow
        .cancel_with_compensation(
            &CancellationRequest {
                expected_workflow_version: Version(1),
                next_snapshot: "cancelling",
                next_snapshot_codec: codec("snapshot"),
                event: "cancel",
                event_codec: codec("cancel-event"),
                invalidations: vec![],
                reducer_inbox_events: vec![],
                compensation_plan: TransitionPlan {
                    next_status: WorkflowStatus::Active,
                    snapshot: "should-not-reopen",
                    snapshot_codec: codec("snapshot"),
                    event: "compensate",
                    event_codec: codec("compensate-event"),
                    effects: vec![],
                    dependencies: vec![],
                    barriers: vec![],
                    barrier_members: vec![],
                    invalidations: vec![],
                    owed_acceptances: None,
                },
            },
            &BTreeMap::new(),
        )
        .expect("cancellation succeeds");
    assert_eq!(result.outcome, CommitOutcome::Committed);
    assert_eq!(workflow.status, WorkflowStatus::Cancelling);
    assert_eq!(workflow.snapshot, "cancelling");
    assert_eq!(
        workflow
            .transition_log
            .last()
            .expect("cancel transition")
            .event,
        "cancel"
    );
}

#[test]
fn reducer_only_inbox_consumes_without_owed_but_receipt_requires_exact_runtime_acceptance() {
    let mut workflow = workflow();
    workflow
        .commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(0),
                plan: plan(),
            },
            &barrier_events(),
        )
        .expect("commit succeeds");
    let reducer_only = ReducerInboxId(7000);
    workflow.reducer_inbox.insert(
        reducer_only,
        ReducerInboxEvent {
            id: reducer_only,
            effect_id: None,
            barrier_id: Some(BarrierId(77)),
            kind: ReducerInboxKind::BarrierSatisfied,
            event_codec: codec("barrier"),
            requires_runtime_acceptance: false,
            payload: ReducerInboxPayload::Barrier("barrier-only"),
            delivery_status: DeliveryStatus::Pending,
            consumed_by: None,
        },
    );
    let consumed = workflow
        .consume_reducer_inbox_atomically(
            &InboxDecisionBinding {
                inbox: [reducer_only]
                    .iter()
                    .map(|id| workflow.reducer_inbox[id].clone())
                    .collect(),
                decision: ReducerDecision {
                    expected_workflow_version: Version(1),
                    plan: TransitionPlan {
                        next_status: WorkflowStatus::Active,
                        snapshot: "reducer-only-consumed",
                        snapshot_codec: codec("snapshot"),
                        event: "consume-reducer-only",
                        event_codec: codec("event"),
                        effects: vec![],
                        dependencies: vec![],
                        barriers: vec![],
                        barrier_members: vec![],
                        invalidations: vec![],
                        owed_acceptances: None,
                    },
                },
            },
            &BTreeMap::new(),
        )
        .expect("reducer-only inbox can commit without owed acceptance");
    assert_eq!(consumed.outcome, CommitOutcome::Committed);
    assert!(workflow.owed_acceptances.is_empty());

    let claim = workflow.claim_effect(EffectId(1), "worker-a", Timestamp(0), LeaseExpiry(10));
    let authority = claim.authority.expect("authority");
    let attempt = claim.attempt.expect("attempt");
    let accepted = workflow.accept_receipt(
        &authority,
        Timestamp(1),
        Some(attempt.id),
        ReceiptOrigin::Execution,
        codec("receipt"),
        "done",
        codec("receipt-event"),
        "receipt-event",
    );
    let receipt_inbox_id = accepted.receipt_inbox_ids[0];
    let rejected = workflow.consume_reducer_inbox_atomically(
        &InboxDecisionBinding {
            inbox: [receipt_inbox_id]
                .iter()
                .map(|id| workflow.reducer_inbox[id].clone())
                .collect(),
            decision: ReducerDecision {
                expected_workflow_version: Version(2),
                plan: TransitionPlan {
                    next_status: WorkflowStatus::Active,
                    snapshot: "missing-owed",
                    snapshot_codec: codec("snapshot"),
                    event: "consume-receipt",
                    event_codec: codec("event"),
                    effects: vec![],
                    dependencies: vec![],
                    barriers: vec![],
                    barrier_members: vec![],
                    invalidations: vec![],
                    owed_acceptances: None,
                },
            },
        },
        &BTreeMap::new(),
    );
    assert_eq!(rejected, Err(EngineError::InvalidInbox));
}

#[test]
fn shadow_drain_excludes_all_authority_categories_except_unresolved_blocking_divergence() {
    let profile = profile();
    let mut workflow = WorkflowState::<TestProfile>::new_shadow(
        WorkflowId(9),
        WorkflowId(1),
        &profile,
        &protocol(),
        codec("snapshot"),
        "initial",
    )
    .expect("shadow workflow");
    workflow.record_shadow_divergence(
        ShadowDivergenceKind::Receipt,
        "receipt-divergence".to_string(),
        ShadowComparisonEvidence {
            profile_detail_kind: "receipt".to_string(),
            expected_codec: Some(codec("expected")),
            expected_payload: Some("a".to_string()),
            actual_codec: Some(codec("actual")),
            actual_payload: Some("b".to_string()),
        },
    );
    let proof = drain_proof(workflow.binding.accepted_protocol(), [&workflow]);
    for category in exact_drain_categories() {
        if category == "blocking_divergences" {
            assert_eq!(proof.categories[category].count, 1);
        } else {
            assert_eq!(
                proof.categories[category].count, 0,
                "category {category} should be excluded for shadow drain"
            );
        }
    }
}

#[test]
fn monotonic_renewal_uses_stored_lease_and_updates_destructive_lock() {
    let mut workflow = workflow();
    let mut destructive_plan = plan();
    destructive_plan.effects[0].destructive_resource = Some("resource-a");
    destructive_plan.effects[1].effect_id = EffectId(3);
    destructive_plan.effects[1].destructive_resource = Some("resource-b");
    destructive_plan.dependencies[0] = DependencyDecl {
        effect_id: EffectId(3),
        depends_on_effect_id: EffectId(1),
    };
    destructive_plan.barrier_members[1].effect_id = EffectId(3);
    workflow
        .commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(0),
                plan: destructive_plan,
            },
            &barrier_events(),
        )
        .expect("commit succeeds");
    let claim = workflow.claim_effect(EffectId(1), "worker-a", Timestamp(0), LeaseExpiry(10));
    let authority = claim.authority.expect("authority");
    assert_eq!(
        workflow
            .renew_claim(&authority, Timestamp(1), LeaseExpiry(10))
            .outcome,
        AuthorityOutcome::StaleAuthority
    );
    assert_eq!(
        workflow
            .renew_claim(&authority, Timestamp(1), LeaseExpiry(9))
            .outcome,
        AuthorityOutcome::StaleAuthority
    );
    assert_eq!(
        workflow
            .renew_claim(&authority, Timestamp(1), LeaseExpiry(20))
            .outcome,
        AuthorityOutcome::Authorized
    );
    let effect = &workflow.effects[&EffectId(1)];
    assert_eq!(
        effect.claim.as_ref().expect("claim").lease_until,
        LeaseExpiry(20)
    );
    assert_eq!(
        effect.destructive_lock.as_ref().expect("lock").lease_until,
        LeaseExpiry(20)
    );
}

#[test]
fn observation_codec_persists_and_empty_family_is_rejected() {
    let mut workflow = workflow();
    workflow
        .commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(0),
                plan: plan(),
            },
            &barrier_events(),
        )
        .expect("commit succeeds");
    let claim = workflow.claim_effect(EffectId(1), "worker-a", Timestamp(0), LeaseExpiry(10));
    let authority = claim.authority.expect("authority");
    let attempt = claim.attempt.expect("attempt");
    assert_eq!(
        workflow.record_observation(
            &authority,
            Timestamp(1),
            attempt.id,
            codec("observation"),
            "observed"
        ),
        AuthorityOutcome::Authorized
    );
    assert_eq!(
        workflow.effects[&EffectId(1)].observations[0].observation_codec,
        codec("observation")
    );
    let before = workflow.clone();
    assert_eq!(
        workflow.record_observation(
            &authority,
            Timestamp(1),
            attempt.id,
            CodecRef {
                family: "",
                version: 1
            },
            "ignored"
        ),
        AuthorityOutcome::StaleAuthority
    );
    assert_eq!(workflow, before);
}

#[test]
fn empty_manual_choices_are_rejected() {
    let mut workflow = workflow();
    workflow
        .commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(0),
                plan: plan(),
            },
            &barrier_events(),
        )
        .expect("commit succeeds");
    let claim = workflow.claim_effect(EffectId(1), "worker-a", Timestamp(0), LeaseExpiry(10));
    let authority = claim.authority.expect("authority");
    let before = workflow.clone();
    let outcome = workflow.require_manual_resolution(&authority, Timestamp(1), vec![]);
    assert_eq!(outcome.outcome, AuthorityOutcome::StaleAuthority);
    assert_eq!(workflow, before);
}

#[test]
fn empty_cancellation_codecs_are_rejected_atomically() {
    let mut workflow = workflow();
    workflow
        .commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(0),
                plan: plan(),
            },
            &barrier_events(),
        )
        .expect("commit succeeds");
    let before = workflow.clone();
    let result = workflow.cancel_with_compensation(
        &CancellationRequest {
            expected_workflow_version: Version(1),
            next_snapshot: "cancelled",
            next_snapshot_codec: CodecRef {
                family: "",
                version: 1,
            },
            event: "cancel",
            event_codec: codec("cancel-event"),
            invalidations: vec![],
            reducer_inbox_events: vec![],
            compensation_plan: TransitionPlan {
                next_status: WorkflowStatus::Cancelling,
                snapshot: "cancelled",
                snapshot_codec: codec("snapshot"),
                event: "cancel",
                event_codec: codec("event"),
                effects: vec![],
                dependencies: vec![],
                barriers: vec![],
                barrier_members: vec![],
                invalidations: vec![],
                owed_acceptances: None,
            },
        },
        &BTreeMap::new(),
    );
    assert_eq!(
        result,
        Err(EngineError::InvalidPlan(PlanError::MissingCodec(
            "cancellation"
        )))
    );
    assert_eq!(workflow, before);
}

#[test]
fn empty_manual_commit_codecs_and_invalid_manual_status_are_rejected_atomically() {
    let mut workflow = workflow();
    workflow
        .commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(0),
                plan: plan(),
            },
            &barrier_events(),
        )
        .expect("commit succeeds");
    let claim = workflow.claim_effect(EffectId(1), "worker-a", Timestamp(0), LeaseExpiry(10));
    let authority = claim.authority.expect("authority");
    let resolution = workflow
        .require_manual_resolution(
            &authority,
            Timestamp(1),
            vec![ManualChoice {
                kind: ManualChoiceKind::Adopt,
                codec: codec("manual"),
                payload: "adopt",
                receipt_codec: codec("receipt"),
                receipt: "receipt",
                receipt_event_codec: codec("receipt-event"),
                receipt_event: "manual-receipt-event",
            }],
        )
        .manual_resolution
        .expect("resolution");
    let before = workflow.clone();
    let empty_codec = workflow.resolve_manual(
        resolution.id,
        Version(1),
        "operator-a",
        &ManualChoice {
            kind: ManualChoiceKind::Adopt,
            codec: codec("manual"),
            payload: "adopt",
            receipt_codec: codec("receipt"),
            receipt: "receipt",
            receipt_event_codec: codec("receipt-event"),
            receipt_event: "manual-receipt-event",
        },
        ManualResolutionCommit {
            transition_codec: CodecRef {
                family: "",
                version: 1,
            },
            transition_event: "manual-transition",
            next_status: WorkflowStatus::Active,
        },
    );
    assert_eq!(empty_codec.outcome, CommitOutcome::InvalidPlan);
    assert_eq!(workflow, before);
    workflow.status = WorkflowStatus::Cancelling;
    let before_cancelling = workflow.clone();
    let invalid_status = workflow.resolve_manual(
        resolution.id,
        Version(1),
        "operator-a",
        &ManualChoice {
            kind: ManualChoiceKind::Adopt,
            codec: codec("manual"),
            payload: "adopt",
            receipt_codec: codec("receipt"),
            receipt: "receipt",
            receipt_event_codec: codec("receipt-event"),
            receipt_event: "manual-receipt-event",
        },
        ManualResolutionCommit {
            transition_codec: codec("manual-transition"),
            transition_event: "manual-transition",
            next_status: WorkflowStatus::Active,
        },
    );
    assert_eq!(invalid_status.outcome, CommitOutcome::InvalidPlan);
    assert_eq!(workflow, before_cancelling);
}

#[test]
fn empty_receipt_codecs_are_rejected() {
    let mut workflow = workflow();
    workflow
        .commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(0),
                plan: plan(),
            },
            &barrier_events(),
        )
        .expect("commit succeeds");
    let claim = workflow.claim_effect(EffectId(1), "worker-a", Timestamp(0), LeaseExpiry(10));
    let authority = claim.authority.expect("authority");
    let attempt = claim.attempt.expect("attempt");
    let before = workflow.clone();
    let accepted = workflow.accept_receipt(
        &authority,
        Timestamp(1),
        Some(attempt.id),
        ReceiptOrigin::Execution,
        CodecRef {
            family: "",
            version: 1,
        },
        "done",
        codec("receipt-event"),
        "receipt-event",
    );
    assert_eq!(accepted.outcome, AuthorityOutcome::StaleAuthority);
    assert_eq!(workflow, before);
}

#[test]
fn explicit_reauthorize_divergence_action_is_persisted() {
    let profile = profile();
    let mut workflow = WorkflowState::<TestProfile>::new_shadow(
        WorkflowId(11),
        WorkflowId(1),
        &profile,
        &protocol(),
        codec("snapshot"),
        "initial",
    )
    .expect("shadow workflow");
    workflow.record_shadow_divergence(
        ShadowDivergenceKind::Capability,
        "cap-1".to_string(),
        ShadowComparisonEvidence {
            profile_detail_kind: "capability".to_string(),
            expected_codec: None,
            expected_payload: Some("expected".to_string()),
            actual_codec: None,
            actual_payload: Some("actual".to_string()),
        },
    );
    let divergence_id = workflow.shadow_divergences[0].id;
    assert!(workflow.resolve_shadow_divergence(
        divergence_id,
        DivergenceResolutionAction::Reauthorize,
        "operator-z",
    ));
    assert!(matches!(
        workflow.shadow_divergences[0].resolution,
        ShadowDivergenceResolution::Resolved {
            action: DivergenceResolutionAction::Reauthorize,
            resolved_by: "operator-z"
        }
    ));
}

#[test]
fn forged_or_missing_destructive_resource_lock_is_rejected() {
    let mut workflow = workflow();
    let mut destructive_plan = plan();
    destructive_plan.effects[0].destructive_resource = Some("resource-a");
    destructive_plan.effects[1].effect_id = EffectId(3);
    destructive_plan.effects[1].destructive_resource = Some("resource-b");
    destructive_plan.dependencies[0] = DependencyDecl {
        effect_id: EffectId(3),
        depends_on_effect_id: EffectId(1),
    };
    destructive_plan.barrier_members[1].effect_id = EffectId(3);
    workflow
        .commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(0),
                plan: destructive_plan,
            },
            &barrier_events(),
        )
        .expect("commit succeeds");
    let claim = workflow.claim_effect(EffectId(1), "worker-a", Timestamp(0), LeaseExpiry(10));
    let authority = claim.authority.expect("authority");
    let attempt = claim.attempt.expect("attempt");

    let mut forged = authority.clone();
    forged.resource_lock = None;
    assert_eq!(
        workflow.record_observation(
            &forged,
            Timestamp(1),
            attempt.id,
            codec("observation"),
            "forged"
        ),
        AuthorityOutcome::StaleAuthority
    );

    let mut forged_lock = authority.clone();
    if let Some(lock) = &mut forged_lock.resource_lock {
        lock.claim_token += 1;
    }
    assert_eq!(
        workflow
            .accept_receipt(
                &forged_lock,
                Timestamp(1),
                Some(attempt.id),
                ReceiptOrigin::Execution,
                codec("receipt"),
                "done",
                codec("receipt-event"),
                "receipt-event",
            )
            .outcome,
        AuthorityOutcome::StaleAuthority
    );

    assert!(workflow.effects[&EffectId(1)].receipt.is_none());
    assert_eq!(workflow.effects[&EffectId(1)].status, EffectStatus::Claimed);
}

proptest! {
    #[test]
    fn plan_cycle_detection_matches_simple_generator(extra_edges in prop::collection::vec((0u8..4, 0u8..4), 0..8)) {
        let effects = (0u64..4)
            .map(|idx| effect(idx + 1, EffectRole::Required, Generation(0)))
            .collect::<Vec<_>>();
        let dependencies = extra_edges
            .into_iter()
            .filter(|(a, b)| a != b)
            .map(|(a, b)| DependencyDecl {
                effect_id: EffectId(u64::from(a) + 1),
                depends_on_effect_id: EffectId(u64::from(b) + 1),
            })
            .collect::<Vec<_>>();
        let plan: TransitionPlan<TestProfile> = TransitionPlan {
            next_status: WorkflowStatus::Active,
            snapshot: "next",
            snapshot_codec: codec("snapshot"),
            event: "evt",
            event_codec: codec("event"),
            effects,
            dependencies: dependencies.clone(),
            barriers: vec![BarrierDecl { barrier_id: BarrierId(10), reducer_event_codec: codec("barrier") }],
            barrier_members: vec![
                BarrierMemberDecl { barrier_id: BarrierId(10), effect_id: EffectId(1), receipt_family: ReceiptFamily::CurrentGenerationEffect },
            ],
            invalidations: vec![],
            owed_acceptances: None,
        };
        let result = validate_plan(WorkflowStatus::Active, &plan, &BTreeMap::from([(BarrierId(10), "barrier")]));
        let has_two_cycle = dependencies.iter().any(|left| {
            dependencies.iter().any(|right| {
                left.effect_id == right.depends_on_effect_id && left.depends_on_effect_id == right.effect_id
            })
        });
        prop_assert!(!(has_two_cycle && result.is_ok()));
    }
}

#[test]
fn review_regressions_validate_identity_codecs_status_and_compensation_path() {
    let empty = CodecRef {
        family: "",
        version: 1,
    };
    assert_eq!(
        WorkflowState::<TestProfile>::new_authoritative(
            WorkflowId(44),
            &profile(),
            &protocol(),
            empty,
            "initial",
        ),
        Err(EngineError::InvalidPlan(PlanError::MissingCodec(
            "snapshot"
        )))
    );

    let mut registry = ExternalAcceptanceRegistry::new();
    assert_eq!(
        registry.accept(&protocol(), "", "key", "intent", WorkflowId(1), "handle"),
        ExternalAcceptanceOutcome::Unsupported
    );
    assert_eq!(
        registry.accept(&protocol(), "scope", "", "intent", WorkflowId(1), "handle"),
        ExternalAcceptanceOutcome::Unsupported
    );
    assert!(registry.is_empty());

    let mut invalid = plan();
    invalid.effects[0].family = "";
    assert_eq!(
        validate_plan(WorkflowStatus::Active, &invalid, &barrier_events()),
        Err(PlanError::MissingEffectFamily(EffectId(1)))
    );
    let mut invalid = plan();
    invalid.effects[0].kind = "";
    assert_eq!(
        validate_plan(WorkflowStatus::Active, &invalid, &barrier_events()),
        Err(PlanError::MissingEffectKind(EffectId(1)))
    );

    let mut workflow = workflow();
    let mut compensation = plan();
    compensation.effects = vec![effect(9, EffectRole::Compensation, Generation(1))];
    compensation.dependencies.clear();
    compensation.barriers.clear();
    compensation.barrier_members.clear();
    assert_eq!(
        workflow.commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(0),
                plan: compensation,
            },
            &BTreeMap::new(),
        ),
        Err(EngineError::InvalidPlan(
            PlanError::CompensationOutsideCancellation(EffectId(9))
        ))
    );

    let mut cancelled = plan();
    cancelled.next_status = WorkflowStatus::Cancelled;
    assert_eq!(
        workflow.commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(0),
                plan: cancelled,
            },
            &barrier_events(),
        ),
        Err(EngineError::InvalidPlan(
            PlanError::InvalidStatusTransition {
                current: WorkflowStatus::Active,
                next: WorkflowStatus::Cancelled,
            }
        ))
    );

    workflow.status = WorkflowStatus::DeletionPending;
    let completed = TransitionPlan {
        next_status: WorkflowStatus::Completed,
        snapshot: "deleted",
        snapshot_codec: codec("snapshot"),
        event: "cleanup-complete",
        event_codec: codec("event"),
        effects: vec![],
        dependencies: vec![],
        barriers: vec![],
        barrier_members: vec![],
        invalidations: vec![],
        owed_acceptances: None,
    };
    assert_eq!(
        workflow
            .commit_transition(
                &ReducerDecision {
                    expected_workflow_version: Version(0),
                    plan: completed,
                },
                &BTreeMap::new(),
            )
            .expect("deletion cleanup can complete")
            .outcome,
        CommitOutcome::Committed
    );
}

#[test]
fn review_regressions_preserve_renewed_authority_and_ambiguity_family_contract() {
    let mut workflow = workflow();
    let mut destructive = plan();
    destructive.effects[0].destructive_resource = Some("resource-a");
    workflow
        .commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(0),
                plan: destructive,
            },
            &barrier_events(),
        )
        .expect("plan");
    let claim = workflow.claim_effect(EffectId(1), "worker", Timestamp(0), LeaseExpiry(10));
    let attempt = claim.attempt.expect("attempt");
    let authority = claim.authority.expect("authority");
    let renewed = workflow.renew_claim(&authority, Timestamp(1), LeaseExpiry(20));
    let renewed_authority = renewed.authority.expect("renewed authority returned");
    assert_eq!(renewed_authority.lease_until, LeaseExpiry(20));
    assert_eq!(
        renewed_authority
            .resource_lock
            .as_ref()
            .expect("lock")
            .lease_until,
        LeaseExpiry(20)
    );
    assert_eq!(
        workflow.record_observation(
            &renewed_authority,
            Timestamp(2),
            attempt.id,
            codec("observation"),
            "after-renewal",
        ),
        AuthorityOutcome::Authorized
    );

    let mut mismatch = TransitionPlan {
        next_status: WorkflowStatus::Active,
        snapshot: "next",
        snapshot_codec: codec("snapshot"),
        event: "next",
        event_codec: codec("event"),
        effects: vec![effect_with_ambiguity(
            30,
            EffectRole::Required,
            Generation(0),
            EffectAmbiguity::ManualResolution,
        )],
        dependencies: vec![],
        barriers: vec![],
        barrier_members: vec![],
        invalidations: vec![],
        owed_acceptances: None,
    };
    mismatch.effects[0].family = "test";
    assert_eq!(
        workflow.commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(1),
                plan: mismatch,
            },
            &BTreeMap::new(),
        ),
        Err(EngineError::InvalidPlan(
            PlanError::EffectFamilyAmbiguityMismatch { family: "test" }
        ))
    );
}

#[test]
fn renewed_claim_rejects_the_pre_renewal_authority_for_non_destructive_effects() {
    let mut workflow = workflow();
    workflow
        .commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(0),
                plan: plan(),
            },
            &barrier_events(),
        )
        .expect("plan");
    let claim = workflow.claim_effect(EffectId(1), "worker", Timestamp(0), LeaseExpiry(10));
    let original_authority = claim.authority.expect("authority");
    let attempt = claim.attempt.expect("attempt");
    let renewed_authority = workflow
        .renew_claim(&original_authority, Timestamp(1), LeaseExpiry(20))
        .authority
        .expect("renewed authority");

    assert_eq!(
        workflow.record_observation(
            &original_authority,
            Timestamp(2),
            attempt.id,
            codec("observation"),
            "stale bearer",
        ),
        AuthorityOutcome::StaleAuthority
    );
    assert_eq!(
        workflow.record_observation(
            &renewed_authority,
            Timestamp(2),
            attempt.id,
            codec("observation"),
            "renewed bearer",
        ),
        AuthorityOutcome::Authorized
    );
}

#[test]
fn post_terminal_receipt_is_durably_suppressed_instead_of_pending() {
    let mut workflow = workflow();
    workflow
        .commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(0),
                plan: plan(),
            },
            &barrier_events(),
        )
        .expect("plan");
    let claim = workflow.claim_effect(EffectId(1), "worker", Timestamp(0), LeaseExpiry(10));
    workflow
        .commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(1),
                plan: TransitionPlan {
                    next_status: WorkflowStatus::Completed,
                    snapshot: "completed",
                    snapshot_codec: codec("snapshot"),
                    event: "complete",
                    event_codec: codec("event"),
                    effects: vec![],
                    dependencies: vec![],
                    barriers: vec![],
                    barrier_members: vec![],
                    invalidations: vec![],
                    owed_acceptances: None,
                },
            },
            &BTreeMap::new(),
        )
        .expect("complete workflow");
    let accepted = workflow.accept_receipt(
        &claim.authority.expect("authority"),
        Timestamp(1),
        Some(claim.attempt.expect("attempt").id),
        ReceiptOrigin::Execution,
        codec("receipt"),
        "done",
        codec("receipt-event"),
        "receipt-event",
    );
    assert_eq!(accepted.outcome, AuthorityOutcome::StaleAuthority);
    assert!(accepted.receipt_inbox_ids.is_empty());
    assert_eq!(
        drain_proof(&protocol(), [&workflow]).categories["pending_reducer_inbox"].count,
        0
    );
}

#[test]
fn review_regressions_manual_resolution_survives_versions_and_holds_lock() {
    let mut workflow = workflow();
    let mut first = plan();
    first.effects[0].destructive_resource = Some("resource-a");
    workflow
        .commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(0),
                plan: first,
            },
            &barrier_events(),
        )
        .expect("plan");
    let claim = workflow.claim_effect(EffectId(1), "worker", Timestamp(0), LeaseExpiry(10));
    let authority = claim.authority.expect("authority");
    let resolution = workflow
        .require_manual_resolution(
            &authority,
            Timestamp(1),
            vec![ManualChoice {
                kind: ManualChoiceKind::Adopt,
                codec: codec("manual"),
                payload: "adopt",
                receipt_codec: codec("receipt"),
                receipt: "receipt",
                receipt_event_codec: codec("receipt-event"),
                receipt_event: "manual-receipt-event",
            }],
        )
        .manual_resolution
        .expect("resolution");
    assert_eq!(
        workflow.effects[&EffectId(1)]
            .destructive_lock
            .as_ref()
            .expect("ambiguity lock retained")
            .lease_until,
        LeaseExpiry(u64::MAX)
    );

    workflow
        .commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(1),
                plan: TransitionPlan {
                    next_status: WorkflowStatus::Active,
                    snapshot: "unrelated",
                    snapshot_codec: codec("snapshot"),
                    event: "unrelated",
                    event_codec: codec("event"),
                    effects: vec![],
                    dependencies: vec![],
                    barriers: vec![],
                    barrier_members: vec![],
                    invalidations: vec![],
                    owed_acceptances: None,
                },
            },
            &BTreeMap::new(),
        )
        .expect("unrelated transition");
    let resolved = workflow.resolve_manual(
        resolution.id,
        Version(2),
        "operator",
        &resolution.permitted_choices[0],
        ManualResolutionCommit {
            transition_codec: codec("manual-transition"),
            transition_event: "manual",
            next_status: WorkflowStatus::Active,
        },
    );
    assert_eq!(resolved.outcome, CommitOutcome::Committed);
    assert_eq!(
        match resolved.effect_outcome.expect("manual outcome") {
            ManualEffectOutcome::Receipt { receipt, .. } => {
                receipt.authority.declared_workflow_version
            }
            outcome @ (ManualEffectOutcome::Retry
            | ManualEffectOutcome::Compensate
            | ManualEffectOutcome::Failed
            | ManualEffectOutcome::Suppressed) => {
                panic!("unexpected manual outcome: {outcome:?}")
            }
        },
        Version(1)
    );
}

#[test]
fn review_regressions_reject_misbound_inbox_and_cross_workflow_observation() {
    let mut workflow = workflow();
    workflow.reducer_inbox.insert(
        ReducerInboxId(50),
        ReducerInboxEvent {
            id: ReducerInboxId(50),
            effect_id: None,
            barrier_id: None,
            kind: ReducerInboxKind::ReceiptAccepted,
            event_codec: codec("event-a"),
            requires_runtime_acceptance: false,
            payload: ReducerInboxPayload::Receipt("event-a"),
            delivery_status: DeliveryStatus::Pending,
            consumed_by: None,
        },
    );
    let linked = workflow.reducer_inbox[&ReducerInboxId(50)].clone();
    let result = workflow
        .consume_reducer_inbox_atomically(
            &InboxDecisionBinding {
                inbox: vec![linked],
                decision: ReducerDecision {
                    expected_workflow_version: Version(0),
                    plan: TransitionPlan {
                        next_status: WorkflowStatus::Active,
                        snapshot: "wrong",
                        snapshot_codec: codec("snapshot"),
                        event: "event-b",
                        event_codec: codec("event"),
                        effects: vec![],
                        dependencies: vec![],
                        barriers: vec![],
                        barrier_members: vec![],
                        invalidations: vec![],
                        owed_acceptances: None,
                    },
                },
            },
            &BTreeMap::new(),
        )
        .expect("typed rejection");
    assert_eq!(result.outcome, CommitOutcome::InvalidPlan);
    assert_eq!(
        workflow.reducer_inbox[&ReducerInboxId(50)].delivery_status,
        DeliveryStatus::Pending
    );

    let mut other = WorkflowState::<TestProfile>::new_authoritative(
        WorkflowId(2),
        &profile(),
        &protocol(),
        codec("snapshot"),
        "initial",
    )
    .expect("other workflow");
    other.binding = match other.binding {
        WorkflowBinding::Authoritative(mut binding) => {
            binding.workflow_id = WorkflowId(2);
            WorkflowBinding::Authoritative(binding)
        }
        WorkflowBinding::Shadow(_) => unreachable!(),
    };
    other
        .commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(0),
                plan: plan(),
            },
            &barrier_events(),
        )
        .expect("other plan");
    let claim = other.claim_effect(EffectId(1), "worker", Timestamp(0), LeaseExpiry(1));
    let authority = claim.authority.expect("other authority");
    assert_eq!(
        workflow.record_observation(
            &authority,
            Timestamp(2),
            claim.attempt.expect("attempt").id,
            codec("observation"),
            "misrouted",
        ),
        AuthorityOutcome::StaleAuthority
    );
    assert!(workflow
        .effects
        .get(&EffectId(1))
        .is_none_or(|effect| effect.stale_observations.is_empty()));
}

#[test]
fn review_regressions_protocol_drain_aggregates_and_counts_deletion_pending() {
    let mut first = workflow();
    first.status = WorkflowStatus::DeletionPending;
    let mut second = workflow();
    second.binding = match second.binding {
        WorkflowBinding::Authoritative(mut binding) => {
            binding.workflow_id = WorkflowId(2);
            WorkflowBinding::Authoritative(binding)
        }
        WorkflowBinding::Shadow(_) => unreachable!(),
    };
    second.reducer_inbox.insert(
        ReducerInboxId(9),
        ReducerInboxEvent {
            id: ReducerInboxId(9),
            effect_id: None,
            barrier_id: None,
            kind: ReducerInboxKind::BarrierSatisfied,
            event_codec: codec("barrier"),
            requires_runtime_acceptance: false,
            payload: ReducerInboxPayload::Barrier("pending"),
            delivery_status: DeliveryStatus::Pending,
            consumed_by: None,
        },
    );
    let proof = drain_proof(&protocol(), [&first, &second]);
    assert_eq!(proof.categories["nonterminal_workflows"].count, 2);
    assert_eq!(proof.categories["pending_reducer_inbox"].count, 1);
    assert_eq!(
        proof.categories["pending_reducer_inbox"].identities,
        vec!["inbox:9"]
    );
}

#[test]
fn review_regressions_cancellation_suppresses_manual_and_owed_work_atomically() {
    let mut workflow = workflow();
    workflow
        .commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(0),
                plan: plan(),
            },
            &barrier_events(),
        )
        .expect("plan");
    let claim = workflow.claim_effect(EffectId(1), "worker", Timestamp(0), LeaseExpiry(10));
    let authority = claim.authority.expect("authority");
    let resolution = workflow
        .require_manual_resolution(
            &authority,
            Timestamp(1),
            vec![ManualChoice {
                kind: ManualChoiceKind::Adopt,
                codec: codec("manual"),
                payload: "adopt",
                receipt_codec: codec("receipt"),
                receipt: "receipt",
                receipt_event_codec: codec("receipt-event"),
                receipt_event: "manual-receipt-event",
            }],
        )
        .manual_resolution
        .expect("manual work");
    workflow.owed_acceptances.insert(
        OwedAcceptanceId(77),
        OwedAcceptanceRecord {
            id: OwedAcceptanceId(77),
            reducer_inbox_id: ReducerInboxId(77),
            source_kind: "wake",
            event_codec: codec("owed"),
            event: "event-a",
            disposition: OwedAcceptanceDisposition::Owed,
        },
    );
    let result = workflow
        .cancel_with_compensation(
            &CancellationRequest {
                expected_workflow_version: Version(1),
                next_snapshot: "cancelled",
                next_snapshot_codec: codec("snapshot"),
                event: "cancel",
                event_codec: codec("event"),
                invalidations: vec![EffectInvalidationDecl {
                    effect_id: EffectId(1),
                }],
                reducer_inbox_events: vec![],
                compensation_plan: TransitionPlan {
                    next_status: WorkflowStatus::Cancelled,
                    snapshot: "cancelled",
                    snapshot_codec: codec("snapshot"),
                    event: "cancel",
                    event_codec: codec("event"),
                    effects: vec![],
                    dependencies: vec![],
                    barriers: vec![],
                    barrier_members: vec![],
                    invalidations: vec![],
                    owed_acceptances: None,
                },
            },
            &BTreeMap::new(),
        )
        .expect("cancellation");
    assert_eq!(result.outcome, CommitOutcome::Committed);
    assert!(matches!(
        workflow.manual_resolutions[&resolution.id].status,
        ResolutionStatus::Suppressed {
            reason: SuppressionReason::Cancelled,
            ..
        }
    ));
    assert!(matches!(
        workflow.owed_acceptances[&OwedAcceptanceId(77)].disposition,
        OwedAcceptanceDisposition::Suppressed {
            reason: SuppressionReason::Cancelled,
            ..
        }
    ));
    let proof = drain_proof(&protocol(), [&workflow]);
    assert_eq!(proof.categories["unresolved_manual_resolutions"].count, 0);
    assert_eq!(proof.categories["owed_runtime_acceptances"].count, 0);
    assert_eq!(proof.categories["pending_reducer_inbox"].count, 0);
    assert!(workflow.reducer_inbox.values().all(|event| {
        event.delivery_status
            == DeliveryStatus::Suppressed {
                reason: SuppressionReason::Cancelled,
            }
    }));
}

#[test]
fn review_regressions_reject_misbound_owed_decision_and_profile_controls_receipt_acceptance() {
    let mut workflow = workflow();
    workflow.owed_acceptances.insert(
        OwedAcceptanceId(1),
        OwedAcceptanceRecord {
            id: OwedAcceptanceId(1),
            reducer_inbox_id: ReducerInboxId(1),
            source_kind: "wake",
            event_codec: codec("owed"),
            event: "event-a",
            disposition: OwedAcceptanceDisposition::Owed,
        },
    );
    let result = workflow
        .runtime_accept_atomically(
            &OwedAcceptanceDecisionBinding {
                owed: workflow.owed_acceptances[&OwedAcceptanceId(1)].clone(),
                decision: ReducerDecision {
                    expected_workflow_version: Version(0),
                    plan: TransitionPlan {
                        next_status: WorkflowStatus::Active,
                        snapshot: "wrong",
                        snapshot_codec: codec("snapshot"),
                        event: "event-b",
                        event_codec: codec("event"),
                        effects: vec![],
                        dependencies: vec![],
                        barriers: vec![],
                        barrier_members: vec![],
                        invalidations: vec![],
                        owed_acceptances: None,
                    },
                },
            },
            &BTreeMap::new(),
        )
        .expect("typed rejection");
    assert_eq!(result.outcome, CommitOutcome::InvalidPlan);
    assert_eq!(workflow.version, Version(0));
    assert_eq!(
        workflow.owed_acceptances[&OwedAcceptanceId(1)].disposition,
        OwedAcceptanceDisposition::Owed
    );

    workflow
        .commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(0),
                plan: plan(),
            },
            &barrier_events(),
        )
        .expect("plan");
    let claim = workflow.claim_effect(EffectId(1), "worker", Timestamp(0), LeaseExpiry(10));
    let authority = claim.authority.expect("authority");
    let accepted = workflow.accept_receipt(
        &authority,
        Timestamp(1),
        Some(claim.attempt.expect("attempt").id),
        ReceiptOrigin::Execution,
        codec("receipt"),
        "done",
        codec("receipt-event"),
        "reducer-only",
    );
    assert!(!workflow.reducer_inbox[&accepted.receipt_inbox_ids[0]].requires_runtime_acceptance);
}

#[test]
fn manual_resolution_requirement_rejects_any_empty_choice_codec() {
    for missing_codec in ["choice", "receipt", "receipt-event"] {
        let mut workflow = workflow();
        workflow
            .commit_transition(
                &ReducerDecision {
                    expected_workflow_version: Version(0),
                    plan: plan(),
                },
                &barrier_events(),
            )
            .expect("plan");
        let claim = workflow.claim_effect(EffectId(1), "worker", Timestamp(0), LeaseExpiry(10));
        let mut choice = ManualChoice {
            kind: ManualChoiceKind::Adopt,
            codec: codec("manual"),
            payload: "adopt",
            receipt_codec: codec("receipt"),
            receipt: "receipt",
            receipt_event_codec: codec("receipt-event"),
            receipt_event: "manual-receipt-event",
        };
        match missing_codec {
            "choice" => choice.codec.family = "",
            "receipt" => choice.receipt_codec.family = "",
            "receipt-event" => choice.receipt_event_codec.family = "",
            _ => unreachable!(),
        }
        let before = workflow.clone();
        let outcome = workflow.require_manual_resolution(
            &claim.authority.expect("authority"),
            Timestamp(1),
            vec![choice],
        );

        assert_eq!(outcome.outcome, AuthorityOutcome::StaleAuthority);
        assert_eq!(workflow, before);
    }
}

#[test]
fn review_regression_rejects_empty_manual_choice_codec() {
    let mut workflow = workflow();
    workflow
        .commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(0),
                plan: plan(),
            },
            &barrier_events(),
        )
        .expect("plan");
    let claim = workflow.claim_effect(EffectId(1), "worker", Timestamp(0), LeaseExpiry(10));
    let before = workflow.clone();
    let outcome = workflow.require_manual_resolution(
        &claim.authority.expect("authority"),
        Timestamp(1),
        vec![ManualChoice {
            kind: ManualChoiceKind::Adopt,
            codec: CodecRef {
                family: "",
                version: 1,
            },
            payload: "adopt",
            receipt_codec: codec("receipt"),
            receipt: "receipt",
            receipt_event_codec: codec("receipt-event"),
            receipt_event: "manual-receipt-event",
        }],
    );
    assert_eq!(outcome.outcome, AuthorityOutcome::StaleAuthority);
    assert_eq!(workflow, before);
}

#[test]
fn terminal_cancellation_delivery_is_suppressed_and_drainable() {
    let mut workflow = workflow();
    let result = workflow
        .cancel_with_compensation(
            &CancellationRequest {
                expected_workflow_version: Version(0),
                next_snapshot: "cancelled",
                next_snapshot_codec: codec("snapshot"),
                event: "cancel",
                event_codec: codec("event"),
                invalidations: vec![],
                reducer_inbox_events: vec![ReducerInboxDecl {
                    effect_id: None,
                    barrier_id: None,
                    kind: ReducerInboxKind::ReceiptAccepted,
                    event_codec: codec("cancel-event"),
                    requires_runtime_acceptance: false,
                    payload: ReducerInboxPayload::Receipt("cancel-event"),
                }],
                compensation_plan: TransitionPlan {
                    next_status: WorkflowStatus::Cancelled,
                    snapshot: "cancelled",
                    snapshot_codec: codec("snapshot"),
                    event: "cancel",
                    event_codec: codec("event"),
                    effects: vec![],
                    dependencies: vec![],
                    barriers: vec![],
                    barrier_members: vec![],
                    invalidations: vec![],
                    owed_acceptances: None,
                },
            },
            &BTreeMap::new(),
        )
        .expect("cancellation");

    assert_eq!(result.outcome, CommitOutcome::Committed);
    assert_eq!(result.reducer_events.len(), 1);
    assert_eq!(
        result.reducer_events[0].delivery_status,
        DeliveryStatus::Suppressed {
            reason: SuppressionReason::Cancelled,
        }
    );
    assert_eq!(
        drain_proof(&protocol(), [&workflow]).categories["pending_reducer_inbox"].count,
        0
    );
}

#[test]
fn terminal_cancellation_plan_cannot_declare_remaining_compensation_effects() {
    let mut workflow = workflow();
    let before = workflow.clone();
    let result = workflow.cancel_with_compensation(
        &CancellationRequest {
            expected_workflow_version: Version(0),
            next_snapshot: "cancelled",
            next_snapshot_codec: codec("snapshot"),
            event: "cancel",
            event_codec: codec("event"),
            invalidations: vec![],
            reducer_inbox_events: vec![],
            compensation_plan: TransitionPlan {
                next_status: WorkflowStatus::Cancelled,
                snapshot: "cancelled",
                snapshot_codec: codec("snapshot"),
                event: "cancel",
                event_codec: codec("event"),
                effects: vec![effect(20, EffectRole::Compensation, Generation(1))],
                dependencies: vec![],
                barriers: vec![],
                barrier_members: vec![],
                invalidations: vec![],
                owed_acceptances: None,
            },
        },
        &BTreeMap::new(),
    );

    assert_eq!(
        result,
        Err(EngineError::InvalidPlan(
            PlanError::TerminalPlanDeclaresEffects(WorkflowStatus::Cancelled)
        ))
    );
    assert_eq!(workflow, before);
}

#[test]
fn terminal_inbox_transition_cannot_install_owed_runtime_acceptance() {
    let mut workflow = workflow();
    workflow.reducer_inbox.insert(
        ReducerInboxId(1),
        ReducerInboxEvent {
            id: ReducerInboxId(1),
            effect_id: None,
            barrier_id: None,
            kind: ReducerInboxKind::ReceiptAccepted,
            event_codec: codec("receipt-event"),
            requires_runtime_acceptance: true,
            payload: ReducerInboxPayload::Receipt("receipt-event"),
            delivery_status: DeliveryStatus::Pending,
            consumed_by: None,
        },
    );
    let before = workflow.clone();
    let result = workflow.consume_reducer_inbox_atomically(
        &InboxDecisionBinding {
            inbox: vec![workflow.reducer_inbox[&ReducerInboxId(1)].clone()],
            decision: ReducerDecision {
                expected_workflow_version: Version(0),
                plan: TransitionPlan {
                    next_status: WorkflowStatus::Completed,
                    snapshot: "completed",
                    snapshot_codec: codec("snapshot"),
                    event: "complete",
                    event_codec: codec("event"),
                    effects: vec![],
                    dependencies: vec![],
                    barriers: vec![],
                    barrier_members: vec![],
                    invalidations: vec![],
                    owed_acceptances: Some(vec![OwedAcceptanceDecl {
                        reducer_inbox_id: ReducerInboxId(1),
                        source_kind: "runtime",
                        event_codec: codec("receipt-event"),
                        event: "receipt-event",
                    }]),
                },
            },
        },
        &BTreeMap::new(),
    );

    assert_eq!(result, Err(EngineError::InvalidInbox));
    assert_eq!(workflow, before);
}

#[test]
fn retry_and_compensate_manual_choices_do_not_receipt_or_satisfy_barriers() {
    for (kind, expected) in [
        (ManualChoiceKind::Retry, ManualEffectOutcome::Retry),
        (
            ManualChoiceKind::Compensate,
            ManualEffectOutcome::Compensate,
        ),
    ] {
        let mut workflow = workflow();
        workflow
            .commit_transition(
                &ReducerDecision {
                    expected_workflow_version: Version(0),
                    plan: plan(),
                },
                &barrier_events(),
            )
            .expect("plan");
        let claim = workflow.claim_effect(EffectId(1), "worker", Timestamp(0), LeaseExpiry(10));
        let choice = ManualChoice {
            kind,
            codec: codec("manual"),
            payload: "more-work",
            receipt_codec: codec("receipt"),
            receipt: "must-not-persist",
            receipt_event_codec: codec("receipt-event"),
            receipt_event: "must-not-deliver",
        };
        let resolution = workflow
            .require_manual_resolution(
                &claim.authority.expect("authority"),
                Timestamp(1),
                vec![choice.clone()],
            )
            .manual_resolution
            .expect("manual resolution");
        let outcome = workflow.resolve_manual(
            resolution.id,
            Version(1),
            "operator",
            &choice,
            ManualResolutionCommit {
                transition_codec: codec("manual-transition"),
                transition_event: "manual-more-work",
                next_status: WorkflowStatus::Active,
            },
        );

        assert_eq!(outcome.effect_outcome, Some(expected));
        assert!(workflow.effects[&EffectId(1)].receipt.is_none());
        assert!(workflow.reducer_inbox.is_empty());
        assert_eq!(
            workflow.barriers[&BarrierId(10)].status,
            BarrierStatus::Waiting
        );
        assert_eq!(workflow.effects[&EffectId(2)].status, EffectStatus::Blocked);
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn terminal_manual_receipt_and_post_terminal_barrier_are_suppressed() {
    let mut terminal_workflow = workflow();
    terminal_workflow
        .commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(0),
                plan: plan(),
            },
            &barrier_events(),
        )
        .expect("plan");
    let first =
        terminal_workflow.claim_effect(EffectId(1), "worker-1", Timestamp(0), LeaseExpiry(10));
    let first_receipt = terminal_workflow.accept_receipt(
        &first.authority.expect("authority"),
        Timestamp(1),
        Some(first.attempt.expect("attempt").id),
        ReceiptOrigin::Execution,
        codec("receipt"),
        "done-1",
        codec("receipt-event"),
        "receipt-1",
    );
    let first_inbox = terminal_workflow.reducer_inbox[&first_receipt.receipt_inbox_ids[0]].clone();
    terminal_workflow
        .consume_reducer_inbox_atomically(
            &InboxDecisionBinding {
                inbox: vec![first_inbox],
                decision: ReducerDecision {
                    expected_workflow_version: Version(1),
                    plan: TransitionPlan {
                        next_status: WorkflowStatus::Active,
                        snapshot: "first-done",
                        snapshot_codec: codec("snapshot"),
                        event: "receipt-1",
                        event_codec: codec("event"),
                        effects: vec![],
                        dependencies: vec![],
                        barriers: vec![],
                        barrier_members: vec![],
                        invalidations: vec![],
                        owed_acceptances: Some(vec![OwedAcceptanceDecl {
                            reducer_inbox_id: first_receipt.receipt_inbox_ids[0],
                            source_kind: "runtime",
                            event_codec: codec("receipt-event"),
                            event: "receipt-1",
                        }]),
                    },
                },
            },
            &BTreeMap::new(),
        )
        .expect("consume first receipt");
    let second =
        terminal_workflow.claim_effect(EffectId(2), "worker-2", Timestamp(1), LeaseExpiry(10));
    terminal_workflow.status = WorkflowStatus::Completed;
    let second_receipt = terminal_workflow.accept_receipt(
        &second.authority.expect("authority"),
        Timestamp(2),
        Some(second.attempt.expect("attempt").id),
        ReceiptOrigin::Execution,
        codec("receipt"),
        "done-2",
        codec("receipt-event"),
        "receipt-2",
    );
    let barrier = second_receipt
        .reducer_events
        .first()
        .expect("barrier event");
    assert_eq!(
        barrier.delivery_status,
        DeliveryStatus::Suppressed {
            reason: SuppressionReason::LifecycleTerminal,
        }
    );

    let mut manual = workflow();
    manual
        .commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(0),
                plan: plan(),
            },
            &barrier_events(),
        )
        .expect("manual plan");
    let claim = manual.claim_effect(EffectId(1), "worker", Timestamp(0), LeaseExpiry(10));
    let choice = ManualChoice {
        kind: ManualChoiceKind::Adopt,
        codec: codec("manual"),
        payload: "adopt",
        receipt_codec: codec("receipt"),
        receipt: "manual-receipt",
        receipt_event_codec: codec("receipt-event"),
        receipt_event: "manual-event",
    };
    let resolution = manual
        .require_manual_resolution(
            &claim.authority.expect("authority"),
            Timestamp(1),
            vec![choice.clone()],
        )
        .manual_resolution
        .expect("resolution");
    let outcome = manual.resolve_manual(
        resolution.id,
        Version(1),
        "operator",
        &choice,
        ManualResolutionCommit {
            transition_codec: codec("manual-transition"),
            transition_event: "manual-terminal",
            next_status: WorkflowStatus::Completed,
        },
    );
    let ManualEffectOutcome::Receipt { reducer_event, .. } =
        outcome.effect_outcome.expect("manual receipt outcome")
    else {
        panic!("adopt must receipt");
    };
    assert_eq!(
        reducer_event.delivery_status,
        DeliveryStatus::Suppressed {
            reason: SuppressionReason::LifecycleTerminal,
        }
    );
}

#[test]
fn deletion_compensation_can_generation_bump_from_failed_or_cancelled() {
    for status in [WorkflowStatus::Failed, WorkflowStatus::Cancelled] {
        let mut workflow = workflow();
        workflow.status = status;
        let result = workflow
            .cancel_with_compensation(
                &CancellationRequest {
                    expected_workflow_version: Version(0),
                    next_snapshot: "deleting",
                    next_snapshot_codec: codec("snapshot"),
                    event: "delete",
                    event_codec: codec("event"),
                    invalidations: vec![],
                    reducer_inbox_events: vec![],
                    compensation_plan: TransitionPlan {
                        next_status: WorkflowStatus::DeletionPending,
                        snapshot: "deleting",
                        snapshot_codec: codec("snapshot"),
                        event: "delete",
                        event_codec: codec("event"),
                        effects: vec![effect(20, EffectRole::Compensation, Generation(1))],
                        dependencies: vec![],
                        barriers: vec![],
                        barrier_members: vec![],
                        invalidations: vec![],
                        owed_acceptances: None,
                    },
                },
                &BTreeMap::new(),
            )
            .expect("deletion compensation");

        assert_eq!(result.outcome, CommitOutcome::Committed);
        assert_eq!(workflow.status, WorkflowStatus::DeletionPending);
        assert_eq!(workflow.generation, Generation(1));
        assert_eq!(
            workflow.effects[&EffectId(20)].status,
            EffectStatus::Eligible
        );
    }
}
