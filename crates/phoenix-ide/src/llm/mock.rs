//! Mock LLM provider for frontend development without real API keys.
//!
//! Streams lorem-ipsum-style responses with realistic delays and cycles
//! through different response types: plain text, markdown, and tool calls.
//!
//! Test-driver markers (parsed from the latest user message text):
//! - `[[scenario:NAME]]` — force a specific scripted response
//!   (`plain_text`, `markdown`, `bash`, `read_file`, `think`,
//!   `multi_tool`, `long`, `patch`).
//! - `[[perf:N]]` — deterministic text-only response of ~N words
//!   (performance fingerprint, no rand).
//! - `[[ttft:N]]` — override time-to-first-token sleep with N ms.
//!   Useful for exercising the `StateBar`'s pre-first-byte
//!   `awaiting LLM response Ns` window
//!   (specs/working-phase-visibility/ REQ-WPV-007).
//! - `[[stall:after_n,ms]]` — emit `after_n` chunks, sleep `ms`,
//!   then continue. Drives the heartbeat watchdog (REQ-WPV-004)
//!   without ending the turn.
//! - `[[retry:KIND,N]]` — first N calls for this conversation
//!   return `LlmError::<KIND>` where KIND ∈ {`rate_limit`,
//!   `server_error`, `network`}; (N+1)th call succeeds. Drives
//!   `Effect::ScheduleRetry` end-to-end (Stage B / specs/llm-retry-visibility/).
//!
//! Markers compose freely. Opt-in via `PHOENIX_ENABLE_MOCK_MODEL=1`.

use super::types::{ContentBlock, LlmRequest, LlmResponse, Usage};
use super::{LlmError, LlmService, TokenChunk};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Mutex;
use tokio::sync::broadcast;

/// Per-conversation retry-attempt counter, keyed by `LlmRequest.cache_key`
/// (which is the conversation id at the call site — `executor.rs` builds
/// it via `PromptCacheKey::stable(&conv_id)`). The `[[retry:KIND,N]]`
/// marker reads & increments this on each `complete_streaming` call so a
/// scripted scenario can request "first N attempts fail, the (N+1)th
/// succeeds" without the request carrying its own attempt counter.
///
/// Entries are never garbage-collected — the mock is dev-only and the
/// table size is bounded by active conversations, so the leak is
/// acceptable. Tests that need a clean slate use a fresh conversation
/// per case (the normal pattern) rather than poking this map.
static RETRY_COUNTS: Mutex<Option<HashMap<String, u32>>> = Mutex::new(None);

fn bump_retry_count(key: &str) -> u32 {
    let mut guard = RETRY_COUNTS.lock().expect("RETRY_COUNTS mutex");
    let map = guard.get_or_insert_with(HashMap::new);
    let entry = map.entry(key.to_string()).or_insert(0);
    *entry += 1;
    *entry
}

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
    /// Marker-only (not in hash rotation): emits a `patch` `tool_use` that
    /// overwrites `e2e-mock-patch-out.txt` in the conversation's cwd.
    /// Authored for E2E tests; see `[[scenario:patch]]` callers.
    PatchToolCall,
}

