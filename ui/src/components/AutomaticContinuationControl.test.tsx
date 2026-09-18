import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { AutomaticContinuationControl } from './AutomaticContinuationControl';
import type { AutomaticContinuationView } from '../api';

const { apiMock } = vi.hoisted(() => ({
  apiMock: {
    getProductConversationAutomaticContinuation: vi.fn(),
    updateProductConversationAutomaticContinuation: vi.fn(),
    getCoordinatorAutomaticContinuation: vi.fn(),
    updateCoordinatorAutomaticContinuation: vi.fn(),
  },
}));

vi.mock('../api', async () => {
  const actual = await vi.importActual<typeof import('../api')>('../api');
  return { ...actual, api: { ...actual.api, ...apiMock } };
});

function view(overrides: Partial<AutomaticContinuationView> = {}): AutomaticContinuationView {
  return {
    aggregate: { kind: 'ordinary', product_conversation_id: 'pc-1' },
    auto_continue_on_context_exhaustion: false,
    admission: null,
    ...overrides,
  };
}

async function openControl() {
  fireEvent.click(await screen.findByText(/Auto-continue/));
  return screen.findByRole('checkbox', { name: 'Automatically accept future generated handoffs and continue' });
}

describe('AutomaticContinuationControl', () => {
  beforeEach(() => {
    vi.resetAllMocks();
    vi.useRealTimers();
    apiMock.getProductConversationAutomaticContinuation.mockResolvedValue(view());
    apiMock.updateProductConversationAutomaticContinuation.mockResolvedValue(view({
      auto_continue_on_context_exhaustion: true,
    }));
    apiMock.getCoordinatorAutomaticContinuation.mockResolvedValue(view({
      aggregate: { kind: 'coordinator', product_conversation_id: 'coordinator-pc' },
    }));
    apiMock.updateCoordinatorAutomaticContinuation.mockResolvedValue(view({
      aggregate: { kind: 'coordinator', product_conversation_id: 'coordinator-pc' },
      auto_continue_on_context_exhaustion: true,
    }));
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('defaults an ordinary ProductConversation OFF from GET and immediately PUTs the checked value', async () => {
    render(<AutomaticContinuationControl scope={{ kind: 'ordinary', reference: 'pc/1' }} />);

    const checkbox = await openControl();
    expect(checkbox).not.toBeChecked();
    expect(apiMock.getProductConversationAutomaticContinuation).toHaveBeenCalledWith('pc/1');
    expect(screen.getByText(/Applies only to future entries into context exhaustion/)).toBeInTheDocument();
    expect(screen.getByText(/does not start or resume an already-exhausted conversation/)).toBeInTheDocument();
    expect(screen.getByText(/or cancel continuation work already admitted/)).toBeInTheDocument();

    fireEvent.click(checkbox);
    expect(screen.getByText('Saving…')).toBeInTheDocument();
    await waitFor(() => expect(apiMock.updateProductConversationAutomaticContinuation).toHaveBeenCalledWith('pc/1', true));
    expect(await screen.findByText('Saved')).toBeInTheDocument();
    expect(checkbox).toBeChecked();
  });

  it('uses the Global Coordinator contract and exposes admitted progress', async () => {
    apiMock.getCoordinatorAutomaticContinuation.mockResolvedValueOnce(view({
      aggregate: { kind: 'coordinator', product_conversation_id: 'coordinator-pc' },
      admission: {
        predecessor_transcript_row_id: 'coordinator-row',
        phase: 'ownership_transferred',
        no_progress_attempts: 1,
        actionable_failure: null,
      },
    }));
    render(<AutomaticContinuationControl scope={{ kind: 'coordinator' }} />);

    const checkbox = await openControl();
    expect(checkbox).not.toBeChecked();
    expect(screen.getByText('Ownership transferred', { selector: 'strong' })).toBeInTheDocument();
    expect(screen.getByText(/1 no-progress attempt/)).toBeInTheDocument();

    fireEvent.click(checkbox);
    await waitFor(() => expect(apiMock.updateCoordinatorAutomaticContinuation).toHaveBeenCalledWith(true));
    expect(await screen.findByText('Saved')).toBeInTheDocument();
  });

  it('keeps the server value on save failure and offers an idempotent retry', async () => {
    apiMock.updateProductConversationAutomaticContinuation
      .mockRejectedValueOnce(new Error('Network unavailable'))
      .mockResolvedValueOnce(view({ auto_continue_on_context_exhaustion: true }));
    render(<AutomaticContinuationControl scope={{ kind: 'ordinary', reference: 'pc-1' }} />);

    const checkbox = await openControl();
    fireEvent.click(checkbox);
    expect(await screen.findByRole('alert')).toHaveTextContent('Network unavailable');
    expect(checkbox).not.toBeChecked();

    fireEvent.click(screen.getByRole('button', { name: 'Retry save' }));
    await waitFor(() => expect(apiMock.updateProductConversationAutomaticContinuation).toHaveBeenCalledTimes(2));
    expect(await screen.findByText('Saved')).toBeInTheDocument();
    expect(checkbox).toBeChecked();
  });

  it('does not let an older poll overwrite a newer successful save', async () => {
    vi.useFakeTimers();
    let resolvePoll: ((next: AutomaticContinuationView) => void) | undefined;
    apiMock.getProductConversationAutomaticContinuation
      .mockResolvedValueOnce(view({ auto_continue_on_context_exhaustion: false }))
      .mockImplementationOnce(() => new Promise((resolve) => { resolvePoll = resolve; }));
    apiMock.updateProductConversationAutomaticContinuation.mockResolvedValueOnce(
      view({ auto_continue_on_context_exhaustion: true }),
    );
    render(<AutomaticContinuationControl scope={{ kind: 'ordinary', reference: 'pc-1' }} />);

    await act(async () => { await Promise.resolve(); });
    const checkbox = screen.getByRole('checkbox', { name: /automatically accept future generated handoffs/i });
    await act(async () => { await vi.advanceTimersByTimeAsync(5_000); });
    fireEvent.click(checkbox);
    await act(async () => { await Promise.resolve(); });
    expect(checkbox).toBeChecked();

    await act(async () => {
      resolvePoll?.(view({ auto_continue_on_context_exhaustion: false }));
      await Promise.resolve();
    });
    expect(checkbox).toBeChecked();
  });

  it('renders a manual race winner as a terminal superseded admission', async () => {
    apiMock.getProductConversationAutomaticContinuation.mockResolvedValueOnce(view({
      admission: {
        predecessor_transcript_row_id: 'row-exhausted',
        phase: 'superseded',
        no_progress_attempts: 0,
        actionable_failure: null,
      },
    }));
    render(<AutomaticContinuationControl scope={{ kind: 'ordinary', reference: 'pc-1' }} />);

    expect(await screen.findByText('Continued manually', { selector: 'strong' })).toBeInTheDocument();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('refreshes durable progress and exposes a later breaker failure without remounting', async () => {
    vi.useFakeTimers();
    apiMock.getProductConversationAutomaticContinuation
      .mockResolvedValueOnce(view({
        auto_continue_on_context_exhaustion: true,
        admission: {
          predecessor_transcript_row_id: 'row-exhausted',
          phase: 'admitted',
          no_progress_attempts: 0,
          actionable_failure: null,
        },
      }))
      .mockResolvedValueOnce(view({
        auto_continue_on_context_exhaustion: true,
        admission: {
          predecessor_transcript_row_id: 'row-exhausted',
          phase: 'failed',
          no_progress_attempts: 5,
          actionable_failure: {
            message: 'Successor dispatch could not be accepted.',
            first_message_id: 'automatic-first-message',
          },
        },
      }));
    render(<AutomaticContinuationControl scope={{ kind: 'ordinary', reference: 'pc-1' }} />);

    await act(async () => { await Promise.resolve(); });
    expect(screen.getByText('Admitted', { selector: 'strong' })).toBeInTheDocument();
    await act(async () => { await vi.advanceTimersByTimeAsync(5_000); });
    expect(screen.getByRole('alert')).toHaveTextContent('Successor dispatch could not be accepted.');
  });

  it('shows failed admission status with manual generated-handoff recovery guidance', async () => {
    apiMock.getProductConversationAutomaticContinuation.mockResolvedValueOnce(view({
      auto_continue_on_context_exhaustion: true,
      admission: {
        predecessor_transcript_row_id: 'row-exhausted',
        phase: 'failed',
        no_progress_attempts: 5,
        actionable_failure: {
          message: 'Successor dispatch could not be accepted.',
          first_message_id: 'automatic-first-message',
        },
      },
    }));
    render(<AutomaticContinuationControl scope={{ kind: 'ordinary', reference: 'pc-1' }} />);

    const control = await screen.findByTestId('automatic-continuation-control');
    const alert = await screen.findByRole('alert');
    expect(control).toHaveAttribute('open');
    expect(alert).toHaveTextContent('Successor dispatch could not be accepted.');
    expect(alert).toHaveTextContent('existing Continue control on the generated handoff');
    expect(alert).toHaveTextContent('remains enabled for future exhaustions');
    expect(screen.getByText(/5 no-progress attempts/)).toBeInTheDocument();
  });
});
