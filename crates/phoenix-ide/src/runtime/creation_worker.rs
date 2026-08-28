use crate::api::handlers::{
    create_branch_worktree_blocking, create_detached_task_worktree_blocking,
    create_managed_explore_worktree_blocking, generate_slug, slugify_label, title_from_text,
    validate_detached_task_worktree, validate_user_ref, AppError, BranchWorktreeError,
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
use phoenix_llm::ModelRegistry;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone)]
pub(crate) struct PublishedProductCreation {
    pub product_conversation_id: String,
    pub transcript_row_id: String,
}

pub(crate) async fn process_product_creation_request(
    manager: &Arc<RuntimeManager>,
    request_id: &str,
) -> Result<PublishedProductCreation, String> {
    if let Some(job) = manager
        .db()
        .get_product_creation_job(request_id)
        .await
        .map_err(|e| e.to_string())?
    {
        if job.status == "delivery_pending" {
            if job
                .delivery_retry_at
                .is_some_and(|retry_at| retry_at > chrono::Utc::now())
            {
                return Err(
                    "product creation objective delivery is awaiting its retry deadline"
                        .to_string(),
                );
            }
            let claim = crate::db::ProductCreationClaim {
                worker_id: job
                    .claim_worker_id
                    .clone()
                    .ok_or_else(|| "delivery_pending job missing claim worker id".to_string())?,
                token: job
                    .claim_token
                    .clone()
                    .ok_or_else(|| "delivery_pending job missing claim token".to_string())?,
                generation: job.claim_generation,
                lease_until: job
                    .claim_lease_until
                    .ok_or_else(|| "delivery_pending job missing claim lease".to_string())?,
            };
            let claimed = crate::db::ClaimedProductCreationJob {
                claim,
                job: job.clone(),
            };
            return deliver_product_creation_objective(manager, &claimed).await;
        }
        if job.status == "delivery_failed" {
            return Err(
                "product creation delivery previously failed and requires explicit retry"
                    .to_string(),
            );
        }
        if job.status == "published" {
            if let (Some(product_id), Some(conversation_id)) =
                (job.published_product_id, job.published_conversation_id)
            {
                return Ok(PublishedProductCreation {
                    product_conversation_id: product_id.to_string(),
                    transcript_row_id: conversation_id,
                });
            }
        }
    }
    let worker_id = format!("product-creation-worker-{}", uuid::Uuid::new_v4());
    let token = uuid::Uuid::new_v4().to_string();
    let Some(claimed) = manager
        .db()
        .claim_product_creation(
            request_id,
            &worker_id,
            &token,
            chrono::Utc::now(),
            chrono::Duration::seconds(30),
        )
        .await
        .map_err(|e| e.to_string())?
    else {
        return Err("product creation is already being processed or awaiting retry".to_string());
    };
    match process_product_creation_until_closed(manager, claimed.clone()).await {
        Ok(published) => Ok(published),
        Err(error) => {
            cleanup_and_retry_unpublished_product_creation(
                manager,
                request_id,
                &claimed.claim,
                &claimed.job,
                &error,
            )
            .await
            .map_err(|db_error| format!("{error}; retry persistence failed: {db_error}"))?;
            Err(error)
        }
    }
}

#[allow(clippy::too_many_lines)]
async fn process_claimed_product_creation(
    manager: &Arc<RuntimeManager>,
    claimed: &crate::db::ClaimedProductCreationJob,
) -> Result<PublishedProductCreation, String> {
    let job = &claimed.job;
    let cwd = tokio::task::spawn_blocking({
        let cwd = job.intent.cwd.clone();
        let home = manager.runtime_home().to_path_buf();
        move || {
            crate::conversation_cwd::ensure_product_creation_cwd(&cwd, &home)
                .map(crate::conversation_cwd::ValidConversationCwd::into_raw)
                .map_err(|error| error.to_string())
        }
    })
    .await
    .map_err(|error| format!("directory creation join failed: {error}"))??;
    let repo_root = tokio::task::spawn_blocking({
        let cwd = cwd.clone();
        move || discover_product_repository(Path::new(&cwd))
    })
    .await
    .map_err(|error| format!("repository discovery join failed: {error}"))??;

    let (
        effective_cwd,
        conv_mode,
        authority_kind,
        environment,
        logical_base,
        staging_repo,
        staging_oid,
    ) = if let Some(repo_root) = repo_root {
        let repo_root = repo_root.trim().to_string();
        let (oid, logical_base) = if let (Some(oid), Some(base)) = (
            job.pin_exact_checkout_oid.clone(),
            job.pin_logical_base.clone(),
        ) {
            (oid, base)
        } else {
            let resolved = tokio::task::spawn_blocking({
                let repo_root = repo_root.clone();
                move || strict_product_creation_pin(Path::new(&repo_root))
            })
            .await
            .map_err(|error| format!("starting-pin join failed: {error}"))??;
            match manager
                .db()
                .pin_product_creation_once(
                    &job.request_id,
                    &claimed.claim,
                    &resolved.0,
                    &resolved.1,
                    "fresh",
                )
                .await
                .map_err(|error| error.to_string())?
            {
                crate::db::ProductCreationPinOutcome::Pinned(_)
                | crate::db::ProductCreationPinOutcome::Same(_) => resolved,
                crate::db::ProductCreationPinOutcome::Conflict(existing) => (
                    existing
                        .pin_exact_checkout_oid
                        .ok_or_else(|| "conflicting pin lacked OID".to_string())?,
                    existing
                        .pin_logical_base
                        .ok_or_else(|| "conflicting pin lacked logical base".to_string())?,
                ),
            }
        };
        let planned_path =
            deterministic_worktree_path(&repo_root, job.product_conversation_id.as_str());
        let reservation_id = format!("{}:worktree", job.request_id);
        let reserved = manager
            .db()
            .reserve_product_creation_resource(
                &reservation_id,
                &job.request_id,
                &claimed.claim,
                &repo_root,
                planned_path.to_string_lossy().as_ref(),
                chrono::Utc::now(),
            )
            .await
            .map_err(|error| error.to_string())?;
        if !reserved {
            return Err("product creation claim was lost before reserving worktree".to_string());
        }
        if !manager
            .db()
            .record_product_creation_staging(
                &job.request_id,
                &claimed.claim,
                planned_path.to_string_lossy().as_ref(),
                &repo_root,
                &oid,
            )
            .await
            .map_err(|error| error.to_string())?
        {
            return Err("product creation staging claim was lost".to_string());
        }
        let worktree = tokio::task::spawn_blocking({
            let repo_root = repo_root.clone();
            let oid = oid.clone();
            let key = job.product_conversation_id.to_string();
            move || {
                let _lock = RepositoryMutationLock::acquire(&repo_root).map_err(|e| e.0)?;
                ensure_phoenix_staging_ignored(Path::new(&repo_root))?;
                let path = deterministic_worktree_path(&repo_root, &key);
                if reconcile_owned_worktree_path(&repo_root, &path).map_err(|e| e.0)? {
                    crate::api::handlers::validate_detached_task_worktree(&path, &oid)?;
                    Ok::<String, String>(path.to_string_lossy().to_string())
                } else {
                    std::fs::create_dir_all(
                        path.parent()
                            .ok_or_else(|| "worktree path lacked parent".to_string())?,
                    )
                    .map_err(|error| error.to_string())?;
                    run_git_with_timeout(
                        Path::new(&repo_root),
                        &[
                            "worktree",
                            "add",
                            "--detach",
                            path.to_string_lossy().as_ref(),
                            &oid,
                        ],
                        Duration::from_secs(30),
                    )?;
                    Ok::<String, String>(path.to_string_lossy().to_string())
                }
            }
        })
        .await
        .map_err(|error| format!("worktree materialization join failed: {error}"))??;
        let non_empty = NonEmptyString::new(worktree.clone())
            .map_err(|_| "materialized worktree path was empty".to_string())?;
        let present = manager
            .db()
            .mark_product_creation_resource_present(
                &job.request_id,
                &claimed.claim,
                &planned_path.to_string_lossy(),
                chrono::Utc::now(),
            )
            .await
            .map_err(|error| error.to_string())?;
        if !present {
            return Err("product creation claim was lost after materializing worktree".to_string());
        }
        (
            worktree.clone(),
            ConvMode::Explore {
                worktree_path: Some(non_empty),
                next_taskmd_id_hint: None,
            },
            phoenix_core::work_scope::AuthorityKind::RestrictedExplore,
            phoenix_core::work_scope::EnvironmentContext::AllocatedWorktree {
                cwd: worktree.clone(),
                worktree_path: worktree,
                branch_name: None,
                base_branch: Some(logical_base.clone()),
            },
            Some(logical_base),
            Some(repo_root),
            Some(oid),
        )
    } else {
        (
            cwd.clone(),
            ConvMode::Direct,
            phoenix_core::work_scope::AuthorityKind::Direct,
            phoenix_core::work_scope::EnvironmentContext::UnownedCwd { cwd },
            None,
            None,
            None,
        )
    };

    let product_id = job.product_conversation_id.clone();
    let conversation_id = uuid::Uuid::new_v4().to_string();
    let scope_id = phoenix_core::work_scope::WorkScopeId::new();
    let now = chrono::Utc::now();
    let conversation = crate::db::Conversation {
        id: conversation_id.clone(),
        product_conversation_id: product_id.clone(),
        slug: Some(generate_slug()),
        title: (!job.intent.objective.trim().is_empty())
            .then(|| title_from_text(&job.intent.objective)),
        cwd: effective_cwd,
        parent_conversation_id: None,
        user_initiated: true,
        state: ConvState::Idle,
        state_updated_at: now,
        created_at: now,
        updated_at: now,
        archived: true,
        model: job.intent.model.clone(),
        effort: job.intent.effort,
        service_tier: phoenix_core::domain::llm_types::ServiceTier::Standard,
        project_id: None,
        conv_mode,
        runtime_role: phoenix_core::work_scope::RuntimeRole::User,
        attached_work_scope_id: Some(scope_id),
        desired_base_branch: logical_base,
        message_count: 0,
        transcript_generation: 1,
        seed_parent_id: None,
        seed_label: None,
        continued_in_conv_id: None,
        chain_name: None,
        llm_language: job.intent.llm_language,
        spawned_from_conversation_id: None,
    };
    let repository_attachment = match staging_repo.zip(staging_oid) {
        Some((repository_root, exact_checkout_oid)) => {
            let git_common_dir = git_common_dir_for_repository_root(Path::new(&repository_root))?;
            let repository_id = None;
            Some(crate::db::ProductCreationRepositoryAttachment {
                repository_id,
                exact_checkout_oid,
                repository_root,
                git_common_dir,
            })
        }
        None => None,
    };
    let published = manager
        .db()
        .publish_product_creation_atomically(&crate::db::ProductCreationPublishInput {
            request_id: job.request_id.clone(),
            claim: claimed.claim.clone(),
            conversation,
            authority_kind,
            environment,
            repository_attachment,
        })
        .await
        .map_err(|error| error.to_string())?;
    if !published {
        return Err("product creation publication claim was lost".to_string());
    }
    let delivery = manager
        .db()
        .get_product_creation_job(&job.request_id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "published product creation job disappeared".to_string())?;
    let claimed = crate::db::ClaimedProductCreationJob {
        claim: claimed.claim.clone(),
        job: delivery,
    };
    deliver_product_creation_objective(manager, &claimed).await
}

