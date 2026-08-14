# Phoenix Compatibility Guarantees — Executive Status

## Current reality

Phoenix supports forward SQLite migrations, but compatibility guarantees have not previously had one project-wide authority. Some feature tests and implementations therefore provide stronger cross-version behavior than a named product requirement demands.

Production deployment restores the previous binary, configuration, environment, and service state after failed activation. It does not restore a matching database, so its existing outcome is runtime-artifact rollback. The release-update UI still describes that outcome as the previous release being “restored and verified,” which overstates the database guarantee. Phoenix also still accepts an older release candidate without proving that its paired database backup was restored. Phoenix does not provide a general automatic database rollback subsystem. Manual version rollback is offline: stop Phoenix, restore the database backup paired with the target binary, then start that binary. Task 44016 owns the deployment rejection boundary and truthful rollback messaging.

Database restore or replacement is an offline operator procedure. Phoenix does not provide live replacement fencing for an SQLite file already opened by a running process.

Internal SQLite timestamps use mixed representations. New or changed internal timestamp columns must converge on explicitly unit-named integer Unix microseconds unless a more specific normative requirement identifies an external reader and requires another representation.

## Requirement coverage

| Requirement | Current coverage / gap |
| --- | --- |
| REQ-COMP-001 | The requirement is defined in `requirements.md`, and contributor guidance points agents to that authority. A broader guarantee inventory remains follow-up work. |
| REQ-COMP-002 | Phoenix-managed forward migrations and intentional seeder gaps are implemented and tested. Damaged or manually changed ledgers, historical-binary access to newer migrations, and project-wide data preservation are not guaranteed. |
| REQ-COMP-003 | Foundation acceptance covers manual offline paired restore, but production deployment can still launch an older candidate without proving that restore occurred, and the UI overstates runtime-artifact rollback. Enforcement and truthful messaging are tracked in task 44016. |
| REQ-COMP-004 | Single managed-runtime ownership and offline replacement are defined; no live replacement or mixed-version sharing protocol is supported. Operator guidance and stale fencing machinery require follow-up audit. |
| REQ-COMP-005 | Policy applies to newly introduced or structurally changed internal timestamp columns. Unchanged historical timestamp columns are outside this initial convergence rule. |

## Next work

Task 44016 should enforce the offline downgrade boundary and make runtime-artifact rollback messaging truthful. A wider compatibility effort should inventory existing guarantees, classify accidental compatibility, remove machinery that serves unsupported scenarios, and audit feature-owned migration data policies.
