use crate::api::global_read::{GlobalReadService, OpenWorkFilter};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;

use crate::send_chat_service::{SendChatApplicationService, SendChatRequest, SendChatServiceError};
use crate::tools::{Tool, ToolContext, ToolOutput};

pub(crate) fn tools(
    service: GlobalReadService,
    send_chat: Arc<SendChatApplicationService>,
) -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(SearchConversations(service.clone())),
        Arc::new(ReadConversation(service.clone())),
        Arc::new(ListOpenWork(service.clone())),
        Arc::new(ResolveReference(service.clone())),
        Arc::new(SendConversationMessage { service, send_chat }),
    ]
}

struct SearchConversations(GlobalReadService);
struct ReadConversation(GlobalReadService);
struct ListOpenWork(GlobalReadService);
struct ResolveReference(GlobalReadService);
struct SendConversationMessage {
    service: GlobalReadService,
    send_chat: Arc<SendChatApplicationService>,
}

#[derive(Debug, Deserialize)]
struct SendConversationMessageInput {
    target: String,
    message: String,
    message_id: String,
}

#[derive(Debug, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
enum SendConversationMessageOutput {
    Delivered {
        target: String,
        conversation_id: String,
        message_id: String,
    },
    QueuedAsSteering {
        target: String,
        conversation_id: String,
        message_id: String,
    },
    Rejected {
        target: Option<String>,
        conversation_id: Option<String>,
        message_id: String,
        reason_code: &'static str,
        message: String,
    },
}

#[async_trait]
impl Tool for SearchConversations {
    fn name(&self) -> &'static str {
        "search_conversations"
    }
    fn description(&self) -> String {
        "Search all Phoenix conversation messages. Results include stable conversation/message references and app-local citation links.".to_string()
    }
    fn input_schema(&self) -> Value {
        json!({"type":"object","properties":{"query":{"type":"string"}},"required":["query"]})
    }
    fn clearable(&self) -> bool {
        true
    }
    async fn run(&self, input: Value, _ctx: ToolContext) -> ToolOutput {
        let query = input.get("query").and_then(Value::as_str).unwrap_or("");
        result(self.0.search(query).await)
    }
}

#[async_trait]
impl Tool for ReadConversation {
    fn name(&self) -> &'static str {
        "read_conversation"
    }
    fn description(&self) -> String {
        "Read one source conversation transcript in bounded pages. Pass a conversation id, @conv reference, or app-local conversation link. Use cursor when the result says more content is available.".to_string()
    }
    fn input_schema(&self) -> Value {
        json!({"type":"object","properties":{"conversation_id":{"type":"string"},"cursor":{"type":"integer","minimum":0}},"required":["conversation_id"]})
    }
    fn clearable(&self) -> bool {
        true
    }
    async fn run(&self, input: Value, _ctx: ToolContext) -> ToolOutput {
        let conversation = input
            .get("conversation_id")
            .and_then(Value::as_str)
            .unwrap_or("");
        let cursor = input
            .get("cursor")
            .and_then(Value::as_u64)
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or(0);
        result(self.0.read_conversation(conversation, cursor).await)
    }
}

#[async_trait]
impl Tool for ListOpenWork {
    fn name(&self) -> &'static str {
        "list_open_work"
    }
    fn description(&self) -> String {
        "Read the deterministic current fleet/open-work projection grouped by project, including stable references and explainable inclusion signals.".to_string()
    }
    fn input_schema(&self) -> Value {
        json!({"type":"object","properties":{"offset":{"type":"integer","minimum":0},"query":{"type":"string"}}})
    }
    fn clearable(&self) -> bool {
        true
    }
    async fn run(&self, input: Value, _ctx: ToolContext) -> ToolOutput {
        let offset = input
            .get("offset")
            .and_then(Value::as_u64)
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or(0);
        let filter = OpenWorkFilter::from_query(input.get("query").and_then(Value::as_str));
        result(self.0.open_work_page(offset, &filter).await)
    }
}

#[async_trait]
impl Tool for ResolveReference {
    fn name(&self) -> &'static str {
        "resolve_reference"
    }
    fn description(&self) -> String {
        "Resolve @conv, @chain, @work, and app-local conversation/chain references to durable source metadata.".to_string()
    }
    fn input_schema(&self) -> Value {
        json!({"type":"object","properties":{"reference":{"type":"string"}},"required":["reference"]})
    }
    fn clearable(&self) -> bool {
        true
    }
    async fn run(&self, input: Value, _ctx: ToolContext) -> ToolOutput {
        let reference = input.get("reference").and_then(Value::as_str).unwrap_or("");
        match self.0.resolve_reference(reference).await {
            Ok(value) => match serde_json::to_string_pretty(&value) {
                Ok(value) => ToolOutput::success(value),
                Err(error) => ToolOutput::error(format!("failed to encode reference: {error}")),
            },
            Err(error) => ToolOutput::error(format!("{error:?}")),
        }
    }
}

