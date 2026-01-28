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
    
    /// LLM request in flight
    LlmRequesting,
    
    /// Executing one or more tools
    ToolExecuting {
        pending_tool_ids: Vec<String>,
        completed_results: Vec<ToolResult>,
    },
    
    /// User requested cancellation, waiting for graceful completion
    Cancelling,
    
    /// Waiting for sub-agents to complete (REQ-BED-008)
    AwaitingSubAgents {
        pending_ids: Vec<String>,
        completed_results: Vec<SubAgentResult>,
    },
    
    /// Unrecoverable error occurred
    Error { message: String, retryable: bool },
    
    /// Server restarted, need to resume
    RestartPending,
}
```

### Events

```rust
enum Event {
    // User events (REQ-BED-002)
    UserMessage { text: String, images: Vec<Image> },
    UserCancel,  // REQ-BED-005
    UserRetry,   // REQ-BED-006
    
    // LLM events (REQ-BED-003)
    LlmResponse { content: Vec<Content>, end_turn: bool, usage: Usage },
    LlmError { error: LlmError, attempt: u32 },
    LlmStreamChunk { content: Content },
    
    // Tool events (REQ-BED-004)
    ToolComplete { tool_use_id: String, result: ToolResult },
    ToolError { tool_use_id: String, error: String },
    
    // Sub-agent events (REQ-BED-008, REQ-BED-009)
    SubAgentComplete { agent_id: String, result: SubAgentResult },
    SubAgentError { agent_id: String, error: String },
    
    // System events (REQ-BED-007)
    ServerRestart,
    Timeout { reason: TimeoutReason },
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
    
    // Tools (REQ-BED-004)
    ExecuteTool { tool_use_id: String, name: String, input: Value },
    CancelTool { tool_use_id: String },
    
    // Sub-agents (REQ-BED-008)
    SpawnSubAgent { agent_id: String, prompt: String, model: String },
    
    // Client notifications (REQ-BED-011)
    NotifyClient { event_type: String, data: Value },
    
    // Scheduling
    ScheduleTimeout { delay: Duration, reason: TimeoutReason },
    CancelTimeout { reason: TimeoutReason },
}
```

### Transition Function (REQ-BED-001)

```rust
fn transition(
    state: &ConvState,
    context: &ConvContext,  // immutable conversation metadata
    event: Event,
) -> Result<TransitionResult, InvalidTransition>

struct TransitionResult {
    new_state: ConvState,
    effects: Vec<Effect>,
}

