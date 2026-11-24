import { cleanup, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { ReactionSource } from '../conversation/InlineReactionStore';
import { useAnnotationHighlights } from './useAnnotationHighlights';

const source: ReactionSource = { messageId: 'answer', sequenceId: 2, occurrenceToken: 'earlier:answer', quote: 'second', textAnchor: { start: { fragmentId: 'text-0', offset: 6 }, end: { fragmentId: 'text-0', offset: 12 } } };

class FakeHighlight {
  readonly ranges = new Set<Range>();
  add(range: Range) { this.ranges.add(range); }
  clear() { this.ranges.clear(); }
}
let registry: Map<string, FakeHighlight>;

function Host({ mounted, transcriptKey = 'transcript', churn = false }: { mounted: boolean; transcriptKey?: string; churn?: boolean }) {
  useAnnotationHighlights(source, undefined);
  return <div key={transcriptKey} id="messages">
    {churn && <div data-testid="churn" />}
    {mounted && <div data-inline-reaction-message="answer" data-message-occurrence="earlier:answer"><div className="agent-text-block" data-fragment-id="text-0">first <strong>second</strong> third</div></div>}
  </div>;
}

const highlighted = () => [...(registry.get('phoenix-pending-annotations')?.ranges ?? [])].map(range => range.toString());

beforeEach(() => {
  registry = new Map();
  vi.stubGlobal('CSS', { highlights: registry });
  vi.stubGlobal('Highlight', FakeHighlight);
});
afterEach(() => { cleanup(); vi.unstubAllGlobals(); });

describe('annotation highlights', () => {
  it('paints a source that mounts after the transcript element is replaced', async () => {
    const view = render(<Host mounted={false} />);
    expect(highlighted()).toEqual([]);
    view.rerender(<Host mounted={false} transcriptKey="remounted" />);
    view.rerender(<Host mounted transcriptKey="remounted" />);
    await waitFor(() => expect(highlighted()).toEqual(['second']));
  });

  it('paints while the transcript keeps changing faster than a frame', async () => {
    vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => window.setTimeout(() => callback(performance.now()), 16));
    vi.stubGlobal('cancelAnimationFrame', (handle: number) => window.clearTimeout(handle));
    const view = render(<Host mounted={false} churn />);
    const churn = screen.getByTestId('churn');
    const interval = window.setInterval(() => { churn.replaceChildren(document.createElement('span')); }, 1);
    try {
      view.rerender(<Host mounted churn />);
      await waitFor(() => expect(highlighted()).toEqual(['second']));
    } finally {
      window.clearInterval(interval);
    }
  });
});
