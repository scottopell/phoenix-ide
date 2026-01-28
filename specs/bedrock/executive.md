# Bedrock - Executive Summary

## Requirements Summary

Bedrock provides the core conversation state machine for PhoenixIDE. Users interact with an LLM agent through a reliable, predictable execution model. Messages flow through well-defined states: idle, awaiting LLM (with retry tracking), executing tools serially, and handling errors. Tools execute one at a time in LLM-requested order to respect intent and prevent conflicts. Cancellation generates synthetic tool results to maintain message chain integrity required by LLM APIs. Error handling distinguishes retryable errors (network, rate limit) from non-retryable (auth), with automatic retry and UI-visible state. Sub-agents complete by calling a dedicated result submission tool. Each conversation has a fixed working directory set at creation.

## Technical Summary

Implements Elm Architecture: pure `transition(state, event) -> (new_state, effects)` function with all I/O isolated in effect executors. State machine tracks retry attempts in `LlmRequesting{attempt}` state, pending/completed tools in `ToolExecuting` state. Cancellation produces synthetic `ToolResult::Cancelled` for current and remaining tools. Error state includes `ErrorKind` enum for UI display. Sub-agents receive `submit_result` tool instead of `spawn_sub_agent` tool, preventing nesting. Database schema stores `state_data` JSON for state-specific fields. One runtime event loop per conversation coordinates state machine and executor.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-BED-001:** Pure State Transitions | ❌ Not Started | Core state machine module |
| **REQ-BED-002:** User Message Handling | ❌ Not Started | Depends on REQ-BED-001 |
| **REQ-BED-003:** LLM Response Processing | ❌ Not Started | Depends on REQ-BED-001 |
| **REQ-BED-004:** Tool Execution Coordination | ❌ Not Started | Serial execution |
| **REQ-BED-005:** Cancellation Handling | ❌ Not Started | Synthetic tool results |
| **REQ-BED-006:** Error Recovery | ❌ Not Started | Retry logic in state, ErrorKind |
| **REQ-BED-007:** State Persistence | ❌ Not Started | Database schema + executor |
| **REQ-BED-008:** Sub-Agent Spawning | ❌ Not Started | submit_result tool |
| **REQ-BED-009:** Sub-Agent Isolation | ❌ Not Started | Tool set restriction |
| **REQ-BED-010:** Fixed Working Directory | ❌ Not Started | Set at creation |
| **REQ-BED-011:** Real-time Event Streaming | ❌ Not Started | SSE infrastructure |
| **REQ-BED-012:** Context Window Tracking | ❌ Not Started | Usage data in messages |

**Progress:** 0 of 12 complete
