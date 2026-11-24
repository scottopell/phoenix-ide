use std::{sync::Arc, time::Duration};

use phoenix_core::domain::{
    llm_types::{EffectiveEffort, EffectiveServiceTier},
    tool_availability::ToolAvailability,
};
use phoenix_llm::{
    ContentBlock, LlmMessage, LlmRequest, LlmResponse, LlmService, MessageRole, ModelEffort,
    ModelRegistry, PromptCacheKey, SystemContent,
};
use serde::Serialize;
use sha2::{Digest, Sha256};

pub(super) use phoenix_db::coordinator_scanner::{
    SCAN_MAX_INPUT_BYTES as MAX_CONTENT_BYTES, SCAN_MAX_OUTPUT_BYTES as MAX_OUTPUT_BYTES,
    SCAN_MAX_OUTPUT_TOKENS as MAX_OUTPUT_TOKENS, SCAN_MODEL as MODEL,
};
pub(super) const EFFORT: ModelEffort = ModelEffort::Low;
pub(super) const TIMEOUT: Duration =
    Duration::from_secs(phoenix_db::coordinator_scanner::SCAN_TIMEOUT_SECONDS);

pub(super) fn eligible_service(registry: &ModelRegistry) -> Option<Arc<dyn LlmService>> {
    if !registry.is_builtin_model(MODEL) {
        return None;
    }
    let connection = registry.connection_for_model(MODEL)?;
    if !matches!(connection.as_str(), "codex" | "openai_responses") {
        return None;
    }
    registry
        .validate_execution_route(MODEL, &connection, Some(EFFORT))
        .ok()?;
    let service = registry.get_execution_service(MODEL, &connection)?;
    eligible_identity(service.as_ref()).then_some(service)
}

pub(super) fn eligible_identity(service: &dyn LlmService) -> bool {
    service.model_id() == MODEL
        && service.outbound_model_name() == Some(MODEL)
        && (service.uses_codex_bridge() || service.uses_official_openai_responses())
}

pub(super) fn endpoint_hash(service: &dyn LlmService) -> [u8; 32] {
    Sha256::digest(service.continuation_route_key().as_bytes()).into()
}

pub(super) struct BoundedRequest {
    request: LlmRequest,
    content_bytes: usize,
}

impl BoundedRequest {
    pub(super) fn new(system: &str, input: &impl Serialize) -> Result<Self, String> {
        let input = serde_json::to_string(input)
            .map_err(|error| format!("scanner input serialization failed: {error}"))?;
        let content_bytes = system.len().saturating_add(input.len());
        if content_bytes > MAX_CONTENT_BYTES {
            return Err(format!(
                "scanner inference content exceeds {MAX_CONTENT_BYTES} bytes: {content_bytes}"
            ));
        }
        Ok(Self {
            request: LlmRequest {
                system: vec![SystemContent::new(system)],
                messages: vec![LlmMessage {
                    source_message_id: None,
                    role: MessageRole::User,
                    content: vec![ContentBlock::text(input)],
                }],
                provider_replay: None,
                output_replay: vec![],
                tool_availability: ToolAvailability::all(vec![]),
                max_tokens: MAX_OUTPUT_TOKENS,
                effective_effort: EffectiveEffort::explicit(EFFORT),
                service_tier: EffectiveServiceTier::Standard,
                telemetry: None,
                cache_key: PromptCacheKey::stable("coordinator-scanner-v1"),
            },
            content_bytes,
        })
    }

    pub(super) fn request(&self) -> &LlmRequest {
        &self.request
    }

    pub(super) fn content_bytes(&self) -> usize {
        self.content_bytes
    }
}

pub(super) fn complete_text(response: &LlmResponse) -> Result<String, &'static str> {
    if !response.end_turn {
        return Err("scanner output is incomplete or truncated");
    }
    if !response.usage_observation.output {
        return Err("scanner output token usage is unavailable");
    }
    if response.usage.output_tokens > u64::from(MAX_OUTPUT_TOKENS) {
        return Err("scanner output exceeds the accepted reported-token bound");
    }
    let mut text = String::new();
    for block in &response.content {
        match block {
            ContentBlock::Text { text: fragment } => {
                if text.len().saturating_add(fragment.len()) > MAX_OUTPUT_BYTES {
                    return Err("scanner output exceeds the accepted UTF8-byte bound");
                }
                text.push_str(fragment);
            }
            ContentBlock::Image { .. }
            | ContentBlock::ToolUse { .. }
            | ContentBlock::ToolResult { .. }
            | ContentBlock::ServerToolUse { .. }
            | ContentBlock::ToolSearchToolResult { .. }
            | ContentBlock::WebSearchToolResult { .. }
            | ContentBlock::WebFetchToolResult { .. }
            | ContentBlock::CodeExecutionToolResult { .. }
            | ContentBlock::BashCodeExecutionToolResult { .. }
            | ContentBlock::TextEditorCodeExecutionToolResult { .. }
            | ContentBlock::McpToolUse { .. }
            | ContentBlock::McpToolResult { .. } => {
                return Err("scanner output contains a non-text or tool block");
            }
        }
    }
    if text.trim().is_empty() {
        return Err("scanner output contains no complete result");
    }
    Ok(text)
}
