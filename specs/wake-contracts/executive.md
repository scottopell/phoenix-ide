# Wake Contracts — Executive Summary

## Requirements Summary

Wake contracts let an LLM agent register a persistent, conversation-
scoped commitment of the shape "wake me when condition X holds, or in
N seconds, whichever comes first." Today's spawn-and-wait surfaces
(`bash op=wait`, `tmux capture-pane` polling, `subagent` hardcoded
block) all consume one full conversation turn per wait window. For a
3-minute task with the default 30s wait, that's six turns of context
re-feed, six tool-description re-feeds, and six assistant-message
round-trips — all to deliver one bit of information (the task
finished).

Wake contracts replace the poll-loop with a single state-machine
transition: the LLM registers a contract, the conversation enters
`AwaitingWake`, and the runtime resumes the conversation when the
condition fires (or expires). The LLM consumes zero turns in between.

V1 supports one condition kind: `HandleTerminal { handle_kind,
handle_id }` — fires when a named bash/tmux/subagent handle reaches a
terminal state. Future revisions add regex-in-pane, file-changed,
port-listening, and other condition kinds against the same edge
without revisiting the state-machine or persistence layers.

## Technical Summary

A new `wake_contracts` SQLite table persists every active contract
with `(conv_id, handle_kind, handle_id, condition_json, expires_at,
fire_template_json)`. A new background `wake_router` task polls
contracts each tick (1s for HandleTerminal) and fires SSE +
synthetic-tool-result on resolution. Conversation state machine gains
`AwaitingWake { contract_ids: NonEmptyVec<ContractId> }` as a first-
class state; `is_busy()` returns true in this state; lifecycle
cascades (archive / hard-delete / abandon) cancel all contracts before
running.

The synthetic tool result delivered on fire is byte-shape-identical to
the tool result the equivalent synchronous `op=wait` would have
returned. This makes wake a drop-in replacement for polling from the
LLM's vantage point: same payload, same tool_use_id correlation, just
zero intervening turns.

Wake contracts are conversation-scoped, not WorkScope-scoped — even
when the underlying handle is shared across continuation, the
contract fires only the registering conversation. This is the
explicit deconfliction from the WorkScope work that landed last
week: WorkScope is resource lifetime; wake is conversation
resumption; conflating them was the design error the panel
identified.

Mandatory `expires_at` (default 600s, cap 1800s) prevents unbounded
commitments. Restart resync re-registers non-fired contracts; any
contract whose underlying handle did not survive restart (today: all
bash handles) immediately fires `Forgotten`, never silently
abandoned.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| REQ-WAKE-001 Registration | Proposed | LLM-facing tool surface, returns no payload synchronously |
| REQ-WAKE-002 Persistence | Proposed | `wake_contracts` SQLite table + restart resync |
| REQ-WAKE-003 Router Service | Proposed | Background poll task, 1s tick for HandleTerminal v1 |
| REQ-WAKE-004 AwaitingWake State | Proposed | First-class conv state, `is_busy()` returns true |
| REQ-WAKE-005 V1 Condition Kinds | Proposed | HandleTerminal only; regex/file/port/webhook deferred |
| REQ-WAKE-006 Wake Event Delivery | Proposed | Synthetic tool result, shape-identical to `op=wait` response |
| REQ-WAKE-007 Mandatory Timeout | Proposed | Default 600s, cap 1800s |
| REQ-WAKE-008 User Cancel | Proposed | UI surface + cancel endpoint |
| REQ-WAKE-009 Conv-Scoped | Proposed | Not WorkScope-scoped; explicit deconfliction |
| REQ-WAKE-010 First-Fire-Wins | Proposed | Multi-contract semantics for v1 |
| REQ-WAKE-011 Terminal Cause | Proposed | Fired / Expired / Cancelled / Forgotten |
| REQ-WAKE-012 Continuation Inheritance | **Open question** | Blocks on [[bash-cascade-skips-inheritor-scope]] |
| REQ-WAKE-013 User-Interrupt Semantics | **Open question** | Queue / Cancel / Reject |
| REQ-WAKE-014 Tool Description Guidance | Proposed | Description discipline for pit-of-success |
| REQ-WAKE-015 Cost Observability | Proposed | Metrics on registration rate, fire latency, forgotten ratio |

**Progress:** 0 of 15 implemented. **2 open questions must be
resolved before implementation begins** per AGENTS.md spec discipline
(open questions become explicit design decisions, not deferred prose).

## Dependencies

- `specs/bash/` REQ-BASH-002 (wait semantics) — REQ-WAKE-006 reuses
  bash wait response shape for delivery
- `specs/tmux-integration/` — TmuxPane handle condition kind
- `specs/subagents/` — SubAgent handle condition kind
- `specs/bedrock/` REQ-BED-032 (hard-delete cascade) — REQ-WAKE-004
  cancel-on-lifecycle joins the cascade

## Related Work

- **Task 62009** ([[bash-cascade-skips-inheritor-scope]], p1) —
  blocking for REQ-WAKE-012
- **WorkScope foundation** (PRs #136, #143, #139, merged) —
  established the resource-cleanup cascade infrastructure this spec
  extends with cancel-on-lifecycle (REQ-WAKE-004)
- **Persona panel review** (`/persona-panel` session 2026-05-24) —
  4 reviewers (correct-by-construction, LLM-cognitive-load, failure-
  mode, token-economics) converged on "no wake primitive" as the
  single root cause behind 6+ distinct surface complaints

## Why This Spec Exists

Phoenix today treats turn-count as free. Every spawn-and-wait flow
that exceeds `wait_seconds` pays a full LLM round-trip per poll. The
handle-cap (8) and ring-size (4MB) are explicit budgets in
`specs/bash/`; turn-count is not budgeted anywhere. Wake contracts are
the runtime acknowledging that turn-count is a first-class cost the
same way bytes-in-ring are.

The secondary motivation is correctness: today, if the LLM stops
polling, an exit reaches no one. The tombstone sits in memory until
Phoenix restart wipes it. Wake contracts make every spawn-and-wait
deliver a terminal answer in all cases — `Fired`, `Expired`,
`Cancelled`, or `Forgotten` — never silently abandoned.
