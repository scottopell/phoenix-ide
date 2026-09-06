import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { CompletedContinuationBoundary, ContextExhaustedHandoff } from './ContextExhaustedHandoff';

const generated = 'Generated operational handoff';

function renderHandoff(overrides: Partial<React.ComponentProps<typeof ContextExhaustedHandoff>> = {}) {
  const props: React.ComponentProps<typeof ContextExhaustedHandoff> = {
    parentId: 'parent-1',
    generatedHandoff: generated,
    continuedInConvId: null,
    onOpenExisting: vi.fn(),
    onContinue: vi.fn().mockResolvedValue('accepted'),
    onCopy: vi.fn(),
    ...overrides,
  };
  return { ...render(<ContextExhaustedHandoff {...props} />), props };
}

describe('ContextExhaustedHandoff', () => {
  beforeEach(() => {
    localStorage.clear();
    vi.restoreAllMocks();
  });

  it('offers explicit review actions without workspace terminal actions', () => {
    renderHandoff();
    expect(screen.getByRole('button', { name: 'Continue' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Edit first' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Copy handoff' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /clean up/i })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /abandon/i })).not.toBeInTheDocument();
  });

  it('submits the immutable generated handoff as-is', async () => {
    const onContinue = vi.fn().mockResolvedValue('accepted');
    renderHandoff({ onContinue });
    fireEvent.click(screen.getByRole('button', { name: 'Continue' }));
    await waitFor(() => expect(onContinue).toHaveBeenCalledWith(generated));
  });

  it('does not reset an immediate edit interaction during mount effects', async () => {
    renderHandoff();

    fireEvent.click(screen.getByRole('button', { name: 'Edit first' }));

    expect(await screen.findByRole('textbox', { name: 'Edit handoff' })).toHaveValue(generated);
  });

  it('keeps edits browser-local across cancel and remount', () => {
    const first = renderHandoff();
    fireEvent.click(screen.getByRole('button', { name: 'Edit first' }));
    fireEvent.change(screen.getByRole('textbox', { name: 'Edit handoff' }), { target: { value: 'My edited handoff' } });
    fireEvent.click(screen.getByRole('button', { name: 'Cancel editing' }));
    expect(screen.getByText('Local edit saved')).toBeInTheDocument();
    first.unmount();

    renderHandoff();
    fireEvent.click(screen.getByRole('button', { name: 'Edit first' }));
    expect(screen.getByRole('textbox', { name: 'Edit handoff' })).toHaveValue('My edited handoff');
  });

  it('reverts only the local draft and restores generated text', () => {
    vi.stubGlobal('confirm', vi.fn(() => true));
    renderHandoff();
    fireEvent.click(screen.getByRole('button', { name: 'Edit first' }));
    fireEvent.change(screen.getByRole('textbox', { name: 'Edit handoff' }), { target: { value: 'Changed' } });
    fireEvent.click(screen.getByRole('button', { name: 'Revert to generated' }));
    expect(screen.getByRole('textbox', { name: 'Edit handoff' })).toHaveValue(generated);
    expect(localStorage.getItem('handoff-edit-draft:parent-1')).toBeNull();
  });

  it('rejects a blank edit before invoking continuation', () => {
    const onContinue = vi.fn().mockResolvedValue('accepted');
    renderHandoff({ onContinue });
    fireEvent.click(screen.getByRole('button', { name: 'Edit first' }));
    fireEvent.change(screen.getByRole('textbox', { name: 'Edit handoff' }), { target: { value: '   ' } });
    fireEvent.click(screen.getByRole('button', { name: 'Continue with edits' }));
    expect(screen.getByRole('alert')).toHaveTextContent('cannot be empty');
    expect(onContinue).not.toHaveBeenCalled();
  });

  it('copies generated text in review and edited text in edit mode', () => {
    const onCopy = vi.fn();
    renderHandoff({ onCopy });
    fireEvent.click(screen.getByRole('button', { name: 'Copy handoff' }));
    expect(onCopy).toHaveBeenLastCalledWith(generated);
    fireEvent.click(screen.getByRole('button', { name: 'Edit first' }));
    fireEvent.change(screen.getByRole('textbox', { name: 'Edit handoff' }), { target: { value: 'External refinement' } });
    fireEvent.click(screen.getByRole('button', { name: 'Copy edited handoff' }));
    expect(onCopy).toHaveBeenLastCalledWith('External refinement');
  });

  it('presents a completed continuation as a reviewable historical boundary', () => {
    render(<CompletedContinuationBoundary summary={generated} />);
    const boundary = screen.getByRole('region', { name: 'Conversation continuation boundary' });
    const review = screen.getByText('Review handoff');

    expect(boundary).toHaveTextContent('Conversation continued in the next segment');
    expect(review.closest('details')).not.toHaveAttribute('open');
    fireEvent.click(review);
    expect(review.closest('details')).toHaveAttribute('open');
    expect(boundary).toHaveTextContent(generated);
    expect(screen.queryByRole('button', { name: 'Continue' })).not.toBeInTheDocument();
  });

  it('reveals a completed handoff when transcript Find selects its summary', () => {
    render(
      <CompletedContinuationBoundary
        summary={generated}
        revealedSummary={<mark>operational</mark>}
        revealSummary
      />,
    );

    const review = screen.getByText('Review handoff');
    expect(review.closest('details')).toHaveAttribute('open');
    expect(screen.getByText('operational').tagName).toBe('MARK');
    expect(screen.getByText('operational').closest('[data-fragment-id="message-text"]')).not.toBeNull();
  });

  it('shows only navigation after a successor exists', () => {
    renderHandoff({ continuedInConvId: 'successor-1' });
    expect(screen.getByRole('button', { name: 'Open continuation' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Continue' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Edit first' })).not.toBeInTheDocument();
  });
});
