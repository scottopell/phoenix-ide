# Sub-Agents: Design Document

## Overview

Sub-agents enable parallel task execution by spawning independent child conversations that run concurrently and report results back to a parent conversation.

**Requirements**: REQ-BED-008 (Sub-Agent Spawning), REQ-BED-009 (Sub-Agent Isolation)

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                      PARENT CONVERSATION                         │
│                                                                  │
│  ToolExecuting ───[spawn_agents]───▶ AwaitingSubAgents          │
│                                             │                    │
│       ┌─────────────────────────────────────┤ (SpawnSubAgent     │
│       │               │               │       effects)           │
│       ▼               ▼               ▼                          │
│  ┌─────────┐    ┌─────────┐    ┌─────────┐                      │
│  │SubAgent1│    │SubAgent2│    │SubAgent3│  (independent)       │
│  │  ...    │    │  ...    │    │  ...    │                      │
│  │Completed│    │ Failed  │    │Completed│  (terminal states)   │
│  └────┬────┘    └────┬────┘    └────┬────┘                      │
│       │              │              │                            │
│       │    SubAgentResult events    │                            │
│       └──────────────┼──────────────┘                            │
│                      ▼                                           │
│              AwaitingSubAgents ────▶ LlmRequesting               │
│              (all results collected)                             │
└─────────────────────────────────────────────────────────────────┘
```

## State Machine Changes

### New/Modified States

```rust
enum ConvState {
    // ... existing states ...

    // Modified: tracks spawned sub-agents during tool execution
    ToolExecuting {
        current_tool: ToolCall,
        remaining_tools: Vec<ToolCall>,
        completed_results: Vec<ToolResult>,
        pending_sub_agents: Vec<String>,  // NEW: accumulated from spawn_agents
    },

    // Existing but now reachable
    AwaitingSubAgents {
        pending_ids: Vec<String>,
        completed_results: Vec<SubAgentResult>,
    },

    // NEW: waiting for sub-agents to acknowledge cancellation
    CancellingSubAgents {
        pending_ids: Vec<String>,
        completed_results: Vec<SubAgentResult>,
    },

    // NEW: sub-agent terminal states
    Completed { result: String },
    Failed { error: String, error_kind: ErrorKind },
}
```

### New Events

```rust
enum Event {
    // ... existing events ...

    // NEW: spawn_agents tool completion (distinct from ToolComplete)
    SpawnAgentsComplete {
        tool_use_id: String,
        result: ToolResult,           // Normal result for LLM context
        agent_ids: Vec<String>,       // Spawned sub-agent conversation IDs
    },

    // Existing but now used
    SubAgentResult {
        agent_id: String,
        outcome: SubAgentOutcome,
    },
}

// Typed outcome - pit of success, no invalid states
enum SubAgentOutcome {
    Success { result: String },
    Failure { error: String, error_kind: ErrorKind },
}
```

### New Effects

```rust
enum Effect {
    // ... existing effects ...

    // NEW: spawn a sub-agent conversation
    SpawnSubAgent {
        agent_id: String,
        task: String,
        cwd: String,
        timeout: Option<Duration>,
    },

    // NEW: cancel all pending sub-agents
    CancelSubAgents { ids: Vec<String> },

    // NEW: notify parent of sub-agent completion (sub-agent only)
    NotifyParent { outcome: SubAgentOutcome },
}

// Extended error kinds
enum ErrorKind {
    // ... existing ...
    TimedOut,   // NEW: sub-agent exceeded time limit
    Cancelled,  // NEW: explicit cancellation
}
```

## Edge Cases and Clarifications

### Early SubAgentResult (Race Condition)

**Problem:** Sub-agent completes while parent still in `ToolExecuting`.

**Solution:** Executor buffers `SubAgentResult` events until parent reaches `AwaitingSubAgents`. This keeps the state machine simple and avoids adding SubAgentResult handling to ToolExecuting.

```rust
// In executor/runtime
struct SubAgentResultBuffer {
    results: Vec<SubAgentResult>,
}

