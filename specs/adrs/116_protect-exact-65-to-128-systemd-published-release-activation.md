# ADR-116: Protect exact 65-to-128 systemd published-release activation

- **Status:** Accepted
- **Date:** 2026-10-17
- **Affects:** REQ-COMP-014, REQ-PD-003, REQ-PD-010, REQ-PD-012, REQ-PD-019, REQ-PD-023–027; `ProtectedSystemdReleaseDeployment`

## Context

A Linux systemd installation has an existing predecessor and a complete source migration ledger through65. The desired candidate is the already published `v0.13.0-rc.4`, not a rebuilt local binary or a prepared launchd artifact. Its migration interval can commit irreversible changes before runtime health is observed. Runtime-artifact rollback alone cannot make the old binary safe after candidate database writes. ADR-034 requires an explicit bounded compatibility guarantee rather than defensive generic recovery machinery.

The candidate was published before the protecting controller implementation. Candidate identity must remain immutable while the controller/helper comes from a separately identified different commit. Installed environment, service/socket units and canonical database path are operator-owned inputs to preserve, not an opportunity to replace production configuration from an ambient env file.

## Options considered

1. **Ordinary published activation with runtime-artifact rollback.** Reuses the normal path but can start an incompatible predecessor after partial migration and does not rehearse the actual database.
2. **Generalize paired deployment across backends and schema intervals.** Offers broad backup/restore machinery but imports a large downgrade/compatibility obligation and unrelated launchd mechanics.
3. **Qualify one exact published systemd transition, with pristine backup and isolated rehearsal before a durable live boundary.** Requires bounded transaction evidence and operator-only forward recovery but preserves the published candidate and makes predecessor safety explicit.

## Decision

Choose option 3. REQ-PD-023 is the immutable identity authority: tag `v0.13.0-rc.4`, candidate commit `3f0ba4602faa71ebad5088e1ffc856f4c7d511d9`, embedded version `0.13.0-rc.4`, `phoenix_ide-x86_64-unknown-linux-musl`, asset SHA-256 `bcb74d565c828843596e5109ea9034a2b0e505aab4ba6c18c5cca8e116f5677e`, and candidate migration-source SHA-256 `700162dd8e41305546cf2acd2763bad5603c642840f5be851fb788353879a36a`, ending at128. Admission compares the entire ordered `(version, name)` source ledger to that exact registry's prefix through65. It does not compare only maximum version or count. The separately explicit full controller commit must differ from the candidate commit; the retained helper hash is verified against that controller source and bound independently of release evidence.

Require an existing predecessor and preserve installed env/service/socket/canonical DB path. Validate all immutable evidence before disruption. Stop both service and socket, prove inactivity and exclusive database ownership, and take a persistent pristine SQLite backup with context-bound hash/proof. Run the exact candidate's `--migrate-only` on an isolated backup copy. Verify successful exit, exact target ledger, integrity/FK and unchanged pristine backup before permitting live migration. Backup/rehearsal capacity or verification failure is pre-marker and permits only proven unchanged-predecessor recovery, never backup replay to manufacture unchanged-source proof. No launchd 20 GiB floor is imported.

Durably record `migration_started` immediately before the first live candidate invocation. At or after the marker, never restore the backup or predecessor runtime artifacts or start the old binary, including on failure, interruption, partial commits or uncertain writes. Retain candidate, claim, backup and evidence on unresolved failure and attempt to stop both units. Explicit `recover-systemd-current` can use only the exact retained candidate/controller/helper and preserved configuration, after proving matching ownership, intact marker/backup/rehearsal evidence and an exclusive ledger exactly equal to a prefix of the same registry through65..128. It may rerun only that candidate; exact target may finalize without rerun. Verify complete target/integrity/FK before candidate startup and exact runtime identity before durable publication/commit and claim release. Keep the pristine backup after success.

Ordinary systemd through the repaired controller rejects observed pending/divergent ledgers before handoff; exact-target/no-pending ordinary activation remains. This decision does not retroactively guarantee behavior of historical controllers. Global operation locking plus exact staging status written before claim publication permits only positively identified partial staging retirement. Ambiguous, prepared and post-marker transactions remain fenced.

This decision is independent of other installer work and depends only on ADR-034's explicit-compatibility policy and ADR-017's backend-owned activation architecture. It does not change launchd contracts or authorize other releases, source baselines, target registries, local/prepared sources, bare-Linux transitions, generic rollback, downgrade, or production execution.

## Consequences

- **Positive:** The exact published candidate can be rehearsed against the captured source without touching live data, and candidate-owned writes cannot silently fall back to the old runtime.
- **Negative:** A failed live migration can leave production stopped behind a retained claim; recovery is exact-candidate-only and cannot substitute a future release. Persistent backup and rehearsal storage are required, and capacity failure blocks the transition safely before the marker.
- **Neutral:** The pristine backup is retained evidence, not an automatic post-marker restore option. New release or baseline support requires another explicit qualification. Documentation and source validation do not establish disposable-systemd or production acceptance.

## References

- [Compatibility requirements](../compatibility/requirements.md), REQ-COMP-014.
- [Production deployment requirements](../production-deployment/requirements.md), REQ-PD-023–027, and `production-deployment.allium`.
- [ADR-034](034_compatibility-guarantees-are-explicit-and-data-aware.md), explicit compatibility policy.
- [ADR-017](017_production-deployment-shares-preparation-not-activation-ownership.md), backend-owned activation.
- `ProdDeployControllerOptions::require_systemd_current`, `cmd_prod_deploy_native` in `dev.py`.
- `snapshot_current`, `rehearse_current_migration`, `run_current_migration`, `recover_current`, `retire_partial_staging` in `scripts/systemd_deploy_helper.py`.
