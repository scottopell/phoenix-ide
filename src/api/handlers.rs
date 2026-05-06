//! HTTP request handlers
//!
//! REQ-API-001 through REQ-API-010

use super::assets::{get_index_html, serve_favicon, serve_service_worker, serve_static};
use super::chains::{
    archive_chain_handler, delete_chain_handler, get_chain, set_chain_name, stream_chain,
    submit_chain_question, unarchive_chain_handler,
};
use super::git_handlers::{get_conversation_diff, list_git_branches};
use super::lifecycle_handlers::{
    abandon_task, approve_task, mark_merged, reject_task, task_feedback,
};
use super::sse::sse_stream;
use super::types::{
    CancelResponse, ChatRequest, ChatResponse, ConflictErrorResponse, ContinueConversationResponse,
    ConversationListResponse, ConversationResponse, ConversationWithMessagesResponse,
    CreateConversationRequest, CredentialStatusApi, DirectoryEntry, ErrorResponse,
    ExpansionErrorResponse, FileEntry, FileSearchEntry, FileSearchQuery, FileSearchResponse,
    GatewayStatusApi, ListDirectoryResponse, ListFilesResponse, MkdirResponse, ModelsResponse,
    ReadFileResponse, RenameRequest, SkillEntry, SkillsResponse, SuccessResponse,
    SystemPromptResponse, TaskEntry, TasksResponse, UpgradeModelRequest, ValidateCwdResponse,
};
use super::AppState;
use crate::db::{ConvMode, ConversationUsage, ImageData, Message, MessageContent, MessageType};
use crate::git_ops::{
    check_branch_conflict, create_worktree, effective_base_ref, materialize_branch, run_git,
    BranchConflict, GitOpError,
};
use crate::llm::{ContentBlock, GatewayStatus};
use crate::runtime::SseEvent;
use crate::state_machine::{ConvState, Event};
use crate::terminal::terminal_ws_handler;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    middleware,
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::Datelike;
use chrono::{Local, Timelike};
use rand::seq::SliceRandom;
use serde::Deserialize;
use serde_json::Value;
use std::fs;
use std::path::PathBuf;

/// Create the API router
pub fn create_router(state: AppState) -> Router {
    Router::new()
        // Root serves the SPA
        .route("/", get(serve_spa))
        // Deep links to conversations
        .route("/c/:slug", get(serve_spa))
        // New conversation page
        .route("/new", get(serve_spa))
        // Service worker
        .route("/service-worker.js", get(serve_service_worker))
        // Favicon (referenced from index.html)
        .route("/phoenix.svg", get(serve_favicon))
        // Static assets (embedded or filesystem fallback)
        .route("/assets/*path", get(serve_static))
        // Preview: serves files from absolute paths so relative references work
        .route("/preview/*filepath", get(serve_preview_file))
        // Conversation listing (REQ-API-001)
        .route("/api/conversations", get(list_conversations))
        .route(
            "/api/conversations/archived",
            get(list_archived_conversations),
        )
        // Conversation creation (REQ-API-002)
        .route("/api/conversations/new", post(create_conversation))
        // Conversation retrieval (REQ-API-003)
        .route("/api/conversations/:id", get(get_conversation))
        .route("/api/conversations/:id/slug", get(get_conversation_slug))
        // SSE streaming (REQ-API-005)
        .route("/api/conversations/:id/stream", get(stream_conversation))
        // Terminal WebSocket (REQ-TERM-001 through REQ-TERM-014)
        .route("/api/conversations/:id/terminal", get(terminal_ws_handler))
        // User actions (REQ-API-004)
        .route("/api/conversations/:id/chat", post(send_chat))
        .route("/api/conversations/:id/cancel", post(cancel_conversation))
        .route(
            "/api/conversations/:id/trigger-continuation",
            post(trigger_continuation),
        )
        // Context continuation worktree transfer (REQ-BED-030)
        .route(
            "/api/conversations/:id/continue",
            post(continue_conversation),
        )
        // Task approval (REQ-BED-028)
        .route("/api/conversations/:id/approve-task", post(approve_task))
        .route("/api/conversations/:id/reject-task", post(reject_task))
        .route("/api/conversations/:id/task-feedback", post(task_feedback))
        // User question response (REQ-AUQ-003)
        .route("/api/conversations/:id/respond", post(respond_to_question))
        // Task abandon (REQ-PROJ-010)
        .route("/api/conversations/:id/abandon-task", post(abandon_task))
        // Mark as merged (REQ-PROJ-026)
        .route("/api/conversations/:id/mark-merged", post(mark_merged))
        // Lifecycle (REQ-API-006)
        .route("/api/conversations/:id/archive", post(archive_conversation))
        .route(
            "/api/conversations/:id/unarchive",
            post(unarchive_conversation),
        )
        .route("/api/conversations/:id/delete", post(delete_conversation))
        .route("/api/conversations/:id/rename", post(rename_conversation))
        // Token usage (Phase 4)
        .route(
            "/api/conversations/:id/usage",
            get(get_conversation_usage_handler),
        )
        // System prompt inspection
        .route(
            "/api/conversations/:id/system-prompt",
            get(get_system_prompt),
        )
        // Slug resolution (REQ-API-007)
        .route("/api/conversations/by-slug/:slug", get(get_by_slug))
        // Phoenix Chains v1 (REQ-CHN-003 / 004 / 005 / 007)
        .route("/api/chains/:rootId", get(get_chain))
        .route("/api/chains/:rootId/qa", post(submit_chain_question))
        .route(
            "/api/chains/:rootId/name",
            axum::routing::patch(set_chain_name),
        )
        .route("/api/chains/:rootId/stream", get(stream_chain))
        .route("/api/chains/:rootId/archive", post(archive_chain_handler))
        .route(
            "/api/chains/:rootId/unarchive",
            post(unarchive_chain_handler),
        )
        .route(
            "/api/chains/:rootId",
            axum::routing::delete(delete_chain_handler),
        )
        // Directory browser (REQ-API-008)
        .route("/api/validate-cwd", get(validate_cwd))
        .route("/api/list-directory", get(list_directory))
        .route("/api/mkdir", post(mkdir))
        // File browser API (REQ-PF-001 through REQ-PF-004)
        .route("/api/files/list", get(list_files))
        .route("/api/files/read", get(read_file))
        .route(
            "/api/conversations/:id/files/search",
            get(search_conversation_files),
        )
        // Skill discovery for autocomplete (REQ-IR-005)
        .route(
            "/api/conversations/:id/skills",
            get(list_conversation_skills),
        )
        // Task listing
        .route("/api/conversations/:id/tasks", get(list_conversation_tasks))
        // Projects (REQ-PROJ-014)
        .route("/api/projects", get(list_projects))
        // Model info (REQ-API-009)
        .route("/api/models", get(list_models))
        // Interactive credential helper (REQ-CREDHELPER-003)
        .route("/api/credential-helper/run", get(run_credential_helper))
        .route(
            "/api/credential-helper/invalidate",
            post(invalidate_credential),
        )
        .route(
            "/api/conversations/:id/upgrade-model",
            post(upgrade_conversation_model),
        )
        // Per-conversation worktree diff (Work/Branch-mode "View diff" action)
        .route("/api/conversations/:id/diff", get(get_conversation_diff))
        // Git utilities
        .route("/api/git/branches", get(list_git_branches))
        // Environment info
        .route("/api/env", get(get_env))
        // MCP management
        .route("/api/mcp/status", get(mcp_status))
        .route("/api/mcp/reload", post(reload_mcp))
        .route("/api/mcp/servers/:name/disable", post(disable_mcp_server))
        .route("/api/mcp/servers/:name/enable", post(enable_mcp_server))
        // Version
        .route("/version", get(get_version))
        // Auth endpoints (REQ-AUTH-002, REQ-AUTH-003)
        .route("/api/auth/status", get(super::auth::auth_status))
        .route("/api/auth/login", post(super::auth::auth_login))
        // Share mode (REQ-AUTH-004 through REQ-AUTH-008)
        .route("/share/c/:slug", get(create_or_redirect_share))
        .route("/s/:token", get(serve_share_page))
        .route(
            "/api/share/:token/conversation",
            get(get_shared_conversation),
        )
        .route("/api/share/:token/events", get(shared_sse_stream))
        // Auth middleware — runs before all route handlers (REQ-AUTH-001)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            super::auth::auth_middleware,
        ))
        .with_state(state)
}

// ============================================================
// Message Transformation
// ============================================================

/// Transform a message for API output by merging `display_data` into content blocks.
///
/// For agent messages with bash `tool_use` blocks, the `display` field shows a
/// simplified command (with cd prefixes stripped when they match cwd).
/// The `display_data` is pre-computed at message creation time and stored in DB.
///
/// This helper exists for non-SSE REST endpoints (conversation fetch, archived
/// list, etc.). The SSE path goes through [`crate::api::wire::EnrichedMessage`]
/// directly; both routes produce byte-for-byte identical output — there's a
/// parity test for every `SseEvent` variant in `src/api/sse.rs`.
pub(crate) fn enrich_message_for_api(msg: &Message) -> Value {
    let enriched = super::wire::EnrichedMessage::from(msg);
    serde_json::to_value(&enriched).unwrap_or(Value::Null)
}

/// Count how many commits `base_branch` is ahead of `task_branch` in `repo_root`.
///
/// Compares against `origin/<base>` when the remote-tracking ref exists
/// (kept fresh by the periodic fetch loop in `stream_conversation`),
/// falling back to bare `<base>` for local-only repos. See task 13001.
///
/// Shells out to `git rev-list --count`. Returns 0 on any error (missing branch,
/// git not available, parse failure). This is a best-effort indicator.
///
/// **Blocking** -- must be called from `spawn_blocking` or an already-blocking context.
fn commits_behind(repo_root: &std::path::Path, base_branch: &str, task_branch: &str) -> u32 {
    let comparator = effective_base_ref(repo_root, base_branch);
    let range = format!("{task_branch}..{comparator}");
    match run_git(repo_root, &["rev-list", "--count", &range]) {
        Ok(output) => output.trim().parse::<u32>().unwrap_or(0),
        Err(e) => {
            tracing::debug!(
                repo = %repo_root.display(),
                base_branch,
                task_branch,
                error = %e,
                "commits_behind check failed, returning 0"
            );
            0
        }
    }
}

/// How many commits the task branch is ahead of the base branch.
///
/// Same `origin/<base>` preference as `commits_behind`; see task 13001.
///
/// Shells out to `git rev-list --count`. Returns 0 on any error.
///
/// **Blocking** -- must be called from `spawn_blocking` or an already-blocking context.
fn commits_ahead(repo_root: &std::path::Path, base_branch: &str, task_branch: &str) -> u32 {
    let comparator = effective_base_ref(repo_root, base_branch);
    let range = format!("{comparator}..{task_branch}");
    match run_git(repo_root, &["rev-list", "--count", &range]) {
        Ok(output) => output.trim().parse::<u32>().unwrap_or(0),
        Err(e) => {
            tracing::debug!(
                repo = %repo_root.display(),
                base_branch,
                task_branch,
                error = %e,
                "commits_ahead check failed, returning 0"
            );
            0
        }
    }
}

/// Merge pre-computed `display_data` into content blocks.
///
/// `display_data` format: `{ "bash": [{ "tool_use_id": "...", "display": "..." }] }`
/// Build an `EnrichedConversation` with derived display fields.
///
/// Note: `seed_parent_slug` is left as `None` here. Call sites that need to
/// render the seed breadcrumb (single-conversation fetch, SSE init) should
/// use [`enrich_conversation_with_seed`] instead to resolve the parent slug.
fn enrich_conversation(conv: &crate::db::Conversation) -> crate::runtime::EnrichedConversation {
    crate::runtime::EnrichedConversation {
        conv_mode_label: conv.conv_mode.label().to_string(),
        branch_name: conv.conv_mode.branch_name().map(String::from),
        worktree_path: conv
            .conv_mode
            .worktree_path()
            .filter(|s| !s.is_empty() && !s.starts_with("__LEGACY"))
            .map(String::from),
        base_branch: conv
            .conv_mode
            .base_branch()
            .filter(|s| !s.is_empty() && !s.starts_with("__LEGACY"))
            .map(String::from),
        task_title: conv.conv_mode.task_title().map(String::from),
        // REQ-TERM-002 / REQ-TERM-017: surface the server-user's $SHELL so
        // the frontend can tailor the OSC 133 enablement snippet. The PTY
        // spawn path reads `$SHELL` from the same env, so this matches what
        // the user's shell will actually be.
        shell: std::env::var("SHELL").ok(),
        // REQ-SEED-*: surface $HOME so the UI can spawn a seeded conversation
        // scoped to the user's home directory (e.g. for shell integration
        // setup).
        home_dir: std::env::var("HOME").ok(),
        seed_parent_slug: None,
        inner: conv.clone(),
    }
}

/// Build an `EnrichedConversation` and resolve `seed_parent_slug` (REQ-SEED-003).
///
/// If the conversation has a seed parent and the parent still exists, the
/// parent's slug is set so the UI can render a clickable breadcrumb. If the
/// parent has been deleted the slug stays `None` and the UI renders unlinked
/// text per REQ-SEED-003.
async fn enrich_conversation_with_seed(
    state: &AppState,
    conv: &crate::db::Conversation,
) -> crate::runtime::EnrichedConversation {
    let mut enriched = enrich_conversation(conv);
    if let Some(parent_id) = conv.seed_parent_id.as_deref() {
        if let Ok(parent) = state.runtime.db().get_conversation(parent_id).await {
            enriched.seed_parent_slug = parent.slug;
        }
    }
    enriched
}

/// Serialize a conversation to JSON with `display_state` included.
///
/// Used by endpoints that return `serde_json::Value` (conversation list, etc.).
/// `display_state` is injected here (not on `EnrichedConversation`) so REST
/// clients still receive it while the typed struct stays clean.
fn conversation_to_json(conv: &crate::db::Conversation) -> Value {
    let mut val = serde_json::to_value(enrich_conversation(conv)).unwrap_or(Value::Null);
    if let Value::Object(ref mut map) = val {
        map.insert(
            "display_state".to_string(),
            Value::String(conv.state.display_state().as_str().to_string()),
        );
    }
    val
}

