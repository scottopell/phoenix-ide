# ADR-126: The standalone deployment controller owns durability and child supervision

- **Status:** Accepted
- **Date:** 2026-10-15
- **Affects:** REQ-SDC-001, REQ-SDC-003–012; `DeploymentAttempt`, `AttemptStore`, `ControllerSqliteStore`, `SupervisedChild`

## Context

A deployment controller must survive application replacement and must not depend on
the database whose migration it is coordinating. Phoenix's application SQLite
store therefore cannot be the controller's only durable authority. An atomic file
journal would avoid that dependency, but the attempt contains addressable child
collections—owned identities, discovery epochs, resource samples, and recovery
obligations—that require relational shape rather than a JSON aggregate.

macOS process mutation has a separate ownership problem. The kernel exposes
PID-based signals, not a pidfd-style operation that atomically binds a signal to a
PID-plus-birth identity. A fresh birth check followed by `kill(2)` retains a final
PID-reuse race. Direct children, however, remain unreaped children of their
controller and therefore retain their PID until the controller waits for them. A
controller-created process group can use that retained leader as stronger positive
ownership evidence, while still having explicit escape and controller-crash gaps.

## Options considered

1. **Phoenix application SQLite plus fresh-check `kill(2)`** — reuses existing
   infrastructure and can signal any observed PID, but couples controller recovery
   to the migrated database and overstates signal identity across the final syscall
   race.
2. **Atomic controller journal plus observation-only macOS adapter** — avoids the
   application database and arbitrary signals, but stores relational child
   collections as an ungoverned aggregate and cannot complete ordinary owned
   cleanup.
3. **Controller-owned normalized SQLite plus owned-child supervision** — gives the
   controller an independent relational authority and makes process mutation depend
   on an affine unreaped child/process-group owner. Arbitrary observed identities
   remain non-signalable.

## Decision

The standalone deployment controller owns a separate SQLite database. Its schema
normalizes attempt phases, owned identities, pending child scans, discovery epochs,
resource samples, and retained candidates. Application-schema migration neither
creates nor upgrades this database.

Checkpoint replacement uses a durable revision compare-and-swap so stale in-memory
states cannot erase newer evidence or replace a retained candidate. A crash load
returns affine recovery evidence rather than a live mutation-capable attempt, and an
unclassified persistence or load failure invalidates that database handle until the
controller reopens and classifies durable state.

On macOS, process mutation begins with controller-spawned direct-child supervision.
The supervisor creates a dedicated process group and retains the direct child
without reaping it until group cleanup and final wait. The general observation
adapter does not call `kill(2)` for an arbitrary PID-plus-birth identity. The
supervision API exposes, rather than hides, three remaining risks: descendants can
escape the group, same-session processes can deliberately join it, and controller
exit loses unreaped-parent authority. Durable recovery must treat lost supervision
as unresolved risk unless a surviving supervisor handoff is later specified.

## Consequences

- **Positive:** Deployment recovery remains independent of the database under
  migration, and relational constraints can make incomplete recovery obligations
  explicit.
- **Positive:** Arbitrary observed PIDs never acquire mutation authority merely
  because a birth check happened immediately before a signal.
- **Negative:** Phoenix now has another SQLite lifecycle, schema, corruption
  boundary, and backup/retirement concern.
- **Negative:** In-process supervision does not survive controller death. A durable
  supervisor handoff or helper process is required before crash-resilient mutation
  can be claimed.
- **Neutral:** Native observation and resource sampling can be implemented and
  qualified before the durable database and supervisor handoff are adopted by a
  deployment entry point.

## References

- ADR-019: runtime ownership requires positive evidence.
- ADR-034: compatibility guarantees are explicit and data-aware.
- ADR-039: durable runtime resource identity fails closed outside proven containment.
- ADR-095: resource policy uses sampled CPU history and demand-driven memory.
- `specs/standalone-deployment-controller/requirements.md`
- `specs/standalone-deployment-controller/standalone-deployment-controller.allium`
- `MacOsControllerOs::spawn_supervised`
- `SupervisedChild::signal_group`
