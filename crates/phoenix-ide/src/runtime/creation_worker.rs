use crate::api::handlers::{
    create_branch_worktree_blocking, create_managed_explore_worktree_blocking, generate_slug,
    slugify_label, title_from_text, validate_user_ref, AppError, BranchWorktreeError,
    BranchWorktreeInfo, ManagedWorktreeError,
};
use crate::db::{
    ConvMode, ConversationCreationMetadataUpdate, CreationClaimOutcome, ErrorKind, NonEmptyString,
};
use crate::runtime::{ConversationMetadataUpdate, EvictionReason, RuntimeManager, SseEvent};
use crate::state_machine::{ConvState, Event};
use fs2::FileExt;
use phoenix_core::domain::creation_protocol::{
    CreationClaimToken, CreationStatus, CreationWorkerId,
};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub(crate) async fn drain_pending_jobs(manager: &Arc<RuntimeManager>) -> Result<(), String> {
    let worker_id = CreationWorkerId(format!("creation-worker-{}", uuid::Uuid::new_v4()));
    while let Some(cleanup) = manager
        .db()
        .claim_next_conversation_creation_cleanup(
            &worker_id.0,
            &uuid::Uuid::new_v4().to_string(),
            chrono::Utc::now(),
            chrono::Duration::seconds(30),
        )
        .await
        .map_err(|error| error.to_string())?
    {
        if let Err(error) = reconcile_creation_cleanup(manager, &cleanup).await {
            tracing::warn!(job_id = %cleanup.job_id, error = %error, "conversation creation cleanup will retry");
            manager
                .db()
                .schedule_conversation_creation_cleanup_retry(
                    &cleanup,
                    chrono::Utc::now() + chrono::Duration::seconds(30),
                )
                .await
                .map_err(|db_error| db_error.to_string())?;
        }
    }
    loop {
        let token = CreationClaimToken(uuid::Uuid::new_v4().to_string());
        let outcome = manager
            .db()
            .claim_next_conversation_creation_job(
                &worker_id,
                &token,
                chrono::Utc::now(),
                chrono::Duration::seconds(30),
            )
            .await
            .map_err(|e| e.to_string())?;
        let CreationClaimOutcome::Claimed(job) = outcome else {
            return Ok(());
        };
        if let Err(error) = process_claimed_job(manager, *job).await {
            tracing::error!(error = %error, "conversation creation job processing failed");
        }
    }
}

fn missing_repository_and_resource(repo: &Path, resource: &Path) -> bool {
    !repo.exists() && !resource.exists()
}

async fn reconcile_creation_cleanup(
    manager: &Arc<RuntimeManager>,
    cleanup: &crate::db::CreationCleanupJob,
) -> Result<(), String> {
    for reservation in &cleanup.reservations {
        if reservation.status == "released" {
            continue;
        }
        let repo = reservation.repository_identity.clone();
        let resource = reservation.resource_identity.clone();
        let reservation_id = reservation.id.clone();
        let cleanup_for_blocking = cleanup.clone();
        let db = manager.db().clone();
        tokio::task::spawn_blocking(move || -> Result<(), String> {
            let _lock = match RepositoryMutationLock::acquire(&repo) {
                Ok(lock) => Some(lock),
                Err((_message, _))
                    if missing_repository_and_resource(Path::new(&repo), Path::new(&resource)) =>
                {
                    None
                }
                Err((message, _)) => return Err(message),
            };
            let runtime = tokio::runtime::Handle::current();
            let reservations = runtime
                .block_on(db.get_creation_resource_reservations(&cleanup_for_blocking.job_id))
                .map_err(|error| error.to_string())?;
            let owned = reservations.iter().any(|current| {
                current.id == reservation_id
                    && current.generation == cleanup_for_blocking.generation
                    && current.status == "cleanup_required"
            });
            if !owned {
                return Ok(());
            }
            remove_owned_worktree_for_cleanup(&repo, Path::new(&resource))?;
            if let Some(branch) = temporary_creation_branch(&cleanup_for_blocking) {
                delete_temporary_creation_branch(&repo, &branch)?;
            }
            Ok(())
        })
        .await
        .map_err(|error| error.to_string())??;
        let outcome = manager
            .db()
            .release_creation_resource(cleanup, &reservation.id, chrono::Utc::now())
            .await
            .map_err(|error| error.to_string())?;
        if matches!(outcome, crate::db::CreationCasOutcome::ClaimLost) {
            return Ok(());
        }
    }
    if cleanup.status == "deletion_pending" {
        if let Err(error) = manager
            .mirror_creation_before_cleanup(&cleanup.job_id)
            .await
        {
            tracing::warn!(
                job_id = %cleanup.job_id,
                error,
                "creation shadow sync failed after resource cleanup; continuing"
            );
        }
    }
    manager
        .db()
        .finish_conversation_creation_cleanup(cleanup, chrono::Utc::now())
        .await
        .map_err(|error| error.to_string())?;
    if cleanup.status != "deletion_pending" {
        manager.mirror_creation_after_commit(cleanup.job_id.clone());
    }
    Ok(())
}

async fn process_claimed_job(
    manager: &Arc<RuntimeManager>,
    job: crate::db::ConversationCreationJob,
) -> Result<(), String> {
    let CreationStatus::Claimed(claim) = job.protocol.status.clone() else {
        return Err("claimed creation job lacked claim authority".to_string());
    };
    let job_id = job.id.clone();
    let mut processing = std::pin::pin!(process_job(manager, job));
    let mut heartbeat = tokio::time::interval(std::time::Duration::from_secs(10));
    heartbeat.tick().await;
    loop {
        tokio::select! {
            result = &mut processing => return result,
            _ = heartbeat.tick() => {
                let outcome = manager
                    .db()
                    .renew_conversation_creation_claim(
                        &job_id,
                        &claim,
                        chrono::Utc::now(),
                        chrono::Duration::seconds(30),
                    )
                    .await
                    .map_err(|error| error.to_string())?;
                if matches!(outcome, crate::db::CreationCasOutcome::ClaimLost) {
                    let terminal = manager
                        .db()
                        .get_conversation_creation_job(&job_id)
                        .await
                        .map(|job| job.protocol.status.is_terminal())
                        .unwrap_or(false);
                    if terminal {
                        tracing::debug!(job_id = %job_id, generation = claim.generation, "creation heartbeat observed terminal commit");
                        return processing.await;
                    }
                    tracing::debug!(job_id = %job_id, generation = claim.generation, "stopping creation worker after lease authority was lost");
                    return Ok(());
                }
            }
        }
    }
}

