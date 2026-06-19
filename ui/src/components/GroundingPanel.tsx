import type { ReactNode } from 'react';
import './GroundingPanel.css';

interface SectionProps {
  icon: string;
  title: string;
  summary?: ReactNode;
  count?: ReactNode;
  expanded: boolean;
  attention?: boolean;
  action?: ReactNode;
  children?: ReactNode;
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
  onToggle,
}: SectionProps) {
  return (
    <section className={`grounding-section${expanded ? ' is-expanded' : ''}${attention ? ' has-attention' : ''}`}>
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
        {count != null && <span className="grounding-count">{count}</span>}
      </button>
      {action && <div className="grounding-header-action">{action}</div>}
      {expanded && <div className="grounding-section-body">{children}</div>}
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
