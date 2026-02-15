# Bedrock - Executive Summary

## Requirements Summary

Bedrock provides the core conversation state machine for PhoenixIDE. Users interact with an LLM agent through a reliable, predictable execution model. Messages flow through well-defined states: idle, awaiting LLM (with retry tracking), executing tools serially, and handling errors. Tools execute one at a time in LLM-requested order to respect intent and prevent conflicts. Cancellation generates synthetic tool results to maintain message chain integrity required by LLM APIs. Error handling distinguishes retryable errors (network, rate limit) from non-retryable (auth), with automatic retry and UI-visible state. Sub-agents complete by calling a dedicated result submission tool. Each conversation has a fixed working directory set at creation. User messages with images are passed through to the LLM provider. Messages sent while agent is busy are rejected (user can cancel if needed).

## Technical Summary

Implements Elm Architecture: pure `transition(state, event) -> (new_state, effects)` function with all I/O isolated in effect executors. State machine tracks retry attempts in `LlmRequesting{attempt}` state, pending/completed tools in `ToolExecuting` state. Cancellation produces synthetic `ToolResult::Cancelled` for current and remaining tools. Error state includes `ErrorKind` enum for UI display. Sub-agents receive `submit_result` tool instead of `spawn_sub_agent` tool, preventing nesting. Database schema stores `state_data` JSON for state-specific fields. One runtime event loop per conversation coordinates state machine and executor. On server restart, conversations resume from idle with full message history preserved.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-BED-001:** Pure State Transitions | ✅ Complete | Core state machine module in src/state_machine/transition.rs |
| **REQ-BED-002:** User Message Handling | ✅ Complete | Rejects while busy, accepts from idle/error |
| **REQ-BED-003:** LLM Response Processing | ✅ Complete | Handles text, tool use, end_turn |
| **REQ-BED-004:** Tool Execution Coordination | ✅ Complete | Serial execution with state tracking |
| **REQ-BED-005:** Cancellation Handling | ✅ Complete | Synthetic tool results for cancelled tools |
| **REQ-BED-006:** Error Recovery | ✅ Complete | Retry logic with exponential backoff, ErrorKind |
| **REQ-BED-007:** State Persistence | ✅ Complete | Database persistence, resume from idle on restart |
| **REQ-BED-008:** Sub-Agent Spawning | ✅ Complete | State machine support (runtime not fully implemented in MVP) |
| **REQ-BED-009:** Sub-Agent Isolation | ✅ Complete | Tool set restriction defined |
| **REQ-BED-010:** Fixed Working Directory | ✅ Complete | Set at creation, passed to tools |
| **REQ-BED-011:** Real-time Event Streaming | ✅ Complete | SSE with broadcast channels |
| **REQ-BED-012:** Context Window Tracking | ✅ Complete | Usage data stored in messages |
| **REQ-BED-013:** Image Handling | ✅ Complete | Base64 images passed to LLM |
| **REQ-BED-014:** Conversation Mode | ❌ Not Started | Restricted/Unrestricted modes with Landlock |
| **REQ-BED-015:** Mode Upgrade Request | ❌ Not Started | Agent requests upgrade, user approves |
| **REQ-BED-016:** Mode Downgrade | ❌ Not Started | Immediate user-initiated downgrade |
| **REQ-BED-017:** Mode Communication | ❌ Not Started | Synthetic messages, error responses |
| **REQ-BED-018:** Sub-Agent Mode Enforcement | ❌ Not Started | Sub-agents always Restricted (when available) |
| **REQ-BED-019:** Context Continuation Threshold | ❌ Not Started | Check at 90%, reject tools, trigger continuation |
| **REQ-BED-020:** Continuation Summary Generation | ❌ Not Started | Tool-less LLM request, fallback on failure |
| **REQ-BED-021:** Context Exhausted State | ❌ Not Started | Read-only terminal state |
| **REQ-BED-022:** Model-Specific Context Limits | ❌ Not Started | Per-model thresholds, conservative default |
| **REQ-BED-023:** Context Warning Indicator | ❌ Not Started | 80% warning, manual trigger option |
| **REQ-BED-024:** Sub-Agent Context Exhaustion | ❌ Not Started | Fail immediately, no continuation flow |

**Progress:** 13 of 24 complete
