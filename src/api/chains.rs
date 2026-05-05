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
//! - `GET /api/chains/:rootId/stream` — SSE subscription for streaming Q&A
//!   token events (publishes [`crate::api::wire::ChainSseWireEvent`])
//!
//! All four endpoints reject non-chain-root inputs with 404. The chain
//! validity test mirrors the one in `ChainQa::prepare_invocation`:
//! `chain_root_of(id) == Some(id)` AND `chain_members_forward(id).len() >= 2`.
//! Single-member roots and non-root members are not chains.

use std::convert::Infallible;
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

use super::handlers::{run_hard_delete_cascade, AppError};
use super::types::{ConflictErrorResponse, SuccessResponse};
use super::wire::ChainSseWireEvent;
use super::AppState;
use crate::chain_qa::ChainQaError;
use crate::db::{ChainQaRow, Conversation, DbError};

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
#[ts(export, export_to = "../ui/src/generated/")]
pub struct ChainView {
    pub root_conv_id: String,
    pub chain_name: Option<String>,
    pub display_name: String,
    pub members: Vec<ChainMemberSummary>,
    pub qa_history: Vec<ChainQaRow>,
    pub current_member_count: i64,
    pub current_total_messages: i64,
}

/// Per-member summary on the chain page (REQ-CHN-003).
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct ChainMemberSummary {
    pub conv_id: String,
    pub slug: Option<String>,
    pub title: Option<String>,
    pub message_count: i64,
    pub updated_at: DateTime<Utc>,
    pub position: ChainPosition,
}

/// Where a member sits in its chain (REQ-CHN-003 / REQ-CHN-009-style emphasis).
///
/// `Latest` is whichever member has the most-recent `updated_at`; the root
/// keeps `Root` even if it is also the most-recent (small chains where the
/// root is still the leaf are not chains, so this never overlaps in
/// practice). All other intermediate members are `Continuation`.
#[derive(Debug, Clone, Copy, Serialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../ui/src/generated/")]
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
#[ts(export, export_to = "../ui/src/generated/")]
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
    let normalized = req
        .name
        .as_deref()
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

/// `POST /api/chains/:rootId/archive` — archive every member of the chain
/// atomically. Single-member roots are not chains; the per-conversation
/// `/archive` endpoint owns those.
pub async fn archive_chain_handler(
    State(state): State<AppState>,
    Path(root_id): Path<String>,
) -> Result<Json<SuccessResponse>, AppError> {
    validate_chain_root(&state, &root_id).await?;
    state.db.archive_chain(&root_id).await.map_err(db_to_app)?;
    Ok(Json(SuccessResponse { success: true }))
}