/// Like [`conversation_to_json`] but also resolves `seed_parent_slug` via the
/// database so the frontend can render the seed breadcrumb (REQ-SEED-003).
/// Prefer this on single-conversation endpoints; the list endpoints stay
/// synchronous because they don't render breadcrumbs.
async fn conversation_to_json_with_seed(state: &AppState, conv: &crate::db::Conversation) -> Value {
    let enriched = enrich_conversation_with_seed(state, conv).await;
    let mut val = serde_json::to_value(enriched).unwrap_or(Value::Null);
    if let Value::Object(ref mut map) = val {
        map.insert(
            "display_state".to_string(),
            Value::String(conv.state.display_state().as_str().to_string()),
        );
    }
    val
}

// ============================================================
// SPA Handler
// ============================================================

/// Serve the SPA index.html for all client-side routes
async fn serve_spa() -> impl IntoResponse {
    match get_index_html() {
        Some(content) => Html(content).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Html("<h1>404 - UI not found. Build with: cd ui && npm run build</h1>".to_string()),
        )
            .into_response(),
    }
}

// ============================================================
// Conversation Listing (REQ-API-001)
// ============================================================

async fn list_conversations(
    State(state): State<AppState>,
) -> Result<Json<ConversationListResponse>, AppError> {
    let conversations = state
        .runtime
        .db()
        .list_conversations()
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let json_convs: Vec<Value> = conversations.iter().map(conversation_to_json).collect();

    Ok(Json(ConversationListResponse {
        conversations: json_convs,
    }))
}

async fn list_archived_conversations(
    State(state): State<AppState>,
) -> Result<Json<ConversationListResponse>, AppError> {
    let conversations = state
        .runtime
        .db()
        .list_archived_conversations()
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let json_convs: Vec<Value> = conversations.iter().map(conversation_to_json).collect();

    Ok(Json(ConversationListResponse {
        conversations: json_convs,
    }))
}

// ============================================================
// Projects (REQ-PROJ-014)
// ============================================================

async fn list_projects(State(state): State<AppState>) -> Result<Json<Value>, AppError> {
    let projects = state
        .db
        .list_projects()
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    Ok(Json(
        serde_json::to_value(projects).unwrap_or(Value::Array(vec![])),
    ))
}

// ============================================================
// Conversation Creation (REQ-API-002)
// ============================================================

#[allow(clippy::too_many_lines)]
async fn create_conversation(
    State(state): State<AppState>,
    Json(req): Json<CreateConversationRequest>,
) -> Result<Json<ConversationResponse>, AppError> {
    // Validate directory exists
    let path = PathBuf::from(&req.cwd);
    if !path.exists() {
        return Err(AppError::BadRequest("Directory does not exist".to_string()));
    }
    if !path.is_dir() {
        return Err(AppError::BadRequest("Path is not a directory".to_string()));
    }

    // REQ-SEED-001: seeded conversations may be created empty so the UI can
    // hydrate the input area with a draft and let the user review before
    // sending. For unseeded creates the text is still required.
    let is_seeded = req.seed_parent_id.is_some() || req.seed_label.is_some();
    if !is_seeded && req.text.trim().is_empty() {
        return Err(AppError::BadRequest(
            "Message text cannot be empty".to_string(),
        ));
    }

    // Validate requested model exists in the registry
    if let Some(ref model) = req.model {
        if state.llm_registry.get(model).is_none() {
            let available = state.llm_registry.available_models().join(", ");
            return Err(AppError::BadRequest(format!(
                "Model '{model}' is not available. Available models: {available}"
            )));
        }
    }

    // Idempotency check: if message_id already exists, find and return that conversation
    if state
        .db
        .message_exists(&req.message_id)
        .await
        .unwrap_or(false)
    {
        tracing::info!(
            message_id = %req.message_id,
            "Duplicate create request detected, returning existing conversation"
        );
        // Find the conversation for this message
        if let Ok(msg) = state.db.get_message_by_id(&req.message_id).await {
            if let Ok(conv) = state
                .runtime
                .db()
                .get_conversation(&msg.conversation_id)
                .await
            {
                return Ok(Json(ConversationResponse {
                    conversation: serde_json::to_value(conv).unwrap_or(Value::Null),
                }));
            }
        }
        // If we can't find it, fall through to create (shouldn't happen)
    }

    // Generate ID
    let id = uuid::Uuid::new_v4().to_string();

    // Try to generate a title using a cheap LLM model.
    //
    // Seeded conversations with empty text skip LLM title generation — we
    // derive the slug from `seed_label` (or fall back to a random slug)
    // because the LLM hallucinates titles from empty input.
    let seed_slug_source = if is_seeded && req.text.trim().is_empty() {
        req.seed_label
            .as_deref()
            .map(slugify_label)
            .filter(|s| !s.is_empty())
    } else {
        None
    };
    let slug = if let Some(s) = seed_slug_source {
        s
    } else if let Some(cheap_model) = state.runtime.model_registry().get_cheap_model() {
        match crate::title_generator::generate_title(&req.text, cheap_model).await {
            Some(title) if !title.is_empty() => {
                tracing::info!(title = %title, "Generated conversation title");
                title
            }
            _ => {
                tracing::info!("Title generation failed, using random slug");
                generate_slug()
            }
        }
    } else {
        tracing::info!("No cheap model available for title generation, using random slug");
        generate_slug()
    };

    // Detect project from git repo root (REQ-PROJ-001)
    let project_id = if let Some(repo_root) = crate::db::detect_git_repo_root(&path) {
        match state.db.find_or_create_project(&repo_root).await {
            Ok(project) => {
                tracing::info!(project_id = %project.id, path = %repo_root, "Associated conversation with project");
                Some(project.id)
            }
            Err(e) => {
                tracing::warn!(error = %e, "Failed to create project, continuing without");
                None
            }
        }
    } else {
        tracing::debug!(cwd = %req.cwd, "Directory is not in a git repo, no project association");
        None
    };

    // Direct mode is the default. "managed" opts in to Explore/Work lifecycle (requires git).
    // "auto" delegates the choice to the backend: managed if cwd is in a git repo,
    // direct otherwise (REQ-SEED-002).
    // "branch" checks out an existing branch in a worktree (REQ-PROJ-024).
    let resolved_mode: &str = match req.mode.as_deref() {
        Some("auto") => {
            if project_id.is_some() {
                tracing::info!(cwd = %req.cwd, "auto mode resolved to managed (git repo detected)");
                "managed"
            } else {
                tracing::info!(cwd = %req.cwd, "auto mode resolved to direct (no git repo)");
                "direct"
            }
        }
        Some(other) => other,
        None => "direct",
    };

    // Branch mode: create worktree on existing branch (REQ-PROJ-024)
    let (conv_mode, effective_cwd) = if resolved_mode == "branch" {
        let branch_name = req.base_branch.as_deref().ok_or_else(|| {
            AppError::BadRequest(
                "Branch mode requires base_branch (the branch name to check out)".to_string(),
            )
        })?;
        if project_id.is_none() {
            return Err(AppError::BadRequest(
                "Branch mode requires a git repository".to_string(),
            ));
        }
        let repo_root = crate::db::detect_git_repo_root(&path).ok_or_else(|| {
            AppError::BadRequest("Could not determine git repository root".to_string())
        })?;

        let conv_id = id.clone();
        let branch = branch_name.to_string();
        let repo = repo_root.clone();
        let db = state.db.clone();

        let result = tokio::task::spawn_blocking(move || {
            create_branch_worktree_blocking(&repo, &conv_id, &branch, &db)
        })
        .await
        .map_err(|e| AppError::Internal(format!("spawn_blocking failed: {e}")))?;

        match result {
            Ok(info) => {
                let mode = crate::db::ConvMode::Branch {
                    branch_name: crate::db::NonEmptyString::new(info.branch_name.clone())
                        .expect("branch_name from worktree creation must be non-empty"),
                    worktree_path: crate::db::NonEmptyString::new(info.worktree_path.clone())
                        .expect("worktree_path from worktree creation must be non-empty"),
                    base_branch: crate::db::NonEmptyString::new(info.base_branch)
                        .expect("base_branch from worktree creation must be non-empty"),
                };
                (mode, info.worktree_path)
            }
            Err(BranchWorktreeError::Conflict { slug }) => {
                return Err(AppError::Conflict(Box::new(
                    ConflictErrorResponse::new(
                        format!(
                            "Branch already has an active conversation: {slug}. \
                             Navigate to that conversation or abandon it first."
                        ),
                        "branch_already_active",
                    )
                    .with_conflict_slug(slug),
                )));
            }
            Err(BranchWorktreeError::Git(msg)) => {
                return Err(AppError::Internal(msg));
            }
            Err(BranchWorktreeError::BadRequest(msg)) => {
                return Err(AppError::BadRequest(msg));
            }
        }
    } else if resolved_mode == "managed" {
        if project_id.is_none() {
            return Err(AppError::BadRequest(
                "Managed mode requires a git repository".to_string(),
            ));
        }

        // REQ-PROJ-028: Managed mode allocates an Explore worktree on the chosen
        // branch up-front so the agent's view tracks the selected branch (not the
        // main checkout) from message zero. base_branch is required — silently
        // falling back to the repo root creates a divergence between the LLM's
        // worktree (correct after task approval) and the terminal pane (frozen
        // at the original spawn cwd) that surfaces as a footgun later.
        //
        // For `mode=auto` (the SDK / seed path), the caller has not made an
        // explicit branch choice — they delegated the decision to the backend.
        // Infer the current branch from the cwd. For an explicit `mode=managed`
        // (the form path), require an explicit base_branch — the form picks one
        // deliberately and the caller deserves a 400 if it's missing.
        let repo_root = crate::db::detect_git_repo_root(&path).ok_or_else(|| {
            AppError::BadRequest("Could not determine git repository root".to_string())
        })?;
        let inferred_base = if req.mode.as_deref() == Some("auto") && req.base_branch.is_none() {
            run_git(
                std::path::Path::new(&repo_root),
                &["rev-parse", "--abbrev-ref", "HEAD"],
            )
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty() && s != "HEAD")
        } else {
            None
        };
        let base_branch = req
            .base_branch
            .as_deref()
            .or(inferred_base.as_deref())
            .ok_or_else(|| {
                AppError::BadRequest(
                    "Managed mode requires base_branch (the branch to allocate \
                     the Explore worktree against)"
                        .to_string(),
                )
            })?;

        let conv_id = id.clone();
        let branch = base_branch.to_string();
        let repo = repo_root.clone();

        let result = tokio::task::spawn_blocking(move || {
            create_managed_explore_worktree_blocking(&repo, &conv_id, &branch)
        })
        .await
        .map_err(|e| AppError::Internal(format!("spawn_blocking failed: {e}")))?;

        let worktree_path = match result {
            Ok(p) => p,
            Err(ManagedWorktreeError::BadRequest(msg)) => return Err(AppError::BadRequest(msg)),
            Err(ManagedWorktreeError::Git(msg)) => return Err(AppError::Internal(msg)),
        };

        let worktree_nes = crate::db::NonEmptyString::new(&worktree_path)
            .map_err(|_| AppError::Internal("managed worktree path was empty".to_string()))?;
        (
            crate::db::ConvMode::Explore {
                worktree_path: Some(worktree_nes),
            },
            worktree_path,
        )
    } else {
        (crate::db::ConvMode::Direct, req.cwd.clone())
    };

    let desired_base_branch = if resolved_mode == "managed" {
        req.base_branch.as_deref()
    } else {
        None
    };
    // Resolve the model NOW so the conversation record reflects what is
    // actually being used (instead of leaving NULL and forcing every
    // consumer to reach for a default).
    //
    // - Explicit `req.model` always wins.
    // - Explore mode with no explicit model: drop to the cheap model for
    //   the registry's default-provider family (task 08670). Explore is
    //   read-only planning — Haiku-tier is fast enough for the iterative
    //   "think out loud, refine" loop and avoids charging Sonnet rates
    //   for plan iteration. Mirrors the sub-agent path at
    //   `runtime/executor.rs:914`.
    // - All other modes default to the registry default.
    //   Task 08609: a NULL `model` field surfaces as a literal "null" in
    //   tooltips, so we always persist a concrete id.
    let registry_default = state.llm_registry.default_model_id();
    let cheap_for_explore = state
        .llm_registry
        .cheap_model_id_for_provider(registry_default);
    let resolved_model = req.model.as_deref().map_or_else(
        || {
            if matches!(conv_mode, crate::db::ConvMode::Explore { .. }) {
                cheap_for_explore
            } else {
                registry_default.to_string()
            }
        },
        String::from,
    );
    let conversation = state
        .runtime
        .db()
        .create_conversation_with_project(
            &id,
            &slug,
            &effective_cwd,
            true,                          // user_initiated
            None,                          // no parent
            Some(resolved_model.as_str()), // resolved model (default if not explicit)
            project_id.as_deref(),
            &conv_mode,
            desired_base_branch,
            req.seed_parent_id.as_deref(),
            req.seed_label.as_deref(),
        )
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    // REQ-SEED-001: seeded conversations may be created with an empty
    // `text` — the UI will hydrate the input area from localStorage and the
    // user sends the first message manually. Skip expansion + initial event
    // dispatch in that case.
    if !(is_seeded && req.text.trim().is_empty()) {
        // Expand `@file` inline references before sending (REQ-IR-001, REQ-IR-007)
        let working_dir_for_expand = std::path::PathBuf::from(&effective_cwd);
        let expanded_initial = crate::message_expander::expand(&req.text, &working_dir_for_expand)
            .map_err(|e| {
                AppError::UnprocessableEntity(ExpansionErrorResponse {
                    error: e.to_string(),
                    error_type: e.error_type().to_string(),
                    reference: e.reference(),
                })
            })?;

        // Convert images
        let images: Vec<ImageData> = req
            .images
            .into_iter()
            .map(|img| ImageData {
                data: img.data,
                media_type: img.media_type,
            })
            .collect();

        // Only set llm_text when expansion actually changed the text (REQ-IR-001)
        let initial_llm_text = (expanded_initial.llm_text != expanded_initial.display_text)
            .then_some(expanded_initial.llm_text);

        // Send the initial message to the runtime
        let event = Event::UserMessage {
            text: expanded_initial.display_text,
            llm_text: initial_llm_text,
            images,
            message_id: req.message_id,
            user_agent: None,
            skill_invocation: expanded_initial.skill_invocation,
        };

        state
            .runtime
            .send_event(&id, event)
            .await
            .map_err(|e| AppError::Internal(e.clone()))?;
    }

    Ok(Json(ConversationResponse {
        conversation: serde_json::to_value(conversation).unwrap_or(Value::Null),
    }))
}

// ============================================================
// Branch Mode Worktree Creation (REQ-PROJ-024)
// ============================================================

