# Fix stale PR feedback freshness after addressing comments

## Problem

The PR remediation button can continue showing an `updated` / `new` freshness marker immediately after the user clicks **Address CI & comments**, even when the captured context was successfully sent to the agent.

Reasoning from the current implementation points to an Option B bug:

- `WorkActions.tsx` calls `api.createPrAutoFixContext(conversationId)`, then invokes `onSendMessage(context.message)` but does not await it.
- The actual feedback baseline is not persisted by `create_pr_auto_fix_context`; the returned capture includes a baseline, but `git_handlers.rs` discards it as `_result_baseline`.
- The baseline is persisted later in `record_pr_auto_fix_context_baseline(...)`, after `/api/conversations/:id/chat` accepts the generated remediation message.
- Because the UI neither awaits the send nor refreshes/clears PR status after the baseline write, the existing `feedback_freshness` marker can remain visible until the next 60s PR polling cycle or a visibility-triggered refetch. This makes it look like new feedback arrived immediately after clicking.
- There is also a narrower robustness risk: `record_pr_auto_fix_context_baseline` reads the artifact via `conv.cwd.join(artifact_path)`. If a Work/Branch conversation’s cwd is a subdirectory while the artifact was written at the worktree root, the agent message can already have been sent but baseline recording can fail, leaving the badge stale.

## Plan

1. Make the remediation action treat "captured and sent to the agent" as an awaited operation.
   - Change the `onSendMessage` prop contract for WorkActions to return `Promise<void> | void`.
   - Await it in `PrRemediationActions` before clearing loading state.

2. Ensure the visible PR freshness state is updated after a successful remediation send.
   - Extend `ConversationPrStatusHandle` with a `refresh()` method that performs an immediate `getPrStatus` fetch and updates state.
   - Call `refresh()` after the remediation message send completes.
   - This should clear `feedback_freshness` when the new baseline matches the current PR feedback.

3. Add a backend test or small refactor for baseline persistence robustness.
   - Prefer resolving the artifact relative to the conversation worktree path when available, not only `conv.cwd`.
   - Preserve current behavior for conversations without a worktree path.

4. Add UI tests covering the race/regression.
   - Clicking **Address CI & comments** awaits `onSendMessage`.
   - The button remains in loading/capturing state until send completes.
   - A successful send triggers a PR status refresh.

5. Consider follow-up UX only if still needed: expose a preview of the new PR feedback items before sending them to the agent. The current evidence suggests the immediate badge is primarily stale UI/baseline timing rather than a real trailing GitHub comment.
