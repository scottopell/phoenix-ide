import { render, screen, fireEvent, within, act, waitFor } from '@testing-library/react';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { __diffViewFindTestables, DiffView } from './DiffView';
import * as searchProjectionModule from '../viewer-find/searchProjections';
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

const DEFAULT_CHECKOUT_STATUS: import('../../api').CheckoutStatus = {
  kind: 'named_branch',
  branch_name: 'main',
  head_oid: '1234567890abcdef1234567890abcdef12345678',
  remote_status: { kind: 'no_known' },
};

function renderDiff(committed = COMMITTED, uncommitted = '', checkoutStatus?: import('../../api').CheckoutStatus) {
  return render(
    <ReviewNotesProvider>
      <DiffView
        open
        comparator="origin/main"
        commitLog=""
        committedDiff={committed}
        uncommittedDiff={uncommitted}
        checkoutStatus={checkoutStatus ?? DEFAULT_CHECKOUT_STATUS}
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

describe('DiffView find identities', () => {
  it('bounds identities for arbitrarily long changed lines', () => {
    const text = `token ${'x'.repeat(20_000)}`;
    const id = __diffViewFindTestables.stableDiffMatchIds()({
      sourceId: 'diff-line',
      sourceText: text,
      start: 0,
      end: 5,
      target: {
        kind: 'diff-line',
        section: 'committed',
        filePath: 'src/min.js',
        itemId: 'item-1',
        side: 'additions',
        lineNumber: 1,
        startColumn: 0,
        endColumn: 5,
      },
    });

    expect(id).not.toContain(text);
    expect(id.length).toBeLessThan(128);
  });
});

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
    const addNoteButton = screen.getByRole('button', { name: 'Add note to line' });
    expect(addNoteButton).toHaveAttribute('data-utility-button');
  });

  it('reuses the memoized diff projection when a new query navigates to its first match', async () => {
    const spy = vi.spyOn(searchProjectionModule, 'buildDiffSearchProjection');
    renderDiff(COMMITTED, COMMITTED.replaceAll('foo.txt', 'bar.txt'));

    fireEvent.click(screen.getByRole('button', { name: 'Find in diff' }));
    fireEvent.change(screen.getByRole('textbox', { name: 'Find in viewer' }), { target: { value: 'hello' } });

    await waitFor(() => expect(screen.getByText('1 of 2')).toBeInTheDocument());
    expect(spy).toHaveBeenCalledTimes(1);
    spy.mockRestore();
  });

  it('lets DiffView opt out of shell-owned scrolling so CodeView owns the scroll container', () => {
    const { container } = renderDiff();
    expect(container.querySelector('.viewer-shell-body--scroll-children')).toBeTruthy();
    expect(container.querySelector('.diff-viewer-body .phoenix-diff-codeview')).toBeTruthy();
  });

  it('counts and navigates visible commit-log matches in diff find', async () => {
    render(
      <ReviewNotesProvider>
        <DiffView
          open
          checkoutStatus={DEFAULT_CHECKOUT_STATUS}
          comparator="origin/main"
          commitLog="abc123 add hello context"
          committedDiff={COMMITTED}
          uncommittedDiff=""
          onClose={() => undefined}
          onSendNotes={() => undefined}
        />
      </ReviewNotesProvider>,
    );

    fireEvent.click(screen.getByRole('button', { name: 'Find in diff' }));
    const input = screen.getByRole('textbox', { name: 'Find in viewer' });
    fireEvent.change(input, { target: { value: 'hello' } });

    expect(screen.getByText('1 of 2')).toBeInTheDocument();

    const commitLine = document.getElementById('commit-log:0');
    const scrollIntoView = vi.fn();
    if (commitLine) commitLine.scrollIntoView = scrollIntoView;

    expect(commitLine?.querySelectorAll('[data-find-occurrence]').length).toBe(1);
    expect(commitLine).toHaveClass('viewer-find-row-match', 'viewer-find-row-match--active');

    fireEvent.click(screen.getByRole('button', { name: 'Previous' }));
    expect(screen.getByText('2 of 2')).toBeInTheDocument();
    expect(commitLine).toHaveClass('viewer-find-row-match');
    expect(commitLine).not.toHaveClass('viewer-find-row-match--active');

    fireEvent.click(screen.getByRole('button', { name: 'Next' }));
    expect(screen.getByText('1 of 2')).toBeInTheDocument();
    expect(commitLine).toHaveClass('viewer-find-row-match', 'viewer-find-row-match--active');
    expect(scrollIntoView).toHaveBeenCalledWith({ block: 'center', behavior: 'smooth' });
  });

  it('wraps diff navigation from the clamped active match when results shrink', async () => {
    render(
      <ReviewNotesProvider>
        <DiffView
          open
          checkoutStatus={DEFAULT_CHECKOUT_STATUS}
          comparator="origin/main"
          commitLog=""
          committedDiff={[
            'diff --git a/foo.txt b/foo.txt',
            'index 0000000..1111111 100644',
            '--- a/foo.txt',
            '+++ b/foo.txt',
            '@@ -0,0 +1,3 @@',
            '+alpha one',
            '+alpha two',
            '+alpha three',
          ].join('\n')}
          uncommittedDiff=""
          onClose={() => undefined}
          onSendNotes={() => undefined}
        />
      </ReviewNotesProvider>,
    );

    fireEvent.click(screen.getByRole('button', { name: 'Find in diff' }));
    const input = screen.getByRole('textbox', { name: 'Find in viewer' });
    fireEvent.change(input, { target: { value: 'alpha' } });
    fireEvent.click(screen.getByRole('button', { name: 'Next' }));
    fireEvent.click(screen.getByRole('button', { name: 'Next' }));
    expect(screen.getByText('3 of 3')).toBeInTheDocument();

    fireEvent.change(input, { target: { value: 'three' } });
    expect(screen.getByText('1 of 1')).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Next' }));
    expect(screen.getByText('1 of 1')).toBeInTheDocument();
  });

  it('preserves and re-reveals the exact active diff match when insertions move it', async () => {
    const base = [
      'diff --git a/foo.txt b/foo.txt',
      'index 0000000..1111111 100644',
      '--- a/foo.txt',
      '+++ b/foo.txt',
      '@@ -0,0 +1,2 @@',
      '+alpha one',
      '+alpha two',
    ].join('\n');
    const insertedBefore = [
      'diff --git a/foo.txt b/foo.txt',
      'index 0000000..1111111 100644',
      '--- a/foo.txt',
      '+++ b/foo.txt',
      '@@ -0,0 +1,3 @@',
      '+alpha zero',
      '+alpha one',
      '+alpha two',
    ].join('\n');

    const { rerender } = render(
      <ReviewNotesProvider>
        <DiffView
          open
          comparator="origin/main"
          commitLog=""
          committedDiff={base}
          uncommittedDiff=""
          checkoutStatus={DEFAULT_CHECKOUT_STATUS}
          onClose={() => undefined}
          onSendNotes={() => undefined}
        />
      </ReviewNotesProvider>,
    );

    fireEvent.click(screen.getByRole('button', { name: 'Find in diff' }));
    fireEvent.change(screen.getByRole('textbox', { name: 'Find in viewer' }), { target: { value: 'alpha' } });
    await waitFor(() => expect(screen.getByText('1 of 2')).toBeInTheDocument());
    fireEvent.click(screen.getByRole('button', { name: 'Next' }));
    expect(screen.getByText('2 of 2')).toBeInTheDocument();

    const scrollCountBefore = codeViewMockState.scrollToCalls.length;

    rerender(
      <ReviewNotesProvider>
        <DiffView
          open
          comparator="origin/main"
          commitLog=""
          committedDiff={insertedBefore}
          uncommittedDiff=""
          checkoutStatus={DEFAULT_CHECKOUT_STATUS}
          onClose={() => undefined}
          onSendNotes={() => undefined}
        />
      </ReviewNotesProvider>,
    );

    await waitFor(() => expect(screen.getByText('3 of 3')).toBeInTheDocument());
    await waitFor(() => expect(codeViewMockState.scrollToCalls.length).toBe(scrollCountBefore + 1));
  });

  it('keeps repeated identical diff matches distinct', async () => {
    const repeated = [
      'diff --git a/foo.txt b/foo.txt',
      '--- a/foo.txt',
      '+++ b/foo.txt',
      '@@ -0,0 +1,2 @@',
      '+same',
      '+same',
    ].join('\n');
    renderDiff(repeated, '');

    fireEvent.click(screen.getByRole('button', { name: 'Find in diff' }));
    fireEvent.change(screen.getByRole('textbox', { name: 'Find in viewer' }), { target: { value: 'same' } });

    await waitFor(() => expect(screen.getByText('1 of 2')).toBeInTheDocument());
    fireEvent.click(screen.getByRole('button', { name: 'Next' }));
    expect(screen.getByText('2 of 2')).toBeInTheDocument();
  });

  it('keeps diff find inert until open with a non-empty query', () => {
    renderDiff(COMMITTED, COMMITTED.replaceAll('foo.txt', 'bar.txt'));

    expect(screen.queryByText('1 of 1')).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Find in diff' }));
    expect(screen.getByRole('textbox', { name: 'Find in viewer' })).toHaveValue('');
    fireEvent.change(screen.getByRole('textbox', { name: 'Find in viewer' }), { target: { value: 'bar' } });
    expect(screen.getByText('1 of 1')).toBeInTheDocument();
  });

  it('does not register diff find shortcuts while a closed diff remains mounted', () => {
    render(
      <ReviewNotesProvider>
        <DiffView
          open={false}
          checkoutStatus={DEFAULT_CHECKOUT_STATUS}
          comparator="origin/main"
          commitLog=""
          committedDiff={COMMITTED}
          uncommittedDiff=""
          onClose={() => undefined}
          onSendNotes={() => undefined}
        />
      </ReviewNotesProvider>,
    );

    fireEvent.keyDown(window, { key: 'f', metaKey: true, bubbles: true, cancelable: true });
    expect(screen.queryByRole('textbox', { name: 'Find in viewer' })).toBeNull();
  });

  it('opens shared viewer find for diff-viewer scope and navigates header then line matches via typed scroll targets', async () => {
    renderDiff(COMMITTED, COMMITTED.replaceAll('foo.txt', 'bar.txt'));

    fireEvent.click(screen.getByRole('button', { name: 'Find in diff' }));
    const input = screen.getByRole('textbox', { name: 'Find in viewer' });
    fireEvent.change(input, { target: { value: 'bar' } });

    await waitFor(() => expect(screen.getByText('1 of 1')).toBeInTheDocument());

    fireEvent.change(input, { target: { value: 'hello' } });
    await waitFor(() => expect(screen.getByText('1 of 2')).toBeInTheDocument());
    fireEvent.click(screen.getByRole('button', { name: 'Next' }));
    fireEvent.click(screen.getByRole('button', { name: 'Next' }));
    await waitFor(() => expect(codeViewMockState.scrollToCalls.at(-1)).toEqual({
      type: 'line',
      id: 'committed:foo.txt',
      lineNumber: 1,
      side: 'additions',
      align: 'center',
      behavior: 'smooth',
    }));
  });

  it('marks non-active diff header matches distinctly while keeping the exact colon-containing header active', () => {
    const committed = [
      'diff --git a/foo b/foo',
      'index 0000000..1111111 100644',
      '--- a/foo',
      '+++ b/foo',
      '@@ -0,0 +1,1 @@',
      '+plain file',
      'diff --git a/foo:bar b/foo:bar',
      'index 2222222..3333333 100644',
      '--- a/foo:bar',
      '+++ b/foo:bar',
      '@@ -0,0 +1,1 @@',
      '+colon file',
    ].join('\n');

    renderDiff(committed, '');

    const fooVersionBefore = itemVersion('committed:foo') ?? 0;
    const fooBarVersionBefore = itemVersion('committed:foo:bar') ?? 0;

    fireEvent.click(screen.getByRole('button', { name: 'Find in diff' }));
    fireEvent.change(screen.getByRole('textbox', { name: 'Find in viewer' }), { target: { value: 'foo' } });

    const fooHeader = screen.getByTestId('mock-header-committed:foo');
    const fooBarHeader = screen.getByTestId('mock-header-committed:foo:bar');
    expect(fooHeader.querySelector('.phoenix-diff-section-badge--find-active')).not.toBeNull();
    expect(fooHeader.querySelector('.phoenix-diff-file-note-btn--find-active')).not.toBeNull();
    expect(fooBarHeader.querySelector('.phoenix-diff-section-badge--find-match')).not.toBeNull();
    expect(fooBarHeader.querySelector('.phoenix-diff-file-note-btn--find-match')).not.toBeNull();
    expect(fooBarHeader.querySelector('.phoenix-diff-section-badge--find-active')).toBeNull();
    expect(fooBarHeader.querySelector('.phoenix-diff-file-note-btn--find-active')).toBeNull();

    expect((itemVersion('committed:foo') ?? 0)).toBeGreaterThan(fooVersionBefore);
    expect((itemVersion('committed:foo:bar') ?? 0)).toBeGreaterThan(fooBarVersionBefore);
  });

  it('decorates matched diff lines with unsafeCSS and clears decorations when the find bar closes', () => {
    renderDiff(COMMITTED, COMMITTED.replaceAll('foo.txt', 'bar.txt'));
    fireEvent.click(screen.getByRole('button', { name: 'Find in diff' }));
    const input = screen.getByRole('textbox', { name: 'Find in viewer' });

    fireEvent.change(input, { target: { value: 'hello' } });
    expect(codeViewMockState.lastUnsafeCss).toContain('[data-item-id="committed:foo.txt"] [data-additions=""] [data-line="1"]');

    fireEvent.change(input, { target: { value: 'bar' } });
    fireEvent.keyDown(input, { key: 'Escape' });
    expect(codeViewMockState.lastUnsafeCss).toBe('');
  });

  it('lets shell Escape close an open find bar after focus returns to the viewer body', async () => {
    renderDiff(COMMITTED, COMMITTED.replaceAll('foo.txt', 'bar.txt'));

    fireEvent.click(screen.getByRole('button', { name: 'Find in diff' }));
    const input = screen.getByRole('textbox', { name: 'Find in viewer' });
    fireEvent.change(input, { target: { value: 'hello' } });

    const bodyLine = screen.getByTestId('mock-line-el-committed:foo.txt');
    bodyLine.focus();
    fireEvent.keyDown(window, { key: 'Escape' });

    await waitFor(() => expect(screen.queryByRole('textbox', { name: 'Find in viewer' })).toBeNull());
    expect(codeViewMockState.lastUnsafeCss).toBe('');
  });

  it('restores diff find focus to the actual opener when closed from body Escape', async () => {
    renderDiff(COMMITTED, COMMITTED.replaceAll('foo.txt', 'bar.txt'));

    const bodyLine = screen.getByTestId('mock-line-el-committed:foo.txt');
    const findButton = screen.getByRole('button', { name: 'Find in diff' });
    bodyLine.tabIndex = 0;
    bodyLine.focus();
    fireEvent.click(findButton);

    const input = screen.getByRole('textbox', { name: 'Find in viewer' });
    fireEvent.change(input, { target: { value: 'hello' } });

    bodyLine.focus();
    fireEvent.keyDown(window, { key: 'Escape' });

    await waitFor(() => expect(screen.queryByRole('textbox', { name: 'Find in viewer' })).toBeNull());
    expect(bodyLine).toHaveFocus();
  });

  it('lets find Escape close the find bar before the shell closes the viewer', () => {
    const onClose = vi.fn();
    render(
      <ReviewNotesProvider>
        <DiffView
          open
          checkoutStatus={DEFAULT_CHECKOUT_STATUS}
          comparator="origin/main"
          commitLog=""
          committedDiff={COMMITTED}
          uncommittedDiff=""
          onClose={onClose}
          onSendNotes={() => undefined}
        />
      </ReviewNotesProvider>,
    );

    fireEvent.click(screen.getByRole('button', { name: 'Find in diff' }));
    const input = screen.getByRole('textbox', { name: 'Find in viewer' });
    fireEvent.keyDown(input, { key: 'Escape' });

    expect(screen.queryByRole('textbox', { name: 'Find in viewer' })).not.toBeInTheDocument();
    expect(onClose).not.toHaveBeenCalled();

    fireEvent.keyDown(window, { key: 'Escape' });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it('clears diff find state when open transitions false on the same mounted viewer', async () => {
    const { rerender } = render(
      <ReviewNotesProvider>
        <DiffView
          open
          checkoutStatus={DEFAULT_CHECKOUT_STATUS}
          comparator="origin/main"
          commitLog=""
          committedDiff={COMMITTED}
          uncommittedDiff=""
          onClose={() => undefined}
          onSendNotes={() => undefined}
        />
      </ReviewNotesProvider>,
    );

    fireEvent.click(screen.getByRole('button', { name: 'Find in diff' }));
    fireEvent.change(screen.getByRole('textbox', { name: 'Find in viewer' }), { target: { value: 'hello' } });
    expect(screen.getByText('1 of 1')).toBeInTheDocument();

    rerender(
      <ReviewNotesProvider>
        <DiffView
          open={false}
          checkoutStatus={DEFAULT_CHECKOUT_STATUS}
          comparator="origin/main"
          commitLog=""
          committedDiff={COMMITTED}
          uncommittedDiff=""
          onClose={() => undefined}
          onSendNotes={() => undefined}
        />
      </ReviewNotesProvider>,
    );

    rerender(
      <ReviewNotesProvider>
        <DiffView
          open
          checkoutStatus={DEFAULT_CHECKOUT_STATUS}
          comparator="origin/main"
          commitLog=""
          committedDiff={COMMITTED}
          uncommittedDiff=""
          onClose={() => undefined}
          onSendNotes={() => undefined}
        />
      </ReviewNotesProvider>,
    );

    await waitFor(() => expect(screen.queryByRole('textbox', { name: 'Find in viewer' })).toBeNull());
    expect(screen.queryByText('1 of 1')).not.toBeInTheDocument();
  });

  it('does not open find behind an annotation dialog', () => {
    renderDiff(COMMITTED, '');
    fireEvent.click(screen.getByTestId('mock-line-click-committed:foo.txt'));
    expect(screen.getByRole('dialog')).toBeInTheDocument();

    fireEvent.keyDown(screen.getByRole('button', { name: 'Cancel' }), { key: 'f', metaKey: true });
    expect(screen.queryByRole('textbox', { name: 'Find in viewer' })).toBeNull();
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
      // The press is dispatched on the line element so its composed path lets
      // the touch resolver recover the line under the finger (no hover ref).
      const lineEl = screen.getByTestId('mock-line-el-committed:foo.txt');

      const press = (type: string, x = 0, y = 0) => {
        const ev = new Event(type, { bubbles: true });
        Object.assign(ev, { pointerType: 'touch', clientX: x, clientY: y });
        act(() => {
          lineEl.dispatchEvent(ev);
        });
      };

      // Movement before the threshold cancels the press → no dialog.
      press('pointerdown');
      press('pointermove', 50, 50);
      act(() => vi.advanceTimersByTime(600));
      expect(screen.queryByText('foo.txt:1')).not.toBeInTheDocument();

      // A still 500ms hold opens the dialog anchored at the resolved line.
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



  it('shows named-branch checkout status before commits with shortened refs and last-fetched remote state', () => {
    renderDiff(COMMITTED, '', {
      kind: 'named_branch',
      branch_name: 'task-82003',
      head_oid: '1234567890abcdef1234567890abcdef12345678',
      remote_status: {
        kind: 'tracked',
        remote_ref: 'refs/remotes/origin/task-82003',
        ahead: 2,
        behind: 0,
      },
    });

    const panel = screen.getByLabelText('Checkout status');
    expect(panel).toHaveTextContent(/Branch\s*task-82003/);
    expect(panel).toHaveTextContent('task-82003');
    expect(panel).toHaveTextContent('Tracked upstream:');
    expect(panel).toHaveTextContent('origin/task-82003');
    expect(panel).toHaveTextContent('Last fetched 2 to push');
    const summary = screen.getByText(/Committed changes \(vs origin\/main\)/);
    expect(summary.compareDocumentPosition(panel) & Node.DOCUMENT_POSITION_PRECEDING).toBeTruthy();
    expect(screen.getByTitle('refs/remotes/origin/task-82003')).toBeInTheDocument();
  });

  it('shows checkout status even when the diff is empty', () => {
    renderDiff('', '', {
      kind: 'named_branch',
      branch_name: 'main',
      head_oid: '1234567890abcdef1234567890abcdef12345678',
      remote_status: { kind: 'no_known' },
    });

    expect(screen.getByLabelText('Checkout status')).toHaveTextContent('No known remote branch (last fetched state).');
    expect(screen.getByText(/No changes vs/)).toBeInTheDocument();
  });

  it('distinguishes matching remotes, detached heads, and unavailable checkout state', () => {
    const { rerender } = render(
      <ReviewNotesProvider>
        <DiffView
          open
          comparator="origin/main"
          commitLog=""
          committedDiff={COMMITTED}
          uncommittedDiff=""
          checkoutStatus={{
            kind: 'named_branch',
            branch_name: 'task-82003',
            head_oid: '1234567890abcdef1234567890abcdef12345678',
            remote_status: {
              kind: 'matching',
              remote_ref: 'refs/remotes/origin/task-82003',
              ahead: 1,
              behind: 3,
            },
          }}
          onClose={() => undefined}
          onSendNotes={() => undefined}
        />
      </ReviewNotesProvider>,
    );

    expect(screen.getByLabelText('Checkout status')).toHaveTextContent('Matching remote:');
    expect(screen.getByLabelText('Checkout status')).toHaveTextContent('diverged (1 to push, 3 behind)');
    expect(screen.getByLabelText('Checkout status')).toHaveTextContent('(not configured as upstream)');

    rerender(
      <ReviewNotesProvider>
        <DiffView
          open
          comparator="origin/main"
          commitLog=""
          committedDiff={COMMITTED}
          uncommittedDiff=""
          checkoutStatus={{
            kind: 'detached',
            head_oid: '1234567890abcdef1234567890abcdef12345678',
            pointing_refs: ['refs/remotes/origin/main', 'refs/tags/v1.2.3'],
          }}
          onClose={() => undefined}
          onSendNotes={() => undefined}
        />
      </ReviewNotesProvider>,
    );

    expect(screen.getByLabelText('Checkout status')).toHaveTextContent('Detached HEAD');
    expect(screen.getByLabelText('Checkout status')).toHaveTextContent('1234567890ab');
    expect(screen.getByLabelText('Checkout status')).toHaveTextContent('origin/main, v1.2.3');

    rerender(
      <ReviewNotesProvider>
        <DiffView
          open
          comparator="origin/main"
          commitLog=""
          committedDiff={COMMITTED}
          uncommittedDiff=""
          checkoutStatus={{ kind: 'unavailable', reason: 'git status failed' }}
          onClose={() => undefined}
          onSendNotes={() => undefined}
        />
      </ReviewNotesProvider>,
    );

    expect(screen.getByLabelText('Checkout status')).toHaveTextContent('Checkout status unavailable');
    expect(screen.getByLabelText('Checkout status')).toHaveTextContent('git status failed');
  });

  it('renders an unreferenced detached HEAD without crashing', () => {
    renderDiff('', '', {
      kind: 'detached',
      head_oid: 'abcdef1234567890abcdef1234567890abcdef12',
      pointing_refs: [],
    });

    const panel = screen.getByLabelText('Checkout status');
    expect(panel).toHaveTextContent('Detached HEAD');
    expect(panel).toHaveTextContent('abcdef123456');
    expect(panel).not.toHaveTextContent('Points to:');
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
          checkoutStatus={DEFAULT_CHECKOUT_STATUS}
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
