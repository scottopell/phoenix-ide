import { useEffect, useRef, useState } from 'react';
import type { Breadcrumb } from '../types';

const BREADCRUMB_TITLES: Record<string, string> = {
  user: 'Your message',
  llm: 'AI is thinking',
  tool: 'Running a tool',
  subagents: 'Running sub-agents in parallel',
};

interface BreadcrumbBarProps {
  breadcrumbs: Breadcrumb[];
  visible: boolean;
}

interface TooltipPosition {
  /** Left edge of the tooltip box in viewport px */
  tooltipLeft: number;
  /** Left position of the arrow within the tooltip box in px */
  arrowLeft: number;
}

const TOOLTIP_WIDTH = 280;
const TOOLTIP_MARGIN = 8; // min distance from viewport edge

function calcTooltipPosition(rect: DOMRect): TooltipPosition {
  const itemCenterX = rect.left + rect.width / 2;
  const viewportWidth = window.innerWidth;

  // Ideal: center tooltip over the item
  let tooltipLeft = itemCenterX - TOOLTIP_WIDTH / 2;

  // Clamp to viewport edges
  tooltipLeft = Math.max(TOOLTIP_MARGIN, tooltipLeft);
  tooltipLeft = Math.min(viewportWidth - TOOLTIP_WIDTH - TOOLTIP_MARGIN, tooltipLeft);

  // Arrow should always point at the item center, relative to tooltip box
  const arrowLeft = Math.max(12, Math.min(TOOLTIP_WIDTH - 12, itemCenterX - tooltipLeft));

  return { tooltipLeft, arrowLeft };
}

export function BreadcrumbBar({ breadcrumbs, visible }: BreadcrumbBarProps) {
  const barRef = useRef<HTMLElement>(null);
  const [hoveredIndex, setHoveredIndex] = useState<number | null>(null);
  const [tooltipPos, setTooltipPos] = useState<TooltipPosition | null>(null);
  const hoverTimeoutRef = useRef<number | null>(null);

  // Auto-scroll to end when breadcrumbs change
  useEffect(() => {
    if (barRef.current) {
      barRef.current.scrollLeft = barRef.current.scrollWidth;
    }
  }, [breadcrumbs]);

  // Cleanup timeout on unmount
  useEffect(() => {
    return () => {
      if (hoverTimeoutRef.current) {
        clearTimeout(hoverTimeoutRef.current);
      }
    };
  }, []);

  if (!visible || breadcrumbs.length === 0) {
    return null;
  }

  const handleClick = (b: Breadcrumb) => {
    if (b.sequenceId === undefined) return;

    const el = document.querySelector(`[data-sequence-id="${b.sequenceId}"]`);
    if (el) {
      el.scrollIntoView({ behavior: 'smooth', block: 'center' });
      // Add brief highlight
      el.classList.add('breadcrumb-highlight');
      setTimeout(() => el.classList.remove('breadcrumb-highlight'), 1500);
    }
  };

  const handleMouseEnter = (index: number, e: React.MouseEvent<HTMLSpanElement>) => {
    const target = e.currentTarget;
    // Clear any pending hide
    if (hoverTimeoutRef.current) {
      clearTimeout(hoverTimeoutRef.current);
      hoverTimeoutRef.current = null;
    }
    // Show after 150ms delay
    hoverTimeoutRef.current = window.setTimeout(() => {
      const rect = target.getBoundingClientRect();
      setTooltipPos(calcTooltipPosition(rect));
      setHoveredIndex(index);
    }, 150);
  };

  const handleMouseLeave = () => {
    // Clear pending show
    if (hoverTimeoutRef.current) {
      clearTimeout(hoverTimeoutRef.current);
      hoverTimeoutRef.current = null;
    }
    // Hide immediately
    setHoveredIndex(null);
    setTooltipPos(null);
  };

  return (
    <nav id="breadcrumb-bar" ref={barRef}>
      <div id="breadcrumb-trail">
        {breadcrumbs.map((b, i) => {
          const isLast = i === breadcrumbs.length - 1;
          const classes = [
            'breadcrumb-item',
            isLast ? 'active' : '',
            b.type === 'tool' ? 'tool' : '',
            b.type === 'subagents' ? 'subagents' : '',
          ].filter(Boolean).join(' ');

          const tooltipText = b.resultSummary ?? b.preview;
          const showTooltip = hoveredIndex === i && !!tooltipText && tooltipPos !== null;

          return (
            <span key={`${b.type}-${i}-${b.toolId || ''}`}>
              <span
                className={classes}
                data-index={i}
                onClick={() => handleClick(b)}
                onMouseEnter={(e) => handleMouseEnter(i, e)}
                onMouseLeave={handleMouseLeave}
                title={BREADCRUMB_TITLES[b.type] || b.label}
              >
                {b.label.replace(/^LLM/, 'AI')}
                {showTooltip && (
                  <span
                    className="breadcrumb-tooltip"
                    style={{ left: tooltipPos!.tooltipLeft, transform: 'none' }}
                  >
                    <strong>{b.label.replace(/^LLM/, 'AI')}</strong>
                    <span className="breadcrumb-tooltip-preview">{tooltipText}</span>
                    <span
                      className="breadcrumb-tooltip-arrow"
                      style={{ left: tooltipPos!.arrowLeft }}
                    />
                  </span>
                )}
              </span>
              {!isLast && <span className="breadcrumb-arrow">→</span>}
            </span>
          );
        })}
      </div>
    </nav>
  );
}