struct BranchWorktreeInfo {
    branch_name: String,
    worktree_path: String,
    base_branch: String,
}

enum BranchWorktreeError {
    Conflict { slug: String },
    Git(String),
    BadRequest(String),
}

/// Create a git worktree for an existing branch. Runs on a blocking thread.
///
/// Delegates to `git_ops::{materialize_branch, check_branch_conflict, create_worktree}`.
fn create_branch_worktree_blocking(
    repo_root: &str,
    conv_id: &str,
    branch_name: &str,
    db: &crate::db::Database,
) -> Result<BranchWorktreeInfo, BranchWorktreeError> {
    let cwd = std::path::Path::new(repo_root);

    if run_git(cwd, &["rev-parse", "--is-inside-work-tree"]).is_err() {
        return Err(BranchWorktreeError::BadRequest(
            "Directory is not a git repository".to_string(),
        ));
    }

    let current_branch = run_git(cwd, &["rev-parse", "--abbrev-ref", "HEAD"])
        .unwrap_or_else(|_| "HEAD".to_string())
        .trim()
        .to_string();
    let default_branch = run_git(cwd, &["symbolic-ref", "refs/remotes/origin/HEAD"])
        .ok()
        .and_then(|s| {
            s.trim()
                .strip_prefix("refs/remotes/origin/")
                .map(String::from)
        })
        .unwrap_or_else(|| current_branch.clone());

    materialize_branch(cwd, branch_name).map_err(|e| match e {
        GitOpError::BranchNotFound(b) => {
            BranchWorktreeError::BadRequest(format!("Branch '{b}' not found locally or at origin"))
        }
        other => BranchWorktreeError::Git(other.to_string()),
    })?;

    // REQ-PROJ-025: check if branch is already checked out BEFORE attempting worktree add.
    match check_branch_conflict(cwd, db, branch_name) {
        Ok(()) => {}
        Err(BranchConflict::PhoenixConversation { slug }) => {
            return Err(BranchWorktreeError::Conflict { slug });
        }
        Err(BranchConflict::ExternalCheckout { branch, location }) => {
            return Err(BranchWorktreeError::Git(format!(
                "Branch '{branch}' is already checked out in {location}. \
                 Git doesn't allow a branch to be checked out in two places at once. \
                 Switch to a different branch there first, or use Direct mode."
            )));
        }
    }

    let worktree_path_str =
        create_worktree(cwd, conv_id, branch_name, None).map_err(|e| match e {
            GitOpError::Io(msg) | GitOpError::Git(msg) => BranchWorktreeError::Git(msg),
            other @ GitOpError::BranchNotFound(_) => BranchWorktreeError::Git(other.to_string()),
        })?;

    tracing::info!(
        branch = %branch_name,
        worktree = %worktree_path_str,
        "Created Branch-mode worktree (REQ-PROJ-024)"
    );

    Ok(BranchWorktreeInfo {
        branch_name: branch_name.to_string(),
        worktree_path: worktree_path_str,
        base_branch: default_branch,
    })
}

// ============================================================
// Managed Mode Early Worktree (REQ-PROJ-028)
// ============================================================

/// Create a worktree at conversation start for Managed mode so the agent
/// explores the selected base branch, not the main checkout.
///
/// Creates a temporary branch `task-pending-{conv_id_prefix}` from the
/// base branch. At approval time, `execute_approve_task_blocking` detects
/// the existing worktree and renames the branch.
enum ManagedWorktreeError {
    /// User-input failure (e.g. branch doesn't exist locally or at origin).
    BadRequest(String),
    /// Infrastructure failure (worktree creation, generic git errors).
    Git(String),
}

fn create_managed_explore_worktree_blocking(
    repo_root: &str,
    conv_id: &str,
    base_branch: &str,
) -> Result<String, ManagedWorktreeError> {
    let cwd = std::path::Path::new(repo_root);

    materialize_branch(cwd, base_branch).map_err(|e| match e {
        GitOpError::BranchNotFound(b) => ManagedWorktreeError::BadRequest(format!(
            "Branch '{b}' not found locally or at origin",
        )),
        other => ManagedWorktreeError::Git(other.to_string()),
    })?;

    let id_prefix: String = conv_id.chars().take(8).collect();
    let temp_branch = format!("task-pending-{id_prefix}");

    let worktree_path_str = create_worktree(cwd, conv_id, &temp_branch, Some(base_branch))
        .map_err(|e| {
            ManagedWorktreeError::Git(format!(
                "Failed to create early worktree from '{base_branch}': {e}",
            ))
        })?;

    tracing::info!(
        conv_id = %conv_id,
        base_branch = %base_branch,
        temp_branch = %temp_branch,
        worktree = %worktree_path_str,
        "Created early Managed-mode worktree (REQ-PROJ-028)"
    );

    Ok(worktree_path_str)
}

// ============================================================
// Conversation Retrieval (REQ-API-003)
// ============================================================

#[derive(Debug, Deserialize)]
struct GetConversationQuery {
    after_sequence: Option<i64>,
}

async fn get_conversation(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<GetConversationQuery>,
) -> Result<Json<ConversationWithMessagesResponse>, AppError> {
    let conversation = state
        .runtime
        .db()
        .get_conversation(&id)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;

    let messages = if let Some(after) = query.after_sequence {
        state.runtime.db().get_messages_after(&id, after).await
    } else {
        state.runtime.db().get_messages(&id).await
    }
    .map_err(|e| AppError::Internal(e.to_string()))?;

    let json_msgs: Vec<Value> = messages.iter().map(enrich_message_for_api).collect();

    // Calculate context window from last usage
    let context_window_size = messages
        .iter()
        .filter_map(|m| m.usage_data.as_ref())
        .next_back()
        .map_or(0, crate::db::UsageData::context_window_used);

    Ok(Json(ConversationWithMessagesResponse {
        conversation: conversation_to_json_with_seed(&state, &conversation).await,
        messages: json_msgs,
        agent_working: conversation.is_agent_working(),
        display_state: conversation.state.display_state().as_str().to_string(),
        context_window_size,
    }))
}

/// `GET /api/conversations/:id/slug` — minimal lookup that returns just the
/// current slug. The full `get_conversation` payload includes every message
/// in the conversation, which is wasteful when a caller only needs to
/// resolve `agent_id` → slug for navigation (sub-agent links, task 08533).
async fn get_conversation_slug(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let conversation = state
        .runtime
        .db()
        .get_conversation(&id)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;

    Ok(Json(serde_json::json!({ "slug": conversation.slug })))
}

