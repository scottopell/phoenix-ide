---
created: 2026-04-20
priority: p3
status: ready
artifact: src/state_machine/state.rs
---

# Consider moving AwaitingContinuation from CoreState to ParentState

## Summary

Continuation is arguably parent-only behavior. Sub-agents fail when context
is exhausted rather than continuing. Currently in CoreState because both paths
flow through it, but the sub-agent path goes straight to Failed.

## Done When

Decision documented. If moved, all tests pass and the sub-agent path is verified.
