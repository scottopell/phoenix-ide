import { useState, useCallback, useRef, useEffect } from 'react';

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
  /** Pointer-down handler to wire to PaneDivider */
  startDrag: (e: React.PointerEvent, axis: 'x' | 'y', invert?: boolean) => void;
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
  } | null>(null);

  const startDrag = useCallback(
    (e: React.PointerEvent, axis: 'x' | 'y', invert = false) => {
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
      };
      document.body.style.userSelect = 'none';
      document.body.style.cursor = axis === 'x' ? 'col-resize' : 'row-resize';

      const onMove = (ev: PointerEvent) => {
        const drag = dragRef.current;
        if (!drag || ev.pointerId !== drag.pointerId) return;
        const delta = (drag.axis === 'x' ? ev.clientX : ev.clientY) - drag.startCoord;
        const signedDelta = drag.invert ? -delta : delta;
        const proposed = drag.startSize + signedDelta;

        sizeInteracted.current = true;
        collapsedInteracted.current = true;
        if (collapseThreshold !== undefined && proposed < collapseThreshold) {
          setCollapsedState(true);
          // Keep last good size at min so expand restores to a sensible value.
          setSize(clamp(min));
        } else {
          setCollapsedState(false);
          setSize(clamp(proposed));
        }
      };

      const onUp = (ev: PointerEvent) => {
        const drag = dragRef.current;
        if (!drag || ev.pointerId !== drag.pointerId) return;
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
