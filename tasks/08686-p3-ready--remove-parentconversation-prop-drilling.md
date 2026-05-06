---
created: 2026-05-08
priority: p3
status: ready
artifact: ui/src/components/TaskViewer.tsx
---

# remove-parentconversation-prop-drilling

## Summary

The 08684 plan (Phase 4) called for removing the `parentConversation`
prop chain from `DesktopLayout → FileExplorerPanel → TaskViewer` and
letting `TaskViewer` read the active conversation directly from the
store. The chain still exists; closing this loose end is mechanical.

## Plan

- `TaskViewer` accepts `parentConversation: Conversation | null` and
  reads `parentConversation.cwd`, `.model`, `.id` to start work on a
  task. Switch to `useConversationSnapshot(activeSlug)` inside
  TaskViewer. The active slug is available from `useParams` or by
  passing the slug (not the full conversation) through the chain.
- Drop the `parentConversation` prop from `FileExplorerPanel`'s
  interface; it has no use of its own (only forwards to TaskViewer).
- Drop the `parentConversation={activeConversation}` prop pass in
  `DesktopLayout.tsx`.
- Delete the `useMemo` for `activeConversation` if nothing else in
  DesktopLayout still needs it (likely yes — it's also used for
  `branchName` and `conversationId` props on `FileExplorerPanel`,
  so check before removing).

## Acceptance

- `parentConversation` no longer appears as a prop in any component.
- `TaskViewer` works as before — 'Start work' creates a child
  conversation rooted at the parent's cwd/model.
- No regression in any existing TaskViewer interaction.
- `./dev.py check` passes.

## Why deferred from 08684

Mechanical, low-risk, low-traffic. Keeping 08684's PR focused on the
store-as-single-source-of-truth refactor; the prop drilling cleanup is
the last footgun that one PR has not removed.
