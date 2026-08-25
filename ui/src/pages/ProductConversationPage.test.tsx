import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { ProductConversationPage } from './ProductConversationPage';
import { ConversationReadinessProvider } from '../contexts/ConversationReadinessContext';
import type { ChainView, ProductConversationSnapshotView } from '../api';

const conversationNavStackSpy = vi.fn();
const embeddedConversationPageSpy = vi.fn();
const chainQaColumnSpy = vi.fn();
const chainWorkScopeDockSpy = vi.fn();

vi.mock('../components/ConversationNavStack', () => ({
  ConversationNavStack: (props: Record<string, unknown>) => {
    conversationNavStackSpy(props);
    const messages = Array.isArray(props['messages']) ? props['messages'] as Array<{
      message_id: string;
      message_type: string;
      content?: { text?: string } | unknown;
    }> : [];
    return (
      <div>
        <button onClick={() => (props['onLoadOlderMessages'] as (() => void) | undefined)?.()}>
          load older
        </button>
        <div data-testid="message-count">{messages.length}</div>
        <div data-testid="message-order">{messages.map((m) => m.message_id).join(',')}</div>
        <div data-testid="message-types">{messages.map((m) => m.message_type).join(',')}</div>
        <div data-testid="message-text-order">{messages.map((m) => typeof m.content === 'object' && m.content !== null && 'text' in m.content ? String((m.content as { text?: string }).text ?? '') : '').join('|')}</div>
      </div>
    );
  },
}));

vi.mock('./ConversationPage', () => ({
  EmbeddedConversationPage: ({ slug }: { slug: string }) => {
    embeddedConversationPageSpy(slug);
    return <div data-testid="embedded-conversation-page">embedded {slug}</div>;
  },
}));

vi.mock('./ChainPage', () => ({
  ChainQaColumn: (props: Record<string, unknown>) => {
    chainQaColumnSpy(props);
    return (
      <div data-testid="chain-qa-column">
        <div data-testid="chain-qa-persisted-count">{Array.isArray(props['persisted']) ? props['persisted'].length : 0}</div>
        <button onClick={() => (props['onRetryConnection'] as (() => void) | undefined)?.()}>retry chain</button>
      </div>
    );
  },
  ChainWorkScopeDock: (props: Record<string, unknown>) => {
    chainWorkScopeDockSpy(props);
    return <div data-testid="chain-work-scope-dock">dock</div>;
  },
}));

vi.mock('../components/Skeleton', () => ({
  MessageListSkeleton: ({ count }: { count: number }) => <div data-testid="skeleton">skeleton {count}</div>,
}));

const { closeMock, subscribeToChainStreamMock } = vi.hoisted(() => {
  const close = vi.fn();
  return {
    closeMock: close,
    subscribeToChainStreamMock: vi.fn(() => ({ close })),
  };
});

vi.mock('../api', async () => {
  const actual = await vi.importActual<typeof import('../api')>('../api');
  return {
    ...actual,
    api: {
      ...actual.api,
      getProductConversationSnapshot: vi.fn(),
      getChain: vi.fn(),
      submitChainQuestion: vi.fn(),
    },
    streamApi: {
      ...actual.streamApi,
      subscribeToChainStream: subscribeToChainStreamMock,
    },
  };
});

function makeMessage(message_id: string, sequence_id: number, conversation_id = 'conv-a') {
  return {
    message_id,
    conversation_id,
    sequence_id,
    message_type: sequence_id % 2 === 0 ? 'agent' as const : 'user' as const,
    content: { text: message_id },
    display_data: null,
    usage_data: null,
    created_at: `2026-01-01T00:00:${String(sequence_id).padStart(2, '0')}Z`,
  };
}

