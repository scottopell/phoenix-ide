# Offline repository authority activation

## Blocker

Do not implement until a named live ProductConversation or destructive Close capability has a correctness contract that cannot be satisfied while legacy `Project` remains the sole repository authority. Dormant Foundation availability, infrastructure completeness, or work preserved on the rejected hot-Cutover branch does not unblock this task.

When that consumer exists, reconcile this task against its normative requirements before implementation and name the exact consuming journey here.

## Goal

Replace legacy `Project` repository authority with hidden `GitRepository` authority through one bounded offline maintenance operation that serves the named consumer without introducing live runtime coordination or changing repository identity topology.

Normative authority: REQ-GITREP-009, `RepositoryAuthorityActivationIsConsumerTriggeredAndOffline`, ADR-035, and `specs/compatibility/requirements.md`.

## Required operation

1. Stop Phoenix and prove no Phoenix server, worker, one-shot runtime, or maintenance process has the target database open.
2. Verify a recoverable database backup paired with the Project-authority binary.
3. Stage the exact GitRepository-authority binary and offline maintenance operation.
4. Acquire exclusive SQLite access.
5. Validate the dormant Foundation schema, deterministic Project-seeded GitRepository rows, and WorkScope attachments against the then-current Project authority.
6. Preserve every seeded GitRepository identity and existing attachment; perform no linked-worktree convergence or live Git/filesystem observation.
7. In one SQLite transaction, update every authority-bearing reference required by the named consumer, reject legacy Project authority writes, and change persisted repository authority generation from `1` to `2`.
8. Roll back the transaction wholly on any pre-commit validation, reference, or activation failure.
9. Start the exact generation-2 binary and prove normal GitRepository-authority journeys work.
10. Prove generation-1 Project-authority binaries and alternate database openers fail closed against the activated database.

## Acceptance evidence

- [ ] The task names the live ProductConversation or destructive Close consumer that requires activation and links its normative requirement.
- [ ] Offline exclusion is process-level and exclusive; no request, worker, terminal, browser, MCP, or deployment-backend drain protocol is required.
- [ ] The paired pre-activation backup is verified before database mutation.
- [ ] Foundation validation is query-only before the authority transaction and fails closed on missing, conflicting, or incomplete required state.
- [ ] The authority transaction preserves Project-seeded identities and WorkScope attachments without convergence, live observation, guessed continuity, dual writes, or fallback.
- [ ] Migration and opener tests prove the only transition is generation `1 → 2`, transaction failure leaves generation `1`, generation `2` cannot revert in place, and each wrong-role binary/opener fails closed without bootstrapping or mutating the database.
- [ ] Post-commit recovery is roll-forward with generation `2` or an offline paired restore; no automatic authority rollback subsystem is added.
- [ ] A static repository-authority census, if retained, is CI/review evidence only and cannot authorize production activation.
- [ ] Focused migration/opener/consumer tests, full spec checks, task validation, and `./dev.py check --all` pass on the exact implementation head.

## Non-goals

- Hot or in-process authority switching.
- Runtime-wide request/worker/poller/terminal/tmux/browser/MCP drain.
- Durable cross-backend Cutover claims or capability minting.
- Live Git observation or linked-worktree identity convergence.
- ProductConversation or Close go-live merely because authority activation exists.
- Project UI retirement, physical Project schema deletion, or `specs/projects/` cleanup.
- General downgrade enforcement or deployment messaging owned by task 44016.
- Automatic rollback, live database replacement, or cross-version database compatibility.
- Wholesale merge of `task-59004-repository-authority-cutover`; any independently valuable runtime fix requires its own feature-owned task and PR.
