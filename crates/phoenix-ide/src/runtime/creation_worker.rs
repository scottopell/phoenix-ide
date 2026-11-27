#[cfg(test)]
use crate::api::handlers::AppError;
use crate::api::handlers::{generate_slug, slugify_label, title_from_text};
use crate::db::{
    ConvMode, ConversationCreationMetadataUpdate, CreationClaimOutcome, ErrorKind, NonEmptyString,
    PRODUCT_CREATION_OWNER_MARKER,
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

async fn checkpoint_product_creation_stage(
    manager: &RuntimeManager,
    claimed: &crate::db::ClaimedProductCreationJob,
    current_stage: &mut crate::db::ProductCreationProvisioningStage,
    stage: crate::db::ProductCreationProvisioningStage,
) -> Result<(), String> {
    let stage = if stage.rank() < current_stage.rank() {
        *current_stage
    } else {
        stage
    };
    if manager
        .db()
        .advance_product_creation_provisioning_stage(&claimed.job.request_id, &claimed.claim, stage)
        .await
        .map_err(|error| error.to_string())?
    {
        *current_stage = stage;
        Ok(())
    } else {
        Err("product creation claim was lost or stage transition was rejected".to_string())
    }
}

#[allow(clippy::too_many_lines)]
async fn process_claimed_product_creation(
    manager: &Arc<RuntimeManager>,
    claimed: &crate::db::ClaimedProductCreationJob,
) -> Result<(), String> {
    use crate::db::ProductCreationProvisioningStage as Stage;

    let job = &claimed.job;
    let mut current_stage = job.provisioning_stage;
    checkpoint_product_creation_stage(manager, claimed, &mut current_stage, Stage::ValidateIntent)
        .await?;
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
    checkpoint_product_creation_stage(
        manager,
        claimed,
        &mut current_stage,
        Stage::ResolveRepository,
    )
    .await?;
    let direct_gateway = matches!(
        job.intent.origin,
        crate::db::ProductCreationOrigin::SlackGateway { .. }
    );
    let repo_root = if direct_gateway {
        None
    } else {
        tokio::task::spawn_blocking({
            let cwd = cwd.clone();
            move || discover_product_repository(Path::new(&cwd))
        })
        .await
        .map_err(|error| format!("repository discovery join failed: {error}"))??
    };

    if let Some(staged_repo) = job.staging_repo_root.as_deref() {
        if repo_root.as_deref().map(str::trim) != Some(staged_repo) {
            return Err("repository identity changed after recording creation staging".to_string());
        }
    } else if repo_root.is_none() && job.pin_exact_checkout_oid.is_some() {
        return Err("pinned Git creation can no longer resolve its repository".to_string());
    }

    let (effective_cwd, conv_mode, authority_kind, environment, logical_base, git_publication) =
        if let Some(repo_root) = repo_root {
            let repo_root = repo_root.trim().to_string();
            checkpoint_product_creation_stage(
                manager,
                claimed,
                &mut current_stage,
                Stage::ResolveStartingPin,
            )
            .await?;
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
            checkpoint_product_creation_stage(
                manager,
                claimed,
                &mut current_stage,
                Stage::MaterializeWorktree,
            )
            .await?;
            let planned_path = match job.staging_path.as_deref() {
                Some(path)
                    if job.staging_exact_oid.as_deref() == Some(oid.as_str())
                        && job.staging_repo_root.as_deref() == Some(repo_root.as_str()) =>
                {
                    PathBuf::from(path)
                }
                Some(_) => {
                    return Err(
                        "staged worktree does not match the durable repository and pin".to_string(),
                    );
                }
                None => product_worktree_path(
                    Path::new(&repo_root),
                    job.product_conversation_id.as_str(),
                )?,
            };
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
            let reservation = manager
                .db()
                .get_product_creation_resource_reservations(&job.request_id)
                .await
                .map_err(|error| error.to_string())?
                .into_iter()
                .find(|reservation| reservation.id == reservation_id)
                .ok_or_else(|| "product creation worktree reservation disappeared".to_string())?;
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
            let materialization_lock = tokio::task::spawn_blocking({
                let repo_root = repo_root.clone();
                move || {
                    RepositoryMutationLock::acquire(Path::new(&repo_root)).map_err(|error| error.0)
                }
            })
            .await
            .map_err(|error| format!("repository lock acquisition join failed: {error}"))??;
            let persisted_ownership_token = reservation.ownership_token.clone();
            let ownership_token = persisted_ownership_token
                .clone()
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            if !manager
                .db()
                .record_product_creation_resource_ownership(
                    &job.request_id,
                    &claimed.claim,
                    &planned_path.to_string_lossy(),
                    &ownership_token,
                    chrono::Utc::now(),
                )
                .await
                .map_err(|error| error.to_string())?
            {
                return Err(
                    "product creation claim was lost before materializing worktree".to_string(),
                );
            }
            let had_materialization_authority = persisted_ownership_token.is_some();
            let staged_checkout = tokio::task::spawn_blocking({
            let repo_root = repo_root.clone();
            let planned_path = planned_path.clone();
            let oid = oid.clone();
            move || {
                let _lock = materialization_lock;
                let path = planned_path;
                let already_present = match std::fs::symlink_metadata(&path) {
                    Ok(_) => true,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                    Err(error) => {
                        return Err(format!("could not inspect staging path {}: {error}", path.display()));
                    }
                };
                let (ownership_token, checkout_fingerprint) = if already_present {
                    verify_existing_product_creation_staging(
                        &repo_root,
                        &path,
                        &oid,
                        persisted_ownership_token.as_deref(),
                    )?
                } else {
                    if had_materialization_authority {
                        return Err("previously authorized worktree path is missing; inspect its Git registration".to_string());
                    }
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
                    let fingerprint = product_creation_checkout_fingerprint(&path)?;
                    verify_product_creation_checkout(&repo_root, &path, &oid)?;
                    write_product_creation_owner_marker(&path, &ownership_token, &fingerprint)?;
                    (ownership_token, fingerprint)
                };
                let staged_token = read_product_creation_owner_marker(&path)?;
                if staged_token.as_deref() != Some(ownership_token.as_str())
                    || product_creation_checkout_fingerprint(&path)? != checkout_fingerprint
                {
                    return Err("staging checkout changed after verification".to_string());
                }
                Ok::<_, String>((
                    path.to_string_lossy().to_string(),
                    ownership_token,
                    checkout_fingerprint,
                ))
            }
        })
        .await
        .map_err(|error| format!("worktree materialization join failed: {error}"))??;
            let (worktree, ownership_token, checkout_fingerprint) = staged_checkout;
            let non_empty = NonEmptyString::new(worktree.clone())
                .map_err(|_| "materialized worktree path was empty".to_string())?;
            if reservation.status == "reserved" {
                let present = manager
                    .db()
                    .mark_product_creation_resource_present(
                        &job.request_id,
                        &claimed.claim,
                        &planned_path.to_string_lossy(),
                        &ownership_token,
                        chrono::Utc::now(),
                    )
                    .await
                    .map_err(|error| error.to_string())?;
                if !present {
                    return Err(
                        "product creation claim was lost after materializing worktree".to_string(),
                    );
                }
            }
            (
                worktree.clone(),
                ConvMode::DetachedProductCreation {
                    worktree_path: non_empty,
                    base_branch: NonEmptyString::new(logical_base.clone())
                        .map_err(|_| "logical base was empty".to_string())?,
                },
                phoenix_core::work_scope::AuthorityKind::RestrictedExplore,
                phoenix_core::work_scope::EnvironmentContext::AllocatedWorktree {
                    cwd: worktree.clone(),
                    worktree_path: worktree,
                    branch_name: None,
                    base_branch: Some(logical_base.clone()),
                },
                Some(logical_base),
                Some(crate::db::ProductCreationGitPublicationFacts {
                    exact_checkout_oid: oid,
                    repository_root: repo_root,
                    ownership_token,
                    checkout_fingerprint,
                }),
            )
        } else {
            checkpoint_product_creation_stage(
                manager,
                claimed,
                &mut current_stage,
                Stage::ResolveStartingPin,
            )
            .await?;
            checkpoint_product_creation_stage(
                manager,
                claimed,
                &mut current_stage,
                Stage::MaterializeWorktree,
            )
            .await?;
            (
                cwd.clone(),
                ConvMode::Direct,
                phoenix_core::work_scope::AuthorityKind::Direct,
                phoenix_core::work_scope::EnvironmentContext::UnownedCwd { cwd },
                None,
                None,
            )
        };

    checkpoint_product_creation_stage(
        manager,
        claimed,
        &mut current_stage,
        Stage::FinalizeAttachments,
    )
    .await?;

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

        llm_language: job.intent.llm_language,
        spawned_from_conversation_id: None,
    };
    checkpoint_product_creation_stage(manager, claimed, &mut current_stage, Stage::CommitMetadata)
        .await?;
    let publication_lock = if let Some(facts) = git_publication.clone() {
        let path = PathBuf::from(&conversation.cwd);
        Some(
            tokio::task::spawn_blocking(move || lock_product_creation_publication(&path, &facts))
                .await
                .map_err(|error| format!("checkout verification join failed: {error}"))??,
        )
    } else {
        None
    };
    let published = manager
        .db()
        .publish_product_creation_atomically(&crate::db::ProductCreationPublishInput {
            request_id: job.request_id.clone(),
            claim: claimed.claim.clone(),
            conversation,
            authority_kind,
            environment,
            git_publication,
        })
        .await
        .map_err(|error| error.to_string())?;
    drop(publication_lock);
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
    Box::pin(deliver_product_creation_objective_inner(manager, &claimed)).await
}

async fn expand_product_creation_objective(
    manager: &RuntimeManager,
    conversation_id: &str,
    objective: &str,
    origin: &crate::db::ProductCreationOrigin,
) -> Result<crate::send_chat_service::ExpandedDispatchMessage, String> {
    let policy = match origin {
        crate::db::ProductCreationOrigin::UnknownHistorical
        | crate::db::ProductCreationOrigin::UserApi
        | crate::db::ProductCreationOrigin::SlackGateway { .. } => {
            crate::send_chat_service::MessageExpansionPolicy::ExpandReferences
        }
        crate::db::ProductCreationOrigin::HistoricalInternal(_)
        | crate::db::ProductCreationOrigin::InternalConversation { .. }
        | crate::db::ProductCreationOrigin::FederatedConversation { .. } => {
            crate::send_chat_service::MessageExpansionPolicy::LiteralText
        }
    };
    let conversation = manager
        .db()
        .get_conversation(conversation_id)
        .await
        .map_err(|error| error.to_string())?;
    crate::send_chat_service::expand_message(
        manager.db(),
        conversation_id,
        &conversation.cwd,
        objective,
        policy,
    )
    .await
    .map_err(|error| error.to_string())
}

#[allow(clippy::too_many_lines)]
async fn enqueue_product_creation_objective(
    manager: &Arc<RuntimeManager>,
    claimed: &crate::db::ClaimedProductCreationJob,
    event: Event,
    request_fingerprint: String,
) -> Result<(), String> {
    manager
        .enqueue_product_creation_steer_message(claimed, event, &request_fingerprint)
        .await
        .map_err(|error| error.to_string())
}

async fn persist_retryable_delivery_failure(
    manager: &Arc<RuntimeManager>,
    claimed: &crate::db::ClaimedProductCreationJob,
    delivery_error: &str,
) -> Result<String, String> {
    let scheduled = manager
        .db()
        .schedule_product_creation_delivery_retry(
            &claimed.job.request_id,
            &claimed.claim,
            delivery_error,
            product_creation_now(manager),
        )
        .await
        .map_err(|db_error| {
            format!("{delivery_error}; delivery retry persistence failed: {db_error}")
        })?;
    if !scheduled {
        return Err(format!(
            "{delivery_error}; delivery retry persistence lost its claim"
        ));
    }
    manager.kick_creation_worker();
    Ok(delivery_error.to_string())
}

async fn complete_already_accepted_product_creation_delivery(
    manager: &Arc<RuntimeManager>,
    claimed: &crate::db::ClaimedProductCreationJob,
    conversation_id: &str,
) -> Result<bool, String> {
    let job = &claimed.job;
    let steering_fingerprint = manager
        .db()
        .get_steering_acceptance_fingerprint(conversation_id, &job.request_id)
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
                &job.intent.live_intent().map_err(str::to_string)?,
            )
            .await
            .map_err(|error| error.to_string())?
        {
            let completed = manager
                .db()
                .complete_product_creation_delivery(
                    &job.request_id,
                    &claimed.claim,
                    exact_fingerprint,
                    &job.intent.live_intent().map_err(str::to_string)?,
                )
                .await
                .map_err(|error| error.to_string())?;
            if !completed {
                return Err(
                    "product creation delivery claim was lost before completion".to_string()
                );
            }
            return Ok(true);
        }
    }
    Ok(false)
}

async fn deliver_product_creation_objective(
    manager: &Arc<RuntimeManager>,
    claimed: &crate::db::ClaimedProductCreationJob,
) -> Result<(), String> {
    run_product_creation_until_closed(
        manager,
        claimed,
        Box::pin(deliver_product_creation_objective_inner(manager, claimed)),
    )
    .await
}

