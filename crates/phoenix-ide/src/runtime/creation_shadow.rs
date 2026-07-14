//! Best-effort production bridge from committed creation jobs to shadow diagnostics.

use crate::db::{
    ConvMode, ConvState, ConversationCreationJob, CreationResourceReservation, Database,
    FileAttachment,
};
use phoenix_core::domain::creation_protocol::{
    CreationKind as CoreCreationKind, CreationStage, CreationStatus,
};
use phoenix_db::workflow::{
    creation_shadow::{
        CreationShadowAdapter, CreationShadowConfig, CreationShadowEvidence,
        CreationShadowPersistence,
    },
    WorkflowRepository,
};
use phoenix_workflow::creation_profile::{
    AuthoritativeCreationOracle, AuthoritativeCreationStage, AuthoritativeCreationStatus,
    CapabilityAvailability, CleanupOwnership, CreationCapabilities, CreationFailure,
    CreationIntent, CreationProjectionStatus, CreationRuntimeEvidence, CreationStart,
    CreationWorkspace,
};

const ENABLE_ENV: &str = "PHOENIX_CREATION_SHADOW_ENABLED";

#[derive(Default)]
struct JobSyncGate {
    lock: tokio::sync::Mutex<()>,
    users: std::sync::atomic::AtomicUsize,
}

type JobSyncGates = std::sync::Arc<
    tokio::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<JobSyncGate>>>,
>;

#[derive(Clone)]
pub(crate) struct CreationShadowCoordinator {
    db: Database,
    enabled: bool,
    job_gates: JobSyncGates,
}

impl CreationShadowCoordinator {
    pub(crate) fn from_env(db: Database) -> Self {
        Self {
            db,
            enabled: std::env::var(ENABLE_ENV).ok().as_deref() == Some("1"),
            job_gates: std::sync::Arc::default(),
        }
    }

    #[cfg(test)]
    fn with_enabled(db: Database, enabled: bool) -> Self {
        Self {
            db,
            enabled,
            job_gates: std::sync::Arc::default(),
        }
    }

    /// Returns before any shadow read or write. The spawned task owns all diagnostic work.
    pub(crate) fn schedule(&self, job_id: String) {
        if !self.enabled {
            return;
        }
        let coordinator = self.clone();
        tokio::spawn(async move {
            if let Err(error) = coordinator.sync_committed_job(&job_id).await {
                tracing::warn!(job_id, error = %error, "creation shadow sync failed; authoritative state is unchanged");
            }
        });
    }

    pub(crate) async fn sync(&self, job_id: &str) -> Result<(), String> {
        self.sync_committed_job(job_id).await
    }

    async fn sync_committed_job(&self, job_id: &str) -> Result<(), String> {
        if !self.enabled {
            return Ok(());
        }
        let gate = {
            let mut gates = self.job_gates.lock().await;
            let gate = gates
                .entry(job_id.to_owned())
                .or_insert_with(|| std::sync::Arc::new(JobSyncGate::default()))
                .clone();
            gate.users
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            gate
        };
        let guard = gate.lock.lock().await;
        let result = self.sync_committed_job_while_gated(job_id).await;
        drop(guard);
        let mut gates = self.job_gates.lock().await;
        let remaining = gate
            .users
            .fetch_sub(1, std::sync::atomic::Ordering::Relaxed)
            - 1;
        if remaining == 0
            && gates
                .get(job_id)
                .is_some_and(|registered| std::sync::Arc::ptr_eq(registered, &gate))
        {
            gates.remove(job_id);
        }
        result
    }

