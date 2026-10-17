# Phoenix Compatibility Guarantees — Executive Status

## Current reality

Phoenix supports forward SQLite migrations, but compatibility guarantees have not previously had one project-wide authority. Some feature tests and implementations therefore provide stronger cross-version behavior than a named product requirement demands.

Production deployment restores the previous binary, configuration, environment, and service state after failed activation. It does not restore a matching database, so its existing outcome is runtime-artifact rollback. The release-update UI still describes that outcome as the previous release being “restored and verified,” which overstates the database guarantee. Phoenix also still accepts an older release candidate without proving that its paired database backup was restored. Phoenix does not provide a general automatic database rollback subsystem. Manual version rollback is offline: stop Phoenix, restore the database backup paired with the target binary, then start that binary. Task 44016 owns the deployment rejection boundary and truthful rollback messaging.

Database restore or replacement is an offline operator procedure. Phoenix does not provide live replacement fencing for an SQLite file already opened by a running process.

The narrow prepared-artifact launchd exception admits either the exact migration-123 baseline or a source ledger exactly equal to the migration-128 candidate registry under ADR-105 and ADR-114, including named migrations 126/127/128 and immutable registry/digest checks. The 128-to-128 controller and regression changes are authored; hosted CI qualification is pending. Both sources require an offline snapshot and mandatory candidate `--migrate-only` invocation, permit automatic predecessor rollback only before invocation with unchanged transaction-bound source contents, and forbid snapshot replay or predecessor startup afterward. This is one bounded prepared replacement, not a general same-version, cross-binary, downgrade, or runtime compatibility guarantee.

[ADR-115](../adrs/115_protect-exact-114-to-128-published-release-activation.md) adds one commissioned protected published-release admission: the exact complete candidate-registry prefix through 114 to the exact target 128. The release supplies the candidate binary and registry; a separately explicit controller commit, not the release commit, supplies the retained hash-bound helper. It reuses the full protected snapshot/capacity/exclusivity and durable `migration_started` lifecycle, including candidate-only recovery after invocation. Prepared exact-123/exact-target and ordinary exact-123-to-128 admission remain unchanged. Documentation is authored; controller/regression qualification, runtime and deployment acceptance are not established by this documentation change. Ordinary unqualified pending migration intervals must reject before disruption, not silently fall back to artifact rollback.

The ordinary in-place route does not gain a backup requirement. For the same exact 123-to-128 interval, the authored policy retains immutable registry/digest evidence and treats the first candidate startup attempt as a conservative downgrade boundary: later failure retains the candidate and stops forward-only because migration or initialization writes may already have committed. Hosted CI qualification remains pending.

Internal SQLite timestamps use mixed representations. New or changed internal timestamp columns must converge on explicitly unit-named integer Unix microseconds unless a more specific normative requirement identifies an external reader and requires another representation.

Compiler caching supports an exact Kache 0.26.0 local command/daemon contract as an explicit opt-in, with sccache and uncached automatic fallbacks. Automatic selection excludes Kache for every build surface because restored-archive source-level debug fidelity is unqualified; the fidelity blocker remains open. Cache selection is scoped to Cargo subprocesses and does not establish a general cross-version, remote-cache, performance, or debug-symbol compatibility guarantee.

## Requirement coverage

| Requirement | Current coverage / gap |
| --- | --- |
| REQ-COMP-013 | ADR-115 documents pre-disruption rejection of ordinary unqualified pending intervals; qualification pending. Authored; hosted CI pending for exact 123-to-128 ordinary no-backup pre-start rollback, post-start forward-only fencing, no predecessor launch, immutable registry/digest checks, and exact-candidate resume regressions. No execution or deployment acceptance claimed. |
| REQ-COMP-012 | ADR-115 exact-114-to-128 protected release and independent controller pinning documented; controller/regression qualification pending. Authored; hosted CI pending for exact 123-to-128 transition and prepared exact-target-ledger admission, named 126/127/128 identities, immutable registry/digest checks, mandatory offline candidate invocation, pre-invocation-only rollback, and partial/ambiguous-write fail-closed regressions in the launchd prepared helper. No execution or deployment acceptance claimed. |
| REQ-COMP-001 | The requirement is defined in `requirements.md`, and contributor guidance points agents to that authority. A broader guarantee inventory remains follow-up work. |
| REQ-COMP-002 | Phoenix-managed forward migrations and intentional seeder gaps are implemented and tested. Damaged or manually changed ledgers, historical-binary access to newer migrations, and project-wide data preservation are not guaranteed. |
| REQ-COMP-003 | Foundation acceptance covers manual offline paired restore, but production deployment can still launch an older candidate without proving that restore occurred, and the UI overstates runtime-artifact rollback. Enforcement and truthful messaging are tracked in task 44016. |
| REQ-COMP-004 | Single managed-runtime ownership and offline replacement are defined; no live replacement or mixed-version sharing protocol is supported. Operator guidance and stale fencing machinery require follow-up audit. |
| REQ-COMP-005 | Policy applies to newly introduced or structurally changed internal timestamp columns. Unchanged historical timestamp columns are outside this initial convergence rule. |
| REQ-COMP-006 | Legacy Direct WorkScopes are repaired forward to Direct authority without adding rollback guarantees. |
| REQ-COMP-007 | Historical accepted source-locator payloads retain their original fingerprint and encoding authority. |
| REQ-COMP-008 | `dev.py` keeps automatic selection on usable sccache/no cache, requires exact Kache 0.26.0 on macOS arm64 as an explicit opt-in, verifies daemon readiness/socket identity, reports actual selection, and scopes generated cache variables to build subprocesses. Restored-archive debug fidelity remains unqualified. |

## Next work

Task 44016 should enforce the offline downgrade boundary and make runtime-artifact rollback messaging truthful. A wider compatibility effort should inventory existing guarantees, classify accidental compatibility, remove machinery that serves unsupported scenarios, and audit feature-owned migration data policies.
