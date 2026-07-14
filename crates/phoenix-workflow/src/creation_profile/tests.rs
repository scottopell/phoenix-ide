use std::collections::{BTreeMap, BTreeSet};

use proptest::prelude::*;

use crate::{
    EffectAmbiguity, EffectRole, EngineError, ExecutionMode, SemanticAuthority, WorkflowBinding,
    WorkflowId,
};

use super::{
    adapt_authoritative_creation, compensation_plan, project_authoritative_creation,
    AuthoritativeCreationOracle, AuthoritativeCreationStage, AuthoritativeCreationStatus,
    CapabilityAvailability, CleanupOwnership, CompensationPrediction, CompletionPrediction,
    CreationFailure, CreationProjectionStatus, CreationRuntimeEvidence, CreationStart,
    CreationWorkspace, EffectPrediction, WorktreeProvisioningEvidence,
    WorktreeReconciliationClassification, BOOTSTRAP_RUNTIME, COMMIT_METADATA,
    COMPENSATION_BARRIER_ID, DELETE_STAGED_ATTACHMENTS, DISPATCH_INITIAL_LLM_REQUEST,
    EXPAND_INITIAL_MESSAGE, FINALIZE_ATTACHMENTS, FINISH_CANCELLATION_OR_DELETION,
    MATERIALIZE_OR_RECONCILE_WORKTREE, RELEASE_RESERVATION, REMOVE_OWNED_WORKTREE,
    RESERVE_WORKTREE, RESOLVE_REPOSITORY, REVOKE_RUNTIME,
};

fn oracle(start: CreationStart) -> AuthoritativeCreationOracle {
    AuthoritativeCreationOracle {
        intent: super::CreationIntent {
            job_id: "job-1".into(),
            conversation_id: "conv-1".into(),
            idempotency_key: "request-1".into(),
            workspace: CreationWorkspace::Worktree {
                repository_path: "/repo".into(),
                worktree_path: "/repo-worktree".into(),
                branch_name: "task-1".into(),
            },
            attachment_ids: vec!["attachment-1".into()],
            start,
        },
        status: AuthoritativeCreationStatus::Accepted,
        stage: AuthoritativeCreationStage::ValidateIntent,
        attempt: 2,
        generation: 3,
        revision: 7,
        cleanup_ownership: CleanupOwnership::None,
        worktree_evidence: WorktreeProvisioningEvidence::None,
        runtime_evidence: CreationRuntimeEvidence::no_runtime_signals(),
    }
}

fn dependencies(
    adapter: &super::CreationShadowAdapter,
) -> BTreeSet<(crate::EffectId, crate::EffectId)> {
    adapter
        .plan
        .dependencies
        .iter()
        .map(|dependency| (dependency.effect_id, dependency.depends_on_effect_id))
        .collect()
}

#[test]
fn initial_turn_dag_has_required_order_and_completion_barrier() {
    let adapter = adapt_authoritative_creation(
        WorkflowId(10),
        WorkflowId(77),
        &oracle(CreationStart::InitialTurn {
            message_id: "message-1".into(),
            text: "do the thing".into(),
        }),
    )
    .expect("shadow adapter");
    let ids = adapter
        .plan
        .effects
        .iter()
        .map(|effect| effect.effect_id)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        ids,
        BTreeSet::from([
            RESOLVE_REPOSITORY,
            RESERVE_WORKTREE,
            MATERIALIZE_OR_RECONCILE_WORKTREE,
            FINALIZE_ATTACHMENTS,
            EXPAND_INITIAL_MESSAGE,
            COMMIT_METADATA,
            BOOTSTRAP_RUNTIME,
            DISPATCH_INITIAL_LLM_REQUEST,
        ])
    );
    assert!(adapter
        .plan
        .effects
        .iter()
        .all(|effect| effect.role == EffectRole::Required));
    let dependencies = dependencies(&adapter);
    for edge in [
        (RESERVE_WORKTREE, RESOLVE_REPOSITORY),
        (MATERIALIZE_OR_RECONCILE_WORKTREE, RESERVE_WORKTREE),
        (FINALIZE_ATTACHMENTS, RESOLVE_REPOSITORY),
        (EXPAND_INITIAL_MESSAGE, MATERIALIZE_OR_RECONCILE_WORKTREE),
        (EXPAND_INITIAL_MESSAGE, FINALIZE_ATTACHMENTS),
        (COMMIT_METADATA, EXPAND_INITIAL_MESSAGE),
        (BOOTSTRAP_RUNTIME, COMMIT_METADATA),
        (DISPATCH_INITIAL_LLM_REQUEST, BOOTSTRAP_RUNTIME),
    ] {
        assert!(dependencies.contains(&edge), "missing dependency {edge:?}");
    }
    assert_eq!(adapter.plan.barriers.len(), 1);
    assert_eq!(
        adapter.plan.barriers[0].barrier_id,
        super::COMPLETION_BARRIER_ID
    );
    assert_eq!(adapter.plan.barrier_members.len(), 8);
    assert_eq!(
        adapter.projection.readiness_effects,
        adapter
            .plan
            .barrier_members
            .iter()
            .map(|member| member.effect_id)
            .collect::<Vec<_>>()
    );
}

