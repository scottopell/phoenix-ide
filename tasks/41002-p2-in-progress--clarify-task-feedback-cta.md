# Clarify task approval actions when feedback notes exist

## User story

As a user reviewing a proposed task, once I have added one or more feedback notes, the UI should make it clear that sending those notes is the expected next action, while still allowing intentional approval. The UI must not silently swap button positions because users learn the toolbar layout over time.

## Problem

The task approval toolbar currently keeps `Send Feedback` and `Approve` adjacent. After a user has already added a feedback note, it is easy to accidentally click `Approve` when the intended action is `Send Feedback`, especially after submitting or preparing comments.

## Proposed UX

Keep the existing action order and physical button locations stable:

1. `Discard`
2. `Send Feedback`
3. `Approve`

When `notes.length > 0`:

- Keep `Send Feedback` in the same middle position, but make it the visually primary/recommended action.
- Keep approval in the same right-side position, but reduce its visual emphasis.
- Change the approval label from `Approve` to `Approve without sending feedback` or `Approve despite notes` so the user understands the consequence.
- Add an inline cue near the toolbar or notes badge explaining the state, e.g. `You have 2 unsent feedback notes`.
- Ensure the shift is explicit, not silent: the button positions remain unchanged, and the text/visual cue explains why the recommended action changed.

When `notes.length === 0`:

- Preserve the current default approval-oriented layout and copy.
- `Send Feedback (0)` remains disabled with its existing explanatory title.

## Implementation notes

Likely surface: `ui/src/components/TaskApprovalReader.tsx`.

Avoid adding a confirmation modal unless usability testing shows the text/emphasis change is insufficient. The goal is to reduce accidental approval without slowing down intentional approval.

Potential class/API approach:

- Derive `hasUnsentNotes = notes.length > 0`.
- Conditionally apply a primary/recommended style to the feedback button when `hasUnsentNotes`.
- Conditionally apply a subdued/secondary style to the approval button when `hasUnsentNotes`.
- Conditionally render approval copy that names the consequence.
- Add/update tests around the button labels and stable ordering when notes exist.

## Acceptance criteria

- With no notes, the toolbar behaves as it does today: `Discard`, disabled `Send Feedback (0)`, and emphasized `Approve`.
- After adding at least one note, the toolbar order is still `Discard`, `Send Feedback`, `Approve...`.
- After adding at least one note, `Send Feedback` is visually the recommended action.
- After adding at least one note, the approval button copy makes clear that approval will not send the unsent feedback notes.
- The UI shows an explicit cue that unsent feedback notes exist.
- Tests cover both the zero-note and one-or-more-notes states, including stable action ordering.