// When SubAgentResult arrives and parent not in AwaitingSubAgents:
//   buffer.push(result)
// When parent transitions to AwaitingSubAgents:
//   for result in buffer.drain() { send_event(result) }
```

### Terminal Tool Handling (submit_result / submit_error)

**Rule:** `submit_result` and `submit_error` MUST be the sole tool in the response.

**Detection:** Transition function inspects tool_calls BEFORE entering ToolExecuting:

```rust
// In transition, when is_sub_agent:
(LlmRequesting, LlmResponse { tool_calls, .. }) => {
    let terminal_tool = tool_calls.iter().find(|t| 
        t.name() == "submit_result" || t.name() == "submit_error"
    );
    
    if let Some(tool) = terminal_tool {
        if tool_calls.len() > 1 {
            // Error: terminal tool must be alone
            return Err(TransitionError::InvalidToolCombination(
                "submit_result/submit_error must be the only tool in response"
            ));
        }
        // Transition directly to terminal state
        match tool.name() {
            "submit_result" => Completed { result: tool.input.result },
            "submit_error" => Failed { error: tool.input.error, error_kind: SubAgentError },
        }
    } else {
        // Normal tool execution
        ToolExecuting { ... }
    }
}
```

### Cancellation During ToolExecuting with Pending Sub-Agents

**Problem:** Parent cancelled while sub-agents already spawned but more tools remain.

**Solution:** Transition to CancellingTool (existing) AND emit CancelSubAgents:

```
ToolExecuting { pending_sub_agents: [ids...] } + UserCancel
    → CancellingTool { ... }
    + Effect::AbortTool { current_tool }
    + Effect::CancelSubAgents { ids }  // NEW: also cancel spawned sub-agents
```

The CancellingTool flow continues normally. Buffered SubAgentResults are discarded since parent won't reach AwaitingSubAgents.

### agent_id Generation

**Responsibility:** Tool executor generates UUIDs when processing spawn_agents tool.

```rust
// In spawn_agents tool executor
fn execute_spawn_agents(input: SpawnAgentsInput, ctx: &ConvContext) -> SpawnAgentsResult {
    let agent_ids: Vec<String> = input.tasks.iter()
        .map(|_| Uuid::new_v4().to_string())
        .collect();
    
    // Return immediately with IDs; effects will spawn the actual agents
    SpawnAgentsResult {
        agent_ids: agent_ids.clone(),
        output: format!("Spawning {} sub-agents: {:?}", agent_ids.len(), agent_ids),
    }
}

// Executor then sends SpawnAgentsComplete event with these IDs
// AND emits SpawnSubAgent effects with the same IDs
```

### Partial Spawn Failure

**Policy:** All-or-nothing. If any spawn fails:
1. Cancel already-spawned agents
2. Return error to parent LLM
3. Parent remains in ToolExecuting (or transitions to next tool)

```rust
// SpawnAgentsComplete indicates success
// On failure, send ToolComplete with error result instead
Event::ToolComplete {
    tool_use_id,
    result: ToolResult {
        output: "Failed to spawn sub-agents: DB error",
        is_error: true,
    }
}
```

### Missing Runtime During Cancel

**Problem:** Sub-agent runtime crashed; CancelSubAgents can't reach it.

**Solution:** Synthesize failure result immediately:

```rust
async fn handle_cancel_sub_agents(ids: Vec<String>, runtime_manager: &RuntimeManager, parent_tx: &EventSender) {
    for id in ids {
        if let Some(runtime) = runtime_manager.get(&id) {
            runtime.send_event(Event::UserCancel).await;
        } else {
            // Runtime gone - synthesize result
            parent_tx.send(Event::SubAgentResult {
                agent_id: id,
                outcome: SubAgentOutcome::Failure {
                    error: "Sub-agent runtime not found".into(),
                    error_kind: ErrorKind::Cancelled,
                },
            }).await;
        }
    }
}
```

### Timeout Behavior

**Default:** No timeout if `None`. Sub-agent runs until:
- It calls submit_result/submit_error
- Parent is cancelled (propagates to sub-agents)
- Sub-agent hits unrecoverable error

**Recommendation:** Callers should specify reasonable timeouts. Future: system-wide default configurable via settings.

### Terminal State Exclusion from Wildcard Cancel

**Clarification:** The wildcard `* + UserCancel → Failed` explicitly excludes terminal states:

```rust
// This does NOT apply to Completed or Failed states
(state, Event::UserCancel) if ctx.is_sub_agent && !state.is_terminal() => {
    Failed { error: "Cancelled", error_kind: Cancelled }
}

