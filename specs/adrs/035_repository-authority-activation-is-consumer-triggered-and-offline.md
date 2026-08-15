# ADR-035: Repository authority activation is consumer-triggered and offline

- **Status:** Accepted
- **Date:** 2026-08-15
- **Affects:** REQ-GITREP-004, REQ-GITREP-009; `GitRepository`, `WorkScope.repository`, repository authority generation

## Context

The additive GitRepository Foundation established deterministic Project-seeded hidden identities and dormant WorkScope attachments while leaving `Project` as the sole live repository authority. Activating that dormant model was then framed as an independent hot in-process cutover.

A hot transition had to prove that every request path, worker, poller, terminal, browser session, MCP transport, one-shot runtime, deployment backend, and stale binary had stopped using Project authority before one process could change authority. It also attempted to observe Git state and converge linked-worktree identities during activation, and to bind a static source census into production authorization. The result was a whole-runtime coordination protocol for a private database authority change.

No live ProductConversation or destructive Close capability requires hidden GitRepository authority. Foundation availability by itself creates no user need to activate it. The project-wide compatibility contract also makes database replacement and version rollback offline operations rather than live compatibility protocols.

## Options considered

1. **Continue hot in-process activation.** Drain and fence the full runtime, bind source and database evidence, observe live Git state, converge linked identities, change authority, then resume admission. This minimizes planned downtime but imposes permanent cross-runtime coordination and recovery machinery before a product consumer exists.
2. **Perform offline activation immediately.** Stop Phoenix, run a bounded exclusive SQLite operation, and restart the GitRepository-authority binary even though no live capability requires the new authority. This removes runtime-drain complexity but still pays migration and operational cost ahead of user value.
3. **Activate offline only when an exact consumer contract requires it.** Keep Foundation dormant and Project authoritative. When an owning ProductConversation or destructive Close requirement explicitly requires generation `2`, stop Phoenix, acquire exclusive SQLite access, capture the exact pre-activation snapshot, preserve seeded identities, change authority transactionally, and start the matching binary.
4. **Abandon hidden GitRepository authority.** Leave Project authoritative permanently and reconsider the repository model if a consumer appears. This avoids migration cost but discards the accepted hidden-identity direction and the independently useful dormant Foundation.

## Decision

Phoenix uses option 3.

Repository authority activation is not an independent portfolio milestone. The dormant Foundation remains sufficient until an owning normative requirement for an exact ProductConversation or destructive Close capability requires hidden GitRepository authority generation `2`. The activation operation accepts that exact capability and requirement as a typed mandate; a broad consumer category or generic domain reference cannot authorize the transition.

Activation is offline-only. Phoenix is stopped and the maintenance operation acquires exclusive SQLite access. While that access is held and before any activation mutation, it captures and verifies a recoverable snapshot of the exact database state to be activated and pairs that snapshot with a Project-authority binary verified to operate it. Foundation validation and authority changes use that same exclusively held source state.

The operation preserves the deterministic Project-seeded GitRepository identity partition and existing WorkScope attachments. It does not observe live Git state or merge identities. Identity convergence is outside this decision; any future convergence requires its own atomic survivor-selection, lossless reference-rewrite, and losing-identity-retirement contract.

Because repository authority generation is global, the generation-2 binary must migrate every repository-sensitive reader and writer to GitRepository authority or structurally quarantine it from generation-2 operation. Migrating only the triggering consumer is insufficient. Every surviving Project-shaped repository value is read-only compatibility output or retained data and cannot feed a correctness-sensitive decision.

One SQLite transaction updates authority-bearing references and changes persisted repository authority generation from Project generation `1` to GitRepository generation `2`. The one offline operation binds the exact mandate and pre-activation snapshot to the exact committed database-state fingerprint and staged GitRepository-authority binary artifact without copying those external recovery artifacts into a second persisted representation inside the activated database. A failure before commit rolls the transaction back wholly. After commit, normal operation requires that generation-2 Phoenix binary; generation-1 Project-authority binaries fail closed. Recovery rolls forward with generation 2 or manually selects the exact pre-activation snapshot and paired Project-authority binary under the project-wide offline contract.

The repository-authority generation is feature state: it selects which repository model is writable. It is not the database-instance identity, live replacement fence, compare-and-swap token, or general compatibility mechanism rejected by ADR-033.

A source census may remain CI and review evidence that identifies repository-sensitive code. It is not production authorization and does not mint an activation capability. Phoenix does not implement runtime-wide drain, durable cross-backend cutover claims, terminal or transport quiescence, live authority switching, or automatic authority rollback for this transition.

This decision replaces ADR-032's coordinated live-reader/writer activation mechanism. ADR-032 remains authoritative for hidden opaque identity, single writable authority, observation semantics, WorkScope attachment, retained repair evidence, and the absence of a user-facing Repository product.

## Consequences

- **Positive:** Authority activation is bounded by process shutdown and one exclusive database transaction instead of whole-runtime distributed coordination.
- **Positive:** Migration work begins only when it unlocks a named user-facing capability.
- **Positive:** Seeded identities remain replay-stable; activation no longer depends on a live filesystem or Git observation.
- **Positive:** The offline operation verifies exact snapshot-and-binary recovery evidence instead of relying on compatibility alone.
- **Positive:** Wrong-generation binaries fail closed without requiring old processes and workers to participate in an in-process cutover protocol.
- **Negative:** Activation requires planned downtime.
- **Negative:** Dormant Foundation data remains temporary duplicate storage until a consumer justifies activation or the repository direction changes.
- **Negative:** Linked worktrees seeded from distinct Project identities remain distinct after activation unless a later feature explicitly converges them.
- **Neutral:** Independently valuable runtime correctness fixes discovered during the abandoned hot design require feature-owned review and delivery; their implementation effort does not justify activation.

## References

- ADR-032: GitRepository is hidden infrastructure; Project is retired.
- ADR-033: Database rollback is offline and Foundation observations use relational scalar storage.
- ADR-034: Compatibility guarantees are explicit and data-aware.
- `specs/compatibility/requirements.md`
- `specs/git-repository/requirements.md`
- `specs/git-repository/git-repository.allium`
