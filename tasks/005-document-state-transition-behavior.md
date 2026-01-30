---
created: 2026-01-29
priority: p3
status: done
---

# Document State Transition Behavior Inconsistencies

## Summary

Document or fix the inconsistent error types returned when user messages are rejected in different busy states.

## Context

During property testing, we found:
- `LlmRequesting` + `UserMessage` → `Err(AgentBusy)`
- `ToolExecuting` + `UserMessage` → `Err(AgentBusy)`
- `AwaitingLlm` + `UserMessage` → `Err(InvalidTransition)` ← inconsistent!
- `Cancelling` + `UserMessage` → `Err(CancellationInProgress)`

## Resolution

Fixed by adding `AwaitingLlm` to the list of busy states that return `AgentBusy`.

Now all "busy" states consistently return `AgentBusy`, while cancelling states return `CancellationInProgress`.

## Changes

- `src/state_machine/transition.rs`: Added `AwaitingLlm` to the busy states pattern match
