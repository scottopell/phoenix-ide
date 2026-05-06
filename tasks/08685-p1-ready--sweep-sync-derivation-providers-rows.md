---
created: 2026-05-05
priority: p1
status: ready
artifact: ui/src/hooks/useMessageQueue.ts
---

# sweep-sync-derivation-providers-rows

## Summary

Cleanup pass that closes the remaining structural items the post-02703
React panel surfaced. Three threads, each individually small, batched into
one task because they share a theme: **state should be derived
synchronously from the source of truth, not reacted-to via `useEffect` or
held in parallel**.

1. **Synchronous-derivation audit of slug-scoped hooks.** 02703 established
   the in-render reset pattern. `useMessageQueue` predates it and uses
   internal `useState` reset via `useEffect([conversationId])`, which
   causes one frame of conv A's pending messages to flash in conv B on
   *returning* navigation (panel Correctness finding #2; "thing others
   will miss" — only fires on returning visits, never on first visits).
   Audit every hook in `ui/src/hooks/` and `ui/src/conversation/` for the
   same shape and fix.

2. **Provider topology — make `FileExplorerProvider` scope structural.**
   Today the provider lives globally in `DesktopLayout` with a `scopeKey`
   reset, which works but leaves a cooperative invariant: `CommandPalette`
   is mounted globally and can call `openFile()` against whatever
   `activeSlug` happens to be. The single-slot file/diff mutex is
   enforced across multiple call sites by code discipline, not by type.
   Convert the provider to a slug-keyed-state shape where hooks take a
   slug argument (`useFileExplorer(slug)`); calling `openFile` against
   the wrong slug becomes type-impossible.

   Same task — fix the **frozen `isDesktop`** in `ConversationPage`
   (panel Architecture's "thing others will miss"): introduce a shared
   `useViewport()` hook with a single media-query listener; delete the
   four parallel implementations across `DesktopLayout`,
   `ConversationListPage`, `CommandPalette`, and `ConversationPage`. The
   ConversationPage one is the bug — it captures viewport at mount and
   never updates, silently routing file opens to the desktop path on a
   resized-to-mobile session.

3. **`ConversationList` rows are components, not inline functions.**
   `renderConvRow` and `renderChainBlock` are local functions defined
   inside the parent, so every parent render re-evaluates every row.
   Extract `<ConversationRow>` and `<ChainBlock>` as `React.memo`
   components with narrow prop shapes. All parent handlers `useCallback`.
   Verify `<li>` keys are stable IDs.

   This is the smallest of the three but worth doing as part of the
   structural pass — it changes the mental model from "memoize this
   loop" to "rows are first-class components," which incidentally makes
   future virtualisation trivial.

## Context

Relevant files:
- `ui/src/hooks/useMessageQueue.ts` — known offender; the headline of
  the audit
- `ui/src/hooks/`, `ui/src/conversation/` — sweep targets
- `ui/src/components/FileExplorer/FileExplorerContext.tsx` — provider
  topology change
- `ui/src/hooks/useViewport.ts` — new file (currently four parallel
  implementations)
- `ui/src/components/ConversationList.tsx` — row componentisation

## Plan

### Phase 1: Synchronous-derivation audit

- Walk every hook in `ui/src/hooks/` and `ui/src/conversation/`.
- For each: does it hold internal `useState` that is reset via
  `useEffect([prop])`? That is the bug pattern.
- Fix by either:
  - **(a)** Deriving directly from the prop (no internal state), OR
  - **(b)** Applying the in-render reset pattern from 02703, OR
  - **(c)** Moving the state into the appropriate atom (after task C's
    store expansion).
- `useMessageQueue` is the headline. List every other hook found in the
  PR description so the next audit has a starting list.
- Add a regression test for `useMessageQueue` proving returning-navigation
  no longer flashes stale state. The test must exercise *returning*
  navigation specifically (visit A → visit B → return to A); first-visit
  paths are not affected.

### Phase 2: Provider topology

- `FileExplorerProvider` becomes a `Map<slug, FileExplorerScopeState>`
  internally (or an equivalent keyed structure).
- `useFileExplorer(slug)` is the consumer hook; `slug` is required (not
  optional, not defaulted).
- `FileExplorerPanel` calls `useFileExplorer(activeSlug)`.
- `CommandPalette`'s `openFile` call becomes
  `useFileExplorer(activeSlug).openFile(...)` — `activeSlug` is read
  from the routed-store hook, not from a context guess.
- The `scopeKey` prop and the synchronous reset inside the provider go
  away (state is naturally per-key now; switching slugs is just
  switching which key the hooks read).
- Update `FileExplorerContext.test.tsx` against the new shape, or
  delete and rewrite — the scopeKey reset semantics no longer apply.

For `ReviewNotesProvider` and `DiffViewerStateProvider`: these are
inside `ConversationPage` and only have one consumer subtree. **Leave
them scopeKey'd** — the topology is already honest there. Document the
asymmetry in a comment so future readers know why.

- `useViewport()`: single hook, single media-query listener, returns
  `{ isDesktop, isWideDesktop, ... }`. All four call sites consume it.
  Delete the four parallel implementations.

### Phase 3: ConversationList row components

- Extract `<ConversationRow>` and `<ChainBlock>` as `React.memo`
  components with narrow prop types (no whole-context-object
  dependencies).
- Parent handlers `useCallback` with stable deps. The
  `onConversationCreated` callback in particular cascades into ~8 other
  callbacks — stabilise it at the source.
- Verify keys are conversation IDs / chain root IDs (stable across
  reorders; not array indices).

### Phase 4: Tests

- Returning-navigation flash test for `useMessageQueue` (and any
  sibling hooks the audit surfaces).
- Type-level test (or runtime assertion) that `useFileExplorer`
  requires a slug argument — calling without one is a compile error.
- **Resize test**: mount `ConversationPage` on desktop viewport, resize
  to mobile, click to open a file → file opens via the mobile path
  (regression for the frozen `isDesktop`).
- Render-count test for `ConversationList`: arrow-key navigation does
  not re-render unaffected rows. (Mirror existing render-count test
  patterns in the codebase if any; otherwise document the manual QA
  step.)

## Acceptance Criteria

- No hook in `ui/src/hooks/` or `ui/src/conversation/` resets internal
  state via `useEffect([prop])`. Synchronous derivation only. The PR
  description lists every hook audited and the resolution for each.
- `useFileExplorer(slug)` requires a slug argument; openFile-against-
  wrong-slug is type-impossible.
- A single `useViewport()` hook owns the media-query subscription; the
  frozen `isDesktop` in `ConversationPage` is fixed.
- `ConversationRow` and `ChainBlock` are `React.memo` components with
  narrow prop types and stable keys.
- `onConversationCreated` (and any callbacks it cascades into) are
  reference-stable across `DesktopLayout` re-renders.
- `./dev.py check` passes.

## Dependencies

- **Task C** (single Conversation source of truth) lands first. This
  task assumes the store can answer "give me the active slug" cleanly;
  without C, the provider topology change is awkward (the slug for
  `useFileExplorer(slug)` is naturally the active slug from the store).
- Tasks A and B can land in any order relative to this task, but in
  practice this task is the cleanup pass that runs last.
