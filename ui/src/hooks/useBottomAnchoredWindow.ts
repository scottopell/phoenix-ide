import {
  useState,
  useRef,
  useLayoutEffect,
  useEffect,
  useMemo,
  useCallback,
  useSyncExternalStore,
  type RefObject,
} from 'react';
import type { HistoricalUnit } from '../conversation/renderUnits';
import type { UnitHeightCache } from '../conversation/unitHeightCache';

/**
 * Bottom-anchored render-virtualization window over a typed
 * `HistoricalUnit[]`.
 *
 * The window operates on render units, not raw messages — tool messages
 * are owned by their `agent_turn` unit and never appear here. The
 * rendered DOM is exactly `historicalUnits.slice(firstRenderedUnitIndex)`
 * preceded by one spacer div; everything older collapses into that
 * spacer.
 *
 * Boundary expansion uses an `IntersectionObserver` rooted at the scroll
 * container, observing a sentinel placed between the spacer and the
 * rendered slice. The sentinel is attached via a *callback ref* so the
 * observer is wired the instant the DOM node mounts — even when it
 * mounts on a later render than the hook's first run (e.g., a
 * conversation that started empty and grew past the initial window).
 *
 * Spacer height is measured-when-cached, kind-estimated otherwise. The
 * window applies exact scroll compensation in two distinct cases:
 *   - expansion (firstRenderedUnitIndex decreases): scrollHeight is
 *     captured before the state mutation, delta is applied after commit
 *   - spacer-height changes from measured-height writes (cache version
 *     bump): a separate compensation effect tracks spacerHeight changes
 *     and adjusts scrollTop by the same delta so the visible content
 *     stays anchored when ResizeObserver-driven measurements settle
 *
 * See specs/messagelist-render-units/windowing.allium for the window
 * lifecycle state machine; this file is the React-bound implementation.
 */

export const INITIAL_WINDOW = 12;
export const EXPAND_BATCH = 12;
export const MAX_RENDERED_UNITS = 48;
export const SENTINEL_ROOT_MARGIN = '600px 0px 0px 0px';
export const BOTTOM_SENTINEL_ROOT_MARGIN = '0px 0px 600px 0px';
export const RESTORE_OVERSCAN = 4;

export const KIND_ESTIMATES: Record<HistoricalUnit['kind'], number> = {
  user: 100,
  skill: 80,
  agent_turn: 400,
  system: 100,
};

/**
 * Saved scroll position keyed by render-unit identity. Restoring by unit
 * key + offset is structurally correct regardless of intervening
 * row-height variation in the prefix.
 *
 * `unitCountAtSave` lets the restore path detect that messages arrived
 * while the user was away (current historicalUnits.length >
 * unitCountAtSave) so the "↓ New messages" surface can fire on return —
 * preserving the REQ-CONV-013 affordance that the prior scrollTop+
 * msgcount pair provided.
 */
export interface SavedScrollAnchor {
  topVisibleUnitKey: string;
  offsetWithinUnit: number;
  /** Number of historical units present at save time. The restore
   *  path compares this to current historicalUnits.length to detect
   *  that messages arrived while the user was away and surface the
   *  "↓ New messages" affordance.
   *
   *  Optional only because the field is forward-compatible: anchors
   *  written by older app builds (without the field) still parse and
   *  simply don't surface the new-messages indicator. captureAnchor
   *  in MessageList always populates it on writes. */
  unitCountAtSave?: number;
}

export interface UseBottomAnchoredWindowArgs {
  historicalUnits: HistoricalUnit[];
  conversationId: string | undefined;
  scrollRootRef: RefObject<HTMLElement | null>;
  /** When set and the key exists in `historicalUnits`, the initial
   *  window widens so the anchored unit and `RESTORE_OVERSCAN` units
   *  above it are rendered. The actual scrollTop placement is the
   *  caller's responsibility (handled in MessageList's layout effect). */
  savedAnchor?: SavedScrollAnchor | null;
  /** Per-conversation measured-height cache. Spacer height uses
   *  measured values when present, per-kind estimates otherwise. */
  heightCache?: UnitHeightCache | null;
}

export interface UseBottomAnchoredWindowResult {
  /** Units at index >= this render real; [0, firstRenderedUnitIndex)
   *  collapse into the top spacer. */
  firstRenderedUnitIndex: number;
  /** Units at index < this render real; [lastRenderedUnitIndex, length)
   *  collapse into the bottom spacer. */
  lastRenderedUnitIndex: number;
  /** Pixel height of the top spacer, computed from measured heights or
   *  per-kind estimates over the collapsed prefix. */
  spacerHeight: number;
  /** Pixel height of the bottom spacer, computed from measured heights or
   *  per-kind estimates over the collapsed suffix. */
  bottomSpacerHeight: number;
  /** Callback ref for the sentinel `<div aria-hidden />` placed between
   *  the top spacer and the rendered slice. */
  topSentinelRef: (node: HTMLDivElement | null) => void;
  /** Callback ref for the sentinel placed between the rendered slice and
   *  the bottom spacer. */
  bottomSentinelRef: (node: HTMLDivElement | null) => void;
  /** Reset the bounded range to the bottom-pinned tail window. */
  resetToBottom: () => void;
}

