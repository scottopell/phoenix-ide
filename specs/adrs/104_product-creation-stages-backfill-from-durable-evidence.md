# ADR-104: Product creation stages backfill from durable evidence

- **Status:** Accepted
- **Date:** 2026-10-15
- **Affects:** REQ-CCR-002, REQ-CCR-003, REQ-CCR-012, REQ-COMP-001, REQ-COMP-002, REQ-COMP-005

## Context

Product creation jobs gain a relational provisioning stage and a stage timestamp. Accepted jobs already carry durable status, starting-pin, and staging evidence, but do not carry a phase-by-phase history. Assigning every row the initial phase would discard useful evidence; marking staged jobs complete would fabricate acknowledgement of external effects. The project compatibility policy requires the owning feature to specify an explicit forward transformation rather than silently treating schema defaults as a recovery contract.

## Options considered

1. **Reset every row to intent validation at migration time.** Simple defaults, but loses known progress and gives old work a fabricated fresh timestamp.
2. **Infer progress from the filesystem or repository during migration.** Can observe more external facts, but makes a database migration depend on mutable resources and confuses existence with acknowledged ownership or completion.
3. **Backfill from durable row evidence and retain the row update timestamp.** Preserves accepted work without inventing a phase history; resumed work must still reconcile uncertain external effects.

## Decision

Choose the evidence-based forward transformation defined by REQ-CCR-012. Preserve existing product creation rows and their accepted data. Apply this precedence: `published` becomes `finalize_publication`; `delivery_pending` and `delivery_failed` become `bootstrap_initial_turn`; otherwise a non-null staging path becomes `materialize_worktree`; otherwise a non-null exact checkout pin becomes `resolve_starting_pin`; otherwise use `validate_intent`.

Copy `updated_at_unix_micros` exactly into `stage_updated_at_unix_micros`. The timestamp is the durable row's update observation, not a reconstructed phase-entry time. The migration does not inspect external resources, rewrite claim authority, or treat a staging path as proof that materialization finished.

After migration, a worker retains the persisted phase as its checkpoint floor while reconciling earlier external facts. Actual advancement requires a live current claim, the exact predecessor, and a non-regressing persistence-layer clock sample. Direct-path no-op phases are traversed in sequence. The metadata-publication and delivery-completion transactions own their respective stage transitions.

## Consequences

- **Positive:** Existing jobs retain identity, intent, durable progress evidence, and timestamp ordering without an external-resource-dependent migration.
- **Negative:** Historical stage timestamps are coarser evidence than true phase-entry timestamps. Reconciliation cannot skip ownership checks merely because a stage was backfilled.
- **Neutral:** The policy grants a bounded forward migration only. Downgrade, mixed-version writers, and live database replacement remain outside this feature's compatibility guarantee.

## References

- [Conversation creation requirements](../conversation-creation/requirements.md), especially REQ-CCR-002, REQ-CCR-003, and REQ-CCR-012.
- [Compatibility requirements](../compatibility/requirements.md).
- ADR-034 establishes explicit feature-owned compatibility and data policies.
- ADR-043 defines the private staging ownership boundary; ADR-044 defines request-bound creation identity and immutable pins.
- `MIGRATION_128`, `ProductCreationProvisioningStage`, `Database::advance_product_creation_provisioning_stage`, `checkpoint_product_creation_stage`.
