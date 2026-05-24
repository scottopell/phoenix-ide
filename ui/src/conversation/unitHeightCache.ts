import { useEffect, useMemo } from 'react';

// Per-conversation cache of measured render-unit heights, mirrored to
// sessionStorage so first-paint spacer geometry is exact across remounts
// (REQ-MLRU-013).
//
// Reads are O(1) from the in-memory Map. Writes coalesce on a short
// timer to batch ResizeObserver bursts during scroll; subscribers are
// notified synchronously on set so React sees the fresh value
// immediately. The sessionStorage mirror is best-effort: quota or
// availability failures degrade silently to memory-only operation.
//
// Eviction: sessionStorage entries persist for the browser session.
// The hard-delete cascade in `useConversationsRefresh` calls
// `UnitHeightCache.clearConversation(conversationId)` to drop matching
// entries when a conversation is removed, so deleted conversations
// don't leak per-unit entries.

const STORAGE_PREFIX = 'phoenix:hcache:';
const FLUSH_DELAY_MS = 16;

export class UnitHeightCache {
  private readonly heights = new Map<string, number>();
  private readonly listeners = new Set<() => void>();
  private readonly pendingWrites = new Set<string>();
  private flushTimer: ReturnType<typeof setTimeout> | null = null;

  /** Version counter incremented on each `set` that mutates the cache.
   *  Suitable as a `useSyncExternalStore` snapshot for triggering
   *  re-renders of spacer geometry. */
  public version = 0;

  constructor(public readonly conversationId: string | undefined) {
    this.hydrateFromStorage();
  }

  get(key: string): number | undefined {
    return this.heights.get(key);
  }

  set(key: string, height: number): void {
    const prev = this.heights.get(key);
    if (prev === height) return;
    this.heights.set(key, height);
    this.pendingWrites.add(key);
    this.scheduleFlush();
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

  /** Force-flush pending writes to sessionStorage. Useful before
   *  unmount or when transitioning state. */
  flush(): void {
    if (this.flushTimer !== null) {
      clearTimeout(this.flushTimer);
      this.flushTimer = null;
    }
    this.writePending();
  }

  /** Release all retained resources: cancel the pending flush timer
   *  (after one last drain), clear listeners, and clear the in-memory
   *  heights/pending sets. Call from the host component's unmount or
   *  conversation-change cleanup; without this, a setTimeout closure
   *  keeps the cache instance alive until the timer fires. */
  dispose(): void {
    this.flush();
    this.listeners.clear();
    this.heights.clear();
    this.pendingWrites.clear();
  }

  private writePending(): void {
    if (this.pendingWrites.size === 0) return;
    if (!this.conversationId) {
      this.pendingWrites.clear();
      return;
    }
    try {
      for (const key of this.pendingWrites) {
        const value = this.heights.get(key);
        if (value !== undefined) {
          sessionStorage.setItem(this.storageKey(key), String(value));
        }
      }
    } catch {
      // Quota exceeded or storage unavailable — degrade to memory-only.
    }
    this.pendingWrites.clear();
  }

  private scheduleFlush(): void {
    if (this.flushTimer !== null) return;
    this.flushTimer = setTimeout(() => {
      this.flushTimer = null;
      this.writePending();
    }, FLUSH_DELAY_MS);
  }

  private storageKey(unitKey: string): string {
    return `${STORAGE_PREFIX}${this.conversationId}:${unitKey}`;
  }

  private hydrateFromStorage(): void {
    if (!this.conversationId) return;
    try {
      const prefix = `${STORAGE_PREFIX}${this.conversationId}:`;
      for (let i = 0; i < sessionStorage.length; i++) {
        const k = sessionStorage.key(i);
        if (!k || !k.startsWith(prefix)) continue;
        const raw = sessionStorage.getItem(k);
        if (raw === null) continue;
        const n = Number(raw);
        if (!Number.isFinite(n)) continue;
        const unitKey = k.slice(prefix.length);
        this.heights.set(unitKey, n);
      }
    } catch {
      // sessionStorage unavailable — degrade to memory-only.
    }
  }

  /** Drop all sessionStorage entries for a conversation. Called from
   *  the conversation-delete cascade so deleted conversations don't
   *  leak per-unit entries. */
  static clearConversation(conversationId: string): void {
    try {
      const prefix = `${STORAGE_PREFIX}${conversationId}:`;
      const toRemove: string[] = [];
      for (let i = 0; i < sessionStorage.length; i++) {
        const k = sessionStorage.key(i);
        if (k && k.startsWith(prefix)) toRemove.push(k);
      }
      for (const k of toRemove) sessionStorage.removeItem(k);
    } catch {
      // sessionStorage unavailable
    }
  }
}

/** React hook that creates and manages a `UnitHeightCache` per
 *  conversation. The cache is replaced when conversationId changes;
 *  the prior cache is disposed (flushed, listeners cleared) via a
 *  useEffect cleanup.
 *
 *  useMemo-based construction is concurrent-mode-safe: a discarded
 *  render simply discards the unused cache instance — no side effects
 *  ran beyond the constructor's read-only sessionStorage scan. The
 *  previous useRef-mutation-in-render pattern would prematurely flush
 *  the committed cache if a render attempt was discarded. */
export function useUnitHeightCache(conversationId: string | undefined): UnitHeightCache {
  const cache = useMemo(
    () => new UnitHeightCache(conversationId),
    [conversationId],
  );

  useEffect(() => {
    // Cleanup runs when conversationId changes (new cache replaces this
    // one) and when the host unmounts. Either way, drain pending writes
    // and release the timer.
    return () => {
      cache.dispose();
    };
  }, [cache]);

  return cache;
}
