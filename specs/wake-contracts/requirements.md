# Wake Contracts

## User Story

As an LLM agent, when I have started a long-running command, a tmux-backed
process, or a sub-agent and have nothing else to do until that handle reaches a
terminal state, I need a way to tell Phoenix "wake me when this handle is done"
rather than polling the runtime every N seconds. Today every process wait costs a
full conversation round-trip — tool descriptions re-fed, history re-fed, an
assistant turn spent saying "still waiting" — even when I had nothing to
contribute in the interim. Sub-agent fan-in avoids polling only by hardcoding a
blocking parent state. I need a primitive that consumes zero turns until the
specific terminal condition fires, while the parent conversation remains Idle and
user-interruptible.

## V1 scope

Wake contracts v1 is intentionally narrow. It supports terminal waits on concrete
Phoenix handles only:

- **bash handles** returned by `bash op=run` when `wait_seconds` elapses;
- **tmux window handles** returned by `tmux_run` as `window_id` / referenced
  through the tmux registry;
- **sub-agent handles** identified by the child conversation / agent id.

V1 does not define a general conversation-actor system, request/reply messaging,
parent-to-child continuation, arbitrary clarification questions, or automatic
sub-agent budget extension. Future actor-style messaging can reuse the same
persisted wake-router infrastructure only if a later concrete use case justifies
it.

## Background: from polling to wake

Today three tools spawn potentially-long work:

- **`bash`** — `op=run` returns a handle if `wait_seconds` elapses before
  exit; `op=wait` blocks the same way again on the same handle. To learn
  that a multi-minute command exited, the LLM must repeatedly invoke
  `op=wait`, each invocation consuming one full conversation turn.
- **`tmux_run`** — readiness modes are `return_immediately` and
  `wait_for_text`. No `wait_for_exit`. For finite long-running commands
  the LLM polls `tmux capture-pane`, same round-trip cost as bash.
- **`subagent`** — spawns a sub-conversation; the parent can wait on
  completion through state-machine fan-in. That path avoids LLM polling by
  hardcoding the wait into the state machine rather than exposing a reusable
  terminal-handle primitive.

The subagent path proves the runtime *can* resume a conversation on an
external signal. Wake contracts generalize only that terminal-wait capability in
v1: a conversation registers a wait on a bash, tmux, or sub-agent handle, returns
to Idle, and receives a synthetic tool result when the handle reaches a terminal,
expired, cancelled, or forgotten outcome.

**No new conversation state.** Wake contracts do *not* introduce an
`AwaitingWake` conv state. The conversation stays in `Idle` (or
whatever state it would otherwise be in) while waiting. The
`Awaiting*` family of states in this codebase exclusively represents
"waiting on the user" (`AwaitingTaskApproval`, `AwaitingContinuation`).
Wake is "waiting on the runtime," which is a different category —
it does not block the user from interacting with the conversation,
and it is invisible to the user except as a status indicator. The
contract row in the `wake_contracts` table is the single source of
truth for "is this conv waiting on something." ADR-006 records the rejected
`AwaitingWake` alternative.

**Persistence boundary:** wake contracts are persisted to SQLite. They survive
Phoenix restart. The contract makes the wait intent durable, not the watched
handle. A contract registered against a handle that itself does not survive
restart (e.g., bash handles per `specs/bash/`, or active sub-agent runtimes per
`specs/subagents/`) MUST deliver a terminal `forgotten` event during startup
reconciliation so the LLM is not left waiting on a signal that can no longer
occur. The contract is a *Phoenix-side commitment to deliver exactly one terminal
outcome* — its durability does not depend on the underlying handle's durability.

**Scope:** wake contracts are conversation-scoped, not WorkScope-scoped.
A contract belongs to the conversation that registered it; resolution
delivers a synthetic tool result into that conversation. The handles a
contract can watch fall into two keying classes. Bash and tmux handles
are WorkScope-keyed: when a conversation continues into a successor that
inherits the same WorkScope, the underlying handle transfers to the
successor and a pending contract on it re-keys its `conv_id` to the child,
so subsequent fires deliver there. A subagent handle is keyed by the
sub-agent's own agent / child-conversation id (per `specs/subagents/`),
independent of the parent's WorkScope; a subagent-keyed contract is not
transferred by WorkScope inheritance. A contract fires `forgotten` only
when its watched handle is genuinely destroyed, not at a routine
continuation boundary.