impl ConvState {
    fn is_terminal(&self) -> bool {
        matches!(self, ConvState::Completed { .. } | ConvState::Failed { .. })
    }
}
```

### NotifyParent Failure Handling

**If parent_event_tx is None:** Programming error - sub-agent created without parent link. Log error, sub-agent still transitions to terminal state.

**If send() fails:** Parent terminated. Sub-agent transitions to terminal state anyway; result is lost but that's acceptable (parent is gone).

## State Transitions

### Parent: Tool Execution with Sub-Agent Spawning

```
// spawn_agents completes (more tools remaining)
ToolExecuting { current, remaining: [next, ...], pending_sub_agents }
    + SpawnAgentsComplete { agent_ids }
    → ToolExecuting { 
        current: next, 
        remaining: [...],
        pending_sub_agents: pending_sub_agents ++ agent_ids 
      }
    + Effect::ExecuteTool { next }
    + Effect::SpawnSubAgent × len(agent_ids)

// Last tool completes, sub-agents pending
ToolExecuting { remaining: [], pending_sub_agents: [..] }
    + ToolComplete | SpawnAgentsComplete
    → AwaitingSubAgents { pending_ids: pending_sub_agents, completed_results: [] }

// Last tool completes, no sub-agents
ToolExecuting { remaining: [], pending_sub_agents: [] }
    + ToolComplete
    → LlmRequesting { attempt: 1 }
    + Effect::RequestLlm
```

### Parent: Awaiting Sub-Agent Results (Fan-In)

```
// Sub-agent completes (more pending)
AwaitingSubAgents { pending_ids: [id, ...rest], completed_results }
    + SubAgentResult { agent_id: id, outcome }
    → AwaitingSubAgents { 
        pending_ids: rest, 
        completed_results: completed_results ++ [result] 
      }

// Last sub-agent completes
AwaitingSubAgents { pending_ids: [id], completed_results }
    + SubAgentResult { agent_id: id, outcome }
    → LlmRequesting { attempt: 1 }
    + Effect::PersistMessage { aggregated results }
    + Effect::RequestLlm

// Unknown agent_id - reject
AwaitingSubAgents { pending_ids }
    + SubAgentResult { agent_id } where agent_id ∉ pending_ids
    → Error: InvalidTransition
```

### Parent: Cancellation While Awaiting Sub-Agents

```
// User cancels while waiting
AwaitingSubAgents { pending_ids, completed_results }
    + UserCancel
    → CancellingSubAgents { pending_ids, completed_results }
    + Effect::CancelSubAgents { ids: pending_ids }

// Sub-agent acknowledges cancellation (or completes naturally)
CancellingSubAgents { pending_ids: [id, ...rest], completed_results }
    + SubAgentResult { agent_id: id, outcome }
    → CancellingSubAgents { pending_ids: rest, completed_results ++ [result] }

// Last sub-agent done during cancellation
CancellingSubAgents { pending_ids: [id], completed_results }
    + SubAgentResult { agent_id: id, outcome }
    → Idle
    + Effect::NotifyAgentDone
```

### Sub-Agent: Terminal State Transitions

```
// LLM calls submit_result - transition to Completed (not tool execution)
LlmRequesting + LlmResponse { tool_calls: [submit_result { result }] }
    where context.is_sub_agent
    → Completed { result }
    + Effect::NotifyParent { outcome: Success { result } }

// LLM calls submit_error - transition to Failed
LlmRequesting + LlmResponse { tool_calls: [submit_error { error }] }
    where context.is_sub_agent
    → Failed { error, error_kind: SubAgentError }
    + Effect::NotifyParent { outcome: Failure { error, error_kind } }

// Sub-agent hits unrecoverable error - also terminal
Error { message, error_kind } where context.is_sub_agent
    → Failed { error: message, error_kind }
    + Effect::NotifyParent { outcome: Failure { ... } }

// Sub-agent receives cancellation (from parent or timeout)
* + UserCancel where context.is_sub_agent
    → Failed { error: "Cancelled", error_kind: Cancelled }
    + Effect::NotifyParent { outcome: Failure { ... } }
