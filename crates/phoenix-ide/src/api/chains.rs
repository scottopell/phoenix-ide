#![allow(clippy::wildcard_enum_match_arm)]
//! Phoenix Chains v1 — HTTP API handlers (REQ-CHN-003 / 004 / 005 / 007).
//!
//! Four endpoints live here:
//!
//! - `GET /api/chains/:rootId` — chain page snapshot (members + Q&A history
//!   + name + computed totals for staleness comparison)
//! - `POST /api/chains/:rootId/qa { question }` — submit a question; returns
//!   the `chain_qa_id` synchronously while streaming + persistence run on a
//!   detached task in [`crate::chain_qa::ChainQa::submit_question`]
//! - `PATCH /api/chains/:rootId/name { name? }` — set or clear the chain's
//!   user-overridden name; returns the refreshed snapshot
//! - `POST /api/chains/:rootId/regenerate-name` (no body) — derive a prose name
//!   by summarizing each member's first user message via a cheap LLM and persist
//!   it as `chain_name`; returns the refreshed snapshot (REQ-CHN-010)
//! - `GET /api/chains/:rootId/stream` — SSE subscription for streaming Q&A
//!   token events (publishes [`crate::api::wire::ChainSseWireEvent`])
//!
//! All endpoints reject non-chain-root inputs with 404. The chain
//! validity test mirrors the one in `ChainQa::prepare_invocation`:
//! `chain_root_of(id) == Some(id)` AND `chain_members_forward(id).len() >= 2`.
//! Single-member roots and non-root members are not chains.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
use axum::Json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;
use ts_rs::TS;

use super::handlers::{run_archive_cascade, run_hard_delete_cascade, AppError};
use super::types::{ConflictErrorResponse, SuccessResponse};
use super::wire::ChainSseWireEvent;
use super::AppState;
use crate::chain_qa::ChainQaError;
use crate::db::{ChainQaRow, Conversation, DbError};
use crate::state_machine::ConvState;

/// Maximum length (in chars) of a user-set chain name. The cap is arbitrary
/// — short enough that the value comfortably fits as a sidebar label and the
/// chain page header without truncation, long enough that a reasonable label
/// like "auth refactor — staged migration" is not rejected.
const CHAIN_NAME_MAX_CHARS: usize = 200;

// ---------------------------------------------------------------------------
// Response/request shapes
// ---------------------------------------------------------------------------

/// Chain snapshot returned by `GET /api/chains/:rootId` and the body of the
/// PATCH name response.
///
/// `display_name` is the resolved label the UI renders without re-running
/// the `chain_name → root.title → slug` fallback. `chain_name` is the
/// user-set override (or `None` when unset) — kept distinct so an "edit"
/// affordance can show the unset state. `current_member_count` and
/// `current_total_messages` let the UI compute staleness against each
/// stored Q&A's snapshot integers (REQ-CHN-005) without a second roundtrip.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct ChainView {
    pub root_conv_id: String,
    pub chain_name: Option<String>,
    pub display_name: String,
    /// `true` when the chain is archived. Chain archive is a write-cascade
    /// across all members, so any member's `archived` flag is authoritative;
    /// we read it off the root for clarity. Archive is a terminal lifecycle
    /// transition — archived chain roots 404 on the chain route, so the UI
    /// has no unarchive affordance.
    pub archived: bool,
    pub members: Vec<ChainMemberSummary>,
    pub qa_history: Vec<ChainQaRow>,
    pub current_member_count: i64,
    pub current_total_messages: i64,
    /// The chain's work identity — worktree / branch / base / task — or `None`
    /// when the chain has no managed work scope (REQ-CHN-008). PR health is not
    /// carried here; the dock fetches it client-side off `work_conv_id` via the
    /// per-conversation PR-status pipeline (see [`ChainWorkIdentity`]).
    pub work_identity: Option<ChainWorkIdentity>,
}

