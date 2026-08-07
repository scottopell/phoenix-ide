#![allow(clippy::wildcard_enum_match_arm)]

use super::handlers::{
    broadcast_conversation_hard_deleted, run_resource_cleanup_cascade, AppError,
};
use super::types::{ConflictErrorResponse, SuccessResponse};
use super::AppState;
use crate::db::{Conversation, DbError};
use crate::git_ops::{inspect_worktree_loss_inventory, WorktreeInspectionScope};
use crate::runtime::SseEvent;
use crate::state_machine::ConvState;
use axum::{
    extract::{Path, State},
    Json,
};
use phoenix_core::domain::close::{
    CloseInspection, CloseInspectionLoss, CloseObligation, ClosePhase, CloseRetiredResource,
    LossCategory, ProductConversationId, RetiredResourceKind, RetirementFailureReason,
    RetirementOutcome,
};
use phoenix_db::{
    BeginCloseOutcome, ConfirmInspectionOutcome, DeleteHistoryAggregateOutcome, LossRowInput,
    ScopeInspectionInput,
};
use phoenix_tools::work_scope_inventory::assemble_inventory;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path as FsPath, PathBuf};

const NO_WORKTREE_AGGREGATE: &str = "no-worktree";

#[derive(Debug, Clone, PartialEq, Eq)]
struct CloseInspectionAggregate {
    generation: String,
    fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CloseInspectionComputation {
    aggregate: CloseInspectionAggregate,
    inspections: Vec<ScopeInspectionInput>,
    has_losses: bool,
}

#[derive(Debug, Clone)]
struct ScopeRepresentative {
    scope_id: phoenix_core::work_scope::WorkScopeId,
    conversation: Conversation,
}

#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools)]
struct ScopeInventorySnapshot {
    worktree_path: Option<String>,
    worktree_exists: bool,
    bash_handles: Vec<phoenix_core::domain::work_scope_inventory::BashHandleInventory>,
    tmux_live: bool,
    browser_live: bool,
    pty_live: bool,
}

#[derive(Debug, Clone)]
struct CleanupRecord {
    scope: Option<phoenix_core::work_scope::WorkScopeId>,
    pre: Option<ScopeInventorySnapshot>,
    post: Option<ScopeInventorySnapshot>,
    externally_owned: bool,
}

#[derive(Debug, Serialize)]
struct AggregateFingerprintCanonical<'a> {
    scopes: &'a [AggregateFingerprintScopeCanonical],
}

#[derive(Debug, Serialize)]
struct AggregateFingerprintScopeCanonical {
    scope: String,
    generation: Option<String>,
    fingerprint: Option<String>,
    losses: Vec<AggregateFingerprintLossCanonical>,
}

