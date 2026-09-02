import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter, Route, Routes, useNavigate } from 'react-router-dom';
import { ProductConversationPage } from './ProductConversationPage';
import { ConversationReadinessProvider } from '../contexts/ConversationReadinessContext';
import { FileExplorerProvider } from '../components/FileExplorer';
import { ViewerSlotProvider } from '../contexts/ViewerSlotContext';
import { ChainProvider } from '../chain';
import { ApiResponseError, type ChainView, type ProductConversationSnapshotView } from '../api';
import type { ProductConversationCloseView } from '../generated/ProductConversationCloseView';
import { notifyCloseSnapshotChanged } from '../notifications';

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
        <button onClick={() => (props['onHistoryScrollCommandHandled'] as ((token: number, result: string, view: unknown) => void) | undefined)?.(
          ((props['transcriptPositioning'] as { command?: { token?: number } } | undefined)?.command?.token ?? 0),
          'applied',
          {},
        )}>
          complete positioning
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
          disabled={Boolean(props['disabled'])}
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

vi.mock('../components/TaskApprovalReader', () => ({
  TaskApprovalReader: (props: Record<string, unknown>) => (
    <div data-testid="aggregate-task-approval-owner">
      <div data-testid="aggregate-task-approval-title">{String(props['title'])}</div>
      <div data-testid="aggregate-task-approval-copy">Continue here|Start in new conversation</div>
      <div data-testid="aggregate-task-approval-handlers">
        {String(typeof props['onApprove'] === 'function')}|{String(typeof props['onReject'] === 'function')}|{String(typeof props['onSendFeedback'] === 'function')}
      </div>
      <div data-testid="aggregate-task-approval-mutation-enabled">{String(props['mutationEnabled'])}</div>
      <button disabled={props['mutationEnabled'] === false} onClick={() => (props['onApprove'] as ((handoff: string) => void))('continue')}>approve task</button>
    </div>
  ),
}));

