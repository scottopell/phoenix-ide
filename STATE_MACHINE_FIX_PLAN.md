# State Machine Fix Plan

Plan to bring Phoenix IDE state machine into full compliance with best practices.

## Overview

| Phase | Focus | Effort | Risk |
|-------|-------|--------|------|
| 1 | Move `pending_tools` into state | Medium | Low |
| 2 | Add property-based tests | Medium | Low |
| 3 | Abstract I/O for testing | High | Medium |
| 4 | Stronger typing (optional) | Low | Low |

**Recommended order:** Phase 1 → Phase 2 → Phase 3

Phase 1 must come first because property tests (Phase 2) need the state to be complete.

---

## Phase 1: Move `pending_tools` into State Machine

**Goal:** Eliminate executor-side state; make state machine the single source of truth.

### Problem

Currently, tool information is split:
- State machine: `ToolExecuting { current_tool_id, remaining_tool_ids, ... }`
- Executor: `pending_tools: Vec<(String, String, Value)>` (id, name, input)

This violates "state machine as single source of truth."

### Solution

#### 1.1 Create `ToolCall` type

```rust
// src/state_machine/state.rs
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: Value,
}
```

#### 1.2 Update `ConvState::ToolExecuting`

```rust
// Before
ToolExecuting {
    current_tool_id: String,
    remaining_tool_ids: Vec<String>,
    completed_results: Vec<ToolResult>,
}

// After
ToolExecuting {
    current_tool: ToolCall,
    remaining_tools: Vec<ToolCall>,
    completed_results: Vec<ToolResult>,
}
```

#### 1.3 Update `Event::LlmResponse` to include tool calls

```rust
// Before
LlmResponse {
    content: Vec<ContentBlock>,
    end_turn: bool,
    usage: Usage,
}

// After  
LlmResponse {
    content: Vec<ContentBlock>,
    tool_calls: Vec<ToolCall>,  // Extracted from content
    end_turn: bool,
    usage: Usage,
}
```

#### 1.4 Update transition logic

```rust
// LlmRequesting + LlmResponse with tools
(ConvState::LlmRequesting { .. }, Event::LlmResponse { content, tool_calls, .. }) => {
    if tool_calls.is_empty() {
        // No tools -> Idle
        ...
    } else {
        let (first, rest) = (tool_calls[0].clone(), tool_calls[1..].to_vec());
        Ok(TransitionResult::new(ConvState::ToolExecuting {
            current_tool: first.clone(),
            remaining_tools: rest,
            completed_results: vec![],
        })
        .with_effect(Effect::ExecuteTool {
            tool_use_id: first.id,
            name: first.name,
            input: first.input,
        })
        ...)
    }
}
```

#### 1.5 Update executor

- Remove `pending_tools` field
- Extract tool calls in `make_llm_request_event()` and include in event
- Simplify `execute_effect(Effect::ExecuteTool)` - info comes from effect, not stored state

#### 1.6 Update `Effect::ExecuteTool` for next tool

When transitioning from one tool to the next:

```rust
// ToolExecuting + ToolComplete (more tools)
(ConvState::ToolExecuting { current_tool, remaining_tools, completed_results },
 Event::ToolComplete { tool_use_id, result })
    if tool_use_id == current_tool.id && !remaining_tools.is_empty() =>
{
    let next = remaining_tools[0].clone();
    let rest = remaining_tools[1..].to_vec();
    
    Ok(TransitionResult::new(ConvState::ToolExecuting {
        current_tool: next.clone(),
        remaining_tools: rest,
        completed_results: new_results,
    })
    .with_effect(Effect::ExecuteTool {
        tool_use_id: next.id,
        name: next.name,
        input: next.input,
    })
    ...)
}
```

### Files to Modify

1. `src/state_machine/state.rs` - Add `ToolCall`, update `ConvState`
2. `src/state_machine/event.rs` - Update `Event::LlmResponse`
3. `src/state_machine/transition.rs` - Update all `ToolExecuting` transitions
4. `src/runtime/executor.rs` - Remove `pending_tools`, update event creation
5. `src/db/schema.rs` - Update `ConversationState` enum if needed

### Testing

- Existing unit tests should still pass (update as needed)
- Manual test: run the "Hello World → Hello Phoenix IDE" demo

---

## Phase 2: Add Property-Based Tests

**Goal:** Comprehensive proptest coverage for state machine invariants.

