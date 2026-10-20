# ADR-184: Local goal-watch facts belong to their causal event

- **Status:** Accepted
- **Date:** 2026-10-20
- **Affects:** REQ-GOAL-011 through REQ-GOAL-014, REQ-GR-015, REQ-CEVT-006; extends ADR-070/182/183 without expanding ADR-180/181's federation contract

## Context

Ordinary goal storage, report checkpoints, independent Stop controls and the
ordinary factual UI have exact-head hosted evidence. Watch notifications already
have one transactional event outbox, typed subscription and mandatory Close
routes, wait-entry identities, durable input acceptance and continuation rules.
The next commission requests factual goal context through that existing lane,
not a parallel watcher or automatic follow-through engine.

A blind notification at every accepted goal-history append would notify twice
when a report is followed by a terminal or wait event. Suppressing execution
notifications by goal version would instead lose distinct execution facts.
Reports can also return to `LlmRequesting`; that state does not prove a specific
future notification boundary. Stop commits an independent latch/receipt, not a
goal suspension or a guaranteed Cancelled occurrence.

ADR-182 made objectives immutable and removed objective revisions. The current
goal is not the admission-time Underway binding: it can be Blocked, Complete,
Exhausted or Suspended. Transcript/root usage is not a goal-scoped ledger.
Historical notification facts must not follow subsequent current-goal changes or
pin supported source deletion. Existing outbox constraints require a new
forward schema change. Parent coordination reserved migration 135 for this local
watch increment after inspecting the live federation stack and other owners;
132/133/134 remain imported goal/federation/control identities, not edit targets.

## Options considered

1. **Fan out every goal-history append.** Small insertion point, but duplicates
   reports at actual boundaries and requires new notification ownership and
   Stop/recovery rules to repair those duplicates.
2. **Poll live goals from another worker or reproject on delivery/read.** Avoids
   changing source transactions, but creates a second scheduler and changes the
   meaning of historical events as the source changes.
3. **Assign one causal owner and capture in its transaction.** Enrich actual
   local execution/wait events; use standalone goal-change occurrences only for
   accepted settled Create/State mutations. Leave nonboundary-report coverage
   and remote goal-change fanout outside this bounded cut.

## Decision

Choose option 3. Goal facts supplement rather than replace the existing outcome
and route. Retain Completed/Failed/Cancelled and wait semantics; Completed is
still “Turn finished,” never inferred goal completion. Preserve mandatory Close
and federation terminal-fact payloads, provenance, subscriptions and cursors.

### Ownership and ordering

| Source sequence | Owner and ordering chosen for this cut |
|---|---|
| Report → terminal | `persist_goal_terminal_tool_round` records the report checkpoint and terminal obligation; actual `terminalize_authoritative_turn_with_occurrence_at_cut` → `record_terminal_event_tx` owns the execution event and saves goal facts then. No earlier report goal-change event. |
| Report → wait | `finalize_round_with_management_tx` resolves reports before `persist_tool_round_state_tx` → `record_wait_entry_tx`; the actual wait entry owns one packet with the resulting facts. |
| Report → nonboundary state | No standalone report event or assumed terminal guarantee. A later actual boundary captures facts in its own transaction. |
| Settled Create/State | The accepted target-mutation transaction appends the standalone obligation. A Coordinator caller's own execution completion is not the target's boundary. |
| Stop → possible Cancelled | `apply_goal_control` remains the independent latch/receipt transaction, with no added goal-history or watch write. Only a later actual cancellation owns a Cancelled event; no active execution means no invented cancellation. |
| Continuation/context handoff | Existing suppression and terminal-obligation ownership remain authoritative. No synthetic terminal, duplicate report packet or new wake worker. |

Standalone identity is ordinary owner, accepted owner/status version and retained
watch enrollment identity, through the existing delivery identity. Rejects and
exact accepted retries create no obligation; re-enrollment does not replay one.
Accepted versions with equal states but different reasons remain distinct.
Normal execution and wait occurrences retain their existing occurrence/generation
identities and are never deduplicated by goal version.

### Transaction failure policy

