use crate::api::handlers::{
    create_branch_worktree_blocking, create_managed_explore_worktree_blocking, generate_slug,
    slugify_label, validate_user_ref, AppError, BranchWorktreeError, BranchWorktreeInfo,
    ManagedWorktreeError,
};
use crate::db::{
    ConvMode, ConversationCreationMetadataUpdate, ConversationCreationPhase, ErrorKind,
    NonEmptyString,
};
use crate::runtime::{ConversationMetadataUpdate, EvictionReason, RuntimeManager, SseEvent};
use crate::state_machine::{ConvState, Event};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub(crate) async fn drain_pending_jobs(manager: &Arc<RuntimeManager>) -> Result<(), String> {
    let jobs = manager
        .db()
        .list_pending_conversation_creation_jobs()
        .await
        .map_err(|e| e.to_string())?;
    for job in jobs {
        if let Err(error) = process_job(manager, job).await {
            tracing::error!(error = %error, "conversation creation job processing failed");
        }
    }
    Ok(())
}

async fn process_job(
    manager: &Arc<RuntimeManager>,
    job: crate::db::ConversationCreationJob,
) -> Result<(), String> {
    let conv_id = job.conversation_id.clone();
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
        manager
            .db()
            .mark_conversation_creation_job_complete(&job.id)
            .await
            .map_err(|e| e.to_string())?;
        return Ok(());
    }

    manager
        .db()
        .mark_conversation_creation_job_phase(&job.id, ConversationCreationPhase::Provisioning)
        .await
        .map_err(|e| e.to_string())?;
    let provisioning = ConvState::Provisioning {
        job_id: job.id.clone(),
        phase: ConversationCreationPhase::Provisioning,
    };
    manager
        .db()
        .update_conversation_state(&conv_id, &provisioning)
        .await
        .map_err(|e| e.to_string())?;
    if let Some(broadcast_tx) = {
        let runtimes = manager.runtimes.read().await;
        runtimes.get(&conv_id).map(|h| h.broadcast_tx.clone())
    } {
        let _ = broadcast_tx.send_seq(|seq| SseEvent::StateChange {
            sequence_id: seq,
            state: provisioning.clone(),
            presentation_mode: provisioning.presentation_mode().to_string(),
            state_updated_at: chrono::Utc::now(),
        });
    }

    match provision_conversation(manager, &job).await {
        Ok(ProvisionOutcome::SeededEmpty) => {
            manager
                .db()
                .mark_conversation_creation_job_complete(&job.id)
                .await
                .map_err(|e| e.to_string())?;
            Ok(())
        }
        Ok(ProvisionOutcome::InitialMessageSubmitted) => Ok(()),
        Err((message, kind)) => {
            let failed = ConvState::CreationFailed {
                job_id: job.id.clone(),
                error: message.clone(),
                error_kind: kind,
            };
            manager
                .db()
                .update_conversation_state(&conv_id, &failed)
                .await
                .map_err(|e| e.to_string())?;
            manager
                .db()
                .mark_conversation_creation_job_failed(&job.id, &message)
                .await
                .map_err(|e| e.to_string())?;
            if let Some(broadcast_tx) = {
                let runtimes = manager.runtimes.read().await;
                runtimes.get(&conv_id).map(|h| h.broadcast_tx.clone())
            } {
                let _ = broadcast_tx.send_seq(|seq| SseEvent::StateChange {
                    sequence_id: seq,
                    state: failed.clone(),
                    presentation_mode: failed.presentation_mode().to_string(),
                    state_updated_at: chrono::Utc::now(),
                });
            }
            manager
                .evict_runtime(&conv_id, EvictionReason::CreationProvisioned)
                .await;
            Err(message)
        }
    }
}

enum ProvisionOutcome {
    SeededEmpty,
    InitialMessageSubmitted,
}

