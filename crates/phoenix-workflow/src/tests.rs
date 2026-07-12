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
    assert_eq!(validate_plan(&plan(), &barrier_events()), Ok(()));
}

#[test]
fn rejects_duplicate_effect_ids() {
    let mut plan = plan();
    plan.effects
        .push(effect(1, EffectRole::Required, Generation(0)));
    assert_eq!(
        validate_plan(&plan, &barrier_events()),
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
        validate_plan(&plan, &barrier_events()),
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
        workflow.record_observation(&authority, Timestamp(1), AttemptId(999), "saw"),
        AuthorityOutcome::StaleAuthority
    );
    assert_eq!(
        workflow.record_observation(&authority, Timestamp(1), attempt.id, "saw"),
        AuthorityOutcome::Authorized
    );
    let accepted = workflow.accept_receipt(
        &authority,
        Timestamp(2),
        Some(AttemptId(999)),
        ReceiptOrigin::Execution,
        "done",
        "receipt-event",
    );
    assert_eq!(accepted.outcome, AuthorityOutcome::StaleAuthority);
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
        workflow.record_observation(&authority, Timestamp(2), attempt.id, "late"),
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
        "done-1",
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
        "done-2",
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
        },
        ManualResolutionCommit {
            transition_codec: codec("manual-transition"),
            transition_event: "manual-transition",
            receipt: "receipt",
            receipt_event: "manual-receipt-event",
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
        },
        ManualResolutionCommit {
            transition_codec: codec("manual-transition"),
            transition_event: "manual-transition",
            receipt: "receipt",
            receipt_event: "manual-receipt-event",
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
        },
        ManualResolutionCommit {
            transition_codec: codec("manual-transition"),
            transition_event: "manual-transition",
            receipt: "receipt",
            receipt_event: "manual-receipt-event",
        },
    );
    assert_eq!(committed.outcome, CommitOutcome::Committed);
    let receipt = committed.receipt.expect("manual receipt persisted");
    assert_eq!(receipt.origin, ReceiptOrigin::Manual);
    assert_eq!(receipt.authority.worker_id, "manual");
    assert_eq!(receipt.authority.declared_workflow_version, Version(1));
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
        },
        ManualResolutionCommit {
            transition_codec: codec("manual-transition"),
            transition_event: "manual-transition",
            receipt: "receipt",
            receipt_event: "manual-receipt-event",
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
fn owed_acceptance_requires_exact_consumed_inbox_link() {
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
        "done",
        "receipt-event",
    );
    let inbox_id = accepted.receipt_inbox_ids[0];
    let mut consume_plan = TransitionPlan {
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
            event: "runtime-event",
        }]),
    };
    let result = workflow
        .consume_reducer_inbox_atomically(
            &[inbox_id],
            &ReducerDecision {
                expected_workflow_version: Version(1),
                plan: consume_plan.clone(),
            },
            &BTreeMap::new(),
        )
        .expect("consume succeeds");
    assert_eq!(result.outcome, CommitOutcome::Committed);
    let owed = workflow
        .owed_acceptances
        .values()
        .next()
        .expect("owed created");
    assert_eq!(owed.reducer_inbox_id, inbox_id);

    consume_plan.owed_acceptances = Some(vec![OwedAcceptanceDecl {
        reducer_inbox_id: ReducerInboxId(999),
        source_kind: "wake",
        event: "runtime-event",
    }]);
    let rejected = workflow.consume_reducer_inbox_atomically(
        &[],
        &ReducerDecision {
            expected_workflow_version: Version(2),
            plan: consume_plan,
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
        snapshot: "cancelled",
        snapshot_codec: codec("snapshot"),
        event: "cancel",
        event_codec: codec("event"),
        effects: vec![effect(20, EffectRole::Compensation, Generation(1))],
        dependencies: vec![],
        barriers: vec![BarrierDecl {
            barrier_id: BarrierId(11),
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
                compensation_plan,
            },
            &cancel_events,
        )
        .expect("cancel succeeds");
    assert_eq!(workflow.generation, Generation(1));
    assert_eq!(
        workflow.renew_claim(&authority, Timestamp(1), LeaseExpiry(30)),
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
            compensation_plan: TransitionPlan {
                snapshot: "cancelled",
                snapshot_codec: codec("snapshot"),
                event: "cancel",
                event_codec: codec("event"),
                effects: vec![effect(20, EffectRole::Compensation, Generation(1))],
                dependencies: vec![],
                barriers: vec![BarrierDecl {
                    barrier_id: BarrierId(11),
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
                compensation_plan: TransitionPlan {
                    snapshot: "cancelled",
                    snapshot_codec: codec("snapshot"),
                    event: "cancel",
                    event_codec: codec("event"),
                    effects: vec![effect(20, EffectRole::Compensation, Generation(1))],
                    dependencies: vec![],
                    barriers: vec![BarrierDecl {
                        barrier_id: BarrierId(11),
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
        },
        ManualResolutionCommit {
            transition_codec: codec("manual-transition"),
            transition_event: "manual-transition",
            receipt: "receipt",
            receipt_event: "manual-receipt-event",
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
        "receipt",
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
        },
        ManualResolutionCommit {
            transition_codec: codec("manual-transition"),
            transition_event: "manual-transition",
            receipt: "receipt",
            receipt_event: "manual-receipt-event",
        },
    );
    assert_eq!(
        committed.receipt.expect("manual receipt persisted").origin,
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
        "done",
        "receipt-event",
    );
    let inbox_id = accepted.receipt_inbox_ids[0];

    let duplicate = workflow.consume_reducer_inbox_atomically(
        &[inbox_id, inbox_id],
        &ReducerDecision {
            expected_workflow_version: Version(1),
            plan: TransitionPlan {
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
                        event: "runtime-event-a",
                    },
                    OwedAcceptanceDecl {
                        reducer_inbox_id: inbox_id,
                        source_kind: "wake",
                        event: "runtime-event-b",
                    },
                ]),
            },
        },
        &BTreeMap::new(),
    );
    assert_eq!(duplicate, Err(EngineError::InvalidInbox));
    assert_eq!(workflow.version, Version(1));
    assert!(workflow.owed_acceptances.is_empty());

    let missing = workflow.consume_reducer_inbox_atomically(
        &[inbox_id],
        &ReducerDecision {
            expected_workflow_version: Version(1),
            plan: TransitionPlan {
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
        &BTreeMap::new(),
    );
    assert_eq!(missing, Err(EngineError::InvalidInbox));
    assert_eq!(workflow.version, Version(1));
    assert!(workflow.owed_acceptances.is_empty());
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
        "done",
        "receipt-event",
    );
    let inbox_id = receipt.receipt_inbox_ids[0];
    let consume = workflow.consume_reducer_inbox_atomically(
        &[inbox_id],
        &ReducerDecision {
            expected_workflow_version: Version(1),
            plan: TransitionPlan {
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
                    event: "runtime-event",
                }]),
            },
        },
        &BTreeMap::new(),
    );
    assert_eq!(consume, Err(EngineError::InvalidInbox));
    assert_eq!(workflow.version, Version(1));
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
            &[],
            &ReducerDecision {
                expected_workflow_version: Version(99),
                plan: invalid.clone(),
            },
            &barrier_events(),
        )
        .expect("cas handled");
    assert_eq!(consume.outcome, CommitOutcome::VersionConflict);

    let runtime = workflow
        .runtime_accept_atomically(
            OwedAcceptanceId(1),
            &ReducerDecision {
                expected_workflow_version: Version(99),
                plan: invalid.clone(),
            },
            &barrier_events(),
        )
        .expect("cas handled");
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
        "done",
        "receipt-event",
    );
    let inbox_id = receipt.receipt_inbox_ids[0];
    workflow
        .consume_reducer_inbox_atomically(
            &[inbox_id],
            &ReducerDecision {
                expected_workflow_version: Version(1),
                plan: TransitionPlan {
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
                        event: "runtime-event",
                    }]),
                },
            },
            &BTreeMap::new(),
        )
        .expect("owed recorded");
    let owed_id = *workflow.owed_acceptances.keys().next().expect("owed");
    let decision = ReducerDecision {
        expected_workflow_version: workflow.version,
        plan: TransitionPlan {
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
        .runtime_accept_atomically(owed_id, &decision, &BTreeMap::new())
        .expect("first acceptance commits");
    assert_eq!(first.outcome, CommitOutcome::Committed);
    let accepted = first.owed_acceptance.expect("accepted obligation");
    let retry = workflow
        .runtime_accept_atomically(owed_id, &decision, &BTreeMap::new())
        .expect("retry is idempotent");
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
        "done",
        "receipt-event",
    );
    let inbox_id = accepted.receipt_inbox_ids[0];
    workflow
        .consume_reducer_inbox_atomically(
            &[inbox_id],
            &ReducerDecision {
                expected_workflow_version: Version(1),
                plan: TransitionPlan {
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
                        event: "runtime-event",
                    }]),
                },
            },
            &BTreeMap::new(),
        )
        .expect("owed recorded");
    let owed_id = *workflow.owed_acceptances.keys().next().expect("owed id");
    let suppressed = workflow
        .suppress_runtime_acceptance_atomically(
            owed_id,
            &ReducerDecision {
                expected_workflow_version: Version(2),
                plan: TransitionPlan {
                    snapshot: "suppressed",
                    snapshot_codec: codec("snapshot"),
                    event: "suppress",
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
        "done",
        "receipt-event",
    );
    assert_eq!(
        workflow.commit_transition(
            &ReducerDecision {
                expected_workflow_version: Version(1),
                plan: TransitionPlan {
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
    assert_eq!(outcome.outcome, AuthorityOutcome::Authorized);
    assert_eq!(
        outcome.decision,
        Some(ReconciliationDecision::RequestManualResolution)
    );
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
        },
        ManualResolutionCommit {
            transition_codec: codec("manual-transition"),
            transition_event: "manual-transition",
            receipt: "receipt",
            receipt_event: "manual-receipt-event",
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
        },
        ManualResolutionCommit {
            transition_codec: codec("manual-transition"),
            transition_event: "manual-transition",
            receipt: "receipt",
            receipt_event: "manual-receipt-event",
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
        "done",
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
                compensation_plan: TransitionPlan {
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
    let proof = drain_proof(&workflow);
    let categories = exact_drain_categories();
    for category in categories {
        assert!(proof.categories.contains_key(category));
    }
    assert_eq!(proof.selector, "selector-v1");
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
        WorkflowId(1),
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
        workflow.shadow_divergences[0].action,
        DivergenceAction::HaltAcceptance
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
    assert_eq!(
        sim.workflow
            .schedule_retry(&reconciliation, Timestamp(10), Timestamp(15))
            .outcome,
        AuthorityOutcome::Authorized
    );
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
            snapshot: "next",
            snapshot_codec: codec("snapshot"),
            event: "evt",
            event_codec: codec("event"),
            effects,
            dependencies: dependencies.clone(),
            barriers: vec![BarrierDecl { barrier_id: BarrierId(10) }],
            barrier_members: vec![
                BarrierMemberDecl { barrier_id: BarrierId(10), effect_id: EffectId(1), receipt_family: ReceiptFamily::CurrentGenerationEffect },
            ],
            invalidations: vec![],
            owed_acceptances: None,
        };
        let result = validate_plan(&plan, &BTreeMap::from([(BarrierId(10), "barrier")]));
        let has_two_cycle = dependencies.iter().any(|left| {
            dependencies.iter().any(|right| {
                left.effect_id == right.depends_on_effect_id && left.depends_on_effect_id == right.effect_id
            })
        });
        prop_assert!(!(has_two_cycle && result.is_ok()));
    }
}
