use crate::{
    BarrierDecl, BarrierId, BarrierMemberDecl, CodecRef, DependencyDecl, EffectAmbiguity,
    EffectDecl, EffectId, EffectRole, Generation, ProfileRef, ProtocolSelection, ReceiptFamily,
    ReducerInboxPayload, SemanticAuthority, TransitionPlan, WorkflowId, WorkflowProfile,
    WorkflowState, WorkflowStatus,
};

pub const PROFILE_ID: &str = "conversation_creation";
pub const PROTOCOL_VERSION: u32 = 1;
pub const COMPLETION_BARRIER_ID: BarrierId = BarrierId(1);
pub const COMPENSATION_BARRIER_ID: BarrierId = BarrierId(2);

pub const RESOLVE_REPOSITORY: EffectId = EffectId(1);
pub const RESERVE_WORKTREE: EffectId = EffectId(2);
pub const MATERIALIZE_OR_RECONCILE_WORKTREE: EffectId = EffectId(3);
pub const FINALIZE_ATTACHMENTS: EffectId = EffectId(4);
pub const EXPAND_INITIAL_MESSAGE: EffectId = EffectId(5);
pub const COMMIT_METADATA: EffectId = EffectId(6);
pub const DISPATCH_INITIAL_LLM_REQUEST: EffectId = EffectId(7);
pub const BOOTSTRAP_RUNTIME: EffectId = EffectId(8);