#[allow(clippy::too_many_lines)]
async fn deliver_product_creation_objective(
    manager: &Arc<RuntimeManager>,
    claimed: &crate::db::ClaimedProductCreationJob,
) -> Result<PublishedProductCreation, String> {
    let job = &claimed.job;
    let product_id = job
        .published_product_id
        .clone()
        .ok_or_else(|| "delivery obligation lacked product id".to_string())?;
    let conversation_id = job
        .published_conversation_id
        .clone()
        .ok_or_else(|| "delivery obligation lacked conversation id".to_string())?;
    let steering_fingerprint = manager
        .db()
        .get_steering_acceptance_fingerprint(&conversation_id, &job.request_id)
        .await
        .map_err(|error| error.to_string())?;
    if let Some(exact_fingerprint) = steering_fingerprint.as_ref().filter(|fingerprint| {
        matches!(
            fingerprint,
            phoenix_db::SteeringAcceptanceFingerprint::Exact(_)
        )
    }) {
        if manager
            .db()
            .product_creation_objective_already_durably_accepted(
                &job.request_id,
                Some(exact_fingerprint),
                &job.intent,
            )
            .await
            .map_err(|error| error.to_string())?
        {
            manager
                .db()
                .complete_product_creation_delivery(
                    &job.request_id,
                    &claimed.claim,
                    exact_fingerprint,
                    &job.intent,
                )
                .await
                .map_err(|error| error.to_string())?;
            return Ok(PublishedProductCreation {
                product_conversation_id: product_id.to_string(),
                transcript_row_id: conversation_id,
            });
        }
    }
    let images = job
        .intent
        .images
        .iter()
        .map(|image| crate::db::ImageData {
            data: image.data.clone(),
            media_type: image.media_type.clone(),
        })
        .collect();
    let enqueue_result = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        manager.enqueue_steer_message(
            &conversation_id,
            Event::SteerMessage {
                text: job.intent.objective.clone(),
                llm_text: None,
                images,
                files: Vec::new(),
                message_id: job.request_id.clone(),
                user_agent: None,
                skill_invocation: None,
            },
            &format!("product-create:{}", job.request_id),
        ),
    )
    .await;
    if let Err(error) = enqueue_result {
        manager
            .db()
            .schedule_product_creation_delivery_retry(
                &job.request_id,
                &claimed.claim,
                chrono::Utc::now(),
            )
            .await
            .map_err(|db_error| {
                format!(
                    "delivery acceptance timed out; delivery retry persistence failed: {db_error}"
                )
            })?;
        manager.kick_creation_worker();
        return Err(format!(
            "product creation objective delivery timed out after 15 seconds: {error}"
        ));
    }
    if let Err(error) = enqueue_result.expect("checked timeout above") {
        manager
            .db()
            .schedule_product_creation_delivery_retry(
                &job.request_id,
                &claimed.claim,
                chrono::Utc::now(),
            )
            .await
            .map_err(|db_error| {
                format!("{error}; delivery retry persistence failed: {db_error}")
            })?;
        manager.kick_creation_worker();
        return Err(error.to_string());
    }
    manager
        .db()
        .complete_product_creation_delivery(
            &job.request_id,
            &claimed.claim,
            &crate::db::SteeringAcceptanceFingerprint::Exact(format!(
                "product-create:{}",
                job.request_id
            )),
            &job.intent,
        )
        .await
        .map_err(|error| error.to_string())?;
    Ok(PublishedProductCreation {
        product_conversation_id: product_id.to_string(),
        transcript_row_id: conversation_id,
    })
}

async fn cleanup_and_retry_unpublished_product_creation(
    manager: &Arc<RuntimeManager>,
    request_id: &str,
    claim: &crate::db::ProductCreationClaim,
    fallback_job: &crate::db::ProductCreationJobRecord,
    provisioning_error: &str,
) -> Result<(), String> {
    let current = manager
        .db()
        .get_product_creation_job(request_id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "product creation job disappeared after worker failure".to_string())?;
    if current.status != "claimed"
        || current.published_product_id.is_some()
        || current.published_conversation_id.is_some()
    {
        return Ok(());
    }
    let cleanup_job = if current.staging_path.is_some() {
        &current
    } else {
        fallback_job
    };
    let cleanup_lock = match cleanup_job.staging_repo_root.as_deref() {
        Some(repo_root) => {
            Some(RepositoryMutationLock::acquire(repo_root).map_err(|error| error.0)?)
        }
        None => None,
    };
    if !manager
        .db()
        .renew_product_creation_claim(
            request_id,
            claim,
            chrono::Utc::now(),
            chrono::Duration::seconds(30),
        )
        .await
        .map_err(|error| error.to_string())?
    {
        return Ok(());
    }
    let cleaned = cleanup_unpublished_product_staging(manager, cleanup_job, cleanup_lock).await;
    if !cleaned {
        return Ok(());
    }
    if let Some(resource_identity) = cleanup_job.staging_path.as_deref() {
        if !manager
            .db()
            .reset_product_creation_resource_after_cleanup(
                request_id,
                claim,
                resource_identity,
                chrono::Utc::now(),
            )
            .await
            .map_err(|error| error.to_string())?
        {
            return Ok(());
        }
    }
    manager
        .db()
        .schedule_product_creation_retry(request_id, claim, provisioning_error, chrono::Utc::now())
        .await
        .map_err(|error| error.to_string())?;
    manager.kick_creation_worker();
    Ok(())
}

