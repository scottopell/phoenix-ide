# ADR-032: GitRepository is hidden infrastructure; Project is retired

- **Status:** Accepted
- **Date:** 2026-08-11
- **Affects:** REQ-GITREP-001, REQ-GITREP-002, REQ-GITREP-003, REQ-GITREP-004, REQ-GITREP-005, REQ-GITREP-006, REQ-GITREP-007, REQ-GITREP-008, REQ-PROJ-015, REQ-PROJ-020, REQ-PROJ-021, REQ-PROJ-024, REQ-PROJ-025, REQ-PROJ-028a, REQ-WL-002b, `GitRepository`, `WorkScope.repository`, `RestartRepairEvidence`, `RetirementAbsenceEvidence`

## Context

ADR-026 separates ProductConversation lifecycle, transcript topology, and WorkScope resource ownership. ADR-031 then establishes first-class ProductConversation persistence with staged single authority. The remaining repository-backed gap is that Phoenix still talks about "projects" in places where it really needs a hidden local repository identity plus truthful retained evidence about a worktree that disappeared or became inaccessible.

The repository concept here is not a user-facing product. Phoenix needs it only so provisioning, branch observation, mutation locking, orphan recovery, and restart repair can reason about one local repository without guessing from mutable paths or remote strings. The old `Project` vocabulary encouraged exactly that confusion: path looked like identity, branch looked like lifecycle, and deletion of one conversation looked too close to deletion of the repository fact Phoenix still needed.

The restart-repair path sharpens the decision. When Phoenix restarts and finds that a registered worktree is missing or inaccessible, later Close or retry logic needs retained evidence. If Phoenix rewrites that evidence in place, or infers sameness from a reappearing path, it collapses "what Phoenix observed" into "what later cleanup guessed," which makes fail-closed adoption impossible.

## Options considered

1. **Keep repository identity implicit in canonical paths and legacy project rows.** This avoids a new hidden identity, but it keeps mutable path strings as authority and makes linked-worktree sameness or separate-clone distinctness depend on guesswork.
2. **Use remote URL or repository slug as hidden repository identity.** This feels stable across paths, but it incorrectly merges separate local clones and turns off-host metadata into local identity authority.
3. **Rewrite restart-repair observations in place as cleanup learns more.** This keeps fewer rows, but it destroys the distinction between source observation and later adoption/repair decisions.
4. **Delete hidden repository facts with the conversation that first created them.** This makes ownership look simple, but it breaks orphan recovery, default-branch observation history, repository-scoped mutation locking, and later repair flows that still need repository truth after one aggregate is gone.
5. **Introduce a hidden opaque GitRepository identity, keep repository facts staged behind single authority, and preserve restart repair as immutable evidence adopted by later Close attempts.** This separates local repository identity from user-facing product surfaces, keeps one writable authority per repository fact, and lets lifecycle consume retained evidence without guessing continuity.

## Decision

Phoenix adopts **hidden opaque `GitRepository` identity** as the local repository authority. That identity is Phoenix-local and distinct from path strings, remote URLs, slugs, titles, or user-facing labels. Linked worktrees that share one Git common directory may resolve to the same hidden repository identity; separate clones remain distinct identities even when they point at the same remotes or contain the same commits.

Repository-adjacent paths are **mutable locator observations**, not identity. Phoenix records Git common directory and management-root locators with explicit `present`, `missing`, or `inaccessible` status. Locator updates never mint a replacement hidden identity by themselves, and missing/inaccessible are distinct observations rather than one generic "gone" state.

Canonical default branch is an **optional observation with provenance**. Phoenix records default-branch evidence from `remote_head_cache`, `local_checked_out_branch`, or `user_selected`, with an observation time. A checked-out branch fallback is not proof of the remote default, and Phoenix never fabricates `main`.

