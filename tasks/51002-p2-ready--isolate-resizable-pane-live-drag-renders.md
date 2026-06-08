# Isolate resizable pane live-drag renders

Task 51001 validated candidate C3 as a real render-risk: `useResizablePane` commits React state on every pointermove, and the terminal/viewer pane instances are owned by broad layout/page components. During drag, this can schedule page/layout subtree renders at pointer frequency.

Evidence from the 51001 audit:
- `ui/src/hooks/useResizablePane.ts` calls `setSize` / `setCollapsedState` from the pointermove listener.
- `ConversationPage` owns `terminalPane` and applies it around the conversation subtree.
- `DesktopLayout` owns sidebar/file-explorer/sub-agent pane state.
- The safe small part of C4 was fixed by stabilizing inline max policies; the live-drag state boundary remains.

Acceptance criteria:
- Define a drag scenario for terminal and viewer/sub-agent dividers with raw per-run samples under CPU throttling.
- Count renders for `ConversationPage`, `MessageList`, `ConnectedStateBar`, `ConnectedInputArea`, and pane-local components during drag.
- Implement a structural boundary: either move live drag state to a smaller component boundary or introduce a typed transient live-drag channel (e.g. CSS variable/ref) with committed React state at drag end.
- Re-run the same scenario and keep the patch only if unrelated conversation UI render counts/drop-frame work meaningfully improves.
