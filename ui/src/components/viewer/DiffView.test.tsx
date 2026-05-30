import { render, screen, fireEvent, within } from '@testing-library/react';
import { describe, it, expect } from 'vitest';
import { DiffView } from './DiffView';
import { ReviewNotesProvider } from '../../contexts/ReviewNotesContext';

const COMMITTED = [
  'diff --git a/foo.txt b/foo.txt',
  'index 0000000..1111111 100644',
  '--- a/foo.txt',
  '+++ b/foo.txt',
  '@@ -0,0 +1,1 @@',
  '+hello world',
].join('\n');

function renderDiff() {
  return render(
    <ReviewNotesProvider>
      <DiffView
        open
        comparator="origin/main"
        commitLog=""
        committedDiff={COMMITTED}
        uncommittedDiff=""
        onClose={() => undefined}
        onSendNotes={() => undefined}
      />
    </ReviewNotesProvider>,
  );
}

describe('DiffView review-note lifecycle (useDiffReviewNotes)', () => {
  it('renders both diff sections and the committed change', () => {
    renderDiff();
    expect(screen.getByText(/Committed changes \(vs origin\/main\)/)).toBeInTheDocument();
    expect(screen.getByText('+hello world')).toBeInTheDocument();
  });

  it('adds a line note via the annotate affordance and surfaces it in the badge + panel', () => {
    renderDiff();

    // The added line's annotate button opens the dialog anchored to that line.
    const addedRow = screen.getByText('+hello world').closest('.diff-line') as HTMLElement;
    fireEvent.click(within(addedRow).getByRole('button', { name: 'Add note' }));

    // Dialog anchored at foo.txt:1 (newLine 1).
    expect(screen.getByText('foo.txt:1')).toBeInTheDocument();

    const textarea = screen.getByPlaceholderText(/Add your note/);
    fireEvent.change(textarea, { target: { value: 'looks good' } });
    fireEvent.click(screen.getByRole('button', { name: 'Add Note' }));

    // Badge reflects the new note; opening the panel shows its body.
    const badge = screen.getByRole('button', { name: '1 notes' });
    expect(badge).toBeInTheDocument();
    fireEvent.click(badge);
    expect(screen.getByText('looks good')).toBeInTheDocument();
  });
});
