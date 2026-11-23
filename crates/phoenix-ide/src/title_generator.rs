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
    let prompt = title_prompt(message_text);

    let request = LlmRequest {
        system: vec![],
        messages: vec![LlmMessage {
            source_message_id: None,
            role: MessageRole::User,
            content: vec![ContentBlock::text(prompt)],
        }],
        provider_replay: None,
        output_replay: Vec::new(),
        tool_availability: phoenix_core::domain::tool_availability::ToolAvailability::all(vec![]),
        max_tokens: max_output_tokens.map_or(50, |limit| limit.min(50)), // Title should be very short
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

/// Longest excerpt of the opening message sent to the title model.
const TITLE_EXCERPT_BYTES: usize = 500;

fn title_prompt(message_text: &str) -> String {
    let excerpt =
        phoenix_core::text_bounds::truncate_with_marker(message_text, TITLE_EXCERPT_BYTES, "...");
    format!("{TITLE_PROMPT}\n{excerpt}")
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
    use super::{clean_prose_name, title_prompt, TITLE_EXCERPT_BYTES};
    use phoenix_core::llm_language::TITLE_PROMPT;

    #[test]
    fn title_prompt_bounds_multibyte_openings() {
        // Byte 500 falls inside a 3-byte char; the excerpt must still be cut
        // rather than the whole message being sent to the title model.
        let opening = "日本語".repeat(70_000);
        let prompt = title_prompt(&opening);
        let excerpt = prompt.strip_prefix(&format!("{TITLE_PROMPT}\n")).unwrap();
        assert!(
            excerpt.len() <= TITLE_EXCERPT_BYTES + "...".len(),
            "{}",
            excerpt.len()
        );
        assert!(excerpt.ends_with("..."));
        assert!(opening.starts_with(excerpt.strip_suffix("...").unwrap()));
    }

    #[test]
    fn title_prompt_keeps_ascii_excerpt_unchanged() {
        let opening = "a".repeat(600);
        assert_eq!(
            title_prompt(&opening),
            format!("{TITLE_PROMPT}\n{}...", "a".repeat(500))
        );
        assert_eq!(title_prompt("short"), format!("{TITLE_PROMPT}\nshort"));
    }

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
