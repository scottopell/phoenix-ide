//! HTTP request handlers
//!
//! REQ-API-001 through REQ-API-010

use super::assets::{get_index_html, serve_favicon, serve_service_worker, serve_static};
use super::sse::sse_stream;
use super::types::{
    CancelResponse, ChatRequest, ChatResponse, ConversationListResponse, ConversationResponse,
    ConversationWithMessagesResponse, CreateConversationRequest, DirectoryEntry, ErrorResponse,
    FileEntry, ListDirectoryResponse, ListFilesResponse, MkdirResponse, ModelsResponse,
    ReadFileResponse, RenameRequest, SuccessResponse, SystemPromptResponse, ValidateCwdResponse,
};
use super::AppState;
use crate::db::{ImageData, Message, MessageContent, MessageType};
use crate::llm::ContentBlock;
use crate::runtime::SseEvent;
use crate::state_machine::Event;

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
use serde::Serialize;
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
        // Model info (REQ-API-009)
        .route("/api/models", get(list_models))
        // Environment info
        .route("/api/env", get(get_env))
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
fn enrich_message_for_api(msg: &Message) -> Value {
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

/// Merge pre-computed `display_data` into content blocks.
///
/// `display_data` format: `{ "bash": [{ "tool_use_id": "...", "display": "..." }] }`
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
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let json_convs: Vec<Value> = conversations
        .into_iter()
        .map(|c| serde_json::to_value(c).unwrap_or(Value::Null))
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
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let json_convs: Vec<Value> = conversations
        .into_iter()
        .map(|c| serde_json::to_value(c).unwrap_or(Value::Null))
        .collect();

    Ok(Json(ConversationListResponse {
        conversations: json_convs,
    }))
}

