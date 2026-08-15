# Offline repository authority activation

## Blocker

Do not implement until an owning normative requirement for an exact ProductConversation or destructive Close capability explicitly requires hidden `GitRepository` authority generation `2` for correctness. Dormant Foundation availability, a broad consumer category, a generic domain reference, or infrastructure completeness does not unblock this task.

When that mandate exists, reconcile this task against its normative requirements before implementation and record the exact consumer capability and owning requirement here as typed operation input.

## Goal

Replace legacy `Project` repository authority with hidden `GitRepository` authority through one bounded offline maintenance operation that serves the named consumer without introducing live runtime coordination or changing repository identity topology.

Normative authority: REQ-GITREP-009, `RepositoryAuthorityActivationIsConsumerTriggeredAndOffline`, ADR-035, and `specs/compatibility/requirements.md`.

## Required operation

1. Construct a typed activation mandate referencing the exact consumer capability contract and owning normative requirement that requires generation `2`.
2. Stage the exact GitRepository-authority binary and offline maintenance operation.
3. Prove that binary migrates every repository-sensitive reader and writer to GitRepository authority or structurally quarantines it from generation-2 operation; migrating only the triggering consumer is insufficient.
4. Stop Phoenix and prove no Phoenix server, worker, one-shot runtime, or maintenance process has the target database open.
5. Acquire exclusive SQLite access.
6. While exclusive access remains held and before any activation mutation, capture and verify a recoverable snapshot of the exact database state to be activated, paired with a Project-authority binary verified to operate that snapshot.
7. Validate the dormant Foundation schema, deterministic Project-seeded GitRepository rows, and WorkScope attachments against that exact source state.
8. Preserve every seeded GitRepository identity and existing attachment; perform no identity convergence or live Git/filesystem observation.
9. In one SQLite transaction, update every authority-bearing database reference, make every surviving Project-shaped value incapable of feeding a correctness-sensitive decision, and change repository authority generation from `1` to `2`. The operation binds the exact mandate and snapshot inputs to the committed database-state fingerprint and staged generation-2 Phoenix binary artifact without copying external recovery artifacts into a second persisted representation inside the activated database.
10. Roll back the transaction wholly on any pre-commit validation, reference, migration-completeness, or activation failure.
11. Start the exact generation-2 Phoenix binary and prove every repository-sensitive journey uses GitRepository authority or remains structurally unavailable.
12. Prove Phoenix binaries that require generation `1` fail closed against the activated database.

## Acceptance evidence

- [ ] A typed activation mandate references the exact ProductConversation or destructive Close capability contract and owning normative requirement that requires generation `2`; the operation rejects broad categories and generic domain references.
- [ ] Offline exclusion is process-level and exclusive; no request, worker, terminal, browser, MCP, or deployment-backend drain protocol is required.
- [ ] Exclusive SQLite access is acquired before the exact pre-activation snapshot is captured and remains held through validation and commit or abort.
- [ ] The snapshot is identity-bound to the exact database state being activated and to a Project-authority binary artifact verified to restore its operation; the operation binds that input to the committed generation-2 database-state fingerprint and staged binary artifact without introducing parallel persisted recovery evidence.
- [ ] Foundation validation is query-only against that exact source state before the authority transaction and fails closed on missing, conflicting, or incomplete required state.
- [ ] A maintained repository-sensitive census proves every reader and writer is migrated to GitRepository authority or structurally quarantined from generation-2 operation; no frozen Project fact can feed a correctness-sensitive decision.
- [ ] The authority transaction preserves Project-seeded identities and WorkScope attachments without convergence, live observation, guessed continuity, dual writes, or fallback.
- [ ] Migration and Phoenix database-open tests prove the only transition is generation `1 → 2`, transaction failure leaves generation `1`, generation `2` cannot revert in place, and each wrong-generation Phoenix binary fails closed without bootstrapping or mutating the database.
- [ ] Post-commit recovery is roll-forward with generation `2` or manual offline selection of the exact pre-activation snapshot and its paired binary; no automatic restore-time validator, database-instance fence, or authority rollback subsystem is added.
- [ ] A static repository-authority census, if retained, is CI/review evidence only and cannot authorize production activation.
- [ ] Focused migration/opener/consumer tests, full spec checks, task validation, and `./dev.py check --all` pass on the exact implementation head.

## Non-goals

- Hot or in-process authority switching.
- Runtime-wide request/worker/poller/terminal/tmux/browser/MCP drain.
- Durable cross-backend exclusion claims or runtime capability minting.
- Live Git observation or linked-worktree identity convergence.
- ProductConversation or Close go-live merely because authority activation exists.
- Project UI retirement, physical Project schema deletion, or `specs/projects/` cleanup.
- General downgrade enforcement or deployment messaging owned by task 44016.
- Automatic rollback, live database replacement, or cross-version database compatibility.