async fn deliver_product_creation_objective_inner(
    manager: &Arc<RuntimeManager>,
    claimed: &crate::db::ClaimedProductCreationJob,
) -> Result<(), String> {
    let mut current_stage = claimed.job.provisioning_stage;
    checkpoint_product_creation_stage(
        manager,
        claimed,
        &mut current_stage,
        crate::db::ProductCreationProvisioningStage::BootstrapInitialTurn,
    )
    .await?;
    let job = &claimed.job;
    job.published_product_id
        .as_ref()
        .ok_or_else(|| "delivery obligation lacked product id".to_string())?;
    let conversation_id = job
        .published_conversation_id
        .clone()
        .ok_or_else(|| "delivery obligation lacked conversation id".to_string())?;
    if complete_already_accepted_product_creation_delivery(manager, claimed, &conversation_id)
        .await?
    {
        return Ok(());
    }
    let live_intent = job.intent.live_intent().map_err(str::to_string)?;
    let images = live_intent
        .images
        .iter()
        .map(|image| crate::db::ImageData {
            data: image.data.clone(),
            media_type: image.media_type.clone(),
        })
        .collect();
    let expanded = match expand_product_creation_objective(
        manager,
        &conversation_id,
        &job.intent.objective,
        &job.intent.origin,
    )
    .await
    {
        Ok(expanded) => expanded,
        Err(error) => {
            return Err(persist_retryable_delivery_failure(manager, claimed, &error).await?);
        }
    };
    checkpoint_product_creation_stage(
        manager,
        claimed,
        &mut current_stage,
        crate::db::ProductCreationProvisioningStage::BootstrapInitialTurn,
    )
    .await?;
    let enqueue_result = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        enqueue_product_creation_objective(
            manager,
            claimed,
            Event::SteerMessage {
                origin: job.intent.origin.input_origin(),
                text: expanded.display_text,
                llm_text: expanded.llm_text,
                images,
                files: Vec::new(),
                message_id: job.request_id.clone(),
                user_agent: None,
                skill_invocation: expanded.skill_invocation,
            },
            format!("product-create:{}", job.request_id),
        ),
    )
    .await;
    if let Err(error) = enqueue_result {
        return Err(persist_retryable_delivery_failure(
            manager,
            claimed,
            &format!("product creation objective delivery timed out after 15 seconds: {error}"),
        )
        .await?);
    }
    if let Err(error) = enqueue_result.expect("checked timeout above") {
        return Err(persist_retryable_delivery_failure(manager, claimed, &error.clone()).await?);
    }
    let completed = manager
        .db()
        .complete_product_creation_delivery(
            &job.request_id,
            &claimed.claim,
            &crate::db::SteeringAcceptanceFingerprint::Exact(format!(
                "product-create:{}",
                job.request_id
            )),
            &job.intent.live_intent().map_err(str::to_string)?,
        )
        .await
        .map_err(|error| error.to_string())?;
    if !completed {
        return Err("product creation delivery claim was lost before completion".to_string());
    }
    Ok(())
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
    let reservations = manager
        .db()
        .get_product_creation_resource_reservations(request_id)
        .await
        .map_err(|error| error.to_string())?;
    let may_have_materialized = cleanup_job.staging_path.as_deref().is_some_and(|path| {
        !reservations.iter().any(|reservation| {
            reservation.resource_identity == path
                && reservation.status == "reserved"
                && reservation.ownership_token.is_none()
        })
    });
    if let Err(cleanup_error) =
        reconcile_unpublished_staging(cleanup_job.staging_path.as_deref(), may_have_materialized)
            .await
    {
        manager
            .db()
            .mark_product_creation_cleanup_ambiguous(
                request_id,
                claim,
                &cleanup_error,
                chrono::Utc::now(),
            )
            .await
            .map_err(|error| error.to_string())?;
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

async fn reconcile_unpublished_staging(
    staging_path: Option<&str>,
    may_have_materialized: bool,
) -> Result<(), String> {
    if may_have_materialized {
        return Err(format!(
            "unpublished worktree custody is unverified; inspect the checkout and Git registration for {}",
            staging_path.unwrap_or("the recorded creation reservation"),
        ));
    }
    let Some(path) = staging_path else {
        return Ok(());
    };
    match tokio::fs::symlink_metadata(path).await {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(format!(
            "unpublished path {path} has an occupant without proven custody"
        )),
        Err(error) => Err(format!(
            "could not inspect {path}: {error}; staging was retained"
        )),
    }
}

async fn reconcile_product_creation_cleanup(
    manager: &Arc<RuntimeManager>,
    cleanup: &crate::db::ProductCreationCleanupJob,
) -> Result<(), String> {
    for reservation in &cleanup.reservations {
        if reservation.status == "released" {
            continue;
        }
        if let Err(cleanup_error) = reconcile_unpublished_staging(
            Some(&reservation.resource_identity),
            reservation.ownership_token.is_some() || reservation.status != "reserved",
        )
        .await
        {
            if !manager
                .db()
                .mark_product_creation_resource_cleanup_conflict(
                    cleanup,
                    &reservation.id,
                    chrono::Utc::now(),
                )
                .await
                .map_err(|error| error.to_string())?
            {
                return Ok(());
            }
            manager
                .db()
                .mark_claimed_product_creation_cleanup_ambiguous(
                    cleanup,
                    &cleanup_error,
                    chrono::Utc::now(),
                )
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
) -> Result<(), String> {
    run_product_creation_until_closed(
        manager,
        &claimed,
        Box::pin(process_claimed_product_creation(manager, &claimed)),
    )
    .await
}

#[cfg(test)]
#[derive(Clone)]
pub(super) struct ClaimGuardPolicy {
    heartbeats: Arc<tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<()>>>,
    renewed: tokio::sync::mpsc::UnboundedSender<chrono::DateTime<chrono::Utc>>,
}

#[cfg(test)]
impl ClaimGuardPolicy {
    fn observe_renewal(&self, renewed_at: chrono::DateTime<chrono::Utc>) {
        let _ = self.renewed.send(renewed_at);
    }
}

fn product_creation_now(manager: &RuntimeManager) -> chrono::DateTime<chrono::Utc> {
    #[cfg(test)]
    if let Some(now) = manager.db().test_product_creation_now() {
        return now;
    }
    let _ = manager;
    chrono::Utc::now()
}

async fn run_product_creation_until_closed(
    manager: &Arc<RuntimeManager>,
    claimed: &crate::db::ClaimedProductCreationJob,
    processing: impl std::future::Future<Output = Result<(), String>>,
) -> Result<(), String> {
    let _admitted = manager.acquire_local_authority_pass().map_err(|()| {
        "product creation rejected after fatal local authority closure".to_string()
    })?;
    let request_id = &claimed.job.request_id;
    let claim = &claimed.claim;
    let (lease, heartbeat_interval) = (chrono::Duration::seconds(30), Duration::from_secs(10));
    #[cfg(test)]
    let policy = manager
        .product_creation_claim_guard_policy
        .lock()
        .unwrap()
        .clone();
    let renewed_at = product_creation_now(manager);
    let mut fatal = manager.fatal_local_authority_receiver();
    if !manager
        .db()
        .renew_product_creation_claim(request_id, claim, renewed_at, lease)
        .await
        .map_err(|error| error.to_string())?
    {
        return Err("product creation claim was lost before processing".to_string());
    }
    #[cfg(test)]
    if let Some(policy) = &policy {
        policy.observe_renewal(renewed_at);
    }
    let mut processing = std::pin::pin!(processing);
    let mut heartbeat = tokio::time::interval(heartbeat_interval);
    heartbeat.tick().await;
    loop {
        tokio::select! {
            biased;
            _ = crate::tls::wait_for_fatal_local_authority(&mut fatal) => {
                return Err("product creation stopped after fatal local authority closure".to_string());
            }
            result = &mut processing => return result,
            renewed_at = async {
                #[cfg(test)]
                if let Some(policy) = &policy {
                    policy.heartbeats.lock().await.recv().await
                        .expect("test must retain the heartbeat controller until work ends");
                    return Some(manager.db().test_product_creation_now()
                        .expect("controlled heartbeat requires the shared creation clock"));
                }
                heartbeat.tick().await;
                None::<chrono::DateTime<chrono::Utc>>
            } => {
                let _admitted = manager.acquire_local_authority_pass().map_err(|()| {
                    "product creation claim renewal rejected after fatal local authority closure".to_string()
                })?;
                let renewed_at = renewed_at.unwrap_or_else(chrono::Utc::now);
                let renewed = manager.db().renew_product_creation_claim(
                    request_id,
                    claim,
                    renewed_at,
                    lease,
                ).await.map_err(|error| error.to_string())?;
                if !renewed {
                    return Err("product creation claim was lost during heartbeat renewal".to_string());
                }
                #[cfg(test)]
                if let Some(policy) = &policy {
                    policy.observe_renewal(renewed_at);
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
    match crate::git_ops::run_git_with_env(
        cwd,
        &["rev-parse", "--show-toplevel"],
        &[("LC_ALL", "C")],
    ) {
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

fn verify_existing_product_creation_staging(
    repo_root: &str,
    path: &Path,
    expected_oid: &str,
    persisted_ownership_token: Option<&str>,
) -> Result<(String, String), String> {
    let expected = persisted_ownership_token
        .ok_or_else(|| "existing staging occupant lacked durable ownership".to_string())?;
    let before = product_creation_checkout_fingerprint(path)?;
    verify_product_creation_checkout(repo_root, path, expected_oid)?;
    let token = read_product_creation_owner_marker(path)?;
    if token.as_deref() != Some(expected) || product_creation_checkout_fingerprint(path)? != before
    {
        return Err("existing staging occupant did not match durable ownership".to_string());
    }
    Ok((expected.to_string(), before))
}

fn lock_product_creation_publication(
    path: &Path,
    facts: &crate::db::ProductCreationGitPublicationFacts,
) -> Result<RepositoryMutationLock, String> {
    let lock = RepositoryMutationLock::acquire(Path::new(&facts.repository_root))
        .map_err(|(error, _)| error)?;
    let (_, observed) = verify_existing_product_creation_staging(
        &facts.repository_root,
        path,
        &facts.exact_checkout_oid,
        Some(&facts.ownership_token),
    )?;
    if observed != facts.checkout_fingerprint {
        return Err("staging checkout changed before publication".to_string());
    }
    Ok(lock)
}

fn verify_product_creation_checkout(
    repo_root: &str,
    path: &Path,
    expected_oid: &str,
) -> Result<(), String> {
    let canonicalize = |path: &Path| path.canonicalize().map_err(|error| error.to_string());
    let dot_git = path.join(".git");
    let file_type = |path: &Path| {
        std::fs::symlink_metadata(path)
            .map(|metadata| metadata.file_type())
            .map_err(|error| error.to_string())
    };
    if !file_type(path)?.is_dir() || !file_type(&dot_git)?.is_file() {
        return Err("existing staging path is not a linked worktree directory".to_string());
    }
    let common = git_common_dir_for_repository_root(Path::new(repo_root))?;
    let admin_dir = crate::git_ops::run_git_bounded(
        path,
        &["rev-parse", "--absolute-git-dir"],
        Duration::from_secs(5),
    )?;
    let admin_dir = canonicalize(Path::new(admin_dir.trim()))?;
    let backpointer = admin_dir.join("gitdir");
    let registrations = canonicalize(&Path::new(&common).join("worktrees"))?;
    if admin_dir.parent() != Some(registrations.as_path()) || !file_type(&backpointer)?.is_file() {
        return Err(
            "existing staging has no exact Git registration in the reserved repository".to_string(),
        );
    }
    let (_, registered) = phoenix_core::git::read_worktree_registration_file(&backpointer)
        .map_err(|error| error.to_string())?;
    let registered = std::str::from_utf8(&registered).map_err(|error| error.to_string())?;
    let registered = admin_dir.join(registered.trim_end_matches(['\r', '\n']));
    if canonicalize(&registered)? != canonicalize(&dot_git)? {
        return Err("existing staging registration points to another checkout".to_string());
    }
    crate::api::handlers::validate_detached_task_worktree(path, expected_oid)
}

fn product_creation_owner_marker_path(worktree_path: &Path) -> Result<PathBuf, String> {
    let git_dir = crate::git_ops::run_git(
        worktree_path,
        &["rev-parse", "--path-format=absolute", "--git-dir"],
    )?;
    Ok(PathBuf::from(git_dir.trim()).join(PRODUCT_CREATION_OWNER_MARKER))
}

fn write_product_creation_owner_marker(
    worktree_path: &Path,
    ownership_token: &str,
    fingerprint: &str,
) -> Result<(), String> {
    use std::io::Write as _;

    if ownership_token.trim().is_empty() || ownership_token.contains('\n') {
        return Err("product creation ownership token was invalid".to_string());
    }
    let marker = product_creation_owner_marker_path(worktree_path)?;
    if product_creation_checkout_fingerprint(worktree_path)? != fingerprint {
        return Err("staging checkout changed before writing ownership".to_string());
    }
    let mut file = std::fs::File::create_new(marker)
        .map_err(|error| format!("could not create product creation owner marker: {error}"))?;
    write!(file, "{ownership_token}\n{fingerprint}")
        .map_err(|error| format!("could not write product creation owner marker: {error}"))
}

#[cfg(test)]
pub(crate) fn write_test_owner_marker(path: &Path, token: &str) {
    let fingerprint = product_creation_checkout_fingerprint(path).unwrap();
    write_product_creation_owner_marker(path, token, &fingerprint).unwrap();
}

fn product_creation_checkout_fingerprint(worktree_path: &Path) -> Result<String, String> {
    phoenix_core::git::observe_linked_worktree_fingerprint(worktree_path)
        .ok_or_else(|| "could not identify the exact linked checkout marker".to_string())
}

fn read_product_creation_owner_marker(worktree_path: &Path) -> Result<Option<String>, String> {
    let marker = product_creation_owner_marker_path(worktree_path)?;
    let bytes = match phoenix_core::git::read_worktree_registration_file(&marker) {
        Ok((_, bytes)) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "could not read product creation owner marker: {error}"
            ))
        }
    };
    let record = std::str::from_utf8(&bytes).map_err(|error| error.to_string())?;
    let Some((token, expected_fingerprint)) = record.split_once('\n') else {
        return Ok(None);
    };
    if product_creation_checkout_fingerprint(worktree_path)? != expected_fingerprint {
        return Ok(None);
    }
    Ok(Some(token.to_string()))
}

fn product_worktree_path(repo_root: &Path, id: &str) -> Result<PathBuf, String> {
    let common = PathBuf::from(git_common_dir_for_repository_root(repo_root)?);
    Ok(common.join("phoenix-worktrees").join(id))
}

fn strict_product_creation_pin(repo_root: &Path) -> Result<(String, String), String> {
    let _lock = RepositoryMutationLock::acquire(repo_root).map_err(|e| e.0)?;
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
        let oid = stdout
            .lines()
            .filter_map(|line| line.strip_suffix("\tHEAD"))
            .find(|oid| {
                matches!(oid.len(), 40 | 64) && oid.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
            .ok_or_else(|| "origin did not advertise a valid default commit ID".to_string())?;
        run_git_with_timeout(
            repo_root,
            &[
                "fetch",
                "--no-tags",
                "--no-prune",
                "--no-prune-tags",
                "--no-recurse-submodules",
                "--no-auto-maintenance",
                "--no-write-fetch-head",
                "--refmap=",
                "origin",
                oid,
            ],
            Duration::from_secs(30),
        )?;
        crate::git_ops::run_git(repo_root, &["cat-file", "-e", &format!("{oid}^{{commit}}")])?;
        return Ok((oid.to_string(), branch.to_string()));
    }
    let oid = crate::git_ops::run_git(repo_root, &["rev-parse", "refs/heads/main^{commit}"])
        .map_err(|_| "repository has no origin and no local refs/heads/main".to_string())?;
    Ok((oid.trim().to_string(), "main".to_string()))
}

pub(crate) fn resolve_creation_model(
    registry: &ModelRegistry,
    explicit_model: Option<&str>,
) -> Result<String, String> {
    if let Some(model) = explicit_model {
        return registry.resolve_model_id(model);
    }

    Ok(registry.default_model_id())
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
        let Some(job) = job else {
            return Ok(());
        };
        let control = process(job).await;
        drop(owner);
        if matches!(control, CreationDrainControl::StopDrain) {
            return Ok(());
        }
    }
}

#[cfg(test)]
async fn run_admitted_blocking<R, F>(
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
        if Box::pin(deliver_product_creation_objective(manager, &delivery))
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
        if let Err(error) = Box::pin(process_product_creation_until_closed(
            manager,
            claimed.clone(),
        ))
        .await
        {
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

fn legacy_creation_mode_is_supported(mode: &str) -> bool {
    mode == "direct"
}

#[cfg(test)]
mod legacy_creation_mode_tests {
    use super::legacy_creation_mode_is_supported;

    #[test]
    fn only_direct_creation_is_supported_by_the_legacy_worker() {
        assert!(legacy_creation_mode_is_supported("direct"));
        for retired_mode in ["branch", "managed", "auto", "approved_task"] {
            assert!(!legacy_creation_mode_is_supported(retired_mode));
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
    if !legacy_creation_mode_is_supported(requested_mode) {
        return Err((
            format!(
                "Legacy conversation creation mode '{requested_mode}' is no longer supported; use direct creation or the product-creation workflow"
            ),
            ErrorKind::InvalidRequest,
        )
            .into());
    }

    let resolved_model = resolve_creation_model(&manager.llm_registry, intent.model.as_deref())
        .map_err(|error| (error, ErrorKind::InvalidRequest))?;

    let conv_mode = ConvMode::Direct;
    let effective_cwd = initial_cwd;
    let mut project_id = None;
    let desired_base_branch = intent.base_branch.clone();

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

    let name_baseline = manager
        .db()
        .capture_conversation_name_baseline(&job.conversation_id)
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
                &name_baseline,
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
                    archived: Some(persisted_conversation.archived),
                    continued_in_conv_id: None,
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
            origin: phoenix_core::domain::db_schema::InputOrigin::UserApi,
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

pub(crate) struct RepositoryMutationLock {
    file: std::fs::File,
}

impl RepositoryMutationLock {
    pub(crate) fn acquire(repo_root: &Path) -> Result<Self, (String, ErrorKind)> {
        Self::acquire_with_timeout(repo_root, Duration::from_secs(30))
    }

    pub(crate) fn acquire_common_directory_until(
        common_dir: &Path,
        deadline: std::time::Instant,
    ) -> Result<Self, (String, ErrorKind)> {
        let (file, lock_path) = Self::open_common_directory_file_until(common_dir, deadline)?;
        Self::acquire_open_file_until(file, &lock_path, deadline)
    }

    fn acquire_with_timeout(
        repo_root: &Path,
        timeout: Duration,
    ) -> Result<Self, (String, ErrorKind)> {
        let deadline = std::time::Instant::now() + timeout;
        let (file, lock_path) = Self::open_file(repo_root, timeout)?;
        Self::acquire_open_file_until(file, &lock_path, deadline)
    }

    fn acquire_open_file_until(
        file: std::fs::File,
        lock_path: &Path,
        deadline: std::time::Instant,
    ) -> Result<Self, (String, ErrorKind)> {
        loop {
            if std::time::Instant::now() >= deadline {
                return Err(Self::timed_out());
            }
            match file.try_lock_exclusive() {
                Ok(()) => {
                    let guard = Self { file };
                    if std::time::Instant::now() >= deadline {
                        return Err(Self::timed_out());
                    }
                    return Ok(guard);
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => {
                    return Err((
                        format!("could not lock repository {}: {error}", lock_path.display()),
                        ErrorKind::ServerError,
                    ));
                }
            }
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return Err(Self::timed_out());
            }
            std::thread::sleep(remaining.min(Duration::from_millis(25)));
        }
    }

    fn timed_out() -> (String, ErrorKind) {
        (
            "repository mutation lock acquisition timed out".to_string(),
            ErrorKind::ServerError,
        )
    }

    fn open_file(
        repo_root: &Path,
        timeout: Duration,
    ) -> Result<(std::fs::File, PathBuf), (String, ErrorKind)> {
        let common_dir = run_git_with_timeout(
            repo_root,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
            timeout,
        )
        .map_err(|error| (error, ErrorKind::ServerError))?;
        Self::open_common_directory_file(Path::new(common_dir.trim()))
    }

    fn open_common_directory_file(
        common_dir: &Path,
    ) -> Result<(std::fs::File, PathBuf), (String, ErrorKind)> {
        let lock_path = common_dir.join("phoenix-creation.lock");
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

    fn open_common_directory_file_until(
        common_dir: &Path,
        deadline: std::time::Instant,
    ) -> Result<(std::fs::File, PathBuf), (String, ErrorKind)> {
        if std::time::Instant::now() >= deadline {
            return Err(Self::timed_out());
        }
        let (file, lock_path) = Self::open_common_directory_file(common_dir)?;
        if std::time::Instant::now() >= deadline {
            drop(file);
            return Err(Self::timed_out());
        }
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

#[cfg(test)]
mod product_creation_delivery_replay_tests {
    use super::*;
    use crate::db::{Database, ProductCreationIntent, ProductCreationPublishInput};
    use crate::platform::PlatformCapability;
    use crate::runtime::RuntimeManager;
    use crate::tools::mcp::McpClientManager;
    use phoenix_core::domain::db_schema::Conversation;
    use phoenix_core::domain::llm_types::ServiceTier;
    use phoenix_core::llm_language::LlmLanguage;
    use phoenix_core::work_scope::WorkScopeId;
    use phoenix_core::work_scope::{AuthorityKind, EnvironmentContext, RuntimeRole};
    use phoenix_llm::ModelRegistry;

    fn stale_delivery_conversation(
        conversation_id: &str,
        product_conversation_id: &phoenix_core::domain::close::ProductConversationId,
        cwd: &str,
    ) -> Conversation {
        Conversation {
            id: conversation_id.to_string(),
            product_conversation_id: product_conversation_id.clone(),
            slug: Some(conversation_id.to_string()),
            title: Some(conversation_id.to_string()),
            cwd: cwd.to_string(),
            parent_conversation_id: None,
            user_initiated: true,
            state: ConvState::Idle,
            state_updated_at: chrono::Utc::now(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            archived: true,
            model: None,
            effort: None,
            service_tier: ServiceTier::Standard,
            project_id: None,
            conv_mode: ConvMode::Direct,
            runtime_role: RuntimeRole::User,
            attached_work_scope_id: Some(WorkScopeId::new()),
            desired_base_branch: None,
            message_count: 0,
            transcript_generation: 1,
            seed_parent_id: None,
            seed_label: None,
            continued_in_conv_id: None,

            llm_language: LlmLanguage::Caveman,
            spawned_from_conversation_id: None,
        }
    }
    fn test_runtime_manager(db: Database) -> Arc<RuntimeManager> {
        Arc::new(RuntimeManager::new(
            db,
            Arc::new(ModelRegistry::new_empty()),
            PlatformCapability::None {
                details: "test".to_string(),
            },
            Arc::new(McpClientManager::new()),
            None,
        ))
    }

    async fn stop_test_creation_worker(manager: Arc<RuntimeManager>) {
        let stopped = Arc::downgrade(&manager);
        manager.signal_fatal_local_authority("test_creation_scheduler_shutdown");
        drop(manager);
        tokio::time::timeout(Duration::from_secs(5), async {
            while stopped.strong_count() != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("creation worker must release its runtime manager before test teardown");
    }

    async fn advance_product_metadata_for_test(
        db: &Database,
        claimed: &crate::db::ClaimedProductCreationJob,
    ) {
        use phoenix_db::ProductCreationProvisioningStage as Stage;
        for stage in [
            Stage::ResolveRepository,
            Stage::ResolveStartingPin,
            Stage::MaterializeWorktree,
            Stage::FinalizeAttachments,
            Stage::CommitMetadata,
        ] {
            assert!(db
                .advance_product_creation_provisioning_stage(
                    &claimed.job.request_id,
                    &claimed.claim,
                    stage,
                )
                .await
                .unwrap());
        }
    }

    async fn product_creation_job(
        db: &Database,
        request_id: &str,
    ) -> crate::db::ProductCreationJobRecord {
        db.get_product_creation_job(request_id)
            .await
            .unwrap()
            .unwrap()
    }

    #[tokio::test]
    async fn retry_checkpoints_skip_completed_phases_without_regressing_durable_stage() {
        use phoenix_db::ProductCreationProvisioningStage as Stage;
        let db = Database::open_in_memory().await.unwrap();
        let manager = test_runtime_manager(db.clone());
        db.accept_product_creation(
            "later-stage",
            &ProductCreationIntent {
                origin: phoenix_db::ProductCreationOrigin::UserApi,
                cwd: "/tmp/later-stage".to_string(),
                objective: "inspect".to_string(),
                model: None,
                effort: None,
                llm_language: LlmLanguage::Caveman,
                images: Vec::new(),
            },
        )
        .await
        .unwrap();
        let claimed = db
            .claim_product_creation(
                "later-stage",
                "worker",
                "token",
                chrono::Utc::now(),
                chrono::Duration::minutes(5),
            )
            .await
            .unwrap()
            .unwrap();
        for stage in [
            Stage::ResolveRepository,
            Stage::ResolveStartingPin,
            Stage::MaterializeWorktree,
            Stage::FinalizeAttachments,
        ] {
            assert!(db
                .advance_product_creation_provisioning_stage("later-stage", &claimed.claim, stage)
                .await
                .unwrap());
        }
        assert!(db
            .schedule_product_creation_retry(
                "later-stage",
                &claimed.claim,
                "retry",
                chrono::Utc::now()
            )
            .await
            .unwrap());
        let retry = product_creation_job(&db, "later-stage").await;
        let claimed = db
            .claim_product_creation(
                "later-stage",
                "replacement",
                "replacement-token",
                retry.retry_at.unwrap(),
                chrono::Duration::minutes(5),
            )
            .await
            .unwrap()
            .unwrap();
        let mut current_stage = claimed.job.provisioning_stage;
        let before = claimed.job.clone();
        for stage in [
            Stage::ValidateIntent,
            Stage::ResolveRepository,
            Stage::ResolveStartingPin,
            Stage::MaterializeWorktree,
            Stage::FinalizeAttachments,
        ] {
            checkpoint_product_creation_stage(&manager, &claimed, &mut current_stage, stage)
                .await
                .unwrap();
            assert_eq!(current_stage, Stage::FinalizeAttachments);
            assert_eq!(product_creation_job(&db, "later-stage").await, before);
        }
        checkpoint_product_creation_stage(
            &manager,
            &claimed,
            &mut current_stage,
            Stage::CommitMetadata,
        )
        .await
        .unwrap();
        assert_eq!(current_stage, Stage::CommitMetadata);
        let advanced = product_creation_job(&db, "later-stage").await;
        assert_eq!(advanced.provisioning_stage, Stage::CommitMetadata);
        assert!(advanced.stage_updated_at >= before.stage_updated_at);
    }

    async fn accept_and_claim_product_at(
        db: &Database,
        request_id: &str,
        cwd: &Path,
    ) -> crate::db::ClaimedProductCreationJob {
        db.accept_product_creation(
            request_id,
            &ProductCreationIntent {
                origin: phoenix_db::ProductCreationOrigin::UserApi,
                cwd: cwd.to_string_lossy().into_owned(),
                objective: "inspect".to_string(),
                model: None,
                effort: None,
                llm_language: LlmLanguage::Caveman,
                images: Vec::new(),
            },
        )
        .await
        .unwrap();
        db.claim_product_creation(
            request_id,
            "worker",
            "token",
            chrono::Utc::now(),
            chrono::Duration::seconds(30),
        )
        .await
        .unwrap()
        .unwrap()
    }

    #[tokio::test]
    async fn failed_creation_retains_owned_checkout_and_fences_new_claims() {
        let repo = super::product_creation_staging_ownership_tests::initialized_repo();
        let root = repo.path().canonicalize().unwrap();
        let root = root.to_str().unwrap();
        let db = Database::open_in_memory().await.unwrap();
        let manager = test_runtime_manager(db.clone());
        let request_id = "failed-owned-creation";
        let claimed = accept_and_claim_product_at(&db, request_id, Path::new(root)).await;
        let path = product_worktree_path(
            Path::new(root),
            claimed.job.product_conversation_id.as_str(),
        )
        .unwrap();
        let location = path.to_str().unwrap();
        let git = |args: &[&str]| crate::git_ops::run_git(Path::new(root), args).unwrap();
        let oid = git(&["rev-parse", "HEAD"]);
        assert!(db
            .reserve_product_creation_resource(
                "reservation",
                request_id,
                &claimed.claim,
                root,
                location,
                chrono::Utc::now(),
            )
            .await
            .unwrap());
        assert!(db
            .record_product_creation_staging(request_id, &claimed.claim, location, root, &oid)
            .await
            .unwrap());
        assert!(db
            .record_product_creation_resource_ownership(
                request_id,
                &claimed.claim,
                location,
                "owner-token",
                chrono::Utc::now(),
            )
            .await
            .unwrap());
        git(&["worktree", "add", "--detach", location, &oid]);
        write_test_owner_marker(&path, "owner-token");
        let common = git_common_dir_for_repository_root(Path::new(root)).unwrap();
        std::fs::write(Path::new(&common).join("info/exclude"), "late-data\n").unwrap();
        let registrations = git(&["worktree", "list", "--porcelain", "-z"]);
        assert!(
            crate::git_ops::run_git(&path, &["status", "--porcelain", "--ignored"])
                .unwrap()
                .is_empty()
        );
        let ignored = path.join("late-data");
        std::fs::write(&ignored, "preserve me").unwrap();

        let _lock = RepositoryMutationLock::acquire(Path::new(root)).unwrap();
        cleanup_and_retry_unpublished_product_creation(
            &manager,
            request_id,
            &claimed.claim,
            &claimed.job,
            "injected provisioning error",
        )
        .await
        .unwrap();

        let job = db
            .get_product_creation_job(request_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(job.status, "cleanup_ambiguous");
        assert_eq!(job.staging_path.as_deref(), Some(location));
        let reservations = db
            .get_product_creation_resource_reservations(request_id)
            .await
            .unwrap();
        assert_eq!(reservations.len(), 1);
        assert_eq!(
            reservations[0].ownership_token.as_deref(),
            Some("owner-token")
        );
        assert_eq!(
            git(&["worktree", "list", "--porcelain", "-z"]),
            registrations
        );
        assert_eq!(std::fs::read_to_string(ignored).unwrap(), "preserve me");
        assert!(db
            .claim_product_creation(
                request_id,
                "replacement",
                "replacement-token",
                chrono::Utc::now() + chrono::Duration::minutes(10),
                chrono::Duration::seconds(30),
            )
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn skipped_retry_checkpoint_rejects_replaced_and_cancelled_claims() {
        use phoenix_db::ProductCreationProvisioningStage as Stage;
        let db = Database::open_in_memory().await.unwrap();
        let manager = test_runtime_manager(db.clone());
        let mut old = accept_and_claim_product_at(&db, "skipped-fence", Path::new("/tmp")).await;
        advance_product_metadata_for_test(&db, &old).await;
        old.job = db
            .get_product_creation_job("skipped-fence")
            .await
            .unwrap()
            .unwrap();
        assert!(db
            .schedule_product_creation_retry(
                "skipped-fence",
                &old.claim,
                "retry",
                chrono::Utc::now(),
            )
            .await
            .unwrap());
        let retry = db
            .get_product_creation_job("skipped-fence")
            .await
            .unwrap()
            .unwrap();
        let replacement = db
            .claim_product_creation(
                "skipped-fence",
                "replacement",
                "new-token",
                retry.retry_at.unwrap(),
                chrono::Duration::seconds(30),
            )
            .await
            .unwrap()
            .unwrap();
        let mut stage = old.job.provisioning_stage;
        assert!(checkpoint_product_creation_stage(
            &manager,
            &old,
            &mut stage,
            Stage::ResolveStartingPin,
        )
        .await
        .is_err());
        assert_eq!(stage, Stage::CommitMetadata);
        assert_eq!(
            db.get_product_creation_job("skipped-fence")
                .await
                .unwrap()
                .unwrap(),
            replacement.job
        );

        assert!(db
            .cancel_product_creation("skipped-fence", chrono::Utc::now())
            .await
            .unwrap());
        let cancelled = db
            .get_product_creation_job("skipped-fence")
            .await
            .unwrap()
            .unwrap();
        let mut stage = replacement.job.provisioning_stage;
        assert!(checkpoint_product_creation_stage(
            &manager,
            &replacement,
            &mut stage,
            Stage::ValidateIntent,
        )
        .await
        .is_err());
        assert_eq!(
            db.get_product_creation_job("skipped-fence")
                .await
                .unwrap()
                .unwrap(),
            cancelled
        );
    }

    fn initialized_product_repository() -> (tempfile::TempDir, std::path::PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let repo = directory.path().canonicalize().unwrap();
        for args in [
            vec!["init", "--initial-branch=main"],
            vec!["config", "user.name", "Product Creation Test"],
            vec!["config", "user.email", "product-creation@example.invalid"],
            vec![
                "-c",
                "commit.gpgSign=false",
                "commit",
                "--allow-empty",
                "-m",
                "main pin",
            ],
            vec!["tag", "protected-tag"],
            vec!["checkout", "-b", "user-work"],
            vec![
                "-c",
                "commit.gpgSign=false",
                "commit",
                "--allow-empty",
                "-m",
                "user work",
            ],
        ] {
            crate::git_ops::run_git(repo.as_path(), &args).unwrap();
        }
        (directory, repo)
    }

    fn product_repository_refs(repo: &Path) -> String {
        crate::git_ops::run_git(
            repo,
            &[
                "for-each-ref",
                "--format=%(refname) %(objectname)",
                "refs/heads",
                "refs/tags",
            ],
        )
        .unwrap()
    }

    #[tokio::test]
    async fn product_worker_materializes_persisted_pin_detached_without_changing_refs() {
        let (_directory, repo) = initialized_product_repository();
        let db = Database::open_in_memory().await.unwrap();
        let manager = test_runtime_manager(db.clone());
        let mut claimed = accept_and_claim_product_at(&db, "git-pin", repo.as_path()).await;
        let (oid, base) = strict_product_creation_pin(repo.as_path()).unwrap();
        assert_eq!(base, "main");
        assert_ne!(
            crate::git_ops::run_git(repo.as_path(), &["rev-parse", "HEAD"])
                .unwrap()
                .trim(),
            oid
        );
        assert!(matches!(
            db.pin_product_creation_once("git-pin", &claimed.claim, &oid, &base, "fresh")
                .await
                .unwrap(),
            crate::db::ProductCreationPinOutcome::Pinned(_)
        ));
        claimed.job = db
            .get_product_creation_job("git-pin")
            .await
            .unwrap()
            .unwrap();
        let persisted_oid = claimed.job.pin_exact_checkout_oid.clone().unwrap();
        assert_eq!(persisted_oid, oid);
        // Move main after pin persistence so re-resolving the branch cannot pass this test.
        crate::git_ops::run_git(repo.as_path(), &["update-ref", "refs/heads/main", "HEAD"])
            .unwrap();
        let before = product_repository_refs(repo.as_path());
        assert_eq!(
            before.lines().count(),
            3,
            "snapshot must include both heads and the tag"
        );
        assert!(before.contains("refs/heads/main "));
        assert!(before.contains("refs/heads/user-work "));
        assert!(before.contains("refs/tags/protected-tag "));
        let original_head =
            crate::git_ops::run_git(repo.as_path(), &["rev-parse", "HEAD"]).unwrap();
        assert_ne!(original_head.trim(), persisted_oid);
        let worktree =
            product_worktree_path(repo.as_path(), claimed.job.product_conversation_id.as_str())
                .unwrap();
        let result = Box::pin(process_product_creation_until_closed(&manager, claimed)).await;
        let job = db
            .get_product_creation_job("git-pin")
            .await
            .unwrap()
            .unwrap();
        assert!(job.published_conversation_id.is_some(), "{result:?}");
        assert_eq!(job.pin_exact_checkout_oid.as_deref(), Some(oid.as_str()));
        crate::api::handlers::validate_detached_task_worktree(&worktree, &persisted_oid).unwrap();
        assert_eq!(
            crate::git_ops::run_git(&worktree, &["rev-parse", "HEAD"])
                .unwrap()
                .trim(),
            persisted_oid
        );
        assert!(crate::git_ops::run_git(&worktree, &["symbolic-ref", "-q", "HEAD"]).is_err());
        assert_eq!(product_repository_refs(repo.as_path()), before);
        assert_eq!(
            crate::git_ops::run_git(repo.as_path(), &["rev-parse", "HEAD"]).unwrap(),
            original_head
        );
        assert_eq!(
            crate::git_ops::run_git(repo.as_path(), &["symbolic-ref", "HEAD"])
                .unwrap()
                .trim(),
            "refs/heads/user-work"
        );
    }

    #[tokio::test]
    async fn cancellation_while_waiting_for_repository_lock_fences_late_pin_and_stage() {
        use phoenix_db::ProductCreationProvisioningStage as Stage;
        let (_directory, repo) = initialized_product_repository();
        let before_refs = product_repository_refs(repo.as_path());
        let lock = RepositoryMutationLock::acquire(repo.as_path()).unwrap();
        let db = Database::open_in_memory().await.unwrap();
        let manager = test_runtime_manager(db.clone());
        let claimed = accept_and_claim_product_at(&db, "cancel-lock", repo.as_path()).await;
        let worktree =
            product_worktree_path(repo.as_path(), claimed.job.product_conversation_id.as_str())
                .unwrap();
        let worker = tokio::spawn({
            let manager = Arc::clone(&manager);
            let claimed = claimed.clone();
            async move { Box::pin(process_product_creation_until_closed(&manager, claimed)).await }
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let job = db
                    .get_product_creation_job("cancel-lock")
                    .await
                    .unwrap()
                    .unwrap();
                if job.provisioning_stage == Stage::ResolveStartingPin {
                    break;
                }
                assert!(
                    !worker.is_finished(),
                    "worker exited before the pin lock barrier"
                );
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(!worker.is_finished());
        assert!(db
            .cancel_product_creation("cancel-lock", chrono::Utc::now())
            .await
            .unwrap());
        let cancelled = db
            .get_product_creation_job("cancel-lock")
            .await
            .unwrap()
            .unwrap();
        assert!(cancelled.claim_generation > claimed.claim.generation);
        drop(lock);
        assert!(tokio::time::timeout(Duration::from_secs(5), worker)
            .await
            .unwrap()
            .unwrap()
            .is_err());
        assert_eq!(
            db.get_product_creation_job("cancel-lock")
                .await
                .unwrap()
                .unwrap(),
            cancelled
        );
        assert_eq!(cancelled.provisioning_stage, Stage::ResolveStartingPin);
        assert!(cancelled.pin_exact_checkout_oid.is_none());
        assert!(cancelled.staging_path.is_none());
        assert!(cancelled.published_conversation_id.is_none());
        assert!(cancelled.published_product_id.is_none());
        assert!(db
            .get_product_creation_resource_reservations("cancel-lock")
            .await
            .unwrap()
            .is_empty());
        assert!(!worktree.exists());
        assert_eq!(product_repository_refs(repo.as_path()), before_refs);
        let mut stage = Stage::ResolveStartingPin;
        assert!(checkpoint_product_creation_stage(
            &manager,
            &claimed,
            &mut stage,
            Stage::MaterializeWorktree,
        )
        .await
        .is_err());
        assert_eq!(
            db.get_product_creation_job("cancel-lock")
                .await
                .unwrap()
                .unwrap(),
            cancelled
        );
    }

    #[tokio::test]
    async fn unpublished_cleanup_settles_a_tokenless_reservation() {
        let (_directory, repo) = initialized_product_repository();
        let repo_root = repo.to_str().unwrap();
        let oid = crate::git_ops::run_git(&repo, &["rev-parse", "HEAD"]).unwrap();
        for delete in [false, true] {
            let db = Database::open_in_memory().await.unwrap();
            let manager = test_runtime_manager(db.clone());
            let request = "cancel-before-git";
            let claimed = accept_and_claim_product_at(&db, request, &repo).await;
            let path =
                product_worktree_path(&repo, claimed.job.product_conversation_id.as_str()).unwrap();
            let path_str = path.to_str().unwrap();
            let now = chrono::Utc::now();
            assert!(db
                .reserve_product_creation_resource(
                    "reservation",
                    request,
                    &claimed.claim,
                    repo_root,
                    path_str,
                    now,
                )
                .await
                .unwrap());
            assert!(db
                .record_product_creation_staging(
                    request,
                    &claimed.claim,
                    path_str,
                    repo_root,
                    oid.trim(),
                )
                .await
                .unwrap());
            let accepted = if delete {
                db.request_product_creation_deletion(request, now).await
            } else {
                db.cancel_product_creation(request, now).await
            };
            assert!(accepted.unwrap());
            assert!(!db
                .record_product_creation_resource_ownership(
                    request,
                    &claimed.claim,
                    path_str,
                    "stale-owner",
                    now,
                )
                .await
                .unwrap());
            let cleanup = db
                .claim_next_product_creation_cleanup(
                    "cleaner",
                    "cleanup-token",
                    now,
                    chrono::Duration::seconds(30),
                )
                .await
                .unwrap()
                .unwrap();
            reconcile_product_creation_cleanup(&manager, &cleanup)
                .await
                .unwrap();
            assert!(!path.exists());
            let job = db.get_product_creation_job(request).await.unwrap();
            if delete {
                assert!(job.is_none());
            } else {
                assert_eq!(job.unwrap().status, "cancelled");
            }
        }
    }

    #[tokio::test]
    async fn direct_product_creation_advances_noop_phases_in_sequence() {
        let db = Database::open_in_memory().await.unwrap();
        let manager = test_runtime_manager(db.clone());
        let directory = tempfile::tempdir().unwrap();
        let cwd = directory.path().canonicalize().unwrap();
        sqlx::raw_sql(
            "CREATE TABLE observed_creation_stages (stage TEXT NOT NULL);
             CREATE TRIGGER observe_creation_stage AFTER UPDATE OF provisioning_stage
             ON product_creation_jobs WHEN NEW.provisioning_stage <> OLD.provisioning_stage
             BEGIN
                 INSERT INTO observed_creation_stages VALUES (NEW.provisioning_stage);
             END;",
        )
        .execute(db.pool())
        .await
        .unwrap();
        db.accept_product_creation(
            "direct-stages",
            &ProductCreationIntent {
                origin: phoenix_db::ProductCreationOrigin::UserApi,
                cwd: cwd.to_string_lossy().into_owned(),
                objective: "inspect".to_string(),
                model: None,
                effort: None,
                llm_language: LlmLanguage::Caveman,
                images: Vec::new(),
            },
        )
        .await
        .unwrap();
        let claimed = db
            .claim_product_creation(
                "direct-stages",
                "worker",
                "token",
                chrono::Utc::now(),
                chrono::Duration::minutes(5),
            )
            .await
            .unwrap()
            .unwrap();
        let _delivery_result = process_claimed_product_creation(&manager, &claimed).await;
        let stages: Vec<String> =
            sqlx::query_scalar("SELECT stage FROM observed_creation_stages ORDER BY rowid")
                .fetch_all(db.pool())
                .await
                .unwrap();
        assert!(
            stages.starts_with(&[
                "resolve_repository".to_string(),
                "resolve_starting_pin".to_string(),
                "materialize_worktree".to_string(),
                "finalize_attachments".to_string(),
                "commit_metadata".to_string(),
                "bootstrap_initial_turn".to_string(),
            ]),
            "{stages:?}"
        );
        let job = db
            .get_product_creation_job("direct-stages")
            .await
            .unwrap()
            .unwrap();
        let conversation = db
            .get_conversation(job.published_conversation_id.as_ref().unwrap())
            .await
            .unwrap();
        assert_eq!(conversation.conv_mode, ConvMode::Direct);
        assert!(job.stage_updated_at >= claimed.job.stage_updated_at);
    }

    #[tokio::test]
    async fn scheduler_discovers_accepted_job_without_request_wake() {
        let db = Database::open_in_memory().await.unwrap();
        let manager = test_runtime_manager(db.clone());
        manager.start_creation_worker().await.unwrap();
        let (idle_tx, idle_rx) = tokio::sync::oneshot::channel();
        *manager.creation_worker_idle_signal.lock().unwrap() = Some(idle_tx);
        tokio::time::timeout(Duration::from_secs(5), idle_rx)
            .await
            .expect("worker must finish draining and poll its no-deadline wait")
            .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let unusable_cwd = directory.path().join("not-a-directory");
        std::fs::write(&unusable_cwd, "occupied").unwrap();
        db.accept_product_creation(
            "no-request-wake",
            &ProductCreationIntent {
                origin: phoenix_db::ProductCreationOrigin::UserApi,
                cwd: unusable_cwd.to_string_lossy().into_owned(),
                objective: "inspect".to_string(),
                model: None,
                effort: None,
                llm_language: LlmLanguage::Caveman,
                images: Vec::new(),
            },
        )
        .await
        .unwrap();
        let observed = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let job = db
                    .get_product_creation_job("no-request-wake")
                    .await
                    .unwrap()
                    .unwrap();
                if job.status == "retry_scheduled" || job.status == "failed" {
                    break job;
                }
                tokio::task::yield_now().await;
            }
        })
        .await;
        stop_test_creation_worker(manager).await;
        let observed = observed.expect("scheduler must discover a committed row without a wake");
        assert!(observed.claim_generation > 0);
        assert!(observed.published_conversation_id.is_none());
        assert_eq!(
            observed.provisioning_stage,
            phoenix_db::ProductCreationProvisioningStage::ValidateIntent
        );
    }

    #[tokio::test]
    async fn scheduler_recovers_expired_product_claim_without_another_request() {
        let db = Database::open_in_memory().await.unwrap();
        let directory = tempfile::tempdir().unwrap();
        let unusable_cwd = directory.path().join("not-a-directory");
        std::fs::write(&unusable_cwd, "occupied").unwrap();
        db.accept_product_creation(
            "expired-product-claim",
            &ProductCreationIntent {
                origin: phoenix_db::ProductCreationOrigin::UserApi,
                cwd: unusable_cwd.to_string_lossy().into_owned(),
                objective: "inspect".to_string(),
                model: None,
                effort: None,
                llm_language: LlmLanguage::Caveman,
                images: Vec::new(),
            },
        )
        .await
        .unwrap();
        let expired = db
            .claim_product_creation(
                "expired-product-claim",
                "lost-worker",
                "lost-token",
                chrono::Utc::now() - chrono::Duration::seconds(60),
                chrono::Duration::seconds(30),
            )
            .await
            .unwrap()
            .unwrap();
        let manager = test_runtime_manager(db.clone());
        manager.start_creation_worker().await.unwrap();
        let observed = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let job = db
                    .get_product_creation_job("expired-product-claim")
                    .await
                    .unwrap()
                    .unwrap();
                if job.status == "retry_scheduled" || job.status == "failed" {
                    break job;
                }
                tokio::task::yield_now().await;
            }
        })
        .await;
        stop_test_creation_worker(manager).await;
        let observed = observed.expect("scheduler must replace the expired generation");
        assert!(observed.claim_generation > expired.claim.generation);
        assert!(observed.published_conversation_id.is_none());
    }

    async fn seed_retry_identity_delivery(
        db: &Database,
        cwd: &str,
        request_id: &str,
        conversation_id: &str,
        objective: &str,
    ) -> (
        ProductCreationIntent,
        phoenix_core::domain::close::ProductConversationId,
        chrono::DateTime<chrono::Utc>,
    ) {
        let intent = ProductCreationIntent {
            origin: phoenix_db::ProductCreationOrigin::UserApi,
            cwd: cwd.to_string(),
            objective: objective.to_string(),
            model: None,
            effort: None,
            images: Vec::new(),
            llm_language: LlmLanguage::Caveman,
        };
        db.accept_product_creation(request_id, &intent)
            .await
            .unwrap();
        let provisioning = db
            .claim_product_creation(
                request_id,
                "provisioner",
                "provision-token",
                chrono::Utc::now(),
                chrono::Duration::minutes(5),
            )
            .await
            .unwrap()
            .unwrap();
        let accepted_product_id = provisioning.job.product_conversation_id.clone();
        let publish_claim_lease_until = provisioning.claim.lease_until;
        let conversation =
            stale_delivery_conversation(conversation_id, &accepted_product_id, &intent.cwd);
        advance_product_metadata_for_test(db, &provisioning).await;
        assert!(db
            .publish_product_creation_atomically(&ProductCreationPublishInput {
                request_id: request_id.to_string(),
                claim: provisioning.claim,
                conversation,
                authority_kind: AuthorityKind::Direct,
                environment: EnvironmentContext::UnownedCwd {
                    cwd: intent.cwd.clone(),
                },
                git_publication: None,
            })
            .await
            .unwrap());
        (intent, accepted_product_id, publish_claim_lease_until)
    }

    async fn seed_live_delivery(
        db: &Database,
        cwd: &Path,
        request_id: &str,
        conversation_id: &str,
    ) -> crate::db::ClaimedProductCreationJob {
        seed_retry_identity_delivery(
            db,
            cwd.to_str().unwrap(),
            request_id,
            conversation_id,
            "deliver objective",
        )
        .await;
        let job = db
            .get_product_creation_job(request_id)
            .await
            .unwrap()
            .unwrap();
        crate::db::ClaimedProductCreationJob {
            claim: crate::db::ProductCreationClaim {
                worker_id: job.claim_worker_id.clone().unwrap(),
                token: job.claim_token.clone().unwrap(),
                generation: job.claim_generation,
                lease_until: job.claim_lease_until.unwrap(),
            },
            job,
        }
    }

    #[tokio::test]
    async fn delivery_claim_renewal_preserves_identity_and_expiry_fences() {
        let db = Database::open_in_memory().await.unwrap();
        let directory = tempfile::tempdir().unwrap();
        let delivery =
            seed_live_delivery(&db, directory.path(), "renew-delivery", "renew-root").await;
        let now = chrono::Utc::now();
        assert!(db
            .renew_product_creation_claim(
                "renew-delivery",
                &delivery.claim,
                now,
                chrono::Duration::seconds(30),
            )
            .await
            .unwrap());
        for wrong_field in 0..3 {
            let mut stale = delivery.claim.clone();
            match wrong_field {
                0 => stale.generation += 1,
                1 => stale.worker_id.push_str("-wrong"),
                _ => stale.token.push_str("-wrong"),
            }
            assert!(!db
                .renew_product_creation_claim(
                    "renew-delivery",
                    &stale,
                    now + chrono::Duration::seconds(1),
                    chrono::Duration::seconds(30),
                )
                .await
                .unwrap());
        }
        let expiry = db
            .get_product_creation_job("renew-delivery")
            .await
            .unwrap()
            .unwrap()
            .claim_lease_until
            .unwrap();
        assert_eq!(
            expiry.timestamp_micros(),
            (now + chrono::Duration::seconds(30)).timestamp_micros()
        );
        assert!(!db
            .renew_product_creation_claim(
                "renew-delivery",
                &delivery.claim,
                expiry,
                chrono::Duration::seconds(30),
            )
            .await
            .unwrap());
        let replacement = db
            .claim_next_product_creation_delivery(
                "replacement",
                "new-token",
                expiry,
                chrono::Duration::seconds(30),
            )
            .await
            .unwrap()
            .unwrap();
        assert!(replacement.claim.generation > delivery.claim.generation);
        assert!(!db
            .renew_product_creation_claim(
                "renew-delivery",
                &delivery.claim,
                expiry,
                chrono::Duration::seconds(30),
            )
            .await
            .unwrap());
        assert!(db
            .renew_product_creation_claim(
                "renew-delivery",
                &replacement.claim,
                expiry + chrono::Duration::seconds(1),
                chrono::Duration::seconds(30),
            )
            .await
            .unwrap());
        assert!(db
            .schedule_product_creation_delivery_retry(
                "renew-delivery",
                &replacement.claim,
                "retry",
                expiry + chrono::Duration::seconds(2),
            )
            .await
            .unwrap());
        assert!(!db
            .renew_product_creation_claim(
                "renew-delivery",
                &replacement.claim,
                expiry + chrono::Duration::seconds(3),
                chrono::Duration::seconds(30),
            )
            .await
            .unwrap());
    }

    struct ControlledClaimGuard {
        db: Database,
        now: chrono::DateTime<chrono::Utc>,
        heartbeats: tokio::sync::mpsc::UnboundedSender<()>,
        renewals: tokio::sync::mpsc::UnboundedReceiver<chrono::DateTime<chrono::Utc>>,
    }

    impl ControlledClaimGuard {
        fn install(
            manager: &RuntimeManager,
            delivery: &crate::db::ClaimedProductCreationJob,
        ) -> Self {
            let (heartbeats, ticks) = tokio::sync::mpsc::unbounded_channel();
            let (renewed, renewals) = tokio::sync::mpsc::unbounded_channel();
            let db = manager.db().clone();
            let now = delivery.job.updated_at;
            db.test_set_product_creation_now(now);
            *manager.product_creation_claim_guard_policy.lock().unwrap() = Some(ClaimGuardPolicy {
                heartbeats: Arc::new(tokio::sync::Mutex::new(ticks)),
                renewed,
            });
            Self {
                db,
                now,
                heartbeats,
                renewals,
            }
        }

        fn heartbeat_after(&mut self, elapsed: chrono::Duration) {
            self.now += elapsed;
            self.db.test_set_product_creation_now(self.now);
            self.heartbeats.send(()).unwrap();
        }

        async fn observe_renewal(&mut self) {
            let renewed_at = tokio::time::timeout(Duration::from_secs(5), self.renewals.recv())
                .await
                .expect("claim guard did not acknowledge the controlled heartbeat")
                .expect("claim guard renewal channel closed");
            assert_eq!(renewed_at, self.now);
        }

        async fn renew_after(&mut self, elapsed: chrono::Duration) {
            self.heartbeat_after(elapsed);
            self.observe_renewal().await;
        }
    }

    #[derive(Clone, Copy)]
    enum EnqueueLease {
        Renewed,
        MissingExtension,
        ExactExpiry,
    }

    #[tokio::test]
    async fn slow_delivery_keeps_claim_live_through_enqueue() {
        slow_delivery_enqueue_fence(EnqueueLease::Renewed).await;
    }

    #[tokio::test]
    async fn slow_delivery_enqueue_rejects_missing_lease_extension() {
        slow_delivery_enqueue_fence(EnqueueLease::MissingExtension).await;
    }

    #[tokio::test]
    async fn slow_delivery_enqueue_rejects_exact_expiry() {
        slow_delivery_enqueue_fence(EnqueueLease::ExactExpiry).await;
    }

    #[allow(clippy::too_many_lines)]
    async fn slow_delivery_enqueue_fence(lease: EnqueueLease) {
        let db = Database::open_in_memory().await.unwrap();
        let manager = test_runtime_manager(db.clone());
        let directory = tempfile::tempdir().unwrap();
        let delivery =
            seed_live_delivery(&db, directory.path(), "slow-delivery", "slow-root").await;
        let mut clock = ControlledClaimGuard::install(&manager, &delivery);
        let first_lease = clock.now + chrono::Duration::seconds(30);
        let enqueue = Arc::new(tokio::sync::Barrier::new(2));
        manager
            .steering_enqueue_handle_barriers
            .lock()
            .await
            .insert("slow-root".to_string(), Arc::clone(&enqueue));
        let worker = tokio::spawn({
            let manager = Arc::clone(&manager);
            async move { deliver_product_creation_objective(&manager, &delivery).await }
        });
        tokio::time::timeout(Duration::from_secs(5), enqueue.wait())
            .await
            .expect("delivery must reach the real enqueue barrier");
        clock.observe_renewal().await;
        assert_eq!(
            global_objective_artifact_counts(&db, "slow-delivery").await,
            (0, 0)
        );
        for _ in 0..4 {
            clock.renew_after(chrono::Duration::seconds(10)).await;
            let current = product_creation_job(&db, "slow-delivery").await;
            assert_eq!(
                current.claim_lease_until.unwrap(),
                clock.now + chrono::Duration::seconds(30),
                "a renewal acknowledgement must correspond to a persisted lease extension"
            );
        }
        let current = db
            .get_product_creation_job("slow-delivery")
            .await
            .unwrap()
            .unwrap();
        assert!(clock.now > first_lease);
        assert!(current.claim_lease_until.unwrap() > clock.now);
        assert!(current.claim_lease_until.unwrap() > first_lease);
        assert_eq!(current.status, "delivery_pending");
        assert!(!worker.is_finished());
        assert!(db
            .claim_next_product_creation_delivery(
                "competitor",
                "competitor-token",
                clock.now,
                chrono::Duration::seconds(30),
            )
            .await
            .unwrap()
            .is_none());
        let refused_expiry = match lease {
            EnqueueLease::Renewed => None,
            EnqueueLease::MissingExtension => Some(first_lease),
            EnqueueLease::ExactExpiry => Some(clock.now),
        };
        if let Some(expiry) = refused_expiry {
            sqlx::query(
                "UPDATE product_creation_jobs SET claim_lease_until_unix_micros = ?1
                 WHERE request_id = 'slow-delivery'",
            )
            .bind(expiry.timestamp_micros())
            .execute(db.pool())
            .await
            .unwrap();
        }
        enqueue.wait().await;
        let result = tokio::time::timeout(Duration::from_secs(5), worker)
            .await
            .unwrap()
            .unwrap();
        if refused_expiry.is_some() {
            assert!(result.unwrap_err().contains("claim"));
            assert_eq!(
                global_objective_artifact_counts(&db, "slow-delivery").await,
                (0, 0)
            );
            let refused = product_creation_job(&db, "slow-delivery").await;
            assert_eq!(refused.status, "delivery_pending");
            assert_eq!(refused.claim_lease_until, refused_expiry);
            assert!(db
                .get_steering_acceptance_fingerprint("slow-root", "slow-delivery")
                .await
                .unwrap()
                .is_none());
            assert!(db.get_conversation("slow-root").await.unwrap().archived);
            return;
        }
        result.unwrap();
        let completed = db
            .get_product_creation_job("slow-delivery")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(completed.status, "published");
        assert_eq!(completed.updated_at, clock.now);
        assert_eq!(completed.stage_updated_at, clock.now);
        assert_eq!(
            completed.provisioning_stage,
            phoenix_db::ProductCreationProvisioningStage::FinalizePublication
        );
        let (committed, queued) = global_objective_artifact_counts(&db, "slow-delivery").await;
        assert_eq!(committed + queued, 1);
    }

    #[tokio::test]
    async fn delivery_heartbeat_at_exact_expiry_drops_work_without_reviving_claim() {
        let db = Database::open_in_memory().await.unwrap();
        let manager = test_runtime_manager(db.clone());
        let directory = tempfile::tempdir().unwrap();
        let delivery =
            seed_live_delivery(&db, directory.path(), "expired-heartbeat", "expired-root").await;
        let mut clock = ControlledClaimGuard::install(&manager, &delivery);
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        let worker = tokio::spawn({
            let manager = Arc::clone(&manager);
            async move {
                run_product_creation_until_closed(&manager, &delivery, async {
                    release_rx.await.unwrap();
                    panic!("expired delivery must be dropped rather than resumed");
                })
                .await
            }
        });
        clock.observe_renewal().await;
        let before = product_creation_job(&db, "expired-heartbeat").await;
        clock.heartbeat_after(chrono::Duration::seconds(30));
        let error = tokio::time::timeout(Duration::from_secs(5), worker)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert_eq!(
            error,
            "product creation claim was lost during heartbeat renewal"
        );
        assert!(release_tx.send(()).is_err());
        assert_eq!(product_creation_job(&db, "expired-heartbeat").await, before);
        assert_eq!(
            global_objective_artifact_counts(&db, "expired-heartbeat").await,
            (0, 0)
        );
    }

    #[tokio::test]
    async fn delivery_heartbeat_observes_committed_cancellation_and_drops_work() {
        let db = Database::open_in_memory().await.unwrap();
        let manager = test_runtime_manager(db.clone());
        let directory = tempfile::tempdir().unwrap();
        let delivery = seed_live_delivery(
            &db,
            directory.path(),
            "cancel-heartbeat",
            "cancel-heartbeat-root",
        )
        .await;
        let mut clock = ControlledClaimGuard::install(&manager, &delivery);
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let worker = tokio::spawn({
            let manager = Arc::clone(&manager);
            async move {
                run_product_creation_until_closed(
                    &manager,
                    &delivery,
                    Box::pin(async {
                        started_tx.send(()).unwrap();
                        release_rx.await.unwrap();
                        Box::pin(deliver_product_creation_objective_inner(
                            &manager, &delivery,
                        ))
                        .await
                    }),
                )
                .await
            }
        });
        tokio::time::timeout(Duration::from_secs(1), started_rx)
            .await
            .unwrap()
            .unwrap();
        assert!(db
            .cancel_product_creation("cancel-heartbeat", clock.now)
            .await
            .unwrap());
        clock.heartbeat_after(chrono::Duration::seconds(10));
        let error = tokio::time::timeout(Duration::from_secs(1), worker)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert_eq!(
            error,
            "product creation claim was lost during heartbeat renewal"
        );
        assert!(release_tx.send(()).is_err());
        assert_eq!(
            global_objective_artifact_counts(&db, "cancel-heartbeat").await,
            (0, 0)
        );
        assert_eq!(
            db.get_product_creation_job("cancel-heartbeat")
                .await
                .unwrap()
                .unwrap()
                .status,
            "cancelling"
        );
    }

    #[tokio::test]
    async fn delivery_heartbeat_stops_replaced_claim_instead_of_awaiting_unfenced_work() {
        let db = Database::open_in_memory().await.unwrap();
        let manager = test_runtime_manager(db.clone());
        let directory = tempfile::tempdir().unwrap();
        let delivery =
            seed_live_delivery(&db, directory.path(), "replace-delivery", "replace-root").await;
        let mut clock = ControlledClaimGuard::install(&manager, &delivery);
        let old_claim = delivery.claim.clone();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let worker = tokio::spawn({
            let manager = Arc::clone(&manager);
            async move {
                run_product_creation_until_closed(
                    &manager,
                    &delivery,
                    Box::pin(async {
                        started_tx.send(()).unwrap();
                        release_rx.await.unwrap();
                        Box::pin(deliver_product_creation_objective_inner(
                            &manager, &delivery,
                        ))
                        .await
                    }),
                )
                .await
            }
        });
        tokio::time::timeout(Duration::from_secs(1), started_rx)
            .await
            .unwrap()
            .unwrap();
        sqlx::query(
            "UPDATE product_creation_jobs
             SET claim_generation = claim_generation + 1,
                 claim_worker_id = 'replacement', claim_token = 'replacement-token',
                 claim_lease_until_unix_micros = ?1
             WHERE request_id = 'replace-delivery'",
        )
        .bind((clock.now + chrono::Duration::seconds(30)).timestamp_micros())
        .execute(db.pool())
        .await
        .unwrap();
        let replacement = product_creation_job(&db, "replace-delivery").await;
        assert!(replacement.claim_generation > old_claim.generation);
        clock.heartbeat_after(chrono::Duration::seconds(10));
        let error = tokio::time::timeout(Duration::from_secs(1), worker)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert_eq!(
            error,
            "product creation claim was lost during heartbeat renewal"
        );
        assert!(
            release_tx.send(()).is_err(),
            "lost-claim work must have been dropped"
        );
        assert_eq!(
            db.get_product_creation_job("replace-delivery")
                .await
                .unwrap()
                .unwrap(),
            replacement
        );
        assert_eq!(
            global_objective_artifact_counts(&db, "replace-delivery").await,
            (0, 0)
        );
        assert!(db.get_conversation("replace-root").await.unwrap().archived);
    }

    #[tokio::test]
    async fn fatal_authority_closure_drops_pending_delivery_without_late_enqueue() {
        let db = Database::open_in_memory().await.unwrap();
        let manager = test_runtime_manager(db.clone());
        let directory = tempfile::tempdir().unwrap();
        let delivery =
            seed_live_delivery(&db, directory.path(), "fatal-delivery", "fatal-root").await;
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let worker = tokio::spawn({
            let manager = Arc::clone(&manager);
            async move {
                run_product_creation_until_closed(
                    &manager,
                    &delivery,
                    Box::pin(async {
                        started_tx.send(()).unwrap();
                        release_rx.await.unwrap();
                        Box::pin(deliver_product_creation_objective_inner(
                            &manager, &delivery,
                        ))
                        .await
                    }),
                )
                .await
            }
        });
        started_rx.await.unwrap();
        let before = db
            .get_product_creation_job("fatal-delivery")
            .await
            .unwrap()
            .unwrap();
        manager.signal_fatal_local_authority("test_product_delivery_closure");
        let error = tokio::time::timeout(Duration::from_secs(5), worker)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert_eq!(
            error,
            "product creation stopped after fatal local authority closure"
        );
        assert!(release_tx.send(()).is_err());
        assert_eq!(
            db.get_product_creation_job("fatal-delivery")
                .await
                .unwrap()
                .unwrap(),
            before
        );
        assert_eq!(
            global_objective_artifact_counts(&db, "fatal-delivery").await,
            (0, 0)
        );
        assert!(db.get_conversation("fatal-root").await.unwrap().archived);
    }

    async fn claim_delivery_attempt(
        db: &Database,
        request_id: &str,
        attempt_label: &str,
        claim_time: chrono::DateTime<chrono::Utc>,
    ) -> crate::db::ClaimedProductCreationJob {
        db.claim_next_product_creation_delivery(
            &format!("delivery-worker-{attempt_label}"),
            &format!("delivery-token-{attempt_label}"),
            claim_time,
            chrono::Duration::minutes(5),
        )
        .await
        .unwrap()
        .unwrap_or_else(|| panic!("expected delivery claim for {request_id} at {claim_time}"))
    }

    async fn global_objective_artifact_counts(db: &Database, request_id: &str) -> (i64, i64) {
        let committed_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE message_id = ?1")
                .bind(request_id)
                .fetch_one(db.pool())
                .await
                .unwrap();
        let queued_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM steering_messages WHERE message_id = ?1")
                .bind(request_id)
                .fetch_one(db.pool())
                .await
                .unwrap();
        (committed_count, queued_count)
    }
    struct RetryIdentityExpectations {
        job_count: i64,
        product_count: i64,
        artifact_delta: (i64, i64),
        total_delta: i64,
    }

    async fn assert_retry_identity_job_state(
        db: &Database,
        request_id: &str,
        conversation_id: &str,
        accepted_product_id: &phoenix_core::domain::close::ProductConversationId,
        expected_status: &str,
    ) -> crate::db::ProductCreationJobRecord {
        let job = db
            .get_product_creation_job(request_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(job.product_conversation_id, *accepted_product_id);
        assert_eq!(job.published_product_id, Some(accepted_product_id.clone()));
        assert_eq!(
            job.published_conversation_id.as_deref(),
            Some(conversation_id)
        );
        assert_eq!(job.last_error.as_deref(), Some("steering queue is full"));
        assert_eq!(job.status, expected_status);
        assert!(db
            .get_steering_acceptance_fingerprint(conversation_id, request_id)
            .await
            .unwrap()
            .is_none());
        job
    }

    async fn assert_retry_identity_counts(
        db: &Database,
        request_id: &str,
        expected_published_product_id: &phoenix_core::domain::close::ProductConversationId,
        expected_published_conversation_id: &str,
        expectations: RetryIdentityExpectations,
    ) {
        let job_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM product_creation_jobs")
            .fetch_one(db.pool())
            .await
            .unwrap();
        let product_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM product_conversations")
            .fetch_one(db.pool())
            .await
            .unwrap();
        let published_root_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*)
             FROM product_creation_jobs
             WHERE published_product_id IS NOT NULL
                OR published_conversation_id IS NOT NULL",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        let transcript_count_for_request_publication: i64 = sqlx::query_scalar(
            "SELECT COUNT(*)
             FROM conversations
             WHERE product_conversation_id = (
                 SELECT published_product_id
                 FROM product_creation_jobs
                 WHERE request_id = ?1
             )",
        )
        .bind(request_id)
        .fetch_one(db.pool())
        .await
        .unwrap();
        let (committed_objective_count, queued_objective_count) =
            global_objective_artifact_counts(db, request_id).await;
        let published = db
            .get_product_creation_job(request_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(job_count, expectations.job_count);
        assert_eq!(product_count, expectations.product_count);
        assert_eq!(published_root_count, 1);
        assert_eq!(transcript_count_for_request_publication, 1);
        assert_eq!(
            published.published_product_id,
            Some(expected_published_product_id.clone())
        );
        assert_eq!(
            published.published_conversation_id.as_deref(),
            Some(expected_published_conversation_id)
        );
        assert_eq!(
            committed_objective_count + queued_objective_count,
            expectations.total_delta
        );
        if expectations.artifact_delta != (-1, -1) {
            assert_eq!(
                (committed_objective_count, queued_objective_count),
                expectations.artifact_delta
            );
        }
    }

    async fn exhaust_delivery_into_failed_state(
        db: &Database,
        manager: &Arc<RuntimeManager>,
        request_id: &str,
        conversation_id: &str,
        accepted_product_id: &phoenix_core::domain::close::ProductConversationId,
        first_claim_time: chrono::DateTime<chrono::Utc>,
    ) -> chrono::DateTime<chrono::Utc> {
        let mut claim_time = first_claim_time;
        let mut last_delivery_lease_until = first_claim_time;
        for attempt in 1..=4 {
            let delivery =
                claim_delivery_attempt(db, request_id, &attempt.to_string(), claim_time).await;
            assert_eq!(
                delivery.job.published_product_id,
                Some(accepted_product_id.clone())
            );
            assert_eq!(
                delivery.job.published_conversation_id.as_deref(),
                Some(conversation_id)
            );

            let error = Box::pin(deliver_product_creation_objective(manager, &delivery))
                .await
                .unwrap_err();
            assert_eq!(error, "steering queue is full");
            last_delivery_lease_until = delivery.claim.lease_until;

            let expected_status = if attempt < 4 {
                "delivery_pending"
            } else {
                "delivery_failed"
            };
            let job = assert_retry_identity_job_state(
                db,
                request_id,
                conversation_id,
                accepted_product_id,
                expected_status,
            )
            .await;

            if attempt < 4 {
                claim_time = job
                    .delivery_retry_at
                    .expect("retryable delivery failure schedules a next attempt")
                    + chrono::Duration::seconds(1);
            } else {
                assert!(job.delivery_retry_at.is_none());
            }
        }
        last_delivery_lease_until
    }

    async fn create_non_target_conversation_and_enqueue(
        db: &Database,
        manager: &Arc<RuntimeManager>,
        cwd: &str,
    ) {
        let other_conversation_id = "conv-retry-other";
        db.create_conversation(
            other_conversation_id,
            other_conversation_id,
            cwd,
            true,
            None,
            None,
        )
        .await
        .unwrap();
        manager
            .enqueue_steer_message(
                other_conversation_id,
                Event::SteerMessage {
                    origin: phoenix_core::domain::db_schema::InputOrigin::SystemGenerated,
                    text: "non-target".to_string(),
                    llm_text: None,
                    images: Vec::new(),
                    files: Vec::new(),
                    message_id: "non-target-message".to_string(),
                    user_agent: None,
                    skill_invocation: None,
                },
                "product-create:non-target",
            )
            .await
            .unwrap();
    }

    async fn assert_explicit_retry_publish_success(
        db: &Database,
        manager: &Arc<RuntimeManager>,
        request_id: &str,
        conversation_id: &str,
        accepted_product_id: &phoenix_core::domain::close::ProductConversationId,
        claim_time: chrono::DateTime<chrono::Utc>,
    ) {
        let retried = claim_delivery_attempt(db, request_id, "explicit-retry", claim_time).await;
        assert_eq!(retried.job.product_conversation_id, *accepted_product_id);
        assert_eq!(
            retried.job.published_product_id,
            Some(accepted_product_id.clone())
        );
        assert_eq!(
            retried.job.published_conversation_id.as_deref(),
            Some(conversation_id)
        );

        Box::pin(deliver_product_creation_objective(manager, &retried))
            .await
            .unwrap();
        let origin: String = sqlx::query_scalar("SELECT origin_kind FROM steering_messages WHERE message_id = ?1 UNION ALL SELECT origin_kind FROM messages WHERE message_id = ?1 LIMIT 1").bind(request_id).fetch_one(db.pool()).await.unwrap();
        assert_eq!(origin, "user_api");

        let completed = db
            .get_product_creation_job(request_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(completed.status, "published");
        assert_eq!(completed.product_conversation_id, *accepted_product_id);
        assert_eq!(
            completed.published_product_id,
            Some(accepted_product_id.clone())
        );
        assert_eq!(
            completed.published_conversation_id.as_deref(),
            Some(conversation_id)
        );
        assert_eq!(
            db.get_steering_acceptance_fingerprint(conversation_id, request_id)
                .await
                .unwrap(),
            Some(phoenix_db::SteeringAcceptanceFingerprint::Exact(
                "product-create:req-retry-identities".to_string()
            ))
        );
    }

    #[tokio::test]
    async fn internal_creation_objective_is_literal_agent_content() {
        let root = tempfile::tempdir().unwrap();
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation(
            "literal-objective",
            "literal-objective",
            root.path().to_str().unwrap(),
            false,
            None,
            None,
        )
        .await
        .unwrap();
        let source_instance_id = db.instance_id().await.unwrap();
        let manager = test_runtime_manager(db);
        let origin = crate::db::ProductCreationOrigin::InternalConversation {
            source_instance_id,
            product_conversation_id: "coordinator-product".parse().unwrap(),
            transcript_id: "coordinator-source".into(),
            source_call: phoenix_core::domain::db_schema::SourceToolCall {
                message_id: "assistant-source".into(),
                tool_use_id: "create-call".into(),
            },
        };
        // Neither an @token nor an unknown /skill may be expanded for agent input.
        for objective in ["Read @missing.md", "/missing-skill @missing.md"] {
            let expanded = expand_product_creation_objective(
                &manager,
                "literal-objective",
                objective,
                &origin,
            )
            .await
            .unwrap();
            assert_eq!(expanded.display_text, objective);
            assert!(expanded.llm_text.is_none());
            assert!(expanded.skill_invocation.is_none());
        }
    }

    #[tokio::test]
    async fn historical_internal_creation_objective_remains_literal() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("notes.md"), "should not be expanded").unwrap();
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation(
            "historical-objective",
            "historical-objective",
            root.path().to_str().unwrap(),
            false,
            None,
            None,
        )
        .await
        .unwrap();
        db.accept_product_creation(
            "historical-origin-job",
            &ProductCreationIntent {
                origin: crate::db::ProductCreationOrigin::UserApi,
                cwd: root.path().to_string_lossy().into_owned(),
                objective: "Read @notes.md".into(),
                model: None,
                effort: None,
                images: Vec::new(),
                llm_language: LlmLanguage::Caveman,
            },
        )
        .await
        .unwrap();
        sqlx::query("UPDATE product_creation_jobs SET objective_origin = 'historical_internal', objective_source_product_conversation_id = 'pc_source', objective_source_transcript_id = char(9), objective_source_message_id = 'message', objective_source_tool_use_id = 'tool' WHERE request_id = 'historical-origin-job'")
            .execute(db.pool())
            .await
            .unwrap();
        let origin = db
            .get_product_creation_job("historical-origin-job")
            .await
            .unwrap()
            .unwrap()
            .intent
            .origin;
        assert!(matches!(
            &origin,
            crate::db::ProductCreationOrigin::HistoricalInternal(_)
        ));
        let manager = test_runtime_manager(db);

        let expanded = expand_product_creation_objective(
            &manager,
            "historical-objective",
            "Read @notes.md",
            &origin,
        )
        .await
        .unwrap();
        assert_eq!(expanded.display_text, "Read @notes.md");
        assert!(expanded.llm_text.is_none());
        assert!(expanded.skill_invocation.is_none());
    }

    #[tokio::test]
    async fn initial_objective_expands_against_published_conversation_root() {
        let root = tempfile::tempdir().unwrap();
        let skill_dir = root.path().join(".claude/skills/probe-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: probe-skill\ndescription: Probe\n---\npublished root contents $ARGUMENTS",
        )
        .unwrap();
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation(
            "expanded-product-objective",
            "expanded-product-objective",
            root.path().to_string_lossy().as_ref(),
            true,
            None,
            None,
        )
        .await
        .unwrap();
        let manager = RuntimeManager::new(
            db,
            Arc::new(ModelRegistry::new_empty()),
            PlatformCapability::None {
                details: "test".into(),
            },
            Arc::new(McpClientManager::new()),
            None,
        );

        for origin in [
            crate::db::ProductCreationOrigin::UnknownHistorical,
            crate::db::ProductCreationOrigin::UserApi,
            crate::db::ProductCreationOrigin::SlackGateway {
                request_id: "slack-request".into(),
            },
        ] {
            let expanded = expand_product_creation_objective(
                &manager,
                "expanded-product-objective",
                "/probe-skill read @notes.md",
                &origin,
            )
            .await
            .unwrap();
            assert_eq!(expanded.display_text, "/probe-skill read @notes.md");
            assert!(expanded
                .llm_text
                .as_deref()
                .is_some_and(|text| text.contains("published root contents read @notes.md")));
        }
    }

    #[tokio::test]
    async fn exact_fingerprint_replay_with_stale_delivery_claim_remains_pending() {
        let db = Database::open_in_memory().await.unwrap();
        let intent = ProductCreationIntent {
            origin: phoenix_db::ProductCreationOrigin::UserApi,
            cwd: "/repo/a".to_string(),
            objective: "deliver objective".to_string(),
            model: None,
            effort: None,
            images: Vec::new(),
            llm_language: LlmLanguage::Caveman,
        };
        db.accept_product_creation("req-stale-delivery", &intent)
            .await
            .unwrap();
        let provisioning = db
            .claim_product_creation(
                "req-stale-delivery",
                "provisioner",
                "provision-token",
                chrono::Utc::now(),
                chrono::Duration::minutes(5),
            )
            .await
            .unwrap()
            .unwrap();
        let conversation_id = "conv-stale-delivery";
        let conversation = stale_delivery_conversation(
            conversation_id,
            &provisioning.job.product_conversation_id,
            &intent.cwd,
        );
        advance_product_metadata_for_test(&db, &provisioning).await;
        assert!(db
            .publish_product_creation_atomically(&ProductCreationPublishInput {
                request_id: "req-stale-delivery".to_string(),
                claim: provisioning.claim.clone(),
                conversation,
                authority_kind: AuthorityKind::Work,
                environment: EnvironmentContext::UnownedCwd {
                    cwd: intent.cwd.clone(),
                },
                git_publication: None,
            })
            .await
            .unwrap());
        let delivery = crate::db::ClaimedProductCreationJob {
            claim: provisioning.claim,
            job: db
                .get_product_creation_job("req-stale-delivery")
                .await
                .unwrap()
                .unwrap(),
        };
        db.append_product_creation_steering_entry(
            &delivery,
            &phoenix_core::domain::sm_event::SteerEntry {
                origin: phoenix_core::domain::db_schema::InputOrigin::UnknownHistorical,
                text: intent.objective.clone(),
                llm_text: None,
                images: Vec::new(),
                files: Vec::new(),
                message_id: "req-stale-delivery".to_string(),
                user_agent: None,
                skill_invocation: None,
            },
            "product-create:req-stale-delivery",
        )
        .await
        .unwrap();
        let pending = db
            .get_product_creation_job("req-stale-delivery")
            .await
            .unwrap()
            .unwrap();
        let stale_delivery = crate::db::ClaimedProductCreationJob {
            claim: crate::db::ProductCreationClaim {
                worker_id: pending.claim_worker_id.clone().unwrap(),
                token: "stale-token".to_string(),
                generation: pending.claim_generation,
                lease_until: pending.claim_lease_until.unwrap(),
            },
            job: pending,
        };
        let manager = test_runtime_manager(db.clone());

        let error = Box::pin(deliver_product_creation_objective(
            &manager,
            &stale_delivery,
        ))
        .await
        .unwrap_err();
        assert_eq!(error, "product creation claim was lost before processing");
        let still_pending = db
            .get_product_creation_job("req-stale-delivery")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(still_pending.status, "delivery_pending");
        assert!(db.get_conversation(conversation_id).await.unwrap().archived);
    }

    #[tokio::test]
    async fn explicit_retry_after_queue_full_reuses_published_identities_without_duplicate_aggregate(
    ) {
        let root = tempfile::tempdir().unwrap();
        let cwd = root.path().to_string_lossy().to_string();
        let db = Database::open_in_memory().await.unwrap();
        let request_id = "req-retry-identities";
        let conversation_id = "conv-retry-identities";
        let manager = test_runtime_manager(db.clone());
        let (_intent, accepted_product_id, publish_claim_lease_until) =
            seed_retry_identity_delivery(
                &db,
                &cwd,
                request_id,
                conversation_id,
                "deliver objective",
            )
            .await;
        let baseline_artifact_counts = global_objective_artifact_counts(&db, request_id).await;
        assert_eq!(baseline_artifact_counts, (0, 0));
        let baseline_job_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM product_creation_jobs")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let baseline_product_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM product_conversations")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let capacity_full = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let prior_capacity =
            db.test_block_steering_conversation(conversation_id, Arc::clone(&capacity_full));
        assert!(prior_capacity.is_none());

        create_non_target_conversation_and_enqueue(&db, &manager, &cwd).await;

        let last_delivery_lease_until = Box::pin(exhaust_delivery_into_failed_state(
            &db,
            &manager,
            request_id,
            conversation_id,
            &accepted_product_id,
            publish_claim_lease_until + chrono::Duration::seconds(1),
        ))
        .await;

        assert_retry_identity_counts(
            &db,
            request_id,
            &accepted_product_id,
            conversation_id,
            RetryIdentityExpectations {
                job_count: baseline_job_count,
                product_count: baseline_product_count + 1,
                artifact_delta: baseline_artifact_counts,
                total_delta: 0,
            },
        )
        .await;
        assert!(db
            .get_steering_acceptance_fingerprint(conversation_id, request_id)
            .await
            .unwrap()
            .is_none());
        capacity_full.store(false, std::sync::atomic::Ordering::SeqCst);
        assert!(db
            .test_unblock_steering_conversation(conversation_id)
            .is_some());
        assert!(db
            .retry_failed_product_creation_delivery(request_id)
            .await
            .unwrap());
        Box::pin(assert_explicit_retry_publish_success(
            &db,
            &manager,
            request_id,
            conversation_id,
            &accepted_product_id,
            last_delivery_lease_until + chrono::Duration::seconds(1),
        ))
        .await;
        assert_retry_identity_counts(
            &db,
            request_id,
            &accepted_product_id,
            conversation_id,
            RetryIdentityExpectations {
                job_count: baseline_job_count,
                product_count: baseline_product_count + 1,
                artifact_delta: (-1, -1),
                total_delta: 1,
            },
        )
        .await;
    }
}

#[cfg(test)]
mod product_creation_path_classification_tests {
    use super::*;

    #[test]
    fn newly_created_standalone_directory_has_no_repository() {
        let root = tempfile::tempdir().unwrap();
        let cwd = root.path().join("standalone");
        std::fs::create_dir(&cwd).unwrap();

        assert_eq!(discover_product_repository(&cwd).unwrap(), None);
    }

    #[test]
    fn localized_environment_still_classifies_standalone_directory_as_direct() {
        let root = tempfile::tempdir().unwrap();
        let cwd = root.path().join("standalone");
        std::fs::create_dir(&cwd).unwrap();

        let localized_probe = crate::git_ops::run_git_with_env(
            &cwd,
            &["rev-parse", "--show-toplevel"],
            &[("LC_ALL", "de_DE.UTF-8")],
        );
        assert!(localized_probe.is_err());
        assert_eq!(discover_product_repository(&cwd).unwrap(), None);
    }

    #[test]
    fn newly_created_subdirectory_in_repository_is_git_backed() {
        let repo = tempfile::tempdir().unwrap();
        crate::git_ops::run_git(repo.path(), &["init"]).unwrap();
        let cwd = repo.path().join("new-product");
        std::fs::create_dir(&cwd).unwrap();

        let discovered = discover_product_repository(&cwd).unwrap().unwrap();
        assert_eq!(Path::new(&discovered), repo.path().canonicalize().unwrap());
    }
}

#[cfg(test)]
mod product_creation_staging_ownership_tests {
    use super::*;

    pub(super) fn initialized_repo() -> tempfile::TempDir {
        let repo = tempfile::tempdir().unwrap();
        crate::git_ops::run_git(repo.path(), &["init"]).unwrap();
        crate::git_ops::run_git(repo.path(), &["config", "user.email", "test@example.com"])
            .unwrap();
        crate::git_ops::run_git(repo.path(), &["config", "user.name", "Test User"]).unwrap();
        std::fs::write(repo.path().join("README"), "test").unwrap();
        crate::git_ops::run_git(repo.path(), &["add", "README"]).unwrap();
        crate::git_ops::run_git(repo.path(), &["commit", "-m", "initial"]).unwrap();
        repo
    }

    fn add_detached_worktree(repo: &Path, path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        crate::git_ops::run_git(
            repo,
            &[
                "worktree",
                "add",
                "--detach",
                path.to_string_lossy().as_ref(),
                "HEAD",
            ],
        )
        .unwrap();
    }

    #[test]
    fn occupied_ordinary_directory_is_preserved_before_ownership_proof() {
        let repo = initialized_repo();
        let path = repo.path().join(".phoenix/worktrees/replaced");
        std::fs::create_dir_all(&path).unwrap();
        let user_file = path.join("user-owned");
        std::fs::write(&user_file, "preserve me").unwrap();
        let oid = crate::git_ops::run_git(repo.path(), &["rev-parse", "HEAD"]).unwrap();

        assert!(verify_existing_product_creation_staging(
            repo.path().to_str().unwrap(),
            &path,
            oid.trim(),
            Some("owner-token"),
        )
        .is_err());
        assert_eq!(std::fs::read_to_string(user_file).unwrap(), "preserve me");
    }

    #[test]
    fn standalone_checkout_is_not_adopted_for_reserved_repository() {
        let repo = initialized_repo();
        let foreign = initialized_repo();
        let oid = crate::git_ops::run_git(foreign.path(), &["rev-parse", "HEAD"]).unwrap();
        assert!(verify_existing_product_creation_staging(
            repo.path().to_str().unwrap(),
            foreign.path(),
            &oid,
            Some("owner-token"),
        )
        .is_err());
    }

    #[test]
    #[cfg(unix)]
    fn adoption_requires_the_exact_registration_at_the_recorded_location() {
        let repo = initialized_repo();
        let original = repo.path().join(".phoenix/worktrees/recorded");
        let moved = original.with_file_name("moved");
        add_detached_worktree(repo.path(), &original);
        write_test_owner_marker(&original, "owner-token");
        let git = |args: &[&str]| crate::git_ops::run_git(repo.path(), args).unwrap();
        let oid = git(&["rev-parse", "HEAD"]);
        let can_adopt_original = || {
            verify_existing_product_creation_staging(
                repo.path().to_str().unwrap(),
                &original,
                &oid,
                Some("owner-token"),
            )
            .is_ok()
        };
        assert!(can_adopt_original());
        git(&[
            "worktree",
            "move",
            original.to_str().unwrap(),
            moved.to_str().unwrap(),
        ]);
        let registrations = git(&["worktree", "list", "--porcelain", "-z"]);
        std::os::unix::fs::symlink(&moved, &original).unwrap();
        assert!(!can_adopt_original());
        std::fs::remove_file(&original).unwrap();
        std::fs::create_dir(&original).unwrap();
        std::fs::copy(moved.join(".git"), original.join(".git")).unwrap();
        assert!(!can_adopt_original());
        assert_eq!(
            git(&["worktree", "list", "--porcelain", "-z"]),
            registrations
        );
        assert!(moved.join(".git").is_file());
    }

    #[test]
    fn token_only_staging_marker_cannot_prove_checkout_continuity() {
        let repo = initialized_repo();
        let path = repo.path().join(".phoenix/worktrees/recorded");
        add_detached_worktree(repo.path(), &path);
        let git = |args: &[&str]| crate::git_ops::run_git(repo.path(), args).unwrap();
        let oid = git(&["rev-parse", "HEAD"]);
        let registrations = git(&["worktree", "list", "--porcelain", "-z"]);
        let marker = product_creation_owner_marker_path(&path).unwrap();
        std::fs::write(&marker, b"owner-token").unwrap();

        let root = repo.path().to_str().unwrap();
        assert!(verify_product_creation_checkout(root, &path, &oid).is_ok());
        let error =
            verify_existing_product_creation_staging(root, &path, &oid, Some("owner-token"))
                .unwrap_err();
        assert_eq!(
            error,
            "existing staging occupant did not match durable ownership"
        );
        assert_eq!(std::fs::read(&marker).unwrap(), b"owner-token");
        assert_eq!(std::fs::read(path.join("README")).unwrap(), b"test");
        assert_eq!(
            git(&["worktree", "list", "--porcelain", "-z"]),
            registrations
        );
    }

    #[test]
    #[cfg(unix)]
    fn adoption_rejects_a_copied_pointer_even_with_the_same_backpointer() {
        let repo = initialized_repo();
        let original = product_worktree_path(repo.path(), "recorded").unwrap();
        let moved = original.with_file_name("moved");
        add_detached_worktree(repo.path(), &original);
        write_test_owner_marker(&original, "owner-token");
        let root = repo.path().to_str().unwrap();
        let oid = crate::git_ops::run_git(repo.path(), &["rev-parse", "HEAD"]).unwrap();
        std::fs::rename(&original, &moved).unwrap();
        std::fs::create_dir(&original).unwrap();
        std::fs::copy(moved.join(".git"), original.join(".git")).unwrap();

        assert!(verify_product_creation_checkout(root, &original, &oid).is_ok());
        assert!(verify_existing_product_creation_staging(
            root,
            &original,
            &oid,
            Some("owner-token"),
        )
        .is_err());
        assert!(moved.join(".git").is_file());
    }

    #[test]
    fn publication_rejects_attached_head_with_unchanged_checkout_marker() {
        let repo = initialized_repo();
        let path = product_worktree_path(repo.path(), "staged").unwrap();
        add_detached_worktree(repo.path(), &path);
        write_test_owner_marker(&path, "owner-token");
        let facts = crate::db::ProductCreationGitPublicationFacts {
            repository_root: repo.path().to_str().unwrap().to_string(),
            exact_checkout_oid: crate::git_ops::run_git(&path, &["rev-parse", "HEAD"]).unwrap(),
            ownership_token: "owner-token".to_string(),
            checkout_fingerprint: product_creation_checkout_fingerprint(&path).unwrap(),
        };
        assert!(lock_product_creation_publication(&path, &facts).is_ok());
        crate::git_ops::run_git(&path, &["switch", "-c", "attached-after-staging"]).unwrap();
        assert_eq!(
            product_creation_checkout_fingerprint(&path).unwrap(),
            facts.checkout_fingerprint
        );
        assert!(lock_product_creation_publication(&path, &facts).is_err());
        assert!(path.join(".git").is_file());
    }

    #[tokio::test]
    async fn even_clean_owned_staging_and_registration_are_retained() {
        let repo = initialized_repo();
        let path = repo.path().join(".phoenix/worktrees/owned");
        add_detached_worktree(repo.path(), &path);
        write_test_owner_marker(&path, "owner-token");

        let outcome = reconcile_unpublished_staging(path.to_str(), true).await;

        assert!(outcome.is_err());
        assert!(path.join(".git").is_file());
        assert_eq!(
            read_product_creation_owner_marker(&path)
                .unwrap()
                .as_deref(),
            Some("owner-token"),
        );
    }

    #[tokio::test]
    async fn owned_staging_with_ignored_content_is_not_removed() {
        let repo = initialized_repo();
        let path = product_worktree_path(repo.path(), "owned").unwrap();
        add_detached_worktree(repo.path(), &path);
        write_test_owner_marker(&path, "owner-token");
        let common = git_common_dir_for_repository_root(repo.path()).unwrap();
        let exclude = Path::new(&common).join("info/exclude");
        std::fs::write(&exclude, "ignored-data\n").unwrap();
        let ignored = path.join("ignored-data");
        std::fs::write(&ignored, "preserve me").unwrap();

        let outcome = reconcile_unpublished_staging(path.to_str(), true).await;

        assert!(outcome.is_err());
        assert_eq!(std::fs::read_to_string(ignored).unwrap(), "preserve me");
        assert_eq!(std::fs::read_to_string(exclude).unwrap(), "ignored-data\n");
        assert!(
            crate::git_ops::run_git(repo.path(), &["status", "--porcelain"])
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn owned_staging_with_hidden_tracked_changes_is_not_removed() {
        for flag in ["--assume-unchanged", "--skip-worktree"] {
            let repo = initialized_repo();
            let path = product_worktree_path(repo.path(), "owned").unwrap();
            add_detached_worktree(repo.path(), &path);
            write_test_owner_marker(&path, "owner-token");
            crate::git_ops::run_git(&path, &["update-index", flag, "README"]).unwrap();
            std::fs::write(path.join("README"), "preserve me").unwrap();

            let outcome = reconcile_unpublished_staging(path.to_str(), true).await;

            assert!(outcome.is_err());
            assert_eq!(
                std::fs::read_to_string(path.join("README")).unwrap(),
                "preserve me"
            );
            assert!(path.join(".git").is_file());
        }
    }

    #[tokio::test]
    async fn absent_path_does_not_clear_previous_materialization_authority() {
        let repo = initialized_repo();
        let path = product_worktree_path(repo.path(), "owned").unwrap();
        assert!(reconcile_unpublished_staging(path.to_str(), false)
            .await
            .is_ok());
        assert!(reconcile_unpublished_staging(path.to_str(), true)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn same_path_and_oid_replacement_is_preserved_as_ownership_conflict() {
        let repo = initialized_repo();
        let path = repo.path().join(".phoenix/worktrees/replaced");
        add_detached_worktree(repo.path(), &path);
        write_test_owner_marker(&path, "original-owner");
        crate::git_ops::run_git(
            repo.path(),
            &[
                "worktree",
                "remove",
                "--force",
                path.to_string_lossy().as_ref(),
            ],
        )
        .unwrap();
        add_detached_worktree(repo.path(), &path);
        let replacement_file = path.join("replacement-owned-by-user");
        std::fs::write(&replacement_file, "preserve me").unwrap();

        let outcome = reconcile_unpublished_staging(path.to_str(), true).await;

        assert!(outcome.is_err());
        assert_eq!(
            std::fs::read_to_string(replacement_file).unwrap(),
            "preserve me"
        );
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

    struct CountingOwner(Arc<std::sync::atomic::AtomicUsize>);

    impl CountingOwner {
        fn acquire(owners: &Arc<std::sync::atomic::AtomicUsize>) -> Self {
            owners.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            Self(Arc::clone(owners))
        }
    }

    impl Drop for CountingOwner {
        fn drop(&mut self) {
            self.0.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
        }
    }

    #[tokio::test]
    async fn continue_retains_owner_through_processing_then_claims_next_pass() {
        let owners = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let admission_owners = Arc::clone(&owners);
        let claim_owners = Arc::clone(&owners);
        let claims = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed_claims = Arc::clone(&claims);
        let (process_started_tx, process_started_rx) = tokio::sync::oneshot::channel();
        let (release_process_tx, release_process_rx) = tokio::sync::oneshot::channel();
        let mut process_started_tx = Some(process_started_tx);
        let mut release_process_rx = Some(release_process_rx);

        let drain = tokio::spawn(drain_claimed_jobs(
            move || Ok(CountingOwner::acquire(&admission_owners)),
            move || {
                assert_eq!(claim_owners.load(std::sync::atomic::Ordering::Acquire), 1);
                let claim_number =
                    observed_claims.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                async move {
                    if claim_number == 0 {
                        Ok(Some("job"))
                    } else {
                        Ok(None)
                    }
                }
            },
            move |_| {
                let process_started_tx = process_started_tx.take().unwrap();
                let release_process_rx = release_process_rx.take().unwrap();
                async move {
                    process_started_tx.send(()).unwrap();
                    release_process_rx.await.unwrap();
                    CreationDrainControl::Continue
                }
            },
        ));

        process_started_rx.await.unwrap();
        assert_eq!(owners.load(std::sync::atomic::Ordering::Acquire), 1);

        release_process_tx.send(()).unwrap();
        drain.await.unwrap().unwrap();

        assert_eq!(owners.load(std::sync::atomic::Ordering::Acquire), 0);
        assert_eq!(claims.load(std::sync::atomic::Ordering::Acquire), 2);
    }

    #[tokio::test]
    async fn stop_drain_retains_owner_through_processing_without_second_claim() {
        let owners = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let admission_owners = Arc::clone(&owners);
        let claim_owners = Arc::clone(&owners);
        let claims = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed_claims = Arc::clone(&claims);
        let (process_started_tx, process_started_rx) = tokio::sync::oneshot::channel();
        let (release_process_tx, release_process_rx) = tokio::sync::oneshot::channel();
        let mut process_started_tx = Some(process_started_tx);
        let mut release_process_rx = Some(release_process_rx);

        let drain = tokio::spawn(drain_claimed_jobs(
            move || Ok(CountingOwner::acquire(&admission_owners)),
            move || {
                assert_eq!(claim_owners.load(std::sync::atomic::Ordering::Acquire), 1);
                observed_claims.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                async { Ok(Some("job")) }
            },
            move |_| {
                let process_started_tx = process_started_tx.take().unwrap();
                let release_process_rx = release_process_rx.take().unwrap();
                async move {
                    process_started_tx.send(()).unwrap();
                    release_process_rx.await.unwrap();
                    CreationDrainControl::StopDrain
                }
            },
        ));

        process_started_rx.await.unwrap();
        assert_eq!(owners.load(std::sync::atomic::Ordering::Acquire), 1);

        release_process_tx.send(()).unwrap();
        drain.await.unwrap().unwrap();

        assert_eq!(owners.load(std::sync::atomic::Ordering::Acquire), 0);
        assert_eq!(claims.load(std::sync::atomic::Ordering::Acquire), 1);
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
    use std::time::{Duration, Instant};

    #[test]
    fn repository_mutation_lock_guard_is_send_sync_static() {
        fn assert_send_sync_static<T: Send + Sync + 'static>() {}
        assert_send_sync_static::<RepositoryMutationLock>();
    }

    #[test]
    fn expired_common_directory_deadline_does_not_create_lock_file() {
        let parent = tempfile::tempdir().unwrap();
        let nonexistent_common_dir = parent.path().join("not-created");
        let result = RepositoryMutationLock::acquire_common_directory_until(
            &nonexistent_common_dir,
            Instant::now(),
        );
        let Err((error, _)) = result else {
            panic!("expired common-directory lock unexpectedly acquired");
        };
        assert!(error.contains("timed out"), "{error}");
        assert!(!nonexistent_common_dir.exists());
        assert!(!nonexistent_common_dir
            .join("phoenix-creation.lock")
            .exists());
    }

    #[test]
    fn common_directory_contention_exhausts_the_supplied_deadline() {
        let temp = tempfile::tempdir().unwrap();
        let holder = RepositoryMutationLock::acquire_common_directory_until(
            temp.path(),
            Instant::now() + Duration::from_secs(30),
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_millis(100);
        let result = RepositoryMutationLock::acquire_common_directory_until(temp.path(), deadline);
        let Err((error, _)) = result else {
            panic!("contended repository lock unexpectedly acquired");
        };
        assert!(error.contains("timed out"), "{error}");
        assert!(Instant::now() >= deadline);
        // A coarse liveness guard catches an accidental fresh 30-second budget;
        // the live holder, not elapsed time, establishes lock contention.
        assert!(Instant::now() < deadline + Duration::from_secs(5));
        let contender = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(temp.path().join("phoenix-creation.lock"))
            .unwrap();
        assert!(contender.try_lock_exclusive().is_err());
        drop(holder);
        contender.try_lock_exclusive().unwrap();
        contender.unlock().unwrap();
    }

    #[test]
    fn common_directory_lock_serializes_holders_and_releases_on_same_path() {
        let repo = tempfile::tempdir().unwrap();
        let status = phoenix_core::git::command()
            .args(["init", "--quiet"])
            .current_dir(repo.path())
            .status()
            .unwrap();
        assert!(status.success());
        let (_, lock_path) =
            RepositoryMutationLock::open_file(repo.path(), Duration::from_secs(30)).unwrap();
        let common_dir = lock_path.parent().unwrap();

        let first = RepositoryMutationLock::acquire_common_directory_until(
            common_dir,
            Instant::now() + Duration::from_secs(30),
        )
        .unwrap();
        let contender = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lock_path)
            .unwrap();
        assert!(contender.try_lock_exclusive().is_err());
        drop(first);

        let second = RepositoryMutationLock::acquire_common_directory_until(
            common_dir,
            Instant::now() + Duration::from_secs(30),
        )
        .unwrap();
        assert!(contender.try_lock_exclusive().is_err());
        drop(second);
        contender.try_lock_exclusive().unwrap();
        contender.unlock().unwrap();
    }

    #[test]
    fn blocked_repository_mutation_lock_times_out_and_releases_waiter() {
        let repo = tempfile::tempdir().unwrap();
        let status = phoenix_core::git::command()
            .args(["init", "--quiet"])
            .current_dir(repo.path())
            .status()
            .unwrap();
        assert!(status.success());
        let first = RepositoryMutationLock::acquire(repo.path()).unwrap();
        let started = std::time::Instant::now();
        let result = RepositoryMutationLock::acquire_with_timeout(
            repo.path(),
            std::time::Duration::from_millis(100),
        );
        let Err((error, _)) = result else {
            panic!("blocked repository lock unexpectedly acquired");
        };
        assert!(error.contains("timed out"), "{error}");
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        drop(first);
        assert!(RepositoryMutationLock::acquire(repo.path()).is_ok());
    }

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
        let first = RepositoryMutationLock::acquire(std::path::Path::new(&repo_path)).unwrap();
        let (second, _) = RepositoryMutationLock::open_file(
            std::path::Path::new(&repo_path),
            std::time::Duration::from_secs(30),
        )
        .unwrap();

        assert!(second.try_lock_exclusive().is_err());
        drop(first);
        second.try_lock_exclusive().unwrap();
        second.unlock().unwrap();
    }
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

#[cfg(test)]
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
        AppError::TranscriptSnapshotChanged => (
            "transcript changed during opening observation read".to_string(),
            ErrorKind::ServerError,
        ),
        AppError::Forbidden(message) => (message, ErrorKind::Auth),
    }
}

#[cfg(test)]
#[test]
fn snapshot_instability_maps_to_server_error() {
    let (message, kind) = app_error_to_kind(AppError::TranscriptSnapshotChanged);
    assert_eq!(
        message,
        "transcript changed during opening observation read"
    );
    assert!(matches!(kind, ErrorKind::ServerError));
}

#[cfg(test)]
mod product_pin_tests {
    use super::strict_product_creation_pin;
    use std::path::Path;

    fn git(path: &Path, args: &[&str]) -> String {
        crate::git_ops::run_git(path, args).expect("git command")
    }

    #[test]
    fn origin_fetch_pins_advertised_default_without_mutating_refs_or_fetch_head() {
        let dir = tempfile::tempdir().unwrap();
        let bare = dir.path().join("origin.git");
        let seed = dir.path().join("seed");
        let checkout = dir.path().join("checkout");
        std::fs::create_dir_all(&seed).unwrap();
        git(&seed, &["init", "-b", "main"]);
        git(&seed, &["config", "user.email", "test@example.com"]);
        git(&seed, &["config", "user.name", "Test"]);
        std::fs::write(seed.join("README"), "initial\n").unwrap();
        git(&seed, &["add", "README"]);
        git(&seed, &["commit", "-m", "initial"]);
        let initial_oid = git(&seed, &["rev-parse", "HEAD"]);

        git(dir.path(), &["init", "--bare", bare.to_str().unwrap()]);
        git(&seed, &["remote", "add", "origin", bare.to_str().unwrap()]);
        git(&seed, &["push", "origin", "main", "HEAD:refs/heads/trunk"]);
        git(&bare, &["symbolic-ref", "HEAD", "refs/heads/main"]);
        git(
            dir.path(),
            &["clone", bare.to_str().unwrap(), checkout.to_str().unwrap()],
        );
        assert_eq!(
            git(&checkout, &["symbolic-ref", "refs/remotes/origin/HEAD"]).trim(),
            "refs/remotes/origin/main"
        );
        git(&checkout, &["fetch", "origin"]);
        git(&checkout, &["tag", "local-only"]);
        git(&checkout, &["config", "fetch.prune", "true"]);
        git(&checkout, &["config", "fetch.pruneTags", "true"]);
        git(
            &checkout,
            &[
                "config",
                "--replace-all",
                "remote.origin.fetch",
                "+refs/heads/*:refs/remotes/origin/*",
            ],
        );
        git(
            &checkout,
            &[
                "update-ref",
                "refs/remotes/origin/stale",
                initial_oid.trim(),
            ],
        );

        std::fs::write(seed.join("README"), "advanced default\n").unwrap();
        git(&seed, &["add", "README"]);
        git(&seed, &["commit", "-m", "advance advertised default"]);
        let advertised_oid = git(&seed, &["rev-parse", "HEAD"]);
        git(&seed, &["push", "origin", "HEAD:refs/heads/trunk"]);
        git(&bare, &["symbolic-ref", "HEAD", "refs/heads/trunk"]);
        assert_ne!(initial_oid.trim(), advertised_oid.trim());

        let fetch_head_path = git(&checkout, &["rev-parse", "--git-path", "FETCH_HEAD"]);
        let fetch_head_path = checkout.join(fetch_head_path.trim());
        let fetch_head_before = std::fs::read(&fetch_head_path).unwrap();
        assert!(
            !fetch_head_before.is_empty(),
            "the preceding fetch should leave real FETCH_HEAD data"
        );
        let refs_command = ["for-each-ref", "--format=%(refname) %(objectname) %(symref)"];
        let refs_before = git(&checkout, &refs_command);

        let (selected_oid, logical_base) = strict_product_creation_pin(&checkout).unwrap();

        assert_eq!(selected_oid, advertised_oid.trim());
        assert_eq!(logical_base, "trunk");
        assert_eq!(
            git(&checkout, &refs_command),
            refs_before,
            "exact-pin fetch must not apply configured refmaps or prune refs"
        );
        assert_eq!(
            std::fs::read(&fetch_head_path).unwrap(),
            fetch_head_before,
            "exact-pin fetch must preserve the prior FETCH_HEAD"
        );
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
        let repo = dir.path();
        git(repo, &["init", "-b", "main"]);
        git(repo, &["config", "user.email", "test@example.com"]);
        git(repo, &["config", "user.name", "Test"]);
        git(repo, &["commit", "--allow-empty", "-m", "cached default"]);
        let missing = repo.join("missing-origin.git");
        git(repo, &["remote", "add", "origin", missing.to_str().unwrap()]);
        git(repo, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
        git(
            repo,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/main",
            ],
        );
        let cached = git(repo, &["rev-parse", "refs/remotes/origin/HEAD^{commit}"]);
        assert_eq!(cached, git(repo, &["rev-parse", "refs/heads/main^{commit}"]));

        let error = strict_product_creation_pin(repo).unwrap_err();
        assert!(
            error.starts_with("could not discover origin default:"),
            "{error}"
        );
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
    fn ordinary_creation_uses_product_default_not_auxiliary_cheap_model() {
        let registry = ModelRegistry::new(&phoenix_llm::LlmConfig {
            openai_api_key: Some("test-key".into()),
            ..Default::default()
        });
        assert_eq!(
            resolve_creation_model(&registry, None).unwrap(),
            "gpt-6.2-sol"
        );
        assert_ne!(
            registry.cheap_model_id_for_provider("gpt-6.2-sol"),
            "gpt-6.2-sol"
        );
    }

    #[test]
    fn auto_creation_uses_direct_default_without_repository() {
        let registry = registry();
        assert_eq!(
            resolve_creation_model(&registry, None).unwrap(),
            registry.default_model_id()
        );
    }

    #[test]
    fn retired_creation_job_model_resolves_to_current_route() {
        let registry = ModelRegistry::new(&phoenix_llm::LlmConfig {
            openai_api_key: Some("test-key".into()),
            ..Default::default()
        });
        assert_eq!(
            resolve_creation_model(&registry, Some("gpt-5.3-codex")).unwrap(),
            "gpt-5.6-sol"
        );
    }

    #[test]
    fn explicit_model_wins_over_mode_defaults() {
        let registry = registry();
        assert_eq!(
            resolve_creation_model(&registry, Some("mock")).unwrap(),
            "mock"
        );
    }
}

/// `task-{id}-{slug}`, or `task-{id}` when the title has no ASCII alphanumerics.
#[cfg(test)]
fn approved_task_branch_name(task_id: &str, task_title: &str) -> String {
    let slug = slugify_label(task_title);
    if slug.is_empty() {
        format!("task-{task_id}")
    } else {
        format!("task-{task_id}-{slug}")
    }
}

#[cfg(test)]
mod approved_task_branch_tests {
    use super::approved_task_branch_name;

    #[test]
    fn branch_name_has_no_dangling_separator() {
        assert_eq!(
            approved_task_branch_name("123", "Fix login"),
            "task-123-fix-login"
        );
        assert_eq!(approved_task_branch_name("123", "日本語"), "task-123");
        assert!(approved_task_branch_name("123", &"x".repeat(500)).len() < 120);
    }
}
