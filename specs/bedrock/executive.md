# Bedrock - Executive Summary

## Requirements Summary

Bedrock provides the core conversation state machine for PhoenixIDE. Users interact with an LLM agent through a reliable, predictable execution model: messages move through explicit idle, LLM, tool, cancellation, error, continuation, and approval states; tools execute serially; cancellation synthesizes tool results when needed to preserve transcript/API integrity; retryable versus non-retryable failures stay distinct; and committed transcript history survives restarts.

Current normative authority is `requirements.md` for timeless rules and `bedrock.allium` for precise lifecycle/state-machine behavior. ADR-026 records lifecycle-versus-WorkScope ownership; ADR-031 records first-class ProductConversation persistence and its staged single-authority cutover; ADR-032 retires Project authority and leaves repository context derived through the attached WorkScope; ADR-036 records process fail-stop when local SQLite authority cannot be established.

## Current Reality

The implementation still reflects the pre-unification Project/mode product model in several user-facing places. The durable conversation row continues to carry `ConvMode` (`Direct`, `Explore`, `Work`, `Branch`), task approval still parks in `AwaitingTaskApproval`, task resolution still drives legacy terminal actions (`/abandon-task`, `/mark-merged`), and archived versus non-archived remains the shipped lifecycle authority. Continuation already uses `continued_in_conv_id` while reusing the ProductConversation's attached WorkScope and environment. The branch-only Close foundation now carries dormant normalized Close evidence, but first-class ProductConversation persistence is not yet redesigned by migration 64 (`create_close_retirement_tables`); dormant lifecycle serves no readers and receives no dual writes, and the Open/History authority cutover remains separate work. The local SQLite fail-stop doctrine is adopted, but production does not yet provide its closed authoritative-result boundary, bounded fatal termination, or narrow authority-task supervision.

## Technical Summary

