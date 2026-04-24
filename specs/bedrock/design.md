# Bedrock: Design Document

## Architecture Overview

Bedrock implements the Elm Architecture pattern: a pure state machine at the core with
all I/O isolated in effect executors. The SM has two pure entry points — one for user
events, one for executor outcomes — both returning `(new_state, effects)`.

```
┌──────────────────────────────────────────────────────────────────┐
│                           Runtime                                │
│                                                                  │
│  User API ──▶ handle_user_event(state, event)                    │
│                           │                                      │
│                     (new_state, effects)                          │
│                           │                                      │
│                           ▼                                      │
│                    ┌─────────────┐                                │
│                    │  New State  │──────▶ BroadcastState (SSE)    │
│                    └─────────────┘                                │
│                           │                                      │
│                     dispatch effects                             │
│                       │    │    │                                 │
│                       ▼    ▼    ▼                                 │
│              ┌─────┐ ┌────┐ ┌──────┐                             │
│              │ LLM │ │Tool│ │Persist│  (background tasks)        │
│              └──┬──┘ └─┬──┘ └──┬───┘                             │
│                 │      │       │                                  │
│           oneshot<LlmOutcome>  │  oneshot<PersistOutcome>         │
│                 │  oneshot<ToolOutcome>                           │
│                 │      │       │                                  │
│                 ▼      ▼       ▼                                  │
│          handle_outcome(state, outcome) ─┐                       │
│                 │                        │                        │
│           (new_state, effects)    Err: log + discard              │
│                 │                                                 │
│                 └──────▶ (loop)                                   │
└──────────────────────────────────────────────────────────────────┘
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
    /// assistant_message is held here, NOT yet persisted — persistence is atomic
    /// at the end of the tool round via CheckpointData::ToolRound (see Persistence Model)
    ToolExecuting {
        assistant_message: AssistantMessage,  // held, not yet persisted
        current_tool: ToolUse,
        remaining_tools: Vec<ToolUse>,
        completed_results: Vec<ToolResult>,
        pending_sub_agents: Vec<String>,      // accumulated from spawn_agents
    },

    /// User requested cancellation, waiting for graceful completion
    Cancelling { pending_tool_id: Option<String> },

    /// Waiting for sub-agents to complete (REQ-BED-008, REQ-SA-004)
    AwaitingSubAgents {
        pending_ids: Vec<String>,
        completed_results: Vec<SubAgentResult>,
        spawn_tool_id: ToolUseId,
        deadline: Instant,                    // REQ-BED-026: mandatory timeout
    },

    /// Cancelling sub-agents (REQ-SA-005)
    CancellingSubAgents {
        pending_ids: Vec<String>,
        completed_results: Vec<SubAgentResult>,
    },

    /// Error occurred - UI displays this state directly (REQ-BED-006)
    Error {
        message: String,
        error_kind: ErrorKind,
    },

    // --- Context continuation states (REQ-BED-019 through REQ-BED-024) ---
    // AwaitingContinuation, ContextExhausted — see Context Continuation section

    // --- Sub-agent terminal states (REQ-SA-003) ---
    // Completed, Failed — see Sub-Agent design
}

/// Error classification for UI display (REQ-BED-006, REQ-LLM-006)
/// No Unknown variant. No catch-all. Adding a new error class requires
/// adding a variant and handling it — the compiler forces it.
enum ErrorKind {
    Auth,           // 401, 403 - non-retryable
    RateLimit,      // 429 - was retried, exhausted
    Network,        // Timeout, connection - was retried, exhausted
    ServerError,    // 5xx - was retried, exhausted
    InvalidRequest, // 400 - non-retryable
    ContextExhausted, // REQ-BED-024
    TimedOut,       // REQ-BED-026: sub-agent timeout
    Cancelled,      // Explicit cancellation
}
```

### User Events

User-initiated events enter the SM through the API layer. These are the only events
not produced by the executor:

```rust
enum UserEvent {
    Message { text: String, images: Vec<Image> },  // REQ-BED-002, REQ-BED-013
    Cancel,                                         // REQ-BED-005
    TriggerContinuation,                            // REQ-BED-023
}
```

### Typed Effects with Oneshot Channels

Effects carry a `oneshot::Sender` for their expected outcome type. The executor runs
the background work and sends the result back on the channel. The channel's type
constrains what can come back — you cannot send an `LlmOutcome` down a
`Sender<ToolOutcome>`.

```rust
enum Effect {
    // LLM request (REQ-BED-003, REQ-BED-025)
    RequestLlm {
        request: LlmRequest,
        reply: oneshot::Sender<LlmOutcome>,
        token_sink: broadcast::Sender<TokenChunk>,  // fire-and-forget streaming
    },

    // Tool execution - serial (REQ-BED-004)
    ExecuteTool {
        invocation: ToolInvocation,
        reply: oneshot::Sender<ToolOutcome>,
    },

    // Sub-agent spawning (REQ-BED-008, REQ-SA-001)
    SpawnSubAgent {
        config: SubAgentConfig,
        reply: oneshot::Sender<SubAgentOutcome>,
    },

    // Atomic persistence (REQ-BED-007)
    PersistCheckpoint {
        data: CheckpointData,
        reply: oneshot::Sender<PersistOutcome>,
    },

    // Fire-and-forget effects (no reply expected)
    StreamToken { chunk: String, request_id: RequestId },  // REQ-BED-025
    BroadcastState { snapshot: StateSnapshot },             // REQ-BED-011
    ScheduleRetry { delay: Duration, attempt: u32 },        // REQ-BED-006
    CancelSubAgents { ids: Vec<String> },                   // REQ-SA-005
}
```

