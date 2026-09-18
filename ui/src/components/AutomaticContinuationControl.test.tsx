import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
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
  return screen.findByRole('checkbox', { name: 'Always accept generated handoffs and continue' });
}

describe('AutomaticContinuationControl', () => {
  beforeEach(() => {
    vi.clearAllMocks();
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

  it('defaults an ordinary ProductConversation OFF from GET and immediately PUTs the checked value', async () => {
    render(<AutomaticContinuationControl scope={{ kind: 'ordinary', reference: 'pc/1' }} />);

    const checkbox = await openControl();
    expect(checkbox).not.toBeChecked();
    expect(apiMock.getProductConversationAutomaticContinuation).toHaveBeenCalledWith('pc/1');
    expect(screen.getByText(/Applies only the next time this conversation reaches context exhaustion/)).toBeInTheDocument();
    expect(screen.getByText(/will not continue an already-exhausted conversation/)).toBeInTheDocument();

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

    await openControl();
    const alert = screen.getByRole('alert');
    expect(alert).toHaveTextContent('Successor dispatch could not be accepted.');
    expect(alert).toHaveTextContent('existing Continue control on the generated handoff');
    expect(alert).toHaveTextContent('remains enabled for future exhaustions');
    expect(screen.getByText(/5 no-progress attempts/)).toBeInTheDocument();
  });
});