pub const REVOKE_RUNTIME: EffectId = EffectId(101);
pub const REMOVE_OWNED_WORKTREE: EffectId = EffectId(102);
pub const RELEASE_RESERVATION: EffectId = EffectId(103);
pub const DELETE_STAGED_ATTACHMENTS: EffectId = EffectId(104);
pub const FINISH_CANCELLATION_OR_DELETION: EffectId = EffectId(105);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreationKind {
    InitialTurn { message_id: String },
    SeededEmpty,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreationIntent {
    pub job_id: String,
    pub conversation_id: String,
    pub idempotency_key: String,
    pub repository_path: String,
    pub worktree_path: String,
    pub uses_worktree: bool,
    pub branch_name: String,
    pub initial_text: String,
    pub attachment_ids: Vec<String>,
    pub kind: CreationKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AuthoritativeCreationStage {
    ValidateIntent,
    ResolveRepository,
    ReserveResources,
    MaterializeWorktree,
    FinalizeAttachments,
    ExpandInitialMessage,
    CommitMetadata,
    BootstrapInitialTurn,
    Finalize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreationFailure {
    pub kind: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthoritativeCreationStatus {
    Accepted,
    Claimed {
        worker_id: String,
    },
    RetryScheduled {
        next_attempt_at: u64,
        error: CreationFailure,
    },
    Cancelling,
    Cancelled,
    DeletionPending,
    Ready,
    Failed(CreationFailure),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupOwnership {
    None,
    HistoricalReservation,
    OwnedResources,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CreationRuntimeEvidence {
    pub runtime_bootstrapped: bool,
    pub initial_llm_dispatched: bool,
    pub ready_capabilities: Option<CreationCapabilities>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthoritativeCreationOracle {
    pub intent: CreationIntent,
    pub status: AuthoritativeCreationStatus,
    pub stage: AuthoritativeCreationStage,
    pub attempt: u32,
    pub generation: u64,
    /// Committed source revision for monotonic diagnostic projection writes.
    pub revision: u64,
    pub cleanup_ownership: CleanupOwnership,
    pub runtime_evidence: CreationRuntimeEvidence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum WorktreeReconciliationClassification {
    AbsentPath,
    ValidOwnedWorktree,
    PartialOwnedDirectory,
    ForeignGitRoot,
    ConflictingBranchOrWorktree,
    MissingRepository,
    TransientInfrastructureFailure,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreationEffectIntent {
    ResolveRepository {
        repository_path: String,
    },
    ReserveWorktree {
        repository_path: String,
        worktree_path: String,
        branch_name: String,
    },
    MaterializeOrReconcileWorktree {
        repository_path: String,
        worktree_path: String,
        branch_name: String,
    },
    FinalizeAttachments {
        conversation_id: String,
        attachment_ids: Vec<String>,
    },
    ExpandInitialMessage {
        conversation_id: String,
        message_id: String,
        text: String,
    },
    CommitMetadata {
        conversation_id: String,
        worktree_path: String,
    },
    BootstrapRuntime {
        conversation_id: String,
    },
    DispatchInitialLlmRequest {
        conversation_id: String,
        message_id: String,
    },
    RevokeRuntime {
        conversation_id: String,
    },
    RemoveOwnedWorktree {
        repository_path: String,
        worktree_path: String,
    },
    ReleaseReservation {
        conversation_id: String,
        worktree_path: String,
    },
    DeleteStagedAttachments {
        conversation_id: String,
        attachment_ids: Vec<String>,
    },
    FinishCancellationOrDeletion {
        conversation_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreationObservation {
    RepositoryResolved {
        repository_path: String,
    },
    WorktreeInspected {
        worktree_path: String,
        classification: WorktreeReconciliationClassification,
    },
    AuthoritativeStageObserved {
        job_id: String,
        stage: AuthoritativeCreationStage,
    },
    FailureObserved(CreationFailure),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreationReceipt {
    RepositoryResolved {
        repository_path: String,
    },
    ReservationHeld {
        worktree_path: String,
    },
    WorktreeReconciled {
        worktree_path: String,
        classification: WorktreeReconciliationClassification,
    },
    AttachmentsFinalized {
        attachment_ids: Vec<String>,
    },
    InitialMessageExpanded {
        message_id: String,
    },
    MetadataCommitted {
        conversation_id: String,
    },
    RuntimeBootstrapped {
        conversation_id: String,
    },
    InitialLlmRequestDispatched {
        request_id: String,
    },
    RuntimeRevoked {
        conversation_id: String,
    },
    OwnedWorktreeRemoved {
        worktree_path: String,
    },
    ReservationReleased {
        worktree_path: String,
    },
    StagedAttachmentsDeleted {
        attachment_ids: Vec<String>,
    },
    CompensationFinished {
        conversation_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreationEvent {
    ShadowPlanProjected {
        job_id: String,
    },
    AuthoritativeProgressObserved {
        job_id: String,
        stage: AuthoritativeCreationStage,
    },
    CancellationOrDeletionProjected {
        job_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreationBarrierEvent {
    CreationCompleted { conversation_id: String },
    CompensationCompleted { conversation_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreationManualPayload {
    AdoptOwnedResource { worktree_path: String },
    RepairOwnedResource { worktree_path: String },
    PreserveForeignResource { worktree_path: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreationSnapshot {
    pub intent: CreationIntent,
    pub authoritative_status: AuthoritativeCreationStatus,
    pub authoritative_stage: AuthoritativeCreationStage,
    pub attempt: u32,
    pub generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreationProjectionStatus {
    Provisioning,
    Failed,
    Cancelled,
    DeletionPending,
    Ready,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityAvailability {
    Allowed,
    Forbidden,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CreationCapabilities {
    pub read: CapabilityAvailability,
    pub write: CapabilityAvailability,
    pub runtime: CapabilityAvailability,
    pub cancel: CapabilityAvailability,
    pub start_over: CapabilityAvailability,
    pub delete: CapabilityAvailability,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectPrediction {
    Completed,
    Eligible,
    Blocked,
    Omitted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionPrediction {
    Pending,
    Complete,
    Failed,
    Cancelled,
    DeletionPending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompensationPrediction {
    None,
    RequiredForCancellation,
    RequiredForDeletion,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreationProjection {
    pub conversation_id: String,
    pub kind: CreationKind,
    pub readiness_effects: Vec<EffectId>,
    pub status: CreationProjectionStatus,
    pub capabilities: CreationCapabilities,
    pub effect_predictions: Vec<(EffectId, EffectPrediction)>,
    pub completion: CompletionPrediction,
    pub compensation: CompensationPrediction,
    pub hidden: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreationProfile;

impl WorkflowProfile for CreationProfile {
    type Snapshot = CreationSnapshot;
    type Event = CreationEvent;
    type Intent = CreationEffectIntent;
    type Observation = CreationObservation;
    type Receipt = CreationReceipt;
    type ReceiptReducerEvent = CreationReceipt;
    type BarrierEvent = CreationBarrierEvent;
    type OwedAcceptanceEvent = CreationReceipt;
    type ManualPayload = CreationManualPayload;

    fn runtime_start_allowed(snapshot: &Self::Snapshot) -> bool {
        matches!(
            snapshot.authoritative_status,
            AuthoritativeCreationStatus::Ready
        )
    }

    fn receipt_requires_runtime_acceptance(_event: &Self::ReceiptReducerEvent) -> bool {
        false
    }

    fn decision_handles_inbox(
        _event: &ReducerInboxPayload<Self>,
        _decision_event: &Self::Event,
    ) -> bool {
        false
    }

    fn owed_acceptance_matches_inbox(
        event: &Self::OwedAcceptanceEvent,
        inbox_payload: &ReducerInboxPayload<Self>,
    ) -> bool {
        matches!(inbox_payload, ReducerInboxPayload::Receipt(receipt) if receipt == event)
    }

    fn decision_handles_owed_acceptance_suppression(
        _event: &Self::OwedAcceptanceEvent,
        _decision_event: &Self::Event,
    ) -> bool {
        false
    }

    fn decision_handles_owed_acceptance(
        _event: &Self::OwedAcceptanceEvent,
        _decision_event: &Self::Event,
    ) -> bool {
        false
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreationShadowAdapter {
    pub workflow: WorkflowState<CreationProfile>,
    pub plan: TransitionPlan<CreationProfile>,
    pub projection: CreationProjection,
}

#[must_use]
pub fn profile() -> ProfileRef {
    ProfileRef {
        profile_id: PROFILE_ID,
        protocol_version: PROTOCOL_VERSION,
    }
}

#[must_use]
pub fn shadow_protocol() -> ProtocolSelection {
    ProtocolSelection {
        profile: profile(),
        authority: SemanticAuthority::LegacyProtocol,
        accepting: true,
        runtime_acceptance_enabled: false,
        external_acceptance_enabled: false,
        selector: "conversation-creation-shadow-v1",
    }
}

#[must_use]
pub fn snapshot_codec() -> CodecRef {
    CodecRef {
        family: "creation.snapshot",
        version: PROTOCOL_VERSION,
    }
}

fn event_codec() -> CodecRef {
    CodecRef {
        family: "creation.event",
        version: PROTOCOL_VERSION,
    }
}

fn intent_codec() -> CodecRef {
    CodecRef {
        family: "creation.intent",
        version: PROTOCOL_VERSION,
    }
}

fn barrier_codec() -> CodecRef {
    CodecRef {
        family: "creation.barrier",
        version: PROTOCOL_VERSION,
    }
}

fn snapshot(oracle: &AuthoritativeCreationOracle) -> CreationSnapshot {
    CreationSnapshot {
        intent: oracle.intent.clone(),
        authoritative_status: oracle.status.clone(),
        authoritative_stage: oracle.stage,
        attempt: oracle.attempt,
        generation: oracle.generation,
    }
}

/// Builds a diagnostic workflow and plan from the legacy creation oracle.
///
/// The returned workflow is structurally shadow-bound. It cannot claim or execute effects, and
/// the protocol disables both runtime and external acceptance.
///
/// # Errors
/// Returns an engine validation error if the fixed shadow profile cannot be instantiated.
pub fn adapt_authoritative_creation(
    workflow_id: WorkflowId,
    authoritative_job_workflow_id: WorkflowId,
    oracle: &AuthoritativeCreationOracle,
) -> Result<CreationShadowAdapter, crate::EngineError> {
    let protocol = shadow_protocol();
    let snapshot = snapshot(oracle);
    let workflow = WorkflowState::new_shadow(
        workflow_id,
        authoritative_job_workflow_id,
        &profile(),
        &protocol,
        snapshot_codec(),
        snapshot.clone(),
    )?;
    let plan = if uses_compensation_plan(oracle) {
        compensation_plan(oracle)
    } else {
        creation_plan(oracle, snapshot)
    };
    let mut projection = project_authoritative_creation(oracle);
    projection.readiness_effects = plan
        .barrier_members
        .iter()
        .map(|member| member.effect_id)
        .collect();
    Ok(CreationShadowAdapter {
        workflow,
        plan,
        projection,
    })
}

fn uses_compensation_plan(oracle: &AuthoritativeCreationOracle) -> bool {
    matches!(
        oracle.status,
        AuthoritativeCreationStatus::Cancelling | AuthoritativeCreationStatus::DeletionPending
    ) || matches!(
        (&oracle.status, oracle.cleanup_ownership),
        (
            AuthoritativeCreationStatus::Failed(_),
            CleanupOwnership::OwnedResources
        )
    )
}

fn creation_dependencies(oracle: &AuthoritativeCreationOracle) -> Vec<DependencyDecl> {
    let mut dependencies = if oracle.intent.uses_worktree {
        vec![
            dependency(RESERVE_WORKTREE, RESOLVE_REPOSITORY),
            dependency(MATERIALIZE_OR_RECONCILE_WORKTREE, RESERVE_WORKTREE),
            dependency(FINALIZE_ATTACHMENTS, RESOLVE_REPOSITORY),
            dependency(COMMIT_METADATA, MATERIALIZE_OR_RECONCILE_WORKTREE),
            dependency(COMMIT_METADATA, FINALIZE_ATTACHMENTS),
        ]
    } else {
        vec![
            dependency(FINALIZE_ATTACHMENTS, RESOLVE_REPOSITORY),
            dependency(COMMIT_METADATA, FINALIZE_ATTACHMENTS),
        ]
    };
    if matches!(oracle.intent.kind, CreationKind::InitialTurn { .. }) {
        dependencies.retain(|dependency| dependency.effect_id != COMMIT_METADATA);
        dependencies.push(dependency(EXPAND_INITIAL_MESSAGE, FINALIZE_ATTACHMENTS));
        if oracle.intent.uses_worktree {
            dependencies.push(dependency(
                EXPAND_INITIAL_MESSAGE,
                MATERIALIZE_OR_RECONCILE_WORKTREE,
            ));
        }
        dependencies.extend([
            dependency(COMMIT_METADATA, EXPAND_INITIAL_MESSAGE),
            dependency(BOOTSTRAP_RUNTIME, COMMIT_METADATA),
            dependency(DISPATCH_INITIAL_LLM_REQUEST, BOOTSTRAP_RUNTIME),
        ]);
    }
    dependencies
}

#[must_use]
pub fn creation_plan(
    oracle: &AuthoritativeCreationOracle,
    snapshot: CreationSnapshot,
) -> TransitionPlan<CreationProfile> {
    let initial_turn = match &oracle.intent.kind {
        CreationKind::InitialTurn { message_id } => Some(message_id.clone()),
        CreationKind::SeededEmpty => None,
    };
    let mut effects = base_effects(&oracle.intent, oracle.generation);
    if !oracle.intent.uses_worktree {
        effects.retain(|effect| {
            !matches!(
                effect.effect_id,
                RESERVE_WORKTREE | MATERIALIZE_OR_RECONCILE_WORKTREE
            )
        });
    }
    effects.push(effect(
        COMMIT_METADATA,
        "commit_metadata",
        CreationEffectIntent::CommitMetadata {
            conversation_id: oracle.intent.conversation_id.clone(),
            worktree_path: oracle.intent.worktree_path.clone(),
        },
        oracle.generation,
        EffectAmbiguity::ExternalIdempotency,
        None,
    ));
    if let Some(message_id) = initial_turn {
        effects.push(effect(
            EXPAND_INITIAL_MESSAGE,
            "expand_initial_message",
            CreationEffectIntent::ExpandInitialMessage {
                conversation_id: oracle.intent.conversation_id.clone(),
                message_id: message_id.clone(),
                text: oracle.intent.initial_text.clone(),
            },
            oracle.generation,
            EffectAmbiguity::ExternalIdempotency,
            None,
        ));
        effects.push(effect(
            BOOTSTRAP_RUNTIME,
            "bootstrap_runtime",
            CreationEffectIntent::BootstrapRuntime {
                conversation_id: oracle.intent.conversation_id.clone(),
            },
            oracle.generation,
            EffectAmbiguity::ExternalIdempotency,
            None,
        ));
        effects.push(effect(
            DISPATCH_INITIAL_LLM_REQUEST,
            "dispatch_initial_llm_request",
            CreationEffectIntent::DispatchInitialLlmRequest {
                conversation_id: oracle.intent.conversation_id.clone(),
                message_id,
            },
            oracle.generation,
            EffectAmbiguity::ExternalIdempotency,
            None,
        ));
    }

    let dependencies = creation_dependencies(oracle);
    let barrier_members = effects
        .iter()
        .map(|effect| BarrierMemberDecl {
            barrier_id: COMPLETION_BARRIER_ID,
            effect_id: effect.effect_id,
            receipt_family: ReceiptFamily::CurrentGenerationEffect,
        })
        .collect();
    TransitionPlan {
        next_status: workflow_status(oracle),
        snapshot,
        snapshot_codec: snapshot_codec(),
        event: CreationEvent::ShadowPlanProjected {
            job_id: oracle.intent.job_id.clone(),
        },
        event_codec: event_codec(),
        effects,
        dependencies,
        barriers: vec![BarrierDecl {
            barrier_id: COMPLETION_BARRIER_ID,
            reducer_event_codec: barrier_codec(),
        }],
        barrier_members,
        invalidations: vec![],
        owed_acceptances: None,
    }
}

fn base_effects(intent: &CreationIntent, generation: u64) -> Vec<EffectDecl<CreationEffectIntent>> {
    vec![
        effect(
            RESOLVE_REPOSITORY,
            "resolve_repository",
            CreationEffectIntent::ResolveRepository {
                repository_path: intent.repository_path.clone(),
            },
            generation,
            EffectAmbiguity::ObservableReconciliation,
            None,
        ),
        effect(
            RESERVE_WORKTREE,
            "reserve_worktree",
            CreationEffectIntent::ReserveWorktree {
                repository_path: intent.repository_path.clone(),
                worktree_path: intent.worktree_path.clone(),
                branch_name: intent.branch_name.clone(),
            },
            generation,
            EffectAmbiguity::ObservableReconciliation,
            Some("repository"),
        ),
        effect(
            MATERIALIZE_OR_RECONCILE_WORKTREE,
            "materialize_or_reconcile_worktree",
            CreationEffectIntent::MaterializeOrReconcileWorktree {
                repository_path: intent.repository_path.clone(),
                worktree_path: intent.worktree_path.clone(),
                branch_name: intent.branch_name.clone(),
            },
            generation,
            EffectAmbiguity::ObservableReconciliation,
            Some("repository"),
        ),
        effect(
            FINALIZE_ATTACHMENTS,
            "finalize_attachments",
            CreationEffectIntent::FinalizeAttachments {
                conversation_id: intent.conversation_id.clone(),
                attachment_ids: intent.attachment_ids.clone(),
            },
            generation,
            EffectAmbiguity::ObservableReconciliation,
            None,
        ),
    ]
}

fn effect(
    effect_id: EffectId,
    kind: &'static str,
    intent: CreationEffectIntent,
    generation: u64,
    ambiguity: EffectAmbiguity,
    destructive_resource: Option<&'static str>,
) -> EffectDecl<CreationEffectIntent> {
    EffectDecl {
        effect_id,
        family: PROFILE_ID,
        kind,
        codec: intent_codec(),
        generation: Generation(generation),
        role: EffectRole::Required,
        ambiguity,
        intent,
        next_eligible_at: None,
        destructive_resource,
    }
}

const fn dependency(effect_id: EffectId, depends_on_effect_id: EffectId) -> DependencyDecl {
    DependencyDecl {
        effect_id,
        depends_on_effect_id,
    }
}

#[must_use]
fn compensation_dependencies(owns_resources: bool) -> Vec<DependencyDecl> {
    if owns_resources {
        vec![
            dependency(DELETE_STAGED_ATTACHMENTS, REVOKE_RUNTIME),
            dependency(REMOVE_OWNED_WORKTREE, DELETE_STAGED_ATTACHMENTS),
            dependency(RELEASE_RESERVATION, REMOVE_OWNED_WORKTREE),
            dependency(FINISH_CANCELLATION_OR_DELETION, RELEASE_RESERVATION),
            dependency(FINISH_CANCELLATION_OR_DELETION, DELETE_STAGED_ATTACHMENTS),
        ]
    } else {
        vec![
            dependency(DELETE_STAGED_ATTACHMENTS, REVOKE_RUNTIME),
            dependency(FINISH_CANCELLATION_OR_DELETION, DELETE_STAGED_ATTACHMENTS),
        ]
    }
}

#[must_use]
pub fn compensation_plan(oracle: &AuthoritativeCreationOracle) -> TransitionPlan<CreationProfile> {
    let generation = oracle.generation;
    let intents = [
        (
            REVOKE_RUNTIME,
            "revoke_runtime",
            CreationEffectIntent::RevokeRuntime {
                conversation_id: oracle.intent.conversation_id.clone(),
            },
            None,
        ),
        (
            REMOVE_OWNED_WORKTREE,
            "remove_owned_worktree",
            CreationEffectIntent::RemoveOwnedWorktree {
                repository_path: oracle.intent.repository_path.clone(),
                worktree_path: oracle.intent.worktree_path.clone(),
            },
            Some("repository"),
        ),
        (
            RELEASE_RESERVATION,
            "release_reservation",
            CreationEffectIntent::ReleaseReservation {
                conversation_id: oracle.intent.conversation_id.clone(),
                worktree_path: oracle.intent.worktree_path.clone(),
            },
            Some("repository"),
        ),
        (
            DELETE_STAGED_ATTACHMENTS,
            "delete_staged_attachments",
            CreationEffectIntent::DeleteStagedAttachments {
                conversation_id: oracle.intent.conversation_id.clone(),
                attachment_ids: oracle.intent.attachment_ids.clone(),
            },
            None,
        ),
        (
            FINISH_CANCELLATION_OR_DELETION,
            "finish_cancellation_or_deletion",
            CreationEffectIntent::FinishCancellationOrDeletion {
                conversation_id: oracle.intent.conversation_id.clone(),
            },
            None,
        ),
    ];
    let owns_resources = oracle.cleanup_ownership == CleanupOwnership::OwnedResources;
    let effects = intents
        .into_iter()
        .filter(|(id, ..)| {
            owns_resources || !matches!(*id, REMOVE_OWNED_WORKTREE | RELEASE_RESERVATION)
        })
        .map(|(id, kind, intent, resource)| {
            let mut declaration = effect(
                id,
                kind,
                intent,
                generation,
                EffectAmbiguity::ObservableReconciliation,
                resource,
            );
            declaration.role = EffectRole::Compensation;
            declaration
        })
        .collect::<Vec<_>>();
    let barrier_members = effects
        .iter()
        .map(|effect| BarrierMemberDecl {
            barrier_id: COMPENSATION_BARRIER_ID,
            effect_id: effect.effect_id,
            receipt_family: ReceiptFamily::CompensationEffect,
        })
        .collect();
    TransitionPlan {
        next_status: workflow_status(oracle),
        snapshot: snapshot(oracle),
        snapshot_codec: snapshot_codec(),
        event: CreationEvent::CancellationOrDeletionProjected {
            job_id: oracle.intent.job_id.clone(),
        },
        event_codec: event_codec(),
        effects,
        dependencies: compensation_dependencies(owns_resources),
        barriers: vec![BarrierDecl {
            barrier_id: COMPENSATION_BARRIER_ID,
            reducer_event_codec: barrier_codec(),
        }],
        barrier_members,
        invalidations: vec![],
        owed_acceptances: None,
    }
}

#[must_use]
pub fn project_authoritative_creation(oracle: &AuthoritativeCreationOracle) -> CreationProjection {
    let (status, capabilities, completion, compensation, hidden) = match &oracle.status {
        AuthoritativeCreationStatus::Accepted
        | AuthoritativeCreationStatus::Claimed { .. }
        | AuthoritativeCreationStatus::RetryScheduled { .. } => (
            CreationProjectionStatus::Provisioning,
            capabilities([true, false, false, true, false, true]),
            CompletionPrediction::Pending,
            CompensationPrediction::None,
            false,
        ),
        AuthoritativeCreationStatus::Cancelling => (
            CreationProjectionStatus::Cancelled,
            capabilities([true, false, false, false, true, true]),
            CompletionPrediction::Cancelled,
            CompensationPrediction::RequiredForCancellation,
            false,
        ),
        AuthoritativeCreationStatus::Failed(_) => (
            CreationProjectionStatus::Failed,
            capabilities([true, false, false, false, true, true]),
            CompletionPrediction::Failed,
            if oracle.cleanup_ownership == CleanupOwnership::OwnedResources {
                CompensationPrediction::RequiredForCancellation
            } else {
                CompensationPrediction::None
            },
            false,
        ),
        AuthoritativeCreationStatus::Cancelled => (
            CreationProjectionStatus::Cancelled,
            capabilities([true, false, false, false, true, true]),
            CompletionPrediction::Cancelled,
            CompensationPrediction::None,
            false,
        ),
        AuthoritativeCreationStatus::DeletionPending => (
            CreationProjectionStatus::DeletionPending,
            capabilities([false, false, false, false, false, false]),
            CompletionPrediction::DeletionPending,
            CompensationPrediction::RequiredForDeletion,
            true,
        ),
        AuthoritativeCreationStatus::Ready => (
            CreationProjectionStatus::Ready,
            oracle
                .runtime_evidence
                .ready_capabilities
                .unwrap_or_else(|| capabilities([true, true, true, false, false, true])),
            completion_for_ready(oracle),
            CompensationPrediction::None,
            false,
        ),
    };
    let readiness_effects = if uses_compensation_plan(oracle) {
        vec![
            REVOKE_RUNTIME,
            REMOVE_OWNED_WORKTREE,
            RELEASE_RESERVATION,
            DELETE_STAGED_ATTACHMENTS,
            FINISH_CANCELLATION_OR_DELETION,
        ]
    } else {
        match &oracle.intent.kind {
            CreationKind::InitialTurn { .. } => vec![
                RESOLVE_REPOSITORY,
                RESERVE_WORKTREE,
                MATERIALIZE_OR_RECONCILE_WORKTREE,
                FINALIZE_ATTACHMENTS,
                COMMIT_METADATA,
                EXPAND_INITIAL_MESSAGE,
                BOOTSTRAP_RUNTIME,
                DISPATCH_INITIAL_LLM_REQUEST,
            ],
            CreationKind::SeededEmpty => vec![
                RESOLVE_REPOSITORY,
                RESERVE_WORKTREE,
                MATERIALIZE_OR_RECONCILE_WORKTREE,
                FINALIZE_ATTACHMENTS,
                COMMIT_METADATA,
            ],
        }
    };
    CreationProjection {
        conversation_id: oracle.intent.conversation_id.clone(),
        kind: oracle.intent.kind.clone(),
        readiness_effects,
        status,
        capabilities,
        effect_predictions: if uses_compensation_plan(oracle) {
            compensation_effect_predictions(oracle)
        } else {
            effect_predictions(oracle)
        },
        completion,
        compensation,
        hidden,
    }
}

fn completion_for_ready(oracle: &AuthoritativeCreationOracle) -> CompletionPrediction {
    match oracle.intent.kind {
        CreationKind::SeededEmpty => CompletionPrediction::Complete,
        CreationKind::InitialTurn { .. }
            if oracle.runtime_evidence.runtime_bootstrapped
                && oracle.runtime_evidence.initial_llm_dispatched =>
        {
            CompletionPrediction::Complete
        }
        CreationKind::InitialTurn { .. } => CompletionPrediction::Pending,
    }
}

const fn availability(allowed: bool) -> CapabilityAvailability {
    if allowed {
        CapabilityAvailability::Allowed
    } else {
        CapabilityAvailability::Forbidden
    }
}

const fn capabilities(flags: [bool; 6]) -> CreationCapabilities {
    CreationCapabilities {
        read: availability(flags[0]),
        write: availability(flags[1]),
        runtime: availability(flags[2]),
        cancel: availability(flags[3]),
        start_over: availability(flags[4]),
        delete: availability(flags[5]),
    }
}

fn compensation_effect_predictions(
    oracle: &AuthoritativeCreationOracle,
) -> Vec<(EffectId, EffectPrediction)> {
    let completed = matches!(oracle.status, AuthoritativeCreationStatus::Cancelled);
    let owns_resources = oracle.cleanup_ownership == CleanupOwnership::OwnedResources;
    [
        (REVOKE_RUNTIME, EffectPrediction::Eligible),
        (DELETE_STAGED_ATTACHMENTS, EffectPrediction::Blocked),
        (REMOVE_OWNED_WORKTREE, EffectPrediction::Blocked),
        (RELEASE_RESERVATION, EffectPrediction::Blocked),
        (FINISH_CANCELLATION_OR_DELETION, EffectPrediction::Blocked),
    ]
    .into_iter()
    .filter(|(effect, _)| {
        owns_resources || !matches!(*effect, REMOVE_OWNED_WORKTREE | RELEASE_RESERVATION)
    })
    .map(|(effect, prediction)| {
        (
            effect,
            if completed {
                EffectPrediction::Completed
            } else {
                prediction
            },
        )
    })
    .collect()
}

fn effect_predictions(oracle: &AuthoritativeCreationOracle) -> Vec<(EffectId, EffectPrediction)> {
    let mut effects = vec![
        (
            RESOLVE_REPOSITORY,
            AuthoritativeCreationStage::ResolveRepository,
        ),
        (
            RESERVE_WORKTREE,
            AuthoritativeCreationStage::ReserveResources,
        ),
        (
            MATERIALIZE_OR_RECONCILE_WORKTREE,
            AuthoritativeCreationStage::MaterializeWorktree,
        ),
        (
            FINALIZE_ATTACHMENTS,
            AuthoritativeCreationStage::FinalizeAttachments,
        ),
        (COMMIT_METADATA, AuthoritativeCreationStage::CommitMetadata),
    ];
    if !oracle.intent.uses_worktree {
        effects
            .retain(|(id, _)| !matches!(*id, RESERVE_WORKTREE | MATERIALIZE_OR_RECONCILE_WORKTREE));
    }
    match oracle.intent.kind {
        CreationKind::InitialTurn { .. } => effects.extend([
            (
                EXPAND_INITIAL_MESSAGE,
                AuthoritativeCreationStage::ExpandInitialMessage,
            ),
            (
                BOOTSTRAP_RUNTIME,
                AuthoritativeCreationStage::BootstrapInitialTurn,
            ),
            (
                DISPATCH_INITIAL_LLM_REQUEST,
                AuthoritativeCreationStage::BootstrapInitialTurn,
            ),
        ]),
        CreationKind::SeededEmpty => effects.extend([
            (EXPAND_INITIAL_MESSAGE, AuthoritativeCreationStage::Finalize),
            (BOOTSTRAP_RUNTIME, AuthoritativeCreationStage::Finalize),
            (
                DISPATCH_INITIAL_LLM_REQUEST,
                AuthoritativeCreationStage::Finalize,
            ),
        ]),
    }
    effects
        .into_iter()
        .map(|(id, stage)| {
            let prediction = if matches!(oracle.intent.kind, CreationKind::SeededEmpty)
                && matches!(
                    id,
                    EXPAND_INITIAL_MESSAGE | BOOTSTRAP_RUNTIME | DISPATCH_INITIAL_LLM_REQUEST
                ) {
                EffectPrediction::Omitted
            } else if (id == RESERVE_WORKTREE && oracle.cleanup_ownership != CleanupOwnership::None)
                || (id == COMMIT_METADATA
                    && oracle.stage == AuthoritativeCreationStage::CommitMetadata)
            {
                EffectPrediction::Completed
            } else if id == BOOTSTRAP_RUNTIME {
                if oracle.runtime_evidence.runtime_bootstrapped {
                    EffectPrediction::Completed
                } else if oracle.stage >= AuthoritativeCreationStage::BootstrapInitialTurn {
                    EffectPrediction::Eligible
                } else {
                    EffectPrediction::Blocked
                }
            } else if id == DISPATCH_INITIAL_LLM_REQUEST {
                if oracle.runtime_evidence.initial_llm_dispatched {
                    EffectPrediction::Completed
                } else if oracle.runtime_evidence.runtime_bootstrapped {
                    EffectPrediction::Eligible
                } else {
                    EffectPrediction::Blocked
                }
            } else if oracle.stage > stage
                || matches!(oracle.status, AuthoritativeCreationStatus::Ready)
            {
                EffectPrediction::Completed
            } else if oracle.stage == stage {
                EffectPrediction::Eligible
            } else {
                EffectPrediction::Blocked
            };
            (id, prediction)
        })
        .collect()
}

fn workflow_status(oracle: &AuthoritativeCreationOracle) -> WorkflowStatus {
    match &oracle.status {
        AuthoritativeCreationStatus::Ready
            if completion_for_ready(oracle) == CompletionPrediction::Complete =>
        {
            WorkflowStatus::Completed
        }
        AuthoritativeCreationStatus::Accepted
        | AuthoritativeCreationStatus::Claimed { .. }
        | AuthoritativeCreationStatus::RetryScheduled { .. }
        | AuthoritativeCreationStatus::Ready => WorkflowStatus::Active,
        AuthoritativeCreationStatus::Cancelling => WorkflowStatus::Cancelling,
        AuthoritativeCreationStatus::Cancelled => WorkflowStatus::Cancelled,
        AuthoritativeCreationStatus::DeletionPending => WorkflowStatus::DeletionPending,
        AuthoritativeCreationStatus::Failed(_) => WorkflowStatus::Failed,
    }
}

#[cfg(test)]
#[path = "creation_profile/tests.rs"]
mod tests;
