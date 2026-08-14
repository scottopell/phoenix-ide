# ADR-034: Compatibility guarantees are explicit and data-aware

- **Status:** Accepted
- **Date:** 2026-08-14
- **Affects:** REQ-COMP-001 through REQ-COMP-005

## Context

Phoenix has forward SQLite migrations and production deployment rollback, but it lacks one explicit authority for compatibility guarantees. Defensive implementation work can therefore turn unusual scenarios into permanent promises without a product decision. Examples include requiring an old binary to open a future schema, detecting replacement of an SQLite file beneath live connection pools, and validating one exact textual spelling of an internal timestamp.

These choices look local but create project-wide commitments. Cross-version database access constrains every migration. Live database replacement requires resource identity and fencing across operations. Text timestamps in SQLite require the application and schema to share a date parser and formatting contract.

Production rollback exposes a related gap. Restoring only the previous binary and configuration is unsafe after a candidate may have migrated data, because the previous binary is not guaranteed to understand the newer schema.

## Options considered

1. **Preserve every behavior that defensive code can support** — maximizes best-effort compatibility, but turns implementation accidents into permanent guarantees and distributes their cost across migrations, deployment, persistence, and testing.
2. **Make guarantees explicit and recover matching runtime/data pairs** — support forward migration, restore a matching database with a rolled-back runtime, require offline database replacement, and use one simple integer representation for Phoenix-owned SQLite timestamps.
3. **Remove rollback and compatibility behavior entirely** — minimizes machinery, but leaves routine upgrades without a safe recovery contract and provides too little operational support.

## Decision

Choose option 2.

A compatibility, downgrade, rollback, recovery, or live-resource-replacement behavior is guaranteed only by a normative requirement. Missing guarantees are unsupported rather than best-effort obligations.

Phoenix guarantees ordered forward migration when every recorded migration version exists in the binary's embedded migration set. Pending migrations are determined by individual version membership, allowing intentional ledger gaps; Phoenix does not guarantee backward access by an older binary to migrations it does not contain.

Restoring a previous binary without its matching database is runtime-artifact rollback, not database-compatible rollback. Phoenix reports database-compatible rollback only when the matching runtime and database are restored and verified as one recovery unit. This policy does not choose or prescribe a future snapshot mechanism.

One backend-managed Phoenix runtime version exclusively owns a production database. Database restore and replacement occur only while that runtime is stopped; mixed-version or independently launched Phoenix processes sharing the database are unsupported. Phoenix does not add live database-file replacement fencing.

Phoenix-owned internal SQLite timestamps default to explicitly unit-named integer Unix microseconds. A different representation requires a more specific normative requirement that identifies the external reader and representation.

## Consequences

- **Positive:** Migration design is not constrained by accidental historical-binary compatibility.
- **Positive:** Runtime-artifact rollback cannot be mistaken for database-compatible rollback.
- **Positive:** Offline restore avoids a database-instance fencing protocol across connection pools and operations.
- **Positive:** Integer timestamps preserve ordering with one representation and simple SQLite type checks.
- **Negative:** Database-compatible rollback requires a separate design and implementation for restoring and verifying matching data; this decision does not provide it.
- **Negative:** Operators cannot replace or restore a database while Phoenix is running.
- **Negative:** Rolling or mixed-version Phoenix processes cannot share one production SQLite database.
- **Negative:** Existing accidental compatibility and mixed timestamp representations require a separate inventory and deliberate cleanup.
- **Neutral:** A more specific normative specification can add a compatibility guarantee or externally required representation after its cost is reviewed.

## References

- `specs/compatibility/requirements.md`
- `specs/production-deployment/executive.md`
- `specs/launchd-deployment/executive.md`
- ADR-033: Database rollback is offline and Foundation observations use relational scalar storage.
- ADR-017: production deployment shares preparation but keeps backend-owned activation