## Requirements

### REQ-WAKE-001: Wake Contract Registration

WHEN agent calls `wait_until { handle: { kind, id }, condition,
max_wait_seconds }`
THE SYSTEM SHALL persist a new wake contract row and return control to
the dispatcher without invoking the LLM and without modifying the
conversation state

WHEN the contract is persisted
THE SYSTEM SHALL emit an SSE `WakeContractRegistered` event with the
contract id, handle reference, condition summary, and `expires_at`

**Rationale:** Registration is the LLM's explicit commitment "I have
nothing further to do until this fires." The tool call itself does
not return a synchronous payload to the LLM. The conversation state
is unchanged — the LLM's next invocation is when the contract fires
(or expires / is cancelled / is forgotten). See REQ-WAKE-006 for
delivery semantics; ADR-006 records the explicit non-state rationale.

---

### REQ-WAKE-002: Contract Persistence

THE SYSTEM SHALL persist every wake contract in a `wake_contracts` SQLite table
with columns `(id, conv_id, handle_kind, handle_id, condition_json, expires_at,
registered_at, fire_template_json, registering_tool_use_id, status,
terminal_cause, forgotten_reason, terminal_payload, resolved_at)`

THE `terminal_cause` and `forgotten_reason` columns SHALL be the queryable
terminal discriminators used for metrics and operator views. `forgotten_reason`
SHALL be populated whenever `terminal_cause = Forgotten` and SHALL be drawn from a
finite set: `phoenix_restart`, `cascade_destroyed_handle`,
`subagent_handle_missing`, or `tmux_handle_missing`. The `terminal_payload` column
SHALL hold only the cause-specific body and SHALL NOT repeat those discriminator
values or the watched `handle_kind`; replay derives the fired payload variant
from the contract row.

THE SYSTEM SHALL persist captured bash and tmux wake tails in child tables keyed
by `(contract_id, ordinal)` rather than inside `terminal_payload`. A missing tail
row set means no captured tail output; ordering is the `ordinal` column.

THE SYSTEM SHALL update `status`, `terminal_cause`, `forgotten_reason`,
`terminal_payload`, and `resolved_at` atomically when a contract resolves

WHEN Phoenix restarts
THE SYSTEM SHALL reconcile every non-terminal contract before normal serving:
durable terminal evidence recorded before `expires_at` delivers a fired payload;
contracts whose handles cannot still be evaluated resolve as `forgotten`; overdue
evaluable contracts with no in-deadline terminal evidence resolve through the
expiry path; and still-pending durable handles re-register with the wake-router

**Rationale:** The contract is the durable thing AND the single source of truth
for "is this conv waiting on something." The underlying handle may or may not be
durable; the contract's persistence is the runtime's commitment to deliver a
terminal answer in all cases. Silent loss on restart is the failure mode this
spec exists to eliminate. A restart may make a bash result unknowable, but it must
not erase the parent conversation's recovery path.

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
  3. append the tool result to the conversation message log,
  4. trigger the conversation's next LLM turn (same path as a normal
     user message arrival or tool result completion), and
  5. emit `WakeContractFired` SSE

WHEN a contract's `expires_at` passes without firing
THE SYSTEM SHALL fire the contract with cause `Expired` and the same
delivery path as a normal fire

**Rationale:** The router is the single writer of contract terminal
state. Per-condition-kind evaluators are pure functions over (handle
state, condition); the router orchestrates polling and fan-out. This
mirrors the existing bash `reaper.rs` and tmux registry patterns.

---

### REQ-WAKE-004: Pending-Wake Lifecycle Guard

THE conversation `is_busy()` derivation SHALL remain unchanged by pending wake
contracts alone

WHEN a conversation would otherwise remain idle AND
`has_pending_wake_contracts(conv)` is true
THE SYSTEM SHALL keep runtime busy/idle semantics unchanged and SHALL instead
expose the pending wake through wake-specific presentation detail, capability
guards, and lifecycle conflict checks driven directly by the pending-wake
lifecycle

