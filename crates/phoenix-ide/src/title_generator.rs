//! Conversation title generation using a fast/cheap LLM
//!
//! Generates short, meaningful titles based on the initial user message.

use phoenix_llm::{
    ContentBlock, LlmMessage, LlmRequest, LlmResponse, LlmService, MessageRole, PromptCacheKey,
};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;

use phoenix_core::llm_language::{CHAIN_NAME_PROMPT, TITLE_PROMPT};

const TITLE_TIMEOUT: Duration = Duration::from_secs(5);

/// Per-message input cap for chain-name generation — mirrors `generate_title`'s
/// 500-char truncation so a few huge first-messages don't dominate the prompt.
const CHAIN_NAME_PER_MESSAGE_CHARS: usize = 500;

/// Total joined-input cap for chain-name generation — bounds the prompt for
/// long chains regardless of per-message length.
const CHAIN_NAME_TOTAL_CHARS: usize = 4000;

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

/// Generate a prose display name for a continuation chain by summarizing the
/// first user message of each member (in chain order) via a cheap LLM
/// (REQ-CHN-010).
///
/// `first_messages` is each member's first user message in chain order. Empty
/// members should already be dropped by the caller; an empty slice yields
/// `None` (nothing to summarize).
///
/// Returns `None` on timeout, LLM error, empty input, or an empty model
/// response. The caller leaves the existing name untouched on `None`
/// (REQ-CHN-010 — no partial/empty name written).
pub async fn generate_chain_name(
    first_messages: &[String],
    llm_service: Arc<dyn LlmService>,
    effective_effort: phoenix_core::domain::llm_types::EffectiveEffort,
    max_output_tokens: Option<u32>,
) -> Option<String> {
    if first_messages.is_empty() {
        return None;
    }

    // Defensive truncation: cap each message, then cap the joined total so a
    // long chain doesn't blow the prompt. Numbered for the model's orientation.
    let mut joined = String::new();
    for (idx, msg) in first_messages.iter().enumerate() {
        let truncated = truncate_chars(msg.trim(), CHAIN_NAME_PER_MESSAGE_CHARS);
        if truncated.is_empty() {
            continue;
        }
        let line = format!("{}. {}\n", idx + 1, truncated);
        if joined.len() + line.len() > CHAIN_NAME_TOTAL_CHARS {
            break;
        }
        joined.push_str(&line);
    }

    if joined.trim().is_empty() {
        return None;
    }

    let prompt = format!("{CHAIN_NAME_PROMPT}\n{joined}");

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
        max_tokens: Some(max_output_tokens.map_or(50, |limit| limit.min(50))), // Name should be very short
        effective_effort,
        service_tier: phoenix_core::domain::llm_types::EffectiveServiceTier::Standard,
        telemetry: None,
        // Distinct key from the title generator so the two prompts cache
        // independently — they have different prefixes and output shapes.
        cache_key: PromptCacheKey::stable("chain-name-generator"),
    };

    let result = timeout(TITLE_TIMEOUT, llm_service.complete(&request)).await;

    match result {
        Ok(Ok(response)) => extract_title_from_response(&response)
            .map(|t| clean_prose_name(&t))
            .filter(|s| !s.is_empty()),
        Ok(Err(e)) => {
            tracing::warn!("Chain name generation LLM error: {}", e.message);
            None
        }
        Err(_) => {
            tracing::warn!("Chain name generation timed out");
            None
        }
    }
}

/// Truncate `s` to at most `max_chars` characters (char-safe, not byte-safe).
fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        s.chars().take(max_chars).collect()
    }
}

/// Clean a model-produced prose display name: drop control characters and
/// collapse internal whitespace to single spaces. Does NOT kebab-case or
/// length-truncate — that is the caller's normalization (single length
/// authority, REQ-CHN-010).
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
    use super::*;

    #[test]
    fn generated_names_keep_unicode_case_and_full_words() {
        assert_eq!(
            clean_prose_name("  Fix Über 東京 Auth  "),
            "Fix Über 東京 Auth"
        );
    }

    #[test]
    fn clean_prose_name_collapses_whitespace_and_keeps_case() {
        // Prose name is NOT kebab-cased and NOT lowercased.
        assert_eq!(
            clean_prose_name("  Auth   Refactor\tAnd Tests  "),
            "Auth Refactor And Tests"
        );
    }

    #[test]
    fn clean_prose_name_strips_control_chars() {
        assert_eq!(clean_prose_name("Auth\u{0007}Refactor"), "AuthRefactor");
        // Newlines are whitespace, so they collapse to a single space.
        assert_eq!(clean_prose_name("Auth\nRefactor"), "Auth Refactor");
    }

    #[test]
    fn clean_prose_name_does_not_length_truncate() {
        // The generator imposes no length cap; that is the caller's authority.
        let long = "Word ".repeat(100);
        let cleaned = clean_prose_name(&long);
        assert!(cleaned.chars().count() > 100);
    }

    #[test]
    fn truncate_chars_is_char_safe() {
        assert_eq!(truncate_chars("hello", 10), "hello");
        assert_eq!(truncate_chars("hello", 3), "hel");
        // Multibyte chars are counted, not bytes — no panic on a boundary.
        assert_eq!(truncate_chars("héllo", 2), "hé");
    }
}
