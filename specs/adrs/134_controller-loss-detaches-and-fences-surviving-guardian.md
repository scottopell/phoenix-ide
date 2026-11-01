# ADR-134: Controller loss detaches and fences the surviving guardian

- **Status:** Accepted
- **Date:** 2026-10-15
- **Affects:** REQ-SDC-008, REQ-SDC-012; `GuardianBinding`, `SurvivingGuardianBoundary`

## Context

Owned-child mutation authority is process-local. A restartable policy controller
cannot reconstruct direct-child or reaping authority from SQLite, a PID-plus-birth
observation, a process-group identifier, a launchd label, or a socket pathname. A
separate launchd-owned guardian can survive policy-controller exit only when it was
the workload parent from the initial spawn and retains the unreaped child and its
original group.

Controller loss still requires a policy for already-started work. Automatically
terminating children after a heartbeat or socket failure would grant cleanup
authority from an unreliable liveness signal. Allowing the guardian to continue
policy steps would create a second policy authority outside the controller database.

## Decision

A transaction-scoped launchd-owned guardian is the stable native parent. The policy
controller may restart behind it. When the controller exits, the guardian detaches
and fences the ended controller session:

- already-started child work may continue;
- native child ownership and completed results remain retained;
- no new step starts and no new signal is sent;
- socket loss or heartbeat expiry does not trigger cancellation.

A replacement controller may resume only after positively identifying the same live
guardian instance and its retained child slots, then committing interruption and the
complete fresh-reconciliation barrier through the controller-owned SQLite authority.
The original absolute deadline and any forward-only migration boundary remain
unchanged.

Guardian death, replacement, an unprovable handoff, or deadline expiry preserves
unresolved supervision risk. A new guardian never adopts historical PIDs or process
groups. Administrative closeout remains distinct from verified cleanup and grants no
implicit signal.

The guardian is an effect custodian, not a second policy engine. It validates
committed, attempt-bound operation identities; retains accepted results until durable
acknowledgement; and refuses stale controller sessions. It does not choose retries,
escalation, rollback, migration compatibility, or claim release.

## Options considered

### Preauthorized cancellation on controller loss

Rejected for the initial guarantee. It would require every signal sequence, trigger,
grace interval, affected group, and forward-only restriction to be durably authorized
before spawn. A heartbeat remains insufficient evidence even with such a policy.

### Reconstruct supervision from persisted process identity

Rejected. Persisted identity is observation evidence, not retained parent or reaping
authority, and cannot close the final observation-to-signal race.

### Let the guardian continue deployment policy autonomously

Rejected. This duplicates policy authority and can diverge from the normalized
controller checkpoint after an ambiguous persistence or communication failure.

### Treat a replacement launchd job as the same guardian

Rejected. Matching labels, paths, protocol versions, or executable bytes do not prove
continuity of the process that retains the child.

## Consequences

- The guardian must be the direct parent from the first workload spawn; parenthood is
  not transferred during controller restart.
- Guardian instance identity, retained child slots, controller-session fencing, and
  unacknowledged effect results require normalized controller-store representation
  before native integration.
- Harmless qualification must kill and reconnect controllers while proving the same
  guardian survives, and must separately prove guardian loss fails stop.
- There is no guarantee across guardian death, logout, reboot, or cross-boot deadline
  interpretation.
- Production adoption remains blocked until the guardian protocol and disposable
  controller-death qualification are implemented.


## References

- [ADR-133](133_standalone-controller-owns-durability-and-supervision.md)
- [Standalone deployment controller requirements](../standalone-deployment-controller/requirements.md)
- [Standalone deployment controller behavior](../standalone-deployment-controller/standalone-deployment-controller.allium)
- [Standalone deployment controller status](../standalone-deployment-controller/executive.md)
