---
created: 2026-05-05
priority: p1
status: done
artifact: pending
---

# smooth-conversation-sidebar

## Plan

## Summary

Three related sidebar / per-conversation-state polish issues:

1. **Conversation→conversation navigation flashes the whole content area blank.** `KeyedConversationPage` in `ui/src/App.tsx` uses `key={slug}` to fully unmount and remount `ConversationPage` on every slug change — too aggressive a hammer.

2. **Sidebar reorders / re-renders even when nothing meaningful changed.** `DesktopLayout` polls every 5s and unconditionally calls `setConversations(freshActive)` with a brand-new array reference, *and* refetches on every `location.pathname` change.

3. **File viewer state leaks across conversations.** `FileExplorerProvider` is mounted in `DesktopLayout` as a single shared instance. The open file in the prose-reader pane stays mounted when the user switches conversations — the chat transcript updates, but the viewer keeps showing a file from the previous conversation. Per-conversation state must be per-conversation.

## Guiding principle: honest UI

The screen must reflect the current selection. If the sidebar selection has moved to conversation DEF, the user must NOT see ABC's content (transcript, viewer, breadcrumbs, anything) for any duration — even half a second is a bug. When DEF's data isn't ready yet, render a clean, complete skeleton/loading shell — never stale content from another conversation.

This rules out the "show previous conversation until new is ready" pattern from the original plan.

## Context

Relevant files:
- `ui/src/App.tsx` — `KeyedConversationPage` / `KeyedChainPage` `key=` remount
- `ui/src/pages/ConversationPage.tsx` — page-level state, the `if (!conversation)` skeleton early-return at ~L726
- `ui/src/components/DesktopLayout.tsx` — `FileExplorerProvider` placement, `loadConversations` polling, the `location.pathname` effect
- `ui/src/components/FileExplorer/FileExplorerContext.tsx` — `proseReaderState`, `activeFile`
- `ui/src/contexts/ViewerStateContext.tsx` — `DiffViewerStateProvider` (already inside `ConversationPage`, so currently OK; verify)
- `ui/src/conversation/ConversationStore.ts` — atoms persist across navigation, keyed by slug
- `ui/src/hooks/useMessageQueue.ts` — already keyed on `conversationId`
- `src/db.rs:506`, `src/db.rs:1363` — server orders by `updated_at DESC`; every message insert bumps `updated_at`

## Plan

### Part A: stop unmounting `ConversationPage` on slug change

A1. **Audit page-level state for cross-conversation leak risk.** Walk every `useState` / `useRef` in `ConversationPageContent`. For each, decide:
   - Already keyed on `conversationId` (e.g. `useMessageQueue(conversationId)`) → no action
   - Per-conversation but currently relying on remount → reset explicitly via `useEffect(() => { ... }, [slug])`
   - Truly page-global (e.g. `isDesktop`, `isWideDesktop`) → leave alone

   To reset on slug change:
   - `images` (image attachments)
   - `showFileBrowser`, `mobileProseFile`, `showTaskApproval`, `showFirstTaskWelcome`, `contextExhaustedExpanded`, `abandoningContextExhausted`
   - `error`, `conversationIdForSSE` (verify the load effect resets these)
   - Refs: `sendingMessagesRef`, `seedHydratedRef`, `cachedMsgCountRef`, the `*Ref` mirrors

   Providers inside the page (`ReviewNotesProvider`, `DiffViewerStateProvider`) need their state reset on slug change. Either accept a `slug` prop and reset internally on change, or wrap them in a small keyed boundary. **Important**: this keyed boundary, if used, must wrap *only* the providers, not the rendering tree — its purpose is state reset, not unmount. Prefer the prop-driven reset if it's clean.

A2. **Move `FileExplorerProvider` from `DesktopLayout` into the conversation scope.** Currently it lives in `DesktopLayout` (one instance for the whole app), so the open file persists across slug changes. Two viable placements:

   - Inside `ConversationPage`, alongside `ReviewNotesProvider` / `DiffViewerStateProvider`. But `DesktopLayout` also renders `FileExplorerPanel` (the left-side tree), which needs the same context — so the provider must wrap both. Push `FileExplorerProvider` down into a per-slug subtree that includes both the panel and the page.
   - Or: keep the provider where it is, but tie its state to the active slug and clear `proseReaderState` / `activeFile` on slug change.

   Pick the placement that gives the cleanest "this state belongs to this conversation" boundary. The first option is structurally honest ("provider lives at the scope its data lives at"); the second is less invasive but encodes the same correctness via an effect. Decide during implementation; document the choice in the PR.

   Whichever path, the user-visible behavior is: switching from conv ABC to conv DEF closes any open file/diff viewer. (DEF can independently re-open something later if the user clicks.)

A3. **Remove `key={slug}` from `KeyedConversationPage`** and `key={rootConvId}` from `KeyedChainPage`. Apply the same audit to `ChainPage` (per-chain state that the key was protecting → explicit resets). Keep the `KeyedX` wrapper functions if they still help readability, just without the `key=`.

