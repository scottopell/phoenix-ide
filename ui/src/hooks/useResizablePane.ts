import { useState, useCallback, useRef, useEffect, useLayoutEffect } from 'react';

export interface UseResizablePaneOptions {
  /** localStorage key (size persisted at `${key}`, collapsed at `${key}-collapsed`) */
  key: string;
  /** Absolute minimum size in px before collapse logic triggers */
  min: number;
  /** Absolute maximum size in px (number or function-of-viewport) */
  max: number | (() => number);
  /** Default size in px if nothing is persisted */
  defaultSize: number;
  /** Drag below this px → snap to collapsed */
  collapseThreshold?: number;
  /** Initial collapsed state when nothing is persisted (default: false) */
  defaultCollapsed?: boolean;
}

export interface UseResizablePaneResult {
  /** Current size in px (the value the parent should apply to width/height) */
  size: number;
  /** True when the pane is in its collapsed state */
  collapsed: boolean;
  /** Pointer-down handler to wire to PaneDivider.
   *
   *  `onLiveResize`, when supplied, is the transient live-drag channel: the hook
   *  calls it (coalesced to one call per animation frame) with the in-progress
   *  size/collapsed on every pointer move and does NOT commit React state during
   *  the drag. The consumer applies the value straight to the DOM (e.g. a CSS
   *  variable), so dragging the divider does not re-render the owning component
   *  subtree. React `size`/`collapsed` state is committed once, on pointer-up.
   *  Omit it to keep the legacy behaviour where state is committed during the
   *  drag (still frame-capped via rAF). */
  startDrag: (
    e: React.PointerEvent,
    axis: 'x' | 'y',
    invert?: boolean,
    onLiveResize?: (size: number, collapsed: boolean) => void,
  ) => void;
  /** Imperative collapse control (used by toggle buttons) */
  setCollapsed: (value: boolean) => void;
  /** Restore to last remembered non-collapsed size */
  expandFromCollapsed: () => void;
  /** Imperatively set the size in px (clamped to [min, max]). Used by
   *  keyboard resize handlers — arrow-key nudges from a focused
   *  separator role. */
  setSize: (px: number) => void;
}

function readNumber(key: string, fallback: number): number {
  try {
    const raw = localStorage.getItem(key);
    if (raw == null) return fallback;
    const n = parseFloat(raw);
    return Number.isFinite(n) ? n : fallback;
  } catch {
    return fallback;
  }
}

function readBool(key: string, fallback: boolean): boolean {
  try {
    const raw = localStorage.getItem(key);
    if (raw == null) return fallback;
    return raw === 'true';
  } catch {
    return fallback;
  }
}

function writeNumber(key: string, value: number): void {
  try {
    localStorage.setItem(key, String(value));
  } catch {
    // ignore
  }
}

function writeBool(key: string, value: boolean): void {
  try {
    localStorage.setItem(key, String(value));
  } catch {
    // ignore
  }
}

