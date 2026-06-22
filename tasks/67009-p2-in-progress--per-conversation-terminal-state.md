# Per-conversation terminal open state

## User need

The terminal open/collapsed toggle should be sticky per conversation in browser `localStorage`.

Expected behavior:

- Open terminal in conversation A.
- Switch to fresh/default conversation B: terminal is closed.
- Switch back to conversation A: terminal is open again.
- Storage stays browser-local only; no backend persistence.

## Current finding

`ConversationPage` calls `useResizablePane` with global key `terminal-height`, so `terminal-height-collapsed` is shared across all conversations.

Relevant code:

- `ui/src/pages/ConversationPage.tsx` uses `useResizablePane({ key: 'terminal-height', ... defaultCollapsed: true })`.
- `ui/src/hooks/useResizablePane.ts` persists size at `${key}` and collapsed state at `${key}-collapsed`.
- `NewConversationPage` separately uses `global-terminal-height`; this should remain global for `/new` unless product says otherwise.

## Plan

1. Update `ConversationPage` terminal pane key to include the conversation slug or canonical conversation id, e.g. `terminal-height:${slug}`.
2. Keep default collapsed for conversations with no stored key.
3. Ensure key changes re-hydrate pane state when navigating between conversations without a full remount. If `useResizablePane` does not currently re-read localStorage on key change, add key-change hydration there in a way that preserves existing pane behavior.
4. Add tests around `useResizablePane` key switching:
   - key A expanded persists as open;
   - switch to key B with no storage uses default collapsed;
   - switch back to key A restores open state.
5. Run focused UI tests, then `./dev.py check` if time permits.

## Acceptance

- Terminal collapsed/open state no longer leaks from one conversation to another.
- Per-conversation state survives browser reload via `localStorage`.
- Terminal size can still persist per conversation if the implementation keys size the same way.
