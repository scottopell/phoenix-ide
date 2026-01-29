---
created: 2026-01-29
priority: p3
status: ready
---

# Document State Transition Behavior Inconsistencies

## Summary

Document or fix the inconsistent error types returned when user messages are rejected in different busy states.

## Context

During property testing, we found:
- `LlmRequesting` + `UserMessage` → `Err(AgentBusy)`
- `ToolExecuting` + `UserMessage` → `Err(AgentBusy)`
- `AwaitingLlm` + `UserMessage` → `Err(InvalidTransition)`
- `Cancelling` + `UserMessage` → `Err(CancellationInProgress)`

The `AwaitingLlm` case returns a different error type than other busy states. This may be intentional (AwaitingLlm is a transient internal state) but should be documented.

## Acceptance Criteria

- [ ] Either: Add explicit handling for `AwaitingLlm` + `UserMessage` → `AgentBusy`
- [ ] Or: Document in code comments why `AwaitingLlm` uses `InvalidTransition`
- [ ] Update property test `prop_busy_rejects_messages` to reflect intended behavior

## Notes

Location: `src/state_machine/transition.rs`

The current proptest just checks `result.is_err()` which works but doesn't verify the specific error type is appropriate.
