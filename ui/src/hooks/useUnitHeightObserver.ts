import { useCallback, useEffect, useMemo, useRef } from 'react';
import type { UnitHeightCache } from '../conversation/unitHeightCache';
import type { HistoricalUnit } from '../conversation/renderUnits';

// Per-unit ResizeObserver wiring (REQ-MLRU-008). The hook returns a
// stable ref callback per unit key; attached observers write measured
// heights into the cache and remove themselves when the unit unmounts.
//
// The previous `getElement` API was used by REQ-MLRU-009's saved-anchor
// capture/restore, which was deprecated alongside REQ-CONV-013. The
// observer no longer maintains an element map.

export interface UnitHeightObserver {
  /** Returns a stable ref callback for the given unit. Calling the
   *  callback with a DOM element attaches a ResizeObserver; calling
   *  with null detaches it. */
  observe: (unit: HistoricalUnit) => (el: HTMLElement | null) => void;
}

export function useUnitHeightObserver(cache: UnitHeightCache): UnitHeightObserver {
  const callbacksRef = useRef(new Map<string, (el: HTMLElement | null) => void>());
  const observersRef = useRef(new Map<string, ResizeObserver>());

  // Tear down all per-unit observers when the cache changes (conversation
  // switch) or the host component unmounts. The ref callbacks themselves
  // are re-created lazily on the next observe() call.
  useEffect(() => {
    const observers = observersRef.current;
    const callbacks = callbacksRef.current;
    return () => {
      for (const o of observers.values()) o.disconnect();
      observers.clear();
      callbacks.clear();
    };
  }, [cache]);

  const observe = useCallback((unit: HistoricalUnit) => {
    const key = unit.key;
    const cached = callbacksRef.current.get(key);
    if (cached) return cached;
    const cb = (el: HTMLElement | null) => {
      const existing = observersRef.current.get(key);
      if (existing) {
        existing.disconnect();
        observersRef.current.delete(key);
      }
      if (!el) return;
      if (typeof ResizeObserver === 'undefined') return;
      const observer = new ResizeObserver((entries) => {
        for (const entry of entries) {
          const blockSize = entry.borderBoxSize?.[0]?.blockSize;
          const h = Math.round(blockSize ?? entry.contentRect.height);
          if (h > 0) cache.set(key, h);
        }
      });
      observer.observe(el);
      observersRef.current.set(key, observer);
    };
    callbacksRef.current.set(key, cb);
    return cb;
  }, [cache]);

  // useMemo the returned object so the wrapper reference is stable
  // across renders.
  return useMemo(() => ({ observe }), [observe]);
}