Archive, hard-delete, abandon, mark-merged, and any equivalent destructive
lifecycle operation SHALL query pending wake contracts directly and SHALL reject
or serialize the lifecycle transition until those contracts are resolved

**Rationale:** Wake waits are runtime-owned obligations, not proof that the LLM
is actively executing. Fabricating `is_busy()` would create a duplicate semantic
state alongside the durable wake rows and would misrepresent an otherwise idle
conversation. Lifecycle safety still matters, so the pit-of-success guard moves
to an explicit pending-wake conflict check rather than overloading runtime busy.

---

### REQ-WAKE-005: V1 Condition Kinds

THE SYSTEM SHALL support the following condition kind in v1:

- `HandleTerminal { handle_kind: Bash | TmuxPane | SubAgent, handle_id }`
  — fires when the named handle reaches a terminal state.

For `Bash`, fired payloads SHALL include the terminal statuses reported by the
synchronous wait surface for an observed handle, including exited, killed (with
signal metadata when relevant), and `kill_pending_kernel`. A bash handle that is
lost across restart or teardown resolves through the wake contract's `Forgotten`
cause, not through a fired bash payload.

For `TmuxPane`, fired payloads SHALL include observing the Phoenix tmux-run exit
marker for the watched `window_id`, or observing that the watched window was
explicitly killed after the registry recorded that terminal state. The evaluator
SHALL NOT require the tmux window process to exit, because `tmux_run` keeps the
window inspectable after command exit by default. If the tmux server/session or
registered window handle is absent without a recorded killed/exit-marker terminal
state, the contract SHALL resolve as `Forgotten`, not as a fired tmux payload.

For `SubAgent`, terminal fired payloads SHALL include every durable child
terminal cause admitted by bedrock: explicit `submit_result`, explicit
`submit_error`, wall-clock timeout, child cancellation observed independently of
parent wake cancellation, turn-limit hard-stop fallback, implicit text completion,
non-retryable runtime failure, and
context exhaustion. A missing child handle is not a fired payload; it resolves the
contract as `Forgotten` per REQ-WAKE-011 and REQ-WAKE-017.

THE SYSTEM SHALL NOT support in v1:
- arbitrary conversation-actor messages or request/reply continuation
- parent-to-child continuation (`continue_subagent`) or `NeedMoreBudget`
- automatic sub-agent budget extension
- child clarification questions delivered to the parent
- `RegexInTmuxPane { pane, pattern }` — deferred
- `FileChanged { path }` — deferred
- `PortListening { host, port }` — deferred
- `WebhookFired { id }` — deferred (security surface)
- deadline-only waits with no owned handle, such as usage-limit reset sweeps —
  deferred to their owning scheduler instead of `wait_until`

**Rationale:** HandleTerminal covers the highest-leverage cases: build/test
processes finishing and delegated sub-agents reaching their terminal result. It
validates the persistence, delivery, and wake-router edges without committing
Phoenix to a general actor framework. Other condition kinds are separate
evaluators; actor-style messaging is a separate product contract. Deadline-only
wakes are also separate: they have no handle ownership, no substrate terminal
payload, and no `Forgotten` semantics, so modeling them as `wait_until` contracts
would create a parallel scheduler inside the handle-wake plane.

---

### REQ-WAKE-006: Wake Event Delivery

WHEN a contract fires
THE SYSTEM SHALL deliver to the conversation a synthetic tool result
shaped as the tool result the equivalent successful synchronous wait would have
returned.

For `HandleTerminal/Bash`, the tool result MUST carry the handle's terminal
status (`exited` / `killed` / `kill_pending_kernel` / `forgotten` and any other
status exposed by `bash op=wait`), `exit_code`, `duration_ms`, and a final tail
window per REQ-BASH-004. The final tail is reconstructed from normalized
wake-tail child rows, not from `terminal_payload`.

For `HandleTerminal/TmuxPane`, the delivered tool result MUST identify the
watched `window_id` from the contract's `handle_id` and carry terminal status,
exit information when available, and a final captured tail window equivalent to
the information the LLM would gather by inspecting the window after exit. The
captured tail MUST be represented as a list in the delivered result; no output is
an empty list, not an absent field. Persisted captured tail lines live in
normalized wake-tail child rows. The persisted terminal payload body MUST NOT
repeat `window_id`; replay derives the delivered identity from the contract row.

