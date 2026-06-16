import { useEffect, useMemo, useRef } from 'react';

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
}

export function PillStrip({
  items,
  autoScrollToEnd = false,
  navId,
  trailId,
  pillClassName = 'pill-item',
  activeClassName = 'active',
  arrowClassName = 'pill-arrow',
}: PillStripProps) {
  const barRef = useRef<HTMLElement>(null);

  const activeItemKey = useMemo(() => items.find((item) => item.active)?.key ?? null, [items]);

  useEffect(() => {
    if (autoScrollToEnd && barRef.current) {
      barRef.current.scrollLeft = barRef.current.scrollWidth;
    }
  }, [items, autoScrollToEnd]);

  useEffect(() => {
    if (!barRef.current || activeItemKey === null) {
      return;
    }

    const bar = barRef.current;
    const activePill = bar.querySelector<HTMLElement>('[data-active="true"]');
    if (!activePill) {
      return;
    }

    const visibleLeft = bar.scrollLeft;
    const visibleRight = visibleLeft + bar.clientWidth;
    const pillLeft = activePill.offsetLeft;
    const pillRight = pillLeft + activePill.offsetWidth;

    if (pillLeft < visibleLeft) {
      bar.scrollLeft = pillLeft;
    } else if (pillRight > visibleRight) {
      bar.scrollLeft = pillRight - bar.clientWidth;
    }
  }, [activeItemKey]);


  return (
    <nav id={navId} ref={barRef}>
        <div id={trailId}>
          {items.map((item, i) => {
            const isLast = i === items.length - 1;
            const classes = [
              pillClassName,
              item.active ? activeClassName : '',
              item.className ?? '',
            ].filter(Boolean).join(' ');

            // A pill with an `onClick` is an interactive control, so it must
            // be keyboard-operable (focusable + Enter/Space), not just
            // clickable — these are navigation/expand affordances. Pills
            // without `onClick` stay inert, non-focusable text.
            const onClick = item.onClick;

            return (
              <span key={item.key}>
                <span
                  className={classes}
                  data-index={i}
                  data-active={item.active ? 'true' : undefined}
                  role={onClick ? 'button' : undefined}
                  tabIndex={onClick ? 0 : undefined}
                  onClick={onClick}
                  onKeyDown={
                    onClick
                      ? (e) => {
                          if (e.key === 'Enter' || e.key === ' ') {
                            e.preventDefault();
                            onClick();
                          }
                        }
                      : undefined
                  }
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
  );
}
