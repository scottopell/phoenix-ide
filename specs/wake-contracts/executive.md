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

Wake contracts replace the poll-loop with persistence + an asynchronous router:
the LLM registers a contract, the conversation stays in `Idle`, and the runtime
delivers exactly one synthetic terminal result into the conversation when the
condition fires, expires, is cancelled, or becomes forgotten. The LLM consumes
zero turns in between. Persistence makes the wait intent durable; it does not make
every watched handle durable.

V1 supports one condition kind over three concrete handle kinds:
`HandleTerminal { handle_kind, handle_id }` — fires when a named bash, tmux
`window_id`, or sub-agent handle reaches a terminal state. For sub-agents, the
terminal payload covers every durable child terminal cause admitted by bedrock:
explicit `submit_result`, explicit `submit_error`, timeout, child cancellation,
turn-limit hard-stop fallback, implicit completion, runtime failure, and context
exhaustion. Missing child handles resolve as `Forgotten`, not as fired child
payloads. V1 does not define parent-to-child
continuation, `NeedMoreBudget`, arbitrary child
questions, automatic sub-agent budget extension, or a general conversation-actor
framework.

## Technical Summary

A new `wake_contracts` SQLite table persists every contract with registration
fields plus terminal accounting fields: `status`, `terminal_cause`,
`forgotten_reason`, `terminal_payload`, and `resolved_at`. Terminal cause and the
finite forgotten-reason discriminator are queryable columns for metrics;
`terminal_payload` stores only the cause-specific body.
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

The synthetic tool result delivered on fire is byte-shape-identical to the tool
result the equivalent synchronous wait would have returned when such a
synchronous surface exists. Bash delivery mirrors `bash op=wait`; tmux delivery
includes the watched `window_id` terminal status and final captured tail; sub-agent
delivery uses the structured terminal outcome described above. This makes wake a
drop-in replacement for polling from the LLM's vantage point: same payload, same
`tool_use_id` correlation, just zero intervening turns.

Wake contracts are conversation-scoped, not WorkScope-scoped: a contract is
owned by one conversation and delivers its synthetic result there. The handles
it can watch fall into two keying classes. Bash and tmux handles are
WorkScope-keyed: when a conversation continues into a successor that inherits
the same WorkScope, the handle transfers to the successor and a pending contract
on it re-keys its `conv_id` along with it. A subagent handle is keyed by the
sub-agent's own agent / child-conversation id (not by WorkScope), so a
subagent-keyed contract is not transferred by WorkScope inheritance. A contract
fires `Forgotten` only when its watched handle is genuinely destroyed (a Phoenix
restart, or a hard-delete with no inheriting scope), not as a routine
consequence of continuation.

Mandatory `expires_at` (default 600s, cap 1800s) is the delivery deadline for the
wait obligation and prevents unbounded commitments. While Phoenix is running, the
router resolves a pending contract no later than the first tick at or after that
timestamp. After downtime, restart resync delivers in-deadline durable terminal
evidence, emits `Forgotten` for handles that became unknowable, expires only
evaluable contracts with no such evidence, or re-registers still-pending durable
handles.

The LLM-facing surface is a single unified `wait_until { handle: {
kind, id }, condition, max_wait_seconds }` tool with a `#[serde(tag
= "kind")]` enum on the handle discriminator. No per-substrate
variants (`bash_wait_until`, etc.) — keeps tool-description tax low
and forward-aligns with the unified `WorkHandle` trait that will
land separately.

## Current Reality

The durable workflow substrate provides typed wake profiles, persisted bindings,
background observation, terminal projection, replay-safe materialization and
runtime acceptance, restart recovery, and continuation transfer. Phoenix exposes
the unified `wait_until` tool for `HandleTerminal` on tagged Bash handles. A
successful explicit registration checkpoints the provider-valid tool round, parks
the conversation in `Idle`, and resumes the LLM exactly once after durable terminal
delivery. Registration errors continue as ordinary tool errors. Existing
`bash op=wait` behavior is unchanged, and ordinary Bash or tmux handle creation
does not register a wake obligation.