#[allow(clippy::too_many_lines)]
async fn process_job(
    manager: &Arc<RuntimeManager>,
    mut job: crate::db::ConversationCreationJob,
) -> Result<(), String> {
    let conv_id = job.conversation_id.clone();
    let CreationStatus::Claimed(claim) = job.protocol.status.clone() else {
        return Err("claimed creation job lacked claim authority".to_string());
    };
    let message_already_exists = if let Some(message_id) = job.message_id.as_deref() {
        manager
            .db()
            .message_exists(message_id)
            .await
            .map_err(|e| e.to_string())?
    } else {
        false
    };
    if message_already_exists {
        let conversation = manager
            .db()
            .get_conversation(&conv_id)
            .await
            .map_err(|e| e.to_string())?;
        if creation_state_allows_existing_message_completion(&conversation.state) {
            let outcome = manager
                .db()
                .complete_conversation_creation_job(&job.id, &claim, chrono::Utc::now())
                .await
                .map_err(|e| e.to_string())?;
            if matches!(outcome, crate::db::CreationCasOutcome::ClaimLost) {
                tracing::debug!(job_id = %job.id, generation = claim.generation, "creation completion rejected after claim loss");
            }
            if matches!(outcome, crate::db::CreationCasOutcome::Applied) {
                manager.mirror_creation_after_commit(job.id.clone());
            }
            return Ok(());
        }
        if matches!(conversation.state, ConvState::LlmRequesting { .. }) {
            tracing::info!(job_id = %job.id, message_id = ?job.message_id, "starting creation runtime to resume the persisted LLM request");
            let _ = manager.get_or_create(&conv_id).await?;
            return Ok(());
        }
        tracing::info!(job_id = %job.id, message_id = ?job.message_id, "replaying creation bootstrap after message persisted without state advancement");
    }

    match provision_conversation(manager, &mut job).await {
        Ok(ProvisionOutcome::SeededEmpty | ProvisionOutcome::InitialMessageSubmitted) => Ok(()),
        Err((message, kind)) => {
            if creation_error_is_retryable(&kind) && job.protocol.attempt < 4 {
                let delay_ms = crate::state_machine::creation_protocol::creation_retry_delay_ms(
                    job.protocol.attempt,
                )
                .ok_or_else(|| "retryable creation lacked configured delay".to_string())?;
                let delay = chrono::Duration::milliseconds(
                    i64::try_from(delay_ms)
                        .map_err(|_| "creation retry delay exceeds chrono range".to_string())?,
                );
                let now = chrono::Utc::now();
                let outcome = manager
                    .db()
                    .schedule_conversation_creation_retry(
                        &job.id,
                        &claim,
                        &message,
                        now,
                        now + delay,
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                if matches!(outcome, crate::db::CreationCasOutcome::Applied) {
                    tracing::warn!(job_id = %job.id, attempt = job.protocol.attempt, retry_at = %(now + delay), error = %message, "conversation creation retry scheduled");
                }
                if matches!(outcome, crate::db::CreationCasOutcome::Applied) {
                    manager.mirror_creation_after_commit(job.id.clone());
                }
                return Ok(());
            }
            let failed = ConvState::CreationFailed {
                job_id: job.id.clone(),
                error: message.clone(),
                error_kind: kind.clone(),
            };
            let outcome = manager
                .db()
                .fail_conversation_creation_job(
                    &job.id,
                    &claim,
                    &message,
                    &kind,
                    chrono::Utc::now(),
                )
                .await
                .map_err(|e| e.to_string())?;
            if matches!(outcome, crate::db::CreationCasOutcome::ClaimLost) {
                tracing::debug!(job_id = %job.id, generation = claim.generation, "creation failure rejected after claim loss");
                return Ok(());
            }
            manager.mirror_creation_after_commit(job.id.clone());
            let broadcast_tx = manager.conversation_broadcaster(&conv_id).await;
            let _ = broadcast_tx.send_seq(|seq| SseEvent::StateChange {
                sequence_id: seq,
                state: failed.clone(),
                presentation_mode: failed.presentation_mode().to_string(),
                state_updated_at: chrono::Utc::now(),
            });
            manager
                .evict_runtime(&conv_id, EvictionReason::CreationProvisioned)
                .await;
            Err(message)
        }
    }
}

fn creation_state_allows_existing_message_completion(state: &ConvState) -> bool {
    !matches!(
        state,
        ConvState::Provisioning { .. } | ConvState::LlmRequesting { .. }
    )
}

enum ProvisionOutcome {
    SeededEmpty,
    InitialMessageSubmitted,
}

#[allow(clippy::too_many_lines)]
async fn provision_conversation(
    manager: &Arc<RuntimeManager>,
    job: &mut crate::db::ConversationCreationJob,
) -> Result<ProvisionOutcome, (String, ErrorKind)> {
    let intent = job.intent.clone();
    let CreationStatus::Claimed(claim) = job.protocol.status.clone() else {
        return Err((
            "creation provisioning lacks claim authority".to_string(),
            ErrorKind::ServerError,
        ));
    };
    let valid_cwd = crate::conversation_cwd::validate_conversation_cwd(&intent.cwd)
        .map_err(|e| (e.to_string(), ErrorKind::InvalidRequest))?;
    checkpoint_creation_stage(
        manager,
        job,
        &claim,
        phoenix_core::domain::creation_protocol::CreationStage::ResolveRepository,
    )
    .await?;
    let initial_cwd = valid_cwd.into_raw();
    let repo_root = phoenix_core::git::detect_git_repo_root(Path::new(&initial_cwd));
    let requested_mode = intent.mode.as_deref().unwrap_or("direct");

    let registry_default = manager.llm_registry.default_model_id();
    let mut resolved_model = intent.model.clone().unwrap_or_else(|| {
        if requested_mode == "managed" {
            manager
                .llm_registry
                .cheap_model_id_for_provider(registry_default)
        } else {
            registry_default.to_string()
        }
    });

    let mut conv_mode = ConvMode::Direct;
    let mut effective_cwd = initial_cwd.clone();
    let mut project_id = None;
    let mut desired_base_branch = intent.base_branch.clone();

    if let Some(repo_root) = repo_root.clone() {
        match manager.db().find_or_create_project(&repo_root).await {
            Ok(project) => project_id = Some(project.id),
            Err(e) => {
                tracing::warn!(
                    conversation_id = %job.conversation_id,
                    repo_root,
                    error = %e,
                    "project association failed during async conversation creation; continuing without project"
                );
            }
        }
    }

    match requested_mode {
        "direct" => {}
        "branch" => {
            let repo_root = repo_root.clone().ok_or_else(|| {
                (
                    "Branch mode requires a git repository".to_string(),
                    ErrorKind::InvalidRequest,
                )
            })?;
            let branch_name = desired_base_branch.clone().ok_or_else(|| {
                (
                    "Branch mode requires base_branch naming the existing branch".to_string(),
                    ErrorKind::InvalidRequest,
                )
            })?;
            validate_user_ref(&branch_name).map_err(app_error_to_kind)?;
            let existing_path = deterministic_worktree_path(&repo_root, &job.conversation_id);
            checkpoint_creation_stage(
                manager,
                job,
                &claim,
                phoenix_core::domain::creation_protocol::CreationStage::ReserveResources,
            )
            .await?;
            reserve_worktree(manager, job, &claim, &repo_root, &existing_path).await?;
            checkpoint_creation_stage(
                manager,
                job,
                &claim,
                phoenix_core::domain::creation_protocol::CreationStage::MaterializeWorktree,
            )
            .await?;
            let db = manager.db().clone();
            let conv_id = job.conversation_id.clone();
            let repo_for_blocking = repo_root.clone();
            let path_for_blocking = existing_path.clone();
            let info = tokio::task::spawn_blocking(move || {
                let _lock = RepositoryMutationLock::acquire(&repo_for_blocking)?;
                if reconcile_owned_worktree_path(&repo_for_blocking, &path_for_blocking)? {
                    validate_worktree_branch(&path_for_blocking, &branch_name)?;
                    let worktree_path = path_for_blocking.to_string_lossy().to_string();
                    let default_branch = crate::git_ops::run_git(
                        Path::new(&repo_for_blocking),
                        &["symbolic-ref", "refs/remotes/origin/HEAD"],
                    )
                    .ok()
                    .and_then(|s| {
                        s.trim()
                            .strip_prefix("refs/remotes/origin/")
                            .map(String::from)
                    })
                    .or_else(|| {
                        crate::git_ops::run_git(
                            Path::new(&repo_for_blocking),
                            &["rev-parse", "--abbrev-ref", "HEAD"],
                        )
                        .ok()
                        .map(|s| s.trim().to_string())
                    })
                    .unwrap_or_else(|| branch_name.clone());
                    Ok(BranchWorktreeInfo {
                        branch_name,
                        worktree_path,
                        base_branch: default_branch,
                    })
                } else {
                    create_branch_worktree_blocking(&repo_for_blocking, &conv_id, &branch_name, &db)
                        .map_err(branch_worktree_error_to_kind)
                }
            })
            .await
            .map_err(|e| {
                (
                    format!("spawn_blocking failed: {e}"),
                    ErrorKind::ServerError,
                )
            })?;
            let info = info?;
            mark_worktree_present(manager, job, &claim, &existing_path).await?;
            checkpoint_creation_stage(
                manager,
                job,
                &claim,
                phoenix_core::domain::creation_protocol::CreationStage::FinalizeAttachments,
            )
            .await?;
            effective_cwd.clone_from(&info.worktree_path);
            desired_base_branch = Some(info.base_branch.clone());
            conv_mode = ConvMode::Branch {
                branch_name: NonEmptyString::new(info.branch_name)
                    .map_err(|_| ("empty branch name".to_string(), ErrorKind::ServerError))?,
                worktree_path: NonEmptyString::new(info.worktree_path)
                    .map_err(|_| ("empty worktree path".to_string(), ErrorKind::ServerError))?,
                base_branch: NonEmptyString::new(info.base_branch)
                    .map_err(|_| ("empty base branch".to_string(), ErrorKind::ServerError))?,
            };
        }
        "auto" if repo_root.is_none() => {}
        "managed" | "auto" => {
            let repo_root = repo_root.clone().ok_or_else(|| {
                (
                    "Could not determine git repository root".to_string(),
                    ErrorKind::InvalidRequest,
                )
            })?;
            let inferred_base = if requested_mode == "auto" && desired_base_branch.is_none() {
                crate::git_ops::run_git(
                    Path::new(&repo_root),
                    &["rev-parse", "--abbrev-ref", "HEAD"],
                )
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty() && s != "HEAD")
            } else {
                None
            };
            let base_branch = desired_base_branch
                .as_deref()
                .or(inferred_base.as_deref())
                .ok_or_else(|| {
                    (
                        "Managed mode requires base_branch (the branch to allocate the Explore worktree against)".to_string(),
                        ErrorKind::InvalidRequest,
                    )
                })?
                .to_string();
            validate_user_ref(&base_branch).map_err(app_error_to_kind)?;
            if let Some(checkout_ref) = intent.checkout_ref.as_deref() {
                validate_user_ref(checkout_ref).map_err(app_error_to_kind)?;
            }
            let existing_path = deterministic_worktree_path(&repo_root, &job.conversation_id);
            checkpoint_creation_stage(
                manager,
                job,
                &claim,
                phoenix_core::domain::creation_protocol::CreationStage::ReserveResources,
            )
            .await?;
            reserve_worktree(manager, job, &claim, &repo_root, &existing_path).await?;
            checkpoint_creation_stage(
                manager,
                job,
                &claim,
                phoenix_core::domain::creation_protocol::CreationStage::MaterializeWorktree,
            )
            .await?;
            let conv_id = job.conversation_id.clone();
            let repo_for_blocking = repo_root.clone();
            let path_for_blocking = existing_path.clone();
            let base_branch_for_blocking = base_branch.clone();
            let checkout_ref = intent.checkout_ref.clone();
            let worktree = tokio::task::spawn_blocking(move || {
                let _lock = RepositoryMutationLock::acquire(&repo_for_blocking)?;
                if reconcile_owned_worktree_path(&repo_for_blocking, &path_for_blocking)? {
                    Ok(path_for_blocking.to_string_lossy().to_string())
                } else {
                    create_managed_explore_worktree_blocking(
                        &repo_for_blocking,
                        &conv_id,
                        &base_branch_for_blocking,
                        checkout_ref.as_deref(),
                    )
                    .map_err(managed_worktree_error_to_kind)
                }
            })
            .await
            .map_err(|e| {
                (
                    format!("spawn_blocking failed: {e}"),
                    ErrorKind::ServerError,
                )
            })?;
            let worktree = worktree?;
            mark_worktree_present(manager, job, &claim, &existing_path).await?;
            checkpoint_creation_stage(
                manager,
                job,
                &claim,
                phoenix_core::domain::creation_protocol::CreationStage::FinalizeAttachments,
            )
            .await?;
            effective_cwd.clone_from(&worktree);
            desired_base_branch = Some(base_branch.clone());
            let tasks_dir_name = taskmd_core::discover::discover_or_default(Path::new(&worktree));
            let next_taskmd_id_hint = crate::system_prompt::snapshot_next_taskmd_id_hint(
                Path::new(&worktree),
                &tasks_dir_name.to_string_lossy(),
            );
            conv_mode = ConvMode::Explore {
                worktree_path: Some(NonEmptyString::new(worktree).map_err(|_| {
                    (
                        "managed worktree path was empty".to_string(),
                        ErrorKind::ServerError,
                    )
                })?),
                next_taskmd_id_hint,
            };
            resolved_model = intent.model.clone().unwrap_or_else(|| {
                manager
                    .llm_registry
                    .cheap_model_id_for_provider(registry_default)
            });
        }
        other => {
            return Err((
                format!("Invalid mode '{other}'. Expected one of: direct, managed, branch, auto"),
                ErrorKind::InvalidRequest,
            ))
        }
    }

    let files = manager
        .db()
        .get_conversation_creation_job_files(&job.id)
        .await
        .map_err(|e| (e.to_string(), ErrorKind::ServerError))?;
    let images = manager
        .db()
        .get_conversation_creation_job_images(&job.id)
        .await
        .map_err(|e| (e.to_string(), ErrorKind::ServerError))?;

    let seed_title = intent.seed_label.clone().filter(|s| !s.trim().is_empty());
    let mut title_source = intent.text.trim().to_string();
    if !images.is_empty() {
        if !title_source.is_empty() {
            title_source.push('\n');
        }
        let _ = write!(
            title_source,
            "{} image attachment{}",
            images.len(),
            if images.len() == 1 { "" } else { "s" }
        );
    }
    if !files.is_empty() {
        if !title_source.is_empty() {
            title_source.push('\n');
        }
        let _ = write!(
            title_source,
            "{} file attachment{}",
            files.len(),
            if files.len() == 1 { "" } else { "s" }
        );
    }
    let generated_title = if seed_title.is_none() && !title_source.is_empty() {
        if let Some(cheap_model) = manager.model_registry().get_cheap_model() {
            crate::title_generator::generate_title(&title_source, cheap_model).await
        } else {
            None
        }
    } else {
        None
    };
    let title = seed_title
        .or(generated_title)
        .or_else(|| (!title_source.is_empty()).then_some(title_from_text(&title_source)));
    let slug = title
        .as_deref()
        .map(slugify_label)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(generate_slug);

    checkpoint_creation_stage(
        manager,
        job,
        &claim,
        phoenix_core::domain::creation_protocol::CreationStage::FinalizeAttachments,
    )
    .await?;
    let seeded_empty = (intent.seed_parent_id.is_some() || intent.seed_label.is_some())
        && intent.text.trim().is_empty()
        && images.is_empty()
        && files.is_empty();
    let expanded_initial_message = if seeded_empty {
        None
    } else {
        let expanded = if intent.expansion_preflighted {
            (
                intent.text.clone(),
                intent.llm_text.clone(),
                intent.skill_invocation.clone(),
            )
        } else {
            let resolution_root =
                crate::resolution_root::ResolutionRoot::working_dir(&effective_cwd);
            let expanded = crate::message_expander::expand(&intent.text, &resolution_root)
                .map_err(|e| {
                    (
                        format!("{} ({})", e, e.error_type()),
                        ErrorKind::InvalidRequest,
                    )
                })?;
            let llm_text =
                (expanded.llm_text != expanded.display_text).then_some(expanded.llm_text);
            (expanded.display_text, llm_text, expanded.skill_invocation)
        };
        checkpoint_creation_stage(
            manager,
            job,
            &claim,
            phoenix_core::domain::creation_protocol::CreationStage::ExpandInitialMessage,
        )
        .await?;
        Some(expanded)
    };
    let metadata_expected_stage = if seeded_empty {
        phoenix_core::domain::creation_protocol::CreationStage::FinalizeAttachments
    } else {
        phoenix_core::domain::creation_protocol::CreationStage::ExpandInitialMessage
    };

    if creation_metadata_needs_commit(job.protocol.stage) {
        let metadata_outcome = manager
            .db()
            .update_conversation_creation_metadata_and_mode(
                &job.id,
                &claim,
                &job.conversation_id,
                &ConversationCreationMetadataUpdate {
                    slug: Some(slug.clone()),
                    title: Some(title.clone()),
                    cwd: Some(effective_cwd.clone()),
                    project_id: Some(project_id.clone()),
                    desired_base_branch: Some(desired_base_branch.clone()),
                },
                &conv_mode,
                &resolved_model,
                metadata_expected_stage,
                phoenix_core::domain::creation_protocol::CreationStage::CommitMetadata,
            )
            .await
            .map_err(|error| (error.to_string(), ErrorKind::ServerError))?;
        if matches!(metadata_outcome, crate::db::CreationCasOutcome::ClaimLost) {
            return Err((
                "creation claim was lost before metadata commit".to_string(),
                ErrorKind::Cancelled,
            ));
        }
        job.protocol.stage = phoenix_core::domain::creation_protocol::CreationStage::CommitMetadata;
        manager.mirror_creation_after_commit(job.id.clone());
    }
    let persisted_conversation = manager
        .db()
        .get_conversation(&job.conversation_id)
        .await
        .map_err(|e| (e.to_string(), ErrorKind::ServerError))?;
    let persisted_slug = persisted_conversation.slug.unwrap_or_else(|| slug.clone());
    let persisted_title = persisted_conversation.title.or_else(|| title.clone());

    if let Some(broadcast_tx) = {
        let runtimes = manager.runtimes.read().await;
        runtimes
            .get(&job.conversation_id)
            .map(|h| h.broadcast_tx.clone())
    } {
        let _ = broadcast_tx.send_seq(|seq| SseEvent::ConversationUpdate {
            sequence_id: seq,
            update: ConversationMetadataUpdate {
                slug: Some(persisted_slug.clone()),
                title: persisted_title.clone(),
                cwd: Some(effective_cwd.clone()),
                project_id: persisted_conversation.project_id.clone(),
                project_name: None,
                updated_at: Some(persisted_conversation.updated_at.to_rfc3339()),
                branch_name: conv_mode.branch_name().map(ToString::to_string),
                worktree_path: conv_mode.worktree_path().map(ToString::to_string),
                conv_mode_label: Some(conv_mode.label().to_string()),
                base_branch: conv_mode.base_branch().map(ToString::to_string),
                task_title: conv_mode.task_title().map(ToString::to_string),
                work_scope_key: Some(
                    crate::work_scope::WorkScope::resolve(
                        &job.conversation_id,
                        conv_mode.worktree_path().map(Path::new),
                    )
                    .stable_key(),
                ),
                model: Some(resolved_model.clone()),
            },
        });
    }

    if seeded_empty {
        let outcome = manager
            .db()
            .complete_seeded_empty_conversation_creation(
                &job.id,
                &claim,
                &job.conversation_id,
                chrono::Utc::now(),
            )
            .await
            .map_err(|e| (e.to_string(), ErrorKind::ServerError))?;
        if matches!(outcome, crate::db::CreationCasOutcome::ClaimLost) {
            return Err((
                "creation claim was lost before seeded completion".to_string(),
                ErrorKind::Cancelled,
            ));
        }
        manager.mirror_creation_after_commit(job.id.clone());
        if let Some(broadcast_tx) = {
            let runtimes = manager.runtimes.read().await;
            runtimes
                .get(&job.conversation_id)
                .map(|h| h.broadcast_tx.clone())
        } {
            let idle = ConvState::Idle;
            let _ = broadcast_tx.send_seq(|seq| SseEvent::StateChange {
                sequence_id: seq,
                state: idle.clone(),
                presentation_mode: idle.presentation_mode().to_string(),
                state_updated_at: chrono::Utc::now(),
            });
        }
        manager
            .evict_runtime(&job.conversation_id, EvictionReason::CreationProvisioned)
            .await;
        return Ok(ProvisionOutcome::SeededEmpty);
    }

    let (display_text, llm_text, skill_invocation) = expanded_initial_message.ok_or_else(|| {
        (
            "initial message expansion was missing".to_string(),
            ErrorKind::ServerError,
        )
    })?;
    let message_id = job.message_id.clone().ok_or_else(|| {
        (
            "creation job missing initial message_id".to_string(),
            ErrorKind::ServerError,
        )
    })?;
    let event = Event::CreationProvisioned {
        job_id: job.id.clone(),
        claim: claim.clone(),
        initial_message: phoenix_core::domain::sm_event::SteerEntry {
            text: display_text,
            llm_text,
            images,
            files,
            message_id,
            user_agent: None,
            skill_invocation,
        },
    };
    manager
        .evict_runtime(&job.conversation_id, EvictionReason::CreationProvisioned)
        .await;
    let handle = manager
        .get_or_create(&job.conversation_id)
        .await
        .map_err(|e| (e, ErrorKind::ServerError))?;
    let authority = manager
        .db()
        .renew_conversation_creation_claim(
            &job.id,
            &claim,
            chrono::Utc::now(),
            chrono::Duration::seconds(30),
        )
        .await
        .map_err(|error| (error.to_string(), ErrorKind::ServerError))?;
    if matches!(authority, crate::db::CreationCasOutcome::ClaimLost) {
        return Err((
            "creation claim was lost before runtime bootstrap enqueue".to_string(),
            ErrorKind::Cancelled,
        ));
    }
    handle
        .event_tx
        .send(event)
        .await
        .map_err(|e| (format!("Failed to send event: {e}"), ErrorKind::ServerError))?;
    checkpoint_creation_stage(
        manager,
        job,
        &claim,
        phoenix_core::domain::creation_protocol::CreationStage::Finalize,
    )
    .await?;
    Ok(ProvisionOutcome::InitialMessageSubmitted)
}

