# Wake Contracts

## User Story

As an LLM agent, when I have started a long-running command (a build, a test
suite, a dev server, a sub-agent) and have nothing else to do until it
produces a useful event, I need a way to tell Phoenix "wake me when this
specific condition fires" rather than polling the runtime every N seconds.
Today every wait costs a full conversation round-trip — tool descriptions
re-fed, history re-fed, an assistant turn spent saying "still waiting" —
even when I had nothing to contribute in the interim. I need a primitive
that consumes zero turns until the condition the LLM cares about actually
holds.

## Background: from polling to wake

Today three tools spawn potentially-long work:

- **`bash`** — `op=run` returns a handle if `wait_seconds` elapses before
  exit; `op=wait` blocks the same way again on the same handle. To learn
  that a multi-minute command exited, the LLM must repeatedly invoke
  `op=wait`, each invocation consuming one full conversation turn.
- **`tmux_run`** — readiness modes are `return_immediately` and
  `wait_for_text`. No `wait_for_exit`. For finite long-running commands
  the LLM polls `tmux capture-pane`, same round-trip cost as bash.
- **`subagent`** — spawns a sub-conversation; the parent blocks on
  completion inside the state machine. This is the *only* current
  spawn-and-wait surface that does not require LLM polling, and it
  achieves that by hardcoding the wait into the state machine rather
  than exposing a reusable primitive.

The subagent path proves the runtime *can* resume a conversation on an
external signal. Wake contracts generalize that capability: any handle
that the agent has spawned can register a wake condition, and the
runtime fires the conversation back when the condition holds — without
the LLM having to poll.

**Persistence boundary:** wake contracts are persisted to SQLite. They
survive Phoenix restart. A contract registered against a handle that
itself does not survive restart (e.g., bash handles per
`specs/bash/`) MUST fire a terminal `forgotten` event on restart so the
LLM is not left waiting on a signal that can no longer occur. The
contract is a *Phoenix-side commitment to fire the conversation* — its
durability does not depend on the underlying handle's durability.

**Scope:** wake contracts are conversation-scoped, not WorkScope-scoped.
A contract belongs to the conversation that registered it; resolution
fires that conversation. Continuation chains inherit contracts only when
the underlying handle is itself inherited (see REQ-WAKE-012; consistent
with [[bash-cascade-skips-inheritor-scope]] resolution).

## Requirements

### REQ-WAKE-001: Wake Contract Registration

WHEN agent calls a wake-registering tool with `{ handle_id, condition,
max_wait_seconds, on_fire_metadata? }`
THE SYSTEM SHALL persist a new wake contract row, transition the
conversation to `AwaitingWake { contract_ids }`, and return control to
the dispatcher without invoking the LLM

WHEN the contract is persisted
THE SYSTEM SHALL emit an SSE `WakeContractRegistered` event with the
contract id, condition summary, and `expires_at`

**Rationale:** Registration is the LLM's explicit commitment "I have
nothing further to do until this fires." It is structurally a
*conversation state transition*, not a tool result — the tool call
itself does not return a payload that the LLM consumes synchronously.
The LLM's next invocation is when the contract fires (or expires).

---

### REQ-WAKE-002: Contract Persistence

THE SYSTEM SHALL persist every active wake contract in a `wake_contracts`
SQLite table with columns `(id, conv_id, handle_kind, handle_id,
condition_json, expires_at, registered_at, fire_template_json)`

WHEN Phoenix restarts
THE SYSTEM SHALL on startup re-register every non-fired, non-expired
contract with the wake-router, and immediately fire `forgotten` for
every contract whose underlying handle did not survive restart (see
REQ-WAKE-005 for handle-kind durability)

**Rationale:** The contract is the durable thing. The underlying handle
may or may not be durable; the contract's persistence is the runtime's
commitment to deliver a terminal answer in all cases. Silent loss on
restart is the failure mode this whole spec exists to eliminate.

---

### REQ-WAKE-003: Wake Router Service

THE SYSTEM SHALL run a background `wake_router` task that, on each tick
(target cadence: 1s for handle-terminal conditions; condition-kind-
specific for future kinds), evaluates every registered contract's
condition against the current state of the underlying handle

WHEN a condition evaluates to `Fired`
THE SYSTEM SHALL atomically:
  1. mark the contract row as fired with cause and observed payload,
  2. construct a synthetic tool result per REQ-WAKE-006,
  3. inject the result into the conversation,
  4. transition the conversation out of `AwaitingWake`, and
  5. emit `WakeContractFired` SSE

WHEN a contract's `expires_at` passes without firing
THE SYSTEM SHALL fire the contract with cause `Expired` and the same
delivery path as a normal fire

**Rationale:** The router is the single writer of contract terminal
state. Per-condition-kind evaluators are pure functions over (handle
state, condition); the router orchestrates polling and fan-out. This
mirrors the existing bash `reaper.rs` and tmux registry patterns.

---

### REQ-WAKE-004: AwaitingWake Conversation State

THE conversation state machine SHALL include a state
`AwaitingWake { contract_ids: NonEmptyVec<ContractId> }`

