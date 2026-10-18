# ADR-135: Retained exact-root identity results use application-only recovery

- **Status:** Accepted
- **Date:** 2026-10-17
- **Affects:** REQ-SDC-018, REQ-SDC-022; completed-before-application native `ObserveIdentity`, controller schema v7, guardian protocol v9

## Context

ADR-134 admits one policy-issued exact-root identity observation, persists its typed
application before ACK, and permits ACK-only recovery after application. It deliberately
does not authorize recovery when the same live guardian completed and retained the
observation before the issuing runner durably applied it.

Repeating observation is unsafe: the process table may change between observations, and
the retained typed result is the fact correlated to the original durable issue. Rebuilding
a dispatch ticket or accepting a caller-selected PID or birth token would turn persisted or
wire diagnostics into authority.

## Options considered

1. **Repeat identity observation.** This discards the retained fact and can observe a later
   process state under the original issue key.
2. **Reconstruct the original dispatch authority.** This makes durable rows or DTOs capable
   of recreating an affine ticket.
3. **Verify retained custody and mint application-only authority.** Bind a fresh same-live
   reconciling session to the sole pending issue, exact issuing provenance, unchanged sole
   root slot, and original deadline, then apply only the retained typed result.

## Decision

Choose option 3.

The controller schema remains v7 because normalized identity issue, typed application, and
ACK-confirmation rows already represent the complete durable fact. The guardian protocol
advances from v8 to v9 and rejects v8 peers. The recovery verification request names only
the current session, dispatch key, and issuing-session provenance. The guardian derives the
PID-plus-birth target from the durable issue and unchanged sole owned-root slot and verifies
its retained completion without another native observation.

A fresh same-live runner may prepare recovery only from an exact `Interrupted` checkpoint,
a fresh `Reconciling` binding, and the sole `IssuedPendingApplication` identity issue before
the original deadline. The correlated guardian response mints a distinct affine
application-only capability. Neither the durable candidate nor the wire response alone can
apply policy.

Application acquires SQLite writer ownership, revalidates the candidate and deadline,
folds the retained typed observation into the interrupted ownership resume state, checks
the deadline again immediately before commit, and atomically writes the checkpoint and
existing normalized application row. It remains `Interrupted` and returns no executable
plan, dispatch, reconciliation activation, process mutation, cleanup continuation,
readiness, or ACK authority. After commit, the existing durable-application ACK path may
release guardian custody. A later crash uses ACK-only recovery and never reapplies.

## Consequences

- Completed-before-application and applied-before-ACK exact-root identity crash windows
  both preserve the original result and exactly-once durable application.
- Recovery does not reconstruct a policy loop or the original dispatch ticket and does not
  repeat identity observation.
- Caller-provided PID, birth token, label, socket path, or liveness remains non-authoritative.
- Schema v7 remains exact; protocol v9 is same-build pre-adoption only and rejects v8.
- Discovered-child identity acquisition, signal mutation, readiness continuation, guardian
  replacement, cross-boot recovery, installer integration, and production deployment remain
  outside this decision.

## Supersession

This decision refines ADR-134 only for retained completed-before-application exact-root
identity results. It does not broaden the admitted identity target or any mutation authority.

## References

- [ADR-130: Detach-and-fence](130_controller-loss-detaches-and-fences-surviving-guardian.md).
- [ADR-131: Same-build native guardian IPC](131_native-guardian-ipc-and-runner-are-same-build-pre-adoption.md).
- [ADR-132: Retained native application-only recovery](132_retained-native-results-use-application-only-recovery.md).
- [ADR-134: Exact-root identity observation](134_exact-root-identity-observation-is-distinct-native-authority.md).
- [Standalone controller requirements](../standalone-deployment-controller/requirements.md), REQ-SDC-018 and REQ-SDC-022.
- [Standalone controller behavior](../standalone-deployment-controller/standalone-deployment-controller.allium), `NativeGuardianPersistenceBoundary`.
- [Compatibility requirements](../compatibility/requirements.md), REQ-COMP-001 and REQ-COMP-005.
