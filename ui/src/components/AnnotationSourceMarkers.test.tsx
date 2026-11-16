import { cleanup, render, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { AnnotationDraftStore } from '../conversation/AnnotationDraftStore';
import { AnnotationSourceMarkers } from './AnnotationSourceMarkers';
import { restoreReactionRange } from './reactionRange';

vi.mock('./reactionRange', () => ({ restoreReactionRange: vi.fn() }));

class FakeHighlight {
  priority = 0;
  ranges = new Set<Range>();
  add(range: Range) { this.ranges.add(range); }
  clear() { this.ranges.clear(); }
}
let registry: Map<string, FakeHighlight>;
let range: Range;
const draft = () => {
  const draft = new AnnotationDraftStore().forScope('a');
  for (const body of ['First', 'Second']) draft.add({ source: { messageId: 'answer', sequenceId: 2, quote: 'shared passage' }, body });
  return draft;
};

beforeEach(() => {
  registry = new Map();
  vi.stubGlobal('CSS', { highlights: registry });
  vi.stubGlobal('Highlight', FakeHighlight);
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockReturnValue(new DOMRect(0, 0, 400, 500));
  range = document.createRange();
  const block = document.createElement('div');
  block.className = 'agent-text-block';
  block.textContent = 'shared passage';
  vi.spyOn(block, 'getBoundingClientRect').mockReturnValue(new DOMRect(20, 100, 360, 100));
  range.selectNodeContents(block);
  Object.defineProperty(range, 'getClientRects', { value: () => [new DOMRect(20, 100, 100, 20)] });
  vi.mocked(restoreReactionRange).mockReturnValue(range);
});
afterEach(() => { cleanup(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });

it('groups colliding source numbers and emphasizes only the chosen annotation', () => {
  const annotations = draft().getSnapshot();
  const view = render(<><div id="messages" /><AnnotationSourceMarkers annotations={annotations} emphasizedId={null} /></>);
  expect(document.querySelectorAll('.annotation-source-marker')).toHaveLength(1);
  expect(document.querySelector('.annotation-source-marker')).toHaveTextContent('1+');
  expect(document.querySelector('.annotation-source-marker')).toHaveAttribute('title', 'Annotations 1, 2');
  view.rerender(<><div id="messages" /><AnnotationSourceMarkers annotations={annotations} emphasizedId={annotations[1]!.id} /></>);
  expect(document.querySelector('.annotation-source-marker.is-emphasized')).toHaveTextContent('2+');
  expect([...registry.get('phoenix-focused-annotation')!.ranges]).toEqual([range]);
  view.rerender(<><div id="messages" /><AnnotationSourceMarkers annotations={annotations} emphasizedId={null} /></>);
  expect(registry.get('phoenix-focused-annotation')!.ranges.size).toBe(0);
});

it('keeps mid-line selections in the gutter and groups different starts on one line', () => {
  const annotations = draft().getSnapshot();
  const middle = range.cloneRange();
  Object.defineProperty(middle, 'getClientRects', { value: () => [new DOMRect(100, 100, 60, 20)] });
  vi.mocked(restoreReactionRange).mockImplementation(source => source === annotations[1]!.source ? middle : range);
  render(<><div id="messages" /><AnnotationSourceMarkers annotations={annotations} emphasizedId={annotations[1]!.id} /></>);
  const markers = document.querySelectorAll<HTMLElement>('.annotation-source-marker');
  expect(markers).toHaveLength(1);
  expect(markers[0]).toHaveTextContent('2+');
  expect(Number.parseFloat(markers[0]!.style.left) + 18).toBeLessThan(20);
});

it('removes stale markers on source unmount and restores them after transcript replacement', async () => {
  const annotations = draft().getSnapshot();
  const view = render(<><div id="messages" /><AnnotationSourceMarkers annotations={annotations} emphasizedId={null} /></>);
  expect(document.querySelector('.annotation-source-marker')).toBeInTheDocument();
  vi.mocked(restoreReactionRange).mockReturnValue(null);
  view.rerender(<><div key="replacement" id="messages" /><AnnotationSourceMarkers annotations={annotations} emphasizedId={null} /></>);
  await waitFor(() => expect(document.querySelector('.annotation-source-marker')).not.toBeInTheDocument());
  vi.mocked(restoreReactionRange).mockReturnValue(range);
  document.getElementById('messages')!.append(document.createElement('p'));
  await waitFor(() => expect(document.querySelector('.annotation-source-marker')).toBeInTheDocument());
  view.unmount();
  expect(registry.has('phoenix-focused-annotation')).toBe(false);
});
