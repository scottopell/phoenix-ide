//! HTTP request handlers
//!
//! REQ-API-001 through REQ-API-010

use super::sse::sse_stream;
use super::types::{
    CancelResponse, ChatRequest, ChatResponse, ConversationListResponse, ConversationResponse,
    ConversationWithMessagesResponse, CreateConversationRequest, DirectoryEntry, ErrorResponse,
    FileEntry, ListDirectoryResponse, ListFilesResponse, MkdirResponse, ModelsResponse,
    ReadFileResponse, RenameRequest, SuccessResponse, ValidateCwdResponse,
};
use super::AppState;
use crate::runtime::SseEvent;
use crate::db::ImageData;
use crate::state_machine::Event;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response, Html},
    routing::{get, post},
    Json, Router,
};
use super::assets::{get_index_html, serve_static, serve_service_worker};
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
        // Lifecycle (REQ-API-006)
        .route("/api/conversations/:id/archive", post(archive_conversation))
        .route(
            "/api/conversations/:id/unarchive",
            post(unarchive_conversation),
        )
        .route("/api/conversations/:id/delete", post(delete_conversation))
        .route("/api/conversations/:id/rename", post(rename_conversation))
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
        // Version
        .route("/version", get(get_version))
        .with_state(state)
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
        return Err(AppError::BadRequest("Message text cannot be empty".to_string()));
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
            &id, &slug, &req.cwd, true, // user_initiated
            None, // no parent
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
        .map_err(|e| AppError::Internal(e.to_string()))?;

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

    let json_msgs: Vec<Value> = messages
        .iter()
        .map(|m| serde_json::to_value(m).unwrap_or(Value::Null))
        .collect();

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

// ============================================================
// SSE Streaming (REQ-API-005)
// ============================================================

#[derive(Debug, Deserialize)]
struct StreamQuery {
    after: Option<i64>,
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

    let json_msgs: Vec<Value> = messages
        .iter()
        .map(|m| serde_json::to_value(m).unwrap_or(Value::Null))
        .collect();

    // Subscribe to updates
    let broadcast_rx = state
        .runtime
        .subscribe(&id)
        .await
        .map_err(AppError::Internal)?;

    // Create init event
    let init_event = SseEvent::Init {
        conversation: serde_json::to_value(&conversation).unwrap_or(Value::Null),
        messages: json_msgs,
        agent_working: conversation.is_agent_working(),
        last_sequence_id,
        context_window_size,
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

    let json_msgs: Vec<Value> = messages
        .iter()
        .map(|m| serde_json::to_value(m).unwrap_or(Value::Null))
        .collect();

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

    // Don't allow creating directories outside of /home or /tmp
    let path_str = path.to_string_lossy();
    if !path_str.starts_with("/home/") && !path_str.starts_with("/tmp/") {
        return Json(MkdirResponse {
            created: false,
            error: Some("Can only create directories under /home or /tmp".to_string()),
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
        .map(|e| e.to_lowercase());

    match ext.as_deref() {
        // Markdown
        Some("md") | Some("markdown") => ("markdown".to_string(), true),
        // Code files
        Some("rs") | Some("ts") | Some("tsx") | Some("js") | Some("jsx") | Some("py")
        | Some("go") | Some("java") | Some("cpp") | Some("c") | Some("h") | Some("hpp")
        | Some("css") | Some("html") | Some("htm") | Some("vue") | Some("svelte")
        | Some("php") | Some("rb") | Some("swift") | Some("kt") | Some("scala")
        | Some("sh") | Some("bash") | Some("zsh") | Some("fish") | Some("ps1")
        | Some("sql") | Some("graphql") | Some("proto") => ("code".to_string(), true),
        // Config files
        Some("json") | Some("yaml") | Some("yml") | Some("toml") | Some("ini")
        | Some("env") | Some("conf") | Some("cfg") | Some("xml") | Some("properties") => {
            ("config".to_string(), true)
        }
        // Text files
        Some("txt") | Some("log") | Some("csv") | Some("tsv") | Some("rtf") => {
            ("text".to_string(), true)
        }
        // Image files
        Some("png") | Some("jpg") | Some("jpeg") | Some("gif") | Some("svg") | Some("webp")
        | Some("ico") | Some("bmp") | Some("tiff") | Some("tif") => {
            ("image".to_string(), false)
        }
        // Data/binary files
        Some("db") | Some("sqlite") | Some("sqlite3") | Some("bin") | Some("dat")
        | Some("exe") | Some("dll") | Some("so") | Some("dylib") | Some("o") | Some("a")
        | Some("wasm") | Some("class") | Some("jar") | Some("war") | Some("pyc")
        | Some("pyo") | Some("pdf") | Some("doc") | Some("docx") | Some("xls")
        | Some("xlsx") | Some("ppt") | Some("pptx") | Some("zip") | Some("tar")
        | Some("gz") | Some("bz2") | Some("xz") | Some("7z") | Some("rar")
        | Some("mp3") | Some("mp4") | Some("wav") | Some("avi") | Some("mkv")
        | Some("mov") | Some("webm") | Some("flac") | Some("ogg") => {
            ("data".to_string(), false)
        }
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

            let is_directory = metadata
                .as_ref()
                .map(|m| m.is_dir())
                .unwrap_or(false);

            let (file_type, is_text_file) = if is_directory {
                ("folder".to_string(), false)
            } else {
                detect_file_type(&entry_path)
            };

            let size = if is_directory {
                None
            } else {
                metadata.as_ref().map(|m| m.len())
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
    items.sort_by(|a, b| {
        match (a.is_directory, b.is_directory) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        }
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
    let content = fs::read(&path)
        .map_err(|e| AppError::BadRequest(format!("Cannot read file: {e}")))?;

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
