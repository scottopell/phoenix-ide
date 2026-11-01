import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { api, ConflictError, type HandoffCapacityReview } from '../api';
import { CompletedContinuationBoundary, ContextExhaustedHandoff } from './ContextExhaustedHandoff';

const generated = 'Generated operational handoff';

function renderHandoff(overrides: Partial<React.ComponentProps<typeof ContextExhaustedHandoff>> = {}) {
  const props: React.ComponentProps<typeof ContextExhaustedHandoff> = {
    parentId: 'parent-1',
    generatedHandoff: generated,
    generatedSource: { kind: 'summary', message_id: 'actual-summary' },
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
    vi.spyOn(api, 'handoffCandidates').mockResolvedValue([]);
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
    await waitFor(() => expect(onContinue).toHaveBeenCalledWith(generated, { kind: 'summary', message_id: 'actual-summary' }));
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
    const review = screen.getByText('Context boundary · Review handoff');

    expect(boundary).toHaveClass('conversation-history-boundary');
    expect(boundary).not.toHaveClass('context-exhausted-banner');
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

    const review = screen.getByText('Context boundary · Review handoff');
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

const capacity = (kind: 'ready' | 'handoff_too_large' | 'required_overhead' = 'ready'): HandoffCapacityReview => ({
  counting_method: 'conservative_local_estimate', context_window_tokens: 32000,
  required_overhead_tokens: 3000, output_reservation_tokens: 8000,
  successor_working_room_tokens: 8192, handoff_tokens: kind === 'ready' ? 400 : 16000,
  provider_input_tokens: null, status: kind === 'required_overhead' ? { kind } : { kind, target_tokens: 12808 },
});

describe('reviewed capacity recovery', () => {
  beforeEach(() => {
    localStorage.clear(); vi.restoreAllMocks();
    vi.spyOn(api, 'handoffCandidates').mockResolvedValue([]);
  });

  it('keeps the actual generated source identity distinct from explicit literal edits', async () => {
    const onContinue = vi.fn().mockResolvedValue('dispatch_failed');
    renderHandoff({ generatedSource: { kind: 'summary', message_id: 'actual-summary' }, onContinue });
    fireEvent.click(screen.getByRole('button', { name: 'Continue' }));
    await waitFor(() => expect(onContinue).toHaveBeenCalledWith(generated, { kind: 'summary', message_id: 'actual-summary' }));
    fireEvent.click(screen.getByRole('button', { name: 'Edit first' }));
    fireEvent.click(screen.getByRole('button', { name: 'Continue with edits' }));
    await waitFor(() => expect(onContinue).toHaveBeenLastCalledWith(generated, { kind: 'manual' }));
  });

  it('requires explicit preview acceptance and submits the named candidate', async () => {
    vi.spyOn(api, 'reviewHandoff').mockResolvedValue({ source: capacity('handoff_too_large'), shortening: {
      kind: 'generated', candidate_id: 'short-1', text: 'Short reviewed result', capacity: capacity(),
    } });
    const onContinue = vi.fn().mockResolvedValue('accepted');
    renderHandoff({ onContinue });
    fireEvent.click(screen.getByRole('button', { name: 'Request AI shortening' }));
    expect(await screen.findByRole('region', { name: 'AI shortening preview' })).toHaveTextContent('Short reviewed result');
    expect(onContinue).not.toHaveBeenCalled();
    expect(screen.getByText(generated)).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Accept AI shortening and continue' }));
    await waitFor(() => expect(onContinue).toHaveBeenCalledWith('Short reviewed result', { kind: 'candidate', candidate_id: 'short-1' }));
  });

  it('retains the original draft when shortening fails or remains oversized', async () => {
    vi.spyOn(api, 'reviewHandoff').mockResolvedValueOnce({ source: capacity('handoff_too_large'), shortening: { kind: 'failed', error: 'Rewrite cannot fit' } })
      .mockResolvedValueOnce({ source: capacity('handoff_too_large'), shortening: { kind: 'generated', candidate_id: 'still-large', text: 'Still oversized', capacity: capacity('handoff_too_large') } });
    renderHandoff();
    fireEvent.click(screen.getByRole('button', { name: 'Edit first' }));
    fireEvent.change(screen.getByRole('textbox'), { target: { value: 'My original draft' } });
    fireEvent.click(screen.getByRole('button', { name: 'Request AI shortening' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('Rewrite cannot fit');
    expect(screen.getByRole('textbox')).toHaveValue('My original draft');
    fireEvent.click(screen.getByRole('button', { name: 'Request AI shortening' }));
    expect(await screen.findByRole('button', { name: 'Accept AI shortening and continue' })).toBeDisabled();
    expect(screen.getByRole('textbox')).toHaveValue('My original draft');
    expect(localStorage.getItem('handoff-edit-draft:parent-1')).toContain('My original draft');
  });

  it('ignores an obsolete review after editing and after navigation', async () => {
    let resolve!: (value: Awaited<ReturnType<typeof api.reviewHandoff>>) => void;
    vi.spyOn(api, 'reviewHandoff').mockImplementation(() => new Promise((done) => { resolve = done; }));
    const { rerender, props } = renderHandoff();
    fireEvent.click(screen.getByRole('button', { name: 'Request AI shortening' }));
    fireEvent.click(screen.getByRole('button', { name: 'Edit first' }));
    fireEvent.change(screen.getByRole('textbox'), { target: { value: 'Newer edit' } });
    resolve({ source: capacity(), shortening: { kind: 'generated', candidate_id: 'stale', text: 'Stale text', capacity: capacity() } });
    await waitFor(() => expect(screen.queryByRole('region', { name: 'AI shortening preview' })).not.toBeInTheDocument());
    expect(screen.getByRole('textbox')).toHaveValue('Newer edit');
    fireEvent.click(screen.getByRole('button', { name: 'Request AI shortening' }));
    rerender(<ContextExhaustedHandoff {...props} parentId="different-parent" />);
    resolve({ source: capacity(), shortening: { kind: 'generated', candidate_id: 'stale-2', text: 'Stale navigation', capacity: capacity() } });
    await waitFor(() => expect(screen.queryByText('Stale navigation')).not.toBeInTheDocument());
  });

  it('shows authoritative admission conflicts and distinguishes irreducible overhead', async () => {
    renderHandoff({ onContinue: vi.fn().mockRejectedValue(new ConflictError({ error: 'Fresh tools no longer fit', error_type: 'continuation_capacity_conflict', capacity: capacity('required_overhead') })) });
    fireEvent.click(screen.getByRole('button', { name: 'Continue' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('Fresh tools no longer fit');
    expect(screen.getByRole('status')).toHaveTextContent('Editing the handoff cannot fix this');
    expect(screen.getByRole('button', { name: 'Continue' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Request AI shortening' })).toBeDisabled();
  });

  it('reviews historical retained inputs as manual and generated candidates by identity', async () => {
    vi.mocked(api.handoffCandidates).mockResolvedValue([
      { id: 'input', conversation_id: 'parent-1', origin: { kind: 'review_input', revision: { kind: 'operation', operation_id: 'op', attempt: 1 } }, source: { kind: 'unattributed_input' }, text: 'Retained literal' },
      { id: 'candidate', conversation_id: 'parent-1', origin: { kind: 'oversized_summary', operation_id: 'op', attempt: 1 }, source: { kind: 'unattributed_input' }, text: 'Generated candidate' },
    ]);
    const review = vi.spyOn(api, 'reviewHandoff').mockResolvedValue({ source: capacity(), shortening: { kind: 'not_requested' } });
    const onContinue = vi.fn().mockResolvedValue('dispatch_failed');
    renderHandoff({ capacityConflict: true, generatedHandoff: '', onContinue });
    const select = await screen.findByRole('combobox', { name: 'Retained handoff source' });
    fireEvent.change(select, { target: { value: 'input' } });
    expect(screen.getByRole('button', { name: 'Accept selected handoff' })).toBeDisabled();
    fireEvent.click(screen.getByRole('button', { name: 'Check fit' }));
    await waitFor(() => expect(screen.getByRole('button', { name: 'Accept selected handoff' })).toBeEnabled());
    expect(review).toHaveBeenLastCalledWith('parent-1', { handoff: 'Retained literal', source: { kind: 'manual' }, shorten: false });
    fireEvent.click(screen.getByRole('button', { name: 'Accept selected handoff' }));
    await waitFor(() => expect(onContinue).toHaveBeenCalledWith('Retained literal', { kind: 'manual' }));
    fireEvent.change(select, { target: { value: 'candidate' } });
    fireEvent.click(screen.getByRole('button', { name: 'Check fit' }));
    await waitFor(() => expect(review).toHaveBeenLastCalledWith('parent-1', { handoff: 'Generated candidate', source: { kind: 'candidate', candidate_id: 'candidate' }, shorten: false }));
  });

  it('offers regeneration on every exhausted handoff and preserves existing navigation', () => {
    const onRegenerate = vi.fn();
    const { rerender, props } = renderHandoff({ generatedHandoff: 'Any historical failure notice', onRegenerate });
    fireEvent.click(screen.getByRole('button', { name: 'Regenerate handoff' }));
    expect(onRegenerate).toHaveBeenCalledOnce();
    rerender(<ContextExhaustedHandoff {...props} continuedInConvId="existing" />);
    expect(screen.queryByRole('button', { name: 'Regenerate handoff' })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Open continuation' }));
    expect(props.onOpenExisting).toHaveBeenCalledOnce();
  });
});


describe('recovery authority and submission ownership', () => {
  beforeEach(() => { localStorage.clear(); vi.restoreAllMocks(); vi.spyOn(api, 'handoffCandidates').mockResolvedValue([]); });
  it('blocks unnamed generated sources until a user explicitly chooses manual editing', async () => {
    const review = vi.spyOn(api, 'reviewHandoff');
    const { props } = renderHandoff({ generatedSource: null });
    expect(screen.getByRole('button', { name: 'Continue' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Check fit' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Request AI shortening' })).toBeDisabled();
    expect(screen.getByRole('status')).toHaveTextContent('summary source is not loaded');
    expect(review).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: 'Edit first' }));
    fireEvent.click(screen.getByRole('button', { name: 'Continue with edits' }));
    await waitFor(() => expect(props.onContinue).toHaveBeenCalledWith(generated, { kind: 'manual' }));
  });
  it('does not clear the old saved draft after a late submission outlives navigation', async () => {
    let resolve!: (value: 'accepted') => void;
    const { rerender, props } = renderHandoff({ onContinue: vi.fn(() => new Promise<'accepted'>((done) => { resolve = done; })) });
    fireEvent.click(screen.getByRole('button', { name: 'Edit first' }));
    fireEvent.change(screen.getByRole('textbox'), { target: { value: 'Original saved edit' } });
    fireEvent.click(screen.getByRole('button', { name: 'Continue with edits' }));
    rerender(<ContextExhaustedHandoff {...props} parentId="new-parent" />);
    resolve('accepted');
    await waitFor(() => expect(screen.getByRole('button', { name: 'Continue' })).toBeEnabled());
    expect(localStorage.getItem('handoff-edit-draft:parent-1')).toContain('Original saved edit');
  });
});


describe('local review accounting', () => {
  beforeEach(() => { localStorage.clear(); vi.restoreAllMocks(); vi.spyOn(api, 'handoffCandidates').mockResolvedValue([]); });
  it('shows local input numerically without counting output or successor working room as input', async () => {
    vi.spyOn(api, 'reviewHandoff').mockResolvedValue({ source: capacity(), shortening: { kind: 'not_requested' } });
    renderHandoff();
    fireEvent.click(screen.getByRole('button', { name: 'Check fit' }));
    expect(await screen.findByText('Complete input (local estimate)')).toBeInTheDocument();
    expect(screen.getByText('3,400 · instructions/tools + handoff')).toBeInTheDocument();
    expect(screen.getByText('Provider complete input').nextElementSibling).toHaveTextContent('Not available');
  });
});
