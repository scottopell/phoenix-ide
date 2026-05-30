# WorkScope-owned PR association

## Summary

Persist GitHub PR association as normalized WorkScope-owned history, populate that history whenever Phoenix reasonably observes PR facts, then use the associated primary PR in PR status, PR auto-fix context, and abandon/cleanup messaging.

This follows up on:

- PR #140 / `tasks/27109-p2-done--pr-ci-monitoring-autofix.md`, which added PR/CI monitoring and auto-fix context but still discovers PRs primarily from the current branch/worktree.
- `tasks/67005-p2-done--closed-pr-abandon-cleanup.md`, which fixed WorkActions UI copy for closed-unmerged PRs but intentionally paused durable PR association and backend abandon semantics until WorkScope landed.
- WorkScope resource ownership from PRs #136/#139.

## Load-bearing decisions

- **Owner:** `WorkScope` owns PR association.
- **Persistence shape:** introduce a normalized `work_scopes` table and store PR associations by `work_scope_id`.
- **First durable WorkScope component:** this task should make PR association the first persisted WorkScope-owned component. Existing browser/tmux WorkScope usage appears to be runtime/in-memory ownership rather than durable DB rows; double-check before implementation, but do not invent migrations for non-persisted resources.
- **History model:** persist PR history for the WorkScope plus derive a primary PR.
- **Observation principle:** knowledge should populate history. If Phoenix has PR facts or can reasonably acquire them in an existing PR-related flow, it should upsert association history.
- **Observation flows:** `/pr-status`, `/pr-auto-fix-context`, and abandon/cleanup best-effort refresh may all observe and upsert PR association history.
- **Primary selection:** most actionable PR wins:
  1. open non-draft
  2. draft
  3. merged
  4. closed
  5. tie-break by GitHub `updated_at`, then `last_seen_at`
- **Stale status:** if refresh is unavailable but a persisted primary exists, return stale PR data plus an explicit refresh-unavailable reason. Do not make stale-vs-fresh ambiguous.
- **Auto-fix targeting:** auto-fix should target the WorkScope-associated primary PR first, then refresh PR/check/review data before writing the context artifact. Branch discovery is a fallback when no association exists.
- **Cleanup semantics:** abandon/cleanup may do a bounded, non-fatal best-effort PR refresh before deleting resources, but PR lookup failure must never block cleanup.

## Proposed data model

Add migration-backed tables along these lines:

```text
work_scopes
  id
  scope_type              -- Worktree | Conversation, matching WorkScope
  scope_value             -- WorkScope value
  created_at
  updated_at
```

