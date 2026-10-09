#![allow(clippy::needless_pass_by_value)]

use chrono::{DateTime, Utc};
use phoenix_core::domain::close::{
    AbsenceBasis, CapturedConversationStateKind, CapturedWorktreeIdentity, CloseAttemptId,
    CloseAttemptMember, CloseAttemptScope, CloseCompletionOutcome, CloseExpectedRetirementResource,
    CloseInspection, CloseInspectionLoss, CloseLossItem, CloseMemberRole, CloseObligation,
    CloseOwnedResourceInventory, ClosePhase, CloseRetiredResource, CloseRetirementSnapshot,
    CloseRetirementTarget, CloseRun, CloseRunOrdinal, CloseRunRef, CloseRunStatus,
    CloseStopCertainty, GitOidIdentity, GitPathIdentity, LossCategory, LossItemIdentity,
    OpaqueIdentity, ProductConversationId, RetiredResourceIdentity, RetiredResourceKind,
    RetirementFailureReason, RetirementOutcome, TranscriptConversationId, WorktreeIdentity,
};
use phoenix_core::domain::db_schema::MessageContent;
use phoenix_core::work_scope::{RuntimeRole, WorkScopeId};
use sqlx::sqlite::SqliteRow;
use sqlx::{Connection, Row, Sqlite, SqliteConnection, Transaction};
use std::fmt::Write as _;

use crate::coordinator_watches::append_mandatory_close_failure_event_tx;

use crate::{
    conv_state_kind, parse_conversation_row, CloseFoundationRepair, ConvState, Conversation,
    Database, DbError, DbResult,
};

#[derive(Debug, Clone)]
pub struct CloseFoundationTopologyMember {
    pub conversation: Conversation,
    pub role: CloseMemberRole,
}

#[derive(Debug, Clone)]
pub struct CloseFoundationTopology {
    pub root: Conversation,
    pub latest: Conversation,
    pub members: Vec<CloseFoundationTopologyMember>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloseDirectTurnSettlementTarget {
    pub conversation_id: String,
    pub turn_id: u64,
    pub expected_generation: u64,
}

/// Exact durable Close attempt that currently fences aggregate work admission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloseAdmissionFence {
    pub product_conversation_id: ProductConversationId,
    pub attempt_id: CloseAttemptId,
    pub phase: ClosePhase,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloseProjection {
    pub obligation: CloseObligation,
    pub latest_run: CloseRun,
    pub inspections: Vec<CloseInspection>,
    pub losses: Vec<CloseInspectionLoss>,
    pub residuals: Vec<CloseRetiredResource>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProductConversationAdmission {
    Accepted {
        product_conversation_id: ProductConversationId,
    },
    Refused(CloseAdmissionFence),
    History(ProductConversationId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageTargetAdmission {
    Aggregate(ProductConversationAdmission),
    StandaloneAvailable,
    StandaloneArchived,
}

impl ProductConversationAdmission {
    #[must_use]
    pub fn is_accepted(&self) -> bool {
        matches!(self, Self::Accepted { .. })
    }
}

impl CloseFoundationTopology {
    #[must_use]
    pub fn member_ids(&self) -> Vec<&str> {
        self.members
            .iter()
            .map(|member| member.conversation.id.as_str())
            .collect()
    }
}

pub(crate) async fn admit_product_conversation_operation_tx(
    tx: &mut Transaction<'_, Sqlite>,
    conversation_id: &str,
) -> DbResult<ProductConversationAdmission> {
    let product_conversation_id: String =
        sqlx::query_scalar("SELECT product_conversation_id FROM conversations WHERE id = ?1")
            .bind(conversation_id)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or_else(|| DbError::ConversationNotFound(conversation_id.to_string()))?;
    let product_conversation_id = parse_product_conversation_id(
        product_conversation_id,
        "conversations.product_conversation_id",
    )?;
    let aggregate =
        sqlx::query("SELECT kind, ordinary_lifecycle FROM product_conversations WHERE id = ?1")
            .bind(product_conversation_id.as_str())
            .fetch_optional(&mut **tx)
            .await?
            .ok_or_else(|| {
                DbError::Serialization(format!(
                    "missing product conversation {}",
                    product_conversation_id.as_str()
                ))
            })?;
    let kind: String = aggregate.try_get("kind")?;
    if kind == "coordinator" {
        return Ok(ProductConversationAdmission::Accepted {
            product_conversation_id,
        });
    }
    let lifecycle: String = aggregate.try_get("ordinary_lifecycle")?;
    if lifecycle == "history" {
        return Ok(ProductConversationAdmission::History(
            product_conversation_id,
        ));
    }
    let obligation = sqlx::query(
        "SELECT attempt_id, phase FROM close_obligations
         WHERE product_conversation_id = ?1 AND (phase <> 'completed' OR close_outcome = 'close_incomplete')",
    )
    .bind(product_conversation_id.as_str())
    .fetch_optional(&mut **tx)
    .await?;
    match obligation {
        None => Ok(ProductConversationAdmission::Accepted {
            product_conversation_id,
        }),
        Some(row) => {
            let attempt_id = parse_close_attempt_id(row.try_get("attempt_id")?)?;
            let phase_raw: String = row.try_get("phase")?;
            let phase = ClosePhase::from_db_str(&phase_raw).ok_or_else(|| {
                DbError::Serialization(format!("unknown close phase {phase_raw}"))
            })?;
            Ok(ProductConversationAdmission::Refused(CloseAdmissionFence {
                product_conversation_id,
                attempt_id,
                phase,
            }))
        }
    }
}

pub(crate) async fn require_product_conversation_admission_tx(
    tx: &mut Transaction<'_, Sqlite>,
    conversation_id: &str,
) -> DbResult<ProductConversationId> {
    match admit_product_conversation_operation_tx(tx, conversation_id).await? {
        ProductConversationAdmission::Accepted {
            product_conversation_id,
        } => Ok(product_conversation_id),
        ProductConversationAdmission::Refused(fence) => Err(DbError::CloseAdmissionFenced(fence)),
        ProductConversationAdmission::History(product_conversation_id) => Err(
            DbError::ProductConversationUnavailable(product_conversation_id),
        ),
    }
}

fn parse_rfc3339_utc(value: String, field: &str) -> DbResult<DateTime<Utc>> {
    chrono::DateTime::parse_from_rfc3339(&value)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|error| DbError::Serialization(format!("invalid {field}: {error}")))
}

fn parse_product_conversation_id(value: String, field: &str) -> DbResult<ProductConversationId> {
    ProductConversationId::parse(value)
        .map_err(|error| DbError::Serialization(format!("invalid {field}: {error}")))
}

fn parse_transcript_conversation_id(
    value: String,
    field: &str,
) -> DbResult<TranscriptConversationId> {
    TranscriptConversationId::parse(value)
        .map_err(|error| DbError::Serialization(format!("invalid {field}: {error}")))
}

fn parse_close_attempt_id(value: String) -> DbResult<CloseAttemptId> {
    CloseAttemptId::parse(value).map_err(|error| DbError::Serialization(error.to_string()))
}

fn parse_work_scope_id_opt(value: Option<String>, field: &str) -> DbResult<Option<WorkScopeId>> {
    value
        .map(WorkScopeId::parse)
        .transpose()
        .map_err(|error| DbError::Serialization(format!("invalid {field}: {error}")))
}

fn parse_close_member_role(value: &str) -> DbResult<CloseMemberRole> {
    match value {
        "root" => Ok(CloseMemberRole::Root),
        "intermediate" => Ok(CloseMemberRole::Intermediate),
        "latest" => Ok(CloseMemberRole::Latest),
        "root_latest" => Ok(CloseMemberRole::RootLatest),
        other => Err(DbError::Serialization(format!(
            "unknown close member role {other}"
        ))),
    }
}

fn close_member_role_db_str(role: CloseMemberRole) -> &'static str {
    match role {
        CloseMemberRole::Root => "root",
        CloseMemberRole::Intermediate => "intermediate",
        CloseMemberRole::Latest => "latest",
        CloseMemberRole::RootLatest => "root_latest",
    }
}

fn close_precondition(message: impl Into<String>) -> DbError {
    DbError::CloseFoundationPrecondition(message.into())
}

fn parse_close_completion_outcome(
    value: Option<String>,
) -> DbResult<Option<CloseCompletionOutcome>> {
    value
        .map(|value| {
            CloseCompletionOutcome::from_db_str(&value).ok_or_else(|| {
                DbError::Serialization(format!("unknown close completion outcome {value}"))
            })
        })
        .transpose()
}

fn parse_close_obligation_row(row: SqliteRow) -> DbResult<CloseObligation> {
    let phase_raw: String = row.try_get("phase")?;
    let phase = ClosePhase::from_db_str(&phase_raw)
        .ok_or_else(|| DbError::Serialization(format!("unknown close phase {phase_raw}")))?;
    let inspection_generation: Option<String> = row.try_get("inspection_generation")?;
    let inspection_fingerprint: Option<String> = row.try_get("inspection_fingerprint")?;
    let snapshot = match (inspection_generation, inspection_fingerprint) {
        (Some(inspection_generation), Some(inspection_fingerprint)) => Some(
            CloseRetirementSnapshot::parse(inspection_generation, inspection_fingerprint)
                .map_err(|error| DbError::Serialization(error.to_string()))?,
        ),
        (None, None) => None,
        _ => {
            return Err(DbError::Serialization(
                "close obligation inspection pair mismatch".to_string(),
            ));
        }
    };
    CloseObligation::parse(
        parse_close_attempt_id(row.try_get("attempt_id")?)?,
        parse_product_conversation_id(
            row.try_get("product_conversation_id")?,
            "product_conversation_id",
        )?,
        phase,
        snapshot,
        parse_rfc3339_utc(row.try_get("created_at")?, "created_at")?,
        parse_rfc3339_utc(row.try_get("updated_at")?, "updated_at")?,
        row.try_get::<Option<String>, _>("completed_at")?
            .map(|value| parse_rfc3339_utc(value, "completed_at"))
            .transpose()?,
        parse_close_completion_outcome(row.try_get("close_outcome")?)?,
    )
    .map_err(|error| DbError::Serialization(error.to_string()))
}

fn parse_close_run_row(row: SqliteRow) -> DbResult<CloseRun> {
    let status: String = row.try_get("status")?;
    let status = match status.as_str() {
        "running" => CloseRunStatus::Running,
        "stopped" => CloseRunStatus::Stopped,
        "completed" => CloseRunStatus::Completed,
        _ => {
            return Err(DbError::Serialization(format!(
                "unknown Close run status {status}"
            )))
        }
    };
    Ok(CloseRun {
        run: CloseRunRef {
            attempt_id: parse_close_attempt_id(row.try_get("attempt_id")?)?,
            ordinal: CloseRunOrdinal::parse(row.try_get("run_ordinal")?)
                .map_err(|error| DbError::Serialization(error.to_string()))?,
        },
        status,
    })
}

fn parse_close_attempt_member_row(row: SqliteRow) -> DbResult<CloseAttemptMember> {
    Ok(CloseAttemptMember {
        attempt_id: parse_close_attempt_id(row.try_get("attempt_id")?)?,
        conversation_id: parse_transcript_conversation_id(
            row.try_get("conversation_id")?,
            "conversation_id",
        )?,
        role: parse_close_member_role(&row.try_get::<String, _>("member_role")?)?,
        continuation_ordinal: u32::try_from(row.try_get::<i64, _>("continuation_ordinal")?)
            .map_err(|error| DbError::Serialization(error.to_string()))?,
        captured_continued_in_conv_id: row
            .try_get::<Option<String>, _>("captured_continued_in_conv_id")?
            .map(|value| parse_transcript_conversation_id(value, "captured_continued_in_conv_id"))
            .transpose()?,
        captured_state_kind: CapturedConversationStateKind::from_db_str(
            &row.try_get::<String, _>("captured_state_kind")?,
        )
        .ok_or_else(|| DbError::Serialization("unknown captured state kind".to_string()))?,
        captured_runtime_role: RuntimeRole::from_db_str(
            &row.try_get::<String, _>("captured_runtime_role")?,
        )
        .ok_or_else(|| DbError::Serialization("unknown captured runtime role".to_string()))?,
        captured_work_scope_id: parse_work_scope_id_opt(
            row.try_get("captured_work_scope_id")?,
            "captured_work_scope_id",
        )?,
        captured_at: parse_rfc3339_utc(row.try_get("captured_at")?, "captured_at")?,
    })
}

fn parse_close_attempt_scope_row(row: SqliteRow) -> DbResult<CloseAttemptScope> {
    Ok(CloseAttemptScope {
        attempt_id: parse_close_attempt_id(row.try_get("attempt_id")?)?,
        scope: WorkScopeId::parse(row.try_get::<String, _>("scope")?)
            .map_err(|error| DbError::Serialization(error.to_string()))?,
        captured_worktree: match (
            row.try_get::<Option<String>, _>("captured_worktree_identity")?,
            row.try_get::<Option<String>, _>("captured_worktree_fingerprint")?,
            row.try_get::<Option<String>, _>("captured_worktree_locator")?,
        ) {
            (Some(id), Some(fingerprint), Some(locator)) => Some(
                CapturedWorktreeIdentity::Resolved(WorktreeIdentity::from_parts(
                    phoenix_core::domain::close::WorktreeId::parse(id)
                        .map_err(|error| DbError::Serialization(error.to_string()))?,
                    phoenix_core::domain::close::WorktreeFingerprint::parse(fingerprint)
                        .map_err(|error| DbError::Serialization(error.to_string()))?,
                    GitPathIdentity::decode_exact(&locator)
                        .map_err(|error| DbError::Serialization(error.to_string()))?,
                )),
            ),
            (None, None, Some(locator)) => Some(CapturedWorktreeIdentity::Unresolved {
                locator: GitPathIdentity::decode_exact(&locator)
                    .map_err(|error| DbError::Serialization(error.to_string()))?,
            }),
            (None, None, None) => None,
            _ => {
                return Err(DbError::Serialization(
                    "partial worktree identity".to_string(),
                ))
            }
        },
        captured_at: parse_rfc3339_utc(row.try_get("captured_at")?, "captured_at")?,
    })
}

fn parse_loss_category(value: &str) -> DbResult<LossCategory> {
    match value {
        "staged_tracked_paths" => Ok(LossCategory::StagedTrackedPaths),
        "unstaged_tracked_paths" => Ok(LossCategory::UnstagedTrackedPaths),
        "untracked_non_ignored_paths" => Ok(LossCategory::UntrackedNonIgnoredPaths),
        "initialized_submodule_state" => Ok(LossCategory::InitializedSubmoduleState),
        "detached_unreachable_commits" => Ok(LossCategory::DetachedUnreachableCommits),
        other => Err(DbError::Serialization(format!(
            "unknown loss category {other}"
        ))),
    }
}

fn parse_retired_resource_kind(value: &str) -> DbResult<RetiredResourceKind> {
    match value {
        "worktree" => Ok(RetiredResourceKind::Worktree),
        "work_scope" => Ok(RetiredResourceKind::WorkScope),
        "bash_process_group" => Ok(RetiredResourceKind::BashProcessGroup),
        "tmux_server" => Ok(RetiredResourceKind::TmuxServer),
        "pty_session" => Ok(RetiredResourceKind::PtySession),
        "browser_session" => Ok(RetiredResourceKind::BrowserSession),
        "equivalent_live_resource" => Ok(RetiredResourceKind::EquivalentLiveResource),
        other => Err(DbError::Serialization(format!(
            "unknown retired resource kind {other}"
        ))),
    }
}

fn parse_absence_basis(value: &str) -> DbResult<AbsenceBasis> {
    match value {
        "same_attempt_prior_retirement" => Ok(AbsenceBasis::SameAttemptPriorRetirement),
        "preexisting_exact_identity_evidence" => Ok(AbsenceBasis::PreexistingExactIdentityEvidence),
        other => Err(DbError::Serialization(format!(
            "unknown absence basis {other}"
        ))),
    }
}

fn parse_retirement_failure_reason(value: &str) -> DbResult<RetirementFailureReason> {
    match value {
        "removal_failed" => Ok(RetirementFailureReason::RemovalFailed),
        "still_shared_by_live_owner" => Ok(RetirementFailureReason::StillSharedByLiveOwner),
        "residual_process_alive" => Ok(RetirementFailureReason::ResidualProcessAlive),
        "identity_not_proven" => Ok(RetirementFailureReason::IdentityNotProven),
        "interrupted" => Ok(RetirementFailureReason::Interrupted),
        "manual_repair_required" => Ok(RetirementFailureReason::ManualRepairRequired),
        other => Err(DbError::Serialization(format!(
            "unknown retirement failure reason {other}"
        ))),
    }
}

fn parse_loss_item_identity(
    kind: &str,
    codec: &str,
    value: &str,
    worktree_fingerprint: Option<String>,
    worktree_locator: Option<String>,
) -> DbResult<LossItemIdentity> {
    match kind {
        "git_path" => {
            if codec != "git_path_bytes_hex_v1" {
                return Err(DbError::Serialization(format!(
                    "unexpected git_path codec {codec}"
                )));
            }
            Ok(LossItemIdentity::GitPath(
                GitPathIdentity::decode_exact(value)
                    .map_err(|error| DbError::Serialization(error.to_string()))?,
            ))
        }
        "git_oid" => {
            if codec != "hex" {
                return Err(DbError::Serialization(format!(
                    "unexpected git_oid codec {codec}"
                )));
            }
            Ok(LossItemIdentity::GitOid(
                GitOidIdentity::parse_hex(value)
                    .map_err(|error| DbError::Serialization(error.to_string()))?,
            ))
        }
        "opaque" => {
            if codec
                != OpaqueIdentity::parse("x")
                    .map_err(|error| DbError::Serialization(error.to_string()))?
                    .codec()
            {
                return Err(DbError::Serialization(format!(
                    "unexpected opaque codec {codec}"
                )));
            }
            Ok(LossItemIdentity::Opaque(
                OpaqueIdentity::parse(value)
                    .map_err(|error| DbError::Serialization(error.to_string()))?,
            ))
        }
        "worktree" => {
            if codec != "worktree_id_v1" {
                return Err(DbError::Serialization(format!(
                    "unexpected worktree codec {codec}"
                )));
            }
            let fingerprint = worktree_fingerprint.ok_or_else(|| {
                DbError::Serialization("worktree identity missing fingerprint".to_string())
            })?;
            let locator = worktree_locator.ok_or_else(|| {
                DbError::Serialization("worktree identity missing locator".to_string())
            })?;
            Ok(LossItemIdentity::Worktree(WorktreeIdentity::from_parts(
                phoenix_core::domain::close::WorktreeId::parse(value)
                    .map_err(|error| DbError::Serialization(error.to_string()))?,
                phoenix_core::domain::close::WorktreeFingerprint::parse(fingerprint)
                    .map_err(|error| DbError::Serialization(error.to_string()))?,
                GitPathIdentity::decode_exact(&locator)
                    .map_err(|error| DbError::Serialization(error.to_string()))?,
            )))
        }
        other => Err(DbError::Serialization(format!(
            "unknown loss item identity kind {other}"
        ))),
    }
}

fn parse_close_loss_item(
    category: LossCategory,
    kind: &str,
    codec: &str,
    value: &str,
) -> DbResult<CloseLossItem> {
    let identity = parse_loss_item_identity(kind, codec, value, None, None)?;
    match (category, identity) {
        (LossCategory::StagedTrackedPaths, LossItemIdentity::GitPath(path)) => {
            Ok(CloseLossItem::StagedTrackedPath(path))
        }
        (LossCategory::UnstagedTrackedPaths, LossItemIdentity::GitPath(path)) => {
            Ok(CloseLossItem::UnstagedTrackedPath(path))
        }
        (LossCategory::UntrackedNonIgnoredPaths, LossItemIdentity::GitPath(path)) => {
            Ok(CloseLossItem::UntrackedNonIgnoredPath(path))
        }
        (LossCategory::InitializedSubmoduleState, LossItemIdentity::GitPath(path)) => {
            Ok(CloseLossItem::InitializedSubmoduleState(path))
        }
        (LossCategory::DetachedUnreachableCommits, LossItemIdentity::GitOid(oid)) => {
            Ok(CloseLossItem::DetachedUnreachableCommit(oid))
        }
        (category, identity) => Err(DbError::Serialization(format!(
            "invalid close loss pairing: category {} cannot use {}",
            category.as_str(),
            identity.identity_kind()
        ))),
    }
}

async fn validate_adopted_absence_evidence(
    tx: &mut Transaction<'_, Sqlite>,
    request: &RecordCloseRetirementEvidenceRequest,
    absence_basis: AbsenceBasis,
) -> DbResult<()> {
    let evidence_matches_basis = match absence_basis {
        AbsenceBasis::SameAttemptPriorRetirement => {
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(
                     SELECT 1 FROM close_retirement_resources
                     WHERE attempt_id = ?1 AND scope = ?2 AND inspection_generation = ?3
                       AND inspection_fingerprint = ?4
                       AND resource_kind = ?5 AND identity_kind = ?6
                       AND identity_codec = ?7 AND identity_value = ?8
                       AND proof_kind = 'retired'
                     UNION ALL
                     SELECT 1 FROM close_retirement_resource_dispatches dispatch
                     WHERE dispatch.attempt_id = ?1 AND dispatch.scope = ?2
                       AND dispatch.inspection_generation = ?3
                       AND dispatch.inspection_fingerprint = ?4
                       AND dispatch.resource_kind = ?5 AND dispatch.identity_kind = ?6
                       AND dispatch.identity_codec = ?7 AND dispatch.identity_value = ?8
                       AND (
                           dispatch.resource_kind <> 'worktree'
                           OR EXISTS (
                               SELECT 1 FROM close_worktree_cleanup_plans plan
                               WHERE plan.attempt_id = dispatch.attempt_id
                                 AND plan.scope = dispatch.scope
                                 AND plan.inspection_generation = dispatch.inspection_generation
                                 AND plan.inspection_fingerprint = dispatch.inspection_fingerprint
                                 AND plan.resource_kind = dispatch.resource_kind
                                 AND plan.identity_kind = dispatch.identity_kind
                                 AND plan.identity_codec = dispatch.identity_codec
                                 AND plan.identity_value = dispatch.identity_value
                           )
                       )
                 )",
            )
            .bind(request.attempt_id.as_str())
            .bind(request.scope.as_str())
            .bind(request.snapshot.generation())
            .bind(request.snapshot.fingerprint())
            .bind(request.resource.kind().as_str())
            .bind(request.resource.identity().identity_kind())
            .bind(request.resource.identity().codec())
            .bind(request.resource.identity().value())
            .fetch_one(&mut **tx)
            .await?
        }
        AbsenceBasis::PreexistingExactIdentityEvidence => {
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(
                     SELECT 1
                     FROM close_retirement_resources evidence
                     JOIN close_obligations prior ON prior.attempt_id = evidence.attempt_id
                     JOIN close_obligations current ON current.attempt_id = ?1
                     WHERE prior.product_conversation_id = current.product_conversation_id
                       AND prior.attempt_id <> current.attempt_id
                       AND evidence.scope = ?2 AND evidence.resource_kind = ?3
                       AND evidence.identity_kind = ?4 AND evidence.identity_codec = ?5
                       AND evidence.identity_value = ?6
                       AND evidence.inspection_generation = prior.inspection_generation
                       AND evidence.inspection_fingerprint = prior.inspection_fingerprint
                       AND evidence.proof_kind IN ('retired', 'absence_adopted')
                 )",
            )
            .bind(request.attempt_id.as_str())
            .bind(request.scope.as_str())
            .bind(request.resource.kind().as_str())
            .bind(request.resource.identity().identity_kind())
            .bind(request.resource.identity().codec())
            .bind(request.resource.identity().value())
            .fetch_one(&mut **tx)
            .await?
        }
    };
    if evidence_matches_basis {
        Ok(())
    } else {
        Err(close_precondition(format!(
            "attempt {} has no retained exact-identity evidence for adopted absence",
            request.attempt_id
        )))
    }
}

fn parse_close_inspection_row(row: SqliteRow) -> DbResult<CloseInspection> {
    let snapshot = CloseRetirementSnapshot::parse(
        row.try_get::<String, _>("generation")?,
        row.try_get::<String, _>("fingerprint")?,
    )
    .map_err(|error| DbError::Serialization(error.to_string()))?;
    Ok(CloseInspection {
        attempt_id: parse_close_attempt_id(row.try_get("attempt_id")?)?,
        target: CloseRetirementTarget {
            scope: WorkScopeId::parse(row.try_get::<String, _>("scope")?)
                .map_err(|error| DbError::Serialization(error.to_string()))?,
        },
        snapshot,
        inspected_at: parse_rfc3339_utc(row.try_get("inspected_at")?, "inspected_at")?,
    })
}

fn parse_close_inspection_loss_row(row: SqliteRow) -> DbResult<CloseInspectionLoss> {
    let identity_kind: String = row.try_get("identity_kind")?;
    let identity_codec: String = row.try_get("identity_codec")?;
    let identity_value: String = row.try_get("identity_value")?;
    let category = parse_loss_category(&row.try_get::<String, _>("category")?)?;
    Ok(CloseInspectionLoss {
        attempt_id: parse_close_attempt_id(row.try_get("attempt_id")?)?,
        scope: WorkScopeId::parse(row.try_get::<String, _>("scope")?)
            .map_err(|error| DbError::Serialization(error.to_string()))?,
        snapshot: CloseRetirementSnapshot::parse(
            row.try_get::<String, _>("generation")?,
            row.try_get::<String, _>("fingerprint")?,
        )
        .map_err(|error| DbError::Serialization(error.to_string()))?,
        item: parse_close_loss_item(category, &identity_kind, &identity_codec, &identity_value)?,
    })
}

fn parse_retirement_outcome(
    proof_kind: &str,
    absence_basis: Option<String>,
    residual_reason: Option<String>,
) -> DbResult<RetirementOutcome> {
    match proof_kind {
        "retired" => Ok(RetirementOutcome::Retired),
        "absence_adopted" => Ok(RetirementOutcome::AbsenceAdopted {
            absence_basis: parse_absence_basis(&absence_basis.ok_or_else(|| {
                DbError::Serialization("absence_adopted missing absence_basis".to_string())
            })?)?,
        }),
        "residual" => Ok(RetirementOutcome::Residual {
            residual_reason: parse_retirement_failure_reason(&residual_reason.ok_or_else(
                || DbError::Serialization("residual missing residual_reason".to_string()),
            )?)?,
        }),
        other => Err(DbError::Serialization(format!(
            "unknown retirement proof kind {other}"
        ))),
    }
}

fn parse_close_retired_resource_row(row: SqliteRow) -> DbResult<CloseRetiredResource> {
    let identity_kind: String = row.try_get("identity_kind")?;
    let identity_codec: String = row.try_get("identity_codec")?;
    let identity_value: String = row.try_get("identity_value")?;
    let proof_kind: String = row.try_get("proof_kind")?;
    let resource_kind = parse_retired_resource_kind(&row.try_get::<String, _>("resource_kind")?)?;
    let resource_identity = parse_loss_item_identity(
        &identity_kind,
        &identity_codec,
        &identity_value,
        row.try_get("captured_worktree_fingerprint")?,
        row.try_get("captured_worktree_locator")?,
    )?;
    Ok(CloseRetiredResource {
        attempt_id: parse_close_attempt_id(row.try_get("attempt_id")?)?,
        scope: WorkScopeId::parse(row.try_get::<String, _>("scope")?)
            .map_err(|error| DbError::Serialization(error.to_string()))?,
        snapshot: CloseRetirementSnapshot::parse(
            row.try_get::<String, _>("inspection_generation")?,
            row.try_get::<String, _>("inspection_fingerprint")?,
        )
        .map_err(|error| DbError::Serialization(error.to_string()))?,
        resource: RetiredResourceIdentity::parse(resource_kind, resource_identity)
            .map_err(|error| DbError::Serialization(error.to_string()))?,
        outcome: parse_retirement_outcome(
            &proof_kind,
            row.try_get("absence_basis")?,
            row.try_get("residual_reason")?,
        )?,
        detail: row.try_get("detail")?,
        created_at: parse_rfc3339_utc(row.try_get("created_at")?, "created_at")?,
        updated_at: parse_rfc3339_utc(row.try_get("updated_at")?, "updated_at")?,
    })
}

#[derive(Debug, Clone)]
pub struct ReplaceCloseInspectionScopeRequest {
    pub scope: WorkScopeId,
    pub snapshot: CloseRetirementSnapshot,
    pub losses: Vec<CloseLossItem>,
}

#[derive(Debug, Clone)]
pub struct ReplaceCloseInspectionRequest {
    pub attempt_id: CloseAttemptId,
    pub scopes: Vec<ReplaceCloseInspectionScopeRequest>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureCloseRetirementInventoryScopeRequest {
    pub scope: WorkScopeId,
    pub inventory: CloseOwnedResourceInventory,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureCloseRetirementInventoryRequest {
    pub attempt_id: CloseAttemptId,
    pub snapshot: CloseRetirementSnapshot,
    pub scopes: Vec<CaptureCloseRetirementInventoryScopeRequest>,
}

#[derive(Debug, Clone)]
pub struct RecordCloseRetirementEvidenceRequest {
    pub attempt_id: CloseAttemptId,
    pub scope: WorkScopeId,
    pub snapshot: CloseRetirementSnapshot,
    pub resource: RetiredResourceIdentity,
    pub outcome: RetirementOutcome,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloseCleanupFailureAuthority {
    AttemptInterrupted,
    ExpectedResource {
        scope: WorkScopeId,
        snapshot: CloseRetirementSnapshot,
        resource: RetiredResourceIdentity,
    },
    CapturedScope {
        scope: WorkScopeId,
        resource: RetiredResourceIdentity,
    },
    ObservedProcessResource {
        scope: WorkScopeId,
        resource: RetiredResourceIdentity,
    },
}

impl CloseCleanupFailureAuthority {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::AttemptInterrupted => "attempt_interrupted",
            Self::ExpectedResource { .. } => "expected_resource",
            Self::CapturedScope { .. } => "captured_scope",
            Self::ObservedProcessResource { .. } => "observed_process_resource",
        }
    }

    #[must_use]
    pub fn resource(&self) -> Option<&RetiredResourceIdentity> {
        match self {
            Self::AttemptInterrupted => None,
            Self::ExpectedResource { resource, .. }
            | Self::CapturedScope { resource, .. }
            | Self::ObservedProcessResource { resource, .. } => Some(resource),
        }
    }

    #[must_use]
    pub fn scope(&self) -> Option<&WorkScopeId> {
        match self {
            Self::AttemptInterrupted => None,
            Self::ExpectedResource { scope, .. }
            | Self::CapturedScope { scope, .. }
            | Self::ObservedProcessResource { scope, .. } => Some(scope),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseCleanupResourceDisposition {
    Failed,
    Residual,
    Unattempted,
    Unknown,
}

impl CloseCleanupResourceDisposition {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Failed => "failed",
            Self::Residual => "residual",
            Self::Unattempted => "unattempted",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloseCleanupFailureResource {
    pub scope: WorkScopeId,
    pub resource: RetiredResourceIdentity,
    pub disposition: CloseCleanupResourceDisposition,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloseCleanupFailure {
    pub run_ordinal: CloseRunOrdinal,
    pub occurrence: TerminalizeInitialCloseCleanupFailureRequest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalizeInitialCloseCleanupFailureRequest {
    pub failure_occurrence_id: String,
    pub attempt_id: CloseAttemptId,
    pub source_product_conversation_id: ProductConversationId,
    pub authority: CloseCleanupFailureAuthority,
    pub remaining_resources: Vec<CloseCleanupFailureResource>,
    pub reason: RetirementFailureReason,
    pub detail: String,
    pub stop_certainty: CloseStopCertainty,
    pub occurred_at_us: i64,
}

fn parse_cleanup_resource_identity(row: &SqliteRow) -> DbResult<RetiredResourceIdentity> {
    RetiredResourceIdentity::parse(
        parse_retired_resource_kind(&row.try_get::<String, _>("resource_kind")?)?,
        parse_loss_item_identity(
            &row.try_get::<String, _>("identity_kind")?,
            &row.try_get::<String, _>("identity_codec")?,
            &row.try_get::<String, _>("identity_value")?,
            row.try_get("captured_worktree_fingerprint")?,
            row.try_get("captured_worktree_locator")?,
        )?,
    )
    .map_err(|error| DbError::Serialization(error.to_string()))
}

fn parse_cleanup_failure_authority(row: &SqliteRow) -> DbResult<CloseCleanupFailureAuthority> {
    let kind: &str = row.try_get("authority_kind")?;
    if kind == "attempt_interrupted" {
        return Ok(CloseCleanupFailureAuthority::AttemptInterrupted);
    }
    let scope = WorkScopeId::parse(row.try_get::<String, _>("scope")?)
        .map_err(|error| DbError::Serialization(error.to_string()))?;
    let resource = parse_cleanup_resource_identity(row)?;
    match kind {
        "expected_resource" => Ok(CloseCleanupFailureAuthority::ExpectedResource {
            scope,
            snapshot: CloseRetirementSnapshot::parse(
                row.try_get::<String, _>("inspection_generation")?,
                row.try_get::<String, _>("inspection_fingerprint")?,
            )
            .map_err(|error| DbError::Serialization(error.to_string()))?,
            resource,
        }),
        "captured_scope" => Ok(CloseCleanupFailureAuthority::CapturedScope { scope, resource }),
        "observed_process_resource" => {
            Ok(CloseCleanupFailureAuthority::ObservedProcessResource { scope, resource })
        }
        other => Err(DbError::Serialization(format!(
            "unknown cleanup failure authority {other}"
        ))),
    }
}

async fn close_cleanup_failure_resources_tx(
    tx: &mut Transaction<'_, Sqlite>,
    failure_occurrence_id: &str,
) -> DbResult<Vec<CloseCleanupFailureResource>> {
    sqlx::query(
        "SELECT resource.*, captured.captured_worktree_fingerprint, captured.captured_worktree_locator
         FROM close_cleanup_failure_resources resource
         JOIN close_cleanup_failures failure ON failure.failure_occurrence_id = resource.failure_occurrence_id
         JOIN close_attempt_scopes captured ON captured.attempt_id = failure.attempt_id AND captured.scope = resource.scope
         WHERE resource.failure_occurrence_id = ?1 ORDER BY resource.ordinal",
    ).bind(failure_occurrence_id).fetch_all(&mut **tx).await?.into_iter().map(|row| {
        Ok(CloseCleanupFailureResource {
            scope: WorkScopeId::parse(row.try_get::<String, _>("scope")?)
                .map_err(|error| DbError::Serialization(error.to_string()))?,
            resource: parse_cleanup_resource_identity(&row)?,
            disposition: match row.try_get::<&str, _>("disposition")? {
                "failed" => CloseCleanupResourceDisposition::Failed,
                "residual" => CloseCleanupResourceDisposition::Residual,
                "unattempted" => CloseCleanupResourceDisposition::Unattempted,
                "unknown" => CloseCleanupResourceDisposition::Unknown,
                other => return Err(DbError::Serialization(format!("unknown cleanup resource disposition {other}"))),
            },
        })
    }).collect()
}

async fn retained_interrupted_stop_certainty_tx(
    tx: &mut Transaction<'_, Sqlite>,
    run: &CloseRunRef,
) -> DbResult<CloseStopCertainty> {
    let confirmed_at_us: Option<i64> = sqlx::query_scalar(
        "SELECT confirmed_at_us
         FROM close_cleanup_failures
         WHERE attempt_id = ?1 AND cleanup_run_ordinal < ?2
           AND stop_certainty = 'conversation_and_processes_stopped'
         ORDER BY cleanup_run_ordinal DESC LIMIT 1",
    )
    .bind(run.attempt_id.as_str())
    .bind(run.ordinal.get())
    .fetch_optional(&mut **tx)
    .await?
    .flatten();
    Ok(match confirmed_at_us {
        Some(confirmed_at_us) => {
            CloseStopCertainty::ConversationAndProcessesStopped { confirmed_at_us }
        }
        None => CloseStopCertainty::ShutdownUncertain,
    })
}

async fn unresolved_expected_close_cleanup_resources_tx(
    tx: &mut Transaction<'_, Sqlite>,
    attempt_id: &str,
) -> DbResult<Vec<CloseCleanupFailureResource>> {
    let inventory_complete: bool = sqlx::query_scalar(
        "SELECT
            (SELECT COUNT(*) FROM close_attempt_scopes WHERE attempt_id = ?1)
              = (SELECT COUNT(*) FROM close_retirement_inventories inventory
                 JOIN close_obligations obligation ON obligation.attempt_id = inventory.attempt_id
                 WHERE inventory.attempt_id = ?1
                   AND inventory.inspection_generation = obligation.inspection_generation
                   AND inventory.inspection_fingerprint = obligation.inspection_fingerprint)
            AND NOT EXISTS (
                SELECT 1 FROM close_retirement_inventories inventory
                JOIN close_obligations obligation ON obligation.attempt_id = inventory.attempt_id
                WHERE inventory.attempt_id = ?1 AND inventory.sealed = 0
                  AND inventory.inspection_generation = obligation.inspection_generation
                  AND inventory.inspection_fingerprint = obligation.inspection_fingerprint
            )",
    )
    .bind(attempt_id)
    .fetch_one(&mut **tx)
    .await?;
    if !inventory_complete {
        return Ok(Vec::new());
    }
    let expected = list_close_expected_retirement_resources_tx(tx, attempt_id).await?;
    let run = parse_close_run_row(sqlx::query("SELECT attempt_id, run_ordinal, status FROM close_runs WHERE attempt_id = ?1 ORDER BY run_ordinal DESC LIMIT 1")
        .bind(attempt_id).fetch_one(&mut **tx).await?)?.run;
    let mut remaining = Vec::new();
    for target in expected {
        if close_process_step_succeeded_tx(tx, &run, &target.scope, &target.resource).await? {
            continue;
        }
        let identity = target.resource.identity();
        let proof: Option<String> = sqlx::query_scalar(
            "SELECT proof_kind FROM close_retirement_resources
             WHERE attempt_id = ?1 AND scope = ?2 AND inspection_generation = ?3 AND inspection_fingerprint = ?4
               AND resource_kind = ?5 AND identity_kind = ?6 AND identity_codec = ?7 AND identity_value = ?8",
        )
        .bind(attempt_id)
        .bind(target.scope.as_str())
        .bind(target.snapshot.generation())
        .bind(target.snapshot.fingerprint())
        .bind(target.resource.kind().as_str())
        .bind(identity.identity_kind())
        .bind(identity.codec())
        .bind(identity.value())
        .fetch_optional(&mut **tx)
        .await?;
        if proof.as_deref().is_some_and(|kind| kind != "residual") {
            continue;
        }
        remaining.push(CloseCleanupFailureResource {
            scope: target.scope,
            resource: target.resource,
            disposition: if proof.is_some() {
                CloseCleanupResourceDisposition::Residual
            } else {
                CloseCleanupResourceDisposition::Unattempted
            },
        });
    }
    Ok(remaining)
}

async fn expected_close_cleanup_failure_resources_tx(
    tx: &mut Transaction<'_, Sqlite>,
    attempt_id: &str,
    failed_scope: &WorkScopeId,
    failed_resource: &RetiredResourceIdentity,
) -> DbResult<Vec<CloseCleanupFailureResource>> {
    let expected = list_close_expected_retirement_resources_tx(tx, attempt_id).await?;
    let run = parse_close_run_row(sqlx::query("SELECT attempt_id, run_ordinal, status FROM close_runs WHERE attempt_id = ?1 ORDER BY run_ordinal DESC LIMIT 1")
        .bind(attempt_id).fetch_one(&mut **tx).await?)?.run;
    let mut remaining = Vec::new();
    for target in expected {
        if close_process_step_succeeded_tx(tx, &run, &target.scope, &target.resource).await? {
            continue;
        }
        let identity = target.resource.identity();
        let proof: Option<String> = sqlx::query_scalar(
            "SELECT proof_kind FROM close_retirement_resources
             WHERE attempt_id = ?1 AND scope = ?2 AND inspection_generation = ?3 AND inspection_fingerprint = ?4
               AND resource_kind = ?5 AND identity_kind = ?6 AND identity_codec = ?7 AND identity_value = ?8",
        ).bind(attempt_id).bind(target.scope.as_str()).bind(target.snapshot.generation()).bind(target.snapshot.fingerprint())
            .bind(target.resource.kind().as_str()).bind(identity.identity_kind()).bind(identity.codec()).bind(identity.value())
            .fetch_optional(&mut **tx).await?;
        if proof.as_deref().is_some_and(|kind| kind != "residual") {
            continue;
        }
        let disposition = if &target.scope == failed_scope && &target.resource == failed_resource {
            CloseCleanupResourceDisposition::Failed
        } else if proof.is_some() {
            CloseCleanupResourceDisposition::Residual
        } else {
            CloseCleanupResourceDisposition::Unattempted
        };
        remaining.push(CloseCleanupFailureResource {
            scope: target.scope,
            resource: target.resource,
            disposition,
        });
    }
    if !remaining
        .iter()
        .any(|target| target.disposition == CloseCleanupResourceDisposition::Failed)
    {
        return Err(close_precondition(
            "failed resource must be unresolved in the exact sealed inventory",
        ));
    }
    Ok(remaining)
}

async fn validate_cleanup_observed_resource_tx(
    tx: &mut Transaction<'_, Sqlite>,
    attempt_id: &CloseAttemptId,
    scope: &WorkScopeId,
    resource: &RetiredResourceIdentity,
) -> DbResult<()> {
    let row = sqlx::query("SELECT captured_worktree_identity, captured_worktree_fingerprint, captured_worktree_locator FROM close_attempt_scopes WHERE attempt_id = ?1 AND scope = ?2")
        .bind(attempt_id.as_str()).bind(scope.as_str()).fetch_optional(&mut **tx).await?
        .ok_or_else(|| close_precondition("observed resource scope is not captured by the Close attempt"))?;
    let identity = resource.identity();
    let is_sealed_expected: bool = sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM close_expected_retirement_resources expected
            JOIN close_retirement_inventories inventory
              ON inventory.attempt_id = expected.attempt_id AND inventory.scope = expected.scope
             AND inventory.inspection_generation = expected.inspection_generation
             AND inventory.inspection_fingerprint = expected.inspection_fingerprint
            WHERE expected.attempt_id = ?1 AND expected.scope = ?2
              AND expected.resource_kind = ?3 AND expected.identity_kind = ?4
              AND expected.identity_codec = ?5 AND expected.identity_value = ?6
              AND inventory.sealed = 1
         )",
    )
    .bind(attempt_id.as_str())
    .bind(scope.as_str())
    .bind(resource.kind().as_str())
    .bind(identity.identity_kind())
    .bind(identity.codec())
    .bind(identity.value())
    .fetch_one(&mut **tx)
    .await?;
    if is_sealed_expected {
        return Ok(());
    }
    let valid = match resource.identity() {
        LossItemIdentity::Worktree(worktree) => {
            row.try_get::<Option<String>, _>("captured_worktree_identity")?
                .as_deref()
                == Some(worktree.id().as_str())
                && row
                    .try_get::<Option<String>, _>("captured_worktree_fingerprint")?
                    .as_deref()
                    == Some(worktree.fingerprint().as_str())
                && row.try_get::<Option<String>, _>("captured_worktree_locator")?
                    == Some(worktree.locator().encode())
        }
        LossItemIdentity::Opaque(identity) if resource.kind() == RetiredResourceKind::WorkScope => {
            identity.as_str() == scope.as_str()
        }
        LossItemIdentity::Opaque(_) => matches!(
            resource.kind(),
            RetiredResourceKind::BashProcessGroup
                | RetiredResourceKind::PtySession
                | RetiredResourceKind::BrowserSession
                | RetiredResourceKind::EquivalentLiveResource
        ),
        LossItemIdentity::GitPath(_) | LossItemIdentity::GitOid(_) => false,
    };
    if !valid {
        return Err(close_precondition("observed resource requires exact captured scope/worktree or process-epoch identity; tmux requires sealed expected authority"));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseProcessResourceKind {
    BashProcessGroup,
    PtySession,
    BrowserSession,
}

impl CloseProcessResourceKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BashProcessGroup => "bash_process_group",
            Self::PtySession => "pty_session",
            Self::BrowserSession => "browser_session",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseProcessStepOutcome {
    Retired,
    AbsenceVerified,
}

impl CloseProcessStepOutcome {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Retired => "retired",
            Self::AbsenceVerified => "absence_verified",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloseProcessStepSuccess {
    pub run: CloseRunRef,
    pub scope: WorkScopeId,
    pub resource_kind: CloseProcessResourceKind,
    pub identity: OpaqueIdentity,
    pub outcome: CloseProcessStepOutcome,
    pub observed_at_us: i64,
}

async fn close_process_step_succeeded_tx(
    tx: &mut Transaction<'_, Sqlite>,
    run: &CloseRunRef,
    scope: &WorkScopeId,
    resource: &RetiredResourceIdentity,
) -> DbResult<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM close_process_step_successes
        WHERE attempt_id = ?1 AND run_ordinal = ?2 AND scope = ?3
          AND resource_kind = ?4 AND identity_value = ?5)",
    )
    .bind(run.attempt_id.as_str())
    .bind(run.ordinal.get())
    .bind(scope.as_str())
    .bind(resource.kind().as_str())
    .bind(resource.identity().value())
    .fetch_one(&mut **tx)
    .await?)
}

const VERIFIED_COMPLETION_SOURCE: &str = "SELECT EXISTS(SELECT 1 FROM close_runs run
    JOIN close_cleanup_failures failure ON failure.attempt_id = run.attempt_id
      AND failure.cleanup_run_ordinal = run.run_ordinal
    WHERE run.attempt_id = ?1 AND run.run_ordinal = ?2 AND run.run_ordinal > 1
      AND run.status = 'stopped' AND failure.authority_kind = 'attempt_interrupted'
      AND NOT EXISTS (SELECT 1 FROM close_cleanup_failure_resources child
        WHERE child.failure_occurrence_id = failure.failure_occurrence_id)
      AND ((run.retry_evidence_kind = 'resource_plan'
        AND EXISTS (SELECT 1 FROM close_run_retry_effects effect
          WHERE effect.attempt_id = run.attempt_id AND effect.run_ordinal = run.run_ordinal)
        AND NOT EXISTS (SELECT 1 FROM close_run_retry_effects effect
          WHERE effect.attempt_id = run.attempt_id AND effect.run_ordinal = run.run_ordinal
            AND NOT EXISTS (SELECT 1 FROM close_run_retry_successes success
              WHERE success.attempt_id = effect.attempt_id AND success.run_ordinal = effect.run_ordinal
                AND success.ordinal = effect.ordinal)))
      OR (run.retry_evidence_kind = 'verified_completion'
        AND NOT EXISTS (SELECT 1 FROM close_run_retry_effects effect
          WHERE effect.attempt_id = run.attempt_id AND effect.run_ordinal = run.run_ordinal))))";

async fn verified_completion_source_tx(
    tx: &mut Transaction<'_, Sqlite>,
    run: &CloseRunRef,
) -> DbResult<bool> {
    Ok(sqlx::query_scalar(VERIFIED_COMPLETION_SOURCE)
        .bind(run.attempt_id.as_str())
        .bind(run.ordinal.get())
        .fetch_one(&mut **tx)
        .await?)
}

async fn verified_completion_running_tx(
    tx: &mut Transaction<'_, Sqlite>,
    run: &CloseRunRef,
) -> DbResult<bool> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM close_runs run
        JOIN close_obligations obligation ON obligation.attempt_id = run.attempt_id
        WHERE run.attempt_id = ?1 AND run.run_ordinal = ?2 AND run.run_ordinal > 1
          AND run.run_ordinal = (SELECT MAX(run_ordinal) FROM close_runs WHERE attempt_id = run.attempt_id)
          AND run.status = 'running'
          AND obligation.phase = 'completed' AND obligation.close_outcome IN ('close_incomplete', 'archived_cleanup_attention')
          AND ((run.retry_evidence_kind = 'resource_plan'
            AND EXISTS (SELECT 1 FROM close_run_retry_effects effect WHERE effect.attempt_id = run.attempt_id AND effect.run_ordinal = run.run_ordinal)
            AND NOT EXISTS (SELECT 1 FROM close_run_retry_effects effect WHERE effect.attempt_id = run.attempt_id AND effect.run_ordinal = run.run_ordinal
              AND NOT EXISTS (SELECT 1 FROM close_run_retry_successes success WHERE success.attempt_id = effect.attempt_id AND success.run_ordinal = effect.run_ordinal AND success.ordinal = effect.ordinal)))
          OR (run.retry_evidence_kind = 'verified_completion'
            AND NOT EXISTS (SELECT 1 FROM close_run_retry_effects effect WHERE effect.attempt_id = run.attempt_id AND effect.run_ordinal = run.run_ordinal))))")
        .bind(run.attempt_id.as_str()).bind(run.ordinal.get()).fetch_one(&mut **tx).await?)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseRetryRequestedBy {
    User,
    Global,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloseSafeRetryEffect {
    pub scope: WorkScopeId,
    pub resource: RetiredResourceIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloseSafeRetryProgress {
    pub successful: Vec<CloseSafeRetryEffect>,
    pub pending: Vec<CloseSafeRetryEffect>,
}

async fn close_safe_retry_progress_tx(
    tx: &mut Transaction<'_, Sqlite>,
    run: &CloseRunRef,
) -> DbResult<CloseSafeRetryProgress> {
    let valid: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM close_runs WHERE attempt_id = ?1 AND run_ordinal = ?2
         AND run_ordinal = (SELECT MAX(run_ordinal) FROM close_runs WHERE attempt_id = ?1)
         AND status = 'running' AND retry_evidence_kind = 'resource_plan')",
    )
    .bind(run.attempt_id.as_str())
    .bind(run.ordinal.get())
    .fetch_one(&mut **tx)
    .await?;
    if !valid {
        return Err(close_precondition(
            "retry progress requires the exact latest running resource plan",
        ));
    }
    let rows = sqlx::query(
        "SELECT effect.*, captured.captured_worktree_fingerprint, captured.captured_worktree_locator,
                success.ordinal AS success_ordinal
         FROM close_run_retry_effects effect
         JOIN close_attempt_scopes captured ON captured.attempt_id = effect.attempt_id AND captured.scope = effect.scope
         LEFT JOIN close_run_retry_successes success ON success.attempt_id = effect.attempt_id
           AND success.run_ordinal = effect.run_ordinal AND success.ordinal = effect.ordinal
         WHERE effect.attempt_id = ?1 AND effect.run_ordinal = ?2 ORDER BY effect.ordinal",
    )
    .bind(run.attempt_id.as_str())
    .bind(run.ordinal.get())
    .fetch_all(&mut **tx)
    .await?;
    let mut progress = CloseSafeRetryProgress {
        successful: Vec::new(),
        pending: Vec::new(),
    };
    for row in rows {
        let effect = CloseSafeRetryEffect {
            scope: WorkScopeId::parse(row.try_get::<String, _>("scope")?)
                .map_err(|error| DbError::Serialization(error.to_string()))?,
            resource: parse_cleanup_resource_identity(&row)?,
        };
        if row.try_get::<Option<i64>, _>("success_ordinal")?.is_some() {
            progress.successful.push(effect);
        } else {
            progress.pending.push(effect);
        }
    }
    if progress.successful.is_empty() && progress.pending.is_empty() {
        return Err(close_precondition("retry resource plan is empty"));
    }
    Ok(progress)
}

async fn retry_failure_resources_tx(
    tx: &mut Transaction<'_, Sqlite>,
    run: &CloseRunRef,
    progress: &CloseSafeRetryProgress,
    failed: &CloseSafeRetryEffect,
) -> DbResult<Vec<CloseCleanupFailureResource>> {
    if !progress.pending.contains(failed) {
        return Err(close_precondition(
            "retry failure requires a pending exact-run effect",
        ));
    }
    let mut remaining = Vec::with_capacity(progress.pending.len());
    for effect in &progress.pending {
        let identity = effect.resource.identity();
        let proof: Option<String> = sqlx::query_scalar(
            "SELECT receipt.proof_kind FROM close_retirement_resources receipt
             JOIN close_obligations obligation ON obligation.attempt_id = receipt.attempt_id
             WHERE receipt.attempt_id = ?1 AND receipt.scope = ?2
               AND receipt.inspection_generation = obligation.inspection_generation
               AND receipt.inspection_fingerprint = obligation.inspection_fingerprint
               AND receipt.resource_kind = ?3 AND receipt.identity_kind = ?4
               AND receipt.identity_codec = ?5 AND receipt.identity_value = ?6",
        )
        .bind(run.attempt_id.as_str())
        .bind(effect.scope.as_str())
        .bind(effect.resource.kind().as_str())
        .bind(identity.identity_kind())
        .bind(identity.codec())
        .bind(identity.value())
        .fetch_optional(&mut **tx)
        .await?;
        remaining.push(CloseCleanupFailureResource {
            scope: effect.scope.clone(),
            resource: effect.resource.clone(),
            disposition: if effect == failed {
                CloseCleanupResourceDisposition::Failed
            } else if proof.as_deref() == Some("residual") {
                CloseCleanupResourceDisposition::Residual
            } else {
                CloseCleanupResourceDisposition::Unattempted
            },
        });
    }
    Ok(remaining)
}

/// Read-only safety evidence supplied by the explicit user/Global retry command.
#[derive(Debug, Clone)]
pub struct AdmitCloseSafeRetryRequest {
    pub failed_run: CloseRunRef,
    pub requested_by: CloseRetryRequestedBy,
    pub observed_at_us: i64,
    pub precondition_resolution: String,
    pub safety_evidence: String,
    pub remaining_effects: Vec<CloseSafeRetryEffect>,
}

#[derive(Debug, Clone)]
pub struct RouteCloseAttemptToRepairRequest {
    pub attempt_id: CloseAttemptId,
    pub scope: WorkScopeId,
    pub residual: RetiredResourceIdentity,
    pub reason: RetirementFailureReason,
    pub detail: String,
}

#[derive(Debug, Clone)]
pub struct RecordCloseRetirementDispatchRequest {
    pub attempt_id: CloseAttemptId,
    pub scope: WorkScopeId,
    pub snapshot: CloseRetirementSnapshot,
    pub resource: RetiredResourceIdentity,
}

#[derive(Debug, Clone)]
pub struct RecordCloseWorktreeCleanupPlanRequest {
    pub attempt_id: CloseAttemptId,
    pub scope: WorkScopeId,
    pub snapshot: CloseRetirementSnapshot,
    pub resource: RetiredResourceIdentity,
    pub administrative_dir_incarnation: String,
    pub administrative_dir: std::path::PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloseWorktreeFinalTombstone {
    pub root: std::path::PathBuf,
    pub device: u64,
    pub inode: u64,
    pub object_device: Option<u64>,
    pub object_inode: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct BindCloseWorktreeFinalTombstoneRequest {
    pub attempt_id: CloseAttemptId,
    pub scope: WorkScopeId,
    pub snapshot: CloseRetirementSnapshot,
    pub resource: RetiredResourceIdentity,
    pub tombstone: CloseWorktreeFinalTombstone,
}

#[derive(Debug, Clone)]
pub struct BindCloseWorktreeFinalTombstoneObjectRequest {
    pub attempt_id: CloseAttemptId,
    pub scope: WorkScopeId,
    pub snapshot: CloseRetirementSnapshot,
    pub resource: RetiredResourceIdentity,
    pub object_device: u64,
    pub object_inode: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloseWorktreeCleanupPlan {
    pub administrative_dir: std::path::PathBuf,
    pub administrative_dir_incarnation: String,
    pub final_tombstone: Option<CloseWorktreeFinalTombstone>,
}

async fn close_obligation_for_update(
    tx: &mut Transaction<'_, Sqlite>,
    attempt_id: &str,
) -> DbResult<CloseObligation> {
    sqlx::query(
        "SELECT attempt_id, product_conversation_id, phase, inspection_generation,
                inspection_fingerprint, created_at, updated_at, completed_at, close_outcome
         FROM close_obligations WHERE attempt_id = ?1",
    )
    .bind(attempt_id)
    .fetch_optional(&mut **tx)
    .await?
    .map(parse_close_obligation_row)
    .transpose()?
    .ok_or_else(|| DbError::CloseFoundationNotFound(attempt_id.to_string()))
}

async fn set_close_phase_tx(
    tx: &mut Transaction<'_, Sqlite>,
    attempt_id: &str,
    phase: ClosePhase,
) -> DbResult<()> {
    sqlx::query("UPDATE close_obligations SET phase = ?2 WHERE attempt_id = ?1")
        .bind(attempt_id)
        .bind(phase.as_str())
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn read_topology_tx(
    tx: &mut Transaction<'_, Sqlite>,
    product_conversation_id: &ProductConversationId,
) -> DbResult<Option<CloseFoundationTopology>> {
    let rows = sqlx::query(
        "WITH RECURSIVE root AS (
             SELECT candidate.id
             FROM conversations candidate
             WHERE candidate.product_conversation_id = ?1
               AND candidate.parent_conversation_id IS NULL
               AND candidate.runtime_role = 'user'
               AND NOT EXISTS (
                   SELECT 1 FROM conversations predecessor
                   WHERE predecessor.continued_in_conv_id = candidate.id
               )
         ),
         forward(id, next_id, depth, path) AS (
             SELECT c.id, c.continued_in_conv_id, 0, json_array(c.id)
             FROM conversations c
             JOIN root r ON r.id = c.id
             UNION ALL
             SELECT c.id, c.continued_in_conv_id, forward.depth + 1,
                    json_insert(forward.path, '$[#]', c.id)
             FROM conversations c
             JOIN forward ON c.id = forward.next_id
             WHERE c.product_conversation_id = ?1
               AND c.parent_conversation_id IS NULL
               AND NOT EXISTS (
                   SELECT 1 FROM json_each(forward.path) visited WHERE visited.value = c.id
               )
         )
         SELECT c.id, c.product_conversation_id, c.slug, c.title,
                COALESCE(c.sub_agent_cwd_override, e.cwd, '') AS cwd,
                c.parent_conversation_id, c.user_initiated, c.state,
                c.state_updated_at, c.created_at, c.updated_at, c.archived,
                c.transcript_generation, c.model, c.effort,
                c.project_id, c.desired_base_branch,
                c.runtime_role, c.work_scope_id,
                c.cm_kind, e.branch_name AS env_branch_name,
                e.worktree_path AS env_worktree_path, e.base_branch AS env_base_branch,
                c.cm_task_id, c.cm_task_title, c.cm_next_taskmd_id_hint,
                c.seed_parent_id, c.seed_label, c.continued_in_conv_id, c.chain_name,
                c.llm_language, c.spawned_from_conversation_id,
                (SELECT COUNT(*) FROM messages m WHERE m.conversation_id = c.id) AS message_count,
                forward.depth AS close_depth,
                CASE
                    WHEN forward.depth = 0 AND forward.next_id IS NULL THEN 'root_latest'
                    WHEN forward.depth = 0 THEN 'root'
                    WHEN forward.next_id IS NULL THEN 'latest'
                    ELSE 'intermediate'
                END AS close_role
         FROM forward
         JOIN conversations c ON c.id = forward.id
         LEFT JOIN work_scope_environments e ON e.work_scope_id = c.work_scope_id
         ORDER BY forward.depth",
    )
    .bind(product_conversation_id.as_str())
    .fetch_all(&mut **tx)
    .await?;

    if rows.is_empty() {
        return Ok(None);
    }

    let mut members = Vec::with_capacity(rows.len());
    for row in rows {
        let role = parse_close_member_role(&row.try_get::<String, _>("close_role")?)?;
        let conversation = parse_conversation_row(row)?;
        members.push(CloseFoundationTopologyMember { conversation, role });
    }
    let root = members
        .first()
        .cloned()
        .ok_or_else(|| DbError::Serialization("topology missing root".to_string()))?;
    let latest = members
        .last()
        .cloned()
        .ok_or_else(|| DbError::Serialization("topology missing latest".to_string()))?;

    let topology = CloseFoundationTopology {
        root: root.conversation,
        latest: latest.conversation,
        members,
    };
    validate_topology_tx(tx, &topology, product_conversation_id).await?;
    Ok(Some(topology))
}

async fn validate_topology_tx(
    tx: &mut Transaction<'_, Sqlite>,
    topology: &CloseFoundationTopology,
    product_conversation_id: &ProductConversationId,
) -> DbResult<()> {
    if topology.members.is_empty() {
        return Err(close_precondition("topology is empty"));
    }
    if topology
        .members
        .iter()
        .any(|member| member.conversation.product_conversation_id != *product_conversation_id)
    {
        return Err(close_precondition(format!(
            "topology contains a conversation outside ProductConversation {product_conversation_id}"
        )));
    }

    let mut ids = std::collections::BTreeSet::new();
    for member in &topology.members {
        if !ids.insert(member.conversation.id.clone()) {
            return Err(close_precondition(format!(
                "topology contains duplicate member {}",
                member.conversation.id
            )));
        }
    }

    for (index, member) in topology.members.iter().enumerate() {
        let (predecessor_count, live_next): (i64, Option<String>) = sqlx::query_as(
            "SELECT
                 (SELECT COUNT(*) FROM conversations predecessor
                  WHERE predecessor.continued_in_conv_id = current.id) AS predecessor_count,
                 current.continued_in_conv_id AS live_next
             FROM conversations current
             WHERE current.id = ?1",
        )
        .bind(&member.conversation.id)
        .fetch_one(&mut **tx)
        .await?;
        if index == 0 {
            if predecessor_count != 0 {
                return Err(close_precondition(format!(
                    "topology root {} has {} predecessors",
                    member.conversation.id, predecessor_count
                )));
            }
        } else if predecessor_count != 1 {
            return Err(close_precondition(format!(
                "topology member {} has {} predecessors",
                member.conversation.id, predecessor_count
            )));
        }

        let expected_next = topology
            .members
            .get(index + 1)
            .map(|next| next.conversation.id.as_str());
        if live_next.as_deref() != expected_next {
            return Err(close_precondition(format!(
                "topology member {} next {:?} does not match expected {:?}",
                member.conversation.id, live_next, expected_next
            )));
        }
    }

    Ok(())
}

fn validate_begin_preconditions(
    topology: &CloseFoundationTopology,
    addressed_id: &str,
) -> DbResult<()> {
    let root = &topology.root;
    let latest = &topology.latest;

    if root.runtime_role != RuntimeRole::User {
        return Err(close_precondition(
            "root conversation must have runtime_role=user",
        ));
    }
    if !root.user_initiated {
        return Err(close_precondition(
            "root conversation must be user initiated",
        ));
    }
    if addressed_id != latest.id {
        return Err(DbError::CloseFoundationStaleLatest {
            expected: addressed_id.to_string(),
            actual: latest.id.clone(),
        });
    }
    if matches!(latest.state, ConvState::HandedOff { .. }) {
        return Err(close_precondition(
            "latest conversation has handed off without a usable continuation",
        ));
    }
    if topology.members.iter().any(|member| {
        matches!(
            member.conversation.state,
            ConvState::AwaitingTaskApproval { .. }
        )
    }) {
        return Err(close_precondition(
            "a chain member is awaiting task approval",
        ));
    }
    if topology.members.iter().any(|member| {
        matches!(
            member.conversation.state,
            ConvState::AwaitingContinuation { .. }
        )
    }) {
        return Err(close_precondition(
            "a chain member is awaiting continuation",
        ));
    }
    Ok(())
}

// These close-foundation database APIs all return the same DbError contract:
fn encode_aggregate_snapshot_component<'a>(
    scopes: impl IntoIterator<Item = (&'a WorkScopeId, &'a str)>,
) -> String {
    let mut scopes = scopes.into_iter().peekable();
    if scopes.peek().is_none() {
        return "no-worktree".to_string();
    }
    let mut encoded = String::from("v1");
    for (scope, value) in scopes {
        let scope = scope.as_str();
        write!(encoded, "{}:{scope}{}:{value}", scope.len(), value.len())
            .expect("writing to String cannot fail");
    }
    encoded
}

// The close-foundation database API intentionally exposes `DbResult` for
// storage failures from sqlx, row decoding/serialization failures, and explicit
// close-foundation precondition/not-found errors enforced by this module.
#[allow(clippy::missing_errors_doc)]
impl Database {
    pub async fn product_conversation_admission(
        &self,
        conversation_id: &str,
    ) -> DbResult<ProductConversationAdmission> {
        let mut tx = self.pool.begin().await?;
        let admission = admit_product_conversation_operation_tx(&mut tx, conversation_id).await?;
        tx.commit().await?;
        Ok(admission)
    }

    pub async fn message_target_admission(
        &self,
        conversation_id: &str,
    ) -> DbResult<MessageTargetAdmission> {
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query(
            "SELECT conversation.archived, product.kind
             FROM conversations conversation
             JOIN product_conversations product
               ON product.id = conversation.product_conversation_id
             WHERE conversation.id = ?1",
        )
        .bind(conversation_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| DbError::ConversationNotFound(conversation_id.to_string()))?;
        let admission = if row.try_get::<String, _>("kind")? == "ordinary" {
            MessageTargetAdmission::Aggregate(
                admit_product_conversation_operation_tx(&mut tx, conversation_id).await?,
            )
        } else if row.try_get::<bool, _>("archived")? {
            MessageTargetAdmission::StandaloneArchived
        } else {
            MessageTargetAdmission::StandaloneAvailable
        };
        tx.commit().await?;
        Ok(admission)
    }

    pub async fn close_foundation_topology(
        &self,
        product_conversation_id: &ProductConversationId,
    ) -> DbResult<CloseFoundationTopology> {
        let mut tx = self.pool.begin().await?;
        let topology = read_topology_tx(&mut tx, product_conversation_id)
            .await?
            .ok_or_else(|| DbError::CloseFoundationNotFound(product_conversation_id.to_string()))?;
        tx.commit().await?;
        Ok(topology)
    }

    pub async fn begin_close_foundation(
        &self,
        product_conversation_id: &ProductConversationId,
        expected_latest_transcript_id: &TranscriptConversationId,
        attempt_id: &str,
    ) -> DbResult<CloseObligation> {
        self.begin_close_foundation_inner(
            product_conversation_id,
            expected_latest_transcript_id,
            attempt_id,
            false,
        )
        .await
    }

    pub async fn begin_direct_close_foundation(
        &self,
        product_conversation_id: &ProductConversationId,
        expected_latest_transcript_id: &TranscriptConversationId,
        attempt_id: &str,
    ) -> DbResult<CloseObligation> {
        self.begin_close_foundation_inner(
            product_conversation_id,
            expected_latest_transcript_id,
            attempt_id,
            true,
        )
        .await
    }

    #[allow(clippy::too_many_lines)]
    async fn begin_close_foundation_inner(
        &self,
        product_conversation_id: &ProductConversationId,
        expected_latest_transcript_id: &TranscriptConversationId,
        attempt_id: &str,
        require_idle_latest: bool,
    ) -> DbResult<CloseObligation> {
        let mut conn = self.pool.acquire().await?;
        let mut tx = conn.begin_with("BEGIN IMMEDIATE").await?;
        #[cfg(test)]
        if let Some(latch) = &self.close_foundation_test_latch {
            latch.transaction_entered.notify_waiters();
            latch.release_transaction.notified().await;
        }
        if let Some(row) = sqlx::query(
            "SELECT attempt_id, product_conversation_id, phase, inspection_generation,
                    inspection_fingerprint, created_at, updated_at, completed_at, close_outcome,
                    topology_sealed
             FROM close_obligations WHERE attempt_id = ?1",
        )
        .bind(attempt_id)
        .fetch_optional(&mut *tx)
        .await?
        {
            if row.try_get::<i64, _>("topology_sealed")? != 1 {
                return Err(DbError::CloseFoundationConflict(format!(
                    "attempt {attempt_id} does not have a complete sealed topology"
                )));
            }
            let obligation = parse_close_obligation_row(row)?;
            let captured_members = sqlx::query(
                "SELECT conversation_id, member_role
                 FROM close_attempt_members
                 WHERE attempt_id = ?1 AND member_role IN ('latest', 'root_latest')",
            )
            .bind(attempt_id)
            .fetch_all(&mut *tx)
            .await?;
            if obligation.product_conversation_id() != product_conversation_id {
                return Err(DbError::CloseFoundationConflict(format!(
                    "attempt {attempt_id} belongs to ProductConversation {}, not {}",
                    obligation.product_conversation_id(),
                    product_conversation_id
                )));
            }
            if captured_members.len() != 1 {
                return Err(DbError::CloseFoundationConflict(format!(
                    "attempt {attempt_id} does not capture exactly one latest transcript"
                )));
            }
            let topology = read_topology_tx(&mut tx, product_conversation_id)
                .await?
                .ok_or_else(|| {
                    DbError::CloseFoundationNotFound(product_conversation_id.to_string())
                })?;
            validate_begin_preconditions(&topology, expected_latest_transcript_id.as_str())?;
            let captured_latest = parse_transcript_conversation_id(
                captured_members[0].try_get("conversation_id")?,
                "close_attempt_members.conversation_id",
            )?;
            if captured_latest != *expected_latest_transcript_id {
                return Err(DbError::CloseFoundationConflict(format!(
                    "attempt {attempt_id} captures latest transcript {captured_latest}, not {expected_latest_transcript_id}"
                )));
            }

            tx.commit().await?;
            return Ok(obligation);
        }

        let lifecycle: Option<String> = sqlx::query_scalar(
            "SELECT ordinary_lifecycle FROM product_conversations
             WHERE id = ?1 AND kind = 'ordinary'",
        )
        .bind(product_conversation_id.as_str())
        .fetch_optional(&mut *tx)
        .await?;
        if lifecycle.as_deref() == Some("history") {
            return Err(DbError::CloseFoundationConflict(format!(
                "ProductConversation {product_conversation_id} is already in History"
            )));
        }
        sqlx::query(
            "UPDATE conversations SET archived = 0
             WHERE product_conversation_id = ?1 AND archived = 1",
        )
        .bind(product_conversation_id.as_str())
        .execute(&mut *tx)
        .await?;
        let topology = read_topology_tx(&mut tx, product_conversation_id)
            .await?
            .ok_or_else(|| DbError::CloseFoundationNotFound(product_conversation_id.to_string()))?;
        validate_begin_preconditions(&topology, expected_latest_transcript_id.as_str())?;

        if require_idle_latest
            && CapturedConversationStateKind::from_db_str(conv_state_kind(&topology.latest.state))
                .is_some_and(CapturedConversationStateKind::is_busy)
        {
            return Err(DbError::CloseFoundationConflict(format!(
                "ProductConversation {product_conversation_id} latest transcript is working"
            )));
        }

        if let Some(row) = sqlx::query(
            "SELECT attempt_id, product_conversation_id, phase, inspection_generation,
                    inspection_fingerprint, created_at, updated_at, completed_at, close_outcome
             FROM close_obligations
             WHERE product_conversation_id = ?1 AND (phase <> 'completed' OR close_outcome = 'close_incomplete')",
        )
        .bind(product_conversation_id.as_str())
        .fetch_optional(&mut *tx)
        .await?
        {
            let obligation = parse_close_obligation_row(row)?;
            return Err(DbError::CloseFoundationConflict(format!(
                "ProductConversation {} already has active close attempt {} in phase {}",
                product_conversation_id,
                obligation.attempt_id(),
                obligation.phase().as_str()
            )));
        }

        let automatic_continuation_pending: i64 = sqlx::query_scalar(
            "SELECT EXISTS(
                 SELECT 1 FROM automatic_continuation_admissions
                 WHERE product_conversation_id = ?1
                   AND phase NOT IN ('message_settled', 'superseded', 'failed')
             )",
        )
        .bind(product_conversation_id.as_str())
        .fetch_one(&mut *tx)
        .await?;
        if automatic_continuation_pending != 0 {
            return Err(DbError::CloseFoundationConflict(format!(
                "ProductConversation {product_conversation_id} has pending automatic continuation"
            )));
        }

        let now_utc = Utc::now();
        let now = now_utc.to_rfc3339();
        let captured_at_unix_micros = now_utc.timestamp_micros();
        sqlx::query(
            "INSERT INTO close_obligations (
                 attempt_id, product_conversation_id, phase, created_at, updated_at, completed_at
             ) VALUES (?1, ?2, ?3, ?4, ?4, NULL)",
        )
        .bind(attempt_id)
        .bind(product_conversation_id.as_str())
        .bind(ClosePhase::AwaitingBlockerResolution.as_str())
        .bind(&now)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "INSERT INTO close_attempt_participants (
                 attempt_id, product_conversation_id, conversation_id, captured_at_unix_micros
             )
             SELECT ?1, ?3, id, ?2 FROM conversations
             WHERE product_conversation_id = ?3",
        )
        .bind(attempt_id)
        .bind(captured_at_unix_micros)
        .bind(product_conversation_id.as_str())
        .execute(&mut *tx)
        .await?;

        for (continuation_ordinal, member) in topology.members.iter().enumerate() {
            sqlx::query(
                "INSERT INTO close_attempt_members (
                     attempt_id, conversation_id, member_role, continuation_ordinal,
                     captured_continued_in_conv_id, captured_state_kind, captured_runtime_role,
                     captured_work_scope_id, captured_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )
            .bind(attempt_id)
            .bind(&member.conversation.id)
            .bind(close_member_role_db_str(member.role))
            .bind(i64::try_from(continuation_ordinal).map_err(|error| {
                DbError::Serialization(format!("continuation ordinal overflow: {error}"))
            })?)
            .bind(member.conversation.continued_in_conv_id.as_deref())
            .bind(conv_state_kind(&member.conversation.state))
            .bind(member.conversation.runtime_role.as_str())
            .bind(
                member
                    .conversation
                    .attached_work_scope_id
                    .as_ref()
                    .map(WorkScopeId::as_str),
            )
            .bind(&now)
            .execute(&mut *tx)
            .await?;
        }

        let mut distinct_scopes = std::collections::BTreeSet::new();
        for member in &topology.members {
            if let Some(scope) = &member.conversation.attached_work_scope_id {
                distinct_scopes.insert(scope.clone());
            }
        }
        for scope in distinct_scopes {
            let captured_worktree =
                sqlx::query_as::<_, (Option<String>, Option<String>, Option<String>)>(
                    "SELECT worktree_id, worktree_fingerprint,
                        CASE WHEN environment_kind = 'allocated_worktree' THEN
                            'git_path_bytes_hex_v1:' || lower(hex(CAST(worktree_path AS BLOB)))
                        END
                 FROM work_scopes WHERE id = ?1",
                )
                .bind(scope.as_str())
                .fetch_one(&mut *tx)
                .await?;
            sqlx::query(
                "INSERT INTO close_attempt_scopes (
                     attempt_id, scope, captured_worktree_identity,
                     captured_worktree_fingerprint, captured_worktree_locator, captured_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )
            .bind(attempt_id)
            .bind(scope.as_str())
            .bind(captured_worktree.0)
            .bind(captured_worktree.1)
            .bind(captured_worktree.2)
            .bind(&now)
            .execute(&mut *tx)
            .await?;
        }

        sqlx::query("UPDATE close_obligations SET topology_sealed = 1 WHERE attempt_id = ?1")
            .bind(attempt_id)
            .execute(&mut *tx)
            .await?;

        let row = sqlx::query(
            "SELECT attempt_id, product_conversation_id, phase, inspection_generation,
                    inspection_fingerprint, created_at, updated_at, completed_at, close_outcome
             FROM close_obligations WHERE attempt_id = ?1",
        )
        .bind(attempt_id)
        .fetch_one(&mut *tx)
        .await?;
        let obligation = parse_close_obligation_row(row)?;
        tx.commit().await?;
        Ok(obligation)
    }

    async fn capture_close_direct_turn_settlement_targets_tx(
        tx: &mut Transaction<'_, Sqlite>,
        attempt_id: &str,
    ) -> DbResult<()> {
        sqlx::query(
            "INSERT INTO close_attempt_direct_turn_settlement_captures (attempt_id, captured_at)
             VALUES (?1, ?2)",
        )
        .bind(attempt_id)
        .bind(Utc::now().to_rfc3339())
        .execute(&mut **tx)
        .await?;
        sqlx::query(
            "INSERT INTO close_attempt_direct_turn_settlements (
                 attempt_id, turn_id, expected_generation
             )
             SELECT ?1, turn.turn_id, turn.generation
             FROM durable_turns turn
             JOIN close_attempt_participants participant
               ON participant.conversation_id = turn.conversation_id
             WHERE participant.attempt_id = ?1
               AND turn.owns_conversation = 1 AND turn.terminal_kind IS NULL",
        )
        .bind(attempt_id)
        .execute(&mut **tx)
        .await?;
        Ok(())
    }

    pub async fn confirm_close_stop_work(&self, attempt_id: &str) -> DbResult<CloseObligation> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let obligation = close_obligation_for_update(&mut tx, attempt_id).await?;
        match obligation.phase() {
            ClosePhase::AwaitingBlockerResolution => {
                set_close_phase_tx(
                    &mut tx,
                    attempt_id,
                    ClosePhase::AwaitingStopWorkConfirmation,
                )
                .await?;
            }
            ClosePhase::AwaitingStopWorkConfirmation => {}
            phase @ (ClosePhase::SettlingActiveWork
            | ClosePhase::CancelRequestedDuringSettlement
            | ClosePhase::AwaitingRetirementInspection
            | ClosePhase::AwaitingLossConfirmation
            | ClosePhase::RetirementRequested
            | ClosePhase::NeedsRepair
            | ClosePhase::Completed) => {
                return Err(close_precondition(format!(
                    "attempt {attempt_id} phase {} does not admit stop-work confirmation",
                    phase.as_str()
                )));
            }
        }
        let obligation = close_obligation_for_update(&mut tx, attempt_id).await?;
        tx.commit().await?;
        Ok(obligation)
    }

    pub async fn begin_close_idle_settlement(&self, attempt_id: &str) -> DbResult<CloseObligation> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let obligation = close_obligation_for_update(&mut tx, attempt_id).await?;
        match obligation.phase() {
            ClosePhase::AwaitingBlockerResolution => {
                set_close_phase_tx(&mut tx, attempt_id, ClosePhase::SettlingActiveWork).await?;
                Self::capture_close_direct_turn_settlement_targets_tx(&mut tx, attempt_id).await?;
            }
            ClosePhase::SettlingActiveWork => {}
            phase @ (ClosePhase::AwaitingStopWorkConfirmation
            | ClosePhase::CancelRequestedDuringSettlement
            | ClosePhase::AwaitingRetirementInspection
            | ClosePhase::AwaitingLossConfirmation
            | ClosePhase::RetirementRequested
            | ClosePhase::NeedsRepair
            | ClosePhase::Completed) => {
                return Err(close_precondition(format!(
                    "attempt {attempt_id} phase {} does not admit idle settlement",
                    phase.as_str()
                )));
            }
        }
        let obligation = close_obligation_for_update(&mut tx, attempt_id).await?;
        tx.commit().await?;
        Ok(obligation)
    }

    pub async fn begin_close_active_work_settlement(
        &self,
        attempt_id: &str,
    ) -> DbResult<CloseObligation> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let obligation = close_obligation_for_update(&mut tx, attempt_id).await?;
        match obligation.phase() {
            ClosePhase::AwaitingStopWorkConfirmation => {
                set_close_phase_tx(&mut tx, attempt_id, ClosePhase::SettlingActiveWork).await?;
                Self::capture_close_direct_turn_settlement_targets_tx(&mut tx, attempt_id).await?;
            }
            ClosePhase::SettlingActiveWork => {}
            phase @ (ClosePhase::AwaitingBlockerResolution
            | ClosePhase::CancelRequestedDuringSettlement
            | ClosePhase::AwaitingRetirementInspection
            | ClosePhase::AwaitingLossConfirmation
            | ClosePhase::RetirementRequested
            | ClosePhase::NeedsRepair
            | ClosePhase::Completed) => {
                return Err(close_precondition(format!(
                    "attempt {attempt_id} phase {} does not admit active-work settlement",
                    phase.as_str()
                )));
            }
        }
        let obligation = close_obligation_for_update(&mut tx, attempt_id).await?;
        tx.commit().await?;
        Ok(obligation)
    }

    pub async fn cancel_close_before_retirement(
        &self,
        attempt_id: &str,
    ) -> DbResult<CloseObligation> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let obligation = close_obligation_for_update(&mut tx, attempt_id).await?;
        match obligation.phase() {
            ClosePhase::SettlingActiveWork => {
                set_close_phase_tx(
                    &mut tx,
                    attempt_id,
                    ClosePhase::CancelRequestedDuringSettlement,
                )
                .await?;
            }
            ClosePhase::CancelRequestedDuringSettlement => {}
            ClosePhase::AwaitingBlockerResolution
            | ClosePhase::AwaitingStopWorkConfirmation
            | ClosePhase::AwaitingRetirementInspection
            | ClosePhase::AwaitingLossConfirmation => {
                let now = Utc::now().to_rfc3339();
                sqlx::query(
                    "UPDATE close_obligations
                     SET phase = 'completed', completed_at = ?2, updated_at = ?2,
                         close_outcome = 'cancelled',
                         inspection_generation = NULL, inspection_fingerprint = NULL
                     WHERE attempt_id = ?1 AND phase = ?3",
                )
                .bind(attempt_id)
                .bind(now)
                .bind(obligation.phase().as_str())
                .execute(&mut *tx)
                .await?;
            }
            phase @ (ClosePhase::RetirementRequested
            | ClosePhase::NeedsRepair
            | ClosePhase::Completed) => {
                return Err(close_precondition(format!(
                    "attempt {attempt_id} phase {} does not admit pre-retirement cancellation",
                    phase.as_str()
                )));
            }
        }
        let obligation = close_obligation_for_update(&mut tx, attempt_id).await?;
        tx.commit().await?;
        Ok(obligation)
    }

    pub async fn request_close_settlement_cancellation(
        &self,
        attempt_id: &str,
    ) -> DbResult<CloseObligation> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let obligation = close_obligation_for_update(&mut tx, attempt_id).await?;
        match obligation.phase() {
            ClosePhase::SettlingActiveWork => {
                set_close_phase_tx(
                    &mut tx,
                    attempt_id,
                    ClosePhase::CancelRequestedDuringSettlement,
                )
                .await?;
            }
            ClosePhase::CancelRequestedDuringSettlement => {}
            phase @ (ClosePhase::AwaitingBlockerResolution
            | ClosePhase::AwaitingStopWorkConfirmation
            | ClosePhase::AwaitingRetirementInspection
            | ClosePhase::AwaitingLossConfirmation
            | ClosePhase::RetirementRequested
            | ClosePhase::NeedsRepair
            | ClosePhase::Completed) => {
                return Err(close_precondition(format!(
                    "attempt {attempt_id} phase {} does not admit settlement cancellation",
                    phase.as_str()
                )));
            }
        }
        let obligation = close_obligation_for_update(&mut tx, attempt_id).await?;
        tx.commit().await?;
        Ok(obligation)
    }

    async fn reconcile_close_direct_turn_settlement_receipts_tx(
        tx: &mut Transaction<'_, Sqlite>,
        attempt_id: &str,
    ) -> DbResult<()> {
        sqlx::query(
            "UPDATE close_attempt_direct_turn_settlements
             SET settled_at = ?2
             WHERE attempt_id = ?1 AND settled_at IS NULL
               AND EXISTS (
                 SELECT 1 FROM durable_turns turn
                 WHERE turn.turn_id = close_attempt_direct_turn_settlements.turn_id
                   AND turn.terminal_kind IS NOT NULL
                   AND turn.owns_conversation = 0
                   AND turn.generation = close_attempt_direct_turn_settlements.expected_generation + 1
               )",
        )
        .bind(attempt_id)
        .bind(Utc::now().to_rfc3339())
        .execute(&mut **tx)
        .await?;
        let unsettled: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM close_attempt_direct_turn_settlements
             WHERE attempt_id = ?1 AND settled_at IS NULL",
        )
        .bind(attempt_id)
        .fetch_one(&mut **tx)
        .await?;
        if unsettled == 0 {
            Ok(())
        } else {
            Err(close_precondition(format!(
                "attempt {attempt_id} still has {unsettled} unsettled direct-turn receipt(s)"
            )))
        }
    }

    async fn require_close_participants_quiescent_tx(
        tx: &mut Transaction<'_, Sqlite>,
        attempt_id: &str,
    ) -> DbResult<()> {
        let active_members: i64 = sqlx::query_scalar(
            "SELECT COUNT(*)
             FROM close_attempt_participants captured
             JOIN conversations participant ON participant.id = captured.conversation_id
             WHERE captured.attempt_id = ?1
               AND captured.settlement_state = 'live'
               AND (
                 EXISTS (
                   SELECT 1 FROM durable_turns turn
                   WHERE turn.conversation_id = participant.id
                     AND (
                       (turn.owns_conversation = 1 AND turn.terminal_kind IS NULL)
                       OR EXISTS (
                         SELECT 1 FROM direct_turn_terminal_obligations terminal
                         WHERE terminal.turn_id = turn.turn_id
                       )
                     )
                 )
                 OR EXISTS (
                   SELECT 1 FROM conversation_creation_jobs creation
                   WHERE creation.conversation_id = participant.id
                     AND (
                       creation.status IN ('accepted', 'claimed', 'retry_scheduled', 'cancelling')
                       OR (creation.status = 'failed' AND EXISTS (
                         SELECT 1 FROM conversation_creation_resource_reservations reservation
                         WHERE reservation.job_id = creation.id AND reservation.status != 'released'
                       ))
                     )
                 )
                 OR EXISTS (
                   SELECT 1 FROM wake_bindings binding
                   JOIN workflows workflow ON workflow.workflow_id = binding.workflow_id
                   WHERE binding.conversation_id = participant.id
                     AND (
                       binding.resolved_at IS NULL
                       OR workflow.status IN ('Active', 'Cancelling', 'ManualResolution', 'Incompatible', 'DeletionPending')
                       OR EXISTS (
                         SELECT 1 FROM workflow_deliveries delivery
                         WHERE delivery.workflow_id = binding.workflow_id
                           AND (delivery.status = 'Pending' OR delivery.runtime_acceptance_status = 'Owed')
                       )
                     )
                 )
               )",
        )
        .bind(attempt_id)
        .fetch_one(&mut **tx)
        .await?;
        if active_members != 0 {
            return Err(close_precondition(format!(
                "attempt {attempt_id} still has {active_members} active durable member obligation(s)"
            )));
        }
        Ok(())
    }

    pub async fn advance_close_settlement_when_quiescent(
        &self,
        attempt_id: &str,
    ) -> DbResult<CloseObligation> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let obligation = close_obligation_for_update(&mut tx, attempt_id).await?;
        match obligation.phase() {
            ClosePhase::AwaitingRetirementInspection | ClosePhase::Completed => {
                tx.commit().await?;
                return Ok(obligation);
            }
            ClosePhase::SettlingActiveWork | ClosePhase::CancelRequestedDuringSettlement => {}
            phase @ (ClosePhase::AwaitingBlockerResolution
            | ClosePhase::AwaitingStopWorkConfirmation
            | ClosePhase::AwaitingLossConfirmation
            | ClosePhase::RetirementRequested
            | ClosePhase::NeedsRepair) => {
                return Err(close_precondition(format!(
                    "attempt {attempt_id} phase {} is not settling active work",
                    phase.as_str()
                )));
            }
        }
        Self::reconcile_close_direct_turn_settlement_receipts_tx(&mut tx, attempt_id).await?;
        Self::require_close_participants_quiescent_tx(&mut tx, attempt_id).await?;
        if obligation.phase() == ClosePhase::CancelRequestedDuringSettlement {
            sqlx::query(
                "UPDATE close_obligations
                 SET phase = 'completed', completed_at = ?2, close_outcome = 'cancelled'
                 WHERE attempt_id = ?1",
            )
            .bind(attempt_id)
            .bind(Utc::now().to_rfc3339())
            .execute(&mut *tx)
            .await?;
        } else {
            set_close_phase_tx(
                &mut tx,
                attempt_id,
                ClosePhase::AwaitingRetirementInspection,
            )
            .await?;
        }
        let obligation = close_obligation_for_update(&mut tx, attempt_id).await?;
        tx.commit().await?;
        Ok(obligation)
    }

    pub async fn list_close_settlement_conversation_ids(
        &self,
        attempt_id: &str,
    ) -> DbResult<Vec<String>> {
        let rows = sqlx::query_scalar(
            "SELECT conversation_id
             FROM close_attempt_participants
             WHERE attempt_id = ?1
             ORDER BY conversation_id",
        )
        .bind(attempt_id)
        .fetch_all(&self.pool)
        .await?;
        if rows.is_empty() {
            return Err(DbError::CloseFoundationNotFound(attempt_id.to_string()));
        }
        Ok(rows)
    }

    pub async fn list_close_retirement_archived_conversation_ids(
        &self,
        attempt_id: &str,
    ) -> DbResult<Vec<String>> {
        sqlx::query_scalar(
            "SELECT participant.conversation_id
             FROM close_attempt_participants participant
             JOIN conversations conversation ON conversation.id = participant.conversation_id
             WHERE participant.attempt_id = ?1
             ORDER BY participant.conversation_id",
        )
        .bind(attempt_id)
        .fetch_all(&self.pool)
        .await
        .map_err(Into::into)
    }

    pub async fn wake_delivery_requires_close_settlement_recheck(
        &self,
        workflow_id: phoenix_workflow::WorkflowId,
    ) -> DbResult<bool> {
        let phase: Option<String> = sqlx::query_scalar(
            "SELECT obligation.phase
             FROM wake_bindings binding
             JOIN close_attempt_participants participant
               ON participant.conversation_id = binding.conversation_id
             JOIN close_obligations obligation ON obligation.attempt_id = participant.attempt_id
             WHERE binding.workflow_id = ?1
               AND obligation.phase IN ('settling_active_work', 'cancel_requested_during_settlement')
             ORDER BY obligation.chronology_ordinal DESC
             LIMIT 1",
        )
        .bind(i64::try_from(workflow_id.0).map_err(|_| {
            DbError::Serialization("wake workflow id exceeds SQLite range".to_string())
        })?)
        .fetch_optional(&self.pool)
        .await?;
        Ok(matches!(
            phase.as_deref(),
            Some("settling_active_work" | "cancel_requested_during_settlement")
        ))
    }

    pub async fn suppress_materialized_close_settlement_wakes(
        &self,
        attempt_id: &str,
    ) -> DbResult<usize> {
        let rows: Vec<i64> = sqlx::query_scalar(
            "SELECT DISTINCT binding.workflow_id
             FROM wake_bindings binding
             JOIN close_attempt_participants participant
               ON participant.conversation_id = binding.conversation_id
             JOIN close_obligations obligation ON obligation.attempt_id = participant.attempt_id
             WHERE participant.attempt_id = ?1
               AND obligation.phase IN ('settling_active_work', 'cancel_requested_during_settlement')
               AND EXISTS (
                 SELECT 1 FROM workflow_deliveries delivery
                 JOIN wake_delivery_messages message
                   ON message.workflow_id = delivery.workflow_id
                  AND message.delivery_id = delivery.delivery_id
                 WHERE delivery.workflow_id = binding.workflow_id
                   AND delivery.status = 'Pending'
               )
             ORDER BY binding.workflow_id",
        )
        .bind(attempt_id)
        .fetch_all(&self.pool)
        .await?;
        let wake_repo = crate::workflow::wake::WakeRepository::new(self.pool.clone());
        let mut suppressed = 0;
        for workflow_id in rows {
            let outcome = wake_repo
                .resolve_materialized_pending_for_workflow(
                    phoenix_workflow::WorkflowId(u64::try_from(workflow_id).map_err(|_| {
                        DbError::Serialization("negative wake workflow id".to_string())
                    })?),
                    crate::workflow::wake::WakeResolveMaterializedDecision::Suppress,
                    phoenix_workflow::Timestamp(u64::try_from(Utc::now().timestamp()).map_err(
                        |_| {
                            DbError::Serialization(
                                "negative wake suppression timestamp".to_string(),
                            )
                        },
                    )?),
                )
                .await?;
            match outcome {
                Ok(
                    crate::workflow::wake::WakeResolveMaterializedPendingOutcome::Resolved {
                        ..
                    }
                    | crate::workflow::wake::WakeResolveMaterializedPendingOutcome::AlreadyResolved,
                ) => suppressed += 1,
                Ok(crate::workflow::wake::WakeResolveMaterializedPendingOutcome::NothingPending)
                | Err(
                    crate::workflow::wake::WakeResolveMaterializedPendingError::NotFullyMaterialized {
                        ..
                    },
                ) => {}
            }
        }
        Ok(suppressed)
    }

    pub async fn cancel_close_settlement_wakes(&self, attempt_id: &str) -> DbResult<usize> {
        let wake_repo = crate::workflow::wake::WakeRepository::new(self.pool.clone());
        let rows: Vec<(i64, String, String)> = sqlx::query_as(
            "SELECT binding.workflow_id, binding.conversation_id, binding.contract_id
             FROM wake_bindings binding
             JOIN workflows workflow ON workflow.workflow_id = binding.workflow_id
             JOIN close_attempt_participants participant
               ON participant.conversation_id = binding.conversation_id
             WHERE participant.attempt_id = ?1
               AND binding.resolved_at IS NULL
               AND workflow.status IN ('Active', 'Cancelling', 'ManualResolution', 'Incompatible', 'DeletionPending')
             ORDER BY binding.workflow_id",
        )
        .bind(attempt_id)
        .fetch_all(&self.pool)
        .await?;
        let mut cancelled = 0;
        for (workflow_id, conversation_id, contract_id) in rows {
            let workflow_id =
                phoenix_workflow::WorkflowId(u64::try_from(workflow_id).map_err(|_| {
                    DbError::Serialization("negative wake workflow id".to_string())
                })?);
            match wake_repo
                .cancel_allocated(&crate::workflow::wake::WakeCancelIfUnresolvedInput {
                    workflow_id,
                    expected_conversation_id: Some(conversation_id),
                    expected_contract_id: Some(contract_id),
                    timestamp: phoenix_workflow::Timestamp(
                        u64::try_from(Utc::now().timestamp()).map_err(|_| {
                            DbError::Serialization(
                                "negative wake cancellation timestamp".to_string(),
                            )
                        })?,
                    ),
                    reason: phoenix_workflow::wake_profile::WakeCancellationReason::ExplicitCancel,
                })
                .await?
            {
                crate::workflow::wake::WakeCancellationOutcome::Cancelled { .. }
                | crate::workflow::wake::WakeCancellationOutcome::Replayed { .. } => cancelled += 1,
                crate::workflow::wake::WakeCancellationOutcome::Stale => {}
            }
        }
        Ok(cancelled)
    }

    pub async fn list_unsettled_close_direct_turn_settlement_targets(
        &self,
        attempt_id: &str,
    ) -> DbResult<Vec<CloseDirectTurnSettlementTarget>> {
        let rows: Vec<(String, i64, i64)> = sqlx::query_as(
            "SELECT turn.conversation_id, target.turn_id, target.expected_generation
             FROM close_attempt_direct_turn_settlements target
             JOIN durable_turns turn ON turn.turn_id = target.turn_id
             WHERE target.attempt_id = ?1 AND target.settled_at IS NULL
             ORDER BY target.turn_id",
        )
        .bind(attempt_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|(conversation_id, turn_id, expected_generation)| {
                Ok(CloseDirectTurnSettlementTarget {
                    conversation_id,
                    turn_id: u64::try_from(turn_id).map_err(|_| {
                        DbError::Serialization("negative durable turn id".to_string())
                    })?,
                    expected_generation: u64::try_from(expected_generation).map_err(|_| {
                        DbError::Serialization("negative durable turn generation".to_string())
                    })?,
                })
            })
            .collect()
    }

    pub async fn record_close_direct_turn_settlement_if_released(
        &self,
        attempt_id: &str,
        target: &CloseDirectTurnSettlementTarget,
    ) -> DbResult<bool> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let result = sqlx::query(
            "UPDATE close_attempt_direct_turn_settlements
             SET settled_at = ?4
             WHERE attempt_id = ?1 AND turn_id = ?2 AND expected_generation = ?3
               AND settled_at IS NULL
               AND EXISTS (
                 SELECT 1 FROM durable_turns turn
                 WHERE turn.turn_id = close_attempt_direct_turn_settlements.turn_id
                   AND turn.terminal_kind IS NOT NULL
                   AND turn.owns_conversation = 0
                   AND turn.generation = close_attempt_direct_turn_settlements.expected_generation + 1
               )",
        )
        .bind(attempt_id)
        .bind(i64::try_from(target.turn_id).map_err(|error| {
            DbError::Serialization(format!("turn id overflow: {error}"))
        })?)
        .bind(i64::try_from(target.expected_generation).map_err(|error| {
            DbError::Serialization(format!("turn generation overflow: {error}"))
        })?)
        .bind(Utc::now().to_rfc3339())
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() == 0 {
            let settled: Option<i64> =
                sqlx::query_scalar(
                    "SELECT settled_at IS NOT NULL
                 FROM close_attempt_direct_turn_settlements
                 WHERE attempt_id = ?1 AND turn_id = ?2 AND expected_generation = ?3",
                )
                .bind(attempt_id)
                .bind(i64::try_from(target.turn_id).map_err(|error| {
                    DbError::Serialization(format!("turn id overflow: {error}"))
                })?)
                .bind(i64::try_from(target.expected_generation).map_err(|error| {
                    DbError::Serialization(format!("turn generation overflow: {error}"))
                })?)
                .fetch_optional(&mut *tx)
                .await?;
            tx.commit().await?;
            return match settled {
                Some(1) => Ok(true),
                Some(0) => Ok(false),
                _ => Err(close_precondition(format!(
                    "attempt {attempt_id} has no matching direct-turn settlement target"
                ))),
            };
        }
        tx.commit().await?;
        Ok(true)
    }

    pub async fn get_close_obligation(&self, attempt_id: &str) -> DbResult<CloseObligation> {
        sqlx::query(
            "SELECT attempt_id, product_conversation_id, phase, inspection_generation,
                    inspection_fingerprint, created_at, updated_at, completed_at, close_outcome
             FROM close_obligations WHERE attempt_id = ?1",
        )
        .bind(attempt_id)
        .fetch_optional(&self.pool)
        .await?
        .map(parse_close_obligation_row)
        .transpose()?
        .ok_or_else(|| DbError::CloseFoundationNotFound(attempt_id.to_string()))
    }

    pub async fn get_active_close_obligation_for_product(
        &self,
        product_conversation_id: &ProductConversationId,
    ) -> DbResult<Option<CloseObligation>> {
        sqlx::query(
            "SELECT attempt_id, product_conversation_id, phase, inspection_generation,
                    inspection_fingerprint, created_at, updated_at, completed_at, close_outcome
             FROM close_obligations
             WHERE product_conversation_id = ?1 AND (phase <> 'completed' OR close_outcome = 'close_incomplete')",
        )
        .bind(product_conversation_id.as_str())
        .fetch_optional(&self.pool)
        .await?
        .map(parse_close_obligation_row)
        .transpose()
    }

    pub async fn list_latest_close_obligations(&self) -> DbResult<Vec<CloseObligation>> {
        let rows = sqlx::query(
            "SELECT chronology_ordinal, attempt_id, product_conversation_id, phase,
                    inspection_generation, inspection_fingerprint, created_at, updated_at,
                    completed_at, close_outcome
             FROM close_obligations",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut latest_by_root = std::collections::HashMap::new();
        for row in rows {
            let chronology_ordinal: i64 = row.try_get("chronology_ordinal")?;
            let obligation = parse_close_obligation_row(row)?;
            latest_by_root
                .entry(obligation.product_conversation_id().clone())
                .and_modify(|current: &mut (i64, CloseObligation)| {
                    if chronology_ordinal > current.0 {
                        *current = (chronology_ordinal, obligation.clone());
                    }
                })
                .or_insert((chronology_ordinal, obligation));
        }
        let mut latest: Vec<_> = latest_by_root.into_values().collect();
        latest.sort_by_key(|(chronology_ordinal, _)| std::cmp::Reverse(*chronology_ordinal));
        Ok(latest
            .into_iter()
            .map(|(_, obligation)| obligation)
            .collect())
    }

    pub async fn list_pending_close_obligations(&self) -> DbResult<Vec<CloseObligation>> {
        let rows = sqlx::query(
            "SELECT attempt_id, product_conversation_id, phase, inspection_generation,
                    inspection_fingerprint, created_at, updated_at, completed_at, close_outcome
             FROM close_obligations
             WHERE phase <> 'completed'",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut obligations = rows
            .into_iter()
            .map(parse_close_obligation_row)
            .collect::<DbResult<Vec<_>>>()?;
        obligations.sort_by(|left, right| {
            (right.created_at(), right.attempt_id().as_str())
                .cmp(&(left.created_at(), left.attempt_id().as_str()))
        });
        Ok(obligations)
    }

    pub async fn list_close_attempt_members(
        &self,
        attempt_id: &str,
    ) -> DbResult<Vec<CloseAttemptMember>> {
        sqlx::query(
            "SELECT attempt_id, conversation_id, member_role, continuation_ordinal,
                    captured_continued_in_conv_id, captured_state_kind, captured_runtime_role,
                    captured_work_scope_id, captured_at
             FROM close_attempt_members
             WHERE attempt_id = ?1
             ORDER BY continuation_ordinal",
        )
        .bind(attempt_id)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(parse_close_attempt_member_row)
        .collect()
    }

    pub async fn close_attempt_latest_was_busy(&self, attempt_id: &str) -> DbResult<bool> {
        let captured_state: String = sqlx::query_scalar(
            "SELECT captured_state_kind
             FROM close_attempt_members
             WHERE attempt_id = ?1 AND member_role IN ('latest', 'root_latest')",
        )
        .bind(attempt_id)
        .fetch_one(&self.pool)
        .await?;
        let captured_state = CapturedConversationStateKind::from_db_str(&captured_state)
            .ok_or_else(|| DbError::Serialization("unknown captured state kind".to_string()))?;
        Ok(captured_state.is_busy())
    }

    pub async fn list_close_attempt_scopes(
        &self,
        attempt_id: &str,
    ) -> DbResult<Vec<CloseAttemptScope>> {
        sqlx::query(
            "SELECT attempt_id, scope, captured_worktree_identity,
                    captured_worktree_fingerprint, captured_worktree_locator, captured_at
             FROM close_attempt_scopes
             WHERE attempt_id = ?1
             ORDER BY scope, captured_at",
        )
        .bind(attempt_id)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(parse_close_attempt_scope_row)
        .collect()
    }

    /// Lists every work scope whose unresolved or stopped Close still fences ordinary execution.
    pub async fn list_close_execution_fence_scopes(&self) -> DbResult<Vec<WorkScopeId>> {
        sqlx::query_scalar::<_, String>(
            "SELECT DISTINCT scopes.scope
             FROM close_attempt_scopes scopes
             JOIN close_obligations obligations ON obligations.attempt_id = scopes.attempt_id
             WHERE obligations.phase <> 'completed'
                OR obligations.close_outcome IN ('archived_cleanup_attention', 'close_incomplete')
             ORDER BY scopes.scope",
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(|scope| {
            WorkScopeId::parse(scope).map_err(|error| DbError::Serialization(error.to_string()))
        })
        .collect()
    }

    pub async fn replace_close_inspection(
        &self,
        request: ReplaceCloseInspectionRequest,
    ) -> DbResult<()> {
        self.replace_close_inspection_with_empty_generation(request, None)
            .await
    }

    #[allow(clippy::too_many_lines)]
    pub async fn replace_close_inspection_with_empty_generation(
        &self,
        request: ReplaceCloseInspectionRequest,
        empty_generation: Option<&str>,
    ) -> DbResult<()> {
        let mut ordered_scopes = request.scopes.iter().collect::<Vec<_>>();
        ordered_scopes.sort_by(|left, right| left.scope.cmp(&right.scope));
        let aggregate_generation = if ordered_scopes.is_empty() {
            empty_generation.unwrap_or("no-worktree").to_string()
        } else {
            encode_aggregate_snapshot_component(
                ordered_scopes
                    .iter()
                    .map(|scope| (&scope.scope, scope.snapshot.generation())),
            )
        };
        let aggregate_snapshot = CloseRetirementSnapshot::parse(
            aggregate_generation,
            encode_aggregate_snapshot_component(
                ordered_scopes
                    .iter()
                    .map(|scope| (&scope.scope, scope.snapshot.fingerprint())),
            ),
        )
        .map_err(|error| DbError::Serialization(error.to_string()))?;

        let mut conn = self.pool.acquire().await?;
        let mut tx = conn.begin_with("BEGIN IMMEDIATE").await?;
        let obligation = sqlx::query(
            "SELECT attempt_id, product_conversation_id, phase, inspection_generation,
                    inspection_fingerprint, created_at, updated_at, completed_at, close_outcome
             FROM close_obligations WHERE attempt_id = ?1",
        )
        .bind(request.attempt_id.as_str())
        .fetch_optional(&mut *tx)
        .await?
        .map(parse_close_obligation_row)
        .transpose()?
        .ok_or_else(|| DbError::CloseFoundationNotFound(request.attempt_id.as_str().to_string()))?;
        if obligation.phase() != ClosePhase::AwaitingRetirementInspection {
            let mut persisted_inspections = sqlx::query(
                "SELECT attempt_id, scope, generation, fingerprint, inspected_at
                 FROM close_retirement_inspections
                 WHERE attempt_id = ?1
                 ORDER BY scope, inspected_at",
            )
            .bind(request.attempt_id.as_str())
            .fetch_all(&mut *tx)
            .await?
            .into_iter()
            .map(parse_close_inspection_row)
            .collect::<DbResult<Vec<_>>>()?
            .into_iter()
            .map(|inspection| {
                (
                    inspection.target.scope.as_str().to_string(),
                    inspection.snapshot.generation().to_string(),
                    inspection.snapshot.fingerprint().to_string(),
                )
            })
            .collect::<Vec<_>>();
            persisted_inspections.sort();
            let mut requested_inspections = request
                .scopes
                .iter()
                .map(|scope| {
                    (
                        scope.scope.as_str().to_string(),
                        scope.snapshot.generation().to_string(),
                        scope.snapshot.fingerprint().to_string(),
                    )
                })
                .collect::<Vec<_>>();
            requested_inspections.sort();
            let mut persisted_losses = sqlx::query(
                "SELECT loss.attempt_id, loss.scope, loss.generation, inspection.fingerprint,
                        loss.category, loss.identity_kind, loss.identity_codec, loss.identity_value
                 FROM close_retirement_losses loss
                 JOIN close_retirement_inspections inspection
                   ON inspection.attempt_id = loss.attempt_id
                  AND inspection.scope = loss.scope
                  AND inspection.generation = loss.generation
                 WHERE loss.attempt_id = ?1
                 ORDER BY loss.scope, loss.generation, loss.category, loss.identity_kind,
                          loss.identity_value",
            )
            .bind(request.attempt_id.as_str())
            .fetch_all(&mut *tx)
            .await?
            .into_iter()
            .map(parse_close_inspection_loss_row)
            .collect::<DbResult<Vec<_>>>()?
            .into_iter()
            .map(|loss| {
                (
                    loss.scope.as_str().to_string(),
                    loss.snapshot.generation().to_string(),
                    loss.item.category().as_str().to_string(),
                    loss.item.identity().identity_kind().to_string(),
                    loss.item.identity().value(),
                )
            })
            .collect::<Vec<_>>();
            persisted_losses.sort();
            let mut requested_losses = request
                .scopes
                .iter()
                .flat_map(|scope| {
                    scope.losses.iter().map(|loss| {
                        (
                            scope.scope.as_str().to_string(),
                            scope.snapshot.generation().to_string(),
                            loss.category().as_str().to_string(),
                            loss.identity().identity_kind().to_string(),
                            loss.identity().value(),
                        )
                    })
                })
                .collect::<Vec<_>>();
            requested_losses.sort();
            if obligation.snapshot() == Some(&aggregate_snapshot)
                && persisted_inspections == requested_inspections
                && persisted_losses == requested_losses
            {
                tx.commit().await?;
                return Ok(());
            }
            if obligation.phase() != ClosePhase::AwaitingLossConfirmation {
                return Err(close_precondition(format!(
                    "attempt {} inspection replacement replay differs from persisted inspection",
                    request.attempt_id
                )));
            }
        }

        if obligation.phase() == ClosePhase::AwaitingLossConfirmation {
            set_close_phase_tx(
                &mut tx,
                request.attempt_id.as_str(),
                ClosePhase::AwaitingRetirementInspection,
            )
            .await?;
        }
        self.ensure_inspection_replacement_allowed(&mut tx, &request)
            .await?;
        self.clear_retirement_inspection_rows(&mut tx, request.attempt_id.as_str())
            .await?;
        self.insert_retirement_inspection_rows(&mut tx, &request)
            .await?;
        self.advance_obligation_after_inspection(&mut tx, &request, &aggregate_snapshot)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Confirms the exact loss snapshot before admitting resource retirement.
    ///
    /// # Errors
    /// Returns [`DbError`] when the attempt is not awaiting loss confirmation,
    /// the supplied snapshot is stale, or no persisted loss remains to confirm.
    pub async fn confirm_close_loss_retirement(
        &self,
        attempt_id: &CloseAttemptId,
        snapshot: &CloseRetirementSnapshot,
    ) -> DbResult<CloseObligation> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let obligation = close_obligation_for_update(&mut tx, attempt_id.as_str()).await?;
        if obligation.phase() != ClosePhase::AwaitingLossConfirmation {
            return Err(close_precondition(format!(
                "attempt {attempt_id} is not awaiting loss confirmation"
            )));
        }
        if obligation.snapshot() != Some(snapshot) {
            return Err(close_precondition(format!(
                "attempt {attempt_id} loss confirmation snapshot is stale"
            )));
        }
        let has_loss: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                 SELECT 1 FROM close_retirement_losses loss
                 JOIN close_retirement_inspections inspection
                   ON inspection.attempt_id = loss.attempt_id
                  AND inspection.scope = loss.scope
                  AND inspection.generation = loss.generation
                 WHERE loss.attempt_id = ?1
               )",
        )
        .bind(attempt_id.as_str())
        .fetch_one(&mut *tx)
        .await?;
        if !has_loss {
            return Err(close_precondition(format!(
                "attempt {attempt_id} has no exact loss evidence to confirm"
            )));
        }
        set_close_phase_tx(
            &mut tx,
            attempt_id.as_str(),
            ClosePhase::RetirementRequested,
        )
        .await?;
        let confirmed = close_obligation_for_update(&mut tx, attempt_id.as_str()).await?;
        tx.commit().await?;
        Ok(confirmed)
    }

    /// Reads the current Close authority, including a stopped outcome, from one `SQLite` snapshot.
    /// # Errors
    /// Returns [`DbError`] when the obligation or normalized evidence cannot be read.
    pub async fn get_active_close_projection_for_product(
        &self,
        product_conversation_id: &ProductConversationId,
    ) -> DbResult<Option<CloseProjection>> {
        let mut connection = self.pool.acquire().await?;
        let mut tx = connection.begin().await?;
        let projection =
            Self::get_active_close_projection_for_product_on(&mut tx, product_conversation_id)
                .await?;
        tx.rollback().await?;
        Ok(projection)
    }

    pub(crate) async fn get_active_close_projection_for_product_on(
        connection: &mut SqliteConnection,
        product_conversation_id: &ProductConversationId,
    ) -> DbResult<Option<CloseProjection>> {
        let obligation = sqlx::query(
            "SELECT attempt_id, product_conversation_id, phase,
                    inspection_generation, inspection_fingerprint,
                    created_at, updated_at, completed_at, close_outcome
             FROM close_obligations
             WHERE product_conversation_id = ?1
               AND (phase <> 'completed' OR close_outcome IN ('close_incomplete', 'archived_cleanup_attention'))
             ORDER BY chronology_ordinal DESC LIMIT 1",
        )
        .bind(product_conversation_id.as_str())
        .fetch_optional(&mut *connection)
        .await?
        .map(parse_close_obligation_row)
        .transpose()?;
        let Some(obligation) = obligation else {
            return Ok(None);
        };
        let inspections = sqlx::query(
            "SELECT attempt_id, scope, generation, fingerprint, inspected_at
             FROM close_retirement_inspections
             WHERE attempt_id = ?1
             ORDER BY scope, inspected_at",
        )
        .bind(obligation.attempt_id().as_str())
        .fetch_all(&mut *connection)
        .await?
        .into_iter()
        .map(parse_close_inspection_row)
        .collect::<DbResult<Vec<_>>>()?;
        let losses = sqlx::query(
            "SELECT loss.attempt_id, loss.scope, loss.generation, inspection.fingerprint,
                    loss.category, loss.identity_kind, loss.identity_codec, loss.identity_value
             FROM close_retirement_losses loss
             JOIN close_retirement_inspections inspection
               ON inspection.attempt_id = loss.attempt_id
              AND inspection.scope = loss.scope
              AND inspection.generation = loss.generation
             WHERE loss.attempt_id = ?1
             ORDER BY loss.scope, loss.generation, loss.category, loss.identity_kind, loss.identity_value",
        )
        .bind(obligation.attempt_id().as_str())
        .fetch_all(&mut *connection)
        .await?
        .into_iter()
        .map(parse_close_inspection_loss_row)
        .collect::<DbResult<Vec<_>>>()?;
        let residuals = sqlx::query(
            "SELECT resource.attempt_id, resource.scope, resource.inspection_generation,
                    resource.inspection_fingerprint, resource.resource_kind, resource.identity_kind,
                    resource.identity_codec, resource.identity_value, resource.proof_kind,
                    resource.absence_basis, resource.residual_reason, resource.detail,
                    resource.created_at, resource.updated_at,
                    captured.captured_worktree_fingerprint, captured.captured_worktree_locator
             FROM close_retirement_resources resource
             JOIN close_attempt_scopes captured
               ON captured.attempt_id = resource.attempt_id AND captured.scope = resource.scope
             WHERE resource.attempt_id = ?1
               AND resource.proof_kind = 'residual'
               AND resource.inspection_generation = ?2
               AND resource.inspection_fingerprint = ?3
             ORDER BY resource.scope, resource.resource_kind, resource.identity_value",
        )
        .bind(obligation.attempt_id().as_str())
        .bind(
            obligation
                .snapshot()
                .map(CloseRetirementSnapshot::generation),
        )
        .bind(
            obligation
                .snapshot()
                .map(CloseRetirementSnapshot::fingerprint),
        )
        .fetch_all(&mut *connection)
        .await?
        .into_iter()
        .map(parse_close_retired_resource_row)
        .collect::<DbResult<Vec<_>>>()?;
        let latest_run = sqlx::query(
            "SELECT attempt_id, run_ordinal, status FROM close_runs
             WHERE attempt_id = ?1 ORDER BY run_ordinal DESC LIMIT 1",
        )
        .bind(obligation.attempt_id().as_str())
        .fetch_one(&mut *connection)
        .await?;
        let latest_run = parse_close_run_row(latest_run)?;
        Ok(Some(CloseProjection {
            obligation,
            latest_run,
            inspections,
            losses,
            residuals,
        }))
    }

    pub async fn list_close_retirement_inspections(
        &self,
        attempt_id: &str,
    ) -> DbResult<Vec<CloseInspection>> {
        sqlx::query(
            "SELECT attempt_id, scope, generation, fingerprint, inspected_at
             FROM close_retirement_inspections
             WHERE attempt_id = ?1
             ORDER BY scope, inspected_at",
        )
        .bind(attempt_id)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(parse_close_inspection_row)
        .collect()
    }

    pub async fn list_close_retirement_losses(
        &self,
        attempt_id: &str,
    ) -> DbResult<Vec<CloseInspectionLoss>> {
        sqlx::query(
            "SELECT loss.attempt_id, loss.scope, loss.generation, inspection.fingerprint,
                    loss.category, loss.identity_kind, loss.identity_codec, loss.identity_value
             FROM close_retirement_losses loss
             JOIN close_retirement_inspections inspection
               ON inspection.attempt_id = loss.attempt_id
              AND inspection.scope = loss.scope
              AND inspection.generation = loss.generation
             WHERE loss.attempt_id = ?1
             ORDER BY loss.scope, loss.generation, loss.category, loss.identity_kind, loss.identity_value",
        )
        .bind(attempt_id)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(parse_close_inspection_loss_row)
        .collect()
    }

    /// Captures the immutable exact-snapshot inventory that retirement must prove.
    ///
    /// # Errors
    /// Returns [`DbError`] when the attempt/snapshot/scope set is not current or persistence fails.
    #[allow(clippy::too_many_lines)]
    pub async fn capture_close_retirement_inventory(
        &self,
        request: CaptureCloseRetirementInventoryRequest,
    ) -> DbResult<Vec<CloseExpectedRetirementResource>> {
        let mut conn = self.pool.acquire().await?;
        let mut tx = conn.begin_with("BEGIN IMMEDIATE").await?;
        let attempt_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM close_obligations WHERE attempt_id = ?1)",
        )
        .bind(request.attempt_id.as_str())
        .fetch_one(&mut *tx)
        .await?;
        if !attempt_exists {
            return Err(DbError::CloseFoundationNotFound(
                request.attempt_id.as_str().to_string(),
            ));
        }
        let authority_matches: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                 SELECT 1 FROM close_obligations
                 WHERE attempt_id = ?1
                   AND phase IN ('retirement_requested', 'needs_repair')
                   AND inspection_generation = ?2
                   AND inspection_fingerprint = ?3
             )",
        )
        .bind(request.attempt_id.as_str())
        .bind(request.snapshot.generation())
        .bind(request.snapshot.fingerprint())
        .fetch_one(&mut *tx)
        .await?;
        if !authority_matches {
            return Err(close_precondition(format!(
                "attempt {} retirement inventory requires its exact authorized snapshot",
                request.attempt_id.as_str()
            )));
        }
        let target_scopes = sqlx::query_scalar::<_, String>(
            "SELECT scope FROM close_attempt_scopes WHERE attempt_id = ?1 ORDER BY scope",
        )
        .bind(request.attempt_id.as_str())
        .fetch_all(&mut *tx)
        .await?
        .into_iter()
        .map(|scope| {
            WorkScopeId::parse(scope).map_err(|error| DbError::Serialization(error.to_string()))
        })
        .collect::<DbResult<std::collections::BTreeSet<_>>>()?;
        let provided_scopes = request
            .scopes
            .iter()
            .map(|scope| scope.scope.clone())
            .collect::<std::collections::BTreeSet<_>>();
        if request.scopes.len() != provided_scopes.len() || target_scopes != provided_scopes {
            return Err(close_precondition(format!(
                "attempt {} retirement inventory must cover every captured scope exactly once",
                request.attempt_id.as_str()
            )));
        }

        for scope in &request.scopes {
            let captured_worktree =
                sqlx::query_as::<_, (Option<String>, Option<String>, Option<String>)>(
                    "SELECT captured_worktree_identity, captured_worktree_fingerprint,
                        captured_worktree_locator
                 FROM close_attempt_scopes
                 WHERE attempt_id = ?1 AND scope = ?2",
                )
                .bind(request.attempt_id.as_str())
                .bind(scope.scope.as_str())
                .fetch_one(&mut *tx)
                .await?;
            let captured_worktree = match captured_worktree {
                (Some(id), Some(fingerprint), Some(locator)) => Some(WorktreeIdentity::from_parts(
                    phoenix_core::domain::close::WorktreeId::parse(id)
                        .map_err(|error| DbError::Serialization(error.to_string()))?,
                    phoenix_core::domain::close::WorktreeFingerprint::parse(fingerprint)
                        .map_err(|error| DbError::Serialization(error.to_string()))?,
                    GitPathIdentity::decode_exact(&locator)
                        .map_err(|error| DbError::Serialization(error.to_string()))?,
                )),
                (None, None, Some(locator)) => {
                    let repair = CloseFoundationRepair::UnresolvedWorktreeIdentity {
                        attempt_id: request.attempt_id.clone(),
                        scope: scope.scope.clone(),
                        locator: GitPathIdentity::decode_exact(&locator)
                            .map_err(|error| DbError::Serialization(error.to_string()))?,
                    };
                    route_close_attempt_to_repair_tx(
                        &mut tx,
                        &RouteCloseAttemptToRepairRequest {
                            attempt_id: request.attempt_id.clone(),
                            scope: scope.scope.clone(),
                            residual: RetiredResourceIdentity::parse(
                                RetiredResourceKind::WorkScope,
                                LossItemIdentity::Opaque(
                                    OpaqueIdentity::parse(scope.scope.as_str().to_owned())
                                        .map_err(|error| {
                                            DbError::Serialization(error.to_string())
                                        })?,
                                ),
                            )
                            .map_err(|error| DbError::Serialization(error.to_string()))?,
                            reason: RetirementFailureReason::IdentityNotProven,
                            detail: format!(
                                "scope {} has unresolved captured worktree identity",
                                scope.scope
                            ),
                        },
                    )
                    .await?;
                    tx.commit().await?;
                    return Err(DbError::CloseFoundationRepairRequired(repair));
                }
                (None, None, None) => None,
                _ => {
                    return Err(DbError::Serialization(
                        "partial worktree identity".to_string(),
                    ))
                }
            };
            if scope.inventory.worktree != captured_worktree {
                return Err(close_precondition(format!(
                    "attempt {} scope {} inventory worktree must equal its captured scope snapshot",
                    request.attempt_id.as_str(),
                    scope.scope.as_str()
                )));
            }
        }

        let mut requested_resources = Vec::new();
        for scope in &request.scopes {
            let mut unique = std::collections::BTreeSet::new();
            for resource in scope.inventory.resources() {
                if resource.kind() == RetiredResourceKind::WorkScope {
                    return Err(close_precondition(format!(
                        "scope {} inventory cannot supply a WorkScope resource",
                        scope.scope
                    )));
                }
                let identity = resource.identity();
                let resource_key = (
                    resource.kind().as_str().to_string(),
                    identity.identity_kind().to_string(),
                    identity.codec().to_string(),
                    identity.value(),
                );
                if !unique.insert(resource_key.clone()) {
                    return Err(close_precondition(format!(
                        "scope {} retirement inventory contains duplicate resource identity",
                        scope.scope
                    )));
                }
                requested_resources.push((
                    scope.scope.as_str().to_string(),
                    resource_key.0,
                    resource_key.1,
                    resource_key.2,
                    resource_key.3,
                ));
            }
            let scope_identity = OpaqueIdentity::parse(scope.scope.as_str().to_owned())
                .map_err(|error| DbError::Serialization(error.to_string()))?;
            requested_resources.push((
                scope.scope.as_str().to_string(),
                RetiredResourceKind::WorkScope.as_str().to_string(),
                "opaque".to_string(),
                scope_identity.codec().to_string(),
                scope_identity.as_str().to_string(),
            ));
        }
        requested_resources.sort();
        let existing_inventories: Vec<(String, String, String, i64)> = sqlx::query_as(
            "SELECT scope, inspection_generation, inspection_fingerprint, sealed
             FROM close_retirement_inventories
             WHERE attempt_id = ?1
               AND inspection_generation = ?2 AND inspection_fingerprint = ?3
             ORDER BY scope",
        )
        .bind(request.attempt_id.as_str())
        .bind(request.snapshot.generation())
        .bind(request.snapshot.fingerprint())
        .fetch_all(&mut *tx)
        .await?;
        if !existing_inventories.is_empty() {
            let inventory_matches = existing_inventories.len() == target_scopes.len()
                && existing_inventories
                    .iter()
                    .all(|(scope, generation, fingerprint, sealed)| {
                        target_scopes.iter().any(|target| target.as_str() == scope)
                            && generation == request.snapshot.generation()
                            && fingerprint == request.snapshot.fingerprint()
                            && *sealed == 1
                    });
            let persisted_resources: Vec<(String, String, String, String, String)> =
                sqlx::query_as(
                    "SELECT scope, resource_kind, identity_kind, identity_codec, identity_value
                 FROM close_expected_retirement_resources
                 WHERE attempt_id = ?1
                   AND inspection_generation = ?2 AND inspection_fingerprint = ?3
                 ORDER BY scope, resource_kind, identity_kind,
                     identity_codec, identity_value",
                )
                .bind(request.attempt_id.as_str())
                .bind(request.snapshot.generation())
                .bind(request.snapshot.fingerprint())
                .fetch_all(&mut *tx)
                .await?;
            if !inventory_matches || persisted_resources != requested_resources {
                return Err(close_precondition(format!(
                    "attempt {} retirement inventory replay differs from sealed inventory",
                    request.attempt_id
                )));
            }
            let resources =
                list_close_expected_retirement_resources_tx(&mut tx, request.attempt_id.as_str())
                    .await?;
            tx.commit().await?;
            return Ok(resources);
        }

        let product_conversation_id: String = sqlx::query_scalar(
            "SELECT product_conversation_id FROM close_obligations WHERE attempt_id = ?1",
        )
        .bind(request.attempt_id.as_str())
        .fetch_one(&mut *tx)
        .await?;
        for target_scope in &target_scopes {
            let conflicting_owner: Option<String> = sqlx::query_scalar(
                "SELECT candidate.product_conversation_id
                 FROM conversations candidate
                 WHERE candidate.work_scope_id = ?1
                   AND candidate.runtime_role = 'user'
                   AND candidate.parent_conversation_id IS NULL
                   AND candidate.archived = 0
                   AND candidate.product_conversation_id <> ?2
                 LIMIT 1",
            )
            .bind(target_scope.as_str())
            .bind(&product_conversation_id)
            .fetch_optional(&mut *tx)
            .await?;
            if let Some(owner) = conflicting_owner {
                return Err(close_precondition(format!(
                    "scope {target_scope} is retained by distinct open aggregate {owner}"
                )));
            }
        }

        let now = Utc::now().to_rfc3339();
        for scope in &request.scopes {
            let environment =
                sqlx::query_as::<_, (Option<String>, Option<String>, Option<String>)>(
                    "SELECT worktree_id, worktree_fingerprint,
                        CASE WHEN environment_kind = 'allocated_worktree' THEN
                            'git_path_bytes_hex_v1:' || lower(hex(CAST(worktree_path AS BLOB)))
                        END
                 FROM work_scopes WHERE id = ?1",
                )
                .bind(scope.scope.as_str())
                .fetch_one(&mut *tx)
                .await?;
            let environment = match environment {
                (Some(id), Some(fingerprint), Some(locator)) => Some(WorktreeIdentity::from_parts(
                    phoenix_core::domain::close::WorktreeId::parse(id)
                        .map_err(|error| DbError::Serialization(error.to_string()))?,
                    phoenix_core::domain::close::WorktreeFingerprint::parse(fingerprint)
                        .map_err(|error| DbError::Serialization(error.to_string()))?,
                    GitPathIdentity::decode_exact(&locator)
                        .map_err(|error| DbError::Serialization(error.to_string()))?,
                )),
                (None, None, Some(locator)) => {
                    let repair = CloseFoundationRepair::UnresolvedWorktreeIdentity {
                        attempt_id: request.attempt_id.clone(),
                        scope: scope.scope.clone(),
                        locator: GitPathIdentity::decode_exact(&locator)
                            .map_err(|error| DbError::Serialization(error.to_string()))?,
                    };
                    route_close_attempt_to_repair_tx(
                        &mut tx,
                        &RouteCloseAttemptToRepairRequest {
                            attempt_id: request.attempt_id.clone(),
                            scope: scope.scope.clone(),
                            residual: RetiredResourceIdentity::parse(
                                RetiredResourceKind::WorkScope,
                                LossItemIdentity::Opaque(
                                    OpaqueIdentity::parse(scope.scope.as_str().to_owned())
                                        .map_err(|error| {
                                            DbError::Serialization(error.to_string())
                                        })?,
                                ),
                            )
                            .map_err(|error| DbError::Serialization(error.to_string()))?,
                            reason: RetirementFailureReason::IdentityNotProven,
                            detail: format!(
                                "scope {} has unresolved captured worktree identity",
                                scope.scope
                            ),
                        },
                    )
                    .await?;
                    tx.commit().await?;
                    return Err(DbError::CloseFoundationRepairRequired(repair));
                }
                (None, None, None) => None,
                _ => {
                    return Err(DbError::Serialization(
                        "partial worktree identity".to_string(),
                    ))
                }
            };
            if scope.inventory.worktree != environment {
                return Err(close_precondition(format!(
                    "scope {} expected worktree must match its stable allocated identity",
                    scope.scope
                )));
            }
            sqlx::query(
                "INSERT INTO close_retirement_inventories (
                     attempt_id, scope, inspection_generation, inspection_fingerprint, captured_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5)",
            )
            .bind(request.attempt_id.as_str())
            .bind(scope.scope.as_str())
            .bind(request.snapshot.generation())
            .bind(request.snapshot.fingerprint())
            .bind(&now)
            .execute(&mut *tx)
            .await?;
            let resources = scope.inventory.resources();
            let mut unique = std::collections::BTreeSet::new();
            for resource in &resources {
                let kind = resource.kind();
                let identity = resource.identity();
                let identity_kind = identity.identity_kind();
                let codec = identity.codec();
                let value = identity.value();
                if !unique.insert((kind.as_str(), identity_kind, codec, value.clone())) {
                    return Err(close_precondition(format!(
                        "scope {} retirement inventory contains duplicate resource identity",
                        scope.scope
                    )));
                }
                sqlx::query(
                    "INSERT INTO close_expected_retirement_resources (
                         attempt_id, scope, inspection_generation, inspection_fingerprint,
                         resource_kind, identity_kind, identity_codec, identity_value
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                )
                .bind(request.attempt_id.as_str())
                .bind(scope.scope.as_str())
                .bind(request.snapshot.generation())
                .bind(request.snapshot.fingerprint())
                .bind(kind.as_str())
                .bind(identity_kind)
                .bind(codec)
                .bind(value)
                .execute(&mut *tx)
                .await?;
            }
            let scope_identity = OpaqueIdentity::parse(scope.scope.as_str().to_owned())
                .map_err(|error| DbError::Serialization(error.to_string()))?;
            sqlx::query(
                "INSERT INTO close_expected_retirement_resources (
                     attempt_id, scope, inspection_generation, inspection_fingerprint,
                     resource_kind, identity_kind, identity_codec, identity_value
                 ) VALUES (?1, ?2, ?3, ?4, ?5, 'opaque', ?6, ?7)",
            )
            .bind(request.attempt_id.as_str())
            .bind(scope.scope.as_str())
            .bind(request.snapshot.generation())
            .bind(request.snapshot.fingerprint())
            .bind(RetiredResourceKind::WorkScope.as_str())
            .bind(scope_identity.codec())
            .bind(scope_identity.as_str())
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "UPDATE close_retirement_inventories SET sealed = 1
                 WHERE attempt_id = ?1 AND scope = ?2
                   AND inspection_generation = ?3 AND inspection_fingerprint = ?4
                   AND sealed = 0",
            )
            .bind(request.attempt_id.as_str())
            .bind(scope.scope.as_str())
            .bind(request.snapshot.generation())
            .bind(request.snapshot.fingerprint())
            .execute(&mut *tx)
            .await?;
        }
        let resources =
            list_close_expected_retirement_resources_tx(&mut tx, request.attempt_id.as_str())
                .await?;
        tx.commit().await?;
        Ok(resources)
    }

    /// Atomically records the exact scope-level residual and routes its Close attempt to repair.
    ///
    /// # Errors
    /// Returns [`DbError`] when the attempt is not in a retirement phase or persistence fails.
    pub async fn route_close_attempt_to_repair(
        &self,
        request: RouteCloseAttemptToRepairRequest,
    ) -> DbResult<()> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        route_close_attempt_to_repair_tx(&mut tx, &request).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Returns a pre-quarantine worktree snapshot mismatch to fresh inspection.
    ///
    /// # Errors
    /// Returns [`DbError`] unless the attempt still has retirement authority.
    pub async fn return_close_attempt_to_reinspection(
        &self,
        attempt_id: &CloseAttemptId,
    ) -> DbResult<()> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let obligation = close_obligation_for_update(&mut tx, attempt_id.as_str()).await?;
        if obligation.phase() != ClosePhase::RetirementRequested {
            return Err(close_precondition(format!(
                "attempt {attempt_id} reinspection requires retirement_requested"
            )));
        }
        set_close_phase_tx(
            &mut tx,
            attempt_id.as_str(),
            ClosePhase::AwaitingRetirementInspection,
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Advances retained same-attempt dispatch authority after retry observes the
    /// already-dispatched worktree absent.
    #[allow(clippy::too_many_lines)]
    pub async fn resume_close_retirement_after_dispatched_absence(
        &self,
        attempt_id: &CloseAttemptId,
        retained_snapshot: &CloseRetirementSnapshot,
        replacement_generation: &str,
    ) -> DbResult<CloseRetirementSnapshot> {
        let inspections = self
            .list_close_retirement_inspections(attempt_id.as_str())
            .await?;
        let losses = self
            .list_close_retirement_losses(attempt_id.as_str())
            .await?;
        let scopes = inspections
            .into_iter()
            .map(|inspection| {
                let snapshot = CloseRetirementSnapshot::parse(
                    replacement_generation,
                    inspection.snapshot.fingerprint().to_string(),
                )
                .map_err(|error| DbError::Serialization(error.to_string()))?;
                let scope = inspection.target.scope;
                let scoped_losses = losses
                    .iter()
                    .filter(|loss| loss.scope == scope)
                    .map(|loss| loss.item.clone())
                    .collect();
                Ok(ReplaceCloseInspectionScopeRequest {
                    scope,
                    snapshot,
                    losses: scoped_losses,
                })
            })
            .collect::<DbResult<Vec<_>>>()?;
        let mut ordered_scopes = scopes.iter().collect::<Vec<_>>();
        ordered_scopes.sort_by(|left, right| left.scope.cmp(&right.scope));
        let replacement_snapshot = CloseRetirementSnapshot::parse(
            encode_aggregate_snapshot_component(
                ordered_scopes
                    .iter()
                    .map(|scope| (&scope.scope, scope.snapshot.generation())),
            ),
            encode_aggregate_snapshot_component(
                ordered_scopes
                    .iter()
                    .map(|scope| (&scope.scope, scope.snapshot.fingerprint())),
            ),
        )
        .map_err(|error| DbError::Serialization(error.to_string()))?;

        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let obligation = close_obligation_for_update(&mut tx, attempt_id.as_str()).await?;
        if obligation.phase() != ClosePhase::AwaitingRetirementInspection
            || obligation.snapshot() != Some(retained_snapshot)
        {
            return Err(close_precondition(format!(
                "attempt {attempt_id} dispatched absence requires retained retry inspection authority"
            )));
        }
        let request = ReplaceCloseInspectionRequest {
            attempt_id: attempt_id.clone(),
            scopes,
        };
        self.clear_retirement_inspection_rows(&mut tx, attempt_id.as_str())
            .await?;
        self.insert_retirement_inspection_rows(&mut tx, &request)
            .await?;
        sqlx::query(
            "UPDATE close_obligations
             SET phase = 'retirement_requested',
                 inspection_generation = ?2,
                 inspection_fingerprint = ?3,
                 updated_at = ?4
             WHERE attempt_id = ?1 AND phase = 'awaiting_retirement_inspection'",
        )
        .bind(attempt_id.as_str())
        .bind(replacement_snapshot.generation())
        .bind(replacement_snapshot.fingerprint())
        .bind(Utc::now().to_rfc3339())
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO close_retirement_inventories (
                 attempt_id, scope, inspection_generation, inspection_fingerprint,
                 sealed, captured_at
             )
             SELECT attempt_id, scope, ?2, ?3, 0, captured_at
             FROM close_retirement_inventories
             WHERE attempt_id = ?1
               AND inspection_generation = ?4
               AND inspection_fingerprint = ?3",
        )
        .bind(attempt_id.as_str())
        .bind(replacement_snapshot.generation())
        .bind(replacement_snapshot.fingerprint())
        .bind(retained_snapshot.generation())
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO close_expected_retirement_resources (
                 attempt_id, scope, inspection_generation, inspection_fingerprint,
                 resource_kind, identity_kind, identity_codec, identity_value
             )
             SELECT attempt_id, scope, ?2, ?3, resource_kind, identity_kind,
                    identity_codec, identity_value
             FROM close_expected_retirement_resources
             WHERE attempt_id = ?1
               AND inspection_generation = ?4
               AND inspection_fingerprint = ?3",
        )
        .bind(attempt_id.as_str())
        .bind(replacement_snapshot.generation())
        .bind(replacement_snapshot.fingerprint())
        .bind(retained_snapshot.generation())
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE close_retirement_inventories SET sealed = 1
             WHERE attempt_id = ?1
               AND inspection_generation = ?2
               AND inspection_fingerprint = ?3",
        )
        .bind(attempt_id.as_str())
        .bind(replacement_snapshot.generation())
        .bind(replacement_snapshot.fingerprint())
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO close_retirement_resource_dispatches (
                 attempt_id, scope, inspection_generation, inspection_fingerprint,
                 resource_kind, identity_kind, identity_codec, identity_value, dispatched_at_us
             )
             SELECT dispatch.attempt_id, dispatch.scope, ?3, ?4,
                    dispatch.resource_kind, dispatch.identity_kind, dispatch.identity_codec,
                    dispatch.identity_value, dispatch.dispatched_at_us
             FROM close_retirement_resource_dispatches dispatch
             JOIN close_expected_retirement_resources expected
               ON expected.attempt_id = dispatch.attempt_id
              AND expected.scope = dispatch.scope
              AND expected.inspection_generation = ?3
              AND expected.inspection_fingerprint = ?4
              AND expected.resource_kind = dispatch.resource_kind
              AND expected.identity_kind = dispatch.identity_kind
              AND expected.identity_codec = dispatch.identity_codec
              AND expected.identity_value = dispatch.identity_value
             WHERE dispatch.attempt_id = ?1
               AND dispatch.inspection_generation = ?2
               AND dispatch.inspection_fingerprint = ?4",
        )
        .bind(attempt_id.as_str())
        .bind(retained_snapshot.generation())
        .bind(replacement_snapshot.generation())
        .bind(replacement_snapshot.fingerprint())
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO close_worktree_cleanup_plans (
                 attempt_id, scope, inspection_generation, inspection_fingerprint,
                 resource_kind, identity_kind, identity_codec, identity_value,
                 administrative_dir_codec, administrative_dir_value,
                 administrative_dir_incarnation, planned_at_us
             )
             SELECT plan.attempt_id, plan.scope, ?3, ?4, plan.resource_kind,
                    plan.identity_kind, plan.identity_codec, plan.identity_value,
                    plan.administrative_dir_codec, plan.administrative_dir_value,
                    plan.administrative_dir_incarnation, plan.planned_at_us
             FROM close_worktree_cleanup_plans plan
             WHERE plan.attempt_id = ?1
               AND plan.inspection_generation = ?2
               AND plan.inspection_fingerprint = ?4",
        )
        .bind(attempt_id.as_str())
        .bind(retained_snapshot.generation())
        .bind(replacement_snapshot.generation())
        .bind(replacement_snapshot.fingerprint())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(replacement_snapshot)
    }

    /// Reports whether every captured scope has a sealed inventory for the active snapshot.
    pub async fn close_retirement_inventory_is_complete(&self, attempt_id: &str) -> DbResult<bool> {
        let status = sqlx::query(
            "SELECT
                 (SELECT COUNT(*) FROM close_attempt_scopes WHERE attempt_id = ?1) AS target_count,
                 (SELECT COUNT(*) FROM close_retirement_inventories inventory
                  JOIN close_obligations obligation ON obligation.attempt_id = inventory.attempt_id
                  WHERE inventory.attempt_id = ?1 AND inventory.sealed = 1
                    AND inventory.inspection_generation = obligation.inspection_generation
                    AND inventory.inspection_fingerprint = obligation.inspection_fingerprint) AS sealed_count
             WHERE EXISTS (SELECT 1 FROM close_obligations WHERE attempt_id = ?1)",
        )
        .bind(attempt_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| DbError::CloseFoundationNotFound(attempt_id.to_string()))?;
        Ok(
            status.try_get::<i64, _>("target_count")?
                == status.try_get::<i64, _>("sealed_count")?,
        )
    }

    /// Lists expected retirement resources for the current exact attempt snapshot.
    ///
    /// # Errors
    /// Returns [`DbError`] when persistence or decoding fails.
    pub async fn list_close_expected_retirement_resources(
        &self,
        attempt_id: &str,
    ) -> DbResult<Vec<CloseExpectedRetirementResource>> {
        let mut tx = self.pool.begin().await?;
        let resources = list_close_expected_retirement_resources_tx(&mut tx, attempt_id).await?;
        tx.commit().await?;
        Ok(resources)
    }
}

#[allow(clippy::too_many_lines)]
async fn route_close_attempt_to_repair_tx(
    tx: &mut Transaction<'_, Sqlite>,
    request: &RouteCloseAttemptToRepairRequest,
) -> DbResult<()> {
    let obligation = close_obligation_for_update(tx, request.attempt_id.as_str()).await?;
    if !matches!(
        obligation.phase(),
        ClosePhase::AwaitingRetirementInspection
            | ClosePhase::RetirementRequested
            | ClosePhase::NeedsRepair
    ) {
        return Err(close_precondition(format!(
            "attempt {} repair requires awaiting_retirement_inspection, retirement_requested, or needs_repair",
            request.attempt_id
        )));
    }
    let captured_scope: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM close_attempt_scopes WHERE attempt_id = ?1 AND scope = ?2)",
    )
    .bind(request.attempt_id.as_str())
    .bind(request.scope.as_str())
    .fetch_one(&mut **tx)
    .await?;
    if !captured_scope {
        return Err(close_precondition(format!(
            "attempt {} repair scope {} was not captured",
            request.attempt_id, request.scope
        )));
    }

    let generation = obligation
        .snapshot()
        .map_or("no-worktree", CloseRetirementSnapshot::generation);
    let fingerprint = obligation
        .snapshot()
        .map_or("no-worktree", CloseRetirementSnapshot::fingerprint);
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT OR IGNORE INTO close_retirement_inventories (
             attempt_id, scope, inspection_generation, inspection_fingerprint, sealed, captured_at
         ) VALUES (?1, ?2, ?3, ?4, 0, ?5)",
    )
    .bind(request.attempt_id.as_str())
    .bind(request.scope.as_str())
    .bind(generation)
    .bind(fingerprint)
    .bind(&now)
    .execute(&mut **tx)
    .await?;
    let resource_kind = request.residual.kind().as_str();
    let identity_kind = request.residual.identity().identity_kind();
    let identity_codec = request.residual.identity().codec();
    let identity_value = request.residual.identity().value();
    sqlx::query(
        "INSERT INTO close_expected_retirement_resources (
             attempt_id, scope, inspection_generation, inspection_fingerprint,
             resource_kind, identity_kind, identity_codec, identity_value
         ) SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8
           WHERE NOT EXISTS (
               SELECT 1 FROM close_expected_retirement_resources
               WHERE attempt_id = ?1 AND scope = ?2
                 AND inspection_generation = ?3 AND inspection_fingerprint = ?4
                 AND resource_kind = ?5 AND identity_kind = ?6
                 AND identity_codec = ?7 AND identity_value = ?8
           )",
    )
    .bind(request.attempt_id.as_str())
    .bind(request.scope.as_str())
    .bind(generation)
    .bind(fingerprint)
    .bind(resource_kind)
    .bind(identity_kind)
    .bind(identity_codec)
    .bind(&identity_value)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "UPDATE close_retirement_inventories SET sealed = 1
         WHERE attempt_id = ?1 AND scope = ?2
           AND inspection_generation = ?3 AND inspection_fingerprint = ?4 AND sealed = 0",
    )
    .bind(request.attempt_id.as_str())
    .bind(request.scope.as_str())
    .bind(generation)
    .bind(fingerprint)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "INSERT OR IGNORE INTO close_retirement_resource_history (
             attempt_id, scope, inspection_generation, inspection_fingerprint, resource_kind,
             identity_kind, identity_codec, identity_value, proof_kind, absence_basis,
             residual_reason, detail, recorded_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'residual', NULL, ?9, ?10, ?11)",
    )
    .bind(request.attempt_id.as_str())
    .bind(request.scope.as_str())
    .bind(generation)
    .bind(fingerprint)
    .bind(resource_kind)
    .bind(identity_kind)
    .bind(identity_codec)
    .bind(&identity_value)
    .bind(request.reason.as_str())
    .bind(&request.detail)
    .bind(&now)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "INSERT OR IGNORE INTO close_retirement_resources (
             attempt_id, scope, inspection_generation, inspection_fingerprint, resource_kind,
             identity_kind, identity_codec, identity_value, proof_kind, absence_basis,
             residual_reason, detail, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'residual', NULL, ?9, ?10, ?11, ?11)",
    )
    .bind(request.attempt_id.as_str())
    .bind(request.scope.as_str())
    .bind(generation)
    .bind(fingerprint)
    .bind(resource_kind)
    .bind(identity_kind)
    .bind(identity_codec)
    .bind(&identity_value)
    .bind(request.reason.as_str())
    .bind(&request.detail)
    .bind(&now)
    .execute(&mut **tx)
    .await?;
    if obligation.snapshot().is_none() {
        sqlx::query(
            "UPDATE close_obligations
             SET inspection_generation = 'no-worktree', inspection_fingerprint = 'no-worktree'
             WHERE attempt_id = ?1 AND inspection_generation IS NULL AND inspection_fingerprint IS NULL",
        )
        .bind(request.attempt_id.as_str())
        .execute(&mut **tx)
        .await?;
    }
    if obligation.phase() != ClosePhase::NeedsRepair {
        set_close_phase_tx(tx, request.attempt_id.as_str(), ClosePhase::NeedsRepair).await?;
    }
    Ok(())
}

async fn list_close_expected_retirement_resources_tx(
    tx: &mut Transaction<'_, Sqlite>,
    attempt_id: &str,
) -> DbResult<Vec<CloseExpectedRetirementResource>> {
    let status = sqlx::query(
            "SELECT
                 (SELECT COUNT(*) FROM close_attempt_scopes WHERE attempt_id = ?1) AS target_count,
                 (SELECT COUNT(*) FROM close_retirement_inventories inventory
                  JOIN close_obligations obligation ON obligation.attempt_id = inventory.attempt_id
                  WHERE inventory.attempt_id = ?1
                    AND inventory.inspection_generation = obligation.inspection_generation
                    AND inventory.inspection_fingerprint = obligation.inspection_fingerprint) AS inventory_count,
                 (SELECT COUNT(*) FROM close_retirement_inventories inventory
                  JOIN close_obligations obligation ON obligation.attempt_id = inventory.attempt_id
                  WHERE inventory.attempt_id = ?1 AND inventory.sealed = 0
                    AND inventory.inspection_generation = obligation.inspection_generation
                    AND inventory.inspection_fingerprint = obligation.inspection_fingerprint) AS unsealed_count
             WHERE EXISTS (SELECT 1 FROM close_obligations WHERE attempt_id = ?1)",
        )
        .bind(attempt_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| DbError::CloseFoundationNotFound(attempt_id.to_string()))?;
    let target_count: i64 = status.try_get("target_count")?;
    let inventory_count: i64 = status.try_get("inventory_count")?;
    let unsealed_count: i64 = status.try_get("unsealed_count")?;
    if target_count != inventory_count || unsealed_count != 0 {
        return Err(close_precondition(format!(
            "attempt {attempt_id} expected resources require a complete sealed inventory"
        )));
    }
    sqlx::query(
            "SELECT expected.attempt_id, expected.scope, expected.inspection_generation,
                    expected.inspection_fingerprint, expected.resource_kind,
                    expected.identity_kind, expected.identity_codec, expected.identity_value,
                    captured.captured_worktree_fingerprint,
                    captured.captured_worktree_locator
             FROM close_expected_retirement_resources expected
             JOIN close_obligations obligation ON obligation.attempt_id = expected.attempt_id
             JOIN close_attempt_scopes captured
               ON captured.attempt_id = expected.attempt_id AND captured.scope = expected.scope
             WHERE expected.attempt_id = ?1
               AND expected.inspection_generation = obligation.inspection_generation
               AND expected.inspection_fingerprint = obligation.inspection_fingerprint
             ORDER BY expected.scope, expected.resource_kind, expected.identity_kind, expected.identity_value",
        )
        .bind(attempt_id)
        .fetch_all(&mut **tx)
        .await?
        .into_iter()
        .map(|row| {
            let resource_kind_raw: String = row.try_get("resource_kind")?;
            let resource_kind = parse_retired_resource_kind(&resource_kind_raw)?;
            let identity_kind: String = row.try_get("identity_kind")?;
            let identity_codec: String = row.try_get("identity_codec")?;
            let identity_value: String = row.try_get("identity_value")?;
            Ok(CloseExpectedRetirementResource {
                attempt_id: parse_close_attempt_id(row.try_get("attempt_id")?)?,
                scope: WorkScopeId::parse(row.try_get::<String, _>("scope")?)
                    .map_err(|error| DbError::Serialization(error.to_string()))?,
                snapshot: CloseRetirementSnapshot::parse(
                    row.try_get::<String, _>("inspection_generation")?,
                    row.try_get::<String, _>("inspection_fingerprint")?,
                )
                .map_err(|error| DbError::Serialization(error.to_string()))?,
                resource: RetiredResourceIdentity::parse(
                    resource_kind,
                    parse_loss_item_identity(
                        &identity_kind,
                        &identity_codec,
                        &identity_value,
                        row.try_get("captured_worktree_fingerprint")?,
                        row.try_get("captured_worktree_locator")?,
                    )?,
                )
                .map_err(|error| DbError::Serialization(error.to_string()))?,
            })
        })
        .collect()
}

fn encode_host_path(path: &std::path::Path) -> String {
    #[cfg(unix)]
    let bytes = {
        use std::os::unix::ffi::OsStrExt as _;
        path.as_os_str().as_bytes()
    };
    #[cfg(not(unix))]
    let bytes = path.to_string_lossy().as_bytes();
    bytes.iter().fold(String::new(), |mut encoded, byte| {
        write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
        encoded
    })
}

fn decode_host_path(codec: &str, value: &str) -> DbResult<std::path::PathBuf> {
    if codec != "hex_path_v1" || !value.len().is_multiple_of(2) {
        return Err(DbError::Serialization(
            "invalid durable host path".to_string(),
        ));
    }
    let bytes = value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let text = std::str::from_utf8(pair)
                .map_err(|error| DbError::Serialization(error.to_string()))?;
            u8::from_str_radix(text, 16).map_err(|error| DbError::Serialization(error.to_string()))
        })
        .collect::<DbResult<Vec<_>>>()?;
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt as _;
        Ok(std::path::PathBuf::from(std::ffi::OsString::from_vec(
            bytes,
        )))
    }
    #[cfg(not(unix))]
    {
        String::from_utf8(bytes)
            .map(std::path::PathBuf::from)
            .map_err(|error| DbError::Serialization(error.to_string()))
    }
}

type OptionalFinalTombstoneColumns = (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);
type WorktreeCleanupPlanColumns = (
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

impl Database {
    /// Durably records intent to remove one exact sealed resource before external
    /// teardown begins. A restart can adopt absence only from this same-attempt
    /// dispatch record, never from a path or scope-wide guess.
    ///
    /// # Errors
    /// Returns a database error unless the request names one sealed resource in
    /// the exact active Close snapshot.
    pub async fn record_close_retirement_dispatch(
        &self,
        request: RecordCloseRetirementDispatchRequest,
    ) -> DbResult<()> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let current = close_obligation_for_update(&mut tx, request.attempt_id.as_str()).await?;
        let identity = request.resource.identity();
        let retry_authorized = if current.phase() == ClosePhase::Completed
            && matches!(
                current.close_outcome(),
                Some(
                    CloseCompletionOutcome::CloseIncomplete
                        | CloseCompletionOutcome::ArchivedCleanupAttention
                )
            )
            && request.resource.kind() == RetiredResourceKind::Worktree
        {
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM close_runs run JOIN close_run_retry_effects effect
                   ON effect.attempt_id = run.attempt_id AND effect.run_ordinal = run.run_ordinal
                 WHERE run.attempt_id = ?1 AND run.status = 'running' AND run.retry_evidence_kind = 'resource_plan'
                   AND run.run_ordinal = (SELECT MAX(run_ordinal) FROM close_runs WHERE attempt_id = run.attempt_id)
                   AND effect.scope = ?2 AND effect.resource_kind = ?3 AND effect.identity_kind = ?4
                   AND effect.identity_codec = ?5 AND effect.identity_value = ?6
                   AND NOT EXISTS (SELECT 1 FROM close_run_retry_successes success
                     WHERE success.attempt_id = effect.attempt_id AND success.run_ordinal = effect.run_ordinal
                       AND success.ordinal = effect.ordinal))",
            )
            .bind(request.attempt_id.as_str()).bind(request.scope.as_str())
            .bind(request.resource.kind().as_str()).bind(identity.identity_kind())
            .bind(identity.codec()).bind(identity.value()).fetch_one(&mut *tx).await?
        } else {
            false
        };
        if !(matches!(
            current.phase(),
            ClosePhase::RetirementRequested | ClosePhase::NeedsRepair
        ) || retry_authorized)
            || current.snapshot() != Some(&request.snapshot)
        {
            return Err(close_precondition(format!(
                "attempt {} dispatch lacks the exact active retirement authority",
                request.attempt_id
            )));
        }
        let inserted = sqlx::query(
            "INSERT OR IGNORE INTO close_retirement_resource_dispatches (
                 attempt_id, scope, inspection_generation, inspection_fingerprint,
                 resource_kind, identity_kind, identity_codec, identity_value, dispatched_at_us
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        )
        .bind(request.attempt_id.as_str())
        .bind(request.scope.as_str())
        .bind(request.snapshot.generation())
        .bind(request.snapshot.fingerprint())
        .bind(request.resource.kind().as_str())
        .bind(identity.identity_kind())
        .bind(identity.codec())
        .bind(identity.value())
        .bind(Utc::now().timestamp_micros())
        .execute(&mut *tx)
        .await?;
        if inserted.rows_affected() == 0 {
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(
                     SELECT 1 FROM close_retirement_resource_dispatches
                     WHERE attempt_id = ?1 AND scope = ?2
                       AND inspection_generation = ?3 AND inspection_fingerprint = ?4
                       AND resource_kind = ?5 AND identity_kind = ?6
                       AND identity_codec = ?7 AND identity_value = ?8
                 )",
            )
            .bind(request.attempt_id.as_str())
            .bind(request.scope.as_str())
            .bind(request.snapshot.generation())
            .bind(request.snapshot.fingerprint())
            .bind(request.resource.kind().as_str())
            .bind(identity.identity_kind())
            .bind(identity.codec())
            .bind(identity.value())
            .fetch_one(&mut *tx)
            .await?;
            if !exists {
                return Err(close_precondition(format!(
                    "attempt {} dispatch resource is not in the exact sealed inventory",
                    request.attempt_id
                )));
            }
        }
        tx.commit().await?;
        Ok(())
    }

    /// Durably binds the validated Git administrative directory to an exact
    /// dispatched worktree retirement before either filesystem location is deleted.
    ///
    /// # Errors
    /// Returns a database error unless the exact dispatch exists and the plan is
    /// either new or byte-for-byte identical to the prior plan.
    pub async fn record_close_worktree_cleanup_plan(
        &self,
        request: RecordCloseWorktreeCleanupPlanRequest,
    ) -> DbResult<()> {
        if request.resource.kind() != RetiredResourceKind::Worktree {
            return Err(close_precondition(
                "cleanup plan resource is not a worktree",
            ));
        }
        let identity = request.resource.identity();
        let administrative_dir_value = encode_host_path(&request.administrative_dir);
        let result = sqlx::query(
            "INSERT INTO close_worktree_cleanup_plans (
                 attempt_id, scope, inspection_generation, inspection_fingerprint,
                 resource_kind, identity_kind, identity_codec, identity_value,
                 administrative_dir_codec, administrative_dir_value,
                 administrative_dir_incarnation, planned_at_us
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
             ON CONFLICT DO NOTHING",
        )
        .bind(request.attempt_id.as_str())
        .bind(request.scope.as_str())
        .bind(request.snapshot.generation())
        .bind(request.snapshot.fingerprint())
        .bind(request.resource.kind().as_str())
        .bind(identity.identity_kind())
        .bind(identity.codec())
        .bind(identity.value())
        .bind("hex_path_v1")
        .bind(&administrative_dir_value)
        .bind(&request.administrative_dir_incarnation)
        .bind(Utc::now().timestamp_micros())
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            let prior: Option<(String, String, String)> = sqlx::query_as(
                "SELECT administrative_dir_codec, administrative_dir_value,
                        administrative_dir_incarnation
                 FROM close_worktree_cleanup_plans
                 WHERE attempt_id = ?1 AND scope = ?2
                   AND inspection_generation = ?3 AND inspection_fingerprint = ?4
                   AND resource_kind = ?5 AND identity_kind = ?6
                   AND identity_codec = ?7 AND identity_value = ?8",
            )
            .bind(request.attempt_id.as_str())
            .bind(request.scope.as_str())
            .bind(request.snapshot.generation())
            .bind(request.snapshot.fingerprint())
            .bind(request.resource.kind().as_str())
            .bind(identity.identity_kind())
            .bind(identity.codec())
            .bind(identity.value())
            .fetch_optional(&self.pool)
            .await?;
            if prior.as_ref().map(|(codec, value, incarnation)| {
                (codec.as_str(), value.as_str(), incarnation.as_str())
            }) != Some((
                "hex_path_v1",
                administrative_dir_value.as_str(),
                request.administrative_dir_incarnation.as_str(),
            )) {
                return Err(close_precondition(
                    "exact worktree cleanup plan conflicts with durable plan",
                ));
            }
        }
        Ok(())
    }

    /// Binds the private final tombstone to an exact worktree cleanup plan.
    ///
    /// A replay is accepted only when it presents the identical path and filesystem identity.
    ///
    /// # Errors
    /// Returns a database error or a precondition error when the exact plan is absent
    /// or already binds a different path or filesystem identity.
    pub async fn bind_close_worktree_final_tombstone(
        &self,
        request: BindCloseWorktreeFinalTombstoneRequest,
    ) -> DbResult<()> {
        let identity = request.resource.identity();
        let root = encode_host_path(&request.tombstone.root);
        let device = request.tombstone.device.to_string();
        let inode = request.tombstone.inode.to_string();
        let result = sqlx::query(
            "UPDATE close_worktree_cleanup_plans
             SET final_tombstone_root_codec = 'hex_path_v1',
                 final_tombstone_root_value = ?1,
                 final_tombstone_root_device = ?2,
                 final_tombstone_root_inode = ?3
             WHERE attempt_id = ?4 AND scope = ?5
               AND inspection_generation = ?6 AND inspection_fingerprint = ?7
               AND resource_kind = ?8 AND identity_kind = ?9
               AND identity_codec = ?10 AND identity_value = ?11
               AND final_tombstone_root_codec IS NULL",
        )
        .bind(&root)
        .bind(&device)
        .bind(&inode)
        .bind(request.attempt_id.as_str())
        .bind(request.scope.as_str())
        .bind(request.snapshot.generation())
        .bind(request.snapshot.fingerprint())
        .bind(request.resource.kind().as_str())
        .bind(identity.identity_kind())
        .bind(identity.codec())
        .bind(identity.value())
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 1 {
            return Ok(());
        }
        let prior: Option<OptionalFinalTombstoneColumns> = sqlx::query_as(
            "SELECT final_tombstone_root_codec, final_tombstone_root_value,
                    final_tombstone_root_device, final_tombstone_root_inode,
                    final_tombstone_object_device, final_tombstone_object_inode
             FROM close_worktree_cleanup_plans
             WHERE attempt_id = ?1 AND scope = ?2
               AND inspection_generation = ?3 AND inspection_fingerprint = ?4
               AND resource_kind = ?5 AND identity_kind = ?6
               AND identity_codec = ?7 AND identity_value = ?8",
        )
        .bind(request.attempt_id.as_str())
        .bind(request.scope.as_str())
        .bind(request.snapshot.generation())
        .bind(request.snapshot.fingerprint())
        .bind(request.resource.kind().as_str())
        .bind(identity.identity_kind())
        .bind(identity.codec())
        .bind(identity.value())
        .fetch_optional(&self.pool)
        .await?;
        if prior
            .as_ref()
            .map(|(codec, value, prior_device, prior_inode, _, _)| {
                match (
                    codec.as_deref(),
                    value.as_deref(),
                    prior_device.as_deref(),
                    prior_inode.as_deref(),
                ) {
                    (Some(codec), Some(value), Some(device), Some(inode)) => {
                        (codec, value, device, inode)
                    }
                    _ => ("", "", "", ""),
                }
            })
            == Some((
                "hex_path_v1",
                root.as_str(),
                device.as_str(),
                inode.as_str(),
            ))
        {
            Ok(())
        } else {
            Err(close_precondition(
                "exact worktree final tombstone conflicts with durable plan",
            ))
        }
    }

    /// Binds the moved final tombstone object after rename and before deletion.
    ///
    /// # Errors
    /// Returns a database or precondition error when the exact root plan is absent
    /// or the object identity conflicts with an earlier binding.
    pub async fn bind_close_worktree_final_tombstone_object(
        &self,
        request: BindCloseWorktreeFinalTombstoneObjectRequest,
    ) -> DbResult<()> {
        let identity = request.resource.identity();
        let device = request.object_device.to_string();
        let inode = request.object_inode.to_string();
        let result = sqlx::query(
            "UPDATE close_worktree_cleanup_plans
             SET final_tombstone_object_device = ?1,
                 final_tombstone_object_inode = ?2
             WHERE attempt_id = ?3 AND scope = ?4
               AND inspection_generation = ?5 AND inspection_fingerprint = ?6
               AND resource_kind = ?7 AND identity_kind = ?8
               AND identity_codec = ?9 AND identity_value = ?10
               AND final_tombstone_root_codec IS NOT NULL
               AND final_tombstone_object_device IS NULL",
        )
        .bind(&device)
        .bind(&inode)
        .bind(request.attempt_id.as_str())
        .bind(request.scope.as_str())
        .bind(request.snapshot.generation())
        .bind(request.snapshot.fingerprint())
        .bind(request.resource.kind().as_str())
        .bind(identity.identity_kind())
        .bind(identity.codec())
        .bind(identity.value())
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 1 {
            return Ok(());
        }
        let prior: Option<(Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT final_tombstone_object_device, final_tombstone_object_inode
             FROM close_worktree_cleanup_plans
             WHERE attempt_id = ?1 AND scope = ?2
               AND inspection_generation = ?3 AND inspection_fingerprint = ?4
               AND resource_kind = ?5 AND identity_kind = ?6
               AND identity_codec = ?7 AND identity_value = ?8",
        )
        .bind(request.attempt_id.as_str())
        .bind(request.scope.as_str())
        .bind(request.snapshot.generation())
        .bind(request.snapshot.fingerprint())
        .bind(request.resource.kind().as_str())
        .bind(identity.identity_kind())
        .bind(identity.codec())
        .bind(identity.value())
        .fetch_optional(&self.pool)
        .await?;
        if prior.as_ref().map(|(d, i)| (d.as_deref(), i.as_deref()))
            == Some((Some(device.as_str()), Some(inode.as_str())))
        {
            Ok(())
        } else {
            Err(close_precondition(
                "exact worktree final tombstone object conflicts with durable plan",
            ))
        }
    }

    /// Returns the administrative directory from an exact durable worktree cleanup plan.
    ///
    /// # Errors
    /// Returns a database or identity-decoding error when the plan cannot be read.
    pub async fn close_worktree_cleanup_plan(
        &self,
        attempt_id: &CloseAttemptId,
        scope: &WorkScopeId,
        snapshot: &CloseRetirementSnapshot,
        resource: &RetiredResourceIdentity,
    ) -> DbResult<Option<CloseWorktreeCleanupPlan>> {
        let identity = resource.identity();
        let rows: Vec<WorktreeCleanupPlanColumns> = sqlx::query_as(
            "SELECT DISTINCT administrative_dir_codec, administrative_dir_value,
                             administrative_dir_incarnation, final_tombstone_root_codec,
                             final_tombstone_root_value, final_tombstone_root_device,
                             final_tombstone_root_inode, final_tombstone_object_device,
                             final_tombstone_object_inode
             FROM close_worktree_cleanup_plans
             WHERE attempt_id = ?1 AND scope = ?2
               AND resource_kind = ?3 AND identity_kind = ?4
               AND identity_codec = ?5 AND identity_value = ?6
               AND inspection_generation = ?7 AND inspection_fingerprint = ?8",
        )
        .bind(attempt_id.as_str())
        .bind(scope.as_str())
        .bind(resource.kind().as_str())
        .bind(identity.identity_kind())
        .bind(identity.codec())
        .bind(identity.value())
        .bind(snapshot.generation())
        .bind(snapshot.fingerprint())
        .fetch_all(&self.pool)
        .await?;
        match rows.as_slice() {
            [] => Ok(None),
            [(
                codec,
                value,
                incarnation,
                tombstone_codec,
                tombstone_value,
                tombstone_device,
                tombstone_inode,
                object_device,
                object_inode,
            )] => {
                let final_tombstone = match (
                    tombstone_codec.as_deref(),
                    tombstone_value.as_deref(),
                    tombstone_device.as_deref(),
                    tombstone_inode.as_deref(),
                ) {
                    (None, None, None, None) => None,
                    (Some(tombstone_codec), Some(tombstone_value), Some(device), Some(inode)) => {
                        Some(CloseWorktreeFinalTombstone {
                            root: decode_host_path(tombstone_codec, tombstone_value)?,
                            device: device.parse().map_err(|_| {
                                close_precondition("invalid final tombstone device")
                            })?,
                            inode: inode
                                .parse()
                                .map_err(|_| close_precondition("invalid final tombstone inode"))?,
                            object_device: object_device
                                .as_deref()
                                .map(str::parse)
                                .transpose()
                                .map_err(|_| {
                                    close_precondition("invalid final tombstone object device")
                                })?,
                            object_inode: object_inode
                                .as_deref()
                                .map(str::parse)
                                .transpose()
                                .map_err(|_| {
                                    close_precondition("invalid final tombstone object inode")
                                })?,
                        })
                    }
                    _ => return Err(close_precondition("incomplete final tombstone binding")),
                };
                Ok(Some(CloseWorktreeCleanupPlan {
                    administrative_dir: decode_host_path(codec, value)?,
                    administrative_dir_incarnation: incarnation.clone(),
                    final_tombstone,
                }))
            }
            _ => Err(close_precondition(
                "exact worktree cleanup authority has conflicting plans",
            )),
        }
    }

    /// Returns whether the current exact attempt dispatched this resource before
    /// process-local teardown. This is deliberately narrower than prior evidence.
    ///
    /// # Errors
    /// Returns a database error when dispatch evidence cannot be read.
    pub async fn close_retirement_resource_was_dispatched(
        &self,
        attempt_id: &CloseAttemptId,
        scope: &WorkScopeId,
        snapshot: &CloseRetirementSnapshot,
        resource: &RetiredResourceIdentity,
    ) -> DbResult<bool> {
        let identity = resource.identity();
        sqlx::query_scalar(
            "SELECT EXISTS(
                 SELECT 1 FROM close_retirement_resource_dispatches
                 WHERE attempt_id = ?1 AND scope = ?2
                   AND inspection_generation = ?3 AND inspection_fingerprint = ?4
                   AND resource_kind = ?5 AND identity_kind = ?6
                   AND identity_codec = ?7 AND identity_value = ?8
             )",
        )
        .bind(attempt_id.as_str())
        .bind(scope.as_str())
        .bind(snapshot.generation())
        .bind(snapshot.fingerprint())
        .bind(resource.kind().as_str())
        .bind(identity.identity_kind())
        .bind(identity.codec())
        .bind(identity.value())
        .fetch_one(&self.pool)
        .await
        .map_err(Into::into)
    }

    /// Records one exact resource-retirement outcome for an authorized Close snapshot.
    ///
    /// # Errors
    /// Returns [`DbError`] when authority, replay, identity, or persistence validation fails.
    #[allow(clippy::too_many_lines)]
    pub async fn record_close_retirement_evidence(
        &self,
        request: RecordCloseRetirementEvidenceRequest,
    ) -> DbResult<()> {
        let mut conn = self.pool.acquire().await?;
        let mut tx = conn.begin_with("BEGIN IMMEDIATE").await?;
        let phase_record = sqlx::query(
            "SELECT phase, inspection_generation, inspection_fingerprint FROM close_obligations WHERE attempt_id = ?1",
        )
        .bind(request.attempt_id.as_str())
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| DbError::CloseFoundationNotFound(request.attempt_id.to_string()))?;
        let phase_raw: String = phase_record.try_get("phase")?;
        let phase = ClosePhase::from_db_str(&phase_raw)
            .ok_or_else(|| DbError::Serialization(format!("unknown close phase {phase_raw}")))?;
        let current_generation: Option<String> = phase_record.try_get("inspection_generation")?;
        let current_fingerprint: Option<String> = phase_record.try_get("inspection_fingerprint")?;
        let snapshot_is_authorized = current_generation.as_deref()
            == Some(request.snapshot.generation())
            && current_fingerprint.as_deref() == Some(request.snapshot.fingerprint());
        if !snapshot_is_authorized {
            return Err(close_precondition(format!(
                "attempt {} retirement evidence snapshot is stale",
                request.attempt_id.as_str()
            )));
        }
        let inspection_generation = request.snapshot.generation().to_string();
        if !matches!(
            phase,
            ClosePhase::RetirementRequested | ClosePhase::NeedsRepair
        ) {
            return Err(close_precondition(format!(
                "attempt {} phase {} does not admit retirement evidence",
                request.attempt_id,
                phase.as_str()
            )));
        }
        let targeted =
            sqlx::query("SELECT 1 FROM close_attempt_scopes WHERE attempt_id = ?1 AND scope = ?2")
                .bind(request.attempt_id.as_str())
                .bind(request.scope.as_str())
                .fetch_optional(&mut *tx)
                .await?;
        if targeted.is_none() {
            return Err(DbError::CloseFoundationPrecondition(format!(
                "attempt {} does not target scope {}",
                request.attempt_id,
                request.scope.as_str()
            )));
        }
        let identity = request.resource.identity();
        if let LossItemIdentity::Worktree(requested) = identity {
            let captured = sqlx::query_as::<_, (String, String, String)>(
                "SELECT captured_worktree_identity, captured_worktree_fingerprint,
                        captured_worktree_locator
                 FROM close_attempt_scopes
                 WHERE attempt_id = ?1 AND scope = ?2
                   AND captured_worktree_identity IS NOT NULL",
            )
            .bind(request.attempt_id.as_str())
            .bind(request.scope.as_str())
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| {
                close_precondition(format!(
                    "attempt {} scope {} has no captured worktree identity",
                    request.attempt_id, request.scope
                ))
            })?;
            let captured = WorktreeIdentity::from_parts(
                phoenix_core::domain::close::WorktreeId::parse(captured.0)
                    .map_err(|error| DbError::Serialization(error.to_string()))?,
                phoenix_core::domain::close::WorktreeFingerprint::parse(captured.1)
                    .map_err(|error| DbError::Serialization(error.to_string()))?,
                GitPathIdentity::decode_exact(&captured.2)
                    .map_err(|error| DbError::Serialization(error.to_string()))?,
            );
            if requested != &captured {
                return Err(close_precondition(format!(
                    "attempt {} worktree proof does not match the complete captured identity",
                    request.attempt_id
                )));
            }
        }
        let expected: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                 SELECT 1
                 FROM close_retirement_inventories inventory
                 JOIN close_expected_retirement_resources resource
                   ON resource.attempt_id = inventory.attempt_id
                  AND resource.scope = inventory.scope
                  AND resource.inspection_generation = inventory.inspection_generation
                  AND resource.inspection_fingerprint = inventory.inspection_fingerprint
                 WHERE inventory.attempt_id = ?1 AND inventory.scope = ?2
                   AND inventory.inspection_generation = ?3
                   AND inventory.inspection_fingerprint = ?4 AND inventory.sealed = 1
                   AND resource.resource_kind = ?5 AND resource.identity_kind = ?6
                   AND resource.identity_codec = ?7 AND resource.identity_value = ?8
             )",
        )
        .bind(request.attempt_id.as_str())
        .bind(request.scope.as_str())
        .bind(request.snapshot.generation())
        .bind(request.snapshot.fingerprint())
        .bind(request.resource.kind().as_str())
        .bind(identity.identity_kind())
        .bind(identity.codec())
        .bind(identity.value())
        .fetch_one(&mut *tx)
        .await?;
        if !expected {
            return Err(close_precondition(format!(
                "attempt {} resource is not in the exact sealed inventory",
                request.attempt_id
            )));
        }

        if let RetirementOutcome::AbsenceAdopted { absence_basis } = &request.outcome {
            validate_adopted_absence_evidence(&mut tx, &request, *absence_basis).await?;
        }

        let (proof_kind, absence_basis, residual_reason) = match &request.outcome {
            RetirementOutcome::Retired => ("retired", None, None),
            RetirementOutcome::AbsenceAdopted { absence_basis } => (
                "absence_adopted",
                Some(match absence_basis {
                    AbsenceBasis::SameAttemptPriorRetirement => "same_attempt_prior_retirement",
                    AbsenceBasis::PreexistingExactIdentityEvidence => {
                        "preexisting_exact_identity_evidence"
                    }
                }),
                None,
            ),
            RetirementOutcome::Residual { residual_reason } => {
                ("residual", None, Some(residual_reason.as_str()))
            }
        };

        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT OR IGNORE INTO close_retirement_resource_history (
                attempt_id, scope, inspection_generation, inspection_fingerprint,
                resource_kind, identity_kind, identity_codec, identity_value,
                proof_kind, absence_basis, residual_reason, detail, recorded_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        )
        .bind(request.attempt_id.as_str())
        .bind(request.scope.as_str())
        .bind(&inspection_generation)
        .bind(request.snapshot.fingerprint())
        .bind(request.resource.kind().as_str())
        .bind(request.resource.identity().identity_kind())
        .bind(request.resource.identity().codec())
        .bind(request.resource.identity().value())
        .bind(proof_kind)
        .bind(absence_basis)
        .bind(residual_reason)
        .bind(request.detail.as_deref().filter(|value| !value.is_empty()))
        .bind(&now)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "INSERT INTO close_retirement_resources (
                attempt_id, scope, inspection_generation, inspection_fingerprint, resource_kind, identity_kind, identity_codec, identity_value,
                proof_kind, absence_basis, residual_reason, detail, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?13)
             ON CONFLICT(attempt_id, scope, inspection_generation, inspection_fingerprint, resource_kind, identity_kind, identity_value)
             DO UPDATE SET
                 proof_kind = excluded.proof_kind,
                 absence_basis = excluded.absence_basis,
                 residual_reason = excluded.residual_reason,
                 detail = excluded.detail,
                 updated_at = excluded.updated_at
             WHERE close_retirement_resources.proof_kind = 'residual'
               AND excluded.proof_kind IN ('retired', 'absence_adopted')",
        )
        .bind(request.attempt_id.as_str())
        .bind(request.scope.as_str())
        .bind(&inspection_generation)
        .bind(request.snapshot.fingerprint())
        .bind(request.resource.kind().as_str())
        .bind(request.resource.identity().identity_kind())
        .bind(request.resource.identity().codec())
        .bind(request.resource.identity().value())
        .bind(proof_kind)
        .bind(absence_basis)
        .bind(residual_reason)
        .bind(request.detail.as_deref().filter(|value| !value.is_empty()))
        .bind(&now)
        .execute(&mut *tx)
        .await?;

        let persisted = sqlx::query_as::<
            _,
            (
                String,
                Option<String>,
                Option<String>,
                Option<String>,
                String,
            ),
        >(
            "SELECT proof_kind, absence_basis, residual_reason, detail, identity_codec
             FROM close_retirement_resources
             WHERE attempt_id = ?1 AND scope = ?2
               AND inspection_generation = ?3 AND inspection_fingerprint = ?4
               AND resource_kind = ?5 AND identity_kind = ?6 AND identity_value = ?7",
        )
        .bind(request.attempt_id.as_str())
        .bind(request.scope.as_str())
        .bind(&inspection_generation)
        .bind(request.snapshot.fingerprint())
        .bind(request.resource.kind().as_str())
        .bind(request.resource.identity().identity_kind())
        .bind(request.resource.identity().value())
        .fetch_one(&mut *tx)
        .await?;
        let requested = (
            proof_kind.to_string(),
            absence_basis.map(ToOwned::to_owned),
            residual_reason.map(ToOwned::to_owned),
            request
                .detail
                .as_deref()
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned),
            request.resource.identity().codec().to_string(),
        );
        let reuses_same_attempt_retirement = matches!(
            request.outcome,
            RetirementOutcome::AbsenceAdopted {
                absence_basis: AbsenceBasis::SameAttemptPriorRetirement
            }
        ) && persisted.0 == "retired"
            && persisted.4 == request.resource.identity().codec();
        if persisted != requested && !reuses_same_attempt_retirement {
            return Err(close_precondition(format!(
                "attempt {} retirement evidence replay differs from persisted evidence",
                request.attempt_id
            )));
        }

        if matches!(request.outcome, RetirementOutcome::Residual { .. })
            && phase == ClosePhase::RetirementRequested
        {
            set_close_phase_tx(
                &mut tx,
                request.attempt_id.as_str(),
                ClosePhase::NeedsRepair,
            )
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// # Errors
    /// Returns an error when the exact run is absent or cannot be decoded.
    pub async fn get_close_run(&self, run: &CloseRunRef) -> DbResult<CloseRun> {
        let row = sqlx::query("SELECT attempt_id, run_ordinal, status FROM close_runs WHERE attempt_id = ?1 AND run_ordinal = ?2")
            .bind(run.attempt_id.as_str()).bind(run.ordinal.get()).fetch_one(&self.pool).await?;
        parse_close_run_row(row)
    }

    /// # Errors
    /// Returns an error when retained run identities cannot be read.
    pub async fn list_running_close_runs(&self) -> DbResult<Vec<CloseRunRef>> {
        sqlx::query("SELECT attempt_id, run_ordinal, status FROM close_runs WHERE status = 'running' ORDER BY attempt_id, run_ordinal")
            .fetch_all(&self.pool).await?.into_iter()
            .map(|row| parse_close_run_row(row).map(|run| run.run)).collect()
    }

    /// # Errors
    /// Rejects conflicting evidence or a new success for a non-running run.
    pub async fn record_close_process_step_success(
        &self,
        success: &CloseProcessStepSuccess,
    ) -> DbResult<()> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let retained: Option<(String, i64)> = sqlx::query_as("SELECT outcome, observed_at_us FROM close_process_step_successes
            WHERE attempt_id = ?1 AND run_ordinal = ?2 AND scope = ?3 AND resource_kind = ?4 AND identity_value = ?5")
            .bind(success.run.attempt_id.as_str()).bind(success.run.ordinal.get()).bind(success.scope.as_str())
            .bind(success.resource_kind.as_str()).bind(success.identity.as_str()).fetch_optional(&mut *tx).await?;
        if let Some(retained) = retained {
            if retained != (success.outcome.as_str().to_owned(), success.observed_at_us) {
                return Err(close_precondition(
                    "process success replay conflicts with immutable evidence",
                ));
            }
        } else {
            sqlx::query("INSERT INTO close_process_step_successes
                (attempt_id, run_ordinal, scope, resource_kind, identity_value, outcome, observed_at_us)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)")
                .bind(success.run.attempt_id.as_str()).bind(success.run.ordinal.get()).bind(success.scope.as_str())
                .bind(success.resource_kind.as_str()).bind(success.identity.as_str()).bind(success.outcome.as_str())
                .bind(success.observed_at_us).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// # Errors
    /// Returns persistence or decoding errors without dropping malformed evidence.
    pub async fn list_close_process_step_successes(
        &self,
        run: &CloseRunRef,
    ) -> DbResult<Vec<CloseProcessStepSuccess>> {
        sqlx::query(
            "SELECT scope, resource_kind, identity_value, outcome, observed_at_us
            FROM close_process_step_successes WHERE attempt_id = ?1 AND run_ordinal = ?2
            ORDER BY scope, resource_kind, identity_value",
        )
        .bind(run.attempt_id.as_str())
        .bind(run.ordinal.get())
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(|row| {
            Ok(CloseProcessStepSuccess {
                run: run.clone(),
                scope: WorkScopeId::parse(row.try_get::<String, _>("scope")?)
                    .map_err(|error| DbError::Serialization(error.to_string()))?,
                resource_kind: match row.try_get::<&str, _>("resource_kind")? {
                    "bash_process_group" => CloseProcessResourceKind::BashProcessGroup,
                    "pty_session" => CloseProcessResourceKind::PtySession,
                    "browser_session" => CloseProcessResourceKind::BrowserSession,
                    other => {
                        return Err(DbError::Serialization(format!(
                            "unknown process kind {other}"
                        )))
                    }
                },
                identity: OpaqueIdentity::parse(row.try_get::<String, _>("identity_value")?)
                    .map_err(|error| DbError::Serialization(error.to_string()))?,
                outcome: match row.try_get::<&str, _>("outcome")? {
                    "retired" => CloseProcessStepOutcome::Retired,
                    "absence_verified" => CloseProcessStepOutcome::AbsenceVerified,
                    other => {
                        return Err(DbError::Serialization(format!(
                            "unknown process outcome {other}"
                        )))
                    }
                },
                observed_at_us: row.try_get("observed_at_us")?,
            })
        })
        .collect()
    }

    /// Admits a fresh bounded run; performs no settlement, inspection, or cleanup.
    ///
    /// # Errors
    /// Rejects stale/repeated requests, missing fresh safety evidence, expanded targets,
    /// previously completed effects, and persistence failures.
    pub async fn admit_close_safe_retry(
        &self,
        request: &AdmitCloseSafeRetryRequest,
    ) -> DbResult<CloseRunRef> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let run = Self::admit_close_safe_retry_tx(&mut tx, request).await?;
        tx.commit().await?;
        Ok(run)
    }

    /// # Errors
    /// The caller must roll back on invalid admission or persistence failure.
    pub async fn admit_close_safe_retry_tx(
        tx: &mut Transaction<'_, Sqlite>,
        request: &AdmitCloseSafeRetryRequest,
    ) -> DbResult<CloseRunRef> {
        let now = Utc::now().timestamp_micros();
        if request.observed_at_us < 0
            || request.observed_at_us > now
            || request.precondition_resolution.trim().is_empty()
            || request.safety_evidence.trim().is_empty()
        {
            return Err(close_precondition("safe retry requires fresh read-only precondition and safety evidence and exact remaining effects"));
        }
        let eligible: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM close_runs run
             JOIN close_cleanup_failures failure ON failure.attempt_id = run.attempt_id AND failure.cleanup_run_ordinal = run.run_ordinal
             JOIN close_obligations obligation ON obligation.attempt_id = run.attempt_id
             WHERE run.attempt_id = ?1 AND run.run_ordinal = ?2 AND run.status = 'stopped'
               AND obligation.phase = 'completed' AND obligation.close_outcome IN ('close_incomplete', 'archived_cleanup_attention')
               AND failure.occurred_at_us < ?3
               AND NOT EXISTS (SELECT 1 FROM close_runs later WHERE later.attempt_id = run.attempt_id
                   AND (later.run_ordinal > run.run_ordinal OR later.status = 'running')))",
        ).bind(request.failed_run.attempt_id.as_str()).bind(request.failed_run.ordinal.get())
            .bind(request.observed_at_us).fetch_one(&mut **tx).await?;
        if !eligible {
            return Err(close_precondition(
                "safe retry requires the latest stopped run, fresh evidence, and no running run",
            ));
        }
        let completion_only = request.remaining_effects.is_empty();
        if completion_only && !verified_completion_source_tx(tx, &request.failed_run).await? {
            return Err(close_precondition(
                "completion-only retry requires an interrupted fully successful retry plan",
            ));
        }
        let run = CloseRunRef {
            attempt_id: request.failed_run.attempt_id.clone(),
            ordinal: request
                .failed_run
                .ordinal
                .checked_next()
                .map_err(|error| close_precondition(error.to_string()))?,
        };
        for (ordinal, effect) in request.remaining_effects.iter().enumerate() {
            let identity = effect.resource.identity();
            if let LossItemIdentity::Worktree(worktree) = identity {
                let captured: Option<(String, String, String)> = sqlx::query_as(
                    "SELECT captured_worktree_identity, captured_worktree_fingerprint, captured_worktree_locator
                     FROM close_attempt_scopes WHERE attempt_id = ?1 AND scope = ?2 AND captured_worktree_identity IS NOT NULL",
                ).bind(run.attempt_id.as_str()).bind(effect.scope.as_str()).fetch_optional(&mut **tx).await?;
                if captured
                    != Some((
                        worktree.id().as_str().to_owned(),
                        worktree.fingerprint().as_str().to_owned(),
                        worktree.locator().encode(),
                    ))
                {
                    return Err(close_precondition(
                        "safe retry worktree differs from the complete captured identity",
                    ));
                }
            }
            sqlx::query(
                "INSERT INTO close_run_retry_effects (attempt_id, run_ordinal, ordinal, scope, resource_kind, identity_kind, identity_codec, identity_value)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            ).bind(run.attempt_id.as_str()).bind(run.ordinal.get())
                .bind(i64::try_from(ordinal).map_err(|error| close_precondition(error.to_string()))?)
                .bind(effect.scope.as_str()).bind(effect.resource.kind().as_str())
                .bind(identity.identity_kind()).bind(identity.codec()).bind(identity.value())
                .execute(&mut **tx).await?;
        }
        sqlx::query(
            "INSERT INTO close_runs (attempt_id, run_ordinal, status, created_at_us, retry_requested_by,
                 retry_observed_at_us, precondition_resolution, safety_evidence, retry_evidence_kind)
             VALUES (?1, ?2, 'running', ?3, ?4, ?5, ?6, ?7, ?8)",
        ).bind(run.attempt_id.as_str()).bind(run.ordinal.get()).bind(now)
            .bind(match request.requested_by { CloseRetryRequestedBy::User => "user", CloseRetryRequestedBy::Global => "global" })
            .bind(request.observed_at_us).bind(&request.precondition_resolution).bind(&request.safety_evidence)
            .bind(if completion_only { "verified_completion" } else { "resource_plan" })
            .execute(&mut **tx).await?;
        Ok(run)
    }

    /// Reports whether the latest stopped run permits an explicit completion-only retry.
    /// # Errors
    /// Returns a database error if retained proof cannot be read.
    pub async fn close_retry_verified_completion_eligible(
        &self,
        failed_run: &CloseRunRef,
    ) -> DbResult<bool> {
        let mut tx = self.pool.begin().await?;
        let eligible = verified_completion_source_tx(&mut tx, failed_run).await?
            && sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM close_runs run
                JOIN close_obligations obligation ON obligation.attempt_id = run.attempt_id
                WHERE run.attempt_id = ?1 AND run.run_ordinal = ?2
                  AND run.run_ordinal = (SELECT MAX(run_ordinal) FROM close_runs WHERE attempt_id = run.attempt_id)
                  AND obligation.phase = 'completed' AND obligation.close_outcome IN ('close_incomplete', 'archived_cleanup_attention'))")
                .bind(failed_run.attempt_id.as_str()).bind(failed_run.ordinal.get()).fetch_one(&mut *tx).await?;
        Ok(eligible)
    }

    /// Returns whether a prior retry run durably established this exact resource success.
    ///
    /// # Errors
    /// Returns a database error when the durable success query fails.
    pub async fn close_resource_has_retry_success(
        &self,
        attempt_id: &CloseAttemptId,
        scope: &WorkScopeId,
        resource: &RetiredResourceIdentity,
    ) -> DbResult<bool> {
        Ok(sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM close_run_retry_successes success
             JOIN close_run_retry_effects effect ON effect.attempt_id = success.attempt_id
               AND effect.run_ordinal = success.run_ordinal AND effect.ordinal = success.ordinal
             WHERE success.attempt_id = ?1 AND effect.scope = ?2
               AND effect.resource_kind = ?3 AND effect.identity_kind = ?4
               AND effect.identity_codec = ?5 AND effect.identity_value = ?6)",
        )
        .bind(attempt_id.as_str())
        .bind(scope.as_str())
        .bind(resource.kind().as_str())
        .bind(resource.identity().identity_kind())
        .bind(resource.identity().codec())
        .bind(resource.identity().value())
        .fetch_one(&self.pool)
        .await?)
    }

    /// Lists the immutable retry plan for this exact run in failure-child ordinal order.
    ///
    /// # Errors
    /// Rejects absent/non-retry runs or malformed retained identities.
    pub async fn list_close_safe_retry_effects(
        &self,
        run: &CloseRunRef,
    ) -> DbResult<Vec<CloseSafeRetryEffect>> {
        let valid: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM close_runs WHERE attempt_id = ?1 AND run_ordinal = ?2 AND retry_evidence_kind IN ('resource_plan', 'verified_completion'))")
            .bind(run.attempt_id.as_str()).bind(run.ordinal.get()).fetch_one(&self.pool).await?;
        if !valid {
            return Err(close_precondition(
                "safe retry plan requires an exact retry run",
            ));
        }
        sqlx::query("SELECT effect.*, captured.captured_worktree_fingerprint, captured.captured_worktree_locator
            FROM close_run_retry_effects effect
            JOIN close_attempt_scopes captured ON captured.attempt_id = effect.attempt_id AND captured.scope = effect.scope
            WHERE effect.attempt_id = ?1 AND effect.run_ordinal = ?2 ORDER BY effect.ordinal")
            .bind(run.attempt_id.as_str()).bind(run.ordinal.get()).fetch_all(&self.pool).await?
            .into_iter().map(|row| Ok(CloseSafeRetryEffect {
                scope: WorkScopeId::parse(row.try_get::<String, _>("scope")?)
                    .map_err(|error| DbError::Serialization(error.to_string()))?,
                resource: parse_cleanup_resource_identity(&row)?,
            })).collect()
    }

    /// Reads successful and pending effects of the exact latest running resource plan.
    ///
    /// # Errors
    /// Rejects stale, stopped, or malformed plans.
    pub async fn close_safe_retry_progress(
        &self,
        run: &CloseRunRef,
    ) -> DbResult<CloseSafeRetryProgress> {
        let mut tx = self.pool.begin().await?;
        close_safe_retry_progress_tx(&mut tx, run).await
    }

    /// Persists successful retirement of one exact-run planned resource.
    ///
    /// # Errors
    /// Rejects empty evidence, unplanned resources, stopped runs, and duplicate success.
    pub async fn record_close_safe_retry_success(
        &self,
        run: &CloseRunRef,
        effect: &CloseSafeRetryEffect,
        detail: &str,
    ) -> DbResult<()> {
        if detail.trim().is_empty() {
            return Err(close_precondition("retry success requires evidence"));
        }
        let identity = effect.resource.identity();
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let retained: Option<String> = sqlx::query_scalar(
                "SELECT success.detail FROM close_run_retry_successes success
                 JOIN close_run_retry_effects planned ON planned.attempt_id = success.attempt_id
                   AND planned.run_ordinal = success.run_ordinal AND planned.ordinal = success.ordinal
                 JOIN close_runs run ON run.attempt_id = planned.attempt_id AND run.run_ordinal = planned.run_ordinal
                 WHERE run.retry_evidence_kind = 'resource_plan'
                   AND run.run_ordinal = (SELECT MAX(run_ordinal) FROM close_runs WHERE attempt_id = run.attempt_id)
                   AND success.attempt_id = ?1 AND success.run_ordinal = ?2 AND planned.scope = ?3
                   AND planned.resource_kind = ?4 AND planned.identity_kind = ?5
                   AND planned.identity_codec = ?6 AND planned.identity_value = ?7",
            )
            .bind(run.attempt_id.as_str()).bind(run.ordinal.get()).bind(effect.scope.as_str())
            .bind(effect.resource.kind().as_str()).bind(identity.identity_kind()).bind(identity.codec())
            .bind(identity.value()).fetch_optional(&mut *tx).await?;
        if let Some(retained) = retained {
            if retained == detail {
                return Ok(());
            }
            return Err(close_precondition(
                "retry success replay conflicts with retained detail",
            ));
        }
        let progress = close_safe_retry_progress_tx(&mut tx, run).await?;
        if !progress.pending.contains(effect) {
            return Err(close_precondition(
                "retry success requires a pending exact-run effect",
            ));
        }
        let inserted = sqlx::query("INSERT INTO close_run_retry_successes (attempt_id, run_ordinal, ordinal, detail, observed_at_us)
            SELECT effect.attempt_id, effect.run_ordinal, effect.ordinal, ?8, ?9 FROM close_run_retry_effects effect
            JOIN close_runs run ON run.attempt_id = effect.attempt_id AND run.run_ordinal = effect.run_ordinal
            WHERE effect.attempt_id = ?1 AND effect.run_ordinal = ?2 AND effect.scope = ?3 AND effect.resource_kind = ?4
              AND effect.identity_kind = ?5 AND effect.identity_codec = ?6 AND effect.identity_value = ?7
              AND run.status = 'running' AND run.retry_evidence_kind = 'resource_plan'")
            .bind(run.attempt_id.as_str()).bind(run.ordinal.get()).bind(effect.scope.as_str())
            .bind(effect.resource.kind().as_str()).bind(identity.identity_kind()).bind(identity.codec())
            .bind(identity.value()).bind(detail).bind(Utc::now().timestamp_micros())
            .execute(&mut *tx).await?;
        if inserted.rows_affected() != 1 {
            return Err(close_precondition(
                "retry success requires exact running planned resource",
            ));
        }
        tx.commit().await?;
        Ok(())
    }

    /// Atomically archives the original outcome after complete exact-run retry proof.
    ///
    /// # Errors
    /// Rejects incomplete, stopped, stale, or unauthorized scope-free retry runs.
    pub async fn complete_close_safe_retry(&self, run: &CloseRunRef) -> DbResult<CloseObligation> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let eligible: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM close_runs run
            JOIN close_obligations obligation ON obligation.attempt_id = run.attempt_id
            WHERE run.attempt_id = ?1 AND run.run_ordinal = ?2 AND run.run_ordinal > 1
              AND run.status = 'running' AND run.run_ordinal = (SELECT MAX(run_ordinal) FROM close_runs WHERE attempt_id = run.attempt_id)
              AND obligation.phase = 'completed' AND obligation.close_outcome IN ('close_incomplete', 'archived_cleanup_attention')
              AND ((run.retry_evidence_kind = 'resource_plan'
              AND EXISTS (SELECT 1 FROM close_run_retry_effects effect WHERE effect.attempt_id = run.attempt_id AND effect.run_ordinal = run.run_ordinal))
              OR (run.retry_evidence_kind = 'verified_completion'
              AND NOT EXISTS (SELECT 1 FROM close_run_retry_effects effect WHERE effect.attempt_id = run.attempt_id AND effect.run_ordinal = run.run_ordinal)
              AND EXISTS (SELECT 1 FROM close_runs prior JOIN close_cleanup_failures failure
                ON failure.attempt_id = prior.attempt_id AND failure.cleanup_run_ordinal = prior.run_ordinal
                WHERE prior.attempt_id = run.attempt_id AND prior.run_ordinal = run.run_ordinal - 1
                  AND prior.status = 'stopped' AND prior.retry_evidence_kind IN ('resource_plan', 'verified_completion')
                  AND failure.authority_kind = 'attempt_interrupted'
                  AND NOT EXISTS (SELECT 1 FROM close_cleanup_failure_resources child WHERE child.failure_occurrence_id = failure.failure_occurrence_id)
                  AND ((prior.retry_evidence_kind = 'resource_plan'
                    AND EXISTS (SELECT 1 FROM close_run_retry_effects effect WHERE effect.attempt_id = prior.attempt_id AND effect.run_ordinal = prior.run_ordinal)
                    AND NOT EXISTS (SELECT 1 FROM close_run_retry_effects effect WHERE effect.attempt_id = prior.attempt_id AND effect.run_ordinal = prior.run_ordinal
                      AND NOT EXISTS (SELECT 1 FROM close_run_retry_successes success WHERE success.attempt_id = effect.attempt_id AND success.run_ordinal = effect.run_ordinal AND success.ordinal = effect.ordinal)))
                  OR (prior.retry_evidence_kind = 'verified_completion'
                    AND NOT EXISTS (SELECT 1 FROM close_run_retry_effects effect WHERE effect.attempt_id = prior.attempt_id AND effect.run_ordinal = prior.run_ordinal))))))
              AND NOT EXISTS (SELECT 1 FROM close_run_retry_effects effect WHERE effect.attempt_id = run.attempt_id
                AND effect.run_ordinal = run.run_ordinal AND NOT EXISTS (SELECT 1 FROM close_run_retry_successes success
                  WHERE success.attempt_id = effect.attempt_id AND success.run_ordinal = effect.run_ordinal AND success.ordinal = effect.ordinal)))")
            .bind(run.attempt_id.as_str()).bind(run.ordinal.get()).fetch_one(&mut *tx).await?;
        if !eligible {
            return Err(close_precondition(
                "retry completion requires exact running run and complete successful plan",
            ));
        }
        let now = Utc::now().to_rfc3339();
        let latest: String = sqlx::query_scalar("SELECT conversation_id FROM close_attempt_members WHERE attempt_id = ?1 AND member_role IN ('latest', 'root_latest')")
            .bind(run.attempt_id.as_str()).fetch_one(&mut *tx).await?;
        let sequence: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(sequence_id), 0) + 1 FROM messages WHERE conversation_id = ?1",
        )
        .bind(&latest)
        .fetch_one(&mut *tx)
        .await?;
        let content = MessageContent::system(format!("Close retry run {} for attempt {} completed; cleanup retired and conversation archived in History.", run.ordinal.get(), run.attempt_id));
        let content = serde_json::to_string(&content.to_stored_json())
            .map_err(|error| DbError::Serialization(error.to_string()))?;
        sqlx::query("INSERT INTO messages (message_id, conversation_id, sequence_id, message_type, content, created_at)
            VALUES (?1, ?2, ?3, 'system', ?4, ?5)")
            .bind(format!("close-run-outcome:{}:{}", run.ordinal.get(), run.attempt_id))
            .bind(&latest).bind(sequence).bind(content).bind(&now).execute(&mut *tx).await?;
        sqlx::query("UPDATE conversations SET archived = 1, updated_at = ?2
            WHERE id IN (SELECT conversation_id FROM close_attempt_participants WHERE attempt_id = ?1)")
            .bind(run.attempt_id.as_str()).bind(&now).execute(&mut *tx).await?;
        sqlx::query("UPDATE product_conversations SET ordinary_lifecycle = 'history'
            WHERE id = (SELECT product_conversation_id FROM close_obligations WHERE attempt_id = ?1) AND kind = 'ordinary'")
            .bind(run.attempt_id.as_str()).execute(&mut *tx).await?;
        let changed = sqlx::query("UPDATE close_obligations SET close_outcome = 'archived', updated_at = ?2
            WHERE attempt_id = ?1 AND phase = 'completed' AND close_outcome IN ('close_incomplete', 'archived_cleanup_attention')")
            .bind(run.attempt_id.as_str()).bind(&now).execute(&mut *tx).await?;
        if changed.rows_affected() != 1 {
            return Err(close_precondition(
                "retry completion lost terminal authority",
            ));
        }
        let obligation = close_obligation_for_update(&mut tx, run.attempt_id.as_str()).await?;
        tx.commit().await?;
        Ok(obligation)
    }

    /// Records interrupted-run uncertainty and one mandatory event without executing cleanup.
    /// An already stopped/completed run is an observation-only no-op.
    ///
    /// # Errors
    /// Rejects absent runs, unavailable captured scope authority, and persistence failures.
    #[allow(clippy::too_many_lines)]
    pub async fn classify_interrupted_close_run(
        &self,
        run: &CloseRunRef,
    ) -> DbResult<CloseObligation> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let row = sqlx::query("SELECT attempt_id, run_ordinal, status FROM close_runs WHERE attempt_id = ?1 AND run_ordinal = ?2")
            .bind(run.attempt_id.as_str()).bind(run.ordinal.get()).fetch_one(&mut *tx).await?;
        let retained = parse_close_run_row(row)?;
        let obligation = close_obligation_for_update(&mut tx, run.attempt_id.as_str()).await?;
        if retained.status != CloseRunStatus::Running {
            tx.commit().await?;
            return Ok(obligation);
        }
        let completion_only_running: bool = if run.ordinal == CloseRunOrdinal::INITIAL {
            false
        } else {
            sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM close_runs WHERE attempt_id = ?1 AND run_ordinal = ?2
                 AND status = 'running' AND retry_evidence_kind = 'verified_completion')",
            )
            .bind(run.attempt_id.as_str())
            .bind(run.ordinal.get())
            .fetch_one(&mut *tx)
            .await?
        };
        let authority = if run.ordinal == CloseRunOrdinal::INITIAL {
            let unresolved =
                unresolved_expected_close_cleanup_resources_tx(&mut tx, run.attempt_id.as_str())
                    .await?;
            if let Some(first) = unresolved.first() {
                CloseCleanupFailureAuthority::ExpectedResource {
                    scope: first.scope.clone(),
                    snapshot: obligation.snapshot().cloned().ok_or_else(|| {
                        close_precondition(
                            "interrupted sealed inventory requires its retained snapshot",
                        )
                    })?,
                    resource: first.resource.clone(),
                }
            } else {
                let scope: Option<String> = sqlx::query_scalar("SELECT scope FROM close_attempt_scopes WHERE attempt_id = ?1 ORDER BY scope LIMIT 1")
                    .bind(run.attempt_id.as_str()).fetch_optional(&mut *tx).await?;
                if let Some(scope) = scope {
                    let resource = RetiredResourceIdentity::parse(
                        RetiredResourceKind::WorkScope,
                        LossItemIdentity::Opaque(
                            OpaqueIdentity::parse(&scope)
                                .map_err(|error| close_precondition(error.to_string()))?,
                        ),
                    )
                    .map_err(|error| close_precondition(error.to_string()))?;
                    CloseCleanupFailureAuthority::CapturedScope {
                        scope: WorkScopeId::parse(scope)
                            .map_err(|error| close_precondition(error.to_string()))?,
                        resource,
                    }
                } else {
                    CloseCleanupFailureAuthority::AttemptInterrupted
                }
            }
        } else if completion_only_running {
            CloseCleanupFailureAuthority::AttemptInterrupted
        } else {
            let progress = close_safe_retry_progress_tx(&mut tx, run).await?;
            match progress.pending.first() {
                Some(first) => CloseCleanupFailureAuthority::ExpectedResource {
                    scope: first.scope.clone(),
                    snapshot: obligation.snapshot().cloned().ok_or_else(|| {
                        close_precondition("retry requires original retirement snapshot")
                    })?,
                    resource: first.resource.clone(),
                },
                None => CloseCleanupFailureAuthority::AttemptInterrupted,
            }
        };
        let remaining_resources = if completion_only_running {
            Vec::new()
        } else if run.ordinal != CloseRunOrdinal::INITIAL {
            let progress = close_safe_retry_progress_tx(&mut tx, run).await?;
            match progress.pending.first() {
                Some(first) => retry_failure_resources_tx(&mut tx, run, &progress, first).await?,
                None => Vec::new(),
            }
        } else if let CloseCleanupFailureAuthority::ExpectedResource {
            scope, resource, ..
        } = &authority
        {
            expected_close_cleanup_failure_resources_tx(
                &mut tx,
                run.attempt_id.as_str(),
                scope,
                resource,
            )
            .await?
        } else {
            sqlx::query_scalar::<_, String>(
                "SELECT scope FROM close_attempt_scopes WHERE attempt_id = ?1 ORDER BY scope",
            )
            .bind(run.attempt_id.as_str())
            .fetch_all(&mut *tx)
            .await?
            .into_iter()
            .map(|captured| {
                let captured = WorkScopeId::parse(captured)
                    .map_err(|error| close_precondition(error.to_string()))?;
                Ok(CloseCleanupFailureResource {
                    resource: RetiredResourceIdentity::parse(
                        RetiredResourceKind::WorkScope,
                        LossItemIdentity::Opaque(
                            OpaqueIdentity::parse(captured.as_str())
                                .map_err(|error| close_precondition(error.to_string()))?,
                        ),
                    )
                    .map_err(|error| close_precondition(error.to_string()))?,
                    disposition: if authority.scope() == Some(&captured) {
                        CloseCleanupResourceDisposition::Failed
                    } else {
                        CloseCleanupResourceDisposition::Unknown
                    },
                    scope: captured,
                })
            })
            .collect::<DbResult<Vec<_>>>()?
        };
        let request = TerminalizeInitialCloseCleanupFailureRequest {
            failure_occurrence_id: run.failure_occurrence_id(),
            attempt_id: run.attempt_id.clone(),
            source_product_conversation_id: obligation.product_conversation_id().clone(),
            authority,
            remaining_resources,
            reason: RetirementFailureReason::Interrupted,
            detail: format!(
                "Close run {} interrupted in {}; startup observed retained authority only",
                run.ordinal.get(),
                obligation.phase().as_str()
            ),
            stop_certainty: retained_interrupted_stop_certainty_tx(&mut tx, run).await?,
            occurred_at_us: Utc::now().timestamp_micros(),
        };
        let result = Self::terminalize_close_run_cleanup_failure_tx(&mut tx, run, &request).await?;
        tx.commit().await?;
        Ok(result)
    }

    /// Derives every unresolved target from retained sealed inventory and exact proofs.
    ///
    /// # Errors
    /// Returns a database error when retained inventory or proof rows cannot be read.
    pub async fn unresolved_expected_close_cleanup_resources(
        &self,
        attempt_id: &str,
    ) -> DbResult<Vec<CloseCleanupFailureResource>> {
        let mut tx = self.pool.begin().await?;
        let remaining = unresolved_expected_close_cleanup_resources_tx(&mut tx, attempt_id).await?;
        tx.commit().await?;
        Ok(remaining)
    }

    /// Derives unresolved targets from the complete sealed inventory and exact retirement proofs.
    ///
    /// # Errors
    /// Rejects incomplete inventories and a failed target with successful retirement proof.
    pub async fn expected_close_cleanup_failure_resources(
        &self,
        attempt_id: &str,
        failed_scope: &WorkScopeId,
        failed_resource: &RetiredResourceIdentity,
    ) -> DbResult<Vec<CloseCleanupFailureResource>> {
        let mut tx = self.pool.begin().await?;
        let remaining = expected_close_cleanup_failure_resources_tx(
            &mut tx,
            attempt_id,
            failed_scope,
            failed_resource,
        )
        .await?;
        tx.commit().await?;
        Ok(remaining)
    }

    /// Reads immutable diagnostic occurrences and their ordered resource rows in one snapshot.
    ///
    /// # Errors
    /// Returns persistence or decoding errors without dropping malformed evidence.
    pub async fn list_close_cleanup_failures(
        &self,
        attempt_id: &str,
    ) -> DbResult<Vec<CloseCleanupFailure>> {
        let mut tx = self.pool.begin().await?;
        let rows = sqlx::query(
            "SELECT failure.*, captured.captured_worktree_fingerprint, captured.captured_worktree_locator
             FROM close_cleanup_failures failure LEFT JOIN close_attempt_scopes captured
               ON captured.attempt_id = failure.attempt_id AND captured.scope = failure.scope
             WHERE failure.attempt_id = ?1 ORDER BY failure.cleanup_run_ordinal",
        ).bind(attempt_id).fetch_all(&mut *tx).await?;
        let mut failures = Vec::with_capacity(rows.len());
        for row in rows {
            let authority = parse_cleanup_failure_authority(&row)?;
            let stop_certainty = match row.try_get::<&str, _>("stop_certainty")? {
                "shutdown_uncertain" => CloseStopCertainty::ShutdownUncertain,
                "conversation_and_processes_stopped" => {
                    CloseStopCertainty::ConversationAndProcessesStopped {
                        confirmed_at_us: row.try_get("confirmed_at_us")?,
                    }
                }
                other => {
                    return Err(DbError::Serialization(format!(
                        "unknown cleanup stop certainty {other}"
                    )))
                }
            };
            let failure_occurrence_id: String = row.try_get("failure_occurrence_id")?;
            let remaining_resources =
                close_cleanup_failure_resources_tx(&mut tx, &failure_occurrence_id).await?;
            let attempt_id = parse_close_attempt_id(row.try_get("attempt_id")?)?;
            failures.push(CloseCleanupFailure {
                run_ordinal: CloseRunOrdinal::parse(row.try_get("cleanup_run_ordinal")?)
                    .map_err(|error| DbError::Serialization(error.to_string()))?,
                occurrence: TerminalizeInitialCloseCleanupFailureRequest {
                    failure_occurrence_id,
                    attempt_id,
                    source_product_conversation_id: ProductConversationId::parse(
                        row.try_get::<String, _>("source_product_conversation_id")?,
                    )
                    .map_err(|error| DbError::Serialization(error.to_string()))?,
                    authority,
                    remaining_resources,
                    reason: parse_retirement_failure_reason(&row.try_get::<String, _>("reason")?)?,
                    detail: row.try_get("detail")?,
                    stop_certainty,
                    occurred_at_us: row.try_get("occurred_at_us")?,
                },
            });
        }
        tx.commit().await?;
        Ok(failures)
    }

    /// Returns immutable timing for an already-recorded cleanup failure.
    ///
    /// # Errors
    /// Returns a database error when the query fails.
    pub async fn close_cleanup_failure_timing(
        &self,
        failure_occurrence_id: &str,
    ) -> DbResult<Option<(i64, Option<i64>)>> {
        sqlx::query_as(
            "SELECT occurred_at_us, confirmed_at_us
             FROM close_cleanup_failures WHERE failure_occurrence_id = ?1",
        )
        .bind(failure_occurrence_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(Into::into)
    }

    /// Atomically stops run 1 and records the original Close authority's visible outcome.
    ///
    /// # Errors
    /// Returns an error for stale authority, conflicting replay, or persistence failure.
    pub async fn terminalize_initial_close_cleanup_failure(
        &self,
        request: &TerminalizeInitialCloseCleanupFailureRequest,
    ) -> DbResult<CloseObligation> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let result = Self::terminalize_initial_close_cleanup_failure_tx(&mut tx, request).await?;
        tx.commit().await?;
        Ok(result)
    }

    /// Transaction seam for pairing terminalization with delivery in the caller's transaction.
    ///
    /// # Errors
    /// Returns an error for stale authority, conflicting replay, or persistence failure.
    /// The caller must roll back its transaction on error.
    pub async fn terminalize_initial_close_cleanup_failure_tx(
        tx: &mut Transaction<'_, Sqlite>,
        request: &TerminalizeInitialCloseCleanupFailureRequest,
    ) -> DbResult<CloseObligation> {
        let run = CloseRunRef::initial(request.attempt_id.clone());
        Self::terminalize_close_run_cleanup_failure_tx(tx, &run, request).await
    }

    /// Stops exactly one admitted run without resuming or altering an earlier run.
    ///
    /// # Errors
    /// Rejects stale authority, conflicting replay, or persistence failure.
    pub async fn terminalize_close_run_cleanup_failure(
        &self,
        run: &CloseRunRef,
        request: &TerminalizeInitialCloseCleanupFailureRequest,
    ) -> DbResult<CloseObligation> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let obligation =
            Self::terminalize_close_run_cleanup_failure_tx(&mut tx, run, request).await?;
        tx.commit().await?;
        Ok(obligation)
    }

    /// # Errors
    /// The caller must roll back on stale authority, conflicting replay, or persistence failure.
    #[allow(clippy::too_many_lines)]
    pub async fn terminalize_close_run_cleanup_failure_tx(
        tx: &mut Transaction<'_, Sqlite>,
        run: &CloseRunRef,
        request: &TerminalizeInitialCloseCleanupFailureRequest,
    ) -> DbResult<CloseObligation> {
        if run.attempt_id != request.attempt_id
            || request.failure_occurrence_id != run.failure_occurrence_id()
        {
            return Err(close_precondition(
                "cleanup failure requires the exact run and deterministic occurrence identity",
            ));
        }
        let initial = run.ordinal == CloseRunOrdinal::INITIAL;
        if request.occurred_at_us < 0
            || request
                .stop_certainty
                .confirmed_at_us()
                .is_some_and(|value| value < 0)
            || request.failure_occurrence_id.trim().is_empty()
        {
            return Err(close_precondition("cleanup failure requires nonnegative timestamps and a nonempty occurrence identity"));
        }
        let (scope, resource, snapshot) = match &request.authority {
            CloseCleanupFailureAuthority::AttemptInterrupted => {
                return Self::terminalize_scope_free_interruption_tx(tx, run, request).await;
            }
            CloseCleanupFailureAuthority::ExpectedResource {
                scope,
                snapshot,
                resource,
            } => (scope, resource, Some(snapshot)),
            CloseCleanupFailureAuthority::CapturedScope { scope, resource }
            | CloseCleanupFailureAuthority::ObservedProcessResource { scope, resource } => {
                if request.stop_certainty != CloseStopCertainty::ShutdownUncertain {
                    return Err(close_precondition(
                        "captured-scope failure authority only permits shutdown-uncertain disposition",
                    ));
                }
                (scope, resource, None)
            }
        };
        let occurred_at = DateTime::from_timestamp_micros(request.occurred_at_us)
            .ok_or_else(|| {
                close_precondition("cleanup failure occurrence timestamp is out of range")
            })?
            .to_rfc3339();
        let obligation = close_obligation_for_update(tx, request.attempt_id.as_str()).await?;
        let identity = resource.identity();
        let captured_scope: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM close_attempt_scopes WHERE attempt_id=?1 AND scope=?2)",
        )
        .bind(request.attempt_id.as_str())
        .bind(scope.as_str())
        .fetch_one(&mut **tx)
        .await?;
        if !captured_scope {
            return Err(close_precondition(
                "cleanup failure scope is not in the captured Close scope set",
            ));
        }
        if let LossItemIdentity::Worktree(worktree) = identity {
            let captured: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM close_attempt_scopes
                 WHERE attempt_id = ?1 AND scope = ?2 AND captured_worktree_identity = ?3
                   AND captured_worktree_fingerprint = ?4 AND captured_worktree_locator = ?5)",
            )
            .bind(request.attempt_id.as_str())
            .bind(scope.as_str())
            .bind(worktree.id().as_str())
            .bind(worktree.fingerprint().as_str())
            .bind(worktree.locator().encode())
            .fetch_one(&mut **tx)
            .await?;
            if !captured {
                return Err(close_precondition(
                    "cleanup failure worktree differs from the complete captured identity",
                ));
            }
        }
        if matches!(
            &request.authority,
            CloseCleanupFailureAuthority::CapturedScope { .. }
        ) && !matches!(identity, LossItemIdentity::Worktree(_))
            && !(resource.kind() == RetiredResourceKind::WorkScope
                && matches!(identity, LossItemIdentity::Opaque(value) if value.as_str() == scope.as_str()))
        {
            return Err(close_precondition(
                "captured-scope failure requires the captured worktree or exact WorkScope identity",
            ));
        }
        if matches!(
            &request.authority,
            CloseCleanupFailureAuthority::ObservedProcessResource { .. }
        ) && !matches!(
            resource.kind(),
            RetiredResourceKind::BashProcessGroup
                | RetiredResourceKind::PtySession
                | RetiredResourceKind::BrowserSession
                | RetiredResourceKind::EquivalentLiveResource
        ) {
            return Err(close_precondition("observed-process failure requires a process-epoch resource; tmux requires expected authority"));
        }
        let captured_scope_authority = matches!(
            &request.authority,
            CloseCleanupFailureAuthority::CapturedScope { .. }
        );
        if close_process_step_succeeded_tx(tx, run, scope, resource).await? {
            return Err(close_precondition(
                "successful process cannot be the failure authority",
            ));
        }
        let mut authority_resource_present = false;
        for (ordinal, remaining) in request.remaining_resources.iter().enumerate() {
            if close_process_step_succeeded_tx(tx, run, &remaining.scope, &remaining.resource)
                .await?
            {
                return Err(close_precondition(
                    "remaining resources cannot contain a successful process",
                ));
            }
            let is_authority_resource =
                &remaining.scope == scope && &remaining.resource == resource;
            let valid_disposition = if is_authority_resource {
                remaining.disposition == CloseCleanupResourceDisposition::Failed
                    || (captured_scope_authority
                        && remaining.disposition == CloseCleanupResourceDisposition::Unknown)
            } else {
                remaining.disposition != CloseCleanupResourceDisposition::Failed
            };
            if !valid_disposition
                || request.remaining_resources[..ordinal].iter().any(|prior| {
                    prior.scope == remaining.scope && prior.resource == remaining.resource
                })
            {
                return Err(close_precondition("remaining resources require distinct exact identities and a matching failed or scope-unknown authority resource"));
            }
            authority_resource_present |= is_authority_resource;
            if snapshot.is_none() {
                validate_cleanup_observed_resource_tx(
                    tx,
                    &request.attempt_id,
                    &remaining.scope,
                    &remaining.resource,
                )
                .await?;
            }
        }
        if !authority_resource_present {
            return Err(close_precondition(
                "remaining resources must include the exact failure authority resource",
            ));
        }
        let existing: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM close_cleanup_failures
             WHERE failure_occurrence_id = ?1 OR (attempt_id = ?2 AND cleanup_run_ordinal = ?3))",
        )
        .bind(&request.failure_occurrence_id)
        .bind(request.attempt_id.as_str())
        .bind(run.ordinal.get())
        .fetch_one(&mut **tx)
        .await?;
        if existing {
            let exact: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM close_cleanup_failures
                 WHERE failure_occurrence_id = ?1 AND attempt_id = ?2 AND cleanup_run_ordinal = ?16
                   AND source_product_conversation_id = ?3 AND scope = ?4
                   AND inspection_generation IS ?5 AND inspection_fingerprint IS ?6
                   AND resource_kind = ?7 AND identity_kind = ?8 AND identity_codec = ?9
                   AND identity_value = ?10 AND reason = ?11 AND detail = ?12
                   AND stop_certainty = ?13 AND confirmed_at_us IS ?14 AND occurred_at_us = ?15
                   AND authority_kind = ?17)",
            )
            .bind(&request.failure_occurrence_id)
            .bind(request.attempt_id.as_str())
            .bind(request.source_product_conversation_id.as_str())
            .bind(scope.as_str())
            .bind(snapshot.map(CloseRetirementSnapshot::generation))
            .bind(snapshot.map(CloseRetirementSnapshot::fingerprint))
            .bind(resource.kind().as_str())
            .bind(identity.identity_kind())
            .bind(identity.codec())
            .bind(identity.value())
            .bind(request.reason.as_str())
            .bind(&request.detail)
            .bind(request.stop_certainty.as_str())
            .bind(request.stop_certainty.confirmed_at_us())
            .bind(request.occurred_at_us)
            .bind(run.ordinal.get())
            .bind(request.authority.as_str())
            .fetch_one(&mut **tx)
            .await?;
            let retained_resources =
                close_cleanup_failure_resources_tx(tx, &request.failure_occurrence_id).await?;
            if !exact
                || retained_resources != request.remaining_resources
                || obligation.phase() != ClosePhase::Completed
                || (initial
                    && obligation.close_outcome()
                        != Some(request.stop_certainty.completion_outcome()))
                || snapshot.is_some_and(|snapshot| obligation.snapshot() != Some(snapshot))
                || obligation.product_conversation_id() != &request.source_product_conversation_id
            {
                return Err(close_precondition(
                    "cleanup failure replay conflicts with the committed occurrence",
                ));
            }
            append_mandatory_close_failure_event_tx(tx, &request.failure_occurrence_id).await?;
            return Ok(obligation);
        }
        if !initial {
            let progress = close_safe_retry_progress_tx(tx, run).await?;
            let failed = CloseSafeRetryEffect {
                scope: scope.clone(),
                resource: resource.clone(),
            };
            let expected = retry_failure_resources_tx(tx, run, &progress, &failed).await?;
            if request.remaining_resources != expected {
                return Err(close_precondition(
                    "retry failure children must equal pending exact-run plan",
                ));
            }
        }
        let running: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM close_runs WHERE attempt_id = ?1 AND run_ordinal = ?2 AND status = 'running')",
        ).bind(run.attempt_id.as_str()).bind(run.ordinal.get()).fetch_one(&mut **tx).await?;
        if !running {
            return Err(close_precondition(
                "cleanup failure requires the exact running Close run",
            ));
        }
        if obligation.product_conversation_id() != &request.source_product_conversation_id {
            return Err(close_precondition(
                "cleanup failure source does not match the active Close attempt",
            ));
        }
        if let Some(snapshot) = snapshot {
            if (initial && obligation.phase() != ClosePhase::RetirementRequested)
                || (!initial
                    && (obligation.phase() != ClosePhase::Completed
                        || !matches!(
                            obligation.close_outcome(),
                            Some(
                                CloseCompletionOutcome::CloseIncomplete
                                    | CloseCompletionOutcome::ArchivedCleanupAttention
                            )
                        )))
                || obligation.snapshot() != Some(snapshot)
            {
                return Err(close_precondition(
                    "expected-resource failure requires the exact active RetirementRequested snapshot",
                ));
            }
        } else if obligation.phase() == ClosePhase::Completed {
            return Err(close_precondition(
                "captured-scope failure requires an active Close attempt",
            ));
        }
        if let Some(snapshot) = snapshot {
            let expected: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM close_expected_retirement_resources resource
                 JOIN close_retirement_inventories inventory
                   ON inventory.attempt_id = resource.attempt_id AND inventory.scope = resource.scope
                  AND inventory.inspection_generation = resource.inspection_generation
                  AND inventory.inspection_fingerprint = resource.inspection_fingerprint
                 WHERE resource.attempt_id = ?1 AND resource.scope = ?2
                   AND resource.inspection_generation = ?3 AND resource.inspection_fingerprint = ?4
                   AND resource.resource_kind = ?5 AND resource.identity_kind = ?6
                   AND resource.identity_codec = ?7 AND resource.identity_value = ?8
                   AND inventory.sealed = 1)",
            )
            .bind(request.attempt_id.as_str())
            .bind(scope.as_str())
            .bind(snapshot.generation())
            .bind(snapshot.fingerprint())
            .bind(resource.kind().as_str())
            .bind(identity.identity_kind())
            .bind(identity.codec())
            .bind(identity.value())
            .fetch_one(&mut **tx)
            .await?;
            if !expected {
                return Err(close_precondition(
                    "cleanup failure resource is not in the exact sealed inventory",
                ));
            }
        }
        if matches!(
            request.stop_certainty,
            CloseStopCertainty::ConversationAndProcessesStopped { .. }
        ) {
            let unresolved_process_resources: i64 = sqlx::query_scalar(
                "SELECT COUNT(*)
                 FROM close_expected_retirement_resources expected
                 LEFT JOIN close_retirement_resources retired
                   ON retired.attempt_id=expected.attempt_id AND retired.scope=expected.scope
                  AND retired.inspection_generation=expected.inspection_generation
                  AND retired.inspection_fingerprint=expected.inspection_fingerprint
                  AND retired.resource_kind=expected.resource_kind
                  AND retired.identity_kind=expected.identity_kind
                  AND retired.identity_codec=expected.identity_codec
                  AND retired.identity_value=expected.identity_value
                 WHERE expected.attempt_id=?1
                   AND expected.resource_kind IN (
                       'bash_process_group', 'tmux_server', 'pty_session',
                       'browser_session', 'equivalent_live_resource'
                   )
                   AND (retired.proof_kind IS NULL OR retired.proof_kind='residual')
                   AND NOT EXISTS (SELECT 1 FROM close_run_retry_effects effect
                       JOIN close_run_retry_successes success ON success.attempt_id = effect.attempt_id
                         AND success.run_ordinal = effect.run_ordinal AND success.ordinal = effect.ordinal
                WHERE effect.attempt_id = expected.attempt_id
                  AND effect.scope = expected.scope AND effect.resource_kind = expected.resource_kind

                         AND effect.identity_kind = expected.identity_kind AND effect.identity_codec = expected.identity_codec
                         AND effect.identity_value = expected.identity_value)",
            )
            .bind(request.attempt_id.as_str())
            .fetch_one(&mut **tx)
            .await?;
            if unresolved_process_resources != 0 {
                return Err(close_precondition(
                    "confirmed Close stop requires successful proof for every process resource",
                ));
            }
        }
        if initial && snapshot.is_some() {
            let expected = expected_close_cleanup_failure_resources_tx(
                tx,
                request.attempt_id.as_str(),
                scope,
                resource,
            )
            .await?;
            if expected
                .iter()
                .any(|target| !request.remaining_resources.contains(target))
            {
                return Err(close_precondition(
                    "remaining resources differ from the complete unresolved expected inventory",
                ));
            }
            for supplied in request.remaining_resources.iter().filter(|supplied| {
                !expected.iter().any(|target| {
                    target.scope == supplied.scope && target.resource == supplied.resource
                })
            }) {
                if !matches!(
                    supplied.resource.kind(),
                    RetiredResourceKind::BashProcessGroup
                        | RetiredResourceKind::PtySession
                        | RetiredResourceKind::BrowserSession
                        | RetiredResourceKind::EquivalentLiveResource
                ) {
                    return Err(close_precondition(
                        "remaining resources expand the complete unresolved expected inventory",
                    ));
                }
                let identity = supplied.resource.identity();
                let already_succeeded: bool = sqlx::query_scalar(
                    "SELECT EXISTS(
                        SELECT 1 FROM close_retirement_resources retired
                        WHERE retired.attempt_id = ?1 AND retired.scope = ?2
                          AND retired.resource_kind = ?3 AND retired.identity_kind = ?4
                          AND retired.identity_codec = ?5 AND retired.identity_value = ?6
                          AND retired.proof_kind IN ('retired', 'absence_adopted')
                        UNION ALL
                        SELECT 1 FROM close_process_step_successes success
                        WHERE success.attempt_id = ?1 AND success.scope = ?2
                          AND success.resource_kind = ?3 AND success.identity_value = ?6
                    )",
                )
                .bind(request.attempt_id.as_str())
                .bind(supplied.scope.as_str())
                .bind(supplied.resource.kind().as_str())
                .bind(identity.identity_kind())
                .bind(identity.codec())
                .bind(identity.value())
                .fetch_one(&mut **tx)
                .await?;
                if already_succeeded {
                    return Err(close_precondition(
                        "remaining resources include an already successful process identity",
                    ));
                }
            }
        }
        if let Some(snapshot) = snapshot.filter(|_| initial) {
            sqlx::query(
                "INSERT INTO close_retirement_resources (
                attempt_id, scope, inspection_generation, inspection_fingerprint,
                resource_kind, identity_kind, identity_codec, identity_value,
                proof_kind, absence_basis, residual_reason, detail, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'residual', NULL, ?9, ?10, ?11, ?11)",
            )
            .bind(request.attempt_id.as_str())
            .bind(scope.as_str())
            .bind(snapshot.generation())
            .bind(snapshot.fingerprint())
            .bind(resource.kind().as_str())
            .bind(identity.identity_kind())
            .bind(identity.codec())
            .bind(identity.value())
            .bind(request.reason.as_str())
            .bind(&request.detail)
            .bind(&occurred_at)
            .execute(&mut **tx)
            .await?;
            sqlx::query(
                "INSERT INTO close_retirement_resource_history (
                attempt_id, scope, inspection_generation, inspection_fingerprint,
                resource_kind, identity_kind, identity_codec, identity_value,
                proof_kind, absence_basis, residual_reason, detail, recorded_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'residual', NULL, ?9, ?10, ?11)",
            )
            .bind(request.attempt_id.as_str())
            .bind(scope.as_str())
            .bind(snapshot.generation())
            .bind(snapshot.fingerprint())
            .bind(resource.kind().as_str())
            .bind(identity.identity_kind())
            .bind(identity.codec())
            .bind(identity.value())
            .bind(request.reason.as_str())
            .bind(&request.detail)
            .bind(&occurred_at)
            .execute(&mut **tx)
            .await?;
        }
        sqlx::query(
            "INSERT INTO close_cleanup_failures (
                failure_occurrence_id, attempt_id, cleanup_run_ordinal,
                source_product_conversation_id, scope, inspection_generation, inspection_fingerprint,
                resource_kind, identity_kind, identity_codec, identity_value, reason, detail,
                stop_certainty, confirmed_at_us, occurred_at_us, authority_kind
             ) VALUES (?1, ?2, ?16, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?17)",
        )
        .bind(&request.failure_occurrence_id).bind(request.attempt_id.as_str())
        .bind(request.source_product_conversation_id.as_str()).bind(scope.as_str())
        .bind(snapshot.map(CloseRetirementSnapshot::generation))
        .bind(snapshot.map(CloseRetirementSnapshot::fingerprint))
        .bind(resource.kind().as_str()).bind(identity.identity_kind())
        .bind(identity.codec()).bind(identity.value()).bind(request.reason.as_str()).bind(&request.detail)
        .bind(request.stop_certainty.as_str()).bind(request.stop_certainty.confirmed_at_us())
        .bind(request.occurred_at_us).bind(run.ordinal.get()).bind(request.authority.as_str()).execute(&mut **tx).await?;
        for (ordinal, remaining) in request.remaining_resources.iter().enumerate() {
            let identity = remaining.resource.identity();
            sqlx::query(
                "INSERT INTO close_cleanup_failure_resources (failure_occurrence_id, ordinal, scope,
                    resource_kind, identity_kind, identity_codec, identity_value, disposition)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            ).bind(&request.failure_occurrence_id)
                .bind(i64::try_from(ordinal).map_err(|error| close_precondition(error.to_string()))?)
                .bind(remaining.scope.as_str()).bind(remaining.resource.kind().as_str())
                .bind(identity.identity_kind()).bind(identity.codec()).bind(identity.value())
                .bind(remaining.disposition.as_str()).execute(&mut **tx).await?;
        }
        Self::finish_close_failure_tx(tx, run, request, obligation).await
    }

    async fn terminalize_scope_free_interruption_tx(
        tx: &mut Transaction<'_, Sqlite>,
        run: &CloseRunRef,
        request: &TerminalizeInitialCloseCleanupFailureRequest,
    ) -> DbResult<CloseObligation> {
        if request.reason != RetirementFailureReason::Interrupted
            || !request.remaining_resources.is_empty()
            || (run.ordinal == CloseRunOrdinal::INITIAL
                && (request.stop_certainty != CloseStopCertainty::ShutdownUncertain
                    || sqlx::query_scalar::<_, i64>(
                        "SELECT COUNT(*) FROM close_attempt_scopes WHERE attempt_id = ?1",
                    )
                    .bind(run.attempt_id.as_str())
                    .fetch_one(&mut **tx)
                    .await?
                        != 0))
        {
            return Err(close_precondition(
                "attempt interruption requires exact resource-free authority",
            ));
        }

        let obligation = close_obligation_for_update(tx, run.attempt_id.as_str()).await?;
        if request.failure_occurrence_id != run.failure_occurrence_id()
            || request.attempt_id != run.attempt_id
            || request.source_product_conversation_id != *obligation.product_conversation_id()
        {
            return Err(close_precondition(
                "attempt interruption must name the exact run and product",
            ));
        }
        let existing = sqlx::query(
            "SELECT authority_kind, detail, occurred_at_us FROM close_cleanup_failures
            WHERE attempt_id = ?1 AND cleanup_run_ordinal = ?2",
        )
        .bind(run.attempt_id.as_str())
        .bind(run.ordinal.get())
        .fetch_optional(&mut **tx)
        .await?;
        if let Some(existing) = existing {
            if (run.ordinal != CloseRunOrdinal::INITIAL
                && !verified_completion_source_tx(tx, run).await?)
                || existing.try_get::<&str, _>("authority_kind")? != "attempt_interrupted"
                || existing.try_get::<&str, _>("detail")? != request.detail
                || existing.try_get::<i64, _>("occurred_at_us")? != request.occurred_at_us
                || obligation.product_conversation_id() != &request.source_product_conversation_id
            {
                return Err(close_precondition(
                    "scope-free failure replay conflicts with retained occurrence",
                ));
            }
            append_mandatory_close_failure_event_tx(tx, &request.failure_occurrence_id).await?;
            return Ok(obligation);
        }
        if run.ordinal != CloseRunOrdinal::INITIAL
            && !verified_completion_running_tx(tx, run).await?
        {
            return Err(close_precondition(
                "resource-free retry failure requires a nonempty fully successful running plan",
            ));
        }
        sqlx::query("INSERT INTO close_cleanup_failures
            (failure_occurrence_id, attempt_id, cleanup_run_ordinal, source_product_conversation_id,
             authority_kind, reason, detail, stop_certainty, occurred_at_us)
            VALUES (?1, ?2, ?3, ?4, 'attempt_interrupted', 'interrupted', ?5, 'shutdown_uncertain', ?6)")
            .bind(&request.failure_occurrence_id).bind(run.attempt_id.as_str()).bind(run.ordinal.get())
            .bind(request.source_product_conversation_id.as_str()).bind(&request.detail).bind(request.occurred_at_us)
            .execute(&mut **tx).await?;
        Self::finish_close_failure_tx(tx, run, request, obligation).await
    }

    async fn finish_close_failure_tx(
        tx: &mut Transaction<'_, Sqlite>,
        run: &CloseRunRef,
        request: &TerminalizeInitialCloseCleanupFailureRequest,
        obligation: CloseObligation,
    ) -> DbResult<CloseObligation> {
        let initial = run.ordinal == CloseRunOrdinal::INITIAL;
        let occurred_at = DateTime::from_timestamp_micros(request.occurred_at_us)
            .ok_or_else(|| close_precondition("failure timestamp out of range"))?
            .to_rfc3339();
        append_mandatory_close_failure_event_tx(tx, &request.failure_occurrence_id).await?;
        let latest: String = sqlx::query_scalar(
            "SELECT conversation_id FROM close_attempt_members
             WHERE attempt_id = ?1 AND member_role IN ('latest', 'root_latest')",
        )
        .bind(request.attempt_id.as_str())
        .fetch_one(&mut **tx)
        .await?;
        let sequence: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(sequence_id), 0) + 1 FROM messages WHERE conversation_id = ?1",
        )
        .bind(&latest)
        .fetch_one(&mut **tx)
        .await?;
        let archived = matches!(
            request.stop_certainty,
            CloseStopCertainty::ConversationAndProcessesStopped { .. }
        );
        let status = if !initial
            && obligation.close_outcome() == Some(CloseCompletionOutcome::ArchivedCleanupAttention)
        {
            "Cleanup retry stopped — conversation remains in History; cleanup needs attention"
        } else if !initial {
            "Cleanup retry stopped — original Close remains incomplete; cleanup needs attention"
        } else if archived {
            "Closed — cleanup needs attention"
        } else {
            "Close incomplete — shutdown uncertain; cleanup needs attention"
        };
        let content = MessageContent::system(format!(
            "{status}. Close attempt {}; failure {} ({}): {}. No automatic cleanup retry will run.",
            request.attempt_id,
            request.failure_occurrence_id,
            request.reason.as_str(),
            request.detail,
        ));
        let content = serde_json::to_string(&content.to_stored_json())
            .map_err(|error| DbError::Serialization(error.to_string()))?;
        sqlx::query(
            "INSERT INTO messages (message_id, conversation_id, sequence_id, message_type, content, created_at)
             VALUES (?1, ?2, ?3, 'system', ?4, ?5)",
        ).bind(if initial { format!("close-outcome:{}", request.attempt_id) }
            else { format!("close-run-outcome:{}:{}", run.ordinal.get(), request.attempt_id) }).bind(&latest).bind(sequence)
        .bind(content).bind(&occurred_at).execute(&mut **tx).await?;
        if !initial {
            return Ok(obligation);
        }
        if archived {
            sqlx::query(
                "UPDATE conversations SET archived = 1, updated_at = ?2
                 WHERE id IN (SELECT conversation_id FROM close_attempt_participants WHERE attempt_id = ?1)",
            ).bind(request.attempt_id.as_str()).bind(&occurred_at).execute(&mut **tx).await?;
            sqlx::query("UPDATE product_conversations SET ordinary_lifecycle = 'history' WHERE id = ?1 AND kind = 'ordinary'")
                .bind(request.source_product_conversation_id.as_str()).execute(&mut **tx).await?;
        }
        let completed = sqlx::query(
            "UPDATE close_obligations SET phase = 'completed', close_outcome = ?2,
                completed_at = ?3, updated_at = ?3 WHERE attempt_id = ?1 AND phase = ?4",
        )
        .bind(request.attempt_id.as_str())
        .bind(request.stop_certainty.completion_outcome().as_str())
        .bind(&occurred_at)
        .bind(obligation.phase().as_str())
        .execute(&mut **tx)
        .await?;
        if completed.rows_affected() != 1 {
            return Err(close_precondition(
                "cleanup failure terminalization lost retirement authority",
            ));
        }
        close_obligation_for_update(tx, request.attempt_id.as_str()).await
    }

    /// Completes one fully retired Close attempt and archives its captured members.
    ///
    /// # Errors
    /// Returns [`DbError`] when the attempt is absent, not retirement-ready, or persistence fails.
    pub async fn complete_close_retirement(
        &self,
        attempt_id: &CloseAttemptId,
    ) -> DbResult<CloseObligation> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let obligation = close_obligation_for_update(&mut tx, attempt_id.as_str()).await?;
        if obligation.phase() == ClosePhase::Completed {
            tx.commit().await?;
            return Ok(obligation);
        }
        if obligation.phase() != ClosePhase::RetirementRequested {
            return Err(close_precondition(format!(
                "attempt {attempt_id} completion requires retirement_requested"
            )));
        }
        let now = Utc::now().to_rfc3339();
        let aggregate_transcript_id: String = sqlx::query_scalar(
            "SELECT conversation_id
             FROM close_attempt_members
             WHERE attempt_id = ?1 AND member_role IN ('latest', 'root_latest')",
        )
        .bind(attempt_id.as_str())
        .fetch_one(&mut *tx)
        .await?;
        let persisted_sequence_max: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(sequence_id), 0) FROM messages WHERE conversation_id = ?1",
        )
        .bind(&aggregate_transcript_id)
        .fetch_one(&mut *tx)
        .await?;
        let outcome_content = MessageContent::system(format!(
            "Close attempt {attempt_id} completed; this conversation is now archived in History."
        ));
        let outcome_content_json = serde_json::to_string(&outcome_content.to_stored_json())
            .map_err(|error| DbError::Serialization(error.to_string()))?;
        sqlx::query(
            "INSERT INTO messages (
                 message_id, conversation_id, sequence_id, message_type, content, created_at
             ) VALUES (?1, ?2, ?3, 'system', ?4, ?5)",
        )
        .bind(format!("close-outcome:{attempt_id}"))
        .bind(&aggregate_transcript_id)
        .bind(persisted_sequence_max + 1)
        .bind(outcome_content_json)
        .bind(&now)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE conversations
             SET archived = 1, updated_at = ?2
             WHERE id IN (
                 SELECT conversation_id FROM close_attempt_participants WHERE attempt_id = ?1
             )",
        )
        .bind(attempt_id.as_str())
        .bind(&now)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE product_conversations
             SET ordinary_lifecycle = 'history'
             WHERE id = (SELECT product_conversation_id FROM close_obligations WHERE attempt_id = ?1)
               AND kind = 'ordinary'",
        )
        .bind(attempt_id.as_str())
        .execute(&mut *tx)
        .await?;
        let completed = sqlx::query(
            "UPDATE close_obligations
             SET phase = 'completed', close_outcome = 'archived',
                 completed_at = ?2, updated_at = ?2
             WHERE attempt_id = ?1 AND phase = 'retirement_requested'",
        )
        .bind(attempt_id.as_str())
        .bind(&now)
        .execute(&mut *tx)
        .await?;
        if completed.rows_affected() != 1 {
            return Err(close_precondition(format!(
                "attempt {attempt_id} completion lost retirement authority"
            )));
        }
        let obligation = close_obligation_for_update(&mut tx, attempt_id.as_str()).await?;
        tx.commit().await?;
        Ok(obligation)
    }

    /// Reopens a legacy `NeedsRepair` phase only; stopped runs require fresh-run admission.
    ///
    /// # Errors
    /// Returns [`DbError`] when the attempt is absent, is not in repair, or persistence fails.
    pub async fn retry_close_retirement(
        &self,
        attempt_id: &CloseAttemptId,
    ) -> DbResult<CloseObligation> {
        let mut tx = self.pool.begin().await?;
        let obligation = close_obligation_for_update(&mut tx, attempt_id.as_str()).await?;
        let legacy_running: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM close_runs run WHERE run.attempt_id = ?1 AND run.run_ordinal = 1
                AND run.status = 'running' AND NOT EXISTS (SELECT 1 FROM close_cleanup_failures failure WHERE failure.attempt_id = run.attempt_id))",
        ).bind(attempt_id.as_str()).fetch_one(&mut *tx).await?;
        if !legacy_running {
            return Err(close_precondition(
                "stopped Close outcomes require explicit fresh-run safe retry admission",
            ));
        }
        if obligation.phase() != ClosePhase::NeedsRepair {
            return Err(close_precondition(format!(
                "attempt {attempt_id} retry requires needs_repair"
            )));
        }
        set_close_phase_tx(
            &mut tx,
            attempt_id.as_str(),
            ClosePhase::AwaitingRetirementInspection,
        )
        .await?;
        let obligation = close_obligation_for_update(&mut tx, attempt_id.as_str()).await?;
        tx.commit().await?;
        Ok(obligation)
    }

    /// Lists retained retirement evidence for an attempt.
    ///
    /// # Errors
    /// Returns [`DbError`] when persistence or decoding fails.
    pub async fn list_close_retirement_evidence(
        &self,
        attempt_id: &str,
    ) -> DbResult<Vec<CloseRetiredResource>> {
        sqlx::query(
            "SELECT resource.attempt_id, resource.scope, resource.inspection_generation,
                    resource.inspection_fingerprint, resource.resource_kind, resource.identity_kind,
                    resource.identity_codec, resource.identity_value,
                    resource.proof_kind, resource.absence_basis, resource.residual_reason, resource.detail,
                    resource.created_at, resource.updated_at,
                    captured.captured_worktree_fingerprint,
                    captured.captured_worktree_locator
             FROM close_retirement_resources resource
             JOIN close_obligations obligation ON obligation.attempt_id = resource.attempt_id
             JOIN close_attempt_scopes captured
               ON captured.attempt_id = resource.attempt_id AND captured.scope = resource.scope
             WHERE resource.attempt_id = ?1
               AND resource.inspection_generation = obligation.inspection_generation
               AND resource.inspection_fingerprint = obligation.inspection_fingerprint
             ORDER BY resource.scope, resource.resource_kind, resource.identity_kind, resource.identity_value",
        )
        .bind(attempt_id)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(parse_close_retired_resource_row)
        .collect()
    }
}

type DbTx<'a> = sqlx::Transaction<'a, sqlx::Sqlite>;

async fn parse_targeted_close_attempt_scopes(
    tx: &mut DbTx<'_>,
    attempt_id: &str,
) -> DbResult<std::collections::BTreeSet<WorkScopeId>> {
    let targeted_scope_rows = sqlx::query(
        "SELECT cas.scope
         FROM close_attempt_scopes cas
         JOIN work_scopes ws ON ws.id = cas.scope
         WHERE cas.attempt_id = ?1
           AND ws.environment_kind = 'allocated_worktree'
         ORDER BY cas.scope",
    )
    .bind(attempt_id)
    .fetch_all(&mut **tx)
    .await?;

    targeted_scope_rows
        .into_iter()
        .map(|row| row.try_get::<String, _>("scope"))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(|scope| {
            WorkScopeId::parse(scope).map_err(|error| {
                DbError::Serialization(format!("invalid close attempt scope: {error}"))
            })
        })
        .collect()
}

impl Database {
    async fn ensure_inspection_replacement_allowed(
        &self,
        tx: &mut DbTx<'_>,
        request: &ReplaceCloseInspectionRequest,
    ) -> DbResult<()> {
        let row = sqlx::query("SELECT phase FROM close_obligations WHERE attempt_id = ?1")
            .bind(request.attempt_id.as_str())
            .fetch_optional(&mut **tx)
            .await?
            .ok_or_else(|| {
                DbError::CloseFoundationNotFound(request.attempt_id.as_str().to_string())
            })?;
        let phase_raw: String = row.try_get("phase")?;
        let phase = ClosePhase::from_db_str(&phase_raw)
            .ok_or_else(|| DbError::Serialization(format!("unknown close phase {phase_raw}")))?;
        if phase != ClosePhase::AwaitingRetirementInspection {
            return Err(close_precondition(format!(
                "attempt {} phase {} does not admit retirement inspection",
                request.attempt_id,
                phase.as_str()
            )));
        }

        let targeted = parse_targeted_close_attempt_scopes(tx, request.attempt_id.as_str()).await?;
        let provided = request
            .scopes
            .iter()
            .map(|scope| scope.scope.clone())
            .collect::<std::collections::BTreeSet<_>>();
        if request.scopes.len() != provided.len() {
            return Err(close_precondition(format!(
                "attempt {} replacement contains duplicate scopes",
                request.attempt_id.as_str()
            )));
        }
        for scope in &request.scopes {
            let unique_losses = scope
                .losses
                .iter()
                .collect::<std::collections::HashSet<_>>();
            if unique_losses.len() != scope.losses.len() {
                return Err(close_precondition(format!(
                    "attempt {} scope {} contains duplicate loss items",
                    request.attempt_id, scope.scope
                )));
            }
        }
        if targeted != provided {
            return Err(close_precondition(format!(
                "attempt {} replacement scopes do not exactly match targeted scopes",
                request.attempt_id.as_str()
            )));
        }
        Ok(())
    }

    async fn clear_retirement_inspection_rows(
        &self,
        tx: &mut DbTx<'_>,
        attempt_id: &str,
    ) -> DbResult<()> {
        sqlx::query("DELETE FROM close_retirement_losses WHERE attempt_id = ?1")
            .bind(attempt_id)
            .execute(&mut **tx)
            .await?;
        sqlx::query("DELETE FROM close_retirement_inspections WHERE attempt_id = ?1")
            .bind(attempt_id)
            .execute(&mut **tx)
            .await?;
        Ok(())
    }

    async fn insert_retirement_inspection_rows(
        &self,
        tx: &mut DbTx<'_>,
        request: &ReplaceCloseInspectionRequest,
    ) -> DbResult<()> {
        let inspected_at = Utc::now().to_rfc3339();
        for scope_request in &request.scopes {
            sqlx::query(
                "INSERT INTO close_retirement_inspections (attempt_id, scope, generation, fingerprint, inspected_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )
            .bind(request.attempt_id.as_str())
            .bind(scope_request.scope.as_str())
            .bind(scope_request.snapshot.generation())
            .bind(scope_request.snapshot.fingerprint())
            .bind(&inspected_at)
            .execute(&mut **tx)
            .await?;

            for item in &scope_request.losses {
                let identity = item.identity();
                sqlx::query(
                    "INSERT INTO close_retirement_losses (
                        attempt_id, scope, generation, category, identity_kind, identity_codec, identity_value
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                )
                .bind(request.attempt_id.as_str())
                .bind(scope_request.scope.as_str())
                .bind(scope_request.snapshot.generation())
                .bind(item.category().as_str())
                .bind(identity.identity_kind())
                .bind(identity.codec())
                .bind(identity.value())
                .execute(&mut **tx)
                .await?;
            }
        }
        Ok(())
    }

    async fn advance_obligation_after_inspection(
        &self,
        tx: &mut DbTx<'_>,
        request: &ReplaceCloseInspectionRequest,
        aggregate_snapshot: &CloseRetirementSnapshot,
    ) -> DbResult<()> {
        let phase = if request.scopes.iter().any(|scope| !scope.losses.is_empty()) {
            ClosePhase::AwaitingLossConfirmation
        } else {
            ClosePhase::RetirementRequested
        };
        sqlx::query(
            "UPDATE close_obligations
             SET phase = ?2, inspection_generation = ?3, inspection_fingerprint = ?4
             WHERE attempt_id = ?1",
        )
        .bind(request.attempt_id.as_str())
        .bind(phase.as_str())
        .bind(aggregate_snapshot.generation())
        .bind(aggregate_snapshot.fingerprint())
        .execute(&mut **tx)
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use phoenix_core::domain::close::{
        AbsenceBasis, CloseLossItem, CloseRetirementSnapshot, GitOidIdentity, GitPathIdentity,
        LossItemIdentity, OpaqueIdentity, RetiredResourceKind, RetirementFailureReason,
        RetirementOutcome,
    };
    use phoenix_core::domain::db_schema::{ConversationCreationPhase, ErrorKind};
    use phoenix_core::domain::llm_types::ContentBlock;
    use phoenix_core::domain::sm_state::{
        AssistantMessage, ContinuationSummaryRequest, RecoverableContinuationFailure, ToolCall,
        ToolInput,
    };

    fn product_id(id: &str) -> ProductConversationId {
        ProductConversationId::parse(id.to_string()).unwrap()
    }

    fn transcript_id(id: &str) -> TranscriptConversationId {
        TranscriptConversationId::parse(id.to_string()).unwrap()
    }

    async fn create_root(db: &Database, id: &str) {
        let mut conversation = db
            .create_conversation(id, id, "/tmp", true, None, None)
            .await
            .unwrap();
        let allocated_product_id = conversation.product_conversation_id.clone();
        sqlx::query("DELETE FROM conversations WHERE id = ?1")
            .bind(id)
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query("DELETE FROM product_conversations WHERE id = ?1")
            .bind(allocated_product_id.as_str())
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO product_conversations (id, kind, ordinary_lifecycle)
             VALUES (?1, 'ordinary', 'open')",
        )
        .bind(id)
        .execute(db.pool())
        .await
        .unwrap();
        conversation.product_conversation_id = product_id(id);
        let mut tx = db.pool().begin().await.unwrap();
        crate::insert_conversation_tx(&mut tx, &conversation)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }

    async fn create_child(db: &Database, id: &str, parent_id: &str) {
        let parent = db.get_conversation(parent_id).await.unwrap();
        let mut conversation = db
            .create_conversation(id, id, "/tmp", true, None, None)
            .await
            .unwrap();
        let allocated_product_id = conversation.product_conversation_id.clone();
        sqlx::query("DELETE FROM conversations WHERE id = ?1")
            .bind(id)
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query("DELETE FROM product_conversations WHERE id = ?1")
            .bind(allocated_product_id.as_str())
            .execute(db.pool())
            .await
            .unwrap();
        conversation.product_conversation_id = parent.product_conversation_id;
        let mut tx = db.pool().begin().await.unwrap();
        sqlx::query("PRAGMA defer_foreign_keys = ON")
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO product_continuation_reservations (
                 predecessor_conversation_id, successor_conversation_id,
                 product_conversation_id
             ) VALUES (?1, ?2, ?3)",
        )
        .bind(parent_id)
        .bind(id)
        .bind(conversation.product_conversation_id.as_str())
        .execute(&mut *tx)
        .await
        .unwrap();
        sqlx::query("UPDATE conversations SET continued_in_conv_id = ?1 WHERE id = ?2")
            .bind(id)
            .bind(parent_id)
            .execute(&mut *tx)
            .await
            .unwrap();
        crate::insert_conversation_tx(&mut tx, &conversation)
            .await
            .unwrap();
        sqlx::query(
            "DELETE FROM product_continuation_reservations
             WHERE predecessor_conversation_id = ?1",
        )
        .bind(parent_id)
        .execute(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }

    async fn allocate_scope_worktree(db: &Database, conversation_id: &str) -> WorkScopeId {
        let scope = db
            .get_conversation(conversation_id)
            .await
            .unwrap()
            .attached_work_scope_id
            .unwrap();

        sqlx::query(
            "UPDATE work_scopes
             SET environment_kind = 'allocated_worktree',
                 cwd = '/tmp',
                 worktree_path = '/tmp/worktree',
                 worktree_id = lower(hex(randomblob(16))),
                 worktree_fingerprint = lower(hex(randomblob(32))),
                 branch_name = 'branch',
                 base_branch = 'main'
             WHERE id = ?1",
        )
        .bind(scope.as_str())
        .execute(db.pool())
        .await
        .unwrap();

        scope
    }

    async fn set_state(db: &Database, id: &str, state: ConvState) {
        db.update_conversation_state(id, &state).await.unwrap();
    }

    async fn set_archived(db: &Database, id: &str, archived: bool) {
        sqlx::query("UPDATE conversations SET archived = ?1 WHERE id = ?2")
            .bind(archived)
            .bind(id)
            .execute(db.pool())
            .await
            .unwrap();
    }

    async fn set_user_initiated(db: &Database, id: &str, user_initiated: bool) {
        sqlx::query("UPDATE conversations SET user_initiated = ?1 WHERE id = ?2")
            .bind(user_initiated)
            .bind(id)
            .execute(db.pool())
            .await
            .unwrap();
    }

    async fn insert_scope_inspection(
        db: &Database,
        attempt_id: &str,
        scope: &WorkScopeId,
        snapshot: &CloseRetirementSnapshot,
    ) {
        sqlx::query(
            "INSERT INTO close_retirement_inspections (attempt_id, scope, generation, fingerprint, inspected_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .bind(attempt_id)
        .bind(scope.as_str())
        .bind(snapshot.generation())
        .bind(snapshot.fingerprint())
        .bind(Utc::now().to_rfc3339())
        .execute(db.pool())
        .await
        .unwrap();
    }

    async fn set_obligation_snapshot(
        db: &Database,
        attempt_id: &str,
        snapshot: &CloseRetirementSnapshot,
    ) {
        sqlx::query(
            "UPDATE close_obligations
             SET inspection_generation = ?2, inspection_fingerprint = ?3
             WHERE attempt_id = ?1",
        )
        .bind(attempt_id)
        .bind(snapshot.generation())
        .bind(snapshot.fingerprint())
        .execute(db.pool())
        .await
        .unwrap();
    }

    async fn current_test_worktree(db: &Database, scope: &WorkScopeId) -> WorktreeIdentity {
        let (id, fingerprint, locator): (String, String, String) = sqlx::query_as(
            "SELECT worktree_id, worktree_fingerprint,
                    'git_path_bytes_hex_v1:' || lower(hex(CAST(worktree_path AS BLOB)))
             FROM work_scopes WHERE id = ?1 AND environment_kind = 'allocated_worktree'",
        )
        .bind(scope.as_str())
        .fetch_one(db.pool())
        .await
        .unwrap();
        WorktreeIdentity::from_parts(
            phoenix_core::domain::close::WorktreeId::parse(id).unwrap(),
            phoenix_core::domain::close::WorktreeFingerprint::parse(fingerprint).unwrap(),
            GitPathIdentity::decode_exact(&locator).unwrap(),
        )
    }

    #[allow(clippy::too_many_lines)]
    async fn capture_test_inventory(
        db: &Database,
        attempt_id: &str,
        scope: &WorkScopeId,
        snapshot: &CloseRetirementSnapshot,
        resources: Vec<RetiredResourceIdentity>,
    ) {
        let mut inventory = CloseOwnedResourceInventory {
            worktree: None,
            work_scopes: std::collections::BTreeSet::default(),
            bash_process_groups: std::collections::BTreeSet::default(),
            tmux_servers: std::collections::BTreeSet::default(),
            pty_sessions: std::collections::BTreeSet::default(),
            browser_sessions: std::collections::BTreeSet::default(),
            equivalent_live_resources: std::collections::BTreeSet::default(),
        };
        for resource in resources {
            let identity = match resource.identity() {
                LossItemIdentity::Opaque(identity) => identity.clone(),
                LossItemIdentity::Worktree(identity) => {
                    inventory.worktree = Some(identity.clone());
                    continue;
                }
                LossItemIdentity::GitPath(_) | LossItemIdentity::GitOid(_) => unreachable!(),
            };
            match resource.kind() {
                RetiredResourceKind::WorkScope => {
                    inventory.work_scopes.insert(identity);
                }
                RetiredResourceKind::BashProcessGroup => {
                    inventory.bash_process_groups.insert(identity);
                }
                RetiredResourceKind::TmuxServer => {
                    inventory.tmux_servers.insert(identity);
                }
                RetiredResourceKind::PtySession => {
                    inventory.pty_sessions.insert(identity);
                }
                RetiredResourceKind::BrowserSession => {
                    inventory.browser_sessions.insert(identity);
                }
                RetiredResourceKind::EquivalentLiveResource => {
                    inventory.equivalent_live_resources.insert(identity);
                }
                RetiredResourceKind::Worktree => unreachable!(),
            }
        }
        let target_scopes = sqlx::query_scalar::<_, String>(
            "SELECT scope FROM close_attempt_scopes WHERE attempt_id = ?1 ORDER BY scope",
        )
        .bind(attempt_id)
        .fetch_all(db.pool())
        .await
        .unwrap();
        let mut scopes = Vec::new();
        for target_scope in target_scopes {
            let target_scope = WorkScopeId::parse(target_scope).unwrap();
            let mut target_inventory = if &target_scope == scope {
                inventory.clone()
            } else {
                CloseOwnedResourceInventory {
                    worktree: None,
                    work_scopes: std::collections::BTreeSet::default(),
                    bash_process_groups: std::collections::BTreeSet::default(),
                    tmux_servers: std::collections::BTreeSet::default(),
                    pty_sessions: std::collections::BTreeSet::default(),
                    browser_sessions: std::collections::BTreeSet::default(),
                    equivalent_live_resources: std::collections::BTreeSet::default(),
                }
            };
            if target_inventory.worktree.is_none() {
                let worktree_identity =
                    sqlx::query_as::<_, (Option<String>, Option<String>, Option<String>)>(
                        "SELECT worktree_id, worktree_fingerprint,
                            CASE WHEN environment_kind = 'allocated_worktree' THEN
                                'git_path_bytes_hex_v1:' || lower(hex(CAST(worktree_path AS BLOB)))
                            END
                     FROM work_scopes WHERE id = ?1",
                    )
                    .bind(target_scope.as_str())
                    .fetch_one(db.pool())
                    .await
                    .unwrap();
                target_inventory.worktree = match worktree_identity {
                    (Some(id), Some(fingerprint), Some(locator)) => {
                        Some(WorktreeIdentity::from_parts(
                            phoenix_core::domain::close::WorktreeId::parse(id).unwrap(),
                            phoenix_core::domain::close::WorktreeFingerprint::parse(fingerprint)
                                .unwrap(),
                            GitPathIdentity::decode_exact(&locator).unwrap(),
                        ))
                    }
                    (None, None, None) => None,
                    _ => panic!("partial worktree identity"),
                };
            }
            scopes.push(CaptureCloseRetirementInventoryScopeRequest {
                scope: target_scope,
                inventory: target_inventory,
            });
        }
        db.capture_close_retirement_inventory(CaptureCloseRetirementInventoryRequest {
            attempt_id: CloseAttemptId::parse(attempt_id).unwrap(),
            snapshot: snapshot.clone(),
            scopes,
        })
        .await
        .unwrap();
    }

    async fn current_test_snapshot(db: &Database, attempt_id: &str) -> CloseRetirementSnapshot {
        db.get_close_obligation(attempt_id)
            .await
            .unwrap()
            .snapshot()
            .unwrap()
            .clone()
    }

    #[allow(clippy::too_many_lines)]
    async fn set_close_phase(db: &Database, attempt_id: &str, phase: ClosePhase) {
        let current_raw: String =
            sqlx::query_scalar("SELECT phase FROM close_obligations WHERE attempt_id = ?1")
                .bind(attempt_id)
                .fetch_one(db.pool())
                .await
                .unwrap();
        let current = ClosePhase::from_db_str(&current_raw).unwrap();
        if current != phase && !current.can_transition_to(phase) {
            let predecessor = match phase {
                ClosePhase::NeedsRepair => ClosePhase::RetirementRequested,
                ClosePhase::RetirementRequested => ClosePhase::AwaitingRetirementInspection,
                ClosePhase::AwaitingRetirementInspection => ClosePhase::SettlingActiveWork,
                ClosePhase::AwaitingBlockerResolution
                | ClosePhase::AwaitingStopWorkConfirmation
                | ClosePhase::SettlingActiveWork
                | ClosePhase::CancelRequestedDuringSettlement
                | ClosePhase::AwaitingLossConfirmation
                | ClosePhase::Completed => {
                    panic!("test helper cannot route {current:?} to {phase:?}")
                }
            };
            Box::pin(set_close_phase(db, attempt_id, predecessor)).await;
            Box::pin(set_close_phase(db, attempt_id, phase)).await;
            return;
        }
        if current == ClosePhase::AwaitingRetirementInspection
            && matches!(
                phase,
                ClosePhase::AwaitingLossConfirmation
                    | ClosePhase::RetirementRequested
                    | ClosePhase::NeedsRepair
                    | ClosePhase::Completed
            )
        {
            sqlx::query(
                "INSERT INTO close_retirement_inspections (
                    attempt_id, scope, generation, fingerprint, inspected_at
                 )
                 SELECT target.attempt_id, target.scope, 'test-gen', 'test-fp', ?2
                 FROM close_attempt_scopes target
                 JOIN work_scopes scope ON scope.id = target.scope
                 WHERE target.attempt_id = ?1
                   AND scope.environment_kind = 'allocated_worktree'
                 ON CONFLICT(attempt_id, scope) DO NOTHING",
            )
            .bind(attempt_id)
            .bind(Utc::now().to_rfc3339())
            .execute(db.pool())
            .await
            .unwrap();
        }
        let inspection_rows = sqlx::query_as::<_, (String, String, String)>(
            "SELECT scope, generation, fingerprint
             FROM close_retirement_inspections
             WHERE attempt_id = ?1 ORDER BY scope",
        )
        .bind(attempt_id)
        .fetch_all(db.pool())
        .await
        .unwrap();
        let scopes = inspection_rows
            .iter()
            .map(|(scope, _, _)| WorkScopeId::parse(scope).unwrap())
            .collect::<Vec<_>>();
        let aggregate_generation = encode_aggregate_snapshot_component(
            scopes
                .iter()
                .zip(&inspection_rows)
                .map(|(scope, (_, generation, _))| (scope, generation.as_str())),
        );
        let aggregate_fingerprint = encode_aggregate_snapshot_component(
            scopes
                .iter()
                .zip(&inspection_rows)
                .map(|(scope, (_, _, fingerprint))| (scope, fingerprint.as_str())),
        );
        let (generation, fingerprint, completed_at) = match phase {
            ClosePhase::AwaitingLossConfirmation
            | ClosePhase::RetirementRequested
            | ClosePhase::NeedsRepair => (
                Some(aggregate_generation),
                Some(aggregate_fingerprint),
                None,
            ),
            ClosePhase::Completed => (
                Some(aggregate_generation),
                Some(aggregate_fingerprint),
                Some(Utc::now().to_rfc3339()),
            ),
            ClosePhase::AwaitingBlockerResolution
            | ClosePhase::AwaitingStopWorkConfirmation
            | ClosePhase::SettlingActiveWork
            | ClosePhase::CancelRequestedDuringSettlement
            | ClosePhase::AwaitingRetirementInspection => (None, None, None),
        };
        let has_sealed_inventory: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                 SELECT 1 FROM close_retirement_inventories
                 WHERE attempt_id = ?1 AND sealed = 1
               )",
        )
        .bind(attempt_id)
        .fetch_one(db.pool())
        .await
        .unwrap();
        if phase == ClosePhase::Completed && !has_sealed_inventory {
            let generation = generation.as_deref().unwrap();
            let fingerprint = fingerprint.as_deref().unwrap();
            sqlx::query(
                "INSERT INTO close_retirement_inventories (
                     attempt_id, scope, inspection_generation, inspection_fingerprint, sealed, captured_at
                 )
                 SELECT target.attempt_id, target.scope, ?2, ?3, 0, ?4
                 FROM close_attempt_scopes target
                 WHERE target.attempt_id = ?1
                   AND NOT EXISTS (
                       SELECT 1 FROM close_retirement_inventories existing
                       WHERE existing.attempt_id = target.attempt_id
                         AND existing.scope = target.scope
                         AND existing.inspection_generation = ?2
                         AND existing.inspection_fingerprint = ?3
                   )",
            )
            .bind(attempt_id)
            .bind(generation)
            .bind(fingerprint)
            .bind(Utc::now().to_rfc3339())
            .execute(db.pool())
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO close_expected_retirement_resources (
                     attempt_id, scope, inspection_generation, inspection_fingerprint,
                     resource_kind, identity_kind, identity_codec, identity_value
                 )
                 SELECT target.attempt_id, target.scope, ?2, ?3, 'worktree', 'worktree',
                        'worktree_id_v1', target.captured_worktree_identity
                 FROM close_attempt_scopes target
                 JOIN work_scopes scope ON scope.id = target.scope
                 WHERE target.attempt_id = ?1 AND scope.environment_kind = 'allocated_worktree'
                   AND NOT EXISTS (
                       SELECT 1 FROM close_expected_retirement_resources existing
                       WHERE existing.attempt_id = target.attempt_id
                         AND existing.scope = target.scope
                         AND existing.inspection_generation = ?2
                         AND existing.inspection_fingerprint = ?3
                         AND existing.resource_kind = 'worktree'
                   )",
            )
            .bind(attempt_id)
            .bind(generation)
            .bind(fingerprint)
            .execute(db.pool())
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO close_expected_retirement_resources (
                     attempt_id, scope, inspection_generation, inspection_fingerprint,
                     resource_kind, identity_kind, identity_codec, identity_value
                 )
                 SELECT target.attempt_id, target.scope, ?2, ?3, 'work_scope', 'opaque',
                        'opaque_string_v1', target.scope
                 FROM close_attempt_scopes target
                 WHERE target.attempt_id = ?1",
            )
            .bind(attempt_id)
            .bind(generation)
            .bind(fingerprint)
            .execute(db.pool())
            .await
            .unwrap();
            sqlx::query(
                "UPDATE close_retirement_inventories SET sealed = 1
                 WHERE attempt_id = ?1 AND inspection_generation = ?2
                   AND inspection_fingerprint = ?3 AND sealed = 0",
            )
            .bind(attempt_id)
            .bind(generation)
            .bind(fingerprint)
            .execute(db.pool())
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO close_retirement_resources (
                     attempt_id, scope, inspection_generation, inspection_fingerprint,
                     resource_kind, identity_kind, identity_codec, identity_value,
                     proof_kind, residual_reason, created_at, updated_at
                 )
                 SELECT target.attempt_id, target.scope, ?2, ?3, 'worktree', 'worktree',
                        'worktree_id_v1', target.captured_worktree_identity,
                        ?5, ?6, ?4, ?4
                 FROM close_attempt_scopes target
                 JOIN work_scopes scope ON scope.id = target.scope
                 WHERE target.attempt_id = ?1 AND scope.environment_kind = 'allocated_worktree'
                   AND NOT EXISTS (
                       SELECT 1 FROM close_retirement_resources existing
                       WHERE existing.attempt_id = target.attempt_id
                         AND existing.scope = target.scope
                         AND existing.inspection_generation = ?2
                         AND existing.inspection_fingerprint = ?3
                         AND existing.resource_kind = 'worktree'
                   )",
            )
            .bind(attempt_id)
            .bind(generation)
            .bind(fingerprint)
            .bind(Utc::now().to_rfc3339())
            .bind(if phase == ClosePhase::NeedsRepair {
                "residual"
            } else {
                "retired"
            })
            .bind((phase == ClosePhase::NeedsRepair).then_some("manual_repair_required"))
            .execute(db.pool())
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO close_retirement_resources (
                     attempt_id, scope, inspection_generation, inspection_fingerprint,
                     resource_kind, identity_kind, identity_codec, identity_value,
                     proof_kind, created_at, updated_at
                 )
                 SELECT target.attempt_id, target.scope, ?2, ?3, 'work_scope', 'opaque',
                        'opaque_string_v1', target.scope, 'retired', ?4, ?4
                 FROM close_attempt_scopes target
                 WHERE target.attempt_id = ?1",
            )
            .bind(attempt_id)
            .bind(generation)
            .bind(fingerprint)
            .bind(Utc::now().to_rfc3339())
            .execute(db.pool())
            .await
            .unwrap();
            sqlx::query(
                "UPDATE close_obligations SET phase = 'completed', close_outcome = 'archived'
                 WHERE attempt_id = ?1",
            )
            .bind(attempt_id)
            .execute(db.pool())
            .await
            .unwrap();
            sqlx::query(
                "UPDATE conversations SET archived = 1
                 WHERE id IN (
                     SELECT conversation_id FROM close_attempt_members WHERE attempt_id = ?1
                 )",
            )
            .bind(attempt_id)
            .execute(db.pool())
            .await
            .unwrap();
        }
        if phase == ClosePhase::Completed {
            let snapshot = CloseRetirementSnapshot::parse(
                generation.as_deref().unwrap(),
                fingerprint.as_deref().unwrap(),
            )
            .unwrap();
            let recorded = db.list_close_retirement_evidence(attempt_id).await.unwrap();
            for expected in db
                .list_close_expected_retirement_resources(attempt_id)
                .await
                .unwrap()
            {
                if recorded.iter().any(|evidence| {
                    evidence.scope == expected.scope && evidence.resource == expected.resource
                }) {
                    continue;
                }
                db.record_close_retirement_evidence(RecordCloseRetirementEvidenceRequest {
                    attempt_id: CloseAttemptId::parse(attempt_id).unwrap(),
                    snapshot: snapshot.clone(),
                    scope: expected.scope,
                    resource: expected.resource,
                    outcome: RetirementOutcome::Retired,
                    detail: Some("test evidence".to_string()),
                })
                .await
                .unwrap();
            }
        }
        sqlx::query(
            "UPDATE close_obligations
             SET phase = ?2, inspection_generation = ?3, inspection_fingerprint = ?4,
                 completed_at = ?5, close_outcome = ?6
             WHERE attempt_id = ?1",
        )
        .bind(attempt_id)
        .bind(phase.as_str())
        .bind(generation)
        .bind(fingerprint)
        .bind(completed_at)
        .bind((phase == ClosePhase::Completed).then_some("archived"))
        .execute(db.pool())
        .await
        .unwrap();
    }

    fn approval_state() -> ConvState {
        ConvState::AwaitingTaskApproval {
            task_file: "tasks/00001-p1-ready--x.md".to_string(),
            title: "t".to_string(),
            priority: phoenix_core::task_source::Priority::P1,
            plan: "p".to_string(),
        }
    }

    fn awaiting_continuation_state() -> ConvState {
        ConvState::AwaitingContinuation {
            request: ContinuationSummaryRequest {
                operation_id: "op-1".to_string(),
                rejected_tool_calls: Vec::new(),
                attempt: 1,
            },
        }
    }

    fn recoverable_failure_state() -> ConvState {
        ConvState::RecoverableContinuationFailure {
            failure: RecoverableContinuationFailure {
                request: ContinuationSummaryRequest {
                    operation_id: "op-2".to_string(),
                    rejected_tool_calls: Vec::new(),
                    attempt: 1,
                },
                error_kind: ErrorKind::ServerError,
                message: "broken".to_string(),
            },
        }
    }

    #[tokio::test]
    async fn begin_close_rejects_history_without_creating_second_attempt() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "history-retry").await;
        sqlx::query(
            "UPDATE product_conversations SET ordinary_lifecycle = 'history' WHERE id = 'history-retry'",
        )
        .execute(db.pool())
        .await
        .unwrap();

        let error = db
            .begin_close_foundation(
                &product_id("history-retry"),
                &transcript_id("history-retry"),
                "history-retry-attempt",
            )
            .await
            .unwrap_err();
        assert!(matches!(error, DbError::CloseFoundationConflict(_)));
        let attempts: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM close_obligations WHERE product_conversation_id = 'history-retry'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(attempts, 0);
    }

    #[tokio::test]
    async fn participant_snapshot_rejects_cross_product_identity() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "participant-a").await;
        create_root(&db, "participant-b").await;
        let attempt = db
            .begin_close_foundation(
                &product_id("participant-a"),
                &transcript_id("participant-a"),
                "participant-attempt",
            )
            .await
            .unwrap();

        let error = sqlx::query(
            "INSERT INTO close_attempt_participants (
                 attempt_id, product_conversation_id, conversation_id, captured_at_unix_micros
             ) VALUES (?1, ?2, ?3, 0)",
        )
        .bind(attempt.attempt_id().as_str())
        .bind(product_id("participant-a").as_str())
        .bind("participant-b")
        .execute(db.pool())
        .await
        .expect_err("cross-product participant identity must be rejected");
        assert!(error
            .to_string()
            .contains("close participant must belong to the attempted ProductConversation"));
    }

    #[tokio::test]
    async fn completed_close_outcome_controls_new_participant_admission() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "completed-history").await;
        create_root(&db, "completed-cancelled").await;
        sqlx::query(
            "UPDATE product_conversations SET ordinary_lifecycle = 'history'
             WHERE id = 'completed-history'",
        )
        .execute(db.pool())
        .await
        .unwrap();
        let history_parent = db.get_conversation("completed-history").await.unwrap();
        let history_error = db
            .create_subagent_conversation(
                "history-child",
                "history-child",
                "/tmp",
                "completed-history",
                "test-model",
                &crate::ConvMode::Direct,
                phoenix_core::llm_language::LlmLanguage::default(),
                history_parent.attached_work_scope_id.as_ref(),
                crate::SubAgentExecution {
                    connection: "mock",
                    effort: None,
                    persona: None,
                },
            )
            .await
            .expect_err("History aggregate must reject late participants");
        assert!(
            history_error
                .to_string()
                .contains("non-writable ProductConversation"),
            "unexpected error: {history_error:?}"
        );
        let cancelled_parent = db.get_conversation("completed-cancelled").await.unwrap();
        db.create_subagent_conversation(
            "cancelled-child",
            "cancelled-child",
            "/tmp",
            "completed-cancelled",
            "test-model",
            &crate::ConvMode::Direct,
            phoenix_core::llm_language::LlmLanguage::default(),
            cancelled_parent.attached_work_scope_id.as_ref(),
            crate::SubAgentExecution {
                connection: "mock",
                effort: None,
                persona: None,
            },
        )
        .await
        .expect("completed-cancelled Open aggregate remains writable");
    }

    #[tokio::test]
    async fn active_participant_cannot_delete_but_completed_close_cascades_cleanup() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "delete-sealed").await;
        db.begin_close_foundation(
            &product_id("delete-sealed"),
            &transcript_id("delete-sealed"),
            "delete-sealed-attempt",
        )
        .await
        .unwrap();

        let error = sqlx::query("DELETE FROM conversations WHERE id = 'delete-sealed'")
            .execute(db.pool())
            .await
            .expect_err("active sealed participant must be undeletable");
        assert!(error
            .to_string()
            .contains("active Close rejects sealed participant deletion"));
        sqlx::query(
            "UPDATE close_obligations
             SET phase = 'completed', completed_at = updated_at, close_outcome = 'cancelled'
             WHERE attempt_id = 'delete-sealed-attempt'",
        )
        .execute(db.pool())
        .await
        .unwrap();
        sqlx::query("DELETE FROM conversations WHERE id = 'delete-sealed'")
            .execute(db.pool())
            .await
            .unwrap();
        let participant: (i64, String) = sqlx::query_as(
            "SELECT COUNT(*), MIN(settlement_state) FROM close_attempt_participants
             WHERE attempt_id = 'delete-sealed-attempt'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(participant, (1, "live".to_string()));
        sqlx::query("DELETE FROM close_attempt_members WHERE attempt_id = 'delete-sealed-attempt'")
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query(
            "DELETE FROM close_attempt_participants WHERE attempt_id = 'delete-sealed-attempt'",
        )
        .execute(db.pool())
        .await
        .unwrap();
        let participants: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM close_attempt_participants
             WHERE attempt_id = 'delete-sealed-attempt'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(participants, 0);
    }

    #[tokio::test]
    async fn concurrent_begin_returns_domain_conflict_after_immediate_lock() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("close-race.db");
        let db = Database::open(path.to_str().unwrap()).await.unwrap();
        crate::migrations::run_pending_migrations(db.pool())
            .await
            .unwrap();
        create_root(&db, "race-root").await;
        set_state(&db, "race-root", ConvState::Idle).await;

        let mut blocker = db.pool().acquire().await.unwrap();
        sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO close_obligations
             (attempt_id, product_conversation_id, phase, created_at, updated_at)
             VALUES ('winner', 'race-root', 'awaiting_blocker_resolution', ?1, ?1)",
        )
        .bind(&now)
        .execute(&mut *blocker)
        .await
        .unwrap();

        let contender_db = db.clone();
        let contender = tokio::spawn(async move {
            contender_db
                .begin_close_foundation(
                    &product_id("race-root"),
                    &transcript_id("race-root"),
                    "loser",
                )
                .await
        });
        tokio::task::yield_now().await;
        sqlx::query("COMMIT").execute(&mut *blocker).await.unwrap();

        let error = contender.await.unwrap().unwrap_err();
        assert!(matches!(error, DbError::CloseFoundationConflict(_)));
    }

    #[tokio::test]
    async fn fresh_distinct_product_and_transcript_ids_admit_close() {
        let db = Database::open_in_memory().await.unwrap();
        let conversation = db
            .create_conversation(
                "fresh-transcript",
                "fresh-transcript",
                "/tmp",
                true,
                None,
                None,
            )
            .await
            .unwrap();
        assert_ne!(
            conversation.id,
            conversation.product_conversation_id.as_str()
        );

        let obligation = db
            .begin_close_foundation(
                &conversation.product_conversation_id,
                &transcript_id(&conversation.id),
                "fresh-attempt",
            )
            .await
            .unwrap();
        assert_eq!(
            obligation.product_conversation_id(),
            &conversation.product_conversation_id
        );
        let members: Vec<String> = sqlx::query_scalar(
            "SELECT conversation_id FROM close_attempt_members
             WHERE attempt_id = 'fresh-attempt' ORDER BY continuation_ordinal",
        )
        .fetch_all(db.pool())
        .await
        .unwrap();
        assert_eq!(members, vec![conversation.id]);
    }

    #[tokio::test]
    async fn idle_close_enters_settlement_without_stop_work_confirmation() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        db.begin_close_foundation(
            &product_id("root"),
            &transcript_id("root"),
            "attempt-idle-settlement",
        )
        .await
        .unwrap();

        let obligation = db
            .begin_close_idle_settlement("attempt-idle-settlement")
            .await
            .unwrap();
        assert_eq!(obligation.phase(), ClosePhase::SettlingActiveWork);
    }

    #[tokio::test]
    async fn active_work_settlement_advances_only_once_when_captured_members_are_quiescent() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        create_child(&db, "latest", "root").await;
        db.begin_close_foundation(
            &product_id("root"),
            &transcript_id("latest"),
            "attempt-settlement",
        )
        .await
        .unwrap();

        assert_eq!(
            db.confirm_close_stop_work("attempt-settlement")
                .await
                .unwrap()
                .phase(),
            ClosePhase::AwaitingStopWorkConfirmation
        );
        assert_eq!(
            db.begin_close_active_work_settlement("attempt-settlement")
                .await
                .unwrap()
                .phase(),
            ClosePhase::SettlingActiveWork
        );
        assert_eq!(
            db.advance_close_settlement_when_quiescent("attempt-settlement")
                .await
                .unwrap()
                .phase(),
            ClosePhase::AwaitingRetirementInspection
        );
        assert_eq!(
            db.advance_close_settlement_when_quiescent("attempt-settlement")
                .await
                .unwrap()
                .phase(),
            ClosePhase::AwaitingRetirementInspection
        );
        assert!(matches!(
            db.begin_close_active_work_settlement("attempt-settlement")
                .await
                .unwrap_err(),
            DbError::CloseFoundationPrecondition(_)
        ));
    }

    #[tokio::test]
    async fn active_close_rejects_new_aggregate_participant() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        db.begin_close_foundation(
            &product_id("root"),
            &transcript_id("root"),
            "attempt-seals-participants",
        )
        .await
        .unwrap();

        let error = db
            .create_subagent_conversation(
                "late-subordinate",
                "late-subordinate",
                "/tmp",
                "root",
                "test-model",
                &crate::ConvMode::Direct,
                phoenix_core::llm_language::LlmLanguage::default(),
                db.get_conversation("root")
                    .await
                    .unwrap()
                    .attached_work_scope_id
                    .as_ref(),
                crate::SubAgentExecution {
                    connection: "mock",
                    effort: None,
                    persona: None,
                },
            )
            .await
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("non-writable ProductConversation rejects new aggregate participants"));
        assert!(matches!(
            db.get_conversation("late-subordinate").await,
            Err(DbError::ConversationNotFound(_))
        ));
    }

    #[tokio::test]
    async fn settlement_captures_live_subordinate_aggregate_participant() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        db.create_subagent_conversation(
            "subordinate",
            "subordinate",
            "/tmp",
            "root",
            "test-model",
            &crate::ConvMode::Direct,
            phoenix_core::llm_language::LlmLanguage::default(),
            db.get_conversation("root")
                .await
                .unwrap()
                .attached_work_scope_id
                .as_ref(),
            crate::SubAgentExecution {
                connection: "mock",
                effort: None,
                persona: None,
            },
        )
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO conversation_creation_jobs (
                 id, conversation_id, message_id, status, stage, attempt, generation,
                 intent_json, error, accepted_at, provisioning_started_at, completed_at,
                 failed_at, cancelled_at, deletion_requested_at, created_at, updated_at
             ) VALUES (
                 'subordinate-live', 'subordinate', NULL, 'accepted', 'validate_intent', 0, 0,
                 '{}', NULL, '2025-01-01T00:00:00Z', NULL, NULL,
                 NULL, NULL, NULL, '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z'
             )",
        )
        .execute(db.pool())
        .await
        .unwrap();

        db.begin_close_foundation(
            &product_id("root"),
            &transcript_id("root"),
            "attempt-subordinate",
        )
        .await
        .unwrap();
        db.begin_close_idle_settlement("attempt-subordinate")
            .await
            .unwrap();

        assert_eq!(
            db.list_close_settlement_conversation_ids("attempt-subordinate")
                .await
                .unwrap(),
            vec!["root".to_string(), "subordinate".to_string()]
        );
        assert!(matches!(
            db.advance_close_settlement_when_quiescent("attempt-subordinate")
                .await
                .unwrap_err(),
            DbError::CloseFoundationPrecondition(message)
                if message.contains("active durable member obligation")
        ));
        let lifecycle: String = sqlx::query_scalar(
            "SELECT ordinary_lifecycle FROM product_conversations WHERE id = 'root'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(lifecycle, "open");

        sqlx::query(
            "UPDATE conversation_creation_jobs
             SET status = 'cancelled', cancelled_at = '2025-01-01T00:00:01Z',
                 updated_at = '2025-01-01T00:00:01Z'
             WHERE id = 'subordinate-live'",
        )
        .execute(db.pool())
        .await
        .unwrap();
        assert_eq!(
            db.advance_close_settlement_when_quiescent("attempt-subordinate")
                .await
                .unwrap()
                .phase(),
            ClosePhase::AwaitingRetirementInspection
        );
    }

    #[tokio::test]
    async fn direct_turn_settlement_captures_sealed_aggregate_authority_and_receipts_exact_release()
    {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        create_child(&db, "latest", "root").await;
        sqlx::query(
            "INSERT INTO workflows (
                 workflow_id, profile_kind, profile_version, runtime_acceptance_enabled,
                 external_acceptance_enabled, version, generation, status,
                 snapshot_codec_family, snapshot_codec_version, snapshot_payload, created_at, updated_at
             ) VALUES (1, 'direct_turn', 1, 1, 0, 0, 0, 'Active', 'direct_turn', 1, X'00', 1, 1),
                      (2, 'direct_turn', 1, 1, 0, 0, 0, 'Active', 'direct_turn', 1, X'00', 1, 1)",
        )
        .execute(db.pool())
        .await
        .unwrap();
        for (turn_id, conversation_id) in [(1, "root"), (2, "latest")] {
            sqlx::query(
                "INSERT INTO durable_turns (
                     turn_id, workflow_id, conversation_id, client_turn_key, prepared_fingerprint,
                     prepared_payload, disposition, generation, terminal_kind, terminal_reason,
                     owns_conversation, canonical_message_id
                 ) VALUES (?1, ?1, ?2, 'turn-key', 'prepared', X'00', 'Runtime', 0, NULL, NULL, 1, NULL)",
            )
            .bind(turn_id)
            .bind(conversation_id)
            .execute(db.pool())
            .await
            .unwrap();
        }
        db.begin_close_foundation(
            &product_id("root"),
            &transcript_id("latest"),
            "attempt-exact-receipt",
        )
        .await
        .unwrap();
        db.confirm_close_stop_work("attempt-exact-receipt")
            .await
            .unwrap();
        db.begin_close_active_work_settlement("attempt-exact-receipt")
            .await
            .unwrap();

        let targets = db
            .list_unsettled_close_direct_turn_settlement_targets("attempt-exact-receipt")
            .await
            .unwrap();
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].conversation_id, "root");
        assert_eq!(targets[0].turn_id, 1);
        assert_eq!(targets[1].conversation_id, "latest");
        assert_eq!(targets[1].turn_id, 2);
        assert!(targets.iter().all(|target| target.expected_generation == 0));
        sqlx::query(
            "UPDATE durable_turns
             SET generation = generation + 1, terminal_kind = 'Completed', owns_conversation = 0
             WHERE turn_id IN (1, 2)",
        )
        .execute(db.pool())
        .await
        .unwrap();
        for target in &targets {
            assert!(db
                .record_close_direct_turn_settlement_if_released("attempt-exact-receipt", target,)
                .await
                .unwrap());
            assert!(db
                .record_close_direct_turn_settlement_if_released("attempt-exact-receipt", target,)
                .await
                .unwrap());
        }
        assert!(db
            .list_unsettled_close_direct_turn_settlement_targets("attempt-exact-receipt")
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn active_work_settlement_requires_stop_work_confirmation() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        db.begin_close_foundation(
            &product_id("root"),
            &transcript_id("root"),
            "attempt-confirm",
        )
        .await
        .unwrap();

        assert!(matches!(
            db.begin_close_active_work_settlement("attempt-confirm")
                .await
                .unwrap_err(),
            DbError::CloseFoundationPrecondition(message)
                if message.contains("awaiting_blocker_resolution")
        ));
        assert_eq!(
            db.confirm_close_stop_work("attempt-confirm")
                .await
                .unwrap()
                .phase(),
            ClosePhase::AwaitingStopWorkConfirmation
        );
        assert_eq!(
            db.begin_close_active_work_settlement("attempt-confirm")
                .await
                .unwrap()
                .phase(),
            ClosePhase::SettlingActiveWork
        );
    }

    #[tokio::test]
    async fn aggregate_latest_turn_remains_exact_settlement_target() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        create_child(&db, "participant", "root").await;
        let product_conversation_id = product_id("root");
        sqlx::query(
            "INSERT INTO workflows (
                 workflow_id, profile_kind, profile_version, runtime_acceptance_enabled,
                 external_acceptance_enabled, version, generation, status,
                 snapshot_codec_family, snapshot_codec_version, snapshot_payload, created_at, updated_at
             ) VALUES (1, 'direct_turn', 1, 1, 0, 0, 0, 'Active', 'direct_turn', 1, X'00', 1, 1)",
        )
        .execute(db.pool())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO durable_turns (
                 turn_id, workflow_id, conversation_id, client_turn_key, prepared_fingerprint,
                 prepared_payload, disposition, generation, terminal_kind, terminal_reason,
                 owns_conversation, canonical_message_id
             ) VALUES (1, 1, 'participant', 'turn-key', 'prepared', X'00', 'Runtime', 0, NULL, NULL, 1, NULL)",
        )
        .execute(db.pool())
        .await
        .unwrap();
        db.begin_close_foundation(
            &product_conversation_id,
            &transcript_id("participant"),
            "attempt-participant",
        )
        .await
        .unwrap();
        assert_eq!(
            db.list_close_settlement_conversation_ids("attempt-participant")
                .await
                .unwrap(),
            vec!["participant".to_string(), "root".to_string()]
        );
        db.confirm_close_stop_work("attempt-participant")
            .await
            .unwrap();
        db.begin_close_active_work_settlement("attempt-participant")
            .await
            .unwrap();

        assert!(matches!(
            db.advance_close_settlement_when_quiescent("attempt-participant")
                .await
                .unwrap_err(),
            DbError::CloseFoundationPrecondition(message)
                if message.contains("unsettled direct-turn receipt")
        ));
    }

    #[tokio::test]
    async fn aggregate_participant_remains_unsettled_until_exact_receipt() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        create_child(&db, "participant", "root").await;
        create_child(&db, "latest", "participant").await;
        sqlx::query(
            "INSERT INTO workflows (
                 workflow_id, profile_kind, profile_version, runtime_acceptance_enabled,
                 external_acceptance_enabled, version, generation, status,
                 snapshot_codec_family, snapshot_codec_version, snapshot_payload, created_at, updated_at
             ) VALUES (1, 'direct_turn', 1, 1, 0, 0, 0, 'Active', 'direct_turn', 1, X'00', 1, 1),
                      (2, 'direct_turn', 1, 1, 0, 0, 0, 'Active', 'direct_turn', 1, X'00', 1, 1)",
        )
        .execute(db.pool())
        .await
        .unwrap();
        for (turn_id, conversation_id) in [(1, "latest"), (2, "participant")] {
            sqlx::query(
                "INSERT INTO durable_turns (
                     turn_id, workflow_id, conversation_id, client_turn_key, prepared_fingerprint,
                     prepared_payload, disposition, generation, terminal_kind, terminal_reason,
                     owns_conversation, canonical_message_id
                 ) VALUES (?1, ?1, ?2, 'turn-key', 'prepared', X'00', 'Runtime', 0, NULL, NULL, 1, NULL)",
            )
            .bind(turn_id)
            .bind(conversation_id)
            .execute(db.pool())
            .await
            .unwrap();
        }
        db.begin_close_foundation(
            &product_id("root"),
            &transcript_id("latest"),
            "attempt-all-members",
        )
        .await
        .unwrap();
        db.confirm_close_stop_work("attempt-all-members")
            .await
            .unwrap();
        db.begin_close_active_work_settlement("attempt-all-members")
            .await
            .unwrap();
        let targets = db
            .list_unsettled_close_direct_turn_settlement_targets("attempt-all-members")
            .await
            .unwrap();
        assert_eq!(targets.len(), 2);
        let latest_target = targets
            .iter()
            .find(|target| target.conversation_id == "latest")
            .unwrap();
        let participant_target = targets
            .iter()
            .find(|target| target.conversation_id == "participant")
            .unwrap();
        sqlx::query(
            "UPDATE durable_turns SET generation = 1, terminal_kind = 'Completed', owns_conversation = 0 WHERE turn_id = 1",
        )
        .execute(db.pool())
        .await
        .unwrap();
        assert!(db
            .record_close_direct_turn_settlement_if_released("attempt-all-members", latest_target)
            .await
            .unwrap());

        assert!(matches!(
            db.advance_close_settlement_when_quiescent("attempt-all-members")
                .await
                .unwrap_err(),
            DbError::CloseFoundationPrecondition(message)
                if message.contains("unsettled direct-turn receipt")
        ));
        sqlx::query(
            "UPDATE durable_turns SET generation = 1, terminal_kind = 'Completed', owns_conversation = 0 WHERE turn_id = 2",
        )
        .execute(db.pool())
        .await
        .unwrap();
        assert!(db
            .record_close_direct_turn_settlement_if_released(
                "attempt-all-members",
                participant_target,
            )
            .await
            .unwrap());
        assert_eq!(
            db.advance_close_settlement_when_quiescent("attempt-all-members")
                .await
                .unwrap()
                .phase(),
            ClosePhase::AwaitingRetirementInspection
        );
    }

    #[tokio::test]
    async fn subordinate_direct_turn_is_captured_and_archived_with_aggregate() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        db.create_subagent_conversation(
            "subordinate-turn",
            "subordinate-turn",
            "/tmp",
            "root",
            "test-model",
            &crate::ConvMode::Direct,
            phoenix_core::llm_language::LlmLanguage::default(),
            db.get_conversation("root")
                .await
                .unwrap()
                .attached_work_scope_id
                .as_ref(),
            crate::SubAgentExecution {
                connection: "mock",
                effort: None,
                persona: None,
            },
        )
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO workflows (
                 workflow_id, profile_kind, profile_version, runtime_acceptance_enabled,
                 external_acceptance_enabled, version, generation, status,
                 snapshot_codec_family, snapshot_codec_version, snapshot_payload, created_at, updated_at
             ) VALUES (31, 'direct_turn', 1, 1, 0, 0, 0, 'Active',
                       'direct_turn', 1, X'00', 1, 1)",
        )
        .execute(db.pool())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO durable_turns (
                 turn_id, workflow_id, conversation_id, client_turn_key,
                 prepared_fingerprint, prepared_payload, disposition, generation,
                 terminal_kind, terminal_reason, owns_conversation, canonical_message_id
             ) VALUES (31, 31, 'subordinate-turn', 'key', 'fingerprint', X'00',
                       'Runtime', 0, NULL, NULL, 1, NULL)",
        )
        .execute(db.pool())
        .await
        .unwrap();

        db.begin_close_foundation(
            &product_id("root"),
            &transcript_id("root"),
            "attempt-subordinate-turn",
        )
        .await
        .unwrap();
        db.begin_close_idle_settlement("attempt-subordinate-turn")
            .await
            .unwrap();
        let targets = db
            .list_unsettled_close_direct_turn_settlement_targets("attempt-subordinate-turn")
            .await
            .unwrap();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].conversation_id, "subordinate-turn");

        sqlx::query(
            "UPDATE durable_turns
             SET generation = 1, terminal_kind = 'Cancelled', owns_conversation = 0
             WHERE turn_id = 31",
        )
        .execute(db.pool())
        .await
        .unwrap();
        assert!(db
            .record_close_direct_turn_settlement_if_released(
                "attempt-subordinate-turn",
                &targets[0],
            )
            .await
            .unwrap());
        assert_eq!(
            db.advance_close_settlement_when_quiescent("attempt-subordinate-turn")
                .await
                .unwrap()
                .phase(),
            ClosePhase::AwaitingRetirementInspection
        );
    }

    #[tokio::test]
    async fn deleted_participant_settlement_preserves_other_live_authority() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "deleted-a").await;
        create_child(&db, "live-b", "deleted-a").await;
        sqlx::query(
            "INSERT INTO workflows (
                 workflow_id, profile_kind, profile_version, runtime_acceptance_enabled,
                 external_acceptance_enabled, version, generation, status,
                 snapshot_codec_family, snapshot_codec_version, snapshot_payload, created_at, updated_at
             ) VALUES (41, 'direct_turn', 1, 1, 0, 0, 0, 'Active', 'direct_turn', 1, X'00', 1, 1);
             INSERT INTO durable_turns (
                 turn_id, workflow_id, conversation_id, client_turn_key, prepared_fingerprint,
                 prepared_payload, disposition, generation, terminal_kind, terminal_reason,
                 owns_conversation, canonical_message_id
             ) VALUES (41, 41, 'live-b', 'live-b-turn', 'prepared', X'00', 'Runtime', 0, NULL, NULL, 1, NULL);",
        )
        .execute(db.pool())
        .await
        .unwrap();
        db.begin_close_foundation(
            &product_id("deleted-a"),
            &transcript_id("live-b"),
            "deleted-a-attempt",
        )
        .await
        .unwrap();
        db.confirm_close_stop_work("deleted-a-attempt")
            .await
            .unwrap();
        db.begin_close_active_work_settlement("deleted-a-attempt")
            .await
            .unwrap();
        sqlx::query(
            "UPDATE close_attempt_participants SET settlement_state = 'deleted'
             WHERE attempt_id = 'deleted-a-attempt' AND conversation_id = 'deleted-a'",
        )
        .execute(db.pool())
        .await
        .unwrap();
        let states: Vec<(String, String)> = sqlx::query_as(
            "SELECT conversation_id, settlement_state FROM close_attempt_participants
             WHERE attempt_id = 'deleted-a-attempt' ORDER BY conversation_id",
        )
        .fetch_all(db.pool())
        .await
        .unwrap();
        assert_eq!(
            states,
            vec![
                ("deleted-a".to_string(), "deleted".to_string()),
                ("live-b".to_string(), "live".to_string()),
            ]
        );
        assert!(db
            .advance_close_settlement_when_quiescent("deleted-a-attempt")
            .await
            .is_err());
        db.request_close_settlement_cancellation("deleted-a-attempt")
            .await
            .unwrap();
        assert!(db
            .advance_close_settlement_when_quiescent("deleted-a-attempt")
            .await
            .is_err());
        assert_eq!(
            db.get_close_obligation("deleted-a-attempt")
                .await
                .unwrap()
                .phase(),
            ClosePhase::CancelRequestedDuringSettlement
        );
    }

    #[tokio::test]
    async fn active_work_settlement_remains_fenced_until_every_captured_turn_is_terminal() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        create_child(&db, "latest", "root").await;
        sqlx::query(
            "INSERT INTO workflows (
                 workflow_id, profile_kind, profile_version, runtime_acceptance_enabled,
                 external_acceptance_enabled, version, generation, status,
                 snapshot_codec_family, snapshot_codec_version, snapshot_payload, created_at, updated_at
             ) VALUES (1, 'direct_turn', 1, 1, 0, 0, 0, 'Active', 'direct_turn', 1, X'00', 1, 1)",
        )
        .execute(db.pool())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO durable_turns (
                 turn_id, workflow_id, conversation_id, client_turn_key, prepared_fingerprint,
                 prepared_payload, disposition, generation, terminal_kind, terminal_reason,
                 owns_conversation, canonical_message_id
             ) VALUES (1, 1, 'latest', 'turn-key', 'prepared', X'00', 'Runtime', 0, NULL, NULL, 1, NULL)",
        )
        .execute(db.pool())
        .await
        .unwrap();
        db.begin_close_foundation(
            &product_id("root"),
            &transcript_id("latest"),
            "attempt-busy",
        )
        .await
        .unwrap();
        db.confirm_close_stop_work("attempt-busy").await.unwrap();
        db.begin_close_active_work_settlement("attempt-busy")
            .await
            .unwrap();

        assert!(matches!(
            db.advance_close_settlement_when_quiescent("attempt-busy")
                .await
                .unwrap_err(),
            DbError::CloseFoundationPrecondition(message)
                if message.contains("unsettled direct-turn receipt")
        ));
        assert_eq!(
            db.get_close_obligation("attempt-busy")
                .await
                .unwrap()
                .phase(),
            ClosePhase::SettlingActiveWork
        );

        sqlx::query(
            "UPDATE durable_turns
             SET generation = generation + 1, terminal_kind = 'Cancelled', owns_conversation = 0
             WHERE turn_id = 1",
        )
        .execute(db.pool())
        .await
        .unwrap();
        assert_eq!(
            db.advance_close_settlement_when_quiescent("attempt-busy")
                .await
                .unwrap()
                .phase(),
            ClosePhase::AwaitingRetirementInspection
        );
    }

    #[tokio::test]
    async fn active_work_settlement_remains_fenced_until_creation_job_is_terminal() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        sqlx::query(
            "INSERT INTO conversation_creation_jobs (
                 id, conversation_id, message_id, status, stage, attempt, generation,
                 intent_json, error, accepted_at, provisioning_started_at, completed_at,
                 failed_at, cancelled_at, deletion_requested_at, created_at, updated_at
             ) VALUES (
                 'creation-job', 'root', NULL, 'accepted', 'validate_intent', 0, 0,
                 '{}', NULL, '2025-01-01T00:00:00Z', NULL, NULL,
                 NULL, NULL, NULL, '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z'
             )",
        )
        .execute(db.pool())
        .await
        .unwrap();
        db.begin_close_foundation(
            &product_id("root"),
            &transcript_id("root"),
            "attempt-creation",
        )
        .await
        .unwrap();
        db.confirm_close_stop_work("attempt-creation")
            .await
            .unwrap();
        db.begin_close_active_work_settlement("attempt-creation")
            .await
            .unwrap();

        assert!(matches!(
            db.advance_close_settlement_when_quiescent("attempt-creation")
                .await
                .unwrap_err(),
            DbError::CloseFoundationPrecondition(_)
        ));
        sqlx::query(
            "UPDATE conversation_creation_jobs
             SET status = 'cancelled', cancelled_at = '2025-01-01T00:00:01Z'
             WHERE id = 'creation-job'",
        )
        .execute(db.pool())
        .await
        .unwrap();
        assert_eq!(
            db.advance_close_settlement_when_quiescent("attempt-creation")
                .await
                .unwrap()
                .phase(),
            ClosePhase::AwaitingRetirementInspection
        );
    }

    #[tokio::test]
    async fn cancelled_active_work_settlement_completes_after_members_quiesce() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        db.begin_close_foundation(
            &product_id("root"),
            &transcript_id("root"),
            "attempt-cancel-settlement",
        )
        .await
        .unwrap();
        db.confirm_close_stop_work("attempt-cancel-settlement")
            .await
            .unwrap();
        db.begin_close_active_work_settlement("attempt-cancel-settlement")
            .await
            .unwrap();
        assert_eq!(
            db.request_close_settlement_cancellation("attempt-cancel-settlement")
                .await
                .unwrap()
                .phase(),
            ClosePhase::CancelRequestedDuringSettlement
        );

        let obligation = db
            .advance_close_settlement_when_quiescent("attempt-cancel-settlement")
            .await
            .unwrap();
        assert_eq!(obligation.phase(), ClosePhase::Completed);
        assert_eq!(
            obligation.close_outcome(),
            Some(CloseCompletionOutcome::Cancelled)
        );
        assert!(obligation.completed_at().is_some());
        assert_eq!(
            db.advance_close_settlement_when_quiescent("attempt-cancel-settlement")
                .await
                .unwrap()
                .close_outcome(),
            Some(CloseCompletionOutcome::Cancelled)
        );
    }

    #[tokio::test]
    async fn cancelled_active_work_settlement_stays_fenced_until_busy_turn_releases() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        sqlx::query(
            "INSERT INTO workflows (
                 workflow_id, profile_kind, profile_version, runtime_acceptance_enabled,
                 external_acceptance_enabled, version, generation, status,
                 snapshot_codec_family, snapshot_codec_version, snapshot_payload, created_at, updated_at
             ) VALUES (1, 'direct_turn', 1, 1, 0, 0, 0, 'Active', 'direct_turn', 1, X'00', 1, 1)",
        )
        .execute(db.pool())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO durable_turns (
                 turn_id, workflow_id, conversation_id, client_turn_key, prepared_fingerprint,
                 prepared_payload, disposition, generation, terminal_kind, terminal_reason,
                 owns_conversation, canonical_message_id
             ) VALUES (1, 1, 'root', 'turn-key', 'prepared', X'00', 'Runtime', 0, NULL, NULL, 1, NULL)",
        )
        .execute(db.pool())
        .await
        .unwrap();
        db.begin_close_foundation(
            &product_id("root"),
            &transcript_id("root"),
            "attempt-cancel-busy",
        )
        .await
        .unwrap();
        db.confirm_close_stop_work("attempt-cancel-busy")
            .await
            .unwrap();
        db.begin_close_active_work_settlement("attempt-cancel-busy")
            .await
            .unwrap();
        assert_eq!(
            db.request_close_settlement_cancellation("attempt-cancel-busy")
                .await
                .unwrap()
                .phase(),
            ClosePhase::CancelRequestedDuringSettlement
        );

        assert!(matches!(
            db.advance_close_settlement_when_quiescent("attempt-cancel-busy")
                .await
                .unwrap_err(),
            DbError::CloseFoundationPrecondition(_)
        ));
        assert_eq!(
            db.get_close_obligation("attempt-cancel-busy")
                .await
                .unwrap()
                .phase(),
            ClosePhase::CancelRequestedDuringSettlement
        );

        sqlx::query(
            "UPDATE durable_turns
             SET generation = generation + 1, terminal_kind = 'Cancelled', owns_conversation = 0
             WHERE turn_id = 1",
        )
        .execute(db.pool())
        .await
        .unwrap();
        let obligation = db
            .advance_close_settlement_when_quiescent("attempt-cancel-busy")
            .await
            .unwrap();
        assert_eq!(obligation.phase(), ClosePhase::Completed);
        assert_eq!(
            obligation.close_outcome(),
            Some(CloseCompletionOutcome::Cancelled)
        );
    }

    #[tokio::test]
    async fn message_admission_uses_typed_aggregate_or_standalone_authority() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "open-archived-drift").await;
        create_root(&db, "history-target").await;
        sqlx::query("UPDATE conversations SET archived = 1 WHERE id = 'open-archived-drift'")
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query(
            "UPDATE product_conversations SET ordinary_lifecycle = 'history'
             WHERE id = 'history-target'",
        )
        .execute(db.pool())
        .await
        .unwrap();
        let coordinator = db
            .get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
            .await
            .unwrap();
        sqlx::query("UPDATE conversations SET archived = 1 WHERE id = ?1")
            .bind(&coordinator.id)
            .execute(db.pool())
            .await
            .unwrap();

        assert!(matches!(
            db.message_target_admission("open-archived-drift")
                .await
                .unwrap(),
            MessageTargetAdmission::Aggregate(ProductConversationAdmission::Accepted { .. })
        ));
        assert!(matches!(
            db.message_target_admission("history-target").await.unwrap(),
            MessageTargetAdmission::Aggregate(ProductConversationAdmission::History(_))
        ));
        assert_eq!(
            db.message_target_admission(&coordinator.id).await.unwrap(),
            MessageTargetAdmission::StandaloneArchived
        );
    }

    #[tokio::test]
    async fn product_conversation_admission_is_open_without_a_close_attempt() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        assert!(matches!(
            db.product_conversation_admission("root").await.unwrap(),
            ProductConversationAdmission::Accepted { product_conversation_id }
                if product_conversation_id == product_id("root")
        ));
    }

    #[tokio::test]
    async fn product_conversation_admission_accepts_coordinator_without_ordinary_lifecycle() {
        let db = Database::open_in_memory().await.unwrap();
        let coordinator = db
            .get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
            .await
            .unwrap();

        assert!(matches!(
            db.product_conversation_admission(&coordinator.id).await.unwrap(),
            ProductConversationAdmission::Accepted { product_conversation_id }
                if product_conversation_id == coordinator.product_conversation_id
        ));
    }

    #[tokio::test]
    async fn product_conversation_admission_refuses_history_after_close_completes() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        let product_conversation_id = product_id("root");
        sqlx::query(
            "UPDATE product_conversations SET ordinary_lifecycle = 'history' WHERE id = ?1",
        )
        .bind(product_conversation_id.as_str())
        .execute(db.pool())
        .await
        .unwrap();

        assert!(matches!(
            db.product_conversation_admission("root").await.unwrap(),
            ProductConversationAdmission::History(id) if id == product_conversation_id
        ));
    }

    #[tokio::test]
    async fn product_conversation_admission_refuses_every_captured_member_after_close_begins() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        create_child(&db, "latest", "root").await;
        db.begin_close_foundation(
            &product_id("root"),
            &transcript_id("latest"),
            "admission-fence",
        )
        .await
        .unwrap();

        for conversation_id in ["root", "latest"] {
            assert!(matches!(
                db.product_conversation_admission(conversation_id)
                    .await
                    .unwrap(),
                ProductConversationAdmission::Refused(CloseAdmissionFence { attempt_id, phase, .. })
                    if attempt_id.as_str() == "admission-fence"
                        && phase == ClosePhase::AwaitingBlockerResolution
            ));
        }
    }

    #[tokio::test]
    async fn fresh_distinct_product_identity_admits_retirement_inventory() {
        let db = Database::open_in_memory().await.unwrap();
        let conversation = db
            .create_conversation(
                "fresh-retirement-transcript",
                "fresh-retirement-transcript",
                "/tmp",
                true,
                None,
                None,
            )
            .await
            .unwrap();
        assert_ne!(
            conversation.id,
            conversation.product_conversation_id.as_str()
        );
        let scope = allocate_scope_worktree(&db, &conversation.id).await;
        db.begin_close_foundation(
            &conversation.product_conversation_id,
            &transcript_id(&conversation.id),
            "fresh-retirement-attempt",
        )
        .await
        .unwrap();
        set_close_phase(
            &db,
            "fresh-retirement-attempt",
            ClosePhase::RetirementRequested,
        )
        .await;
        let snapshot = current_test_snapshot(&db, "fresh-retirement-attempt").await;

        let resources = db
            .capture_close_retirement_inventory(CaptureCloseRetirementInventoryRequest {
                attempt_id: CloseAttemptId::parse("fresh-retirement-attempt").unwrap(),
                snapshot,
                scopes: vec![CaptureCloseRetirementInventoryScopeRequest {
                    scope: scope.clone(),
                    inventory: CloseOwnedResourceInventory {
                        work_scopes: std::collections::BTreeSet::new(),
                        worktree: Some(current_test_worktree(&db, &scope).await),
                        bash_process_groups: std::collections::BTreeSet::default(),
                        tmux_servers: std::collections::BTreeSet::default(),
                        pty_sessions: std::collections::BTreeSet::default(),
                        browser_sessions: std::collections::BTreeSet::default(),
                        equivalent_live_resources: std::collections::BTreeSet::default(),
                    },
                }],
            })
            .await
            .unwrap();
        assert_eq!(resources.len(), 2);
        assert!(resources
            .iter()
            .any(|resource| { resource.resource.kind() == RetiredResourceKind::WorkScope }));
    }

    #[tokio::test]
    async fn close_product_ownership_rejects_null_and_reassignment() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        create_root(&db, "other").await;
        db.begin_close_foundation(&product_id("root"), &transcript_id("root"), "attempt-owned")
            .await
            .unwrap();

        for replacement in [None, Some("other")] {
            assert!(sqlx::query(
                "UPDATE close_obligations
                 SET product_conversation_id = ?1
                 WHERE attempt_id = 'attempt-owned'",
            )
            .bind(replacement)
            .execute(db.pool())
            .await
            .is_err());
        }
        assert_eq!(
            db.get_close_obligation("attempt-owned")
                .await
                .unwrap()
                .product_conversation_id(),
            &product_id("root")
        );
    }

    #[tokio::test]
    async fn three_unarchived_chain_latest_admits_and_reads_topology() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        create_child(&db, "mid", "root").await;
        create_child(&db, "leaf", "mid").await;
        set_archived(&db, "mid", false).await;
        set_archived(&db, "leaf", false).await;

        let topology = db
            .close_foundation_topology(&product_id("root"))
            .await
            .unwrap();
        assert_eq!(topology.root.id, "root");
        assert_eq!(topology.latest.id, "leaf");
        assert_eq!(topology.member_ids(), vec!["root", "mid", "leaf"]);
        assert_eq!(topology.members[0].role, CloseMemberRole::Root);
        assert_eq!(topology.members[1].role, CloseMemberRole::Intermediate);
        assert_eq!(topology.members[2].role, CloseMemberRole::Latest);

        let obligation = db
            .begin_close_foundation(&product_id("root"), &transcript_id("leaf"), "attempt-1")
            .await
            .unwrap();
        assert_eq!(obligation.attempt_id().as_str(), "attempt-1");
        assert_eq!(obligation.phase(), ClosePhase::AwaitingBlockerResolution);
        assert_eq!(obligation.product_conversation_id().as_str(), "root");
        assert!(sqlx::query(
            "INSERT INTO close_attempt_members (
                attempt_id, conversation_id, member_role, continuation_ordinal, captured_continued_in_conv_id,
                captured_state_kind, captured_runtime_role, captured_work_scope_id, captured_at
             ) VALUES ('attempt-1', 'other', 'intermediate', 1, NULL, 'idle', 'user', NULL, ?1)",
        )
        .bind(Utc::now().to_rfc3339())
        .execute(db.pool())
        .await
        .is_err());
        assert!(sqlx::query(
            "UPDATE close_obligations SET topology_sealed = 0 WHERE attempt_id = 'attempt-1'",
        )
        .execute(db.pool())
        .await
        .is_err());

        let active = db
            .get_active_close_obligation_for_product(&product_id("root"))
            .await
            .unwrap();
        assert_eq!(active.unwrap().attempt_id().as_str(), "attempt-1");
        assert_eq!(db.list_pending_close_obligations().await.unwrap().len(), 1);
        assert_eq!(db.list_latest_close_obligations().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn topology_seal_rejects_scope_snapshot_that_differs_from_live_member() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        create_root(&db, "other").await;
        let wrong_scope = db
            .get_conversation("other")
            .await
            .unwrap()
            .attached_work_scope_id
            .unwrap();
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO close_obligations (
                 attempt_id, product_conversation_id, phase, created_at, updated_at, completed_at
             ) VALUES (
                 'attempt-wrong-scope', 'root', 'awaiting_blocker_resolution', ?1, ?1, NULL
             )",
        )
        .bind(&now)
        .execute(db.pool())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO close_attempt_members (
                 attempt_id, conversation_id, member_role, continuation_ordinal,
                 captured_continued_in_conv_id, captured_state_kind, captured_runtime_role,
                 captured_work_scope_id, captured_at
             ) VALUES (
                 'attempt-wrong-scope', 'root', 'root_latest', 0,
                 NULL, 'idle', 'user', ?1, ?2
             )",
        )
        .bind(wrong_scope.as_str())
        .bind(&now)
        .execute(db.pool())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO close_attempt_scopes (
                 attempt_id, scope, captured_worktree_identity,
                 captured_worktree_fingerprint, captured_worktree_locator, captured_at
             )
             SELECT 'attempt-wrong-scope', ?1, worktree_id, worktree_fingerprint,
                    CASE WHEN environment_kind = 'allocated_worktree'
                         THEN 'git_path_bytes_hex_v1:' || lower(hex(CAST(worktree_path AS BLOB)))
                         ELSE NULL END,
                    ?2
             FROM work_scopes WHERE id = ?1",
        )
        .bind(wrong_scope.as_str())
        .bind(&now)
        .execute(db.pool())
        .await
        .unwrap();

        assert!(sqlx::query(
            "UPDATE close_obligations SET topology_sealed = 1
             WHERE attempt_id = 'attempt-wrong-scope'",
        )
        .execute(db.pool())
        .await
        .is_err());
    }
    #[tokio::test]
    async fn topology_seal_rejects_captured_continuation_edge_that_differs_from_live_member() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        create_child(&db, "leaf", "root").await;
        create_root(&db, "other").await;
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO close_obligations (
                 attempt_id, product_conversation_id, phase, created_at, updated_at, completed_at
             ) VALUES (
                 'attempt-wrong-edge', 'root', 'awaiting_blocker_resolution', ?1, ?1, NULL
             )",
        )
        .bind(&now)
        .execute(db.pool())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO close_attempt_members (
                 attempt_id, conversation_id, member_role, continuation_ordinal,
                 captured_continued_in_conv_id, captured_state_kind, captured_runtime_role,
                 captured_work_scope_id, captured_at
             ) VALUES
                 ('attempt-wrong-edge', 'root', 'root', 0, 'other', 'idle', 'user', NULL, ?1),
                 ('attempt-wrong-edge', 'leaf', 'latest', 1, NULL, 'idle', 'user', NULL, ?1)",
        )
        .bind(&now)
        .execute(db.pool())
        .await
        .unwrap();

        assert!(sqlx::query(
            "UPDATE close_obligations SET topology_sealed = 1
             WHERE attempt_id = 'attempt-wrong-edge'",
        )
        .execute(db.pool())
        .await
        .is_err());
    }

    #[tokio::test]
    async fn topology_rejects_cross_aggregate_predecessor_before_seal() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "predecessor").await;
        create_root(&db, "root").await;
        assert!(sqlx::query(
            "UPDATE conversations SET continued_in_conv_id = 'root' WHERE id = 'predecessor'",
        )
        .execute(db.pool())
        .await
        .is_err());
    }

    #[tokio::test]
    async fn topology_rejects_cross_aggregate_second_predecessor_before_seal() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        create_child(&db, "leaf", "root").await;
        create_root(&db, "fork").await;
        assert!(sqlx::query(
            "UPDATE conversations SET continued_in_conv_id = 'leaf' WHERE id = 'fork'"
        )
        .execute(db.pool())
        .await
        .is_err());
    }

    #[tokio::test]
    async fn delimiter_bearing_ids_do_not_truncate_topology() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "a|b").await;
        create_child(&db, "b", "a|b").await;

        let topology = db
            .close_foundation_topology(&product_id("a|b"))
            .await
            .unwrap();
        assert_eq!(topology.member_ids(), vec!["a|b", "b"]);
        db.begin_close_foundation(&product_id("a|b"), &transcript_id("b"), "attempt-delimiter")
            .await
            .unwrap();
        assert_eq!(
            db.list_close_attempt_members("attempt-delimiter")
                .await
                .unwrap()
                .len(),
            2
        );
    }

    #[tokio::test]
    async fn singleton_root_is_root_latest_and_captures_one_snapshot() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;

        let topology = db
            .close_foundation_topology(&product_id("root"))
            .await
            .unwrap();
        assert_eq!(topology.member_ids(), vec!["root"]);
        assert_eq!(topology.members[0].role, CloseMemberRole::RootLatest);

        db.begin_close_foundation(&product_id("root"), &transcript_id("root"), "attempt-1")
            .await
            .unwrap();
        let members = db.list_close_attempt_members("attempt-1").await.unwrap();
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].conversation_id.as_str(), "root");
        assert_eq!(members[0].role, CloseMemberRole::RootLatest);
    }

    #[tokio::test]
    async fn unresolved_worktree_routes_from_inspection_directly_to_repair() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("repair-projection-reload.db");
        let db = Database::open(path.to_str().unwrap()).await.unwrap();
        crate::migrations::run_pending_migrations(db.pool())
            .await
            .unwrap();
        create_root(&db, "root").await;
        allocate_scope_worktree(&db, "root").await;
        db.begin_close_foundation(
            &product_id("root"),
            &transcript_id("root"),
            "attempt-inspection-repair",
        )
        .await
        .unwrap();
        set_close_phase(
            &db,
            "attempt-inspection-repair",
            ClosePhase::AwaitingRetirementInspection,
        )
        .await;

        let captured = db
            .list_close_attempt_scopes("attempt-inspection-repair")
            .await
            .unwrap()
            .remove(0);
        let scope = captured.scope;
        let Some(CapturedWorktreeIdentity::Resolved(worktree)) = captured.captured_worktree else {
            panic!("allocated scope must capture a resolved worktree");
        };
        db.route_close_attempt_to_repair(RouteCloseAttemptToRepairRequest {
            attempt_id: CloseAttemptId::parse("attempt-inspection-repair").unwrap(),
            scope: scope.clone(),
            residual: RetiredResourceIdentity::parse(
                RetiredResourceKind::Worktree,
                LossItemIdentity::Worktree(worktree),
            )
            .unwrap(),
            reason: RetirementFailureReason::IdentityNotProven,
            detail: "captured worktree identity cannot be proven".to_string(),
        })
        .await
        .unwrap();

        assert_eq!(
            db.get_close_obligation("attempt-inspection-repair")
                .await
                .unwrap()
                .phase(),
            ClosePhase::NeedsRepair
        );
        drop(db);
        let reloaded = Database::open(path.to_str().unwrap()).await.unwrap();
        let projection = reloaded
            .get_active_close_projection_for_product(&product_id("root"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(projection.residuals.len(), 1);
        assert_eq!(projection.residuals[0].scope.as_str(), scope.as_str());
        assert_eq!(
            projection.residuals[0].resource.kind(),
            RetiredResourceKind::Worktree
        );
        assert_eq!(
            projection.residuals[0].outcome,
            RetirementOutcome::Residual {
                residual_reason: RetirementFailureReason::IdentityNotProven
            }
        );
        assert_eq!(
            projection.residuals[0].detail.as_deref(),
            Some("captured worktree identity cannot be proven")
        );
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn unresolved_allocated_worktree_admits_close_and_routes_inventory_to_repair() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        let scope = allocate_scope_worktree(&db, "root").await;
        sqlx::query(
            "UPDATE work_scopes SET worktree_id = NULL, worktree_fingerprint = NULL WHERE id = ?1",
        )
        .bind(scope.as_str())
        .execute(db.pool())
        .await
        .unwrap();

        db.begin_close_foundation(
            &product_id("root"),
            &transcript_id("root"),
            "attempt-unresolved",
        )
        .await
        .unwrap();
        let scopes = db
            .list_close_attempt_scopes("attempt-unresolved")
            .await
            .unwrap();
        assert!(matches!(
            scopes[0].captured_worktree,
            Some(CapturedWorktreeIdentity::Unresolved { .. })
        ));

        set_close_phase(
            &db,
            "attempt-unresolved",
            ClosePhase::AwaitingRetirementInspection,
        )
        .await;
        db.replace_close_inspection(ReplaceCloseInspectionRequest {
            attempt_id: CloseAttemptId::parse("attempt-unresolved").unwrap(),
            scopes: vec![ReplaceCloseInspectionScopeRequest {
                scope: scope.clone(),
                snapshot: CloseRetirementSnapshot::parse("unresolved:g1", "unresolved:fp1")
                    .unwrap(),
                losses: vec![],
            }],
        })
        .await
        .unwrap();
        let error = db
            .capture_close_retirement_inventory(CaptureCloseRetirementInventoryRequest {
                attempt_id: CloseAttemptId::parse("attempt-unresolved").unwrap(),
                snapshot: current_test_snapshot(&db, "attempt-unresolved").await,
                scopes: vec![CaptureCloseRetirementInventoryScopeRequest {
                    scope: scope.clone(),
                    inventory: CloseOwnedResourceInventory {
                        work_scopes: std::collections::BTreeSet::new(),
                        worktree: None,
                        bash_process_groups: std::collections::BTreeSet::new(),
                        tmux_servers: std::collections::BTreeSet::new(),
                        pty_sessions: std::collections::BTreeSet::new(),
                        browser_sessions: std::collections::BTreeSet::new(),
                        equivalent_live_resources: std::collections::BTreeSet::new(),
                    },
                }],
            })
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            DbError::CloseFoundationRepairRequired(
                CloseFoundationRepair::UnresolvedWorktreeIdentity {
                    attempt_id,
                    scope: repair_scope,
                    locator,
                }
            ) if attempt_id.as_str() == "attempt-unresolved"
                && repair_scope == scope
                && locator.as_bytes() == b"/tmp/worktree"
        ));
        assert_eq!(
            db.get_close_obligation("attempt-unresolved")
                .await
                .unwrap()
                .phase(),
            ClosePhase::NeedsRepair
        );
        let replay_error = db
            .capture_close_retirement_inventory(CaptureCloseRetirementInventoryRequest {
                attempt_id: CloseAttemptId::parse("attempt-unresolved").unwrap(),
                snapshot: current_test_snapshot(&db, "attempt-unresolved").await,
                scopes: vec![CaptureCloseRetirementInventoryScopeRequest {
                    scope: scope.clone(),
                    inventory: CloseOwnedResourceInventory {
                        work_scopes: std::collections::BTreeSet::new(),
                        worktree: None,
                        bash_process_groups: std::collections::BTreeSet::new(),
                        tmux_servers: std::collections::BTreeSet::new(),
                        pty_sessions: std::collections::BTreeSet::new(),
                        browser_sessions: std::collections::BTreeSet::new(),
                        equivalent_live_resources: std::collections::BTreeSet::new(),
                    },
                }],
            })
            .await
            .unwrap_err();
        assert!(
            matches!(
                replay_error,
                DbError::CloseFoundationRepairRequired(
                    CloseFoundationRepair::UnresolvedWorktreeIdentity { .. }
                )
            ),
            "unexpected replay error: {replay_error:?}"
        );
    }

    #[tokio::test]
    async fn aggregate_identity_derives_latest_instead_of_rejecting_predecessor() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        create_child(&db, "leaf", "root").await;

        let obligation = db
            .begin_close_foundation(&product_id("root"), &transcript_id("leaf"), "attempt-1")
            .await
            .unwrap();
        assert_eq!(obligation.product_conversation_id(), &product_id("root"));
        let topology = db
            .close_foundation_topology(&product_id("root"))
            .await
            .unwrap();
        assert_eq!(topology.latest.id, "leaf");
    }

    #[tokio::test]
    async fn approval_state_is_rejected() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        set_state(&db, "root", approval_state()).await;

        let err = db
            .begin_close_foundation(&product_id("root"), &transcript_id("root"), "attempt-1")
            .await
            .unwrap_err();
        assert!(matches!(err, DbError::CloseFoundationPrecondition(_)));
    }

    #[tokio::test]
    async fn busy_latest_states_admit() {
        let db = Database::open_in_memory().await.unwrap();
        let assistant = AssistantMessage::new(
            "busy-asst".to_string(),
            vec![ContentBlock::tool_use(
                "tool-1",
                "think",
                serde_json::json!({"thoughts": "busy"}),
            )],
            None,
            None,
        );
        let states = vec![
            ("llm", ConvState::LlmRequesting { attempt: 1 }),
            (
                "seeded",
                ConvState::SeededLlmRequesting {
                    seed_message_id: "seed-1".to_string(),
                    attempt: 1,
                },
            ),
            (
                "provisioning",
                ConvState::Provisioning {
                    job_id: "job-1".to_string(),
                    phase: ConversationCreationPhase::Accepted,
                },
            ),
            (
                "tool",
                ConvState::ToolExecuting {
                    current_tool: ToolCall::new(
                        "tool-1",
                        ToolInput::Unknown {
                            name: "think".to_string(),
                            input: serde_json::json!({"thoughts": "busy"}),
                        },
                    ),
                    remaining_tools: Vec::new(),
                    completed_results: Vec::new(),
                    pending_sub_agents: Vec::new(),
                    assistant_message: assistant.clone(),
                },
            ),
            (
                "cancel-tool",
                ConvState::CancellingTool {
                    cause: phoenix_core::domain::sm_event::CancelCause::UserRequested,
                    tool_use_id: "tool-1".to_string(),
                    skipped_tools: Vec::new(),
                    completed_results: Vec::new(),
                    assistant_message: assistant.clone(),
                    pending_sub_agents: Vec::new(),
                },
            ),
            (
                "awaiting-subagents",
                ConvState::AwaitingSubAgents {
                    pending: Vec::new(),
                    completed_results: Vec::new(),
                    spawn_tool_id: Some("tool-1".to_string()),
                },
            ),
            (
                "cancelling-subagents",
                ConvState::CancellingSubAgents {
                    pending: Vec::new(),
                    completed_results: Vec::new(),
                    cause: phoenix_core::domain::sm_event::CancelCause::UserRequested,
                    spawn_tool_id: Some("tool-1".to_string()),
                },
            ),
        ];
        for (id, state) in states {
            create_root(&db, id).await;
            set_state(&db, id, state).await;
            let obligation = db
                .begin_close_foundation(
                    &product_id(id),
                    &transcript_id(id),
                    &format!("attempt-{id}"),
                )
                .await
                .unwrap();
            assert_eq!(obligation.product_conversation_id().as_str(), id);
            sqlx::query("UPDATE close_obligations SET phase = 'completed', completed_at = ?2, close_outcome = 'cancelled' WHERE attempt_id = ?1")
                .bind(format!("attempt-{id}"))
                .bind(Utc::now().to_rfc3339())
                .execute(db.pool())
                .await
                .unwrap();
        }
    }

    #[tokio::test]
    async fn direct_close_rejects_busy_latest_without_creating_an_attempt() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        set_state(&db, "root", ConvState::LlmRequesting { attempt: 1 }).await;

        let error = db
            .begin_direct_close_foundation(
                &product_id("root"),
                &transcript_id("root"),
                "attempt-direct",
            )
            .await
            .unwrap_err();

        assert!(matches!(error, DbError::CloseFoundationConflict(_)));
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM close_obligations WHERE attempt_id = 'attempt-direct'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn close_busy_classification_uses_the_transactional_admission_snapshot() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        set_state(&db, "root", ConvState::LlmRequesting { attempt: 1 }).await;
        let obligation = db
            .begin_close_foundation(&product_id("root"), &transcript_id("root"), "attempt-busy")
            .await
            .unwrap();

        set_state(&db, "root", ConvState::Idle).await;

        assert!(db
            .close_attempt_latest_was_busy(obligation.attempt_id().as_str())
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn handed_off_latest_is_rejected() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        db.update_conversation_state(
            "root",
            &ConvState::HandedOff {
                successor_conv_id: "missing-successor".to_string(),
            },
        )
        .await
        .unwrap();

        assert!(matches!(
            db.begin_close_foundation(&product_id("root"), &transcript_id("root"), "attempt-1")
                .await
                .unwrap_err(),
            DbError::CloseFoundationPrecondition(_)
        ));
    }

    #[tokio::test]
    async fn awaiting_continuation_on_any_member_is_rejected() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        create_child(&db, "mid", "root").await;
        create_child(&db, "leaf", "mid").await;
        set_state(&db, "mid", awaiting_continuation_state()).await;

        let err = db
            .begin_close_foundation(&product_id("root"), &transcript_id("leaf"), "attempt-1")
            .await
            .unwrap_err();
        assert!(matches!(err, DbError::CloseFoundationPrecondition(_)));
    }

    #[tokio::test]
    async fn eligible_latest_states_admit() {
        let db = Database::open_in_memory().await.unwrap();
        let states = vec![
            ("idle", ConvState::Idle),
            (
                "error",
                ConvState::Error {
                    error_kind: ErrorKind::ServerError,
                    message: "err".to_string(),
                    resets_at: None,
                },
            ),
            ("recoverable", recoverable_failure_state()),
            (
                "context",
                ConvState::ContextExhausted {
                    summary: "summary".to_string(),
                },
            ),
            (
                "question",
                ConvState::AwaitingUserResponse {
                    questions: Vec::new(),
                    tool_use_id: "question-tool".to_string(),
                    request_authority:
                        phoenix_core::domain::sm_state::QuestionRequestAuthority::new(),
                },
            ),
        ];
        for (id, state) in states {
            create_root(&db, id).await;
            set_state(&db, id, state).await;
            let obligation = db
                .begin_close_foundation(
                    &product_id(id),
                    &transcript_id(id),
                    &format!("attempt-{id}"),
                )
                .await
                .unwrap();
            assert_eq!(obligation.product_conversation_id().as_str(), id);
            sqlx::query("UPDATE close_obligations SET phase = 'completed', completed_at = ?2, close_outcome = 'cancelled' WHERE attempt_id = ?1")
                .bind(format!("attempt-{id}"))
                .bind(Utc::now().to_rfc3339())
                .execute(db.pool())
                .await
                .unwrap();
        }
    }

    #[tokio::test]
    async fn explicit_latest_state_blockers_reject() {
        let db = Database::open_in_memory().await.unwrap();

        create_root(&db, "approval").await;
        set_state(&db, "approval", approval_state()).await;
        assert!(matches!(
            db.begin_close_foundation(
                &product_id("approval"),
                &transcript_id("approval"),
                "attempt-approval"
            )
            .await
            .unwrap_err(),
            DbError::CloseFoundationPrecondition(_)
        ));

        create_root(&db, "awaiting").await;
        set_state(&db, "awaiting", awaiting_continuation_state()).await;
        assert!(matches!(
            db.begin_close_foundation(
                &product_id("awaiting"),
                &transcript_id("awaiting"),
                "attempt-awaiting"
            )
            .await
            .unwrap_err(),
            DbError::CloseFoundationPrecondition(_)
        ));
    }

    #[tokio::test]
    async fn stale_latest_token_is_rejected_atomically_under_begin_immediate() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        create_child(&db, "latest", "root").await;

        let stale = db
            .begin_close_foundation(&product_id("root"), &transcript_id("root"), "attempt-stale")
            .await
            .unwrap_err();
        assert!(matches!(
            stale,
            DbError::CloseFoundationStaleLatest { expected, actual }
                if expected == "root" && actual == "latest"
        ));

        let obligation = db
            .begin_close_foundation(
                &product_id("root"),
                &transcript_id("latest"),
                "attempt-fresh",
            )
            .await
            .unwrap();
        assert_eq!(obligation.attempt_id().as_str(), "attempt-fresh");
    }

    #[tokio::test]
    async fn idempotent_same_attempt_survives_latest_mutation_and_conflict_different_attempt() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        create_child(&db, "latest", "root").await;

        let first = db
            .begin_close_foundation(&product_id("root"), &transcript_id("latest"), "attempt-1")
            .await
            .unwrap();

        set_state(&db, "latest", ConvState::LlmRequesting { attempt: 2 }).await;

        let second = db
            .begin_close_foundation(&product_id("root"), &transcript_id("latest"), "attempt-1")
            .await
            .unwrap();
        assert_eq!(first, second);

        assert!(sqlx::query(
            "UPDATE conversations SET continued_in_conv_id = 'new-latest' WHERE id = 'latest'",
        )
        .execute(db.pool())
        .await
        .is_err());
        let latest_scope = db
            .get_conversation("latest")
            .await
            .unwrap()
            .attached_work_scope_id
            .unwrap();
        assert!(sqlx::query(
            "INSERT INTO conversations (
                 id, title, cwd, active_model_id, state_json, created_at, updated_at,
                 archived, version, runtime_role, work_scope_id, continued_in_conv_id
             ) VALUES (
                 'new-predecessor', 'new-predecessor', '/tmp', 'model',
                 '{\"type\":\"awaiting_user_input\"}', ?1, ?1, 0, 0, 'user', ?2, 'latest'
             )",
        )
        .bind(Utc::now().to_rfc3339())
        .bind(latest_scope.as_str())
        .execute(db.pool())
        .await
        .is_err());
        assert!(
            sqlx::query("UPDATE conversations SET work_scope_id = NULL WHERE id = 'latest'")
                .execute(db.pool())
                .await
                .is_err()
        );
        assert_eq!(
            db.get_conversation("latest")
                .await
                .unwrap()
                .attached_work_scope_id,
            Some(latest_scope.clone())
        );
        assert!(
            sqlx::query("UPDATE work_scopes SET worktree_path = '/tmp/rebound' WHERE id = ?1",)
                .bind(latest_scope.as_str())
                .execute(db.pool())
                .await
                .is_err()
        );
        let third = db
            .begin_close_foundation(&product_id("root"), &transcript_id("latest"), "attempt-1")
            .await
            .unwrap();
        assert_eq!(first, third);

        let err = db
            .begin_close_foundation(&product_id("root"), &transcript_id("latest"), "attempt-2")
            .await
            .unwrap_err();
        assert!(matches!(err, DbError::CloseFoundationConflict(_)));
    }

    #[tokio::test]
    async fn idempotent_begin_rejects_incomplete_unsealed_topology() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        create_child(&db, "latest", "root").await;
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO close_obligations (
                 attempt_id, product_conversation_id, phase, created_at, updated_at
             ) VALUES (
                 'attempt-partial', 'root', 'awaiting_blocker_resolution', ?1, ?1
             )",
        )
        .bind(&now)
        .execute(db.pool())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO close_attempt_members (
                 attempt_id, conversation_id, member_role, continuation_ordinal,
                 captured_continued_in_conv_id, captured_state_kind, captured_runtime_role,
                 captured_work_scope_id, captured_at
             )
             SELECT 'attempt-partial', id, 'latest', 1, continued_in_conv_id,
                    state_kind, runtime_role, work_scope_id, ?1
             FROM conversations WHERE id = 'latest'",
        )
        .bind(&now)
        .execute(db.pool())
        .await
        .unwrap();

        let error = db
            .begin_close_foundation(
                &product_id("root"),
                &transcript_id("latest"),
                "attempt-partial",
            )
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            DbError::CloseFoundationConflict(message)
                if message.contains("does not have a complete sealed topology")
        ));
    }

    #[tokio::test]
    async fn begin_close_returns_error_for_nul_worktree_path() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        let scope = WorkScopeId::parse("scope-nul").unwrap();
        sqlx::query(
            "INSERT INTO work_scopes (
                 id, authority_kind, created_at, updated_at,
                 environment_kind, cwd, worktree_path, worktree_id, worktree_fingerprint
             ) VALUES (
                 ?1, 'work', '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z',
                 'allocated_worktree', '/tmp/nul', CAST(X'610062' AS TEXT),
                 'nul-worktree', 'nul-fingerprint'
             )",
        )
        .bind(scope.as_str())
        .execute(db.pool())
        .await
        .unwrap();
        sqlx::query("UPDATE conversations SET work_scope_id = ?1 WHERE id = 'root'")
            .bind(scope.as_str())
            .execute(db.pool())
            .await
            .unwrap();

        let error = db
            .begin_close_foundation(&product_id("root"), &transcript_id("root"), "attempt-nul")
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            DbError::Sqlx(_) | DbError::Serialization(_)
        ));
    }

    #[tokio::test]
    async fn snapshots_capture_roles_and_distinct_scopes() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        create_child(&db, "mid", "root").await;
        create_child(&db, "leaf", "mid").await;
        let root_scope = db
            .get_conversation("root")
            .await
            .unwrap()
            .attached_work_scope_id
            .unwrap();
        let synthetic_scope = WorkScopeId::parse("close-scope-synthetic").unwrap();
        sqlx::query(
            "INSERT INTO work_scopes (
                id, authority_kind, lifecycle, environment_kind, cwd,
                worktree_path, branch_name, base_branch, created_at, updated_at,
                worktree_id, worktree_fingerprint
             ) VALUES (
                ?1, 'work', 'active', 'allocated_worktree', '/tmp', '/tmp/worktree',
                'branch', 'main', ?2, ?2, lower(hex(randomblob(16))), lower(hex(randomblob(32)))
             )",
        )
        .bind(synthetic_scope.as_str())
        .bind(Utc::now().to_rfc3339())
        .execute(db.pool())
        .await
        .unwrap();
        sqlx::query("UPDATE conversations SET work_scope_id = ?1 WHERE id = 'leaf'")
            .bind(synthetic_scope.as_str())
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query("UPDATE conversations SET work_scope_id = ?1 WHERE id = 'mid'")
            .bind(root_scope.as_str())
            .execute(db.pool())
            .await
            .unwrap();

        db.begin_close_foundation(&product_id("root"), &transcript_id("leaf"), "attempt-1")
            .await
            .unwrap();
        let members = db.list_close_attempt_members("attempt-1").await.unwrap();
        assert_eq!(members.len(), 3);
        assert_eq!(members[0].role, CloseMemberRole::Root);
        assert_eq!(members[1].role, CloseMemberRole::Intermediate);
        assert_eq!(members[2].role, CloseMemberRole::Latest);

        let scopes = db.list_close_attempt_scopes("attempt-1").await.unwrap();
        assert_eq!(scopes.len(), 2);
        assert_ne!(scopes[0].scope, scopes[1].scope);
    }

    #[tokio::test]
    async fn active_close_seals_live_topology_and_preserves_snapshots() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        create_child(&db, "leaf", "root").await;
        db.begin_close_foundation(&product_id("root"), &transcript_id("leaf"), "attempt-1")
            .await
            .unwrap();

        assert!(sqlx::query(
            "UPDATE conversations SET continued_in_conv_id = 'later' WHERE id = 'leaf'",
        )
        .execute(db.pool())
        .await
        .is_err());
        let live = db
            .close_foundation_topology(&product_id("root"))
            .await
            .unwrap();
        assert_eq!(live.member_ids(), vec!["root", "leaf"]);

        let snapshots = db.list_close_attempt_members("attempt-1").await.unwrap();
        let ids: Vec<_> = snapshots
            .iter()
            .map(|member| member.conversation_id.as_str().to_string())
            .collect();
        assert_eq!(ids, vec!["root".to_string(), "leaf".to_string()]);
    }

    #[tokio::test]
    async fn open_aggregate_ignores_archived_member_drift() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        create_child(&db, "latest", "root").await;
        set_archived(&db, "latest", true).await;
        db.begin_close_foundation(
            &product_id("root"),
            &transcript_id("latest"),
            "attempt-archived-member",
        )
        .await
        .expect("aggregate lifecycle, not legacy archived drift, owns Close admission");
    }

    #[tokio::test]
    async fn archived_root_non_user_and_non_user_initiated_are_rejected() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "subagent-parent").await;
        let parent = db.get_conversation("subagent-parent").await.unwrap();
        db.create_conversation_with_project(
            "subagent",
            "subagent",
            "/tmp",
            false,
            Some(&parent.id),
            None,
            None,
            &phoenix_core::domain::db_schema::ConvMode::Explore {
                worktree_path: None,
                next_taskmd_id_hint: None,
            },
            None,
            None,
            None,
            phoenix_core::llm_language::LlmLanguage::default(),
        )
        .await
        .unwrap();
        assert!(db
            .begin_close_foundation(
                &parent.product_conversation_id,
                &transcript_id(&parent.id),
                "attempt-b"
            )
            .await
            .is_ok());
        let member_ids: Vec<String> = db
            .list_close_attempt_members("attempt-b")
            .await
            .unwrap()
            .into_iter()
            .map(|member| member.conversation_id.as_str().to_string())
            .collect();
        assert_eq!(member_ids, vec!["subagent-parent"]);

        create_root(&db, "not-user-init").await;
        set_user_initiated(&db, "not-user-init", false).await;
        assert!(matches!(
            db.begin_close_foundation(
                &product_id("not-user-init"),
                &transcript_id("not-user-init"),
                "attempt-c"
            )
            .await
            .unwrap_err(),
            DbError::CloseFoundationPrecondition(_)
        ));
    }

    #[tokio::test]
    async fn topology_rejects_cycle_deterministically() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        create_child(&db, "mid", "root").await;
        create_child(&db, "leaf", "mid").await;
        assert!(sqlx::query(
            "UPDATE conversations SET continued_in_conv_id = 'root' WHERE id = 'leaf'",
        )
        .execute(db.pool())
        .await
        .is_err());

        let topology = db
            .close_foundation_topology(&product_id("root"))
            .await
            .unwrap();
        assert_eq!(topology.member_ids(), vec!["root", "mid", "leaf"]);
    }

    #[tokio::test]
    async fn topology_rejects_cross_aggregate_fork_deterministically() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        create_child(&db, "leaf", "root").await;
        create_root(&db, "fork").await;
        assert!(sqlx::query(
            "UPDATE conversations SET continued_in_conv_id = 'leaf' WHERE id = 'fork'"
        )
        .execute(db.pool())
        .await
        .is_err());
    }

    #[tokio::test]
    async fn replace_close_inspection_returns_typed_not_found() {
        let db = Database::open_in_memory().await.unwrap();
        let error = db
            .replace_close_inspection(ReplaceCloseInspectionRequest {
                attempt_id: CloseAttemptId::parse("missing-attempt").unwrap(),
                scopes: Vec::new(),
            })
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            DbError::CloseFoundationNotFound(attempt_id) if attempt_id == "missing-attempt"
        ));
    }

    #[tokio::test]
    async fn replace_close_inspection_round_trips_exact_identities_and_aggregate_pair() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        create_child(&db, "leaf", "root").await;
        let scope = allocate_scope_worktree(&db, "root").await;
        db.begin_close_foundation(&product_id("root"), &transcript_id("leaf"), "attempt-1")
            .await
            .unwrap();
        set_close_phase(&db, "attempt-1", ClosePhase::AwaitingRetirementInspection).await;
        let scope_snapshot = CloseRetirementSnapshot::parse("scope-gen", "scope-fp").unwrap();
        let non_utf8_path = GitPathIdentity::from_bytes(vec![0x66, 0x6f, 0x80, 0x2f, 0xff]);
        let lossy_left = GitPathIdentity::from_bytes(b"a\x80".to_vec());
        let lossy_right = GitPathIdentity::from_bytes("a\u{fffd}".as_bytes().to_vec());
        let oid = GitOidIdentity::parse_hex("1234567890123456789012345678901234567890").unwrap();

        db.replace_close_inspection(ReplaceCloseInspectionRequest {
            attempt_id: CloseAttemptId::parse("attempt-1").unwrap(),
            scopes: vec![ReplaceCloseInspectionScopeRequest {
                scope: scope.clone(),
                snapshot: scope_snapshot.clone(),
                losses: vec![
                    CloseLossItem::UntrackedNonIgnoredPath(non_utf8_path.clone()),
                    CloseLossItem::StagedTrackedPath(lossy_left.clone()),
                    CloseLossItem::StagedTrackedPath(lossy_right.clone()),
                    CloseLossItem::DetachedUnreachableCommit(oid.clone()),
                ],
            }],
        })
        .await
        .unwrap();

        let obligation = db.get_close_obligation("attempt-1").await.unwrap();
        assert!(obligation.snapshot().is_some());

        let inspections = db
            .list_close_retirement_inspections("attempt-1")
            .await
            .unwrap();
        assert_eq!(inspections.len(), 1);
        assert_eq!(inspections[0].target.scope, scope);
        assert_eq!(inspections[0].snapshot.clone(), scope_snapshot.clone());

        let losses = db.list_close_retirement_losses("attempt-1").await.unwrap();
        assert_eq!(losses.len(), 4);
        assert!(losses.iter().all(|loss| loss.snapshot == scope_snapshot));
        assert!(losses.iter().any(|loss| {
            loss.item == CloseLossItem::UntrackedNonIgnoredPath(non_utf8_path.clone())
        }));
        assert!(losses
            .iter()
            .any(|loss| loss.item == CloseLossItem::StagedTrackedPath(lossy_left.clone())));
        assert!(losses
            .iter()
            .any(|loss| loss.item == CloseLossItem::StagedTrackedPath(lossy_right.clone())));
        assert!(losses
            .iter()
            .any(|loss| { loss.item == CloseLossItem::DetachedUnreachableCommit(oid.clone()) }));
    }

    #[tokio::test]
    async fn concurrent_identical_inspection_replacements_are_idempotent() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        let scope = allocate_scope_worktree(&db, "root").await;
        db.begin_close_foundation(
            &product_id("root"),
            &transcript_id("root"),
            "attempt-concurrent-inspection",
        )
        .await
        .unwrap();
        set_close_phase(
            &db,
            "attempt-concurrent-inspection",
            ClosePhase::AwaitingRetirementInspection,
        )
        .await;
        let request = ReplaceCloseInspectionRequest {
            attempt_id: CloseAttemptId::parse("attempt-concurrent-inspection").unwrap(),
            scopes: vec![ReplaceCloseInspectionScopeRequest {
                scope,
                snapshot: CloseRetirementSnapshot::parse("g1", "fp1").unwrap(),
                losses: Vec::new(),
            }],
        };

        let (first, second) = tokio::join!(
            db.replace_close_inspection(request.clone()),
            db.replace_close_inspection(request)
        );
        first.unwrap();
        second.unwrap();
        assert_eq!(
            db.list_close_retirement_inspections("attempt-concurrent-inspection")
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn list_close_retirement_losses_rejects_invalid_category_identity_pairing() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        create_child(&db, "leaf", "root").await;
        allocate_scope_worktree(&db, "root").await;

        let scope = db
            .get_conversation("root")
            .await
            .unwrap()
            .attached_work_scope_id
            .unwrap();
        db.begin_close_foundation(&product_id("root"), &transcript_id("leaf"), "attempt-1")
            .await
            .unwrap();
        set_close_phase(&db, "attempt-1", ClosePhase::AwaitingRetirementInspection).await;
        sqlx::query(
            "INSERT INTO close_retirement_inspections (attempt_id, scope, generation, fingerprint, inspected_at)
             VALUES ('attempt-1', ?1, 'g1', 'fp1', ?2)",
        )
        .bind(scope.as_str())
        .bind(Utc::now().to_rfc3339())
        .execute(db.pool())
        .await
        .unwrap();
        let err = sqlx::query(
            "INSERT INTO close_retirement_losses (
                attempt_id, scope, generation, category, identity_kind, identity_codec, identity_value
             ) VALUES (?1, ?2, 'g1', 'detached_unreachable_commits', 'git_path', 'git_path_bytes_hex_v1', 'git_path_bytes_hex_v1:737263')",
        )
        .bind("attempt-1")
        .bind(scope.as_str())
        .execute(db.pool())
        .await
        .unwrap_err();
        assert!(err.to_string().contains("CHECK constraint failed"));
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn inspection_reentry_invalidates_prior_snapshot_and_rows() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        let scope = allocate_scope_worktree(&db, "root").await;
        db.begin_close_foundation(
            &product_id("root"),
            &transcript_id("root"),
            "attempt-reentry",
        )
        .await
        .unwrap();
        set_close_phase(
            &db,
            "attempt-reentry",
            ClosePhase::AwaitingRetirementInspection,
        )
        .await;

        let snapshot = CloseRetirementSnapshot::parse("scope-gen", "scope-fp").unwrap();
        let loss = CloseLossItem::UntrackedNonIgnoredPath(GitPathIdentity::from_bytes(
            b"stale-path".to_vec(),
        ));
        let replacement = || ReplaceCloseInspectionRequest {
            attempt_id: CloseAttemptId::parse("attempt-reentry").unwrap(),
            scopes: vec![ReplaceCloseInspectionScopeRequest {
                scope: scope.clone(),
                snapshot: snapshot.clone(),
                losses: vec![loss.clone()],
            }],
        };

        db.replace_close_inspection(replacement()).await.unwrap();
        let prior_obligation = db.get_close_obligation("attempt-reentry").await.unwrap();
        assert_eq!(
            prior_obligation.phase(),
            ClosePhase::AwaitingLossConfirmation
        );
        assert!(prior_obligation.snapshot().is_some());
        assert_eq!(
            db.list_close_retirement_inspections("attempt-reentry")
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            db.list_close_retirement_losses("attempt-reentry")
                .await
                .unwrap()
                .len(),
            1
        );

        sqlx::query(
            "UPDATE close_obligations
             SET phase = 'awaiting_retirement_inspection'
             WHERE attempt_id = 'attempt-reentry'",
        )
        .execute(db.pool())
        .await
        .unwrap();

        let reentered = db.get_close_obligation("attempt-reentry").await.unwrap();
        assert_eq!(reentered.phase(), ClosePhase::AwaitingRetirementInspection);
        assert!(reentered.snapshot().is_none());
        assert!(db
            .list_close_retirement_inspections("attempt-reentry")
            .await
            .unwrap()
            .is_empty());
        assert!(db
            .list_close_retirement_losses("attempt-reentry")
            .await
            .unwrap()
            .is_empty());

        sqlx::query(
            "UPDATE close_obligations
             SET phase = 'retirement_requested'
             WHERE attempt_id = 'attempt-reentry'",
        )
        .execute(db.pool())
        .await
        .unwrap_err();
        let still_reentered = db.get_close_obligation("attempt-reentry").await.unwrap();
        assert_eq!(
            still_reentered.phase(),
            ClosePhase::AwaitingRetirementInspection
        );
        assert!(still_reentered.snapshot().is_none());

        db.replace_close_inspection(replacement()).await.unwrap();
        let refreshed = db.get_close_obligation("attempt-reentry").await.unwrap();
        assert_eq!(refreshed.phase(), ClosePhase::AwaitingLossConfirmation);
        assert!(refreshed.snapshot().is_some());
        assert_eq!(
            db.list_close_retirement_inspections("attempt-reentry")
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            db.list_close_retirement_losses("attempt-reentry")
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn changed_loss_confirmation_inspection_replaces_evidence_and_token_atomically() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        let scope = allocate_scope_worktree(&db, "root").await;
        db.begin_close_foundation(
            &product_id("root"),
            &transcript_id("root"),
            "attempt-changed-confirmation",
        )
        .await
        .unwrap();
        set_close_phase(
            &db,
            "attempt-changed-confirmation",
            ClosePhase::AwaitingRetirementInspection,
        )
        .await;

        let first_loss = CloseLossItem::UntrackedNonIgnoredPath(GitPathIdentity::from_bytes(
            b"first-path".to_vec(),
        ));
        db.replace_close_inspection(ReplaceCloseInspectionRequest {
            attempt_id: CloseAttemptId::parse("attempt-changed-confirmation").unwrap(),
            scopes: vec![ReplaceCloseInspectionScopeRequest {
                scope: scope.clone(),
                snapshot: CloseRetirementSnapshot::parse("first-generation", "first-fingerprint")
                    .unwrap(),
                losses: vec![first_loss],
            }],
        })
        .await
        .unwrap();
        let first_token = db
            .get_close_obligation("attempt-changed-confirmation")
            .await
            .unwrap()
            .snapshot()
            .unwrap()
            .clone();

        let second_loss =
            CloseLossItem::StagedTrackedPath(GitPathIdentity::from_bytes(b"second-path".to_vec()));
        db.replace_close_inspection(ReplaceCloseInspectionRequest {
            attempt_id: CloseAttemptId::parse("attempt-changed-confirmation").unwrap(),
            scopes: vec![ReplaceCloseInspectionScopeRequest {
                scope,
                snapshot: CloseRetirementSnapshot::parse("second-generation", "second-fingerprint")
                    .unwrap(),
                losses: vec![second_loss.clone()],
            }],
        })
        .await
        .unwrap();

        let refreshed = db
            .get_close_obligation("attempt-changed-confirmation")
            .await
            .unwrap();
        assert_eq!(refreshed.phase(), ClosePhase::AwaitingLossConfirmation);
        let second_token = refreshed.snapshot().unwrap().clone();
        assert_ne!(first_token, second_token);
        assert_eq!(
            db.list_close_retirement_losses("attempt-changed-confirmation")
                .await
                .unwrap()
                .into_iter()
                .map(|loss| loss.item)
                .collect::<Vec<_>>(),
            vec![second_loss]
        );

        let attempt_id = CloseAttemptId::parse("attempt-changed-confirmation").unwrap();
        let stale = db
            .confirm_close_loss_retirement(&attempt_id, &first_token)
            .await
            .unwrap_err();
        assert!(stale.to_string().contains("snapshot is stale"));
        let confirmed = db
            .confirm_close_loss_retirement(&attempt_id, &second_token)
            .await
            .unwrap();
        assert_eq!(confirmed.phase(), ClosePhase::RetirementRequested);
    }

    #[tokio::test]
    async fn replace_close_inspection_clean_scopes_skip_loss_confirmation() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        create_child(&db, "leaf", "root").await;
        let scope = allocate_scope_worktree(&db, "root").await;
        db.begin_close_foundation(&product_id("root"), &transcript_id("leaf"), "attempt-clean")
            .await
            .unwrap();
        set_close_phase(
            &db,
            "attempt-clean",
            ClosePhase::AwaitingRetirementInspection,
        )
        .await;

        db.replace_close_inspection(ReplaceCloseInspectionRequest {
            attempt_id: CloseAttemptId::parse("attempt-clean").unwrap(),
            scopes: vec![ReplaceCloseInspectionScopeRequest {
                scope,
                snapshot: CloseRetirementSnapshot::parse("scope-clean", "scope-clean-fp").unwrap(),
                losses: Vec::new(),
            }],
        })
        .await
        .unwrap();

        let obligation = db.get_close_obligation("attempt-clean").await.unwrap();
        assert_eq!(obligation.phase(), ClosePhase::RetirementRequested);
        assert!(obligation.snapshot().is_some());
        assert!(db
            .list_close_retirement_losses("attempt-clean")
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn replace_close_inspection_no_scopes_skip_loss_confirmation() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        db.begin_close_foundation(&product_id("root"), &transcript_id("root"), "attempt-empty")
            .await
            .unwrap();
        set_close_phase(
            &db,
            "attempt-empty",
            ClosePhase::AwaitingRetirementInspection,
        )
        .await;
        db.replace_close_inspection(ReplaceCloseInspectionRequest {
            attempt_id: CloseAttemptId::parse("attempt-empty").unwrap(),
            scopes: Vec::new(),
        })
        .await
        .unwrap();

        let obligation = db.get_close_obligation("attempt-empty").await.unwrap();
        assert_eq!(obligation.phase(), ClosePhase::RetirementRequested);
        let snapshot = obligation.snapshot().expect("no-worktree snapshot");
        assert_eq!(snapshot.generation(), "no-worktree");
        assert_eq!(snapshot.fingerprint(), "no-worktree");
    }

    #[tokio::test]
    async fn retry_no_worktree_inspection_rotates_inventory_generation() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        db.begin_close_foundation(
            &product_id("root"),
            &transcript_id("root"),
            "attempt-empty-retry",
        )
        .await
        .unwrap();
        set_close_phase(
            &db,
            "attempt-empty-retry",
            ClosePhase::AwaitingRetirementInspection,
        )
        .await;
        db.replace_close_inspection_with_empty_generation(
            ReplaceCloseInspectionRequest {
                attempt_id: CloseAttemptId::parse("attempt-empty-retry").unwrap(),
                scopes: Vec::new(),
            },
            Some("server_git_status_v2_retry_test"),
        )
        .await
        .unwrap();

        let obligation = db
            .get_close_obligation("attempt-empty-retry")
            .await
            .unwrap();
        let snapshot = obligation.snapshot().expect("rotated no-worktree snapshot");
        assert_eq!(snapshot.generation(), "server_git_status_v2_retry_test");
        assert_eq!(snapshot.fingerprint(), "no-worktree");
    }

    #[tokio::test]
    async fn replace_close_inspection_requires_only_allocated_worktree_scopes() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        create_child(&db, "mid", "root").await;
        create_child(&db, "leaf", "mid").await;

        let root_scope = allocate_scope_worktree(&db, "root").await;
        let unowned_scope = WorkScopeId::parse("close-scope-unowned").unwrap();
        let none_scope = WorkScopeId::parse("close-scope-none").unwrap();
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO work_scopes (
                id, authority_kind, lifecycle, environment_kind, cwd,
                worktree_path, branch_name, base_branch, created_at, updated_at
             ) VALUES
                (?1, 'work', 'active', 'unowned_cwd', '/tmp', NULL, NULL, NULL, ?3, ?3),
                (?2, 'work', 'active', 'none', NULL, NULL, NULL, NULL, ?3, ?3)",
        )
        .bind(unowned_scope.as_str())
        .bind(none_scope.as_str())
        .bind(&now)
        .execute(db.pool())
        .await
        .unwrap();
        sqlx::query("UPDATE conversations SET work_scope_id = ?1 WHERE id = 'mid'")
            .bind(unowned_scope.as_str())
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query("UPDATE conversations SET work_scope_id = ?1 WHERE id = 'leaf'")
            .bind(none_scope.as_str())
            .execute(db.pool())
            .await
            .unwrap();

        let leaf_scope = db
            .get_conversation("leaf")
            .await
            .unwrap()
            .attached_work_scope_id
            .unwrap();
        assert_eq!(leaf_scope, none_scope);

        db.begin_close_foundation(&product_id("root"), &transcript_id("leaf"), "attempt-1")
            .await
            .unwrap();
        let captured = db.list_close_attempt_scopes("attempt-1").await.unwrap();
        assert_eq!(captured.len(), 3);

        set_close_phase(&db, "attempt-1", ClosePhase::AwaitingRetirementInspection).await;
        db.replace_close_inspection(ReplaceCloseInspectionRequest {
            attempt_id: CloseAttemptId::parse("attempt-1").unwrap(),
            scopes: vec![ReplaceCloseInspectionScopeRequest {
                scope: root_scope,
                snapshot: CloseRetirementSnapshot::parse("scope-root", "scope-root-fp").unwrap(),
                losses: Vec::new(),
            }],
        })
        .await
        .unwrap();
    }

    #[test]
    fn aggregate_snapshot_encoding_is_injective_across_delimiter_collisions() {
        let first = WorkScopeId::parse("scope-a").unwrap();
        let second = WorkScopeId::parse("scope-b").unwrap();
        assert_ne!(
            encode_aggregate_snapshot_component([(&first, "a\u{1f}b"), (&second, "c")]),
            encode_aggregate_snapshot_component([(&first, "a"), (&second, "b\u{1f}c")]),
        );
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn replace_close_inspection_supports_multi_scope_fingerprints_and_replacement_clears_stale(
    ) {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        create_child(&db, "mid", "root").await;
        create_child(&db, "leaf", "mid").await;
        let root_scope = allocate_scope_worktree(&db, "root").await;
        let leaf_scope = WorkScopeId::parse("close-scope-other").unwrap();
        sqlx::query(
            "INSERT INTO work_scopes (
                id, authority_kind, lifecycle, environment_kind, cwd,
                worktree_path, branch_name, base_branch, created_at, updated_at,
                worktree_id, worktree_fingerprint
             ) VALUES (
                ?1, 'work', 'active', 'allocated_worktree', '/tmp', '/tmp/other',
                'branch', 'main', ?2, ?2, lower(hex(randomblob(16))), lower(hex(randomblob(32)))
             )",
        )
        .bind(leaf_scope.as_str())
        .bind(Utc::now().to_rfc3339())
        .execute(db.pool())
        .await
        .unwrap();
        sqlx::query("UPDATE conversations SET work_scope_id = ?1 WHERE id = 'leaf'")
            .bind(leaf_scope.as_str())
            .execute(db.pool())
            .await
            .unwrap();

        db.begin_close_foundation(&product_id("root"), &transcript_id("leaf"), "attempt-1")
            .await
            .unwrap();
        set_close_phase(&db, "attempt-1", ClosePhase::AwaitingRetirementInspection).await;

        db.replace_close_inspection(ReplaceCloseInspectionRequest {
            attempt_id: CloseAttemptId::parse("attempt-1").unwrap(),
            scopes: vec![
                ReplaceCloseInspectionScopeRequest {
                    scope: leaf_scope.clone(),
                    snapshot: CloseRetirementSnapshot::parse("gen-leaf-1", "scope-leaf-one")
                        .unwrap(),
                    losses: vec![CloseLossItem::UntrackedNonIgnoredPath(
                        GitPathIdentity::from_bytes(b"leaf:stale".to_vec()),
                    )],
                },
                ReplaceCloseInspectionScopeRequest {
                    scope: root_scope.clone(),
                    snapshot: CloseRetirementSnapshot::parse("gen-root-0", "scope-root-zero")
                        .unwrap(),
                    losses: vec![CloseLossItem::InitializedSubmoduleState(
                        GitPathIdentity::from_bytes(b"submodule:stale".to_vec()),
                    )],
                },
            ],
        })
        .await
        .unwrap();

        sqlx::query(
            "UPDATE close_obligations
             SET phase = 'awaiting_retirement_inspection'
             WHERE attempt_id = 'attempt-1'",
        )
        .execute(db.pool())
        .await
        .unwrap();

        db.replace_close_inspection(ReplaceCloseInspectionRequest {
            attempt_id: CloseAttemptId::parse("attempt-1").unwrap(),
            scopes: vec![
                ReplaceCloseInspectionScopeRequest {
                    scope: leaf_scope.clone(),
                    snapshot: CloseRetirementSnapshot::parse("gen-leaf-2", "scope-leaf-two")
                        .unwrap(),
                    losses: vec![CloseLossItem::UntrackedNonIgnoredPath(
                        GitPathIdentity::from_bytes(b"leaf:fresh".to_vec()),
                    )],
                },
                ReplaceCloseInspectionScopeRequest {
                    scope: root_scope.clone(),
                    snapshot: CloseRetirementSnapshot::parse("gen-root-1", "scope-root-one")
                        .unwrap(),
                    losses: vec![CloseLossItem::InitializedSubmoduleState(
                        GitPathIdentity::from_bytes(b"submodule:one".to_vec()),
                    )],
                },
            ],
        })
        .await
        .unwrap();

        let obligation = db.get_close_obligation("attempt-1").await.unwrap();
        let expected_snapshot = if root_scope < leaf_scope {
            CloseRetirementSnapshot::parse(
                encode_aggregate_snapshot_component([
                    (&root_scope, "gen-root-1"),
                    (&leaf_scope, "gen-leaf-2"),
                ]),
                encode_aggregate_snapshot_component([
                    (&root_scope, "scope-root-one"),
                    (&leaf_scope, "scope-leaf-two"),
                ]),
            )
        } else {
            CloseRetirementSnapshot::parse(
                encode_aggregate_snapshot_component([
                    (&leaf_scope, "gen-leaf-2"),
                    (&root_scope, "gen-root-1"),
                ]),
                encode_aggregate_snapshot_component([
                    (&leaf_scope, "scope-leaf-two"),
                    (&root_scope, "scope-root-one"),
                ]),
            )
        }
        .unwrap();
        assert_eq!(obligation.snapshot().unwrap(), &expected_snapshot);
        let inspections = db
            .list_close_retirement_inspections("attempt-1")
            .await
            .unwrap();
        assert_eq!(inspections.len(), 2);
        assert!(inspections
            .iter()
            .any(|i| i.target.scope == root_scope && i.snapshot.fingerprint() == "scope-root-one"));
        assert!(inspections
            .iter()
            .any(|i| i.target.scope == leaf_scope && i.snapshot.fingerprint() == "scope-leaf-two"));
        let losses = db.list_close_retirement_losses("attempt-1").await.unwrap();
        assert_eq!(losses.len(), 2);
        assert!(!losses.iter().any(|loss| matches!(&loss.item, CloseLossItem::UntrackedNonIgnoredPath(v) if v.as_bytes() == b"leaf:stale")));
        assert!(losses.iter().any(|loss| matches!(&loss.item, CloseLossItem::UntrackedNonIgnoredPath(v) if v.as_bytes() == b"leaf:fresh")));
        let evidence = db
            .list_close_retirement_evidence("attempt-1")
            .await
            .unwrap();
        assert!(evidence.is_empty());
    }

    #[tokio::test]
    async fn replace_close_inspection_rejects_incomplete_scope_set_without_mutation() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        create_child(&db, "leaf", "root").await;
        let root_scope = allocate_scope_worktree(&db, "root").await;
        let leaf_scope = WorkScopeId::parse("close-scope-other").unwrap();
        sqlx::query(
            "INSERT INTO work_scopes (
                id, authority_kind, lifecycle, environment_kind, cwd,
                worktree_path, branch_name, base_branch, created_at, updated_at,
                worktree_id, worktree_fingerprint
             ) VALUES (
                ?1, 'work', 'active', 'allocated_worktree', '/tmp', '/tmp/other',
                'branch', 'main', ?2, ?2, lower(hex(randomblob(16))), lower(hex(randomblob(32)))
             )",
        )
        .bind(leaf_scope.as_str())
        .bind(Utc::now().to_rfc3339())
        .execute(db.pool())
        .await
        .unwrap();
        sqlx::query("UPDATE conversations SET work_scope_id = ?1 WHERE id = 'leaf'")
            .bind(leaf_scope.as_str())
            .execute(db.pool())
            .await
            .unwrap();

        db.begin_close_foundation(&product_id("root"), &transcript_id("leaf"), "attempt-1")
            .await
            .unwrap();
        set_close_phase(&db, "attempt-1", ClosePhase::AwaitingRetirementInspection).await;
        sqlx::query(
            "INSERT INTO close_retirement_inspections (attempt_id, scope, generation, fingerprint, inspected_at)
             VALUES ('attempt-1', ?1, 'old-gen', 'old-fp', ?2)"
        )
        .bind(root_scope.as_str())
        .bind(Utc::now().to_rfc3339())
        .execute(db.pool()).await.unwrap();

        let err = db
            .replace_close_inspection(ReplaceCloseInspectionRequest {
                attempt_id: CloseAttemptId::parse("attempt-1").unwrap(),
                scopes: vec![ReplaceCloseInspectionScopeRequest {
                    scope: root_scope.clone(),
                    snapshot: CloseRetirementSnapshot::parse("new-root", "new-root-fp").unwrap(),
                    losses: Vec::new(),
                }],
            })
            .await
            .unwrap_err();
        assert!(matches!(err, DbError::CloseFoundationPrecondition(_)));

        let obligation = db.get_close_obligation("attempt-1").await.unwrap();
        assert_eq!(obligation.phase(), ClosePhase::AwaitingRetirementInspection);
        assert!(obligation.snapshot().is_none());
        let inspections = db
            .list_close_retirement_inspections("attempt-1")
            .await
            .unwrap();
        assert_eq!(inspections.len(), 1);
        assert_eq!(inspections[0].target.scope, root_scope);
        assert_eq!(inspections[0].snapshot.generation(), "old-gen");
    }

    #[tokio::test]
    async fn replace_close_inspection_rejects_untargeted_scope() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        db.begin_close_foundation(&product_id("root"), &transcript_id("root"), "attempt-1")
            .await
            .unwrap();
        set_close_phase(&db, "attempt-1", ClosePhase::AwaitingRetirementInspection).await;
        let other_scope = WorkScopeId::parse("close-scope-other").unwrap();
        sqlx::query(
            "INSERT INTO work_scopes (
                id, authority_kind, lifecycle, environment_kind, cwd,
                worktree_path, branch_name, base_branch, created_at, updated_at,
                worktree_id, worktree_fingerprint
             ) VALUES (
                ?1, 'work', 'active', 'allocated_worktree', '/tmp', '/tmp/other',
                'branch', 'main', ?2, ?2, lower(hex(randomblob(16))), lower(hex(randomblob(32)))
             )",
        )
        .bind(other_scope.as_str())
        .bind(Utc::now().to_rfc3339())
        .execute(db.pool())
        .await
        .unwrap();

        let err = db
            .replace_close_inspection(ReplaceCloseInspectionRequest {
                attempt_id: CloseAttemptId::parse("attempt-1").unwrap(),
                scopes: vec![ReplaceCloseInspectionScopeRequest {
                    scope: other_scope,
                    snapshot: CloseRetirementSnapshot::parse("g", "fp").unwrap(),
                    losses: Vec::new(),
                }],
            })
            .await
            .unwrap_err();
        assert!(matches!(err, DbError::CloseFoundationPrecondition(_)));
    }

    #[tokio::test]
    async fn pre_quarantine_change_returns_to_reinspection_and_uses_new_inventory() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        let scope = allocate_scope_worktree(&db, "root").await;
        db.begin_close_foundation(
            &product_id("root"),
            &transcript_id("root"),
            "attempt-reinspect",
        )
        .await
        .unwrap();
        set_close_phase(
            &db,
            "attempt-reinspect",
            ClosePhase::AwaitingRetirementInspection,
        )
        .await;
        let old_snapshot = CloseRetirementSnapshot::parse("old", "old-fingerprint").unwrap();
        db.replace_close_inspection(ReplaceCloseInspectionRequest {
            attempt_id: CloseAttemptId::parse("attempt-reinspect").unwrap(),
            scopes: vec![ReplaceCloseInspectionScopeRequest {
                scope: scope.clone(),
                snapshot: old_snapshot.clone(),
                losses: Vec::new(),
            }],
        })
        .await
        .unwrap();
        let old_aggregate = current_test_snapshot(&db, "attempt-reinspect").await;
        capture_test_inventory(&db, "attempt-reinspect", &scope, &old_aggregate, Vec::new()).await;

        db.return_close_attempt_to_reinspection(
            &CloseAttemptId::parse("attempt-reinspect").unwrap(),
        )
        .await
        .unwrap();
        let new_snapshot = CloseRetirementSnapshot::parse("new", "new-fingerprint").unwrap();
        db.replace_close_inspection(ReplaceCloseInspectionRequest {
            attempt_id: CloseAttemptId::parse("attempt-reinspect").unwrap(),
            scopes: vec![ReplaceCloseInspectionScopeRequest {
                scope: scope.clone(),
                snapshot: new_snapshot.clone(),
                losses: Vec::new(),
            }],
        })
        .await
        .unwrap();
        let new_aggregate = current_test_snapshot(&db, "attempt-reinspect").await;
        capture_test_inventory(&db, "attempt-reinspect", &scope, &new_aggregate, Vec::new()).await;

        let resources = db
            .list_close_expected_retirement_resources("attempt-reinspect")
            .await
            .unwrap();
        assert!(!resources.is_empty());
        assert!(resources
            .iter()
            .all(|resource| resource.snapshot == new_aggregate));
    }

    #[tokio::test]
    async fn retirement_inventory_rejects_terminal_unarchived_root_on_scope() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        allocate_scope_worktree(&db, "root").await;
        let scope = db
            .get_conversation("root")
            .await
            .unwrap()
            .attached_work_scope_id
            .unwrap();
        create_root(&db, "other-root").await;
        let conflict = sqlx::query("UPDATE conversations SET work_scope_id = ?2 WHERE id = ?1")
            .bind("other-root")
            .bind(scope.as_str())
            .execute(db.pool())
            .await
            .unwrap_err();
        assert!(conflict
            .to_string()
            .contains("different ordinary product conversation"));
        set_state(&db, "other-root", ConvState::Terminal).await;
        db.begin_close_foundation(
            &product_id("root"),
            &transcript_id("root"),
            "attempt-terminal-owner",
        )
        .await
        .unwrap();
        set_close_phase(
            &db,
            "attempt-terminal-owner",
            ClosePhase::RetirementRequested,
        )
        .await;
        let snapshot = current_test_snapshot(&db, "attempt-terminal-owner").await;

        let error = db
            .capture_close_retirement_inventory(CaptureCloseRetirementInventoryRequest {
                attempt_id: CloseAttemptId::parse("attempt-terminal-owner").unwrap(),
                snapshot,
                scopes: vec![CaptureCloseRetirementInventoryScopeRequest {
                    scope,
                    inventory: CloseOwnedResourceInventory {
                        work_scopes: std::collections::BTreeSet::new(),
                        worktree: None,
                        bash_process_groups: std::collections::BTreeSet::default(),
                        tmux_servers: std::collections::BTreeSet::default(),
                        pty_sessions: std::collections::BTreeSet::default(),
                        browser_sessions: std::collections::BTreeSet::default(),
                        equivalent_live_resources: std::collections::BTreeSet::default(),
                    },
                }],
            })
            .await
            .unwrap_err();
        assert!(matches!(error, DbError::CloseFoundationPrecondition(_)));
    }

    #[tokio::test]
    async fn concurrent_identical_retirement_inventory_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("inventory-race.db");
        let db = Database::open(path.to_str().unwrap()).await.unwrap();
        crate::migrations::run_pending_migrations(db.pool())
            .await
            .unwrap();
        create_root(&db, "root").await;
        let scope = allocate_scope_worktree(&db, "root").await;
        db.begin_close_foundation(
            &product_id("root"),
            &transcript_id("root"),
            "attempt-concurrent-inventory",
        )
        .await
        .unwrap();
        set_close_phase(
            &db,
            "attempt-concurrent-inventory",
            ClosePhase::RetirementRequested,
        )
        .await;
        let worktree = current_test_worktree(&db, &scope).await;
        let request = CaptureCloseRetirementInventoryRequest {
            attempt_id: CloseAttemptId::parse("attempt-concurrent-inventory").unwrap(),
            snapshot: current_test_snapshot(&db, "attempt-concurrent-inventory").await,
            scopes: vec![CaptureCloseRetirementInventoryScopeRequest {
                scope,
                inventory: CloseOwnedResourceInventory {
                    worktree: Some(worktree),
                    work_scopes: std::collections::BTreeSet::default(),
                    bash_process_groups: std::collections::BTreeSet::default(),
                    tmux_servers: std::collections::BTreeSet::default(),
                    pty_sessions: std::collections::BTreeSet::default(),
                    browser_sessions: std::collections::BTreeSet::default(),
                    equivalent_live_resources: std::collections::BTreeSet::default(),
                },
            }],
        };

        let (first, second) = tokio::join!(
            db.capture_close_retirement_inventory(request.clone()),
            db.capture_close_retirement_inventory(request)
        );
        assert_eq!(first.unwrap(), second.unwrap());
    }

    #[tokio::test]
    async fn zero_scope_inventory_requires_exact_authorized_snapshot() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        db.begin_close_foundation(&product_id("root"), &transcript_id("root"), "attempt-zero")
            .await
            .unwrap();
        let request = CaptureCloseRetirementInventoryRequest {
            attempt_id: CloseAttemptId::parse("attempt-zero").unwrap(),
            snapshot: CloseRetirementSnapshot::parse("wrong-generation", "wrong-fingerprint")
                .unwrap(),
            scopes: Vec::new(),
        };

        let error = db
            .capture_close_retirement_inventory(request)
            .await
            .unwrap_err();
        assert!(matches!(error, DbError::CloseFoundationPrecondition(_)));
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn retirement_inventory_round_trips_exact_expected_resources() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        allocate_scope_worktree(&db, "root").await;

        let scope = db
            .get_conversation("root")
            .await
            .unwrap()
            .attached_work_scope_id
            .unwrap();
        db.begin_close_foundation(&product_id("root"), &transcript_id("root"), "attempt-1")
            .await
            .unwrap();
        set_close_phase(&db, "attempt-1", ClosePhase::RetirementRequested).await;
        let snapshot = current_test_snapshot(&db, "attempt-1").await;
        let worktree = RetiredResourceIdentity::parse(
            RetiredResourceKind::Worktree,
            LossItemIdentity::Worktree(current_test_worktree(&db, &scope).await),
        )
        .unwrap();

        let resources = db
            .capture_close_retirement_inventory(CaptureCloseRetirementInventoryRequest {
                attempt_id: CloseAttemptId::parse("attempt-1").unwrap(),
                snapshot: snapshot.clone(),
                scopes: vec![CaptureCloseRetirementInventoryScopeRequest {
                    scope: scope.clone(),
                    inventory: CloseOwnedResourceInventory {
                        work_scopes: std::collections::BTreeSet::new(),
                        worktree: match worktree.identity() {
                            LossItemIdentity::Worktree(identity) => Some(identity.clone()),
                            LossItemIdentity::GitPath(_)
                            | LossItemIdentity::GitOid(_)
                            | LossItemIdentity::Opaque(_) => unreachable!(),
                        },
                        bash_process_groups: std::collections::BTreeSet::default(),
                        tmux_servers: std::collections::BTreeSet::default(),
                        pty_sessions: std::collections::BTreeSet::default(),
                        browser_sessions: std::collections::BTreeSet::default(),
                        equivalent_live_resources: std::collections::BTreeSet::default(),
                    },
                }],
            })
            .await
            .unwrap();
        assert_eq!(resources.len(), 2);
        assert!(resources.iter().any(|resource| {
            resource.scope == scope
                && resource.snapshot == snapshot
                && resource.resource == worktree
        }));
        assert!(resources.iter().any(|resource| {
            resource.scope == scope && resource.resource.kind() == RetiredResourceKind::WorkScope
        }));
        let replayed = db
            .capture_close_retirement_inventory(CaptureCloseRetirementInventoryRequest {
                attempt_id: CloseAttemptId::parse("attempt-1").unwrap(),
                snapshot: snapshot.clone(),
                scopes: vec![CaptureCloseRetirementInventoryScopeRequest {
                    scope: scope.clone(),
                    inventory: CloseOwnedResourceInventory {
                        work_scopes: std::collections::BTreeSet::new(),
                        worktree: match worktree.identity() {
                            LossItemIdentity::Worktree(identity) => Some(identity.clone()),
                            LossItemIdentity::GitPath(_)
                            | LossItemIdentity::GitOid(_)
                            | LossItemIdentity::Opaque(_) => unreachable!(),
                        },
                        bash_process_groups: std::collections::BTreeSet::default(),
                        tmux_servers: std::collections::BTreeSet::default(),
                        pty_sessions: std::collections::BTreeSet::default(),
                        browser_sessions: std::collections::BTreeSet::default(),
                        equivalent_live_resources: std::collections::BTreeSet::default(),
                    },
                }],
            })
            .await
            .unwrap();
        assert_eq!(replayed, resources);
        assert!(sqlx::query(
            "INSERT INTO close_expected_retirement_resources (
                 attempt_id, scope, inspection_generation, inspection_fingerprint,
                 resource_kind, identity_kind, identity_codec, identity_value
             ) VALUES (
                 'attempt-1', ?1, ?2, ?3, 'browser_session', 'opaque',
                 'opaque_string_v1', 'late-browser'
             )",
        )
        .bind(scope.as_str())
        .bind(snapshot.generation())
        .bind(snapshot.fingerprint())
        .execute(db.pool())
        .await
        .is_err());

        let wrong_worktree = RetiredResourceIdentity::parse(
            RetiredResourceKind::Worktree,
            LossItemIdentity::Worktree(WorktreeIdentity::from_parts(
                phoenix_core::domain::close::WorktreeId::parse("wrong-worktree").unwrap(),
                phoenix_core::domain::close::WorktreeFingerprint::parse("wrong-fingerprint")
                    .unwrap(),
                GitPathIdentity::from_bytes(b"/tmp/worktree".to_vec()),
            )),
        )
        .unwrap();
        let db2 = Database::open_in_memory().await.unwrap();
        create_root(&db2, "root-2").await;
        allocate_scope_worktree(&db2, "root-2").await;
        let scope2 = db2
            .get_conversation("root-2")
            .await
            .unwrap()
            .attached_work_scope_id
            .unwrap();
        db2.begin_close_foundation(&product_id("root-2"), &transcript_id("root-2"), "attempt-2")
            .await
            .unwrap();
        set_close_phase(&db2, "attempt-2", ClosePhase::RetirementRequested).await;
        assert!(db2
            .capture_close_retirement_inventory(CaptureCloseRetirementInventoryRequest {
                attempt_id: CloseAttemptId::parse("attempt-2").unwrap(),
                snapshot: current_test_snapshot(&db2, "attempt-2").await,
                scopes: vec![CaptureCloseRetirementInventoryScopeRequest {
                    scope: scope2,
                    inventory: CloseOwnedResourceInventory {
                        work_scopes: std::collections::BTreeSet::new(),
                        worktree: match wrong_worktree.identity() {
                            LossItemIdentity::Worktree(identity) => Some(identity.clone()),
                            LossItemIdentity::GitPath(_)
                            | LossItemIdentity::GitOid(_)
                            | LossItemIdentity::Opaque(_) => unreachable!(),
                        },
                        bash_process_groups: std::collections::BTreeSet::default(),
                        tmux_servers: std::collections::BTreeSet::default(),
                        pty_sessions: std::collections::BTreeSet::default(),
                        browser_sessions: std::collections::BTreeSet::default(),
                        equivalent_live_resources: std::collections::BTreeSet::default(),
                    },
                }],
            })
            .await
            .is_err());

        assert!(db
            .capture_close_retirement_inventory(CaptureCloseRetirementInventoryRequest {
                attempt_id: CloseAttemptId::parse("attempt-1").unwrap(),
                snapshot: current_test_snapshot(&db, "attempt-1").await,
                scopes: Vec::new(),
            })
            .await
            .is_err());
    }

    #[tokio::test]
    async fn retirement_inventory_replay_rejects_partial_unsealed_capture() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        allocate_scope_worktree(&db, "root").await;
        let scope = db
            .get_conversation("root")
            .await
            .unwrap()
            .attached_work_scope_id
            .unwrap();
        db.begin_close_foundation(
            &product_id("root"),
            &transcript_id("root"),
            "attempt-partial",
        )
        .await
        .unwrap();
        set_close_phase(&db, "attempt-partial", ClosePhase::RetirementRequested).await;
        let snapshot = current_test_snapshot(&db, "attempt-partial").await;
        sqlx::query(
            "INSERT INTO close_retirement_inventories (
                 attempt_id, scope, inspection_generation, inspection_fingerprint,
                 sealed, captured_at
             ) VALUES ('attempt-partial', ?1, ?2, ?3, 0, ?4)",
        )
        .bind(scope.as_str())
        .bind(snapshot.generation())
        .bind(snapshot.fingerprint())
        .bind(Utc::now().to_rfc3339())
        .execute(db.pool())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO close_expected_retirement_resources (
                 attempt_id, scope, inspection_generation, inspection_fingerprint,
                 resource_kind, identity_kind, identity_codec, identity_value
             ) VALUES (
                 'attempt-partial', ?1, ?2, ?3, 'browser_session', 'opaque',
                 'opaque_string_v1', 'partial-browser'
             )",
        )
        .bind(scope.as_str())
        .bind(snapshot.generation())
        .bind(snapshot.fingerprint())
        .execute(db.pool())
        .await
        .unwrap();

        let error = db
            .capture_close_retirement_inventory(CaptureCloseRetirementInventoryRequest {
                attempt_id: CloseAttemptId::parse("attempt-partial").unwrap(),
                snapshot,
                scopes: vec![CaptureCloseRetirementInventoryScopeRequest {
                    scope: scope.clone(),
                    inventory: CloseOwnedResourceInventory {
                        work_scopes: std::collections::BTreeSet::new(),
                        worktree: Some(current_test_worktree(&db, &scope).await),
                        bash_process_groups: std::collections::BTreeSet::default(),
                        tmux_servers: std::collections::BTreeSet::default(),
                        pty_sessions: std::collections::BTreeSet::default(),
                        browser_sessions: std::collections::BTreeSet::default(),
                        equivalent_live_resources: std::collections::BTreeSet::default(),
                    },
                }],
            })
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            DbError::CloseFoundationPrecondition(message)
                if message.contains("replay differs from sealed inventory")
        ));
    }

    #[tokio::test]
    async fn expected_resources_reject_incomplete_inventory_read() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        allocate_scope_worktree(&db, "root").await;
        let scope = db
            .get_conversation("root")
            .await
            .unwrap()
            .attached_work_scope_id
            .unwrap();
        db.begin_close_foundation(
            &product_id("root"),
            &transcript_id("root"),
            "attempt-incomplete",
        )
        .await
        .unwrap();
        set_close_phase(&db, "attempt-incomplete", ClosePhase::RetirementRequested).await;
        let snapshot = current_test_snapshot(&db, "attempt-incomplete").await;
        sqlx::query(
            "INSERT INTO close_retirement_inventories (
                 attempt_id, scope, inspection_generation, inspection_fingerprint,
                 sealed, captured_at
             ) VALUES ('attempt-incomplete', ?1, ?2, ?3, 0, ?4)",
        )
        .bind(scope.as_str())
        .bind(snapshot.generation())
        .bind(snapshot.fingerprint())
        .bind(Utc::now().to_rfc3339())
        .execute(db.pool())
        .await
        .unwrap();

        let error = db
            .list_close_expected_retirement_resources("attempt-incomplete")
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            DbError::CloseFoundationPrecondition(message)
                if message.contains("complete sealed inventory")
        ));
    }

    #[tokio::test]
    async fn inventory_capture_returns_typed_not_found() {
        let db = Database::open_in_memory().await.unwrap();
        let error = db
            .capture_close_retirement_inventory(CaptureCloseRetirementInventoryRequest {
                attempt_id: CloseAttemptId::parse("missing-attempt").unwrap(),
                snapshot: CloseRetirementSnapshot::parse("g1", "fp1").unwrap(),
                scopes: Vec::new(),
            })
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            DbError::CloseFoundationNotFound(attempt_id) if attempt_id == "missing-attempt"
        ));
    }

    #[tokio::test]
    async fn retirement_inventory_rejects_distinct_open_aggregate_on_scope() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        allocate_scope_worktree(&db, "root").await;
        let scope = db
            .get_conversation("root")
            .await
            .unwrap()
            .attached_work_scope_id
            .unwrap();
        create_root(&db, "other-root").await;
        let conflict =
            sqlx::query("UPDATE conversations SET work_scope_id = ?1 WHERE id = 'other-root'")
                .bind(scope.as_str())
                .execute(db.pool())
                .await
                .unwrap_err();
        assert!(conflict
            .to_string()
            .contains("different ordinary product conversation"));
        db.begin_close_foundation(
            &product_id("root"),
            &transcript_id("root"),
            "attempt-shared",
        )
        .await
        .unwrap();
        set_close_phase(&db, "attempt-shared", ClosePhase::RetirementRequested).await;

        let result = db
            .capture_close_retirement_inventory(CaptureCloseRetirementInventoryRequest {
                attempt_id: CloseAttemptId::parse("attempt-shared").unwrap(),
                snapshot: current_test_snapshot(&db, "attempt-shared").await,
                scopes: vec![CaptureCloseRetirementInventoryScopeRequest {
                    scope: scope.clone(),
                    inventory: CloseOwnedResourceInventory {
                        work_scopes: std::collections::BTreeSet::new(),
                        worktree: Some(current_test_worktree(&db, &scope).await),
                        bash_process_groups: std::collections::BTreeSet::default(),
                        tmux_servers: std::collections::BTreeSet::default(),
                        pty_sessions: std::collections::BTreeSet::default(),
                        browser_sessions: std::collections::BTreeSet::default(),
                        equivalent_live_resources: std::collections::BTreeSet::default(),
                    },
                }],
            })
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn cancelled_completion_round_trips_typed_outcome() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        db.begin_close_foundation(
            &product_id("root"),
            &transcript_id("root"),
            "attempt-cancelled",
        )
        .await
        .unwrap();
        sqlx::query(
            "UPDATE close_obligations
             SET phase = 'completed', completed_at = ?2, close_outcome = 'cancelled'
             WHERE attempt_id = ?1",
        )
        .bind("attempt-cancelled")
        .bind(Utc::now().to_rfc3339())
        .execute(db.pool())
        .await
        .unwrap();

        let obligation = db.get_close_obligation("attempt-cancelled").await.unwrap();
        assert_eq!(obligation.phase(), ClosePhase::Completed);
        assert_eq!(
            obligation.close_outcome(),
            Some(CloseCompletionOutcome::Cancelled)
        );
    }

    #[tokio::test]
    async fn close_runs_initial_admission_replay_and_normal_completion() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        let obligation = db
            .begin_close_foundation(&product_id("root"), &transcript_id("root"), "run-attempt")
            .await
            .unwrap();
        let run = CloseRunRef::initial(obligation.attempt_id().clone());
        assert_eq!(
            db.get_close_run(&run).await.unwrap().status,
            CloseRunStatus::Running
        );
        assert_eq!(
            db.list_running_close_runs().await.unwrap(),
            vec![run.clone()]
        );
        assert_eq!(
            db.begin_close_foundation(&product_id("root"), &transcript_id("root"), "run-attempt")
                .await
                .unwrap(),
            obligation
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM close_runs")
                .fetch_one(db.pool())
                .await
                .unwrap(),
            1
        );
        db.cancel_close_before_retirement(run.attempt_id.as_str())
            .await
            .unwrap();
        assert_eq!(
            db.get_close_run(&run).await.unwrap().status,
            CloseRunStatus::Completed
        );
        for sql in [
            "UPDATE close_runs SET status = 'running', ended_at_us = NULL",
            "DELETE FROM close_runs",
        ] {
            assert!(sqlx::query(sqlx::AssertSqlSafe(sql))
                .execute(db.pool())
                .await
                .is_err());
        }
    }

    fn safe_retry_request(
        failure: &TerminalizeInitialCloseCleanupFailureRequest,
    ) -> AdmitCloseSafeRetryRequest {
        AdmitCloseSafeRetryRequest {
            failed_run: CloseRunRef::initial(failure.attempt_id.clone()),
            requested_by: CloseRetryRequestedBy::User,
            observed_at_us: Utc::now().timestamp_micros(),
            precondition_resolution: "read-only check confirms the removal blocker is resolved"
                .into(),
            safety_evidence:
                "exact retained worktree identity remains reconstructible under original authority"
                    .into(),
            remaining_effects: failure
                .remaining_resources
                .iter()
                .map(|remaining| CloseSafeRetryEffect {
                    scope: remaining.scope.clone(),
                    resource: remaining.resource.clone(),
                })
                .collect(),
        }
    }

    #[tokio::test]
    async fn close_runs_failure_fences_admission_and_projects_both_outcomes() {
        for certainty in [
            CloseStopCertainty::ShutdownUncertain,
            CloseStopCertainty::ConversationAndProcessesStopped { confirmed_at_us: 1 },
        ] {
            let db = Database::open_in_memory().await.unwrap();
            let failure = initial_cleanup_failure_fixture(&db, certainty).await;
            let original = db
                .terminalize_initial_close_cleanup_failure(&failure)
                .await
                .unwrap();
            let run = CloseRunRef::initial(failure.attempt_id.clone());
            assert_eq!(
                db.get_close_run(&run).await.unwrap().status,
                CloseRunStatus::Stopped
            );
            assert!(db.list_running_close_runs().await.unwrap().is_empty());
            assert!(db
                .list_pending_close_obligations()
                .await
                .unwrap()
                .is_empty());
            let projection = db
                .get_active_close_projection_for_product(&product_id("root"))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(projection.obligation, original);
            assert_eq!(projection.latest_run, db.get_close_run(&run).await.unwrap());
            assert_eq!(projection.residuals.len(), 1);
            match db.product_conversation_admission("latest").await.unwrap() {
                ProductConversationAdmission::Refused(fence) => {
                    assert_eq!(certainty, CloseStopCertainty::ShutdownUncertain);
                    assert_eq!(fence.attempt_id, run.attempt_id);
                }
                ProductConversationAdmission::History(_) => {
                    assert!(certainty.confirmed_at_us().is_some());
                }
                other @ ProductConversationAdmission::Accepted { .. } => {
                    panic!("admitted stopped Close: {other:?}")
                }
            }
            assert!(db
                .begin_close_foundation(
                    &product_id("root"),
                    &transcript_id("latest"),
                    "unrelated-new-attempt"
                )
                .await
                .is_err());
            assert!(db.retry_close_retirement(&run.attempt_id).await.is_err());
            assert!(
                sqlx::query("UPDATE close_runs SET status = 'running', ended_at_us = NULL")
                    .execute(db.pool())
                    .await
                    .is_err()
            );
        }
    }

    #[tokio::test]
    async fn close_runs_safe_retry_allocates_run_two_without_resuming_stopped_authority() {
        for certainty in [
            CloseStopCertainty::ShutdownUncertain,
            CloseStopCertainty::ConversationAndProcessesStopped { confirmed_at_us: 1 },
        ] {
            let db = Database::open_in_memory().await.unwrap();
            let failure = initial_cleanup_failure_fixture(&db, certainty).await;
            let original = db
                .terminalize_initial_close_cleanup_failure(&failure)
                .await
                .unwrap();
            let request = safe_retry_request(&failure);
            let original_evidence = db
                .list_close_retirement_evidence(failure.attempt_id.as_str())
                .await
                .unwrap();
            let run = db.admit_close_safe_retry(&request).await.unwrap();
            assert_eq!(run.ordinal.get(), 2);
            assert_eq!(run.attempt_id, failure.attempt_id);
            assert_eq!(
                db.get_close_run(&request.failed_run).await.unwrap().status,
                CloseRunStatus::Stopped
            );
            assert_eq!(
                db.get_close_run(&run).await.unwrap().status,
                CloseRunStatus::Running
            );
            assert!(db.admit_close_safe_retry(&request).await.is_err());
            assert_eq!(
                db.get_close_obligation(failure.attempt_id.as_str())
                    .await
                    .unwrap(),
                original
            );
            assert_eq!(
                db.terminalize_initial_close_cleanup_failure(&failure)
                    .await
                    .unwrap(),
                original
            );
            assert_eq!(
                db.list_close_retirement_evidence(failure.attempt_id.as_str())
                    .await
                    .unwrap(),
                original_evidence
            );
            assert_eq!(
                db.classify_interrupted_close_run(&run).await.unwrap(),
                original
            );
            assert_eq!(
                db.classify_interrupted_close_run(&run).await.unwrap(),
                original
            );
            assert_eq!(
                db.get_close_run(&run).await.unwrap().status,
                CloseRunStatus::Stopped
            );
            assert!(db.admit_close_safe_retry(&request).await.is_err());
            assert_eq!(
                sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM close_cleanup_failures")
                    .fetch_one(db.pool())
                    .await
                    .unwrap(),
                2
            );
            assert_eq!(
                sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM coordinator_watch_events")
                    .fetch_one(db.pool())
                    .await
                    .unwrap(),
                2
            );
            assert_eq!(
                db.list_close_retirement_evidence(failure.attempt_id.as_str())
                    .await
                    .unwrap(),
                original_evidence
            );
            assert_eq!(
                db.get_conversation("latest").await.unwrap().archived,
                certainty.confirmed_at_us().is_some()
            );
            assert_eq!(
                db.get_active_close_projection_for_product(&product_id("root"))
                    .await
                    .unwrap()
                    .unwrap()
                    .latest_run
                    .run,
                run
            );
        }
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn close_runs_safe_retry_requires_complete_run_bound_success_to_archive() {
        for certainty in [
            CloseStopCertainty::ShutdownUncertain,
            CloseStopCertainty::ConversationAndProcessesStopped { confirmed_at_us: 1 },
        ] {
            let db = Database::open_in_memory().await.unwrap();
            let failure = initial_cleanup_failure_fixture(&db, certainty).await;
            let original = db
                .terminalize_initial_close_cleanup_failure(&failure)
                .await
                .unwrap();
            let request = safe_retry_request(&failure);
            let run = db.admit_close_safe_retry(&request).await.unwrap();
            assert_eq!(
                db.list_close_safe_retry_effects(&run).await.unwrap(),
                request.remaining_effects
            );
            assert_eq!(
                db.close_safe_retry_progress(&run).await.unwrap(),
                CloseSafeRetryProgress {
                    successful: vec![],
                    pending: request.remaining_effects.clone(),
                }
            );
            assert!(db
                .close_safe_retry_progress(&request.failed_run)
                .await
                .is_err());
            assert!(db
                .list_close_safe_retry_effects(&request.failed_run)
                .await
                .is_err());
            assert!(db.complete_close_safe_retry(&run).await.is_err());
            assert_eq!(
                db.get_close_obligation(run.attempt_id.as_str())
                    .await
                    .unwrap(),
                original
            );
            assert!(db
                .record_close_safe_retry_success(
                    &request.failed_run,
                    &request.remaining_effects[0],
                    "not this run"
                )
                .await
                .is_err());
            db.record_close_safe_retry_success(
                &run,
                &request.remaining_effects[0],
                "verified exact scope retired",
            )
            .await
            .unwrap();
            db.record_close_safe_retry_success(
                &run,
                &request.remaining_effects[0],
                "verified exact scope retired",
            )
            .await
            .unwrap();
            assert_eq!(
                db.close_safe_retry_progress(&run).await.unwrap(),
                CloseSafeRetryProgress {
                    successful: vec![request.remaining_effects[0].clone()],
                    pending: request.remaining_effects[1..].to_vec(),
                }
            );
            assert!(db
                .record_close_safe_retry_success(&run, &request.remaining_effects[0], "duplicate")
                .await
                .is_err());
            assert!(db.complete_close_safe_retry(&run).await.is_err());
            for effect in request.remaining_effects.iter().skip(1) {
                db.record_close_safe_retry_success(&run, effect, "verified exact resource retired")
                    .await
                    .unwrap();
            }
            let result = db.complete_close_safe_retry(&run).await.unwrap();
            assert_eq!(
                result.close_outcome(),
                Some(CloseCompletionOutcome::Archived)
            );
            assert_eq!(
                db.get_close_run(&run).await.unwrap().status,
                CloseRunStatus::Completed
            );
            assert_eq!(
                db.get_close_run(&request.failed_run).await.unwrap().status,
                CloseRunStatus::Stopped
            );
            assert!(db.get_conversation("latest").await.unwrap().archived);
            assert!(db.get_conversation("participant").await.unwrap().archived);
            assert!(db.complete_close_safe_retry(&run).await.is_err());
            assert!(db.admit_close_safe_retry(&request).await.is_err());
            assert_eq!(
                db.list_close_cleanup_failures(failure.attempt_id.as_str())
                    .await
                    .unwrap()
                    .len(),
                1
            );
            assert_eq!(
                sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM messages WHERE message_id = ?1")
                    .bind(format!("close-run-outcome:2:{}", run.attempt_id))
                    .fetch_one(db.pool())
                    .await
                    .unwrap(),
                1
            );
        }
    }

    #[tokio::test]
    async fn close_runs_safe_retry_rejects_stale_expanded_or_duplicate_plan_and_rolls_back() {
        let db = Database::open_in_memory().await.unwrap();
        let failure =
            initial_cleanup_failure_fixture(&db, CloseStopCertainty::ShutdownUncertain).await;
        db.terminalize_initial_close_cleanup_failure(&failure)
            .await
            .unwrap();
        let request = safe_retry_request(&failure);
        let mut stale = request.clone();
        stale.observed_at_us = failure.occurred_at_us;
        assert!(db.admit_close_safe_retry(&stale).await.is_err());
        let mut expanded = request.clone();
        expanded.remaining_effects[0].resource = RetiredResourceIdentity::parse(
            RetiredResourceKind::EquivalentLiveResource,
            LossItemIdentity::Opaque(OpaqueIdentity::parse("invented-resource").unwrap()),
        )
        .unwrap();
        assert!(db.admit_close_safe_retry(&expanded).await.is_err());
        let mut duplicate = request.clone();
        duplicate
            .remaining_effects
            .push(duplicate.remaining_effects[0].clone());
        assert!(db.admit_close_safe_retry(&duplicate).await.is_err());
        let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await.unwrap();
        Database::admit_close_safe_retry_tx(&mut tx, &request)
            .await
            .unwrap();
        tx.rollback().await.unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM close_runs")
                .fetch_one(db.pool())
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM close_run_retry_effects")
                .fetch_one(db.pool())
                .await
                .unwrap(),
            0
        );
        db.admit_close_safe_retry(&request).await.unwrap();
        assert!(
            sqlx::query("UPDATE close_run_retry_effects SET identity_value = 'changed'")
                .execute(db.pool())
                .await
                .is_err()
        );
        assert!(sqlx::query("DELETE FROM close_run_retry_effects")
            .execute(db.pool())
            .await
            .is_err());
    }

    #[tokio::test]
    async fn close_runs_retry_failure_replays_exactly_and_keeps_run_one_unchanged() {
        let db = Database::open_in_memory().await.unwrap();
        let failure =
            initial_cleanup_failure_fixture(&db, CloseStopCertainty::ShutdownUncertain).await;
        let original = db
            .terminalize_initial_close_cleanup_failure(&failure)
            .await
            .unwrap();
        let retry = safe_retry_request(&failure);
        let run_two = db.admit_close_safe_retry(&retry).await.unwrap();
        let mut changed = failure.clone();
        changed.detail = "late callback from stopped run one".into();
        assert!(db
            .terminalize_initial_close_cleanup_failure(&changed)
            .await
            .is_err());
        assert_eq!(
            db.get_close_run(&run_two).await.unwrap().status,
            CloseRunStatus::Running
        );
        changed.failure_occurrence_id = run_two.failure_occurrence_id();
        changed.detail = "retry run encountered a distinct failure".into();
        changed.stop_certainty = CloseStopCertainty::ConversationAndProcessesStopped {
            confirmed_at_us: Utc::now().timestamp_micros(),
        };
        changed.occurred_at_us = Utc::now().timestamp_micros();
        assert_eq!(
            db.terminalize_close_run_cleanup_failure(&run_two, &changed)
                .await
                .unwrap(),
            original
        );
        assert_eq!(
            db.terminalize_close_run_cleanup_failure(&run_two, &changed)
                .await
                .unwrap(),
            original
        );
        changed.detail = "conflicting replay".into();
        assert!(db
            .terminalize_close_run_cleanup_failure(&run_two, &changed)
            .await
            .is_err());
        let mut next = safe_retry_request(&failure);
        next.failed_run = run_two;
        let run_three = db.admit_close_safe_retry(&next).await.unwrap();
        assert_eq!(run_three.ordinal.get(), 3);
        assert!(db.admit_close_safe_retry(&retry).await.is_err());
        assert_eq!(
            db.terminalize_initial_close_cleanup_failure(&failure)
                .await
                .unwrap(),
            original
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM coordinator_watch_events")
                .fetch_one(db.pool())
                .await
                .unwrap(),
            2
        );
    }

    #[tokio::test]
    async fn retry_interruption_uses_pending_effect_not_successful_prior_resource() {
        let db = Database::open_in_memory().await.unwrap();
        let failure =
            initial_cleanup_failure_fixture(&db, CloseStopCertainty::ShutdownUncertain).await;
        db.terminalize_initial_close_cleanup_failure(&failure)
            .await
            .unwrap();
        let request = safe_retry_request(&failure);
        assert!(request.remaining_effects.len() >= 2);
        let run = db.admit_close_safe_retry(&request).await.unwrap();
        db.record_close_safe_retry_success(&run, &request.remaining_effects[0], "worktree removed")
            .await
            .unwrap();
        assert!(db
            .record_close_safe_retry_success(
                &run,
                &request.remaining_effects[0],
                "different detail"
            )
            .await
            .is_err());
        db.record_close_safe_retry_success(&run, &request.remaining_effects[0], "worktree removed")
            .await
            .unwrap();
        assert_eq!(
            db.close_safe_retry_progress(&run).await.unwrap().pending,
            request.remaining_effects[1..]
        );
        let original = db.classify_interrupted_close_run(&run).await.unwrap();
        assert_eq!(
            original.close_outcome(),
            Some(CloseCompletionOutcome::CloseIncomplete)
        );
        let failures = db
            .list_close_cleanup_failures(failure.attempt_id.as_str())
            .await
            .unwrap();
        assert_eq!(failures.len(), 2);
        assert_eq!(
            failures[1].occurrence.remaining_resources.len(),
            request.remaining_effects.len() - 1
        );
        assert_eq!(
            failures[1]
                .occurrence
                .remaining_resources
                .iter()
                .map(|remaining| &remaining.resource)
                .collect::<Vec<_>>(),
            request.remaining_effects[1..]
                .iter()
                .map(|effect| &effect.resource)
                .collect::<Vec<_>>()
        );
        assert!(db.close_safe_retry_progress(&run).await.is_err());
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn retry_interruption_after_all_successes_stops_until_explicit_completion_only_run() {
        for certainty in [
            CloseStopCertainty::ShutdownUncertain,
            CloseStopCertainty::ConversationAndProcessesStopped { confirmed_at_us: 1 },
        ] {
            let db = Database::open_in_memory().await.unwrap();
            let failure = initial_cleanup_failure_fixture(&db, certainty).await;
            let original = db
                .terminalize_initial_close_cleanup_failure(&failure)
                .await
                .unwrap();
            let request = safe_retry_request(&failure);
            let run = db.admit_close_safe_retry(&request).await.unwrap();
            assert!(!db
                .close_retry_verified_completion_eligible(&run)
                .await
                .unwrap());
            for effect in &request.remaining_effects {
                db.record_close_safe_retry_success(&run, effect, "retired")
                    .await
                    .unwrap();
            }
            assert!(db
                .close_safe_retry_progress(&run)
                .await
                .unwrap()
                .pending
                .is_empty());
            assert_eq!(
                db.classify_interrupted_close_run(&run).await.unwrap(),
                original
            );
            assert_eq!(
                db.classify_interrupted_close_run(&run).await.unwrap(),
                original
            );
            assert_eq!(
                db.get_close_run(&run).await.unwrap().status,
                CloseRunStatus::Stopped
            );
            assert_eq!(
                db.get_conversation("latest").await.unwrap().archived,
                certainty.confirmed_at_us().is_some()
            );
            let failures = db
                .list_close_cleanup_failures(failure.attempt_id.as_str())
                .await
                .unwrap();
            assert_eq!(failures.len(), 2);
            assert_eq!(
                failures[1].occurrence.authority,
                CloseCleanupFailureAuthority::AttemptInterrupted
            );
            assert!(failures[1].occurrence.remaining_resources.is_empty());
            assert_eq!(
                sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM coordinator_watch_events")
                    .fetch_one(db.pool())
                    .await
                    .unwrap(),
                2
            );
            assert_eq!(
                sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*) FROM close_run_retry_successes WHERE run_ordinal = 2"
                )
                .fetch_one(db.pool())
                .await
                .unwrap(),
                i64::try_from(request.remaining_effects.len()).unwrap()
            );
            assert!(db
                .close_retry_verified_completion_eligible(&run)
                .await
                .unwrap());
            assert!(!db
                .close_retry_verified_completion_eligible(&request.failed_run)
                .await
                .unwrap());
            assert!(db.complete_close_safe_retry(&run).await.is_err());
            let mut explicit = safe_retry_request(&failure);
            explicit.failed_run = run.clone();
            explicit.remaining_effects.clear();
            explicit.requested_by = CloseRetryRequestedBy::Global;
            explicit.precondition_resolution =
                "fresh read-only check confirms finalization is safe".into();
            explicit.safety_evidence = "exact completed retry effect proofs reverified".into();
            assert!(db
                .admit_close_safe_retry(&safe_retry_request(&failure))
                .await
                .is_err());
            let mut stale = explicit.clone();
            stale.observed_at_us = failures[1].occurrence.occurred_at_us;
            assert!(db.admit_close_safe_retry(&stale).await.is_err());
            let completion = db.admit_close_safe_retry(&explicit).await.unwrap();
            assert_eq!(completion.ordinal.get(), 3);
            assert!(db
                .list_close_safe_retry_effects(&completion)
                .await
                .unwrap()
                .is_empty());
            assert_eq!(sqlx::query_scalar::<_, String>("SELECT retry_evidence_kind FROM close_runs WHERE attempt_id = ?1 AND run_ordinal = 3")
            .bind(failure.attempt_id.as_str()).fetch_one(db.pool()).await.unwrap(), "verified_completion");
            assert!(db.admit_close_safe_retry(&explicit).await.is_err());
            assert!(!db
                .close_retry_verified_completion_eligible(&run)
                .await
                .unwrap());
            assert_eq!(
                db.complete_close_safe_retry(&completion)
                    .await
                    .unwrap()
                    .close_outcome(),
                Some(CloseCompletionOutcome::Archived)
            );
            assert_eq!(
                db.get_close_run(&completion).await.unwrap().status,
                CloseRunStatus::Completed
            );
            assert_eq!(
                db.get_close_run(&run).await.unwrap().status,
                CloseRunStatus::Stopped
            );
            assert!(db.complete_close_safe_retry(&completion).await.is_err());
        }
    }

    #[tokio::test]
    async fn completion_only_retry_rejects_initial_missing_capture_and_incomplete_retry_plan() {
        let db = Database::open_in_memory().await.unwrap();
        let initial = scope_free_close_fixture(&db).await;
        db.classify_interrupted_close_run(&initial).await.unwrap();
        assert!(!db
            .close_retry_verified_completion_eligible(&initial)
            .await
            .unwrap());
        let missing_capture = AdmitCloseSafeRetryRequest {
            failed_run: initial,
            requested_by: CloseRetryRequestedBy::User,
            observed_at_us: Utc::now().timestamp_micros(),
            precondition_resolution: "fresh read-only review".into(),
            safety_evidence: "no retirement plan exists".into(),
            remaining_effects: vec![],
        };
        assert!(db.admit_close_safe_retry(&missing_capture).await.is_err());

        let db = Database::open_in_memory().await.unwrap();
        let failure =
            initial_cleanup_failure_fixture(&db, CloseStopCertainty::ShutdownUncertain).await;
        db.terminalize_initial_close_cleanup_failure(&failure)
            .await
            .unwrap();
        let request = safe_retry_request(&failure);
        let run = db.admit_close_safe_retry(&request).await.unwrap();
        db.record_close_safe_retry_success(&run, &request.remaining_effects[0], "retired")
            .await
            .unwrap();
        let forged = TerminalizeInitialCloseCleanupFailureRequest {
            failure_occurrence_id: run.failure_occurrence_id(),
            attempt_id: run.attempt_id.clone(),
            source_product_conversation_id: failure.source_product_conversation_id.clone(),
            authority: CloseCleanupFailureAuthority::AttemptInterrupted,
            remaining_resources: vec![],
            reason: RetirementFailureReason::Interrupted,
            detail: "cannot erase pending effect".into(),
            stop_certainty: CloseStopCertainty::ShutdownUncertain,
            occurred_at_us: Utc::now().timestamp_micros(),
        };
        assert!(db
            .terminalize_close_run_cleanup_failure(&run, &forged)
            .await
            .is_err());
        assert_eq!(
            db.get_close_run(&run).await.unwrap().status,
            CloseRunStatus::Running
        );
        let stopped = db.classify_interrupted_close_run(&run).await.unwrap();
        assert_eq!(
            stopped.close_outcome(),
            Some(CloseCompletionOutcome::CloseIncomplete)
        );
        assert!(!db
            .close_retry_verified_completion_eligible(&run)
            .await
            .unwrap());
        let mut empty = safe_retry_request(&failure);
        empty.failed_run = run;
        empty.remaining_effects.clear();
        assert!(db.admit_close_safe_retry(&empty).await.is_err());
    }

    #[tokio::test]
    async fn close_runs_retry_rejects_omitted_or_process_epoch_effects() {
        let db = Database::open_in_memory().await.unwrap();
        let failure =
            observed_cleanup_failure_fixture(&db, RetiredResourceKind::BashProcessGroup).await;
        db.terminalize_initial_close_cleanup_failure(&failure)
            .await
            .unwrap();
        let request = safe_retry_request(&failure);
        assert!(failure.remaining_resources.len() > 1);
        let mut omitted = request.clone();
        omitted.remaining_effects.pop();
        assert!(db.admit_close_safe_retry(&omitted).await.is_err());
        assert!(db.admit_close_safe_retry(&request).await.is_err());
        assert_eq!(
            db.get_close_run(&request.failed_run).await.unwrap().status,
            CloseRunStatus::Stopped
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM close_runs")
                .fetch_one(db.pool())
                .await
                .unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn close_runs_safe_retry_rejects_completed_effects() {
        let db = Database::open_in_memory().await.unwrap();
        let process = RetiredResourceIdentity::parse(
            RetiredResourceKind::BashProcessGroup,
            LossItemIdentity::Opaque(OpaqueIdentity::parse("already-stopped-process").unwrap()),
        )
        .unwrap();
        let mut failure = initial_cleanup_failure_fixture_with_resources(
            &db,
            CloseStopCertainty::ShutdownUncertain,
            vec![process.clone()],
        )
        .await;
        let CloseCleanupFailureAuthority::ExpectedResource { snapshot, .. } = &failure.authority
        else {
            panic!("expected resource fixture")
        };
        db.record_close_retirement_evidence(RecordCloseRetirementEvidenceRequest {
            attempt_id: failure.attempt_id.clone(),
            scope: failure.authority.scope().unwrap().clone(),
            snapshot: snapshot.clone(),
            resource: process.clone(),
            outcome: RetirementOutcome::Retired,
            detail: Some("stopped before cleanup failure".into()),
        })
        .await
        .unwrap();
        failure.remaining_resources = db
            .expected_close_cleanup_failure_resources(
                failure.attempt_id.as_str(),
                failure.authority.scope().unwrap(),
                failure.authority.resource().unwrap(),
            )
            .await
            .unwrap();
        db.terminalize_initial_close_cleanup_failure(&failure)
            .await
            .unwrap();
        let mut request = safe_retry_request(&failure);
        request.remaining_effects.push(CloseSafeRetryEffect {
            scope: failure.authority.scope().unwrap().clone(),
            resource: process,
        });
        assert!(db.admit_close_safe_retry(&request).await.is_err());
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM close_run_retry_effects")
                .fetch_one(db.pool())
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            db.get_close_run(&request.failed_run).await.unwrap().status,
            CloseRunStatus::Stopped
        );
    }

    #[tokio::test]
    async fn close_runs_interrupted_classification_is_atomic_idempotent_and_observation_only() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        allocate_scope_worktree(&db, "root").await;
        let obligation = db
            .begin_close_foundation(&product_id("root"), &transcript_id("root"), "interrupted")
            .await
            .unwrap();
        let run = CloseRunRef::initial(obligation.attempt_id().clone());
        sqlx::raw_sql("CREATE TRIGGER fail_interruption_event BEFORE INSERT ON coordinator_watch_events BEGIN SELECT RAISE(ABORT, 'injected event failure'); END;")
            .execute(db.pool()).await.unwrap();
        assert!(db.classify_interrupted_close_run(&run).await.is_err());
        assert_eq!(
            db.get_close_run(&run).await.unwrap().status,
            CloseRunStatus::Running
        );
        assert_eq!(
            db.get_close_obligation(run.attempt_id.as_str())
                .await
                .unwrap(),
            obligation
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM close_cleanup_failures")
                .fetch_one(db.pool())
                .await
                .unwrap(),
            0
        );
        sqlx::raw_sql("DROP TRIGGER fail_interruption_event")
            .execute(db.pool())
            .await
            .unwrap();
        let stopped = db.classify_interrupted_close_run(&run).await.unwrap();
        assert_eq!(
            stopped.close_outcome(),
            Some(CloseCompletionOutcome::CloseIncomplete)
        );
        assert_eq!(
            db.classify_interrupted_close_run(&run).await.unwrap(),
            stopped
        );
        let counts: (i64, i64, i64, i64) = sqlx::query_as("SELECT (SELECT COUNT(*) FROM close_cleanup_failures), (SELECT COUNT(*) FROM coordinator_watch_events), (SELECT COUNT(*) FROM close_retirement_resources), (SELECT COUNT(*) FROM close_retirement_inspections)")
            .fetch_one(db.pool()).await.unwrap();
        assert_eq!(counts, (1, 1, 0, 0));
        assert!(!db.get_conversation("root").await.unwrap().archived);
        assert!(matches!(
            db.product_conversation_admission("root").await.unwrap(),
            ProductConversationAdmission::Refused(_)
        ));
        let event_id: String = sqlx::query_scalar("SELECT event_id FROM coordinator_watch_events")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(event_id, run.failure_occurrence_id());
    }

    async fn scope_free_close_fixture(db: &Database) -> CloseRunRef {
        create_root(db, "root").await;
        let now = Utc::now();
        let now_text = now.to_rfc3339();
        sqlx::query(
            "INSERT INTO close_obligations (
                attempt_id, product_conversation_id, phase, topology_sealed,
                created_at, updated_at)
             VALUES ('scope-free', 'root', 'awaiting_blocker_resolution', 0, ?1, ?1)",
        )
        .bind(&now_text)
        .execute(db.pool())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO close_attempt_participants (
                attempt_id, product_conversation_id, conversation_id, captured_at_unix_micros)
             VALUES ('scope-free', 'root', 'root', ?1)",
        )
        .bind(now.timestamp_micros())
        .execute(db.pool())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO close_attempt_members (
                attempt_id, conversation_id, member_role, continuation_ordinal,
                captured_continued_in_conv_id, captured_state_kind, captured_runtime_role,
                captured_work_scope_id, captured_at)
             SELECT 'scope-free', id, 'root_latest', 0, continued_in_conv_id,
                    state_kind, runtime_role, work_scope_id, ?1
             FROM conversations WHERE id = 'root'",
        )
        .bind(&now_text)
        .execute(db.pool())
        .await
        .unwrap();
        assert!(db
            .list_close_attempt_scopes("scope-free")
            .await
            .unwrap()
            .is_empty());
        CloseRunRef::initial(CloseAttemptId::parse("scope-free").unwrap())
    }

    #[tokio::test]
    async fn close_scope_free_interruption_is_atomic_exact_and_has_no_resource_children() {
        let db = Database::open_in_memory().await.unwrap();
        let run = scope_free_close_fixture(&db).await;
        sqlx::raw_sql("CREATE TRIGGER reject_scope_free_event BEFORE INSERT ON coordinator_watch_events BEGIN SELECT RAISE(ABORT, 'event failed'); END;")
            .execute(db.pool()).await.unwrap();
        assert!(db.classify_interrupted_close_run(&run).await.is_err());
        assert_eq!(
            db.get_close_run(&run).await.unwrap().status,
            CloseRunStatus::Running
        );
        assert!(db
            .list_close_cleanup_failures("scope-free")
            .await
            .unwrap()
            .is_empty());
        sqlx::raw_sql("DROP TRIGGER reject_scope_free_event")
            .execute(db.pool())
            .await
            .unwrap();
        let stopped = db.classify_interrupted_close_run(&run).await.unwrap();
        assert_eq!(
            stopped.close_outcome(),
            Some(CloseCompletionOutcome::CloseIncomplete)
        );
        assert_eq!(
            db.classify_interrupted_close_run(&run).await.unwrap(),
            stopped
        );
        let failures = db.list_close_cleanup_failures("scope-free").await.unwrap();
        assert_eq!(failures.len(), 1);
        assert_eq!(
            failures[0].occurrence.authority,
            CloseCleanupFailureAuthority::AttemptInterrupted
        );
        assert!(failures[0].occurrence.remaining_resources.is_empty());
        assert_eq!(
            db.terminalize_initial_close_cleanup_failure(&failures[0].occurrence)
                .await
                .unwrap(),
            stopped
        );
        let mut conflicting = failures[0].occurrence.clone();
        conflicting.detail.push_str(" changed");
        assert!(db
            .terminalize_initial_close_cleanup_failure(&conflicting)
            .await
            .is_err());
        let counts: (i64, i64, i64) = sqlx::query_as("SELECT
            (SELECT COUNT(*) FROM coordinator_watch_events WHERE event_id = 'close-failure:1:scope-free'),
            (SELECT COUNT(*) FROM close_cleanup_failure_resources),
            (SELECT COUNT(*) FROM close_retirement_resources)")
            .fetch_one(db.pool()).await.unwrap();
        assert_eq!(counts, (1, 0, 0));
        let shape: bool = sqlx::query_scalar(
            "SELECT scope IS NULL AND resource_kind IS NULL
            AND identity_kind IS NULL AND identity_codec IS NULL AND identity_value IS NULL
            FROM close_cleanup_failures",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert!(shape);
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn close_scope_free_failure_check_rejects_partial_resource_shapes() {
        let db = Database::open_in_memory().await.unwrap();
        scope_free_close_fixture(&db).await;
        let mut tx = db.pool().begin().await.unwrap();
        sqlx::raw_sql("DROP TRIGGER close_cleanup_failures_require_initial_authority;")
            .execute(&mut *tx)
            .await
            .unwrap();
        for (kind, scope, resource, identity_kind, codec, value, reason) in [
            (
                "attempt_interrupted",
                None,
                Some("browser_session"),
                None,
                None,
                None,
                "interrupted",
            ),
            (
                "attempt_interrupted",
                None,
                None,
                Some("opaque"),
                None,
                None,
                "interrupted",
            ),
            (
                "attempt_interrupted",
                None,
                None,
                None,
                Some("opaque_string_v1"),
                None,
                "interrupted",
            ),
            (
                "attempt_interrupted",
                None,
                None,
                None,
                None,
                Some("epoch:id"),
                "interrupted",
            ),
            (
                "attempt_interrupted",
                Some("invented"),
                None,
                None,
                None,
                None,
                "interrupted",
            ),
            (
                "attempt_interrupted",
                None,
                None,
                None,
                None,
                None,
                "removal_failed",
            ),
            (
                "observed_process_resource",
                None,
                Some("browser_session"),
                Some("opaque"),
                Some("opaque_string_v1"),
                Some("epoch:id"),
                "interrupted",
            ),
            (
                "captured_scope",
                None,
                None,
                None,
                None,
                None,
                "interrupted",
            ),
            (
                "expected_resource",
                None,
                None,
                None,
                None,
                None,
                "interrupted",
            ),
        ] {
            assert!(sqlx::query("INSERT INTO close_cleanup_failures
                (failure_occurrence_id, attempt_id, cleanup_run_ordinal, source_product_conversation_id,
                 authority_kind, scope, resource_kind, identity_kind, identity_codec, identity_value, reason,
                 detail, stop_certainty, occurred_at_us)
                VALUES ('close-failure:1:scope-free', 'scope-free', 1, 'root', ?1, ?2, ?3, ?4, ?5, ?6, ?7, '', 'shutdown_uncertain', 1)")
                .bind(kind).bind(scope).bind(resource).bind(identity_kind).bind(codec).bind(value).bind(reason)
                .execute(&mut *tx).await.is_err(), "accepted invalid {kind} shape");
        }
        tx.rollback().await.unwrap();
        db.classify_interrupted_close_run(&CloseRunRef::initial(
            CloseAttemptId::parse("scope-free").unwrap(),
        ))
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn close_process_success_exact_replay_conflict_stopped_fence_and_failure_subtraction() {
        for (kind, resource_kind) in [
            (
                CloseProcessResourceKind::BashProcessGroup,
                RetiredResourceKind::BashProcessGroup,
            ),
            (
                CloseProcessResourceKind::PtySession,
                RetiredResourceKind::PtySession,
            ),
            (
                CloseProcessResourceKind::BrowserSession,
                RetiredResourceKind::BrowserSession,
            ),
        ] {
            let db = Database::open_in_memory().await.unwrap();
            let mut failure = observed_cleanup_failure_fixture(&db, resource_kind).await;
            let run = CloseRunRef::initial(failure.attempt_id.clone());
            let success = CloseProcessStepSuccess {
                run: run.clone(),
                scope: failure.authority.scope().unwrap().clone(),
                resource_kind: kind,
                identity: OpaqueIdentity::parse("epoch-A:target-18").unwrap(),
                outcome: CloseProcessStepOutcome::Retired,
                observed_at_us: 1,
            };
            db.record_close_process_step_success(&success)
                .await
                .unwrap();
            db.record_close_process_step_success(&success)
                .await
                .unwrap();
            assert_eq!(
                db.list_close_process_step_successes(&run).await.unwrap(),
                vec![success.clone()]
            );
            let mut conflict = success.clone();
            conflict.outcome = CloseProcessStepOutcome::AbsenceVerified;
            assert!(db
                .record_close_process_step_success(&conflict)
                .await
                .is_err());
            conflict = success.clone();
            conflict.observed_at_us += 1;
            assert!(db
                .record_close_process_step_success(&conflict)
                .await
                .is_err());
            assert!(db
                .terminalize_initial_close_cleanup_failure(&failure)
                .await
                .is_err());
            failure
                .remaining_resources
                .retain(|target| target.resource.identity().value() != "epoch-A:target-18");
            db.terminalize_initial_close_cleanup_failure(&failure)
                .await
                .unwrap();
            db.record_close_process_step_success(&success)
                .await
                .unwrap();
            conflict = success.clone();
            conflict.identity = OpaqueIdentity::parse("epoch-A:new-target").unwrap();
            assert!(db
                .record_close_process_step_success(&conflict)
                .await
                .is_err());
            assert!(sqlx::query(
                "UPDATE close_process_step_successes SET outcome = 'absence_verified'"
            )
            .execute(db.pool())
            .await
            .is_err());
            assert!(sqlx::query("DELETE FROM close_process_step_successes")
                .execute(db.pool())
                .await
                .is_err());
            assert_eq!(
                db.list_close_cleanup_failures(failure.attempt_id.as_str())
                    .await
                    .unwrap()[0]
                    .occurrence
                    .remaining_resources
                    .len(),
                1
            );
        }
    }

    #[tokio::test]
    async fn close_process_success_excludes_sealed_inventory_and_failed_authority() {
        let db = Database::open_in_memory().await.unwrap();
        let process =
            cleanup_process_resource(RetiredResourceKind::BrowserSession, "epoch:retired-browser");
        let mut failure = initial_cleanup_failure_fixture_with_resources(
            &db,
            CloseStopCertainty::ShutdownUncertain,
            vec![process.clone()],
        )
        .await;
        let run = CloseRunRef::initial(failure.attempt_id.clone());
        let scope = failure.authority.scope().unwrap().clone();
        let success = CloseProcessStepSuccess {
            run,
            scope: scope.clone(),
            resource_kind: CloseProcessResourceKind::BrowserSession,
            identity: OpaqueIdentity::parse("epoch:retired-browser").unwrap(),
            outcome: CloseProcessStepOutcome::AbsenceVerified,
            observed_at_us: 1,
        };
        db.record_close_process_step_success(&success)
            .await
            .unwrap();
        assert!(!db
            .unresolved_expected_close_cleanup_resources(failure.attempt_id.as_str())
            .await
            .unwrap()
            .iter()
            .any(|target| target.resource == process));
        assert!(db
            .expected_close_cleanup_failure_resources(failure.attempt_id.as_str(), &scope, &process)
            .await
            .is_err());
        let mut wrong = failure.clone();
        wrong.authority = CloseCleanupFailureAuthority::ObservedProcessResource {
            scope: scope.clone(),
            resource: process.clone(),
        };
        wrong.remaining_resources = vec![CloseCleanupFailureResource {
            scope: scope.clone(),
            resource: process,
            disposition: CloseCleanupResourceDisposition::Failed,
        }];
        assert!(db
            .terminalize_initial_close_cleanup_failure(&wrong)
            .await
            .is_err());
        failure.remaining_resources = db
            .expected_close_cleanup_failure_resources(
                failure.attempt_id.as_str(),
                &scope,
                failure.authority.resource().unwrap(),
            )
            .await
            .unwrap();
        db.terminalize_initial_close_cleanup_failure(&failure)
            .await
            .unwrap();
        assert_eq!(
            db.list_close_process_step_successes(&success.run)
                .await
                .unwrap(),
            vec![success]
        );
    }

    async fn initial_cleanup_failure_fixture(
        db: &Database,
        stop_certainty: CloseStopCertainty,
    ) -> TerminalizeInitialCloseCleanupFailureRequest {
        initial_cleanup_failure_fixture_with_resources(db, stop_certainty, Vec::new()).await
    }

    async fn initial_cleanup_failure_fixture_with_resources(
        db: &Database,
        stop_certainty: CloseStopCertainty,
        resources: Vec<RetiredResourceIdentity>,
    ) -> TerminalizeInitialCloseCleanupFailureRequest {
        create_root(db, "root").await;
        let scope = allocate_scope_worktree(db, "root").await;
        create_child(db, "latest", "root").await;
        db.create_subagent_conversation(
            "participant",
            "participant",
            "/tmp",
            "latest",
            "test-model",
            &crate::ConvMode::Direct,
            phoenix_core::llm_language::LlmLanguage::default(),
            db.get_conversation("latest")
                .await
                .unwrap()
                .attached_work_scope_id
                .as_ref(),
            crate::SubAgentExecution {
                connection: "mock",
                effort: None,
                persona: None,
            },
        )
        .await
        .unwrap();
        db.begin_close_foundation(
            &product_id("root"),
            &transcript_id("latest"),
            "attempt-failure",
        )
        .await
        .unwrap();
        set_close_phase(
            db,
            "attempt-failure",
            ClosePhase::AwaitingRetirementInspection,
        )
        .await;
        db.replace_close_inspection(ReplaceCloseInspectionRequest {
            attempt_id: CloseAttemptId::parse("attempt-failure").unwrap(),
            scopes: vec![ReplaceCloseInspectionScopeRequest {
                scope: scope.clone(),
                snapshot: CloseRetirementSnapshot::parse("failure-gen", "failure-fp").unwrap(),
                losses: Vec::new(),
            }],
        })
        .await
        .unwrap();
        let snapshot = current_test_snapshot(db, "attempt-failure").await;
        capture_test_inventory(db, "attempt-failure", &scope, &snapshot, resources).await;
        let resource = db
            .list_close_expected_retirement_resources("attempt-failure")
            .await
            .unwrap()
            .into_iter()
            .find(|row| row.resource.kind() == RetiredResourceKind::Worktree)
            .unwrap()
            .resource;
        TerminalizeInitialCloseCleanupFailureRequest {
            failure_occurrence_id: "close-failure:1:attempt-failure".into(),
            attempt_id: CloseAttemptId::parse("attempt-failure").unwrap(),
            source_product_conversation_id: product_id("root"),
            remaining_resources: db
                .expected_close_cleanup_failure_resources("attempt-failure", &scope, &resource)
                .await
                .unwrap(),
            authority: CloseCleanupFailureAuthority::ExpectedResource {
                scope,
                snapshot,
                resource,
            },
            reason: RetirementFailureReason::RemovalFailed,
            detail: "exact cleanup detail".into(),
            stop_certainty,
            occurred_at_us: Utc::now().timestamp_micros(),
        }
    }

    fn cleanup_process_resource(
        kind: RetiredResourceKind,
        identity: &str,
    ) -> RetiredResourceIdentity {
        RetiredResourceIdentity::parse(
            kind,
            LossItemIdentity::Opaque(OpaqueIdentity::parse(identity).unwrap()),
        )
        .unwrap()
    }

    async fn observed_cleanup_failure_fixture(
        db: &Database,
        kind: RetiredResourceKind,
    ) -> TerminalizeInitialCloseCleanupFailureRequest {
        create_root(db, "root").await;
        let scope = allocate_scope_worktree(db, "root").await;
        db.begin_close_foundation(
            &product_id("root"),
            &transcript_id("root"),
            "attempt-observed",
        )
        .await
        .unwrap();
        set_close_phase(db, "attempt-observed", ClosePhase::SettlingActiveWork).await;
        let resource = cleanup_process_resource(kind, "epoch-A:target-17");
        TerminalizeInitialCloseCleanupFailureRequest {
            failure_occurrence_id: "close-failure:1:attempt-observed".into(),
            attempt_id: CloseAttemptId::parse("attempt-observed").unwrap(),
            source_product_conversation_id: product_id("root"),
            remaining_resources: vec![
                CloseCleanupFailureResource {
                    scope: scope.clone(),
                    resource: resource.clone(),
                    disposition: CloseCleanupResourceDisposition::Failed,
                },
                CloseCleanupFailureResource {
                    scope: scope.clone(),
                    resource: cleanup_process_resource(kind, "epoch-A:target-18"),
                    disposition: CloseCleanupResourceDisposition::Unattempted,
                },
            ],
            authority: CloseCleanupFailureAuthority::ObservedProcessResource { scope, resource },
            reason: RetirementFailureReason::ResidualProcessAlive,
            detail: "process permit could not confirm shutdown".into(),
            stop_certainty: CloseStopCertainty::ShutdownUncertain,
            occurred_at_us: 1,
        }
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn initial_cleanup_failure_observed_process_exact_identity_replay_and_rollback() {
        for kind in [
            RetiredResourceKind::BashProcessGroup,
            RetiredResourceKind::PtySession,
            RetiredResourceKind::BrowserSession,
            RetiredResourceKind::EquivalentLiveResource,
        ] {
            let db = Database::open_in_memory().await.unwrap();
            let request = observed_cleanup_failure_fixture(&db, kind).await;
            let mut tx = db.pool().begin().await.unwrap();
            Database::terminalize_initial_close_cleanup_failure_tx(&mut tx, &request)
                .await
                .unwrap();
            assert_eq!(
                sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*) FROM close_cleanup_failure_resources"
                )
                .fetch_one(&mut *tx)
                .await
                .unwrap(),
                2
            );
            tx.rollback().await.unwrap();
            let counts: (i64, i64, i64, i64) = sqlx::query_as("SELECT (SELECT COUNT(*) FROM close_cleanup_failures), (SELECT COUNT(*) FROM close_cleanup_failure_resources), (SELECT COUNT(*) FROM coordinator_watch_events), (SELECT COUNT(*) FROM messages)").fetch_one(db.pool()).await.unwrap();
            assert_eq!(counts, (0, 0, 0, 0));
            assert_eq!(
                db.get_close_run(&CloseRunRef::initial(request.attempt_id.clone()))
                    .await
                    .unwrap()
                    .status,
                CloseRunStatus::Running
            );

            for invalid in 0..5 {
                let mut changed = request.clone();
                match invalid {
                    0 => changed.remaining_resources.clear(),
                    1 => changed
                        .remaining_resources
                        .push(changed.remaining_resources[0].clone()),
                    2 => {
                        changed.remaining_resources[1].scope =
                            WorkScopeId::parse("not-captured").unwrap();
                    }
                    3 => {
                        changed.remaining_resources[0].resource =
                            cleanup_process_resource(kind, "epoch-B:target-17");
                    }
                    4 => {
                        changed.stop_certainty =
                            CloseStopCertainty::ConversationAndProcessesStopped {
                                confirmed_at_us: 1,
                            }
                    }
                    _ => unreachable!(),
                }
                assert!(db
                    .terminalize_initial_close_cleanup_failure(&changed)
                    .await
                    .is_err());
            }
            db.terminalize_initial_close_cleanup_failure(&request)
                .await
                .unwrap();
            db.terminalize_initial_close_cleanup_failure(&request)
                .await
                .unwrap();
            let failures = db
                .list_close_cleanup_failures(request.attempt_id.as_str())
                .await
                .unwrap();
            assert_eq!(failures.len(), 1);
            assert_eq!(failures[0].occurrence, request);
            assert_eq!(failures[0].run_ordinal, CloseRunOrdinal::INITIAL);
            let identities: Vec<String> = sqlx::query_scalar(
                "SELECT identity_value FROM close_cleanup_failure_resources ORDER BY ordinal",
            )
            .fetch_all(db.pool())
            .await
            .unwrap();
            assert_eq!(identities, ["epoch-A:target-17", "epoch-A:target-18"]);
            let counts: (i64, i64, i64, i64) = sqlx::query_as("SELECT (SELECT COUNT(*) FROM close_expected_retirement_resources), (SELECT COUNT(*) FROM close_retirement_resources), (SELECT COUNT(*) FROM close_retirement_resource_history), (SELECT COUNT(*) FROM coordinator_watch_events)").fetch_one(db.pool()).await.unwrap();
            assert_eq!(counts, (0, 0, 0, 1));
            for invalid in 0..4 {
                let mut changed = request.clone();
                match invalid {
                    0 => {
                        changed.authority = CloseCleanupFailureAuthority::CapturedScope {
                            scope: request.authority.scope().unwrap().clone(),
                            resource: request.authority.resource().unwrap().clone(),
                        }
                    }
                    1 => {
                        changed.authority = CloseCleanupFailureAuthority::ExpectedResource {
                            scope: request.authority.scope().unwrap().clone(),
                            snapshot: CloseRetirementSnapshot::parse("fake", "fake").unwrap(),
                            resource: request.authority.resource().unwrap().clone(),
                        }
                    }
                    2 => {
                        changed.remaining_resources[1].disposition =
                            CloseCleanupResourceDisposition::Unknown;
                    }
                    3 => changed.remaining_resources.reverse(),
                    _ => unreachable!(),
                }
                assert!(db
                    .terminalize_initial_close_cleanup_failure(&changed)
                    .await
                    .is_err());
            }
            assert!(db
                .admit_close_safe_retry(&AdmitCloseSafeRetryRequest {
                    failed_run: CloseRunRef::initial(request.attempt_id.clone()),
                    requested_by: CloseRetryRequestedBy::Global,
                    observed_at_us: Utc::now().timestamp_micros(),
                    precondition_resolution: "observed stopped".into(),
                    safety_evidence: "diagnostic identity only".into(),
                    remaining_effects: vec![CloseSafeRetryEffect {
                        scope: request.authority.scope().unwrap().clone(),
                        resource: request.authority.resource().unwrap().clone()
                    }],
                })
                .await
                .is_err());
            for sql in [
                "UPDATE close_cleanup_failure_resources SET disposition = 'unknown'",
                "DELETE FROM close_cleanup_failure_resources",
                "INSERT INTO close_cleanup_failure_resources SELECT failure_occurrence_id, 2, scope, resource_kind, identity_kind, identity_codec, 'late-target', 'unknown' FROM close_cleanup_failure_resources WHERE ordinal = 0",
            ] {
                assert!(sqlx::query(sql).execute(db.pool()).await.is_err());
            }
            assert!(!db.get_conversation("root").await.unwrap().archived);
        }
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn initial_cleanup_failure_observed_process_schema_rejects_wrong_authority() {
        let db = Database::open_in_memory().await.unwrap();
        let request =
            observed_cleanup_failure_fixture(&db, RetiredResourceKind::BashProcessGroup).await;
        create_root(&db, "other-root").await;
        let outside_scope = db
            .get_conversation("other-root")
            .await
            .unwrap()
            .attached_work_scope_id
            .unwrap();
        for (authority, generation, fingerprint, kind, scope, certainty, confirmed) in [
            (
                "captured_scope",
                None,
                None,
                "bash_process_group",
                request.authority.scope().unwrap().as_str(),
                "shutdown_uncertain",
                None,
            ),
            (
                "expected_resource",
                None,
                None,
                "bash_process_group",
                request.authority.scope().unwrap().as_str(),
                "shutdown_uncertain",
                None,
            ),
            (
                "expected_resource",
                Some("fake"),
                Some("fake"),
                "bash_process_group",
                request.authority.scope().unwrap().as_str(),
                "shutdown_uncertain",
                None,
            ),
            (
                "observed_process_resource",
                Some("fake"),
                Some("fake"),
                "bash_process_group",
                request.authority.scope().unwrap().as_str(),
                "shutdown_uncertain",
                None,
            ),
            (
                "observed_process_resource",
                None,
                None,
                "tmux_server",
                request.authority.scope().unwrap().as_str(),
                "shutdown_uncertain",
                None,
            ),
            (
                "observed_process_resource",
                None,
                None,
                "work_scope",
                request.authority.scope().unwrap().as_str(),
                "shutdown_uncertain",
                None,
            ),
            (
                "observed_process_resource",
                None,
                None,
                "bash_process_group",
                outside_scope.as_str(),
                "shutdown_uncertain",
                None,
            ),
            (
                "observed_process_resource",
                None,
                None,
                "bash_process_group",
                request.authority.scope().unwrap().as_str(),
                "conversation_and_processes_stopped",
                Some(1),
            ),
        ] {
            let result = sqlx::query("INSERT INTO close_cleanup_failures (failure_occurrence_id, attempt_id, cleanup_run_ordinal,
                source_product_conversation_id, scope, inspection_generation, inspection_fingerprint, resource_kind,
                identity_kind, identity_codec, identity_value, reason, detail, stop_certainty, confirmed_at_us, occurred_at_us, authority_kind)
                VALUES ('close-failure:1:attempt-observed', 'attempt-observed', 1, 'root', ?1, ?2, ?3, ?4,
                    'opaque', 'opaque_string_v1', 'epoch-A:target-17', 'residual_process_alive', 'test detail', ?5, ?6, 1, ?7)")
                .bind(scope).bind(generation).bind(fingerprint).bind(kind).bind(certainty).bind(confirmed).bind(authority).execute(db.pool()).await;
            assert!(result.is_err(), "invalid {authority}/{kind} must fail");
        }
        db.terminalize_initial_close_cleanup_failure(&request)
            .await
            .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT authority_kind FROM close_cleanup_failures")
                .fetch_one(db.pool())
                .await
                .unwrap(),
            "observed_process_resource"
        );
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn initial_cleanup_failure_remaining_inventory_spans_scopes_and_excludes_success() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        let root_scope = allocate_scope_worktree(&db, "root").await;
        create_child(&db, "latest", "root").await;
        sqlx::query("INSERT INTO work_scopes (id, authority_kind, lifecycle, environment_kind, cwd, created_at, updated_at)
            SELECT 'later-scope', 'work', 'active', 'unowned_cwd', '/tmp/later', created_at, updated_at FROM work_scopes WHERE id = ?1")
            .bind(root_scope.as_str()).execute(db.pool()).await.unwrap();
        sqlx::query("UPDATE conversations SET work_scope_id = 'later-scope' WHERE id = 'latest'")
            .execute(db.pool())
            .await
            .unwrap();
        db.begin_close_foundation(
            &product_id("root"),
            &transcript_id("latest"),
            "attempt-multi",
        )
        .await
        .unwrap();
        set_close_phase(
            &db,
            "attempt-multi",
            ClosePhase::AwaitingRetirementInspection,
        )
        .await;
        db.replace_close_inspection(ReplaceCloseInspectionRequest {
            attempt_id: CloseAttemptId::parse("attempt-multi").unwrap(),
            scopes: vec![ReplaceCloseInspectionScopeRequest {
                scope: root_scope.clone(),
                snapshot: CloseRetirementSnapshot::parse("multi-gen", "multi-fp").unwrap(),
                losses: Vec::new(),
            }],
        })
        .await
        .unwrap();
        let snapshot = current_test_snapshot(&db, "attempt-multi").await;
        let done = cleanup_process_resource(RetiredResourceKind::BrowserSession, "browser-done");
        let failed =
            cleanup_process_resource(RetiredResourceKind::BrowserSession, "browser-failed");
        let later = cleanup_process_resource(RetiredResourceKind::BrowserSession, "browser-later");
        capture_test_inventory(
            &db,
            "attempt-multi",
            &root_scope,
            &snapshot,
            vec![done.clone(), failed.clone(), later.clone()],
        )
        .await;
        db.record_close_retirement_evidence(RecordCloseRetirementEvidenceRequest {
            attempt_id: CloseAttemptId::parse("attempt-multi").unwrap(),
            scope: root_scope.clone(),
            snapshot: snapshot.clone(),
            resource: done.clone(),
            outcome: RetirementOutcome::Retired,
            detail: None,
        })
        .await
        .unwrap();
        let remaining = db
            .expected_close_cleanup_failure_resources("attempt-multi", &root_scope, &failed)
            .await
            .unwrap();
        assert!(!remaining.iter().any(|target| target.resource == done));
        assert!(remaining.iter().any(|target| target.resource == failed
            && target.disposition == CloseCleanupResourceDisposition::Failed));
        assert!(remaining.iter().any(|target| target.resource == later
            && target.disposition == CloseCleanupResourceDisposition::Unattempted));
        assert!(remaining
            .iter()
            .any(|target| target.scope.as_str() == "later-scope"));
        let request = TerminalizeInitialCloseCleanupFailureRequest {
            failure_occurrence_id: "close-failure:1:attempt-multi".into(),
            attempt_id: CloseAttemptId::parse("attempt-multi").unwrap(),
            source_product_conversation_id: product_id("root"),
            authority: CloseCleanupFailureAuthority::ExpectedResource {
                scope: root_scope.clone(),
                snapshot: snapshot.clone(),
                resource: failed.clone(),
            },
            remaining_resources: remaining,
            reason: RetirementFailureReason::ResidualProcessAlive,
            detail: "failed at exact browser target".into(),
            stop_certainty: CloseStopCertainty::ShutdownUncertain,
            occurred_at_us: 1,
        };
        for invalid in 0..4 {
            let mut changed = request.clone();
            match invalid {
                0 => changed
                    .remaining_resources
                    .retain(|target| target.scope == root_scope),
                1 => changed
                    .remaining_resources
                    .push(CloseCleanupFailureResource {
                        scope: root_scope.clone(),
                        resource: done.clone(),
                        disposition: CloseCleanupResourceDisposition::Unattempted,
                    }),
                2 => {
                    changed
                        .remaining_resources
                        .iter_mut()
                        .find(|target| target.resource == later)
                        .unwrap()
                        .disposition = CloseCleanupResourceDisposition::Unknown;
                }
                3 => {
                    changed
                        .remaining_resources
                        .iter_mut()
                        .find(|target| target.resource == failed)
                        .unwrap()
                        .disposition = CloseCleanupResourceDisposition::Unattempted;
                }
                _ => unreachable!(),
            }
            assert!(db
                .terminalize_initial_close_cleanup_failure(&changed)
                .await
                .is_err());
        }
        db.terminalize_initial_close_cleanup_failure(&request)
            .await
            .unwrap();
        db.terminalize_initial_close_cleanup_failure(&request)
            .await
            .unwrap();
        assert_eq!(
            db.list_close_cleanup_failures("attempt-multi")
                .await
                .unwrap()[0]
                .occurrence,
            request
        );
        assert!(db
            .admit_close_safe_retry(&AdmitCloseSafeRetryRequest {
                failed_run: CloseRunRef::initial(request.attempt_id.clone()),
                requested_by: CloseRetryRequestedBy::User,
                observed_at_us: Utc::now().timestamp_micros(),
                precondition_resolution: "live permit verified".into(),
                safety_evidence: "original browser target only".into(),
                remaining_effects: vec![CloseSafeRetryEffect {
                    scope: root_scope,
                    resource: later,
                }],
            })
            .await
            .is_err());
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM coordinator_watch_events")
                .fetch_one(db.pool())
                .await
                .unwrap(),
            1
        );
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn initial_cleanup_failure_captured_scope_before_inventory() {
        for phase in [
            ClosePhase::SettlingActiveWork,
            ClosePhase::AwaitingRetirementInspection,
        ] {
            for use_worktree in [false, true] {
                let db = Database::open_in_memory().await.unwrap();
                create_root(&db, "root").await;
                let scope = allocate_scope_worktree(&db, "root").await;
                db.begin_close_foundation(
                    &product_id("root"),
                    &transcript_id("root"),
                    "attempt-captured",
                )
                .await
                .unwrap();
                set_close_phase(&db, "attempt-captured", phase).await;
                let captured = db
                    .list_close_attempt_scopes("attempt-captured")
                    .await
                    .unwrap();
                let Some(CapturedWorktreeIdentity::Resolved(worktree)) =
                    captured[0].captured_worktree.clone()
                else {
                    panic!("expected resolved captured worktree")
                };
                let resource = if use_worktree {
                    RetiredResourceIdentity::parse(
                        RetiredResourceKind::Worktree,
                        LossItemIdentity::Worktree(worktree),
                    )
                    .unwrap()
                } else {
                    RetiredResourceIdentity::parse(
                        RetiredResourceKind::WorkScope,
                        LossItemIdentity::Opaque(OpaqueIdentity::parse(scope.as_str()).unwrap()),
                    )
                    .unwrap()
                };
                let request = TerminalizeInitialCloseCleanupFailureRequest {
                    failure_occurrence_id: "close-failure:1:attempt-captured".into(),
                    attempt_id: CloseAttemptId::parse("attempt-captured").unwrap(),
                    source_product_conversation_id: product_id("root"),
                    remaining_resources: vec![
                        CloseCleanupFailureResource {
                            scope: scope.clone(),
                            resource: resource.clone(),
                            disposition: CloseCleanupResourceDisposition::Failed,
                        },
                        CloseCleanupFailureResource {
                            scope: scope.clone(),
                            resource: cleanup_process_resource(
                                RetiredResourceKind::PtySession,
                                "captured-epoch:unproven",
                            ),
                            disposition: CloseCleanupResourceDisposition::Unknown,
                        },
                    ],
                    authority: CloseCleanupFailureAuthority::CapturedScope { scope, resource },
                    reason: RetirementFailureReason::IdentityNotProven,
                    detail: "failed before inventory".into(),
                    stop_certainty: CloseStopCertainty::ShutdownUncertain,
                    occurred_at_us: 1,
                };
                let mut invalid = request.clone();
                invalid.stop_certainty =
                    CloseStopCertainty::ConversationAndProcessesStopped { confirmed_at_us: 1 };
                assert!(db
                    .terminalize_initial_close_cleanup_failure(&invalid)
                    .await
                    .is_err());
                for (kind, value) in [
                    (RetiredResourceKind::WorkScope, "wrong-scope"),
                    (
                        RetiredResourceKind::BrowserSession,
                        request.authority.scope().unwrap().as_str(),
                    ),
                ] {
                    invalid = request.clone();
                    invalid.authority = CloseCleanupFailureAuthority::CapturedScope {
                        scope: request.authority.scope().unwrap().clone(),
                        resource: RetiredResourceIdentity::parse(
                            kind,
                            LossItemIdentity::Opaque(OpaqueIdentity::parse(value).unwrap()),
                        )
                        .unwrap(),
                    };
                    assert!(db
                        .terminalize_initial_close_cleanup_failure(&invalid)
                        .await
                        .is_err());
                }
                for (generation, fingerprint, kind, codec, value, certainty, confirmed) in [
                    (
                        None,
                        Some("partial"),
                        "work_scope",
                        "opaque_string_v1",
                        request.authority.scope().unwrap().as_str(),
                        "shutdown_uncertain",
                        None,
                    ),
                    (
                        Some("partial"),
                        None,
                        "work_scope",
                        "opaque_string_v1",
                        request.authority.scope().unwrap().as_str(),
                        "shutdown_uncertain",
                        None,
                    ),
                    (
                        Some("invented"),
                        Some("invented"),
                        "work_scope",
                        "opaque_string_v1",
                        request.authority.scope().unwrap().as_str(),
                        "shutdown_uncertain",
                        None,
                    ),
                    (
                        None,
                        None,
                        "work_scope",
                        "wrong_codec",
                        request.authority.scope().unwrap().as_str(),
                        "shutdown_uncertain",
                        None,
                    ),
                    (
                        None,
                        None,
                        "work_scope",
                        "opaque_string_v1",
                        "wrong-scope",
                        "shutdown_uncertain",
                        None,
                    ),
                    (
                        None,
                        None,
                        "browser_session",
                        "opaque_string_v1",
                        request.authority.scope().unwrap().as_str(),
                        "shutdown_uncertain",
                        None,
                    ),
                    (
                        None,
                        None,
                        "work_scope",
                        "opaque_string_v1",
                        request.authority.scope().unwrap().as_str(),
                        "conversation_and_processes_stopped",
                        Some(1),
                    ),
                ] {
                    assert!(sqlx::query(
                        "INSERT INTO close_cleanup_failures (failure_occurrence_id, attempt_id, cleanup_run_ordinal,
                         source_product_conversation_id, scope, inspection_generation, inspection_fingerprint,
                         resource_kind, identity_kind, identity_codec, identity_value, reason, detail,
                         stop_certainty, confirmed_at_us, occurred_at_us, authority_kind)
                         VALUES ('close-failure:1:attempt-captured', 'attempt-captured', 1, 'root', ?1, ?2, ?3, ?4, 'opaque', ?5, ?6,
                         'identity_not_proven', 'invalid authority', ?7, ?8, 1, 'captured_scope')",
                    ).bind(request.authority.scope().unwrap().as_str()).bind(generation).bind(fingerprint).bind(kind).bind(codec)
                        .bind(value).bind(certainty).bind(confirmed).execute(db.pool()).await.is_err());
                }
                let completed = db
                    .terminalize_initial_close_cleanup_failure(&request)
                    .await
                    .unwrap();
                assert_eq!(completed.phase(), ClosePhase::Completed);
                assert_eq!(completed.snapshot(), None);
                assert_eq!(
                    completed.close_outcome(),
                    Some(CloseCompletionOutcome::CloseIncomplete)
                );
                assert_eq!(
                    db.terminalize_initial_close_cleanup_failure(&request)
                        .await
                        .unwrap(),
                    completed
                );
                let row: (Option<String>, Option<String>) = sqlx::query_as(
                    "SELECT inspection_generation, inspection_fingerprint FROM close_cleanup_failures",
                ).fetch_one(db.pool()).await.unwrap();
                assert_eq!(row, (None, None));
                let counts: (i64, i64, i64, i64, i64, i64) = sqlx::query_as(
                    "SELECT (SELECT COUNT(*) FROM close_expected_retirement_resources),
                     (SELECT COUNT(*) FROM close_retirement_resources),
                     (SELECT COUNT(*) FROM close_retirement_resource_history),
                     (SELECT COUNT(*) FROM close_cleanup_failures),
                     (SELECT COUNT(*) FROM coordinator_watch_events), (SELECT COUNT(*) FROM messages)",
                ).fetch_one(db.pool()).await.unwrap();
                assert_eq!(counts, (0, 0, 0, 1, 1, 1));
                assert_eq!(
                    db.list_close_cleanup_failures("attempt-captured")
                        .await
                        .unwrap()[0]
                        .occurrence,
                    request
                );
                assert!(!db.get_conversation("root").await.unwrap().archived);
                assert_eq!(
                    sqlx::query_scalar::<_, String>(
                        "SELECT ordinary_lifecycle FROM product_conversations WHERE id='root'"
                    )
                    .fetch_one(db.pool())
                    .await
                    .unwrap(),
                    "open"
                );
                let mut conflicting = request.clone();
                let CloseCleanupFailureAuthority::CapturedScope { resource, .. } =
                    &request.authority
                else {
                    unreachable!()
                };
                conflicting.authority = CloseCleanupFailureAuthority::ExpectedResource {
                    scope: request.authority.scope().unwrap().clone(),
                    snapshot: CloseRetirementSnapshot::parse("invented", "invented").unwrap(),
                    resource: resource.clone(),
                };
                assert!(db
                    .terminalize_initial_close_cleanup_failure(&conflicting)
                    .await
                    .is_err());
                assert!(sqlx::query("PRAGMA foreign_key_check")
                    .fetch_all(db.pool())
                    .await
                    .unwrap()
                    .is_empty());
            }
        }
    }

    #[tokio::test]
    async fn initial_cleanup_failure_captured_scope_preserves_existing_inspection_on_replay() {
        let db = Database::open_in_memory().await.unwrap();
        let mut request =
            initial_cleanup_failure_fixture(&db, CloseStopCertainty::ShutdownUncertain).await;
        let CloseCleanupFailureAuthority::ExpectedResource {
            scope,
            snapshot,
            resource,
        } = request.authority
        else {
            unreachable!()
        };
        request.authority = CloseCleanupFailureAuthority::CapturedScope { scope, resource };
        let completed = db
            .terminalize_initial_close_cleanup_failure(&request)
            .await
            .unwrap();
        assert_eq!(completed.snapshot(), Some(&snapshot));
        assert_eq!(
            db.terminalize_initial_close_cleanup_failure(&request)
                .await
                .unwrap(),
            completed
        );
        let counts: (i64, i64, i64) = sqlx::query_as(
            "SELECT (SELECT COUNT(*) FROM close_retirement_resources),
             (SELECT COUNT(*) FROM close_retirement_resource_history), (SELECT COUNT(*) FROM coordinator_watch_events)",
        ).fetch_one(db.pool()).await.unwrap();
        assert_eq!(counts, (0, 0, 1));
    }

    #[tokio::test]
    async fn initial_cleanup_failure_confirmed_stop_rejects_unresolved_processes() {
        for kind in [
            RetiredResourceKind::BashProcessGroup,
            RetiredResourceKind::TmuxServer,
            RetiredResourceKind::PtySession,
            RetiredResourceKind::BrowserSession,
            RetiredResourceKind::EquivalentLiveResource,
        ] {
            let db = Database::open_in_memory().await.unwrap();
            let process = RetiredResourceIdentity::parse(
                kind,
                LossItemIdentity::Opaque(OpaqueIdentity::parse("live-resource").unwrap()),
            )
            .unwrap();
            let mut request = initial_cleanup_failure_fixture_with_resources(
                &db,
                CloseStopCertainty::ConversationAndProcessesStopped { confirmed_at_us: 1 },
                vec![process.clone()],
            )
            .await;
            assert!(db
                .terminalize_initial_close_cleanup_failure(&request)
                .await
                .is_err());
            assert_initial_cleanup_unchanged(&db).await;
            let CloseCleanupFailureAuthority::ExpectedResource { snapshot, .. } =
                &request.authority
            else {
                unreachable!()
            };
            db.record_close_retirement_evidence(RecordCloseRetirementEvidenceRequest {
                attempt_id: request.attempt_id.clone(),
                scope: request.authority.scope().unwrap().clone(),
                snapshot: snapshot.clone(),
                resource: process,
                outcome: RetirementOutcome::Retired,
                detail: None,
            })
            .await
            .unwrap();
            request.remaining_resources = db
                .expected_close_cleanup_failure_resources(
                    request.attempt_id.as_str(),
                    request.authority.scope().unwrap(),
                    request.authority.resource().unwrap(),
                )
                .await
                .unwrap();
            db.terminalize_initial_close_cleanup_failure(&request)
                .await
                .unwrap();
        }
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn initial_cleanup_failure_terminalizes_both_certainties_and_exact_replay() {
        for certainty in [
            CloseStopCertainty::ConversationAndProcessesStopped {
                confirmed_at_us: Utc::now().timestamp_micros(),
            },
            CloseStopCertainty::ShutdownUncertain,
        ] {
            let db = Database::open_in_memory().await.unwrap();
            let request = initial_cleanup_failure_fixture(&db, certainty).await;
            let completed = db
                .terminalize_initial_close_cleanup_failure(&request)
                .await
                .unwrap();
            assert_eq!(completed.phase(), ClosePhase::Completed);
            assert_eq!(
                completed.close_outcome(),
                Some(certainty.completion_outcome())
            );
            assert_eq!(
                db.terminalize_initial_close_cleanup_failure(&request)
                    .await
                    .unwrap(),
                completed
            );
            let events: Vec<(String, String, i64)> = sqlx::query_as(
                "SELECT event_id, route_kind, occurred_at_us FROM coordinator_watch_events",
            )
            .fetch_all(db.pool())
            .await
            .unwrap();
            assert_eq!(
                events,
                vec![(
                    request.failure_occurrence_id.clone(),
                    "mandatory_close_failure".into(),
                    request.occurred_at_us
                )]
            );
            let archived = certainty.confirmed_at_us().is_some();
            for id in ["root", "latest", "participant"] {
                assert_eq!(db.get_conversation(id).await.unwrap().archived, archived);
            }
            let lifecycle: String = sqlx::query_scalar(
                "SELECT ordinary_lifecycle FROM product_conversations WHERE id = 'root'",
            )
            .fetch_one(db.pool())
            .await
            .unwrap();
            assert_eq!(lifecycle, if archived { "history" } else { "open" });
            let messages: Vec<(String, String)> = sqlx::query_as(
                "SELECT message_id, content FROM messages WHERE conversation_id = 'latest'",
            )
            .fetch_all(db.pool())
            .await
            .unwrap();
            assert_eq!(messages.len(), 1);
            assert_eq!(messages[0].0, "close-outcome:attempt-failure");
            assert!(messages[0].1.contains(if archived {
                "Closed — cleanup needs attention"
            } else {
                "Close incomplete — shutdown uncertain"
            }));
            let failure: (i64, String, Option<i64>, i64, String) = sqlx::query_as(
                "SELECT cleanup_run_ordinal, stop_certainty, confirmed_at_us, occurred_at_us, identity_codec FROM close_cleanup_failures",
            ).fetch_one(db.pool()).await.unwrap();
            assert_eq!(
                failure,
                (
                    1,
                    certainty.as_str().into(),
                    certainty.confirmed_at_us(),
                    request.occurred_at_us,
                    match &request.authority {
                        CloseCleanupFailureAuthority::AttemptInterrupted => unreachable!(),
                        CloseCleanupFailureAuthority::ExpectedResource { resource, .. }
                        | CloseCleanupFailureAuthority::CapturedScope { resource, .. }
                        | CloseCleanupFailureAuthority::ObservedProcessResource {
                            resource, ..
                        } => resource.identity().codec().into(),
                    }
                )
            );
            let evidence = db
                .list_close_retirement_evidence("attempt-failure")
                .await
                .unwrap();
            assert_eq!(evidence.len(), 1);
            assert_eq!(
                evidence[0].outcome,
                RetirementOutcome::Residual {
                    residual_reason: request.reason
                }
            );
            let mut changed = request.clone();
            changed.detail.push_str("different");
            assert!(db
                .terminalize_initial_close_cleanup_failure(&changed)
                .await
                .is_err());
            changed = request.clone();
            changed.failure_occurrence_id = "other-failure".into();
            assert!(db
                .terminalize_initial_close_cleanup_failure(&changed)
                .await
                .is_err());
            changed = request.clone();
            changed.occurred_at_us += 1;
            assert!(db
                .terminalize_initial_close_cleanup_failure(&changed)
                .await
                .is_err());
            changed = request.clone();
            changed.stop_certainty = if archived {
                CloseStopCertainty::ShutdownUncertain
            } else {
                CloseStopCertainty::ConversationAndProcessesStopped { confirmed_at_us: 1 }
            };
            assert!(db
                .terminalize_initial_close_cleanup_failure(&changed)
                .await
                .is_err());
            assert!(db
                .retry_close_retirement(&request.attempt_id)
                .await
                .is_err());
            let fk_errors = sqlx::query("PRAGMA foreign_key_check")
                .fetch_all(db.pool())
                .await
                .unwrap();
            assert!(fk_errors.is_empty());
        }
    }

    #[tokio::test]
    async fn initial_cleanup_failure_schema_enforces_certainty_ordinal_identity_and_immutability() {
        let db = Database::open_in_memory().await.unwrap();
        let request =
            initial_cleanup_failure_fixture(&db, CloseStopCertainty::ShutdownUncertain).await;
        db.terminalize_initial_close_cleanup_failure(&request)
            .await
            .unwrap();
        assert!(
            sqlx::query("UPDATE close_cleanup_failures SET detail = 'changed'")
                .execute(db.pool())
                .await
                .is_err()
        );
        let mut tx = db.pool().begin().await.unwrap();
        Database::admit_close_safe_retry_tx(&mut tx, &safe_retry_request(&request))
            .await
            .unwrap();
        sqlx::raw_sql("DROP TRIGGER close_cleanup_failures_require_initial_authority;")
            .execute(&mut *tx)
            .await
            .unwrap();
        for (ordinal, certainty, confirmed, occurred, codec, product) in [
            (-1, "shutdown_uncertain", None, 1, "worktree_id_v1", "root"),
            (0, "shutdown_uncertain", None, 1, "worktree_id_v1", "root"),
            (
                2,
                "shutdown_uncertain",
                Some(1),
                1,
                "worktree_id_v1",
                "root",
            ),
            (
                2,
                "conversation_and_processes_stopped",
                None,
                1,
                "worktree_id_v1",
                "root",
            ),
            (
                2,
                "conversation_and_processes_stopped",
                Some(-1),
                1,
                "worktree_id_v1",
                "root",
            ),
            (2, "shutdown_uncertain", None, -1, "worktree_id_v1", "root"),
            (
                2,
                "shutdown_uncertain",
                None,
                1,
                "worktree_id_v1",
                "missing-product",
            ),
        ] {
            assert!(sqlx::query(
                "INSERT INTO close_cleanup_failures SELECT 'close-failure:' || ?1 || ':' || attempt_id, attempt_id, ?1, ?6,
                 scope, inspection_generation, inspection_fingerprint, resource_kind, identity_kind,
                 ?5, identity_value, reason, detail, ?2, ?3, ?4, authority_kind FROM close_cleanup_failures
                 WHERE failure_occurrence_id = 'close-failure:1:attempt-failure'",
            )
            .bind(ordinal)
            .bind(certainty)
            .bind(confirmed)
            .bind(occurred)
            .bind(codec)
            .bind(product)
            .execute(&mut *tx)
            .await
            .is_err());
        }
        for (generation, fingerprint) in [(None, Some("partial")), (Some("partial"), None)] {
            assert!(sqlx::query(
                "INSERT INTO close_cleanup_failures SELECT 'close-failure:2:' || attempt_id, attempt_id, 2, source_product_conversation_id,
                 scope, ?1, ?2, resource_kind, identity_kind, identity_codec, identity_value, reason, detail,
                 stop_certainty, confirmed_at_us, occurred_at_us, authority_kind FROM close_cleanup_failures
                 WHERE failure_occurrence_id = 'close-failure:1:attempt-failure'",
            ).bind(generation).bind(fingerprint).execute(&mut *tx).await.is_err());
        }
        sqlx::query(
            "INSERT INTO close_cleanup_failures SELECT 'close-failure:2:' || attempt_id, attempt_id, 2, source_product_conversation_id,
             scope, inspection_generation, inspection_fingerprint, resource_kind, identity_kind,
             identity_codec, identity_value, reason, detail, 'conversation_and_processes_stopped', 1, 1, authority_kind
             FROM close_cleanup_failures WHERE failure_occurrence_id = 'close-failure:1:attempt-failure'",
        ).execute(&mut *tx).await.unwrap();
        tx.rollback().await.unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("PRAGMA writable_schema")
                .fetch_one(db.pool())
                .await
                .unwrap(),
            0
        );
        assert!(sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(db.pool())
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn initial_cleanup_failure_rolls_back_after_late_failure_and_caller_abort() {
        let db = Database::open_in_memory().await.unwrap();
        let request = initial_cleanup_failure_fixture(
            &db,
            CloseStopCertainty::ConversationAndProcessesStopped { confirmed_at_us: 1 },
        )
        .await;
        sqlx::raw_sql("CREATE TRIGGER injected_cleanup_crash BEFORE UPDATE OF phase ON close_obligations WHEN NEW.phase = 'completed' BEGIN SELECT RAISE(ABORT, 'injected crash'); END;")
            .execute(db.pool()).await.unwrap();
        assert!(db
            .terminalize_initial_close_cleanup_failure(&request)
            .await
            .is_err());
        assert_initial_cleanup_unchanged(&db).await;
        sqlx::raw_sql("DROP TRIGGER injected_cleanup_crash;")
            .execute(db.pool())
            .await
            .unwrap();
        let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await.unwrap();
        Database::terminalize_initial_close_cleanup_failure_tx(&mut tx, &request)
            .await
            .unwrap();
        tx.rollback().await.unwrap();
        assert_initial_cleanup_unchanged(&db).await;
        db.terminalize_initial_close_cleanup_failure(&request)
            .await
            .unwrap();
    }

    async fn assert_initial_cleanup_unchanged(db: &Database) {
        assert_eq!(
            db.get_close_run(&CloseRunRef::initial(
                CloseAttemptId::parse("attempt-failure").unwrap()
            ))
            .await
            .unwrap()
            .status,
            CloseRunStatus::Running
        );
        assert_eq!(
            db.get_close_obligation("attempt-failure")
                .await
                .unwrap()
                .phase(),
            ClosePhase::RetirementRequested
        );
        assert!(!db.get_conversation("latest").await.unwrap().archived);
        assert!(!db.get_conversation("participant").await.unwrap().archived);
        let counts: (i64, i64, i64, i64, i64, i64) = sqlx::query_as(
            "SELECT (SELECT COUNT(*) FROM close_cleanup_failures), (SELECT COUNT(*) FROM close_retirement_resources),
             (SELECT COUNT(*) FROM close_retirement_resource_history), (SELECT COUNT(*) FROM messages),
             (SELECT COUNT(*) FROM coordinator_watch_events), (SELECT COUNT(*) FROM close_cleanup_failure_resources)",
        ).fetch_one(db.pool()).await.unwrap();
        assert_eq!(counts, (0, 0, 0, 0, 0, 0));
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT ordinary_lifecycle FROM product_conversations WHERE id = 'root'"
            )
            .fetch_one(db.pool())
            .await
            .unwrap(),
            "open"
        );
    }

    #[tokio::test]
    async fn initial_cleanup_failure_rejects_stale_snapshot_resource_and_transition_without_occurrence(
    ) {
        let db = Database::open_in_memory().await.unwrap();
        let request =
            initial_cleanup_failure_fixture(&db, CloseStopCertainty::ShutdownUncertain).await;
        let mut changed = request.clone();
        let CloseCleanupFailureAuthority::ExpectedResource {
            snapshot, resource, ..
        } = &request.authority
        else {
            panic!("expected inventory authority")
        };
        changed.authority = CloseCleanupFailureAuthority::ExpectedResource {
            scope: request.authority.scope().unwrap().clone(),
            snapshot: CloseRetirementSnapshot::parse("stale", "stale").unwrap(),
            resource: resource.clone(),
        };
        assert!(db
            .terminalize_initial_close_cleanup_failure(&changed)
            .await
            .is_err());
        changed = request.clone();
        changed.authority = CloseCleanupFailureAuthority::ExpectedResource {
            scope: request.authority.scope().unwrap().clone(),
            snapshot: snapshot.clone(),
            resource: RetiredResourceIdentity::parse(
                RetiredResourceKind::BrowserSession,
                LossItemIdentity::Opaque(OpaqueIdentity::parse("not-in-inventory").unwrap()),
            )
            .unwrap(),
        };
        assert!(db
            .terminalize_initial_close_cleanup_failure(&changed)
            .await
            .is_err());
        for outcome in ["close_incomplete", "archived_cleanup_attention"] {
            assert!(sqlx::query("UPDATE close_obligations SET phase = 'completed', close_outcome = ?1, completed_at = updated_at WHERE attempt_id = 'attempt-failure'")
                .bind(outcome).execute(db.pool()).await.is_err());
        }
        assert_initial_cleanup_unchanged(&db).await;
    }

    #[tokio::test]
    async fn retirement_completion_persists_outcome_before_history_transition() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        let scope = allocate_scope_worktree(&db, "root").await;
        create_child(&db, "latest", "root").await;
        db.create_subagent_conversation(
            "completion-subordinate",
            "completion-subordinate",
            "/tmp",
            "latest",
            "test-model",
            &crate::ConvMode::Direct,
            phoenix_core::llm_language::LlmLanguage::default(),
            db.get_conversation("latest")
                .await
                .unwrap()
                .attached_work_scope_id
                .as_ref(),
            crate::SubAgentExecution {
                connection: "mock",
                effort: None,
                persona: None,
            },
        )
        .await
        .unwrap();
        db.begin_close_foundation(
            &product_id("root"),
            &transcript_id("latest"),
            "attempt-complete",
        )
        .await
        .unwrap();
        set_close_phase(
            &db,
            "attempt-complete",
            ClosePhase::AwaitingRetirementInspection,
        )
        .await;
        db.replace_close_inspection(ReplaceCloseInspectionRequest {
            attempt_id: CloseAttemptId::parse("attempt-complete").unwrap(),
            scopes: vec![ReplaceCloseInspectionScopeRequest {
                scope: scope.clone(),
                snapshot: CloseRetirementSnapshot::parse("complete-gen", "complete-fp").unwrap(),
                losses: Vec::new(),
            }],
        })
        .await
        .unwrap();
        let snapshot = current_test_snapshot(&db, "attempt-complete").await;
        capture_test_inventory(&db, "attempt-complete", &scope, &snapshot, Vec::new()).await;
        for expected in db
            .list_close_expected_retirement_resources("attempt-complete")
            .await
            .unwrap()
        {
            db.record_close_retirement_evidence(RecordCloseRetirementEvidenceRequest {
                attempt_id: CloseAttemptId::parse("attempt-complete").unwrap(),
                snapshot: snapshot.clone(),
                scope: expected.scope,
                resource: expected.resource,
                outcome: RetirementOutcome::Retired,
                detail: None,
            })
            .await
            .unwrap();
        }

        let completed = db
            .complete_close_retirement(&CloseAttemptId::parse("attempt-complete").unwrap())
            .await
            .unwrap();

        assert_eq!(completed.phase(), ClosePhase::Completed);
        assert_eq!(
            completed.close_outcome(),
            Some(CloseCompletionOutcome::Archived)
        );
        let lifecycle: String = sqlx::query_scalar(
            "SELECT ordinary_lifecycle FROM product_conversations WHERE id = ?1",
        )
        .bind("root")
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(lifecycle, "history");
        assert!(
            db.get_conversation("completion-subordinate")
                .await
                .unwrap()
                .archived
        );
        let outcome: (String, String, String) = sqlx::query_as(
            "SELECT message_id, message_type, content
             FROM messages WHERE conversation_id = ?1",
        )
        .bind("latest")
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(outcome.0, "close-outcome:attempt-complete");
        assert_eq!(outcome.1, "system");
        assert!(outcome.2.contains("attempt-complete"));
    }

    #[tokio::test]
    async fn successful_retirement_completion_replay_is_idempotent() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        let scope = allocate_scope_worktree(&db, "root").await;
        create_child(&db, "latest", "root").await;
        db.begin_close_foundation(
            &product_id("root"),
            &transcript_id("latest"),
            "attempt-replay-complete",
        )
        .await
        .unwrap();
        set_close_phase(
            &db,
            "attempt-replay-complete",
            ClosePhase::AwaitingRetirementInspection,
        )
        .await;
        db.replace_close_inspection(ReplaceCloseInspectionRequest {
            attempt_id: CloseAttemptId::parse("attempt-replay-complete").unwrap(),
            scopes: vec![ReplaceCloseInspectionScopeRequest {
                scope: scope.clone(),
                snapshot: CloseRetirementSnapshot::parse("complete-gen", "complete-fp").unwrap(),
                losses: Vec::new(),
            }],
        })
        .await
        .unwrap();
        let snapshot = current_test_snapshot(&db, "attempt-replay-complete").await;
        capture_test_inventory(
            &db,
            "attempt-replay-complete",
            &scope,
            &snapshot,
            Vec::new(),
        )
        .await;
        for expected in db
            .list_close_expected_retirement_resources("attempt-replay-complete")
            .await
            .unwrap()
        {
            db.record_close_retirement_evidence(RecordCloseRetirementEvidenceRequest {
                attempt_id: CloseAttemptId::parse("attempt-replay-complete").unwrap(),
                snapshot: snapshot.clone(),
                scope: expected.scope,
                resource: expected.resource,
                outcome: RetirementOutcome::Retired,
                detail: None,
            })
            .await
            .unwrap();
        }
        let attempt = CloseAttemptId::parse("attempt-replay-complete").unwrap();
        let first = db.complete_close_retirement(&attempt).await.unwrap();
        let replay = db.complete_close_retirement(&attempt).await.unwrap();
        assert_eq!(first, replay);
        let outcome_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM messages WHERE message_id = 'close-outcome:attempt-replay-complete'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(outcome_count, 1);
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn retirement_evidence_round_trips_and_rejects_divergent_replay() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        create_child(&db, "leaf", "root").await;
        allocate_scope_worktree(&db, "root").await;

        let scope = db
            .get_conversation("root")
            .await
            .unwrap()
            .attached_work_scope_id
            .unwrap();
        db.begin_close_foundation(&product_id("root"), &transcript_id("leaf"), "attempt-1")
            .await
            .unwrap();
        set_close_phase(&db, "attempt-1", ClosePhase::RetirementRequested).await;

        let retired_identity = LossItemIdentity::Worktree(current_test_worktree(&db, &scope).await);
        let browser_identity =
            LossItemIdentity::Opaque(OpaqueIdentity::parse("browser:1").unwrap());
        let equivalent_identity = LossItemIdentity::Opaque(
            OpaqueIdentity::parse("equivalent:abcdefabcdefabcdefabcdefabcdefabcdefabcd").unwrap(),
        );
        let snapshot = current_test_snapshot(&db, "attempt-1").await;
        capture_test_inventory(
            &db,
            "attempt-1",
            &scope,
            &snapshot,
            vec![
                RetiredResourceIdentity::parse(
                    RetiredResourceKind::Worktree,
                    retired_identity.clone(),
                )
                .unwrap(),
                RetiredResourceIdentity::parse(
                    RetiredResourceKind::BrowserSession,
                    browser_identity.clone(),
                )
                .unwrap(),
                RetiredResourceIdentity::parse(
                    RetiredResourceKind::EquivalentLiveResource,
                    equivalent_identity.clone(),
                )
                .unwrap(),
            ],
        )
        .await;
        let mismatched_worktree = match &retired_identity {
            LossItemIdentity::Worktree(identity) => {
                LossItemIdentity::Worktree(WorktreeIdentity::from_parts(
                    phoenix_core::domain::close::WorktreeId::parse(identity.id().as_str()).unwrap(),
                    phoenix_core::domain::close::WorktreeFingerprint::parse(
                        "replacement-fingerprint",
                    )
                    .unwrap(),
                    identity.locator().clone(),
                ))
            }
            LossItemIdentity::GitPath(_)
            | LossItemIdentity::GitOid(_)
            | LossItemIdentity::Opaque(_) => unreachable!(),
        };
        let mismatch = db
            .record_close_retirement_evidence(RecordCloseRetirementEvidenceRequest {
                attempt_id: CloseAttemptId::parse("attempt-1").unwrap(),
                snapshot: current_test_snapshot(&db, "attempt-1").await,
                scope: scope.clone(),
                resource: RetiredResourceIdentity::parse(
                    RetiredResourceKind::Worktree,
                    mismatched_worktree,
                )
                .unwrap(),
                outcome: RetirementOutcome::Retired,
                detail: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(mismatch, DbError::CloseFoundationPrecondition(_)));

        db.record_close_retirement_evidence(RecordCloseRetirementEvidenceRequest {
            attempt_id: CloseAttemptId::parse("attempt-1").unwrap(),
            snapshot: current_test_snapshot(&db, "attempt-1").await,
            scope: scope.clone(),
            resource: RetiredResourceIdentity::parse(
                RetiredResourceKind::Worktree,
                retired_identity.clone(),
            )
            .unwrap(),
            outcome: RetirementOutcome::Retired,
            detail: None,
        })
        .await
        .unwrap();
        db.record_close_retirement_evidence(RecordCloseRetirementEvidenceRequest {
            attempt_id: CloseAttemptId::parse("attempt-1").unwrap(),
            snapshot: current_test_snapshot(&db, "attempt-1").await,
            scope: scope.clone(),
            resource: RetiredResourceIdentity::parse(
                RetiredResourceKind::Worktree,
                retired_identity.clone(),
            )
            .unwrap(),
            outcome: RetirementOutcome::Retired,
            detail: None,
        })
        .await
        .unwrap();

        let browser_identity =
            LossItemIdentity::Opaque(OpaqueIdentity::parse("browser:1").unwrap());
        db.record_close_retirement_evidence(RecordCloseRetirementEvidenceRequest {
            attempt_id: CloseAttemptId::parse("attempt-1").unwrap(),
            snapshot: current_test_snapshot(&db, "attempt-1").await,
            scope: scope.clone(),
            resource: RetiredResourceIdentity::parse(
                RetiredResourceKind::BrowserSession,
                browser_identity.clone(),
            )
            .unwrap(),
            outcome: RetirementOutcome::Retired,
            detail: Some("retired before absence".to_string()),
        })
        .await
        .unwrap();
        let browser_resource =
            RetiredResourceIdentity::parse(RetiredResourceKind::BrowserSession, browser_identity)
                .unwrap();
        db.record_close_retirement_evidence(RecordCloseRetirementEvidenceRequest {
            attempt_id: CloseAttemptId::parse("attempt-1").unwrap(),
            snapshot: current_test_snapshot(&db, "attempt-1").await,
            scope: scope.clone(),
            resource: browser_resource.clone(),
            outcome: RetirementOutcome::AbsenceAdopted {
                absence_basis: AbsenceBasis::SameAttemptPriorRetirement,
            },
            detail: Some("prior evidence matched".to_string()),
        })
        .await
        .unwrap();
        let divergent = db
            .record_close_retirement_evidence(RecordCloseRetirementEvidenceRequest {
                attempt_id: CloseAttemptId::parse("attempt-1").unwrap(),
                snapshot: current_test_snapshot(&db, "attempt-1").await,
                scope: scope.clone(),
                resource: browser_resource,
                outcome: RetirementOutcome::Retired,
                detail: Some("different retirement detail".to_string()),
            })
            .await
            .unwrap_err();
        assert!(matches!(
            divergent,
            DbError::CloseFoundationPrecondition(message)
                if message.contains("replay differs from persisted evidence")
        ));
        db.record_close_retirement_evidence(RecordCloseRetirementEvidenceRequest {
            attempt_id: CloseAttemptId::parse("attempt-1").unwrap(),
            snapshot: current_test_snapshot(&db, "attempt-1").await,
            scope: scope.clone(),
            resource: RetiredResourceIdentity::parse(
                RetiredResourceKind::EquivalentLiveResource,
                equivalent_identity,
            )
            .unwrap(),
            outcome: RetirementOutcome::Residual {
                residual_reason: RetirementFailureReason::ManualRepairRequired,
            },
            detail: Some("manual cleanup".to_string()),
        })
        .await
        .unwrap();

        let evidence = db
            .list_close_retirement_evidence("attempt-1")
            .await
            .unwrap();
        assert_eq!(evidence.len(), 3);
        assert!(evidence
            .iter()
            .any(|item| item.resource.kind() == RetiredResourceKind::Worktree
                && item.resource.identity() == &retired_identity
                && item.outcome == RetirementOutcome::Retired
                && item.detail.is_none()));
        assert!(evidence.iter().any(|item| item.resource.kind()
            == RetiredResourceKind::BrowserSession
            && item.outcome == RetirementOutcome::Retired
            && item.detail.as_deref() == Some("retired before absence")));
        assert!(evidence.iter().any(|item| matches!(
            item.outcome,
            RetirementOutcome::Residual {
                residual_reason: RetirementFailureReason::ManualRepairRequired
            }
        ) && item.detail.as_deref() == Some("manual cleanup")));
    }

    #[tokio::test]
    async fn worktree_cleanup_plan_round_trips_exact_path_and_rejects_divergence() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        let scope = allocate_scope_worktree(&db, "root").await;
        db.begin_close_foundation(&product_id("root"), &transcript_id("root"), "attempt-1")
            .await
            .unwrap();
        set_close_phase(&db, "attempt-1", ClosePhase::RetirementRequested).await;
        let snapshot = current_test_snapshot(&db, "attempt-1").await;
        let worktree = current_test_worktree(&db, &scope).await;
        let resource = RetiredResourceIdentity::parse(
            RetiredResourceKind::Worktree,
            LossItemIdentity::Worktree(worktree),
        )
        .unwrap();
        capture_test_inventory(&db, "attempt-1", &scope, &snapshot, vec![resource.clone()]).await;
        let attempt_id = CloseAttemptId::parse("attempt-1").unwrap();
        db.record_close_retirement_dispatch(RecordCloseRetirementDispatchRequest {
            attempt_id: attempt_id.clone(),
            scope: scope.clone(),
            snapshot: snapshot.clone(),
            resource: resource.clone(),
        })
        .await
        .unwrap();
        let administrative_dir = std::path::PathBuf::from("/tmp/git/worktrees/exact");
        let request = RecordCloseWorktreeCleanupPlanRequest {
            attempt_id: attempt_id.clone(),
            scope: scope.clone(),
            snapshot: snapshot.clone(),
            resource: resource.clone(),
            administrative_dir: administrative_dir.clone(),
            administrative_dir_incarnation: "admin-v1".to_string(),
        };
        db.record_close_worktree_cleanup_plan(request.clone())
            .await
            .unwrap();
        db.record_close_worktree_cleanup_plan(request)
            .await
            .unwrap();
        assert_eq!(
            db.close_worktree_cleanup_plan(&attempt_id, &scope, &snapshot, &resource)
                .await
                .unwrap(),
            Some(CloseWorktreeCleanupPlan {
                administrative_dir,
                administrative_dir_incarnation: "admin-v1".to_string(),
                final_tombstone: None,
            })
        );
        let timestamp_types: (String, String, i64, i64) = sqlx::query_as(
            "SELECT typeof(dispatch.dispatched_at_us), typeof(plan.planned_at_us),
                    dispatch.dispatched_at_us, plan.planned_at_us
             FROM close_retirement_resource_dispatches dispatch
             JOIN close_worktree_cleanup_plans plan
               ON plan.attempt_id = dispatch.attempt_id
              AND plan.scope = dispatch.scope
              AND plan.inspection_generation = dispatch.inspection_generation
              AND plan.inspection_fingerprint = dispatch.inspection_fingerprint
              AND plan.resource_kind = dispatch.resource_kind
              AND plan.identity_kind = dispatch.identity_kind
              AND plan.identity_value = dispatch.identity_value",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(timestamp_types.0, "integer");
        assert_eq!(timestamp_types.1, "integer");
        assert!(timestamp_types.2 >= 0);
        assert!(timestamp_types.3 >= 0);
        assert!(sqlx::query(
            "UPDATE close_retirement_resource_dispatches SET dispatched_at_us = -1"
        )
        .execute(db.pool())
        .await
        .is_err());
        assert!(sqlx::query(
            "UPDATE close_worktree_cleanup_plans SET planned_at_us = 'not-a-timestamp'"
        )
        .execute(db.pool())
        .await
        .is_err());

        let divergent = db
            .record_close_worktree_cleanup_plan(RecordCloseWorktreeCleanupPlanRequest {
                attempt_id,
                scope,
                snapshot,
                resource,
                administrative_dir: std::path::PathBuf::from("/tmp/git/worktrees/replacement"),
                administrative_dir_incarnation: "admin-v2".to_string(),
            })
            .await
            .unwrap_err();
        assert!(matches!(divergent, DbError::CloseFoundationPrecondition(_)));
    }

    #[tokio::test]
    async fn interrupted_worktree_cleanup_adopts_absence_only_with_exact_durable_plan() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        let scope = allocate_scope_worktree(&db, "root").await;
        db.begin_close_foundation(
            &product_id("root"),
            &transcript_id("root"),
            "attempt-crash-boundary",
        )
        .await
        .unwrap();
        set_close_phase(
            &db,
            "attempt-crash-boundary",
            ClosePhase::RetirementRequested,
        )
        .await;
        let attempt_id = CloseAttemptId::parse("attempt-crash-boundary").unwrap();
        let snapshot = current_test_snapshot(&db, attempt_id.as_str()).await;
        let resource = RetiredResourceIdentity::parse(
            RetiredResourceKind::Worktree,
            LossItemIdentity::Worktree(current_test_worktree(&db, &scope).await),
        )
        .unwrap();
        capture_test_inventory(
            &db,
            attempt_id.as_str(),
            &scope,
            &snapshot,
            vec![resource.clone()],
        )
        .await;
        db.record_close_retirement_dispatch(RecordCloseRetirementDispatchRequest {
            attempt_id: attempt_id.clone(),
            scope: scope.clone(),
            snapshot: snapshot.clone(),
            resource: resource.clone(),
        })
        .await
        .unwrap();
        let absence = || RecordCloseRetirementEvidenceRequest {
            attempt_id: attempt_id.clone(),
            scope: scope.clone(),
            snapshot: snapshot.clone(),
            resource: resource.clone(),
            outcome: RetirementOutcome::AbsenceAdopted {
                absence_basis: AbsenceBasis::SameAttemptPriorRetirement,
            },
            detail: Some("restart observed completed planned cleanup".to_string()),
        };

        let missing_plan = db
            .record_close_retirement_evidence(absence())
            .await
            .unwrap_err();
        assert!(matches!(
            missing_plan,
            DbError::CloseFoundationPrecondition(_)
        ));

        db.record_close_worktree_cleanup_plan(RecordCloseWorktreeCleanupPlanRequest {
            attempt_id: attempt_id.clone(),
            scope: scope.clone(),
            snapshot: snapshot.clone(),
            resource: resource.clone(),
            administrative_dir: std::path::PathBuf::from("/tmp/crash-boundary-admin"),
            administrative_dir_incarnation: "admin-crash-v1".to_string(),
        })
        .await
        .unwrap();
        db.record_close_retirement_evidence(absence())
            .await
            .unwrap();

        let evidence = db
            .list_close_retirement_evidence("attempt-crash-boundary")
            .await
            .unwrap();
        assert_eq!(evidence.len(), 1);
        assert_eq!(
            evidence[0].outcome,
            RetirementOutcome::AbsenceAdopted {
                absence_basis: AbsenceBasis::SameAttemptPriorRetirement,
            }
        );
    }

    #[tokio::test]
    async fn retirement_absence_requires_retained_exact_identity_evidence() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        allocate_scope_worktree(&db, "root").await;

        let scope = db
            .get_conversation("root")
            .await
            .unwrap()
            .attached_work_scope_id
            .unwrap();
        db.begin_close_foundation(&product_id("root"), &transcript_id("root"), "attempt-1")
            .await
            .unwrap();
        set_close_phase(&db, "attempt-1", ClosePhase::RetirementRequested).await;
        let identity = LossItemIdentity::Opaque(OpaqueIdentity::parse("browser:1").unwrap());
        let snapshot = current_test_snapshot(&db, "attempt-1").await;
        capture_test_inventory(
            &db,
            "attempt-1",
            &scope,
            &snapshot,
            vec![RetiredResourceIdentity::parse(
                RetiredResourceKind::BrowserSession,
                identity.clone(),
            )
            .unwrap()],
        )
        .await;

        for absence_basis in [
            AbsenceBasis::SameAttemptPriorRetirement,
            AbsenceBasis::PreexistingExactIdentityEvidence,
        ] {
            let error = db
                .record_close_retirement_evidence(RecordCloseRetirementEvidenceRequest {
                    attempt_id: CloseAttemptId::parse("attempt-1").unwrap(),
                    snapshot: current_test_snapshot(&db, "attempt-1").await,
                    scope: scope.clone(),
                    resource: RetiredResourceIdentity::parse(
                        RetiredResourceKind::BrowserSession,
                        identity.clone(),
                    )
                    .unwrap(),
                    outcome: RetirementOutcome::AbsenceAdopted { absence_basis },
                    detail: None,
                })
                .await
                .unwrap_err();
            assert!(matches!(error, DbError::CloseFoundationPrecondition(_)));
        }

        db.record_close_retirement_evidence(RecordCloseRetirementEvidenceRequest {
            attempt_id: CloseAttemptId::parse("attempt-1").unwrap(),
            snapshot: current_test_snapshot(&db, "attempt-1").await,
            scope: scope.clone(),
            resource: RetiredResourceIdentity::parse(
                RetiredResourceKind::BrowserSession,
                identity.clone(),
            )
            .unwrap(),
            outcome: RetirementOutcome::Retired,
            detail: None,
        })
        .await
        .unwrap();
        db.record_close_retirement_evidence(RecordCloseRetirementEvidenceRequest {
            attempt_id: CloseAttemptId::parse("attempt-1").unwrap(),
            snapshot: current_test_snapshot(&db, "attempt-1").await,
            scope,
            resource: RetiredResourceIdentity::parse(RetiredResourceKind::BrowserSession, identity)
                .unwrap(),
            outcome: RetirementOutcome::AbsenceAdopted {
                absence_basis: AbsenceBasis::SameAttemptPriorRetirement,
            },
            detail: None,
        })
        .await
        .unwrap();

        let evidence = db
            .list_close_retirement_evidence("attempt-1")
            .await
            .unwrap();
        assert_eq!(evidence.len(), 1);
        assert_eq!(evidence[0].outcome, RetirementOutcome::Retired);
        assert!(evidence[0].detail.is_none());
    }

    #[tokio::test]
    async fn concurrent_identical_retirement_evidence_is_idempotent() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        let scope = allocate_scope_worktree(&db, "root").await;
        db.begin_close_foundation(
            &product_id("root"),
            &transcript_id("root"),
            "attempt-concurrent-evidence",
        )
        .await
        .unwrap();
        set_close_phase(
            &db,
            "attempt-concurrent-evidence",
            ClosePhase::RetirementRequested,
        )
        .await;
        let snapshot = current_test_snapshot(&db, "attempt-concurrent-evidence").await;
        let resource = RetiredResourceIdentity::parse(
            RetiredResourceKind::BrowserSession,
            LossItemIdentity::Opaque(OpaqueIdentity::parse("browser-1").unwrap()),
        )
        .unwrap();
        capture_test_inventory(
            &db,
            "attempt-concurrent-evidence",
            &scope,
            &snapshot,
            vec![resource.clone()],
        )
        .await;
        let request = RecordCloseRetirementEvidenceRequest {
            attempt_id: CloseAttemptId::parse("attempt-concurrent-evidence").unwrap(),
            snapshot,
            scope,
            resource,
            outcome: RetirementOutcome::Retired,
            detail: None,
        };

        let (first, second) = tokio::join!(
            db.record_close_retirement_evidence(request.clone()),
            db.record_close_retirement_evidence(request)
        );
        first.unwrap();
        second.unwrap();
        assert_eq!(
            db.list_close_retirement_evidence("attempt-concurrent-evidence")
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn retirement_evidence_rejects_early_phase_and_allows_needs_repair() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        create_child(&db, "leaf", "root").await;
        allocate_scope_worktree(&db, "root").await;

        let scope = db
            .get_conversation("root")
            .await
            .unwrap()
            .attached_work_scope_id
            .unwrap();
        db.begin_close_foundation(&product_id("root"), &transcript_id("leaf"), "attempt-1")
            .await
            .unwrap();

        let req = RecordCloseRetirementEvidenceRequest {
            attempt_id: CloseAttemptId::parse("attempt-1").unwrap(),
            snapshot: CloseRetirementSnapshot::parse("not-inspected", "not-inspected").unwrap(),
            scope: scope.clone(),
            resource: RetiredResourceIdentity::parse(
                RetiredResourceKind::BrowserSession,
                LossItemIdentity::Opaque(OpaqueIdentity::parse("worktree:/tmp/root").unwrap()),
            )
            .unwrap(),
            outcome: RetirementOutcome::Retired,
            detail: None,
        };
        let err = db
            .record_close_retirement_evidence(req.clone())
            .await
            .unwrap_err();
        assert!(matches!(err, DbError::CloseFoundationPrecondition(_)));

        set_close_phase(&db, "attempt-1", ClosePhase::RetirementRequested).await;
        let snapshot = current_test_snapshot(&db, "attempt-1").await;
        let req = RecordCloseRetirementEvidenceRequest {
            snapshot: snapshot.clone(),
            outcome: RetirementOutcome::Residual {
                residual_reason: RetirementFailureReason::ManualRepairRequired,
            },
            ..req
        };
        capture_test_inventory(
            &db,
            "attempt-1",
            &scope,
            &snapshot,
            vec![req.resource.clone()],
        )
        .await;
        db.record_close_retirement_evidence(req.clone())
            .await
            .unwrap();
        assert_eq!(
            db.get_close_obligation("attempt-1").await.unwrap().phase(),
            ClosePhase::NeedsRepair
        );
        db.record_close_retirement_evidence(req.clone())
            .await
            .unwrap();
        assert_eq!(
            db.list_close_retirement_evidence("attempt-1")
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn retirement_evidence_is_monotonic_after_first_proof() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        let scope = allocate_scope_worktree(&db, "root").await;
        db.begin_close_foundation(&product_id("root"), &transcript_id("root"), "attempt-1")
            .await
            .unwrap();
        set_close_phase(&db, "attempt-1", ClosePhase::RetirementRequested).await;
        let snapshot = current_test_snapshot(&db, "attempt-1").await;
        capture_test_inventory(
            &db,
            "attempt-1",
            &scope,
            &snapshot,
            vec![RetiredResourceIdentity::parse(
                RetiredResourceKind::Worktree,
                LossItemIdentity::Worktree(current_test_worktree(&db, &scope).await),
            )
            .unwrap()],
        )
        .await;
        let worktree = current_test_worktree(&db, &scope).await;
        let resource = RetiredResourceIdentity::parse(
            RetiredResourceKind::Worktree,
            LossItemIdentity::Worktree(worktree),
        )
        .unwrap();
        let request = RecordCloseRetirementEvidenceRequest {
            attempt_id: CloseAttemptId::parse("attempt-1").unwrap(),
            snapshot: snapshot.clone(),
            scope,
            resource,
            outcome: RetirementOutcome::Retired,
            detail: Some("exact retired proof".to_string()),
        };
        db.record_close_retirement_evidence(request.clone())
            .await
            .unwrap();
        db.record_close_retirement_evidence(RecordCloseRetirementEvidenceRequest {
            outcome: RetirementOutcome::AbsenceAdopted {
                absence_basis: AbsenceBasis::SameAttemptPriorRetirement,
            },
            detail: Some("already retired is absent on replay".to_string()),
            ..request
        })
        .await
        .unwrap();
        assert_eq!(
            db.list_close_retirement_evidence("attempt-1")
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn delayed_retirement_evidence_requires_retained_aggregate_snapshot() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        create_child(&db, "leaf", "root").await;
        let root_scope = allocate_scope_worktree(&db, "root").await;
        let leaf_scope = WorkScopeId::parse("close-scope-üther").unwrap();
        sqlx::query(
            "INSERT INTO work_scopes (
                id, authority_kind, lifecycle, environment_kind, cwd,
                worktree_path, branch_name, base_branch, created_at, updated_at,
                worktree_id, worktree_fingerprint
             ) VALUES (
                ?1, 'work', 'active', 'allocated_worktree', '/tmp', '/tmp/other',
                'branch', 'main', ?2, ?2, lower(hex(randomblob(16))), lower(hex(randomblob(32)))
             )",
        )
        .bind(leaf_scope.as_str())
        .bind(Utc::now().to_rfc3339())
        .execute(db.pool())
        .await
        .unwrap();
        sqlx::query("UPDATE conversations SET work_scope_id = ?1 WHERE id = 'leaf'")
            .bind(leaf_scope.as_str())
            .execute(db.pool())
            .await
            .unwrap();
        db.begin_close_foundation(&product_id("root"), &transcript_id("leaf"), "attempt-1")
            .await
            .unwrap();
        set_close_phase(&db, "attempt-1", ClosePhase::AwaitingRetirementInspection).await;
        db.replace_close_inspection(ReplaceCloseInspectionRequest {
            attempt_id: CloseAttemptId::parse("attempt-1").unwrap(),
            scopes: vec![
                ReplaceCloseInspectionScopeRequest {
                    scope: root_scope.clone(),
                    snapshot: CloseRetirementSnapshot::parse("root-gen", "root-fp").unwrap(),
                    losses: vec![CloseLossItem::UntrackedNonIgnoredPath(
                        GitPathIdentity::from_bytes(b"delayed-loss".to_vec()),
                    )],
                },
                ReplaceCloseInspectionScopeRequest {
                    scope: leaf_scope,
                    snapshot: CloseRetirementSnapshot::parse("leaf-gén", "leaf-fp-ß").unwrap(),
                    losses: Vec::new(),
                },
            ],
        })
        .await
        .unwrap();
        let aggregate_snapshot = db
            .get_close_obligation("attempt-1")
            .await
            .unwrap()
            .snapshot()
            .unwrap()
            .clone();
        let identity = LossItemIdentity::Opaque(OpaqueIdentity::parse("browser:delayed").unwrap());
        sqlx::query(
            "UPDATE close_obligations SET phase = 'retirement_requested' WHERE attempt_id = 'attempt-1'",
        )
        .execute(db.pool())
        .await
        .unwrap();
        capture_test_inventory(
            &db,
            "attempt-1",
            &root_scope,
            &aggregate_snapshot,
            vec![RetiredResourceIdentity::parse(
                RetiredResourceKind::BrowserSession,
                identity.clone(),
            )
            .unwrap()],
        )
        .await;
        let per_scope_error = db
            .record_close_retirement_evidence(RecordCloseRetirementEvidenceRequest {
                attempt_id: CloseAttemptId::parse("attempt-1").unwrap(),
                snapshot: CloseRetirementSnapshot::parse("root-gen", "root-fp").unwrap(),
                scope: root_scope.clone(),
                resource: RetiredResourceIdentity::parse(
                    RetiredResourceKind::BrowserSession,
                    identity.clone(),
                )
                .unwrap(),
                outcome: RetirementOutcome::Retired,
                detail: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(
            per_scope_error,
            DbError::CloseFoundationPrecondition(_)
        ));

        db.record_close_retirement_evidence(RecordCloseRetirementEvidenceRequest {
            attempt_id: CloseAttemptId::parse("attempt-1").unwrap(),
            snapshot: aggregate_snapshot.clone(),
            scope: root_scope,
            resource: RetiredResourceIdentity::parse(RetiredResourceKind::BrowserSession, identity)
                .unwrap(),
            outcome: RetirementOutcome::Retired,
            detail: None,
        })
        .await
        .unwrap();
        assert_eq!(
            db.list_close_retirement_evidence("attempt-1")
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn cancel_close_before_retirement_clears_snapshot_and_completes() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        let scope = allocate_scope_worktree(&db, "root").await;
        db.begin_close_foundation(
            &product_id("root"),
            &transcript_id("root"),
            "attempt-cancel-before-retirement",
        )
        .await
        .unwrap();
        set_close_phase(
            &db,
            "attempt-cancel-before-retirement",
            ClosePhase::AwaitingRetirementInspection,
        )
        .await;
        db.replace_close_inspection(ReplaceCloseInspectionRequest {
            attempt_id: CloseAttemptId::parse("attempt-cancel-before-retirement").unwrap(),
            scopes: vec![ReplaceCloseInspectionScopeRequest {
                scope,
                snapshot: CloseRetirementSnapshot::parse("scope-gen", "scope-fp").unwrap(),
                losses: vec![CloseLossItem::UntrackedNonIgnoredPath(
                    GitPathIdentity::from_bytes(b"dirty.txt".to_vec()),
                )],
            }],
        })
        .await
        .unwrap();
        let awaiting = db
            .get_close_obligation("attempt-cancel-before-retirement")
            .await
            .unwrap();
        assert_eq!(awaiting.phase(), ClosePhase::AwaitingLossConfirmation);
        assert!(awaiting.snapshot().is_some());

        let cancelled = db
            .cancel_close_before_retirement("attempt-cancel-before-retirement")
            .await
            .unwrap();
        assert_eq!(cancelled.phase(), ClosePhase::Completed);
        assert_eq!(
            cancelled.close_outcome(),
            Some(CloseCompletionOutcome::Cancelled)
        );
        assert!(cancelled.snapshot().is_none());

        let stored = db
            .get_close_obligation("attempt-cancel-before-retirement")
            .await
            .unwrap();
        assert_eq!(stored.phase(), ClosePhase::Completed);
        assert_eq!(
            stored.close_outcome(),
            Some(CloseCompletionOutcome::Cancelled)
        );
        assert!(stored.snapshot().is_none());
        let snapshot_columns: (Option<String>, Option<String>) = sqlx::query_as(
            "SELECT inspection_generation, inspection_fingerprint
             FROM close_obligations WHERE attempt_id = ?1",
        )
        .bind("attempt-cancel-before-retirement")
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(snapshot_columns, (None, None));
    }

    #[tokio::test]
    async fn cancel_close_before_retirement_covers_every_pre_retirement_phase() {
        for stop_work_confirmed in [false, true] {
            let db = Database::open_in_memory().await.unwrap();
            create_root(&db, "root").await;
            let attempt_id = format!("attempt-cancel-stop-{stop_work_confirmed}");
            db.begin_close_foundation(&product_id("root"), &transcript_id("root"), &attempt_id)
                .await
                .unwrap();
            if stop_work_confirmed {
                db.confirm_close_stop_work(&attempt_id).await.unwrap();
            }
            let cancelled = db
                .cancel_close_before_retirement(&attempt_id)
                .await
                .unwrap();
            assert_eq!(cancelled.phase(), ClosePhase::Completed);
            assert_eq!(
                cancelled.close_outcome(),
                Some(CloseCompletionOutcome::Cancelled)
            );
        }

        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        let attempt_id = "attempt-cancel-settling-all-phases";
        db.begin_close_foundation(&product_id("root"), &transcript_id("root"), attempt_id)
            .await
            .unwrap();
        db.confirm_close_stop_work(attempt_id).await.unwrap();
        db.begin_close_active_work_settlement(attempt_id)
            .await
            .unwrap();
        assert_eq!(
            db.cancel_close_before_retirement(attempt_id)
                .await
                .unwrap()
                .phase(),
            ClosePhase::CancelRequestedDuringSettlement
        );
        assert_eq!(
            db.cancel_close_before_retirement(attempt_id)
                .await
                .unwrap()
                .phase(),
            ClosePhase::CancelRequestedDuringSettlement
        );

        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        let attempt_id = "attempt-cancel-inspection";
        db.begin_close_foundation(&product_id("root"), &transcript_id("root"), attempt_id)
            .await
            .unwrap();
        db.confirm_close_stop_work(attempt_id).await.unwrap();
        db.begin_close_active_work_settlement(attempt_id)
            .await
            .unwrap();
        assert_eq!(
            db.advance_close_settlement_when_quiescent(attempt_id)
                .await
                .unwrap()
                .phase(),
            ClosePhase::AwaitingRetirementInspection
        );
        assert_eq!(
            db.cancel_close_before_retirement(attempt_id)
                .await
                .unwrap()
                .phase(),
            ClosePhase::Completed
        );
    }

    #[tokio::test]
    async fn loss_confirmation_requires_the_exact_persisted_snapshot() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        let scope = allocate_scope_worktree(&db, "root").await;
        db.begin_close_foundation(
            &product_id("root"),
            &transcript_id("root"),
            "attempt-confirm-loss",
        )
        .await
        .unwrap();
        set_close_phase(
            &db,
            "attempt-confirm-loss",
            ClosePhase::AwaitingRetirementInspection,
        )
        .await;
        let scope_snapshot = CloseRetirementSnapshot::parse("scope-gen", "scope-fp").unwrap();
        db.replace_close_inspection(ReplaceCloseInspectionRequest {
            attempt_id: CloseAttemptId::parse("attempt-confirm-loss").unwrap(),
            scopes: vec![ReplaceCloseInspectionScopeRequest {
                scope,
                snapshot: scope_snapshot,
                losses: vec![CloseLossItem::UntrackedNonIgnoredPath(
                    GitPathIdentity::from_bytes(b"new.txt".to_vec()),
                )],
            }],
        })
        .await
        .unwrap();
        let obligation = db
            .get_close_obligation("attempt-confirm-loss")
            .await
            .unwrap();
        let stale =
            CloseRetirementSnapshot::parse("stale", obligation.snapshot().unwrap().fingerprint())
                .unwrap();
        assert!(matches!(
            db.confirm_close_loss_retirement(
                &CloseAttemptId::parse("attempt-confirm-loss").unwrap(),
                &stale,
            )
            .await,
            Err(DbError::CloseFoundationPrecondition(message)) if message.contains("snapshot is stale")
        ));
        let confirmed = db
            .confirm_close_loss_retirement(
                &CloseAttemptId::parse("attempt-confirm-loss").unwrap(),
                obligation.snapshot().unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(confirmed.phase(), ClosePhase::RetirementRequested);
    }

    #[tokio::test]
    async fn awaiting_retirement_inspection_cannot_skip_loss_confirmation_when_losses_persist() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        create_child(&db, "leaf", "root").await;
        let scope = allocate_scope_worktree(&db, "root").await;
        db.begin_close_foundation(&product_id("root"), &transcript_id("leaf"), "attempt-lossy")
            .await
            .unwrap();
        set_close_phase(
            &db,
            "attempt-lossy",
            ClosePhase::AwaitingRetirementInspection,
        )
        .await;
        let scope_snapshot = CloseRetirementSnapshot::parse("scope-gen", "scope-fp").unwrap();
        insert_scope_inspection(&db, "attempt-lossy", &scope, &scope_snapshot).await;
        let loss = GitPathIdentity::from_bytes(b"still-dirty".to_vec());
        sqlx::query(
            "INSERT INTO close_retirement_losses (
                 attempt_id, scope, generation, category, identity_kind,
                 identity_codec, identity_value
             ) VALUES (?1, ?2, ?3, 'untracked_non_ignored_paths', 'git_path', ?4, ?5)",
        )
        .bind("attempt-lossy")
        .bind(scope.as_str())
        .bind(scope_snapshot.generation())
        .bind(loss.codec())
        .bind(loss.encode())
        .execute(db.pool())
        .await
        .unwrap();
        let aggregate_snapshot = CloseRetirementSnapshot::parse(
            encode_aggregate_snapshot_component([(&scope, scope_snapshot.generation())]),
            encode_aggregate_snapshot_component([(&scope, scope_snapshot.fingerprint())]),
        )
        .unwrap();
        set_obligation_snapshot(&db, "attempt-lossy", &aggregate_snapshot).await;

        assert!(sqlx::query(
            "UPDATE close_obligations
             SET phase = 'retirement_requested'
             WHERE attempt_id = 'attempt-lossy'",
        )
        .execute(db.pool())
        .await
        .is_err());
        let obligation = db.get_close_obligation("attempt-lossy").await.unwrap();
        assert_eq!(obligation.phase(), ClosePhase::AwaitingRetirementInspection);
    }

    #[tokio::test]
    async fn awaiting_retirement_inspection_cannot_enter_loss_confirmation_without_losses() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        create_child(&db, "leaf", "root").await;
        let scope = allocate_scope_worktree(&db, "root").await;
        db.begin_close_foundation(
            &product_id("root"),
            &transcript_id("leaf"),
            "attempt-clean-branch",
        )
        .await
        .unwrap();
        set_close_phase(
            &db,
            "attempt-clean-branch",
            ClosePhase::AwaitingRetirementInspection,
        )
        .await;
        let scope_snapshot = CloseRetirementSnapshot::parse("scope-gen", "scope-fp").unwrap();
        insert_scope_inspection(&db, "attempt-clean-branch", &scope, &scope_snapshot).await;
        let aggregate_snapshot = CloseRetirementSnapshot::parse(
            encode_aggregate_snapshot_component([(&scope, scope_snapshot.generation())]),
            encode_aggregate_snapshot_component([(&scope, scope_snapshot.fingerprint())]),
        )
        .unwrap();
        set_obligation_snapshot(&db, "attempt-clean-branch", &aggregate_snapshot).await;

        assert!(sqlx::query(
            "UPDATE close_obligations
             SET phase = 'awaiting_loss_confirmation'
             WHERE attempt_id = 'attempt-clean-branch'",
        )
        .execute(db.pool())
        .await
        .is_err());
        let obligation = db
            .get_close_obligation("attempt-clean-branch")
            .await
            .unwrap();
        assert_eq!(obligation.phase(), ClosePhase::AwaitingRetirementInspection);
    }

    #[tokio::test]
    async fn latest_close_obligation_uses_persisted_chronology_over_clock_time() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        sqlx::query("DROP TRIGGER close_obligations_require_admission_phase_on_insert")
            .execute(db.pool())
            .await
            .unwrap();
        for (attempt_id, created_at) in [
            ("later-instant", "2025-01-01T00:00:00Z"),
            ("later-text-earlier-instant", "2025-01-01T00:30:00+01:00"),
        ] {
            sqlx::query(
                "INSERT INTO close_obligations (
                     attempt_id, product_conversation_id, phase, created_at, updated_at,
                     completed_at, close_outcome, topology_sealed
                 ) VALUES (
                     ?1, 'root', 'completed', ?2, ?2, ?2, 'cancelled', 1
                 )",
            )
            .bind(attempt_id)
            .bind(created_at)
            .execute(db.pool())
            .await
            .unwrap();
        }

        let latest = db.list_latest_close_obligations().await.unwrap();
        assert_eq!(latest.len(), 1);
        assert_eq!(
            latest[0].attempt_id().as_str(),
            "later-text-earlier-instant"
        );
    }

    #[tokio::test]
    async fn latest_close_obligation_breaks_equal_instant_ties_by_persisted_order() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        sqlx::query("DROP TRIGGER close_obligations_require_admission_phase_on_insert")
            .execute(db.pool())
            .await
            .unwrap();
        for attempt_id in ["lexically-later", "lexically-earlier"] {
            sqlx::query(
                "INSERT INTO close_obligations (
                     attempt_id, product_conversation_id, phase, created_at, updated_at,
                     completed_at, close_outcome, topology_sealed
                 ) VALUES (
                     ?1, 'root', 'completed', '2025-01-01T00:00:00Z',
                     '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z', 'cancelled', 1
                 )",
            )
            .bind(attempt_id)
            .execute(db.pool())
            .await
            .unwrap();
        }
        let latest = db.list_latest_close_obligations().await.unwrap();
        assert_eq!(latest[0].attempt_id().as_str(), "lexically-earlier");
    }

    #[tokio::test]
    async fn latest_close_obligation_preserves_submillisecond_order() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        sqlx::query("DROP TRIGGER close_obligations_require_admission_phase_on_insert")
            .execute(db.pool())
            .await
            .unwrap();
        for (attempt_id, created_at) in [
            ("lexically-later-but-older", "2025-01-01T00:00:00.000100Z"),
            ("lexically-earlier-but-newer", "2025-01-01T00:00:00.000200Z"),
        ] {
            sqlx::query(
                "INSERT INTO close_obligations (
                     attempt_id, product_conversation_id, phase, created_at, updated_at,
                     completed_at, close_outcome, topology_sealed
                 ) VALUES (?1, 'root', 'completed', ?2, ?2, ?2, 'cancelled', 1)",
            )
            .bind(attempt_id)
            .bind(created_at)
            .execute(db.pool())
            .await
            .unwrap();
        }
        let latest = db.list_latest_close_obligations().await.unwrap();
        assert_eq!(
            latest[0].attempt_id().as_str(),
            "lexically-earlier-but-newer"
        );
    }

    #[tokio::test]
    async fn pending_close_obligations_rank_created_at_by_instant() {
        let db = Database::open_in_memory().await.unwrap();
        for root in ["root-a", "root-b"] {
            create_root(&db, root).await;
        }
        for (attempt_id, root, created_at) in [
            ("later-instant", "root-a", "2025-01-01T00:00:00Z"),
            (
                "later-text-earlier-instant",
                "root-b",
                "2025-01-01T00:30:00+01:00",
            ),
        ] {
            sqlx::query(
                "INSERT INTO close_obligations (
                     attempt_id, product_conversation_id, phase, created_at, updated_at
                 ) VALUES (?1, ?2, 'awaiting_blocker_resolution', ?3, ?3)",
            )
            .bind(attempt_id)
            .bind(root)
            .bind(created_at)
            .execute(db.pool())
            .await
            .unwrap();
        }
        let pending = db.list_pending_close_obligations().await.unwrap();
        assert_eq!(pending[0].attempt_id().as_str(), "later-instant");
    }

    #[tokio::test]
    async fn retry_reopens_reinspection_so_resources_admitted_during_repair_are_resealed() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        let scope = allocate_scope_worktree(&db, "root").await;
        let attempt = CloseAttemptId::parse("attempt-reseal-after-repair").unwrap();
        db.begin_close_foundation(
            &product_id("root"),
            &transcript_id("root"),
            attempt.as_str(),
        )
        .await
        .unwrap();
        set_close_phase(&db, attempt.as_str(), ClosePhase::RetirementRequested).await;
        let snapshot = current_test_snapshot(&db, attempt.as_str()).await;
        let resources = db
            .capture_close_retirement_inventory(CaptureCloseRetirementInventoryRequest {
                attempt_id: attempt.clone(),
                snapshot: snapshot.clone(),
                scopes: vec![CaptureCloseRetirementInventoryScopeRequest {
                    scope: scope.clone(),
                    inventory: CloseOwnedResourceInventory {
                        worktree: Some(current_test_worktree(&db, &scope).await),
                        work_scopes: std::collections::BTreeSet::default(),
                        bash_process_groups: std::collections::BTreeSet::default(),
                        tmux_servers: std::collections::BTreeSet::default(),
                        pty_sessions: std::collections::BTreeSet::default(),
                        browser_sessions: std::collections::BTreeSet::default(),
                        equivalent_live_resources: std::collections::BTreeSet::default(),
                    },
                }],
            })
            .await
            .unwrap();
        let worktree = resources
            .into_iter()
            .find(|resource| resource.resource.kind() == RetiredResourceKind::Worktree)
            .unwrap();
        db.record_close_retirement_evidence(RecordCloseRetirementEvidenceRequest {
            attempt_id: attempt.clone(),
            snapshot: snapshot.clone(),
            scope,
            resource: worktree.resource,
            outcome: RetirementOutcome::Residual {
                residual_reason: RetirementFailureReason::RemovalFailed,
            },
            detail: Some("repair remains required".to_string()),
        })
        .await
        .unwrap();

        let retried = db.retry_close_retirement(&attempt).await.unwrap();
        assert_eq!(retried.phase(), ClosePhase::AwaitingRetirementInspection);
        assert_eq!(retried.snapshot(), Some(&snapshot));
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn dispatched_absence_retry_resumes_retirement_with_retained_snapshot() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        let scope = allocate_scope_worktree(&db, "root").await;
        let attempt = CloseAttemptId::parse("attempt-dispatched-absence-retry").unwrap();
        db.begin_close_foundation(
            &product_id("root"),
            &transcript_id("root"),
            attempt.as_str(),
        )
        .await
        .unwrap();
        set_close_phase(&db, attempt.as_str(), ClosePhase::RetirementRequested).await;
        let snapshot = current_test_snapshot(&db, attempt.as_str()).await;
        let resources = db
            .capture_close_retirement_inventory(CaptureCloseRetirementInventoryRequest {
                attempt_id: attempt.clone(),
                snapshot: snapshot.clone(),
                scopes: vec![CaptureCloseRetirementInventoryScopeRequest {
                    scope: scope.clone(),
                    inventory: CloseOwnedResourceInventory {
                        worktree: Some(current_test_worktree(&db, &scope).await),
                        work_scopes: std::collections::BTreeSet::default(),
                        bash_process_groups: std::collections::BTreeSet::default(),
                        tmux_servers: std::collections::BTreeSet::default(),
                        pty_sessions: std::collections::BTreeSet::default(),
                        browser_sessions: std::collections::BTreeSet::default(),
                        equivalent_live_resources: std::collections::BTreeSet::default(),
                    },
                }],
            })
            .await
            .unwrap();
        let worktree = resources
            .into_iter()
            .find(|resource| resource.resource.kind() == RetiredResourceKind::Worktree)
            .unwrap();
        db.record_close_retirement_dispatch(RecordCloseRetirementDispatchRequest {
            attempt_id: attempt.clone(),
            snapshot: snapshot.clone(),
            scope: scope.clone(),
            resource: worktree.resource.clone(),
        })
        .await
        .unwrap();
        let cleanup_dir = std::path::PathBuf::from("/tmp/dispatched-absence-admin");
        db.record_close_worktree_cleanup_plan(RecordCloseWorktreeCleanupPlanRequest {
            attempt_id: attempt.clone(),
            snapshot: snapshot.clone(),
            scope: scope.clone(),
            resource: worktree.resource.clone(),
            administrative_dir: cleanup_dir.clone(),
            administrative_dir_incarnation: "admin-cleanup-v1".to_string(),
        })
        .await
        .unwrap();
        db.record_close_retirement_evidence(RecordCloseRetirementEvidenceRequest {
            attempt_id: attempt.clone(),
            snapshot: snapshot.clone(),
            scope: scope.clone(),
            resource: worktree.resource.clone(),
            outcome: RetirementOutcome::Residual {
                residual_reason: RetirementFailureReason::RemovalFailed,
            },
            detail: Some("later cleanup requires repair".to_string()),
        })
        .await
        .unwrap();
        db.retry_close_retirement(&attempt).await.unwrap();

        let replacement = db
            .resume_close_retirement_after_dispatched_absence(
                &attempt,
                &snapshot,
                "server_git_status_v2_retry_dispatched_absence",
            )
            .await
            .unwrap();
        let resumed = db.get_close_obligation(attempt.as_str()).await.unwrap();
        assert_eq!(resumed.phase(), ClosePhase::RetirementRequested);
        assert_eq!(resumed.snapshot(), Some(&replacement));
        assert!(db
            .close_retirement_resource_was_dispatched(
                &attempt,
                &scope,
                &replacement,
                &worktree.resource,
            )
            .await
            .unwrap());
        assert_eq!(
            db.close_worktree_cleanup_plan(&attempt, &scope, &replacement, &worktree.resource,)
                .await
                .unwrap(),
            Some(CloseWorktreeCleanupPlan {
                administrative_dir: cleanup_dir,
                administrative_dir_incarnation: "admin-cleanup-v1".to_string(),
                final_tombstone: None,
            }),
        );
        assert!(db
            .close_retirement_inventory_is_complete(attempt.as_str())
            .await
            .unwrap());
    }

    #[test]
    fn retirement_evidence_request_requires_typed_resource_identity() {
        assert!(RetiredResourceIdentity::parse(
            RetiredResourceKind::BrowserSession,
            LossItemIdentity::GitPath(GitPathIdentity::from_bytes(b"browser".to_vec())),
        )
        .is_err());
    }

    #[tokio::test]
    async fn retirement_evidence_rejects_untargeted_scope() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        db.begin_close_foundation(&product_id("root"), &transcript_id("root"), "attempt-1")
            .await
            .unwrap();
        let other_scope = WorkScopeId::parse("close-scope-other").unwrap();
        sqlx::query(
            "INSERT INTO work_scopes (
                id, authority_kind, lifecycle, environment_kind, cwd,
                worktree_path, branch_name, base_branch, created_at, updated_at,
                worktree_id, worktree_fingerprint
             ) VALUES (
                ?1, 'work', 'active', 'allocated_worktree', '/tmp', '/tmp/other',
                'branch', 'main', ?2, ?2, lower(hex(randomblob(16))), lower(hex(randomblob(32)))
             )",
        )
        .bind(other_scope.as_str())
        .bind(Utc::now().to_rfc3339())
        .execute(db.pool())
        .await
        .unwrap();

        let other_worktree = current_test_worktree(&db, &other_scope).await;
        let err = db
            .record_close_retirement_evidence(RecordCloseRetirementEvidenceRequest {
                attempt_id: CloseAttemptId::parse("attempt-1").unwrap(),
                snapshot: CloseRetirementSnapshot::parse("not-inspected", "not-inspected").unwrap(),
                scope: other_scope,
                resource: RetiredResourceIdentity::parse(
                    RetiredResourceKind::Worktree,
                    LossItemIdentity::Worktree(other_worktree),
                )
                .unwrap(),
                outcome: RetirementOutcome::Retired,
                detail: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, DbError::CloseFoundationPrecondition(_)));
    }
}
