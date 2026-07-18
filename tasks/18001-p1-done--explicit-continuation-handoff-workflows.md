# Make conversation continuation workflows explicit and safe

## Problem

The context-exhausted view mixes two unrelated decision surfaces:

- continuing from the generated operational handoff; and
- destructive workspace lifecycle actions such as **Clean up** and **Abandon**.

Those terminal actions sit beside the primary continuation action even though this view is principally a handoff/recovery experience. The proximity is dangerous, and the current continuation behavior is also implicit: **Continue in new conversation** creates a successor and silently places the handoff in its unsent composer. Users must know that it does not auto-send, and externally refined handoffs require creating the successor before replacing its input.

The desired workflows are:

1. Continue immediately with the generated handoff, unchanged.
2. Copy the generated handoff for use or refinement elsewhere, then optionally paste a refined version into Phoenix before continuing.
3. Make small edits in Phoenix and continue with those edits, while always retaining the pristine generated handoff as a reset point.

## Product model

Treat the generated continuation summary as an immutable **generated handoff**. Never overwrite or conflate it with user edits.

Maintain an optional, browser-local **handoff edit draft** keyed to the exhausted parent conversation. The edit draft is a separate presentation-layer value, not conversation history and not a second persisted representation of the generated handoff.

The context-exhausted card has two explicit modes.

### Review mode

- Primary: **Continue**
  - Creates the single successor conversation.
  - Submits the pristine generated handoff as the successor's first user message.
  - Navigates to the running successor.
- Secondary: **Edit first**
  - Opens edit mode without creating a successor.
  - Restores an existing browser-local handoff edit draft when present; otherwise starts from the generated handoff.
- Secondary: **Copy handoff**
  - Copies the pristine generated handoff.
  - Does not create or mutate a conversation.
