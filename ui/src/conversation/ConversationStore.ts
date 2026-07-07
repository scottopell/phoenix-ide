import { createInitialAtom, conversationReducer } from './atom';
import type { ConversationAtom, SSEAction } from './atom';
import type { CachedPrSummary, Conversation } from '../api';
import { RoutedStore } from './RoutedStore';
import { notifyConversationSnapshotChange } from '../notifications';

function cachedPrEqual(a: CachedPrSummary | null | undefined, b: CachedPrSummary | null | undefined): boolean {
  if (a === b) return true;
  if (!a || !b) return false;
  return a.number === b.number
    && a.title === b.title
    && a.url === b.url
    && a.display_state === b.display_state
    && a.base === b.base
    && a.head === b.head;
}

interface SnapshotUpsertOptions {
  allowEqualTimestampSlugMove?: boolean;
}

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
 *     `connectionEpoch`, etc. on top of the existing `conversation`.
 *
 * Polling and cache hydration write through `upsertSnapshot` /
 * `upsertSnapshots`, which only touch `atom.conversation` and only
 * when the row is genuinely newer (`(id, updated_at)` per-row
 * idempotency). SSE-driven fields (`messages`,
 * `lastSequenceId`, `connectionEpoch`, etc.) are never affected by
 * snapshot upserts — a polling tick mid-stream cannot clobber a live
 * conversation's state.
 */
export class ConversationStore extends RoutedStore<string, ConversationAtom, SSEAction> {
  private slugByConvId = new Map<string, string>();

  constructor() {
    super(() => createInitialAtom(), conversationReducer);
  }

  override dispatch(slug: string, action: SSEAction): void {
    super.dispatch(slug, action);
    if (action.type === 'sse_init' || action.type === 'set_initial_data') {
      this.indexDirectHydration(slug);
    }
  }

