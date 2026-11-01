# ADR-114: Admit the exact target ledger for prepared current-schema activation

- **Status:** Accepted
- **Date:** 2026-10-16
- **Affects:** REQ-COMP-012, REQ-LDD-019–022, REQ-PD-021
- **Supersedes:** ADR-105 only where it requires the prepared source ledger to be exactly migration 123

## Context

The prepared current-schema transaction qualified by ADR-105 admits the exact migration-123 source ledger and migrates it to the exact migration-128 candidate registry. A later prepared artifact can target that same exact registry while production already records the complete migration-128 ledger. Rejecting that source prevents the protected prepared transaction from replacing the runtime even though the database ledger already equals the candidate registry.

Ledger equality does not make the replacement an ordinary runtime-only install. The candidate is a different signed binary, its offline `--migrate-only` operation owns candidate-specific reconciliation, and startup may write. Skipping the snapshot or migration-invocation boundary would create an unreviewed cross-binary rollback guarantee.

## Options considered

1. **Require the migration-123 source for every prepared artifact.** Preserves the prior gate but makes a later exact-registry replacement impossible through the prepared transaction.
2. **Add a migration-skipping same-schema route.** Avoids offline invocation but creates a second recovery lifecycle and assumes that ledger equality proves no candidate-owned reconciliation writes are needed.
3. **Admit the exact candidate registry as a second source while retaining the existing lifecycle.** Reuses the exact registry, capacity, snapshot, invocation, verification, and forward-recovery boundaries without adding a generic compatibility promise.

## Decision

Choose option 3. This decision partially supersedes ADR-105 only where ADR-105 requires the prepared source ledger to be exactly migration 123. Prepared current-schema activation admits exactly either the complete qualified migration-123 baseline or the complete qualified migration-128 candidate registry. Any partial, differently named, missing, unknown, or newer ledger fails closed. ADR-105's exact target registry and ADR-102's snapshot, invocation, and recovery boundary remain binding. ADR-103 and the ordinary 123-to-128 route remain unchanged.

Both admitted sources traverse the same protected transaction. The helper reserves backup and restore capacity, proves the free-space floor and evidence budget, stops the predecessor, proves exclusive database ownership, creates and verifies the SQLite snapshot, installs the exact candidate, durably records `migration_started`, and invokes the candidate's bounded offline `--migrate-only` operation. This invocation is required when source and target ledgers are equal because candidate-owned reconciliation may write.

Before invocation, recovery compares the database against the transaction-bound source ledger and logical contents, whether migration 123 or 128. Invocation remains the irreversible forward boundary: afterward the snapshot is audit evidence only, the predecessor cannot restart, and finalization may start only the retained exact candidate after exact target-ledger verification.

This decision qualifies one prepared deployment admission. It does not establish generic same-version or cross-binary compatibility, downgrade support, mixed-version operation, live database replacement, compatibility for another baseline or future registry, or any expansion of ordinary in-place deployment.

## Consequences

- **Positive:** A later signed candidate targeting the already-installed exact registry can use the same capacity, snapshot, identity, and forward-recovery protections as the original migration.
- **Negative:** Even an equal-ledger replacement incurs the full snapshot and offline candidate invocation, and a post-invocation failure remains forward-only.
- **Neutral:** ADR-105's exact 123-to-128 ordinary-deployment interval and ADR-103's ordinary launch boundary remain unchanged.

## References

- [ADR-102](102_current-schema-prepared-activation-commits-at-database-write.md): snapshot and invocation boundary retained.
- [ADR-105](105_qualify-migration-128-for-bounded-current-schema-activation.md): exact candidate registry retained; prepared source admission extended.
- [ADR-034](034_compatibility-guarantees-are-explicit-and-data-aware.md): bounded compatibility policy.
- [Launchd deployment requirements](../launchd-deployment/requirements.md): REQ-LDD-019–022.
- [Production deployment requirements](../production-deployment/requirements.md): REQ-PD-021.
- `validate_current_schema_database`, `_snapshot_proof`, `run_current_schema_migration`.
