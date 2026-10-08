# ADR-087: Close retry adopts exact cleanup lineage without automatic repair dispatch

- **Status:** Accepted
- **Date:** 2026-10-07
- **Affects:** REQ-WL-002c, REQ-WL-004, REQ-WL-005, REQ-WL-008; `WorktreeCleanupDispatch`, `WorktreeCleanupPlan`, `WorktreeCleanupAdoption`; REQ-COMP-001, REQ-COMP-002

## Context

PR #865 extracts a bounded Close repair from the unmerged broader retirement work. The retained foreign-key failure leaves one attempt with a newer sealed inventory and exact worktree residuals but cleanup dispatch/plan authority in an older inspection pair. A retry must not invent a new attempt or treat a path as ownership. Copying only a dispatch can fail its plan's exact-generation foreign key; copying only a plan leaves destructive dispatch unproven.

The incident's retained inspection phase is distinguishable from arbitrary repair by its exact row shape and complete retained diagnostic value. Startup retry of every `NeedsRepair` attempt would also bypass the explicitly coordinated sole-executor cleanup: external repair becoming possible is not permission to delete.

## Options considered

1. **Automatic database surgery or startup retry of all repair states.** Easy convergence, but destroys the explicit retry boundary and assumes arbitrary legacy evidence is trustworthy.
2. **Abandon retained generations and create a new Close attempt.** Avoids adoption machinery, but loses exact-attempt continuity and cannot safely account for moved worktrees and bound tombstones.
3. **Feature-scoped recognition plus transactional same-attempt adoption.** Preserves evidence and explicit repair admission at the cost of exact lineage constraints and live identity validation.

## Decision

Choose option 3. Require explicit `CloseRetirementRetryRequested` to leave `NeedsRepair`. An interrupted attempt already in `RetirementRequested` may recover at startup, but still reacquires scope leases and checks live identity and writer safety.

Support only the partial-generation conjunction in REQ-WL-005: retained pair in `awaiting_retirement_inspection`, captured worktree scopes with the exact inspection/residual counts, sealed active inventories for every scope, exact `worktree_id_v1` residuals with `manual_repair_required` and detail equal to the complete retained diagnostic `Database error: error returned from database: (code: 787) FOREIGN KEY constraint failed`, no active-pair dispatch or plan, and exact prior-pair dispatch/plan authority for every captured worktree. Diagnostic equality is exact: prefixed, suffixed, generic FK-787, and cleanup-plan-prefixed messages are not compatible values. Recognition changes only the phase, in one transaction. It does not rewrite observations or itself permit removal.

Adoption uses the exact attempt/scope/pair/resource identity and fresh administrative locator/incarnation. Select the newest eligible unconsumed source by inventory capture time then plan row order. Commit target dispatch, full target plan and source-to-target lineage atomically. Identical replay revalidates and returns existing authority; conflict rolls every write back. Each source has at most one outgoing edge and each target one incoming edge. Lineage and adopted-source payloads are immutable during repair/retry. Completed aggregate hard deletion is a separate authority, allowed to erase dependencies only after all transcript members are removed.

Migration 119 installs relational constraints without changing existing Close rows or phases, fabricating lineage, retrying or deleting anything. Invalid existing keys fail rather than being silently rewritten. The guarantee is forward-only and specific to this evidence shape; no generic SQLite repair, downgrade, mixed-version or production-execution authority is granted.

## Consequences

- **Positive:** Fresh generations regain exact cleanup authority without discarding incident evidence or duplicating destructive ownership.
- **Positive:** Repair stays explicitly retryable and restart cannot silently authorize cleanup of a repair-gated attempt.
- **Negative:** Recognition and adoption require a narrowly maintained regression matrix; unsupported shapes remain manual repair inputs.
- **Negative:** Retained lineage requires ordered dependency deletion during authorized History hard deletion.
- **Neutral:** Qualification, deployment and coordinated production cleanup remain distinct from implementing this contract.

## References

- [Work lifecycle requirements](../work-lifecycle/requirements.md), REQ-WL-002c and REQ-WL-004–008
- [Work lifecycle behavior](../work-lifecycle/work-lifecycle.allium)
- [Compatibility requirements](../compatibility/requirements.md), REQ-COMP-001–004
- ADR-034: compatibility guarantees are explicit and data-aware
- ADR-040: Close uses WorkScope gates and tmux-only durable identity
- ADR-042: Close directory retirement trusts its private namespace
- `Database::adopt_close_worktree_cleanup_plan`, `Database::resume_legacy_fk787_close_retirement_generation`, `Database::retry_close_retirement`, `MIGRATION_119`