  private indexDirectHydration(slug: string): void {
    const conversation = this.atomByKey(slug)?.conversation;
    if (!conversation) return;
    const indexedSlug = this.slugByConvId.get(conversation.id);
    if (!indexedSlug) {
      this.slugByConvId.set(conversation.id, slug);
      this.notifyChanged(slug);
      return;
    }
    const indexedConversation = this.atomByKey(indexedSlug)?.conversation;
    if (indexedSlug !== slug && (!indexedConversation || conversation.updated_at > indexedConversation.updated_at)) {
      this.slugByConvId.set(conversation.id, slug);
      this.notifyChanged(slug);
    }
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
  upsertSnapshot(slug: string, conversation: Conversation, options: SnapshotUpsertOptions = {}): boolean {
    const destination = this.getSnapshot(slug);
    const destinationConversation = destination.conversation;
    if (
      destinationConversation &&
      destinationConversation.id !== conversation.id &&
      conversation.updated_at <= destinationConversation.updated_at
    ) {
      return false;
    }
    const existingForId = this.bestConversationForId(conversation.id);
    if (existingForId) {
      // Monotonic: only accept newer or equal-but-different rows.
      // We compare ISO timestamps as strings (lexicographic = chronological).
      if (conversation.updated_at < existingForId.updated_at) {
        return false;
      }
      if (conversation.updated_at === existingForId.updated_at) {
        if (!cachedPrEqual(conversation.cached_pr, existingForId.cached_pr)) {
          const destinationConversation = destination.conversation;
          if (destinationConversation?.id === conversation.id && destinationConversation.slug === conversation.slug) {
            return this.setConversationSnapshot(
              slug,
              this.withCachedPr(destinationConversation, conversation.cached_pr),
              destination,
            );
          }
          const canonicalSlug = this.validIndexedSlugForId(conversation.id) ?? existingForId.slug;
          const canonicalDestination = this.getSnapshot(canonicalSlug);
          return this.setConversationSnapshot(
            canonicalSlug,
            this.withCachedPr(existingForId, conversation.cached_pr),
            canonicalDestination,
          );
        }
        const destinationConversation = destination.conversation;
        if (
          destinationConversation?.id === conversation.id &&
          destinationConversation.slug === conversation.slug
        ) {
          return false;
        }
        if (!options.allowEqualTimestampSlugMove) {
          return false;
        }
      }
    }
    return this.setConversationSnapshot(slug, conversation, destination);
  }

  private setConversationSnapshot(
    slug: string,
    conversation: Conversation,
    destination: ConversationAtom,
  ): boolean {
    this.removeInactiveAliases(conversation.id, slug);
    const destinationAtom = this.atomByKey(slug);
    const overwrittenId = destinationAtom?.conversation?.id;
    if (overwrittenId && overwrittenId !== conversation.id && this.slugByConvId.get(overwrittenId) === slug) {
      this.slugByConvId.delete(overwrittenId);
    }
    this.slugByConvId.set(conversation.id, slug);
    notifyConversationSnapshotChange(conversation);
    return this.setAtom(slug, { ...destination, conversation });
  }

  private withCachedPr(
    conversation: Conversation,
    cachedPr: CachedPrSummary | null | undefined,
  ): Conversation {
    const merged: Conversation = { ...conversation };
    if (cachedPr === undefined) {
      delete merged.cached_pr;
    } else {
      merged.cached_pr = cachedPr;
    }
    return merged;
  }

  /**
   * Bulk variant of {@link upsertSnapshot}. Returns the slugs whose
   * atoms actually changed (callers can use this to gate cache writes).
   */
  upsertSnapshots(rows: readonly Conversation[], options: SnapshotUpsertOptions = {}): string[] {
    const changed: string[] = [];
    for (const row of rows) {
      if (this.upsertSnapshot(row.slug, row, options)) {
        changed.push(row.slug);
      }
    }
    return changed;
  }

  replaceSlugSnapshot(oldSlug: string, conversation: Conversation): boolean {
    const newSlug = conversation.slug;
    if (oldSlug === newSlug) {
      return this.upsertSnapshot(newSlug, conversation);
    }

    const changed = this.upsertSnapshot(newSlug, conversation, { allowEqualTimestampSlugMove: true });
    const oldAtom = this.atomByKey(oldSlug);
    if (oldAtom?.conversation?.id === conversation.id) {
      this.removeAtom(oldSlug);
    }
    return changed;
  }

  private validIndexedSlugForId(convId: string): string | undefined {
    const indexedSlug = this.slugByConvId.get(convId);
    if (!indexedSlug) return undefined;
    const indexedConversation = this.atomByKey(indexedSlug)?.conversation;
    if (indexedConversation?.id === convId) return indexedSlug;
    this.slugByConvId.delete(convId);
    return undefined;
  }

  private bestConversationForId(convId: string): Conversation | undefined {
    let best: Conversation | undefined;
    for (const [, atom] of this.entries()) {
      const conversation = atom.conversation;
      if (conversation?.id !== convId) continue;
      if (!best || this.preferForSidebar(conversation, best)) best = conversation;
    }
    return best;
  }

  private removeInactiveAliases(convId: string, keepSlug: string): void {
    for (const [slug, atom] of this.entries()) {
      if (slug === keepSlug || atom.conversation?.id !== convId) continue;
      if (this.isSnapshotOnly(slug, atom)) {
        this.removeAtom(slug);
      }
    }
  }

  private isSnapshotOnly(slug: string, atom: ConversationAtom): boolean {
    return !this.hasSubscribers(slug)
      && atom.conversationId === null
      && atom.messages.length === 0
      && atom.connectionEpoch === null
      && atom.streamingBuffer === null
      && atom.systemPrompt === null;
  }

  private preferForSidebar(candidate: Conversation, existing: Conversation): boolean {
    if (candidate.updated_at > existing.updated_at) return true;
    if (candidate.updated_at < existing.updated_at) return false;
    const indexedSlug = this.slugByConvId.get(candidate.id);
    if (candidate.slug === indexedSlug && existing.slug !== indexedSlug) return true;
    if (existing.slug === indexedSlug && candidate.slug !== indexedSlug) return false;
    return candidate.slug > existing.slug;
  }

  /**
   * Read all currently-held conversation snapshots. Returns a fresh
   * array — callers that need reference stability across calls should
   * memoize on a derived signature (e.g. via `useSyncExternalStore` with
   * a snapshot equality function).
   */
  listSnapshots(): Conversation[] {
    const byId = new Map<string, Conversation>();
    for (const [, atom] of this.entries()) {
      const conversation = atom.conversation;
      if (!conversation) continue;
      const existing = byId.get(conversation.id);
      if (!existing || this.preferForSidebar(conversation, existing)) {
        byId.set(conversation.id, conversation);
      }
    }
    return [...byId.values()];
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
      if (convId && this.slugByConvId.get(convId) === slug) this.slugByConvId.delete(convId);
    }
    this.removeAtom(slug);
  }

  removeByConversationId(convId: string): string[] {
    const removed: string[] = [];
    for (const [slug, atom] of this.entries()) {
      if (atom.conversation?.id !== convId) continue;
      if (this.removeAtom(slug)) removed.push(slug);
    }
    this.slugByConvId.delete(convId);
    return removed;
  }
}