async fn get_system_prompt(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<SystemPromptResponse>, AppError> {
    let conversation = state
        .runtime
        .db()
        .get_conversation(&id)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;

    let cwd = std::path::PathBuf::from(&conversation.cwd);
    let system_prompt = crate::system_prompt::build_system_prompt(&cwd, false, None);

    Ok(Json(SystemPromptResponse { system_prompt }))
}

// ============================================================
// SSE Streaming (REQ-API-005)
// ============================================================

/// Type alias -- breadcrumb type now lives in `runtime.rs` as `SseBreadcrumb`.
type Breadcrumb = crate::runtime::SseBreadcrumb;

/// Extract breadcrumbs from the last turn in message history
/// A "turn" starts with the last user message and includes all subsequent agent/tool messages
fn extract_breadcrumbs(messages: &[Message]) -> Vec<Breadcrumb> {
    // Find the last user message index
    let last_user_idx = messages
        .iter()
        .rposition(|m| m.message_type == MessageType::User);

    let Some(start_idx) = last_user_idx else {
        return vec![];
    };

    // Extract preview from user message
    let user_preview = messages.get(start_idx).and_then(|msg| {
        if let MessageContent::User(user_content) = &msg.content {
            if user_content.text.is_empty() {
                None
            } else {
                Some(truncate_preview(&user_content.text, 50))
            }
        } else {
            None
        }
    });

    let user_seq_id = messages.get(start_idx).map(|m| m.sequence_id);

    let mut breadcrumbs = vec![Breadcrumb {
        crumb_type: "user".to_string(),
        label: "User".to_string(),
        tool_id: None,
        sequence_id: user_seq_id,
        preview: user_preview,
    }];

    // Track subagent calls for grouping
    let mut pending_subagents: Vec<(String, i64, Option<String>)> = vec![]; // (tool_id, seq_id, slug)

    // Process messages after the last user message
    for msg in messages.iter().skip(start_idx + 1) {
        if let MessageContent::Agent(blocks) = &msg.content {
            // Check for tool_use blocks
            for block in blocks {
                if let ContentBlock::ToolUse { id, name, input } = block {
                    if name == "subagent" {
                        // Collect subagent calls for grouping
                        let slug = input.get("slug").and_then(|v| v.as_str()).map(String::from);
                        pending_subagents.push((id.clone(), msg.sequence_id, slug));
                    } else {
                        // Flush any pending subagents before adding this tool
                        flush_subagents(&mut breadcrumbs, &mut pending_subagents);

                        let preview = extract_tool_preview(name, input);
                        breadcrumbs.push(Breadcrumb {
                            crumb_type: "tool".to_string(),
                            label: name.clone(),
                            tool_id: Some(id.clone()),
                            sequence_id: Some(msg.sequence_id),
                            preview,
                        });
                    }
                }
            }
            // Add LLM breadcrumb if there's text content (final response)
            if blocks
                .iter()
                .any(|b| matches!(b, ContentBlock::Text { .. }))
            {
                // Flush any pending subagents
                flush_subagents(&mut breadcrumbs, &mut pending_subagents);

                // Only add LLM if it's not already the last crumb
                if breadcrumbs.last().is_none_or(|b| b.crumb_type != "llm") {
                    breadcrumbs.push(Breadcrumb {
                        crumb_type: "llm".to_string(),
                        label: "LLM".to_string(),
                        tool_id: None,
                        sequence_id: Some(msg.sequence_id),
                        preview: Some("Agent response".to_string()),
                    });
                }
            }
        }
    }

    // Flush any remaining subagents
    flush_subagents(&mut breadcrumbs, &mut pending_subagents);

    breadcrumbs
}

/// Flush pending subagent calls into a single breadcrumb
fn flush_subagents(
    breadcrumbs: &mut Vec<Breadcrumb>,
    pending: &mut Vec<(String, i64, Option<String>)>,
) {
    if pending.is_empty() {
        return;
    }

    let count = pending.len();
    let (first_id, first_seq, first_slug) = pending.first().cloned().unwrap();

    let (label, preview) = if count == 1 {
        let slug_preview = first_slug.as_ref().map_or_else(
            || "Spawning subagent".to_string(),
            |s| format!("Spawning: {s}"),
        );
        ("subagent".to_string(), Some(slug_preview))
    } else {
        let slugs: Vec<_> = pending.iter().filter_map(|(_, _, s)| s.as_ref()).collect();
        let preview = if slugs.is_empty() {
            format!("{count} subagents")
        } else if slugs.len() <= 3 {
            slugs
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        } else {
            format!(
                "{}, +{} more",
                slugs[..2]
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                slugs.len() - 2
            )
        };
        (format!("{count} subagents"), Some(preview))
    };

    breadcrumbs.push(Breadcrumb {
        crumb_type: "subagents".to_string(),
        label,
        tool_id: Some(first_id),
        sequence_id: Some(first_seq),
        preview,
    });

    pending.clear();
}

/// Extract a preview string from tool input based on tool type
fn extract_tool_preview(tool_name: &str, input: &serde_json::Value) -> Option<String> {
    match tool_name {
        "bash" => input
            .get("command")
            .and_then(|v| v.as_str())
            .map(|s| truncate_preview(s, 60)),
        "patch" => {
            // Get path or first patch path
            input
                .get("path")
                .and_then(|v| v.as_str())
                .or_else(|| {
                    input
                        .get("patches")
                        .and_then(|v| v.as_array())
                        .and_then(|arr| arr.first())
                        .and_then(|p| p.get("path"))
                        .and_then(|v| v.as_str())
                })
                .map(|s| truncate_preview(s, 60))
        }
        "think" => Some("Internal reasoning".to_string()),
        "keyword_search" => input
            .get("query")
            .and_then(|v| v.as_str())
            .map(|s| truncate_preview(s, 60)),
        "read_image" => input
            .get("path")
            .and_then(|v| v.as_str())
            .map(|s| truncate_preview(s, 60)),
        "browser_navigate" => input
            .get("url")
            .and_then(|v| v.as_str())
            .map(|s| truncate_preview(s, 60)),
        "browser_eval" => input
            .get("expression")
            .and_then(|v| v.as_str())
            .map(|s| truncate_preview(s, 50)),
        "change_dir" => input
            .get("path")
            .and_then(|v| v.as_str())
            .map(|s| truncate_preview(s, 60)),
        "output_iframe" => input
            .get("title")
            .and_then(|v| v.as_str())
            .or_else(|| input.get("path").and_then(|v| v.as_str()))
            .map(|s| truncate_preview(s, 60)),
        _ => None,
    }
}

/// Truncate a string for preview, adding ellipsis if needed
fn truncate_preview(s: &str, max_len: usize) -> String {
    // Take first line only
    let first_line = s.lines().next().unwrap_or(s);
    let trimmed = first_line.trim();
    if trimmed.len() <= max_len {
        trimmed.to_string()
    } else {
        // Find a char boundary at or before max_len - 1 to avoid slicing
        // inside a multi-byte UTF-8 character (e.g., box-drawing chars).
        let end = trimmed
            .char_indices()
            .take_while(|&(i, _)| i < max_len - 1)
            .last()
            .map_or(0, |(i, c)| i + c.len_utf8());
        // Safety: `end` is computed from `char_indices()` on `trimmed`
        #[allow(clippy::string_slice)]
        let prefix = &trimmed[..end];
        format!("{prefix}…")
    }
}

#[allow(clippy::too_many_lines)]
async fn stream_conversation(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let conversation = state
        .runtime
        .db()
        .get_conversation(&id)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;

    let messages = state
        .runtime
        .db()
        .get_messages(&id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let last_sequence_id = state
        .runtime
        .db()
        .get_last_sequence_id(&id)
        .await
        .unwrap_or(0);

    let context_window_size = messages
        .iter()
        .filter_map(|m| m.usage_data.as_ref())
        .next_back()
        .map_or(0, crate::db::UsageData::context_window_used);

    // Extract breadcrumbs from the last turn
    let breadcrumbs = extract_breadcrumbs(&messages);

    // Get the conversation handle (subscribes + gives us broadcast_tx for polling)
    let handle = state
        .runtime
        .get_or_create(&id)
        .await
        .map_err(AppError::Internal)?;
    let broadcast_rx = handle.broadcast_tx.subscribe();

    // Compute initial commits_behind for Work conversations.
    // Extract the git info we need for both the init value and the polling task.
    let work_git_info = match &conversation.conv_mode {
        ConvMode::Work {
            branch_name,
            base_branch,
            ..
        }
        | ConvMode::Branch {
            branch_name,
            base_branch,
            ..
        } if !base_branch.as_str().starts_with("__LEGACY")
            && !branch_name.as_str().starts_with("__LEGACY") =>
        {
            // Resolve repo root from project
            let repo_root = if let Some(ref project_id) = conversation.project_id {
                state
                    .db
                    .get_project(project_id)
                    .await
                    .ok()
                    .map(|p| PathBuf::from(p.canonical_path))
            } else {
                None
            };
            repo_root.map(|root| (root, base_branch.to_string(), branch_name.to_string()))
        }
        _ => None,
    };

    let (initial_commits_behind, initial_commits_ahead) =
        if let Some((ref repo_root, ref base, ref task)) = work_git_info {
            let root1 = repo_root.clone();
            let base1 = base.clone();
            let task1 = task.clone();
            let root2 = repo_root.clone();
            let base2 = base.clone();
            let task2 = task.clone();
            let (behind, ahead) = tokio::join!(
                tokio::task::spawn_blocking(move || commits_behind(&root1, &base1, &task1)),
                tokio::task::spawn_blocking(move || commits_ahead(&root2, &base2, &task2)),
            );
            (behind.unwrap_or(0), ahead.unwrap_or(0))
        } else {
            (0, 0)
        };

    // Derive project_name from the project's canonical_path (repo root dirname).
    let project_name = if let Some(ref project_id) = conversation.project_id {
        state.db.get_project(project_id).await.ok().and_then(|p| {
            std::path::Path::new(&p.canonical_path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
        })
    } else {
        None
    };

    // Ensure the broadcaster's counter has at least absorbed the highest
    // persisted message id, then take the current tip as the Init's own
    // sequence_id. Init's `sequence_id` and `last_sequence_id` are the same
    // number by construction: the snapshot IS the highest fact the client has
    // seen so far, and it sets the floor for subsequent `applyIfNewer` checks.
    handle.broadcast_tx.observe_seq(last_sequence_id);
    let init_seq = handle.broadcast_tx.current_seq();

    // Create init event with typed data -- serialization deferred to SSE layer
    let init_event = SseEvent::Init {
        sequence_id: init_seq,
        conversation: Box::new(enrich_conversation_with_seed(&state, &conversation).await),
        messages,
        agent_working: conversation.is_agent_working(),
        display_state: conversation.state.display_state().as_str().to_string(),
        last_sequence_id: init_seq,
        context_window_size,
        breadcrumbs,
        commits_behind: initial_commits_behind,
        commits_ahead: initial_commits_ahead,
        project_name,
    };

    // Spawn periodic git delta polling for Work conversations (REQ-PROJ-011)
    if let Some((repo_root, base_branch, task_branch)) = work_git_info {
        let broadcast_tx = handle.broadcast_tx.clone();
        tokio::spawn(async move {
            let mut last_behind = initial_commits_behind;
            let mut last_ahead = initial_commits_ahead;
            loop {
                tokio::time::sleep(std::time::Duration::from_mins(1)).await;

                // REQ-PROJ-023: single-branch fetch for the base branch only.
                let fetch_root = repo_root.clone();
                let fetch_branch = base_branch.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    let refspec = format!(
                        "refs/heads/{fetch_branch}:refs/remotes/origin/{fetch_branch}"
                    );
                    if let Err(e) = run_git(&fetch_root, &["fetch", "origin", &refspec]) {
                        tracing::debug!(error = %e, "periodic single-branch fetch failed (non-fatal)");
                    }
                })
                .await;

                let root1 = repo_root.clone();
                let base1 = base_branch.clone();
                let task1 = task_branch.clone();
                let root2 = repo_root.clone();
                let base2 = base_branch.clone();
                let task2 = task_branch.clone();
                let (new_behind, new_ahead) = tokio::join!(
                    tokio::task::spawn_blocking(move || commits_behind(&root1, &base1, &task1)),
                    tokio::task::spawn_blocking(move || commits_ahead(&root2, &base2, &task2)),
                );
                let new_behind = new_behind.unwrap_or(last_behind);
                let new_ahead = new_ahead.unwrap_or(last_ahead);

                if new_behind != last_behind || new_ahead != last_ahead {
                    last_behind = new_behind;
                    last_ahead = new_ahead;
                    let result = broadcast_tx.send_seq(|seq| SseEvent::ConversationUpdate {
                        sequence_id: seq,
                        update: crate::runtime::ConversationMetadataUpdate {
                            cwd: None,
                            branch_name: None,
                            worktree_path: None,
                            conv_mode_label: None,
                            base_branch: None,
                            commits_behind: Some(new_behind),
                            commits_ahead: Some(new_ahead),
                            task_title: None,
                        },
                    });
                    // No receivers left -- client disconnected, exit polling loop
                    if result.is_err() {
                        break;
                    }
                }
            }
            tracing::debug!("git delta polling task exited");
        });
    }

    Ok(sse_stream(id, init_event, broadcast_rx))
}

// ============================================================
// User Actions (REQ-API-004)
// ============================================================

async fn send_chat(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<ChatRequest>,
) -> Result<Json<ChatResponse>, AppError> {
    // Idempotency check: if message_id already exists, return success without creating duplicate
    if state
        .db
        .message_exists(&req.message_id)
        .await
        .unwrap_or(false)
    {
        tracing::info!(
            conversation_id = %id,
            message_id = %req.message_id,
            "Duplicate message detected, returning success (idempotent)"
        );
        return Ok(Json(ChatResponse { queued: true }));
    }

    // Expand `@file` inline references before sending to the LLM (REQ-IR-001, REQ-IR-007)
    let conversation = state
        .runtime
        .db()
        .get_conversation(&id)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;

    let working_dir = std::path::PathBuf::from(&conversation.cwd);
    let expanded = crate::message_expander::expand(&req.text, &working_dir).map_err(|e| {
        AppError::UnprocessableEntity(ExpansionErrorResponse {
            error: e.to_string(),
            error_type: e.error_type().to_string(),
            reference: e.reference(),
        })
    })?;

    // Convert images
    let images: Vec<ImageData> = req
        .images
        .into_iter()
        .map(|img| ImageData {
            data: img.data,
            media_type: img.media_type,
        })
        .collect();

    // Only set llm_text when expansion actually changed the text (REQ-IR-001)
    let chat_llm_text = (expanded.llm_text != expanded.display_text).then_some(expanded.llm_text);

    // Send event to runtime with message_id and user_agent.
    // `text` carries the `display_text` (stored in DB, shown in history — REQ-IR-006).
    // `llm_text` is the expanded form delivered to the model when present (REQ-IR-001).
    let event = Event::UserMessage {
        text: expanded.display_text,
        llm_text: chat_llm_text,
        images,
        message_id: req.message_id,
        user_agent: req.user_agent,
        skill_invocation: expanded.skill_invocation,
    };

    state
        .runtime
        .send_event(&id, event)
        .await
        .map_err(AppError::BadRequest)?;

    Ok(Json(ChatResponse { queued: true }))
}

async fn cancel_conversation(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<CancelResponse>, AppError> {
    // Task 24682: guard against cancelling a conversation that's already
    // idle or in a terminal state. Before this guard, the state machine
    // would reject `UserCancel` from `Idle` with an `InvalidTransition`
    // error, which then leaked as a raw `Debug`-formatted toast in the UI.
    // Doing nothing is the right answer — there's nothing to cancel —
    // and the response's `no_op: true` lets callers distinguish this
    // from the "we stopped something in flight" case.
    let conversation = state
        .runtime
        .db()
        .get_conversation(&id)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;

    if matches!(conversation.state, ConvState::Idle) || conversation.state.is_terminal() {
        tracing::debug!(
            conv_id = %id,
            state = conversation.state.variant_name(),
            "cancel no-op: conversation has nothing in flight"
        );
        return Ok(Json(CancelResponse {
            ok: true,
            no_op: true,
        }));
    }

    state
        .runtime
        .send_event(&id, Event::UserCancel { reason: None })
        .await
        .map_err(AppError::BadRequest)?;

    Ok(Json(CancelResponse {
        ok: true,
        no_op: false,
    }))
}

/// Upgrade a conversation's model (e.g., from 200k to 1M context).
/// Requires the conversation to be idle -- cannot upgrade mid-turn.
async fn upgrade_conversation_model(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpgradeModelRequest>,
) -> Result<Json<SuccessResponse>, AppError> {
    // Validate the target model exists
    if state.llm_registry.get(&req.model).is_none() {
        return Err(AppError::BadRequest(format!(
            "Unknown model '{}'. Available: {:?}",
            req.model,
            state.llm_registry.available_models()
        )));
    }

    // Validate conversation exists and is idle
    let conv = state
        .runtime
        .db()
        .get_conversation(&id)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;

    if !matches!(conv.state, ConvState::Idle) {
        return Err(AppError::BadRequest(
            "Conversation must be idle to upgrade model".to_string(),
        ));
    }

    // Update in DB
    state
        .runtime
        .db()
        .update_conversation_model(&id, &req.model)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    // Evict the active runtime so it gets recreated with the new model
    state.runtime.evict_runtime(&id).await;

    tracing::info!(
        conv_id = %id,
        old_model = conv.model.as_deref().unwrap_or("default"),
        new_model = %req.model,
        "Conversation model upgraded"
    );

    Ok(Json(SuccessResponse { success: true }))
}

/// Manually trigger context continuation (REQ-BED-023)
async fn trigger_continuation(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<SuccessResponse>, AppError> {
    state
        .runtime
        .send_event(&id, Event::UserTriggerContinuation)
        .await
        .map_err(AppError::BadRequest)?;

    Ok(Json(SuccessResponse { success: true }))
}

/// Context continuation worktree transfer (REQ-BED-030).
///
/// Creates a new conversation that inherits the parent's environment
/// (`conv_mode`, `cwd`, worktree fields for Work/Branch/Explore, `task_id`
/// for Work). Parent's `continued_in_conv_id` is atomically set to the new
/// conversation's id in the same DB transaction.
///
/// Single-continuation policy: if the parent already has a continuation,
/// the endpoint returns the existing continuation's id with `already_existed:
/// true` (idempotent-return rather than 409 reject — friendlier to UI
/// retries, and the UI can route directly to the existing continuation).
///
/// Error shape:
///   - 404 if the parent id does not exist
///   - 409 if the parent is not in `ContextExhausted` state
///   - 500 on DB/transaction failure
async fn continue_conversation(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ContinueConversationResponse>, AppError> {
    use crate::db::{ContinueOutcome, DbError};

    let outcome = state
        .runtime
        .db()
        .continue_conversation(&id)
        .await
        .map_err(|e| match e {
            DbError::ConversationNotFound(msg) => AppError::NotFound(msg),
            other => AppError::Internal(other.to_string()),
        })?;

    match outcome {
        ContinueOutcome::Created(new_conv) => {
            tracing::info!(
                parent_id = %id,
                continuation_id = %new_conv.id,
                mode = new_conv.conv_mode.label(),
                "continuation created",
            );
            // Spawn runtime for the new conversation so SSE subscribers can
            // immediately find it. Fire-and-forget: the DB transaction is
            // the load-bearing side; a spawn failure does not roll the
            // conversation back (the handler's contract is the DB write
            // succeeded, not that the runtime is up).
            let conv_id = new_conv.id.clone();
            let runtime = state.runtime.clone();
            tokio::spawn(async move {
                if let Err(e) = runtime.get_or_create(&conv_id).await {
                    tracing::warn!(
                        conv_id = %conv_id,
                        error = %e,
                        "failed to spawn runtime for continuation (SSE subscribers will retry)",
                    );
                }
            });

            Ok(Json(ContinueConversationResponse {
                conversation_id: new_conv.id,
                slug: new_conv.slug,
                already_existed: false,
            }))
        }
        ContinueOutcome::AlreadyContinued(existing) => {
            tracing::info!(
                parent_id = %id,
                existing_continuation = %existing.id,
                "continuation already existed; returning existing id idempotently",
            );
            Ok(Json(ContinueConversationResponse {
                conversation_id: existing.id,
                slug: existing.slug,
                already_existed: true,
            }))
        }
        ContinueOutcome::ParentNotContextExhausted { state_variant } => {
            tracing::debug!(
                parent_id = %id,
                state = state_variant,
                "continuation rejected: parent is not context-exhausted",
            );
            Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
                format!(
                    "Conversation is not in context-exhausted state (current: {state_variant}); \
                     only context-exhausted conversations can be continued."
                ),
                "parent_not_context_exhausted",
            ))))
        }
    }
}

// ============================================================
// User Question Response (REQ-AUQ-003)
// ============================================================

#[derive(Deserialize)]
struct RespondToQuestionPayload {
    answers: std::collections::HashMap<String, String>,
    #[serde(default)]
    annotations:
        Option<std::collections::HashMap<String, crate::state_machine::state::QuestionAnnotation>>,
}

async fn respond_to_question(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<RespondToQuestionPayload>,
) -> Result<Json<SuccessResponse>, AppError> {
    // 1. Validate conversation exists and is in AwaitingUserResponse state
    let conv = state
        .runtime
        .db()
        .get_conversation(&id)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;

    if !matches!(conv.state, ConvState::AwaitingUserResponse { .. }) {
        return Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
            "Conversation is not awaiting a user response",
            "wrong_state",
        ))));
    }

    // 2. Dispatch response event to state machine
    state
        .runtime
        .send_event(
            &id,
            Event::UserQuestionResponse {
                answers: req.answers,
                annotations: req.annotations,
            },
        )
        .await
        .map_err(AppError::BadRequest)?;

    Ok(Json(SuccessResponse { success: true }))
}

// ============================================================
// Lifecycle (REQ-API-006)
// ============================================================