```

## Property Invariants

### Fan-In Conservation

```rust
// pending_ids.len() + completed_results.len() == N (constant)
#[proptest]
fn prop_subagent_count_conserved(initial_ids: Vec<String>, completions: Vec<SubAgentResult>) {
    let n = initial_ids.len();
    let mut state = AwaitingSubAgents { pending_ids: initial_ids, completed_results: vec![] };
    
    for result in completions {
        state = transition(&state, &ctx, SubAgentResult(result)).new_state;
        match &state {
            AwaitingSubAgents { pending_ids, completed_results } |
            CancellingSubAgents { pending_ids, completed_results } => {
                assert_eq!(pending_ids.len() + completed_results.len(), n);
            }
            _ => {}
        }
    }
}
```

### Monotonicity

```rust
// pending_ids only decreases
#[proptest]
fn prop_pending_decreases_monotonically(...) { ... }

// completed_results only increases
#[proptest]
fn prop_completed_increases_monotonically(...) { ... }
```

### Terminal State Properties

```rust
// Completed and Failed are terminal - no transitions out
#[proptest]
fn prop_terminal_states_are_terminal(event: Event) {
    let completed = ConvState::Completed { result: "done".into() };
    let failed = ConvState::Failed { error: "err".into(), error_kind: ... };
    
    assert!(transition(&completed, &sub_agent_ctx, event.clone()).is_err());
    assert!(transition(&failed, &sub_agent_ctx, event).is_err());
}
```

### Rejection Properties

```rust
// Unknown agent_id rejected
#[proptest]
fn prop_unknown_agent_rejected(pending_ids: Vec<String>, unknown: String) {
    prop_assume!(!pending_ids.contains(&unknown));
    let state = AwaitingSubAgents { pending_ids, completed_results: vec![] };
    let event = SubAgentResult { agent_id: unknown, ... };
    assert!(transition(&state, &ctx, event).is_err());
}

