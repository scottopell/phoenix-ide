import { useEffect, useRef, useState } from 'react';
import type { MouseEvent } from 'react';
import { createPortal } from 'react-dom';
import { calcTooltipPosition } from './breadcrumbTooltipPosition';
import type { Breadcrumb } from '../types';

const BREADCRUMB_TITLES: Record<string, string> = {
  user: 'Your message',
  llm: 'Awaiting LLM response',
  tool: 'Running a tool',
  subagents: 'Running sub-agents in parallel',
};

interface BreadcrumbBarProps {
  breadcrumbs: Breadcrumb[];
  visible: boolean;
}

export function BreadcrumbBar({ breadcrumbs, visible }: BreadcrumbBarProps) {
  const barRef = useRef<HTMLElement>(null);
  const [hoveredIndex, setHoveredIndex] = useState<number | null>(null);
  const [tooltipPos, setTooltipPos] = useState<ReturnType<typeof calcTooltipPosition> | null>(null);
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

  const handleMouseEnter = (index: number, e: MouseEvent<HTMLSpanElement>) => {
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

  const hoveredBreadcrumb = hoveredIndex !== null ? breadcrumbs[hoveredIndex] : null;
  const tooltipText = hoveredBreadcrumb?.resultSummary ?? hoveredBreadcrumb?.preview;
  const tooltip = hoveredBreadcrumb && tooltipText && tooltipPos !== null && typeof document !== 'undefined'
    ? createPortal(
        <span
          className="breadcrumb-tooltip"
          style={{
            left: tooltipPos.tooltipLeft,
            top: tooltipPos.tooltipTop,
            transform: 'translateY(-100%)',
          }}
        >
          <strong>{hoveredBreadcrumb.label.replace(/^LLM/, 'AI')}</strong>
          <span className="breadcrumb-tooltip-preview">{tooltipText}</span>
          <span
            className="breadcrumb-tooltip-arrow"
            style={{ left: tooltipPos.arrowLeft }}
          />
        </span>,
        document.body,
      )
    : null;

  return (
    <>
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

            const displayLabel = b.label.replace(/^LLM/, 'AI');
            const accessibleLabel = `${displayLabel}: ${BREADCRUMB_TITLES[b.type] || b.label}`;

            return (
              <span key={`${b.type}-${i}-${b.toolId || ''}`}>
                <span
                  className={classes}
                  data-index={i}
                  onClick={() => handleClick(b)}
                  onMouseEnter={(e) => handleMouseEnter(i, e)}
                  onMouseLeave={handleMouseLeave}
                  aria-label={accessibleLabel}
                >
                  {displayLabel}
                </span>
                {!isLast && <span className="breadcrumb-arrow">→</span>}
              </span>
            );
          })}
        </div>
      </nav>
      {tooltip}
    </>
  );
}
