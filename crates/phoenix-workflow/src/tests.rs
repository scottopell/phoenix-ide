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

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
struct RuntimeOnlyProfile;

impl WorkflowProfile for RuntimeOnlyProfile {
    type RuntimeAcceptance = RuntimeAcceptanceEnabled;
    type ExternalAcceptance = ExternalAcceptanceDisabled;
    type Snapshot = &'static str;
    type Event = TestEvent;
    type Intent = &'static str;
    type Observation = &'static str;
    type Receipt = &'static str;
    type ReceiptReducerEvent = &'static str;
    type BarrierEvent = TestBarrierEvent;
    type ManualPayload = &'static str;

    fn runtime_start_allowed(snapshot: &Self::Snapshot) -> bool {
        TestProfile::runtime_start_allowed(snapshot)
    }
    fn receipt_requires_runtime_acceptance(event: &Self::ReceiptReducerEvent) -> bool {
        TestProfile::receipt_requires_runtime_acceptance(event)
    }
    fn decision_handles_delivery(item: &DeliveryItem<Self>, decision_event: &Self::Event) -> bool {
        match (&item.payload, decision_event) {
            (DeliveryPayload::Receipt(payload), TestEvent::Delivery(expected)) => {
                payload == expected
            }
            (
                DeliveryPayload::Barrier(TestBarrierEvent::Family(payload)),
                TestEvent::Delivery(expected),
            ) => payload == expected,
            _ => false,
        }
    }
    fn decision_handles_runtime_acceptance(
        item: &DeliveryItem<Self>,
        decision_event: &Self::Event,
    ) -> bool {
        matches!((&item.payload, decision_event), (DeliveryPayload::Receipt(payload), TestEvent::RuntimeAccept(expected)) if payload == expected)
    }
    fn decision_handles_runtime_suppression(
        item: &DeliveryItem<Self>,
        decision_event: &Self::Event,
    ) -> bool {
        matches!((&item.payload, decision_event), (DeliveryPayload::Receipt(payload), TestEvent::RuntimeSuppress(expected)) if payload == expected)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
struct NoRuntimeProfile;

impl WorkflowProfile for NoRuntimeProfile {
    type RuntimeAcceptance = RuntimeAcceptanceDisabled;
    type ExternalAcceptance = ExternalAcceptanceEnabled;
    type Snapshot = &'static str;
    type Event = TestEvent;
    type Intent = &'static str;
    type Observation = &'static str;
    type Receipt = &'static str;
    type ReceiptReducerEvent = &'static str;
    type BarrierEvent = TestBarrierEvent;
    type ManualPayload = &'static str;

    fn runtime_start_allowed(snapshot: &Self::Snapshot) -> bool {
        TestProfile::runtime_start_allowed(snapshot)
    }
    fn receipt_requires_runtime_acceptance(event: &Self::ReceiptReducerEvent) -> bool {
        TestProfile::receipt_requires_runtime_acceptance(event)
    }
    fn decision_handles_delivery(item: &DeliveryItem<Self>, decision_event: &Self::Event) -> bool {
        match (&item.payload, decision_event) {
            (DeliveryPayload::Receipt(payload), TestEvent::Delivery(expected)) => {
                payload == expected
            }
            (
                DeliveryPayload::Barrier(TestBarrierEvent::Family(payload)),
                TestEvent::Delivery(expected),
            ) => payload == expected,
            _ => false,
        }
    }
    fn decision_handles_runtime_acceptance(
        item: &DeliveryItem<Self>,
        decision_event: &Self::Event,
    ) -> bool {
        matches!((&item.payload, decision_event), (DeliveryPayload::Receipt(payload), TestEvent::RuntimeAccept(expected)) if payload == expected)
    }
    fn decision_handles_runtime_suppression(
        item: &DeliveryItem<Self>,
        decision_event: &Self::Event,
    ) -> bool {
        matches!((&item.payload, decision_event), (DeliveryPayload::Receipt(payload), TestEvent::RuntimeSuppress(expected)) if payload == expected)
    }
}

impl WorkflowProfile for TestProfile {
    type RuntimeAcceptance = RuntimeAcceptanceEnabled;
    type ExternalAcceptance = ExternalAcceptanceEnabled;
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
            (DeliveryPayload::Receipt(payload), TestEvent::Delivery(expected)) => {
                payload == expected
            }
            (
                DeliveryPayload::Barrier(TestBarrierEvent::Family(payload)),
                TestEvent::Delivery(expected),
            ) => payload == expected,
            _ => false,
        }
    }

    fn decision_handles_runtime_acceptance(
        item: &DeliveryItem<Self>,
        decision_event: &Self::Event,
    ) -> bool {
        matches!((&item.payload, decision_event),
            (DeliveryPayload::Receipt(payload), TestEvent::RuntimeAccept(expected)) if payload == expected)
    }

    fn decision_handles_runtime_suppression(
        item: &DeliveryItem<Self>,
        decision_event: &Self::Event,
    ) -> bool {
        matches!((&item.payload, decision_event),
            (DeliveryPayload::Receipt(payload), TestEvent::RuntimeSuppress(expected)) if payload == expected)
    }
}

fn profile() -> ProfileRef {
    ProfileRef {
        profile_kind: "test".to_string(),
        profile_version: 1,
    }
}

fn profile_v2() -> ProfileRef {
    ProfileRef {
        profile_kind: "test-v2".to_string(),
        profile_version: 2,
    }
}

fn codec(family: &'static str) -> CodecRef {
    CodecRef { family, version: 1 }
}

fn acceptance() -> AcceptanceProfile<RuntimeAcceptanceEnabled, ExternalAcceptanceEnabled> {
    AcceptanceProfile::new(
        profile(),
        SupportedCodecRegistry::new([
            codec("snapshot"),
            codec("event"),
            codec("intent"),
            codec("receipt"),
            codec("barrier"),
            codec("manual"),
        ])
        .expect("codecs"),
    )
}

fn runtime_only_acceptance(
) -> AcceptanceProfile<RuntimeAcceptanceEnabled, ExternalAcceptanceDisabled> {
    AcceptanceProfile::new(
        profile(),
        SupportedCodecRegistry::new([
            codec("snapshot"),
            codec("event"),
            codec("intent"),
            codec("receipt"),
            codec("barrier"),
            codec("manual"),
        ])
        .expect("codecs"),
    )
}

fn no_runtime_acceptance() -> AcceptanceProfile<RuntimeAcceptanceDisabled, ExternalAcceptanceEnabled>
{
    AcceptanceProfile::new(
        profile(),
        SupportedCodecRegistry::new([
            codec("snapshot"),
            codec("event"),
            codec("intent"),
            codec("receipt"),
            codec("barrier"),
            codec("manual"),
        ])
        .expect("codecs"),
    )
}

fn erased_runtime_only_acceptance() -> ErasedAcceptanceProfile {
    runtime_only_acceptance().erase()
}

fn erased_no_runtime_acceptance() -> ErasedAcceptanceProfile {
    no_runtime_acceptance().erase()
}

fn erased_no_acceptance() -> ErasedAcceptanceProfile {
    AcceptanceProfile::<RuntimeAcceptanceDisabled, ExternalAcceptanceDisabled>::new(
        profile(),
        SupportedCodecRegistry::new([
            codec("snapshot"),
            codec("event"),
            codec("intent"),
            codec("receipt"),
            codec("barrier"),
            codec("manual"),
        ])
        .expect("codecs"),
    )
    .erase()
}