// Duplicate completion rejected
#[proptest]
fn prop_duplicate_rejected(agent_id: String) {
    let state = AwaitingSubAgents { 
        pending_ids: vec![], 
        completed_results: vec![SubAgentResult { agent_id: agent_id.clone(), ... }]
    };
    let event = SubAgentResult { agent_id, ... };
    assert!(transition(&state, &ctx, event).is_err());
}
```

### No Nested Sub-Agents

```rust
// spawn_agents not available to sub-agents (enforced at tool filtering)
#[test]
fn test_subagent_tools_exclude_spawn_agents() {
    let tools = tools_for_context(&sub_agent_context);
    assert!(!tools.iter().any(|t| t.name() == "spawn_agents"));
}
```

## Tool Definitions

### spawn_agents (Parent Only)

```json
{
  "name": "spawn_agents",
  "description": "Spawn sub-agents to execute tasks in parallel. Each sub-agent runs independently and returns a result. Use for: multiple perspectives on code review, exploring unfamiliar parts of a codebase, parallel research or analysis tasks, or divide-and-conquer problem solving.",
  "input_schema": {
    "type": "object",
    "required": ["tasks"],
    "properties": {
      "tasks": {
        "type": "array",
        "items": {
          "type": "object",
          "required": ["task"],
          "properties": {
            "task": {
              "type": "string",
              "description": "Task description for the sub-agent"
            },
            "cwd": {
              "type": "string",
              "description": "Working directory (defaults to parent's cwd)"
            }
          }
        },
        "minItems": 1,
        "description": "List of tasks to execute in parallel"
      }
    }
  }
}
```

### submit_result (Sub-Agent Only)

```json
{
  "name": "submit_result",
  "description": "Submit your final result to the parent conversation. Call this when you have completed your assigned task. After calling this, your conversation ends.",
  "input_schema": {
    "type": "object",
    "required": ["result"],
    "properties": {
      "result": {
        "type": "string",
        "description": "Your final result, summary, or output"
      }
    }
  }
}
```

### submit_error (Sub-Agent Only)

```json
{
  "name": "submit_error",
  "description": "Report that you cannot complete the assigned task. Call this if you encounter an unrecoverable error or determine the task is impossible. After calling this, your conversation ends.",
  "input_schema": {
    "type": "object",
    "required": ["error"],
    "properties": {
      "error": {
        "type": "string",
        "description": "Description of why the task could not be completed"
      }
    }
  }
}
```

## Tool Availability

```rust
fn tools_for_context(ctx: &ConvContext) -> Vec<Tool> {
    let mut tools = vec![
        bash_tool(),
        patch_tool(),
        think_tool(),
        keyword_search_tool(),
        read_image_tool(),
        // ... other standard tools
    ];

    if ctx.is_sub_agent {
        // Sub-agents get completion tools, no spawning
        tools.push(submit_result_tool());
        tools.push(submit_error_tool());
    } else {
        // Main conversations can spawn sub-agents
        tools.push(spawn_agents_tool());
    }

    tools
}
```

## Runtime / Executor Responsibilities

### Sub-Agent Spawning (Effect Handler)

```rust
async fn handle_spawn_sub_agent(effect: SpawnSubAgent, parent_ctx: &ConvContext) {
    // 1. Create conversation in DB
    let conv = db.create_conversation(CreateConversation {
        cwd: effect.cwd,
        parent_conversation_id: Some(parent_ctx.conversation_id.clone()),
        user_initiated: false,
    });

    // 2. Insert initial task as synthetic user message
    db.add_message(conv.id, MessageContent::User { 
        text: effect.task,
        images: vec![],
    });

    // 3. Create sub-agent context
    let sub_ctx = ConvContext {
        conversation_id: conv.id,
        working_dir: effect.cwd.into(),
        model_id: parent_ctx.model_id.clone(),
        is_sub_agent: true,
        parent_event_tx: Some(parent_event_tx.clone()),
    };

    // 4. Start runtime with optional timeout
    let runtime = ConversationRuntime::new(sub_ctx);
    if let Some(timeout) = effect.timeout {
        runtime.set_timeout(timeout);
    }
    
    // 5. Spawn runtime task
    tokio::spawn(runtime.run());
}
```

### Parent Notification (Effect Handler)

```rust
async fn handle_notify_parent(outcome: SubAgentOutcome, ctx: &ConvContext) {
    if let Some(parent_tx) = &ctx.parent_event_tx {
        parent_tx.send(Event::SubAgentResult {
            agent_id: ctx.conversation_id.clone(),
            outcome,
        }).await;
    }
}
```

### Cancel Propagation (Effect Handler)

```rust
async fn handle_cancel_sub_agents(ids: Vec<String>, runtime_manager: &RuntimeManager) {
    for id in ids {
        if let Some(runtime) = runtime_manager.get(&id) {
            runtime.send_event(Event::UserCancel).await;
        }
    }
}
```

### Timeout (Executor Concern)

```rust
impl ConversationRuntime {
    fn set_timeout(&mut self, duration: Duration) {
        let event_tx = self.event_tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(duration).await;
            // Timeout triggers cancellation
            let _ = event_tx.send(Event::UserCancel).await;
        });
    }
}
```

## Database

### Schema (No Changes Required)

Existing fields support sub-agents:

```sql
CREATE TABLE conversations (
    id TEXT PRIMARY KEY,
    parent_conversation_id TEXT,    -- Set for sub-agents
    user_initiated BOOLEAN NOT NULL, -- FALSE for sub-agents
    ...
    FOREIGN KEY (parent_conversation_id) 
        REFERENCES conversations(id) ON DELETE CASCADE
);
```

### Queries

```sql
-- List user conversations (excludes sub-agents) - ALREADY EXISTS
SELECT * FROM conversations 
WHERE user_initiated = 1 AND archived = 0;

-- Get sub-agents for a parent
SELECT * FROM conversations 
WHERE parent_conversation_id = ?;
```

## Sub-Agent Initial Message

Sub-agents receive their task as a synthetic `UserMessage`:

```rust
// When spawning sub-agent
db.add_message(conv.id, Message {
    message_type: MessageType::User,
    content: MessageContent::User { 
        text: task,  // From spawn_agents input
        images: vec![],
    },
    ...
});
```

This triggers the normal flow: `Idle → LlmRequesting → ...`

The sub-agent's LLM sees:
```
[User]: Review the error handling in src/api/ and identify potential issues
[Assistant]: I'll examine the error handling patterns...
```

## Aggregated Results Format

When all sub-agents complete, parent's LLM receives:

```json
{
  "sub_agent_results": [
    {
      "agent_id": "uuid-1",
      "task": "Review error handling from a security perspective",
      "outcome": {
        "success": {
          "result": "Found 3 issues: 1) Auth errors leak internal details in src/api/handlers.rs:45, 2) ..."
        }
      }
    },
    {
      "agent_id": "uuid-2", 
      "task": "Review error handling from a performance perspective",
      "outcome": {
        "failure": {
          "error": "Codebase too large to analyze within time limit",
          "error_kind": "sub_agent_error"
        }
      }
    }
  ]
}
```

## Complete Lifecycle Trace

This section traces a complete sub-agent flow from spawn to completion.

### Happy Path: Two Sub-Agents Complete Successfully

```
1. Parent in LlmRequesting, LLM returns spawn_agents tool
   State: LlmRequesting { attempt: 1 }
   Event: LlmResponse { tool_calls: [spawn_agents { tasks: [A, B] }] }
   
