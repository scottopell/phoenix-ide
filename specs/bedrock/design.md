# Bedrock: Design Document

## Architecture Overview

Bedrock implements the Elm Architecture pattern: a pure state machine at the core with all I/O isolated in effect executors.

```
┌─────────────────────────────────────────────────────────────┐
│                        Runtime                               │
│  ┌─────────────────────────────────────────────────────┐    │
│  │                   Event Loop                         │    │
│  │                                                      │    │
│  │   Event ───▶ transition(state, event) ───▶ Effects  │    │
│  │     ▲              │                          │      │    │
│  │     │              ▼                          ▼      │    │
│  │     │         New State              Effect Executor │    │
│  │     │                                         │      │    │
│  │     └─────────────────────────────────────────┘      │    │
│  └─────────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────────┘
```

## State Machine (REQ-BED-001)

### Conversation States

```rust
enum ConvState {
    /// Ready for user input, no pending operations
    Idle,
    
    /// User message received, preparing LLM request
    AwaitingLlm,
    
    /// LLM request in flight, with retry tracking
    LlmRequesting { attempt: u32 },
    
    /// Executing tools serially (REQ-BED-004)
    ToolExecuting {
        current_tool_id: String,
        remaining_tool_ids: Vec<String>,
        completed_results: Vec<ToolResult>,
    },
    
    /// User requested cancellation, waiting for graceful completion
    Cancelling { pending_tool_id: Option<String> },
    
    /// Waiting for sub-agents to complete (REQ-BED-008)
    AwaitingSubAgents {
        pending_ids: Vec<String>,
        completed_results: Vec<SubAgentResult>,
    },
    
    /// Error occurred - UI displays this state directly (REQ-BED-006)
    Error { 
        message: String, 
        error_kind: ErrorKind,  // Auth, RateLimit, Network, Unknown
    },
}

/// Error classification for UI display (REQ-BED-006)
enum ErrorKind {
    Auth,           // 401, 403 - non-retryable
    RateLimit,      // 429 - was retried, exhausted
    Network,        // Timeout, connection - was retried, exhausted  
    InvalidRequest, // 400 - non-retryable
    Unknown,        // Other errors
}
```

### Events

```rust
enum Event {
    // User events (REQ-BED-002, REQ-BED-013)
    UserMessage { text: String, images: Vec<Image> },  // Images handled per REQ-BED-013
    UserCancel,  // REQ-BED-005
    
    // LLM events (REQ-BED-003, REQ-BED-006)
    LlmResponse { content: Vec<Content>, end_turn: bool, usage: Usage },
    LlmError { error: LlmError, attempt: u32 },
    LlmStreamChunk { content: Content },
    RetryTimeout,  // Scheduled retry timer fired
    
    // Tool events (REQ-BED-004)
    ToolComplete { tool_use_id: String, result: ToolResult },
    
    // Sub-agent events (REQ-BED-008, REQ-BED-009)
    SubAgentResult { agent_id: String, result: SubAgentResult },
    
}
```

### Effects

```rust
enum Effect {
    // Persistence (REQ-BED-007)
    PersistMessage { content: MessageContent, msg_type: MessageType },
    PersistState { new_state: ConvState },
    
    // LLM (REQ-BED-003)
    RequestLlm { messages: Vec<Message>, model: String },
    
    // Tools - serial execution (REQ-BED-004)
    ExecuteTool { tool_use_id: String, name: String, input: Value },
    
    // Sub-agents (REQ-BED-008)
    SpawnSubAgent { agent_id: String, prompt: String, model: String },
    
    // Client notifications (REQ-BED-011)
    NotifyClient { event_type: String, data: Value },
    
    // Scheduling (REQ-BED-006)
    ScheduleRetry { delay: Duration, attempt: u32 },
}
```

### Transition Function (REQ-BED-001)

```rust
fn transition(
    state: &ConvState,
    context: &ConvContext,
    event: Event,
) -> Result<TransitionResult, InvalidTransition>

struct TransitionResult {
    new_state: ConvState,
    effects: Vec<Effect>,
}

struct ConvContext {
    conversation_id: String,
    working_dir: PathBuf,  // REQ-BED-010: fixed at creation
    model_id: String,
    is_sub_agent: bool,    // REQ-BED-009
}
```

