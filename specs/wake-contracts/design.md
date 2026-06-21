# Wake Contracts — Design

## Why Two Axes (Wake vs WorkScope)

WorkScope answers a *resource ownership* question: when a conversation
continues into a child, which side owns the tmux server / browser
session / worktree? The answer is scope-equality preservation — if the
child inherits the scope, the cascade preserves the resource; if not,
the cascade kills.

Wake contracts answer a fundamentally different question: when an
external event happens, which conversation gets resumed?

These look related ("both involve conversation boundaries") but the
relation is incidental. WorkScope is *resource lifetime*. Wake is
*conversation resumption*. A regex-on-tmux-pane wake fires the
registering conversation even if the underlying tmux server is
inherited by a different conversation — because the contract is the
runtime's commitment to *that LLM session*, not to the underlying
substrate.

Conflating these axes is the design error this split avoids. WorkScope
governs resource lifetime as scope-equality; wake is a separate
concern keyed to a conversation, not another condition on the same
edge.

## Why No AwaitingWake State

An `AwaitingWake` conv state — modelled by analogy with
`AwaitingTaskApproval` / `AwaitingContinuation` — is deliberately not
introduced. The existing `Awaiting*` family is exclusively "waiting on
the user." Wake is "waiting on the runtime," which is a different
category by every relevant property:

| Property | `Awaiting*` (today) | Wake |
|----------|---------------------|------|
| Who unblocks the wait? | The user | The runtime / external event |
| Does the user need to take an action? | Yes | No |
| Should user input be disabled? | Implicitly yes (the conv is asking the user something) | No (the conv is doing background work) |
| Is there a single "answer" expected? | Yes (approve, continue) | No (the answer is "the event happened") |

Modelling wake as `AwaitingWake` would create a state with exactly
the wrong semantics: a conv that *can* accept user input but that the
state name suggests *cannot*. The pit-of-success direction is to keep
the conv in `Idle`, persist the contract row as the single source of
truth for "is this conv waiting on something," and augment `is_busy()`
to read from the contract table (REQ-WAKE-004).

Concretely, this avoids:
- Two representations of the same semantic value (conv state AND
  contract row), a recurring correctness anti-pattern in this codebase
- A state-machine special case for "user message during
  AwaitingWake" — with the conv in `Idle`, user messages are normal
  Idle conv messages and need no special policy
- UI confusion about whether the conv is "waiting on you" or "waiting
  on something else" — `Idle + wake indicator` is unambiguous; the
  user knows they can still type

## Design Decisions

### Persisted contract, not in-memory wake handle

A pure in-memory wake (router holds Vec<Contract> in process, fires
SSE on condition) would be simpler to build but inherits every silent-
loss path that the bash module already pays for. Phoenix restarts
during a long wait would orphan the LLM, exactly the failure mode the
spec exists to eliminate. SQLite persistence is the cost of being
honest about "this is a real commitment to fire the LLM."

### Independent multi-contract semantics

Each contract fires independently into the conversation's message log.
A conversation may hold several pending contracts; the first to
resolve delivers its payload and the siblings continue to be
evaluated. The rejected alternative — "first-fire-wins, cancel
siblings" — only makes sense in a model where the conv is in a
single-contract-bound `AwaitingWake` state and must "leave" that state
on fire. With contracts as free-standing rows decoupled from conv
state, independent firing is the honest semantics. Supersede
behaviour, if needed, belongs in a single contract over a compound
`wait_any { handle_a, handle_b }` condition rather than folded into
the single-condition contract shape.

### Synthetic tool result vs new conversation event kind

A contract fires by delivering a synthetic tool result that looks
identical to the synchronous-wait response (REQ-WAKE-006), rather than
a distinct `WakeFired` conversation event kind. The synthetic-tool-
result approach makes the wake primitive a drop-in replacement for
`op=wait` from the LLM's vantage point — the only difference is that
wake consumes zero turns until fire. The LLM should not have to learn
a parallel taxonomy for "I waited synchronously" vs "I registered a
contract."

### HandleTerminal is the sole condition kind

`HandleTerminal` (fires on process exit) is the only condition kind
whose evaluator is a pure read of existing state — the handle's
`HandleState` is already tracked. Other candidate kinds —
`RegexInTmuxPane` (regex match in pane capture), `FileChanged` (file
mtime advance) — require new poller infrastructure (tmux capture-pane
scheduling, a file-watcher) and are out of scope (see "Out of Scope").
`HandleTerminal` exercises the persistence + delivery + state-machine
pieces in isolation; condition-kind growth is a matter of adding
evaluators against the same edge.

### No WebhookFired condition kind

Webhook wake (Phoenix exposes an endpoint that fires a contract on
HTTP POST) opens a security surface: who is authorized to wake a
conversation, what payload format, what rate-limiting, how the auth
model interacts with the browser-tool auth surface. It is out of scope
and belongs in a separate spec governing that surface.

### Polling cadence, not push-from-handle