fn workflow() -> WorkflowState<TestProfile> {
    WorkflowState::new(
        WorkflowId(1),
        &profile(),
        acceptance(),
        codec("snapshot"),
        "initial",
    )
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

fn add_delivery_source(plan: &mut TransitionPlan<TestProfile>, runtime: bool) {
    if runtime {
        if !plan
            .effects
            .iter()
            .any(|effect| effect.effect_id == EffectId(1))
        {
            plan.effects.push(effect_decl(
                1,
                EffectRole::Required,
                ExecutionCapability::SafelyRepeatable,
                Generation(0),
            ));
        }
    } else if !plan
        .effects
        .iter()
        .any(|effect| effect.effect_id == EffectId(1))
    {
        plan.effects.push(effect_decl(
            1,
            EffectRole::Required,
            ExecutionCapability::SafelyRepeatable,
            Generation(0),
        ));
    }
}

fn delivery_decl(
    payload: &'static str,
    requires_runtime_acceptance: bool,
) -> DeliveryDecl<TestProfile> {
    if requires_runtime_acceptance {
        delivery_decl_with_source(payload, true, Some(EffectId(1)), None)
    } else {
        delivery_decl_with_source(payload, false, Some(EffectId(1)), None)
    }
}

fn delivery_decl_with_source(
    payload: &'static str,
    requires_runtime_acceptance: bool,
    effect_id: Option<EffectId>,
    barrier_id: Option<BarrierId>,
) -> DeliveryDecl<TestProfile> {
    if requires_runtime_acceptance {
        DeliveryDecl::runtime_owed(
            effect_id,
            barrier_id,
            "reducer",
            codec("receipt"),
            DeliveryPayload::Receipt(payload),
        )
    } else {
        DeliveryDecl::immediate(
            effect_id,
            barrier_id,
            "reducer",
            codec("receipt"),
            DeliveryPayload::Receipt(payload),
        )
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

fn decision(
    expected_workflow_version: Version,
    plan: TransitionPlan<TestProfile>,
) -> ReducerDecision<TestProfile> {
    ReducerDecision {
        expected_workflow_version,
        plan,
    }
}

fn claim(
    workflow: &mut WorkflowState<TestProfile>,
    effect_id: u64,
    now: u64,
    lease_until: Option<u64>,
) -> ClaimResult {
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
    plan.effects.push(effect_decl(
        1,
        EffectRole::Required,
        capability,
        Generation(0),
    ));
    workflow
        .commit_transition(&decision(Version(0), plan), &BTreeMap::new())
        .expect("commit");
    let claim = claim(&mut workflow, 1, 0, lease_until);
    (workflow, claim.authority.expect("authority"))
}

fn manual_choice(kind: ManualChoiceKind) -> ManualChoice<TestProfile> {
    ManualChoice {
        kind,
        codec: codec("manual"),
        payload: "accept",
        receipt_codec: codec("receipt"),
        receipt: "terminal",
        receipt_event_codec: codec("receipt"),
        receipt_event: "terminal",
    }
}

fn manual_commit(next_status: WorkflowStatus) -> ManualResolutionCommit<TestProfile> {
    ManualResolutionCommit {
        transition_codec: codec("event"),
        transition_event: TestEvent::Transition("manual"),
        next_status,
        retry_at: None,
        compensation_effects: vec![],
        compensation_dependencies: vec![],
    }
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
    let mut first = effect_decl(
        1,
        EffectRole::Required,
        ExecutionCapability::SafelyRepeatable,
        Generation(0),
    );
    first.next_eligible_at = Some(Timestamp(1));
    plan.effects = vec![
        first,
        effect_decl(
            2,
            EffectRole::Required,
            ExecutionCapability::SafelyRepeatable,
            Generation(0),
        ),
    ];
    plan.dependencies.push(DependencyDecl {
        effect_id: EffectId(2),
        depends_on_effect_id: EffectId(1),
    });
    workflow
        .commit_transition(&decision(Version(0), plan), &BTreeMap::new())
        .expect("commit");
    assert_eq!(workflow.effects[&EffectId(1)].status, EffectStatus::Blocked);
    assert_eq!(workflow.effects[&EffectId(2)].status, EffectStatus::Blocked);
    workflow.refresh_eligibility(Timestamp(1));
    assert_eq!(
        workflow.effects[&EffectId(1)].status,
        EffectStatus::Eligible
    );
    assert_eq!(workflow.effects[&EffectId(2)].status, EffectStatus::Blocked);

    let claim = claim(&mut workflow, 1, 1, None);
    let authority = claim.authority.expect("authority");
    let receipt_acceptance = workflow.accept_receipt(
        &authority,
        Timestamp(0),
        Some(authority.attempt_id),
        ReceiptOrigin::Execution,
        codec("receipt"),
        "done-1",
        codec("receipt"),
        "done-1",
    );
    assert_eq!(receipt_acceptance.outcome, AuthorityOutcome::Authorized);
    workflow.refresh_eligibility(Timestamp(0));
    assert_eq!(
        workflow.effects[&EffectId(2)].status,
        EffectStatus::Eligible
    );

    let mut cyclic = base_plan();
    cyclic.effects = vec![
        effect_decl(
            1,
            EffectRole::Required,
            ExecutionCapability::SafelyRepeatable,
            Generation(0),
        ),
        effect_decl(
            2,
            EffectRole::Required,
            ExecutionCapability::SafelyRepeatable,
            Generation(0),
        ),
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
        validate_plan(
            WorkflowStatus::Active,
            &cyclic,
            &BTreeMap::new(),
            &acceptance().supported_codecs
        ),
        Err(PlanError::DependencyCycle)
    );
}

#[test]
fn barriers_require_matching_receipt_family_and_generation() {
    let mut workflow = workflow();
    let mut plan = base_plan();
    plan.effects = vec![
        effect_decl(
            1,
            EffectRole::Required,
            ExecutionCapability::SafelyRepeatable,
            Generation(0),
        ),
        effect_decl(
            2,
            EffectRole::Required,
            ExecutionCapability::SafelyRepeatable,
            Generation(0),
        ),
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
            receipt_family: ReceiptFamily::CurrentGenerationEffect,
        },
    ];
    let events = BTreeMap::from([(BarrierId(7), TestBarrierEvent::Family("barrier-ready"))]);
    workflow
        .commit_transition(&decision(Version(0), plan), &events)
        .expect("commit");
    assert_eq!(workflow.barriers[&BarrierId(7)].required_members.len(), 2);
    assert_eq!(
        workflow.barriers[&BarrierId(7)].status,
        BarrierStatus::Waiting
    );

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
        validate_plan(
            WorkflowStatus::Active,
            &mismatch,
            &events,
            &acceptance().supported_codecs
        ),
        Err(PlanError::BarrierReceiptFamilyMismatch {
            barrier_id: BarrierId(9),
            effect_id: EffectId(1),
        })
    );
}

#[test]
fn delivery_validation_requires_exactly_one_source_and_runtime_gate() {
    let mut source_ambiguous = base_plan();
    source_ambiguous.effects.push(effect_decl(
        1,
        EffectRole::Required,
        ExecutionCapability::SafelyRepeatable,
        Generation(0),
    ));
    source_ambiguous.deliveries.push(delivery_decl_with_source(
        "deliver-me",
        false,
        Some(EffectId(1)),
        Some(BarrierId(9)),
    ));
    assert_eq!(
        validate_plan(
            WorkflowStatus::Active,
            &source_ambiguous,
            &BTreeMap::new(),
            &acceptance().supported_codecs,
        ),
        Err(PlanError::DeliverySourceCount {
            effect_id: Some(EffectId(1)),
            barrier_id: Some(BarrierId(9)),
        })
    );

    let mut source_missing = base_plan();
    source_missing
        .deliveries
        .push(delivery_decl_with_source("deliver-me", false, None, None));
    assert_eq!(
        validate_plan(
            WorkflowStatus::Active,
            &source_missing,
            &BTreeMap::new(),
            &acceptance().supported_codecs,
        ),
        Err(PlanError::DeliverySourceCount {
            effect_id: None,
            barrier_id: None,
        })
    );

    let mut runtime_blocked = base_plan();
    runtime_blocked.snapshot = "runtime-blocked";
    runtime_blocked.effects.push(effect_decl(
        1,
        EffectRole::Required,
        ExecutionCapability::SafelyRepeatable,
        Generation(0),
    ));
    runtime_blocked
        .deliveries
        .push(delivery_decl("runtime", true));
    assert_eq!(
        validate_plan(
            WorkflowStatus::Active,
            &runtime_blocked,
            &BTreeMap::new(),
            &acceptance().supported_codecs,
        ),
        Err(PlanError::RuntimeStartNotAllowed)
    );
}

#[test]
fn process_incarnation_and_attempt_ids_fence_authority() {
    let (mut workflow, authority) =
        begin_observation_effect(ExecutionCapability::ReclaimableObservation, Some(5));
    let mut stale_process = authority.clone();
    stale_process.process_incarnation = ProcessIncarnation(99);
    assert_eq!(
        workflow.record_observation(
            &stale_process,
            Timestamp(1),
            Timestamp(1),
            codec("receipt"),
            "obs"
        ),
        AuthorityOutcome::StaleAuthority
    );

    let mut stale_attempt = authority.clone();
    stale_attempt.attempt_id = AttemptId(authority.attempt_id.0 + 1);
    assert_eq!(
        workflow.record_observation(
            &stale_attempt,
            Timestamp(1),
            Timestamp(1),
            codec("receipt"),
            "obs"
        ),
        AuthorityOutcome::StaleAuthority
    );

    assert_eq!(workflow.effects[&EffectId(1)].stale_observations.len(), 2);
    assert_eq!(
        workflow.effects[&EffectId(1)].stale_observations[1].attempt_id,
        stale_attempt.attempt_id
    );
    assert_eq!(
        workflow.record_observation(
            &authority,
            Timestamp(1),
            Timestamp(1),
            codec("receipt"),
            "obs"
        ),
        AuthorityOutcome::Authorized
    );
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
    no_lease
        .commit_transition(&decision(Version(0), plan), &BTreeMap::new())
        .expect("commit");
    assert_eq!(
        claim(&mut no_lease, 1, 0, None).outcome,
        ClaimOutcome::AuthorityConflict
    );

    let (mut workflow, authority) =
        begin_observation_effect(ExecutionCapability::ReclaimableObservation, Some(5));
    assert_eq!(
        workflow
            .renew_lease(
                &authority,
                Timestamp(1),
                LeaseExpiry::finite(8).expect("finite")
            )
            .outcome,
        AuthorityOutcome::Authorized
    );
    assert_eq!(
        workflow.effects[&EffectId(1)]
            .reclaimable_lease
            .as_ref()
            .map(|l| l.lease_until),
        Some(LeaseExpiry(8))
    );
    assert_eq!(
        workflow.expire_lease(EffectId(1), authority.attempt_id, Timestamp(7)),
        AuthorityOutcome::StaleAuthority
    );
    assert_eq!(
        workflow.expire_lease(EffectId(1), authority.attempt_id, Timestamp(8)),
        AuthorityOutcome::Authorized
    );
    assert_eq!(
        workflow.effects[&EffectId(1)].status,
        EffectStatus::Eligible
    );
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
        assert_eq!(
            workflow.expire_lease(EffectId(1), authority.attempt_id, Timestamp(1)),
            AuthorityOutcome::StaleAuthority
        );
        let effect = workflow.effects.get_mut(&EffectId(1)).expect("effect");
        effect.reclaimable_lease = Some(ReclaimableLease {
            attempt_id: authority.attempt_id,
            lease_until: LeaseExpiry::finite(1).expect("finite"),
        });
        assert_eq!(
            workflow.expire_lease(EffectId(1), authority.attempt_id, Timestamp(1)),
            AuthorityOutcome::Authorized
        );
        assert_eq!(
            workflow.effects[&EffectId(1)].status,
            EffectStatus::AmbiguityWait
        );
        assert_eq!(
            claim(&mut workflow, 1, 2, None).outcome,
            ClaimOutcome::Ineligible
        );
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
    assert_eq!(
        workflow
            .schedule_retry(&authority, Timestamp(0), Timestamp(10))
            .outcome,
        AuthorityOutcome::Authorized
    );
    assert_eq!(
        workflow.effects[&EffectId(1)].status,
        EffectStatus::RetryWait
    );
    workflow.refresh_eligibility(Timestamp(10));
    assert_eq!(
        workflow.effects[&EffectId(1)].status,
        EffectStatus::Eligible
    );
    let second = claim(&mut workflow, 1, 10, None);
    assert_eq!(second.outcome, ClaimOutcome::Started);
    assert_eq!(second.attempt.expect("attempt").ordinal, 2);
}

#[test]
fn receipt_and_delivery_idempotency_is_single_winner() {
    let (mut workflow, authority) =
        begin_observation_effect(ExecutionCapability::SafelyRepeatable, None);
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
    add_delivery_source(&mut plan, false);
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
    assert_eq!(
        workflow.deliveries[&item.id].status,
        DeliveryStatus::Accepted
    );

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
fn delivery_consumption_rejects_empty_batch_and_applies_full_plan() {
    let mut workflow = workflow();
    let mut initial = base_plan();
    initial.effects.push(effect_decl(
        1,
        EffectRole::Required,
        ExecutionCapability::SafelyRepeatable,
        Generation(0),
    ));
    add_delivery_source(&mut initial, false);
    initial.deliveries.push(delivery_decl("deliver-me", false));
    workflow
        .commit_transition(&decision(Version(0), initial), &BTreeMap::new())
        .expect("commit");
    let delivery = workflow
        .deliveries
        .values()
        .find(|item| matches!(item.payload, DeliveryPayload::Receipt("deliver-me")))
        .cloned()
        .expect("delivery");

    let empty = workflow.consume_deliveries(&DeliveryDecisionBinding {
        items: vec![],
        decision: decision(
            workflow.version,
            TransitionPlan {
                event: TestEvent::Delivery("deliver-me"),
                ..base_plan()
            },
        ),
    });
    assert_eq!(
        empty,
        Err(EngineError::InvalidPlan(PlanError::EmptyDeliveryBatch))
    );

    let mut consume_plan = base_plan();
    consume_plan.event = TestEvent::Delivery("deliver-me");
    consume_plan.effects.push(effect_decl(
        2,
        EffectRole::Required,
        ExecutionCapability::SafelyRepeatable,
        Generation(0),
    ));
    consume_plan.deliveries.push(delivery_decl_with_source(
        "follow-up",
        false,
        Some(EffectId(2)),
        None,
    ));
    let consume = workflow
        .consume_deliveries(&DeliveryDecisionBinding {
            items: vec![delivery.clone()],
            decision: decision(Version(1), consume_plan),
        })
        .expect("consume");
    assert_eq!(consume.outcome, CommitOutcome::Committed);
    assert_eq!(
        workflow.deliveries[&delivery.id].status,
        DeliveryStatus::Accepted
    );
    assert_eq!(
        workflow.effects[&EffectId(1)].status,
        EffectStatus::Eligible
    );
    assert_eq!(
        workflow.effects[&EffectId(2)].status,
        EffectStatus::Eligible
    );
    assert!(workflow
        .deliveries
        .values()
        .any(|item| matches!(item.payload, DeliveryPayload::Receipt("follow-up"))));
}

#[test]
fn runtime_acceptance_is_atomic_and_duplicate_safe() {
    let mut workflow = workflow();
    let mut plan = base_plan();
    add_delivery_source(&mut plan, true);
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
    assert_eq!(
        workflow.deliveries[&item.id].runtime_acceptance_status,
        Some(RuntimeAcceptanceStatus::Accepted)
    );

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
fn runtime_suppression_marks_each_selected_delivery() {
    let mut workflow = workflow();
    let mut plan = base_plan();
    add_delivery_source(&mut plan, true);
    plan.deliveries.push(delivery_decl("runtime-a", true));
    plan.deliveries.push(delivery_decl("runtime-b", true));
    let committed = workflow
        .commit_transition(&decision(Version(0), plan), &BTreeMap::new())
        .expect("commit");
    let mut items = committed.deliveries;
    items.sort_by_key(|item| item.id.0);

    let first = workflow
        .accept_runtime_delivery(
            items[0].id,
            &decision(
                Version(1),
                TransitionPlan {
                    event: TestEvent::RuntimeSuppress("runtime-a"),
                    ..base_plan()
                },
            ),
            true,
        )
        .expect("suppress first");
    assert_eq!(first.outcome, CommitOutcome::Committed);
    assert_eq!(
        workflow.deliveries[&items[0].id].status,
        DeliveryStatus::Suppressed
    );
    assert_eq!(
        workflow.deliveries[&items[0].id].runtime_acceptance_status,
        Some(RuntimeAcceptanceStatus::Suppressed)
    );
    assert_eq!(
        workflow.deliveries[&items[1].id].status,
        DeliveryStatus::Pending
    );
    assert_eq!(
        workflow.deliveries[&items[1].id].runtime_acceptance_status,
        Some(RuntimeAcceptanceStatus::Owed)
    );

    let second = workflow
        .accept_runtime_delivery(
            items[1].id,
            &decision(
                workflow.version,
                TransitionPlan {
                    event: TestEvent::RuntimeSuppress("runtime-b"),
                    ..base_plan()
                },
            ),
            true,
        )
        .expect("suppress second");
    assert_eq!(second.outcome, CommitOutcome::Committed);
    assert_eq!(
        workflow.deliveries[&items[1].id].status,
        DeliveryStatus::Suppressed
    );
    assert_eq!(
        workflow.deliveries[&items[1].id].runtime_acceptance_status,
        Some(RuntimeAcceptanceStatus::Suppressed)
    );
}

#[test]
fn runtime_acceptance_applies_plan_and_rejects_runtime_blocked_next_snapshot() {
    let mut workflow = workflow();
    let mut initial = base_plan();
    initial.effects.push(effect_decl(
        1,
        EffectRole::Required,
        ExecutionCapability::SafelyRepeatable,
        Generation(0),
    ));
    initial.deliveries.push(delivery_decl("runtime", true));
    workflow
        .commit_transition(&decision(Version(0), initial), &BTreeMap::new())
        .expect("commit");
    let item = workflow
        .deliveries
        .values()
        .next()
        .cloned()
        .expect("delivery");

    let mut blocked = base_plan();
    blocked.snapshot = "runtime-blocked";
    blocked.event = TestEvent::RuntimeAccept("runtime");
    blocked.effects.push(effect_decl(
        2,
        EffectRole::Required,
        ExecutionCapability::SafelyRepeatable,
        Generation(0),
    ));
    blocked.deliveries.push(delivery_decl_with_source(
        "next-runtime",
        true,
        Some(EffectId(2)),
        None,
    ));
    assert_eq!(
        workflow.accept_runtime_delivery(item.id, &decision(Version(1), blocked), false),
        Err(EngineError::InvalidPlan(PlanError::RuntimeStartNotAllowed))
    );

    let mut accept_plan = base_plan();
    accept_plan.event = TestEvent::RuntimeAccept("runtime");
    accept_plan.effects.push(effect_decl(
        2,
        EffectRole::Required,
        ExecutionCapability::SafelyRepeatable,
        Generation(0),
    ));
    accept_plan.invalidations.push(EffectInvalidationDecl {
        effect_id: EffectId(1),
    });
    accept_plan.deliveries.push(delivery_decl_with_source(
        "follow-up",
        false,
        Some(EffectId(2)),
        None,
    ));
    let accepted = workflow
        .accept_runtime_delivery(item.id, &decision(Version(1), accept_plan), false)
        .expect("accept runtime");
    assert_eq!(accepted.outcome, CommitOutcome::Committed);
    assert_eq!(
        workflow.effects[&EffectId(1)].status,
        EffectStatus::Invalidated
    );
    assert_eq!(
        workflow.effects[&EffectId(2)].status,
        EffectStatus::Eligible
    );
    assert!(workflow
        .deliveries
        .values()
        .any(|delivery| matches!(delivery.payload, DeliveryPayload::Receipt("follow-up"))));
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
    workflow
        .commit_transition(&decision(Version(0), initial), &BTreeMap::new())
        .expect("commit");

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
        invalidations: vec![EffectInvalidationDecl {
            effect_id: EffectId(1),
        }],
        terminal_receipt: None,
        compensation_plan: compensation,
    };
    let result = workflow
        .cancel_with_compensation(&request, &BTreeMap::new())
        .expect("cancel");
    assert_eq!(result.outcome, CommitOutcome::Committed);
    assert_eq!(workflow.generation, Generation(1));
    assert_eq!(
        workflow.effects[&EffectId(1)].status,
        EffectStatus::Invalidated
    );
    assert_eq!(
        workflow.effects[&EffectId(2)].status,
        EffectStatus::Eligible
    );
}

#[test]
fn cancellation_invalid_plan_leaves_state_unchanged() {
    let mut workflow = workflow();
    let mut initial = base_plan();
    initial.effects.push(effect_decl(
        1,
        EffectRole::Required,
        ExecutionCapability::SafelyRepeatable,
        Generation(0),
    ));
    workflow
        .commit_transition(&decision(Version(0), initial), &BTreeMap::new())
        .expect("commit");
    let before = workflow.clone();

    let request = CancellationRequest {
        expected_workflow_version: Version(1),
        next_snapshot: "cancel-snapshot",
        next_snapshot_codec: codec("snapshot"),
        event: TestEvent::Cancel,
        event_codec: codec("event"),
        invalidations: vec![EffectInvalidationDecl {
            effect_id: EffectId(1),
        }],
        terminal_receipt: None,
        compensation_plan: base_plan(),
    };
    let result = workflow.cancel_with_compensation(&request, &BTreeMap::new());
    assert_eq!(
        result,
        Err(EngineError::InvalidPlan(
            PlanError::InvalidStatusTransition {
                current: WorkflowStatus::Cancelling,
                next: WorkflowStatus::Active,
            }
        ))
    );
    assert_eq!(workflow, before);
}

#[test]
fn stale_cancellation_request_leaves_state_unchanged() {
    let mut workflow = workflow();
    let before = workflow.clone();

    let request = CancellationRequest {
        expected_workflow_version: Version(99),
        next_snapshot: "cancel-snapshot",
        next_snapshot_codec: codec("snapshot"),
        event: TestEvent::Cancel,
        event_codec: codec("event"),
        invalidations: vec![],
        terminal_receipt: None,
        compensation_plan: base_plan(),
    };
    let result = workflow
        .cancel_with_compensation(&request, &BTreeMap::new())
        .expect("stale version returns outcome");
    assert_eq!(result.outcome, CommitOutcome::VersionConflict);
    assert_eq!(workflow, before);
}

#[test]
fn cancellation_and_receipt_generate_deterministic_single_winner_state() {
    let mut workflow = workflow();
    let mut initial = base_plan();
    initial.effects.push(effect_decl(
        1,
        EffectRole::Required,
        ExecutionCapability::SafelyRepeatable,
        Generation(0),
    ));
    workflow
        .commit_transition(&decision(Version(0), initial), &BTreeMap::new())
        .expect("commit");
    let authority = claim(&mut workflow, 1, 0, None)
        .authority
        .expect("authority");

    let accepted = workflow.accept_receipt(
        &authority,
        Timestamp(0),
        Some(authority.attempt_id),
        ReceiptOrigin::Execution,
        codec("receipt"),
        "done",
        codec("receipt"),
        "done",
    );
    assert_eq!(accepted.outcome, AuthorityOutcome::Authorized);

    let mut compensation = base_plan();
    compensation.next_status = WorkflowStatus::Cancelling;
    compensation.effects.push(effect_decl(
        2,
        EffectRole::Compensation,
        ExecutionCapability::SafelyRepeatable,
        Generation(1),
    ));
    let mut request = CancellationRequest {
        expected_workflow_version: workflow.version,
        next_snapshot: "cancel-snapshot",
        next_snapshot_codec: codec("snapshot"),
        event: TestEvent::Cancel,
        event_codec: codec("event"),
        invalidations: vec![EffectInvalidationDecl {
            effect_id: EffectId(1),
        }],
        terminal_receipt: None,
        compensation_plan: compensation,
    };
    request.terminal_receipt = Some(CancellationReceiptDecl {
        effect_id: EffectId(1),
        receipt_codec: codec("receipt"),
        receipt: "done",
        event_codec: codec("receipt"),
        event: "done",
    });
    let cancel = workflow
        .cancel_with_compensation(&request, &BTreeMap::new())
        .expect("cancel commits deterministic winner");
    assert_eq!(cancel.outcome, CommitOutcome::Committed);
    assert_eq!(workflow.generation, Generation(1));
    assert_eq!(
        workflow.effects[&EffectId(1)].status,
        EffectStatus::Receipted
    );
    let receipt = workflow.effects[&EffectId(1)]
        .receipt
        .as_ref()
        .expect("receipt preserved");
    assert_eq!(receipt.origin, ReceiptOrigin::Execution);
    assert_eq!(receipt.generation, Generation(0));
    assert_eq!(
        workflow.effects[&EffectId(2)].status,
        EffectStatus::Eligible
    );
}

#[test]
fn cancellation_before_receipt_makes_receipt_stale_by_generation() {
    let mut workflow = workflow();
    let mut initial = base_plan();
    initial.effects.push(effect_decl(
        1,
        EffectRole::Required,
        ExecutionCapability::SafelyRepeatable,
        Generation(0),
    ));
    workflow
        .commit_transition(&decision(Version(0), initial), &BTreeMap::new())
        .expect("commit");
    let authority = claim(&mut workflow, 1, 0, None)
        .authority
        .expect("authority");

    let mut compensation = base_plan();
    compensation.next_status = WorkflowStatus::Cancelling;
    compensation.effects.push(effect_decl(
        2,
        EffectRole::Compensation,
        ExecutionCapability::SafelyRepeatable,
        Generation(1),
    ));
    let request = CancellationRequest {
        expected_workflow_version: workflow.version,
        next_snapshot: "cancel-snapshot",
        next_snapshot_codec: codec("snapshot"),
        event: TestEvent::Cancel,
        event_codec: codec("event"),
        invalidations: vec![EffectInvalidationDecl {
            effect_id: EffectId(1),
        }],
        terminal_receipt: None,
        compensation_plan: compensation,
    };
    workflow
        .cancel_with_compensation(&request, &BTreeMap::new())
        .expect("cancel");

    let accepted = workflow.accept_receipt(
        &authority,
        Timestamp(0),
        Some(authority.attempt_id),
        ReceiptOrigin::Execution,
        codec("receipt"),
        "done",
        codec("receipt"),
        "done",
    );
    assert_eq!(accepted.outcome, AuthorityOutcome::StaleAuthority);
    assert_eq!(
        workflow.effects[&EffectId(1)].status,
        EffectStatus::Invalidated
    );
    assert_eq!(workflow.generation, Generation(1));
}

#[test]
fn cancellation_cascades_invalidations_to_current_generation_dependents() {
    let mut workflow = workflow();
    let mut initial = base_plan();
    initial.effects.push(effect_decl(
        1,
        EffectRole::Required,
        ExecutionCapability::SafelyRepeatable,
        Generation(0),
    ));
    initial.effects.push(effect_decl(
        2,
        EffectRole::Required,
        ExecutionCapability::SafelyRepeatable,
        Generation(0),
    ));
    initial.effects.push(effect_decl(
        3,
        EffectRole::Compensation,
        ExecutionCapability::SafelyRepeatable,
        Generation(0),
    ));
    initial.dependencies.push(DependencyDecl {
        effect_id: EffectId(2),
        depends_on_effect_id: EffectId(1),
    });
    initial.dependencies.push(DependencyDecl {
        effect_id: EffectId(3),
        depends_on_effect_id: EffectId(1),
    });
    workflow
        .commit_transition(&decision(Version(0), initial), &BTreeMap::new())
        .expect("commit");

    let mut compensation = base_plan();
    compensation.next_status = WorkflowStatus::Cancelling;
    let request = CancellationRequest {
        expected_workflow_version: workflow.version,
        next_snapshot: "cancel-snapshot",
        next_snapshot_codec: codec("snapshot"),
        event: TestEvent::Cancel,
        event_codec: codec("event"),
        invalidations: vec![EffectInvalidationDecl {
            effect_id: EffectId(1),
        }],
        terminal_receipt: None,
        compensation_plan: compensation,
    };
    workflow
        .cancel_with_compensation(&request, &BTreeMap::new())
        .expect("cancel");

    assert_eq!(
        workflow.effects[&EffectId(1)].status,
        EffectStatus::Invalidated
    );
    assert_eq!(workflow.effects[&EffectId(2)].status, EffectStatus::Blocked);
    assert_eq!(workflow.effects[&EffectId(3)].status, EffectStatus::Blocked);
}

#[test]
fn manual_ambiguity_produces_resolution_and_terminal_choice_emits_receipt() {
    let (mut workflow, authority) =
        begin_observation_effect(ExecutionCapability::ManualOnAmbiguity, None);
    assert_eq!(
        workflow.record_observation(
            &authority,
            Timestamp(0),
            Timestamp(0),
            codec("receipt"),
            "evidence"
        ),
        AuthorityOutcome::Authorized
    );
    let choices = vec![manual_choice(ManualChoiceKind::AcceptAsTerminal)];
    let reconciliation =
        workflow.require_manual_resolution(&authority, Timestamp(1), choices.clone());
    assert_eq!(reconciliation.outcome, AuthorityOutcome::Authorized);
    let resolution = reconciliation.manual_resolution.expect("resolution");
    assert_eq!(resolution.evidence.len(), 1);

    let outcome = workflow.resolve_manual(
        resolution.id,
        Version(1),
        "reviewer",
        &choices[0],
        &manual_commit(WorkflowStatus::Active),
    );
    assert_eq!(outcome.outcome, CommitOutcome::Committed);
    match outcome.effect_outcome.expect("effect outcome") {
        ManualEffectOutcome::Receipt {
            receipt,
            reducer_event,
        } => {
            assert_eq!(receipt.receipt, "terminal");
            assert_eq!(reducer_event.payload, DeliveryPayload::Receipt("terminal"));
        }
        other @ (ManualEffectOutcome::Retry
        | ManualEffectOutcome::Compensate
        | ManualEffectOutcome::Failed
        | ManualEffectOutcome::Suppressed) => {
            panic!("unexpected outcome: {other:?}");
        }
    }
}

#[test]
fn manual_resolution_blocks_claims_until_resolved() {
    let (mut workflow, authority) =
        begin_observation_effect(ExecutionCapability::ManualOnAmbiguity, None);
    let choices = vec![manual_choice(ManualChoiceKind::Retry)];
    let reconciliation = workflow.require_manual_resolution(&authority, Timestamp(1), choices);
    assert_eq!(reconciliation.outcome, AuthorityOutcome::Authorized);
    assert_eq!(workflow.status, WorkflowStatus::ManualResolution);
    assert_eq!(
        claim(&mut workflow, 1, 1, None).outcome,
        ClaimOutcome::Ineligible
    );
}

#[test]
fn manual_resolution_invalid_choice_leaves_state_unchanged() {
    let (mut workflow, authority) =
        begin_observation_effect(ExecutionCapability::ManualOnAmbiguity, None);
    let choices = vec![manual_choice(ManualChoiceKind::Retry)];
    let reconciliation =
        workflow.require_manual_resolution(&authority, Timestamp(1), choices.clone());
    let resolution = reconciliation.manual_resolution.expect("resolution");
    let before = workflow.clone();

    let outcome = workflow.resolve_manual(
        resolution.id,
        Version(1),
        "reviewer",
        &manual_choice(ManualChoiceKind::AcceptAsTerminal),
        &manual_commit(WorkflowStatus::Active),
    );
    assert_eq!(outcome.outcome, CommitOutcome::InvalidPlan);
    assert_eq!(workflow, before);
}

#[test]
fn manual_resolution_stale_version_leaves_state_unchanged() {
    let (mut workflow, authority) =
        begin_observation_effect(ExecutionCapability::ManualOnAmbiguity, None);
    let choices = vec![manual_choice(ManualChoiceKind::Retry)];
    let reconciliation =
        workflow.require_manual_resolution(&authority, Timestamp(1), choices.clone());
    let resolution = reconciliation.manual_resolution.expect("resolution");
    let before = workflow.clone();

    let outcome = workflow.resolve_manual(
        resolution.id,
        Version(0),
        "reviewer",
        &choices[0],
        &manual_commit(WorkflowStatus::Active),
    );
    assert_eq!(outcome.outcome, CommitOutcome::InvalidPlan);
    assert_eq!(workflow, before);
}

#[test]
fn manual_retry_requires_retry_at_and_terminal_outcomes_block_claims() {
    let (mut retry_workflow, authority) =
        begin_observation_effect(ExecutionCapability::ManualOnAmbiguity, None);
    let choices = vec![manual_choice(ManualChoiceKind::Retry)];
    let reconciliation =
        retry_workflow.require_manual_resolution(&authority, Timestamp(1), choices.clone());
    let resolution = reconciliation.manual_resolution.expect("resolution");
    let before = retry_workflow.clone();
    let outcome = retry_workflow.resolve_manual(
        resolution.id,
        Version(1),
        "reviewer",
        &choices[0],
        &manual_commit(WorkflowStatus::Active),
    );
    assert_eq!(outcome.outcome, CommitOutcome::InvalidPlan);
    assert_eq!(retry_workflow, before);

    let (mut terminal_workflow, terminal_authority) =
        begin_observation_effect(ExecutionCapability::ManualOnAmbiguity, None);
    let terminal_choices = vec![manual_choice(ManualChoiceKind::AcceptAsTerminal)];
    let reconciliation = terminal_workflow.require_manual_resolution(
        &terminal_authority,
        Timestamp(1),
        terminal_choices.clone(),
    );
    let resolution = reconciliation.manual_resolution.expect("resolution");
    let mut commit = manual_commit(WorkflowStatus::Completed);
    commit.retry_at = Some(Timestamp(10));
    let outcome = terminal_workflow.resolve_manual(
        resolution.id,
        Version(1),
        "reviewer",
        &terminal_choices[0],
        &commit,
    );
    assert_eq!(outcome.outcome, CommitOutcome::Committed);
    assert_eq!(terminal_workflow.status, WorkflowStatus::Completed);
    assert_eq!(
        claim(&mut terminal_workflow, 1, 2, None).outcome,
        ClaimOutcome::Ineligible
    );
}

#[test]
fn incompatible_status_blocks_claims() {
    let mut workflow = workflow();
    let mut plan = base_plan();
    plan.effects.push(effect_decl(
        1,
        EffectRole::Required,
        ExecutionCapability::SafelyRepeatable,
        Generation(0),
    ));
    workflow
        .commit_transition(&decision(Version(0), plan), &BTreeMap::new())
        .expect("commit");
    assert_eq!(
        workflow.migrate_profile(&profile_v2(), Timestamp(7)),
        ProfileMigrationOutcome::Incompatible
    );
    assert_eq!(workflow.status, WorkflowStatus::Incompatible);
    assert_eq!(
        claim(&mut workflow, 1, 7, None).outcome,
        ClaimOutcome::Ineligible
    );
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
    workflow
        .commit_transition(&decision(Version(0), plan), &BTreeMap::new())
        .expect("commit");
    assert_eq!(
        workflow.migrate_profile(&profile_v2(), Timestamp(7)),
        ProfileMigrationOutcome::Incompatible
    );
    let incompatible = workflow.incompatible.expect("marker");
    assert_eq!(incompatible.disposition, "manual-preservation");
    assert_eq!(incompatible.detected_at, Timestamp(7));
}

#[test]
fn acceptance_capability_helpers_expose_profile_shape() {
    let runtime_only = runtime_only_acceptance();
    assert!(runtime_only.runtime_acceptance_enabled());
    assert!(!runtime_only.external_acceptance_enabled());

    let no_runtime = no_runtime_acceptance();
    assert!(!no_runtime.runtime_acceptance_enabled());
    assert!(no_runtime.external_acceptance_enabled());
}

#[test]
fn erased_acceptance_profile_preserves_only_valid_capability_shapes() {
    let runtime_only = erased_runtime_only_acceptance();
    assert!(runtime_only.runtime_acceptance_enabled());
    assert!(!runtime_only.external_acceptance_enabled());

    let no_runtime = erased_no_runtime_acceptance();
    assert!(!no_runtime.runtime_acceptance_enabled());
    assert!(no_runtime.external_acceptance_enabled());

    let no_acceptance = erased_no_acceptance();
    assert!(!no_acceptance.runtime_acceptance_enabled());
    assert!(!no_acceptance.external_acceptance_enabled());

    let full = acceptance().erase();
    assert!(full.runtime_acceptance_enabled());
    assert!(full.external_acceptance_enabled());
}

fn external_binding_parts(
    key: &NonEmptyExternalKey,
    workflow_id: WorkflowId,
    receipt_handle: &'static str,
    disposition_handle: &'static str,
) -> (
    ExternalAcceptanceReceipt<&'static str>,
    ExternalAcceptanceDisposition<&'static str>,
) {
    (
        ExternalAcceptanceReceipt {
            idempotency_key: key.clone(),
            workflow_id,
            handle: receipt_handle,
        },
        ExternalAcceptanceDisposition {
            workflow_id,
            handle: disposition_handle,
        },
    )
}

fn expect_created(
    outcome: ExternalAcceptanceOutcome<&'static str>,
) -> ExternalAcceptanceBinding<&'static str> {
    match outcome {
        ExternalAcceptanceOutcome::Created(binding) => binding,
        other @ (ExternalAcceptanceOutcome::Replayed(_) | ExternalAcceptanceOutcome::Conflict) => {
            panic!("unexpected outcome: {other:?}")
        }
    }
}

fn expect_replayed(
    outcome: ExternalAcceptanceOutcome<&'static str>,
) -> ExternalAcceptanceBinding<&'static str> {
    match outcome {
        ExternalAcceptanceOutcome::Replayed(binding) => binding,
        other @ (ExternalAcceptanceOutcome::Created(_) | ExternalAcceptanceOutcome::Conflict) => {
            panic!("unexpected outcome: {other:?}")
        }
    }
}