/// Work-identity facet for the chain page's work-scope dock (REQ-CHN-008):
/// the chain's worktree, branch, base branch, and — for Managed work — the
/// task. `None` on [`ChainView`] when the chain has no managed work scope.
///
/// PR health is not carried here; the dock fetches it client-side off
/// `work_conv_id`. The design rationale (why PR health rides the separate
/// PR-status pipeline and stays out of `WorkScopeInventory`) lives in
/// `specs/chains/design.md` REQ-CHN-008.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct ChainWorkIdentity {
    /// The worktree-owning member whose git metadata this is — and the
    /// conversation the UI keys PR-status off.
    pub work_conv_id: String,
    pub worktree_path: String,
    pub branch_name: String,
    pub base_branch: String,
    /// Task id + title when the chain is doing Managed (Work-mode) work; both
    /// absent for a plain Branch worktree, which carries no task.
    pub task_id: Option<String>,
    pub task_title: Option<String>,
}

/// Per-member summary on the chain page (REQ-CHN-003).
///
/// `has_worktree` is true when the member is in `Work` or `Branch` mode
/// (i.e. owns a git worktree directory on disk). The chain delete confirm
/// uses this to render an accurate worktree count without loading every
/// conversation client-side.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct ChainMemberSummary {
    pub conv_id: String,
    pub slug: Option<String>,
    pub title: Option<String>,
    pub message_count: i64,
    pub updated_at: DateTime<Utc>,
    pub position: ChainPosition,
    pub has_worktree: bool,
}

/// Where a member sits in its chain — drives the chain-page emphasis
/// on the most-recent / leaf member (REQ-CHN-003).
///
/// `Latest` is whichever member has the most-recent `updated_at`; the root
/// keeps `Root` even if it is also the most-recent (small chains where the
/// root is still the leaf are not chains, so this never overlaps in
/// practice). All other intermediate members are `Continuation`.
#[derive(Debug, Clone, Copy, Serialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum ChainPosition {
    Root,
    Continuation,
    Latest,
}

/// Body of `POST /api/chains/:rootId/qa`.
#[derive(Debug, Deserialize)]
pub struct SubmitChainQaRequest {
    pub question: String,
}

/// Response of `POST /api/chains/:rootId/qa`. The `chain_qa_id` doubles as
/// the SSE stream demux key — subscribers filter incoming events on this id
/// to render only their own question's tokens (REQ-CHN-006).
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct SubmitChainQaResponse {
    pub chain_qa_id: String,
}