For `HandleTerminal/SubAgent`, the delivered tool result MUST identify the child
conversation / agent id from the contract's `handle_id` and carry the spawned
task text, an optional label, and the structured sub-agent terminal payload
defined by REQ-WAKE-017. The persisted terminal payload body MUST NOT repeat the
handle identity and MUST NOT persist separate agent-id and conversation-id fields
for the same handle identity.

THE delivered tool result SHALL be addressable back to the original
tool call that registered the contract (via `tool_use_id`)

**Rationale:** Pit-of-success: the LLM sees the same shape whether it
synchronously waited or registered a wake contract. The decision between the two
is "should I block this turn or not"; the response shape is the same whenever a
synchronous analogue exists. Sub-agent waits have no existing synchronous
`op=wait` tool, so REQ-WAKE-017 defines the equivalent terminal payload.

---

### REQ-WAKE-007: Mandatory Timeout Cap

EVERY wake contract SHALL have an `expires_at` populated at
registration time

THE SYSTEM SHALL reject registration where `max_wait_seconds` exceeds
`WAKE_MAX_SECONDS` (v1: 1800s = 30 minutes)

THE SYSTEM SHALL apply a default `max_wait_seconds` of
`WAKE_DEFAULT_SECONDS` (v1: 600s = 10 minutes) when the caller omits
it

**Rationale:** A persisted commitment to fire the LLM is a persisted commitment
to spend money. Unbounded waits are not expressible. The cap is enforced at
registration so the contract row's `expires_at` is always the delivery deadline:
while Phoenix is running, the router resolves the contract no later than the
first tick at or after that timestamp; after downtime, startup reconciliation
resolves contracts before normal serving resumes, delivering in-deadline durable
terminal evidence, forgetting handles that became unknowable, and expiring only
evaluable contracts with no such evidence. The running wake router uses the same
precedence: durable terminal evidence recorded before `expires_at` fires even when
the next router tick happens after `expires_at`. The deadline bounds the wait
obligation, not the lifetime of the underlying process or child agent.

---

### REQ-WAKE-008: User-Visible Status and Cancel

THE conversation UI SHALL render a status indicator on conversations
with at least one pending wake contract, showing:
- the count of pending contracts (or a per-contract list if N <= 3)
- the soonest `expires_at` timestamp
- a cancel affordance per contract (button or chip dropdown)

THE `phoenix-client.py` CLI SHALL expose a wake-status command that reports the
same pending contract count, soonest `expires_at`, per-contract ids, handle kinds,
and terminal status for non-browser inspection

WHEN the cancel endpoint
(`POST /api/conversations/:id/wake/:contract_id/cancel`) is invoked
THE SYSTEM SHALL fire the contract with cause `Cancelled` via the
same delivery path as REQ-WAKE-003

**Rationale:** The user must not be confused about "why isn't anything
happening?" The conv stays in `Idle` while waiting, so the indicator
is what distinguishes a true-idle conv from a wake-pending conv. The
indicator is also how the user discovers that
`POST /api/conversations/:id/messages` will succeed during a wait —
the wait is non-blocking from the user's standpoint.

---

### REQ-WAKE-009: Conversation-Scoped, Not WorkScope-Scoped

A wake contract SHALL be owned by the conversation id stored on the contract row

THE wake-router SHALL fire only the contract's current conversation id, even when
the underlying handle is WorkScope-keyed and shared across continuation. When
REQ-WAKE-012 transfers a contract to an inheriting conversation, that successor is
the only wake delivery target.

**Rationale:** WorkScope governs *resource* sharing across continuation
boundaries; wake governs *conversation* resumption. Conflating them
is the invalid-state class that ADR-006's WorkScope/wake split avoids.

---

### REQ-WAKE-010: Multiple-Contracts-Per-Conversation Resolution

A conversation MAY hold multiple pending contracts (e.g., agent
registered two waits in parallel via two `wait_until` calls in one
turn)