## Serial Tool Execution (REQ-BED-004)

Tools execute one at a time in LLM-requested order:

```rust
// When LLM responds with tool requests
LlmRequesting { .. } + LlmResponse { tools: [t1, t2, t3], .. } => {
    ToolExecuting {
        current_tool_id: t1.id,
        remaining_tool_ids: vec![t2.id, t3.id],
        completed_results: vec![],
    }
    // Effect: ExecuteTool { t1 }  -- only first tool
}

// When a tool completes, start next
ToolExecuting { remaining: [t2, t3], results } + ToolComplete { t1_result } => {
    ToolExecuting {
        current_tool_id: t2.id,
        remaining_tool_ids: vec![t3.id],
        completed_results: vec![t1_result],
    }
    // Effect: ExecuteTool { t2 }
}

// When last tool completes
ToolExecuting { remaining: [], results } + ToolComplete { last_result } => {
    AwaitingLlm
    // Effect: PersistMessage (all tool results), RequestLlm
}
```

## Cancellation with Synthetic Results (REQ-BED-005)

LLM APIs require tool_use to have matching tool_result. On cancellation:

```rust
// Cancellation during tool execution
ToolExecuting { current, remaining, completed } + UserCancel => {
    // Generate synthetic results for current + remaining tools
    let synthetic_current = ToolResult::Cancelled { 
        tool_use_id: current,
        message: "Cancelled by user" 
    };
    let synthetic_remaining: Vec<_> = remaining.iter().map(|id| {
        ToolResult::Cancelled { tool_use_id: id, message: "Skipped due to cancellation" }
    }).collect();
    
    Idle
    // Effects: 
    //   PersistMessage(synthetic results for all pending tools)
    //   NotifyClient
}
```

Message chain remains valid:
```
[agent: tool_use id=1, tool_use id=2, tool_use id=3]
[tool: result id=1 (completed)]
[tool: result id=2 (cancelled)]
[tool: result id=3 (skipped)]
```

## Error Handling and Retry (REQ-BED-006)

Retry logic is embedded in state machine, visible to UI:

```rust
// Retryable error with attempts remaining
LlmRequesting { attempt: 1 } + LlmError { retryable: true, .. } => {
    LlmRequesting { attempt: 2 }  // State reflects retry attempt
    // Effect: ScheduleRetry { delay: 1s, attempt: 2 }
    // Effect: NotifyClient { "retrying", attempt: 2 }
}

// Retry timer fires
LlmRequesting { attempt: 2 } + RetryTimeout => {
    LlmRequesting { attempt: 2 }  // Same state
    // Effect: RequestLlm
}

// Retries exhausted
LlmRequesting { attempt: 3 } + LlmError { retryable: true, kind } => {
    Error { message: "Failed after 3 attempts", error_kind: kind }
    // Effect: NotifyClient { "error", details }
}

// Non-retryable error - immediate failure
LlmRequesting { .. } + LlmError { retryable: false, kind: Auth } => {
    Error { message: "Authentication failed", error_kind: Auth }
}

// Recovery from error state
Error { .. } + UserMessage { .. } => {
    AwaitingLlm
    // Effect: PersistMessage, RequestLlm
}
```

## Mode Upgrade Tool (REQ-BED-015)

The `request_mode_upgrade` tool allows agents to request transition from Restricted to Unrestricted mode:

```rust
struct RequestModeUpgradeTool;

impl Tool for RequestModeUpgradeTool {
    fn name(&self) -> &str { "request_mode_upgrade" }
    
    fn schema(&self) -> Schema {
        json!({
            "type": "object",
            "required": ["reason"],
            "properties": {
                "reason": {
                    "type": "string",
                    "description": "Explanation of why write access is needed"
                }
            }
        })
    }
}
```

**Behavior:**
- Called in Unrestricted mode: Returns error "Already in Unrestricted mode"
- Called in Restricted mode: Pauses execution, awaits user decision, returns result after user responds
- Not available to sub-agents (REQ-BED-018)

---

## Sub-Agent Result Submission (REQ-BED-008, REQ-BED-009)

Sub-agents have a special tool to submit their final result:

```rust
/// Tool only available to sub-agents
struct SubmitResultTool;

impl Tool for SubmitResultTool {
    fn name(&self) -> &str { "submit_result" }
    
    fn schema(&self) -> Schema {
        json!({
            "type": "object",
            "required": ["result"],
            "properties": {
                "result": {
                    "type": "string",
                    "description": "Final result to return to parent conversation"
                },
                "success": {
                    "type": "boolean",
                    "description": "Whether the task completed successfully"
                }
            }
        })
    }
    
    async fn run(&self, input: Value) -> ToolResult {
        // This tool's execution triggers SubAgentResult event to parent
        // Sub-agent conversation transitions to Completed
        ToolResult::SubAgentComplete {
            result: input["result"].as_str().unwrap().to_string(),
            success: input["success"].as_bool().unwrap_or(true),
        }
    }
}
```

Sub-agent tool set:
```rust
fn sub_agent_tools() -> Vec<Tool> {
    vec![
        bash_tool(),
        patch_tool(),
        think_tool(),
        keyword_search_tool(),
        submit_result_tool(),  // Sub-agent only
        // NO spawn_sub_agent tool - prevents nesting
    ]
}
```

## State Transition Matrix

| Current State | Event | Next State | Effects |
|--------------|-------|------------|----------|
| Idle | UserMessage | AwaitingLlm | PersistMessage, PersistState |
| AwaitingLlm | (internal) | LlmRequesting{1} | RequestLlm |
| LlmRequesting | LlmResponse(end=true, no tools) | Idle | PersistMessage, NotifyClient |
| LlmRequesting | LlmResponse(tools) | ToolExecuting | PersistMessage, ExecuteTool(first) |
| LlmRequesting{n<3} | LlmError(retryable) | LlmRequesting{n+1} | ScheduleRetry, NotifyClient |
| LlmRequesting{n} | RetryTimeout | LlmRequesting{n} | RequestLlm |
| LlmRequesting{3} | LlmError(retryable) | Error | NotifyClient |
| LlmRequesting | LlmError(non-retryable) | Error | NotifyClient |
| LlmRequesting | UserCancel | Cancelling | PersistState |
| LlmRequesting | UserMessage | **REJECT** | Return error "agent is busy" |
| ToolExecuting(last) | ToolComplete | AwaitingLlm | PersistMessage |
| ToolExecuting | ToolComplete | ToolExecuting(next) | PersistMessage, ExecuteTool(next) |
| ToolExecuting | UserCancel | Idle | PersistMessage(synthetic), NotifyClient |
| ToolExecuting | UserMessage | **REJECT** | Return error "agent is busy" |
| Cancelling | LlmResponse | Idle | NotifyClient |
| Cancelling | UserMessage | **REJECT** | Return error "cancellation in progress" |
| Error | UserMessage | AwaitingLlm | PersistMessage, PersistState |
| Idle | SpawnSubAgents | AwaitingSubAgents | SpawnSubAgent×N |
| AwaitingSubAgents | SubAgentResult(last) | AwaitingLlm | PersistMessage |
| AwaitingSubAgents | UserMessage | **REJECT** | Return error "agent is busy" |

## Database Schema (REQ-BED-007)

```sql
CREATE TABLE conversations (
    id TEXT PRIMARY KEY,
    slug TEXT UNIQUE,
    cwd TEXT NOT NULL,                    -- REQ-BED-010: fixed at creation
    parent_conversation_id TEXT,          -- REQ-BED-009: NULL for user conversations
    user_initiated BOOLEAN NOT NULL,      -- REQ-BED-009: FALSE for sub-agents
    state TEXT NOT NULL DEFAULT 'idle',
    state_data TEXT,                       -- JSON: retry attempt, pending tools, etc.
    state_updated_at TIMESTAMP NOT NULL,
    created_at TIMESTAMP NOT NULL,
    updated_at TIMESTAMP NOT NULL,
    archived BOOLEAN NOT NULL DEFAULT FALSE,
    
    FOREIGN KEY (parent_conversation_id) 
        REFERENCES conversations(id) ON DELETE CASCADE
);

CREATE TABLE messages (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL,
    sequence_id INTEGER NOT NULL,
    message_type TEXT NOT NULL,           -- user, agent, tool, system, error
    actor_kind TEXT NOT NULL,             -- human, llm_agent, system
    content TEXT NOT NULL,                -- JSON
    display_data TEXT,                    -- JSON for UI rendering
    usage_data TEXT,                      -- JSON: token counts (REQ-BED-012)
    created_at TIMESTAMP NOT NULL,
    
    FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
);

```

