# ADR-075: Active direct turn is the restart baton

- **Status:** Accepted
- **Date:** 2026-10-03
- **Affects:** REQ-BED-007, REQ-DWF-CHAT-014, REQ-DWF-CHAT-016

## Context

Phoenix already persisted separate authorities for accepted direct turns, steering, continuation, approval, creation, and wakes. Startup workers recovered most of them independently. One gap remained after an interrupted tool round: startup safely materialized unknown tool effects as explicit error results and reset the conversation to idle, but the next model step was only inferred when a client later opened the conversation. Transcript inference alone could not safely authorize background work because prose can promise follow-through and old tool rows can belong to settled turns.

## Options considered

- Treat every unfinished-looking transcript as work: rejected because prose and stale history are not execution authority.
- Add a restart queue or general scheduler: rejected because it duplicates existing workflow owners and expands concurrency policy.
- Add a new baton table: rejected because the active materialized direct turn already supplies exact accepted identity, generation fencing, cancellation, and terminal settlement.
- Use the active materialized direct turn as the baton and classify its persisted transcript projection: chosen.

## Decision

Startup discovers idle conversations still owned by an active materialized direct turn. That exact accepted-turn identity and generation are the sole authority for restart recovery. A complete persisted tool-result round may dispatch one next model step without client activity. Unknown or running tools are first represented by typed terminal evidence or explicit interrupted results and are never replayed from transcript reconstruction.

Existing typed owners remain authoritative for queued input, committed steering, approvals, continuation, creation, cancellation, and wakes. Continuation transfers the single outstanding obligation through its durable operation rather than creating concurrent predecessor and successor owners. A transcript or prompt without an active durable owner creates no work.

Restart attempts remain bounded by durable restart markers scoped to the same user turn. Exhaustion atomically fails the exact accepted turn, advances its generation, releases conversation ownership, and persists an explicit error.

## Consequences

No database migration, scheduler rewrite, default concurrency cap, or second queue is introduced. Ordinary and Coordinator conversations use the same startup path. Restart discovery becomes proactive for the missing post-tool model step while ordinary interrupted provider requests without recoverable effect evidence retain idle settlement. Deterministic tests must cover owner presence and absence, unknown-effect materialization, bounded repeated crashes, Coordinator recovery, and the existing typed-owner startup paths.
