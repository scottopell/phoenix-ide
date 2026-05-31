import { render, screen, fireEvent, within, act } from '@testing-library/react';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { DiffView } from './DiffView';
import { ReviewNotesProvider } from '../../contexts/ReviewNotesContext';
import { codeViewMockState, itemVersion, resetCodeViewMock } from './__testutils__/codeViewMock';

vi.mock('@pierre/diffs/react', async () => {
  const { makeCodeViewMock } = await import('./__testutils__/codeViewMock');
  return makeCodeViewMock();
});

const COMMITTED = [
  'diff --git a/foo.txt b/foo.txt',
  'index 0000000..1111111 100644',
  '--- a/foo.txt',
  '+++ b/foo.txt',
  '@@ -0,0 +1,1 @@',
  '+hello world',
].join('\n');

function renderDiff(committed = COMMITTED, uncommitted = '') {
  return render(
    <ReviewNotesProvider>
      <DiffView
        open
        comparator="origin/main"
        commitLog=""
        committedDiff={committed}
        uncommittedDiff={uncommitted}
        onClose={() => undefined}
        onSendNotes={() => undefined}
      />
    </ReviewNotesProvider>,
  );
}

function addNoteViaGutter(body: string) {
  fireEvent.click(screen.getByRole('button', { name: 'Add note to line' }));
  expect(screen.getByText('foo.txt:1')).toBeInTheDocument();
  fireEvent.change(screen.getByPlaceholderText(/Add your note/), { target: { value: body } });
  fireEvent.click(screen.getByRole('button', { name: 'Add Note' }));
}

describe('DiffView (Pierre CodeView wiring)', () => {
  beforeEach(() => resetCodeViewMock());

  it('renders the section summary, file header badge, and the parsed file', () => {
    renderDiff();
    expect(screen.getByText(/Committed changes \(vs origin\/main\)/)).toBeInTheDocument();
    // File name comes from the parsed Pierre item, surfaced by the CodeView mock.
    expect(screen.getByText('foo.txt')).toBeInTheDocument();
    // The committed section badge appears both in the summary bar and the file
    // header — at least one is present.
    expect(screen.getAllByText('committed').length).toBeGreaterThan(0);
  });

  it('adds a line note via the gutter affordance and surfaces it in the badge, panel, and inline annotation', () => {
    renderDiff();
    addNoteViaGutter('looks good');

    const badge = screen.getByRole('button', { name: '1 notes' });
    expect(badge).toBeInTheDocument();

    // Inline annotation (renderAnnotation) shows the body next to the line.
    expect(screen.getByRole('note')).toHaveTextContent('looks good');

    // Panel also lists it.
    fireEvent.click(badge);
    expect(within(screen.getByText(/^Notes \(/).closest('.notes-panel')!).getByText('looks good')).toBeInTheDocument();
  });

  it('bumps the CodeView item version when a note is added (controlled reconcile)', () => {
    renderDiff();
    const before = itemVersion('committed:foo.txt');
    addNoteViaGutter('versioned');
    const after = itemVersion('committed:foo.txt');
    expect(typeof after).toBe('number');
    expect(after!).toBeGreaterThan(before ?? 0);
  });

  it('adds a line note via line click (onLineClick)', () => {
    renderDiff();
    fireEvent.click(screen.getByTestId('mock-line-click-committed:foo.txt'));
    expect(screen.getByText('foo.txt:1')).toBeInTheDocument();
    fireEvent.change(screen.getByPlaceholderText(/Add your note/), { target: { value: 'via click' } });
    fireEvent.click(screen.getByRole('button', { name: 'Add Note' }));
    expect(screen.getByRole('button', { name: '1 notes' })).toBeInTheDocument();
  });

  it('adds a line note via a 500ms touch long-press (cancels on movement)', () => {
    vi.useFakeTimers();
    try {
      renderDiff();
      const container = screen.getByTestId('codeview-mock');
      // Pierre reports the hovered line; then a held touch pointer fires.
      fireEvent.click(screen.getByTestId('mock-line-enter-committed:foo.txt'));

      const press = (type: string, x = 0, y = 0) => {
        const ev = new Event(type, { bubbles: true });
        Object.assign(ev, { pointerType: 'touch', clientX: x, clientY: y });
        act(() => {
          container.dispatchEvent(ev);
        });
      };

      // Movement before the threshold cancels the press → no dialog.
      press('pointerdown');
      press('pointermove', 50, 50);
      act(() => vi.advanceTimersByTime(600));
      expect(screen.queryByText('foo.txt:1')).not.toBeInTheDocument();

      // A still 500ms hold opens the dialog anchored at the hovered line.
      press('pointerdown');
      act(() => vi.advanceTimersByTime(500));
      expect(screen.getByText('foo.txt:1')).toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
  });

  it('does not annotate on a plain touch tap (onLineClick ignores touch)', () => {
    renderDiff();
    fireEvent.click(screen.getByTestId('mock-line-tap-committed:foo.txt'));
    expect(screen.queryByText('foo.txt:1')).not.toBeInTheDocument();
  });

  it('adds a file-level note from the header metadata affordance', () => {
    renderDiff();
    fireEvent.click(screen.getByRole('button', { name: 'Add file-level note to foo.txt' }));
    expect(screen.getByText('foo.txt (file-level)')).toBeInTheDocument();
    fireEvent.change(screen.getByPlaceholderText(/Add your note/), { target: { value: 'whole file' } });
    fireEvent.click(screen.getByRole('button', { name: 'Add Note' }));
    expect(screen.getByRole('button', { name: '1 notes' })).toBeInTheDocument();
  });

  it('jumps to a note via the typed scroll target (no DOM scraping)', () => {
    renderDiff();
    addNoteViaGutter('jump me');
    fireEvent.click(screen.getByRole('button', { name: '1 notes' }));
    // Click the note's anchor in the panel.
    fireEvent.click(screen.getByRole('button', { name: 'New line 1' }));
    expect(codeViewMockState.scrollToCalls).toEqual([
      { type: 'line', id: 'committed:foo.txt', lineNumber: 1, side: 'additions', align: 'center', behavior: 'smooth' },
    ]);
  });

  it('shows the empty state when there are no changes', () => {
    renderDiff('', '');
    expect(screen.getByText(/No changes vs/)).toBeInTheDocument();
  });

  it('renders the truncation indicator with a ≥ prefix when saturated', () => {
    render(
      <ReviewNotesProvider>
        <DiffView
          open
          comparator="origin/main"
          commitLog=""
          committedDiff={COMMITTED}
          committedTruncatedKib={5}
          committedSaturated
          uncommittedDiff=""
          onClose={() => undefined}
          onSendNotes={() => undefined}
        />
      </ReviewNotesProvider>,
    );
    expect(screen.getByText(/truncated; ≥5 KiB total/)).toBeInTheDocument();
  });

  it('shows a section-scoped parse error for a malformed diff instead of crashing', () => {
    renderDiff('this is not a diff at all', '');
    expect(screen.getByRole('alert')).toHaveTextContent(/could not be parsed/i);
  });

  it('keeps the same path in committed and uncommitted as distinct items', () => {
    const { container } = renderDiff(COMMITTED, COMMITTED);
    expect(container.querySelector('[data-item-id="committed:foo.txt"]')).toBeTruthy();
    expect(container.querySelector('[data-item-id="uncommitted:foo.txt"]')).toBeTruthy();
  });
});