## Runtime Event Loop

One runtime per active conversation:

```rust
impl ConversationRuntime {
    async fn run(&mut self) -> Result<()> {
        while let Some(event) = self.event_rx.recv().await {
            // REQ-BED-001: Pure transition
            let result = transition(&self.state, &self.context, event)?;
            
            // REQ-BED-007: Persist state first
            self.persist_state(&result.new_state).await?;
            self.state = result.new_state;
            
            // Execute effects serially (tools are already serial per REQ-BED-004)
            for effect in result.effects {
                self.executor.execute(effect, self.event_tx.clone()).await?;
            }
            
            // REQ-BED-011: State is streamed to clients
            
            if self.state.is_terminal() {
                break;
            }
        }
        Ok(())
    }
}
```

## Testing Strategy

### Property-Based Tests (REQ-BED-001)

```rust
#[proptest]
fn state_transitions_are_deterministic(state: ConvState, event: Event) {
    let ctx = test_context();
    let result1 = transition(&state, &ctx, event.clone());
    let result2 = transition(&state, &ctx, event);
    assert_eq!(result1, result2);
}

#[proptest]
fn cancellation_produces_synthetic_results_for_all_pending_tools(
    current: String,
    remaining: Vec<String>,
) {
    let state = ConvState::ToolExecuting { current, remaining: remaining.clone(), .. };
    let result = transition(&state, &test_context(), Event::UserCancel).unwrap();
    
    // Should have synthetic result for current + all remaining
    let persist_effects: Vec<_> = result.effects.iter()
        .filter(|e| matches!(e, Effect::PersistMessage { .. }))
        .collect();
    assert_eq!(persist_effects.len(), 1 + remaining.len());
}
```

### Integration Tests
- Full conversation flow: user message → LLM → tools (serial) → response
- Cancellation at each state with message chain verification
- Error recovery: retry exhaustion, non-retryable errors
- Sub-agent spawning, result submission, aggregation
- Server restart recovery

## Context Continuation (REQ-BED-019 through REQ-BED-024)

### Context Exhaustion Behavior

Conversations have a behavior mode that determines how context exhaustion is handled:

```rust
enum ContextExhaustionBehavior {
    /// Normal conversations: trigger continuation at 90% threshold
    ThresholdBasedContinuation,
    /// Sub-agents: fail immediately (no continuation flow)
    IntentionallyUnhandled,
}

struct ConvContext {
    // ... existing fields ...
    context_exhaustion_behavior: ContextExhaustionBehavior,
}
```

Set at conversation creation:
- User-initiated conversations: `ThresholdBasedContinuation`
- Sub-agents: `IntentionallyUnhandled`

### New States

```rust
enum ConvState {
    // ... existing states ...
    
    /// Awaiting continuation summary from LLM (tool-less request in flight)
    AwaitingContinuation {
        /// Tool calls that were requested but not executed
        rejected_tool_calls: Vec<ToolCall>,
        /// Usage data from the triggering response
        trigger_usage: UsageData,
        /// Retry attempt for the continuation request
        attempt: u32,
    },
    
    /// Context window exhausted - conversation is read-only
    ContextExhausted {
        /// The continuation summary
        summary: String,
        /// Final context usage when exhausted
        final_usage: UsageData,
    },
}
```

### New Events

```rust
enum Event {
    // ... existing events ...
    
    /// Continuation summary received from LLM
    ContinuationResponse {
        summary: String,
        usage: UsageData,
    },
    
    /// Continuation request failed after retries
    ContinuationFailed {
        error: String,
    },
    
    /// User manually triggered continuation (REQ-BED-023)
    UserTriggerContinuation,
}
```

### New Effects

```rust
enum Effect {
    // ... existing effects ...
    
    /// Request continuation summary from LLM (no tools)
    RequestContinuation {
        rejected_tool_calls: Vec<ToolCall>,
    },
    
    /// Notify client of context exhaustion
    NotifyContextExhausted {
        summary: String,
    },
}
```

