---
created: 2026-04-10
priority: p2
status: done
artifact: ui/src/components/DesktopLayout.tsx
---

# ConversationPage local state lost on desktop/mobile breakpoint crossing

`DesktopLayout` toggles between two distinct React subtrees when the viewport crosses the 1025 px breakpoint (live media query listener). This unmounts and remounts `ConversationPage`, resetting all its local state (e.g. `terminalOpen`, task approval overlays, image attachments).

Discovered during terminal QA: resizing the browser below 1025 px while the terminal panel was open caused the panel to disappear and the shell to be orphaned.

## Root Cause

```tsx
// DesktopLayout.tsx
if (!isDesktop) {
  return <FileExplorerProvider>{children}</FileExplorerProvider>; // different tree → remounts ConversationPage
}
return (
  <FileExplorerProvider>
    <div className="desktop-layout"> ... {children} ... </div>
  </FileExplorerProvider>
);
```

## Fix Options

1. **Hoist volatile state** — lift `terminalOpen` (and other transient UI state) into the `ConversationAtom` or `ConversationProvider` so it survives remounts.
2. **CSS-only breakpoint** — render both layouts always, hide/show with CSS media queries so no unmount occurs.
3. **Stable tree** — always render the desktop layout wrapper, conditionally show/hide the sidebar/file-explorer panels via CSS.

Option 3 is probably simplest and safest.
