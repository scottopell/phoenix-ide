# ADR-043: Creation staging uses a private locked namespace

- **Status:** Accepted
- **Date:** 2026-08-29
- **Affects:** REQ-CCR-005; product-creation worktree ownership and cleanup

## Context

Git-backed conversation creation materializes unpublished worktrees at deterministic paths beneath a repository's `.phoenix/worktrees` directory. Automatic cleanup needs to distinguish a resource created by the current accepted request from a pre-existing or replacement occupant. Durable request-bound ownership tokens and worktree-local owner markers provide that evidence, while the repository mutation lock serializes supported Phoenix writers.

A stronger threat model would require automatic deletion to remain safe despite arbitrary same-user processes replacing path components between ownership verification and Git worktree removal. That would require a substantially different filesystem transaction or quarantine design and would still not make ordinary Git commands participate in Phoenix's ownership protocol. The product instead needs a narrow, explicit trust boundary for its private staging namespace.

## Options considered

1. **Treat every same-user filesystem mutation as hostile** — build descriptor-relative recursive verification or quarantine before every cleanup. This broadens the deletion subsystem far beyond creation staging and cannot make external Git writers honor Phoenix's protocol.
2. **Infer ownership from deterministic paths** — delete whatever occupies the expected staging path. This is simple but permits stale workers to remove replacement or user-owned resources.
3. **Use a private namespace with locked Phoenix writers and exact ownership evidence** — serialize supported Phoenix mutations, persist request-bound ownership, verify the matching owner marker, and fail closed whenever observed identity differs.

## Decision

Phoenix treats deterministic creation-staging paths beneath a repository's `.phoenix/worktrees` directory as a Phoenix-private namespace. Every supported Phoenix writer that creates, adopts, reconciles, or removes those staging worktrees participates in the canonical repository mutation lock.

Automatic cleanup requires matching durable request-bound ownership and the matching worktree owner marker. A missing, unreadable, or mismatched marker, an unexpected occupant, or any other observed identity conflict produces a durable non-destructive cleanup-ambiguous outcome. Deterministic location alone is never ownership proof.

Arbitrary hostile mutation by another same-user process outside supported Phoenix writers is excluded from this automatic-cleanup threat model. Phoenix does not add a universal filesystem transaction manager or quarantine subsystem for creation staging.

## Consequences

- **Positive:** Stale Phoenix workers cannot delete replacement occupants merely because they reuse a deterministic path.
- **Positive:** Cleanup remains small and auditable: one repository lock, one durable owner token, one marker comparison, and a fail-closed ambiguity state.
- **Negative:** Unsupported same-user mutation can violate the private-namespace assumption; operators must resolve any resulting ambiguous cleanup rather than Phoenix guessing.
- **Neutral:** The boundary governs unpublished creation staging only. It does not grant GitRepository authority or change user-selected directory ownership.

## References

- `specs/conversation-creation/requirements.md` — REQ-CCR-005
- `specs/conversation-creation/conversation-creation.allium`
- ADR-007: Conversation creation uses fenced reconciliation
- ADR-044: Creation publication uses request-bound identity and immutable starting pins
- ADR-035: Repository authority activation is consumer-triggered and offline