/// Refuse per-conversation lifecycle ops (archive / unarchive / delete) on
/// chain members. A chain is an atomic unit; mutating one member would
/// either fragment the chain (delete) or produce a half-state where the
/// sidebar shows part of the chain hidden (archive). The caller is
/// directed to `/api/chains/:rootId/{op}` via `conflict_slug` carrying
/// the root's slug.
async fn refuse_if_chain_member(state: &AppState, id: &str, op: &str) -> Result<(), AppError> {
    let db = state.runtime.db();
    let Some(root_id) = db
        .chain_root_if_member(id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
    else {
        return Ok(());
    };
    let root = db
        .get_conversation(&root_id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let mut response = ConflictErrorResponse::new(
        format!(
            "Cannot {op} a single chain member. Use the chain endpoint to {op} the whole chain.",
        ),
        "chain_member",
    );
    if let Some(slug) = root.slug {
        response = response.with_conflict_slug(slug);
    }
    Err(AppError::Conflict(Box::new(response)))
}

async fn archive_conversation(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<SuccessResponse>, AppError> {
    refuse_if_chain_member(&state, &id, "archive").await?;

    state
        .runtime
        .db()
        .archive_conversation(&id)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;

    Ok(Json(SuccessResponse { success: true }))
}

async fn unarchive_conversation(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<SuccessResponse>, AppError> {
    refuse_if_chain_member(&state, &id, "unarchive").await?;

    state
        .runtime
        .db()
        .unarchive_conversation(&id)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;

    Ok(Json(SuccessResponse { success: true }))
}

/// REQ-BED-032: Hard-delete cascade orchestrator.
///
/// Sequence (matching the Allium @guidance on
/// `UserHardDeletesConversationRule`):
///   1. Reject if busy (`RejectHardDeleteWhileBusy`) — 409 with
///      `error_type: "cancel_first"`. v1 is reject-only; the cancel-and-
///      wait branch is deferred. The `is_busy` derivation is the single
///      source of truth in `ConvState::is_busy`.
///   2. `cascade_bash_on_delete` — kill live handles, drop tombstones.
///   3. `cascade_tmux_on_delete` — kill-server, unlink socket, drop
///      registry entry.
///   4. `cascade_projects_on_delete` — worktree/branch removal for
///      Work / Branch / Explore-with-worktree conversations. Direct mode
///      and conversations whose worktree was already cleaned at terminal
///      transition: no-op.
///   5. `db.delete_conversation` — `SQLite` ON DELETE CASCADE removes
///      messages, tool calls, and other dependent rows. This is the only
///      step whose failure is surfaced to the user as a 5xx; the
///      cleanups in 2-4 log WARN and continue.
///   6. Broadcast `ConversationHardDeleted` on the conversation's
///      channel (if a runtime handle exists). Subscribers refresh
///      sidebar / navigation. Task 02697 wires the typed wire variant
///      through to the UI; this handler emits the in-process
///      `SseEvent::ConversationHardDeleted` today.
///
/// Failure isolation: cascades log structured WARN fields sufficient
/// for an operator to manually clean up orphans. Phoenix does NOT
/// attempt automatic recovery on subsequent startup — see REQ-BED-032
/// rationale.
async fn delete_conversation(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<SuccessResponse>, AppError> {
    refuse_if_chain_member(&state, &id, "delete").await?;
    run_hard_delete_cascade(&state, &id).await?;
    Ok(Json(SuccessResponse { success: true }))
}

/// Body of the [`delete_conversation`] handler, factored out so tests can
/// drive it directly without going through axum routing. Returns `Ok(())`
/// on success; the only fatal-to-the-request error is the DB row delete
/// (see `Internal` variant) — bash / tmux / projects cleanup failures
/// log WARN and continue per REQ-BED-032.
pub(super) async fn run_hard_delete_cascade(state: &AppState, id: &str) -> Result<(), AppError> {
    // Step 1: reject-if-busy. Read the conversation's persisted state
    // (the DB is updated before any side effect per persist-before-broadcast,
    // so DB state is the authoritative answer to "is this conversation busy?").
    let conv = state
        .runtime
        .db()
        .get_conversation(id)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;

    if conv.state.is_busy() {
        return Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
            "Cannot hard-delete a busy conversation. Cancel the in-flight \
             operation first, then retry.",
            "cancel_first",
        ))));
    }

    // Step 2: bash handles.
    let bash_report =
        crate::tools::bash::registry::cascade_bash_on_delete(state.runtime.bash_handles(), id)
            .await;
    let had_live_handles = !bash_report.live_handle_pgids.is_empty();
    let had_kill_failures = !bash_report.kill_failures.is_empty();
    if had_kill_failures {
        tracing::warn!(
            conv_id = %id,
            live_handle_pids = ?bash_report.live_handle_pids,
            live_handle_pgids = ?bash_report.live_handle_pgids,
            kill_pending_kernel_pids = ?bash_report.kill_pending_kernel_pids,
            kill_failures = ?bash_report.kill_failures,
            "bash cleanup had kill failures; orphan process groups may remain"
        );
    } else if had_live_handles {
        // We killed handles cleanly — log at debug so an operator
        // chasing leaks can correlate. Skipping the log entirely on the
        // pure no-op path keeps test output and prod logs quiet.
        tracing::debug!(
            conv_id = %id,
            live_handle_pids = ?bash_report.live_handle_pids,
            live_handle_pgids = ?bash_report.live_handle_pgids,
            kill_pending_kernel_pids = ?bash_report.kill_pending_kernel_pids,
            "bash cascade: SIGKILL'd live process groups"
        );
    }

    // Step 3: tmux server.
    //
    // worktree_path for socket keying (task 03001): use the typed worktree
    // field for Work/Branch, cwd for Explore (REQ-PROJ-028 guarantees cwd IS
    // the worktree for Explore), None for Direct. Mirrors the Explore
    // fallback in src/terminal/ws.rs — without it, the cascade looks up the
    // wrong (conv-{id}.sock) deterministic socket and the actual
    // wt-{hash}.sock tmux server is orphaned.
    let tmux_worktree_buf: Option<std::path::PathBuf> =
        conv.conv_mode.worktree_path().map(std::path::PathBuf::from);
    let tmux_report = crate::tools::tmux::registry::cascade_tmux_on_delete(
        state.runtime.tmux_registry(),
        id,
        tmux_worktree_buf.as_deref(),
        conv.continued_in_conv_id.as_deref(),
    )
    .await;
    if tmux_report.kill_server_error.is_some() || tmux_report.unlink_error.is_some() {
        let kill_status = tmux_report.kill_server_error.as_deref().unwrap_or("ok");
        tracing::warn!(
            conv_id = %id,
            socket_path = %tmux_report.socket_path.display(),
            kill_server_status = %kill_status,
            unlink_error = ?tmux_report.unlink_error,
            "tmux cleanup partial failure; orphan socket/server may remain"
        );
    }

    // Step 4: project worktree.
    let project_report = cascade_projects_on_delete(state, &conv).await;
    if let Some(err) = &project_report.error {
        tracing::warn!(
            conv_id = %id,
            worktree_path = ?project_report.worktree_path,
            branch_name = ?project_report.branch_name,
            error = %err,
            "project cleanup failed; orphan worktree/branch may remain"
        );
    }

    // Step 5: row deletion. SQLite ON DELETE CASCADE removes dependent
    // rows. This is the only step whose failure is fatal to the request
    // — partial cleanup above is non-fatal but a missing row deletion
    // means the user's "delete this conversation" never actually
    // happened.
    state
        .runtime
        .db()
        .delete_conversation(id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to delete conversation row: {e}")))?;

    // Step 6: broadcast. The conversation broadcaster is per-conv today;
    // task 02697 will route this to a sidebar-scoped channel on the UI
    // side. Until then, broadcasting on the per-conv channel reaches any
    // client currently subscribed to this conversation (so its tab can
    // close gracefully).
    if let Some(handle) = state.runtime.try_get_handle(id).await {
        let conv_id = id.to_string();
        let _ = handle
            .broadcast_tx
            .send_seq(|seq| SseEvent::ConversationHardDeleted {
                sequence_id: seq,
                conversation_id: conv_id,
            });
    }

    Ok(())
}

/// Best-effort report from [`cascade_projects_on_delete`]. The orchestrator
/// logs partial failures at WARN with these fields.
#[derive(Debug, Clone, Default)]
struct CascadeProjectsReport {
    /// Absolute worktree path that was (attempted to be) removed.
    /// `None` when the conversation has no worktree (Direct mode).
    worktree_path: Option<String>,
    /// Branch name considered for deletion. `None` for Direct/Explore-
    /// without-worktree.
    branch_name: Option<String>,
    /// Set when the worktree-removal flow returned an error after
    /// exhausting fallbacks. Branch deletion is best-effort and does
    /// not populate this field.
    error: Option<String>,
}

/// REQ-BED-032 step 4 / `WorktreeRemovedByConversationDelete`: clean
/// up the conversation's worktree (and, where applicable, branch) on
/// hard-delete. Reuses the same git incantations as `abandon_task`'s
/// step 2c — but explicitly NOT abandon: no diff snapshot, no system
/// message, no state-machine transition. The conversation row is about
/// to be deleted entirely; uncommitted work in the worktree is lost
/// per spec.
///
/// No-op cases:
///   - `ConvMode::Direct` — no worktree was ever created.
///   - `ConvMode::Explore { worktree_path: None }` — sub-agent Explore;
///     no worktree of its own (REQ-PROJ-008 sub-agents share the parent's).
///   - Already-terminal Work/Branch conversations — abandon /
///     mark-merged already removed the worktree at terminal transition.
///     We still attempt removal (it's idempotent) so a partial-failure
///     prior abandon gets a second chance.
///
/// Explore-with-worktree (top-level managed): the worktree is normally torn
/// down on terminal-state transition (`cleanup_worktree_if_present`). Hard-
/// delete short-circuits that path — the row is removed before the executor
/// reaches Terminal — so this cascade must remove the worktree itself, plus
/// the temporary `task-pending-{id_prefix}` branch that
/// `create_managed_explore_worktree_blocking` created (REQ-PROJ-028). The
/// branch was never promoted to a real task branch; it would otherwise
/// linger as a dangling ref.
async fn cascade_projects_on_delete(
    state: &AppState,
    conv: &crate::db::Conversation,
) -> CascadeProjectsReport {
    let (branch_name, worktree_path, is_work_mode) = match &conv.conv_mode {
        ConvMode::Work {
            branch_name,
            worktree_path,
            ..
        } => (branch_name.to_string(), worktree_path.to_string(), true),
        ConvMode::Branch {
            branch_name,
            worktree_path,
            ..
        } => (branch_name.to_string(), worktree_path.to_string(), false),
        ConvMode::Explore {
            worktree_path: Some(wt),
        } => {
            // Top-level managed Explore: temp branch follows the
            // REQ-PROJ-028 naming scheme. `is_work_mode = true` so the
            // blocking closure also runs `branch -D` on it.
            let id_prefix: String = conv.id.chars().take(8).collect();
            let temp_branch = format!("task-pending-{id_prefix}");
            (temp_branch, wt.to_string(), true)
        }
        ConvMode::Direct
        | ConvMode::Explore {
            worktree_path: None,
        } => {
            return CascadeProjectsReport::default();
        }
    };

    let mut report = CascadeProjectsReport {
        worktree_path: Some(worktree_path.clone()),
        branch_name: Some(branch_name.clone()),
        error: None,
    };

    // Resolve repo root from the project; if the conversation is not
    // project-scoped, we can't run `git worktree remove` against the
    // correct repo. The worktree path is still removable from disk.
    let repo_root: Option<PathBuf> = if let Some(project_id) = conv.project_id.as_deref() {
        match state.db.get_project(project_id).await {
            Ok(p) => Some(PathBuf::from(p.canonical_path)),
            Err(e) => {
                tracing::debug!(
                    conv_id = %conv.id,
                    project_id = %project_id,
                    error = %e,
                    "project lookup failed during cascade; falling back to fs-only worktree cleanup"
                );
                None
            }
        }
    } else {
        None
    };

    let outcome = tokio::task::spawn_blocking(move || -> Result<(), String> {
        let worktree_dir = PathBuf::from(&worktree_path);

        if let Some(repo) = repo_root.as_ref() {
            if let Err(e) = run_git(repo, &["worktree", "remove", &worktree_path, "--force"]) {
                tracing::debug!(
                    error = %e,
                    worktree = %worktree_path,
                    "git worktree remove failed; trying filesystem fallback"
                );
                if worktree_dir.exists() {
                    if let Err(rm_err) = std::fs::remove_dir_all(&worktree_dir) {
                        return Err(format!(
                            "git worktree remove failed: {e}; fs fallback also failed: {rm_err}"
                        ));
                    }
                }
                let _ = run_git(repo, &["worktree", "prune"]);
            }

            if is_work_mode {
                if let Err(e) = run_git(repo, &["branch", "-D", &branch_name]) {
                    tracing::debug!(
                        error = %e,
                        branch = %branch_name,
                        "branch delete failed (non-fatal in cascade)"
                    );
                }
            }
        } else if worktree_dir.exists() {
            if let Err(rm_err) = std::fs::remove_dir_all(&worktree_dir) {
                return Err(format!(
                    "no project context; fs-only worktree cleanup failed: {rm_err}"
                ));
            }
        }

        Ok(())
    })
    .await;

    match outcome {
        Ok(Ok(())) => {}
        Ok(Err(msg)) => report.error = Some(msg),
        Err(join_err) => report.error = Some(format!("worktree-cleanup task panicked: {join_err}")),
    }

    report
}

async fn rename_conversation(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<RenameRequest>,
) -> Result<Json<ConversationResponse>, AppError> {
    state
        .runtime
        .db()
        .rename_conversation(&id, &req.name)
        .await
        .map_err(|e| match e {
            crate::db::DbError::SlugExists(_) => {
                AppError::BadRequest("Slug already exists".to_string())
            }
            _ => AppError::NotFound(e.to_string()),
        })?;

    let conversation = state
        .runtime
        .db()
        .get_conversation(&id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    Ok(Json(ConversationResponse {
        conversation: serde_json::to_value(conversation).unwrap_or(Value::Null),
    }))
}

// ============================================================
// Slug Resolution (REQ-API-007)
// ============================================================

async fn get_by_slug(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> Result<Json<ConversationWithMessagesResponse>, AppError> {
    let conversation = state
        .runtime
        .db()
        .get_conversation_by_slug(&slug)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;

    let messages = state
        .runtime
        .db()
        .get_messages(&conversation.id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let json_msgs: Vec<Value> = messages.iter().map(enrich_message_for_api).collect();

    let context_window_size = messages
        .iter()
        .filter_map(|m| m.usage_data.as_ref())
        .next_back()
        .map_or(0, crate::db::UsageData::context_window_used);

    Ok(Json(ConversationWithMessagesResponse {
        conversation: conversation_to_json_with_seed(&state, &conversation).await,
        messages: json_msgs,
        agent_working: conversation.is_agent_working(),
        display_state: conversation.state.display_state().as_str().to_string(),
        context_window_size,
    }))
}

// ============================================================
// Directory Browser (REQ-API-008)
// ============================================================

#[derive(Debug, Deserialize)]
struct PathQuery {
    path: String,
}

async fn validate_cwd(Query(query): Query<PathQuery>) -> Json<ValidateCwdResponse> {
    // Normalize path: remove trailing slashes (except for root)
    let path_str = query.path.trim_end_matches('/');
    let path_str = if path_str.is_empty() { "/" } else { path_str };
    let path = PathBuf::from(path_str);

    if !path.exists() {
        return Json(ValidateCwdResponse {
            valid: false,
            error: Some("Directory does not exist".to_string()),
            is_git: false,
        });
    }

    if !path.is_dir() {
        return Json(ValidateCwdResponse {
            valid: false,
            error: Some("Path is not a directory".to_string()),
            is_git: false,
        });
    }

    // Check if this directory is inside a git repository by walking up to find .git
    let is_git = {
        let mut check = path.as_path();
        loop {
            if check.join(".git").exists() {
                break true;
            }
            match check.parent() {
                Some(parent) => check = parent,
                None => break false,
            }
        }
    };

    Json(ValidateCwdResponse {
        valid: true,
        error: None,
        is_git,
    })
}

async fn list_directory(
    Query(query): Query<PathQuery>,
) -> Result<Json<ListDirectoryResponse>, AppError> {
    // Normalize path: remove trailing slashes (except for root)
    let path_str = query.path.trim_end_matches('/');
    let path_str = if path_str.is_empty() { "/" } else { path_str };
    let path = PathBuf::from(path_str);

    let entries = fs::read_dir(&path)
        .map_err(|e| AppError::BadRequest(format!("Cannot read directory: {e}")))?;

    let mut result: Vec<DirectoryEntry> = entries
        .filter_map(Result::ok)
        .map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let is_dir = e.file_type().is_ok_and(|t| t.is_dir());
            DirectoryEntry { name, is_dir }
        })
        .collect();

    // Sort: directories first, then alphabetically
    result.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.cmp(&b.name),
    });

    Ok(Json(ListDirectoryResponse { entries: result }))
}

/// Create a directory (with parents if needed)
async fn mkdir(Json(payload): Json<PathQuery>) -> Json<MkdirResponse> {
    // Normalize path: remove trailing slashes (except for root)
    let path_str = payload.path.trim_end_matches('/');
    let path_str = if path_str.is_empty() { "/" } else { path_str };
    let path = PathBuf::from(path_str);

    // Security: ensure path is absolute and under allowed roots
    if !path.is_absolute() {
        return Json(MkdirResponse {
            created: false,
            error: Some("Path must be absolute".to_string()),
        });
    }

    // Don't allow creating directories outside of user's home or /tmp
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_default();
    let path_str = path.to_string_lossy();
    if (home.is_empty() || !path_str.starts_with(&home)) && !path_str.starts_with("/tmp/") {
        return Json(MkdirResponse {
            created: false,
            error: Some(format!(
                "Can only create directories under {} or /tmp",
                if home.is_empty() { "$HOME" } else { &home }
            )),
        });
    }

    // Check if already exists
    if path.exists() {
        if path.is_dir() {
            return Json(MkdirResponse {
                created: true, // Already exists, that's fine
                error: None,
            });
        }
        return Json(MkdirResponse {
            created: false,
            error: Some("Path exists but is not a directory".to_string()),
        });
    }

    // Create the directory (and parents)
    match fs::create_dir_all(&path) {
        Ok(()) => Json(MkdirResponse {
            created: true,
            error: None,
        }),
        Err(e) => Json(MkdirResponse {
            created: false,
            error: Some(format!("Failed to create directory: {e}")),
        }),
    }
}

// ============================================================
// File Browser API (REQ-PF-001 through REQ-PF-004)
// ============================================================

/// Detect file type from extension (REQ-PF-004)
fn detect_file_type(path: &std::path::Path) -> (String, bool) {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase);

    match ext.as_deref() {
        // Markdown
        Some("md" | "markdown") => ("markdown".to_string(), true),
        // Code files
        Some(
            "rs" | "ts" | "tsx" | "js" | "jsx" | "py" | "go" | "java" | "cpp" | "c" | "h" | "hpp"
            | "css" | "html" | "htm" | "vue" | "svelte" | "php" | "rb" | "swift" | "kt" | "scala"
            | "sh" | "bash" | "zsh" | "fish" | "ps1" | "sql" | "graphql" | "proto",
        ) => ("code".to_string(), true),
        // Config files
        Some(
            "json" | "yaml" | "yml" | "toml" | "ini" | "env" | "conf" | "cfg" | "xml"
            | "properties",
        ) => ("config".to_string(), true),
        // Text files
        Some("txt" | "log" | "csv" | "tsv" | "rtf") => ("text".to_string(), true),
        // Image files
        Some("png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "ico" | "bmp" | "tiff" | "tif") => {
            ("image".to_string(), false)
        }
        // Data/binary files
        Some(
            "db" | "sqlite" | "sqlite3" | "bin" | "dat" | "exe" | "dll" | "so" | "dylib" | "o"
            | "a" | "wasm" | "class" | "jar" | "war" | "pyc" | "pyo" | "pdf" | "doc" | "docx"
            | "xls" | "xlsx" | "ppt" | "pptx" | "zip" | "tar" | "gz" | "bz2" | "xz" | "7z" | "rar"
            | "mp3" | "mp4" | "wav" | "avi" | "mkv" | "mov" | "webm" | "flac" | "ogg",
        ) => ("data".to_string(), false),
        // Unknown - could be text, need to check when reading
        _ => ("unknown".to_string(), true),
    }
}

