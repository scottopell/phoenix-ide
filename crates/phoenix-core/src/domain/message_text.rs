//! Plain-text extraction from a typed [`Message`] for the conversation
//! retrieval index (`specs/conversation-retrieval/` REQ-RET-004).
//!
//! The index stores extracted prose, never the raw `content` JSON, so a
//! search matches words a human wrote rather than serialization structure.
//! Tool results are machine output but still content-bearing — the answer
//! to a recall question is often a path or an error that lives only in a
//! build log — so they are kept as a size-capped **head + tail** excerpt
//! rather than dropped to a bare marker (a failing test name or error is
//! usually near the *end* of a long log, so head-only truncation would miss
//! it). The full body is always reachable through the read path; this is the
//! ranking-signal projection.

// Extraction keys off the prose-bearing variants; non-prose ContentBlock
// kinds (images, tool-use/result blocks, …) are uniformly skipped, so a
// blanket arm is the intent here rather than per-variant enumeration of the
// dozen-plus block kinds (mirrors `chain_qa`'s transcript renderer).
#![allow(clippy::wildcard_enum_match_arm)]

use super::db_schema::{Message, MessageContent};
use super::llm_types::ContentBlock;

/// Leading slice (in characters) of an over-long tool result kept in the
/// index excerpt.
pub const TOOL_EXCERPT_HEAD_CHARS: usize = 1024;
/// Trailing slice (in characters) of an over-long tool result kept in the
/// index excerpt.
pub const TOOL_EXCERPT_TAIL_CHARS: usize = 1024;

/// Extract the searchable text of a single message for the retrieval index.
///
/// Returns the message body only — no role label or framing — because the
/// role is carried in the index's `message_type` column, not the indexed
/// text. Tool-use blocks within an agent message are omitted (they are
/// structured calls, not prose); tool *results* are excerpted (head + tail).
///
/// Messages the UI deliberately hides (`display_data.hidden == true`, e.g.
/// dismissed-error / dismissed-question recovery markers persisted as
/// `System`) yield empty text, so internal artifacts never surface in recall.
#[must_use]
pub fn index_text(message: &Message) -> String {
    if is_hidden(message) {
        return String::new();
    }
    match &message.content {
        MessageContent::User(c) => c.text.clone(),
        MessageContent::Agent(blocks) => blocks
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        MessageContent::Tool(c) => {
            tool_excerpt(&c.content, TOOL_EXCERPT_HEAD_CHARS, TOOL_EXCERPT_TAIL_CHARS)
        }
        MessageContent::System(c) => c.text.clone(),
        MessageContent::Error(c) => c.message.clone(),
        MessageContent::Continuation(c) => c.summary.clone(),
        MessageContent::Skill(c) => format!("/{} {}", c.name, c.trigger),
    }
}