fn assert_registry_binding_shape(
    binding: &ExternalAcceptanceBinding<&'static str>,
    expected_scope: &ScopeId,
    expected_key: &NonEmptyExternalKey,
    expected_receipt: &ExternalAcceptanceReceipt<&'static str>,
    expected_disposition: &ExternalAcceptanceDisposition<&'static str>,
) {
    assert_eq!(binding.profile, profile());
    assert_eq!(&binding.target_scope, expected_scope);
    assert_eq!(&binding.idempotency_key, expected_key);
    assert_eq!(binding.intent_fingerprint, "fp-a");
    assert_eq!(&binding.receipt, expected_receipt);
    assert_eq!(&binding.disposition, expected_disposition);
}

#[allow(clippy::too_many_lines)]
#[test]
fn external_acceptance_registry_created_replayed_conflict_and_target_independence() {
    let profile_acceptance = acceptance();
    let mut registry = ExternalAcceptanceRegistry::<TestProfile, &'static str>::new();
    let key = NonEmptyExternalKey::new("client-1").expect("key");
    let target_scope = ScopeId::new("conversation:1").expect("scope");
    let (receipt, disposition) =
        external_binding_parts(&key, WorkflowId(1), "receipt", "disposition");

    let created_binding = expect_created(registry.accept(
        &profile_acceptance,
        target_scope.clone(),
        key.clone(),
        "fp-a".into(),
        receipt.clone(),
        disposition.clone(),
    ));
    assert_registry_binding_shape(
        &created_binding,
        &target_scope,
        &key,
        &receipt,
        &disposition,
    );

    let replayed_binding = expect_replayed(registry.accept(
        &profile_acceptance,
        target_scope.clone(),
        key.clone(),
        "fp-a".into(),
        created_binding.receipt.clone(),
        created_binding.disposition.clone(),
    ));
    assert_eq!(replayed_binding, created_binding);

    let (receipt_conflict_receipt, _) =
        external_binding_parts(&key, WorkflowId(2), "receipt", "disposition");
    assert_eq!(
        registry.accept(
            &profile_acceptance,
            target_scope.clone(),
            key.clone(),
            "fp-a".into(),
            receipt_conflict_receipt,
            created_binding.disposition.clone(),
        ),
        ExternalAcceptanceOutcome::Conflict
    );

    let (_, disposition_conflict_disposition) =
        external_binding_parts(&key, WorkflowId(2), "receipt", "disposition");
    assert_eq!(
        registry.accept(
            &profile_acceptance,
            target_scope.clone(),
            key.clone(),
            "fp-a".into(),
            created_binding.receipt.clone(),
            disposition_conflict_disposition,
        ),
        ExternalAcceptanceOutcome::Conflict
    );

    assert_eq!(
        registry.accept(
            &profile_acceptance,
            target_scope.clone(),
            key.clone(),
            "fp-b".into(),
            created_binding.receipt.clone(),
            created_binding.disposition.clone(),
        ),
        ExternalAcceptanceOutcome::Conflict
    );

    let exact = registry
        .get_exact(&profile(), &target_scope, &key)
        .expect("binding");
    assert_eq!(exact, &created_binding);

    let other_scope = ScopeId::new("conversation:2").expect("scope");
    assert!(matches!(
        registry.accept(
            &profile_acceptance,
            other_scope.clone(),
            key.clone(),
            "fp-z".into(),
            created_binding.receipt.clone(),
            created_binding.disposition.clone(),
        ),
        ExternalAcceptanceOutcome::Created(_)
    ));

    let other_key = NonEmptyExternalKey::new("client-2").expect("key");
    let (other_receipt, other_disposition) =
        external_binding_parts(&other_key, WorkflowId(3), "receipt-3", "disposition-3");
    assert!(matches!(
        registry.accept(
            &profile_acceptance,
            target_scope.clone(),
            other_key,
            "fp-c".into(),
            other_receipt,
            other_disposition,
        ),
        ExternalAcceptanceOutcome::Created(_)
    ));

    let exact_target_bindings = registry.list_for_target(&profile(), &target_scope);
    assert_eq!(exact_target_bindings.len(), 2);
    assert!(exact_target_bindings
        .iter()
        .all(|binding| binding.profile == profile()));
    assert!(exact_target_bindings
        .iter()
        .all(|binding| binding.target_scope == target_scope));
    assert_eq!(registry.list_for_target(&profile(), &other_scope).len(), 1);
}

