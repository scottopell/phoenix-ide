# ADR-076: Committed paired finalization is publication-only

- **Status:** Accepted
- **Date:** 2026-10-05
- **Supersedes:** ADR-075 only for committed finalization and recovery-startup checkpoint
- **Affects:** specs/launchd-deployment (REQ-LDD-017, REQ-LDD-018), specs/production-deployment (REQ-PD-018, REQ-PD-020), specs/compatibility (REQ-COMP-003)

## Context

ADR-075 established prepared-artifact paired fail-closed database recovery.
A committed interruption can leave plist publication or temporary cleanup pending,
and a recovered predecessor may accept writes before terminal status is durable.
Manual-only blocked finalization and replaying original activation/recovery were
considered; the latter can discard writes or roll back an already committed runtime.

## Options considered

- Manual-only blocked committed finalization, requiring bespoke operator repair.
- Replaying activation/rollback, risking already committed runtime or later DB writes.
- Retained proof-bound publication/temporary-cleanup-only finalization.

## Decision

Global commissioned a bounded finalization-only resumer as delegated engineering
scope for safe deployment/RC delivery; this is not a fabricated detailed user
approval and does not authorize a live production operation. The alternative was
manual-only blocked finalization after a committed interruption. The decision is
`prod finalize-paired TXN`: verify retained byte-bound transaction/helper, matching
claim, absent activation helper, mutual exclusion, and exact running committed
candidate/binary/private loaded configuration; retry only captured plist publication
and temporary restore-reservation cleanup. No runtime stop/bootstrap/restart,
database access, old-version restore, ambient configuration, or generalized replay.
Pending survives interruption; verified completion releases owned claim and a
completed rerun is idempotent. REQ-LDD-018 / REQ-PD-020 own this narrow boundary.

Recovered predecessor startup also records a durable checkpoint and typed outcome;
a retry verifies/finalizes a running predecessor without replaying an old snapshot
and discarding later writes. Unknown/stopped post-checkpoint state remains fenced.

## Consequences

The finalization command is intentionally not a recovery platform. Unknown identity,
configuration, claim or helper absence preserves the fence. A post-start checkpoint
permits running-predecessor verification/finalization only, never snapshot replay;
unknown or stopped predecessor requires operator investigation. No live action or
release publication is authorized by this engineering decision.

## References

- [ADR-075](075_prepared-artifact-paired-launchd-upgrade-fails-closed.md)
- `specs/launchd-deployment/requirements.md` REQ-LDD-017 and REQ-LDD-018
- `specs/production-deployment/requirements.md` REQ-PD-018 and REQ-PD-020
- `specs/compatibility/requirements.md` REQ-COMP-003
