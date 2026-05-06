/**
 * Generic external store keyed by `K`, holding atoms of shape `S`, mutated
 * by actions of shape `A` through a pure reducer. Per-key subscriptions
 * mean that a dispatch into key X never re-renders consumers of key Y.
 *
 * Designed to be paired with React's `useSyncExternalStore` (see e.g.
 * `useConversationAtom`). The store itself is framework-agnostic and
 * unit-testable without a renderer.
 *
 * Why this exists as a primitive (task 08682): both `ConversationStore`
 * (slug-keyed conversation atoms) and `ChainStore` (rootConvId-keyed chain
 * atoms) had nearly identical shapes — the same Map of atoms, the same
 * Map-of-Sets of listeners, the same dispatch-then-notify-only-this-key
 * algorithm. Promoting that into `RoutedStore` lets each domain store
 * specialise the atom shape and the action union without re-implementing
 * the routing.
 *
 * The `getInitial` factory is invoked lazily when a key is first observed
 * (via `getSnapshot` or `dispatch`). Atoms are never garbage-collected
 * by the store — callers that need eviction can compose a thin layer on
 * top.
 *
 * Dead-atom semantics: `dispatch(key, action)` for a key that has never
 * been observed *does* create the atom and apply the action. This is the
 * same behaviour the pre-extraction `ConversationStore.dispatch` had, and
 * it matters for SSE handlers whose closures may dispatch slightly after
 * the consumer that originally observed the key has unmounted.
 */
export class RoutedStore<K, S, A> {
  private atoms = new Map<K, S>();
  private listenersByKey = new Map<K, Set<() => void>>();
  private anyListeners = new Set<() => void>();
  private readonly getInitial: (key: K) => S;
  private readonly reducer: (atom: S, action: A) => S;

  constructor(getInitial: (key: K) => S, reducer: (atom: S, action: A) => S) {
    this.getInitial = getInitial;
    this.reducer = reducer;
  }

  /**
   * Get the current atom for `key`, creating a fresh initial atom if none
   * exists yet. The returned reference is stable until the next dispatch
   * that actually changes the atom.
   */
  getSnapshot(key: K): S {
    let atom = this.atoms.get(key);
    if (atom === undefined) {
      atom = this.getInitial(key);
      this.atoms.set(key, atom);
    }
    return atom;
  }

  /** Subscribe to changes for a specific key only. */
  subscribe(key: K, listener: () => void): () => void {
    let set = this.listenersByKey.get(key);
    if (!set) {
      set = new Set();
      this.listenersByKey.set(key, set);
    }
    set.add(listener);
    return () => {
      const current = this.listenersByKey.get(key);
      if (!current) return;
      current.delete(listener);
      if (current.size === 0) {
        this.listenersByKey.delete(key);
      }
    };
  }

  /**
   * Apply an action through the reducer. If the reducer returns the same
   * reference (no-op), nothing is emitted. Otherwise, only the listeners
   * for this key are notified — unrelated keys do not re-render.
   */
  dispatch(key: K, action: A): void {
    const current = this.getSnapshot(key);
    const next = this.reducer(current, action);
    if (next === current) return;
    this.atoms.set(key, next);
    this.notify(key);
    this.notifyAny();
  }

  /**
   * Direct atom replacement. Subclasses use this for upserts driven by
   * sources other than the action reducer (e.g. polling fills in
   * snapshot-only data). Listeners are notified iff the new reference
   * differs from the current one.
   *
   * Returns true iff the atom changed.
   */
  protected setAtom(key: K, next: S): boolean {
    const current = this.getSnapshot(key);
    if (next === current) return false;
    this.atoms.set(key, next);
    this.notify(key);
    this.notifyAny();
    return true;
  }

  /** Iterate every (key, atom) currently held. Used by list-derivation
   *  hooks that need to scan the whole map. */
  protected entries(): IterableIterator<[K, S]> {
    return this.atoms.entries();
  }

  /** Read an atom without creating one if absent. Returns undefined
   *  for unobserved keys. Use {@link getSnapshot} when the caller wants
   *  the lazy-create behaviour. */
  protected atomByKey(key: K): S | undefined {
    return this.atoms.get(key);
  }

  /** Remove an atom from the store. Notifies per-key listeners and
   *  the any-listener if the atom existed. Returns true iff something
   *  was removed. Listener buckets for the key are kept — future
   *  subscriptions may still want to learn when a fresh atom is
   *  observed under the same key. */
  protected removeAtom(key: K): boolean {
    if (!this.atoms.has(key)) return false;
    this.atoms.delete(key);
    this.notify(key);
    this.notifyAny();
    return true;
  }

  /**
   * Subscribe to ANY atom change anywhere in the store. Fires once per
   * mutation that produced a new atom reference, regardless of which
   * key changed. List-derivation hooks (e.g. `useConversationsList`)
   * use this so they re-derive whenever any atom in the store mutates.
   *
   * Note: this fires more often than per-key subscriptions — every
   * dispatch that changes any atom triggers it. Consumers should derive
   * a stable snapshot (e.g. with `(id, updated_at)` equality) so React
   * elides re-renders when the derived value is unchanged.
   */
  subscribeAny(listener: () => void): () => void {
    this.anyListeners.add(listener);
    return () => {
      this.anyListeners.delete(listener);
    };
  }

  private notify(key: K): void {
    const listeners = this.listenersByKey.get(key);
    if (!listeners) return;
    for (const listener of listeners) {
      listener();
    }
  }

  private notifyAny(): void {
    for (const listener of this.anyListeners) {
      listener();
    }
  }
}
