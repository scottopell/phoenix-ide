# Add UI choice for task approval handoff mode

## Summary

The backend and API already support approving a proposed task either by continuing in the current conversation or by starting a fresh Work conversation. The current UI always calls `api.approveTask(conversationId)` without passing a handoff value, and the API helper defaults that call to `start_fresh_work_conversation`. As a result, users cannot choose the inline continuation path even though it is implemented backend-side.

Add a visible UI choice to the task approval reader so users can approve either:

- **Continue here** — approve the task and keep execution in the current conversation.
- **Start fresh conversation** — approve the task and hand off to a new Work conversation.

## Implementation plan

1. Introduce/export a small frontend `TaskApprovalHandoff` type matching the existing API literals:
   - `continue_in_current_conversation`
   - `start_fresh_work_conversation`

2. Update `TaskApprovalReader`:
   - Change `onApprove` from `() => void` to `(handoff: TaskApprovalHandoff) => void`.
   - Replace the single Approve button with two explicit approval actions, or an approval mode selector plus one submit button.
   - Keep the existing unsent-feedback warning behavior.
   - Track loading per selected handoff so double-submits are prevented and the clicked action shows progress.

3. Update `ConversationPage.handleApproveTask`:
   - Accept a handoff argument.
   - Pass that argument through to `api.approveTask(conversationId, handoff)`.

4. Keep existing discard and feedback flows unchanged.

5. Add/update tests:
   - Verify the task approval reader exposes both approval choices.
   - Verify each choice calls `onApprove` with the correct handoff literal.
   - Verify the conversation page/API path sends the selected handoff to `api.approveTask`.
   - Preserve existing tests around unsent feedback, discard, and markdown rendering.

## Acceptance criteria

- The task approval UI offers both inline continuation and fresh-conversation approval choices.
- Choosing inline approval sends `continue_in_current_conversation` to `/api/conversations/:id/approve-task`.
- Choosing fresh approval sends `start_fresh_work_conversation` to `/api/conversations/:id/approve-task`.
- Existing feedback/discard behavior remains unchanged.
- UI tests cover both approval modes.

## Notes

No backend work appears necessary. The backend request type, state-machine enum, and executor effects already distinguish the two handoff modes. The missing piece is frontend affordance and plumbing.

Primary files expected:

- `ui/src/api.ts`
- `ui/src/pages/ConversationPage.tsx`
- `ui/src/components/TaskApprovalReader.tsx`
- `ui/src/components/TaskApprovalReader.test.tsx`

Run the relevant UI tests, and run the project check lane appropriate for this UI-only change before committing.
