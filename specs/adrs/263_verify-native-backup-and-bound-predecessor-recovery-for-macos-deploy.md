# ADR-263: Verify a native backup and bound predecessor recovery for macOS deploy

- **Status:** Accepted
- **Date:** 2026-11-16
- **Affects:** REQ-LDD-002, REQ-LDD-004, REQ-LDD-009, REQ-LDD-029–031, REQ-PD-010, REQ-COMP-021
- **Qualifies:** [ADR-230](230_simplify-macos-deployment-to-candidate-migration-and-manual-recovery.md)

## Context

ADR-230 removed the predecessor startup and database snapshot/rehearsal routes
from routine macOS deployment. Candidate-owned migration remains the right
boundary for schema decisions, but the absence of any verified backup leaves no
transaction-local recovery artifact after a candidate failure. Equally, a
failure provably before binary replacement or candidate migration need not
forbid restoration of the exact unchanged installed runtime. Neither observation
justifies replay, schema-specific admission or optimistic recovery when database
writes are uncertain.

## Options considered

1. Keep ADR-230 unchanged: stop and retain only, without a native backup or
   preboundary predecessor recovery.
2. Reinstate the previous scan/rehearsal/snapshot/replay and automatic rollback
   machinery, including candidate-stage fallback.
3. Have the independent deploy helper verify a native backup after observed
   predecessor stop and permit only proof-gated unchanged predecessor recovery
   before the candidate boundary; fail-stop after any attempted or uncertain
   candidate boundary.

## Decision

Choose option 3. For an existing database, check backup capacity plus bounded
overhead before quarantine and stop. After observed stop, hold a bounded SQLite
`BEGIN IMMEDIATE` writer reservation across native backup verification against
the stopped source and durable candidate-boundary recording. Retain the fsynced
backup with transaction-matched path, size and SHA-256 proof; verification
failure prevents candidate installation or migration. Only a true first install
with an absent database, no managed predecessor and no installed binary, plist
or loaded service may omit the backup. Record private transaction-matched
`database_source: absent` and `backup: null` proof, and recheck path absence
before marking the candidate boundary; an existing database never uses this
exception. Check bounded first-install capacity instead of backup capacity.
Recheck the operational reserve, mark the candidate boundary attempted *before*
binary replacement or migration, and release the writer reservation before
candidate migration or predecessor recovery. Maintenance requires no external
database clients writing throughout the update, including after the reservation
is released at candidate handoff. The candidate owns migration and
reconciliation. Capacity checks do not promise migration peak space; neither
backup verification nor absence proof authorizes a schema-specific upgrade route.

Before that boundary, attempt private startup only for the captured unchanged
predecessor after positive stop/PID-absence, artifact identity and target-absence
checks; verify exact identity and stable new PID before canonical publication and
reporting recovery. If any proof is unavailable, retain quarantine and ownership
for manual resolution. At or after an attempted or uncertain boundary, never
start the predecessor automatically: attempt target stop and retain the claim,
transaction evidence and either the existing database's verified backup or
first-install absence proof for manual recovery. An attempted stop is not proof
of absence. Do not automatically restore a database, replay a transaction
or assume an older binary can read candidate-written data. Report transaction-
matched proof and unknown boundary/backup when proof is unavailable. The
independent deploy handoff is protocol 3; installed-runtime restart stays on
its independent protocol 2.

This **supersedes only** ADR-230's prohibitions on a routine native backup and
all predecessor startup, replacing them with the bounded rules above. ADR-230's
rejection of exhaustive scans, migration-registry parsing, schema-specific
admission, upgrade rehearsal, automatic postboundary rollback and replay remains
in effect. No guardian, standalone controller, floor waiver or new cross-version
availability/downgrade guarantee is authorized.

## Consequences

- For an existing database, the stopped interval includes backup verification;
  insufficient capacity or verification failure may leave availability to a
  proven preboundary recovery or operator action. A true first install has
  absence proof rather than a backup.
- Where a database existed, its verified backup remains for offline operator
  recovery, not an automatic restore or a guarantee against postboundary writes.
  Downgrade after an attempted/uncertain boundary still requires the matching
  offline database restore while all users are stopped.
- Only unambiguous preboundary failures can automatically recover the exact
  predecessor. Ambiguous boundaries fail closed, potentially leaving service
  unavailable for manual recovery.

## References

- [ADR-230](230_simplify-macos-deployment-to-candidate-migration-and-manual-recovery.md)
- [macOS launchd deployment requirements](../launchd-deployment/requirements.md)
- [Compatibility policy](../compatibility/requirements.md)