async fn cleanup_unpublished_product_staging(
    manager: &Arc<RuntimeManager>,
    job: &crate::db::ProductCreationJobRecord,
    cleanup_lock: Option<RepositoryMutationLock>,
) -> bool {
    let cleanup = cleanup_unpublished_product_staging_path(
        job.staging_path.as_deref(),
        job.staging_repo_root.as_deref(),
        job.staging_exact_oid.as_deref(),
        cleanup_lock,
    )
    .await;
    if matches!(cleanup, Ok(Ok(true))) {
        return true;
    }
    manager
        .db()
        .mark_product_creation_cleanup_ambiguous(
            &job.request_id,
            &crate::db::ProductCreationClaim {
                worker_id: job.claim_worker_id.clone().unwrap_or_default(),
                token: job.claim_token.clone().unwrap_or_default(),
                generation: job.claim_generation,
                lease_until: job.claim_lease_until.unwrap_or_else(chrono::Utc::now),
            },
            chrono::Utc::now(),
        )
        .await
        .unwrap_or(false)
}

async fn cleanup_unpublished_product_staging_path(
    staging_path: Option<&str>,
    staging_repo_root: Option<&str>,
    staging_exact_oid: Option<&str>,
    cleanup_lock: Option<RepositoryMutationLock>,
) -> Result<Result<bool, String>, tokio::task::JoinError> {
    let (Some(path), Some(repo_root), Some(expected_oid)) = (
        staging_path.map(ToOwned::to_owned),
        staging_repo_root.map(ToOwned::to_owned),
        staging_exact_oid.map(ToOwned::to_owned),
    ) else {
        return Ok(Ok(true));
    };
    tokio::task::spawn_blocking(move || {
        let path = PathBuf::from(path);
        if !path.exists() {
            return Ok::<bool, String>(true);
        }
        let _lock = cleanup_lock.ok_or_else(|| "cleanup repository lock missing".to_string())?;
        let actual_oid = crate::git_ops::run_git(&path, &["rev-parse", "HEAD^{commit}"])?;
        let actual_root = crate::git_ops::run_git(&path, &["rev-parse", "--show-toplevel"])?;
        validate_worktree_belongs_to_repository(&repo_root, &path).map_err(|error| error.0)?;
        if actual_oid.trim() != expected_oid || Path::new(actual_root.trim()) != path {
            return Ok::<bool, String>(false);
        }
        crate::git_ops::run_git(
            Path::new(&repo_root),
            &[
                "worktree",
                "remove",
                "--force",
                path.to_string_lossy().as_ref(),
            ],
        )?;
        Ok(true)
    })
    .await
}

async fn reconcile_product_creation_cleanup(
    manager: &Arc<RuntimeManager>,
    cleanup: &crate::db::ProductCreationCleanupJob,
) -> Result<(), String> {
    for reservation in &cleanup.reservations {
        if reservation.status == "released" {
            continue;
        }
        let cleanup_lock = match RepositoryMutationLock::acquire(&reservation.repository_identity) {
            Ok(lock) => Some(lock),
            Err((_, _))
                if missing_repository_and_resource(
                    Path::new(&reservation.repository_identity),
                    Path::new(&reservation.resource_identity),
                ) =>
            {
                None
            }
            Err((message, _)) => return Err(message),
        };
        let cleaned = cleanup_unpublished_product_staging_path(
            Some(&reservation.resource_identity),
            Some(&reservation.repository_identity),
            cleanup.job.staging_exact_oid.as_deref(),
            cleanup_lock,
        )
        .await
        .map_err(|error| error.to_string())??;
        if !cleaned {
            manager
                .db()
                .mark_claimed_product_creation_cleanup_ambiguous(cleanup, chrono::Utc::now())
                .await
                .map_err(|error| error.to_string())?;
            return Ok(());
        }
        let released = manager
            .db()
            .release_product_creation_resource(cleanup, &reservation.id, chrono::Utc::now())
            .await
            .map_err(|error| error.to_string())?;
        if !released {
            return Ok(());
        }
    }
    if !manager
        .db()
        .finish_product_creation_cleanup(cleanup, chrono::Utc::now())
        .await
        .map_err(|error| error.to_string())?
    {
        return Ok(());
    }
    Ok(())
}

async fn process_product_creation_until_closed(
    manager: &Arc<RuntimeManager>,
    claimed: crate::db::ClaimedProductCreationJob,
) -> Result<PublishedProductCreation, String> {
    let request_id = claimed.job.request_id.clone();
    let claim = claimed.claim.clone();
    let mut fatal = manager.fatal_local_authority_receiver();
    let mut processing = std::pin::pin!(process_claimed_product_creation(manager, &claimed));
    let mut heartbeat = tokio::time::interval(Duration::from_secs(10));
    heartbeat.tick().await;
    loop {
        tokio::select! {
            result = &mut processing => return result,
            _ = crate::tls::wait_for_fatal_local_authority(&mut fatal) => {
                return Err("product creation stopped after fatal local authority closure".to_string());
            }
            _ = heartbeat.tick() => {
                let _admitted = manager.acquire_local_authority_pass().map_err(|()| {
                    "product creation claim renewal rejected after fatal local authority closure".to_string()
                })?;
                let renewed = manager.db().renew_product_creation_claim(
                    &request_id,
                    &claim,
                    chrono::Utc::now(),
                    chrono::Duration::seconds(30),
                ).await.map_err(|error| error.to_string())?;
                if !renewed {
                    let current = manager
                        .db()
                        .get_product_creation_job(&request_id)
                        .await
                        .map_err(|error| error.to_string())?;
                    if current.is_some_and(|job| {
                        job.status != "claimed"
                            && job.published_product_id.is_some()
                            && job.published_conversation_id.is_some()
                    }) {
                        return (&mut processing).await;
                    }
                    return Err("product creation claim was lost during heartbeat renewal".to_string());
                }
            }
        }
    }
}

fn run_git_with_timeout(
    repo_root: &Path,
    args: &[&str],
    timeout: Duration,
) -> Result<String, String> {
    crate::git_ops::run_git_bounded(repo_root, args, timeout)
}

fn discover_product_repository(cwd: &Path) -> Result<Option<String>, String> {
    match crate::git_ops::run_git(cwd, &["rev-parse", "--show-toplevel"]) {
        Ok(root) => Path::new(root.trim())
            .canonicalize()
            .map(|path| Some(path.to_string_lossy().to_string()))
            .map_err(|error| format!("could not canonicalize repository checkout root: {error}")),
        Err(error) if error.contains("not a git repository") => Ok(None),
        Err(error) => Err(format!("could not determine repository context: {error}")),
    }
}

