import { createRef } from 'react';
import { describe, it, expect, vi } from 'vitest';
import { render, fireEvent } from '@testing-library/react';
import type { ChainView, ChainMemberSummary, ChainQaRow } from '../api';
import { ChainQaColumn } from './ChainQaColumn';

const ROOT_ID = 'root-1';

const makeMember = (
  conv_id: string,
  position: ChainMemberSummary['position'],
  overrides: Partial<ChainMemberSummary> = {},
): ChainMemberSummary => ({
  conv_id,
  slug: `slug-${conv_id}`,
  title: `Title ${conv_id}`,
  message_count: 12,
  updated_at: '2026-04-29T12:00:00Z',
  position,
  has_worktree: false,
  ...overrides,
});

const makeQa = (
  id: string,
  overrides: Partial<ChainQaRow> = {},
): ChainQaRow => ({
  id,
  root_conv_id: ROOT_ID,
  question: `Q ${id}`,
  answer: `A ${id}`,
  model: 'sonnet-test',
  status: 'completed',
  chain_members_at_answer: 2,
  chain_messages_at_answer: 20,
  created_at: '2026-04-28T10:00:00Z',
  completed_at: '2026-04-28T10:00:30Z',
  ...overrides,
});

const makeChain = (overrides: Partial<ChainView> = {}): ChainView => ({
  root_conv_id: ROOT_ID,
  product_conversation_id: 'pc-chain',
  chain_name: 'auth refactor',
  display_name: 'auth refactor',
  archived: false,
  members: [
    makeMember('m1', 'root'),
    makeMember('m2', 'continuation'),
    makeMember('m3', 'latest'),
  ],
  qa_history: [],
  current_member_count: 3,
  current_total_messages: 36,
  work_identity: null,
  ...overrides,
});

function renderQa(overrides: Partial<Parameters<typeof ChainQaColumn>[0]> = {}) {
  const props: Parameters<typeof ChainQaColumn>[0] = {
    chain: makeChain(), persisted: [], inflight: [], draft: '', setDraft: vi.fn(),
    submitting: false, sseLost: false, onSubmit: vi.fn((event) => event.preventDefault()),
    onReask: vi.fn(), activeTextareaRef: createRef<HTMLTextAreaElement>(),
    onRetryConnection: vi.fn(), autoFocusActive: false, ...overrides,
  };
  return { ...render(<ChainQaColumn {...props} />), props };
}

describe('ChainQaColumn', () => {
  it('renders persisted question and answer with staleness independently', () => {
    const { getByText } = renderQa({ persisted: [makeQa('qa-1', { question: 'What does this chain do?', answer: 'It does a thing.' })] });
    expect(getByText('What does this chain do?')).toBeInTheDocument();
    expect(getByText('It does a thing.')).toBeInTheDocument();
    expect(getByText(/answered when chain had 2 conversations/)).toBeInTheDocument();
  });

  it('renders newest persisted cards first below the active input', () => {
    const { container } = renderQa({ persisted: [
      makeQa('old', { question: 'Old question' }),
      makeQa('new', { question: 'New question' }),
    ] });
    expect([...container.querySelectorAll('.chain-qa-pair-question')].map((node) => node.textContent))
      .toEqual(['New question', 'Old question']);
  });

  it('renders pre-token, streaming and failed inflight answers without a management page', () => {
    const { getByText, props } = renderQa({ inflight: [
      { chainQaId: 'pre', question: 'Waiting question', answer: '', preToken: true, error: null },
      { chainQaId: 'stream', question: 'Streaming question', answer: 'Live answer', preToken: false, error: null },
      { chainQaId: 'failed', question: 'Failed question', answer: '', preToken: false, error: 'Stream unavailable' },
    ] });
    expect(getByText('Waiting question')).toBeInTheDocument();
    expect(getByText('Live answer')).toBeInTheDocument();
    expect(getByText(/Failed: Stream unavailable/)).toBeInTheDocument();
    fireEvent.click(getByText(/Re-ask/));
    expect(props.onReask).toHaveBeenCalledWith('Failed question');
  });

  it('keeps the active controlled draft and delegates submit', () => {
    const { getByRole, container, props } = renderQa({ draft: 'Current question' });
    const input = getByRole('textbox');
    expect(input).toHaveValue('Current question');
    fireEvent.change(input, { target: { value: 'Edited question' } });
    expect(props.setDraft).toHaveBeenCalledWith('Edited question');
    fireEvent.submit(container.querySelector('form')!);
    expect(props.onSubmit).toHaveBeenCalledTimes(1);
  });

  it('disables asking and re-asking in read-only Recall', () => {
    const { getByRole } = renderQa({ disabled: true, draft: 'Question',
      persisted: [makeQa('failed', { status: 'failed' })],
    });
    expect(getByRole('textbox')).toBeDisabled();
    expect(getByRole('button', { name: 'Ask' })).toBeDisabled();
    expect(getByRole('button', { name: /Re-ask/ })).toBeDisabled();
  });

  it('retains the editable draft on lost SSE and delegates reconnect', () => {
    const { getByRole, props } = renderQa({ sseLost: true, draft: 'Retained question' });
    expect(getByRole('textbox')).toBeEnabled();
    expect(getByRole('textbox')).toHaveValue('Retained question');
    expect(getByRole('status')).toHaveTextContent('Connection lost');
    fireEvent.click(getByRole('button', { name: 'Retry' }));
    expect(props.onRetryConnection).toHaveBeenCalledTimes(1);
  });
});