### Typed Outcome Enums

Each outcome type is exhaustive — no `Unknown`, no `_ =>` match arms. Adding a new
variant is a compile error at every handler site.

```rust
/// Returned by executor LLM task via oneshot channel
enum LlmOutcome {
    Response(AssistantMessage, TokenUsage),
    RateLimited { retry_after: Option<Duration> },
    ServerError { status: u16, body: String },
    NetworkError { message: String },
    TokenBudgetExceeded { partial: Option<AssistantMessage> },
    Cancelled,
}

/// Returned by executor tool task via oneshot channel
enum ToolOutcome {
    Completed(ToolResult),
    Aborted { tool_use_id: ToolUseId, reason: AbortReason },
    Failed { tool_use_id: ToolUseId, error: String },
}

/// AbortReason is set by the component requesting cancellation,
/// never inferred from output content (FM-1 prevention)
enum AbortReason {
    CancellationRequested,
    Timeout,
    ParentCancelled,
}

/// Returned by executor sub-agent task via oneshot channel
enum SubAgentOutcome {
    Success { result: String },
    Failure { error: String, error_kind: ErrorKind },
    TimedOut,
}

/// Returned by executor persistence task via oneshot channel
enum PersistOutcome {
    Ok,
    Failed { error: String },
}

/// Streamed chunks during LLM generation (for token_sink broadcast)
enum TokenChunk {
    Text(String),
    ToolUseStart { tool_use_id: ToolUseId, tool_name: String },
    ToolUseInput { tool_use_id: ToolUseId, partial_json: String },
    ToolUseDone { tool_use_id: ToolUseId },
}
```

### Transition Functions (REQ-BED-001)

The SM has two pure entry points. Both are `(state, input) -> (state, effects)` with
no I/O:

```rust
/// Entry point 1: User-initiated events (from API layer)
fn handle_user_event(
    state: &ConvState,
    context: &ConvContext,
    event: UserEvent,
) -> Result<TransitionResult, TransitionError>

/// Entry point 2: Executor outcomes (from background tasks)
/// This is the executor boundary — the second layer of defense.
/// Even with typed channels constraining what CAN arrive, this function
/// rejects outcomes that are invalid for the current state.
fn handle_outcome(
    state: &ConvState,
    context: &ConvContext,
    outcome: EffectOutcome,
) -> Result<TransitionResult, InvalidOutcome>

/// Union type for all outcomes the executor can produce.
/// The executor constructs this from the typed oneshot channel result.
enum EffectOutcome {
    Llm(LlmOutcome),
    Tool(ToolOutcome),
    SubAgent(SubAgentOutcome),
    Persist(PersistOutcome),
    RetryTimeout,
}

struct TransitionResult {
    new_state: ConvState,
    effects: Vec<Effect>,
}

/// InvalidOutcome carries both the rejected outcome and the current state,
/// enabling the executor to log and discard without corrupting state.
struct InvalidOutcome {
    outcome: EffectOutcome,
    state: ConvState,
    reason: String,
}

struct ConvContext {
    conversation_id: String,
    working_dir: PathBuf,       // REQ-BED-010: fixed at creation
    model_id: String,
    is_sub_agent: bool,         // REQ-BED-009
    context_exhaustion_behavior: ContextExhaustionBehavior,
}
```

**Two layers of defense:**

1. **Typed channels** constrain what the executor CAN produce. A `Sender<ToolOutcome>`
   physically cannot send an `LlmOutcome`. This prevents FM-1 at the structural level.

2. **`handle_outcome()` returning `Result`** rejects outcomes that are invalid for the
   current state. If a `ToolOutcome::Aborted` arrives while SM is in `ToolExecuting`
   without a cancellation in flight, `handle_outcome` returns `Err` and the executor
   logs and discards it. State is unchanged. This is the safety net for edge cases
   the type system cannot catch (e.g., race between cancellation and completion).

## Serial Tool Execution (REQ-BED-004)

Tools execute one at a time in LLM-requested order. The assistant message is held in
state (not persisted) until all tools complete:

```rust
// When LLM responds with tool requests (via handle_outcome)
LlmRequesting + LlmOutcome::Response(msg with tools [t1, t2, t3]) => {
    ToolExecuting {
        assistant_message: msg,  // held, NOT persisted yet
        current_tool: t1,
        remaining_tools: vec![t2, t3],
        completed_results: vec![],
        pending_sub_agents: vec![],
    }
    // Effect: ExecuteTool { t1, reply: oneshot }
}

// When a tool completes, start next (via handle_outcome)
ToolExecuting { remaining: [t2, t3], results } + ToolOutcome::Completed(t1_result) => {
    ToolExecuting {
        current_tool: t2,
        remaining_tools: vec![t3],
        completed_results: vec![t1_result],
        ..  // assistant_message, pending_sub_agents carried forward
    }
    // Effect: ExecuteTool { t2, reply: oneshot }
}

// When last tool completes — atomic persistence
ToolExecuting { remaining: [], assistant_message, results }
    + ToolOutcome::Completed(last_result) => {
    AwaitingLlm
    // Effect: PersistCheckpoint(ToolRound { assistant_message, all_results })
    // Effect: RequestLlm { reply: oneshot, token_sink: broadcast }
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

Retry logic is embedded in state machine, visible to UI. The `handle_outcome` function
maps `LlmOutcome` variants to retry/fail decisions via an exhaustive match:

```rust
/// Total function — every LlmOutcome variant has an explicit handler.
/// No _ arm. Adding a new variant is a compile error here.
fn map_llm_outcome_to_transition(
    outcome: LlmOutcome,
    attempt: u32,
    max_retries: u32,
) -> TransitionResult {
    match outcome {
        LlmOutcome::Response(msg, usage) => { /* -> process response */ }
        LlmOutcome::RateLimited { retry_after } => {
            if attempt < max_retries {
                /* -> LlmRequesting { attempt + 1 }, ScheduleRetry */
            } else {
                /* -> Error { error_kind: RateLimit } */
            }
        }
        LlmOutcome::ServerError { status, .. } => {
            if attempt < max_retries {
                /* -> LlmRequesting { attempt + 1 }, ScheduleRetry */
            } else {
                /* -> Error { error_kind: ServerError } */
            }
        }
        LlmOutcome::NetworkError { .. } => {
            if attempt < max_retries {
                /* -> LlmRequesting { attempt + 1 }, ScheduleRetry */
            } else {
                /* -> Error { error_kind: Network } */
            }
        }
        LlmOutcome::TokenBudgetExceeded { .. } => {
            /* -> AwaitingContinuation or Failed (sub-agent) */
        }
        LlmOutcome::Cancelled => {
            /* -> Idle */
        }
        // No catch-all. Every future variant must be handled explicitly.
    }
}
```

Recovery from error state remains unchanged:
```rust
// User sends message from Error state
Error { .. } + UserMessage { .. } => AwaitingLlm
    // Effects: PersistCheckpoint, RequestLlm
```

## Conversation Mode (REQ-BED-027, REQ-PROJ-002, REQ-PROJ-007)

Conversation mode is stored as a field on the conversation record alongside the state
machine state. It is NOT embedded inside `ConvState` variants — mode is
conversation-level identity that persists across all state transitions.

```rust
enum ConvMode {
    Explore {
        pinned_commit: String,  // SHA of main HEAD when conversation was created
    },
    Work {
        worktree_path: PathBuf, // .phoenix/worktrees/{conv-id}/
        branch: String,         // phoenix/{task-id}-{slug}
        task_id: String,
    },
}
```

Persisted as JSON in `conversations.conv_mode TEXT NOT NULL DEFAULT '{"Explore":{}}'`.

The executor reads `ConvMode` at the start of each turn and configures the tool
registry accordingly. Tool configuration is never derived from ConvState variants —
only from `ConvMode`. This ensures mode survives state machine transitions without
requiring every state variant to carry mode.

**Tool registry configuration by mode:**

| Tool | Explore | Work |
|------|---------|------|
| `patch` | Disabled | Enabled (worktree only) |
| `bash` | Allowed (read-only enforced) | Allowed (write in worktree) |
| `propose_plan` | Allowed (intercepted, not executed) | Disabled |
| `think`, `keyword_search`, `read_image`, `browser_*` | Allowed | Allowed |

## Task Approval State (REQ-BED-028, REQ-PROJ-003, REQ-PROJ-004)

A new state handles the human review loop for task plans:

```rust
AwaitingTaskApproval {
    task_id: String,
    task_path: PathBuf,
    reply: oneshot::Sender<TaskApprovalOutcome>,
}

enum TaskApprovalOutcome {
    Approved,
    Rejected,
    FeedbackProvided { annotations: String },
}
```

Transitions:

```
LlmRequesting + LlmResponse(propose_plan) → AwaitingTaskApproval
    Effects: PersistCheckpoint(ToolRound), BroadcastState
    Note: intercepted at LlmResponse like submit_result, never enters ToolExecuting

AwaitingTaskApproval + Approved → Idle (mode becomes Work)
    Effects: CommitTaskFile, CreateBranch, CheckoutBranch, PersistMode

AwaitingTaskApproval + FeedbackProvided → Idle (mode stays Explore)
    Effects: (annotations delivered as user message; agent may call propose_plan again)

AwaitingTaskApproval + Rejected → Idle (mode stays Explore)
    Effects: (no git operations — nothing was written to disk)