The wake-router polls. The alternative — the handle code
(bash/tmux/subagent) pushing terminal events directly into the router
— is more efficient (no idle ticks) but requires each spawn substrate
to know about the wake router, a coupling that does not otherwise
exist. Poll-pull keeps the router as the single point of contract
knowledge and lets each handle substrate stay unaware.

### Unified `wait_until` tool, not per-substrate

A single `wait_until` tool takes a tagged-enum handle discriminator
rather than splitting into per-substrate tools (`bash_wait_until`,
`tmux_wait_until`, `subagent_wait_until`). Per-substrate tools would
each be tightly typed to their handle namespace at lower upfront
design cost, but triple the tool-description tax carried in context
every turn. The unified shape pays one shared tool surface to spec
in exchange for low context cost, and aligns with a unified
`WorkHandle` trait: under such a trait the tool surface is unchanged
and only the runtime dispatch differs.

The implementation uses a `#[serde(tag = "kind")]` enum on the
`handle` parameter so that `{ kind: Bash, id: "x" }` paired with a
non-existent bash handle id fails at validation rather than as a
runtime error — correct-by-construction at the tool surface.

### `is_busy()` augmentation, not state mutation

REQ-WAKE-004 makes `is_busy()` consult `wake_contracts` rather than
introducing an `AwaitingWake` state. The cost is one SQLite count
query per `is_busy()` evaluation. Lifecycle endpoints already do at
least one SQLite read; the additional count is negligible. The
correctness payoff is that the contract row remains the single
source of truth — there is no "the state says I'm AwaitingWake but
the contract table says I have zero contracts" failure mode.

### Continuation inheritance: how a watched handle's keying determines transfer

A watched handle's keying determines what happens to a pending
contract when its conversation continues into a successor.

The watchable handle kinds fall into two keying classes:

- **WorkScope-keyed: bash and tmux.** A backgrounded bash process and
  a tmux pane are `WorkScope`-level resources (bash per `specs/bash/`
  REQ-BASH-WS-001). When a conversation continues into a successor that
  inherits the same `WorkScope`, the underlying handle transfers to the
  successor, and a pending contract on that handle transfers with it:
  its `conv_id` is re-keyed to the child so the eventual fire or expire
  lands in the continuation.

- **Agent-id-keyed: subagent.** A sub-agent is tracked by its own
  agent / child-conversation id (per `specs/subagents/` — a fresh
  `agent_id` is generated at spawn), not by the parent's `WorkScope`.
  The sub-agent's completion signal is therefore stable and independent
  of any `WorkScope` the parent shares with a continuation: continuing
  into a same-`WorkScope` successor does not make an already-spawned
  sub-agent a `WorkScope`-owned resource of the successor, and a
  subagent-keyed contract is NOT transferred by `WorkScope` inheritance.
  Re-keying it by `WorkScope` would re-point a sub-agent's completion
  wake at the wrong conversation.

`Forgotten` is the terminal cause when a watched handle is destroyed:
a Phoenix restart that drops in-memory bash handles (see REQ-WAKE-002),
or a hard-delete that tears down a WorkScope with no inheritor.

### User-interrupt semantics during a wait

Because the conv stays in `Idle` while waiting (no `AwaitingWake`
state), user messages and wake fires are both just events that
append to the conv message log. Ordering is by arrival; the next
LLM turn includes whatever has accumulated. The race "both arrive
in the same millisecond" is serialized by the existing per-conv
lock around message-log appends. No special policy is needed.

### Tool surface — one tool

The LLM-facing surface is the unified `wait_until` tool
(REQ-WAKE-016). See "Unified `wait_until` tool" above.

## Out of Scope

- Cross-conversation wake (one conversation wakes another)
- Webhook-triggered wake
- Compound conditions (`wake_when_A_or_B`, `wait_any`, `wait_all`)
- Wake with retry (`fire_on_condition_for_max_N_times`)
- Wake against external systems (GitHub API, Slack)
- File-watcher conditions
- Regex/content-match conditions
- Wall-clock / deadline conditions — fire at an absolute timestamp,
  independent of any watched handle. Structurally distinct from the
  handle-owned conditions above: there is no handle to authorize against
  (`handle_owned_by` does not apply) and nothing to fire `forgotten`,
  so it would relax the handle-ownership precondition rather than add an
  evaluator on the same edge. A degenerate form of this — returning a
  conversation stuck on a quota-window error to Idle once its reported
  reset time passes — needs none of the handle machinery and is therefore
  handled outside this subsystem rather than as a wake contract.

Each of these is a separate condition-kind or contract-shape governed
by its own spec revision. The foundation — persistence, delivery,
the wake-router edge — accommodates additional condition kinds via a
discriminator without being revisited.

## Observability

- SSE events: `WakeContractRegistered`, `WakeContractFired`,
  `WakeContractCancelled`, `WakeContractExpired`,
  `WakeContractForgotten`
- UI: wake indicator on Idle convs with pending contracts;
  per-contract cancel affordance
- Metrics (REQ-WAKE-015): registration rate, fire latency,
  expired/forgotten ratios with reason breakdown
- `phoenix-client.py` exposes a `wake-status <conv-id>` verb for CLI
  debug
