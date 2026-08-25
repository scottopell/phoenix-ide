import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter, Route, Routes, useNavigate } from 'react-router-dom';
import { ProductConversationPage } from './ProductConversationPage';
import { ConversationReadinessProvider } from '../contexts/ConversationReadinessContext';
import { FileExplorerProvider } from '../components/FileExplorer';
import { ViewerSlotProvider } from '../contexts/ViewerSlotContext';
import { ChainProvider } from '../chain';
import { ApiResponseError, type ChainView, type ProductConversationSnapshotView } from '../api';

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
        <div data-testid="message-sidepanel-enabled">{String(props['enableMessageSidepanel'])}</div>
        <div data-testid="message-fullscreen-enabled">{String(props['enableMessageFullscreen'])}</div>
        <button onClick={() => (props['onLoadOlderMessages'] as ((basis?: unknown) => void) | undefined)?.()}>
          load older
        </button>
        <button onClick={() => (props['onLoadOlderMessages'] as ((basis?: unknown) => void) | undefined)?.({ kind: 'reader_anchor', messageId: 'm-3', viewportStartOffset: 17 })}>
          load older with anchor
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
  EmbeddedConversationPage: (props: Record<string, unknown>) => {
    embeddedConversationPageSpy(props);
    return (
      <div data-testid="embedded-conversation-page">
        embedded {String(props['slug'])}
        <button onClick={() => window.dispatchEvent(new CustomEvent('phoenix:open-message-viewer', {
          detail: { sequenceId: 3, messageId: 'm-3', presentation: 'pane' },
        }))}>
          open aggregate viewer
        </button>
      </div>
    );
  },
}));

