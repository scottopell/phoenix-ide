# Terminal panel: default to collapsed

Right now the terminal split-pane in `ConversationPage` defaults to expanded (~300px). Most conversations don't need the terminal, and it eats vertical space and pulls in xterm + the WebSocket connection on every conv load.

Make it default to **collapsed** instead. User must drag the divider or hit Ctrl+\` to open it.

## Scope

1. `ui/src/hooks/useResizablePane.ts`
   - Add `defaultCollapsed?: boolean` option (default `false` — preserves behavior for sidebar / file-explorer / viewer panes).
   - Use it as the fallback in the `collapsed` `useState` initializer instead of the hardcoded `false`.
   - **Stop auto-persisting initial state.** The existing `useEffect` that writes `collapsed` to localStorage on mount causes the initial default to become "sticky" the moment the page loads, which means flipping a default never reaches existing users. Change the persistence so it only writes when the value diverges from what's already in localStorage (or only writes after the first user-driven change). Same treatment for `size` for consistency.
   - Likely shape: track whether the value has been hydrated from storage vs. only the default; only persist after a real `setCollapsedState` / `setSize` call.

2. `ui/src/pages/ConversationPage.tsx`
   - Pass `defaultCollapsed: true` to the `useResizablePane` call for `terminalPane` (around line 272).

## Migration behavior (per user decision)

- Users with an explicit persisted value (`terminal-height-collapsed` in localStorage) keep their choice.
- Users who never explicitly toggled the panel (the vast majority — the value was only written by the initial-state effect) get the new collapsed default on next load, because we no longer write initial state.
- No localStorage key bump, no migration code.

## Verification

- Fresh browser profile: open a conversation → terminal is collapsed (32px header strip only).
- Existing profile where user has explicitly expanded the terminal at some point: behavior unchanged (still open).
- Ctrl+\` still toggles. Drag from collapsed state still expands. `expandFromCollapsed` / `setCollapsed` paths unchanged.
- Sidebar, file explorer, task viewer panes unchanged (they don't pass `defaultCollapsed`, so they keep their current default-`false` behavior, and their persistence behavior shouldn't visibly change for any user who has used those panes — worth a quick smoke test).
- `cargo test` / `./dev.py check` clean.

## Out of scope

- Per-conversation terminal state (the key is shared across all convs by design — see task 08664).
- Lazy-mounting the `TerminalPanel` component when collapsed (it's already `lazy()`-imported; whether the xterm instance constructs while collapsed is a separate question, not addressed here).
