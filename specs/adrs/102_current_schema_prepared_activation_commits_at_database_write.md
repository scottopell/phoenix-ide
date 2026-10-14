# ADR-102: Current-schema prepared activation commits at the database-write boundary

**Status:** Accepted
**Date:** 2026-10-14

## Context

The prepared-artifact launchd path was deliberately restricted to legacy ProductConversation upgrades at migration 69 or earlier. Production databases at migration 123 contain ProductConversation tables and are correctly rejected by that eligibility gate, even when the signed candidate is the intended next production binary.

The candidate applies migrations independently. Migration 126 retires wake behavior while retaining history, and migration 127 rebuilds coordinator-owned work-scope authority. The binary's existing `--migrate-only` operation also performs startup reconciliation. A migration process can therefore commit a proper prefix or reconciliation writes before a later failure. Replaying an older snapshot after those writes, or starting the predecessor against the migrated database, would create an unowned downgrade and data-loss guarantee.

## Decision

Add a distinct explicit current-schema prepared-artifact mode. Do not relax or reuse paired legacy eligibility.

The controller binds the candidate's existing signed-artifact receipt and identity evidence together with the retained helper and an exact candidate migration registry. The qualified interval is migration 123 through the exact migration-127 registry. Unknown, mismatched, missing, or newer ledgers and registries fail closed.

Before disruption, the helper verifies exact ledger, ProductConversation tables, SQLite integrity and foreign keys, materializes candidate installation overhead, physically reserves backup and restore capacity, and requires 20 GiB to remain free. After stop and exclusive-ownership proof, it creates a private SQLite snapshot whose hash, logical contents, source/target ledgers, candidate, predecessor, helper, controller, and transaction identity are durably bound.

The exact installed candidate runs `--migrate-only` while the database remains offline. Automatic predecessor rollback ends before that subprocess is invoked: after invocation, a failed or interrupted process cannot prove that no migration or reconciliation write committed, even when the highest ledger and logical values appear unchanged. Snapshot replay and predecessor startup are therefore forbidden after invocation. A complete target ledger may proceed to exact-candidate start and persistent publication; a partial or ambiguous outcome remains fenced for candidate repair.

Current-schema finalization is a separate explicit command. It re-verifies the committed target ledger and proof before starting only the exact retained compatible candidate, never reruns offline migration, and never restores the snapshot. Normal candidate startup retains ownership of its idempotent initialization and reconciliation. Paired legacy finalization remains publication-only and cannot bootstrap a runtime.

## Consequences

- Schema 123 production databases gain an ordinary, explicit prepared-artifact path without weakening legacy pairing or supply-chain checks.
- Automatic rollback is narrower than process failure: it exists only before offline candidate migration is invoked and only for the matching predecessor pair.
- Migrations 126 and 127 are treated as irreversible for deployment recovery even though a pre-migration audit backup remains retained.
- A crash or failure after committed migration writes can require operator-qualified compatible-candidate repair; availability is sacrificed rather than guessing across a compatibility boundary.
- The guarantee is intentionally limited to the reviewed 123-to-127 transition and must be amended for future schema intervals.

## References

- `specs/compatibility/requirements.md` REQ-COMP-012
- `specs/launchd-deployment/requirements.md` REQ-LDD-019 through REQ-LDD-022
- `specs/production-deployment/requirements.md` REQ-PD-021
- ADR-076