struct RepositoryMutationLock {
    file: std::fs::File,
}

impl RepositoryMutationLock {
    fn acquire(repo_root: &str) -> Result<Self, (String, ErrorKind)> {
        let (file, lock_path) = Self::open_file(repo_root)?;
        file.lock_exclusive().map_err(|error| {
            (
                format!("could not lock repository {}: {error}", lock_path.display()),
                ErrorKind::ServerError,
            )
        })?;
        Ok(Self { file })
    }

    fn open_file(repo_root: &str) -> Result<(std::fs::File, PathBuf), (String, ErrorKind)> {
        let common_dir = crate::git_ops::run_git(
            Path::new(repo_root),
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )
        .map_err(|error| (error, ErrorKind::ServerError))?;
        let lock_path = PathBuf::from(common_dir.trim()).join("phoenix-creation.lock");
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .map_err(|error| {
                (
                    format!(
                        "could not open repository creation lock {}: {error}",
                        lock_path.display()
                    ),
                    ErrorKind::ServerError,
                )
            })?;
        Ok((file, lock_path))
    }
}

impl Drop for RepositoryMutationLock {
    fn drop(&mut self) {
        if let Err(error) = self.file.unlock() {
            tracing::warn!(error = %error, "failed to unlock repository creation lock");
        }
    }
}

