import { createRef } from 'react';
import { render, screen, fireEvent } from '@testing-library/react';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { PhoenixFileCodeView } from './PhoenixFileCodeView';
import type { PhoenixFileCodeViewHandle } from './PhoenixFileCodeView';
import type { ReviewNote } from '../../contexts/ReviewNotesContext';
import { codeViewMockState, resetCodeViewMock } from './__testutils__/codeViewMock';

vi.mock('@pierre/diffs/react', async () => {
  const { makeCodeViewMock } = await import('./__testutils__/codeViewMock');
  return makeCodeViewMock();
});

const PATH = '/tmp/project/main.rs';
const ITEM_ID = `file:${PATH}`;

function renderView(overrides: Partial<React.ComponentProps<typeof PhoenixFileCodeView>> = {}) {
  const onAnnotateLine = vi.fn();
  const ref = createRef<PhoenixFileCodeViewHandle>();
  render(
    <PhoenixFileCodeView
      ref={ref}
      filePath={PATH}
      content={'line one\nline two'}
      notes={[]}
      modifiedLines={new Set()}
      highlightedLine={null}
      scrollKey={`scroll:${PATH}`}
      onAnnotateLine={onAnnotateLine}
      {...overrides}
    />,
  );
  return { onAnnotateLine, ref };
}

describe('PhoenixFileCodeView (Pierre file wiring)', () => {
  beforeEach(() => resetCodeViewMock());

  it('renders the file through Pierre CodeView', () => {
    renderView();
    expect(screen.getByTestId('codeview-mock')).toBeInTheDocument();
    expect(screen.getByText(PATH)).toBeInTheDocument();
  });

  it('annotates the clicked line with its quoted source text (mouse)', () => {
    const { onAnnotateLine } = renderView();
    fireEvent.click(screen.getByTestId(`mock-line-click-${ITEM_ID}`));
    expect(onAnnotateLine).toHaveBeenCalledWith(1, 'line one');
  });

  it('does not annotate on a touch tap (long-press owns touch)', () => {
    const { onAnnotateLine } = renderView();
    fireEvent.click(screen.getByTestId(`mock-line-tap-${ITEM_ID}`));
    expect(onAnnotateLine).not.toHaveBeenCalled();
  });

  it('annotates the hovered line via the gutter add-note affordance', () => {
    const { onAnnotateLine } = renderView();
    fireEvent.click(screen.getByRole('button', { name: 'Add note to line' }));
    expect(onAnnotateLine).toHaveBeenCalledWith(1, 'line one');
  });

  it('renders an inline note for a file annotation', () => {
    const note: ReviewNote = { id: 'n1', anchor: { kind: 'file', filePath: PATH, lineNumber: 1 }, body: 'looks good', lineContent: '', createdAt: 0 };
    renderView({ notes: [note] });
    expect(screen.getByText('looks good')).toBeInTheDocument();
  });

  it('jumps to a line through the typed scroll handle', () => {
    const { ref } = renderView();
    ref.current?.scrollToLine(2);
    expect(codeViewMockState.scrollToCalls).toContainEqual(
      expect.objectContaining({ type: 'line', id: ITEM_ID, lineNumber: 2 }),
    );
  });

  it('scrolls find navigation through the typed file target and decorates matched lines', () => {
    const { ref } = renderView({
      findMatches: [
        { kind: 'file-line', lineNumber: 1, startColumn: 0, endColumn: 4 },
        { kind: 'file-line', lineNumber: 2, startColumn: 0, endColumn: 4 },
      ],
      activeFindMatch: { kind: 'file-line', lineNumber: 2, startColumn: 0, endColumn: 4 },
    });

    ref.current?.scrollToFindTarget({ kind: 'file-line', lineNumber: 2, startColumn: 0, endColumn: 4 });

    expect(codeViewMockState.scrollToCalls).toContainEqual(
      expect.objectContaining({ type: 'line', id: ITEM_ID, lineNumber: 2 }),
    );
    expect(codeViewMockState.lastUnsafeCss).toContain('[data-line="1"]');
    expect(codeViewMockState.lastUnsafeCss).toContain('[data-line="2"]');
    expect(codeViewMockState.lastUnsafeCss).toContain('viewer-find-active-outline');
  });
});
