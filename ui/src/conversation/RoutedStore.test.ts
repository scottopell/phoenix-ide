import { describe, it, expect, vi } from 'vitest';
import { RoutedStore } from './RoutedStore';

// Minimal counter-style atom for testing the routing primitive without
// pulling in conversation-specific machinery.
interface Atom {
  count: number;
}
type Action = { type: 'INC' } | { type: 'ADD'; n: number } | { type: 'NOOP' };

function reducer(atom: Atom, action: Action): Atom {
  switch (action.type) {
    case 'INC':
      return { count: atom.count + 1 };
    case 'ADD':
      return { count: atom.count + action.n };
    case 'NOOP':
      return atom;
  }
}

const initial = (): Atom => ({ count: 0 });

describe('RoutedStore', () => {
  it('lazily creates an initial atom on first getSnapshot', () => {
    const store = new RoutedStore<string, Atom, Action>(initial, reducer);
    expect(store.getSnapshot('a')).toEqual({ count: 0 });
    // Same key returns the same reference until something mutates it.
    expect(store.getSnapshot('a')).toBe(store.getSnapshot('a'));
  });

  it('applies actions through the reducer', () => {
    const store = new RoutedStore<string, Atom, Action>(initial, reducer);
    store.dispatch('a', { type: 'INC' });
    expect(store.getSnapshot('a').count).toBe(1);
    store.dispatch('a', { type: 'ADD', n: 4 });
    expect(store.getSnapshot('a').count).toBe(5);
  });

  it('isolates keys: dispatch on one key does not affect another', () => {
    const store = new RoutedStore<string, Atom, Action>(initial, reducer);
    store.dispatch('a', { type: 'INC' });
    store.dispatch('a', { type: 'INC' });
    store.dispatch('b', { type: 'INC' });
    expect(store.getSnapshot('a').count).toBe(2);
    expect(store.getSnapshot('b').count).toBe(1);
  });

  it('does not notify listeners when the reducer returns the same reference', () => {
    const store = new RoutedStore<string, Atom, Action>(initial, reducer);
    const listener = vi.fn();
    store.subscribe('a', listener);
    store.dispatch('a', { type: 'NOOP' });
    expect(listener).not.toHaveBeenCalled();
    store.dispatch('a', { type: 'INC' });
    expect(listener).toHaveBeenCalledTimes(1);
  });

  it('notifies only listeners for the dispatched key', () => {
    const store = new RoutedStore<string, Atom, Action>(initial, reducer);
    const listenerA = vi.fn();
    const listenerB = vi.fn();
    store.subscribe('a', listenerA);
    store.subscribe('b', listenerB);
    store.dispatch('a', { type: 'INC' });
    expect(listenerA).toHaveBeenCalledTimes(1);
    expect(listenerB).not.toHaveBeenCalled();
  });

  it('unsubscribe stops further notifications and cleans up empty buckets', () => {
    const store = new RoutedStore<string, Atom, Action>(initial, reducer);
    const listener = vi.fn();
    const unsub = store.subscribe('a', listener);
    store.dispatch('a', { type: 'INC' });
    expect(listener).toHaveBeenCalledTimes(1);
    unsub();
    store.dispatch('a', { type: 'INC' });
    expect(listener).toHaveBeenCalledTimes(1);
  });

  it('dispatching on a never-observed key creates the atom (dead-atom semantics)', () => {
    // SSE handlers may dispatch slightly after every consumer of a key has
    // unmounted; the dispatch must be safe and the action must apply, not
    // be silently dropped, in case the key is re-observed later.
    const store = new RoutedStore<string, Atom, Action>(initial, reducer);
    store.dispatch('zombie', { type: 'INC' });
    expect(store.getSnapshot('zombie').count).toBe(1);
  });

  it('supports non-string keys', () => {
    // ChainStore uses (string) rootConvId; future stores may use composite
    // keys. Map equality is identity, which suits any key shape the caller
    // provides as long as references are stable.
    const k1 = { id: 1 };
    const k2 = { id: 1 }; // structurally equal, referentially distinct
    const store = new RoutedStore<typeof k1, Atom, Action>(initial, reducer);
    store.dispatch(k1, { type: 'INC' });
    expect(store.getSnapshot(k1).count).toBe(1);
    expect(store.getSnapshot(k2).count).toBe(0);
  });
});