#[test]
fn direct_creation_omits_worktree_effects() {
    let mut oracle = oracle(CreationStart::SeededEmpty);
    oracle.intent.workspace = CreationWorkspace::Direct {
        cwd: "/repo-direct".into(),
    };
    let adapted = adapt_authoritative_creation(WorkflowId(31), WorkflowId(101), &oracle).unwrap();
    let plan = adapted.plan;
    let ids = plan
        .effects
        .iter()
        .map(|effect| effect.effect_id)
        .collect::<BTreeSet<_>>();
    assert!(!ids.contains(&RESERVE_WORKTREE));
    assert!(!ids.contains(&MATERIALIZE_OR_RECONCILE_WORKTREE));
}

#[test]
fn seeded_empty_has_distinct_required_dag_without_expansion_or_dispatch() {
    let adapter = adapt_authoritative_creation(
        WorkflowId(11),
        WorkflowId(78),
        &oracle(CreationStart::SeededEmpty),
    )
    .expect("shadow adapter");
    let ids = adapter
        .plan
        .effects
        .iter()
        .map(|effect| effect.effect_id)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        ids,
        BTreeSet::from([
            RESOLVE_REPOSITORY,
            RESERVE_WORKTREE,
            MATERIALIZE_OR_RECONCILE_WORKTREE,
            FINALIZE_ATTACHMENTS,
            COMMIT_METADATA,
        ])
    );
    assert!(!ids.contains(&EXPAND_INITIAL_MESSAGE));
    assert!(!ids.contains(&DISPATCH_INITIAL_LLM_REQUEST));
    assert!(!ids.contains(&BOOTSTRAP_RUNTIME));
    assert_eq!(adapter.plan.barrier_members.len(), 5);
    assert_eq!(
        adapter.projection.readiness_effects,
        adapter
            .plan
            .barrier_members
            .iter()
            .map(|member| member.effect_id)
            .collect::<Vec<_>>()
    );
    assert!(adapter
        .projection
        .effect_predictions
        .iter()
        .all(|(id, _)| !matches!(
            id,
            &EXPAND_INITIAL_MESSAGE | &BOOTSTRAP_RUNTIME | &DISPATCH_INITIAL_LLM_REQUEST
        )));
}

#[test]
fn compensation_dag_orders_destructive_cleanup_and_finish_barrier() {
    let mut oracle = oracle(CreationStart::SeededEmpty);
    oracle.status = AuthoritativeCreationStatus::DeletionPending;
    oracle.cleanup_ownership = CleanupOwnership::OwnedResources;
    let plan = compensation_plan(&oracle);
    let ids = plan
        .effects
        .iter()
        .map(|effect| effect.effect_id)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        ids,
        BTreeSet::from([
            REVOKE_RUNTIME,
            REMOVE_OWNED_WORKTREE,
            RELEASE_RESERVATION,
            DELETE_STAGED_ATTACHMENTS,
            FINISH_CANCELLATION_OR_DELETION,
        ])
    );
    assert!(plan
        .effects
        .iter()
        .all(|effect| effect.role == EffectRole::Compensation));
    let dependencies = plan
        .dependencies
        .iter()
        .map(|dependency| (dependency.effect_id, dependency.depends_on_effect_id))
        .collect::<BTreeSet<_>>();
    assert!(dependencies.contains(&(DELETE_STAGED_ATTACHMENTS, REVOKE_RUNTIME)));
    assert!(dependencies.contains(&(REMOVE_OWNED_WORKTREE, DELETE_STAGED_ATTACHMENTS)));
    assert!(dependencies.contains(&(RELEASE_RESERVATION, REMOVE_OWNED_WORKTREE)));
    assert!(dependencies.contains(&(FINISH_CANCELLATION_OR_DELETION, RELEASE_RESERVATION)));
    assert!(dependencies.contains(&(FINISH_CANCELLATION_OR_DELETION, DELETE_STAGED_ATTACHMENTS)));
    assert_eq!(plan.barriers[0].barrier_id, COMPENSATION_BARRIER_ID);
    assert_eq!(plan.barrier_members.len(), 5);

    let adapter = adapt_authoritative_creation(WorkflowId(13), WorkflowId(80), &oracle)
        .expect("compensation adapter");
    assert_eq!(
        adapter.projection.readiness_effects,
        adapter
            .plan
            .barrier_members
            .iter()
            .map(|member| member.effect_id)
            .collect::<Vec<_>>()
    );
}