2. Transition to ToolExecuting
   State: ToolExecuting { current: spawn_agents, remaining: [], pending_sub_agents: [] }
   Effect: ExecuteTool { spawn_agents }

3. Executor runs spawn_agents tool, generates IDs ["sa-1", "sa-2"]
   Executor sends: SpawnAgentsComplete { agent_ids: ["sa-1", "sa-2"], result: "Spawned 2 agents" }
   Executor emits: SpawnSubAgent { agent_id: "sa-1", task: A }
   Executor emits: SpawnSubAgent { agent_id: "sa-2", task: B }

4. Transition to AwaitingSubAgents (last tool, has pending)
   State: AwaitingSubAgents { pending_ids: ["sa-1", "sa-2"], completed_results: [] }
   Effect: PersistMessage { "Spawned 2 agents" }

5. SpawnSubAgent effects execute:
   - Create conversation sa-1 (user_initiated=false, parent_id=parent)
   - Insert synthetic UserMessage with task A
   - Start sa-1 runtime
   - (same for sa-2)

6. Sub-agent sa-1 runs its own state machine:
   Idle → LlmRequesting → ToolExecuting → ... → LlmRequesting
   LLM returns: submit_result { result: "Completed task A" }
   Transition: Completed { result: "Completed task A" }
   Effect: NotifyParent { Success { result: "Completed task A" } }

7. NotifyParent effect sends to parent:
   Event: SubAgentResult { agent_id: "sa-1", outcome: Success { "Completed task A" } }

8. Parent receives first result:
   State: AwaitingSubAgents { pending: ["sa-1", "sa-2"], completed: [] }
   Event: SubAgentResult { agent_id: "sa-1", ... }
   State: AwaitingSubAgents { pending: ["sa-2"], completed: [sa-1-result] }

9. Sub-agent sa-2 completes similarly, parent receives:
   State: AwaitingSubAgents { pending: ["sa-2"], completed: [sa-1-result] }
   Event: SubAgentResult { agent_id: "sa-2", ... }
   State: LlmRequesting { attempt: 1 }  // Last result triggers exit
   Effect: PersistMessage { aggregated results }
   Effect: RequestLlm

10. Parent LLM receives aggregated results, continues conversation.
```

### Cancellation Path: User Cancels During AwaitingSubAgents

```
1. Parent in AwaitingSubAgents { pending: ["sa-1", "sa-2"], completed: [] }
   Event: UserCancel

2. Transition to CancellingSubAgents
   State: CancellingSubAgents { pending: ["sa-1", "sa-2"], completed: [] }
   Effect: CancelSubAgents { ids: ["sa-1", "sa-2"] }

3. Executor sends UserCancel to each sub-agent runtime

4. Sub-agent sa-1 receives cancel:
   State: (whatever it was)
   Event: UserCancel
   State: Failed { error: "Cancelled", error_kind: Cancelled }
   Effect: NotifyParent { Failure { error: "Cancelled", error_kind: Cancelled } }

5. Parent receives cancellation acknowledgment:
   State: CancellingSubAgents { pending: ["sa-1", "sa-2"], completed: [] }
   Event: SubAgentResult { agent_id: "sa-1", outcome: Failure { Cancelled } }
   State: CancellingSubAgents { pending: ["sa-2"], completed: [sa-1-result] }

6. Sub-agent sa-2 similarly cancelled and reports

7. Last cancellation result:
   State: CancellingSubAgents { pending: ["sa-2"], completed: [sa-1-result] }
   Event: SubAgentResult { agent_id: "sa-2", ... }
   State: Idle
   Effect: NotifyAgentDone
```

### Early Completion Race: Sub-Agent Finishes During Parent ToolExecuting

```
1. Parent calls [spawn_agents, bash]
   State: ToolExecuting { current: spawn_agents, remaining: [bash], pending_sub_agents: [] }