#[allow(clippy::too_many_lines)]
async fn provision_conversation(
    manager: &Arc<RuntimeManager>,
    job: &crate::db::ConversationCreationJob,
) -> Result<ProvisionOutcome, (String, ErrorKind)> {
    let intent = &job.intent;
    let valid_cwd = crate::conversation_cwd::validate_conversation_cwd(&intent.cwd)
        .map_err(|e| (e.to_string(), ErrorKind::InvalidRequest))?;
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
            let info = if existing_path.exists() {
                validate_existing_worktree(&existing_path)?;
                let worktree_path = existing_path.to_string_lossy().to_string();
                let default_branch = crate::git_ops::run_git(
                    Path::new(&repo_root),
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
                        Path::new(&repo_root),
                        &["rev-parse", "--abbrev-ref", "HEAD"],
                    )
                    .ok()
                    .map(|s| s.trim().to_string())
                })
                .unwrap_or_else(|| branch_name.clone());
                Ok(BranchWorktreeInfo {
                    branch_name: branch_name.clone(),
                    worktree_path,
                    base_branch: default_branch,
                })
            } else {
                let db = manager.db().clone();
                let conv_id = job.conversation_id.clone();
                tokio::task::spawn_blocking(move || {
                    create_branch_worktree_blocking(&repo_root, &conv_id, &branch_name, &db)
                })
                .await
                .map_err(|e| {
                    (
                        format!("spawn_blocking failed: {e}"),
                        ErrorKind::ServerError,
                    )
                })?
            };
            let info = match info {
                Ok(info) => info,
                Err(BranchWorktreeError::Conflict { slug }) => {
                    return Err((
                        format!("Branch already owned by conversation {slug}"),
                        ErrorKind::InvalidRequest,
                    ))
                }
                Err(BranchWorktreeError::Git(msg) | BranchWorktreeError::BadRequest(msg)) => {
                    return Err((msg, ErrorKind::InvalidRequest))
                }
            };
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
            let worktree = if existing_path.exists() {
                validate_existing_worktree(&existing_path)?;
                Ok(existing_path.to_string_lossy().to_string())
            } else {
                let conv_id = job.conversation_id.clone();
                let repo_for_blocking = repo_root.clone();
                let base_branch_for_blocking = base_branch.clone();
                let checkout_ref = intent.checkout_ref.clone();
                tokio::task::spawn_blocking(move || {
                    create_managed_explore_worktree_blocking(
                        &repo_for_blocking,
                        &conv_id,
                        &base_branch_for_blocking,
                        checkout_ref.as_deref(),
                    )
                })
                .await
                .map_err(|e| {
                    (
                        format!("spawn_blocking failed: {e}"),
                        ErrorKind::ServerError,
                    )
                })?
            };
            let worktree = match worktree {
                Ok(path) => path,
                Err(ManagedWorktreeError::BadRequest(msg)) => {
                    return Err((msg, ErrorKind::InvalidRequest))
                }
                Err(ManagedWorktreeError::Git(msg)) => return Err((msg, ErrorKind::ServerError)),
            };
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
    let title = seed_title.or(generated_title);
    let slug = title
        .as_deref()
        .map(slugify_label)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(generate_slug);

    if let Err(e) = manager
        .db()
        .update_conversation_creation_metadata_and_mode(
            &job.conversation_id,
            &ConversationCreationMetadataUpdate {
                slug: Some(slug.clone()),
                title: Some(title.clone()),
                cwd: Some(effective_cwd.clone()),
                project_id: Some(project_id.clone()),
                desired_base_branch: Some(desired_base_branch.clone()),
            },
            &conv_mode,
        )
        .await
    {
        cleanup_unpersisted_worktree(repo_root.as_deref(), &job.conversation_id, &conv_mode);
        return Err((e.to_string(), ErrorKind::ServerError));
    }
    manager
        .db()
        .update_conversation_model(&job.conversation_id, &resolved_model)
        .await
        .map_err(|e| (e.to_string(), ErrorKind::ServerError))?;

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

    let seeded_empty = (intent.seed_parent_id.is_some() || intent.seed_label.is_some())
        && intent.text.trim().is_empty()
        && images.is_empty()
        && files.is_empty();

    if seeded_empty {
        manager
            .db()
            .update_conversation_state(&job.conversation_id, &ConvState::Idle)
            .await
            .map_err(|e| (e.to_string(), ErrorKind::ServerError))?;
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

    let (display_text, llm_text, skill_invocation) = if intent.expansion_preflighted {
        (
            intent.text.clone(),
            intent.llm_text.clone(),
            intent.skill_invocation.clone(),
        )
    } else {
        let resolution_root = crate::resolution_root::ResolutionRoot::working_dir(&effective_cwd);
        let expanded =
            crate::message_expander::expand(&intent.text, &resolution_root).map_err(|e| {
                (
                    format!("{} ({})", e, e.error_type()),
                    ErrorKind::InvalidRequest,
                )
            })?;
        let llm_text = (expanded.llm_text != expanded.display_text).then_some(expanded.llm_text);
        (expanded.display_text, llm_text, expanded.skill_invocation)
    };
    let message_id = job.message_id.clone().ok_or_else(|| {
        (
            "creation job missing initial message_id".to_string(),
            ErrorKind::ServerError,
        )
    })?;
    let event = Event::UserMessage {
        text: display_text,
        llm_text,
        images,
        files,
        message_id,
        user_agent: None,
        skill_invocation,
    };
    manager
        .db()
        .update_conversation_state(&job.conversation_id, &ConvState::Idle)
        .await
        .map_err(|e| (e.to_string(), ErrorKind::ServerError))?;
    manager
        .evict_runtime(&job.conversation_id, EvictionReason::CreationProvisioned)
        .await;
    let _handle = manager
        .get_or_create(&job.conversation_id)
        .await
        .map_err(|e| (e, ErrorKind::ServerError))?;
    let provisioning = ConvState::Provisioning {
        job_id: job.id.clone(),
        phase: ConversationCreationPhase::Provisioning,
    };
    manager
        .db()
        .update_conversation_state(&job.conversation_id, &provisioning)
        .await
        .map_err(|e| (e.to_string(), ErrorKind::ServerError))?;
    manager
        .send_event(&job.conversation_id, event)
        .await
        .map_err(|e| (e, ErrorKind::ServerError))?;
    Ok(ProvisionOutcome::InitialMessageSubmitted)
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

fn cleanup_unpersisted_worktree(
    repo_root: Option<&str>,
    conversation_id: &str,
    conv_mode: &ConvMode,
) {
    let Some(worktree_path) = conv_mode.worktree_path() else {
        return;
    };
    let worktree = PathBuf::from(worktree_path);
    let branch_to_delete = match conv_mode {
        ConvMode::Explore {
            worktree_path: Some(_),
            ..
        } => {
            let id_prefix: String = conversation_id.chars().take(8).collect();
            Some(format!("task-pending-{id_prefix}"))
        }
        ConvMode::Work { .. }
        | ConvMode::Branch { .. }
        | ConvMode::Explore {
            worktree_path: None,
            ..
        }
        | ConvMode::Direct => None,
    };

    if let Some(repo_root) = repo_root {
        let worktree_str = worktree.to_string_lossy().to_string();
        if let Err(e) = crate::git_ops::run_git(
            Path::new(repo_root),
            &["worktree", "remove", &worktree_str, "--force"],
        ) {
            tracing::warn!(conversation_id, worktree = %worktree.display(), error = %e, "failed to remove unpersisted worktree via git; trying filesystem fallback");
            if let Err(rm_err) = std::fs::remove_dir_all(&worktree) {
                tracing::warn!(conversation_id, worktree = %worktree.display(), error = %rm_err, "failed to remove unpersisted worktree via filesystem fallback");
            }
        }
        if let Some(branch) = branch_to_delete {
            if let Err(e) =
                crate::git_ops::run_git(Path::new(repo_root), &["branch", "-D", &branch])
            {
                tracing::debug!(conversation_id, branch, error = %e, "failed to delete unpersisted temporary branch");
            }
        }
    } else if let Err(e) = std::fs::remove_dir_all(&worktree) {
        tracing::warn!(conversation_id, worktree = %worktree.display(), error = %e, "failed to remove unpersisted worktree without repo root");
    }
}

fn app_error_to_kind(error: AppError) -> (String, ErrorKind) {
    match error {
        AppError::Conflict(response) => (response.error.clone(), ErrorKind::InvalidRequest),
        AppError::BadRequest(message)
        | AppError::UnprocessableEntity(crate::api::ExpansionErrorResponse {
            error: message,
            ..
        })
        | AppError::NotFound(message) => (message, ErrorKind::InvalidRequest),
        AppError::Internal(message) => (message, ErrorKind::ServerError),
        AppError::Forbidden(message) => (message, ErrorKind::Auth),
    }
}