#[test]
fn compensation_projection_predicts_every_selected_effect() {
    let mut oracle = oracle(CreationStart::SeededEmpty);
    oracle.status = AuthoritativeCreationStatus::DeletionPending;
    let adapter = adapt_authoritative_creation(WorkflowId(13), WorkflowId(80), &oracle)
        .expect("compensation adapter");
    let ids = adapter
        .plan
        .effects
        .iter()
        .map(|effect| effect.effect_id)
        .collect::<BTreeSet<_>>();
    let predictions = adapter
        .projection
        .effect_predictions
        .iter()
        .copied()
        .collect::<BTreeMap<_, _>>();
    assert_eq!(predictions.keys().copied().collect::<BTreeSet<_>>(), ids);
    assert_eq!(
        predictions.get(&REVOKE_RUNTIME),
        Some(&EffectPrediction::Eligible)
    );
    assert_eq!(
        predictions.get(&FINISH_CANCELLATION_OR_DELETION),
        Some(&EffectPrediction::Blocked)
    );
}

#[test]
fn declared_effect_policies_and_prebootstrap_readiness_match_profile() {
    let oracle = oracle(CreationStart::InitialTurn {
        message_id: "msg-policy".to_owned(),
        text: "do the thing".to_owned(),
    });
    let adapter = adapt_authoritative_creation(WorkflowId(14), WorkflowId(81), &oracle)
        .expect("initial-turn adapter");
    let policies = adapter
        .plan
        .effects
        .iter()
        .map(|effect| (effect.effect_id, effect.ambiguity))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(
        policies.get(&RESOLVE_REPOSITORY),
        Some(&EffectAmbiguity::ObservableReconciliation)
    );
    assert_eq!(
        policies.get(&EXPAND_INITIAL_MESSAGE),
        Some(&EffectAmbiguity::ExternalIdempotency)
    );
    assert_eq!(
        policies.get(&COMMIT_METADATA),
        Some(&EffectAmbiguity::ExternalIdempotency)
    );
    assert_eq!(
        policies.get(&BOOTSTRAP_RUNTIME),
        Some(&EffectAmbiguity::ExternalIdempotency)
    );
    assert!(adapter
        .projection
        .effect_predictions
        .contains(&(BOOTSTRAP_RUNTIME, EffectPrediction::Blocked)));
}

#[test]
fn adapter_has_no_execution_or_semantic_authority_leakage() {
    let mut adapter = adapt_authoritative_creation(
        WorkflowId(12),
        WorkflowId(79),
        &oracle(CreationStart::SeededEmpty),
    )
    .expect("shadow adapter");
    assert_eq!(
        adapter.workflow.binding.execution_mode(),
        ExecutionMode::Shadow
    );
    assert!(matches!(
        adapter.workflow.binding,
        WorkflowBinding::Shadow(_)
    ));
    assert_eq!(adapter.workflow.semantic_authority, None);
    let protocol = adapter.workflow.binding.accepted_protocol();
    assert_eq!(protocol.authority, SemanticAuthority::LegacyProtocol);
    assert!(!protocol.runtime_acceptance_enabled);
    assert!(!protocol.external_acceptance_enabled);
    assert_eq!(
        adapter.workflow.claim_effect(
            RESOLVE_REPOSITORY,
            "forbidden-worker",
            crate::Timestamp(1),
            crate::LeaseExpiry(5),
        ),
        crate::ClaimResult {
            outcome: crate::ClaimOutcome::AuthorityConflict,
            authority: None,
            attempt: None,
        }
    );
    let plan = adapter.plan;
    let mut workflow = adapter.workflow;
    assert_eq!(
        workflow.commit_transition(
            &crate::ReducerDecision {
                expected_workflow_version: workflow.version,
                plan,
            },
            &BTreeMap::new()
        ),
        Err(EngineError::ShadowCannotExecute)
    );
}

