# Backend-owned PR feedback background poll

## Problem

Task 35397 stabilized the Work Actions primary action structurally so cached open PRs keep `Address feedback` as the primary and only change the non-primary link-out as fresh status arrives. That removes the unsafe last-second button swap, but it does not implement the full backend-owned GitHub PR feedback sync daemon originally envisioned.

Today PR association/freshness data is still primarily refreshed by open-time `GET /api/conversations/:id/pr-status` calls and by PR-context capture. A background poll would make conversation list / opened conversation snapshots fresher before the user navigates, improving fast scanning for work needing attention.

## Desired behavior

- Backend polls active work scopes with cached/active PR associations approximately every 5 minutes with ±90s jitter.
- Poll persists structured PR status / check / feedback-freshness / feedback-coverage snapshot data, rather than overloading `CachedPrSummary`.
- Conversation list and conversation payloads expose enough of that persisted snapshot to seed Work Actions and PR badges without waiting for an open-time fetch.
- Poll failures persist explicit freshness/availability metadata and never block conversation loading.
- Connected clients are nudged/refreshed when a poll changes a scope's PR snapshot.
- `create_pr_auto_fix_context` remains the final fresh capture path before sending an auto-fix message.

## Implementation notes

- Model persisted data relationally where fields are independently updated/read; avoid a JSON blob for queried status/freshness fields.
- Do not create a parallel semantic representation of the same PR state without a clear consumer boundary.
- Existing `WorkActions` no longer depends on this daemon to avoid primary misclicks, so this can focus on background freshness and conversation-list scanning.

## Acceptance criteria

- Background polling cadence is tested with jitter bounds.
- Poll persistence/failure behavior is covered by backend tests.
- UI receives live refreshes or snapshot changes after poll results change.
- Conversation list can surface PR-feedback attention state from background-polled data.