WHEN the conversation is in `AwaitingWake` AND a contract fires
THE SYSTEM SHALL transition the conversation out of `AwaitingWake`
(target state: same as a normal tool-result delivery — typically
`LlmRequesting`)

WHEN the conversation is in `AwaitingWake` AND the user sends a new
message
THE SYSTEM SHALL [OPEN QUESTION — see design.md §"User-interrupt
semantics"]

WHEN the conversation is in `AwaitingWake` AND the user archives /
hard-deletes / abandons the conversation
THE SYSTEM SHALL cancel every registered contract (cause: `Cancelled`)
before the lifecycle cascade runs

**Rationale:** Making this a first-class state (not a flag) is the
correct-by-construction lever. `is_busy()` derivation, SSE event
fan-out, UI representation, and concurrency rules all key off this
state existing. Today's "implicit waiting" via stuck `op=wait` is
invisible to every observer.

---

### REQ-WAKE-005: V1 Condition Kinds

THE SYSTEM SHALL support the following condition kinds in v1:

- `HandleTerminal { handle_kind: Bash | TmuxPane | SubAgent, handle_id }`
  — fires when the named handle reaches a terminal state (exited,
  killed, signaled, kill_pending_kernel for bash; pane-process-exit for
  tmux; child-conversation terminal for subagent)

THE SYSTEM SHALL NOT support in v1:
- `RegexInTmuxPane { pane, pattern }` — deferred
- `FileChanged { path }` — deferred
- `PortListening { host, port }` — deferred
- `WebhookFired { id }` — deferred (security surface)

**Rationale:** HandleTerminal covers the highest-leverage case (build
finished, test suite done, subagent submitted) and validates the
abstraction with the simplest possible evaluator (handle status read).
Other condition kinds are the same edge with different evaluators; the
v1 contract row schema must accommodate them as a forward-compat
discriminator.

---

### REQ-WAKE-006: Wake Event Delivery

WHEN a contract fires
THE SYSTEM SHALL deliver to the conversation a synthetic tool result
shaped as the tool result the equivalent successful `op=wait` would
have returned — i.e., for `HandleTerminal/Bash` the tool result MUST
carry the handle's terminal status (`exited` / `killed` / `signaled` /
`kill_pending_kernel` / `forgotten`), `exit_code`, `duration_ms`, and a
final tail window per REQ-BASH-004

THE delivered tool result SHALL be addressable back to the original
tool call that registered the contract (via tool_use_id)

**Rationale:** Pit-of-success: the LLM sees the same shape whether it
synchronously waited or registered a wake contract. The decision
between the two is "should I block this turn or not"; the response
shape is the same. This makes the wake primitive a drop-in replacement
for `op=wait`, not a parallel pathway.

---

### REQ-WAKE-007: Mandatory Timeout Cap

EVERY wake contract SHALL have an `expires_at` populated at registration
time

THE SYSTEM SHALL reject registration where `max_wait_seconds` exceeds
`WAKE_MAX_SECONDS` (v1: 1800s = 30 minutes)

THE SYSTEM SHALL apply a default `max_wait_seconds` of
`WAKE_DEFAULT_SECONDS` (v1: 600s = 10 minutes) when the caller omits it

**Rationale:** A persisted commitment to fire the LLM is a persisted
commitment to spend money. Unbounded waits are not expressible.
The cap is enforced at registration so the contract row's `expires_at`
is always a true upper bound on the conversation's wake latency.

---

### REQ-WAKE-008: User-Visible Cancel

THE conversation UI SHALL render the active `AwaitingWake` state with:
- the contract condition summary (e.g., "waiting for handle b-3 to exit")
- the `expires_at` timestamp
- a cancel button that POSTs to `/api/conversations/:id/wake/:contract_id/cancel`

WHEN the cancel endpoint is invoked
THE SYSTEM SHALL fire the contract with cause `Cancelled` via the same
delivery path as REQ-WAKE-003

**Rationale:** Pit-of-success at the user surface: the user must never
be confused about "why isn't anything happening?" An `AwaitingWake`
conversation looks identical to an `Idle` one in today's UI; that is
not acceptable for a state that may persist for tens of minutes.

---

### REQ-WAKE-009: Conversation-Scoped, Not WorkScope-Scoped

A wake contract SHALL be owned by the conversation that registered it

THE wake-router SHALL fire only the registering conversation, even when
the underlying handle is WorkScope-keyed and shared across continuation

**Rationale:** WorkScope governs *resource* sharing across continuation
boundaries; wake governs *conversation* resumption. Conflating them was
explicitly identified as the design error that motivated the
WorkScope+wake split (see executive.md §"Why two axes").

---

### REQ-WAKE-010: Multiple-Contracts-Per-Conversation Resolution

A conversation in `AwaitingWake { contract_ids }` MAY hold multiple
contract ids (e.g., agent registered two waits in parallel via a single
tool call returning multiple contract registrations)

WHEN the first contract fires
THE SYSTEM SHALL deliver only that contract's payload, transition out
of `AwaitingWake`, and cancel every other contract in the set with
cause `Superseded`

