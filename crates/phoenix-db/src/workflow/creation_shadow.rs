//! Normalized diagnostic persistence for the conversation-creation shadow profile.
//!
//! The adapter is invoked only after the authoritative creation-job commit. Its transaction owns
//! only workflow diagnostics; failures are returned and never update `conversation_creation_jobs`.

use chrono::{DateTime, Utc};
use phoenix_workflow::{
    creation_profile::{
        self, AuthoritativeCreationOracle, CapabilityAvailability, CompensationPrediction,
        CompletionPrediction, CreationProjectionStatus, EffectPrediction,
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
            domain.projection.status,
            observed,
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
        let existing: Option<(String, i64, i64, i64)> = sqlx::query_as(
            "SELECT profile_id, protocol_version, runtime_acceptance_enabled, external_acceptance_enabled \
             FROM workflow_protocol_selections WHERE id = ?1",
        )
        .bind(SELECTION_ID)
        .fetch_optional(self.repository.pool())
        .await?;
        if let Some((profile, version, runtime, external)) = existing {
            if profile == creation_profile::PROFILE_ID
                && version == i64::from(creation_profile::PROTOCOL_VERSION)
                && runtime == 0
                && external == 0
            {
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
                accepting: true,
                runtime_acceptance_enabled: false,
                external_acceptance_enabled: false,
                registered_at: now,
                drained_at: None,
                supported_codecs: [
                    "creation.snapshot",
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
        "SELECT conversation_id, generation, attempt FROM conversation_creation_jobs WHERE id = ?1",
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
    {
        return Err(WorkflowRepositoryError::CorruptState(
            "creation oracle does not match committed authoritative job".to_owned(),
        ));
    }
    Ok(())
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
         VALUES (?1, ?2, ?3, 'legacy_protocol', 'authoritative', NULL, ?4, 0, ?5, 'active', \
         'creation.authoritative_anchor', 1, '{}', ?6) \
         ON CONFLICT(id) DO UPDATE SET generation = excluded.generation",
    )
    .bind(&config.authoritative_anchor_workflow_id)
    .bind(creation_profile::PROFILE_ID)
    .bind(i64::from(creation_profile::PROTOCOL_VERSION))
    .bind(SELECTION_ID)
    .bind(to_i64(oracle.generation)?)
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
    .bind(status_sql(domain.workflow.status))
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

async fn replace_graph(
    tx: &mut Transaction<'_, Sqlite>,
    config: &CreationShadowConfig,
    oracle: &AuthoritativeCreationOracle,
    domain: &creation_profile::CreationShadowAdapter,
    now: DateTime<Utc>,
) -> WorkflowRepositoryResult<()> {
    sqlx::query("DELETE FROM workflow_transitions WHERE workflow_id = ?1")
        .bind(&config.shadow_workflow_id)
        .execute(&mut **tx)
        .await?;
    let transition_id = format!("{}:projection", config.shadow_workflow_id);
    sqlx::query(
        "INSERT INTO workflow_transitions (id, workflow_id, from_version, to_version, generation, \
         event_codec_family, event_codec_version, event_payload, committed_at) \
         VALUES (?1, ?2, 0, 1, ?3, 'creation.event', 1, '{}', ?4)",
    )
    .bind(&transition_id)
    .bind(&config.shadow_workflow_id)
    .bind(to_i64(oracle.generation)?)
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

async fn upsert_projection(
    tx: &mut Transaction<'_, Sqlite>,
    config: &CreationShadowConfig,
    oracle: &AuthoritativeCreationOracle,
    domain: &creation_profile::CreationShadowAdapter,
    now: DateTime<Utc>,
) -> WorkflowRepositoryResult<()> {
    let p = &domain.projection;
    sqlx::query(
        "INSERT INTO creation_shadow_projections (shadow_workflow_id, oracle_generation, oracle_attempt, \
         projection_status, completion, compensation, hidden, can_read, can_write, can_runtime, can_cancel, \
         can_start_over, can_delete, projected_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14) \
         ON CONFLICT(shadow_workflow_id) DO UPDATE SET oracle_generation=excluded.oracle_generation, \
         oracle_attempt=excluded.oracle_attempt, projection_status=excluded.projection_status, completion=excluded.completion, \
         compensation=excluded.compensation, hidden=excluded.hidden, can_read=excluded.can_read, can_write=excluded.can_write, \
         can_runtime=excluded.can_runtime, can_cancel=excluded.can_cancel, can_start_over=excluded.can_start_over, \
         can_delete=excluded.can_delete, projected_at=excluded.projected_at",
    )
    .bind(&config.shadow_workflow_id).bind(to_i64(oracle.generation)?).bind(i64::from(oracle.attempt))
    .bind(projection_status_sql(p.status)).bind(completion_sql(p.completion)).bind(compensation_sql(p.compensation))
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
    expected: CreationProjectionStatus,
    observed: CreationShadowEvidence,
    now: DateTime<Utc>,
) -> WorkflowRepositoryResult<()> {
    let CreationShadowEvidence::ProjectionStatus(actual) = observed;
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
    Ok(())
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
