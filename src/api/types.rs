//! API request and response types

use serde::{Deserialize, Serialize};

/// Request to create a new conversation
#[derive(Debug, Deserialize)]
pub struct CreateConversationRequest {
    pub cwd: String,
    #[allow(dead_code)] // Reserved for model selection
    pub model: Option<String>,
}

/// Request to send a chat message
#[derive(Debug, Deserialize)]
pub struct ChatRequest {
    pub text: String,
    #[serde(default)]
    pub images: Vec<ImageAttachment>,
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

/// Directory entry
#[derive(Debug, Serialize)]
pub struct DirectoryEntry {
    pub name: String,
    pub is_dir: bool,
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