Implements Elm Architecture with a typed-effect executor boundary. The SM has two pure entry points: `handle_user_event()` for API-initiated events and `handle_outcome()` for executor results. Effects carry oneshot channels typed to their expected outcome (`LlmOutcome`, `ToolOutcome`, `SubAgentOutcome`, `PersistOutcome`) — the compiler prevents invalid event/state combinations. Persistence uses `CheckpointData::ToolRound` which structurally requires matched `tool_use`/`tool_result` pairs. Error classification is exhaustive with no `Unknown` variant. Executor loop uses `StepResult::Terminal` to force explicit exit on terminal states. Token streaming uses fire-and-forget `StreamToken` effects routed to SSE without SM state transitions. Sub-agents require mandatory `timeout: Duration` with deadline enforcement in executor `select!`. On server restart, ordinary interrupted conversations resume from idle with full message history preserved. A committed steering turn whose accepted user or skill message is still awaiting its first response preserves `LlmRequesting` from immutable receipt/transcript evidence; a synchronous recovery dispatch failure settles as a persisted typed error, and a failed error write retires the runtime for reconstruction. Durable continuation operations retain their operation identity and recovery state.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-BED-001:** Pure State Transitions | ✅ Complete | Core state machine module in src/state_machine/transition.rs |
| **REQ-BED-002:** User Message Handling | ✅ Complete | Rejects while busy, accepts from idle/error |
| **REQ-BED-003:** LLM Response Processing | ✅ Complete | Handles text, tool use, end_turn |
| **REQ-BED-004:** Tool Execution Coordination | ✅ Complete | Serial execution with state tracking |
| **REQ-BED-005:** Cancellation Handling | ✅ Complete | Synthetic tool results for cancelled tools |
| **REQ-BED-006:** Error Recovery | ✅ Complete | Retry logic with exponential backoff, ErrorKind |
| **REQ-BED-007:** State Persistence | ✅ Complete | Database persistence; ordinary turns resume from idle, committed steering turns retain bounded first-response recovery ownership, and durable continuation operations retain recovery state |
| **REQ-BED-008:** Sub-Agent Spawning | ✅ Complete | State machine support (runtime not fully implemented in MVP) |
| **REQ-BED-009:** Sub-Agent Isolation | ✅ Complete | Tool set restriction defined |
| **REQ-BED-010:** Fixed Working Directory | ✅ Complete | Set at creation, passed to tools |
| **REQ-BED-011:** Real-time Event Streaming | ✅ Complete | SSE with broadcast channels |
| **REQ-BED-012:** Context Window Tracking | ✅ Complete | Usage data stored in messages |
| **REQ-BED-013:** Image Handling | ✅ Complete | Base64 images passed to LLM |
| **REQ-BED-014:** Conversation Mode | ⏭️ Deprecated | Replaced by REQ-BED-027. Restricted/Unrestricted model superseded by Explore/Work with git worktrees |
| **REQ-BED-015:** Mode Upgrade Request | ⏭️ Deprecated | Replaced by REQ-PROJ-003/004 + REQ-BED-028. `request_mode_upgrade` tool replaced by `propose_task` flow |
| **REQ-BED-016:** Mode Downgrade | ⏭️ Deprecated | Replaced by work-lifecycle REQ-WL-002/REQ-WL-001. Mode return now tied to task merge or abandon |
| **REQ-BED-017:** Mode Communication | ✅ Complete | Mode-aware tool errors in the tool-registry builder; the system prompt directs Explore agents to `propose_task` |
| **REQ-BED-018:** Sub-Agent Mode Enforcement | ✅ Complete | Sub-agent tool sets are restricted by the tool-registry builder (tested); sub-agents inherit the parent worktree |
| **REQ-BED-019:** Context Continuation Threshold | ✅ Complete | Check at 90%, reject tools, trigger continuation |
| **REQ-BED-020:** Continuation Summary Generation | ✅ Complete | Durable operation identity, restart resume, transient retry, recoverable failure, atomic idempotent commit |
| **REQ-BED-021:** Context Exhausted State | ✅ Complete | Read-only terminal state |
| **REQ-BED-022:** Model-Specific Context Limits | ✅ Complete | Per-model thresholds, conservative default |
| **REQ-BED-023:** Context Warning Indicator | ✅ Complete | 80% warning, manual trigger option |
| **REQ-BED-024:** Sub-Agent Context Exhaustion | ✅ Complete | Fail immediately, no continuation flow |
| **REQ-BED-025:** Token-by-Token LLM Output | ✅ Complete | Task 582. Fire-and-forget `StreamToken` effects via SSE |
| **REQ-BED-026:** Sub-Agent Timeout Enforcement | ✅ Complete | Task 578. Mandatory `timeout: Duration`, deadline in executor `select!` |
| **REQ-BED-027:** Explore, Work, and Direct Conversation Modes | ✅ Complete (legacy current reality) | `ConvMode` is still persisted by the database conversion layer and surfaced by `conversationIdentity`; this remains shipped compatibility behavior until the unified lifecycle migration removes mode-driven product semantics |
| **REQ-BED-028:** Task Approval State | ✅ Complete (legacy current reality) | `ConvState::AwaitingTaskApproval` and `propose_task` parking are live in the reducer/runtime (`crates/phoenix-state-machine/src/transition.rs`, `ui/src/components/TaskApprovalReader.tsx`); approval-placement unification is not yet implemented |
| **REQ-BED-029:** Close Conversation Finalizes Open Work into History | Not implemented | Legacy archived/non-archived state remains lifecycle authority. Dormant ProductConversation lifecycle and aggregate-bound Close persistence are foundation work only; no reader may use the dormant value before the atomic Open/History cutover recomputes it from legacy truth. |
| **REQ-BED-030:** Context Continuation Inherits Parent Environment | ✅ Complete | Continuation reuses the ProductConversation's attached WorkScope and environment while `continued_in_conv_id` records parent-row topology (`create_continuation_with_source`) |
| **REQ-BED-030A:** ProductConversation Owns Aggregate Identity and Lifecycle | Not implemented | Shipped persistence still infers aggregate identity/lifecycle from root conversation rows and archived state; first-class identity/kind and explicit transcript membership are dormant foundation work, while lifecycle authority remains legacy until cutover. |
| **REQ-BED-031:** Exhausted Parent Post-Handoff Behavior | ✅ Complete | Parent cleanup is already suppressed once continuation exists; legacy abandon / mark-merged handlers reject when `continued_in_conv_id` is set |
| **REQ-BED-031A:** Start Follow-up Creates Fresh Open Conversation | Not implemented | Normative target requires a fresh ProductConversation/WorkScope, exact follow-up objective, no transcript injection, and typed `follow_up` provenance |
| **REQ-BED-031B:** Permanent Delete Removes Only the Conversation Aggregate and Is Idempotent | Not implemented | Shipped hard delete removes a legacy row-oriented shape; complete ProductConversation aggregate deletion, normalized-child coverage, and typed surviving provenance tombstones remain migration targets |
| **REQ-BED-032:** Conversation Terminal-Transition Cascade | ✅ Complete (legacy current reality) | Existing hard-delete cascade invokes bash/tmux/project-named/browser cleanup and broadcasts `ConversationHardDeleted`; aggregate-aware Delete remains governed by REQ-BED-031B and WorkScope/GitRepository authority remains a migration target |
| **REQ-BED-033:** Unclassified Local Persistence Authority Fails Stop | Not implemented (doctrine adopted) | ADR-036 and the normative requirement define the fail-stop boundary; production enforcement, bounded fatal termination, coordinated-shutdown disposition, and restored-authority admission gate remain follow-up work |

**Current implementation:** 28 requirements complete, 3 deprecated, and 5 requirements not implemented.
