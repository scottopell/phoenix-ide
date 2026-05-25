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

## Design Decisions

### Persisted contract, not in-memory wake handle

A pure in-memory wake (router holds Vec\<Contract\> in process, fires
SSE on condition) would be simpler to build but inherits every silent-
loss path that the bash module already pays for. Phoenix restarts
during a long wait would orphan the LLM, exactly the failure mode the
spec exists to eliminate. SQLite persistence is the cost of being
honest about "this is a real commitment to fire the LLM."

### First-fire-wins for multi-contract

Considered: `wait_all`, `wait_any`, `wait_first_then_cancel_others`.
The first two are useful and absent. The third is what v1 ships
(REQ-WAKE-010) because it's the only one that maps cleanly onto a
single tool result delivery — the conversation gets one tool result on
fire, transitions out of `AwaitingWake`, and continues. Adding
`wait_all` would require either a result-aggregation phase (delay
fire-delivery until last contract resolves) or a multi-tool-result
delivery model (deliver each as it fires, conversation stays in
`AwaitingWake` until all resolved). Both are real designs; neither is
load-bearing for v1.

### Synthetic tool result vs new conversation event kind

REQ-WAKE-006 chose "synthetic tool result that looks identical to the
synchronous-wait response." Alternative was a new `WakeFired`
conversation event kind that the LLM would have to learn to recognize
distinctly. The synthetic-tool-result approach makes the wake primitive
a drop-in replacement for `op=wait` from the LLM's vantage point — the
only difference is that wake consumes zero turns until fire. Marin
panel review specifically argued for this shape: "the LLM should not
have to learn a parallel taxonomy for 'I waited synchronously' vs 'I
registered a contract.'"

### Why HandleTerminal first

Three condition kinds were considered for v1:
- `HandleTerminal` — fires on process exit
- `RegexInTmuxPane` — fires on regex match in pane capture
- `FileChanged` — fires on file mtime advance

`HandleTerminal` is the only one whose evaluator is a pure read of
existing state (`HandleState` is already there). The other two require
new poller infrastructure (tmux capture-pane scheduling, file-watcher).
Shipping `HandleTerminal` first validates the persistence + delivery +
state-machine pieces in isolation; condition-kind growth becomes a
matter of adding evaluators against the same edge.

### Why no WebhookFired in v1

Webhook wake (Phoenix exposes an endpoint that fires a contract on
HTTP POST) opens a security surface: who is authorized to wake a
conversation, what payload format, what rate-limiting, how does the
auth model interact with the existing browser-tool auth surface. Not a
v1 question. Will be a separate spec when it lands.

### Polling cadence, not push-from-handle

The wake-router polls. An alternative would be the handle code
(bash/tmux/subagent) pushing terminal events directly into the router.
Push is more efficient (no idle ticks) but requires each spawn substrate
to know about the wake router — a coupling that today does not exist.
Poll-pull keeps the router as the single point of contract knowledge
and lets each handle substrate stay unaware. v2 may convert
`HandleTerminal` specifically to push (it's a one-line change in the
bash waiter-task) once the abstraction is proven.

## Open Questions (Must Resolve Before Implementation)

### Continuation inheritance (REQ-WAKE-012)

When a conversation A continues into conversation B, and A had
registered wake contracts:

- (a) Contracts transfer to B; B is now in `AwaitingWake` from the
  moment of continuation; the LLM in B sees the wake fire when it
  happens.
- (b) Contracts fire `Forgotten` on continuation; A's transcript
  records the forgotten event; B starts in `Idle`.
- (c) Contracts depend on whether the underlying handle is inherited
  by B — if yes, contract transfers; if no, contract fires
  `Forgotten`.

(c) is the most coherent but is blocked on [[bash-cascade-skips-
inheritor-scope]] resolution. Until bash handle inheritance is
decided, (b) is the safe fallback for v1.

### User-interrupt semantics (REQ-WAKE-013)

Three positions, each defensible:

- **Queue:** user message accumulates; on fire, both deliver together.
  Honors user intent without losing the wait. Complicates the state
  machine (where does the queued message live?).
- **Cancel:** user message cancels all contracts. Honors the principle
  "user attention overrides background work." Loses the wait result.
- **Reject:** explicit "the conversation is busy; cancel the wake or
  wait." Honors no-implicit-state-change but is the worst UX (user
  sees an error).

Cancel is the simplest correct behavior. Queue is the best UX but
requires a `pending_user_message` field on `AwaitingWake`. Reject is
not acceptable as a default.

### Tool surface — one tool or many?

Two viable shapes:

- **Per-substrate registration:** `bash_wait_until { handle, condition }`,
  `tmux_wait_until { handle, condition }`, `subagent_wait_until {
  handle, condition }`. Each is a thin wrapper that knows how to
  resolve its handle type to a contract row.
- **Unified registration:** a single `wait_until { handle, condition }`
  tool that takes a `handle` discriminator carrying both kind and id.

The per-substrate shape has more tool-description tax (Henrik panel
concern) but keeps each tool's docs short and its handle-id format
obvious. The unified shape is consistent with the eventual `WorkHandle`
trait Voss panel recommended. Decision should defer to whether the
`WorkHandle` trait gets built before or after this spec ships. If
`WorkHandle` first, build the unified tool. If wake-contracts first,
build per-substrate and converge later.

## Out of Scope (v1)

- Cross-conversation wake (one conversation wakes another)
- Webhook-triggered wake
- Conditional wake (`wake_when_A_or_B`)
- Wake with retry (`fire_on_condition_for_max_N_times`)
- Wake against external systems (GitHub API, Slack)
- File-watcher conditions
- Regex/content-match conditions

Each of these is a future condition-kind or future contract-shape
against the same `AwaitingWake` edge. v1 ships the edge + one condition
kind; v2+ adds kinds without touching the edge.

## Observability Plan

- New SSE events: `WakeContractRegistered`, `WakeContractFired`,
  `WakeContractCancelled`, `WakeContractExpired`, `WakeContractForgotten`
- New conv state visible in UI: `AwaitingWake` chip with `expires_at`
  countdown
- Metrics (REQ-WAKE-015): registration rate, fire latency,
  expired/forgotten ratios
- `phoenix-client.py` gets a `wake-status <conv-id>` verb for CLI debug

## Migration / Rollout

- Phase 0: ship the spec; resolve open questions
- Phase 1: types, DB migration, `AwaitingWake` state machine variant,
  router service (no LLM-facing tools yet, exercised by tests only)
- Phase 2: ship `bash_wait_until` (HandleTerminal only) as the first
  LLM-facing surface; validate against real conversations
- Phase 3: `tmux_wait_until` + `subagent_wait_until` (still
  HandleTerminal only)
- Phase 4 (separate spec revision): additional condition kinds

Each phase is independently shippable. Phase 1 has no LLM surface so
ships dark; phases 2-4 are progressive LLM capability rollout.
