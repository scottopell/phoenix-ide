# Preserve /new conversation draft across navigation

## Problem

On `/new`, the message composer draft lives only in component state inside `useCreateConversation`. When the user clicks into an existing conversation, `NewConversationPage` unmounts; returning to `/new` creates fresh state and the typed draft is lost.

## Goal

Preserve the new-conversation draft when navigating away from `/new` and back, without accidentally resurrecting text after the user successfully starts a conversation.

## Plan

1. Add browser-side draft persistence for the `/new` composer text.
   - Initialize `draft` from a dedicated storage key, e.g. `phoenix-new-conversation-draft`.
   - Persist changes whenever the user edits the draft.
   - Keep voice input behavior compatible with the persisted draft.
2. Clear the persisted draft on successful conversation creation, before or immediately after navigating to the created conversation.
3. Decide whether image attachments should remain ephemeral for now.
   - The reported bug is about typed message text.
   - Persisting images is more complex and may have storage-size/privacy implications, so leave images out unless explicitly required.
4. Add regression coverage.
   - Verify draft text survives unmount/remount of `NewConversationPage`.
   - Verify successful send clears the stored draft so returning to `/new` starts empty.

## Acceptance criteria

- Type a draft on `/new`, open an existing conversation, return to `/new`: the draft text is still present.
- Send/start a new conversation: returning to `/new` does not show the submitted draft.
- Existing localStorage preferences for cwd/model/recent dirs continue to work.
- Tests cover persistence and clearing behavior.
