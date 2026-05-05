---
created: 2026-05-05
priority: p1
status: ready
artifact: ui/src/conversation/ConversationStore.ts
---

# unify-conversation-source-of-truth

## Summary

`Conversation` data lives in two places today:

- `DesktopLayout`'s `conversations: Conversation[]` and
  `archivedConversations`, populated by a 5s poll plus cache hydration.
- `ConversationStore`'s atoms, populated by SSE in real time and read by
  `ConversationPage`.

The same logical entity exists in two representations with no coordination.
`useConversationCwd` is a hand-wired bridge for the one field that bit hard
enough — `cwd` lags up to 5s on poll-only — and patches the gap by reading
through the store when the active slug matches. `branch_name`,
`conv_mode_label`, `home_dir`, archive state, and other fields that drive
UI in `FileExplorerPanel`, `BreadcrumbBar`, `Sidebar`, etc. have no
equivalent bridge and can be stale up to 5 seconds.

This is the project's "no parallel representations of the same semantic
value" rule, broken by accident. The fix is to make `ConversationStore` the
single home for every `Conversation` snapshot the app holds. SSE updates
the store, the store updates every consumer, no per-field bridge.

## Context

Relevant files:
- `ui/src/conversation/ConversationStore.ts` — primary surface (expand)
- `ui/src/components/DesktopLayout.tsx` — deletes its `conversations`
  state, reads from store
- `ui/src/components/Sidebar.tsx`, `ConversationList.tsx` — consume new
  store hook
- `ui/src/components/FileExplorer/FileExplorerPanel.tsx` — drops
  `parentConversation` prop, reads from store
- `ui/src/components/BreadcrumbBar.tsx`, `StateBar.tsx` — same
- `ui/src/utils/conversationDiff.ts` — moves into the store as the
  per-row idempotency primitive
- `ui/src/conversation/useConversationCwd.ts` — deleted

## Plan

### Phase 1: Expand the store's per-atom surface

Today an atom holds the conversation only after the user has visited it
(via `ConversationPage`'s mount). Generalise: an atom can exist in a
**snapshot-only** state — the server returned it from a list endpoint, we
have its `Conversation` snapshot, but we have not opened SSE for it.

- `Atom = { conversation: Conversation, messages: ..., connection: ..., ... }`
- Snapshot-only atoms have empty `messages` and no `connection`. They are
  still valid for sidebar reads.
- Visiting a conversation upgrades the atom to "live" — opens SSE,
  hydrates messages from cache, etc. (This already exists; the change is
  that the atom is *pre-existing* when visited, not created on first
  visit.)

### Phase 2: Polling fills the store, not parallel state

Replace `DesktopLayout.loadConversations`:

- **Old:** polls `api.listConversations()` and `api.listArchivedConversations()`
  → `setConversations` / `setArchivedConversations`.
- **New:** polls the same endpoints → `store.upsertSnapshots(rows)`. The
  store reconciles each row into the matching atom (creating
  snapshot-only atoms for new rows, updating `conversation` on existing
  atoms).

The `(id, updated_at)` idempotency comparison from `conversationDiff.ts`
moves *into* the store as the per-atom "did this row actually change"
check. The pure helper itself stays — it is the right primitive — but the
upsert path uses it to decide whether to publish a new atom snapshot.

### Phase 3: Sidebar reads from the store

- New hook: `useConversationsList()` returns
  `{ active: Conversation[], archived: Conversation[] }` derived from the
  atom collection, sorted by `updated_at DESC`.
- The hook returns reference-stable arrays when the underlying atom set
  has not changed (conversationDiff applied at the list level).
- `Sidebar` and `ConversationList` consume `useConversationsList()`
  instead of taking the lists as props.
- `DesktopLayout` deletes its `conversations` / `archivedConversations`
  state entirely. `loadConversations` becomes
  `store.refreshSnapshots()` (or similar; verb is the store's choice).

### Phase 4: Delete the bridges

- `useConversationCwd` deleted; every call site reads
  `useConversation(slug).cwd` (or whatever the natural store hook is
  named after the refactor — likely `useConversationAtom(slug).conversation.cwd`).
- `parentConversation` prop drilling from `DesktopLayout` to
  `FileExplorerPanel` is replaced by direct store reads inside the panel.
- Audit `BreadcrumbBar`, `StateBar`, and any other component consuming
  per-conv fields — convert to store reads.

### Phase 5: SSE-driven updates already wire through

This is the win: SSE updates the atom in real time, and every consumer
(sidebar, breadcrumb, file panel) sees the update immediately. No 5s lag,
no per-field bridge. The poll becomes a backstop for "we missed an SSE
event" / "this tab was offline" rather than the primary data path.

### Phase 6: Online/offline + cache hydration

- The cache hydration path (`cacheDB.getAllConversations()`) becomes
  another upsert-into-store source. The store's per-row `(id, updated_at)`
  check keeps cache from clobbering fresh server data (panel Concurrency
  finding #4).
- Verify the offline → online transition: when `navigator.onLine` flips,
  trigger a refresh; the upsert reconciles any drift.
- Verify the in-flight-poll-vs-onConversationCreated race (panel
  Concurrency finding #3): a manual upsert from a creation callback
  should not be blocked by an in-flight poll. The store handles ordering
  via `(id, updated_at)`; the loadingRef guard in `DesktopLayout` is
  deleted along with `loadConversations`.

### Phase 7: Tests

- Unit tests for `store.upsertSnapshots` — idempotency, partial updates,
  archive transitions, deletes (rows that disappear from the list).
- **Integration**: SSE-driven mutation on conv A while sidebar is
  rendered; verify the sidebar row updates within one render frame, not
  on the next poll.
- **Cache-clobber regression**: write a regression test that the new
  upsert path cannot produce the "stale cache overwrites fresh server
  data" ordering.
- All existing sidebar tests pass.
- `conversationListsEqual` tests stay (the helper still exists, just
  consumed inside the store).

## Acceptance Criteria

- `DesktopLayout` holds zero `Conversation` arrays as React state.
- `ConversationStore` is the single source of truth for every
  `Conversation` snapshot the UI displays.
- `useConversationCwd` is deleted; no other per-field bridges remain.
- `parentConversation` prop drilling from `DesktopLayout` is gone;
  consumers read from the store directly.
- SSE-driven mutations on a non-active conversation update the sidebar
  within one render frame (no 5s lag). New test exercises this.
- Polling continues to backstop missed SSE events; cache hydration
  cannot clobber fresher server data. Regression tests in place.
- `./dev.py check` passes.

## Dependencies

- **Task A** (routed-store pattern + ChainPage migration) lands first.
  The store pattern should be settled before this task expands its
  responsibilities.
- Task B (epoch stamping) is independent of this task and can land in
  any order.
