---
created: 2026-04-20
priority: p2
status: ready
artifact: src/runtime/executor.rs
---

# Extract dispatch_llm_request and dispatch_tool_execution from execute_effect

## Summary

`execute_effect` is ~700 lines with 14 match arms. The `RequestLlm` arm is
~240 lines with 7 inlined concerns (cycle cap, turn limits, grace turns,
message building, streaming channels, LLM task spawn, forwarder task).

## Done When

execute_effect is a ~200-line dispatcher. Guard logic in dispatch_llm_request
is independently testable. All tests pass.
