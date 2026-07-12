# Wake Contracts — Executive Summary

## Requirements Summary

Wake contracts let an LLM agent register a persistent, conversation-scoped
commitment of the shape "wake me when this bash handle or tmux window reaches a
terminal outcome, or when this bounded wait expires." The registration call
returns an immediate receipt after durable persistence. The later terminal fact
is delivered as a typed Phoenix runtime observation correlated by `contract_id`,
not as a delayed synthetic tool result.

The core plane is durable and normalized: pending contracts, terminal outcomes,
child tail rows, and unconsumed wake inbox items all survive restart. The wake
observer and startup reconciler both resolve contracts by the same durable
protocol: terminal evidence with `evidence_at <= expires_at` wins over expiry;
otherwise expiry occurs at `now >= expires_at`; explicit cancel produces a
cancelled observation; and forgotten is reserved for handles Phoenix can no
longer observe.

Wake ownership is conversation-scoped and continuation-safe. When a continuation
creates a successor, all pending contracts and all unconsumed wake observations
transfer to that successor before any later delivery, regardless of WorkScope
inheritance. Handle survival remains a separate concern.

Wake scheduling is durable. Materializing an inbox snapshot creates its meta-user
observation and a pending resume outbox row atomically. Busy runtimes and failed
runtime sends leave that row pending, and startup retries it. Acceptance atomically
persists `LlmRequesting` and marks the row accepted before LLM dispatch. A
continuation copies the exact pending observation into successor history and
rekeys the outbox to that successor-safe message while retaining predecessor
history. Explicit cancel appends a cancelled observation but does not itself
create a resume outbox row.

Runtime observability records stable structured fields for registration, terminal
resolution and latency, reconciliation batches, inbox coalescing, queued/pending
resume dispatch, atomic outbox acceptance, and startup recovery counts. Logs carry
handle and cause metadata but omit terminal payloads and output tails.

Pending wake contracts also derive a lifecycle-blocking signal distinct from
`is_busy`. Archive, abandon, mark-merged, and hard-delete lifecycle actions
reject or conflict while pending wake obligations remain.

## Technical Summary

The spec models three durable structures:
- `Contract` for the obligation and terminal accounting,
- `WakeInboxItem` for unconsumed runtime observations, and
- tail child rows for bash/tmux final output snippets.

The Allium contract now models the user-visible wake payload as a tagged runtime
observation envelope. Registration receipts contain `contract_id`, handle, and
`expires_at`, with `registering_tool_use_id` present only as audit metadata.
Delivered observations are correlated by `contract_id`; they are not attributable
through delayed tool-result semantics.

The handle payload shape is structural rather than conventional. `WaitHandle` is
a tagged variant, so a single wake registration cannot simultaneously contain a
bash body and a tmux body. Fired payloads are likewise tagged, which keeps bash
terminal metadata and tmux terminal metadata disjoint by construction.

The Allium file now explicitly declares actors, surfaces, and helper names for
every externally initiated trigger in scope:
- `wait_until` registration,
- explicit cancel,
- observer ticks and evidence arrival,
- startup reconciliation,
- continuation transfer,
- lifecycle actions, and
- wake resume consumption.

It also imports the tmux Allium spec alongside bedrock and bash so the bash/tmux
scope is represented directly in the dependency set.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| REQ-WAKE-001 | Specified | Registration returns immediate persisted receipt; later delivery is runtime observation |
| REQ-WAKE-002 | Specified | Normalized contract, inbox, and tail-row storage model |
| REQ-WAKE-003 | Specified | Scope limited to bash handles and tmux window handles |
| REQ-WAKE-004 | Implemented | Continuation transfers contracts, inbox rows, and successor-visible pending resume messages |
| REQ-WAKE-005 | Implemented | Durable inbox/outbox scheduling with atomic acceptance and restart retry |
| REQ-WAKE-006 | Specified | Exactly-once terminal resolution transaction |
| REQ-WAKE-007 | Specified | Evidence-vs-expiry semantics use `evidence_at <= expires_at` and `now >= expires_at` |
| REQ-WAKE-008 | Specified | Startup reconciliation runs before live serving and uses the same durable rules |
| REQ-WAKE-009 | Specified | Terminal vocabulary is Fired / Expired / Cancelled / Forgotten |
| REQ-WAKE-010 | Specified | Explicit cancel appends observation but does not itself schedule an LLM resume |
| REQ-WAKE-011 | Specified | Lifecycle blocking is separate from busy execution and gates destructive actions |
| REQ-WAKE-012 | Specified | One LLM at a time; busy arrivals persist; sibling contracts remain independent |
| REQ-WAKE-013 | Specified | Delivery is typed Phoenix runtime observation, not synthetic tool result |
| REQ-WAKE-014 | Specified | Registration/cancel authorization follows conversation ownership boundary |
| REQ-WAKE-015 | Specified | Timeout default is 600s; explicit range is 1..=1800s |

## Dependencies

- `specs/bedrock/bedrock.allium` — conversation ownership, busy execution, and continuation context
- `specs/bash/bash.allium` — bash terminal status and tail semantics mirrored by wake observation
- `specs/tmux-integration/tmux-integration.allium` — tmux window identity and durable terminal evidence surface

## Verification

- Database tests cover snapshot/outbox creation, atomic acceptance, accepted-row exclusion, and continuation transfer into successor history.
- State-machine tests cover idle acceptance and duplicate/stale absorption without another turn.
- Runtime tests cover cancellation-only behavior, busy and send-failure retention, successful scheduling, and retry of persisted pending outbox rows.
- Runtime helper tests cover stable observability cause mappings, coalesced cause counts, and registered-to-resolved latency derivation.
- `allium check` / `allium analyse` over the wake, bedrock, bash, and tmux dependency set
- spec validation lanes via `./dev.py check --lanes allium,spec-shape,spec-anchors`

## Why This Spec Exists

Wake contracts make "owed later runtime delivery" a first-class durable
capability instead of a poll loop. The spec exists to define that capability in a
way that survives restart, preserves conversation ownership across continuation,
and keeps runtime observations distinct from tool-result attribution.