function makeSnapshot(overrides: Partial<ProductConversationSnapshotView> = {}): ProductConversationSnapshotView {
  return {
    product_conversation_id: 'pc-1',
    canonical_route: '/product-conversations/pc-1',
    requested_transcript_row_id: 'row-2',
    canonical_root: { transcript_row_id: 'row-1', slug: 'root-slug', title: 'Root title' },
    ordinary_lifecycle: 'open',
    latest_transcript_row_id: 'row-2',
    writable_transcript_row_id: 'row-2',
    updated_at: '2026-01-01T00:00:00Z',
    presentation: { kind: 'state', display_name: 'Product Alpha', presentation_mode: 'idle' },
    work_identity: {
      work_transcript_row_id: 'row-2',
      worktree_path: '/tmp/worktree',
      branch_name: 'feature/test',
      base_branch: 'main',
      task_id: '40012',
      task_title: 'Product foundation',
    },
    source: {
      status: 'present',
      source_product_conversation_id: 'pc-source',
      source_conversation_id: 'conv-source',
      relation: 'approved_task',
      relation_key: 'task-40012',
    },
    chain_qa_compatibility: { root_transcript_row_id: 'root-chain', url: '/chains/root-chain' },
    segments: [
      {
        segment_ordinal: 1,
        transcript_row_id: 'row-1',
        slug: 'row-1',
        title: 'Earlier',
        messages: [makeMessage('m-1', 1), makeMessage('m-2', 2)],
        handoff: {
          kind: 'historical',
          predecessor_transcript_row_id: 'row-0',
          successor_transcript_row_id: 'row-1',
          continuation_message_id: 'cont-1',
          summary: 'First handoff',
        },
      },
      {
        segment_ordinal: 2,
        transcript_row_id: 'row-2',
        slug: 'row-2',
        title: 'Latest',
        messages: [makeMessage('m-3', 3), makeMessage('m-4', 4)],
        handoff: {
          kind: 'historical',
          predecessor_transcript_row_id: 'row-1',
          successor_transcript_row_id: 'row-2',
          continuation_message_id: 'cont-2',
          summary: 'Second handoff should not repeat',
        },
      },
    ],
    before: 'cursor-1',
    has_older: true,
    ...overrides,
  };
}

function makeChain(overrides: Partial<ChainView> = {}): ChainView {
  return {
    root_conv_id: 'root-chain',
    chain_name: null,
    display_name: 'Product Alpha',
    archived: false,
    members: [],
    qa_history: [
      {
        id: 'qa-1',
        root_conv_id: 'root-chain',
        question: 'What changed?',
        answer: 'A lot.',
        model: 'gpt-5',
        status: 'completed',
        chain_members_at_answer: 2,
        chain_messages_at_answer: 4,
        created_at: '2026-01-01T00:00:10Z',
        completed_at: '2026-01-01T00:00:20Z',
      },
    ],
    current_member_count: 2,
    current_total_messages: 4,
    work_identity: {
      work_conv_id: 'row-2',
      worktree_path: '/tmp/worktree',
      branch_name: 'feature/test',
      base_branch: 'main',
      task_id: '40012',
      task_title: 'Product foundation',
    },
    ...overrides,
  };
}

function renderPage() {
  return render(
    <ConversationReadinessProvider>
      <MemoryRouter initialEntries={['/product-conversations/pc-1']}>
        <Routes>
          <Route path="/product-conversations/:productConversationId" element={<ProductConversationPage />} />
        </Routes>
      </MemoryRouter>
    </ConversationReadinessProvider>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  conversationNavStackSpy.mockClear();
  embeddedConversationPageSpy.mockClear();
  chainQaColumnSpy.mockClear();
  chainWorkScopeDockSpy.mockClear();
  closeMock.mockClear();
});

afterEach(() => {
  cleanup();
});

async function waitForPageReady() {
  await screen.findByTestId('message-order');
  await waitFor(() => {
    expect(chainQaColumnSpy).toHaveBeenCalled();
  });
}

