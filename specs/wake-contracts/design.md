# Wake Contracts — Design

## Why Two Axes (Wake vs WorkScope)

The WorkScope work just landed (PRs #136, #143, #139) and answered a
*resource ownership* question: when a conversation continues into a
child, which side owns the tmux server / browser session / worktree?
The answer was scope-equality preservation — if the child inherits the
scope, the cascade preserves the resource; if not, the cascade kills.

Wake contracts answer a fundamentally different question: when an
external event happens, which conversation gets resumed?

These look related ("both involve conversation boundaries") but the
relation is incidental. WorkScope is *resource lifetime*. Wake is
*conversation resumption*. A future regex-on-tmux-pane wake will fire
the registering conversation even if the underlying tmux server is
inherited by a different conversation — because the contract is the
runtime's commitment to *that LLM session*, not to the underlying
substrate.

The earlier draft of "watch" conflated these axes. Splitting them is
why WorkScope shipped first as scope-equality and wake is a separate
spec rather than another condition on the same edge.

## Why No AwaitingWake State

The earliest draft of this spec introduced an `AwaitingWake` conv
state by analogy with `AwaitingTaskApproval` / `AwaitingContinuation`.
Resolved against: the existing `Awaiting*` family is exclusively
"waiting on the user." Wake is "waiting on the runtime," which is a
different category by every relevant property:

| Property | `Awaiting*` (today) | Wake |
|----------|---------------------|------|
| Who unblocks the wait? | The user | The runtime / external event |
| Does the user need to take an action? | Yes | No |
| Should user input be disabled? | Implicitly yes (the conv is asking the user something) | No (the conv is doing background work) |
| Is there a single "answer" expected? | Yes (approve, continue) | No (the answer is "the event happened") |

Modelling wake as `AwaitingWake` would have created a state with
exactly the wrong semantics: a conv that *can* accept user input but
that the state name suggests *cannot*. The pit-of-success direction
is to keep the conv in `Idle`, persist the contract row as the single
source of truth for "is this conv waiting on something," and augment
`is_busy()` to read from the contract table (REQ-WAKE-004).

Concretely, this avoids:
- Two representations of the same semantic value (conv state AND
  contract row), which Voss panel identified as a recurring
  correctness anti-pattern in this codebase
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

Earlier draft was "first-fire-wins, cancel siblings." Reconsidered
when removing AwaitingWake: that semantics only made sense in a
model where the conv was in a single-contract-bound `AwaitingWake`
state and needed to "leave" that state on fire. With contracts as
free-standing rows decoupled from conv state, the more honest
semantics is "each contract fires independently into the conv's
message log." If the LLM wants supersede behaviour, it can register
a single contract over a `wait_any { handle_a, handle_b }` condition
in v2; conflating supersede into v1's single-condition shape was
premature.

### Synthetic tool result vs new conversation event kind

REQ-WAKE-006 chose "synthetic tool result that looks identical to the
synchronous-wait response." Alternative was a new `WakeFired`
conversation event kind that the LLM would have to learn to recognize
distinctly. The synthetic-tool-result approach makes the wake
primitive a drop-in replacement for `op=wait` from the LLM's vantage
point — the only difference is that wake consumes zero turns until
fire. Marin panel review specifically argued for this shape: "the
LLM should not have to learn a parallel taxonomy for 'I waited
synchronously' vs 'I registered a contract.'"

### Why HandleTerminal first

Three condition kinds were considered for v1:
- `HandleTerminal` — fires on process exit
- `RegexInTmuxPane` — fires on regex match in pane capture
- `FileChanged` — fires on file mtime advance

`HandleTerminal` is the only one whose evaluator is a pure read of
existing state (`HandleState` is already there). The other two
require new poller infrastructure (tmux capture-pane scheduling,
file-watcher). Shipping `HandleTerminal` first validates the
persistence + delivery + state-machine pieces in isolation;
condition-kind growth becomes a matter of adding evaluators against
the same edge.

### Why no WebhookFired in v1

Webhook wake (Phoenix exposes an endpoint that fires a contract on
HTTP POST) opens a security surface: who is authorized to wake a
conversation, what payload format, what rate-limiting, how does the
auth model interact with the existing browser-tool auth surface. Not
a v1 question. Will be a separate spec when it lands.

### Polling cadence, not push-from-handle

The wake-router polls. An alternative would be the handle code
(bash/tmux/subagent) pushing terminal events directly into the
router. Push is more efficient (no idle ticks) but requires each
spawn substrate to know about the wake router — a coupling that
today does not exist. Poll-pull keeps the router as the single point
of contract knowledge and lets each handle substrate stay unaware.
v2 may convert `HandleTerminal` specifically to push (it's a one-line
change in the bash waiter-task) once the abstraction is proven.

### Unified `wait_until` tool, not per-substrate

Resolved per asking-questions Q3 (2026-05-24). Trade-off:
- **Per-substrate** (rejected): three tools (`bash_wait_until`,
  `tmux_wait_until`, `subagent_wait_until`), each tightly typed to
  its handle namespace. Lower upfront design cost. Higher
  description tax in context (~3x).
- **Unified** (chosen): one tool with tagged-enum handle
  discriminator. Higher upfront design cost (one shared tool surface
  to spec). Lower context cost. Forward-aligned with the eventual
  `WorkHandle` trait Voss + Marin both recommended.

Implementation must use a `#[serde(tag = "kind")]` enum on the
`handle` parameter so that `{ kind: Bash, id: "x" }` paired with a
non-existent bash handle id fails at validation rather than as a
runtime error. This is the Voss panel's correct-by-construction
principle applied at the tool surface.

### `is_busy()` augmentation, not state mutation

REQ-WAKE-004 makes `is_busy()` consult `wake_contracts` rather than
introducing an `AwaitingWake` state. The cost is one SQLite count
query per `is_busy()` evaluation. Lifecycle endpoints already do at
least one SQLite read; the additional count is negligible. The
correctness payoff is that the contract row remains the single
source of truth — there is no "the state says I'm AwaitingWake but
the contract table says I have zero contracts" failure mode.

## Resolved Questions

### REQ-WAKE-012 — continuation inheritance: RESOLVED

Every handle kind a contract can watch — bash, tmux, subagent — is
WorkScope-keyed (bash per `specs/bash/` REQ-BASH-WS-001). When a conversation
continues into a successor that inherits the same WorkScope, the underlying
handle transfers to the successor, and the pending contract transfers with it:
its `conv_id` is re-keyed to the child so the eventual fire or expire lands in
the continuation. No contract fires `Forgotten` at the continuation boundary.
`Forgotten` remains the terminal cause when a handle is destroyed for another
reason — notably a Phoenix restart, which drops in-memory bash handles (see
REQ-WAKE-002), or a hard-delete with no inheriting scope.

### REQ-WAKE-013 — user-interrupt semantics: RESOLVED (N/A)

Because the conv stays in `Idle` while waiting (no `AwaitingWake`
state), user messages and wake fires are both just events that
append to the conv message log. Ordering is by arrival; the next
LLM turn includes whatever has accumulated. The race "both arrive
in the same millisecond" is serialized by the existing per-conv
lock around message-log appends. No new policy needed.

### Tool surface — one tool or many: RESOLVED

Unified `wait_until` (REQ-WAKE-016). See "Unified `wait_until`
tool" decision above.

## Out of Scope (v1)

- Cross-conversation wake (one conversation wakes another)
- Webhook-triggered wake
- Compound conditions (`wake_when_A_or_B`, `wait_any`, `wait_all`)
- Wake with retry (`fire_on_condition_for_max_N_times`)
- Wake against external systems (GitHub API, Slack)
- File-watcher conditions
- Regex/content-match conditions

Each of these is a future condition-kind or future contract-shape.
v1 ships the foundation; v2+ adds kinds without revisiting the
foundation.

## Observability Plan

- New SSE events: `WakeContractRegistered`, `WakeContractFired`,
  `WakeContractCancelled`, `WakeContractExpired`,
  `WakeContractForgotten`
- New UI: wake indicator on Idle convs with pending contracts;
  per-contract cancel affordance
- Metrics (REQ-WAKE-015): registration rate, fire latency,
  expired/forgotten ratios with reason breakdown
- `phoenix-client.py` gets a `wake-status <conv-id>` verb for CLI
  debug

## Migration / Rollout

- Phase 0: ship the spec (this PR)
- Phase 1: types, DB migration, wake-router service, `is_busy()`
  augmentation, restart resync (no LLM-facing tool yet; exercised
  by tests only)
- Phase 2: ship `wait_until` tool with HandleTerminal/Bash only;
  validate against real conversations; metrics + observability
  surface
- Phase 3: HandleTerminal/TmuxPane + HandleTerminal/SubAgent
  (still single condition kind)
- Phase 4 (separate spec revision): additional condition kinds
  (regex, file, port)

Each phase is independently shippable. Phase 1 has no LLM surface
so ships dark; phases 2-4 are progressive LLM capability rollout.