WHEN the first contract fires
THE SYSTEM SHALL deliver only that contract's payload as the next
synthetic tool result, and the other pending contracts SHALL continue
to be evaluated normally (no auto-cancellation of siblings)

**Rationale:** Independent contracts represent independent things the
LLM cares about. There is no reason to cancel a still-relevant
`cargo build` watch just because an unrelated `subagent` finished
first. The LLM consumes the first fire, makes whatever decisions, and
either lets the other contracts continue to fire on their own
schedule or cancels them explicitly via the cancel endpoint. The
rejected "first-fire-wins-cancel-siblings" semantics only makes sense
in a model where the conv is in a single-contract-bound `AwaitingWake`
state; with contracts as free-standing rows, independent firing is the
honest semantics.

---

### REQ-WAKE-011: Terminal-Cause Distinction

Every accepted wake contract whose current `conv_id` remains queryable SHALL
transition from pending to exactly one terminal outcome and SHALL deliver that
outcome to the contract's current `conv_id`. If REQ-WAKE-012 re-keys a contract
to an inheriting conversation, delivery targets the inheriting conversation, not
the original registering conversation. Hard-delete paths that remove the current
conversation cancel/remove the contract before deleting the row and do not append
a synthetic result into the deleted conversation. The wake event payload SHALL
distinguish:
- `Fired { observed_payload }` — condition held
- `Expired` — delivery deadline reached before the condition held
- `Cancelled` — user cancelled, or lifecycle cascade
- `Forgotten { reason }` — underlying handle became unknowable before the
  condition could be evaluated while the contract's current conversation remains
  queryable: a WorkScope teardown with no inheriting WorkScope, a Phoenix restart
  that dropped an in-memory handle (bash), a missing tmux window/session with no
  recorded terminal state, or a missing sub-agent child

**Rationale:** "The wait returned" is not a sufficient description.
Forgotten is structurally different from expired — the contract was
never going to be able to fire, regardless of how long the LLM
waited. The LLM's response to `Forgotten` should be "re-spawn or
escalate to the user," which is different from `Expired`'s "this
took too long."

---

### REQ-WAKE-012: Continuation Chain Inheritance

WHEN a conversation has a pending wake contract whose watched handle is
WorkScope-keyed (a bash or tmux handle) AND continues into a child
conversation that inherits the same `WorkScope`
THE SYSTEM SHALL re-key that contract's `conv_id` to the child
conversation, so subsequent fires deliver into the child — the
WorkScope-keyed handle transfers to the child along with the contract

WHEN a WorkScope-keyed handle's `WorkScope` is torn down with no
inheriting successor (the handle is therefore destroyed) AND the contract's
current conversation remains queryable
THE SYSTEM SHALL fire the contract with cause `Forgotten`

WHEN the same teardown deletes the contract's current conversation
THE SYSTEM SHALL cancel/remove the contract before deleting the row and SHALL NOT
append a synthetic result into the deleted conversation

A wake contract whose watched handle is a subagent handle is keyed by
the sub-agent's own agent / child-conversation id (per
`specs/subagents/`), not by the parent conversation's `WorkScope`. A
WorkScope-inheriting continuation does not make an already-spawned
sub-agent a WorkScope-owned resource of the successor, so such a
contract is NOT re-keyed by WorkScope inheritance: its completion wake
remains keyed to the sub-agent id that the contract was registered
against.

**Rationale:** Bash and tmux handles are WorkScope-level resources
(bash per `specs/bash/` REQ-BASH-WS-001), so they transfer across a
continuation that inherits the scope and their contracts transfer with
them. A sub-agent, by contrast, is tracked by agent id — re-keying a
sub-agent's completion wake by WorkScope would re-point it at the wrong
conversation. A contract reaches `Forgotten` only when its handle is
genuinely destroyed — a WorkScope torn down with no inheritor, or a
Phoenix restart that drops an in-memory bash handle (REQ-WAKE-002) —
not as a routine consequence of continuation.

---

### REQ-WAKE-013: Concurrent User Message and Wake Fire

WHEN the conversation has a pending wake contract AND the user sends
a new message to the conversation
THE SYSTEM SHALL accept the user message normally (conv stays in
Idle and accepts the message; the contract continues to be evaluated;
both events append to the message log in the order they arrive)

