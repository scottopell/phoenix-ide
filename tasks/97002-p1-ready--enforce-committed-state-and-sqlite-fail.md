# Enforce committed direct-turn publication and local SQLite fail-stop

## Problem

The direct-turn reducer proposes `LlmRequesting` before authoritative materialization commits. `ConversationRuntime::apply_transition_result` installs that proposed state in `self.state` and can publish it through the live state watcher before `WorkflowRepository::materialize_authoritative_turn` commits the accepted turn, reducer projection, and canonical message. Live routing and admission consult that watcher, so a proposed semantic state can become process-local authority before its owning SQLite transaction commits.

Phoenix also lacks the narrow process fail-stop boundary required when an authoritative local SQLite command returns no typed result and one exact query against the owning authoritative rows cannot establish the durable fact needed to continue. A panic, unexpected exit, or cancellation of the task owning that exact local SQLite authority boundary has the same gap when no typed result was delivered.

This task implements `REQ-DWF-043`, the strengthened `REQ-DWF-CHAT-013`, `REQ-BED-033`, and ADR-036. It must not introduce local ambiguity `ConvState`, persisted SSE events, a generic outbox, automatic rollback, live SQLite replacement, or general Tokio task supervision.

## Serial deliverables

### 1. Commit-before-publication authority ordering

- Proposed direct-turn transition state remains private and structurally distinct from committed observer/routing/admission authority until authoritative materialization commits.
- No provisional semantic state is published or exposed as adopted committed state to observers, routing, or admission.
- A failed, stale, replayed, or otherwise non-fresh materialization cannot publish the proposed state or make it authoritative; its exact command-scoped typed result governs behavior.
- Successful materialization commits the accepted turn and reducer projection before exposing or publishing the committed semantic state.
- Deterministic tests cover successful commit, pre-commit observation, and every non-fresh or failed materialization result without sleeps.

### 2. One top-level persistence fail-stop signal

- An authoritative local SQLite command that returns no typed result permits at most one exact classification query against the rows owning the required fact.
- Failure to establish that fact emits one top-level fail-stop signal consumed by the process boundary; it does not create conversation/workflow state or competing runtime-local recovery paths.
- The process boundary stops admission and semantic publication, avoids database-backed cleanup through the suspect persistence path, and exits nonzero.
- Deterministic tests distinguish a successfully classified command result from a failed or inconclusive classification.

### 3. Narrow critical-task supervision

- Panic, unexpected exit, or cancellation escalates only for the task owning this exact local SQLite authority boundary and only when no typed result or exact authoritative-row classification was delivered.
- Ordinary non-authoritative task failure and genuine external ambiguity retain their feature-owned behavior.
- Deterministic tests cover boundary-owner disappearance without broadening supervision to general Tokio tasks.

### 4. Crash and restart verification

- Crash/restart tests prove that another process reconstructs unfinished obligations and committed conversation/workflow authority from the same SQLite database without process-local runtime, observer, timer, queue, or connection identity.
- Tests prove provisional semantic state is absent after restart and that an unavailable authority database does not permit continued processing.
