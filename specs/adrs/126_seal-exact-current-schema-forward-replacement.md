# ADR-126: Seal one exact current-schema forward replacement

- **Status:** Accepted
- **Date:** 2026-10-18
- **Affects:** REQ-COMP-015, REQ-PD-028, REQ-LDD-025

## Context

The original ADR-125 prepared candidate completed target128 migration, but activation/finalization is pending with its service absent and original claim retained. Running the predecessor or replaying its snapshot is forbidden after the original migration invocation. Repeatedly starting the original candidate is not an acceptable repair. The commissioned successor is the exact merged, hosted-qualified PR #946 source pinned in REQ-COMP-015, which contains the PR #944 runtime repair and its build-fixture correction; its unchanged complete registry does not establish general candidate compatibility. The exact original/repair full-SHA pair, RC4 embedded version and unchanged registry digest are centralized in the REQ-COMP-015 pin table and launchd helper constants; tests and other documentation refer to those authorities rather than duplicating pins. The separate controller protocol requires independent hosted qualification, review and merge before production authorization.

## Options considered

1. Rewrite the original manifest/proof/status or replace its candidate bytes: rejected; destroys the evidence that explains the committed migration and makes recovery authority ambiguous.
2. Reuse ordinary deploy or release/reacquire the original claim: rejected; creates a window where an incompatible runtime can be admitted and loses original ownership.
3. Add a repair-child pointer to original mutable status: rejected; unnecessarily changes the original committed audit record and creates a second selection representation.
4. Seal one deterministic child with the original claim and operation lock: selected.

## Decision

Use `<parent>/current-schema-repair/manifest.json` as the sole sealed-child selection boundary. Retain private candidate/helper/receipt/registry/plist/proof and closed protocol-v1 manifest/status records. Copy the exact original committed/pending global status into private immutable child `parent-status.json`; bind its hash together with original manifest/proof hashes and the separately clean controller/helper. The repair never writes the parent's status. Active operation checks the live global status against the copy until terminal commit; historical terminal audit uses the copy after another transaction overwrites global status. After full admission and durable child staging but before sealing, atomically replace the installed original binary with a non-executable empty stopped fence and remove the published target plist. This prevents both historical retained helpers and login/reboot persistence from starting the original candidate without altering its private parent evidence. Seal only after the snapshot, all retained artifacts, prepared status and stopped runtime fence are durable; clean only known unsealed, uncheckpointed owned paths. Refuse malformed sealed evidence rather than falling back to original finalization. No child claim or parent pointer is created.

Separate lightweight sealed-child classification from operation admission: classification checks exact stored record shapes, paths, parent manifest/status-snapshot/proof hashes and child proof, without database or backup scans. Activation/resume performs full backup/proof validation and offline exclusive live integrity/foreign-key/full-ledger checks before mutation. Pending committed retries with the verified repair running perform health/configuration and publication/cleanup only, not live-DB reads or reconciliation; stopped retries verify target128 and restart only the repair. Terminal command reentry verifies installed/runtime/publication identity without mutations beyond completing exact claim release.

Verify the original exact committed/pending parent and target128, absent service/helpers, integrity/foreign keys, exclusivity, capacity and captured configuration before preparation and activation. Install only the repair candidate, durably checkpoint `reconciliation_started` before its `--migrate-only`, then persist target proof and target/start checkpoints. Resume ambiguous reconciliation only with the same retained repair candidate; never repeat reconciliation after a verified checkpoint. Neither original nor predecessor startup nor database replay is available at any repair phase.

After exact candidate health/configuration verification, persist pending commit, publish repair source/plist and clean only owned temporaries/old restore capacity. Retain both proofs and original backup. Persist terminal child status, then release/fsync only original ownership, restoring a visible original claim on uncertain unlink durability. Failures attempt stop and record the actual confirmation level with the last durable checkpoint. Malformed child status does not suppress teardown when the parent routing manifest hash and original ownership are valid: attempt stop, report confirmed absence or uncertainty, retain the claim and malformed bytes, and never fabricate a terminal state or resume checkpoint. An unavailable/malformed receipt or retained-helper import fails closed through the same hash-bound teardown route. Original committed/pending status stays untouched throughout repair.

## Consequences

- Compatibility expansion is one exact successor on one exact parent shape, not a generic repair/rollback facility or another baseline/backend.
- A sealed child cannot be replaced by another candidate. Pin changes before sealing require renewed candidate qualification; changes after sealing require a separately specified operation, not edits to retained evidence.
- Static Python/diff checks and authored fault regressions are not runtime acceptance. Hosted tests, macOS disposable validation, exact-head review and production authorization remain separate.
- Historical retained helpers cannot pass their installed-original checksum after the stopped runtime fence is installed; direct manual invocation remains unsupported and cannot restore activation authority.

## References

- [Compatibility](../compatibility/requirements.md), REQ-COMP-015
- [Production deployment](../production-deployment/requirements.md), REQ-PD-028
- [Launchd deployment](../launchd-deployment/requirements.md), REQ-LDD-025