    async fn sync_committed_job_while_gated(&self, job_id: &str) -> Result<(), String> {
        let job = self
            .db
            .get_conversation_creation_job(job_id)
            .await
            .map_err(|error| error.to_string())?;
        let files = self
            .db
            .get_conversation_creation_job_files(job_id)
            .await
            .map_err(|error| error.to_string())?;
        // Image bytes stay in their authoritative normalized table. Only stable ordinals enter the
        // in-memory oracle, and the persistence adapter writes no semantic payload bytes.
        let image_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM conversation_creation_job_images WHERE job_id = ?1",
        )
        .bind(job_id)
        .fetch_one(self.db.pool())
        .await
        .map_err(|error| error.to_string())?;
        let reservations = self
            .db
            .get_creation_resource_reservations(job_id)
            .await
            .map_err(|error| error.to_string())?;
        let preserved_evidence: Option<(String, i64, Option<i64>, Option<String>)> =
            sqlx::query_as(
                "SELECT cwd, attachment_count, uses_worktree, branch_name
             FROM creation_shadow_creation_evidence WHERE creation_job_id = ?1",
            )
            .bind(job_id)
            .fetch_optional(self.db.pool())
            .await
            .map_err(|error| error.to_string())?;
        let conversation = self
            .db
            .get_conversation(&job.conversation_id)
            .await
            .map_err(|error| error.to_string())?;
        let conv_mode = &conversation.conv_mode;
        let mut oracle = oracle_from_committed(
            job,
            &files,
            usize::try_from(image_count).map_err(|_| "negative image count".to_string())?,
            &reservations,
            Some(conv_mode),
            preserved_evidence.as_ref(),
            &conversation.state,
        );
        let persistence = CreationShadowPersistence::Enabled(CreationShadowConfig {
            shadow_workflow_id: format!("creation-shadow:{}", oracle.intent.job_id),
            authoritative_anchor_workflow_id: format!(
                "creation-authoritative:{}",
                oracle.intent.job_id
            ),
        });
        let visible = self
            .db
            .get_conversation(&oracle.intent.conversation_id)
            .await
            .map_err(|error| error.to_string())?;
        oracle.runtime_evidence = runtime_evidence_from_projection(
            &oracle.status,
            &visible.state,
            capabilities_for_state(&visible.state),
        );
        let observed = observed_projection(&visible.state, visible.archived, &oracle.status);
        CreationShadowAdapter::new(
            &WorkflowRepository::new(self.db.pool().clone()),
            &persistence,
        )
        .persist_after_authoritative_commit(&oracle, observed, chrono::Utc::now())
        .await
        .map_err(|error| error.to_string())?;
        tracing::debug!(job_id, "creation shadow synchronized");
        Ok(())
    }
}

fn active_runtime_evidence(state: &ConvState) -> bool {
    matches!(
        state,
        ConvState::LlmRequesting { .. }
            | ConvState::SeededLlmRequesting { .. }
            | ConvState::ToolExecuting { .. }
            | ConvState::AwaitingSubAgents { .. }
            | ConvState::AwaitingContinuation { .. }
            | ConvState::AwaitingRecovery { .. }
            | ConvState::CancellingTool { .. }
            | ConvState::CancellingSubAgents { .. }
    )
}

