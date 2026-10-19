//! Conversation title generation using a fast/cheap LLM
//!
//! Generates short, meaningful titles based on the initial user message.

use phoenix_llm::{
    ContentBlock, LlmMessage, LlmRequest, LlmResponse, LlmService, MessageRole, PromptCacheKey,
};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;

use phoenix_core::llm_language::TITLE_PROMPT;

const TITLE_TIMEOUT: Duration = Duration::from_secs(5);

/// Generate a title for a conversation based on the initial message.
///
/// Returns None if title generation fails (timeout, error, etc.)
/// The caller supplies the fallback name on failure.
pub async fn generate_title(
    message_text: &str,
    llm_service: Arc<dyn LlmService>,
    effective_effort: phoenix_core::domain::llm_types::EffectiveEffort,
    max_output_tokens: Option<u32>,
) -> Option<String> {
    // Truncate very long messages for the prompt
    let truncated = if message_text.len() > 500 {
        format!("{}...", message_text.get(..500).unwrap_or(message_text))
    } else {
        message_text.to_string()
    };

    let prompt = format!("{TITLE_PROMPT}\n{truncated}");

    let request = LlmRequest {
        system: vec![],
        messages: vec![LlmMessage {
            source_message_id: None,
            role: MessageRole::User,
            content: vec![ContentBlock::text(prompt)],
        }],
        provider_replay: None,
        responses_replay: Vec::new(),
        tool_availability: phoenix_core::domain::tool_availability::ToolAvailability::all(vec![]),
        max_tokens: Some(max_output_tokens.map_or(50, |limit| limit.min(50))), // Title should be very short
        effective_effort,
        service_tier: phoenix_core::domain::llm_types::EffectiveServiceTier::Standard,
        telemetry: None,
        // Shared by every title-generation call so TITLE_PROMPT caches.
        cache_key: PromptCacheKey::stable("title-generator"),
    };

    // Apply timeout
    let result = timeout(TITLE_TIMEOUT, llm_service.complete(&request)).await;

    match result {
        Ok(Ok(response)) => {
            // Extract text from response
            let title = extract_title_from_response(&response);
            title
                .map(|t| clean_prose_name(&t))
                .filter(|name| !name.is_empty())
        }
        Ok(Err(e)) => {
            tracing::warn!("Title generation LLM error: {}", e.message);
            None
        }
        Err(_) => {
            tracing::warn!("Title generation timed out");
            None
        }
    }
}

fn clean_prose_name(name: &str) -> String {
    name.split_whitespace()
        .map(|word| word.chars().filter(|c| !c.is_control()).collect::<String>())
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Extract the title text from the LLM response
fn extract_title_from_response(response: &LlmResponse) -> Option<String> {
    for block in &response.content {
        if let ContentBlock::Text { text } = block {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::clean_prose_name;

    #[test]
    fn generated_names_preserve_prose_and_unicode() {
        assert_eq!(
            clean_prose_name("  Fix Über 東京 Auth  "),
            "Fix Über 東京 Auth"
        );
        assert_eq!(
            clean_prose_name("Auth\nRefactor\tAnd Tests"),
            "Auth Refactor And Tests"
        );
        assert_eq!(clean_prose_name("Auth\u{0007}Refactor"), "AuthRefactor");
        assert_eq!(clean_prose_name("\u{0007}"), "");
        assert!(clean_prose_name(&"Word ".repeat(100)).chars().count() > 200);
    }
}