describe('ProductConversationPage', () => {
  beforeEach(async () => {
    const { api } = await import('../api');
    vi.mocked(api.getProductConversationSnapshot).mockReset();
    vi.mocked(api.getProductConversationSnapshot).mockResolvedValue(makeSnapshot());
    vi.mocked(api.getChain).mockReset();
    vi.mocked(api.getChain).mockResolvedValue(makeChain());
  });

  it('orders flattened segment messages chronologically', async () => {
    const { api } = await import('../api');
    vi.mocked(api.getProductConversationSnapshot).mockResolvedValueOnce(makeSnapshot({
      segments: [
        { segment_ordinal: 2, transcript_row_id: 'row-2', slug: null, title: null, messages: [makeMessage('m-3', 3), makeMessage('m-4', 4)], handoff: null },
        { segment_ordinal: 1, transcript_row_id: 'row-1', slug: null, title: null, messages: [makeMessage('m-1', 1), makeMessage('m-2', 2)], handoff: null },
      ],
    }));

    renderPage();

    await waitFor(() => {
      expect(screen.getByTestId('message-order').textContent).toBe('m-1,m-2,m-3,m-4');
    });
  });

  it('renders every typed historical handoff exactly once at the segment boundary', async () => {
    const { api } = await import('../api');
    vi.mocked(api.getProductConversationSnapshot).mockResolvedValueOnce(makeSnapshot({
      segments: [
        {
          segment_ordinal: 3,
          transcript_row_id: 'row-3',
          slug: 'row-3',
          title: 'Latest',
          messages: [makeMessage('m-5', 5, 'conv-c'), makeMessage('m-6', 6, 'conv-c')],
          handoff: {
            kind: 'historical',
            predecessor_transcript_row_id: 'row-2',
            successor_transcript_row_id: 'row-3',
            continuation_message_id: 'cont-3',
            summary: 'Third handoff',
          },
        },
        {
          segment_ordinal: 1,
          transcript_row_id: 'row-1',
          slug: 'row-1',
          title: 'Oldest',
          messages: [makeMessage('m-1', 1, 'conv-a'), makeMessage('m-2', 2, 'conv-a')],
          handoff: {
            kind: 'historical',
            predecessor_transcript_row_id: 'row-0',
            successor_transcript_row_id: 'row-1',
            continuation_message_id: 'cont-1',
            summary: 'First handoff',
          },
        },
        {
          segment_ordinal: 2,
          transcript_row_id: 'row-2',
          slug: 'row-2',
          title: 'Middle',
          messages: [makeMessage('m-3', 3, 'conv-b'), makeMessage('m-4', 4, 'conv-b')],
          handoff: {
            kind: 'historical',
            predecessor_transcript_row_id: 'row-1',
            successor_transcript_row_id: 'row-2',
            continuation_message_id: 'cont-2',
            summary: 'Second handoff',
          },
        },
      ],
    }));

    renderPage();

    await waitFor(() => {
      expect(screen.getByTestId('message-order').textContent).toBe(
        'product-handoff:pc-1:row-1:cont-1,m-1,m-2,product-handoff:pc-1:row-2:cont-2,m-3,m-4,product-handoff:pc-1:row-3:cont-3,m-5,m-6'
      );
    });
    expect(screen.getByTestId('message-types').textContent).toBe('system,user,agent,system,user,agent,system,user,agent');
    expect(screen.getByTestId('message-text-order').textContent).toContain('First handoff');
    expect(screen.getByTestId('message-text-order').textContent).toContain('Second handoff');
    expect(screen.getByTestId('message-text-order').textContent).toContain('Third handoff');
    const messages = conversationNavStackSpy.mock.lastCall?.[0]?.['messages'] as Array<{ message_id: string }>;
    expect(messages.filter((message) => message.message_id.includes('product-handoff:'))).toHaveLength(3);
  });

  it('keeps exact order across 100+ messages spanning multiple paginated segments', async () => {
    const { api } = await import('../api');
    const segment0Messages = Array.from({ length: 35 }, (_, index) => makeMessage(`m-${index + 1}`, index + 1, 'conv-0'));
    const segment1Messages = Array.from({ length: 35 }, (_, index) => makeMessage(`m-${index + 36}`, index + 36, 'conv-1'));
    const segment2Messages = Array.from({ length: 40 }, (_, index) => makeMessage(`m-${index + 71}`, index + 71, 'conv-2'));
    vi.mocked(api.getProductConversationSnapshot)
      .mockResolvedValueOnce(makeSnapshot({
        segments: [
          {
            segment_ordinal: 2,
            transcript_row_id: 'row-2',
            slug: 'row-2',
            title: 'Middle',
            messages: segment1Messages,
            handoff: {
              kind: 'historical',
              predecessor_transcript_row_id: 'row-0',
              successor_transcript_row_id: 'row-2',
              continuation_message_id: 'cont-2',
              summary: 'Middle handoff',
            },
          },
          {
            segment_ordinal: 3,
            transcript_row_id: 'row-3',
            slug: 'row-3',
            title: 'Latest',
            messages: segment2Messages,
            handoff: {
              kind: 'historical',
              predecessor_transcript_row_id: 'row-2',
              successor_transcript_row_id: 'row-3',
              continuation_message_id: 'cont-3',
              summary: 'Latest handoff',
            },
          },
        ],
        before: 'cursor-many',
        has_older: true,
      }))
      .mockResolvedValueOnce(makeSnapshot({
        segments: [
          {
            segment_ordinal: 1,
            transcript_row_id: 'row-1',
            slug: 'row-1',
            title: 'Oldest',
            messages: segment0Messages,
            handoff: {
              kind: 'historical',
              predecessor_transcript_row_id: 'row--1',
              successor_transcript_row_id: 'row-1',
              continuation_message_id: 'cont-1',
              summary: 'Oldest handoff',
            },
          },
          {
            segment_ordinal: 2,
            transcript_row_id: 'row-2',
            slug: 'row-2',
            title: 'Middle',
            messages: segment1Messages,
            handoff: {
              kind: 'historical',
              predecessor_transcript_row_id: 'row-0',
              successor_transcript_row_id: 'row-2',
              continuation_message_id: 'cont-2',
              summary: 'Middle handoff',
            },
          },
          {
            segment_ordinal: 3,
            transcript_row_id: 'row-3',
            slug: 'row-3',
            title: 'Latest',
            messages: segment2Messages,
            handoff: {
              kind: 'historical',
              predecessor_transcript_row_id: 'row-2',
              successor_transcript_row_id: 'row-3',
              continuation_message_id: 'cont-3',
              summary: 'Latest handoff',
            },
          },
        ],
        before: null,
        has_older: false,
      }));

    renderPage();
    await waitForPageReady();
    fireEvent.click(screen.getByRole('button', { name: 'load older' }));

    await waitFor(() => {
      expect(screen.getByTestId('message-count')).toHaveTextContent('113');
    });
    const order = screen.getByTestId('message-order').textContent?.split(',') ?? [];
    expect(order[0]).toBe('product-handoff:pc-1:row-1:cont-1');
    expect(order[1]).toBe('m-1');
    expect(order[36]).toBe('product-handoff:pc-1:row-2:cont-2');
    expect(order[72]).toBe('product-handoff:pc-1:row-3:cont-3');
    expect(order.at(-1)).toBe('m-110');
    expect(new Set(order).size).toBe(order.length);
  });

  it('shows loading then request error state', async () => {
    const { api } = await import('../api');
    vi.mocked(api.getProductConversationSnapshot).mockRejectedValueOnce(new Error('boom'));

    renderPage();

    expect(screen.getByTestId('skeleton')).toBeInTheDocument();
    expect(await screen.findByRole('alert')).toHaveTextContent('boom');
  });

  it('surfaces pagination failure through aggregated status', async () => {
    const { api } = await import('../api');
    vi.mocked(api.getProductConversationSnapshot)
      .mockResolvedValueOnce(makeSnapshot())
      .mockRejectedValueOnce(new Error('older failed'));

    renderPage();
    await waitForPageReady();

    fireEvent.click(screen.getByRole('button', { name: 'load older' }));

    await waitFor(() => {
      expect(screen.getByText('older failed')).toBeInTheDocument();
    });
  });

  it('shows shared chain qa and embeds the writable conversation composer', async () => {
    const { api } = await import('../api');
    vi.mocked(api.getProductConversationSnapshot).mockResolvedValueOnce(makeSnapshot());

    renderPage();

    await waitForPageReady();
    expect(screen.getByTestId('chain-qa-column')).toBeInTheDocument();
    expect(chainQaColumnSpy).toHaveBeenLastCalledWith(expect.objectContaining({
      persisted: expect.arrayContaining([expect.objectContaining({ id: 'qa-1' })]),
    }));
    expect(screen.getByTestId('embedded-conversation-page')).toHaveTextContent('embedded row-2');
    expect(embeddedConversationPageSpy).toHaveBeenCalledWith('row-2');
    expect(screen.getByTestId('chain-work-scope-dock')).toBeInTheDocument();
    expect(chainWorkScopeDockSpy).toHaveBeenLastCalledWith(expect.objectContaining({
      activeConvId: 'row-2',
    }));
  });

  it('omits composer for history-only snapshots', async () => {
    const { api } = await import('../api');
    vi.mocked(api.getProductConversationSnapshot).mockResolvedValueOnce(makeSnapshot({
      writable_transcript_row_id: null,
      ordinary_lifecycle: 'history',
    }));

    renderPage();

    await waitForPageReady();
    expect(screen.getByTestId('chain-qa-column')).toBeInTheDocument();
    expect(screen.queryByTestId('embedded-conversation-page')).not.toBeInTheDocument();
    expect(screen.getByText('History is read-only.')).toBeInTheDocument();
  });

  it('subscribes to chain streaming with compatibility root id', async () => {
    const { api } = await import('../api');
    vi.mocked(api.getProductConversationSnapshot).mockResolvedValueOnce(makeSnapshot());

    renderPage();

    await waitForPageReady();
    expect(subscribeToChainStreamMock).toHaveBeenCalledWith('root-chain', expect.any(Function), expect.any(Function));
  });
});