fn creation_metadata_needs_commit(
    stage: phoenix_core::domain::creation_protocol::CreationStage,
) -> bool {
    stage < phoenix_core::domain::creation_protocol::CreationStage::CommitMetadata
}

async fn checkpoint_creation_stage(
    manager: &Arc<RuntimeManager>,
    job: &mut crate::db::ConversationCreationJob,
    claim: &phoenix_core::domain::creation_protocol::CreationClaim,
    target: phoenix_core::domain::creation_protocol::CreationStage,
) -> Result<(), (String, ErrorKind)> {
    while job.protocol.stage < target {
        let current = job.protocol.stage;
        let Some(next) = current.next() else {
            return Err((
                "creation stage cannot advance beyond finalize".to_string(),
                ErrorKind::ServerError,
            ));
        };
        let outcome = manager
            .db()
            .advance_conversation_creation_stage(&job.id, claim, current, next, chrono::Utc::now())
            .await
            .map_err(|error| (error.to_string(), ErrorKind::ServerError))?;
        if matches!(outcome, crate::db::CreationCasOutcome::ClaimLost) {
            return Err((
                "creation claim was lost while checkpointing".to_string(),
                ErrorKind::Cancelled,
            ));
        }
        job.protocol.stage = next;
        manager.mirror_creation_after_commit(job.id.clone());
    }
    Ok(())
}

