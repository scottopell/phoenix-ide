# Add PR CI monitoring and auto-fix action

## Context

Phoenix already has basic PR awareness:

- Backend route `GET /api/conversations/:id/pr-status` in `crates/phoenix-ide/src/api/git_handlers.rs` uses `gh pr list` and `gh pr checks` for Work/Branch conversations.
- UI `StateBar` polls that route and shows a compact PR badge.
- `WorkActions` gates local cleanup on PR merge status and already accepts an `onSendMessage?: (text: string) => void` prop, but does not currently use it.

The requested feature is a richer PR/CI affordance like the screenshot: a CI dropdown/popover that shows PR check health and exposes an “Auto-fix CI & address comments” action.

## Goal

Add a first-class PR monitoring panel for Work/Branch conversations that lets the user ask Phoenix to fix failing CI and PR review comments using the existing conversation/agent workflow and the authenticated GitHub CLI (`gh`).

## Proposed scope

### 1. Extend PR status data

Extend the backend PR status model to include enough structured data for a useful CI popover:

- check summary counts: passing, pending, failing, skipped/unknown
- failing/pending check names where available
- review/comment discovery strategy, with explicit coverage for the different GitHub surfaces where feedback can live (PR review comments, issue comments, review summaries, unresolved review threads if available)
- a conservative typed summary of discovered feedback, plus debug logging/metadata that makes gaps visible during testing
- last updated timestamp for the fetched status
- Treat review-comment discovery as a first-class investigation, not a one-command assumption. GitHub feedback can appear through multiple APIs/surfaces; the implementation should document and test the chosen `gh api` / GraphQL / REST calls against representative PRs before declaring coverage.

The response must use typed Rust fields and matching TypeScript types. Do not embed opaque provider JSON in API responses; if raw `gh`/GitHub payloads are useful for debugging, keep them in logs or in the auto-fix context artifact described below, with a typed wrapper/manifest.

### 2. Add a CI/PR monitoring UI

Add a compact statebar control similar to the screenshot:

- Green/yellow/red CI pill based on `check_state`.
- Dropdown/popover with:
  - CI monitoring status (`Passed`, `Pending`, `Failed`, `Unknown`) and counts.
  - Link to open the PR/checks in GitHub.
  - Button: `Auto-fix CI & address comments`.
- Respect Phoenix UI density conventions: inline status, compact labels, clear disabled/tooltips for unavailable `gh`/auth cases.

### 3. Implement explicit auto-fix trigger

Wire `Auto-fix CI & address comments` to gather the already-fetched PR context into a durable context artifact, then send a steering/user message into the active conversation using the existing `onSendMessage` plumbing.

Proposed flow:

1. Backend exposes an explicit auto-fix context endpoint/action for the conversation/PR.
2. That action refreshes or reuses the PR status payload plus the detailed failing checks, check log URLs/snippets where available, and discovered review/comment feedback.
3. It writes a JSON context artifact inside the conversation worktree, for example `.phoenix/pr-context/pr-<number>-<timestamp>.json`. The artifact should be intentionally structured and typed enough for agents to consume predictably: PR metadata, check summaries/details, comments grouped by source surface, fetched-at timestamp, and known coverage limitations.
4. The UI sends a concise message that points the agent at that file instead of asking it to rediscover everything with `gh`.

Example generated message:

> Address the PR feedback captured in `.phoenix/pr-context/pr-N-TIMESTAMP.json`. Use that file as the source of truth for failing CI checks and review comments, fix the issues in this worktree, run targeted tests, commit the changes, and summarize what changed.

Important behavior:

- Available only for Work/Branch conversations with a found, open PR.
- If the conversation is idle, send immediately after the context artifact is written.
- If the conversation is busy, write the context artifact immediately and rely on the existing steering queue behavior from `api.sendMessage` so the agent handles it when it next reaches idle.
- Do not auto-push; normal Phoenix agent guidance still applies for commits and any push/deploy action.
- Log and surface `gh` capability gaps instead of silently hiding the action.
- If context artifact creation fails, do not send a vague auto-fix message; show the error so the user can retry.

### 4. Explicit action only; no background loop

This task should implement a user-triggered action, not a daemon/scheduler:

- Phoenix may poll PR status for display, as it does today, but polling must not automatically enqueue agent work.
- The agent should start fixing only when the user clicks `Auto-fix CI & address comments`.
- No persisted “keep watching and fix future failures” toggle in this task.
- No retry loop that repeatedly re-enqueues work for the same failing checks/comments.

This boundary keeps the first version reviewable and avoids accidental infinite fix loops. A later task can add background monitoring with persisted per-conversation settings, debounce/cooldown, and last-handled signatures.

### 5. Tests

Backend:

- Unit tests for normalizing check summaries and unavailable `gh` cases.
- Unit/API tests for the auto-fix context artifact: creates a file under the worktree, includes typed PR/check/comment sections, records coverage limitations, and fails without sending a message when context capture fails.

Frontend:

- Component tests for the CI pill/popover states: passing, pending, failing, unknown, unavailable.
- Test that clicking auto-fix creates/receives a context artifact path, sends a message referencing that path via `onSendMessage`, and disables/labels correctly when no PR or `gh` is unavailable.
- Update existing `StateBar` / `WorkActions` tests as needed.

End-to-end/manual verification:

- With no `gh` auth: UI shows unavailable state and no unsafe action.
- With an open PR and passing checks: popover shows green status.
- With failing checks or review comments: auto-fix writes a PR context JSON artifact and sends a message referencing that exact artifact path.

## Non-goals for this task

- Auto-merging PRs, including auto-merge UI placeholders.
- Creating PRs from Phoenix branches.
- Replacing GitHub’s checks UI; Phoenix should summarize and link out.
- A full background daemon/monitor that automatically starts future fix attempts without a user click.