/** Pure helper: where should `firstRenderedUnitIndex` start on mount?
 *  If a savedAnchor is provided and its key exists in `units`, widen the
 *  window so the anchored unit (plus `RESTORE_OVERSCAN` units above) is
 *  rendered. Otherwise default to bottom-pin (last `INITIAL_WINDOW`). */
export function computeInitialStart(
  units: HistoricalUnit[],
  savedAnchor: SavedScrollAnchor | null | undefined,
): number {
  if (savedAnchor) {
    const idx = units.findIndex((u) => u.key === savedAnchor.topVisibleUnitKey);
    if (idx >= 0) {
      return Math.max(0, idx - RESTORE_OVERSCAN);
    }
  }
  return Math.max(0, units.length - INITIAL_WINDOW);
}

/** Pure helper: sum measured-or-estimated heights over the collapsed
 *  prefix. `getHeight` returns the measured value for a unit key when
 *  available; missing entries fall back to the per-kind estimate. */
export function computeSpacerHeight(
  units: HistoricalUnit[],
  firstIdx: number,
  getHeight: (key: string) => number | undefined = () => undefined,
): number {
  return computeRangeHeight(units, 0, firstIdx, getHeight);
}

export function computeRangeHeight(
  units: HistoricalUnit[],
  startIdx: number,
  endIdx: number,
  getHeight: (key: string) => number | undefined = () => undefined,
): number {
  let h = 0;
  const start = Math.max(0, Math.min(startIdx, units.length));
  const end = Math.max(start, Math.min(endIdx, units.length));
  for (let i = start; i < end; i++) {
    const unit = units[i]!;
    const measured = getHeight(unit.key);
    h += measured ?? KIND_ESTIMATES[unit.kind];
  }
  return h;
}

function computeInitialEnd(
  units: HistoricalUnit[],
  firstIdx: number,
  savedAnchor: SavedScrollAnchor | null | undefined,
): number {
  if (savedAnchor) {
    const idx = units.findIndex((u) => u.key === savedAnchor.topVisibleUnitKey);
    if (idx >= 0) {
      return Math.min(
        units.length,
        Math.max(idx + 1 + RESTORE_OVERSCAN, firstIdx + INITIAL_WINDOW),
      );
    }
  }
  return units.length;
}

const noopSubscribe = (): (() => void) => () => {};

