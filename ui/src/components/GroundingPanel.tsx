import { useEffect, useRef, type ReactNode } from 'react';
import './GroundingPanel.css';

interface SectionProps {
  icon: string;
  title: string;
  summary?: ReactNode;
  /** Headline count for the section. A count of 0 renders no pill — empty
   *  sections are presented identically across the panel (the summary label
   *  carries the "nothing here" state). Typed as a number, not ReactNode, so
   *  "0" can be suppressed structurally rather than per-caller. */
  count?: number;
  expanded: boolean;
  attention?: boolean;
  action?: ReactNode;
  children?: ReactNode;
  scrollTop?: number | undefined;
  onScrollTopChange?: ((scrollTop: number) => void) | undefined;
  onToggle: () => void;
}

export function GroundingSection({
  icon,
  title,
  summary,
  count,
  expanded,
  attention = false,
  action,
  children,
  scrollTop,
  onScrollTopChange,
  onToggle,
}: SectionProps) {
  const bodyRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    const body = bodyRef.current;
    if (body && scrollTop !== undefined) body.scrollTop = scrollTop;
  }, [expanded, children, scrollTop]);

  const handleBodyScroll = (event: React.UIEvent<HTMLDivElement>) => {
    onScrollTopChange?.(event.currentTarget.scrollTop);
  };

  return (
    <section className={`grounding-section${expanded ? ' is-expanded' : ''}${attention ? ' has-attention' : ''}${action ? ' has-action' : ''}`}>
      <button
        type="button"
        className="grounding-section-header"
        onClick={onToggle}
        aria-expanded={expanded}
      >
        <span className={`grounding-chevron${expanded ? ' expanded' : ''}`} aria-hidden="true">&#9654;</span>
        <span className="grounding-icon" aria-hidden="true">{icon}</span>
        <span className="grounding-title">{title}</span>
        {summary && <span className="grounding-summary">{summary}</span>}
        {count ? <span className="grounding-count">{count}</span> : null}
      </button>
      {action && <div className="grounding-header-action">{action}</div>}
      {expanded && (
        <div
          className="grounding-section-body"
          ref={bodyRef}
          onScroll={handleBodyScroll}
        >
          {children}
        </div>
      )}
    </section>
  );
}

interface StateProps {
  children: ReactNode;
  tone?: 'muted' | 'attention' | 'error' | 'loading';
}

export function GroundingState({ children, tone = 'muted' }: StateProps) {
  return <div className={`grounding-state grounding-state--${tone}`}>{children}</div>;
}
