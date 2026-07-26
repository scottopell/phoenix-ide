Child of task 92010.

Concern:
- Migrate any affected legacy design.md content into spEARS v2 homes so lifecycle-related rationale, behavior, and status content live in the correct artifact types.

Settled terminology guardrails:
- Preserve the settled lifecycle vocabulary while migrating text; do not copy legacy aliases forward unless they are explicitly marked as historical.
- Requirements remain timeless; ADRs hold rationale/history; Allium holds precise behavior; executive docs hold status/current reality.
- Use “legacy design.md” only to describe the source artifact, not the normative destination terminology.
- Do not let migrated prose reintroduce conflicting authority terms.

Done evidence:
- Every affected legacy design.md section is either migrated to the right v2 artifact or intentionally left with a documented reason.
- No migrated timeless artifact contains task IDs, implementation-status language, or decision-log prose.
- Cross-references among requirements, ADRs, Allium, and executive docs resolve cleanly after migration.
- Validation notes identify which specs were migrated and where each content class landed.
