# ADR-036: Local SQLite authority loss fails stop

- **Status:** Accepted
- **Date:** 2026-08-16
- **Affects:** REQ-DWF-043, REQ-DWF-CHAT-013, REQ-BED-033

## Context

Phoenix uses one bundled SQLite database as the durable local authority for
conversation and workflow facts. Process-local workers, runtime objects, provider
tasks, SSE connections, replay buffers, timers, kicks, and queued events can
project or advance that authority, but none survives arbitrary process loss.

An authoritative local SQLite command can fail without delivering the typed result
its execution boundary needs. One exact query against the rows that own the fact
may still establish a command-scoped typed result. If both the command and that
query fail to establish the durable fact, continuing in the same process would
require an assumption about SQLite plus coordinated repair across independent
in-memory owners.

Two closed implementation attempts exposed that coordination cost. PR #683
explored authoritative direct-turn materialization while preserving the live
stream, and PR #687 explored abandoning one runtime and SSE incarnation before
reconnect and resnapshot. Review of those approaches found dependencies across
effect ordering, worker readiness, runtime creation and publication, provider
startup, broadcaster and reserved-cursor ownership, queued events, cleanup, and
reconnect initialization. They are historical evidence for this decision, not
implementation branches to continue.

## Options considered

1. **Persist local epistemic uncertainty and repair it in process.** This could
   preserve more live continuity, but it would make inability to establish local
   authority a conversation or workflow state and introduce parallel facts about
   whether the SQLite command committed.
2. **Abandon and replace one runtime or SSE incarnation in process.** This avoids
   a semantic uncertainty state, but exact replacement still coordinates several
   independently owned live resources and cleanup paths.
3. **Treat the local SQLite outcome like an ambiguous external side effect.** This
   would reuse reconciliation or manual-resolution concepts, but a same-process
   SQLite transaction is the local authority boundary, not independently executing
   remote work.
4. **Fail stop and reconstruct from SQLite.** Use one exact authority query after
   an untyped command failure; if it cannot establish the durable fact, stop the
   process and let another process reconstruct from the database.

## Decision

Choose option 4.

The owning command boundary returns a closed result that structurally separates an
established durable fact carrying its typed domain result from an unclassified
durable fact. An ordinary database or library error is not evidence of commit or
non-commit. After an unclassified result, the owning execution boundary may issue
one exact query against the rows that own the fact needed to continue. Phoenix
follows an established typed domain result. If the query is unavailable or remains
unclassified, Phoenix stops admission and semantic publication, avoids cleanup
that depends on the suspect persistence path, attempts only bounded best-effort
shutdown work, and terminates nonzero or aborts when the bound expires.

While Phoenix is serving, a task that owns this local SQLite authority boundary
cannot disappear without a typed result. Panic, unexpected exit, or cancellation
at that boundary selects the same fail-stop outcome unless the one exact query
against the owning authoritative rows establishes the durable fact. A typed
coordinated-shutdown disposition is not fatal. This is not a general
task-supervision policy; ordinary task failures remain feature-owned.

Inability to establish local persistence authority is not conversation or workflow
semantic state. Runtime and observer identity and continuity are sacrificed across
the fail-stop boundary. Another process reconstructs from committed SQLite facts
and durable time when that database can again establish authority.

A same-process SQLite transaction is not modeled as a distributed external side
effect. Genuine ambiguous outcomes in providers, GitHub, remote tools, and other
services remain governed by their feature-owned normative recovery contract.

A direct turn may privately propose semantic state, but the runtime must not expose
that state as its adopted committed state or publish it, and routing and admission
must not treat it as committed, before the transaction owning direct-turn
materialization commits. The implementation must preserve this authority ordering
without requiring one particular runtime representation.

## Consequences

- **Positive:** SQLite remains the single authority for local durable facts; no
  conversation or workflow state encodes uncertainty about its own transaction.
- **Positive:** Correctness does not depend on preserving or coordinating a live
  runtime, SSE stream, replay buffer, timer, task, or queued event across local
  persistence-authority loss.
- **Positive:** External ambiguity retains the recovery policy selected by the
  feature that owns the external effect.
- **Negative:** One unclassifiable local persistence result terminates the process
  within a bounded shutdown path and interrupts every process-local runtime and
  observer connection.
- **Negative:** Authority-boundary tasks and direct-turn publication ordering need
  structural implementation enforcement rather than call-site convention.
- **Neutral:** This decision does not itself change SQLite write frequency or add
  live SQLite replacement or rollback guarantees.

## References

- ADR-014: workflow CAS and effect authority
- ADR-020: one scheduler authority and durable acknowledgement
- ADR-024: direct-turn authority is partitioned by semantic fact
- ADR-034: compatibility guarantees are explicit and data-aware
- `specs/durable-workflows/requirements.md`
- `specs/bedrock/requirements.md`
- `ConversationRuntime::apply_transition_result`
- `ConversationRuntime::persist_state_effect`
- `WorkflowRepository::materialize_authoritative_turn`