/// Check if file content appears to be valid text
fn is_valid_text(content: &[u8]) -> bool {
    // Check for null bytes (common in binary files)
    if content.contains(&0) {
        return false;
    }

    // Try to parse as UTF-8
    std::str::from_utf8(content).is_ok()
}

/// List files in a directory with metadata (REQ-PF-001, REQ-PF-002)
async fn list_files(Query(query): Query<PathQuery>) -> Result<Json<ListFilesResponse>, AppError> {
    let path_str = query.path.trim_end_matches('/');
    let path_str = if path_str.is_empty() { "/" } else { path_str };
    let path = PathBuf::from(path_str);

    if !path.exists() {
        return Err(AppError::NotFound("Directory does not exist".to_string()));
    }
    if !path.is_dir() {
        return Err(AppError::BadRequest("Path is not a directory".to_string()));
    }

    // Build gitignore matcher by walking up to find .gitignore files
    let gitignore = {
        let mut builder = ignore::gitignore::GitignoreBuilder::new(&path);
        let mut search_dir = path.clone();
        loop {
            let gitignore_path = search_dir.join(".gitignore");
            if gitignore_path.exists() {
                builder.add(gitignore_path);
            }
            if !search_dir.pop() {
                break;
            }
        }
        builder.build().ok()
    };

    let entries = fs::read_dir(&path)
        .map_err(|e| AppError::BadRequest(format!("Cannot read directory: {e}")))?;

    let mut items: Vec<FileEntry> = entries
        .filter_map(Result::ok)
        .map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            let entry_path = entry.path();
            let full_path = entry_path.to_string_lossy().to_string();
            let metadata = entry.metadata().ok();

            let is_directory = metadata.as_ref().is_some_and(std::fs::Metadata::is_dir);

            let (file_type, is_text_file) = if is_directory {
                ("folder".to_string(), false)
            } else {
                detect_file_type(&entry_path)
            };

            let size = if is_directory {
                None
            } else {
                metadata.as_ref().map(std::fs::Metadata::len)
            };

            let modified_time = metadata
                .as_ref()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs());

            let is_gitignored = gitignore.as_ref().is_some_and(|gi| {
                gi.matched_path_or_any_parents(&entry_path, is_directory)
                    .is_ignore()
            });

            FileEntry {
                name,
                path: full_path,
                is_directory,
                size,
                modified_time,
                file_type,
                is_text_file,
                is_gitignored,
            }
        })
        .collect();

    // Sort: directories first, then alphabetically (case-insensitive)
    items.sort_by(|a, b| match (a.is_directory, b.is_directory) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });

    Ok(Json(ListFilesResponse { items }))
}

/// Read file contents with text encoding validation (REQ-PF-005)
async fn read_file(Query(query): Query<PathQuery>) -> Result<Json<ReadFileResponse>, AppError> {
    let path = PathBuf::from(&query.path);

    if !path.exists() {
        return Err(AppError::NotFound("File does not exist".to_string()));
    }
    if path.is_dir() {
        return Err(AppError::BadRequest("Path is a directory".to_string()));
    }

    // Check file size (limit to 10MB for safety)
    let metadata = fs::metadata(&path)
        .map_err(|e| AppError::BadRequest(format!("Cannot read file metadata: {e}")))?;
    if metadata.len() > 10 * 1024 * 1024 {
        return Err(AppError::BadRequest(
            "File too large (max 10MB)".to_string(),
        ));
    }

    // Read file content
    let content =
        fs::read(&path).map_err(|e| AppError::BadRequest(format!("Cannot read file: {e}")))?;

    // Validate text encoding
    if !is_valid_text(&content) {
        return Err(AppError::BadRequest(
            "File appears to be binary or has invalid encoding".to_string(),
        ));
    }

    // Convert to string (we know it's valid UTF-8 from is_valid_text check)
    let text = String::from_utf8(content)
        .map_err(|_| AppError::BadRequest("Invalid UTF-8 encoding".to_string()))?;

    Ok(Json(ReadFileResponse {
        content: text,
        encoding: "utf-8".to_string(),
    }))
}

/// Serve a file from an absolute path with native Content-Type.
/// Used by "Open in browser" for HTML preview -- the path-based URL means
/// relative references (CSS, JS, images) resolve correctly against the
/// file's directory.
///
/// URL: `/preview/Users/scott/dev/site/index.html`
/// A `<link href="style.css">` resolves to `/preview/Users/scott/dev/site/style.css`
async fn serve_preview_file(
    Path(filepath): Path<String>,
) -> Result<axum::response::Response, AppError> {
    use axum::response::IntoResponse;

    // filepath comes without leading slash from the wildcard capture
    let path = PathBuf::from(format!("/{filepath}"));

    if !path.exists() {
        return Err(AppError::NotFound("File does not exist".to_string()));
    }
    if path.is_dir() {
        // Try index.html for directory requests
        let index = path.join("index.html");
        if index.exists() {
            let content = fs::read(&index)
                .map_err(|e| AppError::BadRequest(format!("Cannot read file: {e}")))?;
            let content_type = mime_guess::from_path(&index)
                .first_or_octet_stream()
                .to_string();
            return Ok(
                ([(axum::http::header::CONTENT_TYPE, content_type)], content).into_response(),
            );
        }
        return Err(AppError::BadRequest("Path is a directory".to_string()));
    }

    let metadata = fs::metadata(&path)
        .map_err(|e| AppError::BadRequest(format!("Cannot read file metadata: {e}")))?;
    if metadata.len() > 10 * 1024 * 1024 {
        return Err(AppError::BadRequest(
            "File too large (max 10MB)".to_string(),
        ));
    }

    let content =
        fs::read(&path).map_err(|e| AppError::BadRequest(format!("Cannot read file: {e}")))?;

    let content_type = mime_guess::from_path(&path)
        .first_or_octet_stream()
        .to_string();

    Ok(([(axum::http::header::CONTENT_TYPE, content_type)], content).into_response())
}

// ============================================================
// Conversation-scoped File Search (REQ-IR-004)
// ============================================================

/// Gitignore-aware recursive file search within the conversation's working directory.
///
/// Uses the `ignore` crate to respect `.gitignore`, `.ignore`, and other standard
/// exclusion files. Results are fuzzy-matched against the query when provided.
async fn search_conversation_files(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<FileSearchQuery>,
) -> Result<Json<FileSearchResponse>, AppError> {
    let conversation = state
        .runtime
        .db()
        .get_conversation(&id)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;

    let root = std::path::PathBuf::from(&conversation.cwd);
    if !root.exists() {
        return Err(AppError::NotFound(
            "Conversation working directory does not exist".to_string(),
        ));
    }

    let limit = query.limit.unwrap_or(50);
    let q = query.q.to_lowercase();

    // Walk the directory tree with gitignore awareness
    let walker = ignore::WalkBuilder::new(&root)
        .hidden(false) // include dot-files unless gitignored
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .ignore(true)
        .filter_entry(|e| e.file_name() != ".git") // .git/ is not gitignored, exclude explicitly
        .build();

    let mut items: Vec<(i32, FileSearchEntry)> = Vec::new();
    let mut matcher = nucleo_matcher::Matcher::new(nucleo_matcher::Config::DEFAULT);
    let mut buf: Vec<char> = Vec::new();

    for result in walker {
        let Ok(entry) = result else { continue };

        // Skip directories — only return files
        if entry.file_type().is_some_and(|ft| ft.is_dir()) {
            continue;
        }

        let abs_path = entry.path();
        let rel_path = abs_path
            .strip_prefix(&root)
            .unwrap_or(abs_path)
            .to_string_lossy()
            .to_string();

        let (_, is_text_file) = detect_file_type(abs_path);

        if q.is_empty() {
            // No query: return all files up to limit
            items.push((
                0i32,
                FileSearchEntry {
                    path: rel_path,
                    is_text_file,
                },
            ));
            if items.len() >= limit {
                break;
            }
        } else {
            // Score the match using nucleo (path-aware fuzzy matching).
            // Prefer filename matches over deep path matches.
            let score = fuzzy_score_path(&rel_path, &q, &mut matcher, &mut buf);
            if let Some(s) = score {
                items.push((
                    s,
                    FileSearchEntry {
                        path: rel_path,
                        is_text_file,
                    },
                ));
            }
        }
    }

    // Sort by score (highest first) when query is present, alphabetically otherwise
    if q.is_empty() {
        items.sort_by(|a, b| a.1.path.cmp(&b.1.path));
    } else {
        items.sort_by_key(|item| std::cmp::Reverse(item.0));
        items.truncate(limit);
    }

    let results: Vec<FileSearchEntry> = items.into_iter().map(|(_, entry)| entry).collect();
    Ok(Json(FileSearchResponse { items: results }))
}