fn git_common_dir_for_repository_root(repo_root: &Path) -> Result<String, String> {
    crate::git_ops::run_git(
        repo_root,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .map(|value| value.trim().to_string())
}

fn ensure_phoenix_staging_ignored(repo_root: &Path) -> Result<(), String> {
    let common_dir = crate::git_ops::run_git(repo_root, &["rev-parse", "--git-common-dir"])?;
    let common_dir = PathBuf::from(common_dir.trim());
    let common_dir = if common_dir.is_absolute() {
        common_dir
    } else {
        repo_root.join(common_dir)
    };
    let exclude_path = common_dir.join("info").join("exclude");
    std::fs::create_dir_all(
        exclude_path
            .parent()
            .ok_or_else(|| "git exclude path had no parent".to_string())?,
    )
    .map_err(|error| error.to_string())?;
    let existing = std::fs::read_to_string(&exclude_path).unwrap_or_default();
    if !existing.lines().any(|line| line.trim() == "/.phoenix/") {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&exclude_path)
            .map_err(|error| error.to_string())?;
        writeln!(file, "/.phoenix/").map_err(|error| error.to_string())?;
    }
    let gitignore = repo_root.join(".gitignore");
    let mut contents = std::fs::read_to_string(&gitignore).unwrap_or_default();
    if !contents
        .lines()
        .any(|line| line.trim() == ".phoenix/worktrees/")
    {
        if !contents.is_empty() && !contents.ends_with('\n') {
            contents.push('\n');
        }
        contents.push_str(".phoenix/worktrees/\n");
        std::fs::write(&gitignore, contents).map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn strict_product_creation_pin(repo_root: &Path) -> Result<(String, String), String> {
    let _lock =
        RepositoryMutationLock::acquire(repo_root.to_string_lossy().as_ref()).map_err(|e| e.0)?;
    let remotes = crate::git_ops::run_git(repo_root, &["remote"])
        .map_err(|error| format!("could not inspect configured remotes: {error}"))?;
    if remotes.lines().any(|remote| remote.trim() == "origin") {
        let stdout = run_git_with_timeout(
            repo_root,
            &["ls-remote", "--symref", "origin", "HEAD"],
            Duration::from_secs(30),
        )
        .map_err(|error| format!("could not discover origin default: {error}"))?;
        let branch = stdout
            .lines()
            .find_map(|line| line.strip_prefix("ref: refs/heads/"))
            .and_then(|line| line.split_whitespace().next())
            .ok_or_else(|| {
                "origin did not advertise an authoritative default branch".to_string()
            })?;
        run_git_with_timeout(
            repo_root,
            &[
                "fetch",
                "origin",
                "--no-tags",
                &format!("+refs/heads/{branch}:refs/remotes/origin/{branch}"),
            ],
            Duration::from_secs(30),
        )?;
        let oid = crate::git_ops::run_git(
            repo_root,
            &[
                "rev-parse",
                &format!("refs/remotes/origin/{branch}^{{commit}}"),
            ],
        )?;
        return Ok((oid.trim().to_string(), branch.to_string()));
    }
    let oid = crate::git_ops::run_git(repo_root, &["rev-parse", "refs/heads/main^{commit}"])
        .map_err(|_| "repository has no origin and no local refs/heads/main".to_string())?;
    Ok((oid.trim().to_string(), "main".to_string()))
}

fn materialize_approved_task_snapshot(
    worktree_path: &Path,
    snapshot: &phoenix_core::task_handoff::ApprovedTaskSnapshot,
) -> Result<(), String> {
    let relative = Path::new(&snapshot.task_file);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(
            "approved task artifact path must stay within the detached worktree".to_string(),
        );
    }
    let filename = relative
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "approved task artifact path lacked a filename".to_string())?;
    let promoted_filename = taskmd_core::filename::parse_filename(filename).map_or_else(
        || filename.to_string(),
        |parsed| {
            taskmd_core::filename::format_filename(
                &parsed.id,
                parsed.priority,
                taskmd_core::constants::Status::InProgress,
                &parsed.slug,
            )
        },
    );
    let parent = relative.parent().unwrap_or_else(|| Path::new(""));
    let canonical_worktree = worktree_path
        .canonicalize()
        .map_err(|error| format!("failed to canonicalize detached worktree: {error}"))?;
    let target_parent = canonical_worktree.join(parent);
    let mut verified_parent = canonical_worktree.clone();
    for component in parent.components() {
        verified_parent.push(component);
        if verified_parent.exists() {
            if std::fs::symlink_metadata(&verified_parent)
                .map_err(|error| format!("failed to inspect approved task parent: {error}"))?
                .file_type()
                .is_symlink()
            {
                return Err("approved task artifact parent must not contain symlinks".to_string());
            }
        } else {
            std::fs::create_dir(&verified_parent).map_err(|error| {
                format!("failed to create approved task artifact parent: {error}")
            })?;
        }
    }
    let canonical_parent = target_parent
        .canonicalize()
        .map_err(|error| format!("failed to canonicalize approved task parent: {error}"))?;
    if !canonical_parent.starts_with(&canonical_worktree) {
        return Err("approved task artifact parent escaped the detached worktree".to_string());
    }
    let source_path = canonical_parent.join(filename);
    let path = canonical_parent.join(promoted_filename);
    if path
        .symlink_metadata()
        .is_ok_and(|metadata| metadata.file_type().is_symlink())
    {
        return Err("approved task artifact target must not be a symlink".to_string());
    }
    if source_path != path && source_path.exists() {
        std::fs::remove_file(&source_path)
            .map_err(|error| format!("failed to remove superseded task artifact: {error}"))?;
    }
    std::fs::write(&path, &snapshot.artifact_body)
        .map_err(|error| format!("failed to materialize approved task artifact: {error}"))?;
    Ok(())
}

pub(crate) fn resolve_creation_model(
    registry: &ModelRegistry,
    explicit_model: Option<&str>,
    requested_mode: &str,
    repo_present: bool,
) -> String {
    if let Some(model) = explicit_model {
        return registry.resolve_model_id(model);
    }

    let registry_default = registry.default_model_id();
    if requested_mode == "managed" || (requested_mode == "auto" && repo_present) {
        registry.cheap_model_id_for_provider(&registry_default)
    } else {
        registry_default.clone()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CreationDrainControl {
    Continue,
    StopDrain,
}

async fn drain_claimed_jobs<Admission, Guard, Job, Claim, ClaimFuture, Process, ProcessFuture>(
    mut admit: Admission,
    mut claim: Claim,
    mut process: Process,
) -> Result<(), String>
where
    Admission: FnMut() -> Result<Guard, ()>,
    Claim: FnMut() -> ClaimFuture,
    ClaimFuture: std::future::Future<Output = Result<Option<Job>, String>>,
    Process: FnMut(Job) -> ProcessFuture,
    ProcessFuture: std::future::Future<Output = CreationDrainControl>,
{
    loop {
        let Ok(owner) = admit() else {
            return Ok(());
        };
        let job = claim().await?;
        drop(owner);
        let Some(job) = job else {
            return Ok(());
        };
        if matches!(process(job).await, CreationDrainControl::StopDrain) {
            return Ok(());
        }
    }
}

pub(crate) async fn run_admitted_blocking<R, F>(
    admitted: crate::runtime::AdmittedOperation,
    operation: F,
) -> Result<R, tokio::task::JoinError>
where
    R: Send + 'static,
    F: FnOnce() -> R + Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        let _admitted = admitted;
        operation()
    })
    .await
}