#[async_trait]
impl Tool for SendConversationMessage {
    fn name(&self) -> &'static str {
        "send_conversation_message"
    }

    fn description(&self) -> String {
        "Coordinator-only delivery tool. Send one user message to another conversation by durable target reference (@work, @conv, app-local link, or conversation id). Never target the Coordinator chain itself.".to_string()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "target": { "type": "string", "minLength": 1 },
                "message": { "type": "string", "minLength": 1 },
                "message_id": { "type": "string", "format": "uuid" }
            },
            "required": ["target", "message", "message_id"]
        })
    }

    async fn run(&self, input: Value, ctx: ToolContext) -> ToolOutput {
        let parsed = match serde_json::from_value::<SendConversationMessageInput>(input) {
            Ok(value) => value,
            Err(error) => return ToolOutput::error(format!("invalid input: {error}")),
        };
        if parsed.message.trim().is_empty() || uuid::Uuid::parse_str(&parsed.message_id).is_err() {
            return ToolOutput::error(
                "message must be non-empty and message_id must be a UUID".to_string(),
            );
        }
        let target = match self.service.resolve_message_target(&parsed.target).await {
            Ok(value) => value,
            Err(error) => {
                let output = SendConversationMessageOutput::Rejected {
                    target: Some(parsed.target),
                    conversation_id: None,
                    message_id: parsed.message_id,
                    reason_code: error.stable_code(),
                    message: error.to_string(),
                };
                return encode_message_output(&output);
            }
        };
        let conversation_id = target.conversation_id;
        let request = SendChatRequest {
            conversation_id: conversation_id.clone(),
            text: parsed.message,
            message_id: parsed.message_id.clone(),
            images: Vec::new(),
            files: Vec::new(),
            user_agent: None,
        };
        let output = match self.send_chat.send(request).await {
            Ok(crate::send_chat_service::SendChatOutcome::Delivered) => {
                SendConversationMessageOutput::Delivered {
                    target: parsed.target,
                    conversation_id: conversation_id.clone(),
                    message_id: parsed.message_id.clone(),
                }
            }
            Ok(crate::send_chat_service::SendChatOutcome::QueuedAsSteering) => {
                SendConversationMessageOutput::QueuedAsSteering {
                    target: parsed.target,
                    conversation_id: conversation_id.clone(),
                    message_id: parsed.message_id.clone(),
                }
            }
            Ok(crate::send_chat_service::SendChatOutcome::Rejected { message, code }) => {
                SendConversationMessageOutput::Rejected {
                    target: Some(parsed.target),
                    conversation_id: Some(conversation_id.clone()),
                    message_id: parsed.message_id.clone(),
                    reason_code: code,
                    message,
                }
            }
            Err(error) => SendConversationMessageOutput::Rejected {
                target: Some(parsed.target),
                conversation_id: Some(conversation_id.clone()),
                message_id: parsed.message_id.clone(),
                reason_code: service_error_code(&error),
                message: error.to_string(),
            },
        };
        tracing::info!(
            origin_coordinator_id = %ctx.conversation_id,
            resolved_target_id = %conversation_id,
            message_id = %parsed.message_id,
            outcome = output.kind(),
            "Coordinator message action committed"
        );
        encode_message_output(&output)
    }
}

impl SendConversationMessageOutput {
    fn kind(&self) -> &'static str {
        match self {
            Self::Delivered { .. } => "delivered",
            Self::QueuedAsSteering { .. } => "queued_as_steering",
            Self::Rejected { .. } => "rejected",
        }
    }
}

fn encode_message_output(output: &SendConversationMessageOutput) -> ToolOutput {
    match serde_json::to_string_pretty(&output) {
        Ok(body) => ToolOutput::success(body),
        Err(error) => ToolOutput::error(format!("failed to encode output: {error}")),
    }
}

fn service_error_code(error: &SendChatServiceError) -> &'static str {
    match error {
        SendChatServiceError::NotFound(_) => "target_not_found",
        SendChatServiceError::AttachmentValidation(_) => "attachment_validation_failed",
        SendChatServiceError::Expansion { .. } => "message_expansion_failed",
        SendChatServiceError::Internal(_) => "internal_error",
        SendChatServiceError::Dispatch(_) => "dispatch_failed",
        SendChatServiceError::IdempotencyConflict => "idempotency_conflict",
    }
}

fn result(value: Result<String, String>) -> ToolOutput {
    match value {
        Ok(value) => ToolOutput::success(value),
        Err(error) => ToolOutput::error(error),
    }
}