fn oracle_from_committed(
    job: ConversationCreationJob,
    files: &[FileAttachment],
    image_count: usize,
    reservations: &[CreationResourceReservation],
    conv_mode: Option<&ConvMode>,
    preserved_evidence: Option<&(String, i64, Option<i64>, Option<String>)>,
    conversation_state: &ConvState,
) -> AuthoritativeCreationOracle {
    let live_attachment_count = files.len().saturating_add(image_count);
    let attachment_count = preserved_evidence
        .and_then(|(_, count, _, _)| usize::try_from(*count).ok())
        .unwrap_or(live_attachment_count);
    let attachment_ids = (0..attachment_count)
        .map(|ordinal| format!("attachment:{ordinal}"))
        .collect();
    let reservation = reservations
        .iter()
        .find(|reservation| reservation.status != "released")
        .or_else(|| reservations.first());
    let preserved_cwd = preserved_evidence.map(|(cwd, _, _, _)| cwd.clone());
    let preserved_uses_worktree = preserved_evidence
        .and_then(|(_, _, uses_worktree, _)| *uses_worktree)
        .map(|value| value != 0);
    let preserved_branch =
        preserved_evidence.and_then(|(_, _, _, branch_name)| branch_name.clone());
    let repository_path = reservation.map_or_else(
        || {
            preserved_cwd
                .clone()
                .unwrap_or_else(|| job.intent.cwd.clone())
        },
        |reservation| reservation.repository_identity.clone(),
    );
    let worktree_path = reservation
        .map(|reservation| reservation.resource_identity.clone())
        .or_else(|| {
            conv_mode
                .and_then(ConvMode::worktree_path)
                .map(str::to_owned)
        })
        .or_else(|| preserved_cwd.clone())
        .unwrap_or_else(|| job.intent.cwd.clone());
    let branch_name = conv_mode
        .and_then(ConvMode::branch_name)
        .map(str::to_owned)
        .or_else(|| {
            job.intent.mode.as_deref().and_then(|mode| {
                matches!(mode, "managed" | "auto").then(|| {
                    format!(
                        "task-pending-{}",
                        job.conversation_id.chars().take(8).collect::<String>()
                    )
                })
            })
        })
        .or(preserved_branch)
        .or_else(|| job.intent.base_branch.clone())
        .unwrap_or_default();
    let start = match &job.protocol.kind {
        CoreCreationKind::InitialTurn { message_id } => CreationStart::InitialTurn {
            message_id: message_id.clone(),
            text: job.intent.text.clone(),
        },
        CoreCreationKind::SeededEmpty => CreationStart::SeededEmpty,
    };
    let uses_worktree = preserved_uses_worktree
        .unwrap_or_else(|| uses_worktree(job.intent.mode.as_deref(), reservations, conv_mode));
    let runtime_evidence = runtime_evidence(&job.protocol.status, conversation_state);
    let status = map_status(job.protocol.status);
    let stage = map_stage(job.protocol.stage);
    AuthoritativeCreationOracle {
        intent: CreationIntent {
            job_id: job.id.clone(),
            conversation_id: job.conversation_id,
            idempotency_key: job.id,
            workspace: if uses_worktree {
                CreationWorkspace::Worktree {
                    repository_path,
                    worktree_path,
                    branch_name,
                }
            } else {
                CreationWorkspace::Direct { cwd: worktree_path }
            },
            attachment_ids,
            start,
        },
        status,
        stage,
        attempt: job.protocol.attempt,
        generation: job.protocol.generation,
        revision: job.shadow_projection_revision,
        cleanup_ownership: cleanup_ownership(reservations),
        runtime_evidence,
    }
}

fn cleanup_ownership(reservations: &[CreationResourceReservation]) -> CleanupOwnership {
    if reservations
        .iter()
        .any(|reservation| reservation.status != "released")
    {
        CleanupOwnership::OwnedResources
    } else if reservations.is_empty() {
        CleanupOwnership::None
    } else {
        CleanupOwnership::HistoricalReservation
    }
}

fn runtime_evidence_from_projection(
    status: &AuthoritativeCreationStatus,
    state: &ConvState,
    capabilities: CreationCapabilities,
) -> CreationRuntimeEvidence {
    if matches!(status, AuthoritativeCreationStatus::Ready) {
        CreationRuntimeEvidence::ready(capabilities)
    } else if active_runtime_evidence(state) {
        CreationRuntimeEvidence::initial_request_dispatched()
    } else if runtime_bootstrapped_from_projection(status, state) {
        CreationRuntimeEvidence::runtime_bootstrapped()
    } else {
        CreationRuntimeEvidence::no_runtime_signals()
    }
}

fn runtime_bootstrapped_from_projection(
    status: &AuthoritativeCreationStatus,
    state: &ConvState,
) -> bool {
    matches!(status, AuthoritativeCreationStatus::Ready)
        || matches!(
            state,
            ConvState::LlmRequesting { .. }
                | ConvState::SeededLlmRequesting { .. }
                | ConvState::ToolExecuting { .. }
                | ConvState::AwaitingSubAgents { .. }
                | ConvState::AwaitingContinuation { .. }
                | ConvState::AwaitingRecovery { .. }
                | ConvState::CancellingTool { .. }
                | ConvState::CancellingSubAgents { .. }
                | ConvState::Idle
        )
}

