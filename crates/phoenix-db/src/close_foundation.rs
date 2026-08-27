#![allow(clippy::needless_pass_by_value)]

use chrono::{DateTime, Utc};
use phoenix_core::domain::close::{
    AbsenceBasis, CapturedConversationStateKind, CapturedWorktreeIdentity, CloseAttemptId,
    CloseAttemptMember, CloseAttemptScope, CloseCompletionOutcome, CloseExpectedRetirementResource,
    CloseInspection, CloseInspectionLoss, CloseLossItem, CloseMemberRole, CloseObligation,
    CloseOwnedResourceInventory, ClosePhase, CloseRetiredResource, CloseRetirementSnapshot,
    CloseRetirementTarget, GitOidIdentity, GitPathIdentity, LossCategory, LossItemIdentity,
    OpaqueIdentity, ProductConversationId, RetiredResourceIdentity, RetiredResourceKind,
    RetirementFailureReason, RetirementOutcome, TranscriptConversationId, WorktreeIdentity,
};
use phoenix_core::work_scope::{RuntimeRole, WorkScopeId};
use sqlx::sqlite::SqliteRow;
use sqlx::{Connection, Row, Sqlite, Transaction};
use std::fmt::Write as _;

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
pub enum ProductConversationAdmission {
    Accepted {
        product_conversation_id: ProductConversationId,
    },
    Refused(CloseAdmissionFence),
    History(ProductConversationId),
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
         WHERE product_conversation_id = ?1 AND phase <> 'completed'",
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

#[derive(Debug, Clone)]
pub struct RecordCloseRetirementDispatchRequest {
    pub attempt_id: CloseAttemptId,
    pub scope: WorkScopeId,
    pub snapshot: CloseRetirementSnapshot,
    pub resource: RetiredResourceIdentity,
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

    if root.archived {
        return Err(close_precondition("root conversation is archived"));
    }
    if topology
        .members
        .iter()
        .any(|member| member.conversation.archived)
    {
        return Err(close_precondition(
            "Close topology contains an archived conversation",
        ));
    }
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
        return Err(close_precondition(format!(
            "addressed conversation {} is not latest {}",
            addressed_id, latest.id
        )));
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

    #[allow(clippy::too_many_lines)]
    pub async fn begin_close_foundation(
        &self,
        product_conversation_id: &ProductConversationId,
        attempt_id: &str,
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

            tx.commit().await?;
            return Ok(obligation);
        }

        let topology = read_topology_tx(&mut tx, product_conversation_id)
            .await?
            .ok_or_else(|| DbError::CloseFoundationNotFound(product_conversation_id.to_string()))?;
        validate_begin_preconditions(&topology, &topology.latest.id)?;

