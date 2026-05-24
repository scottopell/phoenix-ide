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
   *  collapse into the spacer. */
  firstRenderedUnitIndex: number;
  /** Pixel height of the top spacer, computed from per-kind estimates
   *  over the collapsed prefix. */
  spacerHeight: number;
  /** Callback ref for the sentinel `<div aria-hidden />` placed between
   *  the spacer and the rendered slice. Using a callback ref (not a
   *  RefObject) makes the IntersectionObserver wiring re-fire whenever
   *  the sentinel mounts/unmounts — critical for empty-then-grow
   *  conversations where the sentinel is conditionally rendered. */
  topSentinelRef: (node: HTMLDivElement | null) => void;
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

  // Sentinel is stored as state via a callback ref so the
  // IntersectionObserver effect re-runs whenever the DOM node mounts or
  // unmounts. A useRef-based sentinel would never trigger the effect
  // when the node attaches on a later render (e.g., empty conversation
  // grows past INITIAL_WINDOW within the same session).
  const [sentinelEl, setSentinelEl] = useState<HTMLDivElement | null>(null);
  const topSentinelRef = useCallback((node: HTMLDivElement | null) => {
    setSentinelEl(node);
  }, []);

  const prevScrollHeightRef = useRef<number | null>(null);
  const pendingFirstIndexRef = useRef(0);
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

  // Spacer-height compensation: when measured heights land via the
  // height cache and shift spacerHeight, the content below the spacer
  // visually shifts by the same delta. Adjust scrollTop to keep the
  // visible content anchored.
  //
  // Skipped while an expansion is in flight (prevScrollHeightRef !==
  // null) — that path's scrollHeight delta already includes the spacer
  // change, and applying both compensations would double-correct.
  const prevSpacerHeightRef = useRef(spacerHeight);
  useLayoutEffect(() => {
    if (prevScrollHeightRef.current !== null) {
      // Expansion-driven compensation is handling this commit.
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

  // IntersectionObserver on the sentinel. The sentinel sits at the
  // structural boundary between the collapsed spacer and the rendered
  // slice; when it crosses into the buffered viewport the window
  // expands by EXPAND_BATCH units. The `sentinelEl` state in the deps
  // makes this effect re-run when the sentinel mounts on a later
  // render — without it, an empty-conversation grow-path would never
  // attach the observer.
  useEffect(() => {
    const root = scrollRootRef.current;
    if (!root || !sentinelEl) return;
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
    observer.observe(sentinelEl);
    return () => observer.disconnect();
  }, [scrollRootRef, conversationId, sentinelEl]);

  return { firstRenderedUnitIndex, spacerHeight, topSentinelRef };
}
