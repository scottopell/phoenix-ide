import { useEffect, useMemo } from 'react';

// In-memory per-conversation cache of measured render-unit heights.
//
// REQ-MLRU-013's sessionStorage mirror was removed alongside REQ-CONV-013
// and REQ-MLRU-009: persistence existed solely to make the saved-scroll
// restore land precisely on first paint, and with that restore gone
// there is no first-paint geometry contract that persistence served.
// The cache is reconstructed per mount; first paint uses per-kind
// estimates from `useBottomAnchoredWindow`'s `KIND_ESTIMATES` and
// converges as ResizeObserver callbacks fire.
//
// Reads are O(1) from the in-memory Map. Subscribers are notified
// synchronously on `set` so React sees the fresh value immediately.

export class UnitHeightCache {
  private readonly heights = new Map<string, number>();
  private readonly listeners = new Set<() => void>();

  /** Version counter incremented on each `set` that mutates the cache.
   *  Suitable as a `useSyncExternalStore` snapshot for triggering
   *  re-renders of spacer geometry. */
  public version = 0;

  constructor(public readonly conversationId: string | undefined) {}

  get(key: string): number | undefined {
    return this.heights.get(key);
  }

  set(key: string, height: number): void {
    const prev = this.heights.get(key);
    if (prev === height) return;
    this.heights.set(key, height);
    this.version++;
    for (const listener of this.listeners) listener();
  }

  /** Bound subscribe so callers can pass directly to
   *  `useSyncExternalStore` without re-binding per render. */
  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  /** Release all retained resources. Call from the host component's
   *  unmount or conversation-change cleanup. */
  dispose(): void {
    this.listeners.clear();
    this.heights.clear();
  }
}

/** React hook that creates and manages a `UnitHeightCache` per
 *  conversation. The cache is replaced when conversationId changes;
 *  the prior cache is disposed via a useEffect cleanup. */
export function useUnitHeightCache(conversationId: string | undefined): UnitHeightCache {
  const cache = useMemo(
    () => new UnitHeightCache(conversationId),
    [conversationId],
  );

  useEffect(() => {
    return () => {
      cache.dispose();
    };
  }, [cache]);

  return cache;
}