#[allow(clippy::too_many_lines)]
pub(crate) async fn drain_pending_jobs(manager: &Arc<RuntimeManager>) -> Result<(), String> {
    let worker_id = CreationWorkerId(format!("creation-worker-{}", uuid::Uuid::new_v4()));
    let Ok(_product_authority) = manager.acquire_local_authority_pass() else {
        return Ok(());
    };
    while let Some(delivery) = manager
        .db()
        .claim_next_product_creation_delivery(
            &worker_id.0,
            &uuid::Uuid::new_v4().to_string(),
            chrono::Utc::now(),
            chrono::Duration::seconds(30),
        )
        .await
        .map_err(|error| error.to_string())?
    {
        if deliver_product_creation_objective(manager, &delivery)
            .await
            .is_err()
        {
            break;
        }
    }
    loop {
        let Some(claimed) = manager
            .db()
            .claim_next_product_creation(
                &worker_id.0,
                &uuid::Uuid::new_v4().to_string(),
                chrono::Utc::now(),
                chrono::Duration::seconds(30),
            )
            .await
            .map_err(|error| error.to_string())?
        else {
            break;
        };
        if let Err(error) = process_product_creation_until_closed(manager, claimed.clone()).await {
            cleanup_and_retry_unpublished_product_creation(
                manager,
                &claimed.job.request_id,
                &claimed.claim,
                &claimed.job,
                &error,
            )
            .await?;
        }
    }
    loop {
        let Some(cleanup) = manager
            .db()
            .claim_next_product_creation_cleanup(
                &worker_id.0,
                &uuid::Uuid::new_v4().to_string(),
                chrono::Utc::now(),
                chrono::Duration::seconds(30),
            )
            .await
            .map_err(|error| error.to_string())?
        else {
            break;
        };
        let _owner = manager.acquire_local_authority_pass().map_err(|()| {
            "product creation cleanup rejected after fatal local authority closure".to_string()
        })?;
        if let Err(error) = reconcile_product_creation_cleanup(manager, &cleanup).await {
            tracing::warn!(request_id = %cleanup.job.request_id, error = %error, "product creation cleanup will retry");
            manager
                .db()
                .schedule_product_creation_cleanup_retry(
                    &cleanup,
                    chrono::Utc::now() + chrono::Duration::seconds(30),
                )
                .await
                .map_err(|db_error| db_error.to_string())?;
        }
    }
    loop {
        let Ok(_owner) = manager.acquire_local_authority_pass() else {
            return Ok(());
        };
        let Some(cleanup) = manager
            .db()
            .claim_next_conversation_creation_cleanup(
                &worker_id.0,
                &uuid::Uuid::new_v4().to_string(),
                chrono::Utc::now(),
                chrono::Duration::seconds(30),
            )
            .await
            .map_err(|error| error.to_string())?
        else {
            break;
        };
        if let Err(error) = reconcile_creation_cleanup(manager, &cleanup).await {
            tracing::warn!(job_id = %cleanup.job_id, error = %error, "conversation creation cleanup will retry");
            let _admitted = manager.acquire_local_authority_pass().map_err(|()| {
                "creation cleanup retry rejected after fatal local authority closure".to_string()
            })?;
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
    drain_claimed_jobs(
        || manager.acquire_local_authority_pass(),
        || {
            let manager = Arc::clone(manager);
            let worker_id = worker_id.clone();
            async move {
                let token = CreationClaimToken(uuid::Uuid::new_v4().to_string());
                match manager
                    .db()
                    .claim_next_conversation_creation_job(
                        &worker_id,
                        &token,
                        chrono::Utc::now(),
                        chrono::Duration::seconds(30),
                    )
                    .await
                    .map_err(|error| error.to_string())?
                {
                    CreationClaimOutcome::Claimed(job) => Ok(Some(*job)),
                    CreationClaimOutcome::NoEligibleJob => Ok(None),
                }
            }
        },
        |job| {
            let manager = Arc::clone(manager);
            async move {
                match process_claimed_job(&manager, job).await {
                    Ok(control) => control,
                    Err(error) => {
                        tracing::error!(error = %error, "conversation creation job processing failed");
                        CreationDrainControl::Continue
                    }
                }
            }
        },
    )
    .await
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
        let cleanup_admission = manager.acquire_local_authority_pass().map_err(|()| {
            "creation cleanup mutation rejected after fatal local authority closure".to_string()
        })?;
        run_admitted_blocking(cleanup_admission, move || -> Result<(), String> {
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
        let _admitted = manager.acquire_local_authority_pass().map_err(|()| {
            "creation resource release rejected after fatal local authority closure".to_string()
        })?;
        let outcome = manager
            .db()
            .release_creation_resource(cleanup, &reservation.id, chrono::Utc::now())
            .await
            .map_err(|error| error.to_string())?;
        if matches!(outcome, crate::db::CreationCasOutcome::ClaimLost) {
            return Ok(());
        }
    }
    let _admitted = manager.acquire_local_authority_pass().map_err(|()| {
        "creation cleanup completion rejected after fatal local authority closure".to_string()
    })?;
    manager
        .db()
        .finish_conversation_creation_cleanup(cleanup, chrono::Utc::now())
        .await
        .map_err(|error| error.to_string())?;
    manager.resume_pending_close_settlements().await?;
    Ok(())
}

async fn renew_creation_claim_with_admission(
    manager: &Arc<RuntimeManager>,
    job_id: &str,
    claim: &phoenix_core::domain::creation_protocol::CreationClaim,
) -> Result<crate::db::CreationCasOutcome, String> {
    let _admitted = manager.acquire_local_authority_pass().map_err(|()| {
        "creation claim renewal rejected after fatal local authority closure".to_string()
    })?;
    manager
        .db()
        .renew_conversation_creation_claim(
            job_id,
            claim,
            chrono::Utc::now(),
            chrono::Duration::seconds(30),
        )
        .await
        .map_err(|error| error.to_string())
}

async fn process_claimed_job_until_closed<
    Process,
    ProcessFuture,
    Renew,
    RenewFuture,
    Closure,
    ClosureFuture,
>(
    processing: Process,
    mut renew: Renew,
    closure: Closure,
) -> Result<CreationDrainControl, String>
where
    Process: FnOnce() -> ProcessFuture,
    ProcessFuture: std::future::Future<Output = Result<CreationDrainControl, String>>,
    Renew: FnMut() -> RenewFuture,
    RenewFuture: std::future::Future<Output = Result<crate::db::CreationCasOutcome, String>>,
    Closure: FnOnce() -> ClosureFuture,
    ClosureFuture: std::future::Future<Output = ()>,
{
    let mut processing = std::pin::pin!(processing());
    let mut heartbeat = tokio::time::interval(std::time::Duration::from_secs(10));
    heartbeat.tick().await;
    let closure = closure();
    tokio::pin!(closure);
    loop {
        tokio::select! {
            result = &mut processing => return result,
            () = &mut closure => return Ok(CreationDrainControl::StopDrain),
            _ = heartbeat.tick() => {
                if matches!(renew().await?, crate::db::CreationCasOutcome::ClaimLost) {
                    return Ok(CreationDrainControl::StopDrain);
                }
            }
        }
    }
}

async fn process_claimed_job(
    manager: &Arc<RuntimeManager>,
    job: crate::db::ConversationCreationJob,
) -> Result<CreationDrainControl, String> {
    let CreationStatus::Claimed(claim) = job.protocol.status.clone() else {
        return Err("claimed creation job lacked claim authority".to_string());
    };
    let job_id = job.id.clone();
    let mut fatal = manager.fatal_local_authority_receiver();
    process_claimed_job_until_closed(
        || process_job(manager, job),
        || renew_creation_claim_with_admission(manager, &job_id, &claim),
        || async move {
            crate::tls::wait_for_fatal_local_authority(&mut fatal).await;
        },
    )
    .await
}

#[allow(clippy::too_many_lines)]
async fn process_job(
    manager: &Arc<RuntimeManager>,
    mut job: crate::db::ConversationCreationJob,
) -> Result<CreationDrainControl, String> {
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
            let _admitted = manager.acquire_local_authority_pass().map_err(|()| {
                "creation completion rejected after fatal local authority closure".to_string()
            })?;
            let outcome = manager
                .db()
                .complete_conversation_creation_job(&job.id, &claim, chrono::Utc::now())
                .await
                .map_err(|e| e.to_string())?;
            if matches!(outcome, crate::db::CreationCasOutcome::ClaimLost) {
                tracing::debug!(job_id = %job.id, generation = claim.generation, "creation completion rejected after claim loss");
            }
            return Ok(CreationDrainControl::Continue);
        }
        if matches!(conversation.state, ConvState::LlmRequesting { .. }) {
            tracing::info!(job_id = %job.id, message_id = ?job.message_id, "starting creation runtime to resume the persisted LLM request");
            let _ = manager.get_or_create(&conv_id).await?;
            return Ok(CreationDrainControl::Continue);
        }
        tracing::info!(job_id = %job.id, message_id = ?job.message_id, "replaying creation bootstrap after message persisted without state advancement");
    }

    match provision_conversation(manager, &mut job).await {
        Ok(ProvisionOutcome::SeededEmpty | ProvisionOutcome::InitialMessageSubmitted) => {
            Ok(CreationDrainControl::Continue)
        }
        Err(CreationProvisionError::FatalAuthorityDeferred) => {
            tracing::info!(job_id = %job.id, "leaving admitted creation job recoverable after fatal authority closure");
            Ok(CreationDrainControl::StopDrain)
        }
        Err(CreationProvisionError::Failed(message, kind)) => {
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
                let _admitted = manager.acquire_local_authority_pass().map_err(|()| {
                    "creation retry rejected after fatal local authority closure".to_string()
                })?;
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
                return Ok(CreationDrainControl::Continue);
            }
            let failed = ConvState::CreationFailed {
                job_id: job.id.clone(),
                error: message.clone(),
                error_kind: kind.clone(),
            };
            let mut admitted = manager.acquire_local_authority_pass().map_err(|()| {
                "creation failure commit rejected after fatal local authority closure".to_string()
            })?;
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
                return Ok(CreationDrainControl::Continue);
            }
            let broadcast_tx = manager.conversation_broadcaster(&conv_id).await;
            let _ = broadcast_tx
                .admitted_publication(&mut admitted)
                .state_change(
                    failed.clone(),
                    failed.presentation_mode().to_string(),
                    chrono::Utc::now(),
                );
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

enum CreationProvisionError {
    Failed(String, ErrorKind),
    FatalAuthorityDeferred,
}

fn creation_admission_error(message: &str) -> CreationProvisionError {
    CreationProvisionError::Failed(message.to_string(), ErrorKind::Cancelled)
}

fn acquire_creation_admission(
    manager: &Arc<RuntimeManager>,
    message: &str,
) -> Result<crate::runtime::AdmittedOperation, CreationProvisionError> {
    manager
        .acquire_local_authority_pass()
        .map_err(|()| creation_admission_error(message))
}

impl From<(String, ErrorKind)> for CreationProvisionError {
    fn from(error: (String, ErrorKind)) -> Self {
        Self::Failed(error.0, error.1)
    }
}

enum ProvisionOutcome {
    SeededEmpty,
    InitialMessageSubmitted,
}

#[allow(clippy::too_many_lines)]
async fn provision_conversation(
    manager: &Arc<RuntimeManager>,
    job: &mut crate::db::ConversationCreationJob,
) -> Result<ProvisionOutcome, CreationProvisionError> {
    let intent = job.intent.clone();
    let CreationStatus::Claimed(claim) = job.protocol.status.clone() else {
        return Err((
            "creation provisioning lacks claim authority".to_string(),
            ErrorKind::ServerError,
        )
            .into());
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
    let approved_task_creation = requested_mode == "approved_task";

    let mut resolved_model = resolve_creation_model(
        &manager.llm_registry,
        intent.model.as_deref(),
        requested_mode,
        repo_root.is_some(),
    );

    let mut conv_mode = ConvMode::Direct;
    let mut effective_cwd = initial_cwd.clone();
    let mut project_id = None;
    let mut desired_base_branch = intent.base_branch.clone();

    if let Some(repo_root) = repo_root.clone() {
        let _admitted = acquire_creation_admission(
            manager,
            "creation project association rejected after fatal local authority closure",
        )?;
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
        "branch" | "approved_task" => {
            let repo_root = repo_root.clone().ok_or_else(|| {
                (
                    "Branch mode requires a git repository".to_string(),
                    ErrorKind::InvalidRequest,
                )
            })?;
            let approved_snapshot = if requested_mode == "approved_task" {
                Some(intent.approved_task.clone().ok_or_else(|| {
                    (
                        "Approved-task creation requires the reviewed task artifact snapshot"
                            .to_string(),
                        ErrorKind::InvalidRequest,
                    )
                })?)
            } else {
                None
            };
            let (branch_name, approved_commit_oid, approved_base_branch) = if let Some(snapshot) =
                approved_snapshot.as_ref()
            {
                let (oid, base_branch) = if let Some(pin) = job.starting_pin.as_ref() {
                    (pin.exact_checkout_oid.clone(), pin.logical_base.clone())
                } else {
                    run_admitted_blocking(
                        acquire_creation_admission(
                            manager,
                            "approved-task pin resolution rejected after fatal local authority closure",
                        )?,
                        {
                            let repo_root = repo_root.clone();
                            move || strict_product_creation_pin(Path::new(&repo_root))
                        },
                    )
                    .await
                    .map_err(|error| {
                        (
                            format!("approved-task pin resolution join failed: {error}"),
                            ErrorKind::ServerError,
                        )
                    })?
                    .map_err(|error| (error, ErrorKind::ServerError))?
                };
                (
                    format!(
                        "task-{}-{}",
                        snapshot.task_id,
                        slugify_label(&snapshot.task_title)
                    ),
                    Some(oid),
                    Some(base_branch),
                )
            } else {
                let branch_name = desired_base_branch.clone().ok_or_else(|| {
                    (
                        "Branch mode requires base_branch naming the existing branch".to_string(),
                        ErrorKind::InvalidRequest,
                    )
                })?;
                validate_user_ref(&branch_name).map_err(app_error_to_kind)?;
                (branch_name, None, None)
            };
            if let Some(oid) = approved_commit_oid.as_deref() {
                if !manager
                    .db()
                    .persist_conversation_creation_checkout_pin(
                        &job.conversation_id,
                        &claim,
                        oid,
                        approved_base_branch
                            .as_deref()
                            .expect("approved-task pin includes its logical base"),
                    )
                    .await
                    .map_err(|error| (error.to_string(), ErrorKind::ServerError))?
                {
                    return Err(CreationProvisionError::Failed(
                        "approved-task starting pin lost its provisioning claim".to_string(),
                        ErrorKind::ServerError,
                    ));
                }
            }
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
            let approved_task_snapshot = approved_snapshot.clone();
            let approved_commit_oid_for_blocking = approved_commit_oid.clone();
            let approved_base_branch_for_blocking = approved_base_branch.clone();
            let worktree_admission = acquire_creation_admission(
                manager,
                "branch worktree mutation rejected after fatal local authority closure",
            )?;
            let info = run_admitted_blocking(worktree_admission, move || {
                let _lock = RepositoryMutationLock::acquire(&repo_for_blocking)?;
                if reconcile_owned_worktree_path(&repo_for_blocking, &path_for_blocking)? {
                    if approved_task_creation {
                        validate_worktree_belongs_to_repository(
                            &repo_for_blocking,
                            &path_for_blocking,
                        )?;
                        validate_detached_task_worktree(
                            &path_for_blocking,
                            approved_commit_oid_for_blocking
                                .as_ref()
                                .expect("approved_task mode has strict pin")
                                .as_str(),
                        )
                        .map_err(|error| (error, ErrorKind::InvalidRequest))?;
                        materialize_approved_task_snapshot(
                            &path_for_blocking,
                            approved_task_snapshot
                                .as_ref()
                                .expect("approved_task mode has reviewed snapshot"),
                        )
                        .map_err(|error| (error, ErrorKind::InvalidRequest))?;
                    } else {
                        validate_worktree_branch(&path_for_blocking, &branch_name)?;
                    }
                    let worktree_path = path_for_blocking.to_string_lossy().to_string();
                    let base_branch = if approved_task_creation {
                        approved_base_branch_for_blocking
                            .clone()
                            .expect("approved_task mode has strict base branch")
                    } else {
                        crate::git_ops::run_git(
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
                        .unwrap_or_else(|| branch_name.clone())
                    };
                    Ok(BranchWorktreeInfo {
                        branch_name,
                        worktree_path,
                        base_branch,
                    })
                } else if approved_task_creation {
                    let worktree_path = create_detached_task_worktree_blocking(
                        &repo_for_blocking,
                        &path_for_blocking,
                        approved_commit_oid_for_blocking
                            .as_ref()
                            .expect("approved_task mode has strict pin")
                            .as_str(),
                    )
                    .map_err(branch_worktree_error_to_kind)?;
                    materialize_approved_task_snapshot(
                        Path::new(&worktree_path),
                        approved_task_snapshot
                            .as_ref()
                            .expect("approved_task mode has reviewed snapshot"),
                    )
                    .map_err(|error| (error, ErrorKind::InvalidRequest))?;
                    Ok(BranchWorktreeInfo {
                        branch_name,
                        worktree_path,
                        base_branch: approved_base_branch_for_blocking
                            .expect("approved_task mode has strict base branch"),
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
            conv_mode = if let Some(snapshot) = intent.approved_task.as_ref() {
                ConvMode::DetachedApprovedTask {
                    worktree_path: NonEmptyString::new(info.worktree_path)
                        .map_err(|_| ("empty worktree path".to_string(), ErrorKind::ServerError))?,
                    base_branch: NonEmptyString::new(info.base_branch)
                        .map_err(|_| ("empty base branch".to_string(), ErrorKind::ServerError))?,
                    task_id: NonEmptyString::new(snapshot.task_id.clone())
                        .map_err(|_| ("empty task id".to_string(), ErrorKind::ServerError))?,
                    task_title: NonEmptyString::new(snapshot.task_title.clone())
                        .map_err(|_| ("empty task title".to_string(), ErrorKind::ServerError))?,
                }
            } else {
                ConvMode::Branch {
                    branch_name: NonEmptyString::new(info.branch_name)
                        .map_err(|_| ("empty branch name".to_string(), ErrorKind::ServerError))?,
                    worktree_path: NonEmptyString::new(info.worktree_path)
                        .map_err(|_| ("empty worktree path".to_string(), ErrorKind::ServerError))?,
                    base_branch: NonEmptyString::new(info.base_branch)
                        .map_err(|_| ("empty base branch".to_string(), ErrorKind::ServerError))?,
                }
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
            let worktree_admission = acquire_creation_admission(
                manager,
                "managed worktree mutation rejected after fatal local authority closure",
            )?;
            let worktree = run_admitted_blocking(worktree_admission, move || {
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

            resolved_model = resolve_creation_model(
                &manager.llm_registry,
                intent.model.as_deref(),
                requested_mode,
                true,
            );
        }
        other => {
            return Err((
                format!("Invalid mode '{other}'. Expected one of: direct, managed, branch, auto"),
                ErrorKind::InvalidRequest,
            )
                .into());
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
        if let Some((cheap_model_id, cheap_model)) =
            manager.model_registry().get_cheap_model_with_id()
        {
            let effective_effort = manager
                .model_registry()
                .effective_effort(&cheap_model_id, None);
            crate::title_generator::generate_title(
                &title_source,
                cheap_model,
                effective_effort,
                manager.model_registry().output_token_limit(&cheap_model_id),
            )
            .await
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

    if let Some(effort) = intent.effort {
        if !manager
            .llm_registry
            .supports_effort(&resolved_model, effort)
        {
            return Err((
                format!("Effort '{effort}' is not supported by resolved model '{resolved_model}'"),
                ErrorKind::InvalidRequest,
            )
                .into());
        }
    }

    let mut metadata_admission = acquire_creation_admission(
        manager,
        "creation metadata publication rejected after fatal local authority closure",
    )?;
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
            )
                .into());
        }
        job.protocol.stage = phoenix_core::domain::creation_protocol::CreationStage::CommitMetadata;
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
        let _ = broadcast_tx
            .admitted_publication(&mut metadata_admission)
            .event(|seq| SseEvent::ConversationUpdate {
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
                        crate::work_scope::ResourceScopeKey::Work(
                            persisted_conversation
                                .attached_work_scope_id
                                .clone()
                                .expect("persisted conversation has work scope"),
                        )
                        .stable_key(),
                    ),
                    model: Some(resolved_model.clone()),
                },
            });
    }

    if seeded_empty {
        let mut admitted = acquire_creation_admission(
            manager,
            "seeded creation completion rejected after fatal local authority closure",
        )?;
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
            )
                .into());
        }
        if let Some(broadcast_tx) = {
            let runtimes = manager.runtimes.read().await;
            runtimes
                .get(&job.conversation_id)
                .map(|h| h.broadcast_tx.clone())
        } {
            let idle = ConvState::Idle;
            let _ = broadcast_tx
                .admitted_publication(&mut admitted)
                .state_change(
                    idle.clone(),
                    idle.presentation_mode().to_string(),
                    chrono::Utc::now(),
                );
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
    let authority = renew_creation_claim_with_admission(manager, &job.id, &claim)
        .await
        .map_err(|error| (error, ErrorKind::ServerError))?;
    if matches!(authority, crate::db::CreationCasOutcome::ClaimLost) {
        return Err((
            "creation claim was lost before runtime bootstrap settlement".to_string(),
            ErrorKind::Cancelled,
        )
            .into());
    }
    let conversation_id = job.conversation_id.clone();
    let delivery = checkpoint_then_deliver_creation(
        || {
            checkpoint_creation_stage(
                manager,
                job,
                &claim,
                phoenix_core::domain::creation_protocol::CreationStage::Finalize,
            )
        },
        || manager.send_event(&conversation_id, event),
    )
    .await;
    classify_creation_delivery(delivery, manager.local_authority_is_closed())?;
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

async fn checkpoint_then_deliver_creation<Checkpoint, CheckpointFuture, Deliver, DeliverFuture>(
    checkpoint: Checkpoint,
    deliver: Deliver,
) -> Result<(), (String, ErrorKind)>
where
    Checkpoint: FnOnce() -> CheckpointFuture,
    CheckpointFuture: std::future::Future<Output = Result<(), (String, ErrorKind)>>,
    Deliver: FnOnce() -> DeliverFuture,
    DeliverFuture: std::future::Future<Output = Result<(), String>>,
{
    checkpoint().await?;
    deliver().await.map_err(|error| {
        (
            format!("creation runtime event did not settle: {error}"),
            ErrorKind::ServerError,
        )
    })
}

fn classify_creation_delivery(
    delivery: Result<(), (String, ErrorKind)>,
    fatal_authority_closed: bool,
) -> Result<(), CreationProvisionError> {
    match delivery {
        Ok(()) => Ok(()),
        Err(_) if fatal_authority_closed => Err(CreationProvisionError::FatalAuthorityDeferred),
        Err(error) => Err(error.into()),
    }
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
        let _admitted = manager.acquire_local_authority_pass().map_err(|()| {
            (
                "creation stage commit rejected after fatal local authority closure".to_string(),
                ErrorKind::Cancelled,
            )
        })?;
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
    let _admitted = manager.acquire_local_authority_pass().map_err(|()| {
        (
            "creation resource reservation rejected after fatal local authority closure"
                .to_string(),
            ErrorKind::Cancelled,
        )
    })?;
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
    Ok(())
}

async fn mark_worktree_present(
    manager: &Arc<RuntimeManager>,
    job: &crate::db::ConversationCreationJob,
    claim: &phoenix_core::domain::creation_protocol::CreationClaim,
    worktree_path: &Path,
) -> Result<(), (String, ErrorKind)> {
    let _admitted = manager.acquire_local_authority_pass().map_err(|()| {
        (
            "creation resource commit rejected after fatal local authority closure".to_string(),
            ErrorKind::Cancelled,
        )
    })?;
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
                effort: None,
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
                approved_task: None,
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
mod runtime_bootstrap_settlement_tests {
    use super::*;

    #[tokio::test]
    async fn runtime_delivery_waits_for_finalize_checkpoint() {
        let (checkpoint_started_tx, checkpoint_started_rx) = tokio::sync::oneshot::channel();
        let (release_checkpoint_tx, release_checkpoint_rx) = tokio::sync::oneshot::channel();
        let (delivery_started_tx, mut delivery_started_rx) = tokio::sync::oneshot::channel();

        let settlement = checkpoint_then_deliver_creation(
            || async move {
                checkpoint_started_tx.send(()).unwrap();
                release_checkpoint_rx.await.unwrap();
                Ok(())
            },
            || async move {
                delivery_started_tx.send(()).unwrap();
                Ok(())
            },
        );
        tokio::pin!(settlement);

        tokio::select! {
            result = checkpoint_started_rx => result.unwrap(),
            result = &mut settlement => panic!("settlement completed before checkpoint gate: {result:?}"),
        }
        assert!(matches!(
            delivery_started_rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));

        release_checkpoint_tx.send(()).unwrap();
        settlement.await.unwrap();
        delivery_started_rx.await.unwrap();
    }

    #[tokio::test]
    async fn fatal_closure_between_finalize_and_delivery_defers_creation_job() {
        let fatal_closed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let close_after_checkpoint = Arc::clone(&fatal_closed);
        let observe_closed_delivery = Arc::clone(&fatal_closed);

        let delivery = checkpoint_then_deliver_creation(
            || async move {
                close_after_checkpoint.store(true, std::sync::atomic::Ordering::Release);
                Ok(())
            },
            || async move {
                assert!(observe_closed_delivery.load(std::sync::atomic::Ordering::Acquire));
                Err("runtime admission closed after fatal local authority loss".to_string())
            },
        )
        .await;

        assert!(matches!(
            classify_creation_delivery(
                delivery,
                fatal_closed.load(std::sync::atomic::Ordering::Acquire)
            ),
            Err(CreationProvisionError::FatalAuthorityDeferred)
        ));
    }

    #[tokio::test]
    async fn blocking_worktree_operation_retains_owner_after_awaiter_is_cancelled() {
        let fence = crate::runtime::FatalLocalAuthorityFence::new();
        let admitted = fence
            .try_acquire()
            .expect("admit blocking worktree operation");
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let awaiting = tokio::spawn(run_admitted_blocking(admitted, move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        }));
        tokio::task::spawn_blocking(move || started_rx.recv().unwrap())
            .await
            .unwrap();

        awaiting.abort();
        let _ = awaiting.await;
        fence.close("test_blocking_worktree_operation");
        assert_eq!(fence.owners_at_first_close(), Some(1));
        assert_eq!(fence.owner_count(), 1);

        release_tx.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(10), fence.wait_for_owners())
            .await
            .expect("blocking worktree owner drains after operation completes");
        assert_eq!(fence.owner_count(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn closure_during_blocked_creation_stops_heartbeat_and_stage_work() {
        let (process_started_tx, process_started_rx) = tokio::sync::oneshot::channel();
        let (_release_process_tx, release_process_rx) = tokio::sync::oneshot::channel::<()>();
        let renewals = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed_renewals = Arc::clone(&renewals);
        let (close_tx, close_rx) = tokio::sync::oneshot::channel();

        let worker = tokio::spawn(process_claimed_job_until_closed(
            || async move {
                process_started_tx.send(()).unwrap();
                let _ = release_process_rx.await;
                panic!("blocked creation stage resumed after fatal closure");
            },
            move || {
                observed_renewals.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                async { Ok(crate::db::CreationCasOutcome::Applied) }
            },
            || async move {
                close_rx.await.unwrap();
            },
        ));
        process_started_rx.await.unwrap();

        tokio::time::advance(std::time::Duration::from_secs(10)).await;
        tokio::task::yield_now().await;
        assert_eq!(renewals.load(std::sync::atomic::Ordering::Acquire), 1);

        close_tx.send(()).unwrap();
        assert!(matches!(
            worker.await.unwrap().unwrap(),
            CreationDrainControl::StopDrain
        ));
        tokio::time::advance(std::time::Duration::from_secs(30)).await;
        assert_eq!(renewals.load(std::sync::atomic::Ordering::Acquire), 1);
    }

    #[tokio::test]
    async fn fatal_deferral_stops_drain_before_second_job_claim() {
        let claims = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed_claims = Arc::clone(&claims);
        let processed = Arc::new(std::sync::Mutex::new(Vec::new()));
        let observed_processed = Arc::clone(&processed);

        drain_claimed_jobs(
            || Ok(()),
            move || {
                let claim_number =
                    observed_claims.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                async move {
                    match claim_number {
                        0 => Ok(Some("first-job")),
                        1 => Ok(Some("second-job")),
                        _ => Ok(None),
                    }
                }
            },
            move |job| {
                observed_processed.lock().unwrap().push(job);
                async move { CreationDrainControl::StopDrain }
            },
        )
        .await
        .unwrap();

        assert_eq!(claims.load(std::sync::atomic::Ordering::Acquire), 1);
        assert_eq!(*processed.lock().unwrap(), vec!["first-job"]);
    }

    #[tokio::test]
    async fn closure_between_jobs_prevents_second_claim() {
        let admitted = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let admission = Arc::clone(&admitted);
        let claims = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed_claims = Arc::clone(&claims);
        let close_after_first = Arc::clone(&admitted);

        drain_claimed_jobs(
            move || {
                admission
                    .load(std::sync::atomic::Ordering::Acquire)
                    .then_some(())
                    .ok_or(())
            },
            move || {
                observed_claims.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                async { Ok(Some("job")) }
            },
            move |_| {
                close_after_first.store(false, std::sync::atomic::Ordering::Release);
                async { CreationDrainControl::Continue }
            },
        )
        .await
        .unwrap();

        assert_eq!(claims.load(std::sync::atomic::Ordering::Acquire), 1);
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
    let root = Path::new(repo_root);
    let anchor = git_common_dir_for_repository_root(root)
        .ok()
        .and_then(|common_dir| {
            let common = PathBuf::from(common_dir);
            if common.file_name().is_some_and(|name| name == ".git") {
                common.parent().map(Path::to_path_buf)
            } else {
                Some(common)
            }
        })
        .unwrap_or_else(|| root.to_path_buf());
    anchor.join(".phoenix").join("worktrees").join(conv_id)
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

#[cfg(test)]
mod product_pin_tests {
    use super::strict_product_creation_pin;
    use std::path::Path;

    fn git(path: &Path, args: &[&str]) -> String {
        crate::git_ops::run_git(path, args).expect("git command")
    }

    #[test]
    fn no_origin_pins_only_local_main() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-b", "main"]);
        git(dir.path(), &["config", "user.email", "test@example.com"]);
        git(dir.path(), &["config", "user.name", "Test"]);
        std::fs::write(dir.path().join("README"), "one").unwrap();
        git(dir.path(), &["add", "README"]);
        git(dir.path(), &["commit", "-m", "initial"]);
        let expected = git(dir.path(), &["rev-parse", "refs/heads/main^{commit}"]);
        assert_eq!(
            strict_product_creation_pin(dir.path()).unwrap(),
            (expected.trim().to_string(), "main".to_string())
        );
    }

    #[test]
    fn no_origin_does_not_fall_back_to_master() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-b", "master"]);
        assert!(strict_product_creation_pin(dir.path()).is_err());
    }

    #[test]
    fn origin_discovery_failure_does_not_fall_back_to_cached_head() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-b", "main"]);
        git(
            dir.path(),
            &["remote", "add", "origin", "/definitely/missing/remote"],
        );
        assert!(strict_product_creation_pin(dir.path()).is_err());
    }
}

#[cfg(test)]
mod model_resolution_tests {
    use super::resolve_creation_model;
    use phoenix_llm::ModelRegistry;

    fn registry() -> ModelRegistry {
        ModelRegistry::new_empty()
    }

    #[test]
    fn managed_creation_defaults_to_cheap_model() {
        let registry = registry();
        assert_eq!(
            resolve_creation_model(&registry, None, "managed", true),
            registry.cheap_model_id_for_provider(&registry.default_model_id())
        );
    }

    #[test]
    fn auto_creation_uses_direct_default_without_repository() {
        let registry = registry();
        assert_eq!(
            resolve_creation_model(&registry, None, "auto", false),
            registry.default_model_id()
        );
    }

    #[test]
    fn retired_creation_job_model_resolves_to_current_route() {
        let registry = registry();
        assert_eq!(
            resolve_creation_model(&registry, Some("gpt-5.3-codex"), "direct", false),
            "gpt-5.4"
        );
    }

    #[test]
    fn explicit_model_wins_over_mode_defaults() {
        let registry = registry();
        assert_eq!(
            resolve_creation_model(&registry, Some("gpt-5.4"), "managed", true),
            "gpt-5.4"
        );
    }
}