/// Body of `PATCH /api/chains/:rootId/name`. `null` (`None`) clears the
/// override and falls back to the conversation's title for display.
#[derive(Debug, Deserialize)]
pub struct SetChainNameRequest {
    #[serde(default)]
    pub name: Option<String>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `GET /api/chains/:rootId`
pub async fn get_chain(
    State(state): State<AppState>,
    Path(root_id): Path<String>,
) -> Result<Json<ChainView>, AppError> {
    let view = build_chain_view(&state, &root_id).await?;
    Ok(Json(view))
}

/// `POST /api/chains/:rootId/qa`
pub async fn submit_chain_question(
    State(state): State<AppState>,
    Path(root_id): Path<String>,
    Json(req): Json<SubmitChainQaRequest>,
) -> Result<Json<SubmitChainQaResponse>, AppError> {
    let trimmed = req.question.trim();
    if trimmed.is_empty() {
        return Err(AppError::BadRequest(
            "question must not be empty or whitespace-only".to_string(),
        ));
    }

    // Validate up front so the 404 vs 400 distinction is visible to the
    // caller. `submit_question` itself would also reject, but it would
    // surface as a 500 unless we map the variant explicitly.
    validate_chain_root(&state, &root_id).await?;

    let chain_qa_id = state
        .chain_qa
        .submit_question(&root_id, trimmed)
        .await
        .map_err(map_chain_qa_error)?;
    Ok(Json(SubmitChainQaResponse { chain_qa_id }))
}

/// `PATCH /api/chains/:rootId/name`
pub async fn set_chain_name(
    State(state): State<AppState>,
    Path(root_id): Path<String>,
    Json(req): Json<SetChainNameRequest>,
) -> Result<Json<ChainView>, AppError> {
    validate_chain_root(&state, &root_id).await?;

    // Normalize: trim outer whitespace, then treat empty/whitespace-only as
    // a clear (None). This matches REQ-CHN-007: setting whitespace is
    // indistinguishable from "no name" so the wire contract collapses both
    // to a single state rather than persisting invisible names.
    let normalized = normalize_chain_name(req.name.as_deref())?;

    state
        .db
        .set_chain_name(&root_id, normalized.as_deref())
        .await
        .map_err(|e| match e {
            DbError::ConversationNotFound(_) => AppError::NotFound(format!("chain {root_id}")),
            other => AppError::Internal(other.to_string()),
        })?;

    let view = build_chain_view(&state, &root_id).await?;
    Ok(Json(view))
}

/// `POST /api/chains/:rootId/regenerate-name` (REQ-CHN-010).
///
/// Derives a prose display name by summarizing the first user message of each
/// chain member (in chain order) via a cheap LLM, then persists it as the
/// chain's `chain_name` override on the root — the same write path the typed
/// PATCH uses. Takes no request body; the chain root id in the path is the only
/// input.
///
/// Status choices:
/// - `<2` members or non-root: 404 via [`validate_chain_root`], matching every
///   other chain endpoint — a single conversation is not a chain (REQ-CHN-002).
/// - LLM failure / timeout, or no usable member messages: 500 (`AppError::Internal`).
///   The existing name is left untouched (REQ-CHN-010 — no partial/empty name
///   written). 500 matches the existing chain-Q&A LLM-failure convention
///   (`map_chain_qa_error` maps `ChainQaError::Llm` → `Internal`); a dedicated
///   502/503 variant would be a one-off in `AppError` that nothing else uses.
/// - Success: 200 with the rebuilt [`ChainView`], identical to the PATCH shape.
pub async fn regenerate_chain_name(
    State(state): State<AppState>,
    Path(root_id): Path<String>,
) -> Result<Json<ChainView>, AppError> {
    validate_chain_root(&state, &root_id).await?;

    let member_ids = state
        .db
        .chain_members_forward(&root_id)
        .await
        .map_err(db_to_app)?;

    // Collect each member's opening message in chain order, dropping members
    // that have none (a member may have only agent/tool/continuation content).
    // An opening is a user message or a skill invocation (its trigger text).
    let mut first_messages: Vec<String> = Vec::with_capacity(member_ids.len());
    for id in &member_ids {
        if let Some(text) = state
            .db
            .first_opening_message_text(id)
            .await
            .map_err(db_to_app)?
        {
            first_messages.push(text);
        }
    }

    if first_messages.is_empty() {
        // Nothing to summarize — every member lacked a usable opening message.
        // Leave the name untouched (REQ-CHN-010).
        tracing::debug!(
            root_conv_id = %root_id,
            members = member_ids.len(),
            "regenerate-name: no member had an opening message; leaving name unchanged"
        );
        return Err(AppError::Internal(
            "cannot regenerate chain name: no member has an opening message to summarize"
                .to_string(),
        ));
    }

    let (cheap_model_id, cheap_model) =
        state
            .llm_registry
            .get_cheap_model_with_id()
            .ok_or_else(|| {
                AppError::Internal(
                    "no cheap LLM model is available for name regeneration".to_string(),
                )
            })?;
    let effective_effort = state.llm_registry.effective_effort(&cheap_model_id, None);

    let Some(generated) = crate::title_generator::generate_chain_name(
        &first_messages,
        cheap_model,
        effective_effort,
        state.llm_registry.output_token_limit(&cheap_model_id),
    )
    .await
    else {
        // LLM failure/timeout — existing name stays as-is (REQ-CHN-010).
        return Err(AppError::Internal(
            "chain name regeneration failed — the existing name is unchanged".to_string(),
        ));
    };

    // Run the generated name through the same normalization the typed path uses
    // — single length authority. If it normalizes to empty (e.g. control-char
    // soup), treat as failure rather than writing an empty name.
    let normalized = normalize_chain_name(Some(&generated))?;
    if normalized.is_none() {
        return Err(AppError::Internal(
            "chain name regeneration produced an empty name — the existing name is unchanged"
                .to_string(),
        ));
    }

    state
        .db
        .set_chain_name(&root_id, normalized.as_deref())
        .await
        .map_err(|e| match e {
            DbError::ConversationNotFound(_) => AppError::NotFound(format!("chain {root_id}")),
            other => AppError::Internal(other.to_string()),
        })?;

    let view = build_chain_view(&state, &root_id).await?;
    Ok(Json(view))
}

/// Normalize a chain-name input to its persisted form (REQ-CHN-007): trim outer
/// whitespace, collapse empty/whitespace-only to `None` (a clear), and reject
/// anything over [`CHAIN_NAME_MAX_CHARS`]. This is the single length authority
/// shared by the typed PATCH and the regenerate path (REQ-CHN-010).
fn normalize_chain_name(name: Option<&str>) -> Result<Option<String>, AppError> {
    let normalized = name
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    if let Some(ref name) = normalized {
        if name.chars().count() > CHAIN_NAME_MAX_CHARS {
            return Err(AppError::BadRequest(format!(
                "chain name must be at most {CHAIN_NAME_MAX_CHARS} characters",
            )));
        }
    }
    Ok(normalized)
}

fn chain_member_blocks_cascade(state: &ConvState) -> bool {
    state.is_busy() || matches!(state, ConvState::AwaitingContinuation { .. })
}

/// `POST /api/chains/:rootId/archive` — archive every member of the chain.
/// Single-member roots are not chains; the per-conversation `/archive`
/// endpoint owns those.
///
/// Performs the same resource cleanup (bash kill, tmux kill, worktree /
/// branch removal) per member as the per-conversation archive cascade,
/// then sets `archived = 1` on each member row via `archive_conversation`.
/// Pre-checks every member's busy state up front and refuses the whole
/// operation if any member is busy (no partial cleanup).
///
/// **Not atomic.** Side effects + DB writes happen per member. If a later
/// member errors (e.g. TOCTOU-races into busy after the precheck), earlier
/// members may already be cleaned up + archived while later members are
/// untouched. The cascade itself can't be atomic (worktree/tmux kills are
/// non-transactional), and we don't wrap the per-row `archived = 1` writes
/// in a transaction either — keeping cleanup and the flag flip in lockstep
/// per member is more useful than rolling back the flag while the resources
/// are gone. Same shape as `delete_chain_handler`.
pub async fn archive_chain_handler(
    State(state): State<AppState>,
    Path(root_id): Path<String>,
) -> Result<Json<SuccessResponse>, AppError> {
    validate_chain_root(&state, &root_id).await?;

    let member_ids = state
        .db
        .chain_members_forward(&root_id)
        .await
        .map_err(db_to_app)?;
    let _admission_guards = lock_chain_admissions(&state, &member_ids).await;
    for id in &member_ids {
        super::handlers::refuse_if_coordinator(&state, id, "archive").await?;
    }

    for id in &member_ids {
        let conv = state.db.get_conversation(id).await.map_err(db_to_app)?;
        if chain_member_blocks_cascade(&conv.state) {
            return Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
                format!(
                    "Cannot archive chain: member {id} is busy. Cancel the in-flight \
                     operation first, then retry.",
                ),
                "cancel_first",
            ))));
        }
    }
    let wake_repo = state.db.wake_repository();
    for id in &member_ids {
        if wake_repo
            .has_owed_work_for_conversation(id)
            .await
            .map_err(|error| AppError::Internal(error.to_string()))?
        {
            return Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
                format!("Cannot archive chain: member {id} has pending background work."),
                "pending_wake",
            ))));
        }
    }

    for id in &member_ids {
        run_archive_cascade(&state, id).await?;
    }

    Ok(Json(SuccessResponse { success: true }))
}

