//! HTTP request handlers
//!
//! REQ-API-001 through REQ-API-010

use super::sse::sse_stream;
use super::types::{
    CancelResponse, ChatRequest, ChatResponse, ConversationListResponse, ConversationResponse,
    ConversationWithMessagesResponse, CreateConversationRequest, DirectoryEntry, ErrorResponse,
    ListDirectoryResponse, MkdirResponse, ModelsResponse, RenameRequest, SuccessResponse,
    ValidateCwdResponse,
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

    // Generate ID and slug
    let id = uuid::Uuid::new_v4().to_string();
    let slug = generate_slug();

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
    // Convert images
    let images: Vec<ImageData> = req
        .images
        .into_iter()
        .map(|img| ImageData {
            data: img.data,
            media_type: img.media_type,
        })
        .collect();

    // Send event to runtime
    let event = Event::UserMessage {
        text: req.text,
        images,
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
    let path = PathBuf::from(&query.path);

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
    let path = PathBuf::from(&query.path);

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
    let path = PathBuf::from(&payload.path);

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