/// Score a file path against a fuzzy query using nucleo-matcher.
/// Returns None if the path doesn't match. Higher scores = better matches.
///
/// Scores each path segment individually and takes the best. All segments
/// get the same +1000 bonus so nucleo's match quality alone determines the
/// winner — an exact directory-name match (nucleo ≈ 244 → total 1244) beats
/// a scattered-char match in a long UUID filename (nucleo ≈ 142 → total 1142).
fn fuzzy_score_path(
    path: &str,
    query: &str,
    matcher: &mut nucleo_matcher::Matcher,
    buf: &mut Vec<char>,
) -> Option<i32> {
    use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};

    let pattern = Pattern::new(
        query,
        CaseMatching::Ignore,
        Normalization::Smart,
        AtomKind::Fuzzy,
    );

    let best_segment = path
        .split('/')
        .filter_map(|seg| {
            buf.clear();
            buf.extend(seg.chars());
            let haystack = nucleo_matcher::Utf32Str::Unicode(buf);
            pattern
                .score(haystack, matcher)
                .map(|s| i32::try_from(s).unwrap_or(i32::MAX).saturating_add(1000))
        })
        .max();

    if best_segment.is_some() {
        return best_segment;
    }

    // Nothing matched on any segment — try full path as last resort.
    buf.clear();
    buf.extend(path.chars());
    let haystack = nucleo_matcher::Utf32Str::Unicode(buf);
    pattern
        .score(haystack, matcher)
        .map(|s| i32::try_from(s).unwrap_or(i32::MAX))
}

