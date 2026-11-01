# ADR-115: Protect exact 114-to-128 published-release activation with an independently pinned controller

- **Status:** Accepted
- **Date:** 2026-10-17
- **Affects:** REQ-COMP-012/013, REQ-LDD-004/011/019–024, REQ-PD-010/012/021–022
- **Supersedes:** ADR-102 and ADR-105 only where their protected current-schema source restriction excludes the exact-114 published-release admission defined here

## Context

The commissioned controller repair must activate an immutable published candidate whose complete registry ends at migration 128 against a production database whose ledger is the exact complete prefix of that registry through 114. Prepared current-schema admission is restricted to exact source 123 or exact target 128 by ADR-105 and ADR-114. Ordinary forward-only activation qualifies only exact 123-to-128. Neither contract authorizes treating source 114 as another ordinary interval.

The candidate release and the repaired activation controller have separate provenance. Selecting the release commit as the activation-helper authority would execute the candidate's controller rather than the explicitly selected repair. Selecting the controller commit for candidate bytes or its migration registry would instead change the release being activated.

## Options considered

1. **Retain the protected source restriction from ADR-102/105 and prepared extension from ADR-114.** Rejects the commissioned published-release transition even when its exact source lineage is established.
2. **Admit a generic baseline/range or use ordinary deployment fallback.** Avoids a bounded qualification but assumes compatibility for unreviewed intervals and permits predecessor rollback after candidate writes without proof.
3. **Add only exact-114-to-128 published-release admission to the protected lifecycle, with separate controller pinning.** Preserves release identity and reuses the snapshot, capacity, invocation and candidate-only recovery boundaries without a new recovery mechanism.

## Decision

Choose option 3. Partially supersede only the protected source restriction in ADR-102 and ADR-105. Admit exactly a published-release candidate bound to an exact tag, expected complete 40-character release commit, full embedded version/SHA, host-target asset and published checksum, with the complete qualified migration-128 registry, including the named 126/127/128 entries. Its source ledger must equal every version and name in the complete candidate-registry prefix through 114, with no missing, extra, or differently named entries. A highest-version check is insufficient. Aliases, another baseline, a partial ledger, and a different or newer target registry fail before disruption.

Require an explicit full controller commit separate from, and not equal to, the candidate release commit. Obtain and retain the activation helper from that controller commit, with its path, bytes and SHA-256 independently bound in the immutable transaction. The candidate binary and complete migration registry remain sourced from the release commit. Ambient checkout policy, candidate helper selection, or replacement registry bytes cannot stand in for those bindings. Source kind remains `published_release`; mode, status, cleanup, recovery and finalization must agree with that source. Retain captured installed configuration and the same canonical database path.

Reuse the protected current-schema lifecycle without a release-specific weakening: physically held snapshot/restore capacity and free-space/evidence budgets; service stop and exclusive DB/WAL/SHM ownership; private verified SQLite backup and transaction-bound source/logical-content proof; exact candidate installation; durable `migration_started` before bounded offline candidate `--migrate-only`; exact target/integrity/foreign-key verification before runtime traffic. Re-verify immutable candidate, registry/digest and independently pinned helper/controller evidence before migration, activation and recovery.

Before invocation, only proven unchanged source contents and the bound predecessor snapshot/runtime/configuration permit predecessor recovery. At or after `migration_started`, no snapshot replay or predecessor startup is authorized, including failed, interrupted, partial or ambiguous writes. Retain the claim and candidate evidence. `finalize-current` may start only the exact retained candidate after durable target verification, without rerunning offline migration. An incomplete or ambiguous target requires forward repair, not inferred success.

Prepared exact-123/exact-target admission under ADR-114 remains unchanged. Ordinary exact-123-to-128 qualification under ADR-103/105 remains backup-free and retains its candidate-start boundary. Ordinary pending migration intervals without explicit qualification reject before runtime disruption; the protected exact-114 admission is not ordinary qualification.

This is exactly one additional protected transition, not generic baseline/range admission, future-registry compatibility, automatic post-invocation database rollback, downgrade support, mixed-version operation, or a new snapshot/recovery subsystem.

## Consequences

- **Positive:** The published candidate can use existing protected migration and recovery boundaries while its release identity stays independent of the repaired controller.
- **Negative:** Operators must pin two full commits and retain helper/registry evidence; full capacity and offline snapshot costs apply, and post-invocation failures sacrifice predecessor availability rather than guess compatibility.
- **Neutral:** Artifact, runtime, deployment and user acceptance remain separate from the normative admission; no production execution is authorized by this decision.

## References

- [ADR-102](102_current-schema-prepared-activation-commits-at-database-write.md): source restriction partially superseded; snapshot/invocation/recovery boundary retained.
- [ADR-105](105_qualify-migration-128-for-bounded-current-schema-activation.md): source restriction partially superseded; exact target and ordinary interval retained.
- [ADR-114](114_admit-exact-target-ledger-for-prepared-current-schema-activation.md): prepared exact-target admission retained.
- [ADR-034](034_compatibility-guarantees-are-explicit-and-data-aware.md): explicit compatibility policy.
- [Compatibility requirements](../compatibility/requirements.md): REQ-COMP-012/013.
- [Launchd deployment requirements](../launchd-deployment/requirements.md) and [behavioral model](../launchd-deployment/launchd-deployment.allium): REQ-LDD-019–024.
- [Production deployment requirements](../production-deployment/requirements.md): REQ-PD-021–022.
