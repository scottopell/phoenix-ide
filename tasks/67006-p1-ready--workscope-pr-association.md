# WorkScope-owned PR association

## Summary

Persist GitHub PR association as WorkScope-owned history, then use that association in PR status, PR auto-fix context, and abandon/cleanup messaging.

This follows up on:

- PR #140 / `tasks/27109-p2-done--pr-ci-monitoring-autofix.md`, which added PR/CI monitoring and auto-fix context but still discovers PRs from the current branch/worktree.
- `tasks/67005-p2-done--closed-pr-abandon-cleanup.md`, which fixed WorkActions UI copy for closed-unmerged PRs but intentionally paused durable PR association and backend abandon semantics until WorkScope landed.
- WorkScope resource ownership from PRs #136/#139.

## Load-bearing decisions already made

- **Owner:** `WorkScope` owns PR association.
- **Shape:** persist PR history for the WorkScope plus a derived primary PR.
- **Discovery:** persist associations on observation from `/pr-status` and related PR-monitoring flows.
- **Primary selection:** most actionable PR wins:
  1. open non-draft
  2. draft
  3. merged
  4. closed
  5. tie-break by GitHub `updated_at`, then `last_seen_at`

## Proposed data model

Add a migration-backed table along these lines:

```text
work_scope_pr_associations
  scope_type              -- Worktree | Conversation, matching WorkScope
  scope_value             -- WorkScope value
  repo_owner
  repo_name
  pr_number
  title
  url
  state
  draft
  display_state
  base
  head
  github_updated_at
  first_seen_at
  last_seen_at
```

Use a uniqueness constraint over `(scope_type, scope_value, repo_owner, repo_name, pr_number)`.

Do not store this only in JSON text. This is durable schema, so add a real migration.

## Backend behavior

### `/api/conversations/:id/pr-status`

- Resolve the conversation's `WorkScope`.
- Fetch PR observations using the PR-monitoring code from PR #140.
- Upsert all observed PRs for the WorkScope.
- Derive the primary PR from persisted history plus fresh observations using the "most actionable PR wins" rule.
- Return the primary PR in the existing `PrStatusResponse` shape so StateBar and WorkActions keep a single obvious PR to render.
- Preserve existing unavailable behavior (`gh_missing`, `not_authenticated`, `not_git_repo`, `command_failed`). If refresh is unavailable but a persisted primary exists, decide explicitly whether the response should include stale persisted PR data plus an unavailable marker or only unavailable. Do not let stale-vs-fresh be ambiguous.

### `/api/conversations/:id/pr-auto-fix-context`

- Prefer the WorkScope primary PR when present.
- Refresh/persist observations if the current worktree is still available.
- Avoid relying solely on `gh pr list --head <branch>` once an association exists.
- Continue to write the typed PR context artifact under the worktree as PR #140 does today.

### Abandon / cleanup messaging

Use known PR association history to improve terminal messages without changing cleanup semantics:

- Work mode: `Task abandoned. Worktree and branch deleted. PR #133 preserves history.`
- Branch mode: `Abandoned. Worktree removed, branch kept. PR #133 preserves history.`

Preserve the Work vs Branch distinction:

- Work abandon deletes the Phoenix worktree and Phoenix-created task branch.
- Branch abandon deletes the worktree but keeps the user branch.

Closed-unmerged PRs remain abandonable cleanup, not "waiting to merge" work.

## UI behavior

- StateBar/PR popover should show the derived primary PR.
- WorkActions should keep the existing closed-unmerged guidance from task 67005.
- If persisted history contains only closed PRs, the primary can be closed for abandon/history messaging, but mark-as-merged must remain unavailable/disabled.
- If a new open PR is observed for the same WorkScope, it becomes primary over older closed PRs.

## Tests

- DB migration/unit tests for upsert, `first_seen_at`, `last_seen_at`, and uniqueness.
- Primary-selection tests covering open vs draft vs merged vs closed and updated-at tie-breaks.
- `/pr-status` tests: observing PRs persists them under the resolved WorkScope and returns the derived primary.
- Continuation test: parent and continuation resolving to the same WorkScope see the same PR history/primary.
- Auto-fix context test: uses associated primary PR when present.
- Abandon messaging tests for Work and Branch modes with known PR association.
- Regression: open/draft PRs still block mark-as-merged cleanup until merged; closed-unmerged PRs still point users to Abandon.

## Non-goals

- Do not implement webhooks or a background scanner in this task.
- Do not replace the 100 KiB abandon diff snapshot yet. A known PR may supplement the snapshot, but removing the snapshot needs a separate decision.
- Do not add a manual PR attach/switch UI unless primary-selection ambiguity forces it.