struct ConvContext {
    conversation_id: String,
    working_dir: PathBuf,  // REQ-BED-010
    model_id: String,
    is_sub_agent: bool,    // REQ-BED-009
}
```

## State Transition Matrix

| Current State | Event | Next State | Effects |
|--------------|-------|------------|----------|
| Idle | UserMessage | AwaitingLlm | PersistMessage, PersistState |
| AwaitingLlm | (internal) | LlmRequesting | RequestLlm |
| LlmRequesting | LlmResponse(end=true, no tools) | Idle | PersistMessage, PersistState, NotifyClient |
| LlmRequesting | LlmResponse(tools) | ToolExecuting | PersistMessage, ExecuteTool×N |
| LlmRequesting | LlmError(retryable, attempt<3) | AwaitingLlm | ScheduleTimeout(retry) |
| LlmRequesting | LlmError(attempt>=3) | Error | PersistState, NotifyClient |
| LlmRequesting | UserCancel | Cancelling | PersistState |
| ToolExecuting | ToolComplete(last) | AwaitingLlm | PersistMessage, PersistState |
| ToolExecuting | ToolComplete(not last) | ToolExecuting | PersistMessage |
| ToolExecuting | UserCancel | Cancelling | CancelTool×N |
| Cancelling | LlmResponse/ToolComplete | Idle | PersistState, NotifyClient |
| Error | UserRetry | AwaitingLlm | PersistState |
| Error | UserMessage | AwaitingLlm | PersistMessage, PersistState |
| * | UserMessage(while busy) | * (unchanged) | QueueFollowup |
| Idle | SpawnSubAgents | AwaitingSubAgents | SpawnSubAgent×N |
| AwaitingSubAgents | SubAgentComplete(last) | AwaitingLlm | PersistMessage |

## Database Schema (REQ-BED-007)

```sql
CREATE TABLE conversations (
    id TEXT PRIMARY KEY,
    slug TEXT UNIQUE,
    cwd TEXT NOT NULL,                    -- REQ-BED-010
    parent_conversation_id TEXT,          -- REQ-BED-009: NULL for user conversations
    user_initiated BOOLEAN NOT NULL,      -- REQ-BED-009: FALSE for sub-agents
    state TEXT NOT NULL DEFAULT 'idle',
    state_data TEXT,                       -- JSON for state-specific data
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

CREATE TABLE pending_followups (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL,
    sequence INTEGER NOT NULL,
    content TEXT NOT NULL,
    created_at TIMESTAMP NOT NULL,
    
    FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
);
```

## Effect Executor

The executor handles all I/O, converting effects into real operations and emitting events back to the state machine.

```rust
impl EffectExecutor {
    async fn execute(&self, effect: Effect, event_tx: Sender<Event>) -> Result<()> {
        match effect {
            Effect::RequestLlm { messages, model } => {
                // REQ-BED-003, REQ-BED-006
                let result = self.llm_client.complete(&messages, &model).await;
                match result {
                    Ok(response) => {
                        event_tx.send(Event::LlmResponse {
                            content: response.content,
                            end_turn: response.end_turn,
                            usage: response.usage,
                        }).await?;
                    }
                    Err(e) => {
                        event_tx.send(Event::LlmError {
                            error: e,
                            attempt: self.retry_count,
                        }).await?;
                    }
                }
            }
            Effect::ExecuteTool { tool_use_id, name, input } => {
                // REQ-BED-004
                let result = self.tool_registry.execute(&name, input).await;
                event_tx.send(Event::ToolComplete { tool_use_id, result }).await?;
            }
            Effect::SpawnSubAgent { agent_id, prompt, model } => {
                // REQ-BED-008, REQ-BED-009
                self.spawn_sub_agent(agent_id, prompt, model, event_tx).await?;
            }
            // ... other effects
        }
        Ok(())
    }
}
```

## Runtime Event Loop

One runtime per active conversation, managing the event loop:

```rust
impl ConversationRuntime {
    async fn run(&mut self) -> Result<()> {
        while let Some(event) = self.event_rx.recv().await {
            // REQ-BED-001: Pure transition
            let result = transition(&self.state, &self.context, event)?;
            
            // REQ-BED-007: Persist state first
            self.persist_state(&result.new_state).await?;
            self.state = result.new_state;
            
            // Execute effects (may spawn async tasks)
            for effect in result.effects {
                self.executor.execute(effect, self.event_tx.clone()).await?;
            }
            
            // REQ-BED-011: Notify clients
            self.notify_state_change().await?;
            
            if self.state.is_terminal() {
                break;
            }
        }
        Ok(())
    }
}
```

## Sub-Agent Architecture (REQ-BED-008, REQ-BED-009)

Sub-agents are independent conversations with:
- Own state machine instance
- Own working directory (inherited from parent or specified)
- No ability to spawn nested sub-agents (enforced in transition function)
- Lifecycle tied to parent - if parent cancels, sub-agents are cancelled

```rust
struct SubAgentConfig {
    prompt: String,
    model: String,
    working_dir: Option<PathBuf>,  // defaults to parent's cwd
    timeout: Duration,
}

// State machine enforces no nesting
fn transition(state: &ConvState, context: &ConvContext, event: Event) -> Result<TransitionResult> {
    if let Event::SpawnSubAgents { .. } = event {
        if context.is_sub_agent {
            return Err(InvalidTransition::SubAgentNestingNotAllowed);
        }
    }
    // ... rest of transition logic
}
```

## Error Handling Strategy (REQ-BED-006)

### Retryable Errors
- Network timeouts
- Rate limiting (429)
- Temporary service unavailability (503)

### Non-Retryable Errors  
- Authentication failures (401, 403)
- Invalid requests (400)
- Model not found (404)

### Retry Schedule
- Attempt 1: Immediate
- Attempt 2: 1 second delay
- Attempt 3: 3 seconds delay
- After 3 failures: Transition to Error state

## Testing Strategy

### Property-Based Tests (REQ-BED-001)

```rust
#[proptest]
fn terminal_states_produce_no_async_effects(event: Event) {
    for terminal in [ConvState::Error { .. }] {
        let result = transition(&terminal, &test_context(), event.clone());
        if let Ok(tr) = result {
            assert!(tr.effects.iter().all(|e| !e.is_async()));
        }
    }
}

#[proptest]
fn state_transitions_are_deterministic(state: ConvState, event: Event) {
    let ctx = test_context();
    let result1 = transition(&state, &ctx, event.clone());
    let result2 = transition(&state, &ctx, event);
    assert_eq!(result1, result2);
}
```

### Integration Tests
- Full conversation flow: user message → LLM → tools → response
- Cancellation at each state
- Error recovery scenarios
- Sub-agent spawning and completion
- Server restart recovery

## File Organization

```
src/
├── state_machine/
│   ├── mod.rs
│   ├── state.rs          # ConvState, ConvContext
│   ├── event.rs          # Event enum
│   ├── effect.rs         # Effect enum
│   ├── transition.rs     # Pure transition function
│   └── tests.rs          # Property tests
├── executor/
│   ├── mod.rs
│   ├── llm.rs            # LLM effect handler
│   ├── tool.rs           # Tool effect handler
│   ├── persistence.rs    # DB effect handler
│   ├── notification.rs   # Client notification handler
│   └── subagent.rs       # Sub-agent spawning
├── runtime/
│   ├── mod.rs
│   ├── loop.rs           # Event loop
│   └── manager.rs        # Conversation lifecycle
└── db/
    ├── mod.rs
    ├── schema.rs
    └── migrations/
```
