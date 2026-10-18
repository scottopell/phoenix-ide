# ADR-125: Qualify one exact prepared source114 candidate

- **Status:** Accepted
- **Date:** 2026-10-17
- **Affects:** REQ-COMP-012, REQ-PD-021, REQ-LDD-019–022, REQ-LDD-024

## Context

Prepared current-schema launchd admission qualifies exact prefix123 or exact target128. ADR-115 separately qualifies published-release prefix114. The commissioned prepared artifact is candidate `23badb8be4bc8d05a7e1c49c5367ceb14ebccd92` with migration source SHA-256 `a4eb7e213b363892f316f0034bc45b52e809124e85394730705599cecb257d6f`. Its semantic migration registry is unchanged from RC4, but a prepared artifact is not a published release: unchanged migrations alone cannot authorize a new source-kind/candidate admission under ADR-034. A newer clean controller must supply the admission helper without substituting its own registry or rebuilding/resigning the prepared candidate.

## Options considered

1. Generalize all prepared candidates to source114: rejected; expands compatibility beyond the commissioned candidate.
2. Pretend the prepared artifact is a published release or require controller=candidate: rejected; misstates provenance or prevents the newer helper from controlling the retained exact candidate.
3. Add one typed exact-candidate prepared qualification, preserving protected lifecycle: selected.

## Decision

Admit exact complete prefix114 or exact target128 only for the full receipt/source/expected/embedded SHA above, the exact candidate git-object registry digest above, and the exact parsed registry128. Use `PreparedSource114Upgrade`, with mandatory integer `prepared_source_baseline: 114` and fixed `candidate_registry_expected_sha256`, inside `current_schema_upgrade`. Reject partial/extra/mistyped or contradictory fields before DB admission/stop. Without both fields, retain generic prepared123/target128 semantics.

The unchanged public prepared current-schema command automatically emits this evidence for that receipt commit only. Obtain migration source through `git show FULL_CANDIDATE_SHA:crates/phoenix-db/src/migrations.rs`; the candidate commit must be locally present. Never read controller working-tree registry bytes. Retain the newer clean controller commit and hash-bound helper independently; it must differ from the candidate. Preserve all existing receipt, signing, notarization and exact-binary identity checks.

Reuse capacity, offline snapshot, exclusivity, durable `migration_started`, candidate-only post-invocation recovery and finalization unchanged. Bind typed qualification into snapshot proof and reject tampering/removal during recovery. Once invoked, migration failure, partial writes, or startup failure never permit predecessor startup or snapshot replay. Keep published-release114, ordinary activation, generic prepared123 and target admission distinct.

## Consequences

- One explicit source-kind/candidate expansion; no generic prepared114, source range, new target or registry semantics.
- Operators need the exact prepared artifact plus candidate git object and a different clean newer controller checkout; no local candidate rebuild is needed.
- Authored regressions and static Python/diff validation do not establish runtime/deployment acceptance; hosted CI and exact-head review remain qualification gates.

## References

- [Compatibility requirements](../compatibility/requirements.md), REQ-COMP-012
- [Production requirements](../production-deployment/requirements.md), REQ-PD-021
- [Launchd requirements](../launchd-deployment/requirements.md), REQ-LDD-019–024
- ADR-034, ADR-102, ADR-105, ADR-114, ADR-115
