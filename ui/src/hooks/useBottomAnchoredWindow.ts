import { useState, useRef, useLayoutEffect, useEffect, type RefObject } from 'react';

/**
 * Bottom-anchored render-virtualization window.
 *
 * Only the last `INITIAL_WINDOW` messages render as real subtrees on mount;
 * everything older collapses into ONE estimated-height top spacer. Because the
 * user is pinned to the bottom on a conversation switch, that spacer is always
 * OFFSCREEN above the viewport — its rough `COLLAPSED_EST_PX` height only
 * affects the scrollbar thumb, never viewport content. This is what structurally
 * prevents the two rejected-v1 regressions:
 *   1. "switch not pinned to bottom" — the newest message is always inside the
 *      initial window (indices [count-INITIAL_WINDOW, count)), so it is a REAL
 *      row the ResizeObserver scroll-to-bottom can land on.
 *   2. "estimate -> measured scroll jump" — only REAL measured rows ever enter
 *      the viewport. Revealing older rows on scroll-up prepends REAL rows and
 *      shrinks the spacer; the net height delta is applied to scrollTop in a
 *      layout effect (exact compensation, the revealed rows are not estimated).
 *      The only estimated geometry is the offscreen top spacer.
 */

export const INITIAL_WINDOW = 12;
export const EXPAND_BATCH = 12;
export const EXPAND_TRIGGER_PX = 600;
export const COLLAPSED_EST_PX = 360;
export const RESTORE_OVERSCAN = 4;

interface UseBottomAnchoredWindowArgs {
  messageCount: number;
  conversationId: string | undefined;
  scrollRootRef: RefObject<HTMLElement | null>;
  /**
   * Saved scroll pixel offset for REQ-CONV-013 restore, read synchronously on
   * the restoring mount. The initial window widens so the estimated spacer ends
   * before the saved offset, with extra real rows above the viewport as a buffer.
   * This preserves bottom-window virtualization for common bottom-pinned revisits
   * while avoiding a restored viewport that lands wholly inside the spacer. The
   * render-units follow-up will replace this estimate-bound compromise with
   * measured render-unit geometry.
   */
  savedScrollPos?: number | null;
}

interface UseBottomAnchoredWindowResult {
  /** Messages at index >= this render REAL; [0, firstRenderedIndex) collapse to one spacer. */
  firstRenderedIndex: number;
  /** Per-collapsed-message estimated height for the top spacer only. */
  collapsedEstPx: number;
}

function computeDefaultStart(messageCount: number): number {
  return Math.max(0, messageCount - INITIAL_WINDOW);
}

/**
 * Returns the initial `firstRenderedIndex` for a given mount, widening the
 * window when a saved scroll position must land in real content.
 */
function computeInitialStart(
  messageCount: number,
  savedScrollPos: number | null | undefined,
): number {
  const defaultStart = computeDefaultStart(messageCount);
  if (savedScrollPos != null) {
    const rowsBeforeRestore = Math.floor(savedScrollPos / COLLAPSED_EST_PX);
    return Math.max(0, Math.min(defaultStart, rowsBeforeRestore - RESTORE_OVERSCAN));
  }
  return defaultStart;
}

export function useBottomAnchoredWindow({
  messageCount,
  conversationId,
  scrollRootRef,
  savedScrollPos,
}: UseBottomAnchoredWindowArgs): UseBottomAnchoredWindowResult {
  // The DEFAULT window is DERIVED from messageCount every render (messages
  // load async AFTER mount: messageCount goes 0 -> N, and a one-shot useState
  // init would freeze firstRenderedIndex at computeInitialStart(0)=0 and
  // never virtualize — the v2 init-timing bug). Only an explicit user
  // scroll-up "pins" the window to a lower index; until then it tracks the
  // bottom reactively.
  const [userExpandedWindow, setUserExpandedWindow] = useState<{
    conversationId: string | undefined;
    index: number;
  } | null>(null);
  const prevScrollHeightRef = useRef<number | null>(null);

  const userExpandedIndex = userExpandedWindow !== null
    && userExpandedWindow.conversationId === conversationId
    ? userExpandedWindow.index
    : null;

  const firstRenderedIndex =
    userExpandedIndex !== null
      ? userExpandedIndex
      : computeInitialStart(messageCount, savedScrollPos);

  useLayoutEffect(() => {
    prevScrollHeightRef.current = null;
  }, [conversationId]);

  // Scroll-compensation bookkeeping: capture scrollHeight BEFORE the state
  // update that shrinks firstRenderedIndex, then in a layout effect add the
  // net growth back to scrollTop so viewport content stays visually fixed.
  const pendingFirstIndexRef = useRef(firstRenderedIndex);

  useLayoutEffect(() => {
    const el = scrollRootRef.current;
    if (el && prevScrollHeightRef.current !== null) {
      const delta = el.scrollHeight - prevScrollHeightRef.current;
      if (delta !== 0) {
        el.scrollTop += delta;
      }
      prevScrollHeightRef.current = null;
    }
    pendingFirstIndexRef.current = firstRenderedIndex;
  }, [firstRenderedIndex, scrollRootRef]);

  // Hook's OWN scroll listener — does NOT touch the existing handleScroll.
  useEffect(() => {
    const el = scrollRootRef.current;
    if (!el) return;

    const onScroll = () => {
      if (pendingFirstIndexRef.current <= 0) return;
      const spacerHeight = pendingFirstIndexRef.current * COLLAPSED_EST_PX;
      if (el.scrollTop - spacerHeight > EXPAND_TRIGGER_PX) return;
      // Capture pre-update geometry for exact scroll compensation.
      prevScrollHeightRef.current = el.scrollHeight;
      const next = Math.max(0, pendingFirstIndexRef.current - EXPAND_BATCH);
      pendingFirstIndexRef.current = next;
      setUserExpandedWindow({ conversationId, index: next });
    };

    el.addEventListener('scroll', onScroll, { passive: true });
    return () => el.removeEventListener('scroll', onScroll);
    // Re-attach when conversation changes so the listener observes the
    // (possibly remounted) scroll root and resets its expansion gate.
  }, [scrollRootRef, conversationId]);

  return { firstRenderedIndex, collapsedEstPx: COLLAPSED_EST_PX };
}
