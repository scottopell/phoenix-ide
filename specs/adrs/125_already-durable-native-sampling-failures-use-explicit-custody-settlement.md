# ADR-125: Already-durable native sampling failures use explicit custody settlement

- **Status:** Accepted
- **Date:** 2026-10-17
- **Affects:** REQ-SDC-018–020; native sampling failure custody, controller schema v8, guardian protocol v10

## Context

The native guardian retains every completed result until the controller proves durable
application or explicit settlement. Successful samples have normalized application and ACK
evidence. A typed sampling failure is also durably applied, but remains `ParkedRetained`:
policy does not advance and the guardian intentionally keeps the exact failed result.

The existing schema cannot distinguish “failure recorded for diagnosis” from “a later
fresh session explicitly settled that result so its custody may be released.” Reusing the
successful-sample ACK row would falsely represent policy application and could make a
failed round appear settled for reconciliation. Successful terminal samples are different:
applying one would enter ownership or `ResourceBlocked`, so they cannot be settled without
choosing and persisting those policy transitions.

## Options considered

1. **ACK every durable failure application.** This conflates parking with deliberate
   settlement and erases the recovery decision.
2. **Treat failure settlement as successful sample application.** This fabricates resource
   evidence and can incorrectly authorize continuation.
3. **Add a distinct, normalized custody-settlement lifecycle for already-durable failures.**
   Preserve the interrupted sampling obligation while recording and ACKing only the exact
   retained failure.

## Decision

Choose option 3.

Controller schema v8 adds immutable `native_sampling_failure_settlements` rows referencing
the exact failed application and separate
`native_sampling_failure_ack_confirmations`. The failure payload remains solely in the
application row; settlement does not duplicate or rewrite it. Exact schema-v7 stores are
rejected unchanged. Same-build guardian protocol v10 rejects v9 peers.

Only the sole exact `ParkedRetained` sampling failure is eligible. A fresh same-live
`Reconciling` session must prove the interrupted sampling obligation, exact issue and
sequence, failed application and applying revision, issuing-session provenance retained by
the guardian, unchanged guardian identity and slot inventory, and original deadline. The
wire verification request carries only session, key, and issuing provenance; it does not
accept a caller-selected failure, process identity, or effect as authority.

Settlement acquires SQLite writer ownership, revalidates the complete candidate and
deadline, advances only the interrupted checkpoint revision, inserts the settlement, checks
the deadline again immediately before commit, and returns no transition, effect, dispatch,
activation, mutation, continuation, readiness, or ACK capability. Policy, sampling
obligation, owned tree, binding, slots, and deadline remain unchanged. A settled or
confirmed failure never satisfies the successful-sample reconciliation barrier.

After commit, the guardian independently rereads and matches the exact settlement, retained
typed failure, provenance, same-live reconciling session, and deadline before dropping only
that result. Immediate ACK uses the settling session and its successor settlement revision.
A crash or lost response uses ACK-only replay in a later fresh interrupted reconciling
session whose interruption revision includes the settlement. Confirmation is revision
neutral and distinct from successful-sample ACK evidence.

## Consequences

- Exactly one already-durable failed result can release custody without native reexecution.
- Settlement preserves `Interrupted` and does not authorize retry or continuation.
- Failed results without durable application and successful terminal samples remain retained.
- Child-list failure settlement, effect retry, resource admission, child adoption, signals,
  cleanup, production integration, guardian replacement, and cross-boot recovery remain
  outside this decision.

## References

- [ADR-119: Detach-and-fence](119_controller-loss-detaches-and-fences-surviving-guardian.md).
- [ADR-120: Same-build native guardian IPC](120_native-guardian-ipc-and-runner-are-same-build-pre-adoption.md).
- [ADR-121: Retained native application-only recovery](121_retained-native-results-use-application-only-recovery.md).
- [ADR-124: Retained exact-root identity application-only recovery](124_retained-exact-root-identity-results-use-application-only-recovery.md).
- [Standalone controller requirements](../standalone-deployment-controller/requirements.md), REQ-SDC-018–020.
- [Standalone controller behavior](../standalone-deployment-controller/standalone-deployment-controller.allium), `NativeGuardianPersistenceBoundary`.
- [Compatibility requirements](../compatibility/requirements.md), REQ-COMP-001 and REQ-COMP-005.
