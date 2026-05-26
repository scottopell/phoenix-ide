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
 * Mount lands pinned to the bottom (REQ-MLRU-005). Saved-scroll
 * restoration was removed alongside REQ-CONV-013; the previous
 * `savedAnchor` argument and `RESTORE_OVERSCAN` constant are gone.
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

export const KIND_ESTIMATES: Record<HistoricalUnit['kind'], number> = {
  user: 100,
  skill: 80,
  agent_turn: 400,
  system: 100,
  pending_user: 100,
};

export interface UseBottomAnchoredWindowArgs {
  historicalUnits: HistoricalUnit[];
  conversationId: string | undefined;
  scrollRootRef: RefObject<HTMLElement | null>;
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
 *  Always bottom-pinned to the last `INITIAL_WINDOW` units. */
export function computeInitialStart(units: HistoricalUnit[]): number {
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

const noopSubscribe = (): (() => void) => () => {};

export function useBottomAnchoredWindow({
  historicalUnits,
  conversationId,
  scrollRootRef,
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

  const initialFirst = computeInitialStart(historicalUnits);
  const initialLast = historicalUnits.length;
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

  // Reset compensation bookkeeping on conversation change so stale
  // values from a prior conversation don't apply to the new one. Both
  // `prevScrollHeightRef` (expansion delta) and `prevSpacerHeightRef`
  // (spacer-height delta) are baselines from the previous conversation
  // and would produce nonsensical deltas after the swap — leaving them
  // dirty caused random landing positions on conversation switch.
  const prevConversationIdRef = useRef(conversationId);
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

  // Spacer-height compensation for ResizeObserver-driven measurement
  // settle: as kind-estimate fallbacks get replaced by measured values
  // the spacer height changes, and scrollTop needs an equal-and-opposite
  // bump to keep the visible content anchored.
  //
  // The effect must ONLY fire on measurement settle, not on structural
  // changes (historicalUnits length grew, firstRenderedUnitIndex moved,
  // conversation switched). Structural changes have their own paths:
  //   - sentinel expansion → exact scroll comp via prevScrollHeightRef
  //   - conversation switch → reseat without delta (mount lands bottom-pinned)
  //   - units hydration on initial mount → reseat without delta (the
  //     ResizeObserver in MessageList scrolls to bottom once content arrives)
  //
  // Earlier this effect fired on any spacerHeight change, which produced
  // a phantom delta on first message hydration and landed users at
  // arbitrary mid-conversation scroll positions.
  const prevSpacerHeightRef = useRef(spacerHeight);
  const prevStructureRef = useRef({
    length: historicalUnits.length,
    first: firstRenderedUnitIndex,
    last: lastRenderedUnitIndex,
  });
  useLayoutEffect(() => {
    const structureChanged =
      prevStructureRef.current.length !== historicalUnits.length
      || prevStructureRef.current.first !== firstRenderedUnitIndex
      || prevStructureRef.current.last !== lastRenderedUnitIndex;
    const conversationChanged = prevConversationIdRef.current !== conversationId;

    if (conversationChanged || structureChanged || prevScrollHeightRef.current !== null) {
      // Reseat baseline without applying delta — caller paths above
      // own the scroll positioning for these transitions.
      prevConversationIdRef.current = conversationId;
      prevSpacerHeightRef.current = spacerHeight;
      prevStructureRef.current = {
        length: historicalUnits.length,
        first: firstRenderedUnitIndex,
        last: lastRenderedUnitIndex,
      };
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
  }, [
    spacerHeight,
    scrollRootRef,
    conversationId,
    historicalUnits.length,
    firstRenderedUnitIndex,
    lastRenderedUnitIndex,
  ]);

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