Repository attachment flows through **one singular nullable `WorkScope.repository` authority**. ProductConversation derives repository context from its attached WorkScope; it does not gain an independent writable ProductConversation-to-repository relation. Pre-scope provisioning evidence may still reference the hidden repository directly when no WorkScope exists yet, because inventing a scope to carry repository identity would create false work ownership.

Restart repair records **immutable retained evidence**: ProductConversation, WorkScope, hidden GitRepository, worktree identity/path/fingerprint, exact `missing` or `inaccessible` observation kind, observation generation, and observation time. Later Close/retry logic does not rewrite that source evidence. Instead, an exact Close attempt may adopt it only when identity is complete, the same ProductConversation/WorkScope/repository/worktree fingerprint still matches, no replacement appeared, and the attempt is the exact active one. Adoption creates new attempt-bound retirement evidence plus an explicit adoption relation; conflict remains typed repair.

Migration stays staged and single-authority:

- Deterministic backfill may seed `GitRepository.id` from the corresponding legacy `Project.id`; that byte equality is migration-only and never authorizes substituting one identity domain for the other at runtime.
- A dormant nullable `WorkScope.repository` attachment may be populated from legacy rows before cutover. Conflicting legacy Project assignments within one WorkScope are a migration error, not an invitation to select one heuristically.
- Legacy `project`-named readers and compatibility outputs may continue before cutover, but they are compatibility carriers, not the new authority.
- Hidden `GitRepository` identity and restart-repair evidence may be introduced before every consumer moves, but repository identity, default branch, and scope attachment each have exactly one writable authority; Phoenix never dual-writes Project and GitRepository representations.
- `WorkScope.repository` becomes authoritative only in the coordinated cutover that moves every relevant reader and writer together. Old binaries or stale workers that could recreate or mutate Project authority are excluded before writer cutover.
- Project readers, UI grouping, and API collection are removed before legacy Project storage is dropped.
- Restart-repair adoption lands with Close/retry consumers only when those consumers can fail closed on exact identity. Before that cutover, retained evidence may exist without being the shipped resolution path.
- Conversation deletion removes only aggregate-owned rows; hidden repository identity survives when Phoenix still needs it for orphan recovery, default-branch observation, restart repair, or repository-scoped mutation locking.

Phoenix does **not** create a new user-facing repository product. Hidden repository identity owns no title, grouping UI, lifecycle, task inventory, branch inventory, or PR workflow. Repository surfaces remain observational context used by other product flows.

## Consequences

- **Positive:** Phoenix can distinguish hidden repository identity from mutable locator paths and from remote metadata, which prevents path-based continuity guessing.
- **Positive:** Restart repair now has an immutable evidence source that later Close attempts can adopt fail-closed without rewriting history.
- **Positive:** Separate clones stay distinct while linked worktrees can share one hidden local identity.
- **Positive:** Conversation deletion no longer threatens repository facts Phoenix still needs for recovery or locking.
- **Negative:** The migration must thread another hidden domain concept through provisioning, observation, repair, and deletion without creating dual-write authorities.
- **Negative:** Compatibility code will keep old `project` names for a while, so readers must distinguish legacy names from actual authority.
- **Neutral:** Hidden repository identity may remain entirely invisible to end users even though it becomes a stronger internal contract.
- **Neutral:** Some restart-repair evidence may exist before Close/retry consumers fully adopt it.

## References

- Related ADRs: ADR-026, ADR-031
- ADR-035 replaces this ADR's coordinated live-reader/writer activation mechanism with consumer-triggered offline activation; the hidden-identity and single-authority decisions remain in force.
- Specs: `specs/git-repository/requirements.md`, `specs/git-repository/git-repository.allium`, `specs/work-lifecycle/requirements.md`, `specs/work-lifecycle/work-lifecycle.allium`, `specs/conversation-creation/requirements.md`
- Key symbols: `GitRepository`, `WorkScope.repository`, `RestartRepairEvidence`, `RetirementAbsenceEvidence`, `CloseObligation`
