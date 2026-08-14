# Phoenix Compatibility Guarantees — Executive Status

## Current reality

Phoenix supports forward SQLite migrations, but compatibility guarantees have not previously had one project-wide authority. Some feature tests and implementations therefore provide stronger cross-version behavior than a named product requirement demands.

Production deployment restores the previous binary, configuration, environment, and service state after failed activation. It does not restore a matching database, so its existing outcome is runtime-artifact rollback. Phoenix does not provide a general automatic database rollback subsystem; a feature needing matching runtime/database recovery must define and own a narrower mechanism.

Database restore or replacement is an offline operator procedure. Phoenix does not provide live replacement fencing for an SQLite file already opened by a running process.

Internal SQLite timestamps use mixed representations. New or changed internal timestamp columns must converge on explicitly unit-named integer Unix microseconds unless a more specific normative requirement identifies an external reader and requires another representation.

## Requirement coverage

| Requirement | Current coverage / gap |
| --- | --- |
| REQ-COMP-001 | The requirement is defined in `requirements.md`, and contributor guidance points agents to that authority. A broader guarantee inventory remains follow-up work. |
| REQ-COMP-002 | Phoenix-managed forward migrations and intentional seeder gaps are implemented and tested. Damaged or manually changed ledgers, historical-binary access to newer migrations, and project-wide data preservation are not guaranteed. |
| REQ-COMP-003 | Production deployment implements runtime-artifact rollback only. No general automatic database rollback subsystem exists. |
| REQ-COMP-004 | Single managed-runtime ownership and offline replacement are defined; no live replacement or mixed-version sharing protocol is supported. Operator guidance and stale fencing machinery require follow-up audit. |
| REQ-COMP-005 | Policy established. Existing internal timestamp columns are not migrated by this policy-only change; new Repository observation storage requires follow-up alignment. |

## Next work

A wider compatibility effort should inventory existing guarantees, classify accidental compatibility, remove machinery that serves unsupported scenarios, audit feature-owned migration data policies, and align persisted timestamp representations when their owning schemas change.
