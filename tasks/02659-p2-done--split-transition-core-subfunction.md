---
created: 2026-04-20
priority: p2
status: done
artifact: src/state_machine/transition.rs
---

# Extract handle_llm_response, handle_tool_complete from transition_core

## Summary

`transition_core` is ~800 lines. Extracting `handle_llm_response`,
`handle_tool_complete`, and `handle_cancellation` as sub-functions would
reduce ordering dependencies and make each concern independently testable.

## Done When

transition_core is a ~200-line router dispatching to domain-specific handlers.
All 598+ tests pass.