**Rationale:** "First fire wins" is the simplest correct semantics for
v1. A `wait_for_all` variant is a future contract kind, not a v1
concern. The cancel-on-supersede ensures no orphan timers, no orphan
pollers, no double-fire into a conversation that has already moved on.

---

### REQ-WAKE-011: Terminal-Cause Distinction

THE wake event payload SHALL distinguish:
- `Fired { observed_payload }` — condition held
- `Expired` — timeout reached, condition never held
- `Cancelled` — user cancelled, or supersede, or lifecycle cascade
- `Forgotten { reason }` — underlying handle was destroyed by an
  external action (cascade, restart-without-durable-handle) before the
  condition could be evaluated

**Rationale:** "The wait returned" is not a sufficient description.
Forgotten is structurally different from expired — the contract was
never going to be able to fire, regardless of how long the LLM waited.
The LLM's response to `Forgotten` should be "re-spawn or escalate to
the user," which is different from `Expired`'s "this took too long."

---

### REQ-WAKE-012: Continuation Chain Inheritance

WHEN a conversation has registered wake contracts AND continues into
a child conversation
THE SYSTEM SHALL [OPEN QUESTION — see design.md §"Continuation
inheritance" — depends on resolution of
[[bash-cascade-skips-inheritor-scope]]]

**Rationale:** Cannot resolve before the bash/handle inheritance
question is resolved. Two coherent positions exist; spec stays
explicit-pending until that is decided.

---

### REQ-WAKE-013: User-Interrupt Semantics

WHEN the conversation is in `AwaitingWake` AND the user sends a new
message
THE SYSTEM SHALL [OPEN QUESTION — see design.md §"User-interrupt
semantics"]

Candidate behaviors (decision needed):
A. User message is queued; current contracts continue; on fire, the
   user message is processed alongside the wake result.
B. User message cancels all contracts (cause: `Cancelled`); user
   message becomes the next turn.
C. User message is rejected with "conversation is waiting; cancel
   the wake or wait for it to fire."

---

### REQ-WAKE-014: Tool Description Guidance

THE description of every wake-registering tool SHALL state explicitly:
- when to register a wake vs use synchronous `op=wait`
- the cost model: synchronous wait blocks one turn AT LEAST, wake
  contracts consume zero turns until fire
- the cancel mechanism

**Rationale:** Pit-of-success applies to the tool surface. Without
explicit guidance the LLM will default to synchronous polling because
that is the shape it learned from training data. The description is
how that prior is corrected.

---

### REQ-WAKE-015: Cost Observability

THE SYSTEM SHALL emit metrics on:
- wake contract registration rate (per conv, per condition kind)
- average fire latency (registration to fire)
- expired-vs-fired ratio
- forgotten-vs-fired ratio

**Rationale:** A wake contract is a Phoenix-side commitment to spend
money. Operators must be able to see "how much wake is happening" and
"how often is wake the right primitive vs the wrong one." Metrics also
gate v2 condition kinds — we will not ship `RegexInTmuxPane` until v1
metrics show the abstraction is being used as intended.

---

## Status

| Requirement | Status | Notes |
|-------------|--------|-------|
| REQ-WAKE-001 | Proposed | Registration API + state transition |
| REQ-WAKE-002 | Proposed | SQLite persistence + restart resync |
| REQ-WAKE-003 | Proposed | wake_router background service |
| REQ-WAKE-004 | Proposed | AwaitingWake conv state |
| REQ-WAKE-005 | Proposed | HandleTerminal only in v1 |
| REQ-WAKE-006 | Proposed | Synthetic tool result delivery |
| REQ-WAKE-007 | Proposed | Mandatory expires_at cap |
| REQ-WAKE-008 | Proposed | UI cancel surface |
| REQ-WAKE-009 | Proposed | Conv-scoped (not WorkScope) |
| REQ-WAKE-010 | Proposed | First-fire-wins multi-contract |
| REQ-WAKE-011 | Proposed | Terminal cause discriminator |
| REQ-WAKE-012 | Open question | Depends on [[bash-cascade-skips-inheritor-scope]] |
| REQ-WAKE-013 | Open question | User-interrupt policy |
| REQ-WAKE-014 | Proposed | Tool description discipline |
| REQ-WAKE-015 | Proposed | Cost observability metrics |

**Progress:** 0 of 15 implemented. 2 open questions must be resolved
before implementation begins per AGENTS.md spec discipline (open
questions become deliberate design decisions, not prose-as-future-work).

## Dependencies

- `specs/bash/` REQ-BASH-002 (wait semantics) — REQ-WAKE-006 delivery
  format reuses BASH wait response shape
- `specs/tmux-integration/` — TmuxPane condition kind
- `specs/subagents/` — SubAgent condition kind
- `specs/bedrock/` REQ-BED-032 (hard-delete cascade) — REQ-WAKE-004
  cancel-on-lifecycle path joins the cascade

## Related Tasks

- [[bash-cascade-skips-inheritor-scope]] (62009) — blocking for
  REQ-WAKE-012
