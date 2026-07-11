# Fix stale sidebar row after deleting a conversation

## Problem

Deleting a conversation can leave it visible in the conversation sidebar until a full page reload.

Initial investigation points at the conversation store refresh path:

- `Sidebar` and `ConversationListPage` both call `api.deleteConversation(...)` and then trigger `refresh()` / `onConversationCreated()`.
- `useConversationsRefresh.refreshOnce()` fetches active + archived lists and calls `store.upsertSnapshots(...)`.
- `ConversationStore.upsertSnapshots(...)` only creates/updates rows that are present in the server response; it does not remove rows that are absent.
- The hard-delete window event (`phoenix:conversation-hard-deleted`) removes the atom immediately, but that event is currently produced from the deleted conversation's per-conversation SSE channel. If the sidebar/list is not subscribed to that deleted conversation, or the event is missed, the store keeps the stale snapshot forever (or at least until reload rebuilds from server/cache).

## Scope

Make successful list refreshes reconcile deletions, not just upserts, so the sidebar reflects the server's active+archived conversation set immediately after delete and after ordinary polling.

## Proposed implementation

1. Add a store-level reconciliation method, e.g. `reconcileSnapshots(rows: readonly Conversation[])`, that:
   - upserts all provided rows using the existing monotonic snapshot rules;
   - removes any snapshot atom whose `conversation.id` is not present in the provided authoritative active+archived server result;
   - updates `slugByConvId` consistently when rows are removed.
2. Use that reconciliation method in the network-success branch of `refreshOnce()` after `Promise.all([listConversations, listArchivedConversations])`.
3. Keep cache hydration as upsert-only so a stale IndexedDB cache cannot prune live store rows before the network response arrives.
4. Ensure cache writeback still receives the full active+archived set.
5. Add regression coverage for:
   - a row present in the store disappearing after a successful network refresh whose active+archived result omits it;
   - cache hydration does not prune absent rows;
   - hard-delete event path still clears slug-keyed/draft/unit-height state.

## Acceptance criteria

- After deleting a conversation from the sidebar, its row disappears without reloading the page.
- A missed per-conversation `conversation_hard_deleted` SSE event is not required for eventual immediate refresh correctness.
- Existing archive/unarchive/rename refresh behavior remains intact.
- `./dev.py check` passes.