#[test]
fn all_reconciliation_classifications_are_typed_and_distinct() {
    let classifications = BTreeSet::from([
        WorktreeReconciliationClassification::AbsentPath,
        WorktreeReconciliationClassification::ValidOwnedWorktree,
        WorktreeReconciliationClassification::PartialOwnedDirectory,
        WorktreeReconciliationClassification::ForeignGitRoot,
        WorktreeReconciliationClassification::ConflictingBranchOrWorktree,
        WorktreeReconciliationClassification::MissingRepository,
        WorktreeReconciliationClassification::TransientInfrastructureFailure,
    ]);
    assert_eq!(classifications.len(), 7);
}

fn add_ready_evidence(oracle: &mut AuthoritativeCreationOracle) {
    if matches!(oracle.status, AuthoritativeCreationStatus::Ready) {
        oracle.runtime_evidence = CreationRuntimeEvidence::ready(super::capabilities([
            true, true, true, false, false, true,
        ]));
    }
}

#[test]
fn status_table_maps_visibility_capabilities_and_predictions() {
    let failure = CreationFailure {
        kind: "permanent".into(),
        message: "no repository".into(),
    };
    let cases = [
        (
            AuthoritativeCreationStatus::Accepted,
            CreationProjectionStatus::Provisioning,
            false,
            false,
            true,
            false,
            CompletionPrediction::Pending,
            CompensationPrediction::None,
        ),
        (
            AuthoritativeCreationStatus::Claimed {
                worker_id: "worker".into(),
            },
            CreationProjectionStatus::Provisioning,
            false,
            false,
            true,
            false,
            CompletionPrediction::Pending,
            CompensationPrediction::None,
        ),
        (
            AuthoritativeCreationStatus::Cancelling,
            CreationProjectionStatus::Cancelled,
            false,
            false,
            false,
            true,
            CompletionPrediction::Cancelled,
            CompensationPrediction::RequiredForCancellation,
        ),
        (
            AuthoritativeCreationStatus::Failed(failure),
            CreationProjectionStatus::Failed,
            false,
            false,
            false,
            true,
            CompletionPrediction::Failed,
            CompensationPrediction::None,
        ),
        (
            AuthoritativeCreationStatus::Cancelled,
            CreationProjectionStatus::Cancelled,
            false,
            false,
            false,
            true,
            CompletionPrediction::Cancelled,
            CompensationPrediction::None,
        ),
        (
            AuthoritativeCreationStatus::DeletionPending,
            CreationProjectionStatus::DeletionPending,
            false,
            true,
            false,
            false,
            CompletionPrediction::DeletionPending,
            CompensationPrediction::RequiredForDeletion,
        ),
        (
            AuthoritativeCreationStatus::Ready,
            CreationProjectionStatus::Ready,
            true,
            false,
            false,
            false,
            CompletionPrediction::Complete,
            CompensationPrediction::None,
        ),
    ];
    for (status, expected_status, runtime, hidden, cancel, start_over, completion, compensation) in
        cases
    {
        let mut oracle = oracle(CreationStart::SeededEmpty);
        oracle.status = status;
        add_ready_evidence(&mut oracle);
        let projection = project_authoritative_creation(&oracle);
        assert_eq!(projection.status, expected_status);
        let availability = |allowed| {
            if allowed {
                CapabilityAvailability::Allowed
            } else {
                CapabilityAvailability::Forbidden
            }
        };
        assert_eq!(projection.capabilities.runtime, availability(runtime));
        assert_eq!(projection.hidden, hidden);
        assert_eq!(projection.capabilities.cancel, availability(cancel));
        assert_eq!(projection.capabilities.start_over, availability(start_over));
        assert_eq!(projection.completion, completion);
        assert_eq!(projection.compensation, compensation);
    }
}

#[test]
fn cleanup_states_adapt_to_compensation_graphs() {
    let statuses = [
        AuthoritativeCreationStatus::Cancelling,
        AuthoritativeCreationStatus::DeletionPending,
    ];
    for status in statuses {
        let mut oracle = oracle(CreationStart::SeededEmpty);
        oracle.status = status;
        oracle.cleanup_ownership = CleanupOwnership::OwnedResources;
        let adapted =
            adapt_authoritative_creation(WorkflowId(20), WorkflowId(90), &oracle).unwrap();
        assert_eq!(adapted.plan.effects.len(), 5);
        assert!(adapted
            .plan
            .effects
            .iter()
            .all(|effect| effect.role == crate::EffectRole::Compensation));
        assert_eq!(adapted.plan.dependencies.len(), 5);
    }
}

