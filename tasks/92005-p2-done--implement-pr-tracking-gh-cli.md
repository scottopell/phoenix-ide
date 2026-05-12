---
created: 2026-05-09
priority: p2
status: done
artifact: pending
---

# implement-pr-tracking-gh-cli

## Plan

## Summary

Implement first-class PR tracking for Work/Branch/Managed conversations using the GitHub CLI (`gh`), based primarily on existing task `02663-p2-ready--pr-tracking-integration.md`, and make the completion flow PR-aware so users are not encouraged to use a stale manual “mark as merged” workflow when `gh` can tell us the PR state.

## Context found

- `02663-p2-ready--pr-tracking-integration.md`: show PR status badge in `StateBar` next to branch name; detect with `gh pr list --head <branch>`; show open/merged/CI status; badge links to PR URL.
- `02664-p2-ready--auto-detect-merged.md`: follow-up idea for auto-detecting merged PRs. We should not fully auto-cleanup/archive in the background yet, but this implementation should be merge-aware and use `gh` to guide the user.
- `specs/projects/requirements.md` currently describes a manual user merge + cleanup flow. This task should preserve the explicit cleanup step, but replace the stale “Mark as merged” affordance with a PR-state-aware completion action when `gh` confirms the PR is merged.

## What to do

1. Move task `02663` to `in-progress` using `taskmd status`.
2. Add a backend PR-status API, likely `GET /api/conversations/:id/pr-status`:
   - Only meaningful when the conversation has a `branch_name` and repo/worktree cwd.
   - Treat `gh` as the integration boundary, not as an optional toy path.
   - Invoke `gh` via a bounded/timeout subprocess from the repository/worktree directory.
   - Query PRs with `gh pr list --head <branch> --state all --limit 1 --json ...` and, where needed, `gh pr checks --json ...` or equivalent `gh` fields.
   - Return a typed response such as:
     - `found: boolean`
     - `unavailable_reason?: "gh_missing" | "not_authenticated" | "not_git_repo" | "command_failed"`
     - PR number/title/url/state/draft/base/head
     - normalized check state: `passing | pending | failing | unknown`
     - normalized display state: `open | draft | merged | closed`
   - Do not silently fail: log command failures/debug context server-side, but keep the UI resilient.
3. Add frontend API types/client method in `ui/src/api.ts`.
4. Update `StateBar` to fetch PR status when a conversation has a branch:
   - Fetch on page load / conversation change.
   - Refresh periodically at a conservative interval and when the document becomes visible.
   - If no PR exists, show no badge.
   - If `gh` is missing or unauthenticated, surface a compact non-blocking hint only where useful, not a noisy persistent error.
5. Render a compact clickable PR badge beside the existing branch display:
   - Purple for merged.
   - Green for checks passing.
   - Yellow for pending/draft/unknown pending.
   - Red for checks failing/closed unmerged.
   - Link opens the PR URL in a new tab.
   - Tooltip includes PR number, title, state, and check summary.
6. Make the completion affordance PR-aware:
   - For Work/Branch conversations with a PR, replace or augment “Mark as merged” with language like “Complete after merged” / “Clean up merged PR”.
   - If `gh` reports the PR is merged, enable the cleanup action and make it clear Phoenix is cleaning up local worktree/branch state after a verified merge.
   - If `gh` reports the PR is still open, failing, pending, draft, or closed-unmerged, discourage cleanup with a clear disabled/warning state rather than pretending the user should manually assert merge state.
   - Provide a deliberate fallback only for `gh` unavailable/failed cases, so local-only or temporarily broken environments are not permanently blocked.
   - Do not implement unattended background cleanup in this pass; the user still initiates destructive local cleanup.
7. Update the project requirements/spec text if needed to reflect the refined behavior:
   - Phoenix still does not push or merge.
   - Phoenix does use `gh` to observe PR state and gate/guide cleanup when available.
   - Manual “Mark as merged” is now the fallback, not the primary happy path.
8. Add tests:
   - Rust tests for parsing/normalizing `gh` JSON into PR/check status.
   - Rust/API tests for unavailable `gh`/not-authenticated/no-PR behavior where practical.
   - UI tests for StateBar badge rendering across merged/passing/pending/failing/not-found states.
   - UI tests for PR-aware completion affordance: merged enables cleanup; open/failing/pending discourages or disables normal cleanup; unavailable allows explicit fallback.
9. Run validation:
   - `./dev.py check`
   - Because this includes Rust changes, run `./dev.py restart` afterward and report the UI URL from its output.
10. Commit the completed change locally with a conventional commit message.

## Acceptance criteria

- Work and Branch/Managed conversations with an associated GitHub PR show a PR badge in the StateBar next to branch info.
- Clicking the badge opens the PR URL.
- Merged PRs are visibly purple, so they are easy to scan.
- CI/check status is reflected as green/yellow/red where available from `gh`.
- Conversations without a PR do not show a misleading badge.
- Missing `gh`, unauthenticated `gh`, or command failures do not break the conversation page and are visible enough to diagnose.
- The cleanup/completion action is PR-aware: verified merged PRs get the happy path; unmerged PRs are discouraged from cleanup; manual assertion is only a fallback.
- Phoenix does not push, merge, or perform unattended background cleanup in this task.
- Existing task `02664` remains the follow-up for any future fully automatic merge detection/cleanup behavior.

## Progress

- Implemented on branch `task-92005-implement-pr-tracking-gh-cli-cli` and merged into the
  review branch. Backend: `GET /api/conversations/:id/pr-status` → `get_pr_status_for_branch`
  shells out to `gh pr list`/`gh pr checks` from the worktree, normalised to
  `display_state`/`check_state` + `unavailable_reason` (`api/git_handlers.rs`, `api/types.rs`).
  Frontend: PR badge in `StateBar`, PR-aware completion affordance in `WorkActions`
  (`api.ts`, `StateBar.tsx`, `WorkActions.tsx`, `index.css`), plus the branch-divergence
  badge removed. Specs: REQ-PROJ-011 reframed (PR status, not commit divergence),
  REQ-PROJ-023 retired; `02663` marked done. `./dev.py check` green. Task 02664 remains the
  follow-up for automatic merge detection/cleanup.