impl Scenario {
    fn from_message(request: &LlmRequest) -> Self {
        // Explicit selection via `[[scenario:NAME]]` marker wins over hashing.
        if let Some(s) = parse_scenario(request) {
            return s;
        }
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
            .map_or(0, |text| text.bytes().map(|b| b as usize).sum());

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

/// Scenario-fixture marker: a user message containing `[[scenario:NAME]]`
/// forces selection of a specific scripted response, bypassing the hash-based
/// roulette. Symmetric with `[[perf:N]]` — both make the mock authorable for
/// E2E tests. Recognized NAMEs: `plain_text`, `markdown`, `bash`, `read_file`,
/// `think`, `multi_tool`, `long`, `patch`. Dev-only: mock is opt-in
/// (`PHOENIX_ENABLE_MOCK_MODEL=1`).
fn parse_scenario(request: &LlmRequest) -> Option<Scenario> {
    let text = request.messages.iter().rev().find_map(|m| {
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
    })?;
    let start = text.find("[[scenario:")? + "[[scenario:".len();
    let rest = text.get(start..)?;
    let end = rest.find("]]")?;
    let name = rest.get(..end)?.trim();
    match name {
        "plain_text" => Some(Scenario::PlainText),
        "markdown" => Some(Scenario::Markdown),
        "bash" => Some(Scenario::BashToolCall),
        "read_file" => Some(Scenario::ReadFileToolCall),
        "think" => Some(Scenario::ThinkThenRespond),
        "multi_tool" => Some(Scenario::MultiToolCall),
        "long" => Some(Scenario::LongStreaming),
        "patch" => Some(Scenario::PatchToolCall),
        _ => None,
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

const MAX_PERF_WORDS: usize = 5_000;

/// Perf-fixture marker: a user message containing `[[perf:N]]` forces a
/// fully deterministic text-only response of ~N whitespace-separated words,
/// bypassing scenario selection and `rand`. This makes the streaming path a
/// reproducible performance fingerprint at a caller-chosen length — the same
/// N always yields byte-identical content at a fixed cadence. Dev-only: mock
/// is opt-in (`PHOENIX_ENABLE_MOCK_MODEL=1`).
fn parse_perf_words(request: &LlmRequest) -> Option<usize> {
    let text = request.messages.iter().rev().find_map(|m| {
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
    })?;
    let start = text.find("[[perf:")? + "[[perf:".len();
    let rest = text.get(start..)?;
    let end = rest.find("]]")?;
    rest.get(..end)?
        .trim()
        .parse::<usize>()
        .ok()
        .map(|n| n.min(MAX_PERF_WORDS))
}

/// TTFT-override marker (REQ-WPV-001 / 006 test driver): a user message
/// containing `[[ttft:N]]` overrides the mock's default 200ms
/// time-to-first-token sleep with `N` milliseconds. Useful range
/// 100ms…30000ms; values above `MAX_TTFT_MS` are clamped to that ceiling
/// so a typo doesn't park the dev-mock for an hour. Composes freely with
/// `[[scenario:NAME]]`, `[[perf:N]]`, and `[[stall:…]]`. Dev-only
/// (`PHOENIX_ENABLE_MOCK_MODEL=1`).
fn parse_ttft_ms(request: &LlmRequest) -> Option<u64> {
    let text = latest_user_text(request)?;
    let start = text.find("[[ttft:")? + "[[ttft:".len();
    let rest = text.get(start..)?;
    let end = rest.find("]]")?;
    rest.get(..end)?
        .trim()
        .parse::<u64>()
        .ok()
        .map(|n| n.min(MAX_TTFT_MS))
}

/// Mid-stream stall marker (REQ-WPV-004 test driver): a user message
/// containing `[[stall:after_n,ms]]` makes the mock emit `after_n` token
/// chunks normally, sleep for `ms` milliseconds, then continue streaming
/// the remainder. The chunk channel stays alive across the sleep so the
/// turn does NOT end during the stall — exactly the failure mode the
/// heartbeat watchdog is meant to surface (server holds the connection
/// open but stops sending data). Use `ms ≥ 35000` to drive the watchdog
/// past its threshold. Both args clamp to `MAX_STALL_AFTER_N` /
/// `MAX_STALL_MS` so a typo can't park dev forever.
fn parse_stall(request: &LlmRequest) -> Option<(usize, u64)> {
    let text = latest_user_text(request)?;
    let start = text.find("[[stall:")? + "[[stall:".len();
    let rest = text.get(start..)?;
    let end = rest.find("]]")?;
    let body = rest.get(..end)?.trim();
    let (after_n, ms) = body.split_once(',')?;
    let after_n = after_n.trim().parse::<usize>().ok()?;
    let ms = ms.trim().parse::<u64>().ok()?;
    Some((after_n.min(MAX_STALL_AFTER_N), ms.min(MAX_STALL_MS)))
}

/// Retry-simulation marker (Stage B `llm-retry-visibility` test driver):
/// a user message containing `[[retry:KIND,N]]` makes the mock return
/// `LlmError::<KIND>` (without emitting any tokens) for the first N
/// `complete_streaming` calls bound to the same conversation; the
/// (N+1)th call streams the normal scenario. KIND ∈ {`rate_limit`,
/// `server_error`, `network`} — exactly the retryable subset of
/// `LlmErrorKind` per `is_retryable` in `llm/error.rs:111`. The
/// per-conversation attempt counter lives in `RETRY_COUNTS` keyed by
/// `request.cache_key` (the conversation id at the call site).
///
/// Combined with the state machine's `MAX_RETRY_ATTEMPTS = 3`
/// (`transition.rs:183`), `[[retry:rate_limit,2]]` produces:
///   attempt 1 → `LlmError::RateLimit` → `ScheduleRetry`, attempt 2
///   attempt 2 → `LlmError::RateLimit` → `ScheduleRetry`, attempt 3
///   attempt 3 → success → turn completes
/// `[[retry:rate_limit,3]]` exercises the give-up path
/// (transition to Error after `MAX_RETRY_ATTEMPTS`).
fn parse_retry(request: &LlmRequest) -> Option<(crate::llm::LlmErrorKind, u32)> {
    let text = latest_user_text(request)?;
    let start = text.find("[[retry:")? + "[[retry:".len();
    let rest = text.get(start..)?;
    let end = rest.find("]]")?;
    let body = rest.get(..end)?.trim();
    let (kind_str, n_str) = body.split_once(',')?;
    let kind = match kind_str.trim() {
        "rate_limit" => crate::llm::LlmErrorKind::RateLimit,
        "server_error" => crate::llm::LlmErrorKind::ServerError,
        "network" => crate::llm::LlmErrorKind::Network,
        _ => return None,
    };
    let n = n_str.trim().parse::<u32>().ok()?.min(MAX_RETRY_N);
    Some((kind, n))
}

/// Slow-tool marker (REQ-WPV-002 visual driver): a user message
/// containing `[[slow-tool:N]]` makes the mock emit a `bash` `tool_use`
/// whose command is `sleep N && echo done`, so the real bash tool's
/// execution genuinely takes ~N seconds. Used to visually verify the
/// inline elapsed-time counter in the tool-use block ticks while the
/// tool is running. `N` clamps to `MAX_SLOW_TOOL_S` so a typo can't
/// park dev forever. Composes with `[[ttft:…]]` and `[[retry:…]]`.
fn parse_slow_tool(request: &LlmRequest) -> Option<u64> {
    let text = latest_user_text(request)?;
    let start = text.find("[[slow-tool:")? + "[[slow-tool:".len();
    let rest = text.get(start..)?;
    let end = rest.find("]]")?;
    rest.get(..end)?
        .trim()
        .parse::<u64>()
        .ok()
        .map(|n| n.min(MAX_SLOW_TOOL_S))
}

/// Build the `(content, streamable_text)` tuple for a `[[slow-tool:N]]`
/// invocation: a short preamble + a `bash` `tool_use` that sleeps N
/// seconds. Standalone so both the streaming and non-streaming
/// `LlmService` paths can share it.
fn build_slow_tool_response(seconds: u64) -> (Vec<ContentBlock>, String) {
    let text = format!("Running a slow command ({seconds}s) so you can watch the timer tick.");
    (
        vec![
            ContentBlock::Text { text: text.clone() },
            ContentBlock::ToolUse {
                id: tool_use_id(),
                name: "bash".to_string(),
                input: serde_json::json!({
                    "op": "run",
                    "cmd": format!("sleep {seconds} && echo done")
                }),
            },
        ],
        text,
    )
}

/// Shared helper: the latest user message's text, concatenated across any
/// Text content blocks. Returns `None` if there are no user messages or
/// they carry no Text blocks. Used by every `[[…:…]]` marker parser.
fn latest_user_text(request: &LlmRequest) -> Option<String> {
    request.messages.iter().rev().find_map(|m| {
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
}

/// Ceilings for the test-driver markers — clamped so a typo (`[[ttft:1000000000]]`)
/// can't park dev forever. Generous enough to cover every realistic test scenario.
const MAX_TTFT_MS: u64 = 60_000; // 60s — covers heartbeat watchdog at 35s + headroom
const MAX_STALL_AFTER_N: usize = 10_000;
const MAX_STALL_MS: u64 = 120_000; // 2min
const MAX_RETRY_N: u32 = 10; // far above any real MAX_RETRY_ATTEMPTS
const MAX_SLOW_TOOL_S: u64 = 120; // 2min — generous; goal is visual verification

/// Deterministic text of exactly `n_words` words, built by cycling the words
/// of `LONG_TEXT`. Pure function of `n_words` — no rand, no time, no state.
fn perf_text(n_words: usize) -> String {
    let words: Vec<&str> = LONG_TEXT.split_whitespace().collect();
    if words.is_empty() || n_words == 0 {
        return String::new();
    }
    let mut out = String::with_capacity(n_words * 8);
    for i in 0..n_words {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(words[i % words.len()]);
    }
    out
}

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

#[allow(clippy::too_many_lines)] // flat match over scenarios; splitting would add no value
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
                            "op": "run",
                            "cmd": "git status --short"
                        }),
                    },
                ],
                text,
            )
        }

        Scenario::ReadFileToolCall => {
            let text =
                "I'll read the configuration file to understand the current setup.".to_string();
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
                            "thoughts": think_text
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
            let text = "I'll check the project structure and recent changes.".to_string();
            (
                vec![
                    ContentBlock::Text { text: text.clone() },
                    ContentBlock::ToolUse {
                        id: tool_use_id(),
                        name: "bash".to_string(),
                        input: serde_json::json!({
                            "op": "run",
                            "cmd": "ls -la src/"
                        }),
                    },
                    ContentBlock::ToolUse {
                        id: tool_use_id(),
                        name: "bash".to_string(),
                        input: serde_json::json!({
                            "op": "run",
                            "cmd": "git log --oneline -5"
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

        Scenario::PatchToolCall => {
            let text = "I'll create the file via patch overwrite.".to_string();
            (
                vec![
                    ContentBlock::Text { text: text.clone() },
                    ContentBlock::ToolUse {
                        id: tool_use_id(),
                        name: "patch".to_string(),
                        input: serde_json::json!({
                            "path": "e2e-mock-patch-out.txt",
                            "patches": [{
                                "operation": "overwrite",
                                "newText": "hello from mock patch scenario\n",
                            }],
                        }),
                    },
                ],
                text,
            )
        }
    }
}

/// Stream text word-by-word with small delays to simulate real LLM output.
///
/// `stall = Some((after_n, ms))` inserts a single sleep of `ms`
/// milliseconds after the first `after_n` chunks have been sent (and
/// before the (`after_n+1)th`). `after_n = 0` is special-cased to stall
/// *before* the first chunk lands — useful for driving the watchdog
/// when no tokens flow at all. Set via the `[[stall:after_n,ms]]`
/// test marker. The chunk channel stays open across the sleep, so
/// the turn does NOT end during the stall — this is exactly the
/// failure mode the heartbeat watchdog (REQ-WPV-004) is meant to
/// surface (server holds the connection open but stops sending data).
async fn stream_text_with_optional_stall(
    text: &str,
    chunk_tx: &broadcast::Sender<TokenChunk>,
    stall: Option<(usize, u64)>,
) {
    // Split into small chunks (roughly word-sized) for realistic streaming
    let mut chars = text.chars().peekable();
    let mut buf = String::new();
    let mut chunks_sent: usize = 0;
    let mut stall_fired = false;

    while let Some(ch) = chars.next() {
        buf.push(ch);
        // Emit on whitespace boundaries or after newlines
        let flush = ch.is_whitespace() || ch == '\n' || buf.len() > 15 || chars.peek().is_none();

        if flush && !buf.is_empty() {
            // Mid-stream stall hook (REQ-WPV-004): fires exactly once,
            // before sending the (after_n+1)th chunk. `after_n = 0`
            // fires before the very first chunk, so the watchdog
            // observes a stalled stream without any tokens flowing.
            if let Some((after_n, ms)) = stall {
                if !stall_fired && chunks_sent >= after_n {
                    stall_fired = true;
                    tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
                }
            }
            let _ = chunk_tx.send(TokenChunk::Text(buf.clone()));
            buf.clear();
            chunks_sent += 1;
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
        let content = if let Some(seconds) = parse_slow_tool(request) {
            build_slow_tool_response(seconds).0
        } else if let Some(n) = parse_perf_words(request) {
            vec![ContentBlock::Text { text: perf_text(n) }]
        } else {
            build_response(&Scenario::from_message(request)).0
        };

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
        // `[[retry:KIND,N]]` driver — the first N calls for this
        // conversation fail with the requested LlmErrorKind; the
        // (N+1)th call falls through to the normal scenario. The
        // conversation key comes from `request.cache_key`, which the
        // executor builds via `PromptCacheKey::stable(&conv_id)`.
        if let Some((kind, fail_n)) = parse_retry(request) {
            let attempt = bump_retry_count(request.cache_key.as_str());
            if attempt <= fail_n {
                let message =
                    format!("mock retry simulation: attempt {attempt}/{fail_n} returning {kind:?}");
                return Err(match kind {
                    crate::llm::LlmErrorKind::RateLimit => LlmError::rate_limit(message),
                    crate::llm::LlmErrorKind::ServerError => LlmError::server_error(message),
                    crate::llm::LlmErrorKind::Network => LlmError::network(message),
                    // parse_retry only emits the three retryable variants;
                    // any other kind escaped its validation and is a bug.
                    _ => unreachable!("parse_retry only emits retryable kinds"),
                });
            }
            // attempt > fail_n: fall through to the normal scenario.
        }

        let (content, streamable_text) = if let Some(seconds) = parse_slow_tool(request) {
            build_slow_tool_response(seconds)
        } else if let Some(n) = parse_perf_words(request) {
            let t = perf_text(n);
            (vec![ContentBlock::Text { text: t.clone() }], t)
        } else {
            build_response(&Scenario::from_message(request))
        };

        // Initial latency (time-to-first-token). `[[ttft:N]]` overrides
        // the default 200ms — useful for exercising the StateBar's
        // pre-first-byte `awaiting LLM response Ns` window.
        let ttft_ms = parse_ttft_ms(request).unwrap_or(200);
        tokio::time::sleep(std::time::Duration::from_millis(ttft_ms)).await;

        // Stream the text portion. `[[stall:after_n,ms]]` inserts a
        // mid-stream sleep after the first `after_n` chunks to drive
        // the heartbeat watchdog without ending the turn.
        if !streamable_text.is_empty() {
            let stall = parse_stall(request);
            stream_text_with_optional_stall(&streamable_text, chunk_tx, stall).await;
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

    #[allow(clippy::unnecessary_literal_bound)] // trait signature requires &str, not &'static str
    fn model_id(&self) -> &str {
        "mock"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::types::{LlmMessage, MessageRole, PromptCacheKey};

    fn user_req(text: &str) -> LlmRequest {
        LlmRequest {
            system: vec![],
            messages: vec![LlmMessage {
                role: MessageRole::User,
                content: vec![ContentBlock::Text {
                    text: text.to_string(),
                }],
            }],
            tools: vec![],
            max_tokens: None,
            cache_key: PromptCacheKey::ephemeral(),
        }
    }

    #[test]
    fn perf_marker_parsed() {
        assert_eq!(parse_perf_words(&user_req("[[perf:800]] go")), Some(800));
        assert_eq!(
            parse_perf_words(&user_req("[[perf:999999]] go")),
            Some(MAX_PERF_WORDS)
        );
        assert_eq!(parse_perf_words(&user_req("no marker here")), None);
        assert_eq!(parse_perf_words(&user_req("[[perf:bad]]")), None);
    }

    #[test]
    fn scenario_marker_recognizes_each_name() {
        let cases = [
            ("[[scenario:plain_text]] hi", Scenario::PlainText),
            ("[[scenario:markdown]]", Scenario::Markdown),
            ("[[scenario:bash]]", Scenario::BashToolCall),
            ("[[scenario:read_file]]", Scenario::ReadFileToolCall),
            ("[[scenario:think]]", Scenario::ThinkThenRespond),
            ("[[scenario:multi_tool]]", Scenario::MultiToolCall),
            ("[[scenario:long]]", Scenario::LongStreaming),
            ("[[scenario:patch]]", Scenario::PatchToolCall),
        ];
        for (text, expected) in cases {
            let req = user_req(text);
            let got = parse_scenario(&req).unwrap_or_else(|| panic!("no match for {text}"));
            assert!(
                std::mem::discriminant(&got) == std::mem::discriminant(&expected),
                "wrong variant for {text}"
            );
        }
        assert!(parse_scenario(&user_req("no marker here")).is_none());
        assert!(parse_scenario(&user_req("[[scenario:bogus]]")).is_none());
    }

    #[test]
    fn ttft_marker_parsed() {
        assert_eq!(parse_ttft_ms(&user_req("[[ttft:5000]] hi")), Some(5000));
        assert_eq!(parse_ttft_ms(&user_req("[[ttft:0]] hi")), Some(0));
        // Clamped to the ceiling so a typo can't park dev forever.
        assert_eq!(
            parse_ttft_ms(&user_req("[[ttft:9999999]]")),
            Some(MAX_TTFT_MS)
        );
        assert_eq!(parse_ttft_ms(&user_req("no marker")), None);
        assert_eq!(parse_ttft_ms(&user_req("[[ttft:abc]]")), None);
    }

    #[test]
    fn slow_tool_marker_parsed() {
        assert_eq!(parse_slow_tool(&user_req("[[slow-tool:5]] hi")), Some(5));
        assert_eq!(parse_slow_tool(&user_req("[[slow-tool:0]]")), Some(0));
        // Clamped to the ceiling.
        assert_eq!(
            parse_slow_tool(&user_req("[[slow-tool:9999999]]")),
            Some(MAX_SLOW_TOOL_S)
        );
        assert_eq!(parse_slow_tool(&user_req("no marker")), None);
        assert_eq!(parse_slow_tool(&user_req("[[slow-tool:abc]]")), None);
    }

    #[test]
    fn slow_tool_emits_bash_sleep_tool_use() {
        let (content, _) = build_slow_tool_response(7);
        // Expect a text preamble + a bash ToolUse whose cmd is `sleep 7 && echo done`.
        assert_eq!(content.len(), 2);
        let cmd = match &content[1] {
            ContentBlock::ToolUse { name, input, .. } => {
                assert_eq!(name, "bash");
                input
                    .get("cmd")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            }
            _ => None,
        };
        assert_eq!(cmd.as_deref(), Some("sleep 7 && echo done"));
    }

    #[test]
    fn stall_marker_parsed() {
        assert_eq!(
            parse_stall(&user_req("[[stall:5,40000]] hi")),
            Some((5, 40000))
        );
        assert_eq!(parse_stall(&user_req("[[stall:0,1000]]")), Some((0, 1000)));
        // Both args clamp; verify each ceiling.
        assert_eq!(
            parse_stall(&user_req("[[stall:99999999,1000]]")),
            Some((MAX_STALL_AFTER_N, 1000))
        );
        assert_eq!(
            parse_stall(&user_req("[[stall:5,99999999]]")),
            Some((5, MAX_STALL_MS))
        );
        // Malformed shapes are rejected.
        assert_eq!(parse_stall(&user_req("[[stall:5]]")), None);
        assert_eq!(parse_stall(&user_req("[[stall:abc,def]]")), None);
        assert_eq!(parse_stall(&user_req("no marker")), None);
    }

    #[test]
    fn retry_marker_parsed() {
        use crate::llm::LlmErrorKind;
        assert_eq!(
            parse_retry(&user_req("[[retry:rate_limit,2]] hi")),
            Some((LlmErrorKind::RateLimit, 2))
        );
        assert_eq!(
            parse_retry(&user_req("[[retry:server_error,3]]")),
            Some((LlmErrorKind::ServerError, 3))
        );
        assert_eq!(
            parse_retry(&user_req("[[retry:network,1]]")),
            Some((LlmErrorKind::Network, 1))
        );
        // KINDs outside the retryable subset are rejected — auth, usage_limit,
        // etc. don't reach the retry loop in the real runtime either, so the
        // mock refuses to fake them.
        assert_eq!(parse_retry(&user_req("[[retry:auth,1]]")), None);
        assert_eq!(parse_retry(&user_req("[[retry:bogus,1]]")), None);
        // N is clamped to MAX_RETRY_N.
        assert_eq!(
            parse_retry(&user_req("[[retry:rate_limit,9999]]")),
            Some((LlmErrorKind::RateLimit, MAX_RETRY_N))
        );
        assert_eq!(parse_retry(&user_req("no marker")), None);
    }

    #[test]
    fn retry_counter_increments_per_call() {
        // Two distinct cache_keys must not share a counter.
        let key_a = "conv-a";
        let key_b = "conv-b";
        assert_eq!(bump_retry_count(key_a), 1);
        assert_eq!(bump_retry_count(key_a), 2);
        assert_eq!(bump_retry_count(key_b), 1);
        assert_eq!(bump_retry_count(key_a), 3);
        assert_eq!(bump_retry_count(key_b), 2);
    }

    #[test]
    fn scenario_marker_overrides_hash() {
        // The hash for "[[scenario:plain_text]]" alone would not naturally
        // pick PlainText for every value; verify the marker forces it.
        let req = user_req("[[scenario:bash]] something something");
        let picked = Scenario::from_message(&req);
        assert!(matches!(picked, Scenario::BashToolCall));
    }

    #[test]
    fn perf_text_deterministic_and_sized() {
        // Same N -> byte-identical output (the fingerprint invariant).
        assert_eq!(perf_text(800), perf_text(800));
        // Exactly N whitespace-separated words.
        assert_eq!(perf_text(800).split_whitespace().count(), 800);
        assert_eq!(perf_text(3200).split_whitespace().count(), 3200);
        // Longer N strictly contains more bytes (scaling-curve precondition).
        assert!(perf_text(3200).len() > perf_text(800).len());
        assert_eq!(perf_text(0), "");
    }
}
