# Bedrock - Executive Summary

## Requirements Summary

Bedrock provides the core conversation state machine for PhoenixIDE. Users interact with an LLM agent through a reliable, predictable execution model. Messages flow through well-defined states: idle, awaiting LLM (with retry tracking), executing tools serially, and handling errors. Tools execute one at a time in LLM-requested order to respect intent and prevent conflicts. Cancellation generates synthetic tool results to maintain message chain integrity required by LLM APIs. Error handling distinguishes retryable errors (network, rate limit) from non-retryable (auth), with automatic retry and UI-visible state. Sub-agents complete by calling a dedicated result submission tool. Each conversation has a fixed working directory set at creation. User messages with images are passed through to the LLM provider. Messages sent while agent is busy are rejected (user can cancel if needed). Conversations have an Explore or Work mode (see `specs/projects/`) stored alongside state; mode determines tool availability and is separate from state machine state.

## Technical Summary

Implements Elm Architecture with a typed-effect executor boundary. The SM has two pure entry points: `handle_user_event()` for API-initiated events and `handle_outcome()` for executor results. Effects carry oneshot channels typed to their expected outcome (`LlmOutcome`, `ToolOutcome`, `SubAgentOutcome`, `PersistOutcome`) — the compiler prevents invalid event/state combinations. Persistence uses `CheckpointData::ToolRound` which structurally requires matched `tool_use`/`tool_result` pairs. Error classification is exhaustive with no `Unknown` variant. Executor loop uses `StepResult::Terminal` to force explicit exit on terminal states. Token streaming uses fire-and-forget `StreamToken` effects routed to SSE without SM state transitions. Sub-agents require mandatory `timeout: Duration` with deadline enforcement in executor `select!`. On server restart, conversations resume from idle with full message history preserved.

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
| **REQ-BED-014:** Conversation Mode | ⏭️ Deprecated | Replaced by REQ-BED-027. Restricted/Unrestricted model superseded by Explore/Work with git worktrees |
| **REQ-BED-015:** Mode Upgrade Request | ⏭️ Deprecated | Replaced by REQ-PROJ-003/004 + REQ-BED-028. `request_mode_upgrade` tool replaced by `propose_plan` flow |
| **REQ-BED-016:** Mode Downgrade | ⏭️ Deprecated | Replaced by REQ-PROJ-009/010. Mode return now tied to task merge or abandon |
| **REQ-BED-017:** Mode Communication | ✅ Complete | Mode-aware tool errors in `crates/phoenix-ide/src/tools.rs:479`; system prompt directs Explore agents to `propose_task` (`system_prompt.rs:578`) |
| **REQ-BED-018:** Sub-Agent Mode Enforcement | ✅ Complete | Sub-agent tool sets restricted by mode in `crates/phoenix-ide/src/tools.rs:647-677` (tested); sub-agents inherit parent worktree |
| **REQ-BED-019:** Context Continuation Threshold | ✅ Complete | Check at 90%, reject tools, trigger continuation |
| **REQ-BED-020:** Continuation Summary Generation | ✅ Complete | Tool-less LLM request, fallback on failure |
| **REQ-BED-021:** Context Exhausted State | ✅ Complete | Read-only terminal state |
| **REQ-BED-022:** Model-Specific Context Limits | ✅ Complete | Per-model thresholds, conservative default |
| **REQ-BED-023:** Context Warning Indicator | ✅ Complete | 80% warning, manual trigger option |
| **REQ-BED-024:** Sub-Agent Context Exhaustion | ✅ Complete | Fail immediately, no continuation flow |
| **REQ-BED-025:** Token-by-Token LLM Output | ✅ Complete | Task 582. Fire-and-forget `StreamToken` effects via SSE |
| **REQ-BED-026:** Sub-Agent Timeout Enforcement | ✅ Complete | Task 578. Mandatory `timeout: Duration`, deadline in executor `select!` |
| **REQ-BED-027:** Explore, Work, and Direct Conversation Modes | ✅ Complete | `ConvMode` enum + DB column (migration in `db.rs:130-138`); UI surfaces via `conv_mode_label`; replaces REQ-BED-014. `Direct` absorbed the former `Standalone` mode (see REQ-PROJ-018) |
| **REQ-BED-028:** Task Approval State | ✅ Complete | `ConvState::AwaitingTaskApproval` (`state.rs:454,992`); `propose_task` interception; git ops in `effect.rs:148` / `executor.rs:2143`; UI overlay in `ui/src/components/TaskApprovalReader.tsx`; replaces REQ-BED-015 |
| **REQ-BED-029:** Conversation Terminal State on Task Resolution | ✅ Complete | `ConvState::Terminal` transitions on complete/abandon (`runtime/executor.rs:2091,2121`); task resolution events in `state_machine/event.rs:139`; UI handles `phase.type === 'terminal'`; replaces REQ-BED-016 |
| **REQ-BED-030:** Context Continuation Inherits Parent Environment | ✅ Complete | Task 24696. Worktree ownership transfers via `continued_in_conv_id` pointer; mode mapping W→W/B→B/E→E/D→D; idempotent `POST /api/conversations/:id/continue`. Obsoletes task 08678 |
| **REQ-BED-031:** Exhausted Parent Post-Handoff Behavior | ✅ Complete | Task 24696. Auto-cleanup removed; `reconcile_worktrees` skips context-exhausted + continued rows; abandon/mark-as-merged gated on `continued_in_conv_id = NULL`; typed `continuation_id` on 409 response |
| **REQ-BED-032:** Conversation Hard-Delete Cascade | ✅ Complete | `ConversationHardDeleted` lifecycle event emitted from `runtime.rs:426,1198`; bash + tmux subscribers wired; `RejectHardDeleteWhileBusy` enforced (see `bedrock.allium:899-958`) |

**Progress:** 29 of 32 complete (3 deprecated, not counted)

## Audit Trail

| Date | Pass | Notes |
|---|---|---|
| 2026-05-10 | Automated review (Copilot/codex) | First pass over the legacy spec since the doc-rot audit. Goal: surface drift between spec language, status claims, and current code. Findings are addressed inline on this PR. |
