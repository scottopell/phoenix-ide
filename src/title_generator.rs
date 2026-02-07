//! Conversation title generation using a fast/cheap LLM
//!
//! Generates short, meaningful titles based on the initial user message.

use crate::llm::{ContentBlock, LlmMessage, LlmRequest, LlmResponse, LlmService, MessageRole};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;

const TITLE_PROMPT: &str = r#"Generate a very short (3-6 words) title summarizing this request. Output only the title, no quotes or punctuation. Examples:
- "Fix login page CSS bug" -> Fix Login Page CSS
- "Help me write a Python script to parse CSV files" -> Python CSV Parser Script
- "What's the best way to implement caching?" -> Implementing Caching Strategy

Request:"#;

const TITLE_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_TITLE_LENGTH: usize = 60;

/// Generate a title for a conversation based on the initial message.
///
/// Returns None if title generation fails (timeout, error, etc.)
/// The caller should fall back to a random slug in that case.
pub async fn generate_title(
    message_text: &str,
    llm_service: Arc<dyn LlmService>,
) -> Option<String> {
    // Truncate very long messages for the prompt
    let truncated = if message_text.len() > 500 {
        format!("{}...", &message_text[..500])
    } else {
        message_text.to_string()
    };

    let prompt = format!("{TITLE_PROMPT}\n{truncated}");

    let request = LlmRequest {
        system: vec![],
        messages: vec![LlmMessage {
            role: MessageRole::User,
            content: vec![ContentBlock::text(prompt)],
        }],
        tools: vec![],
        max_tokens: Some(50), // Title should be very short
    };

    // Apply timeout
    let result = timeout(TITLE_TIMEOUT, llm_service.complete(&request)).await;

    match result {
        Ok(Ok(response)) => {
            // Extract text from response
            let title = extract_title_from_response(&response);
            title.map(|t| sanitize_title(&t))
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

/// Sanitize the title for use as a slug
/// - Truncate to max length
/// - Replace problematic characters
/// - Convert to lowercase kebab-case
fn sanitize_title(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace() || *c == '-' || *c == '_')
        .collect();

    let kebab: String = cleaned
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-")
        .to_lowercase();

    // Truncate if too long
    if kebab.len() > MAX_TITLE_LENGTH {
        // Try to cut at a word boundary
        let truncated = &kebab[..MAX_TITLE_LENGTH];
        if let Some(last_dash) = truncated.rfind('-') {
            truncated[..last_dash].to_string()
        } else {
            truncated.to_string()
        }
    } else {
        kebab
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_title() {
        assert_eq!(sanitize_title("Fix Login Page CSS"), "fix-login-page-css");
        assert_eq!(
            sanitize_title("Python CSV Parser Script"),
            "python-csv-parser-script"
        );
        assert_eq!(sanitize_title("What's the best way?"), "whats-the-best-way");
        assert_eq!(sanitize_title("  Multiple   Spaces  "), "multiple-spaces");
    }

    #[test]
    fn test_sanitize_title_truncation() {
        let long_title = "This is a very long title that should be truncated at some point";
        let result = sanitize_title(long_title);
        assert!(result.len() <= MAX_TITLE_LENGTH);
    }
}
