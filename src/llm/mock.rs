//! Mock LLM provider for frontend development without real API keys.
//!
//! Streams lorem-ipsum-style responses with realistic delays and cycles
//! through different response types: plain text, markdown, and tool calls.

use super::types::{ContentBlock, LlmRequest, LlmResponse, Usage};
use super::{LlmError, LlmService, TokenChunk};
use async_trait::async_trait;
use tokio::sync::broadcast;

/// Mock LLM service that produces canned responses for UI development.
pub struct MockLlmService;

/// Response scenarios the mock cycles through based on a counter derived
/// from the user message content (so repeated identical messages get the
/// same response, making UI work predictable).
enum Scenario {
    PlainText,
    Markdown,
    BashToolCall,
    ReadFileToolCall,
    ThinkThenRespond,
    MultiToolCall,
    LongStreaming,
}

impl Scenario {
    fn from_message(request: &LlmRequest) -> Self {
        // Use the last user message to pick a scenario deterministically.
        let hash: usize = request
            .messages
            .iter()
            .rev()
            .find_map(|m| {
                if m.role == super::types::MessageRole::User {
                    Some(
                        m.content
                            .iter()
                            .filter_map(|b| match b {
                                ContentBlock::Text { text } => Some(text.as_str()),
                                _ => None,
                            })
                            .collect::<String>(),
                    )
                } else {
                    None
                }
            })
            .map(|text| text.bytes().map(|b| b as usize).sum())
            .unwrap_or(0);

        match hash % 7 {
            0 => Self::PlainText,
            1 => Self::Markdown,
            2 => Self::BashToolCall,
            3 => Self::ReadFileToolCall,
            4 => Self::ThinkThenRespond,
            5 => Self::MultiToolCall,
            6 => Self::LongStreaming,
            _ => unreachable!(),
        }
    }
}

const PLAIN_TEXT: &str = "I've analyzed the situation and here's what I found. \
The configuration looks correct overall, but there's a subtle issue with how \
the timeout is being calculated. The current implementation uses milliseconds \
where the upstream library expects seconds, causing requests to time out \
1000x earlier than intended.\n\n\
I'll fix this by dividing the value by 1000 before passing it to the client constructor.";

const MARKDOWN_TEXT: &str = r#"## Analysis

The issue is in the request pipeline. Here's what's happening:

1. **Request arrives** at the handler with correct headers
2. **Middleware** strips the `Authorization` header (this is the bug)
3. **Downstream service** rejects with 401

### Root Cause

The `strip_internal_headers` middleware is using a prefix match:

```rust
if header.starts_with("Auth") {
    // This catches Authorization AND Auth-Token
    headers.remove(header);
}
```

### Fix

Change to exact match:

```rust
let internal_headers = ["Auth-Token", "Auth-Internal"];
if internal_headers.contains(&header.as_str()) {
    headers.remove(header);
}
```

This preserves `Authorization` while still stripping internal auth headers.

| Header | Before | After |
|--------|--------|-------|
| `Authorization` | Stripped | Preserved |
| `Auth-Token` | Stripped | Stripped |
| `Auth-Internal` | Stripped | Stripped |"#;

const LONG_TEXT: &str = "Let me walk through this step by step.\n\n\
First, I need to understand the data flow. The input comes from the WebSocket \
connection, gets deserialized into a `Frame` struct, then passes through the \
validation layer before hitting the state machine. The state machine is where \
things get interesting -- it maintains a directed acyclic graph of dependencies \
between active tasks, and each transition must preserve the topological ordering.\n\n\
The bug you're seeing happens when two tasks complete simultaneously. The current \
implementation processes completions sequentially, which is fine, but it checks \
the dependency graph *before* removing the completed task from it. This means the \
second completion sees stale graph state and can incorrectly conclude that a \
dependent task is still blocked.\n\n\
Here's the sequence:\n\
1. Task A completes, triggers check for dependents\n\
2. Task B completes concurrently, also triggers check\n\
3. Task A's check runs first, finds Task C depends on A and B\n\
4. Task A removes itself from graph, sees B still present, C stays blocked (correct)\n\
5. Task B's check runs, but graph already has A removed, finds C only depends on B\n\
6. Task B removes itself, unblocks C (correct)\n\n\
Wait -- actually this sequence works. Let me re-examine the actual code path...\n\n\
Ah, I see it now. The issue is in the *notification* path, not the check. When Task A \
completes, it sends a `TaskCompleted` event. The event handler for this event \
re-reads the graph, but between the event being queued and processed, Task B \
may have already modified the graph. The fix is to make the completion + graph \
update + notification atomic.";

fn tool_use_id() -> String {
    format!("mock_toolu_{:016x}", rand_u64())
}

fn rand_u64() -> u64 {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let s = RandomState::new();
    let mut h = s.build_hasher();
    h.write_u8(0);
    h.finish()
}

