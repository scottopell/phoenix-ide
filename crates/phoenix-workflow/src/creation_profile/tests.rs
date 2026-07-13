use std::collections::{BTreeMap, BTreeSet};

use proptest::prelude::*;

use crate::{
    EffectRole, EngineError, ExecutionMode, SemanticAuthority, WorkflowBinding, WorkflowId,
};

use super::{
    adapt_authoritative_creation, compensation_plan, project_authoritative_creation,
    AuthoritativeCreationOracle, AuthoritativeCreationStage, AuthoritativeCreationStatus,
    CapabilityAvailability, CompensationPrediction, CompletionPrediction, CreationFailure,
    CreationKind, CreationProjectionStatus, EffectPrediction, WorktreeReconciliationClassification,
    COMMIT_METADATA, COMPENSATION_BARRIER_ID, DELETE_STAGED_ATTACHMENTS,
    DISPATCH_INITIAL_LLM_REQUEST, EXPAND_INITIAL_MESSAGE, FINALIZE_ATTACHMENTS,
    FINISH_CANCELLATION_OR_DELETION, MATERIALIZE_OR_RECONCILE_WORKTREE, RELEASE_RESERVATION,
    REMOVE_OWNED_WORKTREE, RESERVE_WORKTREE, RESOLVE_REPOSITORY, REVOKE_RUNTIME,
};

fn oracle(kind: CreationKind) -> AuthoritativeCreationOracle {
    AuthoritativeCreationOracle {
        intent: super::CreationIntent {
            job_id: "job-1".into(),
            conversation_id: "conv-1".into(),
            idempotency_key: "request-1".into(),
            repository_path: "/repo".into(),
            worktree_path: "/repo-worktree".into(),
            branch_name: "task-1".into(),
            initial_text: "do the thing".into(),
            attachment_ids: vec!["attachment-1".into()],
            kind,
        },
        status: AuthoritativeCreationStatus::Accepted,
        stage: AuthoritativeCreationStage::ValidateIntent,
        attempt: 0,
        generation: 3,
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
        &oracle(CreationKind::InitialTurn {
            message_id: "message-1".into(),
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
        (FINALIZE_ATTACHMENTS, MATERIALIZE_OR_RECONCILE_WORKTREE),
        (EXPAND_INITIAL_MESSAGE, FINALIZE_ATTACHMENTS),
        (COMMIT_METADATA, MATERIALIZE_OR_RECONCILE_WORKTREE),
        (COMMIT_METADATA, FINALIZE_ATTACHMENTS),
        (DISPATCH_INITIAL_LLM_REQUEST, COMMIT_METADATA),
        (DISPATCH_INITIAL_LLM_REQUEST, EXPAND_INITIAL_MESSAGE),
    ] {
        assert!(dependencies.contains(&edge), "missing dependency {edge:?}");
    }
    assert_eq!(adapter.plan.barriers.len(), 1);
    assert_eq!(
        adapter.plan.barriers[0].barrier_id,
        super::COMPLETION_BARRIER_ID
    );
    assert_eq!(adapter.plan.barrier_members.len(), 7);
}

#[test]
fn seeded_empty_has_distinct_required_dag_without_expansion_or_dispatch() {
    let adapter = adapt_authoritative_creation(
        WorkflowId(11),
        WorkflowId(78),
        &oracle(CreationKind::SeededEmpty),
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
    assert_eq!(adapter.plan.barrier_members.len(), 5);
    assert!(adapter
        .projection
        .effect_predictions
        .contains(&(EXPAND_INITIAL_MESSAGE, EffectPrediction::Omitted)));
    assert!(adapter
        .projection
        .effect_predictions
        .contains(&(DISPATCH_INITIAL_LLM_REQUEST, EffectPrediction::Omitted)));
}

#[test]
fn compensation_dag_orders_destructive_cleanup_and_finish_barrier() {
    let mut oracle = oracle(CreationKind::SeededEmpty);
    oracle.status = AuthoritativeCreationStatus::DeletionPending;
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
}

#[test]
fn adapter_has_no_execution_or_semantic_authority_leakage() {
    let mut adapter = adapt_authoritative_creation(
        WorkflowId(12),
        WorkflowId(79),
        &oracle(CreationKind::SeededEmpty),
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
        let mut oracle = oracle(CreationKind::SeededEmpty);
        oracle.status = status;
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
        AuthoritativeCreationStatus::Cancelled,
        AuthoritativeCreationStatus::Failed(CreationFailure {
            kind: "permanent".into(),
            message: "cleanup still required".into(),
        }),
    ];
    for status in statuses {
        let mut oracle = oracle(CreationKind::SeededEmpty);
        oracle.status = status;
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

proptest! {
    #[test]
    fn adapter_is_deterministic_and_preserves_owned_runtime_strings(
        job_id in "[a-z0-9-]{1,16}",
        conversation_id in "[a-z0-9-]{1,16}",
        generation in 0_u64..1000,
        attempt in 0_u32..100,
    ) {
        let mut oracle = oracle(CreationKind::InitialTurn { message_id: format!("message-{job_id}") });
        oracle.intent.job_id = job_id.clone();
        oracle.intent.conversation_id = conversation_id.clone();
        oracle.intent.initial_text = format!("runtime text {job_id}");
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
        let mut oracle = oracle(CreationKind::InitialTurn { message_id: "message-1".into() });
        oracle.stage = stages[stage_index];
        let projection = project_authoritative_creation(&oracle);
        let by_id = projection.effect_predictions.into_iter().collect::<BTreeMap<_, _>>();
        let ordered = [RESOLVE_REPOSITORY, RESERVE_WORKTREE, MATERIALIZE_OR_RECONCILE_WORKTREE, FINALIZE_ATTACHMENTS];
        let completed_prefix = ordered.iter().take_while(|id| by_id[id] == EffectPrediction::Completed).count();
        prop_assert!(ordered[..completed_prefix].iter().all(|id| by_id[id] == EffectPrediction::Completed));
        prop_assert!(ordered[completed_prefix..].iter().all(|id| by_id[id] != EffectPrediction::Completed));
    }
}