```

On server restart with conversation in `AwaitingTaskApproval`: restore state from DB,
re-emit `SSE::TaskApprovalRequested` to reconnecting clients.

## Task Completion and Abandon (REQ-BED-029, REQ-PROJ-009, REQ-PROJ-010)

There is no `AwaitingMergeApproval` state. Task completion and abandonment are
user-initiated actions dispatched to the executor, not state machine transitions.

The conversation must be in `Idle` state (Work mode) for these actions to be available.
If the agent is working, the user must cancel first.

**Complete action:** The executor runs pre-checks, generates a commit message via LLM,
shows a confirmation dialog, and on confirm executes the squash merge sequence
(see `specs/projects/design.md` REQ-PROJ-009 section for full flow). On success,
the conversation transitions to `Terminal`.

**Abandon action:** The executor shows a confirmation dialog, and on confirm deletes
the worktree and branch, updates the task file to `wont-do` on base_branch, and
transitions the conversation to `Terminal`.

Both actions use the existing `Terminal` state -- no new `ConvState` variant is needed.
The executor dispatches the cleanup effects and feeds back the terminal transition
through `handle_outcome()`.

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

User events use `handle_user_event()`. Outcomes use `handle_outcome()`.

### User Event Transitions

| Current State | User Event | Next State | Effects |
|--------------|-----------|------------|----------|
| Idle | Message | AwaitingLlm | PersistCheckpoint(UserMessage), RequestLlm |
| Idle | TriggerContinuation | AwaitingContinuation | PersistCheckpoint(StateOnly), RequestContinuation |
| Error | Message | AwaitingLlm | PersistCheckpoint(UserMessage), RequestLlm |
| LlmRequesting | Cancel | Cancelling | — (LLM request abort is an executor concern) |
| ToolExecuting | Cancel | Idle | PersistCheckpoint(ToolRound with synthetics), BroadcastState |
| AwaitingSubAgents | Cancel | CancellingSubAgents | CancelSubAgents |
| AwaitingContinuation | Cancel | ContextExhausted | PersistCheckpoint(ContinuationSummary) |
| (any busy state) | Message | **REJECT** | "agent is busy" |

### Outcome Transitions

| Current State | Outcome | Next State | Effects |
|--------------|---------|------------|----------|
| LlmRequesting | LlmOutcome::Response (text only, end_turn) | Idle | PersistCheckpoint(PlainAssistant), BroadcastState |
| LlmRequesting | LlmOutcome::Response (tools) | ToolExecuting | ExecuteTool(first) |
| LlmRequesting{n<3} | LlmOutcome::RateLimited/ServerError/NetworkError | LlmRequesting{n+1} | ScheduleRetry, BroadcastState |
| LlmRequesting{3} | LlmOutcome::RateLimited/ServerError/NetworkError | Error | BroadcastState |
| LlmRequesting | LlmOutcome::Cancelled | Idle | BroadcastState |
| LlmRequesting | RetryTimeout | LlmRequesting | RequestLlm |
| ToolExecuting | ToolOutcome::Completed (more remaining) | ToolExecuting(next) | ExecuteTool(next) |
| ToolExecuting | ToolOutcome::Completed (last, no sub-agents) | AwaitingLlm | PersistCheckpoint(ToolRound), RequestLlm |
| ToolExecuting | ToolOutcome::Completed (last, has sub-agents) | AwaitingSubAgents | PersistCheckpoint(ToolRound) |
| ToolExecuting | SpawnAgentsComplete | ToolExecuting(next) | SpawnSubAgent x N, ExecuteTool(next) |
| AwaitingSubAgents | SubAgentOutcome (more pending) | AwaitingSubAgents | — |
| AwaitingSubAgents | SubAgentOutcome (last) | AwaitingLlm | PersistCheckpoint, RequestLlm |
| AwaitingSubAgents | SubAgentOutcome::TimedOut (last) | AwaitingLlm | PersistCheckpoint, RequestLlm |
| CancellingSubAgents | SubAgentOutcome (more pending) | CancellingSubAgents | — |
| CancellingSubAgents | SubAgentOutcome (last) | Idle | BroadcastState |
| Cancelling | LlmOutcome::Response/Cancelled | Idle | BroadcastState |
| AwaitingContinuation | ContinuationResponse | ContextExhausted | PersistCheckpoint(ContinuationSummary) |
| AwaitingContinuation | ContinuationFailed | ContextExhausted | PersistCheckpoint(ContinuationSummary, fallback) |
| (any terminal state) | (any outcome) | **REJECT** | InvalidOutcome (logged and discarded) |

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

## Persistence Model (REQ-BED-007, FM-2 Prevention)

The key design tension: *when* do tool exchanges get written to SQLite? The answer:
atomically, at the end of the tool round, with both sides guaranteed present.

### CheckpointData

`db::persist_checkpoint()` accepts only `CheckpointData`. The half-written history
state is structurally unrepresentable:

```rust
enum CheckpointData {
    /// A complete tool round: assistant message + all tool results.
    /// Constructor enforces matching counts — cannot be created with
    /// mismatched tool_use/tool_result pairs.
    ToolRound {
        assistant_message: AssistantMessage,  // contains tool_use blocks
        tool_results: Vec<ToolResult>,        // same length as tool_uses
    },

    /// Plain text response (no tools)
    PlainAssistant { message: AssistantMessage },

    /// User message
    UserMessage { text: String, images: Vec<Image> },

    /// Continuation summary (REQ-BED-020)
    ContinuationSummary { summary: String },

    /// State-only checkpoint (no new messages)
    StateOnly { state: ConvState },
}