export function useResizablePane(options: UseResizablePaneOptions): UseResizablePaneResult {
  const { key, min, max, defaultSize, collapseThreshold, defaultCollapsed = false } = options;
  const collapsedKey = `${key}-collapsed`;

  const resolveMax = useCallback(() => (typeof max === 'function' ? max() : max), [max]);

  const clamp = useCallback(
    (n: number) => Math.max(min, Math.min(resolveMax(), n)),
    [min, resolveMax],
  );

  const [size, setSize] = useState<number>(() => clamp(readNumber(key, defaultSize)));
  const [collapsed, setCollapsedState] = useState<boolean>(() =>
    readBool(collapsedKey, defaultCollapsed),
  );

  const hydrationRef = useRef({ clamp, defaultSize, defaultCollapsed });
  hydrationRef.current = { clamp, defaultSize, defaultCollapsed };

  useLayoutEffect(() => {
    const hydration = hydrationRef.current;
    sizeInteracted.current = false;
    collapsedInteracted.current = false;
    setSize(hydration.clamp(readNumber(key, hydration.defaultSize)));
    setCollapsedState(readBool(collapsedKey, hydration.defaultCollapsed));
  }, [key, collapsedKey]);

  // Persistence is gated on real user interaction. Writing on mount makes the
  // initial default "sticky" — flipping the default in code never reaches
  // anyone whose first page-load already wrote the old default to storage.
  // The *Interacted refs flip in the imperative setters (startDrag,
  // setCollapsed, expandFromCollapsed, setSize); the viewport re-clamp effect
  // deliberately leaves them alone.
  const sizeInteracted = useRef(false);
  const collapsedInteracted = useRef(false);
  useEffect(() => {
    if (sizeInteracted.current) writeNumber(key, size);
  }, [key, size]);
  useEffect(() => {
    if (collapsedInteracted.current) writeBool(collapsedKey, collapsed);
  }, [collapsedKey, collapsed]);

  // Re-clamp on viewport resize (cheap)
  useEffect(() => {
    const handler = () => setSize((s) => clamp(s));
    window.addEventListener('resize', handler);
    return () => window.removeEventListener('resize', handler);
  }, [clamp]);

  const dragRef = useRef<{
    startCoord: number;
    startSize: number;
    axis: 'x' | 'y';
    invert: boolean;
    pointerId: number;
    onLiveResize: ((size: number, collapsed: boolean) => void) | undefined;
  } | null>(null);
  // Latest drag target awaiting a frame, and the pending rAF handle. Pointer
  // moves arrive faster than the display refresh (and faster than React can
  // usefully render); coalescing to one flush per frame caps the work at the
  // refresh rate whether the flush commits React state or drives the live
  // channel.
  const dragPendingRef = useRef<{ size: number; collapsed: boolean } | null>(null);
  const dragRafRef = useRef<number | null>(null);

  useEffect(
    () => () => {
      if (dragRafRef.current !== null) cancelAnimationFrame(dragRafRef.current);
    },
    [],
  );

  const startDrag = useCallback(
    (
      e: React.PointerEvent,
      axis: 'x' | 'y',
      invert = false,
      onLiveResize?: (size: number, collapsed: boolean) => void,
    ) => {
      const target = e.currentTarget as HTMLElement;
      try {
        target.setPointerCapture(e.pointerId);
      } catch {
        // Pointer may have been released between pointerdown firing and the
        // capture call (real browsers do this occasionally). Listeners below
        // still work without capture — drag just won't follow past the element
        // edge, which is acceptable degradation.
      }
      dragRef.current = {
        startCoord: axis === 'x' ? e.clientX : e.clientY,
        // When dragging out from a collapsed pane, treat the start size as `min`
        // so the first pixel of motion immediately uncollapses past the threshold.
        startSize: collapsed ? min : size,
        axis,
        invert,
        pointerId: e.pointerId,
        onLiveResize,
      };
      document.body.style.userSelect = 'none';
      document.body.style.cursor = axis === 'x' ? 'col-resize' : 'row-resize';

      // Committed `collapsed` tracked across this drag. On the live path the
      // continuous size never touches React, but a collapse *transition* must
      // still commit so markup keyed on `collapsed` (a sidebar/file-explorer
      // rail, the terminal's collapsed strip) switches state during the drag
      // rather than only on release. Transitions are rare (one threshold
      // crossing), so the commit cost is negligible.
      let liveCollapsed = collapsed;

      // Resolve the (size, collapsed) a proposed pixel delta maps to. Collapse
      // pins the remembered size at `min` so a later expand restores sensibly.
      const resolve = (proposed: number): { size: number; collapsed: boolean } =>
        collapseThreshold !== undefined && proposed < collapseThreshold
          ? { size: clamp(min), collapsed: true }
          : { size: clamp(proposed), collapsed: false };

      const commitCollapsed = (nextCollapsed: boolean) => {
        if (nextCollapsed === liveCollapsed) return false;
        liveCollapsed = nextCollapsed;
        setCollapsedState(nextCollapsed);
        return true;
      };

      const flushFrame = () => {
        dragRafRef.current = null;
        const pending = dragPendingRef.current;
        const drag = dragRef.current;
        if (!pending || !drag) return;

        if (drag.onLiveResize) {
          drag.onLiveResize(pending.size, pending.collapsed);
        } else {
          commitCollapsed(pending.collapsed);
          setSize(pending.size);
        }
      };

      const onMove = (ev: PointerEvent) => {
        const drag = dragRef.current;
        if (!drag || ev.pointerId !== drag.pointerId) return;
        const delta = (drag.axis === 'x' ? ev.clientX : ev.clientY) - drag.startCoord;
        const signedDelta = drag.invert ? -delta : delta;
        const resolved = resolve(drag.startSize + signedDelta);
        dragPendingRef.current = resolved;
        if (drag.onLiveResize) {
          // Size rides the live channel (rAF-coalesced below); a collapse
          // transition commits immediately so dependent markup swaps now.
          const collapsedChanged = commitCollapsed(resolved.collapsed);
          if (collapsedChanged) {
            sizeInteracted.current = true;
            collapsedInteracted.current = true;
            setSize(resolved.size);
          }
        } else {
          // Legacy path commits React state — record the interaction so the
          // persistence effects fire for the dragged value.
          sizeInteracted.current = true;
          collapsedInteracted.current = true;
        }
        if (dragRafRef.current === null) {
          dragRafRef.current = requestAnimationFrame(flushFrame);
        }
      };

      const onUp = (ev: PointerEvent) => {
        const drag = dragRef.current;
        if (!drag || ev.pointerId !== drag.pointerId) return;
        if (dragRafRef.current !== null) {
          cancelAnimationFrame(dragRafRef.current);
          dragRafRef.current = null;
        }
        // Commit the release position to React state synchronously — this is the
        // single state write for an `onLiveResize` drag (syncing the live DOM
        // channel back to the source of truth) and the final write for the
        // legacy path. Synchronous so callers can read the settled size right
        // after pointer-up.
        const pending = dragPendingRef.current;
        dragPendingRef.current = null;
        if (pending) {
          const collapsedChanged = pending.collapsed !== liveCollapsed;
          if (collapsedChanged) {
            collapsedInteracted.current = true;
            commitCollapsed(pending.collapsed);
          }
          sizeInteracted.current = true;
          setSize(pending.size);
        }
        try {
          target.releasePointerCapture(drag.pointerId);
        } catch {
          // ignore
        }
        dragRef.current = null;
        document.body.style.userSelect = '';
        document.body.style.cursor = '';
        target.removeEventListener('pointermove', onMove);
        target.removeEventListener('pointerup', onUp);
        target.removeEventListener('pointercancel', onUp);
      };

      target.addEventListener('pointermove', onMove);
      target.addEventListener('pointerup', onUp);
      target.addEventListener('pointercancel', onUp);
    },
    [size, collapsed, clamp, collapseThreshold, min],
  );

  const setCollapsed = useCallback((value: boolean) => {
    collapsedInteracted.current = true;
    setCollapsedState(value);
    if (!value) {
      // Restoring: ensure size is at least defaultSize so expand looks sensible.
      sizeInteracted.current = true;
      setSize((s) => (s < defaultSize ? defaultSize : s));
    }
  }, [defaultSize]);

  const expandFromCollapsed = useCallback(() => {
    collapsedInteracted.current = true;
    sizeInteracted.current = true;
    setCollapsedState(false);
    setSize((s) => (s < defaultSize ? defaultSize : s));
  }, [defaultSize]);

  const setSizeClamped = useCallback(
    (px: number) => {
      sizeInteracted.current = true;
      collapsedInteracted.current = true;
      setSize(clamp(px));
      setCollapsedState(false);
    },
    [clamp],
  );

  return { size, collapsed, startDrag, setCollapsed, expandFromCollapsed, setSize: setSizeClamped };
}