export function useBottomAnchoredWindow({
  historicalUnits,
  conversationId,
  scrollRootRef,
  savedAnchor,
  heightCache,
}: UseBottomAnchoredWindowArgs): UseBottomAnchoredWindowResult {
  const [windowRange, setWindowRange] = useState<{
    conversationId: string | undefined;
    first: number;
    last: number;
  } | null>(null);

  const [topSentinelEl, setTopSentinelEl] = useState<HTMLDivElement | null>(null);
  const topSentinelRef = useCallback((node: HTMLDivElement | null) => {
    setTopSentinelEl(node);
  }, []);
  const [bottomSentinelEl, setBottomSentinelEl] = useState<HTMLDivElement | null>(null);
  const bottomSentinelRef = useCallback((node: HTMLDivElement | null) => {
    setBottomSentinelEl(node);
  }, []);

  const prevScrollHeightRef = useRef<number | null>(null);
  const pendingFirstIndexRef = useRef(0);
  const pendingLastIndexRef = useRef(0);
  const conversationIdRef = useRef(conversationId);
  conversationIdRef.current = conversationId;

  const activeRange =
    windowRange !== null && windowRange.conversationId === conversationId
      ? windowRange
      : null;

  const initialFirst = computeInitialStart(historicalUnits, savedAnchor ?? null);
  const initialLast = computeInitialEnd(historicalUnits, initialFirst, savedAnchor ?? null);
  const firstRenderedUnitIndex = Math.max(
    0,
    Math.min(activeRange?.first ?? initialFirst, historicalUnits.length),
  );
  const lastRenderedUnitIndex = Math.max(
    firstRenderedUnitIndex,
    Math.min(activeRange?.last ?? initialLast, historicalUnits.length),
  );

  const resetToBottom = useCallback(() => {
    setWindowRange({
      conversationId: conversationIdRef.current,
      first: Math.max(0, historicalUnits.length - INITIAL_WINDOW),
      last: historicalUnits.length,
    });
  }, [historicalUnits.length]);

  // Subscribe to the height cache via useSyncExternalStore so spacer
  // geometry re-renders when ResizeObserver writes land. The version
  // counter is a primitive — referentially stable across reads when
  // unchanged, so React skips re-renders for no-op set() calls.
  const cacheVersion = useSyncExternalStore(
    heightCache?.subscribe ?? noopSubscribe,
    () => heightCache?.version ?? 0,
    () => 0,
  );

  const getCachedHeight = heightCache ? (key: string) => heightCache.get(key) : undefined;
  const spacerHeight = useMemo(
    () => computeSpacerHeight(
      historicalUnits,
      firstRenderedUnitIndex,
      getCachedHeight,
    ),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [historicalUnits, firstRenderedUnitIndex, heightCache, cacheVersion],
  );
  const bottomSpacerHeight = useMemo(
    () => computeRangeHeight(
      historicalUnits,
      lastRenderedUnitIndex,
      historicalUnits.length,
      getCachedHeight,
    ),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [historicalUnits, lastRenderedUnitIndex, heightCache, cacheVersion],
  );

  // Reset compensation bookkeeping on conversation change so a stale
  // pre-expand scrollHeight from a prior conversation doesn't apply to
  // the new one.
  useLayoutEffect(() => {
    prevScrollHeightRef.current = null;
  }, [conversationId]);

  // Exact scroll compensation for both prepend and bounded-window range
  // shifts: capture scrollHeight before the state mutation and apply the
  // committed delta before paint so the viewport's content anchor holds.
  useLayoutEffect(() => {
    const el = scrollRootRef.current;
    if (el && prevScrollHeightRef.current !== null) {
      const delta = el.scrollHeight - prevScrollHeightRef.current;
      if (delta !== 0) {
        el.scrollTop += delta;
      }
      prevScrollHeightRef.current = null;
    }
    pendingFirstIndexRef.current = firstRenderedUnitIndex;
    pendingLastIndexRef.current = lastRenderedUnitIndex;
  }, [firstRenderedUnitIndex, lastRenderedUnitIndex, scrollRootRef]);

  const prevSpacerHeightRef = useRef(spacerHeight);
  useLayoutEffect(() => {
    if (prevScrollHeightRef.current !== null) {
      prevSpacerHeightRef.current = spacerHeight;
      return;
    }
    const el = scrollRootRef.current;
    if (!el) {
      prevSpacerHeightRef.current = spacerHeight;
      return;
    }
    const delta = spacerHeight - prevSpacerHeightRef.current;
    if (delta !== 0) {
      el.scrollTop += delta;
    }
    prevSpacerHeightRef.current = spacerHeight;
  }, [spacerHeight, scrollRootRef]);

  useEffect(() => {
    const root = scrollRootRef.current;
    if (!root || !topSentinelEl) return;
    if (typeof IntersectionObserver === 'undefined') return;

    const observer = new IntersectionObserver(
      (entries) => {
        const entry = entries[0];
        if (!entry || !entry.isIntersecting) return;
        if (pendingFirstIndexRef.current <= 0) return;

        prevScrollHeightRef.current = root.scrollHeight;
        const nextFirst = Math.max(0, pendingFirstIndexRef.current - EXPAND_BATCH);
        const candidateLast = pendingLastIndexRef.current;
        const nextLast = Math.min(
          candidateLast,
          nextFirst + MAX_RENDERED_UNITS,
        );
        pendingFirstIndexRef.current = nextFirst;
        pendingLastIndexRef.current = nextLast;
        setWindowRange({
          conversationId: conversationIdRef.current,
          first: nextFirst,
          last: nextLast,
        });
      },
      { root, rootMargin: SENTINEL_ROOT_MARGIN },
    );
    observer.observe(topSentinelEl);
    return () => observer.disconnect();
  }, [scrollRootRef, conversationId, topSentinelEl]);

  useEffect(() => {
    const root = scrollRootRef.current;
    if (!root || !bottomSentinelEl) return;
    if (typeof IntersectionObserver === 'undefined') return;

    const observer = new IntersectionObserver(
      (entries) => {
        const entry = entries[0];
        if (!entry || !entry.isIntersecting) return;
        if (pendingLastIndexRef.current >= historicalUnits.length) return;

        prevScrollHeightRef.current = root.scrollHeight;
        const nextLast = Math.min(historicalUnits.length, pendingLastIndexRef.current + EXPAND_BATCH);
        const nextFirst = Math.max(
          0,
          Math.min(pendingFirstIndexRef.current + EXPAND_BATCH, nextLast - MAX_RENDERED_UNITS),
        );
        pendingFirstIndexRef.current = nextFirst;
        pendingLastIndexRef.current = nextLast;
        setWindowRange({
          conversationId: conversationIdRef.current,
          first: nextFirst,
          last: nextLast,
        });
      },
      { root, rootMargin: BOTTOM_SENTINEL_ROOT_MARGIN },
    );
    observer.observe(bottomSentinelEl);
    return () => observer.disconnect();
  }, [scrollRootRef, conversationId, bottomSentinelEl, historicalUnits.length]);

  return {
    firstRenderedUnitIndex,
    lastRenderedUnitIndex,
    spacerHeight,
    bottomSpacerHeight,
    topSentinelRef,
    bottomSentinelRef,
    resetToBottom,
  };
}