#[test]
fn runtime_acceptance_can_be_suppressed_and_exactly_targets_single_item() {
    let mut first_workflow = workflow();
    let mut first_plan = base_plan();
    add_delivery_source(&mut first_plan, true);
    first_plan.deliveries.push(delivery_decl("runtime", true));
    first_plan.deliveries.push(delivery_decl_with_source(
        "plain",
        false,
        Some(EffectId(1)),
        None,
    ));
    first_workflow
        .commit_transition(&decision(Version(0), first_plan), &BTreeMap::new())
        .expect("commit");

    let runtime_id = first_workflow
        .deliveries
        .iter()
        .find(|(_, item)| item.runtime_acceptance_status == Some(RuntimeAcceptanceStatus::Owed))
        .map(|(id, _)| *id)
        .expect("runtime owed");
    let plain_id = first_workflow
        .deliveries
        .iter()
        .find(|(_, item)| item.runtime_acceptance_status.is_none())
        .map(|(id, _)| *id)
        .expect("plain");

    let accepted = first_workflow
        .accept_runtime_delivery(
            runtime_id,
            &decision(
                Version(1),
                TransitionPlan {
                    event: TestEvent::RuntimeAccept("runtime"),
                    ..base_plan()
                },
            ),
            false,
        )
        .expect("accept");
    assert_eq!(
        accepted
            .delivery
            .expect("delivery")
            .runtime_acceptance_status,
        Some(RuntimeAcceptanceStatus::Accepted)
    );
    assert_eq!(
        first_workflow.deliveries[&plain_id].runtime_acceptance_status,
        None
    );

    let mut second_workflow = workflow();
    let mut second_plan = base_plan();
    add_delivery_source(&mut second_plan, true);
    second_plan.deliveries.push(delivery_decl("runtime", true));
    second_workflow
        .commit_transition(&decision(Version(0), second_plan), &BTreeMap::new())
        .expect("commit");
    let runtime_id = *second_workflow.deliveries.keys().next().expect("delivery");
    let suppressed = second_workflow
        .accept_runtime_delivery(
            runtime_id,
            &decision(
                Version(1),
                TransitionPlan {
                    event: TestEvent::RuntimeSuppress("runtime"),
                    ..base_plan()
                },
            ),
            true,
        )
        .expect("suppress");
    let delivery = suppressed.delivery.expect("delivery");
    assert_eq!(
        delivery.runtime_acceptance_status,
        Some(RuntimeAcceptanceStatus::Suppressed)
    );
    assert_eq!(delivery.status, DeliveryStatus::Suppressed);
}

