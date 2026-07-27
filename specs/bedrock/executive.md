# Bedrock - Executive Summary

## Requirements Summary

Bedrock provides the core conversation state machine for PhoenixIDE. Users interact with an LLM agent through a reliable, predictable execution model: messages move through explicit idle, LLM, tool, cancellation, error, continuation, and approval states; tools execute serially; cancellation synthesizes tool results when needed to preserve transcript/API integrity; retryable versus non-retryable failures stay distinct; and committed transcript history survives restarts.

Current normative authority is `requirements.md` for timeless rules, `bedrock.allium` for precise lifecycle/state-machine behavior, and ADR-025 for lifecycle-versus-WorkScope ownership.

## Current Reality

The implementation still reflects the pre-unification product model in several user-facing places. The durable conversation row continues to carry `ConvMode` (`Direct`, `Explore`, `Work`, `Branch`), task approval still parks in `AwaitingTaskApproval`, task resolution still drives legacy terminal actions (`/abandon-task`, `/mark-merged`), and archived versus non-archived remains the shipped lifecycle split. Continuation already uses `continued_in_conv_id` and transfers the live worktree/work-scope forward, but Phoenix has not yet cut over to one Open/History conversation surface.

## Technical Summary

Implements Elm Architecture with a typed-effect executor boundary. The SM has two pure entry points: `handle_user_event()` for API-initiated events and `handle_outcome()` for executor results. Effects carry oneshot channels typed to their expected outcome (`LlmOutcome`, `ToolOutcome`, `SubAgentOutcome`, `PersistOutcome`) — the compiler prevents invalid event/state combinations. Persistence uses `CheckpointData::ToolRound` which structurally requires matched `tool_use`/`tool_result` pairs. Error classification is exhaustive with no `Unknown` variant. Executor loop uses `StepResult::Terminal` to force explicit exit on terminal states. Token streaming uses fire-and-forget `StreamToken` effects routed to SSE without SM state transitions. Sub-agents require mandatory `timeout: Duration` with deadline enforcement in executor `select!`. On server restart, ordinary interrupted conversations resume from idle with full message history preserved; durable continuation operations retain their operation identity and recovery state.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-BED-001:** Pure State Transitions | ✅ Complete | Core state machine module in src/state_machine/transition.rs |
| **REQ-BED-002:** User Message Handling | ✅ Complete | Rejects while busy, accepts from idle/error |
| **REQ-BED-003:** LLM Response Processing | ✅ Complete | Handles text, tool use, end_turn |
| **REQ-BED-004:** Tool Execution Coordination | ✅ Complete | Serial execution with state tracking |
| **REQ-BED-005:** Cancellation Handling | ✅ Complete | Synthetic tool results for cancelled tools |
| **REQ-BED-006:** Error Recovery | ✅ Complete | Retry logic with exponential backoff, ErrorKind |
| **REQ-BED-007:** State Persistence | ✅ Complete | Database persistence; ordinary turns resume from idle while durable continuation operations retain recovery state |
| **REQ-BED-008:** Sub-Agent Spawning | ✅ Complete | State machine support (runtime not fully implemented in MVP) |
| **REQ-BED-009:** Sub-Agent Isolation | ✅ Complete | Tool set restriction defined |
| **REQ-BED-010:** Fixed Working Directory | ✅ Complete | Set at creation, passed to tools |
| **REQ-BED-011:** Real-time Event Streaming | ✅ Complete | SSE with broadcast channels |
| **REQ-BED-012:** Context Window Tracking | ✅ Complete | Usage data stored in messages |
| **REQ-BED-013:** Image Handling | ✅ Complete | Base64 images passed to LLM |
| **REQ-BED-014:** Conversation Mode | ⏭️ Deprecated | Replaced by REQ-BED-027. Restricted/Unrestricted model superseded by Explore/Work with git worktrees |
| **REQ-BED-015:** Mode Upgrade Request | ⏭️ Deprecated | Replaced by REQ-PROJ-003/004 + REQ-BED-028. `request_mode_upgrade` tool replaced by `propose_task` flow |
| **REQ-BED-016:** Mode Downgrade | ⏭️ Deprecated | Replaced by work-lifecycle REQ-WL-002/REQ-WL-001. Mode return now tied to task merge or abandon |
| **REQ-BED-017:** Mode Communication | ✅ Complete | Mode-aware tool errors in `crates/phoenix-ide/src/tools.rs:479`; system prompt directs Explore agents to `propose_task` (`system_prompt.rs:578`) |
| **REQ-BED-018:** Sub-Agent Mode Enforcement | ✅ Complete | Sub-agent tool sets restricted by mode in `crates/phoenix-ide/src/tools.rs:647-677` (tested); sub-agents inherit parent worktree |
| **REQ-BED-019:** Context Continuation Threshold | ✅ Complete | Check at 90%, reject tools, trigger continuation |
| **REQ-BED-020:** Continuation Summary Generation | ✅ Complete | Durable operation identity, restart resume, transient retry, recoverable failure, atomic idempotent commit |
| **REQ-BED-021:** Context Exhausted State | ✅ Complete | Read-only terminal state |
| **REQ-BED-022:** Model-Specific Context Limits | ✅ Complete | Per-model thresholds, conservative default |
| **REQ-BED-023:** Context Warning Indicator | ✅ Complete | 80% warning, manual trigger option |
| **REQ-BED-024:** Sub-Agent Context Exhaustion | ✅ Complete | Fail immediately, no continuation flow |
| **REQ-BED-025:** Token-by-Token LLM Output | ✅ Complete | Task 582. Fire-and-forget `StreamToken` effects via SSE |
| **REQ-BED-026:** Sub-Agent Timeout Enforcement | ✅ Complete | Task 578. Mandatory `timeout: Duration`, deadline in executor `select!` |
| **REQ-BED-027:** Explore, Work, and Direct Conversation Modes | ✅ Complete (legacy current reality) | `ConvMode` is still persisted and surfaced (`crates/phoenix-db/src/lib.rs:8147-8255`, `ui/src/utils/conversationIdentity.ts`); this remains shipped compatibility behavior until the unified lifecycle migration removes mode-driven product semantics |
| **REQ-BED-028:** Task Approval State | ✅ Complete (legacy current reality) | `ConvState::AwaitingTaskApproval` and `propose_task` parking are live in the reducer/runtime (`crates/phoenix-state-machine/src/transition.rs`, `ui/src/components/TaskApprovalReader.tsx`); approval-placement unification is not yet implemented |
| **REQ-BED-029:** Conversation Terminal State on Task Resolution | ✅ Complete (legacy current reality) | Task resolution still reaches terminal cleanup through legacy abandon / mark-merged flows (`crates/phoenix-ide/src/api/lifecycle_handlers.rs`, `crates/phoenix-state-machine/src/transition.rs`) rather than the future Close→History cutover |
| **REQ-BED-030:** Context Continuation Inherits Parent Environment | ✅ Complete | Continuation still reuses the attached environment via `continued_in_conv_id` / transferred work-scope ownership (`crates/phoenix-db/src/lib.rs:5316-5537`) |
| **REQ-BED-031:** Exhausted Parent Post-Handoff Behavior | ✅ Complete | Parent cleanup is already suppressed once continuation exists; legacy abandon / mark-merged endpoints reject when `continued_in_conv_id` is set (`crates/phoenix-ide/src/api/lifecycle_handlers.rs:908-918`) |
| **REQ-BED-032:** Conversation Hard-Delete Cascade | ✅ Complete | `ConversationHardDeleted` lifecycle event emitted from `runtime.rs:426,1198`; bash + tmux subscribers wired; `RejectHardDeleteWhileBusy` enforced (see `bedrock.allium:899-958`) |

**Progress:** 29 of 32 complete (3 deprecated, not counted)
