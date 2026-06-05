import { useMemo } from 'react';
import { PillStrip } from './PillStrip';
import type { PillItem } from './PillStrip';
import type { Breadcrumb } from '../types';

const BREADCRUMB_TITLES: Record<string, string> = {
  user: 'Your message',
  llm: 'Awaiting LLM response',
  tool: 'Running a tool',
  subagents: 'Running sub-agents in parallel',
};

// Module-level so it's a stable reference (not a useMemo dependency). Pure:
// reads only the DOM and its argument.
function jumpToBreadcrumb(b: Breadcrumb) {
  if (b.sequenceId === undefined) return;

  const el = document.querySelector(`[data-sequence-id="${b.sequenceId}"]`);
  if (el) {
    el.scrollIntoView({ behavior: 'smooth', block: 'center' });
    // Add brief highlight
    el.classList.add('breadcrumb-highlight');
    setTimeout(() => el.classList.remove('breadcrumb-highlight'), 1500);
  }
}

interface BreadcrumbBarProps {
  breadcrumbs: Breadcrumb[];
  visible: boolean;
}

export function BreadcrumbBar({ breadcrumbs, visible }: BreadcrumbBarProps) {
  // Memoize on `breadcrumbs` so the `items` reference is stable across the
  // parent's unrelated re-renders — otherwise PillStrip's auto-scroll effect
  // (keyed on `items`) would re-scroll the bar to its end on every render,
  // yanking it back while the user is scrolled left reading an earlier pill.
  const items: PillItem[] = useMemo(
    () =>
      breadcrumbs.map((b, i) => {
        const isLast = i === breadcrumbs.length - 1;
        const displayLabel = b.label.replace(/^LLM/, 'AI');
        const tooltipText = b.resultSummary ?? b.preview;

        return {
          key: `${b.type}-${i}-${b.toolId || ''}`,
          label: displayLabel,
          active: isLast,
          className: b.type === 'tool' ? 'tool' : b.type === 'subagents' ? 'subagents' : undefined,
          ariaLabel: `${displayLabel}: ${BREADCRUMB_TITLES[b.type] || b.label}`,
          tooltip: tooltipText ? { title: displayLabel, body: tooltipText } : undefined,
          onClick: () => jumpToBreadcrumb(b),
        };
      }),
    [breadcrumbs],
  );

  if (!visible || breadcrumbs.length === 0) {
    return null;
  }

  return (
    <PillStrip
      items={items}
      autoScrollToEnd
      navId="breadcrumb-bar"
      trailId="breadcrumb-trail"
      pillClassName="breadcrumb-item"
      arrowClassName="breadcrumb-arrow"
      tooltipClassName="breadcrumb-tooltip"
    />
  );
}
