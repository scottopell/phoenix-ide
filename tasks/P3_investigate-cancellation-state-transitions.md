---
created: 2025-02-07
priority: p3
status: ready
---

# Investigate: Cancellation as First-Class State Transitions

## ⚠️ INVESTIGATION ONLY

**This task is for investigation and documentation only. Do NOT implement any code fixes.**

Deliver findings as a report appended to this file. Document architectural differences, identify any gaps in test coverage or potential issues, but do not write implementation code.

## Summary

Compare cancellation architecture between rustey-shelley and phoenix-ide. Both have explicit cancellation states, but the implementations differ. Goal: identify which patterns are more robust.

## rustey-shelley's Evolution (commit b77197d)

rustey-shelley refactored cancellation from ad-hoc to first-class:

### New States Added
```rust
pub enum ConvState {
    Idle,
    AwaitingLlm,
    LlmRequesting,    // NEW: distinguishes awaiting vs active LLM calls
    ToolExecuting,
    AwaitingToolResponse,
    Cancelling,       // NEW: explicit cancellation transition state
    Completed,
    Error,
}
```

### Key Methods
```rust
impl ConvState {
    fn is_cancellable(&self) -> bool {
        matches!(self, 
            Self::AwaitingLlm | 
            Self::LlmRequesting | 
            Self::ToolExecuting | 
            Self::AwaitingToolResponse
        )
    }
}
```

### State Machine Module
```rust
pub enum StateEvent {
    UserMessage,
    LlmRequest,
    LlmResponse,
    ToolCallsReceived,
    ToolStart,
    ToolComplete,
    CancelRequested,
    CancelCompleted,
    ErrorOccurred,
}

pub fn transition(current: ConvState, event: StateEvent) -> Option<ConvState> {
    match (current, event) {
        // Cancellation only from cancellable states
        (state, CancelRequested) if state.is_cancellable() => Some(Cancelling),
        (Cancelling, CancelCompleted) => Some(Idle),
        // ...
    }
}
```

### Property Tests
```rust
proptest! {
    fn test_cancellation_always_leads_to_cancelling_or_none(state in arb_state()) {
        let result = transition(state, StateEvent::CancelRequested);
        match result {
            Some(new_state) => prop_assert_eq!(new_state, ConvState::Cancelling),
            None => prop_assert!(!state.is_cancellable()),
        }
    }
    
    fn test_cancelling_only_goes_to_idle(event in arb_event()) {
        let result = transition(ConvState::Cancelling, event);
        // Can only go to Idle (via CancelCompleted) or Error
    }
}
```

## Investigation Tasks

### 1. Map phoenix-ide's cancellation states

- [ ] List all states in `src/state_machine/state.rs`
- [ ] Identify cancellation-related states
- [ ] Compare with rustey-shelley's model

### 2. Analyze transition rules

- [ ] What events trigger cancellation?
- [ ] From which states can cancellation occur?
- [ ] What's the transition path? (direct to Idle? via Cancelling?)

### 3. Compare property tests

- [ ] What invariants does phoenix-ide test?
- [ ] Does it test cancellation specifically?
- [ ] Are there gaps in test coverage?

### 4. Identify differences

Phoenix has richer state machine with sub-agents. Compare:
- [ ] `CancellingLlm` vs `Cancelling`
- [ ] `CancellingTool` vs generic cancelling
- [ ] `CancellingSubAgents` (phoenix-specific)

### 5. Test edge cases

- [ ] Cancel during LLM streaming
- [ ] Cancel during tool execution
- [ ] Cancel during sub-agent execution (phoenix-specific)
- [ ] Double-cancel (cancel while already cancelling)
- [ ] Cancel completed conversation (should fail)

## Pit of Success Analysis

Both implementations aim for pit of success. Compare:

1. **State granularity:** Phoenix has more states (CancellingLlm, CancellingTool, CancellingSubAgents). Is this better or over-engineering?

2. **Effect-driven:** Phoenix separates transitions from side effects. Rustey-shelley couples them more.

3. **Property coverage:** Which has more comprehensive invariant tests?

4. **Error recovery:** What happens if cancellation fails mid-way?

## Reference Files

**rustey-shelley:**
- `src/agent/state_machine.rs` - full state machine with proptests
- `src/agent/loop.rs` - cancellation integration
- `src/api/handlers.rs` - cancel endpoint validation

**phoenix-ide:**
- `src/state_machine/state.rs` - state definitions
- `src/state_machine/transition.rs` - transition rules
- `src/state_machine/effect.rs` - side effects
- `src/state_machine/proptests.rs` - property tests

## Success Criteria

- Document architectural differences in cancellation handling
- Identify any states/transitions that could be invalid
- Compare proptest coverage - are there gaps?
- **Document any recommended improvements** (do not implement)

---

## Investigation Findings

*(Append findings below this line)*