### 2.1 Create test infrastructure

```rust
// src/state_machine/proptests.rs
use proptest::prelude::*;
use super::*;

fn arb_tool_call() -> impl Strategy<Value = ToolCall> {
    ("[a-z]{8}", "[a-z_]{3,10}", Just(json!({}))).prop_map(|(id, name, input)| {
        ToolCall { id, name, input }
    })
}

fn arb_tool_result() -> impl Strategy<Value = ToolResult> {
    ("[a-z]{8}", any::<bool>(), ".*").prop_map(|(id, success, output)| {
        ToolResult { tool_use_id: id, success, output, is_error: !success }
    })
}

fn arb_state() -> impl Strategy<Value = ConvState> {
    prop_oneof![
        Just(ConvState::Idle),
        (1u32..5).prop_map(|a| ConvState::LlmRequesting { attempt: a }),
        (arb_tool_call(), vec(arb_tool_call(), 0..3), vec(arb_tool_result(), 0..3))
            .prop_map(|(curr, rem, res)| ConvState::ToolExecuting {
                current_tool: curr,
                remaining_tools: rem,
                completed_results: res,
            }),
        (".*", Just(ErrorKind::Network)).prop_map(|(msg, kind)| ConvState::Error {
            message: msg, error_kind: kind
        }),
    ]
}

fn arb_event() -> impl Strategy<Value = Event> { ... }
```

### 2.2 Invariant tests

```rust
proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    // Invariant 1: Valid state after any transition
    #[test]
    fn prop_transitions_preserve_validity(events in vec(arb_event(), 0..20)) {
        let mut state = ConvState::Idle;
        let ctx = test_context();
        
        for event in events {
            match transition(&state, &ctx, event) {
                Ok(result) => {
                    state = result.new_state;
                    prop_assert!(is_valid_state(&state));
                    prop_assert!(effects_are_valid(&result.effects, &state));
                }
                Err(_) => { /* Invalid transition is OK */ }
            }
        }
    }

    // Invariant 2: Error state is always recoverable
    #[test]
    fn prop_error_always_recoverable(
        message in ".*",
        kind in arb_error_kind()
    ) {
        let state = ConvState::Error { message, error_kind: kind };
        let event = Event::UserMessage { text: "retry".into(), images: vec![] };
        
        let result = transition(&state, &test_context(), event);
        prop_assert!(result.is_ok());
        prop_assert!(matches!(result.unwrap().new_state, ConvState::LlmRequesting { .. }));
    }

    // Invariant 3: Cancel from any working state reaches Idle or Cancelling
    #[test]
    fn prop_cancel_stops_work(state in arb_working_state()) {
        let result = transition(&state, &test_context(), Event::UserCancel);
        prop_assert!(result.is_ok());
        let new_state = result.unwrap().new_state;
        prop_assert!(matches!(new_state, ConvState::Idle | ConvState::Cancelling { .. }));
    }

    // Invariant 4: Tool completion with matching ID always succeeds
    #[test]
    fn prop_tool_complete_with_matching_id_succeeds(
        current in arb_tool_call(),
        remaining in vec(arb_tool_call(), 0..3),
        completed in vec(arb_tool_result(), 0..3),
        result in arb_tool_result()
    ) {
        let state = ConvState::ToolExecuting {
            current_tool: current.clone(),
            remaining_tools: remaining,
            completed_results: completed,
        };
        let event = Event::ToolComplete {
            tool_use_id: current.id.clone(),
            result: ToolResult { tool_use_id: current.id, ..result },
        };
        
        let trans_result = transition(&state, &test_context(), event);
        prop_assert!(trans_result.is_ok());
    }

    // Invariant 5: Busy states reject user messages
    #[test]
    fn prop_busy_rejects_messages(state in arb_busy_state()) {
        let event = Event::UserMessage { text: "hi".into(), images: vec![] };
        let result = transition(&state, &test_context(), event);
        prop_assert!(matches!(result, Err(TransitionError::AgentBusy) | 
                                      Err(TransitionError::CancellationInProgress)));
    }

    // Invariant 6: PersistState effect always emitted on state change
    #[test]
    fn prop_state_changes_persist(
        state in arb_state(),
        event in arb_event()
    ) {
        if let Ok(result) = transition(&state, &test_context(), event) {
            if result.new_state != state {
                prop_assert!(result.effects.iter().any(|e| matches!(e, Effect::PersistState)));
            }
        }
    }
}
```

