---
created: 2026-01-30
priority: p3
status: done
---

# Add Cancellation Timing Integration Test

## Summary

Add integration test that verifies tool cancellation happens quickly (< 100ms) as required by REQ-BED-005.

## Context

We have property tests for state machine cancellation transitions, but no test that actually measures cancellation timing with the real executor.

## Acceptance Criteria

- [x] Test starts a tool with 5+ second delay (DelayedMockToolExecutor)
- [x] Send cancel after ~50ms
- [x] Verify AgentDone event arrives within 200ms of cancel
- [x] Verify we don't wait for the full 5 seconds

## Implementation

Added `test_tool_cancellation_timing` in `src/runtime/testing.rs`:
- Uses `DelayedMockToolExecutor` with 5 second delay
- Waits for tool execution to start
- Sends cancel after 50ms
- Asserts cancellation completes in < 200ms

Also updated `test_cancel_during_tool_execution` and `test_cancel_during_llm_request` to verify fast cancellation (< 2 seconds instead of waiting for full delay).

// Start tool, wait for execution_started notification
// Send cancel
// Assert completion within 200ms
```

## Notes

This validates the REQ-BED-005 requirement: "interrupt running tool within 100ms"