async fn reserve_worktree(
    manager: &Arc<RuntimeManager>,
    job: &crate::db::ConversationCreationJob,
    claim: &phoenix_core::domain::creation_protocol::CreationClaim,
    repo_root: &str,
    worktree_path: &Path,
) -> Result<(), (String, ErrorKind)> {
    let outcome = manager
        .db()
        .reserve_conversation_creation_resource(
            &format!("{}:worktree", job.id),
            &job.id,
            claim,
            repo_root,
            &worktree_path.to_string_lossy(),
            chrono::Utc::now(),
        )
        .await
        .map_err(|error| (error.to_string(), ErrorKind::ServerError))?;
    if matches!(outcome, crate::db::CreationCasOutcome::ClaimLost) {
        return Err((
            "creation claim was lost before reserving worktree".to_string(),
            ErrorKind::Cancelled,
        ));
    }
    manager.mirror_creation_after_commit(job.id.clone());
    Ok(())
}

async fn mark_worktree_present(
    manager: &Arc<RuntimeManager>,
    job: &crate::db::ConversationCreationJob,
    claim: &phoenix_core::domain::creation_protocol::CreationClaim,
    worktree_path: &Path,
) -> Result<(), (String, ErrorKind)> {
    let outcome = manager
        .db()
        .mark_creation_resource_present(
            &job.id,
            claim,
            &worktree_path.to_string_lossy(),
            chrono::Utc::now(),
        )
        .await
        .map_err(|error| (error.to_string(), ErrorKind::ServerError))?;
    if matches!(outcome, crate::db::CreationCasOutcome::ClaimLost) {
        return Err((
            "creation claim was lost after materializing worktree".to_string(),
            ErrorKind::Cancelled,
        ));
    }
    manager.mirror_creation_after_commit(job.id.clone());
    Ok(())
}

