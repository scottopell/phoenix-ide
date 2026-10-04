# ADR-075: Prepared-artifact paired launchd upgrade fails closed

- **Status:** Accepted
- **Date:** 2026-10-04
- **Affects:** specs/production-deployment, specs/launchd-deployment, specs/compatibility

## Context

A protected prepare-main standalone binary is produced by a release controller, while the local clean checkout owns the activation helper and the installed launchd configuration. The ProductConversation schema transition requires the predecessor database to remain recoverable if activation or health verification fails.

## Decision

The deployment command accepts this combination only when the prepared artifact, exact full candidate SHA, and explicit paired-upgrade flag are all present. The receipt identifies a submission UUID, host-target exact standalone basename, SHA, and qualified signing/notarization evidence. The candidate is strict-codesign checked and never ad-hoc resigned. The artifact directory is trusted operator input obtained from the protected Actions run; the controller validates its contents but does not authenticate the JSON receipt's origin. The operator must establish the run/source association before invocation. The clean controller checkout must be clean and its helper bytes are bound into the private manifest.

Paired state is one typed optional aggregate containing absolute database, backup, proof, and helper paths plus controller provenance. Before disruption the helper requires a predecessor, matching database paths in candidate and predecessor plists, a read-only existing migration ledger at version 69 or below, and no ProductConversation tables. After quiesce it proves no database/WAL/SHM owners with bounded `lsof`, snapshots through SQLite's backup API using closed connections, writes mode-600 backup/proof files in a mode-700 private transaction, and verifies integrity and context.

Any failed proof leaves the candidate stopped and retains the claim, transaction, snapshot, and status for manual recovery. Database sidecars are removed only after exclusive offline proof. Runtime-only rollback retains its existing artifact-only behavior.

## Consequences

The feature has no generic database rollback semantics and does not permit cleanup that could destroy the only recovery snapshot. Standalone Gatekeeper assessment is not imposed because the stapled ticket may belong to the containing application; strict codesign metadata and the qualified receipt remain mandatory.

## Options considered

- Generic automatic database snapshots would expand compatibility guarantees beyond the feature contract.
- Resigning the protected binary would invalidate the controller's artifact provenance.
- Treating a missing `lsof`, malformed ledger, or missing proof as safe would make recovery depend on an unverified runtime.

## References

- `specs/launchd-deployment/requirements.md`
- `specs/compatibility/requirements.md`
- `scripts/launchd_deploy_helper.py`
- `dev.py`