2. spawn_agents completes:
   Event: SpawnAgentsComplete { agent_ids: ["sa-1"] }
   State: ToolExecuting { current: bash, remaining: [], pending_sub_agents: ["sa-1"] }
   Effect: ExecuteTool { bash }
   Effect: SpawnSubAgent { "sa-1" }

3. Sub-agent sa-1 spawned and runs FAST, completes before bash:
   sa-1: Completed { result }
   sa-1: Effect::NotifyParent
   
4. Executor receives SubAgentResult but parent still in ToolExecuting:
   Executor: buffer.push(SubAgentResult { "sa-1" })

5. bash completes:
   Event: ToolComplete { bash result }
   State: AwaitingSubAgents { pending: ["sa-1"], completed: [] }  // has pending_sub_agents
   
6. Executor sees transition to AwaitingSubAgents, drains buffer:
   Event: SubAgentResult { "sa-1" } (from buffer)
   State: LlmRequesting  // immediate transition, sa-1 already done
```

## Example Use Cases

### Multi-Perspective Code Review

```
User: Review the authentication module for potential issues

Agent calls spawn_agents with:
{
  "tasks": [
    { "task": "Review src/auth/ from a security perspective. Look for vulnerabilities, credential handling issues, and attack vectors." },
    { "task": "Review src/auth/ from a maintainability perspective. Assess code clarity, test coverage, and documentation." },
    { "task": "Review src/auth/ from a performance perspective. Identify bottlenecks, unnecessary allocations, or N+1 patterns." }
  ]
}

Three sub-agents analyze the same code with different lenses,
parent aggregates findings into comprehensive review.
```

### Codebase Exploration

```
User: I'm new to this project. Help me understand the architecture.

Agent calls spawn_agents with:
{
  "tasks": [
    { "task": "Explore the database layer. Document the schema, key queries, and data access patterns." },
    { "task": "Explore the API layer. Document the endpoints, request/response formats, and middleware." },
    { "task": "Explore the core business logic. Document the main abstractions and how they interact." }
  ]
}

Sub-agents explore different areas in parallel,
parent synthesizes into architectural overview.
```

### Focused Deep-Dive (Single Sub-Agent)

```
User: How does error handling work in this codebase?

Agent calls spawn_agents with:
{
  "tasks": [
    { "task": "Thoroughly investigate error handling patterns in this codebase. Trace how errors propagate from tools through the state machine to the API. Document the error types, conversion points, and user-facing messages." }
  ]
}

Single sub-agent does focused research without polluting
parent's context with exploration details.
```

### Comparative Analysis

```
User: Should we use approach A or B for the new feature?

Agent calls spawn_agents with:
{
  "tasks": [
    { "task": "Analyze approach A: [description]. Evaluate pros, cons, implementation complexity, and how it fits with existing patterns in this codebase." },
    { "task": "Analyze approach B: [description]. Evaluate pros, cons, implementation complexity, and how it fits with existing patterns in this codebase." }
  ]
}

Sub-agents research independently without biasing each other,
parent makes informed recommendation based on both analyses.
```

### Persona-Based Review

```
User: Get feedback on this API design from different stakeholders

Agent calls spawn_agents with:
{
  "tasks": [
    { "task": "Review the API design as a frontend developer. Is it easy to consume? Are the response shapes convenient? Is error handling clear?" },
    { "task": "Review the API design as a DevOps engineer. Is it easy to monitor? Are there health checks? How's the logging?" },
    { "task": "Review the API design as a new team member. Is it well documented? Are the conventions consistent? Can you understand it without tribal knowledge?" }
  ]
}

Different perspectives surface different issues.
```

## Implementation Order

1. **State machine changes**
   - Add `pending_sub_agents` to `ToolExecuting`
   - Add `CancellingSubAgents` state
   - Add `Completed` / `Failed` terminal states
   - Add `SpawnAgentsComplete` event
   - Implement new transitions
   - Add property tests

2. **Tools**
   - Implement `spawn_agents` tool
   - Implement `submit_result` tool
   - Implement `submit_error` tool
   - Tool filtering by context

3. **Runtime support**
   - Effect handlers for `SpawnSubAgent`, `CancelSubAgents`, `NotifyParent`
   - Timeout support
   - Event routing between parent/child

4. **Integration**
   - End-to-end tests
   - Error handling edge cases
   - Documentation