impl CheckpointData {
    /// ToolRound constructor enforces the atomicity invariant.
    pub fn tool_round(
        assistant_message: AssistantMessage,
        tool_results: Vec<ToolResult>,
    ) -> Result<Self, PersistError> {
        let tool_use_count = assistant_message.tool_uses().len();
        if tool_use_count != tool_results.len() {
            return Err(PersistError::ResultCountMismatch {
                tool_uses: tool_use_count,
                results: tool_results.len(),
            });
        }
        Ok(Self::ToolRound { assistant_message, tool_results })
    }
}
```

### Persistence Flow

The assistant message is held in `ToolExecutingState.assistant_message` — NOT persisted
on LLM response. Only when all tools complete does the SM emit:

```
ToolExecuting { remaining: [], assistant_message, completed_results }
    + ToolOutcome::Completed(last_result)
    → AwaitingLlm
    + Effect::PersistCheckpoint(CheckpointData::tool_round(assistant_message, all_results))
    + Effect::RequestLlm
```

On crash during tool execution: DB has no half-written `tool_use` without matching
`tool_result`. The assistant message was never persisted. The conversation resumes from
idle with the last consistent state.

### No Parallel Representations (FM-4 Prevention)

`ToolExecutingState.completed_results` is the single source of truth for what has been
completed. There is no `persisted_tool_ids: HashSet` tracking a parallel "what has been
persisted" fact. Persistence is a single atomic write at the end, not a running tally.

## Runtime Event Loop (REQ-BED-001)

One runtime per active conversation:

```rust
impl ConversationRuntime {
    async fn run(&mut self) -> Result<()> {
        loop {
            // Select across all active channels: user events, outcome receivers,
            // and retry timers. Only channels for in-flight effects are polled.
            let input = self.select_next_input().await;

            // Route to the appropriate pure transition function
            let result = match input {
                RuntimeInput::User(event) => {
                    handle_user_event(&self.state, &self.context, event)
                }
                RuntimeInput::Outcome(outcome) => {
                    match handle_outcome(&self.state, &self.context, outcome) {
                        Ok(r) => Ok(r),
                        Err(invalid) => {
                            // Log and discard — state unchanged, conversation safe.
                            // This is the safety net for race conditions the type
                            // system cannot prevent.
                            tracing::warn!(?invalid, "rejected invalid outcome");
                            continue;
                        }
                    }
                }
            }?;

            // Apply transition
            self.state = result.new_state;

            // Dispatch effects — each spawns a background task that will
            // eventually produce an outcome on its oneshot channel
            for effect in result.effects {
                self.dispatch_effect(effect).await;
            }

            // REQ-BED-011: Broadcast state to SSE clients
            self.broadcast_state().await;

            // FM-5 prevention: terminal states exit the loop explicitly
            if let StepResult::Terminal(outcome) = self.state.step_result() {
                self.runtime_manager.notify_completion(
                    self.context.conversation_id.clone(),
                    outcome,
                ).await;
                self.cleanup().await;
                return Ok(());
            }
        }
    }
}

/// Executor lifecycle signal — forces explicit handling of terminal states
enum StepResult {
    Continue,
    Terminal(TerminalOutcome),
}

enum TerminalOutcome {
    Completed(String),               // Sub-agent success (REQ-SA-003)
    Failed(String, ErrorKind),       // Sub-agent failure
    ContextExhausted { summary: String }, // REQ-BED-021
}

impl ConvState {
    fn step_result(&self) -> StepResult {
        match self {
            ConvState::Completed { result } =>
                StepResult::Terminal(TerminalOutcome::Completed(result.clone())),
            ConvState::Failed { error, error_kind } =>
                StepResult::Terminal(TerminalOutcome::Failed(error.clone(), *error_kind)),
            ConvState::ContextExhausted { summary, .. } =>
                StepResult::Terminal(TerminalOutcome::ContextExhausted {
                    summary: summary.clone(),
                }),
            _ => StepResult::Continue,
        }
    }
}
```

## Token Streaming Architecture (REQ-BED-025)

Tokens are fire-and-forget — they pass through the effect system but do NOT cause state
transitions. The SM stays in `LlmRequesting` throughout streaming. This is a deliberate
decision: tokens carry no control-flow significance, and making them state transitions
would either (a) trigger `PersistCheckpoint` on every token (massive churn) or (b)
require special-casing "don't persist on token transitions" which undermines the
persistence invariant.

### Data Flow

```
Provider HTTP stream
  │
  ├─ text delta ──▶ Effect::StreamToken ──▶ broadcast_tx ──▶ SSE token event ──▶ UI
  │                 (fire-and-forget)
  └─ accumulate tool input JSON
       │
       └─ stream complete ──▶ LlmOutcome::Response ──▶ oneshot channel ──▶ handle_outcome
                                                                               │
                                                          PersistCheckpoint ◀──┘
                                                               │
                                                          SSE message event ──▶ UI (final)
