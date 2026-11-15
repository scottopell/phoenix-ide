# ADR-246: Admit the Exact Canonical 158-Before-156 Ledger

- **Status:** Accepted
- **Date:** 2026-11-09
- **Affects:** REQ-COMP-002, REQ-COMP-020, REQ-COMP-022

**Qualifies:** [ADR-187](187_goal-main-migration-identity-reconciliation.md), only its requirement that every registered predecessor through a post-129 recorded tail be present. Its canonical-identity, schema-evidence, unsupported-lineage, and fail-before-mutation decisions remain accepted.

## Context

The canonical main registry records migration 158 `federated_conversation_actions`. The remote Coordinator Goals work retains its reserved migration 156 `remote_coordinator_goals`. A database migrated by the supported main binary therefore can contain every canonical embedded migration from 129 through 155 plus canonical 158 while lacking 156. Integrating both migrations into one registry makes that supported ledger appear to have a missing registered predecessor under ADR-187's general post-129 continuity check, so preflight refuses before the ordinary membership-based runner can apply 156.

The migration-156 identity is part of the coordinated Goals integration contract, while migration 158 is already canonical history. The system needs to admit the exact main-produced post-129 predecessor registry without converting arbitrary missing rows in the registered tail into a repair promise or changing the existing treatment of pre-129 ledgers.

## Options considered

1. **Renumber migration 156 above 158** — preserves strict predecessor continuity but abandons the coordinated migration-156 identity and its integration evidence.
2. **Allow any canonical migration missing below the recorded tail** — follows membership-based pending detection broadly but weakens lineage damage detection and expands sparse-ledger compatibility without evidence.
3. **Admit only a canonical 158-high ledger whose sole missing embedded predecessor in the post-129 registered tail is 156** — preserves both identities, supports the actual main-produced source, and keeps every other post-129 hole fail-closed.

## Decision

Choose option 3. After every recorded version and name matches the embedded canonical registry, lineage preflight computes the complete set of missing embedded predecessors from 129 through the highest recorded version. It admits a nonempty set only when the highest recorded version is 158 and the complete post-129 missing set is exactly `[156]`. All existing schema-marker and table-group validation remains mandatory. The ordinary pending-migration runner then applies 156 by individual version membership.

A ledger with another missing post-129 predecessor, multiple post-129 gaps, a highest recorded version above 158, a wrong identity, an unknown migration, or a conflict detected by the existing lineage schema-evidence validators remains unsupported and is refused before migration or ledger mutation.

## Consequences

- **Positive:** Databases produced by the canonical main registry ending at 158 can move forward without renumbering either migration or bypassing the normal migration runner.
- **Positive:** The exception is structurally bounded by the complete post-129 missing-version set and highest recorded version rather than a broad registered-tail allowance.
- **Negative:** Lineage validation carries one permanent historical registry-transition exception.
- **Neutral:** Once migration 156 is recorded, the ledger is complete and follows the ordinary predecessor rule on subsequent opens.

## References

- `specs/compatibility/requirements.md` — REQ-COMP-002, REQ-COMP-020, and REQ-COMP-022
- [ADR-187](187_goal-main-migration-identity-reconciliation.md)
- `lineage::preflight`
- `lineage::is_supported_canonical_gap`
- `migrations::tests::migration_156_applies_when_158_is_already_recorded`