vi.mock('./ChainPage', () => ({
  ChainQaColumn: (props: Record<string, unknown>) => {
    chainQaColumnSpy(props);
    return (
      <div data-testid="chain-qa-column">
        <div data-testid="chain-qa-persisted-count">{Array.isArray(props['persisted']) ? props['persisted'].length : 0}</div>
        <button onClick={() => (props['onRetryConnection'] as (() => void) | undefined)?.()}>retry chain</button>
        <textarea
          aria-label="recall draft"
          value={String(props['draft'] ?? '')}
          onChange={(event) => (props['setDraft'] as ((value: string) => void))(event.target.value)}
        />
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

function NavigateToSecondProduct() {
  const navigate = useNavigate();
  return <button onClick={() => navigate('/product-conversations/pc-2')}>open second product</button>;
}

function renderPage(initialEntry = '/product-conversations/pc-1', withRouteSwitch = false) {
  return render(
    <ConversationReadinessProvider>
      <MemoryRouter initialEntries={[initialEntry]}>
        <ViewerSlotProvider browserSessionActive={false}>
          <ChainProvider>
            <FileExplorerProvider>
            {withRouteSwitch && <NavigateToSecondProduct />}
            <Routes>
              <Route path="/product-conversations/:productConversationId" element={<ProductConversationPage />} />
            </Routes>
            </FileExplorerProvider>
          </ChainProvider>
        </ViewerSlotProvider>
      </MemoryRouter>
    </ConversationReadinessProvider>,
  );
}

function emitLatestProjection(overrides: Partial<Record<string, unknown>> = {}) {
  const props = embeddedConversationPageSpy.mock.lastCall?.[0] as Record<string, unknown> | undefined;
  const onProjectionChange = props?.['onProjectionChange'] as ((projection: Record<string, unknown> | null) => void) | undefined;
  if (!onProjectionChange) throw new Error('missing onProjectionChange');
  onProjectionChange({
    slug: 'row-2',
    conversationId: 'conv-latest',
    conversation: { work_scope_key: 'ws-live' },
    messages: [makeMessage('live-sent', 5, 'conv-latest'), makeMessage('live-streamed', 6, 'conv-latest')],
    pendingMessages: [{
      localId: 'pending-local',
      text: 'pending-local',
      images: [],
      status: 'pending',
    }],
    convState: { type: 'awaiting_user_response', questions: [] },
    isArchived: false,
    onRetryPending: vi.fn(),
    onCancelSteering: vi.fn(),
    onOpenFile: vi.fn(),
    filePathRootDir: '/tmp/latest-root',
    systemPrompt: 'Preserve this system prompt',
    ...overrides,
  });
}

beforeEach(() => {
  vi.clearAllMocks();
  localStorage.clear();
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
      latest_transcript_row_id: 'row-3',
      writable_transcript_row_id: 'row-3',
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
        'm-1,m-2,product-handoff:pc-1:row-1:cont-1,m-3,m-4,product-handoff:pc-1:row-2:cont-2,m-5,m-6,product-handoff:pc-1:row-3:cont-3'
      );
    });
    expect(screen.getByTestId('message-types').textContent).toBe('user,agent,system,user,agent,system,user,agent,system');
    expect(screen.getByTestId('message-text-order').textContent).toContain('First handoff');
    expect(screen.getByTestId('message-text-order').textContent).toContain('Second handoff');
    expect(screen.getByTestId('message-text-order').textContent).toContain('Third handoff');
    const messages = conversationNavStackSpy.mock.lastCall?.[0]?.['messages'] as Array<{ message_id: string }>;
    expect(messages.filter((message) => message.message_id.includes('product-handoff:'))).toHaveLength(3);
  });

  it('renders completed handoffs as the single typed segment boundary', async () => {
    const { api } = await import('../api');
    vi.mocked(api.getProductConversationSnapshot).mockResolvedValueOnce(makeSnapshot({
      segments: [
        {
          segment_ordinal: 1,
          transcript_row_id: 'row-1',
          slug: 'row-1',
          title: 'Earlier',
          messages: [makeMessage('m-1', 1), makeMessage('m-2', 2)],
          handoff: null,
        },
        {
          segment_ordinal: 2,
          transcript_row_id: 'row-2',
          slug: 'row-2',
          title: 'Latest',
          messages: [makeMessage('accepted-successor', 4)],
          handoff: {
            kind: 'completed',
            predecessor_transcript_row_id: 'row-1',
            successor_transcript_row_id: 'row-2',
            continuation_message_id: 'continuation-request',
            accepted_successor_message_id: 'accepted-successor',
            summary: 'Completed handoff',
          },
        },
      ],
    }));

    renderPage();

    await waitFor(() => {
      expect(screen.getByTestId('message-order').textContent).toBe(
        'm-1,m-2,accepted-successor,product-handoff:pc-1:row-2:continuation-request'
      );
    });
    expect(screen.getByTestId('message-text-order')).toHaveTextContent('Completed handoff');
  });

  it('keeps exact order across 100+ messages spanning multiple paginated segments', async () => {
    const { api } = await import('../api');
    const segment0Messages = Array.from({ length: 35 }, (_, index) => makeMessage(`m-${index + 1}`, index + 1, 'conv-0'));
    const segment1Messages = Array.from({ length: 35 }, (_, index) => makeMessage(`m-${index + 36}`, index + 36, 'conv-1'));
    const segment2Messages = Array.from({ length: 40 }, (_, index) => makeMessage(`m-${index + 71}`, index + 71, 'conv-2'));
    vi.mocked(api.getProductConversationSnapshot)
      .mockResolvedValueOnce(makeSnapshot({
        latest_transcript_row_id: 'row-3',
        writable_transcript_row_id: 'row-3',
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
        latest_transcript_row_id: 'row-3',
        writable_transcript_row_id: 'row-3',
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
    expect(order[0]).toBe('m-1');
    expect(order[35]).toBe('product-handoff:pc-1:row-1:cont-1');
    expect(order[71]).toBe('product-handoff:pc-1:row-2:cont-2');
    expect(order[111]).toBe('m-110');
    expect(order.at(-1)).toBe('product-handoff:pc-1:row-3:cont-3');
    expect(new Set(order).size).toBe(order.length);
  });

  it('shows loading then request error state', async () => {
    const { api } = await import('../api');
    vi.mocked(api.getProductConversationSnapshot).mockRejectedValueOnce(new Error('boom'));

    renderPage();

    expect(screen.getByTestId('skeleton')).toBeInTheDocument();
    expect(await screen.findByRole('alert')).toHaveTextContent('boom');
  });

  it('keeps chain freshness counts in sync with live aggregate growth', async () => {
    renderPage();
    await waitForPageReady();

    emitLatestProjection({
      messages: [
        makeMessage('m-3', 3),
        makeMessage('m-4', 4),
        makeMessage('live-sent', 5),
        makeMessage('live-streamed', 6),
      ],
    });

    await waitFor(() => {
      expect(chainQaColumnSpy).toHaveBeenCalled();
    });
    const chain = chainQaColumnSpy.mock.lastCall?.[0]?.['chain'] as ChainView;
    expect(chain.current_member_count).toBe(2);
    expect(chain.current_total_messages).toBe(8);
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
    expect(embeddedConversationPageSpy).toHaveBeenCalledWith(expect.objectContaining({
      slug: 'row-2',
      suppressCanonicalization: true,
      ordinaryComposerEnabled: true,
    }));
    expect(screen.getByTestId('chain-work-scope-dock')).toBeInTheDocument();
    expect(chainWorkScopeDockSpy).toHaveBeenLastCalledWith(expect.objectContaining({
      activeConvId: 'row-2',
    }));
  });

  it('keeps latest open row mounted even when writable transcript is null', async () => {
    const { api } = await import('../api');
    vi.mocked(api.getProductConversationSnapshot).mockResolvedValueOnce(makeSnapshot({
      writable_transcript_row_id: null,
      chain_qa_compatibility: null,
    }));

    renderPage();

    await screen.findByTestId('embedded-conversation-page');
    expect(embeddedConversationPageSpy).toHaveBeenCalledWith(expect.objectContaining({
      slug: 'row-2',
      ordinaryComposerEnabled: true,
      suppressCanonicalization: true,
    }));
  });

  it('re-enables the ordinary composer from live latest projection after approval ends', async () => {
    renderPage();
    await waitForPageReady();

    emitLatestProjection({ convState: { type: 'awaiting_task_approval' } });
    await waitFor(() => {
      expect(embeddedConversationPageSpy).toHaveBeenLastCalledWith(expect.objectContaining({
        ordinaryComposerEnabled: true,
      }));
    });

    emitLatestProjection({ convState: { type: 'idle' } });
    await waitFor(() => {
      expect(embeddedConversationPageSpy).toHaveBeenLastCalledWith(expect.objectContaining({
        ordinaryComposerEnabled: true,
      }));
    });
  });

  it('projects persisted and pending latest messages through distinct aggregate inputs', async () => {
    renderPage();
    await waitForPageReady();

    emitLatestProjection();

    await waitFor(() => {
      expect(screen.getByTestId('message-order').textContent).toBe(
        'm-1,m-2,product-handoff:pc-1:row-1:cont-1,m-3,m-4,live-sent,live-streamed,product-handoff:pc-1:row-2:cont-2'
      );
      expect(conversationNavStackSpy.mock.lastCall?.[0]?.['pendingMessages']).toEqual([
        expect.objectContaining({ localId: 'pending-local', status: 'pending' }),
      ]);
    });
  });

  it('lets projected latest state drive aggregate nav metadata', async () => {
    renderPage();
    await waitForPageReady();

    emitLatestProjection({ slug: 'row-live-2', conversationId: 'conv-live-2', conversation: { work_scope_key: 'ws-latest' } });

    await waitFor(() => {
      const props = conversationNavStackSpy.mock.lastCall?.[0] as Record<string, unknown>;
      expect(props['slug']).toBe('row-live-2');
      expect(props['conversationId']).toBe('conv-live-2');
      expect(props['workScopeKey']).toBe('ws-latest');
    });
  });

  it('keeps history snapshots read-only while retaining lineage recall', async () => {
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

  it('keeps aggregate transcript viewer actions reachable while the aggregate route owns placement', async () => {
    renderPage();
    await waitForPageReady();
    expect(screen.getByTestId('message-sidepanel-enabled')).toHaveTextContent('true');
    expect(screen.getByTestId('message-fullscreen-enabled')).toHaveTextContent('true');
  });

  it('wires aggregate-route message-viewer events through reachable transcript actions', async () => {
    renderPage();
    await waitForPageReady();
    fireEvent.click(screen.getByRole('button', { name: 'open aggregate viewer' }));
    expect(screen.getByTestId('message-sidepanel-enabled')).toHaveTextContent('true');
    expect(screen.getByTestId('message-fullscreen-enabled')).toHaveTextContent('true');
  });

  it('enables the live runtime for an open aggregate despite stale writable-row metadata', async () => {
    const { api } = await import('../api');
    vi.mocked(api.getProductConversationSnapshot).mockResolvedValueOnce(makeSnapshot({
      writable_transcript_row_id: null,
      ordinary_lifecycle: 'open',
    }));

    renderPage();
    await waitForPageReady();

    expect(embeddedConversationPageSpy.mock.lastCall?.[0]?.['ordinaryComposerEnabled']).toBe(true);
  });

  it('retries older-page fetch once with a fresh tail cursor after a stale 400 cursor error', async () => {
    const { api } = await import('../api');
    vi.mocked(api.getProductConversationSnapshot)
      .mockResolvedValueOnce(makeSnapshot())
      .mockRejectedValueOnce(new ApiResponseError('stale cursor', 400))
      .mockResolvedValueOnce(makeSnapshot({ before: 'cursor-2', has_older: true }))
      .mockResolvedValueOnce(makeSnapshot({
        segments: [{ segment_ordinal: 0, transcript_row_id: 'row-old', slug: 'old', title: 'Old', messages: [makeMessage('old-message', 0)], handoff: null }],
        before: null,
        has_older: false,
      }));

    renderPage();
    await waitForPageReady();
    fireEvent.click(screen.getByRole('button', { name: 'load older with anchor' }));

    await waitFor(() => expect(screen.getByTestId('message-order')).toHaveTextContent('old-message,m-1,m-2,product-handoff:pc-1:row-1:cont-1,m-3,m-4'));
    expect(vi.mocked(api.getProductConversationSnapshot).mock.calls.slice(1, 4)).toEqual([
      ['pc-1', { message_limit: 100, before: 'cursor-1' }],
      ['pc-1', { message_limit: 100 }],
      ['pc-1', { message_limit: 100, before: 'cursor-2' }],
    ]);
    expect(conversationNavStackSpy.mock.lastCall?.[0]?.['transcriptPositioning']).toEqual(expect.objectContaining({
      kind: 'positioning',
      command: expect.objectContaining({
        kind: 'restore_after_prefix_expansion',
        messageId: 'm-3',
        viewportStartOffset: 17,
      }),
    }));
  });

  it('discards delayed older-page results after a new product route wins', async () => {
    const { api } = await import('../api');
    let resolveOlder!: (value: ProductConversationSnapshotView) => void;
    const older = new Promise<ProductConversationSnapshotView>((resolve) => { resolveOlder = resolve; });
    vi.mocked(api.getProductConversationSnapshot)
      .mockResolvedValueOnce(makeSnapshot())
      .mockReturnValueOnce(older)
      .mockResolvedValueOnce(makeSnapshot({
        product_conversation_id: 'pc-2',
        canonical_route: '/product-conversations/pc-2',
        presentation: { kind: 'state', display_name: 'Product Beta', presentation_mode: 'idle' },
        segments: [{ segment_ordinal: 1, transcript_row_id: 'row-b', slug: 'row-b', title: 'Beta', messages: [makeMessage('beta-message', 1, 'beta')], handoff: null }],
        latest_transcript_row_id: 'row-b',
        writable_transcript_row_id: 'row-b',
        before: null,
        has_older: false,
      }));

    renderPage('/product-conversations/pc-1', true);
    await waitForPageReady();
    fireEvent.click(screen.getByRole('button', { name: 'load older' }));
    fireEvent.click(screen.getByRole('button', { name: 'open second product' }));
    await screen.findByRole('heading', { name: 'Product Beta' });
    await act(async () => resolveOlder(makeSnapshot({ segments: [{ segment_ordinal: 0, transcript_row_id: 'row-old', slug: 'old', title: 'Old A', messages: [makeMessage('stale-a', 1)], handoff: null }] })));

    expect(screen.getByRole('heading', { name: 'Product Beta' })).toBeInTheDocument();
    expect(screen.getByTestId('message-order')).toHaveTextContent('beta-message');
    expect(screen.getByTestId('message-order')).not.toHaveTextContent('stale-a');
  });

  it('discards delayed pagination errors and cleanup after the route owner changes', async () => {
    const { api } = await import('../api');
    let rejectOlder!: (reason: Error) => void;
    const older = new Promise<ProductConversationSnapshotView>((_resolve, reject) => { rejectOlder = reject; });
    vi.mocked(api.getProductConversationSnapshot)
      .mockResolvedValueOnce(makeSnapshot())
      .mockReturnValueOnce(older)
      .mockResolvedValueOnce(makeSnapshot({
        product_conversation_id: 'pc-2',
        canonical_route: '/product-conversations/pc-2',
        presentation: { kind: 'state', display_name: 'Product Beta', presentation_mode: 'idle' },
        segments: [{ segment_ordinal: 1, transcript_row_id: 'row-b', slug: 'row-b', title: 'Beta', messages: [makeMessage('beta-message', 1, 'beta')], handoff: null }],
        latest_transcript_row_id: 'row-b',
        writable_transcript_row_id: 'row-b',
        before: null,
        has_older: false,
      }));

    renderPage('/product-conversations/pc-1', true);
    await waitForPageReady();
    fireEvent.click(screen.getByRole('button', { name: 'load older' }));
    fireEvent.click(screen.getByRole('button', { name: 'open second product' }));
    await screen.findByRole('heading', { name: 'Product Beta' });
    await act(async () => rejectOlder(new Error('stale A failure')));

    expect(screen.queryByText('stale A failure')).not.toBeInTheDocument();
    expect(conversationNavStackSpy.mock.lastCall?.[0]?.['loadingOlderMessages']).toBe(false);
  });

  it('threads restore basis, root directory, system prompt, and explicit absent work scope', async () => {
    const { api } = await import('../api');
    vi.mocked(api.getProductConversationSnapshot)
      .mockResolvedValueOnce(makeSnapshot({ work_identity: null }))
      .mockResolvedValueOnce(makeSnapshot({
        segments: [{ segment_ordinal: 0, transcript_row_id: 'row-old', slug: 'old', title: 'Old', messages: [makeMessage('old-message', 0)], handoff: null }],
        before: null,
        has_older: false,
      }));
    vi.mocked(api.getChain).mockResolvedValueOnce(makeChain({ work_identity: null }));
    renderPage();
    await waitForPageReady();
    emitLatestProjection();
    fireEvent.click(screen.getByRole('button', { name: 'load older with anchor' }));
    await waitFor(() => expect(conversationNavStackSpy.mock.lastCall?.[0]?.['transcriptPositioning']).toEqual(expect.objectContaining({
      kind: 'positioning',
      command: expect.objectContaining({
        kind: 'restore_after_prefix_expansion',
        messageId: 'm-3',
        viewportStartOffset: 17,
        view: expect.objectContaining({ generation: 1, transcriptGeneration: 1 }),
      }),
    })));
    expect(conversationNavStackSpy.mock.lastCall?.[0]?.['filePathRootDir']).toBe('/tmp/latest-root');
    expect(conversationNavStackSpy.mock.lastCall?.[0]?.['systemPrompt']).toBe('Preserve this system prompt');
    expect(screen.getByText('No managed work scope')).toBeInTheDocument();
  });

  it('keeps successful pagination visually silent and ignores malformed message hashes', async () => {
    renderPage('/product-conversations/pc-1#message-%');
    await waitForPageReady();
    expect(screen.queryByText(/Earlier history available|Complete snapshot loaded|Loading earlier history/)).not.toBeInTheDocument();
    expect(screen.getByRole('heading', { name: 'Product Alpha' })).toBeInTheDocument();
  });

  it('persists recall drafts by product conversation identity across unmount', async () => {
    const first = renderPage();
    await waitForPageReady();
    fireEvent.change(screen.getByLabelText('recall draft'), { target: { value: 'durable aggregate draft' } });
    first.unmount();
    expect(localStorage.getItem('phoenix:product-conversation-draft:pc-1')).toBe('durable aggregate draft');

    renderPage();
    await waitFor(() => expect(screen.getByLabelText('recall draft')).toHaveValue('durable aggregate draft'));
    expect(localStorage.getItem('phoenix:chain-draft:root-chain')).toBeNull();
  });

  it('subscribes to chain streaming with compatibility root id', async () => {
    const { api } = await import('../api');
    vi.mocked(api.getProductConversationSnapshot).mockResolvedValueOnce(makeSnapshot());

    renderPage();

    await waitForPageReady();
    expect(subscribeToChainStreamMock).toHaveBeenCalledWith('root-chain', expect.any(Function), expect.any(Function));
  });
});