async fn lock_chain_admissions(
    state: &AppState,
    member_ids: &[String],
) -> Vec<tokio::sync::OwnedMutexGuard<()>> {
    let mut sorted_ids = member_ids.to_vec();
    sorted_ids.sort();
    let mut guards = Vec::with_capacity(sorted_ids.len());
    for id in sorted_ids {
        let admission: Arc<tokio::sync::Mutex<()>> =
            state.runtime.conversation_admission(&id).await;
        guards.push(admission.lock_owned().await);
    }
    guards
}

/// `DELETE /api/chains/:rootId` — hard-delete every member of the chain.
///
/// Pre-checks every member's busy state up front and refuses the whole
/// operation if any member is busy (atomic refuse — no partial wipe).
/// Iterates root-first so the existing FK on `continued_in_conv_id`
/// (`NO ACTION`) does not reject the row delete: the root has no
/// incoming reference, and removing it frees its successor to be
/// deleted next. Reuses [`run_hard_delete_cascade`] per-member so
/// bash / tmux / worktree cleanup runs identically to the per-
/// conversation path.
pub async fn delete_chain_handler(
    State(state): State<AppState>,
    Path(root_id): Path<String>,
) -> Result<Json<SuccessResponse>, AppError> {
    validate_chain_root(&state, &root_id).await?;

    let member_ids = state
        .db
        .chain_members_forward(&root_id)
        .await
        .map_err(db_to_app)?;
    let _admission_guards = lock_chain_admissions(&state, &member_ids).await;
    for id in &member_ids {
        super::handlers::refuse_if_coordinator(&state, id, "delete").await?;
    }

    for id in &member_ids {
        let conv = state.db.get_conversation(id).await.map_err(db_to_app)?;
        if chain_member_blocks_cascade(&conv.state) {
            return Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
                format!(
                    "Cannot delete chain: member {id} is busy. Cancel the in-flight \
                     operation first, then retry.",
                ),
                "cancel_first",
            ))));
        }
    }
    let wake_repo = state.db.wake_repository();
    for id in &member_ids {
        if wake_repo
            .has_owed_work_for_conversation(id)
            .await
            .map_err(|error| AppError::Internal(error.to_string()))?
        {
            return Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
                format!("Cannot delete chain: member {id} has pending background work."),
                "pending_wake",
            ))));
        }
    }

    // TOCTOU note: the busy precheck is best-effort. A member can transition
    // to busy after this loop and before/during its individual cascade, in
    // which case `run_hard_delete_cascade` returns a 409 mid-iteration with
    // earlier members already deleted. Same shape as per-conversation delete
    // (which has no precheck at all). A locking mitigation belongs in a
    // future task — not in this PR.
    for id in &member_ids {
        run_hard_delete_cascade(&state, id).await?;
    }

    Ok(Json(SuccessResponse { success: true }))
}

