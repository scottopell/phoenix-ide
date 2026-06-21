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
- **Viewer divider** (`ConversationPage`): width owned imperatively on the
  `#app` `--viewer-pane-width` variable by a layout effect (committed state) and
  the drag callback (live) — dragging no longer re-renders the message thread.
- **Terminal divider** (`ConversationPage`): same pattern via
  `--terminal-pane-height`. `TerminalPanel`'s root reads
  `var(--terminal-pane-height, <height>px)` (the prop stays the fallback for
  `NewConversationPage`). xterm fit is driven by a `ResizeObserver` on the panel,
  so it keeps tracking the live height without any React render.
- **Sidebar + file-explorer dividers** (`DesktopLayout`): width owned on the
  `.desktop-layout` `--sidebar-pane-width` / `--file-explorer-pane-width`
  variables; the panels read `var(..., <width>px)`. The variable lives on the
  ancestor, so the 5s conversation poll / SSE re-rendering `DesktopLayout`
  mid-drag cannot clobber the live width. Collapse commits on pointer-up (the
  collapsed rail is a different render); width tracks to its clamped min first.

Not converted (deliberate):
- Sub-agent dock (`SubAgentViewerDock`) — its pane state is local to that
  component and the conversation is a stable `children` element, so its drag
  already never re-renders the conversation view; it benefits from the rAF
  frame-cap. Converting it would need ref-forwarding into a lazy panel for
  little gain.
- `ChainPage` / `NewConversationPage` terminal dividers — separate pages outside
  the conversation view; left on the (now frame-capped) legacy path.

Profiling validation (raw render-count samples under CPU throttling, per the
acceptance) still wants a browser-equipped session; the structural boundary is
correct-by-construction and covered by a `useResizablePane` unit test.