Capture and required append operate on the existing owning transaction and only
the relevant owner's rows. They do not decode unrelated retained watch payloads.
Unclassified SQL, shape, constraint, I/O and commit errors propagate as storage
or checkpoint failures, not semantic refusals or success with dropped facts.

| Failure boundary | Retained result |
|---|---|
| Report → wait checkpoint | Report changes, paired tool evidence, state, wait fact and obligation roll back together. |
| Terminal transaction after a committed report checkpoint | Terminal projection/outbox changes roll back. Already committed report evidence and terminal obligation remain governed by existing settlement; no cross-transaction atomicity is invented. |
| Settled mutation | Mutation, history, actors, request result and standalone obligation roll back together. |
| Stop control | No new watch dependency. Existing control storage failure remains failure. A later Cancelled-event failure cannot undo a previously committed latch. |
| Delivery after commit | Existing pending-event admission/retry handles it without rolling back source facts. |

### Capture, storage and presentation

Use typed captured-goal, captured-no-goal with observed owner version, and legacy
not-captured cases. SavedGoal carries the current owner, immutable goal
identity/objective, accepted owner/status version, state and exact reason at the
event transaction, including inactive states. No objective revision or second
live goal authority is introduced. Usage is explicitly unavailable/not captured;
this cut adds no accounting reads, sums, budget inference or ceiling.

Store normalized immutable event-owned capture, with an event-lifetime foreign
key but no source goal/history/product/transcript foreign key. The snapshot is
historical evidence for one consumer, not an independently writable current
record. Reads and delivery use that evidence; they never join a later current
goal as a substitute. Existing events remain legacy not-captured rather than
being backfilled from current state.

A separately coordinated forward migration extends local goal-change constraints
and capture storage. At this decision parent coordination has reserved 135; no
registry entry or DDL is authored. Reservation does not authorize importing or
stacking another owner's branch. Preserve all embedded migrations, including
132/133/134. Inventory the latest `question_wait_outbox.sql` schema plus
federation and retirement overlays:
wait and mandatory route checks, payload immutability, ordinary-turn/steering
acceptance triggers, pending-source restrictions, all three inbound message,
steering-message and durable-turn origin references, subscription delivery
identities and retirement dependencies. Preserve populated IDs, payloads,
acceptance/continuation/delivery state and supported deletion behavior.

Thread the small typed metadata through existing event presentation, generated
wire types, decoders and event cards. Keep shared-read redaction, historical and
pending read paths, unavailable fallback and source navigation unchanged. Do
not manually edit generated exports or alter provider diagnostic originals.

## Consequences

- **Positive:** One causal notification owner prevents duplicate report packets
  without losing genuine execution occurrences. Saved evidence is stable across
  later goal changes, continuation and supported source deletion.
- **Negative:** Relevant capture/append storage errors fail their source
  transaction. Standalone nonboundary-report and remote goal-change notification
  coverage are explicitly deferred; this is not a complete goal-watch feature.
- **Neutral:** Source history/model changes require a coordinated forward schema
  change and preservation tests, not edits to imported SQL. Migration ordinal
  reservation does not itself satisfy registry, DDL, preservation or execution
  gates.
- **Boundary:** No startup installation, new worker, control recovery promise,
  resource expansion, activation or automatic follow-through is authorized.

## References

- [Goal requirements](../conversation-goals/requirements.md), REQ-GOAL-011–014.
- [Coordinator requirements](../global-recall/requirements.md), REQ-GR-015.
- [Event presentation](../conversation-events/requirements.md), REQ-CEVT-006.
- [ADR-070](070_trusted-input-provenance-and-global-watches.md): existing watch lane.
- [ADR-182](182_immutable-conversation-goals-use-owner-version-fences.md): immutable goals and no objective revisions.
- [ADR-183](183_owner-goal-controls-reference-accepted-command-receipts.md): independent command receipts.
- [ADR-185](185_goal-control-installation-is-an-explicit-owner-policy.md): explicit owner policy for local goal-control installation.
- [ADR-180](180_federated-subscriptions-carry-bounded-terminal-facts.md) and [ADR-181](181_federated-subscriptions-end-with-their-source.md): unchanged federation boundary.