/// Whether the UI deliberately hides this message (`display_data.hidden ==
/// true`), e.g. dismissed-error / dismissed-question recovery markers. Hidden
/// messages are kept out of the retrieval index so internal artifacts don't
/// pollute recall.
fn is_hidden(message: &Message) -> bool {
    message
        .display_data
        .as_ref()
        .and_then(|d| d.get("hidden"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

/// Size-capped head + tail excerpt of a tool-result body. Short results pass
/// through whole; long ones keep the leading `head` and trailing `tail`
/// characters with the elided middle marked. Counts characters (not bytes)
/// so the slice never splits a UTF-8 scalar.
fn tool_excerpt(content: &str, head: usize, tail: usize) -> String {
    let total = content.chars().count();
    if total <= head + tail {
        return content.to_string();
    }
    let head_str: String = content.chars().take(head).collect();
    let tail_str: String = {
        // take the last `tail` chars, preserving order
        let mut t: Vec<char> = content.chars().rev().take(tail).collect();
        t.reverse();
        t.into_iter().collect()
    };
    let elided = total - head - tail;
    format!("{head_str}\n[… {elided} chars elided …]\n{tail_str}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::db_schema::{
        ContinuationContent, ErrorContent, Message, MessageContent, MessageType, SkillContent,
        SystemContent, ToolContent, UserContent,
    };
    use crate::domain::llm_types::ContentBlock;
    use chrono::Utc;

    fn msg(content: MessageContent) -> Message {
        Message {
            message_id: "m1".into(),
            conversation_id: "c1".into(),
            sequence_id: 1,
            message_type: MessageType::User,
            content,
            display_data: None,
            usage_data: None,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn user_text_is_extracted_verbatim() {
        let m = msg(MessageContent::User(UserContent {
            text: "fix the auth schema".into(),
            images: vec![],
            files: vec![],
            llm_text: None,
            is_meta: false,
        }));
        assert_eq!(index_text(&m), "fix the auth schema");
    }

    #[test]
    fn agent_concatenates_text_blocks_and_drops_tool_use() {
        let m = msg(MessageContent::Agent(vec![
            ContentBlock::Text {
                text: "first".into(),
            },
            ContentBlock::ToolUse {
                id: "t".into(),
                name: "bash".into(),
                input: serde_json::json!({}),
            },
            ContentBlock::Text {
                text: "second".into(),
            },
        ]));
        assert_eq!(index_text(&m), "first\nsecond");
    }

    #[test]
    fn short_tool_result_passes_through_whole() {
        let m = msg(MessageContent::Tool(ToolContent {
            tool_use_id: "t".into(),
            content: "error: missing semicolon at foo.rs:42".into(),
            is_error: true,
            images: vec![],
        }));
        assert_eq!(index_text(&m), "error: missing semicolon at foo.rs:42");
    }

    #[test]
    fn long_tool_result_keeps_head_and_tail() {
        // head marker ... (filler) ... tail marker
        let filler = "x".repeat(5000);
        let body = format!("HEAD_START compiling crate\n{filler}\nFAILED at zzz.rs:7 TAIL_END");
        let m = msg(MessageContent::Tool(ToolContent {
            tool_use_id: "t".into(),
            content: body,
            is_error: true,
            images: vec![],
        }));
        let out = index_text(&m);
        assert!(out.contains("HEAD_START"), "head signal must survive");
        assert!(out.contains("TAIL_END"), "tail signal must survive");
        assert!(out.contains("chars elided"), "elision must be marked");
        assert!(
            out.len() < 5000,
            "excerpt must be bounded, got {}",
            out.len()
        );
    }

    #[test]
    fn tool_excerpt_respects_char_boundaries() {
        // multibyte content; ensure no panic and bounded output
        let body = "é".repeat(4000);
        let out = tool_excerpt(&body, 1024, 1024);
        assert!(out.contains("elided"));
        // round-trips as valid UTF-8 (String guarantees it; assert non-empty head/tail)
        assert!(out.starts_with('é'));
        assert!(out.ends_with('é'));
    }

    #[test]
    fn other_variants_extract_their_prose() {
        assert_eq!(
            index_text(&msg(MessageContent::System(SystemContent {
                text: "sys".into()
            }))),
            "sys"
        );
        assert_eq!(
            index_text(&msg(MessageContent::Error(ErrorContent {
                message: "boom".into()
            }))),
            "boom"
        );
        assert_eq!(
            index_text(&msg(MessageContent::Continuation(ContinuationContent {
                summary: "did things".into()
            }))),
            "did things"
        );
        assert_eq!(
            index_text(&msg(MessageContent::Skill(SkillContent {
                name: "review".into(),
                body: "b".into(),
                trigger: "/review".into(),
                files: vec![],
            }))),
            "/review /review"
        );
    }

    #[test]
    fn hidden_messages_are_not_indexed() {
        let mut m = msg(MessageContent::System(SystemContent {
            text: "user dismissed the error".into(),
        }));
        m.display_data = Some(serde_json::json!({ "hidden": true }));
        assert_eq!(
            index_text(&m),
            "",
            "hidden recovery markers must not enter the index",
        );

        // A non-hidden system message still indexes normally.
        let mut visible = msg(MessageContent::System(SystemContent {
            text: "context restored".into(),
        }));
        visible.display_data = Some(serde_json::json!({ "hidden": false }));
        assert_eq!(index_text(&visible), "context restored");
    }
}