/// `GET /api/chains/:rootId/stream`
pub async fn stream_chain(
    State(state): State<AppState>,
    Path(root_id): Path<String>,
) -> Result<axum::response::Response, AppError> {
    validate_chain_root(&state, &root_id).await?;

    let runtime = state
        .chain_qa
        .runtime_registry()
        .get_or_create(&root_id)
        .await;
    let (rx, guard) = runtime.subscribe();

    // Move the subscriber guard into the per-event closure so it lives as
    // long as the stream itself. When the client disconnects, the
    // `BroadcastStream` drops, dropping the guard, decrementing the
    // subscriber counter; the next `release_if_idle` then clears the
    // runtime if no Q&A is in flight (Phase 3 lifecycle contract).
    let mut guard_holder: Option<crate::chain_runtime::ChainSubscriberGuard> = Some(guard);

    let stream = BroadcastStream::new(rx)
        .take_while({
            let root_for_log = root_id.clone();
            move |result| {
                if let Err(BroadcastStreamRecvError::Lagged(n)) = result {
                    tracing::warn!(
                        root_conv_id = %root_for_log,
                        lagged_by = n,
                        "chain SSE broadcast lagged; closing stream so client reconnects",
                    );
                    false
                } else {
                    true
                }
            }
        })
        .filter_map(move |result| {
            let Ok(event) = result else {
                // The take_while above turned Lagged into stream
                // completion; any other Err here would be unreachable but
                // we still drop the subscriber guard to be safe.
                guard_holder.take();
                return None;
            };
            let wire: ChainSseWireEvent = event.into();
            let event_type = wire.event_type();
            let data =
                serde_json::to_string(&wire).expect("ChainSseWireEvent is always serializable");
            Some(Ok::<Event, Infallible>(
                Event::default().event(event_type).data(data),
            ))
        });

    let sse = Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("ping"),
    );

    let mut headers = HeaderMap::new();
    headers.insert("x-accel-buffering", HeaderValue::from_static("no"));
    Ok((headers, sse).into_response())
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