#[test]
fn completed_cancellation_has_no_compensation_effects() {
    let mut cancelled = oracle(CreationStart::SeededEmpty);
    cancelled.status = AuthoritativeCreationStatus::Cancelled;
    cancelled.cleanup_ownership = CleanupOwnership::OwnedResources;
    let adapted = adapt_authoritative_creation(WorkflowId(20), WorkflowId(90), &cancelled).unwrap();
    assert!(adapted
        .plan
        .effects
        .iter()
        .all(|effect| effect.role != crate::EffectRole::Compensation));
}

#[test]
fn early_cancellation_omits_unowned_resource_cleanup() {
    let mut oracle = oracle(CreationStart::SeededEmpty);
    oracle.status = AuthoritativeCreationStatus::Cancelling;
    oracle.cleanup_ownership = CleanupOwnership::None;
    let adapted = adapt_authoritative_creation(WorkflowId(21), WorkflowId(91), &oracle).unwrap();
    let ids = adapted
        .plan
        .effects
        .iter()
        .map(|effect| effect.effect_id)
        .collect::<BTreeSet<_>>();
    assert!(!ids.contains(&REMOVE_OWNED_WORKTREE));
    assert!(!ids.contains(&RELEASE_RESERVATION));
    assert_eq!(ids.len(), 3);
}

#[test]
fn failed_jobs_only_adapt_to_compensation_when_cleanup_resources_are_owned() {
    let mut failed = oracle(CreationStart::SeededEmpty);
    failed.status = AuthoritativeCreationStatus::Failed(CreationFailure {
        kind: "validation".into(),
        message: "failed before reservation".into(),
    });
    let without_resources =
        adapt_authoritative_creation(WorkflowId(21), WorkflowId(91), &failed).unwrap();
    assert!(without_resources
        .plan
        .effects
        .iter()
        .all(|effect| effect.role == EffectRole::Required));

    failed.cleanup_ownership = CleanupOwnership::OwnedResources;
    let with_resources =
        adapt_authoritative_creation(WorkflowId(22), WorkflowId(92), &failed).unwrap();
    assert!(with_resources
        .plan
        .effects
        .iter()
        .all(|effect| effect.role == EffectRole::Compensation));
}

#[test]
fn finalize_stage_does_not_predict_runtime_or_dispatch_completion_without_evidence() {
    let mut pending = oracle(CreationStart::InitialTurn {
        message_id: "message-1".into(),
        text: "do the thing".into(),
    });
    pending.stage = AuthoritativeCreationStage::Finalize;
    pending.status = AuthoritativeCreationStatus::Ready;
    let projection = project_authoritative_creation(&pending);
    let predictions = projection
        .effect_predictions
        .into_iter()
        .collect::<BTreeMap<_, _>>();
    assert_eq!(predictions[&BOOTSTRAP_RUNTIME], EffectPrediction::Eligible);
    assert_eq!(
        predictions[&DISPATCH_INITIAL_LLM_REQUEST],
        EffectPrediction::Blocked
    );
    assert_eq!(projection.completion, CompletionPrediction::Pending);

    pending.runtime_evidence =
        CreationRuntimeEvidence::ready(super::capabilities([true, true, true, false, false, true]));
    let complete = project_authoritative_creation(&pending);
    assert_eq!(complete.completion, CompletionPrediction::Complete);
    assert!(complete
        .effect_predictions
        .contains(&(DISPATCH_INITIAL_LLM_REQUEST, EffectPrediction::Completed)));
}

