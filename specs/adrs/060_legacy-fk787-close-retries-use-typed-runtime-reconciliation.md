# ADR-060: Legacy FK787 Close retries use typed runtime reconciliation

- **Status:** Accepted
- **Date:** 2026-09-16
- **Affects:** REQ-WL-002b; Close retirement compatibility

## Context

A deployed Close retry could persist a fresh inspection generation before retirement continuation. If cleanup-plan persistence then failed with SQLite foreign-key code 787, repair routing persisted a `manual_repair_required` residual under that fresh generation. The obligation consequently pointed at a partial generation with inspection, sealed inventory, and residual evidence but no dispatch or cleanup plan, while the prior generation retained identity-exact dispatch and cleanup authority.

Exact-inventory hardening prevents current code from producing that shape, but an already-persisted attempt cannot pass ordinary reinspection: repair inventory insertion is rejected because the obligation retains a non-null active snapshot. ADR-059 deliberately did not authorize automatic retry of an existing production attempt, so preserving this persisted state requires an explicit compatibility decision under ADR-034.

A database migration cannot safely finish retirement because migrations cannot freshly validate the retained filesystem identity, administrative-directory incarnation, or ambient writers.

## Options considered

1. **Rewrite or delete the partial generation in a migration** — makes current code accept the attempt, but destroys historical evidence and cannot validate live filesystem state.
2. **Broaden generic retry semantics for any unattempted generation** — may converge this case, but makes unrelated malformed or ambiguous states destructive-cleanup candidates.
3. **Recognize only the typed legacy FK787 shape at the lifecycle boundary** — preserves history, fails closed for every other shape, and delegates live identity validation, exact prior-authority adoption, and cleanup to the ordinary executor.

## Decision

Choose option 3, narrowly superseding ADR-059 only where its no-existing-production-attempt consequence conflicts with this explicit compatibility guarantee.

Ordinary startup or user-authorized resume may atomically advance an `awaiting_retirement_inspection` attempt to `retirement_requested` only when every captured WorkScope has an active inspection and exactly one identity-matching `manual_repair_required` residual containing SQLite code 787, the active generation has sealed inventory but no dispatch or cleanup plan, and an older generation of the same attempt has identity-exact dispatch plus cleanup-plan authority for every captured worktree.

The reconciliation does not alter historical evidence. The ordinary retirement executor must freshly inspect the retained worktree, validate administrative-directory identity and ambient writers, atomically adopt the prior dispatch/plan pair, and route any mismatch back to typed repair.

## Consequences

- **Positive:** The known persisted legacy attempt can converge through supported lifecycle operations without database surgery or fabricated retirement.
- **Positive:** Exact-shape recognition keeps unrelated repair states fail-closed.
- **Negative:** Runtime retains a narrowly named compatibility recognizer for the lifetime of the supported persisted state.
- **Neutral:** This does not authorize automatic production mutation, deployment, or retry; those remain separate operator decisions.

## References

- ADR-034, ADR-059
- `specs/compatibility/requirements.md`
- `specs/work-lifecycle/requirements.md`
- `specs/work-lifecycle/work-lifecycle.allium`
- `Database::resume_legacy_fk787_close_retirement_generation`
- `RuntimeManager::inspect_close_retirement_with_continuation`