**Rationale:** Because the conv is in `Idle` while waiting
(REQ-WAKE-004 is the only sticky aspect, and it does not block user
input), user messages and wake fires are both just events that
append to the message log. Ordering is by arrival; the next LLM
turn includes whatever has accumulated since the previous turn.
The race "both arrive in the same millisecond" is serialized by the
existing per-conversation lock around message-log appends.

---

### REQ-WAKE-014: Tool Description Guidance

THE description of the `wait_until` tool SHALL state explicitly:
- when to register a wake vs use synchronous `op=wait` (rule of
  thumb: if you have anything else useful to do, do that; if you have
  nothing to do until the handle resolves, register a wake)
- the cost model: synchronous wait blocks at minimum one turn AT
  LEAST; wake contracts consume zero turns until fire
- the cancel mechanism

**Rationale:** Pit-of-success applies to the tool surface. Without
explicit guidance the LLM will default to synchronous polling
because that is the shape it learned from training data. The
description is how that prior is corrected.

---

### REQ-WAKE-015: Cost Observability

THE SYSTEM SHALL emit metrics on:
- wake contract registration rate (per conv, per handle kind)
- average fire latency (registration to fire)
- expired-vs-fired ratio
- forgotten-vs-fired ratio (broken out by forgotten reason:
  `phoenix_restart`, `cascade_destroyed_handle`, `subagent_handle_missing`,
  `tmux_handle_missing`)

**Rationale:** A wake contract is a Phoenix-side commitment to spend
money. Operators must be able to see "how much wake is happening"
and "how often is wake the right primitive vs the wrong one."
Metrics also gate additional condition kinds: a kind such as
`RegexInTmuxPane` is justified only once these metrics show the
abstraction is used as intended.

---

### REQ-WAKE-016: Unified Tool Surface

THE SYSTEM SHALL expose a single `wait_until` tool to the LLM, taking
`{ handle: { kind: Bash | TmuxPane | SubAgent, id }, condition,
max_wait_seconds }`

THE tool SHALL NOT be split per substrate (no `bash_wait_until`,
`tmux_wait_until`, `subagent_wait_until`)

**Rationale:** A single tool means one description re-fed per turn
instead of three, bounding the tool-description tax. The unified shape
also forward-aligns with a unified `WorkHandle` trait — when that trait
lands, the tool surface stays the same and only the runtime dispatch
changes. The handle discriminator (`kind` + `id`) is structurally
explicit and validated at deserialization: a tagged enum, not
flat-Option-soup, makes `{ kind: Bash, id: "t-3" }` fail to parse when
there is no bash handle named `t-3`.

---

### REQ-WAKE-017: Sub-Agent Terminal Wake Payload

WHEN a `HandleTerminal/SubAgent` contract fires because the child called
`submit_result`
THE SYSTEM SHALL deliver a tagged `success` outcome with the submitted result text
and the child conversation / agent id derived from the contract's `handle_id`.

WHEN a `HandleTerminal/SubAgent` contract fires because the child called
`submit_error`
THE SYSTEM SHALL deliver a tagged `submitted_error` outcome with the submitted
error text, the same typed `ErrorKind` taxonomy used by normal sub-agent failure,
and the child conversation / agent id derived from the contract's `handle_id`.

WHEN the child reaches wall-clock timeout
THE SYSTEM SHALL preserve that timeout as the child's durable terminal cause and
deliver a tagged `timed_out` outcome with a message stating that the child
exceeded its configured timeout and the child conversation / agent id derived
from the contract's `handle_id`.

WHEN the child independently reaches a durable cancellation terminal state while
the parent wake contract remains active
THE SYSTEM SHALL deliver a tagged `cancelled` outcome with the cancellation reason
when available and the child conversation / agent id derived from the contract's
`handle_id`.

WHEN the waiting parent or wake contract itself is cancelled
THE SYSTEM SHALL resolve the wake contract with the top-level `Cancelled` cause,
not a fired sub-agent `cancelled` payload.

