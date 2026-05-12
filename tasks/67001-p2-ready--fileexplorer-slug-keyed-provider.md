Follow-up to task 08685 Phase 2. The synchronous-derivation sweep + ConversationList row componentization shipped; this task carries the Phase 2 structural change forward as a focused PR.

## Scope

1. **FileExplorerProvider becomes slug-keyed.** Convert the provider to a
   `Map<slug, FileExplorerScopeState>` (or equivalent) and change the
   consumer hook to `useFileExplorer(slug)` — `slug` is required, not
   optional. Calling `openFile` against the wrong slug becomes
   type-impossible. Today the provider lives globally in DesktopLayout with
   a `scopeKey` reset, which works but leaves a cooperative invariant:
   CommandPalette is mounted globally and can call `openFile()` against
   whatever `activeSlug` happens to be.

2. **useViewport() hook.** Today there are named hooks (`useIsDesktop`,
   `useIsWideDesktop`, `useIsMobile`) that share a single `useMediaQuery`
   primitive — the parallel-implementations problem from the original
   audit is already gone. The remaining ergonomic win: a single hook that
   returns an object so a consumer needing two breakpoints subscribes
   once instead of twice. Consider whether the marginal API improvement
   is worth the churn.

## Call sites to migrate

- `ui/src/components/FileExplorer/FileExplorerContext.tsx` — provider
  topology change
- `ui/src/components/FileExplorer/FileExplorerPanel.tsx` — `useFileExplorer(activeSlug)`
- `ui/src/components/CommandPalette/CommandPalette.tsx` — needs the
  routed-store active slug
- `ui/src/components/WorkActions.tsx` — call site
- `ui/src/pages/ConversationPage.tsx` — call site
- `ui/src/components/FileExplorer/FileExplorerContext.test.tsx` — rewrite
  against the new shape

## Acceptance

- `useFileExplorer(slug)` requires a slug — calling without one is a
  compile error.
- The `scopeKey` prop on `FileExplorerProvider` and the synchronous reset
  inside the provider go away (state is naturally per-key now).
- Document why `ReviewNotesProvider` and `DiffViewerStateProvider` stay
  scopeKey-d (they live inside ConversationPage with a single consumer
  subtree — the topology is already honest there).