### Threshold Check Location

The check happens in `transition()` at the `(LlmRequesting, LlmResponse)` arm, BEFORE entering `ToolExecuting`:

```rust
(ConvState::LlmRequesting { .. }, Event::LlmResponse { content, tool_calls, usage, .. }) => {
    let usage_data = usage_to_data(&usage);
    
    // REQ-BED-019: Check context threshold BEFORE tool execution
    if should_trigger_continuation(&usage_data, &context.model_info) {
        match context.context_exhaustion_behavior {
            ContextExhaustionBehavior::ThresholdBasedContinuation => {
                // Persist agent message (what LLM said), then request continuation
                return Ok(TransitionResult::new(ConvState::AwaitingContinuation {
                    rejected_tool_calls: tool_calls,
                    trigger_usage: usage_data.clone(),
                    attempt: 1,
                })
                .with_effect(Effect::persist_agent_message(content, Some(usage_data)))
                .with_effect(Effect::PersistState)
                .with_effect(Effect::RequestContinuation { rejected_tool_calls: tool_calls }));
            }
            ContextExhaustionBehavior::IntentionallyUnhandled => {
                // REQ-BED-024: Sub-agents fail immediately
                return Ok(TransitionResult::new(ConvState::Failed {
                    error: "Context window exhausted".to_string(),
                    error_kind: ErrorKind::ContextExhausted,
                })
                .with_effect(Effect::NotifyParent { 
                    outcome: SubAgentOutcome::Failure { 
                        error: "Context window exhausted".to_string(),
                        error_kind: ErrorKind::ContextExhausted,
                    }
                }));
            }
        }
    }
    
    // Normal flow continues...
    if tool_calls.is_empty() {
        // ...
    } else {
        // -> ToolExecuting
    }
}
```

### Continuation Prompt

The continuation prompt includes context about rejected tools:

```rust
const CONTINUATION_PROMPT: &str = r#"
The conversation context is nearly full. Please provide a brief continuation summary that could seed a new conversation.

Include:
1. Current task status (if any)
2. Key files or concepts discussed
3. Suggested next steps

Keep your response concise.
"#;

fn build_continuation_prompt(rejected_tool_calls: &[ToolCall]) -> String {
    let mut prompt = CONTINUATION_PROMPT.to_string();
    
    if !rejected_tool_calls.is_empty() {
        prompt.push_str("\n\nNote: The following tool calls were requested but not executed due to context limits:\n");
        for tool in rejected_tool_calls {
            prompt.push_str(&format!("- {}\n", tool.name()));
        }
    }
    
    prompt
}
```

### Continuation Transitions

```rust
// Continuation response received
(ConvState::AwaitingContinuation { trigger_usage, .. }, Event::ContinuationResponse { summary, usage }) => {
    let final_usage = UsageData {
        input_tokens: trigger_usage.input_tokens + usage.input_tokens,
        output_tokens: trigger_usage.output_tokens + usage.output_tokens,
    };
    
    Ok(TransitionResult::new(ConvState::ContextExhausted {
        summary: summary.clone(),
        final_usage,
    })
    .with_effect(Effect::persist_continuation_message(summary))
    .with_effect(Effect::PersistState)
    .with_effect(Effect::NotifyContextExhausted { summary }))
}

// Continuation failed after retries
(ConvState::AwaitingContinuation { trigger_usage, .. }, Event::ContinuationFailed { .. }) => {
    let fallback = "Context limit reached. The continuation summary could not be generated. \
                    Please start a new conversation.".to_string();
    
    Ok(TransitionResult::new(ConvState::ContextExhausted {
        summary: fallback.clone(),
        final_usage: trigger_usage,
    })
    .with_effect(Effect::persist_continuation_message(fallback.clone()))
    .with_effect(Effect::PersistState)
    .with_effect(Effect::NotifyContextExhausted { summary: fallback }))
}

// User cancels during continuation
(ConvState::AwaitingContinuation { trigger_usage, .. }, Event::UserCancel) => {
    let cancelled = "Continuation cancelled by user.".to_string();
    
    Ok(TransitionResult::new(ConvState::ContextExhausted {
        summary: cancelled.clone(),
        final_usage: trigger_usage,
    })
    .with_effect(Effect::persist_continuation_message(cancelled.clone()))
    .with_effect(Effect::PersistState)
    .with_effect(Effect::NotifyContextExhausted { summary: cancelled }))
}

// Context exhausted rejects user messages
(ConvState::ContextExhausted { .. }, Event::UserMessage { .. }) => {
    Err(TransitionError::ContextExhausted)
}
```

