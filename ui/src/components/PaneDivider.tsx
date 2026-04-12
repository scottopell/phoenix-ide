import { useState, useCallback } from 'react';

interface PaneDividerProps {
  orientation: 'vertical' | 'horizontal';
  /** Pointer-down handler — typically `(e) => startDrag(e, axis, invert)` from useResizablePane */
  onPointerDown: (e: React.PointerEvent) => void;
  onDoubleClick?: () => void;
  /** Native tooltip text shown on hover */
  title?: string;
}

/**
 * Drag handle between two flex children.
 *
 * Vertical orientation = a vertical line between two side-by-side panes (col-resize).
 * Horizontal orientation = a horizontal line between two stacked panes (row-resize).
 */
export function PaneDivider({ orientation, onPointerDown, onDoubleClick, title }: PaneDividerProps) {
  const [dragging, setDragging] = useState(false);

  const handlePointerDown = useCallback(
    (e: React.PointerEvent) => {
      setDragging(true);
      onPointerDown(e);

      const stop = () => {
        setDragging(false);
        window.removeEventListener('pointerup', stop);
        window.removeEventListener('pointercancel', stop);
      };
      window.addEventListener('pointerup', stop);
      window.addEventListener('pointercancel', stop);
    },
    [onPointerDown],
  );

  const cls = [
    'pane-divider',
    `pane-divider--${orientation}`,
    dragging ? 'pane-divider--dragging' : '',
  ]
    .filter(Boolean)
    .join(' ');

  return (
    <div
      className={cls}
      onPointerDown={handlePointerDown}
      onDoubleClick={onDoubleClick}
      role="separator"
      aria-orientation={orientation}
      title={title}
    >
      <span className="pane-divider-grip" aria-hidden="true" />
    </div>
  );
}
