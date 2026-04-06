//! HTTP request handlers
//!
//! REQ-API-001 through REQ-API-010

use super::assets::{get_index_html, serve_favicon, serve_service_worker, serve_static};
use super::sse::sse_stream;
use super::types::{
    CancelResponse, ChatRequest, ChatResponse, CompleteTaskResponse, ConfirmCompleteRequest,
    ConfirmCompleteResponse, ConflictErrorResponse, ConversationListResponse, ConversationResponse,
    ConversationWithMessagesResponse, CreateConversationRequest, DirectoryEntry, ErrorResponse,
    ExpansionErrorResponse, FileEntry, FileSearchEntry, FileSearchQuery, FileSearchResponse,
    GatewayStatusApi, ListDirectoryResponse, ListFilesResponse, MkdirResponse, ModelsResponse,
    ReadFileResponse, RenameRequest, SkillEntry, SkillsResponse, SuccessResponse,
    SystemPromptResponse, TaskApprovalResponse, TaskEntry, TaskFeedbackRequest, TasksResponse,
    ValidateCwdResponse,
};
use super::AppState;
use crate::db::{ConvMode, ImageData, Message, MessageContent, MessageType};
use crate::llm::{
    ContentBlock, GatewayStatus, LlmMessage, LlmRequest, MessageRole,
    SystemContent as LlmSystemContent,
};
use crate::runtime::executor::{run_git, TASK_APPROVAL_MUTEX};
use crate::runtime::SseEvent;
use crate::state_machine::state::TaskApprovalOutcome;
use crate::state_machine::{ConvState, Event};

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
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
        // SSE streaming (REQ-API-005)
        .route("/api/conversations/:id/stream", get(stream_conversation))
        // User actions (REQ-API-004)
        .route("/api/conversations/:id/chat", post(send_chat))
        .route("/api/conversations/:id/cancel", post(cancel_conversation))
        .route(
            "/api/conversations/:id/trigger-continuation",
            post(trigger_continuation),
        )
        // Task approval (REQ-BED-028)
        .route("/api/conversations/:id/approve-task", post(approve_task))
        .route("/api/conversations/:id/reject-task", post(reject_task))
        .route("/api/conversations/:id/task-feedback", post(task_feedback))
        // User question response (REQ-AUQ-003)
        .route("/api/conversations/:id/respond", post(respond_to_question))
        // Task completion (REQ-PROJ-009)
        .route("/api/conversations/:id/complete-task", post(complete_task))
        .route(
            "/api/conversations/:id/confirm-complete",
            post(confirm_complete),
        )
        // Task abandon (REQ-PROJ-010)
        .route("/api/conversations/:id/abandon-task", post(abandon_task))
        // Lifecycle (REQ-API-006)
        .route("/api/conversations/:id/archive", post(archive_conversation))
        .route(
            "/api/conversations/:id/unarchive",
            post(unarchive_conversation),
        )
        .route("/api/conversations/:id/delete", post(delete_conversation))
        .route("/api/conversations/:id/rename", post(rename_conversation))
        // System prompt inspection
        .route(
            "/api/conversations/:id/system-prompt",
            get(get_system_prompt),
        )
        // Slug resolution (REQ-API-007)
        .route("/api/conversations/by-slug/:slug", get(get_by_slug))
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
        // Environment info
        .route("/api/env", get(get_env))
        // MCP management
        .route("/api/mcp/status", get(mcp_status))
        .route("/api/mcp/reload", post(reload_mcp))
        .route("/api/mcp/servers/:name/disable", post(disable_mcp_server))
        .route("/api/mcp/servers/:name/enable", post(enable_mcp_server))
        // Version
        .route("/version", get(get_version))
        .with_state(state)
}

// ============================================================
// Message Transformation
// ============================================================

/// Transform a message for API output, enriching bash `tool_use` blocks with display info.
///
/// Transform a message for API output by merging `display_data` into content blocks.
///
/// For agent messages with bash `tool_use` blocks, the `display` field shows a
/// simplified command (with cd prefixes stripped when they match cwd).
/// The `display_data` is pre-computed at message creation time and stored in DB.
pub(crate) fn enrich_message_for_api(msg: &Message) -> Value {
    let mut json = serde_json::to_value(msg).unwrap_or(Value::Null);

    // Only process agent messages with display_data
    if msg.message_type != MessageType::Agent {
        return json;
    }

    if let Some(display_data) = &msg.display_data {
        merge_display_data_into_content(&mut json, display_data);
    }

    json
}

