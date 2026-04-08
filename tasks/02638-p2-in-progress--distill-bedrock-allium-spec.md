---
created: 2026-04-08
priority: p2
status: in-progress
artifact: pending
---

# distill-bedrock-allium-spec

## Plan


# Distill Bedrock Conversation Engine into Allium Specification

## Summary

Create `specs/bedrock/bedrock.allium` — a formal behavioral specification of the Phoenix conversation engine, distilled from the existing spEARS spec (requirements.md, design.md) and the Rust implementation (src/state_machine/).

This is the first Allium spec in the project, demonstrating how spEARS + Allium work together: spEARS captures *why* (user stories, requirement IDs), Allium captures *what exactly* (states, transitions, invariants, preconditions).

## Context

The bedrock spec has 26 active requirements (REQ-BED-001 through REQ-BED-029), a thorough design doc, and a clean Elm Architecture implementation. Despite this, a deep analysis found:

- **5 implementation-blocking ambiguities** in the design doc
- **10+ design clarity issues** (e.g., "busy state" undefined, crash recovery semantics unclear)
- **Invalid state combinations** representable but not structurally prevented

Allium forces these ambiguities into the open through transition graphs (completeness obligations), invariants (structural properties), and precondition/postcondition rules.

## Scope

**Includes:**
- Conversation lifecycle state machine (15 states, all transitions)
- Conversation mode system (Explore, Work, Direct) as a sum type
- Tool execution lifecycle (serial execution, atomic persistence)
- Cancellation flows (LLM, tool, sub-agent)
- Error retry logic (3 attempts, exponential backoff)
- Context continuation (80% warning, 90% threshold, exhaustion)
- Task approval flow (propose → approve/reject/feedback)
- Sub-agent lifecycle (spawning, fan-in, timeout, cancellation)
- Sole-tool interception (propose_task, ask_user_question, submit_result/submit_error)
- User question flow (ask → respond/cancel)
- Terminal state semantics

**Excludes (separate specs / implementation):**
- Git operations (projects.allium — future)
- Individual tool implementations (bash, patch, etc.)
- LLM provider specifics (Anthropic, OpenAI)
- Database persistence mechanics (SQLite, serde)
- SSE streaming / HTTP API design
- UI rendering / display state mapping
- Token streaming mechanics
- Message expansion (@file references)
- System prompt construction

## What to do

### 1. Create `specs/bedrock/bedrock.allium`

The spec will follow Allium v3 syntax and contain:

**Scope block** — documents included/excluded scope with requirement traceability

**External entities:**
- `LlmProvider` — the LLM service (external, produces responses/errors)
- `Tool` — a tool implementation (external, produces results)

**Value types:**
- `ToolCall` — a tool invocation with ID and input
- `SubAgentSpec` — specification for spawning a sub-agent

**Enums:**
- `ConversationStatus` — the 15-state lifecycle (with transition graph)
- `ErrorClass` — retryable | non_retryable
- `SubAgentOutcome` — success | failure | timed_out
- `TaskApprovalDecision` — approved | rejected | feedback_provided
- `ContextExhaustionBehavior` — threshold_based | immediate_failure

**Entities:**
- `Conversation` — core entity with status, mode (sum type with Explore/Work/Direct variants), context tracking, sub-agent flag
- `ToolRound` — atomic unit of assistant message + tool results
- `SubAgent` — child conversation with parent reference, task, timeout
- `TaskProposal` — proposed plan awaiting approval

**Config:**
- `max_retry_attempts: Integer = 3`
- `context_warning_threshold: Decimal = 0.80`
- `context_continuation_threshold: Decimal = 0.90`
- `max_sub_agents_per_spawn: Integer = 10`

**Rules organized by flow:**
1. **User Message Handling** (REQ-BED-002): Idle/Error → LlmRequesting; busy states → reject
2. **LLM Response Processing** (REQ-BED-003): text-only → Idle; tools → ToolExecuting; sole-tool interception
3. **Tool Execution** (REQ-BED-004): serial queue, SpawnAgents accumulation, atomic checkpoint on completion
4. **Cancellation** (REQ-BED-005): LLM abort, tool abort with synthetic results, sub-agent cancellation
5. **Error Recovery** (REQ-BED-006): retryable errors → retry with backoff; non-retryable → Error state; sub-agents → Failed
6. **Sub-Agent Lifecycle** (REQ-BED-008/009/018/024/026): spawn, fan-in, timeout, mode enforcement
7. **Task Approval** (REQ-BED-028): propose_task interception → AwaitingTaskApproval → approve/reject/feedback
8. **User Questions** (REQ-AUQ-001): ask_user_question interception → AwaitingUserResponse → answer/cancel
9. **Context Continuation** (REQ-BED-019-024): threshold detection, continuation summary, exhaustion
10. **Terminal States** (REQ-BED-029): reject user messages, absorb other events

**Invariants:**
- Tool round atomicity: assistant message tool_use count = tool result count
- Sub-agent fan-in conservation: pending.count + completed.count = total spawned
- Sub-agents cannot spawn sub-agents (no recursion)
- Sole-tool constraint: propose_task, ask_user_question, submit_result, submit_error must be the only tool in a response
- One Work sub-agent per parent maximum
- Terminal states have no outbound transitions
- Context exhaustion threshold > warning threshold

**Surfaces:**
- `UserConversation` — user-facing boundary (send messages, cancel, approve tasks, answer questions, trigger continuation)
- `AgentExecution` — agent-facing boundary (LLM responses flow in, tool calls flow out)

### 2. Add requirement traceability comments

Each rule will reference its spEARS requirement ID:
```
-- REQ-BED-002: User message handling
rule UserSendsMessage { ... }
```

### 3. Validate against implementation

Cross-check every transition in `src/state_machine/transition.rs` against the Allium spec to ensure completeness. Document any discrepancies as open questions.

## Acceptance criteria

- [ ] `specs/bedrock/bedrock.allium` exists and follows Allium v3 syntax
- [ ] All 15 ConvState variants are modeled as conversation status values
- [ ] ConvMode is modeled as a sum type (Explore/Work/Direct variants)  
- [ ] Transition graph declares all valid status transitions with terminal states
- [ ] Every transition in `src/state_machine/transition.rs` has a corresponding rule
- [ ] Each rule references its spEARS requirement ID (REQ-BED-*)
- [ ] Invariants capture the structural properties (atomicity, fan-in conservation, sole-tool, no recursion)
- [ ] At least one surface defines the user-facing boundary contract
- [ ] No implementation details leak (no SQLite, no Rust types, no HTTP routes, no SSE events)
- [ ] Open questions document any ambiguities found during distillation
- [ ] Scope block at top of file documents includes/excludes


## Progress

