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
    type ManualPayload = &'static str;
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
        selector: "selector-v1",
    }
}

fn effect(effect_id: u64, role: EffectRole, generation: Generation) -> EffectDecl<&'static str> {
    EffectDecl {
        effect_id: EffectId(effect_id),
        family: "test",
        kind: "step",
        codec: codec("intent"),
        generation,
        role,
        ambiguity: EffectAmbiguity::SafeRepeatability,
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
        protocol(),
        codec("snapshot"),
        "initial",
    )
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
        protocol(),
        codec("snapshot"),
        "initial",
    );
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
        workflow.take_over_expired_claim(EffectId(1), &authority, Timestamp(9)),
        AuthorityOutcome::StaleAuthority
    );
    assert_eq!(
        workflow.take_over_expired_claim(EffectId(1), &authority, Timestamp(10)),
        AuthorityOutcome::Authorized
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
        workflow.record_observation(&authority, Timestamp(1), AttemptId(999), "saw", true),
        AuthorityOutcome::StaleAuthority
    );
    assert_eq!(
        workflow.record_observation(&authority, Timestamp(1), attempt.id, "saw", true),
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
        workflow.record_observation(&authority, Timestamp(2), attempt.id, "late", true),
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
        &ManualChoice {
            kind: ManualChoiceKind::Adopt,
            codec: codec("manual"),
            payload: "adopt",
        },
        "receipt",
        "manual-receipt-event",
    );
    assert_eq!(stale.outcome, CommitOutcome::VersionConflict);
    let invalid = workflow.resolve_manual(
        resolution.id,
        Version(1),
        &ManualChoice {
            kind: ManualChoiceKind::Retry,
            codec: codec("manual"),
            payload: "retry",
        },
        "receipt",
        "manual-receipt-event",
    );
    assert_eq!(invalid.outcome, CommitOutcome::InvalidPlan);

    let committed = workflow.resolve_manual(
        resolution.id,
        Version(1),
        &ManualChoice {
            kind: ManualChoiceKind::Adopt,
            codec: codec("manual"),
            payload: "adopt",
        },
        "receipt",
        "manual-receipt-event",
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
        &ManualChoice {
            kind: ManualChoiceKind::Retry,
            codec: codec("manual"),
            payload: "retry",
        },
        "receipt",
        "manual-receipt-event",
    );
    assert_eq!(outcome.outcome, CommitOutcome::InvalidPlan);
    assert_eq!(workflow, before);
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
        protocol(),
        codec("snapshot"),
        "initial",
    );
    workflow.record_shadow_divergence(
        WorkflowId(1),
        ShadowDivergenceKind::Snapshot,
        "snap-1".to_string(),
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
    sim.apply(SimOp::CrashClaim {
        effect_id: EffectId(1),
    });
    assert!(sim.workflow.effects[&EffectId(1)].claim.is_some());
    assert!(sim.workflow.effects[&EffectId(1)].pending_reconciliation);
    sim.workflow.effects.get_mut(&EffectId(1)).unwrap().status = EffectStatus::RetryWait;
    sim.workflow
        .effects
        .get_mut(&EffectId(1))
        .unwrap()
        .pending_reconciliation = false;
    sim.workflow
        .effects
        .get_mut(&EffectId(1))
        .unwrap()
        .declaration
        .next_eligible_at = Some(Timestamp(5));
    sim.apply(SimOp::AdvanceTime(Timestamp(5)));
    assert_eq!(
        sim.workflow.effects[&EffectId(1)].status,
        EffectStatus::Eligible
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
