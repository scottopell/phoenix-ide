#![allow(clippy::wildcard_enum_match_arm)]
//! HTTP request handlers
//!
//! REQ-API-001 through REQ-API-010

use super::assets::{get_index_html, serve_favicon, serve_service_worker, serve_static};
use super::chains::{
    archive_chain_handler, delete_chain_handler, get_chain, regenerate_chain_name, set_chain_name,
    stream_chain, submit_chain_question,
};
use super::git_handlers::{
    create_pr_auto_fix_context, get_active_pr_diff, get_conversation_diff,
    get_conversation_pr_status, list_git_branches, pin_associated_pr,
    record_pr_auto_fix_context_baseline, resume_associated_pr_inference,
};
use super::global_recall;
use super::lifecycle_handlers::{
    abandon_task, approve_commission_review, approve_fork_proposal, approve_task,
    dismiss_fork_proposal, list_fork_proposals, mark_merged, reject_commission_review, reject_task,
    request_changes_on_fork_proposal, task_feedback,
};
use super::sse::sse_stream;
use super::types::{
    AttachmentUploadResponse, CancelResponse, ChatRequest, ChatResponse, CodeSearchEntry,
    CodeSearchQuery, CodeSearchResponse, ConflictErrorResponse, ContinueConversationResponse,
    ConversationListResponse, ConversationMessageRangeResponse, ConversationMessageSliceResponse,
    ConversationMessagesAroundResponse, ConversationMetaResponse, ConversationResponse,
    ConversationWithMessagesResponse, CreateConversationRequest, CredentialStatusApi,
    DirectoryEntry, ErrorResponse, ExpansionErrorResponse, FileEntry, FileSearchEntry,
    FileSearchQuery, FileSearchResponse, FileViewerKind, ListDirectoryResponse, ListFilesResponse,
    MkdirResponse, ModelsResponse, NotificationSettingsRequest, ProjectFileSearchQuery,
    ProjectSkillsQuery, ProjectTasksQuery, ReadFileResponse, RenameRequest, SkillEntry,
    SkillsResponse, SuccessResponse, SuggestRequest, SuggestResponse, SystemPromptResponse,
    TaskCountQuery, TaskCountResponse, TaskEntry, TasksResponse, UpgradeModelRequest,
    ValidateCwdResponse,
};
use super::AppState;
use crate::api::terminal_ws::{terminal_ws_global_handler, terminal_ws_handler};
use crate::db::{ConvMode, ConversationUsage, DbError, ImageData, NotificationSettings};
use crate::git_ops::{
    check_branch_conflict, create_worktree, materialize_branch, run_git, BranchConflict,
    GitOpError, PhoenixIgnoreStrategy,
};
use crate::runtime::SseEvent;
use crate::state_machine::{check_user_message_acceptable, ConvState, Event, TransitionError};

use super::browser_view::browser_view_ws_handler;

use axum::{
    extract::{DefaultBodyLimit, MatchedPath, Multipart, Path, Query, State},
    http::StatusCode,
    middleware,
    response::{Html, IntoResponse, Redirect, Response},
    routing::{delete, get, post},
    Json, Router,
};
use chrono::Datelike;
use chrono::{Local, Timelike};
use futures::future::BoxFuture;
use rand::seq::IndexedRandom;
use serde::Deserialize;
use serde_json::Value;
use sqlx::Row;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path as FsPath, PathBuf};
use std::time::{Duration, SystemTime};
use tokio::io::AsyncWriteExt;
use tower_http::trace::TraceLayer;

async fn trajectory_export_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match crate::analytics::trajectory_export(&state.db, &id).await {
        Ok(payload) => Json(payload).into_response(),
        Err(e) => {
            tracing::error!(error = %e, conv_id = %id, "trajectory export projection failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "analytics export failed").into_response()
        }
    }
}

/// Long-lived connection routes (SSE, WebSocket). Their `TraceLayer` span
/// lives until the response *body* ends, so it measures connection lifetime,
/// not request latency — exporting it would skew latency percentiles and hold
/// the span unexported until disconnect. `make_span_with` names these spans
/// "http.stream", which the `OTel` layer in `logging.rs` drops from export; the
/// stdout/file access log keeps them.
const STREAMING_ROUTES: &[&str] = &[
    "/api/conversations/:id/stream",
    "/api/chains/:rootId/stream",
    "/api/share/:token/events",
    "/api/conversations/:id/terminal",
    "/api/terminal/global",
    "/api/conversations/:id/browser-view",
];

/// Create the API router
pub fn create_router(state: AppState) -> Router {
    // The SPA client routes (`/`, `/new`, `/c/:slug`, …) are registered below
    // from `spa_routes::SPA_ROUTES` — the single source of truth shared with the
    // auth exemption (`auth::is_exempt_path`) so the two cannot drift. Adding a
    // React route means adding one entry there, not here.
    let router = Router::new()
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
        .route("/api/global/open-work", get(global_recall::open_work))
        .route(
            "/api/global/recall/sessions",
            get(global_recall::list_sessions).post(global_recall::create_session),
        )
        .route(
            "/api/global/recall/sessions/:id",
            get(global_recall::get_session),
        )
        .route(
            "/api/global/recall/sessions/:id/ask",
            post(global_recall::ask_session),
        )
        .route(
            "/api/global/resolve",
            post(global_recall::resolve_reference),
        )
        .route(
            "/api/conversations/archived",
            get(list_archived_conversations),
        )
        // Conversation creation (REQ-API-002)
        .route("/api/conversations/new", post(create_conversation))
        .route(
            "/api/conversations/new/with-attachments",
            post(create_conversation_with_attachments)
                .layer(DefaultBodyLimit::max(MAX_MULTIPART_BODY_BYTES)),
        )
        // Conversation retrieval (REQ-API-003)
        .route("/api/conversations/:id", get(get_conversation))
        .route(
            "/api/conversations/:id/browser-session",
            delete(stop_conversation_browser_session),
        )
        .route("/api/conversations/:id/slug", get(get_conversation_slug))
        .route("/api/conversations/:id/meta", get(get_conversation_meta))
        .route(
            "/api/conversations/:id/messages/latest",
            get(get_conversation_messages_latest),
        )
        .route(
            "/api/conversations/:id/messages",
            get(get_conversation_messages),
        )
        .route(
            "/api/conversations/:id/messages/range",
            get(get_conversation_message_range),
        )
        .route(
            "/api/conversations/:id/messages/around/:sequence",
            get(get_conversation_messages_around),
        )
        // SSE streaming (REQ-API-005)
        .route("/api/conversations/:id/stream", get(stream_conversation))
        // Terminal WebSocket (REQ-TERM-001 through REQ-TERM-014)
        .route("/api/conversations/:id/terminal", get(terminal_ws_handler))
        // Global terminal WebSocket — singleton scope, unbound to any
        // conversation (REQ-TERM-WS-001). Surfaced on /new.
        .route("/api/terminal/global", get(terminal_ws_global_handler))
        // Live browser view WebSocket (REQ-BT-018)
        .route(
            "/api/conversations/:id/browser-view",
            get(browser_view_ws_handler),
        )
        // User actions (REQ-API-004)
        .route("/api/conversations/:id/chat", post(send_chat))
        .route(
            "/api/conversations/:id/attachments",
            post(upload_conversation_attachments)
                .layer(DefaultBodyLimit::max(MAX_MULTIPART_BODY_BYTES)),
        )
        .route("/api/conversations/:id/cancel", post(cancel_conversation))
        // Steering queue management (task 01001)
        .route(
            "/api/conversations/:id/steering-queue/:message_id",
            delete(cancel_steering_message),
        )
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
        .route(
            "/api/conversations/:id/approve-commission-review",
            post(approve_commission_review),
        )
        .route(
            "/api/conversations/:id/reject-commission-review",
            post(reject_commission_review),
        )
        .route("/api/conversations/:id/task-feedback", post(task_feedback))
        // Fork proposal resolution (REQ-PROJ-034 / 037)
        .route("/api/conversations/:id/proposals", get(list_fork_proposals))
        .route(
            "/api/conversations/:id/proposals/:proposal_id/approve",
            post(approve_fork_proposal),
        )
        .route(
            "/api/conversations/:id/proposals/:proposal_id/dismiss",
            post(dismiss_fork_proposal),
        )
        .route(
            "/api/conversations/:id/proposals/:proposal_id/request-changes",
            post(request_changes_on_fork_proposal),
        )
        // User question response (REQ-AUQ-003)
        .route("/api/conversations/:id/respond", post(respond_to_question))
        .route(
            "/api/conversations/:id/dismiss-question",
            post(dismiss_question),
        )
        // Error dismissal: Error -> Idle (server-authoritative)
        .route("/api/conversations/:id/dismiss-error", post(dismiss_error))
        // Task abandon (REQ-PROJ-010)
        .route("/api/conversations/:id/abandon-task", post(abandon_task))
        // Mark as merged (REQ-PROJ-026)
        .route("/api/conversations/:id/mark-merged", post(mark_merged))
        // Lifecycle (REQ-API-006). Archive and delete are both terminal
        // transitions that run the resource-cleanup cascade (REQ-BED-032);
        // archive preserves the row, delete removes it. There is no
        // unarchive — archive is not reversible.
        .route("/api/conversations/:id/archive", post(archive_conversation))
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
        // One-shot shell-command suggestion. Stateless: a single LLM
        // completion with no conversation, no tools, no persistence. The
        // terminal renders results as click-to-run affordances.
        .route("/api/suggest", post(suggest_handler))
        // Slug resolution (REQ-API-007)
        .route(
            "/api/conversations/by-slug/:slug/meta",
            get(get_by_slug_meta),
        )
        .route("/api/conversations/by-slug/:slug", get(get_by_slug))
        // Phoenix Chains v1 (REQ-CHN-003 / 004 / 005 / 007)
        // Work-scope observability inventory (read-projection over the
        // bash/tmux/browser registries). `:scope_key` is a
        // `WorkScope::stable_key()` value.
        .route(
            "/api/work-scope/:scope_key/inventory",
            get(get_work_scope_inventory),
        )
        .route(
            "/api/work-scope/:scope_key/browser-session",
            delete(stop_work_scope_browser_session),
        )
        // Process inspector: per-handle drill-down (identity/state, output
        // delta, live resource sample). `:scope_key` is a
        // `WorkScope::stable_key()`; `:handle_id` names a bash handle in that
        // scope. See `specs/process-inspector/` REQ-PINSP-005.
        .route(
            "/api/work-scope/:scope_key/bash/:handle_id/inspect",
            get(inspect_bash_handle),
        )
        .route("/api/chains/:rootId", get(get_chain))
        .route("/api/chains/:rootId/qa", post(submit_chain_question))
        .route(
            "/api/chains/:rootId/name",
            axum::routing::patch(set_chain_name),
        )
        .route(
            "/api/chains/:rootId/regenerate-name",
            post(regenerate_chain_name),
        )
        .route("/api/chains/:rootId/stream", get(stream_chain))
        .route("/api/chains/:rootId/archive", post(archive_chain_handler))
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
        .route("/api/files/reveal", post(super::local_reveal::reveal_path))
        .route(
            "/api/conversations/:id/files/search",
            get(search_conversation_files),
        )
        .route(
            "/api/conversations/:id/code/search",
            get(search_conversation_code),
        )
        // Directory-scoped file search for the new-conversation composer,
        // before a conversation exists to hang the per-conversation route off
        // of (REQ-IR-004).
        .route("/api/files/search", get(search_project_files))
        // Skill discovery for autocomplete (REQ-IR-005)
        .route(
            "/api/conversations/:id/skills",
            get(list_conversation_skills),
        )
        // Directory-scoped skill discovery for the new-conversation composer
        // (REQ-IR-005).
        .route("/api/skills", get(list_project_skills))
        // Task listing
        .route("/api/conversations/:id/tasks", get(list_conversation_tasks))
        .route(
            "/api/conversations/:id/tasks/count",
            get(get_conversation_task_count),
        )
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
            "/api/conversations/:id/regenerate-name",
            post(regenerate_conversation_name),
        )
        .route(
            "/api/conversations/:id/upgrade-model",
            post(upgrade_conversation_model),
        )
        // Per-conversation worktree diff and PR state
        .route("/api/conversations/:id/diff", get(get_conversation_diff))
        .route(
            "/api/conversations/:id/active-pr/diff",
            get(get_active_pr_diff),
        )
        .route(
            "/api/conversations/:id/pr-status",
            get(get_conversation_pr_status),
        )
        .route(
            "/api/conversations/:id/associated-pr/pin",
            post(pin_associated_pr),
        )
        .route(
            "/api/conversations/:id/associated-pr/resume-inference",
            post(resume_associated_pr_inference),
        )
        .route(
            "/api/conversations/:id/pr-auto-fix-context",
            post(create_pr_auto_fix_context),
        )
        // Project task files available before a conversation exists
        .route("/api/tasks", get(list_project_tasks))
        // Git utilities
        .route("/api/git/branches", get(list_git_branches))
        .route(
            "/api/discovery/services",
            get(super::discovery::list_services),
        )
        // Environment info
        .route("/api/env", get(get_env))
        // MCP management
        .route("/api/mcp/status", get(mcp_status))
        .route("/api/mcp/reload", post(reload_mcp))
        .route("/api/mcp/servers/:name/disable", post(disable_mcp_server))
        .route("/api/mcp/servers/:name/enable", post(enable_mcp_server))
        .route("/api/mcp/oauth/callback", get(mcp_oauth_callback))
        // Notification settings (REQ-NOTIF-006, REQ-NOTIF-009)
        .route(
            "/api/settings/notifications",
            get(get_notification_settings).put(update_notification_settings),
        )
        // Global default LLM language. Read at conversation-create time and
        // pinned onto the new row; chain continuations inherit from the
        // parent rather than re-reading this default.
        .route(
            "/api/settings/llm-language",
            get(get_llm_language_setting).put(update_llm_language_setting),
        )
        // Version
        .route("/version", get(get_version))
        .route("/api/version", get(get_version_json))
        // About this deployment diagnostics
        .route("/api/deployment", get(super::deployment::deployment_info))
        .route(
            "/api/about/resources",
            get(super::deployment::about_resources),
        )
        .route(
            "/api/deployment/disk",
            get(super::deployment::deployment_disk),
        )
        .route(
            "/api/deployment/disk/managed-worktrees/cleanup",
            post(super::deployment::cleanup_managed_worktree),
        )
        // Usage analytics (read-only)
        .route("/api/usage", get(super::usage::usage_overview))
        .route(
            "/api/analytics/conversation/:id/trajectory-export",
            get(trajectory_export_handler),
        )
        .route(
            "/api/usage/conversation/:id",
            get(super::usage::usage_conversation_detail),
        )
        // Auth endpoints (REQ-AUTH-002, REQ-AUTH-003)
        .route("/api/auth/status", get(super::auth::auth_status))
        .route("/api/auth/login", post(super::auth::auth_login))
        // Codex / ChatGPT OAuth login (task 27104). PKCE+loopback and OpenAI's
        // custom device-code flow, both writing Phoenix's own
        // ~/.phoenix-ide/codex-auth.json (NOT Codex CLI's ~/.codex/auth.json —
        // see api/codex_login.rs module docs).
        .route(
            "/api/codex/login/preflight",
            get(super::codex_login::login_preflight),
        )
        .route(
            "/api/codex/login/pkce/start",
            post(super::codex_login::pkce_start),
        )
        .route(
            "/api/codex/login/pkce/:id/manual",
            post(super::codex_login::pkce_manual),
        )
        .route(
            "/api/codex/login/pkce/:id/status",
            get(super::codex_login::pkce_status),
        )
        .route(
            "/api/codex/login/pkce/:id/cancel",
            post(super::codex_login::pkce_cancel),
        )
        .route(
            "/api/codex/login/device/start",
            post(super::codex_login::device_start),
        )
        .route(
            "/api/codex/login/device/:id/status",
            get(super::codex_login::device_status),
        )
        .route(
            "/api/codex/login/device/:id/cancel",
            post(super::codex_login::device_cancel),
        )
        .route(
            "/api/codex/login/signout",
            post(super::codex_login::signout),
        )
        // Share mode (REQ-AUTH-004 through REQ-AUTH-008)
        .route("/share/c/:slug", get(create_or_redirect_share))
        .route("/s/:token", get(serve_share_page))
        .route(
            "/api/share/:token/conversation",
            get(get_shared_conversation),
        )
        .route("/api/share/:token/events", get(shared_sse_stream));

    // Register every SPA client route to serve the index.html shell, from the
    // single source of truth. These must be added before the auth layer below
    // so the middleware (which exempts them via the same SPA_ROUTES) wraps them.
    let router = super::spa_routes::SPA_ROUTES
        .iter()
        .fold(router, |router, route| {
            router.route(route.pattern(), get(serve_spa))
        });

    router
        // Auth middleware — runs before all route handlers (REQ-AUTH-001)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            super::auth::auth_middleware,
        ))
        // HTTP access log + Datadog tracing: applied via `route_layer` so it
        // runs AFTER routing, making axum's `MatchedPath` (the route template,
        // e.g. `/api/conversations/:id/stream`) available in `make_span_with`.
        //
        // The `otel.kind = "server"` field is read by the tracing-opentelemetry
        // bridge to set SpanKind::Server, which the datadog-opentelemetry
        // exporter maps to type=web. The `http.request.method` and `http.route`
        // fields are mapped to OTel HTTP semantic conventions, which the
        // exporter uses to set the operation name to "http.server.request" and
        // the resource to "METHOD /template". The `http.response.status_code`
        // field is declared as Empty in the span and recorded in on_response so
        // it appears as meta.http.status_code. For 5xx responses, the OTel span
        // status is set to ERROR so Datadog counts them in error-rate metrics.
        //
        // The raw `path` field is intentionally omitted from the span to avoid
        // exporting sensitive URL segments (share tokens, file paths) to
        // Datadog. The `http.route` template is sufficient for endpoint
        // grouping; the `method` field is retained for local log output. For
        // unmatched/fallback routes where MatchedPath is not available,
        // `http.route` is set to "unmatched" to avoid leaking raw paths.
        //
        // Health check endpoint (/version) uses Span::none() to suppress it
        // from both logging and OTel export entirely. The on_response callback
        // checks for Span::none() via id().is_none() before emitting the access
        // log event.
        .route_layer(
            TraceLayer::new_for_http()
                .make_span_with(|request: &axum::http::Request<_>| {
                    let path = request.uri().path();
                    if path == "/version" {
                        // Suppress health-check spans entirely — no log output,
                        // no OTel export.
                        tracing::Span::none()
                    } else {
                        // MatchedPath is the route template (e.g.
                        // /api/conversations/:id/stream), available because
                        // route_layer runs after routing. For unmatched/fallback
                        // routes where MatchedPath is not available, use
                        // "unmatched" to avoid leaking raw paths that may
                        // contain tokens or file paths.
                        let route = request
                            .extensions()
                            .get::<MatchedPath>()
                            .map_or_else(|| "unmatched".to_string(), |m| m.as_str().to_string());
                        if STREAMING_ROUTES.contains(&route.as_str()) {
                            tracing::info_span!(
                                "http.stream",
                                otel.kind = "server",
                                method = %request.method(),
                                "http.request.method" = %request.method(),
                                "http.route" = %route,
                                "http.response.status_code" = tracing::field::Empty,
                                "otel.status_code" = tracing::field::Empty,
                            )
                        } else {
                            tracing::info_span!(
                                "http",
                                otel.kind = "server",
                                method = %request.method(),
                                "http.request.method" = %request.method(),
                                "http.route" = %route,
                                "http.response.status_code" = tracing::field::Empty,
                                "otel.status_code" = tracing::field::Empty,
                            )
                        }
                    }
                })
                .on_response(
                    |response: &axum::http::Response<_>,
                     latency: std::time::Duration,
                     span: &tracing::Span| {
                        // Skip access-log event for suppressed spans (e.g.
                        // /version health checks that use Span::none()).
                        if span.id().is_none() {
                            return;
                        }
                        let status = response.status().as_u16();
                        // Record status code as a span attribute so the Datadog
                        // exporter maps it to meta.http.status_code.
                        span.record("http.response.status_code", status);
                        // Mark 5xx responses as errors so Datadog counts them
                        // in APM error-rate metrics.
                        if status >= 500 {
                            span.record("otel.status_code", "ERROR");
                        }
                        tracing::info!(
                            parent: span,
                            status = status,
                            latency_ms = u64::try_from(latency.as_millis()).unwrap_or(u64::MAX),
                        );
                    },
                )
                .on_request(tower_http::trace::DefaultOnRequest::new().level(tracing::Level::DEBUG))
                .on_failure(
                    tower_http::trace::DefaultOnFailure::new().level(tracing::Level::ERROR),
                ),
        )
        .with_state(state)
}

// ============================================================
// Message Transformation
// ============================================================

/// Render a message to its API `Value` shape by going through
/// [`crate::api::wire::EnrichedMessage`].
///
/// Production paths — both SSE and REST — carry the typed `EnrichedMessage`
/// and let serde serialize it once at the response boundary. This helper
/// exists only for the legacy `json!()` gold-standard reference in
/// `src/api/sse.rs`, which builds events as `Value` to byte-for-byte
/// cross-check the typed wire path.
#[cfg(test)]
pub(crate) fn enrich_message_for_api(msg: &crate::db::Message) -> Value {
    let enriched = super::wire::EnrichedMessage::from(msg);
    serde_json::to_value(&enriched).unwrap_or(Value::Null)
}

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
        // REQ-SEED-*: surface the home directory so the UI can spawn a seeded
        // conversation scoped to it (e.g. for shell integration setup).
        home_dir: Some(
            phoenix_core::runtime_env::PhoenixRuntimeEnvironment::detect()
                .home()
                .to_string_lossy()
                .into_owned(),
        ),
        seed_parent_slug: None,
        parent_conversation_slug: None,
        // Default to `false`; callers that have access to `AppState` set
        // this from the manager's `HashMap` via
        // `enrich_conversation_with_seed`.
        browser_session_active: false,
        // Stateless default. Both SSE init *and* the list-endpoint
        // serializer route through `enrich_conversation_with_runtime`,
        // which overwrites this with `TmuxRegistry::binary_available()`
        // — so consumers of `EnrichedConversation` never observe the
        // `false` default for a real conversation. The list path is
        // required because the 5s `listConversations` poll's
        // `upsertSnapshot` would otherwise regress an SSE-set `true`
        // back to `false` whenever a newer row landed.
        terminal_uses_tmux: false,
        // Resolved from the same inputs as `browser_session_active`'s
        // lookup (conversation id + worktree path). Computable without
        // AppState, so every enrich path emits the correct key.
        work_scope_key: crate::work_scope::WorkScope::resolve(
            &conv.id,
            conv.conv_mode.worktree_path().map(std::path::Path::new),
        )
        .stable_key(),
        creation_prompt: None,
        creation_error: None,
        cached_pr: None,
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
    include_cached_pr: bool,
) -> Result<crate::runtime::EnrichedConversation, AppError> {
    let mut enriched = enrich_conversation_with_runtime(state, conv);
    if include_cached_pr {
        enriched.cached_pr = cached_pr_summary_for_conversation(state, conv).await?;
    }
    if let Some(parent_id) = conv.seed_parent_id.as_deref() {
        if let Ok(parent) = state.runtime.db().get_conversation(parent_id).await {
            enriched.seed_parent_slug = parent.slug;
        }
    }
    // Resolve the sub-agent parent slug for the breadcrumb, same as the seed
    // parent above. Sub-agents set `parent_conversation_id` (not
    // `seed_parent_id`), so the two breadcrumbs are mutually exclusive.
    if let Some(parent_id) = conv.parent_conversation_id.as_deref() {
        if let Ok(parent) = state.runtime.db().get_conversation(parent_id).await {
            enriched.parent_conversation_slug = parent.slug;
        }
    }
    // Reflect current `BrowserSessionManager` state at hydration. The single
    // source of truth is the manager's `HashMap`; the SSE
    // `BrowserSessionState` event keeps the client in sync after this point.
    // Sessions are keyed by `WorkScope` (REQ-BROWSER-WS-001), so a
    // continuation of a worktree-backed conversation sees `true` here as
    // soon as its predecessor's session is live.
    let work_scope = crate::work_scope::WorkScope::resolve(
        &conv.id,
        conv.conv_mode.worktree_path().map(std::path::Path::new),
    );
    enriched.browser_session_active = state
        .runtime
        .browser_sessions()
        .is_active(&work_scope)
        .await;
    Ok(enriched)
}

/// Build an `EnrichedConversation` and apply runtime-derived fields that are
/// process-wide and synchronously readable. Used by both the AppState-aware
/// `enrich_conversation_with_seed` and the list-endpoint serializer
/// `conversation_to_json` so the same fields land in every wire payload.
///
/// Without this routing, list endpoints (which used to call the stateless
/// `enrich_conversation` directly) would emit `terminal_uses_tmux: false`
/// for every row. The 5s `listConversations` poll then upserted those rows
/// into the conversation atom's `Conversation` slot via
/// `RoutedStore.upsertSnapshot`, clobbering the `true` value previously set
/// by `sse_init` — and the terminal-selection composer label silently
/// regressed from `From tmux pane main:1.0` to `From terminal` for ~5s
/// windows. Codex review on PR #92.
fn enrich_conversation_with_runtime(
    state: &AppState,
    conv: &crate::db::Conversation,
) -> crate::runtime::EnrichedConversation {
    let mut enriched = enrich_conversation(conv);
    enriched.terminal_uses_tmux = state.runtime.tmux_registry().binary_available();
    enriched
}

/// Compute the full `presentation_mode` for a conversation, including the
/// `ContextExhausted` + `continued_in_conv_id` check.
///
/// - `ContextExhausted` with a continuation → `"done"` (read-only, continued elsewhere)
/// - `ContextExhausted` without a continuation → `"needs_action"` (user must act)
/// - All other states → delegate to `ConvState::presentation_mode()`
fn conv_presentation_mode(conv: &crate::db::Conversation) -> &'static str {
    if matches!(
        conv.state,
        crate::state_machine::ConvState::ContextExhausted { .. }
    ) {
        if conv.continued_in_conv_id.is_some() {
            return "done";
        }
        return "needs_action";
    }
    conv.state.presentation_mode()
}

fn conversation_work_scope(conv: &crate::db::Conversation) -> crate::work_scope::WorkScope {
    crate::work_scope::WorkScope::resolve(
        &conv.id,
        conv.conv_mode.worktree_path().map(std::path::Path::new),
    )
}

fn sidebar_cached_pr_summary(
    pr: &crate::db::WorkScopePrAssociation,
) -> crate::runtime::CachedPrSummary {
    crate::runtime::CachedPrSummary {
        number: pr.pr_number,
        title: pr.title.clone(),
        url: pr.url.clone(),
        display_state: pr.display_state.clone(),
        base: pr.base.clone(),
        head: pr.head.clone(),
        feedback_status: pr.feedback_status,
    }
}

async fn cached_pr_summary_for_conversation(
    state: &AppState,
    conv: &crate::db::Conversation,
) -> Result<Option<crate::runtime::CachedPrSummary>, AppError> {
    let scope = conversation_work_scope(conv);
    Ok(state
        .runtime
        .db()
        .primary_work_scope_pr_association(&scope)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
        .map(|pr| sidebar_cached_pr_summary(&pr)))
}

async fn cached_pr_summaries_for_conversations(
    state: &AppState,
    conversations: &[crate::db::Conversation],
) -> Result<HashMap<String, crate::runtime::CachedPrSummary>, AppError> {
    let scopes: Vec<_> = conversations.iter().map(conversation_work_scope).collect();
    let associations = state
        .runtime
        .db()
        .primary_work_scope_pr_associations(&scopes)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(associations
        .into_iter()
        .map(|(key, pr)| (key, sidebar_cached_pr_summary(&pr)))
        .collect())
}

/// Serialize a conversation to JSON with `presentation_mode` included.
///
/// Used by endpoints that return `serde_json::Value` (conversation list, etc.).
/// `presentation_mode` is injected here (not on `EnrichedConversation`) so REST
/// clients still receive it while the typed struct stays clean. Routes through
/// `enrich_conversation_with_runtime` so process-wide fields (`terminal_uses_tmux`)
/// land in every list response — see that helper's comment for the bug it fixes.
fn conversation_to_json(
    state: &AppState,
    conv: &crate::db::Conversation,
    cached_pr: Option<&crate::runtime::CachedPrSummary>,
) -> Value {
    let mut val =
        serde_json::to_value(enrich_conversation_with_runtime(state, conv)).unwrap_or(Value::Null);
    if let Value::Object(ref mut map) = val {
        map.insert(
            "presentation_mode".to_string(),
            Value::String(conv_presentation_mode(conv).to_string()),
        );
        map.insert(
            "requires_action".to_string(),
            Value::Bool(conv_requires_action(conv)),
        );
        if let Some(pr) = cached_pr {
            map.insert(
                "cached_pr".to_string(),
                serde_json::to_value(pr).unwrap_or(Value::Null),
            );
        }
    }
    val
}

async fn inject_creation_job_state_fields(
    state: &AppState,
    conv: &crate::db::Conversation,
    val: &mut Value,
) {
    if !matches!(
        conv.state,
        ConvState::Provisioning { .. }
            | ConvState::CreationFailed { .. }
            | ConvState::CreationCancelled { .. }
    ) {
        return;
    }
    let Ok(Some(job)) = state
        .runtime
        .db()
        .get_conversation_creation_job_for_conversation(&conv.id)
        .await
    else {
        return;
    };
    let Value::Object(map) = val else {
        return;
    };
    let Some(Value::Object(state_obj)) = map.get_mut("state") else {
        return;
    };
    let file_count = state
        .runtime
        .db()
        .get_conversation_creation_job_files(&job.id)
        .await
        .map(|files| files.len())
        .unwrap_or(0);
    let image_count = state
        .runtime
        .db()
        .get_conversation_creation_job_images(&job.id)
        .await
        .map(|images| images.len())
        .unwrap_or(0);
    state_obj.insert(
        "prompt".to_string(),
        Value::String(creation_intent_display_text(
            &job.intent,
            image_count,
            file_count,
        )),
    );
    if let Some(error) = job.error {
        state_obj.insert("message".to_string(), Value::String(error));
    }
}

fn creation_intent_display_text(
    intent: &crate::db::ConversationCreationIntent,
    image_count: usize,
    file_count: usize,
) -> String {
    let mut parts = Vec::new();
    let text = intent.text.trim();
    if !text.is_empty() {
        parts.push(text.to_string());
    }
    if image_count > 0 {
        parts.push(format!(
            "{} image attachment{}",
            image_count,
            if image_count == 1 { "" } else { "s" }
        ));
    }
    if file_count > 0 {
        parts.push(format!(
            "{} file attachment{}",
            file_count,
            if file_count == 1 { "" } else { "s" }
        ));
    }
    parts.join("\n")
}

/// Like [`conversation_to_json`] but also resolves `seed_parent_slug` via the
/// database so the frontend can render the seed breadcrumb (REQ-SEED-003).
/// Prefer this on single-conversation endpoints; the list endpoints stay
/// synchronous because they don't render breadcrumbs.
async fn conversation_to_json_with_seed(
    state: &AppState,
    conv: &crate::db::Conversation,
    include_cached_pr: bool,
) -> Result<Value, AppError> {
    let enriched = enrich_conversation_with_seed(state, conv, include_cached_pr).await?;
    let mut val = serde_json::to_value(&enriched).unwrap_or(Value::Null);
    if let Value::Object(ref mut map) = val {
        map.insert(
            "presentation_mode".to_string(),
            Value::String(conv_presentation_mode(conv).to_string()),
        );
        map.insert(
            "requires_action".to_string(),
            Value::Bool(conv_requires_action(conv)),
        );
        if let Some(pr) = enriched.cached_pr.clone() {
            map.insert(
                "cached_pr".to_string(),
                serde_json::to_value(pr).unwrap_or(Value::Null),
            );
        }
    }
    inject_creation_job_state_fields(state, conv, &mut val).await;
    Ok(val)
}

fn conv_requires_action(conv: &crate::db::Conversation) -> bool {
    match &conv.state {
        ConvState::ContextExhausted { .. } => conv.continued_in_conv_id.is_none(),
        state => matches!(
            state.display_state(),
            crate::state_machine::state::DisplayState::AwaitingApproval
        ),
    }
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
            Html(
                "<h1>404 - UI not found. Build with: corepack pnpm --dir ui run build</h1>"
                    .to_string(),
            ),
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

    let cached_prs = cached_pr_summaries_for_conversations(&state, &conversations).await?;
    let json_convs: Vec<Value> = conversations
        .iter()
        .map(|c| {
            let scope_key = conversation_work_scope(c).stable_key();
            conversation_to_json(&state, c, cached_prs.get(&scope_key))
        })
        .collect();

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

    let cached_prs = cached_pr_summaries_for_conversations(&state, &conversations).await?;
    let json_convs: Vec<Value> = conversations
        .iter()
        .map(|c| {
            let scope_key = conversation_work_scope(c).stable_key();
            conversation_to_json(&state, c, cached_prs.get(&scope_key))
        })
        .collect();

    Ok(Json(ConversationListResponse {
        conversations: json_convs,
    }))
}

// ============================================================
// Notification Settings (REQ-NOTIF-006, REQ-NOTIF-009)
// ============================================================

async fn get_notification_settings(
    State(state): State<AppState>,
) -> Result<Json<NotificationSettings>, AppError> {
    let settings = state
        .db
        .get_notification_settings()
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(Json(settings))
}

async fn update_notification_settings(
    State(state): State<AppState>,
    Json(req): Json<NotificationSettingsRequest>,
) -> Result<Json<NotificationSettings>, AppError> {
    let settings: NotificationSettings = req.into();
    state
        .db
        .set_notification_settings(&settings)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(Json(settings))
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct LlmLanguageSettingResponse {
    /// Current global default language for new conversations.
    language: String,
    /// All values the client may choose between.
    available: Vec<String>,
    /// Metadata and prompt templates for every built-in language.
    languages: Vec<crate::llm_language::LlmLanguageCatalogEntry>,
}

#[derive(Debug, serde::Deserialize)]
struct LlmLanguageSettingRequest {
    language: String,
}

fn llm_language_setting_response(
    lang: crate::llm_language::LlmLanguage,
) -> LlmLanguageSettingResponse {
    LlmLanguageSettingResponse {
        language: lang.as_str().to_string(),
        available: crate::llm_language::LlmLanguage::ALL
            .iter()
            .map(|l| l.as_str().to_string())
            .collect(),
        languages: crate::llm_language::language_catalog(),
    }
}

async fn get_llm_language_setting(
    State(state): State<AppState>,
) -> Result<Json<LlmLanguageSettingResponse>, AppError> {
    let lang = state
        .db
        .get_default_llm_language()
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(Json(llm_language_setting_response(lang)))
}

async fn update_llm_language_setting(
    State(state): State<AppState>,
    Json(req): Json<LlmLanguageSettingRequest>,
) -> Result<Json<LlmLanguageSettingResponse>, AppError> {
    let lang = crate::llm_language::LlmLanguage::parse(&req.language).ok_or_else(|| {
        let allowed: Vec<&str> = crate::llm_language::LlmLanguage::ALL
            .iter()
            .map(|l| l.as_str())
            .collect();
        AppError::BadRequest(format!(
            "invalid `language` field: {:?} (allowed: {})",
            req.language,
            allowed.join(", "),
        ))
    })?;
    state
        .db
        .set_default_llm_language(lang)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(Json(llm_language_setting_response(lang)))
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

const MAX_ATTACHMENT_SIZE_BYTES: usize = 10 * 1024 * 1024;
const MAX_TOTAL_ATTACHMENT_BYTES: usize = 25 * 1024 * 1024;
const MAX_MULTIPART_BODY_BYTES: usize = 30 * 1024 * 1024;
const MAX_ATTACHMENTS_PER_MESSAGE: usize = 10;
const ATTACHMENT_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);

fn attachment_root() -> PathBuf {
    phoenix_core::runtime_env::PhoenixRuntimeEnvironment::detect().attachments_dir()
}

async fn referenced_attachment_paths(db: &crate::db::Database) -> Result<HashSet<PathBuf>, String> {
    let file_rows = sqlx::query(
        "SELECT stored_path FROM message_files
         UNION
         SELECT stored_path FROM steering_message_files
         UNION
         SELECT f.stored_path
         FROM conversation_creation_job_files f
         JOIN conversation_creation_jobs j ON j.id = f.job_id
         WHERE j.status IN ('accepted', 'claimed', 'retry_scheduled', 'cancelling', 'cancelled', 'deletion_pending', 'failed')",
    )
    .fetch_all(db.pool())
    .await
    .map_err(|e| format!("failed to read attachment references: {e}"))?;

    let mut paths = HashSet::new();
    for row in file_rows {
        if let Ok(stored_path) = row.try_get::<String, _>("stored_path") {
            paths.insert(PathBuf::from(stored_path));
        }
    }
    Ok(paths)
}

fn sweep_expired_attachments_blocking(
    root: &std::path::Path,
    cutoff: SystemTime,
    referenced: &HashSet<PathBuf>,
) -> std::io::Result<()> {
    if !root.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        if metadata.is_dir() {
            sweep_expired_attachments_blocking(&path, cutoff, referenced)?;
            match std::fs::remove_dir(&path) {
                Ok(()) => {}
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::DirectoryNotEmpty | std::io::ErrorKind::NotFound
                    ) => {}
                Err(e) => return Err(e),
            }
        } else if metadata.modified().is_ok_and(|modified| modified < cutoff)
            && !referenced.contains(&path)
        {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
    }
    Ok(())
}

async fn cleanup_expired_attachments(db: &crate::db::Database) {
    let root = attachment_root();
    let referenced = match referenced_attachment_paths(db).await {
        Ok(paths) => paths,
        Err(e) => {
            tracing::warn!(error = %e, "failed to read attachment references; skipping TTL attachment sweep");
            return;
        }
    };
    let cutoff = SystemTime::now()
        .checked_sub(ATTACHMENT_TTL)
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let result = tokio::task::spawn_blocking(move || {
        sweep_expired_attachments_blocking(&root, cutoff, &referenced)
    })
    .await;
    match result {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::warn!(error = %e, "failed to sweep expired attachments"),
        Err(e) => tracing::warn!(error = %e, "attachment sweep task failed"),
    }
}

pub(super) fn start_attachment_cleanup_task(db: crate::db::Database) {
    tokio::spawn(async move {
        cleanup_expired_attachments(&db).await;
        let mut interval = tokio::time::interval(Duration::from_secs(24 * 60 * 60));
        loop {
            interval.tick().await;
            cleanup_expired_attachments(&db).await;
        }
    });
}

async fn delete_conversation_attachments_at_root(root: PathBuf, conversation_id: &str) {
    let path = root.join(conversation_id);
    match tokio::fs::remove_dir_all(&path).await {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            tracing::warn!(conversation_id, path = %path.display(), error = %e, "failed to delete conversation attachments");
        }
    }
}

async fn delete_conversation_attachments(conversation_id: &str) {
    delete_conversation_attachments_at_root(attachment_root(), conversation_id).await;
}

async fn delete_files(paths: &[String]) {
    for path in paths {
        match tokio::fs::remove_file(path).await {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                tracing::warn!(path, error = %e, "failed to delete uploaded attachment after rejected create request");
            }
        }
    }
}

fn sanitize_attachment_name(name: &str) -> String {
    let basename = FsPath::new(name)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("attachment");
    let sanitized: String = basename
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = sanitized.trim_matches('.').trim_matches('_');
    if trimmed.is_empty() {
        "attachment".to_string()
    } else {
        trimmed.chars().take(120).collect()
    }
}

fn validate_attachment_file(name: &str, media_type: &str, size: usize) -> Result<(), AppError> {
    if size == 0 {
        return Err(AppError::BadRequest(format!(
            "Attachment '{name}' is empty"
        )));
    }
    if size > MAX_ATTACHMENT_SIZE_BYTES {
        return Err(AppError::BadRequest(format!(
            "Attachment '{name}' exceeds the 10 MB per-file limit"
        )));
    }
    if media_type.starts_with("image/") {
        return Err(AppError::BadRequest(
            "Image files must use the image attachment channel".to_string(),
        ));
    }
    Ok(())
}

async fn validate_submitted_attachments(
    conversation_id: &str,
    files: &[crate::api::types::FileAttachment],
) -> Result<Vec<phoenix_core::domain::db_schema::FileAttachment>, AppError> {
    validate_submitted_attachments_at_root(&attachment_root(), conversation_id, files).await
}

async fn validate_submitted_attachments_at_root(
    root: &std::path::Path,
    conversation_id: &str,
    files: &[crate::api::types::FileAttachment],
) -> Result<Vec<phoenix_core::domain::db_schema::FileAttachment>, AppError> {
    if files.is_empty() {
        return Ok(Vec::new());
    }
    if files.len() > MAX_ATTACHMENTS_PER_MESSAGE {
        return Err(AppError::BadRequest(format!(
            "A message can include at most {MAX_ATTACHMENTS_PER_MESSAGE} files"
        )));
    }
    let expected_dir = root.join(conversation_id);
    let canonical_expected_dir = tokio::fs::canonicalize(&expected_dir)
        .await
        .map_err(|_| AppError::BadRequest("Attachment directory does not exist".to_string()))?;
    let mut total_bytes = 0usize;
    let mut validated = Vec::with_capacity(files.len());
    for file in files {
        let size = usize::try_from(file.size_bytes)
            .map_err(|_| AppError::BadRequest("Attachment size is invalid".to_string()))?;
        validate_attachment_file(&file.original_name, &file.media_type, size)?;
        total_bytes = total_bytes.saturating_add(size);
        if total_bytes > MAX_TOTAL_ATTACHMENT_BYTES {
            return Err(AppError::BadRequest(
                "Attachments exceed the 25 MB total limit".to_string(),
            ));
        }
        let path = PathBuf::from(&file.stored_path);
        let canonical_path = tokio::fs::canonicalize(&path).await.map_err(|_| {
            AppError::BadRequest(format!("Attachment '{}' is missing", file.original_name))
        })?;
        if !canonical_path.starts_with(&canonical_expected_dir) {
            return Err(AppError::BadRequest(format!(
                "Attachment '{}' does not belong to this conversation",
                file.original_name
            )));
        }
        let metadata = tokio::fs::metadata(&canonical_path).await.map_err(|_| {
            AppError::BadRequest(format!("Attachment '{}' is missing", file.original_name))
        })?;
        if !metadata.is_file() || metadata.len() != file.size_bytes {
            return Err(AppError::BadRequest(format!(
                "Attachment '{}' metadata does not match stored file",
                file.original_name
            )));
        }
        validated.push(file.clone().into());
    }
    Ok(validated)
}

async fn make_attachment_dir_private(dir: &std::path::Path) -> Result<(), AppError> {
    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to create attachment directory: {e}")))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o700);
        tokio::fs::set_permissions(dir, perms).await.map_err(|e| {
            AppError::Internal(format!("Failed to secure attachment directory: {e}"))
        })?;
    }
    Ok(())
}

async fn write_attachment_file_private(
    path: &std::path::Path,
    bytes: &[u8],
) -> Result<(), AppError> {
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to create attachment: {e}")))?;
    file.write_all(bytes)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to write attachment: {e}")))?;
    // Dropping a tokio File does NOT flush its internal buffer — buffered
    // bytes can land after a subsequent read observes the file (empty reads
    // on slow runners; a size mismatch in validate_attachments racing a
    // fresh upload).
    file.flush()
        .await
        .map_err(|e| AppError::Internal(format!("Failed to flush attachment: {e}")))?;
    Ok(())
}

async fn store_attachment_bytes(
    conversation_id: &str,
    original_name: String,
    media_type: String,
    bytes: axum::body::Bytes,
) -> Result<crate::api::types::FileAttachment, AppError> {
    validate_attachment_file(&original_name, &media_type, bytes.len())?;
    let dir = attachment_root().join(conversation_id);
    make_attachment_dir_private(&dir).await?;
    let filename = format!(
        "{}-{}",
        uuid::Uuid::new_v4(),
        sanitize_attachment_name(&original_name)
    );
    let path = dir.join(filename);
    write_attachment_file_private(&path, &bytes).await?;
    Ok(crate::api::types::FileAttachment {
        original_name,
        media_type,
        size_bytes: bytes.len() as u64,
        stored_path: path.to_string_lossy().into_owned(),
    })
}

struct RawAttachmentPart {
    original_name: String,
    media_type: String,
    bytes: axum::body::Bytes,
}

async fn read_multipart_create_parts(
    mut multipart: Multipart,
) -> Result<(CreateConversationRequest, Vec<RawAttachmentPart>), AppError> {
    let mut metadata: Option<CreateConversationRequest> = None;
    let mut files = Vec::new();
    let mut total_bytes = 0usize;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("Invalid multipart payload: {e}")))?
    {
        let name = field.name().unwrap_or_default().to_string();
        if name == "metadata" {
            let bytes = field
                .bytes()
                .await
                .map_err(|e| AppError::BadRequest(format!("Invalid metadata part: {e}")))?;
            metadata =
                Some(serde_json::from_slice(&bytes).map_err(|e| {
                    AppError::BadRequest(format!("Invalid metadata JSON part: {e}"))
                })?);
            continue;
        }
        if name != "files" {
            tracing::debug!(part = %name, "ignoring unexpected multipart field");
            continue;
        }
        if files.len() >= MAX_ATTACHMENTS_PER_MESSAGE {
            return Err(AppError::BadRequest(format!(
                "A message can include at most {MAX_ATTACHMENTS_PER_MESSAGE} files"
            )));
        }
        let original_name = field
            .file_name()
            .map_or_else(|| "attachment".to_string(), ToString::to_string);
        let media_type = field.content_type().map_or_else(
            || "application/octet-stream".to_string(),
            ToString::to_string,
        );
        let bytes = field
            .bytes()
            .await
            .map_err(|e| AppError::BadRequest(format!("Invalid file part: {e}")))?;
        total_bytes = total_bytes.saturating_add(bytes.len());
        if total_bytes > MAX_TOTAL_ATTACHMENT_BYTES {
            return Err(AppError::BadRequest(
                "Attachments exceed the 25 MB total limit".to_string(),
            ));
        }
        validate_attachment_file(&original_name, &media_type, bytes.len())?;
        files.push(RawAttachmentPart {
            original_name,
            media_type,
            bytes,
        });
    }

    let metadata = metadata.ok_or_else(|| {
        AppError::BadRequest("Multipart create requires a metadata JSON part".to_string())
    })?;
    Ok((metadata, files))
}

async fn read_multipart_attachments(
    conversation_id: &str,
    mut multipart: Multipart,
) -> Result<
    (
        Option<CreateConversationRequest>,
        Vec<crate::api::types::FileAttachment>,
    ),
    AppError,
> {
    let mut metadata: Option<CreateConversationRequest> = None;
    let mut files = Vec::new();
    let mut total_bytes = 0usize;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("Invalid multipart payload: {e}")))?
    {
        let name = field.name().unwrap_or_default().to_string();
        if name == "metadata" {
            let bytes = field
                .bytes()
                .await
                .map_err(|e| AppError::BadRequest(format!("Invalid metadata part: {e}")))?;
            metadata =
                Some(serde_json::from_slice(&bytes).map_err(|e| {
                    AppError::BadRequest(format!("Invalid metadata JSON part: {e}"))
                })?);
            continue;
        }

        if name != "files" {
            tracing::debug!(part = %name, "ignoring unexpected multipart field");
            continue;
        }

        if files.len() >= MAX_ATTACHMENTS_PER_MESSAGE {
            return Err(AppError::BadRequest(format!(
                "A message can include at most {MAX_ATTACHMENTS_PER_MESSAGE} files"
            )));
        }
        let original_name = field
            .file_name()
            .map_or_else(|| "attachment".to_string(), ToString::to_string);
        let media_type = field.content_type().map_or_else(
            || "application/octet-stream".to_string(),
            ToString::to_string,
        );
        let bytes = field
            .bytes()
            .await
            .map_err(|e| AppError::BadRequest(format!("Invalid file part: {e}")))?;
        total_bytes = total_bytes.saturating_add(bytes.len());
        if total_bytes > MAX_TOTAL_ATTACHMENT_BYTES {
            return Err(AppError::BadRequest(
                "Attachments exceed the 25 MB total limit".to_string(),
            ));
        }
        files
            .push(store_attachment_bytes(conversation_id, original_name, media_type, bytes).await?);
    }

    Ok((metadata, files))
}

// ============================================================
// Conversation Creation (REQ-API-002)
// ============================================================

#[allow(clippy::too_many_lines)]
async fn create_conversation(
    State(state): State<AppState>,
    Json(req): Json<CreateConversationRequest>,
) -> Result<Json<ConversationResponse>, AppError> {
    create_conversation_with_id(state, req, Vec::new()).await
}

#[allow(clippy::too_many_lines)]
async fn create_conversation_with_id(
    state: AppState,
    mut req: CreateConversationRequest,
    raw_files: Vec<RawAttachmentPart>,
) -> Result<Json<ConversationResponse>, AppError> {
    let id = match req.conversation_id.as_deref() {
        Some(conversation_id) => {
            uuid::Uuid::parse_str(conversation_id).map_err(|_| {
                AppError::BadRequest("conversation_id must be a valid UUID".to_string())
            })?;
            conversation_id.to_string()
        }
        None => uuid::Uuid::new_v4().to_string(),
    };
    // REQ-SEED-001: seeded conversations may be created empty so the UI can
    // hydrate the input area with a draft and let the user review before
    // sending. For unseeded creates the text is still required.
    let is_seeded = req.seed_parent_id.is_some() || req.seed_label.is_some();
    let has_file_content = !req.files.is_empty() || !raw_files.is_empty();
    if !is_seeded && req.text.trim().is_empty() && req.images.is_empty() && !has_file_content {
        return Err(AppError::BadRequest(
            "Message text cannot be empty".to_string(),
        ));
    }

    if req.message_id.trim().is_empty() {
        return Err(AppError::BadRequest(
            "message_id must not be empty".to_string(),
        ));
    }

    if let Some(mode) = req.mode.as_deref() {
        if !matches!(mode, "direct" | "managed" | "branch" | "auto") {
            return Err(AppError::BadRequest(format!(
                "Invalid mode '{mode}'. Expected one of: direct, managed, branch, auto"
            )));
        }
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

    if let Ok(conv) = state.runtime.db().get_conversation(&id).await {
        if let Some(existing_job) = state
            .runtime
            .db()
            .get_conversation_creation_job_for_conversation(&id)
            .await
            .ok()
            .flatten()
        {
            let is_same_create =
                existing_job.message_id.as_deref() == Some(req.message_id.as_str());
            if !is_same_create {
                return Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
                    "conversation_id already belongs to an existing conversation",
                    "conversation_id_exists",
                ))));
            }
            tracing::info!(conversation_id = %id, "Create request hit existing conversation id");
            let mut conversation_json = conversation_to_json(&state, &conv, None);
            inject_creation_job_state_fields(&state, &conv, &mut conversation_json).await;
            state.runtime.kick_creation_worker();
            return Ok(Json(ConversationResponse {
                conversation: conversation_json,
            }));
        }
        return Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
            "conversation_id already belongs to an existing conversation",
            "conversation_id_exists",
        ))));
    }

    if let Ok(Some(existing_job)) = state
        .runtime
        .db()
        .get_conversation_creation_job_for_message(&req.message_id)
        .await
    {
        if let Ok(conv) = state
            .runtime
            .db()
            .get_conversation(&existing_job.conversation_id)
            .await
        {
            tracing::info!(message_id = %req.message_id, "Create request hit existing creation job message id");
            let mut conversation_json = conversation_to_json(&state, &conv, None);
            inject_creation_job_state_fields(&state, &conv, &mut conversation_json).await;
            state.runtime.kick_creation_worker();
            return Ok(Json(ConversationResponse {
                conversation: conversation_json,
            }));
        }
    }

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
        if let Ok(msg) = state.db.get_message_by_id(&req.message_id).await {
            if let Ok(conv) = state
                .runtime
                .db()
                .get_conversation(&msg.conversation_id)
                .await
            {
                return Ok(Json(ConversationResponse {
                    conversation: conversation_to_json(&state, &conv, None),
                }));
            }
        }
    }

    let requested_mode = req.mode.as_deref().unwrap_or("direct");
    let persisted_prompt_text = req.text.clone();

    let short_id: String = id.chars().take(8).collect();
    let slug = format!("conv-{short_id}");
    let conv_mode = crate::db::ConvMode::Direct;
    let effective_cwd = req.cwd.clone();
    let desired_base_branch = req.base_branch.as_deref();
    let registry_default_model = state.llm_registry.default_model_id().to_string();
    let shell_model = req
        .model
        .clone()
        .unwrap_or_else(|| registry_default_model.clone());
    let intent_model = req.model.clone();
    let resolved_mode_for_intent = match requested_mode {
        "direct" => None,
        other => Some(other.to_string()),
    };
    // New conversations are pinned to the current global default LLM language.
    // Once set, this conversation (and all its chain continuations / sub-agents)
    // stays in that language even if the global default later changes.
    //
    // A DB read failure here would silently flip every new conversation back
    // to phoenix-native, hiding a partially-migrated or otherwise unhealthy
    // settings table. Log at warn so operators see it, then fall back —
    // refusing to create the conversation over a *preference* read is too
    // strong a coupling.
    let default_language = match state.runtime.db().get_default_llm_language().await {
        Ok(lang) => lang,
        Err(e) => {
            tracing::warn!(
                error = %e,
                "Failed to read default LLM language from app_settings; falling back to default. \
                 Check that migration 010 ran and app_settings is readable."
            );
            crate::llm_language::LlmLanguage::default()
        }
    };
    let mut stored_file_paths = Vec::new();
    for raw_file in raw_files {
        match store_attachment_bytes(
            &id,
            raw_file.original_name,
            raw_file.media_type,
            raw_file.bytes,
        )
        .await
        {
            Ok(file) => {
                stored_file_paths.push(file.stored_path.clone());
                req.files.push(file);
            }
            Err(e) => {
                delete_files(&stored_file_paths).await;
                return Err(e);
            }
        }
    }
    let validated_files = match validate_submitted_attachments(&id, &req.files).await {
        Ok(files) => files,
        Err(error) => {
            delete_files(&stored_file_paths).await;
            return Err(error);
        }
    };

    let intent = crate::db::ConversationCreationIntent {
        cwd: effective_cwd.clone(),
        model: intent_model,
        text: persisted_prompt_text,
        expansion_preflighted: false,
        llm_text: None,
        skill_invocation: None,
        message_id: req.message_id.clone(),
        images: req
            .images
            .iter()
            .cloned()
            .map(|img| ImageData {
                data: img.data,
                media_type: img.media_type,
            })
            .collect(),
        files: validated_files,
        mode: resolved_mode_for_intent,
        base_branch: req.base_branch.clone(),
        checkout_ref: req.checkout_ref.clone(),
        seed_parent_id: req.seed_parent_id.clone(),
        seed_label: req.seed_label.clone(),
    };
    let job_id = uuid::Uuid::new_v4().to_string();
    let creation_job = crate::db::InsertConversationCreationJob {
        id: job_id.clone(),
        conversation_id: id.clone(),
        message_id: Some(req.message_id.clone()),
        intent,
    };

    let conversation = match state
        .runtime
        .db()
        .create_conversation_with_creation_job(
            &id,
            &slug,
            &effective_cwd,
            true,
            Some(shell_model.as_str()),
            &conv_mode,
            desired_base_branch,
            req.seed_parent_id.as_deref(),
            req.seed_label.as_deref(),
            default_language,
            &creation_job,
        )
        .await
    {
        Ok(conversation) => conversation,
        Err(crate::db::DbError::Sqlx(sqlx::Error::Database(db_err)))
            if db_err.code().as_deref() == Some("2067") =>
        {
            delete_files(&stored_file_paths).await;
            if let Ok(Some(existing_job)) = state
                .runtime
                .db()
                .get_conversation_creation_job_for_message(&req.message_id)
                .await
            {
                let existing_conversation = state
                    .runtime
                    .db()
                    .get_conversation(&existing_job.conversation_id)
                    .await
                    .map_err(|e| AppError::Internal(e.to_string()))?;
                let mut conversation_json =
                    conversation_to_json(&state, &existing_conversation, None);
                inject_creation_job_state_fields(
                    &state,
                    &existing_conversation,
                    &mut conversation_json,
                )
                .await;
                state.runtime.kick_creation_worker();
                return Ok(Json(ConversationResponse {
                    conversation: conversation_json,
                }));
            }
            return Err(AppError::Internal(
                "failed to create conversation shell".to_string(),
            ));
        }
        Err(crate::db::DbError::ConversationAlreadyExists(_)) => {
            delete_files(&stored_file_paths).await;
            let existing_conversation = state
                .runtime
                .db()
                .get_conversation(&id)
                .await
                .map_err(|e| AppError::Internal(e.to_string()))?;
            let existing_job = state
                .runtime
                .db()
                .get_conversation_creation_job_for_conversation(&id)
                .await
                .map_err(|e| AppError::Internal(e.to_string()))?;
            let is_same_create = existing_job
                .as_ref()
                .and_then(|job| job.message_id.as_deref())
                == Some(req.message_id.as_str());
            if is_same_create {
                let mut conversation_json =
                    conversation_to_json(&state, &existing_conversation, None);
                inject_creation_job_state_fields(
                    &state,
                    &existing_conversation,
                    &mut conversation_json,
                )
                .await;
                state.runtime.kick_creation_worker();
                return Ok(Json(ConversationResponse {
                    conversation: conversation_json,
                }));
            }
            return Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
                "conversation_id already belongs to an existing conversation",
                "conversation_id_exists",
            ))));
        }
        Err(e) => {
            delete_files(&stored_file_paths).await;
            return Err(AppError::Internal(e.to_string()));
        }
    };

    state.runtime.kick_creation_worker();

    let mut conversation_json = conversation_to_json(&state, &conversation, None);
    inject_creation_job_state_fields(&state, &conversation, &mut conversation_json).await;
    Ok(Json(ConversationResponse {
        conversation: conversation_json,
    }))
}

#[allow(clippy::too_many_lines)]
async fn create_conversation_with_attachments(
    State(state): State<AppState>,
    multipart: Multipart,
) -> Result<Json<ConversationResponse>, AppError> {
    let (req, raw_files) = read_multipart_create_parts(multipart).await?;
    create_conversation_with_id(state, req, raw_files).await
}

// ============================================================
// Branch Mode Worktree Creation (REQ-PROJ-024)
// ============================================================

pub(crate) struct BranchWorktreeInfo {
    pub(crate) branch_name: String,
    pub(crate) worktree_path: String,
    pub(crate) base_branch: String,
}

pub(crate) enum BranchWorktreeError {
    Conflict { slug: String },
    Git(String),
    BadRequest(String),
}

/// Reject a client-supplied git ref that could be misparsed as a command-line
/// option when it later reaches a `git` invocation (argument injection). Git
/// branch names cannot begin with `-`, so this rejects no legitimate input
/// while closing the `-`-prefixed-ref vector at the HTTP boundary — before the
/// name is interpolated into any `git worktree add` / `rev-parse` argv.
pub(crate) fn validate_user_ref(name: &str) -> Result<(), AppError> {
    if name.starts_with('-') {
        return Err(AppError::BadRequest(format!(
            "Invalid branch name '{name}': must not begin with '-'"
        )));
    }
    Ok(())
}

/// Create a git worktree for an existing branch. Runs on a blocking thread.
///
/// Delegates to `git_ops::{materialize_branch, check_branch_conflict, create_worktree}`.
pub(crate) fn create_branch_worktree_blocking(
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
            return Err(BranchWorktreeError::BadRequest(format!(
                "Branch '{branch}' is already checked out in {location}. \
                 Git doesn't allow a branch to be checked out in two places at once. \
                 Switch to a different branch there first, or use Direct mode."
            )));
        }
    }

    let worktree_path_str = create_worktree(
        cwd,
        conv_id,
        branch_name,
        None,
        PhoenixIgnoreStrategy::StageGitignore,
    )
    .map_err(|e| match e {
        GitOpError::Io(msg) | GitOpError::Git(msg) => BranchWorktreeError::Git(msg),
        GitOpError::BranchNotFound(branch) => BranchWorktreeError::BadRequest(format!(
            "Branch '{branch}' not found locally or at origin"
        )),
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
pub(crate) enum ManagedWorktreeError {
    /// User-input failure (e.g. branch doesn't exist locally or at origin).
    BadRequest(String),
    /// Infrastructure failure (worktree creation, generic git errors).
    Git(String),
}

pub(crate) fn create_managed_explore_worktree_blocking(
    repo_root: &str,
    conv_id: &str,
    base_branch: &str,
    checkout_ref: Option<&str>,
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

    let worktree_path_str = create_worktree(
        cwd,
        conv_id,
        &temp_branch,
        checkout_ref.or(Some(base_branch)),
        PhoenixIgnoreStrategy::StageGitignore,
    )
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

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum StreamInitMode {
    Full,
    MessagesAfterFloor,
}

#[derive(Debug, Deserialize, Default, Clone)]
struct StreamConversationQuery {
    after_event_sequence: Option<i64>,
    #[allow(dead_code)]
    after_sequence: Option<i64>,
    init_mode: Option<StreamInitMode>,
    after_message_floor: Option<i64>,
    transcript_generation: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StreamDbMessageSelection {
    Full,
    AfterFloor(i64),
    None,
}

#[derive(Debug, Deserialize)]
struct LatestMessagesQuery {
    limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct MessageHistoryQuery {
    before_message_sequence: Option<i64>,
    after_message_sequence: Option<i64>,
    limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct MessageRangeQuery {
    start_message_sequence: i64,
    end_message_sequence: i64,
}

#[derive(Debug, Deserialize)]
struct AroundMessagesQuery {
    before: Option<i64>,
    after: Option<i64>,
}

const MAX_EXACT_MESSAGE_RANGE_SPAN: i64 = 10_000;
const MAX_MESSAGE_HISTORY_LIMIT: i64 = 500;
const MAX_RENDER_UNIT_ALIGNED_RESPONSE_MESSAGES: usize = 2_048;

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

    let enriched_msgs: Vec<super::wire::EnrichedMessage> = messages
        .iter()
        .map(super::wire::EnrichedMessage::from)
        .collect();

    let context_window_size = messages
        .iter()
        .filter_map(|m| m.usage_data.as_ref())
        .next_back()
        .map_or(0, crate::db::UsageData::context_window_used);

    Ok(Json(ConversationWithMessagesResponse {
        conversation: conversation_to_json_with_seed(&state, &conversation, true).await?,
        messages: enriched_msgs,
        agent_working: conversation.is_agent_working(),
        presentation_mode: conv_presentation_mode(&conversation).to_string(),
        context_window_size,
    }))
}

async fn get_conversation_meta(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ConversationMetaResponse>, AppError> {
    let conversation = state
        .runtime
        .db()
        .get_conversation(&id)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;

    let context_window_size = state
        .runtime
        .db()
        .get_latest_usage_data(&conversation.id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
        .as_ref()
        .map_or(0, crate::db::UsageData::context_window_used);

    Ok(Json(ConversationMetaResponse {
        conversation: conversation_to_json_with_seed(&state, &conversation, true).await?,
        agent_working: conversation.is_agent_working(),
        presentation_mode: conv_presentation_mode(&conversation).to_string(),
        context_window_size,
    }))
}

fn validate_message_history_limit(
    name: &str,
    value: Option<i64>,
    default: i64,
) -> Result<i64, AppError> {
    let value = value.unwrap_or(default);
    if value <= 0 {
        return Err(AppError::BadRequest(format!(
            "{name} must be greater than 0"
        )));
    }
    Ok(value.min(MAX_MESSAGE_HISTORY_LIMIT))
}

const STABLE_TRANSCRIPT_READ_MAX_ATTEMPTS: usize = 3;

#[derive(Debug, Clone)]
struct StableTranscriptRead<T> {
    conversation: crate::db::Conversation,
    value: T,
    #[cfg(test)]
    attempts: usize,
}

#[cfg(test)]
type StableTranscriptReadTestHook = Box<dyn FnOnce(&str) -> bool>;

#[cfg(test)]
thread_local! {
    static STABLE_TRANSCRIPT_READ_TEST_HOOK: std::cell::RefCell<Option<StableTranscriptReadTestHook>> =
        std::cell::RefCell::new(None);
}

async fn stable_transcript_read<T, F>(
    db: &crate::db::Database,
    conversation_id: &str,
    mut read_value: F,
) -> Result<StableTranscriptRead<T>, AppError>
where
    F: for<'a> FnMut(&'a crate::db::Database, &'a str, usize) -> BoxFuture<'a, Result<T, AppError>>,
{
    for attempt in 1..=STABLE_TRANSCRIPT_READ_MAX_ATTEMPTS {
        let before = db
            .get_conversation(conversation_id)
            .await
            .map_err(|e| AppError::NotFound(e.to_string()))?;
        let value = read_value(db, conversation_id, attempt).await?;
        bump_transcript_generation_for_test(db, conversation_id).await?;
        let after = db
            .get_conversation(conversation_id)
            .await
            .map_err(|e| AppError::NotFound(e.to_string()))?;
        if before.transcript_generation == after.transcript_generation {
            return Ok(StableTranscriptRead {
                conversation: after,
                value,
                #[cfg(test)]
                attempts: attempt,
            });
        }
        tracing::debug!(
            conversation_id,
            attempt,
            before_generation = before.transcript_generation,
            after_generation = after.transcript_generation,
            "discarding transcript read because generation changed during response assembly"
        );
    }

    Err(AppError::Internal(format!(
        "conversation {conversation_id} transcript generation changed during {STABLE_TRANSCRIPT_READ_MAX_ATTEMPTS} read attempts"
    )))
}

#[cfg(test)]
async fn bump_transcript_generation_for_test(
    db: &crate::db::Database,
    conversation_id: &str,
) -> Result<(), AppError> {
    if STABLE_TRANSCRIPT_READ_TEST_HOOK
        .with(|hook| hook.borrow_mut().take())
        .is_some_and(|hook| hook(conversation_id))
    {
        sqlx::query(
            "UPDATE conversations
                 SET transcript_generation = transcript_generation + 1
                 WHERE id = ?1",
        )
        .bind(conversation_id)
        .execute(db.pool())
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    }
    Ok(())
}

#[cfg(not(test))]
#[allow(clippy::unused_async)]
async fn bump_transcript_generation_for_test(
    _db: &crate::db::Database,
    _conversation_id: &str,
) -> Result<(), AppError> {
    Ok(())
}

fn build_message_slice_response(
    messages: &[crate::db::Message],
    transcript_generation: i64,
    server_message_tail: Option<i64>,
    has_older_messages: bool,
) -> ConversationMessageSliceResponse {
    ConversationMessageSliceResponse {
        messages: messages
            .iter()
            .map(super::wire::EnrichedMessage::from)
            .collect(),
        tombstones: vec![],
        transcript_generation: Some(transcript_generation),
        server_message_tail,
        has_older_messages,
    }
}

fn build_message_range_response(
    messages: &[crate::db::Message],
    start_message_sequence: i64,
    end_message_sequence: i64,
    transcript_generation: i64,
    server_message_tail: Option<i64>,
) -> ConversationMessageRangeResponse {
    let present: std::collections::HashSet<i64> = messages.iter().map(|m| m.sequence_id).collect();
    let missing_sequences = (start_message_sequence..=end_message_sequence)
        .filter(|seq| !present.contains(seq))
        .collect();
    ConversationMessageRangeResponse {
        messages: messages
            .iter()
            .map(super::wire::EnrichedMessage::from)
            .collect(),
        missing_sequences,
        tombstones: vec![],
        transcript_generation: Some(transcript_generation),
        server_message_tail,
    }
}

fn build_messages_around_response(
    before: &[crate::db::Message],
    after: &[crate::db::Message],
    transcript_generation: i64,
    server_message_tail: Option<i64>,
) -> ConversationMessagesAroundResponse {
    ConversationMessagesAroundResponse {
        before: before
            .iter()
            .map(super::wire::EnrichedMessage::from)
            .collect(),
        after: after
            .iter()
            .map(super::wire::EnrichedMessage::from)
            .collect(),
        tombstones: vec![],
        transcript_generation: Some(transcript_generation),
        server_message_tail,
    }
}

fn message_starts_render_unit(message: &crate::db::Message) -> bool {
    matches!(
        message.message_type,
        crate::db::MessageType::User
            | crate::db::MessageType::Agent
            | crate::db::MessageType::Skill
    )
}

const RENDER_UNIT_BACKFILL_CHUNK_SIZE: i64 = 64;

fn render_unit_alignment_ceiling_error() -> AppError {
    AppError::TypedBadRequest {
        message: format!(
            "Aligned message slice exceeds the server response ceiling of {MAX_RENDER_UNIT_ALIGNED_RESPONSE_MESSAGES} messages"
        ),
        error_type: "message_slice_render_unit_ceiling_exceeded".to_string(),
    }
}

async fn align_slice_start_to_render_unit(
    db: &crate::db::Database,
    conversation_id: &str,
    messages: &mut Vec<crate::db::Message>,
) -> Result<(), AppError> {
    let Some(first) = messages.first() else {
        return Ok(());
    };
    if message_starts_render_unit(first) {
        return Ok(());
    }

    if messages.len() > MAX_RENDER_UNIT_ALIGNED_RESPONSE_MESSAGES {
        return Err(render_unit_alignment_ceiling_error());
    }

    let mut before_sequence = first.sequence_id;
    let mut intervening = Vec::new();
    let mut remaining_prefix_budget = MAX_RENDER_UNIT_ALIGNED_RESPONSE_MESSAGES - messages.len();
    while remaining_prefix_budget > 0 {
        let fetch_limit = RENDER_UNIT_BACKFILL_CHUNK_SIZE.min(
            i64::try_from(remaining_prefix_budget)
                .map_err(|_| AppError::Internal("render-unit ceiling exceeds i64".to_string()))?,
        );
        let previous = db
            .get_messages_before(conversation_id, before_sequence, fetch_limit)
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        if previous.is_empty() {
            intervening.append(messages);
            *messages = intervening;
            return Ok(());
        }
        remaining_prefix_budget -= previous.len();
        let oldest_sequence = previous[0].sequence_id;

        if let Some(owner_index) = previous.iter().rposition(message_starts_render_unit) {
            let mut prefix = previous.into_iter().skip(owner_index).collect::<Vec<_>>();
            prefix.append(&mut intervening);
            prefix.append(messages);
            *messages = prefix;
            return Ok(());
        }

        let mut older_intervening = previous;
        older_intervening.append(&mut intervening);
        intervening = older_intervening;
        before_sequence = oldest_sequence;
    }

    let has_uncollected_older = if intervening.is_empty() {
        has_messages_before(db, conversation_id, messages).await?
    } else {
        has_messages_before(db, conversation_id, &intervening).await?
    };
    if has_uncollected_older {
        return Err(render_unit_alignment_ceiling_error());
    }

    intervening.append(messages);
    *messages = intervening;
    Ok(())
}

async fn has_messages_before(
    db: &crate::db::Database,
    conversation_id: &str,
    messages: &[crate::db::Message],
) -> Result<bool, AppError> {
    let Some(first) = messages.first() else {
        return Ok(false);
    };
    Ok(!db
        .get_messages_before(conversation_id, first.sequence_id, 1)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
        .is_empty())
}

async fn get_latest_aligned_messages(
    db: &crate::db::Database,
    conversation_id: &str,
    limit: i64,
) -> Result<(Vec<crate::db::Message>, bool), AppError> {
    let requested_count = usize::try_from(limit)
        .map_err(|_| AppError::BadRequest("limit is too large for this server".to_string()))?;
    let mut messages = db
        .get_latest_messages(conversation_id, limit + 1)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    if messages.len() > requested_count {
        messages.remove(0);
    }
    align_slice_start_to_render_unit(db, conversation_id, &mut messages).await?;
    let has_older_messages = has_messages_before(db, conversation_id, &messages).await?;
    Ok((messages, has_older_messages))
}

async fn get_messages_before_aligned(
    db: &crate::db::Database,
    conversation_id: &str,
    before: i64,
    limit: i64,
) -> Result<(Vec<crate::db::Message>, bool), AppError> {
    let requested_count = usize::try_from(limit)
        .map_err(|_| AppError::BadRequest("limit is too large for this server".to_string()))?;
    let mut messages = db
        .get_messages_before(conversation_id, before, limit + 1)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    if messages.len() > requested_count {
        messages.remove(0);
    }
    align_slice_start_to_render_unit(db, conversation_id, &mut messages).await?;
    let has_older_messages = has_messages_before(db, conversation_id, &messages).await?;
    Ok((messages, has_older_messages))
}

async fn get_server_message_tail(
    db: &crate::db::Database,
    conversation_id: &str,
) -> Result<Option<i64>, AppError> {
    let tail = db
        .get_last_sequence_id(conversation_id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok((tail > 0).then_some(tail))
}

async fn get_conversation_messages_latest(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<LatestMessagesQuery>,
) -> Result<Json<ConversationMessageSliceResponse>, AppError> {
    let db = state.runtime.db();
    db.get_conversation(&id)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;
    let limit = validate_message_history_limit("limit", query.limit, 100)?;
    let stable = stable_transcript_read(db, &id, |db, id, _attempt| {
        Box::pin(async move {
            let (messages, has_older_messages) = get_latest_aligned_messages(db, id, limit).await?;
            let server_message_tail = get_server_message_tail(db, id).await?;
            Ok((messages, has_older_messages, server_message_tail))
        })
    })
    .await?;
    let (messages, has_older_messages, server_message_tail) = stable.value;
    Ok(Json(build_message_slice_response(
        &messages,
        stable.conversation.transcript_generation,
        server_message_tail,
        has_older_messages,
    )))
}

async fn get_conversation_messages(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<MessageHistoryQuery>,
) -> Result<Json<ConversationMessageSliceResponse>, AppError> {
    let db = state.runtime.db();
    db.get_conversation(&id)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;
    let limit = validate_message_history_limit("limit", query.limit, 100)?;
    match (query.before_message_sequence, query.after_message_sequence) {
        (Some(_), Some(_)) => Err(AppError::BadRequest(
            "Specify exactly one of before_message_sequence or after_message_sequence".to_string(),
        )),
        (None, None) => Err(AppError::BadRequest(
            "Either before_message_sequence or after_message_sequence is required".to_string(),
        )),
        (Some(before), None) => {
            let stable = stable_transcript_read(db, &id, |db, id, _attempt| {
                Box::pin(async move {
                    let (messages, has_older_messages) =
                        get_messages_before_aligned(db, id, before, limit).await?;
                    let server_message_tail = get_server_message_tail(db, id).await?;
                    Ok((messages, has_older_messages, server_message_tail))
                })
            })
            .await?;
            let (messages, has_older_messages, server_message_tail) = stable.value;
            Ok(Json(build_message_slice_response(
                &messages,
                stable.conversation.transcript_generation,
                server_message_tail,
                has_older_messages,
            )))
        }
        (None, Some(after)) => {
            let stable = stable_transcript_read(db, &id, |db, id, _attempt| {
                Box::pin(async move {
                    let messages = db
                        .get_messages_after_limited(id, after, limit)
                        .await
                        .map_err(|e| AppError::Internal(e.to_string()))?;
                    let server_message_tail = get_server_message_tail(db, id).await?;
                    Ok((messages, server_message_tail))
                })
            })
            .await?;
            let (messages, server_message_tail) = stable.value;
            Ok(Json(build_message_slice_response(
                &messages,
                stable.conversation.transcript_generation,
                server_message_tail,
                false,
            )))
        }
    }
}

async fn get_conversation_message_range(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<MessageRangeQuery>,
) -> Result<Json<ConversationMessageRangeResponse>, AppError> {
    let conversation = state
        .runtime
        .db()
        .get_conversation(&id)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;
    if query.start_message_sequence <= 0 || query.end_message_sequence <= 0 {
        return Err(AppError::BadRequest(
            "start_message_sequence and end_message_sequence must be greater than 0".to_string(),
        ));
    }
    if query.start_message_sequence > query.end_message_sequence {
        return Err(AppError::BadRequest(
            "start_message_sequence must be less than or equal to end_message_sequence".to_string(),
        ));
    }
    let range_span = query.end_message_sequence - query.start_message_sequence + 1;
    if range_span > MAX_EXACT_MESSAGE_RANGE_SPAN {
        return Err(AppError::BadRequest(format!(
            "message range span must be at most {MAX_EXACT_MESSAGE_RANGE_SPAN}"
        )));
    }
    let db = state.runtime.db();
    let messages = db
        .get_message_range(
            &id,
            query.start_message_sequence,
            query.end_message_sequence,
        )
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let server_message_tail = get_server_message_tail(db, &id).await?;
    Ok(Json(build_message_range_response(
        &messages,
        query.start_message_sequence,
        query.end_message_sequence,
        conversation.transcript_generation,
        server_message_tail,
    )))
}

async fn get_conversation_messages_around(
    State(state): State<AppState>,
    Path((id, sequence)): Path<(String, i64)>,
    Query(query): Query<AroundMessagesQuery>,
) -> Result<Json<ConversationMessagesAroundResponse>, AppError> {
    let conversation = state
        .runtime
        .db()
        .get_conversation(&id)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;
    let before = validate_message_history_limit("before", query.before, 50)?;
    let after = validate_message_history_limit("after", query.after, 50)?;
    let db = state.runtime.db();
    let (before_messages, after_messages) = db
        .get_messages_around(&id, sequence, before, after)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let server_message_tail = get_server_message_tail(db, &id).await?;
    Ok(Json(build_messages_around_response(
        &before_messages,
        &after_messages,
        conversation.transcript_generation,
        server_message_tail,
    )))
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

/// `GET /api/work-scope/:scope_key/inventory` — read-projection over the
/// three work-affine registries (bash handles, tmux, browser) for one
/// `WorkScope` (REQ-WSUI-006). `:scope_key` is a `WorkScope::stable_key()`
/// value; it is parsed back into a `WorkScope` to query the registries. The
/// assembly is read-only — it never spawns a process or allocates a registry
/// table for a scope that has none.
async fn get_work_scope_inventory(
    State(state): State<AppState>,
    Path(scope_key): Path<String>,
) -> Result<Json<phoenix_core::domain::work_scope_inventory::WorkScopeInventory>, AppError> {
    let work_scope = crate::work_scope::WorkScope::from_stable_key(&scope_key)
        .ok_or_else(|| AppError::BadRequest(format!("malformed work-scope key: {scope_key}")))?;

    let inventory = phoenix_tools::work_scope_inventory::assemble_inventory(
        &work_scope,
        state.runtime.bash_handles(),
        state.runtime.tmux_registry(),
        state.runtime.browser_sessions(),
    )
    .await;

    Ok(Json(inventory))
}

async fn stop_conversation_browser_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<SuccessResponse>, AppError> {
    let conversation = state
        .runtime
        .db()
        .get_conversation(&id)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;
    let work_scope = crate::work_scope::WorkScope::resolve(
        &conversation.id,
        conversation
            .conv_mode
            .worktree_path()
            .map(std::path::Path::new),
    );

    state
        .runtime
        .browser_sessions()
        .request_kill_session(&work_scope)
        .await;

    let conversation_scope = crate::work_scope::WorkScope::Conversation(conversation.id.clone());
    if conversation_scope != work_scope {
        state
            .runtime
            .browser_sessions()
            .request_kill_session(&conversation_scope)
            .await;
    }

    Ok(Json(SuccessResponse { success: true }))
}

/// `DELETE /api/work-scope/:scope_key/browser-session` — user-initiated
/// lifecycle control for the live browser session owned by one `WorkScope`.
/// Closing the viewer is separate; this terminates Chromium through the same
/// manager path used by delete cascades and idle cleanup, so normal browser and
/// work-scope lifecycle events drive the UI back to not-live.
async fn stop_work_scope_browser_session(
    State(state): State<AppState>,
    Path(scope_key): Path<String>,
) -> Result<Json<SuccessResponse>, AppError> {
    let work_scope = crate::work_scope::WorkScope::from_stable_key(&scope_key)
        .ok_or_else(|| AppError::BadRequest(format!("malformed work-scope key: {scope_key}")))?;

    state
        .runtime
        .browser_sessions()
        .request_kill_session(&work_scope)
        .await;

    Ok(Json(SuccessResponse { success: true }))
}

/// Optional `since=K` incremental output cursor for the inspect endpoint.
#[derive(serde::Deserialize)]
struct InspectQuery {
    since: Option<u64>,
}

/// `GET /api/work-scope/:scope_key/bash/:handle_id/inspect?since=K` — the
/// per-handle drill-down snapshot (`specs/process-inspector/` REQ-PINSP-005).
///
/// Resolves `:scope_key` into a `WorkScope` (400 on a malformed key, like the
/// inventory handler), looks up `:handle_id` in that scope's bash table (404
/// when absent), reads the output window for the optional `since` cursor via
/// the existing ring/tombstone read helpers, and attaches a request-time
/// process-group resource sample iff the handle is live.
async fn inspect_bash_handle(
    State(state): State<AppState>,
    Path((scope_key, handle_id)): Path<(String, String)>,
    Query(query): Query<InspectQuery>,
) -> Result<Json<phoenix_core::domain::process_inspection::BashHandleInspection>, AppError> {
    let work_scope = crate::work_scope::WorkScope::from_stable_key(&scope_key)
        .ok_or_else(|| AppError::BadRequest(format!("malformed work-scope key: {scope_key}")))?;

    let mut assembly = phoenix_tools::process_inspection::assemble_inspection(
        &work_scope,
        &handle_id,
        query.since,
        state.runtime.bash_handles(),
    )
    .await
    .ok_or_else(|| {
        AppError::NotFound(format!(
            "handle {handle_id} not found in work scope {scope_key}"
        ))
    })?;

    // Sample resources only for a live handle (REQ-PINSP-004): a terminal
    // handle has no process group, so `resources` stays `None`.
    if let Some(pgid) = assembly.live_pgid {
        assembly.inspection.resources =
            Some(super::process_sample::sample_process_group(pgid).await);
    }

    Ok(Json(assembly.inspection))
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
    let tasks_dir_name = taskmd_core::discover::discover_or_default(&cwd)
        .to_string_lossy()
        .into_owned();
    // Reflect the real prompt for named sub-agents: a sub-agent (one with a
    // parent) gets the sub-agent suffix, and its persona replaces the base
    // preamble (REQ-AG-006). Without this the inspection endpoint shows a
    // generic prompt exactly for the named-agent feature.
    let is_sub_agent = conversation.parent_conversation_id.is_some();
    let persona = if is_sub_agent {
        state
            .runtime
            .db()
            .get_sub_agent_persona(&id)
            .await
            .ok()
            .flatten()
    } else {
        None
    };
    // Mirror the mode context the live request uses (worktree boundaries,
    // Explore guidance) so the inspected prompt matches what the model sees.
    let mode_context = crate::runtime::conv_mode_to_context(&conversation.conv_mode);
    let explore_bash = if matches!(
        mode_context,
        crate::system_prompt::ModeContext::Explore { .. }
    ) && state.platform.has_sandbox()
    {
        phoenix_core::domain::sm_state::ExploreBashCapability::Sandboxed
    } else {
        phoenix_core::domain::sm_state::ExploreBashCapability::Unavailable
    };
    let system_prompt = crate::system_prompt::build_system_prompt(
        &cwd,
        &tasks_dir_name,
        is_sub_agent,
        Some(&mode_context),
        conversation.llm_language,
        persona.as_deref(),
        explore_bash,
    );

    Ok(Json(SystemPromptResponse { system_prompt }))
}

// ============================================================
// SSE Streaming (REQ-API-005)
// ============================================================

fn snapshot_pending_for_stream(
    broadcast_tx: &crate::runtime::SseBroadcaster,
    query: &StreamConversationQuery,
) -> (i64, bool, i64, Vec<SseEvent>, bool) {
    if let Some(after_event_sequence) = query.after_event_sequence {
        if let Some((anchor, truncated, highest, events)) =
            broadcast_tx.snapshot_pending_after(after_event_sequence)
        {
            (anchor, truncated, highest, events, true)
        } else {
            let server_tip = broadcast_tx.current_seq();
            broadcast_tx.observe_seq(after_event_sequence.min(server_tip));
            let (anchor, truncated, highest, events) = broadcast_tx.snapshot_pending();
            (anchor, truncated, highest, events, false)
        }
    } else {
        let (anchor, truncated, highest, events) = broadcast_tx.snapshot_pending();
        (anchor, truncated, highest, events, false)
    }
}

fn db_message_selection_for_stream(
    cursor_replay_served: bool,
    query: &StreamConversationQuery,
    last_sequence_id: i64,
    transcript_generation: i64,
) -> StreamDbMessageSelection {
    if query
        .transcript_generation
        .is_some_and(|query_generation| query_generation != transcript_generation)
    {
        return StreamDbMessageSelection::Full;
    }

    if cursor_replay_served
        && query.transcript_generation == Some(transcript_generation)
        && query
            .after_event_sequence
            .is_some_and(|after_event_sequence| after_event_sequence >= last_sequence_id)
    {
        return StreamDbMessageSelection::None;
    }

    match (query.init_mode, query.after_message_floor) {
        (Some(StreamInitMode::MessagesAfterFloor), Some(after_floor))
            if query.transcript_generation == Some(transcript_generation) =>
        {
            StreamDbMessageSelection::AfterFloor(after_floor)
        }
        _ => StreamDbMessageSelection::Full,
    }
}

fn stream_state_starts_runtime(state: &ConvState) -> bool {
    !matches!(
        state,
        ConvState::CreationFailed { .. } | ConvState::CreationCancelled { .. }
    )
}

#[allow(clippy::too_many_lines)]
async fn stream_conversation(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<StreamConversationQuery>,
) -> Result<impl IntoResponse, AppError> {
    let conversation_for_runtime = state
        .runtime
        .db()
        .get_conversation(&id)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;

    // Subscribe and snapshot replay coverage before reading DB messages, then
    // read messages last. If a persisted message races stream-open, it is
    // therefore either included in the final DB snapshot or has a sequence_id
    // above the init floor and survives the client's live-event replay guard.
    let runtime_handle = if stream_state_starts_runtime(&conversation_for_runtime.state) {
        Some(state.runtime.get_or_create(&id).await)
    } else {
        None
    };
    let (broadcast_tx, broadcast_rx) = match runtime_handle {
        None => {
            let broadcaster = state.runtime.conversation_broadcaster(&id).await;
            let broadcast_rx = broadcaster.subscribe();
            (broadcaster, broadcast_rx)
        }
        Some(Ok(handle)) => {
            tracing::debug!(
                conv_id = %id,
                receivers_before = handle.broadcast_tx.receiver_count(),
                "SSE client subscribing"
            );
            let broadcast_rx = handle.broadcast_tx.subscribe();
            (handle.broadcast_tx, broadcast_rx)
        }
        Some(Err(e)) if is_invalid_runtime_cwd_error(&e) => {
            tracing::warn!(conv_id = %id, error = %e, "Serving static SSE transcript without starting runtime because persisted cwd is invalid");
            let broadcaster = state.runtime.conversation_broadcaster(&id).await;
            let broadcast_rx = broadcaster.subscribe();
            (broadcaster, broadcast_rx)
        }
        Some(Err(e)) => return Err(AppError::Internal(e)),
    };

    // Snapshot the ReplayRing before the DB message read. The later DB read is
    // the durable catch-up for any persisted Message that commits before init is
    // constructed; live SSE covers events that commit after that read.
    let (
        pending_anchor_sequence_id,
        pending_truncated,
        highest_pending_seq,
        pending_events,
        cursor_replay_served,
    ) = snapshot_pending_for_stream(&broadcast_tx, &query);

    let db = state.runtime.db();
    let stable = stable_transcript_read(db, &id, |db, id, attempt| {
        let query = query.clone();
        Box::pin(async move {
            let last_sequence_id = db.get_last_sequence_id(id).await.unwrap_or(0);
            let mut db_message_selection = db_message_selection_for_stream(
                cursor_replay_served,
                &query,
                last_sequence_id,
                db.get_conversation(id)
                    .await
                    .map_err(|e| AppError::NotFound(e.to_string()))?
                    .transcript_generation,
            );
            if attempt > 1 {
                db_message_selection = StreamDbMessageSelection::Full;
            }
            let messages = match db_message_selection {
                StreamDbMessageSelection::None => Vec::new(),
                StreamDbMessageSelection::Full => db
                    .get_messages(id)
                    .await
                    .map_err(|e| AppError::Internal(e.to_string()))?,
                StreamDbMessageSelection::AfterFloor(after_floor) => db
                    .get_messages_after(id, after_floor)
                    .await
                    .map_err(|e| AppError::Internal(e.to_string()))?,
            };
            Ok((last_sequence_id, db_message_selection, messages))
        })
    })
    .await?;
    let conversation = stable.conversation;
    let (last_sequence_id, db_message_selection, messages) = stable.value;
    let highest_message_seq = match db_message_selection {
        StreamDbMessageSelection::None => last_sequence_id,
        StreamDbMessageSelection::Full | StreamDbMessageSelection::AfterFloor(_) => {
            messages.iter().map(|m| m.sequence_id).max().unwrap_or(0)
        }
    };
    let init_seq = std::cmp::max(
        std::cmp::max(last_sequence_id, highest_pending_seq),
        highest_message_seq,
    );
    broadcast_tx.observe_seq(init_seq);

    let context_window_size = if matches!(db_message_selection, StreamDbMessageSelection::Full) {
        messages
            .iter()
            .filter_map(|m| m.usage_data.as_ref())
            .next_back()
            .map_or(0, crate::db::UsageData::context_window_used)
    } else {
        state
            .runtime
            .db()
            .get_latest_usage_data(&id)
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?
            .as_ref()
            .map_or(0, crate::db::UsageData::context_window_used)
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

    let mut init_conversation = enrich_conversation_with_seed(&state, &conversation, true).await?;
    if matches!(
        conversation.state,
        ConvState::Provisioning { .. }
            | ConvState::CreationFailed { .. }
            | ConvState::CreationCancelled { .. }
    ) {
        if let Ok(Some(job)) = state
            .runtime
            .db()
            .get_conversation_creation_job_for_conversation(&conversation.id)
            .await
        {
            init_conversation.creation_prompt = Some(job.intent.text.clone());
            init_conversation.creation_error = job.error;
        }
    }

    // Create init event with typed data -- serialization deferred to SSE layer
    let init_event = SseEvent::Init {
        sequence_id: init_seq,
        conversation: Box::new(init_conversation),
        transcript_generation: conversation.transcript_generation,
        messages,
        agent_working: conversation.is_agent_working(),
        presentation_mode: conv_presentation_mode(&conversation).to_string(),
        last_sequence_id: init_seq,
        context_window_size,
        project_name,
        pending_anchor_sequence_id,
        pending_events,
        pending_truncated,
    };

    Ok(sse_stream(id, init_event, broadcast_rx))
}

fn is_invalid_runtime_cwd_error(error: &str) -> bool {
    error.contains("has an invalid working directory")
}

// ============================================================
// User Actions (REQ-API-004)
// ============================================================

async fn upload_conversation_attachments(
    State(state): State<AppState>,
    Path(id): Path<String>,
    multipart: Multipart,
) -> Result<Json<AttachmentUploadResponse>, AppError> {
    state
        .runtime
        .db()
        .get_conversation(&id)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;
    let (_metadata, files) = read_multipart_attachments(&id, multipart).await?;
    if files.is_empty() {
        return Err(AppError::BadRequest(
            "No file attachments provided".to_string(),
        ));
    }
    Ok(Json(AttachmentUploadResponse { files }))
}

#[allow(clippy::too_many_lines)]
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
        return Ok(Json(ChatResponse {
            queued: true,
            steering: false,
        }));
    }

    // Expand `@file` inline references before sending to the LLM (REQ-IR-001, REQ-IR-007)
    let conversation = state
        .runtime
        .db()
        .get_conversation(&id)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;
    let steering_queue = state
        .runtime
        .db()
        .get_steering_queue(&id)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;

    // Steering queue idempotency: if message_id is already in the queue a
    // retry returns the same accepted response without double-enqueuing.
    if steering_queue
        .iter()
        .any(|e| e.message_id == req.message_id)
    {
        return Ok(Json(ChatResponse {
            queued: true,
            steering: true,
        }));
    }

    let validated_files = validate_submitted_attachments(&id, &req.files).await?;

    // Route using live runtime state when a handle is present; fall back to
    // the DB row for stable rejection states when no handle is active.
    // See specs/bedrock/design.md FM-7 for the full authority rule.
    let effective_state = if let Some(live_state) =
        state.runtime.effective_conversation_state(&id).await
    {
        // Live handle present — its state_rx is authoritative.
        live_state
    } else {
        // No live handle yet. Stable DB rejection states don't require a
        // runtime to be created — their DB row is always current.
        if let Err(stable_err) = check_user_message_acceptable(&conversation.state) {
            if !matches!(
                stable_err,
                TransitionError::AgentBusy | TransitionError::CancellationInProgress
            ) {
                return Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
                    stable_err.to_string(),
                    match stable_err {
                        TransitionError::ContextExhausted => "context_exhausted",
                        TransitionError::ConversationTerminal => "conversation_terminal",
                        TransitionError::AwaitingTaskApproval => "awaiting_task_approval",
                        TransitionError::AwaitingUserResponse => "awaiting_user_response",
                        TransitionError::AgentBusy => "agent_busy",
                        TransitionError::CancellationInProgress => "cancellation_in_progress",
                        TransitionError::InvalidTransition { .. } => "invalid_state_for_message",
                    },
                ))));
            }
        }
        // DB says Idle (or a transient-busy variant) — materialise the
        // runtime so determine_resume_state derives the true initial state.
        let _handle = state
            .runtime
            .get_or_create(&id)
            .await
            .map_err(AppError::BadRequest)?;
        state
            .runtime
            .effective_conversation_state(&id)
            .await
            .unwrap_or_else(|| conversation.state.clone())
    };
    if let Err(err) = check_user_message_acceptable(&effective_state) {
        // `AgentBusy` and `CancellationInProgress` states are transient — the
        // conversation will reach `Idle` once the current operation completes.
        // Instead of rejecting, queue the message as a steering directive so
        // it is delivered automatically when `Idle` is next entered.
        let steer = matches!(
            err,
            TransitionError::AgentBusy | TransitionError::CancellationInProgress
        );
        if steer {
            // Enforce queue depth limit before accepting.
            const MAX_STEER_QUEUE_DEPTH: usize = 5;
            if steering_queue.len() >= MAX_STEER_QUEUE_DEPTH {
                return Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
                    "Steering queue is full; try again once a queued message has been delivered."
                        .to_string(),
                    "steering_queue_full",
                ))));
            }

            let resolution_root =
                crate::resolution_root::ResolutionRoot::working_dir(&conversation.cwd);
            let expanded =
                crate::message_expander::expand(&req.text, &resolution_root).map_err(|e| {
                    AppError::UnprocessableEntity(ExpansionErrorResponse {
                        error: e.to_string(),
                        error_type: e.error_type().to_string(),
                        reference: e.reference(),
                    })
                })?;
            let images: Vec<ImageData> = req
                .images
                .into_iter()
                .map(|img| ImageData {
                    data: img.data,
                    media_type: img.media_type,
                })
                .collect();
            let files = validated_files.clone();
            let chat_llm_text =
                (expanded.llm_text != expanded.display_text).then_some(expanded.llm_text);
            let display_text = expanded.display_text;
            let steer_event = Event::SteerMessage {
                text: display_text.clone(),
                llm_text: chat_llm_text,
                images,
                files,
                message_id: req.message_id,
                user_agent: req.user_agent,
                skill_invocation: expanded.skill_invocation,
            };
            tracing::info!(
                conv_id = %id,
                state = effective_state.variant_name(),
                "Chat queued as steering message (conversation busy)"
            );
            state
                .runtime
                .enqueue_steer_message(&id, steer_event)
                .await
                .map_err(AppError::BadRequest)?;
            record_pr_auto_fix_context_baseline(state.runtime.db(), &id, &display_text).await?;
            return Ok(Json(ChatResponse {
                queued: true,
                steering: true,
            }));
        }

        let error_type = match err {
            TransitionError::ContextExhausted => "context_exhausted",
            TransitionError::ConversationTerminal => "conversation_terminal",
            TransitionError::AwaitingTaskApproval => "awaiting_task_approval",
            TransitionError::AwaitingUserResponse => "awaiting_user_response",
            TransitionError::AgentBusy => "agent_busy",
            TransitionError::CancellationInProgress => "cancellation_in_progress",
            TransitionError::InvalidTransition { .. } => "invalid_state_for_message",
        };
        tracing::info!(
            conv_id = %id,
            state = effective_state.variant_name(),
            error_type,
            "Chat rejected: conversation state cannot accept UserMessage"
        );
        return Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
            err.to_string(),
            error_type,
        ))));
    }

    let resolution_root = crate::resolution_root::ResolutionRoot::working_dir(&conversation.cwd);
    let expanded = crate::message_expander::expand(&req.text, &resolution_root).map_err(|e| {
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

    let files = validated_files;

    // Only set llm_text when expansion actually changed the text (REQ-IR-001)
    let chat_llm_text = (expanded.llm_text != expanded.display_text).then_some(expanded.llm_text);

    // Send event to runtime with message_id and user_agent.
    // `text` carries the `display_text` (stored in DB, shown in history — REQ-IR-006).
    // `llm_text` is the expanded form delivered to the model when present (REQ-IR-001).
    let display_text = expanded.display_text;
    let event = Event::UserMessage {
        text: display_text.clone(),
        llm_text: chat_llm_text,
        images,
        files,
        message_id: req.message_id,
        user_agent: req.user_agent,
        skill_invocation: expanded.skill_invocation,
    };

    state
        .runtime
        .send_event(&id, event)
        .await
        .map_err(AppError::BadRequest)?;
    record_pr_auto_fix_context_baseline(state.runtime.db(), &id, &display_text).await?;

    Ok(Json(ChatResponse {
        queued: true,
        steering: false,
    }))
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

    if matches!(conversation.state, ConvState::Provisioning { .. }) {
        state
            .runtime
            .db()
            .cancel_conversation_creation(&id, chrono::Utc::now())
            .await
            .map_err(|error| AppError::Internal(error.to_string()))?;
        let cancelled = state
            .runtime
            .db()
            .get_conversation(&id)
            .await
            .map_err(|error| AppError::Internal(error.to_string()))?;
        let broadcast_tx = state.runtime.conversation_broadcaster(&id).await;
        let cancelled_state = cancelled.state.clone();
        let state_updated_at = cancelled.state_updated_at;
        let _ = broadcast_tx.send_seq(|seq| SseEvent::StateChange {
            sequence_id: seq,
            presentation_mode: cancelled_state.presentation_mode().to_string(),
            state: cancelled_state.clone(),
            state_updated_at,
        });
        state
            .runtime
            .evict_runtime(&id, crate::runtime::EvictionReason::CreationProvisioned)
            .await;
        state.runtime.kick_creation_worker();
        return Ok(Json(CancelResponse {
            ok: true,
            no_op: false,
        }));
    }

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

    if !conversation.state.allows_user_cancel() {
        return Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
            format!(
                "Conversation cannot be cancelled while in {} state",
                conversation.state.variant_name()
            ),
            "cannot_cancel_state",
        ))));
    }

    state
        .runtime
        .send_event(
            &id,
            Event::UserCancel {
                reason: None,
                cause: crate::state_machine::event::CancelCause::UserRequested,
            },
        )
        .await
        .map_err(AppError::BadRequest)?;

    Ok(Json(CancelResponse {
        ok: true,
        no_op: false,
    }))
}

/// Upgrade a conversation's model (e.g., from 200k to 1M context).
/// Allowed from `Idle` or `Error` -- cannot upgrade while an LLM request,
/// tool execution, or other operation is in flight (see
/// `ConvState::allows_model_change`).
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

    // Validate conversation exists and is in a state that allows model change
    let conv = state
        .runtime
        .db()
        .get_conversation(&id)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;

    if !conv.state.allows_model_change() {
        return Err(AppError::BadRequest(format!(
            "Cannot change model while conversation is {} -- finish or cancel the current operation first",
            conv.state.variant_name()
        )));
    }

    // Update in DB
    state
        .runtime
        .db()
        .update_conversation_model(&id, &req.model)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    // Evict the active runtime so it gets recreated with the new model
    state
        .runtime
        .evict_runtime(&id, crate::runtime::EvictionReason::ModelUpgrade)
        .await;

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

/// Cancel a specific queued steering message (task 01001).
///
/// Removes the entry with the given `message_id` from the conversation's
/// steering queue and persists the change. Returns 404 if the conversation or
/// message is not found. Returns 200 (success: true) if the message was removed
/// or was already absent (idempotent).
async fn cancel_steering_message(
    State(state): State<AppState>,
    Path((id, message_id)): Path<(String, String)>,
) -> Result<Json<SuccessResponse>, AppError> {
    // 404 if the conversation does not exist; the removal itself is idempotent.
    state
        .runtime
        .db()
        .get_conversation(&id)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;

    // Delete the entry directly (cascading its attachments); a missing entry is
    // a no-op, matching the idempotent contract.
    state
        .runtime
        .db()
        .remove_steering_entries(&id, std::slice::from_ref(&message_id))
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    // Notify the live executor (if running) to remove the entry from its
    // in-memory queue. DB is already updated above, so the executor write
    // in its SteerMessage handler is a no-op if the executor restarts.
    if let Some(handle) = state.runtime.try_get_handle(&id).await {
        let _ = handle
            .event_tx
            .send(Event::CancelSteerMessage {
                message_id: message_id.clone(),
            })
            .await;
    }

    tracing::info!(conv_id = %id, %message_id, "Steering message cancelled");

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

async fn dismiss_question(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<SuccessResponse>, AppError> {
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

    state
        .runtime
        .send_event(&id, Event::UserQuestionDismissed)
        .await
        .map_err(AppError::BadRequest)?;

    Ok(Json(SuccessResponse { success: true }))
}

/// Dismiss a persisted, user-resumable `Error` state, returning the
/// conversation to `Idle`. Server-authoritative counterpart to the error
/// banner's "Dismiss" button: the client sends this instead of faking the idle
/// phase locally, so the displayed state and the server state cannot diverge.
///
/// Only user-resumable errors are dismissable — a non-resumable error is a dead
/// end the policy says to abandon for a new conversation, and returning it to
/// Idle would reopen the resume path the policy denies.
async fn dismiss_error(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<SuccessResponse>, AppError> {
    let conv = state
        .runtime
        .db()
        .get_conversation(&id)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;

    match &conv.state {
        ConvState::Error { error_kind, .. } if error_kind.is_user_resumable() => {}
        ConvState::Error { .. } => {
            return Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
                "This error is not dismissable; start a new conversation to continue",
                "non_resumable_error",
            ))));
        }
        _ => {
            return Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
                "Conversation is not in an error state",
                "wrong_state",
            ))));
        }
    }

    state
        .runtime
        .send_event(&id, Event::DismissError)
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
    run_archive_cascade(&state, &id).await?;
    Ok(Json(SuccessResponse { success: true }))
}

/// Archive cascade: same resource cleanup as hard-delete (bash kill, tmux
/// kill, worktree/branch removal) but flips `archived = 1` instead of
/// deleting the row. "Done-but-keep-history": the conversation, its
/// messages, and tool calls remain queryable; live resources are gone.
///
/// Rejects busy conversations with the same `cancel_first` 409 as
/// hard-delete — cleanup would race in-flight tool execution otherwise.
/// Resource cleanup failures (bash / tmux / worktree) log WARN and
/// continue; only the final `archived = 1` write is fatal.
pub(super) async fn run_archive_cascade(state: &AppState, id: &str) -> Result<(), AppError> {
    let conv = state
        .runtime
        .db()
        .get_conversation(id)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;

    if conv.state.is_busy() {
        return Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
            "Cannot archive a busy conversation. Cancel the in-flight \
             operation first, then retry.",
            "cancel_first",
        ))));
    }

    run_resource_cleanup_cascade(state, &conv).await?;

    state
        .runtime
        .db()
        .archive_conversation(id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to set archived flag: {e}")))?;

    Ok(())
}

/// Decide whether `work_scope` is still owned by a live conversation OTHER
/// THAN `conv` once `conv` goes away — the preservation signal shared by every
/// scope-keyed cascade (bash, tmux, terminal, browser). True when EITHER a
/// continuation inherits the same scope OR a live sibling resolves to it.
///
/// A Work-mode sub-agent inherits its parent's `conv_mode`, so it resolves to
/// the parent's `WorkScope` but has no continuation. The continuation check
/// alone would yield `false` and SIGKILL the still-open parent's resources;
/// the live-sibling check is what preserves them.
///
/// The deleted conversation is EXCLUDED from the live-owner enumeration: the
/// cascade runs before the terminal-state write, so `conv` still reads
/// non-terminal in the DB — without excluding it the scope would always look
/// owned and never tear down.
///
/// Returns `Err` only when a set `continued_in_conv_id` cannot be resolved
/// (DB error). Without the continuation's scope the preservation decision
/// can't be made, so the cascade is refused; the caller retries once the DB
/// is healthy. This runs BEFORE any cleanup side effect, so an early return
/// leaves no partial state.
async fn scope_still_owned_after_delete(
    state: &AppState,
    conv: &crate::db::Conversation,
    work_scope: &crate::work_scope::WorkScope,
) -> Result<bool, AppError> {
    let id = conv.id.as_str();

    let continuation_inherits_scope = if let Some(cont_id) = conv.continued_in_conv_id.as_deref() {
        match state.runtime.db().get_conversation(cont_id).await {
            Ok(continuation) => {
                let cont_scope = crate::work_scope::WorkScope::resolve(
                    &continuation.id,
                    continuation
                        .conv_mode
                        .worktree_path()
                        .map(std::path::Path::new),
                );
                &cont_scope == work_scope
            }
            Err(e) => {
                return Err(AppError::Internal(format!(
                    "cleanup cascade refused: continuation lookup failed for \
                     conv={id} continuation={cont_id}: {e} \
                     (preservation decision requires the inheritor's scope; \
                     retry once the DB is healthy)"
                )));
            }
        }
    } else {
        false
    };

    if continuation_inherits_scope {
        return Ok(true);
    }

    // The cleanup cascade fails loud: an unreadable DB makes the liveness of a
    // sibling owner unknowable. Proceeding on a fail-closed "assume live" would
    // skip every resource + worktree teardown while the row is still
    // archived/deleted, orphaning those resources with no retry. Failing the
    // request instead lets the caller retry once the DB is healthy.
    state
        .runtime
        .scope_has_live_conversation_excluding(work_scope, id)
        .await
        .map_err(|e| {
            AppError::Internal(format!(
                "cleanup cascade refused: sibling liveness lookup failed for \
                 conv={id} scope={work_scope}: {e} \
                 (proceeding would skip resource + worktree teardown while \
                 archiving the row; retry once the DB is healthy)"
            ))
        })
}

/// REQ-BED-032 steps 2-5 (bash + tmux + projects + browser cleanup),
/// factored out so hard-delete, archive, abandon, and mark-merged share the
/// exact same resource teardown. Authoritative failures (worktree-removal,
/// bash kill, tmux kill-server, browser kill) log WARN with the fields
/// needed for manual cleanup; best-effort branch deletion inside
/// `cascade_projects_on_delete` logs at DEBUG and does NOT populate
/// `project_report.error` — branch cleanup is opportunistic, not
/// authoritative. Callers own the final DB write and any state-machine
/// transition.
///
/// Returns `Err` only when the continuation `WorkScope` cannot be
/// resolved (DB error on `get_conversation`) and `continued_in_conv_id`
/// was set. Without a known inheritor scope, the cascade cannot make
/// the preservation decision, so the only defensible response is to
/// refuse the cascade entirely so the caller can retry once the DB is
/// healthy. All cleanup side effects (kill, unlink, fs remove) run AFTER
/// this lookup, so an early return here leaves no partial state.
///
/// The `WorkScope` is resolved once from `conv` and passed to every
/// scope-keyed cascade (bash, tmux, terminal, browser) so the orchestrator
/// owns the single derivation point. Preservation passes
/// `inheritor_scope = Some(work_scope)` iff the scope is still owned by a
/// live (non-terminal) conversation OTHER THAN `conv` — either a
/// continuation that inherits the same scope, or a live sibling (e.g. a
/// Work-mode sub-agent sharing its parent's scope, or vice versa). When the
/// last live conversation on the scope is the one being deleted, every
/// cascade tears down. Projects retains a conv-shaped API: it inspects
/// `conv.conv_mode` for the branch/worktree mode discriminant.
pub(super) async fn run_resource_cleanup_cascade(
    state: &AppState,
    conv: &crate::db::Conversation,
) -> Result<(), AppError> {
    let id = conv.id.as_str();
    let work_scope = crate::work_scope::WorkScope::resolve(
        &conv.id,
        conv.conv_mode.worktree_path().map(std::path::Path::new),
    );

    // `inheritor_scope = Some(work_scope)` means "preserve"; `None` means
    // "tear down". Threaded to every scope-keyed cascade (bash, tmux,
    // terminal, browser) so they all honor the same any-live-owner signal.
    let scope_still_owned = scope_still_owned_after_delete(state, conv, &work_scope).await?;
    let inheritor_scope: Option<&crate::work_scope::WorkScope> =
        scope_still_owned.then_some(&work_scope);

    // Step 2: bash handles. Preserve iff the scope is still owned by a live
    // conversation other than this one (REQ-BASH-WS-002).
    let bash_report = crate::tools::bash::registry::cascade_bash_on_delete(
        state.runtime.bash_handles(),
        &work_scope,
        inheritor_scope,
    )
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
        tracing::debug!(
            conv_id = %id,
            live_handle_pids = ?bash_report.live_handle_pids,
            live_handle_pgids = ?bash_report.live_handle_pgids,
            kill_pending_kernel_pids = ?bash_report.kill_pending_kernel_pids,
            "bash cascade: SIGKILL'd live process groups"
        );
    }

    // Step 3: tmux server. Preserve iff the scope is still owned by a live
    // conversation other than this one (REQ-TMUX-WS-002).
    let tmux_report = crate::tools::tmux::registry::cascade_tmux_on_delete(
        state.runtime.tmux_registry(),
        &work_scope,
        inheritor_scope,
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

    // Step 4: terminal PTY. Same any-live-owner preservation rule
    // (REQ-TERM-WS-001, REQ-TERM-012). Sub-agent / no-terminal scopes
    // hit the no-op fast path inside the cascade — registry miss is the
    // common case during conversation cleanup.
    crate::terminal::cascade_terminal_on_delete(&state.terminals, &work_scope, inheritor_scope)
        .await;

    // Step 5: project worktree. Preserve iff the scope is still owned by a
    // live conversation other than this one — a Work sub-agent inherits the
    // parent's `worktree_path`, so removing the worktree / deleting the
    // branch here would destroy the live parent's checkout (REQ-PROJ-029).
    let project_report = cascade_projects_on_delete(state, conv, inheritor_scope).await;
    if let Some(err) = &project_report.error {
        tracing::warn!(
            conv_id = %id,
            worktree_path = ?project_report.worktree_path,
            branch_name = ?project_report.branch_name,
            error = %err,
            "project cleanup failed; orphan worktree/branch may remain"
        );
    }

    // Step 6: browser session. Same any-live-owner rule as tmux
    // (REQ-BROWSER-WS-002, REQ-BROWSER-WS-003).
    crate::tools::browser::session::cascade_browser_on_delete(
        state.runtime.browser_sessions(),
        &work_scope,
        inheritor_scope,
    )
    .await;

    Ok(())
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

    if matches!(
        conv.state,
        ConvState::Provisioning { .. }
            | ConvState::CreationCancelled { .. }
            | ConvState::CreationFailed { .. }
    ) {
        state
            .runtime
            .db()
            .request_conversation_creation_deletion(id, chrono::Utc::now())
            .await
            .map_err(|error| AppError::Internal(error.to_string()))?;
        state
            .runtime
            .evict_runtime(id, crate::runtime::EvictionReason::CreationProvisioned)
            .await;
        delete_conversation_attachments(id).await;
        broadcast_conversation_hard_deleted(state, id).await;
        state.runtime.kick_creation_worker();
        return Ok(());
    }

    if conv.state.is_busy() {
        return Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
            "Cannot hard-delete a busy conversation. Cancel the in-flight \
             operation first, then retry.",
            "cancel_first",
        ))));
    }

    // ForkProposalsRemovedOnOriginDelete (REQ-PROJ-035): dismiss every
    // still-`pending` proposal bound to this origin and clean its deterministic
    // spawn/promote git orphan — BEFORE the long resource-cleanup teardown opens
    // its window. Dismissing under the fork actor first is what makes a
    // fork-from-a-being-deleted-origin structurally impossible: an approve /
    // request-changes that races the cascade enters the actor, finds the proposal
    // non-`pending`, and aborts before creating a worktree. Ordering this AFTER
    // the long teardown would leave a window where a concurrent approve sees the
    // origin still live + proposal pending and spawns a child the later cleanup
    // then skips as resolved. A pending proposal's deterministic path can only be
    // a crashed-approve orphan; a spawned/promoted proposal's same path is the
    // LIVE decoupled fork/refinement, which survives origin deletion and is NOT
    // touched. The proposal ROWS are removed by the fork_proposals.origin_conv_id
    // ON DELETE CASCADE on the row deletion below — not duplicated here.
    cleanup_pending_fork_orphans_on_delete(state, &conv).await;

    // Steps 2-5: bash handles, tmux server, project worktree, browser
    // session. Cleanup-step failures log WARN and continue; the only
    // fatal error from this call is a continuation-row DB lookup failure
    // (returned as 500 so the user can retry). Shared with archive /
    // abandon / mark-merged so the resource teardown is byte-for-byte
    // identical.
    run_resource_cleanup_cascade(state, &conv).await?;

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

    delete_conversation_attachments(id).await;

    broadcast_conversation_hard_deleted(state, id).await;

    Ok(())
}

async fn broadcast_conversation_hard_deleted(state: &AppState, id: &str) {
    if let Some(handle) = state.runtime.try_get_handle(id).await {
        let conv_id = id.to_string();
        let _ = handle
            .broadcast_tx
            .send_seq(|seq| SseEvent::ConversationHardDeleted {
                sequence_id: seq,
                conversation_id: conv_id,
            });
    }
    if let Some(tx) = state.runtime.take_evicted_broadcaster(id).await {
        let conv_id = id.to_string();
        let _ = tx.send_seq(|seq| SseEvent::ConversationHardDeleted {
            sequence_id: seq,
            conversation_id: conv_id,
        });
    }
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
///
/// `inheritor_scope = Some(_)` means a live conversation OTHER THAN `conv`
/// still owns the same `WorkScope` (e.g. a Work-mode sub-agent inherits the
/// parent's `worktree_path`, so the parent shares the scope). In that case
/// the worktree is still in use — removing it or deleting the branch would
/// destroy the live owner's checkout — so this cascade is a no-op and
/// reports no worktree work (REQ-PROJ-029). When `None`, the conversation
/// being deleted is the last owner and the worktree/branch are reaped as
/// usual.
/// The `(branch_name, worktree_path, is_work_mode)` cleanup target for a
/// conversation, or `None` when there is no worktree/branch to reap (Direct,
/// or Explore with no worktree). `is_work_mode` gates the `branch -D`.
fn cascade_project_target(conv: &crate::db::Conversation) -> Option<(String, String, bool)> {
    match &conv.conv_mode {
        ConvMode::Work {
            branch_name,
            worktree_path,
            ..
        } => Some((branch_name.to_string(), worktree_path.to_string(), true)),
        ConvMode::Branch {
            branch_name,
            worktree_path,
            ..
        } => Some((branch_name.to_string(), worktree_path.to_string(), false)),
        ConvMode::Explore {
            worktree_path: Some(wt),
            ..
        } => {
            // Top-level managed Explore: temp branch follows the REQ-PROJ-028
            // naming scheme. `is_work_mode = true` so the blocking closure
            // also runs `branch -D` on it.
            let id_prefix: String = conv.id.chars().take(8).collect();
            Some((format!("task-pending-{id_prefix}"), wt.to_string(), true))
        }
        ConvMode::Direct
        | ConvMode::Explore {
            worktree_path: None,
            ..
        } => None,
    }
}

async fn cascade_projects_on_delete(
    state: &AppState,
    conv: &crate::db::Conversation,
    inheritor_scope: Option<&crate::work_scope::WorkScope>,
) -> CascadeProjectsReport {
    if inheritor_scope.is_some() {
        tracing::debug!(
            conv_id = %conv.id,
            "skipping worktree/branch cleanup -- scope still owned by a live conversation"
        );
        return CascadeProjectsReport::default();
    }

    // Chain-member preservation: if this conversation has a successor in
    // a continuation chain, the worktree + branch are shared with that
    // successor -- only the leaf (continued_in_conv_id = None) actually
    // owns them. Skip cleanup here; the leaf's cascade will handle it.
    // Mirrors the tmux preservation logic at tools/tmux/registry.rs:456,
    // making chain delete/archive correct by construction: no redundant
    // worktree-remove or branch-D calls walking the chain, and no race
    // window where root's cascade tears down resources the leaf is using.
    //
    // Per-conv archive/delete/unarchive are gated by `refuse_if_chain_member`
    // so for those callers `continued_in_conv_id` is None and this check
    // is a no-op. Abandon and mark-merged are gated by `reject_if_continued`
    // (leaf-only), so this is also a no-op there.
    if conv.continued_in_conv_id.is_some() {
        tracing::debug!(
            conv_id = %conv.id,
            continuation = %conv.continued_in_conv_id.as_deref().unwrap_or(""),
            "skipping worktree/branch cleanup -- transferred to continuation"
        );
        return CascadeProjectsReport::default();
    }

    let Some((branch_name, worktree_path, is_work_mode)) = cascade_project_target(conv) else {
        return CascadeProjectsReport::default();
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

/// `ForkProposalsRemovedOnOriginDelete` (REQ-PROJ-035): on hard-delete of a fork
/// origin, enqueue a `CleanupOnHardDelete` command on the single serialized
/// fork-resolution consumer and await it. The consumer dismisses every
/// still-`pending` proposal bound to the origin and cleans its deterministic
/// spawn/promote git orphan — guarded to STILL-PENDING proposals, since a
/// spawned/promoted proposal's deterministic path is the LIVE decoupled
/// fork/refinement (which must survive origin deletion). The proposal rows
/// themselves are removed by the `fork_proposals.origin_conv_id` ON DELETE CASCADE
/// when the conversation row is deleted below.
///
/// Because the consumer is single-threaded, dismissing the pending proposals here
/// makes a fork-from-a-deleted-origin structurally impossible: any
/// approve/request-changes queued behind this command runs after it, finds the
/// proposal non-`pending`, and aborts before creating a worktree.
async fn cleanup_pending_fork_orphans_on_delete(state: &AppState, conv: &crate::db::Conversation) {
    state
        .runtime
        .cleanup_pending_fork_orphans_on_delete(&conv.id)
        .await;
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
    let conversation = enrich_conversation_with_seed(&state, &conversation, true).await?;

    Ok(Json(ConversationResponse {
        conversation: serde_json::to_value(conversation).unwrap_or(Value::Null),
    }))
}

async fn regenerate_conversation_name(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ConversationResponse>, AppError> {
    let opening = state
        .runtime
        .db()
        .first_opening_message_text(&id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
        .ok_or_else(|| {
            tracing::debug!(
                conv_id = %id,
                "conversation regenerate-name: no opening message; leaving slug unchanged"
            );
            AppError::Internal(
                "cannot regenerate conversation name: no opening message to summarize".to_string(),
            )
        })?;

    let cheap_model = state.llm_registry.get_cheap_model().ok_or_else(|| {
        AppError::Internal("no cheap LLM model is available for name regeneration".to_string())
    })?;

    let generated = crate::title_generator::generate_title(&opening, cheap_model)
        .await
        .filter(|slug| !slug.is_empty())
        .ok_or_else(|| {
            AppError::Internal(
                "conversation name regeneration failed — the existing name is unchanged"
                    .to_string(),
            )
        })?;

    rename_conversation_slug(&state, &id, &generated).await?;
    conversation_response(&state, &id).await
}

async fn rename_conversation_slug(state: &AppState, id: &str, slug: &str) -> Result<(), AppError> {
    state
        .runtime
        .db()
        .rename_conversation(id, slug)
        .await
        .map_err(|e| match e {
            crate::db::DbError::SlugExists(_) => {
                AppError::BadRequest("Slug already exists".to_string())
            }
            crate::db::DbError::ConversationNotFound(_) => AppError::NotFound(e.to_string()),
            _ => AppError::Internal(e.to_string()),
        })
}

async fn conversation_response(
    state: &AppState,
    id: &str,
) -> Result<Json<ConversationResponse>, AppError> {
    let conversation = state
        .runtime
        .db()
        .get_conversation(id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    Ok(Json(ConversationResponse {
        conversation: serde_json::to_value(conversation).unwrap_or(Value::Null),
    }))
}

pub(crate) fn title_from_text(text: &str) -> String {
    text.split_whitespace()
        .take(8)
        .collect::<Vec<_>>()
        .join(" ")
}

// ============================================================
// One-shot command suggestion
// ============================================================

/// `POST /api/suggest` — return suggested shell commands for a natural-language
/// query. Stateless and tool-less: a single LLM completion, nothing executed or
/// persisted server-side.
async fn suggest_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<SuggestRequest>,
) -> Result<Json<SuggestResponse>, AppError> {
    // Capability-token gate: the endpoint is exempt from the password
    // middleware, so authorization rests entirely on the scoped token the
    // server injected into the PTY env (held by `phx`, never the password).
    let provided = headers
        .get("x-phoenix-suggest-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    // Constant-time compare, matching the password path: an empty provided token
    // is rejected up front so it can never match an (unexpected) empty server token.
    if provided.is_empty()
        || !super::auth::constant_time_eq(provided.as_bytes(), state.suggest_token.as_bytes())
    {
        return Err(AppError::Forbidden(
            "invalid or missing suggest token".to_string(),
        ));
    }

    let query = req.query.trim();
    if query.is_empty() {
        return Err(AppError::BadRequest("query must not be empty".to_string()));
    }

    let llm = match &req.model {
        Some(id) => state.llm_registry.get(id),
        None => state.llm_registry.get_cheap_model(),
    }
    .ok_or_else(|| AppError::Internal("no LLM model available".to_string()))?;

    let commands = crate::suggest::suggest_commands(query, llm)
        .await
        .map_err(AppError::Internal)?;

    Ok(Json(SuggestResponse { commands }))
}

// ============================================================
// Slug Resolution (REQ-API-007)
// ============================================================

async fn get_by_slug_meta(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> Result<Json<ConversationMetaResponse>, AppError> {
    let conversation = state
        .runtime
        .db()
        .get_conversation_by_slug(&slug)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;

    let context_window_size = state
        .runtime
        .db()
        .get_latest_usage_data(&conversation.id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
        .as_ref()
        .map_or(0, crate::db::UsageData::context_window_used);

    Ok(Json(ConversationMetaResponse {
        conversation: conversation_to_json_with_seed(&state, &conversation, true).await?,
        agent_working: conversation.is_agent_working(),
        presentation_mode: conv_presentation_mode(&conversation).to_string(),
        context_window_size,
    }))
}

async fn get_by_slug(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> Result<Json<ConversationWithMessagesResponse>, AppError> {
    let conversation = match state.runtime.db().get_conversation_by_slug(&slug).await {
        Ok(conversation) => conversation,
        Err(DbError::ConversationNotFound(_)) => state
            .runtime
            .db()
            .get_conversation(&slug)
            .await
            .map_err(|e| AppError::NotFound(e.to_string()))?,
        Err(e) => return Err(AppError::NotFound(e.to_string())),
    };

    let messages = state
        .runtime
        .db()
        .get_messages(&conversation.id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let enriched_msgs: Vec<super::wire::EnrichedMessage> = messages
        .iter()
        .map(super::wire::EnrichedMessage::from)
        .collect();

    let context_window_size = messages
        .iter()
        .filter_map(|m| m.usage_data.as_ref())
        .next_back()
        .map_or(0, crate::db::UsageData::context_window_used);

    Ok(Json(ConversationWithMessagesResponse {
        conversation: conversation_to_json_with_seed(&state, &conversation, true).await?,
        messages: enriched_msgs,
        agent_working: conversation.is_agent_working(),
        presentation_mode: conv_presentation_mode(&conversation).to_string(),
        context_window_size,
    }))
}

// ============================================================
// Directory Browser (REQ-API-008)
// ============================================================

#[derive(Debug, Deserialize)]
struct PathQuery {
    path: String,
    /// Optional project working directory whose task/skill subtrees should be
    /// readable even before a conversation exists for it. Only `read_file`
    /// honors this, and only to widen the allowlist by `<cwd>/tasks`,
    /// `<cwd>/.claude/skills`, `<cwd>/.agents/skills` — never the whole cwd.
    #[serde(default)]
    cwd: Option<String>,
}

/// Query for `serve_preview_file`: an optional `cwd` mirroring `read_file`'s, so
/// a file admitted only by the cwd-widened allowlist is also previewable.
#[derive(serde::Deserialize)]
struct PreviewQuery {
    #[serde(default)]
    cwd: Option<String>,
}

// `validate_cwd` and `list_directory` power the new-conversation directory
// picker, which browses the filesystem to choose a cwd *before* any conversation
// (and thus any preview root) exists. Confining them to `preview_roots()` would
// make new-conversation creation impossible, so they are intentionally NOT
// root-confined. They return only directory names and a git/exists boolean — no
// file contents — so the recon surface is bounded to directory structure, which
// the picker exists to expose. Content-reading handlers (`read_file`,
// `list_files`) ARE confined via `canonicalize_within_roots`.
async fn validate_cwd(Query(query): Query<PathQuery>) -> Json<ValidateCwdResponse> {
    // Normalize path: remove trailing slashes (except for root)
    let path_str = query.path.trim_end_matches('/');
    let path_str = if path_str.is_empty() { "/" } else { path_str };
    match crate::conversation_cwd::validate_conversation_cwd(path_str) {
        Ok(valid) => {
            let is_git = phoenix_core::git::detect_git_repo_root(valid.as_path()).is_some();
            Json(ValidateCwdResponse {
                valid: true,
                error: None,
                is_git,
            })
        }
        Err(error) => Json(ValidateCwdResponse {
            valid: false,
            error: Some(error.to_string()),
            is_git: false,
        }),
    }
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

/// Whether `path` is a safe `POST /api/mkdir` target: confined to `$HOME` or
/// `/tmp`.
///
/// A raw string prefix on a non-canonicalized path is bypassable two ways:
/// `..` traversal (`/tmp/../etc/x` string-matches `/tmp/`, but `create_dir_all`
/// resolves the `..` at the OS level and escapes) and sibling-prefix
/// (`/home/userevil` string-matches `/home/user`). This rejects `..` components
/// outright, then requires the nearest EXISTING ancestor — canonicalized so
/// symlinks in the existing portion are resolved — to live under a canonical
/// allowed root via component-wise `Path::starts_with`. With no `..` and a
/// contained anchor, the not-yet-existing leaf components only extend downward,
/// so the created directory stays confined.
fn mkdir_target_is_confined(path: &FsPath, home: &FsPath) -> bool {
    if path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return false;
    }

    let allowed_roots: Vec<PathBuf> = [home.to_path_buf(), PathBuf::from("/tmp")]
        .into_iter()
        .filter_map(|root| fs::canonicalize(root).ok())
        .collect();

    path.ancestors()
        .find(|a| a.exists())
        .and_then(|anchor| fs::canonicalize(anchor).ok())
        .is_some_and(|anchor| allowed_roots.iter().any(|root| anchor.starts_with(root)))
}

/// Create a directory (with parents if needed)
async fn mkdir(
    State(state): State<AppState>,
    Json(payload): Json<PathQuery>,
) -> Json<MkdirResponse> {
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

    // Confine creation to $HOME or /tmp (see [`mkdir_target_is_confined`]).
    let home = state.runtime_env.home();
    if !mkdir_target_is_confined(&path, home) {
        let home = home.to_string_lossy();
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

/// Check if file content appears to be valid text
fn is_valid_text(content: &[u8]) -> bool {
    if content.contains(&0) {
        return false;
    }

    std::str::from_utf8(content).is_ok()
}

fn preview_url_for_path(path: &std::path::Path) -> String {
    format!("/preview{}", percent_encode_path(path))
}

fn percent_encode_path(path: &std::path::Path) -> String {
    use std::fmt::Write;

    let path_str = path.to_string_lossy().replace('\\', "/");
    let mut out = String::with_capacity(path_str.len());
    for &b in path_str.as_bytes() {
        if b == b'/' || b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

/// Percent-encode a URL query-parameter value (e.g. the `cwd` carried on a
/// `/preview` image URL). Unlike [`percent_encode_path`], `/` is encoded too,
/// since this is a single opaque value, not a path.
fn percent_encode_query_value(s: &str) -> String {
    use std::fmt::Write;

    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

/// Canonicalize `requested` and confine it to a directory Phoenix is actively
/// serving. The allowed set is every conversation working directory
/// ([`Database::preview_roots`]) plus the user's `~/.claude` config tree, which
/// holds globally-discovered skills/tasks the file viewer legitimately reads.
///
/// This mirrors the containment in [`serve_preview_file`]: canonicalize first so
/// `.`, `..`, and symlinks are resolved before the `starts_with` check, then
/// require the resolved path to live under some allowed root. A path that does
/// not resolve, or resolves outside every root, is reported as `NotFound` —
/// indistinguishable from out-of-scope, so existence is never leaked.
async fn canonicalize_within_roots(
    state: &AppState,
    requested: &std::path::Path,
) -> Result<PathBuf, AppError> {
    canonicalize_within_roots_with_cwd(state, requested, None).await
}

/// As [`canonicalize_within_roots`], but additionally admits the task/skill
/// subtrees of `cwd` (see [`read_root_allowlist`]). `cwd == None` is identical
/// to [`canonicalize_within_roots`]; a provided cwd only ever widens the set by
/// the three bounded subtrees, never by arbitrary directories.
async fn canonicalize_within_roots_with_cwd(
    state: &AppState,
    requested: &std::path::Path,
    cwd: Option<&str>,
) -> Result<PathBuf, AppError> {
    let path = fs::canonicalize(requested)
        .map_err(|_| AppError::NotFound("Path does not exist".to_string()))?;

    let roots = read_root_allowlist(state, cwd).await;

    if roots.iter().any(|root| path.starts_with(root)) {
        Ok(path)
    } else {
        Err(AppError::NotFound("Path does not exist".to_string()))
    }
}

/// The canonicalized set of directories Phoenix will serve files from: every
/// conversation working directory ([`Database::preview_roots`]) plus the two
/// globally-discovered skill trees.
///
/// Both [`canonicalize_within_roots`] (file viewer / `read_file` / `list_files`)
/// and [`serve_preview_file`] (the `/preview/*` HTTP handler) MUST consult the
/// same allowlist: `read_file` hands the UI a `/preview<path>` URL for any file
/// it admits, and the follow-up preview request would 404 if the two disagreed.
///
/// The skill trees are included because globally-discovered skills resolve
/// outside any conversation cwd, so the viewer must be able to read them. This
/// is scoped to the skill directories ONLY — emphatically NOT all of `~/.claude`,
/// which also holds `.credentials.json`, settings, and conversation history that
/// must never be readable here.
async fn preview_root_allowlist(state: &AppState) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = state
        .db
        .preview_roots()
        .await
        .unwrap_or_default()
        .iter()
        .filter_map(|root| fs::canonicalize(root).ok())
        .collect();

    let home = state.runtime_env.home();
    for skill_root in [
        home.join(".claude").join("skills"),
        home.join(".agents").join("skills"),
        state.runtime_env.builtin_skills_dir(),
    ] {
        if let Ok(dir) = fs::canonicalize(skill_root) {
            roots.push(dir);
        }
    }

    roots
}

/// The base [`preview_root_allowlist`] widened with the task/skill subtrees of an
/// optional `cwd` — used by `read_file` so a project's tasks and skills are
/// readable *before* any conversation exists for that project (and thus before
/// the project dir is in [`Database::preview_roots`]). The new-conversation UI
/// enumerates tasks (`/api/tasks?cwd=…`) and skills (`/api/skills?cwd=…`) for a
/// freshly-picked directory, then opens a selected file via `/api/files/read`.
///
/// The widening is deliberately scoped to three subtrees of the provided cwd —
/// the project's configured task directory (via `discover_or_default`),
/// `<cwd>/.claude/skills`, and `<cwd>/.agents/skills` — each canonicalized, kept
/// only if it exists AND resolves INSIDE the canonical cwd, NEVER the whole cwd.
/// The inside-cwd check is load-bearing: it rejects a subtree that is a symlink
/// escaping the project (e.g. `tasks -> /`), which would otherwise re-open
/// arbitrary host-file read. An attacker passing `cwd=/` gains at most the
/// task/skill subdirs of `/`, not `/etc/passwd`. With `cwd == None` the set is
/// exactly the base allowlist, so existing callers are unchanged.
async fn read_root_allowlist(state: &AppState, cwd: Option<&str>) -> Vec<PathBuf> {
    let mut roots = preview_root_allowlist(state).await;

    if let Some(cwd) = cwd {
        let cwd = PathBuf::from(cwd);
        // The canonical cwd anchors containment. A widened subtree is honored
        // only if it resolves to a STRICT descendant of the canonical cwd —
        // otherwise a project whose task/skill dir is a symlink to `/` or `/etc`
        // would re-open arbitrary host-file read, and a symlink to the cwd itself
        // (`tasks -> .`) would collapse the allowed root to the whole project
        // (admitting e.g. `<cwd>/.env`), which this widening must never do.
        let Ok(canonical_cwd) = fs::canonicalize(&cwd) else {
            return roots;
        };
        // Honor the project's configured task directory (which may not be
        // literally `tasks`), matching what `/api/tasks` enumerates.
        let tasks_dir = taskmd_core::discover::discover_or_default(&cwd);
        for subtree in [
            cwd.join(&tasks_dir),
            cwd.join(".claude").join("skills"),
            cwd.join(".agents").join("skills"),
        ] {
            if let Ok(dir) = fs::canonicalize(subtree) {
                if dir != canonical_cwd && dir.starts_with(&canonical_cwd) {
                    roots.push(dir);
                }
            }
        }
    }

    roots
}

/// List files in a directory with metadata (REQ-PF-001, REQ-PF-002)
async fn list_files(
    State(state): State<AppState>,
    Query(query): Query<PathQuery>,
) -> Result<Json<ListFilesResponse>, AppError> {
    let path_str = query.path.trim_end_matches('/');
    let path_str = if path_str.is_empty() { "/" } else { path_str };
    let requested = PathBuf::from(path_str);

    // Confine to allowed roots before touching the filesystem. Canonicalization
    // resolves traversal/symlink escape; out-of-scope is reported as NotFound.
    let path = canonicalize_within_roots(&state, &requested).await?;

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

            // Directories are entered, not opened — they report Opaque, and
            // `is_directory` carries the "expandable" affordance separately.
            let viewer = if is_directory {
                FileViewerKind::Opaque
            } else {
                FileViewerKind::for_path(&entry_path)
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
                viewer,
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

/// Read file contents with an explicit text/image contract (REQ-PF-005).
async fn read_file(
    State(state): State<AppState>,
    Query(query): Query<PathQuery>,
) -> Result<Json<ReadFileResponse>, AppError> {
    let requested = PathBuf::from(&query.path);

    // Confine to allowed roots before reading bytes. Without this the handler is
    // an arbitrary host-file read of any non-binary file <=10MB. An optional
    // `cwd` widens the allowlist by that project's task/skill subtrees only, so a
    // freshly-picked project's tasks/skills are readable before a conversation
    // (and thus a preview root) exists for it — without admitting arbitrary read.
    let path = canonicalize_within_roots_with_cwd(&state, &requested, query.cwd.as_deref()).await?;

    if path.is_dir() {
        return Err(AppError::BadRequest("Path is a directory".to_string()));
    }

    let metadata = fs::metadata(&path)
        .map_err(|e| AppError::BadRequest(format!("Cannot read file metadata: {e}")))?;
    if metadata.len() > 10 * 1024 * 1024 {
        return Err(AppError::BadRequest(
            "File too large (max 10MB)".to_string(),
        ));
    }

    let category = match FileViewerKind::for_path(&path) {
        FileViewerKind::Image => {
            let mime_type = mime_guess::from_path(&path)
                .first_or_octet_stream()
                .to_string();
            // Carry `cwd` into the preview URL so an image admitted only by the
            // cwd-widened allowlist (a fresh project's task/skill subtree, before
            // a conversation roots it) is also servable by `serve_preview_file`,
            // which otherwise consults only the base allowlist and would 404 it.
            let url = match query.cwd.as_deref() {
                Some(cwd) => format!(
                    "{}?cwd={}",
                    preview_url_for_path(&path),
                    percent_encode_query_value(cwd)
                ),
                None => preview_url_for_path(&path),
            };
            return Ok(Json(ReadFileResponse::Image { mime_type, url }));
        }
        FileViewerKind::Opaque => {
            return Err(AppError::BadRequest(
                "File appears to be binary or has invalid encoding".to_string(),
            ));
        }
        FileViewerKind::Text { category } => category,
    };

    let content =
        fs::read(&path).map_err(|e| AppError::BadRequest(format!("Cannot read file: {e}")))?;

    if !is_valid_text(&content) {
        return Err(AppError::BadRequest(
            "File appears to be binary or has invalid encoding".to_string(),
        ));
    }

    let text = String::from_utf8(content)
        .map_err(|_| AppError::BadRequest("Invalid UTF-8 encoding".to_string()))?;

    Ok(Json(ReadFileResponse::Text {
        content: text,
        encoding: "utf-8".to_string(),
        category,
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
    State(state): State<AppState>,
    Path(filepath): Path<String>,
    Query(query): Query<PreviewQuery>,
) -> Result<axum::response::Response, AppError> {
    use axum::response::IntoResponse;

    let requested = PathBuf::from(format!("/{filepath}"));

    // Canonicalize before any check: resolves `.`, `..`, and symlinks so the
    // containment test below cannot be defeated by traversal. A path that does
    // not resolve is reported as not-found, indistinguishable from out-of-scope.
    let path = fs::canonicalize(&requested)
        .map_err(|_| AppError::NotFound("File does not exist".to_string()))?;

    // Containment: the resolved path must live inside a directory Phoenix is
    // actively serving (a conversation working directory or skill tree). Without
    // this, the handler is an arbitrary host-file read — `/preview/etc/passwd`,
    // `/preview/home/<user>/.ssh/id_rsa`, the prod DB, etc. The allowlist is
    // shared with `read_file`'s `read_root_allowlist` (including the optional
    // `cwd` widening) so any file `read_file` admits is also previewable.
    let roots = read_root_allowlist(&state, query.cwd.as_deref()).await;
    let within_roots = |p: &std::path::Path| roots.iter().any(|root| p.starts_with(root));

    if !within_roots(&path) {
        return Err(AppError::NotFound("File does not exist".to_string()));
    }

    if path.is_dir() {
        // Directory request: serve index.html if present. Canonicalize the index
        // target and re-check containment BEFORE reading — index.html may itself
        // be a symlink pointing outside the served root, which would otherwise be
        // an arbitrary-file-read escape through malicious project contents.
        let Ok(index) = fs::canonicalize(path.join("index.html")) else {
            return Err(AppError::BadRequest("Path is a directory".to_string()));
        };
        if !within_roots(&index) {
            return Err(AppError::NotFound("File does not exist".to_string()));
        }
        let content =
            fs::read(&index).map_err(|e| AppError::BadRequest(format!("Cannot read file: {e}")))?;
        let content_type = mime_guess::from_path(&index)
            .first_or_octet_stream()
            .to_string();
        return Ok(([(axum::http::header::CONTENT_TYPE, content_type)], content).into_response());
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

/// Gitignore-aware recursive file search within the conversation's file root.
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

    // Search the same directory that `message_expander::expand` resolves
    // `@file` references against at send time (the conversation's `cwd`), so
    // every autocomplete candidate is one that will actually expand.
    let root = std::path::PathBuf::from(&conversation.cwd);
    if !root.exists() {
        return Err(AppError::NotFound(
            "Conversation working directory does not exist".to_string(),
        ));
    }

    let limit = query.limit.unwrap_or(50);
    Ok(Json(FileSearchResponse {
        items: search_files_in_root(&root, &query.q, limit),
    }))
}

/// Directory-scoped file search for the new-conversation composer (REQ-IR-004).
///
/// Walks an explicit working directory rather than a conversation's file root,
/// so the composer on the `/new` page — which has no conversation yet — can
/// offer the same `@file` / `./path` autocomplete as an in-conversation composer.
async fn search_project_files(
    Query(query): Query<ProjectFileSearchQuery>,
) -> Result<Json<FileSearchResponse>, AppError> {
    let cwd = std::path::PathBuf::from(&query.cwd);
    if !cwd.exists() || !cwd.is_dir() {
        return Err(AppError::BadRequest("Directory does not exist".to_string()));
    }
    let root = crate::resolution_root::ResolutionRoot::for_create(
        &query.cwd,
        query.mode.as_deref().unwrap_or("direct"),
        query.base_branch.as_deref(),
    );
    let limit = query.limit.unwrap_or(50);
    Ok(Json(FileSearchResponse {
        items: root.list_files(&query.q, limit),
    }))
}

/// Gitignore-aware fuzzy file search within `root`.
///
/// Walks `root` respecting `.gitignore`/`.ignore`/git excludes, scores each file
/// against `q` (empty `q` = all files alphabetically up to `limit`), and returns
/// paths relative to `root`. Shared by the conversation-scoped and
/// directory-scoped search handlers.
pub(crate) fn search_files_in_root(
    root: &std::path::Path,
    query: &str,
    limit: usize,
) -> Vec<FileSearchEntry> {
    let q = query.to_lowercase();

    // Walk the directory tree with gitignore awareness
    let walker = ignore::WalkBuilder::new(root)
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
            .strip_prefix(root)
            .unwrap_or(abs_path)
            .to_string_lossy()
            .to_string();

        let viewer = FileViewerKind::for_path(abs_path);

        if q.is_empty() {
            // No query: return all files up to limit
            items.push((
                0i32,
                FileSearchEntry {
                    path: rel_path,
                    viewer,
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
                        viewer,
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

    items.into_iter().map(|(_, entry)| entry).collect()
}

/// Score a file path against a fuzzy query using nucleo-matcher.
/// Returns None if the path doesn't match. Higher scores = better matches.
///
/// Scores each path segment individually and takes the best. All segments
/// get the same +1000 bonus so nucleo's match quality alone determines the
/// winner — an exact directory-name match (nucleo ≈ 244 → total 1244) beats
/// a scattered-char match in a long UUID filename (nucleo ≈ 142 → total 1142).
pub(crate) fn fuzzy_score_path(
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

const CODE_SEARCH_DEFAULT_LIMIT: usize = 50;
const CODE_SEARCH_MAX_LIMIT: usize = 100;
const CODE_SEARCH_MAX_FILE_BYTES: u64 = 1_000_000;
const CODE_SEARCH_MAX_SCANNED_FILES: usize = 5_000;
const CODE_SEARCH_MAX_SCANNED_BYTES: u64 = 32_000_000;

/// Gitignore-aware literal content search within the conversation's file root.
async fn search_conversation_code(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<CodeSearchQuery>,
) -> Result<Json<CodeSearchResponse>, AppError> {
    let q = query.q.trim();
    if q.is_empty() {
        return Ok(Json(CodeSearchResponse { items: Vec::new() }));
    }

    let conversation = state
        .runtime
        .db()
        .get_conversation(&id)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;

    let root = std::path::PathBuf::from(conversation.file_root());
    if !root.exists() {
        return Err(AppError::NotFound(
            "Conversation file root does not exist".to_string(),
        ));
    }

    let limit = query
        .limit
        .unwrap_or(CODE_SEARCH_DEFAULT_LIMIT)
        .clamp(1, CODE_SEARCH_MAX_LIMIT);
    let case_sensitive = q.chars().any(char::is_uppercase);
    let mut items = Vec::new();
    let mut scanned_files = 0usize;
    let mut scanned_bytes = 0u64;

    let walker = ignore::WalkBuilder::new(&root)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .ignore(true)
        .filter_entry(|e| e.file_name() != ".git")
        .build();

    for result in walker {
        if items.len() >= limit
            || scanned_files >= CODE_SEARCH_MAX_SCANNED_FILES
            || scanned_bytes >= CODE_SEARCH_MAX_SCANNED_BYTES
        {
            break;
        }

        let Ok(entry) = result else { continue };
        if entry.file_type().is_some_and(|ft| ft.is_dir()) {
            continue;
        }

        let abs_path = entry.path();
        // Only grep files the viewer would open as text; images and binaries
        // have no greppable text content.
        if !matches!(
            FileViewerKind::for_path(abs_path),
            FileViewerKind::Text { .. }
        ) {
            continue;
        }

        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let size = metadata.len();
        if size > CODE_SEARCH_MAX_FILE_BYTES {
            continue;
        }
        if scanned_bytes.saturating_add(size) > CODE_SEARCH_MAX_SCANNED_BYTES {
            break;
        }
        scanned_files += 1;
        scanned_bytes += size;

        let Ok(bytes) = fs::read(abs_path) else {
            continue;
        };
        if bytes.contains(&0) {
            continue;
        }
        let Ok(content) = String::from_utf8(bytes) else {
            continue;
        };
        let Ok(rel_path) = abs_path.strip_prefix(&root) else {
            continue;
        };
        let rel_path = rel_path.to_string_lossy().to_string();

        for (idx, raw_line) in content.lines().enumerate() {
            let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
            if let Some((match_start, match_end)) = find_literal_match(line, q, case_sensitive) {
                items.push(CodeSearchEntry {
                    path: rel_path.clone(),
                    line_number: idx + 1,
                    line_text: line.to_string(),
                    match_start,
                    match_end,
                });
                if items.len() >= limit {
                    break;
                }
            }
        }
    }

    Ok(Json(CodeSearchResponse { items }))
}

fn find_literal_match(line: &str, query: &str, case_sensitive: bool) -> Option<(usize, usize)> {
    if query.is_empty() {
        return None;
    }

    let line_chars: Vec<char> = line.chars().collect();
    let query_chars: Vec<char> = query.chars().collect();
    if query_chars.len() > line_chars.len() {
        return None;
    }

    for start in 0..=line_chars.len() - query_chars.len() {
        let matched = query_chars.iter().enumerate().all(|(offset, query_char)| {
            let line_char = line_chars[start + offset];
            if case_sensitive {
                line_char == *query_char
            } else {
                line_char.to_lowercase().eq(query_char.to_lowercase())
            }
        });
        if matched {
            return Some((start, start + query_chars.len()));
        }
    }
    None
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
    Ok(Json(SkillsResponse {
        skills: skill_entries_from_dir(&cwd, None),
    }))
}

/// Directory-scoped skill discovery for the new-conversation composer (REQ-IR-005).
///
/// Discovers skills from an explicit working directory rather than a
/// conversation's `cwd`, so the composer on the `/new` page — which has no
/// conversation yet — can offer the same `/skill` autocomplete.
async fn list_project_skills(
    Query(query): Query<ProjectSkillsQuery>,
) -> Result<Json<SkillsResponse>, AppError> {
    let cwd = std::path::PathBuf::from(&query.cwd);
    if !cwd.exists() || !cwd.is_dir() {
        return Err(AppError::BadRequest("Directory does not exist".to_string()));
    }
    let root = crate::resolution_root::ResolutionRoot::for_create(
        &query.cwd,
        query.mode.as_deref().unwrap_or("direct"),
        query.base_branch.as_deref(),
    );
    // The view owns a temp materialization for a GitTree root; it must outlive
    // the discovery walk below. `strip` rewrites the temp paths back to
    // ref-relative so we never hand the frontend an ephemeral filesystem path.
    let view = root.skills_view();
    let strip = match &root {
        crate::resolution_root::ResolutionRoot::GitTree { .. } => Some(view.dir.as_path()),
        crate::resolution_root::ResolutionRoot::WorkingDir(_) => None,
    };
    Ok(Json(SkillsResponse {
        skills: skill_entries_from_dir(&view.dir, strip),
    }))
}

/// Discover skills available from `dir` and map them to API entries.
///
/// Walks the user's skill catalog (`discover_skills`) and flattens each
/// [`crate::system_prompt::SkillSource`] into a `(source, path)` pair for the
/// frontend. When `strip_prefix` is set (a `GitTree` materialization root),
/// filesystem skill paths are rewritten relative to it so the frontend sees the
/// ref-relative `SKILL.md` location instead of an ephemeral temp path; built-in
/// skill paths (outside the prefix) are left absolute. Shared by the
/// conversation-scoped and directory-scoped handlers.
fn skill_entries_from_dir(
    dir: &std::path::Path,
    strip_prefix: Option<&std::path::Path>,
) -> Vec<SkillEntry> {
    crate::system_prompt::discover_skills(dir)
        .into_iter()
        .map(|s| {
            let (source, mut path) = match &s.source {
                crate::system_prompt::SkillSource::Filesystem { path, source_dir } => {
                    (source_dir.clone(), path.to_string_lossy().to_string())
                }
                crate::system_prompt::SkillSource::Builtin { path } => {
                    ("builtin".to_string(), path.to_string_lossy().to_string())
                }
            };
            if let Some(prefix) = strip_prefix {
                if let Ok(rel) = std::path::Path::new(&path).strip_prefix(prefix) {
                    path = rel.to_string_lossy().to_string();
                }
            }
            SkillEntry {
                name: s.name,
                description: s.description,
                argument_hint: s.argument_hint,
                source,
                path,
            }
        })
        .collect()
}

// ============================================================
// Tasks
// ============================================================

async fn task_entries_for_cwd(state: &AppState, cwd: &std::path::Path) -> Vec<TaskEntry> {
    let tasks_dir_name = taskmd_core::discover::discover_or_default(cwd)
        .to_string_lossy()
        .into_owned();
    let tasks_dir = cwd.join(&tasks_dir_name);

    let all_convs = state
        .runtime
        .db()
        .list_conversations()
        .await
        .unwrap_or_default();
    let target_project_id = state
        .runtime
        .db()
        .list_projects()
        .await
        .unwrap_or_default()
        .into_iter()
        .find(|p| std::path::Path::new(&p.canonical_path) == cwd)
        .map(|p| p.id);
    let task_to_slug: std::collections::HashMap<String, String> = all_convs
        .iter()
        .filter(|c| match target_project_id.as_deref() {
            Some(project_id) => c.project_id.as_deref() == Some(project_id),
            None => std::path::Path::new(&c.cwd) == cwd,
        })
        .filter_map(|c| {
            let task_id = c.conv_mode.task_id()?;
            let slug = c.slug.as_deref()?;
            Some((task_id.to_string(), slug.to_string()))
        })
        .collect();

    taskmd_core::tasks::list_tasks(&tasks_dir)
        .into_iter()
        .map(|t| {
            let conversation_slug = task_to_slug.get(&t.id).cloned();
            TaskEntry {
                id: t.id,
                priority: t.priority.to_string(),
                status: t.status.to_string(),
                slug: t.slug,
                path: t.path.to_string_lossy().into_owned(),
                source_ref: None,
                content: None,
                conversation_slug,
            }
        })
        .collect()
}

/// List task files from a project's tasks/ directory before a conversation exists.
async fn list_project_tasks(
    State(state): State<AppState>,
    Query(query): Query<ProjectTasksQuery>,
) -> Result<Json<TasksResponse>, AppError> {
    let cwd = std::path::PathBuf::from(&query.cwd);
    if !cwd.exists() || !cwd.is_dir() {
        return Err(AppError::BadRequest("Directory does not exist".to_string()));
    }
    Ok(Json(TasksResponse {
        tasks: task_entries_for_cwd(&state, &cwd).await,
    }))
}

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
    Ok(Json(TasksResponse {
        tasks: task_entries_for_cwd(&state, &cwd).await,
    }))
}

/// Status counts for a project's tasks/ directory. Scans the task files but
/// skips the conversation/project DB queries and slug mapping that
/// `task_entries_for_cwd` does — the collapsed header needs only the counts.
fn task_counts_for_cwd(cwd: &std::path::Path, current_task_id: Option<&str>) -> TaskCountResponse {
    let tasks_dir_name = taskmd_core::discover::discover_or_default(cwd)
        .to_string_lossy()
        .into_owned();
    let tasks_dir = cwd.join(&tasks_dir_name);

    let mut active = 0u32;
    let mut closed = 0u32;
    let mut blocked = 0u32;
    let mut current = false;
    for task in taskmd_core::tasks::list_tasks(&tasks_dir) {
        let status = task.status.to_string();
        if status == "done" || status == "wont-do" {
            closed += 1;
        } else {
            active += 1;
        }
        if status == "blocked" {
            blocked += 1;
        }
        if current_task_id.is_some_and(|id| id == task.id) {
            current = true;
        }
    }

    TaskCountResponse {
        active,
        closed,
        blocked,
        current,
    }
}

/// Lightweight task status counts for the conversation's project, for the Tasks
/// panel's collapsed header (full list fetched only on expand).
async fn get_conversation_task_count(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<TaskCountQuery>,
) -> Result<Json<TaskCountResponse>, AppError> {
    let conversation = state
        .runtime
        .db()
        .get_conversation(&id)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;

    let cwd = std::path::PathBuf::from(&conversation.cwd);
    Ok(Json(task_counts_for_cwd(
        &cwd,
        query.current_task_id.as_deref(),
    )))
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

    let llm_configured = state.llm_registry.has_models();

    let credential_status = if let Some(ref hs) = state.credential_helper {
        use phoenix_llm::CredentialStatus;
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
            phoenix_llm::credential_helper::HelperEvent::Line(text) => {
                serde_json::json!({ "type": "line", "text": text })
            }
            phoenix_llm::credential_helper::HelperEvent::Complete => {
                serde_json::json!({ "type": "complete" })
            }
            phoenix_llm::credential_helper::HelperEvent::Error { exit_code, stderr } => {
                serde_json::json!({ "type": "error", "exit_code": exit_code, "stderr": stderr })
            }
        };
        Ok::<Event, Infallible>(Event::default().event("message").data(data.to_string()))
    });

    // Typed `ping` event with non-empty data so the client EventSource
    // observes the keep-alive (the previous `.text("ping")` form emitted an
    // SSE comment line that EventSource does NOT surface). See
    // specs/working-phase-visibility/ REQ-WPV-004 + design.md "Server
    // keep-alive observation."
    Sse::new(event_stream)
        .keep_alive(
            KeepAlive::new()
                .interval(std::time::Duration::from_secs(15))
                .event(Event::default().event("ping").data("ping")),
        )
        .into_response()
}

async fn invalidate_credential(State(state): State<AppState>) -> impl IntoResponse {
    use phoenix_llm::CredentialSource;

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

async fn get_env(State(state): State<AppState>) -> Json<serde_json::Value> {
    let home = state.runtime_env.home().to_string_lossy().into_owned();
    Json(serde_json::json!({ "home_dir": home }))
}

// ============================================================
// Version
// ============================================================

async fn get_version() -> &'static str {
    concat!("phoenix-ide ", env!("CARGO_PKG_VERSION"))
}

async fn get_version_json() -> impl IntoResponse {
    Json(serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "git_sha": env!("PHOENIX_GIT_SHA"),
    }))
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
        restarted = ?result.restarted,
        failed = ?result.failed,
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

/// Query parameters of an OAuth authorization-code redirect (RFC 6749 §4.1.2
/// plus the `iss` of RFC 9207). Either `code` (success) or `error` (denial)
/// is present.
#[derive(serde::Deserialize)]
struct McpOAuthCallbackParams {
    code: Option<String>,
    state: Option<String>,
    iss: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

/// Render the minimal page the operator's browser lands on after the
/// authorization server redirects. No app chrome: the tab's only job is to
/// report the outcome and be closed.
fn mcp_oauth_callback_page(title: &str, detail: &str) -> Html<String> {
    let title = html_escape(title);
    let detail = html_escape(detail);
    Html(format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>{title}</title></head>\
         <body style=\"font-family: system-ui, sans-serif; margin: 4rem auto; max-width: 32rem;\">\
         <h1 style=\"font-size: 1.2rem;\">{title}</h1><p>{detail}</p>\
         <p>You can close this tab and return to Phoenix.</p></body></html>"
    ))
}

fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The local OAuth redirect endpoint for MCP servers (REQ-MCP-011): validates
/// the callback against the pending flow (state nonce + iss), exchanges the
/// code, and reconnects the server in the background.
async fn mcp_oauth_callback(
    State(state): State<AppState>,
    Query(params): Query<McpOAuthCallbackParams>,
) -> Response {
    // An error redirect (operator denied, server rejected) fails the flow it
    // belongs to.
    if let Some(error) = &params.error {
        let detail = params.error_description.as_deref().unwrap_or(error);
        if let Some(state_nonce) = &params.state {
            state
                .mcp_manager
                .fail_oauth_authorization(state_nonce, detail)
                .await;
        }
        return (
            StatusCode::BAD_REQUEST,
            mcp_oauth_callback_page("Authorization failed", detail),
        )
            .into_response();
    }

    let (Some(code), Some(state_nonce)) = (&params.code, &params.state) else {
        return (
            StatusCode::BAD_REQUEST,
            mcp_oauth_callback_page(
                "Invalid callback",
                "The redirect is missing its 'code' or 'state' parameter.",
            ),
        )
            .into_response();
    };

    match state
        .mcp_manager
        .complete_oauth_authorization(state_nonce, code, params.iss.as_deref())
        .await
    {
        Ok(server_name) => mcp_oauth_callback_page(
            "Authorization complete",
            &format!("Phoenix is connecting to MCP server '{server_name}'."),
        )
        .into_response(),
        Err(e) => {
            tracing::warn!("MCP OAuth callback rejected: {e}");
            (
                StatusCode::BAD_REQUEST,
                mcp_oauth_callback_page("Authorization failed", &e),
            )
                .into_response()
        }
    }
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
pub(crate) fn slugify_label(label: &str) -> String {
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

pub(crate) fn generate_slug() -> String {
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

    let mut rng = rand::rng();
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
            Html(
                "<h1>404 - UI not found. Build with: corepack pnpm --dir ui run build</h1>"
                    .to_string(),
            ),
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

    let enriched_msgs: Vec<super::wire::EnrichedMessage> = messages
        .iter()
        .map(super::wire::EnrichedMessage::from)
        .collect();

    let context_window_size = messages
        .iter()
        .filter_map(|m| m.usage_data.as_ref())
        .next_back()
        .map_or(0, crate::db::UsageData::context_window_used);

    Ok(Json(ConversationWithMessagesResponse {
        conversation: conversation_to_json_with_seed(&state, &conversation, false).await?,
        messages: enriched_msgs,
        agent_working: conversation.is_agent_working(),
        presentation_mode: conv_presentation_mode(&conversation).to_string(),
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

    // A read-only share viewer must never *start* a runtime: `shared_sse_stream`
    // is auth-exempt (token-gated), so calling `get_or_create` here would let an
    // unauthenticated caller spawn an executor and allocate server resources for
    // a conversation that is otherwise idle. Attach to a live runtime only if one
    // is already running; otherwise serve the static transcript from a fresh
    // broadcaster seeded at the last persisted sequence id (the DB messages below
    // carry the full history). Live events then begin only if/when an
    // authenticated path starts the runtime.
    let (broadcast_tx, broadcast_rx) =
        if let Some(handle) = state.runtime.try_get_handle(&conversation_id).await {
            let broadcast_rx = handle.broadcast_tx.subscribe();
            (handle.broadcast_tx, broadcast_rx)
        } else {
            let last_sequence_id = state
                .runtime
                .db()
                .get_last_sequence_id(&conversation_id)
                .await
                .unwrap_or(0);
            let broadcaster = crate::runtime::SseBroadcaster::new(1, last_sequence_id);
            let broadcast_rx = broadcaster.subscribe();
            (broadcaster, broadcast_rx)
        };

    let last_sequence_id = state
        .runtime
        .db()
        .get_last_sequence_id(&conversation_id)
        .await
        .unwrap_or(0);

    let (pending_anchor_sequence_id, pending_truncated, highest_pending_seq, pending_events) =
        broadcast_tx.snapshot_pending();

    let messages = state
        .runtime
        .db()
        .get_messages(&conversation_id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let highest_message_seq = messages.iter().map(|m| m.sequence_id).max().unwrap_or(0);
    let init_seq = std::cmp::max(
        std::cmp::max(last_sequence_id, highest_pending_seq),
        highest_message_seq,
    );
    broadcast_tx.observe_seq(init_seq);

    let context_window_size = messages
        .iter()
        .filter_map(|m| m.usage_data.as_ref())
        .next_back()
        .map_or(0, crate::db::UsageData::context_window_used);

    let project_name = if let Some(ref project_id) = conversation.project_id {
        state.db.get_project(project_id).await.ok().and_then(|p| {
            std::path::Path::new(&p.canonical_path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
        })
    } else {
        None
    };

    let init_event = SseEvent::Init {
        sequence_id: init_seq,
        conversation: Box::new(enrich_conversation_with_seed(&state, &conversation, false).await?),
        transcript_generation: conversation.transcript_generation,
        messages,
        agent_working: conversation.is_agent_working(),
        presentation_mode: conv_presentation_mode(&conversation).to_string(),
        last_sequence_id: init_seq,
        context_window_size,
        project_name,
        pending_anchor_sequence_id,
        pending_events,
        pending_truncated,
    };

    Ok(sse_stream(conversation_id, init_event, broadcast_rx))
}

// ============================================================
// Error Handling
// ============================================================

#[derive(Debug)]
pub(crate) enum AppError {
    BadRequest(String),
    TypedBadRequest {
        message: String,
        error_type: String,
    },
    NotFound(String),
    /// 403 — the action is restricted to a caller on the server host.
    Forbidden(String),
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
            AppError::TypedBadRequest {
                message,
                error_type,
            } => {
                tracing::debug!(error = %message, error_type = %error_type, "400 Bad Request");
                (
                    StatusCode::BAD_REQUEST,
                    Json(ErrorResponse::typed(message, error_type)),
                )
                    .into_response()
            }
            AppError::NotFound(ref msg) => {
                tracing::debug!(error = %msg, "404 Not Found");
                (StatusCode::NOT_FOUND, Json(ErrorResponse::new(msg.clone()))).into_response()
            }
            AppError::Forbidden(ref msg) => {
                tracing::debug!(error = %msg, "403 Forbidden");
                (StatusCode::FORBIDDEN, Json(ErrorResponse::new(msg.clone()))).into_response()
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
// Conversation cwd validation tests
// ============================================================
#[cfg(test)]
mod conversation_cwd_validation_tests {
    use super::*;
    use crate::api::types::CreateConversationRequest;

    fn create_request(cwd: String) -> CreateConversationRequest {
        CreateConversationRequest {
            conversation_id: None,
            cwd,
            model: None,
            text: "hello".to_string(),
            message_id: uuid::Uuid::new_v4().to_string(),
            images: Vec::new(),
            files: Vec::new(),
            mode: Some("direct".to_string()),
            base_branch: None,
            seed_parent_id: None,
            checkout_ref: None,
            seed_label: None,
        }
    }

    #[tokio::test]
    async fn create_conversation_accepts_root_cwd_for_async_validation() {
        let state = hard_delete_cascade_tests::make_test_state().await;
        let Json(response) =
            create_conversation_with_id(state, create_request("/".to_string()), Vec::new())
                .await
                .expect("root cwd is persisted before worker validation");
        assert_eq!(response.conversation["cwd"].as_str(), Some("/"));
        assert_eq!(
            response.conversation["state"]["type"].as_str(),
            Some("provisioning")
        );
    }

    #[tokio::test]
    async fn submitted_attachment_must_belong_to_creation_conversation() {
        let root = tempfile::tempdir().expect("attachment root");
        let other_dir = root.path().join("other-conversation");
        tokio::fs::create_dir_all(&other_dir)
            .await
            .expect("other dir");
        let stored_path = other_dir.join("file.txt");
        tokio::fs::write(&stored_path, b"hello")
            .await
            .expect("file");
        let file = crate::api::types::FileAttachment {
            original_name: "file.txt".to_string(),
            stored_path: stored_path.to_string_lossy().to_string(),
            media_type: "text/plain".to_string(),
            size_bytes: 5,
        };
        tokio::fs::create_dir_all(root.path().join("new-conversation"))
            .await
            .expect("expected dir");

        let error =
            validate_submitted_attachments_at_root(root.path(), "new-conversation", &[file])
                .await
                .expect_err("cross-conversation attachment must be rejected");
        assert!(
            matches!(error, AppError::BadRequest(message) if message.contains("does not belong"))
        );
    }

    #[tokio::test]
    async fn create_conversation_preserves_omitted_model_in_durable_intent() {
        let state = hard_delete_cascade_tests::make_test_state().await;
        let conversation_id = uuid::Uuid::new_v4().to_string();
        let mut request = create_request("/".to_string());
        request.conversation_id = Some(conversation_id.clone());
        request.mode = Some("managed".to_string());

        let _response = create_conversation_with_id(state.clone(), request, Vec::new())
            .await
            .expect("creation shell accepted");
        let job = state
            .runtime
            .db()
            .get_conversation_creation_job_for_conversation(&conversation_id)
            .await
            .expect("job lookup")
            .expect("creation job");

        assert_eq!(job.intent.model, None);
    }

    #[tokio::test]
    async fn create_conversation_preserves_raw_cwd_for_async_validation() {
        let state = hard_delete_cascade_tests::make_test_state().await;
        let Json(response) =
            create_conversation_with_id(state, create_request("/..".to_string()), Vec::new())
                .await
                .expect("raw cwd accepted");
        assert_eq!(response.conversation["cwd"].as_str(), Some("/.."));
    }

    #[tokio::test]
    async fn create_conversation_accepts_deep_cwd() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let deep = tmp.path().join("project/src");
        std::fs::create_dir_all(&deep).expect("deep dir");
        let state = hard_delete_cascade_tests::make_test_state().await;

        let Json(response) = create_conversation_with_id(
            state,
            create_request(deep.to_string_lossy().to_string()),
            Vec::new(),
        )
        .await
        .expect("deep cwd accepted");

        assert_eq!(
            response.conversation["cwd"].as_str(),
            Some(deep.to_str().unwrap())
        );
        assert_eq!(
            response.conversation["state"]["type"].as_str(),
            Some("provisioning")
        );
    }

    #[tokio::test]
    async fn explicit_worktree_mode_outside_git_returns_provisioning_shell() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let state = hard_delete_cascade_tests::make_test_state().await;
        let mut req = create_request(tmp.path().to_string_lossy().to_string());
        req.mode = Some("managed".to_string());

        let Json(response) = create_conversation_with_id(state, req, Vec::new())
            .await
            .expect("managed intent accepted before git validation");
        assert_eq!(
            response.conversation["state"]["type"].as_str(),
            Some("provisioning")
        );
    }

    fn init_git_repo(path: &std::path::Path) {
        crate::git_ops::run_git(path, &["init", "--initial-branch=main"]).expect("git init");
        crate::git_ops::run_git(path, &["commit", "--allow-empty", "-m", "init"])
            .expect("git commit");
    }

    #[tokio::test]
    async fn managed_mode_without_base_branch_is_deferred_to_worker() {
        let tmp = tempfile::tempdir().expect("tempdir");
        init_git_repo(tmp.path());
        let state = hard_delete_cascade_tests::make_test_state().await;
        let mut req = create_request(tmp.path().to_string_lossy().to_string());
        req.mode = Some("managed".to_string());

        let Json(response) = create_conversation_with_id(state, req, Vec::new())
            .await
            .expect("managed intent accepted before base validation");
        assert_eq!(
            response.conversation["state"]["type"].as_str(),
            Some("provisioning")
        );
    }

    #[tokio::test]
    async fn retry_existing_creation_shell_bypasses_git_preflight() {
        let tmp = tempfile::tempdir().expect("tempdir");
        init_git_repo(tmp.path());
        let state = hard_delete_cascade_tests::make_test_state().await;
        let conv_id = uuid::Uuid::new_v4().to_string();
        state
            .db
            .create_conversation(
                &conv_id,
                "conv-existing-shell",
                tmp.path().to_str().unwrap(),
                true,
                None,
                None,
            )
            .await
            .expect("existing shell");
        state
            .db
            .insert_conversation_creation_job(&crate::db::InsertConversationCreationJob {
                id: uuid::Uuid::new_v4().to_string(),
                conversation_id: conv_id.clone(),
                message_id: Some("msg-existing-shell".to_string()),
                intent: crate::db::ConversationCreationIntent {
                    cwd: tmp.path().to_string_lossy().to_string(),
                    model: None,
                    text: "retry".to_string(),
                    expansion_preflighted: false,
                    llm_text: None,
                    skill_invocation: None,
                    message_id: "msg-existing-shell".to_string(),
                    images: vec![],
                    files: vec![],
                    mode: Some("branch".to_string()),
                    base_branch: Some("does-not-exist".to_string()),
                    checkout_ref: None,
                    seed_parent_id: None,
                    seed_label: None,
                },
            })
            .await
            .expect("creation job");
        let mut req = create_request(tmp.path().to_string_lossy().to_string());
        req.conversation_id = Some(conv_id.clone());
        req.message_id = "msg-existing-shell".to_string();
        req.mode = Some("branch".to_string());
        req.base_branch = Some("does-not-exist".to_string());

        let Json(response) = create_conversation_with_id(state, req, Vec::new())
            .await
            .expect("idempotent retry returns existing shell before git checks");

        assert_eq!(response.conversation["id"].as_str(), Some(conv_id.as_str()));
    }

    #[tokio::test]
    async fn auto_mode_stale_checkout_ref_is_deferred_to_worker() {
        let tmp = tempfile::tempdir().expect("tempdir");
        init_git_repo(tmp.path());
        let state = hard_delete_cascade_tests::make_test_state().await;
        let mut req = create_request(tmp.path().to_string_lossy().to_string());
        req.mode = Some("auto".to_string());
        req.checkout_ref = Some("does-not-exist".to_string());

        let Json(response) = create_conversation_with_id(state.clone(), req, Vec::new())
            .await
            .expect("auto create accepts shell before worker ref validation");

        assert_eq!(
            response.conversation["state"]["type"].as_str(),
            Some("provisioning")
        );
        let conv_id = response.conversation["id"].as_str().expect("id");
        let job = state
            .db
            .get_conversation_creation_job_for_conversation(conv_id)
            .await
            .expect("load job")
            .expect("job");
        assert_eq!(job.intent.checkout_ref.as_deref(), Some("does-not-exist"));
    }

    #[tokio::test]
    async fn managed_mode_nonexistent_base_branch_is_deferred_to_worker() {
        let tmp = tempfile::tempdir().expect("tempdir");
        init_git_repo(tmp.path());
        let state = hard_delete_cascade_tests::make_test_state().await;
        let mut req = create_request(tmp.path().to_string_lossy().to_string());
        req.mode = Some("managed".to_string());
        req.base_branch = Some("does-not-exist".to_string());

        let Json(response) = create_conversation_with_id(state.clone(), req, Vec::new())
            .await
            .expect("managed create accepts shell before worker ref validation");

        assert_eq!(
            response.conversation["state"]["type"].as_str(),
            Some("provisioning")
        );
        let conv_id = response.conversation["id"].as_str().expect("id");
        let job = state
            .db
            .get_conversation_creation_job_for_conversation(conv_id)
            .await
            .expect("load job")
            .expect("job");
        assert_eq!(job.intent.base_branch.as_deref(), Some("does-not-exist"));
    }

    #[tokio::test]
    async fn completed_creation_job_different_message_id_conflicts() {
        let state = hard_delete_cascade_tests::make_test_state().await;
        let conv_id = uuid::Uuid::new_v4().to_string();
        state
            .db
            .create_conversation(&conv_id, "completed-shell", "/tmp", true, None, None)
            .await
            .expect("existing conversation");
        state
            .db
            .insert_conversation_creation_job(&crate::db::InsertConversationCreationJob {
                id: "job-completed-shell".to_string(),
                conversation_id: conv_id.clone(),
                message_id: Some("msg-original".to_string()),
                intent: crate::db::ConversationCreationIntent {
                    cwd: "/tmp".to_string(),
                    model: None,
                    text: "original".to_string(),
                    expansion_preflighted: false,
                    llm_text: None,
                    skill_invocation: None,
                    message_id: "msg-original".to_string(),
                    images: vec![],
                    files: vec![],
                    mode: None,
                    base_branch: None,
                    checkout_ref: None,
                    seed_parent_id: None,
                    seed_label: None,
                },
            })
            .await
            .expect("insert job");
        let claimed = state
            .db
            .claim_next_conversation_creation_job(
                &phoenix_core::domain::creation_protocol::CreationWorkerId("test-worker".into()),
                &phoenix_core::domain::creation_protocol::CreationClaimToken("test-token".into()),
                chrono::Utc::now(),
                chrono::Duration::minutes(1),
            )
            .await
            .expect("claim job");
        let crate::db::CreationClaimOutcome::Claimed(job) = claimed else {
            panic!("expected claimed job");
        };
        let phoenix_core::domain::creation_protocol::CreationStatus::Claimed(claim) =
            job.protocol.status
        else {
            panic!("expected claim authority");
        };
        state
            .db
            .complete_conversation_creation_job("job-completed-shell", &claim, chrono::Utc::now())
            .await
            .expect("complete job");
        let mut req = create_request("/tmp".to_string());
        req.conversation_id = Some(conv_id);
        req.message_id = "msg-different".to_string();

        let err = create_conversation_with_id(state, req, Vec::new())
            .await
            .expect_err("completed creation id reuse with a new message must conflict");

        match err {
            AppError::Conflict(detail) => assert_eq!(detail.error_type, "conversation_id_exists"),
            other => panic!("expected Conflict, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn existing_non_creation_conversation_id_conflict_is_typed() {
        let state = hard_delete_cascade_tests::make_test_state().await;
        let conv_id = uuid::Uuid::new_v4().to_string();
        state
            .db
            .create_conversation(&conv_id, "existing", "/tmp", true, None, None)
            .await
            .expect("existing conversation");
        let mut req = create_request("/tmp".to_string());
        req.conversation_id = Some(conv_id);

        let err = create_conversation_with_id(state, req, Vec::new())
            .await
            .expect_err("non-creation id reuse must conflict");

        match err {
            AppError::Conflict(detail) => {
                assert_eq!(detail.error_type, "conversation_id_exists");
                assert!(
                    detail.error.contains("existing conversation"),
                    "got: {}",
                    detail.error
                );
            }
            other => panic!("expected Conflict, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn branch_mode_nonexistent_branch_is_deferred_to_worker() {
        let tmp = tempfile::tempdir().expect("tempdir");
        init_git_repo(tmp.path());
        let state = hard_delete_cascade_tests::make_test_state().await;
        let mut req = create_request(tmp.path().to_string_lossy().to_string());
        req.mode = Some("branch".to_string());
        req.base_branch = Some("does-not-exist".to_string());

        let Json(response) = create_conversation_with_id(state, req, Vec::new())
            .await
            .expect("branch intent accepted before git validation");
        assert_eq!(
            response.conversation["state"]["type"].as_str(),
            Some("provisioning")
        );
    }

    #[tokio::test]
    async fn creation_intent_preserves_raw_cwd() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project = tmp.path().join("project");
        std::fs::create_dir_all(&project).expect("project dir");
        let raw_cwd = project.join("..").join("project");
        let state = hard_delete_cascade_tests::make_test_state().await;
        let mut req = create_request(raw_cwd.to_string_lossy().to_string());
        req.conversation_id = Some(uuid::Uuid::new_v4().to_string());
        let conv_id = req.conversation_id.clone().expect("id");

        let _ = create_conversation_with_id(state.clone(), req, Vec::new())
            .await
            .expect("create shell");

        let job = state
            .db
            .get_conversation_creation_job_for_conversation(&conv_id)
            .await
            .expect("load job")
            .expect("job exists");
        assert_eq!(job.intent.cwd, raw_cwd.to_string_lossy());
    }

    #[tokio::test]
    async fn validate_cwd_rejects_filesystem_root() {
        let Json(response) = validate_cwd(Query(PathQuery {
            path: "/".to_string(),
            cwd: None,
        }))
        .await;

        assert!(!response.valid);
        assert!(!response.is_git);
        assert!(
            response
                .error
                .as_deref()
                .is_some_and(|e| e.contains("filesystem root")),
            "got: {:?}",
            response.error
        );
    }

    #[test]
    fn creation_recovery_shell_stream_does_not_start_runtime() {
        assert!(!stream_state_starts_runtime(&ConvState::CreationFailed {
            job_id: "job".to_string(),
            error: "failed".to_string(),
            error_kind: crate::db::ErrorKind::ServerError,
        }));
        assert!(!stream_state_starts_runtime(
            &ConvState::CreationCancelled {
                job_id: "job".to_string(),
            }
        ));
        assert!(stream_state_starts_runtime(&ConvState::Idle));
    }

    #[tokio::test]
    async fn stream_conversation_serves_transcript_when_cwd_is_stale() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let stale_cwd = tmp.path().to_string_lossy().to_string();
        let state = hard_delete_cascade_tests::make_test_state().await;
        state
            .db
            .create_conversation("stale-cwd", "stale-cwd", &stale_cwd, true, None, None)
            .await
            .expect("create");
        drop(tmp);

        let response = stream_conversation(
            State(state),
            Path("stale-cwd".to_string()),
            Query(StreamConversationQuery::default()),
        )
        .await
        .expect("stale cwd should still stream transcript")
        .into_response();

        assert_eq!(response.status(), StatusCode::OK);
    }
}

// ============================================================
// Hard-delete cascade tests (REQ-BED-032)
// ============================================================
#[cfg(test)]
pub(crate) mod hard_delete_cascade_tests {
    use super::*;
    use crate::chain_qa::ChainQa;
    use crate::db::{Database, MessageContent};
    use crate::platform::PlatformCapability;
    use crate::runtime::RuntimeManager;
    use crate::state_machine::ConvState;
    use crate::tools::mcp::McpClientManager;
    use phoenix_llm::ModelRegistry;
    use std::sync::Arc;

    /// Construct a minimal `AppState` backed by an in-memory database.
    /// The state machine handler is started so `runtime.try_get_handle`
    /// works when the test wants to verify SSE events; conversations
    /// are otherwise inert (no LLM calls fire).
    pub(crate) async fn make_test_state() -> AppState {
        let db = Database::open_in_memory().await.expect("open db");
        let llm_registry = Arc::new(ModelRegistry::new_empty());
        let platform = PlatformCapability::None {
            details: "test".into(),
        };
        let mcp_manager = Arc::new(McpClientManager::new());
        let runtime = Arc::new(RuntimeManager::new(
            db.clone(),
            llm_registry.clone(),
            platform.clone(),
            mcp_manager.clone(),
            None,
        ));
        let terminals = runtime.terminals.clone();
        let message_retriever: std::sync::Arc<dyn crate::db::MessageRetriever> =
            std::sync::Arc::new(crate::db::Fts5Retriever::new(db.pool().clone()));
        let chain_qa = ChainQa::new(db.clone(), llm_registry.clone(), message_retriever.clone());
        let sessions = super::super::auth::SessionStore::new(db.clone(), String::new());
        AppState {
            runtime,
            llm_registry,
            db,
            platform,
            mcp_manager,
            credential_helper: None,
            password: None,
            sessions,
            login_throttle: super::super::auth::LoginThrottle::new(),
            terminals,
            chain_qa,
            message_retriever,
            codex_login: super::super::codex_login::CodexLoginManager::new(),
            deployment: Arc::new(super::super::deployment::DeploymentConfig::for_tests()),
            runtime_env: Arc::new(phoenix_core::runtime_env::PhoenixRuntimeEnvironment::detect()),
            suggest_token: String::new(),
            discovery: crate::discovery::start(crate::discovery::DiscoveryConfig {
                enabled: false,
                ..crate::discovery::DiscoveryConfig::from_env()
            }),
        }
    }

    fn bump_generation_after_next_stable_read_value(conversation_id: &'static str) {
        STABLE_TRANSCRIPT_READ_TEST_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move |read_conversation_id| {
                read_conversation_id == conversation_id
            }));
        });
    }

    #[tokio::test]
    async fn message_history_latest_and_range_endpoints_return_expected_shapes() {
        let state = make_test_state().await;
        state
            .db
            .create_conversation("conv-history", "history", "/tmp", true, None, None)
            .await
            .expect("create conversation");

        for idx in 1..=5 {
            state
                .db
                .add_message(
                    &format!("hist-msg-{idx}"),
                    "conv-history",
                    &crate::db::MessageContent::user(format!("m{idx}")),
                    None,
                    None,
                )
                .await
                .expect("add message");
        }

        let Json(latest) = get_conversation_messages_latest(
            State(state.clone()),
            Path("conv-history".to_string()),
            Query(LatestMessagesQuery { limit: Some(2) }),
        )
        .await
        .expect("latest messages");
        assert_eq!(
            latest
                .messages
                .iter()
                .map(|m| m.sequence_id)
                .collect::<Vec<_>>(),
            vec![4, 5]
        );
        assert!(latest.tombstones.is_empty());
        assert_eq!(latest.server_message_tail, Some(5));
        assert_eq!(latest.transcript_generation, Some(1));

        let Json(range) = get_conversation_message_range(
            State(state),
            Path("conv-history".to_string()),
            Query(MessageRangeQuery {
                start_message_sequence: 2,
                end_message_sequence: 6,
            }),
        )
        .await
        .expect("range messages");
        assert_eq!(
            range
                .messages
                .iter()
                .map(|m| m.sequence_id)
                .collect::<Vec<_>>(),
            vec![2, 3, 4, 5]
        );
        assert_eq!(range.missing_sequences, vec![6]);
        assert!(range.tombstones.is_empty());
        assert_eq!(range.server_message_tail, Some(5));
    }

    #[tokio::test]
    async fn latest_message_slice_aligns_to_agent_turn_boundary_without_losing_has_older() {
        use phoenix_core::domain::llm_types::ContentBlock;

        let state = make_test_state().await;
        state
            .db
            .create_conversation(
                "conv-history-latest-turn",
                "history-latest-turn",
                "/tmp",
                true,
                None,
                None,
            )
            .await
            .expect("create conversation");

        state
            .db
            .add_message(
                "turn-user-1",
                "conv-history-latest-turn",
                &crate::db::MessageContent::user("before"),
                None,
                None,
            )
            .await
            .expect("add user 1");
        state
            .db
            .add_message(
                "turn-user-2",
                "conv-history-latest-turn",
                &crate::db::MessageContent::user("prompt"),
                None,
                None,
            )
            .await
            .expect("add user 2");
        state
            .db
            .add_message(
                "turn-agent",
                "conv-history-latest-turn",
                &crate::db::MessageContent::agent(vec![ContentBlock::ToolUse {
                    id: "tool-a".to_string(),
                    name: "read_file".to_string(),
                    input: serde_json::json!({"path": "foo"}),
                }]),
                None,
                None,
            )
            .await
            .expect("add agent");
        state
            .db
            .add_message(
                "turn-tool",
                "conv-history-latest-turn",
                &crate::db::MessageContent::tool("tool-a", "tool output", false),
                None,
                None,
            )
            .await
            .expect("add tool");

        let Json(latest) = get_conversation_messages_latest(
            State(state),
            Path("conv-history-latest-turn".to_string()),
            Query(LatestMessagesQuery { limit: Some(2) }),
        )
        .await
        .expect("latest messages");

        assert_eq!(
            latest
                .messages
                .iter()
                .map(|m| m.sequence_id)
                .collect::<Vec<_>>(),
            vec![3, 4]
        );
        assert!(latest.has_older_messages);
    }

    #[tokio::test]
    async fn latest_message_slice_owner_backfill_reports_complete_when_owner_is_first() {
        use phoenix_core::domain::llm_types::ContentBlock;

        let state = make_test_state().await;
        state
            .db
            .create_conversation(
                "conv-complete-owner",
                "complete-owner",
                "/tmp",
                true,
                None,
                None,
            )
            .await
            .expect("create");
        state
            .db
            .add_message(
                "complete-agent",
                "conv-complete-owner",
                &crate::db::MessageContent::agent(vec![ContentBlock::ToolUse {
                    id: "tool-complete".to_string(),
                    name: "read_file".to_string(),
                    input: serde_json::json!({}),
                }]),
                None,
                None,
            )
            .await
            .expect("agent");
        state
            .db
            .add_message(
                "complete-tool",
                "conv-complete-owner",
                &crate::db::MessageContent::tool("tool-complete", "output", false),
                None,
                None,
            )
            .await
            .expect("tool");

        let Json(latest) = get_conversation_messages_latest(
            State(state),
            Path("conv-complete-owner".to_string()),
            Query(LatestMessagesQuery { limit: Some(1) }),
        )
        .await
        .expect("latest");

        assert_eq!(
            latest
                .messages
                .iter()
                .map(|m| m.sequence_id)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert!(!latest.has_older_messages);
    }

    #[tokio::test]
    async fn latest_message_slice_preserves_standalone_rows_between_owner_and_tools() {
        use phoenix_core::domain::llm_types::ContentBlock;

        let state = make_test_state().await;
        state
            .db
            .create_conversation(
                "conv-standalone-owner",
                "standalone-owner",
                "/tmp",
                true,
                None,
                None,
            )
            .await
            .expect("create");
        state
            .db
            .add_message(
                "standalone-agent",
                "conv-standalone-owner",
                &crate::db::MessageContent::agent(vec![ContentBlock::ToolUse {
                    id: "tool-standalone".to_string(),
                    name: "read_file".to_string(),
                    input: serde_json::json!({}),
                }]),
                None,
                None,
            )
            .await
            .expect("agent");
        state
            .db
            .add_message(
                "standalone-system",
                "conv-standalone-owner",
                &crate::db::MessageContent::system("checkpoint"),
                None,
                None,
            )
            .await
            .expect("system");
        state
            .db
            .add_message(
                "standalone-tool",
                "conv-standalone-owner",
                &crate::db::MessageContent::tool("tool-standalone", "output", false),
                None,
                None,
            )
            .await
            .expect("tool");

        let Json(latest) = get_conversation_messages_latest(
            State(state),
            Path("conv-standalone-owner".to_string()),
            Query(LatestMessagesQuery { limit: Some(1) }),
        )
        .await
        .expect("latest");

        assert_eq!(
            latest
                .messages
                .iter()
                .map(|m| m.sequence_id)
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
    }

    #[tokio::test]
    async fn system_user_transcript_preserves_full_latest_and_before_boundaries() {
        let state = make_test_state().await;
        state
            .db
            .create_conversation("conv-system-user", "system-user", "/tmp", true, None, None)
            .await
            .expect("create");
        state
            .db
            .add_message(
                "system-user-system",
                "conv-system-user",
                &crate::db::MessageContent::system("preamble"),
                None,
                None,
            )
            .await
            .expect("system");
        state
            .db
            .add_message(
                "system-user-user",
                "conv-system-user",
                &crate::db::MessageContent::user("prompt"),
                None,
                None,
            )
            .await
            .expect("user");

        let Json(full) = get_conversation(
            State(state.clone()),
            Path("conv-system-user".to_string()),
            Query(GetConversationQuery {
                after_sequence: None,
            }),
        )
        .await
        .expect("full conversation");
        assert_eq!(
            full.messages
                .iter()
                .map(|m| m.sequence_id)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );

        let Json(latest) = get_conversation_messages_latest(
            State(state.clone()),
            Path("conv-system-user".to_string()),
            Query(LatestMessagesQuery { limit: Some(1) }),
        )
        .await
        .expect("latest");
        assert_eq!(
            latest
                .messages
                .iter()
                .map(|m| m.sequence_id)
                .collect::<Vec<_>>(),
            vec![2]
        );
        assert!(latest.has_older_messages);

        let Json(before) = get_conversation_messages(
            State(state),
            Path("conv-system-user".to_string()),
            Query(MessageHistoryQuery {
                before_message_sequence: Some(2),
                after_message_sequence: None,
                limit: Some(1),
            }),
        )
        .await
        .expect("before");
        assert_eq!(
            before
                .messages
                .iter()
                .map(|m| m.sequence_id)
                .collect::<Vec<_>>(),
            vec![1]
        );
        assert!(!before.has_older_messages);
    }

    #[tokio::test]
    async fn latest_message_slice_preserves_standalone_prefix_at_transcript_start() {
        let state = make_test_state().await;
        state
            .db
            .create_conversation(
                "conv-standalone-prefix",
                "standalone-prefix",
                "/tmp",
                true,
                None,
                None,
            )
            .await
            .expect("create");
        state
            .db
            .add_message(
                "standalone-prefix-system",
                "conv-standalone-prefix",
                &crate::db::MessageContent::system("preamble"),
                None,
                None,
            )
            .await
            .expect("system");
        state
            .db
            .add_message(
                "standalone-prefix-tool",
                "conv-standalone-prefix",
                &crate::db::MessageContent::tool("orphan-tool", "output", false),
                None,
                None,
            )
            .await
            .expect("tool");

        let Json(latest) = get_conversation_messages_latest(
            State(state),
            Path("conv-standalone-prefix".to_string()),
            Query(LatestMessagesQuery { limit: Some(1) }),
        )
        .await
        .expect("latest");

        assert_eq!(
            latest
                .messages
                .iter()
                .map(|m| m.sequence_id)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert!(!latest.has_older_messages);
    }

    #[tokio::test]
    async fn latest_message_slice_accepts_exact_ceiling_when_no_older_rows_exist() {
        let state = make_test_state().await;
        state
            .db
            .create_conversation(
                "conv-history-exact-ceiling-prefix",
                "history-exact-ceiling-prefix",
                "/tmp",
                true,
                None,
                None,
            )
            .await
            .expect("create conversation");
        for idx in 0..MAX_RENDER_UNIT_ALIGNED_RESPONSE_MESSAGES {
            let content = if idx == 0 {
                crate::db::MessageContent::system("preamble")
            } else {
                crate::db::MessageContent::tool("orphan-tool", format!("tool output {idx}"), false)
            };
            state
                .db
                .add_message(
                    &format!("exact-ceiling-prefix-{idx}"),
                    "conv-history-exact-ceiling-prefix",
                    &content,
                    None,
                    None,
                )
                .await
                .expect("add message");
        }

        let Json(latest) = get_conversation_messages_latest(
            State(state),
            Path("conv-history-exact-ceiling-prefix".to_string()),
            Query(LatestMessagesQuery { limit: Some(1) }),
        )
        .await
        .expect("exact-ceiling aligned slice should be accepted");

        assert_eq!(
            latest.messages.len(),
            MAX_RENDER_UNIT_ALIGNED_RESPONSE_MESSAGES
        );
        assert_eq!(latest.messages.first().map(|m| m.sequence_id), Some(1));
        assert_eq!(
            latest.messages.last().map(|m| m.sequence_id),
            Some(
                i64::try_from(MAX_RENDER_UNIT_ALIGNED_RESPONSE_MESSAGES).expect("ceiling fits i64")
            )
        );
        assert!(!latest.has_older_messages);
    }

    #[tokio::test]
    async fn latest_message_slice_backfills_contiguous_tool_run_past_chunk_cutoff() {
        use phoenix_core::domain::llm_types::ContentBlock;

        let state = make_test_state().await;
        state
            .db
            .create_conversation(
                "conv-history-latest-long-turn",
                "history-latest-long-turn",
                "/tmp",
                true,
                None,
                None,
            )
            .await
            .expect("create conversation");

        state
            .db
            .add_message(
                "long-user",
                "conv-history-latest-long-turn",
                &crate::db::MessageContent::user("prompt"),
                None,
                None,
            )
            .await
            .expect("add user");
        state
            .db
            .add_message(
                "long-agent",
                "conv-history-latest-long-turn",
                &crate::db::MessageContent::agent(vec![ContentBlock::ToolUse {
                    id: "tool-long".to_string(),
                    name: "read_file".to_string(),
                    input: serde_json::json!({"path": "foo"}),
                }]),
                None,
                None,
            )
            .await
            .expect("add agent");
        for idx in 0..600 {
            state
                .db
                .add_message(
                    &format!("long-tool-{idx}"),
                    "conv-history-latest-long-turn",
                    &crate::db::MessageContent::tool(
                        "tool-long",
                        format!("tool output {idx}"),
                        false,
                    ),
                    None,
                    None,
                )
                .await
                .expect("add tool");
        }

        let Json(latest) = get_conversation_messages_latest(
            State(state),
            Path("conv-history-latest-long-turn".to_string()),
            Query(LatestMessagesQuery { limit: Some(50) }),
        )
        .await
        .expect("latest messages");

        let sequence_ids = latest
            .messages
            .iter()
            .map(|m| m.sequence_id)
            .collect::<Vec<_>>();
        assert_eq!(sequence_ids, (2..=602).collect::<Vec<_>>());
        assert!(latest.has_older_messages);
    }

    #[tokio::test]
    async fn before_message_slice_aligns_to_agent_turn_boundary_without_losing_has_older() {
        use phoenix_core::domain::llm_types::ContentBlock;

        let state = make_test_state().await;
        state
            .db
            .create_conversation(
                "conv-history-before-turn",
                "history-before-turn",
                "/tmp",
                true,
                None,
                None,
            )
            .await
            .expect("create conversation");

        state
            .db
            .add_message(
                "before-user-1",
                "conv-history-before-turn",
                &crate::db::MessageContent::user("older"),
                None,
                None,
            )
            .await
            .expect("add user 1");
        state
            .db
            .add_message(
                "before-user-2",
                "conv-history-before-turn",
                &crate::db::MessageContent::user("prompt"),
                None,
                None,
            )
            .await
            .expect("add user 2");
        state
            .db
            .add_message(
                "before-agent",
                "conv-history-before-turn",
                &crate::db::MessageContent::agent(vec![ContentBlock::ToolUse {
                    id: "tool-b".to_string(),
                    name: "read_file".to_string(),
                    input: serde_json::json!({"path": "bar"}),
                }]),
                None,
                None,
            )
            .await
            .expect("add agent");
        state
            .db
            .add_message(
                "before-tool",
                "conv-history-before-turn",
                &crate::db::MessageContent::tool("tool-b", "tool output", false),
                None,
                None,
            )
            .await
            .expect("add tool");
        state
            .db
            .add_message(
                "before-user-3",
                "conv-history-before-turn",
                &crate::db::MessageContent::user("tail"),
                None,
                None,
            )
            .await
            .expect("add user 3");

        let Json(before) = get_conversation_messages(
            State(state),
            Path("conv-history-before-turn".to_string()),
            Query(MessageHistoryQuery {
                before_message_sequence: Some(5),
                after_message_sequence: None,
                limit: Some(2),
            }),
        )
        .await
        .expect("before messages");

        assert_eq!(
            before
                .messages
                .iter()
                .map(|m| m.sequence_id)
                .collect::<Vec<_>>(),
            vec![3, 4]
        );
        assert!(before.has_older_messages);
    }

    #[tokio::test]
    async fn latest_message_slice_fails_when_render_unit_backfill_exceeds_ceiling() {
        use phoenix_core::domain::llm_types::ContentBlock;

        let state = make_test_state().await;
        state
            .db
            .create_conversation(
                "conv-history-over-ceiling-turn",
                "history-over-ceiling-turn",
                "/tmp",
                true,
                None,
                None,
            )
            .await
            .expect("create conversation");
        state
            .db
            .add_message(
                "over-ceiling-agent",
                "conv-history-over-ceiling-turn",
                &crate::db::MessageContent::agent(vec![ContentBlock::ToolUse {
                    id: "tool-over-ceiling".to_string(),
                    name: "read_file".to_string(),
                    input: serde_json::json!({"path": "foo"}),
                }]),
                None,
                None,
            )
            .await
            .expect("add agent");
        for idx in 0..MAX_RENDER_UNIT_ALIGNED_RESPONSE_MESSAGES {
            state
                .db
                .add_message(
                    &format!("over-ceiling-tool-{idx}"),
                    "conv-history-over-ceiling-turn",
                    &crate::db::MessageContent::tool(
                        "tool-over-ceiling",
                        format!("tool output {idx}"),
                        false,
                    ),
                    None,
                    None,
                )
                .await
                .expect("add tool");
        }

        let err = get_conversation_messages_latest(
            State(state),
            Path("conv-history-over-ceiling-turn".to_string()),
            Query(LatestMessagesQuery { limit: Some(1) }),
        )
        .await
        .expect_err("over-ceiling aligned slice should fail explicitly");

        match err {
            AppError::TypedBadRequest {
                error_type,
                message,
            } => {
                assert_eq!(error_type, "message_slice_render_unit_ceiling_exceeded");
                assert!(message.contains(&MAX_RENDER_UNIT_ALIGNED_RESPONSE_MESSAGES.to_string()));
            }
            other => panic!("expected typed bad request, got {other:?}"),
        }
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn repeated_older_page_traversal_returns_each_sequence_exactly_once() {
        use phoenix_core::domain::llm_types::ContentBlock;
        use std::collections::HashSet;

        let state = make_test_state().await;
        state
            .db
            .create_conversation(
                "conv-history-page-each-once",
                "history-page-each-once",
                "/tmp",
                true,
                None,
                None,
            )
            .await
            .expect("create conversation");

        state
            .db
            .add_message(
                "page-user-1",
                "conv-history-page-each-once",
                &crate::db::MessageContent::user("older prompt"),
                None,
                None,
            )
            .await
            .expect("add older user");
        state
            .db
            .add_message(
                "page-agent-1",
                "conv-history-page-each-once",
                &crate::db::MessageContent::agent(vec![ContentBlock::ToolUse {
                    id: "tool-page-1".to_string(),
                    name: "read_file".to_string(),
                    input: serde_json::json!({"path": "older"}),
                }]),
                None,
                None,
            )
            .await
            .expect("add older agent");
        for idx in 0..90 {
            state
                .db
                .add_message(
                    &format!("page-tool-1-{idx}"),
                    "conv-history-page-each-once",
                    &crate::db::MessageContent::tool(
                        "tool-page-1",
                        format!("older tool output {idx}"),
                        false,
                    ),
                    None,
                    None,
                )
                .await
                .expect("add older tool");
        }
        state
            .db
            .add_message(
                "page-user-2",
                "conv-history-page-each-once",
                &crate::db::MessageContent::user("newer prompt"),
                None,
                None,
            )
            .await
            .expect("add newer user");
        state
            .db
            .add_message(
                "page-agent-2",
                "conv-history-page-each-once",
                &crate::db::MessageContent::agent(vec![ContentBlock::ToolUse {
                    id: "tool-page-2".to_string(),
                    name: "read_file".to_string(),
                    input: serde_json::json!({"path": "newer"}),
                }]),
                None,
                None,
            )
            .await
            .expect("add newer agent");
        for idx in 0..45 {
            state
                .db
                .add_message(
                    &format!("page-tool-2-{idx}"),
                    "conv-history-page-each-once",
                    &crate::db::MessageContent::tool(
                        "tool-page-2",
                        format!("newer tool output {idx}"),
                        false,
                    ),
                    None,
                    None,
                )
                .await
                .expect("add newer tool");
        }

        let mut pages = Vec::new();
        let Json(latest) = get_conversation_messages_latest(
            State(state.clone()),
            Path("conv-history-page-each-once".to_string()),
            Query(LatestMessagesQuery { limit: Some(20) }),
        )
        .await
        .expect("latest messages");
        pages.push(latest);

        while pages.last().is_some_and(|page| page.has_older_messages) {
            let before_sequence = pages
                .last()
                .and_then(|page| page.messages.first())
                .map(|message| message.sequence_id)
                .expect("non-empty page");
            let Json(older) = get_conversation_messages(
                State(state.clone()),
                Path("conv-history-page-each-once".to_string()),
                Query(MessageHistoryQuery {
                    before_message_sequence: Some(before_sequence),
                    after_message_sequence: None,
                    limit: Some(20),
                }),
            )
            .await
            .expect("older messages");
            pages.push(older);
        }

        let mut chronological = Vec::new();
        for page in pages.iter().rev() {
            chronological.extend(page.messages.iter().map(|message| message.sequence_id));
        }

        let expected = (1..=139).collect::<Vec<_>>();
        assert_eq!(chronological, expected);
        assert_eq!(
            chronological.iter().copied().collect::<HashSet<_>>().len(),
            chronological.len()
        );
    }

    #[tokio::test]
    async fn message_history_before_after_and_around_use_explicit_query_names() {
        let state = make_test_state().await;
        state
            .db
            .create_conversation("conv-history-2", "history-2", "/tmp", true, None, None)
            .await
            .expect("create conversation");

        for idx in 1..=6 {
            state
                .db
                .add_message(
                    &format!("hist2-msg-{idx}"),
                    "conv-history-2",
                    &crate::db::MessageContent::user(format!("m{idx}")),
                    None,
                    None,
                )
                .await
                .expect("add message");
        }

        let Json(before) = get_conversation_messages(
            State(state.clone()),
            Path("conv-history-2".to_string()),
            Query(MessageHistoryQuery {
                before_message_sequence: Some(5),
                after_message_sequence: None,
                limit: Some(2),
            }),
        )
        .await
        .expect("before query");
        assert_eq!(
            before
                .messages
                .iter()
                .map(|m| m.sequence_id)
                .collect::<Vec<_>>(),
            vec![3, 4]
        );
        assert_eq!(before.server_message_tail, Some(6));

        let Json(after) = get_conversation_messages(
            State(state.clone()),
            Path("conv-history-2".to_string()),
            Query(MessageHistoryQuery {
                before_message_sequence: None,
                after_message_sequence: Some(2),
                limit: Some(2),
            }),
        )
        .await
        .expect("after query");
        assert_eq!(
            after
                .messages
                .iter()
                .map(|m| m.sequence_id)
                .collect::<Vec<_>>(),
            vec![3, 4]
        );
        assert_eq!(after.server_message_tail, Some(6));

        let Json(around) = get_conversation_messages_around(
            State(state.clone()),
            Path(("conv-history-2".to_string(), 4)),
            Query(AroundMessagesQuery {
                before: Some(2),
                after: Some(1),
            }),
        )
        .await
        .expect("around query");
        assert_eq!(
            around
                .before
                .iter()
                .map(|m| m.sequence_id)
                .collect::<Vec<_>>(),
            vec![2, 3]
        );
        assert_eq!(
            around
                .after
                .iter()
                .map(|m| m.sequence_id)
                .collect::<Vec<_>>(),
            vec![5]
        );
        assert!(around.tombstones.is_empty());
        assert_eq!(around.server_message_tail, Some(6));

        let err = get_conversation_messages(
            State(state),
            Path("conv-history-2".to_string()),
            Query(MessageHistoryQuery {
                before_message_sequence: Some(5),
                after_message_sequence: Some(2),
                limit: Some(2),
            }),
        )
        .await
        .expect_err("ambiguous query should fail");
        assert!(matches!(err, AppError::BadRequest(_)));
    }

    #[tokio::test]
    async fn message_history_limit_zero_is_bad_request_for_latest_before_after_and_around() {
        let state = make_test_state().await;
        state
            .db
            .create_conversation(
                "conv-history-zero-limit",
                "history-zero-limit",
                "/tmp",
                true,
                None,
                None,
            )
            .await
            .expect("create conversation");
        state
            .db
            .add_message(
                "zero-limit-msg-1",
                "conv-history-zero-limit",
                &crate::db::MessageContent::user("m1"),
                None,
                None,
            )
            .await
            .expect("add message");

        let latest_err = get_conversation_messages_latest(
            State(state.clone()),
            Path("conv-history-zero-limit".to_string()),
            Query(LatestMessagesQuery { limit: Some(0) }),
        )
        .await
        .expect_err("latest limit=0 should fail");
        assert!(matches!(latest_err, AppError::BadRequest(_)));

        let before_err = get_conversation_messages(
            State(state.clone()),
            Path("conv-history-zero-limit".to_string()),
            Query(MessageHistoryQuery {
                before_message_sequence: Some(1),
                after_message_sequence: None,
                limit: Some(0),
            }),
        )
        .await
        .expect_err("before limit=0 should fail");
        assert!(matches!(before_err, AppError::BadRequest(_)));

        let after_err = get_conversation_messages(
            State(state.clone()),
            Path("conv-history-zero-limit".to_string()),
            Query(MessageHistoryQuery {
                before_message_sequence: None,
                after_message_sequence: Some(1),
                limit: Some(0),
            }),
        )
        .await
        .expect_err("after limit=0 should fail");
        assert!(matches!(after_err, AppError::BadRequest(_)));

        let around_before_err = get_conversation_messages_around(
            State(state.clone()),
            Path(("conv-history-zero-limit".to_string(), 1)),
            Query(AroundMessagesQuery {
                before: Some(0),
                after: Some(1),
            }),
        )
        .await
        .expect_err("around before=0 should fail");
        assert!(matches!(around_before_err, AppError::BadRequest(_)));

        let around_after_err = get_conversation_messages_around(
            State(state),
            Path(("conv-history-zero-limit".to_string(), 1)),
            Query(AroundMessagesQuery {
                before: Some(1),
                after: Some(0),
            }),
        )
        .await
        .expect_err("around after=0 should fail");
        assert!(matches!(around_after_err, AppError::BadRequest(_)));
    }

    #[tokio::test]
    async fn latest_message_slice_retries_when_transcript_generation_changes_mid_read() {
        let state = make_test_state().await;
        state
            .db
            .create_conversation(
                "conv-history-latest-race",
                "latest-race",
                "/tmp",
                true,
                None,
                None,
            )
            .await
            .expect("create conversation");

        for idx in 1..=3 {
            state
                .db
                .add_message(
                    &format!("latest-race-msg-{idx}"),
                    "conv-history-latest-race",
                    &crate::db::MessageContent::user(format!("m{idx}")),
                    None,
                    None,
                )
                .await
                .expect("add message");
        }

        bump_generation_after_next_stable_read_value("conv-history-latest-race");
        let Json(latest) = get_conversation_messages_latest(
            State(state),
            Path("conv-history-latest-race".to_string()),
            Query(LatestMessagesQuery { limit: Some(2) }),
        )
        .await
        .expect("latest messages");

        assert_eq!(
            latest
                .messages
                .iter()
                .map(|m| m.sequence_id)
                .collect::<Vec<_>>(),
            vec![2, 3]
        );
        assert_eq!(latest.transcript_generation, Some(2));
        assert_eq!(latest.server_message_tail, Some(3));
    }

    #[tokio::test]
    async fn before_message_slice_retries_when_transcript_generation_changes_mid_read() {
        let state = make_test_state().await;
        state
            .db
            .create_conversation(
                "conv-history-before-race",
                "before-race",
                "/tmp",
                true,
                None,
                None,
            )
            .await
            .expect("create conversation");

        for idx in 1..=4 {
            state
                .db
                .add_message(
                    &format!("before-race-msg-{idx}"),
                    "conv-history-before-race",
                    &crate::db::MessageContent::user(format!("m{idx}")),
                    None,
                    None,
                )
                .await
                .expect("add message");
        }

        bump_generation_after_next_stable_read_value("conv-history-before-race");
        let Json(before) = get_conversation_messages(
            State(state),
            Path("conv-history-before-race".to_string()),
            Query(MessageHistoryQuery {
                before_message_sequence: Some(4),
                after_message_sequence: None,
                limit: Some(2),
            }),
        )
        .await
        .expect("before messages");

        assert_eq!(
            before
                .messages
                .iter()
                .map(|m| m.sequence_id)
                .collect::<Vec<_>>(),
            vec![2, 3]
        );
        assert_eq!(before.transcript_generation, Some(2));
        assert_eq!(before.server_message_tail, Some(4));
    }

    #[tokio::test]
    async fn message_history_range_rejects_inverted_bounds() {
        let state = make_test_state().await;
        state
            .db
            .create_conversation(
                "conv-history-range-invalid",
                "history-range-invalid",
                "/tmp",
                true,
                None,
                None,
            )
            .await
            .expect("create conversation");

        let err = get_conversation_message_range(
            State(state),
            Path("conv-history-range-invalid".to_string()),
            Query(MessageRangeQuery {
                start_message_sequence: 4,
                end_message_sequence: 3,
            }),
        )
        .await
        .expect_err("inverted range should fail");
        assert!(matches!(err, AppError::BadRequest(_)));
    }

    #[tokio::test]
    async fn stream_query_prefers_after_event_sequence_and_ignores_legacy_after_sequence() {
        let state = make_test_state().await;

        state
            .db
            .create_conversation("stream-query", "Query test", "/tmp", true, None, None)
            .await
            .expect("create conversation");

        let handle = state
            .runtime
            .get_or_create("stream-query")
            .await
            .expect("runtime handle");
        let _sub = handle.broadcast_tx.subscribe();

        let persisted = state
            .db
            .add_message(
                "stream-msg-1",
                "stream-query",
                &crate::db::MessageContent::agent(vec![phoenix_llm::ContentBlock::text(
                    "persisted",
                )]),
                None,
                None,
            )
            .await
            .expect("persisted message");
        handle
            .broadcast_tx
            .send_persisted_message(persisted.clone())
            .expect("broadcast persisted message");
        handle
            .broadcast_tx
            .send_seq(|seq| SseEvent::Token {
                sequence_id: seq,
                text: "token-a".to_string(),
                request_id: "req".to_string(),
            })
            .expect("token-a");
        handle
            .broadcast_tx
            .send_seq(|seq| SseEvent::Token {
                sequence_id: seq,
                text: "token-b".to_string(),
                request_id: "req".to_string(),
            })
            .expect("token-b");

        let (
            pending_anchor_sequence_id,
            pending_truncated,
            highest_pending_seq,
            pending_events,
            cursor_replay_served,
        ) = snapshot_pending_for_stream(
            &handle.broadcast_tx,
            &StreamConversationQuery {
                after_event_sequence: Some(persisted.sequence_id + 1),
                after_sequence: Some(999_999),
                init_mode: None,
                after_message_floor: None,
                transcript_generation: None,
            },
        );

        assert!(!pending_truncated);
        assert!(cursor_replay_served);
        assert_eq!(highest_pending_seq, persisted.sequence_id + 2);
        assert_eq!(pending_anchor_sequence_id, persisted.sequence_id);
        assert_eq!(
            pending_events.len(),
            1,
            "replay should start after explicit event cursor"
        );
        match &pending_events[0] {
            SseEvent::Token { text, .. } => assert_eq!(text, "token-b"),
            other => panic!("expected token replay, got {other:?}"),
        }
    }

    #[test]
    fn cursored_stream_omits_db_messages_only_when_cursor_covers_db_tail() {
        assert_eq!(
            db_message_selection_for_stream(
                true,
                &StreamConversationQuery {
                    after_event_sequence: Some(10),
                    after_sequence: None,
                    init_mode: None,
                    after_message_floor: None,
                    transcript_generation: Some(1),
                },
                10,
                1,
            ),
            StreamDbMessageSelection::None
        );
        assert_eq!(
            db_message_selection_for_stream(
                true,
                &StreamConversationQuery {
                    after_event_sequence: Some(10),
                    after_sequence: None,
                    init_mode: None,
                    after_message_floor: None,
                    transcript_generation: None,
                },
                10,
                1,
            ),
            StreamDbMessageSelection::Full,
            "a covering event cursor cannot omit DB messages unless the client also proves it is on the current transcript generation"
        );
        assert_eq!(
            db_message_selection_for_stream(
                true,
                &StreamConversationQuery {
                    after_event_sequence: Some(10),
                    after_sequence: None,
                    init_mode: None,
                    after_message_floor: None,
                    transcript_generation: Some(2),
                },
                10,
                3,
            ),
            StreamDbMessageSelection::Full,
            "a served cursor covering the DB tail cannot omit DB messages when the supplied transcript generation is stale"
        );
        assert_eq!(
            db_message_selection_for_stream(
                true,
                &StreamConversationQuery {
                    after_event_sequence: Some(9),
                    after_sequence: None,
                    init_mode: None,
                    after_message_floor: None,
                    transcript_generation: None,
                },
                10,
                1,
            ),
            StreamDbMessageSelection::Full
        );
        assert_eq!(
            db_message_selection_for_stream(
                false,
                &StreamConversationQuery {
                    after_event_sequence: Some(10),
                    after_sequence: None,
                    init_mode: None,
                    after_message_floor: None,
                    transcript_generation: None,
                },
                10,
                1,
            ),
            StreamDbMessageSelection::Full
        );
    }

    #[tokio::test]
    async fn stable_stream_read_forces_full_selection_after_none_generation_race() {
        let state = make_test_state().await;
        state
            .db
            .create_conversation("stream-none-race", "none-race", "/tmp", true, None, None)
            .await
            .expect("create conversation");
        for idx in 1..=2 {
            state
                .db
                .add_message(
                    &format!("stream-none-race-msg-{idx}"),
                    "stream-none-race",
                    &crate::db::MessageContent::user(format!("m{idx}")),
                    None,
                    None,
                )
                .await
                .expect("add message");
        }
        let query = StreamConversationQuery {
            after_event_sequence: Some(2),
            after_sequence: None,
            init_mode: None,
            after_message_floor: None,
            transcript_generation: Some(1),
        };

        bump_generation_after_next_stable_read_value("stream-none-race");
        let stable =
            stable_transcript_read(state.runtime.db(), "stream-none-race", |db, id, attempt| {
                let query = query.clone();
                Box::pin(async move {
                    let last_sequence_id = db.get_last_sequence_id(id).await.unwrap_or(0);
                    let mut selection = db_message_selection_for_stream(
                        true,
                        &query,
                        last_sequence_id,
                        db.get_conversation(id)
                            .await
                            .map_err(|e| AppError::NotFound(e.to_string()))?
                            .transcript_generation,
                    );
                    if attempt > 1 {
                        selection = StreamDbMessageSelection::Full;
                    }
                    let messages = match selection {
                        StreamDbMessageSelection::None => Vec::new(),
                        StreamDbMessageSelection::Full => db
                            .get_messages(id)
                            .await
                            .map_err(|e| AppError::Internal(e.to_string()))?,
                        StreamDbMessageSelection::AfterFloor(after_floor) => db
                            .get_messages_after(id, after_floor)
                            .await
                            .map_err(|e| AppError::Internal(e.to_string()))?,
                    };
                    Ok((selection, messages))
                })
            })
            .await
            .expect("stable stream read");

        let (selection, messages) = stable.value;
        assert_eq!(stable.attempts, 2);
        assert_eq!(stable.conversation.transcript_generation, 2);
        assert_eq!(selection, StreamDbMessageSelection::Full);
        assert_eq!(
            messages.iter().map(|m| m.sequence_id).collect::<Vec<_>>(),
            vec![1, 2]
        );
    }

    #[tokio::test]
    async fn stable_stream_read_forces_full_selection_after_after_floor_generation_race() {
        let state = make_test_state().await;
        state
            .db
            .create_conversation("stream-floor-race", "floor-race", "/tmp", true, None, None)
            .await
            .expect("create conversation");
        for idx in 1..=3 {
            state
                .db
                .add_message(
                    &format!("stream-floor-race-msg-{idx}"),
                    "stream-floor-race",
                    &crate::db::MessageContent::user(format!("m{idx}")),
                    None,
                    None,
                )
                .await
                .expect("add message");
        }
        let query = StreamConversationQuery {
            after_event_sequence: None,
            after_sequence: None,
            init_mode: Some(StreamInitMode::MessagesAfterFloor),
            after_message_floor: Some(2),
            transcript_generation: Some(1),
        };

        bump_generation_after_next_stable_read_value("stream-floor-race");
        let stable = stable_transcript_read(
            state.runtime.db(),
            "stream-floor-race",
            |db, id, attempt| {
                let query = query.clone();
                Box::pin(async move {
                    let last_sequence_id = db.get_last_sequence_id(id).await.unwrap_or(0);
                    let mut selection = db_message_selection_for_stream(
                        false,
                        &query,
                        last_sequence_id,
                        db.get_conversation(id)
                            .await
                            .map_err(|e| AppError::NotFound(e.to_string()))?
                            .transcript_generation,
                    );
                    if attempt > 1 {
                        selection = StreamDbMessageSelection::Full;
                    }
                    let messages = match selection {
                        StreamDbMessageSelection::None => Vec::new(),
                        StreamDbMessageSelection::Full => db
                            .get_messages(id)
                            .await
                            .map_err(|e| AppError::Internal(e.to_string()))?,
                        StreamDbMessageSelection::AfterFloor(after_floor) => db
                            .get_messages_after(id, after_floor)
                            .await
                            .map_err(|e| AppError::Internal(e.to_string()))?,
                    };
                    Ok((selection, messages))
                })
            },
        )
        .await
        .expect("stable stream read");

        let (selection, messages) = stable.value;
        assert_eq!(stable.attempts, 2);
        assert_eq!(stable.conversation.transcript_generation, 2);
        assert_eq!(selection, StreamDbMessageSelection::Full);
        assert_eq!(
            messages.iter().map(|m| m.sequence_id).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
    }

    #[test]
    fn demand_driven_stream_modes_select_db_messages_by_rest_floor() {
        assert_eq!(
            db_message_selection_for_stream(
                false,
                &StreamConversationQuery {
                    after_event_sequence: None,
                    after_sequence: None,
                    init_mode: Some(StreamInitMode::MessagesAfterFloor),
                    after_message_floor: Some(50),
                    transcript_generation: Some(3),
                },
                75,
                3,
            ),
            StreamDbMessageSelection::AfterFloor(50)
        );
        assert_eq!(
            db_message_selection_for_stream(
                false,
                &StreamConversationQuery {
                    after_event_sequence: None,
                    after_sequence: None,
                    init_mode: Some(StreamInitMode::MessagesAfterFloor),
                    after_message_floor: Some(50),
                    transcript_generation: Some(2),
                },
                75,
                3,
            ),
            StreamDbMessageSelection::Full
        );
        assert_eq!(
            db_message_selection_for_stream(
                false,
                &StreamConversationQuery {
                    after_event_sequence: None,
                    after_sequence: None,
                    init_mode: Some(StreamInitMode::MessagesAfterFloor),
                    after_message_floor: None,
                    transcript_generation: Some(3),
                },
                75,
                3,
            ),
            StreamDbMessageSelection::Full
        );
    }

    #[tokio::test]
    async fn single_conversation_response_includes_cached_pr_for_primary_work_scope_association() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cwd = tmp.path().join("repo");
        let worktree = tmp.path().join("worktree");
        std::fs::create_dir_all(&cwd).expect("cwd");
        std::fs::create_dir_all(&worktree).expect("worktree");
        let state = make_test_state().await;
        let mode = crate::db::ConvMode::Work {
            branch_name: crate::db::NonEmptyString::new("task-36002".to_string()).unwrap(),
            worktree_path: crate::db::NonEmptyString::new(worktree.to_string_lossy().to_string())
                .unwrap(),
            base_branch: crate::db::NonEmptyString::new("main".to_string()).unwrap(),
            task_id: crate::db::NonEmptyString::new("36002".to_string()).unwrap(),
            task_title: crate::db::NonEmptyString::new("Seed PR badge".to_string()).unwrap(),
        };
        state
            .db
            .create_conversation_with_project(
                "c-cached-pr",
                "cached-pr",
                cwd.to_str().unwrap(),
                true,
                None,
                None,
                None,
                &mode,
                None,
                None,
                None,
                crate::llm_language::LlmLanguage::default(),
            )
            .await
            .expect("create conversation");
        state
            .db
            .upsert_work_scope_pr_observations(
                &crate::work_scope::WorkScope::resolve(
                    "c-cached-pr",
                    Some(std::path::Path::new(worktree.to_str().unwrap())),
                ),
                &[crate::db::WorkScopePrObservation {
                    repo_owner: "example".to_string(),
                    repo_name: "repo".to_string(),
                    pr_number: 44,
                    title: "Cached PR".to_string(),
                    url: "https://github.com/example/repo/pull/44".to_string(),
                    state: "OPEN".to_string(),
                    draft: false,
                    display_state: phoenix_core::domain::pr_display_state::PrDisplayState::Open,
                    base: "main".to_string(),
                    head: "task-36002".to_string(),
                    github_updated_at: Some("2026-01-01T00:00:00Z".to_string()),
                }],
            )
            .await
            .expect("upsert pr association");

        let conv = state
            .db
            .get_conversation("c-cached-pr")
            .await
            .expect("conversation");
        let enriched = enrich_conversation_with_seed(&state, &conv, true)
            .await
            .expect("enriched conversation");
        let init_cached_pr = enriched.cached_pr.expect("init cached_pr");
        assert_eq!(init_cached_pr.number, 44);

        let token = state
            .db
            .create_share_token("c-cached-pr")
            .await
            .expect("share token");
        let Json(shared_response) = get_shared_conversation(State(state.clone()), Path(token))
            .await
            .expect("shared conversation");
        assert!(
            shared_response.conversation.get("cached_pr").is_none(),
            "share payload must not expose private PR metadata"
        );

        let Json(response) = get_conversation(
            State(state),
            Path("c-cached-pr".to_string()),
            Query(GetConversationQuery {
                after_sequence: None,
            }),
        )
        .await
        .expect("get conversation");

        let cached_pr = response.conversation.get("cached_pr").expect("cached_pr");
        assert_eq!(cached_pr["number"], serde_json::json!(44));
        assert_eq!(cached_pr["title"], serde_json::json!("Cached PR"));
        assert_eq!(
            cached_pr["url"],
            serde_json::json!("https://github.com/example/repo/pull/44")
        );
        assert_eq!(cached_pr["display_state"], serde_json::json!("open"));
        assert_eq!(cached_pr["base"], serde_json::json!("main"));
        assert_eq!(cached_pr["head"], serde_json::json!("task-36002"));
    }

    #[tokio::test]
    async fn mcp_oauth_callback_rejects_unmatched_state_and_error_redirects() {
        use axum::extract::Query;

        let state = make_test_state().await;

        // A callback matching no pending flow is rejected (REQ-MCP-011).
        let response = super::mcp_oauth_callback(
            State(state.clone()),
            Query(super::McpOAuthCallbackParams {
                code: Some("code-1".to_string()),
                state: Some("nonexistent".to_string()),
                iss: None,
                error: None,
                error_description: None,
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        // A missing code/state pair is rejected.
        let response = super::mcp_oauth_callback(
            State(state.clone()),
            Query(super::McpOAuthCallbackParams {
                code: None,
                state: None,
                iss: None,
                error: None,
                error_description: None,
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        // An error redirect reports the denial.
        let response = super::mcp_oauth_callback(
            State(state),
            Query(super::McpOAuthCallbackParams {
                code: None,
                state: Some("nonexistent".to_string()),
                iss: None,
                error: Some("access_denied".to_string()),
                error_description: Some("the user said no".to_string()),
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn work_scope_inventory_empty_scope_returns_empty_inventory() {
        let state = make_test_state().await;
        let scope = crate::work_scope::WorkScope::Conversation("conv-empty".to_string());
        let Json(inv) = super::get_work_scope_inventory(State(state), Path(scope.stable_key()))
            .await
            .expect("inventory");
        assert_eq!(inv.scope_key, scope.stable_key());
        assert!(inv.bash.is_empty());
        assert!(inv.tmux.is_none());
        assert!(inv.browser.is_none());
    }

    #[tokio::test]
    async fn work_scope_inventory_reports_a_live_bash_handle() {
        use phoenix_core::domain::work_scope_inventory::BashHandleState;
        use phoenix_tools::bash::handle::{Handle, HandleId};
        use phoenix_tools::bash::ring::RING_BUFFER_BYTES;

        let state = make_test_state().await;
        let scope = crate::work_scope::WorkScope::Conversation("conv-live".to_string());

        // Insert a live handle directly into the WorkScope-keyed registry.
        let table = state.runtime.bash_handles().get_or_create(&scope).await;
        let handle = Handle::new_live(
            scope.clone(),
            HandleId::new("b-1"),
            "npm run dev".into(),
            Some("dev".into()),
            4321,
            1234,
            RING_BUFFER_BYTES,
        );
        table.write().await.insert(handle);

        let Json(inv) = super::get_work_scope_inventory(State(state), Path(scope.stable_key()))
            .await
            .expect("inventory");

        assert_eq!(inv.bash.len(), 1);
        let h = &inv.bash[0];
        assert_eq!(h.handle_id, "b-1");
        assert_eq!(h.label.as_deref(), Some("dev"));
        assert_eq!(h.state, BashHandleState::Running);
        assert_eq!(h.pid, Some(1234));
        assert_eq!(h.pgid, Some(4321));
        assert!(h.duration_ms.is_none());
    }

    #[tokio::test]
    async fn work_scope_inventory_rejects_malformed_key() {
        let state = make_test_state().await;
        let err = super::get_work_scope_inventory(State(state), Path("bogus-no-namespace".into()))
            .await
            .expect_err("malformed key must be rejected");
        assert!(matches!(err, AppError::BadRequest(_)));
    }

    #[tokio::test]
    async fn stop_conversation_browser_session_rejects_missing_conversation() {
        let state = make_test_state().await;
        let err = super::stop_conversation_browser_session(
            State(state),
            Path("missing-conversation".into()),
        )
        .await
        .expect_err("missing conversation must be rejected");
        assert!(matches!(err, AppError::NotFound(_)));
    }

    #[tokio::test]
    async fn stop_work_scope_browser_session_rejects_malformed_key() {
        let state = make_test_state().await;
        let err =
            super::stop_work_scope_browser_session(State(state), Path("bogus-no-namespace".into()))
                .await
                .expect_err("malformed key must be rejected");
        assert!(matches!(err, AppError::BadRequest(_)));
    }

    #[tokio::test]
    async fn stop_work_scope_browser_session_absent_session_is_successful_noop() {
        let state = make_test_state().await;
        let scope = crate::work_scope::WorkScope::Conversation("conv-no-browser".to_string());

        let Json(resp) =
            super::stop_work_scope_browser_session(State(state.clone()), Path(scope.stable_key()))
                .await
                .expect("absent browser stop should succeed");

        assert!(resp.success);
        assert!(!state.runtime.browser_sessions().is_active(&scope).await);
    }

    // ----------------------------------------------------------------
    // Process inspector endpoint (specs/process-inspector/ REQ-PINSP-*)
    // ----------------------------------------------------------------

    /// REQ-PINSP-001/002/003/004: a live handle's inspection reports identity
    /// and state, an output window (with the appended line), and — on the host
    /// platform — a non-null resource sample. The handle wraps a REAL spawned
    /// child in its own process group so the macOS `proc_listpgrppids` /
    /// `proc_pid_rusage` (and Linux `/proc`) reads find live members.
    #[cfg(unix)]
    #[tokio::test]
    // `pid` and `pgid` are the canonical Unix process/process-group names.
    #[allow(clippy::similar_names)]
    async fn inspect_live_handle_reports_state_output_and_resources() {
        use phoenix_core::domain::work_scope_inventory::BashHandleState;
        use phoenix_tools::bash::handle::{Handle, HandleId, HandleState};
        use phoenix_tools::bash::ring::RING_BUFFER_BYTES;
        use std::os::unix::process::CommandExt as _;
        use std::process::Stdio;

        let state = make_test_state().await;
        let scope = crate::work_scope::WorkScope::Conversation("conv-inspect-live".to_string());

        // Spawn a real, long-lived child as its own process-group leader so the
        // sampler has a live group to read. setpgid(0,0) makes pgid == pid.
        let mut cmd = std::process::Command::new("sleep");
        cmd.arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        unsafe {
            cmd.pre_exec(|| {
                if libc::setpgid(0, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = cmd.spawn().expect("spawn sleep");
        let pid = child.id();
        #[allow(clippy::cast_possible_wrap)]
        let pgid = pid as i32;

        let table = state.runtime.bash_handles().get_or_create(&scope).await;
        let handle = Handle::new_live(
            scope.clone(),
            HandleId::new("b-1"),
            "sleep 30".into(),
            Some("sleeper".into()),
            pgid,
            pid,
            RING_BUFFER_BYTES,
        );
        // Seed the ring with a line so the output window is non-empty.
        if let HandleState::Live(live) = handle.state().await.as_ref() {
            live.ring.lock().await.append(b"hello from inspector\n");
        }
        table.write().await.insert(handle);

        let Json(inspection) = super::inspect_bash_handle(
            State(state),
            Path((scope.stable_key(), "b-1".to_string())),
            Query(super::InspectQuery { since: None }),
        )
        .await
        .expect("inspection");

        assert_eq!(inspection.handle_id, "b-1");
        assert_eq!(inspection.label.as_deref(), Some("sleeper"));
        assert_eq!(inspection.state, BashHandleState::Running);
        assert_eq!(inspection.pid, Some(pid));
        assert_eq!(inspection.pgid, Some(pgid));
        assert!(inspection.duration_ms.is_none());
        // Output window carries the seeded line.
        assert!(
            inspection
                .output
                .lines
                .iter()
                .any(|l| l.bytes.contains("hello from inspector")),
            "output window must include the seeded ring line"
        );
        // Resource sample present and populated on the host platform.
        let resources = inspection
            .resources
            .expect("live handle must carry a resource sample");
        assert!(
            resources.process_count.is_some_and(|c| c >= 1),
            "process_count must see the live group: {:?}",
            resources.process_count
        );
        assert!(
            resources.memory_bytes.is_some_and(|m| m > 0),
            "memory_bytes must be a real proportional figure: {:?}",
            resources.memory_bytes
        );
        assert!(
            resources.cpu_pct.is_some(),
            "cpu_pct must be a real (possibly 0.0) sample, not null"
        );

        // Clean up the spawned child.
        unsafe {
            let _ = libc::kill(-pgid, libc::SIGKILL);
        }
        let _ = child.wait();
    }

    /// REQ-PINSP-002/003/004: a terminal handle reports the exit cause and
    /// duration, serves its output from the tombstone tail, and carries NO
    /// resource sample (`resources: None`).
    #[tokio::test]
    async fn inspect_terminal_handle_has_no_resources_and_serves_tombstone_output() {
        use phoenix_core::domain::work_scope_inventory::BashHandleState;
        use phoenix_tools::bash::handle::{FinalCause, Handle, HandleId, HandleState};
        use phoenix_tools::bash::ring::RING_BUFFER_BYTES;

        let state = make_test_state().await;
        let scope = crate::work_scope::WorkScope::Conversation("conv-inspect-term".to_string());

        let table = state.runtime.bash_handles().get_or_create(&scope).await;
        let handle = Handle::new_live(
            scope.clone(),
            HandleId::new("b-1"),
            "echo bye".into(),
            None,
            7,
            7,
            RING_BUFFER_BYTES,
        );
        // Seed output, then demote so the tombstone retains the tail.
        if let HandleState::Live(live) = handle.state().await.as_ref() {
            live.ring.lock().await.append(b"final line\n");
        }
        handle
            .transition_to_terminal(
                FinalCause::Exited { exit_code: Some(0) },
                std::time::Duration::from_millis(21),
                phoenix_tools::bash::handle::TOMBSTONE_TAIL_LINES,
            )
            .await;
        table.write().await.insert(handle);

        let Json(inspection) = super::inspect_bash_handle(
            State(state),
            Path((scope.stable_key(), "b-1".to_string())),
            Query(super::InspectQuery { since: None }),
        )
        .await
        .expect("inspection");

        assert_eq!(inspection.state, BashHandleState::Tombstoned);
        assert_eq!(inspection.exit_code, Some(0));
        assert_eq!(inspection.duration_ms, Some(21));
        assert!(inspection.pid.is_none());
        assert!(inspection.pgid.is_none());
        assert!(
            inspection.resources.is_none(),
            "a terminal handle has no process group — resources must be None"
        );
        assert!(
            inspection
                .output
                .lines
                .iter()
                .any(|l| l.bytes.contains("final line")),
            "tombstone output tail must include the final line"
        );
    }

    #[tokio::test]
    async fn inspect_rejects_malformed_scope_key() {
        let state = make_test_state().await;
        let err = super::inspect_bash_handle(
            State(state),
            Path(("bogus-no-namespace".to_string(), "b-1".to_string())),
            Query(super::InspectQuery { since: None }),
        )
        .await
        .expect_err("malformed key must be rejected");
        assert!(matches!(err, AppError::BadRequest(_)));
    }

    #[tokio::test]
    async fn inspect_unknown_handle_is_not_found() {
        let state = make_test_state().await;
        let scope = crate::work_scope::WorkScope::Conversation("conv-inspect-missing".to_string());
        // Scope with a table but no such handle.
        let _ = state.runtime.bash_handles().get_or_create(&scope).await;
        let err = super::inspect_bash_handle(
            State(state),
            Path((scope.stable_key(), "b-404".to_string())),
            Query(super::InspectQuery { since: None }),
        )
        .await
        .expect_err("unknown handle must be not-found");
        assert!(matches!(err, AppError::NotFound(_)));
    }

    /// REQ-WSUI-007 / REQ-WSUI-008: a work-scope change for a scope with a
    /// live runtime handle broadcasts a `WorkScopeUpdate` carrying the full
    /// refreshed inventory to that conversation's SSE stream. Mirrors
    /// `broadcasts_hard_deleted_event_to_existing_subscribers` — exercises the
    /// bridge's assemble + scope-routed broadcast directly via
    /// `broadcast_work_scope_update`, the path both the bash and browser
    /// signal arms funnel through.
    #[tokio::test]
    async fn work_scope_update_broadcast_carries_inventory_for_live_bash_handle() {
        use phoenix_core::domain::work_scope_inventory::BashHandleState;
        use phoenix_tools::bash::handle::{Handle, HandleId};
        use phoenix_tools::bash::ring::RING_BUFFER_BYTES;

        let state = make_test_state().await;
        state
            .db
            .create_conversation("c-ws", "test", "/tmp", true, None, None)
            .await
            .expect("create");

        // A Direct-mode conversation resolves to WorkScope::Conversation(id).
        let scope = crate::work_scope::WorkScope::Conversation("c-ws".to_string());

        // Insert a live bash handle into the scope's registry table so the
        // assembled inventory has something to report.
        let table = state.runtime.bash_handles().get_or_create(&scope).await;
        table.write().await.insert(Handle::new_live(
            scope.clone(),
            HandleId::new("b-1"),
            "npm run dev".into(),
            Some("dev".into()),
            4321,
            1234,
            RING_BUFFER_BYTES,
        ));

        // Force a runtime handle (so a broadcaster exists) and subscribe
        // before the bridge fires.
        let mut rx = state.runtime.subscribe("c-ws").await.expect("subscribe");

        state.runtime.broadcast_work_scope_update(&scope).await;

        let mut saw_update = false;
        while let Ok(event) =
            tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await
        {
            match event {
                Ok(SseEvent::WorkScopeUpdate { inventory, .. }) => {
                    assert_eq!(inventory.scope_key, scope.stable_key());
                    assert_eq!(inventory.bash.len(), 1);
                    let h = &inventory.bash[0];
                    assert_eq!(h.handle_id, "b-1");
                    assert_eq!(h.label.as_deref(), Some("dev"));
                    assert_eq!(h.state, BashHandleState::Running);
                    assert_eq!(h.pid, Some(1234));
                    saw_update = true;
                    break;
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
        assert!(
            saw_update,
            "WorkScopeUpdate SSE event must be broadcast to the scope's conversation"
        );
    }

    #[tokio::test]
    async fn sse_init_db_read_after_replay_snapshot_covers_persist_between_old_reads() {
        let state = make_test_state().await;
        let conv_id = "c-sse-gap";
        state
            .db
            .create_conversation(conv_id, "sse-gap", "/tmp", true, None, None)
            .await
            .expect("create conversation");

        let handle = state.runtime.get_or_create(conv_id).await.expect("handle");
        let mut broadcast_rx = handle.broadcast_tx.subscribe();

        let last_sequence_id_before_persist = state
            .db
            .get_last_sequence_id(conv_id)
            .await
            .expect("initial last seq");
        let (pending_anchor_sequence_id, _, highest_pending_seq, _) =
            handle.broadcast_tx.snapshot_pending();
        let init_seq_before_db_read =
            std::cmp::max(last_sequence_id_before_persist, highest_pending_seq);

        let seq = handle.broadcast_tx.next_seq();
        let msg = state
            .db
            .add_message_with_seq(
                "local-user-1",
                conv_id,
                seq,
                &MessageContent::User(crate::db::UserContent::new("hello")),
                None,
                None,
            )
            .await
            .expect("persist message");
        handle
            .broadcast_tx
            .send_persisted_message(msg.clone())
            .expect("subscribed receiver observes broadcast");

        let messages = state.db.get_messages(conv_id).await.expect("messages");
        let highest_message_seq = messages.iter().map(|m| m.sequence_id).max().unwrap_or(0);
        let init_seq = std::cmp::max(init_seq_before_db_read, highest_message_seq);

        assert_eq!(pending_anchor_sequence_id, 0);
        assert!(
            messages.iter().any(|m| m.message_id == "local-user-1"),
            "DB snapshot is taken after replay snapshot, so the raced persist is durable in init"
        );
        assert_eq!(init_seq, seq);

        let live = broadcast_rx
            .recv()
            .await
            .expect("live message after subscribe");
        match live {
            SseEvent::Message { message } => assert_eq!(message.message_id, msg.message_id),
            other => panic!("expected live message event, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn cancel_rejects_awaiting_continuation_without_changing_state() {
        let state = make_test_state().await;
        state
            .db
            .create_conversation("c-continuation", "continuation", "/tmp", true, None, None)
            .await
            .expect("create");
        state
            .db
            .update_conversation_state(
                "c-continuation",
                &ConvState::AwaitingContinuation {
                    rejected_tool_calls: vec![],
                    attempt: 1,
                },
            )
            .await
            .expect("update state");

        let err = cancel_conversation(State(state.clone()), Path("c-continuation".to_string()))
            .await
            .expect_err("awaiting continuation cancel should be rejected");

        match err {
            AppError::Conflict(detail) => assert_eq!(detail.error_type, "cannot_cancel_state"),
            other => panic!("expected conflict, got {other:?}"),
        }

        let conv = state
            .db
            .get_conversation("c-continuation")
            .await
            .expect("conversation still exists");
        assert!(matches!(
            conv.state,
            ConvState::AwaitingContinuation { attempt: 1, .. }
        ));
    }

    #[tokio::test]
    async fn cancel_rejects_awaiting_user_response_without_dispatching_invalid_transition() {
        let state = make_test_state().await;
        state
            .db
            .create_conversation("c-question", "question", "/tmp", true, None, None)
            .await
            .expect("create");
        state
            .db
            .update_conversation_state(
                "c-question",
                &ConvState::AwaitingUserResponse {
                    questions: vec![],
                    tool_use_id: "tool-question".to_string(),
                },
            )
            .await
            .expect("update state");

        let err = cancel_conversation(State(state.clone()), Path("c-question".to_string()))
            .await
            .expect_err("question response prompt is dismissed through its dedicated endpoint");

        match err {
            AppError::Conflict(detail) => assert_eq!(detail.error_type, "cannot_cancel_state"),
            other => panic!("expected conflict, got {other:?}"),
        }

        let conv = state
            .db
            .get_conversation("c-question")
            .await
            .expect("conversation still exists");
        assert!(matches!(conv.state, ConvState::AwaitingUserResponse { .. }));
    }

    #[tokio::test]
    async fn search_conversation_files_resolves_against_cwd_not_worktree() {
        // File search resolves against the conversation's `cwd` — the same root
        // `message_expander::expand` uses for `@file` references at send time —
        // even for Work-mode conversations that also have a worktree. A
        // worktree-only file must NOT autocomplete, because it would fail to
        // expand.
        let tmp = tempfile::tempdir().expect("tempdir");
        let cwd = tmp.path().join("repo");
        let worktree = tmp.path().join("worktree");
        std::fs::create_dir_all(cwd.join("src")).expect("cwd dirs");
        std::fs::create_dir_all(worktree.join("src")).expect("worktree dirs");
        std::fs::write(cwd.join("src/in_cwd.rs"), "fn in_cwd() {}\n").expect("cwd file");
        std::fs::write(worktree.join("src/in_worktree.rs"), "fn in_worktree() {}\n")
            .expect("worktree file");

        let state = make_test_state().await;
        let mode = crate::db::ConvMode::Work {
            branch_name: crate::db::NonEmptyString::new("task-93001").unwrap(),
            worktree_path: crate::db::NonEmptyString::new(worktree.to_string_lossy().to_string())
                .unwrap(),
            base_branch: crate::db::NonEmptyString::new("main").unwrap(),
            task_id: crate::db::NonEmptyString::new("93001").unwrap(),
            task_title: crate::db::NonEmptyString::new("Restore Cmd P").unwrap(),
        };
        state
            .db
            .create_conversation_with_project(
                "c-file-root",
                "file-root",
                cwd.to_str().unwrap(),
                true,
                None,
                None,
                None,
                &mode,
                None,
                None,
                None,
                crate::llm_language::LlmLanguage::default(),
            )
            .await
            .expect("create");

        // Unfiltered search returns the cwd file and not the worktree-only file.
        let Json(response) = search_conversation_files(
            State(state),
            Path("c-file-root".to_string()),
            Query(FileSearchQuery {
                q: String::new(),
                limit: Some(10),
            }),
        )
        .await
        .expect("search");

        let paths: Vec<_> = response.items.into_iter().map(|item| item.path).collect();
        assert_eq!(paths, vec!["src/in_cwd.rs"]);
    }

    #[tokio::test]
    async fn search_conversation_files_falls_back_to_cwd_for_direct_conversations() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cwd = tmp.path().join("repo");
        std::fs::create_dir_all(cwd.join("src")).expect("dirs");
        std::fs::write(cwd.join("src/direct.rs"), "fn direct() {}\n").expect("file");

        let state = make_test_state().await;
        state
            .db
            .create_conversation_with_project(
                "c-direct-root",
                "direct-root",
                cwd.to_str().unwrap(),
                true,
                None,
                None,
                None,
                &crate::db::ConvMode::Direct,
                None,
                None,
                None,
                crate::llm_language::LlmLanguage::default(),
            )
            .await
            .expect("create");

        let Json(response) = search_conversation_files(
            State(state),
            Path("c-direct-root".to_string()),
            Query(FileSearchQuery {
                q: "direct".to_string(),
                limit: Some(10),
            }),
        )
        .await
        .expect("search");

        let paths: Vec<_> = response.items.into_iter().map(|item| item.path).collect();
        assert_eq!(paths, vec!["src/direct.rs"]);
    }

    #[tokio::test]
    async fn search_project_files_walks_an_explicit_directory() {
        // The new-conversation composer searches a raw directory with no
        // conversation in the database.
        let tmp = tempfile::tempdir().expect("tempdir");
        let cwd = tmp.path().join("repo");
        std::fs::create_dir_all(cwd.join("src")).expect("dirs");
        std::fs::write(cwd.join("src/project.rs"), "fn project() {}\n").expect("file");

        let Json(response) = search_project_files(Query(ProjectFileSearchQuery {
            cwd: cwd.to_string_lossy().to_string(),
            q: "project".to_string(),
            limit: Some(10),
            mode: None,
            base_branch: None,
        }))
        .await
        .expect("search");

        let paths: Vec<_> = response.items.into_iter().map(|item| item.path).collect();
        assert_eq!(paths, vec!["src/project.rs"]);
    }

    #[tokio::test]
    async fn search_project_files_rejects_missing_directory() {
        let err = search_project_files(Query(ProjectFileSearchQuery {
            cwd: "/nonexistent/phoenix/test/dir".to_string(),
            q: String::new(),
            limit: None,
            mode: None,
            base_branch: None,
        }))
        .await
        .expect_err("missing dir rejected");
        assert!(matches!(err, AppError::BadRequest(_)));
    }

    #[tokio::test]
    async fn search_conversation_code_uses_smart_case_and_line_metadata() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cwd = tmp.path().join("repo");
        std::fs::create_dir_all(cwd.join("src")).expect("dirs");
        std::fs::write(
            cwd.join("src/main.rs"),
            "let metricSourceToOriginProduct = 1;\nlet metricsourcetooriginproduct = 2;\n",
        )
        .expect("file");

        let state = make_test_state().await;
        state
            .db
            .create_conversation_with_project(
                "c-code-case",
                "code-case",
                cwd.to_str().unwrap(),
                true,
                None,
                None,
                None,
                &crate::db::ConvMode::Direct,
                None,
                None,
                None,
                crate::llm_language::LlmLanguage::default(),
            )
            .await
            .expect("create");

        let Json(lower) = search_conversation_code(
            State(state.clone()),
            Path("c-code-case".to_string()),
            Query(CodeSearchQuery {
                q: "metricsource".to_string(),
                limit: Some(10),
            }),
        )
        .await
        .expect("lower search");
        assert_eq!(lower.items.len(), 2);
        assert_eq!(lower.items[0].path, "src/main.rs");
        assert_eq!(lower.items[0].line_number, 1);
        assert_eq!(lower.items[0].match_start, 4);
        assert_eq!(lower.items[0].match_end, 16);

        let Json(mixed) = search_conversation_code(
            State(state),
            Path("c-code-case".to_string()),
            Query(CodeSearchQuery {
                q: "metricSource".to_string(),
                limit: Some(10),
            }),
        )
        .await
        .expect("mixed search");
        let lines: Vec<_> = mixed
            .items
            .into_iter()
            .map(|item| item.line_number)
            .collect();
        assert_eq!(lines, vec![1]);
    }

    #[tokio::test]
    async fn search_conversation_code_respects_ignore_git_dir_and_bounds() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cwd = tmp.path().join("repo");
        std::fs::create_dir_all(cwd.join("src")).expect("src dirs");
        std::fs::create_dir_all(cwd.join("ignored")).expect("ignored dirs");
        std::fs::create_dir_all(cwd.join(".git")).expect("git dirs");
        std::fs::write(cwd.join(".gitignore"), "ignored/\n").expect("gitignore");
        std::fs::write(cwd.join("src/a.rs"), "needle one\nneedle two\n").expect("a");
        std::fs::write(cwd.join("src/b.rs"), "needle three\n").expect("b");
        std::fs::write(cwd.join("ignored/hidden.rs"), "needle ignored\n").expect("ignored");
        std::fs::write(cwd.join(".git/config"), "needle git\n").expect("git");
        std::fs::write(cwd.join("src/blob.bin"), b"needle\0binary\n").expect("binary");

        let state = make_test_state().await;
        state
            .db
            .create_conversation_with_project(
                "c-code-ignore",
                "code-ignore",
                cwd.to_str().unwrap(),
                true,
                None,
                None,
                None,
                &crate::db::ConvMode::Direct,
                None,
                None,
                None,
                crate::llm_language::LlmLanguage::default(),
            )
            .await
            .expect("create");

        let Json(response) = search_conversation_code(
            State(state),
            Path("c-code-ignore".to_string()),
            Query(CodeSearchQuery {
                q: "needle".to_string(),
                limit: Some(2),
            }),
        )
        .await
        .expect("search");

        assert_eq!(response.items.len(), 2);
        assert!(response
            .items
            .iter()
            .all(|item| item.path.starts_with("src/")));
        assert!(response
            .items
            .iter()
            .all(|item| item.path != "src/blob.bin"));
    }

    #[tokio::test]
    async fn search_conversation_code_uses_worktree_path_when_present() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cwd = tmp.path().join("repo");
        let worktree = tmp.path().join("worktree");
        std::fs::create_dir_all(cwd.join("src")).expect("cwd dirs");
        std::fs::create_dir_all(worktree.join("src")).expect("worktree dirs");
        std::fs::write(cwd.join("src/wrong.rs"), "needle wrong\n").expect("cwd file");
        std::fs::write(worktree.join("src/right.rs"), "needle right\n").expect("worktree file");

        let state = make_test_state().await;
        let mode = crate::db::ConvMode::Work {
            branch_name: crate::db::NonEmptyString::new("task-26001").unwrap(),
            worktree_path: crate::db::NonEmptyString::new(worktree.to_string_lossy().to_string())
                .unwrap(),
            base_branch: crate::db::NonEmptyString::new("main").unwrap(),
            task_id: crate::db::NonEmptyString::new("26001").unwrap(),
            task_title: crate::db::NonEmptyString::new("Code Search").unwrap(),
        };
        state
            .db
            .create_conversation_with_project(
                "c-code-root",
                "code-root",
                cwd.to_str().unwrap(),
                true,
                None,
                None,
                None,
                &mode,
                None,
                None,
                None,
                crate::llm_language::LlmLanguage::default(),
            )
            .await
            .expect("create");

        let Json(response) = search_conversation_code(
            State(state),
            Path("c-code-root".to_string()),
            Query(CodeSearchQuery {
                q: "needle".to_string(),
                limit: Some(10),
            }),
        )
        .await
        .expect("search");

        let paths: Vec<_> = response.items.into_iter().map(|item| item.path).collect();
        assert_eq!(paths, vec!["src/right.rs"]);
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
        let _ = state
            .runtime
            .bash_handles()
            .get_or_create(&crate::work_scope::WorkScope::Conversation(
                "c-2".to_string(),
            ))
            .await;

        run_hard_delete_cascade(&state, "c-2")
            .await
            .expect("delete");

        assert!(
            state
                .runtime
                .bash_handles()
                .remove(&crate::work_scope::WorkScope::Conversation(
                    "c-2".to_string()
                ))
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
        let _ = state
            .runtime
            .bash_handles()
            .get_or_create(&crate::work_scope::WorkScope::Conversation(
                "c-6".to_string(),
            ))
            .await;

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
            let _ = state
                .runtime
                .bash_handles()
                .get_or_create(&crate::work_scope::WorkScope::Conversation(id.to_string()))
                .await;
        }

        for id in ids {
            run_hard_delete_cascade(&state, id).await.expect("delete");
        }

        for id in ids {
            assert!(
                state
                    .runtime
                    .bash_handles()
                    .remove(&crate::work_scope::WorkScope::Conversation(id.to_string()))
                    .await
                    .is_none(),
                "bash registry leaked entry for {id}"
            );
            assert!(state.db.get_conversation(id).await.is_err());
        }
    }

    /// Build a 2-member chain via raw SQL — same trick as the chains.rs
    /// test helper. The cascade tests only need the linkage; they don't
    /// exercise the `continue_conversation` gating on `context_exhausted`.
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

    /// Archive cascade rejects a busy conversation with the same
    /// `cancel_first` 409 as hard-delete, leaves the row unarchived,
    /// then succeeds once the conversation settles to idle.
    #[tokio::test]
    async fn archive_rejects_busy_with_409_then_succeeds() {
        let state = make_test_state().await;
        state
            .db
            .create_conversation("c-arc-1", "test", "/tmp", true, None, None)
            .await
            .expect("create");

        state
            .db
            .update_conversation_state("c-arc-1", &ConvState::LlmRequesting { attempt: 0 })
            .await
            .expect("set busy");

        let err = run_archive_cascade(&state, "c-arc-1")
            .await
            .expect_err("must reject while busy");
        match err {
            AppError::Conflict(detail) => {
                assert_eq!(detail.error_type, "cancel_first");
                assert!(detail.error.contains("Cancel"));
            }
            other => panic!("expected 409 Conflict, got {other:?}"),
        }

        let conv = state
            .db
            .get_conversation("c-arc-1")
            .await
            .expect("row still present");
        assert!(
            !conv.archived,
            "archived flag must NOT be set after refused archive"
        );

        state
            .db
            .update_conversation_state("c-arc-1", &ConvState::Idle)
            .await
            .expect("settle");

        run_archive_cascade(&state, "c-arc-1")
            .await
            .expect("archive");

        let conv = state
            .db
            .get_conversation("c-arc-1")
            .await
            .expect("row preserved");
        assert!(conv.archived, "archived flag must be set after archive");
    }

    /// Archive cascade preserves the conversation row + messages (done-but-
    /// keep-history) while running the same resource cleanup as hard-delete
    /// — verified here via the bash registry, which the cascade must drop.
    #[tokio::test]
    async fn archive_sets_flag_and_drops_bash_registry() {
        let state = make_test_state().await;
        state
            .db
            .create_conversation("c-arc-2", "test", "/tmp", true, None, None)
            .await
            .expect("create");
        let _ = state
            .runtime
            .bash_handles()
            .get_or_create(&crate::work_scope::WorkScope::Conversation(
                "c-arc-2".to_string(),
            ))
            .await;

        run_archive_cascade(&state, "c-arc-2")
            .await
            .expect("archive");

        let conv = state
            .db
            .get_conversation("c-arc-2")
            .await
            .expect("row preserved");
        assert!(conv.archived, "archived flag must be set");

        assert!(
            state
                .runtime
                .bash_handles()
                .remove(&crate::work_scope::WorkScope::Conversation(
                    "c-arc-2".to_string()
                ))
                .await
                .is_none(),
            "bash registry entry must be dropped by archive cascade"
        );
    }

    /// `archive_chain_handler` runs the cascade against every member and
    /// flips `archived = 1` on each — symmetrical with chain delete.
    #[tokio::test]
    async fn archive_chain_handler_archives_every_member() {
        let state = make_test_state().await;
        build_chain_for_test(&state, &["ca-a", "ca-b", "ca-c"]).await;
        for id in ["ca-a", "ca-b", "ca-c"] {
            let _ = state
                .runtime
                .bash_handles()
                .get_or_create(&crate::work_scope::WorkScope::Conversation(id.to_string()))
                .await;
        }

        let _ = crate::api::chains::archive_chain_handler(
            axum::extract::State(state.clone()),
            axum::extract::Path("ca-a".to_string()),
        )
        .await
        .expect("chain archive");

        for id in ["ca-a", "ca-b", "ca-c"] {
            let conv = state
                .db
                .get_conversation(id)
                .await
                .unwrap_or_else(|_| panic!("{id} row preserved"));
            assert!(conv.archived, "{id} must be archived");
            assert!(
                state
                    .runtime
                    .bash_handles()
                    .remove(&crate::work_scope::WorkScope::Conversation(id.to_string()))
                    .await
                    .is_none(),
                "bash registry leaked entry for {id}"
            );
        }
    }

    /// If any member of a chain is busy, `archive_chain_handler` refuses
    /// the whole operation up-front — no flags flipped, no cleanup.
    #[tokio::test]
    async fn archive_chain_refuses_if_any_member_busy() {
        let state = make_test_state().await;
        build_chain_for_test(&state, &["cab-a", "cab-b"]).await;
        state
            .db
            .update_conversation_state("cab-b", &ConvState::LlmRequesting { attempt: 0 })
            .await
            .expect("set busy");

        let err = crate::api::chains::archive_chain_handler(
            axum::extract::State(state.clone()),
            axum::extract::Path("cab-a".to_string()),
        )
        .await
        .expect_err("must refuse while busy");
        match err {
            AppError::Conflict(detail) => assert_eq!(detail.error_type, "cancel_first"),
            other => panic!("expected 409, got {other:?}"),
        }

        for id in ["cab-a", "cab-b"] {
            let conv = state.db.get_conversation(id).await.expect("row preserved");
            assert!(
                !conv.archived,
                "{id} must NOT be archived after refused chain archive"
            );
        }
    }

    /// Build a 3-member Work-mode chain (A -> A2 -> A3) sharing a real git
    /// worktree + branch on disk. Returns the tempdir guard (kept alive by
    /// caller), the repo path, the worktree path, and the branch name so
    /// the test can assert against the filesystem after a chain operation.
    async fn build_workmode_chain_with_shared_worktree(
        state: &AppState,
        ids: &[&str; 3],
    ) -> (
        tempfile::TempDir,
        std::path::PathBuf,
        std::path::PathBuf,
        String,
    ) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("repo dir");

        // Initial commit so `git worktree add <path> <branch>` has a
        // commit to base the new branch on.
        crate::git_ops::run_git(&repo, &["init", "--initial-branch=main"]).expect("git init");
        crate::git_ops::run_git(&repo, &["config", "user.email", "test@phoenix"])
            .expect("git config email");
        crate::git_ops::run_git(&repo, &["config", "user.name", "phoenix-test"])
            .expect("git config name");
        crate::git_ops::run_git(&repo, &["commit", "--allow-empty", "-m", "init"])
            .expect("initial commit");

        let branch = format!("task-{}", ids[0]);
        let worktree = tmp.path().join("worktree");
        crate::git_ops::run_git(
            &repo,
            &[
                "worktree",
                "add",
                "-b",
                &branch,
                worktree.to_str().unwrap(),
                "main",
            ],
        )
        .expect("worktree add");

        let project = state
            .db
            .find_or_create_project(repo.to_str().unwrap())
            .await
            .expect("project");
        let mode = crate::db::ConvMode::Work {
            branch_name: crate::db::NonEmptyString::new(branch.clone()).unwrap(),
            worktree_path: crate::db::NonEmptyString::new(worktree.to_string_lossy().to_string())
                .unwrap(),
            base_branch: crate::db::NonEmptyString::new("main").unwrap(),
            task_id: crate::db::NonEmptyString::new("00001").unwrap(),
            task_title: crate::db::NonEmptyString::new("test chain").unwrap(),
        };

        for id in ids {
            state
                .db
                .create_conversation_with_project(
                    id,
                    &format!("slug-{id}"),
                    worktree.to_str().unwrap(),
                    true,
                    None,
                    None,
                    Some(&project.id),
                    &mode,
                    None,
                    None,
                    None,
                    crate::llm_language::LlmLanguage::default(),
                )
                .await
                .expect("create conv");
        }
        for pair in ids.windows(2) {
            sqlx::query("UPDATE conversations SET continued_in_conv_id = ?1 WHERE id = ?2")
                .bind(pair[1])
                .bind(pair[0])
                .execute(state.db.pool())
                .await
                .expect("link");
        }

        (tmp, repo, worktree, branch)
    }

    /// Chain archive must tear down the chain's shared worktree + branch
    /// exactly once (from the leaf's cascade) and flip `archived = 1` on
    /// every member. Verifies the correct-by-construction continuation
    /// preservation: root + mid skip worktree cleanup, leaf actually
    /// removes it. End state: shared resources gone, all rows archived.
    #[tokio::test]
    async fn archive_chain_cleans_shared_worktree_and_branch_once() {
        let state = make_test_state().await;
        let ids = ["sc-a", "sc-a2", "sc-a3"];
        let (_tmp, repo, worktree, branch) =
            build_workmode_chain_with_shared_worktree(&state, &ids).await;

        assert!(worktree.exists(), "precondition: worktree must exist");
        assert!(
            crate::git_ops::run_git(&repo, &["rev-parse", "--verify", &branch]).is_ok(),
            "precondition: branch must exist"
        );

        let _ = crate::api::chains::archive_chain_handler(
            axum::extract::State(state.clone()),
            axum::extract::Path("sc-a".to_string()),
        )
        .await
        .expect("chain archive");

        assert!(
            !worktree.exists(),
            "shared worktree must be removed after chain archive"
        );
        assert!(
            crate::git_ops::run_git(&repo, &["rev-parse", "--verify", &branch]).is_err(),
            "shared task branch must be deleted after chain archive (Work mode)"
        );
        for id in ids {
            let conv = state
                .db
                .get_conversation(id)
                .await
                .unwrap_or_else(|_| panic!("{id} row preserved"));
            assert!(conv.archived, "{id} must be archived");
        }
    }

    /// Chain hard-delete must tear down the chain's shared worktree +
    /// branch exactly once and remove every row. Root-first iteration
    /// (FK on `continued_in_conv_id` requires it) plus the in-cascade
    /// continuation-preservation check together ensure the worktree is
    /// only touched at the leaf -- no race where root's cascade pulls
    /// the worktree out from under the leaf's row before the leaf row
    /// is even deleted.
    #[tokio::test]
    async fn delete_chain_cleans_shared_worktree_and_branch_once() {
        let state = make_test_state().await;
        let ids = ["dc-a", "dc-a2", "dc-a3"];
        let (_tmp, repo, worktree, branch) =
            build_workmode_chain_with_shared_worktree(&state, &ids).await;

        assert!(worktree.exists(), "precondition: worktree must exist");

        let _ = crate::api::chains::delete_chain_handler(
            axum::extract::State(state.clone()),
            axum::extract::Path("dc-a".to_string()),
        )
        .await
        .expect("chain delete");

        assert!(
            !worktree.exists(),
            "shared worktree must be removed after chain delete"
        );
        assert!(
            crate::git_ops::run_git(&repo, &["rev-parse", "--verify", &branch]).is_err(),
            "shared task branch must be deleted after chain delete (Work mode)"
        );
        for id in ids {
            assert!(
                state.db.get_conversation(id).await.is_err(),
                "{id} row must be gone after chain delete"
            );
        }
    }

    /// Per-conversation cascade on a chain root must NOT touch the worktree
    /// -- it belongs to the leaf. Pure invariant test for the new
    /// `continued_in_conv_id` guard in `cascade_projects_on_delete`. Drives
    /// the cascade directly (bypassing the chain-member API gate) so the
    /// preservation logic is exercised in isolation.
    #[tokio::test]
    async fn cascade_skips_worktree_when_continuation_exists() {
        let state = make_test_state().await;
        let ids = ["pc-a", "pc-a2"];
        let (_tmp, _repo, worktree, _branch) =
            build_workmode_chain_with_shared_worktree(&state, &[ids[0], ids[1], "pc-a3"]).await;

        let root_conv = state.db.get_conversation("pc-a").await.expect("root");
        let report = cascade_projects_on_delete(&state, &root_conv, None).await;
        assert!(
            report.worktree_path.is_none(),
            "cascade on chain root must report no worktree work (continuation owns it), got {report:?}"
        );
        assert!(
            report.branch_name.is_none(),
            "cascade on chain root must not name branch for deletion"
        );
        assert!(
            report.error.is_none(),
            "skip path must not surface an error"
        );
        assert!(
            worktree.exists(),
            "worktree must remain intact after non-leaf cascade"
        );
    }

    /// Create a single Work-mode conversation bound to `worktree`, so it
    /// resolves to `WorkScope::Worktree(worktree)`. No continuation, no
    /// parent — the caller wires those up as the scenario needs. Returns
    /// once the row exists.
    async fn create_workmode_conv_on_worktree(
        state: &AppState,
        id: &str,
        worktree: &std::path::Path,
        branch: &str,
        project_id: &str,
        parent_conversation_id: Option<&str>,
    ) {
        let mode = crate::db::ConvMode::Work {
            branch_name: crate::db::NonEmptyString::new(branch.to_string()).unwrap(),
            worktree_path: crate::db::NonEmptyString::new(worktree.to_string_lossy().to_string())
                .unwrap(),
            base_branch: crate::db::NonEmptyString::new("main").unwrap(),
            task_id: crate::db::NonEmptyString::new("00001").unwrap(),
            task_title: crate::db::NonEmptyString::new("scope-sharing test").unwrap(),
        };
        state
            .db
            .create_conversation_with_project(
                id,
                &format!("slug-{id}"),
                worktree.to_str().unwrap(),
                true,
                parent_conversation_id,
                None,
                Some(project_id),
                &mode,
                None,
                None,
                None,
                crate::llm_language::LlmLanguage::default(),
            )
            .await
            .expect("create conv");
    }

    /// Seed a live bash handle on `scope` so cascade teardown vs preservation
    /// is observable: after a teardown `registry.remove(scope)` returns
    /// `None` (the cascade consumed the table); after preservation it
    /// returns `Some` (the table is left in place).
    async fn seed_live_bash_handle(state: &AppState, scope: &crate::work_scope::WorkScope) {
        use phoenix_tools::bash::handle::{Handle, HandleId};
        use phoenix_tools::bash::ring::RING_BUFFER_BYTES;

        let table = state.runtime.bash_handles().get_or_create(scope).await;
        let handle = Handle::new_live(
            scope.clone(),
            HandleId::new("b-parent"),
            "npm run dev".into(),
            Some("dev".into()),
            4321,
            1234,
            RING_BUFFER_BYTES,
        );
        table.write().await.insert(handle);
    }

    /// REQ-BASH-WS-002: deleting a Work-mode sub-agent that shares its
    /// parent's `WorkScope` (no continuation of its own) must PRESERVE the
    /// still-live parent's bash handles. The parent has a live runtime
    /// handle and is non-terminal, so it is a live owner of the scope; the
    /// any-live-owner preservation signal keeps the handle table intact.
    ///
    /// Regression guard for the P1 where the continuation-only signal
    /// resolved `inheritor_scope = None` for a sub-agent and SIGKILL'd the
    /// parent's b-* handles.
    #[tokio::test]
    async fn delete_subagent_sharing_parent_scope_preserves_parent_bash() {
        let state = make_test_state().await;

        let tmp = tempfile::tempdir().expect("tempdir");
        let worktree = tmp.path().join("worktree");
        std::fs::create_dir_all(&worktree).expect("worktree dir");
        let project = state
            .db
            .find_or_create_project(worktree.to_str().unwrap())
            .await
            .expect("project");

        // Parent + sub-agent, both Work-mode on the same worktree → same
        // WorkScope. Neither has a continuation; the sub-agent points at
        // the parent via parent_conversation_id.
        create_workmode_conv_on_worktree(
            &state,
            "sa-parent",
            &worktree,
            "task-sa",
            &project.id,
            None,
        )
        .await;
        create_workmode_conv_on_worktree(
            &state,
            "sa-child",
            &worktree,
            "task-sa",
            &project.id,
            Some("sa-parent"),
        )
        .await;

        let scope = crate::work_scope::WorkScope::Worktree(worktree.to_string_lossy().into_owned());
        seed_live_bash_handle(&state, &scope).await;

        // Register a live runtime handle for the parent so it counts as a
        // live owner of the scope during the sub-agent's cascade.
        let _parent_rx = state
            .runtime
            .subscribe("sa-parent")
            .await
            .expect("subscribe");

        // Delete the sub-agent. Its cascade resolves the shared scope; the
        // parent is a live sibling, so the scope is still owned → preserve.
        run_hard_delete_cascade(&state, "sa-child")
            .await
            .expect("delete sub-agent");

        assert!(
            state.runtime.bash_handles().remove(&scope).await.is_some(),
            "parent's bash handle table must survive the sub-agent's deletion \
             (scope still owned by the live parent)"
        );
        assert!(
            state.db.get_conversation("sa-child").await.is_err(),
            "sub-agent row must be gone"
        );
        assert!(
            state.db.get_conversation("sa-parent").await.is_ok(),
            "parent row must remain"
        );
    }

    /// Counterpart: when the conversation being deleted is the LAST live
    /// conversation on its scope (no continuation, no live sibling), the
    /// cascade must tear the scope down — the deleted conv reads
    /// non-terminal in the DB at cascade time, so excluding it from the
    /// live-owner enumeration is what lets teardown fire.
    #[tokio::test]
    async fn delete_last_live_conv_on_scope_tears_down_bash() {
        let state = make_test_state().await;

        let tmp = tempfile::tempdir().expect("tempdir");
        let worktree = tmp.path().join("worktree");
        std::fs::create_dir_all(&worktree).expect("worktree dir");
        let project = state
            .db
            .find_or_create_project(worktree.to_str().unwrap())
            .await
            .expect("project");

        create_workmode_conv_on_worktree(&state, "solo", &worktree, "task-solo", &project.id, None)
            .await;

        let scope = crate::work_scope::WorkScope::Worktree(worktree.to_string_lossy().into_owned());
        seed_live_bash_handle(&state, &scope).await;

        // The conversation being deleted is the only live owner. It still
        // reads non-terminal in the DB at cascade time, but it is excluded
        // from the enumeration, so the scope is NOT still owned → tear down.
        run_hard_delete_cascade(&state, "solo")
            .await
            .expect("delete");

        assert!(
            state.runtime.bash_handles().remove(&scope).await.is_none(),
            "scope must be torn down when its last live conversation is deleted"
        );
    }

    /// Build a real git repo + shared worktree with a parent + sub-agent
    /// pair of Work-mode conversations bound to it. Both resolve to
    /// `WorkScope::Worktree(worktree)`; the sub-agent points at the parent
    /// via `parent_conversation_id` and inherits the same `worktree_path`.
    /// Returns the temp dir (kept alive by the caller), repo root, worktree
    /// path, and branch name.
    async fn build_parent_subagent_on_shared_worktree(
        state: &AppState,
        parent_id: &str,
        child_id: &str,
    ) -> (
        tempfile::TempDir,
        std::path::PathBuf,
        std::path::PathBuf,
        String,
    ) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("repo dir");

        crate::git_ops::run_git(&repo, &["init", "--initial-branch=main"]).expect("git init");
        crate::git_ops::run_git(&repo, &["config", "user.email", "test@phoenix"])
            .expect("git config email");
        crate::git_ops::run_git(&repo, &["config", "user.name", "phoenix-test"])
            .expect("git config name");
        crate::git_ops::run_git(&repo, &["commit", "--allow-empty", "-m", "init"])
            .expect("initial commit");

        let branch = format!("task-{parent_id}");
        let worktree = tmp.path().join("worktree");
        crate::git_ops::run_git(
            &repo,
            &[
                "worktree",
                "add",
                "-b",
                &branch,
                worktree.to_str().unwrap(),
                "main",
            ],
        )
        .expect("worktree add");

        let project = state
            .db
            .find_or_create_project(repo.to_str().unwrap())
            .await
            .expect("project");

        create_workmode_conv_on_worktree(state, parent_id, &worktree, &branch, &project.id, None)
            .await;
        create_workmode_conv_on_worktree(
            state,
            child_id,
            &worktree,
            &branch,
            &project.id,
            Some(parent_id),
        )
        .await;

        (tmp, repo, worktree, branch)
    }

    /// REQ-PROJ-029: deleting a Work-mode sub-agent that shares its live
    /// parent's worktree must PRESERVE the worktree on disk and the task
    /// branch. The sub-agent inherits the parent's `worktree_path`, so the
    /// project cascade would `git worktree remove --force` + `branch -D`
    /// the parent's still-in-use checkout — destructive data loss against a
    /// live conversation. The any-live-owner signal must suppress it.
    #[tokio::test]
    async fn delete_subagent_sharing_parent_scope_preserves_worktree_and_branch() {
        let state = make_test_state().await;
        let (_tmp, repo, worktree, branch) =
            build_parent_subagent_on_shared_worktree(&state, "wp-parent", "wp-child").await;

        assert!(worktree.exists(), "precondition: worktree must exist");
        assert!(
            crate::git_ops::run_git(&repo, &["rev-parse", "--verify", &branch]).is_ok(),
            "precondition: branch must exist"
        );

        // Register a live runtime handle for the parent so it counts as a
        // live owner of the scope during the sub-agent's cascade.
        let _parent_rx = state
            .runtime
            .subscribe("wp-parent")
            .await
            .expect("subscribe");

        run_hard_delete_cascade(&state, "wp-child")
            .await
            .expect("delete sub-agent");

        assert!(
            worktree.exists(),
            "REQ-PROJ-029: parent's worktree must survive the sub-agent's deletion \
             (scope still owned by the live parent)"
        );
        assert!(
            crate::git_ops::run_git(&repo, &["rev-parse", "--verify", &branch]).is_ok(),
            "parent's task branch must survive the sub-agent's deletion"
        );
        assert!(
            state.db.get_conversation("wp-child").await.is_err(),
            "sub-agent row must be gone"
        );
        assert!(
            state.db.get_conversation("wp-parent").await.is_ok(),
            "parent row must remain"
        );
    }

    /// Counterpart: deleting the LAST live owner of the worktree scope (the
    /// parent here, with no live sibling) must still reap the worktree and
    /// the task branch as a normal solo Work conversation would.
    #[tokio::test]
    async fn delete_last_owner_still_removes_worktree_and_branch() {
        let state = make_test_state().await;
        let (_tmp, repo, worktree, branch) =
            build_parent_subagent_on_shared_worktree(&state, "wl-parent", "wl-child").await;

        // Keep the parent live (registered runtime handle) while the
        // sub-agent is deleted, so the sub-agent's cascade preserves the
        // shared worktree.
        let parent_rx = state
            .runtime
            .subscribe("wl-parent")
            .await
            .expect("subscribe");
        run_hard_delete_cascade(&state, "wl-child")
            .await
            .expect("delete sub-agent");

        assert!(
            worktree.exists(),
            "worktree must still exist after sub-agent delete (parent owns it)"
        );

        // Drop the parent's runtime handle so it is no longer a live owner:
        // evicting removes it from the `runtimes` map that
        // `scope_has_live_conversation_excluding` enumerates. The parent is
        // now the sole, last owner and its cascade tears down.
        drop(parent_rx);
        state
            .runtime
            .evict_runtime("wl-parent", crate::runtime::EvictionReason::ModelUpgrade)
            .await;

        run_hard_delete_cascade(&state, "wl-parent")
            .await
            .expect("delete parent");

        assert!(
            !worktree.exists(),
            "worktree must be removed when its last owner is deleted"
        );
        assert!(
            crate::git_ops::run_git(&repo, &["rev-parse", "--verify", &branch]).is_err(),
            "task branch must be deleted when the last owner is deleted (Work mode)"
        );
    }

    /// The cleanup cascade fails loud when the sibling-liveness lookup cannot
    /// reach the DB. Proceeding on a fail-closed "assume live" would skip every
    /// resource + worktree teardown while the row is still archived/deleted,
    /// orphaning those resources with no retry — so the cascade refuses and
    /// surfaces the error to the caller, who can retry once the DB is healthy.
    /// The early return precedes all side effects, so refusal leaves no partial
    /// state.
    #[tokio::test]
    async fn cleanup_cascade_fails_loud_when_sibling_liveness_lookup_errors() {
        let state = make_test_state().await;
        let worktree = "/repo/.phoenix/worktrees/cascade-err";
        let mode = crate::db::ConvMode::Work {
            branch_name: crate::db::NonEmptyString::new("task-branch").unwrap(),
            worktree_path: crate::db::NonEmptyString::new(worktree.to_string()).unwrap(),
            base_branch: crate::db::NonEmptyString::new("main").unwrap(),
            task_id: crate::db::NonEmptyString::new("T1").unwrap(),
            task_title: crate::db::NonEmptyString::new("title").unwrap(),
        };
        state
            .db
            .create_conversation_with_project(
                "leaf",
                "leaf",
                worktree,
                false,
                None,
                None,
                None,
                &mode,
                None,
                None,
                None,
                crate::llm_language::LlmLanguage::default(),
            )
            .await
            .expect("create");

        // Capture the conversation before disabling the DB; the cascade takes
        // the row by value and never re-reads it.
        let conv = state.db.get_conversation("leaf").await.expect("get conv");

        // Fault injection: closing the pool makes the WorkScope::Worktree
        // sibling-liveness query (`list_conversations_for_worktree`) return a
        // non-NotFound DbError — the exact transient-failure shape the fix
        // must propagate rather than swallow to "assume live".
        state.db.pool().close().await;

        let result = run_resource_cleanup_cascade(&state, &conv).await;

        assert!(
            matches!(result, Err(AppError::Internal(_))),
            "an unreadable DB during the sibling-liveness lookup must fail the \
             cascade, not silently preserve-and-archive; got {result:?}"
        );
    }

    /// F5: `run_hard_delete_cascade` must dismiss + clean pending fork proposals
    /// BEFORE the long resource-cleanup teardown opens its window, so a concurrent
    /// approve racing the cascade finds the proposal non-pending and aborts.
    ///
    /// The origin is a Direct-mode conversation (no worktree of its own), so the
    /// ONLY thing that removes the proposal's deterministic orphan worktree is the
    /// fork-cleanup step. Asserting that orphan is gone after the cascade proves
    /// the fork cleanup ran as part of the delete — and because resource cleanup
    /// never touches a fork orphan path, its removal is attributable solely to the
    /// fork-cleanup step the cascade now runs first.
    #[tokio::test]
    async fn hard_delete_dismisses_pending_fork_proposal_and_cleans_orphan() {
        use crate::db::{ForkProposal, ForkProposalStatus};
        use crate::runtime::fork_resolve::{derive_conv_id, ResolutionKind};

        let state = make_test_state().await;
        // Start the single fork-resolution consumer so the cascade's
        // `cleanup_pending_fork_orphans_on_delete` routes through it.
        state.runtime.start_sub_agent_handler().await;

        // Real repo so the deterministic orphan worktree can be created + cleaned.
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("repo dir");
        crate::git_ops::run_git(&repo, &["init", "--initial-branch=main"]).expect("git init");
        crate::git_ops::run_git(&repo, &["config", "user.email", "t@phoenix"]).expect("email");
        crate::git_ops::run_git(&repo, &["config", "user.name", "t"]).expect("name");
        crate::git_ops::run_git(&repo, &["commit", "--allow-empty", "-m", "init"]).expect("commit");

        let project = state
            .db
            .find_or_create_project(repo.to_str().unwrap())
            .await
            .expect("project");
        let origin = "fd-origin";
        state
            .db
            .create_conversation_with_project(
                origin,
                "fd-origin",
                repo.to_str().unwrap(),
                true,
                None,
                None,
                Some(&project.id),
                &crate::db::ConvMode::Direct,
                None,
                None,
                None,
                crate::llm_language::LlmLanguage::default(),
            )
            .await
            .expect("create origin");

        // Pending proposal with a crashed-approve deterministic orphan worktree.
        let pid = uuid::Uuid::new_v4().to_string();
        state
            .db
            .insert_fork_proposal(&ForkProposal {
                id: pid.clone(),
                origin_conversation_id: origin.to_string(),
                task_file: "tasks/12345-p1-ready--x.md".to_string(),
                title: "x".to_string(),
                priority: "p1".to_string(),
                body: "# x\n".to_string(),
                status: ForkProposalStatus::Pending,
                fork_conversation_id: None,
                refinement_conversation_id: None,
                created_at: chrono::Utc::now(),
                resolved_at: None,
            })
            .await
            .expect("insert proposal");

        let orphan_id = derive_conv_id(&pid, ResolutionKind::Spawn);
        let orphan_wt = repo.join(".phoenix/worktrees").join(&orphan_id);
        std::fs::create_dir_all(orphan_wt.parent().unwrap()).unwrap();
        crate::git_ops::run_git(
            &repo,
            &[
                "worktree",
                "add",
                "-b",
                "task-12345-x",
                orphan_wt.to_str().unwrap(),
                "main",
            ],
        )
        .expect("orphan worktree");
        assert!(orphan_wt.is_dir(), "precondition: orphan worktree exists");

        run_hard_delete_cascade(&state, origin)
            .await
            .expect("hard delete");

        // The origin row is gone (ON DELETE CASCADE removed the proposal row too)
        // and the deterministic orphan worktree was cleaned by the fork step that
        // the cascade ran BEFORE resource cleanup.
        assert!(
            state.db.get_conversation(origin).await.is_err(),
            "origin row must be deleted"
        );
        assert!(
            !orphan_wt.exists(),
            "pending proposal's deterministic orphan must be cleaned by the fork step"
        );
    }
}

#[cfg(test)]
mod regenerate_conversation_name_tests {
    use super::*;
    use crate::chain_qa::ChainQa;
    use crate::db::Database;
    use crate::platform::PlatformCapability;
    use crate::runtime::RuntimeManager;
    use crate::tools::mcp::McpClientManager;
    use async_trait::async_trait;
    use phoenix_core::domain::db_schema::{MessageContent, UserContent};
    use phoenix_llm::{
        ContentBlock, LlmError, LlmRequest, LlmResponse, LlmService, ModelRegistry, Usage,
    };
    use std::sync::Arc;

    #[derive(Debug)]
    enum StubLlm {
        Ok(&'static str),
        Err,
    }

    #[async_trait]
    impl LlmService for StubLlm {
        async fn complete(&self, _r: &LlmRequest) -> Result<LlmResponse, LlmError> {
            match self {
                StubLlm::Ok(text) => Ok(LlmResponse {
                    content: vec![ContentBlock::text(*text)],
                    end_turn: true,
                    usage: Usage::default(),
                }),
                StubLlm::Err => Err(LlmError::server_error("temporary outage")),
            }
        }

        async fn complete_streaming(
            &self,
            r: &LlmRequest,
            _: &tokio::sync::mpsc::Sender<phoenix_llm::TokenChunk>,
        ) -> Result<LlmResponse, LlmError> {
            self.complete(r).await
        }

        #[allow(clippy::unnecessary_literal_bound)]
        fn model_id(&self) -> &str {
            "claude-sonnet-5"
        }
    }

    async fn make_test_state(llm_registry: Arc<ModelRegistry>) -> AppState {
        let db = Database::open_in_memory().await.expect("open db");
        let platform = PlatformCapability::None {
            details: "test".into(),
        };
        let mcp_manager = Arc::new(McpClientManager::new());
        let runtime = Arc::new(RuntimeManager::new(
            db.clone(),
            llm_registry.clone(),
            platform.clone(),
            mcp_manager.clone(),
            None,
        ));
        let terminals = runtime.terminals.clone();
        let message_retriever: std::sync::Arc<dyn crate::db::MessageRetriever> =
            std::sync::Arc::new(crate::db::Fts5Retriever::new(db.pool().clone()));
        let chain_qa = ChainQa::new(db.clone(), llm_registry.clone(), message_retriever.clone());
        let sessions = super::super::auth::SessionStore::new(db.clone(), String::new());
        AppState {
            runtime,
            llm_registry,
            db,
            platform,
            mcp_manager,
            credential_helper: None,
            password: None,
            sessions,
            login_throttle: super::super::auth::LoginThrottle::new(),
            terminals,
            chain_qa,
            message_retriever,
            codex_login: super::super::codex_login::CodexLoginManager::new(),
            deployment: Arc::new(super::super::deployment::DeploymentConfig::for_tests()),
            runtime_env: Arc::new(phoenix_core::runtime_env::PhoenixRuntimeEnvironment::detect()),
            suggest_token: String::new(),
            discovery: crate::discovery::start(crate::discovery::DiscoveryConfig {
                enabled: false,
                ..crate::discovery::DiscoveryConfig::from_env()
            }),
        }
    }

    async fn seed_conversation(state: &AppState, id: &str, slug: &str) {
        state
            .db
            .create_conversation(id, slug, "/tmp", true, None, None)
            .await
            .expect("create conversation");
    }

    async fn seed_opening(state: &AppState, conv_id: &str, text: &str) {
        state
            .db
            .add_message(
                &format!("msg-{conv_id}"),
                conv_id,
                &MessageContent::User(UserContent::new(text)),
                None,
                None,
            )
            .await
            .expect("add opening message");
    }

    async fn regenerate(
        state: &AppState,
        id: &str,
    ) -> Result<Json<ConversationResponse>, AppError> {
        regenerate_conversation_name(State(state.clone()), Path(id.to_string())).await
    }

    #[tokio::test]
    async fn successful_generation_renames_with_existing_slug_rules() {
        let state = make_test_state(Arc::new(ModelRegistry::for_test_with_sonnet(Arc::new(
            StubLlm::Ok("Useful Auth Fix"),
        ))))
        .await;
        seed_conversation(&state, "conv-ok", "deterministic-conv-ok").await;
        seed_opening(
            &state,
            "conv-ok",
            "fix authentication retry after quota errors",
        )
        .await;

        let Json(response) = regenerate(&state, "conv-ok").await.expect("regenerate");
        assert_eq!(response.conversation["slug"], "useful-auth-fix");
        let reloaded = state.db.get_conversation("conv-ok").await.expect("reload");
        assert_eq!(reloaded.slug.as_deref(), Some("useful-auth-fix"));
    }

    #[tokio::test]
    async fn missing_opening_leaves_slug_unchanged() {
        let state = make_test_state(Arc::new(ModelRegistry::for_test_with_sonnet(Arc::new(
            StubLlm::Ok("Useful Name"),
        ))))
        .await;
        seed_conversation(&state, "conv-empty", "deterministic-conv-empty").await;

        assert!(regenerate(&state, "conv-empty").await.is_err());
        let reloaded = state
            .db
            .get_conversation("conv-empty")
            .await
            .expect("reload");
        assert_eq!(reloaded.slug.as_deref(), Some("deterministic-conv-empty"));
    }

    #[tokio::test]
    async fn llm_failure_leaves_slug_unchanged() {
        let state = make_test_state(Arc::new(ModelRegistry::for_test_with_sonnet(Arc::new(
            StubLlm::Err,
        ))))
        .await;
        seed_conversation(&state, "conv-fail", "deterministic-conv-fail").await;
        seed_opening(&state, "conv-fail", "rename this later").await;

        assert!(regenerate(&state, "conv-fail").await.is_err());
        let reloaded = state
            .db
            .get_conversation("conv-fail")
            .await
            .expect("reload");
        assert_eq!(reloaded.slug.as_deref(), Some("deterministic-conv-fail"));
    }

    #[tokio::test]
    async fn duplicate_generated_slug_returns_error_and_leaves_slug_unchanged() {
        let state = make_test_state(Arc::new(ModelRegistry::for_test_with_sonnet(Arc::new(
            StubLlm::Ok("Taken Slug"),
        ))))
        .await;
        seed_conversation(&state, "conv-taken", "taken-slug").await;
        seed_conversation(&state, "conv-rename", "deterministic-conv-rename").await;
        seed_opening(&state, "conv-rename", "rename this later").await;

        assert!(regenerate(&state, "conv-rename").await.is_err());
        let reloaded = state
            .db
            .get_conversation("conv-rename")
            .await
            .expect("reload");
        assert_eq!(reloaded.slug.as_deref(), Some("deterministic-conv-rename"));
    }

    #[tokio::test]
    async fn missing_cheap_model_leaves_slug_unchanged() {
        let state = make_test_state(Arc::new(ModelRegistry::new_empty())).await;
        seed_conversation(&state, "conv-nomodel", "deterministic-conv-nomodel").await;
        seed_opening(&state, "conv-nomodel", "rename this later").await;

        assert!(regenerate(&state, "conv-nomodel").await.is_err());
        let reloaded = state
            .db
            .get_conversation("conv-nomodel")
            .await
            .expect("reload");
        assert_eq!(reloaded.slug.as_deref(), Some("deterministic-conv-nomodel"));
    }
}

/// Task 02713: `upgrade_conversation_model` must accept the change from
/// `Idle` and `Error`, and reject it while an operation is in flight.
/// Exercises the real axum handler end to end against an in-memory DB.
#[cfg(test)]
mod upgrade_model_state_guard_tests {
    use super::*;
    use crate::chain_qa::ChainQa;
    use crate::db::Database;
    use crate::platform::PlatformCapability;
    use crate::runtime::RuntimeManager;
    use crate::state_machine::ConvState;
    use crate::tools::mcp::McpClientManager;
    use async_trait::async_trait;
    use phoenix_llm::{
        ContentBlock, LlmError, LlmRequest, LlmResponse, LlmService, ModelRegistry, Usage,
    };
    use std::sync::Arc;

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
            _: &tokio::sync::mpsc::Sender<phoenix_llm::TokenChunk>,
        ) -> Result<LlmResponse, LlmError> {
            self.complete(r).await
        }
        #[allow(clippy::unnecessary_literal_bound)]
        fn model_id(&self) -> &str {
            "claude-sonnet-5"
        }
    }

    async fn make_test_state() -> AppState {
        let db = Database::open_in_memory().await.expect("open db");
        let llm_registry = Arc::new(ModelRegistry::for_test_with_sonnet(Arc::new(StubLlm)));
        let platform = PlatformCapability::None {
            details: "test".into(),
        };
        let mcp_manager = Arc::new(McpClientManager::new());
        let runtime = Arc::new(RuntimeManager::new(
            db.clone(),
            llm_registry.clone(),
            platform.clone(),
            mcp_manager.clone(),
            None,
        ));
        let terminals = runtime.terminals.clone();
        let message_retriever: std::sync::Arc<dyn crate::db::MessageRetriever> =
            std::sync::Arc::new(crate::db::Fts5Retriever::new(db.pool().clone()));
        let chain_qa = ChainQa::new(db.clone(), llm_registry.clone(), message_retriever.clone());
        let sessions = super::super::auth::SessionStore::new(db.clone(), String::new());
        AppState {
            runtime,
            llm_registry,
            db,
            platform,
            mcp_manager,
            credential_helper: None,
            password: None,
            sessions,
            login_throttle: super::super::auth::LoginThrottle::new(),
            terminals,
            chain_qa,
            message_retriever,
            codex_login: super::super::codex_login::CodexLoginManager::new(),
            deployment: Arc::new(super::super::deployment::DeploymentConfig::for_tests()),
            runtime_env: Arc::new(phoenix_core::runtime_env::PhoenixRuntimeEnvironment::detect()),
            suggest_token: String::new(),
            discovery: crate::discovery::start(crate::discovery::DiscoveryConfig {
                enabled: false,
                ..crate::discovery::DiscoveryConfig::from_env()
            }),
        }
    }

    async fn upgrade(state: &AppState, id: &str, model: &str) -> Result<(), AppError> {
        upgrade_conversation_model(
            State(state.clone()),
            Path(id.to_string()),
            Json(UpgradeModelRequest {
                model: model.to_string(),
            }),
        )
        .await
        .map(|_| ())
    }

    async fn seed(state: &AppState, id: &str) {
        state
            .db
            .create_conversation(id, "test", "/tmp", true, None, None)
            .await
            .expect("create");
        // Start from a non-default model so a successful switch is observable.
        state
            .db
            .update_conversation_model(id, "claude-opus-4-7")
            .await
            .expect("seed model");
    }

    #[tokio::test]
    async fn allows_switch_from_error_and_persists() {
        let state = make_test_state().await;
        seed(&state, "c-err").await;
        state
            .db
            .update_conversation_state(
                "c-err",
                &ConvState::Error {
                    message: "overloaded".into(),
                    error_kind: crate::db::ErrorKind::ServerOverloaded,
                    resets_at: None,
                },
            )
            .await
            .expect("set error");

        upgrade(&state, "c-err", "claude-sonnet-5")
            .await
            .expect("model switch must be allowed from Error");

        let conv = state.db.get_conversation("c-err").await.expect("reload");
        assert_eq!(
            conv.model.as_deref(),
            Some("claude-sonnet-5"),
            "new model must be persisted so the next retry picks it up"
        );
    }

    #[tokio::test]
    async fn allows_switch_from_idle() {
        let state = make_test_state().await;
        seed(&state, "c-idle").await;
        // create_conversation leaves the row Idle by default.
        upgrade(&state, "c-idle", "claude-sonnet-5")
            .await
            .expect("model switch must be allowed from Idle");
        let conv = state.db.get_conversation("c-idle").await.expect("reload");
        assert_eq!(conv.model.as_deref(), Some("claude-sonnet-5"));
    }

    #[tokio::test]
    async fn rejects_switch_while_llm_request_in_flight() {
        let state = make_test_state().await;
        seed(&state, "c-busy").await;
        state
            .db
            .update_conversation_state("c-busy", &ConvState::LlmRequesting { attempt: 1 })
            .await
            .expect("set busy");

        let err = upgrade(&state, "c-busy", "claude-sonnet-5")
            .await
            .expect_err("must reject while an LLM request is in flight");
        match err {
            AppError::BadRequest(msg) => assert!(
                msg.contains("LlmRequesting"),
                "error should name the blocking state, got: {msg}"
            ),
            other => panic!("expected 400 BadRequest, got {other:?}"),
        }

        // Model must be unchanged.
        let conv = state.db.get_conversation("c-busy").await.expect("reload");
        assert_eq!(conv.model.as_deref(), Some("claude-opus-4-7"));
    }
}

#[cfg(test)]
mod mkdir_confinement_tests {
    use super::mkdir_target_is_confined;
    use std::fs;

    #[test]
    fn admits_a_new_subdir_under_home() {
        let home = tempfile::tempdir().unwrap();
        let target = home.path().join("new").join("nested");
        assert!(mkdir_target_is_confined(&target, home.path()));
    }

    #[test]
    fn admits_a_new_subdir_under_tmp() {
        // `/tmp` is the second hard-coded allowed root. Use an unrelated home so
        // only the `/tmp` branch can admit it.
        let home = tempfile::tempdir().unwrap();
        let target =
            std::path::Path::new("/tmp").join(format!("phoenix-mkdir-test-{}", std::process::id()));
        assert!(mkdir_target_is_confined(&target, home.path()));
        let _ = fs::remove_dir_all(&target);
    }

    #[test]
    fn rejects_dotdot_traversal_out_of_tmp() {
        // The bypass: `/tmp/../etc/...` string-starts-with `/tmp/` but escapes
        // once the OS resolves `..`. The `..`-component rejection closes it.
        let home = tempfile::tempdir().unwrap();
        let target = std::path::Path::new("/tmp/../etc/phoenix-evil");
        assert!(!mkdir_target_is_confined(target, home.path()));
    }

    #[test]
    fn rejects_dotdot_traversal_out_of_home() {
        let home = tempfile::tempdir().unwrap();
        let target = home.path().join("..").join("escapee");
        assert!(!mkdir_target_is_confined(&target, home.path()));
    }

    /// A unique base directory guaranteed NOT under `/tmp` — which is itself an
    /// always-allowed `mkdir` root, so a fixture placed there would be admitted
    /// by the `/tmp` branch regardless of the home check. Anchored under the
    /// crate dir; the returned guard removes it on drop.
    struct NonTmpBase(std::path::PathBuf);
    impl NonTmpBase {
        fn new(tag: &str) -> Self {
            let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("target")
                .join(format!("mkdir-confine-{}-{}", tag, std::process::id()));
            let _ = fs::remove_dir_all(&base);
            fs::create_dir_all(&base).unwrap();
            Self(base)
        }
        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }
    impl Drop for NonTmpBase {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn rejects_sibling_prefix_of_home() {
        // `<home>evil` shares a string prefix with `<home>` but is a different
        // directory; component-wise `starts_with` must reject it.
        let base = NonTmpBase::new("sibling");
        let home = base.path().join("user");
        fs::create_dir(&home).unwrap();
        let sibling = base.path().join("userevil");
        fs::create_dir(&sibling).unwrap();
        assert!(!mkdir_target_is_confined(&sibling.join("x"), &home));
    }

    #[test]
    fn rejects_path_entirely_outside_roots() {
        let home = tempfile::tempdir().unwrap();
        assert!(!mkdir_target_is_confined(
            std::path::Path::new("/etc/phoenix-evil"),
            home.path()
        ));
    }

    #[test]
    fn user_ref_rejects_flaglike_branch() {
        // A `-`-prefixed ref could be misparsed as a git CLI option; reject it at
        // the boundary. Legitimate branch names never begin with `-`.
        assert!(super::validate_user_ref("--upload-pack=touch /tmp/pwned").is_err());
        assert!(super::validate_user_ref("-x").is_err());
        assert!(super::validate_user_ref("feature/login").is_ok());
        assert!(super::validate_user_ref("main").is_ok());
    }

    #[test]
    fn rejects_existing_ancestor_symlinked_outside_home() {
        // An existing symlinked ancestor that escapes home must be rejected:
        // canonicalizing the nearest existing ancestor resolves the symlink, and
        // the resolved target is outside the allowed root. Anchored outside /tmp
        // so the symlink target isn't rescued by the /tmp root.
        let base = NonTmpBase::new("symlink");
        let home = base.path().join("home");
        fs::create_dir(&home).unwrap();
        let outside = base.path().join("outside");
        fs::create_dir(&outside).unwrap();
        let link = home.join("escape");
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        // `<home>/escape/sub` — nearest existing ancestor is the symlink, which
        // resolves to `outside`, not under home.
        assert!(!mkdir_target_is_confined(&link.join("sub"), &home));
    }
}

#[cfg(test)]
mod file_read_tests {
    use super::*;
    use crate::chain_qa::ChainQa;
    use crate::db::Database;
    use crate::platform::PlatformCapability;
    use crate::runtime::RuntimeManager;
    use crate::tools::mcp::McpClientManager;
    use phoenix_llm::ModelRegistry;
    use std::sync::Arc;

    /// Minimal `AppState` whose `preview_roots()` contains `cwd`, so the
    /// containment check in `read_file`/`list_files` admits files under it.
    async fn state_with_root(cwd: &std::path::Path) -> AppState {
        let db = Database::open_in_memory().await.expect("open db");
        db.create_conversation(
            "c-read",
            "read-test",
            &cwd.to_string_lossy(),
            true,
            None,
            None,
        )
        .await
        .expect("seed conversation");
        let llm_registry = Arc::new(ModelRegistry::new_empty());
        let platform = PlatformCapability::None {
            details: "test".into(),
        };
        let mcp_manager = Arc::new(McpClientManager::new());
        let runtime = Arc::new(RuntimeManager::new(
            db.clone(),
            llm_registry.clone(),
            platform.clone(),
            mcp_manager.clone(),
            None,
        ));
        let terminals = runtime.terminals.clone();
        let message_retriever: std::sync::Arc<dyn crate::db::MessageRetriever> =
            std::sync::Arc::new(crate::db::Fts5Retriever::new(db.pool().clone()));
        let chain_qa = ChainQa::new(db.clone(), llm_registry.clone(), message_retriever.clone());
        let sessions = super::super::auth::SessionStore::new(db.clone(), String::new());
        AppState {
            runtime,
            llm_registry,
            db,
            platform,
            mcp_manager,
            credential_helper: None,
            password: None,
            sessions,
            login_throttle: super::super::auth::LoginThrottle::new(),
            terminals,
            chain_qa,
            message_retriever,
            codex_login: super::super::codex_login::CodexLoginManager::new(),
            deployment: Arc::new(super::super::deployment::DeploymentConfig::for_tests()),
            runtime_env: Arc::new(phoenix_core::runtime_env::PhoenixRuntimeEnvironment::detect()),
            suggest_token: String::new(),
            discovery: crate::discovery::start(crate::discovery::DiscoveryConfig {
                enabled: false,
                ..crate::discovery::DiscoveryConfig::from_env()
            }),
        }
    }

    #[test]
    fn preview_url_percent_encodes_reserved_path_characters() {
        let path = std::path::Path::new("/tmp/screens/a #1?raw%.png");
        assert_eq!(
            preview_url_for_path(path),
            "/preview/tmp/screens/a%20%231%3Fraw%25.png"
        );
    }

    /// Helper: request `dir` (a directory) through `serve_preview_file`, which
    /// triggers the index.html fallback.
    #[cfg(unix)]
    async fn preview_dir(state: AppState, dir: &std::path::Path) -> Result<Response, AppError> {
        let filepath = dir.to_string_lossy().trim_start_matches('/').to_string();
        serve_preview_file(
            State(state),
            Path(filepath),
            Query(PreviewQuery { cwd: None }),
        )
        .await
    }

    /// Helper: request a file through `serve_preview_file` with an optional `cwd`.
    #[cfg(unix)]
    async fn preview_file(
        state: AppState,
        file: &std::path::Path,
        cwd: Option<String>,
    ) -> Result<Response, AppError> {
        let filepath = file.to_string_lossy().trim_start_matches('/').to_string();
        serve_preview_file(State(state), Path(filepath), Query(PreviewQuery { cwd })).await
    }

    /// A file admitted only by the cwd-widened allowlist (a fresh project's
    /// task/skill subtree) is previewable when `serve_preview_file` is given the
    /// same `cwd`, and NOT previewable without it.
    #[cfg(unix)]
    #[tokio::test]
    async fn preview_serves_cwd_widened_file_only_with_cwd() {
        let conv_root = tempfile::tempdir().expect("conv root");
        let project = tempfile::tempdir().expect("project");
        let skills = project.path().join(".claude").join("skills").join("s");
        std::fs::create_dir_all(&skills).expect("skill dir");
        let img = skills.join("diagram.png");
        std::fs::write(&img, [0x89, b'P', b'N', b'G', 0, 1, 2, 3]).expect("png");

        // No conversation roots the project, so without cwd the file is rejected.
        let state = state_with_root(conv_root.path()).await;
        let err = preview_file(state, &img, None)
            .await
            .expect_err("not previewable without cwd");
        assert!(matches!(err, AppError::NotFound(_)));

        // With the project's cwd, the same file is served.
        let state = state_with_root(conv_root.path()).await;
        let resp = preview_file(
            state,
            &img,
            Some(project.path().to_string_lossy().into_owned()),
        )
        .await
        .expect("previewable with cwd");
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
    }

    /// Regression: a directory's `index.html` that is a symlink pointing OUTSIDE
    /// the served root must not be served — otherwise malicious project contents
    /// turn `/preview/<dir>/` into an arbitrary host-file read.
    #[cfg(unix)]
    #[tokio::test]
    async fn preview_directory_index_symlink_escape_is_rejected() {
        let root = tempfile::tempdir().expect("root");
        let outside = tempfile::tempdir().expect("outside");
        let secret = outside.path().join("secret.txt");
        std::fs::write(&secret, b"TOP SECRET").expect("secret");

        let site = root.path().join("site");
        std::fs::create_dir(&site).expect("site dir");
        std::os::unix::fs::symlink(&secret, site.join("index.html")).expect("symlink");

        let state = state_with_root(root.path()).await;
        let err = preview_dir(state, &site)
            .await
            .expect_err("symlinked index escaping the root must be rejected");
        assert!(
            matches!(err, AppError::NotFound(_)),
            "expected NotFound, got {err:?}"
        );
    }

    /// A real `index.html` inside the served root is still served.
    #[cfg(unix)]
    #[tokio::test]
    async fn preview_directory_index_within_root_is_served() {
        let root = tempfile::tempdir().expect("root");
        let site = root.path().join("site");
        std::fs::create_dir(&site).expect("site dir");
        std::fs::write(site.join("index.html"), b"<html>ok</html>").expect("index");

        let state = state_with_root(root.path()).await;
        let resp = preview_dir(state, &site)
            .await
            .expect("in-root index must serve");
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
    }

    #[tokio::test]
    async fn read_file_returns_image_preview_for_png_without_utf8_validation() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("screenshot.png");
        std::fs::write(&path, [0x89, b'P', b'N', b'G', 0, 1, 2, 3]).expect("png bytes");
        let state = state_with_root(tmp.path()).await;

        let Json(response) = read_file(
            State(state),
            Query(PathQuery {
                path: path.to_string_lossy().to_string(),
                cwd: None,
            }),
        )
        .await
        .expect("image response");

        match response {
            ReadFileResponse::Image { mime_type, url } => {
                assert_eq!(mime_type, "image/png");
                // URL reflects the canonicalized (symlink-resolved) path.
                assert_eq!(
                    url,
                    preview_url_for_path(&std::fs::canonicalize(&path).unwrap())
                );
            }
            other @ ReadFileResponse::Text { .. } => {
                panic!("expected image response, got {other:?}")
            }
        }
    }

    #[tokio::test]
    async fn read_file_keeps_text_response_for_utf8_files() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("notes.txt");
        std::fs::write(&path, "hello\n").expect("text");
        let state = state_with_root(tmp.path()).await;

        let Json(response) = read_file(
            State(state),
            Query(PathQuery {
                path: path.to_string_lossy().to_string(),
                cwd: None,
            }),
        )
        .await
        .expect("text response");

        match response {
            ReadFileResponse::Text {
                content,
                encoding,
                category,
            } => {
                assert_eq!(content, "hello\n");
                assert_eq!(encoding, "utf-8");
                assert_eq!(category, crate::api::TextCategory::Plain);
            }
            other @ ReadFileResponse::Image { .. } => {
                panic!("expected text response, got {other:?}")
            }
        }
    }

    /// A file under a globally-discovered skill tree (`$HOME/.claude/skills`)
    /// must be previewable, even though it lives under no conversation cwd:
    /// `read_file` admits skill-tree files via `canonicalize_within_roots`, so
    /// `serve_preview_file` must honor the same allowlist or the follow-up
    /// `<img>`/HTML preview request 404s.
    ///
    #[tokio::test(flavor = "current_thread")]
    async fn preview_serves_file_under_skill_root() {
        // The home directory is supplied to the state via a `with_root`
        // PhoenixRuntimeEnvironment — no process-global `$HOME` mutation.
        let home = tempfile::tempdir().expect("home");
        let skill_dir = home.path().join(".claude").join("skills").join("my-skill");
        std::fs::create_dir_all(&skill_dir).expect("skill dir");
        let img = skill_dir.join("diagram.png");
        std::fs::write(&img, [0x89, b'P', b'N', b'G', 0, 1, 2, 3]).expect("png");

        // A conversation root that does NOT contain the skill file, proving the
        // skill file is admitted by the skill-root allowance, not the cwd.
        let root = tempfile::tempdir().expect("root");
        let mut state = state_with_root(root.path()).await;
        state.runtime_env =
            Arc::new(phoenix_core::runtime_env::PhoenixRuntimeEnvironment::with_root(home.path()));

        let canonical = std::fs::canonicalize(&img).expect("canonicalize img");
        let filepath = canonical
            .to_string_lossy()
            .trim_start_matches('/')
            .to_string();
        let resp = serve_preview_file(
            State(state),
            Path(filepath),
            Query(PreviewQuery { cwd: None }),
        )
        .await
        .expect("skill-root file must be previewable");
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
    }

    #[tokio::test]
    async fn read_file_rejects_path_outside_roots() {
        // A file that exists but lives under no conversation root must read as
        // NotFound — existence is never leaked.
        let root = tempfile::tempdir().expect("root tempdir");
        let outside = tempfile::tempdir().expect("outside tempdir");
        let secret = outside.path().join("secret.txt");
        std::fs::write(&secret, "top secret\n").expect("write secret");
        let state = state_with_root(root.path()).await;

        let err = read_file(
            State(state),
            Query(PathQuery {
                path: secret.to_string_lossy().to_string(),
                cwd: None,
            }),
        )
        .await
        .expect_err("out-of-scope read must fail");
        assert!(matches!(err, AppError::NotFound(_)));
    }

    /// A task file under `<cwd>/tasks` must be readable via the `cwd` query
    /// param even when `cwd` is NOT a conversation root — this is the
    /// new-conversation flow opening a freshly-picked project's task before any
    /// conversation (and thus preview root) exists for it.
    #[tokio::test]
    async fn read_file_admits_task_under_provided_cwd() {
        // The conversation root is unrelated to `project`; only the `cwd` param
        // makes the task readable, proving the widening — not preview_roots — is
        // responsible.
        let conv_root = tempfile::tempdir().expect("conv root");
        let project = tempfile::tempdir().expect("project");
        let tasks_dir = project.path().join("tasks");
        std::fs::create_dir_all(&tasks_dir).expect("tasks dir");
        let task = tasks_dir.join("00001-p1-ready--fix.md");
        std::fs::write(&task, "task body\n").expect("write task");

        let state = state_with_root(conv_root.path()).await;

        // Without cwd: rejected (not under any base root).
        let err = read_file(
            State(state.clone()),
            Query(PathQuery {
                path: task.to_string_lossy().to_string(),
                cwd: None,
            }),
        )
        .await
        .expect_err("task outside roots without cwd must fail");
        assert!(matches!(err, AppError::NotFound(_)));

        // With cwd: admitted.
        let Json(response) = read_file(
            State(state),
            Query(PathQuery {
                path: task.to_string_lossy().to_string(),
                cwd: Some(project.path().to_string_lossy().to_string()),
            }),
        )
        .await
        .expect("task under cwd/tasks must be readable");
        match response {
            ReadFileResponse::Text { content, .. } => assert_eq!(content, "task body\n"),
            other @ ReadFileResponse::Image { .. } => panic!("expected text, got {other:?}"),
        }
    }

    /// Regression: a task/skill subtree that is a symlink to the cwd ITSELF
    /// (`tasks -> .`) must NOT collapse the allowed root to the whole project —
    /// otherwise `read_file?cwd=<project>&path=<project>/.env` would be admitted.
    #[cfg(unix)]
    #[tokio::test]
    async fn read_file_rejects_cwd_subtree_symlinked_to_cwd_itself() {
        let conv_root = tempfile::tempdir().expect("conv root");
        let project = tempfile::tempdir().expect("project");
        // `tasks` resolves to the project root itself.
        std::os::unix::fs::symlink(project.path(), project.path().join("tasks"))
            .expect("symlink tasks -> .");
        let secret = project.path().join(".env");
        std::fs::write(&secret, "SECRET=1\n").expect("write .env");

        let state = state_with_root(conv_root.path()).await;
        let err = read_file(
            State(state),
            Query(PathQuery {
                path: secret.to_string_lossy().to_string(),
                cwd: Some(project.path().to_string_lossy().to_string()),
            }),
        )
        .await
        .expect_err("a subtree symlinked to the cwd itself must not widen to the whole project");
        assert!(matches!(err, AppError::NotFound(_)));
    }

    /// Providing a `cwd` must NOT turn `read_file` into an arbitrary host-file
    /// read: a file under `cwd` but OUTSIDE the three task/skill subtrees stays
    /// rejected. Concretely, an attacker passing `cwd=<project>` cannot read
    /// `<project>/secret.txt` (it is not under tasks/ or .claude/skills/ or
    /// .agents/skills/), nor escape via `..`.
    #[tokio::test]
    async fn read_file_with_cwd_still_rejects_outside_subtrees() {
        let conv_root = tempfile::tempdir().expect("conv root");
        let project = tempfile::tempdir().expect("project");
        // Make the subtrees exist so the widening actually adds roots, yet the
        // requested file lives beside them, not within them.
        std::fs::create_dir_all(project.path().join("tasks")).expect("tasks dir");
        let secret = project.path().join("secret.txt");
        std::fs::write(&secret, "top secret\n").expect("write secret");

        let state = state_with_root(conv_root.path()).await;

        let err = read_file(
            State(state),
            Query(PathQuery {
                path: secret.to_string_lossy().to_string(),
                cwd: Some(project.path().to_string_lossy().to_string()),
            }),
        )
        .await
        .expect_err("file outside the task/skill subtrees must stay rejected");
        assert!(matches!(err, AppError::NotFound(_)));
    }

    /// Regression: a project whose task subtree is a SYMLINK escaping the
    /// project (e.g. `tasks -> /`) must not widen the allowlist to the symlink
    /// target — otherwise `read_file?cwd=<project>&path=<outside file>` would
    /// re-open arbitrary host-file read.
    #[cfg(unix)]
    #[tokio::test]
    async fn read_file_rejects_cwd_subtree_symlinked_outside_project() {
        let conv_root = tempfile::tempdir().expect("conv root");
        let project = tempfile::tempdir().expect("project");
        let outside = tempfile::tempdir().expect("outside");
        let secret = outside.path().join("secret.txt");
        std::fs::write(&secret, "top secret\n").expect("write secret");
        // Malicious project: its task dir is a symlink escaping the project.
        std::os::unix::fs::symlink(outside.path(), project.path().join("tasks"))
            .expect("symlink tasks");

        let state = state_with_root(conv_root.path()).await;

        let err = read_file(
            State(state),
            Query(PathQuery {
                path: secret.to_string_lossy().to_string(),
                cwd: Some(project.path().to_string_lossy().to_string()),
            }),
        )
        .await
        .expect_err("a task subtree symlinked outside the project must not widen the allowlist");
        assert!(matches!(err, AppError::NotFound(_)));
    }

    /// A skill file under the global `$HOME/.agents/skills` tree must be
    /// previewable through the base allowlist — `discover_skills` advertises
    /// skills from `.agents/skills` as well as `.claude/skills`, so the viewer
    /// must be able to read them.
    ///
    #[tokio::test(flavor = "current_thread")]
    async fn read_file_admits_global_agents_skill() {
        // Home supplied via a `with_root` PhoenixRuntimeEnvironment — no
        // process-global `$HOME` mutation.
        let home = tempfile::tempdir().expect("home");
        let skill_dir = home.path().join(".agents").join("skills").join("my-skill");
        std::fs::create_dir_all(&skill_dir).expect("skill dir");
        let skill = skill_dir.join("SKILL.md");
        std::fs::write(&skill, "skill prompt\n").expect("write skill");

        // A conversation root that does NOT contain the skill, proving the
        // `.agents/skills` allowance — not the cwd — admits it.
        let root = tempfile::tempdir().expect("root");
        let mut state = state_with_root(root.path()).await;
        state.runtime_env =
            Arc::new(phoenix_core::runtime_env::PhoenixRuntimeEnvironment::with_root(home.path()));

        let Json(response) = read_file(
            State(state),
            Query(PathQuery {
                path: skill.to_string_lossy().to_string(),
                cwd: None,
            }),
        )
        .await
        .expect(".agents/skills file must be readable");
        match response {
            ReadFileResponse::Text { content, .. } => assert_eq!(content, "skill prompt\n"),
            other @ ReadFileResponse::Image { .. } => panic!("expected text, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod attachment_storage_tests {
    use super::*;

    #[test]
    fn sanitizes_attachment_name_to_basename_ascii() {
        assert_eq!(
            sanitize_attachment_name("../../secret notes.txt"),
            "secret_notes.txt"
        );
        assert_eq!(sanitize_attachment_name("..."), "attachment");
    }

    #[test]
    fn sweep_deletes_expired_files_and_empty_dirs() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        let conv = root.join("conv-old");
        std::fs::create_dir_all(&conv).expect("create conv dir");
        let expired = conv.join("old.txt");
        std::fs::write(&expired, b"old").expect("write old");
        let cutoff = SystemTime::now() + Duration::from_secs(1);
        sweep_expired_attachments_blocking(root, cutoff, &HashSet::new()).expect("sweep");
        assert!(!expired.exists());
        assert!(
            !conv.exists(),
            "empty conversation attachment dir should be removed"
        );
    }

    #[test]
    fn sweep_preserves_expired_referenced_files() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        let conv = root.join("conv-referenced");
        std::fs::create_dir_all(&conv).expect("create conv dir");
        let referenced = conv.join("old-but-referenced.txt");
        std::fs::write(&referenced, b"keep").expect("write referenced");
        let cutoff = SystemTime::now() + Duration::from_secs(1);
        let referenced_set = HashSet::from([referenced.clone()]);
        sweep_expired_attachments_blocking(root, cutoff, &referenced_set).expect("sweep");
        assert!(referenced.exists());
        assert!(conv.exists());
    }

    #[tokio::test]
    async fn referenced_paths_include_creation_job_files() {
        let db = crate::db::Database::open_in_memory()
            .await
            .expect("open db");
        db.create_conversation(
            "conv-creation-file",
            "creation-file",
            "/tmp",
            true,
            None,
            None,
        )
        .await
        .expect("create conversation");
        let stored_path = "/tmp/phoenix-creation-job-attachment.txt".to_string();
        let intent = crate::db::ConversationCreationIntent {
            cwd: "/tmp".to_string(),
            model: None,
            text: "with attachment".to_string(),
            expansion_preflighted: false,
            llm_text: None,
            skill_invocation: None,
            message_id: "msg-creation-file".to_string(),
            images: vec![],
            files: vec![crate::db::FileAttachment {
                original_name: "attachment.txt".to_string(),
                media_type: "text/plain".to_string(),
                size_bytes: 12,
                stored_path: stored_path.clone(),
            }],
            mode: None,
            base_branch: None,
            checkout_ref: None,
            seed_parent_id: None,
            seed_label: None,
        };
        db.insert_conversation_creation_job(&crate::db::InsertConversationCreationJob {
            id: "job-creation-file".to_string(),
            conversation_id: "conv-creation-file".to_string(),
            message_id: Some("msg-creation-file".to_string()),
            intent,
        })
        .await
        .expect("insert job");

        let referenced = referenced_attachment_paths(&db).await.expect("references");
        assert!(
            referenced.contains(&PathBuf::from(stored_path)),
            "creation job intent files must be protected from TTL cleanup"
        );
    }

    #[test]
    fn sweep_keeps_recent_files() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        let conv = root.join("conv-recent");
        std::fs::create_dir_all(&conv).expect("create conv dir");
        let recent = conv.join("recent.txt");
        std::fs::write(&recent, b"recent").expect("write recent");
        let cutoff = SystemTime::UNIX_EPOCH;
        sweep_expired_attachments_blocking(root, cutoff, &HashSet::new()).expect("sweep");
        assert!(recent.exists());
        assert!(conv.exists());
    }

    #[tokio::test]
    async fn hard_delete_attachment_cleanup_removes_conversation_dir() {
        let temp = tempfile::tempdir().expect("tempdir");
        let dir = temp.path().join("conv-delete");
        make_attachment_dir_private(&dir).await.expect("secure dir");
        let file = dir.join("attachment.txt");
        write_attachment_file_private(&file, b"hello")
            .await
            .expect("write file");
        delete_conversation_attachments_at_root(temp.path().to_path_buf(), "conv-delete").await;
        assert!(!dir.exists());
    }

    #[tokio::test]
    async fn writes_private_attachment_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let dir = temp.path().join("conv-1");
        make_attachment_dir_private(&dir).await.expect("secure dir");
        let file = dir.join("attachment.txt");
        write_attachment_file_private(&file, b"hello")
            .await
            .expect("write file");
        assert_eq!(tokio::fs::read(&file).await.expect("read"), b"hello");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let dir_mode = std::fs::metadata(&dir)
                .expect("dir metadata")
                .permissions()
                .mode()
                & 0o777;
            let file_mode = std::fs::metadata(&file)
                .expect("file metadata")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(dir_mode, 0o700);
            assert_eq!(file_mode, 0o600);
        }
    }
}

#[cfg(test)]
mod chat_authority_tests {
    //! Regression: `POST /chat` must use live runtime state as the routing authority.
    //!
    //! The split-brain scenario (FM-7, specs/bedrock/design.md):
    //! - DB row says `Idle` (written by `reset_all_to_idle` on startup)
    //! - Live executor is in `LlmRequesting` (set by `determine_resume_state`
    //!   detecting `InterruptedMidTurn`)
    //!
    //! A handler that reads only the DB row routes a `UserMessage`, gets
    //! `AgentBusy` from the executor, and surfaces a post-200 error.
    //!
    //! The fix: `effective_conversation_state` returns the live state; the
    //! handler queues a steering message instead.
    use super::*;
    use crate::db::Database;
    use crate::platform::PlatformCapability;
    use crate::runtime::RuntimeManager;
    use crate::state_machine::ConvState;
    use crate::tools::mcp::McpClientManager;
    use phoenix_llm::ModelRegistry;
    use std::sync::Arc;

    /// Build a minimal `AppState` backed by an in-memory database.
    async fn make_state() -> AppState {
        let db = Database::open_in_memory().await.expect("open db");
        let llm_registry = Arc::new(ModelRegistry::new_empty());
        let platform = PlatformCapability::None {
            details: "test".to_string(),
        };
        let mcp_manager = Arc::new(McpClientManager::new());
        let runtime = Arc::new(RuntimeManager::new(
            db.clone(),
            llm_registry.clone(),
            platform.clone(),
            mcp_manager.clone(),
            None,
        ));
        let terminals = runtime.terminals.clone();
        let message_retriever: Arc<dyn crate::db::MessageRetriever> =
            Arc::new(crate::db::Fts5Retriever::new(db.pool().clone()));
        let chain_qa = crate::chain_qa::ChainQa::new(
            db.clone(),
            llm_registry.clone(),
            message_retriever.clone(),
        );
        let sessions = super::super::auth::SessionStore::new(db.clone(), String::new());
        AppState {
            runtime,
            llm_registry,
            db,
            platform,
            mcp_manager,
            credential_helper: None,
            password: None,
            sessions,
            login_throttle: super::super::auth::LoginThrottle::new(),
            terminals,
            chain_qa,
            message_retriever,
            codex_login: super::super::codex_login::CodexLoginManager::new(),
            deployment: Arc::new(super::super::deployment::DeploymentConfig::for_tests()),
            runtime_env: Arc::new(phoenix_core::runtime_env::PhoenixRuntimeEnvironment::detect()),
            suggest_token: String::new(),
            discovery: crate::discovery::start(crate::discovery::DiscoveryConfig {
                enabled: false,
                ..crate::discovery::DiscoveryConfig::from_env()
            }),
        }
    }

    /// Regression for FM-7: DB row says `Idle`, live runtime says `LlmRequesting`.
    /// `POST /chat` must queue the message as a steering directive, not reject it
    /// with `AgentBusy` after a spurious `200 OK`.
    #[tokio::test]
    async fn live_llm_requesting_routes_to_steering_despite_idle_db_row() {
        let state = make_state().await;

        // Create conversation — DB row starts Idle by default.
        state
            .db
            .create_conversation("c-fm7", "fm7", "/tmp", true, None, None)
            .await
            .expect("create");

        // Verify DB row is Idle (the safe rest-state after reset_all_to_idle).
        let conv = state.db.get_conversation("c-fm7").await.expect("get");
        assert!(
            matches!(conv.state, ConvState::Idle),
            "precondition: DB row must be Idle"
        );

        // Inject a live handle whose state_rx reports LlmRequesting.
        // This mirrors the window after restart auto-resume where the executor
        // has entered LlmRequesting but no DB write has occurred yet.
        state
            .runtime
            .inject_handle_for_test("c-fm7", ConvState::LlmRequesting { attempt: 1 })
            .await;

        // Verify effective_conversation_state returns the live state.
        let effective = state
            .runtime
            .effective_conversation_state("c-fm7")
            .await
            .expect("handle present");
        assert!(
            matches!(effective, ConvState::LlmRequesting { .. }),
            "effective state must reflect live runtime, got {effective:?}"
        );

        // POST /chat: must queue as steering (not reject AgentBusy).
        let req = ChatRequest {
            text: "continue please".to_string(),
            message_id: uuid::Uuid::new_v4().to_string(),
            images: vec![],
            files: vec![],
            user_agent: None,
        };
        let result = send_chat(State(state.clone()), Path("c-fm7".to_string()), Json(req))
            .await
            .expect("must not return Err — steering should succeed");

        assert!(result.0.queued, "message must be queued");
        assert!(
            result.0.steering,
            "must be routed as steering, not UserMessage"
        );
    }
}
