import { useEffect, useRef, useState } from 'react';
import type { MouseEvent } from 'react';
import { createPortal } from 'react-dom';
import { calcTooltipPosition } from './breadcrumbTooltipPosition';

export interface PillItem {
  /** Unique key for React reconciliation. */
  key: string;
  /** Visible text of the pill. */
  label: string;
  /** Extra CSS class names (e.g. variant/color classes), space-joined onto the base pill class. */
  className?: string | undefined;
  /** When true, the `active` class is applied. */
  active?: boolean | undefined;
  /** Accessible label for the pill element. */
  ariaLabel?: string | undefined;
  /** Optional hover tooltip. When present, hovering shows a portal tooltip after a delay. */
  tooltip?: { title: string; body: string } | undefined;
  /** Click handler for the pill. */
  onClick?: (() => void) | undefined;
}

interface PillStripProps {
  items: PillItem[];
  /** Auto-scroll the strip to its end whenever `items` changes. */
  autoScrollToEnd?: boolean;
  /** id applied to the scrolling <nav> wrapper. */
  navId?: string;
  /** id applied to the inner flex container. */
  trailId?: string;
  /** Base class applied to every pill element. */
  pillClassName?: string;
  /** Class applied to the active pill (in addition to `pillClassName`). */
  activeClassName?: string;
  /** Class for the `→` arrow separators. */
  arrowClassName?: string;
  /** Class for the portal tooltip box. */
  tooltipClassName?: string;
}

const HOVER_DELAY_MS = 150;

export function PillStrip({
  items,
  autoScrollToEnd = false,
  navId,
  trailId,
  pillClassName = 'pill-item',
  activeClassName = 'active',
  arrowClassName = 'pill-arrow',
  tooltipClassName = 'pill-tooltip',
}: PillStripProps) {
  const barRef = useRef<HTMLElement>(null);
  const [hoveredIndex, setHoveredIndex] = useState<number | null>(null);
  const [tooltipPos, setTooltipPos] = useState<ReturnType<typeof calcTooltipPosition> | null>(null);
  const hoverTimeoutRef = useRef<number | null>(null);

  useEffect(() => {
    if (autoScrollToEnd && barRef.current) {
      barRef.current.scrollLeft = barRef.current.scrollWidth;
    }
  }, [items, autoScrollToEnd]);

  useEffect(() => {
    return () => {
      if (hoverTimeoutRef.current) {
        clearTimeout(hoverTimeoutRef.current);
      }
    };
  }, []);

  const handleMouseEnter = (index: number, e: MouseEvent<HTMLSpanElement>) => {
    const target = e.currentTarget;
    if (hoverTimeoutRef.current) {
      clearTimeout(hoverTimeoutRef.current);
      hoverTimeoutRef.current = null;
    }
    hoverTimeoutRef.current = window.setTimeout(() => {
      const rect = target.getBoundingClientRect();
      setTooltipPos(calcTooltipPosition(rect));
      setHoveredIndex(index);
    }, HOVER_DELAY_MS);
  };

  const handleMouseLeave = () => {
    if (hoverTimeoutRef.current) {
      clearTimeout(hoverTimeoutRef.current);
      hoverTimeoutRef.current = null;
    }
    setHoveredIndex(null);
    setTooltipPos(null);
  };

  const hoveredItem = hoveredIndex !== null ? items[hoveredIndex] : null;
  const hoveredTooltip = hoveredItem?.tooltip;
  const tooltip = hoveredTooltip && tooltipPos !== null && typeof document !== 'undefined'
    ? createPortal(
        <span
          className={tooltipClassName}
          style={{
            left: tooltipPos.tooltipLeft,
            top: tooltipPos.tooltipTop,
            transform: 'translateY(-100%)',
          }}
        >
          <strong>{hoveredTooltip.title}</strong>
          <span className={`${tooltipClassName}-preview`}>{hoveredTooltip.body}</span>
          <span
            className={`${tooltipClassName}-arrow`}
            style={{ left: tooltipPos.arrowLeft }}
          />
        </span>,
        document.body,
      )
    : null;

  return (
    <>
      <nav id={navId} ref={barRef}>
        <div id={trailId}>
          {items.map((item, i) => {
            const isLast = i === items.length - 1;
            const classes = [
              pillClassName,
              item.active ? activeClassName : '',
              item.className ?? '',
            ].filter(Boolean).join(' ');

            return (
              <span key={item.key}>
                <span
                  className={classes}
                  data-index={i}
                  onClick={item.onClick}
                  onMouseEnter={item.tooltip ? (e) => handleMouseEnter(i, e) : undefined}
                  onMouseLeave={item.tooltip ? handleMouseLeave : undefined}
                  aria-label={item.ariaLabel}
                >
                  {item.label}
                </span>
                {!isLast && <span className={arrowClassName}>→</span>}
              </span>
            );
          })}
        </div>
      </nav>
      {tooltip}
    </>
  );
}