vi.mock('../components/FirstTaskWelcome', () => ({
  FirstTaskWelcome: () => <div data-testid="first-task-welcome">first task welcome</div>,
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
      approveTask: vi.fn(),
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
    close: null,
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
    serverArchived: false,
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

  it('refetches the authoritative aggregate when SSE changes member archive state', async () => {
    const { api } = await import('../api');
    vi.mocked(api.getProductConversationSnapshot)
      .mockResolvedValueOnce(makeSnapshot({ ordinary_lifecycle: 'open' }))
      .mockResolvedValueOnce(makeSnapshot({ ordinary_lifecycle: 'history' }));

    renderPage();
    await waitForPageReady();

    act(() => emitLatestProjection({ isArchived: false, serverArchived: false }));
    expect(api.getProductConversationSnapshot).toHaveBeenCalledTimes(1);
    act(() => emitLatestProjection({ isArchived: false, serverArchived: true }));

    await waitFor(() => expect(api.getProductConversationSnapshot).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(screen.getByText('History is read-only.')).toBeInTheDocument());
    expect(api.getProductConversationSnapshot).toHaveBeenCalledTimes(2);
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

  it('omits boundary links for unloaded segment messages', async () => {
    const snapshot = makeSnapshot();
    snapshot.segments[0] = { ...snapshot.segments[0]!, messages: [] };
    const { api } = await import('../api');
    vi.mocked(api.getProductConversationSnapshot).mockResolvedValueOnce(snapshot);

    renderPage();
    await waitForPageReady();

    expect(screen.getAllByRole('link', { name: 'boundary' })).toHaveLength(1);
    expect(document.querySelector('a[href$="#message-"]')).not.toBeInTheDocument();
  });

  it('assigns occurrence identity to newly streamed latest messages', async () => {
    renderPage();
    await waitForPageReady();
    emitLatestProjection({ messages: [makeMessage('new-live', 7, 'conv-latest')] });

    await waitFor(() => {
      const props = conversationNavStackSpy.mock.lastCall?.[0] as { messages: Array<{ message_id: string; display_data?: { productOccurrenceToken?: string } }> };
      expect(props.messages.find((message) => message.message_id === 'new-live')?.display_data?.productOccurrenceToken)
        .toBe('row-2:new-live');
    });
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
    expect(chain.current_total_messages).toBe(6);
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
      mutationEnabled: true,
    }));
    expect(screen.getByTestId('chain-work-scope-dock')).toBeInTheDocument();
    expect(chainWorkScopeDockSpy).toHaveBeenLastCalledWith(expect.objectContaining({
      activeConvId: 'row-2',
    }));
  });

  it('shows first-task onboarding after aggregate approval returns first_task', async () => {
    const { api } = await import('../api');
    vi.mocked(api.approveTask).mockResolvedValueOnce({ success: true, first_task: true });
    renderPage();
    await waitForPageReady();

    emitLatestProjection({
      convState: { type: 'awaiting_task_approval', title: 'Plan', priority: 'p1', plan: 'Do it' },
      modelContextWindow: 200_000,
    });
    fireEvent.click(await screen.findByRole('button', { name: 'approve task' }));

    expect(await screen.findByTestId('first-task-welcome')).toBeInTheDocument();
  });

  it('keeps task approval ownership on the aggregate route while suppressing the embedded transcript owner', async () => {
    renderPage();
    await waitForPageReady();

    emitLatestProjection({
      convState: { type: 'awaiting_task_approval', title: 'Plan', priority: 'p1', plan: 'Do it' },
      modelContextWindow: 200_000,
    });

    expect(await screen.findByTestId('aggregate-task-approval-owner')).toBeInTheDocument();
    expect(screen.getByTestId('aggregate-task-approval-title')).toHaveTextContent('Plan');
    expect(screen.getByTestId('aggregate-task-approval-copy')).toHaveTextContent('Continue here|Start in new conversation');
    expect(embeddedConversationPageSpy).toHaveBeenLastCalledWith(expect.objectContaining({
      suppressCanonicalization: true,
      suppressMessageViewerOwner: true,
      suppressTaskApprovalOwner: true,
      showTranscript: false,
    }));
  });

  it('preserves embedded task approval ownership on degraded fallback routes', async () => {
    const { api } = await import('../api');
    vi.mocked(api.getProductConversationSnapshot).mockRejectedValueOnce(new Error('snapshot failed'));

    renderPage();

    expect(await screen.findByRole('alert')).toHaveTextContent('snapshot failed');
    expect(embeddedConversationPageSpy.mock.calls[0]?.[0]).toEqual(expect.objectContaining({
      slug: 'pc-1',
      routePrefix: '/c',
      suppressCanonicalization: true,
    }));
    expect(embeddedConversationPageSpy.mock.lastCall?.[0]?.['suppressTaskApprovalOwner']).toBeUndefined();
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
      mutationEnabled: true,
      suppressCanonicalization: true,
    }));
  });

  it('re-enables the ordinary composer from live latest projection after approval ends', async () => {
    renderPage();
    await waitForPageReady();

    emitLatestProjection({ convState: { type: 'awaiting_task_approval' } });
    await waitFor(() => {
      expect(embeddedConversationPageSpy).toHaveBeenLastCalledWith(expect.objectContaining({
        mutationEnabled: true,
      }));
    });

    emitLatestProjection({ convState: { type: 'idle' } });
    await waitFor(() => {
      expect(embeddedConversationPageSpy).toHaveBeenLastCalledWith(expect.objectContaining({
        mutationEnabled: true,
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

  it('disables live transcript and recall controls while Close is active', async () => {
    const { api } = await import('../api');
    vi.mocked(api.getProductConversationSnapshot).mockResolvedValueOnce(makeSnapshot({
      close: {
        attempt_id: 'close-active',
        phase: 'settling_active_work',
        confirmation_snapshot: null,
        inspections: [],
        losses: [],
        residuals: [],
      },
    }));

    renderPage();
    await waitForPageReady();

    expect(embeddedConversationPageSpy.mock.lastCall?.[0]?.['mutationEnabled']).toBe(false);
    expect(screen.getByRole('textbox', { name: 'recall draft' })).toBeDisabled();
    expect(chainQaColumnSpy.mock.lastCall?.[0]?.['onSubmit']).toBeUndefined();
    expect(chainQaColumnSpy.mock.lastCall?.[0]?.['disabled']).toBe(true);
  });

  it('background-refreshes active Close phases without replacing the page with a skeleton', async () => {
    const { api } = await import('../api');
    const activeClose = {
      attempt_id: 'close-active',
      phase: 'settling_active_work',
      confirmation_snapshot: null,
      inspections: [],
      losses: [],
      residuals: [],
    } satisfies ProductConversationCloseView;
    let resolveRefresh!: (value: ProductConversationSnapshotView) => void;
    const refresh = new Promise<ProductConversationSnapshotView>((resolve) => { resolveRefresh = resolve; });
    vi.mocked(api.getProductConversationSnapshot)
      .mockResolvedValueOnce(makeSnapshot({ close: activeClose }))
      .mockReturnValueOnce(refresh);

    renderPage();
    await waitForPageReady();
    act(() => notifyCloseSnapshotChanged('row-2', 'stream'));

    await waitFor(() => expect(api.getProductConversationSnapshot).toHaveBeenCalledTimes(2));
    expect(screen.queryByTestId('skeleton')).not.toBeInTheDocument();
    expect(screen.getByRole('heading', { name: 'Product Alpha' })).toBeInTheDocument();
    await act(async () => resolveRefresh(makeSnapshot({
      close: { ...activeClose, phase: 'awaiting_retirement_inspection' },
    })));
    expect(screen.getByRole('heading', { name: 'Product Alpha' })).toBeInTheDocument();
  });

  it('preserves loaded history across active Close refreshes', async () => {
    const { api } = await import('../api');
    const activeClose = {
      attempt_id: 'close-active',
      phase: 'settling_active_work',
      confirmation_snapshot: null,
      inspections: [],
      losses: [],
      residuals: [],
    } satisfies ProductConversationCloseView;
    vi.mocked(api.getProductConversationSnapshot)
      .mockResolvedValueOnce(makeSnapshot({ close: activeClose }))
      .mockResolvedValueOnce(makeSnapshot({
        close: activeClose,
        segments: [{ segment_ordinal: 0, transcript_row_id: 'row-old', slug: 'old', title: 'Old', messages: [makeMessage('old-message', 0)], handoff: null }],
        before: null,
        has_older: false,
      }))
      .mockResolvedValueOnce(makeSnapshot({
        close: { ...activeClose, phase: 'awaiting_retirement_inspection' },
        segments: [
          {
            segment_ordinal: 1,
            transcript_row_id: 'row-1',
            slug: 'row-1',
            title: 'First',
            messages: [makeMessage('m-2', 2)],
            handoff: {
              kind: 'historical',
              predecessor_transcript_row_id: 'row-1',
              successor_transcript_row_id: 'row-2',
              continuation_message_id: 'cont-1',
              summary: 'carry context',
            },
          },
          {
            segment_ordinal: 2,
            transcript_row_id: 'row-2',
            slug: 'row-2',
            title: 'Second',
            messages: [makeMessage('m-3', 3), makeMessage('m-4', 4)],
            handoff: null,
          },
        ],
      }));

    renderPage();
    await waitForPageReady();
    fireEvent.click(screen.getByRole('button', { name: 'load older' }));
    await waitFor(() => expect(screen.getByTestId('message-order')).toHaveTextContent('old-message'));

    act(() => notifyCloseSnapshotChanged('row-2', 'stream'));

    await waitFor(() => expect(api.getProductConversationSnapshot).toHaveBeenCalledTimes(3));
    await waitFor(() => expect(screen.getByTestId('message-order')).toHaveTextContent('old-message'));
    expect(screen.getByTestId('message-order')).toHaveTextContent('old-message,m-1,m-2,product-handoff:pc-1:row-1:cont-1,m-3,m-4');
    expect(screen.getByTestId('message-order').textContent?.split(',').filter((id) => id === 'old-message')).toHaveLength(1);
    expect(conversationNavStackSpy.mock.lastCall?.[0]?.['transcriptPositioning']).toEqual(expect.objectContaining({ kind: 'idle' }));
    expect(embeddedConversationPageSpy.mock.lastCall?.[0]?.['mutationEnabled']).toBe(false);
  });

  it('does not retain a removed segment inside the authoritative refreshed tail range', async () => {
    const { api } = await import('../api');
    const activeClose = {
      attempt_id: 'close-active',
      phase: 'settling_active_work',
      confirmation_snapshot: null,
      inspections: [],
      losses: [],
      residuals: [],
    } satisfies ProductConversationCloseView;
    vi.mocked(api.getProductConversationSnapshot)
      .mockResolvedValueOnce(makeSnapshot({
        close: activeClose,
        latest_transcript_row_id: 'row-stale',
        writable_transcript_row_id: 'row-stale',
        segments: [
          {
            segment_ordinal: 2,
            transcript_row_id: 'row-2',
            slug: 'row-2',
            title: 'Second',
            messages: [makeMessage('m-3', 3), makeMessage('m-4', 4)],
            handoff: null,
          },
          {
            segment_ordinal: 3,
            transcript_row_id: 'row-stale',
            slug: 'row-stale',
            title: 'Removed',
            messages: [makeMessage('stale-message', 5)],
            handoff: null,
          },
        ],
      }))
      .mockResolvedValueOnce(makeSnapshot({
        close: { ...activeClose, phase: 'awaiting_retirement_inspection' },
        segments: [{
          segment_ordinal: 2,
          transcript_row_id: 'row-2',
          slug: 'row-2',
          title: 'Second',
          messages: [makeMessage('m-3', 3), makeMessage('m-4', 4)],
          handoff: null,
        }],
      }));
    renderPage();
    await waitForPageReady();

    act(() => notifyCloseSnapshotChanged('pc-1', 'stream'));

    await waitFor(() => expect(api.getProductConversationSnapshot).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(screen.getByTestId('message-order')).not.toHaveTextContent('stale-message'));
    expect(screen.getByTestId('message-order')).toHaveTextContent('m-3,m-4');
  });

  it('does not preserve stale cached segments when the refreshed tail is empty', async () => {
    const { api } = await import('../api');
    const activeClose = {
      attempt_id: 'close-active',
      phase: 'settling_active_work',
      confirmation_snapshot: null,
      inspections: [],
      losses: [],
      residuals: [],
    } satisfies ProductConversationCloseView;
    vi.mocked(api.getProductConversationSnapshot)
      .mockResolvedValueOnce(makeSnapshot({ close: activeClose }))
      .mockResolvedValueOnce(makeSnapshot({
        close: { ...activeClose, phase: 'awaiting_retirement_inspection' },
        segments: [],
      }));
    renderPage();
    await waitForPageReady();

    act(() => notifyCloseSnapshotChanged('pc-1', 'stream'));

    await waitFor(() => expect(api.getProductConversationSnapshot).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(screen.getByTestId('message-count')).toHaveTextContent('0'));
  });

  it('adopts a refreshed pagination boundary when no loaded prefix was retained', async () => {
    const { api } = await import('../api');
    const activeClose = {
      attempt_id: 'close-active',
      phase: 'settling_active_work',
      confirmation_snapshot: null,
      inspections: [],
      losses: [],
      residuals: [],
    } satisfies ProductConversationCloseView;
    vi.mocked(api.getProductConversationSnapshot)
      .mockResolvedValueOnce(makeSnapshot({ close: activeClose, before: null, has_older: false }))
      .mockResolvedValueOnce(makeSnapshot({
        close: { ...activeClose, phase: 'awaiting_retirement_inspection' },
        before: 'm-3',
        has_older: true,
      }));
    renderPage();
    await waitForPageReady();
    expect(conversationNavStackSpy.mock.lastCall?.[0]?.['hasOlderMessages']).toBe(false);

    act(() => notifyCloseSnapshotChanged('pc-1', 'stream'));

    await waitFor(() => expect(api.getProductConversationSnapshot).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(conversationNavStackSpy.mock.lastCall?.[0]?.['hasOlderMessages']).toBe(true));
  });

  it('does not refetch an Open snapshot for ordinary state invalidations', async () => {
    const { api } = await import('../api');
    renderPage();
    await waitForPageReady();

    act(() => notifyCloseSnapshotChanged('row-2', 'stream'));
    await act(async () => Promise.resolve());

    expect(api.getProductConversationSnapshot).toHaveBeenCalledTimes(1);
    expect(screen.queryByTestId('skeleton')).not.toBeInTheDocument();
  });

  it('discovers an explicitly-started Close while the loaded snapshot is Open', async () => {
    const { api } = await import('../api');
    vi.mocked(api.getProductConversationSnapshot)
      .mockResolvedValueOnce(makeSnapshot())
      .mockResolvedValueOnce(makeSnapshot({
        close: {
          attempt_id: 'close-started-elsewhere',
          phase: 'settling_active_work',
          confirmation_snapshot: null,
          inspections: [],
          losses: [],
          residuals: [],
        },
      }));
    renderPage();
    await waitForPageReady();

    act(() => notifyCloseSnapshotChanged('row-2'));

    await waitFor(() => expect(api.getProductConversationSnapshot).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(embeddedConversationPageSpy.mock.lastCall?.[0]?.['mutationEnabled']).toBe(false));
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
    expect(screen.getAllByText('History').length).toBeGreaterThan(0);
    expect(screen.queryByText('history')).not.toBeInTheDocument();
    expect(screen.getByRole('textbox', { name: 'recall draft' })).toBeDisabled();
    expect(chainQaColumnSpy.mock.lastCall?.[0]?.['disabled']).toBe(true);
    expect(chainQaColumnSpy.mock.lastCall?.[0]?.['onSubmit']).toBeUndefined();
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

    expect(embeddedConversationPageSpy.mock.lastCall?.[0]?.['mutationEnabled']).toBe(true);
  });

  it('keeps an Open aggregate mutable after archived drift triggers a failed background refresh', async () => {
    const { api } = await import('../api');
    vi.mocked(api.getProductConversationSnapshot)
      .mockResolvedValueOnce(makeSnapshot({ ordinary_lifecycle: 'open' }))
      .mockRejectedValueOnce(new Error('refresh failed'));
    renderPage();
    await waitForPageReady();

    act(() => emitLatestProjection({ serverArchived: true }));

    expect(await screen.findByRole('alert')).toHaveTextContent('refresh failed');
    expect(api.getProductConversationSnapshot).toHaveBeenCalledTimes(2);
    expect(screen.getByRole('heading', { name: 'Product Alpha' })).toBeInTheDocument();
    expect(screen.queryByText('Showing cached row if available.')).not.toBeInTheDocument();
    expect(embeddedConversationPageSpy.mock.lastCall?.[0]?.['mutationEnabled']).toBe(true);
    expect(embeddedConversationPageSpy.mock.lastCall?.[0]?.['aggregateLifecycleOpen']).toBe(true);
  });

  it('keeps a History aggregate read-only after a failed background refresh despite unarchived drift', async () => {
    const { api } = await import('../api');
    vi.mocked(api.getProductConversationSnapshot)
      .mockResolvedValueOnce(makeSnapshot({ ordinary_lifecycle: 'history' }))
      .mockRejectedValueOnce(new Error('refresh failed'));
    renderPage();
    await waitForPageReady();

    act(() => notifyCloseSnapshotChanged('row-2'));

    expect(await screen.findByRole('alert')).toHaveTextContent('refresh failed');
    expect(api.getProductConversationSnapshot).toHaveBeenCalledTimes(2);
    expect(screen.getByRole('heading', { name: 'Product Alpha' })).toBeInTheDocument();
    expect(screen.getByText('History is read-only.')).toBeInTheDocument();
    expect(screen.queryByTestId('embedded-conversation-page')).not.toBeInTheDocument();
    expect(screen.queryByText('Showing cached row if available.')).not.toBeInTheDocument();
  });
  it('gates task approval mutations behind liveControlsEnabled', async () => {
    const { api } = await import('../api');
    vi.mocked(api.getProductConversationSnapshot).mockResolvedValueOnce(makeSnapshot({
      close: {
        attempt_id: 'attempt-1',
        phase: 'settling_active_work',
        inspections: [],
        losses: [],
        residuals: [],
        confirmation_snapshot: null,
      },
    }));

    renderPage();
    await waitForPageReady();

    expect(screen.queryByTestId('aggregate-task-approval-owner')).not.toBeInTheDocument();
    expect(vi.mocked(api.approveTask)).not.toHaveBeenCalled();
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
    fireEvent.click(screen.getByRole('button', { name: 'complete positioning' }));
    await waitFor(() => {
      expect(conversationNavStackSpy.mock.lastCall?.[0]?.['transcriptPositioning']).toEqual(expect.objectContaining({ kind: 'idle' }));
    });
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

  it('ignores an old stream Close invalidation after a new product route wins', async () => {
    const { api } = await import('../api');
    const activeClose = {
      attempt_id: 'close-active',
      phase: 'settling_active_work',
      confirmation_snapshot: null,
      inspections: [],
      losses: [],
      residuals: [],
    } satisfies ProductConversationCloseView;
    vi.mocked(api.getProductConversationSnapshot)
      .mockResolvedValueOnce(makeSnapshot({ close: activeClose }))
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
    fireEvent.click(screen.getByRole('button', { name: 'open second product' }));
    await screen.findByRole('heading', { name: 'Product Beta' });
    act(() => notifyCloseSnapshotChanged('row-2', 'stream'));
    await act(async () => Promise.resolve());

    expect(api.getProductConversationSnapshot).toHaveBeenCalledTimes(2);
    expect(screen.getByRole('heading', { name: 'Product Beta' })).toBeInTheDocument();
    expect(screen.getByTestId('message-order')).toHaveTextContent('beta-message');
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
