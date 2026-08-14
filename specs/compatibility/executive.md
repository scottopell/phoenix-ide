# Phoenix Compatibility Guarantees — Executive Status

## Current reality

Phoenix supports forward SQLite migrations, but compatibility guarantees have not previously had one project-wide authority. Some feature tests and implementations therefore provide stronger cross-version behavior than a named product requirement demands.

Production deployment restores the previous binary, configuration, environment, and service state after failed activation. It does not restore a matching database, so its existing outcome is runtime-artifact rollback rather than database-compatible rollback. Operators must not assume that a restored older binary can use a database changed by the failed candidate.

Database restore or replacement is an offline operator procedure. Phoenix does not provide live replacement fencing for an SQLite file already opened by a running process.

Internal SQLite timestamps use mixed representations. New or changed internal timestamp columns must converge on explicitly unit-named integer Unix microseconds unless a more specific normative requirement identifies an external reader and requires another representation.

## Requirement coverage

| Requirement | Current coverage / gap |
| --- | --- |
| REQ-COMP-001 | The requirement is defined in `requirements.md`, and contributor guidance points agents to that authority. A broader guarantee inventory remains follow-up work. |
| REQ-COMP-002 | Forward migrations are implemented and tested. Historical-binary-on-newer-database behavior is not a guarantee and compatibility-only machinery requires follow-up removal. |
| REQ-COMP-003 | Production deployment implements runtime-artifact rollback only. It does not expose a distinct database-compatible rollback outcome or restore a matching previous database. |
| REQ-COMP-004 | Single managed-runtime ownership and offline replacement are defined; no live replacement or mixed-version sharing protocol is supported. Operator guidance and stale fencing machinery require follow-up audit. |
| REQ-COMP-005 | Policy established. Existing internal timestamp columns are not migrated by this policy-only change; new Repository observation storage requires follow-up alignment. |

## Next work

A wider compatibility effort should inventory existing guarantees, classify accidental compatibility, decide whether to implement database-compatible deployment rollback or retain runtime-artifact rollback only, remove machinery that serves unsupported scenarios, and align persisted timestamp representations when their owning schemas change.
