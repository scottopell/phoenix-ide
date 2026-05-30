# Clean up stale P1 task queue after triage

## Summary

Apply the P1 triage findings to the task queue so `p1-ready` reflects current work. Several P1-ready files are already implemented, one is blocked by a prerequisite, and a couple are stale batch/meta tasks that should be rebaselined rather than treated as immediately actionable implementation work.

This is not just filename churn: when a task is complete but its spec executive table is stale, this cleanup pass owns the necessary spec-status update or the creation of narrowed follow-up tasks.

## Proposed changes

### Mark done after final evidence check

Rename these from `p1-ready` to `p1-done` if the cited evidence still holds on the work branch:

- `tasks/62005-p1-ready--browser-sessions-on-workscope.md`
  - Evidence found: browser sessions are `WorkScope` keyed; `cascade_browser_on_delete` exists; `specs/browser-tool` marks `REQ-BROWSER-WS-001..004` complete.
- `tasks/92003-p1-ready--mobile-file-explorer-entrypoint.md`
  - Evidence found: mobile `StateBar` exposes `onOpenFiles`; `ConversationPage` calls `setShowFileBrowser(true)` and renders `FileBrowserOverlay`.
- `tasks/01004-p1-ready--messagelist-render-units-virtualization.md`
  - Evidence found: `RenderUnit`/`buildRenderUnits` exist with tests; `MessageList` renders render units.
- `tasks/60410-p1-ready--migrate-messagelist-to-react-virtuoso.md`
  - Evidence found: `react-virtuoso` dependency exists; `MessageList.tsx` imports/renders `Virtuoso`.

### Reconcile likely-complete spec/status tasks

Inspect implementation and executive tables before closing. This task includes doing the spec cleanup, not merely noting it.

- `tasks/58003-p1-ready--spec-llm-retry-visibility.md`
  - Evidence found: `specs/llm-retry-visibility/` exists; generated SSE includes `llm_attempt`; backend has `LlmAttempt`/`LlmAttemptReason`; `working-phase-visibility.allium` imports the sibling spec.
  - Required cleanup: update `specs/llm-retry-visibility/executive.md` rows to match reality if the implementation/spec work is complete. If any rows are genuinely unfinished, split those into concrete current tasks and narrow or close this broad task accordingly.
- `tasks/58001-p1-ready--reliable-agent-activity-indicators.md`
  - Evidence found: `StateBar` has working-phase indicators, retry modifier, watchdog text, typed ping handling, and tests.
  - Required cleanup: reconcile `specs/working-phase-visibility/executive.md`; either mark the task done with executive rows aligned, or split remaining unimplemented requirements into precise tasks.

### Reclassify blocked/stale tasks

- `tasks/46003-p1-ready--pierre-diff-replacement.md`
  - Rename to `p1-blocked` or equivalent taskmd status if supported, because it explicitly depends on `MetaViewer Refactor`.
- `tasks/08685-p1-ready--sweep-sync-derivation-providers-rows.md`
  - Audit further before changing status. Some synchronous-derivation work appears done, but provider-topology work is still not done (`FileExplorerProvider` still uses `scopeKey`; `useFileExplorer()` takes no slug). After the audit, split/rebaseline into smaller current tasks for the remaining work rather than preserving the broad stale batch shape.
- `tasks/27106-p1-ready--continue-spec-audit-bug-hunting.md`
  - Mark won’t-do. This is better represented as a reusable repo skill/workflow than as a standing P1 task.
  - Add a repo-local skill for this workflow (for review/evaluation), capturing the useful behavior from the task: re-run executive-table inventory, prioritize high-ROI 🚧/❌ rows, close small spec-code gaps with matching executive updates, and spin out concrete tasks for ambiguous/significant gaps. Keep the skill concise and actionable so its utility can be evaluated during review.

### Keep as P1-ready

Leave these ready unless deeper inspection disproves the triage:

- `tasks/62007-p1-ready--remove-unarchive-ui.md` — unarchive UI/API/sync callsites still exist.
- `tasks/13025-p1-ready--anthropic-toolresult-cache-breakpoint.md` — `ToolResult` still lacks `cache_control`; breakpoint still matches only `Text | Image`.
- `tasks/67006-p1-ready--workscope-pr-association.md` — no durable WorkScope PR association model/table found.
- `tasks/00003-p1-ready--notification-policy-reducer.md` — no reducer/effect policy engine found.
- `tasks/08690-p1-ready--launchd-socket-activation.md` — no launchd activation branch found.
- `tasks/13011-p1-ready--base-prompt-depth-decision.md` — `BASE_PROMPT` still minimal.
- `tasks/24679-p1-ready--revisit-shell-integration-detection-lock-policy.md` — no evidence of resolved policy.
- `tasks/46002-p1-ready--metaviewer-refactor.md` — `ProseReader`/old viewer state shape still present.

## Implementation plan

1. Re-run targeted searches from the triage to confirm no branch drift.
2. Rename task files only where status is clear; edit task bodies only when needed to narrow stale scope or record why a stale task is being closed.
3. For likely-complete spec/status tasks (`58001`, `58003`), inspect executive tables and perform the required status cleanup:
   - update executive rows when implementation is complete,
   - create narrowed follow-up tasks for any genuinely remaining requirements,
   - then rename the broad task to done only when the docs/tasks agree.
4. For `08685`, audit the current UI state first, then split/rebaseline only the remaining work.
5. For `27106`, rename to won’t-do and add the repo-local skill that captures the reusable spec-audit workflow.
6. Run task/spec validation (`./dev.py tasks validate`, and broader checks if spec files or skills require them).
7. Commit the cleanup as a task/spec/skill-only commit.

## Done when

- Completed P1 tasks no longer appear in `p1-ready`.
- Spec executive tables for closed spec-related tasks match implementation reality.
- Blocked/stale tasks are not mixed into immediately actionable P1-ready work.
- The stale spec-audit task is closed as won’t-do and replaced by a reviewable repo-local skill.
- Remaining `p1-ready` tasks are current and actionable.
- Task filenames validate.
