# ADR-061: GPT-6 Sol is manually qualified for parallel Work

- **Status:** Accepted
- **Date:** 2026-09-26
- **Affects:** REQ-SA-001, REQ-PROJ-008; `QualificationIsExplicitAndFailClosed`

## Context

ADR-060 establishes that parallel Work admission is an exact, fail-closed decision over the parent's resolved model identifier. The supported catalog now contains the additional exact identifiers `gpt-6-sol` and `gpt-6-luna`. Neither identifier inherits qualification from its family, generation, or name.

GPT-6 Sol is intended to coordinate multiple trusted Work collaborators. GPT-6 Luna remains a coding worker and selectable child model, but its parent orchestration behavior remains sequential.

## Options considered

1. **Qualify both GPT-6 models** — simple family policy, but grants orchestration authority to Luna without model-specific qualification.
2. **Leave both models unqualified** — preserves fail-closed behavior but withholds intended orchestration authority from GPT-6 Sol.
3. **Qualify only exact `gpt-6-sol`** — preserves manual qualification and Luna's sequential-parent role while allowing Sol to orchestrate parallel Work.

## Decision

Add exact `gpt-6-sol` to the parallel Work allowlist. Keep exact `gpt-6-luna` unqualified. Apply the decision at every reasoning effort supported by each model.

No family, version, prefix, service tier, reasoning effort, child model, or configuration tier may infer qualification. Unknown and newly introduced identifiers remain unqualified until another explicit decision names them.

## Consequences

- **Positive:** GPT-6 Sol parents can admit multiple Work children, including multiple Luna children, through the durable admission lifecycle.
- **Positive:** GPT-6 Luna remains selectable without receiving parent orchestration authority.
- **Negative:** Future model identifiers require another explicit qualification decision and matching tests.
- **Neutral:** Existing admitted children and in-flight runtime identity are unchanged.

## References

- ADR-060: Parallel Work admission is explicitly model-qualified
- `supports_parallel_work_subagents`
- `specs/subagents/requirements.md`
- `specs/subagents/subagents.allium`