The wake worker starts after a one-time retirement of bindings created by the
removed automatic-registration behavior. Later restarts preserve explicit
registrations. Because Bash processes and handle registries are in memory, a
pending Bash wait whose handle is absent after restart resolves exactly once as
`Forgotten { phoenix_restart }`; Phoenix does not attempt process reattachment.
The same tagged tool does not yet register tmux or sub-agent handles. Those kinds
remain explicit runtime errors until their end-to-end lifecycle is implemented.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| REQ-WAKE-001 Registration | Complete | `wait_until` provides explicit durable Bash registration, typed park disposition, and immediate receipt; unsupported handle kinds remain outside the Bash slice |
| REQ-WAKE-002 Persistence | Partial | Durable Bash bindings, receipts, exactly-once restart reconciliation, and terminal delivery are implemented |
| REQ-WAKE-003 Router Service | Partial | Background observation and replay-safe delivery are active for explicit Bash waits; no ordinary tool implicitly enrolls a handle |
| REQ-WAKE-004 `is_busy()` Derivation | Proposed | Reads contract table; no new state machine variant |
| REQ-WAKE-005 V1 Condition Kinds | Partial | `HandleTerminal` is agent-accessible for Bash; tmux and sub-agent registration remain unsupported |
| REQ-WAKE-006 Wake Event Delivery | Partial | Bash Fired / Expired / Forgotten results use durable materialization and exactly-once auto-resume |
| REQ-WAKE-007 Mandatory Timeout | Partial | Bash registration defaults to 600s and is capped at 1800s |
| REQ-WAKE-008 User Status + Cancel | Partial | Conversation UI, HTTP status/cancel routes, and `phoenix-client.py --wake-status/--wake-cancel` are implemented; CLI status output still lacks the required handle and terminal-status detail |
| REQ-WAKE-009 Conv-Scoped | Proposed | Not WorkScope-scoped; explicit deconfliction |
| REQ-WAKE-010 Independent Contracts | Proposed | No auto-cancel on sibling fire |
| REQ-WAKE-011 Terminal Cause | Partial | Typed Fired / Expired / Cancelled / Forgotten projections exist in the durable substrate |
| REQ-WAKE-012 Continuation Inheritance | Partial | WorkScope transfer and reconciliation exist in the durable substrate |
| REQ-WAKE-013 User Messages | Partial | Explicit Bash waits park in ordinary Idle without introducing an AwaitingWake state |
| REQ-WAKE-014 Tool Description | Partial | Unified tool documents registration, parking, Bash-only support, and timeout bounds |
| REQ-WAKE-015 Cost Observability | Proposed | Metrics on registration / fire / forgotten breakdown |
| REQ-WAKE-016 Unified Tool Surface | Partial | Single `wait_until` tool with tagged handles is implemented; only Bash registration is enabled |
| REQ-WAKE-017 Sub-Agent Terminal Payload | Proposed | Tagged exhaustive sub-agent terminal causes; missing child is Forgotten |
| REQ-WAKE-018 Handle Identity + Lifecycle | Proposed | Bash/tmux WorkScope-keyed; sub-agent keyed by child conversation / agent id |

Requirements remain Partial because the end-to-end surface currently covers only
Bash handles. Tmux and sub-agent registration, user status/cancellation, and the
remaining observability surfaces are not implemented.

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
  REQ-WAKE-012 transfers a bash-keyed contract to the child along with
  its handle
- **`specs/subagents/`** — a sub-agent is tracked by its own agent /
  child-conversation id, so a subagent-keyed contract is keyed
  independently of the parent's WorkScope (REQ-WAKE-012)
- **WorkScope foundation** — the resource-cleanup cascade
  infrastructure this spec extends with cancel-on-lifecycle
  (REQ-WAKE-004 join)

## Why This Spec Exists

Without an explicit wake tool, every spawn-and-wait flow that exceeds
`wait_seconds` pays a full LLM round-trip per poll. The
handle-cap (8) and ring-size (4MB) are explicit budgets in
`specs/bash/`; turn-count is not budgeted anywhere. Wake contracts
are the runtime acknowledging that turn-count is a first-class cost
the same way bytes-in-ring are.

The secondary motivation is correctness: if the LLM stops polling,
an exit reaches no one. The tombstone sits in memory until Phoenix
restart wipes it. Wake contracts make every spawn-and-wait
deliver a terminal answer in all cases — `Fired`, `Expired`,
`Cancelled`, or `Forgotten` — never silently abandoned.
