---
created: 2026-04-10
priority: p2
status: done
artifact: src/state_machine/state.rs
---

# Fix `is_terminal()` Missing `ContextExhausted`

## Summary

`ConvState::is_terminal()` returns `false` for `ContextExhausted`, contradicting
the bedrock spec and the rest of the codebase. The fix is a one-line addition to
the `matches!` macro in `src/state_machine/state.rs`.

## The Bug

**File:** `src/state_machine/state.rs:530-535`

```rust
// CURRENT (wrong):
pub fn is_terminal(&self) -> bool {
    matches!(
        self,
        ConvState::Completed { .. } | ConvState::Failed { .. } | ConvState::Terminal
        // ContextExhausted is MISSING
    )
}
```

**bedrock.allium spec (line 252):**
```allium
is_terminal: status in { context_exhausted, terminal, completed, failed }
```

**The fix:**
```rust
pub fn is_terminal(&self) -> bool {
    matches!(
        self,
        ConvState::ContextExhausted { .. }
            | ConvState::Completed { .. }
            | ConvState::Failed { .. }
            | ConvState::Terminal
    )
}
```

## Why It’s Currently Benign

`is_terminal()` is called in exactly three places, all in `transition.rs`,
all guarding sub-agent wildcard transitions:

```rust
// All three have this shape:
(state, Event::GraceTurnExhausted { .. })
    if context.is_sub_agent && !state.is_terminal() => { ... }

(state, Event::UserCancel { .. })
    if context.is_sub_agent && !state.is_terminal() => { ... }
```

Sub-agents cannot reach `ContextExhausted` — REQ-BED-024 specifies they fail
immediately on context exhaustion (see `src/state_machine/transition.rs`,
`ContextThresholdReachedSubAgent` rule). So the bug never fires in current code.

## Why It Must Be Fixed Now

The terminal feature (task 24657) implements `TerminalAbandonedWithConversation`,
which tears down the PTY when a conversation reaches a terminal state. The
implementation will call `is_terminal()` (or subscribe to `ConversationBecameTerminal`
which itself uses `is_terminal()`). A parent conversation CAN reach `ContextExhausted`,
and if `is_terminal()` returns `false` for it, the terminal session will not be torn
down — an orphan shell with a leaked master fd.

Also: `is_terminal()` is a semantic contract. The rest of the codebase already
treats `ContextExhausted` as terminal everywhere else:
- `step_result()` returns `StepResult::Terminal` for it (line 549)
- `display_state()` maps it to `DisplayState::Terminal` (line 577)
- The bedrock spec is unambiguous

A lying `is_terminal()` is a trap for every future caller.

## Discovered During

Spec-vs-code alignment check on task 24657 (terminal spec). The alignment check
compared `is_terminal` in bedrock.allium against the Rust implementation and
found the divergence.

## Acceptance Criteria

- [ ] `ConvState::ContextExhausted` added to the `matches!` in `is_terminal()`
- [ ] Existing tests pass (`cargo test`)
- [ ] New unit test: `ConvState::ContextExhausted { summary: "...".into() }.is_terminal() == true`
- [ ] `./dev.py check` passes