#[derive(Debug, Serialize)]
struct AggregateFingerprintLossCanonical {
    category: &'static str,
    item_identity: String,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct CloseRequestBody {
    #[serde(default)]
    pub attempt_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ConfirmLossBody {
    pub inspection_generation: String,
    pub inspection_fingerprint: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct CloseStatusResponse {
    pub attempt: CloseAttemptView,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct CloseAttemptView {
    pub attempt_id: String,
    pub conversation_id: String,
    pub root_conversation_id: String,
    pub latest_conversation_id: String,
    pub phase: ClosePhase,
    pub inspection_generation: Option<String>,
    pub inspection_fingerprint: Option<String>,
    pub inspections: Vec<CloseInspection>,
    pub losses: Vec<CloseInspectionLoss>,
    pub retirement_evidence: Vec<CloseRetiredResource>,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
}

pub(crate) async fn request_close(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Option<Json<CloseRequestBody>>,
) -> Result<Json<CloseStatusResponse>, AppError> {
    let admission = state.runtime.conversation_admission(&id).await;
    let _guard = admission.lock().await;
    let request = body
        .map(|Json(body)| body)
        .unwrap_or(CloseRequestBody { attempt_id: None });
    let attempt_id = request
        .attempt_id
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let conv_id = parse_product_conv_id(&id)?;
    let obligation = match state.db.begin_close(&conv_id, &attempt_id).await {
        Ok(
            BeginCloseOutcome::Started(obligation) | BeginCloseOutcome::AlreadyStarted(obligation),
        ) => obligation,
        Err(error) => return Err(map_close_db_error(error)),
    };
    let after = match current_busy_member(&state, &obligation).await? {
        Some(_) => {
            transition_if_current(
                &state,
                &obligation.attempt_id,
                ClosePhase::AwaitingBlockerResolution,
                ClosePhase::AwaitingStopWorkConfirmation,
            )
            .await?
        }
        None => settle_attempt(&state, obligation.attempt_id.clone(), false).await?,
    };
    Ok(Json(load_status(&state, &after).await?))
}

pub(crate) async fn get_close_status(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<CloseStatusResponse>, AppError> {
    let obligation = latest_attempt_for_conversation(&state, &id).await?;
    Ok(Json(load_status(&state, &obligation).await?))
}

pub(crate) async fn confirm_close_stop_work(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<CloseStatusResponse>, AppError> {
    let admission = state.runtime.conversation_admission(&id).await;
    let _guard = admission.lock().await;
    let obligation = latest_attempt_for_conversation(&state, &id).await?;
    let after = match obligation.phase {
        ClosePhase::AwaitingStopWorkConfirmation => {
            settle_attempt(&state, obligation.attempt_id.clone(), true).await?
        }
        _ => obligation,
    };
    Ok(Json(load_status(&state, &after).await?))
}

pub(crate) async fn cancel_close_attempt(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<CloseStatusResponse>, AppError> {
    let admission = state.runtime.conversation_admission(&id).await;
    let _guard = admission.lock().await;
    let obligation = latest_attempt_for_conversation(&state, &id).await?;
    let after = cancel_attempt(&state, obligation).await?;
    Ok(Json(load_status(&state, &after).await?))
}

pub(crate) async fn confirm_close_loss(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<ConfirmLossBody>,
) -> Result<Json<CloseStatusResponse>, AppError> {
    let admission = state.runtime.conversation_admission(&id).await;
    let _guard = admission.lock().await;
    let obligation = latest_attempt_for_conversation(&state, &id).await?;
    if obligation.phase != ClosePhase::AwaitingLossConfirmation {
        return Err(phase_conflict(&obligation, "awaiting_loss_confirmation"));
    }
    let topology = state
        .db
        .product_conversation_topology(&obligation.product_conversation_id)
        .await
        .map_err(map_close_db_error)?;
    let conversations = load_topology_conversations(&state, &topology).await?;
    let fresh = compute_close_inspection(&conversations)?;
    let confirmed = state
        .db
        .confirm_inspection(
            &obligation.attempt_id,
            &body.inspection_generation,
            &body.inspection_fingerprint,
            &fresh.aggregate.generation,
            &fresh.aggregate.fingerprint,
        )
        .await
        .map_err(map_close_db_error)?;
    let after = match confirmed {
        ConfirmInspectionOutcome::Confirmed(obligation) => {
            drive_retirement_then_maybe_finalize(&state, obligation).await?
        }
        ConfirmInspectionOutcome::Mismatch { obligation } => obligation,
    };
    Ok(Json(load_status(&state, &after).await?))
}

pub(crate) async fn retry_close_attempt(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<CloseStatusResponse>, AppError> {
    let admission = state.runtime.conversation_admission(&id).await;
    let _guard = admission.lock().await;
    let obligation = latest_attempt_for_conversation(&state, &id).await?;
    let after = drive_attempt_once(&state, obligation).await?;
    Ok(Json(load_status(&state, &after).await?))
}

pub(crate) async fn finalize_close_history(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<CloseStatusResponse>, AppError> {
    let admission = state.runtime.conversation_admission(&id).await;
    let _guard = admission.lock().await;
    let obligation = latest_attempt_for_conversation(&state, &id).await?;
    let after = finalize_history_message(&state, &obligation).await?;
    Ok(Json(load_status(&state, &after).await?))
}

pub(crate) async fn delete_closed_history(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<SuccessResponse>, AppError> {
    let admission = state.runtime.conversation_admission(&id).await;
    let _guard = admission.lock().await;
    let conversation_id = parse_product_conv_id(&id)?;
    match state.db.delete_history_aggregate(&conversation_id).await {
        Ok(DeleteHistoryAggregateOutcome::Deleted { topology }) => {
            for member in &topology.member_conversation_ids {
                broadcast_conversation_hard_deleted(&state, member.as_str()).await;
            }
            Ok(Json(SuccessResponse { success: true }))
        }
        Ok(DeleteHistoryAggregateOutcome::AlreadyDeleted { .. }) => {
            Ok(Json(SuccessResponse { success: true }))
        }
        Err(error) => Err(map_close_db_error(error)),
    }
}

pub(crate) async fn resume_pending_close_attempts(state: &AppState) -> Result<usize, String> {
    let pending = state
        .db
        .list_pending_close_restart_attempts()
        .await
        .map_err(|error| error.to_string())?;
    let mut resumed = 0;
    for obligation in pending {
        if phase_is_machine_owned(obligation.phase) {
            let conversation_id = obligation.product_conversation_id.as_str().to_string();
            let admission = state.runtime.conversation_admission(&conversation_id).await;
            let _guard = admission.lock().await;
            let fresh = match state.db.get_close_obligation(&obligation.attempt_id).await {
                Ok(Some(fresh)) => fresh,
                Ok(None) => {
                    tracing::warn!(attempt_id = %obligation.attempt_id, "pending close obligation disappeared during resumption");
                    continue;
                }
                Err(error) => {
                    tracing::warn!(attempt_id = %obligation.attempt_id, %error, "failed to reload pending close obligation");
                    continue;
                }
            };
            match drive_attempt_once(state, fresh).await {
                Ok(_) => resumed += 1,
                Err(error) => {
                    tracing::warn!(
                        attempt_id = %obligation.attempt_id,
                        conversation_id = %conversation_id,
                        error = %format!("{error:?}"),
                        "failed to resume pending close attempt"
                    );
                }
            }
        }
    }
    Ok(resumed)
}

fn phase_is_machine_owned(phase: ClosePhase) -> bool {
    matches!(
        phase,
        ClosePhase::AwaitingBlockerResolution
            | ClosePhase::SettlingActiveWork
            | ClosePhase::AwaitingRetirementInspection
            | ClosePhase::RetirementRequested
    )
}

async fn drive_attempt_once(
    state: &AppState,
    obligation: CloseObligation,
) -> Result<CloseObligation, AppError> {
    match obligation.phase {
        ClosePhase::AwaitingBlockerResolution => {
            match current_busy_member(state, &obligation).await? {
                Some(_) => {
                    transition_if_current(
                        state,
                        &obligation.attempt_id,
                        ClosePhase::AwaitingBlockerResolution,
                        ClosePhase::AwaitingStopWorkConfirmation,
                    )
                    .await
                }
                None => settle_attempt(state, obligation.attempt_id, false).await,
            }
        }
        ClosePhase::SettlingActiveWork => settle_attempt(state, obligation.attempt_id, true).await,
        ClosePhase::AwaitingRetirementInspection => {
            inspect_then_maybe_retire(state, obligation).await
        }
        ClosePhase::RetirementRequested => {
            drive_retirement_then_maybe_finalize(state, obligation).await
        }
        ClosePhase::NeedsRepair => {
            let retried = transition_if_current(
                state,
                &obligation.attempt_id,
                ClosePhase::NeedsRepair,
                ClosePhase::RetirementRequested,
            )
            .await?;
            drive_retirement_then_maybe_finalize(state, retried).await
        }
        _ => Ok(obligation),
    }
}

async fn cancel_attempt(
    state: &AppState,
    obligation: CloseObligation,
) -> Result<CloseObligation, AppError> {
    let next = match obligation.phase {
        ClosePhase::AwaitingStopWorkConfirmation => {
            transition_if_current(
                state,
                &obligation.attempt_id,
                ClosePhase::AwaitingStopWorkConfirmation,
                ClosePhase::Completed,
            )
            .await?
        }
        ClosePhase::SettlingActiveWork => {
            transition_if_current(
                state,
                &obligation.attempt_id,
                ClosePhase::SettlingActiveWork,
                ClosePhase::CancelRequestedDuringSettlement,
            )
            .await?
        }
        ClosePhase::AwaitingRetirementInspection => {
            transition_if_current(
                state,
                &obligation.attempt_id,
                ClosePhase::AwaitingRetirementInspection,
                ClosePhase::Completed,
            )
            .await?
        }
        ClosePhase::AwaitingLossConfirmation => {
            transition_if_current(
                state,
                &obligation.attempt_id,
                ClosePhase::AwaitingLossConfirmation,
                ClosePhase::Completed,
            )
            .await?
        }
        _ => obligation,
    };
    if next.phase == ClosePhase::CancelRequestedDuringSettlement {
        transition_if_current(
            state,
            &next.attempt_id,
            ClosePhase::CancelRequestedDuringSettlement,
            ClosePhase::Completed,
        )
        .await
    } else {
        Ok(next)
    }
}

async fn settle_attempt(
    state: &AppState,
    attempt_id: String,
    confirmed_stop: bool,
) -> Result<CloseObligation, AppError> {
    let obligation = state
        .db
        .get_close_obligation(&attempt_id)
        .await
        .map_err(map_close_db_error)?
        .ok_or_else(|| AppError::NotFound(format!("close attempt not found: {attempt_id}")))?;
    match obligation.phase {
        ClosePhase::AwaitingBlockerResolution => {
            transition_if_current(
                state,
                &attempt_id,
                ClosePhase::AwaitingBlockerResolution,
                ClosePhase::SettlingActiveWork,
            )
            .await?;
        }
        ClosePhase::AwaitingStopWorkConfirmation if confirmed_stop => {
            transition_if_current(
                state,
                &attempt_id,
                ClosePhase::AwaitingStopWorkConfirmation,
                ClosePhase::SettlingActiveWork,
            )
            .await?;
        }
        ClosePhase::SettlingActiveWork => {}
        _ => return Ok(obligation),
    }
    cancel_all_direct_turns(state, &attempt_id).await?;
    let current = state
        .db
        .get_close_obligation(&attempt_id)
        .await
        .map_err(map_close_db_error)?
        .ok_or_else(|| AppError::NotFound(format!("close attempt not found: {attempt_id}")))?;
    if current.phase == ClosePhase::CancelRequestedDuringSettlement {
        return transition_if_current(
            state,
            &attempt_id,
            ClosePhase::CancelRequestedDuringSettlement,
            ClosePhase::Completed,
        )
        .await;
    }
    if current_busy_member(state, &current).await?.is_some() {
        return state
            .db
            .get_close_obligation(&attempt_id)
            .await
            .map_err(map_close_db_error)?
            .ok_or_else(|| AppError::NotFound(format!("close attempt not found: {attempt_id}")));
    }
    let moved = transition_if_current(
        state,
        &attempt_id,
        ClosePhase::SettlingActiveWork,
        ClosePhase::AwaitingRetirementInspection,
    )
    .await?;
    inspect_then_maybe_retire(state, moved).await
}

async fn inspect_then_maybe_retire(
    state: &AppState,
    obligation: CloseObligation,
) -> Result<CloseObligation, AppError> {
    if obligation.phase != ClosePhase::AwaitingRetirementInspection {
        return Ok(obligation);
    }
    let topology = state
        .db
        .product_conversation_topology(&obligation.product_conversation_id)
        .await
        .map_err(map_close_db_error)?;
    let conversations = load_topology_conversations(state, &topology).await?;
    let inspection = compute_close_inspection(&conversations)?;
    let phase = if inspection.has_losses {
        ClosePhase::AwaitingLossConfirmation
    } else {
        ClosePhase::RetirementRequested
    };
    let obligation = state
        .db
        .replace_inspection(
            &obligation.attempt_id,
            phase,
            Some(&inspection.aggregate.generation),
            Some(&inspection.aggregate.fingerprint),
            inspection.inspections,
        )
        .await
        .map_err(map_close_db_error)?;
    if obligation.phase == ClosePhase::RetirementRequested {
        drive_retirement_then_maybe_finalize(state, obligation).await
    } else {
        Ok(obligation)
    }
}

async fn drive_retirement_then_maybe_finalize(
    state: &AppState,
    obligation: CloseObligation,
) -> Result<CloseObligation, AppError> {
    if obligation.phase != ClosePhase::RetirementRequested {
        return Ok(obligation);
    }
    let topology = state
        .db
        .product_conversation_topology(&obligation.product_conversation_id)
        .await
        .map_err(map_close_db_error)?;
    let conversations = load_topology_conversations(state, &topology).await?;
    let representatives = collect_scope_representatives(&conversations);
    let aggregate_member_ids: HashSet<String> =
        conversations.iter().map(|c| c.id.clone()).collect();

    let mut records = Vec::new();
    for representative in representatives {
        let scope_id = representative.scope_id.clone();
        let externally_owned =
            scope_has_live_owner_outside_aggregate(state, &scope_id, &aggregate_member_ids).await?;
        let pre = snapshot_scope_inventory(state, &representative.conversation).await?;
        let _cleanup = run_resource_cleanup_cascade(state, &representative.conversation).await?;
        let post = snapshot_scope_inventory(state, &representative.conversation).await?;
        records.push(CleanupRecord {
            scope: Some(scope_id),
            pre,
            post,
            externally_owned,
        });
    }

    let mut failures = false;
    for record in &records {
        failures |= record_retirement_evidence(state, &obligation.attempt_id, record).await?;
    }

    let current = state
        .db
        .get_close_obligation(&obligation.attempt_id)
        .await
        .map_err(map_close_db_error)?
        .ok_or_else(|| {
            AppError::NotFound(format!(
                "close attempt not found: {}",
                obligation.attempt_id
            ))
        })?;
    let next = if failures {
        if current.phase == ClosePhase::RetirementRequested {
            transition_if_current(
                state,
                &current.attempt_id,
                ClosePhase::RetirementRequested,
                ClosePhase::NeedsRepair,
            )
            .await?
        } else {
            current
        }
    } else {
        finalize_history_message(state, &current).await?
    };
    Ok(next)
}

async fn finalize_history_message(
    state: &AppState,
    obligation: &CloseObligation,
) -> Result<CloseObligation, AppError> {
    if obligation.phase == ClosePhase::Completed {
        return Ok(obligation.clone());
    }
    let message_id = uuid::Uuid::new_v4().to_string();
    let topology = state
        .db
        .finalize_history(
            &obligation.attempt_id,
            &message_id,
            "Conversation closed. History finalized.",
        )
        .await
        .map_err(map_close_db_error)?;
    for member in &topology.member_conversation_ids {
        let archived = state
            .db
            .get_conversation(member.as_str())
            .await
            .map_err(|error| AppError::Internal(error.to_string()))?;
        let broadcast_tx = state
            .runtime
            .conversation_broadcaster(member.as_str())
            .await;
        let archived_state = archived.state.clone();
        let _ = broadcast_tx.send_seq(|seq| SseEvent::StateChange {
            sequence_id: seq,
            presentation_mode: archived_state.presentation_mode().to_string(),
            state: archived_state,
            state_updated_at: archived.state_updated_at,
        });
    }
    state
        .db
        .get_close_obligation(&obligation.attempt_id)
        .await
        .map_err(map_close_db_error)?
        .ok_or_else(|| {
            AppError::NotFound(format!(
                "close attempt not found: {}",
                obligation.attempt_id
            ))
        })
}

fn no_worktree_generation() -> &'static str {
    NO_WORKTREE_AGGREGATE
}

fn no_worktree_fingerprint() -> &'static str {
    NO_WORKTREE_AGGREGATE
}

fn collect_scope_representatives(conversations: &[Conversation]) -> Vec<ScopeRepresentative> {
    let mut map = HashMap::new();
    for conv in conversations {
        let Some(scope_id) = conv.attached_work_scope_id.clone() else {
            continue;
        };
        map.entry(scope_id.as_str().to_string())
            .or_insert_with(|| ScopeRepresentative {
                scope_id,
                conversation: conv.clone(),
            });
    }
    let mut representatives: Vec<_> = map.into_values().collect();
    representatives.sort_by(|a, b| a.scope_id.as_str().cmp(b.scope_id.as_str()));
    representatives
}

fn collect_worktree_scopes(conversations: &[Conversation]) -> Vec<WorktreeInspectionScope<'_>> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for conv in conversations {
        let (Some(scope), Some(path)) = (
            conv.attached_work_scope_id.clone(),
            conv.conv_mode.worktree_path(),
        ) else {
            continue;
        };
        if seen.insert(scope.clone()) {
            out.push(WorktreeInspectionScope {
                scope_id: scope,
                root: FsPath::new(path),
            });
        }
    }
    out.sort_by(|a, b| a.scope_id.as_str().cmp(b.scope_id.as_str()));
    out
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    use std::fmt::Write as _;
    sha2::Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut output, byte| {
            write!(output, "{byte:02x}").expect("writing to String cannot fail");
            output
        })
}

fn aggregate_fingerprint(inspections: &[ScopeInspectionInput]) -> Result<String, AppError> {
    let mut canonical_scopes = Vec::with_capacity(inspections.len());
    for inspection in inspections {
        let mut losses: Vec<_> = inspection
            .losses
            .iter()
            .map(|loss| AggregateFingerprintLossCanonical {
                category: loss.category.as_str(),
                item_identity: loss.item_identity.clone(),
            })
            .collect();
        losses.sort_by(|a, b| {
            a.category
                .cmp(b.category)
                .then_with(|| a.item_identity.cmp(&b.item_identity))
        });
        canonical_scopes.push(AggregateFingerprintScopeCanonical {
            scope: inspection.scope.as_str().to_string(),
            generation: inspection.generation.clone(),
            fingerprint: inspection.fingerprint.clone(),
            losses,
        });
    }
    canonical_scopes.sort_by(|a, b| a.scope.cmp(&b.scope));
    let bytes = serde_json::to_vec(&AggregateFingerprintCanonical {
        scopes: &canonical_scopes,
    })
    .map_err(|error| AppError::Internal(error.to_string()))?;
    Ok(sha256_hex(&bytes))
}

fn compute_close_inspection(
    conversations: &[Conversation],
) -> Result<CloseInspectionComputation, AppError> {
    let scopes = collect_worktree_scopes(conversations);
    if scopes.is_empty() {
        return Ok(CloseInspectionComputation {
            aggregate: CloseInspectionAggregate {
                generation: no_worktree_generation().to_string(),
                fingerprint: no_worktree_fingerprint().to_string(),
            },
            inspections: Vec::new(),
            has_losses: false,
        });
    }

    let mut inspections = Vec::new();
    let mut has_losses = false;
    for scope in scopes {
        let repo_root = crate::git_ops::run_git(scope.root, &["rev-parse", "--show-toplevel"])
            .map(PathBuf::from)
            .map_err(AppError::Internal)?;
        let inventory =
            inspect_worktree_loss_inventory(&repo_root, &[scope]).map_err(AppError::Internal)?;
        for inventory_scope in &inventory.scopes {
            let losses: Vec<LossRowInput> = inventory_scope
                .losses
                .iter()
                .map(|loss| LossRowInput {
                    category: map_loss_category(loss.kind),
                    item_identity: loss.item_identity.clone(),
                })
                .collect();
            has_losses |= !losses.is_empty();
            inspections.push(ScopeInspectionInput {
                scope: inventory_scope.scope_id.clone(),
                generation: Some(inventory.generation.clone()),
                fingerprint: Some(inventory.fingerprint.clone()),
                losses,
            });
        }
    }
    inspections.sort_by(|a, b| a.scope.as_str().cmp(b.scope.as_str()));
    let fingerprint = aggregate_fingerprint(&inspections)?;
    let generation = fingerprint.clone();
    for inspection in &mut inspections {
        inspection.generation = Some(generation.clone());
    }
    Ok(CloseInspectionComputation {
        aggregate: CloseInspectionAggregate {
            generation,
            fingerprint,
        },
        inspections,
        has_losses,
    })
}

async fn snapshot_scope_inventory(
    state: &AppState,
    conversation: &Conversation,
) -> Result<Option<ScopeInventorySnapshot>, AppError> {
    let Some(scope_id) = conversation.attached_work_scope_id.clone() else {
        return Ok(None);
    };
    let scope_key = crate::work_scope::ResourceScopeKey::Work(scope_id.clone());
    let actor = crate::work_scope::EffectiveResourceAccess::new(
        conversation.id.clone(),
        match conversation.conv_mode {
            crate::db::ConvMode::Explore { .. } => crate::work_scope::ResourceAuthority::Restricted,
            _ => crate::work_scope::ResourceAuthority::Work,
        },
    );
    let inventory = assemble_inventory(
        &scope_key,
        Some(&actor),
        conversation.runtime_role == crate::work_scope::RuntimeRole::User,
        state.runtime.bash_handles(),
        state.runtime.tmux_registry(),
        state.runtime.browser_sessions(),
    )
    .await;
    Ok(Some(ScopeInventorySnapshot {
        worktree_path: conversation
            .conv_mode
            .worktree_path()
            .map(ToOwned::to_owned),
        worktree_exists: conversation
            .conv_mode
            .worktree_path()
            .is_some_and(|path| FsPath::new(path).exists()),
        bash_handles: inventory.bash,
        tmux_live: inventory.tmux.is_some(),
        browser_live: inventory.browser.is_some(),
        pty_live: state.terminals.get(&scope_key).is_some(),
    }))
}

async fn scope_has_live_owner_outside_aggregate(
    state: &AppState,
    scope_id: &phoenix_core::work_scope::WorkScopeId,
    aggregate_member_ids: &HashSet<String>,
) -> Result<bool, AppError> {
    let candidates = state
        .db
        .list_conversations_for_work_scope(scope_id)
        .await
        .map_err(|error| AppError::Internal(error.to_string()))?;
    Ok(candidates.into_iter().any(|candidate| {
        !aggregate_member_ids.contains(candidate.id.as_str())
            && crate::runtime::conversation_attachment_retains_work_scope(&candidate)
            && !candidate.archived
    }))
}

async fn record_resource(
    state: &AppState,
    attempt_id: &str,
    scope: &phoenix_core::work_scope::WorkScopeId,
    resource_kind: RetiredResourceKind,
    resource_identity: &str,
    outcome: RetirementOutcome,
    detail: Option<String>,
) -> Result<(), AppError> {
    state
        .db
        .record_retirement_resource(
            attempt_id,
            scope,
            resource_kind,
            resource_identity,
            outcome,
            detail.as_deref(),
        )
        .await
        .map_err(map_close_db_error)?;
    Ok(())
}

#[allow(clippy::too_many_lines)]
async fn record_retirement_evidence(
    state: &AppState,
    attempt_id: &str,
    record: &CleanupRecord,
) -> Result<bool, AppError> {
    let Some(scope) = record.scope.as_ref() else {
        return Ok(false);
    };

    let pre = record.pre.as_ref();
    let post = record.post.as_ref();
    let scope_key_identity = crate::work_scope::ResourceScopeKey::Work(scope.clone()).stable_key();
    let mut failures = false;

    let mut all_handle_ids = BTreeSet::new();
    if let Some(pre) = pre {
        for handle in &pre.bash_handles {
            all_handle_ids.insert(handle.handle_id.clone());
        }
    }
    if all_handle_ids.is_empty() {
        record_resource(
            state,
            attempt_id,
            scope,
            RetiredResourceKind::BashProcessGroup,
            &format!("{scope_key_identity}#bash:none"),
            RetirementOutcome::AbsenceAdopted,
            Some("no pre-cleanup bash handles".to_string()),
        )
        .await?;
    } else {
        let post_states: HashMap<_, _> = post
            .map(|snapshot| {
                snapshot
                    .bash_handles
                    .iter()
                    .map(|handle| (handle.handle_id.as_str(), handle.state))
                    .collect::<HashMap<_, _>>()
            })
            .unwrap_or_default();
        for handle_id in all_handle_ids {
            let outcome = match post_states.get(handle_id.as_str()) {
                Some(
                    phoenix_core::domain::work_scope_inventory::BashHandleState::Running
                    | phoenix_core::domain::work_scope_inventory::BashHandleState::KillPendingKernel,
                ) => {
                    failures = true;
                    RetirementOutcome::Residual(RetirementFailureReason::ResidualProcessAlive)
                }
                _ => RetirementOutcome::Retired,
            };
            record_resource(
                state,
                attempt_id,
                scope,
                RetiredResourceKind::BashProcessGroup,
                &handle_id,
                outcome,
                Some(format!("scope={scope_key_identity}")),
            )
            .await?;
        }
    }

    let before_tmux = pre.is_some_and(|snapshot| snapshot.tmux_live);
    let after_tmux = post.is_some_and(|snapshot| snapshot.tmux_live);
    record_simple_resource_outcome(
        state,
        attempt_id,
        scope,
        RetiredResourceKind::TmuxServer,
        &format!("{scope_key_identity}#tmux"),
        before_tmux,
        after_tmux,
    )
    .await?;
    failures |= after_tmux && !record.externally_owned;

    let before_browser = pre.is_some_and(|snapshot| snapshot.browser_live);
    let after_browser = post.is_some_and(|snapshot| snapshot.browser_live);
    record_simple_resource_outcome(
        state,
        attempt_id,
        scope,
        RetiredResourceKind::BrowserSession,
        &format!("{scope_key_identity}#browser"),
        before_browser,
        after_browser,
    )
    .await?;
    failures |= after_browser && !record.externally_owned;

    let before_pty = pre.is_some_and(|snapshot| snapshot.pty_live);
    let after_pty = post.is_some_and(|snapshot| snapshot.pty_live);
    record_simple_resource_outcome(
        state,
        attempt_id,
        scope,
        RetiredResourceKind::PtySession,
        &format!("{scope_key_identity}#pty"),
        before_pty,
        after_pty,
    )
    .await?;
    failures |= after_pty && !record.externally_owned;

    let worktree_identity = pre
        .and_then(|snapshot| snapshot.worktree_path.clone())
        .or_else(|| post.and_then(|snapshot| snapshot.worktree_path.clone()))
        .unwrap_or_else(|| format!("{scope_key_identity}#worktree:none"));
    let before_worktree = pre.is_some_and(|snapshot| snapshot.worktree_exists);
    let after_worktree = post.is_some_and(|snapshot| snapshot.worktree_exists);
    record_simple_resource_outcome(
        state,
        attempt_id,
        scope,
        RetiredResourceKind::Worktree,
        &worktree_identity,
        before_worktree,
        after_worktree,
    )
    .await?;
    failures |= after_worktree && !record.externally_owned;

    let equivalent_outcome = if record.externally_owned {
        RetirementOutcome::Retired
    } else {
        RetirementOutcome::AbsenceAdopted
    };
    let equivalent_detail = record
        .externally_owned
        .then(|| {
            "preserved because another live attachment outside aggregate still owns this scope"
                .to_string()
        })
        .or_else(|| Some("no outside live owner preserved this scope".to_string()));
    record_resource(
        state,
        attempt_id,
        scope,
        RetiredResourceKind::EquivalentLiveResource,
        &scope_key_identity,
        equivalent_outcome,
        equivalent_detail,
    )
    .await?;

    Ok(failures)
}

async fn record_simple_resource_outcome(
    state: &AppState,
    attempt_id: &str,
    scope: &phoenix_core::work_scope::WorkScopeId,
    kind: RetiredResourceKind,
    identity: &str,
    before_present: bool,
    after_present: bool,
) -> Result<(), AppError> {
    let (outcome, detail) = if after_present {
        (
            RetirementOutcome::Residual(RetirementFailureReason::ManualRepairRequired),
            Some("resource still live after cleanup".to_string()),
        )
    } else if before_present {
        (
            RetirementOutcome::Retired,
            Some("resource retired during cleanup".to_string()),
        )
    } else {
        (
            RetirementOutcome::AbsenceAdopted,
            Some("resource absent before cleanup".to_string()),
        )
    };
    record_resource(state, attempt_id, scope, kind, identity, outcome, detail).await
}

async fn current_busy_member(
    state: &AppState,
    obligation: &CloseObligation,
) -> Result<Option<String>, AppError> {
    let topology = state
        .db
        .product_conversation_topology(&obligation.product_conversation_id)
        .await
        .map_err(map_close_db_error)?;
    for member in topology.member_conversation_ids {
        let conv = state
            .db
            .get_conversation(member.as_str())
            .await
            .map_err(|error| AppError::Internal(error.to_string()))?;
        if conv.state.is_busy() || matches!(conv.state, ConvState::AwaitingContinuation { .. }) {
            return Ok(Some(conv.id));
        }
        let repo = phoenix_db::workflow::WorkflowRepository::new(state.db.pool().clone());
        if repo
            .load_active_runtime_turn(&phoenix_workflow::ConversationAuthority(conv.id.clone()))
            .await
            .map_err(|error| AppError::Internal(error.to_string()))?
            .is_some()
        {
            return Ok(Some(conv.id));
        }
    }
    Ok(None)
}

async fn cancel_all_direct_turns(state: &AppState, attempt_id: &str) -> Result<(), AppError> {
    let obligation = state
        .db
        .get_close_obligation(attempt_id)
        .await
        .map_err(map_close_db_error)?
        .ok_or_else(|| AppError::NotFound(format!("close attempt not found: {attempt_id}")))?;
    let topology = state
        .db
        .product_conversation_topology(&obligation.product_conversation_id)
        .await
        .map_err(map_close_db_error)?;
    let repo = phoenix_db::workflow::WorkflowRepository::new(state.db.pool().clone());
    for member in topology.member_conversation_ids {
        if let Some(turn) = repo
            .load_active_runtime_turn(&phoenix_workflow::ConversationAuthority(
                member.as_str().to_string(),
            ))
            .await
            .map_err(|error| AppError::Internal(error.to_string()))?
        {
            repo.terminate_authoritative_turn(phoenix_workflow::TurnCommand::Cancel {
                turn_id: turn.id,
                expected_generation: turn.generation,
            })
            .await
            .map_err(|error| AppError::Internal(error.to_string()))?;
        }
    }
    Ok(())
}

async fn latest_attempt_for_conversation(
    state: &AppState,
    id: &str,
) -> Result<CloseObligation, AppError> {
    let topology = state
        .db
        .product_conversation_topology(&parse_product_conv_id(id)?)
        .await
        .map_err(map_close_db_error)?;
    state
        .db
        .latest_close_obligation_for_product(&topology.root_conversation_id)
        .await
        .map_err(map_close_db_error)?
        .ok_or_else(|| AppError::NotFound(format!("no close attempt for conversation {id}")))
}

async fn load_status(
    state: &AppState,
    obligation: &CloseObligation,
) -> Result<CloseStatusResponse, AppError> {
    let topology = state
        .db
        .product_conversation_topology(&obligation.product_conversation_id)
        .await
        .map_err(map_close_db_error)?;
    let inspections = state
        .db
        .list_close_inspections(&obligation.attempt_id)
        .await
        .map_err(map_close_db_error)?;
    let losses = state
        .db
        .list_close_inspection_losses(&obligation.attempt_id)
        .await
        .map_err(map_close_db_error)?;
    let retirement_evidence = state
        .db
        .list_retirement_evidence(&obligation.attempt_id)
        .await
        .map_err(map_close_db_error)?;
    Ok(CloseStatusResponse {
        attempt: CloseAttemptView {
            attempt_id: obligation.attempt_id.clone(),
            conversation_id: obligation.product_conversation_id.as_str().to_string(),
            root_conversation_id: topology.root_conversation_id.as_str().to_string(),
            latest_conversation_id: topology.latest_conversation_id.as_str().to_string(),
            phase: obligation.phase,
            inspection_generation: obligation.inspection_generation.clone(),
            inspection_fingerprint: obligation.inspection_fingerprint.clone(),
            inspections,
            losses,
            retirement_evidence,
            completed_at: obligation.completed_at,
        },
    })
}

async fn load_topology_conversations(
    state: &AppState,
    topology: &phoenix_db::ProductConversationTopology,
) -> Result<Vec<Conversation>, AppError> {
    let mut out = Vec::with_capacity(topology.member_conversation_ids.len());
    for member in &topology.member_conversation_ids {
        out.push(
            state
                .db
                .get_conversation(member.as_str())
                .await
                .map_err(|error| AppError::Internal(error.to_string()))?,
        );
    }
    Ok(out)
}

fn map_loss_category(kind: crate::git_ops::WorktreeLossKind) -> LossCategory {
    match kind {
        crate::git_ops::WorktreeLossKind::StagedTrackedPaths => LossCategory::StagedTrackedPaths,
        crate::git_ops::WorktreeLossKind::UnstagedTrackedPaths => {
            LossCategory::UnstagedTrackedPaths
        }
        crate::git_ops::WorktreeLossKind::UntrackedNonIgnoredPaths => {
            LossCategory::UntrackedNonIgnoredPaths
        }
        crate::git_ops::WorktreeLossKind::InitializedSubmoduleState => {
            LossCategory::InitializedSubmoduleState
        }
        crate::git_ops::WorktreeLossKind::DetachedUnreachableCommits => {
            LossCategory::DetachedUnreachableCommits
        }
    }
}

async fn transition_if_current(
    state: &AppState,
    attempt_id: &str,
    current: ClosePhase,
    next: ClosePhase,
) -> Result<CloseObligation, AppError> {
    state
        .db
        .transition_close_phase(attempt_id, current, next)
        .await
        .map_err(map_close_db_error)
}

fn parse_product_conv_id(value: &str) -> Result<ProductConversationId, AppError> {
    ProductConversationId::parse(value).map_err(|error| AppError::BadRequest(error.to_string()))
}

fn map_close_db_error(error: DbError) -> AppError {
    match error {
        DbError::CloseConversationNotFound(id)
        | DbError::CloseAttemptNotFound(id)
        | DbError::ConversationNotFound(id) => AppError::NotFound(id),
        DbError::CloseAttemptConflict(message)
        | DbError::CloseDeleteBlocked(message)
        | DbError::CloseScopeOutsideAggregate { scope: message, .. } => AppError::Conflict(
            Box::new(ConflictErrorResponse::new(message, "close_conflict")),
        ),
        DbError::ClosePhaseConflict {
            expected, actual, ..
        } => AppError::Conflict(Box::new(ConflictErrorResponse::new(
            format!("close phase conflict: expected {expected}, actual {actual}"),
            "close_phase_conflict",
        ))),
        DbError::InvalidCloseTransition { from, to, .. } => {
            AppError::Conflict(Box::new(ConflictErrorResponse::new(
                format!("invalid close transition: {from} -> {to}"),
                "close_phase_conflict",
            )))
        }
        other => AppError::Internal(other.to_string()),
    }
}

fn phase_conflict(obligation: &CloseObligation, expected: &str) -> AppError {
    AppError::Conflict(Box::new(ConflictErrorResponse::new(
        format!(
            "close phase conflict: expected {expected}, actual {}",
            obligation.phase.as_str()
        ),
        "close_phase_conflict",
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::handlers::hard_delete_cascade_tests::make_test_state;
    use crate::db::ConvMode;
    use crate::state_machine::ConvState;
    use axum::extract::{Path, State};
    use tempfile::TempDir;

    async fn set_state(db: &crate::db::Database, id: &str, state: &ConvState) {
        db.update_conversation_state(id, state).await.unwrap();
    }

    #[tokio::test]
    async fn non_project_close_succeeds_without_worktree() {
        let state = make_test_state().await;
        state
            .db
            .create_conversation("root", "root", "/tmp", true, None, None)
            .await
            .unwrap();
        set_state(&state.db, "root", &ConvState::Idle).await;
        let Json(status) = request_close(
            State(state.clone()),
            Path("root".to_string()),
            Some(Json(CloseRequestBody {
                attempt_id: Some("a1".into()),
            })),
        )
        .await
        .unwrap();
        assert_eq!(status.attempt.phase, ClosePhase::Completed);
        assert!(state.db.get_conversation("root").await.unwrap().archived);
    }

    #[tokio::test]
    async fn busy_confirmation_requires_explicit_stop_then_retry() {
        let state = make_test_state().await;
        state
            .db
            .create_conversation("root", "root", "/tmp", true, None, None)
            .await
            .unwrap();
        set_state(&state.db, "root", &ConvState::LlmRequesting { attempt: 1 }).await;
        let Json(status) = request_close(
            State(state.clone()),
            Path("root".to_string()),
            Some(Json(CloseRequestBody {
                attempt_id: Some("a2".into()),
            })),
        )
        .await
        .unwrap();
        assert_eq!(
            status.attempt.phase,
            ClosePhase::AwaitingStopWorkConfirmation
        );
    }

    #[tokio::test]
    async fn compute_close_inspection_aggregates_multiple_scopes_deterministically() {
        let repo_a = TempDir::new().unwrap();
        let repo_b = TempDir::new().unwrap();
        for repo in [&repo_a, &repo_b] {
            crate::git_ops::run_git(repo.path(), &["init", "--quiet", "--initial-branch=main"])
                .unwrap();
            crate::git_ops::run_git(repo.path(), &["config", "user.email", "test@example.com"])
                .unwrap();
            crate::git_ops::run_git(repo.path(), &["config", "user.name", "test"]).unwrap();
            std::fs::write(repo.path().join("tracked.txt"), "one").unwrap();
            crate::git_ops::run_git(repo.path(), &["add", "tracked.txt"]).unwrap();
            crate::git_ops::run_git(repo.path(), &["commit", "-q", "-m", "init"]).unwrap();
        }
        std::fs::write(repo_a.path().join("tracked.txt"), "dirty-a").unwrap();
        std::fs::write(repo_b.path().join("tracked.txt"), "dirty-b").unwrap();

        let conv_a = Conversation {
            id: "conv-a".into(),
            slug: Some("a".into()),
            title: Some("a".into()),
            cwd: repo_a.path().to_str().unwrap().into(),
            parent_conversation_id: None,
            user_initiated: true,
            state: ConvState::Idle,
            state_updated_at: chrono::Utc::now(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            archived: false,
            transcript_generation: 1,
            model: None,
            effort: None,
            project_id: None,
            desired_base_branch: None,
            runtime_role: phoenix_core::work_scope::RuntimeRole::User,
            attached_work_scope_id: Some(
                phoenix_core::work_scope::WorkScopeId::parse("scope-a").unwrap(),
            ),
            conv_mode: ConvMode::Work {
                branch_name: crate::db::NonEmptyString::new("task-a").unwrap(),
                worktree_path: crate::db::NonEmptyString::new(repo_a.path().to_str().unwrap())
                    .unwrap(),
                base_branch: crate::db::NonEmptyString::new("main").unwrap(),
                task_id: crate::db::NonEmptyString::new("A").unwrap(),
                task_title: crate::db::NonEmptyString::new("A title").unwrap(),
            },
            seed_parent_id: None,
            spawned_from_conversation_id: None,
            seed_label: None,
            continued_in_conv_id: None,
            chain_name: None,
            llm_language: crate::llm_language::LlmLanguage::default(),
            message_count: 0,
        };
        let mut conv_b = conv_a.clone();
        conv_b.id = "conv-b".into();
        conv_b.slug = Some("b".into());
        conv_b.title = Some("b".into());
        conv_b.cwd = repo_b.path().to_str().unwrap().into();
        conv_b.attached_work_scope_id =
            Some(phoenix_core::work_scope::WorkScopeId::parse("scope-b").unwrap());
        conv_b.conv_mode = ConvMode::Work {
            branch_name: crate::db::NonEmptyString::new("task-b").unwrap(),
            worktree_path: crate::db::NonEmptyString::new(repo_b.path().to_str().unwrap()).unwrap(),
            base_branch: crate::db::NonEmptyString::new("main").unwrap(),
            task_id: crate::db::NonEmptyString::new("B").unwrap(),
            task_title: crate::db::NonEmptyString::new("B title").unwrap(),
        };

        let ab = compute_close_inspection(&[conv_a.clone(), conv_b.clone()]).unwrap();
        let ba = compute_close_inspection(&[conv_b, conv_a]).unwrap();
        assert_eq!(ab.aggregate, ba.aggregate);
        assert_eq!(ab.inspections, ba.inspections);
        assert!(ab.has_losses);
    }

    #[tokio::test]
    async fn stale_fingerprint_reverts_to_reinspection() {
        let state = make_test_state().await;
        let repo = TempDir::new().unwrap();
        crate::git_ops::run_git(repo.path(), &["init", "--quiet", "--initial-branch=main"])
            .unwrap();
        crate::git_ops::run_git(repo.path(), &["config", "user.email", "test@example.com"])
            .unwrap();
        crate::git_ops::run_git(repo.path(), &["config", "user.name", "test"]).unwrap();
        std::fs::write(repo.path().join("tracked.txt"), "one").unwrap();
        crate::git_ops::run_git(repo.path(), &["add", "tracked.txt"]).unwrap();
        crate::git_ops::run_git(repo.path(), &["commit", "-q", "-m", "init"]).unwrap();
        state
            .db
            .create_conversation_with_project(
                "root",
                "root",
                repo.path().to_str().unwrap(),
                true,
                None,
                None,
                None,
                &ConvMode::Work {
                    branch_name: crate::db::NonEmptyString::new("task-1").unwrap(),
                    worktree_path: crate::db::NonEmptyString::new(repo.path().to_str().unwrap())
                        .unwrap(),
                    base_branch: crate::db::NonEmptyString::new("main").unwrap(),
                    task_id: crate::db::NonEmptyString::new("T1").unwrap(),
                    task_title: crate::db::NonEmptyString::new("Task").unwrap(),
                },
                None,
                None,
                None,
                crate::llm_language::LlmLanguage::default(),
            )
            .await
            .unwrap();
        std::fs::write(repo.path().join("tracked.txt"), "two").unwrap();
        let Json(status) = request_close(
            State(state.clone()),
            Path("root".to_string()),
            Some(Json(CloseRequestBody {
                attempt_id: Some("a3".into()),
            })),
        )
        .await
        .unwrap();
        assert_eq!(status.attempt.phase, ClosePhase::AwaitingLossConfirmation);
        std::fs::write(repo.path().join("tracked.txt"), "three").unwrap();
        let Json(status) = confirm_close_loss(
            State(state.clone()),
            Path("root".to_string()),
            Json(ConfirmLossBody {
                inspection_generation: status.attempt.inspection_generation.clone().unwrap(),
                inspection_fingerprint: status.attempt.inspection_fingerprint.clone().unwrap(),
            }),
        )
        .await
        .unwrap();
        assert_eq!(
            status.attempt.phase,
            ClosePhase::AwaitingRetirementInspection
        );
    }

    #[tokio::test]
    async fn restart_resumer_advances_machine_owned_phase() {
        let state = make_test_state().await;
        state
            .db
            .create_conversation("root", "root", "/tmp", true, None, None)
            .await
            .unwrap();
        set_state(&state.db, "root", &ConvState::Idle).await;
        state
            .db
            .begin_close(&ProductConversationId::parse("root").unwrap(), "a4")
            .await
            .unwrap();
        let resumed = resume_pending_close_attempts(&state).await.unwrap();
        assert_eq!(resumed, 1);
        let obligation = state.db.get_close_obligation("a4").await.unwrap().unwrap();
        assert_eq!(obligation.phase, ClosePhase::Completed);
    }

    #[tokio::test]
    async fn delete_idempotence_returns_success_twice() {
        let state = make_test_state().await;
        state
            .db
            .create_conversation("root", "root", "/tmp", true, None, None)
            .await
            .unwrap();
        set_state(&state.db, "root", &ConvState::Idle).await;
        let _ = request_close(
            State(state.clone()),
            Path("root".to_string()),
            Some(Json(CloseRequestBody {
                attempt_id: Some("a5".into()),
            })),
        )
        .await
        .unwrap();
        let _ = delete_closed_history(State(state.clone()), Path("root".to_string()))
            .await
            .unwrap();
        let _ = delete_closed_history(State(state.clone()), Path("root".to_string()))
            .await
            .unwrap();
    }
}
