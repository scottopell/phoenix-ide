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

Wake contracts replace the poll-loop with persistence + an
asynchronous router: the LLM registers a contract, the conversation
stays in `Idle`, and the runtime delivers a synthetic tool result
into the conversation when the condition fires (or expires). The LLM
consumes zero turns in between.

V1 supports one condition kind: `HandleTerminal { handle_kind,
handle_id }` — fires when a named bash/tmux/subagent handle reaches a
terminal state. Future revisions add regex-in-pane, file-changed,
port-listening, and other condition kinds against the same edge
without revisiting the persistence layer or the delivery path.

## Technical Summary

A new `wake_contracts` SQLite table persists every active contract
with `(id, conv_id, handle_kind, handle_id, condition_json,
expires_at, registered_at, fire_template_json, registering_tool_use_id)`.
A new background `wake_router` task polls contracts each tick (1s for
HandleTerminal) and on resolution: marks the row terminal, appends a
synthetic tool result to the conv message log, triggers the conv's
next LLM turn, and emits SSE.

**No new conversation state.** The conv stays in `Idle` while
waiting; the contract row is the single source of truth for "is this
conv waiting on something." The existing `Awaiting*` family of states
in this codebase exclusively encodes "waiting on the user"
(`AwaitingTaskApproval`, `AwaitingContinuation`); wake is "waiting on
the runtime," which is a different category and should not block
user interaction. `is_busy()` is augmented to return true when the
conv has at least one pending contract (one extra SQLite count per
evaluation; lifecycle endpoints already do at least one read).

The synthetic tool result delivered on fire is byte-shape-identical
to the tool result the equivalent synchronous `op=wait` would have
returned. This makes wake a drop-in replacement for polling from the
LLM's vantage point: same payload, same `tool_use_id` correlation,
just zero intervening turns.

Wake contracts are conversation-scoped, not WorkScope-scoped: a contract is
owned by one conversation and delivers its synthetic result there. The handles
it can watch, however, are all WorkScope-keyed (bash, tmux, browser, subagent).
So when a conversation continues into a successor that inherits the same
WorkScope, every pending contract re-keys its `conv_id` to the successor along
with the handle it watches — no contract fires `Forgotten` at the continuation
boundary.

Mandatory `expires_at` (default 600s, cap 1800s) prevents unbounded
commitments. Restart resync re-registers non-fired contracts; any
contract whose underlying handle did not survive the restart — bash handles,
which are in-memory — immediately fires `Forgotten`, never silently
abandoned.

The LLM-facing surface is a single unified `wait_until { handle: {
kind, id }, condition, max_wait_seconds }` tool with a `#[serde(tag
= "kind")]` enum on the handle discriminator. No per-substrate
variants (`bash_wait_until`, etc.) — keeps tool-description tax low
and forward-aligns with the unified `WorkHandle` trait that will
land separately.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| REQ-WAKE-001 Registration | Proposed | LLM-facing tool surface; no synchronous payload; no conv state mutation |
| REQ-WAKE-002 Persistence | Proposed | `wake_contracts` SQLite table + restart resync |
| REQ-WAKE-003 Router Service | Proposed | Background poll task, 1s tick for HandleTerminal v1 |
| REQ-WAKE-004 `is_busy()` Derivation | Proposed | Reads contract table; no new state machine variant |
| REQ-WAKE-005 V1 Condition Kinds | Proposed | HandleTerminal only; regex/file/port/webhook deferred |
| REQ-WAKE-006 Wake Event Delivery | Proposed | Synthetic tool result; shape-identical to `op=wait` response |
| REQ-WAKE-007 Mandatory Timeout | Proposed | Default 600s, cap 1800s |
| REQ-WAKE-008 User Status + Cancel | Proposed | UI indicator on Idle conv + cancel endpoint |
| REQ-WAKE-009 Conv-Scoped | Proposed | Not WorkScope-scoped; explicit deconfliction |
| REQ-WAKE-010 Independent Contracts | Proposed | No auto-cancel on sibling fire |
| REQ-WAKE-011 Terminal Cause | Proposed | Fired / Expired / Cancelled / Forgotten |
| REQ-WAKE-012 Continuation Inheritance | Proposed | All handle kinds (incl. bash) transfer; no continuation-boundary forget |
| REQ-WAKE-013 User Messages | Proposed | Conv stays Idle; user messages just work |
| REQ-WAKE-014 Tool Description | Proposed | Explicit cost model + when-to-use guidance |
| REQ-WAKE-015 Cost Observability | Proposed | Metrics on registration / fire / forgotten breakdown |
| REQ-WAKE-016 Unified Tool Surface | Proposed | Single `wait_until` tool, tagged-enum handle discriminator |

**Progress:** 0 of 16 implemented. **All open questions resolved**
per `/asking-questions` session 2026-05-24.

## Dependencies

- `specs/bash/` REQ-BASH-002 (wait semantics) — REQ-WAKE-006 reuses
  bash wait response shape for delivery
- `specs/tmux-integration/` — TmuxPane handle condition kind
- `specs/subagents/` — SubAgent handle condition kind
- `specs/bedrock/` REQ-BED-032 (hard-delete cascade) — REQ-WAKE-004
  is_busy() augmentation joins the cascade's busy-check

## Related Work

- **`specs/bash/` REQ-BASH-WS-001 / -WS-002** — bash handles are
  WorkScope-keyed and inherit across a scope-sharing continuation, so
  REQ-WAKE-012 transfers every handle kind (bash included) to the child
- **WorkScope foundation** (PRs #136, #143, #139, merged) —
  established the resource-cleanup cascade infrastructure this spec
  extends with cancel-on-lifecycle (REQ-WAKE-004 join)
- **Persona panel review** (`/persona-panel` session 2026-05-24) —
  4 reviewers (correct-by-construction, LLM-cognitive-load, failure-
  mode, token-economics) converged on "no wake primitive" as the
  single root cause behind 6+ distinct surface complaints

## Why This Spec Exists

Phoenix today treats turn-count as free. Every spawn-and-wait flow
that exceeds `wait_seconds` pays a full LLM round-trip per poll. The
handle-cap (8) and ring-size (4MB) are explicit budgets in
`specs/bash/`; turn-count is not budgeted anywhere. Wake contracts
are the runtime acknowledging that turn-count is a first-class cost
the same way bytes-in-ring are.

The secondary motivation is correctness: today, if the LLM stops
polling, an exit reaches no one. The tombstone sits in memory until
Phoenix restart wipes it. Wake contracts make every spawn-and-wait
deliver a terminal answer in all cases — `Fired`, `Expired`,
`Cancelled`, or `Forgotten` — never silently abandoned.