#[test]
fn schedule_coalesces_downtime_and_rejects_stale_or_duplicate_completion() {
    let mut workflow = workflow();
    let mut plan = base_plan();
    plan.schedules.push(ScheduleDecl {
        schedule_id: ScheduleId(12),
        policy: SchedulePolicy::CoalesceLatest,
        next_eligible_at: Timestamp(5),
        key: "cron-late",
    });
    workflow
        .commit_transition(&decision(Version(0), plan), &BTreeMap::new())
        .expect("commit");

    workflow.refresh_eligibility(Timestamp(100));
    let schedule = &workflow.schedules[&ScheduleId(12)];
    assert_eq!(schedule.status, ScheduleStatus::Due);
    let occurrence = schedule.due_occurrence.expect("coalesced once");
    assert_eq!(occurrence.due_at, Timestamp(5));
    assert!(workflow
        .reconcile_schedule_due(ScheduleId(12), Timestamp(100))
        .is_none());

    let started = workflow
        .start_schedule_occurrence(occurrence, Some(EffectId(77)))
        .expect("start");
    assert_eq!(started.status, ScheduleStatus::Active);
    assert_eq!(started.active_occurrence, Some(occurrence));
    assert_eq!(started.active_effect_id, Some(EffectId(77)));
    assert!(workflow
        .complete_schedule_occurrence(occurrence, Timestamp(200))
        .is_some());
    let after_duplicate = workflow.clone();
    assert!(workflow
        .complete_schedule_occurrence(occurrence, Timestamp(300))
        .is_none());
    assert_eq!(workflow, after_duplicate);
    assert_eq!(
        workflow.schedules[&ScheduleId(12)].next_eligible_at,
        Timestamp(200)
    );

    workflow.refresh_eligibility(Timestamp(250));
    let newer = workflow.schedules[&ScheduleId(12)]
        .due_occurrence
        .expect("new due");
    workflow
        .start_schedule_occurrence(newer, None)
        .expect("start newer");
    let before_stale_completion = workflow.clone();
    assert!(workflow
        .complete_schedule_occurrence(occurrence, Timestamp(500))
        .is_none());
    assert_eq!(workflow, before_stale_completion);
    let completed = workflow
        .complete_schedule_occurrence(newer, Timestamp(260))
        .expect("complete newer");
    assert_eq!(completed.status, ScheduleStatus::Idle);
    assert_eq!(completed.next_eligible_at, Timestamp(260));
    assert_eq!(
        workflow.schedules[&ScheduleId(12)].next_eligible_at,
        Timestamp(260)
    );
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
    workflow
        .commit_transition(&decision(Version(0), plan), &BTreeMap::new())
        .expect("commit");
    assert_eq!(
        workflow.reconcile_schedule_due(ScheduleId(11), Timestamp(4)),
        None
    );
    let occurrence = workflow
        .reconcile_schedule_due(ScheduleId(11), Timestamp(5))
        .expect("due");
    let due = workflow
        .start_schedule_occurrence(occurrence, None)
        .expect("started");
    assert_eq!(due.status, ScheduleStatus::Active);
    let reset = workflow
        .complete_schedule_occurrence(occurrence, Timestamp(9))
        .expect("reset");
    assert_eq!(reset.status, ScheduleStatus::Idle);
    assert_eq!(reset.next_eligible_at, Timestamp(9));
}