### Manual Continuation Trigger (REQ-BED-023)

Users can trigger continuation from Idle state when warning threshold (80%) is reached:

```rust
// User manually triggers continuation
(ConvState::Idle, Event::UserTriggerContinuation) => {
    Ok(TransitionResult::new(ConvState::AwaitingContinuation {
        rejected_tool_calls: vec![],  // No rejected tools for manual trigger
        trigger_usage: context.last_usage.clone(),
        attempt: 1,
    })
    .with_effect(Effect::PersistState)
    .with_effect(Effect::RequestContinuation { rejected_tool_calls: vec![] }))
}
```

### Constants

```rust
/// Threshold as fraction of context window (REQ-BED-019)
pub const CONTINUATION_THRESHOLD: f64 = 0.90;

/// Warning threshold for UI indicator (REQ-BED-023)
pub const WARNING_THRESHOLD: f64 = 0.80;

/// Threshold check function
fn should_trigger_continuation(usage: &UsageData, model: &ModelInfo) -> bool {
    let used = usage.input_tokens + usage.output_tokens;
    let threshold = (model.context_window as f64 * CONTINUATION_THRESHOLD) as u64;
    used >= threshold
}
```

### ErrorKind Extension

```rust
enum ErrorKind {
    // ... existing variants ...
    ContextExhausted,  // REQ-BED-024: For sub-agent context failure
}
```

### Database Changes

```sql
-- New message type for continuation summaries
-- (message_type TEXT already supports arbitrary values)
-- Use 'continuation' as the type

-- New conversation state
-- (state TEXT already supports arbitrary values)
-- Use 'context_exhausted' as the state
```

### State Transition Matrix Additions

| Current State | Event | Next State | Effects |
|--------------|-------|------------|----------|
| LlmRequesting | LlmResponse (>= 90%, ThresholdBased) | AwaitingContinuation | PersistMessage, PersistState, RequestContinuation |
| LlmRequesting | LlmResponse (>= 90%, Unhandled) | Failed | NotifyParent |
| AwaitingContinuation | ContinuationResponse | ContextExhausted | PersistMessage, PersistState, NotifyContextExhausted |
| AwaitingContinuation | ContinuationFailed | ContextExhausted | PersistMessage (fallback), PersistState, NotifyContextExhausted |
| AwaitingContinuation | UserCancel | ContextExhausted | PersistMessage (cancelled), PersistState, NotifyContextExhausted |
| ContextExhausted | UserMessage | **REJECT** | Return error "context exhausted" |
| ContextExhausted | * | ContextExhausted | No-op |
| Idle | UserTriggerContinuation | AwaitingContinuation | PersistState, RequestContinuation |

## File Organization

```
src/
├── state_machine/
│   ├── mod.rs
│   ├── state.rs          # ConvState, ConvContext, ErrorKind
│   ├── event.rs          # Event enum
│   ├── effect.rs         # Effect enum
│   ├── transition.rs     # Pure transition function
│   └── tests.rs          # Property tests
├── executor/
│   ├── mod.rs
│   ├── llm.rs            # LLM effect handler
│   ├── tool.rs           # Tool effect handler (serial)
│   ├── persistence.rs    # DB effect handler
│   ├── notification.rs   # Client notification handler
│   └── subagent.rs       # Sub-agent spawning
├── runtime/
│   ├── mod.rs
│   ├── loop.rs           # Event loop
│   └── manager.rs        # Conversation lifecycle
├── tools/
│   ├── mod.rs
│   ├── bash/
│   ├── patch/
│   ├── think.rs
│   ├── keyword_search.rs
│   └── submit_result.rs  # Sub-agent only
└── db/
    ├── mod.rs
    ├── schema.rs
    └── migrations/
```
