# ADR-105: Qualify migration 128 for bounded current-schema activation

- **Status:** Accepted
- **Date:** 2026-10-15
- **Affects:** REQ-COMP-012, REQ-COMP-013, REQ-LDD-019–023, REQ-PD-021–022, REQ-CCR-012

## Context

ADR-102 and ADR-103 bound prepared and ordinary current-schema activation to the migration-123 baseline and exact migration-127 candidate registry. The candidate also includes migration 128, `product_creation_provisioning_stage`. That migration adds relational creation checkpoints and preserves accepted jobs using durable evidence and row timestamps under ADR-104. Leaving the deployment qualification at migration 127 would reject the intended candidate; merely accepting a higher maximum would silently authorize unreviewed registries.

Migration 128 is part of the same forward activation, not evidence that the predecessor can operate on the new schema. Migrations commit independently, and startup reconciliation can write after migration. Neither a failed process nor an unchanged highest ledger proves that predecessor startup or snapshot replay is safe.

## Options considered

1. **Keep the 123-to-127 interval from ADR-102/103.** Preserves the prior exact gate but excludes the candidate containing the creation-checkpoint migration.
2. **Accept migration 128 or any later registry by version ceiling alone.** Avoids another bounded decision but grants compatibility to unreviewed migration identities and contents.
3. **Qualify only the exact migration-128 candidate registry from the exact migration-123 baseline.** Includes the evidence-preserving creation migration while retaining immutable transaction evidence, fail-closed admission, and the existing irreversible boundaries.

## Decision

Choose the exact 123-to-128 interval for both prepared-artifact and ordinary in-place activation. This supersedes only the bounded 123-to-127 interval portions of ADR-102 and ADR-103. Their supply-chain, ownership, snapshot, rollback-boundary, and exact-candidate recovery decisions remain binding.

The complete candidate migration registry must identify migration 126 as `retire_wake_feature_preserving_history`, migration 127 as `coordinator_owned_work_scope_home`, and migration 128 as `product_creation_provisioning_stage`. The source ledger must match the exact migration-123 baseline and the completed target ledger must match the complete qualified candidate registry. Any different, missing, or newer registry or mismatched ledger fails closed; the three named entries are not a substitute for whole-registry verification.

Retain the complete registry and its SHA-256 digest as immutable transaction evidence bound to the exact candidate and retained helper. Verify those identities and digests before migration, activation, and explicit forward recovery. Recovery cannot replace the registry, rebind its digest, substitute a candidate, or infer compatibility from a version number alone.

For prepared activation, automatic predecessor rollback ends before offline candidate migration invocation. After invocation, preserve the snapshot as audit evidence only: no snapshot replay or predecessor startup, including after partial migration or ambiguous reconciliation. Finalization re-verifies the proof and exact target ledger and starts only the retained candidate without rerunning offline migration.

For ordinary activation, do not add a standalone backup requirement. Persist the forward-only boundary before the first candidate startup attempt; later failure or uncertainty retains the candidate and stops forward-only. Explicit recovery starts only the exact retained candidate after immutable evidence checks.

This decision adds migration 128 to one qualified interval. It grants no generic forward compatibility, downgrade support, mixed-version operation, live database replacement, or compatibility for a different baseline or future registry.

## Consequences

- **Positive:** The intended candidate can include evidence-preserving creation checkpoints without relaxing exact migration identity or transaction evidence.
- **Negative:** Any registry change requires a separate explicit qualification; a post-boundary failure can require operator-authorized forward repair rather than automatic availability restoration.
- **Neutral:** Migration 128's data-preservation policy remains owned by REQ-CCR-012 and ADR-104. This interval decision does not establish test, runtime, artifact, or deployment acceptance.

## References

- [ADR-102](102_current-schema-prepared-activation-commits-at-database-write.md) and [ADR-103](103_ordinary-forward-launch-is-the-downgrade-boundary.md): interval portions superseded; activation and recovery boundaries retained.
- [ADR-104](104_product-creation-stages-backfill-from-durable-evidence.md): migration 128's durable-evidence transformation.
- [ADR-034](034_compatibility-guarantees-are-explicit-and-data-aware.md): explicit bounded compatibility policy.
- [Compatibility requirements](../compatibility/requirements.md): REQ-COMP-012 and REQ-COMP-013.
- [Conversation creation requirements](../conversation-creation/requirements.md): REQ-CCR-012.
- [Launchd deployment requirements](../launchd-deployment/requirements.md): REQ-LDD-019–023.
- [Production deployment requirements](../production-deployment/requirements.md): REQ-PD-021–022.
- `MIGRATION_128`, `migration_registry`, `validate_current_schema_database`, `migration_registry_sha256`.