/// Discover skills available for the conversation's working directory (REQ-IR-005).
///
/// Calls `discover_skills()` from `system_prompt.rs` and returns each skill's
/// name, description, and optional `argument_hint` for frontend autocomplete.
async fn list_conversation_skills(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<SkillsResponse>, AppError> {
    let conversation = state
        .runtime
        .db()
        .get_conversation(&id)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;

    let cwd = std::path::PathBuf::from(&conversation.cwd);
    let skills = crate::system_prompt::discover_skills(&cwd);

    let skill_entries: Vec<SkillEntry> = skills
        .into_iter()
        .map(|s| SkillEntry {
            name: s.name,
            description: s.description,
            argument_hint: s.argument_hint,
            source: s.source,
            path: s.path.to_string_lossy().to_string(),
        })
        .collect();

    Ok(Json(SkillsResponse {
        skills: skill_entries,
    }))
}

// ============================================================
// Tasks
// ============================================================

/// List task files from the conversation's project tasks/ directory.
async fn list_conversation_tasks(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<TasksResponse>, AppError> {
    let conversation = state
        .runtime
        .db()
        .get_conversation(&id)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;

    let cwd = std::path::PathBuf::from(&conversation.cwd);
    let tasks_dir = cwd.join("tasks");

    // Build task_id -> conversation_slug map from active Work conversations
    let all_convs = state
        .runtime
        .db()
        .list_conversations()
        .await
        .unwrap_or_default();
    let task_to_slug: std::collections::HashMap<String, String> = all_convs
        .iter()
        .filter_map(|c| {
            let task_id = c.conv_mode.task_id()?;
            let slug = c.slug.as_deref()?;
            Some((task_id.to_string(), slug.to_string()))
        })
        .collect();

    let tasks = taskmd_core::tasks::list_tasks(&tasks_dir)
        .into_iter()
        .map(|t| {
            let conversation_slug = task_to_slug.get(&t.id).cloned();
            TaskEntry {
                id: t.id,
                priority: t.priority,
                status: t.status,
                slug: t.slug,
                path: t.path.to_string_lossy().into_owned(),
                conversation_slug,
            }
        })
        .collect();

    Ok(Json(TasksResponse { tasks }))
}

/// Token usage totals for a conversation (own turns + root rollup including sub-agents).
async fn get_conversation_usage_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ConversationUsage>, AppError> {
    let usage = state
        .db
        .get_conversation_usage(&id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(Json(usage))
}

// ============================================================
// Model Info (REQ-API-009)
// ============================================================

async fn list_models(State(state): State<AppState>) -> Json<ModelsResponse> {
    // Get model metadata from registry
    let models = state.llm_registry.available_model_info();

    let gateway_status = match state.llm_registry.gateway_status {
        GatewayStatus::NotConfigured => GatewayStatusApi::NotConfigured,
        GatewayStatus::Healthy => GatewayStatusApi::Healthy,
        GatewayStatus::Unreachable => GatewayStatusApi::Unreachable,
    };

    let llm_configured = state.llm_registry.has_models()
        || state.llm_registry.gateway_status != GatewayStatus::NotConfigured;

    let credential_status = if let Some(ref hs) = state.credential_helper {
        use crate::llm::CredentialStatus;
        match hs.credential_status().await {
            CredentialStatus::Idle => CredentialStatusApi::Required,
            CredentialStatus::Running => CredentialStatusApi::Running,
            CredentialStatus::Valid => CredentialStatusApi::Valid,
            CredentialStatus::Failed => CredentialStatusApi::Failed,
        }
    } else if llm_configured {
        CredentialStatusApi::Valid
    } else {
        CredentialStatusApi::NotConfigured
    };

    Json(ModelsResponse {
        models,
        default: state.llm_registry.default_model_id().to_string(),
        gateway_status,
        llm_configured,
        credential_status,
    })
}

// ============================================================
// Credential Helper
// ============================================================

async fn run_credential_helper(State(state): State<AppState>) -> impl IntoResponse {
    use axum::response::sse::{Event, KeepAlive, Sse};
    use futures::StreamExt;
    use std::convert::Infallible;
    use std::sync::Arc;

    let Some(ref hs) = state.credential_helper else {
        return (
            axum::http::StatusCode::NOT_FOUND,
            "No credential helper configured",
        )
            .into_response();
    };

    let event_stream = Arc::clone(hs).run_and_stream().await.map(|ev| {
        let data = match &ev {
            crate::llm::credential_helper::HelperEvent::Line(text) => {
                serde_json::json!({ "type": "line", "text": text })
            }
            crate::llm::credential_helper::HelperEvent::Complete => {
                serde_json::json!({ "type": "complete" })
            }
            crate::llm::credential_helper::HelperEvent::Error { exit_code, stderr } => {
                serde_json::json!({ "type": "error", "exit_code": exit_code, "stderr": stderr })
            }
        };
        Ok::<Event, Infallible>(Event::default().event("message").data(data.to_string()))
    });

    Sse::new(event_stream)
        .keep_alive(
            KeepAlive::new()
                .interval(std::time::Duration::from_secs(15))
                .text("ping"),
        )
        .into_response()
}

async fn invalidate_credential(State(state): State<AppState>) -> impl IntoResponse {
    use crate::llm::CredentialSource;

    let Some(ref hs) = state.credential_helper else {
        return (
            axum::http::StatusCode::NOT_FOUND,
            "No credential helper configured",
        )
            .into_response();
    };

    let was_valid = hs.invalidate().await;
    let status = if was_valid {
        "invalidated"
    } else {
        "already_idle"
    };
    tracing::info!(was_valid, "Credential manually invalidated via API");
    axum::Json(serde_json::json!({ "status": status })).into_response()
}

// ============================================================
// Environment Info
// ============================================================

async fn get_env() -> Json<serde_json::Value> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_default();
    Json(serde_json::json!({ "home_dir": home }))
}

// ============================================================
// Version
// ============================================================

async fn get_version() -> &'static str {
    concat!("phoenix-ide ", env!("CARGO_PKG_VERSION"))
}

/// Return status of all connected MCP servers.
async fn mcp_status(State(state): State<AppState>) -> impl IntoResponse {
    Json(state.mcp_manager.status().await)
}

/// Reload MCP server configurations: disconnect removed servers,
/// connect newly added ones, leave existing ones untouched.
async fn reload_mcp(State(state): State<AppState>) -> impl IntoResponse {
    let result = state.mcp_manager.reload().await;
    tracing::info!(
        added = ?result.added,
        removed = ?result.removed,
        unchanged = result.unchanged.len(),
        "MCP config reloaded"
    );
    Json(result)
}

/// Disable an MCP server: its tools are excluded from conversations.
/// The server stays connected for instant re-enable.
async fn disable_mcp_server(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = state.db.disable_mcp_server(&name).await {
        tracing::warn!(server = %name, error = %e, "Failed to persist MCP server disable");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response();
    }
    state.mcp_manager.disable_server(&name).await;
    tracing::info!(server = %name, "MCP server disabled");
    Json(serde_json::json!({"ok": true})).into_response()
}

/// Re-enable a previously disabled MCP server.
async fn enable_mcp_server(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = state.db.enable_mcp_server(&name).await {
        tracing::warn!(server = %name, error = %e, "Failed to persist MCP server enable");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response();
    }
    state.mcp_manager.enable_server(&name).await;
    tracing::info!(server = %name, "MCP server enabled");
    Json(serde_json::json!({"ok": true})).into_response()
}

// ============================================================
// Slug Generation (REQ-API-002)
// ============================================================

/// Slugify a human-readable label (e.g. "Shell integration setup (zsh)") into
/// a kebab-case slug (e.g. "shell-integration-setup-zsh"). Used for seeded
/// conversation titles when the LLM title generator would receive empty text.
fn slugify_label(label: &str) -> String {
    let mut out = String::with_capacity(label.len());
    let mut prev_dash = true;
    for ch in label.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    out.trim_end_matches('-').to_string()
}

fn generate_slug() -> String {
    let now = Local::now();

    // Day of week
    let day = match now.weekday() {
        chrono::Weekday::Mon => "monday",
        chrono::Weekday::Tue => "tuesday",
        chrono::Weekday::Wed => "wednesday",
        chrono::Weekday::Thu => "thursday",
        chrono::Weekday::Fri => "friday",
        chrono::Weekday::Sat => "saturday",
        chrono::Weekday::Sun => "sunday",
    };

    // Time of day
    let time = match now.hour() {
        6..=11 => "morning",
        12..=16 => "afternoon",
        17..=20 => "evening",
        _ => "night",
    };

    // Random words
    let words = &[
        "autumn",
        "river",
        "mountain",
        "forest",
        "meadow",
        "ocean",
        "desert",
        "valley",
        "sunrise",
        "sunset",
        "thunder",
        "lightning",
        "rainbow",
        "crystal",
        "shadow",
        "light",
        "ancient",
        "swift",
        "quiet",
        "brave",
        "golden",
        "silver",
        "azure",
        "emerald",
        "phoenix",
        "dragon",
        "falcon",
        "wolf",
        "raven",
        "tiger",
        "eagle",
        "fox",
        "dream",
        "spark",
        "flame",
        "frost",
        "storm",
        "breeze",
        "tide",
        "star",
    ];

    let mut rng = rand::thread_rng();
    let adjective = words.choose(&mut rng).unwrap_or(&"blue");
    let noun = words.choose(&mut rng).unwrap_or(&"sky");

    format!("{day}-{time}-{adjective}-{noun}")
}

// ============================================================
// Share Mode (REQ-AUTH-004 through REQ-AUTH-008)
// ============================================================

/// Create a share token for a conversation (by slug) and redirect to the share URL.
///
/// REQ-AUTH-004: If a token already exists, reuses it. Always redirects to `/s/{token}`.
async fn create_or_redirect_share(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> Result<Redirect, AppError> {
    let conversation = state
        .runtime
        .db()
        .get_conversation_by_slug(&slug)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;

    let token = state
        .db
        .create_share_token(&conversation.id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    Ok(Redirect::to(&format!("/s/{token}")))
}

/// Serve the SPA for a share link. The frontend handles rendering in read-only mode.
///
/// REQ-AUTH-005: Validates that the token exists before serving the page.
async fn serve_share_page(
    State(state): State<AppState>,
    Path(token): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    // Validate token exists
    state
        .db
        .get_share_token_by_token(&token)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
        .ok_or_else(|| {
            AppError::NotFound("Share link not found or has been revoked".to_string())
        })?;

    match get_index_html() {
        Some(content) => Ok(Html(content).into_response()),
        None => Ok((
            StatusCode::NOT_FOUND,
            Html("<h1>404 - UI not found. Build with: cd ui && npm run build</h1>".to_string()),
        )
            .into_response()),
    }
}

/// Return conversation data + messages for a shared conversation.
///
/// REQ-AUTH-006: Validates share token instead of password.
async fn get_shared_conversation(
    State(state): State<AppState>,
    Path(token): Path<String>,
) -> Result<Json<ConversationWithMessagesResponse>, AppError> {
    let (conversation_id, _) = state
        .db
        .get_share_token_by_token(&token)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
        .ok_or_else(|| AppError::NotFound("Invalid share token".to_string()))?;

    let conversation = state
        .runtime
        .db()
        .get_conversation(&conversation_id)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;

    let messages = state
        .runtime
        .db()
        .get_messages(&conversation_id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let json_msgs: Vec<Value> = messages.iter().map(enrich_message_for_api).collect();

    let context_window_size = messages
        .iter()
        .filter_map(|m| m.usage_data.as_ref())
        .next_back()
        .map_or(0, crate::db::UsageData::context_window_used);

    Ok(Json(ConversationWithMessagesResponse {
        conversation: conversation_to_json_with_seed(&state, &conversation).await,
        messages: json_msgs,
        agent_working: conversation.is_agent_working(),
        display_state: conversation.state.display_state().as_str().to_string(),
        context_window_size,
    }))
}

/// SSE stream for a shared conversation. Validates token, then subscribes.
///
/// REQ-AUTH-006 + REQ-AUTH-007: Token-validated, supports multiple simultaneous viewers.
async fn shared_sse_stream(
    State(state): State<AppState>,
    Path(token): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let (conversation_id, _) = state
        .db
        .get_share_token_by_token(&token)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
        .ok_or_else(|| AppError::NotFound("Invalid share token".to_string()))?;

    let conversation = state
        .runtime
        .db()
        .get_conversation(&conversation_id)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;

    let messages = state
        .runtime
        .db()
        .get_messages(&conversation_id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let last_sequence_id = state
        .runtime
        .db()
        .get_last_sequence_id(&conversation_id)
        .await
        .unwrap_or(0);

    let context_window_size = messages
        .iter()
        .filter_map(|m| m.usage_data.as_ref())
        .next_back()
        .map_or(0, crate::db::UsageData::context_window_used);

    let breadcrumbs = extract_breadcrumbs(&messages);

    let handle = state
        .runtime
        .get_or_create(&conversation_id)
        .await
        .map_err(AppError::Internal)?;
    let broadcast_rx = handle.broadcast_tx.subscribe();

    let project_name = if let Some(ref project_id) = conversation.project_id {
        state.db.get_project(project_id).await.ok().and_then(|p| {
            std::path::Path::new(&p.canonical_path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
        })
    } else {
        None
    };

    handle.broadcast_tx.observe_seq(last_sequence_id);
    let init_seq = handle.broadcast_tx.current_seq();

    let init_event = SseEvent::Init {
        sequence_id: init_seq,
        conversation: Box::new(enrich_conversation_with_seed(&state, &conversation).await),
        messages,
        agent_working: conversation.is_agent_working(),
        display_state: conversation.state.display_state().as_str().to_string(),
        last_sequence_id: init_seq,
        context_window_size,
        breadcrumbs,
        commits_behind: 0,
        commits_ahead: 0,
        project_name,
    };

    Ok(sse_stream(conversation_id, init_event, broadcast_rx))
}

// ============================================================
// Error Handling
// ============================================================

#[derive(Debug)]
pub(super) enum AppError {
    BadRequest(String),
    NotFound(String),
    Internal(String),
    /// 409 — conflict (dirty worktree, merge conflicts, etc.). Boxed because
    /// `ConflictErrorResponse` is the largest variant and grew with
    /// `continuation_id` (REQ-BED-031) — boxing keeps `AppError` compact so
    /// `Result<_, AppError>` isn't needlessly heavy in every handler.
    Conflict(Box<ConflictErrorResponse>),
    /// 422 — expansion reference validation failure (REQ-IR-007)
    UnprocessableEntity(ExpansionErrorResponse),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        match self {
            AppError::BadRequest(ref msg) => {
                tracing::debug!(error = %msg, "400 Bad Request");
                (
                    StatusCode::BAD_REQUEST,
                    Json(ErrorResponse::new(msg.clone())),
                )
                    .into_response()
            }
            AppError::NotFound(ref msg) => {
                tracing::debug!(error = %msg, "404 Not Found");
                (StatusCode::NOT_FOUND, Json(ErrorResponse::new(msg.clone()))).into_response()
            }
            AppError::Internal(ref msg) => {
                tracing::error!(error = %msg, "500 Internal Server Error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse::new(msg.clone())),
                )
                    .into_response()
            }
            AppError::Conflict(detail) => {
                tracing::warn!(error_type = %detail.error_type, error = %detail.error, "409 Conflict");
                (StatusCode::CONFLICT, Json(*detail)).into_response()
            }
            AppError::UnprocessableEntity(ref detail) => {
                tracing::warn!(error = %detail.error, "422 Unprocessable Entity");
                (StatusCode::UNPROCESSABLE_ENTITY, Json(detail.clone())).into_response()
            }
        }
    }
}

// ============================================================
// Hard-delete cascade tests (REQ-BED-032)
// ============================================================
#[cfg(test)]
mod hard_delete_cascade_tests {
    use super::*;
    use crate::chain_qa::ChainQa;
    use crate::db::Database;
    use crate::llm::ModelRegistry;
    use crate::platform::PlatformCapability;
    use crate::runtime::RuntimeManager;
    use crate::state_machine::ConvState;
    use crate::tools::mcp::McpClientManager;
    use std::sync::Arc;

    /// Construct a minimal `AppState` backed by an in-memory database.
    /// The state machine handler is started so `runtime.try_get_handle`
    /// works when the test wants to verify SSE events; conversations
    /// are otherwise inert (no LLM calls fire).
    async fn make_test_state() -> AppState {
        let db = Database::open_in_memory().await.expect("open db");
        let llm_registry = Arc::new(ModelRegistry::new_empty());
        let platform = PlatformCapability::None;
        let mcp_manager = Arc::new(McpClientManager::new());
        let runtime = Arc::new(RuntimeManager::new(
            db.clone(),
            llm_registry.clone(),
            platform,
            mcp_manager.clone(),
            None,
        ));
        let terminals = runtime.terminals.clone();
        let chain_qa = ChainQa::new(db.clone(), llm_registry.clone());
        AppState {
            runtime,
            llm_registry,
            db,
            platform,
            mcp_manager,
            credential_helper: None,
            password: None,
            terminals,
            chain_qa,
        }
    }

    #[tokio::test]
    async fn rejects_when_busy_and_succeeds_after_idle() {
        let state = make_test_state().await;
        state
            .db
            .create_conversation("c-1", "test", "/tmp", true, None, None)
            .await
            .expect("create");

        // Move to a busy state directly via the DB layer. ToolExecuting
        // is the heavy variant; LlmRequesting is the smallest busy state
        // and exercises the same `is_busy()` predicate.
        state
            .db
            .update_conversation_state("c-1", &ConvState::LlmRequesting { attempt: 0 })
            .await
            .expect("update state");

        let err = run_hard_delete_cascade(&state, "c-1")
            .await
            .expect_err("must reject while busy");
        match err {
            AppError::Conflict(detail) => {
                assert_eq!(detail.error_type, "cancel_first");
                assert!(detail.error.contains("Cancel"));
            }
            other => panic!("expected 409 Conflict, got {other:?}"),
        }

        // Conversation row still present.
        assert!(state.db.get_conversation("c-1").await.is_ok());

        // Settle to idle, retry — must succeed.
        state
            .db
            .update_conversation_state("c-1", &ConvState::Idle)
            .await
            .expect("settle");

        run_hard_delete_cascade(&state, "c-1")
            .await
            .expect("delete");
        assert!(
            state.db.get_conversation("c-1").await.is_err(),
            "row must be gone after successful cascade"
        );
    }

    #[tokio::test]
    async fn deletes_idle_conversation_and_drops_bash_registry_entry() {
        let state = make_test_state().await;
        state
            .db
            .create_conversation("c-2", "test", "/tmp", true, None, None)
            .await
            .expect("create");

        // Pre-seed the bash registry with an entry for this conversation
        // (no actual handles — just the per-conv table). The cascade must
        // drop it.
        let _ = state.runtime.bash_handles().get_or_create("c-2").await;

        run_hard_delete_cascade(&state, "c-2")
            .await
            .expect("delete");

        assert!(
            state
                .runtime
                .bash_handles()
                .remove_conversation("c-2")
                .await
                .is_none(),
            "bash registry entry must be removed by cascade"
        );
        assert!(state.db.get_conversation("c-2").await.is_err());
    }

    #[tokio::test]
    async fn broadcasts_hard_deleted_event_to_existing_subscribers() {
        let state = make_test_state().await;
        state
            .db
            .create_conversation("c-3", "test", "/tmp", true, None, None)
            .await
            .expect("create");

        // Force a runtime handle so the broadcaster exists. Subscribe
        // BEFORE the cascade runs; the SseEvent::ConversationHardDeleted
        // should arrive on the channel.
        let mut rx = state.runtime.subscribe("c-3").await.expect("subscribe");

        run_hard_delete_cascade(&state, "c-3")
            .await
            .expect("delete");

        // Drain a few events; the cascade event should be the only one
        // a freshly-subscribed receiver sees (no Init, no StateChange).
        let mut saw_hard_deleted = false;
        while let Ok(event) =
            tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await
        {
            match event {
                Ok(SseEvent::ConversationHardDeleted {
                    conversation_id, ..
                }) if conversation_id == "c-3" => {
                    saw_hard_deleted = true;
                    break;
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
        assert!(
            saw_hard_deleted,
            "ConversationHardDeleted SSE event must be broadcast"
        );
    }

    #[tokio::test]
    async fn cascade_continues_when_tmux_socket_dir_missing() {
        // The default tmux registry's socket_dir lives under
        // PHOENIX_DATA_DIR/HOME; cascade_tmux_on_delete tries
        // `tmux -S <path> kill-server` (best-effort) and `unlink(path)`
        // (NotFound is swallowed). With no prior server and no socket
        // file, this is a no-op success path — verifying that absence-of-
        // resource does not turn into a cascade-blocking error.
        let state = make_test_state().await;
        state
            .db
            .create_conversation("c-4", "test", "/tmp", true, None, None)
            .await
            .expect("create");

        run_hard_delete_cascade(&state, "c-4")
            .await
            .expect("delete");
        assert!(state.db.get_conversation("c-4").await.is_err());
    }

    #[tokio::test]
    async fn terminal_state_is_not_busy() {
        // Terminal-state conversations are deletable: hard-delete is the
        // user saying "remove this conversation entirely" and the row
        // must go regardless of how it reached terminal.
        let state = make_test_state().await;
        state
            .db
            .create_conversation("c-5", "test", "/tmp", true, None, None)
            .await
            .expect("create");
        state
            .db
            .update_conversation_state("c-5", &ConvState::Terminal)
            .await
            .expect("settle");

        run_hard_delete_cascade(&state, "c-5")
            .await
            .expect("delete");
        assert!(state.db.get_conversation("c-5").await.is_err());
    }

    #[tokio::test]
    async fn idempotent_on_repeated_calls() {
        // The first call deletes the row; the second call must surface
        // a NotFound (the row is gone) rather than panicking on a half-
        // cleaned registry.
        let state = make_test_state().await;
        state
            .db
            .create_conversation("c-6", "test", "/tmp", true, None, None)
            .await
            .expect("create");
        let _ = state.runtime.bash_handles().get_or_create("c-6").await;

        run_hard_delete_cascade(&state, "c-6")
            .await
            .expect("first delete");

        let err = run_hard_delete_cascade(&state, "c-6")
            .await
            .expect_err("second delete must 404");
        assert!(matches!(err, AppError::NotFound(_)));
    }

    /// Hand-rolled property-style sweep: across a small set of arbitrary
    /// (id, mode) combinations, every successful cascade leaves the in-
    /// memory bash and tmux registries clean of any reference to the
    /// deleted conversation.
    #[tokio::test]
    async fn registries_never_leak_after_cascade() {
        let state = make_test_state().await;
        let ids = ["c-a", "c-b", "c-c", "c-d", "c-e"];
        for id in ids {
            state
                .db
                .create_conversation(id, id, "/tmp", true, None, None)
                .await
                .expect("create");
            // Pre-seed both registries.
            let _ = state.runtime.bash_handles().get_or_create(id).await;
        }

        for id in ids {
            run_hard_delete_cascade(&state, id).await.expect("delete");
        }

        for id in ids {
            assert!(
                state
                    .runtime
                    .bash_handles()
                    .remove_conversation(id)
                    .await
                    .is_none(),
                "bash registry leaked entry for {id}"
            );
            assert!(state.db.get_conversation(id).await.is_err());
        }
    }

    /// Build a 2-member chain via raw SQL — same trick as the chains.rs
    /// test helper. The cascade tests only need the linkage; they don't
    /// exercise the continue_conversation gating on context_exhausted.
    async fn build_chain_for_test(state: &AppState, ids: &[&str]) {
        for id in ids {
            state
                .db
                .create_conversation(id, &format!("slug-{id}"), "/tmp", true, None, None)
                .await
                .expect("create");
        }
        for pair in ids.windows(2) {
            sqlx::query("UPDATE conversations SET continued_in_conv_id = ?1 WHERE id = ?2")
                .bind(pair[1])
                .bind(pair[0])
                .execute(state.db.pool())
                .await
                .expect("link");
        }
    }

    /// Per-conversation `delete` must refuse a chain member with a 409
    /// pointing at the chain root. Solo conversations remain deletable.
    #[tokio::test]
    async fn delete_refuses_chain_member_with_409() {
        let state = make_test_state().await;
        build_chain_for_test(&state, &["chn-a", "chn-b"]).await;

        // Refused for the root of a chain.
        let err = run_hard_delete_for_router("chn-a", &state)
            .await
            .expect_err("must refuse chain root");
        match err {
            AppError::Conflict(detail) => {
                assert_eq!(detail.error_type, "chain_member");
                assert_eq!(detail.conflict_slug.as_deref(), Some("slug-chn-a"));
            }
            other => panic!("expected 409, got {other:?}"),
        }

        // Refused for a non-root member, with the same root slug.
        let err = run_hard_delete_for_router("chn-b", &state)
            .await
            .expect_err("must refuse mid/leaf chain member");
        match err {
            AppError::Conflict(detail) => {
                assert_eq!(detail.error_type, "chain_member");
                assert_eq!(detail.conflict_slug.as_deref(), Some("slug-chn-a"));
            }
            other => panic!("expected 409, got {other:?}"),
        }

        // Both rows still present.
        assert!(state.db.get_conversation("chn-a").await.is_ok());
        assert!(state.db.get_conversation("chn-b").await.is_ok());
    }

    /// Mirror of the per-conversation `delete_conversation` axum handler
    /// body so the test exercises the chain-member guard + cascade pair
    /// without hitting the router.
    async fn run_hard_delete_for_router(id: &str, state: &AppState) -> Result<(), AppError> {
        refuse_if_chain_member(state, id, "delete").await?;
        run_hard_delete_cascade(state, id).await
    }

    /// `delete_chain_handler` walks the chain leaf-first and removes
    /// every member, leaving no rows behind.
    #[tokio::test]
    async fn chain_delete_handler_removes_every_member() {
        let state = make_test_state().await;
        build_chain_for_test(&state, &["cd-a", "cd-b", "cd-c"]).await;

        let _ = crate::api::chains::delete_chain_handler(
            axum::extract::State(state.clone()),
            axum::extract::Path("cd-a".to_string()),
        )
        .await
        .expect("chain delete");

        for id in ["cd-a", "cd-b", "cd-c"] {
            assert!(
                state.db.get_conversation(id).await.is_err(),
                "{id} must be gone after chain delete"
            );
        }
    }

    /// If any member of a chain is busy, `delete_chain_handler` refuses
    /// the whole operation up-front — no rows removed.
    #[tokio::test]
    async fn chain_delete_refuses_if_any_member_busy() {
        let state = make_test_state().await;
        build_chain_for_test(&state, &["cb-a", "cb-b"]).await;
        state
            .db
            .update_conversation_state("cb-b", &ConvState::LlmRequesting { attempt: 0 })
            .await
            .expect("set busy");

        let err = crate::api::chains::delete_chain_handler(
            axum::extract::State(state.clone()),
            axum::extract::Path("cb-a".to_string()),
        )
        .await
        .expect_err("must refuse while busy");
        match err {
            AppError::Conflict(detail) => assert_eq!(detail.error_type, "cancel_first"),
            other => panic!("expected 409, got {other:?}"),
        }

        // Both rows still present.
        assert!(state.db.get_conversation("cb-a").await.is_ok());
        assert!(state.db.get_conversation("cb-b").await.is_ok());
    }
}
