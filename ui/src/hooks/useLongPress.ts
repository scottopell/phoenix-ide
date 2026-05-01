import { useCallback, useRef } from 'react';

/**
 * Long-press gesture hook — invokes `onLongPress` when the user holds a
 * pointer/touch on the bound element for `thresholdMs` without moving more
 * than `movementThresholdPx` in any direction. Intended for "open
 * annotation dialog on this line" interactions in the viewer components.
 *
 * Returns event handlers to spread onto a target element. The handlers
 * accept an arbitrary payload of generic type `T` so callers can carry
 * line-level context (line number + text, diff position, etc.) without
 * the hook needing to know the shape.
 *
 * Extracted from ProseReader (REQ-PF-006).
 */
export function useLongPress<T>(
  onLongPress: (payload: T, event: React.TouchEvent | React.MouseEvent) => void,
  thresholdMs = 500,
  movementThresholdPx = 10,
) {
  const timerRef = useRef<number | null>(null);
  const startPosRef = useRef<{ x: number; y: number } | null>(null);

  const cancel = useCallback(() => {
    if (timerRef.current) {
      window.clearTimeout(timerRef.current);
      timerRef.current = null;
    }
    startPosRef.current = null;
  }, []);

  const start = useCallback(
    (e: React.TouchEvent | React.MouseEvent, payload: T) => {
      const touch = 'touches' in e ? e.touches[0] : undefined;
      const pos = touch
        ? { x: touch.clientX, y: touch.clientY }
        : { x: (e as React.MouseEvent).clientX, y: (e as React.MouseEvent).clientY };
      startPosRef.current = pos;

      timerRef.current = window.setTimeout(() => {
        if ('vibrate' in navigator) navigator.vibrate(50);
        onLongPress(payload, e);
        cancel();
      }, thresholdMs);
    },
    [onLongPress, thresholdMs, cancel],
  );

  const move = useCallback(
    (e: React.TouchEvent | React.MouseEvent) => {
      if (!startPosRef.current) return;
      const touch = 'touches' in e ? e.touches[0] : undefined;
      const pos = touch
        ? { x: touch.clientX, y: touch.clientY }
        : { x: (e as React.MouseEvent).clientX, y: (e as React.MouseEvent).clientY };
      const dx = Math.abs(pos.x - startPosRef.current.x);
      const dy = Math.abs(pos.y - startPosRef.current.y);
      if (dx > movementThresholdPx || dy > movementThresholdPx) cancel();
    },
    [movementThresholdPx, cancel],
  );

  return { start, move, end: cancel };
}