/// Count how many commits `base_branch` is ahead of `task_branch` in `repo_root`.
///
/// Shells out to `git rev-list --count`. Returns 0 on any error (missing branch,
/// git not available, parse failure). This is a best-effort indicator.
///
/// **Blocking** -- must be called from `spawn_blocking` or an already-blocking context.
fn commits_behind(repo_root: &std::path::Path, base_branch: &str, task_branch: &str) -> u32 {
    let range = format!("{task_branch}..{base_branch}");
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
/// Shells out to `git rev-list --count`. Returns 0 on any error.
///
/// **Blocking** -- must be called from `spawn_blocking` or an already-blocking context.
fn commits_ahead(repo_root: &std::path::Path, base_branch: &str, task_branch: &str) -> u32 {
    let range = format!("{base_branch}..{task_branch}");
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
fn enrich_conversation(conv: &crate::db::Conversation) -> crate::runtime::EnrichedConversation {
    crate::runtime::EnrichedConversation {
        conv_mode_label: conv.conv_mode.label().to_string(),
        branch_name: conv.conv_mode.branch_name().map(String::from),
        worktree_path: conv
            .conv_mode
            .worktree_path()
            .filter(|s| !s.is_empty())
            .map(String::from),
        base_branch: conv
            .conv_mode
            .base_branch()
            .filter(|s| !s.is_empty())
            .map(String::from),
        inner: conv.clone(),
    }
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

fn merge_display_data_into_content(json: &mut Value, display_data: &Value) {
    // Build a map from tool_use_id -> display
    let bash_displays: std::collections::HashMap<String, String> = display_data
        .get("bash")
        .and_then(|b| b.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| {
                    let id = item.get("tool_use_id")?.as_str()?;
                    let display = item.get("display")?.as_str()?;
                    Some((id.to_string(), display.to_string()))
                })
                .collect()
        })
        .unwrap_or_default();

    if bash_displays.is_empty() {
        return;
    }

    // Merge into content blocks
    if let Some(content) = json.get_mut("content").and_then(|c| c.as_array_mut()) {
        for block in content.iter_mut() {
            let is_bash_tool_use = block.get("type").and_then(|t| t.as_str()) == Some("tool_use")
                && block.get("name").and_then(|n| n.as_str()) == Some("bash");

            if !is_bash_tool_use {
                continue;
            }

            if let Some(id) = block.get("id").and_then(|i| i.as_str()) {
                if let Some(display) = bash_displays.get(id) {
                    if let Some(obj) = block.as_object_mut() {
                        obj.insert("display".to_string(), Value::String(display.clone()));
                    }
                }
            }
        }
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

    // Validate message text is not empty
    if req.text.trim().is_empty() {
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

    // Try to generate a title using a cheap LLM model
    let slug = if let Some(cheap_model) = state.runtime.model_registry().get_cheap_model() {
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

    // Create conversation (REQ-PROJ-002: Explore for git repos, Standalone otherwise)
    let conv_mode = if project_id.is_some() {
        crate::db::ConvMode::Explore
    } else {
        crate::db::ConvMode::Standalone
    };
    let conversation = state
        .runtime
        .db()
        .create_conversation_with_project(
            &id,
            &slug,
            &req.cwd,
            true,                 // user_initiated
            None,                 // no parent
            req.model.as_deref(), // selected model
            project_id.as_deref(),
            &conv_mode,
        )
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    // Expand `@file` inline references before sending (REQ-IR-001, REQ-IR-007)
    let working_dir_for_expand = std::path::PathBuf::from(&req.cwd);
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

    Ok(Json(ConversationResponse {
        conversation: serde_json::to_value(conversation).unwrap_or(Value::Null),
    }))
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
        conversation: conversation_to_json(&conversation),
        messages: json_msgs,
        agent_working: conversation.is_agent_working(),
        display_state: conversation.state.display_state().as_str().to_string(),
        context_window_size,
    }))
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

#[derive(Debug, Deserialize)]
struct StreamQuery {
    after: Option<i64>,
}

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
    Query(query): Query<StreamQuery>,
) -> Result<impl IntoResponse, AppError> {
    let conversation = state
        .runtime
        .db()
        .get_conversation(&id)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;

    // Get messages (filtered by after if provided)
    let messages = if let Some(after) = query.after {
        state.runtime.db().get_messages_after(&id, after).await
    } else {
        state.runtime.db().get_messages(&id).await
    }
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

    // Get model's context window for percentage calculation
    let model_id = conversation
        .model
        .as_deref()
        .unwrap_or(state.llm_registry.default_model_id());
    let model_context_window = state.llm_registry.context_window(model_id);

    // Compute initial commits_behind for Work conversations.
    // Extract the git info we need for both the init value and the polling task.
    let work_git_info = match &conversation.conv_mode {
        ConvMode::Work {
            branch_name,
            base_branch,
            ..
        } if !base_branch.is_empty() && !branch_name.is_empty() => {
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
            repo_root.map(|root| (root, base_branch.clone(), branch_name.clone()))
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

    // Create init event with typed data -- serialization deferred to SSE layer
    let init_event = SseEvent::Init {
        conversation: Box::new(enrich_conversation(&conversation)),
        messages,
        agent_working: conversation.is_agent_working(),
        display_state: conversation.state.display_state().as_str().to_string(),
        last_sequence_id,
        context_window_size,
        model_context_window,
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
                    let result = broadcast_tx.send(SseEvent::ConversationUpdate {
                        update: crate::runtime::ConversationMetadataUpdate {
                            cwd: None,
                            branch_name: None,
                            worktree_path: None,
                            conv_mode_label: None,
                            base_branch: None,
                            commits_behind: Some(new_behind),
                            commits_ahead: Some(new_ahead),
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

    Ok(sse_stream(init_event, broadcast_rx))
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
    state
        .runtime
        .send_event(&id, Event::UserCancel)
        .await
        .map_err(AppError::BadRequest)?;

    Ok(Json(CancelResponse { ok: true }))
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

// ============================================================
// Task Approval (REQ-BED-028)
// ============================================================

async fn approve_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<TaskApprovalResponse>, AppError> {
    // 1. Validate conversation exists and is in AwaitingTaskApproval state
    let conv = state
        .runtime
        .db()
        .get_conversation(&id)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;

    if !matches!(conv.state, ConvState::AwaitingTaskApproval { .. }) {
        return Err(AppError::BadRequest(
            "Conversation is not awaiting task approval".to_string(),
        ));
    }

    // 2. Non-project conversations cannot approve tasks (propose_task is project-only)
    if conv.project_id.is_none() {
        return Err(AppError::BadRequest(
            "Task approval requires a project-scoped conversation".to_string(),
        ));
    }

    // 3. Dispatch approval event to state machine
    state
        .runtime
        .send_event(
            &id,
            Event::TaskApprovalResponse {
                outcome: TaskApprovalOutcome::Approved,
            },
        )
        .await
        .map_err(AppError::BadRequest)?;

    Ok(Json(TaskApprovalResponse {
        success: true,
        first_task: None, // Set by executor via SSE if applicable
    }))
}

async fn reject_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<SuccessResponse>, AppError> {
    // Validate conversation exists and is in AwaitingTaskApproval state
    let conv = state
        .runtime
        .db()
        .get_conversation(&id)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;

    if !matches!(conv.state, ConvState::AwaitingTaskApproval { .. }) {
        return Err(AppError::BadRequest(
            "Conversation is not awaiting task approval".to_string(),
        ));
    }

    state
        .runtime
        .send_event(
            &id,
            Event::TaskApprovalResponse {
                outcome: TaskApprovalOutcome::Rejected,
            },
        )
        .await
        .map_err(AppError::BadRequest)?;

    Ok(Json(SuccessResponse { success: true }))
}

async fn task_feedback(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<TaskFeedbackRequest>,
) -> Result<Json<SuccessResponse>, AppError> {
    // Validate conversation exists and is in AwaitingTaskApproval state
    let conv = state
        .runtime
        .db()
        .get_conversation(&id)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;

    if !matches!(conv.state, ConvState::AwaitingTaskApproval { .. }) {
        return Err(AppError::BadRequest(
            "Conversation is not awaiting task approval".to_string(),
        ));
    }

    state
        .runtime
        .send_event(
            &id,
            Event::TaskApprovalResponse {
                outcome: TaskApprovalOutcome::FeedbackProvided {
                    annotations: req.annotations,
                },
            },
        )
        .await
        .map_err(AppError::BadRequest)?;

    Ok(Json(SuccessResponse { success: true }))
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
        return Err(AppError::Conflict(ConflictErrorResponse::new(
            "Conversation is not awaiting a user response",
            "wrong_state",
        )));
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
// Task Completion (REQ-PROJ-009)
// ============================================================

/// Pre-check endpoint: validates worktree state, detects conflicts, generates commit message.
/// Does NOT merge -- the user reviews the commit message first.
#[derive(Debug, Deserialize)]
struct CompleteTaskQuery {
    #[serde(default)]
    auto_stash: bool,
}

#[allow(clippy::too_many_lines)] // Sequential validation + LLM call; splitting hurts readability
async fn complete_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<CompleteTaskQuery>,
) -> Result<Json<CompleteTaskResponse>, AppError> {
    // 1. Validate conversation exists, is Work mode, Idle state, project-scoped
    let conv = state
        .runtime
        .db()
        .get_conversation(&id)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;

    if !matches!(conv.state, ConvState::Idle) {
        return Err(AppError::BadRequest(
            "Conversation must be idle to complete a task".to_string(),
        ));
    }

    let (branch_name, worktree_path, base_branch, task_id) = match &conv.conv_mode {
        ConvMode::Work {
            branch_name,
            worktree_path,
            base_branch,
            task_id,
        } => (
            branch_name.clone(),
            worktree_path.clone(),
            base_branch.clone(),
            task_id.clone(),
        ),
        _ => {
            return Err(AppError::BadRequest(
                "Conversation is not in Work mode".to_string(),
            ));
        }
    };

    let project_id = conv
        .project_id
        .as_deref()
        .ok_or_else(|| AppError::BadRequest("Conversation is not project-scoped".to_string()))?;

    // Look up project to get canonical_path (repo root)
    let project = state
        .db
        .get_project(project_id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let repo_root = PathBuf::from(&project.canonical_path);
    let worktree_dir = PathBuf::from(&worktree_path);

    // Capture what we need for the blocking section
    let base_branch_clone = base_branch.clone();
    let branch_name_clone = branch_name.clone();

    // 2. Pre-checks (blocking git operations)
    let prechecks = tokio::task::spawn_blocking(move || -> Result<String, AppError> {
        // 2a. Worktree must be clean
        let wt_status = run_git(&worktree_dir, &["status", "--porcelain"])
            .map_err(AppError::Internal)?;
        if !wt_status.is_empty() {
            return Err(AppError::Conflict(ConflictErrorResponse::new(
                "Worktree has uncommitted changes. Ask the agent to commit or stash before completing.",
                "dirty_worktree",
            )));
        }

        // 2b. Main checkout must be clean (unless auto_stash was requested)
        if !query.auto_stash {
            let main_status = run_git(&repo_root, &["status", "--porcelain"])
                .map_err(AppError::Internal)?;
            if !main_status.is_empty() {
                let dirty_files: Vec<String> = main_status
                    .lines()
                    .map(|l| l.trim().to_string())
                    .collect();

                // Check if auto-stash would be safe: dirty files must not overlap
                // with files changed by the task branch.
                let can_auto_stash =
                    check_auto_stash_safe(&repo_root, &base_branch_clone, &branch_name_clone);

                return Err(AppError::Conflict(ConflictErrorResponse {
                    error: "Main checkout has uncommitted changes.".to_string(),
                    error_type: "dirty_main_checkout".to_string(),
                    dirty_files,
                    can_auto_stash,
                }));
            }
        }

        // 2c. Conflict detection via merge-tree
        let merge_base = run_git(
            &worktree_dir,
            &["merge-base", &base_branch_clone, "HEAD"],
        )
        .map_err(|e| AppError::Internal(format!("Failed to find merge base: {e}")))?;

        let merge_tree_output = run_git(
            &worktree_dir,
            &["merge-tree", &merge_base, &base_branch_clone, "HEAD"],
        )
        .unwrap_or_default();
        // merge-tree outputs conflict markers if there are conflicts
        if merge_tree_output.contains("<<<<<<") || merge_tree_output.contains("changed in both") {
            return Err(AppError::Conflict(ConflictErrorResponse::new(
                format!("Merge conflicts detected between your branch and {base_branch_clone}. Rebase first."),
                "merge_conflicts",
            )));
        }

        // 2d. Get diff for commit message generation
        let diff = run_git(
            &worktree_dir,
            &["diff", &format!("{base_branch_clone}...HEAD")],
        )
        .unwrap_or_default();

        let diff_content = if diff.len() > 50_000 {
            // Fall back to diff --stat if diff is too large
            run_git(
                &worktree_dir,
                &["diff", "--stat", &format!("{base_branch_clone}...HEAD")],
            )
            .unwrap_or_else(|_| "(no diff available)".to_string())
        } else {
            diff
        };

        Ok(diff_content)
    })
    .await
    .map_err(|e| AppError::Internal(format!("Blocking task failed: {e}")))?;

    let diff_content = prechecks?;

    // 3. Task file nudge check
    let task_not_done = check_task_file_status(&PathBuf::from(&worktree_path), &task_id);

    // 4. Generate commit message via LLM
    let model_id = conv
        .model
        .as_deref()
        .unwrap_or_else(|| state.llm_registry.default_model_id());

    let llm_service = state
        .llm_registry
        .get(model_id)
        .ok_or_else(|| AppError::Internal(format!("LLM model '{model_id}' not available")))?;

    let system_prompt = "You are writing a git commit message for a squash merge. \
        Write a semantic commit message in imperative mood. Focus on WHAT changed and WHY, \
        not which files were modified. The message should have:\n\
        - A concise subject line (max 72 chars), using a conventional prefix \
          (feat:, fix:, refactor:, docs:, test:, chore:)\n\
        - An optional body separated by a blank line with more detail if the change is complex\n\n\
        Output ONLY the commit message text, nothing else. No markdown formatting, no code blocks.";

    let user_msg = if diff_content.is_empty() {
        "No diff found between branches. Write a generic commit message: 'chore: merge task branch'"
            .to_string()
    } else {
        format!("Write a commit message for this diff:\n\n{diff_content}")
    };

    let request = LlmRequest {
        system: vec![LlmSystemContent::new(system_prompt)],
        messages: vec![LlmMessage {
            role: MessageRole::User,
            content: vec![ContentBlock::text(user_msg)],
        }],
        tools: vec![],
        max_tokens: Some(500),
    };

    let commit_message = match llm_service.complete(&request).await {
        Ok(response) => {
            let text = response.text();
            if text.is_empty() {
                format!("feat: complete task from branch {branch_name}")
            } else {
                text
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "LLM commit message generation failed, using fallback");
            format!("feat: complete task from branch {branch_name}")
        }
    };

    Ok(Json(CompleteTaskResponse {
        success: true,
        commit_message,
        task_not_done: if task_not_done { Some(true) } else { None },
    }))
}

/// Confirm and execute the squash merge after the user reviews the commit message.
#[allow(clippy::too_many_lines)] // Sequential git + DB operations; splitting hurts readability
async fn confirm_complete(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<ConfirmCompleteRequest>,
) -> Result<Json<ConfirmCompleteResponse>, AppError> {
    // 1. Re-validate conversation state (race guard)
    let conv = state
        .runtime
        .db()
        .get_conversation(&id)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;

    if !matches!(conv.state, ConvState::Idle) {
        return Err(AppError::BadRequest(
            "Conversation must be idle to complete a task".to_string(),
        ));
    }

    let (branch_name, worktree_path, base_branch, task_id) = match &conv.conv_mode {
        ConvMode::Work {
            branch_name,
            worktree_path,
            base_branch,
            task_id,
        } => (
            branch_name.clone(),
            worktree_path.clone(),
            base_branch.clone(),
            task_id.clone(),
        ),
        _ => {
            return Err(AppError::BadRequest(
                "Conversation is not in Work mode".to_string(),
            ));
        }
    };

    let project_id = conv
        .project_id
        .as_deref()
        .ok_or_else(|| AppError::BadRequest("Conversation is not project-scoped".to_string()))?;

    let project = state
        .db
        .get_project(project_id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let repo_root = PathBuf::from(&project.canonical_path);
    let repo_root_str = repo_root.display().to_string();

    let commit_message = req.commit_message;
    let auto_stash = req.auto_stash;
    let base_branch_for_msg = base_branch.clone();

    // 2. Execute merge sequence (blocking, under global mutex)
    let merge_result = tokio::task::spawn_blocking(move || -> Result<(String, Option<String>), AppError> {
        let _guard = TASK_APPROVAL_MUTEX
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        // 2a. Repo root must be clean (or auto-stashed)
        let status = run_git(&repo_root, &["status", "--porcelain"]).map_err(AppError::Internal)?;
        let did_stash = if status.is_empty() {
            false
        } else if auto_stash {
            run_git(
                &repo_root,
                &[
                    "stash",
                    "push",
                    "--include-untracked",
                    "-m",
                    "phoenix: auto-stash before merge",
                ],
            )
            .map_err(|e| AppError::Internal(format!("Auto-stash failed: {e}")))?;
            tracing::info!("Auto-stashed dirty main checkout before merge");
            true
        } else {
            return Err(AppError::Conflict(ConflictErrorResponse::new(
                "Main checkout has uncommitted changes. Commit or stash them before completing.",
                "dirty_main_checkout",
            )));
        };

        // 2b. Checkout base branch
        if let Err(e) = run_git(&repo_root, &["checkout", &base_branch]) {
            if did_stash {
                let _ = run_git(&repo_root, &["stash", "pop"]);
            }
            return Err(AppError::Internal(format!(
                "Failed to checkout {base_branch}: {e}"
            )));
        }

        // 2c. Squash merge
        if let Err(e) = run_git(&repo_root, &["merge", "--squash", &branch_name]) {
            // Squash merges don't create MERGE_HEAD, so `merge --abort` is a no-op.
            // Use `reset --hard` to restore the checkout to a clean state.
            let _ = run_git(&repo_root, &["reset", "--hard", "HEAD"]);
            if did_stash {
                let _ = run_git(&repo_root, &["stash", "pop"]);
            }
            return Err(AppError::Internal(format!("Squash merge failed: {e}")));
        }

        // 2c½. Mark task file as done (server-side, so agents don't need to
        //       touch the main checkout). rename_status updates frontmatter
        //       and renames the file; we then `git add` both old and new paths
        //       so the change is included in the squash merge commit.
        //
        //       ORDERING INVARIANT: This runs after auto-stash (2a) and
        //       merge --squash (2c), so the working tree is clean plus the
        //       branch's changes. find_task_by_id reads the filesystem, so
        //       it sees the merged state — not any dirty files that were
        //       stashed. Do not reorder above the stash/merge steps.
        if !task_id.is_empty() {
            let tasks_dir = repo_root.join("tasks");
            match taskmd_core::tasks::find_task_by_id(&tasks_dir, &task_id) {
                Some(task) if task.status != "done" => {
                    match taskmd_core::tasks::rename_status(&tasks_dir, &task_id, "done") {
                        Ok((old_filename, new_filename)) => {
                            tracing::info!(
                                old = %old_filename,
                                new = %new_filename,
                                "Server-side task rename to done during merge"
                            );
                            // Stage both the removal of the old file and addition of the new
                            let _ = run_git(&repo_root, &["add", &format!("tasks/{old_filename}"), &format!("tasks/{new_filename}")]);
                        }
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                task_id = %task_id,
                                "Failed to rename task file to done (non-fatal)"
                            );
                        }
                    }
                }
                Some(_) => {
                    tracing::debug!(task_id = %task_id, "Task file already marked done");
                }
                None => {
                    tracing::debug!(task_id = %task_id, "No task file found (may have been deleted)");
                }
            }
        }

        // 2d. Commit (skip if merge --squash produced no changes, e.g., only task file)
        let has_staged = run_git(&repo_root, &["diff", "--cached", "--quiet"]).is_err();
        if has_staged {
            if let Err(e) = run_git(&repo_root, &["commit", "-m", &commit_message]) {
                let _ = run_git(&repo_root, &["reset", "--hard", "HEAD"]);
                if did_stash {
                    let _ = run_git(&repo_root, &["stash", "pop"]);
                }
                return Err(AppError::Internal(format!("Commit failed: {e}")));
            }
        } else {
            tracing::info!("Squash merge produced no changes (task-only branch), skipping commit");
        }

        // 2e. Record short SHA
        let short_sha = run_git(&repo_root, &["rev-parse", "--short", "HEAD"])
            .map_err(|e| AppError::Internal(format!("Failed to get commit SHA: {e}")))?;

        // 2f. Remove worktree
        let worktree_dir = PathBuf::from(&worktree_path);
        if let Err(e) = run_git(
            &repo_root,
            &["worktree", "remove", &worktree_path, "--force"],
        ) {
            tracing::warn!(
                error = %e,
                worktree = %worktree_path,
                "Failed to remove worktree (non-fatal)"
            );
            // Try filesystem removal as fallback
            let _ = std::fs::remove_dir_all(&worktree_dir);
            let _ = run_git(&repo_root, &["worktree", "prune"]);
        }

        // 2g. Delete branch
        if let Err(e) = run_git(&repo_root, &["branch", "-D", &branch_name]) {
            tracing::warn!(
                error = %e,
                branch = %branch_name,
                "Failed to delete branch (non-fatal)"
            );
        }

        // 2h. Pop auto-stash if we stashed earlier
        let stash_warning = if did_stash {
            if let Err(e) = run_git(&repo_root, &["stash", "pop"]) {
                tracing::warn!(error = %e, "Auto-stash pop failed (stash preserved)");
                Some(format!(
                    "Warning: your uncommitted changes could not be restored automatically \
                     (git stash pop failed: {e}). Run `git stash pop` manually to recover them."
                ))
            } else {
                tracing::info!("Auto-stash popped successfully after merge");
                None
            }
        } else {
            None
        };

        Ok((short_sha, stash_warning))
    })
    .await
    .map_err(|e| AppError::Internal(format!("Blocking task failed: {e}")))?;

    let (short_sha, stash_warning) = merge_result?;

    // 3. Atomically update state, mode, and cwd in a single transaction
    state
        .db
        .finalize_conversation(
            &id,
            &ConvState::Terminal,
            &ConvMode::Explore,
            &repo_root_str,
        )
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    // 4. Inject system message (include stash warning if pop failed)
    let mut system_msg =
        format!("Task completed. Squash merged to {base_branch_for_msg} as {short_sha}.");
    if let Some(ref warning) = stash_warning {
        use std::fmt::Write;
        let _ = write!(system_msg, "\n\n{warning}");
    }
    let msg_id = uuid::Uuid::new_v4().to_string();
    let msg = state
        .db
        .add_message(
            &msg_id,
            &id,
            &MessageContent::system(&system_msg),
            None,
            None,
        )
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    // 7. Broadcast SSE events so the frontend updates in real-time
    if let Ok(handle) = state.runtime.get_or_create(&id).await {
        let _ = handle.broadcast_tx.send(SseEvent::Message { message: msg });
        let _ = handle.broadcast_tx.send(SseEvent::StateChange {
            state: ConvState::Terminal,
            display_state: ConvState::Terminal.display_state().as_str().to_string(),
        });
        let _ = handle.broadcast_tx.send(SseEvent::ConversationUpdate {
            update: crate::runtime::ConversationMetadataUpdate {
                cwd: Some(repo_root_str),
                branch_name: None,
                worktree_path: None,
                conv_mode_label: Some("Explore".to_string()),
                base_branch: None,
                commits_behind: None,
                commits_ahead: None,
            },
        });
    }

    Ok(Json(ConfirmCompleteResponse {
        success: true,
        commit_sha: short_sha,
        warning: stash_warning,
    }))
}

/// Check if the task file for a given task ID has status `done`.
/// Returns true if the task file exists and its status is NOT done.
fn check_task_file_status(worktree_path: &std::path::Path, task_id: &str) -> bool {
    let tasks_dir = worktree_path.join("tasks");
    match taskmd_core::tasks::find_task_by_id(&tasks_dir, task_id) {
        Some(task) => task.status != "done",
        None => false, // No matching task file found
    }
}

/// Abandon a Work-mode task: delete worktree/branch, mark task file wont-do, go Terminal.
/// Single-phase endpoint -- the frontend confirms via a dialog before calling this.
#[allow(clippy::too_many_lines)]
async fn abandon_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<SuccessResponse>, AppError> {
    // 1. Validate conversation exists, is Work mode, Idle state, project-scoped
    let conv = state
        .runtime
        .db()
        .get_conversation(&id)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;

    if !matches!(conv.state, ConvState::Idle) {
        return Err(AppError::BadRequest(
            "Conversation must be idle to abandon a task".to_string(),
        ));
    }

    let (branch_name, worktree_path, base_branch, task_id) = match &conv.conv_mode {
        ConvMode::Work {
            branch_name,
            worktree_path,
            base_branch,
            task_id,
        } => (
            branch_name.clone(),
            worktree_path.clone(),
            base_branch.clone(),
            task_id.clone(),
        ),
        _ => {
            return Err(AppError::BadRequest(
                "Conversation is not in Work mode".to_string(),
            ));
        }
    };

    let project_id = conv
        .project_id
        .as_deref()
        .ok_or_else(|| AppError::BadRequest("Conversation is not project-scoped".to_string()))?;

    let project = state
        .db
        .get_project(project_id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let repo_root = PathBuf::from(&project.canonical_path);

    // 2. Execute abandon sequence (blocking)
    let repo_root_clone = repo_root.clone();
    tokio::task::spawn_blocking(move || -> Result<(), AppError> {
        // Phase 1: worktree cleanup (BEFORE mutex -- these don't touch the main checkout)
        let worktree_dir = PathBuf::from(&worktree_path);
        if let Err(e) = run_git(
            &repo_root_clone,
            &["worktree", "remove", &worktree_path, "--force"],
        ) {
            tracing::warn!(
                error = %e,
                worktree = %worktree_path,
                "Failed to remove worktree (non-fatal), trying filesystem fallback"
            );
            let _ = std::fs::remove_dir_all(&worktree_dir);
            let _ = run_git(&repo_root_clone, &["worktree", "prune"]);
        }

        if let Err(e) = run_git(&repo_root_clone, &["branch", "-D", &branch_name]) {
            tracing::warn!(
                error = %e,
                branch = %branch_name,
                "Failed to delete branch (non-fatal)"
            );
        }

        // Phase 2: task file update (UNDER mutex)
        let _guard = TASK_APPROVAL_MUTEX
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        // Check main checkout is clean
        let status =
            run_git(&repo_root_clone, &["status", "--porcelain"]).map_err(AppError::Internal)?;
        if !status.is_empty() {
            return Err(AppError::Conflict(ConflictErrorResponse::new(
                "Main checkout has uncommitted changes. Commit or stash them before abandoning.",
                "dirty_main_checkout",
            )));
        }

        // Checkout base branch
        run_git(&repo_root_clone, &["checkout", &base_branch])
            .map_err(|e| AppError::Internal(format!("Failed to checkout {base_branch}: {e}")))?;

        // Scan tasks/ for matching task file and rename to wont-do
        let tasks_dir = repo_root_clone.join("tasks");
        match taskmd_core::tasks::find_task_by_id(&tasks_dir, &task_id) {
            Some(task) => {
                let old_filename = task
                    .path
                    .file_name()
                    .expect("task path has filename")
                    .to_string_lossy()
                    .to_string();
                let new_filename = taskmd_core::filename::format_filename(
                    &task.id,
                    &task.priority,
                    "wont-do",
                    &task.slug,
                );

                // Update frontmatter status in-place before renaming
                if let Ok(content) = std::fs::read_to_string(&task.path) {
                    let updated = taskmd_core::tasks::update_status_in_content(&content, "wont-do");
                    if let Err(e) = std::fs::write(&task.path, updated) {
                        tracing::warn!(error = %e, "Failed to update task frontmatter (non-fatal)");
                    }
                }

                // Use git mv so the rename is tracked
                let old_path = format!("tasks/{old_filename}");
                let new_path = format!("tasks/{new_filename}");

                if let Err(e) = run_git(&repo_root_clone, &["mv", &old_path, &new_path]) {
                    tracing::warn!(
                        error = %e,
                        old = %old_path,
                        new = %new_path,
                        "Failed to git mv task file (non-fatal)"
                    );
                } else if let Err(e) = run_git(
                    &repo_root_clone,
                    &["commit", "-m", &format!("task {task_id}: mark wont-do")],
                ) {
                    tracing::warn!(
                        error = %e,
                        "Failed to commit task file rename (non-fatal)"
                    );
                    let _ = run_git(&repo_root_clone, &["reset", "HEAD"]);
                }
            }
            None => {
                tracing::warn!(
                    task_id = task_id,
                    "No task file found for task number (may have been manually deleted)"
                );
            }
        }

        Ok(())
    })
    .await
    .map_err(|e| AppError::Internal(format!("Blocking task failed: {e}")))??;

    // 3. Atomically update state, mode, and cwd in a single transaction
    let repo_root_str = repo_root.display().to_string();
    state
        .db
        .finalize_conversation(
            &id,
            &ConvState::Terminal,
            &ConvMode::Explore,
            &repo_root_str,
        )
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    // 4. Inject system message
    let msg_id = uuid::Uuid::new_v4().to_string();
    let msg = state
        .db
        .add_message(
            &msg_id,
            &id,
            &MessageContent::system("Task abandoned. Worktree and branch deleted."),
            None,
            None,
        )
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    // 7. Broadcast SSE events so the frontend updates in real-time
    if let Ok(handle) = state.runtime.get_or_create(&id).await {
        let _ = handle.broadcast_tx.send(SseEvent::Message { message: msg });
        let _ = handle.broadcast_tx.send(SseEvent::StateChange {
            state: ConvState::Terminal,
            display_state: ConvState::Terminal.display_state().as_str().to_string(),
        });
        let _ = handle.broadcast_tx.send(SseEvent::ConversationUpdate {
            update: crate::runtime::ConversationMetadataUpdate {
                cwd: Some(repo_root_str),
                branch_name: None,
                worktree_path: None,
                conv_mode_label: Some("Explore".to_string()),
                base_branch: None,
                commits_behind: None,
                commits_ahead: None,
            },
        });
    }

    Ok(Json(SuccessResponse { success: true }))
}

// ============================================================
// Lifecycle (REQ-API-006)
// ============================================================

async fn archive_conversation(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<SuccessResponse>, AppError> {
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
    state
        .runtime
        .db()
        .unarchive_conversation(&id)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;

    Ok(Json(SuccessResponse { success: true }))
}

async fn delete_conversation(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<SuccessResponse>, AppError> {
    state
        .runtime
        .db()
        .delete_conversation(&id)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;

    Ok(Json(SuccessResponse { success: true }))
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
        conversation: conversation_to_json(&conversation),
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
/// Prefers filename matches over scattered path-segment matches.
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

    // Score against filename first (much more relevant for file search)
    let filename = path.rsplit('/').next().unwrap_or(path);

    buf.clear();
    buf.extend(filename.chars());
    let haystack = nucleo_matcher::Utf32Str::Unicode(buf);

    if let Some(score) = pattern.score(haystack, matcher) {
        // Filename match: boost score significantly
        return Some(
            i32::try_from(score)
                .unwrap_or(i32::MAX)
                .saturating_add(1000),
        );
    }

    // Fall back to full path match (but with lower base score)
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

    let tasks = taskmd_core::tasks::list_tasks(&tasks_dir)
        .into_iter()
        .map(|t| TaskEntry {
            id: t.id,
            priority: t.priority,
            status: t.status,
            slug: t.slug,
        })
        .collect();

    Ok(Json(TasksResponse { tasks }))
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

    Json(ModelsResponse {
        models,
        default: state.llm_registry.default_model_id().to_string(),
        gateway_status,
        llm_configured,
    })
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

/// Check if auto-stashing the dirty main checkout would be safe (pop won't conflict after merge).
///
/// Check whether the dirty files on main overlap with files changed by the task branch.
/// If no overlap, `git stash push` + merge + `git stash pop` is safe.
fn check_auto_stash_safe(
    repo_root: &std::path::Path,
    base_branch: &str,
    task_branch: &str,
) -> bool {
    // Get the files that the task branch changed (relative to base)
    let merge_diff = run_git(
        repo_root,
        &[
            "diff",
            "--name-only",
            &format!("{base_branch}...{task_branch}"),
        ],
    )
    .unwrap_or_default();
    let merge_files: std::collections::HashSet<&str> = merge_diff.lines().collect();

    // Get the dirty files on main (tracked modified + untracked)
    let status = run_git(repo_root, &["status", "--porcelain"]).unwrap_or_default();
    let dirty_files: std::collections::HashSet<&str> = status
        .lines()
        .filter_map(|line| {
            // git status --porcelain format: "XY filename" (3-char prefix)
            let trimmed = line.get(3..)?;
            Some(trimmed.trim())
        })
        .collect();

    // If no overlap, stash pop will succeed
    let overlap: Vec<&&str> = merge_files.intersection(&dirty_files).collect();
    if overlap.is_empty() {
        true
    } else {
        tracing::debug!(
            overlap = ?overlap,
            "Auto-stash not safe: dirty files overlap with merge"
        );
        false
    }
}

// ============================================================
// Slug Generation (REQ-API-002)
// ============================================================

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
// Error Handling
// ============================================================

enum AppError {
    BadRequest(String),
    NotFound(String),
    Internal(String),
    /// 409 — conflict (dirty worktree, merge conflicts, etc.)
    Conflict(ConflictErrorResponse),
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
                (StatusCode::CONFLICT, Json(detail)).into_response()
            }
            AppError::UnprocessableEntity(ref detail) => {
                tracing::warn!(error = %detail.error, "422 Unprocessable Entity");
                (StatusCode::UNPROCESSABLE_ENTITY, Json(detail.clone())).into_response()
            }
        }
    }
}
