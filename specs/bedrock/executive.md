# Bedrock - Executive Summary

## Requirements Summary

Bedrock provides the core conversation state machine for PhoenixIDE. Users interact with an LLM agent through a reliable, predictable execution model. Messages flow through well-defined states: idle, awaiting LLM, executing tools, and handling errors. The system supports concurrent tool execution, graceful cancellation, automatic retry on transient failures, and server restart recovery. Sub-agents enable parallel task execution for bootstrapping and complex operations, with strict isolation preventing nested spawning. Each conversation has an immutable working directory, eliminating state confusion. Real-time event streaming keeps the UI responsive.

## Technical Summary

Implements the Elm Architecture: pure `transition(state, event) -> (new_state, effects)` function at the core, with all I/O isolated in effect executors. State machine has 7 primary states with explicit transitions. Effects include persistence, LLM requests, tool execution, sub-agent spawning, and client notifications. Database schema tracks conversation state, messages, and pending followups. One runtime event loop per active conversation coordinates the state machine and executor. Property-based testing validates invariants: deterministic transitions, no async effects from terminal states, consistent tool ID handling.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-BED-001:** Pure State Transitions | ❌ Not Started | Core state machine module |
| **REQ-BED-002:** User Message Handling | ❌ Not Started | Depends on REQ-BED-001 |
| **REQ-BED-003:** LLM Response Processing | ❌ Not Started | Depends on REQ-BED-001 |
| **REQ-BED-004:** Tool Execution Coordination | ❌ Not Started | Depends on REQ-BED-001 |
| **REQ-BED-005:** Cancellation Handling | ❌ Not Started | Depends on REQ-BED-001 |
| **REQ-BED-006:** Error Recovery | ❌ Not Started | Depends on REQ-BED-001 |
| **REQ-BED-007:** State Persistence | ❌ Not Started | Database schema + executor |
| **REQ-BED-008:** Sub-Agent Spawning | ❌ Not Started | Depends on REQ-BED-001 |
| **REQ-BED-009:** Sub-Agent Isolation | ❌ Not Started | Depends on REQ-BED-008 |
| **REQ-BED-010:** Immutable Working Directory | ❌ Not Started | Context setup |
| **REQ-BED-011:** Real-time Event Streaming | ❌ Not Started | SSE infrastructure |
| **REQ-BED-012:** Context Window Tracking | ❌ Not Started | Usage data in messages |

**Progress:** 0 of 12 complete