#[test]
fn workflow_clone_preserves_erased_acceptance_and_schedule_occurrence_state() {
    let mut workflow = workflow();
    let mut plan = base_plan();
    plan.schedules.push(ScheduleDecl {
        schedule_id: ScheduleId(21),
        policy: SchedulePolicy::CoalesceLatest,
        next_eligible_at: Timestamp(5),
        key: "cloneable-cron",
    });
    workflow
        .commit_transition(&decision(Version(0), plan), &BTreeMap::new())
        .expect("commit");
    workflow.refresh_eligibility(Timestamp(5));
    let original = workflow.schedules[&ScheduleId(21)]
        .due_occurrence
        .expect("due occurrence");

    let cloned = workflow.clone();
    assert_eq!(cloned.binding.acceptance, workflow.binding.acceptance);
    assert_eq!(
        cloned.schedules[&ScheduleId(21)].due_occurrence,
        Some(original)
    );
    assert_eq!(
        cloned.next_schedule_occurrence_id,
        workflow.next_schedule_occurrence_id
    );
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
            persisted_child_terminal_record: PersistedChildTerminalRecord::new("record")
                .expect("record"),
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
    assert!(WakeProfile::decision_handles_delivery(
        &item,
        &WakeRegistrationEvent::TerminalProjected {
            terminal: Box::new(terminal.clone())
        }
    ));
    assert!(WakeProfile::decision_handles_runtime_acceptance(
        &item,
        &WakeRegistrationEvent::RuntimeAccepted {
            terminal: Box::new(terminal.clone())
        }
    ));
    assert!(WakeProfile::decision_handles_runtime_suppression(
        &item,
        &WakeRegistrationEvent::RuntimeSuppressed {
            terminal: Box::new(terminal)
        }
    ));
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
            let mut effect = effect_decl(id, EffectRole::Required, ExecutionCapability::SafelyRepeatable, Generation(0));
            if id == 1 {
                effect.next_eligible_at = Some(Timestamp(1));
            } else {
                effect.next_eligible_at = Some(Timestamp(2));
            }
            plan.effects.push(effect);
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
        workflow.refresh_eligibility(Timestamp(2));
        for id in 1..=4u64 {
            let status = workflow.effects[&EffectId(id)].status;
            if id == 1 {
                prop_assert_eq!(status, if receipted_prefix > 0 { EffectStatus::Receipted } else { EffectStatus::Eligible });
            } else if receipted_prefix > 0 && id <= receipted_prefix.min(4) as u64 + 1 {
                prop_assert_ne!(status, EffectStatus::Blocked);
            } else {
                prop_assert_eq!(status, EffectStatus::Blocked);
            }
        }
    }
}
