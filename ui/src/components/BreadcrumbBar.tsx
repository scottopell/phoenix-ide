import { useEffect, useRef, useState } from 'react';
import type { Breadcrumb } from '../types';

interface BreadcrumbBarProps {
  breadcrumbs: Breadcrumb[];
  visible: boolean;
}

export function BreadcrumbBar({ breadcrumbs, visible }: BreadcrumbBarProps) {
  const barRef = useRef<HTMLElement>(null);
  const [hoveredIndex, setHoveredIndex] = useState<number | null>(null);
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

  const handleMouseEnter = (index: number) => {
    // Clear any pending hide
    if (hoverTimeoutRef.current) {
      clearTimeout(hoverTimeoutRef.current);
      hoverTimeoutRef.current = null;
    }
    // Show after 150ms delay
    hoverTimeoutRef.current = window.setTimeout(() => {
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

          const showTooltip = hoveredIndex === i && b.preview;

          return (
            <span key={`${b.type}-${i}-${b.toolId || ''}`}>
              <span
                className={classes}
                data-index={i}
                onClick={() => handleClick(b)}
                onMouseEnter={() => handleMouseEnter(i)}
                onMouseLeave={handleMouseLeave}
              >
                {b.label}
                {showTooltip && (
                  <span className="breadcrumb-tooltip">
                    <strong>{b.label}</strong>
                    <span className="breadcrumb-tooltip-preview">{b.preview}</span>
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
