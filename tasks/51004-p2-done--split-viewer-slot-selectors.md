# Split viewer slot selectors

Task 51001 validated candidates C9/C13 as broad-context risks: consumers that need only viewer commands or only `browserSessionActive` currently call `useViewerSlot()` and subscribe to the whole slot payload.

Evidence from the 51001 audit:
- `ui/src/contexts/ViewerSlotContext.tsx` provides `{ slot, browserSessionActive, openProse, openDiff, openDiffFullscreen, openBrowser, close }` as one context value.
- `WorkViewerActions`, file-explorer wiring, and page sections can observe URL/viewer payload changes even when they need only stable commands or a single primitive.

Acceptance criteria:
- Profile opening prose/diff/browser viewers and count renders in `WorkControlBar`/`WorkViewerActions`, file explorer provider, and command-only consumers.
- Split commands from state and/or add typed selector hooks for `slotKind`, `browserSessionActive`, and commands.
- Preserve the URL-derived discriminated-union slot as the single source of truth.
- Update existing `ViewerSlotContext` tests or add selector tests proving command-only consumers do not re-render on slot payload changes.