```text
work_scope_pr_associations
  work_scope_id           -- FK to work_scopes(id)
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

Use uniqueness constraints over:

```text
work_scopes(scope_type, scope_value)
work_scope_pr_associations(work_scope_id, repo_owner, repo_name, pr_number)
```

Do not store this only in JSON text. This is durable schema, so add real migrations and DB helpers.

## Backend behavior

### Shared DB/API helpers

- Resolve the conversation's `WorkScope` using the existing WorkScope rules.
- Upsert or fetch the corresponding row in `work_scopes`.
- Upsert PR observations into `work_scope_pr_associations`.
- Preserve `first_seen_at` on repeated observations and update `last_seen_at` every time a PR is observed.
- Store GitHub `updated_at` separately from Phoenix `last_seen_at`.
- Derive the primary PR from persisted history using the primary-selection rule.
- Refactor PR-monitoring code so branch-based discovery and PR-number-based refresh can both produce typed PR observations suitable for persistence.

### `/api/conversations/:id/pr-status`

- Resolve the conversation's `WorkScope`.
- Fetch PR observations using the PR-monitoring code from PR #140.
- Upsert all observed PRs for the WorkScope.
- Derive the primary PR from persisted history plus fresh observations.
- Return PR status in a freshness-aware shape that separates PR identity from refresh metadata.
- If refresh succeeds, return the fresh primary PR and refresh metadata indicating success.
- If refresh is unavailable (`gh_missing`, `not_authenticated`, `not_git_repo`, `command_failed`) but a persisted primary exists, return the persisted primary PR plus refresh metadata indicating unavailable/stale and the reason.
- If refresh is unavailable and no persisted primary exists, return unavailable with no PR.
- Preserve enough compatibility or migrate UI/types cleanly so StateBar and WorkActions keep a single obvious PR to render.

Suggested response concept:

```text
pr: { number, title, url, state, draft, display_state, base, head, updated_at, ... } | null
refresh: {
  state: fresh | unavailable | not_found
  reason?: gh_missing | not_authenticated | not_git_repo | command_failed
  last_attempted_at
  last_refreshed_at?
  stale: bool
}
```

The exact Rust/TypeScript shape can differ, but stale-vs-fresh must be structural and unambiguous.

### `/api/conversations/:id/pr-auto-fix-context`

- Resolve the conversation's `WorkScope`.
- Prefer the WorkScope-associated primary PR when present.
- Refresh that PR by number before writing the typed PR context artifact.
- If no association exists, fall back to current branch-based discovery.
- Upsert any PR facts learned by auto-fix into WorkScope PR history.
- Continue to write the typed PR context artifact under the worktree as PR #140 does today.
- Auto-fix remains available only for open, non-draft PRs.
- Avoid relying solely on `gh pr list --head <branch>` once an association exists.

### Abandon / cleanup messaging

Use known PR association history to improve terminal messages without changing cleanup semantics.

Before cleanup, when the worktree is still available, attempt a bounded best-effort PR refresh/upsert. This refresh must be:

- non-fatal;
- deadline-bounded;
- logged on failure/timeout;
- unable to prevent abandon/cleanup from completing.

Then derive the primary PR from persisted history and include it in the terminal message when known:

- Work mode: `Task abandoned. Worktree and branch deleted. PR #133 preserves history.`
- Branch mode: `Abandoned. Worktree removed, branch kept. PR #133 preserves history.`

Preserve the Work vs Branch distinction:

- Work abandon deletes the Phoenix worktree and Phoenix-created task branch.
- Branch abandon deletes the worktree but keeps the user branch.

Closed-unmerged PRs remain abandonable cleanup, not "waiting to merge" work.

## UI behavior

- StateBar/PR popover should show the derived primary PR.
- StateBar/PR popover should distinguish fresh PR data from stale persisted PR data when refresh is unavailable.
- WorkActions should keep the existing closed-unmerged guidance from task 67005.
- If persisted history contains only closed PRs, the primary can be closed for abandon/history messaging, but mark-as-merged must remain unavailable/disabled.
- If a new open PR is observed for the same WorkScope, it becomes primary over older closed PRs.
- Auto-fix UI/action should align with the associated primary PR so users do not see one PR in StateBar and fix a different PR.

## Tests

- DB migration/unit tests for `work_scopes` uniqueness and lookup.
- DB migration/unit tests for PR association upsert, `first_seen_at`, `last_seen_at`, and uniqueness.
- Primary-selection tests covering open vs draft vs merged vs closed and updated-at / last-seen tie-breaks.
- `/pr-status` tests:
  - observing PRs persists them under the resolved WorkScope;
  - response returns the derived primary;
  - refresh unavailable plus persisted primary returns stale PR plus unavailable reason;
  - refresh unavailable without persisted primary returns unavailable with no PR.
- Continuation test: parent and continuation resolving to the same WorkScope see the same PR history/primary.
- Auto-fix context tests:
  - uses associated primary PR when present;
  - refreshes by PR number before writing the context artifact;
  - falls back to branch discovery when no association exists;
  - upserts learned PR facts.
- Abandon messaging tests for Work and Branch modes with known PR association.
- Cleanup refresh tests:
  - best-effort observation can enrich the final message;
  - timeout/failure does not block cleanup and is logged.
- Regression: open/draft PRs still block mark-as-merged cleanup until merged; closed-unmerged PRs still point users to Abandon.
- UI/schema tests for stale-vs-fresh rendering if existing frontend test coverage supports it.

## Non-goals

- Do not implement webhooks or a background scanner in this task.
- Do not replace the 100 KiB abandon diff snapshot yet. A known PR may supplement the snapshot, but removing the snapshot needs a separate decision.
- Do not add a manual PR attach/switch UI unless primary-selection ambiguity forces it.
- Do not migrate runtime-only WorkScope resources into the database unless implementation discovers an existing durable WorkScope-owned DB representation that would otherwise conflict with `work_scopes`.