        if let Some(row) = sqlx::query(
            "SELECT attempt_id, product_conversation_id, phase, inspection_generation,
                    inspection_fingerprint, created_at, updated_at, completed_at, close_outcome
             FROM close_obligations
             WHERE product_conversation_id = ?1 AND phase <> 'completed'",
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

        let now = Utc::now().to_rfc3339();
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
             JOIN close_attempt_members member
               ON member.conversation_id = turn.conversation_id
             WHERE member.attempt_id = ?1
               AND member.member_role IN ('latest', 'root_latest')
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
        let active_members: i64 = sqlx::query_scalar(
            "SELECT COUNT(*)
             FROM close_attempt_members member
             JOIN conversations participant ON participant.id = member.conversation_id
             WHERE member.attempt_id = ?1
               AND (
                 EXISTS (
                   SELECT 1 FROM durable_turns turn
                   WHERE member.member_role IN ('latest', 'root_latest')
                     AND turn.conversation_id = participant.id
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
                       creation.status IN ('accepted', 'claimed', 'retry_scheduled', 'cancelling', 'deletion_pending')
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
        .fetch_one(&mut *tx)
        .await?;
        if active_members != 0 {
            return Err(close_precondition(format!(
                "attempt {attempt_id} still has {active_members} active durable member obligation(s)"
            )));
        }
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
            "SELECT member.conversation_id
             FROM close_attempt_members member
             WHERE member.attempt_id = ?1
             ORDER BY member.conversation_id",
        )
        .bind(attempt_id)
        .fetch_all(&self.pool)
        .await?;
        if rows.is_empty() {
            return Err(DbError::CloseFoundationNotFound(attempt_id.to_string()));
        }
        Ok(rows)
    }

    pub async fn wake_delivery_requires_close_settlement_recheck(
        &self,
        workflow_id: phoenix_workflow::WorkflowId,
    ) -> DbResult<bool> {
        let phase: Option<String> = sqlx::query_scalar(
            "SELECT obligation.phase
             FROM wake_bindings binding
             JOIN close_attempt_members member
               ON member.conversation_id = binding.conversation_id
             JOIN close_obligations obligation ON obligation.attempt_id = member.attempt_id
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

    pub async fn cancel_close_settlement_wakes(&self, attempt_id: &str) -> DbResult<usize> {
        let wake_repo = crate::workflow::wake::WakeRepository::new(self.pool.clone());
        let rows: Vec<(i64, String, String)> = sqlx::query_as(
            "SELECT binding.workflow_id, binding.conversation_id, binding.contract_id
             FROM wake_bindings binding
             JOIN workflows workflow ON workflow.workflow_id = binding.workflow_id
             JOIN close_attempt_members member
               ON member.conversation_id = binding.conversation_id
             WHERE member.attempt_id = ?1
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
             WHERE product_conversation_id = ?1 AND phase <> 'completed'",
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

    #[allow(clippy::too_many_lines)]
    pub async fn replace_close_inspection(
        &self,
        request: ReplaceCloseInspectionRequest,
    ) -> DbResult<()> {
        let mut ordered_scopes = request.scopes.iter().collect::<Vec<_>>();
        ordered_scopes.sort_by(|left, right| left.scope.cmp(&right.scope));
        let aggregate_snapshot = CloseRetirementSnapshot::parse(
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
                    route_unresolved_worktree_to_repair(&mut tx, &request.attempt_id).await?;
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
             ORDER BY scope",
        )
        .bind(request.attempt_id.as_str())
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
                 ORDER BY scope, resource_kind, identity_kind,
                     identity_codec, identity_value",
                )
                .bind(request.attempt_id.as_str())
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
                    route_unresolved_worktree_to_repair(&mut tx, &request.attempt_id).await?;
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

    /// Routes an exact Close attempt to repair without fabricating a per-resource receipt.
    ///
    /// # Errors
    /// Returns [`DbError`] when the attempt is not in a retirement phase or persistence fails.
    pub async fn route_close_attempt_to_repair(&self, attempt_id: &CloseAttemptId) -> DbResult<()> {
        let mut tx = self.pool.begin().await?;
        route_unresolved_worktree_to_repair(&mut tx, attempt_id).await?;
        tx.commit().await?;
        Ok(())
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

async fn route_unresolved_worktree_to_repair(
    tx: &mut Transaction<'_, Sqlite>,
    attempt_id: &CloseAttemptId,
) -> DbResult<()> {
    let phase: String =
        sqlx::query_scalar("SELECT phase FROM close_obligations WHERE attempt_id = ?1")
            .bind(attempt_id.as_str())
            .fetch_optional(&mut **tx)
            .await?
            .ok_or_else(|| DbError::CloseFoundationNotFound(attempt_id.as_str().to_string()))?;
    match phase.as_str() {
        "awaiting_retirement_inspection" | "retirement_requested" => {
            sqlx::query(
                "UPDATE close_obligations
                 SET phase = 'needs_repair',
                     inspection_generation = COALESCE(
                         inspection_generation,
                         'no-worktree'
                     ),
                     inspection_fingerprint = COALESCE(
                         inspection_fingerprint,
                         'no-worktree'
                     ),
                     updated_at = ?2
                 WHERE attempt_id = ?1
                   AND phase IN ('awaiting_retirement_inspection', 'retirement_requested')",
            )
            .bind(attempt_id.as_str())
            .bind(Utc::now().to_rfc3339())
            .execute(&mut **tx)
            .await?;
        }
        "needs_repair" => {}
        _ => {
            return Err(close_precondition(format!(
                "attempt {attempt_id} unresolved worktree repair requires awaiting_retirement_inspection, retirement_requested, or needs_repair"
            )))
        }
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
                 (SELECT COUNT(*) FROM close_retirement_inventories WHERE attempt_id = ?1) AS inventory_count,
                 (SELECT COUNT(*) FROM close_retirement_inventories
                  WHERE attempt_id = ?1 AND sealed = 0) AS unsealed_count
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
        if !matches!(
            current.phase(),
            ClosePhase::RetirementRequested | ClosePhase::NeedsRepair
        ) || current.snapshot() != Some(&request.snapshot)
        {
            return Err(close_precondition(format!(
                "attempt {} dispatch lacks the exact active retirement authority",
                request.attempt_id
            )));
        }
        let identity = request.resource.identity();
        let inserted = sqlx::query(
            "INSERT OR IGNORE INTO close_retirement_resource_dispatches (
                 attempt_id, scope, inspection_generation, inspection_fingerprint,
                 resource_kind, identity_kind, identity_codec, identity_value, dispatched_at
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
        .bind(Utc::now().to_rfc3339())
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

    /// Reopens one repairable Close attempt for exact retirement retry.
    ///
    /// # Errors
    /// Returns [`DbError`] when the attempt is absent, is not in repair, or persistence fails.
    pub async fn retry_close_retirement(
        &self,
        attempt_id: &CloseAttemptId,
    ) -> DbResult<CloseObligation> {
        let mut tx = self.pool.begin().await?;
        let obligation = close_obligation_for_update(&mut tx, attempt_id.as_str()).await?;
        if obligation.phase() != ClosePhase::NeedsRepair {
            return Err(close_precondition(format!(
                "attempt {attempt_id} retry requires needs_repair"
            )));
        }
        set_close_phase_tx(
            &mut tx,
            attempt_id.as_str(),
            ClosePhase::RetirementRequested,
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
                .begin_close_foundation(&product_id("race-root"), "loser")
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
            .begin_close_foundation(&conversation.product_conversation_id, "fresh-attempt")
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
    async fn active_work_settlement_advances_only_once_when_captured_members_are_quiescent() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        create_child(&db, "latest", "root").await;
        db.begin_close_foundation(&product_id("root"), "attempt-settlement")
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
    async fn direct_turn_settlement_captures_only_latest_authority_and_receipts_exact_release() {
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
        db.begin_close_foundation(&product_id("root"), "attempt-exact-receipt")
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
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].conversation_id, "latest");
        assert_eq!(targets[0].turn_id, 2);
        assert_eq!(targets[0].expected_generation, 0);
        sqlx::query(
            "UPDATE durable_turns
             SET generation = generation + 1, terminal_kind = 'Completed', owns_conversation = 0
             WHERE turn_id = 2",
        )
        .execute(db.pool())
        .await
        .unwrap();
        assert!(db
            .record_close_direct_turn_settlement_if_released("attempt-exact-receipt", &targets[0])
            .await
            .unwrap());
        assert!(db
            .record_close_direct_turn_settlement_if_released("attempt-exact-receipt", &targets[0])
            .await
            .unwrap());
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
        db.begin_close_foundation(&product_id("root"), "attempt-confirm")
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
        db.begin_close_foundation(&product_conversation_id, "attempt-participant")
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
        db.begin_close_foundation(&product_id("root"), "attempt-busy")
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
        db.begin_close_foundation(&product_id("root"), "attempt-creation")
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
        db.begin_close_foundation(&product_id("root"), "attempt-cancel-settlement")
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
        db.begin_close_foundation(&product_id("root"), "attempt-cancel-busy")
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
        db.begin_close_foundation(&product_id("root"), "admission-fence")
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
        db.begin_close_foundation(&product_id("root"), "attempt-owned")
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
            .begin_close_foundation(&product_id("root"), "attempt-1")
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
        db.begin_close_foundation(&product_id("a|b"), "attempt-delimiter")
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

        db.begin_close_foundation(&product_id("root"), "attempt-1")
            .await
            .unwrap();
        let members = db.list_close_attempt_members("attempt-1").await.unwrap();
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].conversation_id.as_str(), "root");
        assert_eq!(members[0].role, CloseMemberRole::RootLatest);
    }

    #[tokio::test]
    async fn unresolved_worktree_routes_from_inspection_directly_to_repair() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        allocate_scope_worktree(&db, "root").await;
        db.begin_close_foundation(&product_id("root"), "attempt-inspection-repair")
            .await
            .unwrap();
        set_close_phase(
            &db,
            "attempt-inspection-repair",
            ClosePhase::AwaitingRetirementInspection,
        )
        .await;

        db.route_close_attempt_to_repair(
            &CloseAttemptId::parse("attempt-inspection-repair").unwrap(),
        )
        .await
        .unwrap();

        assert_eq!(
            db.get_close_obligation("attempt-inspection-repair")
                .await
                .unwrap()
                .phase(),
            ClosePhase::NeedsRepair
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

        db.begin_close_foundation(&product_id("root"), "attempt-unresolved")
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
            .begin_close_foundation(&product_id("root"), "attempt-1")
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
            .begin_close_foundation(&product_id("root"), "attempt-1")
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
                .begin_close_foundation(&product_id(id), &format!("attempt-{id}"))
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
            db.begin_close_foundation(&product_id("root"), "attempt-1")
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
            .begin_close_foundation(&product_id("root"), "attempt-1")
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
                },
            ),
        ];
        for (id, state) in states {
            create_root(&db, id).await;
            set_state(&db, id, state).await;
            let obligation = db
                .begin_close_foundation(&product_id(id), &format!("attempt-{id}"))
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
            db.begin_close_foundation(&product_id("approval"), "attempt-approval")
                .await
                .unwrap_err(),
            DbError::CloseFoundationPrecondition(_)
        ));

        create_root(&db, "awaiting").await;
        set_state(&db, "awaiting", awaiting_continuation_state()).await;
        assert!(matches!(
            db.begin_close_foundation(&product_id("awaiting"), "attempt-awaiting")
                .await
                .unwrap_err(),
            DbError::CloseFoundationPrecondition(_)
        ));
    }

    #[tokio::test]
    async fn idempotent_same_attempt_survives_latest_mutation_and_conflict_different_attempt() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        create_child(&db, "latest", "root").await;

        let first = db
            .begin_close_foundation(&product_id("root"), "attempt-1")
            .await
            .unwrap();

        set_state(&db, "latest", ConvState::LlmRequesting { attempt: 2 }).await;

        let second = db
            .begin_close_foundation(&product_id("root"), "attempt-1")
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
            .begin_close_foundation(&product_id("root"), "attempt-1")
            .await
            .unwrap();
        assert_eq!(first, third);

        let err = db
            .begin_close_foundation(&product_id("root"), "attempt-2")
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
            .begin_close_foundation(&product_id("root"), "attempt-partial")
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
            .begin_close_foundation(&product_id("root"), "attempt-nul")
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

        db.begin_close_foundation(&product_id("root"), "attempt-1")
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
        db.begin_close_foundation(&product_id("root"), "attempt-1")
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
    async fn archived_non_root_member_is_rejected() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        create_child(&db, "latest", "root").await;
        set_archived(&db, "latest", true).await;
        let error = db
            .begin_close_foundation(&product_id("root"), "attempt-archived-member")
            .await
            .unwrap_err();
        assert!(matches!(error, DbError::CloseFoundationPrecondition(_)));
    }

    #[tokio::test]
    async fn archived_root_non_user_and_non_user_initiated_are_rejected() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "archived").await;
        set_archived(&db, "archived", true).await;
        assert!(matches!(
            db.begin_close_foundation(&product_id("archived"), "attempt-a")
                .await
                .unwrap_err(),
            DbError::CloseFoundationPrecondition(_)
        ));

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
            .begin_close_foundation(&parent.product_conversation_id, "attempt-b")
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
            db.begin_close_foundation(&product_id("not-user-init"), "attempt-c")
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
        db.begin_close_foundation(&product_id("root"), "attempt-1")
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
        db.begin_close_foundation(&product_id("root"), "attempt-concurrent-inspection")
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
        db.begin_close_foundation(&product_id("root"), "attempt-1")
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

    #[tokio::test]
    async fn inspection_reentry_invalidates_prior_snapshot_and_rows() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        let scope = allocate_scope_worktree(&db, "root").await;
        db.begin_close_foundation(&product_id("root"), "attempt-reentry")
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
        db.begin_close_foundation(&product_id("root"), "attempt-changed-confirmation")
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
        db.begin_close_foundation(&product_id("root"), "attempt-clean")
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
        db.begin_close_foundation(&product_id("root"), "attempt-empty")
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

        db.begin_close_foundation(&product_id("root"), "attempt-1")
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

        db.begin_close_foundation(&product_id("root"), "attempt-1")
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

        db.begin_close_foundation(&product_id("root"), "attempt-1")
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
        db.begin_close_foundation(&product_id("root"), "attempt-1")
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
        db.begin_close_foundation(&product_id("root"), "attempt-terminal-owner")
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
        db.begin_close_foundation(&product_id("root"), "attempt-concurrent-inventory")
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
        db.begin_close_foundation(&product_id("root"), "attempt-zero")
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
        db.begin_close_foundation(&product_id("root"), "attempt-1")
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
        db2.begin_close_foundation(&product_id("root-2"), "attempt-2")
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
        db.begin_close_foundation(&product_id("root"), "attempt-partial")
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
        db.begin_close_foundation(&product_id("root"), "attempt-incomplete")
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
        db.begin_close_foundation(&product_id("root"), "attempt-shared")
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
        db.begin_close_foundation(&product_id("root"), "attempt-cancelled")
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
        db.begin_close_foundation(&product_id("root"), "attempt-1")
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
        db.begin_close_foundation(&product_id("root"), "attempt-1")
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
        db.begin_close_foundation(&product_id("root"), "attempt-concurrent-evidence")
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
        db.begin_close_foundation(&product_id("root"), "attempt-1")
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
        db.begin_close_foundation(&product_id("root"), "attempt-1")
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
        db.begin_close_foundation(&product_id("root"), "attempt-1")
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
    async fn loss_confirmation_requires_the_exact_persisted_snapshot() {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        let scope = allocate_scope_worktree(&db, "root").await;
        db.begin_close_foundation(&product_id("root"), "attempt-confirm-loss")
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
        db.begin_close_foundation(&product_id("root"), "attempt-lossy")
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
        db.begin_close_foundation(&product_id("root"), "attempt-clean-branch")
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
        db.begin_close_foundation(&product_id("root"), "attempt-1")
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