#[test]
fn committed_boundaries_complete_effects_before_later_stage_checkpoints() {
    let mut oracle = oracle(CreationStart::InitialTurn {
        message_id: "message-1".into(),
        text: "do the thing".into(),
    });
    oracle.stage = AuthoritativeCreationStage::ReserveResources;
    oracle.cleanup_ownership = CleanupOwnership::OwnedResources;
    oracle.worktree_evidence = WorktreeProvisioningEvidence::Reserved;
    let reservation_projection = project_authoritative_creation(&oracle);
    let reservation_by_id = reservation_projection
        .effect_predictions
        .into_iter()
        .collect::<BTreeMap<_, _>>();
    assert_eq!(
        reservation_by_id[&RESERVE_WORKTREE],
        EffectPrediction::Completed
    );

    oracle.cleanup_ownership = CleanupOwnership::HistoricalReservation;
    let released_projection = project_authoritative_creation(&oracle);
    let released_by_id = released_projection
        .effect_predictions
        .into_iter()
        .collect::<BTreeMap<_, _>>();
    assert_eq!(
        released_by_id[&RESERVE_WORKTREE],
        EffectPrediction::Completed
    );
    oracle.status = AuthoritativeCreationStatus::Failed(CreationFailure {
        kind: "failed".into(),
        message: "failed".into(),
    });
    let released_adapter =
        adapt_authoritative_creation(WorkflowId(21), WorkflowId(91), &oracle).unwrap();
    assert!(!released_adapter.plan.effects.iter().any(|effect| matches!(
        effect.effect_id,
        REMOVE_OWNED_WORKTREE | RELEASE_RESERVATION
    )));

    oracle.status = AuthoritativeCreationStatus::Claimed {
        worker_id: "worker-1".into(),
    };
    oracle.stage = AuthoritativeCreationStage::CommitMetadata;
    let metadata_projection = project_authoritative_creation(&oracle);
    let metadata_by_id = metadata_projection
        .effect_predictions
        .into_iter()
        .collect::<BTreeMap<_, _>>();
    assert_eq!(
        metadata_by_id[&COMMIT_METADATA],
        EffectPrediction::Completed
    );
}

proptest! {
    #[test]
    fn adapter_is_deterministic_and_preserves_owned_runtime_strings(
        job_id in "[a-z0-9-]{1,16}",
        conversation_id in "[a-z0-9-]{1,16}",
        generation in 0_u64..1000,
        attempt in 0_u32..100,
    ) {
        let mut oracle = oracle(CreationStart::InitialTurn {
            message_id: format!("message-{job_id}"),
            text: "do the thing".into(),
        });
        oracle.intent.job_id = job_id.clone();
        oracle.intent.conversation_id = conversation_id.clone();
        oracle.intent.start = CreationStart::InitialTurn {
            message_id: format!("message-{job_id}"),
            text: format!("runtime text {job_id}"),
        };
        oracle.generation = generation;
        oracle.attempt = attempt;
        let first = adapt_authoritative_creation(WorkflowId(20), WorkflowId(90), &oracle).unwrap();
        let second = adapt_authoritative_creation(WorkflowId(20), WorkflowId(90), &oracle).unwrap();
        prop_assert_eq!(&first, &second);
        prop_assert_eq!(first.workflow.snapshot.intent.job_id, job_id);
        prop_assert_eq!(first.workflow.snapshot.intent.conversation_id, conversation_id);
        prop_assert_eq!(first.workflow.generation, crate::Generation(0));
        prop_assert!(matches!(first.workflow.binding, WorkflowBinding::Shadow(_)));
    }

    #[test]
    fn effect_prediction_is_monotonic_across_authoritative_stages(stage_index in 0_usize..9) {
        let stages = [
            AuthoritativeCreationStage::ValidateIntent,
            AuthoritativeCreationStage::ResolveRepository,
            AuthoritativeCreationStage::ReserveResources,
            AuthoritativeCreationStage::MaterializeWorktree,
            AuthoritativeCreationStage::FinalizeAttachments,
            AuthoritativeCreationStage::ExpandInitialMessage,
            AuthoritativeCreationStage::CommitMetadata,
            AuthoritativeCreationStage::BootstrapInitialTurn,
            AuthoritativeCreationStage::Finalize,
        ];
        let mut oracle = oracle(CreationStart::InitialTurn {
            message_id: "message-1".into(),
            text: "do the thing".into(),
        });
        oracle.stage = stages[stage_index];
        let projection = project_authoritative_creation(&oracle);
        let by_id = projection.effect_predictions.into_iter().collect::<BTreeMap<_, _>>();
        let ordered = [RESOLVE_REPOSITORY, RESERVE_WORKTREE, MATERIALIZE_OR_RECONCILE_WORKTREE, FINALIZE_ATTACHMENTS];
        let completed_prefix = ordered.iter().take_while(|id| by_id[id] == EffectPrediction::Completed).count();
        prop_assert!(ordered[..completed_prefix].iter().all(|id| by_id[id] == EffectPrediction::Completed));
        prop_assert!(ordered[completed_prefix..].iter().all(|id| by_id[id] != EffectPrediction::Completed));
    }
}