```

### Why Not SM State?

This was the central streaming design question in the architecture review (see
Appendix A). Four of five independent proposals chose fire-and-forget. The argument:

- **Pro SM state (Nguyen):** Mid-stream accumulated text is in canonical state. UI reads
  state directly. No parallel representation.
- **Pro fire-and-forget (consensus):** Tokens have no control-flow significance. SM makes
  no decisions based on individual tokens. Putting them in state adds surface area to
  `transition()` with no behavioral benefit. The "parallel representation" risk is bounded:
  the SM is always authoritative about what phase the conversation is in (`LlmRequesting`),
  and the UI clears streaming text on state transition to the final message.

The SM is always the source of truth for *phase*. The token stream is display-only data
that flows through the effect system and dies on state transition.

### SSE Merge Point

The SSE broadcast channel carries two merged streams:

```rust
select! {
    chunk = token_rx.recv() => sse_send(SseEvent::Token { text: chunk, request_id }),
    snapshot = state_rx.recv() => sse_send(SseEvent::StateChange(snapshot)),
}
```

Every state transition emits `BroadcastState`. The UI never infers state from token
content. `request_id` on token events lets the UI correlate chunks to the correct
in-flight request and detect stale tokens from a previous request.

## Testing Strategy

### Property-Based Tests — Pure Transition Functions

The existing property tests for `transition()` remain. The critical extension is testing
`handle_outcome()` — the executor boundary where every historical bug lived.

#### Suite 1: Outcome Never Corrupts State

```rust
proptest! {
    fn executor_outcomes_never_corrupt_state(
        initial_state in arb_conv_state(),
        outcomes in prop::collection::vec(arb_effect_outcome(), 1..50),
    ) {
        let ctx = test_context();
        let mut state = initial_state;
        for outcome in outcomes {
            match handle_outcome(&state, &ctx, &outcome) {
                Ok((next, _effects)) => {
                    assert_valid_state_invariants(&next);
                    state = next;
                }
                Err(_) => { /* rejected — state unchanged, correct behavior */ }
            }
        }
    }
}
```

#### Suite 2: Checkpoint Always Carries Matched Sides

```rust
proptest! {
    fn checkpoint_tool_round_has_matching_results(
        msg in arb_assistant_message_with_tool_uses(1..5),
        results in prop::collection::vec(arb_tool_result(), 1..5),
    ) {
        // Drive SM through ToolExecuting to completion.
        // Assert every PersistCheckpoint::ToolRound effect has
        // tool_uses.len() == tool_results.len()
    }
}
```

#### Suite 3: Adapter Mapping Functions Are Total

```rust
proptest! {
    fn llm_outcome_mapping_is_total(
        outcome in arb_llm_outcome(),
        attempt in 0u32..3,
    ) {
        // This test is structurally guaranteed by the exhaustive match
        // in map_llm_outcome_to_transition — but the proptest documents
        // the intent and catches regressions if someone adds a variant.
        let _ = map_llm_outcome_to_transition(outcome, attempt, 3);
    }
}
```

#### Suite 4: Terminal States Emit No Further Effects

```rust
proptest! {
    fn terminal_states_are_absorbing(
        outcome in arb_effect_outcome(),
    ) {
        let completed = ConvState::Completed { result: "done".into() };
        assert!(handle_outcome(&completed, &ctx, &outcome).is_err());

        let exhausted = ConvState::ContextExhausted { summary: "...".into(), .. };
        assert!(handle_outcome(&exhausted, &ctx, &outcome).is_err());
    }
}
```

#### Suite 5: Sub-Agent Fan-In Conservation (REQ-SA-004)

```rust
proptest! {
    fn subagent_count_conserved(
        initial_ids in prop::collection::vec("[a-z]{8}", 1..5),
        outcomes in ...,
    ) {
        // pending_ids.len() + completed_results.len() == N (constant)
        // through all transitions in AwaitingSubAgents/CancellingSubAgents
    }
}
```

### Integration Tests

- Full conversation flow: user message → LLM → tools (serial) → response
- Cancellation at each state with message chain verification
- Error recovery: retry exhaustion, non-retryable errors
- Sub-agent spawning, result submission, aggregation, timeout
- Server restart recovery
- Token streaming: chunks arrive, final message replaces them

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

## Context Continuation Worktree Transfer (REQ-BED-030, REQ-BED-031)

### Problem

When a Work- or Branch-mode conversation hits context exhaustion mid-task,
the user's environment — branch, worktree, uncommitted changes — needs to
survive the handoff to a continuation conversation. The prior design
destroyed the worktree on the terminal transition (Work demoted to Explore,
Branch demoted to Direct; branch preserved in git but the conversation
record lost its `branch_name` field). The continuation flow had no
mechanism to reach the preserved branch, and uncommitted changes were lost
to `git worktree remove --force`.

### Design

Worktree cleanup is removed from the automatic context-exhausted terminal
transition. Context-exhausted is a paused state: the worktree persists
until the user takes an explicit action (continue, abandon, or
mark-as-merged on the continuation chain's tail).

When the user clicks Continue on a context-exhausted parent:

1. A new conversation record is created. It inherits the parent's
   `conv_mode` (Work → Work, Branch → Branch, Explore → Explore,
   Direct → Direct) and the parent's `cwd`. For Work/Branch, it
   inherits the parent's `branch_name`, `base_branch`, `worktree_path`,
   and (Work only) `task_id`.
2. The parent's record gains a `continued_in_conv_id: String?` field
   pointing at the new conversation.

Both steps execute in a single DB transaction. Either the continuation
is created with the full inheritance or no state changes.

### Data model

`conversations` table gains:

```sql
continued_in_conv_id TEXT NULLABLE REFERENCES conversations(id)
```

The parent retains `worktree_path` in its `ConvMode::Work` / `ConvMode::Branch`
as a read-only history reference. The continuation's `ConvMode` holds the
same `worktree_path`. Two rows reference the same worktree path; ownership
is derived from the `continued_in_conv_id` pointer:

- A Work/Branch row with `continued_in_conv_id = null` owns its worktree.
- A Work/Branch row with `continued_in_conv_id` set does NOT own its worktree
  (the continuation does). The path is retained for history, not mutation.

There is no separate worktree registry table (per REQ-PROJ-015 DESCOPED);
`ConvMode::Work` and `ConvMode::Branch` rows serve as the de facto
registry. Ownership queries filter by `continued_in_conv_id = null` to
find active owners.

### Mode-specific inheritance

| Parent mode | Continuation mode | Worktree | Branch | Task |
|---|---|---|---|---|
| Work | Work | transferred | same `branch_name` | same `task_id` |
| Branch | Branch | transferred | same `branch_name` | — |
| Explore | Explore | — (no worktree) | — | — |
| Direct | Direct | — (no worktree) | — | — |

For modes without a worktree (Explore, Direct), the continuation
inherits only the `cwd` and the continuation summary. No filesystem or
registry changes are needed. Direct covers both git-repo and non-git
working directories per the `ConvMode::Direct` variant in the
implementation — there is no separate Standalone mode.

### Fork policy

`continued_in_conv_id` is single-valued. A parent has at most one
continuation. If the user revisits the parent after continuing, the
Continue action is replaced by a navigation link to the existing
continuation. Users who want to fork must use git operations outside
the app to create a second branch.

### Abandon policy

| Parent state | Abandon available |
|---|---|
| Context-exhausted, `continued_in_conv_id = null` | Yes; normal abandon flow applies |
| Context-exhausted, `continued_in_conv_id != null` | No; user abandons the continuation instead |

The semantics match REQ-PROJ-026's abandon flow. Work abandon removes
the worktree and the branch; Branch abandon removes the worktree and
preserves the branch.

### Server restart reconciliation

`reconcile_worktrees` at startup treats context-exhausted conversations
as preserved, not orphaned. For each Work/Branch row with an
on-disk-missing `worktree_path`:

- If the conversation is context-exhausted, skip it entirely (worktree
  was intentionally preserved pending user action; missing-on-disk
  shouldn't happen but if it does, don't compound the problem by
  demoting).
- If the conversation has `continued_in_conv_id` set, skip it — the
  history reference on the parent is expected to point at a path the
  continuation may have destroyed via its own terminal action.
- Otherwise, apply the existing demotion: Work → Explore, Branch →
  Direct, cwd reset to project root. This path now covers only genuine
  orphans (crashes, external `git worktree remove` by the user).

### Interaction with other flows

- `ApproveTask` from a continuation (Work mode) commits task completion
  and destroys the worktree per REQ-BED-029 — the chain's normal
  terminal semantics apply at the tail. The parent's `worktree_path`
  is retained in its record for history navigation, but the path no
  longer exists on disk after the destroy. UX handling of stale
  references in the parent's transcript (clickable file refs, tool
  invocations against the old worktree path, etc.) is not resolved by
  this spec and is left as an open concern.
- `ConfirmAbandon` / `MarkAsMerged` from a continuation's tail destroy
  the worktree per REQ-PROJ-026.
- Sub-agents of the parent are unaffected. Sub-agents are bounded
  primarily by the turn-limit / grace-turn flow (REQ-BED-026), which
  typically fires well before context exhaustion could. In the edge
  case that a sub-agent does reach context exhaustion, REQ-BED-024
  requires it to fail immediately with no continuation flow. Either
  way, sub-agents never reach a state where they would need worktree
  transfer — continuation is a parent-only concept.
- Task 08678 (auto-stash on cleanup) is obviated: uncommitted changes
  ride the worktree to the continuation because no cleanup happens.

### Implementation sketch

On the backend, a new `UserTriggerContinuation` handler (or extension of
the existing Continue endpoint) constructs the new conversation's
`ConvMode` variant by cloning the parent's and applying the inheritance
table above. The DB transaction shape:

```sql
BEGIN;
INSERT INTO conversations (id, conv_mode, cwd, ..., task_id)
  VALUES (new_id, <parent's mode clone>, <parent's cwd>, ...);