- Do not render the work actions bar or any **Clean up**/**Abandon** controls in this view.

### Edit mode

- Present a large editable text area initialized from the local edit draft or pristine generated handoff.
- Autosave changes browser-locally as the handoff edit draft.
- Primary: **Continue with edits**
  - Creates the successor and submits the current edited handoff as its first user message.
- Secondary: **Copy edited handoff**
  - Copies the current editor value, supporting refinement elsewhere and paste-back into the same editor.
- Secondary: **Revert to generated**
  - Clearly confirms destructive replacement when the draft differs.
  - Deletes the local edit draft and restores the pristine generated handoff.
  - The user can then edit again from that clean baseline.
- Secondary: **Cancel editing**
  - Returns to review mode without creating a successor.
  - Keeps the browser-local draft so **Edit first** can resume it; show a subtle local-draft indicator in review mode.
- Reject an empty or whitespace-only edited handoff inline; do not create a successor.
- Do not render workspace terminal actions in edit mode.

Once the parent already has a successor, retain the single-continuation rule and replace creation actions with one clear **Open continuation** action. Do not send or resend a handoff when resolving an already-existing continuation.

## Implementation plan

### 1. Update the normative continuation and work-action contracts

Revise `specs/bedrock/requirements.md` (`REQ-BED-021`, `REQ-BED-030`, and their rationale) to define the unchanged-send, edited-send, copy, immutable-generated-handoff, and single-successor behavior.

Revise `specs/work-actions-bar/requirements.md` so `context_exhausted` is no longer a work-actions-bar presentation phase. Remove the `context_exhausted` work-disposition/UI rows and document that the focused continuation surface suppresses terminal workspace controls. Preserve backend lifecycle and ownership safety from `REQ-BED-031` and `work-lifecycle`: this task changes the context-exhausted UI affordance and does not weaken server-side action validation or cleanup semantics.

Check `specs/bedrock/bedrock.allium` and `specs/projects/projects.allium` for affected continuation triggers/contracts. Update precise behavior only where the opening-message dispatch changes the specified operation; retain atomic worktree ownership transfer and the one-successor invariant. Remove any touched stale `design.md` citations in accordance with spec-authoring rules, and run the `specs/AUTHORING.md` pre-flight checklist.

### 2. Introduce an explicit handoff review/editor component

Extract the large context-exhausted block from `ConversationPage` into a focused component with a typed state model, for example:

- `reviewing` with immutable generated handoff and optional local-draft presence;
- `editing` with an editable draft;
- `submitting` with the exact source (`generated` or `edited`) fixed for that attempt;
- `already_continued` with only navigation to the successor.

Keep generated text and edit-draft text as separate fields/types so reset and send-source behavior are structurally clear. Disable duplicate actions while submission is in flight. Keep the summary prominent and selectable in review mode.

Colocate the component-specific CSS with the new component rather than adding more continuation-specific rules to `ui/src/index.css`; move only the clearly owned existing styles and preserve unrelated global rules.

### 3. Add browser-local handoff edit drafts

Add a narrowly scoped storage adapter rather than reading/writing `localStorage` throughout the component.

- Use a namespaced key based on the exhausted parent conversation ID (for example `handoff-edit-draft:<id>`).
- Store only user-edited text and minimal version/source identity needed to reject a stale draft if the generated handoff differs.
- A missing key means no user edit draft; never copy the generated handoff into storage merely by entering edit mode.
- Storage failures degrade to an in-memory editing session and visible non-blocking feedback.
- Revert removes the edit-draft key.
- Successful acceptance of the opening message removes the edit draft.
- Cancel preserves it.

Retire continuation's use of `seed-draft:<successor-id>`. Keep that mechanism intact for unrelated seed/new-conversation workflows.

### 4. Make create-and-send a robust continuation operation

Extend the continuation API contract so the request carries:

- the exact opening handoff text selected by the user; and
- a client-generated idempotency/message ID.

Do not infer generated-versus-edited semantics on the backend; both are validated non-empty opening messages. Preserve the existing atomic database transaction for successor creation and worktree ownership transfer.

After creation, dispatch the opening message through the existing send-chat application service so persistence, expansion, runtime startup, and message idempotency remain centralized rather than duplicated in the handler.

Model partial outcomes explicitly. Successor creation/ownership transfer and opening-message dispatch cross a boundary and cannot honestly be presented as one rollbackable transaction. The typed response must distinguish at least:

- existing successor (navigate only; never resend);
- successor created and opening message accepted;
- successor created but opening message not accepted.

For the partial-failure case, return the successor identity and actionable error details. Navigate to that successor, preserve/re-seed the exact attempted text as a recoverable composer draft, and explain that continuation was created but the handoff still needs sending. Retrying with the same message ID must not duplicate a persisted opening message.

Ensure the UI does not claim the agent is running until message acceptance is confirmed.

### 5. Remove destructive workspace controls from the handoff card

Remove `stuckCleanupBar` from the `context_exhausted` branch in `ConversationPage`. Adjust `WorkControlBar` visibility/derivation so future composition changes cannot accidentally put the bar back into this phase. Continue suppressing all parent terminal verbs once `continued_in_conv_id` exists.

The worktree is preserved while the exhausted parent has no successor, and ownership transfers to the successor when continuation is created. No cleanup, abandon, git, or PR action should be triggered as a side effect of viewing, editing, copying, or continuing a handoff.

### 6. Tests and verification

Add component/integration coverage for:

- review mode shows **Continue**, **Edit first**, and **Copy handoff**;
- no **Clean up**, **Abandon**, work actions bar, or PR action appears anywhere in the context-exhausted handoff surface;
- **Continue** sends exactly the immutable generated handoff once;
- **Edit first** does not create a successor;
- edited continuation sends exactly the editor value;
- empty edited text is rejected before API invocation;
- generated handoff remains unchanged through edits;
- local edit draft autosaves, survives cancel/remount, and is separate per parent conversation;
- **Revert to generated** removes the local draft and restores generated text;
- copy in review mode copies generated text, while copy in edit mode copies editor text;
- successful dispatch clears the local edit draft;
- storage failure still permits editing and continuation;
- repeated clicks are gated;
- an existing continuation only navigates and never resends;
- create-success/send-failure routes to the successor with the exact attempted handoff recoverable and no duplicate on retry;
- Work/Branch/Explore/Direct inheritance and worktree ownership invariants remain intact.

Add API/service tests for each typed continuation outcome, invalid blank handoff, idempotent message retry, and the existing single-continuation race. Update generated TypeScript types if the response is code-generated.

Run relevant UI and Rust tests, `./dev.py codegen` when required, Allium validation for touched specs, and `./dev.py check`.

## Acceptance criteria

- The context-exhausted page is a focused handoff experience with no workspace-destructive controls.
- One click on **Continue** creates the successor and submits the pristine handoff; it does not leave an intentionally unsent draft in the ordinary success path.
- Users can edit before successor creation and can always recover the pristine generated handoff.
- User edits are browser-local, resume after cancel/revisit, and never mutate the stored generated continuation summary.
- Copying supports both external use of the pristine handoff and external refinement of an edit draft without requiring premature successor creation.
- A partial dispatch failure never loses the attempted handoff, strands the user on the parent, or duplicates the opening message.
- Existing environment inheritance, worktree ownership transfer, and single-successor guarantees remain unchanged.