// ============================================================
// Conversation Creation (REQ-API-002)
// ============================================================

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

    // Idempotency check: if message_id already exists, find and return that conversation
    if state.db.message_exists(&req.message_id).unwrap_or(false) {
        tracing::info!(
            message_id = %req.message_id,
            "Duplicate create request detected, returning existing conversation"
        );
        // Find the conversation for this message
        if let Ok(msg) = state.db.get_message_by_id(&req.message_id) {
            if let Ok(conv) = state.runtime.db().get_conversation(&msg.conversation_id) {
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

    // Create conversation
    let conversation = state
        .runtime
        .db()
        .create_conversation(
            &id,
            &slug,
            &req.cwd,
            true,                 // user_initiated
            None,                 // no parent
            req.model.as_deref(), // selected model
        )
        .map_err(|e| AppError::Internal(e.to_string()))?;

    // Convert images
    let images: Vec<ImageData> = req
        .images
        .into_iter()
        .map(|img| ImageData {
            data: img.data,
            media_type: img.media_type,
        })
        .collect();

    // Send the initial message to the runtime
    let event = Event::UserMessage {
        text: req.text,
        images,
        message_id: req.message_id,
        user_agent: None,
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
        .map_err(|e| AppError::NotFound(e.to_string()))?;

    let messages = if let Some(after) = query.after_sequence {
        state.runtime.db().get_messages_after(&id, after)
    } else {
        state.runtime.db().get_messages(&id)
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
        conversation: serde_json::to_value(&conversation).unwrap_or(Value::Null),
        messages: json_msgs,
        agent_working: conversation.is_agent_working(),
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
        .map_err(|e| AppError::NotFound(e.to_string()))?;

    let cwd = std::path::PathBuf::from(&conversation.cwd);
    let system_prompt = crate::system_prompt::build_system_prompt(&cwd, false);

    Ok(Json(SystemPromptResponse { system_prompt }))
}

// ============================================================
// SSE Streaming (REQ-API-005)
// ============================================================

#[derive(Debug, Deserialize)]
struct StreamQuery {
    after: Option<i64>,
}

/// Breadcrumb for showing LLM thought process trail
#[derive(Debug, Clone, Serialize)]
pub struct Breadcrumb {
    #[serde(rename = "type")]
    pub crumb_type: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sequence_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
}

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
        format!("{}…", &trimmed[..max_len - 1])
    }
}

async fn stream_conversation(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<StreamQuery>,
) -> Result<impl IntoResponse, AppError> {
    let conversation = state
        .runtime
        .db()
        .get_conversation(&id)
        .map_err(|e| AppError::NotFound(e.to_string()))?;

    // Get messages (filtered by after if provided)
    let messages = if let Some(after) = query.after {
        state.runtime.db().get_messages_after(&id, after)
    } else {
        state.runtime.db().get_messages(&id)
    }
    .map_err(|e| AppError::Internal(e.to_string()))?;

    let last_sequence_id = state.runtime.db().get_last_sequence_id(&id).unwrap_or(0);

    let context_window_size = messages
        .iter()
        .filter_map(|m| m.usage_data.as_ref())
        .next_back()
        .map_or(0, crate::db::UsageData::context_window_used);

    let json_msgs: Vec<Value> = messages.iter().map(enrich_message_for_api).collect();

    // Extract breadcrumbs from the last turn
    let breadcrumbs = extract_breadcrumbs(&messages);
    let json_breadcrumbs: Vec<Value> = breadcrumbs
        .iter()
        .map(|b| serde_json::to_value(b).unwrap_or(Value::Null))
        .collect();

    // Subscribe to updates
    let broadcast_rx = state
        .runtime
        .subscribe(&id)
        .await
        .map_err(AppError::Internal)?;

    // Get model's context window for percentage calculation
    let model_id = conversation
        .model
        .as_deref()
        .unwrap_or(state.llm_registry.default_model_id());
    let model_context_window = state.llm_registry.context_window(model_id);

    // Create init event
    // Note: messages are enriched with display info above via enrich_message_for_api
    // which handles backwards compatibility for older messages without display field
    let init_event = SseEvent::Init {
        conversation: serde_json::to_value(&conversation).unwrap_or(Value::Null),
        messages: json_msgs,
        agent_working: conversation.is_agent_working(),
        last_sequence_id,
        context_window_size,
        model_context_window,
        breadcrumbs: json_breadcrumbs,
    };

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
    if state.db.message_exists(&req.message_id).unwrap_or(false) {
        tracing::info!(
            conversation_id = %id,
            message_id = %req.message_id,
            "Duplicate message detected, returning success (idempotent)"
        );
        return Ok(Json(ChatResponse { queued: true }));
    }

    // Convert images
    let images: Vec<ImageData> = req
        .images
        .into_iter()
        .map(|img| ImageData {
            data: img.data,
            media_type: img.media_type,
        })
        .collect();

    // Send event to runtime with message_id and user_agent
    let event = Event::UserMessage {
        text: req.text,
        images,
        message_id: req.message_id,
        user_agent: req.user_agent,
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
        .map_err(|e| AppError::NotFound(e.to_string()))?;

    let messages = state
        .runtime
        .db()
        .get_messages(&conversation.id)
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let json_msgs: Vec<Value> = messages.iter().map(enrich_message_for_api).collect();

    let context_window_size = messages
        .iter()
        .filter_map(|m| m.usage_data.as_ref())
        .next_back()
        .map_or(0, crate::db::UsageData::context_window_used);

    Ok(Json(ConversationWithMessagesResponse {
        conversation: serde_json::to_value(&conversation).unwrap_or(Value::Null),
        messages: json_msgs,
        agent_working: conversation.is_agent_working(),
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
        });
    }

    if !path.is_dir() {
        return Json(ValidateCwdResponse {
            valid: false,
            error: Some("Path is not a directory".to_string()),
        });
    }

    Json(ValidateCwdResponse {
        valid: true,
        error: None,
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
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
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

            FileEntry {
                name,
                path: full_path,
                is_directory,
                size,
                modified_time,
                file_type,
                is_text_file,
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

// ============================================================
// Model Info (REQ-API-009)
// ============================================================

async fn list_models(State(state): State<AppState>) -> Json<ModelsResponse> {
    // Get model metadata from registry
    let models = state.llm_registry.available_model_info();

    Json(ModelsResponse {
        models,
        default: state.llm_registry.default_model_id().to_string(),
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
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            AppError::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
            AppError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        };

        let body = Json(ErrorResponse::new(message));
        (status, body).into_response()
    }
}