fn branch_worktree_error_to_kind(error: BranchWorktreeError) -> (String, ErrorKind) {
    match error {
        BranchWorktreeError::Conflict { slug } => (
            format!("Branch already owned by conversation {slug}"),
            ErrorKind::InvalidRequest,
        ),
        BranchWorktreeError::BadRequest(message) => (message, ErrorKind::InvalidRequest),
        BranchWorktreeError::Git(message) => (message, ErrorKind::ServerError),
    }
}

fn managed_worktree_error_to_kind(error: ManagedWorktreeError) -> (String, ErrorKind) {
    match error {
        ManagedWorktreeError::BadRequest(message) => (message, ErrorKind::InvalidRequest),
        ManagedWorktreeError::Git(message) => (message, ErrorKind::ServerError),
    }
}

#[cfg(test)]
mod temporary_creation_branch_tests {
    use super::*;

    #[test]
    fn managed_cleanup_deletes_only_implicit_temporary_branch() {
        let mut cleanup = test_cleanup_job();
        cleanup.intent.mode = Some("managed".to_string());
        cleanup.intent.checkout_ref = None;
        assert_eq!(
            temporary_creation_branch(&cleanup),
            Some("task-pending-conversa".to_string())
        );
        cleanup.intent.checkout_ref = Some("feature".to_string());
        assert_eq!(temporary_creation_branch(&cleanup), None);
        cleanup.intent.mode = Some("direct".to_string());
        cleanup.intent.checkout_ref = None;
        assert_eq!(temporary_creation_branch(&cleanup), None);
    }

    #[test]
    fn temporary_branch_is_deleted_after_worktree_cleanup() {
        let repo = tempfile::tempdir().unwrap();
        crate::git_ops::run_git(repo.path(), &["init"]).unwrap();
        crate::git_ops::run_git(repo.path(), &["config", "user.email", "test@example.com"])
            .unwrap();
        crate::git_ops::run_git(repo.path(), &["config", "user.name", "Test"]).unwrap();
        std::fs::write(repo.path().join("README"), b"test").unwrap();
        crate::git_ops::run_git(repo.path(), &["add", "README"]).unwrap();
        crate::git_ops::run_git(repo.path(), &["commit", "-m", "initial"]).unwrap();
        crate::git_ops::run_git(repo.path(), &["branch", "task-pending-conversa"]).unwrap();

        delete_temporary_creation_branch(&repo.path().to_string_lossy(), "task-pending-conversa")
            .unwrap();
        assert!(crate::git_ops::run_git(
            repo.path(),
            &[
                "show-ref",
                "--verify",
                "--quiet",
                "refs/heads/task-pending-conversa"
            ]
        )
        .is_err());
    }

    fn test_cleanup_job() -> crate::db::CreationCleanupJob {
        crate::db::CreationCleanupJob {
            job_id: "job".to_string(),
            conversation_id: "conversation-id".to_string(),
            intent: phoenix_core::domain::db_schema::ConversationCreationIntent {
                cwd: "/tmp".to_string(),
                model: None,
                text: String::new(),
                expansion_preflighted: false,
                llm_text: None,
                skill_invocation: None,
                message_id: String::new(),
                images: vec![],
                files: vec![],
                mode: None,
                base_branch: None,
                checkout_ref: None,
                seed_parent_id: None,
                seed_label: None,
            },
            status: "cancelling".to_string(),
            generation: 1,
            worker_id: "worker".to_string(),
            token: "token".to_string(),
            lease_until: chrono::Utc::now(),
            reservations: vec![],
        }
    }
}

#[cfg(test)]
mod partial_worktree_cleanup_tests {
    use super::*;

    #[test]
    fn cleanup_removes_unregistered_partial_directory() {
        let repo = tempfile::tempdir().unwrap();
        crate::git_ops::run_git(repo.path(), &["init"]).unwrap();
        let partial = repo.path().join(".phoenix/worktrees/conversation");
        std::fs::create_dir_all(&partial).unwrap();
        std::fs::write(partial.join("partial"), b"incomplete").unwrap();

        remove_owned_worktree_for_cleanup(&repo.path().to_string_lossy(), &partial).unwrap();
        assert!(!partial.exists());
    }
}

#[cfg(test)]
mod worktree_repository_ownership_tests {
    use super::*;