/// Validate that `root_id` names a chain root with at least 2 members.
///
/// Mirrors the check in `ChainQa::prepare_invocation` so failures are
/// surfaced as 404 here instead of bubbling up as 500 from the Q&A backend.
async fn validate_chain_root(state: &AppState, root_id: &str) -> Result<(), AppError> {
    if let Some(coordinator_id) = state
        .db
        .coordinator_conversation_id()
        .await
        .map_err(db_to_app)?
    {
        let coordinator_root = state
            .db
            .chain_root_of(&coordinator_id)
            .await
            .map_err(db_to_app)?
            .unwrap_or(coordinator_id);
        if coordinator_root == root_id {
            return Err(AppError::NotFound(format!("no chain rooted at {root_id}")));
        }
    }

    // chain_root_of returns None when the conversation does not exist; the
    // caller can't tell apart "no such conv" from "this conv is a member,
    // not a root" — both map to 404 from the chain API's perspective.
    let root = state.db.chain_root_of(root_id).await.map_err(db_to_app)?;
    if root.as_deref() != Some(root_id) {
        return Err(AppError::NotFound(format!("no chain rooted at {root_id}",)));
    }
    let members = state
        .db
        .chain_members_forward(root_id)
        .await
        .map_err(db_to_app)?;
    if members.len() < 2 {
        return Err(AppError::NotFound(format!("no chain rooted at {root_id}",)));
    }
    Ok(())
}

async fn build_chain_view(state: &AppState, root_id: &str) -> Result<ChainView, AppError> {
    validate_chain_root(state, root_id).await?;

    let members: Vec<Conversation> = state
        .db
        .chain_members_forward_full(root_id)
        .await
        .map_err(db_to_app)?;

    let root_conv = members
        .first()
        .ok_or_else(|| AppError::Internal("chain validation passed but members empty".to_string()))?
        .clone();

    let qa_history = state
        .chain_qa
        .list_history(root_id)
        .await
        .map_err(map_chain_qa_error)?;

    let current_member_count = i64::try_from(members.len()).unwrap_or(i64::MAX);
    let current_total_messages: i64 = members.iter().map(|c| c.message_count).sum();

    let summaries = build_member_summaries(&members);
    let display_name = resolve_display_name(&root_conv);
    let work_identity = resolve_work_identity(&members);

    Ok(ChainView {
        root_conv_id: root_conv.id.clone(),
        chain_name: root_conv.chain_name.clone(),
        display_name,
        archived: root_conv.archived,
        members: summaries,
        qa_history,
        current_member_count,
        current_total_messages,
        work_identity,
    })
}

