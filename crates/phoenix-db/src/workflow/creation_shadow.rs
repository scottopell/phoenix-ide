//! Normalized diagnostic persistence for the conversation-creation shadow profile.
//!
//! The adapter is invoked only after the authoritative creation-job commit. Its transaction owns
//! only workflow diagnostics; failures are returned and never update `conversation_creation_jobs`.

use chrono::{DateTime, Utc};
use phoenix_workflow::{
    creation_profile::{
        self, AuthoritativeCreationOracle, AuthoritativeCreationStage, AuthoritativeCreationStatus,
        CapabilityAvailability, CompensationPrediction, CompletionPrediction, CreationCapabilities,
        CreationEffectIntent, CreationEvent, CreationProjectionStatus, EffectPrediction,
    },
    SemanticAuthority,
};
use sqlx::{Row, Sqlite, Transaction};

use super::{
    DurableCodecRef, DurableProtocolSelectionRegistration, WorkflowRepository,
    WorkflowRepositoryError, WorkflowRepositoryResult,
};

pub const SELECTION_ID: &str = "conversation-creation-shadow-v1";
const SELECTOR_IDENTITY: &str = "phoenix.conversation-creation.shadow";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreationShadowConfig {
    pub shadow_workflow_id: String,
    pub authoritative_anchor_workflow_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreationShadowPersistence {
    Disabled,
    Enabled(CreationShadowConfig),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreationShadowEvidence {
    ProjectionStatus(CreationProjectionStatus),
    UserProjection {
        status: CreationProjectionStatus,
        capabilities: CreationCapabilities,
        hidden: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreationShadowPersistOutcome {
    Disabled,
    Created,
    Updated,
}

#[derive(Debug)]
pub struct CreationShadowAdapter<'a> {
    repository: &'a WorkflowRepository,
    persistence: &'a CreationShadowPersistence,
}

impl<'a> CreationShadowAdapter<'a> {
    #[must_use]
    pub const fn new(
        repository: &'a WorkflowRepository,
        persistence: &'a CreationShadowPersistence,
    ) -> Self {
        Self {
            repository,
            persistence,
        }
    }

    /// Replaces the bounded shadow graph and typed projection derived from a committed oracle.
    ///
    /// `observed` is independently observed diagnostic evidence. Equality resolves an active
    /// divergence; inequality opens or refreshes one without accumulating duplicate active rows.
    ///
    /// # Errors
    /// Returns validation or `SQLite` errors to the caller; the authoritative job is never updated.
    pub async fn persist_after_authoritative_commit(
        &self,
        oracle: &AuthoritativeCreationOracle,
        observed: CreationShadowEvidence,
        projected_at: DateTime<Utc>,
    ) -> WorkflowRepositoryResult<CreationShadowPersistOutcome> {
        let CreationShadowPersistence::Enabled(config) = self.persistence else {
            return Ok(CreationShadowPersistOutcome::Disabled);
        };
        if config.shadow_workflow_id == config.authoritative_anchor_workflow_id {
            return Err(WorkflowRepositoryError::InvalidPlan(
                "creation shadow and authoritative anchor ids must differ",
            ));
        }
        self.ensure_protocol_selection(projected_at).await?;
        let domain = creation_profile::adapt_authoritative_creation(
            phoenix_workflow::WorkflowId(1),
            phoenix_workflow::WorkflowId(2),
            oracle,
        )
        .map_err(|_| WorkflowRepositoryError::InvalidPlan("invalid creation shadow projection"))?;

        let mut tx = self.repository.pool().begin().await?;
        verify_authoritative_job(&mut tx, oracle).await?;
        if projection_is_newer(&mut tx, config, oracle.revision).await? {
            tx.rollback().await?;
            return Ok(CreationShadowPersistOutcome::Updated);
        }
        let existed: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM creation_shadow_bindings WHERE shadow_workflow_id = ?1)",
        )
        .bind(&config.shadow_workflow_id)
        .fetch_one(&mut *tx)
        .await?;
        upsert_anchor(&mut tx, config, oracle, projected_at).await?;
        upsert_shadow_workflow(&mut tx, config, oracle, &domain, projected_at).await?;
        replace_graph(&mut tx, config, oracle, &domain, projected_at).await?;
        upsert_projection(&mut tx, config, oracle, &domain, projected_at).await?;
        update_divergence(
            &mut tx,
            config,
            observed,
            domain.projection.status,
            domain.projection.capabilities,
            domain.projection.hidden,
            projected_at,
        )
        .await?;
        tx.commit().await?;
        Ok(if existed {
            CreationShadowPersistOutcome::Updated
        } else {
            CreationShadowPersistOutcome::Created
        })
    }

    async fn ensure_protocol_selection(&self, now: DateTime<Utc>) -> WorkflowRepositoryResult<()> {
        let existing: Option<(String, i64, i64, i64, i64)> = sqlx::query_as(
            "SELECT profile_id, protocol_version, accepting, runtime_acceptance_enabled, external_acceptance_enabled \
             FROM workflow_protocol_selections WHERE id = ?1",
        )
        .bind(SELECTION_ID)
        .fetch_optional(self.repository.pool())
        .await?;
        if let Some((profile, version, accepting, runtime, external)) = existing {
            if profile == creation_profile::PROFILE_ID
                && version == i64::from(creation_profile::PROTOCOL_VERSION)
                && accepting == 0
                && runtime == 0
                && external == 0
            {
                for family in [
                    "creation.snapshot",
                    "creation.authoritative_anchor",
                    "creation.diagnostic_sink",
                    "creation.event",
                    "creation.intent",
                    "creation.barrier",
                ] {
                    sqlx::query(
                        "INSERT OR IGNORE INTO workflow_profile_codecs \
                         (selection_id, codec_family, codec_version) VALUES (?1, ?2, ?3)",
                    )
                    .bind(SELECTION_ID)
                    .bind(family)
                    .bind(i64::from(creation_profile::PROTOCOL_VERSION))
                    .execute(self.repository.pool())
                    .await?;
                }
                return Ok(());
            }
            return Err(WorkflowRepositoryError::CorruptState(
                "creation shadow selection has incompatible capabilities".to_owned(),
            ));
        }
        self.repository
            .register_protocol_selection(&DurableProtocolSelectionRegistration {
                selection_id: SELECTION_ID.to_owned(),
                profile_id: creation_profile::PROFILE_ID.to_owned(),
                selector_identity: SELECTOR_IDENTITY.to_owned(),
                selector_version: 1,
                protocol_version: creation_profile::PROTOCOL_VERSION,
                authority: SemanticAuthority::LegacyProtocol,
                accepting: false,
                runtime_acceptance_enabled: false,
                external_acceptance_enabled: false,
                registered_at: now,
                drained_at: Some(now),
                supported_codecs: [
                    "creation.snapshot",
                    "creation.authoritative_anchor",
                    "creation.diagnostic_sink",
                    "creation.event",
                    "creation.intent",
                    "creation.barrier",
                ]
                .into_iter()
                .map(|family| DurableCodecRef {
                    family: family.to_owned(),
                    version: creation_profile::PROTOCOL_VERSION,
                })
                .collect(),
                executor_kinds: vec![],
            })
            .await
    }
}

async fn verify_authoritative_job(
    tx: &mut Transaction<'_, Sqlite>,
    oracle: &AuthoritativeCreationOracle,
) -> WorkflowRepositoryResult<()> {
    let row = sqlx::query(
        "SELECT conversation_id, generation, attempt, status, stage, shadow_projection_revision FROM conversation_creation_jobs WHERE id = ?1",
    )
    .bind(&oracle.intent.job_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| {
        WorkflowRepositoryError::CorruptState("creation shadow job is missing".to_owned())
    })?;
    if row.get::<String, _>("conversation_id") != oracle.intent.conversation_id
        || row.get::<i64, _>("generation")
            != i64::try_from(oracle.generation)
                .map_err(|_| WorkflowRepositoryError::GenerationOutOfRange(oracle.generation))?
        || row.get::<i64, _>("attempt") != i64::from(oracle.attempt)
        || row.get::<String, _>("status") != authoritative_status_sql(&oracle.status)
        || row.get::<String, _>("stage") != authoritative_stage_sql(oracle.stage)
        || row.get::<i64, _>("shadow_projection_revision") != to_i64(oracle.revision)?
    {
        return Err(WorkflowRepositoryError::CorruptState(
            "creation oracle does not match committed authoritative job".to_owned(),
        ));
    }
    Ok(())
}

async fn projection_is_newer(
    tx: &mut Transaction<'_, Sqlite>,
    config: &CreationShadowConfig,
    oracle_revision: u64,
) -> WorkflowRepositoryResult<bool> {
    let oracle_revision = to_i64(oracle_revision)?;
    let persisted: Option<i64> = sqlx::query_scalar(
        "SELECT oracle_revision FROM creation_shadow_projections WHERE shadow_workflow_id = ?1",
    )
    .bind(&config.shadow_workflow_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(persisted.is_some_and(|revision| revision > oracle_revision))
}

async fn upsert_anchor(
    tx: &mut Transaction<'_, Sqlite>,
    config: &CreationShadowConfig,
    oracle: &AuthoritativeCreationOracle,
    now: DateTime<Utc>,
) -> WorkflowRepositoryResult<()> {
    sqlx::query(
        "INSERT INTO workflows (id, profile_id, protocol_version, authority, execution_mode, \
         authoritative_workflow_id, protocol_selection_id, version, generation, status, \
         snapshot_codec_family, snapshot_codec_version, snapshot_payload, accepted_at) \
         VALUES (?1, ?2, ?3, 'legacy_protocol', 'authoritative', NULL, ?4, 0, ?5, ?6, \
         'creation.authoritative_anchor', 1, '{}', ?7) \
         ON CONFLICT(id) DO UPDATE SET generation = excluded.generation, status = excluded.status",
    )
    .bind(&config.authoritative_anchor_workflow_id)
    .bind(creation_profile::PROFILE_ID)
    .bind(i64::from(creation_profile::PROTOCOL_VERSION))
    .bind(SELECTION_ID)
    .bind(to_i64(oracle.generation)?)
    .bind(anchor_status_sql(&oracle.status))
    .bind(now.to_rfc3339())
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn upsert_shadow_workflow(
    tx: &mut Transaction<'_, Sqlite>,
    config: &CreationShadowConfig,
    oracle: &AuthoritativeCreationOracle,
    domain: &creation_profile::CreationShadowAdapter,
    now: DateTime<Utc>,
) -> WorkflowRepositoryResult<()> {
    sqlx::query(
        "INSERT INTO workflows (id, profile_id, protocol_version, authority, execution_mode, \
         authoritative_workflow_id, protocol_selection_id, version, generation, status, \
         snapshot_codec_family, snapshot_codec_version, snapshot_payload, accepted_at) \
         VALUES (?1, ?2, ?3, 'legacy_protocol', 'shadow', ?4, ?5, 1, ?6, ?7, \
         'creation.diagnostic_sink', 1, '{}', ?8) \
         ON CONFLICT(id) DO UPDATE SET generation = excluded.generation, status = excluded.status",
    )
    .bind(&config.shadow_workflow_id)
    .bind(creation_profile::PROFILE_ID)
    .bind(i64::from(creation_profile::PROTOCOL_VERSION))
    .bind(&config.authoritative_anchor_workflow_id)
    .bind(SELECTION_ID)
    .bind(to_i64(oracle.generation)?)
    .bind(status_sql(domain.plan.next_status))
    .bind(now.to_rfc3339())
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "INSERT INTO creation_shadow_bindings \
         (shadow_workflow_id, authoritative_workflow_id, creation_job_id) VALUES (?1, ?2, ?3) \
         ON CONFLICT(shadow_workflow_id) DO NOTHING",
    )
    .bind(&config.shadow_workflow_id)
    .bind(&config.authoritative_anchor_workflow_id)
    .bind(&oracle.intent.job_id)
    .execute(&mut **tx)
    .await?;
    let binding_matches: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM creation_shadow_bindings \
         WHERE shadow_workflow_id = ?1 AND authoritative_workflow_id = ?2 AND creation_job_id = ?3",
    )
    .bind(&config.shadow_workflow_id)
    .bind(&config.authoritative_anchor_workflow_id)
    .bind(&oracle.intent.job_id)
    .fetch_one(&mut **tx)
    .await?;
    if binding_matches != 1 {
        return Err(WorkflowRepositoryError::CorruptState(
            "creation shadow binding is immutable".to_owned(),
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
async fn replace_graph(
    tx: &mut Transaction<'_, Sqlite>,
    config: &CreationShadowConfig,
    oracle: &AuthoritativeCreationOracle,
    domain: &creation_profile::CreationShadowAdapter,
    now: DateTime<Utc>,
) -> WorkflowRepositoryResult<()> {
    sqlx::query(
        "DELETE FROM workflow_barrier_members WHERE barrier_id IN (SELECT id FROM workflow_barriers WHERE workflow_id = ?1)",
    )
    .bind(&config.shadow_workflow_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "DELETE FROM workflow_effect_dependencies WHERE effect_id IN (SELECT id FROM workflow_effects WHERE workflow_id = ?1) OR dependency_effect_id IN (SELECT id FROM workflow_effects WHERE workflow_id = ?1)",
    )
    .bind(&config.shadow_workflow_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query("DELETE FROM workflow_barriers WHERE workflow_id = ?1")
        .bind(&config.shadow_workflow_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query("DELETE FROM workflow_effects WHERE workflow_id = ?1")
        .bind(&config.shadow_workflow_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query("DELETE FROM workflow_transitions WHERE workflow_id = ?1")
        .bind(&config.shadow_workflow_id)
        .execute(&mut **tx)
        .await?;
    let transition_id = format!("{}:projection", config.shadow_workflow_id);
    let event_payload = redacted_event_payload(&domain.plan.event);
    sqlx::query(
        "INSERT INTO workflow_transitions (id, workflow_id, from_version, to_version, generation, \
         event_codec_family, event_codec_version, event_payload, committed_at) \
         VALUES (?1, ?2, 0, 1, ?3, 'creation.event', 1, ?4, ?5)",
    )
    .bind(&transition_id)
    .bind(&config.shadow_workflow_id)
    .bind(to_i64(oracle.generation)?)
    .bind(event_payload)
    .bind(now.to_rfc3339())
    .execute(&mut **tx)
    .await?;
    for effect in &domain.plan.effects {
        sqlx::query(
            "INSERT INTO workflow_effects (id, workflow_id, declaring_transition_id, \
             declared_workflow_version, generation, family, kind, codec_family, codec_version, role, \
             ambiguity_policy, intent_payload, status, pending_reconciliation, next_eligible_at, destructive_resource) \
             VALUES (?1, ?2, ?3, 1, ?4, ?5, ?6, ?7, ?8, ?9, ?10, '{}', 'blocked', 0, NULL, ?11)",
        )
        .bind(effect_db_id(config, effect.effect_id.0))
        .bind(&config.shadow_workflow_id)
        .bind(&transition_id)
        .bind(to_i64(effect.generation.0)?)
        .bind(effect.family)
        .bind(effect.kind)
        .bind(effect.codec.family)
        .bind(i64::from(effect.codec.version))
        .bind(role_sql(effect.role))
        .bind(ambiguity_sql(effect.ambiguity))
        .bind(effect.destructive_resource)
        .execute(&mut **tx)
        .await?;
        insert_typed_effect_intent(tx, config, effect.effect_id.0, &effect.intent).await?;
    }
    for dependency in &domain.plan.dependencies {
        sqlx::query(
            "INSERT INTO workflow_effect_dependencies (effect_id, dependency_effect_id) VALUES (?1, ?2)",
        )
        .bind(effect_db_id(config, dependency.effect_id.0))
        .bind(effect_db_id(config, dependency.depends_on_effect_id.0))
        .execute(&mut **tx)
        .await?;
    }
    for barrier in &domain.plan.barriers {
        let barrier_id = format!(
            "{}:barrier:{}",
            config.shadow_workflow_id, barrier.barrier_id.0
        );
        sqlx::query(
            "INSERT INTO workflow_barriers (id, workflow_id, declaring_transition_id, \
             declaring_workflow_version, status, satisfied_at, event_codec_family, event_codec_version, event_payload) \
             VALUES (?1, ?2, ?3, 1, 'waiting', NULL, ?4, ?5, '{}')",
        )
        .bind(&barrier_id)
        .bind(&config.shadow_workflow_id)
        .bind(&transition_id)
        .bind(barrier.reducer_event_codec.family)
        .bind(i64::from(barrier.reducer_event_codec.version))
        .execute(&mut **tx)
        .await?;
    }
    for member in &domain.plan.barrier_members {
        sqlx::query(
            "INSERT INTO workflow_barrier_members (barrier_id, effect_id, receipt_family) VALUES (?1, ?2, ?3)",
        )
        .bind(format!("{}:barrier:{}", config.shadow_workflow_id, member.barrier_id.0))
        .bind(effect_db_id(config, member.effect_id.0))
        .bind(match member.receipt_family {
            phoenix_workflow::ReceiptFamily::CurrentGenerationEffect => "current_generation_effect",
            phoenix_workflow::ReceiptFamily::CompensationEffect => "compensation_effect",
        })
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
async fn insert_typed_effect_intent(
    tx: &mut Transaction<'_, Sqlite>,
    config: &CreationShadowConfig,
    effect_number: u64,
    intent: &CreationEffectIntent,
) -> WorkflowRepositoryResult<()> {
    let (
        kind,
        conversation_id,
        repository_path,
        worktree_path,
        branch_name,
        message_id,
        attachment_count,
    ) = match intent {
        CreationEffectIntent::ResolveRepository { repository_path } => (
            "resolve_repository",
            None,
            Some(repository_path.as_str()),
            None,
            None,
            None,
            None,
        ),
        CreationEffectIntent::ReserveWorktree {
            repository_path,
            worktree_path,
            branch_name,
        } => (
            "reserve_worktree",
            None,
            Some(repository_path.as_str()),
            Some(worktree_path.as_str()),
            Some(branch_name.as_str()),
            None,
            None,
        ),
        CreationEffectIntent::MaterializeOrReconcileWorktree {
            repository_path,
            worktree_path,
            branch_name,
        } => (
            "materialize_or_reconcile_worktree",
            None,
            Some(repository_path.as_str()),
            Some(worktree_path.as_str()),
            Some(branch_name.as_str()),
            None,
            None,
        ),
        CreationEffectIntent::FinalizeAttachments {
            conversation_id,
            attachment_ids,
        } => (
            "finalize_attachments",
            Some(conversation_id.as_str()),
            None,
            None,
            None,
            None,
            Some(attachment_ids.len()),
        ),
        CreationEffectIntent::ExpandInitialMessage {
            conversation_id,
            message_id,
            ..
        } => (
            "expand_initial_message",
            Some(conversation_id.as_str()),
            None,
            None,
            None,
            Some(message_id.as_str()),
            None,
        ),
        CreationEffectIntent::CommitMetadata {
            conversation_id,
            worktree_path,
        } => (
            "commit_metadata",
            Some(conversation_id.as_str()),
            None,
            Some(worktree_path.as_str()),
            None,
            None,
            None,
        ),
        CreationEffectIntent::BootstrapRuntime { conversation_id } => (
            "bootstrap_runtime",
            Some(conversation_id.as_str()),
            None,
            None,
            None,
            None,
            None,
        ),
        CreationEffectIntent::DispatchInitialLlmRequest {
            conversation_id,
            message_id,
        } => (
            "dispatch_initial_llm_request",
            Some(conversation_id.as_str()),
            None,
            None,
            None,
            Some(message_id.as_str()),
            None,
        ),
        CreationEffectIntent::RevokeRuntime { conversation_id } => (
            "revoke_runtime",
            Some(conversation_id.as_str()),
            None,
            None,
            None,
            None,
            None,
        ),
        CreationEffectIntent::RemoveOwnedWorktree {
            repository_path,
            worktree_path,
        } => (
            "remove_owned_worktree",
            None,
            Some(repository_path.as_str()),
            Some(worktree_path.as_str()),
            None,
            None,
            None,
        ),
        CreationEffectIntent::ReleaseReservation {
            conversation_id,
            worktree_path,
        } => (
            "release_reservation",
            Some(conversation_id.as_str()),
            None,
            Some(worktree_path.as_str()),
            None,
            None,
            None,
        ),
        CreationEffectIntent::DeleteStagedAttachments {
            conversation_id,
            attachment_ids,
        } => (
            "delete_staged_attachments",
            Some(conversation_id.as_str()),
            None,
            None,
            None,
            None,
            Some(attachment_ids.len()),
        ),
        CreationEffectIntent::FinishCancellationOrDeletion { conversation_id } => (
            "finish_cancellation_or_deletion",
            Some(conversation_id.as_str()),
            None,
            None,
            None,
            None,
            None,
        ),
    };
    sqlx::query(
        "INSERT INTO creation_shadow_effect_intents
         (effect_id, intent_kind, conversation_id, repository_path, worktree_path, branch_name, message_id, attachment_count)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )
    .bind(effect_db_id(config, effect_number))
    .bind(kind)
    .bind(conversation_id)
    .bind(repository_path)
    .bind(worktree_path)
    .bind(branch_name)
    .bind(message_id)
    .bind(attachment_count.map(i64::try_from).transpose().map_err(|_| WorkflowRepositoryError::InvalidPlan("too many attachments"))?)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn upsert_projection(
    tx: &mut Transaction<'_, Sqlite>,
    config: &CreationShadowConfig,
    oracle: &AuthoritativeCreationOracle,
    domain: &creation_profile::CreationShadowAdapter,
    now: DateTime<Utc>,
) -> WorkflowRepositoryResult<()> {
    let p = &domain.projection;
    sqlx::query(
        "INSERT INTO creation_shadow_projections (shadow_workflow_id, oracle_generation, oracle_attempt, oracle_revision, \
         projection_status, completion, compensation, hidden, can_read, can_write, can_runtime, can_cancel, \
         can_start_over, can_delete, projected_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15) \
         ON CONFLICT(shadow_workflow_id) DO UPDATE SET oracle_generation=excluded.oracle_generation, \
         oracle_attempt=excluded.oracle_attempt, oracle_revision=excluded.oracle_revision, projection_status=excluded.projection_status, completion=excluded.completion, \
         compensation=excluded.compensation, hidden=excluded.hidden, can_read=excluded.can_read, can_write=excluded.can_write, \
         can_runtime=excluded.can_runtime, can_cancel=excluded.can_cancel, can_start_over=excluded.can_start_over, \
         can_delete=excluded.can_delete, projected_at=excluded.projected_at",
    )
    .bind(&config.shadow_workflow_id).bind(to_i64(oracle.generation)?).bind(i64::from(oracle.attempt))
    .bind(to_i64(oracle.revision)?).bind(projection_status_sql(p.status)).bind(completion_sql(p.completion)).bind(compensation_sql(p.compensation))
    .bind(p.hidden).bind(allowed(p.capabilities.read)).bind(allowed(p.capabilities.write))
    .bind(allowed(p.capabilities.runtime)).bind(allowed(p.capabilities.cancel))
    .bind(allowed(p.capabilities.start_over)).bind(allowed(p.capabilities.delete)).bind(now.to_rfc3339())
    .execute(&mut **tx).await?;
    sqlx::query("DELETE FROM creation_shadow_readiness_effects WHERE shadow_workflow_id = ?1")
        .bind(&config.shadow_workflow_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query("DELETE FROM creation_shadow_effect_predictions WHERE shadow_workflow_id = ?1")
        .bind(&config.shadow_workflow_id)
        .execute(&mut **tx)
        .await?;
    for (ordinal, effect) in p.readiness_effects.iter().enumerate() {
        sqlx::query("INSERT INTO creation_shadow_readiness_effects (shadow_workflow_id, ordinal, effect_number) VALUES (?1, ?2, ?3)")
            .bind(&config.shadow_workflow_id).bind(i64::try_from(ordinal).map_err(|_| WorkflowRepositoryError::InvalidPlan("too many readiness effects"))?)
            .bind(to_i64(effect.0)?).execute(&mut **tx).await?;
    }
    for (effect, prediction) in &p.effect_predictions {
        sqlx::query("INSERT INTO creation_shadow_effect_predictions (shadow_workflow_id, effect_number, prediction) VALUES (?1, ?2, ?3)")
            .bind(&config.shadow_workflow_id).bind(to_i64(effect.0)?).bind(prediction_sql(*prediction))
            .execute(&mut **tx).await?;
    }
    Ok(())
}

async fn update_divergence(
    tx: &mut Transaction<'_, Sqlite>,
    config: &CreationShadowConfig,
    observed: CreationShadowEvidence,
    actual: CreationProjectionStatus,
    actual_capabilities: CreationCapabilities,
    actual_hidden: bool,
    now: DateTime<Utc>,
) -> WorkflowRepositoryResult<()> {
    let expected = match observed {
        CreationShadowEvidence::ProjectionStatus(expected)
        | CreationShadowEvidence::UserProjection {
            status: expected, ..
        } => expected,
    };
    if expected == actual {
        sqlx::query("UPDATE creation_shadow_divergences SET resolved_at = ?1 WHERE shadow_workflow_id = ?2 AND evidence_identity = 'projection_status' AND resolved_at IS NULL")
            .bind(now.to_rfc3339()).bind(&config.shadow_workflow_id).execute(&mut **tx).await?;
    } else {
        let expected = projection_status_sql(expected);
        let actual = projection_status_sql(actual);
        sqlx::query(
            "UPDATE creation_shadow_divergences SET resolved_at = ?1 \
             WHERE shadow_workflow_id = ?2 AND evidence_identity = 'projection_status' \
               AND resolved_at IS NULL AND (expected_value <> ?3 OR actual_value <> ?4)",
        )
        .bind(now.to_rfc3339())
        .bind(&config.shadow_workflow_id)
        .bind(expected)
        .bind(actual)
        .execute(&mut **tx)
        .await?;
        sqlx::query("INSERT INTO creation_shadow_divergences (shadow_workflow_id, evidence_identity, expected_value, actual_value, recorded_at, resolved_at) VALUES (?1, 'projection_status', ?2, ?3, ?4, NULL) ON CONFLICT(shadow_workflow_id, evidence_identity) WHERE resolved_at IS NULL DO NOTHING")
            .bind(&config.shadow_workflow_id).bind(expected).bind(actual)
            .bind(now.to_rfc3339()).execute(&mut **tx).await?;
    }
    if let CreationShadowEvidence::UserProjection {
        capabilities,
        hidden,
        ..
    } = observed
    {
        update_boolean_divergence(tx, config, "projection_hidden", hidden, actual_hidden, now)
            .await?;
        for (identity, expected, actual) in [
            (
                "capability_read",
                allowed(capabilities.read),
                allowed(actual_capabilities.read),
            ),
            (
                "capability_write",
                allowed(capabilities.write),
                allowed(actual_capabilities.write),
            ),
            (
                "capability_runtime",
                allowed(capabilities.runtime),
                allowed(actual_capabilities.runtime),
            ),
            (
                "capability_cancel",
                allowed(capabilities.cancel),
                allowed(actual_capabilities.cancel),
            ),
            (
                "capability_start_over",
                allowed(capabilities.start_over),
                allowed(actual_capabilities.start_over),
            ),
            (
                "capability_delete",
                allowed(capabilities.delete),
                allowed(actual_capabilities.delete),
            ),
        ] {
            update_boolean_divergence(tx, config, identity, expected, actual, now).await?;
        }
    }
    Ok(())
}

async fn update_boolean_divergence(
    tx: &mut Transaction<'_, Sqlite>,
    config: &CreationShadowConfig,
    identity: &str,
    expected: bool,
    actual: bool,
    now: DateTime<Utc>,
) -> WorkflowRepositoryResult<()> {
    if expected == actual {
        sqlx::query("UPDATE creation_shadow_divergences SET resolved_at = ?1 WHERE shadow_workflow_id = ?2 AND evidence_identity = ?3 AND resolved_at IS NULL")
            .bind(now.to_rfc3339()).bind(&config.shadow_workflow_id).bind(identity)
            .execute(&mut **tx).await?;
    } else {
        sqlx::query("INSERT INTO creation_shadow_divergences (shadow_workflow_id, evidence_identity, expected_value, actual_value, recorded_at, resolved_at) VALUES (?1, ?2, ?3, ?4, ?5, NULL) ON CONFLICT(shadow_workflow_id, evidence_identity) WHERE resolved_at IS NULL DO UPDATE SET expected_value=excluded.expected_value, actual_value=excluded.actual_value")
            .bind(&config.shadow_workflow_id).bind(identity).bind(expected.to_string())
            .bind(actual.to_string()).bind(now.to_rfc3339()).execute(&mut **tx).await?;
    }
    Ok(())
}

fn redacted_event_payload(event: &CreationEvent) -> String {
    let (kind, job_id) = match event {
        CreationEvent::ShadowPlanProjected { job_id } => ("shadow_plan_projected", job_id),
        CreationEvent::AuthoritativeProgressObserved { job_id, .. } => {
            ("authoritative_progress_observed", job_id)
        }
        CreationEvent::CancellationOrDeletionProjected { job_id } => {
            ("cancellation_or_deletion_projected", job_id)
        }
    };
    serde_json::json!({ "kind": kind, "job_id": job_id }).to_string()
}

fn anchor_status_sql(value: &AuthoritativeCreationStatus) -> &'static str {
    match value {
        AuthoritativeCreationStatus::Accepted
        | AuthoritativeCreationStatus::Claimed { .. }
        | AuthoritativeCreationStatus::RetryScheduled { .. } => "active",
        AuthoritativeCreationStatus::Cancelling => "cancelling",
        AuthoritativeCreationStatus::Cancelled => "cancelled",
        AuthoritativeCreationStatus::DeletionPending => "deletion_pending",
        AuthoritativeCreationStatus::Ready => "completed",
        AuthoritativeCreationStatus::Failed(_) => "failed",
    }
}

fn authoritative_status_sql(value: &AuthoritativeCreationStatus) -> &'static str {
    match value {
        AuthoritativeCreationStatus::Accepted => "accepted",
        AuthoritativeCreationStatus::Claimed { .. } => "claimed",
        AuthoritativeCreationStatus::RetryScheduled { .. } => "retry_scheduled",
        AuthoritativeCreationStatus::Cancelling => "cancelling",
        AuthoritativeCreationStatus::Cancelled => "cancelled",
        AuthoritativeCreationStatus::DeletionPending => "deletion_pending",
        AuthoritativeCreationStatus::Ready => "ready",
        AuthoritativeCreationStatus::Failed(_) => "failed",
    }
}

fn authoritative_stage_sql(value: AuthoritativeCreationStage) -> &'static str {
    match value {
        AuthoritativeCreationStage::ValidateIntent => "validate_intent",
        AuthoritativeCreationStage::ResolveRepository => "resolve_repository",
        AuthoritativeCreationStage::ReserveResources => "reserve_resources",
        AuthoritativeCreationStage::MaterializeWorktree => "materialize_worktree",
        AuthoritativeCreationStage::FinalizeAttachments => "finalize_attachments",
        AuthoritativeCreationStage::ExpandInitialMessage => "expand_initial_message",
        AuthoritativeCreationStage::CommitMetadata => "commit_metadata",
        AuthoritativeCreationStage::BootstrapInitialTurn => "bootstrap_initial_turn",
        AuthoritativeCreationStage::Finalize => "finalize",
    }
}

fn effect_db_id(config: &CreationShadowConfig, id: u64) -> String {
    format!("{}:effect:{id}", config.shadow_workflow_id)
}
fn to_i64(value: u64) -> WorkflowRepositoryResult<i64> {
    i64::try_from(value).map_err(|_| WorkflowRepositoryError::GenerationOutOfRange(value))
}
fn allowed(value: CapabilityAvailability) -> bool {
    value == CapabilityAvailability::Allowed
}
fn status_sql(value: phoenix_workflow::WorkflowStatus) -> &'static str {
    match value {
        phoenix_workflow::WorkflowStatus::Active => "active",
        phoenix_workflow::WorkflowStatus::Cancelling => "cancelling",
        phoenix_workflow::WorkflowStatus::Cancelled => "cancelled",
        phoenix_workflow::WorkflowStatus::DeletionPending => "deletion_pending",
        phoenix_workflow::WorkflowStatus::Completed => "completed",
        phoenix_workflow::WorkflowStatus::Failed => "failed",
    }
}
fn projection_status_sql(value: CreationProjectionStatus) -> &'static str {
    match value {
        CreationProjectionStatus::Provisioning => "provisioning",
        CreationProjectionStatus::Failed => "failed",
        CreationProjectionStatus::Cancelled => "cancelled",
        CreationProjectionStatus::DeletionPending => "deletion_pending",
        CreationProjectionStatus::Ready => "ready",
    }
}
fn completion_sql(value: CompletionPrediction) -> &'static str {
    match value {
        CompletionPrediction::Pending => "pending",
        CompletionPrediction::Complete => "complete",
        CompletionPrediction::Failed => "failed",
        CompletionPrediction::Cancelled => "cancelled",
        CompletionPrediction::DeletionPending => "deletion_pending",
    }
}
fn compensation_sql(value: CompensationPrediction) -> &'static str {
    match value {
        CompensationPrediction::None => "none",
        CompensationPrediction::RequiredForCancellation => "required_for_cancellation",
        CompensationPrediction::RequiredForDeletion => "required_for_deletion",
    }
}
fn prediction_sql(value: EffectPrediction) -> &'static str {
    match value {
        EffectPrediction::Completed => "completed",
        EffectPrediction::Eligible => "eligible",
        EffectPrediction::Blocked => "blocked",
        EffectPrediction::Omitted => "omitted",
    }
}
fn role_sql(value: phoenix_workflow::EffectRole) -> &'static str {
    match value {
        phoenix_workflow::EffectRole::Required => "required",
        phoenix_workflow::EffectRole::Optional => "optional",
        phoenix_workflow::EffectRole::Compensation => "compensation",
    }
}
fn ambiguity_sql(value: phoenix_workflow::EffectAmbiguity) -> &'static str {
    match value {
        phoenix_workflow::EffectAmbiguity::ObservableReconciliation => "observable_reconciliation",
        phoenix_workflow::EffectAmbiguity::ExternalIdempotency => "external_idempotency",
        phoenix_workflow::EffectAmbiguity::SafeRepeatability => "safe_repeatability",
        phoenix_workflow::EffectAmbiguity::ManualResolution => "manual_resolution",
    }
}

#[cfg(test)]
#[path = "creation_shadow/tests.rs"]
mod tests;
