import { createInitialAtom, conversationReducer } from './atom';
import type { ConversationAtom, SSEAction } from './atom';
import type { Conversation } from '../api';
import { RoutedStore } from './RoutedStore';

/**
 * Per-slug conversation atoms.
 *
 * A specialization of {@link RoutedStore} parameterised by
 * (slug, ConversationAtom, SSEAction). Per-slug subscriptions mean a
 * streaming token in conversation A does not cause conversation B's
 * consumer to re-render.
 *
 * Task 08684 promotes this store to be the single source of truth for
 * every `Conversation` snapshot the UI displays. Atoms now exist in two
 * shapes:
 *
 *   - **snapshot-only**: the server returned the conversation from a
 *     list endpoint or the cache, but no consumer has opened SSE for it.
 *     `atom.conversation` is populated; `atom.messages` is empty;
 *     `atom.connectionEpoch` is null. Sidebar reads these for row
 *     rendering.
 *   - **live**: a `ConversationPage` mounted for this slug, opened SSE,
 *     and the SSE init / wire events have populated `messages`,
 *     `breadcrumbs`, `connectionEpoch`, etc. on top of the existing
 *     `conversation`.
 *
 * Polling and cache hydration write through `upsertSnapshot` /
 * `upsertSnapshots`, which only touch `atom.conversation` and only
 * when the row is genuinely newer (`(id, updated_at)` per-row
 * idempotency). SSE-driven fields (`messages`, `breadcrumbs`,
 * `lastSequenceId`, `connectionEpoch`, etc.) are never affected by
 * snapshot upserts — a polling tick mid-stream cannot clobber a live
 * conversation's state.
 */
export class ConversationStore extends RoutedStore<string, ConversationAtom, SSEAction> {
  private slugByConvId = new Map<string, string>();

  constructor() {
    super(() => createInitialAtom(), conversationReducer);
  }

  /**
   * Upsert a single conversation snapshot. Creates a snapshot-only atom
   * if the slug is unknown; otherwise updates `atom.conversation` if the
   * incoming row's `updated_at` is strictly newer than the held one
   * (defense against stale cache hydration overwriting a fresh server
   * response — see panel concurrency finding #4).
   *
   * Other atom fields are preserved untouched. SSE-driven mutations of a
   * live atom are not visible in the snapshot row — a poll tick must not
   * regress the conversation row to its `(updated_at)` from
   * `listConversations` if SSE has already advanced it. Today the server
   * bumps `conversation.updated_at` for SSE-driven mutations too, so the
   * monotonic check protects both directions.
   *
   * Returns true iff this atom changed.
   */
  upsertSnapshot(slug: string, conversation: Conversation): boolean {
    const current = this.getSnapshot(slug);
    if (current.conversation) {
      // Monotonic: only accept newer or equal-but-different rows.
      // We compare ISO timestamps as strings (lexicographic = chronological).
      if (conversation.updated_at < current.conversation.updated_at) {
        return false;
      }
      // Equal updated_at + same id => same logical row. No-op unless an
      // immutable id mismatch indicates this is actually a different
      // conversation under the same slug (data corruption — should not
      // happen). Use id mismatch as the only equal-timestamp tie-break.
      if (
        conversation.updated_at === current.conversation.updated_at &&
        conversation.id === current.conversation.id
      ) {
        return false;
      }
    }
    this.slugByConvId.set(conversation.id, slug);
    return this.setAtom(slug, { ...current, conversation });
  }

  /**
   * Bulk variant of {@link upsertSnapshot}. Returns the slugs whose
   * atoms actually changed (callers can use this to gate cache writes).
   */
  upsertSnapshots(rows: readonly Conversation[]): string[] {
    const changed: string[] = [];
    for (const row of rows) {
      if (this.upsertSnapshot(row.slug, row)) {
        changed.push(row.slug);
      }
    }
    return changed;
  }

  /**
   * Read all currently-held conversation snapshots. Returns a fresh
   * array — callers that need reference stability across calls should
   * memoize on a derived signature (e.g. via `useSyncExternalStore` with
   * a snapshot equality function).
   */
  listSnapshots(): Conversation[] {
    const out: Conversation[] = [];
    for (const [, atom] of this.entries()) {
      if (atom.conversation) out.push(atom.conversation);
    }
    return out;
  }

  /**
   * Reverse lookup: which slug owns a given `Conversation.id`? Returns
   * undefined if the id has not been observed via upsert or SSE init.
   *
   * Used by hard-delete cascade and other handlers that arrive with a
   * conversation_id but need to dispatch into the slug-keyed atom.
   */
  slugForId(convId: string): string | undefined {
    return this.slugByConvId.get(convId);
  }

  /**
   * Drop an atom entirely — used for hard-delete cascade. Notifies
   * per-key listeners so consumers can react (e.g. unmount), and the
   * any-listener so list-derivation hooks recompute.
   */
  remove(slug: string): void {
    const existing = this.atomByKey(slug);
    if (existing) {
      const convId = existing.conversation?.id;
      if (convId) this.slugByConvId.delete(convId);
    }
    if (this.removeAtom(slug)) {
      // notify already happened inside removeAtom
    }
  }
}
