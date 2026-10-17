# ADR-106: Pre-stop capacity cleanup is a cleanup-only recovery boundary

- **Status:** Accepted
- **Date:** 2026-10-16 (formalized from existing design)
- **Affects:** REQ-LDD-024

## Context

Prepared launchd activation reserves database capacity and writes bounded evidence before stopping the predecessor. Reservation creation, evidence publication, or removal can fail while the predecessor is still healthy and before any database snapshot or migration write exists.

That failure leaves transaction-owned files and an active deployment claim, but it does not create authority to inspect, stop, start, or replace the runtime or to restore database contents. Routing the condition through paired/current-schema recovery would give a pre-stop filesystem cleanup problem the runtime and database capabilities intended for post-disruption recovery. Conversely, terminalizing and releasing ownership before cleanup and its parent-directory durability are proven would admit overlapping mutation while owned files remain uncertain.

The recovery contract therefore needs to distinguish cleanup uncertainty from runtime/database recovery and bind retries to the retained transaction without broadening compatibility or rollback guarantees.

## Options considered

### 1. Route pre-stop cleanup failures through paired/current-schema recovery

This reuses existing operator commands and status plumbing, but grants unnecessary runtime/database actions before disruption and makes recovery mode selection ambiguous when no snapshot exists.

### 2. Treat cleanup as best-effort and release the claim

This keeps the implementation small, but can report terminal success while reservation/evidence files or directory metadata remain uncertain. A later deployment could overlap those unresolved artifacts.

### 3. Add an exact-owner cleanup-only recovery boundary

This introduces dedicated cleanup states and commands. It requires additional mode, claim, evidence-shape, and durability checks, but keeps runtime/database capabilities unavailable and preserves ownership until cleanup completion is durable.

## Decision

Use a dedicated cleanup-only recovery boundary for failures that occur before runtime disruption and before snapshot-proof publication.

A cleanup retry must bind the exact retained transaction, requested paired/current-schema mode, regular non-symlink manifest and status evidence, and matching active claim before mutation. It may remove only transaction-owned capacity reservations, capacity evidence, owned capacity/proof atomic-write temporaries, and SQLite reservation sidecars. It must not construct the runtime controller, inspect or mutate runtime state, or read/write database contents as recovery work.

Cleanup completion requires removal attempts plus parent-directory durability, terminal cleanup status, and durable exact-owner claim release. Cleanup, terminal-status, or claim-release uncertainty returns the transaction to cleanup-pending where persistence remains possible and retains or restores a visible ownership fence. Once snapshot proof or migration evidence exists, cleanup-only recovery is no longer admissible; the corresponding prepared recovery boundary remains authoritative.

This decision records the policy boundary. The normative state names, transitions, preconditions, and operator-visible behavior remain defined by REQ-LDD-024 and the launchd/production Allium specifications.

## Consequences

### Positive

- Pre-stop filesystem cleanup cannot accidentally stop/start the service or restore database contents.
- Ownership remains visible until cleanup and release are durably complete.
- Wrong-mode, wrong-owner, malformed-evidence, and post-snapshot retries fail before mutation.
- Cleanup behavior is independently testable without constructing the runtime backend.

### Negative

- Operators have additional cleanup-specific commands and status to understand.
- Controller and helper must duplicate the trust-boundary checks needed before helper handoff.
- Filesystem durability failures can leave a transaction intentionally fenced until a later exact-owner retry.

### Neutral

- This does not add downgrade, rollback, cross-version, or post-migration recovery guarantees.
- Runtime/database prepared recovery remains unchanged once disruption or snapshot proof exists.

## References

- [ADR-010: launchd deployment uses an independent transaction helper](010_launchd-deployment-uses-independent-transaction-helper.md)
- [ADR-034: Compatibility guarantees are explicit and data-aware](034_compatibility-guarantees-are-explicit-and-data-aware.md)
- [ADR-102: Current-schema prepared activation commits at the database-write boundary](102_current-schema-prepared-activation-commits-at-database-write.md)
- [ADR-105: Qualify migration 128 for bounded current-schema activation](105_qualify-migration-128-for-bounded-current-schema-activation.md)
- [`specs/launchd-deployment/requirements.md`](../launchd-deployment/requirements.md)
- [`specs/launchd-deployment/launchd-deployment.allium`](../launchd-deployment/launchd-deployment.allium)
- [`specs/production-deployment/production-deployment.allium`](../production-deployment/production-deployment.allium)
- `cmd_prod_cleanup_precondition`
- `cleanup_precondition_capacity`
- `release_durable_terminal_claim`
