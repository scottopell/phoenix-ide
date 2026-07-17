use std::collections::BTreeMap;

use proptest::prelude::*;

use crate::*;

#[derive(Debug, Clone, PartialEq, Eq)]
enum TestEvent {
    Transition(&'static str),
    Delivery(&'static str),
    RuntimeAccept(&'static str),
    RuntimeSuppress(&'static str),
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TestBarrierEvent {
    Family(&'static str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TestProfile;

impl WorkflowProfile for TestProfile {
    type Snapshot = &'static str;
    type Event = TestEvent;
    type Intent = &'static str;
    type Observation = &'static str;
    type Receipt = &'static str;
    type ReceiptReducerEvent = &'static str;
    type BarrierEvent = TestBarrierEvent;
    type ManualPayload = &'static str;

    fn runtime_start_allowed(snapshot: &Self::Snapshot) -> bool {
        *snapshot != "runtime-blocked"
    }

    fn receipt_requires_runtime_acceptance(event: &Self::ReceiptReducerEvent) -> bool {
        *event == "runtime"
    }

    fn decision_handles_delivery(item: &DeliveryItem<Self>, decision_event: &Self::Event) -> bool {
        match (&item.payload, decision_event) {
            (DeliveryPayload::Receipt(payload), TestEvent::Delivery(expected)) => payload == expected,
            (DeliveryPayload::Barrier(TestBarrierEvent::Family(payload)), TestEvent::Delivery(expected)) => payload == expected,
            _ => false,
        }
    }

    fn decision_handles_runtime_acceptance(item: &DeliveryItem<Self>, decision_event: &Self::Event) -> bool {
        matches!((&item.payload, decision_event),
            (DeliveryPayload::Receipt(payload), TestEvent::RuntimeAccept(expected)) if payload == expected)
    }

    fn decision_handles_runtime_suppression(item: &DeliveryItem<Self>, decision_event: &Self::Event) -> bool {
        matches!((&item.payload, decision_event),
            (DeliveryPayload::Receipt(payload), TestEvent::RuntimeSuppress(expected)) if payload == expected)
    }
}

fn profile() -> ProfileRef {
    ProfileRef {
        profile_kind: "test",
        profile_version: 1,
    }
}

fn profile_v2() -> ProfileRef {
    ProfileRef {
        profile_kind: "test-v2",
        profile_version: 2,
    }
}

fn codec(family: &'static str) -> CodecRef {
    CodecRef { family, version: 1 }
}

fn acceptance() -> AcceptanceProfile {
    AcceptanceProfile {
        profile: profile(),
        supported_codecs: SupportedCodecRegistry::new([
            codec("snapshot"),
            codec("event"),
            codec("intent"),
            codec("receipt"),
            codec("barrier"),
            codec("manual"),
        ])
        .expect("codecs"),
        runtime_acceptance_enabled: true,
        external_acceptance_enabled: true,
    }
}

fn workflow() -> WorkflowState<TestProfile> {
    WorkflowState::new(WorkflowId(1), &profile(), acceptance(), codec("snapshot"), "initial")
        .expect("workflow")
}

fn effect_decl(
    effect_id: u64,
    role: EffectRole,
    capability: ExecutionCapability,
    generation: Generation,
) -> EffectDecl<&'static str> {
    EffectDecl {
        effect_id: EffectId(effect_id),
        family: "test",
        kind: "step",
        codec: codec("intent"),
        generation,
        role,
        capability,
        intent: "intent",
        next_eligible_at: None,
        destructive_resource: None,
    }
}

fn delivery_decl(payload: &'static str, requires_runtime_acceptance: bool) -> DeliveryDecl<TestProfile> {
    DeliveryDecl {
        effect_id: None,
        barrier_id: None,
        consumer_kind: "reducer",
        event_codec: codec("receipt"),
        requires_runtime_acceptance,
        payload: DeliveryPayload::Receipt(payload),
    }
}

fn base_plan() -> TransitionPlan<TestProfile> {
    TransitionPlan {
        next_status: WorkflowStatus::Active,
        snapshot: "next",
        snapshot_codec: codec("snapshot"),
        event: TestEvent::Transition("advance"),
        event_codec: codec("event"),
        effects: vec![],
        dependencies: vec![],
        barriers: vec![],
        barrier_members: vec![],
        invalidations: vec![],
        deliveries: vec![],
        schedules: vec![],
    }
}

fn decision(expected_workflow_version: Version, plan: TransitionPlan<TestProfile>) -> ReducerDecision<TestProfile> {
    ReducerDecision {
        expected_workflow_version,
        plan,
    }
}

fn claim(workflow: &mut WorkflowState<TestProfile>, effect_id: u64, now: u64, lease_until: Option<u64>) -> ClaimResult {
    workflow.claim_effect(
        EffectId(effect_id),
        Timestamp(now),
        lease_until.and_then(LeaseExpiry::finite),
    )
}

fn begin_observation_effect(
    capability: ExecutionCapability,
    lease_until: Option<u64>,
) -> (WorkflowState<TestProfile>, AttemptAuthority) {
    let mut workflow = workflow();
    let mut plan = base_plan();
    plan.effects.push(effect_decl(1, EffectRole::Required, capability, Generation(0)));
    workflow
        .commit_transition(&decision(Version(0), plan), &BTreeMap::new())
        .expect("commit")
        ;
    let claim = claim(&mut workflow, 1, 0, lease_until);
    (workflow, claim.authority.expect("authority"))
}

#[test]
fn transition_commit_is_compare_and_swap_atomic() {
    let mut workflow = workflow();
    let first = workflow
        .commit_transition(&decision(Version(0), base_plan()), &BTreeMap::new())
        .expect("first commit");
    assert_eq!(first.outcome, CommitOutcome::Committed);
    assert_eq!(workflow.version, Version(1));

    let stale = workflow
        .commit_transition(&decision(Version(0), base_plan()), &BTreeMap::new())
        .expect("stale result");
    assert_eq!(stale.outcome, CommitOutcome::VersionConflict);
    assert_eq!(workflow.version, Version(1));
    assert_eq!(workflow.transition_log.len(), 1);
}

#[test]
fn dag_dependencies_gate_eligibility_and_cycles_are_rejected() {
    let mut workflow = workflow();
    let mut plan = base_plan();
    plan.effects = vec![
        effect_decl(1, EffectRole::Required, ExecutionCapability::SafelyRepeatable, Generation(0)),
        effect_decl(2, EffectRole::Required, ExecutionCapability::SafelyRepeatable, Generation(0)),
    ];
    plan.dependencies.push(DependencyDecl {
        effect_id: EffectId(2),
        depends_on_effect_id: EffectId(1),
    });
    workflow
        .commit_transition(&decision(Version(0), plan), &BTreeMap::new())
        .expect("commit");
    assert_eq!(workflow.effects[&EffectId(1)].status, EffectStatus::Eligible);
    assert_eq!(workflow.effects[&EffectId(2)].status, EffectStatus::Blocked);

    let claim = claim(&mut workflow, 1, 0, None);
    let authority = claim.authority.expect("authority");
    let acceptance = workflow.accept_receipt(
        &authority,
        Timestamp(0),
        Some(authority.attempt_id),
        ReceiptOrigin::Execution,
        codec("receipt"),
        "done-1",
        codec("receipt"),
        "done-1",
    );
    assert_eq!(acceptance.outcome, AuthorityOutcome::Authorized);
    workflow.refresh_eligibility(Timestamp(0));
    assert_eq!(workflow.effects[&EffectId(2)].status, EffectStatus::Eligible);

    let mut cyclic = base_plan();
    cyclic.effects = vec![
        effect_decl(1, EffectRole::Required, ExecutionCapability::SafelyRepeatable, Generation(0)),
        effect_decl(2, EffectRole::Required, ExecutionCapability::SafelyRepeatable, Generation(0)),
    ];
    cyclic.dependencies = vec![
        DependencyDecl {
            effect_id: EffectId(1),
            depends_on_effect_id: EffectId(2),
        },
        DependencyDecl {
            effect_id: EffectId(2),
            depends_on_effect_id: EffectId(1),
        },
    ];
    assert_eq!(
        validate_plan(WorkflowStatus::Active, &cyclic, &BTreeMap::new(), &acceptance().supported_codecs),
        Err(PlanError::DependencyCycle)
    );
}

#[test]
fn barriers_require_matching_receipt_family_and_generation() {
    let mut workflow = workflow();
    let mut plan = base_plan();
    plan.effects = vec![
        effect_decl(1, EffectRole::Required, ExecutionCapability::SafelyRepeatable, Generation(0)),
        effect_decl(2, EffectRole::Compensation, ExecutionCapability::SafelyRepeatable, Generation(0)),
    ];
    plan.barriers.push(BarrierDecl {
        barrier_id: BarrierId(7),
        reducer_event_codec: codec("barrier"),
    });
    plan.barrier_members = vec![
        BarrierMemberDecl {
            barrier_id: BarrierId(7),
            effect_id: EffectId(1),
            receipt_family: ReceiptFamily::CurrentGenerationEffect,
        },
        BarrierMemberDecl {
            barrier_id: BarrierId(7),
            effect_id: EffectId(2),
            receipt_family: ReceiptFamily::CompensationEffect,
        },
    ];
    let events = BTreeMap::from([(BarrierId(7), TestBarrierEvent::Family("barrier-ready"))]);
    workflow
        .commit_transition(&decision(Version(0), plan), &events)
        .expect("commit");
    assert_eq!(workflow.barriers[&BarrierId(7)].required_members.len(), 2);
    assert_eq!(workflow.barriers[&BarrierId(7)].status, BarrierStatus::Waiting);

    let mut mismatch = base_plan();
    mismatch.effects.push(effect_decl(
        1,
        EffectRole::Compensation,
        ExecutionCapability::SafelyRepeatable,
        Generation(0),
    ));
    mismatch.barriers.push(BarrierDecl {
        barrier_id: BarrierId(9),
        reducer_event_codec: codec("barrier"),
    });
    mismatch.barrier_members.push(BarrierMemberDecl {
        barrier_id: BarrierId(9),
        effect_id: EffectId(1),
        receipt_family: ReceiptFamily::CurrentGenerationEffect,
    });
    let events = BTreeMap::from([(BarrierId(9), TestBarrierEvent::Family("bad"))]);
    assert_eq!(
        validate_plan(WorkflowStatus::Active, &mismatch, &events, &acceptance().supported_codecs),
        Err(PlanError::BarrierReceiptFamilyMismatch {
            barrier_id: BarrierId(9),
            effect_id: EffectId(1),
        })
    );
}

#[test]
fn process_incarnation_and_attempt_ids_fence_authority() {
    let (mut workflow, authority) = begin_observation_effect(
        ExecutionCapability::ReclaimableObservation,
        Some(5),
    );
    let mut stale_process = authority.clone();
    stale_process.process_incarnation = ProcessIncarnation(99);
    assert_eq!(
        workflow.record_observation(&stale_process, Timestamp(1), Timestamp(1), authority.attempt_id, codec("receipt"), "obs"),
        AuthorityOutcome::StaleAuthority
    );

    let mut stale_attempt = authority.clone();
    stale_attempt.attempt_id = AttemptId(authority.attempt_id.0 + 1);
    assert_eq!(
        workflow.record_observation(&stale_attempt, Timestamp(1), Timestamp(1), stale_attempt.attempt_id, codec("receipt"), "obs"),
        AuthorityOutcome::StaleAuthority
    );

    assert_eq!(workflow.effects[&EffectId(1)].stale_observations.len(), 2);
    assert_eq!(workflow.record_observation(&authority, Timestamp(1), Timestamp(1), authority.attempt_id, codec("receipt"), "obs"), AuthorityOutcome::Authorized);
}

#[test]
fn lease_admission_renewal_and_expiry_follow_capability_shape() {
    let mut no_lease = workflow();
    let mut plan = base_plan();
    plan.effects.push(effect_decl(
        1,
        EffectRole::Required,
        ExecutionCapability::ReclaimableObservation,
        Generation(0),
    ));
    no_lease.commit_transition(&decision(Version(0), plan), &BTreeMap::new()).expect("commit");
    assert_eq!(claim(&mut no_lease, 1, 0, None).outcome, ClaimOutcome::AuthorityConflict);

    let (mut workflow, authority) = begin_observation_effect(
        ExecutionCapability::ReclaimableObservation,
        Some(5),
    );
    assert_eq!(
        workflow.renew_lease(&authority, Timestamp(1), LeaseExpiry::finite(8).expect("finite"))
            .outcome,
        AuthorityOutcome::Authorized
    );
    assert_eq!(workflow.effects[&EffectId(1)].reclaimable_lease.as_ref().map(|l| l.lease_until), Some(LeaseExpiry(8)));
    assert_eq!(workflow.expire_lease(EffectId(1), authority.attempt_id, Timestamp(7)), AuthorityOutcome::StaleAuthority);
    assert_eq!(workflow.expire_lease(EffectId(1), authority.attempt_id, Timestamp(8)), AuthorityOutcome::Authorized);
    assert_eq!(workflow.effects[&EffectId(1)].status, EffectStatus::Eligible);
}

#[test]
fn expiry_does_not_allow_unsafe_repeat_for_submission_capabilities() {
    for capability in [
        ExecutionCapability::ObservableSubmission {
            stable_command_id: StableCommandId(1),
        },
        ExecutionCapability::ManualOnAmbiguity,
    ] {
        let (mut workflow, authority) = begin_observation_effect(capability, None);
        assert_eq!(workflow.expire_lease(EffectId(1), authority.attempt_id, Timestamp(1)), AuthorityOutcome::StaleAuthority);
        let effect = workflow.effects.get_mut(&EffectId(1)).expect("effect");
        effect.reclaimable_lease = Some(ReclaimableLease {
            attempt_id: authority.attempt_id,
            lease_until: LeaseExpiry::finite(1).expect("finite"),
        });
        assert_eq!(workflow.expire_lease(EffectId(1), authority.attempt_id, Timestamp(1)), AuthorityOutcome::Authorized);
        assert_eq!(workflow.effects[&EffectId(1)].status, EffectStatus::AmbiguityWait);
        assert_eq!(claim(&mut workflow, 1, 2, None).outcome, ClaimOutcome::Ineligible);
    }
}

#[test]
fn stable_command_submission_is_not_auto_repeatable_after_ambiguity() {
    let (mut workflow, authority) = begin_observation_effect(
        ExecutionCapability::IdempotentSubmission {
            stable_command_id: StableCommandId(7),
        },
        None,
    );
    assert_eq!(workflow.schedule_retry(&authority, Timestamp(0), Timestamp(10)).outcome, AuthorityOutcome::Authorized);
    assert_eq!(workflow.effects[&EffectId(1)].status, EffectStatus::RetryWait);
    workflow.refresh_eligibility(Timestamp(10));
    assert_eq!(workflow.effects[&EffectId(1)].status, EffectStatus::Eligible);
    let second = claim(&mut workflow, 1, 10, None);
    assert_eq!(second.outcome, ClaimOutcome::Started);
    assert_eq!(second.attempt.expect("attempt").ordinal, 2);
}

#[test]
fn receipt_and_delivery_idempotency_is_single_winner() {
    let (mut workflow, authority) = begin_observation_effect(
        ExecutionCapability::SafelyRepeatable,
        None,
    );
    let first = workflow.accept_receipt(
        &authority,
        Timestamp(0),
        Some(authority.attempt_id),
        ReceiptOrigin::Execution,
        codec("receipt"),
        "done",
        codec("receipt"),
        "done",
    );
    assert_eq!(first.outcome, AuthorityOutcome::Authorized);
    let duplicate = workflow.accept_receipt(
        &authority,
        Timestamp(0),
        Some(authority.attempt_id),
        ReceiptOrigin::Execution,
        codec("receipt"),
        "done",
        codec("receipt"),
        "done",
    );
    assert_eq!(duplicate.outcome, AuthorityOutcome::StaleAuthority);
    assert_eq!(workflow.deliveries.len(), 1);
}

#[test]
fn canonical_delivery_consumption_is_atomic_and_duplicate_safe() {
    let mut workflow = workflow();
    let mut plan = base_plan();
    plan.deliveries.push(delivery_decl("deliver-me", false));
    let committed = workflow
        .commit_transition(&decision(Version(0), plan), &BTreeMap::new())
        .expect("commit");
    let item = committed.deliveries.into_iter().next().expect("delivery");

    let consume = workflow
        .consume_deliveries(&DeliveryDecisionBinding {
            items: vec![item.clone()],
            decision: decision(
                Version(1),
                TransitionPlan {
                    event: TestEvent::Delivery("deliver-me"),
                    ..base_plan()
                },
            ),
        })
        .expect("consume");
    assert_eq!(consume.outcome, CommitOutcome::Committed);
    assert_eq!(workflow.deliveries[&item.id].status, DeliveryStatus::Accepted);

    let dup = workflow.consume_deliveries(&DeliveryDecisionBinding {
        items: vec![item],
        decision: decision(
            workflow.version,
            TransitionPlan {
                event: TestEvent::Delivery("deliver-me"),
                ..base_plan()
            },
        ),
    });
    assert_eq!(dup, Err(EngineError::InvalidInbox));
}

#[test]
fn runtime_acceptance_is_atomic_and_duplicate_safe() {
    let mut workflow = workflow();
    let mut plan = base_plan();
    plan.deliveries.push(delivery_decl("runtime", true));
    let committed = workflow
        .commit_transition(&decision(Version(0), plan), &BTreeMap::new())
        .expect("commit");
    let item = committed.deliveries.into_iter().next().expect("delivery");

    let accepted = workflow
        .accept_runtime_delivery(
            item.id,
            &decision(
                Version(1),
                TransitionPlan {
                    event: TestEvent::RuntimeAccept("runtime"),
                    ..base_plan()
                },
            ),
            false,
        )
        .expect("accept runtime");
    assert_eq!(accepted.outcome, CommitOutcome::Committed);
    assert_eq!(workflow.deliveries[&item.id].runtime_acceptance_status, Some(RuntimeAcceptanceStatus::Accepted));

    assert_eq!(
        workflow.accept_runtime_delivery(
            item.id,
            &decision(
                workflow.version,
                TransitionPlan {
                    event: TestEvent::RuntimeAccept("runtime"),
                    ..base_plan()
                },
            ),
            false,
        ),
        Err(EngineError::InvalidInbox)
    );
}

#[test]
fn cancellation_bumps_generation_invalidates_and_commits_compensation() {
    let mut workflow = workflow();
    let mut initial = base_plan();
    initial.effects.push(effect_decl(
        1,
        EffectRole::Required,
        ExecutionCapability::SafelyRepeatable,
        Generation(0),
    ));
    workflow.commit_transition(&decision(Version(0), initial), &BTreeMap::new()).expect("commit");

    let mut compensation = base_plan();
    compensation.next_status = WorkflowStatus::Cancelling;
    compensation.effects.push(effect_decl(
        2,
        EffectRole::Compensation,
        ExecutionCapability::SafelyRepeatable,
        Generation(1),
    ));
    let request = CancellationRequest {
        expected_workflow_version: Version(1),
        next_snapshot: "cancel-snapshot",
        next_snapshot_codec: codec("snapshot"),
        event: TestEvent::Cancel,
        event_codec: codec("event"),
        invalidations: vec![EffectInvalidationDecl { effect_id: EffectId(1) }],
        terminal_receipt: None,
        compensation_plan: compensation,
    };
    let result = workflow.cancel_with_compensation(&request, &BTreeMap::new()).expect("cancel");
    assert_eq!(result.outcome, CommitOutcome::Committed);
    assert_eq!(workflow.generation, Generation(1));
    assert_eq!(workflow.effects[&EffectId(1)].status, EffectStatus::Invalidated);
    assert_eq!(workflow.effects[&EffectId(2)].status, EffectStatus::Eligible);
}

#[test]
fn manual_ambiguity_produces_resolution_and_terminal_choice_emits_receipt() {
    let (mut workflow, authority) = begin_observation_effect(
        ExecutionCapability::ManualOnAmbiguity,
        None,
    );
    assert_eq!(workflow.record_observation(&authority, Timestamp(0), Timestamp(0), authority.attempt_id, codec("receipt"), "evidence"), AuthorityOutcome::Authorized);
    let choices = vec![ManualChoice {
        kind: ManualChoiceKind::AcceptAsTerminal,
        codec: codec("manual"),
        payload: "accept",
        receipt_codec: codec("receipt"),
        receipt: "terminal",
        receipt_event_codec: codec("receipt"),
        receipt_event: "terminal",
    }];
    let reconciliation = workflow.require_manual_resolution(&authority, Timestamp(1), choices.clone());
    assert_eq!(reconciliation.outcome, AuthorityOutcome::Authorized);
    let resolution = reconciliation.manual_resolution.expect("resolution");
    assert_eq!(resolution.evidence.len(), 1);

    let outcome = workflow.resolve_manual(
        resolution.id,
        Version(1),
        "reviewer",
        &choices[0],
        &ManualResolutionCommit {
            transition_codec: codec("event"),
            transition_event: TestEvent::Transition("manual"),
            next_status: WorkflowStatus::Active,
            retry_at: None,
            compensation_effects: vec![],
            compensation_dependencies: vec![],
        },
    );
    assert_eq!(outcome.outcome, CommitOutcome::Committed);
    match outcome.effect_outcome.expect("effect outcome") {
        ManualEffectOutcome::Receipt { receipt, reducer_event } => {
            assert_eq!(receipt.receipt, "terminal");
            assert_eq!(reducer_event.payload, DeliveryPayload::Receipt("terminal"));
        }
        other => panic!("unexpected outcome: {other:?}"),
    }
}

#[test]
fn typed_migration_preserves_incompatible_active_workflow() {
    let mut workflow = workflow();
    let mut plan = base_plan();
    plan.effects.push(effect_decl(
        1,
        EffectRole::Required,
        ExecutionCapability::SafelyRepeatable,
        Generation(0),
    ));
    workflow.commit_transition(&decision(Version(0), plan), &BTreeMap::new()).expect("commit");
    assert_eq!(workflow.migrate_profile(&profile_v2(), Timestamp(7)), ProfileMigrationOutcome::Incompatible);
    let incompatible = workflow.incompatible.expect("marker");
    assert_eq!(incompatible.disposition, "manual-preservation");
    assert_eq!(incompatible.detected_at, Timestamp(7));
}

#[test]
fn coalesce_latest_schedule_advances_and_resets() {
    let mut workflow = workflow();
    let mut plan = base_plan();
    plan.schedules.push(ScheduleDecl {
        schedule_id: ScheduleId(11),
        policy: SchedulePolicy::CoalesceLatest,
        next_eligible_at: Timestamp(5),
        key: "cron",
    });
    workflow.commit_transition(&decision(Version(0), plan), &BTreeMap::new()).expect("commit");
    assert_eq!(workflow.advance_schedule(ScheduleId(11), Timestamp(4)), None);
    let due = workflow.advance_schedule(ScheduleId(11), Timestamp(5)).expect("due");
    assert_eq!(due.status, ScheduleStatus::Due);
    let reset = workflow.complete_schedule_occurrence(ScheduleId(11), Timestamp(9)).expect("reset");
    assert_eq!(reset.status, ScheduleStatus::Idle);
    assert_eq!(reset.next_eligible_at, Timestamp(9));
}

#[test]
fn wake_profile_delivery_and_runtime_mapping_matches_events() {
    use crate::wake_profile::*;

    let terminal = WakeTerminalPayload::Fired {
        contract_id: "c".into(),
        resource: WakeResourceIdentity::Subagent(SubagentResourceIdentity {
            child_conversation_id: "child".into(),
        }),
        evidence: WakeTerminalEvidence::Subagent(SubagentTerminalEvidence {
            identity: SubagentResourceIdentity {
                child_conversation_id: "child".into(),
            },
            occurred_at: Timestamp(1),
            persisted_child_terminal_record: PersistedChildTerminalRecord::new("record").expect("record"),
            outcome: SubagentTerminalOutcome::SubmitResult {
                result: "ok".into(),
            },
        }),
        resolved_at: Timestamp(2),
    };
    let item = DeliveryItem::<WakeProfile> {
        id: DeliveryId(1),
        effect_id: Some(REGISTRATION_EFFECT_ID),
        barrier_id: None,
        consumer_kind: "reducer",
        event_codec: terminal_codec(),
        requires_runtime_acceptance: false,
        payload: DeliveryPayload::Receipt(terminal.clone()),
        status: DeliveryStatus::Pending,
        runtime_acceptance_status: None,
        suppression_reason: None,
        accepted_by: None,
    };
    assert!(WakeProfile::decision_handles_delivery(&item, &WakeRegistrationEvent::TerminalProjected { terminal: Box::new(terminal.clone()) }));
    assert!(WakeProfile::decision_handles_runtime_acceptance(&item, &WakeRegistrationEvent::RuntimeAccepted { terminal: Box::new(terminal.clone()) }));
    assert!(WakeProfile::decision_handles_runtime_suppression(&item, &WakeRegistrationEvent::RuntimeSuppressed { terminal: Box::new(terminal) }));
}

proptest! {
    #[test]
    fn version_conflicts_never_mutate_state(stale_version in 0u64..5) {
        let mut workflow = workflow();
        let baseline = workflow.clone();
        let _ = workflow.commit_transition(&decision(Version(stale_version + 1), base_plan()), &BTreeMap::new()).expect("result");
        prop_assert_eq!(workflow, baseline);
    }

    #[test]
    fn lease_liveness_is_strict(now in 0u64..1000, delta in 0u64..5) {
        let expiry = LeaseExpiry::finite(now + delta + 1).expect("finite");
        prop_assert!(expiry.is_live_at(Timestamp(now)));
        prop_assert_eq!(expiry.is_live_at(Timestamp(now + delta + 1)), false);
    }

    #[test]
    fn dependency_chain_unlocks_only_prefix(receipted_prefix in 0usize..4) {
        let mut workflow = workflow();
        let mut plan = base_plan();
        for id in 1..=4 {
            plan.effects.push(effect_decl(id, EffectRole::Required, ExecutionCapability::SafelyRepeatable, Generation(0)));
            if id > 1 {
                plan.dependencies.push(DependencyDecl {
                    effect_id: EffectId(id),
                    depends_on_effect_id: EffectId(id - 1),
                });
            }
        }
        workflow.commit_transition(&decision(Version(0), plan), &BTreeMap::new()).expect("commit");
        for id in 1..=receipted_prefix.min(4) as u64 {
            workflow.effects.get_mut(&EffectId(id)).expect("effect").status = EffectStatus::Receipted;
        }
        workflow.refresh_eligibility(Timestamp(0));
        for id in 1..=4u64 {
            let status = workflow.effects[&EffectId(id)].status;
            if id <= receipted_prefix.min(4) as u64 + 1 {
                prop_assert_ne!(status, EffectStatus::Blocked);
            } else {
                prop_assert_eq!(status, EffectStatus::Blocked);
            }
        }
    }
}
