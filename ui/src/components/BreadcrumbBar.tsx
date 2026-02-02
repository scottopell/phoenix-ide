import { useEffect, useRef } from 'react';
import type { Breadcrumb } from '../types';

interface BreadcrumbBarProps {
  breadcrumbs: Breadcrumb[];
  visible: boolean;
}

export function BreadcrumbBar({ breadcrumbs, visible }: BreadcrumbBarProps) {
  const barRef = useRef<HTMLElement>(null);

  // Auto-scroll to end when breadcrumbs change
  useEffect(() => {
    if (barRef.current) {
      barRef.current.scrollLeft = barRef.current.scrollWidth;
    }
  }, [breadcrumbs]);

  if (!visible || breadcrumbs.length === 0) {
    return (
      <nav id="breadcrumb-bar" ref={barRef}>
        <div id="breadcrumb-trail"></div>
      </nav>
    );
  }

  return (
    <nav id="breadcrumb-bar" ref={barRef}>
      <div id="breadcrumb-trail">
        {breadcrumbs.map((b, i) => {
          const isLast = i === breadcrumbs.length - 1;
          const classes = [
            'breadcrumb-item',
            isLast ? 'active' : '',
            b.type === 'tool' ? 'tool' : '',
          ].filter(Boolean).join(' ');

          return (
            <span key={`${b.type}-${i}-${b.toolId || ''}`}>
              <span className={classes} data-index={i}>
                {b.label}
              </span>
              {!isLast && <span className="breadcrumb-arrow">→</span>}
            </span>
          );
        })}
      </div>
    </nav>
  );
}
