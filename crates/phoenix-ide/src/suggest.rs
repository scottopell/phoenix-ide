//! One-shot shell-command suggestion.
//!
//! Stateless Tier-0 of the `phx suggest` spectrum: a single LLM completion
//! with no conversation, no tools, and no persistence. Given a natural-language
//! request it returns the shell command(s) that accomplish it. The terminal
//! renders each as a click-to-run affordance; nothing executes server-side, so
//! the model is structurally a suggester — it has no bash tool to invoke.
//!
//! Modeled on [`crate::title_generator`]: minimal request, small token budget,
//! shared cache key, bounded by a timeout.

use phoenix_llm::{
    ContentBlock, LlmMessage, LlmRequest, LlmResponse, LlmService, MessageRole, PromptCacheKey,
    SystemContent,
};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;

use phoenix_core::llm_language::SUGGEST_SYSTEM;

const SUGGEST_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_SUGGEST_TOKENS: u32 = 400;

/// Produce a list of suggested shell commands for `query`.
///
/// Returns the commands in model order. Comment (`#`) and stray fence lines are
/// stripped, so every returned string is a runnable command line. `Err` carries
/// a human-readable reason (timeout or upstream LLM error) for the caller to
/// surface.
pub async fn suggest_commands(
    query: &str,
    llm: Arc<dyn LlmService>,
) -> Result<Vec<String>, String> {
    let request = LlmRequest {
        system: vec![SystemContent::cached(SUGGEST_SYSTEM)],
        messages: vec![LlmMessage {
            role: MessageRole::User,
            content: vec![ContentBlock::text(query)],
        }],
        tools: vec![],
        max_tokens: Some(MAX_SUGGEST_TOKENS),
        // Shared by every suggestion call so SUGGEST_SYSTEM caches.
        cache_key: PromptCacheKey::stable("command-suggester"),
    };

    match timeout(SUGGEST_TIMEOUT, llm.complete(&request)).await {
        Ok(Ok(response)) => Ok(parse_command_lines(&response_text(&response))),
        Ok(Err(e)) => Err(e.message),
        Err(_) => Err("command suggestion timed out".to_string()),
    }
}

/// Concatenate the text blocks of a response into one string.
fn response_text(response: &LlmResponse) -> String {
    response
        .content
        .iter()
        .filter_map(|b| {
            if let ContentBlock::Text { text } = b {
                Some(text.as_str())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Split model output into runnable command lines, dropping blanks, comments,
/// and any stray markdown fence the model emitted despite instruction.
fn parse_command_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .filter(|l| !l.starts_with('#'))
        .filter(|l| !l.starts_with("```"))
        .map(ToString::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_commands() {
        let out = parse_command_lines("ls -la\ncd src");
        assert_eq!(out, vec!["ls -la", "cd src"]);
    }

    #[test]
    fn drops_comments_blanks_and_fences() {
        let text = "```sh\n# make the dir\nmkdir -p a/b\n\ngit worktree add ../wt main\n```";
        let out = parse_command_lines(text);
        assert_eq!(out, vec!["mkdir -p a/b", "git worktree add ../wt main"]);
    }

    #[test]
    fn trims_surrounding_whitespace() {
        let out = parse_command_lines("   echo hi   \n");
        assert_eq!(out, vec!["echo hi"]);
    }

    #[test]
    fn empty_output_yields_no_commands() {
        assert!(parse_command_lines("\n\n# just a comment\n").is_empty());
    }
}
