//! API request and response types

use serde::{Deserialize, Serialize};

/// Request to create a new conversation with initial message
#[derive(Debug, Deserialize)]
pub struct CreateConversationRequest {
    pub cwd: String,
    pub model: Option<String>,
    /// Initial message text (required)
    pub text: String,
    /// Client-generated message ID for idempotency
    pub message_id: String,
    /// Optional image attachments
    #[serde(default)]
    pub images: Vec<ImageAttachment>,
}

/// Request to send a chat message
#[derive(Debug, Deserialize)]
pub struct ChatRequest {
    pub text: String,
    /// Client-generated UUID - the canonical identifier for this message
    /// Enables idempotent retries (sending same message_id twice = no duplicate)
    pub message_id: String,
    #[serde(default)]
    pub images: Vec<ImageAttachment>,
    /// Browser user agent for display (e.g., show iPhone icon)
    #[serde(default)]
    pub user_agent: Option<String>,
}

/// Image attachment in a chat message
#[derive(Debug, Clone, Deserialize)]
pub struct ImageAttachment {
    pub data: String,
    pub media_type: String,
}

/// Request to rename a conversation
#[derive(Debug, Deserialize)]
pub struct RenameRequest {
    pub name: String,
}

/// Response with a list of conversations
#[derive(Debug, Serialize)]
pub struct ConversationListResponse {
    pub conversations: Vec<serde_json::Value>,
}

/// Response with a single conversation
#[derive(Debug, Serialize)]
pub struct ConversationResponse {
    pub conversation: serde_json::Value,
}

/// Response with conversation and messages
#[derive(Debug, Serialize)]
pub struct ConversationWithMessagesResponse {
    pub conversation: serde_json::Value,
    pub messages: Vec<serde_json::Value>,
    pub agent_working: bool,
    pub context_window_size: u64,
}

/// Response for chat action
#[derive(Debug, Serialize)]
pub struct ChatResponse {
    pub queued: bool,
}

/// Response for cancel action
#[derive(Debug, Serialize)]
pub struct CancelResponse {
    pub ok: bool,
}

/// Response for lifecycle actions
#[derive(Debug, Serialize)]
pub struct SuccessResponse {
    pub success: bool,
}

/// Response for directory validation
#[derive(Debug, Serialize)]
pub struct ValidateCwdResponse {
    pub valid: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Response for directory listing
#[derive(Debug, Serialize)]
pub struct ListDirectoryResponse {
    pub entries: Vec<DirectoryEntry>,
}

/// Response for mkdir
#[derive(Debug, Serialize)]
pub struct MkdirResponse {
    pub created: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Directory entry
#[derive(Debug, Serialize)]
pub struct DirectoryEntry {
    pub name: String,
    pub is_dir: bool,
}

/// Enhanced file entry for file browser (REQ-PF-001 through REQ-PF-004)
#[derive(Debug, Serialize)]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub is_directory: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified_time: Option<u64>, // Unix timestamp in seconds
    pub file_type: String, // folder, markdown, code, config, text, image, data, unknown
    pub is_text_file: bool,
}

/// Response for file listing
#[derive(Debug, Serialize)]
pub struct ListFilesResponse {
    pub items: Vec<FileEntry>,
}

/// Response for file reading
#[derive(Debug, Serialize)]
pub struct ReadFileResponse {
    pub content: String,
    pub encoding: String,
}

/// Error response for file operations
#[derive(Debug, Serialize)]
pub struct FileErrorResponse {
    pub error: String,
    pub is_binary: bool,
}

/// Model information with metadata
#[derive(Debug, Serialize)]
pub struct ModelInfo {
    pub id: String,
    pub provider: String,
    pub description: String,
    pub context_window: usize,
}

/// Response for model list
#[derive(Debug, Serialize)]
pub struct ModelsResponse {
    pub models: Vec<ModelInfo>,
    pub default: String,
}

/// Error response
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
}

impl ErrorResponse {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            error: message.into(),
        }
    }
}
