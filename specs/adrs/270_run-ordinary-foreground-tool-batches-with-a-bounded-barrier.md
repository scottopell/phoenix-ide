# ADR-270: Run ordinary foreground tool batches with a bounded barrier

- **Status:** Accepted
- **Date:** 2026-11-17
- **Affects:** REQ-BED-003, REQ-BED-004, REQ-BED-005, REQ-BED-005a, REQ-BED-007, REQ-PERM-004, REQ-SA-005

## Context

The runtime preserves native provider multi-call responses but executes one ordinary call at a time. Independent searches, Bash commands and patches therefore pay serial latency. Existing native admission, approval, question, goal and child-terminal operations carry stronger lifecycle contracts that cannot be placed in an arbitrary task pool. The user selects model-declared independence and a foreground result barrier, without conflict inference or background execution.

## Options considered

1. Retain universal serial execution. This preserves simple ownership but leaves independent-call latency unchanged.
2. Add a durable scheduling graph with conflict detection, dependency hints and recovery/replay. This creates unsupported policy and external-effect guarantees without demonstrated need.
3. Bound ordinary foreground owners while retaining one core unresolved/result collection and the existing complete-round checkpoint. Lifecycle-containing responses remain serial.

## Decision

Choose the third option. Admit at most four ordinary preparation/execution owners per conversation, each with exact original assistant/call identity, its gate-cleared payload and its own resources and one-shot outcome. Accept completed siblings immediately into core state, then assemble the complete round in original call order before one model continuation. The process-local owners represent resources and unaccepted receipts, not another semantic pending queue.

Cancel prevents refill, interrupts read work and signals every ordinary invocation. Ready typed outcomes remain owned through their individual commit. Parent and child paths share the aggregate drain barrier while retaining their different final destinations. Per-call wait cutoffs and the aggregate cancellation deadline do not renew on sibling progress. Actual unknown persistence authority still closes the process fence.

A bounded cancellation inbox retains one affine delivery and its acknowledgement outside dispatch-mode replacement. While a state, complete-round checkpoint or cancellation-evidence write is issued, the same pinned command retains its admission and settlement guard; cancellation can still signal resources and service the original drain cutoff. Deadline service snapshots typed receipts before task-only wait retirement or affirmative forced retirement, without recursively changing core state or guessing the write outcome. The safe actor boundary later consumes those proofs and applies cancellation before ordinary dispatch. Uncommitted inline steering entries return to their existing queue so the normal post-cancellation drain delivers them once. This adds no database latency bound or durable control queue.

Preserve native provider protocols, including the Codex Lite multi-emission limitation. The executor consumes native call lists and does not introduce a wrapper language. Existing startup interruption materialization orders accepted and synthetic results by their source identity; it does not re-execute unresolved tools or establish a new recovery guarantee.

## Consequences

- Independent ordinary calls overlap while the model still receives one complete paired result round.
- The model is responsible for choosing independent operations; concurrent Bash or patch calls can conflict, and Phoenix adds no locks or dependency analysis.
- Resource ownership, cancellation, UI activity and timeout logic must handle several exact siblings rather than a single task slot.
- Mixed lifecycle responses remain serial, and native Codex Lite can still decline to emit multiple native calls.
- No new durable queue, external retry, background task feature or exactly-once effect guarantee is introduced.

## References

- `specs/bedrock/requirements.md` and `specs/bedrock/tool-dispatch.allium`
- `ToolRoundAssembly`, `DispatchSelection`, `normalize_in_flight_round`
- `specs/permissions/requirements.md` and `specs/subagents/requirements.md`