A4. **Honest skeleton during slug-change load.** When `slug` changes and the new atom doesn't yet have `conversation` data, render a clean loading shell. Concretely:
   - The `if (!conversation)` early-return at `ConversationPage.tsx` L726 stays; widen its skeleton if the current one looks like a partial render of a real conversation. The skeleton must clearly read as "loading" and must not show data from the prior conversation.
   - Verify nothing in the surrounding tree (sidebar's `activeSlug`, `BreadcrumbBar`, `StateBar`, `FileExplorerPanel`) keeps rendering content tied to the previous slug. The sidebar's active highlight must move to the new slug *before or simultaneously with* the content swap.
   - The "loaded shell with skeleton messages" optimization is **out of scope** — don't render any panel that would show stale or wrong-conversation data.

A5. **Verify SSE reconnects cleanly on slug change.** `useConnection({ conversationId: conversationIdForSSE, dispatch })` keys the EventSource on `conversationIdForSSE`. Confirm that on slug change (no remount) the old EventSource is torn down and a new one opens. Add a test if one doesn't exist.

### Part B: stop the sidebar from churning

B1. **Make `setConversations` idempotent in `DesktopLayout.loadConversations`.** Use `(id, updated_at)` per row as the change signature: server bumps `updated_at` on every mutation that should affect display (state transitions, message inserts, archive, rename, etc. — see `src/db.rs`). If every row matches the previous list by `(id, updated_at)` in order, skip the `setState` entirely. Same for `archivedConversations`.

   `id` alone isn't enough — message counts tick, states flip, slugs get renamed, all without changing the ID set. `updated_at` is the server's existing change marker; reusing it gives us a one-field comparison. Extract the comparison into a small pure helper so it's unit-testable without rendering.

B2. **Drop the per-navigation refresh.** Delete the `useEffect(() => { if (isDesktop) loadConversations(true); }, [location.pathname, ...])` in `DesktopLayout` (~L90–92). Verify every case currently relying on it is covered by other paths:
   - New conversation creation → `onConversationCreated` callback
   - Archive / unarchive / delete (single + chain) → already call `onConversationCreated`
   - Rename → already calls `onConversationCreated`
   - Hard-delete from cascade → `phoenix:conversation-hard-deleted` window event
   - Otherwise → 5s poll

   If any case isn't covered, fix it at the source rather than reinstating the pathname effect.

B3. **(Optional polish — only if a few lines)** Pin the active conversation in place during user interaction: if `activeSlug` is set and a poll would move it, keep it visible at its current position for that frame. If it grows beyond ~10 lines, capture as a follow-up `taskmd new` and ship without.

### Part C: verification

C1. Manual QA — navigation: rapidly click `/c/foo` → `/c/bar` → `/c/baz`. The sidebar's active highlight and the main content must transition together; the user must never see ABC's content under a DEF selection. Loading skeletons are fine; stale content is a bug.

C2. Manual QA — file viewer scope: in conv A, open a file in the prose-reader. Switch to conv B. The viewer must close (or show conv-B-scoped state). Switching back to conv A may either re-show the file or start fresh — pick one and document; both are honest, "stale across switch" is not.

C3. Manual QA — idle sidebar: open the app, leave it idle for 30s on a quiet conversation list. Use React DevTools "Highlight updates" — the sidebar should not re-render. (The state-dot pulse animation is CSS-driven and independent of renders.)

C4. Manual QA — invisible activity: trigger backend activity on a non-active conversation (send a message in another tab). The sidebar updates that row, but no other row should visually re-render.

C5. `./dev.py check` — clippy + fmt + tests + task validation + codegen guard.

C6. Tests:
   - Unit test for the B1 `(id, updated_at)` comparison helper
   - Test that `images` resets on slug change in `ConversationPage` (proxy for the broader A1 reset behavior)
   - Test that file viewer state does not leak across slug change (A2)
   - Existing `ConversationList.test.tsx` continues to pass

## Acceptance Criteria

- Navigating `/c/foo` → `/c/bar` does **not** unmount `ConversationPage`.
- During the slug-change load gap, the user sees a clean loading skeleton — never content (transcript, viewer, breadcrumbs, file pane) from the previous conversation.
- `images`, overlay open-states, and provider-held per-conversation state are explicitly reset on slug change; verified by at least one test.
- File viewer state (`FileExplorerProvider.proseReaderState` / `activeFile`) is scoped to the active conversation: switching slugs does not show the previous conversation's open file.
- `DesktopLayout.loadConversations` skips `setConversations` / `setArchivedConversations` when row `(id, updated_at)` signatures are unchanged. An idle session produces zero sidebar re-renders beyond CSS animations.
- The `location.pathname`-triggered sidebar refetch is removed; every refresh case is covered by an existing change-driven path or the 5s poll.
- `./dev.py check` passes (including codegen guard).
- New tests cover: B1 idempotency helper, A1 image-reset on slug change, A2 viewer-state-doesn't-leak.
- (Nice-to-have) When a poll reorders the list, the active conversation row stays put if it would have moved.


## Progress