/// Resolve the chain's work identity from its worktree-owning member
/// (REQ-CHN-008).
///
/// A chain's members share one work scope (`specs/projects/` REQ-PROJ-025), so
/// the latest member that owns a branch worktree is authoritative. Work and
/// Branch modes both carry `branch_name`; managed Explore has a worktree but no
/// branch identity, and Direct has neither, so neither qualifies. Returns `None`
/// when no member owns a branch worktree (e.g. a chain of Direct conversations)
/// — the dock then indicates "no managed work scope" rather than empty fields.
fn resolve_work_identity(members: &[Conversation]) -> Option<ChainWorkIdentity> {
    let work_member = members
        .iter()
        .rev()
        .find(|c| c.conv_mode.branch_name().is_some())?;
    let mode = &work_member.conv_mode;
    Some(ChainWorkIdentity {
        work_conv_id: work_member.id.clone(),
        worktree_path: mode.worktree_path()?.to_string(),
        branch_name: mode.branch_name()?.to_string(),
        base_branch: mode.base_branch()?.to_string(),
        task_id: mode.task_id().map(str::to_string),
        task_title: mode.task_title().map(str::to_string),
    })
}

/// Build per-member summaries with the `Latest` badge applied to whichever
/// non-root member has the largest `updated_at` value.
fn build_member_summaries(members: &[Conversation]) -> Vec<ChainMemberSummary> {
    // Identify the latest non-root member by `updated_at`. Tie-breaker on
    // chain order means the *last-positioned* member wins, since
    // `iter().enumerate()` later in the chain replaces an earlier tie. The
    // root is excluded so it always renders as `Root`.
    let latest_idx = members
        .iter()
        .enumerate()
        .skip(1)
        .max_by(|a, b| {
            a.1.updated_at
                .cmp(&b.1.updated_at)
                .then_with(|| a.0.cmp(&b.0))
        })
        .map(|(i, _)| i);

    members
        .iter()
        .enumerate()
        .map(|(i, conv)| {
            let position = if i == 0 {
                ChainPosition::Root
            } else if Some(i) == latest_idx {
                ChainPosition::Latest
            } else {
                ChainPosition::Continuation
            };
            ChainMemberSummary {
                conv_id: conv.id.clone(),
                slug: conv.slug.clone(),
                title: conv.title.clone(),
                message_count: conv.message_count,
                updated_at: conv.updated_at,
                position,
                has_worktree: conv.conv_mode.worktree_path().is_some(),
            }
        })
        .collect()
}

/// Resolve the user-visible chain name: explicit `chain_name` if set,
/// else the conversation title, else the slug, else the bare id.
fn resolve_display_name(root: &Conversation) -> String {
    if let Some(name) = root.chain_name.as_deref() {
        if !name.is_empty() {
            return name.to_string();
        }
    }
    if let Some(title) = root.title.as_deref() {
        if !title.is_empty() {
            return title.to_string();
        }
    }
    if let Some(slug) = root.slug.as_deref() {
        if !slug.is_empty() {
            return slug.to_string();
        }
    }
    root.id.clone()
}

fn db_to_app(e: DbError) -> AppError {
    match e {
        DbError::ConversationNotFound(id) => AppError::NotFound(id),
        other => AppError::Internal(other.to_string()),
    }
}

fn map_chain_qa_error(e: ChainQaError) -> AppError {
    match e {
        ChainQaError::NotAChainRoot(id) => AppError::NotFound(format!("no chain rooted at {id}")),
        ChainQaError::Db(DbError::ConversationNotFound(id)) => {
            AppError::NotFound(format!("conversation {id} not found"))
        }
        ChainQaError::Db(other) => AppError::Internal(other.to_string()),
        ChainQaError::Llm(msg) => AppError::Internal(format!("LLM error: {msg}")),
        ChainQaError::NoModelAvailable => AppError::Internal(
            "no mid-tier LLM model is available — chain Q&A is disabled".to_string(),
        ),
    }
}