### 2.3 Helper functions

```rust
fn is_valid_state(state: &ConvState) -> bool {
    match state {
        ConvState::ToolExecuting { current_tool, remaining_tools, .. } => {
            // No duplicate tool IDs
            let mut ids: Vec<_> = std::iter::once(&current_tool.id)
                .chain(remaining_tools.iter().map(|t| &t.id))
                .collect();
            let len = ids.len();
            ids.sort();
            ids.dedup();
            ids.len() == len
        }
        ConvState::LlmRequesting { attempt } => *attempt >= 1 && *attempt <= 10,
        _ => true,
    }
}

fn effects_are_valid(effects: &[Effect], new_state: &ConvState) -> bool {
    // ExecuteTool should only appear when transitioning to ToolExecuting
    let has_execute = effects.iter().any(|e| matches!(e, Effect::ExecuteTool { .. }));
    if has_execute && !matches!(new_state, ConvState::ToolExecuting { .. }) {
        // Also valid if we're going to LlmRequesting after tools complete
        if !matches!(new_state, ConvState::LlmRequesting { .. }) {
            return false;
        }
    }
    true
}

fn arb_working_state() -> impl Strategy<Value = ConvState> {
    prop_oneof![
        (1u32..5).prop_map(|a| ConvState::LlmRequesting { attempt: a }),
        (arb_tool_call(), vec(arb_tool_call(), 0..3), vec(arb_tool_result(), 0..3))
            .prop_map(|(c, r, res)| ConvState::ToolExecuting {
                current_tool: c, remaining_tools: r, completed_results: res
            }),
        Just(ConvState::AwaitingLlm),
    ]
}

fn arb_busy_state() -> impl Strategy<Value = ConvState> {
    prop_oneof![
        arb_working_state(),
        Just(ConvState::Cancelling { pending_tool_id: None }),
    ]
}
```

### Files to Create/Modify

1. `src/state_machine/proptests.rs` - New file with all property tests
2. `src/state_machine/mod.rs` - Add `#[cfg(test)] mod proptests;`
3. `Cargo.toml` - Ensure `proptest` is in dev-dependencies

### Success Criteria

- All 6+ invariants pass with 1000 cases each
- No shrinking failures
- CI runs property tests

---

## Phase 3: Abstract I/O for Integration Testing

**Goal:** Enable testing executor + state machine together without real I/O.

### 3.1 Define runtime traits

```rust
// src/runtime/traits.rs
use async_trait::async_trait;

#[async_trait]
pub trait MessageStore: Send + Sync {
    async fn add_message(&self, conv_id: &str, msg: &Message) -> Result<(), Error>;
    async fn get_messages(&self, conv_id: &str) -> Result<Vec<Message>, Error>;
}

#[async_trait]
pub trait StateStore: Send + Sync {
    async fn get_state(&self, conv_id: &str) -> Result<ConvState, Error>;
    async fn set_state(&self, conv_id: &str, state: &ConvState) -> Result<(), Error>;
}

#[async_trait]
pub trait LlmClient: Send + Sync {
    async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError>;
}

#[async_trait]
pub trait ToolExecutor: Send + Sync {
    async fn execute(&self, name: &str, input: Value) -> Option<ToolOutput>;
}
```

### 3.2 Create mock implementations

```rust
// src/runtime/testing.rs
pub struct MockLlmClient {
    responses: Mutex<VecDeque<Result<LlmResponse, LlmError>>>,
}

impl MockLlmClient {
    pub fn new() -> Self { ... }
    pub fn queue_response(&self, resp: LlmResponse) { ... }
    pub fn queue_error(&self, err: LlmError) { ... }
}

#[async_trait]
impl LlmClient for MockLlmClient {
    async fn complete(&self, _request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        self.responses.lock().unwrap().pop_front()
            .unwrap_or(Err(LlmError::network("No mock response queued")))
    }
}

pub struct MockToolExecutor {
    outputs: HashMap<String, ToolOutput>,
}

impl MockToolExecutor {
    pub fn with_tool(mut self, name: &str, output: ToolOutput) -> Self {
        self.outputs.insert(name.to_string(), output);
        self
    }
}
```

### 3.3 Refactor executor to use traits

```rust
pub struct ConversationRuntime<S, L, T>
where
    S: StateStore + MessageStore,
    L: LlmClient,
    T: ToolExecutor,
{
    context: ConvContext,
    state: ConvState,
    storage: S,
    llm: L,
    tools: T,
    // ... channels
}
```