    #[test]
    fn existing_worktree_must_match_requested_branch() {
        let repo = tempfile::tempdir().unwrap();
        crate::git_ops::run_git(repo.path(), &["init"]).unwrap();
        crate::git_ops::run_git(repo.path(), &["config", "user.email", "test@example.com"])
            .unwrap();
        crate::git_ops::run_git(repo.path(), &["config", "user.name", "Test User"]).unwrap();
        std::fs::write(repo.path().join("README"), "test").unwrap();
        crate::git_ops::run_git(repo.path(), &["add", "README"]).unwrap();
        crate::git_ops::run_git(repo.path(), &["commit", "-m", "initial"]).unwrap();
        crate::git_ops::run_git(repo.path(), &["checkout", "-b", "actual"]).unwrap();

        let error = validate_worktree_branch(repo.path(), "expected")
            .expect_err("wrong branch must not be adopted");
        assert_eq!(error.1, ErrorKind::InvalidRequest);
        assert!(error.0.contains("actual"));
        assert!(error.0.contains("expected"));
    }

    #[test]
    fn standalone_checkout_is_not_adopted_for_reserved_repository() {
        let expected = tempfile::tempdir().unwrap();
        let foreign = tempfile::tempdir().unwrap();
        crate::git_ops::run_git(expected.path(), &["init"]).unwrap();
        crate::git_ops::run_git(foreign.path(), &["init"]).unwrap();

        let error = validate_worktree_belongs_to_repository(
            &expected.path().to_string_lossy(),
            foreign.path(),
        )
        .expect_err("foreign checkout must not be adopted");
        assert_eq!(error.1, ErrorKind::InvalidRequest);
    }
}

#[cfg(test)]
mod partial_worktree_reconciliation_tests {
    use super::*;

    #[test]
    fn owned_partial_directory_is_removed_for_rematerialization() {
        let repo = tempfile::tempdir().unwrap();
        crate::git_ops::run_git(repo.path(), &["init"]).unwrap();
        let partial = repo.path().join(".phoenix/worktrees/conversation");
        std::fs::create_dir_all(&partial).unwrap();
        std::fs::write(partial.join("partial"), b"incomplete").unwrap();

        assert!(!reconcile_owned_worktree_path(&repo.path().to_string_lossy(), &partial).unwrap());
        assert!(!partial.exists());
    }
}

#[cfg(test)]
mod missing_resource_cleanup_tests {
    use super::*;

    #[test]
    fn missing_repository_and_resource_are_already_reconciled() {
        let root = tempfile::tempdir().unwrap();
        assert!(missing_repository_and_resource(
            &root.path().join("repo"),
            &root.path().join("worktree")
        ));
    }

    #[test]
    fn absent_resource_with_live_repository_requires_locking() {
        let repo = tempfile::tempdir().unwrap();
        assert!(!missing_repository_and_resource(
            repo.path(),
            &repo.path().join("worktree")
        ));
    }
}

#[cfg(test)]
mod branch_error_classification_tests {
    use super::*;

    #[test]
    fn infrastructure_git_failure_is_retryable() {
        let (_, kind) = branch_worktree_error_to_kind(BranchWorktreeError::Git(
            "temporary git failure".to_string(),
        ));
        assert_eq!(kind, ErrorKind::ServerError);
    }

    #[test]
    fn user_actionable_branch_failure_is_permanent() {
        let (_, kind) = branch_worktree_error_to_kind(BranchWorktreeError::BadRequest(
            "branch not found".to_string(),
        ));
        assert_eq!(kind, ErrorKind::InvalidRequest);
    }
}

#[cfg(test)]
mod bootstrap_stage_recovery_tests {
    use super::*;
    use phoenix_core::domain::creation_protocol::CreationStage;

    #[test]
    fn metadata_is_not_recommitted_when_bootstrap_replays() {
        assert!(!creation_metadata_needs_commit(
            CreationStage::CommitMetadata
        ));
        assert!(!creation_metadata_needs_commit(
            CreationStage::BootstrapInitialTurn
        ));
    }

    #[test]
    fn metadata_is_committed_after_initial_message_expansion() {
        assert!(creation_metadata_needs_commit(
            CreationStage::ExpandInitialMessage
        ));
    }
}

#[cfg(test)]
mod existing_message_recovery_tests {
    use super::*;

    #[test]
    fn persisted_message_does_not_complete_job_while_state_is_provisioning() {
        let state = ConvState::Provisioning {
            job_id: "job".to_string(),
            phase: phoenix_core::domain::db_schema::ConversationCreationPhase::Provisioning,
        };
        assert!(!creation_state_allows_existing_message_completion(&state));
    }

    #[test]
    fn persisted_message_does_not_complete_job_before_llm_dispatch() {
        let state = ConvState::LlmRequesting { attempt: 1 };
        assert!(!creation_state_allows_existing_message_completion(&state));
    }

