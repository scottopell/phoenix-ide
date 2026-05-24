# Treat closed-unmerged PRs as abandonable cleanup

Closed-unmerged PRs represent abandoned work, not work waiting to be merged. For now, update Phoenix’s Work actions UI copy only; pause any backend lifecycle or PR-association work until the WorkScope stack lands.

## Active scope: Phase 1 UI correctness only

- When PR status is `closed` and not `merged`, do not show “cleanup unlocks after GitHub reports merged”.
- Keep “Mark as Merged” disabled/unavailable for closed-unmerged PRs.
- Make “Abandon” the clear next action for closed-unmerged PRs, with explanatory copy such as:
  - `PR #133 is closed without merge. Use Abandon to clean up local Phoenix state.`
- Preserve existing behavior for open/draft PRs: mark-as-merged cleanup remains blocked until merged.

This phase can rely on the existing `/pr-status` fetch because it only affects visible UI copy and button guidance.

## Explicitly paused: PR association and backend abandon semantics

Do not implement durable PR association or backend abandon changes in this task.

There is an existing stack designed to store worktree-associated data called `WorkScope`. After that stack merges, revisit how git branches and PRs relate to worktrees/conversations, then restack a follow-up design if needed.

Paused follow-up ideas:

- Represent which PR(s) are associated with a Work/Branch conversation or WorkScope.
- Decide whether association means “latest PR for branch”, “all PRs ever observed for branch”, or something else.
- Decide when Phoenix discovers and persists PR associations.
- Consider backend abandon messaging such as:
  - `Task abandoned. Worktree and branch deleted. PR #133 preserves history.`
- Consider whether a known closed PR can safely replace the 100 KiB local diff snapshot.

## Tests

- UI test: closed PR renders “use Abandon” guidance instead of “cleanup unlocks after merged”.
- UI regression: open/draft PRs still block mark-as-merged cleanup until merged.
- No backend/model PR association tests in this task; that work is intentionally paused pending WorkScope.

## Notes

Work mode abandon deletes the Phoenix worktree and Phoenix-created task branch. Branch mode abandon deletes the worktree but keeps the user branch. Any future PR-history messaging must preserve that distinction.