### 3.4 Integration tests

```rust
#[tokio::test]
async fn test_full_conversation_flow() {
    let storage = InMemoryStorage::new();
    let llm = MockLlmClient::new();
    llm.queue_response(LlmResponse {
        content: vec![ContentBlock::text("Hello!")],
        tool_calls: vec![],
        end_turn: true,
        usage: Usage::default(),
    });
    
    let runtime = ConversationRuntime::new(
        test_context(),
        ConvState::Idle,
        storage,
        llm,
        MockToolExecutor::new(),
    );
    
    // Send user message
    runtime.send_event(Event::UserMessage { 
        text: "Hi".into(), 
        images: vec![] 
    }).await;
    
    // Wait for processing
    runtime.wait_idle().await;
    
    // Assert final state
    assert_eq!(runtime.state(), &ConvState::Idle);
    assert_eq!(storage.messages().len(), 2); // user + agent
}

#[tokio::test]
async fn test_tool_execution_flow() {
    let llm = MockLlmClient::new();
    llm.queue_response(LlmResponse {
        content: vec![],
        tool_calls: vec![ToolCall { id: "t1".into(), name: "bash".into(), input: json!({}) }],
        end_turn: false,
        usage: Usage::default(),
    });
    llm.queue_response(LlmResponse {
        content: vec![ContentBlock::text("Done!")],
        tool_calls: vec![],
        end_turn: true,
        usage: Usage::default(),
    });
    
    let tools = MockToolExecutor::new()
        .with_tool("bash", ToolOutput::success("output"));
    
    // ... run and assert
}
```

### Files to Create/Modify

1. `src/runtime/traits.rs` - New trait definitions
2. `src/runtime/testing.rs` - Mock implementations
3. `src/runtime/executor.rs` - Refactor to use traits
4. `src/runtime/mod.rs` - Export new modules
5. `tests/integration_tests.rs` - Integration tests using mocks

### Effort Estimate

This is the largest phase because it requires:
- Defining clean trait boundaries
- Refactoring executor (medium-sized file)
- Creating mock implementations
- Writing integration tests

---

## Phase 4: Stronger Typing (Optional)

**Goal:** Replace `Value` with typed structures where feasible.

This is lower priority because:
- Current loose typing works
- Patch tool already has strong types
- Main benefit is documentation/safety

### Potential improvements

```rust
// Instead of Value for message content
enum MessageContent {
    UserText { text: String, images: Vec<ImageData> },
    AgentResponse { blocks: Vec<ContentBlock> },
    ToolResult { tool_use_id: String, output: String, is_error: bool },
}

// Instead of Value for tool input
// (This requires per-tool input types, which may be overkill)
```

**Recommendation:** Defer this phase. The current approach is working and the ROI is lower than the other phases.

---

## Implementation Order

```
Week 1:
├── Phase 1: Move pending_tools into state
│   ├── Day 1-2: Update types (ToolCall, ConvState, Event)
│   ├── Day 3: Update transition.rs
│   ├── Day 4: Update executor.rs
│   └── Day 5: Test and fix
│
Week 2:
├── Phase 2: Property tests
│   ├── Day 1: Infrastructure (strategies, helpers)
│   ├── Day 2-3: Implement 6 invariant tests
│   ├── Day 4: Run, debug shrinking failures
│   └── Day 5: CI integration
│
Week 3+ (if time permits):
└── Phase 3: I/O abstraction
    ├── Define traits
    ├── Create mocks
    ├── Refactor executor
    └── Integration tests
```

---

## Success Metrics

1. **Phase 1 Complete:**
   - `pending_tools` removed from executor
   - All existing tests pass
   - Demo still works

2. **Phase 2 Complete:**
   - 6+ property tests with 1000 cases each
   - All pass consistently
   - Added to CI

3. **Phase 3 Complete:**
   - Executor uses trait-based I/O
   - 3+ integration tests with mocks
   - No real I/O in test suite

---

## Questions for Review

1. **Phase 1:** Should `ToolCall.input` remain `Value` or use a typed enum?
2. **Phase 2:** Any specific invariants you want tested beyond the 6 proposed?
3. **Phase 3:** Is full I/O abstraction worth the refactor cost, or should we defer?
4. **General:** Any phases you want to skip or reorder?
