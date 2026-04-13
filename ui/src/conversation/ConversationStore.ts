import { createInitialAtom, conversationReducer } from './atom';
import type { ConversationAtom, SSEAction } from './atom';

/**
 * External store holding the per-slug conversation atoms.
 *
 * Using a vanilla TS store (as opposed to a single React state Map) lets each
 * `useConversationAtom(slug)` call subscribe to only its slug — so a streaming
 * token in conversation A does not cause conversation B's consumer to re-render.
 * Paired with `useSyncExternalStore` in the hook.
 */
export class ConversationStore {
  private atoms = new Map<string, ConversationAtom>();
  private listenersBySlug = new Map<string, Set<() => void>>();

  /**
   * Get the current atom for a slug, creating a fresh initial atom if none
   * exists yet. The returned reference is stable until the next dispatch
   * that actually changes the atom.
   */
  getSnapshot(slug: string): ConversationAtom {
    let atom = this.atoms.get(slug);
    if (atom === undefined) {
      atom = createInitialAtom();
      this.atoms.set(slug, atom);
    }
    return atom;
  }

  /** Subscribe to changes for a specific slug only. */
  subscribe(slug: string, listener: () => void): () => void {
    let set = this.listenersBySlug.get(slug);
    if (!set) {
      set = new Set();
      this.listenersBySlug.set(slug, set);
    }
    set.add(listener);
    return () => {
      const current = this.listenersBySlug.get(slug);
      if (!current) return;
      current.delete(listener);
      if (current.size === 0) {
        this.listenersBySlug.delete(slug);
      }
    };
  }

  /**
   * Apply an action through the reducer. If the reducer returns the same
   * reference (no-op), nothing is emitted. Otherwise, only the listeners
   * for this slug are notified — unrelated slugs do not re-render.
   */
  dispatch(slug: string, action: SSEAction): void {
    const current = this.getSnapshot(slug);
    const next = conversationReducer(current, action);
    if (next === current) return;
    this.atoms.set(slug, next);
    const listeners = this.listenersBySlug.get(slug);
    if (listeners) {
      for (const listener of listeners) {
        listener();
      }
    }
  }
}