fn uses_worktree(
    requested_mode: Option<&str>,
    reservations: &[CreationResourceReservation],
    conv_mode: Option<&ConvMode>,
) -> bool {
    !reservations.is_empty()
        || conv_mode.is_some_and(|mode| mode.worktree_path().is_some())
        || matches!(requested_mode, Some("managed" | "branch"))
}

fn runtime_evidence(status: &CreationStatus, state: &ConvState) -> CreationRuntimeEvidence {
    if matches!(status, CreationStatus::Ready) {
        CreationRuntimeEvidence::ready(capabilities_for_state(state))
    } else if active_runtime_evidence(state) {
        CreationRuntimeEvidence::initial_request_dispatched()
    } else if matches!(
        state,
        ConvState::LlmRequesting { .. }
            | ConvState::SeededLlmRequesting { .. }
            | ConvState::ToolExecuting { .. }
            | ConvState::AwaitingSubAgents { .. }
            | ConvState::AwaitingContinuation { .. }
            | ConvState::AwaitingRecovery { .. }
            | ConvState::CancellingTool { .. }
            | ConvState::CancellingSubAgents { .. }
    ) {
        CreationRuntimeEvidence::runtime_bootstrapped()
    } else {
        CreationRuntimeEvidence::no_runtime_signals()
    }
}

fn map_status(status: CreationStatus) -> AuthoritativeCreationStatus {
    match status {
        CreationStatus::Accepted => AuthoritativeCreationStatus::Accepted,
        CreationStatus::Claimed(claim) => AuthoritativeCreationStatus::Claimed {
            worker_id: claim.worker_id.0,
        },
        CreationStatus::RetryScheduled {
            next_attempt_at,
            last_error,
        } => AuthoritativeCreationStatus::RetryScheduled {
            next_attempt_at,
            error: CreationFailure {
                kind: last_error.kind,
                message: last_error.message,
            },
        },
        CreationStatus::Cancelling => AuthoritativeCreationStatus::Cancelling,
        CreationStatus::Cancelled => AuthoritativeCreationStatus::Cancelled,
        CreationStatus::DeletionPending => AuthoritativeCreationStatus::DeletionPending,
        CreationStatus::Ready => AuthoritativeCreationStatus::Ready,
        CreationStatus::Failed(error) => AuthoritativeCreationStatus::Failed(CreationFailure {
            kind: error.kind,
            message: error.message,
        }),
    }
}

fn map_stage(stage: CreationStage) -> AuthoritativeCreationStage {
    match stage {
        CreationStage::ValidateIntent => AuthoritativeCreationStage::ValidateIntent,
        CreationStage::ResolveRepository => AuthoritativeCreationStage::ResolveRepository,
        CreationStage::ReserveResources => AuthoritativeCreationStage::ReserveResources,
        CreationStage::MaterializeWorktree => AuthoritativeCreationStage::MaterializeWorktree,
        CreationStage::FinalizeAttachments => AuthoritativeCreationStage::FinalizeAttachments,
        CreationStage::ExpandInitialMessage => AuthoritativeCreationStage::ExpandInitialMessage,
        CreationStage::CommitMetadata => AuthoritativeCreationStage::CommitMetadata,
        CreationStage::BootstrapInitialTurn => AuthoritativeCreationStage::BootstrapInitialTurn,
        CreationStage::Finalize => AuthoritativeCreationStage::Finalize,
    }
}

fn observed_projection(
    state: &ConvState,
    archived: bool,
    creation_status: &AuthoritativeCreationStatus,
) -> CreationShadowEvidence {
    let (status, mut capabilities) = if matches!(
        creation_status,
        AuthoritativeCreationStatus::DeletionPending
    ) {
        (
            CreationProjectionStatus::DeletionPending,
            creation_capabilities([false, false, false, false, false, false]),
        )
    } else {
        let status = match state {
            ConvState::Provisioning { .. } => CreationProjectionStatus::Provisioning,
            ConvState::CreationFailed { .. } => CreationProjectionStatus::Failed,
            ConvState::CreationCancelled { .. } => CreationProjectionStatus::Cancelled,
            _ => CreationProjectionStatus::Ready,
        };
        (status, capabilities_for_state(state))
    };
    if matches!(
        creation_status,
        AuthoritativeCreationStatus::DeletionPending
    ) {
        capabilities.cancel = CapabilityAvailability::Forbidden;
    }
    CreationShadowEvidence::UserProjection {
        status,
        capabilities,
        hidden: archived,
    }
}