WHEN the child exhausts its turn-limit grace path and the runtime performs the
hard-stop fallback
THE SYSTEM SHALL preserve that hard-stop cause as the child's durable terminal
cause and deliver a tagged `turn_limit_exhausted` outcome with the extracted
partial assistant text when available and the child conversation / agent id
derived from the contract's `handle_id`.

WHEN the child reaches another bedrock terminal failure cause, including context
exhaustion or non-retryable runtime failure
THE SYSTEM SHALL preserve that cause as the child's durable terminal cause and
deliver the corresponding tagged terminal outcome. Runtime-failure outcomes SHALL
include the same typed `ErrorKind` taxonomy used by normal sub-agent failure,
including `invalid_response`, excluding durable terminal causes that have their
own tagged outcome (`timed_out`, `cancelled`, and `context_exhausted`).

WHEN bedrock admits text-only implicit completion as a child terminal state
THE SYSTEM SHALL preserve that cause as the child's durable terminal cause and
deliver a tagged `implicit_success` outcome rather than relabeling it as explicit
`submit_result`.

The tagged outcome SHALL make invalid combinations unrepresentable: each durable
child terminal cause maps to exactly one payload variant; success requires result
text; submitted errors require error text and kind; and at most one terminal
outcome variant is present.

WHEN the child conversation/agent id cannot be found during router evaluation
THE SYSTEM SHALL fire the contract with cause `Forgotten` and reason
`subagent_handle_missing`.

**Rationale:** Sub-agent wake is a terminal-result delivery mechanism, not a
continuation protocol. The parent receives enough structured information to
synthesize or recover, but v1 never asks the parent whether to extend the child or
send it another prompt.

---

### REQ-WAKE-018: V1 Handle Identity and Lifecycle

A `Bash` wake handle SHALL be addressed by the bash handle id returned by the bash
tool. The handle remains WorkScope-keyed: it transfers across a continuation that
inherits the same WorkScope, and a pending wake contract transfers with it per
REQ-WAKE-012. Because bash handles are in-memory, Phoenix restart fires pending
bash waits as `Forgotten { reason: "phoenix_restart" }`.

A `TmuxPane` wake handle SHALL be addressed by the tmux registry's stable
`window_id`. It is WorkScope-keyed for continuation inheritance and lifecycle
teardown. A hard-delete or WorkScope teardown with no inheriting successor fires
pending tmux waits as forgotten unless the registry already recorded a terminal
exit-marker or killed-window state; a surviving tmux handle is re-registered on
router startup.

A `SubAgent` wake handle SHALL be addressed by the child conversation / agent id
created for the sub-agent. It is not WorkScope-keyed and SHALL NOT transfer across
WorkScope inheritance. Because active sub-agent runtimes do not survive Phoenix
restart, startup resync SHALL first inspect the persisted child conversation and
its durable terminal cause: children whose terminal cause occurred before the
contract deadline deliver the corresponding tagged terminal payload;
non-terminal children fire `Forgotten { reason: "phoenix_restart" }`. Child
cancellation observed independently of parent wake cancellation produces a fired
sub-agent `cancelled` payload. Parent hard-delete SHALL
cancel pending wake contracts before deleting the child; hard-delete MUST NOT
report lifecycle cancellation as a missing child handle.

**Rationale:** The three v1 handle kinds deliberately use their existing stable
identities. The wake plane does not invent a parallel handle namespace, and it
does not treat sub-agents as WorkScope resources.


## Dependencies

- `specs/bash/` REQ-BASH-002 (wait semantics) — REQ-WAKE-006
  delivery format reuses BASH wait response shape
- `specs/tmux-integration/` — TmuxPane condition kind
- `specs/subagents/` — SubAgent condition kind
- `specs/bedrock/` REQ-BED-032 (hard-delete cascade) — REQ-WAKE-004
  is_busy() augmentation joins the cascade's busy-check

## Related Specs

- `specs/bash/` REQ-BASH-WS-001 / -WS-002 — bash handles are WorkScope-keyed
  and inherit across a continuation that shares the scope, so REQ-WAKE-012
  transfers a bash-keyed contract to the child along with its handle
- `specs/subagents/` — a sub-agent is tracked by its own agent /
  child-conversation id, so a subagent-keyed contract is keyed independently
  of the parent's WorkScope (REQ-WAKE-012)