UPDATE conversations
  SET continued_in_conv_id = new_id
  WHERE id = parent_id;
COMMIT;
```

The atomicity of both steps is load-bearing: a partial apply would leave
the parent claiming ownership of a worktree while the continuation
expects to inherit it, or a continuation record with no parent linkage
back (stranding the continuation if the parent row is later read).

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

---

## Appendix A: Architecture Review — Failure Modes and Design Decisions

*This appendix preserves the pre-refactor analysis that drove the typed-effect
architecture. It was produced by commissioning five independent expert proposals after
autopsy of all resolved bugs. The failure modes are the WHY behind every design
decision in this document.*

### The Six Failure Modes

Every bug found in the conversation runtime lived outside the pure `transition()`
function — at the boundary between the state machine and the executor. Property tests
verified that `transition()` handles valid events correctly. They said nothing about
whether the executor generates valid events in the first place.

**FM-1: Semantic state inferred from content.**
The executor checked if bash output contained `"[command cancelled]"` and sent
`ToolAborted`. The SM was in `ToolExecuting` (no cancel requested). `ToolAborted` has
no valid transition from that state. Conversation stuck.
*Contract violated: events must derive from semantic/typed state, never from string
scanning of payload content.*
**Prevention:** `AbortReason` enum set by the cancellation requester. Typed
`Sender<ToolOutcome>` on `ToolExecuting` does not include an `Aborted` variant unless
the SM emitted a `CancelTool` effect.

**FM-2: Persistence atomicity violated.**
Executor persisted the agent message (with `tool_use` blocks) immediately on LLM
response, then launched tools as background tasks. On crash, storage had `tool_use`
without matching `tool_result`. LLM API rejected the malformed history. Same class
triggered by context exhaustion mid-tool-call.
*Contract violated: the writer assumed it would always complete; the reader assumed it
always reads consistent state. Neither was enforced.*
**Prevention:** `CheckpointData::ToolRound` constructor requires both sides. Assistant
message held in state until all tools complete, then persisted atomically.

**FM-3: Error classification not exhaustive.**
`ServerError` (5xx) hit a catch-all `_ => ErrorKind::Unknown`. `Unknown` is not
retryable. SM treated transient server errors as permanent failures. Also: wrong event
type (`ContinuationFailed` instead of `LlmError`) bypassed retry machinery.
*Contract violated: every error kind must have an explicit, intentional retryability
decision — catch-alls create accidental behavioral contracts.*
**Prevention:** `LlmErrorKind` has no `Unknown`. No `_ =>` match arms. Exhaustive
`map_llm_outcome_to_transition` function.

**FM-4: Parallel representation drift.**
State held both `completed_results: Vec<ToolResult>` and emitted `PersistToolResults`
effects. Two representations of "what has been persisted" coexisted and could diverge.
*Contract violated: one semantic fact has exactly one owner.*
**Prevention:** `ToolExecutingState.completed_results` is the single source.
`persisted_tool_ids` removed. Persistence is one atomic write at end.

**FM-5: Lifecycle contract not explicit.**
Terminal states never exited the executor loop. Loop exit was delegated to channel-drop
semantics. `ConversationHandle` lived in RuntimeManager, which never removed it, so the
channel never closed, so the loop never exited. Sub-agent executor tasks ran forever.
*Contract violated: terminal state was a semantic signal in the SM but had no
corresponding behavioral contract in the executor.*
**Prevention:** `StepResult::Terminal` forces explicit loop exit. Cleanup is in-task.
RuntimeManager notified via `TerminalOutcome`.

**FM-6: Liveness and ordering contract missing.**
Sub-agent results could arrive before parent transitions to `AwaitingSubAgents` —
compensated by unbounded `Vec` buffer with no size limit. Sub-agents had no timeout —
a parent could wait forever.
*Contract violated: every async dependency needs a stated, enforced bound.*
**Prevention:** Bounded buffer (capacity = sub-agent count). `timeout: Duration`
mandatory on `SubAgentConfig`. `deadline: Instant` in `AwaitingSubAgentsState`.
Executor `select!` races result against `sleep_until(deadline)`.

### Panel Summary and Architecture Selection

Five independent experts were given the full context and asked to propose concrete
solutions addressing property tests, streaming, and UI transparency.

| Proposal | Approach | Key Insight | Adopted? |
|----------|----------|-------------|----------|
| **Dr. Vera Cassini** (Formal Methods) | Phantom-typed handles (`ConvHandle<S>`) with sealed state witnesses | Invalid methods structurally absent from wrong states; `CompletedTurn` persistence gate | Types: no (too much type gymnastics for team); persistence gate: yes (`CheckpointData::ToolRound`) |
| **Olin Soren** (Event Sourcing) | Typed execution log as single source of truth; state derived via `fold_log()` | Entire execution history testable as `Vec<LogEntry>`; UI streams same log | No — largest migration, touches storage/recovery/SSE. Correctness wins achievable incrementally. |
| **Miriam Hecht** (Actor Model) | Typed effect envelopes with `ReplyToken<T>`; supervision-aware executor | `EffectInterpreter` trait makes executor testable; `PersistingBeforeTools` state | Interpreter trait: yes (via oneshot channels); pre-persist state: no (atomic end-of-round simpler) |
| **Tao Nguyen** (Reactive Streams) | `EffectOutcome` as typed executor boundary; `LlmStreaming` as first-class state | `handle_outcome() -> Result` rejects invalid outcomes; streaming tokens accumulate in state | `handle_outcome`: yes; streaming in state: no (no control-flow significance) |
| **Aleksei Volkov** (Rust Type Systems) | Oneshot channels per effect; session-typed executor; `CheckpointData::ToolRound` | Channel types constrain outcomes; `StepResult::Terminal`; total `map_llm_outcome` | Primary skeleton adopted. Oneshot channels + StepResult + ToolRound + exhaustive mapping |

### The Two Fork Decisions

**1. Does streaming belong in the SM?**
Decision: No. Tokens are fire-and-forget effects. See "Why Not SM State?" in the Token
Streaming Architecture section.

**2. Event log vs. effect typing?**
Decision: Effect typing. The event log is the most powerful for crash recovery and
debugging but is the largest migration. Typed effects get 90% of the correctness wins
without touching persistence architecture. If the event log is wanted later, typed
effects make it easier to retrofit — each effect dispatch becomes a log append.
