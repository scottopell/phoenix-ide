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

## Progress

- `useResizablePane` now exposes a typed transient live-drag channel
  (`startDrag(..., onLiveResize)`): when supplied, pointer moves drive the
  channel only (no React state) and `size`/`collapsed` commit once on
  pointer-up. The legacy (no-callback) path is additionally rAF-coalesced, so
  every consumer's drag is capped at one commit per frame instead of one per
  pointer event.
- The **viewer divider** in `ConversationPage` (the one over the conversation
  message area) is wired to the channel: its width is owned imperatively on the
  `#app` `--viewer-pane-width` variable by a layout effect (committed state) and
  the drag callback (live), which never run concurrently — so dragging it no
  longer re-renders `ConversationPage` / the message thread at all.

Remaining (needs in-browser profiling, not available in the remote sandbox):
- Terminal divider — the height prop feeds `TerminalPanel`'s xterm fit logic, so
  a CSS-var live channel must also decide when xterm refits (likely drag-end
  only). Higher risk; do under a profiler.
- Sub-agent / DesktopLayout sidebar + file-explorer dividers — convert to the
  same channel once the terminal pattern is validated.
