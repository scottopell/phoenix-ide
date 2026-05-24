import {
  useState,
  useRef,
  useLayoutEffect,
  useEffect,
  useMemo,
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
 * Spacer height is the sum of per-kind estimates over the collapsed
 * prefix (see KIND_ESTIMATES). A future commit replaces estimates with
 * measured heights from a per-unit ResizeObserver cache; the spacer
 * computation accepts both shapes today by virtue of being a pure
 * function of the units and the index.
 *
 * Boundary expansion uses an `IntersectionObserver` rooted at the scroll
 * container, observing a sentinel placed between the spacer and the
 * rendered slice. Expansion is exact-scroll-compensated: `scrollHeight`
 * is captured before the state update, then `scrollTop` is adjusted by
 * the post-render delta in a layout effect so no visible jump occurs.
 *
 * See specs/messagelist-render-units/windowing.allium for the window
 * lifecycle state machine; this file is the React-bound implementation.
 */

export const INITIAL_WINDOW = 12;
export const EXPAND_BATCH = 12;
export const SENTINEL_ROOT_MARGIN = '600px 0px 0px 0px';
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
 * row-height variation in the prefix. Replaces the prior
 * `savedScrollTop / estimatedRowHeight` heuristic.
 *
 * Written by MessageList on visibility-hidden / unmount; read by this
 * hook on first mount per conversation. Persisted in localStorage at
 * `phoenix:msglist:anchor:{conversationId}` (managed by MessageList,
 * not this hook).
 */
export interface SavedScrollAnchor {
  topVisibleUnitKey: string;
  offsetWithinUnit: number;
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
   *  collapse into the spacer. */
  firstRenderedUnitIndex: number;
  /** Pixel height of the top spacer, computed from per-kind estimates
   *  over the collapsed prefix. */
  spacerHeight: number;
  /** Attach to a `<div aria-hidden />` placed between the spacer and the
   *  rendered slice; the IntersectionObserver uses it as the structural
   *  boundary that triggers expansion. */
  topSentinelRef: RefObject<HTMLDivElement>;
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
  let h = 0;
  const limit = Math.min(firstIdx, units.length);
  for (let i = 0; i < limit; i++) {
    const unit = units[i]!;
    const measured = getHeight(unit.key);
    h += measured ?? KIND_ESTIMATES[unit.kind];
  }
  return h;
}

const noopSubscribe = (): (() => void) => () => {};

export function useBottomAnchoredWindow({
  historicalUnits,
  conversationId,
  scrollRootRef,
  savedAnchor,
  heightCache,
}: UseBottomAnchoredWindowArgs): UseBottomAnchoredWindowResult {
  // Once the user scrolls up and triggers an expansion, the window is
  // pinned to that index for this conversation. Until then,
  // firstRenderedUnitIndex is derived reactively from
  // (historicalUnits, savedAnchor) so async unit arrivals keep the
  // tail visible.
  const [userExpandedWindow, setUserExpandedWindow] = useState<{
    conversationId: string | undefined;
    index: number;
  } | null>(null);

  const prevScrollHeightRef = useRef<number | null>(null);
  const pendingFirstIndexRef = useRef(0);
  const topSentinelRef = useRef<HTMLDivElement | null>(null);
  const conversationIdRef = useRef(conversationId);
  conversationIdRef.current = conversationId;

  const userExpandedIndex =
    userExpandedWindow !== null
    && userExpandedWindow.conversationId === conversationId
      ? userExpandedWindow.index
      : null;

  const firstRenderedUnitIndex =
    userExpandedIndex !== null
      ? userExpandedIndex
      : computeInitialStart(historicalUnits, savedAnchor ?? null);

  // Subscribe to the height cache via useSyncExternalStore so spacer
  // geometry re-renders when ResizeObserver writes land. The version
  // counter is a primitive — referentially stable across reads when
  // unchanged, so React skips re-renders for no-op set() calls.
  const cacheVersion = useSyncExternalStore(
    heightCache?.subscribe ?? noopSubscribe,
    () => heightCache?.version ?? 0,
    () => 0,
  );

  const spacerHeight = useMemo(
    () => computeSpacerHeight(
      historicalUnits,
      firstRenderedUnitIndex,
      heightCache ? (key) => heightCache.get(key) : undefined,
    ),
    // cacheVersion is in the deps so a measured-height write triggers
    // re-computation even though the cache reference is stable. The
    // exhaustive-deps lint can't see that closure read; the dep is the
    // signal, not the closure target.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [historicalUnits, firstRenderedUnitIndex, heightCache, cacheVersion],
  );

  // Reset compensation bookkeeping on conversation change so a stale
  // pre-expand scrollHeight from a prior conversation doesn't apply to
  // the new one.
  useLayoutEffect(() => {
    prevScrollHeightRef.current = null;
  }, [conversationId]);

  // Exact scroll compensation: capture scrollHeight BEFORE the state
  // update (in the observer callback), apply the delta to scrollTop
  // AFTER the render commit. Net: viewport content appears visually
  // fixed while new units appear in the spacer's prior region.
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
  }, [firstRenderedUnitIndex, scrollRootRef]);

  // IntersectionObserver on the sentinel. The sentinel sits at the
  // structural boundary between the collapsed spacer and the rendered
  // slice; when it crosses into the buffered viewport the window
  // expands by EXPAND_BATCH units.
  useEffect(() => {
    const root = scrollRootRef.current;
    const sentinel = topSentinelRef.current;
    if (!root || !sentinel) return;
    if (typeof IntersectionObserver === 'undefined') return;

    const observer = new IntersectionObserver(
      (entries) => {
        const entry = entries[0];
        if (!entry || !entry.isIntersecting) return;
        if (pendingFirstIndexRef.current <= 0) return;

        prevScrollHeightRef.current = root.scrollHeight;
        const next = Math.max(
          0,
          pendingFirstIndexRef.current - EXPAND_BATCH,
        );
        pendingFirstIndexRef.current = next;
        setUserExpandedWindow({
          conversationId: conversationIdRef.current,
          index: next,
        });
      },
      { root, rootMargin: SENTINEL_ROOT_MARGIN },
    );
    observer.observe(sentinel);
    return () => observer.disconnect();
  }, [scrollRootRef, conversationId]);

  return { firstRenderedUnitIndex, spacerHeight, topSentinelRef };
}