fn build_response(scenario: &Scenario) -> (Vec<ContentBlock>, String) {
    match scenario {
        Scenario::PlainText => (
            vec![ContentBlock::Text {
                text: PLAIN_TEXT.to_string(),
            }],
            PLAIN_TEXT.to_string(),
        ),

        Scenario::Markdown => (
            vec![ContentBlock::Text {
                text: MARKDOWN_TEXT.to_string(),
            }],
            MARKDOWN_TEXT.to_string(),
        ),

        Scenario::BashToolCall => {
            let text = "Let me check the current git status.".to_string();
            (
                vec![
                    ContentBlock::Text { text: text.clone() },
                    ContentBlock::ToolUse {
                        id: tool_use_id(),
                        name: "bash".to_string(),
                        input: serde_json::json!({
                            "command": "git status --short"
                        }),
                    },
                ],
                text,
            )
        }

        Scenario::ReadFileToolCall => {
            let text = "I'll read the configuration file to understand the current setup."
                .to_string();
            (
                vec![
                    ContentBlock::Text { text: text.clone() },
                    ContentBlock::ToolUse {
                        id: tool_use_id(),
                        name: "read_file".to_string(),
                        input: serde_json::json!({
                            "path": "Cargo.toml",
                            "start_line": 1,
                            "end_line": 30
                        }),
                    },
                ],
                text,
            )
        }

        Scenario::ThinkThenRespond => {
            let think_text = "The user is asking about the architecture. I should explain \
                the state machine approach and how it connects to the frontend. \
                Let me structure this clearly."
                .to_string();
            let response_text = "The architecture uses a state machine at its core. Each \
                conversation goes through deterministic state transitions, and the \
                frontend subscribes to these via SSE. This means the UI always reflects \
                the true server state -- no optimistic updates that can diverge."
                .to_string();
            (
                vec![
                    ContentBlock::ToolUse {
                        id: tool_use_id(),
                        name: "think".to_string(),
                        input: serde_json::json!({
                            "thought": think_text
                        }),
                    },
                    ContentBlock::Text {
                        text: response_text.clone(),
                    },
                ],
                response_text,
            )
        }

        Scenario::MultiToolCall => {
            let text =
                "I'll check the project structure and recent changes.".to_string();
            (
                vec![
                    ContentBlock::Text { text: text.clone() },
                    ContentBlock::ToolUse {
                        id: tool_use_id(),
                        name: "bash".to_string(),
                        input: serde_json::json!({
                            "command": "ls -la src/"
                        }),
                    },
                    ContentBlock::ToolUse {
                        id: tool_use_id(),
                        name: "bash".to_string(),
                        input: serde_json::json!({
                            "command": "git log --oneline -5"
                        }),
                    },
                ],
                text,
            )
        }

        Scenario::LongStreaming => (
            vec![ContentBlock::Text {
                text: LONG_TEXT.to_string(),
            }],
            LONG_TEXT.to_string(),
        ),
    }
}

/// Stream text word-by-word with small delays to simulate real LLM output.
async fn stream_text(text: &str, chunk_tx: &broadcast::Sender<TokenChunk>) {
    // Split into small chunks (roughly word-sized) for realistic streaming
    let mut chars = text.chars().peekable();
    let mut buf = String::new();

    while let Some(ch) = chars.next() {
        buf.push(ch);
        // Emit on whitespace boundaries or after newlines
        let flush = ch.is_whitespace()
            || ch == '\n'
            || buf.len() > 15
            || chars.peek().is_none();

        if flush && !buf.is_empty() {
            let _ = chunk_tx.send(TokenChunk::Text(buf.clone()));
            buf.clear();
            // Small delay between chunks: 15-40ms feels realistic
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }

    if !buf.is_empty() {
        let _ = chunk_tx.send(TokenChunk::Text(buf));
    }
}

#[async_trait]
impl LlmService for MockLlmService {
    async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        let scenario = Scenario::from_message(request);
        let (content, _) = build_response(&scenario);

        Ok(LlmResponse {
            content,
            end_turn: true,
            usage: Usage {
                input_tokens: 150,
                output_tokens: 80,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            },
        })
    }

    async fn complete_streaming(
        &self,
        request: &LlmRequest,
        chunk_tx: &broadcast::Sender<TokenChunk>,
    ) -> Result<LlmResponse, LlmError> {
        let scenario = Scenario::from_message(request);
        let (content, streamable_text) = build_response(&scenario);

        // Simulate initial latency (time-to-first-token)
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // Stream the text portion
        if !streamable_text.is_empty() {
            stream_text(&streamable_text, chunk_tx).await;
        }

        Ok(LlmResponse {
            content,
            end_turn: true,
            usage: Usage {
                input_tokens: 150,
                output_tokens: 80,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            },
        })
    }

    fn model_id(&self) -> &str {
        "mock"
    }
}
