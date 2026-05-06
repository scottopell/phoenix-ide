// Idempotency helper for the sidebar conversation list refresh.
//
// `DesktopLayout.loadConversations` polls every 5s. Without this check, every
// poll replaces the `conversations` state with a freshly-allocated array,
// causing every consumer (Sidebar, ConversationList, chain-grouping memo,
// every <li> row) to re-render even when nothing changed.
//
// We compare by `(id, updated_at)` per row in order. The server returns the
// list ordered by `updated_at DESC`, and bumps `updated_at` on every mutation
// that should affect display (state transitions, message inserts, archive,
// rename — see src/db.rs). So `(id, updated_at)` is a sufficient signature for
// “something the sidebar would render differently changed.”
//
// Pure function, no React deps — unit-tested independently.

import type { Conversation } from '../api';

/**
 * True iff `a` and `b` describe the same sidebar render: same length, same
 * order, same `(id, updated_at)` for every row.
 */
export function conversationListsEqual(
  a: readonly Conversation[],
  b: readonly Conversation[],
): boolean {
  if (a === b) return true;
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) {
    const x = a[i]!;
    const y = b[i]!;
    if (x.id !== y.id) return false;
    if (x.updated_at !== y.updated_at) return false;
  }
  return true;
}