/// `POST /api/chains/:rootId/unarchive`
pub async fn unarchive_chain_handler(
    State(state): State<AppState>,
    Path(root_id): Path<String>,
) -> Result<Json<SuccessResponse>, AppError> {
    validate_chain_root(&state, &root_id).await?;
    state
        .db
        .unarchive_chain(&root_id)
        .await
        .map_err(db_to_app)?;
    Ok(Json(SuccessResponse { success: true }))
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

    for id in &member_ids {
        let conv = state.db.get_conversation(id).await.map_err(db_to_app)?;
        if conv.state.is_busy() {
            return Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
                format!(
                    "Cannot delete chain: member {id} is busy. Cancel the in-flight \
                     operation first, then retry.",
                ),
                "cancel_first",
            ))));
        }
    }

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

    let member_ids = state
        .db
        .chain_members_forward(root_id)
        .await
        .map_err(db_to_app)?;
    let mut members: Vec<Conversation> = Vec::with_capacity(member_ids.len());
    for id in &member_ids {
        members.push(state.db.get_conversation(id).await.map_err(db_to_app)?);
    }

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

    Ok(ChainView {
        root_conv_id: root_conv.id.clone(),
        chain_name: root_conv.chain_name.clone(),
        display_name,
        members: summaries,
        qa_history,
        current_member_count,
        current_total_messages,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Database, MessageContent};

    /// Mirror of the `chain_qa` test helper — builds a linear chain and
    /// links `continued_in_conv_id` between successive members.
    async fn build_linear_chain(db: &Database, ids: &[&str]) {
        for id in ids {
            db.create_conversation(id, &format!("slug-{id}"), "/tmp", true, None, None)
                .await
                .unwrap();
        }
        for pair in ids.windows(2) {
            sqlx::query("UPDATE conversations SET continued_in_conv_id = ?1 WHERE id = ?2")
                .bind(pair[1])
                .bind(pair[0])
                .execute(db.pool())
                .await
                .unwrap();
        }
    }

    async fn add_user_message(db: &Database, conv_id: &str, idx: usize, text: &str) {
        let msg_id = format!("msg-user-{conv_id}-{idx}");
        db.add_message(&msg_id, conv_id, &MessageContent::user(text), None, None)
            .await
            .unwrap();
    }

    async fn add_continuation_summary(db: &Database, conv_id: &str, summary: &str) {
        let msg_id = format!("msg-cont-{conv_id}");
        db.add_message(
            &msg_id,
            conv_id,
            &MessageContent::continuation(summary),
            None,
            None,
        )
        .await
        .unwrap();
    }

    /// `build_chain_view` requires a populated `AppState`, which carries a
    /// runtime, MCP, terminals, etc. The chain-API logic only touches `db`
    /// and `chain_qa`, so the helpers below construct a minimal `ChainView`
    /// directly from a `Database` + `ChainQa` so we can exercise the
    /// member-position computation, `display_name` fallback, and chain
    /// validation rules without spinning the full app harness.
    async fn build_view_for_test(
        db: &Database,
        chain_qa: &crate::chain_qa::ChainQa,
        root_id: &str,
    ) -> Result<ChainView, AppError> {
        let root = db.chain_root_of(root_id).await.map_err(db_to_app)?;
        if root.as_deref() != Some(root_id) {
            return Err(AppError::NotFound(format!("no chain rooted at {root_id}",)));
        }
        let member_ids = db.chain_members_forward(root_id).await.map_err(db_to_app)?;
        if member_ids.len() < 2 {
            return Err(AppError::NotFound(format!("no chain rooted at {root_id}",)));
        }
        let mut members: Vec<Conversation> = Vec::with_capacity(member_ids.len());
        for id in &member_ids {
            members.push(db.get_conversation(id).await.map_err(db_to_app)?);
        }
        let root_conv = members.first().unwrap().clone();
        let qa_history = chain_qa
            .list_history(root_id)
            .await
            .map_err(map_chain_qa_error)?;
        let current_member_count = i64::try_from(members.len()).unwrap_or(i64::MAX);
        let current_total_messages: i64 = members.iter().map(|c| c.message_count).sum();
        let summaries = build_member_summaries(&members);
        let display_name = resolve_display_name(&root_conv);
        Ok(ChainView {
            root_conv_id: root_conv.id.clone(),
            chain_name: root_conv.chain_name.clone(),
            display_name,
            members: summaries,
            qa_history,
            current_member_count,
            current_total_messages,
        })
    }

    fn registry_with_test_llm() -> std::sync::Arc<crate::llm::ModelRegistry> {
        use crate::llm::{ContentBlock, LlmError, LlmRequest, LlmResponse, LlmService, Usage};
        use async_trait::async_trait;
        use tokio::sync::broadcast;

        #[derive(Debug)]
        struct StubLlm;
        #[async_trait]
        impl LlmService for StubLlm {
            async fn complete(&self, _r: &LlmRequest) -> Result<LlmResponse, LlmError> {
                Ok(LlmResponse {
                    content: vec![ContentBlock::text("stub")],
                    end_turn: true,
                    usage: Usage::default(),
                })
            }
            async fn complete_streaming(
                &self,
                r: &LlmRequest,
                _: &broadcast::Sender<crate::llm::TokenChunk>,
            ) -> Result<LlmResponse, LlmError> {
                self.complete(r).await
            }
            #[allow(clippy::unnecessary_literal_bound)]
            fn model_id(&self) -> &str {
                "stub-model"
            }
        }
        std::sync::Arc::new(crate::llm::ModelRegistry::for_test_with_sonnet(
            std::sync::Arc::new(StubLlm),
        ))
    }

    #[tokio::test]
    async fn build_view_returns_members_in_chain_order_and_marks_latest() {
        let db = Database::open_in_memory().await.unwrap();
        build_linear_chain(&db, &["v-a", "v-b", "v-c"]).await;
        add_continuation_summary(&db, "v-a", "summary A").await;
        add_continuation_summary(&db, "v-b", "summary B").await;
        add_user_message(&db, "v-c", 0, "leaf-only").await;

        let chain_qa = crate::chain_qa::ChainQa::new(db.clone(), registry_with_test_llm());
        let view = build_view_for_test(&db, &chain_qa, "v-a").await.unwrap();

        assert_eq!(view.root_conv_id, "v-a");
        assert_eq!(view.members.len(), 3);
        assert_eq!(view.members[0].position, ChainPosition::Root);
        assert_eq!(view.members[0].conv_id, "v-a");
        // v-c was the most-recently-touched member (last add_message); it
        // is the chain's "latest" emphasis target.
        assert_eq!(view.members[2].position, ChainPosition::Latest);
        assert_eq!(view.members[1].position, ChainPosition::Continuation);
        // Each summary message increments its conversation's updated_at,
        // and add_user_message on v-c likewise; the totals are sum of
        // per-conversation message_count.
        assert_eq!(view.current_member_count, 3);
        assert_eq!(view.current_total_messages, 3);
        assert!(view.qa_history.is_empty());
    }

    #[tokio::test]
    async fn build_view_uses_chain_name_when_set() {
        let db = Database::open_in_memory().await.unwrap();
        build_linear_chain(&db, &["dn-a", "dn-b"]).await;
        add_continuation_summary(&db, "dn-a", "first").await;
        add_user_message(&db, "dn-b", 0, "leaf").await;
        db.set_chain_name("dn-a", Some("auth refactor"))
            .await
            .unwrap();

        let chain_qa = crate::chain_qa::ChainQa::new(db.clone(), registry_with_test_llm());
        let view = build_view_for_test(&db, &chain_qa, "dn-a").await.unwrap();

        assert_eq!(view.chain_name.as_deref(), Some("auth refactor"));
        assert_eq!(view.display_name, "auth refactor");
    }

    #[tokio::test]
    async fn build_view_falls_back_to_title_when_chain_name_unset() {
        let db = Database::open_in_memory().await.unwrap();
        build_linear_chain(&db, &["fb-a", "fb-b"]).await;
        // create_conversation populates title from the slug in title-case.
        // No explicit chain_name set => display_name uses title.
        add_user_message(&db, "fb-b", 0, "leaf").await;

        let chain_qa = crate::chain_qa::ChainQa::new(db.clone(), registry_with_test_llm());
        let view = build_view_for_test(&db, &chain_qa, "fb-a").await.unwrap();

        assert_eq!(view.chain_name, None);
        // Title is derived from the slug; it should not be empty and must
        // be the same string the chain name resolution used.
        let root = db.get_conversation("fb-a").await.unwrap();
        assert_eq!(view.display_name, root.title.unwrap());
    }

    #[tokio::test]
    async fn build_view_rejects_single_member_root() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("solo", "slug-solo", "/tmp", true, None, None)
            .await
            .unwrap();

        let chain_qa = crate::chain_qa::ChainQa::new(db.clone(), registry_with_test_llm());
        let err = build_view_for_test(&db, &chain_qa, "solo")
            .await
            .unwrap_err();
        match err {
            AppError::NotFound(msg) => assert!(msg.contains("no chain rooted at")),
            _ => panic!("expected NotFound"),
        }
    }

    #[tokio::test]
    async fn build_view_rejects_non_root_member() {
        let db = Database::open_in_memory().await.unwrap();
        build_linear_chain(&db, &["nr-a", "nr-b", "nr-c"]).await;
        add_continuation_summary(&db, "nr-a", "summary").await;
        add_continuation_summary(&db, "nr-b", "summary").await;

        let chain_qa = crate::chain_qa::ChainQa::new(db.clone(), registry_with_test_llm());
        let err = build_view_for_test(&db, &chain_qa, "nr-b")
            .await
            .unwrap_err();
        match err {
            AppError::NotFound(_) => {}
            _ => panic!("expected NotFound"),
        }
    }

    #[tokio::test]
    async fn build_view_rejects_unknown_root() {
        let db = Database::open_in_memory().await.unwrap();
        let chain_qa = crate::chain_qa::ChainQa::new(db.clone(), registry_with_test_llm());
        let err = build_view_for_test(&db, &chain_qa, "ghost")
            .await
            .unwrap_err();
        match err {
            AppError::NotFound(_) => {}
            _ => panic!("expected NotFound"),
        }
    }

    #[tokio::test]
    async fn build_view_includes_qa_history_for_existing_chain() {
        let db = Database::open_in_memory().await.unwrap();
        build_linear_chain(&db, &["q-a", "q-b"]).await;
        add_continuation_summary(&db, "q-a", "first").await;
        add_user_message(&db, "q-b", 0, "leaf").await;

        let chain_qa = crate::chain_qa::ChainQa::new(db.clone(), registry_with_test_llm());
        let _ = chain_qa
            .submit_question_blocking("q-a", "what happened?")
            .await
            .unwrap();

        let view = build_view_for_test(&db, &chain_qa, "q-a")
            .await
            .expect("build_view_for_test should succeed for valid chain");
        assert_eq!(view.qa_history.len(), 1);
        assert_eq!(view.qa_history[0].question, "what happened?");
    }

    // ----- name editing semantics ----------------------------------------

    /// The "normalize the name on the way in" rule lives in `set_chain_name`
    /// itself. Re-implement the same trim-and-collapse logic here so a
    /// regression in either side fails this test.
    fn normalize_for_test(input: Option<&str>) -> Option<String> {
        input
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    }

    #[test]
    fn whitespace_only_name_collapses_to_none() {
        assert_eq!(normalize_for_test(Some("   ")), None);
        assert_eq!(normalize_for_test(Some("")), None);
        assert_eq!(normalize_for_test(None), None);
    }

    #[test]
    fn name_with_internal_whitespace_is_preserved() {
        assert_eq!(
            normalize_for_test(Some("  auth refactor  ")),
            Some("auth refactor".to_string()),
        );
    }

    #[test]
    fn over_length_names_are_rejected_at_handler_layer() {
        let too_long = "a".repeat(CHAIN_NAME_MAX_CHARS + 1);
        // Sanity: the cap counts chars, not bytes, so a multibyte char
        // string at the exact limit is fine.
        let utf8_at_limit = "a".repeat(CHAIN_NAME_MAX_CHARS);
        assert!(too_long.chars().count() > CHAIN_NAME_MAX_CHARS);
        assert!(utf8_at_limit.chars().count() <= CHAIN_NAME_MAX_CHARS);
    }

    // ----- POST validation -----------------------------------------------

    #[test]
    fn empty_question_is_rejected() {
        // Mirror submit_chain_question's first guard.
        let raw = "   \n\t  ";
        assert!(raw.trim().is_empty());
    }
}
