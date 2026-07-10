# ADR-007: Conversation creation uses fenced reconciliation

- **Status:** Accepted
- **Date:** 2026-07-09
- **Affects:** REQ-CCR-002, REQ-CCR-003, REQ-CCR-004, REQ-CCR-005, REQ-CCR-007, REQ-CCR-008, REQ-CCR-010; `CreationJob`, `ResourceReservation`

## Context

Conversation creation crosses a transactional database and non-transactional filesystem, Git, and runtime effects. A process may stop after an external effect succeeds but before recording its result. Multiple workers may also observe the same durable job. A stale-time threshold alone cannot distinguish a dead worker from a slow live worker and cannot prevent a late result or cleanup from overwriting newer work.

Users also need an immediate escape from provisioning without discarding the durable ownership information needed to clean external resources safely.

## Options considered

1. **Fenced claim, inspect, then resume** — each worker receives a leased generation; replacement workers reconcile durable reservations with observed reality before continuing. Cancellation revokes authority immediately while cleanup remains durable.
2. **Restart-gated recovery** — expired work remains recovery-required until a process restart or operator action. This reduces automatic concurrency but leaves jobs visibly stuck and does not itself solve late-worker fencing.
3. **Immediate replay after timeout** — a replacement repeats the external operation as soon as the previous claim appears stale. This recovers quickly but assumes external operations are safely idempotent and risks concurrent Git mutation.

## Decision

Conversation creation uses fenced claims with monotonically increasing generations and opaque claim tokens. Every authoritative update requires the current claim. Lease expiry revokes database authority but does not imply that an external effect failed; a replacement obtains repository mutation authority, inspects reservations and external state, then adopts, conflicts, cleans, or resumes.

Transient failures receive at most four total attempts with durable delays of 2 seconds, 10 seconds, and 30 seconds. Permanent failures do not retry. Claim loss is not a user-visible creation failure.

Cancellation and deletion revoke the active generation immediately. Cancellation preserves a visible cancelled record and its creation intent. Deletion hides the conversation but retains the same row as a cleanup tombstone until reconciliation permits physical deletion.

The protocol is verified through a deterministic discrete-event model with generated operation schedules and invariant checks after every operation, complemented by real SQLite and Git adapter tests.

## Consequences

- **Positive:** stale workers cannot complete, fail, or clean newer generations; ambiguous external success is recoverable; users always have an immediate escape; generated failures shrink to readable schedules.
- **Negative:** the job schema and worker become more explicit; external resources require durable reservations and reconciliation; cancellation may leave background cleanup work after the UI responds.
- **Neutral:** in-memory kicks and locks remain useful optimizations but are not correctness boundaries.

## References

- `specs/conversation-creation/requirements.md`
- `specs/conversation-creation/conversation-creation.allium`
- `decide_creation`
- `CreationProtocolState`