    #[test]
    fn persisted_message_completes_job_after_state_advances() {
        assert!(creation_state_allows_existing_message_completion(
            &ConvState::Idle
        ));
    }
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod repository_lock_tests {
    use super::RepositoryMutationLock;
    use fs2::FileExt;

    #[test]
    fn repository_mutation_lock_serializes_live_holders() {
        let repo = tempfile::tempdir().unwrap();
        let status = phoenix_core::git::command()
            .args(["init", "--quiet"])
            .current_dir(repo.path())
            .status()
            .unwrap();
        assert!(status.success());
        let repo_path = repo.path().to_string_lossy().to_string();
        let first = RepositoryMutationLock::acquire(&repo_path).unwrap();
        let (second, _) = RepositoryMutationLock::open_file(&repo_path).unwrap();

        assert!(second.try_lock_exclusive().is_err());
        drop(first);
        second.try_lock_exclusive().unwrap();
        second.unlock().unwrap();
    }
}

fn temporary_creation_branch(cleanup: &crate::db::CreationCleanupJob) -> Option<String> {
    let mode = cleanup.intent.mode.as_deref()?;
    if !matches!(mode, "managed" | "auto") || cleanup.intent.checkout_ref.is_some() {
        return None;
    }
    Some(format!(
        "task-pending-{}",
        cleanup.conversation_id.chars().take(8).collect::<String>()
    ))
}

fn delete_temporary_creation_branch(repo_root: &str, branch: &str) -> Result<(), String> {
    let repo = Path::new(repo_root);
    if crate::git_ops::find_branch_in_worktree_list(repo, branch).is_some() {
        return Err(format!(
            "temporary creation branch '{branch}' remains checked out"
        ));
    }
    let exists = crate::git_ops::run_git(
        repo,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    );
    if exists.is_ok() {
        crate::git_ops::run_git(repo, &["branch", "-D", "--", branch])?;
    }
    Ok(())
}

fn remove_owned_worktree_for_cleanup(repo_root: &str, path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    match phoenix_core::git::detect_git_repo_root(path) {
        Some(detected) => {
            let owning_repo = Path::new(repo_root)
                .canonicalize()
                .map_err(|error| format!("Could not canonicalize repository {repo_root}: {error}"))?
                .to_string_lossy()
                .to_string();
            if detected == owning_repo {
                std::fs::remove_dir_all(path).map_err(|error| {
                    format!(
                        "Could not remove partial worktree {}: {error}",
                        path.display()
                    )
                })?;
            } else {
                validate_existing_worktree(path).map_err(|(message, _)| message)?;
                validate_worktree_belongs_to_repository(repo_root, path)
                    .map_err(|(message, _)| message)?;
                crate::git_ops::run_git(
                    Path::new(repo_root),
                    &["worktree", "remove", "--force", &path.to_string_lossy()],
                )?;
            }
        }
        None => {
            std::fs::remove_dir_all(path).map_err(|error| {
                format!(
                    "Could not remove partial worktree {}: {error}",
                    path.display()
                )
            })?;
        }
    }
    let _ = crate::git_ops::run_git(Path::new(repo_root), &["worktree", "prune"]);
    Ok(())
}

fn reconcile_owned_worktree_path(
    repo_root: &str,
    path: &Path,
) -> Result<bool, (String, ErrorKind)> {
    if !path.exists() {
        return Ok(false);
    }
    if let Some(detected) = phoenix_core::git::detect_git_repo_root(path) {
        let owning_repo = Path::new(repo_root)
            .canonicalize()
            .map_err(|error| {
                (
                    format!("Could not canonicalize repository {repo_root}: {error}"),
                    ErrorKind::ServerError,
                )
            })?
            .to_string_lossy()
            .to_string();
        if detected != owning_repo {
            validate_existing_worktree(path)?;
            validate_worktree_belongs_to_repository(repo_root, path)?;
            return Ok(true);
        }
    }
    let metadata = std::fs::symlink_metadata(path).map_err(|error| {
        (
            format!(
                "Could not inspect partial worktree {}: {error}",
                path.display()
            ),
            ErrorKind::ServerError,
        )
    })?;
    if metadata.file_type().is_symlink() || metadata.is_file() {
        std::fs::remove_file(path)
    } else {
        std::fs::remove_dir_all(path)
    }
    .map_err(|error| {
        (
            format!(
                "Could not remove partial worktree {}: {error}",
                path.display()
            ),
            ErrorKind::ServerError,
        )
    })?;
    let _ = crate::git_ops::run_git(Path::new(repo_root), &["worktree", "prune"]);
    Ok(false)
}

fn validate_worktree_branch(path: &Path, expected_branch: &str) -> Result<(), (String, ErrorKind)> {
    let actual_branch = crate::git_ops::run_git(path, &["rev-parse", "--abbrev-ref", "HEAD"])
        .map_err(|error| (error, ErrorKind::InvalidRequest))?;
    let actual_branch = actual_branch.trim();
    if actual_branch == expected_branch {
        return Ok(());
    }
    Err((
        format!(
            "Existing worktree {} is on branch '{actual_branch}', expected '{expected_branch}'",
            path.display()
        ),
        ErrorKind::InvalidRequest,
    ))
}

fn validate_worktree_belongs_to_repository(
    repo_root: &str,
    worktree: &Path,
) -> Result<(), (String, ErrorKind)> {
    let common_dir = |cwd: &Path| {
        crate::git_ops::run_git(
            cwd,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )
        .map(|value| value.trim().to_string())
        .map_err(|error| (error, ErrorKind::ServerError))
    };
    let expected = common_dir(Path::new(repo_root))?;
    let actual = common_dir(worktree)?;
    if actual != expected {
        return Err((
            format!(
                "Existing worktree {} belongs to a different repository",
                worktree.display()
            ),
            ErrorKind::InvalidRequest,
        ));
    }
    Ok(())
}

fn validate_existing_worktree(path: &Path) -> Result<(), (String, ErrorKind)> {
    let detected = phoenix_core::git::detect_git_repo_root(path).ok_or_else(|| {
        (
            format!(
                "Existing worktree path is not a git worktree: {}",
                path.display()
            ),
            ErrorKind::InvalidRequest,
        )
    })?;
    let expected = path.canonicalize().map_err(|e| {
        (
            format!(
                "Could not canonicalize existing worktree {}: {e}",
                path.display()
            ),
            ErrorKind::ServerError,
        )
    })?;
    let expected = expected.to_string_lossy().to_string();
    if detected != expected {
        return Err((
            format!(
                "Existing worktree path {} resolves to unexpected git root {}",
                path.display(),
                detected
            ),
            ErrorKind::InvalidRequest,
        ));
    }
    Ok(())
}

fn deterministic_worktree_path(repo_root: &str, conv_id: &str) -> std::path::PathBuf {
    Path::new(repo_root)
        .join(".phoenix")
        .join("worktrees")
        .join(conv_id)
}

fn creation_error_is_retryable(kind: &ErrorKind) -> bool {
    matches!(
        kind,
        ErrorKind::RateLimit
            | ErrorKind::Network
            | ErrorKind::InvalidResponse
            | ErrorKind::ServerError
            | ErrorKind::TimedOut
    )
}

fn app_error_to_kind(error: AppError) -> (String, ErrorKind) {
    match error {
        AppError::Conflict(response) => (response.error.clone(), ErrorKind::InvalidRequest),
        AppError::BadRequest(message)
        | AppError::TypedBadRequest { message, .. }
        | AppError::UnprocessableEntity(crate::api::ExpansionErrorResponse {
            error: message,
            ..
        })
        | AppError::NotFound(message) => (message, ErrorKind::InvalidRequest),
        AppError::Internal(message) | AppError::TypedInternal { message, .. } => {
            (message, ErrorKind::ServerError)
        }
        AppError::Forbidden(message) => (message, ErrorKind::Auth),
    }
}