fn capabilities_for_state(state: &ConvState) -> CreationCapabilities {
    let mut capabilities = match state {
        ConvState::Provisioning { .. } => {
            creation_capabilities([true, false, false, true, false, true])
        }
        ConvState::CreationFailed { .. } | ConvState::CreationCancelled { .. } => {
            creation_capabilities([true, false, false, false, true, true])
        }
        ConvState::LlmRequesting { .. }
        | ConvState::SeededLlmRequesting { .. }
        | ConvState::ToolExecuting { .. }
        | ConvState::AwaitingSubAgents { .. }
        | ConvState::AwaitingContinuation { .. }
        | ConvState::AwaitingRecovery { .. } => {
            creation_capabilities([true, true, true, true, false, false])
        }
        ConvState::CancellingTool { .. } | ConvState::CancellingSubAgents { .. } => {
            creation_capabilities([true, false, true, false, false, false])
        }
        _ => creation_capabilities([true, true, true, false, false, true]),
    };
    capabilities.cancel =
        if matches!(state, ConvState::Provisioning { .. }) || state.allows_user_cancel() {
            CapabilityAvailability::Allowed
        } else {
            CapabilityAvailability::Forbidden
        };
    capabilities
}

fn creation_capabilities(flags: [bool; 6]) -> CreationCapabilities {
    let available = |allowed| {
        if allowed {
            CapabilityAvailability::Allowed
        } else {
            CapabilityAvailability::Forbidden
        }
    };
    CreationCapabilities {
        read: available(flags[0]),
        write: available(flags[1]),
        runtime: available(flags[2]),
        cancel: available(flags[3]),
        start_over: available(flags[4]),
        delete: available(flags[5]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn database() -> Database {
        Database::open_in_memory().await.unwrap()
    }

    async fn insert_job(db: &Database) {
        sqlx::query("INSERT INTO conversations (id, slug, cwd, user_initiated, state, state_updated_at, created_at, updated_at, archived, cm_kind) VALUES ('conv-shadow-runtime', 'shadow-runtime', '/tmp', 1, '{\"type\":\"provisioning\",\"job_id\":\"job-shadow-runtime\"}', '2025-01-01', '2025-01-01', '2025-01-01', 0, 'direct')")
            .execute(db.pool()).await.unwrap();
        sqlx::query("INSERT INTO conversation_creation_jobs (id, conversation_id, message_id, status, stage, attempt, generation, intent_json, accepted_at, created_at, updated_at) VALUES ('job-shadow-runtime', 'conv-shadow-runtime', NULL, 'accepted', 'validate_intent', 0, 0, '{\"cwd\":\"/tmp\",\"text\":\"secret semantic bytes\"}', '2025-01-01', '2025-01-01', '2025-01-01')")
            .execute(db.pool()).await.unwrap();
    }

    async fn binding_count(db: &Database) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM creation_shadow_bindings")
            .fetch_one(db.pool())
            .await
            .unwrap()
    }

    #[test]
    fn observed_projection_comes_from_visible_conversation_state() {
        assert_eq!(
            observed_projection(
                &ConvState::CreationCancelled {
                    job_id: "job".to_owned()
                },
                false,
                &AuthoritativeCreationStatus::Cancelled,
            ),
            CreationShadowEvidence::UserProjection {
                status: CreationProjectionStatus::Cancelled,
                capabilities: creation_capabilities([true, false, false, false, true, true]),
                hidden: false,
            }
        );
        assert_eq!(
            observed_projection(&ConvState::Idle, false, &AuthoritativeCreationStatus::Ready),
            CreationShadowEvidence::UserProjection {
                status: CreationProjectionStatus::Ready,
                capabilities: creation_capabilities([true, true, true, false, false, true]),
                hidden: false,
            }
        );
        assert_eq!(
            observed_projection(
                &ConvState::LlmRequesting { attempt: 1 },
                false,
                &AuthoritativeCreationStatus::Ready
            ),
            CreationShadowEvidence::UserProjection {
                status: CreationProjectionStatus::Ready,
                capabilities: creation_capabilities([true, true, true, true, false, false]),
                hidden: false,
            }
        );
        assert_eq!(
            observed_projection(
                &ConvState::AwaitingTaskApproval {
                    task_file: "tasks/1.md".to_owned(),
                    title: "task".to_owned(),
                    priority: phoenix_core::task_source::Priority::P1,
                    plan: "approve".to_owned(),
                },
                false,
                &AuthoritativeCreationStatus::Ready,
            ),
            CreationShadowEvidence::UserProjection {
                status: CreationProjectionStatus::Ready,
                capabilities: creation_capabilities([true, true, true, true, false, true]),
                hidden: false,
            }
        );
        assert_eq!(
            observed_projection(&ConvState::Idle, true, &AuthoritativeCreationStatus::Ready),
            CreationShadowEvidence::UserProjection {
                status: CreationProjectionStatus::Ready,
                capabilities: creation_capabilities([true, true, true, false, false, true]),
                hidden: true,
            }
        );
        assert_eq!(
            observed_projection(
                &ConvState::Idle,
                false,
                &AuthoritativeCreationStatus::DeletionPending
            ),
            CreationShadowEvidence::UserProjection {
                status: CreationProjectionStatus::DeletionPending,
                capabilities: creation_capabilities([false, false, false, false, false, false]),
                hidden: false,
            }
        );
    }

    #[test]
    fn settled_ready_turn_preserves_dispatch_evidence() {
        assert!(matches!(
            runtime_evidence(&CreationStatus::Ready, &ConvState::Idle),
            CreationRuntimeEvidence::Ready { .. }
        ));
    }

    #[test]
    fn unresolved_auto_without_resource_evidence_is_not_worktree_backed() {
        assert!(!uses_worktree(Some("auto"), &[], Some(&ConvMode::Direct)));
    }

    #[test]
    fn released_reservation_preserves_history_without_owing_cleanup() {
        let mut reservation = CreationResourceReservation {
            id: "reservation".to_owned(),
            job_id: "job".to_owned(),
            generation: 1,
            resource_identity: "/tmp/worktree".to_owned(),
            repository_identity: "/tmp/repo".to_owned(),
            status: "released".to_owned(),
        };
        assert_eq!(
            cleanup_ownership(std::slice::from_ref(&reservation)),
            CleanupOwnership::HistoricalReservation
        );

        reservation.status = "reserved".to_owned();
        assert_eq!(
            cleanup_ownership(&[reservation]),
            CleanupOwnership::OwnedResources
        );
        assert_eq!(cleanup_ownership(&[]), CleanupOwnership::None);
    }

    #[test]
    fn redacted_intent_preserves_committed_worktree_evidence() {
        let mode = ConvMode::Explore {
            worktree_path: Some(
                phoenix_core::domain::db_schema::NonEmptyString::new("/tmp/worktree".to_owned())
                    .unwrap(),
            ),
            next_taskmd_id_hint: None,
        };
        assert!(uses_worktree(None, &[], Some(&mode)));
    }

    #[test]
    fn terminal_pre_bootstrap_shells_do_not_count_as_runtime_bootstrapped() {
        assert!(matches!(
            runtime_evidence(
                &CreationStatus::Failed(phoenix_core::domain::creation_protocol::CreationError {
                    kind: "failed".to_owned(),
                    message: "failed".to_owned(),
                }),
                &ConvState::CreationFailed {
                    job_id: "job".to_owned(),
                    error: "failed".to_owned(),
                    error_kind: crate::db::ErrorKind::ServerError,
                },
            ),
            CreationRuntimeEvidence::NoRuntimeSignals
        ));
        assert!(matches!(
            runtime_evidence(
                &CreationStatus::Cancelled,
                &ConvState::CreationCancelled {
                    job_id: "job".to_owned(),
                },
            ),
            CreationRuntimeEvidence::NoRuntimeSignals
        ));
        assert!(matches!(
            runtime_evidence(
                &CreationStatus::Claimed(phoenix_core::domain::creation_protocol::CreationClaim {
                    worker_id: phoenix_core::domain::creation_protocol::CreationWorkerId(
                        "worker".to_owned(),
                    ),
                    generation: 1,
                    token: phoenix_core::domain::creation_protocol::CreationClaimToken(
                        "token".to_owned(),
                    ),
                    lease_until: 2,
                }),
                &ConvState::LlmRequesting { attempt: 1 },
            ),
            CreationRuntimeEvidence::InitialRequestDispatched
        ));
        assert!(matches!(
            runtime_evidence(
                &CreationStatus::Failed(phoenix_core::domain::creation_protocol::CreationError {
                    kind: "failed".to_owned(),
                    message: "failed".to_owned(),
                }),
                &ConvState::CreationFailed {
                    job_id: "job".to_owned(),
                    error: "late failure".to_owned(),
                    error_kind: crate::db::ErrorKind::ServerError,
                },
            ),
            CreationRuntimeEvidence::NoRuntimeSignals
        ));
    }

    #[tokio::test]
    async fn disabled_writes_nothing() {
        let db = database().await;
        insert_job(&db).await;
        CreationShadowCoordinator::with_enabled(db.clone(), false)
            .sync_committed_job("job-shadow-runtime")
            .await
            .unwrap();
        assert_eq!(binding_count(&db).await, 0);
    }

    #[tokio::test]
    async fn enabled_eventually_writes_bounded_non_executable_projection() {
        let db = database().await;
        insert_job(&db).await;
        let coordinator = CreationShadowCoordinator::with_enabled(db.clone(), true);
        coordinator.schedule("job-shadow-runtime".to_string());
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while binding_count(&db).await == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        coordinator
            .sync_committed_job("job-shadow-runtime")
            .await
            .unwrap();
        assert_eq!(binding_count(&db).await, 1);
        let executable: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM workflow_effects WHERE status <> 'blocked' AND workflow_id = 'creation-shadow:job-shadow-runtime'")
            .fetch_one(db.pool()).await.unwrap();
        assert_eq!(executable, 0);
        let transitions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM workflow_transitions WHERE workflow_id = 'creation-shadow:job-shadow-runtime'")
            .fetch_one(db.pool()).await.unwrap();
        assert_eq!(transitions, 1);
    }

    #[tokio::test]
    async fn completed_job_gate_is_released() {
        let db = database().await;
        insert_job(&db).await;
        let coordinator = CreationShadowCoordinator::with_enabled(db, true);
        coordinator
            .sync_committed_job("job-shadow-runtime")
            .await
            .unwrap();
        assert!(coordinator.job_gates.lock().await.is_empty());
    }

    #[tokio::test]
    async fn concurrent_waiters_share_gate_and_last_waiter_removes_it() {
        let db = database().await;
        insert_job(&db).await;
        let coordinator = CreationShadowCoordinator::with_enabled(db, true);
        let first = coordinator.sync_committed_job("job-shadow-runtime");
        let second = coordinator.sync_committed_job("job-shadow-runtime");
        let (first, second) = tokio::join!(first, second);
        first.unwrap();
        second.unwrap();
        assert!(coordinator.job_gates.lock().await.is_empty());
    }

    #[tokio::test]
    async fn scheduling_broken_shadow_database_is_non_blocking_and_non_propagating() {
        let db = database().await;
        insert_job(&db).await;
        sqlx::query("DROP TABLE creation_shadow_bindings")
            .execute(db.pool())
            .await
            .unwrap();
        let coordinator = CreationShadowCoordinator::with_enabled(db, true);
        let start = std::time::Instant::now();
        coordinator.schedule("job-shadow-runtime".to_string());
        assert!(start.elapsed() < std::time::Duration::from_millis(50));
    }
}
